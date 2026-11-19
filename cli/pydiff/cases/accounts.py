"""env 等账号相关的命令（cli/src/cmd/accounts.rs），以及它用到的 codex 探测（cli/src/cmd/codex.rs 的 codex_probe）。"""
from run import case
from cases.table import world, TABLE, NODES

PORTS = {'主号(alice@example.com)': 7901, 'bob@example.com': 7902, '走节点': 7904}

# 假的 codex：认 features list / --help / --version，把每次收到的参数和要紧的环境记进沙箱里的 codex.log
FAKE_CODEX = r'''
printf 'ARGS: %s | CODEX_HOME=%s | OPENAI=%s | cwd=%s\n' "$*" "$CODEX_HOME" "${OPENAI_API_KEY-unset}" "$PWD" >> "$HOME/codex.log"
case "$1 $2" in
	"features list") [ -f "$HOME/codex_noflags" ] && { echo "other_feature  stable  true"; exit 0; }
		[ -f "$HOME/codex_fail" ] && exit 2
		printf 'system_proxy_fallback  experimental  true\nrespect_system_proxy stable true\n\nsomething_else  x\n' ;;
	"--help ") [ -f "$HOME/codex_nodaemon" ] && echo "Usage: codex [OPTIONS]" || echo "  --no-daemon   do not attach" ;;
	"--version ") echo "codex-cli 0.167.2" ;;
esac
'''

for key in ['主号(alice@example.com)', 'bob@example.com', '直连', '走节点', '旧名(alice@example.com)', '没有这个', '']:
    @case('env/claude/%s' % (key or 'empty'), ['env', key])
    def _(b):
        world(b, cfg={'ports': PORTS, 'no_proxy': 'localhost, .local,::1,10.0.0.0/8,,'})


@case('env/claude/full-api', ['env', '全'])
def _(b):
    world(b, table={'全': {'base_url': 'https://api.example.com/v1//', 'model': 'big-model', 'node': 'US-A', 'id': 'quan',
                          'env': {'A': 1, 'B': 'x y', 'HTTPS_PROXY': 'no', '1bad': 'no', 'C': 1.5, 'D': True}}})


@case('env/claude/no-add-dir', ['env', 'bob@example.com'])
def _(b):
    world(b, cfg={'claude_add_dir': False})


@case('env/claude/no-proxy-host', ['env', 'bob@example.com'])
def _(b):
    world(b, cfg={'proxy_host': '', 'router': {}})


@case('env/claude/router-host-only', ['env', 'bob@example.com'])
def _(b):
    world(b, cfg={'proxy_host': '', 'router': {'host': '192.0.2.77'}})


@case('env/claude/self-path', ['env', '直连'], env={'CCA_SELF': "/opt/my tools/cca"})
def _(b):
    world(b)


@case('env/claude/ro', ['env', 'bob@example.com'], env={'CCA_RO': '1'})
def _(b):
    world(b)


@case('env/claude/bad-table', ['env', 'bob@example.com'])
def _(b):
    world(b, table={'bob@example.com': 'ZZ'})


CODEX_TABLE = {'工作(erin@example.com)': 'US-A', '接口': {'base_url': 'https://api.example.com/v1', 'model': 'gpt-x', 'node': 'HK-A'},
               '直连接口': {'base_url': 'https://api3.example.com/v1/'}}


def codex_world(b, **marks):
    world(b, codex=CODEX_TABLE)
    b.fake('codex', FAKE_CODEX)
    for m in marks:
        b.write(m, '')


for key in ['工作(erin@example.com)', '接口', '直连接口']:
    @case('env/codex/%s' % key, ['env', key], tool='codex', env={'CCA_BIN': '%HOME%/bin/codex', 'OPENAI_API_KEY': 'sk-leak', 'CODEX_HOME': '/elsewhere'})
    def _(b):
        codex_world(b)


@case('env/codex/no-bin', ['env', '接口'], tool='codex')
def _(b):
    codex_world(b)


@case('env/codex/missing-bin', ['env', '接口'], tool='codex', env={'CCA_BIN': '%HOME%/bin/nope'})
def _(b):
    codex_world(b)


@case('env/codex/no-flags', ['env', '接口'], tool='codex', env={'CCA_BIN': '%HOME%/bin/codex'})
def _(b):
    codex_world(b, codex_noflags=1, codex_nodaemon=1)


@case('env/codex/features-fail', ['env', '接口'], tool='codex', env={'CCA_BIN': '%HOME%/bin/codex'})
def _(b):
    codex_world(b, codex_fail=1)


@case('env/codex/cached-probe', ['env', '工作(erin@example.com)'], tool='codex', env={'CCA_BIN': '%HOME%/bin/codex'})
def _(b):
    codex_world(b)
    import os
    st = os.stat(b.path('bin/codex'))
    b.json('.cca/codex/probe.json', {'sig': '%s|%d|%d' % (os.path.realpath(b.path('bin/codex')), st.st_size, int(st.st_mtime)),
                                    'flags': ['--disable', 'only_this'], 'no_daemon': False, 'version': '9.9'})


@case('env/codex/stale-probe', ['env', '工作(erin@example.com)'], tool='codex', env={'CCA_BIN': '%HOME%/bin/codex'})
def _(b):
    codex_world(b)
    b.json('.cca/codex/probe.json', {'sig': 'old', 'flags': [], 'no_daemon': False, 'version': '0.1'})


@case('env/codex/npm-js', ['env', '工作(erin@example.com)'], tool='codex', env={'CCA_BIN': '%HOME%/pkg/bin/codex.js'})
def _(b):
    codex_world(b)
    b.write('pkg/bin/codex.js', '#!/bin/sh\n' + FAKE_CODEX, 0o755)
    b.json('pkg/package.json', {'version': '0.200.1'})


@case('env/agy', ['env', 'gina@example.com'], tool='agy')
def _(b):
    world(b, agy={'gina@example.com': 'US-B'})


# --- check ---
CHECKS = [
    ('email', ' Carol@Example.com '), ('email', 'alice@example.com'), ('email', 'ALICE@example.com'), ('email', 'nope'), ('email', ''),
    ('alias', '新别名'), ('alias', '主号'), ('alias', 'a b'), ('alias', 'x' * 25), ('alias', '一二三四五六七八九十一二三'), ('alias', ''),
    ('whatever', 'a@b'), ('url', 'https://api.example.com/v1//'), ('url', 'http://api.example.com'), ('url', 'not a url'), ('url', ''),
    ('model', 'm-1'), ('model', 'a b'), ('host', '192.0.2.10'), ('host', '-oX'), ('host', 'router.lan'), ('host', 'http://x'),
    ('sshuser', 'root'), ('sshuser', 'a b'), ('port', '22'), ('port', '0'), ('port', '65536'), ('port', '2x'), ('port', '00022'),
    ('controller', '127.0.0.1:9090'), ('controller', 'unix:/tmp/a.sock'), ('controller', 'nope'), ('controller', 'https://h:1/'),
    ('gmail', ' Me.X+1@Gmail.com '), ('gmail', 'me@gmail'), ('gmail', 'mé@gmail.com'),
]
for i, (f, v) in enumerate(CHECKS):
    @case('check/%02d-%s' % (i, f), ['check', f, v])
    def _(b):
        world(b)


@case('check/email-codex-table', ['check', 'email', 'erin@example.com'], tool='codex')
def _(b):
    world(b, codex={'erin@example.com': 'US-A'})


@case('check/alias-bad-table', ['check', 'alias', 'x'])
def _(b):
    world(b)
    b.write('.cca/accounts.json', '[]')


# --- add / add-api ---
ADDS = [
    (['add', ' Carol@Example.com ', ' 卡罗 ', 'HK-A'], {}),
    (['add', 'carol@example.com', '', 'US-B'], {}),
    (['add', 'alice@example.com', '', 'US-B'], {}),
    (['add', 'carol@example.com', '主号', 'US-B'], {}),
    (['add', 'carol@example.com', '', 'ZZ'], {}),
    (['add', 'bad', '', 'US-A'], {}),
    (['add', 'carol@example.com', '', 'US-A'], {'policy': {'version': 1, 'claude': {'carol@example.com': 'US-B'}}, 'cfg': {'policy_skip': ['carol@example.com', 'x@example.com']}}),
    (['add', 'carol@example.com', '', 'US-A'], {'policy': {'version': 1, 'claude': {'carol@example.com': 'ZZ'}}, 'cfg': {'policy_skip': ['carol@example.com']}}),
    (['add', 'carol@example.com', '', 'US-A'], {'cfg': {'nodes': []}}),
    (['add-api', ' 新接口 ', ' https://api9.example.com/v1/ ', ' big ', 'US-A'], {}),
    (['add-api', '新接口', 'https://api9.example.com/v1'], {}),
    (['add-api', '', 'https://api9.example.com/v1'], {}),
    (['add-api', '直连', 'https://api9.example.com/v1'], {}),
    (['add-api', '新接口', 'http://api9.example.com/v1', 'a b', 'ZZ'], {}),
    (['add-api', 'zjd', 'https://api9.example.com/v1'], {}),
]
for i, (argv, kw) in enumerate(ADDS):
    @case('add/%02d-%s' % (i, argv[0]), argv)
    def _(b, kw=kw):
        world(b, **kw)


@case('add/codex', ['add', 'erin@example.com', '工作', 'US-A'], tool='codex')
def _(b):
    world(b, cfg={'codex': {'ports': {}, 'last': '', 'policy_skip': ['erin@example.com']}}, codex={})


@case('add/agy-api-refused', ['add-api', 'x', 'https://api.example.com'], tool='agy')
def _(b):
    world(b, agy={})


@case('add/id-collision', ['add-api', 'dup', 'https://api9.example.com/v1'])
def _(b):
    world(b, table={'别的': {'base_url': 'https://a.example.com', 'id': 'dup'}, '再一个': {'base_url': 'https://b.example.com', 'id': 'dup-2'}})


# --- rename ---
RENAMES = [
    ('主号(alice@example.com)', '大号'), ('主号(alice@example.com)', '-'), ('主号(alice@example.com)', ' 主号 '), ('bob@example.com', '鲍勃'),
    ('bob@example.com', '主号'), ('bob@example.com', 'a b'), ('bob@example.com', ''), ('直连', '直连2'), ('直连', '-'), ('直连', '走节点'),
    ('没有', 'x'), ('光别名', '新'),
]
for i, (k, a) in enumerate(RENAMES):
    @case('rename/%02d' % i, ['rename', k, a])
    def _(b):
        world(b, table=dict(TABLE, **{'光别名': 'US-A'}), cfg={'ports': dict(PORTS), 'last': '直连'})


@case('rename/codex-api', ['rename', '接口', '新接口'], tool='codex')
def _(b):
    world(b, codex=CODEX_TABLE, cfg={'codex': {'ports': {'接口': 7907}, 'last': '接口'}})


# --- reorder / rm ---
for i, names in enumerate([['bob', '直连'], ['4', '1'], [], ['bob', 'bob', '主号', 'bob@example.com', '直连', '走节点'], ['nope'], ['主号', 'bob', '直连', '走节点']]):
    @case('reorder/%02d' % i, ['reorder'] + names)
    def _(b):
        world(b, cfg={'ports': dict(PORTS)})


@case('reorder/second-time', ['reorder', '走节点'])
def _(b):
    world(b, cfg={'ports': dict(PORTS), 'order_base': ['x-000000']})


@case('reorder/codex-detached-state', ['reorder', '直连接口'], tool='codex')
def _(b):
    world(b, codex={'接口2': {'base_url': 'https://api.example.com/v1'}, '直连接口': {'base_url': 'https://api3.example.com/v1/'}})


for i, k in enumerate(['主号(alice@example.com)', 'bob@example.com', '直连', '没有', '']):
    @case('rm/%02d' % i, ['rm', k])
    def _(b):
        world(b, cfg={'ports': dict(PORTS), 'last': 'bob@example.com'}, policy={'version': 1, 'claude': {'BOB@example.com': 'US-B'}})


# --- 选节点的菜单 ---
@case('nodes/allowed', ['nodes-allowed'])
def _(b):
    world(b)


@case('nodes/node-rows', ['node-rows'])
def _(b):
    world(b)


@case('nodes/node-rows-router', ['node-rows', 'US-A\nHK-A\nXX', '60'])
def _(b):
    world(b, cfg={'nodes': NODES + ['一个名字特别特别长的节点-用来看截断的效果-还要更长一点才行']})


@case('nodes/node-rows-narrow', ['node-rows', '', '10'])
def _(b):
    world(b)


@case('nodes/node-rows-bad-cols', ['node-rows', '', 'abc'])
def _(b):
    world(b, cfg={'nodes': []})


@case('nodes/node-default', ['node-default'])
def _(b):
    world(b)


@case('nodes/node-default-all-used', ['node-default'])
def _(b):
    world(b, cfg={'nodes': ['US-A', 'US-B']})


@case('nodes/preselect', ['nodes-preselect', 'X', 'US-A', 'US-C', 'HK-A', 'ZZ', 'US-D'])
def _(b):
    world(b, policy={'version': 1, 'claude': {'c@example.com': 'US-C', 'd@example.com': 'ZZ'}, 'codex': {'e@example.com': 'US-D'}},
          cfg={'policy_skip': ['d@example.com']})


@case('nodes/preselect-none', ['nodes-preselect'])
def _(b):
    world(b)


@case('nodes/lost', ['nodes-lost', 'US-A'])
def _(b):
    world(b, codex={'erin@example.com': 'US-B', '科(frank@example.com)': 'HK-A'})


@case('nodes/lost-none', ['nodes-lost', 'US-A', 'US-B', 'HK-A'])
def _(b):
    world(b)


@case('nodes/set', ['set-nodes', 'US-A', '', 'HK-A', 'US-A', 'NEW'])
def _(b):
    world(b)


@case('nodes/set-empty', ['set-nodes', '', ''])
def _(b):
    world(b)


# --- prompt-emit ---
PAGE = '''q=add.basic
title=添加账号
message=填一下
submit=下一步
ignored line without equals
orphan=before any field
field=email
prompt=  登录邮箱 [me@example.com]：
kind=text
hint=用来登录的那个
error=
field=alias
label=别名
prompt=不用它
optional=1
default=主号
field=node
kind=choice
opt=US-A\t美国 A\t已被 bob 使用
opt=HK-A
opt=a\tb\tc\td
min=2
field=code
kind=code
code=line 1
code=line 2
label=
detail=x=y=z
unknown=zzz'''


@case('prompt-emit/first', ['prompt-emit'], stdin=PAGE, env={'CCA_EVENTS': '%HOME%/ev.jsonl'})
def _(b):
    b.write('ev.jsonl', '')


@case('prompt-emit/third', ['prompt-emit'], stdin=PAGE + '\n', env={'CCA_EVENTS': '%HOME%/ev.jsonl'})
def _(b):
    b.write('ev.jsonl', '{"v":1,"ev":"ask","seq":1}\n{"v":1,"ev":"fail"}\n{"ev": "ask"}\n{"v":1,"ev":"ask","seq":2}\nno newline "ev":"ask"')


@case('prompt-emit/empty', ['prompt-emit'], stdin='', env={'CCA_EVENTS': '%HOME%/ev.jsonl'})
def _(b):
    b.write('ev.jsonl', '')


@case('prompt-emit/no-events-env', ['prompt-emit'], stdin=PAGE)
def _(b):
    pass


@case('prompt-emit/no-events-file', ['prompt-emit'], stdin=PAGE, env={'CCA_EVENTS': '%HOME%/missing.jsonl'})
def _(b):
    pass


@case('prompt-emit/bad-min', ['prompt-emit'], stdin='field=a\nmin=x\n', env={'CCA_EVENTS': '%HOME%/ev.jsonl'})
def _(b):
    b.write('ev.jsonl', '')
