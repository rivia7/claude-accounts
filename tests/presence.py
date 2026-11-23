#!/usr/bin/env python3
"""连接设备（presence）：Team 的同步服务器的验签、名单、状态；客户端的上报、cca devices、共享记下的 API key、折合金额的同步。
真的 ssh-keygen、真的 TLS 服务器（127.0.0.1，随机端口）；客户端是 cca 的辅助程序（cca-core，各台设备各一个临时 HOME），不联网。"""
import base64
import fcntl
import hashlib
import http.client
import importlib.util
import io
import json
import os
from pathlib import Path
import pty
import re
import secrets
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
SERVER = ROOT / 'server' / 'cca-presence.py'
sys.path.insert(0, str(Path(__file__).resolve().parent))
from helper_bin import HELPER   # noqa: E402

sys.dont_write_bytecode = True   # 不在 server/ 里留下 __pycache__
spec = importlib.util.spec_from_file_location('cca_presence_server', SERVER)
srvmod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(srvmod)
import spec_wire as sw   # noqa: E402  照 server/README.md 的格式独立实现的密文（不用 cca 的代码）


def keygen(path, comment='test'):
    subprocess.run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-C', comment, '-f', str(path)], check=True,
                   stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return ' '.join(Path(str(path) + '.pub').read_text().split()[:2])


def sign(key, ns, data):
    return subprocess.run(['ssh-keygen', '-Y', 'sign', '-q', '-n', ns, '-f', str(key)], input=data,
                          stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=True).stdout


def make_cert(d):
    subprocess.run(['openssl', 'ecparam', '-name', 'prime256v1', '-genkey', '-noout', '-out', str(d / 'key.pem')],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    subprocess.run(['openssl', 'req', '-new', '-x509', '-key', str(d / 'key.pem'), '-subj', '/CN=cca-presence', '-days', '2',
                    '-out', str(d / 'cert.pem')], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def make_ca_cert(d):
    """证书机构签的证书（SAN 是 127.0.0.1）：照证书机构验的服务器（托管平台上的 Team 的服务器、网页端）用；设备的 curl 经
    CURL_CA_BUNDLE=ca.pem 认这个机构"""
    def run(*a):
        subprocess.run(['openssl'] + [str(x) for x in a], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    run('ecparam', '-name', 'prime256v1', '-genkey', '-noout', '-out', d / 'ca.key')
    run('req', '-new', '-x509', '-key', d / 'ca.key', '-subj', '/CN=cca-test-ca', '-days', '2', '-out', d / 'ca.pem')
    run('ecparam', '-name', 'prime256v1', '-genkey', '-noout', '-out', d / 'key.pem')
    run('req', '-new', '-key', d / 'key.pem', '-subj', '/CN=127.0.0.1', '-out', d / 'csr.pem')
    (d / 'ext.cnf').write_text('subjectAltName=IP:127.0.0.1\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n')
    run('x509', '-req', '-in', d / 'csr.pem', '-CA', d / 'ca.pem', '-CAkey', d / 'ca.key', '-CAcreateserial', '-days', '2',
        '-extfile', d / 'ext.cnf', '-out', d / 'cert.pem')


def openssl_pin(cert):
    """管理员那边（安装脚本、文档）算公钥指纹的办法：和服务器的 --print-pin 必须一样"""
    pub = subprocess.run(['openssl', 'x509', '-in', str(cert), '-pubkey', '-noout'], stdout=subprocess.PIPE, check=True).stdout
    der = subprocess.run(['openssl', 'pkey', '-pubin', '-outform', 'der'], input=pub, stdout=subprocess.PIPE, check=True).stdout
    import hashlib
    return 'sha256//' + base64.b64encode(hashlib.sha256(der).digest()).decode()


class Server:
    """跑一个真的 Team 的同步服务器进程（ttl 2 秒、闲置 3 秒，测试不用干等）。team_root：Team 的根公钥（必须给；不写 --mode：
    默认就是 team）；proxied：--behind-proxy（只说 HTTP，不用证书）；port：监听这个端口（重启后设备照旧连得上），0 是随便挑一个"""
    def __init__(self, d, ttl='2', idle='3', proxied=False, port=0, team_root=None):
        assert team_root, '服务器只有 Team 的一种：要给根公钥'
        self.d, self.proxied = d, proxied
        tls = ['--behind-proxy'] if proxied else ['--cert', str(d / 'cert.pem'), '--key', str(d / 'key.pem')]
        args = [sys.executable, '-I', str(SERVER), '--listen', '127.0.0.1:%d' % port, '--port-file', str(d / 'port')] + tls + [
                '--team-root', team_root, '--state', str(d / 'state.json'), '--ttl', ttl, '--idle', idle]
        (d / 'port').unlink() if (d / 'port').exists() else None
        self.proc = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        for _ in range(100):
            if (d / 'port').exists():
                break
            time.sleep(0.05)
        self.port = int((d / 'port').read_text())

    def stop(self):
        """停掉，返回它写的日志（停过了再停是空的：addCleanup 里的那一次）"""
        if self.proc.stderr.closed:
            return ''
        self.proc.terminate()
        self.proc.wait(5)
        with self.proc.stderr:
            return self.proc.stderr.read().decode()

    def req(self, method, path, body=b'', sig=None, headers=None):
        if self.proxied:
            c = http.client.HTTPConnection('127.0.0.1', self.port, timeout=10)
        else:
            ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
            ctx.check_hostname = False
            ctx.verify_mode = ssl.CERT_NONE   # 测试里不验 CA：客户端靠的是公钥钉住（这里另外核对）
            c = http.client.HTTPSConnection('127.0.0.1', self.port, context=ctx, timeout=10)
        headers = dict({'Content-Type': 'application/json'}, **(headers or {}))
        if sig is not None:
            headers['X-Cca-Sig'] = base64.b64encode(sig).decode()
        try:
            c.request(method, path, body=body, headers=headers)
            r = c.getresponse()
            data = r.read()
        except (BrokenPipeError, ConnectionResetError):   # 服务器没读完就回绝、断开（body 太大）
            return None, {}
        finally:
            c.close()
        return r.status, json.loads(data or b'{}')


class ServerTest(unittest.TestCase):
    """设备的请求（上报、查看、记下的 API key、折合金额）：名单由管理员拿根密钥登记、移除（team_enroll、team_revoke），
    Team 的那几个 op 本身在 TeamServerTest"""
    DEV, DEV2 = '0a' * 8, '0b' * 8   # 两台设备在 Team 里的编号

    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='cca-presence-test-')
        d = cls.d = Path(cls.tmp.name)
        make_cert(d)
        cls.rootpub = keygen(d / 'root', 'test-root')
        cls.devpub, cls.dev2pub, cls.evilpub = keygen(d / 'dev'), keygen(d / 'dev2'), keygen(d / 'evil')
        cls.srv = Server(d, team_root=cls.rootpub)

    @classmethod
    def tearDownClass(cls):
        cls.srv.stop()
        cls.tmp.cleanup()

    @classmethod
    def restart(cls):
        """重起服务器（状态都在文件里，接着用）：限流的配额从头算"""
        cls.srv.stop()
        cls.srv = Server(cls.d, team_root=cls.rootpub)

    @staticmethod
    def fp(pub):
        return srvmod.key_fp(pub.split()[1])

    def call(self, op, key='dev', ns='cca-presence', pub=None, tamper=False, **extra):
        body = dict({'v': 1, 'op': op, 'key': self.fp(pub or getattr(self, key + 'pub')), 'ts': time.time(),
                     'nonce': secrets.token_hex(16)}, **extra)
        raw = json.dumps(body, separators=(',', ':')).encode()
        sig = sign(self.d / key, ns, raw)
        if tamper:
            raw = raw.replace(b'"v":1', b'"v":1 ')
        return self.srv.req('POST', '/v1/' + op, raw, sig)

    def enroll(self, **devs):
        """管理员登记这几台设备（{编号: (公钥, 名字)}）"""
        st, j = self.call('team_enroll', key='root', devices={dev: {'pub': pub, 'name': name} for dev, (pub, name) in devs.items()})
        self.assertEqual(st, 200, j)
        return j

    def both(self):
        return self.enroll(**{self.DEV: (self.devpub, 'alice-mbp'), self.DEV2: (self.dev2pub, 'bob-mini')})

    def revoke(self, dev):
        st, j = self.call('team_revoke', key='root', dev=dev)
        self.assertEqual(st, 200, j)

    def view(self, aid, key='dev2'):
        st, j = self.call('view', key=key, accounts=[aid])
        self.assertEqual(st, 200, j)
        return {r['name']: r for r in j['accounts'][aid]}, j

    def test_health_and_self_test(self):
        st, j = self.srv.req('GET', '/v1/health')
        self.assertEqual(st, 200)
        self.assertTrue(j['ok'] and j['sig_ok'])
        self.assertEqual((j['mode'], j['seq'], j['team']), ('team', 0, sw.team_id(self.rootpub)), '不写 --mode 也是 Team 的服务器')
        self.assertRegex(openssl_pin(self.d / 'cert.pem'), r'^sha256//[A-Za-z0-9+/]{43}=$', 'P-256 证书的公钥指纹（设备钉住的 pins）')

    def test_old_ops(self):
        """以前的维护者的服务器的设备清单（/v1/machines，发布密钥签的）、3.5.3 以前的自己的同步服务器的 op：都不认（404）"""
        self.both()
        for key in ('root', 'dev'):
            doc = json.dumps({'version': 1, 'seq': 1, 'machines': [{'key': self.evilpub, 'name': 'evil'}]}).encode()
            self.assertEqual(self.srv.req('POST', '/v1/machines', doc, sign(self.d / key, 'cca-devices', doc))[0], 404, key)
            self.assertEqual(self.call('machines', key=key, version=1, seq=1, machines=[])[0], 404, key)
        for op in ('join', 'members', 'relink', 'remove'):
            self.assertEqual(self.srv.req('POST', '/v1/' + op, b'{}', b'sig')[0], 404, op)
        self.assertEqual(self.call('report', key='evil', units={})[0], 403, '没登记上')

    def test_states(self):
        self.both()
        aid = secrets.token_hex(16)
        st, _ = self.call('report', units={aid: {'cli': [[100, 1], [50, 9]], 'app': None}}, end=None)
        self.assertEqual(st, 200)
        rows, j = self.view(aid)
        r = rows['alice-mbp']
        self.assertEqual((r['state'], r['cli'], r['app']), ('in_use', 2, False))
        self.assertTrue(99 <= r['since_age'] <= 101 and r['idle_age'] <= 2 and r['used_age'] <= 2)
        self.assertEqual(j['me_name'], 'bob-mini', '问的那台设备叫什么')
        self.call('report', units={aid: {'cli': [[200, 10]], 'app': [30, 5]}})
        r = self.view(aid)[0]['alice-mbp']
        self.assertEqual((r['state'], r['cli'], r['app']), ('idle', 1, True), '都超过 3 秒没动静：闲置')
        self.assertTrue(4 <= r['idle_age'] <= 6)
        self.call('report', units={}, end='none')
        r = self.view(aid)[0]['alice-mbp']
        self.assertEqual((r['state'], r['cli'], r['app']), ('none', 0, False), '会话都关了：没在用，留着上次使用')
        self.assertIsNotNone(r['used_age'])
        time.sleep(2.3)
        self.assertEqual(self.view(aid)[0]['alice-mbp']['state'], 'none', '报过“什么都没开了”：不会因为没有心跳变成离线')
        self.call('report', units={aid: {'cli': [[5, 0]], 'app': None}})
        self.assertEqual(self.view(aid)[0]['alice-mbp']['state'], 'in_use')
        time.sleep(2.3)
        r = self.view(aid)[0]['alice-mbp']
        self.assertEqual((r['state'], r['cli']), ('offline', 1), '开着会话却 ttl 秒没心跳：离线（可能睡眠、断网、被杀掉）')
        other_aid = secrets.token_hex(16)
        self.call('report', units={aid: {'cli': [[5, 0]], 'app': None}})
        self.call('report', units={aid: {'cli': [[5, 0]], 'app': None}}, end='down')
        self.assertEqual(self.view(aid)[0]['alice-mbp']['state'], 'offline', '上报进程被停掉：开着会话的账号马上算离线')
        self.call('report', units={other_aid: {'cli': [[5, 0]], 'app': None}})
        self.call('report', units={aid: {'cli': [[5, 0]], 'app': None}}, end='down')
        self.assertEqual(self.view(other_aid)[0]['alice-mbp']['state'], 'none', '没开会话的账号不跟着算离线')
        other = secrets.token_hex(16)
        self.call('report', key='dev2', units={other: {'cli': [[5, 0]], 'app': None}})
        self.assertEqual(self.view(aid)[0].get('bob-mini'), None, '别的账号的会话不出现在这个账号下')

    def test_auth(self):
        self.both()
        aid = secrets.token_hex(16)
        u = {'units': {aid: {'cli': [[1, 0]], 'app': None}}}
        st, j = self.call('report', key='evil', **u)
        self.assertEqual((st, j['error']), (403, 'unknown_device'), '不在名单里的设备')
        self.assertEqual(self.call('report', tamper=True, **u), (403, {'error': 'denied'}), '签过之后又改过的 body')
        self.assertEqual(self.call('report', ns='cca-devices', **u)[0], 403, '命名空间不对')
        self.assertEqual(self.call('report', key='dev', pub=self.dev2pub, **u)[0], 403, '用自己的 key 冒充别的设备')
        self.assertEqual(self.call('report', key='root', **u)[0], 403, '根密钥不是设备')
        st, j = self.call('report', ts=time.time() - 900, **u)
        self.assertEqual((st, j['error']), (400, 'clock'))
        self.assertIn('now', j)
        body = json.dumps({'v': 1, 'op': 'report', 'key': self.fp(self.devpub), 'ts': time.time(),
                           'nonce': secrets.token_hex(16), 'units': {}, 'end': 'none'}, separators=(',', ':')).encode()
        sig = sign(self.d / 'dev', 'cca-presence', body)
        self.assertEqual(self.srv.req('POST', '/v1/report', body, sig)[0], 200)
        self.assertEqual(self.srv.req('POST', '/v1/report', body, sig)[1].get('error'), 'replay', '同一个请求再发一次')
        self.assertEqual(self.srv.req('POST', '/v1/view', body, sig)[0], 400, '签的是 report，不能拿去 view')
        self.assertEqual(self.call('report', units={'NOT-AN-ID': {'cli': []}})[0], 400)
        self.assertEqual(self.call('report', units={aid: {'cli': [[1, 0]]}}, end='none')[0], 400, 'end=none 却还有会话')
        self.assertIn(self.srv.req('POST', '/v1/report', b'x' * (64 * 1024 + 1), b'sig')[0], (413, None), '太大的 body 不读')
        self.assertEqual(self.srv.req('POST', '/v1/report', b'{}')[0], 400, '没有签名')

    def test_rate_limit_after_verify(self):
        """别的设备拿这台的指纹乱发（签名不对）：扣不到这台的配额"""
        self.both()
        for _ in range(40):
            self.assertEqual(self.call('report', key='dev2', pub=self.devpub, units={})[0], 403)
        self.assertEqual(self.call('report', units={}, end='none')[0], 200)

    def test_connection_caps(self):
        """同一个 IP 最多 8 个连接：不说话的连接占不满服务器，关掉之后照常"""
        socks = [socket.create_connection(('127.0.0.1', self.srv.port), timeout=5) for _ in range(srvmod.MAX_CONN_IP)]
        try:
            time.sleep(0.3)
            extra = socket.create_connection(('127.0.0.1', self.srv.port), timeout=5)
            extra.settimeout(3)
            try:
                self.assertEqual(extra.recv(1), b'', '第 9 个马上被断开')
            except ConnectionResetError:
                pass
            finally:
                extra.close()
        finally:
            for x in socks:
                x.close()
        time.sleep(0.3)
        self.assertEqual(self.srv.req('GET', '/v1/health')[0], 200)

    def test_revoke(self):
        self.both()
        aid = secrets.token_hex(16)
        self.call('report', units={aid: {'cli': [[1, 0]], 'app': None}})
        self.assertIn('alice-mbp', self.view(aid)[0])
        self.revoke(self.DEV)
        self.assertNotIn('alice-mbp', self.view(aid)[0], '移除的设备：记录一起删掉')
        self.assertEqual(self.call('report', units={})[0], 403)

    def test_persist(self):
        self.both()
        aid = secrets.token_hex(16)
        self.call('report', units={aid: {'cli': [[60, 0]], 'app': None}})
        self.restart()
        self.assertEqual(self.view(aid)[0]['alice-mbp']['state'], 'in_use', '重启后照样认得名单和会话')
        self.assertEqual(oct(os.stat(self.d / 'state.json').st_mode & 0o777), '0o600')

    def store(self):
        """直接调 Store 用的（不经网络）：临时目录里一个新的，名单是空的"""
        t = tempfile.TemporaryDirectory()
        self.addCleanup(t.cleanup)
        return srvmod.Store(os.path.join(t.name, 'state.json'), 150, 600, 86400, cost_keep=1000, team_root=self.rootpub)

    def test_keys(self):
        """API key 的密文：设备传上来的整份换掉旧的，问的时候回现在的（谁传的、多久以前）；服务器不懂里面是什么，只查形状和长度"""
        self.restart()   # 限流的配额从头算（前面的用例用掉了不少）
        self.both()
        a1, a2 = secrets.token_hex(16), secrets.token_hex(16)
        blob = lambda n=256: {'n': base64.b64encode(secrets.token_bytes(16)).decode(), 'c': base64.b64encode(secrets.token_bytes(n)).decode(),
                              't': base64.b64encode(secrets.token_bytes(32)).decode()}
        b1 = blob()
        st, j = self.call('keys', put={a1: b1})
        self.assertEqual((st, j['me_name'], list(j['keys'])), (200, 'alice-mbp', [a1]), j)
        self.assertEqual((j['keys'][a1]['blob'], j['keys'][a1]['me'], j['keys'][a1]['name']), (b1, True, 'alice-mbp'))
        st, j = self.call('keys', key='dev2', get=[a1, a2, a1])
        self.assertEqual((st, list(j['keys'])), (200, [a1]), '没有的账号不回；问重复的只回一次')
        r = j['keys'][a1]
        self.assertEqual((r['blob'], r['me'], r['name']), (b1, False, 'alice-mbp'))
        self.assertLessEqual(r['age'], 2)
        b2 = blob(512)
        st, j = self.call('keys', key='dev2', put={a1: b2}, get=[a2])
        self.assertEqual((st, sorted(j['keys'])), (200, [a1]), '传的那些也回（问的里没有的不回）')
        st, j = self.call('keys', get=[a1])
        self.assertEqual((j['keys'][a1]['blob'], j['keys'][a1]['name'], j['keys'][a1]['me']), (b2, 'bob-mini', False), '新传的整个换掉旧的')
        ok = blob()
        for bad in ({'put': {a1: dict(ok, x='1')}}, {'put': {a1: dict(ok, n=ok['n'][:20])}}, {'put': {a1: dict(ok, t=ok['t'] + 'AAAA')}},
                    {'put': {a1: dict(ok, c='')}}, {'put': {a1: dict(ok, c='not base64!')}}, {'put': {a1: dict(ok, c=1)}},
                    {'put': {a1: dict(ok, c='A' * 8196)}}, {'put': {'NOT-AN-ID': ok}}, {'put': [ok]}, {'get': a1},
                    {'get': ['x']}, {'get': [secrets.token_hex(16) for _ in range(65)]}, {'put': {a1: 'x'}}):
            self.assertEqual(self.call('keys', **bad)[0], 400, bad)
        big = {secrets.token_hex(16): blob(4352) for _ in range(20)}
        self.assertEqual(self.call('keys', put=big)[0], 200, '一次传 20 个最长的 key（超过心跳的 64KB）也收')
        raw = (self.d / 'keys.json').read_text()
        self.assertEqual(oct(os.stat(self.d / 'keys.json').st_mode & 0o777), '0o600')
        self.assertIn(b2['c'], raw)
        self.restart()
        st, j = self.call('keys', get=[a1])
        self.assertEqual(j['keys'][a1]['blob'], b2, '重启后还在')
        self.revoke(self.DEV2)
        st, j = self.call('keys', get=[a1])
        self.assertEqual((st, j['keys'][a1]['blob'], j['keys'][a1]['name']), (200, b2, 'bob-mini'),
                         '移除了传它的设备：密文留着（是账号的，登录了这个账号的设备还要用）')
        self.assertEqual(self.call('keys', key='dev2', get=[a1])[0], 403, '移除的设备不能再问')
        # 存满了（MAX_KEYS 个账号）：新的账号不收，已有的照常换
        store = self.store()
        fp = self.fp(self.devpub)
        store.data['machines'][fp] = {'name': 'alice-mbp', 'key': self.devpub, 'dev': self.DEV}
        with mock.patch.object(srvmod, 'MAX_KEYS', 2):
            x, y, z = (secrets.token_hex(16) for _ in range(3))
            self.assertEqual(store.key_sync(fp, {x: ok, y: ok}, [], time.time())[0], 200)
            self.assertEqual(store.key_sync(fp, {z: ok}, [], time.time()), (507, {'error': 'full'}))
            self.assertEqual(store.key_sync(fp, {x: b1}, [], time.time())[0], 200)
            self.assertEqual(store.keys['acc'][x]['blob'], b1)

    def test_cost(self):
        """折合金额的密文：每个账号、每台设备一份，设备只写自己的那一份；问的时候回各台设备的（认证码没变的、自己的不带密文），
        移除的设备照样回（标上 revoked），顺带说哪些设备用过这个账号却没传过；服务器只查形状和长度"""
        self.restart()   # 限流的配额从头算
        self.both()
        a1, a2 = secrets.token_hex(16), secrets.token_hex(16)
        fa, fb = self.fp(self.devpub), self.fp(self.dev2pub)
        blob = lambda n=2048: {'n': base64.b64encode(secrets.token_bytes(16)).decode(), 'c': base64.b64encode(secrets.token_bytes(n)).decode(),
                               't': base64.b64encode(secrets.token_bytes(32)).decode()}
        st, j = self.srv.req('GET', '/v1/health')
        self.assertIn('ledger', j['ops'], 'health 说支持哪些 op（管理员看服务器是不是新的）')
        self.assertEqual(self.call('cost', accounts={a1: {'since': None, 'h': [[1, 2]]}})[0], 404,
                         '早先试过的 /v1/cost（另一种格式）不认：那时的客户端照旧版服务器处理')
        b1 = blob()
        st, j = self.call('ledger', put={a1: b1}, get=[a1])
        self.assertEqual((st, j['me_name'], j['missing']), (200, 'alice-mbp', {a1: []}), j)
        [r] = j['cost'][a1]
        self.assertEqual((r['fp'], r['name'], r['me'], r['revoked'], r['tag'], 'blob' in r), (fa, 'alice-mbp', True, False, b1['t'], False),
                         '自己的不回密文')
        st, j = self.call('ledger', key='dev2', get=[a1, a2, a1])
        self.assertEqual((st, sorted(j['cost'])), (200, sorted([a1, a2])), j)
        self.assertEqual(j['cost'][a2], [])
        [r] = j['cost'][a1]
        self.assertEqual((r['blob'], r['me'], r['name']), (b1, False, 'alice-mbp'))
        self.assertLessEqual(r['age'], 2)
        st, j = self.call('ledger', key='dev2', get=[a1], have={a1: {fa: b1['t']}})
        self.assertNotIn('blob', j['cost'][a1][0], '认证码没变：不回密文')
        st, j = self.call('ledger', key='dev2', get=[a1], have={a1: {fa: blob()['t']}})
        self.assertEqual(j['cost'][a1][0]['blob'], b1, '变了：回')
        b2 = blob(4096)
        self.assertEqual(self.call('ledger', key='dev2', put={a1: b2})[0], 200)
        st, j = self.call('ledger', get=[a1])
        rows = {x['name']: x for x in j['cost'][a1]}
        self.assertEqual((rows['bob-mini']['blob'], 'blob' in rows['alice-mbp']), (b2, False), '各台设备一份，互不覆盖')
        b3 = blob(1024)
        self.call('ledger', put={a1: b3})
        st, j = self.call('ledger', key='dev2', get=[a1])
        self.assertEqual({x['name']: x.get('blob') for x in j['cost'][a1]}, {'alice-mbp': b3, 'bob-mini': None}, '新传的换掉这台设备旧的')
        ok = blob()
        for bad in ({'put': {a1: dict(ok, x='1')}}, {'put': {a1: dict(ok, c='A' * 98308)}}, {'put': {a1: dict(ok, c='not base64!')}},
                    {'put': {'NOT-AN-ID': ok}}, {'put': [ok]}, {'get': a1}, {'get': ['x']}, {'have': ['x']},
                    {'have': {a1: {'bad-fp': ok['t']}}}, {'have': {a1: {fa: 'short'}}}, {'have': {'x': {}}},
                    {'get': [secrets.token_hex(16) for _ in range(65)]}):
            self.assertEqual(self.call('ledger', **bad)[0], 400, bad)
        self.assertEqual(self.call('ledger', put={secrets.token_hex(16): blob(70000) for _ in range(10)})[0], 200,
                         '一次传 10 个最长的（超过心跳的 64KB）也收')
        # 用过这个账号（上报过）、却没传过折合金额的设备：多半是 cca 太旧
        self.call('report', key='dev2', units={a2: {'cli': [[100, 0]], 'app': None}}, end=None)
        st, j = self.call('ledger', get=[a2])
        self.assertEqual([m['name'] for m in j['missing'][a2]], ['bob-mini'])
        self.assertLessEqual(j['missing'][a2][0]['used_age'], 2)
        self.assertEqual(self.call('ledger', key='dev2', get=[a2])[1]['missing'][a2], [],
                         'bob-mini 自己第一次问（还没传）：不把它自己算成没传过的（它这一轮接着就传）')
        self.call('ledger', key='dev2', put={a2: blob()})
        self.assertEqual(self.call('ledger', get=[a2])[1]['missing'][a2], [], '传过了就不提')
        raw = (self.d / 'cost-slots.json').read_text()
        self.assertEqual(oct(os.stat(self.d / 'cost-slots.json').st_mode & 0o777), '0o600')
        self.assertIn(b2['c'], raw)
        self.restart()
        st, j = self.call('ledger', key='dev2', get=[a1])
        self.assertEqual({x['name']: x.get('blob') for x in j['cost'][a1]}, {'alice-mbp': b3, 'bob-mini': None}, '重启后还在')
        self.revoke(self.DEV2)
        st, j = self.call('ledger', get=[a1])
        rows = {x['name']: x for x in j['cost'][a1]}
        self.assertEqual((rows['bob-mini']['revoked'], rows['bob-mini']['blob'], rows['alice-mbp']['revoked']), (True, b2, False),
                         '移除了的设备：它传的留着（是账号的用量），标上 revoked')
        self.assertEqual(self.call('ledger', key='dev2', get=[a1])[0], 403, '移除的设备不能再问')
        store = self.store()
        store.data['machines'] = {fa: {'name': 'alice-mbp', 'key': self.devpub, 'dev': self.DEV},
                                  fb: {'name': 'bob-mini', 'key': self.dev2pub, 'dev': self.DEV2}}
        now = time.time()
        x, y, z = (secrets.token_hex(16) for _ in range(3))
        with mock.patch.object(srvmod, 'COST_SLOTS', 2):
            self.assertEqual(store.cost_sync(fa, {x: ok, y: ok}, [], {}, now)[0], 200)
            self.assertEqual(store.cost_sync(fa, {z: ok}, [], {}, now), (507, {'error': 'full'}), '一台设备最多这么多个账号')
            self.assertEqual(store.cost_sync(fa, {x: b1}, [], {}, now)[0], 200, '已有的照常换')
            self.assertEqual(store.cost_sync(fb, {z: ok}, [], {}, now)[0], 200, '别的设备不受影响')
        with mock.patch.object(srvmod, 'COST_DEVICE_BYTES', len(ok['c']) * 2 + 10):
            self.assertEqual(store.cost_sync(fb, {x: ok, y: ok}, [], {}, now)[0], 507, '一台设备存的加起来有上限')
        with mock.patch.object(srvmod, 'COST_TOTAL_BYTES', len(ok['c']) * 4 - 1):
            self.assertEqual(store.cost_sync(fb, {y: ok}, [], {}, now)[0], 507, '所有设备加起来有上限')
        store.cost_sync(fb, {x: blob(3000)}, [], {}, now + 5)   # 4000 个字符
        fc = 'SHA256:' + 'C' * 43
        store.data['machines'][fc] = {'name': 'carol-air', 'key': self.evilpub, 'dev': '0c' * 8}
        with mock.patch.object(srvmod, 'COST_RESP', 4500):
            st, j = store.cost_sync(fc, {}, [x], {}, now + 6)
            self.assertEqual([(r['name'], 'blob' in r) for r in j['cost'][x]], [('bob-mini', True), ('alice-mbp', False)],
                             '回的密文有上限：最近传的先给，别的下次再取')
        self.assertEqual(store.cost_sync('SHA256:gone', {}, [x], {}, now)[0], 403)
        with mock.patch.object(srvmod, 'COST_TOTAL_BYTES', len(ok['c']) * 2):
            self.assertEqual(store.cost_sync(fb, {y: ok}, [], {}, now + 2000)[0], 200,
                             '存满了：先清掉 cost_keep 秒没再传的，再看放不放得下（不然一直清不掉）')
        self.assertEqual(sorted(store.cost['acc']), [y], 'cost_keep 秒没再传的删掉')


class SignedCalls:
    """Team 的服务器的测试用：self.pubs 是 {名字: 公钥}（私钥是 self.d 下同名的文件），self.srv 是服务器"""
    def fp(self, who):
        return srvmod.key_fp(self.pubs[who].split()[1])

    def call(self, op, who='a', signer=None, tamper=False, srv=None, **extra):
        body = dict({'v': 1, 'op': op, 'key': self.fp(who), 'ts': time.time(), 'nonce': secrets.token_hex(16)}, **extra)
        raw = json.dumps(body, separators=(',', ':')).encode()
        sig = sign(self.d / (signer or who), 'cca-presence', raw)
        if tamper:
            raw = raw.replace(b'"v":1', b'"v":1 ')
        return (srv or self.srv).req('POST', '/v1/' + op, raw, sig)

    def unknown(self, reply):
        """403 unknown_device 带着的 gen（不是这样的回复是 None）"""
        st, j = reply
        ok = st == 403 and set(j) == {'error', 'gen'} and j['error'] == 'unknown_device' and re.match(r'^[0-9a-f]{32}$', j['gen'])
        return j['gen'] if ok else None


T0 = 'Sat Nov  7 12:08:55 2026'   # 假 ps 说的启动时间（ps -o lstart 的写法）
T0_EPOCH = int(time.mktime(time.strptime(T0, '%a %b %d %H:%M:%S %Y')))
SESSION_PID = 999999   # 不可能有这个进程号：上报进程盯不上它（kqueue），每秒回来看一次

# 设备上的 curl：请求照发（真的 curl、真的服务器），顺手记下来——~/curl.argv 是每次的命令行（查凭证从不出现在命令行里），
# ~/sent/<第几个>.json 是签过名的请求，.op 是哪个接口，
# .reply 是服务器回的内容，.code 是它回的状态码（回来了才有：没发出去是 0）。~/reply/<接口> 在时不发，照它回（第一行是状态码，
# 后面是回的内容）：扮演还不认这个接口的旧服务器
CURL = r'''printf '%s\n' "$*" >> "$HOME/curl.argv"
mkdir -p "$HOME/sent"
tmp="$HOME/sent/.req.$$"
cat > "$tmp"
n=0; while ! ln "$tmp" "$HOME/sent/$n.json" 2>/dev/null; do n=$((n + 1)); done
rm -f "$tmp"
for a in "$@"; do url="$a"; done
op="${url##*/}"
printf '%s' "$op" > "$HOME/sent/$n.op"
if [ -f "$HOME/reply/$op" ]; then
	out="$(tail -n +2 "$HOME/reply/$op")
$(head -n 1 "$HOME/reply/$op")"; rc=0
else
	out=$(/usr/bin/curl "$@" < "$HOME/sent/$n.json"); rc=$?
fi
printf '%s' "$out" | sed '$d' > "$HOME/sent/$n.reply"
printf '%s' "$out" | tail -n 1 > "$HOME/sent/$n.code.tmp"
mv "$HOME/sent/$n.code.tmp" "$HOME/sent/$n.code"
printf '%s' "$out"
exit $rc
'''
# 设备上的 ps：进程表照 ~/ps.out（没有就是一个进程都没有）；问某个进程的启动时间一律说 T0。每问一次进程表记一笔（上报进程转了几圈）
PS = r'''case " $* " in
	*" -p "*) for a in "$@"; do pid="$a"; done; printf '%s %s\n' "$pid" "''' + T0 + r'''" ;;
	*) echo x >> "$HOME/ps.log"; cat "$HOME/ps.out" 2>/dev/null ;;
esac
exit 0
'''
# 后台起的 cca（记下 key 以后的 _key-sync 这些）：只记一笔（自己的进程号、参数），不真的起
SELF = 'echo "$$ $*" >> "$HOME/kicks.log"\n'
# 默认：钥匙串碰不到（key 存成文件，CCA_KEYSTORE=file）
NO_KEYCHAIN = 'echo "测试不碰钥匙串：security $*" >> "$HOME/security.log"\nexit 1\n'
# 假的钥匙串（要它的用例才装）：条目是 ~/keychain/<服务名>/<账户名> 这些文件；~/keychain_stuck 在时删不掉（钥匙串锁着）
KEYCHAIN = r'''KC="$HOME/keychain"
one() {
	[ $# -gt 0 ] || return 1
	cmd="$1"; shift
	svc=""; acct=""; hex=""; w=0; prev=""
	for a in "$@"; do
		case "$prev" in -s) svc="$a" ;; -a) acct="$a" ;; -X) hex="$a" ;; esac
		[ "$a" = -w ] && w=1
		prev="$a"
	done
	[ -n "$svc" ] || return 1
	if [ -n "$acct" ]; then f="$KC/$svc/$acct"; else f=$(ls "$KC/$svc/"* 2>/dev/null | head -n 1); fi
	case "$cmd" in
		add-generic-password) mkdir -p "$KC/$svc"; printf '%s' "$hex" | xxd -r -p > "$KC/$svc/${acct:-_}"; return 0 ;;
		find-generic-password) [ -n "$f" ] && [ -f "$f" ] || return 44; [ $w = 1 ] && { cat "$f"; echo; }; return 0 ;;
		delete-generic-password) [ -n "$f" ] && [ -f "$f" ] || return 44; [ -f "$HOME/keychain_stuck" ] && return 51; rm -f "$f"; return 0 ;;
	esac
	return 1
}
if [ "${1:-}" = -i ]; then
	while IFS= read -r line; do set -f; set -- $line; set +f; one "$@"; done
	exit 0
fi
one "$@"
'''


class TeamServerTest(SignedCalls, unittest.TestCase):
    """Team 的同步服务器（--mode team）：管理员拿根密钥登记、邀请、移除设备，存各台设备的配置；设备拿邀请加入、取自己的那一份。
    邀请链接、配置的密文和签名照 server/README.md「Team 的同步服务器」在 spec_wire 里独立实现"""
    SALT = 'cd' * 16

    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='cca-team-test-')
        d = cls.d = Path(cls.tmp.name)
        make_cert(d)
        cls.pubs = {n: keygen(d / n) for n in ('root', 'root2', 'a', 'b', 'c', 'evil')}
        cls.tid = sw.team_id(cls.pubs['root'])
        cls.srv = Server(d, team_root=cls.pubs['root'])

    @classmethod
    def tearDownClass(cls):
        cls.srv.stop()
        cls.tmp.cleanup()

    def admin(self, op, **kw):
        """管理员的请求：拿根密钥签名（限额另算：每秒 2 个、攒得下 120 个；用完了等一会儿再试）"""
        who = kw.pop('who', 'root')
        for _ in range(5):
            st, j = self.call(op, who, **kw)
            if st != 429:
                break
            time.sleep(2.1)
        return st, j

    def invite(self, **kw):
        """发一个邀请：(那串随机数, 令牌, 设备编号, 配置的密钥)"""
        secret = secrets.token_hex(16)
        token, dev, keys = sw.invite_parts(secret)
        st, j = self.admin('team_invite', token=hashlib.sha256(token.encode()).hexdigest(), dev=dev, ttl=3600, **kw)
        self.assertEqual(st, 200, j)
        return secret, token, dev, keys

    def join(self, who, token, name=None, pub=None, **kw):
        return self.call('team_join', who, pub=pub or self.pubs[who], name=name or who + '-mac', token=token, **kw)

    def doc(self, keys, dev, seq, content, root='root'):
        ct = sw.team_doc_seal(keys, self.tid, dev, seq, content)
        return {'seq': seq, 'ct': ct, 'sig': sw.team_doc_sign(str(self.d / root), self.tid, dev, seq, ct)}

    def test_flow(self):
        st, j = self.srv.req('GET', '/v1/health')
        self.assertEqual((st, j['mode'], j['team']), (200, 'team', self.tid))
        self.assertRegex(self.tid, r'^[0-9a-f]{32}$')
        self.assertTrue(set(srvmod.TEAM_OPS) | {'report', 'view', 'keys', 'ledger'} <= set(j['ops']), j['ops'])
        self.assertFalse({'join', 'members', 'relink', 'remove'} & set(j['ops']), '3.5.3 以前的自己的同步服务器的那几个 op 没有了')
        self.assertEqual(self.call('join', pub=self.pubs['a'], name='a', admit='ab' * 16, check='cd' * 16)[0], 404)
        doc = json.dumps({'version': 1, 'seq': 1, 'machines': [{'key': self.pubs['evil'], 'name': 'evil'}]}).encode()
        self.assertEqual(self.srv.req('POST', '/v1/machines', doc, sign(self.d / 'evil', 'cca-devices', doc))[0], 404)
        gen = self.unknown(self.call('team_get'))
        self.assertTrue(gen, '不在名单里：回这一份记录的编号')
        # 管理员的 op 只认根密钥：设备的密钥、冒用根公钥指纹的别的密钥、改过的请求都是 403
        da, db = '0a' * 8, None
        enroll = dict(devices={da: {'pub': self.pubs['a'], 'name': 'studio'}})
        for bad in (dict(who='a'), dict(signer='evil'), dict(tamper=True)):
            self.assertEqual(self.admin('team_enroll', **dict(enroll, **bad)), (403, {'error': 'denied'}), bad)
        self.assertEqual(self.admin('team_members', who='evil'), (403, {'error': 'denied'}))
        self.assertEqual(self.unknown(self.call('report', 'root', units={}, end=None)), gen, '根密钥不是设备：别的 op 不认它')
        # 登记（管理员自己的那台）
        for bad in ({}, {'xyz': {'pub': self.pubs['a'], 'name': 'x'}}, {da: {'pub': 'ssh-rsa AAAA', 'name': 'x'}}, {da: 'x'}, [da],
                    {da: {'pub': self.pubs['a'], 'name': ' x'}}, {da: {'pub': self.pubs['a'], 'name': 'x'}, '0b' * 8: {'pub': self.pubs['a'], 'name': 'y'}}):
            self.assertIsNone(srvmod.parse_enroll(bad), bad)
        self.assertEqual(self.admin('team_enroll', devices={})[0], 400)
        self.assertEqual(self.admin('team_enroll', devices={da: {'pub': self.pubs['root'], 'name': 'root'}})[0], 400, '根密钥不是设备：登记不了')
        st, j = self.admin('team_enroll', **enroll)
        self.assertEqual((st, j['names'], j['members'], j['gen']), (200, {da: 'studio'}, 1, gen), j)
        st, j = self.call('team_get')
        self.assertEqual((st, j['dev'], j['me_name'], j['seq'], j['doc'], j['gen']), (200, da, 'studio', 0, None, gen), j)
        self.assertEqual(self.admin('team_enroll', devices={'0c' * 8: {'pub': self.pubs['a'], 'name': 'again'}}), (409, {'error': 'taken'}),
                         '这把公钥已经是别的编号的设备')
        # 邀请：服务器只存令牌的哈希
        for bad in (dict(token='zz', dev=da, ttl=3600), dict(token='0' * 64, dev='nope', ttl=3600), dict(token='0' * 64, dev='0b' * 8, ttl=5),
                    dict(token='0' * 64, dev='0b' * 8, ttl=40 * 86400)):
            self.assertEqual(self.admin('team_invite', **bad)[0], 400, bad)
        self.assertEqual(self.admin('team_invite', token='0' * 64, dev=da, ttl=3600), (409, {'error': 'taken'}), '这个编号的设备已经加入了')
        secret, token, db, kb = self.invite()
        state = (self.d / 'state.json').read_text()
        self.assertTrue(token not in state and secret not in state, '令牌、那串随机数不落在服务器上')
        self.assertIn(hashlib.sha256(token.encode()).hexdigest(), state)
        # 加入：没有这个邀请、签名不对、pub 和 key 对不上：一样的 403
        for bad in (dict(token='00' * 16), dict(token='nope'), dict(tamper=True), dict(pub=self.pubs['c']), dict(signer='evil')):
            self.assertEqual(self.join('b', **dict(dict(token=token), **bad)), (403, {'error': 'denied'}), bad)
        self.assertEqual(self.join('b', token, name='x' * 65)[0], 400)
        self.assertEqual(self.join('root', token), (403, {'error': 'denied'}), '根密钥不是设备：拿着邀请也加入不了')
        self.assertEqual(self.join('a', token), (409, {'error': 'taken'}), '这把密钥已经是一台设备了')
        self.assertEqual(self.join('b', token, proof='not-a-tag')[0], 400, '认证码的样子不对')
        proof = sw.team_pub_proof(secret, self.pubs['b'])
        st, j = self.join('b', token, name='studio', proof=proof)
        self.assertEqual((st, j['name'], j['dev'], j['members'], j['gen']), (200, 'studio-2', db, 2, gen), j)
        self.assertEqual(self.join('c', token), (403, {'error': 'used'}), '邀请只能用一次（签名对的才问得出来是用过了）')
        self.assertEqual(self.join('c', token, signer='evil'), (403, {'error': 'denied'}))
        self.assertEqual(self.admin('team_invite', token=hashlib.sha256(token.encode()).hexdigest(), dev='0d' * 8, ttl=3600),
                         (409, {'error': 'used'}), '用过的令牌不能再发一次')
        # 同一个编号再发一个邀请：前一个作废
        old = self.invite()
        st, j = self.admin('team_invite', token='1' * 64, dev=old[2], ttl=3600)
        self.assertEqual((st, j['invites']), (200, 1), j)
        self.assertEqual(self.join('c', old[1]), (403, {'error': 'denied'}))
        self.assertEqual(self.admin('team_revoke', dev=old[2])[0], 200, '还没用掉的邀请也能撤回')
        # 配置：每台设备一份，序号只增不减；别的设备的取不到
        content = {'v': 1, 'name': '测试组', 'accounts': {'claude': {'alice@example.com': 'US-A'}}}
        d1 = self.doc(kb, db, 1, content)
        st, j = self.admin('team_put', docs={db: d1, 'ee' * 8: d1})
        self.assertEqual((st, j['stored'], j['stale'], j['unknown']), (200, [db], {}, ['ee' * 8]), j)
        st, j = self.call('team_get', 'b')
        self.assertEqual((st, j['dev'], j['seq'], j['doc']), (200, db, 1, d1), j)
        self.assertTrue(sw.team_doc_verify(self.pubs['root'], self.tid, db, 1, d1['ct'], d1['sig']), '根密钥签的')
        self.assertFalse(sw.team_doc_verify(self.pubs['evil'], self.tid, db, 1, d1['ct'], d1['sig']))
        self.assertFalse(sw.team_doc_verify(self.pubs['root'], self.tid, db, 2, d1['ct'], d1['sig']), '换个序号签名就不对')
        self.assertEqual(sw.team_doc_open(kb, self.tid, db, 1, d1['ct']), content)
        self.assertIsNone(sw.team_doc_open(kb, self.tid, da, 1, d1['ct']), '挪到别的设备上认证不过')
        self.assertIsNone(sw.team_doc_open(kb, self.tid, db, 2, d1['ct']), '换个序号认证不过')
        self.assertEqual(len(base64.b64decode(d1['ct']['c'])) % 1024, 0, '补齐到 1024 的整数倍')
        self.assertEqual(self.call('team_get', 'b', have=1)[1]['doc'], None, '手里的就是这一份：不带')
        self.assertEqual(self.call('team_get', 'b', have=0)[0], 400)
        self.assertEqual(self.call('team_get')[1]['doc'], None, '别的设备取不到它的')
        self.assertEqual(self.admin('team_put', docs={db: d1})[1]['stored'], [db], '同一份再存一次（没收到回复时）')
        self.assertEqual(self.admin('team_put', docs={db: self.doc(kb, db, 1, {'v': 1})})[1]['stale'], {db: 1}, '同一个序号、不同的内容')
        d3 = self.doc(kb, db, 3, dict(content, name='新的'))
        self.assertEqual(self.admin('team_put', docs={db: d3})[1]['stored'], [db])
        self.assertEqual(self.admin('team_put', docs={db: self.doc(kb, db, 2, content)})[1]['stale'], {db: 3}, '序号往回走的不存')
        self.assertEqual(self.call('team_get', 'b', have=1)[1]['doc'], d3)
        self.assertEqual(self.call('team_get', 'b', have=7)[1]['doc'], d3, '设备手里的比存着的新（服务器的那一份被换成旧的了）：照样给，设备自己看序号')
        self.assertEqual(self.call('team_get', 'b', have=2 ** 53)[0], 400)
        for bad in ({}, {db: dict(d1, seq=0)}, {db: dict(d1, seq=True)}, {db: dict(d1, sig='')}, {db: dict(d1, sig='!!!!')}, {db: dict(d1, extra=1)},
                    {'x': d1}, {db: dict(d1, ct={'n': 'x'})}, {db: dict(d1, ct=dict(d1['ct'], c='A' * (srvmod.MAX_TEAM_DOC + 4)))},
                    {db: dict(d1, sig='A' * (srvmod.MAX_TEAM_SIG + 4))}, {'%016x' % i: d1 for i in range(srvmod.MAX_TEAM_BATCH + 1)}):
            self.assertIsNone(srvmod.parse_team_docs(bad), str(bad)[:80])
        self.assertEqual(srvmod.parse_team_docs({db: d1}), {db: d1})
        self.assertEqual(self.admin('team_put', docs={db: dict(d1, seq=0)})[0], 400)
        self.assertEqual(self.call('team_put', 'b', docs={db: d1}), (403, {'error': 'denied'}), '设备存不了配置')
        # 还没加入的设备的配置可以先存着：加入以后马上同步得到
        _, tc, dc, kc = self.invite()
        d0 = self.doc(kc, dc, 1, content)
        self.assertEqual(self.admin('team_put', docs={dc: d0})[1]['stored'], [dc])
        st, j = self.admin('team_members')
        rows = {r['name']: r for r in j['members']}
        self.assertEqual((st, sorted(rows), j['gen']), (200, ['studio', 'studio-2'], gen), j)
        self.assertEqual((rows['studio']['dev'], rows['studio']['pub'], rows['studio']['fp'], rows['studio']['seq']),
                         (da, self.pubs['a'], self.fp('a'), 0))
        self.assertEqual((rows['studio-2']['dev'], rows['studio-2']['seq'], rows['studio-2']['have'], rows['studio']['have']), (db, 3, 7, None),
                         '各台设备同步配置时说的手里那一份的序号')
        self.assertEqual((rows['studio-2']['proof'], rows['studio']['proof']), (proof, None), '设备加入时带的认证码原样回给管理员；直接登记的没有')
        self.assertTrue(all(0 <= r['joined_age'] <= 120 and 0 <= r['seen_age'] <= 120 for r in rows.values()), rows)
        self.assertEqual([(i['dev'], i['seq'], 3500 < i['expires_in'] <= 3600) for i in j['invites']], [(dc, 1, True)], j['invites'])
        self.assertEqual(self.call('team_members', 'b'), (403, {'error': 'denied'}), '设备看不了名单')
        self.assertEqual(self.join('c', tc)[0], 200)
        self.assertEqual(self.call('team_get', 'c')[1]['doc'], d0)
        # 别的 op 照旧
        aid = sw.aid_of(self.SALT, 'claude', 'alice@example.com')
        self.assertEqual(self.call('report', 'b', units={aid: {'cli': [[10, 1]], 'app': None}}, end=None)[0], 200)
        st, j = self.call('view', accounts=[aid])
        self.assertEqual([(r['name'], r['state']) for r in j['accounts'][aid]], [('studio-2', 'in_use')])
        blob = {'n': base64.b64encode(secrets.token_bytes(16)).decode(), 'c': base64.b64encode(secrets.token_bytes(1024)).decode(),
                't': base64.b64encode(secrets.token_bytes(32)).decode()}
        self.assertEqual(self.call('keys', 'b', put={aid: blob})[0], 200)
        self.assertEqual(self.call('ledger', 'b', put={aid: blob})[0], 200)
        self.assertEqual(self.call('keys', get=[aid])[1]['keys'][aid]['name'], 'studio-2')
        # 请求的大小：管理员存配置的最多 1 MB，别的 64 KB
        big = b'x' * (srvmod.MAX_BODY + 1)
        self.assertEqual(self.srv.req('POST', '/v1/team_put', big, b'sig')[0], 400, '过了大小这一关（内容不是 JSON）')
        self.assertIn(self.srv.req('POST', '/v1/team_enroll', big, b'sig')[0], (413, None))
        self.assertIn(self.srv.req('POST', '/v1/team_put', b'x' * (srvmod.MAX_SYNC_BODY + 1), b'sig')[0], (413, None))
        # 移除：密钥就此不认，给它的配置一起删掉
        self.assertEqual(self.admin('team_revoke', dev='nope')[0], 400)
        self.assertEqual(self.admin('team_revoke', dev='ee' * 8), (404, {'error': 'no_device'}))
        self.assertEqual(self.call('team_revoke', 'c', dev=db), (403, {'error': 'denied'}), '设备移除不了别的设备')
        st, j = self.admin('team_revoke', dev=db)
        self.assertEqual((st, j['members']), (200, 2))
        self.assertEqual(self.unknown(self.call('team_get', 'b')), gen, '被移除的设备：编号照旧（是被移除了）')
        self.assertEqual(self.admin('team_put', docs={db: self.doc(kb, db, 9, content)})[1]['unknown'], [db])
        self.assertEqual(self.call('view', accounts=[aid])[1]['accounts'][aid], [], '它上报的记录跟着删掉')
        # 退出
        st, j = self.call('team_leave', 'c')
        self.assertEqual((st, j['members']), (200, 1))
        self.assertEqual(self.unknown(self.call('team_get', 'c')), gen)
        self.assertNotIn(dc, json.loads((self.d / 'team-docs.json').read_text())['docs'])
        # 换了密钥的设备：管理员照编号重新登记，原来那把就此不认
        st, j = self.admin('team_enroll', devices={da: {'pub': self.pubs['c'], 'name': 'studio'}})
        self.assertEqual((st, j['names'], j['members']), (200, {da: 'studio'}, 1), j)
        self.assertEqual(self.unknown(self.call('team_get')), gen)
        self.assertEqual(self.call('team_get', 'c')[1]['dev'], da)
        _, tv, dv, _ = self.invite()
        self.assertEqual(self.admin('team_enroll', devices={dv: {'pub': self.pubs['b'], 'name': 'direct'}})[0], 200)
        self.assertEqual(self.join('evil', tv), (403, {'error': 'denied'}), '这个编号直接登记了：还没用掉的邀请作废')
        self.assertEqual(self.admin('team_revoke', dev=dv)[1]['members'], 1)
        # 重启：同一个根公钥接着用；换一个就从头开始
        da_doc = self.doc(sw.invite_parts('ab' * 16)[2], da, 1, content)
        self.assertEqual(self.admin('team_put', docs={da: da_doc})[1]['stored'], [da])
        self.srv.stop()
        TeamServerTest.srv = Server(self.d, team_root=self.pubs['root'])
        st, j = self.call('team_get', 'c')
        self.assertEqual((st, j['gen'], j['doc']), (200, gen, da_doc), '重启后照样认得设备、配置还在，编号不变')
        self.assertEqual(oct(os.stat(self.d / 'team-docs.json').st_mode & 0o777), '0o600')
        self.srv.stop()
        TeamServerTest.srv = Server(self.d, team_root=self.pubs['root2'])
        self.assertEqual(self.srv.req('GET', '/v1/health')[1]['team'], sw.team_id(self.pubs['root2']))
        gen2 = self.unknown(self.call('team_get', 'c'))
        self.assertTrue(gen2 and gen2 != gen, '换了一个 Team：以前的设备不认了，记录换了编号')
        self.assertEqual(self.admin('team_members'), (403, {'error': 'denied'}), '原来的根密钥不认了')
        st, j = self.admin('team_members', who='root2')
        self.assertEqual((st, j['members'], j['invites']), (200, [], []))
        self.assertEqual(json.loads((self.d / 'team-docs.json').read_text())['docs'], {}, '以前存的配置都删掉')

    def test_records_lost(self):
        """服务器的记录没了（状态文件丢了，配置的根公钥照旧）：设备自己加不回来（邀请只能用一次），管理员照 team_members 回过的
        公钥登记回去、再存一次配置"""
        d = self.d / 'lost'
        d.mkdir()
        srv = Server(d, team_root=self.pubs['root'], proxied=True)
        self.addCleanup(lambda: srv.stop())
        self.assertEqual(self.admin('team_enroll', srv=srv, devices={'0a' * 8: {'pub': self.pubs['a'], 'name': 'studio'}})[0], 200)
        _, token, db, kb = self.invite(srv=srv)
        self.assertEqual(self.join('b', token, srv=srv)[0], 200)
        doc = self.doc(kb, db, 4, {'v': 1})
        self.assertEqual(self.admin('team_put', srv=srv, docs={db: doc})[1]['stored'], [db])
        st, j = self.admin('team_members', srv=srv)
        gen, saved = j['gen'], {r['dev']: {'pub': r['pub'], 'name': r['name']} for r in j['members']}
        srv.stop()
        for f in ('state.json', 'team-docs.json'):
            (d / f).unlink()
        srv = Server(d, team_root=self.pubs['root'], proxied=True)
        gen2 = self.unknown(self.call('team_get', 'b', srv=srv))
        self.assertTrue(gen2 and gen2 != gen, '记录换了编号：设备据此知道不是被移除了')
        self.assertEqual(self.join('b', token, srv=srv), (403, {'error': 'denied'}), '用过的邀请加不回来')
        st, j = self.admin('team_enroll', srv=srv, devices=saved)
        self.assertEqual((st, j['names'], j['members'], j['gen']), (200, {'0a' * 8: 'studio', db: 'b-mac'}, 2, gen2), j)
        st, j = self.call('team_get', 'b', have=4, srv=srv)
        self.assertEqual((st, j['dev'], j['seq'], j['doc']), (200, db, 0, None), '还没有配置：设备接着用手里的那一份')
        self.assertEqual(self.admin('team_put', srv=srv, docs={db: doc})[1]['stored'], [db], '同一个序号再存一次')
        self.assertEqual(self.call('team_get', 'b', have=4, srv=srv)[1]['doc'], None)
        self.assertEqual(self.call('team_get', 'b', srv=srv)[1]['doc'], doc)

    def store(self, name, **kw):
        d = self.d / name
        d.mkdir(exist_ok=True)
        return srvmod.Store(str(d / 'state.json'), 150, 600, 30 * 86400, **kw)

    def test_store(self):
        """过期、清理、上限、换模式：直接调 Store（不用真的等）"""
        log = mock.patch.object(sys, 'stderr', io.StringIO())   # Store 往 stderr 记的那几句不混进测试的输出
        err = log.start()
        self.addCleanup(log.stop)
        root, H = self.pubs['root'], lambda t: hashlib.sha256(t.encode()).hexdigest()
        b64 = lambda n: base64.b64encode(b'\0' * n).decode()
        blob = lambda size=1024: {'n': b64(16), 'c': 'A' * size, 't': b64(32)}
        st = self.store('s1', team_root=root)
        t1, t2 = 'aa' * 16, 'bb' * 16
        self.assertEqual(st.team_invite(H(t1), '01' * 8, 60, 1000)[0], 200)
        self.assertEqual((st.invite_state(t1, 1059), st.invite_state(t1, 1060), st.invite_state(t2, 1000), st.invite_state('zz', 1000)),
                         ('ok', 'expired', None, None))
        self.assertEqual(st.team_join('SHA256:x', self.pubs['a'], 'late', t1, 1100), (403, {'error': 'expired'}))
        self.assertEqual(st.team_put({'01' * 8: {'seq': 1, 'ct': blob(), 'sig': 'AAAA'}}, 1100)[1]['stored'], ['01' * 8],
                         '过期不久的邀请：配置照存（管理员多半会再发一个）')
        self.assertEqual(st.team_members(1100)[1]['invites'], [], '过期的邀请不列出来')
        self.assertEqual(st.team_invite(H(t2), '01' * 8, 60, 1100)[0], 200, '同一个编号再发')
        self.assertEqual(st.team_join('SHA256:x', self.pubs['a'], 'mac', t2, 1101, proof='pf')[0], 200)
        self.assertEqual(st.invite_state(t2, 1102), 'used')
        later = 1101 + srvmod.INVITE_KEEP + 1
        self.assertEqual(st.team_members(later)[0], 200)   # 看名单、存配置、发邀请时顺带清
        self.assertEqual(st.invite_state(t2, later), None, '用掉很久的不记了')
        # 给邀请存的配置：邀请过期很久被清掉时跟着删；只剩配置的编号也移除得掉
        self.assertEqual(st.team_invite(H('cc' * 16), '02' * 8, 60, later)[0], 200)
        self.assertEqual(st.team_put({'02' * 8: {'seq': 1, 'ct': blob(), 'sig': 'AAAA'}}, later)[1]['stored'], ['02' * 8])
        self.assertEqual(st.team_put({'01' * 8: {'seq': 2, 'ct': blob(), 'sig': 'AAAA'}}, later + 60 + srvmod.INVITE_KEEP)[1]['stored'], ['01' * 8])
        self.assertEqual(sorted(json.loads((self.d / 's1' / 'team-docs.json').read_text())['docs']), ['01' * 8], '02 的邀请清掉了，配置跟着删')
        st.docs['docs']['03' * 8] = {'seq': 1, 'ct': blob(), 'sig': 'AAAA', 'at': 1}   # 移除设备时写了一半留下的
        self.assertEqual(st.team_revoke('03' * 8), (200, {'v': 1, 'ok': True, 'members': 1}))
        self.assertEqual(st.team_revoke('03' * 8), (404, {'error': 'no_device'}))
        # 满了
        with mock.patch.object(srvmod, 'MAX_TEAM_MEMBERS', 1):
            self.assertEqual(st.team_enroll({'04' * 8: ('SHA256:y', self.pubs['b'], 'two')}, later), (507, {'error': 'full'}))
            self.assertEqual(st.team_invite(H('dd' * 16), '05' * 8, 60, later)[0], 200)
            self.assertEqual(st.team_join('SHA256:y', self.pubs['b'], 'two', 'dd' * 16, later), (507, {'error': 'full'}))
            self.assertEqual(st.team_enroll({'01' * 8: ('SHA256:x', self.pubs['a'], 'renamed')}, later)[1]['names'], {'01' * 8: 'renamed'},
                             '已经在的设备改名字不算多')
        m = st.data['machines']['SHA256:x']
        self.assertEqual((m['name'], m['joined'], m.get('proof')), ('renamed', 1101, 'pf'), '改名字：加入的时间、加入时的认证码照旧')
        row = [r for r in st.team_members(later)[1]['members'] if r['dev'] == '01' * 8][0]
        self.assertEqual((row['name'], row['proof']), ('renamed', 'pf'), '管理员照旧核对得了它的公钥')
        self.assertEqual(st.team_revoke('05' * 8)[0], 200)
        # 状态文件手改坏了：样子不对的邀请、设备不要，照常启动、照常回话
        raw = json.loads((self.d / 's1' / 'state.json').read_text())
        raw['team']['invites'].update({'x': 1, 'y': {'dev': '06' * 8}, 'z': {'dev': 'nope', 'exp': 9}})
        raw['team']['spent'] = {'x': 'soon'}
        raw['machines'].update({'SHA256:bad': {'name': 'no-dev', 'key': self.pubs['c']}, 'SHA256:worse': 7})
        (self.d / 's1' / 'state.json').write_text(json.dumps(raw))
        st = self.store('s1', team_root=root)
        self.assertEqual((st.data['team']['invites'], st.data['team']['spent'], sorted(st.data['machines'])), ({}, {}, ['SHA256:x']))
        self.assertEqual((st.team_members(later)[0], st.invite_state('aa' * 16, later)), (200, None))
        # 没用掉的邀请最多 MAX_INVITES 个（过期的不算）
        st = self.store('s2', team_root=root)
        self.assertEqual(st.team_invite(H('00' * 16), 'ff' * 8, 60, 0)[0], 200)
        for i in range(srvmod.MAX_INVITES):
            self.assertEqual(st.team_invite(H('%032x' % i), '%016x' % i, 3600, 100)[0], 200, i)
        self.assertEqual(st.team_invite(H('ee' * 16), 'ee' * 8, 3600, 100), (507, {'error': 'full'}))
        # 配置加起来的上限
        big = {'seq': 1, 'ct': blob(srvmod.MAX_TEAM_DOC), 'sig': 'AAAA'}
        n = srvmod.TEAM_DOCS_BYTES // srvmod.MAX_TEAM_DOC
        with mock.patch.object(srvmod, 'MAX_INVITES', 10 ** 6):
            st = self.store('s3', team_root=root)
            for i in range(n + 1):
                st.team_invite(H('%032x' % i), '%016x' % i, 3600, 100)
            for i in range(0, n, 32):
                self.assertEqual(st.team_put({'%016x' % k: big for k in range(i, min(i + 32, n))}, 100)[0], 200, i)
            self.assertEqual(st.team_put({'%016x' % n: big}, 100), (507, {'error': 'full'}))
            self.assertEqual(st.team_put({'%016x' % 0: dict(big, seq=2)}, 100)[0], 200, '换掉已有的一份不算多')
        # 状态文件是以前别的服务器的（3.5.3 以前的自己的同步服务器、3.6.0 以前的维护者的服务器）就从头开始，各台设备的配置也删掉
        st = self.store('s1', team_root=root)
        self.assertEqual((len(st.data['machines']), len(st.docs['docs'])), (1, 1), '同一个根公钥：接着用')
        gen = st.gen()
        # 3.5.3 以前的自己的同步服务器（--mode own）留下的状态文件：从头开始（那时加入的设备、存的密文都不要了）
        raw = json.loads((self.d / 's1' / 'state.json').read_text())
        raw.pop('team')
        raw['own'] = {'seed': '0' * 64, 'admit': '0' * 64, 'gen': 'ab' * 16}
        (self.d / 's1' / 'state.json').write_text(json.dumps(raw))
        st = self.store('s1', team_root=root)
        self.assertEqual((st.data['machines'], st.docs['docs'], 'own' in st.data), ({}, {}, False))
        self.assertNotIn(st.gen(), (gen, 'ab' * 16))
        self.assertIn('以前是自己的同步服务器（3.5.3 以前的 --mode own）：以前的 1 台设备', err.getvalue())
        self.assertEqual(json.loads((self.d / 's1' / 'team-docs.json').read_text())['docs'], {})
        self.assertNotIn('own', json.loads((self.d / 's1' / 'state.json').read_text()))
        self.assertEqual(st.team_enroll({'01' * 8: ('SHA256:x', self.pubs['a'], 'mac')}, 5)[0], 200)
        # 3.6.0 以前的维护者的服务器（--mode maintainer：发布密钥签的设备清单）留下的：清单里的设备不是这个 Team 的，存的密文也不要了
        m = self.d / 'm1'
        m.mkdir()
        (m / 'state.json').write_text(json.dumps({'v': 1, 'seq': 7, 'machines': {'SHA256:m': {'name': 'old-mac', 'key': self.pubs['b']}},
                                                  'beats': {}, 'units': {}}))
        (m / 'keys.json').write_text(json.dumps({'v': 1, 'acc': {'ab' * 16: {'blob': blob(), 'fp': 'SHA256:m', 'name': 'old-mac', 'at': 1}}}))
        st = self.store('m1', team_root=root)
        self.assertEqual((st.data['machines'], st.data['seq'], st.keys['acc'], st.data['team']['root']), ({}, 0, {}, root))
        self.assertIn('以前是维护者的服务器（3.6.0 以前的 --mode maintainer）：以前的 1 台设备', err.getvalue())
        self.assertEqual(json.loads((m / 'keys.json').read_text())['acc'], {}, '存的密文也不要了')
        self.assertEqual(st.team_enroll({'01' * 8: ('SHA256:x', self.pubs['a'], 'mac')}, 5)[0], 200)
        self.assertFalse((m / 'team-docs.json').exists(), '没有配置要删：旁边不留一个空的 team-docs.json')
        # 第一次启动（什么都没有）：不说话，也不留一个空的 team-docs.json
        said = err.getvalue()
        self.assertFalse((self.d / 'none').exists())
        self.store('none', team_root=root)
        self.assertEqual(err.getvalue(), said)
        self.assertFalse((self.d / 'none' / 'team-docs.json').exists())

    def test_cli(self):
        """启动参数：根公钥从 --team-root 或者环境变量 CCA_TEAM_ROOT 来，不是 ssh-ed25519 公钥就不启动；--mode 只剩 team（安装脚本照旧
        写着它），以前的维护者的服务器（--mode maintainer、--release-key）起不来"""
        d = self.d / 'cli'
        d.mkdir()
        base = [sys.executable, '-I', str(SERVER), '--listen', '127.0.0.1:0', '--behind-proxy', '--state', str(d / 'state.json'), '--mode', 'team']
        for extra, env in (([], {}), (['--team-root', 'nope'], {}), (['--team-root', 'ssh-rsa AAAA'], {}), ([], {'CCA_TEAM_ROOT': 'x'})):
            r = subprocess.run(base + extra, env=dict(os.environ, **env), stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, timeout=20)
            self.assertEqual(r.returncode, 2, (extra, env))
            self.assertIn('根公钥', r.stderr.decode())
        (d / 'release.pub').write_text(self.pubs['evil'] + ' test-release\n')
        for old in (['--mode', 'maintainer'], ['--release-key', str(d / 'release.pub')]):
            r = subprocess.run(base[:-2] + old + ['--team-root', self.pubs['root']], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, timeout=20)
            self.assertEqual(r.returncode, 2, old)
            self.assertIn(old[0], r.stderr.decode())
        self.assertFalse((d / 'state.json').exists(), '起不来的时候什么都不写')
        p = subprocess.Popen(base + ['--port-file', str(d / 'port')], env=dict(os.environ, CCA_TEAM_ROOT=self.pubs['root'] + ' 备注'),
                             stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            wait_for('服务器起来', lambda: (d / 'port').exists(), 20)
            c = http.client.HTTPConnection('127.0.0.1', int((d / 'port').read_text()), timeout=10)
            c.request('GET', '/v1/health')
            self.assertEqual(json.loads(c.getresponse().read())['team'], self.tid)
        finally:
            p.terminate()
            p.wait(5)
            self.assertIn('Team 的同步服务器', p.stderr.read().decode())
            p.stderr.close()
        # 只写后面那一段（安装脚本、托管平台的环境变量里这样写：没有空格）；托管平台上（CCA_HUB_HOSTED=1）给了根公钥就是 Team 的服务器，
        # 不用再写 --mode
        (d / 'port').unlink()
        hosted = [sys.executable, '-I', str(SERVER), '--listen', '127.0.0.1:0', '--state', str(d / 'hosted.json'), '--port-file', str(d / 'port')]
        p = subprocess.Popen(hosted, env=dict(os.environ, CCA_HUB_HOSTED='1', CCA_TEAM_ROOT=self.pubs['root'].split()[1]),
                             stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            wait_for('服务器起来', lambda: (d / 'port').exists() and (d / 'port').read_text(), 20)
            c = http.client.HTTPConnection('127.0.0.1', int((d / 'port').read_text()), timeout=10)
            c.request('GET', '/v1/health')
            j = json.loads(c.getresponse().read())
            self.assertEqual((j['mode'], j['team']), ('team', self.tid))
        finally:
            p.terminate()
            p.wait(5)
            p.stderr.close()
        if not os.path.isdir('/data'):   # 托管平台上没挂存储（默认的状态文件在 /data 下）：不启动
            r = subprocess.run([sys.executable, '-I', str(SERVER)], env=dict(os.environ, CCA_HUB_HOSTED='1', CCA_TEAM_ROOT=self.pubs['root'].split()[1]),
                               stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30)
            self.assertEqual(r.returncode, 2)
            self.assertIn('没有 /data', r.stderr)

    def test_behind_proxy(self):
        """--behind-proxy（托管平台、反向代理后面）：只说 HTTP，不用证书；按 X-Forwarded-For 的最后一项限流（前面的是客户端自己写的，
        不算）；同一个 IP 的连接数不限"""
        d = self.d / 'proxied'
        d.mkdir()
        srv = Server(d, team_root=self.pubs['root'], proxied=True)
        self.addCleanup(srv.stop)
        st, j = srv.req('GET', '/v1/health')
        self.assertEqual((st, j['mode']), (200, 'team'))
        burst = 120   # Server 的 ip_bucket：Buckets(2, 120)

        def flood(xff):
            got = [srv.req('POST', '/v1/report', b'{}', headers={'X-Forwarded-For': xff(i)})[0] for i in range(burst + 5)]
            return got.count(429)
        self.assertEqual(flood(lambda i: '198.51.100.%d' % (i % 250)), 0, '各是各的 IP：不限')
        self.assertGreater(flood(lambda i: '203.0.113.%d, 192.0.2.7' % (i % 250)), 0, '最后一项一样：同一个 IP（前面的随便写也没用）')
        socks = [socket.create_connection(('127.0.0.1', srv.port), timeout=5) for _ in range(srvmod.MAX_CONN_IP + 2)]
        try:
            time.sleep(0.3)
            self.assertEqual(srv.req('GET', '/v1/health')[0], 200, '连过来的都是代理：不按同一个 IP 的连接数断开')
        finally:
            for x in socks:
                x.close()
        bad = subprocess.run([sys.executable, '-I', str(SERVER), '--behind-proxy', '--cert', 'x', '--key', 'y', '--mode', 'team',
                              '--team-root', self.pubs['root'], '--state', str(self.d / 'x.json')],
                             stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=30)
        self.assertEqual(bad.returncode, 2, '--behind-proxy 不用证书')
        self.assertIn('不用 --cert', bad.stderr.decode())


def ident(email):
    """账号目录的名字：邮箱里字母数字以外的换成 -（最多 20 个字符），加上邮箱摘要的前 6 位"""
    s = re.sub('[^a-z0-9]+', '-', email.lower()).strip('-')[:20].strip('-') or 'acct'
    return '%s-%s' % (s, hashlib.sha1(email.encode()).hexdigest()[:6])


def wait_for(what, cond, secs=60):
    """等到 cond() 给出真值（返回它）；一直等不到算失败（等得久只在出毛病时才碰得到：机器再忙也该等到的）"""
    end = time.monotonic() + secs
    while True:
        got = cond()
        if got:
            return got
        if time.monotonic() >= end:
            raise AssertionError('没等到：%s' % what)
        time.sleep(0.05)


class Team:
    """测试里的一个 Team：根密钥是 d 下的 key，测试拿着它扮演管理员——在服务器上登记、移除设备（照 server/README.md「Team 的同步
    服务器」自己签名）。设备那边不经邀请：Device.join 直接写好“加入了、同步到了配置”的样子（~/.cca/teams/<编号>/，见
    cli/src/core/team.rs）；邀请、加入、同步配置这一套在 tests/team.py"""
    def __init__(self, d, name='测试组', key='root', salt='00' * 16):
        self.d, self.name, self.salt = d, name, salt
        self.keyfile = d / key
        self.root = keygen(self.keyfile, 'cca-team-root')
        self.id = sw.team_id(self.root)
        self.lane = 'team.%s.' % self.id   # 设备上这个 Team 的服务器的状态文件的前缀（presence/、cost/ 里）

    def doc(self, url, pins, accounts=None, **more):
        """管理员给设备的配置（明文；样子见 cli/src/cmd/team/doc.rs 的 doc_problems）。默认只给 alice 的 Claude 账号"""
        if accounts is None:
            accounts = {'claude': {'alice@example.com': 'US-A'}}
        return dict({'v': 1, 'at': time.time(), 'name': self.name, 'salt': self.salt, 'server': {'u': url, 'p': list(pins)},
                     'accounts': accounts, 'aliases': {}, 'install_node': '', 'nodes': None}, **more)

    def admin(self, port, op, **extra):
        """管理员的请求：拿根密钥签名"""
        return sw.post(str(self.keyfile), port, op, extra)

    def only(self, port, *devs):
        """服务器上的名单正好是这几台设备：没登记的登记上（名字是设备的名字），名单里别的移除"""
        st, j = self.admin(port, 'team_members')
        assert st == 200, (st, j)
        keep = {d.team(self)['dev'] for d in devs}
        for m in j['members']:
            if m['dev'] not in keep:
                st, r = self.admin(port, 'team_revoke', dev=m['dev'])
                assert st == 200, (st, r)
        if devs:
            st, j = self.admin(port, 'team_enroll', devices={d.team(self)['dev']: {'pub': d.team(self)['pub'], 'name': d.name} for d in devs})
            assert st == 200 and sorted(j['names'].values()) == sorted(d.name for d in devs), (st, j)
            for d in devs:
                d.team_json(self, lambda t: t.update(gen=j['gen']))


class Device:
    """一台设备：自己的 HOME（~/.cca）；cca 的辅助程序在它里面跑。命令都在它自己的 bin/ 里（见上面几段）。join 以后它在那个 Team 里：
    keyfile、pub、fp 是它在头一个加入的 Team 里的密钥，lane 是那个 Team 的服务器的状态文件的前缀"""
    KEY = 'alice(alice@example.com)'
    ACCT = ident('alice@example.com')

    def __init__(self, home, name, table):
        self.name, self.home, self.cca = name, home, home / '.cca'
        self.cca.mkdir(parents=True)
        (home / 'bin').mkdir()
        self.keyfile = self.pub = self.fp = self.lane = None
        for cmd, script in (('curl', CURL), ('ps', PS), ('cca', SELF), ('security', NO_KEYCHAIN)):
            self.fake(cmd, script)
        (self.cca / 'config.json').write_text('{}')
        (self.cca / 'accounts.json').write_text(json.dumps(table))
        self.env = {'HOME': str(home), 'CCA_HOME': str(self.cca), 'CCA_TOOL': 'claude', 'PATH': '%s:/usr/bin:/bin' % (home / 'bin'),
                    'CCA_SELF': str(home / 'bin' / 'cca'), 'CCA_KEYSTORE': 'file', 'CCA_COST': 'off'}
        if 'TZ' in os.environ:
            self.env['TZ'] = os.environ['TZ']

    # --- Team ---
    def join(self, team, url, pins, doc=None, dev=None):
        """这台设备加入了 team、同步到了配置 doc（默认 team.doc(url, pins)）：直接写好 ~/.cca/teams/<编号>/ 下的 team.json、doc.json
        和这台设备在 Team 里的密钥（不经邀请；服务器上的名单由 team.only 登记）。dev：它在 Team 里的编号（默认随机）"""
        td = self.cca / 'teams' / team.id
        td.mkdir(parents=True, mode=0o700)
        keyfile = td / 'id_ed25519'
        pub = keygen(keyfile, 'cca-team@' + self.name)
        (td / 'team.json').write_text(json.dumps({'v': 1, 'id': team.id, 'name': team.name, 'url': url, 'pins': list(pins), 'root': team.root,
                                                  'role': 'member', 'dev': dev or secrets.token_hex(8), 'device': self.name,
                                                  'joined_at': time.time()}))
        self.set_doc(team, doc or team.doc(url, pins))
        index = self.cca / 'teams' / 'index.json'
        order = json.loads(index.read_text())['order'] if index.exists() else []
        index.write_text(json.dumps({'v': 1, 'order': order + [team.id]}))
        if self.keyfile is None:
            self.keyfile, self.pub, self.fp, self.lane = keyfile, pub, sw.fp_of(str(keyfile) + '.pub'), team.lane
        return self

    def team(self, team):
        """这台设备在 team 里的：{dev 编号, pub 公钥, fp 指纹, keyfile 私钥}"""
        td = self.cca / 'teams' / team.id
        return {'dev': json.loads((td / 'team.json').read_text())['dev'], 'pub': ' '.join((td / 'id_ed25519.pub').read_text().split()[:2]),
                'fp': sw.fp_of(str(td / 'id_ed25519.pub')), 'keyfile': td / 'id_ed25519'}

    def team_json(self, team, edit):
        """改一改这台设备记着的 team.json（服务器在哪、钉住的指纹……）"""
        p = self.cca / 'teams' / team.id / 'team.json'
        t = json.loads(p.read_text())
        edit(t)
        p.write_text(json.dumps(t))

    def set_doc(self, team, doc):
        """这台设备手里的配置换成 doc（像是同步到了新的一版：序号加一）"""
        p = self.cca / 'teams' / team.id / 'doc.json'
        seq = json.loads(p.read_text())['seq'] + 1 if p.exists() else 1
        p.write_text(json.dumps({'v': 1, 'seq': seq, 'at': time.time(), 'doc': doc}))

    def fake(self, name, script):
        p = self.home / 'bin' / name
        p.write_text('#!/bin/sh\n' + script)
        p.chmod(0o755)

    def run(self, *argv, stdin='', **env):
        return subprocess.run([HELPER] + list(argv), env=dict(self.env, **env), input=stdin, capture_output=True, text=True, timeout=120)

    def ok(self, *argv, **kw):
        """跑一条该成的命令，返回 stdout"""
        r = self.run(*argv, **kw)
        if (r.returncode, r.stderr) != (0, ''):
            raise AssertionError('%s 上 %s 没成：退出码 %d，%s%s' % (self.name, ' '.join(argv[:2]), r.returncode, r.stdout, r.stderr))
        return r.stdout

    def config(self, edit=None):
        """config.json；edit(c) 改一改再写回去"""
        c = json.loads((self.cca / 'config.json').read_text())
        if edit:
            edit(c)
            (self.cca / 'config.json').write_text(json.dumps(c))
        return c

    def acct(self, key=KEY):
        """账号目录"""
        return self.cca / 'accounts' / ident(key.split('(')[-1].rstrip(')'))

    def login(self, uuid, key=KEY):
        """这个账号（默认 alice）在这台设备上登录着（uuid 是它的登录身份）；None：没登录"""
        cd = self.acct(key) / 'claude'
        cd.mkdir(parents=True, exist_ok=True)
        email = key.split('(')[-1].rstrip(')')
        (cd / '.claude.json').write_text(json.dumps({'oauthAccount': {'accountUuid': uuid, 'emailAddress': email}} if uuid else {}))
        return cd / '.claude.json'

    # --- 记下备查的 API key ---
    def keyset(self, s, key=KEY):
        """在这台设备上记下（cca key 账号；key 从 stdin 给）：返回 stdout"""
        return self.ok('secret-set', key, stdin=s)

    def kicks(self):
        """后台起过几次同步（每次的参数；先后不保证）。cca 在后台起东西经 taskpolicy -c background，机器的核都占满时这样的进程
        一直轮不到跑（实测：占满多久它就等多久），所以起了、还没轮到往 kicks.log 记一笔的也算：进程表里命令行是这台设备的假 cca 的那些"""
        me = self.env['CCA_SELF'] + ' '
        table = subprocess.run(['/bin/ps', '-axww', '-o', 'pid=,command='], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL).stdout
        waiting = {}
        for line in table.decode('utf-8', 'replace').splitlines():
            pid, _, cmd = line.strip().partition(' ')
            if me in cmd:
                waiting[pid] = cmd.split(me, 1)[1].strip()
        # 先看进程表、再读文件：这中间记完退出了的在文件里；两边都有的（记了、还没退出）按进程号只算一次
        p = self.home / 'kicks.log'
        done = [line.partition(' ') for line in p.read_text().splitlines()] if p.exists() else []
        logged = {pid for pid, _, _ in done}
        return [args for _, _, args in done] + [args for pid, args in sorted(waiting.items()) if pid not in logged]

    def keys_json(self):
        """头一个加入的 Team 的服务器的那一份 keys.json（presence/team.<编号>.keys.json）"""
        p = self.cca / 'presence' / (self.lane + 'keys.json')
        return json.loads(p.read_text()) if p.exists() else None

    def sync(self, mode='force'):
        """同步一次记下备查的 API key（cca _key-sync）：没同步成的原因，成了是 ''；没开设备统计、还不到时候、有一轮正在跑是 None"""
        before = (self.keys_json() or {}).get('synced')
        self.ok('key-sync', *([mode] if mode else []))
        j = self.keys_json()
        return None if j is None or j.get('synced') == before else j.get('error', '')

    def stored(self, key=KEY):
        """这台设备上记下备查的那一把：(0, key)；没有是 (44, '')——照密钥文件（~/.cca/secrets/<服务名>，CCA_KEYSTORE=file）或者假钥匙串里的"""
        svc = 'cca-ref-' + ident(key.split('(')[-1].rstrip(')'))
        for p, read in ((self.cca / 'secrets' / svc, lambda t: json.loads(t)['key']), (self.home / 'keychain' / svc / 'reference', str)):
            if p.exists():
                return 0, read(p.read_text())
        return 44, ''

    def hint(self):
        """config.json 里记着的这一把（App 的账号页照它说，不读钥匙串：cli/src/cmd/gui.rs 的单元测试 api_key_like_python_test）：
        {saved 存了没有, hint 掩码, from_device 从哪台设备同步来的, saved_epoch 什么时候记下的}"""
        c = self.config()
        h = c.get('key_hints', {}).get(self.ACCT, {})
        return {'saved': self.ACCT in c.get('ref_keys', []), 'hint': h.get('hint', ''), 'from_device': h.get('from'), 'saved_epoch': h.get('at')}

    # --- 发给服务器的 ---
    def sent(self, op=None):
        """这台设备发过、服务器回过了（或者没发出去）的请求，按先后：{op 哪个接口, code 服务器回的状态码（没发出去是 0）,
        reply 它回的内容（没有是 {}）, done 什么时候回来的, 加上请求里的各项}"""
        out = []
        for p in sorted((self.home / 'sent').glob('*.code'), key=lambda p: int(p.stem)) if (self.home / 'sent').exists() else []:
            reply = p.with_suffix('.reply').read_text().strip()
            out.append(dict(json.loads(p.with_suffix('.json').read_text()), op=p.with_suffix('.op').read_text(), code=int(p.read_text() or 0),
                            reply=json.loads(reply) if reply else {}, done=p.stat().st_mtime))
        return [r for r in out if op in (None, r['op'])]

    def post(self, port, op, extra):
        """照文档自己签一个请求发给服务器（不经 cca：扮演这台设备上别的程序、有问题的程序）：拿它在头一个加入的 Team 里的密钥"""
        return sw.post(str(self.keyfile), port, op, extra)

    # --- 折合金额 ---
    def cost_local(self, who, rows, email='alice@example.com'):
        """这台设备以前统计好的（cost/local.json）：身份 who 的小时数据。没有会话记录、没有 scan.json 时统计照它接上"""
        (self.cca / 'cost').mkdir(exist_ok=True)
        (self.cca / 'cost' / 'local.json').write_text(json.dumps({'v': 1, 'build': 'x', 'who': {'claude:' + who: email},
                                                                  'acc': {'claude:' + who: rows}, 'um': {}, 'orphan': {}}))

    def cost_sync(self, force=True):
        """cca cost sync [--now] --json：统计一次、和别的设备同步；返回同步的结果 {state, message, checked_epoch}"""
        return json.loads(self.ok('cost-sync', '--json', *(['--now'] if force else [])))['sync']

    def cost(self, name):
        """cost/ 下的文件；Team 的服务器的那几份带着前缀（self.lane + 'remote.json' 这样写）"""
        return json.loads((self.cca / 'cost' / name).read_text())

    def cost_view(self, team, accounts=(), sync=()):
        """App 看到的（_gui state 的 cost）：经辅助程序只给测试用的 _cost-view 看。team 是同步走的那个 Team 的编号；accounts 的行是
        [工具, 键, usage_weeks, 谁给的（Team 的编号，这台设备自己的是 False）, 登录着没有]，sync 的行是 [slot, 谁给的, 登录着没有]"""
        spec = {'config': {}, 'team': team, 'accounts': [list(a) for a in accounts], 'sync': [list(s) for s in sync], 'tools': ['claude']}
        return json.loads(self.ok('_cost-view', repr(time.time()), json.dumps(spec)))


class FreshServer(Server):
    """ClientTest 每个用例重起一次的服务器：5 秒还没起来（机器忙）接着等；起不来、停不下来也不留下进程"""
    def __init__(self, d, port=0, team_root=None):
        try:
            try:
                super().__init__(d, ttl='60', idle='600', port=port, team_root=team_root)
            except FileNotFoundError:   # 端口文件还没有
                self.port = int(wait_for('服务器起来', lambda: (d / 'port').exists() and (d / 'port').read_text()))
        except BaseException:
            if getattr(self, 'proc', None) is not None:
                self.proc.kill()
                self.proc.wait()
            raise

    def stop(self):
        try:
            return super().stop()
        finally:
            if self.proc.poll() is None:
                self.proc.kill()
                self.proc.wait()


class Sessions:
    """开着的命令行会话、起上报进程、看它转了几圈（ClientTest、tests/team.py 的 TeamTest 共用）"""
    def session(self, dev, key=Device.KEY, pid=SESSION_PID, opened=100, tty='??', started=T0_EPOCH, running=True):
        """这台设备上开着一个命令行会话：账号目录里的登记（cca 启动账号时写的 cli-<进程号>.json，started 是 cca 的启动时间），
        假 ps 的进程表里有这个进程（running；tty 是它的终端，?? 是没有）。返回它是什么时候开的"""
        d, since = dev.acct(key), time.time() - opened
        d.mkdir(parents=True, exist_ok=True)
        (d / ('cli-%d.json' % pid)).write_text(json.dumps({'v': 1, 'tool': 'claude', 'id': d.name, 'pid': pid, 'started': started, 'since': since}))
        if running:
            with open(dev.home / 'ps.out', 'a') as f:
                f.write('%5d %5d %-8s %9s %s %s\n' % (pid, 1, tty, '0:00.50', T0, 'claude'))
        return since

    def agent(self, dev):
        """起这台设备的上报进程（cca _presence-agent）。用例结束时一定停掉：只停这里起的这一个进程"""
        with open(dev.home / 'agent.err', 'w') as err:
            p = subprocess.Popen([HELPER, 'presence-agent'], env=dev.env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=err)

        def stop():
            if p.poll() is None:
                p.terminate()
                try:
                    p.wait(15)
                except subprocess.TimeoutExpired:
                    p.kill()
                    p.wait()
        self.addCleanup(stop)
        return p

    def rounds(self, dev):
        """上报进程看了几次进程表（每转一圈看一次）"""
        p = dev.home / 'ps.log'
        return len(p.read_text().splitlines()) if p.exists() else 0

    def more_rounds(self, devs, n=3):
        """等这几台设备的上报进程各自再转 n 圈"""
        start = [self.rounds(d) for d in devs]
        wait_for('上报进程再转 %d 圈' % n, lambda: all(self.rounds(d) >= s + n for d, s in zip(devs, start)))

    def status(self, dev, lane):
        """上报进程最近一次没报到某个 Team 的服务器（lane 是那一路的前缀 team.<编号>.）的原因（cca devices 显示）"""
        p = dev.cca / 'presence' / (lane + 'status.json')
        return json.loads(p.read_text()).get('error', '') if p.exists() else None


class ClientTest(Sessions, unittest.TestCase):
    """客户端（cca 的辅助程序）：账号摘要、上报、cca devices、共享记下的 API key、折合金额的同步，经 curl + 公钥钉住连真的 Team 的
    同步服务器。设备都在 Team「测试组」里（它给 alice 的 Claude 账号；bob 是 alice-mbp 自己的账号）。只能在进程内测的几处在辅助程序
    自己的单元测试里（cargo test），下面各留了一行说它在哪"""
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='cca-presence-client-')
        d = cls.d = Path(cls.tmp.name).resolve()
        make_cert(d)
        cls.team = Team(d)
        cls.srv = FreshServer(d, team_root=cls.team.root)
        cls.pin = openssl_pin(d / 'cert.pem')

    @classmethod
    def tearDownClass(cls):
        if cls.srv is not None:
            cls.srv.stop()
        cls.tmp.cleanup()

    def setUp(self):
        # 每个用例一个新的服务器进程（名单、存的密文都在文件里，接着用）：限流的配额从头算
        cls = type(self)
        old, cls.srv = cls.srv, None
        if old is not None:
            old.stop()
        cls.srv = FreshServer(self.d, team_root=self.team.root)
        self.key = Device.KEY
        self.salt, self.lane, self.tid = self.team.salt, self.team.lane, self.team.id
        self.aid = sw.aid_of(self.salt, 'claude', 'alice@example.com')
        self.devices = []
        self.dev = self.device('alice-mbp', table={self.key: 'US-A', 'bob@example.com': 'US-B'})
        self.addCleanup(self.no_keychain)

    def no_keychain(self):
        self.assertEqual([d.name for d in self.devices if (d.home / 'security.log').exists()], [], '调到了 security（钥匙串）')

    def doc(self, **over):
        """Team 给设备的配置（服务器是这一次的）"""
        return self.team.doc('https://127.0.0.1:%d' % self.srv.port, [self.pin], **over)

    def device(self, name, uuid=None, table=None):
        """一台设备：自己的 ~/.cca，在 Team 里、同一份配置；uuid 不是 None 时 alice 在它上面登录着"""
        d = Device(self.d / ('%s-%s' % (name, secrets.token_hex(4))), name, table or {self.key: 'US-A'})
        d.join(self.team, 'https://127.0.0.1:%d' % self.srv.port, [self.pin])
        if uuid:
            d.login(uuid)
        self.devices.append(d)
        return d

    def machines(self, *devs):
        """服务器上的名单正好是这几台（管理员拿根密钥登记、移除；名字就是设备的名字）"""
        self.team.only(self.srv.port, *devs)

    def view(self, dev, *args, rc=0, **env):
        """cca devices …：(stdout, 这次问服务器的那个请求)"""
        before = len(dev.sent())
        r = dev.run('presence-view', *args, **env)
        self.assertEqual((r.returncode, r.stderr), (rc, ''), r.stdout)
        return r.stdout, (dev.sent()[before:] or [None])[0]

    def test_identity(self):
        dev = self.dev
        fp = subprocess.run(['ssh-keygen', '-l', '-E', 'sha256', '-f', str(dev.keyfile) + '.pub'],
                            stdout=subprocess.PIPE, text=True, check=True).stdout.split()[1]
        self.machines(dev)
        _, asked = self.view(dev, '--json')
        self.assertEqual((asked['key'], asked['code']), (fp, 200), '和 ssh-keygen -l 的指纹一样（服务器照登记的公钥认它）')
        [a] = asked['accounts']
        self.assertRegex(a, r'^[0-9a-f]{32}$')
        self.assertEqual(a, sw.aid_of(self.salt, 'claude', 'alice@example.com'), '账号摘要和文档写的一样')
        (dev.cca / 'accounts.json').write_text(json.dumps({'alice(Alice@Example.com)': 'US-A'}))
        self.assertEqual(self.view(dev, '--json')[1]['accounts'], [a], '邮箱不分大小写')
        (dev.cca / 'codex.json').write_text(json.dumps({'alice@example.com': 'US-A'}))
        dev.set_doc(self.team, self.doc(accounts={'claude': {'alice@example.com': 'US-A'}, 'codex': {'alice@example.com': 'US-A'}}))
        [x] = self.view(dev, '--json', CCA_TOOL='codex')[1]['accounts']
        self.assertNotEqual(a, x, 'Claude 账号和 Codex 账号不撞')
        self.assertEqual(x, sw.aid_of(self.salt, 'codex', 'alice@example.com'))
        dev.set_doc(self.team, self.doc(salt='11' * 16))
        [y] = self.view(dev, '--json')[1]['accounts']
        self.assertNotEqual(a, y, '换了盐就认不出')
        self.assertEqual(y, sw.aid_of('11' * 16, 'claude', 'alice@example.com'))
        # 用不了的几种（cca devices 说原因，退出码 2）
        dev.config(lambda c: c.update(presence=False))
        self.assertIn('关掉了', self.view(dev, rc=2)[0])
        dev.config(lambda c: c.pop('presence'))
        doc = dev.cca / 'teams' / self.tid / 'doc.json'
        doc.rename(doc.with_name('doc.bak'))   # 配置还没同步下来
        self.assertIn('「测试组」的配置还没同步下来（cca team sync）', self.view(dev, rc=2)[0])
        doc.with_name('doc.bak').rename(doc)
        dev.keyfile.rename(dev.keyfile.with_name('id.bak'))   # 这台设备在 Team 里的密钥不见了
        self.assertIn('这台设备用不了「测试组」的同步服务器（密钥不见了', self.view(dev, rc=2)[0])
        self.assertEqual(len(dev.sent()), 4, '用不了的时候不问服务器')

    def test_scan_cli_idle_from_tty(self):
        """命令行会话的闲置时间 = 现在 - 它终端的最后一次输入 / 输出；登记还在、进程没了或者进程号被别的进程重用了的不算。
        拿固定的时刻逐项核对：cli/src/cmd/presence.rs 的单元测试 scan_cli_idle_from_tty"""
        dev = self.dev
        self.machines(dev)
        master, slave = pty.openpty()
        self.addCleanup(os.close, master)
        self.addCleanup(os.close, slave)
        tty = os.ttyname(slave)
        now = time.time()
        os.utime(tty, (now - 120, now - 200))   # 最后一次输入 120 秒前、输出 200 秒前
        since = self.session(dev, opened=300, tty=tty[len('/dev/'):])   # ps 的写法：macOS 是 ttys005，Linux 是 pts/5
        self.session(dev, pid=SESSION_PID - 1, started=T0_EPOCH + 3)   # 进程号被重用（启动时间对不上）
        self.session(dev, pid=SESSION_PID - 2, running=False)   # cca 被 kill -9 留下的登记：进程没了
        self.agent(dev)
        [report] = wait_for('上报进程报上去', lambda: dev.sent('report'))
        self.assertEqual((report['code'], list(report['units'])), (200, [self.aid]))
        [[opened, idle]] = report['units'][self.aid]['cli']   # 只有终端上的那一个算
        self.assertTrue(opened >= 300 and abs(since + opened - report['done']) < 15, '开了多久')
        self.assertTrue(idle >= 120 and abs(now - 120 + idle - report['done']) < 15, '闲置 = 现在 - 最后一次输入 / 输出')

    # test_scan_app_cpu（桌面 App 看进程树的 CPU：隔 30 秒采样、过了阈值才算有动静，别的资料目录不算）：
    #   cli/src/cmd/presence.rs 的单元测试 scan_app_cpu_like_python_test（还有 scan_app_sampling_like_python、app_proc_re_matches_both_spellings_and_escapes）

    def test_post_and_devices(self):
        dev = self.dev
        out, asked = self.view(dev, '--json', rc=1)
        self.assertEqual((asked['code'], asked['reply'].get('error')), (403, 'unknown_device'), '服务器的名单里还没有这台设备')
        self.assertIn('不认这台设备', out)
        self.machines(dev)
        since = self.session(dev)   # 100 秒前开的
        self.session(dev, 'bob@example.com', SESSION_PID - 1)   # 这台设备自己的账号（Team 没给的）也开着会话
        self.agent(dev)
        [report] = wait_for('上报进程报上去', lambda: dev.sent('report'))
        self.assertEqual((report['code'], list(report['units']), report['end']), (200, [self.aid], None), '只报 Team 给的官方账号')
        [[opened, idle]] = report['units'][self.aid]['cli']
        self.assertEqual((idle, report['units'][self.aid]['app']), (0, None), '没有终端：在跑就算在干活；没开桌面 App')
        self.assertTrue(opened >= 100 and abs(since + opened - report['done']) < 15, '开了多久：到上报的时候为止')
        doc = json.loads(self.view(dev, '--json')[0])
        self.assertEqual((doc['kind'], doc['me'], doc['tool']), ('devices', 'alice-mbp', 'claude'))
        [a] = doc['accounts']
        self.assertEqual((a['key'], a['hub'], a['team']), (self.key, 'team', self.tid))
        [d] = a['devices']
        self.assertEqual((d['name'], d['me'], d['state'], d['cli'], d['app']), ('alice-mbp', True, 'in_use', 1, False))
        self.assertTrue(abs(d['since_epoch'] - since) < 15 and abs(d['last_used_epoch'] - report['done']) < 15,
                        '时间换算成这台 Mac 的时钟（什么时候开的、最近一次用是上报的那一刻）')
        out = self.view(dev)[0]
        self.assertIn('alice-mbp（这台 Mac）  在用 · 命令行里 1 个会话 · ', out)
        self.assertIn(' 起', out)
        self.assertIn('是这台设备自己的账号', self.view(dev, 'bob', rc=2)[0])
        n = len(dev.sent())
        dev.team_json(self.team, lambda t: t.update(pins=['sha256//' + 'A' * 43 + '=']))
        out = self.view(dev, rc=1)[0]
        self.assertIn('钉住的指纹对不上', out, '证书不是钉住的那张：不发')
        self.assertEqual([r['code'] for r in dev.sent()[n:]], [0])
        dev.team_json(self.team, lambda t: t.update(url='https://127.0.0.1:9', pins=[self.pin]))
        self.assertIn('连不上 Team 的同步服务器（127.0.0.1:9）', self.view(dev, rc=1)[0])

    # test_past_time（cca devices 里过去的时刻怎么写：今天、昨天、星期几、日期）：cli/src/cmd/presence.rs 的单元测试 past_time_like_python_test

    def test_agent_backoff(self):
        """报不出去（服务器连不上、不认这台设备）：按退避再试，心跳到期、会话变了都不会马上重发。
        隔多久再试的那几个数（心跳 60 秒上下，退避 10、20 秒……，不认这台设备时 10 分钟）：cli/src/cmd/presence.rs 的单元测试 retry_backs_off"""
        ok, denied, down = self.dev, self.device('bob-mini', 'u-alice-0007'), self.device('carol-air', 'u-alice-0007')
        recent = self.device('dan-air', 'u-alice-0007')
        self.machines(ok, recent)   # bob-mini 不在名单里（403）；carol-air 连不上服务器
        ok.login('u-alice-0007')
        down.team_json(self.team, lambda t: t.update(url='https://127.0.0.1:9'))
        (recent.cca / 'presence').mkdir(mode=0o700)   # dan-air 半分钟前刚同步过记下的 API key
        (recent.cca / 'presence' / (self.lane + 'keys.json')).write_text(json.dumps({'v': 1, 'acc': {}, 'synced': time.time() - 30}))
        everyone = (ok, denied, down, recent)
        for d in everyone:
            self.session(d)
            self.agent(d)
        for d, code in ((ok, 200), (denied, 403), (down, 0), (recent, 200)):
            [r] = wait_for('%s 的第一次上报' % d.name, lambda: d.sent('report'))
            self.assertEqual(r['code'], code)
        self.assertIn('不认这台设备', wait_for('没报成的原因', lambda: self.status(denied, self.lane)))
        self.assertIn('连不上 Team 的同步服务器', wait_for('没报成的原因', lambda: self.status(down, self.lane)))
        self.assertIn(self.status(ok, self.lane), (None, ''), '报成了：没有没报成的原因')
        # 开着会话：马上同步一次记下的 API key（不 force：最近同步过就不发）
        self.assertEqual([[r['code'] for r in d.sent('keys')] for d in everyone], [[200], [403], [0], []])
        for d in everyone:
            self.session(d, pid=SESSION_PID - 2)   # 会话变了
        self.more_rounds(everyone)
        gaps = lambda reports: [b['done'] - a['done'] for a, b in zip(reports, reports[1:])]   # 两次上报之间隔了多久
        sent = ok.sent('report')
        self.assertEqual([len(r['units'][self.aid]['cli']) for r in sent[:2]], [1, 2], '报成了的：会话变了马上再报')
        self.assertTrue(all(gap > 30 for gap in gaps(sent[1:])), '没到心跳的时候（60 秒上下）转多少圈都不再发')
        self.assertEqual(len(denied.sent('report')), 1, '服务器不认这台设备：转了几圈、会话变了都不重发（10 分钟后再试）')
        self.assertTrue(all(gap > 5 for gap in gaps(down.sent('report'))), '连不上：转了几圈、会话变了都不重发，退避到点（10 秒）才再试')
        self.assertEqual([len(d.sent('keys')) for d in everyone], [1, 1, 1, 0], '记下的 API key 隔 10 分钟才再同步，不是每圈都同步')

    # 共享 API key：官方账号记下备查的那一把，经服务器加密同步给登录了这个账号的其他设备
    K1, K2, K3 = 'sk-ant-api03-' + 'a' * 40, 'sk-ant-api03-' + 'b' * 50, 'sk-ant-api03-' + 'c' * 30
    HINT1, HINT2 = 'sk-ant-…aaaa', 'sk-ant-…bbbb'   # 它们的掩码

    def server_blob(self, aid):
        return json.loads((self.d / 'keys.json').read_text())['acc'][aid]['blob']

    def test_keyshare_crypto(self):
        """密文的格式：两把密钥由盐、工具、邮箱、登录身份派生；加密后认证（带着账号摘要）；明文补齐到 256 字节的整数倍。
        每次的 nonce 不同、形状不对的密文不认、认证过内容不对的不认（逐个）：cli/src/cmd/keyshare.rs 的单元测试
        seal_then_open_round_trips_with_a_random_nonce、open_refuses_junk_like_python_test"""
        dev, other, aid = self.dev, self.device('spec-air'), self.aid   # spec-air：照文档自己封、自己发的另一台设备
        self.machines(dev, other)
        (dev.cca / 'accounts.json').write_text(json.dumps({'alice(Alice@Example.com)': 'US-A'}))   # 邮箱不分大小写
        dev.login('u-1234-5678', 'alice(Alice@Example.com)')
        keyset = lambda s: dev.keyset(s, 'alice(Alice@Example.com)')
        stored = lambda: dev.stored('alice(Alice@Example.com)')
        acct = ident('Alice@Example.com')
        at = lambda: dev.config()['key_hints'][acct]['at']
        k = sw.derive(sw.KEYSHARE, self.salt, 'claude', 'alice@example.com', 'u-1234-5678')
        key = 'sk-ant-api03-' + 'x' * 60
        keyset(key)
        self.assertEqual(dev.sync(), '')
        b = self.server_blob(aid)
        self.assertEqual(sw.keyshare_open(k, aid, b), {'v': 1, 'key': key, 'at': at()}, '照文档能解开：密钥和文档写的一样')
        self.assertEqual(srvmod.parse_blob(b), b, '服务器收这个形状')

        def put(blob):
            """服务器上的那一份换成 blob，这台设备再同步一次"""
            self.assertEqual(other.post(self.srv.port, 'keys', {'put': {aid: blob}})[0], 200)
            self.assertEqual(dev.sync(), '')

        newer = {'v': 1, 'key': 'sk-ant-api03-' + 'y' * 60, 'at': time.time()}   # 比这台设备记下的新
        put(sw.keyshare_seal(k, aid, newer))
        self.assertEqual((stored(), at()), ((0, newer['key']), newer['at']), '照文档封的 cca 也认')
        # 认证不过的、内容不对的都不认：这台设备上的不动，服务器上的换回这台设备的（自己修好）
        bad = {'v': 1, 'key': 'sk-ant-api03-' + 'z' * 60, 'at': time.time()}   # 更新的：认了的话这台设备上的会换成它
        sealed = sw.keyshare_seal(k, aid, bad)
        c = bytearray(base64.b64decode(sealed['c']))
        c[5] ^= 1
        for what, blob in (
                ('没登录这个账号（身份不对）', sw.keyshare_seal(sw.derive(sw.KEYSHARE, self.salt, 'claude', 'alice@example.com', 'u-other-0000'), aid, bad)),
                ('换了盐', sw.keyshare_seal(sw.derive(sw.KEYSHARE, '11' * 16, 'claude', 'alice@example.com', 'u-1234-5678'), aid, bad)),
                ('同一个邮箱的 Codex 账号', sw.keyshare_seal(sw.derive(sw.KEYSHARE, self.salt, 'codex', 'alice@example.com', 'u-1234-5678'), aid, bad)),
                ('挪到别的账号上：认证不过', sw.keyshare_seal(k, 'b' * 32, bad)),
                ('改过的', dict(sealed, c=base64.b64encode(bytes(c)).decode())),
                ('版本不对', sw.keyshare_seal(k, aid, dict(bad, v=2))),
                ('key 太短', sw.keyshare_seal(k, aid, dict(bad, key='short'))),
                ('key 里有空格、中文', sw.keyshare_seal(k, aid, dict(bad, key='带空格 的 key')))):
            put(blob)
            self.assertEqual((stored(), at()), ((0, newer['key']), newer['at']), what)
            self.assertEqual(sw.keyshare_open(k, aid, self.server_blob(aid)), newer, what)
        # 删除和 key 一样长：服务器分不出来
        dev.ok('key-forget', 'alice(Alice@Example.com)')
        self.assertEqual(dev.sync(), '')
        t = self.server_blob(aid)
        gone = sw.keyshare_open(k, aid, t)
        self.assertEqual((gone['key'], stored()[0], gone['at'] > newer['at']), (None, 44, True))
        self.assertEqual(len(t['c']), len(b['c']), '删除和 key 一样长：服务器分不出来')
        keyset('k' * 4096)
        self.assertEqual(dev.sync(), '')
        lb = self.server_blob(aid)
        self.assertEqual(srvmod.parse_blob(lb), lb, '最长的 key（4096 个字符）也在服务器收的长度以内')
        self.assertEqual(sw.keyshare_open(k, aid, lb)['key'], 'k' * 4096)

    def test_keyshare(self):
        """两台登录了 alice 的设备（alice-mbp、bob-mini）和一台没登录的（carol-air）：记下、换掉、删掉都同步，新的胜出；
        服务器、没登录的设备看不到 key；重放旧的、伪造的不认，传回自己的（自己修好）；删账号只删这台设备上的"""
        a, K1, K2, K3 = self.dev, self.K1, self.K2, self.K3
        b = self.device('bob-mini', 'u-alice-0001')
        c = self.device('carol-air', None)
        self.machines(a, b, c)
        a.login('u-alice-0001')
        aid, acct = self.aid, Device.ACCT
        k = sw.derive(sw.KEYSHARE, self.salt, 'claude', 'alice@example.com', 'u-alice-0001')
        self.assertEqual((a.keyset(K1, 'bob@example.com'), a.ok('key-share', 'bob@example.com')), ('', ''),
                         '这台设备自己的账号（Team 没给的）：只记在这台设备上，不同步')
        self.assertEqual(a.keyset(K1).strip(), 'shared', 'Team 给的账号、开着设备统计：说一声会同步')
        self.assertEqual(wait_for('后台起同步', a.kicks), ['_key-sync force'], '马上在后台同步（就这一次：bob@example.com 的那一把没起）')
        self.assertEqual(oct(os.stat(a.cca / 'presence' / (self.lane + 'keys.json')).st_mode & 0o777), '0o600')
        self.assertEqual(a.sync(), '')
        raw = (self.d / 'keys.json').read_text()
        for secret in (K1, 'alice@example.com', 'u-alice-0001', 'a' * 40):
            self.assertNotIn(secret, raw, '服务器上没有 key、邮箱、登录身份')
        self.assertEqual(list(json.loads(raw)['acc']), [aid])
        at1 = a.config()['key_hints'][acct]['at']
        self.assertEqual(sw.keyshare_open(k, aid, self.server_blob(aid))['at'], at1, '传上去的时间是记下的时间')
        self.assertEqual(c.sync(), '', '没登录 alice 的设备：不问也不传')
        self.assertEqual(c.sent(), [])
        self.assertFalse((c.cca / 'secrets').exists())
        self.assertEqual(b.sync(), '')
        self.assertEqual(b.stored(), (0, K1), '登录了 alice 的另一台设备取回来了')
        self.assertEqual((b.hint(), b.ok('key-share', self.key).strip(), b.keys_json().get('error')),
                         ({'saved': True, 'hint': self.HINT1, 'from_device': 'alice-mbp', 'saved_epoch': at1}, '1', None),
                         'App 照记下的说：从哪台设备来的、什么时候记下的；会同步、没有没同步成的原因')
        ha = a.hint()
        self.assertEqual((ha['from_device'], ha['saved_epoch']), (None, at1), '这台设备自己记下的：没有来源')
        blob1 = self.server_blob(aid)
        # bob-mini 换了一把：新的胜出，alice-mbp 取回来
        b.keyset(K2)
        self.assertEqual((b.sync(), a.sync()), ('', ''))
        self.assertEqual(a.stored(), (0, K2))
        ha = a.hint()
        self.assertEqual((ha['from_device'], ha['hint']), ('bob-mini', self.HINT2))
        self.assertEqual(a.sync(), '', '再同步一次：没有新的，什么都不变')
        self.assertEqual(a.stored(), (0, K2))
        # carol-air（在名单里、没登录 alice）把旧的那一份传回去（重放），再传一份伪造的：都不认，传回新的
        for bad in (blob1, {'n': blob1['n'], 'c': base64.b64encode(secrets.token_bytes(256)).decode(), 't': blob1['t']}):
            self.assertEqual(c.post(self.srv.port, 'keys', {'put': {aid: bad}})[0], 200)
            self.assertEqual(a.sync(), '')
            self.assertEqual(a.stored(), (0, K2), '旧的、认证不过的不认')
            self.assertEqual(sw.keyshare_open(k, aid, self.server_blob(aid))['key'], K2,
                             '服务器上的旧了、坏了：传回这台设备的（自己修好）')
        # alice-mbp 删掉：bob-mini 上同步来的也删掉
        a.ok('key-forget', self.key)
        self.assertEqual(a.stored()[0], 44)
        self.assertEqual(a.sync(), '')
        self.assertEqual(len(self.server_blob(aid)['c']), len(blob1['c']), '删除和 key 一样长')
        self.assertEqual(b.sync(), '')
        self.assertEqual(b.stored()[0], 44, '删掉也同步')
        self.assertNotIn(acct, b.config().get('ref_keys') or [])
        self.assertFalse(b.hint()['saved'])
        # 记下 K3、两边都有了；bob-mini 删掉这个账号（cca rm）：只删它自己的，不同步，也不再取回来
        a.keyset(K3)
        self.assertEqual((a.sync(), b.sync()), ('', ''))
        self.assertEqual(b.stored(), (0, K3))
        (b.cca / 'accounts.json').write_text('{}')
        b.ok('secret-del', self.key)
        self.assertEqual((b.sync(), a.sync()), ('', ''))
        self.assertEqual((b.stored()[0], a.stored()), (44, (0, K3)))
        # 账号加回来、还登录着：服务器上的取回来（钥匙串里的被删了也一样）
        (b.cca / 'accounts.json').write_text(json.dumps({self.key: 'US-A'}))
        self.assertEqual(b.sync(), '')
        self.assertEqual(b.stored(), (0, K3))
        # 退出登录了：不问也不传（解不开）；本机的 key 还在
        b.login(None)
        b.keyset(K1)
        n = len(b.sent())
        self.assertEqual(b.sync(), '')
        self.assertEqual(len(b.sent()), n)
        self.assertEqual(a.stored(), (0, K3))
        self.assertEqual(a.sync(), '')
        self.assertEqual(a.stored(), (0, K3), '没登录时记下的：登录以前不传')
        b.login('u-alice-0001')
        self.assertEqual((b.sync(), a.sync()), ('', ''))
        self.assertEqual(a.stored(), (0, K1), '登录以后：按记下的时间比，新的胜出')
        self.assertEqual(wait_for('后台起同步', lambda: len(a.kicks()) >= 3 and a.kicks()), ['_key-sync force'] * 3,
                         '在这台设备上记下、删掉（K1、删掉、K3）各起一次同步；照服务器上的改不起')

    def test_keyshare_before_presence(self):
        """没开设备统计时记下的（不记 keys.json）、旧版本记下的（key_hints 里没有时间）：开了以后按记下的时间算"""
        a, K1, K2 = self.dev, self.K1, self.K2
        b = self.device('bob-mini', 'u-alice-0002')
        self.machines(a, b)
        a.login('u-alice-0002')
        aid, acct = self.aid, Device.ACCT
        (a.cca / 'config.json').write_text(json.dumps({'presence': False}))
        self.assertEqual(a.keyset(K1), '', '关掉了设备统计：不说会同步')
        self.assertFalse((a.cca / 'presence').exists(), '也不建 presence 目录')
        self.assertIsNone(a.sync())
        a.config(lambda c: c.pop('presence'))
        # bob-mini 的是旧版本记下的：key_hints 里没有时间，按密钥文件的修改时间算（比 alice-mbp 记下的早）
        (b.cca / 'config.json').write_text(json.dumps({'presence': False}))   # 记的时候不经设备统计（keys.json 里没有它）
        b.keyset(K2)
        b.config(lambda c: (c.pop('presence'), c['key_hints'][acct].pop('at')))
        self.assertFalse((b.cca / 'presence').exists())
        old = time.time() - 86400
        os.utime(b.cca / 'secrets' / ('cca-ref-' + acct), (old, old))
        self.assertEqual((b.sync(), a.sync()), ('', ''))
        self.assertEqual(a.stored(), (0, K1), 'alice-mbp 记下的新：不动')
        self.assertEqual(b.sync(), '')
        self.assertEqual(b.stored(), (0, K1), 'bob-mini 的旧：换成新的')
        self.assertEqual(sw.keyshare_open(sw.derive(sw.KEYSHARE, self.salt, 'claude', 'alice@example.com', 'u-alice-0002'), aid, self.server_blob(aid))['key'], K1)
        self.assertEqual(a.kicks() + b.kicks(), [], '关掉了设备统计时记下的：不起同步')

    def test_keyshare_bad_files(self):
        """账号的 .claude.json 读不出（半写、手改坏了）：当作没登录，这一轮不同步它，不退出；上报进程里同步出的错（config.json 读不了时
        退出的那种也算）不停掉上报；其他设备上删掉的、这台设备的钥匙串里没删掉（锁着）：不记成删掉了，下一轮再删"""
        a, K1 = self.dev, self.K1
        b = self.device('bob-mini', 'u-alice-0005')
        b.fake('security', KEYCHAIN)   # bob-mini 的 key 存在（假的）钥匙串里
        del b.env['CCA_KEYSTORE']
        self.machines(a, b)
        cj = a.login('u-alice-0005')
        a.keyset(K1)
        good = cj.read_text()
        srv = lambda: (self.d / 'keys.json').read_text() if (self.d / 'keys.json').exists() else None
        before = srv()
        cj.write_text('{"oauthAccount": ')
        self.assertEqual(a.sync(), '', '读不出登录身份：当作没登录，不退出（以前 die：上报进程跟着退出）')
        self.assertEqual(srv(), before, '没传')
        cj.write_text(good)
        self.assertEqual((a.sync(), b.sync()), ('', ''))
        self.assertEqual(b.stored(), (0, K1))
        a.ok('key-forget', self.key)
        self.assertEqual(a.sync(), '')
        raw = b.keys_json()['acc']
        (b.home / 'keychain_stuck').write_text('')   # 钥匙串锁着：删不掉
        err = b.sync()
        self.assertIn('其他设备上删掉的 API key 没能从这台设备的钥匙串里删掉', err)
        self.assertEqual(b.stored(), (0, K1))
        self.assertEqual(b.keys_json()['acc'], raw, '没删掉：不记成删掉了')
        (b.home / 'keychain_stuck').unlink()
        self.assertEqual(b.sync(), '', '下一轮再删')
        self.assertEqual(b.stored()[0], 44)
        # 上报进程：同步 API key 时出错退出的那种（config.json 读不了）也接着报
        (a.cca / 'config.json').write_text('{oops')
        j = a.keys_json()
        (a.cca / 'presence' / (self.lane + 'keys.json')).write_text(json.dumps(dict(j, synced=j['synced'] - 3600)))   # 上次同步是一小时以前的事：这一圈该同步了
        self.session(a)
        self.agent(a)
        [r] = wait_for('照常上报', lambda: a.sent('report'))
        self.assertEqual((r['code'], list(r['units'])), (200, [self.aid]))
        self.assertIn('config.json', (a.home / 'agent.err').read_text(), '同步出的错说了一声')

    def test_keyshare_force_waits(self):
        """记下、删掉之后的同步（force）碰上正在跑的一轮：等它跑完再同步，不放弃（不然这次的改动要等到下一轮才传出去）"""
        dev = self.dev
        self.machines(dev)
        dev.login('u-alice-0004')
        dev.keyset(self.K1)
        lock = dev.cca / 'presence' / (self.lane + 'keys-sync.lock')
        held = open(lock, 'a')
        fcntl.flock(held, fcntl.LOCK_EX)
        release = threading.Timer(0.6, held.close)
        release.start()
        t0 = time.monotonic()
        self.assertEqual(dev.sync(), '')
        waited = time.monotonic() - t0
        release.join()
        self.assertGreaterEqual(waited, 0.5, '等正在跑的那一轮放锁')
        self.assertEqual([sorted(set(r) & {'get', 'put'}) for r in dev.sent()], [['get'], ['put']], '放锁以后照常同步：问一次，传上去')
        # 上次同步是一小时以前的事（到时候了）：不是 force 的碰上正在跑的一轮就走
        j = dev.keys_json()
        (dev.cca / 'presence' / (self.lane + 'keys.json')).write_text(json.dumps(dict(j, synced=j['synced'] - 3600)))
        held = open(lock, 'a')
        fcntl.flock(held, fcntl.LOCK_EX)
        try:
            t0 = time.monotonic()
            self.assertIsNone(dev.sync(''), '不是 force 的：有一轮在跑就走')
            self.assertLess(time.monotonic() - t0, 15)   # 等的话要等到放锁（这里一直不放：force 的那种等满 35 秒）
            self.assertEqual(len(dev.sent()), 2)
        finally:
            held.close()
        self.assertEqual(dev.sync(''), '', '没有别的在跑：到时候了就同步')

    # 折合金额：各台设备算自己的，经服务器交换密文
    def slots(self, aid):
        return json.loads((self.d / 'cost-slots.json').read_text())['acc'].get(aid, {})

    def test_cost_sync(self):
        """alice-mbp、bob-mini 登录着 alice，carol-air 没有：两台互相取到对方的，服务器上只有密文；伪造的、重放的不认；
        没变的不重传、不重取；移除的设备留着、标上；用过却没传过的设备说一声"""
        a = self.dev
        b = self.device('bob-mini', 'u-alice-0005')
        c = self.device('carol-air', None)
        self.machines(a, b, c)
        a.login('u-alice-0005')
        aid, lane, slot = self.aid, self.lane, 'claude:alice@example.com'
        hour = int(time.time() // 3600 * 3600) - 3600
        a.cost_local('u-alice-0005', {str(hour): [5000000, 2, 100, 10, 0, 0, 0]})
        b.cost_local('u-alice-0005', {str(hour - 86400): [3000000, 1, 50, 5, 0, 0, 0]})
        c.cost_local('u-carol-0001', {str(hour): [1000000, 1, 1, 1, 0, 0, 0]}, 'carol@example.com')
        r = a.cost_sync()
        self.assertEqual(r['state'], 'ok', r)
        raw = (self.d / 'cost-slots.json').read_text()
        for secret in ('alice@example.com', 'u-alice-0005', '5000000', str(hour)):
            self.assertNotIn(secret, raw, '服务器上没有邮箱、身份、金额、时间')
        fa = a.fp
        self.assertEqual(list(self.slots(aid)), [fa])
        self.assertEqual(b.cost_sync()['state'], 'ok')
        rb = b.cost(lane + 'remote.json')['acc'][slot]
        self.assertEqual([(e['name'], e['h'], e['revoked']) for e in rb.values()], [('alice-mbp', {str(hour): [5000000, 2, 100, 10, 0, 0, 0]}, False)],
                         'bob-mini 取到了 alice-mbp 的')
        self.assertEqual(c.cost_sync()['state'], 'skipped', 'carol-air 没登录 alice（解不开）：不问也不传')
        self.assertEqual(c.sent(), [])
        self.assertEqual(sorted(self.slots(aid)), sorted([fa, b.fp]))
        at_a = self.slots(aid)[fa]['at']
        n = len(a.sent())
        self.assertEqual(a.cost_sync()['state'], 'ok')
        posts = a.sent()[n:]
        self.assertEqual((len(posts), 'put' in posts[0]), (1, False), '这台设备的没变：只问，不传')
        self.assertEqual(self.slots(aid)[fa]['at'], at_a)
        ra = a.cost(lane + 'remote.json')['acc'][slot]
        self.assertEqual([e['name'] for e in ra.values()], ['bob-mini'])
        n = len(a.sent())
        self.assertEqual(a.cost_sync()['state'], 'ok')
        posts = a.sent()[n:]
        self.assertEqual(list(posts[0]['have'][aid].values()).count(None), 0)
        self.assertEqual(len(posts[0]['have'][aid]), 2, '带着已经有的认证码（自己的、bob-mini 的）：没变的服务器不回密文')
        [v] = a.cost_view(self.tid, [('claude', self.key, None, self.tid, True)])['accounts']
        self.assertEqual((v['summary']['days_usd'], [d['name'] for d in v['devices']], v['sync']['state']), (8.0, ['alice-mbp', 'bob-mini'], 'ok'),
                         'App 看到的：两台设备加起来')
        # carol-air 在名单里，在自己的那一份里放伪造的、重放 alice-mbp 的：认证里带着设备指纹，都不认
        fake = sw.cost_seal(sw.derive(sw.COST, self.salt, 'claude', 'alice@example.com', 'u-wrong'), aid, c.fp,
                            {'v': 1, 'at': time.time(), 'h': {str(hour): [10 ** 9, 1, 0, 0, 0, 0, 0]}, 'um': []})
        for bad in (fake, self.slots(aid)[fa]['blob']):
            self.assertEqual(c.post(self.srv.port, 'ledger', {'put': {aid: bad}})[0], 200)
            self.assertEqual(a.cost_sync()['state'], 'ok')
            ra = a.cost(lane + 'remote.json')['acc'][slot]
            self.assertEqual([e['name'] for e in ra.values()], ['bob-mini'], '伪造的、重放的都不认')
        # 移除 bob-mini：它传的留着、标上；dan（用过 alice、没传过）
        d = self.device('dan-air', 'u-alice-0005')
        self.machines(a, c, d)
        self.assertEqual(d.post(self.srv.port, 'report', {'units': {aid: {'cli': [[60, 0]], 'app': None}}, 'end': None})[0], 200)
        self.assertEqual(a.cost_sync()['state'], 'ok')
        ra = a.cost(lane + 'remote.json')['acc'][slot]
        self.assertEqual([(e['name'], e['revoked']) for e in ra.values()], [('bob-mini', True)])
        sync = a.cost(lane + 'sync.json')
        self.assertEqual([m['name'] for m in sync['acc'][slot]['missing']], ['dan-air'])
        [v] = a.cost_view(self.tid, [('claude', self.key, None, self.tid, True)])['accounts']
        self.assertEqual(([(x['name'], x['revoked']) for x in v['devices']], [m['name'] for m in v['sync']['missing']]),
                         ([('alice-mbp', False), ('bob-mini', True)], ['dan-air']))
        for f in ('remote.json', 'sync.json'):
            self.assertEqual(oct(os.stat(a.cca / 'cost' / (lane + f)).st_mode & 0o777), '0o600', f)
        for f in ('remote.json', 'sync.json'):
            self.assertFalse((a.cca / 'cost' / f).exists(), '没有别的服务器的那一份（不带前缀的）')


class VerifyTest(unittest.TestCase):
    """服务器自己验 SSHSIG（RFC 8032 的 Ed25519）：和 ssh-keygen 的结果一致，改过的、换了钥匙或命名空间的一律不收"""
    def test_against_ssh_keygen(self):
        with tempfile.TemporaryDirectory(prefix='cca-presence-verify-') as t:
            d = Path(t)
            pub, other = keygen(d / 'k'), keygen(d / 'o')
            self.assertTrue(srvmod.self_test(), 'RFC 8032 的测试向量')
            for msg in (b'', b'x', secrets.token_bytes(1000), json.dumps({'v': 1, 'op': 'report'}).encode()):
                sig = sign(d / 'k', 'cca-presence', msg)
                self.assertTrue(srvmod.ssh_verify(pub, 'cca-presence', msg, sig))
                self.assertFalse(srvmod.ssh_verify(pub, 'cca-presence', msg + b'!', sig), '消息改过')
                self.assertFalse(srvmod.ssh_verify(pub, 'cca-devices', msg, sig), '命名空间不对')
                self.assertFalse(srvmod.ssh_verify(other, 'cca-presence', msg, sig), '不是这把公钥签的')
                text = sig.decode().strip().splitlines()
                blob = bytearray(base64.b64decode(''.join(text[1:-1])))
                for i in (len(blob) - 1, len(blob) - 40, 30):   # 签名、R、公钥里各改一个字节
                    bad = bytearray(blob)
                    bad[i] ^= 1
                    armored = '\n'.join([text[0], base64.b64encode(bytes(bad)).decode(), text[-1]]).encode()
                    self.assertFalse(srvmod.ssh_verify(pub, 'cca-presence', msg, armored), i)
            sig = sign(d / 'k', 'cca-presence', b'm')
            for junk in (b'', b'garbage', sig.replace(b'BEGIN', b'BEGIM'), sig[:-40] + b'\n-----END SSH SIGNATURE-----\n'):
                self.assertFalse(srvmod.ssh_verify(pub, 'cca-presence', b'm', junk))
            raw = bytes.fromhex('e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b')
            pk = bytes.fromhex('d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a')
            s = int.from_bytes(raw[32:], 'little') + srvmod.EL   # 同一个签名加上阶：数学上也成立，但 s 必须小于阶
            self.assertFalse(srvmod.ed25519_verify(pk, b'', raw[:32] + s.to_bytes(32, 'little')))


if __name__ == '__main__':
    unittest.main(verbosity=1)
