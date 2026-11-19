"""bash 里原来内嵌的几小段 Python（cli/src/cmd/shell.rs 的 sh-…）。对照的一边就是原来那一小段（原样抄在这里，
改了 cca 里的要跟着改），cca-core 的一边是新的子命令。"""
import json
import sys

from run import case

PY = [sys.executable, '-I', '-X', 'utf8', '-c']

AUTH_EMAIL = '''
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    d = {}
print((d.get("email") or "") if d.get("loggedIn") else "")'''
OAUTH_DROP = 'import json,sys; p=sys.argv[1]; j=json.load(open(p)); j.pop("oauthAccount",None); json.dump(j,open(p,"w"),indent=2)'
RE_ESCAPE = 'import re, sys; print(re.escape(sys.argv[1]))'
PS_TREE = '''import sys
root = int(sys.argv[1])
rows = [tuple(map(int, line.split())) for line in sys.stdin if len(line.split()) == 2]
pids = {root}
while True:
    found = {pid for pid, parent in rows if parent in pids}
    if found <= pids:
        break
    pids.update(found)
print(",".join(map(str, sorted(pids))))'''
LSOF_UNDER = '''import sys
prefix = "n" + sys.argv[1] + "/"
sys.exit(0 if any(line.startswith(prefix) for line in sys.stdin) else 1)'''
CLIENTS_TEXT = ('import json,sys; d=json.load(sys.stdin); print("客户端自动检查：" + ("开启" if d.get("mode") == "auto" else "关闭") + "；允许自动更新：" + '
                '("开启（查到新版本就装，客户端开着时等它退出）" if d.get("auto_install") else "关闭（安装前先问你）")); '
                '[print(c.get("label", c.get("id", "")) + "：" + c.get("message", c.get("status", "")) + '
                '("\\n安装：cca clients install " + c["install_token"] if c.get("can_install") and c.get("install_token") else "")) '
                'for c in d.get("clients", []) if c.get("status") != "missing"]')
DOCK_WAIT = '''import fcntl, os, select, sys
pid, d, argv = int(sys.argv[1]), sys.argv[2], sys.argv[3:]
lock = None
if d:
    try:
        os.makedirs(d, 0o700, exist_ok=True)
        lock = open(os.path.join(d, "%d.wait" % pid), "a")
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        sys.exit(0)
    except OSError:
        lock = None
kq = select.kqueue()
try:
    kq.control([select.kevent(pid, select.KQ_FILTER_PROC, select.KQ_EV_ADD | select.KQ_EV_ONESHOT, select.KQ_NOTE_EXIT)], 0, 0)
    kq.control(None, 1)
except OSError:
    pass
if lock:
    try:
        os.unlink(lock.name)
    except OSError:
        pass
try:
    os.execv(argv[0], argv)
except OSError:
    pass'''


def nothing(b):
    pass


for i, text in enumerate([{'loggedIn': True, 'email': 'bob@example.com', 'orgId': 'x'}, {'loggedIn': False, 'email': 'bob@example.com'}, {'loggedIn': True},
                          {'loggedIn': True, 'email': ''}, {'loggedIn': 1, 'email': '邮箱@example.com'}, {'email': 'bob@example.com'}, {}, 'not json', '',
                          {'loggedIn': True, 'email': None}]):
    case('sh-auth-email/%02d' % i, ['sh-auth-email'], stdin=text if isinstance(text, str) else json.dumps(text, ensure_ascii=False), oracle=PY + [AUTH_EMAIL])(nothing)

for name, content in [('with-account', {'userID': 'u' * 8, 'oauthAccount': {'emailAddress': 'bob@example.com'}, 'projects': {'/x': {'a': [1, 2.5, None, True]}}, '中文': '值 ☁'}),
                      ('without-account', {'userID': 'x'}), ('empty', {}), ('compact', '{"a":1,"oauthAccount":{},"b":[]}'), ('not-json', '{oops'), ('float-forms', '{"a": 1.0, "b": 1e22, "c": -0.0}')]:
    @case('sh-oauth-drop/%s' % name, ['sh-oauth-drop', '%HOME%/acct/.claude.json'], oracle=PY + [OAUTH_DROP])
    def _(b, content=content):
        b.write('acct/.claude.json', content if isinstance(content, str) else json.dumps(content, ensure_ascii=False, indent=2) + '\n', 0o600)

case('sh-oauth-drop/missing-file', ['sh-oauth-drop', '%HOME%/nope.json'], oracle=PY + [OAUTH_DROP])(nothing)

for i, text in enumerate(['/Users/me/.cca/accounts/a-b_c (1)/desktop', 'a.b*c+d?e|f^g$h[i]j{k}l\\m&n~o#p\tq\nr', '中文 "\'<>,;:=!@%_`/', '', 'plain']):
    case('sh-re-escape/%02d' % i, ['sh-re-escape', text], oracle=PY + [RE_ESCAPE])(nothing)

PS_OUT = '    1     0\n  100     1\n  200   100\n  201   100\n  300   200\n  400   999\n  500   400\nbad line here\n  600\n\n  700   300   extra\n  301   300\n'
for root in ['100', '1', '400', '12345', '300', 'x']:
    case('sh-ps-tree/%s' % root, ['sh-ps-tree', root], stdin=PS_OUT, oracle=PY + [PS_TREE])(nothing)
case('sh-ps-tree/not-a-number-in-rows', ['sh-ps-tree', '1'], stdin='1 0\na b\n', oracle=PY + [PS_TREE])(nothing)

LSOF = 'p4242\nfcwd\nn/Users/me/x\nf12\nn/Users/me/.cca/accounts/bob/desktop/Local State\nn/Users/me/.cca/accounts/bob/desktop-other/file\np4243\nn/tmp\n'
for name, d in [('inside', '/Users/me/.cca/accounts/bob/desktop'), ('prefix-only', '/Users/me/.cca/accounts/bob/desk'), ('sibling', '/Users/me/.cca/accounts/bob/desktop-other'),
                ('elsewhere', '/Users/me/other'), ('exact-entry-is-not-under', '/Users/me/x'), ('empty-dir', '')]:
    case('sh-lsof-under/%s' % name, ['sh-lsof-under', d], stdin=LSOF, oracle=PY + [LSOF_UNDER])(nothing)
case('sh-lsof-under/no-input', ['sh-lsof-under', '/x'], stdin='', oracle=PY + [LSOF_UNDER])(nothing)

CLIENTS = [
    {'mode': 'auto', 'auto_install': True, 'clients': [{'id': 'claude', 'label': 'Claude', 'status': 'current', 'message': '已是最新 1.2.3'},
                                                       {'id': 'codex', 'label': 'ChatGPT', 'status': 'ready', 'message': '有新版本 2.0', 'can_install': True, 'install_token': 'tok-123'},
                                                       {'id': 'gone', 'status': 'missing', 'message': '没装'}, {'id': 'bare'}, {'label': 'x', 'status': 'error'},
                                                       {'id': 'cli', 'message': 'm', 'can_install': True}, {'id': 'cli2', 'message': 'm', 'can_install': False, 'install_token': 't'}]},
    {'mode': 'off'}, {}, {'mode': 'auto', 'clients': []}, {'clients': [{'label': 5}]}, {'clients': [{'id': 'a', 'message': None}]}, [], 'not json',
]
for i, d in enumerate(CLIENTS):
    case('sh-clients-text/%02d' % i, ['sh-clients-text'], stdin=d if isinstance(d, str) else json.dumps(d, ensure_ascii=False), oracle=PY + [CLIENTS_TEXT])(nothing)


# sh-dock-wait：等一个进程退出，再换成后面的命令。这里只有“进程已经没了、马上往下走”的几条路；真的等到进程退出
# 在 cli/src/cmd/shell.rs 的单元测试里
WAIT_ARGS = ['%HOME%/waitdir', '/bin/sh', '-c', 'echo tidy "$@"; ls "$0" 2>&1', '%HOME%/waitdir', 'App Name']


@case('sh-dock-wait/gone-already', ['sh-dock-wait', '999999'] + WAIT_ARGS, oracle=PY + [DOCK_WAIT])
def _(b):
    pass


@case('sh-dock-wait/no-lock-dir', ['sh-dock-wait', '999999', '', '/bin/echo', 'tidy', 'x'], oracle=PY + [DOCK_WAIT])
def _(b):
    pass


@case('sh-dock-wait/command-missing', ['sh-dock-wait', '999999', '%HOME%/waitdir', '%HOME%/nope', '_dock-tidy'], oracle=PY + [DOCK_WAIT])
def _(b):
    pass


@case('sh-dock-wait/lock-dir-blocked', ['sh-dock-wait', '999999', '%HOME%/file/sub', '/bin/echo', 'tidy'], oracle=PY + [DOCK_WAIT])
def _(b):
    b.write('file', 'in the way')


@case('sh-dock-wait/bad-pid', ['sh-dock-wait', 'abc', '', '/bin/echo', 'x'], oracle=PY + [DOCK_WAIT])
def _(b):
    pass
