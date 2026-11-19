#!/usr/bin/env python3
"""router_merge：多设备共用一个路由器时把本机账号合并进路由器现有的并集，所有权靠路由器上的 owners（槽位 → 设备标识）认。
单设备（owners 全是我的 / 读不到）逐字节等于 router_accounts；多设备保留别的设备的槽位、本机端口写回并集槽位。
临时 HOME、cca 的辅助程序（cca-core router-merge：stdin 是从路由器回读来的 accounts.yaml 和 owners），不联网。"""
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from helper_bin import HELPER

OTHER = 'ffff0000ffff0000'   # 另一台设备的标识
# 会动到这台机器、会联网的命令：沙箱的 PATH 最前面放只会报错的替身（合并只读写 ~/.cca 里的文件，一个都不该调到）
GUARDED = ('security', 'osascript', 'open', 'pbcopy', 'launchctl', 'ssh', 'scp', 'curl', 'git', 'nc', 'crontab', 'defaults', 'killall', 'pkill',
           'sudo', 'lsappinfo', 'networksetup', 'scutil', 'codesign', 'spctl', 'hdiutil', 'ditto', 'claude', 'codex', 'agy', 'mihomo', 'ps')
GUARD = '#!/bin/sh\nprintf \'%s\\n\' "$0 $*" >> "$HOME/unexpected-commands.log"\nexit 127\n'


class RouterMerge(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-rmerge-')
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.cca = self.home / '.cca'
        self.cca.mkdir()
        (self.home / 'bin').mkdir()
        for name in GUARDED:
            (self.home / 'bin' / name).write_text(GUARD)
            (self.home / 'bin' / name).chmod(0o755)
        stray = self.home / 'unexpected-commands.log'
        self.addCleanup(lambda: self.assertEqual(stray.read_text() if stray.exists() else '', '', '调到了没有准备假命令的外部命令'))
        self.env = {'HOME': str(self.home), 'CCA_HOME': str(self.cca), 'CCA_TOOL': 'claude',
                    'PATH': '%s:/usr/bin:/bin' % (self.home / 'bin')}

    def build(self, accounts, nodes=('US-A', 'US-B', 'US-C', 'US-D')):
        """这台设备的账号表（Claude 的官方账号）和 config.json：端口从 7901 起一个账号一个"""
        self.accounts, self.nodes = list(accounts), list(nodes)
        ports = {email: 7901 + i for i, (email, _) in enumerate(accounts)}
        (self.cca / 'config.json').write_text(json.dumps({'version': 2, 'nodes': self.nodes, 'ports': ports}))
        (self.cca / 'accounts.json').write_text(json.dumps(dict(accounts)))

    def cfg(self):
        return json.loads((self.cca / 'config.json').read_text(encoding='utf-8'))

    @staticmethod
    def name(email):
        """账号在路由器端口池里的名字：Claude 的官方账号没有前缀，就是邮箱化成的账号目录名（辅助程序的 acct_id / ident）"""
        s = re.sub(r'[^a-z0-9]+', '-', email.lower()).strip('-')[:20].strip('-') or 'acct'
        return '%s-%s' % (s, hashlib.sha1(email.encode('utf-8')).hexdigest()[:6])

    def router_accounts(self):
        """只有这台设备时路由器上 accounts.yaml 的内容（router_accounts 的输出）。只有官方账号时这一行不能变：
        变了，所有已安装的机器升级后都会重新部署路由器端"""
        return json.dumps({'base': 7900, 'slots': 8, 'nodes': self.nodes,
                           'accounts': [{'slot': i + 1, 'name': self.name(email), 'node': node}
                                        for i, (email, node) in enumerate(self.accounts)]}, ensure_ascii=False)

    def merge(self, remote):
        """跑一次 router-merge。remote 是回读来的 {'accounts': [...], 'owners': {槽位: 设备标识}}，None 是路由器上什么都没有。
        stdin 照 bash 的 router_push 拼：accounts.yaml 的内容、一行 ---CCA-OWN---、owners 的内容。
        返回 (accounts 载荷, owners)；载荷那一行的原文留在 self.sent"""
        remote = remote or {}
        acc = json.dumps({'accounts': remote['accounts']}) if 'accounts' in remote else ''
        own = json.dumps(remote['owners']) if 'owners' in remote else ''
        r = subprocess.run([HELPER, 'router-merge'], env=self.env, input=acc + '\n---CCA-OWN---\n' + own,
                           capture_output=True, encoding='utf-8', timeout=30)
        self.assertEqual((r.returncode, r.stderr), (0, ''))
        self.sent, rest = r.stdout.split('\n---CCA-PORTS---\n', 1)
        return json.loads(self.sent), json.loads(rest.split('\n---CCA-OWN---\n', 1)[1])

    def did(self):
        """这台设备的标识：第一次合并时生成、记进 config.json（这里先对着空的路由器合并一次）"""
        self.merge(None)
        return self.cfg()['device_id']

    # 单设备：回读为空 → accounts 逐字节等于 router_accounts，owners 全标成我这台，并生成 device_id
    def test_single_device_none(self):
        self.build([('alice@example.com', 'US-A'), ('bob@example.com', 'US-B')])
        want = self.router_accounts()
        payload, owners = self.merge(None)
        self.assertEqual(self.sent, want)
        c = self.cfg()
        self.assertEqual(owners, {'1': c['device_id'], '2': c['device_id']})
        self.assertTrue(c['device_id'])

    # 回读到的账号都是我这台的（owners 全是我）：仍然逐字节一样
    def test_single_device_own_remote(self):
        self.build([('alice@example.com', 'US-A'), ('bob@example.com', 'US-B')])
        me = self.did()
        ra = json.loads(self.router_accounts())
        remote = {'accounts': ra['accounts'], 'owners': {str(a['slot']): me for a in ra['accounts']}}
        want = self.router_accounts()
        payload, _ = self.merge(remote)
        self.assertEqual(self.sent, want)

    # owners 读不到（旧版 cca / 全新路由器）：当作全是我的 → 直接覆盖成我当前的账号表
    def test_owners_absent_overwrite(self):
        self.build([('alice@example.com', 'US-A')])
        bob = self.name('bob@example.com')
        remote = {'accounts': [{'slot': 1, 'name': self.name('alice@example.com'), 'node': 'US-A'},
                               {'slot': 2, 'name': bob, 'node': 'US-B'}]}   # 没有 owners
        payload, owners = self.merge(remote)
        names = [a['name'] for a in payload['accounts']]
        self.assertEqual(names, [self.name('alice@example.com')])   # bob 没有 owners 时当作我的、不在表里 → 覆盖掉
        self.assertEqual(owners, {'1': self.cfg()['device_id']})

    # 单设备删了账号：回读里被删的账号 owner 是我 → 认出是我删的，去掉（走单设备覆盖）
    def test_single_device_removed(self):
        self.build([('alice@example.com', 'US-A')])
        me = self.did()
        alice, bob = self.name('alice@example.com'), self.name('bob@example.com')
        remote = {'accounts': [{'slot': 1, 'name': alice, 'node': 'US-A'},
                               {'slot': 2, 'name': bob, 'node': 'US-B'}],
                  'owners': {'1': me, '2': me}}
        payload, owners = self.merge(remote)
        self.assertEqual([a['name'] for a in payload['accounts']], [alice])
        self.assertEqual(owners, {'1': me})

    # 多设备：别的设备的账号（owner 不是我）在槽位 1，我的接在后面；保留它的槽位、端口写回并集槽位、nodes 取并集
    def test_two_devices(self):
        self.build([('alice@example.com', 'US-A'), ('bob@example.com', 'US-B')])
        me = self.did()
        carol = 'carol-on-device-2'
        remote = {'accounts': [{'slot': 1, 'name': carol, 'node': 'US-C'}], 'owners': {'1': OTHER}}
        payload, owners = self.merge(remote)
        by_slot = {a['slot']: a for a in payload['accounts']}
        self.assertEqual(by_slot[1]['name'], carol)
        self.assertEqual(by_slot[2]['name'], self.name('alice@example.com'))
        self.assertEqual(by_slot[3]['name'], self.name('bob@example.com'))
        self.assertEqual(payload['nodes'], ['US-A', 'US-B', 'US-C', 'US-D'])
        self.assertEqual(owners, {'1': OTHER, '2': me, '3': me})
        self.assertEqual(self.cfg()['ports'], {'alice@example.com': 7902, 'bob@example.com': 7903})

    # 共用账号（我和另一台都有同一个邮箱）：天然共用一个槽位，不重复；别的设备的保留
    def test_shared_account(self):
        self.build([('alice@example.com', 'US-A'), ('bob@example.com', 'US-B')])
        me = self.did()
        alice, dave = self.name('alice@example.com'), 'dave-on-device-2'
        remote = {'accounts': [{'slot': 5, 'name': dave, 'node': 'US-D'},
                               {'slot': 6, 'name': alice, 'node': 'US-A'}],
                  'owners': {'5': OTHER, '6': OTHER}}
        payload, owners = self.merge(remote)
        names = [a['name'] for a in payload['accounts']]
        self.assertEqual(names.count(alice), 1)             # alice 只出现一次
        by_name = {a['name']: a for a in payload['accounts']}
        self.assertEqual(by_name[alice]['slot'], 6)         # 沿用对方给的槽位（共用、同一个出口）
        self.assertEqual(by_name[dave]['slot'], 5)
        self.assertEqual(self.cfg()['ports']['alice@example.com'], 7906)
        self.assertEqual(by_name[self.name('bob@example.com')]['slot'], 1)   # bob 独有：取最低空槽
        self.assertEqual(owners['6'], OTHER)                # 共用槽位的 owner 保持对方的
        self.assertEqual(owners['1'], me)

    # 别的设备用着、这台设备没选的节点也进 nodes（接在后面，路由器上它的槽位才选得中）；REJECT 和这台已经有的不重复加。
    # 这一个是改写时加的：上面 test_two_devices 里别的设备的节点本来就在这台的可选节点里，看不出并没并
    def test_other_devices_nodes_join_the_list(self):
        self.build([('alice@example.com', 'US-A')], nodes=('US-A', 'US-B'))
        me = self.did()
        remote = {'accounts': [{'slot': 1, 'name': 'carol-on-device-2', 'node': 'HK-A'},
                               {'slot': 2, 'name': 'dave-on-device-2', 'node': 'REJECT'},
                               {'slot': 3, 'name': 'erin-on-device-2', 'node': 'US-B'}],
                  'owners': {'1': OTHER, '2': OTHER, '3': OTHER}}
        payload, owners = self.merge(remote)
        self.assertEqual(payload['nodes'], ['US-A', 'US-B', 'HK-A'])
        self.assertEqual([(a['slot'], a['node']) for a in payload['accounts']], [(1, 'HK-A'), (2, 'REJECT'), (3, 'US-B'), (4, 'US-A')])
        self.assertEqual(owners, {'1': OTHER, '2': OTHER, '3': OTHER, '4': me})
        self.assertEqual(self.cfg()['nodes'], ['US-A', 'US-B'])     # 这台设备自己的可选节点不动


if __name__ == '__main__':
    unittest.main()
