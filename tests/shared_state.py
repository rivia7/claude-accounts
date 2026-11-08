#!/usr/bin/env python3
"""共享工作资料与账号私有状态：只运行内嵌 Python，所有文件均在临时 HOME。"""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
from device_identity import PY_BOOT, SOURCE_PACKED, ACCOUNTS


class SharedState(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-shared-')
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.cca = self.home / '.cca'
        self.write_json(self.cca / 'accounts.json', dict.fromkeys(ACCOUNTS, 'US-A'))
        self.write_json(self.cca / 'codex.json', dict.fromkeys(ACCOUNTS, 'US-A'))
        self.write_json(self.home / '.claude.json', {
            'theme': 'dark', 'userID': 'a' * 64, 'machineID': 'b' * 64,
            'oauthAccount': {'emailAddress': 'default@example.com'},
        })

    def write(self, path, text):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding='utf-8')

    def write_json(self, path, value):
        self.write(path, json.dumps(value, ensure_ascii=False))

    def atomic_write(self, path, text):
        temporary = path.with_name(path.name + '.test-new')
        self.write(temporary, text)
        os.replace(temporary, path)

    def run_cca(self, tool, command='link', key=ACCOUNTS[0], expected=0):
        args = [sys.executable, '-I', '-X', 'utf8', '-c', PY_BOOT, '', SOURCE_PACKED, command]
        if key is not None:
            args.append(key)
        result = subprocess.run(
            args,
            env={'HOME': str(self.home), 'PATH': '/usr/bin:/bin',
                 'CCA_TOOL': tool, 'CCA_POLICY_DEFAULT': ''},
            capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode, expected, result.stderr)
        return result

    def directory(self, tool, key=ACCOUNTS[0]):
        ident = re.sub('[^a-z0-9]+', '-', key).strip('-')[:20].strip('-')
        ident += '-' + hashlib.sha1(key.encode()).hexdigest()[:6]
        return self.cca / ('codex/accounts' if tool == 'codex' else 'accounts') / ident

    def client(self, tool, key=ACCOUNTS[0]):
        return self.directory(tool, key) / tool

    def master(self, tool):
        return self.home / ('.' + tool)

    def identities(self, tool):
        return [(self.directory(tool, key) / 'device.json').read_bytes() for key in ACCOUNTS]

    def link_both(self, tool):
        for key in ACCOUNTS:
            self.run_cca(tool, key=key)

    def assert_saved_conflict(self, tool, key, original, stderr):
        root = self.cca / ('codex' if tool == 'codex' else '')
        conflicts = root / 'conflicts' / self.directory(tool, key).name
        copies = [p for p in conflicts.rglob('*') if p.is_file() and p.read_text() == original]
        self.assertTrue(copies, '完整账号副本未保存在 %s' % conflicts)
        self.assertTrue(any(str(p) in stderr for p in copies), stderr)

    def test_history_memory_configuration_and_resume_files_remain_shared(self):
        fixtures = {
            'claude': {
                'settings.json': '{"model":"opus"}',
                'settings.local.json': '{"permissions":{"allow":["Read"]}}',
                'CLAUDE.md': '# 通用规则\n',
                'history.jsonl': '{"display":"old prompt"}\n',
                'projects/-workspace/session.jsonl': '{"sessionId":"shared-session"}\n',
                'projects/-workspace/memory/MEMORY.md': '# 项目记忆\n',
                'skills/example/SKILL.md': '# 技能\n',
                'rules/example.md': '# 规则\n',
                'hooks/notify.sh': '#!/bin/sh\n# 通用钩子\n',
            },
            'codex': {
                'config.toml': 'model = "example-model"\n',
                'deep-review.config.toml': 'model_reasoning_effort = "high"\n',
                'AGENTS.md': '# 通用规则\n',
                'history.jsonl': '{"text":"old prompt"}\n',
                'session_index.jsonl': '{"id":"shared-thread"}\n',
                'sessions/2026/11/05/rollout-shared.jsonl': '{"type":"session_meta"}\n',
                'archived_sessions/rollout-old.jsonl': '{"type":"session_meta"}\n',
                'memories/memory.md': '# 项目记忆\n',
                'skills/example/SKILL.md': '# 技能\n',
                'rules/example.rules': '# 规则\n',
            },
        }
        for tool, files in fixtures.items():
            with self.subTest(tool=tool):
                for name, value in files.items():
                    self.write(self.master(tool) / name, value)
                self.link_both(tool)
                original_ids = self.identities(tool)
                for name, value in files.items():
                    for key in ACCOUNTS:
                        path = self.client(tool, key) / name
                        self.assertEqual(path.read_text(), value)
                        self.assertTrue(path.samefile(self.master(tool) / name))
                memory = 'memories/memory.md' if tool == 'codex' else 'projects/-workspace/memory/MEMORY.md'
                self.write(self.client(tool) / memory, '# 第一账号新增记忆\n')
                with (self.client(tool) / 'history.jsonl').open('a') as f:
                    f.write('{"text":"new prompt"}\n')
                self.assertEqual((self.client(tool, ACCOUNTS[1]) / memory).read_text(), '# 第一账号新增记忆\n')
                self.assertIn('new prompt', (self.client(tool, ACCOUNTS[1]) / 'history.jsonl').read_text())
                self.link_both(tool)
                self.assertEqual(self.identities(tool), original_ids)

    def test_unknown_runtime_state_is_not_imported_or_shared(self):
        for tool in ('claude', 'codex'):
            with self.subTest(tool=tool):
                self.write(self.master(tool) / 'future-runtime.json', 'master state')
                self.write(self.master(tool) / 'future-runtime' / 'lease', 'master lease')
                self.link_both(tool)
                for key in ACCOUNTS:
                    for name in ('future-runtime.json', 'future-runtime'):
                        self.assertFalse(os.path.lexists(self.client(tool, key) / name))
                self.write(self.client(tool) / 'future-runtime.json', 'alice state')
                self.write(self.client(tool) / 'future-runtime' / 'lease', 'alice lease')
                self.run_cca(tool)
                self.assertEqual((self.master(tool) / 'future-runtime.json').read_text(), 'master state')
                self.assertEqual((self.master(tool) / 'future-runtime' / 'lease').read_text(), 'master lease')
                self.assertEqual((self.client(tool) / 'future-runtime.json').read_text(), 'alice state')
                self.assertFalse((self.client(tool) / 'future-runtime').is_symlink())

    def test_legacy_unknown_shared_links_become_independent_copies(self):
        tool = 'claude'
        master = self.master(tool)
        self.write(master / 'future-runtime.json', 'legacy state')
        self.write(master / 'future-runtime' / 'lease', 'legacy lease')
        self.link_both(tool)
        original_ids = self.identities(tool)
        for key in ACCOUNTS:
            client = self.client(tool, key)
            for name in ('future-runtime.json', 'future-runtime'):
                if (client / name).is_symlink():
                    (client / name).unlink()
                (client / name).symlink_to(os.path.relpath(master / name, client))
            self.run_cca(tool, key=key)
            self.run_cca(tool, key=key)
            self.assertFalse((client / 'future-runtime.json').is_symlink())
            self.assertFalse((client / 'future-runtime').is_symlink())
            self.assertFalse((client / 'future-runtime.json').samefile(master / 'future-runtime.json'))
            self.assertEqual((client / 'future-runtime' / 'lease').read_text(), 'legacy lease')
        self.write(self.client(tool) / 'future-runtime.json', 'alice private state')
        self.write(self.client(tool) / 'future-runtime' / 'lease', 'alice private lease')
        self.assertEqual((master / 'future-runtime.json').read_text(), 'legacy state')
        self.assertEqual((master / 'future-runtime' / 'lease').read_text(), 'legacy lease')
        self.assertEqual((self.client(tool, ACCOUNTS[1]) / 'future-runtime.json').read_text(), 'legacy state')
        self.assertEqual((self.client(tool, ACCOUNTS[1]) / 'future-runtime' / 'lease').read_text(), 'legacy lease')
        self.assertEqual(self.identities(tool), original_ids)

    def test_unknown_user_links_survive_and_broken_legacy_links_are_removed(self):
        self.run_cca('claude')
        client, master = self.client('claude'), self.master('claude')
        outside = self.home / 'my-state.json'
        self.write(outside, 'user-owned state')
        (client / 'my-state.json').symlink_to(outside)
        self.write(master / 'different-name.json', 'another user-owned state')
        (client / 'custom-state.json').symlink_to(master / 'different-name.json')
        (client / 'old-runtime.json').symlink_to(master / 'old-runtime.json')
        self.run_cca('claude')
        self.assertTrue((client / 'my-state.json').is_symlink())
        self.assertTrue((client / 'custom-state.json').is_symlink())
        self.assertEqual(outside.read_text(), 'user-owned state')
        self.assertFalse(os.path.lexists(client / 'old-runtime.json'))
        self.assertFalse(os.path.lexists(master / 'old-runtime.json'))

    def test_legacy_directory_with_nested_symlink_fails_without_changing_sources(self):
        self.run_cca('claude')
        master = self.master('claude') / 'future-runtime'
        outside = self.home / 'external-state'
        self.write(master / 'state.json', 'shared source state')
        self.write(outside / 'marker', 'external target state')
        (master / 'nested').symlink_to(outside, target_is_directory=True)
        linked = self.client('claude') / 'future-runtime'
        linked.symlink_to(master, target_is_directory=True)
        result = self.run_cca('claude', expected=1)
        self.assertIn('内部符号链接', result.stderr)
        self.assertTrue(linked.is_symlink())
        self.assertEqual(os.readlink(linked), str(master))
        self.assertEqual((master / 'state.json').read_text(), 'shared source state')
        self.assertTrue((master / 'nested').is_symlink())
        self.assertEqual(os.readlink(master / 'nested'), str(outside))
        self.assertEqual((outside / 'marker').read_text(), 'external target state')

    def test_atomic_configuration_save_repairs_link_and_updates_other_account(self):
        for tool, name, before, after in (
            ('claude', 'settings.json', '{"model":"old"}', '{"model":"new"}'),
            ('codex', 'config.toml', 'model = "old"\n', 'model = "new"\n'),
            ('codex', 'deep-review.config.toml', 'model_reasoning_effort = "medium"\n',
             'model_reasoning_effort = "high"\n'),
        ):
            with self.subTest(tool=tool):
                master = self.master(tool) / name
                self.write(master, before)
                self.link_both(tool)
                original_ids = self.identities(tool)
                path = self.client(tool) / name
                self.atomic_write(path, after)
                self.assertFalse(path.is_symlink())
                self.run_cca(tool)
                self.assertTrue(path.is_symlink())
                for visible in (master, self.client(tool, ACCOUNTS[1]) / name):
                    if tool == 'claude':
                        self.assertEqual(json.loads(visible.read_text()), json.loads(after))
                    else:
                        self.assertEqual(visible.read_text(), after)
                self.assertEqual(self.identities(tool), original_ids)

    def test_json_concurrent_edits_merge_fields_and_deletions(self):
        initial = {'model': 'base', 'theme': 'dark', 'obsolete': True,
                   'permissions': {'allow': ['Read'], 'deny': ['Write']}}
        self.write_json(self.master('claude') / 'settings.json', initial)
        self.link_both('claude')
        alice = {'model': 'alice', 'theme': 'dark',
                 'permissions': {'allow': ['Read', 'Bash'], 'deny': ['Write']}}
        bob = dict(initial, theme='light', permissions={'allow': ['Read'], 'deny': ['Edit']})
        for key, value in zip(ACCOUNTS, (alice, bob)):
            self.atomic_write(self.client('claude', key) / 'settings.json', json.dumps(value))
        self.run_cca('claude', key=ACCOUNTS[0])
        self.run_cca('claude', key=ACCOUNTS[1])
        expected = {'model': 'alice', 'theme': 'light',
                    'permissions': {'allow': ['Read', 'Bash'], 'deny': ['Edit']}}
        self.assertEqual(json.loads((self.master('claude') / 'settings.json').read_text()), expected)
        for key in ACCOUNTS:
            self.assertTrue((self.client('claude', key) / 'settings.json').is_symlink())
            self.assertEqual(json.loads((self.client('claude', key) / 'settings.json').read_text()), expected)

    def test_concurrent_config_conflicts_preserve_master_and_complete_local_copy(self):
        cases = (
            ('claude', 'settings.json', '{"model":"base"}', '{"model":"alice"}', '{"model":"bob"}'),
            ('codex', 'config.toml', 'model = "base"\neffort = "medium"\n',
             'model = "alice"\neffort = "medium"\n', 'model = "base"\neffort = "high"\n'),
            ('codex', 'deep-review.config.toml', 'model = "base"\n',
             'model = "alice"\n', 'model = "bob"\n'),
        )
        for tool, name, initial, alice, bob in cases:
            with self.subTest(tool=tool):
                master = self.master(tool) / name
                self.write(master, initial)
                self.link_both(tool)
                for key, value in zip(ACCOUNTS, (alice, bob)):
                    self.atomic_write(self.client(tool, key) / name, value)
                self.run_cca(tool, key=ACCOUNTS[0])
                result = self.run_cca(tool, key=ACCOUNTS[1])
                if tool == 'claude':
                    self.assertEqual(json.loads(master.read_text()), json.loads(alice))
                else:
                    self.assertEqual(master.read_text(), alice)
                self.assert_saved_conflict(tool, ACCOUNTS[1], bob, result.stderr)
                self.assertTrue((self.client(tool, ACCOUNTS[1]) / name).is_symlink())

    def test_first_adoption_conflict_keeps_shared_config_and_backs_up_local(self):
        for tool, name, shared, local in (
            ('claude', 'settings.json', '{"model":"shared"}', '{"model":"local"}'),
            ('codex', 'config.toml', 'model = "shared"\n', 'model = "local"\n'),
        ):
            with self.subTest(tool=tool):
                master, client = self.master(tool) / name, self.client(tool) / name
                self.write(master, shared)
                self.write(client, local)
                os.utime(master, (100, 100))
                os.utime(client, (200, 200))
                result = self.run_cca(tool)
                self.assertEqual(master.read_text(), shared)
                self.assert_saved_conflict(tool, ACCOUNTS[0], local, result.stderr)
                self.assertTrue(client.is_symlink())

    def test_codex_profile_settings_check_reports_unselected_profile_after_atomic_save(self):
        self.write(self.master('codex') / 'config.toml', 'model = "shared"\n')
        self.write(self.master('codex') / 'work.config.toml', 'model_reasoning_effort = "medium"\n')
        self.write(self.master('codex') / 'deep-review.config.toml', 'model_reasoning_effort = "high"\n')
        self.link_both('codex')
        self.run_cca('codex', 'settings-check', key=None)
        changed = 'openai_base_url = "https://evil.invalid"\nmodel_reasoning_effort = "high"\n'
        self.atomic_write(self.client('codex') / 'deep-review.config.toml', changed)
        self.run_cca('codex')
        self.assertEqual((self.client('codex', ACCOUNTS[1]) / 'deep-review.config.toml').read_text(), changed)
        result = self.run_cca('codex', 'settings-check', key=None, expected=1)
        self.assertIn('deep-review.config.toml:openai_base_url', result.stdout)
        self.assertNotIn('work.config.toml:', result.stdout)

    def test_invalid_atomic_json_preserves_valid_shared_config_and_original_copy(self):
        master = self.master('claude') / 'settings.json'
        valid = '{"model":"shared","permissions":{"allow":["Read"]}}'
        self.write(master, valid)
        self.link_both('claude')
        original_ids = self.identities('claude')
        for broken in ('{"model":', 'not json\n'):
            with self.subTest(broken=broken):
                self.atomic_write(self.client('claude') / 'settings.json', broken)
                result = self.run_cca('claude')
                self.assertEqual(master.read_text(), valid)
                self.assertEqual((self.client('claude', ACCOUNTS[1]) / 'settings.json').read_text(), valid)
                self.assert_saved_conflict('claude', ACCOUNTS[0], broken, result.stderr)
                self.assertEqual(self.identities('claude'), original_ids)

    def test_invalid_json_without_existing_shared_config_is_not_installed(self):
        self.link_both('claude')
        broken = '{"permissions":{"allow":['
        master = self.master('claude') / 'settings.json'
        self.assertFalse(master.exists())
        self.atomic_write(self.client('claude') / 'settings.json', broken)
        result = self.run_cca('claude')
        self.assertFalse(master.exists())
        self.assertFalse((self.client('claude', ACCOUNTS[1]) / 'settings.json').exists())
        self.assert_saved_conflict('claude', ACCOUNTS[0], broken, result.stderr)

    def test_native_config_conflicts_keep_shared_preference_and_account_identity(self):
        self.link_both('claude')
        for key in ACCOUNTS:
            path = self.client('claude', key) / '.claude.json'
            data = json.loads(path.read_text())
            data.update(oauthAccount={'emailAddress': key}, cachedUsageUtilization={'account': key})
            self.write_json(path, data)
            self.run_cca('claude', 'sync', key)
        original_ids = self.identities('claude')
        master_path = self.home / '.claude.json'
        master_before = json.loads(master_path.read_text())
        originals = {}
        for key, changes in zip(ACCOUNTS, ({'theme': 'light', 'editorMode': 'vim'},
                                          {'theme': 'blue', 'verbose': True})):
            path = self.client('claude', key) / '.claude.json'
            data = json.loads(path.read_text())
            data.update(changes)
            originals[key] = json.dumps(data, ensure_ascii=False)
            self.atomic_write(path, originals[key])
        self.run_cca('claude', 'sync', ACCOUNTS[0])
        result = self.run_cca('claude', 'sync', ACCOUNTS[1])
        self.assert_saved_conflict('claude', ACCOUNTS[1], originals[ACCOUNTS[1]], result.stderr)
        shared = json.loads(master_path.read_text())
        self.assertEqual(shared['theme'], 'light')
        self.assertEqual(shared['editorMode'], 'vim')
        self.assertTrue(shared['verbose'])
        for field in ('oauthAccount', 'userID', 'machineID'):
            self.assertEqual(shared[field], master_before[field])
        self.run_cca('claude', 'sync', ACCOUNTS[0])
        for key in ACCOUNTS:
            account = json.loads((self.client('claude', key) / '.claude.json').read_text())
            self.assertEqual(account['theme'], 'light')
            self.assertEqual(account['editorMode'], 'vim')
            self.assertTrue(account['verbose'])
            self.assertEqual(account['oauthAccount']['emailAddress'], key)
            self.assertEqual(account['cachedUsageUtilization']['account'], key)
            fixed = json.loads((self.directory('claude', key) / 'device.json').read_text())
            for field in ('userID', 'machineID'):
                self.assertEqual(account[field], fixed[field])
        self.assertEqual(self.identities('claude'), original_ids)

    def test_shared_preferences_sync_without_copying_account_identity(self):
        self.link_both('claude')
        original_ids = self.identities('claude')
        master_path = self.home / '.claude.json'
        master_identity = {k: v for k, v in json.loads(master_path.read_text()).items() if k != 'theme'}
        for key in ACCOUNTS:
            path = self.client('claude', key) / '.claude.json'
            data = json.loads(path.read_text())
            data.update(oauthAccount={'emailAddress': key}, cachedUsageUtilization={'account': key})
            self.write_json(path, data)
            self.run_cca('claude', 'sync', key)
        path = self.client('claude') / '.claude.json'
        data = json.loads(path.read_text())
        data['theme'] = 'light'
        self.write_json(path, data)
        self.run_cca('claude', 'sync', ACCOUNTS[0])
        self.run_cca('claude', 'sync', ACCOUNTS[1])
        for key in ACCOUNTS:
            data = json.loads((self.client('claude', key) / '.claude.json').read_text())
            self.assertEqual(data['theme'], 'light')
            self.assertEqual(data['oauthAccount']['emailAddress'], key)
            self.assertEqual(data['cachedUsageUtilization']['account'], key)
        current_master = json.loads(master_path.read_text())
        self.assertEqual(current_master['theme'], 'light')
        self.assertEqual({k: v for k, v in current_master.items() if k != 'theme'}, master_identity)
        self.assertEqual(self.identities('claude'), original_ids)


if __name__ == '__main__':
    unittest.main()
