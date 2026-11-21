#!/usr/bin/env python3
"""Team（cca team）：辅助程序（cca-core）这一头的建 Team、接上服务器、邀请、加入、取配置、授权、移除、退出，对着真的服务器
（server/cca-presence.py --mode team，127.0.0.1、随机端口）。各台设备各一个临时 HOME（tests/presence.py 的 Device）。
另一头有时由 tests/spec_wire.py（照 server/README.md 独立实现的那一份）扮演：两份实现封的、签的要能互相解开、验过。不联网。"""
import hashlib
import json
import os
from pathlib import Path
import secrets
import stat
import sys
import tempfile
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from presence import Device, Server, ident, keygen, make_cert, openssl_pin   # noqa: E402
import spec_wire as sw   # noqa: E402

POLICY = {'version': 1, 'claude': {'alice@example.com': 'US-A', 'Bob@Example.com': 'US-B'}, 'codex': {'carol@example.com': 'US-A'},
          'aliases': {'alice@example.com': 'alice'}, 'install_node': 'US-D'}
NODE = lambda name, **more: dict({'name': name, 'type': 'socks5', 'server': '192.0.2.10', 'port': 1080 + ord(name[-1])}, **more)
NODES = {'version': 1, 'group': 'OpenAi', 'proxies': [NODE('US-A'), NODE('US-B', **{'dialer-proxy': 'US-C'}), NODE('US-C'), NODE('US-D'), NODE('US-E')]}


class TeamTest(unittest.TestCase):
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
        tid, root = self.lines(admin, 'team-new', name, '--device', device)
        srv, url, pin = self.server(root)
        self.assertEqual(self.lines(admin, 'team-attach', tid, url, pin), ['127.0.0.1:%d' % srv.port, device, '1', '1', ''])
        return admin, tid, root, srv, url, pin

    def load(self, admin, tid):
        """导入现成的映射和参考节点"""
        (self.d / 'policy.json').write_text(json.dumps(POLICY))
        (self.d / 'nodes.json').write_text(json.dumps(NODES))
        return self.lines(admin, 'team-import', tid, str(self.d / 'policy.json'), str(self.d / 'nodes.json'))

    def invite(self, admin, tid, label, *more):
        link, dev, problem = self.lines(admin, 'team-invite', tid, label, *more)
        self.assertEqual(problem, '')
        return link, dev

    def test_admin_local(self):
        """建 Team：只在本机（还没有服务器）；文件都只给自己读写；接不上的各种原因"""
        admin = self.device('admin')
        self.assertEqual(self.code(admin, 'team-new', ''), 'name')
        self.assertEqual(self.lines(admin, 'team-list'), [])
        tid, root = self.lines(admin, 'team-new', '测试组', '--device', 'studio')
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
        # 还没接上服务器：发不了邀请；导入、改映射只改本机的
        self.assertEqual(self.code(admin, 'team-invite', tid, 'bob'), 'not_attached')
        self.assertEqual(self.load(admin, tid), ['3', '5', '0'])
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
        (self.d / 'own').mkdir()
        make_cert(self.d / 'own')
        own = Server(self.d / 'own', admit_hash='0' * 64)
        self.addCleanup(own.stop)
        self.assertEqual(self.code(admin, 'team-attach', tid, 'https://127.0.0.1:%d' % own.port, openssl_pin(self.d / 'own' / 'cert.pem')), 'old')
        self.assertEqual(json.loads((t / 'team.json').read_text())['url'], '', '没接上：本机照旧')

    def test_lifecycle(self):
        admin, tid, root, srv, url, pin = self.new_team()
        t = self.tdir(admin, tid)
        self.assertEqual(self.doc(admin, tid)[0], 1, '接上以后管理员自己这台也取到了配置')
        self.assertEqual(self.load(admin, tid), ['3', '5', '1'])
        self.assertEqual(self.lines(admin, 'team-sync'), ['%s\t测试组\tok\t2\t' % tid])
        seq, doc = self.doc(admin, tid)
        self.assertEqual((doc['accounts'], doc['aliases'], doc['install_node'], doc['name'], doc['server']),
                         ({'claude': {'alice@example.com': 'US-A', 'bob@example.com': 'US-B'}, 'codex': {'carol@example.com': 'US-A'}},
                          {'alice@example.com': 'alice'}, 'US-D', '测试组', {'u': url, 'p': [pin]}))
        self.assertEqual([p['name'] for p in doc['nodes']['proxies']], ['US-A', 'US-B', 'US-C', 'US-D'], '用得着的节点（连同 dialer-proxy 连着的）')
        # 邀请：链接只在 stdout 里出现一次；那串随机数、令牌不落盘、不进命令行
        self.assertEqual(self.code(admin, 'team-invite', tid, 'bob', '--accounts', 'nope'), 'usage')
        self.assertEqual(self.code(admin, 'team-invite', tid, 'bob', '--ttl', '5'), 'usage')
        link, dev_b = self.invite(admin, tid, 'bob-mini', '--accounts', 'claude:Alice@example.com')
        inv = sw.invite_decode(link)
        token, dev, keys = sw.invite_parts(inv['k'])
        self.assertEqual((inv['u'], inv['p'], 'ssh-ed25519 ' + inv['r'], inv['n'], dev, 's' in inv), (url, [pin], root, '测试组', dev_b, False))
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
        got = self.lines(bob, 'team-join', '--name', 'bob-mini', stdin=link)
        self.assertEqual(got, ['测试组', '127.0.0.1:%d' % srv.port, 'bob-mini', '2', tid, 'ok', '1'])
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
        self.assertEqual(admin.ok('team-devices', tid).split('\n')[3].split('\t')[2:], ['joined', '0 个账号', '1', '-', row['problem']])
        self.assertEqual(self.lines(admin, 'team-revoke', 'apply', tid, 'no-proof'), ['no-proof'])
        # 改授权、改映射：发布以后设备取到新的一版
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
        # 移除：设备取配置时知道被移除了；拿新的邀请再加入
        self.assertEqual(self.code(admin, 'team-revoke', 'plan', tid, 'studio'), 'self')
        self.assertEqual(self.lines(admin, 'team-revoke', 'plan', tid, 'bob-mini'), ['%s\t%s\tbob-mini\tjoined' % (tid, dev_b)])
        self.assertEqual(self.lines(admin, 'team-revoke', 'apply', tid, 'bob-mini'), ['bob-mini'])
        line = self.lines(bob, 'team-sync')[0].split('\t')
        self.assertEqual(line[:4], [tid, '测试组', 'removed', '5'])
        self.assertIn('移除', line[4])
        self.assertEqual(json.loads(bob.ok('team-list', '--json'))['teams'][0]['state'], 'removed')
        link3, dev_b3 = self.invite(admin, tid, 'bob-again')
        self.assertEqual(self.lines(bob, 'team-join', '--name', 'bob-mini', stdin=link3)[5:], ['ok', '2'], '被移除了的拿新的邀请直接加入（以前记着的不要了）')
        self.assertEqual(self.doc(bob, tid)[0], 1)
        rows = {r['label']: r['state'] for r in json.loads(admin.ok('team-devices', tid, '--json'))['devices']}
        self.assertEqual((rows['bob-again'], rows['bob-mini'], rows['no-proof']), ('joined', 'revoked', 'revoked'), '管理员和服务器对过一遍：知道它加入了')
        # 退出：服务器上没有它了；管理员那边记成自己退出的，不登记回去
        self.assertEqual(self.lines(bob, 'team-leave', 'plan'), ['%s\t测试组\tmember\t127.0.0.1:%d' % (tid, srv.port)])
        self.assertEqual(self.code(bob, 'team-leave', 'apply', tid[:8]), 'no_team', '真退出要写完整的编号')
        self.assertEqual(self.lines(bob, 'team-leave', 'apply', tid), ['测试组', '127.0.0.1:%d' % srv.port, ''])
        self.assertFalse(tb.exists())
        self.assertEqual(json.loads((bob.cca / 'teams' / 'index.json').read_text())['order'], [])
        self.assertEqual(self.code(bob, 'team-sync', tid), 'no_team')
        rows = {r['label']: r['state'] for r in json.loads(admin.ok('team-devices', tid, '--json'))['devices']}
        self.assertEqual(rows, {'studio': 'joined', 'bob-mini': 'revoked', 'spec': 'joined', 'no-proof': 'revoked', 'bob-again': 'left'})
        self.assertEqual(self.lines(admin, 'team-leave', 'plan', '测试组'), ['%s\t测试组\tadmin\t127.0.0.1:%d' % (tid, srv.port)])
        self.assertTrue(t.exists())
        self.assertEqual(self.lines(admin, 'team-leave', 'apply', tid)[:2], ['测试组', '127.0.0.1:%d' % srv.port])
        self.assertFalse(t.exists())

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
        self.assertEqual(json.loads((self.tdir(bob, tid) / 'team.json').read_text())['url'], url, '还没取过：还连着原来那台')
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
        同一个账号维护者的映射里也有时 Team 的算数，退出以后轮到维护者的映射"""
        admin, tid, root, srv, url, pin = self.new_team()
        self.load(admin, tid)
        link, dev_b = self.invite(admin, tid, 'bob-mini', '--accounts', 'claude:alice@example.com,claude:bob@example.com')
        bob = Device(self.d / 'bob', 'bob', {'mine(bob@example.com)': 'US-D'})
        # 这台设备上维护者的共享也开着，映射里 alice 走 US-C
        bob.config(lambda c: c.update(nodes=['US-A', 'US-B', 'US-C', 'US-D']))
        (bob.cca / 'policy' / 'policy.json').write_text(json.dumps({'version': 1, 'claude': {'alice@example.com': 'US-C'}}))
        table = lambda: json.loads((bob.cca / 'accounts.json').read_text())

        def apply():
            r = bob.run('team-apply')
            self.assertEqual((r.returncode, r.stdout), (0, ''), r.stderr)
            return r.stderr

        self.assertIn('按维护者的映射添加了 Claude 账号 alice@example.com，出口 US-C', apply())
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
        self.assertIn('按「测试组」的配置添加了 Claude 账号 erin@example.com，出口 US-D', apply())
        self.assertEqual(table()['erin@example.com'], 'US-D')
        self.assertEqual(cfg()['team_added'], {ident('erin@example.com'): tid})
        # 手里那一份配置一时读不出来（文件坏了）：说不准还给不给，不把 Team 加的账号收起来
        good = (self.tdir(bob, tid) / 'doc.json').read_text()
        (self.tdir(bob, tid) / 'doc.json').write_text('{')
        self.assertNotIn('收起来', apply())
        self.assertIn('erin@example.com', table())
        (self.tdir(bob, tid) / 'doc.json').write_text(good)
        apply()
        self.assertEqual((table()['erin@example.com'], table()['alice(alice@example.com)']), ('US-D', 'US-A'))
        # 不再给 erin：收起来（登录留着），再给回来照原来的键
        self.assertEqual(self.lines(admin, 'team-grant', tid, 'bob-mini', '-claude:erin@example.com'), ['2 个账号'])
        bob.ok('team-sync')
        self.assertIn('Team 不再给这台设备 Claude 账号 erin@example.com：收起来了', apply())
        self.assertNotIn('erin@example.com', table())
        self.assertEqual((cfg()['parked'], 'team_added' in cfg()),
                         ({ident('erin@example.com'): {'key': 'erin@example.com', 'node': 'US-D', 'team': tid}}, False))
        self.assertEqual(self.lines(admin, 'team-grant', tid, 'bob-mini', '+claude:erin@example.com'), ['3 个账号'])
        bob.ok('team-sync')
        self.assertIn('添加了 Claude 账号 erin@example.com', apply())
        self.assertEqual(('erin@example.com' in table(), 'parked' in cfg()), (True, False))
        # 不再给 bob@example.com：它是这台机器自己的账号，留着（节点不再被固定）
        self.assertEqual(self.lines(admin, 'team-grant', tid, 'bob-mini', '-claude:bob@example.com'), ['2 个账号'])
        bob.ok('team-sync')
        self.assertEqual(apply(), '')
        self.assertEqual(table()['mine(bob@example.com)'], 'US-B')
        # 被管理员移除了：手里那一份不再算数。Team 加的 erin 收起来；alice 轮到维护者的映射（US-C）
        self.assertEqual(self.lines(admin, 'team-revoke', 'apply', tid, 'bob-mini'), ['bob-mini'])
        self.assertEqual(self.lines(bob, 'team-sync')[0].split('\t')[2], 'removed')
        said = apply()
        self.assertIn('Team 不再给这台设备 Claude 账号 erin@example.com：收起来了', said)
        self.assertIn('alice（alice@example.com） 的出口按维护者的映射固定为 US-C（账号表里写的是 US-A，已改回）', said)
        self.assertEqual(table(), {'mine(bob@example.com)': 'US-B', 'alice(alice@example.com)': 'US-C'})

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


if __name__ == '__main__':
    unittest.main()
