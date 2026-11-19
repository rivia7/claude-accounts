"""Codex 的会话库里经账号目录的路径改写到 ~/.codex（cli/src/cmd/codex.rs 的 codex_fix_rollouts；Codex 的 `link` 最后一步）。

原来用 Python 自带的 sqlite3 库，cca-core 起系统的 sqlite3 命令：两边改出来的库字节不一样（文件头里记着 SQLite 的版本），
run.py 对 .sqlite 文件比的是 `.dump` 的内容。夹具里的库在用例的 setup 里用 Python 的 sqlite3 建。"""
import os
import sqlite3

from run import case
from cases.router import ident
from cases.table import world

ERIN = 'erin@example.com'
AID = ident(ERIN)
# link 第一次给账号定设备标识，是随机的：遮掉
IDS = [(r'[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}', '<随机的 UUID>'), (r'\b[0-9a-f]{64}\b', '<随机的标识>')]


def acct(b, rest=''):
    return b.path('.cca/codex/accounts/%s/codex/%s' % (AID, rest))


def db(b, rel, rows, schema='CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, title TEXT)'):
    p = b.path(rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    con = sqlite3.connect(p)
    con.execute(schema)
    if rows:
        con.executemany('INSERT INTO threads VALUES (%s)' % ', '.join('?' * len(rows[0])), rows)
    con.commit()
    con.close()
    return p


def sessions(b, *names, root='.codex'):
    for n in names:
        b.write('%s/%s' % (root, n), '{}\n', 0o644)


def codex_world(b, toml=None):
    world(b, codex={ERIN: 'HK-A'})
    if toml is not None:
        b.write('.codex/config.toml', toml, 0o644)


def rows(b):
    """一批会话：有对应文件的、没有的、归档的、名字里有引号和逗号的、只有带后缀的文件的、不经账号目录的、别的账号目录的"""
    return [
        ('t-moved', acct(b, 'sessions/2026/11/03/rollout-a.jsonl'), 'a'),
        ('t-no-file', acct(b, 'sessions/2026/11/03/rollout-gone.jsonl'), 'b'),
        ('t-archived', acct(b, 'archived_sessions/rollout-old.jsonl'), 'c'),
        ("t-it's, quoted", acct(b, "sessions/2026/11/03/rollout-it's, here.jsonl"), 'd'),
        ('t-prefix', acct(b, 'sessions/2026/11/03/rollout-b.jsonl'), 'e'),
        ('t-elsewhere', b.path('.codex/sessions/2026/11/03/rollout-a.jsonl'), 'f'),
        ('t-other-account', b.path('.cca/codex/accounts/gone-000000/codex/sessions/2026/11/03/rollout-a.jsonl'), 'g'),
        ('t-not-sessions', acct(b, 'worktrees/x/rollout-a.jsonl'), 'h'),
        ('t-directory', acct(b, 'sessions/'), 'i'),
    ]


FILES = ['sessions/2026/11/03/rollout-a.jsonl', 'archived_sessions/rollout-old.jsonl', "sessions/2026/11/03/rollout-it's, here.jsonl",
         'sessions/2026/11/03/rollout-b.jsonl.bak']


@case('codex-rollouts/rewrites', ['link', ERIN], tool='codex', mask=IDS)
def _(b):
    codex_world(b)
    sessions(b, *FILES)
    db(b, '.codex/state_5.sqlite', rows(b))


@case('codex-rollouts/nothing-to-fix', ['link', ERIN], tool='codex', mask=IDS)
def _(b):
    codex_world(b)
    sessions(b, *FILES)
    db(b, '.codex/state_5.sqlite', [r for r in rows(b) if r[0] in ('t-elsewhere', 't-no-file')])


@case('codex-rollouts/several-databases', ['link', ERIN], tool='codex', mask=IDS)
def _(b):
    codex_world(b)
    sessions(b, *FILES)
    db(b, '.codex/state_5.sqlite', rows(b)[:2])
    db(b, '.codex/state_12.sqlite', rows(b)[2:3])
    db(b, '.codex/state_7.sqlite', [('x', 1)], schema='CREATE TABLE threads (id TEXT, title INTEGER)')          # 没有 rollout_path 这一列
    db(b, '.codex/state_8.sqlite', [], schema='CREATE TABLE other (id TEXT)')                                    # 没有 threads 表
    b.write('.codex/state_9.sqlite', 'this is not a database, just text that is long enough to not look empty\n', 0o644)
    b.write('.codex/state_.sqlite', '', 0o644)
    db(b, '.codex/other_5.sqlite', rows(b)[:1])                                                                  # 名字对不上：不看
    db(b, '.codex/state_5.sqlite.bak', rows(b)[:1])


@case('codex-rollouts/integer-ids', ['link', ERIN], tool='codex', mask=IDS)
def _(b):
    codex_world(b)
    sessions(b, *FILES)
    db(b, '.codex/state_5.sqlite', [(i + 1, r[1]) for i, r in enumerate(rows(b))], schema='CREATE TABLE threads (id INTEGER PRIMARY KEY, rollout_path TEXT)')


@case('codex-rollouts/null-and-blob-paths', ['link', ERIN], tool='codex', mask=IDS)
def _(b):
    codex_world(b)
    sessions(b, *FILES)
    db(b, '.codex/state_5.sqlite', [('t-null', None), ('t-blob', b'\x00\x01'), ('t-int', 5), ('t-moved', acct(b, 'sessions/2026/11/03/rollout-a.jsonl'))],
       schema='CREATE TABLE threads (id TEXT, rollout_path)')


@case('codex-rollouts/sqlite-home-elsewhere', ['link', ERIN], tool='codex', mask=IDS)
def _(b):
    codex_world(b, 'model = "x"\nsqlite_home = """\n%s"""\n' % b.path('databases'))
    sessions(b, *FILES)
    db(b, 'databases/state_5.sqlite', rows(b))
    db(b, '.codex/state_5.sqlite', rows(b))          # 设了 sqlite_home 就不看 ~/.codex 里的


@case('codex-rollouts/sqlite-home-tilde', ['link', ERIN], tool='codex', mask=IDS)
def _(b):
    codex_world(b, 'sqlite_home = "~/databases"\n')
    sessions(b, *FILES)
    db(b, 'databases/state_5.sqlite', rows(b))


@case('codex-rollouts/sqlite-home-empty', ['link', ERIN], tool='codex', mask=IDS)
def _(b):
    codex_world(b, 'sqlite_home = ""\n')
    sessions(b, *FILES)
    db(b, '.codex/state_5.sqlite', rows(b))


@case('codex-rollouts/sqlite-home-missing-dir', ['link', ERIN], tool='codex', mask=IDS)
def _(b):
    codex_world(b, 'sqlite_home = "%s"\n' % b.path('nowhere'))
    sessions(b, *FILES)
    db(b, '.codex/state_5.sqlite', rows(b))


for name, toml in [('config-invalid', 'sqlite_home = ['), ('sqlite-home-not-a-string', 'sqlite_home = 5\n'), ('config-not-utf8', b'sqlite_home = "\xff"\n')]:
    @case('codex-rollouts/%s' % name, ['link', ERIN], tool='codex', mask=IDS)
    def _(b, toml=toml):
        codex_world(b, toml)
        sessions(b, *FILES)
        db(b, '.codex/state_5.sqlite', rows(b))


@case('codex-rollouts/claude-link-leaves-it-alone', ['link', 'bob@example.com'], mask=IDS)
def _(b):
    codex_world(b)
    sessions(b, *FILES)
    db(b, '.codex/state_5.sqlite', rows(b))


@case('codex-rollouts/sqlite3-command-missing', ['link', ERIN], tool='codex', env={'PATH': '%HOME%/bin:/bin'}, tree=False, mask=IDS)
def _(b):
    # 这一例两边注定不一样（原来不靠外部命令，cca-core 找不到 sqlite3 就跳过）：只比输出和退出码，不比库
    codex_world(b)
    sessions(b, *FILES)
    db(b, '.codex/state_5.sqlite', rows(b))


# ================================================================================================================
# codex 的 app-server（JSON-RPC）：usage-query 的 Codex 一支、reset-consume、login-flow；登录身份（email）
#
# codex 用沙箱里假的（CODEX）：features list / --help / --version 给 codex_probe 探测参数；最后一个参数是 app-server 时，
# 把 stdin 上收到的每一行记进 rpc.in，照沙箱里 rpc/<行号> 的内容回答（没有这个文件就不回）。行号：1 是 initialize，
# 2 是 initialized（通知，不用回），3 起是这次要做的事。rpc/<行号>.hang 是卡住几秒，.exit 是回答完就退出，.err 是往 stderr 说；
# rpc/err、rpc/exit、rpc/hello 是一起来就说、就退出、就往 stdout 写的。rpc/detach：读请求的那一圈放在另一个进程里——
# cca 取消登录时先发一条取消、紧跟着就杀掉 app-server，这样那一条照样记得下来。rpc/pid：把 cca-core / Python 的进程号
# 记在 rpc/ppid（给发信号的用例）。夹具里的 id_token 是编的，另外两个 token 写成一看就是假的。
# ================================================================================================================
import base64
import json
import signal
import time

UUID = [(r'[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}', '<随机的 UUID>')]
BIN = '%HOME%/bin/codex'
CODEX = r'''# 假的 codex（对拍用，见 cases/codex.py）
T="$HOME"
printf 'codex %s\n' "$*" >> "$T/calls.log"
case "$1" in
	features) if [ -f "$T/codex.features" ]; then cat "$T/codex.features"; else printf 'system_proxy_fallback  stable  true\nrespect_system_proxy  experimental  false\nother_thing  stable  true\n'; fi; exit 0 ;;
	--help) echo 'Usage: codex [OPTIONS] [--no-daemon] [PROMPT]'; exit 0 ;;
	--version) echo 'codex-cli 0.167.0'; exit 0 ;;
esac
printf 'app-server cwd=%s CODEX_HOME=%s fd3=%s\n' "$(pwd -P)" "${CODEX_HOME-unset}" "$( { true <&3; } 2>/dev/null && echo open || echo closed)" >> "$T/calls.log"
[ -f "$T/rpc/pid" ] && echo "$PPID" > "$T/rpc/ppid"
[ -f "$T/rpc/err" ] && cat "$T/rpc/err" >&2
[ -f "$T/rpc/exit" ] && exit "$(cat "$T/rpc/exit")"
[ -f "$T/rpc/hello" ] && cat "$T/rpc/hello"
serve() {
	n=0
	while IFS= read -r line; do
		n=$((n + 1))
		printf '%s\n' "$line" >> "$T/rpc.in"
		[ -f "$T/rpc/$n.hang" ] && exec sleep "$(cat "$T/rpc/$n.hang")"
		[ -f "$T/rpc/$n" ] && cat "$T/rpc/$n"
		[ -f "$T/rpc/$n.err" ] && cat "$T/rpc/$n.err" >&2
		[ -f "$T/rpc/$n.exit" ] && exit "$(cat "$T/rpc/$n.exit")"
	done
	exit 0
}
if [ -f "$T/rpc/detach" ]; then
	exec 4<&0
	( serve <&4 ) &
	wait $!
	exit $?
fi
serve
'''
BROWSER = r'''printf 'cca %s\n' "$*" >> "$HOME/calls.log"
[ -f "$HOME/browser.sleep" ] && exec sleep "$(cat "$HOME/browser.sleep")"
[ -f "$HOME/browser.out" ] && cat "$HOME/browser.out"
exit "$(cat "$HOME/browser.rc" 2>/dev/null || echo 0)"
'''


def line(m):
    return json.dumps(m, ensure_ascii=False) + '\n'


def res(i, result):
    return line({'id': i, 'result': result})


def fail(i, message, code=-32603, **more):
    e = {'code': code, 'message': message}
    e.update(more)
    return line({'id': i, 'error': {k: v for k, v in e.items() if v is not Ellipsis}})


HELLO = res(1, {'userAgent': 'fake'})


def server(b, *calls, init=HELLO, browser=True, home=True, **files):
    """假的 codex 和它的回答：init 是对 initialize 的回答，calls 依次是对后面每个请求的回答（None 是不回）。
    files 是别的开关：n3_exit='1' 就是 rpc/3.exit（第 3 行请求回答完就退出），browser_sleep='5' 是沙箱里的 browser.sleep
    （假的 cca _browser 用），其余的放在 rpc/ 下（err、exit、hello、pid、detach）"""
    if home:
        world(b, codex={ERIN: 'HK-A'})
    b.fake('codex', CODEX)
    os.makedirs(b.path('rpc'), exist_ok=True)
    if init is not None:
        b.write('rpc/1', init)
    for i, text in enumerate(calls):
        if text is not None:
            b.write('rpc/%d' % (i + 3), text)
    for name, text in files.items():
        if name.startswith('browser_'):
            b.write('browser.' + name[len('browser_'):], text)
        elif name[0] == 'n' and name[1].isdigit():
            b.write('rpc/' + name[1:].replace('_', '.'), text)
        else:
            b.write('rpc/' + name, text)
    if browser:
        b.write('.cca/bin/cca', '#!/bin/sh\n' + BROWSER, 0o755)


def b64(d):
    return base64.urlsafe_b64encode(json.dumps(d).encode()).decode().rstrip('=')


def auth_json(b, claims=None, key=ERIN, raw=None, tokens=Ellipsis):
    """账号目录里的 auth.json：id_token 的声明是编的"""
    payload = {'email': key, 'https://api.openai.com/auth': {'chatgpt_plan_type': 'plus', 'chatgpt_account_id': 'ws-1', 'chatgpt_user_id': 'user-12345678'}} if claims is None else claims
    j = {'auth_mode': 'chatgpt', 'OPENAI_API_KEY': None,
         'tokens': {'id_token': '%s.%s.sig' % (b64({'alg': 'none'}), payload if isinstance(payload, str) else b64(payload)), 'access_token': 'test-token-not-real', 'refresh_token': 'test-token-not-real'}
         if tokens is Ellipsis else tokens}
    return b.write('.cca/codex/accounts/%s/codex/auth.json' % ident(key), raw if raw is not None else json.dumps(j))


# --- usage-query（Codex）：account/rateLimits/read ---
def window(pct, mins=Ellipsis, at=1795100000):
    w = {'usedPercent': pct, 'windowDurationMins': mins, 'resetsAt': at}
    return {k: v for k, v in w.items() if v is not Ellipsis}


LIMITS = {'limitId': 'codex', 'limitName': None, 'primary': window(13, 300), 'secondary': window(41.5, 10080, 1795400000), 'planType': 'pro',
          'credits': {'hasCredits': True, 'unlimited': False, 'balance': '12.5'}}
FULL = {'rateLimits': LIMITS,
        'rateLimitsByLimitId': {'codex': LIMITS, 'codex_other': {'limitId': 'codex_other', 'limitName': 'GPT-5-Pro', 'primary': window(5, 300), 'secondary': window(30, 10080)},
                                'weekly_only': {'primary': window(7, 10080, 1795500000), 'secondary': None}, 'five_hour_only': {'limitName': 'Hourly', 'primary': window(9, 300)},
                                'not-an-object': 5, 'named-empty': {'limitName': '', 'secondary': window(1, 20000)}},
        'rateLimitResetCredits': {'availableCount': 2, 'credits': [{'id': 'c-1', 'status': 'available', 'title': None, 'expiresAt': '2026-12-05T00:00:00Z'}]}}
READ = [
    ('full', FULL),
    ('windows-by-position', {'rateLimits': {'primary': window(13), 'secondary': window(41)}}),
    ('both-windows-short-first-wins', {'rateLimits': {'primary': window(13, 300), 'secondary': window(41, 1440)}}),
    ('both-windows-long-first-wins', {'rateLimits': {'primary': window(13, 1441), 'secondary': window(41, 10080)}}),
    ('weekly-in-primary', {'rateLimits': {'primary': window(13, 10080), 'secondary': None, 'planType': 'free'}}),
    ('swapped', {'rateLimits': {'primary': window(41, 10080), 'secondary': window(13, 300)}}),
    ('only-secondary-by-position', {'rateLimits': {'secondary': window(41)}}),
    ('percent-not-a-number', {'rateLimits': {'primary': window(True, 300), 'secondary': window('41', 10080)}}),
    ('percent-null-and-float', {'rateLimits': {'primary': window(None, 300), 'secondary': window(0.0, 10080.5, None)}}),
    ('mins-bool-string-list', {'rateLimits': {'primary': window(13, True), 'secondary': window(41, '10080')}}),
    ('mins-string-first', {'rateLimits': {'primary': window(13, 'x'), 'secondary': window(41, [1])}}),
    ('mins-huge', {'rateLimits': {'primary': window(13, 10 ** 30), 'secondary': window(41, -10 ** 30)}}),
    ('mins-false-is-short', {'rateLimits': {'primary': window(13, False), 'secondary': window(41, 1e9)}}),
    ('windows-not-objects', {'rateLimits': {'primary': 13, 'secondary': [41], 'planType': ''}}),
    ('no-rate-limits', {'rateLimitsByLimitId': {'x': {'secondary': window(3, 10080)}}}),
    ('rate-limits-not-an-object', {'rateLimits': [LIMITS], 'rateLimitsByLimitId': [LIMITS], 'rateLimitResetCredits': [1]}),
    ('main-id-is-not-a-string', {'rateLimits': dict(LIMITS, limitId=5), 'rateLimitsByLimitId': {'5': {'secondary': window(3, 10080)}}}),
    ('plan-type-number', {'rateLimits': dict(LIMITS, planType=5, credits='lots')}),
    ('empty-result', {}),
    ('result-null', None),
    ('result-list', [FULL]),
    ('result-string', 'ok'),
    ('result-with-unicode', {'rateLimits': dict(LIMITS, limitName='额度'), 'rateLimitsByLimitId': {'模型': {'limitName': '模型 β', 'secondary': window(3, 10080)}}}),
]
for name, result in READ:
    @case('usage-query/codex-%s' % name, ['usage-query', BIN, '10'], tool='codex')
    def _(b, result=result):
        server(b, res(2, result))

for name, text in [('nan', 'NaN'), ('inf', 'Infinity'), ('neg-inf', '-Infinity')]:
    @case('usage-query/codex-mins-%s' % name, ['usage-query', BIN, '10'], tool='codex')
    def _(b, text=text):
        server(b, res(2, {'rateLimits': {'primary': window(13, '@@'), 'secondary': window('@@', 300)}}).replace('"@@"', text))

ERRORS = [
    ('method-not-found', fail(2, 'Method not found: account/rateLimits/read', -32601)),
    ('unknown-variant', fail(2, 'Invalid request: unknown variant `account/rateLimits/read`, expected one of `initialize`')),
    ('unknown-request-uppercase', fail(2, 'UNKNOWN REQUEST')),
    ('token-not-refreshed', fail(2, 'Your access token could not be refreshed because you have since logged out or signed in to another account.')),
    ('401', fail(2, 'unexpected status 401 Unauthorized: token_expired')),
    ('401-inside-a-word', fail(2, 'code x401 unauthorizedy')),
    ('401-after-punctuation', fail(2, 'status=401 UNAUTHORIZED.')),
    ('other', fail(2, 'codex account authentication required')),
    ('long', fail(2, '错误 ' * 200)),
    ('message-empty', fail(2, '')),
    ('message-missing', fail(2, Ellipsis, data={'详情': 'x'})),
    ('message-number', fail(2, 404)),
    ('message-list', fail(2, ['a', 1])),
    ('message-true', fail(2, True)),
    ('error-empty-object', line({'id': 2, 'error': {}})),
    ('error-string', line({'id': 2, 'error': 'Method not found'})),
    ('error-number', line({'id': 2, 'error': 5})),
    ('error-list', line({'id': 2, 'error': ['boom']})),
    ('error-says-timeout', fail(2, 'timeout')),
    ('error-says-exit', fail(2, 'exit')),
    ('error-null-with-result', line({'id': 2, 'error': None, 'result': {'rateLimits': LIMITS}})),
    ('error-zero-with-result', line({'id': 2, 'error': 0, 'result': {'rateLimits': LIMITS}})),
    ('error-false-no-result', line({'id': 2, 'error': False})),
    ('error-empty-list-no-result', line({'id': 2, 'error': []})),
    ('error-and-result', line({'id': 2, 'error': {'message': 'boom'}, 'result': {'rateLimits': LIMITS}})),
]
for name, reply in ERRORS:
    @case('usage-query/codex-error-%s' % name, ['usage-query', BIN, '10'], tool='codex')
    def _(b, reply=reply):
        server(b, reply, err='warning: something on stderr\n  second   line\n')

NOISE = [
    ('noise-before-answers', {'init': 'not json\n[1]\n"x"\n\n' + line({'method': 'codex/event', 'params': {'x': 1}}) + line({'id': 99, 'result': {}}) + line({'id': 1}) + HELLO,
                              'calls': [line({'method': 'account/updated'}) + line({'id': '2', 'result': 'wrong: the id is a string'}) + line({'id': None, 'result': 1}) + res(2, {'rateLimits': LIMITS})]}),
    ('id-float', {'init': line({'id': 1.0, 'result': {}}), 'calls': [line({'id': 2.0, 'result': {'rateLimits': LIMITS}})]}),
    ('id-true-is-one', {'init': line({'id': True, 'result': {}}), 'calls': [res(2, {'rateLimits': LIMITS})]}),
    ('request-with-our-id-is-not-an-answer', {'init': line({'id': 1, 'method': 'server/request', 'params': {}}) + HELLO, 'calls': [res(2, {'rateLimits': LIMITS})]}),
    ('answer-with-bom', {'init': b'\xef\xbb\xbf' + HELLO.encode(), 'calls': [res(2, {'rateLimits': LIMITS})]}),
    ('answer-not-utf8-then-good', {'init': b'{"id": 1, "result": "\xff"}\n' + HELLO.encode(), 'calls': [res(2, {'rateLimits': LIMITS})]}),
    ('answers-without-newline-at-the-end', {'calls': [res(2, {'rateLimits': LIMITS}).rstrip('\n')], 'n3_exit': '0'}),
    ('hello-before-anything', {'hello': line({'method': 'server/ready'}), 'calls': [res(2, {'rateLimits': LIMITS})]}),
]
for name, kw in NOISE:
    @case('usage-query/codex-%s' % name, ['usage-query', BIN, '10'], tool='codex')
    def _(b, kw=kw):
        kw = dict(kw)
        server(b, *kw.pop('calls', []), **kw)

GONE = [
    ('not-a-subcommand', {'init': None, 'err': "error: unrecognized subcommand 'app-server'\n\n  tip: a similar subcommand exists: 'apply'\n", 'exit': '2'}, '10'),
    ('exits-quietly', {'init': None, 'exit': '0'}, '10'),
    ('exits-after-noise', {'init': None, 'hello': 'starting…\n', 'exit': '1', 'err': 'fatal: 配置读不了\n'}, '10'),
    ('initialize-fails', {'init': fail(1, 'Invalid request: unexpected argument')}, '10'),
    ('initialize-fails-other', {'init': fail(1, 'not ready'), 'err': 'ignored because the error has its own message\n'}, '10'),
    ('initialize-then-exit', {'n1_exit': '3', 'err': 'thread main panicked\n'}, '10'),
    ('exits-instead-of-answering', {'n3_exit': '1', 'n3_err': 'error: unexpected argument found\n'}, '10'),
    ('exits-instead-of-answering-quietly', {'n3_exit': '0'}, '10'),
    ('initialize-hangs', {'init': None, 'n1_hang': '5', 'err': 'still starting\n'}, '0.5'),
    ('initialize-hangs-quietly', {'init': None, 'n1_hang': '5'}, '0.5'),
    ('read-hangs', {'n3_hang': '5'}, '0.5'),
    ('read-hangs-says-401', {'n3_hang': '5', 'err': 'WARN 401 Unauthorized while refreshing\n'}, '0.5'),
    ('read-hangs-says-method-not-found', {'n3_hang': '5', 'err': 'method not found\n'}, '0.5'),
    ('never-answers', {'init': None}, '0.5'),
    ('stderr-long', {'init': None, 'err': '头' * 100 + '\n' + 'x ' * 500 + 'unrecognized subcommand\n', 'exit': '2'}, '10'),
    ('stderr-not-utf8', {'init': None, 'err': b'bad \xff\xfe bytes\n', 'exit': '2'}, '10'),
]
for name, kw, timeout in GONE:
    @case('usage-query/codex-%s' % name, ['usage-query', BIN, timeout], tool='codex')
    def _(b, kw=kw):
        server(b, **kw)

for name, t in [('bad-timeout', 'soon'), ('nan-timeout', 'nan'), ('inf-timeout', 'inf'), ('huge-timeout', '1e300')]:
    # 超时写成无穷大：等回答的那一步出错（没人接），假 codex 还活着，等它把收到的记完再看沙箱
    @case('usage-query/codex-%s' % name, ['usage-query', BIN, t], tool='codex', settle=0.5)
    def _(b):
        server(b, res(2, {'rateLimits': LIMITS}))


@case('usage-query/codex-default-timeout', ['usage-query', BIN], tool='codex')
def _(b):
    server(b, res(2, {'rateLimits': LIMITS}))


# 超时是负数、0：只等 0.01 秒就杀掉 app-server，它来不来得及把收到的记下来说不准——只比输出和退出码
for name, t in [('timeout-negative', '-5'), ('timeout-zero', '0')]:
    @case('usage-query/codex-%s' % name, ['usage-query', BIN, t], tool='codex', tree=False)
    def _(b):
        server(b, init=None, n1_hang='5')


@case('usage-query/codex-codex-home', ['usage-query', BIN, '10'], tool='codex', env={'CODEX_HOME': '%HOME%/.cca/codex/accounts/' + AID + '/codex', 'OPENAI_API_KEY': 'test-token-not-real', 'CCA_RO': '1'})
def _(b):
    server(b, res(2, {'rateLimits': LIMITS}))
    os.makedirs(b.path('.cca/codex/accounts/%s/codex' % AID))


@case('usage-query/codex-codex-home-missing', ['usage-query', BIN, '10'], tool='codex', env={'CODEX_HOME': '%HOME%/nowhere/codex'})
def _(b):
    server(b, res(2, {'rateLimits': LIMITS}))


@case('usage-query/codex-no-cca-home', ['usage-query', BIN, '10'], tool='codex')
def _(b):
    server(b, res(2, {'rateLimits': LIMITS}), home=False, browser=False)


@case('usage-query/codex-fd3-not-passed-on', ['usage-query', BIN, '10'], tool='codex', fd3='answer\n')
def _(b):
    server(b, res(2, {'rateLimits': LIMITS}))


@case('usage-query/codex-tmpdir-missing', ['usage-query', BIN, '10'], tool='codex', env={'TMPDIR': '%HOME%/nowhere'})
def _(b):
    server(b, n3_exit='1', n3_err='panicked at the disco\n')      # 接 stderr 的临时文件换到 /tmp 去了，说的话照样读得回来


for name, bin_ in [('bare-name', 'codex'), ('missing', '%HOME%/nowhere/codex'), ('missing-name', 'no-such-codex'), ('empty', ''), ('not-executable', '%HOME%/plain'), ('directory', '%HOME%/bin')]:
    @case('usage-query/codex-bin-%s' % name, ['usage-query', bin_, '10'], tool='codex')
    def _(b):
        server(b, res(2, {'rateLimits': LIMITS}))
        b.write('plain', '#!/bin/sh\nexit 0\n', 0o644)


def probe(b, **change):
    """照假 codex 的样子放一份探测结果的缓存（按程序文件的路径、大小、修改时间认）"""
    st = os.stat(b.path('bin/codex'))
    j = {'sig': '%s|%d|%d' % (b.path('bin/codex'), st.st_size, int(st.st_mtime)), 'flags': ['--from-cache', 'x y'], 'no_daemon': False, 'version': '9.9.9'}
    j.update(change)
    b.write('.cca/codex/probe.json', json.dumps({k: v for k, v in j.items() if v is not Ellipsis}))


PROBES = [('cached', {}), ('cached-no-version', {'version': Ellipsis}), ('cached-version-empty', {'version': ''}), ('cached-version-number', {'version': 5}), ('cached-no-flags', {'flags': []}),
          ('cache-stale', {'sig': 'something else'}), ('cache-flags-not-a-list', {'flags': 'x'}), ('cached-flag-not-a-string', {'flags': ['--ok', 5]})]
for name, change in PROBES:
    @case('usage-query/codex-probe-%s' % name, ['usage-query', BIN, '10'], tool='codex')
    def _(b, change=change):
        server(b, res(2, {'rateLimits': LIMITS}))
        probe(b, **change)


@case('usage-query/codex-probe-features-missing-one', ['usage-query', BIN, '10'], tool='codex')
def _(b):
    server(b, res(2, {'rateLimits': LIMITS}))
    b.write('codex.features', 'respect_system_proxy  experimental  false\n')


@case('usage-query/codex-probe-no-features', ['usage-query', BIN, '10'], tool='codex')
def _(b):
    server(b, res(2, {'rateLimits': LIMITS}))
    b.write('codex.features', '')


# ~/.codex 建不了（那里是个文件、断了的链接）：要重新探测时 os.makedirs 报错，app-server 没起来，说的是「出错」；
# 探测结果有缓存就走不到建目录那一步，照常查
def master_blocked(b, how):
    if how == 'file':
        b.write('.codex', 'not a directory')
    else:
        os.symlink('nowhere', b.path('.codex'))


for how in ('file', 'dangling-link'):
    @case('usage-query/codex-probe-master-is-a-%s' % how, ['usage-query', BIN, '10'], tool='codex')
    def _(b, how=how):
        server(b, res(2, {'rateLimits': LIMITS}))
        master_blocked(b, how)

    @case('usage-query/codex-probe-master-is-a-%s-but-cached' % how, ['usage-query', BIN, '10'], tool='codex')
    def _(b, how=how):
        server(b, res(2, {'rateLimits': LIMITS}))
        master_blocked(b, how)
        probe(b)

    @case('reset-consume/probe-master-is-a-%s' % how, ['reset-consume', BIN, 'c-1', '10'], tool='codex', mask=UUID)
    def _(b, how=how):
        server(b, res(2, {'outcome': 'reset'}), res(3, FULL))
        master_blocked(b, how)

    @case('login-flow/probe-master-is-a-%s' % how, ['login-flow', BIN, '10'], tool='codex')
    def _(b, how=how):
        server(b, START + completed())
        master_blocked(b, how)


@case('usage-query/codex-probe-master-is-a-link-to-a-directory', ['usage-query', BIN, '10'], tool='codex')
def _(b):
    server(b, res(2, {'rateLimits': LIMITS}))
    os.makedirs(b.path('elsewhere'))
    os.symlink('elsewhere', b.path('.codex'))


@case('usage-query/codex-probe-master-blocked-but-no-such-bin', ['usage-query', '%HOME%/nowhere/codex', '10'], tool='codex')
def _(b):
    server(b, res(2, {'rateLimits': LIMITS}))
    master_blocked(b, 'file')      # 程序不在：探测在建目录之前就回去了，说的是起不来


# --- reset-consume：account/rateLimitResetCredit/consume，重置了再读一次用量 ---
def consume(name, *calls, argv=('c-1', '10'), **kw):
    @case('reset-consume/%s' % name, ['reset-consume', BIN] + list(argv), tool='codex', mask=UUID)
    def _(b):
        server(b, *calls, **kw)


consume('reset', res(2, {'outcome': 'reset'}), res(3, FULL))
consume('reset-no-credit-given', res(2, {'outcome': 'reset'}), res(3, FULL), argv=())
consume('reset-empty-credit', res(2, {'outcome': 'reset'}), res(3, FULL), argv=('', '10'))
consume('reset-credit-with-quotes', res(2, {'outcome': 'reset'}), res(3, FULL), argv=('c "1"\n额度', '10'))
consume('already-redeemed', res(2, {'outcome': 'alreadyRedeemed', 'more': 1}), res(3, {'rateLimits': LIMITS}))
consume('nothing-to-reset', res(2, {'outcome': 'nothingToReset'}), res(3, 'not asked for'))
consume('no-credit', res(2, {'outcome': 'noCredit'}))
consume('reset-then-read-fails', res(2, {'outcome': 'reset'}), fail(3, 'Method not found'))
consume('reset-then-read-gets-odd-result', res(2, {'outcome': 'reset'}), res(3, None))
consume('reset-then-server-exits', res(2, {'outcome': 'reset'}), n3_exit='0')
consume('reset-then-read-error-is-falsy', res(2, {'outcome': 'reset'}), line({'id': 3, 'error': 0, 'result': {'rateLimits': LIMITS}}))
consume('outcome-unknown', res(2, {'outcome': 'exploded', '说明': '中文'}))
consume('outcome-snake-case-not-theirs', res(2, {'outcome': 'already_redeemed'}))
consume('outcome-missing', res(2, {}))
consume('outcome-null', res(2, {'outcome': None}))
consume('outcome-number', res(2, {'outcome': 5}))
consume('outcome-true', res(2, {'outcome': True}))
consume('outcome-list', res(2, {'outcome': ['reset']}))
consume('outcome-object', res(2, {'outcome': {'reset': True}}))
consume('result-null', res(2, None))
consume('result-string', res(2, 'reset'))
consume('result-long', res(2, {'outcome': 'x', 'detail': '长' * 300}))
consume('unsupported', fail(2, 'Method not found: account/rateLimitResetCredit/consume', -32601))
consume('auth', fail(2, 'codex account authentication required: 401 Unauthorized'))
consume('error', fail(2, 'no credit for you'))
consume('consume-hangs', argv=('c-1', '0.5'), n3_hang='5', err='slow network\n')
consume('server-exits-on-consume', n3_exit='1', n3_err='panicked\n')
consume('initialize-fails', init=fail(1, 'nope'))
consume('server-exits-at-once', init=None, exit='127', err='codex: command not found\n')
consume('bad-timeout', res(2, {'outcome': 'reset'}), argv=('c-1', 'x'))
consume('default-timeout', res(2, {'outcome': 'noCredit'}), argv=('c-1',))


@case('reset-consume/bin-not-there', ['reset-consume', '%HOME%/nowhere/codex', 'c-1'], tool='codex')
def _(b):
    world(b, codex={ERIN: 'HK-A'})


@case('reset-consume/no-args', ['reset-consume'], tool='codex')
def _(b):
    world(b, codex={ERIN: 'HK-A'})


@case('reset-consume/too-many-args', ['reset-consume', BIN, 'c', '5', 'x'], tool='codex')
def _(b):
    server(b)


for tool in ('claude', 'agy'):
    @case('reset-consume/%s-tool' % tool, ['reset-consume', BIN, 'c-1', '10'], tool=tool, mask=UUID)
    def _(b):
        server(b, res(2, {'outcome': 'reset'}), res(3, FULL))      # 不看是哪个工具

    @case('login-flow/%s-tool' % tool, ['login-flow', BIN, '10'], tool=tool)
    def _(b):
        server(b, START + completed())


# --- login-flow：account/login/start 拿登录网址，cca _browser 打开，等 account/login/completed ---
URL = 'https://auth.openai.com/oauth/authorize?client_id=app_x&state=s1&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback'
START = res(2, {'type': 'chatgpt', 'loginId': 'L1', 'authUrl': URL})


def completed(login='L1', success=True, error=None, **more):
    p = {'loginId': login, 'success': success, 'error': error}
    p.update(more)
    return line({'method': 'account/login/completed', 'params': {k: v for k, v in p.items() if v is not Ellipsis}})


def login(name, *calls, argv=('10',), settle=0, **kw):
    @case('login-flow/%s' % name, ['login-flow', BIN] + list(argv), tool='codex', settle=settle)
    def _(b):
        server(b, *calls, **kw)


login('ok', START + completed())
login('ok-default-timeout', START + completed(), argv=())
login('ok-notification-arrives-first', completed() + START)
login('ok-other-logins-first', START + completed('L0', False, 'not ours') + line({'method': 'account/updated', 'params': {'x': 1}}) + completed('L2') + completed())
login('ok-no-login-id-in-notification', START + completed(Ellipsis))
login('ok-login-id-null-in-notification', START + completed(None))
login('ok-no-login-id-at-all', res(2, {'authUrl': URL}) + completed('whatever'))
login('ok-login-id-empty', res(2, {'authUrl': URL, 'loginId': ''}) + completed('L9'))
login('ok-login-id-number', res(2, {'authUrl': URL, 'loginId': 7}) + completed(8, False, 'other') + completed(7.0))
login('ok-success-is-truthy-text', START + completed(success='yes'))
login('ok-browser-fails', START + completed(), browser_rc='5')
login('ok-browser-prints', START + completed(), browser_out='opened in Chrome\n')
login('ok-nan-timeout-waits', START + completed(), argv=('nan',))
login('ok-url-with-spaces', res(2, {'authUrl': 'https://auth.openai.com/a b?x=$(y)&z=\'q\'', 'loginId': 'L1'}) + completed())
login('failed', START + completed(success=False, error='Token exchange failed: invalid_grant'))
login('failed-no-reason', START + completed(success=False))
login('failed-reason-not-text', START + completed(success=0, error={'code': 7}))
login('failed-success-missing', START + completed(success=Ellipsis, error='x'))
login('failed-params-missing', START + line({'method': 'account/login/completed'}) + completed())
login('params-not-an-object', START + line({'method': 'account/login/completed', 'params': 'done'}))
login('params-not-an-object-no-login-id', res(2, {'authUrl': URL}) + line({'method': 'account/login/completed', 'params': ['done']}))
login('unsupported-no-url', res(2, {'type': 'chatgpt', 'loginId': 'L1'}))
login('unsupported-url-empty', res(2, {'authUrl': '', 'loginId': 'L1'}))
login('unsupported-result-not-an-object', res(2, 'https://auth.openai.com/'))
login('unsupported-result-null', res(2, None))
login('unsupported-method-not-found', fail(2, 'Method not found: account/login/start', -32601))
login('unsupported-initialize', init=fail(1, 'unknown method `initialize`'))
login('unsupported-not-a-subcommand', init=None, err="error: unrecognized subcommand 'app-server'\n", exit='2')
login('start-fails', fail(2, 'already logged in with an API key'))
login('start-fails-401', fail(2, '401 Unauthorized'))
login('start-fails-no-message', line({'id': 2, 'error': {'code': -1}}))
login('start-server-exits-quietly', n3_exit='0')
login('start-server-exits-with-stderr', n3_exit='1', n3_err='thread main panicked at login.rs\n')
login('initialize-server-exits-quietly', init=None, exit='0')
login('url-not-text', res(2, {'authUrl': 5, 'loginId': 'L1'}))
login('url-is-a-list', res(2, {'authUrl': [URL], 'loginId': 'L1'}))
login('server-exits-while-waiting', START, n3_exit='0')
login('server-exits-while-waiting-with-stderr', START, n3_exit='1', n3_err='  WARN  login server:\n\tport 1455 in use  \n')
login('never-completes', START, argv=('0.5',))
login('never-completes-two-minutes-ago', START, argv=('-90',))
login('never-completes-fraction', START, argv=('0.99',))
login('no-browser-script', START + completed(), browser=False)
login('bad-timeout', START + completed(), argv=('soon',))
login('inf-timeout', START, argv=('inf',))


@case('login-flow/bin-missing', ['login-flow', '%HOME%/nowhere/codex', '5'], tool='codex')
def _(b):
    world(b, codex={ERIN: 'HK-A'})


@case('login-flow/bin-empty', ['login-flow', '', '5'], tool='codex')
def _(b):
    world(b, codex={ERIN: 'HK-A'})


@case('login-flow/no-args', ['login-flow'], tool='codex')
def _(b):
    world(b, codex={ERIN: 'HK-A'})


@case('login-flow/codex-home', ['login-flow', BIN, '10'], tool='codex', env={'CODEX_HOME': '%HOME%/.cca/codex/accounts/' + AID + '/codex'})
def _(b):
    server(b, START + completed())
    os.makedirs(b.path('.cca/codex/accounts/%s/codex' % AID))


def interrupt(sig, seen, wait=0.3):
    """命令运行期间：等沙箱里出现 seen 说的那个迹象，再给 cca-core / Python 发信号（它的进程号假 codex 记在 rpc/ppid）"""
    def during(b):
        end = time.time() + 15
        while time.time() < end and not (os.path.exists(b.path('rpc/ppid')) and seen(b)):
            time.sleep(0.02)
        time.sleep(wait)
        try:
            with open(b.path('rpc/ppid')) as f:
                pid = int(f.read())
            os.remove(b.path('rpc/ppid'))      # 进程号每次不一样：不留在沙箱里
            os.kill(pid, sig)
        except (OSError, ValueError):
            pass
    return during


def said(rel, text):
    def seen(b):
        try:
            with open(b.path(rel), encoding='utf-8') as f:
                return text in f.read()
        except OSError:
            return False
    return seen


# SIGTERM（服务器上登录时 cca 取消登录用它）：和 Ctrl-C 一样告诉 app-server 取消
@case('login-flow/sigterm-while-waiting', ['login-flow', BIN, '20'], tool='codex', during=interrupt(signal.SIGTERM, said('calls.log', 'cca _browser')), settle=0.5)
def _(b):
    server(b, START, pid='', detach='')


@case('login-flow/sigterm-while-waiting-no-login-id', ['login-flow', BIN, '20'], tool='codex', during=interrupt(signal.SIGTERM, said('calls.log', 'cca _browser')), settle=0.5)
def _(b):
    server(b, res(2, {'authUrl': URL}), pid='', detach='')


@case('login-flow/sigterm-while-browser-opens', ['login-flow', BIN, '20'], tool='codex', during=interrupt(signal.SIGTERM, said('calls.log', 'cca _browser')), settle=0.5)
def _(b):
    server(b, START, pid='', detach='')
    b.write('browser.sleep', '5')      # cca _browser 还没回来就被取消：等它 0.25 秒，杀掉


@case('login-flow/sigterm-before-login-starts', ['login-flow', BIN, '20'], tool='codex', during=interrupt(signal.SIGTERM, said('rpc.in', 'account/login/start')), settle=0.5)
def _(b):
    server(b, pid='', detach='', n3_hang='8')      # 还没拿到登录网址：没有要取消的登录


@case('login-flow/sigterm-during-initialize', ['login-flow', BIN, '20'], tool='codex', during=interrupt(signal.SIGTERM, said('rpc.in', 'initialize')), settle=0.5)
def _(b):
    server(b, init=None, pid='', detach='', n1_hang='8')


# Ctrl-C：进来时 SIGINT 没被忽略才管用（后台跑的时候 shell 把它忽略了，Python 也就不接）；两种情况两边都该一样，这里两种都走得完
@case('login-flow/sigint-while-waiting', ['login-flow', BIN, '3'], tool='codex', during=interrupt(signal.SIGINT, said('calls.log', 'cca _browser')), settle=0.5)
def _(b):
    server(b, START, pid='', detach='')


# --- 登录身份：email 子命令（T.email）读 Codex 的 auth.json、agy 的 token 文件 ---
AUTH = {'chatgpt_plan_type': 'plus', 'chatgpt_account_id': 'ws-1', 'chatgpt_user_id': 'user-12345678'}
CLAIMS = [
    ('ok', {'email': ERIN, 'https://api.openai.com/auth': AUTH}), ('email-in-profile', {'https://api.openai.com/profile': {'email': 'profile@example.com'}, 'https://api.openai.com/auth': AUTH}),
    ('top-level-email-wins', {'email': 'top@example.com', 'https://api.openai.com/profile': {'email': 'profile@example.com'}}),
    ('email-empty-falls-back', {'email': '', 'https://api.openai.com/profile': {'email': 'profile@example.com'}}), ('email-number', {'email': 5, 'https://api.openai.com/profile': {'email': 'p@example.com'}}),
    ('email-zero-falls-back', {'email': 0, 'https://api.openai.com/profile': {'email': 'p@example.com'}}), ('profile-email-number', {'https://api.openai.com/profile': {'email': 7}}),
    ('profile-not-an-object', {'https://api.openai.com/profile': 'x', 'https://api.openai.com/auth': [1]}), ('no-email', {'sub': 'x'}), ('claims-not-an-object', [1, 2]),
    ('claims-unicode', {'email': '用户@example.com'}), ('payload-not-json', base64.urlsafe_b64encode(b'not json').decode().rstrip('=')),
    ('payload-not-utf8', base64.urlsafe_b64encode(b'{"email": "\xff"}').decode().rstrip('=')), ('payload-with-bom', base64.urlsafe_b64encode('﻿{"email": "x@example.com"}'.encode()).decode()),
    ('payload-not-base64', '!!!'), ('payload-odd-length', 'e30x'), ('payload-one-char', 'e'), ('payload-not-ascii', '邮箱'), ('payload-empty', ''),
    ('payload-junk-characters-skipped', '*'.join(b64({'email': 'junk@example.com'}))), ('payload-already-padded', base64.urlsafe_b64encode(json.dumps({'email': 'pad@example.com', 'x': 1}).encode()).decode()),
    ('payload-url-safe-characters', b64({'email': 'safe@example.com', 'p': '>>>???~~~'})),
]
for name, claims in CLAIMS:
    @case('email/codex-%s' % name, ['email', ERIN], tool='codex')
    def _(b, claims=claims):
        world(b, codex={ERIN: 'HK-A'})
        auth_json(b, claims)

TOKENS = [('tokens-null', None), ('tokens-list', ['x']), ('id-token-missing', {'access_token': 'test-token-not-real'}), ('id-token-number', {'id_token': 5}), ('id-token-one-dot', {'id_token': 'a.b'}),
          ('id-token-three-dots', {'id_token': 'a.b.c.d'}), ('id-token-no-dots', {'id_token': 'abc'})]
for name, tokens in TOKENS:
    @case('email/codex-%s' % name, ['email', ERIN], tool='codex')
    def _(b, tokens=tokens):
        world(b, codex={ERIN: 'HK-A'})
        auth_json(b, tokens=tokens)

for name, raw in [('auth-not-json', '{oops'), ('auth-not-an-object', '[1]'), ('auth-empty', '')]:
    @case('email/codex-%s' % name, ['email', ERIN], tool='codex')
    def _(b, raw=raw):
        world(b, codex={ERIN: 'HK-A'})
        auth_json(b, raw=raw)


@case('email/codex-not-logged-in', ['email', ERIN], tool='codex')
def _(b):
    world(b, codex={ERIN: 'HK-A'})


@case('email/codex-alias-key', ['email', '备用(fred@example.com)'], tool='codex')
def _(b):
    world(b, codex={'备用(fred@example.com)': 'HK-A'})
    auth_json(b, {'email': 'someone-else@example.com'}, key='fred@example.com')


def agy_token(b, j, key='gina@example.com', login=None):
    b.write('.cca/agy/accounts/%s/gemini/antigravity-cli/antigravity-oauth-token' % ident(key), j if isinstance(j, (str, bytes)) else json.dumps(j))
    if login is not None:
        b.write('.cca/agy/accounts/%s/agy-login.json' % ident(key), login if isinstance(login, str) else json.dumps(login))


AGY = [
    ('ok', {'id_token_email': 'gina@example.com', 'user_tier': 'g1-pro-tier', 'access_token': 'test-token-not-real'}, None), ('tier-not-text', {'id_token_email': 'gina@example.com', 'user_tier': 5}, None),
    ('email-empty-uses-recorded', {'id_token_email': ''}, {'email': 'recorded@example.com'}), ('email-number-uses-recorded', {'id_token_email': 5}, {'email': 'recorded@example.com'}),
    ('token-not-json-uses-recorded', 'opaque token text', {'email': 'recorded@example.com', 'at': 1}), ('token-list-uses-recorded', '[1]', {'email': 'recorded@example.com'}),
    ('recorded-not-an-email', {}, {'email': 'not an email'}), ('recorded-email-with-newline', {}, {'email': 'recorded@example.com\n'}), ('recorded-number', {}, {'email': 5}),
    ('recorded-not-an-object', {}, '["recorded@example.com"]'), ('recorded-not-json', {}, '{oops'), ('nothing-recorded', {}, None), ('token-empty-file', '', {'email': 'recorded@example.com'}),
]
for name, token, recorded in AGY:
    @case('email/agy-%s' % name, ['email', 'gina@example.com'], tool='agy')
    def _(b, token=token, recorded=recorded):
        world(b, agy={'gina@example.com': 'US-A'})
        agy_token(b, token, login=recorded)


@case('email/agy-no-token-file', ['email', 'gina@example.com'], tool='agy')
def _(b):
    world(b, agy={'gina@example.com': 'US-A'})
    b.write('.cca/agy/accounts/%s/agy-login.json' % ident('gina@example.com'), json.dumps({'email': 'recorded@example.com'}))      # token 文件不在：记下的也不算


@case('email/agy-token-is-a-directory', ['email', 'gina@example.com'], tool='agy')
def _(b):
    world(b, agy={'gina@example.com': 'US-A'})
    os.makedirs(b.path('.cca/agy/accounts/%s/gemini/antigravity-cli/antigravity-oauth-token' % ident('gina@example.com')))
    b.write('.cca/agy/accounts/%s/agy-login.json' % ident('gina@example.com'), json.dumps({'email': 'recorded@example.com'}))
