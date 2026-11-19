"""gen_clients_vectors.py 的另一半：PY_SRC 里客户端更新这一块的函数对一批输入给出的结果（functions），和整条路的场景（scenarios）。

场景是照 tests/client_updates.py 一项一项写的：那里把 cu_run、cu_host、cu_download 换成假的，这里也是，只是假的那几样
（进程列表、哪个命令失败、更新源回什么……）不写成代码、写成数据（fake 里的几项和 hook），Rust 那边照同样的数据重放。
每个场景是一串步骤：改环境变量、写文件、改假数据、装钩子、跑一次 client-updates（cmd）、直接调一个函数（call）。
cmd / call 的结果记在这一步的 want 里。原来测试里的每一条 assert 在这里对 Python 的结果先核对（self.check）。

cli/src/cmd/clients/tests.rs 里有同一套步骤的解释器：两边对这些步骤的理解必须一样，改这里的同时改那里。"""
import base64
import contextlib
import hashlib
import io
import json
import os
import plistlib
import re
import shutil
import stat
import struct
import sys
import tempfile
import time
import zipfile
from unittest import mock

from gen_clients_vectors import DF, DIR, LNK, REG, ST, b64, make_zip, outcome, unb64

BUNDLE = {'codex': 'com.openai.codex', 'claude': 'com.anthropic.claudefordesktop'}
EXE = {'codex': 'ChatGPT', 'claude': 'Claude'}
TEAM = {'codex': '2DC432GLL2', 'claude': 'Q6L2SF6YDW'}
NS = 'http://www.andymatuschak.org/xml-namespaces/sparkle'
SIG = base64.b64encode(b'X' * 64).decode()
LAUNCHD = b'1 /sbin/launchd\n'


class PowerLoss(BaseException):
    """模拟断电：什么 except 都接不住（原来的测试里也是 BaseException）"""


def app_files(client, version, build):
    """假 App 的四个关键文件：{相对 .app 的路径: 内容}"""
    exe = EXE[client]
    return {
        'Contents/Info.plist': plistlib.dumps({'CFBundleIdentifier': BUNDLE[client], 'CFBundleExecutable': exe, 'CFBundleShortVersionString': version, 'CFBundleVersion': build,
                                               'LSMinimumSystemVersion': '13.0'}),
        'Contents/MacOS/' + exe: b'fake binary ' + build.encode(),
        'Contents/Resources/app.asar': b'fake app ' + build.encode(),
        'Contents/_CodeSignature/CodeResources': b'fake seal ' + build.encode(),
    }


def app_zip(client, version, build):
    """安装包：和原来的测试一样是把假 App 的目录树整个打进去（目录各有一项）"""
    root = EXE[client] + '.app'
    files = app_files(client, version, build)
    dirs = sorted({root + '/' + '/'.join(p.split('/')[:n]) + '/' for p in files for n in range(1, len(p.split('/')))})
    entries = [(d, b'', stat.S_IFDIR | 0o755, ST) for d in dirs] + [(root + '/' + p, data, stat.S_IFREG | 0o644, ST) for p, data in files.items()]
    return make_zip(sorted(entries))


def codex_feed(package, *, url=None, build='200', version='2.0', arch='arm64', minimum='13.0', deltas=True):
    url = url or 'https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-2.0.zip'
    return ('<rss xmlns:sparkle="%s"><channel><item><sparkle:version>%s</sparkle:version><sparkle:shortVersionString>%s</sparkle:shortVersionString>'
            '<sparkle:hardwareRequirements>%s</sparkle:hardwareRequirements><sparkle:minimumSystemVersion>%s</sparkle:minimumSystemVersion>'
            '<enclosure url="%s" length="%d" sparkle:edSignature="%s"/>%s</item></channel></rss>'
            % (NS, build, version, arch, minimum, url, len(package), SIG,
               '<sparkle:deltas><enclosure url="https://example.invalid/evil.delta"/></sparkle:deltas>' if deltas else '')).encode()


def claude_feed(package, version='4.2.0', url=None, digest=None, size=None, listed=None):
    url = url or 'https://downloads.claude.ai/releases/darwin/arm64/%s/Claude-%s.zip' % (version, 'a' * 40)
    item = {'name': 'Claude ' + version, 'version': version, 'url': url, 'sha256': hashlib.sha256(package).hexdigest() if digest is None else digest,
            'size': len(package) if size is None else size, 'delta': {'from_version': '4.1.0', 'url': 'https://example.invalid/evil.delta'}}
    return json.dumps({'currentRelease': listed or version, 'releases': [{'version': version, 'updateTo': item}]}).encode()


class Scn:
    """一个场景：临时目录里的一个假世界，一步一步做，边做边记"""
    code = None

    def __init__(self, name, defs, env, note=''):
        if Scn.code is None:
            Scn.code = compile(defs, 'PY_SRC', 'exec')
        self.name, self.note = name, note
        self.temp = tempfile.mkdtemp(prefix='cca-clients-scn-')
        self.root = os.path.realpath(self.temp)
        self.steps = []
        self.tokens = {}
        self.env = {'HOME': self.root, 'CCA_HOME': self.root + '/.cca'}
        self.env.update({k: v.replace('<ROOT>', self.root) for k, v in env.items()})
        self.patch = mock.patch.dict(os.environ, self.env, clear=True)
        self.patch.start()
        self.ns = {}
        exec(Scn.code, self.ns)
        self.fake = {'processes_b64': b64(LAUNCHD), 'comm_b64': None, 'failed': [], 'display': None, 'archs_b64': b64(b'arm64\n'), 'host': ['arm64', '26.0'],
                     'feed_codex_b64': None, 'feed_claude_b64': None, 'feed_codes': [], 'feed_code': None, 'pkg_codex_b64': None, 'pkg_claude_b64': None,
                     'cli_installed': '2.0.0', 'cli_latest': '2.1.0', 'cli_suffix': ' (Claude Code)', 'cli_update_works': True, 'cli_body_b64': None, 'cli_run_rc': None,
                     'download_raises': None, 'rmtree_fails': None, 'curl': []}
        self.hooks = []
        self.calls, self.downloads, self.slept, self.rmtree_saw, self.raw_downloads = [], [], [], [], []
        self.forbidden = set()
        self.last_tree = {}
        self.docs = {}   # 步骤号 → 那一步打出来的状态文档
        ns = self.ns
        self.real = {k: ns[k] for k in ('cu_run', 'cu_host', 'cu_download', 'cu_rename_exclusive', 'cu_hash', 'cu_extract')}
        ns['cu_run'] = self.fake_run
        ns['cu_host'] = lambda: tuple(self.fake['host'])
        ns['cu_download'] = self.fake_download
        ns['cu_rename_exclusive'] = self.fake_rename
        ns['cu_hash'] = self.guard('hash', self.real['cu_hash'])
        ns['cu_extract'] = self.guard('extract', self.real['cu_extract'])
        self.step('world', env={k: v.replace(self.root, '<ROOT>') for k, v in self.env.items() if k not in ('HOME', 'CCA_HOME')})

    def close(self):
        self.patch.stop()
        shutil.rmtree(self.temp, ignore_errors=True)
        return {'name': self.name, 'note': self.note, 'steps': self.steps}

    def path(self, rel):
        return os.path.join(self.root, rel)

    # --- 记步骤 ---
    def step(self, op, **kw):
        s = dict(op=op, **kw)
        self.steps.append(s)
        return s

    def check(self, ok, what=''):
        """tests/client_updates.py 里的一条 assert：对 Python 的结果不成立就不生成"""
        if not ok:
            raise AssertionError('%s：%s' % (self.name, what))

    # --- 归一：每次运行不一样的东西换掉。Rust 那边是同一套规矩、同一个先后 ---
    def norm(self, text):
        text = text.replace(self.root, '<ROOT>').replace(os.path.realpath(tempfile.gettempdir()), '<TMP>').replace(tempfile.gettempdir(), '<TMP>')

        def token(m):
            n = self.tokens.setdefault(m.group(2), len(self.tokens) + 1)
            return '"%s": "<TOKEN %d>"' % (m.group(1), n)
        text = re.sub(r'"(install_token|token)": "([0-9a-f]{64})"', token, text)
        gen = re.search(r'"generated_epoch": (\d{9,}\.\d+)', text)

        def next_check(m):
            return '"next_check_epoch": "<T+%d>"' % (round((float(m.group(1)) - float(gen.group(1))) / 60) * 60)
        if gen:
            text = re.sub(r'"next_check_epoch": (\d{9,}\.\d+)', next_check, text)
        text = re.sub(r'"(generated_epoch|checked_epoch)": \d{9,}\.\d+', r'"\1": "<T>"', text)
        text = re.sub(r'\[\s*\d+,\s*\d+,\s*\d+,\s*\d+,\s*\d+\s*\]', '[<ID>]', text)
        text = re.sub(r'"(device|inode|modified_ns|changed_ns)": \d+', r'"\1": "<N>"', text)
        text = re.sub(r'\b(check|update|cca-cli)-[a-z0-9_]{8}', r'\1-<R>', text)
        text = re.sub(r'\.cca-client-install-[a-z0-9_]{8}', '.cca-client-install-<R>', text)
        return re.sub(r'device_id=[0-9a-f-]{36}', 'device_id=<UUID>', text)

    def tree(self):
        out = {}
        for d, dirs, files in os.walk(self.root):
            dirs.sort()
            for n in sorted(dirs + files):
                p = os.path.join(d, n)
                rel = self.norm(os.path.relpath(p, self.root))
                st = os.lstat(p)
                if stat.S_ISLNK(st.st_mode):
                    out[rel] = 'link -> ' + self.norm(os.readlink(p))
                elif stat.S_ISDIR(st.st_mode):
                    out[rel] = 'dir %o' % (st.st_mode & 0o777)
                else:
                    with open(p, 'rb') as f:
                        data = f.read()
                    try:
                        text = data.decode('utf-8') if len(data) <= 4000 else None
                    except UnicodeDecodeError:
                        text = None
                    out[rel] = 'file %o %s' % (st.st_mode & 0o777, self.norm(text) if text is not None else 'sha256:' + hashlib.sha256(data).hexdigest())
        return out

    def tree_delta(self):
        """和上一次比变了什么（没了的是 None）：整棵树每一步都记一遍太大"""
        now = self.tree()
        delta = {k: v for k, v in now.items() if self.last_tree.get(k) != v}
        delta.update({k: None for k in self.last_tree if k not in now})
        self.last_tree = now
        return delta

    # --- 步骤 ---
    def set_env(self, key, value):
        self.step('env', key=key, value=value)
        if value is None:
            os.environ.pop(key, None)
        else:
            os.environ[key] = value.replace('<ROOT>', self.root)

    def write(self, rel, data, mode=None, dirmode=None, record=True):
        data = data.encode() if isinstance(data, str) else data
        if record:
            self.step('write', path=rel, b64=b64(data), mode=mode, dirmode=dirmode)
        p = self.path(rel)
        if dirmode is None:
            os.makedirs(os.path.dirname(p), exist_ok=True)
        else:
            os.makedirs(os.path.dirname(p), mode=dirmode, exist_ok=True)
        with open(p, 'wb') as f:
            f.write(data)
        if mode is not None:
            os.chmod(p, mode)

    def mkdir(self, rel, mode=0o700):
        self.step('mkdir', path=rel, mode=mode)
        os.makedirs(self.path(rel), mode=mode, exist_ok=True)
        os.chmod(self.path(rel), mode)

    def symlink(self, rel, target):
        self.step('symlink', path=rel, target=target)
        os.symlink(target.replace('<ROOT>', self.root), self.path(rel))

    def json_edit(self, rel, set=None, pop=None, record=True):
        """读进 JSON、改几个键、照 json.dumps 默认的写法写回去（原来的测试里 write_text(json.dumps(…))）"""
        if record:
            self.step('json_edit', path=rel, set=set or {}, pop=pop or [])
        with open(self.path(rel)) as f:
            d = json.load(f)
        d.update(set or {})
        for k in pop or []:
            d.pop(k)
        with open(self.path(rel), 'w') as f:
            f.write(json.dumps(d))

    def app(self, rel, client, version, build, record=True):
        for p, data in app_files(client, version, build).items():
            self.write(rel + '/' + p, data, record=record)

    def set_fake(self, record=True, **kw):
        for k, v in kw.items():
            if isinstance(v, bytes):
                # 进程列表里带着这个假世界的路径：记成 <ROOT>，用的时候再换回来（重放时根目录是另一个）
                k, v = k + '_b64', b64(v.replace(self.root.encode(), b'<ROOT>'))
            assert k in self.fake, k
            if record:
                self.step('fake', key=k, value=json.loads(json.dumps(v)))   # 记一份拷贝：剧本用一条少一条
            self.fake[k] = v

    def hook(self, on, when=None, once=False, before=(), after=(), overwrite=False, raises=None):
        h = {'on': on, 'when': when or {}, 'once': once, 'before': list(before), 'after': list(after), 'overwrite': overwrite, 'raise': raises}
        self.step('hook', **h)
        self.hooks.append(dict(h, fired=0, seen=0))

    def unhook(self):
        self.step('unhook')
        self.hooks = []

    def forbid(self, what, on=True):
        self.step('forbid', what=what, on=on)
        (self.forbidden.add if on else self.forbidden.discard)(what)

    def set_cu(self, client):
        self.step('cu', client=client)
        self.ns['CU'] = next(c for c in self.ns['CU_CLIENTS'] if c.id == client)

    def sub(self, steps):
        """钩子里要做的几步（只有写文件、改 JSON、改假数据）：记成数据，现在不做"""
        return steps

    def run_sub(self, steps):
        for s in steps:
            if s['op'] == 'write':
                self.write(s['path'], unb64(s['b64']), s.get('mode'), record=False)
            elif s['op'] == 'json_edit':
                self.json_edit(s['path'], s.get('set'), s.get('pop'), record=False)
            elif s['op'] == 'fake':
                self.fake[s['key']] = s['value']
            else:
                raise ValueError(s['op'])

    def app_steps(self, rel, client, version, build):
        return [{'op': 'write', 'path': rel + '/' + p, 'b64': b64(data), 'mode': None, 'dirmode': None} for p, data in app_files(client, version, build).items()]

    # --- 假的那几样 ---
    def guard(self, what, real):
        def f(*a, **kw):
            if what in self.forbidden:
                raise AssertionError('%s：这里不该调 cu_%s' % (self.name, what))
            return real(*a, **kw)
        return f

    def fire(self, on, matches, phase):
        """装了的钩子里对得上的：做它 before / after 里的几步；after 的还可能要抛异常"""
        boom = None
        for h in self.hooks:
            if h['on'] != on or not matches(h['when'], h):
                continue
            if phase == 'before':
                if h['once'] and h['fired']:
                    continue
                h['fired'] += 1
                self.run_sub(h['before'])
            else:
                self.run_sub(h['after'])
                boom = boom or h['raise']
        if boom:
            raise {'PowerLoss': PowerLoss, 'KeyboardInterrupt': KeyboardInterrupt}[boom]()

    def fake_run(self, args, folder, timeout=30, env=None):
        f = self.fake
        record = {'args': [self.norm(a) for a in args], 'folder': self.norm(folder), 'timeout': timeout, 'env': [[k, self.norm(v)] for k, v in env.items()] if env else None}
        self.calls.append(record)
        res = lambda rc, out=b'', err=b'': type('Done', (), {'returncode': rc, 'stdout': out, 'stderr': err})()

        def nth(when, h):
            if when.get('last_arg') != args[-1]:
                return False
            h['seen'] += 1
            return h['seen'] == when.get('nth', h['seen'])
        self.fire('run', nth, 'before')
        if args[0] == '/usr/bin/curl':
            r = f['curl'].pop(0)
            if r.get('header_b64') is not None:
                with open(args[args.index('--dump-header') + 1], 'wb') as h:
                    h.write(unb64(r['header_b64']))
            if r.get('body_b64') is not None:
                with open(args[args.index('--output') + 1], 'wb') as h:
                    h.write(unb64(r['body_b64']))
            if r.get('fail'):
                raise self.ns['ClientUpdateError']('error', '更新校验命令失败或超时')
            return res(r.get('rc', 0), unb64(r.get('stdout_b64', '')))
        if f['cli_run_rc'] is not None:
            return res(f['cli_run_rc'])
        if args[-1] == '--version':
            return res(0, (f['cli_installed'] + f['cli_suffix']).encode())
        if args[-1] == 'update':
            conf = dict(env or {}).get('CLAUDE_CONFIG_DIR')
            settings = os.path.join(conf, 'settings.json') if conf else None
            if settings and os.path.exists(settings):
                with open(settings) as h:
                    record['settings'] = h.read()
            if f['cli_update_works']:
                f['cli_installed'] = f['cli_latest']
                return res(0, b'done')
            return res(1, b'', b'first line\nnetwork is down')
        if args[0] in f['failed'] or ('--verify-client-update' in args and 'verifier' in f['failed']):
            return res(1, b'', b'failed')
        out = b''
        if '--verify-client-update' in args:
            for h in self.hooks:
                if h['on'] == 'verify' and h['overwrite']:
                    with open(args[2], 'r+b') as a:
                        size = os.path.getsize(args[2])
                        a.write(b'Z' * size)
        if '--display' in args:
            which = 'codex' if 'ChatGPT.app' in ' '.join(args) else 'claude'
            out = f['display'].encode() if f['display'] is not None else ('TeamIdentifier=%s\nIdentifier=%s\n' % (TEAM[which], BUNDLE[which])).encode()
        if '-archs' in args:
            out = unb64(f['archs_b64'])
        if args[0] == '/bin/ps':
            out = unb64(f['comm_b64'] if args[-1] == 'pid=,comm=' and f['comm_b64'] is not None else f['processes_b64']).replace(b'<ROOT>', self.root.encode())
        return res(0, out)

    def fake_download(self, url, destination, folder, package=False, expected_size=None, empty=()):
        f = self.fake
        self.ns['cu_url'](url, package)   # 真规则：地址不合规就抛错
        self.downloads.append(self.norm(url))
        self.raw_downloads.append(url)
        if f['download_raises']:
            raise KeyboardInterrupt()
        if package:
            data = unb64(f['pkg_codex_b64'] if 'oaistatic' in url else f['pkg_claude_b64'])
        elif 'oaistatic' in url:
            data = unb64(f['feed_codex_b64'])
        elif 'squirrel/update' in url:
            if f['feed_codes']:
                code = f['feed_codes'].pop(0)
                if code:
                    return code
            elif f['feed_code']:
                return f['feed_code']
            data = unb64(f['feed_claude_b64'])
        else:
            data = unb64(f['cli_body_b64']) if f['cli_body_b64'] is not None else (f['cli_latest'] + '\n').encode()
        with open(destination, 'wb') as h:
            h.write(data)
        os.chmod(destination, 0o600)
        self.fire('download', lambda when, h: when.get('package') == bool(package), 'after')
        return None

    def fake_rename(self, source, destination):
        def matches(when, h):
            if 'source' in when and when['source'].replace('<ROOT>', self.root) != source:
                return False
            if 'destination' in when and when['destination'].replace('<ROOT>', self.root) != destination:
                return False
            return source.endswith(when.get('source_endswith', ''))
        self.fire('rename', matches, 'before')
        self.real['cu_rename_exclusive'](source, destination)
        self.fire('rename', matches, 'after')

    def fake_rmtree(self, real):
        def rmtree(path, *a, **kw):
            fails = self.fake['rmtree_fails']
            if fails and os.path.basename(path).startswith(fails):
                self.rmtree_saw.append(os.path.exists(os.path.join(self.ns['cu_root'](), 'transaction.json')))
                raise OSError('simulated cleanup failure')
            return real(path, *a, **kw)
        return rmtree

    # --- 跑一次 client-updates ---
    def cmd(self, action='status', arg=''):
        """arg 可以是 Token（某一步打出来的某个客户端的安装凭据）。返回 (退出码, 状态文档或 None, stdout, stderr)"""
        index = len(self.steps)
        ref = None
        if isinstance(arg, Token):
            ref, arg = [arg.step, arg.client], arg.value
        s = self.step('cmd', action=action, arg=arg if ref is None else None, token=ref)
        self.calls, self.downloads, self.slept, self.rmtree_saw = [], [], [], []
        out, err, code, raised = io.StringIO(), io.StringIO(), 0, None
        real_rmtree = shutil.rmtree
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err), mock.patch('shutil.rmtree', self.fake_rmtree(real_rmtree)), \
                mock.patch.object(self.ns['time'], 'sleep', self.slept.append):
            try:
                self.ns['cmd_client_updates'](action, arg)
            except SystemExit as e:
                code = e.code
            except (PowerLoss, KeyboardInterrupt) as e:
                raised = type(e).__name__
        self.ns['CU_BUDGET'] = None
        stdout, stderr = out.getvalue(), err.getvalue()
        doc = None
        if stdout.startswith('{'):
            doc = json.loads(stdout)
            self.docs[index] = doc
        s['want'] = {'rc': code, 'raised': raised, 'stdout': self.norm(stdout), 'stderr': self.norm(stderr), 'tree': self.tree_delta(), 'calls': self.calls,
                     'downloads': self.downloads, 'slept': self.slept, 'rmtree_saw': self.rmtree_saw}
        self.last = (code, doc, stdout, stderr, raised, index)
        return code, doc, stdout, stderr

    def token(self, client, index=None):
        """某一步（默认上一个 cmd）打出来的这个客户端的安装凭据"""
        index = self.last[5] if index is None else index
        value = next(c for c in self.docs[index]['clients'] if c['id'] == client)['install_token']
        self.check(value and re.fullmatch(r'[0-9a-f]{64}', value), '没有安装凭据')
        return Token(index, client, value)

    def call(self, fn, **args):
        """直接调里面的一个函数（Rust 那边按 fn 的名字调对应的）。返回 Python 的结果"""
        s = self.step('call', fn=fn, **args)
        self.calls, self.slept = [], []
        result = {}
        with mock.patch.object(self.ns['time'], 'sleep', self.slept.append):
            got = outcome(lambda: CALLS[fn](self, result, **args))
        got.update(result)
        s['want'] = json.loads(self.norm(json.dumps(got)))
        s['want'].update(calls=self.calls, slept=self.slept, tree=self.tree_delta())
        return got


class Token:
    def __init__(self, step, client, value):
        self.step, self.client, self.value = step, client, value


def call_download(s, result, url, package, expected_size, empty, dest, stale=False):
    header = s.path('response.headers')
    if not stale and os.path.exists(header):
        os.unlink(header)   # 上一次留下的响应头：不清掉的话，这一次 curl 没写响应头时读到的是上一次的（stale=True 就是测这个）
    try:
        return s.real['cu_download'](url, s.path(dest), s.root, package, expected_size, tuple(empty))
    finally:
        # 报了错的也记：下了一半、大小不对的文件原来是留在那里的
        if os.path.exists(s.path(dest)):
            with open(s.path(dest), 'rb') as f:
                result['dest'] = [b64(f.read()), '%o' % (os.stat(s.path(dest)).st_mode & 0o777)]


def call_next_check(s, result, rows):
    v = s.ns['cu_next_check'](rows)
    now = time.time()
    # 有等着自动安装的：最多 15 分钟以后（现在的时间加 900 秒），记成和现在差多少
    return v if v is None or v < 1e9 else '<T+%d>' % (round((v - now) / 60) * 60)


def call_lock_twice(s, result, path):
    lock = s.ns['ClientUpdateLock']
    with lock(s.path(path)):
        with lock(s.path(path)):
            return 'both'


CALLS = {'cu_download': call_download, 'cu_next_check': call_next_check, 'lock_twice': call_lock_twice}


def row(doc, client):
    return next(c for c in doc['clients'] if c['id'] == client)


# --- 三类场景的开场（照 tests/client_updates.py 三个类的 setUp） -----------------------------------------------------------
CODEX_APP, CLAUDE_APP = 'Applications/ChatGPT.app', 'Applications/Claude.app'


def desktop_codex(name, defs, note=''):
    s = Scn(name, defs, {'CCA_TOOL': 'codex', 'CCA_CLIENT_CODEX_APP': '<ROOT>/' + CODEX_APP, 'CCA_CLIENT_CLAUDE_APP': '', 'CCA_CLIENT_VERIFIER': '<ROOT>/verifier',
                         'OPENAI_API_KEY': 'not-a-real-key', 'CODEX_HOME': '/do-not-read', 'HTTPS_PROXY': 'http://example.invalid', 'NO_PROXY': '*'}, note)
    s.app(CODEX_APP, 'codex', '1.0', '100')
    s.write('verifier', 'fake verifier')
    s.package = app_zip('codex', '2.0', '200')
    # 这一类里只有 ChatGPT：验签名的假命令一律报它的团队和产品标识（原来的测试也是；备份目录的名字里没有 ChatGPT.app）
    s.set_fake(pkg_codex=s.package, feed_codex=codex_feed(s.package), display='TeamIdentifier=%s\nIdentifier=%s\n' % (TEAM['codex'], BUNDLE['codex']))
    return s


def desktop_claude(name, defs, note=''):
    s = Scn(name, defs, {'CCA_CLIENT_CLAUDE_APP': '<ROOT>/' + CLAUDE_APP, 'CCA_CLIENT_CODEX_APP': '', 'CCA_CLIENT_VERIFIER': ''}, note)
    s.app(CLAUDE_APP, 'claude', '4.1.0', '4.1.0')
    s.package = app_zip('claude', '4.2.0', '4.2.0')
    s.set_fake(pkg_claude=s.package, feed_claude=claude_feed(s.package))
    return s


def add_chatgpt(s):
    s.set_env('CCA_CLIENT_CODEX_APP', '<ROOT>/' + CODEX_APP)
    s.set_env('CCA_CLIENT_VERIFIER', '<ROOT>/verifier')
    s.write('verifier', 'fake verifier')
    s.app(CODEX_APP, 'codex', '1.0', '100')
    package = app_zip('codex', '2.0', '200')
    s.set_fake(pkg_codex=package, feed_codex=codex_feed(package, deltas=False))


CLAUDE_BIN, CODEX_BIN = '.local/share/claude/versions/2.0.0', '.codex/packages/standalone/current/bin/codex'


def cli(name, defs, note=''):
    s = Scn(name, defs, {'CCA_TOOL': 'claude', 'PATH': '/usr/bin:/bin', 'CCA_CLIENT_CLAUDE_APP': '', 'CCA_CLIENT_CODEX_APP': '', 'CCA_CLI_CLAUDE': '<ROOT>/' + CLAUDE_BIN,
                         'CCA_CLI_CODEX': '', 'ANTHROPIC_API_KEY': 'not-a-real-key', 'OPENAI_API_KEY': 'not-a-real-key', 'HTTPS_PROXY': 'http://example.invalid',
                         'CLAUDE_CONFIG_DIR': '/do-not-read', 'CODEX_HOME': '/do-not-read'}, note)
    s.write(CLAUDE_BIN, 'fake')
    s.write(CODEX_BIN, 'fake')
    return s


def stage(s, client='codex'):
    code, doc, out, err = s.cmd('check', 'force')
    s.check(code == 0, err + out)
    view = row(doc, client)
    s.check(view['status'] == 'staged', out)
    s.check(view['can_install'])
    return s.token(client)


def build_of(s, rel):
    with open(s.path(rel + '/Contents/Info.plist'), 'rb') as f:
        return plistlib.load(f)['CFBundleVersion']


def exists(s, rel):
    return os.path.lexists(s.path(rel))


def leftovers(s):
    return [n for n in os.listdir(s.path('Applications')) if n.startswith('.cca-client-install-')]


def config(s):
    p = s.path('.cca/config.json')
    if not os.path.exists(p):
        return {}
    with open(p) as f:
        return json.load(f)


def updates(s, all_calls):
    return [c for c in all_calls if c['args'][-1] == 'update']


# --- 场景 ------------------------------------------------------------------------------------------------------------
def scenarios(defs):
    done = []
    os.umask(0o022)

    def add(s):
        done.append(s.close())

    # ===== ClientUpdates：ChatGPT（Sparkle 的 appcast、Ed25519 验签） =====
    s = desktop_codex('codex/status-is-read-only-and-never-hashes', defs)
    s.forbid('hash')
    code, doc, _, _ = s.cmd()
    s.check(code == 0 and row(doc, 'codex')['installed_version'] == '1.0')
    s.check(not exists(s, '.cca') and s.calls == [])
    add(s)

    s = desktop_codex('codex/check-stages-but-does-not-install-and-throttles', defs)
    token = stage(s)
    s.check(re.fullmatch(r'[0-9a-f]{64}', token.value) and build_of(s, CODEX_APP) == '100')
    n = len(s.raw_downloads)
    s.forbid('hash')
    s.check(s.cmd('check')[0] == 0)
    s.forbid('hash', False)
    s.check(len(s.raw_downloads) == 2 and n == 2)
    s.check(s.cmd('check', 'force')[0] == 0)
    s.check(len(s.raw_downloads) == 3, 'same package must not be downloaded twice')
    add(s)

    s = desktop_codex('codex/no-verifier-blocks-before-big-download', defs)
    s.set_env('CCA_CLIENT_VERIFIER', '')
    code, doc, _, _ = s.cmd('check', 'force')
    s.check(code == 1 and row(doc, 'codex')['status'] == 'blocked' and len(s.raw_downloads) == 1)
    add(s)

    s = desktop_codex('codex/bad-current-signature-is-not-replaced', defs)
    s.set_fake(failed=['/usr/bin/codesign'])
    s.check(s.cmd('check', 'force')[0] == 1 and s.raw_downloads == [])
    add(s)

    s = desktop_codex('codex/spctl-rejects', defs)
    s.set_fake(failed=['/usr/sbin/spctl'])
    s.check(s.cmd('check', 'force')[0] == 1 and s.raw_downloads == [])
    add(s)

    s = desktop_codex('codex/lipo-fails', defs)
    s.set_fake(failed=['/usr/bin/lipo'])
    s.check(s.cmd('check', 'force')[0] == 1 and s.raw_downloads == [])
    add(s)

    s = desktop_codex('codex/wrong-architecture', defs)
    s.set_fake(archs=b'x86_64\n')
    s.check(s.cmd('check', 'force')[0] == 1 and s.raw_downloads == [])
    add(s)

    s = desktop_codex('codex/intel-host', defs, 'x64 的机器：更新源里只有 arm64 的包')
    s.set_fake(host=['x64', '26.0'], archs=b'x86_64 arm64\n')
    code, doc, _, _ = s.cmd('check', 'force')
    s.check(code == 1 and row(doc, 'codex')['status'] == 'unsupported')
    add(s)

    s = desktop_codex('codex/old-system', defs, 'App 要的系统比这台机器新')
    s.set_fake(host=['arm64', '12.6'])
    code, doc, _, _ = s.cmd('check', 'force')
    s.check(code == 1 and row(doc, 'codex')['status'] == 'unsupported' and s.raw_downloads == [])
    add(s)

    s = desktop_codex('codex/bad-archive-signature-is-not-extracted', defs)
    s.set_fake(failed=['verifier'])
    s.forbid('extract')
    code, doc, _, _ = s.cmd('check', 'force')
    s.check(code == 1 and not row(doc, 'codex')['can_install'])
    add(s)

    s = desktop_codex('codex/archive-change-during-signature-is-rejected', defs)
    s.hook('verify', overwrite=True)
    s.check(s.cmd('check', 'force')[0] == 1)
    add(s)

    s = desktop_codex('codex/no-equal-build-or-downgrade-download', defs)
    for build in ('100', '99'):
        s.set_fake(feed_codex=codex_feed(s.package, build=build))
        code, doc, _, _ = s.cmd('check', 'force')
        s.check(code == 0 and row(doc, 'codex')['status'] == 'current')
    s.check(len(s.raw_downloads) == 2)
    add(s)

    s = desktop_codex('codex/target-change-invalidates-plan-and-install', defs)
    token = stage(s)
    s.app(CODEX_APP, 'codex', '3.0', '300')
    s.check(s.cmd('plan', token)[0] == 1)
    s.check(s.cmd('install', token)[0] == 1)
    s.check(build_of(s, CODEX_APP) == '300')
    add(s)

    s = desktop_codex('codex/cached-archive-change-hides-install', defs)
    stage(s)
    s.write('.cca/client-updates/codex/package.zip', b'changed')
    view = row(s.cmd()[1], 'codex')
    s.check(not view['can_install'] and view['status'] == 'needs_recheck')
    add(s)

    s = desktop_codex('codex/mode-off-still-works-with-broken-cache', defs)
    s.mkdir('.cca')
    s.mkdir('.cca/client-updates')
    s.mkdir('.cca/client-updates/codex')
    s.write('.cca/client-updates/codex/state.json', '{bad', 0o600)
    s.check(s.cmd('mode', 'off')[0] == 0 and config(s)['client_updates'] == 'off')
    add(s)

    s = desktop_codex('codex/install-waits-for-instance-and-updater', defs)
    token = stage(s)
    app = s.path(CODEX_APP)
    for command in (app + '/Contents/MacOS/ChatGPT', '/tmp/Updater.app/Contents/MacOS/Updater ' + app, '/tmp/com.openai.codex/org.sparkle-project.Sparkle/Updater.app/helper'):
        s.set_fake(processes=('88 ' + command + '\n').encode())
        s.check(s.cmd('install', token)[0] == 1 and build_of(s, CODEX_APP) == '100')
    s.set_fake(processes=b'')
    s.check(s.cmd('install', token)[0] == 1)
    add(s)

    s = desktop_codex('codex/successful-install-and-no-accumulating-backups', defs)
    token = stage(s)
    code, doc, out, err = s.cmd('install', token)
    s.check(code == 0, err + out)
    s.check(build_of(s, CODEX_APP) == '200' and row(doc, 'codex')['status'] == 'updated' and leftovers(s) == [])
    s.check(s.cmd('install', token)[0] == 1)
    add(s)

    s = desktop_codex('codex/native-update-just-before-move-is-restored', defs)
    token = stage(s)
    s.hook('rename', {'source': '<ROOT>/' + CODEX_APP}, once=True, before=s.app_steps(CODEX_APP, 'codex', '3.0', '300'))
    s.check(s.cmd('install', token)[0] == 1)
    s.check(build_of(s, CODEX_APP) == '300' and not exists(s, '.cca/client-updates/codex/transaction.json'))
    add(s)

    s = desktop_codex('codex/new-target-during-commit-is-preserved', defs)
    token = stage(s)
    s.hook('rename', {'destination': '<ROOT>/' + CODEX_APP, 'source_endswith': '/ChatGPT.app'}, before=s.app_steps(CODEX_APP, 'codex', '3.0', '300'))
    s.check(s.cmd('install', token)[0] == 1)
    s.check(build_of(s, CODEX_APP) == '300' and not exists(s, '.cca/client-updates/codex/transaction.json'))
    s.check(s.cmd('check', 'force')[0] == 0)
    add(s)

    s = desktop_codex('codex/interrupted-old-move-recovers-on-check', defs)
    token = stage(s)
    s.hook('rename', {'source': '<ROOT>/' + CODEX_APP}, raises='PowerLoss')
    s.cmd('install', token)
    s.check(s.last[4] == 'PowerLoss' and not exists(s, CODEX_APP))
    s.check(row(s.cmd()[1], 'codex')['status'] == 'needs_recheck')
    s.unhook()
    s.check(s.cmd('check')[0] == 0)
    s.check(build_of(s, CODEX_APP) == '100' and not exists(s, '.cca/client-updates/codex/transaction.json'))
    add(s)

    s = desktop_codex('codex/cleanup-failure-does-not-fail-committed-install', defs)
    token = stage(s)
    s.set_fake(rmtree_fails='.cca-client-install-')
    code, doc, out, _ = s.cmd('install', token)
    s.check(code == 0 and s.rmtree_saw == [False], out)
    s.check(build_of(s, CODEX_APP) == '200')
    add(s)

    s = desktop_codex('codex/locks-are-nonblocking', defs)
    stage(s)
    for lock in ('.cca/client-updates/codex/install.lock', '.cca/client-updates/manager.lock'):
        got = s.call('lock_twice', path=lock)
        s.check(got.get('cu', [None])[0] == 'busy', 'lock was not exclusive')
    add(s)

    s = desktop_codex('codex/updater-state-directory-symlink-is-rejected', defs)
    s.mkdir('.cca')
    s.mkdir('unrelated')
    s.symlink('.cca/client-updates', '<ROOT>/unrelated')
    s.check(s.cmd('check', 'force')[0] == 1 and os.listdir(s.path('unrelated')) == [])
    add(s)

    s = desktop_codex('codex/recovery-cleanup-failure-after-rollback', defs)
    token = stage(s)
    s.hook('rename', {'source': '<ROOT>/' + CODEX_APP}, raises='PowerLoss')
    s.cmd('install', token)
    s.check(s.last[4] == 'PowerLoss')
    s.unhook()
    s.set_fake(rmtree_fails='.cca-client-install-')
    s.check(s.cmd('check')[0] == 0 and s.rmtree_saw == [False])
    s.check(build_of(s, CODEX_APP) == '100' and not exists(s, '.cca/client-updates/codex/transaction.json'))
    add(s)

    s = desktop_codex('codex/interrupted-after-the-swap-is-finished-on-check', defs, '新程序到位以后、状态存下来之前断了：下次检查认出来，算装好了')
    token = stage(s)
    s.hook('rename', {'destination': '<ROOT>/' + CODEX_APP}, raises='PowerLoss')
    s.cmd('install', token)
    s.check(s.last[4] == 'PowerLoss' and build_of(s, CODEX_APP) == '200' and exists(s, '.cca/client-updates/codex/transaction.json'))
    s.unhook()
    code, doc, _, _ = s.cmd('check')
    s.check(code == 0 and not exists(s, '.cca/client-updates/codex/transaction.json') and leftovers(s) == [])
    add(s)

    s = desktop_codex('codex/auto-install-off-by-default-and-follows-checks', defs)
    code, doc, _, _ = s.cmd()
    s.check(code == 0 and doc['auto_install'] is False)
    s.check(s.cmd('mode', 'off')[0] == 0)
    code, doc, _, _ = s.cmd('auto-install', 'on')
    s.check(code == 0 and doc['auto_install'] is True and (config(s)['client_updates'], config(s)['client_auto_install']) == ('auto', True))
    code, doc, _, _ = s.cmd('mode', 'off')
    s.check(code == 0 and doc['auto_install'] is False and 'client_auto_install' not in config(s))
    s.check(s.cmd('auto-install', 'on')[0] == 0)
    code, doc, _, _ = s.cmd('auto-install', 'off')
    s.check(code == 0 and (doc['mode'], doc['auto_install']) == ('auto', False) and 'client_auto_install' not in config(s))
    s.check(s.cmd('auto-install', 'maybe')[0] == 1)
    add(s)

    s = desktop_codex('codex/allowed-auto-install-installs-right-after-check', defs)
    s.check(s.cmd('auto-install', 'on')[0] == 0)
    code, doc, out, err = s.cmd('check', 'force')
    s.check(code == 0, err + out)
    r = row(doc, 'codex')
    s.check((r['status'], r['can_install'], r['auto_install_pending']) == ('updated', False, False) and '已自动安装官方更新 2.0' in r['message'])
    s.check(build_of(s, CODEX_APP) == '200' and leftovers(s) == [])
    add(s)

    s = desktop_codex('codex/turning-it-on-installs-staged-on-next-check-not-on-status', defs)
    stage(s)
    s.check(s.cmd('auto-install', 'on')[0] == 0)
    code, doc, _, _ = s.cmd()
    s.check(code == 0 and build_of(s, CODEX_APP) == '100' and row(doc, 'codex')['status'] == 'staged')
    code, doc, out, err = s.cmd('check')
    s.check(code == 0 and row(doc, 'codex')['status'] == 'updated' and build_of(s, CODEX_APP) == '200', err + out)
    s.check(len(s.raw_downloads) == 2)
    add(s)

    s = desktop_codex('codex/auto-install-waits-for-open-client-then-installs', defs)
    s.check(s.cmd('auto-install', 'on')[0] == 0)
    s.set_fake(processes=('88 ' + s.path(CODEX_APP) + '/Contents/MacOS/ChatGPT\n').encode())
    code, doc, out, err = s.cmd('check', 'force')
    s.check(code == 0, err + out)
    r = row(doc, 'codex')
    s.check((r['status'], r['can_install'], r['auto_install_pending']) == ('staged', True, True) and '退出后自动安装' in r['message'])
    s.check(build_of(s, CODEX_APP) == '100' and doc['next_check_epoch'] <= time.time() + 900 + 5)
    code, doc, _, _ = s.cmd('auto-install', 'off')
    r = row(doc, 'codex')
    s.check(not r['auto_install_pending'] and '退出后自动安装' not in r['message'] and doc['next_check_epoch'] > time.time() + 3600)
    s.check(s.cmd('auto-install', 'on')[0] == 0)
    s.set_fake(processes=LAUNCHD)
    code, doc, out, err = s.cmd('check')
    s.check(code == 0, err + out)
    r = row(doc, 'codex')
    s.check((r['status'], r['auto_install_pending']) == ('updated', False) and build_of(s, CODEX_APP) == '200')
    with open(s.path('.cca/client-updates/codex/state.json')) as f:
        s.check('auto_wait' not in json.load(f))
    add(s)

    s = desktop_codex('codex/turned-off-during-check-does-not-install', defs)
    s.check(s.cmd('auto-install', 'on')[0] == 0)
    s.hook('download', {'package': True}, after=[{'op': 'json_edit', 'path': '.cca/config.json', 'set': {}, 'pop': ['client_auto_install']}])
    code, doc, out, err = s.cmd('check', 'force')
    s.check(code == 0, err + out)
    r = row(doc, 'codex')
    s.check((r['status'], r['can_install'], r['auto_install_pending']) == ('staged', True, False) and build_of(s, CODEX_APP) == '100')
    add(s)

    s = desktop_codex('codex/native-update-while-waiting-is-not-overwritten', defs)
    s.check(s.cmd('auto-install', 'on')[0] == 0)
    s.set_fake(processes=('88 ' + s.path(CODEX_APP) + '/Contents/MacOS/ChatGPT\n').encode())
    s.check(s.cmd('check', 'force')[0] == 0)
    s.app(CODEX_APP, 'codex', '3.0', '300')
    s.set_fake(processes=LAUNCHD)
    code, doc, _, _ = s.cmd('check')
    s.check(code == 1 and build_of(s, CODEX_APP) == '300' and not row(doc, 'codex')['auto_install_pending'])
    with open(s.path('.cca/client-updates/codex/state.json')) as f:
        s.check('auto_wait' not in json.load(f))
    add(s)

    for name, feed, status in [('feed-not-xml', b'hello', 'error'), ('feed-doctype', b'<!DOCTYPE rss><rss/>', 'blocked'), ('feed-wrong-root', b'<feed/>', 'blocked'),
                               ('feed-no-items', b'<rss><channel/></rss>', 'unsupported'), ('feed-other-arch-only', None, 'unsupported')]:
        s = desktop_codex('codex/' + name, defs)
        s.set_fake(feed_codex=feed if feed is not None else codex_feed(s.package, arch='x64'))
        code, doc, _, _ = s.cmd('check', 'force')
        s.check(code == 1 and row(doc, 'codex')['status'] == status, name)
        add(s)

    s = desktop_codex('codex/package-of-another-version', defs, '包里的 App 版本和列表里写的不一样')
    s.set_fake(pkg_codex=app_zip('codex', '2.1', '201'))
    s.set_fake(feed_codex=codex_feed(app_zip('codex', '2.1', '201')))
    code, doc, _, _ = s.cmd('check', 'force')
    s.check(code == 1 and row(doc, 'codex')['status'] == 'blocked')
    add(s)

    s = desktop_codex('codex/restage-after-target-touched', defs, '暂存以后 App 被动过（版本没变）：下次检查不重新下载，换一个安装凭据')
    first = stage(s)
    s.app(CODEX_APP, 'codex', '1.0', '100')
    second = stage(s)
    s.check(first.value != second.value and len(s.raw_downloads) == 3)
    s.check(s.cmd('install', first)[0] == 1)
    s.check(s.cmd('install', second)[0] == 0 and build_of(s, CODEX_APP) == '200')
    add(s)

    # ===== ClaudeClientUpdates：Claude（Squirrel 的 JSON 更新源、SHA-256），和 ChatGPT 并排 =====
    s = desktop_claude('claude/throwaway-device-id-and-no-gui-verifier', defs)
    s.check(s.cmd('check', 'force')[0] == 0)
    s.check(s.cmd('check', 'force')[0] == 0)
    feeds = [u for u in s.raw_downloads if 'squirrel/update' in u]
    ids = [re.search(r'device_id=([0-9a-f-]{36})&', u).group(1) for u in feeds]
    s.check(len(feeds) == 2 and ids[0] != ids[1], 'device_id must not be reused between checks')
    s.check(all(u.startswith('https://api.anthropic.com/api/desktop/darwin/arm64/squirrel/update?') for u in feeds))
    with open(s.path('.cca/client-updates/claude/state.json')) as f:
        state = f.read()
    s.check(ids[0] not in state and ids[1] not in state)
    s.check(sum('downloads.claude.ai' in u for u in s.raw_downloads) == 1, 'same package must not be downloaded twice')
    add(s)

    s = desktop_claude('claude/204-means-nothing-to-install', defs)
    s.set_fake(feed_code='204')
    code, doc, _, err = s.cmd('check', 'force')
    s.check(code == 0 and row(doc, 'claude')['status'] == 'current' and not any('downloads.claude.ai' in u for u in s.raw_downloads), err)
    add(s)

    s = desktop_claude('claude/arch-feed-missing-falls-back-to-universal', defs)
    s.set_fake(feed_codes=['404'], feed_claude=claude_feed(s.package, url='https://downloads.claude.ai/releases/darwin/universal/4.2.0/Claude-%s.zip' % ('b' * 40)))
    s.check(s.cmd('check', 'force')[0] == 0)
    s.check([u.split('/squirrel')[0].rsplit('/', 1)[1] for u in s.raw_downloads if 'squirrel/update' in u] == ['arm64', 'universal'])
    add(s)

    s = desktop_claude('claude/no-feed-for-this-mac', defs, '两种架构的更新源都说没有')
    s.set_fake(feed_codes=['404', '404'])
    code, doc, _, _ = s.cmd('check', 'force')
    s.check(code == 1 and row(doc, 'claude')['status'] == 'unsupported')
    add(s)

    s = desktop_claude('claude/same-or-older-release-is-not-downloaded', defs)
    for version in ('4.1.0', '4.0.9'):
        s.set_fake(feed_claude=claude_feed(s.package, version=version))
        code, doc, out, _ = s.cmd('check', 'force')
        s.check(code == 0 and row(doc, 'claude')['status'] == 'current', out)
    s.check(not any('downloads.claude.ai' in u for u in s.raw_downloads))
    add(s)

    s = desktop_claude('claude/build-is-compared-as-version-not-text', defs)
    s.app(CLAUDE_APP, 'claude', '4.9.0', '4.9.0')
    s.package = app_zip('claude', '4.23118.0', '4.23118.0')
    s.set_fake(pkg_claude=s.package, feed_claude=claude_feed(s.package, version='4.23118.0'))
    stage(s, 'claude')
    add(s)

    s = desktop_claude('claude/wrong-sha256-is-blocked-before-extracting', defs)
    s.set_fake(feed_claude=claude_feed(s.package, digest='0' * 64))
    s.forbid('extract')
    code, doc, _, _ = s.cmd('check', 'force')
    view = row(doc, 'claude')
    s.check(code == 1 and view['status'] == 'blocked' and not view['can_install'])
    add(s)

    s = desktop_claude('claude/wrong-team-or-bundle-is-blocked', defs)
    s.set_fake(display='TeamIdentifier=AAAAAAAAAA\nIdentifier=com.anthropic.claudefordesktop\n')
    code, doc, _, _ = s.cmd('check', 'force')
    s.check(code == 1 and row(doc, 'claude')['status'] == 'blocked' and s.raw_downloads == [], 'must not contact the feed for an unverifiable client')
    add(s)

    s = desktop_claude('claude/install-replaces-claude-and-labels-the-consent', defs)
    token = stage(s, 'claude')
    code, _, out, _ = s.cmd('plan', token)
    s.check(code == 0 and 'CU_LABEL=Claude' in out and 'CU_VERSION=4.2.0' in out)
    code, doc, out, err = s.cmd('install', token)
    s.check(code == 0, err + out)
    s.check(build_of(s, CLAUDE_APP) == '4.2.0' and row(doc, 'claude')['status'] == 'updated' and leftovers(s) == [])
    add(s)

    s = desktop_claude('claude/package-is-not-kept-without-a-pending-update', defs)
    package = '.cca/client-updates/claude/package.zip'
    stage(s, 'claude')
    s.check(exists(s, package))
    s.cmd()
    token = s.token('claude')
    s.check(s.cmd('install', token)[0] == 0)
    s.check(not exists(s, package), 'installed: the staged zip must not linger')
    s.app(CLAUDE_APP, 'claude', '4.1.0', '4.1.0')
    stage(s, 'claude')
    s.check(exists(s, package))
    s.set_fake(feed_code='204')
    s.check(s.cmd('check', 'force')[0] == 0)
    s.check(not exists(s, package), 'nothing to install: the staged zip must not linger')
    add(s)

    s = desktop_claude('claude/any-process-from-the-bundle-blocks-install', defs)
    token = stage(s, 'claude')
    app = s.path(CLAUDE_APP)
    for command in (app + '/Contents/MacOS/Claude --user-data-dir=/x', app + '/Contents/Frameworks/Claude Helper.app/Contents/MacOS/Claude Helper --type=gpu',
                    app + '/Contents/Frameworks/Squirrel.framework/Resources/ShipIt com.anthropic.claudefordesktop.ShipIt /x'):
        s.set_fake(processes=('88 ' + command + '\n').encode())
        code, _, out, err = s.cmd('install', token)
        s.check(code == 1 and 'Claude' in err and build_of(s, CLAUDE_APP) == '4.1.0', out)
    s.set_fake(processes=LAUNCHD)
    s.check(s.cmd('install', token)[0] == 0)
    add(s)

    s = desktop_claude('claude/updater-process-elsewhere-blocks-install', defs, '更新器在别处跑着（看可执行文件的名字，不看参数）')
    token = stage(s, 'claude')
    s.set_fake(processes=b'77 /x/ShipIt y\n', comm=b'77 /Users/me/Library/Caches/com.anthropic.claudefordesktop.ShipIt/ShipIt\n')
    s.check(s.cmd('install', token)[0] == 1)
    s.set_fake(comm=b'77 /usr/bin/python3\n', processes=b'77 /usr/bin/python3 -c com.anthropic.claudefordesktop ShipIt Squirrel\n')
    s.check(s.cmd('install', token)[0] == 0)
    add(s)

    s = desktop_claude('claude/native-update-before-install-stops-it', defs)
    token = stage(s, 'claude')
    s.app(CLAUDE_APP, 'claude', '4.3.0', '4.3.0')
    s.check(s.cmd('install', token)[0] == 1 and build_of(s, CLAUDE_APP) == '4.3.0')
    add(s)

    s = desktop_claude('claude/not-installed-is-not-a-failure', defs)
    s.set_env('CCA_CLIENT_CLAUDE_APP', '')
    code, doc, out, err = s.cmd('check', 'force')
    s.check(code == 0 and row(doc, 'claude')['status'] == 'missing' and s.raw_downloads == [], err + out)
    s.check(not exists(s, '.cca/client-updates/claude') and not exists(s, '.cca/client-updates/codex'))
    add(s)

    s = desktop_claude('claude/both-clients-listed-claude-first-separate-tokens', defs)
    add_chatgpt(s)
    code, doc, out, err = s.cmd('check', 'force')
    s.check(code == 0, err + out)
    s.check([c['id'] for c in doc['clients']] == ['claude', 'codex', 'claude-cli', 'codex-cli'] and [c['status'] for c in doc['clients'][2:]] == ['missing', 'missing'])
    claude, codex = doc['clients'][:2]
    s.check(claude['can_install'] and codex['can_install'] and claude['install_token'] != codex['install_token'])
    code, doc, out, err = s.cmd('install', s.token('codex'))
    s.check(code == 0, err + out)
    s.check(build_of(s, CODEX_APP) == '200' and build_of(s, CLAUDE_APP) == '4.1.0', 'installing ChatGPT must not touch Claude')
    s.check(row(doc, 'claude')['status'] == 'staged' and row(doc, 'claude')['can_install'])
    add(s)

    s = desktop_claude('claude/one-client-failing-does-not-stop-the-other', defs)
    add_chatgpt(s)
    s.set_fake(feed_claude=claude_feed(s.package, digest='0' * 64))
    code, doc, out, err = s.cmd('check', 'force')
    codex = row(doc, 'codex')
    s.check(code == 1 and row(doc, 'claude')['status'] == 'blocked' and codex['status'] == 'staged' and codex['can_install'] and '校验值' in err, out)
    add(s)

    s = desktop_claude('claude/next-check-time-tells-the-app-how-long-to-sleep', defs)
    rows = [{'status': 'current', 'checked_epoch': 1000.0}, {'status': 'current', 'checked_epoch': 500.0}, {'status': 'missing', 'checked_epoch': None}]
    s.check(s.call('cu_next_check', rows=rows)['ok'] == 500.0 + 6 * 3600)
    rows = [{'status': 'current', 'checked_epoch': None}, {'status': 'current', 'checked_epoch': 500.0}, {'status': 'missing', 'checked_epoch': None}]
    s.check(s.call('cu_next_check', rows=rows)['ok'] == 6 * 3600)   # 没查过的：已经到时间了
    s.check(s.call('cu_next_check', rows=[{'status': 'missing'}])['ok'] is None)
    for rows in ([{'status': 'current', 'checked_epoch': 1}, {'status': 'current', 'checked_epoch': 1.0}], [{'status': 'current', 'checked_epoch': 1.0}, {'status': 'current', 'checked_epoch': 1}],
                 [{'status': 'current', 'checked_epoch': True}], [{'status': 'x', 'checked_epoch': 7, 'auto_install_pending': True}], [{'checked_epoch': 'soon'}],
                 [{'checked_epoch': 5}, {'checked_epoch': 'soon'}], [{'checked_epoch': [1]}], [], [{'status': None, 'checked_epoch': 2.5}],
                 [{'status': 'current', 'checked_epoch': 4000000000.5, 'auto_install_pending': 1}], [{'checked_epoch': 0.0}, {'checked_epoch': 0}], [{'checked_epoch': -3}]):
        s.call('cu_next_check', rows=rows)
    s.check(s.cmd('mode', 'off')[0] == 0)
    s.check(s.call('cu_next_check', rows=[{'status': 'current', 'checked_epoch': 1.0}])['ok'] is None)
    add(s)

    s = desktop_claude('claude/check-time-is-saved-before-the-download', defs)
    s.set_fake(download_raises='KeyboardInterrupt')
    s.cmd('check', 'force')
    s.check(s.last[4] == 'KeyboardInterrupt')
    with open(s.path('.cca/client-updates/claude/state.json')) as f:
        s.check(json.load(f)['checked_epoch'] > 0)
    add(s)

    for name, feed, status in [('feed-not-json', b'not json', 'error'), ('feed-list', b'[]', 'blocked'), ('feed-listed-other-version', None, 'blocked')]:
        s = desktop_claude('claude/' + name, defs)
        s.set_fake(feed_claude=feed if feed is not None else claude_feed(s.package, listed='9.9.9'))
        code, doc, _, _ = s.cmd('check', 'force')
        s.check(code == 1 and row(doc, 'claude')['status'] == status, name)
        add(s)

    s = desktop_claude('claude/auto-install', defs, '允许自动更新：Claude 检查完马上装')
    s.check(s.cmd('auto-install', 'on')[0] == 0)
    code, doc, out, err = s.cmd('check', 'force')
    s.check(code == 0 and row(doc, 'claude')['status'] == 'updated' and build_of(s, CLAUDE_APP) == '4.2.0', err + out)
    s.check(not exists(s, '.cca/client-updates/claude/package.zip'))
    add(s)

    # ===== CliToolUpdates：终端里的 Claude Code、Codex =====
    s = cli('cli/status-is-read-only-and-lists-the-terminal-tools', defs)
    code, doc, _, _ = s.cmd()
    r = row(doc, 'claude-cli')
    s.check(code == 0 and (r['kind'], r['status'], r['installed_version'], r['path']) == ('cli', 'unchecked', '2.0.0', s.path(CLAUDE_BIN)))
    s.check(row(doc, 'codex-cli')['status'] == 'missing' and s.raw_downloads == [] and not exists(s, '.cca'))
    add(s)

    def check(s, client='claude-cli', arg='force'):
        code, doc, out, err = s.cmd('check', arg)
        s.check(code == 0, err + out)
        return row(doc, client)

    s = cli('cli/check-finds-a-new-version-but-never-installs', defs)
    r = check(s)
    s.check((r['status'], r['available_version'], r['can_install']) == ('available', '2.1.0', True) and r['install_token'])
    s.check(updates(s, s.calls) == [] and s.raw_downloads == ['https://downloads.claude.ai/claude-code-releases/latest'])
    add(s)

    s = cli('cli/same-or-newer-is-current', defs)
    for latest in ('2.0.0', '1.9.9'):
        s.set_fake(cli_latest=latest)
        r = check(s)
        s.check((r['status'], r['can_install'], r['install_token']) == ('current', False, None))
    add(s)

    s = cli('cli/background-check-is-throttled-and-mode-off-stops-it', defs)
    check(s)
    n = len(s.raw_downloads)
    s.check(s.cmd('check')[0] == 0 and len(s.raw_downloads) == n)
    s.json_edit('.cca/client-updates/claude-cli/state.json', set={'checked_epoch': 1})
    s.check(s.cmd('mode', 'off')[0] == 0)
    s.check(s.cmd('check')[0] == 0 and len(s.raw_downloads) == n)
    add(s)

    s = cli('cli/tool-that-updated-itself-is-current-without-a-stale-button', defs)
    check(s)
    s.set_fake(cli_installed='2.1.0')
    r = row(s.cmd()[1], 'claude-cli')
    s.check((r['status'], r['can_install'], r['install_token']) == ('current', False, None))
    add(s)

    s = cli('cli/install-runs-the-official-updater-in-a-clean-environment', defs)
    check(s)
    code, doc, out, err = s.cmd('install', s.token('claude-cli'))
    s.check(code == 0, err + out)
    r = row(doc, 'claude-cli')
    s.check((r['status'], r['installed_version'], r['can_install']) == ('updated', '2.1.0', False))
    up = updates(s, s.calls)[0]
    env = dict(up['env'])
    s.check(up['args'] == ['<ROOT>/' + CLAUDE_BIN, 'update'] and env['HOME'] == '<ROOT>' and env['PATH'].startswith('<ROOT>/.local/bin:'))
    s.check(env['CLAUDE_CONFIG_DIR'] not in ('/do-not-read', '<ROOT>/.claude') and (env['DISABLE_TELEMETRY'], env['DISABLE_ERROR_REPORTING']) == ('1', '1'))
    s.check(not any(k in env for k in ('ANTHROPIC_API_KEY', 'OPENAI_API_KEY', 'HTTPS_PROXY', 'CODEX_HOME')))
    add(s)

    s = cli('cli/codex-uses-github-and-its-own-updater', defs)
    s.set_env('CCA_CLI_CLAUDE', '')
    s.set_env('CCA_CLI_CODEX', '<ROOT>/' + CODEX_BIN)
    s.set_fake(cli_installed='0.166.0', cli_latest='0.167.0', cli_body=json.dumps({'tag_name': 'rust-v0.167.0'}).encode())
    r = check(s, 'codex-cli')
    s.check((r['status'], r['available_version'], r['can_install']) == ('available', '0.167.0', True))
    s.check(s.raw_downloads == ['https://api.github.com/repos/openai/codex/releases/latest'])
    code, doc, out, err = s.cmd('install', s.token('codex-cli'))
    s.check(code == 0, err + out)
    env = dict(updates(s, s.calls)[0]['env'])
    s.check(env['CODEX_NON_INTERACTIVE'] == '1' and 'CODEX_HOME' not in env)
    add(s)

    s = cli('cli/failed-update-keeps-the-button-and-says-why', defs)
    s.set_fake(cli_update_works=False)
    check(s)
    code, doc, _, err = s.cmd('install', s.token('claude-cli'))
    r = row(doc, 'claude-cli')
    s.check(code == 1 and 'network is down' in err and r['status'] == 'error' and not r['can_install'])
    again = row(s.cmd()[1], 'claude-cli')
    s.check((again['status'], again['can_install']) == ('available', True))
    add(s)

    s = cli('cli/allowed-auto-install-runs-the-updater-after-the-check', defs)
    s.check(s.cmd('auto-install', 'on')[0] == 0)
    r = check(s)
    s.check((r['status'], r['installed_version'], r['can_install']) == ('updated', '2.1.0', False) and '已自动更新到 2.1.0' in r['message'])
    s.check([c['args'] for c in updates(s, s.calls)] == [['<ROOT>/' + CLAUDE_BIN, 'update']])
    n = len(s.raw_downloads)
    s.check(s.cmd('check')[0] == 0 and updates(s, s.calls) == [] and len(s.raw_downloads) == n)   # 装好了：下次检查前不再跑
    add(s)

    # 官方的 update 装上的比检查时查到的还新（检查以后又出了一版）：自动更新以后那句话里写的还是检查时查到的那个版本
    s = cli('cli/auto-install-says-the-version-it-found-not-the-one-installed', defs)
    s.check(s.cmd('auto-install', 'on')[0] == 0)
    s.hook('run', {'last_arg': 'update'}, before=[{'op': 'fake', 'key': 'cli_latest', 'value': '2.2.0'}])
    r = check(s)
    s.check((r['status'], r['installed_version']) == ('updated', '2.2.0') and '已自动更新到 2.1.0' in r['message'], r)
    add(s)

    s = cli('cli/turned-off-right-before-the-update-does-not-run-it', defs)
    s.check(s.cmd('auto-install', 'on')[0] == 0)
    s.hook('run', {'last_arg': '--version', 'nth': 2}, before=[{'op': 'json_edit', 'path': '.cca/config.json', 'set': {}, 'pop': ['client_auto_install']}])
    code, doc, out, err = s.cmd('check', 'force')
    s.check(code == 0, err + out)
    s.check(sum(c['args'][-1] == '--version' for c in s.calls) >= 2 and updates(s, s.calls) == [])
    s.check((row(doc, 'claude-cli')['status'], row(doc, 'claude-cli')['can_install']) == ('available', True))
    add(s)

    s = cli('cli/auto-install-leaves-npm-to-the-user', defs)
    s.check(s.cmd('auto-install', 'on')[0] == 0)
    s.write('lib/node_modules/@anthropic-ai/claude-code/cli.js', 'fake')
    s.set_env('CCA_CLI_CLAUDE', '<ROOT>/lib/node_modules/@anthropic-ai/claude-code/cli.js')
    s.set_fake(cli_body=json.dumps({'version': '2.1.0'}).encode())
    r = check(s)
    s.check((r['status'], r['can_install']) == ('available', False) and updates(s, s.calls) == [])
    add(s)

    s = cli('cli/auto-install-skips-a-tool-that-updated-itself-meanwhile', defs)
    s.check(s.cmd('auto-install', 'on')[0] == 0)
    s.hook('run', {'last_arg': '--version', 'nth': 2}, before=[{'op': 'fake', 'key': 'cli_installed', 'value': '2.1.0'}])
    code, doc, out, err = s.cmd('check', 'force')
    s.check(code == 0 and updates(s, s.calls) == [] and row(doc, 'claude-cli')['status'] == 'current', err + out)
    add(s)

    s = cli('cli/failed-auto-install-says-why-and-keeps-the-button', defs)
    s.check(s.cmd('auto-install', 'on')[0] == 0)
    s.set_fake(cli_update_works=False)
    code, doc, _, err = s.cmd('check', 'force')
    s.check(code == 1 and 'network is down' in err and row(doc, 'claude-cli')['status'] == 'error')
    again = row(s.cmd()[1], 'claude-cli')
    s.check((again['status'], again['can_install']) == ('available', True))
    add(s)

    s = cli('cli/stale-or-foreign-token-runs-nothing', defs)
    check(s)
    token = s.token('claude-cli')
    ran = []
    for bad in ('b' * 64, 'not-a-token'):
        s.check(s.cmd('install', bad)[0] == 1)
        ran += updates(s, s.calls)
    s.set_fake(cli_installed='2.1.0')
    s.check(s.cmd('install', token)[0] == 1)
    s.check(ran + updates(s, s.calls) == [])
    add(s)

    s = cli('cli/homebrew-install-is-checked-against-the-cask', defs)
    s.write('opt/Caskroom/claude-code@latest/2.0.0/claude', 'fake')
    s.set_env('CCA_CLI_CLAUDE', '<ROOT>/opt/Caskroom/claude-code@latest/2.0.0/claude')
    s.set_fake(cli_body=json.dumps({'version': '2.1.0'}).encode())
    r = check(s)
    s.check(s.raw_downloads == ['https://formulae.brew.sh/api/cask/claude-code@latest.json'])
    s.check((r['status'], r['can_install'], r['install_token']) == ('available', False, None) and 'brew upgrade --cask claude-code@latest' in r['message'])
    add(s)

    s = cli('cli/npm-install-is-checked-against-npm', defs)
    s.write('lib/node_modules/@anthropic-ai/claude-code/cli.js', 'fake')
    s.set_env('CCA_CLI_CLAUDE', '<ROOT>/lib/node_modules/@anthropic-ai/claude-code/cli.js')
    s.set_fake(cli_body=json.dumps({'version': '2.1.0'}).encode())
    r = check(s)
    s.check(s.raw_downloads == ['https://registry.npmjs.org/@anthropic-ai%2Fclaude-code/latest'])
    s.check((r['status'], r['can_install']) == ('available', False) and 'npm install -g @anthropic-ai/claude-code@latest' in r['message'])
    add(s)

    s = cli('cli/installed-somewhere-else', defs, '认不出是怎么装的：照官方的地址查，告诉用户运行它自己的 update')
    s.write('bin/claude', 'fake')
    s.set_env('CCA_CLI_CLAUDE', '<ROOT>/bin/claude')
    r = check(s)
    s.check((r['status'], r['can_install']) == ('available', False) and 'claude update' in r['message'])
    add(s)

    s = cli('cli/codex-npm-and-brew', defs)
    s.set_env('CCA_CLI_CLAUDE', '')
    s.write('lib/node_modules/@openai/codex/bin/codex.js', 'fake')
    s.set_env('CCA_CLI_CODEX', '<ROOT>/lib/node_modules/@openai/codex/bin/codex.js')
    s.set_fake(cli_installed='0.166.0', cli_latest='0.167.0', cli_body=json.dumps({'version': '0.167.0'}).encode())
    r = check(s, 'codex-cli')
    s.check(s.raw_downloads == ['https://registry.npmjs.org/@openai%2Fcodex/latest'] and 'npm install -g @openai/codex@latest' in r['message'])
    s.write('opt/Caskroom/codex/0.166.0/codex', 'fake')
    s.set_env('CCA_CLI_CODEX', '<ROOT>/opt/Caskroom/codex/0.166.0/codex')
    r = check(s, 'codex-cli')
    s.check(s.raw_downloads[-1] == 'https://formulae.brew.sh/api/cask/codex.json' and 'brew upgrade --cask codex' in r['message'])
    add(s)

    s = cli('cli/claude-channel-is-respected', defs)
    s.write('.claude/settings.json', json.dumps({'autoUpdatesChannel': 'stable'}))
    check(s)
    s.check(s.raw_downloads == ['https://downloads.claude.ai/claude-code-releases/stable'])
    s.check(s.cmd('install', s.token('claude-cli'))[0] == 0)
    s.check(json.loads(updates(s, s.calls)[0]['settings']) == {'autoUpdatesChannel': 'stable'})
    add(s)

    s = cli('cli/garbage-from-the-network-is-an-error-not-an-update', defs)
    for body in (b'<html>blocked</html>', b'{}', b'{"version": "latest"}', b'', b'{"version": 2}', b'{oops', b'v2.1.0\n', b'rust-v2.1.0', b'[1]', b'{"tag_name": "v2.1.0", "version": ""}',
                 b'\xff2.1.0', b' 2.1.0 \n\n', b'2.1', b'2.1.0.1'):
        s.set_fake(cli_body=body)
        code, doc, _, _ = s.cmd('check', 'force')
        r = row(doc, 'claude-cli')
        if body in (b'<html>blocked</html>', b'{}', b'{"version": "latest"}', b''):
            s.check(code == 1 and (r['status'], r['can_install']) == ('error', False), body)
    add(s)

    s = cli('cli/unreadable-version-blocks-the-row-only', defs)
    s.set_fake(cli_run_rc=1)
    r = row(s.cmd()[1], 'claude-cli')
    s.check((r['status'], r['can_install']) == ('blocked', False))
    add(s)

    s = cli('cli/both-tools-and-a-desktop-client', defs, '终端工具先查，桌面客户端后查；各有各的结果')
    s.set_env('CCA_CLI_CODEX', '<ROOT>/' + CODEX_BIN)
    s.set_env('CCA_CLIENT_CLAUDE_APP', '<ROOT>/' + CLAUDE_APP)
    s.app(CLAUDE_APP, 'claude', '4.1.0', '4.1.0')
    package = app_zip('claude', '4.2.0', '4.2.0')
    s.set_fake(pkg_claude=package, feed_claude=claude_feed(package), cli_body=b'{"tag_name": "rust-v2.1.0", "version": "2.1.0"}')
    code, doc, out, err = s.cmd('check', 'force')
    s.check(code == 0 and [c['status'] for c in doc['clients']] == ['staged', 'missing', 'available', 'available'], err + out)
    s.check([u.split('?')[0] for u in s.raw_downloads][:2] == ['https://downloads.claude.ai/claude-code-releases/latest', 'https://api.github.com/repos/openai/codex/releases/latest'])
    add(s)

    # ===== 真的 cu_download，底下的 curl 是照剧本回的 =====
    s = desktop_codex('download/redirects-retries-and-sizes', defs, 'curl 的命令行、手动跟跳转、三次重试、大小核对')
    feed = 'https://persistent.oaistatic.com/codex-app-prod/appcast.xml'
    pkg = 'https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-2.0.zip'
    ok = lambda body=b'feed body', **kw: dict({'stdout_b64': b64(b'200'), 'body_b64': b64(body)}, **kw)
    redirect = lambda location, code=b'302': {'stdout_b64': b64(code), 'header_b64': b64(b'HTTP/2 302\r\n' + location + b'\r\n\r\n')}
    plans = [
        ('ok', feed, False, None, [], [ok()]),
        ('ok-with-whitespace-around-code', feed, False, None, [], [ok(stdout_b64=b64(b' 200\n'))]),
        ('foreign-redirect', feed, False, None, [], [{'stdout_b64': b64(b'302'), 'header_b64': b64(b'HTTP/2 302\nLocation: https://example.invalid/stolen\n')}]),
        ('redirect-same-origin-relative', pkg, True, None, [], [redirect(b'location: /codex-app-prod/ChatGPT-darwin-arm64-2.0.zip'), ok(b'zip')]),
        ('redirect-absolute', pkg, True, None, [], [redirect(b'LOCATION:   ' + pkg.encode() + b'  ', b'307'), ok(b'zip')]),
        ('feed-url-as-package', feed, True, None, [], []),
        ('redirect-two-locations', pkg, True, None, [], [redirect(b'Location: /a\r\nLocation: /b')]),
        ('redirect-no-location', pkg, True, None, [], [redirect(b'X-Other: 1')]),
        ('redirect-to-feed-when-package', pkg, True, None, [], [redirect(b'Location: /codex-app-prod/appcast.xml')]),
        ('redirect-with-fragment', pkg, True, None, [], [redirect(b'Location: ' + pkg.encode() + b'#x')]),
        ('redirect-http', pkg, True, None, [], [redirect(b'Location: http://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-2.0.zip')]),
        ('redirect-bad-ipv6', pkg, True, None, [], [redirect(b'Location: //[::1/x')]),
        ('redirect-latin1-header', pkg, True, None, [], [redirect(b'X-Name: caf\xe9\r\nLocation:\xa0' + pkg.encode() + b'\x85'), ok(b'zip')]),
        ('redirect-bare-cr-lines', pkg, True, None, [], [{'stdout_b64': b64(b'301'), 'header_b64': b64(b'HTTP/1.1 301\rLocation: ' + pkg.encode() + b'\rX: y')}, ok(b'zip')]),
        ('redirect-six-hops-then-retry', pkg, True, None, [], [redirect(b'Location: ' + pkg.encode())] * 18),
        ('redirect-header-missing', pkg, True, None, [], [{'stdout_b64': b64(b'303')}]),
        ('redirect-then-ok', pkg, True, None, [], [redirect(b'Location: ' + pkg.encode()), ok(b'zip')]),
        ('redirect-header-stale', pkg, True, None, [], [{'stdout_b64': b64(b'303')}, ok(b'zip')]),
        ('code-404-three-times', feed, False, None, [], [{'stdout_b64': b64(b'404')}] * 3),
        ('curl-fails-then-ok', feed, False, None, [], [{'rc': 6}, {'rc': 28}, ok()]),
        ('curl-cannot-run-three-times', feed, False, None, [], [{'fail': True}] * 3),
        ('empty-code-not-listed', feed, False, None, ['204'], [{'stdout_b64': b64(b'404')}] * 3),
        ('size-zero', feed, False, None, [], [ok(b'')]),
        ('size-differs-from-expected', pkg, True, 5, [], [ok(b'1234')]),
        ('size-as-expected', pkg, True, 4, [], [ok(b'1234')]),
        ('size-expected-as-float', pkg, True, 4.0, [], [ok(b'1234')]),
        ('size-expected-as-string', pkg, True, '4', [], [ok(b'1234')]),
        ('feed-over-two-megabytes', feed, False, None, [], [ok(b'x' * (2 * 1024 * 1024 + 1))]),
        ('output-missing', feed, False, None, [], [{'stdout_b64': b64(b'200')}]),
        ('url-not-allowed', 'https://example.invalid/appcast.xml', False, None, [], []),
        ('code-not-ascii', feed, False, None, [], [{'stdout_b64': b64(b'\xff200')}] * 3),
    ]
    for i, (name, url, package, expected, empty, script) in enumerate(plans):
        s.set_fake(curl=[dict(r) for r in script])
        got = s.call('cu_download', url=url, package=package, expected_size=expected, empty=empty, dest='out-%d' % i, label=name, stale=name.endswith('-stale'))
        # 剧本要正好用完：多了少了都说明这一条没走到想测的路
        # 剧本要正好用完：多了少了都说明这一条没走到想测的路
        s.check(not s.fake['curl'] and got.get('err') != 'IndexError', '%s：剧本没对上 %r' % (name, got))
        if name == 'foreign-redirect':
            # test_clean_download_environment_and_manual_redirect_boundary：curl 不读配置、不走代理、不自己跟跳转，跳到别处的不跟
            args = s.calls[0]['args']
            s.check(got == {'cu': ['blocked', '更新地址不符合官方来源规则']} and len(s.calls) == 1 and args[:2] == ['/usr/bin/curl', '-q'] and '--no-netrc' in args
                    and args[args.index('--proxy') + 1] == '' and '-L' not in args and '--location' not in args, args)
    s.set_cu('claude')
    s.set_fake(curl=[redirect(b'Location: https://downloads.claude.ai/releases/darwin/arm64/4.2.0/Claude-abcdef0.zip'), ok(b'zip')])
    got = s.call('cu_download', url='https://downloads.claude.ai/releases/darwin/arm64/4.2.0/Claude-1234567.zip', package=True, expected_size=None, empty=[], dest='out-claude', label='claude', stale=False)
    s.check(not s.fake['curl'] and got == {'ok': None, 'dest': [b64(b'zip'), '600']}, got)
    # 版本列表说“没有更新”的状态码（204、这种芯片没有单独的版本列表时的 404）原样交回去，不算失败
    squirrel = 'https://api.anthropic.com/api/desktop/darwin/arm64/squirrel/update?device_id=%s&version=4.1.0&os_version=26.0' % ('0' * 36)
    for name, empty, script, want in (
            ('empty-code-204', ['204', '404'], [{'stdout_b64': b64(b'204')}], {'ok': '204'}),
            ('empty-code-404-with-whitespace', ['204', '404'], [{'stdout_b64': b64(b' 404\n')}], {'ok': '404'}),
            ('empty-code-after-redirect', ['204', '404'], [{'stdout_b64': b64(b'302'), 'header_b64': b64(b'HTTP/2 302\r\nLocation: ' + squirrel.encode() + b'\r\n\r\n')}, {'stdout_b64': b64(b'204')}], {'ok': '204'}),
            ('empty-code-not-in-the-list', ['204'], [{'stdout_b64': b64(b'404')}] * 3, {'cu': ['error', '无法从官方来源下载更新，请稍后重试']})):
        s.set_fake(curl=[dict(r) for r in script])
        got = s.call('cu_download', url=squirrel, package=False, expected_size=None, empty=empty, dest='out-' + name, label=name, stale=False)
        s.check(not s.fake['curl'] and got == want, '%s：%r' % (name, got))
    add(s)
    return done


def call_download_label(s, result, label, **kw):
    return call_download(s, result, **kw)


CALLS['cu_download'] = call_download_label


def bad_deflate(data):
    """头一项压缩数据的第一个字节换成 0x07：最后一块、块类型 3（没有这种类型）"""
    name_len, extra_len = struct.unpack('<HH', data[26:30])
    at = 30 + name_len + extra_len
    return data[:at] + b'\x07' + data[at + 1:]


# --- 一个个函数 --------------------------------------------------------------------------------------------------------
def functions(defs):
    out = {}
    env = {'HOME': '/nonexistent/home', 'CCA_HOME': '/nonexistent/cca-unit-tests', 'CCA_CLIENT_CODEX_APP': '', 'CCA_CLIENT_CLAUDE_APP': '', 'CCA_CLI_CLAUDE': '', 'CCA_CLI_CODEX': ''}
    ns = {}
    with mock.patch.dict(os.environ, env, clear=True):
        exec(compile(defs, 'PY_SRC', 'exec'), ns)
    clients = {c.id: c for c in ns['CU_CLIENTS']}

    def with_cu(client, f):
        ns['CU'] = clients[client]
        return outcome(f)

    # 地址规则
    urls = [
        'https://persistent.oaistatic.com/codex-app-prod/appcast.xml', 'https://persistent.oaistatic.com/codex-app-prod/appcast.xml?', 'https://persistent.oaistatic.com/codex-app-prod/appcast.xml?x=1',
        'https://persistent.oaistatic.com/codex-app-prod/appcast.xml#f', 'https://PERSISTENT.oaistatic.com/codex-app-prod/appcast.xml', 'https://persistent.oaistatic.com:443/codex-app-prod/appcast.xml',
        'https://persistent.oaistatic.com:8443/codex-app-prod/appcast.xml', 'https://persistent.oaistatic.com:/codex-app-prod/appcast.xml', 'https://user@persistent.oaistatic.com/codex-app-prod/appcast.xml',
        'https://:pw@persistent.oaistatic.com/codex-app-prod/appcast.xml', 'https://@persistent.oaistatic.com/codex-app-prod/appcast.xml', 'http://persistent.oaistatic.com/codex-app-prod/appcast.xml',
        'HTTPS://persistent.oaistatic.com/codex-app-prod/appcast.xml', 'https://persistent.oaistatic.com/codex-app-prod/appcast.xml ', ' https://persistent.oaistatic.com/codex-app-prod/appcast.xml',
        'https://persistent.oaistatic.com/codex-app-prod/app\\cast.xml', 'https://persistent.oaistatic.com/codex-app-prod/appcast.xml\x7f', 'https://persistent.oaistatic.com/codex-app-prod/appcast.xml\n',
        'https://persistent.oaistatic.com.evil.example/codex-app-prod/appcast.xml', 'https://evil.example/persistent.oaistatic.com/codex-app-prod/appcast.xml', 'https://persistent.oaistatic.com//codex-app-prod/appcast.xml',
        'https://persistent.oaistatic.com/codex-app-prod/../codex-app-prod/appcast.xml', 'https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-2.0.zip',
        'https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-x64-2.0.zip', 'https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-universal-2.0.zip',
        'https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-2.zip', 'https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-1.2.3.4.5.6.zip',
        'https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-1.2.3.4.5.6.7.zip', 'https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-2.0.zip.zip',
        'https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-2.0.ZIP', 'https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-٢.٠.zip',
        'https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-2.0.zip?x', 'https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-2..0.zip',
        'https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-.zip', 'https://persistent.oaistatic.com/codex-app-prod/x/ChatGPT-darwin-arm64-2.0.zip',
        'https://downloads.claude.ai/releases/darwin/arm64/4.2.0/Claude-%s.zip' % ('a' * 40), 'https://downloads.claude.ai/releases/darwin/x64/4.2.0/Claude-abcdef0.zip',
        'https://downloads.claude.ai/releases/darwin/universal/4.2/Claude-abcdef0.zip', 'https://downloads.claude.ai/releases/darwin/arm64/4.2.0/Claude-abcdef.zip',
        'https://downloads.claude.ai/releases/darwin/arm64/4.2.0/Claude-%s.zip' % ('a' * 65), 'https://downloads.claude.ai/releases/darwin/arm64/4.2.0/Claude-ABCDEF0.zip',
        'https://downloads.claude.ai/releases/darwin/arm64/4/Claude-abcdef0.zip', 'https://downloads.claude.ai/releases/darwin/arm/4.2.0/Claude-abcdef0.zip',
        'https://downloads.claude.ai/releases/darwin/arm64/4.2.0/x/Claude-abcdef0.zip', 'https://downloads.claude.ai/releases/darwin/arm64/4.2.0/Claude-abcdef0.delta',
        'https://downloads.claude.ai/releases/darwin/arm64/4.2.0/Claude-abcdef0.zip?x=1', 'https://downloads.claude.ai/releases/darwin/arm64/4.2.0/Claude-abcdef0.zip?',
        'https://downloads.claude.ai/releases/darwin/arm64/4.2.0/Claude-abcdef0.zip/', 'https://storage.example.invalid/Claude.zip',
        'https://api.anthropic.com/api/desktop/darwin/arm64/squirrel/update', 'https://api.anthropic.com/api/desktop/darwin/arm64/squirrel/update?device_id=x&version=1&os_version=2',
        'https://api.anthropic.com/api/desktop/darwin/arm64/squirrel/update?device_id=%s&version=4.1.0&os_version=26.0' % ('0123456789abcdef-' * 2 + '01'),
        'https://api.anthropic.com/api/desktop/darwin/universal/squirrel/update?device_id=%s&version=4.1.0&os_version=26.0' % ('-' * 36),
        'https://api.anthropic.com/api/desktop/darwin/x64/squirrel/update?device_id=%s&version=4&os_version=2' % ('a' * 36),
        'https://api.anthropic.com/api/desktop/darwin/arm64/squirrel/update?device_id=%s&version=4.1.0&os_version=26.0&x=1' % ('a' * 36),
        'https://api.anthropic.com/api/desktop/darwin/arm64/squirrel/update?device_id=%s&version=&os_version=26.0' % ('a' * 36),
        'https://api.anthropic.com/api/desktop/darwin/arm64/squirrel/update?device_id=%s&version=4.1.0a&os_version=26.0' % ('a' * 36),
        'https://api.anthropic.com/api/desktop/darwin/arm64/squirrel/update?device_id=%s&version=٤.١&os_version=26.0' % ('a' * 36),
        'https://api.anthropic.com/api/desktop/darwin/arm64/squirrel/update?device_id=%s&version=4.1.0&os_version=26.0' % ('A' * 36),
        'https://api.anthropic.com/api/desktop/darwin/arm64/squirrel/update/?device_id=%s&version=4.1.0&os_version=26.0' % ('a' * 36),
        'https://downloads.claude.ai/claude-code-releases/latest', 'https://downloads.claude.ai/claude-code-releases/stable', 'https://downloads.claude.ai/claude-code-releases/beta',
        'https://downloads.claude.ai/claude-code-releases/2.0.0/claude', 'https://downloads.claude.ai/claude-code-releases/latest?x=1', 'https://downloads.claude.ai/claude-code-releases/latest/',
        'https://example.invalid/claude-code-releases/latest', 'http://downloads.claude.ai/claude-code-releases/latest', 'https://api.github.com/repos/openai/codex/releases/latest',
        'https://api.github.com/repos/evil/codex/releases/latest', 'https://registry.npmjs.org/@openai%2Fcodex/latest', 'https://registry.npmjs.org/@openai/codex/latest',
        'https://registry.npmjs.org/@anthropic-ai%2Fclaude-code/latest', 'https://registry.npmjs.org/@anthropic-ai%2Fclaude-code/stable', 'https://registry.npmjs.org/@anthropic-ai%2Fclaude-code/next',
        'https://registry.npmjs.org/@openai%2Fcodex/stable', 'https://formulae.brew.sh/api/cask/codex.json', 'https://formulae.brew.sh/api/cask/claude-code.json',
        'https://formulae.brew.sh/api/cask/claude-code@latest.json', 'https://formulae.brew.sh/api/cask/evil.json', 'https://formulae.brew.sh/api/cask/claude-code@beta.json',
        'https://formulae.brew.sh/api/cask/codexXjson', 'https://formulae.brew.sh/api/formula/codex.json', 'https://[::1]/claude-code-releases/latest', 'https://[::1/x', 'https://host:99999/x',
        'https://host:x/', 'https://ex℀mple.com/', 'https:///nohost', 'https://', '', 'x', 'ftp://downloads.claude.ai/claude-code-releases/latest',
    ]
    values = urls + [None, 5, ['https://x'], True]
    out['url'] = [[c, u, p, with_cu(c, lambda: ns['cu_url'](u, p))] for c in clients for p in (False, True) for u in values]

    # 版本号、构建号
    versions = ['1', '1.2', '1.2.3.4.5.6', '1.2.3.4.5.6.7', '', '1.', '.1', '1..2', '1.2\n', ' 1.2', '1.2 ', 'a', '1.a', '٣.٤', '１.２', '²', '0', '00.01', '9' * 30, '9' * 4301, '1.' + '9' * 4301,
                '1_0', '+1', '-1', '1.0.0.0.0.0', '4.23118.0', '4.9.0', '26.0', '13.0', '27.2.1', None, 5, 1.5, True, ['1', '2'], {'a': 1}]
    tup = lambda t: [str(x) for x in t]
    out['version'] = [[v, outcome(lambda: tup(ns['cu_version'](v)))] for v in versions]
    builds = versions + ['100', '99', '0100', ' 200 ', '2_00', '-5', '+7', 'abc', '1e3', '0x10', '١٢٣', '1.2.3', ['.'], ['a', '.'], {'.': 1}, {}, [], 200, 200.5, False, '9' * 4300, '．']
    out['build'] = [[v, outcome(lambda: tup(ns['cu_build'](v)))] for v in builds]
    pairs = [('200', '100'), ('100', '200'), ('100', '100'), ('0100', '100'), ('99', '100'), ('9' * 30, '9' * 29 + '8'), ('-5', '3'), ('-5', '-7'), ('4.23118.0', '4.9.0'), ('4.9.0', '4.23118.0'),
             ('4.2.0', '4.2'), ('4.2', '4.2.0.0.1'), ('200', '4.2.0'), ('4.2.0', '200'), ('5', '4.2.0'), ('4', '4.0.0'), ('4.0.0', '4'), ('1.2.3.4.5.6', '1.2.3.4.5.7'), ('٣', '2'), ('1.0', '1.0.0.0.0.0')]
    out['build_order'] = [[a, b, outcome(lambda: [ns['cu_build'](a) <= ns['cu_build'](b), ns['cu_build'](a) < ns['cu_build'](b), ns['cu_build'](a) > ns['cu_build'](b)])] for a, b in pairs]

    # 版本列表
    package = app_zip('codex', '2.0', '200')
    feeds = {
        'good': codex_feed(package), 'foreign-url': codex_feed(package, url='https://example.invalid/package.zip'), 'other-arch': codex_feed(package, arch='x64'),
        'minimum-too-new': codex_feed(package, minimum='99.0'), 'minimum-equal': codex_feed(package, minimum='26.0'), 'minimum-missing': codex_feed(package, minimum=''),
        'minimum-garbage': codex_feed(package, minimum='thirteen'), 'build-not-digits': codex_feed(package, build='2a'), 'build-13-digits': codex_feed(package, build='1' * 13),
        'version-one-part': codex_feed(package, version='2', url='https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-2.zip'),
        'url-version-mismatch': codex_feed(package, version='2.1'), 'no-deltas': codex_feed(package, deltas=False), 'not-xml': b'hello', 'empty': b'', 'doctype': b'<!DOCTYPE rss><rss/>',
        'doctype-lowercase': b'<!doctype rss><rss/>', 'entity': b'<rss><!ENTITY x "y"></rss>', 'entity-word-in-text': b'<rss><channel><item>&lt;!entity</item></channel></rss>',
        'entity-word-literal-in-cdata': b'<rss><![CDATA[<!ENTITY]]></rss>', 'wrong-root': b'<feed/>', 'namespaced-root': b'<rss xmlns="u"/>', 'no-channel': b'<rss/>',
        'too-big': b'<rss>' + b' ' * (2 * 1024 * 1024) + b'</rss>', 'just-under-too-big': b'<rss>' + b' ' * (2 * 1024 * 1024 - 11) + b'</rss>',
    }

    def feed_with(items):
        return ('<rss xmlns:sparkle="%s" xmlns:s2="%s"><channel>%s</channel><channel>%s</channel></rss>' % (NS, NS, ''.join(items[:-1]), items[-1])).encode()

    def item(build='200', version='2.0', arch='arm64', minimum='13.0', url=None, length=None, sig=SIG, more='', enclosure=None, prefix='sparkle'):
        url = url or 'https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-%s.zip' % version
        enclosure = enclosure if enclosure is not None else '<enclosure url="%s" length="%s" %s:edSignature="%s"/>' % (url, len(package) if length is None else length, prefix, sig)
        return ('<item><%s:version>%s</%s:version><%s:shortVersionString>%s</%s:shortVersionString><%s:hardwareRequirements>%s</%s:hardwareRequirements>'
                '<%s:minimumSystemVersion>%s</%s:minimumSystemVersion>%s%s</item>' % (prefix, build, prefix, prefix, version, prefix, prefix, arch, prefix, prefix, minimum, prefix, enclosure, more))
    feeds.update({
        'picks-highest-build': feed_with([item('200'), item('300', '3.0'), item('250', '2.5')]), 'first-of-equal-builds': feed_with([item('300', '3.0'), item('0300', '3.1')]),
        'second-channel-counts': feed_with([item('200'), item('400', '4.0')]), 'other-prefix-same-namespace': feed_with([item(prefix='s2')]),
        'channel-element-skips': feed_with([item('300', '3.0', more='<sparkle:channel>beta</sparkle:channel>'), item('200')]),
        'two-enclosures-skip': feed_with([item('300', '3.0', more='<enclosure url="x"/>'), item('200')]), 'no-enclosure-skips': feed_with([item('300', '3.0', enclosure=''), item('200')]),
        'delta-from-skips': feed_with([item('300', '3.0', enclosure='<enclosure url="x" sparkle:deltaFrom="100"/>'), item('200')]),
        'length-zero': feed_with([item(length='0')]), 'length-negative': feed_with([item(length='-5')]), 'length-too-big': feed_with([item(length=str(2 * 1024 ** 3 + 1))]),
        'length-max': feed_with([item(length=str(2 * 1024 ** 3))]), 'length-spaces': feed_with([item(length=' 12 ')]), 'length-underscore': feed_with([item(length='1_2')]),
        'length-float': feed_with([item(length='12.0')]), 'length-missing': feed_with([item(enclosure='<enclosure url="https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-2.0.zip" sparkle:edSignature="%s"/>' % SIG)]),
        'length-arabic': feed_with([item(length='١٢')]), 'length-huge': feed_with([item(length='9' * 40)]),
        'signature-short': feed_with([item(sig=base64.b64encode(b'X' * 63).decode())]), 'signature-not-base64': feed_with([item(sig='!!!!')]), 'signature-missing': feed_with([item(sig='')]),
        'signature-with-newline': feed_with([item(sig=SIG[:40] + '\n' + SIG[40:])]), 'signature-not-ascii': feed_with([item(sig='é' * 88)]), 'signature-urlsafe': feed_with([item(sig=SIG.replace('Y', '_'))]),
        'signature-unprefixed': feed_with([item(enclosure='<enclosure url="https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-2.0.zip" length="5" edSignature="%s"/>' % SIG)]),
        'url-missing': feed_with([item(enclosure='<enclosure length="5" sparkle:edSignature="%s"/>' % SIG)]), 'url-with-space': feed_with([item(url='https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-2.0.zip ')]),
        'url-with-entity': feed_with([item(url='https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-2&#46;0.zip')]),
        'text-with-cdata-and-comment': feed_with([item(build='2<!-- c -->0<![CDATA[0]]>')]), 'text-with-whitespace': feed_with([item(build=' 200 ')]),
        'text-after-child-ignored': ('<rss xmlns:sparkle="%s"><channel>%s</channel></rss>' % (NS, item().replace('<sparkle:version>200</sparkle:version>', '<sparkle:version><x/>200</sparkle:version>'))).encode(),
        'fullwidth-build': feed_with([item(build='２００')]), 'utf16': feed_with([item()]).decode().encode('utf-16'), 'utf16-with-doctype': ('<!DOCTYPE rss>' + feed_with([item()]).decode()).encode('utf-16'),
        'latin1-declared': b'<?xml version="1.0" encoding="ISO-8859-1"?>' + feed_with([item()]), 'bom': b'\xef\xbb\xbf' + feed_with([item()]),
        'items-outside-channel-ignored': ('<rss xmlns:sparkle="%s">%s<channel><x>%s</x></channel></rss>' % (NS, item(), item())).encode(),
        'duplicate-fields-first-wins': ('<rss xmlns:sparkle="%s"><channel>%s</channel></rss>' % (NS, item().replace('<item>', '<item><sparkle:version>999</sparkle:version>'))).encode(),
    })
    ns['CU'] = clients['codex']
    out['feed'] = [[name, b64(data), arch, system, outcome(lambda: ns['cu_parse_feed'](data, arch, system))]
                   for name, data in feeds.items() for arch, system in (('arm64', '26.0'), ('x64', '26.0'), ('arm64', '12.6'), ('arm64', '13'))
                   if (arch, system) == ('arm64', '26.0') or name in ('good', 'other-arch', 'minimum-equal', 'picks-highest-build', 'minimum-garbage')]

    cpkg = app_zip('claude', '4.2.0', '4.2.0')
    good = json.loads(claude_feed(cpkg))
    hexa = 'a' * 40

    def sq(**kw):
        d = json.loads(json.dumps(good))
        d['releases'][0]['updateTo'].update(kw)
        return json.dumps(d).encode()

    def top(**kw):
        d = json.loads(json.dumps(good))
        d.update(kw)
        return json.dumps(d).encode()
    squirrels = {
        'good': claude_feed(cpkg), 'foreign-url': sq(url='https://example.invalid/Claude.zip'), 'http': sq(url='http://downloads.claude.ai/releases/darwin/arm64/4.2.0/Claude-%s.zip' % hexa),
        'delta-url': sq(url='https://downloads.claude.ai/releases/darwin/arm64/4.2.0/Claude-%s.delta' % hexa), 'x64-url': sq(url='https://downloads.claude.ai/releases/darwin/x64/4.2.0/Claude-%s.zip' % hexa),
        'other-version-url': sq(url='https://downloads.claude.ai/releases/darwin/arm64/4.1.0/Claude-%s.zip' % hexa), 'universal-url': sq(url='https://downloads.claude.ai/releases/darwin/universal/4.2.0/Claude-%s.zip' % hexa),
        'url-number': sq(url=5), 'url-null': sq(url=None), 'digest-nope': sq(sha256='nope'), 'digest-uppercase': sq(sha256='A' * 64), 'digest-number': sq(sha256=5), 'digest-newline': sq(sha256='a' * 64 + '\n'),
        'size-zero': sq(size=0), 'size-true': sq(size=True), 'size-float': sq(size=5.0), 'size-string': sq(size='5'), 'size-max': sq(size=2 * 1024 ** 3), 'size-over': sq(size=2 * 1024 ** 3 + 1),
        'size-huge': sq(size=10 ** 30), 'size-negative': sq(size=-1), 'listed-other': claude_feed(cpkg, listed='9.9.9'), 'not-json': b'not json', 'list': b'[]', 'empty': b'',
        'current-release-number': b'{"currentRelease": 1, "releases": []}', 'releases-dict': top(releases={}), 'no-releases': top(releases=[]), 'release-not-dict': top(releases=['4.2.0']),
        'two-matching-releases': top(releases=good['releases'] * 2), 'update-to-missing': top(releases=[{'version': '4.2.0'}]), 'update-to-list': top(releases=[{'version': '4.2.0', 'updateTo': []}]),
        'update-to-other-version': sq(version='4.3.0'), 'version-one-part': top(currentRelease='4'), 'version-newline': top(currentRelease='4.2.0\n'), 'utf16': claude_feed(cpkg).decode().encode('utf-16'),
        'utf8-bom': b'\xef\xbb\xbf' + claude_feed(cpkg), 'utf32': claude_feed(cpkg).decode().encode('utf-32-le'), 'not-utf8': b'\xff\xfe\xfd', 'too-big': b' ' * (2 * 1024 * 1024 + 1),
        'trailing-garbage': claude_feed(cpkg) + b'x', 'nested-deep': b'[' * 1500 + b']' * 1500 if False else b'{"currentRelease": "4.2.0"}', 'nan': b'{"currentRelease": NaN, "releases": []}',
    }
    ns['CU'] = clients['claude']
    out['squirrel'] = [[name, b64(data), arch, outcome(lambda: ns['cu_parse_squirrel'](data, arch))] for name, data in squirrels.items() for arch in ('arm64', 'universal', 'x64')
                       if arch == 'arm64' or name in ('good', 'universal-url', 'x64-url')]

    bodies = [b'2.1.0\n', b'v2.1.0', b'rust-v0.167.0', b'rust-v', b'vv1.2.3', b'rust-vv1.2.3', b' 2.1.0 \n\n', b'2.1', b'2.1.0.1', b'2.1.0-beta', b'', b'{}', b'{"version": "2.1.0"}',
              b'{"tag_name": "rust-v0.167.0"}', b'{"version": "", "tag_name": "v1.2.3"}', b'{"version": 0, "tag_name": "v1.2.3"}', b'{"version": 2}', b'{"version": ["1.2.3"]}',
              b'{"version": "latest"}', b'{"version": null}', b'{oops', b'{"version": "1.2.3"} x', b'[{"version": "1.2.3"}]', b' {"version": "1.2.3"}', b'\xff2.1.0', b'\xef\xbb\xbf2.1.0',
              '٢.١.٠'.encode(), b'2.1.0\x0b', b'2.1.0\xc2\x85', b'{"version": "1.2.3", "version": "4.5.6"}', b'{"version": true, "tag_name": "v9.9.9"}', b'{"version": 1.5}', b'x2.1.0', b'V2.1.0',
              b'{"version": {"a": 1}}', b'{"tag_name": "rust-v0.167.0", "version": null}']
    ns['CU'] = clients['claude-cli']
    out['cli_latest'] = [[b64(d), outcome(lambda: ns['cu_cli_latest'](d))] for d in bodies]

    # 三个带前瞻、后顾或者拼出来的正则：Rust 里是手写的判断。正则是从原文里抄的，原文改了这里会发现
    patterns = {'version': r"r'(?<![\d.])\d+\.\d+\.\d+(?![\d.])'", 'ps': r"re.match(r'^\s*(\d+)\s+(.+)$', line)",
                'path': r"re.search(r'(?:^|[\s=\x22\x27])' + re.escape(path) + r'(?:[/\s\x22\x27]|$)', command)"}
    for name, literal in patterns.items():
        assert literal in defs, name
    texts = ['2.0.0 (Claude Code)', 'codex-cli 0.166.0', 'v1.2.3', '1.2.3.4', '1.2.3.4 then 5.6.7', 'x1.2.3-beta', '12.34.56', '1.2', '1.2.', '.1.2.3', '1..2.3 4.5.6', 'a1.2.3b', '1.2.3.', '١.٢.٣', '',
             '10.20.30\n', '9 1.2.3', '1.2.x 3.4.5', '1.2.3\n4.5.6', '1.2.34.5', '1.2.3-4.5.6', '0.0.0', '1.2.3٤', '٤1.2.3', '1.2.3．4', 'a.1.2.3', '1.2.3/4', '99999999999999999999.0.1',
             '1 . 2 . 3', '1.2.3\x00', '²1.2.3', '1.2.3²', '\U0001d7ce.1.2', 'x\n\n2.3.4.\n5.6.7']
    out['version_search'] = [[t, (lambda m: m.group(0) if m else None)(re.search(r'(?<![\d.])\d+\.\d+\.\d+(?![\d.])', t))] for t in texts]
    lines = ['  123 /sbin/launchd', '1 a b  c ', '1  ', '1 \t', '1 ', '1', '12a b', 'a 1 b', '', '   ', '　٣٤\xa0x', '7 <defunct>', '1\x1fx', '1 \x85', '1\x0b\x0bx', ' 1 x\ty', '0001 x', '1 ２',
             '1​x', '1 ​', '-1 x', '1.5 x', '1  x ', '\t\t42\t\tcmd', '4 2']
    out['ps_line'] = [[line, (lambda m: [m.group(1), m.group(2)] if m else None)(re.match(r'^\s*(\d+)\s+(.+)$', line))] for line in lines]
    app = '/Applications/ChatGPT.app'
    commands = [app + '/Contents/MacOS/ChatGPT', app, 'open -a ' + app + ' --args x', 'x --path=' + app, 'x "' + app + '"', "x '" + app + "'", app + '2/Contents/MacOS/x', '/tmp' + app + '/x',
                'x:' + app, app + 'arently ' + app, '', app + '\t', '\xa0' + app, 'x　' + app + ' y', 'x,' + app, app + ',', app + '=', '=' + app + '/', app.lower(), 'x ' + app + '.bak',
                app[1:], 'a' + app, '(' + app + ')', app + '"', "'" + app]
    out['path_in_command'] = [[p, c, bool(re.search(r'(?:^|[\s=\x22\x27])' + re.escape(p) + r'(?:[/\s\x22\x27]|$)', c))] for p in (app, 'a a', '/x/应用 程序/A.app', '', '.', 'a+b[c]')
                              for c in commands + ['xa a a', 'xa ab', 'a a', '/x/应用 程序/A.app/y', 'a+b[c] ', 'aab[c]', '.', 'x', ' . ']]

    out['environment'] = [[f, [[k, v] for k, v in ns['cu_environment'](f).items()]] for f in ('/private/tmp/example', '', '/a b/中')]
    for bad in ('OPENAI_API_KEY', 'HTTPS_PROXY'):
        assert bad not in ns['cu_environment']('/private/tmp/example')

    # 进程列表
    src_words = 'CU_CODEX = CuClient(com.openai.codex Sparkle|Autoupdate|Updater\\.app) ' + app
    procs = [
        ('plain', 'codex', b'1 /sbin/launchd\n', None, [app]),
        ('instance', 'codex', b'88 ' + app.encode() + b'/Contents/MacOS/ChatGPT\n', None, [app]),
        ('updater-with-target-argument', 'codex', b'88 /tmp/Updater.app/Contents/MacOS/Updater ' + app.encode() + b'\n', None, [app]),
        ('sparkle-helper', 'codex', b'88 /tmp/com.openai.codex/org.sparkle-project.Sparkle/Updater.app/helper\n', None, [app]),
        ('python-source-in-command-is-not-sparkle', 'codex', ('25 /usr/bin/python3 -c ' + src_words.replace(app, '/elsewhere') + '\n').encode(), b'25 /usr/bin/python3\n', [app]),
        ('python-source-mentions-target', 'codex', ('25 /usr/bin/python3 -c ' + src_words + '\n').encode(), b'25 /usr/bin/python3\n', [app]),
        ('comm-has-bundle-but-no-updater-word', 'codex', b'9 x\n', b'9 /x/com.openai.codex/helper\n', [app]),
        ('comm-has-updater-word-but-no-bundle', 'codex', b'9 x\n', b'9 /x/Sparkle/Autoupdate\n', [app]),
        ('comm-autoupdate', 'codex', b'9 x\n', b'9 /x/com.openai.codex/Autoupdate\n', [app]),
        ('comm-updater-app-dot-is-literal', 'codex', b'9 x\n', b'9 /x/com.openai.codex/UpdaterXapp\n', [app]),
        ('claude-shipit', 'claude', b'9 x\n', b'9 /x/com.anthropic.claudefordesktop.ShipIt/ShipIt\n', ['/Applications/Claude.app']),
        ('claude-squirrel', 'claude', b'9 x\n', b'9 /x/com.anthropic.claudefordesktop/Squirrel\n', ['/Applications/Claude.app']),
        ('claude-sparkle-is-not-its-updater', 'claude', b'9 x\n', b'9 /x/com.anthropic.claudefordesktop/Sparkle\n', ['/Applications/Claude.app']),
        ('pid-only-in-command-list', 'codex', b'9 /x/com.openai.codex/Sparkle\n', b'8 /x/com.openai.codex/Sparkle\n', [app]),
        ('empty-output', 'codex', b'', None, [app]), ('whitespace-output', 'codex', b' \n\n', None, [app]), ('comm-empty', 'codex', b'1 x\n', b'', [app]),
        ('unparsable-line', 'codex', b'1 /sbin/launchd\nnot a pid\n', None, [app]), ('unparsable-comm-line', 'codex', b'1 x\n', b'1 x\n?? y\n', [app]),
        ('line-broken-by-form-feed', 'codex', b'1 /bin/sh -c a\x0cb\n', None, [app]), ('line-broken-by-u2028', 'codex', '1 /bin/echo a b\n'.encode(), None, [app]),
        ('line-broken-by-x85', 'codex', '1 /bin/echo a\x85b\n'.encode(), None, [app]), ('crlf', 'codex', b'1 a\r\n2 b\r\n', None, [app]),
        ('not-utf8', 'codex', b'1 /bin/\xff\xfe\n2 ' + app.encode() + b'\xff\n', None, [app]), ('pid-then-nothing', 'codex', b'1 a\n2\n', None, [app]),
        ('pid-then-spaces', 'codex', b'1 a\n2   \n', None, [app]), ('several-paths-second-matches', 'codex', b'5 /w/previous.app/Contents/MacOS/x\n', None, [app, '/w/previous.app', '/w/ChatGPT.app']),
        ('no-paths', 'codex', b'5 ' + app.encode() + b'\n', None, []), ('path-not-a-string', 'codex', b'5 x\n', None, [5]), ('path-none-after-match', 'codex', b'5 ' + app.encode() + b'\n', None, [app, None]),
        ('path-none-before-match', 'codex', b'5 ' + app.encode() + b'\n', None, [None, app]), ('path-not-a-string-but-comm-unparsable', 'codex', b'5 x\n', b'bad\n', [5]),
        ('blank-line-in-the-middle', 'codex', b'1 a\n\n2 b\n', None, [app]), ('duplicate-pid-last-comm-wins', 'codex', b'7 x\n', b'7 /a\n7 /x/com.openai.codex/Sparkle\n', [app]),
    ]

    def no_processes(client, command, comm, paths):
        ns['CU'] = clients[client]
        calls = []

        def run(args, folder, timeout=30, env=None):
            calls.append([list(args), folder, timeout])
            return type('Done', (), {'returncode': 0, 'stdout': comm if args[-1] == 'pid=,comm=' and comm is not None else command, 'stderr': b''})()
        ns['cu_run'] = run
        got = outcome(lambda: ns['cu_no_processes'](*paths))
        got['calls'] = calls
        return got
    out['processes'] = [[name, client, b64(command), None if comm is None else b64(comm), paths, no_processes(client, command, comm, paths)] for name, client, command, comm, paths in procs]

    def no_processes_rc(rcs):
        ns['CU'] = clients['codex']
        it = iter(rcs)
        ns['cu_run'] = lambda args, folder, timeout=30, env=None: type('Done', (), {'returncode': next(it), 'stdout': b'1 x\n', 'stderr': b''})()
        return outcome(lambda: ns['cu_no_processes'](app))
    out['processes_rc'] = [[rcs, no_processes_rc(rcs)] for rcs in ([0, 0], [1, 0], [0, 1], [1, 1])]

    # 解压
    out['extract'] = extract_vectors(ns, clients)
    return out


def extract_vectors(ns, clients):
    A = 'ChatGPT.app'
    ok_link = make_zip([(A + '/Contents/Frameworks/F.framework/Versions/A/file', b'ok', REG, ST), (A + '/Contents/Frameworks/F.framework/Versions/Current', b'A', LNK, ST)])
    cases = [
        ('internal-framework-link', 'codex', ok_link),
        ('traversal', 'codex', make_zip([(A + '/../outside', b'x', 0o644, ST)])), ('other-root', 'codex', make_zip([('Other.app/file', b'x', 0o644, ST)])),
        ('case-duplicate', 'codex', make_zip([(A + '/A', b'x', 0o644, ST), (A + '/a', b'y', 0o644, ST)])),
        ('unicode-duplicate', 'codex', make_zip([(A + '/é', b'x', 0o644, ST), (A + '/é', b'y', 0o644, ST)])),
        ('external-link', 'codex', make_zip([(A + '/link', b'../../outside', stat.S_IFLNK | 0o644, ST)])), ('fifo', 'codex', make_zip([(A + '/fifo', b'', stat.S_IFIFO | 0o644, ST)])),
        ('claude-needs-the-claude-root', 'claude', make_zip([(A + '/Contents/x', b'x', REG, ST)])),
        ('claude-root-ok', 'claude', make_zip([('Claude.app/Contents/x', b'x', REG, ST)])),
        ('full-app', 'codex', app_zip('codex', '2.0', '200')),
        ('modes', 'codex', make_zip([(A + '/', b'', DIR, ST), (A + '/plain', b'p', REG, ST), (A + '/exec', b'e', stat.S_IFREG | 0o755, DF), (A + '/group-exec', b'g', stat.S_IFREG | 0o610, ST),
                                    (A + '/no-type-exec', b'n', 0o700, ST), (A + '/no-type', b'n', 0, ST), (A + '/setuid', b's', stat.S_IFREG | 0o4755, ST), (A + '/dir-by-mode', b'', DIR, ST),
                                    (A + '/dir-by-slash/', b'', REG, ST), (A + '/dir-with-data/', b'ignored', DIR, ST), (A + '/deep/er/file', b'd' * 5000, REG, DF), (A + '/empty', b'', REG, DF)])),
        ('links', 'codex', make_zip([(A + '/a/target', b't', REG, ST), (A + '/a/same-dir', b'target', LNK, ST), (A + '/b/up', b'../a/target', LNK, ST), (A + '/to-dir', b'a', LNK, ST),
                                    (A + '/dot', b'./a/./target', LNK, ST), (A + '/chain', b'b/up', LNK, ST), (A + '/link-with-slash/', b'a', LNK, ST), (A + '/new/dir/for/link', b'../../../a', LNK, ST)])),
        ('link-dangling', 'codex', make_zip([(A + '/link', b'nowhere', LNK, ST)])), ('link-to-self', 'codex', make_zip([(A + '/link', b'link', LNK, ST)])),
        ('link-loop', 'codex', make_zip([(A + '/a', b'b', LNK, ST), (A + '/b', b'a', LNK, ST)])), ('link-to-root', 'codex', make_zip([(A + '/link', b'.', LNK, ST)])),
        ('link-to-parent-of-root', 'codex', make_zip([(A + '/link', b'..', LNK, ST)])), ('link-up-and-back', 'codex', make_zip([(A + '/x', b'x', REG, ST), (A + '/link', b'../' + A.encode() + b'/x', LNK, ST)])),
        ('link-absolute', 'codex', make_zip([(A + '/link', b'/etc/passwd', LNK, ST)])), ('link-empty', 'codex', make_zip([(A + '/link', b'', LNK, ST)])),
        ('link-with-colon', 'codex', make_zip([(A + '/link', b'a:b', LNK, ST)])), ('link-with-backslash', 'codex', make_zip([(A + '/link', b'a\\b', LNK, ST)])),
        ('link-with-newline', 'codex', make_zip([(A + '/link', b'a\nb', LNK, ST)])), ('link-not-utf8', 'codex', make_zip([(A + '/link', b'\xff', LNK, ST)])),
        ('link-4096', 'codex', make_zip([(A + '/link', b'a' * 4096, LNK, ST)])), ('link-4097', 'codex', make_zip([(A + '/link', b'a' * 4097, LNK, ST)])),
        ('link-deflated', 'codex', make_zip([(A + '/x', b'x', REG, ST), (A + '/link', b'x', LNK, DF)])),
        ('link-escapes-through-another-link', 'codex', make_zip([(A + '/up', b'..', LNK, ST), (A + '/out', b'up/elsewhere', LNK, ST)])),
        ('link-to-case-variant', 'codex', make_zip([(A + '/Target', b't', REG, ST), (A + '/link', b'target', LNK, ST)])),
        ('write-through-link', 'codex', make_zip([(A + '/d', b'x', LNK, ST), (A + '/d/file', b'x', REG, ST)])),
        ('write-through-link-case-variant', 'codex', make_zip([(A + '/D', b'x', LNK, ST), (A + '/d/file', b'x', REG, ST)])),
        ('link-listed-after-its-child', 'codex', make_zip([(A + '/d/file', b'x', REG, ST), (A + '/d', b'x', LNK, ST)])),
        ('root-is-a-link', 'codex', make_zip([(A, b'x', LNK, ST)])), ('root-is-a-file', 'codex', make_zip([(A, b'x', REG, ST)])), ('root-only', 'codex', make_zip([(A + '/', b'', DIR, ST)])),
        ('empty-archive', 'codex', make_zip([])), ('not-a-zip', 'codex', b'hello'), ('absolute', 'codex', make_zip([('/' + A + '/x', b'x', REG, ST)])),
        ('dot-segment', 'codex', make_zip([(A + '/./x', b'x', REG, ST)])), ('double-slash', 'codex', make_zip([(A + '//x', b'x', REG, ST)])), ('trailing-slashes', 'codex', make_zip([(A + '/x///', b'', DIR, ST)])),
        ('backslash', 'codex', make_zip([(A + '/a\\b', b'x', REG, ST)])), ('colon', 'codex', make_zip([(A + '/a:b', b'x', REG, ST)])), ('control-character', 'codex', make_zip([(A + '/a\x01b', b'x', REG, ST)])),
        ('del-character', 'codex', make_zip([(A + '/a\x7fb', b'x', REG, ST)])), ('sharp-s-duplicate', 'codex', make_zip([(A + '/straße', b'x', REG, ST), (A + '/STRASSE', b'y', REG, ST)])),
        ('kelvin-duplicate', 'codex', make_zip([(A + '/K', b'x', REG, ST), (A + '/K', b'y', REG, ST)])), ('dir-and-file-duplicate', 'codex', make_zip([(A + '/a/', b'', DIR, ST), (A + '/a', b'x', REG, ST)])),
        ('root-case-variant', 'codex', make_zip([('chatgpt.app/x', b'x', REG, ST)])), ('file-under-a-file', 'codex', make_zip([(A + '/a', b'x', REG, ST), (A + '/a/b', b'y', REG, ST)])),
        ('file-over-a-directory', 'codex', make_zip([(A + '/a/b', b'y', REG, ST), (A + '/a', b'x', REG, ST)])), ('socket', 'codex', make_zip([(A + '/s', b'', stat.S_IFSOCK | 0o644, ST)])),
        ('block-device', 'codex', make_zip([(A + '/s', b'', stat.S_IFBLK | 0o644, ST)])), ('encrypted-flag', 'codex', make_zip([(A + '/x', b'x', REG, ST)], flags=1)),
        ('unicode-names', 'codex', make_zip([(A + '/中文/名字.txt', b'x', REG, ST), (A + '/é', b'y', REG, ST), (A + '/sp ace', b'z', REG, ST)])),
        ('chinese-root-mismatch', 'codex', make_zip([('ChatGPT。app/x', b'x', REG, ST)])),
        ('bad-crc', 'codex', make_zip([(A + '/x', b'hello world', REG, ST)]).replace(b'hello world', b'hello World', 1)),
        ('bad-crc-in-link', 'codex', make_zip([(A + '/target', b't', REG, ST), (A + '/link', b'target', LNK, ST)]).replace(b'target' + b'PK\x01\x02', b'tarGet' + b'PK\x01\x02', 1)),
        ('cli-client-has-no-app', 'claude-cli', make_zip([(A + '/x', b'x', REG, ST)])),
        # 压缩的数据坏了（开头就是不存在的块类型）：zlib.error 不在 cu_extract 接的那几类里，原来是没人接的异常
        ('deflate-invalid-block', 'codex', bad_deflate(make_zip([(A + '/x', b'hello world ' * 40, REG, DF), (A + '/y', b'y', REG, ST)]))),
        ('deflate-invalid-block-in-link', 'codex', bad_deflate(make_zip([(A + '/link', b'target', LNK, DF), (A + '/target', b't', REG, ST)]))),
        ('too-many-entries', 'codex', None),
    ]
    out = []
    temp = os.path.realpath(tempfile.mkdtemp(prefix='cca-clients-extract-'))
    try:
        for i, (name, client, data) in enumerate(cases):
            if data is None:
                data = make_zip([(A + '/f%d' % n, b'', REG, ST) for n in range(100001)])
            archive, folder = os.path.join(temp, '%d.zip' % i), os.path.join(temp, 'out-%d' % i)
            with open(archive, 'wb') as f:
                f.write(data)
            os.mkdir(folder)
            ns['CU'] = clients[client]
            got = outcome(lambda: ns['cu_extract'](archive, folder))
            if 'ok' in got:
                got['ok'] = os.path.relpath(got['ok'], folder)
            tree = {}
            for d, dirs, files in os.walk(folder):
                for n in dirs + files:
                    p = os.path.join(d, n)
                    st = os.lstat(p)
                    rel = os.path.relpath(p, folder)
                    if stat.S_ISLNK(st.st_mode):
                        tree[rel] = 'link -> ' + os.readlink(p)
                    elif stat.S_ISDIR(st.st_mode):
                        tree[rel] = 'dir %o' % (st.st_mode & 0o777)
                    else:
                        with open(p, 'rb') as f:
                            tree[rel] = 'file %o %s' % (st.st_mode & 0o7777, hashlib.sha256(f.read()).hexdigest())
            # 出了错的：解到一半留下的东西不比（调用方整个目录都删）；条目很多的那个也不记
            out.append([name, client, b64(data) if len(data) < 200000 else None, got, tree if 'ok' in got else None])
    finally:
        shutil.rmtree(temp, ignore_errors=True)
    return out
