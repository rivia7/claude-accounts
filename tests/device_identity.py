#!/usr/bin/env python3
"""设备标识迁移与持久隔离：只运行 cca 内嵌 Python，所有数据都在临时 HOME。"""
import concurrent.futures
import base64
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
import uuid


CCA = Path(__file__).resolve().parents[1] / 'cca'
SCRIPT = CCA.read_text()
SOURCE = SCRIPT.split("PY_SRC=$(cat <<'PY'\n", 1)[1].split('\nPY\n)', 1)[0]
PY_BOOT = SCRIPT.split("PY_BOOT='", 1)[1].split("'\n", 1)[0]
SOURCE_PACKED = base64.b64encode(gzip.compress(SOURCE.encode(), mtime=0)).decode()
ACCOUNTS = ('alice@example.com', 'bob@example.com')


class DeviceIdentity(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-device-')
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.cca = self.home / '.cca'
        self.cca.mkdir()
        self.write_json(self.cca / 'accounts.json', dict.fromkeys(ACCOUNTS, 'US-A'))
        self.write_json(self.cca / 'codex.json', dict.fromkeys(ACCOUNTS, 'US-A'))
        self.write_json(self.home / '.claude.json', {'theme': 'dark', 'userID': 'a' * 64, 'machineID': 'b' * 64})
        (self.home / '.codex').mkdir()
        (self.home / '.codex' / 'installation_id').write_text('11111111-1111-4111-8111-111111111111\n')

    def write_json(self, path, value):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value))

    def run_cca(self, tool, command, key=ACCOUNTS[0]):
        result = subprocess.run(
            [sys.executable, '-I', '-X', 'utf8', '-c', PY_BOOT, '', SOURCE_PACKED, command, key],
            env={'HOME': str(self.home), 'PATH': '/usr/bin:/bin', 'CCA_TOOL': tool, 'CCA_POLICY_DEFAULT': ''},
            capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode, 0, result.stderr)

    def directory(self, tool, key=ACCOUNTS[0]):
        email = key.split('(')[-1].rstrip(')')
        ident = re.sub('[^a-z0-9]+', '-', email.lower()).strip('-')[:20].strip('-')
        ident += '-' + hashlib.sha1(email.encode()).hexdigest()[:6]
        return self.cca / ('codex/accounts' if tool == 'codex' else 'accounts') / ident

    def client_path(self, tool, key=ACCOUNTS[0]):
        return self.directory(tool, key) / tool / ('installation_id' if tool == 'codex' else '.claude.json')

    def fields(self, tool):
        return ('installation_id',) if tool == 'codex' else ('userID', 'machineID')

    def identity(self, tool, key=ACCOUNTS[0]):
        data = json.loads((self.directory(tool, key) / 'device.json').read_text())
        self.assertEqual(data['version'], 1)
        for field in self.fields(tool):
            if tool == 'codex':
                self.assertEqual(str(uuid.UUID(data[field])), data[field])
            else:
                self.assertRegex(data[field], r'^[0-9a-f]{64}$')
        return {field: data[field] for field in self.fields(tool)}

    def client_identity(self, tool, key=ACCOUNTS[0]):
        path = self.client_path(tool, key)
        if tool == 'codex':
            return {'installation_id': path.read_text().strip()}
        data = json.loads(path.read_text())
        return {field: data.get(field) for field in self.fields(tool)}

    def seed_client(self, tool, value, key=ACCOUNTS[0]):
        path = self.client_path(tool, key)
        path.parent.mkdir(parents=True, exist_ok=True)
        if tool == 'codex':
            path.write_text(value['installation_id'] + '\n')
        else:
            self.write_json(path, dict(value, oauthAccount={'emailAddress': key}, localPreference=True))

    def test_new_accounts_are_distinct_and_stable_after_alias_and_sync(self):
        for tool in ('claude', 'codex'):
            with self.subTest(tool=tool):
                for key in ACCOUNTS:
                    self.run_cca(tool, 'link', key)
                identities = [self.identity(tool, key) for key in ACCOUNTS]
                for field in self.fields(tool):
                    self.assertNotEqual(identities[0][field], identities[1][field])
                for key, original in zip(ACCOUNTS, identities):
                    self.run_cca(tool, 'sync', key)
                    self.run_cca(tool, 'link', '工作(' + key + ')')
                    self.assertEqual(self.identity(tool, key), original)
                    self.assertEqual(self.client_identity(tool, key), original)
                    self.assertEqual((self.directory(tool, key) / 'device.json').stat().st_mode & 0o777, 0o600)

    def test_client_reset_restores_the_same_identity(self):
        for tool in ('claude', 'codex'):
            with self.subTest(tool=tool):
                self.run_cca(tool, 'link')
                original = self.identity(tool)
                self.client_path(tool).unlink()
                self.run_cca(tool, 'link')
                self.assertEqual(self.client_identity(tool), original)
                changed = {'installation_id': str(uuid.uuid4())} if tool == 'codex' else {'userID': 'c' * 64, 'machineID': 'd' * 64}
                self.seed_client(tool, changed)
                self.run_cca(tool, 'link')
                self.assertEqual(self.client_identity(tool), original)

    def test_unique_existing_identity_and_other_account_data_survive_migration(self):
        for tool in ('claude', 'codex'):
            with self.subTest(tool=tool):
                original = {'installation_id': str(uuid.uuid4())} if tool == 'codex' else {'userID': 'c' * 64, 'machineID': 'd' * 64}
                self.seed_client(tool, original)
                self.run_cca(tool, 'link')
                self.assertEqual(self.identity(tool), original)
                self.assertEqual(self.client_identity(tool), original)
                if tool == 'claude':
                    data = json.loads(self.client_path(tool).read_text())
                    self.assertTrue(data['localPreference'])
                    self.assertEqual(data['oauthAccount']['emailAddress'], ACCOUNTS[0])

    def test_duplicate_legacy_profiles_get_independent_values(self):
        for tool in ('claude', 'codex'):
            with self.subTest(tool=tool):
                duplicate = {'installation_id': str(uuid.uuid4())} if tool == 'codex' else {'userID': 'c' * 64, 'machineID': 'd' * 64}
                for key in ACCOUNTS:
                    self.seed_client(tool, duplicate, key)
                for key in ACCOUNTS:
                    self.run_cca(tool, 'link', key)
                first, second = [self.identity(tool, key) for key in ACCOUNTS]
                for field in self.fields(tool):
                    self.assertNotEqual(first[field], second[field])
                for key, original in zip(ACCOUNTS, (first, second)):
                    self.run_cca(tool, 'link', key)
                    self.assertEqual(self.identity(tool, key), original)

    def test_shared_links_are_detached_without_changing_master(self):
        for tool in ('claude', 'codex'):
            for kind, key in zip(('symlink', 'hardlink'), ACCOUNTS):
                with self.subTest(tool=tool, kind=kind):
                    master = self.home / ('.codex/installation_id' if tool == 'codex' else '.claude.json')
                    before = master.read_bytes()
                    path = self.client_path(tool, key)
                    path.parent.mkdir(parents=True, exist_ok=True)
                    if kind == 'symlink':
                        path.symlink_to(master)
                    else:
                        os.link(master, path)
                    self.run_cca(tool, 'link', key)
                    self.assertFalse(path.is_symlink())
                    self.assertFalse(os.path.samefile(path, master))
                    self.assertEqual(master.read_bytes(), before)
                    identity = self.identity(tool, key)
                    old = {'installation_id': before.decode().strip()} if tool == 'codex' else json.loads(before)
                    for field in self.fields(tool):
                        self.assertNotEqual(identity[field], old[field])
                    self.assertEqual(self.client_identity(tool, key), identity)

    def test_symlink_alias_of_account_does_not_rotate_identity(self):
        for tool in ('claude', 'codex'):
            with self.subTest(tool=tool):
                self.run_cca(tool, 'link')
                original = self.identity(tool)
                account = self.directory(tool)
                (account.parent / 'linked-copy').symlink_to(account, target_is_directory=True)
                for _ in range(2):
                    self.run_cca(tool, 'link')
                    self.assertEqual(self.identity(tool), original)
                    self.assertEqual(self.client_identity(tool), original)

    def test_legacy_copy_does_not_rotate_an_already_managed_account(self):
        for tool in ('claude', 'codex'):
            with self.subTest(tool=tool):
                self.run_cca(tool, 'link', ACCOUNTS[0])
                original = self.identity(tool, ACCOUNTS[0])
                self.seed_client(tool, original, ACCOUNTS[1])
                self.run_cca(tool, 'link', ACCOUNTS[0])
                self.assertEqual(self.identity(tool, ACCOUNTS[0]), original)
                self.run_cca(tool, 'link', ACCOUNTS[1])
                migrated = self.identity(tool, ACCOUNTS[1])
                for field in self.fields(tool):
                    self.assertNotEqual(original[field], migrated[field])
                for key, expected in zip(ACCOUNTS, (original, migrated)):
                    self.run_cca(tool, 'link', key)
                    self.assertEqual(self.identity(tool, key), expected)
                    self.assertEqual(self.client_identity(tool, key), expected)

    def test_shared_identity_record_is_fully_migrated_on_first_prepare(self):
        for tool in ('claude', 'codex'):
            with self.subTest(tool=tool):
                self.run_cca(tool, 'link', ACCOUNTS[0])
                original = self.identity(tool, ACCOUNTS[0])
                first_record = self.directory(tool, ACCOUNTS[0]) / 'device.json'
                second_record = self.directory(tool, ACCOUNTS[1]) / 'device.json'
                second_record.parent.mkdir(parents=True)
                os.link(first_record, second_record)
                source_bytes = second_record.read_bytes()
                self.assertFalse(self.client_path(tool, ACCOUNTS[1]).exists())
                self.run_cca(tool, 'device', ACCOUNTS[0])
                migrated = self.identity(tool, ACCOUNTS[0])
                for field in self.fields(tool):
                    self.assertNotEqual(migrated[field], original[field])
                self.assertFalse(os.path.samefile(first_record, second_record))
                for _ in range(2):
                    self.run_cca(tool, 'device', ACCOUNTS[0])
                    self.assertEqual(self.identity(tool, ACCOUNTS[0]), migrated)
                    self.assertEqual(self.client_identity(tool, ACCOUNTS[0]), migrated)
                    self.assertEqual(second_record.read_bytes(), source_bytes)
                self.run_cca(tool, 'device', ACCOUNTS[1])
                self.assertEqual(self.identity(tool, ACCOUNTS[1]), original)
                self.assertEqual(self.client_identity(tool, ACCOUNTS[1]), original)
                self.run_cca(tool, 'device', ACCOUNTS[0])
                self.assertEqual(self.identity(tool, ACCOUNTS[0]), migrated)

    def test_sync_can_initialize_claude_without_link(self):
        self.run_cca('claude', 'sync')
        self.assertEqual(self.client_identity('claude'), self.identity('claude'))
        self.assertEqual(json.loads(self.client_path('claude').read_text())['theme'], 'dark')
        master = json.loads((self.home / '.claude.json').read_text())
        self.assertEqual(master['userID'], 'a' * 64)
        self.assertEqual(master['machineID'], 'b' * 64)

    def test_concurrent_startups_keep_one_identity_per_account(self):
        for tool in ('claude', 'codex'):
            with self.subTest(tool=tool):
                with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
                    futures = [pool.submit(self.run_cca, tool, 'link', key) for key in ACCOUNTS * 4]
                    for future in futures:
                        future.result()
                identities = [self.identity(tool, key) for key in ACCOUNTS]
                for field in self.fields(tool):
                    self.assertNotEqual(identities[0][field], identities[1][field])
                for key, original in zip(ACCOUNTS, identities):
                    self.run_cca(tool, 'link', key)
                    self.assertEqual(self.identity(tool, key), original)
                    self.assertEqual(self.client_identity(tool, key), original)


if __name__ == '__main__':
    unittest.main()
