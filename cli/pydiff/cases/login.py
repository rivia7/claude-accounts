"""登录的辅助（cli/src/cmd/login.rs 和 login/ 下的几份）：login-url、login-check、node-fp、login-page-acct、agy-probe、agy-login、
login-forward、login-catch。

不联网、不开真的浏览器、不碰真的剪贴板：
- `cca _browser 网址`（SELF_PATH）是沙箱里假的 cca（下面的 BROWSER）：收到的参数记进 calls.log。
- `agy` 是沙箱里假的：探测用的（AGY_PROBE）照参数里的数据目录写日志；登录用的（AGY_LOGIN）照这个用例放的 agy-plan 说话、
  读 stdin，收到的参数、环境、授权码记进 agy.log。
- `pbcopy` 是沙箱里假的（PBCOPY）：内容写进沙箱的 clipboard。**凡是会拿到验证码的 login-catch 用例都要有它**（catch() 总带上）：
  对拍的沙箱没有挡 pbcopy，没有假的就落到这台机器真的剪贴板上。
- login-forward 连的“本机回调”是这份文件里起的假服务（CALLBACK，跑在对拍这个进程的后台线程里，只听 127.0.0.1 / ::1）：
  收到的请求原样记进沙箱的 forward.log（对拍连它一起比：两边发出去的字节差一个都看得出来），回答什么由用例定。
- login-catch 自己在本机听端口：用例用 during= 在它运行期间从另一条线程连上去（等到它开始听为止），发几个请求，把每个
  回应原样记进沙箱的 reply-N.bin。端口一个用例一个，是登记用例时现找的空端口（两遍先后用同一个）。
- 这些端口（连同假服务听的那两个）都出自这一份对拍自己占着的一段（port_block）：几份对拍同时跑也不会撞。

夹具里的网址、state、验证码都是一眼看得出来的假值。每次不一样的只有：probe.json 的 checked、agy-login.json 的 at（时间戳）、
探测用的临时目录名里那 8 个随机字符，用 mask 换掉。

信号（SIGTERM、SIGINT）由假的 agy / 假的 cca 发给它的父进程（就是被测的那个）：during= 拿不到进程号。3 号描述符是一条会卡住的
管道时的那几条路（卡着的时候来信号、答案晚到、写的那头关了）fd3= 给不了，没有用例：移植时另外拿两边各跑过一遍。

有几个用例是在等时间的（各跑两遍）：login-catch 等半截请求 10 秒、agy 不理 SIGTERM 等 5 秒。要等更久的（login-forward 对方
不回答等 120 秒、agy 探测不退出等 30 秒、pbcopy 不退出等 10 秒）平时不登记，CCA_PYDIFF_SLOW=1 时才跑。"""
import hashlib
import json
import os
import select
import socket
import threading
import time
from urllib.parse import quote

from run import case
from cases.router import ident
from cases.table import FILL, NODES, TABLE, world

SLOW = os.environ.get('CCA_PYDIFF_SLOW') == '1'
ALICE = '主号(alice@example.com)'
BOB = 'bob@example.com'


def fp(kind, server, port, sni=''):
    """节点的摘要（照 cca 的 node_fp 现算）"""
    s = '%s|%s|%s|%s' % (kind.strip().lower(), server.strip().lower().rstrip('.'), str(port).strip(), sni.strip().lower().rstrip('.'))
    return hashlib.sha256(s.encode('utf-8')).hexdigest()[:16]


FPA = fp('vless', 'a.example.com', 443, 'sni-a.example.com')
FPB = fp('ss', '192.0.2.21', 8388)

# ---- login-url：登录网址上补 login_hint ------------------------------------------------------------------------------------
CLAUDE_URL = 'https://claude.ai/oauth/authorize'
CODEX_URL = 'https://auth.openai.com/oauth/authorize'
AGY_URL = 'https://accounts.google.com/o/oauth2/auth'
HINTS = [
    ('plain', ALICE, CLAUDE_URL + '?a=1'), ('no-query', ALICE, CLAUDE_URL), ('empty-query', ALICE, CLAUDE_URL + '?'), ('fragment', ALICE, CLAUDE_URL + '?a=1#frag?x'),
    ('empty-fragment', ALICE, CLAUDE_URL + '?a=1#'), ('upper-scheme-and-host', ALICE, 'HTTPS://Claude.AI/oauth/authorize?a=1'),
    ('leading-space-tabs-newline', ALICE, ' https://claude.ai/oauth/\tauthorize?a=1\n'), ('userinfo-port', ALICE, 'https://u:p@claude.ai:443/oauth/authorize?a=1&&b'),
    ('claude-com-subpath', ALICE, 'https://claude.com/cai/oauth/authorize?code=true'), ('platform', ALICE, 'https://platform.claude.com/oauth/authorize?x=%20'),
    ('console', ALICE, 'https://console.anthropic.com/oauth/authorize?x=1'), ('has-hint', ALICE, CLAUDE_URL + '?a=1&login_hint=x%40y.z'),
    ('has-blank-hint', ALICE, CLAUDE_URL + '?login_hint'), ('hint-other-case', ALICE, CLAUDE_URL + '?LOGIN_HINT=x'), ('http', ALICE, 'http://claude.ai/oauth/authorize?a=1'),
    ('host-trailing-dot', ALICE, 'https://claude.ai./oauth/authorize?a=1'), ('other-host', ALICE, 'https://evil.example.com/oauth/authorize?a=1'),
    ('other-path', ALICE, CLAUDE_URL + '/x?a=1'), ('not-a-url', ALICE, 'hello world'), ('empty-url', ALICE, ''), ('bad-brackets', ALICE, 'https://[::1/oauth/authorize'),
    ('ipv6-host', ALICE, 'https://[::1]/oauth/authorize'), ('email-only-key', BOB, CLAUDE_URL + '?a=1'), ('alias-only-key', '只有别名', CLAUDE_URL + '?a=1'),
    ('empty-key', '', CLAUDE_URL + '?a=1'), ('email-needs-quoting', "o'neil+x/y%z&w=1@example.com", CLAUDE_URL), ('email-not-ascii', '邮箱@例子.com', CLAUDE_URL),
    ('email-trailing-newline', 'a@b.c\n', CLAUDE_URL), ('codex-url-under-claude', ALICE, CODEX_URL + '?a=1'), ('agy-url-under-claude', ALICE, AGY_URL + '?a=1'),
]
for name, key, url in HINTS:
    @case('login-url/' + name, ['login-url', key, url])
    def _(b):
        pass

for name, tool, url in [('codex', 'codex', CODEX_URL + '?a=1'), ('codex-other-path', 'codex', 'https://auth.openai.com/authorize?a=1'), ('codex-claude-url', 'codex', CLAUDE_URL + '?a=1'),
                        ('agy', 'agy', AGY_URL + '?a=1'), ('agy-v2', 'agy', 'https://accounts.google.com/o/oauth2/v2/auth?a=1#f'), ('agy-path-suffix', 'agy', 'https://accounts.google.com/x/o/oauth2/auth'),
                        ('agy-other-path', 'agy', 'https://accounts.google.com/o/oauth2/auth/x?a=1'), ('agy-claude-url', 'agy', CLAUDE_URL + '?a=1')]:
    @case('login-url/' + name, ['login-url', ALICE, url], tool=tool)
    def _(b):
        pass

for name, argv in [('no-args', []), ('one-arg', [ALICE]), ('three-args', [ALICE, CLAUDE_URL, 'x'])]:
    @case('login-url/' + name, ['login-url'] + argv)
    def _(b):
        pass


# ---- login-check：是不是这个工具的登录页 ------------------------------------------------------------------------------------
def authorize(base=CLAUDE_URL, redirect='http://localhost:1455/callback', state='st-fake-1', **change):
    """一个登录网址（参数都是假的）；change 里值是 None 的参数去掉"""
    q = {'client_id': 'fake-client', 'response_type': 'code', 'redirect_uri': redirect, 'scope': 'user:inference', 'code_challenge': 'FAKE-CHALLENGE',
         'code_challenge_method': 'S256', 'state': state}
    q.update(change)
    return base + '?' + '&'.join('%s=%s' % (k, quote(v, safe='')) for k, v in q.items() if v is not None)


GOOGLE = authorize(AGY_URL, 'https://antigravity.google/oauth-callback', 'st-fake-agy')
CHECKS = [
    ('claude', authorize()), ('claude-com', authorize('https://claude.com/cai/oauth/authorize')), ('codex', authorize(CODEX_URL, 'http://localhost:1455/auth/callback')),
    ('agy', GOOGLE), ('agy-v2', authorize('https://accounts.google.com/o/oauth2/v2/auth', 'https://antigravity.google/oauth-callback')),
    ('127-0-0-1', authorize(redirect='http://127.0.0.1:54545/callback')), ('no-redirect-path', authorize(redirect='http://localhost:9')),
    ('redirect-elsewhere', authorize(redirect='http://example.com:1455/callback')), ('redirect-https', authorize(redirect='https://localhost:1455/callback')),
    ('redirect-no-port', authorize(redirect='http://localhost/callback')), ('redirect-port-0', authorize(redirect='http://localhost:0/callback')),
    ('redirect-port-too-big', authorize(redirect='http://localhost:65536/callback')), ('redirect-port-not-a-number', authorize(redirect='http://localhost:abc/callback')),
    ('redirect-ipv6', authorize(redirect='http://[::1]:1455/callback')), ('redirect-bad-brackets', authorize(redirect='http://[::1:1455/callback')),
    ('no-redirect', authorize(redirect=None)), ('no-pkce', authorize(code_challenge=None)), ('empty-pkce', authorize(code_challenge='')),
    ('response-type-token', authorize(response_type='token')), ('no-state', authorize(state=None)), ('empty-state', authorize(state='')),
    ('state-with-hash', authorize(state='a#b')), ('state-with-space', authorize(state='a b')), ('state-trailing-newline', authorize(state='abc\n')),
    ('state-too-long', authorize(state='s' * 4097)), ('state-longest', authorize(state='s' * 4096)), ('state-twice', authorize() + '&state=second'),
    ('http-scheme', authorize('http://claude.ai/oauth/authorize')), ('upper-scheme', authorize('HTTPS://claude.ai/oauth/authorize')),
    ('other-host', authorize('https://evil.example.com/oauth/authorize')), ('path-suffix-only', authorize('https://claude.ai/x/oauth/authorize')),
    ('path-not-authorize', authorize('https://claude.ai/oauth/authorize/')), ('space-inside', authorize() + ' x'), ('trailing-newline', authorize() + '\n'),
    ('two-trailing-newlines', authorize() + '\n\n'), ('not-ascii', authorize() + '&x=中'), ('empty', ''), ('plain-words', 'not a url'), ('bad-brackets', 'https://[::1/oauth/authorize'),
]
# agy 的登录页另有一套认法（回到 antigravity.google 的页面，不回本机）
AGY_PAGE = 'https://antigravity.google/oauth-callback'
AGY_CHECKS = [
    ('agy-wrong-redirect', authorize(AGY_URL, 'https://antigravity.example.com/oauth-callback')), ('agy-local-redirect', authorize(AGY_URL)),
    ('agy-path-with-suffix', authorize('https://accounts.google.com/o/oauth2/auth/', AGY_PAGE)), ('agy-path-prefix', authorize('https://accounts.google.com/x/o/oauth2/auth', AGY_PAGE)),
    ('agy-no-state', authorize(AGY_URL, AGY_PAGE, state=None)), ('agy-state-with-hash', authorize(AGY_URL, AGY_PAGE, 'a#b')), ('agy-state-trailing-newline', authorize(AGY_URL, AGY_PAGE, 'abc\n')),
    ('agy-no-pkce', authorize(AGY_URL, AGY_PAGE, code_challenge=None)), ('agy-empty-pkce', authorize(AGY_URL, AGY_PAGE, code_challenge='')),
    ('agy-response-type-token', authorize(AGY_URL, AGY_PAGE, response_type='token')), ('agy-no-redirect', authorize(AGY_URL, None)),
    ('agy-http-scheme', authorize('http://accounts.google.com/o/oauth2/auth', AGY_PAGE)), ('agy-upper-host', authorize('https://Accounts.Google.COM/o/oauth2/auth', AGY_PAGE)),
    ('agy-other-google-host', authorize('https://google.com/o/oauth2/auth', AGY_PAGE)), ('agy-trailing-newline', GOOGLE + '\n'), ('agy-space-inside', GOOGLE + ' x'),
    ('agy-redirect-twice-last-wins', GOOGLE + '&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fcallback'),
]
# 三种工具各认各的：Claude 把上面的都过一遍；Codex、agy 只过和工具有关的那些
OTHER_TOOLS = ('claude', 'claude-com', 'codex', 'agy', 'agy-v2', '127-0-0-1', 'redirect-elsewhere', 'no-pkce', 'state-with-hash', 'other-host', 'trailing-newline', 'empty', 'bad-brackets')
for tool in ('claude', 'codex', 'agy'):
    for name, url in CHECKS + AGY_CHECKS:
        if tool == 'claude' or name in OTHER_TOOLS or (tool == 'agy' and name.startswith('agy-')):
            @case('login-check/%s/%s' % (tool, name), ['login-check', url], tool=tool)
            def _(b):
                pass

    @case('login-check/%s/no-args' % tool, ['login-check'], tool=tool)
    def _(b):
        pass


# ---- node-fp：这台机器上一个节点的摘要 --------------------------------------------------------------------------------------
CORE = {'proxies': [
    {'name': 'US-B', 'type': 'ss', 'server': '192.0.2.21', 'port': 8388},
    {'name': 'US-A', 'type': 'vless', 'server': 'A.example.com.', 'port': '443', 'servername': 'SNI-A.example.com'},
    {'name': 'no-server', 'type': 'ss', 'port': 1},
    {'name': 'no-server', 'type': 'ss', 'server': 'later.example.com', 'port': 1},
    {'name': 'by-sni', 'type': 'trojan', 'server': 'cdn.example.com', 'port': 443, 'sni': 'one.example.com'},
    {'name': 5, 'type': 'ss', 'server': 'number-name.example.com', 'port': 1}, 'not a dict', None,
    {'name': 'odd-types', 'type': ['x'], 'server': 'odd.example.com', 'port': 1.5, 'servername': 0, 'sni': 'used.example.com'},
], 'saved_at': 1790000000, 'nodes': ['US-A']}
CUSTOM = {'version': 1, 'proxies': [{'name': '自定义', 'type': 'hysteria2', 'server': 'custom.example.com', 'port': 8443}, {'name': 'US-B', 'type': 'ss', 'server': 'shadowed.example.com', 'port': 1},
                                    {'type': 'ss', 'server': 'nameless.example.com', 'port': 1}, 'junk']}
# 参考节点：US-C 是新的；「同一个」和 core 里的 US-B 是同一台服务器（只是叫法不同，fill_plan 不补它）；US-A 重名（以 core 的为准）
FILL_MORE = {'version': 1, 'group': 'OpenAi', 'proxies': FILL['proxies'] + [
    {'name': '同一个', 'type': 'ss', 'server': '192.0.2.21', 'port': 8388}, {'name': 'US-A', 'type': 'ss', 'server': 'fill-a.example.com', 'port': 9}]}


def nodes_world(b, cfg=None, core=CORE, custom=CUSTOM, fill=FILL_MORE, **kw):
    world(b, cfg=cfg, fill=fill, **kw)
    for rel, data in (('.cca/core/nodes.json', core), ('.cca/core/custom-nodes.json', custom)):
        if data is not None:
            b.write(rel, data if isinstance(data, (str, bytes)) else json.dumps(data, ensure_ascii=False))


for name in ['US-A', 'US-B', 'US-C', 'US-D', 'no-server', 'by-sni', '自定义', '同一个', 'odd-types', '5', 'missing', '', 'us-a']:
    @case('node-fp/defs/%s' % (name or 'empty'), ['node-fp', name])
    def _(b):
        nodes_world(b)

for name, ids in [('recorded', {'US-A': 'feedfacefeedface'}), ('recorded-number', {'US-A': 123}), ('recorded-true', {'US-A': True}), ('recorded-list', {'US-A': ['x', 1]}),
                  ('recorded-float', {'US-A': 1.5}), ('recorded-empty-falls-through', {'US-A': ''}), ('recorded-zero-falls-through', {'US-A': 0}),
                  ('recorded-null-falls-through', {'US-A': None}), ('recorded-other-node', {'US-B': 'feedfacefeedface'}), ('ids-not-a-dict', ['US-A']), ('ids-null', None)]:
    @case('node-fp/' + name, ['node-fp', 'US-A'])
    def _(b, ids=ids):
        nodes_world(b, cfg={'node_ids': ids})

for name, kw in [('no-core-files', {'core': None, 'custom': None}), ('no-files-at-all', {'core': None, 'custom': None, 'fill': None}), ('core-not-json', {'core': '{oops'}),
                 ('core-proxies-not-a-list', {'core': {'proxies': {'US-A': 1}}}), ('core-is-a-list', {'core': [1]}), ('custom-not-json', {'custom': b'\xff\xfe'}),
                 ('custom-proxies-not-a-list', {'custom': {'proxies': 'US-A'}}), ('custom-is-a-list', {'custom': []}),
                 ('fill-invalid-ignored', {'core': None, 'custom': None, 'fill': {'version': 2, 'group': 'G', 'proxies': FILL['proxies']}}),
                 ('fill-only', {'core': None, 'custom': None})]:
    for node in ('US-A', 'US-C'):
        @case('node-fp/%s/%s' % (name, node), ['node-fp', node])
        def _(b, kw=kw):
            nodes_world(b, **kw)


@case('node-fp/fill-name-taken-by-a-group-still-found', ['node-fp', 'US-C'])
def _(b):
    # core 里有个叫 US-C 的别的节点：先找到的是它
    nodes_world(b, core={'proxies': [{'name': 'US-C', 'type': 'ss', 'server': 'mine.example.com', 'port': 7}]}, custom=None)


@case('node-fp/dialer-proxy-not-filled-but-listed', ['node-fp', 'via'])
def _(b):
    nodes_world(b, core=None, custom=None, fill={'version': 1, 'group': 'G', 'proxies': [
        {'name': 'via', 'type': 'ss', 'server': 'via.example.com', 'port': 1, 'dialer-proxy': 'nowhere'}]})


for tool in ('codex', 'agy'):
    @case('node-fp/' + tool, ['node-fp', 'US-B'], tool=tool)
    def _(b):
        nodes_world(b)


@case('node-fp/fresh-home', ['node-fp', 'US-A'])
def _(b):
    pass


@case('node-fp/config-not-json', ['node-fp', 'US-A'])
def _(b):
    nodes_world(b)
    b.write('.cca/config.json', '{"nodes": ')


@case('node-fp/config-is-a-list', ['node-fp', 'US-A'])
def _(b):
    nodes_world(b)
    b.write('.cca/config.json', '[1]')


@case('node-fp/ro', ['node-fp', 'US-A'], env={'CCA_RO': '1'})
def _(b):
    nodes_world(b, cfg={'node_ids': {'US-A': FPA}})


for name, argv in [('no-args', []), ('two-args', ['US-A', 'x'])]:
    @case('node-fp/' + name, ['node-fp'] + argv)
    def _(b):
        nodes_world(b)


# ---- login-page-acct：服务器上的账号在这台 Mac 上是哪个 ----------------------------------------------------------------------
IDS = {'US-A': FPA, 'US-B': FPB}
PAGE = [
    # 这台 Mac 上有的账号
    ('email', ['alice@example.com']), ('email-upper-and-spaces', ['  ALICE@Example.COM ']), ('alias', ['主号']), ('prefix', ['bo']), ('number', ['2']),
    ('full-key', [ALICE]), ('email-no-prefix-guessing', ['alice@example.co']), ('alias-unknown', ['没有的']), ('alias-prefix-of-api-account', ['走']), ('empty-who', ['']),
    ('api-account', ['直连']), ('api-account-with-node', ['走节点', 'HK-A']), ('same-node-by-name', ['alice@example.com', 'US-A']),
    ('other-node-by-name', ['alice@example.com', 'HK-A']), ('same-node-by-id', ['alice@example.com', 'US-A1', FPA]), ('other-id-same-name', ['alice@example.com', 'US-A', FPB]),
    ('id-with-trailing-newline', ['alice@example.com', 'US-A1', FPA + '\n']), ('id-without-node', ['alice@example.com', '', FPB]),
    ('bad-id', ['alice@example.com', 'US-A', 'xyz']), ('bad-id-upper', ['alice@example.com', 'US-A', FPA.upper()]), ('bad-id-short', ['alice@example.com', 'US-A', FPA[:15]]),
    ('bad-id-before-lookup', ['nobody@example.com', 'US-A', 'not-hex']), ('alias-ignored-when-known', ['alice@example.com', 'US-A', FPA, '别的名字']),
    # 这台 Mac 上还没有的账号
    ('new-no-node', ['dora@example.com']), ('new-by-name', ['dora@example.com', 'HK-A']), ('new-with-alias', ['Dora@Example.com', 'HK-A', '', ' 新号 ']),
    ('new-alias-taken', ['dora@example.com', 'HK-A', '', '主号']), ('new-alias-bad', ['dora@example.com', 'HK-A', '', 'a b']), ('new-alias-too-wide', ['dora@example.com', 'HK-A', '', '很' * 13]),
    ('new-alias-blank', ['dora@example.com', 'HK-A', '', '   ']), ('new-by-id-other-name', ['dora@example.com', 'US-B2', FPB]), ('new-by-id-prefers-same-name', ['dora@example.com', 'US-A', FPA]),
    ('new-id-unknown-name-known', ['dora@example.com', 'HK-A', '0123456789abcdef']), ('new-id-known-elsewhere-name-differs', ['dora@example.com', 'US-A', FPB]),
    ('new-node-unknown', ['dora@example.com', 'XX-Z', '1111111111111111']), ('new-node-unknown-no-id', ['dora@example.com', 'XX-Z']),
]
for name, argv in PAGE:
    @case('login-page-acct/' + name, ['login-page-acct'] + argv)
    def _(b):
        nodes_world(b, cfg={'node_ids': IDS}, core=None, custom=None, fill=None)

for tool in ('codex', 'agy'):
    for name, argv in [('known', ['erin@example.com', 'US-A', FPA]), ('known-other-node', ['erin@example.com', 'HK-A']), ('new-no-node', ['dora@example.com']), ('new', ['dora@example.com', 'US-B'])]:
        @case('login-page-acct/%s/%s' % (tool, name), ['login-page-acct'] + argv, tool=tool)
        def _(b, tool=tool):
            nodes_world(b, cfg={'node_ids': IDS}, core=None, custom=None, fill=None, **{tool: {'erin@example.com': 'US-A'}})


# 这边认节点的地址靠的不只是 node_ids：拷来的节点定义、参考节点也算
for name, argv in [('known-id-from-core-defs', ['alice@example.com', '别的叫法', FPA]), ('known-id-mismatch-from-core-defs', ['alice@example.com', 'US-A', FPB]),
                   ('new-id-from-core-defs', ['dora@example.com', '别的叫法', FPB]), ('new-id-from-fill', ['dora@example.com', '别的叫法', fp('vless', 'c.example.com', 443, 'sni.example.com')])]:
    @case('login-page-acct/' + name, ['login-page-acct'] + argv)
    def _(b):
        nodes_world(b, cfg={'nodes': NODES + ['US-C']})

# 维护者的映射里有这个账号、这台机器上删掉过（不会自动加回来）：加的时候用映射要的节点
MAPPED = {'version': 1, 'claude': {'Dora@example.com': 'US-B', 'dave@example.com': 'ZZ-none', 'old@example.com': 'OLD-B'}}
for name, argv in [('mapped-same', ['dora@example.com', 'US-B']), ('mapped-same-by-id', ['dora@example.com', '别的叫法', FPB]), ('mapped-differs', ['dora@example.com', 'HK-A']),
                   ('mapped-differs-by-id', ['dora@example.com', 'US-B', FPA]), ('mapped-node-not-selectable', ['dave@example.com', 'ZZ-none']),
                   ('mapped-node-not-selectable-differs', ['dave@example.com', 'US-A']), ('mapped-renamed-node', ['old@example.com', 'US-B']),
                   ('mapped-renamed-node-old-name', ['old@example.com', 'OLD-B'])]:
    @case('login-page-acct/' + name, ['login-page-acct'] + argv)
    def _(b):
        nodes_world(b, cfg={'node_ids': IDS, 'policy_skip': ['dora@example.com', 'old@example.com'], 'node_renames': {'OLD-B': 'US-B'}}, core=None, custom=None, fill=None, policy=MAPPED)


@case('login-page-acct/mapped-account-gets-added-first', ['login-page-acct', 'dora@example.com', 'US-B'])
def _(b):
    # 没删掉过的：ctx 先照映射把它加进账号表，于是算“已有的账号”
    nodes_world(b, cfg={'node_ids': IDS}, core=None, custom=None, fill=None, policy=MAPPED)


@case('login-page-acct/mapped-account-ro', ['login-page-acct', 'dora@example.com', 'US-B'], env={'CCA_RO': '1'})
def _(b):
    nodes_world(b, cfg={'node_ids': IDS}, core=None, custom=None, fill=None, policy=MAPPED)


# 可选节点被手改成别的样子
for name, nodes in [('nodes-string', 'US-A,HK-A'), ('nodes-dict', {'HK-A': 1}), ('nodes-null', None), ('nodes-number', 7), ('nodes-mixed', [1, None, True, 'HK-A', 2.5]),
                    ('nodes-nested-list', ['US-A', ['HK-A']]), ('nodes-nested-dict', [{'HK-A': 1}, 'HK-A']), ('nodes-empty', [])]:
    for who, node in (('dora@example.com', 'HK-A'), ('dora@example.com', 'A')):
        @case('login-page-acct/%s/%s' % (name, node), ['login-page-acct', who, node])
        def _(b, nodes=nodes):
            nodes_world(b, cfg={'nodes': nodes}, core=None, custom=None, fill=None, table={})


for name, argv in [('no-args', []), ('five-args', ['alice@example.com', 'US-A', FPA, 'x', 'y'])]:
    @case('login-page-acct/' + name, ['login-page-acct'] + argv)
    def _(b):
        nodes_world(b)


@case('login-page-acct/ambiguous', ['login-page-acct', 'bo'])
def _(b):
    nodes_world(b, table=dict(TABLE, **{'boris@example.com': 'HK-A'}))


@case('login-page-acct/bad-table', ['login-page-acct', 'alice@example.com'])
def _(b):
    nodes_world(b)
    b.write('.cca/accounts.json', '[]')


# ---- agy-probe：这个版本的 agy 是不是把登录存进数据目录 ----------------------------------------------------------------------
# 假的 agy：收到的参数、工作目录、stdin、看得出该不该带的那些环境变量记进 agy.log；日志照 agy-logs.sh 写（没有就写那一句）
AGY_PROBE = r'''printf '%s\n' "agy $*" >> "$HOME/agy.log"
printf 'cwd=%s\n' "$PWD" >> "$HOME/agy.log"
if read -r line; then echo "stdin=[$line]"; else echo 'stdin=eof'; fi >> "$HOME/agy.log"
env | LC_ALL=C sort | grep -E '^(SSH_|[A-Za-z_]*PROXY|[a-z_]*proxy|AGY_|agy_|GEMINI_|gemini_|GOOGLE_|ANTIGRAVITY_|KEEP_)' >> "$HOME/agy.log"
echo 'noise on stdout'; echo 'noise on stderr' >&2
g="${1#--gemini_dir=}"
L="$g/antigravity-cli/log"
SAID='I1112 11:01:12.000001 1 composite_token_storage.go:125] Using file-based token storage because SSH session detected'
if [ -f "$HOME/agy-logs.sh" ]; then . "$HOME/agy-logs.sh"; else mkdir -p "$L"; echo "$SAID" > "$L/acp-login-20261112_110112-1.log"; fi
exit 0
'''
PROBE_MASK = [(r'cca-agy-probe\.[a-z0-9_]{8}', 'cca-agy-probe.XXXXXXXX'), (r'"checked": \d+', '"checked": T')]


def probe(name, logs=None, argv=None, env=None, tool='agy', setup=None, **kw):
    @case('agy-probe/' + name, ['agy-probe'] + (['%HOME%/bin/agy'] if argv is None else argv), tool=tool, env=env, mask=PROBE_MASK, **kw)
    def _(b):
        world(b)
        b.fake('agy', AGY_PROBE)
        if logs is not None:
            b.write('agy-logs.sh', logs)
        if setup:
            setup(b)


def sig(b, rel='bin/agy'):
    st = os.stat(b.path(rel))
    return [b.path(rel), st.st_size, int(st.st_mtime)]


probe('files')
probe('files-claude-tool', tool='claude')
probe('files-codex-tool', tool='codex')
probe('keychain', logs='mkdir -p "$L"; echo "I1112 main.go:1] agy 9.9.9" > "$L/acp-login-1.log"\n')
probe('no-logs', logs=':\n')
probe('empty-log-dir', logs='mkdir -p "$L"\n')
# 那一句被两份日志接起来才凑成：也算（照名字的先后接）
probe('said-across-two-files', logs='mkdir -p "$L"; printf "x Using file-based token storage because SSH" > "$L/a.log"; printf " session detected\\n" > "$L/b.log"\n')
probe('said-across-two-files-wrong-order', logs='mkdir -p "$L"; printf "x Using file-based token storage because SSH" > "$L/b.log"; printf " session detected\\n" > "$L/a.log"\n')
probe('said-only-in-hidden-file', logs='mkdir -p "$L"; echo "$SAID" > "$L/.hidden.log"\n')
probe('said-only-in-other-suffix', logs='mkdir -p "$L"; echo "$SAID" > "$L/a.txt"; echo "$SAID" > "$L/b.LOG"; echo "$SAID" > "$L/log"\n')
probe('said-only-in-subdirectory', logs='mkdir -p "$L/sub.log"; echo "$SAID" > "$L/sub.log/a.log"\n')
probe('said-next-to-a-directory', logs='mkdir -p "$L/a.log"; echo "$SAID" > "$L/b.log"\n')
probe('said-among-bad-bytes', logs='mkdir -p "$L"; printf "\\377\\376 %s \\344\\270\\r\\n" "$SAID" > "$L/a.log"\n')
probe('said-in-other-case', logs='mkdir -p "$L"; echo "using FILE-BASED token storage because ssh session detected" > "$L/a.log"\n')
probe('said-with-a-line-break-inside', logs='mkdir -p "$L"; printf "Using file-based token storage\\nbecause SSH session detected\\n" > "$L/a.log"\n')
probe('agy-exits-nonzero', logs='mkdir -p "$L"; echo "$SAID" > "$L/a.log"; exit 9\n')
probe('agy-killed', logs='mkdir -p "$L"; echo "$SAID" > "$L/a.log"; kill -9 $$\n')
probe('env-is-scrubbed', env={'AGY_TOKEN': 'x', 'agy_lower': 'x', 'GEMINI_API_KEY': 'x', 'gemini_lower': 'x', 'GOOGLE_CLOUD_PROJECT': 'x', 'ANTIGRAVITY_GEMINI_DIR': 'x',
                              'SSH_AUTH_SOCK': '/x', 'SSH_CONNECTION': '203.0.113.9 1 203.0.113.10 22', 'https_proxy': 'http://192.0.2.10:7901', 'HTTPS_PROXY': 'http://192.0.2.10:7901',
                              'http_proxy': 'x', 'ALL_PROXY': 'socks5://192.0.2.10:1', 'all_proxy': 'x', 'no_proxy': 'localhost', 'NO_PROXY': 'localhost',
                              'KEEP_ME': 'yes', 'KEEP_HTTP_PROXY': 'kept', 'HTTP_PROXY_X': 'kept', 'FTP_PROXY': 'kept', 'XHTTPS_PROXY': 'kept', 'MY_GOOGLE_': 'kept'})
probe('ro', env={'CCA_RO': '1'})
probe('missing', argv=['%HOME%/nowhere/agy'])
probe('empty-path', argv=[''])
probe('no-args', argv=[])
probe('two-args', argv=['%HOME%/bin/agy', 'x'])
probe('relative-path', argv=['bin/agy'])
probe('not-executable', setup=lambda b: os.chmod(b.path('bin/agy'), 0o644))
probe('is-a-directory', argv=['%HOME%/adir'], setup=lambda b: os.makedirs(b.path('adir')))
probe('symlink', argv=['%HOME%/link-to-agy'], setup=lambda b: os.symlink(b.path('bin/agy'), b.path('link-to-agy')))
probe('symlink-cached-by-real-path', argv=['%HOME%/link-to-agy'],
      setup=lambda b: (os.symlink(b.path('bin/agy'), b.path('link-to-agy')), b.json('.cca/agy/probe.json', {'sig': sig(b), 'files': True, 'checked': 5})))
probe('cached-true', setup=lambda b: b.json('.cca/agy/probe.json', {'sig': sig(b), 'files': True, 'checked': 5}))
probe('cached-false', setup=lambda b: b.json('.cca/agy/probe.json', {'sig': sig(b), 'files': False, 'checked': 5, 'more': [1]}))
probe('cached-float-mtime-still-equal', setup=lambda b: b.json('.cca/agy/probe.json', {'sig': sig(b)[:2] + [float(sig(b)[2])], 'files': False}))
probe('cached-loose-mode-kept', setup=lambda b: b.json('.cca/agy/probe.json', {'sig': sig(b), 'files': True}, mode=0o644))
probe('cache-other-size', setup=lambda b: b.json('.cca/agy/probe.json', {'sig': [sig(b)[0], sig(b)[1] + 1, sig(b)[2]], 'files': False, 'checked': 5}))
probe('cache-other-mtime', setup=lambda b: b.json('.cca/agy/probe.json', {'sig': [sig(b)[0], sig(b)[1], sig(b)[2] - 1], 'files': False, 'checked': 5}))
probe('cache-other-path', setup=lambda b: b.json('.cca/agy/probe.json', {'sig': ['/usr/local/bin/agy'] + sig(b)[1:], 'files': False, 'checked': 5}))
probe('cache-sig-shorter', setup=lambda b: b.json('.cca/agy/probe.json', {'sig': sig(b)[:2], 'files': False}))
probe('cache-files-not-bool', setup=lambda b: b.json('.cca/agy/probe.json', {'sig': sig(b), 'files': 'yes'}))
probe('cache-files-one', setup=lambda b: b.json('.cca/agy/probe.json', {'sig': sig(b), 'files': 1}))
probe('cache-files-missing', setup=lambda b: b.json('.cca/agy/probe.json', {'sig': sig(b)}))
probe('cache-not-json', setup=lambda b: b.write('.cca/agy/probe.json', '{oops'))
probe('cache-is-a-list', setup=lambda b: b.write('.cca/agy/probe.json', '[true]'))
probe('cache-loose-mode-tightened', setup=lambda b: b.write('.cca/agy/probe.json', '{}', 0o644))
probe('cache-dir-is-a-file', setup=lambda b: b.write('.cca/agy', 'not a directory'))
# 临时目录的路径里有通配符：原来的 glob 把整个路径当通配符看，成对的方括号对不上它自己，日志读不到
probe('tmpdir-with-brackets', env={'TMPDIR': '%HOME%/t[x]mp'}, setup=lambda b: os.makedirs(b.path('t[x]mp')))
probe('tmpdir-with-star-and-open-bracket', env={'TMPDIR': '%HOME%/t*m[p?'}, setup=lambda b: os.makedirs(b.path('t*m[p?')))
probe('tmpdir-missing-falls-back', env={'TMPDIR': '%HOME%/nowhere'})
if SLOW:
    # agy 不退出：30 秒到点杀掉，照它已经写下的日志认
    probe('timeout', logs='mkdir -p "$L"; echo "$SAID" > "$L/a.log"; exec sleep 45\n', timeout=50)


# ---- agy-login：agy 打出登录网址、读授权码 ----------------------------------------------------------------------------------
# 假的 cca（SELF_PATH）：_browser 收到的参数记进 calls.log（stdin 该是空的），退出码照 browser-rc
BROWSER = r'''if read -r line; then in="[$line]"; else in=eof; fi
printf '%s stdin=%s fd3=%s\n' "cca $*" "$in" "$( { true <&3; } 2>/dev/null && echo open || echo closed)" >> "$HOME/calls.log"
[ -f "$HOME/browser-rc" ] && { read -r rc < "$HOME/browser-rc"; exit "$rc"; }
exit 0
'''
# 假的 agy：参数、环境记进 agy.log，再照这个用例的 agy-plan 做
AGY_LOGIN = r'''printf '%s\n' "agy $*" >> "$HOME/agy.log"
printf 'env SSH_CONNECTION=[%s] CCA_ACCOUNT=[%s] fd3=%s\n' "$SSH_CONNECTION" "$CCA_ACCOUNT" "$( { true <&3; } 2>/dev/null && echo open || echo closed)" >> "$HOME/agy.log"
ask() {
	printf 'Then paste the authorization code, or the full URL of the page you land on, and press Enter: '
	IFS= read -r code || { printf '\nlogin cancelled\n'; echo 'stdin closed' >> "$HOME/agy.log"; exit 1; }
	printf 'code=[%s]\n' "$code" >> "$HOME/agy.log"
}
hold() {   # 等着被停掉：收到 SIGTERM 记一笔就退出。没人来停也不留话（那时沙箱早就是别的用例的了）
	trap 'kill $s 2>/dev/null; echo got-TERM >> "$HOME/agy.log"; exit 0' TERM
	sleep 10 & s=$!
	wait $s
}
. "$HOME/agy-plan"
'''
SAY_URL = "printf 'Open this link in a browser to log in with Google:\\n\\n%%s\\n\\n' '%s'\n" % GOOGLE
DONE = "printf 'Logged in as alice@example.com.\\n'\n"
REJECT = "printf 'Error: failed to exchange the authorization code: invalid_grant\\n'\n"
USUAL = SAY_URL + 'ask\n' + DONE
AT = [(r'"at": \d+', '"at": T')]
AGY_TABLE = {ALICE: 'US-A', BOB: 'US-B', '只有别名': 'HK-A'}
CALLBACK_PAGE = 'https://antigravity.google/oauth-callback'


def agy_login(name, plan=USUAL, fd3='fake-code-1\n', mode=None, key=ALICE, env=None, tool='agy', browser=BROWSER, browser_rc=None, cfg=None, setup=None, argv=None, mask=AT, **kw):
    e = {'CCA_ACCOUNT': key}
    e.update(env or {})
    args = ['agy-login'] + (['%HOME%/bin/agy'] + ([mode] if mode is not None else []) if argv is None else argv)

    @case('agy-login/' + name, args, tool=tool, env=e, fd3=fd3, mask=mask, **kw)
    def _(b):
        world(b, agy=AGY_TABLE, cfg=cfg)
        b.fake('agy', AGY_LOGIN)
        b.write('agy-plan', plan)
        b.write('.cca/bin/cca', '#!/bin/sh\n' + browser, 0o755)
        if browser_rc is not None:
            b.write('browser-rc', '%d\n' % browser_rc)
        if setup:
            setup(b)


agy_login('ok', stdin='left on stdin\n')
agy_login('ok-email-only-key', key=BOB)
agy_login('ok-alias-only-key', key='只有别名')
agy_login('ok-unknown-mode-is-local', mode='elsewhere')
agy_login('ok-ro', env={'CCA_RO': '1'})
agy_login('remote', mode='remote', cfg={'node_ids': {'US-A': FPA}})
agy_login('remote-no-node-id', mode='remote')
agy_login('remote-email-only-key', mode='remote', key=BOB, cfg={'node_ids': {'US-B': FPB}})
agy_login('remote-alias-only-key', mode='remote', key='只有别名')
agy_login('remote-node-id-not-a-string', mode='remote', cfg={'node_ids': {'US-A': 12345}})
agy_login('remote-node-needs-quoting', mode='remote', key='引号(quote@example.com)',
          setup=lambda b: (b.cfg({'version': 2, 'nodes': ["美国 A'1"]}), b.table({"引号(quote@example.com)": "美国 A'1"}, 'agy')))
agy_login('claude-tool', tool='claude')
agy_login('claude-tool-remote', tool='claude', mode='remote')
agy_login('codex-tool', tool='codex', key='erin@example.com', setup=lambda b: b.table({'erin@example.com': 'HK-A'}, 'codex'))
agy_login('no-account', env={'CCA_ACCOUNT': None})
agy_login('empty-account', key='')
agy_login('account-not-in-table', key='nobody@example.com')
agy_login('agy-missing', argv=['%HOME%/nowhere/agy'])
agy_login('agy-empty-path', argv=[''])
agy_login('no-args', argv=[])
agy_login('three-args', argv=['%HOME%/bin/agy', 'local', 'x'])
agy_login('browser-fails', plan=SAY_URL + 'hold\n', browser_rc=1)
agy_login('browser-missing', setup=lambda b: os.remove(b.path('.cca/bin/cca')), plan=SAY_URL + 'ask\n', settle=0.4)   # agy 没人管：cca 退出后它读到头、自己退出
agy_login('empty-answer', fd3='\n', plan=SAY_URL + 'ask\n')
agy_login('blank-answer', fd3='  \t \nfake-code-1\n', plan=SAY_URL + 'ask\n')
agy_login('no-fd3', fd3=None, plan=SAY_URL + 'ask\n')
agy_login('answers-run-out', fd3='', plan=SAY_URL + 'ask\n')
agy_login('answer-without-newline', fd3='fake-code-1')
agy_login('answer-with-spaces-around', fd3='  fake-code-1 \r\nnot read\n')
agy_login('answer-not-utf8', fd3=b'\xff\xfe\nfake-code-1\n')
agy_login('bad-then-good', fd3='has space\n中文\n' + 'x' * 8193 + '\nfake-code-1\n')
agy_login('whole-url-answers', fd3='http://[bad\n%s?error=access_denied%%0A%%E6%%8B%%92&code=abc\n%s?state=st-fake-agy\n%s?code=abc&state=other\n%s?code=fake-abc&state=st-fake-agy\n'
          % ((CALLBACK_PAGE,) * 4))
agy_login('six-bad-answers', fd3='a b\n' * 7, plan=SAY_URL + 'ask\n')
agy_login('agy-asks-again', fd3='fake-bad\nfake-good\n', plan=SAY_URL + 'ask\n' + REJECT + 'ask\n' + DONE)
agy_login('asked-count-is-per-login', fd3='a b\nfake-1\na b\nfake-2\na b\nfake-3\n', plan=SAY_URL + ('ask\n' + REJECT) * 3 + 'hold\n')
agy_login('prompt-other-case-and-own-line', fd3='fake-code-1\nfake-code-2\n',
          plan=SAY_URL + "printf 'PASTE THE AUTHORIZATION CODE:\\n'; IFS= read -r a; printf 'now pa\\305\\277te the author\\304\\260zat\\304\\261on code> '; IFS= read -r b\n"
          'printf "codes=[%s][%s]\\n" "$a" "$b" >> "$HOME/agy.log"\n' + DONE)
agy_login('agy-exits-early', plan="echo 'fatal: no network'; echo 'on stderr too' >&2; exit 7\n")
agy_login('agy-says-nothing', plan='exit 0\n')
agy_login('agy-killed', plan='kill -9 $$\n')
# agy 不读了：写不进去不算错。新的 Python 退出时会为没写出去的那几个字节在 stderr 上抱怨两行（3.14 实测是下面这个说法；照更新
# 说明 3.13 起就说，开头是 Exception ignored in:；3.12、3.9.6 不说），那是解释器的话，cca-core 不学（见 FINDINGS「有意和原实现
# 不一样的地方」），比之前去掉
agy_login('agy-closes-stdin', plan=SAY_URL + "printf 'paste the authorization code: '; exec 0<&-; sleep 0.4\n" + DONE,
          mask=AT + [(r'Exception ignored (?:while finalizing file|in:) <_io\.BufferedWriter name=\d+>:?\nBrokenPipeError: \[Errno 32\] Broken pipe\n', '')])
agy_login('output-is-relayed', plan=(
    "printf 'first line\\r\\n\\n   \\n\\t  Open this link in a browser somewhere\\n'\n"
    "printf 'bytes \\377\\376 and \\344\\270 here\\n'; echo 'to stderr' >&2\n"
    "printf 'Logged in as not-an-email\\nLogged in as x@y.z (free)\\nLogged in as  two-spaces@example.com\\n'\n"
    "printf 'Logged in as carol@example.com   \\r\\n'; printf 'tail without newline'\n"))
agy_login('logged-in-on-last-line-without-newline', plan="printf 'Logged in as dave@example.com.'\n")
agy_login('logged-in-twice-last-wins', plan="printf 'Logged in as one@example.com.\\nLogged in as two@example.com\\n'\n")
agy_login('url-twice', plan=SAY_URL + SAY_URL + 'ask\n' + DONE)
agy_login('url-inside-a-line', plan="printf 'go to %%s now, please\\n' '%s'\nask\n" % GOOGLE + DONE)
agy_login('url-not-a-login-page', fd3='%s?code=fake-abc&state=anything\n' % CALLBACK_PAGE, plan="printf 'https://accounts.google.com/other?x=1\\n'\nask\n" + DONE)
agy_login('url-v2-with-fragment', plan="printf '%%s\\n' '%s#frag'\nask\n" % authorize('https://accounts.google.com/o/oauth2/v2/auth', CALLBACK_PAGE, 'st-fake-v2') + DONE,
          fd3='%s?code=fake-abc&state=st-fake-agy\n%s?code=fake-abc&state=st-fake-v2\n' % (CALLBACK_PAGE, CALLBACK_PAGE))
agy_login('long-line', plan="head -c 150000 /dev/zero | tr '\\0' 'x'; echo\n")
agy_login('mapped-account-added-on-the-way', mode='remote',
          setup=lambda b: b.policy({'version': 1, 'agy': {'new@example.com': 'US-B'}}))
agy_login('login-file-cannot-be-saved', plan=DONE + 'sleep 0.3\n', setup=lambda b: b.write('.cca/agy/accounts/%s' % ident('alice@example.com'), 'in the way'))
# 被信号停下：agy 打出网址、等 cca 开完浏览器回来接着读它的输出（等一秒），再给 cca 发信号（原来是 KeyboardInterrupt：
# 停掉 agy，说一句，退出码 130）。信号和网址同时到的话，浏览器开没开要看谁先，两边都说不准
agy_login('sigterm', plan=SAY_URL + 'sleep 1\nkill -TERM $PPID\nhold\n')
agy_login('sigint', plan=SAY_URL + 'sleep 1\nkill -INT $PPID\nhold\n')
agy_login('sigterm-before-the-url', plan='kill -TERM $PPID\nhold\n')
agy_login('sigterm-while-browser-runs', plan=SAY_URL + 'hold\n', settle=1.4,
          browser='printf \'%s\\n\' "cca $1" >> "$HOME/calls.log"\nkill -TERM $PPID\nsleep 1\necho "not killed" >> "$HOME/calls.log"\n')
# agy 不理 SIGTERM：等 5 秒，杀掉
agy_login('sigterm-ignored-by-agy', plan=SAY_URL + "trap '' TERM\nsleep 1\nkill -TERM $PPID\nsleep 6\necho 'not killed' >> \"$HOME/agy.log\"\n", settle=1.5)


# ---- 这份文件自己在本机开的套接字 --------------------------------------------------------------------------------------------
class off_fd3:
    """在这里面开的描述符不会落在 3 号上。对拍给 fd3= 的用例时把自己这个进程的 3 号换成答案文件、用完关掉：留在 3 号上的
    套接字会被它关掉（之后这个号码归了别人，再去关它就关错了）"""

    def __enter__(self):
        self.held = []
        while True:
            fd = os.open(os.devnull, os.O_RDONLY)
            if fd > 3:
                os.close(fd)
                return self
            self.held.append(fd)

    def __exit__(self, *exc):
        for fd in self.held:
            os.close(fd)


def listening(v6, port=0):
    with off_fd3():
        s = socket.socket(socket.AF_INET6 if v6 else socket.AF_INET, socket.SOCK_STREAM)
    try:
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        if v6:
            s.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
        s.bind(('::1' if v6 else '127.0.0.1', port))
        s.listen(8)
    except OSError:
        s.close()
        raise
    return s


def has_v6():
    try:
        listening(True).close()
        return True
    except OSError:
        return False


HAS_V6 = has_v6()
# 这份文件用的端口都从这个范围里拿：不在系统随手分配的范围里（macOS 从 49152 起，Linux 从 32768 起），一份对拍占一段
PORTS_FROM, PORTS_TO, PORTS_STEP = 20000, 32000, 64   # 一份要五十来个
CLAIMS = []   # 占着的那几段的记号（占到对拍结束）
PORTS = {'next': 0, 'end': 0}


def port_block():
    """占一段端口，返回（头一个能用的，这一段的末尾）。几份对拍会同时跑（各在各的工作树里），端口写死的话 login-catch 的
    用例就互相占着。一段的头一个端口是记号：谁在 127.0.0.1 上听着它，这一段就归谁，听到进程结束（不往沙箱外面写东西）；
    从按进程号挑的一段找起，有人占着就看下一段"""
    blocks = (PORTS_TO - PORTS_FROM) // PORTS_STEP
    for i in range(blocks):
        start = PORTS_FROM + (os.getpid() + i) % blocks * PORTS_STEP
        with off_fd3():
            s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:   # 不带 SO_REUSEADDR：别人带着它也占不了同一个
            s.bind(('127.0.0.1', start))
            s.listen(1)
        except OSError:
            s.close()
            continue
        CLAIMS.append(s)
        return start + 1, start + PORTS_STEP
    raise RuntimeError('%d 到 %d 之间找不到一段没人占的端口' % (PORTS_FROM, PORTS_TO))


def free_port():
    """占着的这一段里下一个现在空着的端口（127.0.0.1 和 ::1 上都空着）：用例要把它写进登录网址，两遍先后用同一个"""
    while True:
        if PORTS['next'] >= PORTS['end']:   # 头一回；或者这一段用完了（被别的程序占着的太多）：再占一段
            PORTS['next'], PORTS['end'] = port_block()
        port = PORTS['next']
        PORTS['next'] += 1
        try:
            listening(False, port).close()
            if HAS_V6:
                listening(True, port).close()
            return port
        except OSError:
            continue


HELD = []   # 用例故意占着的端口（占到对拍结束）


# ---- login-forward：把贴回来的验证码转给本机的回调 ----------------------------------------------------------------------------
class FakeCallback:
    """假的本机登录回调（服务器上 claude / codex 等着的那个），跑在对拍这个进程的后台线程里。两个监听：一个只在 127.0.0.1 上，
    一个只在 ::1 上（端口不同）——网址里写 localhost 加后一个端口时，先连 127.0.0.1 连不上、再连 ::1 才通。
    每来一个连接：把请求读到空行为止，原样记进沙箱的 forward.log（先记再回答），照 arm() 定的回答，等 hold 秒，关掉"""

    def __init__(self):
        self.home, self.reply, self.hold = None, b'', 0
        self.socks = {'v4': listening(False, free_port())}
        if HAS_V6:
            self.socks['v6'] = listening(True, free_port())
        self.port = {name: s.getsockname()[1] for name, s in self.socks.items()}
        threading.Thread(target=self.serve, daemon=True).start()

    def arm(self, box, reply=b'', hold=0):
        self.home, self.reply, self.hold = box.home, reply, hold

    def serve(self):
        names = {s: name for name, s in self.socks.items()}
        while True:
            for s in select.select(list(names), [], [])[0]:
                try:
                    conn = s.accept()[0]
                except OSError:
                    continue
                threading.Thread(target=self.answer, args=(conn, names[s], self.home, self.reply, self.hold), daemon=True).start()

    @staticmethod
    def answer(conn, via, home, reply, hold):
        try:
            conn.settimeout(5)
            data = b''
            try:
                while b'\r\n\r\n' not in data:
                    chunk = conn.recv(4096)
                    if not chunk:
                        break
                    data += chunk
            except OSError:
                pass
            if home and os.path.isdir(home):
                with open(os.path.join(home, 'forward.log'), 'ab') as f:
                    f.write(b'[' + via.encode() + b'] ' + data + b'\n')
            if reply:
                conn.sendall(reply)
            if hold:
                time.sleep(hold)
        except OSError:   # 对方不等回答就走了
            pass
        finally:
            conn.close()


CALLBACK = FakeCallback()
DEAD_PORT = free_port()   # 一个没人听的端口
OK302 = b'HTTP/1.1 302 Found\r\nLocation: https://platform.example.com/oauth/code/success\r\nContent-Length: 0\r\n\r\n'


def login_url(port, host='localhost', path='/callback', state='st-fake-1', base=CLAUDE_URL):
    return authorize(base, 'http://%s:%d%s' % (host, port, path), state)


def forward(name, stdin='fake-code#st-fake-1', reply=OK302, url=None, via='v4', hold=0, argv=None, setup=None, **kw):
    @case('login-forward/' + name, ['login-forward', '%HOME%/url'] if argv is None else argv, stdin=stdin, **kw)
    def _(b):
        world(b)
        CALLBACK.arm(b, reply, hold)
        b.write('url', login_url(CALLBACK.port[via]) + '\n' if url is None else url)
        if setup:
            setup(b)


P4 = CALLBACK.port['v4']
# 登录网址的文件不对：不读 stdin，也不连
forward('no-url-file', setup=lambda b: os.remove(b.path('url')))
forward('url-file-empty', url='')
forward('url-file-not-a-login-page', url='https://example.com/oauth/authorize\n')
forward('url-file-other-tool', url=login_url(P4, base=CODEX_URL) + '\n')
forward('url-file-redirect-elsewhere', url=authorize(redirect='http://example.com:%d/callback' % P4) + '\n')
forward('url-file-two-lines', url=login_url(P4) + '\nsecond line\n')
forward('url-file-not-utf8', url=b'\xff\xfe' + login_url(P4).encode())
forward('url-file-is-a-directory', argv=['login-forward', '%HOME%'])
forward('url-file-crlf-and-spaces', url='  ' + login_url(P4) + ' \r\n\r\n')
forward('no-args', argv=['login-forward'])
forward('two-args', argv=['login-forward', '%HOME%/url', 'x'])
forward('codex-url-under-codex', url=login_url(P4, path='/auth/callback', base=CODEX_URL) + '\n', tool='codex')
forward('claude-url-under-codex', tool='codex')
forward('claude-url-under-agy', tool='agy')
# 贴的不对：不连（退出码 2，可以重新贴）
BAD_PASTE = [
    ('empty', ''), ('blank', ' \n\t\n'), ('no-hash', 'just-a-code'), ('hash-only', '#'), ('code-only', 'fake-code#'), ('state-only', '#st-fake-1'), ('code-with-space', 'fake code#st-fake-1'),
    ('two-hashes', 'a#b#st-fake-1'), ('state-mismatch', 'fake-code#st-other'), ('code-not-ascii', '验证码#st-fake-1'), ('not-utf8', b'\xff\xfe#st-fake-1'),
    ('code-too-long', 'c' * 4097 + '#st-fake-1'), ('url-no-code', 'http://localhost:1/callback?state=st-fake-1'), ('url-no-state', 'http://localhost:1/callback?code=fake-code'),
    ('url-empty-code', 'http://localhost:1/callback?code=&state=st-fake-1'), ('url-state-mismatch', 'http://localhost:1/callback?code=fake-code&state=st-other'),
    ('url-unparseable', 'http://[x?code=fake-code&state=st-fake-1'), ('url-upper-scheme-is-not-a-url', 'HTTP://localhost:1/callback?code=fake-code&state=st-fake-1'),
    ('url-error', 'http://localhost:1/callback?error=access_denied&state=st-fake-1'),
    ('url-error-description', 'https://localhost:1/callback?error=access_denied&error_description=%E4%B8%8D%E8%A1%8C+nope%0A%3Cb%3E&state=st-fake-1'),
    ('url-error-long-description', 'http://localhost:1/callback?error=x&state=st-fake-1&error_description=' + 'd' * 300),
    ('url-error-empty-description', 'http://localhost:1/callback?error=denied&error_description=&state=st-fake-1'),
    ('url-error-not-utf8', b'http://localhost:1/callback?state=st-fake-1&error=bad\xe4\xb8\xffend'),
    ('url-error-other-state', 'http://localhost:1/callback?error=access_denied&state=st-other'),
    ('url-error-other-state-with-code', 'http://localhost:1/callback?error=access_denied&code=fake-code&state=st-other'),
    ('url-error-no-state', 'http://localhost:1/callback?error=access_denied'),
]
for name, pasted in BAD_PASTE:
    forward('paste/' + name, stdin=pasted)
# 转过去了
forward('ok')
forward('ok-spaces-around', stdin='  fake-code#st-fake-1 \r\n')
forward('ok-url-form', stdin='http://localhost:1455/callback?code=fake-code&state=st-fake-1&x=1\n')
forward('ok-https-url-form', stdin='https://whatever.example.com/?state=wrong&code=first&code=fake-code&state=st-fake-1')
forward('ok-url-with-error-but-empty', stdin='http://localhost:1/callback?error=&code=fake-code&state=st-fake-1')
forward('ok-code-needs-quoting', stdin="a+b/c=d&e?f%g~h_i-j.k!*'()\"$,:;<>@[\\]^`{|}#st-fake-1")
forward('ok-code-ends-with-newline', stdin='fake-code\n#st-fake-1')
forward('ok-state-needs-quoting', stdin='fake-code#st+/=&?%', url=login_url(P4, state='st+/=&?%') + '\n')
forward('ok-longest-code', stdin='c' * 4096 + '#st-fake-1')
forward('ok-127-0-0-1', url=login_url(P4, host='127.0.0.1') + '\n')
forward('ok-no-path', url=authorize(redirect='http://localhost:%d' % P4) + '\n')
forward('ok-redirect-with-query-and-fragment', url=authorize(redirect='http://LOCALHOST:%d/cb/x?keep=no#frag' % P4) + '\n')
forward('ok-codex', url=login_url(P4, path='/auth/callback', base=CODEX_URL) + '\n', tool='codex', reply=b'HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 9\r\n\r\nall good\n')
if HAS_V6:
    forward('ok-localhost-second-address', via='v6')
    forward('refused-127-0-0-1-does-not-try-v6', url=login_url(CALLBACK.port['v6'], host='127.0.0.1') + '\n')
forward('refused', url=login_url(DEAD_PORT, host='127.0.0.1') + '\n')
forward('refused-localhost', url=login_url(DEAD_PORT) + '\n')
# 回调的路径发不出去：网址里有空格、控制字符（说的话里带着整个网址：原来就这样，见 FINDINGS），不是 ASCII 的是没人接的异常
forward('path-with-space', url=authorize(redirect='http://localhost:%d/a b' % P4) + '\n')
forward('path-with-control-character', url=authorize(redirect='http://localhost:%d/a\x7fb\x01' % P4) + '\n')
forward('path-with-quotes-and-space', url=authorize(redirect='http://localhost:%d/q\'"x y' % P4) + '\n')
forward('path-not-ascii', url=authorize(redirect='http://localhost:%d/回调' % P4) + '\n')
forward('path-not-ascii-and-space', url=authorize(redirect='http://localhost:%d/回 调' % P4) + '\n')
# 对方各种各样的回答
CHUNKED = b'HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n'
REPLIES = [
    ('200-with-body', b'HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello'), ('399', b'HTTP/1.1 399 Odd\r\nContent-Length: 0\r\n\r\n'), ('400', b'HTTP/1.1 400 Bad Request\r\nContent-Length: 3\r\n\r\nbad'),
    ('404', b'HTTP/1.1 404 Not Found\r\n\r\n'), ('500-html', b'HTTP/1.0 500 Internal Server Error\r\nContent-Type: text/html\r\n\r\n<h1>oops</h1>'), ('999', b'HTTP/1.1 999 Max\r\n\r\n'),
    ('no-answer-closed', b''), ('garbage', b'garbage\r\n\r\n'), ('one-token', b'HTTP/1.1\r\n\r\n'), ('two-tokens', b'HTTP/1.1 200\r\n\r\n'), ('two-tokens-trailing-space', b'HTTP/1.1 200 \r\n\r\n'),
    ('status-not-a-number', b'HTTP/1.1 abc OK\r\n\r\n'), ('status-99', b'HTTP/1.1 99 Low\r\n\r\n'), ('status-1000', b'HTTP/1.1 1000 High\r\n\r\n'),
    ('status-underscore', b'HTTP/1.1 4_00 OK\r\nContent-Length: 0\r\n\r\n'), ('status-plus', b'HTTP/1.1 +200 OK\r\nContent-Length: 0\r\n\r\n'), ('status-negative', b'HTTP/1.1 -200 OK\r\n\r\n'),
    ('status-huge', b'HTTP/1.1 99999999999999999999 OK\r\n\r\n'), ('status-float', b'HTTP/1.1 200.0 OK\r\n\r\n'), ('leading-space', b'  HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n'),
    ('tabs-between', b'HTTP/1.1\t200\tOK\r\nContent-Length: 0\r\n\r\n'), ('nbsp-between', b'HTTP/1.1\xa0500\xa0X\r\nContent-Length: 0\r\n\r\n'), ('lower-http', b'http/1.1 200 OK\r\n\r\n'),
    ('http2', b'HTTP/2.0 200 OK\r\n\r\n'), ('http09', b'HTTP/0.9 200 OK\r\n\r\nbody'), ('http10', b'HTTP/1.0 200 OK\r\n\r\nbody'), ('http11x', b'HTTP/1.9 200 OK\r\n\r\nbody'), ('http-slash-only', b'HTTP/ 200 OK\r\n\r\n'),
    ('blank-first-line', b'\r\nHTTP/1.1 200 OK\r\n\r\n'), ('lf-only', b'HTTP/1.1 400 Bad\nContent-Length: 0\n\n'), ('status-line-then-closed', b'HTTP/1.1 200 OK\r\n'), ('status-line-no-newline', b'HTTP/1.1 200 OK'),
    ('100-then-200', b'HTTP/1.1 100 Continue\r\nX: y\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n'), ('100-twice-then-403', b'HTTP/1.1 100 Continue\r\n\r\n' * 2 + b'HTTP/1.1 403 No\r\nContent-Length: 0\r\n\r\n'),
    ('100-then-closed', b'HTTP/1.1 100 Continue\r\n\r\n'), ('100-then-garbage', b'HTTP/1.1 100 Continue\r\n\r\noops\r\n'), ('101', b'HTTP/1.1 101 Switching\r\n\r\nrest'), ('204', b'HTTP/1.1 204 No Content\r\n\r\n'),
    ('304-with-length', b'HTTP/1.1 304 Not Modified\r\nContent-Length: 50\r\n\r\n'), ('short-body', b'HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nhello'),
    ('body-over-64k', b'HTTP/1.1 200 OK\r\nContent-Length: 100000\r\n\r\n' + b'b' * 100000), ('body-until-close-over-64k', b'HTTP/1.1 200 OK\r\n\r\n' + b'b' * 100000),
    ('content-length-not-a-number', b'HTTP/1.1 200 OK\r\nContent-Length: five\r\n\r\nhello'), ('content-length-negative', b'HTTP/1.1 200 OK\r\nContent-Length: -5\r\n\r\nhello'),
    ('content-length-huge', b'HTTP/1.1 200 OK\r\nContent-Length: 99999999999999999999999\r\n\r\nhello'), ('content-length-spaces-underscore', b'HTTP/1.1 200 OK\r\nContent-Length: \t1_0 \xa0\r\n\r\nhellohello'),
    ('content-length-twice-first-wins', b'HTTP/1.1 200 OK\r\nContent-Length: 2\r\ncontent-length: 500\r\n\r\nhello'), ('content-length-empty', b'HTTP/1.1 200 OK\r\nContent-Length:\r\n\r\nhello'),
    ('chunked', CHUNKED + b'5\r\nhello\r\n0\r\n\r\n'), ('chunked-upper', CHUNKED.replace(b'chunked', b'CHUNKED') + b'5\r\nhello\r\n0\r\n\r\n'), ('chunked-not-exactly', CHUNKED.replace(b'chunked', b'gzip, chunked') + b'hello'),
    ('chunked-and-length', CHUNKED.replace(b'\r\n\r\n', b'\r\nContent-Length: 2\r\n\r\n') + b'5\r\nhello\r\n0\r\n\r\n'), ('chunked-bad-size', CHUNKED + b'zz\r\nhello\r\n0\r\n\r\n'),
    ('chunked-no-size', CHUNKED), ('chunked-short-chunk', CHUNKED + b'5\r\nhe'), ('chunked-closed-after-chunk', CHUNKED + b'5\r\nhello\r\n'), ('chunked-closed-after-two-chunks', CHUNKED + b'5\r\nhello\r\n3\r\nabc\r\n'),
    ('chunked-no-crlf-after-chunk', CHUNKED + b'5\r\nhelloX'), ('chunked-no-trailer-end', CHUNKED + b'5\r\nhello\r\n0\r\n'), ('chunked-with-trailers', CHUNKED + b'5\r\nhello\r\n0\r\nX-T: 1\r\nX-U: 2\r\n\r\n'),
    ('chunked-negative-size', CHUNKED + b'-5\r\nhello\r\n0\r\n\r\n'), ('chunked-size-minus-one', CHUNKED + b'-1\r\nhello\r\n0\r\n\r\n'), ('chunked-size-0x-and-extension', CHUNKED + b' 0x5 ;ext=1\r\nhello\r\n0\r\n\r\n'),
    ('chunked-size-underscore-plus', CHUNKED + b'+0_5\r\nhello\r\n0\r\n\r\n'), ('chunked-size-huge', CHUNKED + b'ffffffffffffffffffffffff\r\nhello'), ('chunked-two-big-chunks', CHUNKED + (b'9c40\r\n' + b'c' * 40000 + b'\r\n') * 2 + b'0\r\n\r\n'),
    ('chunked-exactly-64k', CHUNKED + b'10000\r\n' + b'c' * 65536 + b'\r\n0\r\n\r\n'), ('chunked-size-line-too-long', CHUNKED + b'5;' + b'e' * 70000 + b'\r\nhello\r\n0\r\n\r\n'),
    ('chunked-trailer-line-too-long', CHUNKED + b'0\r\nX: ' + b't' * 70000 + b'\r\n\r\n'),
    ('status-line-too-long', b'HTTP/1.1 200 ' + b'x' * 70000 + b'\r\n\r\n'), ('status-line-65536-bytes', b'HTTP/1.1 200 ' + b'x' * (65536 - 15) + b'\r\n\r\n'),
    ('header-line-too-long', b'HTTP/1.1 200 OK\r\nX: ' + b'x' * 70000 + b'\r\n\r\n'), ('101-headers', b'HTTP/1.1 200 OK\r\n' + b''.join(b'X%d: y\r\n' % i for i in range(100)) + b'\r\n'),
    ('100-headers', b'HTTP/1.1 500 Still Read\r\n' + b''.join(b'X%d: y\r\n' % i for i in range(99)) + b'\r\n'), ('status-latin1', b'HTTP/1.1 200 \xe4\xb8\xad\r\nContent-Length: 0\r\n\r\n'),
    ('garbage-latin1', b'\xff\xfe garbage \xe4\xb8\xad\r\n\r\n'), ('garbage-with-controls', b'not\x00http\x1b[31m\r\n\r\n'),
    ('headers-stop-at-odd-line', b'HTTP/1.1 200 OK\r\nBad Header\r\nTransfer-Encoding: chunked\r\n\r\nnot chunked at all'), ('headers-bare-cr', b'HTTP/1.1 200 OK\r\nX: 1\rTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n'),
    ('header-continuation', b'HTTP/1.1 200 OK\r\nTransfer-Encoding:\r\n chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n'), ('header-from-line-first', b'HTTP/1.1 200 OK\r\nFrom me\r\nContent-Length: 2\r\n\r\nhello'),
]
for name, reply in REPLIES:
    forward('reply/' + name, reply=reply)
if SLOW:
    # 对方接了连接不回答、回了头不给正文也不关：都是等 120 秒
    forward('slow/no-answer', reply=b'', hold=126, timeout=140)
    forward('slow/headers-then-silence', reply=b'HTTP/1.1 200 OK\r\n\r\npartial', hold=126, timeout=140)


# ---- login-catch：在本机等浏览器带着验证码跳回来 ------------------------------------------------------------------------------
PBCOPY = r'''IFS= read -r line; printf '%s' "$line" > "$HOME/clipboard"
[ -f "$HOME/pbcopy-rc" ] && { read -r rc < "$HOME/pbcopy-rc"; exit "$rc"; }
exit 0
'''
LINGER = 20   # 浏览器这边等回应最多等这么久


def browser(port, steps):
    """during：等命令在这个端口上听起来，照 steps 一个个发请求（各用一个新连接），回应原样记进沙箱的 reply-N.bin。
    一步是 (4 或 6, 请求的字节[, 'keep'])：发完就关掉写的那一半（对方读到头）；带 'keep' 的不关，一直等对方先关"""
    def run(b):
        for i, (family, payload, *opts) in enumerate(steps, 1):
            addr = ('::1' if family == 6 else '127.0.0.1', port)
            end = time.time() + (10 if i == 1 else 3)
            s = None
            while s is None:
                try:
                    s = socket.create_connection(addr, timeout=LINGER)
                except OSError:
                    if time.time() > end:
                        break
                    time.sleep(0.02)
            if s is None:
                b.write('reply-%d.bin' % i, 'could not connect\n')
                continue
            data = b''
            try:
                s.sendall(payload)
                if 'keep' not in opts:
                    s.shutdown(socket.SHUT_WR)
                while True:
                    chunk = s.recv(65536)
                    if not chunk:
                        break
                    data += chunk
            except OSError as e:
                data += ('[%s]' % type(e).__name__).encode()
            finally:
                s.close()
            b.write('reply-%d.bin' % i, data)
    return run


def get(target, version=' HTTP/1.1', rest=b'Host: localhost\r\nUser-Agent: fake-browser\r\n\r\n'):
    return b'GET ' + (target if isinstance(target, bytes) else target.encode()) + version.encode() + b'\r\n' + rest


GOOD = '/callback?code=fake-code-1&state=st-fake-1'


def catch(name, steps=None, timeout=None, url=None, state='st-fake-1', path='/callback', base=CLAUDE_URL, tool='claude', browser_script=BROWSER, browser_rc=None, pbcopy_rc=None,
          hold=None, setup=None, argv=None, env=None, **kw):
    port = free_port()
    u = (login_url(port, path=path, state=state, base=base) if url is None else url).replace('PORTNUM', str(port))
    args = ['login-catch', u] + ([timeout] if timeout is not None else []) if argv is None else argv
    plan = None if steps is None else [(s[0], s[1].replace(b'PORTNUM', str(port).encode())) + tuple(s[2:]) for s in steps]

    @case('login-catch/' + name, args, tool=tool, env=env, during=browser(port, plan) if plan else None, **kw)
    def _(b):
        world(b)
        b.write('.cca/bin/cca', '#!/bin/sh\n' + browser_script, 0o755)
        b.fake('pbcopy', PBCOPY)   # 总要有：没有它就是这台机器真的剪贴板
        if browser_rc is not None:
            b.write('browser-rc', '%d\n' % browser_rc)
        if pbcopy_rc is not None:
            b.write('pbcopy-rc', '%d\n' % pbcopy_rc)
        if hold:
            HELD.append(listening(hold == 6, port))
        if setup:
            setup(b)


# 还没开始等就不干了
catch('not-a-login-url', url='https://example.com/oauth/authorize?x=1')
catch('codex-url-under-claude', base=CODEX_URL)
catch('claude-url-under-agy', tool='agy')
catch('agy-url-under-agy', url=GOOGLE, tool='agy')
catch('no-args', argv=['login-catch'])
catch('three-args', argv=['login-catch', 'x', '5', 'y'])
catch('browser-fails', browser_rc=3)
catch('browser-missing', setup=lambda b: os.remove(b.path('.cca/bin/cca')))
catch('port-busy', hold=4, timeout='5')
catch('port-busy-not-a-number-timeout-never-read', hold=4, timeout='abc')
# 超时的参数（都是先开了浏览器才看它）
catch('timeout/zero', timeout='0')
catch('timeout/fraction', timeout='0.4')
catch('timeout/one-second', timeout='1')
catch('timeout/negative', timeout='-90')
catch('timeout/negative-fraction', timeout='-0.5')
catch('timeout/huge-negative', timeout='-1e300')
catch('timeout/minus-infinity', timeout='-inf')
catch('timeout/not-a-number', timeout='soon')
catch('timeout/empty', timeout='')
catch('timeout/nan', timeout='nan')
catch('timeout/only-other-requests', timeout='2.5', steps=[(4, get('/favicon.ico')), (4, get('/callback?code=x&state=wrong'))])
# 拿到验证码
catch('ok', steps=[(4, get('/favicon.ico')), (4, get('/callback?code=fake-x&state=st-other')), (4, get('/callback?code=fake-x')), (4, get('/other?code=fake-x&state=st-fake-1')),
                   (4, get(GOOD).replace(b'GET', b'POST')), (4, get(GOOD).replace(b'GET', b'get')), (4, b'GE'), (4, b'GET'), (4, b''), (4, get(GOOD))])
catch('ok-timeout-given', timeout=' 6_0.5 ', steps=[(4, get(GOOD))])
catch('ok-timeout-infinite', timeout='inf', steps=[(4, get(GOOD))])
catch('ok-request-line-only', steps=[(4, b'GET ' + GOOD.encode())])
catch('ok-absolute-url-in-request-line', steps=[(4, get('http://localhost:PORTNUM' + GOOD))])
catch('ok-last-of-repeated-parameters-wins', steps=[(4, get('/callback?code=first&state=st-other&code=fake-code-2&state=st-fake-1&x'))])
catch('ok-extra-spaces-make-it-another-path', steps=[(4, get(' ' + GOOD)), (4, get(GOOD + ' extra words'))])
catch('ok-code-needs-escaping', steps=[(4, get('/callback?code=%3Cb%3E%26%22%27+x&state=st-fake-1'))])
catch('ok-state-needs-quoting', state='st+/=&?', steps=[(4, get('/callback?code=fake-code-1&state=st+/=&?')), (4, get('/callback?code=fake-code-1&state=st%2B%2F%3D%26%3F'))])
catch('ok-no-path-in-redirect', url=authorize(redirect='http://127.0.0.1:PORTNUM'), steps=[(4, get('/callback?code=fake-x&state=st-fake-1')), (4, get('/?code=fake-code-1&state=st-fake-1'))])
catch('ok-codex', base=CODEX_URL, path='/auth/callback', tool='codex', steps=[(4, get('/callback?code=fake-x&state=st-fake-1')), (4, get('/auth/callback?code=fake-code-1&state=st-fake-1'))])
catch('ok-16384-bytes-without-blank-line', steps=[(4, (get(GOOD, rest=b'X: ') + b'p' * 20000)[:16384], 'keep')])
catch('ok-pbcopy-fails', pbcopy_rc=1, steps=[(4, get(GOOD))])
catch('ok-pbcopy-missing', env={'PATH': '%HOME%/nowhere'}, steps=[(4, get(GOOD))])
catch('ok-path-unparseable-first', steps=[(4, get('//[x?code=fake-x&state=st-fake-1')), (4, get(GOOD))])
if HAS_V6:
    catch('ok-over-ipv6', steps=[(6, get('/callback?code=fake-x&state=st-other')), (6, get(GOOD))])
    catch('ok-ipv6-then-ipv4', steps=[(6, get('/callback?code=fake-x&state=st-other')), (4, get(GOOD))])
    catch('ok-ipv6-port-busy-ipv4-serves', hold=6, steps=[(4, get(GOOD))])
# 回调来了，但没成
catch('refused', steps=[(4, get('/callback?error=access_denied&state=st-fake-1'))])
catch('refused-with-description', steps=[(4, get('/callback?error=access_denied&error_description=%E4%B8%8D%E8%A1%8C+%3Cb%3E%26%22%27%0A' + 'd' * 300 + '&code=fake-x&state=st-fake-1'))])
catch('refused-no-code', steps=[(4, get('/callback?state=st-fake-1'))])
catch('refused-empty-code', steps=[(4, get('/callback?code=&error=&error_description=&state=st-fake-1'))])
catch('code-not-ascii', steps=[(4, get('/callback?code=%E7%A0%81&state=st-fake-1'))])
catch('code-raw-bytes-not-ascii', steps=[(4, get(b'/callback?code=\xe7\xa0\x81&state=st-fake-1'))])
# 半截请求一直不发完：等它 10 秒，不理它，接着等别的
catch('slow-request-is-dropped', steps=[(4, b'GET /callback?code=fake-x&sta', 'keep'), (4, get(GOOD))])
if SLOW:
    # pbcopy 不退出：等它 10 秒、杀掉，算没复制成
    catch('slow/pbcopy-hangs', steps=[(4, get(GOOD))], setup=lambda b: b.fake('pbcopy', 'exec sleep 30\n'))
# Ctrl-C：假的 cca 开完“浏览器”一秒后给等着的进程发 SIGINT（SIGINT 被忽略着的环境里收不到，那就都是等到点）
catch('ctrl-c', timeout='4', browser_script=BROWSER.replace('exit 0\n', '( sleep 1; kill -INT $PPID ) >/dev/null 2>&1 &\nexit 0\n'))
