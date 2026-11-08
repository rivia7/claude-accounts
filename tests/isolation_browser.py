#!/usr/bin/env python3
"""Browser/login launch boundaries with shell doubles; never opens an application."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SOURCE = (Path(__file__).resolve().parents[1] / 'cca').read_text()


def function(name):
    start = SOURCE.index('\n' + name + '() {') + 1
    end = SOURCE.index('\n}', start) + 2
    return SOURCE[start:end]


class BrowserIsolation(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-browser-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def run_shell(self, functions, code):
        prologue = '''
fail_warn() { printf 'FAIL:%s:%s\\n' "$1" "$2" >&2; }
info() { printf 'INFO:%s\\n' "$*"; }
gui_result() { printf 'RESULT:%s\\n' "$*"; }
gui_step() { :; }
die() { echo "$*" >&2; exit 1; }
'''
        return subprocess.run(
            ['/bin/bash', '-c', prologue + '\n'.join(function(n) for n in functions) + '\n' + code],
            cwd=self.root, env={'HOME': str(self.root), 'PATH': '/usr/bin:/bin',
                               'ACC_CHROME_DIR': str(self.root / 'browser'),
                               'ACC_PROXY': 'http://192.0.2.10:7901'},
            text=True, capture_output=True, timeout=10)

    def test_missing_chrome_never_opens_default_browser(self):
        r = self.run_shell(['open_chrome'], '''
has_chrome() { return 1; }
open() { touch unexpected-open; }
open_chrome https://example.com
''')
        self.assertNotEqual(r.returncode, 0)
        self.assertIn('no_chrome', r.stderr)
        self.assertFalse((self.root / 'unexpected-open').exists())
        self.assertFalse((self.root / 'browser').exists())

    def test_browser_receives_private_directory_and_account_proxy(self):
        r = self.run_shell(['open_chrome'], '''
has_chrome() { return 0; }
open() { printf '%s\\n' "$@" > open-args; }
open_chrome 'https://example.com/?a=b&c=d'
''')
        self.assertEqual(r.returncode, 0, r.stderr)
        args = (self.root / 'open-args').read_text().splitlines()
        self.assertIn('--user-data-dir=' + str(self.root / 'browser'), args)
        self.assertIn('--proxy-server=http://192.0.2.10:7901', args)
        self.assertEqual(args[-1], 'https://example.com/?a=b&c=d')
        self.assertEqual((self.root / 'browser').stat().st_mode & 0o777, 0o700)

    def test_browser_open_failure_is_propagated(self):
        r = self.run_shell(['open_chrome'], '''
has_chrome() { return 0; }
open() { return 7; }
open_chrome https://example.com
''')
        self.assertEqual(r.returncode, 7)

    def test_browser_address_cannot_override_launch_flags(self):
        r = self.run_shell(['open_chrome'], '''
has_chrome() { return 0; }
open() { touch unexpected-open; }
open_chrome '--proxy-server=http://example.invalid:8000'
''')
        self.assertNotEqual(r.returncode, 0)
        self.assertIn('invalid_url', r.stderr)
        self.assertFalse((self.root / 'unexpected-open').exists())

    def test_unsupported_codex_login_does_not_use_cli_fallback(self):
        r = self.run_shell(['codex_login'], '''
has_chrome() { return 0; }
child_env() { :; }
pyenv() { return 3; }
CODEX_BIN=/usr/bin/touch
ACC_FLAGS=(unexpected-cli-login)
codex_login
''')
        self.assertNotEqual(r.returncode, 0)
        self.assertIn('login_unsupported', r.stderr)
        self.assertFalse((self.root / 'unexpected-cli-login').exists())

    def test_login_without_chrome_stops_before_authentication(self):
        for name in ('claude_login', 'codex_login'):
            with self.subTest(name=name):
                r = self.run_shell([name], '''
has_chrome() { return 1; }
child_env() { touch unexpected-auth; }
''' + name)
                self.assertNotEqual(r.returncode, 0)
                self.assertFalse((self.root / 'unexpected-auth').exists())

    def web_setup(self):
        return '''
pick_account() { echo alice@example.com; }
py() { echo 'ACC_KIND=official ACC_KEY=alice@example.com ACC_NAME=alice ACC_EMAIL=alice@example.com ACC_NODE=US-A'; }
has_chrome() { return 0; }
sync_if_changed() { :; }
prepare_account() { :; }
interactive() { return 1; }
TRACE_HOST=example.com
TOOL_BIN=unused
'''

    def test_web_does_not_skip_preflight_or_open_after_failure(self):
        r = self.run_shell(['cmd_web'], self.web_setup() + '''
preflight() { printf '%s:%s' "${CCA_SKIP_CHECK:-unset}" "$1" > preflight; return 1; }
open_chrome() { touch unexpected-open; }
cmd_web alice
''')
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual((self.root / 'preflight').read_text(), 'unset:app')
        self.assertFalse((self.root / 'unexpected-open').exists())

    def test_web_does_not_report_success_after_open_failure(self):
        r = self.run_shell(['cmd_web'], self.web_setup() + '''
preflight() { return 0; }
open_chrome() { return 7; }
cmd_web alice
''')
        self.assertNotEqual(r.returncode, 0)
        self.assertNotIn('RESULT:opened', r.stdout)


if __name__ == '__main__':
    unittest.main()
