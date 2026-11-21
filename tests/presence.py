#!/usr/bin/env python3
"""连接设备（presence）：服务端的验签、设备清单、状态；客户端的上报、cca devices、共享记下的 API key、折合金额的同步。
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
import shlex
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

# cli/src/cmd/cost.rs 的同名常量
COST_OLD = '维护者的设备统计服务器还不支持同步折合金额（维护者要更新服务器：./policy.sh presence deploy）'


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
    """证书机构签的证书（SAN 是 127.0.0.1）：照证书机构验的加入链接（托管平台的那种）用；设备的 curl 经 CURL_CA_BUNDLE=ca.pem 认这个机构"""
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
    """维护者那边（部署脚本、文档）算公钥指纹的办法：和服务器的 --print-pin 必须一样"""
    pub = subprocess.run(['openssl', 'x509', '-in', str(cert), '-pubkey', '-noout'], stdout=subprocess.PIPE, check=True).stdout
    der = subprocess.run(['openssl', 'pkey', '-pubin', '-outform', 'der'], input=pub, stdout=subprocess.PIPE, check=True).stdout
    import hashlib
    return 'sha256//' + base64.b64encode(hashlib.sha256(der).digest()).decode()


class Server:
    """跑一个真的服务器进程（ttl 2 秒、闲置 3 秒，测试不用干等）。admit_hash：自己的同步服务器（--mode own）；
    proxied：--behind-proxy（只说 HTTP，不用证书）；port：监听这个端口（重启后设备照旧连得上），0 是随便挑一个"""
    def __init__(self, d, ttl='2', idle='3', admit_hash=None, proxied=False, port=0):
        self.d, self.proxied = d, proxied
        who = ['--mode', 'own', '--admit-hash', admit_hash] if admit_hash else ['--release-key', str(d / 'release.pub')]
        tls = ['--behind-proxy'] if proxied else ['--cert', str(d / 'cert.pem'), '--key', str(d / 'key.pem')]
        args = [sys.executable, '-I', str(SERVER), '--listen', '127.0.0.1:%d' % port, '--port-file', str(d / 'port')] + tls + who + [
                '--state', str(d / 'state.json'), '--ttl', ttl, '--idle', idle]
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
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='cca-presence-test-')
        d = cls.d = Path(cls.tmp.name)
        make_cert(d)
        cls.relpub = keygen(d / 'rel', 'test-release')
        (d / 'release.pub').write_text(cls.relpub + ' test-release\n')
        cls.devpub, cls.dev2pub, cls.evilpub = keygen(d / 'dev'), keygen(d / 'dev2'), keygen(d / 'evil')
        cls.srv = Server(d)
        cls.seq = 0

    @classmethod
    def tearDownClass(cls):
        cls.srv.stop()
        cls.tmp.cleanup()

    @staticmethod
    def fp(pub):
        return srvmod.key_fp(pub.split()[1])

    def machines(self, ms, key='rel', seq=None):
        if seq is None:
            ServerTest.seq += 1
            seq = ServerTest.seq
        doc = json.dumps({'version': 1, 'seq': seq, 'machines': ms}).encode()
        return self.srv.req('POST', '/v1/machines', doc, sign(self.d / key, 'cca-devices', doc))

    def both(self):
        return self.machines([{'key': self.devpub, 'name': 'alice-mbp'}, {'key': self.dev2pub, 'name': 'bob-mini'}])

    def call(self, op, key='dev', ns='cca-presence', pub=None, tamper=False, **extra):
        body = dict({'v': 1, 'op': op, 'key': self.fp(pub or getattr(self, key + 'pub')), 'ts': time.time(),
                     'nonce': secrets.token_hex(16)}, **extra)
        raw = json.dumps(body, separators=(',', ':')).encode()
        sig = sign(self.d / key, ns, raw)
        if tamper:
            raw = raw.replace(b'"v":1', b'"v":1 ')
        return self.srv.req('POST', '/v1/' + op, raw, sig)

    def view(self, aid, key='dev2'):
        st, j = self.call('view', key=key, accounts=[aid])
        self.assertEqual(st, 200, j)
        return {r['name']: r for r in j['accounts'][aid]}, j

    def test_health_and_self_test(self):
        st, j = self.srv.req('GET', '/v1/health')
        self.assertEqual(st, 200)
        self.assertTrue(j['ok'] and j['sig_ok'])
        self.assertRegex(openssl_pin(self.d / 'cert.pem'), r'^sha256//[A-Za-z0-9+/]{43}=$', 'P-256 证书的公钥指纹（写进映射的 pins）')

    def test_machines_only_release_key_and_monotonic(self):
        st, j = self.both()
        self.assertEqual((st, j['machines']), (200, 2))
        st, _ = self.machines([{'key': self.evilpub, 'name': 'evil'}], key='evil')
        self.assertEqual(st, 403, '只认发布密钥签的清单')
        doc = json.dumps({'version': 1, 'seq': ServerTest.seq + 1, 'machines': []}).encode()
        st, _ = self.srv.req('POST', '/v1/machines', doc, sign(self.d / 'rel', 'cca-presence', doc))
        self.assertEqual(st, 403, '命名空间不对的签名不算')
        st, j = self.machines([{'key': self.devpub, 'name': 'x'}], seq=ServerTest.seq - 1)
        self.assertEqual((st, j['error']), (409, 'seq'), '序号不能往回退')
        st, _ = self.machines([{'key': self.devpub, 'name': 'alice-mbp'}, {'key': self.dev2pub, 'name': 'bob-mini'}], seq=ServerTest.seq)
        self.assertEqual(st, 200, '同一个序号、同样的内容：重发也成功（推送的回应丢了时重试）')
        st, _ = self.machines([{'key': self.devpub, 'name': 'renamed'}], seq=ServerTest.seq)
        self.assertEqual(st, 409, '同一个序号、内容不同：不收')
        for bad in ([{'key': 'ssh-rsa AAAAB3NzaC1yc2E', 'name': 'x'}], [{'key': self.devpub, 'name': 'a\x1b[2J'}],
                    [{'key': self.devpub, 'name': 'a'}, {'key': self.dev2pub, 'name': 'a'}],
                    [{'key': self.devpub, 'name': 'a'}, {'key': self.devpub, 'name': 'b'}]):
            st, _ = self.machines(bad)
            self.assertEqual(st, 400, bad)

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
        self.assertEqual(self.call('report', key='evil', **u)[0], 403, '不在清单里的设备')
        self.assertEqual(self.call('report', tamper=True, **u)[0], 403, '签过之后又改过的 body')
        self.assertEqual(self.call('report', ns='cca-devices', **u)[0], 403, '命名空间不对')
        self.assertEqual(self.call('report', key='dev', pub=self.dev2pub, **u)[0], 403, '用自己的 key 冒充别的设备')
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
        st, _ = self.machines([{'key': self.dev2pub, 'name': 'bob-mini'}])
        self.assertEqual(st, 200)
        self.assertNotIn('alice-mbp', self.view(aid)[0], '撤销的设备：记录一起删掉')
        self.assertEqual(self.call('report', units={})[0], 403)

    def test_persist(self):
        self.both()
        aid = secrets.token_hex(16)
        self.call('report', units={aid: {'cli': [[60, 0]], 'app': None}})
        self.srv.stop()
        ServerTest.srv = Server(self.d)
        self.assertEqual(self.view(aid)[0]['alice-mbp']['state'], 'in_use', '重启后照样认得设备清单和会话')
        self.assertEqual(oct(os.stat(self.d / 'state.json').st_mode & 0o777), '0o600')

    def test_keys(self):
        """API key 的密文：设备传上来的整份换掉旧的，问的时候回现在的（谁传的、多久以前）；服务器不懂里面是什么，只查形状和长度"""
        self.srv.stop()
        ServerTest.srv = Server(self.d)   # 限流的配额从头算（前面的用例用掉了不少）
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
        self.srv.stop()
        ServerTest.srv = Server(self.d)
        st, j = self.call('keys', get=[a1])
        self.assertEqual(j['keys'][a1]['blob'], b2, '重启后还在')
        st, _ = self.machines([{'key': self.devpub, 'name': 'alice-mbp'}])
        st, j = self.call('keys', get=[a1])
        self.assertEqual((st, j['keys'][a1]['blob'], j['keys'][a1]['name']), (200, b2, 'bob-mini'),
                         '撤销了传它的设备：密文留着（是账号的，登录了这个账号的设备还要用）')
        self.assertEqual(self.call('keys', key='dev2', get=[a1])[0], 403, '撤销的设备不能再问')
        # 存满了（MAX_KEYS 个账号）：新的账号不收，已有的照常换
        with tempfile.TemporaryDirectory() as t:
            store = srvmod.Store(os.path.join(t, 'state.json'), 150, 600, 86400)
            fp = self.fp(self.devpub)
            store.data['machines'][fp] = {'name': 'alice-mbp', 'key': self.devpub}
            with mock.patch.object(srvmod, 'MAX_KEYS', 2):
                x, y, z = (secrets.token_hex(16) for _ in range(3))
                self.assertEqual(store.key_sync(fp, {x: ok, y: ok}, [], time.time())[0], 200)
                self.assertEqual(store.key_sync(fp, {z: ok}, [], time.time()), (507, {'error': 'full'}))
                self.assertEqual(store.key_sync(fp, {x: b1}, [], time.time())[0], 200)
                self.assertEqual(store.keys['acc'][x]['blob'], b1)


    def test_cost(self):
        """折合金额的密文：每个账号、每台设备一份，设备只写自己的那一份；问的时候回各台设备的（认证码没变的、自己的不带密文），
        撤销的设备照样回（标上 revoked），顺带说哪些设备用过这个账号却没传过；服务器只查形状和长度"""
        self.srv.stop()
        ServerTest.srv = Server(self.d)   # 限流的配额从头算
        self.both()
        a1, a2 = secrets.token_hex(16), secrets.token_hex(16)
        fa, fb = self.fp(self.devpub), self.fp(self.dev2pub)
        blob = lambda n=2048: {'n': base64.b64encode(secrets.token_bytes(16)).decode(), 'c': base64.b64encode(secrets.token_bytes(n)).decode(),
                               't': base64.b64encode(secrets.token_bytes(32)).decode()}
        st, j = self.srv.req('GET', '/v1/health')
        self.assertIn('ledger', j['ops'], 'health 说支持哪些 op（维护者看服务器是不是新的）')
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
        self.srv.stop()
        ServerTest.srv = Server(self.d)
        st, j = self.call('ledger', key='dev2', get=[a1])
        self.assertEqual({x['name']: x.get('blob') for x in j['cost'][a1]}, {'alice-mbp': b3, 'bob-mini': None}, '重启后还在')
        self.machines([{'key': self.devpub, 'name': 'alice-mbp'}])
        st, j = self.call('ledger', get=[a1])
        rows = {x['name']: x for x in j['cost'][a1]}
        self.assertEqual((rows['bob-mini']['revoked'], rows['bob-mini']['blob'], rows['alice-mbp']['revoked']), (True, b2, False),
                         '撤销了的设备：它传的留着（是账号的用量），标上 revoked')
        self.assertEqual(self.call('ledger', key='dev2', get=[a1])[0], 403, '撤销的设备不能再问')
        with tempfile.TemporaryDirectory() as t:
            store = srvmod.Store(os.path.join(t, 'state.json'), 150, 600, 86400, cost_keep=1000)
            store.data['machines'] = {fa: {'name': 'alice-mbp', 'key': self.devpub}, fb: {'name': 'bob-mini', 'key': self.dev2pub}}
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
            store.data['machines'][fc] = {'name': 'carol-air', 'key': self.evilpub}
            with mock.patch.object(srvmod, 'COST_RESP', 4500):
                st, j = store.cost_sync(fc, {}, [x], {}, now + 6)
                self.assertEqual([(r['name'], 'blob' in r) for r in j['cost'][x]], [('bob-mini', True), ('alice-mbp', False)],
                                 '回的密文有上限：最近传的先给，别的下次再取')
            self.assertEqual(store.cost_sync('SHA256:gone', {}, [x], {}, now)[0], 403)
            with mock.patch.object(srvmod, 'COST_TOTAL_BYTES', len(ok['c']) * 2):
                self.assertEqual(store.cost_sync(fb, {y: ok}, [], {}, now + 2000)[0], 200,
                                 '存满了：先清掉 cost_keep 秒没再传的，再看放不放得下（不然一直清不掉）')
            self.assertEqual(sorted(store.cost['acc']), [y], 'cost_keep 秒没再传的删掉')


class OwnServerTest(unittest.TestCase):
    """自己的同步服务器（--mode own）：拿加入链接里的入伙口令加入、列出设备、移除；不在名单里的设备、口令不对、盐对不上；
    别的 op 照旧。格式见 server/README.md「自己的同步服务器」"""
    ADMIT, SALT = 'ab' * 16, 'cd' * 16

    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='cca-hub-test-')
        d = cls.d = Path(cls.tmp.name)
        make_cert(d)
        cls.pubs = {n: keygen(d / n) for n in ('a', 'b', 'c', 'evil')}
        cls.srv = Server(d, admit_hash=sw.admit_hash(cls.ADMIT))

    @classmethod
    def tearDownClass(cls):
        cls.srv.stop()
        cls.tmp.cleanup()

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

    def join(self, who='a', name=None, admit=None, check=None, pub=None, **kw):
        return self.call('join', who, pub=pub or self.pubs[who], name=name or who + '-mac', admit=admit or self.ADMIT,
                         check=check or sw.hub_check(self.SALT), **kw)

    def test_flow(self):
        fa, fb = self.fp('a'), self.fp('b')
        st, j = self.srv.req('GET', '/v1/health')
        self.assertEqual((st, j['mode']), (200, 'own'))
        self.assertTrue({'join', 'members', 'remove', 'report', 'view', 'keys', 'ledger'} <= set(j['ops']), j['ops'])
        doc = json.dumps({'version': 1, 'seq': 1, 'machines': [{'key': self.pubs['evil'], 'name': 'evil'}]}).encode()
        self.assertEqual(self.srv.req('POST', '/v1/machines', doc, sign(self.d / 'evil', 'cca-devices', doc))[0], 404,
                         '没有维护者的设备清单')
        gen = self.unknown(self.call('members'))
        self.assertTrue(gen, '不在名单里：回这一份记录的编号（设备据此分清被移除了还是服务器的记录没了）')
        # 口令不对、签名不对、pub 和 key 对不上、拿别的密钥签：一样的 403（不留判断口）
        for bad in (dict(admit='00' * 16), dict(tamper=True), dict(pub=self.pubs['b']), dict(signer='evil'), dict(admit='AB' * 16)):
            self.assertEqual(self.join('a', **bad), (403, {'error': 'denied'}), bad)
        self.assertEqual(self.join('a', name=' studio')[0], 400, '名字前后有空白')
        self.assertEqual(self.join('a', name='x' * 65)[0], 400)
        self.assertEqual(self.join('a', check='zz')[0], 400)
        st, j = self.join('a', name='studio-mac')
        self.assertEqual((st, j['name'], j['members'], j['gen'], j['seed']), (200, 'studio-mac', 1, gen, True), j)
        self.assertEqual(self.join('b', check=sw.hub_check('ef' * 16)), (409, {'error': 'salt_mismatch'}), '拿着另一份盐的加入不了')
        st, j = self.join('b', name='studio-mac')
        self.assertEqual((st, j['name'], j['members']), (200, 'studio-mac-2', 2), '名字被别的设备用了：后面加 -2')
        st, j = self.join('b', name='bob-mini')
        self.assertEqual((st, j['name'], j['members']), (200, 'bob-mini', 2), '同一把密钥再加入：改名字')
        st, j = self.call('members')
        self.assertEqual((st, j['me_name'], j['gen'], j['link']), (200, 'studio-mac', gen, None), '还没换过入伙口令')
        rows = {r['name']: r for r in j['members']}
        self.assertEqual(sorted(rows), ['bob-mini', 'studio-mac'])
        self.assertEqual((rows['studio-mac']['fp'], rows['studio-mac']['me'], rows['bob-mini']['me']), (fa, True, False))
        self.assertTrue(all(0 <= r['joined_age'] <= 5 and 0 <= r['seen_age'] <= 5 for r in rows.values()), rows)
        self.assertEqual(self.call('members', tamper=True), (403, {'error': 'denied'}), '名单里的设备签名不对')
        # 别的 op 照旧
        aid = sw.aid_of(self.SALT, 'claude', 'alice@example.com')
        self.assertEqual(self.call('report', units={aid: {'cli': [[10, 1]], 'app': None}}, end=None)[0], 200)
        st, j = self.call('view', 'b', accounts=[aid])
        self.assertEqual([(r['name'], r['state']) for r in j['accounts'][aid]], [('studio-mac', 'in_use')])
        blob = {'n': base64.b64encode(secrets.token_bytes(16)).decode(), 'c': base64.b64encode(secrets.token_bytes(1024)).decode(),
                't': base64.b64encode(secrets.token_bytes(32)).decode()}
        self.assertEqual(self.call('ledger', 'b', put={aid: blob})[0], 200)
        # 移除
        self.assertEqual(self.call('remove', target='nope')[0], 400)
        self.assertEqual(self.call('remove', target=self.fp('c')), (404, {'error': 'no_device'}))
        st, j = self.call('remove', target=fb)
        self.assertEqual((st, j['members']), (200, 1))
        self.assertEqual(self.unknown(self.call('members', 'b')), gen, '被移除的设备：密钥就此不认（编号照旧：是被移除了）')
        st, j = self.call('ledger', get=[aid])
        self.assertEqual([(r['name'], r['revoked']) for r in j['cost'][aid]], [('bob-mini', True)], '它传过的折合金额是账号的：留着，标成撤销了')
        st, j = self.call('view', accounts=[aid])
        self.assertEqual([r['name'] for r in j['accounts'][aid]], ['studio-mac'])
        # 退出：移除自己
        st, j = self.call('remove', target=fa)
        self.assertEqual((st, j['members']), (200, 0))
        self.assertEqual(self.unknown(self.call('members')), gen)
        # 重启：同一个入伙口令接着用（盐的校验值还在：另一份盐照样加入不了）；换一个就从头开始
        self.assertEqual(self.join('a')[0], 200)
        self.srv.stop()
        OwnServerTest.srv = Server(self.d, admit_hash=sw.admit_hash(self.ADMIT))
        st, j = self.call('members')
        self.assertEqual((st, j['gen']), (200, gen), '重启后照样认得加入的设备，编号不变')
        self.assertEqual(self.join('b', check=sw.hub_check('ef' * 16)), (409, {'error': 'salt_mismatch'}))
        self.srv.stop()
        new = 'ef' * 16
        OwnServerTest.srv = Server(self.d, admit_hash=sw.admit_hash(new))
        gen2 = self.unknown(self.call('members'))
        self.assertTrue(gen2 and gen2 != gen, '换了加入链接：以前的设备不认了，记录换了编号')
        self.assertEqual(self.join('a', admit=self.ADMIT), (403, {'error': 'denied'}), '旧的口令不认')
        st, j = self.join('b', admit=new, check=sw.hub_check('ef' * 16))
        self.assertEqual((st, j['members']), (200, 1), '盐的校验值也从头登记')
        self.assertEqual(json.loads((self.d / 'cost-slots.json').read_text())['acc'], {}, '以前存的密文都删掉')
        self.assertEqual(oct(os.stat(self.d / 'state.json').st_mode & 0o777), '0o600')

    def fresh(self, name, **kw):
        """另起一台自己的服务器（自己的目录，用完停掉）：默认 --behind-proxy（不用证书）"""
        d = self.d / name
        d.mkdir()
        srv = Server(d, admit_hash=sw.admit_hash(kw.pop('admit', self.ADMIT)), proxied=kw.pop('proxied', True))
        self.addCleanup(srv.stop)
        return srv

    def test_relink(self):
        """换入伙口令（relink）：旧链接不能再用来加入，已经加入的照旧；成员问设备时拿到换的那台存的密文；重启后照旧，
        配置的口令换了才从头开始"""
        srv = self.fresh('relink')
        self.assertEqual(self.join('a', srv=srv)[0], 200)
        self.assertEqual(self.join('b', srv=srv)[0], 200)
        blob = {'n': base64.b64encode(secrets.token_bytes(16)).decode(), 'c': base64.b64encode(secrets.token_bytes(64)).decode(),
                't': base64.b64encode(secrets.token_bytes(32)).decode()}
        new = 'ef' * 16
        for bad in (dict(admit=new, blob=blob), dict(admit=sw.admit_hash(new)), dict(admit=sw.admit_hash(new), blob={'n': 'x'}),
                    dict(admit=sw.admit_hash(new), blob=dict(blob, c='A' * (srvmod.MAX_LINK_BLOB + 4)))):
            self.assertEqual(self.call('relink', srv=srv, **bad)[0], 400, '口令要给哈希、密文的形状和长度要对')
        self.assertTrue(self.unknown(self.call('relink', 'c', srv=srv, admit=sw.admit_hash(new), blob=blob)), '不是成员不能换')
        st, j = self.call('relink', srv=srv, admit=sw.admit_hash(new), blob=blob)
        self.assertEqual((st, j['n']), (200, 1), j)
        self.assertEqual(self.call('members', 'b', srv=srv)[0], 200, '已经加入的设备照旧')
        self.assertEqual(self.join('c', srv=srv), (403, {'error': 'denied'}), '旧的加入链接不能再用来加入')
        st, j = self.join('c', srv=srv, admit=new)
        self.assertEqual((st, j['seed']), (200, False), '新的能用：不是部署时配置的那个口令')
        st, j = self.call('members', 'b', srv=srv)
        self.assertEqual((j['link']['n'], j['link']['blob']), (1, blob), '成员拿到换的那台存的密文')
        st, j = self.call('relink', 'b', srv=srv, admit=sw.admit_hash('12' * 16), blob=blob)
        self.assertEqual(j['n'], 2, '每换一次加一')
        gen = self.call('members', srv=srv)[1]['gen']
        srv.stop()
        again = Server(srv.d, admit_hash=sw.admit_hash(self.ADMIT), proxied=True)
        self.addCleanup(again.stop)
        st, j = self.call('members', srv=again)
        self.assertEqual((st, j['gen'], j['link']['n']), (200, gen, 2), '重启后照旧：配置的还是部署时的口令')
        self.assertEqual(self.join('c', srv=again, admit=new), (403, {'error': 'denied'}), '认的是最后换的那个')
        self.assertEqual(self.join('c', srv=again, admit='12' * 16)[0], 200)

    def test_old_state(self):
        """以前的版本写的状态文件：没有 gen 的补上一个、设备照旧；坏了的 link 不要"""
        d = self.d / 'oldstate'
        d.mkdir()
        h = sw.admit_hash(self.ADMIT)
        doc = {'v': 1, 'seq': 0, 'machines': {self.fp('a'): {'name': 'a-mac', 'key': self.pubs['a'], 'joined': time.time()}},
               'beats': {}, 'units': {}, 'own': {'seed': h, 'admit': h, 'check': sw.hub_check(self.SALT), 'link': {'n': 0, 'blob': {}}}}
        (d / 'state.json').write_text(json.dumps(doc))
        srv = Server(d, admit_hash=h, proxied=True)
        self.addCleanup(srv.stop)
        st, j = self.call('members', srv=srv)
        self.assertEqual((st, [r['name'] for r in j['members']], j['link']), (200, ['a-mac'], None), j)
        self.assertRegex(j['gen'], r'^[0-9a-f]{32}$')
        saved = json.loads((d / 'state.json').read_text())['own']
        self.assertEqual((saved['gen'], 'link' in saved), (j['gen'], False), '补上的编号写回去（重启后还是它）')

    def test_behind_proxy(self):
        """--behind-proxy：只说 HTTP；按 X-Forwarded-For 的最后一项限流（前面的是客户端自己写的，不算）；同一个 IP 的连接数不限"""
        srv = self.fresh('proxied')
        st, j = srv.req('GET', '/v1/health')
        self.assertEqual((st, j['mode']), (200, 'own'))
        burst = 120   # Server 的 ip_bucket：Buckets(2, 120)

        def flood(xff):
            got = [srv.req('POST', '/v1/members', b'{}', headers={'X-Forwarded-For': xff(i)})[0] for i in range(burst + 5)]
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
        bad = subprocess.run([sys.executable, '-I', str(SERVER), '--behind-proxy', '--cert', 'x', '--key', 'y', '--mode', 'own',
                              '--admit-hash', sw.admit_hash(self.ADMIT), '--state', str(self.d / 'x.json')],
                             stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=30)
        self.assertEqual(bad.returncode, 2, '--behind-proxy 不用证书')
        self.assertIn('不用 --cert', bad.stderr.decode())

    def test_hosted_preset(self):
        """CCA_HUB_HOSTED=1（托管平台的部署文件只设环境变量、只写 python3 -I /app/cca-presence.py）：等于 --mode own --behind-proxy
        --listen 0.0.0.0:8080 --state /data/state.json；没有 /data（没挂存储）不启动"""
        env = dict(os.environ, CCA_HUB_HOSTED='1', CCA_HUB_ADMIT_HASH=sw.admit_hash(self.ADMIT))
        if not os.path.isdir('/data'):
            r = subprocess.run([sys.executable, '-I', str(SERVER)], env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30)
            self.assertEqual(r.returncode, 2)
            self.assertIn('没有 /data', r.stderr)
        d = self.d / 'hosted'
        d.mkdir()
        proc = subprocess.Popen([sys.executable, '-I', str(SERVER), '--listen', '127.0.0.1:0', '--port-file', str(d / 'port'),
                                 '--state', str(d / 'state.json')], env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                stderr=subprocess.PIPE)
        self.addCleanup(lambda: (proc.poll() is not None or (proc.kill(), proc.wait()), proc.stderr.close()))
        port = int(wait_for('服务器起来', lambda: (d / 'port').exists() and (d / 'port').read_text()))
        c = http.client.HTTPConnection('127.0.0.1', port, timeout=10)
        c.request('GET', '/v1/health')
        j = json.loads(c.getresponse().read())
        c.close()
        self.assertEqual((j['mode'], 'join' in j['ops']), ('own', True), '只说 HTTP、是自己的同步服务器')
        self.assertTrue(json.loads((d / 'state.json').read_text())['own']['seed'] == sw.admit_hash(self.ADMIT))

    def test_store(self):
        """满了、维护者的服务器接着自己的服务器的状态文件（在进程里直接调 Store，不起服务器）"""
        d = self.d / 'store'
        d.mkdir()
        h = sw.admit_hash(self.ADMIT)
        quiet = mock.patch('sys.stderr', new_callable=io.StringIO)   # 服务器的日志：核对，不打出来
        said = quiet.start()
        self.addCleanup(quiet.stop)
        store = srvmod.Store(str(d / 'state.json'), 60, 600, 86400, mode='own', admit_hash=h)
        check, now = sw.hub_check(self.SALT), time.time()
        fps = ['SHA256:' + base64.b64encode(hashlib.sha256(b'%d' % i).digest()).decode()[:43] for i in range(srvmod.MAX_MEMBERS + 1)]
        for i, f in enumerate(fps[:-1]):
            self.assertEqual(store.join(f, self.pubs['a'], 'mac', check, self.ADMIT, now)[0], 200)
        self.assertEqual(len({m['name'] for m in store.data['machines'].values()}), srvmod.MAX_MEMBERS, '名字都不一样（mac、mac-2……）')
        self.assertEqual(store.join(fps[-1], self.pubs['a'], 'x', check, self.ADMIT, now), (507, {'error': 'full'}))
        self.assertEqual(store.join(fps[0], self.pubs['a'], 'renamed', check, self.ADMIT, now)[0], 200, '已经在里面的照样能改名字')
        self.assertEqual(store.remove(fps[1])[0], 200)
        self.assertEqual(store.join(fps[-1], self.pubs['a'], 'x', check, self.ADMIT, now)[0], 200, '移除一台以后又能加入了')
        again = srvmod.Store(str(d / 'state.json'), 60, 600, 86400, mode='own', admit_hash=h)
        self.assertEqual(len(again.data['machines']), srvmod.MAX_MEMBERS, '同一个口令：接着用')
        maint = srvmod.Store(str(d / 'state.json'), 60, 600, 86400)
        self.assertEqual((maint.data['machines'], 'own' in maint.data), ({}, False), '换成维护者的服务器：加入的设备不认（等设备清单）')
        self.assertNotIn('own', json.loads((d / 'state.json').read_text()))
        self.assertIn('64 台设备不要了', said.getvalue())


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


SHARED_REPO = 'example/cca-policy'


class Device:
    """一台设备：自己的 HOME（~/.cca）和 deploy key；cca 的辅助程序在它里面跑。命令都在它自己的 bin/ 里（见上面几段）"""
    KEY = 'alice(alice@example.com)'
    ACCT = ident('alice@example.com')

    def __init__(self, home, name, table):
        self.name, self.home, self.cca = name, home, home / '.cca'
        (self.cca / 'policy').mkdir(parents=True)
        (home / 'bin').mkdir()
        self.keyfile = self.cca / 'policy' / 'id_ed25519'
        self.pub = keygen(self.keyfile, 'cca-policy@' + name)
        self.fp = sw.fp_of(str(self.keyfile) + '.pub')
        for cmd, script in (('curl', CURL), ('ps', PS), ('cca', SELF), ('security', NO_KEYCHAIN)):
            self.fake(cmd, script)
        # 打开着维护者的共享（映射仓库；辅助程序的 policy_on：没有它时 policy.json 不生效）
        (self.cca / 'config.json').write_text(json.dumps({'policy': SHARED_REPO}))
        (self.cca / 'accounts.json').write_text(json.dumps(table))
        self.env = {'HOME': str(home), 'CCA_HOME': str(self.cca), 'CCA_TOOL': 'claude', 'PATH': '%s:/usr/bin:/bin' % (home / 'bin'),
                    'CCA_SELF': str(home / 'bin' / 'cca'), 'CCA_KEYSTORE': 'file', 'CCA_COST': 'off'}
        if 'TZ' in os.environ:
            self.env['TZ'] = os.environ['TZ']

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

    def policy(self, presence, **more):
        j = dict({'version': 1, 'claude': {'alice@example.com': 'US-A'}}, **more)
        if presence is not None:
            j['presence'] = presence
        (self.cca / 'policy' / 'policy.json').write_text(json.dumps(j))

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
        p = self.cca / 'presence' / 'keys.json'
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
        """照文档自己签一个请求发给服务器（不经 cca：扮演这台设备上别的程序、有问题的程序）"""
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
        return json.loads((self.cca / 'cost' / name).read_text())

    def cost_view(self, accounts=(), sync=()):
        """App 看到的（_gui state 的 cost）：经辅助程序只给测试用的 _cost-view 看，设备统计的配置现读"""
        spec = {'config': {}, 'presence': 'load', 'accounts': [list(a) for a in accounts], 'sync': [list(s) for s in sync], 'tools': ['claude']}
        return json.loads(self.ok('_cost-view', repr(time.time()), json.dumps(spec)))


class FreshServer(Server):
    """ClientTest 每个用例重起一次的服务器：5 秒还没起来（机器忙）接着等；起不来、停不下来也不留下进程"""
    def __init__(self, d, admit_hash=None, port=0):
        try:
            try:
                super().__init__(d, ttl='60', idle='600', admit_hash=admit_hash, port=port)
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
    """开着的命令行会话、起上报进程、看它转了几圈（ClientTest、HubClientTest 共用）"""
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

    def status(self, dev, own=False):
        """上报进程最近一次没报到维护者的服务器（own：自己的同步服务器）的原因（cca devices 显示）"""
        p = dev.cca / 'presence' / ('own.status.json' if own else 'status.json')
        return json.loads(p.read_text()).get('error', '') if p.exists() else None


class ClientTest(Sessions, unittest.TestCase):
    """客户端（cca 的辅助程序）：映射里那一节的检查、账号摘要、上报、cca devices、共享记下的 API key、折合金额的同步，
    经 curl + 公钥钉住连真的服务器。只能在进程内测的几处在辅助程序自己的单元测试里（cargo test），下面各留了一行说它在哪"""
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='cca-presence-client-')
        d = cls.d = Path(cls.tmp.name).resolve()
        make_cert(d)
        cls.relpub = keygen(d / 'rel', 'test-release')
        (d / 'release.pub').write_text(cls.relpub + ' test-release\n')
        cls.srv = FreshServer(d)
        cls.pin = openssl_pin(d / 'cert.pem')

    @classmethod
    def tearDownClass(cls):
        if cls.srv is not None:
            cls.srv.stop()
        cls.tmp.cleanup()

    def setUp(self):
        # 每个用例一个新的服务器进程（设备清单、存的密文都在文件里，接着用）：限流的配额从头算
        cls = type(self)
        old, cls.srv = cls.srv, None
        if old is not None:
            old.stop()
        cls.srv = FreshServer(self.d)
        self.key = Device.KEY
        self.salt = '00' * 16
        self.aid = sw.aid_of(self.salt, 'claude', 'alice@example.com')
        self.devices = []
        self.dev = self.device('alice-mbp', table={self.key: 'US-A', 'bob@example.com': 'US-B'})
        self.addCleanup(self.no_keychain)

    def no_keychain(self):
        self.assertEqual([d.name for d in self.devices if (d.home / 'security.log').exists()], [], '调到了 security（钥匙串）')

    def presence(self, **over):
        return dict({'url': 'https://127.0.0.1:%d' % self.srv.port, 'pins': [self.pin], 'salt': self.salt}, **over)

    def device(self, name, uuid=None, table=None):
        """一台设备：自己的 ~/.cca 和 deploy key，同一份映射、同一个账号；uuid 不是 None 时 alice 在它上面登录着"""
        d = Device(self.d / ('%s-%s' % (name, secrets.token_hex(4))), name, table or {self.key: 'US-A'})
        d.policy(self.presence())
        if uuid:
            d.login(uuid)
        self.devices.append(d)
        return d

    def machines(self, *devs):
        """维护者的设备清单：这几台（名字就是设备的名字）"""
        doc = json.dumps({'version': 1, 'seq': int(time.time() * 1000), 'machines': [{'key': d.pub, 'name': d.name} for d in devs]}).encode()
        st, _ = self.srv.req('POST', '/v1/machines', doc, sign(self.d / 'rel', 'cca-devices', doc))
        self.assertEqual(st, 200)

    def test_policy_block(self):
        def check(j, strict=False):
            """映射文件的问题（py policy-check：拉下来以后查；strict 是维护者发布前查的）"""
            p = self.dev.home / 'candidate.json'
            p.write_text(json.dumps(j))
            r = self.dev.run('policy-check', str(p), *(['strict'] if strict else []))
            self.assertEqual((r.returncode, r.stderr), (1 if r.stdout else 0, ''))
            return r.stdout.splitlines()
        pp = lambda p: check({'version': 1, 'claude': {}, 'presence': p}, True)   # presence 那一节的问题
        ok = {'url': 'https://203.0.113.10:18443', 'pins': ['sha256//' + 'A' * 43 + '='], 'salt': 'ab' * 16}
        self.assertEqual(pp(ok), [])
        for bad in ({**ok, 'url': 'http://203.0.113.10:18443'}, {**ok, 'url': 'https://203.0.113.10'},
                    {**ok, 'url': 'https://203.0.113.10:99999'}, {**ok, 'pins': []}, {**ok, 'pins': ['sha1//x']},
                    {**ok, 'salt': 'XYZ'}, 'nonsense'):
            self.assertTrue(pp(bad), bad)
        strict = check
        j = {'version': 1, 'claude': {}, 'presence': ok}
        self.assertEqual(strict(j, True), [], '维护者的严格检查认识 presence')
        self.assertTrue(strict(dict(j, presence={'url': 'x'}), True))
        self.assertEqual(strict(dict(j, presence={'url': 'x'})), [], '客户端不因为它拒绝整份映射')

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
        self.assertEqual((asked['key'], asked['code']), (fp, 200), '和 ssh-keygen -l 的指纹一样（服务器照设备清单里的公钥认它）')
        [a] = asked['accounts']
        self.assertRegex(a, r'^[0-9a-f]{32}$')
        self.assertEqual(a, sw.aid_of(self.salt, 'claude', 'alice@example.com'), '账号摘要和文档写的一样')
        (dev.cca / 'accounts.json').write_text(json.dumps({'alice(Alice@Example.com)': 'US-A'}))
        self.assertEqual(self.view(dev, '--json')[1]['accounts'], [a], '邮箱不分大小写')
        (dev.cca / 'codex.json').write_text(json.dumps({'alice@example.com': 'US-A'}))
        dev.policy(self.presence(), codex={'alice@example.com': 'US-A'})
        [x] = self.view(dev, '--json', CCA_TOOL='codex')[1]['accounts']
        self.assertNotEqual(a, x, 'Claude 账号和 Codex 账号不撞')
        self.assertEqual(x, sw.aid_of(self.salt, 'codex', 'alice@example.com'))
        dev.policy(self.presence(salt='11' * 16))
        [y] = self.view(dev, '--json')[1]['accounts']
        self.assertNotEqual(a, y, '换了盐就认不出')
        self.assertEqual(y, sw.aid_of('11' * 16, 'claude', 'alice@example.com'))
        # 用不了的几种（cca devices 说原因，退出码 2）
        dev.policy(self.presence())
        (dev.cca / 'config.json').write_text(json.dumps({'policy': SHARED_REPO, 'presence': False}))
        self.assertIn('关掉了', self.view(dev, rc=2)[0])
        dev.config(lambda c: c.pop('presence'))
        dev.policy(None)
        self.assertIn('还没有设备统计', self.view(dev, rc=2)[0])
        dev.policy({'url': 'bad'})
        self.assertIn('不对', self.view(dev, rc=2)[0])
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
        self.assertEqual((asked['code'], asked['reply'].get('error')), (403, 'denied'), '设备清单里还没有这台 Mac')
        self.assertIn('presence sync', out)
        self.machines(dev)
        since = self.session(dev)   # 100 秒前开的
        self.session(dev, 'bob@example.com', SESSION_PID - 1)   # 不在映射里的账号也开着会话
        self.agent(dev)
        [report] = wait_for('上报进程报上去', lambda: dev.sent('report'))
        self.assertEqual((report['code'], list(report['units']), report['end']), (200, [self.aid], None), '只报映射里的官方账号')
        [[opened, idle]] = report['units'][self.aid]['cli']
        self.assertEqual((idle, report['units'][self.aid]['app']), (0, None), '没有终端：在跑就算在干活；没开桌面 App')
        self.assertTrue(opened >= 100 and abs(since + opened - report['done']) < 15, '开了多久：到上报的时候为止')
        doc = json.loads(self.view(dev, '--json')[0])
        self.assertEqual((doc['kind'], doc['me'], doc['tool']), ('devices', 'alice-mbp', 'claude'))
        [a] = doc['accounts']
        self.assertEqual(a['key'], self.key)
        [d] = a['devices']
        self.assertEqual((d['name'], d['me'], d['state'], d['cli'], d['app']), ('alice-mbp', True, 'in_use', 1, False))
        self.assertTrue(abs(d['since_epoch'] - since) < 15 and abs(d['last_used_epoch'] - report['done']) < 15,
                        '时间换算成这台 Mac 的时钟（什么时候开的、最近一次用是上报的那一刻）')
        out = self.view(dev)[0]
        self.assertIn('alice-mbp（这台 Mac）  在用 · 命令行里 1 个会话 · ', out)
        self.assertIn(' 起', out)
        self.assertIn('不在维护者的映射里', self.view(dev, 'bob', rc=2)[0])
        n = len(dev.sent())
        dev.policy(self.presence(pins=['sha256//' + 'A' * 43 + '=']))
        out = self.view(dev, rc=1)[0]
        self.assertIn('钉住的指纹对不上', out, '证书不是钉住的那张：不发')
        self.assertEqual([r['code'] for r in dev.sent()[n:]], [0])
        dev.policy(self.presence(url='https://127.0.0.1:9'))
        self.assertIn('连不上设备统计的服务器（https://127.0.0.1:9）', self.view(dev, rc=1)[0])

    # test_past_time（cca devices 里过去的时刻怎么写：今天、昨天、星期几、日期）：cli/src/cmd/presence.rs 的单元测试 past_time_like_python_test

    def test_agent_backoff(self):
        """报不出去（服务器连不上、不认这台 Mac）：按退避再试，心跳到期、会话变了都不会马上重发。
        隔多久再试的那几个数（心跳 60 秒上下，退避 10、20 秒……，不认这台 Mac 时 10 分钟）：cli/src/cmd/presence.rs 的单元测试 retry_backs_off"""
        ok, denied, down = self.dev, self.device('bob-mini', 'u-alice-0007'), self.device('carol-air', 'u-alice-0007')
        recent = self.device('dan-air', 'u-alice-0007')
        self.machines(ok, recent)   # bob-mini 不在设备清单里（403）；carol-air 连不上服务器
        ok.login('u-alice-0007')
        down.policy(self.presence(url='https://127.0.0.1:9'))
        (recent.cca / 'presence').mkdir(mode=0o700)   # dan-air 半分钟前刚同步过记下的 API key
        (recent.cca / 'presence' / 'keys.json').write_text(json.dumps({'v': 1, 'acc': {}, 'synced': time.time() - 30}))
        everyone = (ok, denied, down, recent)
        for d in everyone:
            self.session(d)
            self.agent(d)
        for d, code in ((ok, 200), (denied, 403), (down, 0), (recent, 200)):
            [r] = wait_for('%s 的第一次上报' % d.name, lambda: d.sent('report'))
            self.assertEqual(r['code'], code)
        self.assertIn('presence sync', wait_for('没报成的原因', lambda: self.status(denied)))
        self.assertIn('连不上设备统计的服务器', wait_for('没报成的原因', lambda: self.status(down)))
        self.assertIn(self.status(ok), (None, ''), '报成了：没有没报成的原因')
        # 开着会话：马上同步一次记下的 API key（不 force：最近同步过就不发）
        self.assertEqual([[r['code'] for r in d.sent('keys')] for d in everyone], [[200], [403], [0], []])
        for d in everyone:
            self.session(d, pid=SESSION_PID - 2)   # 会话变了
        self.more_rounds(everyone)
        gaps = lambda reports: [b['done'] - a['done'] for a, b in zip(reports, reports[1:])]   # 两次上报之间隔了多久
        sent = ok.sent('report')
        self.assertEqual([len(r['units'][self.aid]['cli']) for r in sent[:2]], [1, 2], '报成了的：会话变了马上再报')
        self.assertTrue(all(gap > 30 for gap in gaps(sent[1:])), '没到心跳的时候（60 秒上下）转多少圈都不再发')
        self.assertEqual(len(denied.sent('report')), 1, '服务器不认这台 Mac：转了几圈、会话变了都不重发（10 分钟后再试）')
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
        self.assertEqual((a.keyset(K1, 'bob@example.com'), a.ok('key-share', 'bob@example.com')), ('', ''), '不在映射里的账号：只记在这台设备上，不同步')
        self.assertEqual(a.keyset(K1).strip(), 'shared', '映射里的账号、开着设备统计：说一声会同步')
        self.assertEqual(wait_for('后台起同步', a.kicks), ['_key-sync force'], '马上在后台同步（就这一次：bob@example.com 的那一把没起）')
        self.assertEqual(oct(os.stat(a.cca / 'presence' / 'keys.json').st_mode & 0o777), '0o600')
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
        # carol-air（在设备清单里、没登录 alice）把旧的那一份传回去（重放），再传一份伪造的：都不认，传回新的
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
        (a.cca / 'config.json').write_text(json.dumps({'policy': SHARED_REPO, 'presence': False}))
        self.assertEqual(a.keyset(K1), '', '关掉了设备统计：不说会同步')
        self.assertFalse((a.cca / 'presence').exists(), '也不建 presence 目录')
        self.assertIsNone(a.sync())
        a.config(lambda c: c.pop('presence'))
        # bob-mini 的是旧版本记下的：key_hints 里没有时间，按密钥文件的修改时间算（比 alice-mbp 记下的早）
        (b.cca / 'config.json').write_text(json.dumps({'policy': SHARED_REPO, 'presence': False}))   # 记的时候不经设备统计（keys.json 里没有它）
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
        (a.cca / 'presence' / 'keys.json').write_text(json.dumps(dict(j, synced=j['synced'] - 3600)))   # 上次同步是一小时以前的事：这一圈该同步了
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
        held = open(dev.cca / 'presence' / 'keys-sync.lock', 'a')
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
        (dev.cca / 'presence' / 'keys.json').write_text(json.dumps(dict(j, synced=j['synced'] - 3600)))
        held = open(dev.cca / 'presence' / 'keys-sync.lock', 'a')
        fcntl.flock(held, fcntl.LOCK_EX)
        try:
            t0 = time.monotonic()
            self.assertIsNone(dev.sync(''), '不是 force 的：有一轮在跑就走')
            self.assertLess(time.monotonic() - t0, 15)   # 等的话要等到放锁（这里一直不放：force 的那种等满 35 秒）
            self.assertEqual(len(dev.sent()), 2)
        finally:
            held.close()
        self.assertEqual(dev.sync(''), '', '没有别的在跑：到时候了就同步')

    def test_keyshare_old_server(self):
        """旧的服务器（还不收 key：404）：记下原因（维护者要更新服务器），6 小时后再试；cca devices、App 说一句"""
        dev = self.dev
        self.machines(dev)
        dev.login('u-alice-0003')
        (dev.home / 'reply').mkdir()
        (dev.home / 'reply' / 'keys').write_text('404\n{"error": "not_found"}\n')
        dev.keyset(self.K1)
        err = dev.sync()
        self.assertIn('./policy.sh presence deploy', err)
        j = dev.keys_json()
        self.assertTrue(j['retry'] - time.time() > 5 * 3600 and j['error'] == err, j)
        (dev.cca / 'presence' / 'keys.json').write_text(json.dumps(dict(j, synced=j['synced'] - 3600)))   # 上次同步是一小时以前的事（平时该再同步了）
        self.assertIsNone(dev.sync(''), '不 force：6 小时内不再试')
        self.assertEqual([r['op'] for r in dev.sent()], ['keys'])
        self.assertIn('  记下的 API key 最近一次没同步成：' + err, self.view(dev)[0].splitlines(), 'cca devices 说一句')
        # App：会同步的账号的 api_key.sync_error 是它，不同步的账号不说（cli/src/cmd/gui.rs 的单元测试 api_key_like_python_test）
        self.assertEqual((dev.ok('key-share', self.key).strip(), dev.ok('key-share', 'bob@example.com')), ('1', ''))

    # 折合金额：各台设备算自己的，经服务器交换密文
    def slots(self, aid):
        return json.loads((self.d / 'cost-slots.json').read_text())['acc'].get(aid, {})

    def test_cost_sync(self):
        """alice-mbp、bob-mini 登录着 alice，carol-air 没有：两台互相取到对方的，服务器上只有密文；伪造的、重放的不认；
        没变的不重传、不重取；撤销的设备留着、标上；用过却没传过的设备说一声"""
        a = self.dev
        b = self.device('bob-mini', 'u-alice-0005')
        c = self.device('carol-air', None)
        self.machines(a, b, c)
        a.login('u-alice-0005')
        aid = self.aid
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
        rb = b.cost('remote.json')['acc']['claude:alice@example.com']
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
        ra = a.cost('remote.json')['acc']['claude:alice@example.com']
        self.assertEqual([e['name'] for e in ra.values()], ['bob-mini'])
        n = len(a.sent())
        self.assertEqual(a.cost_sync()['state'], 'ok')
        posts = a.sent()[n:]
        self.assertEqual(list(posts[0]['have'][aid].values()).count(None), 0)
        self.assertEqual(len(posts[0]['have'][aid]), 2, '带着已经有的认证码（自己的、bob-mini 的）：没变的服务器不回密文')
        [v] = a.cost_view([('claude', self.key, None, True, True)])['accounts']
        self.assertEqual((v['summary']['days_usd'], [d['name'] for d in v['devices']], v['sync']['state']), (8.0, ['alice-mbp', 'bob-mini'], 'ok'),
                         'App 看到的：两台设备加起来')
        # carol-air 在设备清单里，在自己的那一份里放伪造的、重放 alice-mbp 的：认证里带着设备指纹，都不认
        fake = sw.cost_seal(sw.derive(sw.COST, self.salt, 'claude', 'alice@example.com', 'u-wrong'), aid, c.fp,
                            {'v': 1, 'at': time.time(), 'h': {str(hour): [10 ** 9, 1, 0, 0, 0, 0, 0]}, 'um': []})
        for bad in (fake, self.slots(aid)[fa]['blob']):
            self.assertEqual(c.post(self.srv.port, 'ledger', {'put': {aid: bad}})[0], 200)
            self.assertEqual(a.cost_sync()['state'], 'ok')
            ra = a.cost('remote.json')['acc']['claude:alice@example.com']
            self.assertEqual([e['name'] for e in ra.values()], ['bob-mini'], '伪造的、重放的都不认')
        # 撤销 bob-mini：它传的留着、标上；dan（用过 alice、没传过）
        d = self.device('dan-air', 'u-alice-0005')
        self.machines(a, c, d)
        self.assertEqual(d.post(self.srv.port, 'report', {'units': {aid: {'cli': [[60, 0]], 'app': None}}, 'end': None})[0], 200)
        self.assertEqual(a.cost_sync()['state'], 'ok')
        ra = a.cost('remote.json')['acc']['claude:alice@example.com']
        self.assertEqual([(e['name'], e['revoked']) for e in ra.values()], [('bob-mini', True)])
        sync = a.cost('sync.json')
        self.assertEqual([m['name'] for m in sync['acc']['claude:alice@example.com']['missing']], ['dan-air'])
        [v] = a.cost_view([('claude', self.key, None, True, True)])['accounts']
        self.assertEqual(([(x['name'], x['revoked']) for x in v['devices']], [m['name'] for m in v['sync']['missing']]),
                         ([('alice-mbp', False), ('bob-mini', True)], ['dan-air']))
        for f in ('remote.json', 'sync.json'):
            self.assertEqual(oct(os.stat(a.cca / 'cost' / f).st_mode & 0o777), '0o600', f)

    def test_cost_old_server(self):
        """旧的服务器（没有 /v1/ledger：404，body 太大时先回 413）：记下原因，6 小时后再试；App 说要维护者更新服务器"""
        dev = self.dev
        dev.login('u-alice-0006')
        (dev.home / 'reply').mkdir()
        for code in (404, 413):
            (dev.home / 'reply' / 'ledger').write_text('%d\n{"error": "not_found"}\n' % code)
            (dev.cca / 'cost' / 'sync.json').unlink(missing_ok=True)
            n = len(dev.sent())
            r = dev.cost_sync()
            self.assertEqual((r['state'], r['message']), ('old_server', COST_OLD), code)
            self.assertIn('./policy.sh presence deploy', r['message'])
            j = dev.cost('sync.json')
            self.assertTrue(j['retry'] - time.time() > 5 * 3600, j)
            self.assertEqual(dev.cost_sync(force=False)['state'], 'waiting', '不 force：6 小时内不再试')
            self.assertEqual([x['op'] for x in dev.sent()[n:]], ['ledger'])
        n = len(dev.sent())
        (dev.cca / 'cost' / 'sync.json').write_text(json.dumps({'v': 1, 'at': time.time() + 3 * 86400, 'retry': time.time() + 30 * 86400,
                                                                'state': 'old_server', 'error': COST_OLD}))
        dev.cost_sync(force=False)
        self.assertEqual([x['op'] for x in dev.sent()[n:]], ['ledger'], '记下的时间在未来（这台设备的时钟快过、又调回来了）：不当真，照常同步')
        [s] = dev.cost_view(sync=[('claude:alice@example.com', True, True)])['sync']
        self.assertEqual(s['state'], 'old_server')


class HubClientTest(Sessions, unittest.TestCase):
    """自己的同步服务器（cca hub）的客户端：经辅助程序加入、看状态、列出设备、移除、退出；维护者的服务器同时在时，映射里的账号
    照旧走维护者的，别的官方账号走自己的（账号摘要、密钥、设备指纹各是各的）；上报进程一台服务器一路"""
    ADMIT, SALT = '12' * 16, '34' * 16
    BOB = 'bob(bob@example.com)'
    K1, K2 = 'sk-ant-api03-' + 'a' * 40, 'sk-ant-api03-' + 'b' * 50

    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='cca-hub-client-')
        d = cls.d = Path(cls.tmp.name).resolve()
        make_cert(d)
        (d / 'release.pub').write_text(keygen(d / 'rel', 'test-release') + ' test-release\n')
        cls.srv, cls.pin = FreshServer(d), openssl_pin(d / 'cert.pem')   # 维护者的（各用例共用）

    @classmethod
    def tearDownClass(cls):
        cls.srv.stop()
        cls.tmp.cleanup()

    def setUp(self):
        # 自己的服务器每个用例一台新的（各自的目录、证书）：加入了几台、谁被移除过都从头来
        o = self.d / ('own-' + secrets.token_hex(4))
        o.mkdir()
        make_cert(o)
        self.own, self.own_pin = FreshServer(o, admit_hash=sw.admit_hash(self.ADMIT)), openssl_pin(o / 'cert.pem')
        self.addCleanup(lambda: self.own.proc.poll() is not None or self.own.stop())   # 用例里停过的不再停

    def link(self, admit=None, salt=None, pin=None, port=None):
        return sw.link_encode('https://127.0.0.1:%d' % (port or self.own.port), [pin or self.own_pin], admit or self.ADMIT, salt or self.SALT)

    def device(self, name, table):
        """一台设备：维护者的映射里只有 alice（bob 是自己的账号）"""
        dev = Device(self.d / ('%s-%s' % (name, secrets.token_hex(4))), name, table)
        dev.policy({'url': 'https://127.0.0.1:%d' % self.srv.port, 'pins': [self.pin], 'salt': '00' * 16})
        return dev

    def machines(self, *devs):
        """维护者的设备清单：这几台"""
        doc = json.dumps({'version': 1, 'seq': time.time_ns() // 1000, 'machines': [{'key': d.pub, 'name': d.name} for d in devs]}).encode()
        self.assertEqual(self.srv.req('POST', '/v1/machines', doc, sign(self.d / 'rel', 'cca-devices', doc))[0], 200)

    def join(self, dev, link=None, *args):
        r = dev.run('hub-join', *args, stdin=link or self.link())
        self.assertEqual(r.stderr, '')
        return r.returncode, r.stdout.strip()

    def joined(self, name, members):
        """加入成了时 hub-join 的 stdout（一行一项：名字、地址、几台设备、关掉了同步的原因；strip 过）"""
        return 0, '%s\n127.0.0.1:%d\n%d' % (name, self.own.port, members)

    def own_fp(self, dev):
        return sw.fp_of(str(dev.cca / 'hub' / 'id_ed25519.pub'))

    def test_lifecycle(self):
        a = self.device('alice-mbp', {Device.KEY: 'US-A', self.BOB: 'US-B'})
        b = self.device('bob-mini', {self.BOB: 'US-B'})
        host = '127.0.0.1:%d' % self.own.port
        why = lambda r: (r[0], r[1].split('\t')[0])
        self.assertEqual(why(self.join(a, 'garbage')), (1, 'link'))
        self.assertEqual(why(self.join(a, self.link()[:-3])), (1, 'link'), '少了一截')
        self.assertEqual(why(self.join(a, self.link(admit='99' * 16))), (1, 'denied'), '入伙口令不对')
        self.assertEqual(why(self.join(a, self.link(pin=self.pin))), (1, 'pin'), '证书和链接里钉住的对不上')
        self.assertEqual(why(self.join(a, self.link(port=self.srv.port, pin=self.pin))), (1, 'old'), '维护者的服务器（不是 --mode own）')
        self.assertFalse((a.cca / 'hub').exists(), '没加入成：什么都不留（刚生成的密钥也删掉）')
        self.assertNotIn('device_id', a.config())
        self.assertEqual(self.join(a), self.joined(os.uname().nodename.split('.')[0] or 'Mac', 1), '没给名字：用主机名')
        self.assertEqual(sorted(wait_for('后台起上报、同步', lambda: len(a.kicks()) >= 2 and a.kicks())), ['_key-sync force', '_presence-agent'])
        self.assertEqual(self.join(a, None, '--name', 'alice-mbp'), self.joined('alice-mbp', 1), '再加入同一台：改名字')
        hub = a.cca / 'hub'
        self.assertEqual([oct(os.stat(p).st_mode & 0o777) for p in (hub, hub / 'hub.json', hub / 'id_ed25519')], ['0o700', '0o600', '0o600'])
        self.assertRegex(a.config()['device_id'], '^[0-9a-f]{16}$', '加入时生成这台机器的标识（折合金额里认同一台机器）')
        argv = (a.home / 'curl.argv').read_text()
        self.assertIn(host, argv, '加入、同步都经 curl')
        for secret in (self.ADMIT, self.SALT, self.link()[-34:-10]):   # 最后一个：链接末尾盐的那一截（base64 的）
            self.assertNotIn(secret, argv + (a.home / 'kicks.log').read_text(), '加入链接、口令、盐不进别的进程的命令行')
        st = json.loads(a.ok('hub-status', '--json'))
        h = st['hub']
        self.assertEqual((st['joined'], st['reason'], h['kind'], h['host'], h['name'], h['members'], h['accounts'], h['state'], h['can_invite']),
                         (True, None, 'own', host, 'alice-mbp', 1, 1, 'pending', True), '经它同步的只有 bob（alice 在维护者的映射里）')
        self.assertIn('自己的同步服务器：' + host, a.ok('hub-status'))
        inv = a.ok('hub-invite').strip()
        self.assertEqual(sw.link_decode(inv), sw.link_decode(self.link()), 'invite 给的就是加入时的那一份')
        self.assertEqual(self.join(b, ' %s\n' % inv, '--name', 'bob-mini'), self.joined('bob-mini', 2))
        c = self.device('carol-air', {self.BOB: 'US-B'})
        self.assertEqual(why(self.join(c, self.link(salt='56' * 16))), (1, 'salt'), '拿着另一份盐的加入不了')
        self.assertFalse((c.cca / 'hub').exists())
        self.assertEqual(why(self.join(a, self.link(port=1))), (1, 'other'), '已经加入了另一台：先退出')
        self.assertEqual(self.join(a, inv), self.joined('alice-mbp', 2), '再加入同一台：照样成（服务器丢了记录时用）；名字照上次的')
        devs = json.loads(a.ok('hub-devices', '--json'))
        self.assertEqual([(x['name'], x['me']) for x in devs['devices']], [('alice-mbp', True), ('bob-mini', False)])
        self.assertTrue(all(abs(x['joined_epoch'] - time.time()) < 60 and abs(x['seen_epoch'] - time.time()) < 60 for x in devs['devices']))
        self.assertEqual(a.run('hub-remove', 'plan', 'nobody').returncode, 3)
        fa, fb = self.own_fp(a), self.own_fp(b)
        self.assertEqual(a.ok('hub-remove', 'plan', 'bob-mini').strip(), '%s\tbob-mini\t0' % fb)
        self.assertEqual(a.ok('hub-remove', 'plan', fb[7:15]).strip(), '%s\tbob-mini\t0' % fb, '指纹的开头也认（不带 SHA256: 也行）')
        self.assertEqual(a.ok('hub-remove', 'plan', 'alice-mbp').strip().split('\t')[1:], ['alice-mbp', '1'], '是这台设备自己')
        self.assertEqual(a.ok('hub-remove', 'apply', fb), '', '移除以后换了加入链接（stdout 是空的）')
        self.assertEqual(json.loads(a.ok('hub-status', '--json'))['hub']['members'], 1)
        inv2 = a.ok('hub-invite').strip()
        new = sw.link_decode(inv2)
        self.assertEqual((new['u'], new['p'], new['s']), (sw.link_decode(inv)['u'], [self.own_pin], self.SALT), '地址、指纹、盐不变')
        self.assertNotEqual(new['a'], self.ADMIT, '移除以后换了入伙口令')
        blob = json.loads((self.own.d / 'state.json').read_text())['own']['link']['blob']
        self.assertEqual(sw.link_open(self.SALT, blob), {'a': new['a'], 's': self.ADMIT}, '服务器上存的照 README 解得开：新的口令、部署时的口令')
        self.assertNotIn(new['a'], (self.own.d / 'state.json').read_text(), '服务器上只有新口令的哈希')
        r = b.run('hub-devices')
        self.assertEqual(r.returncode, 1)
        self.assertIn('被别的设备从 %s 上的自己的同步服务器移除了' % host, r.stdout, '记录的编号没变：是被移除了')
        self.assertFalse((b.cca / 'hub').exists(), '本机记着的删掉（和退出一样）')
        self.assertEqual(json.loads((b.cca / 'hub-removed.json').read_text())['name'], 'bob-mini', '留下说明（在 hub/ 外面：再加入没成时也在）')
        self.assertEqual(oct((b.cca / 'hub-removed.json').stat().st_mode & 0o777), '0o600')
        st = json.loads(b.ok('hub-status', '--json'))
        self.assertEqual((st['joined'], st['hub']), (False, None))
        self.assertIn('要再加入', st['reason'])
        self.assertEqual(why(self.join(b, inv)), (1, 'denied'), '被移除以后拿着原来的链接加入不了')
        self.assertTrue((b.cca / 'hub-removed.json').exists(), '加入没成：说明照旧留着')
        self.assertEqual(self.join(b, inv2), self.joined('bob-mini', 2), '拿新的链接能加入：名字照移除前的')
        self.assertFalse((b.cca / 'hub-removed.json').exists(), '加入成了删掉说明')
        a.ok('hub-remove', 'apply', self.own_fp(b))
        inv = a.ok('hub-invite').strip()
        self.assertEqual(b.ok('hub-leave', 'plan').strip(), host + '\tbob-mini')
        self.assertEqual(b.ok('hub-leave', 'apply'), '', '服务器已经不认它了：照样退出，不用说什么')
        self.assertFalse((b.cca / 'hub').exists())
        self.assertEqual((b.run('hub-leave', 'plan').returncode, b.run('hub-invite').returncode), (2, 2))
        self.assertEqual(json.loads(b.ok('hub-status', '--json'))['hub'], None)
        # 移除这台设备自己：等于退出（不换链接）
        self.assertEqual(self.join(c, inv, '--name', 'carol-air'), self.joined('carol-air', 2))
        self.assertEqual(a.ok('hub-remove', 'apply', fa), '')
        self.assertFalse((a.cca / 'hub').exists(), '移除的是自己：本机记着的也删掉')
        self.assertEqual(set(json.loads((self.own.d / 'state.json').read_text())['machines']), {self.own_fp(c)})
        # 退出时服务器连不上：本机照样退出，说怎么在别的设备上移除它
        self.own.stop()
        out = c.ok('hub-leave', 'apply')
        self.assertTrue('服务器上没能移除这台设备' in out and '连不上' in out and 'cca hub remove' in out, out)
        self.assertFalse((c.cca / 'hub').exists())

    def test_unusable(self):
        """加入了却用不了：关掉了设备统计（state 是 off；管设备照样能用）、密钥不见了（error）"""
        a = self.device('alice-mbp', {self.BOB: 'US-B'})
        self.assertEqual(self.join(a, None, '--name', 'alice-mbp'), self.joined('alice-mbp', 1))
        a.config(lambda c: c.update(presence=False))
        st = json.loads(a.ok('hub-status', '--json'))
        self.assertEqual((st['joined'], st['hub']['state'], st['hub']['error']), (True, 'off', st['reason']))
        self.assertIn('关掉了设备统计', st['reason'])
        self.assertEqual([x['name'] for x in json.loads(a.ok('hub-devices', '--json'))['devices']], ['alice-mbp'], '关掉了同步也能看有哪些设备')
        code, out = self.join(a)
        lines = out.split('\n')
        self.assertEqual((code, lines[:3]), (0, self.joined('alice-mbp', 1)[1].split('\n')))
        self.assertIn('关掉了设备统计', lines[3], '重新加入：说现在不经它同步')
        a.config(lambda c: c.pop('presence'))
        key = a.cca / 'hub' / 'id_ed25519'
        saved = key.read_bytes()
        key.unlink()
        st = json.loads(a.ok('hub-status', '--json'))
        self.assertEqual((st['joined'], st['hub']['state']), (True, 'error'))
        self.assertIn('密钥不见了', st['reason'])
        self.assertEqual(a.run('hub-devices').returncode, 2, '密钥不见了：问不了服务器')
        key.write_bytes(saved)
        key.chmod(0o600)
        self.assertIsNone(json.loads(a.ok('hub-status', '--json'))['reason'])

    def test_cost_error_clears(self):
        """折合金额没同步成时记下原因（cca hub status 说没同步成）；要同步的账号都没了（退出了登录、移走了）以后再同步，原因清掉"""
        a = self.device('alice-mbp', {self.BOB: 'US-B'})
        a.login('u-bob-0001', self.BOB)
        self.assertEqual(self.join(a, None, '--name', 'alice-mbp')[0], 0)
        self.own.stop()
        r = a.cost_sync()
        self.assertEqual([(h['kind'], h['state']) for h in r['hubs']], [('maintainer', 'skipped'), ('own', 'error')], r)
        h = json.loads(a.ok('hub-status', '--json'))['hub']
        self.assertEqual(h['state'], 'error')
        self.assertIn('连不上', h['error'])
        a.login(None, self.BOB)
        r = a.cost_sync()
        self.assertEqual([(h['kind'], h['state'], h['message']) for h in r['hubs']], [('maintainer', 'skipped', None), ('own', 'skipped', None)], r)
        h = json.loads(a.ok('hub-status', '--json'))['hub']
        self.assertEqual((h['state'], h['error']), ('pending', None), '不再一直说没同步成')

    def test_agent_lanes(self):
        """上报进程一台服务器一路：映射里的 alice 只报给维护者的，自己的 bob 只报给自己的；自己的那台连不上时它那一路记下原因，
        维护者那一路照报"""
        a = self.device('alice-mbp', {Device.KEY: 'US-A', self.BOB: 'US-B'})
        self.machines(a)
        self.assertEqual(self.join(a, None, '--name', 'alice-mbp')[0], 0)
        fo = self.own_fp(a)
        reports = lambda fp: [r for r in a.sent('report') if r['key'] == fp]
        self.session(a)
        self.session(a, self.BOB, SESSION_PID - 1)
        self.agent(a)
        [m] = wait_for('报给维护者的', lambda: reports(a.fp))
        [o] = wait_for('报给自己的', lambda: reports(fo))
        self.assertEqual((m['code'], list(m['units'])), (200, [sw.aid_of('00' * 16, 'claude', 'alice@example.com')]), '维护者的只有 alice')
        self.assertEqual((o['code'], list(o['units'])), (200, [sw.aid_of(self.SALT, 'claude', 'bob@example.com')]), '自己的只有 bob')
        self.own.stop()
        self.session(a, self.BOB, SESSION_PID - 2)   # bob 又开了一个：自己的那一路要报，连不上
        self.assertIn('连不上', wait_for('自己的那一路没报成的原因', lambda: self.status(a, own=True)))
        self.session(a, Device.KEY, SESSION_PID - 3)   # alice 又开了一个：维护者那一路照报
        wait_for('维护者那一路又报了', lambda: len(reports(a.fp)) >= 2)
        self.assertEqual([r['code'] for r in reports(a.fp)], [200, 200])
        self.assertIn(self.status(a), (None, ''), '维护者那一路没有没报成的原因')

    def test_agent_rejoin(self):
        """被别的设备移除以后，上报进程那一路退避（10 分钟）；重新加入以后马上从头报，不等退避到点"""
        a, b = self.device('alice-mbp', {self.BOB: 'US-B'}), self.device('bob-mini', {self.BOB: 'US-B'})
        for d in (a, b):
            self.assertEqual(self.join(d, None, '--name', d.name)[0], 0)
        fa = self.own_fp(a)
        codes = lambda: [r['code'] for r in a.sent('report') if r['key'] == fa]
        self.session(a, self.BOB)
        self.agent(a)
        wait_for('第一次上报', lambda: codes() == [200])
        b.ok('hub-remove', 'apply', fa)
        self.session(a, self.BOB, SESSION_PID - 1)   # 会话变了：报上去，服务器已经不认它
        wait_for('被移除以后的那一次上报', lambda: codes() == [200, 403])
        self.assertIn('cca hub join', wait_for('没报成的原因', lambda: self.status(a, own=True)))
        self.assertFalse((a.cca / 'hub' / 'hub.json').exists(), '发现被移除了：本机记着的删掉')
        self.assertEqual(self.join(a, b.ok('hub-invite').strip())[0], 0, '拿新的链接重新加入（新的密钥）')
        fa2 = self.own_fp(a)
        wait_for('重新加入以后马上报', lambda: [r['code'] for r in a.sent('report') if r['key'] == fa2] == [200], secs=30)
        self.assertIn(self.status(a, own=True), (None, ''), '没报成的原因清掉了')

    def doc(self, dev):
        return json.loads((dev.cca / 'hub' / 'hub.json').read_text())

    def test_relink(self):
        """换加入链接（hub-invite --new）：旧的不能再用来加入，已经加入的照旧；别的设备问设备、要链接时换上新的"""
        a, b, c = (self.device(n, {self.BOB: 'US-B'}) for n in ('alice-mbp', 'bob-mini', 'carol-air'))
        for d in (a, b):
            self.assertEqual(self.join(d, None, '--name', d.name)[0], 0)
        self.assertEqual((self.doc(a)['seed'], self.doc(b)['seed']), (self.ADMIT, self.ADMIT), '加入时记下部署时的口令')
        gen = self.doc(a)['gen']
        self.assertRegex(gen, '^[0-9a-f]{32}$')
        inv = a.ok('hub-invite', '--new').strip()
        new = sw.link_decode(inv)['a']
        self.assertNotEqual(new, self.ADMIT)
        self.assertEqual((self.doc(a)['admit'], self.doc(a)['link'], self.doc(a)['seed']), (new, 1, self.ADMIT))
        self.assertEqual(self.doc(b)['admit'], self.ADMIT, 'b 还没问过服务器')
        self.assertEqual(b.ok('hub-invite').strip(), inv, 'b 要链接时先问服务器：换上新的')
        self.assertEqual((self.doc(b)['admit'], self.doc(b)['link'], self.doc(b)['gen']), (new, 1, gen))
        self.assertEqual(self.join(c, self.link())[0], 1, '旧的链接不能再用来加入')
        self.assertFalse((c.cca / 'hub').exists())
        self.assertEqual(self.join(c, inv, '--name', 'carol-air'), self.joined('carol-air', 3))
        self.assertEqual((self.doc(c)['admit'], 'seed' in self.doc(c)), (new, False), '拿换过的链接加入：不知道部署时的口令')
        c.ok('hub-devices')
        self.assertEqual(self.doc(c)['seed'], self.ADMIT, '问设备时从服务器存的密文里拿到')
        # 服务器不认这个请求（旧版服务器）：说清楚，本机不动
        self.own.stop()
        r = a.run('hub-invite', '--new')
        self.assertEqual((r.returncode, self.doc(a)['admit']), (1, new))
        self.assertIn('连不上', r.stdout)
        self.assertEqual(a.ok('hub-invite').strip(), inv, '连不上时照本机记着的打')

    def restart_own(self, admit=None, wipe=True):
        """自己的服务器重启：wipe 时数据目录清空（托管平台上没挂存储、数据目录删了）；admit：换成另一个配置的口令（从头部署）"""
        d = self.own.d
        self.own.stop()
        if wipe:
            for f in ('state.json', 'keys.json', 'cost-slots.json'):
                (d / f).unlink(missing_ok=True)
        self.own = FreshServer(d, admit_hash=sw.admit_hash(admit or self.ADMIT), port=self.own.port)

    def test_records_lost(self):
        """服务器的记录没了（数据目录清空、配置照旧）：设备下次找它时自己重新加入（gen 变了）。换过链接的拿现在的口令加入不了，
        再拿部署时的那个；发现了自己被移除的设备已经删掉了记着的，加不回来"""
        a, b, c = (self.device(n, {self.BOB: 'US-B'}) for n in ('alice-mbp', 'bob-mini', 'carol-air'))
        for d in (a, b, c):
            self.assertEqual(self.join(d, None, '--name', d.name)[0], 0)
        a.ok('hub-remove', 'apply', self.own_fp(c))   # 移除 c（换了链接）
        self.assertEqual(c.run('hub-devices').returncode, 1, 'c 发现被移除了')
        b.ok('hub-devices')   # b 换上新的链接
        gen = self.doc(a)['gen']
        self.restart_own()
        names = lambda d: [x['name'] for x in json.loads(d.ok('hub-devices', '--json'))['devices']]
        self.assertEqual(names(b), ['bob-mini'], '记录没了：b 自己重新加入，再问一次')
        doc = self.doc(b)
        self.assertTrue(doc['gen'] != gen and doc['admit'] == self.ADMIT and 'link' not in doc, '现在的口令不认，拿部署时的那个加入的')
        self.assertEqual([r['op'] for r in b.sent()[-3:]], ['join', 'join', 'members'], '先试现在的口令，再试部署时的那个')
        self.assertEqual(names(a), ['alice-mbp', 'bob-mini'], 'a 也自己重新加入')
        self.assertFalse((c.cca / 'hub' / 'hub.json').exists(), 'c 被移除时已经删掉了记着的：加不回来')
        self.assertEqual(len(json.loads((self.own.d / 'state.json').read_text())['machines']), 2)
        # 服务器从头部署过（配置了另一个口令）：拿记着的都加入不了，记下这一份，不再为它试
        self.restart_own(admit='99' * 16)
        r = a.run('hub-devices')
        self.assertEqual(r.returncode, 1)
        self.assertIn('拿记着的加入链接也加入不了', r.stdout)
        stale = self.doc(a)['stale']
        self.assertRegex(stale, '^[0-9a-f]{32}$')
        joins = len(a.sent('join'))
        self.assertEqual(a.run('hub-devices').returncode, 1)
        self.assertEqual(len(a.sent('join')), joins, '同一份记录不再试')
        # 重启但数据还在：照旧（不重新加入）
        self.restart_own(admit='99' * 16, wipe=False)
        self.assertEqual(json.loads((self.own.d / 'state.json').read_text())['own']['gen'], stale)

    def hosted(self, dev, *args, ok=True):
        """hub-hosted files …：成了是 {platform, dir, hash, update, steps}（照 bash 那样拆 sh 的几行），没成是 (原因码, 原因)"""
        r = dev.run('hub-hosted', 'files', *args)
        self.assertEqual(r.stderr, '')
        if not ok:
            self.assertEqual(r.returncode, 1, r.stdout)
            return tuple(r.stdout.strip().split('\t', 1))
        self.assertEqual(r.returncode, 0, r.stdout)
        return dict(x.split('=', 1) for x in shlex.split(r.stdout))

    def test_hosted(self):
        """部署到托管平台：生成部署文件（只有口令的哈希），还没用上的口令和盐记在 hub-hosted.json；拿平台给的地址加入（照证书机构验：
        链接里没有指纹）；加入着再生成是更新（哈希照部署时的口令，换过链接也不变）；自己部署的那种、不知道部署时的口令时不生成"""
        a = self.device('alice-mbp', {self.BOB: 'US-B'})
        self.assertEqual(a.ok('hub-hosted', 'platform'), '\n', '什么都没记着')
        f = self.hosted(a, 'railway')
        d = Path(f['dir'])
        self.assertEqual((d, f['platform'], f['update']), (a.home / 'cca-hub-railway', 'railway', ''))
        pend = a.cca / 'hub-hosted.json'
        doc = json.loads(pend.read_text())
        self.assertEqual((oct(pend.stat().st_mode & 0o777), doc['platform'], f['hash']), ('0o600', 'railway', sw.admit_hash(doc['admit'])))
        self.assertEqual(sorted(p.name for p in d.iterdir()), ['Dockerfile', 'README.md', 'cca-presence.py'])
        self.assertEqual((d / 'cca-presence.py').read_text(), SERVER.read_text(), '服务端程序就是这一版')
        docker = (d / 'Dockerfile').read_text()
        self.assertIn('ENV CCA_HUB_HOSTED=1 CCA_HUB_ADMIT_HASH=' + f['hash'], docker)
        self.assertIn('CMD ["python3", "-I", "/app/cca-presence.py"]', docker)
        for p in d.iterdir():
            self.assertNotIn(doc['admit'], p.read_text(), '口令、盐不进部署文件（只有哈希）')
            self.assertNotIn(doc['salt'], p.read_text())
        self.assertEqual(f['steps'].split('\n')[-1], 'railway domain --port 8080')
        self.assertIn(f['steps'], (d / 'README.md').read_text())
        self.assertEqual(a.ok('hub-hosted', 'platform').strip(), 'railway')
        self.assertEqual(self.hosted(a, 'railway')['hash'], f['hash'], '再运行一次：照旧用记着的那一份（已经部署了的照样认）')
        busy = a.home / 'busy'
        busy.mkdir()
        (busy / 'notes.txt').write_text('x')
        (busy / '.DS_Store').write_text('x')
        self.assertEqual(self.hosted(a, 'sealos', '--dir', str(busy), ok=False)[0], 'dir', '目录里有别的文件：不碰')
        s = self.hosted(a, 'sealos', '--dir', '~/sealos')
        self.assertEqual((s['dir'], s['hash']), (str(a.home / 'sealos'), f['hash']), '换个平台也是同一份口令')
        self.assertEqual(sorted(p.name for p in Path(s['dir']).iterdir()), ['README.md', 'cca-presence.py'])
        self.assertIn('环境变量：CCA_HUB_ADMIT_HASH=' + f['hash'], s['steps'])
        self.assertEqual(a.ok('hub-hosted', 'platform').strip(), 'sealos', '记着的是最近一次的平台')

        # 平台上部署好了：证书机构签的证书（平台的），地址是平台给的
        o = self.d / ('hosted-' + secrets.token_hex(4))
        o.mkdir()
        make_ca_cert(o)
        srv = FreshServer(o, admit_hash=f['hash'])
        self.addCleanup(lambda: srv.proc.poll() is not None or srv.stop())
        url = 'https://127.0.0.1:%d' % srv.port
        why = lambda r: (r.returncode, r.stdout.split('\t')[0])
        self.assertEqual(why(a.run('hub-hosted', 'join', 'http://127.0.0.1:%d' % srv.port)), (1, 'url'), '要 https')
        self.assertEqual(why(a.run('hub-hosted', 'join', url + '/v1/health')), (1, 'url'), '不带路径')
        self.assertEqual(why(a.run('hub-hosted', 'join', url)), (1, 'cert'), '系统的证书机构不认这个证书')
        self.assertFalse((a.cca / 'hub').exists())
        self.assertTrue(pend.exists(), '没加入成：记着的那一份留着')
        a.env['CURL_CA_BUNDLE'] = str(o / 'ca.pem')
        r = a.run('hub-hosted', 'join', ' %s/ ' % url, '--name', 'alice-mbp')
        self.assertEqual((r.returncode, r.stdout.strip()), (0, 'alice-mbp\n127.0.0.1:%d\n1' % srv.port), r.stdout)
        self.assertFalse(pend.exists(), '加入成了：记着的那一份删掉')
        hub = self.doc(a)
        self.assertEqual((hub['pins'], hub['hosted'], hub['seed'], hub['admit']), ([], 'sealos', doc['admit'], doc['admit']))
        argv = (a.home / 'curl.argv').read_text().strip().split('\n')[-1]
        self.assertTrue('--pinnedpubkey' not in argv and ' -k ' not in argv and argv.endswith('/v1/join'), argv)
        h = json.loads(a.ok('hub-status', '--json'))['hub']
        self.assertEqual((h['pinned'], h['members']), (False, 1))
        self.assertEqual(why(a.run('hub-deploy', 'root@example.com')), (1, 'platform'), '托管平台上的不经 SSH 部署、更新')
        self.assertEqual(why(a.run('hub-hosted', 'join', url)), (1, 'pending'), '没有还没用上的口令了')

        # 更新：哈希照部署时的口令（换过链接也不变）
        inv = a.ok('hub-invite', '--new').strip()
        u = self.hosted(a, 'railway')
        self.assertEqual((u['update'], u['hash'], u['steps'].split('\n')[-1]), ('1', f['hash'], 'railway up --detach'))
        self.assertFalse((a.cca / 'hub-hosted.json').exists(), '更新不记口令')
        b = self.device('bob-mini', {self.BOB: 'US-B'})
        b.env['CURL_CA_BUNDLE'] = str(o / 'ca.pem')
        self.assertEqual(self.join(b, inv, '--name', 'bob-mini')[0], 0)
        self.assertNotIn('seed', self.doc(b), '拿换过的链接加入：还不知道部署时的口令')
        self.assertEqual(self.hosted(b, 'sealos')['hash'], f['hash'], '生成前先问一次服务器：换链接的设备存的密文里有')
        self.assertEqual(self.doc(b)['seed'], doc['admit'])
        # 别处拿新的口令重新部署过（平台上配置的哈希换了，服务器从头开始）：记着原来那一份的设备不生成更新——照它记着的口令生成的
        # 部署文件会让服务器又从头开始
        srv.stop()
        srv = FreshServer(o, admit_hash=sw.admit_hash(secrets.token_hex(16)), port=srv.port)
        code, out = self.hosted(a, 'railway', ok=False)
        self.assertEqual(code, 'denied', out)
        self.assertIn('先不生成', out)
        self.assertIn('stale', self.doc(a), '拿记着的口令重新加入也不认：记下这一份')
        srv.stop()
        hb = self.doc(b)
        hb.pop('seed')
        (b.cca / 'hub' / 'hub.json').write_text(json.dumps(hb))
        self.assertEqual(self.hosted(b, 'railway', ok=False)[0], 'seed', '不知道部署时的口令：不生成（哈希不对的话服务器会丢掉所有设备）')
        c = self.device('carol-air', {self.BOB: 'US-B'})
        self.assertEqual(self.join(c, None, '--name', 'carol-air')[0], 0)
        self.assertEqual(self.hosted(c, 'railway', ok=False)[0], 'not_hosted', '加入的是自己部署的那种：先退出')

    def test_ca_mode(self):
        """加入链接里没有指纹（托管平台、反向代理后面）：照系统的证书机构验证书（不带 -k、--pinnedpubkey）；测试服务器的证书是自签的，
        验不过：原因码 cert，什么都不留"""
        a = self.device('alice-mbp', {self.BOB: 'US-B'})
        link = sw.link_encode('https://127.0.0.1:%d' % self.own.port, [], self.ADMIT, self.SALT)
        code, out = self.join(a, link)
        self.assertEqual((code, out.split('\t')[0]), (1, 'cert'), out)
        self.assertIn('证书验证不过', out)
        argv = (a.home / 'curl.argv').read_text()
        self.assertTrue('--pinnedpubkey' not in argv and ' -k ' not in argv, argv)
        self.assertFalse((a.cca / 'hub').exists())

    def test_routing(self):
        """一台设备两台服务器：映射里的 alice 只到维护者的，自己的 bob 只到自己的（设备统计、记下的 API key、折合金额都是）"""
        a = self.device('alice-mbp', {Device.KEY: 'US-A', self.BOB: 'US-B'})
        self.machines(a)
        a.login('u-alice-0001')
        a.login('u-bob-0001', self.BOB)
        self.assertEqual(self.join(a, None, '--name', 'alice-mbp')[0], 0)
        fo = self.own_fp(a)
        mine = {a.fp: {sw.aid_of('00' * 16, 'claude', 'alice@example.com')}, fo: {sw.aid_of(self.SALT, 'claude', 'bob@example.com')}}

        def aids(op):
            """各台服务器上问过、传过哪些账号：{设备指纹: {账号摘要…}}"""
            out = {}
            for r in a.sent(op):
                got = set(r.get('get') or []) | set(r.get('put') or {}) | set(r.get('accounts') or [])
                out.setdefault(r['key'], set()).update(got)
            return out
        # 记下的 API key：两个账号都同步，各走各的
        self.assertEqual((a.keyset(self.K1).strip(), a.keyset(self.K2, self.BOB).strip()), ('shared', 'shared'))
        self.assertEqual(a.sync(), '')
        self.assertEqual(aids('keys'), mine)
        self.assertEqual([k.get('error') for k in (a.keys_json(), json.loads((a.cca / 'presence' / 'own.keys.json').read_text()))], [None, None],
                         '两台服务器的状态各记各的')
        # 折合金额
        hour = int(time.time() // 3600 * 3600) - 3600
        (a.cca / 'cost').mkdir(exist_ok=True)
        (a.cca / 'cost' / 'local.json').write_text(json.dumps({'v': 1, 'build': 'x', 'who': {'claude:u-alice-0001': 'alice@example.com', 'claude:u-bob-0001': 'bob@example.com'},
                                                               'acc': {'claude:u-alice-0001': {str(hour): [1000000, 1, 1, 1, 0, 0, 0]},
                                                                       'claude:u-bob-0001': {str(hour): [2000000, 1, 1, 1, 0, 0, 0]}},
                                                               'um': {}, 'orphan': {}}))
        r = a.cost_sync()
        self.assertEqual((r['state'], [(h['kind'], h['state']) for h in r['hubs']]), ('ok', [('maintainer', 'ok'), ('own', 'ok')]), r)
        self.assertEqual(aids('ledger'), mine)
        bob_aid = sw.aid_of(self.SALT, 'claude', 'bob@example.com')
        blob = json.loads((self.own.d / 'cost-slots.json').read_text())['acc'][bob_aid][fo]['blob']
        doc = sw.cost_open(sw.derive(sw.COST, self.SALT, 'claude', 'bob@example.com', 'u-bob-0001'), bob_aid, fo, blob)
        self.assertEqual((doc['dv'], list(doc['h'].values())), (a.config()['device_id'], [[2000000, 1, 1, 1, 0, 0, 0]]),
                         '照文档解得开；带着这台机器的标识')
        self.assertTrue((a.cca / 'cost' / 'own.sync.json').exists() and (a.cca / 'cost' / 'sync.json').exists())
        # 设备统计：各问各的
        v = json.loads(a.ok('presence-view', '--json'))
        self.assertEqual([(x['key'], x['hub']) for x in v['accounts']], [(Device.KEY, 'maintainer'), (self.BOB, 'own')])
        self.assertEqual([h['kind'] for h in v['hubs']], ['maintainer', 'own'])
        self.assertEqual(aids('view'), mine)
        spec = {'config': {'device_id': a.config()['device_id']}, 'presence': 'load', 'own': 'load', 'tools': ['claude'],
                'accounts': [['claude', Device.KEY, None, True, True], ['claude', self.BOB, None, False, True]], 'sync': []}
        got = json.loads(a.ok('_cost-view', repr(time.time()), json.dumps(spec)))['accounts']
        self.assertEqual([(x['sync']['hub'], x['sync']['state'], x['devices'][0]['id']) for x in got],
                         [('maintainer', 'ok', a.config()['device_id']), ('own', 'ok', a.config()['device_id'])], 'App 看到的：各走哪一台')


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
