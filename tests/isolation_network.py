#!/usr/bin/env python3
"""Account network boundaries: temporary files and fake router/curl, never network access."""
import ast
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock


CCA = Path(__file__).resolve().parents[1] / 'cca'
SOURCE = CCA.read_text()


def function(name):
    start = SOURCE.index('\n' + name + '() {') + 1
    return SOURCE[start:SOURCE.index('\n}', start) + 2]


class NetworkIsolation(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-network-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def run_shell(self, names, code, extra=None):
        env = {'HOME': str(self.root), 'PATH': '/usr/bin:/bin',
               'ACC_PROXY': 'http://192.0.2.10:7901', 'ACC_PORT': '7901',
               'ACC_KEY': 'alice@example.com', 'ACC_KIND': 'oauth',
               'PROXY_HOST': '192.0.2.10', 'RDIR': '/fake/cca',
               'NO_PROXY': '*', 'no_proxy': '*', 'HTTPS_PROXY': 'http://example.invalid:9999',
               'ALL_PROXY': 'http://example.invalid:9998'}
        env.update(extra or {})
        prologue = '''
fail_warn() { printf 'FAIL:%s:%s\\n' "$1" "$2" >&2; }
warn() { printf 'WARN:%s\\n' "$*" >&2; }
info() { :; }
load_router() { :; }
router_ready() { return 0; }
nodes_follow() { return 1; }
allow_reload() { return 1; }
port_open() { touch checked-port; return 0; }
core_nodes_hook() { touch core-hook; }
'''
        return subprocess.run(['/bin/bash', '-c', prologue + '\n'.join(function(n) for n in names) + '\n' + code],
                              cwd=self.root, env=env, text=True, capture_output=True, timeout=10)

    def test_router_transport_and_selection_failures_stop_before_port_check(self):
        for rc in (1, 3, 255):
            with self.subTest(rc=rc):
                result = self.run_shell(['router_ensure', 'route_router'],
                                        'rsh() { return %d; }\nroute_router' % rc)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('sync_failed', result.stderr)
                self.assertFalse((self.root / 'checked-port').exists())
                self.assertFalse((self.root / 'core-hook').exists())

    def test_missing_router_cannot_use_an_unverified_open_port(self):
        result = self.run_shell(['router_ensure', 'route_router'], '''
router_ready() { return 1; }
rsh() { touch unexpected-ssh; }
route_router
''')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / 'checked-port').exists())
        self.assertFalse((self.root / 'unexpected-ssh').exists())

    def recovery_script(self, *, rename=True, push=0, reload=0, final=0):
        return '''
rsh() {
    if [ -f attempt ]; then echo again >> attempt; return %d; fi
    echo first > attempt; return %d
}
nodes_follow() { return %d; }
router_push() { touch pushed; return %d; }
allow_reload() { return 0; }
router_reload() { touch reloaded; return %d; }
py() { echo 'ACC_NODE=US-B'; }
router_ensure
''' % (final, 1 if rename else 3, 0 if rename else 1, push, reload)

    def test_renamed_node_requires_successful_deployment_and_recheck(self):
        result = self.run_shell(['router_ensure'], self.recovery_script())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.root / 'pushed').exists())
        self.assertEqual((self.root / 'attempt').read_text().splitlines(), ['first', 'again'])

    def test_failed_deployment_cannot_be_reported_as_recovered(self):
        result = self.run_shell(['router_ensure'], self.recovery_script(push=7))
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((self.root / 'attempt').read_text().splitlines(), ['first'])

    def test_successful_reload_requires_a_successful_recheck(self):
        for final in (0, 255):
            with self.subTest(final=final):
                (self.root / 'attempt').unlink(missing_ok=True)
                result = self.run_shell(['router_ensure'], self.recovery_script(rename=False, final=final))
                self.assertEqual(result.returncode == 0, final == 0, result.stderr)
                self.assertTrue((self.root / 'reloaded').exists())
                self.assertEqual((self.root / 'attempt').read_text().splitlines(), ['first', 'again'])

    def test_usage_route_stays_empty_on_router_failure(self):
        result = self.run_shell(['usage_route'], '''
want_local() { return 1; }
router_ensure() { return 1; }
USAGE_HOST=stale
usage_route; rc=$?
printf '%s' "$USAGE_HOST" > usage-host
exit "$rc"
''')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((self.root / 'usage-host').read_text(), '')

    def test_unverified_usage_route_keeps_port_diagnosis_without_authentication(self):
        for opened in (False, True):
            with self.subTest(opened=opened):
                result = self.run_shell(['usage_route', 'usage_one'], '''
want_local() { return 1; }
router_ensure() { return 1; }
port_open() { return %d; }
py() { [ "$1" = email ] && { echo alice@example.com; return 0; }; touch unexpected-query; }
child_env() { touch unexpected-query; }
lower() { printf '%%s' "$1"; }
usage_route || :
ACC_PROXY="${USAGE_HOST:+http://${USAGE_HOST}:${ACC_PORT}}"
usage_one usage-result
''' % (0 if opened else 1))
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual((self.root / 'usage-result.skip').read_text(), 'route\t\n' if opened else 'port\t7901\n')
                self.assertFalse((self.root / 'unexpected-query').exists())

    def test_check_does_not_probe_unverified_router(self):
        result = self.run_shell(['cmd_check'], '''
py() {
    case "$1" in
        keys) echo alice@example.com ;;
        env) echo 'ACC_PORT=7901 ACC_KIND=oauth' ;;
    esac
}
sync_if_changed() { :; }
want_local() { return 1; }
router_ensure() { return 1; }
coffee_check() { touch unexpected-probe; }
cmd_check
''')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / 'checked-port').exists())
        self.assertFalse((self.root / 'unexpected-probe').exists())

    def selection_script(self, local=False, mode='ok'):
        # Fake control plane can acknowledge a PUT without applying it, or fail authentication.
        setup = '''
current=US-OLD
jstr() { printf '"%s"' "$1"; }
py() { printf 'CCA-1\\tUS-A\\n'; }
ctl() {
    if [ "$1" = GET ]; then
        CODE=200
        case "$MODE" in auth) CODE=401 ;; missing) CODE=404 ;; esac
    else
        CODE=204
        case "$3" in
            *REJECT*) current=REJECT; touch rejected ;;
            *) case "$MODE" in ok) current=US-A ;; fail) CODE=500 ;; esac ;;
        esac
    fi
    printf '{"now":"%s"}' "$current" > "$CTLOUT"
}
lctl() { ctl "$@"; LCODE="$CODE"; LBODY=$(cat "$CTLOUT"); }
'''
        (self.root / 'selected').write_text('CCA-1\tUS-A\n')
        return ('MODE=%s\nSEL=selected\nCTLOUT=control.json\nTAB=$\'\\t\'\n' % mode + setup +
                ('local_select_all' if local else 'select_all'))

    def test_router_and_local_selection_confirm_actual_node(self):
        for local in (False, True):
            for mode in ('ok', 'noop', 'fail', 'auth', 'missing'):
                with self.subTest(local=local, mode=mode):
                    (self.root / 'rejected').unlink(missing_ok=True)
                    names = ['local_select_all', 'json_get'] if local else ['select_all', 'now_of', 'json_str']
                    result = self.run_shell(names, self.selection_script(local, mode))
                    expected = 0 if mode == 'ok' else 2 if mode == 'missing' else 1
                    self.assertEqual(result.returncode, expected, result.stderr + result.stdout)
                    if mode in ('noop', 'fail'):
                        self.assertTrue((self.root / 'rejected').exists())

    def test_missing_runtime_and_missing_source_node_fail_without_restart(self):
        setup = self.selection_script(mode='fail').rsplit('select_all', 1)[0].replace('US-A', 'US-D')
        (self.root / 'selected').write_text('CCA-1\tUS-D\n')
        result = self.run_shell(['select_all', 'now_of', 'json_str', 'apply_now'], setup + '''
BK=oc R="$HOME" D="$HOME" NAME=OpenClash
fill_source() { :; }
runtime_config() { echo "$HOME/missing-runtime.yaml"; }
running() { return 0; }
missing_ports() { :; }
in_source() { printf '%s' "$1" > missing-source; return 1; }
restart_now() { touch unexpected-restart; return 0; }
apply_now
''')
        self.assertEqual(result.returncode, 1, result.stderr + result.stdout)
        self.assertIn('切换到 US-D 失败', result.stdout)
        self.assertIn('US-D', (self.root / 'missing-source').read_text())
        self.assertFalse((self.root / 'unexpected-restart').exists())

    def test_local_bad_selection_does_not_reload_and_claim_ready(self):
        result = self.run_shell(['local_ensure'], '''
load_local() { LOCAL_CLIENT=manual LOCAL_APPLIED=same LOCAL_WANT=same; }
local_up() { return 0; }
local_ports_ok() { return 0; }
local_select_all() { return 1; }
local_apply() { touch unexpected-reload; return 0; }
local_ensure
''')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / 'unexpected-reload').exists())

    def test_missing_local_group_can_be_rebuilt_and_checked(self):
        result = self.run_shell(['local_ensure'], '''
load_local() { LOCAL_CLIENT=manual LOCAL_APPLIED=same LOCAL_WANT=same; }
local_up() { return 0; }
local_ports_ok() { return 0; }
local_select_all() { [ -f rebuilt ] && return 0; return 2; }
local_apply() { touch rebuilt; }
local_ensure
''')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.root / 'rebuilt').exists())

    def test_builtin_selection_failure_cannot_be_repaired_by_unrequested_reload(self):
        result = self.run_shell(['core_ensure'], '''
core_possible() { return 0; }
core_fetch() { return 0; }
py() { return 0; }
core_sock() { echo unused; }
core_up() { return 0; }
local_ports_ok() { return 0; }
local_select_all() { return 1; }
core_reload() { touch unexpected-reload; return 0; }
LOCAL_APPLIED=same LOCAL_WANT=same
core_ensure
''')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / 'unexpected-reload').exists())

    def test_failed_local_ensure_cannot_pass_with_a_correct_current_slot(self):
        result = self.run_shell(['route_local', 'json_get'], '''
load_local() { :; }
local_ready() { return 0; }
local_ensure() { return 1; }
lctl() { LCODE=200; LBODY='{"now":"US-A"}'; }
py() { echo US-A; }
gui_fail() { :; }
ACC_NODE=US-A ACC_SLOT=1
route_local
''')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / 'checked-port').exists())

    def test_router_reload_does_not_hide_failed_node_selection(self):
        (self.root / 'runtime.yaml').write_text('fake runtime\n')
        (self.root / 'tmp').mkdir()
        result = self.run_shell(['apply_now'], '''
BK=oc R="$HOME" D="$HOME" ACC=unused CCA_ALLOW_RELOAD=1 NAME=OpenClash
fill_source() { :; }
runtime_config() { echo "$HOME/runtime.yaml"; }
running() { return 0; }
fill_waiting() { return 1; }
ruby() { return 0; }
structure() { echo "$1"; }
core_test() { return 0; }
cca_ports() { echo 7901; }
put_runtime() { cp -f "$2" "$1"; }
reload() { return 0; }
reload_wait() { return 0; }
select_all() { return 1; }
apply_now
''')
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('已重载并建立账号端口池', result.stdout)

    def fake_curl(self):
        # Record argv before returning canned data: no real curl or network calls.
        return '''
curl() {
    printf '%s\\n' "$@" >> curl-args
    printf '%s\\n' END >> curl-args
    case "$*" in
        *cdn-cgi/trace*) printf 'ip=203.0.113.10\\nloc=US\\n' ;;
        *iprisk*) echo '{}' ;;
        *http_code*) printf '{}\\n200' ;;
    esac
}
'''

    def curl_calls(self):
        raw = (self.root / 'curl-args').read_text().splitlines()
        calls, one = [], []
        for arg in raw:
            if arg == 'END':
                calls.append(one)
                one = []
            else:
                one.append(arg)
        return calls

    def assert_curl_route(self, args, bypass):
        self.assertEqual(args[0], '-q', 'curl must ignore .curlrc before parsing other options')
        self.assertEqual(args[args.index('--noproxy') + 1], bypass)

    def test_exit_probes_ignore_host_bypass_and_curl_config(self):
        (self.root / '.curlrc').write_text('proxy = "http://example.invalid:9997"\nnoproxy = "*"\n')
        result = self.run_shell(['coffee_check'], self.fake_curl() + '''
leak_start() { :; }
leak_finish() { :; }
py() { echo verified; }
TRACE_HOST=example.com CCA_MIN_TRUST=0 ACC_NODE=US-A
coffee_check
''')
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.curl_calls()
        self.assertEqual(len(calls), 2)
        for args in calls:
            self.assert_curl_route(args, '')
            self.assertEqual(args[args.index('-x') + 1], 'http://192.0.2.10:7901')

    def test_api_probe_has_explicit_proxy_or_direct_route(self):
        for proxy in ('http://192.0.2.10:7901', ''):
            with self.subTest(proxy=proxy):
                (self.root / 'curl-args').unlink(missing_ok=True)
                result = self.run_shell(['api_check'], self.fake_curl() + '''
health_note() { :; }
ACC_BASE_URL=https://api.example.com
api_check
''', {'ACC_PROXY': proxy})
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assert_curl_route(self.curl_calls()[0], '' if proxy else '*')

    def test_control_plane_never_inherits_host_proxy(self):
        result = self.run_shell(['lctl'], self.fake_curl() + '''
LOCAL_CTL=http://192.0.2.10:9090
lctl GET /version
test "$LCODE" = 200
''')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_curl_route(self.curl_calls()[0], '*')

    def test_python_leak_probe_has_explicit_route_and_no_curl_config(self):
        source = SOURCE.split("PY_SRC=$(cat <<'PY'\n", 1)[1].split('\nPY\n)', 1)[0]
        node = next(n for n in ast.parse(source).body if isinstance(n, ast.FunctionDef) and n.name == 'leak_curl')
        ns = {}
        exec(compile(ast.Module(body=[node], type_ignores=[]), str(CCA), 'exec'), ns)
        for proxy in ('http://192.0.2.10:7901', ''):
            with self.subTest(proxy=proxy), mock.patch('subprocess.run') as run:
                run.return_value = subprocess.CompletedProcess([], 0, b'{}')
                self.assertEqual(ns['leak_curl']('https://example.com', proxy), '{}')
                args = run.call_args.args[0]
                self.assertEqual(args[0], 'curl')
                self.assert_curl_route(args[1:], '' if proxy else '*')


if __name__ == '__main__':
    unittest.main()
