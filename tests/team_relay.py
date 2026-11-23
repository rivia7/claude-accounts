#!/usr/bin/env python3
"""每个 Team 的会话走这个 Team 自己的服务器中转（docs/dev/remote.md「每个 Team 各用各的中转服务器」）。

两台真的服务器进程，各是一个 Team 的同步服务器、都开着中转：「研发组」的（管理员是一台真的 cca：建 Team、接上服务器、发邀请，设备拿
邀请加入）和「测试组」的（tests/remote_relay.py 的 Base 起的：设备直接写好加入了的样子；它的数据端口前面隔着转发器，经过的字节都记下来）。
两台设备先加入研发组、再加入测试组，跑的是 cca 的辅助程序。box 上三个会话：研发组给的账号的（alice）、这台设备自己的账号的（dave：
走这台机器的「家」，加入的第一个 Team 的那一台）、测试组给的账号的（bob：走测试组的那一台）。看：配对记在哪台服务器上、经它看得到的
是走那台服务器的会话；没说在哪台上配对时在家里那一台上配；输入配对码的那一头自己找得到是哪台。不联网。"""
import json
from pathlib import Path
import subprocess
import sys
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from presence import SERVER, Device, make_cert, openssl_pin, wait_for   # noqa: E402
import remote_relay as rr   # noqa: E402

TEAM_KEY, OWN_KEY, OTHER_KEY = 'alice(alice@example.com)', 'mine(dave@example.com)', 'other(bob@example.com)'


class TeamServer:
    """「研发组」的同步服务器（自己的目录、证书），开着中转"""
    def __init__(self, d, root):
        d.mkdir()
        make_cert(d)
        self.d = d
        args = [sys.executable, '-I', str(SERVER), '--listen', '127.0.0.1:0', '--port-file', str(d / 'port'), '--cert', str(d / 'cert.pem'),
                '--key', str(d / 'key.pem'), '--team-root', root, '--state', str(d / 'state.json'), '--ttl', '60',
                '--idle', '600', '--relay-listen', '127.0.0.1:0', '--relay-port-file', str(d / 'rport')]
        self.proc = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            self.port = int(wait_for('Team 的服务器起来', lambda: (d / 'port').exists() and (d / 'port').read_text()))
            wait_for('它的数据端口起来', lambda: (d / 'rport').exists() and (d / 'rport').read_text())
        except BaseException:
            self.stop()
            raise
        self.url, self.pin = 'https://127.0.0.1:%d' % self.port, openssl_pin(d / 'cert.pem')

    def edges(self):
        p = self.d / 'remote.json'
        return json.loads(p.read_text())['edges'] if p.exists() else {}

    def stop(self):
        if self.proc.poll() is None:
            self.proc.kill()
        self.proc.wait()
        if not self.proc.stderr.closed:
            self.proc.stderr.close()


class TeamRelayTest(rr.Base):
    def lines(self, dev, *argv, **kw):
        return dev.ok(*argv, **kw).split('\n')[:-1]

    def member(self, name, admin, tid):
        """一台设备：先拿邀请加入研发组（它给 alice），再加入测试组（它给 bob）；账号表里还有一个这台设备自己的（dave）"""
        dev = self.device(name, join=False)
        table = json.loads((dev.cca / 'accounts.json').read_text())
        table[OWN_KEY], table[OTHER_KEY] = 'US-B', 'US-A'
        (dev.cca / 'accounts.json').write_text(json.dumps(table))
        dev.config(lambda c: c.update(nodes=['US-A', 'US-B']))
        link, _, problem = self.lines(admin, 'team-invite', tid, name, '--accounts', 'claude:alice@example.com')
        self.assertEqual(problem, '')
        self.assertEqual(self.lines(dev, 'team-join', '--name', name, stdin=link)[5:], ['ok', '1'])
        dev.join(self.team, self.url(), [self.pin], doc=self.team.doc(self.url(), [self.pin], accounts={'claude': {'bob@example.com': 'US-A'}}))
        r = dev.run('team-apply')
        self.assertEqual((r.returncode, r.stdout), (0, ''), r.stderr)
        self.assertEqual(sorted(json.loads((dev.cca / 'accounts.json').read_text())), sorted([TEAM_KEY, OWN_KEY, OTHER_KEY]))
        return dev

    def setUp(self):
        super().setUp()
        admin = self.admin = Device(self.d / 'admin', 'admin', {})
        self.tid, root, _ = self.lines(admin, 'team-new', '研发组', '--device', 'studio')
        self.rsrv = TeamServer(self.d / 'team-srv', root)
        self.addCleanup(self.rsrv.stop)
        self.lines(admin, 'team-attach', self.tid, self.rsrv.url, self.rsrv.pin)
        self.lines(admin, 'team-map', self.tid, 'claude', 'alice@example.com', 'US-A')
        self.lines(admin, 'team-alias', self.tid, 'alice@example.com', 'alice')
        self.a, self.b = self.member('box', admin, self.tid), self.member('laptop', admin, self.tid)
        self.machines(self.a, self.b)   # 登记到测试组的服务器上

    def session(self, dev, key, name):
        h = rr.Term([rr.HELPER, 'remote-host', 'claude', key, name, '--', sys.executable, str(dev.home / 'client.py'), name], dev.env, str(dev.work))
        self.terms.append(h)
        h.expect('<ready 24 80')
        return h

    def pair(self, shower, joiner, *hub, join_hub=()):
        """shower 显示配对码（hub：--hub 服务器），joiner 输入。返回这次配对在 joiner 的配对表里的编号"""
        before = set(rr.ClientTest.peers(joiner))
        p = subprocess.Popen([rr.HELPER, 'remote-pair-show', *hub], env=shower.env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, text=True)
        try:
            line = p.stdout.readline()
            self.assertRegex(line, r'^code ', p.stderr.read() if not line else '')
            r = joiner.run('remote-pair-join', *join_hub, stdin=line.split()[1] + '\n')
            self.assertEqual((r.returncode, r.stderr), (0, ''), r.stdout)
            out, err = p.communicate(timeout=60)
        finally:
            p.kill()
            p.stdout.close()
            p.stderr.close()
        self.assertEqual((p.returncode, err), (0, ''), out)
        (pid,) = set(rr.ClientTest.peers(joiner)) - before
        return pid

    def far(self, dev):
        """dev 看到的别的机器上的会话（remote list --json）；两次之间隔开一点（服务器按设备限流）"""
        time.sleep(1.1)
        r = dev.run('remote-list', '--far', '--json')
        self.assertEqual(r.returncode, 0, r.stderr)
        return json.loads(r.stdout)

    def test_each_teams_sessions_go_through_its_own_server(self):
        a, b, tid, other = self.a, self.b, self.tid, self.team.id
        # 用得了两台：没说在哪台上配对时用的那一台（家）是先加入的那个 Team 的
        self.assertEqual(self.lines(a, 'remote-hubs'), ['%s\t「研发组」的服务器\thome' % tid, '%s\t「测试组」的服务器' % other])
        ha, ho, hm = self.session(a, TEAM_KEY, 'alice'), self.session(a, OWN_KEY, 'mine'), self.session(a, OTHER_KEY, 'other')
        self.assertEqual(a.run('remote-pair-show', '--hub', '没有这个组').returncode, 1)
        self.assertEqual((self.rsrv.edges(), self.edges()), ({}, {}))

        # 没说在哪台上配对：在家里那一台（研发组的服务器）上配，记在它上面，测试组的那一台不知道；输入配对码的那一头没说在哪台，自己找到了
        team_pid = self.pair(a, b)
        self.assertEqual((list(self.rsrv.edges()), self.edges()), ([team_pid], {}))
        for dev, shown in ((a, True), (b, False)):
            row = rr.ClientTest.peers(dev)[team_pid]
            self.assertEqual((row['hub'], row['shown']), (tid, shown))
        self.assertIn('经「研发组」的服务器', a.ok('remote-peers'))
        # 经它看得到的：研发组给的账号的会话，和这台设备自己的账号的会话（走家里那一台）；测试组给的账号的会话走测试组的那一台，
        # 那边还没配对：看不到
        doc = wait_for('laptop 看到会话', lambda: (lambda d: d if len(d['sessions']) == 2 else None)(self.far(b)))
        self.assertEqual(sorted((s['name'], s['peer']) for s in doc['sessions']), [('alice', team_pid), ('mine', team_pid)])
        self.assertEqual([(p['id'], p['team'], p['state']) for p in doc['peers']], [(team_pid, tid, 'ok')])
        self.assertEqual(doc['relay']['hubs'], [{'team': tid, 'name': '「研发组」的服务器'}, {'team': other, 'name': '「测试组」的服务器'}])
        self.assertEqual(doc['errors'], [])
        # 在研发组的服务器上配对，名字、编号都认
        self.assertEqual(len(self.rsrv.edges()), 1)

        # 再在测试组的那一台上配一次（同两台设备，各记各的、各有一把密钥）：这才看得到测试组给的那个账号的会话，走的是测试组的服务器
        other_pid = self.pair(a, b, '--hub', '测试组', join_hub=('--hub', other))
        self.assertEqual((list(self.rsrv.edges()), list(self.edges())), ([team_pid], [other_pid]))
        self.assertEqual(rr.ClientTest.peers(b)[other_pid]['hub'], other, '配对表里记着是哪个 Team 的服务器')
        self.assertNotEqual(rr.ClientTest.secret(b, team_pid), rr.ClientTest.secret(b, other_pid))
        doc = wait_for('laptop 看到三个会话', lambda: (lambda d: d if len(d['sessions']) == 3 else None)(self.far(b)))
        self.assertEqual(sorted((s['name'], s['peer']) for s in doc['sessions']), [('alice', team_pid), ('mine', team_pid), ('other', other_pid)])
        self.assertEqual(sorted((p['id'], p['team']) for p in doc['peers']), sorted([(team_pid, tid), (other_pid, other)]))

        # 接入研发组的那个会话：经研发组的服务器握手、收发（测试组那一台的转发器上一个字节都没有经过它的内容）
        sid = next(s['id'] for s in doc['sessions'] if s['name'] == 'alice')
        seen = len(self.sniff.snapshot())
        t = rr.Term([rr.HELPER, 'remote-attach', sid[:10]], b.env, str(b.work))
        self.terms.append(t)
        t.expect('<ready 24 80')
        t.send('say via the team server\r')
        t.expect('<via the team server>')
        ha.expect('<via the team server>')
        self.assertNotIn(b'via the team server', ho.out + hm.out)
        self.assertEqual(len(self.sniff.snapshot()), seen, '这个会话的数据不经测试组的服务器')

        # 取消研发组上的那个配对：告诉的是研发组的服务器，测试组那一台上的照旧
        self.assertEqual(self.lines(b, 'remote-unpair', team_pid[:8]), ['box'])
        self.assertEqual((self.rsrv.edges(), list(self.edges())), ({}, [other_pid]))
        doc = wait_for('只剩测试组那一台上的', lambda: (lambda d: d if [s['name'] for s in d['sessions']] == ['other'] else None)(self.far(b)))
        self.assertEqual([(p['id'], p['team']) for p in doc['peers']], [(other_pid, other)])


class ShellTest(TeamRelayTest):
    """bash 这一层（真的 cca 脚本）：cca remote pair --team 名字，cca remote join 自己找到是哪台服务器"""
    env, answers, cca = rr.ShellTest.env, rr.ShellTest.answers, rr.ShellTest.cca
    test_each_teams_sessions_go_through_its_own_server = None   # 上面那一类的用例不在这里再跑一遍

    def shown(self, dev, *argv):
        """dev 上 cca remote pair 参数…（同意）：返回 (进程, 配对码, 到配对码为止 stderr 上说的)"""
        p = subprocess.Popen(['/bin/bash', str(rr.ROOT / 'cca'), 'remote', 'pair', *argv], env=self.env(dev, CCA_TTY=self.answers('y')),
                             stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.addCleanup(lambda: (p.poll() is None and p.kill(), p.wait(), p.stdout.close(), p.stderr.close()))
        code, seen = None, ''
        while code is None:
            line = p.stderr.readline()
            self.assertTrue(line, '没等到配对码：' + seen)
            seen += line
            if '配对码：' in line:
                code = line.strip().split('：')[1]
        return p, code, seen

    def test_terminal(self):
        a, b, tid = self.a, self.b, self.tid
        self.n = 0
        for bad in (('--team',), ('--nope',)):
            rc, out, err = self.cca(a, 'remote', 'pair', *bad)
            self.assertEqual((rc, out), (1, ''), bad)
            self.assertIn('用法：cca remote pair [--web] [--team 名字', err)
        self.assertEqual(self.cca(b, 'remote', 'join', '--web')[0], 1)
        # 用得了两台、没说在哪台上：在家里那一台（先加入的研发组的）上配，问之前说清楚记在哪台上、对方看得到的是哪些会话、
        # 要在别的上面配怎么写；另一台输入时不用说在哪台
        p, code, seen = self.shown(a)
        self.assertIn('这次配对记在「研发组」的服务器上：对方经它看得到的，是走这台服务器的会话', seen)
        self.assertIn('要在别的 Team 的服务器上配对：--team 名字', seen)
        rc, out, err = self.cca(b, 'remote', 'join', answers=[code])
        self.assertEqual((rc, out), (0, ''), err)
        self.assertIn('已经和「box」配对', err)
        self.assertEqual(p.wait(60), 0, p.stderr.read())
        (team_pid,) = self.rsrv.edges()
        self.assertEqual((rr.ClientTest.peers(b)[team_pid]['hub'], self.edges()), (tid, {}))
        # 写明了在研发组的服务器上（名字）：不再说要在别的上面配怎么写
        p, code, seen = self.shown(a, '--team', '研发组')
        self.assertIn('这次配对记在「研发组」的服务器上', seen)
        self.assertNotIn('要在别的 Team 的服务器上配对', seen)
        p.terminate()   # 不配了：cca 退出时停掉后台的辅助程序（SIGKILL 的话 EXIT 的 trap 不跑，它会一直等着）
        self.assertEqual(p.wait(30), 130)
        # 测试组的那一台：输入的那一头指定了研发组（写编号）的话，在研发组的服务器上找不到这个配对码
        p, code, seen = self.shown(a, '--team', '测试组')
        self.assertIn('这次配对记在「测试组」的服务器上', seen)
        rc, out, err = self.cca(b, 'remote', 'join', '--team', tid, answers=[code])
        self.assertEqual((rc, out), (1, ''))
        self.assertIn('没有这个配对码', err)
        rc, out, err = self.cca(b, 'remote', 'join', '--team', '测试组', answers=[code])
        self.assertEqual((rc, out), (0, ''), err)
        self.assertEqual(p.wait(60), 0, p.stderr.read())
        self.assertEqual((len(self.edges()), list(self.rsrv.edges())), (1, [team_pid]))
        rc, out, err = self.cca(b, 'remote', 'peers')
        self.assertEqual(sorted(line.rsplit('  ', 1)[-1] for line in out.splitlines()), ['经「测试组」的服务器', '经「研发组」的服务器'])


class NoRelayTest(rr.Base):
    relay = True

    def test_pairing_on_a_team_server_without_relay(self):
        admin = Device(self.d / 'admin', 'admin', {})
        tid, root = admin.ok('team-new', '研发组', '--device', 'studio').split('\n')[:2]
        d = self.d / 'team-srv'
        d.mkdir()
        make_cert(d)
        args = [sys.executable, '-I', str(SERVER), '--listen', '127.0.0.1:0', '--port-file', str(d / 'port'), '--cert', str(d / 'cert.pem'),
                '--key', str(d / 'key.pem'), '--team-root', root, '--state', str(d / 'state.json')]
        proc = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.addCleanup(lambda: (proc.kill(), proc.wait()))
        port = int(wait_for('Team 的服务器起来', lambda: (d / 'port').exists() and (d / 'port').read_text()))
        admin.ok('team-attach', tid, 'https://127.0.0.1:%d' % port, openssl_pin(d / 'cert.pem'))
        admin.ok('team-map', tid, 'claude', 'alice@example.com', 'US-A')
        dev = self.device('box')   # 在测试组里（它的服务器开着中转）
        dev.config(lambda c: c.update(nodes=['US-A', 'US-B']))
        link = admin.ok('team-invite', tid, 'box', '--accounts', 'claude:alice@example.com').split('\n')[0]
        dev.ok('team-join', '--name', 'box', stdin=link)
        self.machines(dev)
        r = dev.run('remote-pair-show', '--hub', tid)
        self.assertEqual((r.returncode, r.stdout), (1, ''))
        self.assertIn('「研发组」的服务器还没开远程会话的中转（管理员要重新部署、把中转打开：cca team deploy）', r.stderr)
        # 测试组的那一台照常配得了
        p = subprocess.Popen([rr.HELPER, 'remote-pair-show', '--hub', self.team.id], env=dev.env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, text=True)
        self.addCleanup(lambda: (p.kill(), p.wait(), p.stdout.close(), p.stderr.close()))
        self.assertRegex(p.stdout.readline(), r'^code [0-9A-Z]{4}(-[0-9A-Z]{4}){3}$')


if __name__ == '__main__':
    unittest.main(verbosity=2)
