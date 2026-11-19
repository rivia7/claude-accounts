"""客户端统一更新（cli/src/cmd/clients.rs）：client-updates status / check / mode / auto-install / plan / install。

**这一块会装软件，沙箱只换了 HOME。** 桌面客户端、终端工具在哪由环境变量定（CCA_CLIENT_CODEX_APP、CCA_CLIENT_CLAUDE_APP、
CCA_CODEX_APP、CCA_CLAUDE_APP、CCA_CLI_CLAUDE、CCA_CLI_CODEX）；漏设就会去找这台机器 /Applications 里真的 App。所以这里的用例
一律经 ccase 登记：六个变量每个都显式给出，值只能是空串（没装）或者沙箱里的路径（%HOME%/ 开头），不是就当场报错、不登记。

对拍够得着的：
- status、mode、auto-install、plan 的全部；
- check / install 里终端工具的全部（假的 claude / codex 脚本）——除了「问官方最新版本」那一步：原来写死 `/usr/bin/curl`，
  PATH 上的假 curl 换不掉它，会真的联网。所以这里没有一个用例走到 cu_download：要么还没到检查的时间（state.json 里记着刚查过），
  要么在它之前就停了（读不出版本）；
- 桌面客户端的 check / install 一直到验签名那一步：`/usr/bin/codesign --verify` 是真的跑在沙箱里的假 App 上（只读），假 App 没签名，
  停在「代码签名或系统安全评估未通过」。之前的每一步（快照、暂存的包核对、解压、事务记录的恢复）都走得到；
- `/bin/ps`（核对有没有进程在用这个 App）是真的：列的是这台机器的进程，只读。

够不着的（下载、版本列表、验签名以后的替换）在 cli/src/cmd/clients/tests.rs：拿 PY_SRC 把同样的场景跑出来的结果重放。

暂存好的更新要记着 App 的快照（设备号、inode、时间戳），拷出来的沙箱里每次不一样：这种夹具放在 before 里，拿 PY_SRC 自己的
cu_snapshot 现拍（两边各拍一遍）。"""
import fcntl
import hashlib
import io
import json
import os
import plistlib
import signal
import stat
import subprocess
import sys
import time
import zipfile

from run import case
from cases.presence import src

SLOW = os.environ.get('CCA_PYDIFF_SLOW') == '1'
NOW = time.time()
TOKEN = 'a1' * 32
OTHER = 'b2' * 32
CODEX_APP, CLAUDE_APP = 'Applications/ChatGPT.app', 'Applications/Claude.app'
CLAUDE_BIN, CODEX_BIN = '.local/share/claude/versions/2.0.0', '.codex/packages/standalone/current/bin/codex'
BUNDLE = {'codex': 'com.openai.codex', 'claude': 'com.anthropic.claudefordesktop'}
EXE = {'codex': 'ChatGPT', 'claude': 'Claude'}
APPS = {'codex': CODEX_APP, 'claude': CLAUDE_APP}
# 六个「目标在哪」的变量：默认都是空串（没装）
NOWHERE = {'CCA_CLIENT_CODEX_APP': '', 'CCA_CLIENT_CLAUDE_APP': '', 'CCA_CODEX_APP': '', 'CCA_CLAUDE_APP': '', 'CCA_CLI_CLAUDE': '', 'CCA_CLI_CODEX': '',
           'CCA_CLIENT_VERIFIER': ''}
MASK = [
    (r'\b1[789]\d{8}\.\d+', '<T>'),                                              # 现在的时间戳（generated_epoch、刚记下的 checked_epoch、它加六小时）
    (r'\b(check|update|cca-cli)-[a-z0-9_]{8}\b', r'\1-<R>'),                     # 临时目录的名字
    (r'\.cca-client-install-[a-z0-9_]{8}\b', '.cca-client-install-<R>'),
    (r'\[(\s*\d[\d.e+]*,){4}\s*\d[\d.e+]*\s*\]', '[<ID>]'),                      # 文件的身份：设备号、inode、大小、两个时间（有个用例把它们记成小数）
    (r'"(device|inode|modified_ns|changed_ns)": \d+', r'"\1": <N>'),
]


def safe(name, env, relative=()):
    """这个用例的环境里，每种客户端的位置都是空的或者在沙箱里。relative：点了名的变量可以是相对路径（命令的当前目录就是沙箱的 HOME）"""
    def inside(k):
        v = env[k]
        return v == '' or v.startswith('%HOME%/') or (k in relative and not v.startswith('/') and '..' not in v.split('/'))
    for client, app in (('CCA_CLIENT_CODEX_APP', 'CCA_CODEX_APP'), ('CCA_CLIENT_CLAUDE_APP', 'CCA_CLAUDE_APP')):
        if env.get(client) is None:   # 没设 CCA_CLIENT_…：原来接着看 CCA_…_APP，它是空的就去翻 /Applications
            assert env.get(app) and env[app].startswith('%HOME%/'), '%s：%s 没设时 %s 必须指到沙箱里' % (name, client, app)
        else:
            assert inside(client), '%s：%s 不在沙箱里' % (name, client)
        assert env.get(app) is None or inside(app), '%s：%s 不在沙箱里' % (name, app)
    for k in ('CCA_CLI_CLAUDE', 'CCA_CLI_CODEX', 'CCA_CLIENT_VERIFIER'):
        assert env.get(k) is not None and inside(k), '%s：%s 不在沙箱里' % (name, k)


def ccase(name, argv, env=None, relative=(), **kw):
    full = dict(NOWHERE)
    full.update(env or {})
    safe(name, full, relative)
    kw.setdefault('mask', MASK)
    return case('clients/' + name, ['client-updates'] + list(argv), env=full, **kw)


CODEX_ENV = {'CCA_CLIENT_CODEX_APP': '%HOME%/' + CODEX_APP}
CLAUDE_ENV = {'CCA_CLIENT_CLAUDE_APP': '%HOME%/' + CLAUDE_APP}
BOTH_ENV = dict(CODEX_ENV, **CLAUDE_ENV)
CLI_ENV = {'CCA_CLI_CLAUDE': '%HOME%/' + CLAUDE_BIN}
CODEX_CLI_ENV = {'CCA_CLI_CODEX': '%HOME%/' + CODEX_BIN}


# --- 夹具 -------------------------------------------------------------------------------------------------------------
def info_plist(client, version, build, exe=None, minimum='13.0', **more):
    d = {'CFBundleIdentifier': BUNDLE[client], 'CFBundleExecutable': exe or EXE[client], 'CFBundleShortVersionString': version, 'CFBundleVersion': build,
         'LSMinimumSystemVersion': minimum}
    d.update(more)
    return plistlib.dumps({k: v for k, v in d.items() if v is not None})


def make_app(b, client='codex', version='1.0', build=None, rel=None, plist=None, exe=None):
    """假的桌面客户端：四个关键文件，没有签名"""
    rel = rel or APPS[client]
    build = build or ('100' if client == 'codex' else version)
    exe = exe or EXE[client]
    b.write(rel + '/Contents/Info.plist', plist if plist is not None else info_plist(client, version, build, exe), 0o644)
    b.write(rel + '/Contents/MacOS/' + exe, b'fake binary ' + build.encode(), 0o755)
    b.write(rel + '/Contents/Resources/app.asar', b'fake app ' + build.encode(), 0o644)
    b.write(rel + '/Contents/_CodeSignature/CodeResources', b'fake seal ' + build.encode(), 0o644)
    return b.path(rel)


def zip_bytes(entries):
    """entries：[(名字, 内容, 权限位连同类型, 压缩方法)]。时间固定，字节是定的"""
    out = io.BytesIO()
    with zipfile.ZipFile(out, 'w') as z:
        for name, data, mode, method in entries:
            zi = zipfile.ZipInfo(name, (2026, 1, 2, 3, 4, 6))
            zi.create_system = 3
            zi.external_attr = mode << 16
            zi.compress_type = method
            z.writestr(zi, data)
    return out.getvalue()


def app_zip(client='codex', version='2.0', build=None, extra=()):
    """一个像样的安装包：根目录是这个客户端的 .app"""
    build = build or ('200' if client == 'codex' else version)
    root, exe = APPS[client].split('/')[-1], EXE[client]
    return zip_bytes([
        (root + '/', b'', stat.S_IFDIR | 0o755, zipfile.ZIP_STORED),
        (root + '/Contents/Info.plist', info_plist(client, version, build), stat.S_IFREG | 0o644, zipfile.ZIP_DEFLATED),
        (root + '/Contents/MacOS/' + exe, b'fake binary ' + build.encode(), stat.S_IFREG | 0o755, zipfile.ZIP_DEFLATED),
        (root + '/Contents/Resources/app.asar', b'fake app ' + build.encode() * 2000, stat.S_IFREG | 0o644, zipfile.ZIP_DEFLATED),
        (root + '/Contents/_CodeSignature/CodeResources', b'fake seal ' + build.encode(), 0, zipfile.ZIP_STORED),
        (root + '/Contents/Frameworks/F.framework/Versions/A/F', b'framework', stat.S_IFREG | 0o755, zipfile.ZIP_DEFLATED),
        (root + '/Contents/Frameworks/F.framework/Versions/Current', b'A', stat.S_IFLNK | 0o777, zipfile.ZIP_STORED),
    ] + list(extra))


def put_state(b, client, state, mode=0o600, v=1):
    d = dict(state)
    if v is not None:
        d['v'] = v
    os.makedirs(b.path('.cca/client-updates/' + client), mode=0o700, exist_ok=True)
    for p in ('.cca', '.cca/client-updates', '.cca/client-updates/' + client):
        os.chmod(b.path(p), 0o700)
    return b.json('.cca/client-updates/%s/state.json' % client, d, mode)


def updater_dirs(b, *clients):
    for c in clients or ('',):
        os.makedirs(b.path('.cca/client-updates/' + c), mode=0o700, exist_ok=True)
    for p in ['.cca', '.cca/client-updates'] + ['.cca/client-updates/' + c for c in clients]:
        os.chmod(b.path(p), 0o700)


def snapshot(b, client):
    """PY_SRC 的 cu_snapshot 给这个沙箱里的 App 现拍的快照"""
    ns = src()
    ns['CU'] = ns['CU_CLAUDE' if client == 'claude' else 'CU_CODEX']
    return ns['cu_snapshot'](b.path(APPS[client]))


def staged(client='codex', version='2.0', build=None, status='staged', package=True, edit=None, token=TOKEN, sha256=False, **top):
    """before：记一份暂存好的更新（target 是现拍的快照，archive_identity 是沙箱里那个 package.zip 现在的身份）。
    edit(state, b)：写之前再改一改"""
    build = build or ('200' if client == 'codex' else version)

    def before(b):
        target = snapshot(b, client)
        archive = b.path('.cca/client-updates/%s/package.zip' % client)
        metadata = {'version': version, 'build': build, 'minimum_system': '13.0' if client == 'codex' else '', 'arch': 'arm64',
                    'url': 'https://example.invalid/x.zip', 'length': 0}
        stage = {'metadata': metadata, 'target': target, 'package_sha256': '', 'package': {'version': version, 'build': build, 'hashes': {}}, 'token': token}
        if package and os.path.lexists(archive):
            with open(archive, 'rb') as f:
                data = f.read()
            metadata['length'] = len(data)
            stage['package_sha256'] = hashlib.sha256(data).hexdigest()
            stage['archive_identity'] = src()['cu_file_id'](archive)
            if client == 'claude' or sha256:
                metadata['sha256'] = stage['package_sha256']
            else:
                metadata['signature'] = 'WFhY' * 22
        state = {'checked_epoch': NOW - 60, 'metadata': metadata, 'stage': stage, 'status': status, 'message': '官方更新已暂存，等待确认安装'}
        state.update(top)
        if edit:
            edit(state, b)
        put_state(b, client, state)
    return before


def package(b, client='codex', data=None, mode=0o600, **kw):
    updater_dirs(b, client)
    return b.write('.cca/client-updates/%s/package.zip' % client, app_zip(client, **kw) if data is None else data, mode)


TOOL = r'''# 假的 claude / codex：版本记在 ~/tool/version；update 把 ~/tool/next 换上去，照 ~/tool/update.rc 退出
log="$HOME/tool/log"
printf '%s\n' "$0 $*" >> "$log"
case "$1" in
--version)
	[ -f "$HOME/tool/version.rc" ] && exit "$(cat "$HOME/tool/version.rc")"
	cat "$HOME/tool/version" ;;
update)
	env | LC_ALL=C sort >> "$log"
	[ -n "$CLAUDE_CONFIG_DIR" ] && [ -f "$CLAUDE_CONFIG_DIR/settings.json" ] && cat "$CLAUDE_CONFIG_DIR/settings.json" >> "$log"
	[ -f "$HOME/tool/next" ] && cp "$HOME/tool/next" "$HOME/tool/version"
	[ -f "$HOME/tool/update.out" ] && cat "$HOME/tool/update.out"
	[ -f "$HOME/tool/update.err" ] && cat "$HOME/tool/update.err" >&2
	[ -f "$HOME/tool/update.rc" ] && exit "$(cat "$HOME/tool/update.rc")"
	exit 0 ;;
esac
'''


def make_tool(b, rel=CLAUDE_BIN, version='2.0.0 (Claude Code)\n', nxt=None, script=TOOL):
    """假的终端工具（一个 /bin/sh 脚本）"""
    b.write(rel, '#!/bin/sh\n' + script, 0o755)
    b.write('tool/log', '', 0o644)
    if version is not None:
        b.write('tool/version', version, 0o644)
    if nxt is not None:
        b.write('tool/next', nxt, 0o644)
    return b.path(rel)


def cli_state(b, client='claude-cli', rel=None, latest='2.1.0', installed='2.0.0', method='self', status='available', token=TOKEN, age=60, stage=True, **top):
    """终端工具上次检查记下的：有新版本 latest，官方安装器装的给了安装凭据"""
    rel = rel or (CLAUDE_BIN if client == 'claude-cli' else CODEX_BIN)
    state = {'checked_epoch': NOW - age, 'metadata': {'version': latest}, 'status': status, 'message': '可以直接更新到 %s' % latest}
    if stage:
        state['stage'] = {'token': token, 'method': method, 'metadata': {'version': latest}, 'target': {'path': b.path(rel), 'version': installed}}
    state.update(top)
    return put_state(b, client, state)


def hold(rel):
    """before：另一个进程拿着这把锁"""
    def before(b):
        os.makedirs(os.path.dirname(b.path(rel)), mode=0o700, exist_ok=True)
        fd = os.open(b.path(rel), os.O_RDWR | os.O_CREAT, 0o600)
        fcntl.flock(fd, fcntl.LOCK_EX)
        return lambda: os.close(fd)
    return before


def both(*steps):
    """几个 before 接着做；收尾的倒着做"""
    def before(b):
        done = [s(b) for s in steps]
        return lambda: [d() for d in reversed(done) if callable(d)]
    return before


def running(rel):
    """before：起一个命令行里带着这个路径的进程（原来靠 ps 看有没有进程在用这个 App）；收尾时按进程号杀掉。
    不能用 `sh -c 'sleep …' 路径`：sh 会直接换成 sleep，命令行里就没有这个路径了"""
    def before(b):
        p = subprocess.Popen([sys.executable, '-I', '-c', 'import time; time.sleep(120)', b.path(rel)], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL)
        time.sleep(0.3)

        def stop():
            p.kill()
            p.wait()
        return stop
    return before


# --- 认不出的操作、参数个数 --------------------------------------------------------------------------------------------
@ccase('usage/unknown-action', ['frobnicate'])
def _(b):
    pass


@ccase('usage/empty-action', [''])
def _(b):
    pass


@ccase('usage/too-many-arguments', ['status', 'x', 'y'])
def _(b):
    pass


@ccase('usage/status-ignores-its-argument', ['status', 'whatever'])
def _(b):
    pass


# --- status：什么都没装、配置的各种样子 --------------------------------------------------------------------------------
@ccase('status/nothing-installed', [])
def _(b):
    pass


@ccase('status/nothing-installed-explicit', ['status'])
def _(b):
    b.cfg({'version': 2})


for name, cfg in [('mode-off', {'client_updates': 'off'}), ('mode-auto', {'client_updates': 'auto'}), ('mode-garbage', {'client_updates': 'sometimes'}),
                  ('mode-number', {'client_updates': 0}), ('auto-install-on', {'client_auto_install': True}), ('auto-install-1', {'client_auto_install': 1}),
                  ('auto-install-string', {'client_auto_install': 'true'}), ('auto-install-on-but-mode-off', {'client_auto_install': True, 'client_updates': 'off'}),
                  ('auto-install-false', {'client_auto_install': False})]:
    @ccase('status/config/' + name, ['status'], env=CODEX_ENV)
    def _(b, cfg=cfg):
        b.cfg(cfg)
        make_app(b)


for name, text in [('not-json', '{oops'), ('list', '[1, 2]'), ('null', 'null'), ('string', '"off"'), ('empty', '')]:
    @ccase('status/config-file/' + name, ['status'], env=CODEX_ENV)
    def _(b, text=text):
        b.write('.cca/config.json', text)
        make_app(b)


# --- status：桌面客户端在哪、长什么样 ----------------------------------------------------------------------------------
@ccase('status/desktop/codex', ['status'], env=CODEX_ENV)
def _(b):
    make_app(b)


@ccase('status/desktop/claude', ['status'], env=CLAUDE_ENV)
def _(b):
    make_app(b, 'claude', '4.1.0')


@ccase('status/desktop/both', ['status'], env=BOTH_ENV)
def _(b):
    make_app(b)
    make_app(b, 'claude', '4.1.0')


@ccase('status/desktop/from-app-env', ['status'],   # 没有 CCA_CLIENT_…：照 CCA_CODEX_APP / CCA_CLAUDE_APP（都指在沙箱里）
       env={'CCA_CLIENT_CODEX_APP': None, 'CCA_CLIENT_CLAUDE_APP': None, 'CCA_CODEX_APP': '%HOME%/' + CODEX_APP, 'CCA_CLAUDE_APP': '%HOME%/' + CLAUDE_APP})
def _(b):
    make_app(b)
    make_app(b, 'claude', '4.1.0')


@ccase('status/desktop/client-env-empty-wins-over-app-env', ['status'], env={'CCA_CODEX_APP': '%HOME%/' + CODEX_APP})   # 空的显式路径就是没装
def _(b):
    make_app(b)


@ccase('status/desktop/path-missing', ['status'], env=CODEX_ENV)
def _(b):
    pass


@ccase('status/desktop/path-is-a-file', ['status'], env=CODEX_ENV)
def _(b):
    b.write(CODEX_APP, 'not an app')


@ccase('status/desktop/path-is-a-symlink', ['status'], env={'CCA_CLIENT_CODEX_APP': '%HOME%/Applications/Link.app'})
def _(b):
    make_app(b)
    os.symlink(b.path(CODEX_APP), b.path('Applications/Link.app'))


@ccase('status/desktop/path-through-a-symlinked-parent', ['status'], env={'CCA_CLIENT_CODEX_APP': '%HOME%/Apps/ChatGPT.app'})   # 父目录是链接：realpath 以后照常
def _(b):
    make_app(b)
    os.symlink(b.path('Applications'), b.path('Apps'))


@ccase('status/desktop/path-relative', ['status'], env={'CCA_CLIENT_CODEX_APP': CODEX_APP}, relative=('CCA_CLIENT_CODEX_APP',))   # 相对路径（相对沙箱的 HOME）：不认
def _(b):
    make_app(b)


@ccase('status/desktop/path-with-trailing-slash', ['status'], env={'CCA_CLIENT_CODEX_APP': '%HOME%/' + CODEX_APP + '/'})
def _(b):
    make_app(b)


@ccase('status/desktop/chinese-path', ['status'], env={'CCA_CLIENT_CODEX_APP': '%HOME%/应用 程序/ChatGPT.app'})
def _(b):
    make_app(b, rel='应用 程序/ChatGPT.app')


PLISTS = [
    ('binary-plist', plistlib.dumps({'CFBundleIdentifier': BUNDLE['codex'], 'CFBundleExecutable': 'ChatGPT', 'CFBundleShortVersionString': '1.0', 'CFBundleVersion': '100'},
                                    fmt=plistlib.FMT_BINARY)),
    ('wrong-bundle', info_plist('claude', '1.0', '100', 'ChatGPT')),
    ('no-bundle', info_plist('codex', '1.0', '100', CFBundleIdentifier=None)),
    ('no-executable', info_plist('codex', '1.0', '100', CFBundleExecutable=None)),
    ('executable-empty', info_plist('codex', '1.0', '100', CFBundleExecutable='')),
    ('executable-with-slash', info_plist('codex', '1.0', '100', CFBundleExecutable='../x')),
    ('executable-dot', info_plist('codex', '1.0', '100', CFBundleExecutable='..')),
    ('executable-number', info_plist('codex', '1.0', '100', CFBundleExecutable=5)),
    ('version-one-part', info_plist('codex', '1', '100')),
    ('version-seven-parts', info_plist('codex', '1.2.3.4.5.6.7', '100')),
    ('version-six-parts', info_plist('codex', '1.2.3.4.5.6', '100')),
    ('version-letters', info_plist('codex', '1.0b', '100')),
    ('version-trailing-newline', info_plist('codex', '1.0\n', '100')),
    ('version-fullwidth-digits', info_plist('codex', '１.２', '100')),
    ('version-number', info_plist('codex', '1.0', '100', CFBundleShortVersionString=1.5)),
    ('no-version', info_plist('codex', '1.0', '100', CFBundleShortVersionString=None)),
    ('build-dotted', info_plist('codex', '1.0', '1.0.0')),
    ('build-thirteen-digits', info_plist('codex', '1.0', '1234567890123')),
    ('build-twelve-digits', info_plist('codex', '1.0', '123456789012')),
    ('build-empty', info_plist('codex', '1.0', '')),
    ('build-arabic-digits', info_plist('codex', '1.0', '١٢٣')),
    ('no-build', info_plist('codex', '1.0', '100', CFBundleVersion=None)),
    ('root-is-array', plistlib.dumps(['a', 'b'])),
    ('root-is-string', plistlib.dumps('x')),
    ('xml-malformed', b'<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>com.openai.codex</plist>'),
    ('not-a-plist', b'hello'),
    ('empty-file', b''),
]
for name, raw in PLISTS:
    @ccase('status/desktop/plist/' + name, ['status'], env=CODEX_ENV)
    def _(b, raw=raw):
        make_app(b, plist=raw)


@ccase('status/desktop/plist/claude-build-integer', ['status'], env=CLAUDE_ENV)   # Claude 的构建号要带点
def _(b):
    make_app(b, 'claude', plist=info_plist('claude', '4.1.0', '410'))


@ccase('status/desktop/plist/missing', ['status'], env=CODEX_ENV)
def _(b):
    make_app(b)
    os.unlink(b.path(CODEX_APP + '/Contents/Info.plist'))


@ccase('status/desktop/plist/is-a-directory', ['status'], env=CODEX_ENV)
def _(b):
    make_app(b)
    os.unlink(b.path(CODEX_APP + '/Contents/Info.plist'))
    os.mkdir(b.path(CODEX_APP + '/Contents/Info.plist'))


def unreadable(rel, back=0o644):
    """before：这个文件谁也读不了（夹具里不能直接放这样的文件：对拍要把沙箱拷一份）；收尾时改回来"""
    def before(b):
        os.chmod(b.path(rel), 0)
        return lambda: os.chmod(b.path(rel), back)
    return before


@ccase('status/desktop/plist/unreadable', ['status'], env=CODEX_ENV, before=unreadable(CODEX_APP + '/Contents/Info.plist'))
def _(b):
    make_app(b)


for name, rel in [('executable', 'Contents/MacOS/ChatGPT'), ('asar', 'Contents/Resources/app.asar'), ('seal', 'Contents/_CodeSignature/CodeResources')]:
    @ccase('status/desktop/key-file-missing/' + name, ['status'], env=CODEX_ENV)
    def _(b, rel=rel):
        make_app(b)
        os.unlink(b.path(CODEX_APP + '/' + rel))

    @ccase('status/desktop/key-file-is-a-link/' + name, ['status'], env=CODEX_ENV)
    def _(b, rel=rel):
        make_app(b)
        p = b.path(CODEX_APP + '/' + rel)
        os.rename(p, p + '.real')
        os.symlink(p + '.real', p)


@ccase('status/desktop/key-directory-is-a-link', ['status'], env=CODEX_ENV)
def _(b):
    make_app(b)
    p = b.path(CODEX_APP + '/Contents/Resources')
    os.rename(p, p + '.real')
    os.symlink(p + '.real', p)


@ccase('status/desktop/plist-is-a-link', ['status'], env=CODEX_ENV)
def _(b):
    make_app(b)
    p = b.path(CODEX_APP + '/Contents/Info.plist')
    os.rename(p, p + '.real')
    os.symlink(p + '.real', p)


@ccase('status/desktop/key-file-hard-linked', ['status'], env=CODEX_ENV)   # status 不算摘要：硬链接看不出来
def _(b):
    make_app(b)
    os.link(b.path(CODEX_APP + '/Contents/Resources/app.asar'), b.path('elsewhere'))


# --- status：上次检查记下的状态（state.json） ---------------------------------------------------------------------------
STATES = [
    ('current', {'checked_epoch': 1700000000, 'status': 'current', 'message': '当前客户端已是最新版本'}),
    ('with-metadata', {'checked_epoch': 1700000000.5, 'metadata': {'version': '2.0', 'build': '200'}, 'status': 'current', 'message': 'x'}),
    ('metadata-version-number', {'metadata': {'version': 2.5}, 'status': 'current'}),
    ('metadata-not-dict', {'metadata': ['2.0'], 'status': 'current'}),
    ('no-status-no-message', {'checked_epoch': 5}),
    ('status-null', {'status': None, 'message': None, 'checked_epoch': None}),
    ('status-odd-types', {'status': 5, 'message': ['a'], 'checked_epoch': 0}),
    ('checked-true', {'checked_epoch': True, 'status': 'current'}),
    ('checked-false', {'checked_epoch': False, 'status': 'current'}),
    ('checked-float', {'checked_epoch': 1000.25, 'status': 'current'}),
    ('checked-zero-float', {'checked_epoch': 0.0, 'status': 'current'}),
    ('checked-negative', {'checked_epoch': -5, 'status': 'current'}),
    ('checked-string', {'checked_epoch': 'yesterday', 'status': 'current'}),
    ('checked-list', {'checked_epoch': [1], 'status': 'current'}),
    ('checked-empty-list', {'checked_epoch': [], 'status': 'current'}),
    ('checked-dict', {'checked_epoch': {'a': 1}, 'status': 'current'}),
    ('stage-list', {'stage': ['x'], 'status': 'staged'}),
    ('stage-string', {'stage': 'x', 'status': 'staged'}),
    ('stage-empty-dict', {'stage': {}, 'status': 'staged'}),
    ('stage-no-metadata', {'stage': {'token': TOKEN}, 'status': 'staged'}),
    ('stage-metadata-string', {'stage': {'metadata': 'x', 'token': TOKEN}, 'status': 'staged'}),
    ('stage-metadata-list', {'stage': {'metadata': ['x'], 'token': TOKEN}, 'status': 'staged'}),
    ('stage-metadata-null', {'stage': {'metadata': None, 'token': TOKEN}, 'status': 'staged'}),
    ('stage-metadata-no-build', {'stage': {'metadata': {'version': '2.0'}, 'token': TOKEN}, 'status': 'staged'}),
    ('stage-no-package-file', {'stage': {'metadata': {'version': '2.0', 'build': '200'}, 'token': TOKEN}, 'status': 'staged'}),
    ('retained-recovery-path', {'status': 'needs_recheck', 'message': '已保留原生更新器安装的客户端，请重新检查', 'retained_recovery_path': '/x/.cca-client-install-abc'}),
    ('extra-keys-and-unicode', {'status': 'error', 'message': '无法从官方来源下载更新，请稍后重试', '别的': {'键': [1, 2.5, None, True]}}),
]
for name, state in STATES:
    @ccase('status/state/' + name, ['status'], env=CODEX_ENV)
    def _(b, state=state):
        make_app(b)
        put_state(b, 'codex', state)


for name, v in [('v-missing', None), ('v-2', 2), ('v-true', True), ('v-1.0', 1.0), ('v-string', '1'), ('v-null', 'null')]:
    @ccase('status/state-file/' + name, ['status'], env=CODEX_ENV)
    def _(b, v=v):
        make_app(b)
        put_state(b, 'codex', {'status': 'current', 'message': 'm', 'checked_epoch': 7}, v=None if v in (None, 'null') else v)
        if v == 'null':
            b.write('.cca/client-updates/codex/state.json', '{"status": "current", "v": null}')


for name, text in [('not-json', '{oops'), ('list', '[1]'), ('empty', ''), ('not-utf8', b'\xff\xfe'), ('nested-too-deep', '[' * 1200 + ']' * 1200)]:
    @ccase('status/state-file/' + name, ['status'], env=CODEX_ENV)
    def _(b, text=text):
        make_app(b)
        updater_dirs(b, 'codex')
        b.write('.cca/client-updates/codex/state.json', text)


for name, mode in [('mode-644', 0o644), ('mode-640', 0o640), ('mode-604', 0o604), ('mode-400', 0o400), ('mode-700', 0o700)]:
    @ccase('status/state-file/' + name, ['status'], env=CODEX_ENV)
    def _(b, mode=mode):
        make_app(b)
        put_state(b, 'codex', {'status': 'current', 'message': 'm', 'checked_epoch': 7}, mode=mode)


@ccase('status/state-file/mode-000', ['status'], env=CODEX_ENV, before=unreadable('.cca/client-updates/codex/state.json', 0o600))
def _(b):
    make_app(b)
    put_state(b, 'codex', {'status': 'current', 'message': 'm', 'checked_epoch': 7})


@ccase('status/state-file/symlink', ['status'], env=CODEX_ENV)
def _(b):
    make_app(b)
    put_state(b, 'codex', {'status': 'current', 'message': 'm'})
    os.rename(b.path('.cca/client-updates/codex/state.json'), b.path('.cca/client-updates/codex/real.json'))
    os.symlink('real.json', b.path('.cca/client-updates/codex/state.json'))


@ccase('status/state-file/dangling-symlink', ['status'], env=CODEX_ENV)
def _(b):
    make_app(b)
    updater_dirs(b, 'codex')
    os.symlink('nowhere.json', b.path('.cca/client-updates/codex/state.json'))


@ccase('status/state-file/hard-link', ['status'], env=CODEX_ENV)
def _(b):
    make_app(b)
    put_state(b, 'codex', {'status': 'current', 'message': 'm'})
    os.link(b.path('.cca/client-updates/codex/state.json'), b.path('.cca/client-updates/codex/again.json'))


@ccase('status/state-file/directory', ['status'], env=CODEX_ENV)
def _(b):
    make_app(b)
    updater_dirs(b, 'codex')
    os.mkdir(b.path('.cca/client-updates/codex/state.json'), 0o700)


for name, rel, mode in [('base-755', '.cca/client-updates', 0o755), ('base-770', '.cca/client-updates', 0o770), ('client-dir-755', '.cca/client-updates/codex', 0o755),
                        ('client-dir-701', '.cca/client-updates/codex', 0o701)]:
    @ccase('status/updater-dirs/' + name, ['status'], env=BOTH_ENV)
    def _(b, rel=rel, mode=mode):
        make_app(b)
        make_app(b, 'claude', '4.1.0')
        put_state(b, 'codex', {'status': 'current', 'message': 'm'})
        put_state(b, 'claude', {'status': 'current', 'message': 'm'})
        os.chmod(b.path(rel), mode)


@ccase('status/updater-dirs/base-is-a-symlink', ['status'], env=CODEX_ENV)
def _(b):
    make_app(b)
    os.makedirs(b.path('.cca'), mode=0o700)
    os.makedirs(b.path('elsewhere/codex'), mode=0o700)
    os.chmod(b.path('elsewhere'), 0o700)
    os.symlink(b.path('elsewhere'), b.path('.cca/client-updates'))


@ccase('status/updater-dirs/base-is-a-file', ['status'], env=CODEX_ENV)
def _(b):
    make_app(b)
    b.write('.cca/client-updates', 'x')


@ccase('status/updater-dirs/client-dir-is-a-file', ['status'], env=CODEX_ENV)
def _(b):
    make_app(b)
    updater_dirs(b)
    b.write('.cca/client-updates/codex', 'x')


@ccase('status/updater-dirs/client-dir-is-a-symlink', ['status'], env=CODEX_ENV)
def _(b):
    make_app(b)
    updater_dirs(b, 'elsewhere')
    os.symlink('elsewhere', b.path('.cca/client-updates/codex'))


@ccase('status/transaction-pending', ['status'], env=CODEX_ENV, before=staged())   # 上次安装没做完：只说要恢复，不碰现场
def _(b):
    make_app(b)
    package(b)
    b.write('.cca/client-updates/codex/transaction.json', '{"v": 1}')


@ccase('status/transaction-pending-dangling-link', ['status'], env=CODEX_ENV)
def _(b):
    make_app(b)
    updater_dirs(b, 'codex')
    os.symlink('nowhere', b.path('.cca/client-updates/codex/transaction.json'))


# --- status：暂存好的更新 ----------------------------------------------------------------------------------------------
@ccase('status/staged/can-install', ['status'], env=CODEX_ENV, before=staged())
def _(b):
    make_app(b)
    package(b)


@ccase('status/staged/claude-can-install', ['status'], env=CLAUDE_ENV, before=staged('claude', '4.2.0'))
def _(b):
    make_app(b, 'claude', '4.1.0')
    package(b, 'claude', version='4.2.0')


@ccase('status/staged/both-with-their-own-tokens', ['status'], env=BOTH_ENV, before=both(staged(), staged('claude', '4.2.0', token=OTHER)))
def _(b):
    make_app(b)
    make_app(b, 'claude', '4.1.0')
    package(b)
    package(b, 'claude', version='4.2.0')


for name, status in [('running', 'running'), ('current', 'current'), ('needs-recheck', 'needs_recheck'), ('error', 'error'), ('number', 5)]:
    @ccase('status/staged/state-status-' + name, ['status'], env=CODEX_ENV, before=staged(status=status))
    def _(b):
        make_app(b)
        package(b)


for name, build in [('same-build', '100'), ('older-build', '99'), ('build-not-a-number', 'abc'), ('build-dotted', '1.2'), ('build-spaces', ' 200 '), ('build-underscore', '2_00'),
                    ('build-huge', '9' * 30), ('build-negative', '-5')]:
    @ccase('status/staged/' + name, ['status'], env=CODEX_ENV, before=staged(build=build))
    def _(b):
        make_app(b)
        package(b)


for name, build in [('number', 200), ('null', None), ('list', ['200']), ('list-with-dot', ['.']), ('dict', {'a': 1}), ('dict-with-dot', {'.': 1}), ('true', True), ('float', 200.5)]:
    def edit(state, b, build=build):
        state['stage']['metadata']['build'] = build

    @ccase('status/staged/build-type-' + name, ['status'], env=CODEX_ENV, before=staged(edit=edit))
    def _(b):
        make_app(b)
        package(b)


@ccase('status/staged/claude-build-compared-as-version', ['status'], env=CLAUDE_ENV, before=staged('claude', '4.23118.0'))   # 4.23118.0 比 4.9.0 新（不是按文字比）
def _(b):
    make_app(b, 'claude', '4.9.0')
    package(b, 'claude', version='4.23118.0')


@ccase('status/staged/claude-older', ['status'], env=CLAUDE_ENV, before=staged('claude', '4.0.9'))
def _(b):
    make_app(b, 'claude', '4.1.0')
    package(b, 'claude', version='4.0.9')


@ccase('status/staged/target-changed-since', ['status'], env=CODEX_ENV, before=both(staged(), lambda b: make_app(b, version='3.0', build='300') and None))
def _(b):
    make_app(b)
    package(b)


@ccase('status/staged/target-touched-since', ['status'], env=CODEX_ENV, before=both(staged(), lambda b: os.utime(b.path(CODEX_APP + '/Contents/Resources/app.asar'), (5, 5))))
def _(b):
    make_app(b)
    package(b)


@ccase('status/staged/archive-rewritten-since', ['status'], env=CODEX_ENV, before=both(staged(), lambda b: package(b, data=b'changed') and None))
def _(b):
    make_app(b)
    package(b)


@ccase('status/staged/archive-missing', ['status'], env=CODEX_ENV, before=both(staged(), lambda b: os.unlink(b.path('.cca/client-updates/codex/package.zip'))))
def _(b):
    make_app(b)
    package(b)


@ccase('status/staged/archive-mode-644', ['status'], env=CODEX_ENV, before=staged())
def _(b):
    make_app(b)
    package(b, mode=0o644)


@ccase('status/staged/archive-is-a-symlink', ['status'], env=CODEX_ENV, before=staged())
def _(b):
    make_app(b)
    package(b)
    os.rename(b.path('.cca/client-updates/codex/package.zip'), b.path('.cca/client-updates/codex/real.zip'))
    os.symlink('real.zip', b.path('.cca/client-updates/codex/package.zip'))


STAGE_EDITS = [
    ('no-archive-identity', lambda s, b: s['stage'].pop('archive_identity')),
    ('archive-identity-null', lambda s, b: s['stage'].update(archive_identity=None)),
    ('archive-identity-floats', lambda s, b: s['stage'].update(archive_identity=[float(x) for x in s['stage']['archive_identity']])),
    ('no-token', lambda s, b: s['stage'].pop('token')),
    ('token-null', lambda s, b: s['stage'].update(token=None)),
    ('token-number', lambda s, b: s['stage'].update(token=5)),
    ('no-target', lambda s, b: s['stage'].pop('target')),
    ('target-null', lambda s, b: s['stage'].update(target=None)),
    ('target-number', lambda s, b: s['stage'].update(target=5)),
    ('target-list', lambda s, b: s['stage'].update(target=[])),
    ('target-empty-dict', lambda s, b: s['stage'].update(target={})),
    ('target-without-hashes', lambda s, b: s['stage']['target'].pop('hashes')),
    ('target-other-hashes', lambda s, b: s['stage']['target'].update(hashes={'x': 'y'})),
    ('target-extra-key', lambda s, b: s['stage']['target'].update(extra=1)),
    ('target-version-differs', lambda s, b: s['stage']['target'].update(version='0.9')),
    ('target-inode-as-float', lambda s, b: s['stage']['target'].update(inode=float(s['stage']['target']['inode']))),
    ('metadata-version-number', lambda s, b: s['stage']['metadata'].update(version=2.5)),
    ('metadata-version-null', lambda s, b: s['stage']['metadata'].update(version=None)),
    ('metadata-at-top-not-dict', lambda s, b: s.update(metadata='2.0')),
    ('auto-wait-auto-install-off', lambda s, b: s.update(auto_wait=True)),
    ('auto-wait-1-auto-install-on', lambda s, b: (s.update(auto_wait=1), b.cfg({'client_auto_install': True}))),
    ('auto-wait-false-auto-install-on', lambda s, b: (s.update(auto_wait=False), b.cfg({'client_auto_install': True}))),
    ('auto-wait-auto-install-on-mode-off', lambda s, b: (s.update(auto_wait=True), b.cfg({'client_auto_install': True, 'client_updates': 'off'}))),
]
for name, edit in STAGE_EDITS:
    @ccase('status/staged/' + name, ['status'], env=CODEX_ENV, before=staged(edit=edit))
    def _(b):
        make_app(b)
        package(b)


@ccase('status/staged/auto-install-pending', ['status'], env=BOTH_ENV,   # 下载好了、客户端开着：等它退出；下次检查提前到 15 分钟以后
       before=both(staged(auto_wait=True), staged('claude', '4.2.0', token=OTHER)))
def _(b):
    b.cfg({'client_auto_install': True})
    make_app(b)
    make_app(b, 'claude', '4.1.0')
    package(b)
    package(b, 'claude', version='4.2.0')


@ccase('status/staged/auto-install-pending-claude-chinese-version', ['status'], env=CLAUDE_ENV,
       before=staged('claude', '4.2.0', auto_wait='yes', edit=lambda s, b: s['stage']['metadata'].update(version='四点二')))
def _(b):
    b.cfg({'client_auto_install': True})
    make_app(b, 'claude', '4.1.0')
    package(b, 'claude', version='4.2.0')


# --- status：终端工具 --------------------------------------------------------------------------------------------------
@ccase('status/cli/claude', ['status'], env=CLI_ENV)
def _(b):
    make_tool(b)


@ccase('status/cli/codex', ['status'], env=CODEX_CLI_ENV)
def _(b):
    make_tool(b, CODEX_BIN, 'codex-cli 0.166.0\n')


@ccase('status/cli/both-and-both-apps', ['status'], env=dict(BOTH_ENV, CCA_CLI_CLAUDE='%HOME%/' + CLAUDE_BIN, CCA_CLI_CODEX='%HOME%/' + CODEX_BIN))
def _(b):
    make_app(b)
    make_app(b, 'claude', '4.1.0')
    make_tool(b)
    b.write(CODEX_BIN, '#!/bin/sh\necho "codex-cli 0.166.0"\n', 0o755)


for name, said in [('plain', '2.0.0\n'), ('v-prefix', 'v2.0.0\n'), ('two-versions', 'claude 1.2.3.4 build 2.0.1\n'), ('four-parts', '1.2.3.4\n'), ('two-parts', '2.0\n'),
                   ('nothing', ''), ('garbage', 'command not found\n'), ('not-utf8', b'\xff2.0.0\xfe\n'), ('fullwidth', '２.０.０\n'), ('trailing-dot', '2.0.0.\n'),
                   ('long-numbers', '123456789012345678901234567890.0.1\n'), ('second-line', 'Claude Code\nversion: 2.3.4 (abc)\n')]:
    @ccase('status/cli/version-output/' + name, ['status'], env=CLI_ENV)
    def _(b, said=said):
        make_tool(b, version=said)


@ccase('status/cli/version-exits-1', ['status'], env=CLI_ENV)
def _(b):
    make_tool(b)
    b.write('tool/version.rc', '1')


@ccase('status/cli/version-on-stderr-only', ['status'], env=CLI_ENV)
def _(b):
    make_tool(b, script='echo "2.0.0" >&2\n')


@ccase('status/cli/not-executable', ['status'], env=CLI_ENV)
def _(b):
    b.write(CLAUDE_BIN, '#!/bin/sh\necho 2.0.0\n', 0o644)


@ccase('status/cli/path-missing', ['status'], env=CLI_ENV)
def _(b):
    pass


@ccase('status/cli/path-is-a-directory', ['status'], env=CLI_ENV)
def _(b):
    os.makedirs(b.path(CLAUDE_BIN))


@ccase('status/cli/killed-by-signal', ['status'], env=CLI_ENV)
def _(b):
    make_tool(b, script='echo 2.0.0\nkill -9 $$\n')


@ccase('status/cli/sees-a-clean-environment', ['status'], env=dict(CLI_ENV, ANTHROPIC_API_KEY='not-a-real-key', HTTPS_PROXY='http://example.invalid', CLAUDE_CONFIG_DIR='/do-not-read'))
def _(b):   # 不带账号、代理、API key；配置目录是一次性的
    make_tool(b, script='env | LC_ALL=C sort >> "$HOME/tool/log"\nls -la "$CLAUDE_CONFIG_DIR" | wc -l >> "$HOME/tool/log"\necho 2.0.0\n')


@ccase('status/cli/codex-sees-a-clean-environment', ['status'], env=dict(CODEX_CLI_ENV, OPENAI_API_KEY='not-a-real-key', CODEX_HOME='/do-not-read'))
def _(b):
    make_tool(b, CODEX_BIN, script='env | LC_ALL=C sort >> "$HOME/tool/log"\necho 0.166.0\n')


@ccase('status/cli/stable-channel-in-throwaway-config', ['status'], env=CLI_ENV)   # 更新通道照用户共享的 settings.json
def _(b):
    b.json('.claude/settings.json', {'autoUpdatesChannel': 'stable', 'other': 1})
    make_tool(b, script='cat "$CLAUDE_CONFIG_DIR/settings.json" >> "$HOME/tool/log"\necho 2.0.0\n')


for name, settings in [('latest', {'autoUpdatesChannel': 'latest'}), ('unknown', {'autoUpdatesChannel': 'beta'}), ('number', {'autoUpdatesChannel': 1}), ('list', ['stable'])]:
    @ccase('status/cli/channel-' + name, ['status'], env=CLI_ENV)
    def _(b, settings=settings):
        b.json('.claude/settings.json', settings)
        make_tool(b, script='ls "$CLAUDE_CONFIG_DIR" >> "$HOME/tool/log"\necho 2.0.0\n')


@ccase('status/cli/available-with-button', ['status'], env=CLI_ENV)
def _(b):
    make_tool(b)
    cli_state(b)


@ccase('status/cli/codex-available-with-button', ['status'], env=CODEX_CLI_ENV)
def _(b):
    make_tool(b, CODEX_BIN, 'codex-cli 0.166.0\n')
    cli_state(b, 'codex-cli', latest='0.167.0', installed='0.166.0')


for name, kw in [('updated-itself-meanwhile', dict(latest='2.0.0')), ('older-than-installed', dict(latest='1.9.9')), ('installed-by-npm', dict(method='npm')),
                 ('installed-by-brew', dict(method='brew')), ('method-other', dict(method='other')), ('method-missing', dict(method=None)),
                 ('just-updated-keeps-its-message', dict(latest='2.0.0', status='updated', message='已更新到 2.0.0，下次启动时用新版本', stage=False)),
                 ('status-error', dict(status='error', message='无法从官方来源下载更新，请稍后重试', stage=False)),
                 ('no-stage', dict(stage=False)), ('latest-two-parts', dict(latest='3.0')), ('latest-not-a-version', dict(latest='latest')),
                 ('latest-number', dict(latest=3)), ('latest-empty', dict(latest='')), ('latest-null', dict(latest=None)), ('latest-huge', dict(latest='9' * 4301 + '.0.0'))]:
    @ccase('status/cli/state/' + name, ['status'], env=CLI_ENV)
    def _(b, kw=kw):
        make_tool(b)
        cli_state(b, **kw)


CLI_EDITS = [
    ('path-moved', lambda s: s['stage']['target'].update(path='/somewhere/else/claude')),
    ('no-target', lambda s: s['stage'].pop('target')),
    ('target-string', lambda s: s['stage'].update(target='x')),
    ('target-no-path', lambda s: s['stage']['target'].pop('path')),
    ('no-token', lambda s: s['stage'].pop('token')),
    ('stage-list', lambda s: s.update(stage=[1])),
    ('metadata-list', lambda s: s.update(metadata=['2.1.0'])),
]
for name, edit in CLI_EDITS:
    @ccase('status/cli/state/' + name, ['status'], env=CLI_ENV)
    def _(b, edit=edit):
        make_tool(b)
        p = cli_state(b)
        with open(p) as f:
            s = json.load(f)
        edit(s)
        b.json('.cca/client-updates/claude-cli/state.json', s)


@ccase('status/cli/state-unreadable-and-version-unreadable', ['status'], env=CLI_ENV)
def _(b):
    make_tool(b)
    b.write('tool/version.rc', '3')
    updater_dirs(b, 'claude-cli')
    b.write('.cca/client-updates/claude-cli/state.json', '{oops')


if SLOW:
    @ccase('status/cli/slow/version-hangs', ['status'], env=CLI_ENV, timeout=60)   # 20 秒到点杀掉
    def _(b):
        make_tool(b, script='exec sleep 40\n')   # exec：到点杀掉的就是 sleep 自己，不留一个没人管的 sleep

    # 检查到一半进程被结束（SIGTERM：App 退出、关机）：原来没有接这个信号，`with TemporaryDirectory` 来不及收尾，check-… 目录留在这种
    # 客户端的状态目录里（正在下安装包的话，下到一半的包也在里面），以后也没有谁删。见 FINDINGS.md
    @ccase('check/cli/slow/terminated', ['check', 'force'], env=CLI_ENV, timeout=60, during=lambda b: interrupt(b, signal.SIGTERM),
           before=lambda b: lambda: leftover(b, '.cca/client-updates/claude-cli', 'check-'))
    def _(b):
        make_tool(b, script='echo $PPID > "$HOME/tool/ppid"; echo $$ > "$HOME/tool/pid"; exec sleep 20\n')


# --- mode、auto-install ------------------------------------------------------------------------------------------------
for action, arg in [('mode', 'auto'), ('mode', 'off'), ('auto-install', 'on'), ('auto-install', 'off')]:
    @ccase('%s/%s/no-config' % (action, arg), [action, arg])
    def _(b):
        pass

    @ccase('%s/%s/keeps-other-keys' % (action, arg), [action, arg], env=CODEX_ENV)
    def _(b):
        b.cfg({'version': 2, 'client_updates': 'off', 'nodes': ['US-A'], 'client_auto_install': True, 'min_trust': 50})
        make_app(b)

    @ccase('%s/%s/from-the-other-setting' % (action, arg), [action, arg])
    def _(b):
        b.cfg({'client_updates': 'auto', 'client_auto_install': False})

    @ccase('%s/%s/config-mode-kept' % (action, arg), [action, arg])
    def _(b):
        b.json('.cca/config.json', {'version': 2}, 0o644)

    @ccase('%s/%s/config-is-a-symlink' % (action, arg), [action, arg])
    def _(b):
        b.json('elsewhere/config.json', {'version': 2})
        os.makedirs(b.path('.cca'))
        os.symlink(b.path('elsewhere/config.json'), b.path('.cca/config.json'))

    for name, text in [('not-json', '{oops'), ('list', '[]'), ('nonempty-list', '[1]'), ('null', 'null'), ('string', '"x"'), ('number', '5'), ('true', 'true'), ('empty-file', '')]:
        @ccase('%s/%s/config-%s' % (action, arg, name), [action, arg])
        def _(b, text=text):
            b.write('.cca/config.json', text)


for action, args in [('mode', []), ('mode', ['']), ('mode', ['on']), ('mode', ['AUTO']), ('auto-install', []), ('auto-install', ['auto']), ('auto-install', ['maybe']),
                     ('auto-install', ['ON'])]:
    @ccase('%s/bad-argument/%s' % (action, args[0] if args else 'none') + ('-empty' if args == [''] else ''), [action] + args, env=CODEX_ENV)
    def _(b):
        b.cfg({'version': 2})
        make_app(b)


@ccase('mode/off/cca-home-is-a-file', ['mode', 'off'])
def _(b):
    b.write('.cca', 'x')


@ccase('mode/off/lock-file-is-a-directory', ['mode', 'off'])
def _(b):
    os.makedirs(b.path('.cca/sync.lock'))


@ccase('mode/off/still-works-with-broken-cache', ['mode', 'off'], env=CODEX_ENV)   # 缓存坏了也关得掉
def _(b):
    make_app(b)
    updater_dirs(b, 'codex')
    b.write('.cca/client-updates/codex/state.json', '{bad')


@ccase('auto-install/on/shows-pending', ['auto-install', 'on'], env=CODEX_ENV, before=staged(auto_wait=True))
def _(b):
    make_app(b)
    package(b)


@ccase('auto-install/off/hides-pending', ['auto-install', 'off'], env=CODEX_ENV, before=staged(auto_wait=True))
def _(b):
    b.cfg({'client_auto_install': True})
    make_app(b)
    package(b)


# --- plan：确认框里要写的 ----------------------------------------------------------------------------------------------
for name, token in [('not-hex', 'not-a-token'), ('empty', ''), ('too-short', 'a1' * 31), ('too-long', 'a1' * 33), ('uppercase', 'A1' * 32), ('trailing-newline', 'a1' * 32 + '\n'),
                    ('unknown', OTHER)]:
    @ccase('plan/bad-token/' + name, ['plan', token], env=CODEX_ENV, before=staged())
    def _(b):
        make_app(b)
        package(b)


@ccase('plan/no-token-argument', ['plan'], env=CODEX_ENV, before=staged())
def _(b):
    make_app(b)
    package(b)


@ccase('plan/nothing-staged', ['plan', TOKEN], env=CODEX_ENV)
def _(b):
    make_app(b)


@ccase('plan/desktop/codex', ['plan', TOKEN], env=CODEX_ENV, before=staged())
def _(b):
    make_app(b)
    package(b)


@ccase('plan/desktop/claude', ['plan', TOKEN], env=CLAUDE_ENV, before=staged('claude', '4.2.0'))
def _(b):
    make_app(b, 'claude', '4.1.0')
    package(b, 'claude', version='4.2.0')


@ccase('plan/desktop/second-client-token', ['plan', OTHER], env=BOTH_ENV, before=both(staged('claude', '4.2.0'), staged(token=OTHER)))
def _(b):
    make_app(b)
    make_app(b, 'claude', '4.1.0')
    package(b)
    package(b, 'claude', version='4.2.0')


@ccase('plan/desktop/same-token-twice-first-client-wins', ['plan', TOKEN], env=BOTH_ENV, before=both(staged('claude', '4.2.0'), staged()))
def _(b):
    make_app(b)
    make_app(b, 'claude', '4.1.0')
    package(b)
    package(b, 'claude', version='4.2.0')


def staged_at(rel, **kw):
    """staged，App 在别的地方"""
    def before(b):
        old = APPS['codex']
        APPS['codex'] = rel
        try:
            return staged(**kw)(b)
        finally:
            APPS['codex'] = old
    return before


@ccase('plan/desktop/target-path-is-shell-quoted', ['plan', TOKEN], env={'CCA_CLIENT_CODEX_APP': "%HOME%/My Apps/It's/ChatGPT.app"}, before=staged_at("My Apps/It's/ChatGPT.app"))
def _(b):
    make_app(b, rel="My Apps/It's/ChatGPT.app")
    package(b)


for name, status in [('status-current', 'current'), ('status-error', 'error')]:   # plan 不看 status：凭据、目标、安装包对得上就行
    @ccase('plan/desktop/' + name, ['plan', TOKEN], env=CODEX_ENV, before=staged(status=status))
    def _(b):
        make_app(b)
        package(b)


@ccase('plan/desktop/target-changed', ['plan', TOKEN], env=CODEX_ENV, before=both(staged(), lambda b: make_app(b, version='3.0', build='300') and None))
def _(b):
    make_app(b)
    package(b)


@ccase('plan/desktop/target-gone', ['plan', TOKEN], env=CODEX_ENV, before=both(staged(), lambda b: os.rename(b.path(CODEX_APP), b.path('Applications/Gone.app'))))
def _(b):
    make_app(b)
    package(b)


@ccase('plan/desktop/target-not-installed-anymore', ['plan', TOKEN], before=staged())   # 环境里说没装
def _(b):
    make_app(b)
    package(b)


for name, build in [('same-build', '100'), ('older-build', '99'), ('build-not-a-number', 'x')]:
    @ccase('plan/desktop/' + name, ['plan', TOKEN], env=CODEX_ENV, before=staged(build=build))
    def _(b):
        make_app(b)
        package(b)


@ccase('plan/desktop/archive-changed', ['plan', TOKEN], env=CODEX_ENV, before=both(staged(), lambda b: package(b, data=b'changed') and None))
def _(b):
    make_app(b)
    package(b)


@ccase('plan/desktop/archive-missing', ['plan', TOKEN], env=CODEX_ENV, before=both(staged(), lambda b: os.unlink(b.path('.cca/client-updates/codex/package.zip'))))
def _(b):
    make_app(b)
    package(b)


@ccase('plan/desktop/archive-mode-644', ['plan', TOKEN], env=CODEX_ENV, before=staged())
def _(b):
    make_app(b)
    package(b, mode=0o644)


@ccase('plan/desktop/archive-hard-linked', ['plan', TOKEN], env=CODEX_ENV, before=staged())
def _(b):
    make_app(b)
    os.link(package(b), b.path('copy.zip'))


@ccase('plan/desktop/key-file-hard-linked', ['plan', TOKEN], env=CODEX_ENV)   # 算摘要时不收有别的名字的文件
def _(b):
    make_app(b)
    package(b)
    os.link(b.path(CODEX_APP + '/Contents/Resources/app.asar'), b.path('elsewhere'))
    put_state(b, 'codex', {'stage': {'token': TOKEN, 'metadata': {'version': '2.0', 'build': '200'}}, 'status': 'staged'})


PLAN_EDITS = [
    ('token-non-ascii', lambda s, b: s['stage'].update(token='é' * 64)),
    ('token-number', lambda s, b: s['stage'].update(token=5)),
    ('token-null', lambda s, b: s['stage'].update(token=None)),
    ('token-list', lambda s, b: s['stage'].update(token=[TOKEN])),
    ('no-token', lambda s, b: s['stage'].pop('token')),
    ('no-metadata', lambda s, b: s['stage'].pop('metadata')),
    ('metadata-string', lambda s, b: s['stage'].update(metadata='x')),
    ('no-package-sha256', lambda s, b: s['stage'].pop('package_sha256')),
    ('package-sha256-number', lambda s, b: s['stage'].update(package_sha256=5)),
    ('target-without-version', lambda s, b: (s['stage']['target'].pop('version'))),
    ('version-is-a-list', lambda s, b: s['stage']['metadata'].update(version=['2', 0])),
    ('version-null', lambda s, b: s['stage']['metadata'].update(version=None)),
    ('version-with-quote', lambda s, b: s['stage']['metadata'].update(version="2.0'; rm -rf x")),
]
for name, edit in PLAN_EDITS:
    @ccase('plan/desktop/state/' + name, ['plan', TOKEN], env=CODEX_ENV, before=staged(edit=edit))
    def _(b):
        make_app(b)
        package(b)


@ccase('plan/desktop/state-of-another-client-unreadable', ['plan', TOKEN], env=BOTH_ENV, before=staged())   # 第一个客户端的缓存坏了：跳过它接着找
def _(b):
    make_app(b)
    make_app(b, 'claude', '4.1.0')
    package(b)
    updater_dirs(b, 'claude')
    b.write('.cca/client-updates/claude/state.json', '{oops')


@ccase('plan/cli/claude', ['plan', TOKEN], env=CLI_ENV)
def _(b):
    make_tool(b)
    cli_state(b)


@ccase('plan/cli/codex', ['plan', TOKEN], env=CODEX_CLI_ENV)
def _(b):
    make_tool(b, CODEX_BIN, 'codex-cli 0.166.0\n')
    cli_state(b, 'codex-cli', latest='0.167.0', installed='0.165.0')   # 记下的版本旧了：确认框里写现在的


for name, kw in [('already-this-version', dict(latest='2.0.0')), ('already-newer', dict(latest='1.9.0')), ('method-npm', dict(method='npm')), ('method-missing', dict(method=None)),
                 ('latest-not-a-version', dict(latest='newest')), ('latest-number', dict(latest=3))]:
    @ccase('plan/cli/' + name, ['plan', TOKEN], env=CLI_ENV)
    def _(b, kw=kw):
        make_tool(b)
        cli_state(b, **kw)


@ccase('plan/cli/path-changed', ['plan', TOKEN], env={'CCA_CLI_CLAUDE': '%HOME%/bin/claude'})
def _(b):
    b.fake('claude', 'echo 2.0.0\n')
    cli_state(b)


@ccase('plan/cli/not-installed-anymore', ['plan', TOKEN])
def _(b):
    make_tool(b)
    cli_state(b)


@ccase('plan/cli/version-unreadable', ['plan', TOKEN], env=CLI_ENV)
def _(b):
    make_tool(b)
    b.write('tool/version.rc', '1')
    cli_state(b)


for name, edit in [('no-target', lambda s: s['stage'].pop('target')), ('target-string', lambda s: s['stage'].update(target='x')), ('no-metadata', lambda s: s['stage'].pop('metadata')),
                   ('target-extra-keys', lambda s: s['stage']['target'].update(note='n', version='0.0.1'))]:
    @ccase('plan/cli/state/' + name, ['plan', TOKEN], env=CLI_ENV)
    def _(b, edit=edit):
        make_tool(b)
        p = cli_state(b)
        with open(p) as f:
            s = json.load(f)
        edit(s)
        b.json('.cca/client-updates/claude-cli/state.json', s)


# --- check：这一步之前的检查 --------------------------------------------------------------------------------------------
for name, args in [('bad-argument', ['check', 'now']), ('bad-argument-FORCE', ['check', 'FORCE'])]:
    @ccase('check/' + name, args, env=CODEX_ENV)
    def _(b):
        make_app(b)


for arg in ([], ['force']):
    tag = 'force' if arg else 'due'

    @ccase('check/%s/nothing-installed' % tag, ['check'] + arg)
    def _(b):
        pass

    @ccase('check/%s/nothing-installed-with-leftover-state' % tag, ['check'] + arg)   # 没装的不查，记着的状态也不动
    def _(b):
        put_state(b, 'codex', {'status': 'current', 'message': 'm', 'checked_epoch': 5})

    @ccase('check/%s/manager-lock-held' % tag, ['check'] + arg, env=CODEX_ENV, before=hold('.cca/client-updates/manager.lock'))
    def _(b):
        make_app(b)
        updater_dirs(b)

    @ccase('check/%s/cca-home-group-writable' % tag, ['check'] + arg, env=CODEX_ENV)
    def _(b):
        make_app(b)
        os.makedirs(b.path('.cca'))
        os.chmod(b.path('.cca'), 0o770)

    @ccase('check/%s/cca-home-is-a-symlink' % tag, ['check'] + arg, env=CODEX_ENV)
    def _(b):
        make_app(b)
        os.makedirs(b.path('real-cca'), mode=0o700)
        os.symlink(b.path('real-cca'), b.path('.cca'))

    @ccase('check/%s/cca-home-is-a-file' % tag, ['check'] + arg, env=CODEX_ENV)
    def _(b):
        make_app(b)
        b.write('.cca', 'x')

    @ccase('check/%s/updater-directory-is-a-symlink' % tag, ['check'] + arg, env=CODEX_ENV)   # 不往链接指的地方写东西
    def _(b):
        make_app(b)
        os.makedirs(b.path('.cca'), mode=0o700)
        os.makedirs(b.path('unrelated'), mode=0o700)
        os.symlink(b.path('unrelated'), b.path('.cca/client-updates'))

    @ccase('check/%s/manager-lock-is-a-symlink' % tag, ['check'] + arg, env=CODEX_ENV)
    def _(b):
        make_app(b)
        updater_dirs(b)
        b.write('elsewhere.lock', '')
        os.symlink(b.path('elsewhere.lock'), b.path('.cca/client-updates/manager.lock'))

    @ccase('check/%s/manager-lock-mode-644' % tag, ['check'] + arg, env=CODEX_ENV)
    def _(b):
        make_app(b)
        updater_dirs(b)
        b.write('.cca/client-updates/manager.lock', '', 0o644)

    @ccase('check/%s/install-lock-held' % tag, ['check'] + arg, env=BOTH_ENV, before=hold('.cca/client-updates/codex/install.lock'))   # 一个客户端的锁拿不到，不耽误别的
    def _(b):
        make_app(b)
        make_app(b, 'claude', '4.1.0')
        updater_dirs(b, 'codex', 'claude')
        put_state(b, 'claude', {'status': 'current', 'message': 'm', 'checked_epoch': NOW - 60})

    @ccase('check/%s/state-unreadable' % tag, ['check'] + arg, env=CODEX_ENV)
    def _(b):
        make_app(b)
        updater_dirs(b, 'codex')
        b.write('.cca/client-updates/codex/state.json', '{oops')


# --- check：还没到时间、关了自动检查 ------------------------------------------------------------------------------------
@ccase('check/due/checked-a-minute-ago', ['check'], env=BOTH_ENV)   # 六小时内查过：什么也不做，连 codesign 都不起
def _(b):
    make_app(b)
    make_app(b, 'claude', '4.1.0')
    put_state(b, 'codex', {'status': 'current', 'message': '当前客户端已是最新版本', 'checked_epoch': NOW - 60})
    put_state(b, 'claude', {'status': 'current', 'message': '当前客户端已是最新版本', 'checked_epoch': int(NOW) - 21000})


@ccase('check/due/mode-off-and-long-overdue', ['check'], env=BOTH_ENV)
def _(b):
    b.cfg({'client_updates': 'off'})
    make_app(b)
    make_app(b, 'claude', '4.1.0')
    put_state(b, 'codex', {'status': 'current', 'message': 'm', 'checked_epoch': 5})


@ccase('check/due/mode-off-never-checked-cli', ['check'], env=CLI_ENV)
def _(b):
    b.cfg({'client_updates': 'off'})
    make_tool(b)


for name, value in [('string', 'yesterday'), ('null', None), ('list', [1]), ('true', True), ('future', NOW + 86400), ('huge-int', 10 ** 30)]:
    @ccase('check/due/checked-epoch-' + name, ['check'], env=CODEX_ENV)
    def _(b, value=value):
        make_app(b)
        put_state(b, 'codex', {'status': 'current', 'message': 'm', 'checked_epoch': value})


@ccase('check/due/staged-package-is-kept', ['check'], env=CODEX_ENV, before=staged())
def _(b):
    make_app(b)
    package(b)


@ccase('check/due/package-without-stage-is-dropped', ['check'], env=CODEX_ENV)   # 没有待安装的更新时不留着安装包
def _(b):
    make_app(b)
    package(b)
    put_state(b, 'codex', {'status': 'current', 'message': 'm', 'checked_epoch': NOW - 60})


@ccase('check/due/package-without-stage-wrong-mode-is-left', ['check'], env=CODEX_ENV)
def _(b):
    make_app(b)
    package(b, mode=0o644)
    put_state(b, 'codex', {'status': 'current', 'message': 'm', 'checked_epoch': NOW - 60})


@ccase('check/due/stage-falsy-drops-package', ['check'], env=CODEX_ENV)
def _(b):
    make_app(b)
    package(b)
    put_state(b, 'codex', {'status': 'staged', 'message': 'm', 'checked_epoch': NOW - 60, 'stage': {}})


# --- check：桌面客户端，到验签名为止 -------------------------------------------------------------------------------------
@ccase('check/force/desktop-unsigned-app', ['check', 'force'], env=CODEX_ENV)   # 真的 codesign 说假 App 没签名：不去问更新源
def _(b):
    make_app(b)


@ccase('check/due/desktop-unsigned-app-never-checked', ['check'], env=CLAUDE_ENV)
def _(b):
    make_app(b, 'claude', '4.1.0')


@ccase('check/force/desktop-both-unsigned', ['check', 'force'], env=BOTH_ENV)
def _(b):
    make_app(b)
    make_app(b, 'claude', '4.1.0')


@ccase('check/force/desktop-drops-stage-on-failure', ['check', 'force'], env=CODEX_ENV, before=staged())   # 出了错：暂存的作废，安装包也删掉
def _(b):
    make_app(b)
    package(b)


@ccase('check/force/desktop-bad-plist', ['check', 'force'], env=CODEX_ENV)
def _(b):
    make_app(b, plist=info_plist('codex', '1', '100'))


@ccase('check/force/desktop-malformed-plist', ['check', 'force'], env=CODEX_ENV)
def _(b):
    make_app(b, plist=b'<?xml version="1.0"?><plist><dict><key>x</plist>')


@ccase('check/force/desktop-app-missing', ['check', 'force'], env=CODEX_ENV)
def _(b):
    pass


@ccase('check/force/desktop-key-file-hard-linked', ['check', 'force'], env=CODEX_ENV)
def _(b):
    make_app(b)
    os.link(b.path(CODEX_APP + '/Contents/Resources/app.asar'), b.path('elsewhere'))


@ccase('check/force/desktop-state-file-wrong-mode', ['check', 'force'], env=CODEX_ENV)
def _(b):
    make_app(b)
    put_state(b, 'codex', {'status': 'current', 'message': 'm'}, mode=0o644)


@ccase('check/force/desktop-client-dir-wrong-mode', ['check', 'force'], env=CODEX_ENV)
def _(b):
    make_app(b)
    updater_dirs(b, 'codex')
    os.chmod(b.path('.cca/client-updates/codex'), 0o755)


# --- check：终端工具，到问最新版本之前 ------------------------------------------------------------------------------------
@ccase('check/force/cli-version-unreadable', ['check', 'force'], env=CLI_ENV)   # 读不出版本：停在联网之前
def _(b):
    make_tool(b)
    b.write('tool/version.rc', '1')


@ccase('check/due/cli-version-unreadable-never-checked', ['check'], env=CLI_ENV)
def _(b):
    make_tool(b, version='no version here\n')


@ccase('check/force/cli-version-unreadable-drops-stage', ['check', 'force'], env=CLI_ENV)
def _(b):
    make_tool(b, version='garbage\n')
    cli_state(b)


@ccase('check/force/cli-not-executable', ['check', 'force'], env=CLI_ENV)
def _(b):
    b.write(CLAUDE_BIN, '#!/bin/sh\necho 2.0.0\n', 0o644)


@ccase('check/force/cli-both-unreadable', ['check', 'force'], env=dict(CLI_ENV, **CODEX_CLI_ENV))
def _(b):
    make_tool(b, version='x\n')
    b.write(CODEX_BIN, '#!/bin/sh\nexit 2\n', 0o755)


@ccase('check/force/cli-and-desktop-problems-in-check-order', ['check', 'force'], env=dict(BOTH_ENV, **CLI_ENV))   # 终端工具先查，stderr 里也是它在前
def _(b):
    make_tool(b, version='x\n')
    make_app(b)
    make_app(b, 'claude', '4.1.0')


@ccase('check/due/cli-throttled', ['check'], env=CLI_ENV)
def _(b):
    make_tool(b)
    cli_state(b)


@ccase('check/due/cli-with-journal-but-no-command', ['check'])   # 没装、但目录里有事务记录：照样进去恢复（终端工具没有这种记录，认不出）
def _(b):
    updater_dirs(b, 'claude-cli')
    b.write('.cca/client-updates/claude-cli/transaction.json', '{"v": 2}')


# --- check：允许自动更新时把记下的更新装上（终端工具，不用联网） ---------------------------------------------------------
AUTO = {'client_auto_install': True}


@ccase('check/auto/cli-installs', ['check'], env=CLI_ENV)   # 六小时内查过：不再问最新版本，直接装记下的那个
def _(b):
    b.cfg(AUTO)
    make_tool(b, nxt='2.1.0 (Claude Code)\n')
    cli_state(b)


@ccase('check/auto/codex-cli-installs', ['check'], env=CODEX_CLI_ENV)
def _(b):
    b.cfg(AUTO)
    make_tool(b, CODEX_BIN, 'codex-cli 0.166.0\n', nxt='codex-cli 0.167.0\n')
    cli_state(b, 'codex-cli', latest='0.167.0', installed='0.166.0')


@ccase('check/auto/cli-stable-channel', ['check'], env=CLI_ENV)   # 一次性的配置目录里带着用户设的更新通道
def _(b):
    b.cfg(AUTO)
    b.json('.claude/settings.json', {'autoUpdatesChannel': 'stable'})
    make_tool(b, nxt='2.1.0\n')
    cli_state(b)


@ccase('check/auto/cli-off-does-not-install', ['check'], env=CLI_ENV)
def _(b):
    make_tool(b, nxt='2.1.0\n')
    cli_state(b)


@ccase('check/auto/cli-on-but-mode-off', ['check'], env=CLI_ENV)
def _(b):
    b.cfg({'client_auto_install': True, 'client_updates': 'off'})
    make_tool(b, nxt='2.1.0\n')
    cli_state(b)


for name, kw in [('npm-left-to-the-user', dict(method='npm')), ('brew-left-to-the-user', dict(method='brew')), ('updated-itself-meanwhile', dict(latest='2.0.0')),
                 ('token-empty', dict(token='')), ('token-null', dict(token=None)), ('token-not-hex', dict(token='zz')), ('token-number', dict(token=7)),
                 ('latest-not-a-version', dict(latest='x'))]:
    @ccase('check/auto/cli-' + name, ['check'], env=CLI_ENV)
    def _(b, kw=kw):
        b.cfg(AUTO)
        make_tool(b, nxt='2.1.0\n')
        cli_state(b, **kw)


@ccase('check/auto/cli-path-changed', ['check'], env={'CCA_CLI_CLAUDE': '%HOME%/bin/claude'})
def _(b):
    b.cfg(AUTO)
    b.fake('claude', 'echo 2.0.0\n')
    cli_state(b)


@ccase('check/auto/cli-update-fails', ['check'], env=CLI_ENV)   # 没更新成：说最后一行，按钮留着
def _(b):
    b.cfg(AUTO)
    make_tool(b)
    b.write('tool/update.rc', '1')
    b.write('tool/update.err', 'first line\nnetwork is down\n')
    cli_state(b)


@ccase('check/auto/cli-update-says-nothing', ['check'], env=CLI_ENV)
def _(b):
    b.cfg(AUTO)
    make_tool(b)
    cli_state(b)


@ccase('check/auto/cli-update-partial', ['check'], env=CLI_ENV)   # 更新了，但没到记下的那个版本
def _(b):
    b.cfg(AUTO)
    make_tool(b, nxt='2.0.5\n')
    b.write('tool/update.out', 'updated to 2.0.5\n' + '长' * 300 + '\n')
    cli_state(b)


@ccase('check/auto/cli-update-newer-than-expected', ['check'], env=CLI_ENV)
def _(b):
    b.cfg(AUTO)
    make_tool(b, nxt='2.2.0\n')
    cli_state(b)


@ccase('check/auto/cli-version-unreadable-after-update', ['check'], env=CLI_ENV)
def _(b):
    b.cfg(AUTO)
    make_tool(b, nxt='broken\n')
    cli_state(b)


@ccase('check/auto/cli-install-lock-held', ['check'], env=CLI_ENV, before=hold('.cca/client-updates/claude-cli/install.lock'))
def _(b):
    b.cfg(AUTO)
    make_tool(b, nxt='2.1.0\n')
    cli_state(b)


@ccase('check/auto/cli-stage-metadata-missing', ['check'], env=CLI_ENV)
def _(b):
    b.cfg(AUTO)
    make_tool(b)
    p = cli_state(b)
    with open(p) as f:
        s = json.load(f)
    s['stage'].pop('metadata')
    b.json('.cca/client-updates/claude-cli/state.json', s)


# --- check：允许自动更新，桌面客户端（走到验签名为止） ---------------------------------------------------------------------
@ccase('check/auto/desktop-claude-stops-at-codesign', ['check'], env=CLAUDE_ENV, before=staged('claude', '4.2.0', auto_wait=True))
def _(b):   # 没开着、凭据和安装包都对：核对 SHA-256、解压，到候选程序验签名时停下（假的没签名），候选目录删掉
    b.cfg(AUTO)
    make_app(b, 'claude', '4.1.0')
    package(b, 'claude', version='4.2.0')


@ccase('check/auto/desktop-codex-needs-the-verifier', ['check'], env=CODEX_ENV, before=staged())
def _(b):
    b.cfg(AUTO)
    make_app(b)
    package(b)


@ccase('check/auto/desktop-client-is-open', ['check'], env=CODEX_ENV, before=both(staged(), running(CODEX_APP)))   # 开着不算出错：记下等它退出
def _(b):
    b.cfg(AUTO)
    make_app(b)
    package(b)


@ccase('check/auto/desktop-client-is-open-already-waiting', ['check'], env=CODEX_ENV, before=both(staged(auto_wait=True), running(CODEX_APP)))
def _(b):
    b.cfg(AUTO)
    make_app(b)
    package(b)


@ccase('check/auto/desktop-helper-from-the-bundle-is-open', ['check'], env=CLAUDE_ENV,
       before=both(staged('claude', '4.2.0'), running(CLAUDE_APP + '/Contents/Frameworks/Claude Helper.app/Contents/MacOS/Claude Helper')))
def _(b):
    b.cfg(AUTO)
    make_app(b, 'claude', '4.1.0')
    package(b, 'claude', version='4.2.0')


@ccase('check/auto/desktop-similar-path-is-not-the-client', ['check'], env=CLAUDE_ENV, before=both(staged('claude', '4.2.0'), running(CLAUDE_APP + '2/Contents/MacOS/Claude')))
def _(b):
    b.cfg(AUTO)
    make_app(b, 'claude', '4.1.0')
    package(b, 'claude', version='4.2.0')


for name, status in [('status-current', 'current'), ('status-needs-recheck', 'needs_recheck')]:
    @ccase('check/auto/desktop-' + name, ['check'], env=CLAUDE_ENV, before=staged('claude', '4.2.0', status=status))
    def _(b):
        b.cfg(AUTO)
        make_app(b, 'claude', '4.1.0')
        package(b, 'claude', version='4.2.0')


@ccase('check/auto/desktop-native-update-while-waiting', ['check'], env=CLAUDE_ENV,   # 等的时候原生更新器先装了新的：不覆盖，也不再等
       before=both(staged('claude', '4.2.0', auto_wait=True), lambda b: make_app(b, 'claude', '4.3.0') and None))
def _(b):
    b.cfg(AUTO)
    make_app(b, 'claude', '4.1.0')
    package(b, 'claude', version='4.2.0')


AUTO_EDITS = [
    ('target-path-number', lambda s, b: s['stage']['target'].update(path=5)),
    ('no-target', lambda s, b: s['stage'].pop('target')),
    ('token-empty', lambda s, b: s['stage'].update(token='')),
    ('token-number', lambda s, b: s['stage'].update(token=5)),
    ('metadata-missing', lambda s, b: s['stage'].pop('metadata')),
]
for name, edit in AUTO_EDITS:
    @ccase('check/auto/desktop-state/' + name, ['check'], env=CLAUDE_ENV, before=staged('claude', '4.2.0', edit=edit))
    def _(b):
        b.cfg(AUTO)
        make_app(b, 'claude', '4.1.0')
        package(b, 'claude', version='4.2.0')


# --- install：终端工具 -------------------------------------------------------------------------------------------------
@ccase('install/cli/claude', ['install', TOKEN], env=CLI_ENV)
def _(b):
    make_tool(b, nxt='2.1.0 (Claude Code)\n')
    cli_state(b)


@ccase('install/cli/codex', ['install', TOKEN], env=CODEX_CLI_ENV)
def _(b):
    make_tool(b, CODEX_BIN, 'codex-cli 0.166.0\n', nxt='codex-cli 0.167.0\n')
    cli_state(b, 'codex-cli', latest='0.167.0', installed='0.166.0')


@ccase('install/cli/clean-environment', ['install', TOKEN],
       env=dict(CLI_ENV, ANTHROPIC_API_KEY='not-a-real-key', OPENAI_API_KEY='not-a-real-key', HTTPS_PROXY='http://example.invalid', CLAUDE_CONFIG_DIR='/do-not-read',
                CODEX_HOME='/do-not-read'))
def _(b):
    make_tool(b, nxt='2.1.0\n')
    cli_state(b)


@ccase('install/cli/stable-channel', ['install', TOKEN], env=CLI_ENV)
def _(b):
    b.json('.claude/settings.json', {'autoUpdatesChannel': 'stable'})
    make_tool(b, nxt='2.1.0\n')
    cli_state(b)


@ccase('install/cli/update-fails', ['install', TOKEN], env=CLI_ENV)
def _(b):
    make_tool(b)
    b.write('tool/update.rc', '1')
    b.write('tool/update.err', 'first line\nnetwork is down\n')
    cli_state(b)


for name, out, err in [('says-nothing', '', ''), ('stdout-only', 'line one\nstill 2.0.0\n', ''), ('stderr-wins', 'on stdout\n', 'on stderr\n\n'), ('whitespace-only', '  \n\n', ''),
                       ('not-utf8', b'\xffbad\n', b''), ('long-last-line', 'a\n' + 'x' * 250 + '\n', ''), ('odd-line-breaks', 'a\x0bb\x1cc d', '')]:
    @ccase('install/cli/update-does-not-update/' + name, ['install', TOKEN], env=CLI_ENV)
    def _(b, out=out, err=err):
        make_tool(b)
        b.write('tool/update.out', out)
        b.write('tool/update.err', err)
        cli_state(b)


@ccase('install/cli/update-exits-1-but-updated', ['install', TOKEN], env=CLI_ENV)   # 只看版本到了没有
def _(b):
    make_tool(b, nxt='2.1.0\n')
    b.write('tool/update.rc', '1')
    cli_state(b)


@ccase('install/cli/version-unreadable-after-update', ['install', TOKEN], env=CLI_ENV)
def _(b):
    make_tool(b, nxt='garbage\n')
    cli_state(b)


for name, kw in [('already-updated', dict(latest='2.0.0')), ('method-npm', dict(method='npm')), ('token-of-nobody', dict(token=OTHER))]:
    @ccase('install/cli/' + name, ['install', TOKEN], env=CLI_ENV)
    def _(b, kw=kw):
        make_tool(b, nxt='2.1.0\n')
        cli_state(b, **kw)


for name, token in [('not-hex', 'not-a-token'), ('empty', ''), ('uppercase', 'A1' * 32)]:
    @ccase('install/bad-token/' + name, ['install', token], env=CLI_ENV)
    def _(b):
        make_tool(b, nxt='2.1.0\n')
        cli_state(b)


@ccase('install/no-token-argument', ['install'], env=CLI_ENV)
def _(b):
    make_tool(b, nxt='2.1.0\n')
    cli_state(b)


@ccase('install/cli/manager-lock-held', ['install', TOKEN], env=CLI_ENV, before=hold('.cca/client-updates/manager.lock'))
def _(b):
    make_tool(b, nxt='2.1.0\n')
    cli_state(b)


@ccase('install/cli/install-lock-held', ['install', TOKEN], env=CLI_ENV, before=hold('.cca/client-updates/claude-cli/install.lock'))
def _(b):
    make_tool(b, nxt='2.1.0\n')
    cli_state(b)


@ccase('install/cli/path-changed', ['install', TOKEN], env={'CCA_CLI_CLAUDE': '%HOME%/bin/claude'})
def _(b):
    b.fake('claude', 'echo 2.0.0\n')
    cli_state(b)


@ccase('install/cli/not-executable', ['install', TOKEN], env=CLI_ENV)
def _(b):
    b.write(CLAUDE_BIN, '#!/bin/sh\necho 2.0.0\n', 0o644)
    cli_state(b)


if SLOW:
    @ccase('install/cli/slow/interrupted', ['install', TOKEN], env=CLI_ENV, timeout=60,   # Ctrl-C：临时目录删掉，按钮留着
           during=lambda b: interrupt(b))
    def _(b):
        make_tool(b, script='case "$1" in --version) echo 2.0.0 ;; update) echo $PPID > "$HOME/tool/ppid"; exec sleep 30 ;; esac\n')
        cli_state(b)


def leftover(b, rel, prefix):
    """收尾：留下来的临时目录名字是随机的，改成固定的名字，两边的沙箱才比得了"""
    folder = b.path(rel)
    for n in sorted(os.listdir(folder)):
        if n.startswith(prefix):
            os.rename(os.path.join(folder, n), os.path.join(folder, prefix + 'LEFT'))


def interrupt(b, sig=signal.SIGINT):
    """等假的命令起来，给起它的进程（原来的 Python / cca-core）发信号（默认是 Ctrl-C 的 SIGINT）。假的命令把自己的进程号记在
    tool/pid 里时，发完信号把它也结束掉（按进程号）：起它的进程是被 SIGTERM 结束的话来不及管它"""
    p, own = b.path('tool/ppid'), b.path('tool/pid')
    for _ in range(200):
        if os.path.exists(p) and os.path.getsize(p):
            break
        time.sleep(0.05)
    with open(p) as f:
        pid = int(f.read())
    os.unlink(p)
    time.sleep(0.3)
    os.kill(pid, sig)
    if os.path.exists(own):
        with open(own) as f:
            child = int(f.read())
        os.unlink(own)
        time.sleep(0.3)
        try:
            os.kill(child, signal.SIGKILL)
        except ProcessLookupError:
            pass


# --- install：桌面客户端（走到验签名为止） -------------------------------------------------------------------------------
@ccase('install/desktop/claude-stops-at-codesign', ['install', TOKEN], env=CLAUDE_ENV, before=staged('claude', '4.2.0'))
def _(b):
    make_app(b, 'claude', '4.1.0')
    package(b, 'claude', version='4.2.0')


@ccase('install/desktop/codex-no-verifier', ['install', TOKEN], env=CODEX_ENV, before=staged())
def _(b):
    make_app(b)
    package(b)


VERIFIER = '%HOME%/verifier/cca'
for name, script, mode in [('ok-then-codesign', 'printf "%s\\n" "$0 $1 ${2##*/} $3" >> "$HOME/../verifier.log"; env | LC_ALL=C sort >> "$HOME/../verifier.log"\nexit 0\n', 0o755),
                           ('rejects', 'exit 1\n', 0o755), ('not-executable', 'exit 0\n', 0o644),
                           ('rewrites-the-archive', 'printf x >> "$2"\nexit 0\n', 0o755), ('touches-the-archive', 'touch -t 202001010101 "$2"\nexit 0\n', 0o755)]:
    @ccase('install/desktop/codex-verifier-' + name, ['install', TOKEN], env=dict(CODEX_ENV, CCA_CLIENT_VERIFIER=VERIFIER), before=staged())
    def _(b, script=script, mode=mode):
        make_app(b)
        package(b)
        b.write('verifier/cca', '#!/bin/sh\n' + script, mode)


for name, verifier in [('relative', 'verifier/cca'), ('missing', '%HOME%/verifier/none'), ('directory', '%HOME%/verifier')]:
    @ccase('install/desktop/codex-verifier-path-' + name, ['install', TOKEN], env=dict(CODEX_ENV, CCA_CLIENT_VERIFIER=verifier), before=staged(), relative=('CCA_CLIENT_VERIFIER',))
    def _(b):
        make_app(b)
        package(b)
        b.write('verifier/cca', '#!/bin/sh\nexit 0\n', 0o755)


@ccase('install/desktop/codex-signature-not-a-string', ['install', TOKEN], env=dict(CODEX_ENV, CCA_CLIENT_VERIFIER=VERIFIER),
       before=staged(edit=lambda s, b: s['stage']['metadata'].update(signature=5)))
def _(b):
    make_app(b)
    package(b)
    b.write('verifier/cca', '#!/bin/sh\nexit 0\n', 0o755)


@ccase('install/desktop/codex-signature-with-nul', ['install', TOKEN], env=dict(CODEX_ENV, CCA_CLIENT_VERIFIER=VERIFIER),
       before=staged(edit=lambda s, b: s['stage']['metadata'].update(signature='ab\x00cd')))
def _(b):
    make_app(b)
    package(b)
    b.write('verifier/cca', '#!/bin/sh\nexit 0\n', 0o755)


@ccase('install/desktop/client-is-open', ['install', TOKEN], env=CLAUDE_ENV, before=both(staged('claude', '4.2.0'), running(CLAUDE_APP + '/Contents/MacOS/Claude')))
def _(b):
    make_app(b, 'claude', '4.1.0')
    package(b, 'claude', version='4.2.0')


# 客户端装在带非 ASCII 字符的目录下、正开着：认不出来（ps 是在 LANG=C 下跑的，路径里的非 ASCII 字节打出来成了 M-…，见 FINDINGS.md），
# 一路走到验签名才停（假的 App 没有签名）。同样的路径不带中文时停在“请先退出”（下面那一条）
for name, folder in [('non-ascii', '应用'), ('ascii', 'My Apps')]:
    @ccase('install/desktop/client-is-open-under-%s-path' % name, ['install', TOKEN],
           env={'CCA_CLIENT_CODEX_APP': '%HOME%/' + folder + '/ChatGPT.app', 'CCA_CLIENT_VERIFIER': VERIFIER},
           before=both(staged_at(folder + '/ChatGPT.app'), running(folder + '/ChatGPT.app/Contents/MacOS/ChatGPT')))
    def _(b, folder=folder):
        make_app(b, rel=folder + '/ChatGPT.app')
        package(b)
        b.write('verifier/cca', '#!/bin/sh\nexit 0\n', 0o755)


@ccase('install/desktop/target-changed', ['install', TOKEN], env=CLAUDE_ENV, before=both(staged('claude', '4.2.0'), lambda b: make_app(b, 'claude', '4.3.0') and None))
def _(b):
    make_app(b, 'claude', '4.1.0')
    package(b, 'claude', version='4.2.0')


@ccase('install/desktop/install-lock-held', ['install', TOKEN], env=CLAUDE_ENV, before=both(staged('claude', '4.2.0'), hold('.cca/client-updates/claude/install.lock')))
def _(b):
    make_app(b, 'claude', '4.1.0')
    package(b, 'claude', version='4.2.0')


def readonly(rel):
    def before(b):
        os.chmod(b.path(rel), 0o555)
        return lambda: os.chmod(b.path(rel), 0o755)
    return before


@ccase('install/desktop/directory-not-writable', ['install', TOKEN], env=CLAUDE_ENV, before=both(staged('claude', '4.2.0'), readonly('Applications')))
def _(b):
    make_app(b, 'claude', '4.1.0')
    package(b, 'claude', version='4.2.0')


for name, edit in [('sha256-differs', lambda s, b: s['stage']['metadata'].update(sha256='0' * 64)), ('length-differs', lambda s, b: s['stage']['metadata'].update(length=5)),
                   ('no-length', lambda s, b: s['stage']['metadata'].pop('length')), ('metadata-empty', lambda s, b: s['stage'].update(metadata={'version': '4.2.0', 'build': '4.2.0'})),
                   ('no-package', lambda s, b: s['stage'].pop('package'))]:
    @ccase('install/desktop/state/' + name, ['install', TOKEN], env=CLAUDE_ENV, before=staged('claude', '4.2.0', edit=edit))
    def _(b):
        make_app(b, 'claude', '4.1.0')
        package(b, 'claude', version='4.2.0')


# 安装包的各种毛病：解压那一步说什么（好的包走到候选程序验签名才停）
ROOT = 'Claude.app'
REG, LNK, DIR = stat.S_IFREG | 0o644, stat.S_IFLNK | 0o777, stat.S_IFDIR | 0o755
ST, DF = zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED
BAD_ZIPS = [
    ('not-a-zip', b'hello, this is not a zip file'),
    ('empty-file', b''),
    ('no-entries', zip_bytes([])),
    ('other-root', zip_bytes([('ChatGPT.app/Contents/x', b'x', REG, ST)])),
    ('traversal', zip_bytes([(ROOT + '/../outside', b'x', REG, ST)])),
    ('absolute', zip_bytes([('/' + ROOT + '/x', b'x', REG, ST)])),
    ('dot-segment', zip_bytes([(ROOT + '/./x', b'x', REG, ST)])),
    ('double-slash', zip_bytes([(ROOT + '//x', b'x', REG, ST)])),
    ('backslash', zip_bytes([(ROOT + '/a\\b', b'x', REG, ST)])),
    ('colon', zip_bytes([(ROOT + '/a:b', b'x', REG, ST)])),
    ('control-character', zip_bytes([(ROOT + '/a\x01b', b'x', REG, ST)])),
    ('case-duplicate', zip_bytes([(ROOT + '/A', b'x', REG, ST), (ROOT + '/a', b'y', REG, ST)])),
    ('unicode-duplicate', zip_bytes([(ROOT + '/é', b'x', REG, ST), (ROOT + '/é', b'y', REG, ST)])),
    ('sharp-s-duplicate', zip_bytes([(ROOT + '/straße', b'x', REG, ST), (ROOT + '/STRASSE', b'y', REG, ST)])),
    ('file-and-dir-same-name', zip_bytes([(ROOT + '/a', b'x', REG, ST), (ROOT + '/a/', b'', DIR, ST)])),
    ('fifo', zip_bytes([(ROOT + '/fifo', b'', stat.S_IFIFO | 0o644, ST)])),
    ('link-outside', zip_bytes([(ROOT + '/link', b'../../outside', LNK, ST)])),
    ('link-absolute', zip_bytes([(ROOT + '/link', b'/etc/passwd', LNK, ST)])),
    ('link-empty', zip_bytes([(ROOT + '/link', b'', LNK, ST)])),
    ('link-to-root-itself', zip_bytes([(ROOT + '/link', b'..', LNK, ST)])),
    ('link-not-utf8', zip_bytes([(ROOT + '/link', b'\xff', LNK, ST)])),
    ('link-too-long', zip_bytes([(ROOT + '/link', b'a' * 5000, LNK, ST)])),
    ('link-dangling', zip_bytes([(ROOT + '/link', b'nowhere', LNK, ST)])),
    ('link-loop', zip_bytes([(ROOT + '/a', b'b', LNK, ST), (ROOT + '/b', b'a', LNK, ST)])),
    ('write-through-link', zip_bytes([(ROOT + '/d', b'x', LNK, ST), (ROOT + '/d/file', b'x', REG, ST)])),
    ('file-under-a-file', zip_bytes([(ROOT + '/a', b'x', REG, ST), (ROOT + '/a/b', b'y', REG, ST)])),
    ('root-is-a-file', zip_bytes([(ROOT, b'x', REG, ST)])),
    ('truncated', app_zip('claude', '4.2.0')[:-30]),
    ('garbage-appended', app_zip('claude', '4.2.0') + b'trailing garbage that hides the end record' * 2000),
    ('prepended-data', b'#!/bin/sh\nexit 0\n' + app_zip('claude', '4.2.0')),
]
for name, data in BAD_ZIPS:
    @ccase('install/desktop/package/' + name, ['install', TOKEN], env=CLAUDE_ENV, before=staged('claude', '4.2.0'))
    def _(b, data=data):
        make_app(b, 'claude', '4.1.0')
        package(b, 'claude', data=data)


def corrupt(data, find, replace):
    assert find in data
    return data.replace(find, replace, 1)


GOOD = app_zip('claude', '4.2.0')
for name, data in [('bad-crc', corrupt(GOOD, b'fake seal 4.2.0', b'fake SEAL 4.2.0')), ('bad-local-header-magic-of-a-directory', corrupt(GOOD, b'PK\x03\x04', b'PK\x03\x05')),   # 目录的条目不读：看不出来
                   ('bad-local-header-magic', GOOD[:4] + corrupt(GOOD[4:], b'PK\x03\x04', b'PK\x03\x05')),
                   ('bad-central-magic', corrupt(GOOD, b'PK\x01\x02', b'PK\x01\x03')), ('with-comment', GOOD[:-2] + b'\x05\x00hello')]:
    @ccase('install/desktop/package/' + name, ['install', TOKEN], env=CLAUDE_ENV, before=staged('claude', '4.2.0'))
    def _(b, data=data):
        make_app(b, 'claude', '4.1.0')
        package(b, 'claude', data=data)


# --- 中断的安装：照事务记录恢复（check 和 install 开头都做） ---------------------------------------------------------------
def journal(client='claude', work='.cca-client-install-abc_123', edit=None, backup=None, incoming=None, target_from=None, candidate=False):
    """before：一份事务记录，target 是现拍的快照（target_from：拍别处的那个 App）。backup / incoming：往工作目录里放的假 App 的版本；
    candidate：记下的候选程序就是现在装着的这个"""
    def before(b):
        app = b.path(APPS[client])
        workdir = os.path.join(os.path.dirname(app), work)
        os.makedirs(workdir, mode=0o700, exist_ok=True)
        os.chmod(workdir, 0o700)
        if incoming:
            make_app(b, client, incoming, rel=os.path.relpath(os.path.join(workdir, os.path.basename(app)), b.home))
        if backup:
            make_app(b, client, backup, rel=os.path.relpath(os.path.join(workdir, 'previous.app'), b.home))
        ns = src()
        ns['CU'] = ns['CU_CLAUDE' if client == 'claude' else 'CU_CODEX']
        target = ns['cu_snapshot'](b.path(target_from) if target_from else app) if os.path.isdir(b.path(target_from) if target_from else app) else {'path': app, 'build': '4.1.0'}
        target['path'] = app
        data = {'v': 1, 'target': target, 'candidate': {'version': '4.2.0', 'build': '4.2.0', 'hashes': {}},
                'metadata': {'version': '4.2.0', 'build': '4.2.0', 'minimum_system': '', 'arch': 'arm64'},
                'incoming': os.path.join(workdir, os.path.basename(app)), 'backup': os.path.join(workdir, 'previous.app'), 'work': workdir}
        if candidate:   # 现在装着的就是记下的候选程序
            data['candidate'] = {k: target[k] for k in ('version', 'build', 'hashes')}
        if edit:
            edit(data, b)
        updater_dirs(b, client)
        b.json('.cca/client-updates/%s/transaction.json' % client, data)
    return before


@ccase('recover/before-the-swap', ['check'], env=CLAUDE_ENV, before=journal(incoming='4.2.0'))   # 旧程序还没动：候选目录清掉，说重新检查
def _(b):
    make_app(b, 'claude', '4.1.0')
    put_state(b, 'claude', {'status': 'staged', 'message': 'm', 'checked_epoch': NOW - 60})


@ccase('recover/before-the-swap-keeps-the-stage', ['check'], env=CLAUDE_ENV, before=both(staged('claude', '4.2.0'), journal(incoming='4.2.0')))   # 暂存的更新还在，凭据照旧能用
def _(b):
    make_app(b, 'claude', '4.1.0')
    package(b, 'claude', version='4.2.0')


@ccase('recover/before-the-swap-on-install', ['install', TOKEN], env=CLAUDE_ENV, before=journal(incoming='4.2.0'))
def _(b):
    make_app(b, 'claude', '4.1.0')
    put_state(b, 'claude', {'status': 'staged', 'message': 'm', 'checked_epoch': NOW - 60, 'stage': {'token': TOKEN}})


@ccase('recover/target-in-place-backup-present', ['check'], env=CLAUDE_ENV, before=journal(backup='4.1.0'))   # 新旧都在、现在的不是记下的候选：要先验它的签名（假的过不了），现场留着
def _(b):
    make_app(b, 'claude', '4.1.0')
    put_state(b, 'claude', {'status': 'staged', 'message': 'm', 'checked_epoch': NOW - 60})


@ccase('recover/candidate-in-place-backup-present', ['check'], env=CLAUDE_ENV, before=journal(backup='4.1.0', candidate=True))   # 新程序已经到位：算装好了之前要验它的签名（假的过不了）
def _(b):
    make_app(b, 'claude', '4.2.0')
    put_state(b, 'claude', {'status': 'staged', 'message': 'm', 'checked_epoch': NOW - 60})


@ccase('recover/candidate-in-place-no-metadata', ['check'], env=CLAUDE_ENV, before=journal(backup='4.1.0', candidate=True, edit=lambda d, b: d.pop('metadata')))
def _(b):
    make_app(b, 'claude', '4.2.0')


@ccase('recover/target-changed-no-backup', ['check'], env=CLAUDE_ENV, before=both(journal(incoming='4.2.0'), lambda b: make_app(b, 'claude', '4.3.0') and None))
def _(b):
    make_app(b, 'claude', '4.1.0')


@ccase('recover/only-the-backup-is-left', ['check'], env=CLAUDE_ENV,   # 旧程序移走了、新的还没到位：要验备份的签名才放回去（假的过不了）
       before=both(journal(backup='4.1.0'), lambda b: os.rename(b.path(CLAUDE_APP), b.path('Applications/moved-away'))))
def _(b):
    make_app(b, 'claude', '4.1.0')


@ccase('recover/neither-target-nor-backup', ['check'], env=CLAUDE_ENV, before=both(journal(), lambda b: os.rename(b.path(CLAUDE_APP), b.path('Applications/moved-away'))))
def _(b):
    make_app(b, 'claude', '4.1.0')


@ccase('recover/client-is-open', ['check'], env=CLAUDE_ENV, before=both(journal(incoming='4.2.0'), running(CLAUDE_APP + '/Contents/MacOS/Claude')))
def _(b):
    make_app(b, 'claude', '4.1.0')


@ccase('recover/candidate-is-open', ['check'], env=CLAUDE_ENV,
       before=both(journal(incoming='4.2.0'), running('Applications/.cca-client-install-abc_123/Claude.app/Contents/MacOS/Claude')))
def _(b):
    make_app(b, 'claude', '4.1.0')


@ccase('recover/journal-and-not-installed', ['check'], before=journal())   # 环境里说没装，但有事务记录：照样恢复
def _(b):
    make_app(b, 'claude', '4.1.0')


@ccase('recover/codex', ['check'], env=CODEX_ENV, before=journal('codex', incoming='2.0'))
def _(b):
    make_app(b)


JOURNAL_EDITS = [
    ('v-2', lambda d, b: d.update(v=2)),
    ('v-true', lambda d, b: d.update(v=True)),
    ('no-target', lambda d, b: d.pop('target')),
    ('target-null', lambda d, b: d.update(target=None)),
    ('target-list', lambda d, b: d.update(target=[])),
    ('target-path-number', lambda d, b: d['target'].update(path=5)),
    ('target-path-relative', lambda d, b: d['target'].update(path='Applications/Claude.app')),
    ('target-path-not-real', lambda d, b: d['target'].update(path=d['target']['path'] + '/../Claude.app')),
    ('target-path-with-nul', lambda d, b: d['target'].update(path=d['target']['path'] + '\x00')),
    ('no-work', lambda d, b: d.pop('work')),
    ('work-empty', lambda d, b: d.update(work='')),
    ('work-number', lambda d, b: d.update(work=5)),
    ('work-zero', lambda d, b: d.update(work=0)),
    ('work-elsewhere', lambda d, b: d.update(work=b.path('.cca-client-install-abc_123'))),
    ('work-bad-name', lambda d, b: d.update(work=os.path.join(os.path.dirname(d['work']), '.cca-client-install-ABC'))),
    ('work-name-with-newline', lambda d, b: d.update(work=d['work'] + '\n')),
    ('backup-elsewhere', lambda d, b: d.update(backup=d['backup'] + '2')),
    ('no-backup', lambda d, b: d.pop('backup')),
    ('incoming-elsewhere', lambda d, b: d.update(incoming=os.path.join(d['work'], 'ChatGPT.app'))),
    ('no-incoming', lambda d, b: d.pop('incoming')),
    ('target-build-missing', lambda d, b: d['target'].pop('build')),
    ('target-differs', lambda d, b: d['target'].update(version='0.1')),
    ('no-metadata', lambda d, b: d.pop('metadata')),
    ('no-candidate', lambda d, b: d.pop('candidate')),
]
for name, edit in JOURNAL_EDITS:
    @ccase('recover/journal/' + name, ['check'], env=CLAUDE_ENV, before=journal(incoming='4.2.0', edit=edit))
    def _(b):
        make_app(b, 'claude', '4.1.0')


for name, text in [('not-json', '{oops'), ('list', '[1]'), ('empty', '')]:
    @ccase('recover/journal-file/' + name, ['check'], env=CLAUDE_ENV)
    def _(b, text=text):
        make_app(b, 'claude', '4.1.0')
        updater_dirs(b, 'claude')
        b.write('.cca/client-updates/claude/transaction.json', text)


@ccase('recover/journal-file/mode-644', ['check'], env=CLAUDE_ENV, before=both(journal(incoming='4.2.0'), lambda b: os.chmod(b.path('.cca/client-updates/claude/transaction.json'), 0o644)))
def _(b):
    make_app(b, 'claude', '4.1.0')


@ccase('recover/work-directory-mode-755', ['check'], env=CLAUDE_ENV, before=both(journal(incoming='4.2.0'), lambda b: os.chmod(b.path('Applications/.cca-client-install-abc_123'), 0o755)))
def _(b):
    make_app(b, 'claude', '4.1.0')


@ccase('recover/work-directory-missing', ['check'], env=CLAUDE_ENV, before=both(journal(), lambda b: os.rmdir(b.path('Applications/.cca-client-install-abc_123'))))
def _(b):
    make_app(b, 'claude', '4.1.0')


@ccase('recover/cli-journal-reaches-missing-attribute', ['check'], env=CLI_ENV)   # 终端工具的目录里有一份像样的事务记录：原来在这里是没人接的 AttributeError
def _(b):
    make_tool(b, version='unreadable\n')   # 就算恢复那一步放它过去，也读不出版本、到不了联网那一步
    updater_dirs(b, 'claude-cli')
    os.makedirs(b.path('Applications/.cca-client-install-abc'), mode=0o700)
    app = b.path('Applications/Claude.app')
    os.makedirs(app)
    b.json('.cca/client-updates/claude-cli/transaction.json', {'v': 1, 'target': {'path': app}, 'work': b.path('Applications/.cca-client-install-abc'),
                                                                'backup': b.path('Applications/.cca-client-install-abc/previous.app')})
