#!/usr/bin/env python3
"""Desktop launch isolation with dummy apps, environment, processes, and profiles."""
import json
import os
import signal
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest


SOURCE = (Path(__file__).resolve().parents[1] / 'cca').read_text()
ENV = SOURCE[SOURCE.index("INHERITED_RE='^"):SOURCE.index('\npyenv()')]
APP_OPEN = SOURCE[SOURCE.index('app_open() {'):SOURCE.index('\nclaude_app_first()')]
APP_PREPARE = SOURCE[SOURCE.index('codex_app_prepare() {'):SOURCE.index('\napp_open() {')]
APP_CHECKS = SOURCE[SOURCE.index('app_launch_lock() {'):SOURCE.index('\ncmd_app() {')]
APP_COMMAND = SOURCE[SOURCE.index('cmd_app() {'):SOURCE.index('\n# ---------------------------------------------------------------------------', SOURCE.index('cmd_app() {'))]
APP_PID = SOURCE[SOURCE.index('app_pid() {'):SOURCE.index('\nclaude_app_pid()')]
CHROME = SOURCE[SOURCE.index('chrome_pid() {'):SOURCE.index('\ncmd_web() {')]
REMOVE = SOURCE[SOURCE.index('cmd_rm() {'):SOURCE.index('\ncmd_list() {')]
RESTORE = SOURCE[SOURCE.index('cmd_restore() {'):SOURCE.index('\nusage() {')]
PY_COMMAND_STUB = '''py() {
 case "$1" in
  env) echo 'ACC_NAME=test ACC_LABEL=test ACC_NODE=test' ;;
  private-profile) [ "${PROFILE_RC:-0}" = 0 ] || return "$PROFILE_RC"; echo "$AUDIT_ROOT/desktop" ;;
  runtime-record)
   printf '%s\\n' "$*" >> "$AUDIT_ROOT/record.log"
   if [ "$2" = write ]; then return "${RECORD_WRITE_RC:-0}"; else return "${RECORD_CHECK_RC:-1}"; fi ;;
  codex-fallback-off) return "${FALLBACK_OFF_RC:-0}" ;;
  *) echo "unexpected py command: $1" >&2; return 9 ;;
 esac
}
'''


class AppIsolation(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-app-isolation-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.profile = self.root / 'desktop'
        self.profile.mkdir()
        self.env = {
            'HOME': str(self.root), 'PATH': str(self.bin) + ':/usr/bin:/bin:/usr/sbin:/sbin',
            'PYTHON': sys.executable, 'ACC_DIR': str(self.root), 'AUDIT_ROOT': str(self.root),
            'ACC_HOME': str(self.root / 'config'), 'ACC_MASTER': str(self.root / 'master'),
            'ACC_KEY': 'test@example.com', 'ACC_KIND': 'oauth',
            'ACC_PROXY': 'http://127.0.0.1:7999', 'CCA_NO_PROXY': 'localhost',
            'CCA_PROXY_BYPASS': 'localhost', 'CCA_HOME': str(self.root),
            'TOOL': 'claude', 'ROUTE': 'router',
        }

    def executable(self, name, text):
        p = self.bin / name
        p.write_text('#!/bin/bash\n' + text)
        p.chmod(0o755)

    def shell(self, text, extra=None):
        return subprocess.run(['/bin/bash'], input=text, text=True, capture_output=True,
                              env=dict(self.env, **(extra or {})), timeout=10)

    def test_desktop_filters_inherited_credentials_for_both_tools(self):
        self.executable('open', '''exec "$PYTHON" -I -c 'import json,os,sys
keys = ["ANTHROPIC_BASE_URL", "CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_SECURESTORAGE_CONFIG_DIR", "CODEX_ACCESS_TOKEN", "CODEX_SPARKLE_ENABLED", "OPENAI_API_KEY", "CODEX_CA_CERTIFICATE", "HTTPS_PROXY", "CODEX_HOME", "CLAUDE_CONFIG_DIR"]
print(json.dumps({"env": {k: os.environ.get(k) for k in keys}, "args": sys.argv[1:]}))' "$@"
''')
        for tool, dirty in (
            ('claude', {'ANTHROPIC_BASE_URL': 'https://sentinel.invalid', 'CLAUDE_CODE_OAUTH_TOKEN': 'dummy',
                        'CLAUDE_SECURESTORAGE_CONFIG_DIR': '/another-account'}),
            ('codex', {'CODEX_ACCESS_TOKEN': 'dummy', 'OPENAI_API_KEY': 'dummy', 'CODEX_CA_CERTIFICATE': '/test-ca',
                       'CODEX_SPARKLE_ENABLED': 'true'}),
        ):
            with self.subTest(tool=tool):
                setup = 'tool() { "' + tool + '_$1"; }\n'
                if tool == 'codex':
                    setup += 'INHERITED_RE="$CODEX_INHERITED_RE"\n'
                r = self.shell(ENV + '\n' + setup + APP_OPEN + '\n' + tool + '_app_open /dummy.app "$AUDIT_ROOT/desktop"\n', dirty)
                self.assertEqual(r.returncode, 0, r.stderr)
                data = json.loads(r.stdout)
                for key in dirty:
                    if key not in ('CODEX_CA_CERTIFICATE', 'CLAUDE_SECURESTORAGE_CONFIG_DIR'):
                        self.assertIsNone(data['env'][key])
                self.assertEqual(data['env']['HTTPS_PROXY'], self.env['ACC_PROXY'])
                self.assertIn('--user-data-dir=' + str(self.profile), data['args'])
                if tool == 'codex':
                    self.assertEqual(data['env']['CODEX_CA_CERTIFICATE'], '/test-ca')
                    self.assertEqual(data['env']['CODEX_HOME'], self.env['ACC_HOME'])
                    self.assertIn('CODEX_ELECTRON_USER_DATA_PATH=' + str(self.profile), data['args'])
                    self.assertIn('CODEX_SPARKLE_ENABLED=false', data['args'])
                else:
                    self.assertEqual(data['env']['CLAUDE_CONFIG_DIR'], self.env['ACC_HOME'])
                    self.assertEqual(data['env']['CLAUDE_SECURESTORAGE_CONFIG_DIR'], self.env['ACC_HOME'])
                    self.assertIn('CLAUDE_SECURESTORAGE_CONFIG_DIR=' + self.env['ACC_HOME'], data['args'])

    def test_proxy_fallback_unknown_or_declined_stops_before_login(self):
        stubs = '''need_bin() { :; }
py() { return "${FALLBACK_OFF_RC:-1}"; }
die() { echo "$*" >&2; exit 1; }
warn() { :; }
interactive() { return 1; }
consent() { return 1; }
ensure_login() { echo logged-in; }
'''
        for flags in ('()', '(--disable system_proxy_fallback)'):
            r = self.shell(stubs + 'ACC_FLAGS=' + flags + '\n' + APP_PREPARE + '\ncodex_app_prepare\n')
            self.assertNotEqual(r.returncode, 0)
            self.assertNotIn('logged-in', r.stdout)
        r = self.shell(stubs + 'ACC_FLAGS=()\n' + APP_PREPARE + '\ncodex_app_prepare\n', {'FALLBACK_OFF_RC': '0'})
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(r.stdout.strip(), 'logged-in')

    def process_fixtures(self):
        self.executable('ps', 'if [ "$*" = "-axo pid=" ]; then echo 1; else printf "101 1\\n102 101\\n103 102\\n200 1\\n"; fi\n')
        self.executable('lsof', '''case " $* " in *" -p 101,102,103 "*) ;; *) exit 9 ;; esac
[ -z "${OPEN_FILE:-}" ] || printf 'p103\\nn%s\\n' "$OPEN_FILE"
''')

    def test_profile_process_lookup_accepts_both_flag_forms_without_prefix_matches(self):
        self.executable('pgrep', '''exec "$PYTHON" -I -c 'import os,re,sys
sys.exit(0 if re.search(sys.argv[1], os.environ["AUDIT_COMMAND"]) and not print("101") else 1)' "$2"
''')
        for name in ('Claude', 'Google Chrome'):
            prefix = '/Applications/Test.app/Contents/MacOS/' + name + ' '
            for argument in ('--user-data-dir=' + str(self.profile), '--user-data-dir ' + str(self.profile),
                             '--user-data-dir   ' + str(self.profile), '--user-data-dir=' + str(self.profile) + '/'):
                with self.subTest(name=name, argument=argument):
                    command = prefix + argument + ' --proxy-server=http://127.0.0.1:7999'
                    result = self.shell(APP_PID + '\napp_pid "$AUDIT_NAME" "$AUDIT_ROOT/desktop"\n',
                                        {'AUDIT_NAME': name, 'AUDIT_COMMAND': command})
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(result.stdout.strip(), '101', result.stderr)
            for other in ('desktop-other', 'desktopX', 'desktop/child'):
                result = self.shell(APP_PID + '\napp_pid "$AUDIT_NAME" "$AUDIT_ROOT/desktop"\n',
                                    {'AUDIT_NAME': name, 'AUDIT_COMMAND': prefix + '--user-data-dir=' + str(self.root / other)})
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertEqual(result.stdout, '', result.stderr)

    def test_process_lookup_preserves_errors_and_rejects_empty_success(self):
        self.executable('pgrep', 'exit "$PGREP_RC"\n')
        for code, expected in ((0, 2), (1, 1), (2, 2), (3, 3), (127, 127)):
            with self.subTest(code=code):
                result = self.shell(APP_PID + '\napp_pid Claude "$AUDIT_ROOT/desktop"\n', {'PGREP_RC': str(code)})
                self.assertEqual(result.returncode, expected, result.stderr)
                self.assertEqual(result.stdout, '')

    def test_destructive_commands_stop_when_process_query_fails(self):
        self.executable('pgrep', 'exit 3\n')
        stubs = '''need_tty() { :; }
use_tool() { TOOL="$1"; }
tool() { app_pid Claude "$2"; }
die() { echo "$*" >&2; exit 1; }
py() {
 case "$1" in
  resolve|keys) echo test@example.com ;;
  env) echo 'ACC_NAME=test ACC_LABEL=test' ;;
  *) touch "$AUDIT_ROOT/mutated"; return 9 ;;
 esac
}
confirm() { touch "$AUDIT_ROOT/prompted"; return 1; }
ask() { touch "$AUDIT_ROOT/prompted"; }
logout_account() { touch "$AUDIT_ROOT/mutated"; }
rm() { touch "$AUDIT_ROOT/mutated"; }
'''
        for function, command in ((REMOVE, 'cmd_rm test'), (RESTORE, 'cmd_restore')):
            with self.subTest(command=command):
                result = self.shell(stubs + APP_PID + '\n' + function + '\n' + command + '\n',
                                    {'ACC_DESKTOP_DIR': str(self.profile), 'APP_LABEL': 'Dummy App'})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('无法查询 Dummy App 进程', result.stderr)
                self.assertFalse((self.root / 'prompted').exists())
                self.assertFalse((self.root / 'mutated').exists())

    def test_browser_process_query_failure_never_prepares_or_opens_profile(self):
        self.executable('pgrep', 'exit 3\n')
        stubs = '''has_chrome() { return 0; }
chrome_app_path() { echo /dummy.app; }
fail_warn() { echo "$*" >&2; }
py() {
 case "$1" in
  private-profile) echo "$AUDIT_ROOT/desktop" ;;
  browser-profile) touch "$AUDIT_ROOT/browser-prepared" ;;
  *) echo "unexpected py: $*" >&2; return 9 ;;
 esac
}
'''
        script = stubs + APP_PID + '\n' + CHROME + '''
chrome_open_url() { touch "$AUDIT_ROOT/browser-opened"; }
open_chrome https://example.com
'''
        result = self.shell(script)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('无法查询 Chrome 进程', result.stderr)
        self.assertFalse((self.root / 'browser-prepared').exists())
        self.assertFalse((self.root / 'browser-opened').exists())

    def test_browser_query_failure_after_launch_does_not_deliver_target_url(self):
        self.executable('pgrep', '''if [ ! -e "$AUDIT_ROOT/queried" ]; then
 touch "$AUDIT_ROOT/queried"
 exit 1
fi
exit 3
''')
        stubs = '''has_chrome() { return 0; }
chrome_app_path() { echo /dummy.app; }
fail_warn() { echo "$*" >&2; }
py() {
 case "$1" in
  private-profile) echo "$AUDIT_ROOT/desktop" ;;
  browser-profile) touch "$AUDIT_ROOT/browser-prepared" ;;
  runtime-record) touch "$AUDIT_ROOT/browser-recorded" ;;
  *) echo "unexpected py: $*" >&2; return 9 ;;
 esac
}
'''
        script = stubs + APP_PID + '\n' + CHROME + '''
chrome_open_url() { echo "$2" >> "$AUDIT_ROOT/browser-opened"; }
open_chrome https://example.com
'''
        result = self.shell(script)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('无法查询本次 Chrome 进程', result.stderr)
        self.assertTrue((self.root / 'browser-prepared').exists())
        self.assertEqual((self.root / 'browser-opened').read_text().splitlines(), ['about:blank'])
        self.assertFalse((self.root / 'browser-recorded').exists())

    def test_app_process_query_failure_stops_each_launch_stage(self):
        self.process_fixtures()
        stubs = '''tool() {
 case "$1" in
  app_path) echo /dummy.app ;;
  app_pid)
   n=$(cat "$AUDIT_ROOT/query-count" 2>/dev/null || echo 0)
   n=$((n + 1)); echo "$n" > "$AUDIT_ROOT/query-count"
   [ "$n" -lt "$QUERY_FAIL_AT" ] && return 1
   return 3 ;;
  app_prepare) touch "$AUDIT_ROOT/app-prepared" ;;
  app_open) touch "$AUDIT_ROOT/app-opened" ;;
  *) echo "unexpected tool: $*" >&2; return 9 ;;
 esac
}
pick_account() { echo test@example.com; }
activate_pid() { touch "$AUDIT_ROOT/app-activated"; }
sync_if_changed() { :; }
launchers_sync() { :; }
preflight() { :; }
prepare_account() { :; }
gui_step() { :; }
die() { echo "$*" >&2; exit 1; }
kill() { touch "$AUDIT_ROOT/killed-existing"; }
'''
        script = stubs + PY_COMMAND_STUB + APP_CHECKS + APP_COMMAND + '\ncmd_app test\n'
        for stage in (1, 2, 3):
            with self.subTest(stage=stage):
                for filename in ('query-count', 'app-prepared', 'app-opened'):
                    (self.root / filename).unlink(missing_ok=True)
                result = self.shell(script, {'ACC_DESKTOP_DIR': str(self.profile), 'APP_LABEL': 'Dummy App',
                                             'QUERY_FAIL_AT': str(stage)})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('无法查询', result.stderr)
                self.assertEqual((self.root / 'app-prepared').exists(), stage > 1)
                self.assertEqual((self.root / 'app-opened').exists(), stage > 2)
                self.assertFalse((self.root / 'app-activated').exists())
                self.assertFalse((self.root / 'killed-existing').exists())
                self.assertEqual(self.shell(APP_CHECKS + '\napp_launch_lock\n').returncode, 0)

    def test_stale_profile_is_not_evidence_but_descendant_open_file_is(self):
        self.process_fixtures()
        (self.profile / 'Local State').write_text('{}')
        body = APP_CHECKS + '\napp_profile_in_use 101 "$AUDIT_ROOT/desktop"\n'
        for filename in ('', str(self.root / 'desktop-other' / 'Cookies')):
            with self.subTest(filename=filename):
                self.assertNotEqual(self.shell(body, {'OPEN_FILE': filename}).returncode, 0)
        self.assertEqual(self.shell(body, {'OPEN_FILE': str(self.profile / 'Cookies')}).returncode, 0)

    def test_lock_serializes_launches_and_releases_on_process_exit(self):
        script = APP_CHECKS + '''
app_launch_lock || exit 4
: > "$AUDIT_ROOT/ready"
while [ ! -e "$AUDIT_ROOT/release" ]; do sleep 0.02; done
'''
        first = subprocess.Popen(['/bin/bash'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=self.env)
        self.addCleanup(lambda: first.poll() is None and first.kill())
        first.stdin.write(script)
        first.stdin.close()
        deadline = time.monotonic() + 5
        while not (self.root / 'ready').exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        self.assertTrue((self.root / 'ready').exists())
        r = self.shell(APP_CHECKS + '\napp_launch_lock\n')
        self.assertNotEqual(r.returncode, 0)
        (self.root / 'release').touch()
        self.assertEqual(first.wait(timeout=5), 0)
        first.stdout.close()
        first.stderr.close()
        self.assertEqual(self.shell(APP_CHECKS + '\napp_launch_lock\n').returncode, 0)

    def test_command_rejects_stale_profile_and_closes_launch_fd(self):
        self.process_fixtures()
        (self.profile / 'Local State').write_text('{}')
        stubs = '''tool() {
 case "$1" in
  app_path) echo /dummy.app ;;
  app_pid) [ ! -e "$AUDIT_ROOT/opened" ] || echo 101 ;;
  app_prepare|app_first) : ;;
  app_open)
   "$PYTHON" -I -c 'import os,sys
try: os.fstat(9)
except OSError: sys.exit(0)
sys.exit(9)' || exit 9
   touch "$AUDIT_ROOT/opened" ;;
 esac
}
pick_account() { echo test@example.com; }
activate_pid() { :; }
sync_if_changed() { :; }
launchers_sync() { :; }
preflight() { :; }
prepare_account() { :; }
gui_step() { :; }
gui_result() { echo "result:$1"; }
info() { :; }
okay() { :; }
route_note() { echo route; }
die() { echo "$*" >&2; exit 1; }
sleep() { :; }
kill() { echo "$*" >> "$AUDIT_ROOT/stopped"; }
'''
        script = stubs + PY_COMMAND_STUB + APP_CHECKS + APP_COMMAND + '\ncmd_app test\n'
        r = self.shell(script, {'ACC_DESKTOP_DIR': str(self.profile), 'APP_LABEL': 'Dummy App'})
        self.assertNotEqual(r.returncode, 0)
        self.assertIn('无法确认', r.stderr)
        self.assertNotIn('result:opened', r.stdout)
        self.assertEqual((self.root / 'stopped').read_text().splitlines(), ['101', '102', '103'])
        (self.root / 'opened').unlink()
        r = self.shell(script, {'ACC_DESKTOP_DIR': str(self.profile), 'APP_LABEL': 'Dummy App', 'OPEN_FILE': str(self.profile / 'Cookies')})
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn('result:opened', r.stdout)
        self.assertIn('runtime-record write test@example.com app 101', (self.root / 'record.log').read_text())
        (self.root / 'opened').unlink()
        (self.root / 'stopped').unlink()
        r = self.shell(script, {'ACC_DESKTOP_DIR': str(self.profile), 'APP_LABEL': 'Dummy App',
                               'OPEN_FILE': str(self.profile / 'Cookies'), 'RECORD_WRITE_RC': '1'})
        self.assertNotEqual(r.returncode, 0)
        self.assertNotIn('result:opened', r.stdout)
        self.assertEqual((self.root / 'stopped').read_text().splitlines(), ['101', '102', '103'])


    def test_existing_instance_is_verified_without_ever_killing_it(self):
        self.process_fixtures()
        setup = """
tool() { case "$1" in app_path) echo /dummy.app ;; app_pid) echo 101 ;; *) echo unexpected >&2; exit 9 ;; esac; }
pick_account() { echo test@example.com; }
activate_pid() { echo activated; }
sync_if_changed() { :; }
launchers_sync() { :; }
preflight() { touch "$AUDIT_ROOT/preflight"; return "${PREFLIGHT_RC:-0}"; }
prepare_account() { touch "$AUDIT_ROOT/prepared"; return "${PREPARE_RC:-0}"; }
gui_step() { :; }
info() { :; }
gui_result() { echo "result:$1"; }
die() { echo "$*" >&2; exit 1; }
kill() { touch "$AUDIT_ROOT/killed-existing"; }
"""
        body = setup + PY_COMMAND_STUB + APP_CHECKS + APP_COMMAND + '\ncmd_app test\n'
        extra = {'ACC_DESKTOP_DIR': str(self.profile), 'APP_LABEL': 'Dummy App'}
        refused = self.shell(body, extra)
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn('已有', refused.stderr)
        self.assertNotIn('activated', refused.stdout)
        self.assertFalse((self.root / 'killed-existing').exists())
        self.assertTrue((self.root / 'preflight').exists())
        self.assertTrue((self.root / 'prepared').exists())
        missing_record = self.shell(body, dict(extra, OPEN_FILE=str(self.profile / 'Cookies')))
        self.assertNotEqual(missing_record.returncode, 0)
        self.assertIn('启动记录', missing_record.stderr)
        self.assertNotIn('activated', missing_record.stdout)
        self.assertFalse((self.root / 'killed-existing').exists())
        accepted = self.shell(body, dict(extra, OPEN_FILE=str(self.profile / 'Cookies'), RECORD_CHECK_RC='0'))
        self.assertEqual(accepted.returncode, 0, accepted.stderr)
        self.assertIn('result:activated', accepted.stdout)
        self.assertFalse((self.root / 'killed-existing').exists())
        for failure in ({'PREFLIGHT_RC': '1'}, {'PREPARE_RC': '1'}, {'PROFILE_RC': '1'},
                        {'TOOL': 'codex', 'FALLBACK_OFF_RC': '1'}):
            with self.subTest(failure=failure):
                refused = self.shell(body, dict(extra, OPEN_FILE=str(self.profile / 'Cookies'),
                                                RECORD_CHECK_RC='0', **failure))
                self.assertNotEqual(refused.returncode, 0)
                self.assertNotIn('activated', refused.stdout)
                self.assertFalse((self.root / 'killed-existing').exists())

    def test_interrupted_new_launch_is_cleaned_before_pid_is_recorded(self):
        self.process_fixtures()
        setup = """
tool() {
 case "$1" in
  app_path) echo /dummy.app ;;
  app_pid) [ ! -e "$AUDIT_ROOT/opened" ] || echo 101 ;;
  app_prepare) : ;;
  app_open)
   touch "$AUDIT_ROOT/opened"
   while :; do sleep 0.05; done ;;
 esac
}
pick_account() { echo test@example.com; }
activate_pid() { :; }
sync_if_changed() { :; }
launchers_sync() { :; }
preflight() { :; }
prepare_account() { :; }
gui_step() { :; }
gui_result() { echo "result:$1"; }
info() { :; }
okay() { :; }
die() { echo "$*" >&2; exit 1; }
kill() { echo "$*" >> "$AUDIT_ROOT/stopped"; }
"""
        env = dict(self.env, ACC_DESKTOP_DIR=str(self.profile), APP_LABEL='Dummy App')
        process = subprocess.Popen(['/bin/bash', '-c', setup + PY_COMMAND_STUB + APP_CHECKS + APP_COMMAND + '\ncmd_app test'],
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env)
        self.addCleanup(lambda: process.poll() is None and process.kill())
        deadline = time.monotonic() + 5
        while not (self.root / 'opened').exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        self.assertTrue((self.root / 'opened').exists())
        process.send_signal(signal.SIGTERM)
        stdout, stderr = process.communicate(timeout=5)
        self.assertEqual(process.returncode, 143, stderr)
        self.assertNotIn('result:opened', stdout)
        self.assertEqual((self.root / 'stopped').read_text().splitlines(), ['101', '102', '103'])
        self.assertEqual(self.shell(APP_CHECKS + '\napp_launch_lock\n').returncode, 0)

    def test_cleanup_excludes_all_processes_present_before_launch(self):
        self.process_fixtures()
        setup = """
APP_LAUNCH_ACTIVE=1
APP_LAUNCH_DIR="$AUDIT_ROOT/desktop"
APP_LAUNCH_NEW_PID=101
APP_LAUNCH_BEFORE='1 101 102 103'
tool() { echo 101; }
kill() { touch "$AUDIT_ROOT/killed-existing"; }
"""
        result = self.shell(APP_CHECKS + '\n' + setup + '\napp_launch_cleanup\n')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.root / 'killed-existing').exists())

    def test_actual_existing_process_holding_profile_file_is_accepted(self):
        holding = self.profile / 'Cookies'
        code = 'import pathlib,sys,time; f=open(sys.argv[1], "w"); pathlib.Path(sys.argv[2]).touch(); time.sleep(20)'
        process = subprocess.Popen([sys.executable, '-I', '-c', code, str(holding), str(self.root / 'ready')])
        self.addCleanup(lambda: process.poll() is None and process.kill())
        deadline = time.monotonic() + 5
        while not (self.root / 'ready').exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        self.assertTrue((self.root / 'ready').exists())
        result = self.shell(APP_CHECKS + '\napp_profile_in_use ' + str(process.pid) + ' "$AUDIT_ROOT/desktop"\n')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIsNone(process.poll())
        process.terminate()
        process.wait(timeout=5)


if __name__ == '__main__':
    unittest.main()
