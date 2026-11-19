#!/usr/bin/env python3
"""客户端的运行记录（cca-core runtime-record）：临时 HOME、假的 ps，不碰真实的账号和进程。

辅助程序问 ps 要进程的启动时间和命令行；这里给它一个假的 ps，照沙箱里的两个文件回答（测试改 self.started、self.command
就是改这两个文件）。账号表、config.json 是沙箱里真的文件。只在 macOS 上跑：账号专属的浏览器和桌面 App 只有 macOS 有，
Linux 上进程的启动时间也不问 ps（读 /proc）。"""
import json
import os
import plistlib
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from helper_bin import HELPER

# 假的 ps：-o pid=,lstart= 给启动时间，-o pid=,command= 给命令行（文件不在就是没有这个进程）
PS = '''#!/bin/sh
case "$*" in
  *lstart*) cat "$HOME/ps-lstart" 2>/dev/null ;;
  *command*) cat "$HOME/ps-command" 2>/dev/null ;;
  *) printf '%s\\n' "$0 $*" >> "$HOME/unexpected-commands.log"; exit 127 ;;
esac
exit 0
'''
# 会动到这台机器、会联网的命令：沙箱的 PATH 最前面放只会报错的替身（运行记录只该问 ps）
GUARDED = ('security', 'osascript', 'open', 'pbcopy', 'launchctl', 'ssh', 'scp', 'curl', 'git', 'nc', 'crontab', 'defaults', 'killall', 'pkill',
           'sudo', 'lsappinfo', 'networksetup', 'scutil', 'codesign', 'spctl', 'hdiutil', 'ditto', 'claude', 'codex', 'agy', 'mihomo')
GUARD = '#!/bin/sh\nprintf \'%s\\n\' "$0 $*" >> "$HOME/unexpected-commands.log"\nexit 127\n'
CHANGED = '客户端的运行记录、账号节点或启动条件已变化'


@unittest.skipUnless(sys.platform == 'darwin', '账号专属的浏览器和桌面 App 只有 macOS 有')
class RuntimeIsolation(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-runtime-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        (self.root / '.cca').mkdir()
        (self.root / 'bin').mkdir()
        for name, script in [('ps', PS)] + [(name, GUARD) for name in GUARDED]:
            (self.root / 'bin' / name).write_text(script)
            (self.root / 'bin' / name).chmod(0o755)
        stray = self.root / 'unexpected-commands.log'
        self.addCleanup(lambda: self.assertEqual(stray.read_text() if stray.exists() else '', '', '调到了没有准备假命令的外部命令'))
        self.env = {'HOME': str(self.root), 'CCA_HOME': str(self.root / '.cca'), 'CCA_TOOL': 'claude',
                    'PATH': '%s:/usr/bin:/bin' % (self.root / 'bin')}
        self.key = 'alice@example.com'
        (self.root / '.cca' / 'config.json').write_text(json.dumps({'version': 2, 'nodes': ['US-A', 'US-B']}))
        self.node('US-A')
        self.app = self.root / 'Test Browser.app'
        (self.app / 'Contents/MacOS').mkdir(parents=True)
        (self.app / 'Contents/MacOS/Test').write_text('test binary')
        (self.app / 'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleExecutable': 'Test'}))
        self.started = 1234567890
        self.command = ''
        self.args = self.launch('browser')
        self.account = Path(self.args[3]).parent   # 账号目录：资料目录的上一层，在沙箱的 ~/.cca/accounts 里
        self.assertEqual(self.account.parent, self.root / '.cca' / 'accounts')

    def node(self, name):
        """账号表里这个账号走哪个节点"""
        (self.root / '.cca' / 'accounts.json').write_text(json.dumps({self.key: name}))

    def helper(self, *argv):
        # 进程 123 现在的样子：没有启动时间就是这个进程已经不在了
        lstart = time.strftime('%a %b %d %H:%M:%S %Y', time.localtime(self.started)) if self.started else ''
        (self.root / 'ps-lstart').write_text('  123 %s\n' % lstart if lstart else '')
        (self.root / 'ps-command').write_text('  123 %s\n' % self.command)
        return subprocess.run([HELPER, *argv], env=self.env, stdin=subprocess.DEVNULL, capture_output=True, encoding='utf-8', timeout=30)

    def done(self, *argv):
        r = self.helper(*argv)
        self.assertEqual((r.returncode, r.stderr), (0, ''))
        return r.stdout

    def launch(self, kind):
        profile = self.done('private-profile', self.key, kind).rstrip('\n')
        if kind == 'browser':
            self.done('browser-profile', 'prepare', self.key)
        proxy = 'http://192.0.2.10:7901'
        self.command = str(self.app) + '/Contents/MacOS/Test --user-data-dir=' + profile
        self.command += ' --proxy-server=' + proxy
        if kind == 'app':
            self.command += ' --proxy-bypass-list=localhost'
        else:
            self.command += ' --webrtc-ip-handling-policy=disable_non_proxied_udp'
        return [self.key, kind, '123', profile, str(self.app), proxy, 'router', 'localhost', 'localhost']

    def record(self, action, args=None):
        self.assertEqual(self.done('runtime-record', action, *(args or self.args)), '')

    def refused(self, action='check', args=None, why=CHANGED):
        """拒绝：一句说明（why 是其中的一段）、退出码 1；不是参数不对、内部出错这类"""
        r = self.helper('runtime-record', action, *(args or self.args))
        self.assertEqual((r.returncode, r.stdout), (1, ''), r.stderr)
        self.assertIn(why, r.stderr)
        self.assertEqual(len(r.stderr.splitlines()), 1, r.stderr)
        self.assertNotIn('cca-core', r.stderr)

    def test_valid_receipts_for_both_clients_are_private(self):
        for kind in ('app', 'browser'):
            self.args = self.launch(kind)
            self.record('write')
            self.record('check')
            p = self.account / (kind + '-runtime.json')
            self.assertEqual(p.stat().st_mode & 0o777, 0o600)
            self.assertNotIn('command', json.loads(p.read_text()))

    def test_old_process_without_receipt_is_not_adopted(self):
        self.refused()

    def test_process_restart_and_pid_reuse_are_rejected(self):
        self.record('write')
        self.started += 1
        self.refused()
        self.started = 0
        self.refused(why='无法核验客户端进程或账号代理')

    def test_changed_proxy_node_route_or_binary_requires_restart(self):
        self.record('write')
        self.node('US-B')
        self.refused()
        self.node('US-A')
        for index, value in ((5, 'http://127.0.0.1:7901'), (6, 'local'), (7, '*'), (8, '*')):
            args = list(self.args)
            args[index] = value
            # 代理换了：进程的命令行里还是原来的；别的几项不在命令行里，和记下的对不上
            self.refused(args=args, why='客户端启动参数无法核验：--proxy-server' if index == 5 else CHANGED)
        # App 更新了（Info.plist 照样读得出来，只是大小、修改时间变了）：和记下的版本对不上。这一步是改写时加的：
        # 下面那一步写进去的不是 plist，走的是「读不出版本」，原来没有哪一步走到「版本变了」
        (self.app / 'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleExecutable': 'Test', 'CFBundleVersion': '2'}))
        self.refused()
        (self.app / 'Contents/Info.plist').write_text('new app version and build')
        self.refused(why='无法核验客户端程序版本')

    def test_command_must_match_actual_proxy_and_privacy_flags(self):
        valid = self.command
        for command, flag in ((valid.replace(':7901', ':7902'), 'proxy-server'), (valid + ' --proxy-server=http://127.0.0.1:7902', 'proxy-server'),
                              (valid.replace(' --webrtc-ip-handling-policy=disable_non_proxied_udp', ''), 'webrtc-ip-handling-policy'),
                              ('', 'user-data-dir')):
            self.command = command
            self.refused('write', why='客户端启动参数无法核验：--%s；' % flag)
        self.command = valid
        self.record('write')

    def test_process_must_belong_to_the_selected_installation(self):
        self.command = self.command.replace(str(self.app), '/another-installation/Test Browser.app')
        self.refused('write', why='客户端进程不属于当前选择的程序')

    def test_webrtc_url_exceptions_are_removed_before_launch_and_rejected_live(self):
        path = Path(self.args[3]) / 'Default/Preferences'
        data = json.loads(path.read_text())
        data['unrelated'] = {'keep': True}
        data['webrtc']['ip_handling_url'] = [{'url': '*', 'handling': 'default'}]
        path.write_text(json.dumps(data))
        self.refused('write', why='浏览器 WebRTC 偏好已变化或有 URL 例外')
        self.done('browser-profile', 'prepare', self.key)
        data = json.loads(path.read_text())
        self.assertEqual(data['webrtc']['ip_handling_url'], [])
        self.assertEqual(data['unrelated'], {'keep': True})
        self.record('write')

    def test_symlink_profile_is_rejected_without_touching_target(self):
        profile = Path(self.args[3])
        (profile / 'Default/Preferences').unlink()
        (profile / 'Default').rmdir()
        profile.rmdir()
        other = self.root / 'other-account'
        other.mkdir(mode=0o755)
        profile.symlink_to(other, target_is_directory=True)
        self.refused('write', why='客户端资料目录不能是符号链接：%s' % profile)
        self.assertEqual(other.stat().st_mode & 0o777, 0o755)

    def test_linked_receipt_is_not_trusted_or_written_through(self):
        self.record('write')
        receipt = self.account / 'browser-runtime.json'
        other = self.root / 'shared-record.json'
        receipt.rename(other)
        receipt.symlink_to(other)
        before = other.read_bytes()
        self.refused()
        self.record('write')
        self.assertFalse(receipt.is_symlink())
        self.assertEqual(other.read_bytes(), before)
        receipt.unlink()
        os.link(other, receipt)
        self.refused()

    def test_profile_path_cannot_be_replaced_by_another_account(self):
        other = self.root / 'another-profile'
        other.mkdir()
        args = list(self.args)
        args[3] = str(other)
        self.refused('write', args, why='客户端资料目录与账号不符')


if __name__ == '__main__':
    unittest.main()
