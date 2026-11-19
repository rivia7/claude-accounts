"""资料目录、浏览器偏好、运行记录、命令行会话登记（cli/src/cmd/session.rs），和它们用到的 presence-wake / presence-stop。

ps 用沙箱里假的：按参数给出事先放好的内容（ps-lstart.txt、ps-command.txt），收到的参数和 LC_ALL / LANG 记进 calls.log。"""
import json
import os
import plistlib

from run import case
from cases.router import ident
from cases.table import world

BOB = 'bob@example.com'
SINCE = [(r'"since": [0-9]+\.[0-9]+', '"since": <现在>')]
PS = '''printf '%s LC_ALL=%s LANG=%s\\n' "ps $*" "$LC_ALL" "$LANG" >> "$HOME/calls.log"
case "$*" in
  *lstart*) cat "$HOME/ps-lstart.txt" 2>/dev/null ;;
  *command*) cat "$HOME/ps-command.txt" 2>/dev/null ;;
esac
exit 0
'''
T0 = 'Sat Nov  7 12:08:55 2026'      # 假的 ps 给的启动时间；夹具里登记的 started 照它算（Asia/Shanghai）
T0_EPOCH = 1794024535


def acct(b, key=BOB):
    return b.path('.cca/accounts/%s' % ident(key.split('(')[-1].rstrip(')')))


def ps(b, lstart=None, command=None):
    b.fake('ps', PS)
    if lstart is not None:
        b.write('ps-lstart.txt', lstart)
    if command is not None:
        b.write('ps-command.txt', command)


# --- private-profile ---
for kind in ('app', 'browser', 'other', ''):
    @case('private-profile/new-%s' % (kind or 'empty'), ['private-profile', BOB, kind])
    def _(b):
        world(b)


@case('private-profile/existing-loose-mode', ['private-profile', BOB, 'app'])
def _(b):
    world(b)
    os.makedirs(acct(b) + '/desktop', 0o755)
    os.chmod(acct(b) + '/desktop', 0o755)
    os.chmod(acct(b), 0o755)


@case('private-profile/account-dir-is-link', ['private-profile', BOB, 'app'])
def _(b):
    world(b)
    os.makedirs(b.path('elsewhere'))
    os.makedirs(b.path('.cca/accounts'))
    os.symlink(b.path('elsewhere'), acct(b))


@case('private-profile/profile-is-link', ['private-profile', BOB, 'browser'])
def _(b):
    world(b)
    os.makedirs(b.path('elsewhere'))
    os.makedirs(acct(b))
    os.symlink(b.path('elsewhere'), acct(b) + '/chrome')


@case('private-profile/codex', ['private-profile', 'erin@example.com', 'app'], tool='codex')
def _(b):
    world(b, codex={'erin@example.com': 'HK-A'})


@case('private-profile/no-args', ['private-profile'])
def _(b):
    pass


# --- browser-profile ---
GOOD = {'ip_handling_policy': 'disable_non_proxied_udp', 'ip_handling_url': [], 'local_ips_allowed_urls': []}


def prefs(b, name, data, mode=0o600):
    return b.write('.cca/accounts/%s/chrome/%s/Preferences' % (ident(BOB), name), data if isinstance(data, (str, bytes)) else json.dumps(data), mode)


PROFILES = [
    ('fresh', None), ('other-settings', {'Default': {'browser': {'x': 1}, 'webrtc': {'ip_handling_url': ['https://a.example.com'], 'kept': True}, 'z': None}}),
    ('two-profiles', {'Default': {'webrtc': GOOD}, 'Profile 1': {'a': 1}, 'Profile 2': {'webrtc': dict(GOOD, ip_handling_policy='default')}}),
    ('already-good', {'Default': {'webrtc': dict(GOOD), 'other': [1]}}), ('good-different-order', {'Default': {'webrtc': dict(reversed(list(GOOD.items())))}}),
    ('not-json', {'Default': '{oops'}), ('not-an-object', {'Default': '[1]'}), ('webrtc-not-an-object', {'Default': {'webrtc': 'x'}}),
    ('webrtc-null', {'Default': {'webrtc': None}}), ('empty-file', {'Default': ''}), ('no-default-only-other', {'Work': {'webrtc': GOOD}}),
    ('extra-webrtc-keys', {'Default': {'webrtc': dict(GOOD, multiple_routes_enabled=False)}}), ('url-exception', {'Default': {'webrtc': dict(GOOD, local_ips_allowed_urls=['*'])}}),
]
for action in ('prepare', 'check'):
    for name, layout in PROFILES:
        @case('browser-profile/%s-%s' % (action, name), ['browser-profile', action, BOB])
        def _(b, layout=layout):
            world(b)
            for profile, data in (layout or {}).items():
                prefs(b, profile, data)


for action in ('prepare', 'check'):
    @case('browser-profile/%s-preferences-is-link' % action, ['browser-profile', action, BOB])
    def _(b):
        world(b)
        b.write('shared/Preferences', json.dumps({'webrtc': GOOD}))
        os.makedirs(acct(b) + '/chrome/Default')
        os.symlink(b.path('shared/Preferences'), acct(b) + '/chrome/Default/Preferences')

    @case('browser-profile/%s-preferences-hard-linked' % action, ['browser-profile', action, BOB])
    def _(b):
        world(b)
        os.link(prefs(b, 'Default', {'webrtc': GOOD}), b.path('second-name'))

    @case('browser-profile/%s-folder-is-link' % action, ['browser-profile', action, BOB])
    def _(b):
        world(b)
        b.write('shared/Default/Preferences', json.dumps({'webrtc': GOOD}))
        os.makedirs(acct(b) + '/chrome')
        os.symlink(b.path('shared/Default'), acct(b) + '/chrome/Default')


@case('browser-profile/unknown-action', ['browser-profile', 'fix', BOB])
def _(b):
    world(b)


# --- session-open ---
def mark(b, pid, started=T0_EPOCH, name=None, **kw):
    j = {'v': 1, 'tool': 'claude', 'id': ident(BOB), 'pid': pid, 'started': started, 'since': 1794024000.5}
    j.update(kw)
    return b.write('.cca/accounts/%s/%s' % (ident(BOB), name or 'cli-%d.json' % pid), json.dumps(j))


@case('session-open/first', ['session-open', BOB, '4242'], mask=SINCE)
def _(b):
    world(b)
    os.makedirs(acct(b))
    ps(b, ' 4242 %s\n' % T0)


@case('session-open/cleans-stale-marks', ['session-open', BOB, '4242'], mask=SINCE)
def _(b):
    world(b)
    ps(b, ' 4242 %s\n  100 %s\n  101 %s\n  102 Sat Nov  7 12:08:56 2026\n  103 Sat Nov  7 12:09:30 2026\n  104 garbage here\n' % (T0, T0, T0))
    mark(b, 4242, started=5)                       # 自己这个进程号的旧登记：换掉
    mark(b, 100)                                   # 还在：留着
    mark(b, 101, started='x')                      # started 不是整数：删
    mark(b, 102)                                   # 差一秒：算同一个
    mark(b, 103)                                   # 进程号被别的进程用了：删
    mark(b, 104)                                   # ps 给的时间读不懂：删
    mark(b, 105)                                   # 进程没了：删
    mark(b, 106, started=True)                     # 布尔值不算整数：删
    b.write('.cca/accounts/%s/cli-107.json' % ident(BOB), '{oops')
    b.write('.cca/accounts/%s/cli-abc.json' % ident(BOB), 'not a mark')
    b.write('.cca/accounts/%s/device.json' % ident(BOB), '{}')
    mark(b, 100, name='cli-0100.json')             # 同一个进程号的两个文件名


@case('session-open/own-pid-unknown', ['session-open', BOB, '4242'], mask=SINCE)
def _(b):
    world(b)
    ps(b, '  100 %s\n' % T0)
    mark(b, 100)
    mark(b, 105)


@case('session-open/ps-missing', ['session-open', BOB, '4242'], mask=SINCE, env={'PATH': '%HOME%/nowhere'})
def _(b):
    world(b)
    mark(b, 105)


@case('session-open/no-account-dir', ['session-open', BOB, '4242'], mask=SINCE)
def _(b):
    world(b)
    ps(b, ' 4242 %s\n' % T0)


@case('session-open/bad-pid', ['session-open', BOB, 'abc'])
def _(b):
    world(b)


@case('session-open/codex', ['session-open', 'erin@example.com', '77'], tool='codex', mask=SINCE)
def _(b):
    world(b, codex={'erin@example.com': 'HK-A'})
    os.makedirs(b.path('.cca/codex/accounts/%s' % ident('erin@example.com')))
    ps(b, '   77 %s\n' % T0)


PIN = 'sha256//' + 'A' * 43 + '='
PRESENCE = {'url': 'https://192.0.2.10:18443/', 'pins': [PIN], 'salt': 'ab' * 16}
PUB = 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f cca-policy\n'
AGENT = 'printf \'%s EVENTS=[%s] TOOL=[%s] RO=[%s] KEEP=[%s]\\n\' "cca $*" "$CCA_EVENTS" "$CCA_TOOL" "$CCA_RO" "$KEEP_ME" >> "$HOME/calls.log"\n'


def mapped(b, policy=None, key=True, pub=PUB, cfg=None):
    world(b, cfg=cfg, policy={'version': 1, 'claude': {'BOB@example.com': 'US-B'}, 'presence': PRESENCE} if policy is None else policy)
    os.makedirs(acct(b), exist_ok=True)
    ps(b, ' 4242 %s\n' % T0)
    if pub is not None:
        b.write('.cca/policy/id_ed25519.pub', pub, 0o644)
    if key:
        b.write('.cca/policy/id_ed25519', 'PRIVATE KEY PLACEHOLDER\n')
    b.write('.cca/bin/cca', '#!/bin/sh\n' + AGENT, 0o755)


WAKES = [
    ('wakes-agent', {}), ('presence-off', {'cfg': {'presence': False}}), ('presence-zero-is-not-off', {'cfg': {'presence': 0}}),
    ('not-mapped', {'policy': {'version': 1, 'claude': {'other@example.com': 'US-B'}, 'presence': PRESENCE}}),
    ('no-presence-section', {'policy': {'version': 1, 'claude': {'bob@example.com': 'US-B'}}}),
    ('bad-presence-section', {'policy': {'version': 1, 'claude': {'bob@example.com': 'US-B'}, 'presence': dict(PRESENCE, salt='zz')}}),
    ('no-private-key', {'key': False}), ('no-public-key', {'pub': None}), ('rsa-key', {'pub': PUB.replace('ssh-ed25519', 'ssh-rsa')}),
    ('public-key-not-base64', {'pub': 'ssh-ed25519 not*base64 x\n'}), ('public-key-one-field', {'pub': 'ssh-ed25519\n'}),
    ('public-key-not-utf8', {'pub': b'ssh-ed25519 \xff\xfe x\n'}),
]
for name, kw in WAKES:
    @case('session-open/presence-%s' % name, ['session-open', BOB, '4242'], mask=SINCE, settle=0.6,
          env={'CCA_EVENTS': '/dev/null', 'CCA_RO': '', 'KEEP_ME': 'yes'})
    def _(b, kw=kw):
        mapped(b, **kw)

    @case('presence-wake/%s' % name, ['presence-wake', BOB], settle=0.6, env={'CCA_EVENTS': '/dev/null', 'KEEP_ME': 'yes'})
    def _(b, kw=kw):
        mapped(b, **kw)


@case('presence-wake/api-account', ['presence-wake', '直连'], settle=0.3)
def _(b):
    mapped(b)


@case('presence-wake/codex-not-in-codex-mapping', ['presence-wake', BOB], tool='codex', settle=0.3)
def _(b):
    mapped(b)


# --- presence-stop（只测不发信号的几条路：进程不在、记录不对）---
STOPS = [('no-record', None), ('not-json', '{oops'), ('pid-not-int', {'pid': '77', 'started': T0_EPOCH}), ('started-zero', {'pid': 77, 'started': 0}),
         ('started-float', {'pid': 77, 'started': 1.5}), ('process-gone', {'pid': 77, 'started': T0_EPOCH}), ('pid-reused', {'pid': 4242, 'started': T0_EPOCH - 500}),
         ('pid-bool', {'pid': True, 'started': T0_EPOCH})]
for name, rec in STOPS:
    @case('presence-stop/%s' % name, ['presence-stop'])
    def _(b, rec=rec):
        world(b)
        ps(b, ' 4242 %s\n' % T0)
        if rec is not None:
            b.write('.cca/presence/agent.json', rec if isinstance(rec, str) else json.dumps(rec))


# --- runtime-record ---
PROXY = 'http://192.0.2.10:7902'
BYPASS = 'localhost;127.0.0.1;<local>'


def running(b, kind='app', command=None, lstart=' 4242 %s\n' % T0, info=None, binary=True, table=None, fmt=plistlib.FMT_XML):
    """一个开着的客户端：账号表、资料目录、App 包、ps 的输出"""
    world(b, table=table)
    profile = acct(b) + ('/desktop' if kind == 'app' else '/chrome')
    os.makedirs(profile, 0o700)
    app = b.path('apps/Claude.app')
    b.write('apps/Claude.app/Contents/Info.plist', plistlib.dumps({'CFBundleExecutable': 'Claude', 'CFBundleIdentifier': 'com.example.claude'} if info is None else info, fmt=fmt)
            if not isinstance(info, bytes) else info, 0o644)
    if binary:
        b.write('apps/Claude.app/Contents/MacOS/Claude', '#!/bin/sh\n', 0o755)
    extra = '--proxy-bypass-list=%s' % BYPASS if kind == 'app' else '--webrtc-ip-handling-policy=disable_non_proxied_udp'
    default = '%s/Contents/MacOS/Claude --user-data-dir=%s --proxy-server=%s %s --no-first-run' % (app, profile, PROXY, extra)
    ps(b, lstart, ' 4242 %s\n' % (default if command is None else command.replace('%APP%', app).replace('%PROFILE%', profile)))
    if kind == 'browser':
        prefs(b, 'Default', {'webrtc': GOOD})


def rr(action, kind='app', pid='4242', profile=None, app='%HOME%/apps/Claude.app', proxy=PROXY, rest=('router', BYPASS, 'localhost,127.0.0.1')):
    profile = profile or '%%HOME%%/.cca/accounts/%s/%s' % (ident(BOB), 'desktop' if kind == 'app' else 'chrome')
    return ['runtime-record', action, BOB, kind, pid, profile, app, proxy] + list(rest)


@case('runtime-record/write-app', rr('write'))
def _(b):
    running(b)


@case('runtime-record/write-app-binary-plist', rr('write'))
def _(b):
    running(b, fmt=plistlib.FMT_BINARY)


@case('runtime-record/write-browser', rr('write', 'browser', rest=('local',)))
def _(b):
    running(b, 'browser')


@case('runtime-record/write-minimal-args', rr('write', rest=()))
def _(b):
    running(b, command='%APP%/Contents/MacOS/Claude --user-data-dir=%PROFILE% --proxy-server=' + PROXY + ' --proxy-bypass-list=')


@case('runtime-record/write-replaces-link', rr('write'))
def _(b):
    running(b)
    b.write('shared/app-runtime.json', '{"shared": true}')
    os.symlink(b.path('shared/app-runtime.json'), acct(b) + '/app-runtime.json')


BAD_RUNS = [
    ('pid-gone', {'lstart': '  999 %s\n' % T0}, {}), ('lstart-unreadable', {'lstart': ' 4242 yesterday\n'}, {}), ('no-proxy', {}, {'proxy': ''}),
    ('bad-pid', {}, {'pid': 'x'}), ('profile-mismatch', {}, {'profile': '%HOME%/.cca/accounts'}), ('unknown-kind', {}, {'kind': 'tab'}),
    ('flag-missing', {'command': '%APP%/Contents/MacOS/Claude --user-data-dir=%PROFILE% --proxy-server=' + PROXY}, {}),
    ('flag-twice', {'command': '%APP%/Contents/MacOS/Claude --user-data-dir=%PROFILE% --user-data-dir=/x --proxy-server=' + PROXY + ' --proxy-bypass-list=' + BYPASS}, {}),
    ('flag-twice-adjacent-counts-once', {'command': '%APP%/Contents/MacOS/Claude --user-data-dir --user-data-dir=%PROFILE% --proxy-server=' + PROXY + ' --proxy-bypass-list=' + BYPASS}, {}),
    ('flag-other-value', {'command': '%APP%/Contents/MacOS/Claude --user-data-dir=%PROFILE% --proxy-server=http://192.0.2.99:1 --proxy-bypass-list=' + BYPASS}, {}),
    ('flag-value-prefix', {'command': '%APP%/Contents/MacOS/Claude --user-data-dir=%PROFILE%-evil --proxy-server=' + PROXY + ' --proxy-bypass-list=' + BYPASS}, {}),
    ('flag-space-form', {'command': '%APP%/Contents/MacOS/Claude --user-data-dir %PROFILE% --proxy-server=' + PROXY + ' --proxy-bypass-list=' + BYPASS}, {}),
    ('flag-at-end', {'command': '%APP%/Contents/MacOS/Claude --proxy-server=' + PROXY + ' --proxy-bypass-list=' + BYPASS + ' --user-data-dir=%PROFILE%'}, {}),
    ('other-binary', {'command': '/Applications/Other.app/Contents/MacOS/Claude --user-data-dir=%PROFILE% --proxy-server=' + PROXY + ' --proxy-bypass-list=' + BYPASS}, {}),
    ('binary-prefix-only', {'command': '%APP%/Contents/MacOS/Claude-helper --user-data-dir=%PROFILE% --proxy-server=' + PROXY + ' --proxy-bypass-list=' + BYPASS}, {}),
    ('no-command', {'command': ''}, {}), ('app-missing', {}, {'app': '%HOME%/apps/Nope.app'}), ('binary-missing', {'binary': False}, {}),
    ('plist-garbage', {'info': b'not a plist'}, {}), ('plist-not-dict', {'info': ['x']}, {}), ('executable-missing', {'info': {'CFBundleName': 'x'}}, {}),
    ('executable-not-string', {'info': {'CFBundleExecutable': 5}}, {}), ('executable-has-slash', {'info': {'CFBundleExecutable': '../Claude'}}, {}),
    ('executable-dot', {'info': {'CFBundleExecutable': '.'}}, {}), ('executable-empty', {'info': {'CFBundleExecutable': ''}}, {}),
    ('account-unknown', {'table': {'主号(alice@example.com)': 'US-A'}}, {}),
]
for name, fixture, args in BAD_RUNS:
    @case('runtime-record/bad-%s' % name, rr('write', **args))
    def _(b, fixture=fixture):
        running(b, **fixture)


@case('runtime-record/unknown-action', rr('update'))
def _(b):
    running(b)


@case('runtime-record/browser-prefs-changed', rr('write', 'browser'))
def _(b):
    running(b, 'browser')
    prefs(b, 'Default', {'webrtc': dict(GOOD, ip_handling_policy='default')})


def saved(b, kind='app', **change):
    """照 write 会写出的样子放一份运行记录（app_version 现看文件）"""
    app = b.path('apps/Claude.app')
    st, bst = os.stat(app + '/Contents/Info.plist'), os.stat(app + '/Contents/MacOS/Claude')
    profile = acct(b) + ('/desktop' if kind == 'app' else '/chrome')
    rec = {'v': 1, 'tool': 'claude', 'id': ident(BOB), 'kind': kind, 'pid': 4242, 'started': T0_EPOCH, 'profile': profile,
           'config': acct(b) + '/claude', 'app': app, 'app_version': [st.st_size, st.st_mtime_ns, bst.st_size, bst.st_mtime_ns], 'proxy': PROXY, 'route': 'router',
           'node': 'US-B', 'bypass': BYPASS, 'no_proxy': 'localhost,127.0.0.1', 'launch_policy': 1}
    rec.update(change)
    return b.write('.cca/accounts/%s/%s-runtime.json' % (ident(BOB), kind), json.dumps(rec))


CHECKS = [('same', {}), ('node-changed', {'node': 'US-A'}), ('restarted', {'started': T0_EPOCH - 60}), ('app-updated', {'app_version': [1, 2, 3, 4]}),
          ('extra-field', {'more': 1}), ('float-pid-still-equal', {'pid': 4242.0}), ('route-changed', {'route': 'local'})]
for name, change in CHECKS:
    @case('runtime-record/check-%s' % name, rr('check'))
    def _(b, change=change):
        running(b)
        saved(b, **change)


@case('runtime-record/check-no-record', rr('check'))
def _(b):
    running(b)


@case('runtime-record/check-record-is-link', rr('check'))
def _(b):
    running(b)
    os.rename(saved(b), b.path('moved.json'))
    os.symlink(b.path('moved.json'), acct(b) + '/app-runtime.json')


@case('runtime-record/check-record-hard-linked', rr('check'))
def _(b):
    running(b)
    os.link(saved(b), b.path('second-name.json'))


@case('runtime-record/check-key-order-differs', rr('check'))
def _(b):
    running(b)
    p = saved(b)
    rec = json.load(open(p))
    with open(p, 'w') as f:
        json.dump(dict(reversed(list(rec.items()))), f)


@case('runtime-record/too-few-args', ['runtime-record', 'write', BOB, 'app'])
def _(b):
    world(b)
