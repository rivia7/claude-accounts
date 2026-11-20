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


# --- health-save：出口检测的结果记进 health.json（解析用 leak 模块的 coffee_parse，夹具借它的）---
from cases.leak import RISK, TRACE, risk   # noqa: E402

SAVES = [
    ('ok', ['50', TRACE, json.dumps(RISK)]),
    ('ok-with-leak-line', ['50', TRACE, json.dumps(RISK), '出口干净', 'DNS 没有泄露', 'WebRTC 没有直连风险', 'ok', 'ok']),
    ('leak-partial', ['50', TRACE, json.dumps(RISK), '横幅那一句', '', '', '', 'risk']),
    ('no-leak-line-ignores-rest', ['50', TRACE, json.dumps(RISK), '', 'dns 说明', 'webrtc 说明', 'ok', 'ok']),
    ('low-purity', ['99', TRACE, json.dumps(risk(trust_score=60))]),
    ('restricted', ['50', TRACE.replace('loc=US', 'loc=CN'), json.dumps(risk(countryCode='cn'))]),
    ('no-exit', ['50', '', '']),
    ('no-score', ['50', TRACE, '{}']),
    ('risk-not-json', ['50', TRACE, '<html>502</html>']),
    ('tags', ['50', TRACE, json.dumps(risk(is_datacenter=True, isResidential=False, is_vpn=True))]),
    ('unicode-org', ['50', TRACE, json.dumps(risk(asOrganization='例子网络 ☁'), ensure_ascii=False)]),
    ('bad-min-trust', ['abc', TRACE, json.dumps(RISK)]),
]
for name, tail in SAVES:
    @case('health-save/%s' % name, ['health-save', 'bob@example.com', 'launch', 'router', 'US-B', '7902'] + tail, mask=NOW)
    def _(b):
        world(b)


@case('health-save/codex-no-route', ['health-save', 'erin@example.com', 'check', '', '', '', '50', TRACE, json.dumps(RISK)], tool='codex', mask=NOW)
def _(b):
    world(b, codex={'erin@example.com': 'HK-A'})


@case('health-save/too-few-args', ['health-save', 'bob@example.com', 'launch', 'router', 'US-B', '7902', '50', TRACE])
def _(b):
    world(b)


# --- gui-live：端口池的实况 --------------------------------------------------------------------------------------------
# nc 是假的：端口 7902 连不上，别的连得上；各端口各记一个文件（几个探测是并行的，记进同一个文件先后不定）
import os      # noqa: E402
import time    # noqa: E402

NC = '''for a in "$@"; do p="$a"; done
printf '%s\\n' "nc $*" > "$HOME/nc-$p.log"
case "$p" in 7902) exit 1 ;; esac
exit 0
'''
STATUS = '''OpenClash：运行中
钩子：已安装
自愈 cron：未安装

槽位 CCA-1 :7901 监听中 → US-A
槽位 CCA-2 :7902 监听中 → HK-A（应为 US-B）
槽位 CCA-3 :7903 未监听 → （内核里没有）
槽位 CCA-4 :7904 监听中 → ?
槽位 CCA-5 :7905 监听中 →
槽位 CCA-6 :7906 监听中 → US-A（应为 US-A）
槽位 CCA-9 :7909 监听中 → 不在端口池里的
槽位 写得不对
别的行
'''
LIVE_TABLE = {'主号(alice@example.com)': 'US-A', 'bob@example.com': 'US-B', '走节点': {'base_url': 'https://api2.example.com/v1/', 'node': 'HK-A'},
              'carol@example.com': 'US-A', 'dave@example.com': 'HK-A', '直连': {'base_url': 'https://api.example.com/anthropic'}}
LOCAL_TEXT = 'V\t200\nG\tCCA-1\t200\tUS-A\nG\tCCA-2\t200\tHK-A\nG\tCCA-3\t404\nG\tCCA-4\t200\t\nG\tCCA-5\t200\nG\nX\ty\nG\tCCA-6\n'


def live(name, argv, stdin=STATUS, cfg=None, table=LIVE_TABLE, **kw):
    kw.setdefault('mask', NOW)

    @case('gui-live/' + name, ['gui-live'] + argv, stdin=stdin, **kw)
    def _(b):
        world(b, table=table, cfg=cfg, codex={'erin@example.com': 'US-B'})
        b.fake('nc', NC)


live('router-home', ['router', '192.0.2.10', '0', 'yes', 'yes'])
live('router-reach-defaults-to-at-home', ['router', '192.0.2.10', '0', 'yes'])
live('router-away-but-reachable', ['router', '192.0.2.10', '0', 'no', 'yes'])
live('router-unreachable', ['router', '192.0.2.10', '0', 'no', 'no'])
live('router-at-home-empty', ['router', '192.0.2.10', '0'])
live('router-ssh-failed', ['router', '192.0.2.10', '255', 'yes', 'yes'], stdin='ssh: connect to host 192.0.2.10 port 22: Operation timed out\n')
live('router-status-failed', ['router', '192.0.2.10', '3', 'yes', 'yes'], stdin='line 1\n最后一行说的是原因\n\n')
live('router-status-failed-no-output', ['router', '192.0.2.10', '1', 'yes', 'yes'], stdin='')
live('router-not-running', ['router', '192.0.2.10', '0', 'yes', 'yes'], stdin='ShellCrash：未运行\n钩子：已安装\n自愈 cron：已安装\n槽位 CCA-1 :7901 未监听 → US-A\n')
live('router-not-configured', ['router', '192.0.2.10', '0', 'yes', 'yes'], cfg={'router': {}})
live('router-no-host-arg', ['router', '', '0', 'yes', 'yes'])
live('router-crlf', ['router', '192.0.2.10', '0', 'yes', 'yes'], stdin=STATUS.replace('\n', '\r\n'))
live('router-empty-status', ['router', '192.0.2.10', '0', 'yes', 'yes'], stdin='')
live('local', ['local', '127.0.0.1', '0', 'no'], stdin=LOCAL_TEXT, cfg={'local': {'client': 'builtin'}})
live('local-not-running', ['local', '127.0.0.1', '0', 'no'], stdin='V\t000\n', cfg={'local': {'client': 'builtin'}})
live('local-not-configured', ['local', '127.0.0.1', '0', 'no'], stdin=LOCAL_TEXT)
live('local-no-version-line', ['local', '127.0.0.1'], stdin='G\tCCA-1\t200\tUS-A\n', cfg={'local': {'client': 'builtin'}})
live('none', ['none', '192.0.2.10', '0', 'no'])
live('none-no-host', ['none'])
live('odd-route', ['wifi', '192.0.2.10', '0', 'yes', 'yes'])
live('route-pref', ['router', '192.0.2.10', '0', 'yes', 'yes'], cfg={'route': 'local'})
live('empty-table', ['router', '192.0.2.10', '0', 'yes', 'yes'], table={})
live('no-args', [])
live('too-many-args', ['router', 'h', '0', 'yes', 'yes', 'x'])
live('ro', ['router', '192.0.2.10', '0', 'yes', 'yes'], env={'CCA_RO': '1'})
live('codex', ['router', '192.0.2.10', '0', 'yes', 'yes'], tool='codex')
live('bad-table', ['router', '192.0.2.10', '0', 'yes', 'yes'], table={'主号(alice@example.com)': 'NOPE'})


@case('gui-live/nc-missing', ['gui-live', 'router', '192.0.2.10', '0', 'yes', 'yes'], stdin=STATUS, mask=NOW, env={'PATH': '%HOME%/nowhere:/usr/bin:/bin'})
def _(b):   # 没有 nc：都算连不上
    world(b, table=LIVE_TABLE)
    os.makedirs(b.path('nowhere'))
    b.write('nowhere/nc', '#!/bin/sh\nexit 127\n', 0o644)   # 不能执行


# --- gui-state：界面的只读快照 ------------------------------------------------------------------------------------------
# bash 是带着 CCA_RO=1 跑它的（不写文件）；这里两种都跑。ps 是假的（cases/session.py 的）。Claude 官方账号要经邮箱助手的 mail_view
STATE_MASK = NOW + [(r'"(generated_at|fetched_epoch)": [0-9.]+', r'"\1": <现在>'), (r'"age_seconds": -?[0-9]+', '"age_seconds": <多久以前>'),
                    (r'"fetched_at": "[^"]*"', '"fetched_at": "<查到的时间>"')]
# 切换以后折合金额新加的诊断（devices[].diag、cost_total.unattributed 的 why 这几项）原来的 Python 没有：去掉了再比（cases/cost.py 的 DIAG_MASK，
# 这里是缩进排版的）
STATE_MASK += [(r',\n\s*"diag": (?:null|\{(?:[^{}]|\{[^{}]*\})*\})', ''),
               (r',\n\s*"why": \[(?:[^\[\]{}]|\{[^{}]*\})*\],\n\s*"claude_code": (?:null|"[^"]*"),\n\s*"claude_code_old": (?:true|false)', '')]
# 能不能换自定义代理（can.custom_proxy，cca node use）也是切换以后新加的：同样去掉再比（规则 node_free 在 cargo test、回归 11b2c 里测）
STATE_MASK += [(r',\n\s*"custom_proxy": (?:true|false)', '')]
# 本机 UDP 走节点给别的 VPN 让开了（config.local.udp.other_vpn）也是切换以后新加的（cargo test、tests/local_udp.py、回归 11b4u 里测）
STATE_MASK += [(r',\n\s*"other_vpn": (?:null|"[^"]*")', '')]
# 本机 UDP 走节点在家暂停着没有（config.local.udp.paused）也是切换以后新加的：去掉再比（回归 11b4u 测）
STATE_MASK += [(r',\s*"paused": (?:true|false)', '')]
# 维护者的共享开没开、映射的账号有几个、收起来几个（config.policy 的 state、accounts、parked）也是切换以后新加的（回归 10g、11b2 里测）
STATE_MASK += [(r',\n\s*"state": "(?:off|pending|on)",\n\s*"accounts": [0-9]+,\n\s*"parked": [0-9]+', '')]
# 折合金额的设备菜单（cost_total.devices、账号里各台设备的 used）也是：去掉再比，和折合金额的用例同一份规则
from cases.cost import DEVICES_MASK   # noqa: E402
STATE_MASK += DEVICES_MASK
GUI_ENV = {'CCA_RO': '1', 'CCA_VERSION': '9.9.9', 'CCA_GUI_CLAUDE_BIN': '/usr/local/bin/claude', 'CCA_GUI_CLAUDE_APP': '/Applications/Claude.app',
           'CCA_GUI_CODEX_BIN': '/usr/local/bin/codex', 'CCA_GUI_CODEX_APP': '', 'CCA_GUI_AGY_BIN': '', 'CCA_GUI_CHROME': '1', 'CCA_COST': 'off'}
ROUTER_CODE = 'router code\nline 2\n'


def state(name, setup, env=None, stdin=ROUTER_CODE, tool='claude', **kw):
    kw.setdefault('mask', STATE_MASK)
    e = dict(GUI_ENV)
    e.update(env or {})
    e = {k: v for k, v in e.items() if v is not None}

    @case('gui-state/' + name, ['gui-state'], stdin=stdin, env=e, tool=tool, **kw)
    def _(b):
        from cases.session import PS
        b.fake('ps', PS)
        setup(b)


def no_claude_official(b, **cfg):
    """Claude 这边只有第三方 API 账号，Codex 有官方账号和 API 账号，agy 两个账号"""
    from cases import usage as U
    now = time.time()
    world(b, table={'kimi': {'base_url': 'https://api.example.com/anthropic', 'model': 'm-1'}, '走节点': {'base_url': 'https://api2.example.com/v1/', 'node': 'HK-A', 'id': 'zjd'}},
          codex={'工作(erin@example.com)': 'US-A', 'frank@example.com': 'HK-A', 'kimi': {'base_url': 'https://api.example.com/v1'}},
          agy={'gina@example.com': 'US-B', '副(henry@example.com)': 'US-A'}, cfg=cfg or None)
    U.codex_login(b, '工作(erin@example.com)')
    U.cache(b, '工作(erin@example.com)', U.rl(5, 9, now), now - 30, tool='codex')
    U.agy_login(b, 'gina@example.com')


state('no-claude-official', no_claude_official)
state('no-claude-official-not-ro', no_claude_official, env={'CCA_RO': None})
state('no-claude-official-nothing-installed', no_claude_official, env={'CCA_GUI_CLAUDE_BIN': '', 'CCA_GUI_CLAUDE_APP': '', 'CCA_GUI_CODEX_BIN': '', 'CCA_GUI_CHROME': '0', 'CCA_VERSION': None})
state('no-claude-official-codex-tool', no_claude_official, tool='codex')
state('extra-arg', no_claude_official, stdin='')


def fresh(b):
    pass


state('nothing-at-all', fresh)                                    # 全新的机器：没有 ~/.cca
state('nothing-at-all-not-ro', fresh, env={'CCA_RO': None})


def only_config(b):
    b.cfg({'version': 2, 'router': {'host': '192.0.2.10', 'port': 2222, 'user': 'root'}, 'proxy_host': '192.0.2.11', 'nodes': ['US-A', 'US-B'], 'min_trust': 70,
           'leak_check': False, 'update': 'manual', 'route': 'router', 'policy': 'git@example.com:me/policy.git', 'onboarded': True, 'synced': 'x'})
    p = b.write('.cca/policy/checked', '')
    os.utime(p, (1794000000.5, 1794000000.5))


state('only-config', only_config)


def unreadable(b):
    no_claude_official(b)
    b.write('.cca/config.json', '{oops')
    b.write('.cca/codex.json', '[1, 2]')
    b.write('.cca/agy.json', '{oops')


state('unreadable-files', unreadable)


def table_errors(b):
    world(b, table={'kimi': {'base_url': 'not a url'}}, codex={'erin@example.com': 'NOPE', 'frank@example.com': 'US-A'})


state('table-errors', table_errors)


def synced(b):
    """路由器同步过（摘要对得上）、内置代理配过"""
    import hashlib
    no_claude_official(b, local={'client': 'builtin', 'applied': 'something else'})


state('local-configured', synced)


def codex_rich(b):
    """Codex 官方账号：登录着、用量缓存、每周用量、出口检测的结果、命令行会话、桌面 App 开着、映射里的账号（设备统计、折合金额）"""
    from cases import usage as U
    from cases.cost import ROW, U1, hour
    from cases.secrets import PRESENCE, PUB
    from cases.session import T0_EPOCH
    now = time.time()
    world(b, table={'kimi': {'base_url': 'https://api.example.com/anthropic'}},
          codex={'工作(erin@example.com)': 'US-A', 'frank@example.com': 'HK-A', 'kimi': {'base_url': 'https://api.example.com/v1', 'model': 'gpt-x'}},
          cfg={'policy': 'git@example.com:me/policy.git', 'codex': {'last': 'frank@example.com', 'ref_keys': [ident('erin@example.com')],
                                                                    'key_hints': {ident('erin@example.com'): {'host': 'reference', 'hint': 'sk-proj…QRST', 'at': 1794000000.5, 'from': '书房的 Mac'}}}},
          policy={'version': 1, 'codex': {'Erin@example.com': 'US-A'}, 'presence': PRESENCE})
    b.write('.cca/policy/id_ed25519.pub', PUB, 0o644)
    b.write('.cca/policy/id_ed25519', 'PRIVATE KEY PLACEHOLDER\n')
    os.utime(b.write('.cca/policy/checked', ''), (1794000000.5, 1794000000.5))
    erin, frank = '.cca/codex/accounts/' + ident('erin@example.com'), '.cca/codex/accounts/' + ident('frank@example.com')
    U.codex_login(b, '工作(erin@example.com)', plan='pro', user='user-AbCdEfGhIjKlMnOp')
    U.cache(b, '工作(erin@example.com)', U.rl(5, 9, now), now - 30, tool='codex')
    wk = 7 * 86400
    end = int(now // 3600 * 3600) + 3 * 86400
    b.write(erin + '/usage-weeks.json', json.dumps({'version': 1, 'weeks': [
        {'end': end - 2 * wk, 'peak': 88, 'first': end - 3 * wk + 60, 'last': end - 2 * wk - 60, 'carried': 0, 'resets': 0},
        {'end': end - wk, 'peak': 40.5, 'first': end - 2 * wk + 60, 'last': end - wk - 60, 'carried': 12, 'resets': 1},
        {'end': end, 'peak': 9, 'first': end - wk + 60, 'last': int(now) - 30}]}, indent=2) + '\n')
    b.json(erin + '/health.json', {'checked_at': now - 120, 'source': 'launch', 'result': 'ok', 'route': 'router', 'node': 'US-A', 'port': 7902, 'ip': '198.51.100.7'})
    b.json(frank + '/health.json', {'checked_at': now - 9 * 86400, 'source': 'check', 'result': 'low_purity', 'node': 'US-B'})
    b.write(erin + '/cli-4242.json', json.dumps({'v': 1, 'tool': 'codex', 'id': ident('erin@example.com'), 'pid': 4242, 'started': T0_EPOCH, 'since': 1794024000.5}))
    b.write(erin + '/cli-77.json', json.dumps({'v': 1, 'tool': 'codex', 'id': ident('erin@example.com'), 'pid': 77, 'started': T0_EPOCH, 'since': 1794024000.5}))
    b.write('ps-lstart.txt', ' 4242 Sat Nov  7 12:08:55 2026\n')
    os.makedirs(b.path(erin + '/desktop'))
    b.write('ps-command.txt', '    1 /sbin/launchd\n 5151 /Applications/ChatGPT.app/Contents/MacOS/ChatGPT --user-data-dir=%s --x\n 5152 /usr/bin/other\n' % b.path(erin + '/desktop'))
    # 折合金额：这台设备统计的、别的设备同步来的
    cost = {'v': 1, 'build': '2026-11-13/2', 'who': {'codex:user-AbCdEfGhIjKlMnOp': 'erin@example.com', 'claude:' + U1: 'alice@example.com'},
            'acc': {'codex:user-AbCdEfGhIjKlMnOp': {hour(now - 2 * 3600): ROW, hour(now - 26 * 3600): [2500000, 4, 10, 20, 30, 40, 1], hour(now - 9 * 86400): ROW,
                                                    hour(now - 40 * 86400): [9000000, 9, 1, 1, 1, 1, 0], hour(now - 100 * 86400): ROW},
                    'claude:' + U1: {hour(now - 3 * 3600): ROW}},
            'um': {'codex:user-AbCdEfGhIjKlMnOp': ['gpt-unknown-9']}, 'orphan': {hour(now - 5 * 3600): [700000, 2, 1, 1, 1, 1, 0], hour(now - 60 * 86400): ROW, 'bad': [1]}}
    b.write('.cca/cost/local.json', json.dumps(cost))
    slot = {'SHA256:bob': {'h': {hour(now - 30 * 3600): [2000000, 1, 1, 1, 1, 1, 0]}, 'um': ['gpt-other'], 'at': now - 600, 'name': 'bob-mini', 'revoked': False, 'tag': 't1'},
            'SHA256:bob-old': {'h': {hour(now - 20 * 86400): ROW, hour(now - 28 * 3600): [9000000, 1, 1, 1, 1, 1, 0]}, 'um': [], 'at': now - 30 * 86400, 'name': 'bob-mini', 'revoked': True, 'tag': 't2'},
            'SHA256:carol': {'h': {hour(now - 50 * 3600): ROW}, 'um': [], 'at': now - 2 * 86400, 'name': 'carol-air', 'revoked': False, 'tag': 't3'},
            'SHA256:junk': {'h': {'x': [1]}, 'at': now, 'name': 'junk', 'tag': 't4'}, 'SHA256:quiet': {'h': {}, 'um': [], 'at': now - 60, 'name': 'no-usage', 'tag': 't5'}}
    b.write('.cca/cost/remote.json', json.dumps({'v': 1, 'acc': {'codex:erin@example.com': slot}}))
    b.write('.cca/cost/sync.json', json.dumps({'v': 1, 'state': 'ok', 'at': 1794000100.5, 'ok_at': 1794000100.5, 'me': 'erin-mbp', 'acc': {'codex:erin@example.com': {
        'checked': 1794000100.5, 'missing': [{'name': 'dan-air', 'used_epoch': int(now) - 3600}, {'name': 'old-air', 'used_epoch': int(now) - 60 * 86400}, 'junk']}}}))
    os.utime(b.write('.cca/cost/scanned', ''), (1794000200.25, 1794000200.25))
    os.chmod(b.path('.cca/cost'), 0o700)
    b.write('.cca/presence/keys.json', json.dumps({'v': 1, 'acc': {}, 'error': '读不了这台设备的钥匙串'}))
    os.chmod(b.path('.cca/presence'), 0o700)


state('codex-rich', codex_rich)
state('codex-rich-not-ro', codex_rich, env={'CCA_RO': None})
state('codex-rich-la-time', codex_rich, env={'TZ': 'America/Los_Angeles'})          # 夏令时结束的那一天在最近 30 天里时：那天 25 小时
state('codex-rich-kathmandu', codex_rich, env={'TZ': 'Asia/Kathmandu'})              # 一天从整点过 15 分开始：跨过边界的那个小时按中点归哪一天
state('codex-rich-st-johns', codex_rich, env={'TZ': 'America/St_Johns'})


def me_without_records(b):
    """这台设备没有这个账号的记录，别的设备有（tests/cost_view.py 的 test_me_without_records）：设备里没有这台，cost_total 的 me 是空的"""
    codex_rich(b)
    local = json.load(open(b.path('.cca/cost/local.json')))
    del local['acc']['codex:user-AbCdEfGhIjKlMnOp']
    b.write('.cca/cost/local.json', json.dumps(local))


state('codex-rich-me-without-records', me_without_records)


def nobody_has_records(b):
    codex_rich(b)
    b.write('.cca/cost/local.json', json.dumps({'v': 1, 'acc': {}, 'who': {}, 'um': {}, 'orphan': {}}))
    b.write('.cca/cost/remote.json', json.dumps({'v': 1, 'acc': {}}))


state('codex-rich-no-records-at-all', nobody_has_records)
state('codex-rich-cost-off', lambda b: (codex_rich(b), b.cfg(dict(json.load(open(b.path('.cca/config.json'))), cost=False))))
state('codex-rich-presence-off', lambda b: (codex_rich(b), b.cfg(dict(json.load(open(b.path('.cca/config.json'))), presence=False))))
state('codex-rich-no-deploy-key', lambda b: (codex_rich(b), os.unlink(b.path('.cca/policy/id_ed25519'))))


for name, sync in [('error', {'state': 'error', 'error': '连不上', 'at': 1794000300.0}), ('error-older-than-checked', {'state': 'error', 'error': '连不上', 'at': 1794000000.0}),
                   ('old-server', {'state': 'old_server'}), ('never-checked', {'state': 'ok', 'acc': {}}), ('error-not-str', {'state': 'error', 'error': 5, 'at': 1794000300.0}),
                   ('not-v1', {'v': 2}), ('me-not-str', {'me': 5})]:
    def sync_state(b, sync=sync):
        codex_rich(b)
        base = json.load(open(b.path('.cca/cost/sync.json')))
        base.update(sync)
        b.write('.cca/cost/sync.json', json.dumps(base))
    state('codex-rich-sync-' + name, sync_state)


def scanning(b):
    codex_rich(b)
    os.unlink(b.path('.cca/cost/scanned'))
    from cases.session import T0_EPOCH
    b.write('.cca/cost/worker.json', json.dumps({'v': 1, 'pid': 4242, 'started': T0_EPOCH}))


state('codex-rich-scanning', scanning)
state('codex-rich-never-scanned', lambda b: (codex_rich(b), os.unlink(b.path('.cca/cost/scanned'))))


# --- gui-state：有 Claude 官方账号（账号页的「邮箱」一节：mail_view） ----------------------------------------------------------
def claude_official(b, mail=Ellipsis, **cfg):
    """Claude 有两个官方账号（一个登录着、有用量缓存、有上次的出口检测）、一个第三方 API 账号；Codex 一个官方账号"""
    from cases import usage as U
    now = time.time()
    c = dict(cfg)
    if mail is not Ellipsis:
        c['mail'] = mail
    world(b, table={'alice@example.com': 'US-A', '主号(bob@example.com)': 'HK-A', 'kimi': {'base_url': 'https://api.example.com/anthropic', 'model': 'm-1'}},
          codex={'erin@example.com': 'US-A'}, cfg=c or None)
    U.claude_login(b, 'alice@example.com')
    U.cache(b, 'alice@example.com', U.rl(21, 47, now), now - 45)
    b.json(U.acct('alice@example.com') + '/health.json', {'checked_at': now - 300, 'source': 'launch', 'result': 'ok', 'route': 'router', 'node': 'US-A', 'port': 7901, 'ip': '198.51.100.9'})
    U.claude_login(b, '主号(bob@example.com)', organizationType='claude_pro', organizationRateLimitTier='default_claude_ai')


ALICE_ID, BOB_ID = ident('alice@example.com'), ident('bob@example.com')
state('claude-official', claude_official)                                        # 没配置邮箱助手
state('claude-official-not-ro', claude_official, env={'CCA_RO': None})
state('claude-official-codex-tool', claude_official, tool='codex')
for name, mail in [
    ('mail-default', {'user': 'Inbox@Example.com'}),                                                       # 默认的 Gmail，两个账号都用它
    ('mail-default-auto-off-proxy', {'user': 'inbox@example.com', 'auto': False, 'proxy': 'http://127.0.0.1:8080'}),
    ('mail-auto-not-bool', {'user': 'inbox@example.com', 'auto': 0, 'proxy': ''}),                          # auto 只有 false 才算关；proxy 空的按默认
    ('mail-override', {'user': 'inbox@example.com', 'auto': False, 'accounts': {BOB_ID: {'user': 'Bob-Inbox@example.com'}}}),   # 账号单独配的：没写的继承默认那一项
    ('mail-override-own-auto', {'user': 'inbox@example.com', 'auto': False, 'accounts': {BOB_ID: {'user': 'bob-inbox@example.com', 'auto': True, 'proxy': 'direct'}}}),
    ('mail-override-only', {'accounts': {BOB_ID: {'user': 'bob-inbox@example.com'}}}),                      # 没有默认，只有 bob 单独配了
    ('mail-override-bad-address', {'user': 'inbox@example.com', 'accounts': {BOB_ID: {'user': 'not an address'}}}),   # 覆盖的地址不对：用默认那个
    ('mail-forwarder', {'user': 'inbox@example.com', 'accounts': {ALICE_ID: {'forwarder': 'Relay.Example.com'}, BOB_ID: {'user': 'bob-inbox@example.com', 'forwarder': 'relay2.example.com'}}}),
    ('mail-forwarder-bad', {'user': 'inbox@example.com', 'accounts': {ALICE_ID: {'forwarder': 'not a domain'}, BOB_ID: {'forwarder': 5}}}),
    ('mail-forwarder-without-mailbox', {'accounts': {ALICE_ID: {'forwarder': 'relay.example.com'}}}),       # 登记了中转但没有可连的 Gmail：没配置
    ('mail-bad-address', {'user': 'not an address'}),
    ('mail-user-not-str', {'user': 5}),
    ('mail-not-dict', 'oops'),
    ('mail-empty', {}),
    ('mail-accounts-not-dict', {'user': 'inbox@example.com', 'accounts': ['x']}),
    ('mail-override-not-dict', {'user': 'inbox@example.com', 'accounts': {BOB_ID: 'x', ALICE_ID: None}}),
    ('mail-keyed-by-something-else', {'user': 'inbox@example.com', 'accounts': {'bob@example.com': {'user': 'bob-inbox@example.com'}, '主号(bob@example.com)': {'user': 'x@example.com'}}}),
]:
    state('claude-official-' + name, lambda b, mail=mail: claude_official(b, mail=mail))
