#!/usr/bin/env python3
"""Central client updates: fake signed bundles/network/processes in temporary directories."""
import base64
import contextlib
import io
import json
import os
from pathlib import Path
import plistlib
import shutil
import stat
import subprocess
import tempfile
import types
import unittest
from unittest import mock
import zipfile


CCA = Path(__file__).resolve().parents[1] / 'cca'
SHELL = CCA.read_text()
SOURCE = SHELL.split("PY_SRC=$(cat <<'PY'\n", 1)[1].split('\nPY\n)', 1)[0]
DEFINITIONS = SOURCE.split('\nif len(sys.argv) < 2', 1)[0]


class ClientUpdates(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-client-update-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.app = self.root / 'Applications/ChatGPT.app'
        self.env = {'HOME': str(self.root), 'CCA_HOME': str(self.root / '.cca'),
                    'CCA_TOOL': 'codex', 'CCA_CLIENT_CODEX_APP': str(self.app),
                    'CCA_CLIENT_VERIFIER': str(self.root / 'verifier'),
                    'OPENAI_API_KEY': 'not-a-real-key', 'CODEX_HOME': '/do-not-read',
                    'HTTPS_PROXY': 'http://example.invalid', 'NO_PROXY': '*'}
        patch = mock.patch.dict(os.environ, self.env, clear=True)
        patch.start()
        self.addCleanup(patch.stop)
        self.ns = {}
        exec(compile(DEFINITIONS, str(CCA), 'exec'), self.ns)
        self.make_app(self.app, '1.0', '100')
        (self.root / 'verifier').write_text('fake verifier')
        self.calls = []
        self.processes = b'1 /sbin/launchd\n'
        self.failed = set()
        self.ns['cu_run'] = self.fake_run
        self.ns['cu_host'] = lambda: ('arm64', '26.0')
        self.package = self.make_zip()
        self.feed = self.make_feed()
        self.downloads = []
        self.ns['cu_download'] = self.fake_download

    def make_app(self, path, version, build):
        (path / 'Contents/MacOS').mkdir(parents=True, exist_ok=True)
        (path / 'Contents/Resources').mkdir(exist_ok=True)
        (path / 'Contents/_CodeSignature').mkdir(exist_ok=True)
        (path / 'Contents/Info.plist').write_bytes(plistlib.dumps({
            'CFBundleIdentifier': 'com.openai.codex', 'CFBundleExecutable': 'ChatGPT',
            'CFBundleShortVersionString': version, 'CFBundleVersion': build,
            'LSMinimumSystemVersion': '13.0'}))
        (path / 'Contents/MacOS/ChatGPT').write_bytes(b'fake binary ' + build.encode())
        (path / 'Contents/Resources/app.asar').write_bytes(b'fake app ' + build.encode())
        (path / 'Contents/_CodeSignature/CodeResources').write_bytes(b'fake seal ' + build.encode())

    def make_zip(self):
        source = self.root / 'source/ChatGPT.app'
        self.make_app(source, '2.0', '200')
        out = io.BytesIO()
        with zipfile.ZipFile(out, 'w') as z:
            for path in sorted(source.rglob('*')):
                z.write(path, str(Path('ChatGPT.app') / path.relative_to(source)))
        return out.getvalue()

    def make_feed(self, *, url=None, build='200', version='2.0', arch='arm64', minimum='13.0'):
        signature = base64.b64encode(b'X' * 64).decode()
        url = url or 'https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-2.0.zip'
        return (f'<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"><channel><item>'
                f'<sparkle:version>{build}</sparkle:version><sparkle:shortVersionString>{version}</sparkle:shortVersionString>'
                f'<sparkle:hardwareRequirements>{arch}</sparkle:hardwareRequirements>'
                f'<sparkle:minimumSystemVersion>{minimum}</sparkle:minimumSystemVersion>'
                f'<enclosure url="{url}" length="{len(self.package)}" sparkle:edSignature="{signature}"/>'
                '<sparkle:deltas><enclosure url="https://example.invalid/evil.delta"/></sparkle:deltas>'
                '</item></channel></rss>').encode()

    def fake_run(self, args, folder, timeout=30):
        self.calls.append(args)
        out = b''
        if args[0] in self.failed or '--verify-client-update' in args and 'verifier' in self.failed:
            return types.SimpleNamespace(returncode=1, stdout=b'', stderr=b'failed')
        if '--display' in args:
            out = b'TeamIdentifier=2DC432GLL2\nIdentifier=com.openai.codex\n'
        if '-archs' in args:
            out = b'arm64\n'
        if args[0] == '/bin/ps':
            out = self.processes
        return types.SimpleNamespace(returncode=0, stdout=out, stderr=b'')

    def fake_download(self, url, destination, folder, package=False, expected_size=None):
        self.downloads.append(url)
        Path(destination).write_bytes(self.package if package else self.feed)
        os.chmod(destination, 0o600)

    def command(self, action='status', arg=''):
        output, errors = io.StringIO(), io.StringIO()
        code = 0
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(errors):
            try:
                self.ns['cmd_client_updates'](action, arg)
            except SystemExit as e:
                code = e.code
        return code, output.getvalue(), errors.getvalue()

    def stage(self):
        code, output, errors = self.command('check', 'force')
        self.assertEqual(code, 0, errors + output)
        view = json.loads(output)['clients'][0]
        self.assertEqual(view['status'], 'staged', output)
        self.assertTrue(view['can_install'])
        return view['install_token']

    def test_status_is_read_only_and_never_hashes_big_files(self):
        self.ns['cu_hash'] = mock.Mock(side_effect=AssertionError('status hashed a file'))
        code, out, _ = self.command()
        self.assertEqual(code, 0)
        self.assertEqual(json.loads(out)['clients'][0]['installed_version'], '1.0')
        self.assertFalse((self.root / '.cca').exists())
        self.assertEqual(self.calls, [])

    def test_check_stages_but_does_not_install_and_throttles_metadata(self):
        token = self.stage()
        self.assertRegex(token, r'^[0-9a-f]{64}$')
        self.assertEqual(self.ns['cu_snapshot'](str(self.app))['build'], '100')
        original = self.ns['cu_hash']
        self.ns['cu_hash'] = mock.Mock(side_effect=AssertionError('throttled check hashed data'))
        self.assertEqual(self.command('check')[0], 0)
        self.ns['cu_hash'] = original
        self.assertEqual(len(self.downloads), 2)
        self.assertEqual(self.command('check', 'force')[0], 0)
        self.assertEqual(len(self.downloads), 3, 'same package must not be downloaded twice')

    def test_no_verifier_blocks_before_big_download(self):
        os.environ['CCA_CLIENT_VERIFIER'] = ''
        code, out, _ = self.command('check', 'force')
        self.assertEqual(code, 1)
        self.assertEqual(json.loads(out)['clients'][0]['status'], 'blocked')
        self.assertEqual(len(self.downloads), 1)

    def test_bad_current_signature_is_not_replaced(self):
        self.failed.add('/usr/bin/codesign')
        self.assertEqual(self.command('check', 'force')[0], 1)
        self.assertEqual(self.downloads, [])

    def test_bad_archive_signature_is_not_extracted(self):
        self.failed.add('verifier')
        self.ns['cu_extract'] = mock.Mock(side_effect=AssertionError('unverified extraction'))
        code, out, _ = self.command('check', 'force')
        self.assertEqual(code, 1)
        self.assertFalse(json.loads(out)['clients'][0]['can_install'])

    def test_archive_change_during_signature_is_rejected(self):
        original = self.fake_run
        def run(args, folder, timeout=30):
            if '--verify-client-update' in args:
                path = Path(args[2])
                path.write_bytes(b'Z' * path.stat().st_size)
            return original(args, folder, timeout)
        self.ns['cu_run'] = run
        self.assertEqual(self.command('check', 'force')[0], 1)

    def test_feed_ignores_deltas_and_rejects_foreign_full_package(self):
        data = self.ns['cu_parse_feed'](self.feed, 'arm64', '26.0')
        self.assertEqual(data['build'], '200')
        for feed in (self.make_feed(url='https://example.invalid/package.zip'),
                     self.make_feed(arch='x64'), self.make_feed(minimum='99.0')):
            with self.subTest(feed=feed), self.assertRaises(self.ns['ClientUpdateError']):
                self.ns['cu_parse_feed'](feed, 'arm64', '26.0')

    def test_no_equal_build_or_downgrade_package_download(self):
        for build in ('100', '99'):
            self.feed = self.make_feed(build=build)
            code, out, _ = self.command('check', 'force')
            self.assertEqual(code, 0)
            self.assertEqual(json.loads(out)['clients'][0]['status'], 'current')
        self.assertEqual(len(self.downloads), 2)

    def test_target_change_invalidates_plan_and_install(self):
        token = self.stage()
        self.make_app(self.app, '3.0', '300')
        self.assertEqual(self.command('plan', token)[0], 1)
        self.assertEqual(self.command('install', token)[0], 1)
        self.assertEqual(self.ns['cu_snapshot'](str(self.app))['build'], '300')

    def test_cached_archive_stat_change_hides_install_action(self):
        self.stage()
        path = Path(self.ns['cu_root']()) / 'package.zip'
        path.write_bytes(b'changed')
        view = json.loads(self.command()[1])['clients'][0]
        self.assertFalse(view['can_install'])
        self.assertEqual(view['status'], 'needs_recheck')

    def test_mode_off_still_works_with_broken_cache(self):
        root = Path(self.ns['cu_root'](True))
        (root / 'state.json').write_text('{bad')
        os.chmod(root / 'state.json', 0o600)
        self.assertEqual(self.command('mode', 'off')[0], 0)
        self.assertEqual(json.loads((self.root / '.cca/config.json').read_text())['client_updates'], 'off')

    def test_clean_download_environment_and_manual_redirect_boundary(self):
        environment = self.ns['cu_environment']('/private/tmp/example')
        self.assertNotIn('OPENAI_API_KEY', environment)
        self.assertNotIn('HTTPS_PROXY', environment)
        folder = self.root / 'download'
        folder.mkdir()
        def redirect(args, work, timeout=30):
            self.assertEqual(args[:2], ['/usr/bin/curl', '-q'])
            self.assertIn('--no-netrc', args)
            self.assertEqual(args[args.index('--proxy') + 1], '')
            self.assertNotIn('-L', args)
            Path(args[args.index('--dump-header') + 1]).write_text('HTTP/2 302\nLocation: https://example.invalid/stolen\n')
            return types.SimpleNamespace(returncode=0, stdout=b'302', stderr=b'')
        self.ns['cu_run'] = redirect
        # Use the unpatched production downloader.
        original = {}
        exec(compile(DEFINITIONS, str(CCA), 'exec'), original)
        original['cu_run'] = redirect
        with self.assertRaises(original['ClientUpdateError']):
            original['cu_download'](original['CU_FEED'], str(folder / 'out'), str(folder))

    def test_safe_zip_allows_internal_framework_link(self):
        archive = self.root / 'link.zip'
        with zipfile.ZipFile(archive, 'w') as z:
            z.writestr('ChatGPT.app/Contents/Frameworks/F.framework/Versions/A/file', b'ok')
            entry = zipfile.ZipInfo('ChatGPT.app/Contents/Frameworks/F.framework/Versions/Current')
            entry.create_system = 3
            entry.external_attr = (stat.S_IFLNK | 0o777) << 16
            z.writestr(entry, 'A')
        output = self.root / 'extracted'
        output.mkdir()
        app = Path(self.ns['cu_extract'](str(archive), str(output)))
        self.assertEqual((app / 'Contents/Frameworks/F.framework/Versions/Current/file').read_bytes(), b'ok')

    def test_zip_traversal_external_links_case_and_unicode_duplicates_rejected(self):
        cases = [ [('ChatGPT.app/../outside', b'x', 0)],
                  [('Other.app/file', b'x', 0)],
                  [('ChatGPT.app/A', b'x', 0), ('ChatGPT.app/a', b'y', 0)],
                  [('ChatGPT.app/é', b'x', 0), ('ChatGPT.app/e\u0301', b'y', 0)],
                  [('ChatGPT.app/link', b'../../outside', stat.S_IFLNK)],
                  [('ChatGPT.app/fifo', b'', stat.S_IFIFO)] ]
        for index, entries in enumerate(cases):
            archive = self.root / ('bad%d.zip' % index)
            with zipfile.ZipFile(archive, 'w') as z:
                for name, data, mode in entries:
                    entry = zipfile.ZipInfo(name)
                    entry.create_system = 3
                    entry.external_attr = (mode | 0o644) << 16
                    z.writestr(entry, data)
            folder = self.root / ('bad%d' % index)
            folder.mkdir()
            with self.subTest(index=index), self.assertRaises(self.ns['ClientUpdateError']):
                self.ns['cu_extract'](str(archive), str(folder))

    def test_install_waits_for_plain_instance_and_product_updater(self):
        token = self.stage()
        for command in (str(self.app) + '/Contents/MacOS/ChatGPT',
                        '/tmp/Updater.app/Contents/MacOS/Updater ' + str(self.app),
                        '/tmp/com.openai.codex/org.sparkle-project.Sparkle/Updater.app/helper'):
            self.processes = ('88 ' + command + '\n').encode()
            self.assertEqual(self.command('install', token)[0], 1)
            self.assertEqual(self.ns['cu_snapshot'](str(self.app))['build'], '100')
        self.processes = b''
        self.assertEqual(self.command('install', token)[0], 1)

    def test_successful_install_and_no_accumulating_backups(self):
        token = self.stage()
        code, out, errors = self.command('install', token)
        self.assertEqual(code, 0, errors + out)
        self.assertEqual(self.ns['cu_snapshot'](str(self.app))['build'], '200')
        self.assertEqual(json.loads(out)['clients'][0]['status'], 'updated')
        self.assertEqual(list(self.app.parent.glob('.cca-client-install-*')), [])
        self.assertEqual(self.command('install', token)[0], 1)

    def test_native_update_just_before_move_is_restored_not_downgraded(self):
        token = self.stage()
        rename = self.ns['cu_rename_exclusive']
        changed = False
        def racing(source, destination):
            nonlocal changed
            if source == str(self.app) and not changed:
                changed = True
                self.make_app(self.app, '3.0', '300')
            rename(source, destination)
        self.ns['cu_rename_exclusive'] = racing
        self.assertEqual(self.command('install', token)[0], 1)
        self.assertEqual(self.ns['cu_snapshot'](str(self.app))['build'], '300')
        self.assertFalse((Path(self.ns['cu_root']()) / 'transaction.json').exists())

    def test_new_target_during_commit_is_preserved_and_journal_does_not_loop(self):
        token = self.stage()
        rename = self.ns['cu_rename_exclusive']
        def racing(source, destination):
            if destination == str(self.app) and source.endswith('/ChatGPT.app'):
                self.make_app(self.app, '3.0', '300')
            rename(source, destination)
        self.ns['cu_rename_exclusive'] = racing
        self.assertEqual(self.command('install', token)[0], 1)
        self.assertEqual(self.ns['cu_snapshot'](str(self.app))['build'], '300')
        self.assertFalse((Path(self.ns['cu_root']()) / 'transaction.json').exists())
        self.assertEqual(self.command('check', 'force')[0], 0)

    def test_interrupted_old_move_recovers_on_check_without_install_token(self):
        token = self.stage()
        rename = self.ns['cu_rename_exclusive']
        class PowerLoss(BaseException):
            pass
        def interrupted(source, destination):
            rename(source, destination)
            if source == str(self.app):
                raise PowerLoss()
        self.ns['cu_rename_exclusive'] = interrupted
        with self.assertRaises(PowerLoss):
            self.command('install', token)
        self.assertFalse(self.app.exists())
        self.assertEqual(json.loads(self.command()[1])['clients'][0]['status'], 'needs_recheck')
        self.ns['cu_rename_exclusive'] = rename
        self.assertEqual(self.command('check')[0], 0)
        self.assertEqual(self.ns['cu_snapshot'](str(self.app))['build'], '100')
        self.assertFalse((Path(self.ns['cu_root']()) / 'transaction.json').exists())

    def test_cleanup_failure_does_not_turn_committed_install_into_failure(self):
        token = self.stage()
        rmtree = shutil.rmtree
        def cleanup(path, *args, **kwargs):
            if Path(path).name.startswith('.cca-client-install-'):
                self.assertFalse((Path(self.ns['cu_root']()) / 'transaction.json').exists())
                raise OSError('simulated cleanup failure')
            return rmtree(path, *args, **kwargs)
        with mock.patch('shutil.rmtree', cleanup):
            code, out, _ = self.command('install', token)
        self.assertEqual(code, 0, out)
        self.assertEqual(self.ns['cu_snapshot'](str(self.app))['build'], '200')

    def test_python_source_in_command_is_not_mistaken_for_sparkle(self):
        original = self.fake_run
        def process_rows(args, folder, timeout=30):
            if args[0] == '/bin/ps':
                payload = ('25 /usr/bin/python3 -c ' + SOURCE.replace('\n', ' ') + '\n').encode()
                if args[-1] == 'pid=,comm=':
                    payload = b'25 /usr/bin/python3\n'
                return types.SimpleNamespace(returncode=0, stdout=payload, stderr=b'')
            return original(args, folder, timeout)
        self.ns['cu_run'] = process_rows
        self.ns['cu_no_processes'](str(self.app))

    def test_install_lock_and_manager_lock_are_nonblocking(self):
        self.stage()
        root = Path(self.ns['cu_root']())
        for lock in (root / 'install.lock', root.parent / 'manager.lock'):
            with self.ns['ClientUpdateLock'](str(lock)):
                with self.assertRaises(self.ns['ClientUpdateError']):
                    with self.ns['ClientUpdateLock'](str(lock)):
                        self.fail('lock was not exclusive')

    def test_updater_state_directory_symlink_is_rejected(self):
        cca = self.root / '.cca'
        cca.mkdir(mode=0o700)
        other = self.root / 'unrelated'
        other.mkdir(mode=0o700)
        (cca / 'client-updates').symlink_to(other)
        self.assertEqual(self.command('check', 'force')[0], 1)
        self.assertEqual(list(other.iterdir()), [])

    def test_recovery_cleanup_failure_after_rollback_is_committed(self):
        token = self.stage()
        rename = self.ns['cu_rename_exclusive']
        class PowerLoss(BaseException):
            pass
        def interrupted(source, destination):
            rename(source, destination)
            if source == str(self.app):
                raise PowerLoss()
        self.ns['cu_rename_exclusive'] = interrupted
        with self.assertRaises(PowerLoss):
            self.command('install', token)
        self.ns['cu_rename_exclusive'] = rename
        rmtree = shutil.rmtree
        def cleanup(path, *args, **kwargs):
            if Path(path).name.startswith('.cca-client-install-'):
                self.assertFalse((Path(self.ns['cu_root']()) / 'transaction.json').exists())
                raise OSError('cleanup failure')
            return rmtree(path, *args, **kwargs)
        with mock.patch('shutil.rmtree', cleanup):
            self.assertEqual(self.command('check')[0], 0)
        self.assertEqual(self.ns['cu_snapshot'](str(self.app))['build'], '100')
        self.assertFalse((Path(self.ns['cu_root']()) / 'transaction.json').exists())

    def test_shell_requires_exact_token_authorization_and_valid_plan(self):
        start = SHELL.index('\ncmd_clients() {') + 1
        function = SHELL[start:SHELL.index('\n}', start) + 2]
        token = 'a' * 64
        prologue = '''
die() { echo "$*" >&2; exit 1; }
fail_warn() { :; }
client_update_py() {
  echo "$1 $2" >> "$TEST_LOG"
  if [ "$1" = plan ]; then
    [ "$PLAN_OK" = 1 ] || return 1
    echo "CU_VERSION=2.0; CU_INSTALLED=1.0; CU_TARGET=/tmp/ChatGPT.app"
  else echo '{}'; fi
}
consent() { echo "consent $1 $2" >> "$TEST_LOG"; return "$CONSENT_RC"; }
'''
        for supplied, plan, consent, installs in [(token, '1', '1', False),
                                                   (token, '0', '0', False),
                                                   ('not-a-token', '1', '0', False),
                                                   (token, '1', '0', True)]:
            log = self.root / 'shell.log'
            log.write_text('')
            env = {'PATH': '/usr/bin:/bin', 'TEST_LOG': str(log), 'PLAN_OK': plan, 'CONSENT_RC': consent}
            result = subprocess.run(['/bin/bash', '-c', prologue + function + '\ncmd_clients install "$1" --json',
                                     'test', supplied], env=env, capture_output=True, text=True)
            lines = log.read_text().splitlines()
            self.assertEqual('install ' + token in lines, installs, result.stderr + str(lines))
            if plan == '0' or supplied != token:
                self.assertFalse(any(x.startswith('consent ') for x in lines))
            if installs:
                self.assertIn('consent client_update ' + token, lines)


if __name__ == '__main__':
    unittest.main()
