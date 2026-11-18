#!/usr/bin/env python3
"""本机 UDP 走节点（cca local udp）的 PY 部分：TUN 配置、状态、选节点、要装到 root 那边的文件。临时 HOME，不碰 /Library、不联网。"""
import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import plistlib
import shlex
import tempfile
import unittest
from unittest import mock


CCA = Path(__file__).resolve().parents[1] / 'cca'
SOURCE = CCA.read_text().split("PY_SRC=$(cat <<'PY'\n", 1)[1].split('\nPY\n)', 1)[0]
DEFINITIONS = SOURCE.split('\nif len(sys.argv) < 2', 1)[0]
VLESS = {'name': 'US-A', 'type': 'vless', 'server': '192.0.2.10', 'port': 443, 'uuid': '00000000-0000-4000-8000-000000000000',
         'udp': True, 'tls': True, 'servername': 'example.com', 'flow': 'xtls-rprx-vision',
         'reality-opts': {'public-key': 'secret-public-key', 'short-id': 'abcd'}}
NOUDP = {'name': 'US-B', 'type': 'trojan', 'server': '192.0.2.11', 'port': 443, 'password': 'pw-b'}
HY2 = {'name': 'HK-A', 'type': 'hysteria2', 'server': '192.0.2.12', 'port': 8443, 'password': 'pw-h'}
CHAIN = {'name': 'US-C', 'type': 'ss', 'server': '192.0.2.13', 'port': 8388, 'cipher': 'aes-128-gcm', 'password': 'pw-c',
         'udp': True, 'dialer-proxy': 'HK-A'}


class LocalUdp(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-udp-')
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.cca = self.home / '.cca'
        (self.cca / 'core').mkdir(parents=True)
        self.env = mock.patch.dict(os.environ, {'HOME': str(self.home), 'CCA_HOME': str(self.cca), 'CCA_TOOL': 'claude'}, clear=True)
        self.env.start()
        self.addCleanup(self.env.stop)
        self.ns = {}
        exec(compile(DEFINITIONS, str(CCA), 'exec'), self.ns)
        self.nodes([VLESS, NOUDP, HY2, CHAIN])
        self.config({'client': 'builtin'})

    def nodes(self, proxies):
        (self.cca / 'core' / 'nodes.json').write_text(json.dumps({'saved_at': 1, 'nodes': [p['name'] for p in proxies], 'proxies': proxies}))

    def config(self, local, nodes=('US-A', 'US-B', 'HK-A', 'US-C')):
        (self.cca / 'config.json').write_text(json.dumps({'version': 2, 'nodes': list(nodes), 'local': local}))

    def cfg(self):
        return self.ns['cfg_load']()

    def run_cmd(self, name, *args):
        out, err, code = io.StringIO(), io.StringIO(), 0
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                self.ns[name](*args)
            except SystemExit as e:
                code = e.code if isinstance(e.code, int) else 1
        return out.getvalue(), err.getvalue(), code

    def shvars(self, text):
        return dict(kv.split('=', 1) for kv in shlex.split(text))

    def install(self, name):
        """模仿 root 那边装好了：udp-build 写出来的 info.json 放进沙箱里的 root 目录"""
        folder = self.home / ('build-' + name)
        folder.mkdir()
        (folder / 'cca-udp').write_text('#!/bin/sh\n')
        core = self.home / 'mihomo'
        core.write_bytes(b'fake core')
        out, _, code = self.run_cmd('cmd_udp_build', name, str(folder), str(core), '1.19.33')
        self.assertEqual(code, 0)
        root = Path(self.ns['udp_paths']()[0])
        root.mkdir(parents=True, exist_ok=True)
        (root / 'info.json').write_bytes((folder / 'info.json').read_bytes())
        return folder, self.shvars(out)

    def test_paths_stay_in_sandbox(self):
        root, plist, log, owner = self.ns['udp_paths']()
        for p in (root, plist, log):
            self.assertTrue(p.startswith(str(self.home)), p)
        self.assertTrue(root.endswith('/cca-udp'))
        self.assertEqual(owner, '%d:%d' % (os.getuid(), os.getgid()))
        self.assertEqual(self.ns['UDP_FLAG'], str(self.cca / 'udp' / 'on'))

    def test_config_only_udp_goes_to_node(self):
        c = json.loads(self.ns['udp_config']('US-A'))
        self.assertEqual(c['rules'][0], 'NETWORK,TCP,DIRECT')
        self.assertEqual(c['rules'][1], 'DST-PORT,53,DIRECT')
        self.assertIn('PROCESS-NAME-REGEX', c['rules'][2])
        self.assertEqual(c['rules'][-2:], ['NETWORK,UDP,US-A', 'MATCH,DIRECT'])
        self.assertTrue(c['tun']['enable'] and c['tun']['auto-route'] and c['tun']['auto-detect-interface'])
        self.assertEqual(c['tun']['dns-hijack'], [])
        for net in ('192.168.0.0/16', '10.0.0.0/8', '172.16.0.0/12', '100.64.0.0/10', 'fe80::/10'):
            self.assertIn(net, c['tun']['route-exclude-address'])
        self.assertEqual([p['name'] for p in c['proxies']], ['US-A'])
        for k in ('external-controller', 'external-controller-unix', 'port', 'mixed-port', 'socks-port', 'listeners', 'dns'):
            self.assertNotIn(k, c)
        self.assertFalse(c['allow-lan'])

    def test_config_keeps_dialer_chain(self):
        c = json.loads(self.ns['udp_config']('US-C'))
        self.assertEqual(sorted(p['name'] for p in c['proxies']), ['HK-A', 'US-C'])

    def test_config_refuses_nodes_without_udp(self):
        self.assertIsNone(self.ns['udp_config']('US-B'))        # trojan 没写 udp: true
        self.assertIsNotNone(self.ns['udp_config']('HK-A'))     # hysteria2 本来就转发 UDP
        self.assertIsNone(self.ns['udp_config']('NOPE'))
        self.assertIsNone(self.ns['udp_config']('US-A,DIRECT'))

    def test_pick_node(self):
        out, _, code = self.run_cmd('cmd_udp_node', 'US-A')
        self.assertEqual((out, code), ('US-A\n', 0))
        out, _, code = self.run_cmd('cmd_udp_node', 'US-B')
        self.assertEqual(code, 3)
        self.assertEqual(out.split(), ['US-A', 'HK-A', 'US-C'])
        out, _, code = self.run_cmd('cmd_udp_node', 'NOPE')
        self.assertEqual(code, 2)
        out, _, code = self.run_cmd('cmd_udp_node')
        self.assertEqual(code, 2)
        self.assertNotIn('US-B', out.split())

    def test_state_and_set(self):
        self.assertEqual(self.ns['udp_state'](self.cfg()), ('', 'off'))
        self.run_cmd('cmd_local_set', 'udp', 'US-A')
        self.assertEqual(self.cfg()['local'], {'client': 'builtin', 'udp': 'US-A'})
        self.assertEqual(self.ns['udp_state'](self.cfg()), ('US-A', 'missing'))
        self.install('US-A')
        self.assertEqual(self.ns['udp_state'](self.cfg()), ('US-A', 'ok'))
        moved = dict(VLESS, server='192.0.2.99')          # 路由器上改了这个节点：装着的配置旧了
        self.nodes([moved, NOUDP, HY2, CHAIN])
        self.assertEqual(self.ns['udp_state'](self.cfg()), ('US-A', 'stale'))
        self.nodes([NOUDP, HY2, CHAIN])                     # 节点定义没了
        self.assertEqual(self.ns['udp_state'](self.cfg()), ('US-A', 'undefined'))
        self.nodes([VLESS, NOUDP, HY2, CHAIN])
        self.run_cmd('cmd_local_set', 'udp', '')            # 关掉：这一项去掉
        self.assertEqual(self.cfg()['local'], {'client': 'builtin'})
        self.assertEqual(self.ns['udp_state'](self.cfg()), ('', 'off'))
        self.run_cmd('cmd_local_set', 'udp', 'US-A')
        self.run_cmd('cmd_local_set', 'client', '', 'core_off', '1')   # cca local off：换掉内置代理时一起清掉
        self.assertEqual(self.cfg()['local'], {'core_off': '1'})
        self.config({'client': '', 'udp': 'US-A'})          # 不是内置代理：不算开着
        self.assertEqual(self.ns['udp_state'](self.cfg()), ('', 'off'))

    def test_helper_version_change_is_stale(self):
        self.run_cmd('cmd_local_set', 'udp', 'US-A')
        self.install('US-A')
        self.ns['UDP_HELPER'] += 1
        self.assertEqual(self.ns['udp_state'](self.cfg()), ('US-A', 'stale'))

    def test_build_files(self):
        folder, v = self.install('US-A')
        cfg = folder / 'config.yaml'
        self.assertEqual(cfg.stat().st_mode & 0o777, 0o600)
        self.assertEqual(v['B_CFG'], hashlib.sha256(cfg.read_bytes()).hexdigest())
        self.assertEqual(v['B_PLIST'], hashlib.sha256((folder / 'plist').read_bytes()).hexdigest())
        self.assertEqual(v['B_INFO'], hashlib.sha256((folder / 'info.json').read_bytes()).hexdigest())
        self.assertEqual(v['B_BIN'], hashlib.sha256(b'fake core').hexdigest())
        self.assertEqual(v['B_SAME'], '')
        p = plistlib.loads((folder / 'plist').read_bytes())
        root = self.ns['udp_paths']()[0]
        self.assertEqual(p['Label'], 'local.cca.udp')
        self.assertEqual(p['ProgramArguments'], [root + '/cca-udp'])
        self.assertEqual(p['KeepAlive'], {'PathState': {self.ns['UDP_FLAG']: True}})
        info = json.loads((folder / 'info.json').read_text())
        self.assertEqual((info['node'], info['sum'], info['helper']), ('US-A', v['B_CFG'], self.ns['UDP_HELPER']))
        for f in ('plist', 'info.json'):   # 谁都能读的文件里没有节点密钥
            self.assertNotIn('secret-public-key', (folder / f).read_text(errors='replace'))
        self.assertIn('secret-public-key', cfg.read_text())
        # 再生成一次同样的：B_SAME
        again = self.home / 'again'
        again.mkdir()
        (again / 'cca-udp').write_text('#!/bin/sh\n')
        out, _, code = self.run_cmd('cmd_udp_build', 'US-A', str(again), str(self.home / 'mihomo'), '1.19.33')
        self.assertEqual((code, self.shvars(out)['B_SAME']), (0, '1'))
        out, err, code = self.run_cmd('cmd_udp_build', 'US-B', str(self.home / 'again'), str(self.home / 'mihomo'), '1.19.33')
        self.assertNotEqual(code, 0)

    def test_view_and_running(self):
        c = self.cfg()
        self.assertEqual(self.ns['udp_view'](c, []), {'node': None, 'state': 'off', 'installed': False, 'running': False})
        self.run_cmd('cmd_local_set', 'udp', 'US-A')
        self.install('US-A')
        root = self.ns['udp_paths']()[0]
        procs = [(10, '%s/mihomo -d %s -f %s/config.yaml' % (root, root, root))]
        self.assertEqual(self.ns['udp_view'](self.cfg(), procs), {'node': 'US-A', 'state': 'ok', 'installed': True, 'running': True})
        self.assertTrue(self.ns['udp_running']([(11, '/bin/bash %s/mihomo -d x' % root)]))
        self.assertFalse(self.ns['udp_running']([(12, '/usr/bin/vi %s/mihomo' % root)]))

    def test_env_and_leak_hint(self):
        out, _, code = self.run_cmd('cmd_udp_env')
        v = self.shvars(out)
        self.assertEqual((code, v['UDP_STATE'], v['UDP_NODE'], v['UDP_INSTALLED']), (0, 'off', '', ''))
        self.assertIn('cca local udp on', self.ns['udp_leak_hint']())
        self.run_cmd('cmd_local_set', 'udp', 'US-A')
        self.assertIn('cca local udp 查看', self.ns['udp_leak_hint']())
        self.config({})
        self.assertEqual(self.ns['udp_leak_hint'](), '')


if __name__ == '__main__':
    unittest.main()
