"""给界面 App 的接口（cli/src/cmd/gui.rs）：health.json、check --json、检查更新、切换工具时的变量。"""
import json

from run import case
from cases.router import ident
from cases.table import world

NOW = [(r'"(checked_at|generated_at|generated_epoch)": [0-9]+\.[0-9]+', r'"\1": <现在>')]

NOTES = [
    ['bob@example.com', 'launch', 'api', 'api_ok', 'router', 'US-B', '7902', '接口连得上'],
    ['bob@example.com', 'check', 'port', 'port_closed', '', '', '', ''],
    ['直连', 'check', 'api', 'api_unreachable', 'direct', '', 'abc'],
    ['走节点', 'gui', 'api', 'api_ok', 'local', 'HK-A', '07903', '带 "引号" 和\t制表符'],
    ['新来的@example.com', 'check', 'exit', 'no_exit', 'router', '节点', '0'],
    ['bob@example.com', 'check', 'exit'],
    ['bob@example.com', 'check', 'exit', 'ok', 'router', 'US-B', '²'],
]
for i, argv in enumerate(NOTES):
    @case('health-note/%02d' % i, ['health-note'] + argv, mask=NOW)
    def _(b):
        world(b)


@case('health-note/codex', ['health-note', 'erin@example.com', 'launch', 'api', 'api_ok', 'router', 'HK-A', '7904', 'ok'], tool='codex', mask=NOW)
def _(b):
    world(b, codex={'erin@example.com': 'HK-A'})


@case('health-note/overwrites', ['health-note', 'bob@example.com', 'check', 'port', 'port_closed', 'router', 'US-B', '7902', '端口不通'], mask=NOW)
def _(b):
    world(b)
    b.json('.cca/accounts/%s/health.json' % ident('bob@example.com'), {'old': True}, mode=0o644)


@case('health-note/unwritable-dir', ['health-note', 'bob@example.com', 'check', 'port', 'port_closed', 'router', 'US-B', '7902'], mask=NOW)
def _(b):
    world(b)
    b.write('.cca/accounts', 'a file where the directory should be')


H1 = {'ip': '198.51.100.7', 'node': 'US-A', 'port': 7901, 'result': 'ok'}
H2 = {'ip': '198.51.100.7', 'node': 'US-B', 'port': 7902, 'result': 'ok'}
H3 = {'ip': '203.0.113.9', 'node': None, 'result': 'low_purity', '中文': '值'}
RENDER = '\n'.join([
    '主号(alice@example.com)\t' + json.dumps(H1), 'bob@example.com\t' + json.dumps(H2), '直连\t' + json.dumps(H3, ensure_ascii=False), '走节点\t',
    '没有制表符', '\t{"ip": "x"}', '坏的\t{oops', '不是对象\t[1, 2]', '空对象\t{}', '回车\t{"ip": "203.0.113.9"}\r', '', '最后一行没有换行\t{"ip": ""}',
])
for name, argv, text in [('plain', [], RENDER), ('route', ['router'], RENDER + '\n'), ('empty', [''], ''), ('too-many', ['a', 'b'], '')]:
    @case('check-render/%s' % name, ['check-render'] + argv, stdin=text, mask=NOW)
    def _(b):
        world(b)


@case('check-render/codex', ['check-render', 'local'], stdin='erin@example.com\t{"ip": "198.51.100.7"}\n', tool='codex', mask=NOW)
def _(b):
    world(b)


@case('onboard-done/first', ['onboard-done'])
def _(b):
    world(b)


@case('onboard-done/again', ['onboard-done'])
def _(b):
    world(b, cfg={'onboarded': True})


@case('onboard-done/no-config', ['onboard-done'])
def _(b):
    pass


for tool in ('claude', 'codex', 'agy'):
    @case('tool-env/%s' % tool, ['tool-env'], tool=tool)
    def _(b):
        pass


def rel(tag, **kw):
    r = {'tag_name': tag, 'body': '第一行\r\n\r\n  第二行  \n第三行\n第四行', 'html_url': 'https://example.com/r/%s' % tag,
         'published_at': '2026-11-01T08:00:00Z', 'draft': False}
    r.update(kw)
    return r


RELS = [rel('v3.4.1'), rel('v3.5.0-beta.2'), rel('v3.5.0-beta.10', body=None, html_url=None), rel('V3.4.0'), rel('v9.9.9', draft=True), rel('nightly'),
        rel('3.4.2', published_at=1795075200123, body=''), 'junk', {'tag_name': None}, {'tag_name': 5}, rel('v3.4.2', published_at='not a time')]
CHECKS = [
    ('stable-sees-stable', ['3.4.0'], RELS), ('stable-json', ['3.4.0', 'json', '', 'https://example.com/repo'], RELS),
    ('beta-sees-beta', ['3.5.0-beta.1', '', '', 'https://example.com/repo'], RELS), ('beta-json', ['3.5.0-beta.1', 'json', '', 'https://example.com/repo'], RELS),
    ('up-to-date', ['3.4.2'], RELS), ('newer-than-all', ['4.0.0', 'json'], RELS), ('dev-version', ['dev', 'json'], RELS), ('beta-on-latest-beta', ['3.5.0-beta.10'], RELS),
    ('fallback', ['3.4.0', 'json', '3.4.5', 'https://example.com/repo'], None), ('fallback-bash', ['3.4.5', '', '3.4.5', 'https://example.com/repo'], None),
    ('fallback-bad', ['3.4.0', 'json', 'v3.4.5', 'https://example.com/repo'], None), ('nothing', ['3.4.0'], None), ('not-a-list', ['3.4.0', 'json'], {'message': 'rate limited'}),
    ('only-betas-for-stable', ['3.4.0', 'json', '3.4.1'], [rel('v3.5.0-beta.1')]), ('first-of-equals', ['3.4.0', 'json'], [rel('v3.5.0', body='先'), rel('V3.5.0', body='后')]),
    ('bad-month', ['3.4.0', 'json'], [rel('v3.5.0', published_at='2026-13-01T00:00:00Z')]),
    ('times', ['3.4.0', 'json'], [rel('v3.5.0', published_at='2026-11-01 08:00:30.5+08:00')]), ('time-number', ['3.4.0', 'json'], [rel('v3.5.0', published_at=1795075200)]),
    ('time-bool', ['3.4.0', 'json'], [rel('v3.5.0', published_at=True)]), ('big-numbers', ['3.4.0'], [rel('v99999999999999999999999.0.0'), rel('v9999999999999999999999.9.9')]),
    ('unicode-digits', ['3.4.0'], [rel('v３.５.０')]), ('notes-one-line', ['3.4.0'], [rel('v3.5.0', body='  只有一行  ')]),
    ('notes-not-string', ['3.4.0', 'json'], [rel('v3.5.0', body=['x'])]), ('trailing-newline-tag', ['3.4.0'], [rel('v3.5.0\n')]),
]
for name, argv, rels in CHECKS:
    @case('update-check/%s' % name, ['update-check'] + argv, stdin='' if rels is None else json.dumps(rels, ensure_ascii=False), mask=NOW)
    def _(b):
        pass


@case('update-check/garbage', ['update-check', '3.4.0', 'json', '3.4.1', 'https://example.com/repo'], stdin='<html>502</html>', mask=NOW)
def _(b):
    pass


@case('update-check/no-args', ['update-check'], stdin='[]')
def _(b):
    pass
