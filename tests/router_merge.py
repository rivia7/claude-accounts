#!/usr/bin/env python3
"""router_merge：多设备共用一个路由器时把本机账号合并进路由器现有的并集，所有权靠路由器上的 owners（槽位 → 设备标识）认。
单设备（owners 全是我的 / 读不到）逐字节等于 router_accounts；多设备保留别的设备的槽位、本机端口写回并集槽位。内嵌 Python，不联网。"""
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock


CCA = Path(__file__).resolve().parents[1] / 'cca'
SOURCE = CCA.read_text().split("PY_SRC=$(cat <<'PY'\n", 1)[1].split('\nPY\n)', 1)[0]
DEFINITIONS = SOURCE.split('\nif len(sys.argv) < 2', 1)[0]
OTHER = 'ffff0000ffff0000'   # 另一台设备的标识


class RouterMerge(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-rmerge-')
        self.addCleanup(self.temp.cleanup)
        home = Path(self.temp.name)
        (home / '.cca').mkdir()
        with mock.patch.dict(os.environ,
                             {'HOME': str(home), 'CCA_HOME': str(home / '.cca'), 'CCA_TOOL': 'claude'}, clear=True):
            self.ns = {}
            exec(compile(DEFINITIONS, str(CCA), 'exec'), self.ns)

    def build(self, accounts, nodes=('US-A', 'US-B', 'US-C', 'US-D')):
        ns = self.ns
        c = ns['cfg_fill']({})
        c['nodes'] = list(nodes)
        tab, ports = {}, {}
        for i, (email, node) in enumerate(accounts):
            tab[email] = node
            ports[email] = 7901 + i
        c['ports'] = ports
        ns['TABS']['claude'] = tab
        ns['TABS']['codex'] = {}
        ns['TABS']['agy'] = {}
        return c

    def name(self, email):
        ns = self.ns
        return ns['CLAUDE'].pool_prefix + ns['acct_id'](email, ns['CLAUDE'])

    def did(self, c):
        return self.ns['device_id'](c)

    # 单设备：回读为空 → accounts 逐字节等于 router_accounts，owners 全标成我这台，并生成 device_id
    def test_single_device_none(self):
        c = self.build([('alice@example.com', 'US-A'), ('bob@example.com', 'US-B')])
        want = self.ns['router_accounts'](c)
        payload, owners = self.ns['router_merge'](c, None)
        self.assertEqual(json.dumps(payload, ensure_ascii=False), json.dumps(want, ensure_ascii=False))
        self.assertEqual(owners, {'1': c['device_id'], '2': c['device_id']})
        self.assertTrue(c['device_id'])

    # 回读到的账号都是我这台的（owners 全是我）：仍然逐字节一样
    def test_single_device_own_remote(self):
        c = self.build([('alice@example.com', 'US-A'), ('bob@example.com', 'US-B')])
        me = self.did(c)
        ra = self.ns['router_accounts'](c)
        remote = {'accounts': ra['accounts'], 'owners': {str(a['slot']): me for a in ra['accounts']}}
        want = self.ns['router_accounts'](c)
        payload, _ = self.ns['router_merge'](c, remote)
        self.assertEqual(json.dumps(payload, ensure_ascii=False), json.dumps(want, ensure_ascii=False))

    # owners 读不到（旧版 cca / 全新路由器）：当作全是我的 → 直接覆盖成我当前的账号表
    def test_owners_absent_overwrite(self):
        c = self.build([('alice@example.com', 'US-A')])
        bob = self.name('bob@example.com')
        remote = {'accounts': [{'slot': 1, 'name': self.name('alice@example.com'), 'node': 'US-A'},
                               {'slot': 2, 'name': bob, 'node': 'US-B'}]}   # 没有 owners
        payload, owners = self.ns['router_merge'](c, remote)
        names = [a['name'] for a in payload['accounts']]
        self.assertEqual(names, [self.name('alice@example.com')])   # bob 没有 owners 时当作我的、不在表里 → 覆盖掉
        self.assertEqual(owners, {'1': c['device_id']})

    # 单设备删了账号：回读里被删的账号 owner 是我 → 认出是我删的，去掉（走单设备覆盖）
    def test_single_device_removed(self):
        c = self.build([('alice@example.com', 'US-A')])
        me = self.did(c)
        alice, bob = self.name('alice@example.com'), self.name('bob@example.com')
        remote = {'accounts': [{'slot': 1, 'name': alice, 'node': 'US-A'},
                               {'slot': 2, 'name': bob, 'node': 'US-B'}],
                  'owners': {'1': me, '2': me}}
        payload, owners = self.ns['router_merge'](c, remote)
        self.assertEqual([a['name'] for a in payload['accounts']], [alice])
        self.assertEqual(owners, {'1': me})

    # 多设备：别的设备的账号（owner 不是我）在槽位 1，我的接在后面；保留它的槽位、端口写回并集槽位、nodes 取并集
    def test_two_devices(self):
        c = self.build([('alice@example.com', 'US-A'), ('bob@example.com', 'US-B')])
        me = self.did(c)
        carol = 'carol-on-device-2'
        remote = {'accounts': [{'slot': 1, 'name': carol, 'node': 'US-C'}], 'owners': {'1': OTHER}}
        payload, owners = self.ns['router_merge'](c, remote)
        by_slot = {a['slot']: a for a in payload['accounts']}
        self.assertEqual(by_slot[1]['name'], carol)
        self.assertEqual(by_slot[2]['name'], self.name('alice@example.com'))
        self.assertEqual(by_slot[3]['name'], self.name('bob@example.com'))
        self.assertEqual(payload['nodes'], ['US-A', 'US-B', 'US-C', 'US-D'])
        self.assertEqual(owners, {'1': OTHER, '2': me, '3': me})
        self.assertEqual(c['ports'], {'alice@example.com': 7902, 'bob@example.com': 7903})

    # 共用账号（我和另一台都有同一个邮箱）：天然共用一个槽位，不重复；别的设备的保留
    def test_shared_account(self):
        c = self.build([('alice@example.com', 'US-A'), ('bob@example.com', 'US-B')])
        me = self.did(c)
        alice, dave = self.name('alice@example.com'), 'dave-on-device-2'
        remote = {'accounts': [{'slot': 5, 'name': dave, 'node': 'US-D'},
                               {'slot': 6, 'name': alice, 'node': 'US-A'}],
                  'owners': {'5': OTHER, '6': OTHER}}
        payload, owners = self.ns['router_merge'](c, remote)
        names = [a['name'] for a in payload['accounts']]
        self.assertEqual(names.count(alice), 1)             # alice 只出现一次
        by_name = {a['name']: a for a in payload['accounts']}
        self.assertEqual(by_name[alice]['slot'], 6)         # 沿用对方给的槽位（共用、同一个出口）
        self.assertEqual(by_name[dave]['slot'], 5)
        self.assertEqual(c['ports']['alice@example.com'], 7906)
        self.assertEqual(by_name[self.name('bob@example.com')]['slot'], 1)   # bob 独有：取最低空槽
        self.assertEqual(owners['6'], OTHER)                # 共用槽位的 owner 保持对方的
        self.assertEqual(owners['1'], me)


if __name__ == '__main__':
    unittest.main()
