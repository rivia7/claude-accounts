#!/usr/bin/env python3
"""Credential-source isolation using the actual shell environment functions.

Default tests use fake clients. Set CCA_TEST_CLAUDE_BIN to explicitly test an
installed Claude binary with temporary HOME, dummy tokens and a fake keychain.
The native checks only run local `auth status`; no real credentials are supplied.
"""
import fcntl
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest


SOURCE = (Path(__file__).resolve().parents[1] / 'cca').read_text()
ENV = SOURCE[SOURCE.index("INHERITED_RE='^"):SOURCE.index('\npyenv()')]
APP_OPEN = SOURCE[SOURCE.index('app_open() {'):SOURCE.index('\nclaude_app_first()')]
DESCRIPTORS = ('CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR', 'CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR',
               'CLAUDE_CODE_GATEWAY_TOKEN_FILE_DESCRIPTOR', 'CLAUDE_CODE_WEBSOCKET_AUTH_FILE_DESCRIPTOR')
NATIVE = os.environ.get('CCA_TEST_CLAUDE_BIN', '')


class CredentialIsolation(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-credential-isolation-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.home = self.root / 'alice'
        self.home.mkdir()
        self.env = {
            'HOME': str(self.root), 'PATH': str(self.bin) + ':/usr/bin:/bin:/usr/sbin:/sbin',
            'USER': 'cca-isolation-fixture', 'PYTHON': sys.executable,
            'ACC_HOME': str(self.home), 'ACC_KEY': 'alice@example.com', 'ACC_KIND': 'oauth',
            'ACC_PROXY': 'http://127.0.0.1:9', 'CCA_NO_PROXY': '', 'CCA_PROXY_BYPASS': '',
            'CCA_HOME': str(self.root / '.cca'), 'AUDIT_ROOT': str(self.root),
            'CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC': '1', 'DISABLE_AUTOUPDATER': '1',
        }

    def executable(self, name, content):
        path = self.bin / name
        path.write_text('#!/bin/bash\n' + content)
        path.chmod(0o755)
        return str(path)

    def shell(self, body, extra=None, pass_fds=()):
        return subprocess.run(['/bin/bash', '-c', ENV + '\ntool() { claude_env; }\n' + body],
                              env=dict(self.env, **(extra or {})), cwd=self.root,
                              stdin=subprocess.DEVNULL, capture_output=True, text=True,
                              timeout=20, pass_fds=pass_fds)

    def test_account_directory_wins_and_credential_descriptors_are_removed(self):
        client = self.executable('client', '''exec "$PYTHON" -I -c 'import json,os
print(json.dumps(dict(os.environ)))'
''')
        dirty = dict.fromkeys(DESCRIPTORS, '8')
        dirty.update(CLAUDE_CONFIG_DIR='/wrong-config', CLAUDE_SECURESTORAGE_CONFIG_DIR='/wrong-keychain',
                     CLAUDE_CODE_OAUTH_TOKEN='dummy-token', CLAUDE_CODE_MAX_OUTPUT_TOKENS='1000')
        for kind in ('oauth', 'api'):
            with self.subTest(kind=kind):
                result = self.shell('child_env\nenv "${UNSETS[@]}" "${SETS[@]}" "$CLIENT" 3<&-\n',
                                    dict(dirty, CLIENT=client, ACC_KIND=kind))
                self.assertEqual(result.returncode, 0, result.stderr)
                actual = json.loads(result.stdout)
                self.assertEqual(actual['CLAUDE_CONFIG_DIR'], str(self.home))
                self.assertEqual(actual['CLAUDE_SECURESTORAGE_CONFIG_DIR'], str(self.home))
                for name in DESCRIPTORS + ('CLAUDE_CODE_OAUTH_TOKEN',):
                    self.assertNotIn(name, actual)
                self.assertEqual(actual['CLAUDE_CODE_MAX_OUTPUT_TOKENS'], '1000')

    def test_desktop_explicitly_pins_both_configuration_and_keychain_directories(self):
        self.executable('open', '''exec "$PYTHON" -I -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "$@"
''')
        result = self.shell(APP_OPEN + '\nclaude_app_open /fixture.app "$AUDIT_ROOT/desktop"\n',
                            {'CLAUDE_SECURESTORAGE_CONFIG_DIR': '/wrong-keychain'})
        self.assertEqual(result.returncode, 0, result.stderr)
        args = json.loads(result.stdout)
        for name in ('CLAUDE_CONFIG_DIR', 'CLAUDE_SECURESTORAGE_CONFIG_DIR'):
            index = args.index(name + '=' + str(self.home))
            self.assertEqual(args[index - 1], '--env')


@unittest.skipUnless(NATIVE, 'set CCA_TEST_CLAUDE_BIN for the isolated native-client contract')
class NativeCredentialIsolation(CredentialIsolation):
    def setUp(self):
        super().setUp()
        self.native = str(Path(NATIVE).resolve(strict=True))
        self.log = self.root / 'keychain.log'
        self.executable('security', 'printf "%s\\n" "$*" >> "$AUDIT_ROOT/keychain.log"\nexit 44\n')

    def status(self, home, dirty=None, wrapped=True, pass_fds=()):
        self.log.write_text('')
        env = dict(dirty or {}, CLIENT=self.native, ACC_HOME=str(home), CLAUDE_CONFIG_DIR=str(home))
        if wrapped:
            body = 'child_env\nenv "${UNSETS[@]}" "${SETS[@]}" "$CLIENT" auth status --json 3<&-\n'
        else:
            body = '"$CLIENT" auth status --json 3<&-\n'
            env.update(HTTP_PROXY='http://127.0.0.1:9', HTTPS_PROXY='http://127.0.0.1:9',
                       ALL_PROXY='http://127.0.0.1:9', NO_PROXY='')
        result = self.shell(body, env, pass_fds)
        self.assertIn(result.returncode, (0, 1), result.stderr)
        return json.loads(result.stdout), set(re.findall(r'-s (.+)', self.log.read_text()))

    def test_native_keychain_names_are_compatible_and_different_between_accounts(self):
        stores = []
        for account in ('alice', 'bob'):
            home = self.root / account
            home.mkdir(exist_ok=True)
            baseline, old_stores = self.status(home, wrapped=False)
            checked, new_stores = self.status(home, {'CLAUDE_SECURESTORAGE_CONFIG_DIR': str(self.root / 'shared')})
            self.assertFalse(baseline['loggedIn'])
            self.assertFalse(checked['loggedIn'])
            self.assertTrue(old_stores, 'native client must use the fake security executable')
            self.assertEqual(old_stores, new_stores)
            stores.append(new_stores)
        self.assertTrue(stores[0].isdisjoint(stores[1]))

    def test_native_inherited_oauth_descriptor_cannot_replace_account_login(self):
        token = self.root / 'dummy-token'
        token.write_text('dummy-isolation-token-not-a-real-secret')
        with token.open() as source:
            descriptor = fcntl.fcntl(source.fileno(), fcntl.F_DUPFD, 10)
        try:
            dirty = {'CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR': str(descriptor)}
            baseline, _ = self.status(self.home, dirty, wrapped=False, pass_fds=(descriptor,))
            self.assertTrue(baseline['loggedIn'])
            self.assertEqual(baseline['authMethod'], 'oauth_token')
            os.lseek(descriptor, 0, os.SEEK_SET)
            checked, _ = self.status(self.home, dirty, pass_fds=(descriptor,))
            self.assertFalse(checked['loggedIn'])
        finally:
            os.close(descriptor)


if __name__ == '__main__':
    unittest.main()
