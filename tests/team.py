#!/usr/bin/env python3
"""Team（cca team）：辅助程序（cca-core）这一头的建 Team、接上服务器、邀请、加入、同步配置、授权、移除、退出，对着真的服务器
（server/cca-presence.py，127.0.0.1、随机端口）。各台设备各一个临时 HOME（tests/presence.py 的 Device）。
另一头有时由 tests/spec_wire.py（照 server/README.md 独立实现的那一份）扮演：两份实现封的、签的要能互相解开、验过。不联网。"""
import hashlib
import json
import os
from pathlib import Path
import secrets
import stat
import subprocess
import sys
import tempfile
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from presence import (SESSION_PID, T0, Device, Server, Sessions, ident, keygen, make_ca_cert, make_cert, openssl_pin,   # noqa: E402
                      wait_for)
import spec_wire as sw   # noqa: E402

NODE = lambda name, **more: dict({'name': name, 'type': 'socks5', 'server': '192.0.2.10', 'port': 1080 + ord(name[-1])}, **more)
# 一份现成的 Team 配置（管理员那台 admin.json 里的 config，见 TeamTest.load）：Claude 的 alice → US-A（别名 alice）、bob → US-B（经 US-C
# 连出去），Codex 的 carol → US-A；各台设备经内置代理安装客户端用 US-D；节点的定义 US-A…US-E
CONFIG = {'accounts': {'claude': {'alice@example.com': 'US-A', 'bob@example.com': 'US-B'}, 'codex': {'carol@example.com': 'US-A'}, 'agy': {}},
          'tool_aliases': {'claude': {'alice@example.com': 'alice'}}, 'aliases': {}, 'install_node': 'US-D',
          'nodes': {'group': 'OpenAi', 'proxies': [NODE('US-A'), NODE('US-B', **{'dialer-proxy': 'US-C'}), NODE('US-C'), NODE('US-D'), NODE('US-E')]}}

# 不是 Team 的同步服务器的一个 HTTPS 服务（自己一个进程）：什么请求都回 404（像 3.6.0 以前维护者的服务器、别的网站）。参数：证书、私钥、
# 端口写到哪个文件
NOT_TEAM = r'''
import http.server, os, ssl, sys
cert, key, port_file = sys.argv[1:4]

class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        self.rfile.read(int(self.headers.get('Content-Length') or 0))
        body = b'{"error":"not_found"}'
        self.send_response(404)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    do_GET = do_POST

    def log_message(self, *a):
        pass

srv = http.server.HTTPServer(('127.0.0.1', 0), H)
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain(cert, key)
srv.socket = ctx.wrap_socket(srv.socket, server_side=True)
with open(port_file + '.tmp', 'w') as f:
    f.write(str(srv.server_address[1]))
os.replace(port_file + '.tmp', port_file)
srv.serve_forever()
'''


class TeamTest(Sessions, unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='cca-team-cli-')
        self.addCleanup(self.tmp.cleanup)
        self.d = Path(self.tmp.name)
        self.n = 0

    def device(self, name):
        return Device(self.d / name, name, {})

    def server(self, root, port=0, name='srv'):
        """起一台 Team 的服务器（自己的目录、自己的证书）：(服务器, 地址, 证书公钥的指纹)"""
        d = self.d / name
        if not d.exists():
            d.mkdir()
            make_cert(d)
        srv = Server(d, team_root=root, port=port)
        self.addCleanup(srv.stop)
        return srv, 'https://127.0.0.1:%d' % srv.port, openssl_pin(d / 'cert.pem')

    def lines(self, dev, *argv, **kw):
        return dev.ok(*argv, **kw).split('\n')[:-1]

    def code(self, dev, *argv, **kw):
        """没成的命令的原因码（stdout 是「原因码\\t原因」，退出码 1）"""
        r = dev.run(*argv, **kw)
        self.assertEqual((r.returncode, r.stderr), (1, ''), r.stdout)
        return r.stdout.split('\t')[0]

    def tdir(self, dev, tid):
        return dev.cca / 'teams' / tid

    def doc(self, dev, tid):
        """这台设备手里的配置：(序号, 内容)"""
        j = json.loads((self.tdir(dev, tid) / 'doc.json').read_text())
        return j['seq'], j['doc']

    def new_team(self, name='测试组', device='studio'):
        """一台管理员的设备、建好的 Team、照它的根公钥起的服务器、接上：(管理员, 编号, 根公钥, 服务器, 地址, 指纹)"""
        admin = self.device('admin')
        tid, root, _ = self.lines(admin, 'team-new', name, '--device', device)
        srv, url, pin = self.server(root)
        self.assertEqual(self.lines(admin, 'team-attach', tid, url, pin), ['127.0.0.1:%d' % srv.port, device, '1', '1', ''])
        return admin, tid, root, srv, url, pin

    def load(self, admin, tid):
        """给这个 Team 一份现成的配置（CONFIG）：整份写进管理员这台的 admin.json（3.6.0 以前是 cca team import 导入维护者映射仓库里的
        policy.json、nodes.json），再经 cca team map 编别名（没写的编成 team-N）、发布（还没接上服务器的只改本机的）。返回存上了几台的配置"""
        p = self.tdir(admin, tid) / 'admin.json'
        a = json.loads(p.read_text())
        a['config'] = json.loads(json.dumps(CONFIG))
        p.write_text(json.dumps(a))
        return self.lines(admin, 'team-map', tid, 'claude', 'alice@example.com', 'US-A')

    def not_team(self):
        """起一个不是 Team 的同步服务器的 HTTPS 服务（NOT_TEAM）：(地址, 证书公钥的指纹)"""
        d = self.d / 'not-team'
        d.mkdir()
        make_cert(d)
        p = subprocess.Popen([sys.executable, '-I', '-c', NOT_TEAM, str(d / 'cert.pem'), str(d / 'key.pem'), str(d / 'port')],
                             stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.addCleanup(lambda: (p.kill(), p.wait()))
        port = int(wait_for('那个服务起来', lambda: (d / 'port').exists() and (d / 'port').read_text()))
        return 'https://127.0.0.1:%d' % port, openssl_pin(d / 'cert.pem')

    def invite(self, admin, tid, label, *more):
        link, dev, problem = self.lines(admin, 'team-invite', tid, label, *more)
        self.assertEqual(problem, '')
        return link, dev

    def test_admin_local(self):
        """建 Team：只在本机（还没有服务器）；文件都只给自己读写；接不上的各种原因"""
        admin = self.device('admin')
        self.assertEqual(self.code(admin, 'team-new', ''), 'name')
        self.assertEqual(self.lines(admin, 'team-list'), [])
        tid, root, n = self.lines(admin, 'team-new', '测试组', '--device', 'studio')
        self.assertEqual(n, '0', '没写 --accounts：一个账号都不放')
        self.assertEqual((tid, len(tid)), (sw.team_id(root), 32))
        t = self.tdir(admin, tid)
        self.assertEqual(sorted(p.name for p in t.iterdir()), ['admin.json', 'id_ed25519', 'id_ed25519.pub', 'root_ed25519', 'root_ed25519.pub',
                                                               'secret', 'team.json'])
        self.assertEqual([oct(stat.S_IMODE(os.stat(p).st_mode)) for p in (t, t / 'admin.json', t / 'root_ed25519', t / 'secret', t / 'team.json')],
                         ['0o700', '0o600', '0o600', '0o600', '0o600'])
        self.assertEqual(json.loads((admin.cca / 'teams' / 'index.json').read_text()), {'v': 1, 'order': [tid]})
        self.assertFalse(list((admin.cca / 'teams').glob('.new.*')))
        rows = json.loads(admin.ok('team-list', '--json'))
        self.assertEqual((rows['kind'], [(r['id'], r['name'], r['role'], r['state'], r['host'], r['device']) for r in rows['teams']]),
                         ('team_list', [(tid, '测试组', 'admin', 'local', None, 'studio')]))
        self.assertEqual(self.lines(admin, 'team-list'), ['%s\t测试组\tadmin\t-\tlocal\t-\t-\t-' % tid], '没有的栏写 -')
        # 建到一半被打断（目录有了、还没记进 index.json）的也认得出来
        (admin.cca / 'teams' / 'index.json').unlink()
        self.assertEqual([r['id'] for r in json.loads(admin.ok('team-list', '--json'))['teams']], [tid])
        # team.json 坏了、管理员的密钥还在：拿这个 Team 的邀请来加入不能当成没有（不然根密钥就删掉了）
        good = (t / 'team.json').read_text()
        (t / 'team.json').write_text('{')
        link = sw.invite_encode('https://127.0.0.1:9', [], root, secrets.token_hex(16), '测试组')
        self.assertEqual(self.code(admin, 'team-join', stdin=link), 'joined')
        self.assertTrue((t / 'root_ed25519').exists() and (t / 'admin.json').exists())
        (t / 'team.json').write_text(good)
        # 每台设备那串随机数不存：照主密钥算。管理员自己的那台是第 0 台
        a = json.loads((t / 'admin.json').read_text())
        k0 = (t / 'secret').read_text().strip()
        dev0 = sw.invite_parts(k0)[1]
        self.assertEqual((list(a['devices']), a['devices'][dev0]['state'], json.loads((t / 'team.json').read_text())['dev']), ([dev0], 'local', dev0))
        self.assertNotIn(k0, (t / 'admin.json').read_text())
        import hmac
        self.assertEqual(k0, hmac.new(bytes.fromhex(a['master']), b'cca-team-k-v1\x000', hashlib.sha256).digest()[:16].hex())
        # 还没接上服务器：发不了邀请；改配置只改本机的
        self.assertEqual(self.code(admin, 'team-invite', tid, 'bob'), 'not_attached')
        self.assertEqual(self.load(admin, tid), ['0'])
        self.assertEqual(self.lines(admin, 'team-map', tid, 'claude', 'Dave@Example.com', 'US-E'), ['0'])
        self.assertEqual(json.loads((t / 'admin.json').read_text())['config']['accounts']['claude'],
                         {'alice@example.com': 'US-A', 'bob@example.com': 'US-B', 'dave@example.com': 'US-E'})
        # 接不上：地址、指纹写得不对；不是这个 Team 的服务器；不是 Team 的服务器
        self.assertEqual(self.code(admin, 'team-attach', tid, 'http://127.0.0.1:1'), 'url')
        self.assertEqual(self.code(admin, 'team-attach', tid, 'https://127.0.0.1:1', 'sha256//x'), 'pin')
        self.assertEqual(self.code(admin, 'team-attach', tid, 'https://127.0.0.1:9', 'sha256//' + 'A' * 43 + '='), 'unreachable')
        other = keygen(self.d / 'other-root')
        srv, url, pin = self.server(other, name='other')
        self.assertEqual(self.code(admin, 'team-attach', tid, url, pin), 'denied')
        self.assertEqual(self.code(admin, 'team-attach', tid, url, 'sha256//' + 'A' * 43 + '='), 'pin')
        self.assertEqual(self.code(admin, 'team-attach', tid, *self.not_team()), 'old')
        self.assertEqual(json.loads((t / 'team.json').read_text())['url'], '', '没接上：本机照旧')

    def test_lifecycle(self):
        admin, tid, root, srv, url, pin = self.new_team()
        t = self.tdir(admin, tid)
        self.assertEqual(self.doc(admin, tid)[0], 1, '接上以后管理员自己这台也取到了配置')
        self.assertEqual(self.load(admin, tid), ['1'])
        self.assertEqual(self.lines(admin, 'team-sync'), ['%s\t测试组\tok\t2\t' % tid])
        listing = json.loads(admin.ok('team-devices', tid, '--json'))
        self.assertEqual((listing['name'], sorted(listing['accounts']['claude']), listing['aliases'], listing['tool_aliases'], sorted(listing['nodes'])),
                         ('测试组', ['alice@example.com', 'bob@example.com'], {'alice@example.com': 'alice', 'bob@example.com': 'team-1', 'carol@example.com': 'team-1'},
                          {'claude': {'alice@example.com': 'alice', 'bob@example.com': 'team-1'}, 'codex': {'carol@example.com': 'team-1'}},
                          ['US-A', 'US-B', 'US-C', 'US-D', 'US-E']),
                         '管理页要的：Team 的名字、配置里的账号 → 节点、别名（按工具的和按邮箱的）、带着定义的节点')
        seq, doc = self.doc(admin, tid)
        self.assertEqual((doc['accounts'], doc['tool_aliases'], doc['aliases'], doc['install_node'], doc['name'], doc['server']),
                         ({'claude': {'alice@example.com': 'US-A', 'bob@example.com': 'US-B'}, 'codex': {'carol@example.com': 'US-A'}},
                          {'claude': {'alice@example.com': 'alice', 'bob@example.com': 'team-1'}, 'codex': {'carol@example.com': 'team-1'}},
                          {'alice@example.com': 'alice', 'bob@example.com': 'team-1', 'carol@example.com': 'team-1'}, 'US-D', '测试组', {'u': url, 'p': [pin]}),
                         '配置里没写别名的编成 team-N：各工具各编各的（Claude 的 bob、Codex 的 carol 都是 team-1）')
        self.assertEqual([p['name'] for p in doc['nodes']['proxies']], ['US-A', 'US-B', 'US-C', 'US-D'], '用得着的节点（连同 dialer-proxy 连着的）')
        # 邀请：链接只在 stdout 里出现一次；那串随机数、令牌不落盘、不进命令行
        self.assertEqual(self.code(admin, 'team-invite', tid, 'bob', '--accounts', 'nope'), 'usage')
        self.assertEqual(self.code(admin, 'team-invite', tid, 'bob', '--ttl', '5'), 'usage')
        link, dev_b = self.invite(admin, tid, 'bob-mini', '--accounts', 'claude:Alice@example.com')
        inv = sw.invite_decode(link)
        token, dev, keys = sw.invite_parts(inv['k'])
        self.assertEqual((inv['u'], inv['p'], 'ssh-ed25519 ' + inv['r'], inv['n'], inv['d'], dev, 's' in inv),
                         (url, [pin], root, '测试组', 'bob-mini', dev_b, False), '邀请里带着管理员给这台设备起的名字')
        self.assertLess(abs(inv['e'] - (time.time() + 7 * 86400)), 120, '默认 7 天过期')
        everything = ''.join(p.read_text(errors='replace') for p in admin.home.rglob('*') if p.is_file() and p.name != 'kicks.log')
        self.assertTrue(inv['k'] not in everything and token not in everything, '那串随机数、令牌不在管理员这台的任何文件里（请求、curl 的命令行）')
        state = (self.d / 'srv' / 'state.json').read_text()
        self.assertTrue(inv['k'] not in state and token not in state and hashlib.sha256(token.encode()).hexdigest() in state)
        # 加入
        bob = self.device('bob')
        self.assertEqual(self.code(bob, 'team-join', stdin='nope'), 'link')
        self.assertEqual(self.code(bob, 'team-join', stdin=link[:-3]), 'link')
        self.assertEqual(self.code(bob, 'team-join', '--name', 'x' * 65, stdin=link), 'name')
        self.assertFalse((bob.cca / 'teams').exists() and list((bob.cca / 'teams').iterdir()), '没成：什么都不留')
        self.assertNotIn('device_id', json.loads((bob.cca / 'config.json').read_text()))
        got = self.lines(bob, 'team-join', stdin=link)
        self.assertEqual(got, ['测试组', '127.0.0.1:%d' % srv.port, 'bob-mini', '2', tid, 'ok', '1'], '没写 --name：用邀请里的名字，不用主机名')
        self.assertRegex(json.loads((bob.cca / 'config.json').read_text()).get('device_id', ''), r'^[0-9a-f]{16}$',
                         '加入了才生成这台机器的标识（没有路由器的机器靠它：折合金额的 dv）')
        self.assertRegex(json.loads((admin.cca / 'config.json').read_text()).get('device_id', ''), r'^[0-9a-f]{16}$', '新建 Team 的那台也有')
        argv = (bob.home / 'curl.argv').read_text()
        self.assertTrue(inv['k'] not in argv and token not in argv and link not in argv, '邀请不进 curl 的命令行')
        tb = self.tdir(bob, tid)
        self.assertEqual(sorted(p.name for p in tb.iterdir()), ['doc.json', 'id_ed25519', 'id_ed25519.pub', 'lock', 'secret', 'sync.json', 'team.json'])
        self.assertEqual([oct(stat.S_IMODE(os.stat(p).st_mode)) for p in (tb, tb / 'doc.json', tb / 'secret', tb / 'id_ed25519')],
                         ['0o700', '0o600', '0o600', '0o600'])
        seq, doc = self.doc(bob, tid)
        self.assertEqual((seq, doc['accounts'], doc['aliases'], [p['name'] for p in doc['nodes']['proxies']]),
                         (1, {'claude': {'alice@example.com': 'US-A'}}, {'alice@example.com': 'alice'}, ['US-A', 'US-D']),
                         '只有给了它的账号和这些账号要的节点（安装客户端用的节点也带着）')
        self.assertEqual(len(doc['salt']), 32)
        self.assertEqual(doc['salt'], self.doc(admin, tid)[1]['salt'], '盐是整个 Team 一份')
        rows = json.loads(bob.ok('team-list', '--json'))['teams']
        self.assertEqual([(r['id'], r['role'], r['state'], r['seq'], r['accounts'], r['device'], r['error']) for r in rows],
                         [(tid, 'member', 'ok', 1, 1, 'bob-mini', None)])
        self.assertEqual(self.code(bob, 'team-join', stdin=link), 'joined')
        eve = self.device('eve')
        self.assertEqual(self.code(eve, 'team-join', stdin=link), 'used')
        self.assertFalse(list((eve.cca / 'teams').glob('*/team.json')))
        # 管理员的那几条成员用不了
        for cmd in (['team-invite', tid, 'x'], ['team-devices', tid], ['team-map', tid, 'claude', 'x@example.com', 'US-A'], ['team-publish', tid]):
            self.assertEqual(self.code(bob, *cmd), 'not_admin', cmd)
        # 照文档独立实现的那一份也加入得了、解得开、验得过（辅助程序封的、签的）
        link2, dev_c = self.invite(admin, tid, 'spec', '--accounts', 'codex:carol@example.com,claude:bob@example.com')
        k2 = sw.invite_decode(link2)['k']
        token2, dev2, keys2 = sw.invite_parts(k2)
        pub = keygen(self.d / 'spec-key')
        st, j = sw.post(str(self.d / 'spec-key'), srv.port, 'team_join', {'pub': pub, 'name': 'spec', 'token': token2,
                                                                          'proof': sw.team_pub_proof(k2, pub)})
        self.assertEqual((st, j['dev']), (200, dev_c))
        st, j = sw.post(str(self.d / 'spec-key'), srv.port, 'team_get', {})
        env = j['doc']
        self.assertTrue(sw.team_doc_verify(root, tid, dev_c, env['seq'], env['ct'], env['sig']), '根密钥签的')
        spec_doc = sw.team_doc_open(keys2, tid, dev_c, env['seq'], env['ct'])
        self.assertEqual((spec_doc['accounts'], [p['name'] for p in spec_doc['nodes']['proxies']]),
                         ({'claude': {'bob@example.com': 'US-B'}, 'codex': {'carol@example.com': 'US-A'}}, ['US-A', 'US-B', 'US-C', 'US-D']))
        self.assertIsNone(sw.team_doc_open(keys, tid, dev_c, env['seq'], env['ct']), '别的设备的密钥解不开')
        # 设备的名单
        rows = {r['label']: r for r in json.loads(admin.ok('team-devices', tid, '--json'))['devices']}
        self.assertEqual({k: (r['state'], r['name'], r['me'], r['seq']) for k, r in rows.items()},
                         {'studio': ('joined', 'studio', True, 2), 'bob-mini': ('joined', 'bob-mini', False, 1), 'spec': ('joined', 'spec', False, 1)})
        self.assertEqual(self.lines(admin, 'team-list'), ['%s\t测试组\tadmin\t127.0.0.1:%d\tok\t2\t3\t-' % (tid, srv.port)])
        self.assertEqual((rows['bob-mini']['have'], rows['bob-mini']['grants'], rows['studio']['grants']), (None, {'claude': ['alice@example.com']}, 'all'))
        self.assertEqual([r['problem'] for r in rows.values()], [None, None, None])
        # 公钥要过认证码才记下（设备加入时拿邀请里的 k 算的：服务器不知道 k，换不了）：辅助程序加入的、独立实现带了认证码的都记下了
        saved = json.loads((t / 'admin.json').read_text())['devices']
        self.assertEqual((saved[dev_b]['pub'], saved[dev_c]['pub']), (' '.join((tb / 'id_ed25519.pub').read_text().split()[:2]), pub))
        # 没带认证码的（照旧文档自己实现的）：照样发布配置给它，公钥不记，名单里说一声
        link_n, dev_n = self.invite(admin, tid, 'no-proof', '--accounts', 'none')
        pub_n = keygen(self.d / 'noproof-key')
        self.assertEqual(sw.post(str(self.d / 'noproof-key'), srv.port, 'team_join',
                                 {'pub': pub_n, 'name': 'no-proof', 'token': sw.invite_parts(sw.invite_decode(link_n)['k'])[0]})[0], 200)
        row = next(r for r in json.loads(admin.ok('team-devices', tid, '--json'))['devices'] if r['label'] == 'no-proof')
        self.assertEqual(row['state'], 'joined')
        self.assertIn('认证码', row['problem'])
        self.assertNotIn('pub', json.loads((t / 'admin.json').read_text())['devices'][dev_n])
        listing = admin.ok('team-devices', tid).split('\n')
        self.assertEqual((listing[0], listing[4].split('\t')[2:]), ('0', ['joined', '0 个账号', '1', '-', row['problem']]), '第一行是别名锁定着没有')
        self.assertEqual(self.lines(admin, 'team-revoke', 'apply', tid, 'no-proof'), ['no-proof'])
        # 改授权、改映射：发布以后设备同步到新的一版
        self.assertEqual(self.lines(admin, 'team-grant', tid, 'bob-mini', '+claude:bob@example.com'), ['2 个账号'])
        self.assertEqual(self.lines(bob, 'team-sync', tid[:6]), ['%s\t测试组\tok\t2\t' % tid])
        seq, doc = self.doc(bob, tid)
        self.assertEqual((doc['accounts'], [p['name'] for p in doc['nodes']['proxies']]),
                         ({'claude': {'alice@example.com': 'US-A', 'bob@example.com': 'US-B'}}, ['US-A', 'US-B', 'US-C', 'US-D']))
        self.assertEqual(self.lines(admin, 'team-grant', tid, 'bob-mini', '-claude:alice@example.com'), ['1 个账号'])
        self.assertEqual(self.lines(admin, 'team-grant', tid, 'bob-mini', 'all', '-codex:carol@example.com'), ['2 个账号'], '从全部里去掉一个')
        self.assertEqual(self.code(admin, 'team-grant', tid, 'bob-mini', 'claude:x@example.com'), 'usage')
        self.assertEqual(self.code(admin, 'team-grant', tid, 'nobody', 'all'), 'no_device')
        self.assertEqual(self.lines(admin, 'team-map', tid, 'claude', 'alice@example.com', 'US-E'), ['2'], '管理员自己的和 bob 的变了，spec 的没变')
        self.assertEqual(self.lines(admin, 'team-map', tid, 'codex', 'carol@example.com', '-'), ['2'], '从 Team 里去掉一个账号')
        self.assertEqual(self.lines(bob, 'team-sync'), ['%s\t测试组\tok\t5\t' % tid])
        self.assertEqual(self.doc(bob, tid)[1]['accounts'], {'claude': {'alice@example.com': 'US-E', 'bob@example.com': 'US-B'}})
        rows = {r['label']: r for r in json.loads(admin.ok('team-devices', tid, '--json'))['devices']}
        self.assertEqual((rows['bob-mini']['seq'], rows['bob-mini']['have']), (5, 2), '它上次取的时候说手里是第 2 版')
        self.assertEqual(self.lines(admin, 'team-publish', tid), ['0', ''], '没变的不重新发布')
        # 移除：设备同步配置时知道被移除了；拿新的邀请再加入
        self.assertEqual(self.code(admin, 'team-revoke', 'plan', tid, 'studio'), 'self')
        self.assertEqual(self.lines(admin, 'team-revoke', 'plan', tid, 'bob-mini'), ['%s\t%s\tbob-mini\tjoined' % (tid, dev_b)])
        self.assertEqual(self.lines(admin, 'team-revoke', 'apply', tid, 'bob-mini'), ['bob-mini'])
        self.assertNotIn(dev_b, json.loads((t / 'admin.json').read_text())['devices'], '移除了的这边也不再记着')
        line = self.lines(bob, 'team-sync')[0].split('\t')
        self.assertEqual(line[:4], [tid, '测试组', 'removed', '5'])
        self.assertIn('移除', line[4])
        self.assertEqual(json.loads(bob.ok('team-list', '--json'))['teams'][0]['state'], 'removed')
        link3, dev_b3 = self.invite(admin, tid, 'bob-again')
        self.assertEqual(self.lines(bob, 'team-join', '--name', 'bob-mini', stdin=link3)[5:], ['ok', '2'], '被移除了的拿新的邀请直接加入（以前记着的不要了）')
        self.assertEqual(self.doc(bob, tid)[0], 1)
        rows = {r['label']: r['state'] for r in json.loads(admin.ok('team-devices', tid, '--json'))['devices']}
        self.assertEqual((rows['bob-again'], 'bob-mini' in rows, 'no-proof' in rows), ('joined', False, False),
                         '管理员和服务器对过一遍：知道它加入了；移除了的不列')
        # 退出：服务器上没有它了；管理员那边不再记它，不登记回去
        self.assertEqual(self.lines(bob, 'team-leave', 'plan'), ['%s\t测试组\tmember\t127.0.0.1:%d' % (tid, srv.port)])
        self.assertEqual(self.code(bob, 'team-leave', 'apply', tid[:8]), 'no_team', '真退出要写完整的编号')
        self.assertEqual(self.lines(bob, 'team-leave', 'apply', tid), ['测试组', '127.0.0.1:%d' % srv.port, ''])
        self.assertFalse(tb.exists())
        self.assertEqual(json.loads((bob.cca / 'teams' / 'index.json').read_text())['order'], [])
        self.assertEqual(self.code(bob, 'team-sync', tid), 'no_team')
        rows = {r['label']: r['state'] for r in json.loads(admin.ok('team-devices', tid, '--json'))['devices']}
        self.assertEqual(rows, {'studio': 'joined', 'spec': 'joined'}, '自己退出了的、移除了的都不列')
        self.assertEqual(len(json.loads((t / 'admin.json').read_text())['devices']), 2, 'admin.json 里也只剩这两台')
        # 3.6.5 及以前留下的记录（移除了的 revoked、自己退出了的 left）：对一遍时删掉
        a = json.loads((t / 'admin.json').read_text())
        old = {'%016x' % (0xdead0000 + i): {'n': 90 + i, 'label': 'gone-%d' % i, 'state': s, 'grants': 'all', 'seq': 1} for i, s in enumerate(('revoked', 'left'))}
        a['devices'].update(old)
        (t / 'admin.json').write_text(json.dumps(a))
        rows = {r['label']: r['state'] for r in json.loads(admin.ok('team-devices', tid, '--json'))['devices']}
        self.assertEqual(rows, {'studio': 'joined', 'spec': 'joined'})
        self.assertFalse(set(old) & set(json.loads((t / 'admin.json').read_text())['devices']))
        # 以前记成 left、又出现在服务器上的（3.6.5 及以前：对过以后它又登记回来了）：恢复成 joined
        a = json.loads((t / 'admin.json').read_text())
        spec_dev = next(dev for dev, d in a['devices'].items() if d['label'] == 'spec')
        a['devices'][spec_dev]['state'] = 'left'
        (t / 'admin.json').write_text(json.dumps(a))
        rows = {r['label']: r['state'] for r in json.loads(admin.ok('team-devices', tid, '--json'))['devices']}
        self.assertEqual(rows, {'studio': 'joined', 'spec': 'joined'}, '服务器上还有它：恢复成 joined')
        self.assertEqual(self.lines(admin, 'team-leave', 'plan', '测试组'),
                         ['%s\t测试组\tadmin\t127.0.0.1:%d\troot@127.0.0.1\t22\t/opt/cca-team-%s\t0' % (tid, srv.port, tid[:8])],
                         '管理员：后面是删除时去哪卸载它的同步服务器（手动接上的、没记下部署到哪：照部署的默认猜，最后一项是 0）')
        self.assertTrue(t.exists())
        self.assertEqual(self.lines(admin, 'team-leave', 'apply', tid)[:2], ['测试组', '127.0.0.1:%d' % srv.port])
        self.assertFalse(t.exists())

    def test_rename(self):
        """管理员给设备改名字：这边记着的、服务器上的一起改（同一把公钥再登记：加入的时间、认证码、配对认的指纹都不变），设备下次同步配置时
        跟着改；服务器上别的设备叫这个名字时服务器加 -2；还没加入的只改这边记着的；自己退出了的（对一遍时就不记了）改不了；
        还在的别的设备叫这个名字不行"""
        admin, tid, root, srv, url, pin = self.new_team()
        t = self.tdir(admin, tid)
        state = lambda: json.loads((self.d / 'srv' / 'state.json').read_text())['machines']
        link, dev_b = self.invite(admin, tid, 'bob-mini')
        bob = self.device('bob')
        self.lines(bob, 'team-join', '--name', 'bob-host', stdin=link)
        before = {f: m for f, m in state().items() if m['dev'] == dev_b}
        self.assertEqual([m['name'] for m in before.values()], ['bob-host'], '加入时写了 --name：先照它报')
        self.assertTrue(list(before.values())[0].get('proof'))
        device = lambda: json.loads(bob.ok('team-list', '--json'))['teams'][0]['device']
        # 和服务器对一遍时名字以管理员起的为准：服务器上照它再登记（还是那把公钥，加入的时间、认证码照旧），设备下次同步配置时跟着改
        rows = {r['dev']: r for r in json.loads(admin.ok('team-devices', tid, '--json'))['devices']}
        self.assertEqual((rows[dev_b]['label'], rows[dev_b]['name'], rows[dev_b]['problem']), ('bob-mini', 'bob-mini', None))
        aligned = {f: m for f, m in state().items() if m['dev'] == dev_b}
        self.assertEqual(list(aligned), list(before), '同一把公钥：远程会话的配对按它认，照旧')
        (f, m0), m1 = list(before.items())[0], list(aligned.values())[0]
        self.assertEqual((m1['name'], m1['joined'], m1.get('proof')), ('bob-mini', m0['joined'], m0['proof']), f)
        self.assertEqual(device(), 'bob-host')
        self.assertEqual(self.lines(bob, 'team-sync'), ['%s\t测试组\tok\t1\t' % tid])
        self.assertEqual(device(), 'bob-mini')
        # 改：这边记着的、服务器上的都是新名字；服务器上还是那把公钥，加入的时间、认证码照旧
        self.assertEqual(self.lines(admin, 'team-rename', tid, 'bob-mini', '小王的 MacBook'), ['bob-mini', '小王的 MacBook', ''])
        d = json.loads((t / 'admin.json').read_text())['devices'][dev_b]
        self.assertEqual((d['label'], d['name'], d['state']), ('小王的 MacBook', '小王的 MacBook', 'joined'))
        after = {f: m for f, m in state().items() if m['dev'] == dev_b}
        self.assertEqual(list(after), list(before))
        m2 = list(after.values())[0]
        self.assertEqual((m2['name'], m2['joined'], m2.get('proof')), ('小王的 MacBook', m0['joined'], m0['proof']), f)
        # 设备下次同步配置时跟着改（配置本身没变：序号照旧）
        self.assertEqual(device(), 'bob-mini')
        self.assertEqual(self.lines(bob, 'team-sync'), ['%s\t测试组\tok\t1\t' % tid])
        self.assertEqual(device(), '小王的 MacBook')
        self.assertIsNone(next(r for r in json.loads(admin.ok('team-devices', tid, '--json'))['devices'] if r['dev'] == dev_b)['problem'],
                          '改过名字以后对一遍照样核对得了它的公钥')
        # 不行的：名字不对、找不到设备、还在的别的设备叫这个名字（管理员起的、服务器上的都算）
        self.assertEqual(self.code(admin, 'team-rename', tid, dev_b, ' '), 'name')
        self.assertEqual(self.code(admin, 'team-rename', tid, dev_b, 'x' * 65), 'name')
        self.assertEqual(self.code(admin, 'team-rename', tid, 'nobody', 'x'), 'no_device')
        self.assertEqual(self.code(admin, 'team-rename', tid, dev_b, 'studio'), 'name')
        self.assertEqual(self.code(bob, 'team-rename', tid, dev_b, 'x'), 'not_admin')
        self.assertEqual(self.code(admin, 'team-invite', tid, '小王的 MacBook'), 'name', '发邀请也一样：还在的别的设备叫这个名字不行')
        self.assertEqual(self.code(admin, 'team-invite', tid, 'studio'), 'name')
        # 服务器上有一台管理员这边没记着的叫这个名字：服务器在后面加 -2，这边记着的是管理员起的
        pub_x = keygen(self.d / 'x-key')
        self.assertEqual(sw.post(str(t / 'root_ed25519'), srv.port, 'team_enroll', {'devices': {'0d' * 8: {'pub': pub_x, 'name': 'mini'}}})[0], 200)
        self.assertEqual(self.lines(admin, 'team-rename', tid, dev_b, 'mini'), ['小王的 MacBook', 'mini-2', ''])
        d = json.loads((t / 'admin.json').read_text())['devices'][dev_b]
        self.assertEqual((d['label'], d['name']), ('mini-2', 'mini-2'), '一台设备只有一个名字：这边也记成服务器的')
        enrolls = lambda: (admin.home / 'curl.argv').read_text().count('team_enroll')
        n = enrolls()
        admin.ok('team-devices', tid)
        self.assertEqual(enrolls(), n, '名字一样了：对一遍时不再登记')
        # 管理员自己这台：team.json 里的名字一起改
        self.assertEqual(self.lines(admin, 'team-rename', tid, 'studio', 'studio-2026'), ['studio', 'studio-2026', ''])
        self.assertEqual(json.loads((t / 'team.json').read_text())['device'], 'studio-2026')
        self.assertEqual(self.lines(admin, 'team-sync'), ['%s\t测试组\tok\t1\t' % tid])
        self.assertEqual(json.loads((t / 'team.json').read_text())['device'], 'studio-2026', '同步配置时服务器说的也是它')
        # 还没加入的：只改这边记着的；它照邀请里的（改名之前的）名字加入，对一遍时服务器上换成改好的
        link_c, dev_c = self.invite(admin, tid, 'carol')
        got = self.lines(admin, 'team-rename', tid, 'carol', 'carol-mac')
        self.assertEqual(got[:2], ['carol', 'carol-mac'])
        self.assertIn('还没加入', got[2])
        self.assertEqual(json.loads((t / 'admin.json').read_text())['devices'][dev_c]['label'], 'carol-mac')
        carol = self.device('carol')
        self.assertEqual(self.lines(carol, 'team-join', stdin=link_c)[2], 'carol')
        rows = {r['dev']: r for r in json.loads(admin.ok('team-devices', tid, '--json'))['devices']}
        self.assertEqual((rows[dev_c]['label'], rows[dev_c]['name']), ('carol-mac', 'carol-mac'))
        carol.ok('team-sync')
        self.assertEqual(json.loads(carol.ok('team-list', '--json'))['teams'][0]['device'], 'carol-mac')
        # 自己退出了的：不登记回去，这边也不再记着它
        self.lines(bob, 'team-leave', 'apply', tid)
        self.assertEqual(self.code(admin, 'team-rename', tid, dev_b, 'bob-old'), 'no_device')
        self.assertFalse([m for m in state().values() if m['dev'] == dev_b], '退出了的没被改名字登记回去')
        self.assertNotIn(dev_b, json.loads((t / 'admin.json').read_text())['devices'])
        # 已经不在的设备以前的名字可以再用
        self.assertEqual(self.lines(admin, 'team-rename', tid, 'carol-mac', 'mini-2')[:2], ['carol-mac', 'mini-2'])

    def test_aliases(self):
        """Team 里账号的别名：没写的编成 team-1、team-2……（加账号、发布时；同一个 Team 里不重复，用过的号不再用，--mine 带进这台设备上
        起过的）；cca team alias 改。成员设备上只有邮箱的补上、照 Team 给的用着的跟着改、自己改过的不动；管理员这台
        设备上的不动；两个 Team 给的别名撞了时后来的那个叫「Team 名-N」"""
        admin, tid, root, srv, url, pin = self.new_team()
        aliases = lambda: json.loads((self.tdir(admin, tid) / 'admin.json').read_text())['config']['aliases']
        table = lambda d: json.loads((d.cca / 'accounts.json').read_text())
        nodes = lambda c: c.update(nodes=['US-A', 'US-B', 'US-C', 'US-D'])

        def apply(d):
            r = d.run('team-apply')
            self.assertEqual((r.returncode, r.stdout), (0, ''), r.stderr)
            return r.stderr
        self.load(admin, tid)
        self.assertEqual(aliases(), {'alice@example.com': 'alice', 'bob@example.com': 'team-1', 'carol@example.com': 'team-1'})
        # 多了一个账号：原来的编号不变，新的接着编
        self.lines(admin, 'team-map', tid, 'claude', 'Dave@Example.com', 'US-A')
        self.assertEqual(aliases(), {'alice@example.com': 'alice', 'bob@example.com': 'team-1', 'carol@example.com': 'team-1', 'dave@example.com': 'team-2'})
        # 去掉一个账号、再加一个：去掉的那个的号不给新加的
        self.lines(admin, 'team-map', tid, 'claude', 'dave@example.com', '-')
        self.lines(admin, 'team-map', tid, 'claude', 'erin@example.com', 'US-A')
        self.assertEqual(aliases(), {'alice@example.com': 'alice', 'bob@example.com': 'team-1', 'carol@example.com': 'team-1', 'erin@example.com': 'team-3'})
        # 另一个 Team 也有 team-1（给 frank）
        admin2 = self.device('admin2')
        tid2, root2, _ = self.lines(admin2, 'team-new', '设计 组', '--device', 'desk')
        srv2, url2, pin2 = self.server(root2, name='srv2')
        self.lines(admin2, 'team-attach', tid2, url2, pin2)
        self.lines(admin2, 'team-map', tid2, 'claude', 'frank@example.com', 'US-C')
        # 成员：只有邮箱的补上，加进来的带着 Team 给的；这台设备上起过的不动；两个 Team 的 team-1 撞了：后加入的那个的叫「设计组-1」
        bob = Device(self.d / 'bob', 'bob', {'alice@example.com': 'US-A', 'own(erin@example.com)': 'US-A'})
        bob.config(nodes)
        self.lines(bob, 'team-join', '--name', 'bob-mini', stdin=self.invite(admin, tid, 'bob-mini')[0])
        self.lines(bob, 'team-join', '--name', 'bob-mini', stdin=self.invite(admin2, tid2, 'bob-mini')[0])
        said = apply(bob)
        self.assertIn('按「测试组」的配置给 Claude 账号 alice@example.com 起了别名 alice', said)
        self.assertEqual(table(bob), {'alice(alice@example.com)': 'US-A', 'own(erin@example.com)': 'US-A', 'team-1(bob@example.com)': 'US-B',
                                      '设计组-1(frank@example.com)': 'US-C'})
        self.assertEqual(apply(bob), '', '再对一遍没有要改的')
        # 管理员改别名：照 Team 给的用着的跟着改（撞了以后换成的那个也是），自己起过的不动
        self.assertEqual(self.code(admin, 'team-alias', tid, 'bob@example.com', 'a b'), 'alias')
        self.assertEqual(self.code(admin, 'team-alias', tid, 'bob@example.com', 'alice'), 'alias', '同一个 Team 里别的账号叫这个')
        self.assertEqual(self.code(admin, 'team-alias', tid, 'nobody@example.com', 'x'), 'no_account')
        self.assertEqual(self.code(bob, 'team-alias', tid, 'bob@example.com', 'x'), 'not_admin')
        self.assertEqual(self.lines(admin, 'team-alias', tid, 'claude:Bob@Example.com', '小王'), ['team-1', '2'], '管理员自己这台和 bob 的配置变了')
        self.assertEqual(self.lines(admin, 'team-alias', tid, 'erin@example.com', 'erin-x'), ['team-3', '2'])
        self.assertEqual(self.lines(admin2, 'team-alias', tid2, 'frank@example.com', 'f1'), ['team-1', '2'])
        bob.ok('team-sync')
        said = apply(bob)
        self.assertIn('按「测试组」的配置把 Claude 账号 team-1（bob@example.com） 的别名改成 小王', said)
        self.assertIn('按「设计 组」的配置把 Claude 账号 设计组-1（frank@example.com） 的别名改成 f1', said)
        self.assertEqual(table(bob), {'alice(alice@example.com)': 'US-A', 'own(erin@example.com)': 'US-A', '小王(bob@example.com)': 'US-B',
                                      'f1(frank@example.com)': 'US-C'})
        # 这台设备上自己改了名字的：管理员再改，不跟着
        t = table(bob)
        t['mybob(bob@example.com)'] = t.pop('小王(bob@example.com)')
        (bob.cca / 'accounts.json').write_text(json.dumps(t))
        apply(bob)
        self.lines(admin, 'team-alias', tid, 'bob@example.com', '老王')
        bob.ok('team-sync')
        self.assertNotIn('别名改成', apply(bob))
        self.assertIn('mybob(bob@example.com)', table(bob))
        # 管理员这台设备上的不动：只有邮箱的不补，以前照 Team 加进来的也不跟着改
        (admin.cca / 'accounts.json').write_text(json.dumps({'team-1(bob@example.com)': 'US-B', 'erin@example.com': 'US-A'}))
        admin.config(nodes)
        admin.ok('team-sync')
        apply(admin)
        self.assertEqual({k: v for k, v in table(admin).items() if 'bob' in k or 'erin' in k}, {'team-1(bob@example.com)': 'US-B', 'erin@example.com': 'US-A'})
        # 发布时补上：以前的 Team 里还没有别名的账号（3.5.3-beta.5 以前加进来的）也编上
        a = json.loads((self.tdir(admin2, tid2) / 'admin.json').read_text())
        a['config']['accounts']['claude']['gina@example.com'] = 'US-D'
        (self.tdir(admin2, tid2) / 'admin.json').write_text(json.dumps(a))
        self.assertEqual(self.lines(admin2, 'team-publish', tid2), ['2', ''])
        self.assertEqual(json.loads((self.tdir(admin2, tid2) / 'admin.json').read_text())['config']['aliases'],
                         {'frank@example.com': 'f1', 'gina@example.com': 'team-2'})

    def test_alias_per_tool(self):
        """别名按工具记（3.5.3-beta.6 起）：各工具各自从 team-1 编；同一个邮箱在两个工具里各有各的号，成员设备上各工具的账号表各用各的；
        cca team alias 写 工具:邮箱 只改这个工具里的、只写邮箱改它在的每个工具里的；3.5.3-beta.5 的 Team（alias_n 是一个数、没有 tool_aliases）
        发布时照各工具重新编，成员设备上没有记录的旧编号跟着换；几个账号对调别名一次换好；cca team name 改 Team 的名字，成员设备跟着改"""
        admin, tid, root, srv, url, pin = self.new_team()
        adm = lambda: json.loads((self.tdir(admin, tid) / 'admin.json').read_text())
        self.load(admin, tid)
        self.assertEqual((adm()['config']['tool_aliases'], adm()['alias_n']),
                         ({'claude': {'alice@example.com': 'alice', 'bob@example.com': 'team-1'}, 'codex': {'carol@example.com': 'team-1'}},
                          {'claude': 1, 'codex': 1}))
        # 3.5.3-beta.5 的 Team：alias_n 是一个数，别名只有按邮箱的那一份（各工具一起编：carol 是 team-2）
        a = adm()
        a['alias_n'] = 2
        del a['config']['tool_aliases']
        a['config']['aliases'] = {'alice@example.com': 'alice', 'bob@example.com': 'team-1', 'carol@example.com': 'team-2'}
        (self.tdir(admin, tid) / 'admin.json').write_text(json.dumps(a))
        listing = json.loads(admin.ok('team-devices', tid, '--json'))
        self.assertEqual((listing['tool_aliases']['codex'], listing['aliases']['carol@example.com']), ({'carol@example.com': 'team-1'}, 'team-1'),
                         '还没发布：管理页给的已经是按工具编好的样子')
        self.assertNotIn('tool_aliases', adm()['config'], 'team devices 不改 admin.json 里的别名')
        # 成员设备：Codex 的账号表里有 carol，别名是旧版本照 team-2 补上的（没有记录）
        m = Device(self.d / 'm', 'm', {})
        m.config(lambda c: c.update(nodes=['US-A', 'US-B', 'US-C', 'US-D']))
        (m.cca / 'codex.json').write_text(json.dumps({'team-2(carol@example.com)': 'US-A'}))
        tables = lambda: (json.loads((m.cca / 'accounts.json').read_text()), json.loads((m.cca / 'codex.json').read_text()))

        def apply(*tools):
            said = ''
            for tool in tools:
                r = m.run('team-apply', CCA_TOOL=tool)
                self.assertEqual((r.returncode, r.stdout), (0, ''), r.stderr)
                said += r.stderr
            return said
        self.lines(m, 'team-join', '--name', 'm-mini', stdin=self.invite(admin, tid, 'm-mini')[0])
        self.assertEqual((adm()['config']['tool_aliases']['codex'], adm()['alias_n']), ({'carol@example.com': 'team-1'}, {'claude': 1, 'codex': 1}),
                         '发布（发邀请）时照各工具重新编，alias_n 换成按工具的')
        said = apply('claude', 'codex')
        self.assertIn('按「测试组」的配置把 Codex 账号 team-2（carol@example.com） 的别名改成 team-1', said, '没有记录的旧编号跟着换')
        self.assertEqual(tables(), ({'alice(alice@example.com)': 'US-A', 'team-1(bob@example.com)': 'US-B'}, {'team-1(carol@example.com)': 'US-A'}))
        self.assertEqual(apply('claude', 'codex'), '', '再对一遍没有要改的')
        # bob 在 Codex 里也有：Codex 里接着编（team-2）；Claude 里还是 team-1，按邮箱的那一份用它最前面那个工具（Claude）的
        self.lines(admin, 'team-map', tid, 'codex', 'bob@example.com', 'US-A')
        self.assertEqual((adm()['config']['tool_aliases']['codex'], adm()['config']['aliases']['bob@example.com']),
                         ({'carol@example.com': 'team-1', 'bob@example.com': 'team-2'}, 'team-1'))
        m.ok('team-sync')
        self.assertEqual((self.doc(m, tid)[1]['tool_aliases']['codex']['bob@example.com'], self.doc(m, tid)[1]['aliases']['bob@example.com']),
                         ('team-2', 'team-1'), '配置里两份都带着：旧版本的设备照按邮箱的')
        apply('claude', 'codex')
        self.assertEqual(tables(), ({'alice(alice@example.com)': 'US-A', 'team-1(bob@example.com)': 'US-B'},
                                    {'team-1(carol@example.com)': 'US-A', 'team-2(bob@example.com)': 'US-A'}))
        # 改别名：写了工具的只改这个工具里的；同一个工具里重名不行，别的工具里的可以；只写邮箱改它在的每个工具里的
        self.assertEqual(self.code(admin, 'team-alias', tid, 'codex:bob@example.com', 'team-1'), 'alias', 'Codex 里 carol 已经叫 team-1')
        self.assertEqual(self.code(admin, 'team-alias', tid, 'agy:bob@example.com', 'x'), 'no_account')
        self.assertEqual(self.lines(admin, 'team-alias', tid, 'codex:bob@example.com', 'alice'), ['team-2', '2'], 'Claude 里的 alice 不碍事')
        m.ok('team-sync')
        self.assertIn('按「测试组」的配置把 Codex 账号 team-2（bob@example.com） 的别名改成 alice', apply('claude', 'codex'))
        self.assertEqual(tables(), ({'alice(alice@example.com)': 'US-A', 'team-1(bob@example.com)': 'US-B'},
                                    {'team-1(carol@example.com)': 'US-A', 'alice(bob@example.com)': 'US-A'}))
        self.assertEqual(self.lines(admin, 'team-alias', tid, 'bob@example.com', '小王'), ['team-1、alice', '2'])
        m.ok('team-sync')
        apply('claude', 'codex')
        self.assertEqual(tables(), ({'alice(alice@example.com)': 'US-A', '小王(bob@example.com)': 'US-B'},
                                    {'team-1(carol@example.com)': 'US-A', '小王(bob@example.com)': 'US-A'}))
        # 两个账号对调别名：成员设备上一次换好（要换走的账号原来的别名不算占着）
        self.lines(admin, 'team-map', tid, 'claude', 'erin@example.com', 'US-C')
        m.ok('team-sync')
        apply('claude', 'codex')
        self.assertEqual(tables()[0]['team-2(erin@example.com)'], 'US-C')
        for spec, new in (('claude:bob@example.com', 'tmp'), ('claude:erin@example.com', '小王'), ('claude:bob@example.com', 'team-2')):
            self.lines(admin, 'team-alias', tid, spec, new)
        m.ok('team-sync')
        said = apply('claude')
        self.assertEqual(tables()[0], {'alice(alice@example.com)': 'US-A', 'team-2(bob@example.com)': 'US-B', '小王(erin@example.com)': 'US-C'}, said)
        self.assertEqual(apply('claude', 'codex'), '', '对调完了，再对一遍没有要改的')
        # 改 Team 的名字：管理员这台马上改，成员设备同步到新的一版时跟着改；这台设备上别的 Team 已经叫这个的不行
        self.assertEqual(self.code(admin, 'team-name', tid, ' '), 'name')
        self.lines(admin, 'team-new', '别的组', '--device', 'studio')
        self.assertEqual(self.code(admin, 'team-name', tid, '别的组'), 'name', '这台设备上已经有叫这个的 Team')
        self.assertEqual(self.code(m, 'team-name', tid, '新组'), 'not_admin')
        self.assertEqual(self.lines(admin, 'team-name', tid, ' 新组 '), ['测试组', '新组', '2'], '管理员自己这台和 m 的配置变了')
        self.assertEqual(json.loads((self.tdir(admin, tid) / 'team.json').read_text())['name'], '新组')
        m.ok('team-sync')
        self.assertEqual((json.loads((self.tdir(m, tid) / 'team.json').read_text())['name'], self.doc(m, tid)[1]['name']), ('新组', '新组'))
        # 和服务器对不上（假 curl：连不上）时名字不改，照原来的命令再来一次就行；发布没成时名字已经改了，原因里说照新名字发布
        named = lambda: json.loads((self.tdir(admin, tid) / 'team.json').read_text())['name']
        (admin.home / 'reply').mkdir()
        (admin.home / 'reply' / 'team_members').write_text('000\n')
        self.assertEqual((self.code(admin, 'team-name', tid, '新组二'), named()), ('server', '新组'))
        (admin.home / 'reply' / 'team_members').unlink()
        (admin.home / 'reply' / 'team_put').write_text('000\n')
        r = admin.run('team-name', tid, '新组二')
        self.assertEqual((r.returncode, r.stdout.split('\t')[0], named()), (1, 'publish', '新组二'), r.stdout)
        self.assertIn('名字已经改成「新组二」', r.stdout)
        self.assertIn('cca team publish 新组二', r.stdout)
        (admin.home / 'reply' / 'team_put').unlink()
        self.assertEqual(self.lines(admin, 'team-publish', '新组二'), ['2', ''])
        m.ok('team-sync')
        self.assertEqual(json.loads((self.tdir(m, tid) / 'team.json').read_text())['name'], '新组二')

    def test_alias_lock(self):
        """管理员锁定别名（cca team alias Team --lock）：配置里带 alias_lock，成员设备上照 Team 给的用（自己改过的换回来，cca rename
        改不了：退出码 4），没别名的也补上；管理员这台的账号表不动；解开以后又能改，现在用着的不变"""
        admin, tid, root, srv, url, pin = self.new_team()
        self.load(admin, tid)
        m = Device(self.d / 'm', 'm', {'mine(alice@example.com)': 'US-A', 'bob@example.com': 'US-B'})
        m.config(lambda c: c.update(nodes=['US-A', 'US-B', 'US-C', 'US-D']))
        table = lambda d: json.loads((d.cca / 'accounts.json').read_text())

        def apply(d):
            r = d.run('team-apply')
            self.assertEqual((r.returncode, r.stdout), (0, ''), r.stderr)
            return r.stderr
        self.lines(m, 'team-join', stdin=self.invite(admin, tid, 'm-mini')[0])
        apply(m)
        self.assertEqual(table(m), {'mine(alice@example.com)': 'US-A', 'team-1(bob@example.com)': 'US-B'}, '没锁定：自己起的不动')
        self.assertNotIn('alias_lock', self.doc(m, tid)[1], '没锁定时配置里没有这一项（和以前一样）')
        self.assertEqual(m.run('rename', 'team-1(bob@example.com)', 'b2').returncode, 0)
        # 锁定：管理员自己这台和 m 的配置变了；名单里说锁定着
        self.assertEqual(self.code(m, 'team-alias-lock', tid, 'on'), 'not_admin')
        self.assertEqual(self.lines(admin, 'team-alias-lock', tid, 'on'), ['测试组', '2'])
        self.assertEqual(self.lines(admin, 'team-alias-lock', tid, 'on'), ['测试组', '0'], '已经锁定着：配置没变')
        self.assertIs(json.loads(admin.ok('team-devices', tid, '--json'))['alias_lock'], True)
        self.assertEqual(admin.ok('team-devices', tid).split('\n')[0], '1')
        m.ok('team-sync')
        self.assertIs(self.doc(m, tid)[1]['alias_lock'], True)
        said = apply(m)
        self.assertIn('按「测试组」的配置把 Claude 账号 mine（alice@example.com） 的别名改成 alice', said)
        self.assertEqual(table(m), {'alice(alice@example.com)': 'US-A', 'team-1(bob@example.com)': 'US-B'}, '自己改过的换回 Team 给的')
        self.assertEqual(apply(m), '', '再对一遍没有要改的')
        r = m.run('rename', 'alice(alice@example.com)', 'x')
        self.assertEqual(r.returncode, 4, r.stdout)
        self.assertIn('Team「测试组」', r.stdout)
        self.assertIn('锁定', r.stdout)
        self.assertIn('alice(alice@example.com)', table(m))
        # 手改账号表也换回来；去掉了别名的也补上
        (m.cca / 'accounts.json').write_text(json.dumps({'own(alice@example.com)': 'US-A', 'bob@example.com': 'US-B'}))
        apply(m)
        self.assertEqual(table(m), {'alice(alice@example.com)': 'US-A', 'team-1(bob@example.com)': 'US-B'})
        # 不在 Team 里的账号照样能改
        (m.cca / 'accounts.json').write_text(json.dumps(dict(table(m), **{'zed@example.com': 'US-C'})))
        self.assertEqual(m.run('rename', 'zed@example.com', 'z').returncode, 0)
        # 管理员这台的账号表不动
        (admin.cca / 'accounts.json').write_text(json.dumps({'mybob(bob@example.com)': 'US-B'}))
        admin.config(lambda c: c.update(nodes=['US-A', 'US-B', 'US-C', 'US-D']))
        admin.ok('team-sync')
        apply(admin)
        self.assertIn('mybob(bob@example.com)', table(admin))
        self.assertEqual(admin.run('rename', 'mybob(bob@example.com)', 'b3').returncode, 0)
        # 改别名照样能改，各台设备跟着换
        self.lines(admin, 'team-alias', tid, 'bob@example.com', '小王')
        m.ok('team-sync')
        apply(m)
        self.assertIn('小王(bob@example.com)', table(m))
        # 解开：配置里没有这一项了；又能自己改，现在用着的不变
        self.assertEqual(self.lines(admin, 'team-alias-lock', tid, 'off'), ['测试组', '2'])
        self.assertIs(json.loads(admin.ok('team-devices', tid, '--json'))['alias_lock'], False)
        m.ok('team-sync')
        self.assertNotIn('alias_lock', self.doc(m, tid)[1])
        apply(m)
        self.assertIn('小王(bob@example.com)', table(m))
        self.assertEqual(m.run('rename', '小王(bob@example.com)', 'mybob').returncode, 0)
        self.assertIn('mybob(bob@example.com)', table(m))

    def test_nodes(self):
        """Team 的节点（cca team node）：列出来不带密钥；加、复制（整份复制，密钥不出这台设备）、去掉；还有账号、别的节点、安装客户端
        用着的去不掉；没人用的节点不改各台设备的配置，有账号走它了才发出去；实测用的临时内核配置；照实测的结果判断"""
        admin, tid, root, srv, url, pin = self.new_team()
        self.load(admin, tid)
        bob = self.device('bob')
        self.lines(bob, 'team-join', stdin=self.invite(admin, tid, 'bob', '--accounts', 'claude:alice@example.com')[0])
        seq = self.doc(bob, tid)[0]
        j = json.loads(admin.ok('team-node-ls', tid, '--json'))
        self.assertEqual((j['kind'], j['team'], j['name'], j['group'], j['install_node']), ('team_nodes', tid, '测试组', 'OpenAi', 'US-D'))
        self.assertEqual(j['accounts'], {'claude': {'alice@example.com': 'US-A', 'bob@example.com': 'US-B'}, 'codex': {'carol@example.com': 'US-A'},
                                         'agy': {}})
        by = {n['name']: n for n in j['nodes']}
        self.assertEqual(list(by), ['US-A', 'US-B', 'US-C', 'US-D', 'US-E'])
        self.assertEqual((by['US-A']['accounts'], by['US-B']['dialer'], by['US-D']['install'], by['US-E']['accounts']),
                         (['claude:alice@example.com', 'codex:carol@example.com'], 'US-C', True, []))
        self.assertIn('US-E\tsocks5\t192.0.2.10\t1149\t-\t-\t0', self.lines(admin, 'team-node-ls', tid), '没有的栏写 -')
        self.assertEqual(self.code(bob, 'team-node-ls', tid), 'not_admin')
        # 加：带密码的节点；列出来没有密码
        hk = {'name': 'HK-A', 'type': 'socks5', 'server': '192.0.2.20', 'port': 1090, 'username': 'u', 'password': 's3cret-pass'}
        self.assertEqual(self.lines(admin, 'team-node-add', tid, stdin=json.dumps(hk)), ['HK-A', '0'], '还没有账号走它：哪台设备的配置都不变')
        self.assertEqual(self.code(admin, 'team-node-add', tid, stdin=json.dumps(hk)), 'name')
        self.assertEqual(self.code(admin, 'team-node-add', tid, stdin='[1]'), 'node')
        self.assertEqual(self.code(admin, 'team-node-add', tid, stdin=json.dumps(dict(hk, name='CCA-1'))), 'node', '照成员设备查配置的那一套查')
        for argv in (('team-node-ls', tid), ('team-node-ls', tid, '--json')):
            self.assertNotIn('s3cret-pass', admin.ok(*argv))
        # 复制：定义一样（连密码），名字、端口换掉；模板不跟着变
        self.assertEqual(self.lines(admin, 'team-node-copy', tid, 'HK-A', 'HK-B', '--port', '1091'), ['HK-B', '0'])
        self.assertEqual(self.code(admin, 'team-node-copy', tid, 'HK-X', 'HK-C'), 'no_node')
        self.assertEqual(self.code(admin, 'team-node-copy', tid, 'HK-A', 'US-A'), 'name')
        self.assertEqual(self.code(admin, 'team-node-copy', tid, 'HK-A', 'HK-C', '--port', '0'), 'usage')
        px = {p['name']: p for p in json.loads((self.tdir(admin, tid) / 'admin.json').read_text())['config']['nodes']['proxies']}
        self.assertEqual((px['HK-B'], px['HK-A']['port']), (dict(hk, name='HK-B', port=1091), 1090))
        bob.ok('team-sync')
        self.assertEqual(self.doc(bob, tid)[0], seq, '加了没人用的节点：bob 的配置没变')
        # 有账号走它了才发出去（bob 有 alice）；这时去不掉
        self.assertEqual(self.lines(admin, 'team-map', tid, 'claude', 'alice@example.com', 'HK-B'), ['2'])
        bob.ok('team-sync')
        self.assertEqual([p['name'] for p in self.doc(bob, tid)[1]['nodes']['proxies']], ['US-D', 'HK-B'], '还有安装客户端用的')
        self.assertEqual(self.code(admin, 'team-node-rm', tid, 'HK-B'), 'node_used')
        r = admin.run('team-node-rm', tid, 'HK-B')
        self.assertIn('claude:alice@example.com', r.stdout)
        self.assertEqual([self.code(admin, 'team-node-rm', tid, n) for n in ('US-C', 'US-D', 'NOPE')], ['node_used', 'node_used', 'no_node'],
                         '别的节点经它连出去、安装客户端用它、没有这个节点')
        self.lines(admin, 'team-map', tid, 'claude', 'alice@example.com', 'US-A')
        self.assertEqual(self.lines(admin, 'team-node-rm', tid, 'HK-B'), ['0'])
        self.assertEqual(self.lines(admin, 'team-node-rm', tid, 'HK-A'), ['0'])
        self.assertNotIn('HK-A', [n['name'] for n in json.loads(admin.ok('team-node-ls', tid, '--json'))['nodes']])
        # 安装客户端用的节点：只能是这个 Team 的；换了每台设备的配置都带着新的（连同它的定义），原来的没人用了就不再带；- 是不再指定；
        # stdout 是原来的、存上了几台
        self.assertEqual([self.code(admin, 'team-node-install', tid, n) for n in ('NOPE', ' ')], ['no_node', 'usage'])
        self.assertEqual(self.code(bob, 'team-node-install', tid, 'US-E'), 'not_admin')
        self.assertEqual(self.lines(admin, 'team-node-install', tid, 'US-E'), ['US-D', '2'], '管理员自己这台和 bob 的配置里都有它')
        bob.ok('team-sync')
        doc = self.doc(bob, tid)[1]
        self.assertEqual((doc['install_node'], [p['name'] for p in doc['nodes']['proxies']]), ('US-E', ['US-A', 'US-E']))
        self.assertEqual(json.loads(admin.ok('team-node-ls', tid, '--json'))['install_node'], 'US-E')
        self.assertEqual(self.code(admin, 'team-node-rm', tid, 'US-E'), 'node_used')
        self.assertEqual(self.lines(admin, 'team-node-install', tid, 'US-E'), ['US-E', '0'], '没变：不重新发布')
        self.assertEqual(self.lines(admin, 'team-node-install', tid, '-'), ['US-E', '2'])
        bob.ok('team-sync')
        doc = self.doc(bob, tid)[1]
        self.assertEqual((doc['install_node'], [p['name'] for p in doc['nodes']['proxies']]), ('', ['US-A']))
        self.assertEqual(self.lines(admin, 'team-node-install', tid, '-'), ['', '0'])
        self.assertEqual(self.lines(admin, 'team-node-install', tid, 'US-D'), ['', '2'])
        # 实测用的临时内核的配置：每个节点一个入站、带上 dialer-proxy 连着的；要测的网站看走这些节点的账号是哪些工具的
        f = self.d / 'test.json'
        plan = self.lines(admin, 'team-node-testconfig', tid, str(f), 'US-A', 'US-B')
        self.assertEqual(plan[0], 'claude.ai chatgpt.com www.cloudflare.com')
        rows = [x.split('\t') for x in plan[1:]]
        self.assertEqual([(r[0], r[2], r[3]) for r in rows], [('US-A', '192.0.2.10', '1145'), ('US-B', '192.0.2.10', '1146')])
        self.assertEqual(oct(stat.S_IMODE(os.stat(f).st_mode)), '0o600')
        cfg = json.loads(f.read_text())
        self.assertEqual([(x['name'], x['listen'], str(x['port']), x['proxy']) for x in cfg['listeners']],
                         [('cca-test-1', '127.0.0.1', rows[0][1], 'US-A'), ('cca-test-2', '127.0.0.1', rows[1][1], 'US-B')])
        self.assertEqual(([p['name'] for p in cfg['proxies']], cfg['rules']), (['US-A', 'US-B', 'US-C'], ['MATCH,REJECT']))
        self.assertFalse({'external-controller', 'external-controller-unix', 'mixed-port', 'tun', 'dns'} & set(cfg))
        self.assertNotEqual(admin.run('team-node-testconfig', tid, str(f), 'US-A').returncode, 0, '文件已经有了：不覆盖')
        f.unlink()
        self.assertEqual(self.lines(admin, 'team-node-testconfig', tid, str(f), 'US-E', '--tool', 'agy')[0], 'www.cloudflare.com')
        f.unlink()
        self.assertEqual(self.lines(admin, 'team-node-testconfig', tid, str(f), 'US-E')[0], 'claude.ai www.cloudflare.com', '没有账号走它：按 Claude')
        f.unlink()
        self.assertEqual(len(self.lines(admin, 'team-node-testconfig', tid, str(f))), 6, '没写节点：全部')
        self.assertEqual(self.code(admin, 'team-node-testconfig', tid, str(self.d / 'x.json'), 'NOPE'), 'no_node')
        # 照实测的结果判断：B 和 A 撞了 IP（3）；C 不通、端口连不上（1）；都不通（2）
        txt = ('claude.ai www.cloudflare.com\nA\tclaude.ai\t203.0.113.1\tUS\tSJC\nA\twww.cloudflare.com\t198.51.100.1\tUS\tSJC\n'
               'B\tclaude.ai\t203.0.113.1\tUS\tSJC\nB\twww.cloudflare.com\t198.51.100.1\tUS\tSJC\n')
        r = admin.run('team-node-verdict', stdin=txt)
        self.assertEqual(r.returncode, 3)
        self.assertEqual([x.split('\t')[:2] for x in r.stdout.splitlines()], [['flag', 'A'], ['!', 'A'], ['flag', 'B'], ['!', 'B']])
        r = admin.run('team-node-verdict', stdin=txt + 'C\tclaude.ai\t-\tcurl: (7) refused\nC\ttcp\tclosed\n')
        self.assertEqual(r.returncode, 1)
        j = json.loads(admin.run('team-node-verdict', '--json', stdin=txt + 'C\tclaude.ai\t-\tcurl: (7) refused\nC\ttcp\tclosed\n').stdout)
        self.assertEqual((j['kind'], j['hosts'], [(n['name'], n['state'], n['tcp']) for n in j['nodes']]),
                         ('team_node_test', ['claude.ai', 'www.cloudflare.com'], [('A', 'flag', None), ('B', 'flag', None), ('C', 'down', 'closed')]))
        self.assertEqual((j['nodes'][0]['exits']['claude.ai'], j['nodes'][2]['exits']['claude.ai']),
                         ({'ip': '203.0.113.1', 'loc': 'US', 'colo': 'SJC'}, {'error': 'curl: (7) refused'}))
        self.assertEqual(admin.run('team-node-verdict', stdin='claude.ai www.cloudflare.com\nA\tclaude.ai\t-\t超时\n').returncode, 2)
        # 还没接上服务器的 Team：只改本机；没有节点了是 null
        lone = self.device('lone')
        lid = self.lines(lone, 'team-new', '一个人', '--device', 'mini')[0]
        j = json.loads(lone.ok('team-node-ls', lid, '--json'))
        self.assertEqual((j['group'], j['nodes'], j['accounts']), (None, [], {}))
        self.assertEqual(self.lines(lone, 'team-node-add', lid, stdin=json.dumps(hk)), ['HK-A', '0'])
        self.assertEqual(json.loads(lone.ok('team-node-ls', lid, '--json'))['group'], 'OpenAi')
        self.assertEqual((self.lines(lone, 'team-node-install', lid, 'HK-A'), self.lines(lone, 'team-node-install', lid, '-')),
                         (['', '0'], ['HK-A', '0']))
        self.assertEqual(self.lines(lone, 'team-node-rm', lid, 'HK-A'), ['0'])
        self.assertIsNone(json.loads((self.tdir(lone, lid) / 'admin.json').read_text())['config']['nodes'])

    def test_spec_admin(self):
        """管理员由照文档独立实现的那一份扮演：它封的、签的辅助程序解得开、验得过；伪造的、调包的、旧的不要"""
        root = keygen(self.d / 'root')
        tid = sw.team_id(root)
        srv, url, pin = self.server(root)
        rk = str(self.d / 'root')
        secret = secrets.token_hex(16)
        token, dev, keys = sw.invite_parts(secret)
        st, j = sw.post(rk, srv.port, 'team_invite', {'token': hashlib.sha256(token.encode()).hexdigest(), 'dev': dev, 'ttl': 600})
        self.assertEqual(st, 200, j)
        base = {'v': 1, 'at': 1790000000, 'name': '独立实现的组', 'salt': 'cd' * 16, 'server': {'u': url, 'p': [pin]},
                'accounts': {'claude': {'alice@example.com': 'US-A'}}, 'aliases': {}, 'install_node': '', 'nodes': None}

        def put(seq, doc, signer=rk, to=dev, sealed_for=dev, k=keys):
            ct = sw.team_doc_seal(k, tid, sealed_for, seq, doc)
            env = {'seq': seq, 'ct': ct, 'sig': sw.team_doc_sign(signer, tid, sealed_for, seq, ct)}
            st, j = sw.post(rk, srv.port, 'team_put', {'docs': {to: env}})
            self.assertEqual((st, j.get('stored')), (200, [to]), j)

        put(3, base)
        bob = self.device('bob')
        link = sw.invite_encode(url, [pin], root, secret, '链接里的名字')
        self.assertEqual(self.lines(bob, 'team-join', '--name', 'bob', stdin=link), ['独立实现的组', '127.0.0.1:%d' % srv.port, 'bob', '1', tid, 'ok', '1'],
                         '名字照配置里的（管理员改过名）')
        self.assertEqual(self.doc(bob, tid), (3, base))

        def sync():
            state, seq, why = self.lines(bob, 'team-sync')[0].split('\t')[2:]
            return state, int(seq), why

        self.assertEqual(sync(), ('ok', 3, ''))
        # 不是根密钥签的
        keygen(self.d / 'evil')
        put(4, dict(base, name='伪造的'), signer=str(self.d / 'evil'))
        state, seq, why = sync()
        self.assertEqual((state, seq), ('error', 3))
        self.assertIn('签名', why)
        # 根密钥签了、却是别的设备的那一份（服务器调包）
        other = sw.invite_parts(secrets.token_hex(16))
        put(5, dict(base, name='别人的'), sealed_for=other[1], k=other[2])
        state, seq, why = sync()
        self.assertEqual((state, seq), ('error', 3))
        # 内容样子不对（盐不是十六进制）
        put(6, dict(base, salt='xyz'))
        state, seq, why = sync()
        self.assertEqual((state, seq), ('error', 3))
        self.assertIn('salt', why)
        self.assertEqual(self.doc(bob, tid), (3, base), '不对的都没换上')
        self.assertEqual(json.loads(bob.ok('team-list', '--json'))['teams'][0]['state'], 'error')
        # 对的一版：换上；配置里说服务器搬了，跟着换
        srv2, url2, pin2 = self.server(root, name='srv2')
        moved = dict(base, at=1790000100, server={'u': url2, 'p': [pin2]})
        put(7, moved)
        self.assertEqual(sync(), ('ok', 7, ''))
        tj = json.loads((self.tdir(bob, tid) / 'team.json').read_text())
        self.assertEqual((tj['url'], tj['pins']), (url2, [pin2]))
        # 新的那台上没有这台设备（记录的编号不一样）：等管理员登记回去
        state, seq, why = sync()
        self.assertEqual((state, seq), ('lost', 7))
        pub = (self.tdir(bob, tid) / 'id_ed25519.pub').read_text().split()
        st, j = sw.post(rk, srv2.port, 'team_enroll', {'devices': {dev: {'pub': ' '.join(pub[:2]), 'name': 'bob'}}})
        self.assertEqual(st, 200, j)
        self.assertEqual(sync(), ('ok', 7, ''))
        # 服务器给的比手里的旧（新的那台上存了一份旧的）
        ct = sw.team_doc_seal(keys, tid, dev, 2, base)
        env = {'seq': 2, 'ct': ct, 'sig': sw.team_doc_sign(rk, tid, dev, 2, ct)}
        self.assertEqual(sw.post(rk, srv2.port, 'team_put', {'docs': {dev: env}})[1]['stored'], [dev])
        state, seq, why = sync()
        self.assertEqual((state, seq), ('error', 7))
        self.assertIn('旧', why)
        self.assertEqual(self.doc(bob, tid), (7, moved))

    def test_records_lost_and_move(self):
        """服务器的记录没了：管理员照留着的公钥登记回去、重新发布；换一台服务器：设备从原来那台上知道新的地址，自己换过去"""
        admin, tid, root, srv, url, pin = self.new_team()
        self.load(admin, tid)
        link, dev_b = self.invite(admin, tid, 'bob-mini')
        bob = self.device('bob')
        self.assertEqual(self.lines(bob, 'team-join', '--name', 'bob-mini', stdin=link)[5:], ['ok', '3'])
        link_p, dev_p = self.invite(admin, tid, 'pending')
        self.assertEqual({r['label']: r['state'] for r in json.loads(admin.ok('team-devices', tid, '--json'))['devices']},
                         {'studio': 'joined', 'bob-mini': 'joined', 'pending': 'invited'})
        port = srv.port
        srv.stop()
        for f in ('state.json', 'team-docs.json'):
            (self.d / 'srv' / f).unlink()
        srv, _, _ = self.server(root, port=port)
        self.assertEqual(self.lines(bob, 'team-sync')[0].split('\t')[2], 'lost')
        self.assertEqual(self.code(self.device('late'), 'team-join', stdin=link_p), 'denied', '没用掉的邀请跟着没了')
        rows = {r['label']: r for r in json.loads(admin.ok('team-devices', tid, '--json'))['devices']}
        self.assertEqual({k: r['state'] for k, r in rows.items()}, {'studio': 'joined', 'bob-mini': 'joined', 'pending': 'expired'})
        self.assertEqual(self.lines(admin, 'team-publish', tid), ['2', ''], '各台的配置重新发布')
        self.assertEqual(self.lines(bob, 'team-sync')[0].split('\t')[2:4], ['ok', '2'])
        self.assertEqual(self.lines(admin, 'team-sync')[0].split('\t')[2], 'ok')
        # 换一台服务器（另一张证书）。还没用掉的邀请写的是原来那台的地址：到原来那台上撤回，这边记成过期了（要重新发）
        link_q, dev_q = self.invite(admin, tid, 'pending-2')
        srv2, url2, pin2 = self.server(root, name='srv2')
        self.assertEqual(self.lines(admin, 'team-attach', tid, url2, pin2), ['127.0.0.1:%d' % srv2.port, 'studio', '2', '2', ''])
        self.assertEqual(self.code(self.device('late-2'), 'team-join', stdin=link_q), 'denied', '原来那台上撤回了')
        rows = {r['label']: r['state'] for r in json.loads(admin.ok('team-devices', tid, '--json'))['devices']}
        self.assertEqual((rows['pending-2'], rows['bob-mini']), ('expired', 'joined'))
        self.assertNotIn('moved_from', json.loads((self.tdir(admin, tid) / 'admin.json').read_text()))
        self.assertEqual(json.loads((self.tdir(bob, tid) / 'team.json').read_text())['url'], url, '还没同步过：还连着原来那台')
        self.assertEqual(self.lines(bob, 'team-sync')[0].split('\t')[2:4], ['ok', '3'], '从原来那台上取到指着新地址的配置')
        tj = json.loads((self.tdir(bob, tid) / 'team.json').read_text())
        self.assertEqual((tj['url'], tj['pins']), (url2, [pin2]))
        srv.stop()
        self.assertEqual(self.lines(bob, 'team-sync')[0].split('\t')[2:4], ['ok', '3'], '以后找新的那台：手里的就是它存的那一版')
        self.assertEqual(self.lines(admin, 'team-map', tid, 'claude', 'erin@example.com', 'US-A'), ['2'])
        self.assertEqual(self.lines(bob, 'team-sync')[0].split('\t')[2:4], ['ok', '4'])
        # 原来那台连不上时换：说一声（还连着它的设备要重新邀请）
        srv2.stop()
        srv3, url3, pin3 = self.server(root, name='srv3')
        got = self.lines(admin, 'team-attach', tid, url3, pin3)
        self.assertEqual(got[:4], ['127.0.0.1:%d' % srv3.port, 'studio', '2', '2'])
        self.assertIn('连不上', got[4])


    def test_accounts_follow(self):
        """账号表照 Team 的配置改：给了的账号加进来、节点由它定，这台机器本来就有的只改节点；不再给了的收起来（登录留着）、再给回来照原样；
        被移除以后这台机器本来就有的留着（节点不再被固定）"""
        admin, tid, root, srv, url, pin = self.new_team()
        self.load(admin, tid)
        link, dev_b = self.invite(admin, tid, 'bob-mini', '--accounts', 'claude:alice@example.com,claude:bob@example.com')
        # 两个账号都是这台机器本来就有的：bob 起过别名，alice 只有邮箱、走 US-C
        bob = Device(self.d / 'bob', 'bob', {'mine(bob@example.com)': 'US-D', 'alice@example.com': 'US-C'})
        bob.config(lambda c: c.update(nodes=['US-A', 'US-B', 'US-C', 'US-D']))
        table = lambda: json.loads((bob.cca / 'accounts.json').read_text())

        def apply():
            r = bob.run('team-apply')
            self.assertEqual((r.returncode, r.stdout), (0, ''), r.stderr)
            return r.stderr

        self.assertEqual(apply(), '', '还没加入 Team：没有要改的')
        self.assertEqual(table(), {'mine(bob@example.com)': 'US-D', 'alice@example.com': 'US-C'})
        # 加入：Team 给了 alice（US-A，别名 alice）和 bob（US-B）
        self.assertEqual(self.lines(bob, 'team-join', '--name', 'bob-mini', stdin=link)[5:], ['ok', '2'])
        said = apply()
        self.assertIn('alice（alice@example.com） 的出口按「测试组」的配置固定为 US-A（账号表里写的是 US-C，已改回）', said)
        self.assertIn('mine（bob@example.com） 的出口按「测试组」的配置固定为 US-B（账号表里写的是 US-D，已改回）', said)
        self.assertIn('按「测试组」的配置给 Claude 账号 alice@example.com 起了别名 alice', said)
        self.assertEqual(table(), {'mine(bob@example.com)': 'US-B', 'alice(alice@example.com)': 'US-A'})
        self.assertEqual(apply(), '', '再对一遍没有要改的')
        cfg = lambda: json.loads((bob.cca / 'config.json').read_text())
        self.assertNotIn('team_added', json.dumps(cfg()), '这两个账号都是这台机器本来就有的：Team 只管节点，不记成它加的')
        # 管理员多给一个 erin：这台机器上没有，加进来（记成 Team 加的）
        self.assertEqual(self.lines(admin, 'team-map', tid, 'claude', 'erin@example.com', 'US-D'), ['1'])
        self.assertEqual(self.lines(admin, 'team-grant', tid, 'bob-mini', '+claude:erin@example.com'), ['3 个账号'])
        self.assertEqual(self.lines(bob, 'team-sync')[0].split('\t')[2], 'ok')
        self.assertIn('按「测试组」的配置添加了 Claude 账号 team-2（erin@example.com），出口 US-D', apply(), '新加的账号编成 team-2（Claude 里的第二个：Codex 的 carol 另编）')
        self.assertEqual(table()['team-2(erin@example.com)'], 'US-D')
        self.assertEqual(cfg()['team_added'], {ident('erin@example.com'): tid})
        # 手里那一份配置一时读不出来（文件坏了）：说不准还给不给，不把 Team 加的账号收起来
        good = (self.tdir(bob, tid) / 'doc.json').read_text()
        (self.tdir(bob, tid) / 'doc.json').write_text('{')
        self.assertNotIn('收起来', apply())
        self.assertIn('team-2(erin@example.com)', table())
        (self.tdir(bob, tid) / 'doc.json').write_text(good)
        apply()
        self.assertEqual((table()['team-2(erin@example.com)'], table()['alice(alice@example.com)']), ('US-D', 'US-A'))
        # 不再给 erin：收起来（登录留着），再给回来照原来的键
        self.assertEqual(self.lines(admin, 'team-grant', tid, 'bob-mini', '-claude:erin@example.com'), ['2 个账号'])
        bob.ok('team-sync')
        self.assertIn('Team 不再给这台设备 Claude 账号 team-2（erin@example.com）：收起来了', apply())
        self.assertNotIn('team-2(erin@example.com)', table())
        self.assertEqual((cfg()['parked'], 'team_added' in cfg()),
                         ({ident('erin@example.com'): {'key': 'team-2(erin@example.com)', 'node': 'US-D', 'team': tid}}, False))
        self.assertEqual(self.lines(admin, 'team-grant', tid, 'bob-mini', '+claude:erin@example.com'), ['3 个账号'])
        bob.ok('team-sync')
        self.assertIn('添加了 Claude 账号 team-2（erin@example.com）', apply())
        self.assertEqual(('team-2(erin@example.com)' in table(), 'parked' in cfg()), (True, False))
        # 不再给 bob@example.com：它是这台机器自己的账号，留着（节点不再被固定）
        self.assertEqual(self.lines(admin, 'team-grant', tid, 'bob-mini', '-claude:bob@example.com'), ['2 个账号'])
        bob.ok('team-sync')
        self.assertEqual(apply(), '')
        self.assertEqual(table()['mine(bob@example.com)'], 'US-B')
        # 被管理员移除了：手里那一份不再算数。Team 加的 erin 收起来；这台机器本来就有的 alice 留着（节点不再被固定）
        self.assertEqual(self.lines(admin, 'team-revoke', 'apply', tid, 'bob-mini'), ['bob-mini'])
        self.assertEqual(self.lines(bob, 'team-sync')[0].split('\t')[2], 'removed')
        self.assertIn('Team 不再给这台设备 Claude 账号 team-2（erin@example.com）：收起来了', apply())
        self.assertEqual(table(), {'mine(bob@example.com)': 'US-B', 'alice(alice@example.com)': 'US-A'})
        self.assertEqual(apply(), '', '再对一遍没有要改的')

    def test_pipelines(self):
        """Team 给的账号：记下的 API key、折合金额、设备统计都经这个 Team 的同步服务器（盐是配置里的那一份、签名用这台设备在 Team 里的密钥），
        不去别的服务器；被移除以后不再经它"""
        admin, tid, root, srv, url, pin = self.new_team()
        self.load(admin, tid)
        link, dev_b = self.invite(admin, tid, 'bob-mini', '--accounts', 'claude:alice@example.com')
        bob = Device(self.d / 'bob', 'bob', {})
        bob.config(lambda c: c.update(nodes=['US-A', 'US-B', 'US-C', 'US-D']))
        self.assertEqual(self.lines(bob, 'team-join', '--name', 'bob-mini', stdin=link)[5:], ['ok', '1'])
        self.assertEqual(bob.run('team-apply').returncode, 0)
        key = 'alice(alice@example.com)'
        self.assertEqual(json.loads((bob.cca / 'accounts.json').read_text()), {key: 'US-A'})
        bob.login('u-alice-0001', key)
        salt = self.doc(bob, tid)[1]['salt']
        aid = sw.aid_of(salt, 'claude', 'alice@example.com')
        fp = sw.fp_of(str(self.tdir(bob, tid) / 'id_ed25519.pub'))

        def asked(op):
            """这台设备向服务器发过的这种请求：[(签名的设备指纹, 涉及的账号摘要…)]"""
            return [(r['key'], tuple(sorted(set(r.get('get') or []) | set(r.get('put') or {}) | set(r.get('accounts') or [])))) for r in bob.sent(op)]

        # 记下的 API key
        k1 = 'sk-ant-api03-' + 'a' * 40
        self.assertEqual(bob.keyset(k1, key).strip(), 'shared')
        bob.ok('key-sync', 'force')
        state = json.loads((bob.cca / 'presence' / ('team.%s.keys.json' % tid)).read_text())
        self.assertEqual((state.get('error'), state.get('synced', 0) > 0), (None, True), state)
        self.assertEqual(set(asked('keys')), {(fp, (aid,))})
        blob = json.loads((self.d / 'srv' / 'keys.json').read_text())['acc'][aid]['blob']
        got = sw.keyshare_open(sw.derive(sw.KEYSHARE, salt, 'claude', 'alice@example.com', 'u-alice-0001'), aid, blob)
        self.assertEqual(got['key'], k1, '服务器上存的照文档解得开：盐是 Team 的配置里的那一份')
        self.assertFalse((bob.cca / 'presence' / 'keys.json').exists(), '不带前缀的那一份（3.6.0 以前维护者的服务器的）没有')
        # 折合金额
        hour = int(time.time() // 3600 * 3600) - 3600
        (bob.cca / 'cost').mkdir(exist_ok=True)
        (bob.cca / 'cost' / 'local.json').write_text(json.dumps({'v': 1, 'build': 'x', 'who': {'claude:u-alice-0001': 'alice@example.com'},
                                                                 'acc': {'claude:u-alice-0001': {str(hour): [1000000, 1, 1, 1, 0, 0, 0]}},
                                                                 'um': {}, 'orphan': {}}))
        r = bob.cost_sync()
        self.assertEqual((r['state'], [(h['kind'], h.get('team'), h['state']) for h in r['hubs']]), ('ok', [('team', tid, 'ok')]), r)
        oaid = sw.orphan_aid(salt)
        self.assertEqual(set(asked('ledger')), {(fp, tuple(sorted((aid, oaid))))}, '这个账号的，和认不出是哪个账号的那一份')
        slots = json.loads((self.d / 'srv' / 'cost-slots.json').read_text())['acc']
        doc = sw.cost_open(sw.team_keys(salt), aid, fp, slots[aid][fp]['blob'])
        self.assertEqual(list(doc['h'].values()), [[1000000, 1, 1, 1, 0, 0, 0]])
        self.assertEqual(sw.cost_open(sw.team_keys(salt), oaid, fp, slots[oaid][fp]['blob'])['h'], {}, '认不出的那一份：这台设备没有，传空的')
        self.assertTrue((bob.cca / 'cost' / ('team.%s.sync.json' % tid)).exists())
        # 设备统计
        v = json.loads(bob.ok('presence-view', '--json'))
        self.assertEqual(([(x['key'], x['hub'], x['team']) for x in v['accounts']], [(h['kind'], h['team']) for h in v['hubs']]),
                         ([(key, 'team', tid)], [('team', tid)]), '是哪个 Team 的写在旁边（可能同时在几个 Team 里）')
        self.assertEqual(set(asked('view')), {(fp, (aid,))})
        # 被管理员移除了：不再经它；经它同步时留下的状态文件删掉（里面有解开了的别的设备的折合金额）
        left = lambda: sorted(f.name for d in ('presence', 'cost') for f in (bob.cca / d).glob('team.*'))
        self.assertEqual([n for n in left() if n.endswith('.json')], ['team.%s.keys.json' % tid, 'team.%s.sync.json' % tid])
        self.assertEqual(self.lines(admin, 'team-revoke', 'apply', tid, 'bob-mini'), ['bob-mini'])
        self.assertEqual(self.lines(bob, 'team-sync')[0].split('\t')[2], 'removed')
        self.assertEqual(left(), [])
        n = len(bob.sent())
        bob.ok('key-sync', 'force')
        self.assertEqual(len(bob.sent()), n, '没有用得了的服务器：不发请求')

    def test_agent(self):
        """上报进程里 Team 的那一路：没有会话开着时一次都不报；开着时拿这台设备在 Team 里的密钥报给它的服务器；进程开着时加入的另一个
        Team 接着就有一路（各报各的账号）；被管理员移除以后那一路停了，另一路照旧"""
        admin, tid, root, srv, url, pin = self.new_team()
        self.load(admin, tid)
        link, _ = self.invite(admin, tid, 'bob-mini', '--accounts', 'claude:alice@example.com')
        bob = Device(self.d / 'bob', 'bob', {})
        bob.config(lambda c: c.update(nodes=['US-A', 'US-B', 'US-C', 'US-D']))
        self.assertEqual(self.lines(bob, 'team-join', '--name', 'bob-mini', stdin=link)[5:], ['ok', '1'])
        self.assertEqual(bob.run('team-apply').returncode, 0)
        alice = 'alice(alice@example.com)'
        bob.login('u-alice-0001', alice)
        who = lambda t, email: (sw.fp_of(str(self.tdir(bob, t) / 'id_ed25519.pub')), sw.aid_of(self.doc(bob, t)[1]['salt'], 'claude', email))
        fp, aid = who(tid, 'alice@example.com')
        reports = lambda f: [r for r in bob.sent('report') if r['key'] == f]
        # 没有会话开着：一次都不报（不必告诉那台服务器这台设备开着），进程自己退出
        # 假 ps 的进程表里只有一个不相干的进程（一行都没有时上报进程当成没问到进程表，一直等）
        (bob.home / 'ps.out').write_text('%5d %5d %-8s %9s %s %s\n' % (1, 0, '??', '0:00.50', T0, '/sbin/launchd'))
        self.assertEqual(self.agent(bob).wait(30), 0)
        self.assertEqual(bob.sent('report'), [])
        # 开着一个会话
        self.session(bob, alice)
        self.agent(bob)
        [r] = wait_for('报给 Team 的服务器', lambda: reports(fp))
        self.assertEqual((r['code'], list(r['units'])), (200, [aid]))
        # 进程开着时加入第二个 Team（另一台服务器），它给的是 bob@example.com
        admin2 = self.device('admin2')
        tid2, root2, _ = self.lines(admin2, 'team-new', '第二组', '--device', 'imac')
        srv2, url2, pin2 = self.server(root2, name='srv2')
        self.lines(admin2, 'team-attach', tid2, url2, pin2)
        self.load(admin2, tid2)
        link2, _ = self.invite(admin2, tid2, 'bob-mini', '--accounts', 'claude:bob@example.com')
        self.assertEqual(self.lines(bob, 'team-join', '--name', 'bob-mini', stdin=link2)[5:], ['ok', '1'])
        self.assertEqual(bob.run('team-apply').returncode, 0)
        bob.login('u-bob-0001', 'bob@example.com')
        self.session(bob, 'bob@example.com', SESSION_PID - 1)
        fp2, aid2 = who(tid2, 'bob@example.com')
        [r2] = wait_for('新加入的那个 Team 也有了一路', lambda: reports(fp2))
        self.assertEqual((r2['code'], list(r2['units'])), (200, [aid2]))
        self.assertTrue(all(list(x['units']) == [aid] for x in reports(fp)), '各报各的账号')
        # 被第一个 Team 的管理员移除：同步配置时知道了，那一路停了（alice 又开一个会话也不报）；第二个 Team 那一路照旧
        self.assertEqual(self.lines(admin, 'team-revoke', 'apply', tid, 'bob-mini'), ['bob-mini'])
        self.assertEqual(self.lines(bob, 'team-sync')[0].split('\t')[2], 'removed')
        self.more_rounds([bob], 2)
        n = len(reports(fp))
        self.session(bob, alice, SESSION_PID - 2)
        self.session(bob, 'bob@example.com', SESSION_PID - 3)
        wait_for('第二个 Team 那一路报了新开的会话', lambda: len(reports(fp2)) >= 2)
        self.more_rounds([bob], 2)
        self.assertEqual(len(reports(fp)), n, '被移除的那一路不再报')
        self.assertEqual([x['code'] for x in reports(fp2)], [200] * len(reports(fp2)))

    def test_ca_mode(self):
        """没有指纹的服务器（托管平台上的、反向代理后面的）：照系统的证书机构验证书（不带 -k、--pinnedpubkey）；验不过时接不上、
        加入不了，什么都不留"""
        admin = self.device('admin')
        tid, root, _ = self.lines(admin, 'team-new', '托管组', '--device', 'studio')
        d = self.d / 'ca'
        d.mkdir()
        make_ca_cert(d)
        srv = Server(d, team_root=root)
        self.addCleanup(srv.stop)
        url = 'https://127.0.0.1:%d' % srv.port
        self.assertEqual(self.code(admin, 'team-attach', tid, url), 'cert', '系统的证书机构不认这个证书')
        argv = (admin.home / 'curl.argv').read_text()
        self.assertTrue('--pinnedpubkey' not in argv and ' -k ' not in argv, argv)
        self.assertEqual(json.loads((self.tdir(admin, tid) / 'team.json').read_text())['url'], '', '没接上：本机照旧')
        admin.env['CURL_CA_BUNDLE'] = str(d / 'ca.pem')
        self.assertEqual(self.lines(admin, 'team-attach', tid, url)[0], '127.0.0.1:%d' % srv.port)
        self.assertEqual(json.loads((self.tdir(admin, tid) / 'team.json').read_text())['pins'], [])
        self.assertEqual(self.lines(admin, 'team-leave', 'plan', tid), ['%s\t托管组\tadmin\t127.0.0.1:%d' % (tid, srv.port)],
                         '照证书机构验的（托管平台、自己配的反向代理）不是安装脚本装的：删除 Team 时不猜去哪卸载')
        link, _ = self.invite(admin, tid, 'bob-mini')
        bob = self.device('bob')
        self.assertEqual(self.code(bob, 'team-join', '--name', 'bob-mini', stdin=link), 'cert')
        self.assertEqual([p.name for p in (bob.cca / 'teams').glob('*') if p.is_dir()] if (bob.cca / 'teams').exists() else [], [], '什么都不留')
        bob.env['CURL_CA_BUNDLE'] = str(d / 'ca.pem')
        self.assertEqual(self.lines(bob, 'team-join', '--name', 'bob-mini', stdin=link)[5], 'ok')
        argv = (bob.home / 'curl.argv').read_text()
        self.assertTrue('--pinnedpubkey' not in argv and ' -k ' not in argv, argv)

    def test_delete_plan(self):
        """删除 Team（管理员的 team-leave）时去哪卸载它的同步服务器：cca team deploy 记下的照它（装好了没接上、再手动接上的也是），从接着的
        一台换到别的地址就不算了，照部署的默认猜；中间空着的项照样占一栏（bash 按 \\037 分）。apply --server-gone（刚卸掉了）不再告诉
        服务器。team-undeploy 只收 SSH 的几项和 --dir"""
        admin = self.device('admin')
        tid, root, _ = self.lines(admin, 'team-new', '机房组', '--device', 'studio')
        self.assertEqual(self.lines(admin, 'team-leave', 'plan', tid), ['%s\t机房组\tadmin\t' % tid], '没接上、没部署过：没有要卸的')
        p = self.tdir(admin, tid) / 'admin.json'
        a = json.loads(p.read_text())
        a['deploy'] = {'target': 'ops@vps', 'ssh_port': '', 'dir': '/srv/team'}   # cca team deploy 装好了、没接上
        p.write_text(json.dumps(a))
        self.assertEqual(self.lines(admin, 'team-leave', 'plan', tid), ['%s\t机房组\tadmin\t\tops@vps\t\t/srv/team\t1' % tid])
        key = self.d / 'vps_key'
        key.write_text('私钥\n')
        a['deploy']['identity'] = str(key)   # 部署时写了 -i（记下的是绝对路径）
        p.write_text(json.dumps(a))
        self.assertEqual(self.lines(admin, 'team-leave', 'plan', tid), ['%s\t机房组\tadmin\t\tops@vps\t\t/srv/team\t1\t%s' % (tid, key)],
                         '部署时写了私钥：最后一栏是它（App 里删除时照它登录）')
        key.unlink()
        self.assertEqual(self.lines(admin, 'team-leave', 'plan', tid), ['%s\t机房组\tadmin\t\tops@vps\t\t/srv/team\t1' % tid],
                         '私钥文件不在了：不给（照 ssh 的配置登录）')
        srv, url, pin = self.server(root)
        self.lines(admin, 'team-attach', tid, url, pin)
        self.assertEqual(self.lines(admin, 'team-leave', 'plan', tid),
                         ['%s\t机房组\tadmin\t127.0.0.1:%d\tops@vps\t\t/srv/team\t1' % (tid, srv.port)], '再手动接上：照旧是记下的那台')
        srv2, url2, pin2 = self.server(root, name='srv2')
        self.lines(admin, 'team-attach', tid, url2, pin2)
        self.assertEqual(self.lines(admin, 'team-leave', 'plan', tid),
                         ['%s\t机房组\tadmin\t127.0.0.1:%d\troot@127.0.0.1\t22\t/opt/cca-team-%s\t0' % (tid, srv2.port, tid[:8])],
                         '换到别的地址：记下的那台不算了，照部署的默认猜')
        self.assertNotIn('deploy', json.loads(p.read_text()))
        self.assertEqual(self.code(admin, 'team-undeploy', tid), 'usage', '要写服务器')
        for bad in (('--port', '18445'), ('--no-web',), ('--dir', '/etc/cca'), ('--ssh-port', '0')):
            self.assertEqual(self.code(admin, 'team-undeploy', tid, 'root@192.0.2.10', *bad), 'usage', bad)
        self.assertEqual(self.code(self.device('bob'), 'team-undeploy', tid, 'root@192.0.2.10'), 'no_team')
        srv2.stop()
        self.assertEqual(self.lines(admin, 'team-leave', 'apply', tid, '--server-gone'), ['机房组', '127.0.0.1:%d' % srv2.port, ''],
                         '服务器刚卸掉了：不再去告诉它（连不上也不说）')
        self.assertFalse(self.tdir(admin, tid).exists())

    def test_invite_for_a_key(self):
        """给一把指定的公钥的邀请（cca team invite --key：设备拿已经有的密钥加入，这里由照文档独立实现的那一份扮演）：拿这把密钥加入的
        照常；邀请被别的密钥用了，管理员对账时认出来、名单里说，发布时不再给它账号（加入时取走的那一份收不回来），授权照旧"""
        admin, tid, root, srv, url, pin = self.new_team()
        self.load(admin, tid)
        pubs = [keygen(self.d / ('key-%d' % i)) for i in (1, 2, 3)]
        self.assertEqual(self.code(admin, 'team-invite', tid, 'm1', '--key', '不是公钥'), 'usage')
        link1, d1 = self.invite(admin, tid, 'm1', '--key', pubs[0])
        link2, d2 = self.invite(admin, tid, 'm2', '--key', pubs[1])

        def join(link, n, name):
            """照文档拿 key-n 那把密钥加入：(设备编号, 它那份配置的两把密钥)"""
            k = sw.invite_decode(link)['k']
            token, dev, keys = sw.invite_parts(k)
            pub = pubs[n - 1]
            st, j = sw.post(str(self.d / ('key-%d' % n)), srv.port, 'team_join',
                            {'pub': pub, 'name': name, 'token': token, 'proof': sw.team_pub_proof(k, pub)})
            self.assertEqual(st, 200, j)
            return dev, keys

        def accounts(n, dev, keys):
            """key-n 那台设备现在取得到的配置里给了几个账号"""
            st, j = sw.post(str(self.d / ('key-%d' % n)), srv.port, 'team_get', {})
            self.assertEqual(st, 200, j)
            env = j['doc']
            return sum(len(v) for v in sw.team_doc_open(keys, tid, dev, env['seq'], env['ct'])['accounts'].values())

        m1, m2 = join(link1, 1, 'm1'), join(link2, 3, 'm2')   # m2 的邀请被别的机器（key-3）用了
        self.assertEqual((m1[0], m2[0]), (d1, d2))
        self.assertEqual((accounts(1, *m1), accounts(3, *m2)), (3, 3), '加入时取得到给这个邀请的那一份（全部账号）')
        rows = {r['label']: r for r in json.loads(admin.ok('team-devices', tid, '--json'))['devices']}
        self.assertEqual([(rows[x]['state'], rows[x]['grants']) for x in ('m1', 'm2')], [('joined', 'all'), ('joined', 'all')],
                         '授权没动（服务器一时说错、后来对上了就照旧）')
        self.assertIsNone(rows['m1']['problem'], '拿指定的那把密钥加入的：没问题')
        self.assertTrue(rows['m2']['problem'], '名单里说：拿这个邀请加入的不是指定的那把密钥')
        self.lines(admin, 'team-publish', tid)
        self.assertEqual((accounts(1, *m1), accounts(3, *m2)), (3, 0), '发布以后不再给它账号；拿对了密钥的照旧')

    def test_import_mine(self):
        """把这台设备上自己的官方账号加进 Team：别的 Team 给的（这台设备只是成员）、第三方 API 账号不算；节点、别名照这台设备上的"""
        admin = Device(self.d / 'admin', 'admin', {'me(me@example.com)': 'US-C', 'alice(alice@example.com)': 'US-A', 'plain@example.com': 'US-D',
                                                   'api(k@example.com)': {'node': 'US-B', 'base_url': 'https://api.example.com'}})
        other = self.device('other')   # 别人当管理员的 Team：给这台设备 alice
        tid2, root2, _ = self.lines(other, 'team-new', '别人的组', '--device', 'desk')
        srv2, url2, pin2 = self.server(root2, name='srv2')
        self.lines(other, 'team-attach', tid2, url2, pin2)
        self.lines(other, 'team-map', tid2, 'claude', 'alice@example.com', 'US-A')
        self.assertEqual(self.lines(admin, 'team-join', '--name', 'studio', stdin=self.invite(other, tid2, 'studio')[0])[5], 'ok')
        tid, root, _ = self.lines(admin, 'team-new', '我的设备', '--device', 'studio')
        self.assertEqual(self.lines(admin, 'team-import', tid, '--mine'), ['2', '0', '0'])
        a = json.loads((self.tdir(admin, tid) / 'admin.json').read_text())
        self.assertEqual((a['config']['accounts']['claude'], a['config']['aliases']),
                         ({'me@example.com': 'US-C', 'plain@example.com': 'US-D'}, {'me@example.com': 'me', 'plain@example.com': 'team-1'}),
                         '这台设备上起过的别名带进去，没起过的编成 team-1')
        # 接上服务器以后发布：管理员自己这台拿到的就是这两个账号
        srv, url, pin = self.server(root)
        self.lines(admin, 'team-attach', tid, url, pin)
        self.assertEqual(sorted(self.doc(admin, tid)[1]['accounts']['claude']), ['me@example.com', 'plain@example.com'])
        self.assertEqual(self.code(admin, 'team-import', tid, '--mine', 'x'), 'usage')

    def test_new_with_accounts(self):
        """新建 Team 时放进这台设备上的账号（team-candidates 列出来、team-new --accounts 选）：自己加的，和这台设备当管理员的 Team 里的
        （那个 Team 带着节点的定义的，连同经 dialer-proxy 连着的一起带过来）；这台设备只是成员的 Team 给的、第三方 API 账号不能放。
        节点、别名照这台设备上的（默认样子的 team-N 另编）；写错了什么都不建"""
        admin, tid, root, srv, url, pin = self.new_team()   # 「测试组」：这台设备是它的管理员
        self.load(admin, tid)   # alice → US-A、bob → US-B（经 US-C 连出去）、Codex 的 carol → US-A，节点的定义都在里面
        other = self.device('other')   # 别人当管理员的 Team：这台设备只是成员，它给的 frank 不能转手
        tid2, root2, _ = self.lines(other, 'team-new', '别人的组', '--device', 'desk')
        srv2, url2, pin2 = self.server(root2, name='srv2')
        self.lines(other, 'team-attach', tid2, url2, pin2)
        self.lines(other, 'team-map', tid2, 'claude', 'frank@example.com', 'US-E')
        self.assertEqual(self.lines(admin, 'team-join', '--name', 'studio-2', stdin=self.invite(other, tid2, 'studio-2')[0])[5], 'ok')
        admin.ok('team-sync')   # 配置写好以后这台设备自己的那一份也同步下来：alice、bob、carol 是「测试组」给的
        (admin.cca / 'accounts.json').write_text(json.dumps({
            'me(me@example.com)': 'US-C', 'team-7(old@example.com)': 'US-D', 'alice(alice@example.com)': 'US-A', 'team-1(bob@example.com)': 'US-B',
            'frank@example.com': 'US-E', 'api(k@example.com)': {'node': 'US-B', 'base_url': 'https://api.example.com'}}))
        (admin.cca / 'codex.json').write_text(json.dumps({'carol@example.com': 'US-A', 'cx(cx@example.com)': 'US-B'}))
        self.assertEqual(self.lines(admin, 'team-candidates'), [
            'claude\tClaude\tme@example.com\tme\t-\tUS-C', 'claude\tClaude\told@example.com\tteam-7\t-\tUS-D',
            'claude\tClaude\talice@example.com\talice\t测试组\tUS-A', 'claude\tClaude\tbob@example.com\tteam-1\t测试组\tUS-B',
            'codex\tCodex\tcarol@example.com\t-\t测试组\tUS-A', 'codex\tCodex\tcx@example.com\tcx\t-\tUS-B'])
        for spec, code in (('claude:frank@example.com', 'accounts'), ('claude:k@example.com', 'accounts'),
                           ('claude:me@example.com,claude:nobody@example.com', 'accounts'), ('me@example.com', 'usage')):
            self.assertEqual(self.code(admin, 'team-new', '新组', '--device', 'studio', '--accounts', spec), code, spec)
        self.assertEqual(len(self.lines(admin, 'team-list')), 2, '写错了什么都不建')
        # 照账号表里的先后放（不照 --accounts 写的先后）；别名带进去，默认样子的另编
        tid3, root3, n = self.lines(admin, 'team-new', '新组', '--device', 'studio',
                                    '--accounts', 'codex:carol@example.com,claude:bob@example.com,claude:me@example.com,claude:old@example.com')
        config = json.loads((self.tdir(admin, tid3) / 'admin.json').read_text())['config']
        self.assertEqual((n, config['accounts'], config['tool_aliases']),
                         ('4', {'claude': {'me@example.com': 'US-C', 'old@example.com': 'US-D', 'bob@example.com': 'US-B'}, 'codex': {'carol@example.com': 'US-A'}},
                          {'claude': {'me@example.com': 'me', 'old@example.com': 'team-1', 'bob@example.com': 'team-2'}, 'codex': {'carol@example.com': 'team-1'}}))
        self.assertEqual((config['nodes']['group'], [p['name'] for p in config['nodes']['proxies']]), ('OpenAi', ['US-B', 'US-C', 'US-A']),
                         '「测试组」带着的定义带过来（US-B 经 US-C 连出去）；自己加的账号的节点各台设备自己有，不带')
        # 接上服务器、发布：这台设备拿到的配置里就是这几个账号和要用的节点
        srv3, url3, pin3 = self.server(root3, name='srv3')
        self.lines(admin, 'team-attach', tid3, url3, pin3)
        doc = self.doc(admin, tid3)[1]
        self.assertEqual((sorted(doc['accounts']['claude']), sorted(p['name'] for p in doc['nodes']['proxies'])),
                         (['bob@example.com', 'me@example.com', 'old@example.com'], ['US-A', 'US-B', 'US-C']))
        # all 是列出来的全部
        self.assertEqual(self.lines(admin, 'team-new', '全都放', '--device', 'studio', '--accounts', 'all')[2], '6')

    def test_server_lies(self):
        """服务器说的没有签名，管理员只拿它当线索：换了的公钥不照它的；它那边的配置丢了、旧了就重新发布；它乱说序号不信"""
        admin, tid, root, srv, url, pin = self.new_team()
        self.load(admin, tid)
        link, dev_b = self.invite(admin, tid, 'bob-mini')
        bob = self.device('bob')
        self.assertEqual(self.lines(bob, 'team-join', '--name', 'bob-mini', stdin=link)[5], 'ok')
        self.assertEqual(self.lines(admin, 'team-publish', tid), ['0', ''])
        aj = lambda: json.loads((self.tdir(admin, tid) / 'admin.json').read_text())
        real = aj()['devices'][dev_b]['pub']
        # 服务器把 bob 的公钥换成别人的（停下来改它的状态文件），把给它的配置删了
        port = srv.port
        srv.stop()
        evil = keygen(self.d / 'evil')
        state = json.loads((self.d / 'srv' / 'state.json').read_text())
        fp = next(f for f, m in state['machines'].items() if m['dev'] == dev_b)
        state['machines'][sw.fp_of(str(self.d / 'evil.pub'))] = dict(state['machines'].pop(fp), key=evil)
        (self.d / 'srv' / 'state.json').write_text(json.dumps(state))
        docs = json.loads((self.d / 'srv' / 'team-docs.json').read_text())
        del docs['docs'][dev_b]
        (self.d / 'srv' / 'team-docs.json').write_text(json.dumps(docs))
        srv, _, _ = self.server(root, port=port)
        row = next(r for r in json.loads(admin.ok('team-devices', tid, '--json'))['devices'] if r['label'] == 'bob-mini')
        self.assertIn('公钥', row['problem'])
        self.assertEqual(aj()['devices'][dev_b]['pub'], real, '记下的公钥不照服务器的改')
        self.assertEqual(self.lines(admin, 'team-publish', tid), ['1', ''], '服务器上 bob 的那一份没了：重新发布')
        seq = aj()['devices'][dev_b]['seq']
        # 服务器乱说它存着的序号（假 curl 照 reply/team_put 回话）：比记着的小、大得离谱的都不信，记着的序号不动
        (admin.home / 'reply').mkdir()
        # （每次换一种授权：内容和上次发布成的那一版不一样才会去存）
        grants = ('none', '+claude:alice@example.com', '+claude:bob@example.com', '-claude:alice@example.com', '+codex:carol@example.com')
        for lie, grant in zip((0, seq - 1, 2 ** 53 - 1, 2 ** 63 - 1, 'x'), grants):
            (admin.home / 'reply' / 'team_put').write_text('200\n' + json.dumps({'v': 1, 'ok': True, 'stored': [], 'stale': {dev_b: lie}, 'unknown': []}))
            r = admin.run('team-grant', tid, 'bob-mini', grant)
            self.assertEqual((r.returncode, r.stdout.split('\t')[0]), (1, 'publish'), (lie, r.stdout))
            self.assertIn('对不上', r.stdout)
            self.assertEqual((aj()['devices'][dev_b]['seq'], aj()['devices'][dev_b]['state']), (seq, 'joined'), lie)
        # 回话里哪一种都没说这台设备的：不当成它不在了
        (admin.home / 'reply' / 'team_put').write_text('200\n' + json.dumps({'v': 1, 'ok': True, 'stored': [], 'stale': {}, 'unknown': []}))
        r = admin.run('team-publish', tid)
        self.assertEqual((r.returncode, aj()['devices'][dev_b]['state']), (1, 'joined'), r.stdout)
        self.assertIn('没说', r.stdout)
        # 说得过去的（比记着的大一些：管理员的状态是从备份恢复的）：接着它的往上加，再存一次
        (admin.home / 'reply' / 'team_put').write_text('200\n' + json.dumps({'v': 1, 'ok': True, 'stored': [], 'stale': {dev_b: seq + 5}, 'unknown': []}))
        r = admin.run('team-publish', tid)
        self.assertEqual(r.returncode, 1, r.stdout)
        self.assertEqual(aj()['devices'][dev_b]['seq'], seq + 5, '第二次它还说不够大：不再追，停在它说的那个数')
        (admin.home / 'reply' / 'team_put').unlink()
        self.assertEqual(self.lines(admin, 'team-publish', tid), ['1', ''])
        self.assertEqual(aj()['devices'][dev_b]['seq'], seq + 6)
        # 发布时服务器说不认得（对过名单以后它才走的）：加入过的不再记，邀请了还没加入的记成 expired
        link_c, dev_c = self.invite(admin, tid, 'carol')
        a = aj()
        for dev in (dev_b, dev_c):
            a['devices'][dev].pop('sha', None)   # 都重新发布一次
        (self.tdir(admin, tid) / 'admin.json').write_text(json.dumps(a))
        (admin.home / 'reply' / 'team_put').write_text('200\n' + json.dumps({'v': 1, 'ok': True, 'stored': [], 'stale': {}, 'unknown': [dev_b, dev_c]}))
        r = admin.run('team-publish', tid)
        self.assertEqual((r.returncode, dev_b in aj()['devices'], aj()['devices'][dev_c]['state']), (1, False, 'expired'), r.stdout)
        self.assertIn('服务器不认得', r.stdout)
        (admin.home / 'reply' / 'team_put').unlink()
        # 服务器的名单里没有管理员自己这台（gen 没变；正常不会这样）：不当它退出了，这边照旧记着
        me = json.loads((self.tdir(admin, tid) / 'team.json').read_text())['dev']
        port = srv.port
        srv.stop()
        state = json.loads((self.d / 'srv' / 'state.json').read_text())
        state['machines'] = {f: m for f, m in state['machines'].items() if m['dev'] != me}
        (self.d / 'srv' / 'state.json').write_text(json.dumps(state))
        srv, _, _ = self.server(root, port=port)
        rows = {r['dev']: r for r in json.loads(admin.ok('team-devices', tid, '--json'))['devices']}
        self.assertTrue(me in aj()['devices'] and rows[me]['me'], rows)


if __name__ == '__main__':
    unittest.main()
