#!/usr/bin/env python3
"""本机 UDP 走节点（cca local udp）在辅助程序里的那一半：TUN 配置、状态、选节点、要装到 root 那边的文件。
临时 HOME、cca 的辅助程序（cca-core udp-env / udp-node / udp-build / local-set，界面看的那一份经 gui-state，
WebRTC 警告后面的补充经 leak-verdict），不碰 /Library、不联网。

原来直接调内部函数的地方这里都经子命令测；辅助程序自己的单元测试（cargo test，cli/src/cmd/corenet/udp.rs）里另有直接调的：
节点名和转发 UDP 的判断（names_and_capability）、从命令行认 root 那边的内核在不在跑（running_is_read_from_the_command_line）、
没设节点时的状态和界面看的那一份（off_without_a_node、view_without_a_node）。"""
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shlex
import subprocess
import sys
import tempfile
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from helper_bin import HELPER

UDP_HELPER = 1   # root 那边的启动脚本、launchd 配置、规则的版本（cli/src/cmd/corenet/udp.rs 的同名常量）
VLESS = {'name': 'US-A', 'type': 'vless', 'server': '192.0.2.10', 'port': 443, 'uuid': '00000000-0000-4000-8000-000000000000',
         'udp': True, 'tls': True, 'servername': 'example.com', 'flow': 'xtls-rprx-vision',
         'reality-opts': {'public-key': 'secret-public-key', 'short-id': 'abcd'}}
NOUDP = {'name': 'US-B', 'type': 'trojan', 'server': '192.0.2.11', 'port': 443, 'password': 'pw-b'}
HY2 = {'name': 'HK-A', 'type': 'hysteria2', 'server': '192.0.2.12', 'port': 8443, 'password': 'pw-h'}
CHAIN = {'name': 'US-C', 'type': 'ss', 'server': '192.0.2.13', 'port': 8388, 'cipher': 'aes-128-gcm', 'password': 'pw-c',
         'udp': True, 'dialer-proxy': 'HK-A'}
# 沙箱里的假命令。ps：gui-state 看在跑的进程（照 ps-command 这个文件回答）；route：内置代理查公网的流量走哪块网卡
# （答物理网卡，不用绑定）。别的会动到这台机器、会联网的命令一律是只会报错的替身，调到了收尾时算失败
FAKES = {
    'ps': 'case "$*" in *command*) cat "$HOME/ps-command" 2>/dev/null; exit 0 ;; esac\n',
    'route': 'case "$*" in "-n get "*) echo "  interface: en0"; exit 0 ;; esac\n',
}
GUARDED = ('security', 'osascript', 'open', 'pbcopy', 'launchctl', 'ssh', 'scp', 'curl', 'git', 'nc', 'crontab', 'defaults', 'killall', 'pkill',
           'sudo', 'lsappinfo', 'networksetup', 'scutil', 'codesign', 'spctl', 'hdiutil', 'ditto', 'claude', 'codex', 'agy', 'mihomo',
           'netstat', 'ipconfig')
GUARD = 'printf \'%s\\n\' "$0 $*" >> "$HOME/unexpected-commands.log"\nexit 127\n'


class LocalUdp(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-udp-')
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.cca = self.home / '.cca'
        (self.cca / 'core').mkdir(parents=True)
        (self.home / 'bin').mkdir()
        for name in GUARDED + tuple(FAKES):
            (self.home / 'bin' / name).write_text('#!/bin/sh\n' + FAKES.get(name, '') + GUARD)
            (self.home / 'bin' / name).chmod(0o755)
        stray = self.home / 'unexpected-commands.log'
        self.addCleanup(lambda: self.assertEqual(stray.read_text() if stray.exists() else '', '', '调到了没有准备假命令的外部命令'))
        self.env = {'HOME': str(self.home), 'CCA_HOME': str(self.cca), 'CCA_TOOL': 'claude',
                    'PATH': '%s:/usr/bin:/bin' % (self.home / 'bin')}
        self.nodes([VLESS, NOUDP, HY2, CHAIN])
        self.config({'client': 'builtin'})

    def nodes(self, proxies):
        (self.cca / 'core' / 'nodes.json').write_text(json.dumps({'saved_at': 1, 'nodes': [p['name'] for p in proxies], 'proxies': proxies}))

    def config(self, local, nodes=('US-A', 'US-B', 'HK-A', 'US-C')):
        (self.cca / 'config.json').write_text(json.dumps({'version': 2, 'nodes': list(nodes), 'local': local}))

    def cfg(self):
        return json.loads((self.cca / 'config.json').read_text(encoding='utf-8'))

    def run_cmd(self, name, *args, **env):
        r = subprocess.run([HELPER, name, *args], env=dict(self.env, **env), stdin=subprocess.DEVNULL, capture_output=True,
                           encoding='utf-8', timeout=30)
        self.assertNotIn('cca-core:', r.stderr)   # 辅助程序自己出了错（内部错误、没处理的错误）：不是哪个子命令该有的退出
        return r.stdout, r.stderr, r.returncode

    def shvars(self, text):
        return dict(kv.split('=', 1) for kv in shlex.split(text))

    def udp_env(self):
        """bash 的 udp_env 拿到的：root 那边的位置、设的节点和状态、装着的是哪个节点"""
        out, err, code = self.run_cmd('udp-env')
        self.assertEqual((code, err), (0, ''))
        return self.shvars(out)

    def udp_state(self):
        """(设的节点, 状态)"""
        v = self.udp_env()
        return v['UDP_NODE'], v['UDP_STATE']

    def udp_root(self):
        """root 那边的目录（沙箱里换进了 HOME 下面）"""
        return self.udp_env()['UDP_ROOT']

    def udp_config(self, name):
        """root 那边的 TUN 内核的配置（udp-build 写出来的 config.yaml）；这个节点没有定义、不转发 UDP 时不肯写，是 None"""
        folder = Path(tempfile.mkdtemp(prefix='config-', dir=str(self.home)))
        (folder / 'cca-udp').write_text('#!/bin/sh\n')
        core = self.home / 'mihomo'
        core.write_bytes(b'fake core')
        out, err, code = self.run_cmd('udp-build', name, str(folder), str(core), '1.19.33')
        if code:
            self.assertEqual((code, out, err), (1, '', '内置代理里没有节点 %s 转发 UDP 的定义\n' % name))
            self.assertEqual([p.name for p in folder.iterdir()], ['cca-udp'])
            return None
        return (folder / 'config.yaml').read_text(encoding='utf-8')

    def udp_view(self, procs):
        """界面看的那一份（_gui state 的 config.local.udp；bash 是带着 CCA_RO=1 跑它的）。procs 是 ps 看到的进程 [(进程号, 命令行)]"""
        (self.home / 'ps-command').write_text(''.join('%5d %s\n' % p for p in procs))
        out, err, code = self.run_cmd('gui-state', CCA_RO='1')
        self.assertEqual((code, err), (0, ''))
        state = json.loads(out)
        self.assertEqual(state['errors'], [])
        return state['config']['local']['udp']

    def udp_leak_hint(self):
        """WebRTC 直连风险那一句警告后面的补充：给 leak-verdict 一份 STUN 看到中国大陆地址的探测结果（归属地都带着，不用再查）"""
        geo = {'203.0.113.0/24': {'t': time.time(), 'cc': 'us', 'country': 'United States', 'isp': 'Example ISP'},
               '192.0.2.0/24': {'t': time.time(), 'cc': 'cn', 'country': 'China', 'isp': 'Example ISP'}}
        path = self.home / 'probe.json'
        path.write_text(json.dumps({'dns': {'ok': True, 'servers': ['203.0.113.53'], 'geo': geo}, 'stun': {'ips': ['192.0.2.77']}}))
        out, err, code = self.run_cmd('leak-verdict', str(path), 'local|US-A', '203.0.113.10', 'US', 'detail')
        self.assertEqual((code, err), (0, ''))
        warnings = [row.split('\t', 2)[2] for row in out.splitlines() if row.startswith('W\twebrtc\t')]
        self.assertEqual(len(warnings), 1, out)
        said, end, hint = warnings[0].partition('并限制绕过账号代理的 UDP 流量')
        self.assertTrue(said.startswith('WebRTC 直连风险：') and end, warnings[0])
        return hint

    def install(self, name):
        """模仿 root 那边装好了：udp-build 写出来的 info.json 放进沙箱里的 root 目录"""
        folder = self.home / ('build-' + name)
        folder.mkdir()
        (folder / 'cca-udp').write_text('#!/bin/sh\n')
        core = self.home / 'mihomo'
        core.write_bytes(b'fake core')
        out, _, code = self.run_cmd('udp-build', name, str(folder), str(core), '1.19.33')
        self.assertEqual(code, 0)
        root = Path(self.udp_root())
        root.mkdir(parents=True, exist_ok=True)
        (root / 'info.json').write_bytes((folder / 'info.json').read_bytes())
        return folder, self.shvars(out)

    def test_paths_stay_in_sandbox(self):
        v = self.udp_env()
        root, plist, log, owner = v['UDP_ROOT'], v['UDP_PLIST'], v['UDP_LOG'], v['UDP_OWNER']
        for p in (root, plist, log):
            self.assertTrue(p.startswith(str(self.home)), p)
        self.assertTrue(root.endswith('/cca-udp'))
        self.assertEqual(owner, '%d:%d' % (os.getuid(), os.getgid()))
        self.assertEqual(v['UDP_FLAG'], str(self.cca / 'udp' / 'on'))

    def test_config_only_udp_goes_to_node(self):
        c = json.loads(self.udp_config('US-A'))
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
        c = json.loads(self.udp_config('US-C'))
        self.assertEqual(sorted(p['name'] for p in c['proxies']), ['HK-A', 'US-C'])

    def test_config_refuses_nodes_without_udp(self):
        self.assertIsNone(self.udp_config('US-B'))        # trojan 没写 udp: true
        self.assertIsNotNone(self.udp_config('HK-A'))     # hysteria2 本来就转发 UDP
        self.assertIsNone(self.udp_config('NOPE'))
        self.assertIsNone(self.udp_config('US-A,DIRECT'))

    def test_pick_node(self):
        out, _, code = self.run_cmd('udp-node', 'US-A')
        self.assertEqual((out, code), ('US-A\n', 0))
        out, _, code = self.run_cmd('udp-node', 'US-B')
        self.assertEqual(code, 3)
        self.assertEqual(out.split(), ['US-A', 'HK-A', 'US-C'])
        out, _, code = self.run_cmd('udp-node', 'NOPE')
        self.assertEqual(code, 2)
        out, _, code = self.run_cmd('udp-node')
        self.assertEqual(code, 2)
        self.assertNotIn('US-B', out.split())

    def test_state_and_set(self):
        self.assertEqual(self.udp_state(), ('', 'off'))
        self.run_cmd('local-set', 'udp', 'US-A')
        self.assertEqual(self.cfg()['local'], {'client': 'builtin', 'udp': 'US-A'})
        self.assertEqual(self.udp_state(), ('US-A', 'missing'))
        self.install('US-A')
        self.assertEqual(self.udp_state(), ('US-A', 'ok'))
        moved = dict(VLESS, server='192.0.2.99')          # 路由器上改了这个节点：装着的配置旧了
        self.nodes([moved, NOUDP, HY2, CHAIN])
        self.assertEqual(self.udp_state(), ('US-A', 'stale'))
        self.nodes([NOUDP, HY2, CHAIN])                     # 节点定义没了
        self.assertEqual(self.udp_state(), ('US-A', 'undefined'))
        self.nodes([VLESS, NOUDP, HY2, CHAIN])
        self.run_cmd('local-set', 'udp', '')                # 关掉：这一项去掉
        self.assertEqual(self.cfg()['local'], {'client': 'builtin'})
        self.assertEqual(self.udp_state(), ('', 'off'))
        self.run_cmd('local-set', 'udp', 'US-A')
        self.run_cmd('local-set', 'client', '', 'core_off', '1')   # cca local off：换掉内置代理时一起清掉
        self.assertEqual(self.cfg()['local'], {'core_off': '1'})
        self.config({'client': '', 'udp': 'US-A'})          # 不是内置代理：不算开着
        self.assertEqual(self.udp_state(), ('', 'off'))

    def test_helper_version_change_is_stale(self):
        self.run_cmd('local-set', 'udp', 'US-A')
        self.install('US-A')
        self.assertEqual(self.udp_state(), ('US-A', 'ok'))
        # 辅助程序里的版本从外面改不了，改装着的那一份记的：比现在的旧（升级了 cca）、比现在的新（退回了旧版）都要重新装
        info = Path(self.udp_root()) / 'info.json'
        installed = json.loads(info.read_text())
        self.assertEqual(installed['helper'], UDP_HELPER)
        for helper in (UDP_HELPER - 1, UDP_HELPER + 1):
            info.write_text(json.dumps(dict(installed, helper=helper)))
            self.assertEqual(self.udp_state(), ('US-A', 'stale'))

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
        root = self.udp_root()
        self.assertEqual(p['Label'], 'local.cca.udp')
        self.assertEqual(p['ProgramArguments'], [root + '/cca-udp'])
        self.assertEqual(p['KeepAlive'], {'PathState': {str(self.cca / 'udp' / 'on'): True}})
        info = json.loads((folder / 'info.json').read_text())
        self.assertEqual((info['node'], info['sum'], info['helper']), ('US-A', v['B_CFG'], UDP_HELPER))
        for f in ('plist', 'info.json'):   # 谁都能读的文件里没有节点密钥
            self.assertNotIn('secret-public-key', (folder / f).read_text(errors='replace'))
        self.assertIn('secret-public-key', cfg.read_text())
        # 再生成一次同样的：B_SAME
        again = self.home / 'again'
        again.mkdir()
        (again / 'cca-udp').write_text('#!/bin/sh\n')
        out, _, code = self.run_cmd('udp-build', 'US-A', str(again), str(self.home / 'mihomo'), '1.19.33')
        self.assertEqual((code, self.shvars(out)['B_SAME']), (0, '1'))
        out, err, code = self.run_cmd('udp-build', 'US-B', str(self.home / 'again'), str(self.home / 'mihomo'), '1.19.33')
        self.assertNotEqual(code, 0)
        self.assertIn('没有节点 US-B 转发 UDP 的定义', err)

    @unittest.skipUnless(sys.platform == 'darwin', '本机 UDP 走节点只在 macOS 上有')
    def test_view_and_running(self):
        self.assertEqual(self.udp_view([]), {'node': None, 'state': 'off', 'installed': False, 'running': False})
        self.run_cmd('local-set', 'udp', 'US-A')
        self.install('US-A')
        root = self.udp_root()
        procs = [(10, '%s/mihomo -d %s -f %s/config.yaml' % (root, root, root))]
        self.assertEqual(self.udp_view(procs), {'node': 'US-A', 'state': 'ok', 'installed': True, 'running': True})
        # 在不在跑只认命令行里「root 那边的目录/mihomo -d 」这一段
        self.assertTrue(self.udp_view([(11, '/bin/bash %s/mihomo -d x' % root)])['running'])
        self.assertFalse(self.udp_view([(12, '/usr/bin/vi %s/mihomo' % root)])['running'])

    @unittest.skipUnless(sys.platform == 'darwin', '本机 UDP 走节点只在 macOS 上有')
    def test_env_and_leak_hint(self):
        out, _, code = self.run_cmd('udp-env')
        v = self.shvars(out)
        self.assertEqual((code, v['UDP_STATE'], v['UDP_NODE'], v['UDP_INSTALLED']), (0, 'off', '', ''))
        self.assertIn('cca local udp on', self.udp_leak_hint())
        self.run_cmd('local-set', 'udp', 'US-A')
        self.assertIn('cca local udp 查看', self.udp_leak_hint())
        self.config({})
        self.assertEqual(self.udp_leak_hint(), '')


if __name__ == '__main__':
    unittest.main()
