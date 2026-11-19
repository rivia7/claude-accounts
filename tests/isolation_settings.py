#!/usr/bin/env python3
"""配置隔离：共享的设置、项目设置、启动参数里会把账号换到别处的地方都拦住（完整的 TOML 语法、各个来源、命令行覆盖）。

临时 HOME、cca 的辅助程序（cca-core settings-check / launch-check / codex-fallback-off；账号表里的 env 经 validate 和 env，
会话库的路径修复经 Codex 的 link），不起客户端、不联网。

原来每个场景跑两遍：一遍用标准库的 tomllib，一遍用给旧版 Python 内嵌的 tomli。辅助程序只有一个 TOML 解析器
（cli/src/py/toml.rs，照 tomllib 一个函数一个函数译过来的），所以只跑一遍；这里交给它的每一份 TOML 另外都收在它的对照数据里
（cli/tools/gen_vectors.py 的 TOML_SETTINGS_FILES / TOML_SETTINGS_OVERRIDES → cli/tests/vectors/toml.json，cargo test 的
matches_cpython_vectors 拿 CPython 的解析结果和出错的说法逐份核对）。内嵌 tomli 的摘要和版本、内嵌 Python 的字节码缓存
（PY_BOOT）随内嵌的 Python 一起没有了；辅助程序自己怎么解出来、怎么缓存在 tests/regress.sh 里测。

原来直接调内部函数的三处，这里经子命令测，辅助程序自己的单元测试（cargo test）里另有照原来的结果写的：profile 的名字和
文件名怎么认（cli/src/core/tool.rs 的 profile_names_match_discovery）、哪些变量名是保留的（cli/src/core/base.rs 的
credential_sources_are_reserved）、第三方 API 账号交给 Claude Code 的环境（cli/src/cmd/accounts.rs 的
api_env_never_carries_reserved_names）。"""
import contextlib
import json
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from helper_bin import HELPER

# 会动到这台机器、会联网的命令：沙箱的 PATH 最前面放只会报错的替身。这里用到的子命令只读写沙箱里的文件
# （修会话库的那一个另外起系统的 sqlite3），一个都不该调到
GUARDED = ('security', 'osascript', 'open', 'pbcopy', 'launchctl', 'ssh', 'scp', 'curl', 'git', 'nc', 'crontab', 'defaults', 'killall', 'pkill',
           'sudo', 'lsappinfo', 'networksetup', 'scutil', 'codesign', 'spctl', 'hdiutil', 'ditto', 'claude', 'codex', 'agy', 'mihomo', 'ps')
GUARD = '#!/bin/sh\nprintf \'%s\\n\' "$0 $*" >> "$HOME/unexpected-commands.log"\nexit 127\n'


class SettingsGuardTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-isolation-')
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.project = self.home / 'project'
        self.project.mkdir()
        (self.home / 'bin').mkdir()
        for name in GUARDED:
            (self.home / 'bin' / name).write_text(GUARD)
            (self.home / 'bin' / name).chmod(0o755)
        stray = self.home / 'unexpected-commands.log'
        self.addCleanup(lambda: self.assertEqual(stray.read_text() if stray.exists() else '', '', '调到了没有准备假命令的外部命令'))
        self.env = {'HOME': str(self.home), 'CCA_HOME': str(self.home / '.cca'), 'PATH': '%s:/usr/bin:/bin' % (self.home / 'bin')}

    def write(self, relative, text):
        path = self.home / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding='utf-8')
        return path

    def helper(self, *argv, tool='codex', cwd=None):
        """在项目目录（或 cwd）里跑一个子命令"""
        return subprocess.run([HELPER, *argv], env=dict(self.env, CCA_TOOL=tool), cwd=str(cwd or self.project),
                              stdin=subprocess.DEVNULL, capture_output=True, encoding='utf-8', timeout=30)

    def problems(self, *argv, tool='codex', cwd=None):
        """settings-check / launch-check 查出来的问题。没有：什么都不输出，退出码 0；有：退出码 1，stdout 一行，
        空格隔开的「来源:配置键」（出了别的错——stderr 上有话、退出码对不上——不算查出了问题）"""
        r = self.helper(*argv, tool=tool, cwd=cwd)
        self.assertEqual(r.stderr, '')
        self.assertEqual(r.returncode, 1 if r.stdout else 0, r.stdout)
        if not r.stdout:
            return []
        self.assertEqual(r.stdout.count('\n'), 1, r.stdout)
        return r.stdout.rstrip('\n').split(' ')

    def shared(self, tool='codex'):
        return self.problems('settings-check', tool=tool)

    def launch(self, *args, tool='codex', cwd=None):
        return self.problems('launch-check', *args, tool=tool, cwd=cwd)

    def fallback_off(self):
        """codex-fallback-off 的退出码（0：共享的配置确实关掉了桌面 App 的系统代理回退）"""
        r = self.helper('codex-fallback-off')
        self.assertEqual((r.stdout, r.stderr), ('', ''))
        return r.returncode

    def api_account(self, env):
        """Claude 的账号表里放一个第三方 API 账号，env 是它要带给 Claude Code 的变量"""
        self.write('.cca/accounts.json', json.dumps({'api': {'base_url': 'https://example.invalid', 'env': env}}))

    def api_launch(self):
        """bash 启动这个第三方 API 账号时拿到的（env 子命令的输出）：(交给 Claude Code 的环境, 整份输出)。
        环境在启动参数 ACC_ARGS 里 --settings 的 env"""
        r = self.helper('env', 'api', tool='claude')
        self.assertEqual((r.returncode, r.stderr), (0, ''))
        args = [line for line in r.stdout.splitlines() if line.startswith('ACC_ARGS=(')]
        args = shlex.split(args[0][len('ACC_ARGS=('):-1])
        return json.loads(args[args.index('--settings') + 1])['env'], r.stdout

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
                self.write('.codex/config.toml', 'profile=' + json.dumps(name) + '\n')
                # 叫这个名字的 profile 文件找得到、照样查：里面有不该有的键就报出来（名字里有空格，这里看整行）
                self.write('.codex/' + name + '.config.toml', 'chatgpt_base_url="https://example.invalid"\n')
                self.assertIn(name + '.config.toml:chatgpt_base_url', ' '.join(self.shared()))
                self.write('.codex/' + name + '.config.toml', 'model="sample-model"\n')
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
        self.assertEqual(self.fallback_off(), 1)
        self.write('.codex/config.toml', '[features]\nsystem_proxy_fallback=false\n')
        self.write('.codex/deep.config.toml', 'features={system_proxy_fallback=true}\n')
        self.assertTrue(self.shared())
        (self.home / '.codex/deep.config.toml').unlink()
        self.write('.codex/config.toml', '[features]\nsystem_proxy_fallback=true\n')
        self.assertEqual(self.shared(), [])
        self.assertEqual(self.fallback_off(), 1)

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
                # 第三方 API 账号的 env 里也不能设：校验账号表时拒绝（ENV_RESERVED）；就算表里写了，交给 Claude Code 的
                # 环境里也没有它（api_env），别的变量照带
                self.api_account({name: 'private-fixture-value', 'EDITOR': 'vi'})
                r = self.helper('validate', tool='claude')
                self.assertEqual((r.returncode, r.stdout), (1, ''))
                self.assertIn('env 里不能设 %s（' % name, r.stderr)
                self.assertNotIn('private-fixture-value', r.stderr)
                env, said = self.api_launch()
                self.assertNotIn(name, env)
                self.assertNotIn('private-fixture-value', said)
                self.assertEqual((env['ANTHROPIC_BASE_URL'], env['EDITOR']), ('https://example.invalid', 'vi'))

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
        self.assertTrue(self.launch(cwd=nested))
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
                self.assertEqual(self.fallback_off(), expected)

    def test_command_failure_does_not_echo_values(self):
        self.write('.codex/config.toml', 'chatgpt_base_url="unique-private-value"')
        r = self.helper('launch-check', '-c', 'model=benign')
        self.assertEqual(r.returncode, 1)
        self.assertIn('chatgpt_base_url', r.stdout)
        self.assertNotIn('unique-private-value', r.stdout + r.stderr)

    @unittest.skipUnless(shutil.which('sqlite3'), '辅助程序改会话库用系统的 sqlite3 命令，没装时这一步跳过')
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
        # 会话库的路径修复是 Codex 账号 link（每次会话前后都跑）的最后一步
        self.write('.cca/codex.json', json.dumps({'erin@example.com': 'US-A'}))
        r = self.helper('link', 'erin@example.com')
        self.assertEqual((r.returncode, r.stdout, r.stderr), (0, '', ''))
        with contextlib.closing(sqlite3.connect(database)) as connection:
            self.assertEqual(connection.execute('SELECT rollout_path FROM threads').fetchone()[0], str(target))
        self.write('.codex/config.toml', 'sqlite_home = [')
        r = self.helper('link', 'erin@example.com')
        self.assertEqual((r.returncode, r.stdout), (0, ''))
        self.assertIn('跳过会话路径修复', r.stderr)

    def test_large_inherited_environment(self):
        # 原来是「整份内嵌的 Python 当参数传、再带着很大的环境」起不起得来（BootstrapTests）；辅助程序没有那么长的参数，
        # 留下的是：继承来的环境很大时照常工作
        with tempfile.TemporaryDirectory(prefix='cca-bootstrap-') as d:
            env = {'HOME': d, 'PATH': '/usr/bin:/bin', 'CCA_TOOL': 'codex', 'CCA_POLICY_DEFAULT': '',
                   'CCA_TEST_LARGE_ENV': 'x' * 90000}
            result = subprocess.run([HELPER, 'settings-check'], env=env, stdin=subprocess.DEVNULL, capture_output=True,
                                    encoding='utf-8', timeout=15)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, '')


# 原来还有的几个，现在在别处：
# - EmbeddedParserTests（上面每个场景换成内嵌的 tomli 再跑一遍）：解析器只有一个了；同样的输入由 cli/src/py/toml.rs 的
#   matches_cpython_vectors 拿 cli/tests/vectors/toml.json 逐份对照
# - EmbeddedParserTests.test_embedded_source_integrity_and_version：内嵌的 tomli 没有了，不用再核对它的摘要和版本
# - BootstrapTests.test_snapshot_stdin_arguments_and_cache_digest：PY_BOOT 的字节码缓存没有了；辅助程序按摘要解出来、
#   不重复解、清旧的在 tests/regress.sh 的「辅助程序：…」那几个用例里
# - BootstrapTests.test_full_source_with_large_inherited_environment：就是上面的 test_large_inherited_environment


if __name__ == '__main__':
    unittest.main()
