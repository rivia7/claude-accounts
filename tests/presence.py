#!/usr/bin/env python3
"""连接设备（presence）：服务端的验签、设备清单、状态；客户端的扫描、闲置判定、上报内容。
真的 ssh-keygen、真的 TLS 服务器（127.0.0.1，随机端口），不联网。"""
import base64
import contextlib
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
        units, pids = scan({4242: (1, os.path.basename(tty), 1.0, 1000, 'bash cca')})
        aid = self.ns['presence_aid'](self.salt, self.ns['CLAUDE'], 'alice@example.com')
        self.assertEqual(units, {aid: {'cli': [[300, 120]], 'app': None}}, '闲置 = 现在 - 最后一次输入 / 输出')
        self.assertEqual(pids, {4242}, '盯着这个会话的 cca 进程退出')
        self.assertEqual(scan({4242: (1, os.path.basename(tty), 1.0, 1003, 'bash cca')})[0], {}, '进程号被重用（启动时间对不上）：不算')
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
                  presence_scan=lambda *a: (dict(unit), set()))
        with self.assertRaises(Done):
            ns['cmd_presence_agent']()
        gaps = [round(b - a) for a, b in zip(posts, posts[1:])]
        self.assertGreaterEqual(gaps[0], 54, '报成了：下一次是心跳（60 秒左右）')
        self.assertEqual(gaps[1:3], [10, 20], '报不出去：按退避再试，不是每圈都重发')
        self.assertEqual(gaps[3], 600, '服务器不认这台 Mac（403）：10 分钟后再试')
        self.assertTrue(all(g >= 10 for g in gaps[1:]) and all(w >= 0 for w in waits), (gaps, waits))


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
