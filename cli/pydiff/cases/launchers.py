"""启动器和图标、钥匙串的服务名（cli/src/cmd/launchers.rs）。

画图标要起 osascript、sips、iconutil：沙箱里放假的，把收到的参数记进 calls.log（临时目录的名字每次不一样，遮掉）。
图标真的换了以后原实现会调用系统的 lsregister（绝对路径，假不了）：这里的用例都让图标“内容没变”，不去动本机的
LaunchServices；那一步由整套回归覆盖。"""
import hashlib
import json
import plistlib

from run import case
from cases.router import ident
from cases.table import world

BADGE_COLORS = ['#0A84FF', '#30A46C', '#8E4EC6', '#12A594', '#D6409F', '#3E63DD', '#E5484D', '#B2610B']
SRC = b'ORIGINAL ICON BYTES'
FAKE_ICNS = b'fake icns\n'
TMP = [(r'/tmp[a-z0-9_]{8}/', '/tmp<随机>/')]
ARGS = ['%HOME%/Launchers', '%HOME%/.local/bin/cca', '%HOME%/apps/Claude.app']


def fakes(b, osascript_ok=True):
    log = 'printf \'%s\\n\' "$0 $*" >> "$HOME/calls.log"\n'
    b.fake('osascript', log + ('printf \'PNG %s %s\' "$6" "$7" > "$5"\n' if osascript_ok else 'exit 1\n'))
    b.fake('sips', log + 'cp "$4" "$6"\n')
    b.fake('iconutil', log + 'printf \'fake icns\\n\' > "$5"\n')


def app(b, icon='electron', fmt=plistlib.FMT_XML, info=None, src=SRC):
    """本机装着的 App：Info.plist 里写着图标的名字"""
    data = {'CFBundleIconFile': icon, 'CFBundleIdentifier': 'com.example.claude'} if info is None else info
    b.write('apps/Claude.app/Contents/Info.plist', plistlib.dumps(data, fmt=fmt), 0o644)
    if src is not None:
        name = icon if isinstance(icon, str) and icon.endswith('.icns') else '%s.icns' % icon
        b.write('apps/Claude.app/Contents/Resources/%s' % name, src, 0o644)


def digest(label, color, src=SRC):
    return hashlib.sha1(src + ('|%s|%s|2' % (label, color)).encode('utf-8')).hexdigest()


def color_of(email_or_id, free=BADGE_COLORS):
    return free[int(hashlib.sha1(ident(email_or_id).encode('utf-8')).hexdigest(), 16) % len(free)]


def launcher(b, name, key, acct, tool=None, icon=None, icns=None, badge=('B', '#000000'), where='Launchers'):
    """沙箱里已有的一个 cca 启动器"""
    mark = {'id': ident(acct), 'key': key, 'badge': list(badge)}
    if tool:
        mark['tool'] = tool
    if icon:
        mark['icon'] = icon
    root = '%s/%s.app/Contents' % (where, name)
    b.write(root + '/Resources/cca-launcher.json', json.dumps(mark, ensure_ascii=False), 0o644)
    b.write(root + '/MacOS/cca-launcher', '#!/bin/bash\n# old\n', 0o755)
    b.write(root + '/Info.plist', '<old/>', 0o644)
    if icns is not None:
        b.write(root + '/Resources/AppIcon.icns', icns, 0o644)


ONE = {'bob@example.com': 'US-B'}


@case('launchers/create-without-app-icon', ['launchers'] + ARGS, mask=TMP)
def _(b):
    world(b)
    fakes(b)


@case('launchers/create-app-plist-unreadable', ['launchers'] + ARGS, mask=TMP)
def _(b):
    world(b, table=ONE)
    fakes(b)
    b.write('apps/Claude.app/Contents/Info.plist', 'not a plist', 0o644)


@case('launchers/create-icon-file-missing', ['launchers'] + ARGS, mask=TMP)
def _(b):
    world(b, table=ONE)
    fakes(b)
    app(b, src=None)


@case('launchers/icon-redrawn-same-content', ['launchers'] + ARGS, mask=TMP)
def _(b):
    world(b, table=ONE)
    fakes(b)
    app(b)
    launcher(b, 'Claude bob', 'bob@example.com', 'bob@example.com', icon='0' * 40, icns=FAKE_ICNS)


@case('launchers/icon-up-to-date', ['launchers'] + ARGS, mask=TMP)
def _(b):
    world(b, table=ONE)
    fakes(b)
    app(b, icon='AppIcon.icns', fmt=plistlib.FMT_BINARY)
    launcher(b, 'Claude bob', 'bob@example.com', 'bob@example.com', icon=digest('B', color_of('bob@example.com')), icns=b'whatever is there')


@case('launchers/render-fails-uses-original', ['launchers'] + ARGS, mask=TMP)
def _(b):
    world(b, table=ONE)
    fakes(b, osascript_ok=False)
    app(b)
    launcher(b, 'Claude bob', 'bob@example.com', 'bob@example.com', icon='stale', icns=SRC)


@case('launchers/mark-not-json', ['launchers'] + ARGS, mask=TMP)
def _(b):
    world(b, table=ONE)
    fakes(b)
    app(b)
    launcher(b, 'Claude bob', 'bob@example.com', 'bob@example.com', icns=FAKE_ICNS)
    b.write('Launchers/Claude bob.app/Contents/Resources/cca-launcher.json', '{oops', 0o644)


@case('launchers/empty-icon-name', ['launchers'] + ARGS, mask=TMP)
def _(b):
    world(b, table=ONE)
    fakes(b)
    app(b, info={'CFBundleIdentifier': 'x'}, src=None)
    b.write('apps/Claude.app/Contents/Resources/.icns', SRC, 0o644)
    launcher(b, 'Claude bob', 'bob@example.com', 'bob@example.com', icon='stale', icns=FAKE_ICNS)


@case('launchers/rename-and-remove', ['launchers'] + ARGS, mask=TMP)
def _(b):
    world(b, table={'新名(bob@example.com)': 'US-B', 'carol@example.com': 'US-A'})
    fakes(b)
    launcher(b, 'Claude bob', 'bob@example.com', 'bob@example.com')
    launcher(b, 'Claude gone', 'gone@example.com', 'gone@example.com')
    launcher(b, 'ChatGPT erin', 'erin@example.com', 'erin@example.com', tool='codex')
    launcher(b, 'Antigravity gina', 'gina@example.com', 'gina@example.com', tool='agy')
    b.write('Launchers/Other.app/Contents/Info.plist', '<other/>', 0o644)
    b.write('Launchers/not-an-app.txt', 'x')
    b.write('Launchers/Odd mark.app/Contents/Resources/cca-launcher.json', '[1]', 0o644)
    b.write('Launchers/Empty tool.app/Contents/Resources/cca-launcher.json', '{"tool": ""}', 0o644)


@case('launchers/foreign-same-name', ['launchers'] + ARGS, mask=TMP)
def _(b):
    world(b, table={'bob@example.com': 'US-B', 'carol@example.com': 'US-A'})
    fakes(b)
    b.write('Launchers/Claude bob.app/Contents/Info.plist', '<someone else/>', 0o644)
    b.write('Launchers/Claude carol.app', 'a plain file', 0o644)


for mode, argv in [('remove-all', ARGS + ['remove-all']), ('no-app', ARGS[:2] + ['']), ('no-app-arg', ARGS[:2]), ('remove-all-no-app', ARGS[:2] + ['', 'remove-all'])]:
    @case('launchers/%s' % mode, ['launchers'] + argv, mask=TMP)
    def _(b):
        world(b)
        fakes(b)
        launcher(b, 'Claude bob', 'bob@example.com', 'bob@example.com')
        launcher(b, 'Claude 主号', '主号(alice@example.com)', 'alice@example.com')
        launcher(b, 'ChatGPT erin', 'erin@example.com', 'erin@example.com', tool='codex')


@case('launchers/names', ['launchers'] + ARGS, mask=TMP)
def _(b):
    world(b, table={'bob@example.com': 'US-A', 'bob@example.org': 'US-A', 'bob(bob@example.net)': 'US-A', '中文名(c@example.com)': 'US-B', '中a(d@example.com)': 'US-B',
                    'a中(e@example.com)': 'US-B', 'a/b:c(f@example.com)': 'US-B', 'ßeta(g@example.com)': 'HK-A', '光别名': 'HK-A', '直连': {'base_url': 'https://api.example.com/v1'},
                    'ｆｕｌｌ(h@example.com)': 'HK-A', 'x1(i@example.com)': 'HK-A', 'x2(j@example.com)': 'HK-A'})
    fakes(b)


@case('launchers/order-base-keeps-colors', ['launchers'] + ARGS, mask=TMP)
def _(b):
    world(b, table={'carol@example.com': 'US-A', 'bob@example.com': 'US-B', '主号(alice@example.com)': 'US-A'},
          cfg={'order_base': [ident('alice@example.com'), ident('bob@example.com')]})
    fakes(b)


@case('launchers/no-config', ['launchers'] + ARGS, mask=TMP)
def _(b):
    b.table(ONE)
    fakes(b)


@case('launchers/no-table', ['launchers'] + ARGS, mask=TMP)
def _(b):
    fakes(b)
    launcher(b, 'Claude bob', 'bob@example.com', 'bob@example.com')


@case('launchers/codex', ['launchers', '%HOME%/Launchers', '%HOME%/bin dir/cca', '%HOME%/apps/Claude.app'], tool='codex', mask=TMP)
def _(b):
    world(b, codex={'erin@example.com': 'HK-A', "o'brien(frank@example.com)": 'US-A', '接口': {'base_url': 'https://api.example.com/v1'}})
    fakes(b)
    launcher(b, 'Claude bob', 'bob@example.com', 'bob@example.com')
    launcher(b, 'ChatGPT old', 'old@example.com', 'old@example.com', tool='codex')


@case('launchers/agy', ['launchers'] + ARGS, tool='agy', mask=TMP)
def _(b):
    world(b, agy={'gina@example.com': 'US-B'})
    fakes(b)


@case('launchers/symlinked-launcher', ['launchers'] + ARGS, mask=TMP)
def _(b):
    import os
    world(b, table={})
    fakes(b)
    launcher(b, 'Claude real', 'gone@example.com', 'gone@example.com', where='elsewhere')
    os.makedirs(b.path('Launchers'))
    os.symlink(b.path('elsewhere/Claude real.app'), b.path('Launchers/Claude link.app'))


@case('launchers/too-few-args', ['launchers', '%HOME%/Launchers'])
def _(b):
    world(b)


for i, (key, tool) in enumerate([('bob@example.com', 'claude'), ('主号(alice@example.com)', 'claude'), ('直连', 'claude'), ('走节点', 'claude'), ('erin@example.com', 'codex'),
                                 ('gina@example.com', 'agy'), ('', 'claude')]):
    @case('keychain/%02d' % i, ['keychain', key], tool=tool)
    def _(b):
        world(b)


@case('keychain/home-needs-nfc', ['keychain', 'bob@example.com'], env={'HOME': '%HOME%/café'})
def _(b):
    world(b)


@case('keychain/no-args', ['keychain'])
def _(b):
    pass
