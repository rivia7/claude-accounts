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
    ('gmail', ' Me.X+1@Example.com '), ('gmail', 'me@example'), ('gmail', 'mé@example.com'),
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


# --- 账号表的表单（edit-form / edit-apply）---
def edit_sum(table):
    """账号表的键和先后的摘要：照 cca 的 edit_sum 现算"""
    import hashlib
    import json
    return hashlib.sha1(json.dumps(list(table), ensure_ascii=False).encode('utf-8')).hexdigest()[:12]


def answers(**kw):
    return ''.join('%s\t%s\n' % (k.replace('_', '!'), v) for k, v in kw.items())


KEYED = {'主号(alice@example.com)': 'US-A', 'bob@example.com': 'US-B',
         '直连': {'base_url': 'https://api.example.com/anthropic', 'model': 'm-1', 'key': 'sk-test-0000000000000000'},
         '走节点': {'base_url': 'https://api2.example.com/v1/', 'node': 'HK-A', 'env': {'X': '1', 'Y': 'a b'}, 'id': 'zjd'}}
FORMS = [
    ('plain', '', {}),
    ('prev-and-errors', 'n0\tHK-A\nn1\tZZ\nu2\thttps://other.example.com/v9\nm2\t\nn2\tUS-B\ne2\tA=1\nm3\tbig\nn3\t\ne3\t\n!m2\t模型名里不能有空格或引号\n!\t整页的问题\n'
                        '!!!e3\t名字不对\nno tab here\n!n9\t没有这个字段\n\n', {}),
    ('empty-page-error', '!\t\n', {}),
    ('codex', '', {'tool': 'codex', 'codex': {'erin@example.com': 'HK-A', '接口': {'base_url': 'https://api.example.com/v1', 'model': 'gpt-x', 'node': 'US-A'}}}),
    ('codex-prev', 'e1\tA=1\nm1\tnew-model\n', {'tool': 'codex', 'codex': {'erin@example.com': 'HK-A', '接口': {'base_url': 'https://api.example.com/v1'}}}),
    ('agy', '', {'tool': 'agy', 'agy': {'gina@example.com': 'US-B', '吉(hana@example.com)': 'US-B'}}),
    ('officials-only', '', {'table': {'主号(alice@example.com)': 'US-A', 'bob@example.com': 'US-A', 'carol@example.com': 'US-A'}}),
    ('node-not-allowed', '', {'table': {'bob@example.com': 'ZZ', '接口': {'base_url': 'https://api.example.com/v1', 'node': 'YY'}}}),
    ('pinned', 'n0\tHK-A\n', {'policy': {'version': 1, 'claude': {'Alice@example.com': 'US-B', 'bob@example.com': 'ZZ-none'}}}),
    ('no-nodes-officials', '', {'cfg': {'nodes': []}}),
    ('no-nodes-apis-only', '', {'cfg': {'nodes': []}, 'table': {'直连': {'base_url': 'https://api.example.com/v1', 'env': {'A': 1, 'B': True, 'C': None, 'D': "it's"}}}}),
    ('empty-table', '', {'table': {}}),
    ('odd-values', '', {'table': {'直连': {'base_url': 'https://api.example.com/v1/', 'model': 5, 'env': 'junk'}, '另一个': {'base_url': 'https://api.example.com/v1', 'model': ''}}}),
    ('unfixable-key', '', {'table': {'a b': 'US-A', 'bob@example.com': 'US-B'}}),
    ('unfixable-fields', '', {'table': {'直连': {'base_url': 'https://api.example.com/v1', 'extra': 1, 'key': 5, 'id': 'a b'}}}),
    ('unfixable-long', '', {'table': dict(('坏 %02d' % i, 'US-A') for i in range(40))}),
]
for name, text, kw in FORMS:
    kw = dict(kw)
    tool = kw.pop('tool', 'claude')

    @case('edit-form/%s' % name, ['edit-form'], stdin=text, tool=tool)
    def _(b, kw=kw):
        world(b, **kw)


@case('edit-form/no-table', ['edit-form'])
def _(b):
    b.cfg({'nodes': ['US-A']})


@case('edit-form/table-not-json', ['edit-form'])
def _(b):
    world(b)
    b.write('.cca/accounts.json', '{oops')


@case('edit-form/table-is-a-list', ['edit-form'], tool='codex')
def _(b):
    world(b)
    b.write('.cca/codex.json', '["US-A"]')


SAME_ANSWERS = answers(n0='US-A', n1='US-B', u2='https://api.example.com/anthropic', m2='m-1', n2='', e2='', u3='https://api2.example.com/v1',
                       m3='', n3='HK-A', e3="X=1 Y='a b'")
APPLIES = [
    ('wrong-digest', 'n0\tHK-A\n', {}, 'bad'),
    ('no-answers', '', {}, None),
    ('same', SAME_ANSWERS, {}, None),
    ('same-url-normalized', SAME_ANSWERS.replace('https://api2.example.com/v1', '  https://api2.example.com/v1///  '), {}, None),
    ('official-node', SAME_ANSWERS.replace('n0\tUS-A', 'n0\tHK-A'), {}, None),
    ('official-node-not-allowed', SAME_ANSWERS.replace('n1\tUS-B', 'n1\tZZ').replace('n3\tHK-A', 'n3\t YY '), {}, None),
    ('url-same-host-keeps-key', SAME_ANSWERS.replace('https://api.example.com/anthropic', 'https://API.example.com/v2/'), {}, None),
    ('url-new-host-drops-key', SAME_ANSWERS.replace('https://api.example.com/anthropic', 'https://other.example.com/anthropic'), {}, None),
    ('url-bad', SAME_ANSWERS.replace('https://api.example.com/anthropic', 'http://api.example.com').replace('https://api2.example.com/v1', 'ftp://x'), {}, None),
    ('model-set-and-clear', SAME_ANSWERS.replace('m2\tm-1', 'm2\t').replace('m3\t', 'm3\t  big-2  '), {}, None),
    ('model-bad', SAME_ANSWERS.replace('m2\tm-1', 'm2\ta b'), {}, None),
    ('direct-goes-via-node', SAME_ANSWERS.replace('n2\t', 'n2\tUS-B'), {'cfg': {'ports': {'主号(alice@example.com)': 7901, 'bob@example.com': 7902, '走节点': 7903}}}, None),
    ('node-cleared', SAME_ANSWERS.replace('n3\tHK-A', 'n3\t'), {}, None),
    ('env-changed', SAME_ANSWERS.replace('e2\t', 'e2\tFOO=bar BAZ="q u x" EMPTY=').replace("e3\tX=1 Y='a b'", 'e3\t'), {}, None),
    ('env-requoted-same', SAME_ANSWERS.replace("e3\tX=1 Y='a b'", 'e3\tX="1"   Y=a\\ b'), {}, None),
    ('env-bad-quote', SAME_ANSWERS.replace('e2\t', "e2\tA='oops"), {}, None),
    ('env-no-equals', SAME_ANSWERS.replace('e2\t', 'e2\tA=1 ' + 'x' * 60), {}, None),
    ('env-reserved', SAME_ANSWERS.replace('e2\t', 'e2\tANTHROPIC_API_KEY=x 1BAD=y MY_TOKEN=z HOME=/x'), {}, None),
    ('everything-wrong', answers(n0='ZZ', n1='', u2='', m2='a"b', n2='YY', e2='=x', u3='nope', m3="a'b", n3='ZZ', e3='A'), {}, None),
    ('duplicate-answers', SAME_ANSWERS + 'n0\tHK-A\nn0\tUS-B\n', {}, None),
    ('pinned-ignores-answer', SAME_ANSWERS.replace('n0\tUS-A', 'n0\tHK-A').replace('n1\tUS-B', 'n1\tHK-A'), {'policy': {'version': 1, 'claude': {'ALICE@example.com': 'US-A'}}}, None),
    ('missing-official-answers', answers(u2='https://api.example.com/anthropic', m2='m-1', u3='https://api2.example.com/v1'), {}, None),
]
for name, text, kw, digest in APPLIES:
    @case('edit-apply/%s' % name, ['edit-apply', digest or edit_sum(KEYED)], stdin=text)
    def _(b, kw=kw):
        world(b, table=KEYED, **kw)


CODEX_KEYED = {'erin@example.com': 'HK-A', '接口': {'base_url': 'https://api.example.com/v1', 'model': 'gpt-x', 'node': 'US-A', 'key': 'sk-test-1111111111111111'}}
for name, text in [('same', answers(n0='HK-A', u1='https://api.example.com/v1', m1='gpt-x', n1='US-A')),
                   ('env-ignored', answers(n0='HK-A', u1='https://api.example.com/v1', m1='gpt-x', n1='US-A', e1='A=1')),
                   ('changed', answers(n0='US-B', u1='https://api.example.org/v1', m1='', n1=''))]:
    @case('edit-apply/codex-%s' % name, ['edit-apply', edit_sum(CODEX_KEYED)], stdin=text, tool='codex')
    def _(b):
        world(b, codex=CODEX_KEYED)


@case('edit-apply/agy', ['edit-apply', edit_sum({'gina@example.com': 'US-B'})], stdin='n0\tHK-A\n', tool='agy')
def _(b):
    world(b, agy={'gina@example.com': 'US-B'})


@case('edit-apply/no-digest', ['edit-apply'], stdin='')
def _(b):
    world(b)


# --- sync-state：校验 + 清掉没主的 key + 摘要（要动钥匙串：沙箱里放 cases/secrets.py 那个假的 security）---
from cases.router import ident as acct_ident   # noqa: E402
from cases.secrets import SECURITY             # noqa: E402

CODE = '#!/bin/sh\n# 路由器端的脚本（假的）\necho ok\n'


def synced(b, cfg=None, **kw):
    world(b, cfg=dict({'ports': dict(PORTS), 'synced': 'abc123'}, **(cfg or {})), **kw)
    b.fake('security', SECURITY)


SYNCS = [
    ('plain', [], {}), ('plain-quiet', ['1'], {}), ('never-synced', [], {'cfg': {'synced': ''}}), ('synced-not-a-string', [], {'cfg': {'synced': 0}}),
    ('table-errors', [], {'table': {'a b': 'US-A', 'bob@example.com': 'ZZ'}}), ('table-errors-quiet', ['quiet'], {'table': {'a b': 'US-A', 'bob@example.com': 'ZZ'}}),
    ('notice-then-ok', [], {'policy': {'version': 1, 'claude': {'carol@example.com': 'US-A'}}}),
    ('notice-then-ok-quiet', ['q'], {'policy': {'version': 1, 'claude': {'carol@example.com': 'US-A'}}}),
    ('orphan-keys', [], {'cfg': {'api_keys': ['gone-aaaaaa', acct_ident('直连')], 'ref_keys': ['left-bbbbbb'],
                                 'key_hints': {'gone-aaaaaa': {'host': 'api.example.com', 'hint': ''}, 'left-bbbbbb': {'host': 'reference', 'hint': 'sk-…WXYZ'}}}}),
    ('orphan-keys-quiet', ['1'], {'cfg': {'api_keys': ['gone-aaaaaa'], 'ref_keys': ['left-bbbbbb']}}),
]
for name, argv, kw in SYNCS:
    @case('sync-state/%s' % name, ['sync-state'] + argv, stdin=CODE)
    def _(b, kw=kw):
        synced(b, **kw)
        b.write('keychain/cca-api-gone-aaaaaa/api.example.com', 'sk-test-AAAA1111')
        b.write('keychain/cca-ref-left-bbbbbb/reference', 'sk-ant-api03-REFtest0123456789abcdWXYZ')


for name, argv in [('table-not-json', []), ('table-not-json-quiet', ['1'])]:
    @case('sync-state/%s' % name, ['sync-state'] + argv, stdin=CODE)
    def _(b):
        synced(b)
        b.write('.cca/accounts.json', '{"a": ')


for name, argv in [('config-not-an-object', []), ('config-not-an-object-quiet', ['1'])]:
    @case('sync-state/%s' % name, ['sync-state'] + argv, stdin=CODE)
    def _(b):
        synced(b)
        b.write('.cca/config.json', '[1, 2]')


@case('sync-state/codex', ['sync-state', ''], stdin=CODE, tool='codex')
def _(b):
    synced(b, codex=CODEX_TABLE, cfg={'codex': {'ports': {}, 'last': '', 'api_keys': ['gone-cccccc']}})
    b.write('keychain/cca-codex-api-gone-cccccc/api.example.com', 'sk-test-AAAA1111')


@case('sync-state/empty-stdin', ['sync-state'], stdin='')
def _(b):
    synced(b)


@case('sync-state/too-many-args', ['sync-state', '1', '2'], stdin=CODE)
def _(b):
    synced(b)


# --- rows（菜单里每个账号一行）、set-last（记下这次启动的账号，顺带说套餐过时的那一句）---------------------------------------
# 登录状态、用量缓存的夹具用 cases/usage.py 的。用量那一格里的“还有多久重置”是照现在算的：夹具离整分钟差 30 秒，两边先后跑不会跨过去
def menu_world(b, last=None, **cfg):
    import time as _t
    from cases import usage as U
    now = _t.time()
    table = dict(U.RICH_TABLE, **{'走节点': {'base_url': 'https://api2.example.com/v1/', 'node': 'HK-A'}})
    c = dict(cfg)
    if last:
        c['last'] = last
    world(b, table=table, cfg=c or None)
    U.claude_login(b, U.RICH[0], organizationRateLimitTier='default_claude_max_20x')     # 登录着、有刚查的用量
    U.cache(b, U.RICH[0], U.rl(13, 41, now), now - 90)
    U.claude_login(b, U.RICH[1])                                                         # 登录着、用量是很久以前的
    U.cache(b, U.RICH[1], U.rl(98, 72, now - 9 * 86400), now - 9 * 86400)
    U.claude_login(b, U.RICH[2], emailAddress='someone-else@example.com')                # 登录的不是这个账号
    U.cache(b, U.RICH[2], U.rl(7, 33, now), now - 60)
    # RICH[3]：没登录


for name, argv in [('default', ['rows']), ('cols-200', ['rows', '200']), ('cols-80', ['rows', '80']), ('cols-60', ['rows', '60']), ('cols-46', ['rows', '46']),
                   ('cols-40', ['rows', '40']), ('cols-10', ['rows', '10']), ('cols-word', ['rows', 'wide']), ('cols-empty', ['rows', '']),
                   ('cols-fullwidth', ['rows', '１２０']), ('cols-negative', ['rows', '-80']), ('too-many-args', ['rows', '80', 'x'])]:
    @case('rows/' + name, argv)
    def _(b):
        menu_world(b, last='alice@example.com')


for name, last in [('last-api', 'kimi'), ('last-with-alias', '工作(bob@example.com)'), ('last-unknown', 'ghost@example.com'), ('last-not-str', 5), ('no-last', None)]:
    @case('rows/' + name, ['rows', '120'])
    def _(b, last=last):
        menu_world(b, last=last)


@case('rows/ro', ['rows', '120'], env={'CCA_RO': '1'})
def _(b):
    menu_world(b, last='alice@example.com')


@case('rows/empty-table', ['rows'])
def _(b):
    world(b, table={})


@case('rows/no-table', ['rows'])
def _(b):
    b.cfg({'version': 2, 'nodes': ['US-A']})


@case('rows/only-api', ['rows'])
def _(b):
    world(b, table={'kimi': {'base_url': 'https://api.example.com/anthropic'}, '走节点': {'base_url': 'https://api2.example.com/v1/', 'node': 'HK-A'}})


@case('rows/no-aliases', ['rows', '100'])   # 都没有别名：别名那一列不留
def _(b):
    from cases import usage as U
    world(b, table={'alice@example.com': 'US-A', 'bob@example.com': 'US-B'})
    U.claude_login(b, 'alice@example.com')


@case('rows/long-names', ['rows', '100'])   # 别名最多占 16 格、邮箱 32 格，超出的截掉
def _(b):
    from cases import usage as U
    world(b, table={'一个很长很长的别名一个很长很长的别名(a-very-long-mailbox-name-indeed@mail.example.com)': 'US-A', '短(bob@example.com)': 'US-B'})
    U.claude_login(b, '一个很长很长的别名一个很长很长的别名(a-very-long-mailbox-name-indeed@mail.example.com)')


@case('rows/bad-table', ['rows'])
def _(b):
    world(b, table={'主号(alice@example.com)': 'NOPE'})


@case('rows/codex', ['rows', '120'], tool='codex')
def _(b):
    import time as _t
    from cases import usage as U
    world(b, codex={'erin@example.com': 'US-A', '科(frank@example.com)': 'HK-A', 'kimi': {'base_url': 'https://api.example.com/v1'}}, cfg={'codex': {'last': '科(frank@example.com)'}})
    U.codex_login(b, 'erin@example.com')
    U.cache(b, 'erin@example.com', U.rl(5, 9, _t.time()), _t.time() - 30, tool='codex')


@case('rows/agy', ['rows', '120'], tool='agy')
def _(b):
    from cases import usage as U
    world(b, agy={'gina@example.com': 'US-B', 'henry@example.com': 'US-A'})
    U.agy_login(b, 'gina@example.com')


for name, argv, tool in [('claude', ['set-last', 'alice@example.com'], 'claude'), ('with-alias', ['set-last', '工作(bob@example.com)'], 'claude'),
                         ('unknown-key', ['set-last', 'ghost@example.com'], 'claude'), ('empty-key', ['set-last', ''], 'claude'), ('api', ['set-last', 'kimi'], 'claude'),
                         ('codex', ['set-last', 'erin@example.com'], 'codex'), ('agy', ['set-last', 'gina@example.com'], 'agy'),
                         ('no-args', ['set-last'], 'claude'), ('two-args', ['set-last', 'a', 'b'], 'claude')]:
    @case('set-last/' + name, argv, tool=tool)
    def _(b):
        menu_world(b, last='dave@example.com')
        b.table({'erin@example.com': 'US-A'}, 'codex')
        b.table({'gina@example.com': 'US-B'}, 'agy')


for name, sub, kw in [('outdated-pro', 'pro', {}), ('outdated-max-vs-pro', 'max', {'organizationType': 'claude_pro', 'organizationRateLimitTier': None}), ('same', 'max', {}),
                      ('other-account', 'pro', {'accountUuid': 'uuid-someone-else'}), ('not-logged-in', 'pro', None)]:
    @case('set-last/plan-' + name, ['set-last', 'alice@example.com'])   # Claude Code 登录时记的套餐过时了：顺带说一句
    def _(b, sub=sub, kw=kw):
        import time as _t
        from cases import usage as U
        world(b, table={'alice@example.com': 'US-A'})
        if kw is not None:
            U.claude_login(b, 'alice@example.com', **kw)
        U.cache(b, 'alice@example.com', U.rl(), _t.time() - 3600 - 30, uuid='uuid-alice', login_plan=sub)


@case('set-last/ro', ['set-last', 'alice@example.com'], env={'CCA_RO': '1'})
def _(b):
    menu_world(b)


@case('set-last/config-unreadable', ['set-last', 'alice@example.com'])
def _(b):
    menu_world(b)
    b.write('.cca/config.json', '{oops')
