"""程序坞整理（cli/src/cmd/dock.rs）：dock-tidy。

程序坞的设置是沙箱里的文件（CCA_DOCK_PREFS；不设的话沙箱里什么都不做：HOME 换过就不碰这个用户真正的设置）。
ps、lsappinfo、killall 都是沙箱里假的：ps 给出 ps-comm.txt，lsappinfo 第一次给 lsappinfo-1.txt、以后给 lsappinfo.txt，
killall 只记一笔。CCA_DOCK_SETTLE=0：不等。重启以后又被加回来的那几条路（要假的 killall 去改设置）由整套回归覆盖。"""
import os
import plistlib

from run import case
from cases.table import world

PS = 'printf \'%s\\n\' "ps $*" >> "$HOME/calls.log"\ncat "$HOME/ps-comm.txt" 2>/dev/null\nexit 0\n'
LSAPPINFO = ('printf \'%s\\n\' "lsappinfo $*" >> "$HOME/calls.log"\n'
             'if [ -f "$HOME/lsappinfo-1.txt" ]; then cat "$HOME/lsappinfo-1.txt"; rm -f "$HOME/lsappinfo-1.txt"; else cat "$HOME/lsappinfo.txt" 2>/dev/null; fi\nexit 0\n')
KILLALL = 'printf \'%s\\n\' "killall $*" >> "$HOME/calls.log"\nexit 0\n'
ENV = {'CCA_DOCK_PREFS': '%HOME%/dock/dock.plist', 'CCA_DOCK_SETTLE': '0'}
PID = [(r'exited\.\d+', 'exited.<pid>')]


def app(b, name, bid=True, exe=True):
    info = {}
    if bid:
        info['CFBundleIdentifier'] = 'com.example.%s' % name.lower()
    if exe:
        info['CFBundleExecutable'] = name
    b.write('dock/%s.app/Contents/Info.plist' % name, plistlib.dumps(info), 0o644)
    return b.path('dock/%s.app' % name)


def tile(b, name, bid=True, guid=1):
    data = {'file-data': {'_CFURLString': 'file://%s/' % b.path('dock/%s.app' % name).replace(' ', '%20'), '_CFURLStringType': 15},
            'file-label': name, 'book': b'bookmark\x00' * 3}
    if bid:
        data['bundle-identifier'] = 'com.example.%s' % name.lower()
    return {'GUID': guid, 'tile-data': data, 'tile-type': 'file-tile'}


def dock(b, recent, pinned=(), fmt=plistlib.FMT_BINARY, **more):
    """一份程序坞的设置；recent / pinned 是 tile(…) 的结果"""
    prefs = dict({'recent-apps': list(recent), 'persistent-apps': list(pinned), 'tilesize': 48.0, 'mod-count': 7, 'orientation': 'bottom'}, **more)
    b.write('dock/dock.plist', plistlib.dumps(prefs, fmt=fmt), 0o600)


def scene(b, running=(), exited_first='', exited='', cfg=None):
    world(b, cfg=cfg)
    b.fake('ps', PS)
    b.fake('lsappinfo', LSAPPINFO)
    b.fake('killall', KILLALL)
    for name in ('A', 'B', 'S', 'Other App'):
        app(b, name)
    b.write('ps-comm.txt', ''.join('%s\n' % p for p in running))
    if exited_first:
        b.write('lsappinfo-1.txt', exited_first)
    b.write('lsappinfo.txt', exited)


def exe(b, name):
    return b.path('dock/%s.app/Contents/MacOS/%s' % (name, name))


def tidy(name, *args, env=None, **kw):
    return case('dock-tidy/' + name, ['dock-tidy'] + list(args), env=dict(ENV, **(env or {})), mask=PID, **kw)


@tidy('removes-leftover-icon', '%HOME%/dock/A.app')
def _(b):
    scene(b)
    dock(b, [tile(b, 'A'), tile(b, 'B', guid=2)])


@tidy('removes-by-path-when-tile-has-no-bundle-id', '%HOME%/dock/A.app')
def _(b):
    scene(b)
    dock(b, [tile(b, 'A', bid=False), tile(b, 'Other App', bid=False, guid=2)])


@tidy('running-instance-keeps-its-icon', '%HOME%/dock/A.app')
def _(b):
    scene(b, running=['/sbin/launchd', '%s' % 'placeholder'])
    b.write('ps-comm.txt', '/sbin/launchd\n  %s  \n' % exe(b, 'A'))
    dock(b, [tile(b, 'A')])


@tidy('two-icons-one-instance', '%HOME%/dock/S.app')
def _(b):
    scene(b)
    b.write('ps-comm.txt', '%s\n' % exe(b, 'S'))
    dock(b, [tile(b, 'S'), tile(b, 'S', guid=2), tile(b, 'B', guid=3)])


@tidy('pinned-app-running-needs-no-recent-icon', '%HOME%/dock/S.app')
def _(b):
    scene(b)
    b.write('ps-comm.txt', '%s\n' % exe(b, 'S'))
    dock(b, [tile(b, 'S')], pinned=[tile(b, 'S', guid=9)])


@tidy('other-running-apps-lose-their-recent-icon-too', '%HOME%/dock/A.app')
def _(b):
    scene(b)
    b.write('ps-comm.txt', '%s\n%s\n' % (exe(b, 'B'), exe(b, 'B')))
    dock(b, [tile(b, 'A'), tile(b, 'B', guid=2), tile(b, 'Other App', guid=3)])


@tidy('nothing-left-over', '%HOME%/dock/A.app')
def _(b):
    scene(b)
    dock(b, [tile(b, 'B')])


@tidy('recents-switched-off', '%HOME%/dock/A.app')
def _(b):
    scene(b)
    dock(b, [tile(b, 'A')], **{'show-recents': False})


@tidy('recents-switched-on', '%HOME%/dock/A.app')
def _(b):
    scene(b)
    dock(b, [tile(b, 'A')], **{'show-recents': True})


@tidy('disabled-in-config', '%HOME%/dock/A.app')
def _(b):
    scene(b, cfg={'dock_tidy': False})
    dock(b, [tile(b, 'A')])


@tidy('no-app-argument')
def _(b):
    scene(b)
    dock(b, [tile(b, 'A')])


@tidy('xml-prefs-become-binary', '%HOME%/dock/A.app')
def _(b):
    scene(b)
    dock(b, [tile(b, 'A')], fmt=plistlib.FMT_XML)


@tidy('prefs-file-is-garbage', '%HOME%/dock/A.app')
def _(b):
    scene(b)
    b.write('dock/dock.plist', 'not a plist at all')


@tidy('prefs-file-missing', '%HOME%/dock/A.app')
def _(b):
    scene(b)


@tidy('odd-prefs', '%HOME%/dock/A.app')
def _(b):
    scene(b)
    dock(b, ['junk', {'tile-data': 'x'}, {'tile-data': {'file-data': {'_CFURLString': 'https://example.com/'}}}, tile(b, 'A')], pinned='not a list')


@tidy('app-without-info-plist', '%HOME%/dock/Gone.app')
def _(b):
    scene(b)
    dock(b, [{'tile-data': {'file-data': {'_CFURLString': 'file://%s/' % b.path('dock/Gone.app')}}}, tile(b, 'B', guid=2)])


@tidy('with-lockdir', '%HOME%/dock/A.app', '%HOME%/dock/cca-dock')
def _(b):
    scene(b)
    dock(b, [tile(b, 'A'), tile(b, 'B', guid=2)])


@tidy('lockdir-notes-from-others-are-taken-along', '%HOME%/dock/A.app', '%HOME%/dock/cca-dock')
def _(b):
    scene(b)
    dock(b, [tile(b, 'A'), tile(b, 'B', guid=2), tile(b, 'S', guid=3)])
    b.write('dock/cca-dock/exited', '%s\n\n%s\n' % (b.path('dock/B.app'), b.path('dock/A.app')))
    os.chmod(b.path('dock/cca-dock'), 0o700)


@tidy('lockdir-cannot-be-made', '%HOME%/dock/A.app', '%HOME%/dock/blocked/cca-dock')
def _(b):
    scene(b)
    dock(b, [tile(b, 'A')])
    b.write('dock/blocked', 'a file in the way')


EXITED = ''' 1) "Finder" ASN:0x0-0x1:
    bundleID="com.apple.finder"
    bundle path="/System/Library/CoreServices/Finder.app"
 2) "A" ASN:0x0-0x2: (exited-with-subordinates)
    bundleID="com.example.a"
    bundle path="%s"
'''


@tidy('waits-while-launchservices-still-has-the-record', '%HOME%/dock/A.app', timeout=30)
def _(b):
    scene(b)
    b.write('lsappinfo-1.txt', EXITED % b.path('dock/A.app'))
    dock(b, [tile(b, 'A')])


@tidy('exited-record-for-another-app-goes-too', '%HOME%/dock/A.app')
def _(b):
    scene(b)
    b.write('lsappinfo.txt', EXITED.replace('"A"', '"B"').replace('com.example.a', 'com.example.b') % b.path('dock/B.app'))
    dock(b, [tile(b, 'A'), tile(b, 'B', guid=2), tile(b, 'S', guid=3)])


@case('dock-tidy/real-domain-is-left-alone-in-a-sandbox', ['dock-tidy', '%HOME%/dock/A.app'], env={'CCA_DOCK_SETTLE': '0'})
def _(b):
    scene(b)
    dock(b, [tile(b, 'A')])
