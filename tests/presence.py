#!/usr/bin/env python3
"""连接设备（presence）：服务端的验签、设备清单、状态；客户端的扫描、闲置判定、上报内容。
真的 ssh-keygen、真的 TLS 服务器（127.0.0.1，随机端口），不联网。"""
import base64
import contextlib
import fcntl
import http.client
import importlib.util
import io
import json
import os
from pathlib import Path
import pty
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
CCA = ROOT / 'cca'
SOURCE = CCA.read_text().split("PY_SRC=$(cat <<'PY'\n", 1)[1].split('\nPY\n)', 1)[0]
DEFINITIONS = SOURCE.split('\nif len(sys.argv) < 2', 1)[0]

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


def openssl_pin(cert):
    """维护者那边（部署脚本、文档）算公钥指纹的办法：和服务器的 --print-pin 必须一样"""
    pub = subprocess.run(['openssl', 'x509', '-in', str(cert), '-pubkey', '-noout'], stdout=subprocess.PIPE, check=True).stdout
    der = subprocess.run(['openssl', 'pkey', '-pubin', '-outform', 'der'], input=pub, stdout=subprocess.PIPE, check=True).stdout
    import hashlib
    return 'sha256//' + base64.b64encode(hashlib.sha256(der).digest()).decode()


class Server:
    """跑一个真的服务器进程（ttl 2 秒、闲置 3 秒，测试不用干等）"""
    def __init__(self, d, ttl='2', idle='3'):
        self.d = d
        args = [sys.executable, '-I', str(SERVER), '--listen', '127.0.0.1:0', '--port-file', str(d / 'port'),
                '--cert', str(d / 'cert.pem'), '--key', str(d / 'key.pem'), '--release-key', str(d / 'release.pub'),
                '--state', str(d / 'state.json'), '--ttl', ttl, '--idle', idle]
        (d / 'port').unlink() if (d / 'port').exists() else None
        self.proc = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        for _ in range(100):
            if (d / 'port').exists():
                break
            time.sleep(0.05)
        self.port = int((d / 'port').read_text())

    def stop(self):
        self.proc.terminate()
        self.proc.wait(5)
        with self.proc.stderr:
            return self.proc.stderr.read().decode()

    def req(self, method, path, body=b'', sig=None):
        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        ctx.check_hostname = False
        ctx.verify_mode = ssl.CERT_NONE   # 测试里不验 CA：客户端靠的是公钥钉住（这里另外核对）
        c = http.client.HTTPSConnection('127.0.0.1', self.port, context=ctx, timeout=10)
        headers = {'Content-Type': 'application/json'}
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


class ClientTest(unittest.TestCase):
    """客户端（PY_SRC）：映射里那一节的检查、账号摘要、本机会话的扫描和闲置判定、经 curl + 公钥钉住连服务器"""
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='cca-presence-client-')
        d = cls.d = Path(cls.tmp.name).resolve()
        make_cert(d)
        cls.relpub = keygen(d / 'rel', 'test-release')
        (d / 'release.pub').write_text(cls.relpub + ' test-release\n')
        cls.srv = Server(d, ttl='60', idle='600')
        cls.pin = openssl_pin(d / 'cert.pem')

    @classmethod
    def tearDownClass(cls):
        cls.srv.stop()
        cls.tmp.cleanup()

    def setUp(self):
        self.home = self.d / ('home-%s' % secrets.token_hex(4))
        cca = self.home / '.cca'
        (cca / 'policy').mkdir(parents=True)
        with mock.patch.dict(os.environ, {'HOME': str(self.home), 'CCA_HOME': str(cca), 'CCA_TOOL': 'claude',
                                         'PATH': os.environ.get('PATH', '/usr/bin:/bin')}, clear=True):
            self.ns = {}
            exec(compile(DEFINITIONS, str(CCA), 'exec'), self.ns)
        self.cca = cca
        self.key = 'alice(alice@example.com)'
        self.salt = '00' * 16
        self.pub = keygen(cca / 'policy' / 'id_ed25519', 'cca-policy@test')
        self.policy({'url': 'https://127.0.0.1:%d' % self.srv.port, 'pins': [self.pin], 'salt': self.salt})
        (cca / 'config.json').write_text('{}')
        (cca / 'accounts.json').write_text(json.dumps({self.key: 'US-A', 'bob@example.com': 'US-B'}))
        self.ns['ctx'] = lambda: ({}, json.loads((cca / 'accounts.json').read_text()), {})
        self.ns['keyshare_kick'] = lambda force=False: None   # cca devices 顺带在后台同步 API key：测试里不起进程（同步另外测）
        self.ns['cost_kick'] = lambda: None   # 折合金额也一样

    def policy(self, presence):
        j = {'version': 1, 'claude': {'alice@example.com': 'US-A'}}
        if presence is not None:
            j['presence'] = presence
        (self.cca / 'policy' / 'policy.json').write_text(json.dumps(j))

    def machines(self, ms):
        doc = json.dumps({'version': 1, 'seq': int(time.time() * 1000), 'machines': ms}).encode()
        st, _ = self.srv.req('POST', '/v1/machines', doc, sign(self.d / 'rel', 'cca-devices', doc))
        self.assertEqual(st, 200)

    def test_policy_block(self):
        pp = self.ns['presence_problems']
        ok = {'url': 'https://203.0.113.10:18443', 'pins': ['sha256//' + 'A' * 43 + '='], 'salt': 'ab' * 16}
        self.assertEqual(pp(ok), [])
        for bad in ({**ok, 'url': 'http://203.0.113.10:18443'}, {**ok, 'url': 'https://203.0.113.10'},
                    {**ok, 'url': 'https://203.0.113.10:99999'}, {**ok, 'pins': []}, {**ok, 'pins': ['sha1//x']},
                    {**ok, 'salt': 'XYZ'}, 'nonsense'):
            self.assertTrue(pp(bad), bad)
        strict = self.ns['policy_problems']
        j = {'version': 1, 'claude': {}, 'presence': ok}
        self.assertEqual(strict(j, True), [], '维护者的严格检查认识 presence')
        self.assertTrue(strict(dict(j, presence={'url': 'x'}), True))
        self.assertEqual(strict(dict(j, presence={'url': 'x'})), [], '客户端不因为它拒绝整份映射')

    def test_identity(self):
        fp = subprocess.run(['ssh-keygen', '-l', '-E', 'sha256', '-f', str(self.cca / 'policy' / 'id_ed25519.pub')],
                            stdout=subprocess.PIPE, text=True, check=True).stdout.split()[1]
        self.assertEqual(self.ns['ssh_fp'](self.pub.split()[1]), fp, '和 ssh-keygen -l 的指纹一样')
        aid = self.ns['presence_aid']
        C, X = self.ns['CLAUDE'], self.ns['CODEX']
        a = aid(self.salt, C, 'Alice@Example.com')
        self.assertRegex(a, r'^[0-9a-f]{32}$')
        self.assertEqual(a, aid(self.salt, C, 'alice@example.com'), '邮箱不分大小写')
        self.assertNotEqual(a, aid(self.salt, X, 'alice@example.com'), 'Claude 账号和 Codex 账号不撞')
        self.assertNotEqual(a, aid('11' * 16, C, 'alice@example.com'), '换了盐就认不出')
        reason, cfg = self.ns['presence_load']()
        self.assertEqual((reason, cfg['fp']), ('', fp))
        (self.cca / 'config.json').write_text('{"presence": false}')
        self.assertIn('关掉了', self.ns['presence_load']()[0])
        (self.cca / 'config.json').write_text('{}')
        self.policy(None)
        self.assertIn('还没有设备统计', self.ns['presence_load']()[0])
        self.policy({'url': 'bad'})
        self.assertIn('不对', self.ns['presence_load']()[0])

    def mark(self, pid, started, since):
        d = Path(self.ns['adir'](self.key))
        d.mkdir(parents=True, exist_ok=True)
        (d / ('cli-%d.json' % pid)).write_text(json.dumps({'v': 1, 'tool': 'claude', 'id': self.ns['acct_id'](self.key),
                                                          'pid': pid, 'started': started, 'since': since}))
        return d

    def test_scan_cli_idle_from_tty(self):
        now = time.time()
        d = self.mark(4242, 1000, now - 300)
        master, slave = pty.openpty()
        self.addCleanup(os.close, master)
        self.addCleanup(os.close, slave)
        tty = os.ttyname(slave)
        os.utime(tty, (now - 120, now - 200))   # 最后一次输入 120 秒前、输出 200 秒前
        targets = self.ns['presence_targets']()
        self.assertEqual([(t.name, k) for t, k, _ in targets], [('claude', self.key)], '只有映射里的官方账号')
        scan = lambda procs: self.ns['presence_scan'](self.salt, targets, procs, {}, now, 1000.0)
        units, pids = scan({4242: (1, tty[len('/dev/'):], 1.0, 1000, 'bash cca')})   # ps 的写法：macOS 是 ttys005，Linux 是 pts/5
        aid = self.ns['presence_aid'](self.salt, self.ns['CLAUDE'], 'alice@example.com')
        self.assertEqual(units, {aid: {'cli': [[300, 120]], 'app': None}}, '闲置 = 现在 - 最后一次输入 / 输出')
        self.assertEqual(pids, {4242}, '盯着这个会话的 cca 进程退出')
        self.assertEqual(scan({4242: (1, tty[len('/dev/'):], 1.0, 1003, 'bash cca')})[0], {}, '进程号被重用（启动时间对不上）：不算')
        self.assertEqual(scan({})[0], {}, 'cca 被 kill -9 留下的登记：进程没了，不算')
        self.assertEqual(scan({4242: (1, '??', 1.0, 1000, 'bash cca')})[0][aid]['cli'], [[300, 0]], '没有终端：在跑就算在干活')

    def test_scan_app_cpu(self):
        now = time.time()
        d = Path(self.ns['adir'](self.key))
        cmd = '/Applications/Claude.app/Contents/MacOS/Claude --user-data-dir=%s/desktop --proxy-server=x' % d
        procs = lambda cpu: {500: (1, '??', cpu, round(now - 3600), cmd), 501: (500, '??', 5.0, round(now - 3600), 'Claude Helper')}
        targets = self.ns['presence_targets']()
        apps = {}
        scan = lambda cpu, t, mono: self.ns['presence_scan'](self.salt, targets, procs(cpu), apps, t, mono)[0]
        aid = self.ns['presence_aid'](self.salt, self.ns['CLAUDE'], 'alice@example.com')
        self.assertEqual(scan(10.0, now, 1000.0)[aid]['app'], [3600, 600], '开了很久、第一次见到：先当作刚好闲置满 10 分钟')
        self.assertEqual(scan(10.5, now + 40, 1040.0)[aid]['app'][1], 640, 'CPU 几乎没动：还是闲置')
        self.assertEqual(scan(14.0, now + 80, 1080.0)[aid]['app'][1], 0, '一拍里用掉不少 CPU（含子进程）：有动静')
        self.assertEqual(scan(14.0, now + 90, 1090.0)[aid]['app'][1], 10, '隔得太近的采样不算，最后动静照旧')
        self.assertEqual(scan(14.0, now + 90, 1090.0).get(aid, {}).get('cli'), [], '没有命令行会话')
        other = procs(1.0)
        other[500] = other[500][:4] + ('/Applications/Claude.app/Contents/MacOS/Claude --user-data-dir=%s/desktop-x' % d,)
        self.assertEqual(self.ns['presence_scan'](self.salt, targets, other, {}, now, 1.0)[0], {}, '别的资料目录不算')

    def test_post_and_devices(self):
        _, cfg = self.ns['presence_load']()
        aid = self.ns['presence_aid'](self.salt, self.ns['CLAUDE'], 'alice@example.com')
        code, j = self.ns['presence_post'](cfg, 'report', {'units': {aid: {'cli': [[100, 0]], 'app': None}}, 'end': None})
        self.assertEqual((code, j.get('error')), (403, 'denied'), '设备清单里还没有这台 Mac')
        self.assertIn('presence sync', self.ns['presence_why'](cfg, code, j))
        self.machines([{'key': self.pub, 'name': 'alice-mbp'}])
        code, j = self.ns['presence_post'](cfg, 'report', {'units': {aid: {'cli': [[100, 0]], 'app': None}}, 'end': None})
        self.assertEqual(code, 200, j)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.ns['cmd_presence_view']('--json')
        doc = json.loads(out.getvalue())
        self.assertEqual((doc['kind'], doc['me'], doc['tool']), ('devices', 'alice-mbp', 'claude'))
        [a] = doc['accounts']
        self.assertEqual(a['key'], self.key)
        [dev] = a['devices']
        self.assertEqual((dev['name'], dev['me'], dev['state'], dev['cli'], dev['app']), ('alice-mbp', True, 'in_use', 1, False))
        self.assertTrue(abs(dev['since_epoch'] - (time.time() - 100)) < 5 and abs(dev['last_used_epoch'] - time.time()) < 5,
                        '时间换算成这台 Mac 的时钟')
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.ns['cmd_presence_view']()
        self.assertIn('alice-mbp（这台 Mac）  在用 · 命令行里 1 个会话 · ', out.getvalue())
        self.assertIn(' 起', out.getvalue())
        out = io.StringIO()
        with contextlib.redirect_stdout(out), self.assertRaises(SystemExit) as e:
            self.ns['cmd_presence_view']('bob')
        self.assertEqual(e.exception.code, 2)
        self.assertIn('不在维护者的映射里', out.getvalue())
        bad = dict(cfg, pins=['sha256//' + 'A' * 43 + '='])
        self.assertEqual(self.ns['presence_post'](bad, 'view', {'accounts': []}), (0, {'error': 'pin'}), '证书不是钉住的那张：不发')
        gone = dict(cfg, url='https://127.0.0.1:9')
        self.assertEqual(self.ns['presence_post'](gone, 'view', {'accounts': []})[0], 0)

    def test_past_time(self):
        when = self.ns['past_time']
        now = time.mktime((2026, 11, 10, 15, 30, 0, 0, 0, -1))
        self.assertEqual(when(now - 3600, now), '14:30')
        self.assertEqual(when(time.mktime((2026, 11, 9, 18, 40, 0, 0, 0, -1)), now), '昨天 18:40')
        self.assertEqual(when(time.mktime((2026, 11, 6, 9, 5, 0, 0, 0, -1)), now), '周五 09:05')
        self.assertEqual(when(time.mktime((2026, 10, 20, 9, 5, 0, 0, 0, -1)), now), '10 月 20 日')
        self.assertEqual(when(time.mktime((2025, 10, 20, 9, 5, 0, 0, 0, -1)), now), '2025 年 10 月 20 日')

    def test_agent_backoff(self):
        """报不出去（服务器连不上、不认这台 Mac）：按退避再试，心跳到期、会话变了都不会马上重发"""
        ns, clock = self.ns, {'t': 1000.0}

        class FakeTime:
            def monotonic(self):
                return clock['t']

            def time(self):
                return 1794042000.0 + clock['t']

            def __getattr__(self, name):
                return getattr(time, name)

        class Done(Exception):
            pass
        codes = iter([200, 0, 0, 403, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])
        posts, waits = [], []

        def post(cfg, op, payload, timeout=8):
            posts.append(clock['t'])
            c = next(codes)
            return (c, {}) if c else (0, {'error': 'unreachable'})

        def wait(pids, dirs, timeout):
            waits.append(timeout)
            if len(waits) >= 12:
                raise Done()
            clock['t'] += max(timeout, 1.0)
        unit = {'a' * 32: {'cli': [[1, 0]], 'app': None}}
        ns.update(time=FakeTime(), presence_post=post, presence_wait=wait, presence_stamp=lambda: 1,
                  presence_targets=lambda: [(ns['CLAUDE'], self.key, '/nonexistent')], presence_procs=lambda: {},
                  presence_scan=lambda *a: (dict(unit), set()),
                  keyshare_sync=lambda force=False: syncs.append((clock['t'], force)))
        syncs = []
        with self.assertRaises(Done):
            ns['cmd_presence_agent']()
        self.assertTrue(syncs and syncs[0] == (1000.0, False), '开着会话：马上同步一次记下的 API key（不 force：最近同步过就不发）')
        self.assertTrue(all(b - a >= 600 for (a, _), (b, _) in zip(syncs, syncs[1:])), syncs)
        gaps = [round(b - a) for a, b in zip(posts, posts[1:])]
        self.assertGreaterEqual(gaps[0], 54, '报成了：下一次是心跳（60 秒左右）')
        self.assertEqual(gaps[1:3], [10, 20], '报不出去：按退避再试，不是每圈都重发')
        self.assertEqual(gaps[3], 600, '服务器不认这台 Mac（403）：10 分钟后再试')
        self.assertTrue(all(g >= 10 for g in gaps[1:]) and all(w >= 0 for w in waits), (gaps, waits))

    # 共享 API key：官方账号记下备查的那一把，经服务器加密同步给登录了这个账号的其他设备
    K1, K2, K3 = 'sk-ant-api03-' + 'a' * 40, 'sk-ant-api03-' + 'b' * 50, 'sk-ant-api03-' + 'c' * 30

    @staticmethod
    def nokeychain(*a, **k):
        raise AssertionError('测试不碰钥匙串（CCA_KEYSTORE=file）')

    def device(self, name, uuid):
        """另一台设备：自己的 ~/.cca 和 deploy key，同一份映射、同一个账号；uuid 不是 None 时 alice 在它上面登录着"""
        cca = self.d / ('%s-%s' % (name, secrets.token_hex(4))) / '.cca'
        (cca / 'policy').mkdir(parents=True)
        with mock.patch.dict(os.environ, {'HOME': str(cca.parent), 'CCA_HOME': str(cca), 'CCA_TOOL': 'claude',
                                         'PATH': os.environ.get('PATH', '/usr/bin:/bin')}, clear=True):
            ns = {}
            exec(compile(DEFINITIONS, str(CCA), 'exec'), ns)
        ns['cost_kick'] = lambda: None
        pub = keygen(cca / 'policy' / 'id_ed25519', 'cca-policy@' + name)
        (cca / 'policy' / 'policy.json').write_text((self.cca / 'policy' / 'policy.json').read_text())
        (cca / 'config.json').write_text('{}')
        (cca / 'accounts.json').write_text(json.dumps({self.key: 'US-A'}))
        self.login(ns, uuid)
        return ns, cca, pub

    def login(self, ns, uuid):
        cd = Path(ns['cdir'](self.key))
        cd.mkdir(parents=True, exist_ok=True)
        (cd / '.claude.json').write_text(json.dumps({'oauthAccount': {'accountUuid': uuid, 'emailAddress': 'alice@example.com'}} if uuid else {}))
        ns['ACCT_JSON'].clear()

    def keyset(self, ns, s, key=None):
        """在这台设备上记下（cca key 账号）：cmd_secret_set，只是不在后台起同步。返回 (stdout, 起同步的次数)"""
        kicks, out = [], io.StringIO()
        with mock.patch.dict(ns, {'keyshare_kick': lambda force=False: kicks.append(force), 'security': self.nokeychain}), \
                mock.patch('sys.stdin', io.StringIO(s)), contextlib.redirect_stdout(out):
            ns['cmd_secret_set'](key or self.key)
        return out.getvalue(), kicks

    def sync(self, ns):
        with mock.patch.dict(ns, {'security': self.nokeychain}):
            return ns['keyshare_sync'](True)

    def stored(self, ns):
        return ns['keyfile_read']('reference', ns['kc_svc'](ns['acct_id'](self.key), True))

    def server_blob(self, aid):
        return json.loads((self.d / 'keys.json').read_text())['acc'][aid]['blob']

    def test_keyshare_crypto(self):
        """密文的格式：两把密钥由盐、工具、邮箱、登录身份派生；加密后认证（带着账号摘要）；明文补齐到 256 字节的整数倍"""
        ns = self.ns
        C, X = ns['CLAUDE'], ns['CODEX']
        k = ns['keyshare_keys'](self.salt, C, 'Alice@Example.com', 'u-1234-5678')
        self.assertEqual(k, ns['keyshare_keys'](self.salt, C, 'alice@example.com', 'u-1234-5678'), '邮箱不分大小写')
        self.assertEqual(k, sw.derive(sw.KEYSHARE, self.salt, 'claude', 'alice@example.com', 'u-1234-5678'), '和文档写的一样')
        aid, key = 'a' * 32, 'sk-ant-api03-' + 'x' * 60
        b = ns['keyshare_seal'](k, aid, key, 1794041280.5)
        self.assertEqual(ns['keyshare_open'](k, aid, b), {'key': key, 'at': 1794041280.5})
        self.assertEqual(sw.keyshare_open(k, aid, b)['key'], key, '照文档能解开')
        self.assertEqual(ns['keyshare_open'](k, aid, sw.keyshare_seal(k, aid, {'v': 1, 'key': key, 'at': 5.0})), {'key': key, 'at': 5.0},
                         '照文档封的 cca 也认')
        self.assertEqual(srvmod.parse_blob(b), b, '服务器收这个形状')
        t = ns['keyshare_seal'](k, aid, None, 1794041281.0)
        self.assertEqual(ns['keyshare_open'](k, aid, t), {'key': None, 'at': 1794041281.0})
        self.assertEqual(len(t['c']), len(b['c']), '删除和 key 一样长：服务器分不出来')
        self.assertNotEqual(ns['keyshare_seal'](k, aid, key, 1.0)['c'], ns['keyshare_seal'](k, aid, key, 1.0)['c'], '每次的 nonce 不同')
        for o in (ns['keyshare_keys'](self.salt, C, 'alice@example.com', 'u-other-0000'),   # 没登录这个账号（身份不对）
                  ns['keyshare_keys']('11' * 16, C, 'alice@example.com', 'u-1234-5678'),  # 换了盐
                  ns['keyshare_keys'](self.salt, X, 'alice@example.com', 'u-1234-5678')):  # 同一个邮箱的 Codex 账号
            self.assertIsNone(ns['keyshare_open'](o, aid, b))
        self.assertIsNone(ns['keyshare_open'](k, 'b' * 32, b), '挪到别的账号上：认证不过')
        c = bytearray(base64.b64decode(b['c']))
        c[5] ^= 1
        self.assertIsNone(ns['keyshare_open'](k, aid, dict(b, c=base64.b64encode(bytes(c)).decode())), '改过的')
        for junk in (None, {}, 'x', {'n': 'x', 'c': 'y', 't': 'z'}, dict(b, n=12), dict(b, n=b['t'])):
            self.assertIsNone(ns['keyshare_open'](k, aid, junk), junk)
        for doc in ({'v': 2, 'key': key, 'at': 5.0}, {'v': 1, 'key': key, 'at': 0}, {'v': 1, 'key': key, 'at': 'x'},
                    {'v': 1, 'key': 'short', 'at': 5.0}, {'v': 1, 'key': '带空格 的 key', 'at': 5.0}, [1]):
            self.assertIsNone(ns['keyshare_open'](k, aid, sw.keyshare_seal(k, aid, doc)), '认证过、内容不对的也不认：%s' % doc)
        lb = ns['keyshare_seal'](k, aid, 'k' * 4096, 1.0)
        self.assertEqual(srvmod.parse_blob(lb), lb, '最长的 key（4096 个字符）也在服务器收的长度以内')

    def test_keyshare(self):
        """两台登录了 alice 的设备（alice-mbp、bob-mini）和一台没登录的（carol-air）：记下、换掉、删掉都同步，新的胜出；
        服务器、没登录的设备看不到 key；重放旧的、伪造的不认，传回自己的（自己修好）；删账号只删这台设备上的"""
        ns, K1, K2, K3 = self.ns, self.K1, self.K2, self.K3
        nsb, ccab, pubb = self.device('bob-mini', 'u-alice-0001')
        nsc, ccac, pubc = self.device('carol-air', None)
        self.machines([{'key': self.pub, 'name': 'alice-mbp'}, {'key': pubb, 'name': 'bob-mini'}, {'key': pubc, 'name': 'carol-air'}])
        self.login(ns, 'u-alice-0001')
        aid = ns['presence_aid'](self.salt, ns['CLAUDE'], 'alice@example.com')
        acct = ns['acct_id'](self.key)
        with mock.patch.dict(os.environ, {'CCA_KEYSTORE': 'file'}):
            self.assertEqual(self.keyset(ns, K1, 'bob@example.com'), ('', []), '不在映射里的账号：只记在这台设备上，不同步')
            out, kicks = self.keyset(ns, K1)
            self.assertEqual((out.strip(), kicks), ('shared', [True]), '映射里的账号、开着设备统计：说一声会同步，马上在后台同步')
            self.assertEqual(oct(os.stat(self.cca / 'presence' / 'keys.json').st_mode & 0o777), '0o600')
            self.assertEqual(self.sync(ns), '')
            raw = (self.d / 'keys.json').read_text()
            for secret in (K1, 'alice@example.com', 'u-alice-0001', 'a' * 40):
                self.assertNotIn(secret, raw, '服务器上没有 key、邮箱、登录身份')
            self.assertEqual(list(json.loads(raw)['acc']), [aid])
            at1 = ns['cfg_load']()['key_hints'][acct]['at']
            self.assertEqual(sw.keyshare_open(sw.derive(sw.KEYSHARE, self.salt, 'claude', 'alice@example.com', 'u-alice-0001'), aid, self.server_blob(aid))['at'], at1,
                             '传上去的时间是记下的时间')
            self.assertEqual(self.sync(nsc), '', '没登录 alice 的设备：不问也不传')
            self.assertFalse((ccac / 'secrets').exists())
            self.assertEqual(self.sync(nsb), '')
            self.assertEqual(self.stored(nsb), (0, K1), '登录了 alice 的另一台设备取回来了')
            hb = nsb['gui_api_key'](nsb['cfg_load'](), self.key, 'US-A', True)
            self.assertEqual((hb['saved'], hb['hint'], hb['from_device'], hb['saved_epoch'], hb['shared'], hb['sync_error']),
                             (True, ns['key_hint'](K1), 'alice-mbp', at1, True, None), 'App 照记下的说：从哪台设备来的、什么时候记下的')
            ha = ns['gui_api_key'](ns['cfg_load'](), self.key, 'US-A', True)
            self.assertEqual((ha['from_device'], ha['saved_epoch']), (None, at1), '这台设备自己记下的：没有来源')
            blob1 = self.server_blob(aid)
            # bob-mini 换了一把：新的胜出，alice-mbp 取回来
            self.keyset(nsb, K2)
            self.assertEqual((self.sync(nsb), self.sync(ns)), ('', ''))
            self.assertEqual(self.stored(ns), (0, K2))
            ha = ns['gui_api_key'](ns['cfg_load'](), self.key, 'US-A', True)
            self.assertEqual((ha['from_device'], ha['hint']), ('bob-mini', ns['key_hint'](K2)))
            self.assertEqual(self.sync(ns), '', '再同步一次：没有新的，什么都不变')
            self.assertEqual(self.stored(ns), (0, K2))
            # carol-air（在设备清单里、没登录 alice）把旧的那一份传回去（重放），再传一份伪造的：都不认，传回新的
            cfgc = nsc['presence_load']()[1]
            for bad in (blob1, {'n': blob1['n'], 'c': base64.b64encode(secrets.token_bytes(256)).decode(), 't': blob1['t']}):
                self.assertEqual(nsc['presence_post'](cfgc, 'keys', {'put': {aid: bad}})[0], 200)
                self.assertEqual(self.sync(ns), '')
                self.assertEqual(self.stored(ns), (0, K2), '旧的、认证不过的不认')
                self.assertEqual(sw.keyshare_open(sw.derive(sw.KEYSHARE, self.salt, 'claude', 'alice@example.com', 'u-alice-0001'), aid, self.server_blob(aid))['key'], K2,
                                 '服务器上的旧了、坏了：传回这台设备的（自己修好）')
            # alice-mbp 删掉：bob-mini 上同步来的也删掉
            with mock.patch.dict(ns, {'keyshare_kick': lambda force=False: None, 'security': self.nokeychain}):
                ns['cmd_key_forget'](self.key)
            self.assertEqual(self.stored(ns)[0], 44)
            self.assertEqual(self.sync(ns), '')
            self.assertEqual(len(self.server_blob(aid)['c']), len(blob1['c']), '删除和 key 一样长')
            self.assertEqual(self.sync(nsb), '')
            self.assertEqual(self.stored(nsb)[0], 44, '删掉也同步')
            self.assertNotIn(acct, nsb['cfg_load']().get('ref_keys') or [])
            self.assertFalse(nsb['gui_api_key'](nsb['cfg_load'](), self.key, 'US-A', True)['saved'])
            # 记下 K3、两边都有了；bob-mini 删掉这个账号（cca rm）：只删它自己的，不同步，也不再取回来
            self.keyset(ns, K3)
            self.assertEqual((self.sync(ns), self.sync(nsb)), ('', ''))
            self.assertEqual(self.stored(nsb), (0, K3))
            (ccab / 'accounts.json').write_text('{}')
            with mock.patch.dict(nsb, {'security': self.nokeychain}):
                nsb['cmd_secret_del'](self.key)
            self.assertEqual((self.sync(nsb), self.sync(ns)), ('', ''))
            self.assertEqual((self.stored(nsb)[0], self.stored(ns)), (44, (0, K3)))
            # 账号加回来、还登录着：服务器上的取回来（钥匙串里的被删了也一样）
            (ccab / 'accounts.json').write_text(json.dumps({self.key: 'US-A'}))
            self.assertEqual(self.sync(nsb), '')
            self.assertEqual(self.stored(nsb), (0, K3))
            # 退出登录了：不问也不传（解不开）；本机的 key 还在
            self.login(nsb, None)
            self.keyset(nsb, K1)
            self.assertEqual(self.sync(nsb), '')
            self.assertEqual(self.stored(ns), (0, K3))
            self.assertEqual(self.sync(ns), '')
            self.assertEqual(self.stored(ns), (0, K3), '没登录时记下的：登录以前不传')
            self.login(nsb, 'u-alice-0001')
            self.assertEqual((self.sync(nsb), self.sync(ns)), ('', ''))
            self.assertEqual(self.stored(ns), (0, K1), '登录以后：按记下的时间比，新的胜出')

    def test_keyshare_before_presence(self):
        """没开设备统计时记下的（不记 keys.json）、旧版本记下的（key_hints 里没有时间）：开了以后按记下的时间算"""
        ns, K1, K2 = self.ns, self.K1, self.K2
        nsb, ccab, pubb = self.device('bob-mini', 'u-alice-0002')
        self.machines([{'key': self.pub, 'name': 'alice-mbp'}, {'key': pubb, 'name': 'bob-mini'}])
        self.login(ns, 'u-alice-0002')
        aid = ns['presence_aid'](self.salt, ns['CLAUDE'], 'alice@example.com')
        with mock.patch.dict(os.environ, {'CCA_KEYSTORE': 'file'}):
            (self.cca / 'config.json').write_text('{"presence": false}')
            self.assertEqual(self.keyset(ns, K1), ('', []), '关掉了设备统计：不说会同步，不起同步')
            self.assertFalse((self.cca / 'presence').exists(), '也不建 presence 目录')
            self.assertIsNone(self.sync(ns))
            c = json.loads((self.cca / 'config.json').read_text())
            c.pop('presence')
            (self.cca / 'config.json').write_text(json.dumps(c))
            # bob-mini 的是旧版本记下的：key_hints 里没有时间，按密钥文件的修改时间算（比 alice-mbp 记下的早）
            nsb['kc_put'](nsb['acct_id'](self.key), 'reference', K2, True)
            c = json.loads((ccab / 'config.json').read_text())
            c['key_hints'][nsb['acct_id'](self.key)].pop('at', None)
            (ccab / 'config.json').write_text(json.dumps(c))
            old = time.time() - 86400
            os.utime(ccab / 'secrets' / nsb['kc_svc'](nsb['acct_id'](self.key), True), (old, old))
            self.assertEqual((self.sync(nsb), self.sync(ns)), ('', ''))
            self.assertEqual(self.stored(ns), (0, K1), 'alice-mbp 记下的新：不动')
            self.assertEqual(self.sync(nsb), '')
            self.assertEqual(self.stored(nsb), (0, K1), 'bob-mini 的旧：换成新的')
            self.assertEqual(sw.keyshare_open(sw.derive(sw.KEYSHARE, self.salt, 'claude', 'alice@example.com', 'u-alice-0002'), aid, self.server_blob(aid))['key'], K1)

    def test_keyshare_bad_files(self):
        """账号的 .claude.json 读不出（半写、手改坏了）：当作没登录，这一轮不同步它，不退出；上报进程里同步出的错（包括 die 的
        SystemExit）不停掉上报；其他设备上删掉的、这台设备的钥匙串里没删掉（锁着）：不记成删掉了，下一轮再删"""
        ns, K1 = self.ns, self.K1
        nsb, ccab, pubb = self.device('bob-mini', 'u-alice-0005')
        self.machines([{'key': self.pub, 'name': 'alice-mbp'}, {'key': pubb, 'name': 'bob-mini'}])
        self.login(ns, 'u-alice-0005')
        cj = Path(ns['cdir'](self.key)) / '.claude.json'
        with mock.patch.dict(os.environ, {'CCA_KEYSTORE': 'file'}):
            self.keyset(ns, K1)
            good = cj.read_text()
            srv = lambda: (self.d / 'keys.json').read_text() if (self.d / 'keys.json').exists() else None
            before = srv()
            cj.write_text('{"oauthAccount": ')
            self.assertEqual(self.sync(ns), '', '读不出登录身份：当作没登录，不退出（以前 die：上报进程跟着退出）')
            self.assertEqual(srv(), before, '没传')
            cj.write_text(good)
            self.assertEqual((self.sync(ns), self.sync(nsb)), ('', ''))
            self.assertEqual(self.stored(nsb), (0, K1))
            with mock.patch.dict(ns, {'keyshare_kick': lambda force=False: None, 'security': self.nokeychain}):
                ns['cmd_key_forget'](self.key)
            self.assertEqual(self.sync(ns), '')
            raw = json.loads((ccab / 'presence' / 'keys.json').read_text())['acc']
            with mock.patch.dict(nsb, {'kc_del': lambda aid, ref=False: nsb['kc_unbook'](aid, ref)}):   # 钥匙串锁着：删不掉
                err = self.sync(nsb)
            self.assertIn('没能从这台设备的密钥文件里删掉', err)
            self.assertEqual(self.stored(nsb), (0, K1))
            self.assertEqual(json.loads((ccab / 'presence' / 'keys.json').read_text())['acc'], raw, '没删掉：不记成删掉了')
            self.assertEqual(self.sync(nsb), '', '下一轮再删')
            self.assertEqual(self.stored(nsb)[0], 44)
        # 上报进程：同步 API key 时 die（SystemExit）也接着报
        clock = {'t': 1000.0}

        class FakeTime:
            def monotonic(self):
                return clock['t']

            def time(self):
                return 1794042000.0 + clock['t']

            def __getattr__(self, name):
                return getattr(time, name)

        class Done(Exception):
            pass
        posts, waits = [], []

        def boom(force=False):
            raise SystemExit(1)

        def wait(pids, dirs, timeout):
            waits.append(timeout)
            if len(waits) >= 3:
                raise Done()
            clock['t'] += 700.0   # 每圈都过了 KEYSHARE_EVERY：每圈都同步、都出错
        unit = {'a' * 32: {'cli': [[1, 0]], 'app': None}}
        with mock.patch.dict(ns, {'time': FakeTime(), 'presence_post': lambda cfg, op, payload, timeout=8: posts.append(op) or (200, {}),
                                  'presence_wait': wait, 'presence_stamp': lambda: 1,
                                  'presence_targets': lambda: [(ns['CLAUDE'], self.key, '/nonexistent')], 'presence_procs': lambda: {},
                                  'presence_scan': lambda *a: (dict(unit), set()), 'keyshare_sync': boom}):
            with self.assertRaises(Done):
                ns['cmd_presence_agent']()
        self.assertEqual(posts, ['report'] * 3, '每圈都照常上报')

    def test_keyshare_force_waits(self):
        """记下、删掉之后的同步（force）碰上正在跑的一轮：等它跑完再同步，不放弃（不然这次的改动要等到下一轮才传出去）"""
        ns = self.ns
        self.login(ns, 'u-alice-0004')
        calls = []

        def post(cfg, op, payload, timeout=8):
            calls.append(sorted(payload))
            return 200, {'keys': {}}
        with mock.patch.dict(os.environ, {'CCA_KEYSTORE': 'file'}), mock.patch.dict(ns, {'presence_post': post}):
            self.keyset(ns, self.K1)
            held = open(self.cca / 'presence' / 'keys-sync.lock', 'a')
            fcntl.flock(held, fcntl.LOCK_EX)
            release = threading.Timer(0.6, held.close)
            release.start()
            t0 = time.monotonic()
            self.assertEqual(ns['keyshare_sync'](True, 1), '')
            waited = time.monotonic() - t0
            release.join()
            self.assertGreaterEqual(waited, 0.5, '等正在跑的那一轮放锁')
            self.assertEqual(calls, [['get'], ['put']], '放锁以后照常同步：问一次，传上去')
            held = open(self.cca / 'presence' / 'keys-sync.lock', 'a')
            fcntl.flock(held, fcntl.LOCK_EX)
            try:
                t0 = time.monotonic()
                self.assertIsNone(ns['keyshare_sync'](False), '不是 force 的：有一轮在跑就走')
                self.assertLess(time.monotonic() - t0, 0.5)
            finally:
                held.close()

    def test_keyshare_old_server(self):
        """旧的服务器（还不收 key：404）：记下原因（维护者要更新服务器），6 小时后再试；cca devices、App 说一句"""
        ns = self.ns
        self.login(ns, 'u-alice-0003')
        calls = []

        def post(cfg, op, payload, timeout=8):
            calls.append(op)
            return 404, {'error': 'not_found'}
        with mock.patch.dict(os.environ, {'CCA_KEYSTORE': 'file'}), mock.patch.dict(ns, {'presence_post': post}):
            self.keyset(ns, self.K1)
            err = self.sync(ns)
            self.assertIn('./policy.sh presence deploy', err)
            j = json.loads((self.cca / 'presence' / 'keys.json').read_text())
            self.assertTrue(j['retry'] - time.time() > 5 * 3600 and j['error'] == err, j)
            self.assertIsNone(ns['keyshare_sync'](), '不 force：6 小时内不再试')
            self.assertEqual(calls, ['keys'])
            self.assertEqual(ns['keyshare_error'](), err)
            self.assertEqual(ns['gui_api_key'](ns['cfg_load'](), self.key, 'US-A', True, err)['sync_error'], err)
            self.assertIsNone(ns['gui_api_key'](ns['cfg_load'](), self.key, 'US-A', False, err)['sync_error'], '不同步的账号不说')


    # 折合金额：各台设备算自己的，经服务器交换密文
    def cost_local(self, cca, ident, rows, who='alice@example.com'):
        """这台设备统计好的（cost/local.json）：身份 ident 的小时数据"""
        (cca / 'cost').mkdir(exist_ok=True)
        (cca / 'cost' / 'local.json').write_text(json.dumps({'v': 1, 'build': 'x', 'who': {'claude:' + ident: who},
                                                             'acc': {'claude:' + ident: rows}, 'um': {}, 'orphan': {}}))

    def slots(self, aid):
        return json.loads((self.d / 'cost-slots.json').read_text())['acc'].get(aid, {})

    def test_cost_sync(self):
        """alice-mbp、bob-mini 登录着 alice，carol-air 没有：两台互相取到对方的，服务器上只有密文；伪造的、重放的不认；
        没变的不重传、不重取；撤销的设备留着、标上；用过却没传过的设备说一声"""
        ns = self.ns
        nsb, ccab, pubb = self.device('bob-mini', 'u-alice-0005')
        nsc, ccac, pubc = self.device('carol-air', None)
        self.machines([{'key': self.pub, 'name': 'alice-mbp'}, {'key': pubb, 'name': 'bob-mini'}, {'key': pubc, 'name': 'carol-air'}])
        self.login(ns, 'u-alice-0005')
        aid = ns['presence_aid'](self.salt, ns['CLAUDE'], 'alice@example.com')
        hour = int(time.time() // 3600 * 3600) - 3600
        self.cost_local(self.cca, 'u-alice-0005', {str(hour): [5000000, 2, 100, 10, 0, 0, 0]})
        self.cost_local(ccab, 'u-alice-0005', {str(hour - 86400): [3000000, 1, 50, 5, 0, 0, 0]})
        self.cost_local(ccac, 'u-carol-0001', {str(hour): [1000000, 1, 1, 1, 0, 0, 0]}, 'carol@example.com')
        r = ns['cost_sync'](True)
        self.assertEqual(r['state'], 'ok', r)
        raw = (self.d / 'cost-slots.json').read_text()
        for secret in ('alice@example.com', 'u-alice-0005', '5000000', str(hour)):
            self.assertNotIn(secret, raw, '服务器上没有邮箱、身份、金额、时间')
        fa = ns['presence_load']()[1]['fp']
        self.assertEqual(list(self.slots(aid)), [fa])
        self.assertEqual(nsb['cost_sync'](True)['state'], 'ok')
        rb = json.loads((ccab / 'cost' / 'remote.json').read_text())['acc']['claude:alice@example.com']
        self.assertEqual([(e['name'], e['h'], e['revoked']) for e in rb.values()], [('alice-mbp', {str(hour): [5000000, 2, 100, 10, 0, 0, 0]}, False)],
                         'bob-mini 取到了 alice-mbp 的')
        self.assertEqual(nsc['cost_sync'](True)['state'], 'skipped', 'carol-air 没登录 alice（解不开）：不问也不传')
        self.assertEqual(sorted(self.slots(aid)), sorted([fa, nsb['presence_load']()[1]['fp']]))
        at_a = self.slots(aid)[fa]['at']
        posts = []
        real = ns['presence_post']

        def spy(cfg, op, payload, timeout=8):
            posts.append(payload)
            return real(cfg, op, payload, timeout)
        with mock.patch.dict(ns, {'presence_post': spy}):
            self.assertEqual(ns['cost_sync'](True)['state'], 'ok')
            self.assertEqual(len(posts), 1, '这台设备的没变：只问，不传')
            self.assertEqual(self.slots(aid)[fa]['at'], at_a)
            ra = json.loads((self.cca / 'cost' / 'remote.json').read_text())['acc']['claude:alice@example.com']
            self.assertEqual([e['name'] for e in ra.values()], ['bob-mini'])
            posts.clear()
            self.assertEqual(ns['cost_sync'](True)['state'], 'ok')
            self.assertEqual(list(posts[0]['have'][aid].values()).count(None), 0)
            self.assertEqual(len(posts[0]['have'][aid]), 2, '带着已经有的认证码（自己的、bob-mini 的）：没变的服务器不回密文')
        cv = ns['CostView']({}, time.time(), ns['presence_load']()[1])
        a = cv.account(ns['CLAUDE'], self.key, None, True, True)
        self.assertEqual((a['summary']['days_usd'], [d['name'] for d in a['devices']], a['sync']['state']), (8.0, ['alice-mbp', 'bob-mini'], 'ok'),
                         'App 看到的：两台设备加起来')
        # carol-air 在设备清单里，在自己的那一份里放伪造的、重放 alice-mbp 的：认证里带着设备指纹，都不认
        cfgc = nsc['presence_load']()[1]
        fake = ns['cost_seal'](ns['keyshare_keys'](self.salt, ns['CLAUDE'], 'alice@example.com', 'u-wrong', ns['COST_LABEL']), aid, cfgc['fp'],
                               {'v': 1, 'at': time.time(), 'h': {str(hour): [10 ** 9, 1, 0, 0, 0, 0, 0]}, 'um': []})
        for bad in (fake, self.slots(aid)[fa]['blob']):
            self.assertEqual(nsc['presence_post'](cfgc, 'ledger', {'put': {aid: bad}})[0], 200)
            self.assertEqual(ns['cost_sync'](True)['state'], 'ok')
            ra = json.loads((self.cca / 'cost' / 'remote.json').read_text())['acc']['claude:alice@example.com']
            self.assertEqual([e['name'] for e in ra.values()], ['bob-mini'], '伪造的、重放的都不认')
        # 撤销 bob-mini：它传的留着、标上；dan（用过 alice、没传过）
        nsd, ccad, pubd = self.device('dan-air', 'u-alice-0005')
        self.machines([{'key': self.pub, 'name': 'alice-mbp'}, {'key': pubc, 'name': 'carol-air'}, {'key': pubd, 'name': 'dan-air'}])
        cfgd = nsd['presence_load']()[1]
        self.assertEqual(nsd['presence_post'](cfgd, 'report', {'units': {aid: {'cli': [[60, 0]], 'app': None}}, 'end': None})[0], 200)
        self.assertEqual(ns['cost_sync'](True)['state'], 'ok')
        ra = json.loads((self.cca / 'cost' / 'remote.json').read_text())['acc']['claude:alice@example.com']
        self.assertEqual([(e['name'], e['revoked']) for e in ra.values()], [('bob-mini', True)])
        sync = json.loads((self.cca / 'cost' / 'sync.json').read_text())
        self.assertEqual([m['name'] for m in sync['acc']['claude:alice@example.com']['missing']], ['dan-air'])
        a = ns['CostView']({}, time.time(), ns['presence_load']()[1]).account(ns['CLAUDE'], self.key, None, True, True)
        self.assertEqual(([(d['name'], d['revoked']) for d in a['devices']], [m['name'] for m in a['sync']['missing']]),
                         ([('alice-mbp', False), ('bob-mini', True)], ['dan-air']))
        for f in ('remote.json', 'sync.json'):
            self.assertEqual(oct(os.stat(self.cca / 'cost' / f).st_mode & 0o777), '0o600', f)

    def test_cost_old_server(self):
        """旧的服务器（没有 /v1/ledger：404，body 太大时先回 413）：记下原因，6 小时后再试；App 说要维护者更新服务器"""
        ns = self.ns
        self.login(ns, 'u-alice-0006')
        (self.cca / 'cost').mkdir()
        calls = []
        for code in (404, 413):
            calls.clear()

            def post(cfg, op, payload, timeout=8):
                calls.append(op)
                return code, {'error': 'not_found'}
            (self.cca / 'cost' / 'sync.json').unlink() if (self.cca / 'cost' / 'sync.json').exists() else None
            with mock.patch.dict(ns, {'presence_post': post}):
                r = ns['cost_sync'](True)
                self.assertEqual((r['state'], r['message']), ('old_server', ns['COST_OLD']), code)
                self.assertIn('./policy.sh presence deploy', r['message'])
                j = json.loads((self.cca / 'cost' / 'sync.json').read_text())
                self.assertTrue(j['retry'] - time.time() > 5 * 3600, j)
                self.assertEqual(ns['cost_sync']()['state'], 'waiting', '不 force：6 小时内不再试')
                self.assertEqual(calls, ['ledger'])
        calls.clear()
        (self.cca / 'cost' / 'sync.json').write_text(json.dumps({'v': 1, 'at': time.time() + 3 * 86400, 'retry': time.time() + 30 * 86400,
                                                                 'state': 'old_server', 'error': ns['COST_OLD']}))
        with mock.patch.dict(ns, {'presence_post': post}):
            ns['cost_sync']()
        self.assertEqual(calls, ['ledger'], '记下的时间在未来（这台设备的时钟快过、又调回来了）：不当真，照常同步')
        cv = ns['CostView']({}, time.time(), ns['presence_load']()[1])
        self.assertEqual(cv.sync_view('claude:alice@example.com', True, True)['state'], 'old_server')


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
