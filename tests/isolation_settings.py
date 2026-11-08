#!/usr/bin/env python3
"""Account isolation guards: real TOML grammar, config sources and CLI overrides.

All fixtures live under a temporary HOME; neither clients nor the network are used.
Run once with stdlib tomllib and once with the embedded Python 3.9 fallback.
"""
import contextlib
import base64
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import tempfile
import subprocess
import sys
import unittest
from unittest import mock


SCRIPT = (Path(__file__).resolve().parents[1] / 'cca').read_text()
SOURCE = SCRIPT.split("PY_SRC=$(cat <<'PY'\n", 1)[1].split('\nPY\n)', 1)[0]
PY_BOOT = SCRIPT.split("PY_BOOT='", 1)[1].split("'\n", 1)[0]
DEFINITIONS = compile(SOURCE.split('\nCMDS = {', 1)[0], '<cca-isolation-tests>', 'exec')


class SettingsGuardTests(unittest.TestCase):
    fallback = False

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-isolation-')
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.project = self.home / 'project'
        self.project.mkdir()
        self.ns = {}
        with mock.patch.dict(os.environ, {'HOME': str(self.home), 'CCA_TOOL': 'codex',
                                         'CCA_HOME': str(self.home / '.cca')}, clear=False):
            exec(DEFINITIONS, self.ns)
        if self.fallback:
            self.ns['toml_loads'] = self.ns['tomli_fallback']().loads
        self.getcwd = mock.patch('os.getcwd', return_value=str(self.project))
        self.getcwd.start()
        self.addCleanup(self.getcwd.stop)

    def write(self, relative, text):
        path = self.home / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        return path

    def shared(self, tool='codex'):
        self.ns['T'] = self.ns[tool.upper()]
        return self.ns[tool + '_settings_problems']()

    def launch(self, *args, tool='codex'):
        self.ns['T'] = self.ns[tool.upper()]
        return self.ns['launch_settings_problems'](args)

    def test_multiline_escaped_and_dotted_sensitive_keys(self):
        for text in (
            'chatgpt_base_url = """\nhttps://example.invalid"""\n',
            "chatgpt_base_url = '''\nhttps://example.invalid'''\n",
            '"chatgpt_base_url" = "https://example.invalid"\n',
            '"\\u0063hatgpt_base_url" = "https://example.invalid"\n',
            '[profiles.other]\nchatgpt_base_url = "https://example.invalid"\n',
            'profiles.other.chatgpt_base_url = "https://example.invalid"\n',
            'profiles = { other = { chatgpt_base_url = "https://example.invalid" } }\n',
        ):
            with self.subTest(text=text):
                self.write('.codex/config.toml', text)
                errors = self.shared()
                self.assertTrue(errors)
                self.assertIn('chatgpt_base_url', ' '.join(errors))
                self.assertNotIn('example.invalid', ' '.join(errors))

    def test_inline_nested_and_quoted_feature_tables(self):
        for text in (
            'features = { respect_system_proxy = true }',
            'features.respect_system_proxy = true',
            '["features"]\n"respect_system_proxy" = true',
            'profiles = { "deep.review" = { features = { respect_system_proxy = true } } }',
        ):
            with self.subTest(text=text):
                self.write('.codex/config.toml', text)
                self.assertTrue(self.shared())
        self.write('.codex/config.toml', '"features.respect_system_proxy" = true')
        self.assertEqual(self.shared(), [])

    def test_all_external_and_legacy_profiles_are_checked(self):
        self.write('.codex/config.toml', 'profile = "safe"\n[profiles.safe]\nmodel="m"\n'
                   '[profiles."unused.review"]\ncli_auth_credentials_store="keyring"')
        self.assertIn('profiles.unused.review', ' '.join(self.shared()))
        self.write('.codex/config.toml', 'model="m"')
        self.write('.codex/unused.config.toml', 'forced_chatgpt_workspace_id="test-workspace"')
        errors = self.shared()
        self.assertIn('unused.config.toml:forced_chatgpt_workspace_id', errors)
        self.assertNotIn('test-workspace', ' '.join(errors))

    def test_profile_names_match_discovery_and_cannot_escape_checked_files(self):
        for name in ('deep-review', '工作 配置', 'review.v2', 'with spaces'):
            with self.subTest(name=name):
                self.write('.codex/' + name + '.config.toml', 'model="sample-model"\n')
                self.write('.codex/config.toml', 'profile=' + json.dumps(name) + '\n')
                self.assertTrue(self.ns['codex_profile_file'](name + '.config.toml'))
                self.assertEqual(self.shared(), [])
                self.assertEqual(self.launch('--profile', name), [])
                self.assertEqual(self.launch('-c', 'profile=' + json.dumps(name)), [])
        for name in ('../outside', '/tmp/outside', 'nested/profile', '.hidden', '', 'bad\nname', 123):
            with self.subTest(name=name):
                self.write('.codex/config.toml', 'profile=' + json.dumps(name) + '\n')
                self.assertIn('config.toml:profile:invalid_profile', self.shared())
                self.write('.codex/config.toml', 'model="sample-model"\n')
                self.assertTrue(self.launch('-c', 'profile=' + json.dumps(name)))
                if isinstance(name, str):
                    self.assertTrue(self.launch('--profile', name))
        self.write('project/.codex/config.toml', 'profile="../outside"\n')
        self.assertTrue(self.launch())

    def test_invalid_duplicate_or_unreadable_config_fails_closed(self):
        for text in ('model="unterminated', 'model="a"\nmodel="b"',
                     'features = "not a table"', 'profiles = []'):
            with self.subTest(text=text):
                self.write('.codex/config.toml', text)
                self.assertTrue(self.shared())
        path = self.home / '.codex/config.toml'
        path.unlink()
        path.mkdir()
        self.assertTrue(self.shared())
        path.rmdir()
        path.symlink_to(self.home / 'missing-config')
        self.assertTrue(self.shared())
        path.unlink()
        path.write_bytes(b'\xff')
        self.assertTrue(self.shared())

    def test_missing_configs_and_complex_benign_toml_are_allowed(self):
        self.assertEqual(self.shared(), [])
        self.write('.codex/config.toml', '''model = "sample-model"
model_reasoning_effort = "ultra"
service_tier = "priority"
text = """A note containing chatgpt_base_url = "example"
features = { respect_system_proxy = true }
"""
when = 2026-11-08T12:00:00Z
choices = [1, 2, 3]
[features]
respect_system_proxy = false
system_proxy_fallback = false
[projects."/tmp/test-project"]
trust_level = "trusted"
[[sample]]
name = "one"
''')
        self.assertEqual(self.shared(), [])

    def test_builtin_provider_cannot_be_redefined(self):
        self.write('.codex/config.toml', '[model_providers.openai]\nbase_url="https://example.invalid"')
        self.assertIn('config.toml:model_providers.openai', self.shared())

    def test_profiles_cannot_reenable_desktop_fallback(self):
        self.write('.codex/config.toml', 'profile="x"\n[features]\nsystem_proxy_fallback=false\n'
                   '[profiles.x.features]\nsystem_proxy_fallback=true\n')
        self.assertTrue(self.shared())
        with self.assertRaises(SystemExit) as caught:
            self.ns['cmd_codex_fallback_off']()
        self.assertEqual(caught.exception.code, 1)
        self.write('.codex/config.toml', '[features]\nsystem_proxy_fallback=false\n')
        self.write('.codex/deep.config.toml', 'features={system_proxy_fallback=true}\n')
        self.assertTrue(self.shared())
        (self.home / '.codex/deep.config.toml').unlink()
        self.write('.codex/config.toml', '[features]\nsystem_proxy_fallback=true\n')
        self.assertEqual(self.shared(), [])
        with self.assertRaises(SystemExit) as caught:
            self.ns['cmd_codex_fallback_off']()
        self.assertEqual(caught.exception.code, 1)

    def test_dotenv_credentials_and_proxy_variables_are_blocked(self):
        for name in ('CODEX_ACCESS_TOKEN', 'CODEX_API_KEY', 'CODEX_HOME',
                     'CODEX_CHATGPT_BASE_URL_OVERRIDE', 'OPENAI_API_KEY', 'https_proxy'):
            with self.subTest(name=name):
                self.write('.codex/.env', 'export ' + name + '=dummy-do-not-echo\n')
                errors = self.shared()
                self.assertEqual(errors, ['.env:' + name])
                self.assertNotIn('dummy-do-not-echo', ' '.join(errors))
        self.write('.codex/.env', 'CODEX_CA_CERTIFICATE=/tmp/ca.pem\nEDITOR=vi\n')
        self.assertEqual(self.shared(), [])
        (self.home / '.codex/.env').unlink()
        (self.home / '.codex/.env').mkdir()
        self.assertTrue(self.shared())

    def test_claude_shared_json_and_config_types(self):
        for data in ({'env': {'ANTHROPIC_BASE_URL': 'https://example.invalid'}},
                     {'env': {'NO_PROXY': '*'}}, {'apiKeyHelper': 'test-helper'},
                     [], {'env': []}):
            with self.subTest(data=data):
                self.write('.claude/settings.json', json.dumps(data))
                self.assertTrue(self.shared('claude'))
        self.write('.claude/settings.json', '{bad-json')
        self.assertTrue(self.shared('claude'))
        self.write('.claude/settings.json', '{"model":"test-model","env":{"EDITOR":"vi"}}')
        self.assertEqual(self.shared('claude'), [])

    def test_claude_explicit_settings_json_and_file(self):
        self.assertTrue(self.launch('--settings', '{"env":{"ANTHROPIC_AUTH_TOKEN":"dummy"}}', tool='claude'))
        self.write('project/custom.json', '{"env":{"HTTPS_PROXY":"http://example.invalid"}}')
        self.assertTrue(self.launch('--settings=custom.json', tool='claude'))
        self.assertTrue(self.launch('--settings', 'missing.json', tool='claude'))
        self.assertEqual(self.launch('--settings', '{"model":"sample-model"}', tool='claude'), [])
        self.write('project/custom.json', '{"permissions":{"allow":["Read"]}}')
        self.assertEqual(self.launch('--settings=custom.json', tool='claude'), [])

    def test_claude_configuration_cannot_replace_credential_sources(self):
        for name in ('CLAUDE_CONFIG_DIR', 'CLAUDE_SECURESTORAGE_CONFIG_DIR',
                     'CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR', 'CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR',
                     'CLAUDE_CODE_GATEWAY_TOKEN_FILE_DESCRIPTOR', 'CLAUDE_CODE_WEBSOCKET_AUTH_FILE_DESCRIPTOR'):
            with self.subTest(name=name):
                content = json.dumps({'env': {name: 'private-fixture-value'}})
                shared = self.write('.claude/settings.json', content)
                self.assertEqual(self.shared('claude'), ['settings.json:' + name])
                shared.unlink()
                project = self.write('project/.claude/settings.local.json', content)
                self.assertEqual(self.launch(tool='claude'), [str(project.resolve()) + ':' + name])
                project.unlink()
                self.assertEqual(self.launch('--settings', content, tool='claude'), ['--settings:' + name])
                self.assertTrue(self.ns['ENV_RESERVED'].match(name))
                self.assertNotIn(name, self.ns['api_env']({'base_url': 'https://example.invalid',
                                                        'env': {name: 'private-fixture-value'}}))

    def test_required_option_values_do_not_end_guard_scanning(self):
        self.assertEqual(self.launch('--system-prompt', '--settings', tool='claude'), [])
        self.assertTrue(self.launch('--system-prompt', '--', '--settings',
                                   '{"env":{"ANTHROPIC_BASE_URL":"https://example.invalid"}}', tool='claude'))
        self.assertTrue(self.launch('--model', '--', '--config',
                                   'chatgpt_base_url=https://example.invalid'))

    def test_desktop_handoffs_use_the_managed_entrypoint(self):
        self.assertIn('--desktop:use_cca_app', self.launch('--desktop', tool='claude'))
        self.assertIn('app:use_cca_codex_app', self.launch('app'))
        self.assertIn('app:use_cca_codex_app', self.launch('-m', 'sample-model', 'app'))
        self.assertEqual(self.launch('exec', 'app'), [])
        self.assertEqual(self.launch('-m', 'app', 'write a test'), [])
        self.assertEqual(self.launch('--', 'app'), [])
        self.assertEqual(self.launch('--system-prompt', '--desktop', tool='claude'), [])

    def test_cli_endpoint_proxy_and_remote_overrides_blocked(self):
        for args in (
            ('-c', 'chatgpt_base_url=https://example.invalid'),
            ('--config=chatgpt_base_url="https://example.invalid"',),
            ('-cmodel_provider=custom',),
            ('-c', 'features={respect_system_proxy=true}'),
            ('-c', 'features.system_proxy_fallback=true'),
            ('--enable', 'respect_system_proxy'),
            ('--enable=system_proxy_fallback',),
            ('--remote', 'ws://example.invalid'),
            ('--remote=ws://example.invalid',),
            ('--oss',), ('--profile', '../escape'), ('--profile=/tmp/profile',),
            ('-c',),
        ):
            with self.subTest(args=args):
                self.assertTrue(self.launch(*args))
        self.assertEqual(self.launch('-c', 'model=sample-model', '-cmodel_reasoning_effort="ultra"',
                                    '--disable', 'respect_system_proxy', '--profile', 'deep-review'), [])
        self.assertEqual(self.launch('--', '--remote', 'literal-prompt'), [])

    def test_project_configs_in_selected_directory_and_ancestors_only(self):
        self.write('other/.codex/config.toml', 'chatgpt_base_url="https://example.invalid"')
        self.assertEqual(self.launch(), [])
        self.write('project/.codex/config.toml', 'features={respect_system_proxy=true}')
        nested = self.project / 'nested'
        nested.mkdir()
        with mock.patch('os.getcwd', return_value=str(nested)):
            self.assertTrue(self.launch())
        self.assertTrue(self.launch('-C', str(self.home / 'other')))
        (self.project / '.codex/config.toml').unlink()
        self.assertEqual(self.launch(), [])
        self.write('project/.claude/settings.local.json', '{"env":{"ANTHROPIC_BASE_URL":"https://example.invalid"}}')
        self.assertTrue(self.launch(tool='claude'))
        self.assertEqual(self.launch(), [])

    def test_scoped_cd_override_is_relative_to_original_directory(self):
        self.write('project/a/b/.codex/config.toml', 'model="safe-model"\n')
        self.write('project/b/.codex/config.toml', 'chatgpt_base_url="https://example.invalid"\n')
        errors = self.launch('-C', 'a', 'exec', '-C', 'b')
        self.assertTrue(errors)
        self.assertIn(str(self.project / 'b/.codex/config.toml'), ' '.join(errors))
        self.write('project/a/b/.codex/config.toml', 'chatgpt_base_url="https://example.invalid"\n')
        self.write('project/b/.codex/config.toml', 'model="safe-model"\n')
        self.assertEqual(self.launch('-C', 'a', 'exec', '--cd=b'), [])

    def test_fallback_off_is_semantic_and_invalid_is_not_off(self):
        for text, expected in (
            ('features={system_proxy_fallback=false}', 0),
            ('[features]\nsystem_proxy_fallback=false', 0),
            ('features={system_proxy_fallback=true}', 1),
            ('features={system_proxy_fallback="false"}', 1),
            ('features={', 1),
        ):
            with self.subTest(text=text):
                self.write('.codex/config.toml', text)
                with self.assertRaises(SystemExit) as caught:
                    self.ns['cmd_codex_fallback_off']()
                self.assertEqual(caught.exception.code, expected)

    def test_command_failure_does_not_echo_values(self):
        self.write('.codex/config.toml', 'chatgpt_base_url="unique-private-value"')
        output = io.StringIO()
        with contextlib.redirect_stdout(output), self.assertRaises(SystemExit) as caught:
            self.ns['cmd_launch_check']('-c', 'model=benign')
        self.assertEqual(caught.exception.code, 1)
        self.assertIn('chatgpt_base_url', output.getvalue())
        self.assertNotIn('unique-private-value', output.getvalue())

    def test_rollout_repair_uses_real_toml_and_reports_invalid_config(self):
        import sqlite3
        databases = self.home / 'databases'
        databases.mkdir()
        target = self.write('.codex/sessions/rollout.jsonl', '{}\n')
        original = str(self.home / '.cca/codex/accounts/example/codex/sessions/rollout.jsonl')
        database = databases / 'state_5.sqlite'
        with contextlib.closing(sqlite3.connect(database)) as connection:
            connection.execute('CREATE TABLE threads (id TEXT, rollout_path TEXT)')
            connection.execute('INSERT INTO threads VALUES (?, ?)', ('test-thread', original))
            connection.commit()
        self.write('.codex/config.toml', 'sqlite_home = """\n' + str(databases) + '"""\n')
        self.ns['codex_fix_rollouts']()
        with contextlib.closing(sqlite3.connect(database)) as connection:
            self.assertEqual(connection.execute('SELECT rollout_path FROM threads').fetchone()[0], str(target))
        self.write('.codex/config.toml', 'sqlite_home = [')
        errors = io.StringIO()
        with contextlib.redirect_stderr(errors):
            self.ns['codex_fix_rollouts']()
        self.assertIn('跳过会话路径修复', errors.getvalue())


class EmbeddedParserTests(SettingsGuardTests):
    fallback = True

    def test_embedded_source_integrity_and_version(self):
        import base64
        import zlib
        data = zlib.decompress(base64.b64decode(self.ns['TOMLI_EMBEDDED']))
        recorded = SOURCE.split('# Source JSON SHA256: ', 1)[1].splitlines()[0]
        self.assertEqual(hashlib.sha256(data).hexdigest(), recorded)
        self.assertEqual(self.ns['tomli_fallback']().__version__, '2.3.0')


class BootstrapTests(unittest.TestCase):
    def run_boot(self, source, directory, *args, env=None, stdin=''):
        packed = base64.b64encode(gzip.compress(source.encode(), mtime=0)).decode()
        return subprocess.run([sys.executable, '-I', '-X', 'utf8', '-c', PY_BOOT, str(directory), packed, *args],
                              input=stdin, capture_output=True, text=True, env=env, timeout=15)

    def test_snapshot_stdin_arguments_and_cache_digest(self):
        with tempfile.TemporaryDirectory(prefix='cca-bootstrap-') as d:
            cache = Path(d) / 'cache'
            source = 'import json,sys; print(json.dumps([sys.argv[1:],sys.stdin.read()]))'
            first = self.run_boot(source, cache, 'one', '--two', stdin='stdin stays available')
            self.assertEqual(first.returncode, 0, first.stderr)
            self.assertEqual(json.loads(first.stdout), [['one', '--two'], 'stdin stays available'])
            before = sorted(p.name for p in cache.iterdir())
            self.assertEqual(len(before), 1)
            again = self.run_boot(source, cache, 'again')
            self.assertEqual(again.returncode, 0, again.stderr)
            self.assertEqual(sorted(p.name for p in cache.iterdir()), before)
            changed = self.run_boot('print("new snapshot")', cache)
            self.assertEqual(changed.stdout.strip(), 'new snapshot')
            self.assertEqual(len(list(cache.iterdir())), 2)

    def test_full_source_with_large_inherited_environment(self):
        with tempfile.TemporaryDirectory(prefix='cca-bootstrap-') as d:
            env = {'HOME': d, 'PATH': '/usr/bin:/bin', 'CCA_TOOL': 'codex', 'CCA_POLICY_DEFAULT': '',
                   'CCA_TEST_LARGE_ENV': 'x' * 90000}
            result = self.run_boot(SOURCE, Path(d) / 'cache', 'settings-check', env=env)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, '')


if __name__ == '__main__':
    unittest.main()
