#!/usr/bin/env python3
"""Client launch receipts: temporary state and process doubles, no real accounts."""
import contextlib
import io
import json
import os
import plistlib
from pathlib import Path
import tempfile
import unittest
from unittest import mock


CCA = Path(__file__).resolve().parents[1] / 'cca'
SOURCE = CCA.read_text().split("PY_SRC=$(cat <<'PY'\n", 1)[1].split('\nPY\n)', 1)[0]
DEFINITIONS = SOURCE.split('\nif len(sys.argv) < 2', 1)[0]


class RuntimeIsolation(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-runtime-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        with mock.patch.dict(os.environ, {'HOME': str(self.root), 'CCA_HOME': str(self.root / '.cca'),
                                         'CCA_TOOL': 'claude'}, clear=True):
            self.ns = {}
            exec(compile(DEFINITIONS, str(CCA), 'exec'), self.ns)
        self.key = 'alice@example.com'
        self.table = {self.key: 'US-A'}
        self.ns['ctx'] = lambda: ({}, self.table, {})
        self.account = Path(self.ns['adir'](self.key))
        self.app = self.root / 'Test Browser.app'
        (self.app / 'Contents/MacOS').mkdir(parents=True)
        (self.app / 'Contents/MacOS/Test').write_text('test binary')
        (self.app / 'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleExecutable': 'Test'}))
        self.started = 1234567890
        self.ns['proc_starts'] = lambda pids: {123: self.started} if self.started else {}
        self.ns['ps_rows'] = lambda *a, **k: [(123, self.command)]
        self.args = self.launch('browser')

    def launch(self, kind):
        profile = self.ns['profile_path'](self.key, kind)
        if kind == 'browser':
            self.ns['browser_profile'](self.key, True)
        proxy = 'http://192.0.2.10:7901'
        self.command = str(self.app) + '/Contents/MacOS/Test --user-data-dir=' + profile
        self.command += ' --proxy-server=' + proxy
        if kind == 'app':
            self.command += ' --proxy-bypass-list=localhost'
        else:
            self.command += ' --webrtc-ip-handling-policy=disable_non_proxied_udp'
        return [self.key, kind, '123', profile, str(self.app), proxy, 'router', 'localhost', 'localhost']

    def record(self, action, args=None):
        return self.ns['cmd_runtime_record'](action, *(args or self.args))

    def refused(self, action='check', args=None):
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            self.record(action, args)

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
        self.refused()

    def test_changed_proxy_node_route_or_binary_requires_restart(self):
        self.record('write')
        self.table[self.key] = 'US-B'
        self.refused()
        self.table[self.key] = 'US-A'
        for index, value in ((5, 'http://127.0.0.1:7901'), (6, 'local'), (7, '*'), (8, '*')):
            args = list(self.args)
            args[index] = value
            self.refused(args=args)
        (self.app / 'Contents/Info.plist').write_text('new app version and build')
        self.refused()

    def test_command_must_match_actual_proxy_and_privacy_flags(self):
        valid = self.command
        for command in (valid.replace('7901', '7902'), valid + ' --proxy-server=http://127.0.0.1:7902',
                        valid.replace(' --webrtc-ip-handling-policy=disable_non_proxied_udp', ''), ''):
            self.command = command
            self.refused('write')
        self.command = valid
        self.record('write')

    def test_process_must_belong_to_the_selected_installation(self):
        self.command = self.command.replace(str(self.app), '/another-installation/Test Browser.app')
        self.refused('write')

    def test_webrtc_url_exceptions_are_removed_before_launch_and_rejected_live(self):
        path = Path(self.args[3]) / 'Default/Preferences'
        data = json.loads(path.read_text())
        data['unrelated'] = {'keep': True}
        data['webrtc']['ip_handling_url'] = [{'url': '*', 'handling': 'default'}]
        path.write_text(json.dumps(data))
        self.refused('write')
        self.ns['browser_profile'](self.key, True)
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
        self.refused('write')
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
        self.refused('write', args)


if __name__ == '__main__':
    unittest.main()
