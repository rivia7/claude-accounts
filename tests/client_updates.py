#!/usr/bin/env python3
"""Central client updates: fake signed bundles/network/processes in temporary directories."""
import base64
import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import plistlib
import re
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
                    'CCA_TOOL': 'codex', 'CCA_CLIENT_CODEX_APP': str(self.app), 'CCA_CLIENT_CLAUDE_APP': '',
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

    def row(self, output, client='codex'):
        return next(c for c in json.loads(output)['clients'] if c['id'] == client)

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
        view = self.row(output)
        self.assertEqual(view['status'], 'staged', output)
        self.assertTrue(view['can_install'])
        return view['install_token']

    def test_status_is_read_only_and_never_hashes_big_files(self):
        self.ns['cu_hash'] = mock.Mock(side_effect=AssertionError('status hashed a file'))
        code, out, _ = self.command()
        self.assertEqual(code, 0)
        self.assertEqual(self.row(out)['installed_version'], '1.0')
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
        self.assertEqual(self.row(out)['status'], 'blocked')
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
        self.assertFalse(self.row(out)['can_install'])

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
            self.assertEqual(self.row(out)['status'], 'current')
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
        view = self.row(self.command()[1])
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
        self.assertEqual(self.row(out)['status'], 'updated')
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
        self.assertEqual(self.row(self.command()[1])['status'], 'needs_recheck')
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
    echo "CU_VERSION=2.0; CU_INSTALLED=1.0; CU_TARGET=/tmp/ChatGPT.app; CU_LABEL=ChatGPT"
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



class ClaudeClientUpdates(unittest.TestCase):
    """Claude App：Squirrel 的 JSON 更新源、SHA-256、和 ChatGPT 并排处理"""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-claude-update-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.claude = self.root / 'Applications/Claude.app'
        self.chatgpt = self.root / 'Applications/ChatGPT.app'
        patch = mock.patch.dict(os.environ, {
            'HOME': str(self.root), 'CCA_HOME': str(self.root / '.cca'),
            'CCA_CLIENT_CLAUDE_APP': str(self.claude), 'CCA_CLIENT_CODEX_APP': '',
            'CCA_CLIENT_VERIFIER': ''}, clear=True)
        patch.start()
        self.addCleanup(patch.stop)
        self.ns = {}
        exec(compile(DEFINITIONS, str(CCA), 'exec'), self.ns)
        self.make_app(self.claude, 'Claude', 'com.anthropic.claudefordesktop', '4.1.0', '4.1.0')
        self.calls, self.urls = [], []
        self.processes = b'1 /sbin/launchd\n'
        self.ns['cu_run'] = self.fake_run
        self.ns['cu_host'] = lambda: ('arm64', '26.0')
        self.ns['cu_download'] = self.fake_download
        self.package = self.make_zip('Claude', 'com.anthropic.claudefordesktop', '4.2.0', '4.2.0')
        self.feed_code = None
        self.feed = self.make_feed()
        self.chatgpt_feed = None

    def make_app(self, path, exe, bundle, version, build):
        (path / 'Contents/MacOS').mkdir(parents=True, exist_ok=True)
        (path / 'Contents/Resources').mkdir(exist_ok=True)
        (path / 'Contents/_CodeSignature').mkdir(exist_ok=True)
        (path / 'Contents/Info.plist').write_bytes(plistlib.dumps({
            'CFBundleIdentifier': bundle, 'CFBundleExecutable': exe,
            'CFBundleShortVersionString': version, 'CFBundleVersion': build,
            'LSMinimumSystemVersion': '13.0'}))
        (path / 'Contents/MacOS' / exe).write_bytes(b'fake binary ' + build.encode())
        (path / 'Contents/Resources/app.asar').write_bytes(b'fake app ' + build.encode())
        (path / 'Contents/_CodeSignature/CodeResources').write_bytes(b'fake seal ' + build.encode())

    def make_zip(self, exe, bundle, version, build):
        source = self.root / ('source-%s' % exe) / (exe + '.app')
        self.make_app(source, exe, bundle, version, build)
        out = io.BytesIO()
        with zipfile.ZipFile(out, 'w') as z:
            for path in sorted(source.rglob('*')):
                z.write(path, str(Path(exe + '.app') / path.relative_to(source)))
        return out.getvalue()

    def make_feed(self, version='4.2.0', url=None, digest=None, size=None, listed=None):
        url = url or 'https://downloads.claude.ai/releases/darwin/arm64/%s/Claude-%s.zip' % (version, 'a' * 40)
        item = {'name': 'Claude ' + version, 'version': version, 'url': url,
                'sha256': hashlib.sha256(self.package).hexdigest() if digest is None else digest,
                'size': len(self.package) if size is None else size,
                'delta': {'from_version': '4.1.0', 'url': 'https://example.invalid/evil.delta'}}
        return json.dumps({'currentRelease': listed or version, 'releases': [{'version': version, 'updateTo': item}]}).encode()

    def fake_run(self, args, folder, timeout=30):
        self.calls.append(args)
        out = b''
        target = ' '.join(args)
        if '--display' in args:
            out = (b'TeamIdentifier=2DC432GLL2\nIdentifier=com.openai.codex\n' if 'ChatGPT.app' in target
                   else b'TeamIdentifier=Q6L2SF6YDW\nIdentifier=com.anthropic.claudefordesktop\n')
        if '-archs' in args:
            out = b'arm64\n'
        if args[0] == '/bin/ps':
            out = self.processes
        return types.SimpleNamespace(returncode=0, stdout=out, stderr=b'')

    def fake_download(self, url, destination, folder, package=False, expected_size=None, empty=()):
        self.ns['cu_url'](url, package)   # 真规则：地址不合规就抛错
        self.urls.append(url)
        if package:
            data = self.chatgpt_package if 'oaistatic' in url else self.package
        elif 'oaistatic' in url:
            data = self.chatgpt_feed
        else:
            if self.feed_code:
                return self.feed_code
            data = self.feed
        Path(destination).write_bytes(data)
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

    def row(self, output, client='claude'):
        return next(c for c in json.loads(output)['clients'] if c['id'] == client)

    def use_claude(self):
        self.ns['CU'] = self.ns['CU_CLAUDE']

    def stage(self):
        code, output, errors = self.command('check', 'force')
        self.assertEqual(code, 0, errors + output)
        view = self.row(output)
        self.assertEqual(view['status'], 'staged', output)
        self.assertTrue(view['can_install'])
        return view['install_token']

    def installed_build(self):
        self.use_claude()
        return self.ns['cu_snapshot'](str(self.claude))['build']

    def test_checks_with_throwaway_device_id_and_needs_no_gui_verifier(self):
        self.assertEqual(self.command('check', 'force')[0], 0)
        self.assertEqual(self.command('check', 'force')[0], 0)
        feeds = [u for u in self.urls if 'squirrel/update' in u]
        ids = [re.search(r'device_id=([0-9a-f-]{36})&', u).group(1) for u in feeds]
        self.assertEqual(len(feeds), 2)
        self.assertNotEqual(ids[0], ids[1], 'device_id must not be reused between checks')
        self.assertTrue(all(u.startswith('https://api.anthropic.com/api/desktop/darwin/arm64/squirrel/update?') for u in feeds))
        state = (self.root / '.cca/client-updates/claude/state.json').read_text()
        self.assertNotIn(ids[0], state)
        self.assertNotIn(ids[1], state)
        self.assertEqual(sum('downloads.claude.ai' in u for u in self.urls), 1, 'same package must not be downloaded twice')

    def test_204_means_nothing_to_install(self):
        self.feed_code = '204'
        code, output, errors = self.command('check', 'force')
        self.assertEqual(code, 0, errors)
        self.assertEqual(self.row(output)['status'], 'current')
        self.assertFalse(any('downloads.claude.ai' in u for u in self.urls))

    def test_arch_feed_missing_falls_back_to_universal(self):
        original = self.fake_download
        seen = []
        def download(url, destination, folder, package=False, expected_size=None, empty=()):
            if 'squirrel/update' in url:
                seen.append(url.split('/squirrel')[0].rsplit('/', 1)[1])
                if len(seen) == 1:
                    return '404'
                self.ns['cu_url'](url, package)
                Path(destination).write_bytes(self.make_feed(url='https://downloads.claude.ai/releases/darwin/universal/4.2.0/Claude-%s.zip' % ('b' * 40)))
                os.chmod(destination, 0o600)
                return None
            return original(url, destination, folder, package, expected_size, empty)
        self.ns['cu_download'] = download
        self.assertEqual(self.command('check', 'force')[0], 0)
        self.assertEqual(seen, ['arm64', 'universal'])

    def test_same_or_older_release_is_not_downloaded(self):
        for version in ('4.1.0', '4.0.9'):
            self.feed = self.make_feed(version=version)
            code, output, _ = self.command('check', 'force')
            self.assertEqual(code, 0, output)
            self.assertEqual(self.row(output)['status'], 'current')
        self.assertFalse(any('downloads.claude.ai' in u for u in self.urls))

    def test_build_is_compared_as_version_not_text(self):
        self.make_app(self.claude, 'Claude', 'com.anthropic.claudefordesktop', '4.9.0', '4.9.0')
        self.package = self.make_zip('Claude', 'com.anthropic.claudefordesktop', '4.23118.0', '4.23118.0')
        self.feed = self.make_feed(version='4.23118.0')
        self.stage()

    def test_wrong_sha256_is_blocked_before_extracting(self):
        self.feed = self.make_feed(digest='0' * 64)
        self.ns['cu_extract'] = mock.Mock(side_effect=AssertionError('unverified extraction'))
        code, output, _ = self.command('check', 'force')
        self.assertEqual(code, 1)
        view = self.row(output)
        self.assertEqual(view['status'], 'blocked')
        self.assertFalse(view['can_install'])

    def test_feed_rejects_bad_metadata(self):
        self.use_claude()
        good = self.make_feed()
        self.assertEqual(self.ns['cu_parse_squirrel'](good, 'arm64')['build'], '4.2.0')
        self.assertEqual(self.ns['cu_parse_squirrel'](good, 'arm64')['sha256'], hashlib.sha256(self.package).hexdigest())
        bad = [self.make_feed(url='https://example.invalid/Claude.zip'),
               self.make_feed(url='http://downloads.claude.ai/releases/darwin/arm64/4.2.0/Claude-%s.zip' % ('a' * 40)),
               self.make_feed(url='https://downloads.claude.ai/releases/darwin/arm64/4.2.0/Claude-%s.delta' % ('a' * 40)),
               self.make_feed(url='https://downloads.claude.ai/releases/darwin/x64/4.2.0/Claude-%s.zip' % ('a' * 40)),
               self.make_feed(url='https://downloads.claude.ai/releases/darwin/arm64/4.1.0/Claude-%s.zip' % ('a' * 40)),
               self.make_feed(digest='nope'), self.make_feed(size=0), self.make_feed(size=True),
               self.make_feed(listed='9.9.9'), b'not json', b'[]', b'{"currentRelease": 1, "releases": []}']
        for data in bad:
            with self.subTest(data=data[:120]), self.assertRaises(self.ns['ClientUpdateError']):
                self.ns['cu_parse_squirrel'](data, 'arm64')

    def test_redirect_to_other_host_is_refused(self):
        self.use_claude()
        for url, package in (('https://storage.example.invalid/Claude.zip', True),
                             ('https://api.anthropic.com/api/desktop/darwin/arm64/squirrel/update', False),
                             ('https://api.anthropic.com/api/desktop/darwin/arm64/squirrel/update?device_id=x&version=1&os_version=2', False)):
            with self.subTest(url=url), self.assertRaises(self.ns['ClientUpdateError']):
                self.ns['cu_url'](url, package)

    def test_extract_needs_the_claude_root(self):
        self.use_claude()
        archive = self.root / 'other.zip'
        with zipfile.ZipFile(archive, 'w') as z:
            z.writestr('ChatGPT.app/Contents/x', b'x')
        output = self.root / 'other'
        output.mkdir()
        with self.assertRaises(self.ns['ClientUpdateError']):
            self.ns['cu_extract'](str(archive), str(output))

    def test_wrong_team_or_bundle_is_blocked(self):
        original = self.fake_run
        def run(args, folder, timeout=30):
            result = original(args, folder, timeout)
            if '--display' in args:
                result.stdout = b'TeamIdentifier=AAAAAAAAAA\nIdentifier=com.anthropic.claudefordesktop\n'
            return result
        self.ns['cu_run'] = run
        code, output, _ = self.command('check', 'force')
        self.assertEqual(code, 1)
        self.assertEqual(self.row(output)['status'], 'blocked')
        self.assertEqual(self.urls, [], 'must not contact the feed for an unverifiable client')

    def test_install_replaces_claude_and_labels_the_consent(self):
        token = self.stage()
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.ns['cmd_client_updates']('plan', token)
        self.assertIn('CU_LABEL=Claude', output.getvalue())
        self.assertIn('CU_VERSION=4.2.0', output.getvalue())
        code, out, errors = self.command('install', token)
        self.assertEqual(code, 0, errors + out)
        self.assertEqual(self.installed_build(), '4.2.0')
        self.assertEqual(self.row(out)['status'], 'updated')
        self.assertEqual(list(self.claude.parent.glob('.cca-client-install-*')), [])

    def test_package_is_not_kept_without_a_pending_update(self):
        package = self.root / '.cca/client-updates/claude/package.zip'
        self.stage()
        self.assertTrue(package.exists())
        token = self.row(self.command()[1])['install_token']
        self.assertEqual(self.command('install', token)[0], 0)
        self.assertFalse(package.exists(), 'installed: the staged zip must not linger')
        self.make_app(self.claude, 'Claude', 'com.anthropic.claudefordesktop', '4.1.0', '4.1.0')
        self.stage()
        self.assertTrue(package.exists())
        self.feed_code = '204'
        self.assertEqual(self.command('check', 'force')[0], 0)
        self.assertFalse(package.exists(), 'nothing to install: the staged zip must not linger')

    def test_any_process_from_the_bundle_blocks_install(self):
        token = self.stage()
        for command in (str(self.claude) + '/Contents/MacOS/Claude --user-data-dir=/x',
                        str(self.claude) + '/Contents/Frameworks/Claude Helper.app/Contents/MacOS/Claude Helper --type=gpu',
                        str(self.claude) + '/Contents/Frameworks/Squirrel.framework/Resources/ShipIt com.anthropic.claudefordesktop.ShipIt /x'):
            self.processes = ('88 ' + command + '\n').encode()
            code, output, errors = self.command('install', token)
            self.assertEqual(code, 1, output)
            self.assertIn('Claude', errors)
            self.assertEqual(self.installed_build(), '4.1.0')
        self.processes = b'1 /sbin/launchd\n'
        self.assertEqual(self.command('install', token)[0], 0)

    def test_native_update_before_install_stops_it(self):
        token = self.stage()
        self.make_app(self.claude, 'Claude', 'com.anthropic.claudefordesktop', '4.3.0', '4.3.0')
        self.assertEqual(self.command('install', token)[0], 1)
        self.assertEqual(self.installed_build(), '4.3.0')

    def test_not_installed_is_not_a_failure(self):
        os.environ['CCA_CLIENT_CLAUDE_APP'] = ''
        code, output, errors = self.command('check', 'force')
        self.assertEqual(code, 0, errors + output)
        self.assertEqual(self.row(output)['status'], 'missing')
        self.assertEqual(self.urls, [])
        self.assertFalse((self.root / '.cca/client-updates/claude').exists())
        self.assertFalse((self.root / '.cca/client-updates/codex').exists())

    def add_chatgpt(self):
        os.environ['CCA_CLIENT_CODEX_APP'] = str(self.chatgpt)
        os.environ['CCA_CLIENT_VERIFIER'] = str(self.root / 'verifier')
        (self.root / 'verifier').write_text('fake verifier')
        self.make_app(self.chatgpt, 'ChatGPT', 'com.openai.codex', '1.0', '100')
        self.chatgpt_package = self.make_zip('ChatGPT', 'com.openai.codex', '2.0', '200')
        signature = base64.b64encode(b'X' * 64).decode()
        self.chatgpt_feed = (f'<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"><channel><item>'
                             f'<sparkle:version>200</sparkle:version><sparkle:shortVersionString>2.0</sparkle:shortVersionString>'
                             f'<sparkle:hardwareRequirements>arm64</sparkle:hardwareRequirements>'
                             f'<sparkle:minimumSystemVersion>13.0</sparkle:minimumSystemVersion>'
                             f'<enclosure url="https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-2.0.zip" '
                             f'length="{len(self.chatgpt_package)}" sparkle:edSignature="{signature}"/>'
                             '</item></channel></rss>').encode()

    def test_both_clients_are_listed_claude_first_with_separate_tokens(self):
        self.add_chatgpt()
        code, output, errors = self.command('check', 'force')
        self.assertEqual(code, 0, errors + output)
        doc = json.loads(output)
        self.assertEqual([c['id'] for c in doc['clients']], ['claude', 'codex', 'claude-cli', 'codex-cli'])
        self.assertEqual([c['status'] for c in doc['clients'][2:]], ['missing', 'missing'])
        claude, codex = doc['clients'][:2]
        self.assertTrue(claude['can_install'] and codex['can_install'])
        self.assertNotEqual(claude['install_token'], codex['install_token'])
        code, out, errors = self.command('install', codex['install_token'])
        self.assertEqual(code, 0, errors + out)
        self.ns['CU'] = self.ns['CU_CODEX']
        self.assertEqual(self.ns['cu_snapshot'](str(self.chatgpt))['build'], '200')
        self.assertEqual(self.installed_build(), '4.1.0', 'installing ChatGPT must not touch Claude')
        self.assertEqual(self.row(out)['status'], 'staged')
        self.assertTrue(self.row(out)['can_install'])

    def test_one_client_failing_does_not_stop_the_other(self):
        self.add_chatgpt()
        self.feed = self.make_feed(digest='0' * 64)
        code, output, errors = self.command('check', 'force')
        self.assertEqual(code, 1)
        self.assertEqual(self.row(output)['status'], 'blocked')
        codex = self.row(output, 'codex')
        self.assertEqual(codex['status'], 'staged', output)
        self.assertTrue(codex['can_install'])
        self.assertIn('校验值', errors)

    def test_time_budget_stops_new_downloads(self):
        self.use_claude()
        self.ns['CU_BUDGET'] = self.ns['time'].time() + 5
        with self.assertRaises(self.ns['ClientUpdateError']):
            self.ns['cu_time'](300)
        self.ns['CU_BUDGET'] = self.ns['time'].time() + 100
        self.assertLessEqual(self.ns['cu_time'](300), 100)
        self.ns['CU_BUDGET'] = None
        self.assertEqual(self.ns['cu_time'](300), 300)

    def test_next_check_time_tells_the_app_how_long_to_sleep(self):
        rows = [{'status': 'current', 'checked_epoch': 1000.0}, {'status': 'current', 'checked_epoch': 500.0},
                {'status': 'missing', 'checked_epoch': None}]
        self.assertEqual(self.ns['cu_next_check'](rows), 500.0 + self.ns['CU_CHECK_INTERVAL'])
        self.assertEqual(self.ns['CU_CHECK_INTERVAL'], 6 * 3600)
        rows[0]['checked_epoch'] = None   # 没查过的：已经到时间了
        self.assertEqual(self.ns['cu_next_check'](rows), self.ns['CU_CHECK_INTERVAL'])
        self.assertIsNone(self.ns['cu_next_check']([{'status': 'missing'}]))
        self.assertEqual(self.command('mode', 'off')[0], 0)
        self.assertIsNone(self.ns['cu_next_check']([{'status': 'current', 'checked_epoch': 1.0}]))

    def test_check_time_is_saved_before_the_download(self):
        self.ns['cu_download'] = mock.Mock(side_effect=KeyboardInterrupt())
        with self.assertRaises(KeyboardInterrupt):
            self.command('check', 'force')
        state = json.loads((self.root / '.cca/client-updates/claude/state.json').read_text())
        self.assertGreater(state['checked_epoch'], 0)


class CliToolUpdates(unittest.TestCase):
    """终端里的 Claude Code、Codex：只查版本，更新用它们自己的 update 命令，用户确认后才运行"""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-cli-update-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.claude = self.tool('.local/share/claude/versions/2.0.0')
        self.codex = self.tool('.codex/packages/standalone/current/bin/codex')
        env = {'HOME': str(self.root), 'CCA_HOME': str(self.root / '.cca'), 'CCA_TOOL': 'claude', 'PATH': '/usr/bin:/bin',
               'CCA_CLIENT_CLAUDE_APP': '', 'CCA_CLIENT_CODEX_APP': '', 'CCA_CLI_CLAUDE': str(self.claude), 'CCA_CLI_CODEX': '',
               'ANTHROPIC_API_KEY': 'not-a-real-key', 'OPENAI_API_KEY': 'not-a-real-key', 'HTTPS_PROXY': 'http://example.invalid',
               'CLAUDE_CONFIG_DIR': '/do-not-read', 'CODEX_HOME': '/do-not-read'}
        patch = mock.patch.dict(os.environ, env, clear=True)
        patch.start()
        self.addCleanup(patch.stop)
        self.ns = {}
        exec(compile(DEFINITIONS, str(CCA), 'exec'), self.ns)
        self.installed, self.latest, self.update_works = '2.0.0', '2.1.0', True
        self.runs, self.urls = [], []
        self.body = None
        self.ns['cu_run'] = self.fake_run
        self.ns['cu_download'] = self.fake_download

    def tool(self, relative):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text('fake')
        return path

    def fake_run(self, args, folder, timeout=30, env=None):
        self.runs.append((args, env, timeout))
        if args[-1] == '--version':
            return types.SimpleNamespace(returncode=0, stdout=(self.installed + ' (Claude Code)').encode(), stderr=b'')
        if self.update_works:
            self.installed = self.latest
            return types.SimpleNamespace(returncode=0, stdout=b'done', stderr=b'')
        return types.SimpleNamespace(returncode=1, stdout=b'', stderr=b'first line\nnetwork is down')

    def fake_download(self, url, destination, folder, package=False, expected_size=None):
        self.ns['cu_url'](url, package)
        self.urls.append(url)
        Path(destination).write_bytes(self.body if self.body is not None else self.latest.encode() + b'\n')

    def command(self, action='status', arg=''):
        output, errors = io.StringIO(), io.StringIO()
        code = 0
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(errors):
            try:
                self.ns['cmd_client_updates'](action, arg)
            except SystemExit as e:
                code = e.code
        return code, output.getvalue(), errors.getvalue()

    def row(self, output, client='claude-cli'):
        return next(c for c in json.loads(output)['clients'] if c['id'] == client)

    def updates(self):
        return [r for r in self.runs if r[0][-1] == 'update']

    def check(self):
        code, output, errors = self.command('check', 'force')
        self.assertEqual(code, 0, errors + output)
        return self.row(output)

    def test_status_is_read_only_and_lists_the_terminal_tools(self):
        code, output, _ = self.command()
        self.assertEqual(code, 0)
        row = self.row(output)
        self.assertEqual((row['kind'], row['status'], row['installed_version'], row['path']), ('cli', 'unchecked', '2.0.0', str(self.claude)))
        self.assertEqual(self.row(output, 'codex-cli')['status'], 'missing')
        self.assertEqual(self.urls, [])
        self.assertFalse((self.root / '.cca').exists())

    def test_check_finds_a_new_version_but_never_installs(self):
        row = self.check()
        self.assertEqual((row['status'], row['available_version'], row['can_install']), ('available', '2.1.0', True))
        self.assertTrue(row['install_token'])
        self.assertEqual(self.updates(), [])
        self.assertEqual(self.urls, ['https://downloads.claude.ai/claude-code-releases/latest'])

    def test_same_or_newer_is_current(self):
        for latest in ('2.0.0', '1.9.9'):
            self.latest = latest
            row = self.check()
            self.assertEqual((row['status'], row['can_install'], row['install_token']), ('current', False, None))

    def test_background_check_is_throttled_and_mode_off_stops_it(self):
        self.check()
        self.urls.clear()
        self.assertEqual(self.command('check')[0], 0)
        self.assertEqual(self.urls, [])
        state = self.root / '.cca/client-updates/claude-cli/state.json'
        data = json.loads(state.read_text())
        data['checked_epoch'] = 1
        state.write_text(json.dumps(data))
        self.assertEqual(self.command('mode', 'off')[0], 0)
        self.assertEqual(self.command('check')[0], 0)
        self.assertEqual(self.urls, [])

    def test_tool_that_updated_itself_is_current_without_a_stale_button(self):
        self.check()
        self.installed = '2.1.0'
        row = self.row(self.command()[1])
        self.assertEqual((row['status'], row['can_install'], row['install_token']), ('current', False, None))

    def test_install_runs_the_official_updater_in_a_clean_environment(self):
        token = self.check()['install_token']
        code, output, errors = self.command('install', token)
        self.assertEqual(code, 0, errors + output)
        row = self.row(output)
        self.assertEqual((row['status'], row['installed_version'], row['can_install']), ('updated', '2.1.0', False))
        args, env, timeout = self.updates()[0]
        self.assertEqual(args, [str(self.claude), 'update'])
        self.assertEqual(env['HOME'], str(self.root))
        self.assertTrue(env['PATH'].startswith(str(self.root / '.local/bin') + ':'))
        self.assertNotIn(env['CLAUDE_CONFIG_DIR'], ('/do-not-read', str(self.root / '.claude')))
        self.assertEqual((env['DISABLE_TELEMETRY'], env['DISABLE_ERROR_REPORTING']), ('1', '1'))
        for name in ('ANTHROPIC_API_KEY', 'OPENAI_API_KEY', 'HTTPS_PROXY', 'CODEX_HOME'):
            self.assertNotIn(name, env)

    def test_codex_uses_github_and_its_own_updater(self):
        os.environ['CCA_CLI_CLAUDE'] = ''
        os.environ['CCA_CLI_CODEX'] = str(self.codex)
        self.installed, self.latest = '0.166.0', '0.167.0'
        self.body = json.dumps({'tag_name': 'rust-v0.167.0'}).encode()
        code, output, errors = self.command('check', 'force')
        self.assertEqual(code, 0, errors + output)
        row = self.row(output, 'codex-cli')
        self.assertEqual((row['status'], row['available_version'], row['can_install']), ('available', '0.167.0', True))
        self.assertEqual(self.urls, ['https://api.github.com/repos/openai/codex/releases/latest'])
        code, output, errors = self.command('install', row['install_token'])
        self.assertEqual(code, 0, errors + output)
        _, env, _ = self.updates()[0]
        self.assertEqual(env['CODEX_NON_INTERACTIVE'], '1')
        self.assertNotIn('CODEX_HOME', env)

    def test_failed_update_keeps_the_button_and_says_why(self):
        self.update_works = False
        token = self.check()['install_token']
        code, output, errors = self.command('install', token)
        self.assertEqual(code, 1)
        self.assertIn('network is down', errors)
        row = self.row(output)
        self.assertEqual(row['status'], 'error')
        self.assertFalse(row['can_install'])
        again = self.row(self.command()[1])
        self.assertEqual((again['status'], again['can_install']), ('available', True))

    def test_stale_or_foreign_token_runs_nothing(self):
        token = self.check()['install_token']
        for bad in ('b' * 64, 'not-a-token'):
            self.assertEqual(self.command('install', bad)[0], 1)
        self.installed = '2.1.0'
        self.assertEqual(self.command('install', token)[0], 1)
        self.assertEqual(self.updates(), [])

    def test_homebrew_install_is_checked_against_the_cask_and_left_to_the_user(self):
        cask = self.tool('opt/Caskroom/claude-code@latest/2.0.0/claude')
        os.environ['CCA_CLI_CLAUDE'] = str(cask)
        self.body = json.dumps({'version': '2.1.0'}).encode()
        row = self.check()
        self.assertEqual(self.urls, ['https://formulae.brew.sh/api/cask/claude-code@latest.json'])
        self.assertEqual((row['status'], row['can_install'], row['install_token']), ('available', False, None))
        self.assertIn('brew upgrade --cask claude-code@latest', row['message'])

    def test_npm_install_is_checked_against_npm_and_left_to_the_user(self):
        npm = self.tool('lib/node_modules/@anthropic-ai/claude-code/cli.js')
        os.environ['CCA_CLI_CLAUDE'] = str(npm)
        self.body = json.dumps({'version': '2.1.0'}).encode()
        row = self.check()
        self.assertEqual(self.urls, ['https://registry.npmjs.org/@anthropic-ai%2Fclaude-code/latest'])
        self.assertEqual((row['status'], row['can_install']), ('available', False))
        self.assertIn('npm install -g @anthropic-ai/claude-code@latest', row['message'])

    def test_claude_channel_is_respected(self):
        settings = self.root / '.claude/settings.json'
        settings.parent.mkdir()
        settings.write_text(json.dumps({'autoUpdatesChannel': 'stable'}))
        token = self.check()['install_token']
        self.assertEqual(self.urls, ['https://downloads.claude.ai/claude-code-releases/stable'])
        seen = {}

        def run(args, folder, timeout=30, env=None):
            if args[-1] == 'update':
                seen.update(json.loads(Path(env['CLAUDE_CONFIG_DIR'], 'settings.json').read_text()))
            return self.fake_run(args, folder, timeout, env)
        self.ns['cu_run'] = run
        self.assertEqual(self.command('install', token)[0], 0)
        self.assertEqual(seen, {'autoUpdatesChannel': 'stable'})

    def test_only_the_official_addresses_are_allowed(self):
        self.ns['CU'] = self.ns['CU_CLAUDE_CLI']
        allowed = ['https://downloads.claude.ai/claude-code-releases/latest', 'https://api.github.com/repos/openai/codex/releases/latest',
                   'https://registry.npmjs.org/@openai%2Fcodex/latest', 'https://formulae.brew.sh/api/cask/codex.json']
        for url in allowed:
            self.assertEqual(self.ns['cu_url'](url), url)
        for url in ('https://downloads.claude.ai/claude-code-releases/2.0.0/claude', 'https://example.invalid/claude-code-releases/latest',
                    'http://downloads.claude.ai/claude-code-releases/latest', 'https://api.github.com/repos/evil/codex/releases/latest',
                    'https://downloads.claude.ai/claude-code-releases/latest?x=1', 'https://formulae.brew.sh/api/cask/evil.json'):
            with self.assertRaises(self.ns['ClientUpdateError'], msg=url):
                self.ns['cu_url'](url)
        with self.assertRaises(self.ns['ClientUpdateError']):
            self.ns['cu_url'](allowed[0], True)

    def test_garbage_from_the_network_is_an_error_not_an_update(self):
        for body in (b'<html>blocked</html>', b'{}', b'{"version": "latest"}', b''):
            self.body = body
            code, output, _ = self.command('check', 'force')
            self.assertEqual(code, 1, body)
            row = self.row(output)
            self.assertEqual((row['status'], row['can_install']), ('error', False), body)

    def test_unreadable_version_blocks_the_row_only(self):
        self.ns['cu_run'] = lambda args, folder, timeout=30, env=None: types.SimpleNamespace(returncode=1, stdout=b'', stderr=b'')
        row = self.row(self.command()[1])
        self.assertEqual((row['status'], row['can_install']), ('blocked', False))

    def test_consent_says_how_a_terminal_tool_is_updated(self):
        start = SHELL.index('\ncmd_clients() {') + 1
        function = SHELL[start:SHELL.index('\n}', start) + 2]
        token = 'a' * 64
        prologue = '''
die() { echo "$*" >&2; exit 1; }
fail_warn() { :; }
client_update_py() {
  if [ "$1" = plan ]; then printf '%s\\n' "CU_VERSION=2.1; CU_INSTALLED=2.0; CU_TARGET=/x/claude; CU_LABEL=Claude; CU_NOTE=${NOTE@Q}"
  else echo '{}'; fi
}
consent() { printf '%s\\n' "$5" >> "$TEST_LOG"; return 0; }
'''
        prologue = prologue.replace('${NOTE@Q}', "\"'$NOTE'\"")
        for note, expected in [('用 claude update 更新', '用 claude update 更新'), ('', '目标：/x/claude')]:
            log = self.root / 'shell.log'
            log.write_text('')
            result = subprocess.run(['/bin/bash', '-c', prologue + function + '\ncmd_clients install "$1" --json', 'test', token],
                                    env={'PATH': '/usr/bin:/bin', 'TEST_LOG': str(log), 'NOTE': note}, capture_output=True, text=True)
            self.assertIn(expected, log.read_text(), result.stderr)


if __name__ == '__main__':
    unittest.main()
