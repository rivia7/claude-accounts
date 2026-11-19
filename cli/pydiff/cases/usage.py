"""用量、套餐、重置额度（cli/src/cmd/usage.rs）：usage-query（Claude 的那一支）、usage-render、login-plan-forget、reset-plan、
reset-done。Codex 的 app-server 那一半（usage-query 的 Codex 一支、reset-consume、login-flow、登录身份）在 cases/codex.py。

claude 用沙箱里假的（CLAUDE）：收到的参数、工作目录、stdin 上的请求都记进文件，照沙箱里放好的文件回答（claude.before / reply /
err / exit / hang / kill / close / rc）。夹具里的登录信息、用量都是编的，没有任何 token。

「现在」每次不一样：夹具里的重置时间、缓存时间都在 setup 里照当时的时间现算（别在登记用例的时候算：整套跑下来要几分钟），
离分钟的边界留 30 秒（两边先后跑差不了这么多）。输出里真的每次不一样的只有这几样，用 mask 换掉：generated_at /
generated_epoch（GEN），这次查到的那几行的 fetched_at / fetched_epoch、存下来的 fetchedAtMs、每周用量里的 first / last（LIVE），
缓存那几行的 age_seconds（AGE）。"""
import base64
import json
import os
import time

from run import case
from cases.router import ident
from cases.table import world

ALICE, BOB = '主号(alice@example.com)', 'bob@example.com'
OUT = '%HOME%/out'
YEAR_10000 = 253402300800 + 86400 * 400      # 公元一万年以后的一个时刻（秒）：Python 的 date 装不下
OFF = {'CCA_COST': 'off'}      # 折合金额的后台统计另有专门的用例，别处关掉（省得每个用例都起一次后台进程）
GEN = [(r'"generated_at": "[^"]*"', '"generated_at": "<现在>"'), (r'"generated_epoch": [0-9.e+]+', '"generated_epoch": <现在>')]
LIVE = [(r'"fetched_at": "[^"]*"', '"fetched_at": "<查到的时间>"'), (r'"fetched_epoch": [0-9.e+]+', '"fetched_epoch": <查到的时间>'),
        (r'"fetchedAtMs": [0-9]+', '"fetchedAtMs": <查到的时间>'), (r'"(first|last)": [0-9.e+]+', r'"\1": <查到的时间>')]
AGE = [(r'"age_seconds": -?[0-9]+', '"age_seconds": <多久以前>')]
ALL = GEN + LIVE + AGE

CLAUDE = r'''# 假的 claude（对拍用，见 cases/usage.py 开头）
T="$HOME"
printf 'claude %s\n' "$*" >> "$T/calls.log"
printf 'cwd=%s CLAUDE_CONFIG_DIR=%s fd3=%s\n' "$(pwd -P)" "${CLAUDE_CONFIG_DIR-unset}" "$( { true <&3; } 2>/dev/null && echo open || echo closed)" >> "$T/calls.log"
[ -f "$T/claude.err" ] && cat "$T/claude.err" >&2
[ -f "$T/claude.exit" ] && exit "$(cat "$T/claude.exit")"
[ -f "$T/claude.before" ] && cat "$T/claude.before"
IFS= read -r line && printf '%s\n' "$line" >> "$T/claude.stdin"
[ -f "$T/claude.hang" ] && exec sleep "$(cat "$T/claude.hang")"
[ -f "$T/claude.reply" ] && cat "$T/claude.reply"
[ -f "$T/claude.kill" ] && kill -"$(cat "$T/claude.kill")" $$
[ -f "$T/claude.close" ] && { exec 1>&-; exec sleep "$(cat "$T/claude.close")"; }
cat > /dev/null
[ -f "$T/claude.rc" ] && exit "$(cat "$T/claude.rc")"
exit 0
'''


def iso(t):
    return time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime(t))


def email_of(key):
    return key.split('(')[-1].rstrip(')')


def acct(key, tool='claude'):
    """账号目录（相对沙箱的 HOME）"""
    return '%s/accounts/%s' % ({'claude': '.cca', 'codex': '.cca/codex', 'agy': '.cca/agy'}[tool], ident(email_of(key)))


def claude_login(b, key, cached=None, raw=None, **oa):
    """账号目录里的 .claude.json：登录的是谁、什么套餐；cached 是 Claude Code 自己存的用量（cachedUsageUtilization）"""
    who = {'emailAddress': email_of(key), 'accountUuid': 'uuid-' + email_of(key).split('@')[0], 'organizationType': 'claude_max',
           'organizationRateLimitTier': 'default_claude_max_5x'}
    who.update(oa)
    j = {'userID': 'not-a-real-id', 'oauthAccount': {k: v for k, v in who.items() if v is not None}}
    if cached is not None:
        j['cachedUsageUtilization'] = cached
    return b.write(acct(key) + '/claude/.claude.json', raw if raw is not None else json.dumps(j, ensure_ascii=False))


def b64(d):
    return base64.urlsafe_b64encode(json.dumps(d).encode()).decode().rstrip('=')


def codex_login(b, key, plan='plus', account='ws-1', user='user-12345678', **claims):
    """账号目录里的 auth.json：id_token 的声明是编的，另外两个 token 写成一看就是假的"""
    payload = {'email': email_of(key), 'https://api.openai.com/auth': {'chatgpt_plan_type': plan, 'chatgpt_account_id': account, 'chatgpt_user_id': user}}
    payload.update(claims)
    auth = {'auth_mode': 'chatgpt', 'OPENAI_API_KEY': None,
            'tokens': {'id_token': '%s.%s.sig' % (b64({'alg': 'none'}), b64(payload)), 'access_token': 'test-token-not-real', 'refresh_token': 'test-token-not-real'}}
    return b.write(acct(key, 'codex') + '/codex/auth.json', json.dumps(auth))


def agy_login(b, key, tier='g1-pro-tier', **more):
    j = {'id_token_email': email_of(key), 'user_tier': tier}
    j.update(more)
    return b.write(acct(key, 'agy') + '/gemini/antigravity-cli/antigravity-oauth-token', json.dumps(j))


def rl(five=13, week=41, now=None, **extra):
    """一份用量：5 小时的窗口 2 小时 13 分半后重置，本周的 3 天 5 小时后。extra 里值是 ... 的项去掉"""
    now = now or time.time()
    d = {'five_hour': {'utilization': five, 'resets_at': iso(now + 2 * 3600 + 13 * 60 + 30)},
         'seven_day': {'utilization': week, 'resets_at': iso(now + 3 * 86400 + 5 * 3600)}}
    d.update(extra)
    return {k: v for k, v in d.items() if v is not Ellipsis}


def usage(rate_limits, sub='max', available=True, **more):
    """后台查到了：usage-query 打印的那一行"""
    u = {'subscription_type': sub, 'rate_limits_available': available, 'rate_limits': rate_limits}
    u.update(more)
    return {'ok': True, 'usage': u}


def result(b, i, r):
    """后台查询的结果文件：usage-query 打印的那一行"""
    return b.write('out/%d.json' % i, r if isinstance(r, bytes) else (r if isinstance(r, str) else json.dumps(r, ensure_ascii=False)) + '\n')


def raw_number(r, text):
    """结果里写着 '@@' 的地方换成 NaN / Infinity 这种 json.dumps 写不出来、Python 的 json 却认的数"""
    return json.dumps(r, ensure_ascii=False).replace('"@@"', text)


def skip(b, i, why, msg=''):
    return b.write('out/%d.skip' % i, '%s\t%s\n' % (why, msg))


def cache(b, key, rate_limits, at, tool='claude', uuid=Ellipsis, **more):
    """cca 上次存的 usage.json"""
    if uuid is Ellipsis:
        uuid = 'uuid-' + email_of(key).split('@')[0] if tool == 'claude' else '%s|ws-1' % email_of(key)
    j = {'fetchedAtMs': int(at * 1000), 'accountUuid': uuid, 'utilization': rate_limits}
    j.update(more)
    return b.write(acct(key, tool) + '/usage.json', json.dumps(j, ensure_ascii=False, indent=2) + '\n')


def render(name, keys=(BOB,), mode='table', cols='120', color='', mask=(), tool='claude', env=None, **kw):
    """登记一个 usage-render 的用例"""
    e = dict(OFF)
    e.update(env or {})
    return case('usage-render/' + name, ['usage-render', OUT, mode, cols, color] + list(keys), mask=list(mask), tool=tool, env=e, **kw)


def both(name, keys=(BOB,), mask=(), **kw):
    """表格和 JSON 各登记一个，夹具一样"""
    def reg(setup):
        render(name + '-table', keys, 'table', mask=mask, **kw)(setup)
        render(name + '-json', keys, 'json', mask=list(mask) + GEN, **kw)(setup)
        return setup
    return reg


# ================================================================================================================
# usage-query（Claude）
# ================================================================================================================
USAGE = {'session': {}, 'subscription_type': 'max', 'rate_limits_available': True, 'behaviors': None,
         'rate_limits': {'five_hour': {'utilization': 13.0, 'resets_at': '2026-11-19T10:00:00.123456+00:00'}, 'seven_day': {'utilization': 41, 'resets_at': None},
                         'limits': [{'kind': 'weekly_scoped', 'percent': 1e2, 'scope': {'model': {'display_name': '模型 α'}}}], 'big': 12345678901234567890123, 'tiny': 1e-07}}
INIT = json.dumps({'type': 'system', 'subtype': 'init'}) + '\n'


def answer(resp, **more):
    m = {'type': 'control_response', 'response': resp}
    m.update(more)
    return json.dumps(m, ensure_ascii=False) + '\n'


def ok_reply(response=USAGE, request_id='cca-usage'):
    return answer({'subtype': 'success', 'request_id': request_id, 'response': response})


def err_reply(error, **more):
    r = {'subtype': 'error', 'request_id': 'cca-usage', 'error': error}
    r.update(more)
    return answer({k: v for k, v in r.items() if v is not Ellipsis})


def claude(b, home=True, **files):
    """放好假的 claude 和它要照着回答的文件"""
    if home:
        world(b)
    b.fake('claude', CLAUDE)
    for name, text in files.items():
        b.write('claude.' + name, text)


QUERY = [
    ('ok', {'before': INIT, 'reply': ok_reply()}, {}),
    ('ok-no-init-line', {'reply': ok_reply()}, {}),
    ('ok-reply-without-newline', {'reply': ok_reply().rstrip('\n')}, {}),
    ('ok-crlf', {'reply': ok_reply().replace('\n', '\r\n')}, {}),
    ('ok-exits-nonzero-afterwards', {'reply': ok_reply(), 'rc': '7'}, {}),
    ('usage-not-an-object', {'reply': ok_reply([1, 2])}, {}),
    ('usage-missing', {'reply': answer({'subtype': 'success', 'request_id': 'cca-usage'})}, {}),
    ('usage-empty', {'reply': ok_reply({})}, {}),
    ('unsupported', {'before': INIT, 'reply': err_reply('Unsupported control request subtype: get_usage')}, {}),
    ('error', {'reply': err_reply('boom ' * 60)}, {}),
    ('error-unicode-cut-at-200', {'reply': err_reply('错' * 250)}, {}),
    ('error-is-an-object', {'reply': err_reply({'code': 5, 'why': 'unsupported'})}, {}),
    ('error-is-a-number', {'reply': err_reply(0)}, {}),
    ('error-missing', {'reply': err_reply(Ellipsis)}, {}),
    ('subtype-missing', {'reply': answer({'request_id': 'cca-usage', 'response': USAGE})}, {}),
    ('skips-other-lines', {'before': INIT + 'not json\n[1, 2]\n"text"\n\n' + answer('not an object') + answer({'subtype': 'success', 'request_id': 'other', 'response': {}})
                           + answer({'subtype': 'success', 'response': {}}) + json.dumps({'type': 'other', 'response': {'subtype': 'success', 'request_id': 'cca-usage'}}) + '\n'
                           + json.dumps({'response': {'subtype': 'success', 'request_id': 'cca-usage'}}) + '\n' + '{"type": "control_response", "response": {"request_id": "cca-usage"\n',
                           'reply': ok_reply()}, {}),
    ('line-with-bom', {'before': b'\xef\xbb\xbf' + ok_reply({'bom': True}).encode(), 'reply': ok_reply()}, {}),
    ('line-with-two-boms', {'before': b'\xef\xbb\xbf\xef\xbb\xbf' + ok_reply({'bom': 2}).encode(), 'reply': ok_reply()}, {}),
    ('line-not-utf8', {'before': b'{"type": "control_response", "response": {"request_id": "cca-usage", "subtype": "success", "response": {"x": "\xff"}}}\n', 'reply': ok_reply()}, {}),
    # UTF-16-LE 的换行是 0A 00：按 0A 断行以后这一行少半个字符，剩下的 00 又把下一行（真正的回答）带歪了，只能等到超时
    ('line-utf16-le-cut-by-the-newline', {'before': ok_reply({'utf16': 'le'}).encode('utf-16-le'), 'reply': ok_reply()}, {'timeout': '1'}),
    ('line-utf16-be', {'before': ok_reply({'utf16': 'be', '字': '值'}).encode('utf-16-be'), 'reply': ok_reply()}, {}),
    ('line-utf16-be-with-bom', {'before': b'\xfe\xff' + ok_reply({'utf16': 'bom'}).encode('utf-16-be'), 'reply': ok_reply()}, {}),
    ('line-utf32-be', {'before': ok_reply({'utf32': 'be'}).encode('utf-32-be'), 'reply': ok_reply()}, {}),
    ('line-utf32-be-with-bom', {'before': b'\x00\x00\xfe\xff' + ok_reply({'utf32': 'bom'}).encode('utf-32-be'), 'reply': ok_reply()}, {}),
    ('line-starts-with-nul', {'before': b'\x00\x00{}\n', 'reply': ok_reply()}, {}),
    ('old-version', {'err': "error: unknown option '--safe-mode'\n(Did you mean --model?)\n", 'exit': '1'}, {}),
    ('old-version-unsupported-word', {'err': 'Error: Unsupported flag\nusage: claude [options]\n', 'exit': '2'}, {}),
    ('exits-with-other-error', {'err': 'fatal: something else\n\n  second line  \n\n', 'exit': '1'}, {}),
    ('exits-quietly', {'exit': '3'}, {}),
    ('exits-quietly-zero', {'exit': '0'}, {}),
    ('exits-blank-stderr', {'err': ' \n\n', 'exit': '4'}, {}),
    ('killed-by-signal', {'kill': '9'}, {}),
    ('killed-by-sigterm-after-garbage', {'before': 'garbage\n', 'kill': '15'}, {}),
    ('stderr-long-line', {'err': 'x' * 300 + 'unknown option' + 'y' * 700 + '\n', 'exit': '1'}, {}),
    ('stderr-tail-cuts-a-character', {'err': '头' * 100 + '\n' + '中' * 266 + 'ab\n', 'exit': '1'}, {}),
    ('stderr-over-800-bytes', {'err': 'unknown option far away\n' + 'line\n' * 400 + 'last line\n', 'exit': '1'}, {}),
    ('stderr-not-utf8', {'err': b'bad \xff\xfe bytes\nand \xe4\xb8 half\n', 'exit': '1'}, {}),
    ('stderr-other-line-breaks', {'err': 'one\x0btwo\x1cthree\x85four five unsupported\rsix\n', 'exit': '1'}, {}),
    ('timeout', {'hang': '5'}, {'timeout': '0.5'}),
    # 只等 0.01 秒就杀掉：假 claude 来不来得及把参数记下来说不准，这两例只比输出和退出码
    ('timeout-negative', {'hang': '5'}, {'timeout': '-3', 'tree': False}),
    ('timeout-zero', {'hang': '5'}, {'timeout': '0', 'tree': False}),
    ('timeout-after-garbage', {'before': INIT + 'noise\n', 'hang': '5'}, {'timeout': '0.5'}),
    ('timeout-with-stderr', {'err': 'unknown option but still running\n', 'hang': '5'}, {'timeout': '0.5'}),
    ('timeout-spelled-oddly', {'reply': ok_reply()}, {'timeout': ' 2_5.0 '}),
    ('default-timeout', {'reply': ok_reply()}, {'timeout': None}),
]
for name, files, opt in QUERY:
    @case('usage-query/claude-%s' % name, ['usage-query', 'claude'] + [t for t in [opt.get('timeout', '25')] if t is not None], tree=opt.get('tree', True))
    def _(b, files=files):
        claude(b, **files)


@case('usage-query/claude-closes-stdout-but-lingers', ['usage-query', 'claude', '20'], timeout=30)
def _(b):
    claude(b, close='4')       # 关了 stdout、4 秒后才退出：等它 3 秒没等到（退出码 None），收尾时再等到


@case('usage-query/claude-closes-stdout-with-stderr', ['usage-query', 'claude', '20'], timeout=30)
def _(b):
    claude(b, err='nsupported something\n', close='1')


for name, t in [('bad-timeout', 'soon'), ('empty-timeout', ''), ('nan-timeout', 'nan'), ('inf-timeout', 'inf'), ('huge-timeout', '1e300')]:
    @case('usage-query/claude-%s' % name, ['usage-query', 'claude', t], settle=0.6)
    def _(b):
        claude(b, hang='0.3')      # 假 claude 收到请求后等一会儿就退出：这几例里 cca 在等回答之前就出错了


@case('usage-query/claude-config-dir', ['usage-query', 'claude'], env={'CLAUDE_CONFIG_DIR': '%HOME%/' + acct(BOB) + '/claude'})
def _(b):
    claude(b, reply=ok_reply())
    os.makedirs(b.path(acct(BOB) + '/claude'))


@case('usage-query/claude-config-dir-missing', ['usage-query', 'claude'], env={'CLAUDE_CONFIG_DIR': '%HOME%/' + acct(BOB) + '/claude'})
def _(b):
    claude(b, reply=ok_reply())


@case('usage-query/claude-config-dir-is-a-file', ['usage-query', 'claude'], env={'CLAUDE_CONFIG_DIR': '%HOME%/somefile'})
def _(b):
    claude(b, reply=ok_reply())
    b.write('somefile', 'x')


@case('usage-query/claude-config-dir-relative', ['usage-query', 'claude'], env={'CLAUDE_CONFIG_DIR': 'reldir'})
def _(b):
    claude(b, reply=ok_reply())
    os.makedirs(b.path('reldir'))      # 相对路径的目录：dirname 是空的，子进程换不进去


@case('usage-query/claude-config-dir-empty', ['usage-query', 'claude'], env={'CLAUDE_CONFIG_DIR': ''})
def _(b):
    claude(b, reply=ok_reply())


@case('usage-query/claude-no-cca-home', ['usage-query', 'claude'])
def _(b):
    claude(b, home=False, reply=ok_reply())      # ~/.cca 还没有：子进程的工作目录不存在


@case('usage-query/claude-cca-home-elsewhere', ['usage-query', 'claude'], env={'CCA_HOME': '%HOME%/elsewhere'})
def _(b):
    claude(b, home=False, reply=ok_reply())
    os.makedirs(b.path('elsewhere'))


@case('usage-query/claude-fd3-not-passed-on', ['usage-query', 'claude'], fd3='answer\n')
def _(b):
    claude(b, reply=ok_reply())


# 接子进程 stderr 的临时文件：TMPDIR 指的地方不在、是个文件时照 tempfile 的规矩往下找（/tmp……），说的话照样读得回来
for name in ('missing', 'is-a-file', 'empty'):
    @case('usage-query/claude-tmpdir-%s' % name, ['usage-query', 'claude'], env={'TMPDIR': {'missing': '%HOME%/nowhere', 'is-a-file': '%HOME%/somefile', 'empty': ''}[name]})
    def _(b):
        claude(b, err='warning: something\nlast words\n', exit='1')
        b.write('somefile', 'x')


for name, bin_ in [('by-path', '%HOME%/tools/my claude'), ('missing-path', '%HOME%/nowhere/claude'), ('missing-name', 'no-such-claude'), ('empty-name', ''),
                   ('not-executable', '%HOME%/tools/plain'), ('is-a-directory', '%HOME%/tools'), ('relative-path', 'tools/my claude')]:
    @case('usage-query/claude-bin-%s' % name, ['usage-query', bin_])
    def _(b):
        world(b)
        b.write('tools/my claude', '#!/bin/sh\n' + CLAUDE, 0o755)
        b.write('tools/plain', '#!/bin/sh\nexit 0\n', 0o644)
        b.write('claude.reply', ok_reply())


@case('usage-query/claude-bin-not-on-path', ['usage-query', 'claude'], env={'PATH': '%HOME%/nowhere'})
def _(b):
    claude(b, reply=ok_reply())


@case('usage-query/claude-bin-empty-name-odd-path', ['usage-query', ''], env={'PATH': '%HOME%/nowhere::%HOME%/somefile'})
def _(b):
    world(b)
    b.write('somefile', 'x')


@case('usage-query/agy', ['usage-query', 'agy', '5'], tool='agy')
def _(b):
    world(b, agy={'gina@example.com': 'US-A'})


@case('usage-query/no-args', ['usage-query'])
def _(b):
    world(b)


@case('usage-query/too-many-args', ['usage-query', 'claude', '5', 'x'])
def _(b):
    claude(b, reply=ok_reply())


# ================================================================================================================
# usage-render：一整张表（和 tests/usage_view.py 的那四行一样），各种宽度
# ================================================================================================================
RICH_TABLE = {'工作(bob@example.com)': 'US-A', 'alice@example.com': 'US-B', '副号(carol@example.com)': 'HK-A', 'dave@example.com': 'US-A',
              'kimi': {'base_url': 'https://api.example.com/anthropic'}}
RICH = ['工作(bob@example.com)', 'alice@example.com', '副号(carol@example.com)', 'dave@example.com']
FAR = '2099-01-01T00:00:00Z'


def rich(b):
    """四个官方账号：查到了的（Ultra，备注两项）、查到了但 Claude Code 记的套餐过时的、端口不通用缓存顶上的、没登录的；外加一个第三方 API 账号"""
    now = time.time()
    world(b, table=RICH_TABLE)
    claude_login(b, RICH[0], organizationRateLimitTier='default_claude_max_20x')
    claude_login(b, RICH[1])
    claude_login(b, RICH[2], organizationType='claude_pro', organizationRateLimitTier=None)
    weekly = {'seven_day_sonnet': {'utilization': 60, 'resets_at': FAR}, 'model_scoped': [{'display_name': 'Fable', 'utilization': 34, 'resets_at': FAR}]}
    result(b, 0, usage(rl(13, 41, now, **weekly)))
    result(b, 1, usage(rl(98, 72, now), 'pro'))
    skip(b, 2, 'port', '7902')
    cache(b, RICH[2], rl(7, 33, now), now - 23 * 60 - 30)
    skip(b, 3, 'login')


for cols in list(range(36, 172)) + [200, 400]:
    render('rich-cols-%d' % cols, RICH, cols=str(cols), mask=LIVE)(rich)
for cols in (40, 60, 80, 100, 120, 160):
    render('rich-color-cols-%d' % cols, RICH, cols=str(cols), color='1', mask=LIVE)(rich)
for name, cols in [('empty', ''), ('word', 'wide'), ('zero', '0'), ('negative', '-5'), ('fullwidth-digits', '１２０'), ('superscript', '²'), ('huge', '9' * 30), ('spaces', ' 80 '), ('float', '80.0')]:
    render('rich-cols-%s' % name, RICH, cols=cols, mask=LIVE)(rich)
for name, color in [('zero', '0'), ('true', 'true'), ('one-space', '1 ')]:
    render('rich-color-%s' % name, RICH, color=color, mask=LIVE)(rich)
render('rich-json', RICH, 'json', mask=ALL)(rich)
render('rich-json-mode-spelled-differently', RICH, 'JSON', mask=LIVE)(rich)
render('rich-subset-no-api-line', RICH[:2], mask=LIVE)(rich)
render('rich-subset-json', RICH[1:3], 'json', mask=ALL)(rich)
# 同一个账号列了两次：第二次用的是第一次刚存下的缓存（两次都查到的话，每周用量记不记第二次看两次存盘落没落在同一毫秒里，对不出来）
render('rich-same-key-twice', [RICH[1], RICH[2], RICH[2], RICH[3]], mask=LIVE)(rich)
render('rich-same-key-twice-json', [RICH[1], RICH[2], RICH[2], RICH[3]], 'json', mask=ALL)(rich)
render('rich-with-api-key-in-the-list', RICH + ['kimi'], mask=LIVE)(rich)
render('rich-with-unknown-key', RICH + ['ghost@example.com'], mask=LIVE)(rich)
render('rich-ro', RICH, mask=LIVE, env={'CCA_RO': '1'})(rich)
render('rich-json-ro', RICH, 'json', mask=ALL, env={'CCA_RO': '1'})(rich)


@render('rich-order-reversed', RICH[::-1], mask=LIVE)
def _(b):
    rich(b)
    names = ['0.json', '1.json', '2.skip', '3.skip']
    for n in names:
        os.rename(b.path('out/' + n), b.path('out/x' + n))
    for i, n in enumerate(names):
        os.rename(b.path('out/x' + n), b.path('out/%d.%s' % (3 - i, n.split('.')[1])))


@render('no-keys-table', ())
def _(b):
    world(b)


@render('no-keys-json', (), 'json', mask=GEN)
def _(b):
    world(b)


@render('no-accounts-at-all-json', (), 'json', mask=GEN)
def _(b):
    world(b, table={})


@case('usage-render/too-few-args', ['usage-render', OUT, 'table', '80'])
def _(b):
    world(b)


@render('bad-table', mode='json')
def _(b):
    world(b)
    b.write('.cca/accounts.json', '{"a": ')


# --- 查不了的各种原因：有缓存、没缓存，表格、JSON ---
WHYS = [('login', ''), ('mismatch', 'other@example.com'), ('route', ''), ('port', '7902'), ('trace', ''), ('region', '出口在受限地区 香港（HK）'), ('unsupported', ''),
        ('timeout', ''), ('nolimits', ''), ('auth', ''), ('error', 'something broke: 详情'), ('error', ''), ('weird-new-code', 'x'), ('', '')]
for why, msg in WHYS:
    for cached in (False, True):
        @both('why-%s%s%s' % (why or 'empty', '-msg' if msg else '', '-cached' if cached else ''), mask=AGE if cached else ())
        def _(b, why=why, msg=msg, cached=cached):
            world(b)
            claude_login(b, BOB)
            skip(b, 0, why, msg)
            if cached:
                cache(b, BOB, rl(7, 33), time.time() - 3 * 3600 - 30 * 60)

    if why in ('unsupported', 'timeout', 'error', 'auth', ''):      # usage-query 自己会写出来的那几种：从 N.json 里读到的
        @both('why-%s%s-as-json-result' % (why or 'empty', '-msg' if msg else ''))
        def _(b, why=why, msg=msg):
            world(b)
            claude_login(b, BOB)
            result(b, 0, {'ok': False, 'why': why, 'msg': msg})

# --- 结果文件的各种样子（和时间有关的在 setup 里现做）---
RESULTS = [
    ('missing', None, None),
    ('json-last-line-counts', None, lambda: 'garbage first\n' + json.dumps(usage(rl(1, 2))) + '\n' + json.dumps({'ok': False, 'why': 'timeout', 'msg': ''}) + '\n\n \n'),
    ('json-last-line-is-the-usage', None, lambda: json.dumps({'ok': False, 'why': 'timeout', 'msg': ''}) + '\n' + json.dumps(usage(rl(1, 2))) + '\n\n'),
    ('json-last-line-garbage', None, json.dumps({'ok': False, 'why': 'timeout', 'msg': ''}) + '\nnot json\n'),
    ('json-not-an-object', None, '[1, 2]\n'),
    ('json-empty', None, ''),
    ('json-blank', None, ' \n\n'),
    ('json-bom', None, '﻿' + json.dumps({'ok': False, 'why': 'timeout'}) + '\n'),
    ('json-not-utf8', None, b'{"ok": false, "why": "timeout", "msg": "\xff"}\n'),
    ('json-crlf', None, json.dumps({'ok': False, 'why': 'port', 'msg': '7902'}) + '\r\n'),
    ('json-other-line-breaks', None, 'x\x0b' + json.dumps({'ok': False, 'why': 'port', 'msg': '7902'}) + '\x1c'),
    ('json-no-why', None, json.dumps({'ok': False}) + '\n'),
    ('json-ok-zero', None, lambda: json.dumps({'ok': 0, 'why': 'auth', 'usage': usage(rl())['usage']}) + '\n'),
    ('json-ok-string', None, lambda: json.dumps({'ok': 'yes', 'usage': usage(rl())['usage']}) + '\n'),
    ('json-why-number', None, json.dumps({'ok': False, 'why': 5, 'msg': 'm'}) + '\n'),
    ('json-why-list', None, json.dumps({'ok': False, 'why': ['port'], 'msg': 'm'}) + '\n'),
    ('json-why-true', None, json.dumps({'ok': False, 'why': True}) + '\n'),
    ('json-msg-number', None, json.dumps({'ok': False, 'why': 'port', 'msg': 7902}) + '\n'),
    ('json-msg-list', None, json.dumps({'ok': False, 'why': 'error', 'msg': ['a', 1, None]}) + '\n'),
    ('json-msg-object', None, json.dumps({'ok': False, 'why': 'region', 'msg': {'k': 'v'}}) + '\n'),
    ('json-msg-with-braces', None, json.dumps({'ok': False, 'why': 'error', 'msg': '{name} and {cmd} stay as they are {'}) + '\n'),
    ('skip-no-tab', 'login\n', None),
    ('skip-no-newline', 'port\t7902', None),
    ('skip-empty', '', None),
    ('skip-just-newlines', '\n\n', None),
    ('skip-more-tabs', 'port\t79\t02\n\n\n', None),
    ('skip-crlf', 'port\t7902\r\n', None),
    ('skip-two-lines', 'port\t7902\nsecond\tline\n', None),
    ('skip-not-utf8', b'port\t\xff\n', None),
    ('skip-wins-over-json', 'timeout\t\n', lambda: json.dumps(usage(rl())) + '\n'),
    ('skip-bom', '﻿port\t7902\n', None),
    ('skip-trailing-spaces', 'port \t 7902 \n', None),
]
for name, skipped, js in RESULTS:
    @both('result-%s' % name, mask=LIVE)
    def _(b, skipped=skipped, js=js):
        world(b)
        claude_login(b, BOB)
        if skipped is not None:
            b.write('out/0.skip', skipped)
        if js is not None:
            b.write('out/0.json', js() if callable(js) else js)


@both('result-skip-is-a-directory', mask=LIVE)
def _(b):
    world(b)
    claude_login(b, BOB)
    os.makedirs(b.path('out/0.skip'))
    result(b, 0, usage(rl()))


@both('result-json-is-a-directory')
def _(b):
    world(b)
    claude_login(b, BOB)
    os.makedirs(b.path('out/0.json'))


@both('result-dir-missing')
def _(b):
    world(b)
    claude_login(b, BOB)

# --- 查到了，但回应的样子各种各样 ---
SHAPES = [
    ('not-available', lambda: usage(rl(), available=False)),
    ('available-zero', lambda: usage(rl(), available=0)),
    ('available-string', lambda: usage(rl(), available='yes')),
    ('usage-not-an-object', lambda: {'ok': True, 'usage': [1]}),
    ('usage-missing', lambda: {'ok': True}),
    ('rate-limits-null', lambda: usage(None)),
    ('rate-limits-list', lambda: usage([rl()])),
    ('rate-limits-empty', lambda: usage({})),
    ('only-opus-window', lambda: usage({'seven_day_opus': {'utilization': 5, 'resets_at': FAR}})),
    ('only-oauth-apps-window', lambda: usage({'seven_day_oauth_apps': {'utilization': 0, 'resets_at': None}})),
    ('only-unknown-window', lambda: usage({'thirty_day': {'utilization': 5, 'resets_at': FAR}})),
    ('only-five-hour', lambda: usage(rl(seven_day=Ellipsis))),
    ('only-seven-day', lambda: usage(rl(five_hour=None))),
    ('windows-not-objects', lambda: usage({'five_hour': 13, 'seven_day': [41]})),
    ('no-subscription-type', lambda: usage(rl(), None)),
    ('subscription-type-number', lambda: usage(rl(), 5)),
    ('subscription-type-list', lambda: usage(rl(), ['max'])),
    ('subscription-type-empty', lambda: usage(rl(), '')),
    ('extra-keys', lambda: usage(rl(), more={'a': 1})),
]
for name, make in SHAPES:
    @both('shape-%s' % name, mask=LIVE)
    def _(b, make=make):
        world(b)
        claude_login(b, BOB)
        result(b, 0, make())


def window_cases(now):
    """一个窗口的百分比、重置时间的各种写法：[(名字, 5 小时的窗口, 本周的窗口)]。名字不随时间变，内容照 now 算"""
    soon, later = now + 2 * 3600 + 13 * 60 + 30, now + 3 * 86400 + 5 * 3600
    tz = time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(later + 8 * 3600))
    pcts = [13.4, 13.5, 14.5, 0.5, 1.5, 69.5, 70, 89.4, 89.5, 90, 99.5, 100, 250, -3, -0.4, 0, 0.0, 1e30, 12345678901234567890123, True, '13', None, [13], {'v': 1}]
    out = [('pct-%d' % i, {'utilization': p, 'resets_at': iso(soon)}, {'utilization': p, 'resets_at': iso(later)}) for i, p in enumerate(pcts)]
    resets = [('epoch-int', int(soon), int(later)), ('epoch-float', int(soon) + 0.25, int(later) + 0.75), ('epoch-ms', int(soon) * 1000, int(later) * 1000),
              ('fraction-and-offset', iso(soon)[:-1] + '.123456+00:00', tz + '.5+08:00'), ('offset-no-colon', iso(soon)[:-1] + '-0000', tz + '+0800'),
              ('space-no-seconds', iso(soon)[:16].replace('T', ' '), iso(later)[:16].replace('T', ' ') + ' Z'), ('no-zone', iso(soon)[:-1], iso(later)[:-1]),
              ('padded', '  ' + iso(soon) + '\n', '\t' + iso(later) + '  '), ('garbage', 'soon', 'next week'), ('date-only', iso(soon)[:10], iso(later)[:10]),
              ('null', None, None), ('bool', True, False), ('list', [soon], {'at': later}), ('zero', 0, 0), ('zero-float', 0.0, -0.0), ('past', iso(now - 60), iso(now - 86400)),
              ('past-epoch', int(now) - 1, 1), ('negative', -5, -1e9), ('just-under-an-hour', iso(soon), iso(now + 3599 - 30)), ('just-over-an-hour', iso(soon), iso(now + 3600 + 30)),
              ('under-a-minute', iso(now + 45), iso(now + 45)), ('minutes', iso(now + 25 * 60 + 30), iso(now + 25 * 60 + 30)), ('whole-hours', iso(now + 3 * 3600 + 30), iso(now + 26 * 3600 + 30)),
              ('days', iso(now + 3 * 86400 + 30), iso(now + 6 * 86400 + 30)), ('days-and-hours', iso(now + 3 * 86400 + 4 * 3600 + 30), iso(now + 10 * 86400)),
              # 大于 1e11 的数当毫秒：公元 5138 年以后的秒数其实落在 1978 年（已重置）；要到一万年以后得写成毫秒，或者写成微秒那么大的数
              ('year-9999', '9999-12-31T23:59:59Z', '9999-12-30T00:00:00Z'), ('seconds-past-year-5138-read-as-ms', 253402300800 + 86400 * 400, 253402300800 + 86400 * 400),
              ('year-10000-as-ms', YEAR_10000 * 1000, YEAR_10000 * 1000), ('microseconds', int(soon) * 1000000, int(later) * 1000000),
              # 只有 5 小时的窗口离谱：那一列写的是还有多久，多大的数 Python 都写得出来（几百位的天数）
              ('five-hour-only-far', 5.5e23, iso(later)), ('five-hour-only-hugest', 1e300, iso(later)), ('five-hour-only-integer-huge', 10 ** 40, iso(later)),
              ('huge-epoch', 1e17, 1e17), ('huger-epoch', 1e19, 1e19), ('hugest-epoch', 1e300, 1e300), ('month-13', '2026-13-01T00:00:00Z', iso(later)), ('month-13-week', iso(soon), '2026-13-01T00:00:00Z'),
              ('year-0', '0000-01-01T00:00:00Z', iso(later)), ('day-99', iso(soon), iso(later)[:8] + '99T25:61:00Z'), ('unicode-digits', iso(soon).replace('2', '２', 1), iso(later))]
    out += [('resets-' + n, {'utilization': 13, 'resets_at': a}, {'utilization': 41, 'resets_at': c}) for n, a, c in resets]
    out += [('resets-missing', {'utilization': 13}, {'utilization': 41}), ('pct-missing', {'resets_at': iso(soon)}, {'resets_at': iso(later)})]
    return out


for i, (name, _five, _seven) in enumerate(window_cases(time.time())):
    @both('window-%s' % name, mask=LIVE, color='1')
    def _(b, i=i):
        world(b)
        claude_login(b, BOB)
        _, five, seven = window_cases(time.time())[i]
        result(b, 0, usage({'five_hour': five, 'seven_day': seven}))

for name, text in [('nan', 'NaN'), ('inf', 'Infinity'), ('neg-inf', '-Infinity')]:
    for field in ('utilization', 'resets_at'):
        for win in ('five_hour', 'seven_day'):
            @both('window-%s-%s-%s' % (win, field, name), mask=LIVE)
            def _(b, text=text, field=field, win=win):
                world(b)
                claude_login(b, BOB)
                r = usage(rl())
                r['usage']['rate_limits'][win][field] = '@@'
                result(b, 0, raw_number(r, text))

# --- 备注：分模型的每周额度、额外用量、credits、重置次数 ---
NOTES = [
    ('opus-and-sonnet', {'seven_day_opus': {'utilization': 12.5, 'resets_at': FAR}, 'seven_day_sonnet': {'utilization': 60, 'resets_at': FAR}}),
    ('zero-percent-not-shown', {'seven_day_opus': {'utilization': 0, 'resets_at': FAR}, 'seven_day_sonnet': {'utilization': 0.4, 'resets_at': FAR}}),
    ('negative-not-shown', {'seven_day_opus': {'utilization': -1, 'resets_at': FAR}}),
    ('reset-not-shown', {'seven_day_opus': {'utilization': 50, 'resets_at': '2020-01-01T00:00:00Z'}, 'seven_day_sonnet': {'utilization': 60}}),
    ('model-scoped', {'model_scoped': [{'display_name': 'Fable', 'utilization': 34, 'resets_at': FAR}, {'display_name': 'Ballad', 'utilization': 7.5, 'resets_at': None}]}),
    ('model-scoped-same-name-as-sonnet', {'seven_day_sonnet': {'utilization': 60, 'resets_at': FAR},
                                          'model_scoped': [{'display_name': ' sonnet ', 'utilization': 5}, {'display_name': 'FABLE', 'utilization': 1}, {'display_name': 'fable', 'utilization': 2}]}),
    ('model-scoped-odd-items', {'model_scoped': [5, 'x', None, [], {'utilization': 9}, {'display_name': None, 'utilization': 9}, {'display_name': '', 'utilization': 9}, {'display_name': 0, 'utilization': 9},
                                                 {'display_name': 7, 'utilization': 9}, {'display_name': ['a'], 'utilization': 9}, {'display_name': '  ', 'utilization': 9}, {'display_name': 'NoPct'},
                                                 {'display_name': 'Bool', 'utilization': True}]}),
    ('model-scoped-not-a-list', {'model_scoped': {'display_name': 'Fable', 'utilization': 34}}),
    ('model-scoped-empty', {'model_scoped': []}),
    ('model-scoped-null', {'model_scoped': None}),
    ('extra-usage-percent', {'extra_usage': {'is_enabled': True, 'utilization': 12.5, 'monthly_limit': 5000}}),
    ('extra-usage-percent-half-even', {'extra_usage': {'is_enabled': 1, 'utilization': 13.5}}),
    ('extra-usage-no-percent', {'extra_usage': {'is_enabled': True, 'utilization': None}}),
    ('extra-usage-percent-bool', {'extra_usage': {'is_enabled': True, 'utilization': True}}),
    ('extra-usage-off', {'extra_usage': {'is_enabled': False, 'utilization': 50}}),
    ('extra-usage-not-an-object', {'extra_usage': [1]}),
    ('credits-unlimited', {'credits': {'unlimited': True, 'hasCredits': True, 'balance': '5'}}),
    ('credits-balance', {'credits': {'unlimited': False, 'hasCredits': True, 'balance': '12.5'}}),
    ('credits-balance-number', {'credits': {'hasCredits': True, 'balance': 12.5}}),
    ('credits-balance-zero', {'credits': {'hasCredits': True, 'balance': 0}}),
    ('credits-none', {'credits': {'hasCredits': False, 'balance': '9'}}),
    ('credits-not-an-object', {'credits': 'lots'}),
    ('everything', {'seven_day_opus': {'utilization': 12.5, 'resets_at': FAR}, 'seven_day_sonnet': {'utilization': 60, 'resets_at': FAR},
                    'model_scoped': [{'display_name': 'Fable', 'utilization': 34, 'resets_at': FAR}], 'extra_usage': {'is_enabled': True, 'utilization': 3},
                    'credits': {'hasCredits': True, 'balance': '12.5'}, 'reset_credits': {'availableCount': 2}}),
]
for name, extra in NOTES:
    for cols in ('200', '70'):
        @render('notes-%s-cols-%s' % (name, cols), cols=cols, mask=LIVE)
        def _(b, extra=extra):
            world(b)
            claude_login(b, BOB)
            result(b, 0, usage(rl(**extra)))

    @render('notes-%s-json' % name, mode='json', mask=ALL)
    def _(b, extra=extra):
        world(b)
        claude_login(b, BOB)
        result(b, 0, usage(rl(**extra)))

for name, text in [('nan', 'NaN'), ('inf', 'Infinity')]:
    for what in ('seven_day_opus', 'extra_usage', 'model_scoped'):
        @both('notes-%s-percent-%s' % (what.replace('_', '-'), name), mask=LIVE)
        def _(b, text=text, what=what):
            world(b)
            claude_login(b, BOB)
            extra = {'seven_day_opus': {'utilization': '@@', 'resets_at': FAR}} if what == 'seven_day_opus' else \
                {'extra_usage': {'is_enabled': True, 'utilization': '@@'}} if what == 'extra_usage' else {'model_scoped': [{'display_name': 'Fable', 'utilization': '@@'}]}
            result(b, 0, raw_number(usage(rl(**extra)), text))

# --- 分模型的额度各占本周多少（weekly_scoped_shares 和 limits）---
FABLE = {'kind': 'weekly_scoped', 'percent': 44, 'resets_at': FAR, 'scope': {'model': {'id': None, 'display_name': 'Fable'}}}
WEEKLY = {'kind': 'weekly_all', 'percent': 61, 'resets_at': FAR, 'scope': None}
MS = [{'display_name': 'Fable', 'utilization': 43, 'resets_at': FAR}]
SHARE = {'used_percent_of_weekly': 21, 'allowance_percent_of_weekly': 50}
SHARES = [
    ('by-index', {'limits': [WEEKLY, FABLE], 'model_scoped': MS, 'weekly_scoped_shares': [dict(SHARE, limit_index=1)]}),
    ('index-float', {'limits': [WEEKLY, FABLE], 'model_scoped': MS, 'weekly_scoped_shares': [dict(SHARE, limit_index=1.0)]}),
    ('index-fraction', {'limits': [WEEKLY, FABLE], 'model_scoped': MS, 'weekly_scoped_shares': [dict(SHARE, limit_index=1.5)]}),
    ('index-negative', {'limits': [WEEKLY, FABLE], 'model_scoped': MS, 'weekly_scoped_shares': [dict(SHARE, limit_index=-1)]}),
    ('index-negative-zero', {'limits': [FABLE, WEEKLY], 'model_scoped': MS, 'weekly_scoped_shares': [dict(SHARE, limit_index=-0.0)]}),
    ('index-too-big', {'limits': [WEEKLY, FABLE], 'model_scoped': MS, 'weekly_scoped_shares': [dict(SHARE, limit_index=2)]}),
    ('index-huge', {'limits': [WEEKLY, FABLE], 'model_scoped': MS, 'weekly_scoped_shares': [dict(SHARE, limit_index=10 ** 30)]}),
    ('index-huge-float', {'limits': [WEEKLY, FABLE], 'model_scoped': MS, 'weekly_scoped_shares': [dict(SHARE, limit_index=1e300)]}),
    ('index-string', {'limits': [WEEKLY, FABLE], 'model_scoped': MS, 'weekly_scoped_shares': [dict(SHARE, limit_index='1')]}),
    ('index-bool', {'limits': [WEEKLY, FABLE], 'model_scoped': MS, 'weekly_scoped_shares': [dict(SHARE, limit_index=True)]}),
    ('index-missing', {'limits': [WEEKLY, FABLE], 'model_scoped': MS, 'weekly_scoped_shares': [SHARE]}),
    ('index-points-at-weekly-all', {'limits': [WEEKLY, FABLE], 'model_scoped': MS, 'weekly_scoped_shares': [dict(SHARE, limit_index=0)]}),
    ('limit-not-an-object', {'limits': [WEEKLY, 'x'], 'model_scoped': MS, 'weekly_scoped_shares': [dict(SHARE, limit_index=1)]}),
    ('no-limits-one-and-one', {'model_scoped': MS, 'weekly_scoped_shares': [dict(SHARE, limit_index=1)]}),
    ('no-limits-two-models', {'model_scoped': MS + [{'display_name': 'Ballad', 'utilization': 1}], 'weekly_scoped_shares': [dict(SHARE, limit_index=1)]}),
    ('no-limits-two-shares', {'model_scoped': MS, 'weekly_scoped_shares': [dict(SHARE, limit_index=1), dict(SHARE, limit_index=2)]}),
    ('limits-empty', {'limits': [], 'model_scoped': MS, 'weekly_scoped_shares': [dict(SHARE, limit_index=0)]}),
    ('limits-not-a-list', {'limits': {'0': FABLE}, 'model_scoped': MS, 'weekly_scoped_shares': [dict(SHARE, limit_index=0)]}),
    ('share-fields-not-numbers', {'limits': [WEEKLY, FABLE], 'model_scoped': MS, 'weekly_scoped_shares': [{'limit_index': 1, 'used_percent_of_weekly': '21', 'allowance_percent_of_weekly': True, 'more': 1}]}),
    ('share-one-field', {'limits': [WEEKLY, FABLE], 'model_scoped': MS, 'weekly_scoped_shares': [{'limit_index': 1, 'allowance_percent_of_weekly': 50.5}]}),
    ('two-shares-same-model', {'limits': [FABLE, FABLE], 'model_scoped': MS, 'weekly_scoped_shares': [dict(SHARE, limit_index=0), {'limit_index': 1, 'used_percent_of_weekly': 99}]}),
    ('name-differs-in-case', {'limits': [dict(FABLE, scope={'model': {'display_name': ' FABLE '}})], 'model_scoped': MS, 'weekly_scoped_shares': [dict(SHARE, limit_index=0)]}),
    ('shares-not-a-list', {'limits': [FABLE], 'model_scoped': MS, 'weekly_scoped_shares': dict(SHARE, limit_index=0)}),
    ('shares-odd-items', {'limits': [FABLE], 'model_scoped': MS, 'weekly_scoped_shares': [None, 5, [dict(SHARE, limit_index=0)]]}),
]
for name, extra in SHARES:
    @render('shares-%s' % name, mode='json', mask=ALL)
    def _(b, extra=extra):
        world(b)
        claude_login(b, BOB)
        result(b, 0, usage(rl(**extra)))

for name, text in [('nan', 'NaN'), ('inf', 'Infinity')]:
    @both('shares-index-%s' % name, mask=LIVE)
    def _(b, text=text):
        world(b)
        claude_login(b, BOB)
        result(b, 0, raw_number(usage(rl(limits=[WEEKLY, FABLE], model_scoped=MS, weekly_scoped_shares=[dict(SHARE, limit_index='@@')])), text))

    @both('shares-index-%s-without-limits' % name, mask=LIVE)
    def _(b, text=text):
        world(b)
        claude_login(b, BOB)
        result(b, 0, raw_number(usage(rl(model_scoped=MS, weekly_scoped_shares=[dict(SHARE, limit_index='@@')])), text))

# --- 周结算提醒（use_first）---
for name, pct, secs in [('plenty-left', 41, 10 * 3600), ('rounds-to-80', 80.4, 10 * 3600), ('rounds-to-81', 80.5, 10 * 3600), ('exactly-80', 80, 10 * 3600), ('over-100', 130, 3600),
                        ('negative', -20, 3600), ('far-away', 10, 9 * 86400), ('already-reset', 10, -60), ('huge', 1e300, 3600), ('huge-integer', 10 ** 30, 3600)]:
    @render('use-first-%s' % name, mode='json', mask=ALL)
    def _(b, pct=pct, secs=secs):
        world(b)
        claude_login(b, BOB)
        result(b, 0, usage(rl(13, pct, seven_day={'utilization': pct, 'resets_at': iso(time.time() + secs + 30)})))

for why in ('port', 'timeout', 'trace', 'region', 'route', 'auth', 'nolimits', 'unsupported', 'error'):
    @render('use-first-cached-why-%s' % why, mode='json', mask=GEN + AGE)
    def _(b, why=why):
        world(b)
        claude_login(b, BOB)
        skip(b, 0, why, 'x')
        cache(b, BOB, rl(7, 33), time.time() - 3600 - 30)


# --- 套餐那一列：账号资料（oauthAccount）和查询带回来的 subscription_type ---
PLANS = [
    ('display-name', {'planDisplayName': 'Max 20x'}, 'max'), ('display-name-wins', {'planDisplayName': 'Team Premium', 'organizationType': 'claude_pro'}, 'pro'),
    ('display-name-one-letter', {'planDisplayName': 'X'}, None), ('display-name-with-plus', {'planDisplayName': 'Max+'}, None), ('display-name-dots', {'planDisplayName': 'A.b-c d'}, None),
    ('display-name-chinese', {'planDisplayName': '专业版 2'}, None), ('display-name-40', {'planDisplayName': 'A' + 'b' * 39}, None), ('display-name-41', {'planDisplayName': 'A' + 'b' * 40}, None),
    ('display-name-underscore', {'planDisplayName': 'Max_20'}, None), ('display-name-leading-space', {'planDisplayName': ' Max'}, None), ('display-name-trailing-dot', {'planDisplayName': 'Max.'}, None),
    ('display-name-trailing-newline', {'planDisplayName': 'Max\n'}, None), ('display-name-empty', {'planDisplayName': ''}, None), ('display-name-number', {'planDisplayName': 5}, None),
    ('display-name-two-newlines', {'planDisplayName': 'Max\n\n'}, None), ('display-name-combining-mark', {'planDisplayName': 'Pró'}, None),
    ('max-20x', {'organizationRateLimitTier': 'default_claude_max_20x'}, 'max'), ('max-5x', {}, 'max'), ('max-no-tier', {'organizationRateLimitTier': None}, None),
    ('max-user-tier', {'organizationRateLimitTier': None, 'userRateLimitTier': 'default_claude_max_20x'}, None), ('max-both-tiers', {'userRateLimitTier': 'x_20x'}, None),
    ('max-tier-not-a-word-end', {'organizationRateLimitTier': 'max_20xl'}, None), ('max-tier-uppercase', {'organizationRateLimitTier': 'MAX_20X'}, None),
    ('max-tier-huge', {'organizationRateLimitTier': 'max_%sx' % ('9' * 30)}, None), ('max-tier-fullwidth', {'organizationRateLimitTier': 'max_２０x'}, None),
    ('max-tier-number', {'organizationRateLimitTier': 20}, None), ('max-tier-list', {'organizationRateLimitTier': ['20x']}, None),
    ('team-5x', {'organizationType': 'claude_team', 'organizationRateLimitTier': 'default_claude_team_5x'}, 'team'), ('team', {'organizationType': 'claude_team', 'organizationRateLimitTier': 'x'}, None),
    ('team-20x', {'organizationType': 'claude_team', 'organizationRateLimitTier': 'x_20x'}, None), ('pro', {'organizationType': 'claude_pro'}, 'pro'),
    ('enterprise', {'organizationType': 'claude_enterprise'}, None), ('free', {'organizationType': 'claude_free'}, None), ('edu', {'organizationType': 'claude_edu'}, None),
    ('uppercase-type', {'organizationType': 'CLAUDE_MAX'}, None), ('new-kind', {'organizationType': 'claude_mega_plan'}, None), ('kind-empty-after-prefix', {'organizationType': 'claude_'}, 'pro'),
    ('other-type-uses-sub', {'organizationType': 'api_org'}, 'max'), ('no-type-uses-sub', {'organizationType': None}, 'Pro'), ('no-type-no-sub', {'organizationType': None}, None),
    ('no-type-odd-sub', {'organizationType': None}, 'ßpecial'), ('no-type-georgian-sub', {'organizationType': None}, 'ქართული'), ('no-type-sigma-sub', {'organizationType': None}, 'ΑΣ'),
    ('no-type-digraph-sub', {'organizationType': None}, 'ǆx'), ('no-type-ligature-sub', {'organizationType': None}, 'ﬁne'), ('type-number', {'organizationType': 5}, None),
    ('type-list', {'organizationType': ['claude_max']}, None), ('type-true', {'organizationType': True}, None),
]
for name, oa, sub in PLANS:
    # 套餐叫什么 JSON 里就看得全；表格只多看一眼官方给的名字（宽的、带换行的）怎么排
    @(both('plan-%s' % name, mask=LIVE) if name.startswith('display-name') else render('plan-%s-json' % name, mode='json', mask=ALL))
    def _(b, oa=oa, sub=sub):
        world(b)
        claude_login(b, BOB, **dict({'organizationRateLimitTier': 'default_claude_max_5x'}, **oa))
        result(b, 0, usage(rl(), sub))

# Claude Code 登录时记的套餐（查询带回来的 subscription_type，存成 login_plan）和账号资料对不上
OUTDATED = [('pro-now-max', 'claude_max', 'pro'), ('max-now-pro', 'claude_pro', 'max'), ('same', 'claude_max', 'max'), ('team-now-enterprise', 'claude_enterprise', 'TEAM'),
            ('free-not-compared', 'claude_free', 'pro'), ('recorded-free-not-compared', 'claude_max', 'free'), ('no-type', None, 'pro'), ('recorded-number', 'claude_max', 5),
            ('recorded-empty', 'claude_max', ''), ('type-uppercase', 'CLAUDE_MAX', 'pro')]
for name, org, sub in OUTDATED:
    @both('outdated-%s' % name, mask=LIVE)
    def _(b, org=org, sub=sub):
        world(b)
        claude_login(b, BOB, organizationType=org)
        result(b, 0, usage(rl(), sub))

    @both('outdated-%s-cached' % name, mask=AGE)
    def _(b, org=org, sub=sub):
        world(b)
        claude_login(b, BOB, organizationType=org)
        skip(b, 0, 'timeout')
        cache(b, BOB, rl(), time.time() - 3600 - 30, login_plan=sub)

for name, uuid in [('other-account', 'uuid-someone-else'), ('no-uuid-in-file', None), ('uuid-number', 5)]:
    @both('outdated-login-plan-%s' % name, mask=AGE)
    def _(b, uuid=uuid):
        world(b)
        claude_login(b, BOB)
        skip(b, 0, 'timeout')
        cache(b, BOB, rl(), time.time() - 3600 - 30, uuid=uuid, login_plan='pro')


@both('outdated-not-said-when-logged-out', keys=(ALICE, BOB), mask=AGE)
def _(b):
    world(b)
    for i, (key, why) in enumerate([(ALICE, 'login'), (BOB, 'mismatch')]):
        claude_login(b, key)
        skip(b, i, why, 'x@example.com')
        cache(b, key, rl(), time.time() - 3600 - 30, login_plan='pro')


for cols in ('70', '100', '200'):
    @render('outdated-long-names-cols-%s' % cols, keys=('一个很长很长的别名一个很长很长的别名(bob@example.com)',), mask=LIVE, cols=cols)
    def _(b):
        world(b, table={'一个很长很长的别名一个很长很长的别名(bob@example.com)': 'US-B'})
        claude_login(b, BOB)
        result(b, 0, usage(rl(), 'pro'))


# --- 没查到时用哪份缓存：cca 存的 usage.json、Claude Code 自己的 cachedUsageUtilization ---
CACHE_TABLES = ('only-ours', 'theirs-newer-fills-model-scoped', 'ours-fresh-not-stale', 'ours-just-stale', 'ours-hours-old', 'ours-days-old', 'ours-windows-reset',
                'ours-fetched-at-hugely-negative', 'ours-fetched-at-long-ago', 'ours-fetched-at-in-the-future', 'ours-fetched-at-nan', 'ours-fetched-at-inf', 'both-other-account')


def cached_case(name, mine=None, theirs=None, mask=AGE, raw=None, their_raw=None, **kw):
    """mine / theirs：(多少秒以前, 用量窗口, 那份缓存里别的项)；用量窗口可以是照当时的时间现做的函数。
    挑哪一份、补了什么在 JSON 里看得全；表格只留和「多久以前」的写法有关的那几例（CACHE_TABLES）"""
    @(both('cache-%s' % name, mask=mask, **kw) if name in CACHE_TABLES else render('cache-%s-json' % name, mode='json', mask=list(mask) + GEN, **kw))
    def _(b):
        now = time.time()
        world(b)
        entry = lambda x: dict({'fetchedAtMs': int((now - x[0]) * 1000), 'accountUuid': 'uuid-bob', 'utilization': x[1]() if callable(x[1]) else x[1]}, **(x[2] if len(x) > 2 else {}))
        claude_login(b, BOB, cached=their_raw if their_raw is not None else entry(theirs) if theirs else None)
        skip(b, 0, 'timeout')
        if raw is not None:
            b.write(acct(BOB) + '/usage.json', raw() if callable(raw) else raw)
        elif mine:
            b.write(acct(BOB) + '/usage.json', json.dumps(entry(mine)))


def ours(**change):
    """cca 存的那份（有 model_scoped）"""
    return lambda: dict(rl(60, 60, limits=[dict(WEEKLY, percent=60), dict(FABLE, percent=43)], model_scoped=MS, weekly_scoped_shares=[dict(SHARE, limit_index=1)]), **change)


def theirs(drop=(), **change):
    """Claude Code 自己存的那份（/api/oauth/usage 的原样：没有 model_scoped）"""
    return lambda: {k: v for k, v in dict(rl(61, 61, limits=[WEEKLY, FABLE], weekly_scoped_shares=[dict(SHARE, limit_index=1, used_percent_of_weekly=22)]), **change).items() if k not in drop}


cached_case('only-ours', (1830, ours()))
cached_case('only-theirs', None, (1830, theirs()))
cached_case('theirs-newer-fills-model-scoped', (1830, ours()), (630, theirs()))
cached_case('ours-newer', (630, ours()), (1830, theirs()))
cached_case('same-age-ours-first', (1830, ours()), (1830, theirs(five_hour={'utilization': 99, 'resets_at': FAR})))
cached_case('theirs-newer-without-limits', (1830, ours()), (630, theirs(drop=('limits',))))
cached_case('theirs-newer-limits-lost-the-model', (1830, ours()), (630, theirs(limits=[WEEKLY])))
cached_case('theirs-newer-limit-percent-not-a-number', (1830, ours()), (630, theirs(limits=[WEEKLY, dict(FABLE, percent=None)])))
cached_case('theirs-newer-ours-has-no-models', (1830, ours(model_scoped=[])), (630, theirs()))
cached_case('theirs-newer-ours-models-not-a-list', (1830, ours(model_scoped='x')), (630, theirs()))
cached_case('theirs-newer-ours-duplicate-models', (1830, ours(model_scoped=MS * 2 + [{'utilization': 1}, {'display_name': None, 'utilization': 2}, {'display_name': 'Other', 'utilization': 3}])), (630, theirs()))
cached_case('theirs-newer-has-its-own-models', (1830, ours()), (630, theirs(model_scoped=[{'display_name': 'Theirs', 'utilization': 9}])))
cached_case('theirs-newer-several-limits-same-model', (1830, ours()), (630, theirs(limits=[FABLE, dict(FABLE, percent=99), dict(FABLE, kind='weekly_all', percent=1), dict(FABLE, scope=None),
                                                                                         dict(FABLE, scope={'model': 'Fable'}), dict(FABLE, scope={'model': {'display_name': 5}}), 'x'])))
cached_case('ours-other-account', (1830, ours(), {'accountUuid': 'uuid-other'}), (3630, theirs()))
cached_case('both-other-account', (1830, ours(), {'accountUuid': 'uuid-other'}), (3630, theirs(), {'accountUuid': 'uuid-third'}), mask=())
cached_case('ours-no-uuid', (1830, ours(), {'accountUuid': None}))
cached_case('ours-uuid-empty', (1830, ours(), {'accountUuid': ''}))
cached_case('ours-utilization-not-an-object', (1830, [1]), (3630, theirs()))
cached_case('ours-fetched-at-string', (1830, ours(), {'fetchedAtMs': '1795000000000'}), (3630, theirs()))
cached_case('ours-fetched-at-bool', (1830, ours(), {'fetchedAtMs': True}), (3630, theirs()))
cached_case('ours-fetched-at-float', (1830, ours(), {'fetchedAtMs': 1795000000000.5}))
cached_case('ours-fetched-at-zero', (1830, ours(), {'fetchedAtMs': 0}))
cached_case('ours-fetched-at-zero-theirs-newer', (1830, ours(), {'fetchedAtMs': 0}), (3630, theirs()))
cached_case('ours-fetched-at-negative', (1830, ours(), {'fetchedAtMs': -5000}))
cached_case('ours-fetched-at-in-the-future', (-7200, ours()))
cached_case('ours-fetched-at-huge', (1830, ours(), {'fetchedAtMs': 10 ** 30}))
cached_case('ours-fetched-at-hugely-negative', (1830, ours(), {'fetchedAtMs': -10 ** 30}))      # 表格里「缓存（…天前）」是个二十几位的数；JSON 里那个时间写不出来
cached_case('ours-fetched-at-long-ago', (1830, ours(), {'fetchedAtMs': -5e19}))
cached_case('ours-fetched-at-far-future', (1830, ours(), {'fetchedAtMs': 4e14}))
cached_case('ours-not-json', raw='{oops', mask=())
cached_case('ours-not-an-object', raw='[1]', mask=())
cached_case('ours-empty-windows', (1830, {}))
cached_case('ours-windows-reset', (86400 * 9, lambda: rl(50, 50, time.time() - 86400 * 9)))
cached_case('ours-fresh-not-stale', (90, ours()))
cached_case('ours-just-stale', (630, ours()))
cached_case('ours-hours-old', (3 * 3600 + 1830, ours()))
cached_case('ours-days-old', (3 * 86400 + 1830, ours()))
cached_case('theirs-not-an-object', (1830, ours()), their_raw=[1, 2])
cached_case('theirs-a-string-ours-missing', their_raw='x', mask=())
for name, text in [('nan', 'NaN'), ('inf', 'Infinity')]:
    cached_case('ours-fetched-at-%s' % name, raw=lambda text=text: raw_number({'fetchedAtMs': '@@', 'accountUuid': 'uuid-bob', 'utilization': rl()}, text))


@both('cache-claude-json-broken')
def _(b):
    world(b)
    claude_login(b, BOB, raw='{"oauthAccount": ')
    skip(b, 0, 'timeout')


@both('cache-claude-json-broken-ro', env={'CCA_RO': '1'}, mask=AGE)
def _(b):
    world(b)
    claude_login(b, BOB, raw='{"oauthAccount": ')
    skip(b, 0, 'timeout')
    cache(b, BOB, rl(), time.time() - 1830)


@both('cache-claude-json-not-an-object', mask=AGE)
def _(b):
    world(b)
    claude_login(b, BOB, raw='[1, 2]')
    skip(b, 0, 'timeout')
    cache(b, BOB, rl(), time.time() - 1830)


@both('cache-oauth-account-not-an-object', mask=AGE)
def _(b):
    world(b)
    claude_login(b, BOB, raw=json.dumps({'oauthAccount': 'bob', 'cachedUsageUtilization': 'x'}))
    skip(b, 0, 'timeout')
    cache(b, BOB, rl(), time.time() - 1830)


@both('cache-not-logged-in-no-uuid', mask=AGE)
def _(b):
    world(b)
    skip(b, 0, 'timeout')
    cache(b, BOB, rl(), time.time() - 1830)


@both('cache-alias-only-key-email-from-login', keys=('工作',), mask=AGE)
def _(b):
    world(b, table={'工作': 'US-A'})
    b.write('.cca/accounts/%s/claude/.claude.json' % ident('工作'), json.dumps({'oauthAccount': {'emailAddress': 'someone@example.com', 'accountUuid': 'u-1', 'organizationType': 'claude_pro'}}))
    skip(b, 0, 'port', '7901')
    b.write('.cca/accounts/%s/usage.json' % ident('工作'), json.dumps({'fetchedAtMs': int((time.time() - 1830) * 1000), 'accountUuid': 'u-1', 'utilization': rl()}))


@both('alias-only-key-not-logged-in', keys=('工作',))
def _(b):
    world(b, table={'工作': 'US-A'})
    skip(b, 0, 'login')


@both('alias-only-key-email-is-a-number', keys=('工作',))
def _(b):
    world(b, table={'工作': 'US-A'})
    b.write('.cca/accounts/%s/claude/.claude.json' % ident('工作'), json.dumps({'oauthAccount': {'emailAddress': 0, 'accountUuid': 'u-1'}}))
    skip(b, 0, 'timeout')


# --- 查到了：存进 usage.json，本周的用量记进 usage-weeks.json ---
def saved_case(name, entries=None, raw=None, week=41, at=3 * 86400 + 5 * 3600, sub='max', prior=None, **more):
    """entries：usage-weeks.json 里已经记下的几周，end 写的是相对这次查到的那一周的重置时间差多少秒"""
    @render('save-%s' % name, mode='json', mask=ALL)
    def _(b):
        now = time.time()
        world(b)
        claude_login(b, BOB)
        seven = {'utilization': week, 'resets_at': at(now) if callable(at) else iso(now + at) if isinstance(at, (int, float)) else at}
        result(b, 0, usage(rl(13, 41, now, seven_day=seven, **more), sub))
        if raw is not None:
            b.write(acct(BOB) + '/usage-weeks.json', raw)
        elif entries is not None:
            end = float(int(now + 3 * 86400 + 5 * 3600))
            b.write(acct(BOB) + '/usage-weeks.json', json.dumps({'version': 1, 'weeks': [dict(e, end=end + e.get('end', 0)) for e in entries]}, indent=2) + '\n')
        if prior is not None:
            b.write(acct(BOB) + '/usage.json', prior)


W0 = {'peak': 30.0, 'first': 1794000000.5, 'last': 1794000600.5, 'carried': 0, 'resets': 0}
saved_case('first-time')
saved_case('same-week-higher', [W0])
saved_case('same-week-a-bit-lower', [dict(W0, peak=44.0)])
saved_case('same-week-exactly-5-lower', [dict(W0, peak=46.0)])
saved_case('same-week-dropped-means-reset', [dict(W0, peak=80.0)])
saved_case('same-week-dropped-again', [dict(W0, peak=80.0, carried=95.5, resets=2)])
saved_case('same-week-integers-in-file', [{'peak': 80, 'first': 1794000000, 'last': 1794000600, 'carried': 20, 'resets': 1}])
saved_case('same-week-integer-peak-kept', [{'peak': 41, 'first': 1794000000, 'last': 1794000600, 'carried': 0, 'resets': 0}])
saved_case('same-week-integer-peak-higher-kept', [{'peak': 45, 'first': 1794000000, 'last': 1794000600, 'carried': 3, 'resets': 0}])
saved_case('same-week-end-moved-a-little', [dict(W0, end=1800)])
saved_case('same-week-end-just-within-the-hour', [dict(W0, end=-3599)])
saved_case('other-week-end-over-an-hour-away', [dict(W0, end=-3601)])
saved_case('new-week-after-old-ones', [dict(W0, end=-14 * 86400), dict(W0, end=-7 * 86400, peak=99.5)])
saved_case('new-week-sorted-in-the-middle', [dict(W0, end=-7 * 86400), dict(W0, end=7 * 86400), dict(W0, end=14 * 86400)])
saved_case('last-is-in-the-future', [dict(W0, last=4e9)])
saved_case('keeps-60-weeks', [dict(W0, end=-(i + 1) * 7 * 86400, peak=float(i)) for i in range(62)][::-1])
saved_case('entries-out-of-order-get-sorted', [dict(W0, end=14 * 86400), dict(W0, end=-7 * 86400), dict(W0, end=7 * 86400, peak=1)])
saved_case('bad-entries-dropped', raw=json.dumps({'version': 1, 'weeks': [
    'x', 5, None, [], {}, {'end': 'x', 'peak': 1, 'first': 1, 'last': 1}, {'end': 1, 'peak': True, 'first': 1, 'last': 1}, {'end': 1, 'peak': 1, 'first': 1},
    {'end': 1e18, 'peak': 1, 'first': 1, 'last': 1}, {'end': -1e18, 'peak': 1, 'first': 1, 'last': 1}, {'end': 12345678901234567890, 'peak': 1, 'first': 1, 'last': 1},
    {'end': 100, 'peak': 2, 'first': 3, 'last': 4}, {'end': 50.5, 'peak': 2.5, 'first': 3, 'last': 4, 'carried': 'x', 'resets': 'y', 'more': 1},
    {'end': 75, 'peak': 2, 'first': 3, 'last': 4, 'carried': 1e18, 'resets': 2.9}, {'end': 80, 'peak': 2, 'first': 3, 'last': 4, 'carried': 7.5, 'resets': -2.9},
    {'end': 90, 'peak': 2, 'first': 3, 'last': 4, 'carried': True, 'resets': True}, {'end': 999999999999999999, 'peak': 1, 'first': 1, 'last': 1}]}))
saved_case('nan-entries-dropped', raw='{"version": 1, "weeks": [{"end": NaN, "peak": 1, "first": 1, "last": 1}, {"end": 5, "peak": Infinity, "first": 1, "last": 1},'
                                      ' {"end": 6, "peak": 1, "first": 1, "last": 1, "carried": NaN, "resets": -Infinity}]}')
saved_case('weeks-file-not-json', raw='{oops')
saved_case('weeks-file-not-an-object', raw='[1]')
saved_case('weeks-not-a-list', raw='{"version": 1, "weeks": {"end": 1}}')
saved_case('weeks-file-other-version', raw='{"version": 9, "weeks": [], "extra": true}')
saved_case('week-already-reset-not-recorded', at=-60)
saved_case('week-no-reset-time-not-recorded', at=None)
saved_case('week-reset-time-garbage-not-recorded', at='soon')
saved_case('week-percent-huge-not-recorded', week=1e18)
saved_case('week-percent-float', week=41.25)
saved_case('week-percent-negative', week=-3)
saved_case('week-epoch-reset-time', at=lambda now: int(now) + 3 * 86400)
saved_case('week-epoch-ms-reset-time', at=lambda now: (int(now) + 3 * 86400) * 1000 + 250)
saved_case('week-reset-time-far-away', at=lambda now: 9e17)
saved_case('week-reset-time-too-far', at=lambda now: 1e18)
saved_case('sub-not-a-string-not-recorded', sub=5)
saved_case('sub-empty-not-recorded', sub='')
saved_case('sub-none-not-recorded', sub=None)
saved_case('replaces-old-usage-json', prior=json.dumps({'fetchedAtMs': 5, 'accountUuid': 'old', 'utilization': {}, 'login_plan': 'pro', 'extra': 1}))
saved_case('replaces-broken-usage-json', prior='{oops')


@render('save-not-logged-in-uuid-null', mode='json', mask=ALL)
def _(b):
    world(b)
    result(b, 0, usage(rl()))


@render('save-usage-json-is-a-loose-file', mode='json', mask=ALL)
def _(b):
    world(b)
    claude_login(b, BOB)
    result(b, 0, usage(rl()))
    b.write(acct(BOB) + '/usage.json', '{}', 0o644)
    b.write(acct(BOB) + '/usage-weeks.json', '{}', 0o644)


@render('save-usage-json-is-a-link', mode='json', mask=ALL)
def _(b):
    world(b)
    claude_login(b, BOB)
    result(b, 0, usage(rl()))
    b.write('elsewhere/u.json', '{}')
    b.write('elsewhere/w.json', '{}')
    os.symlink(b.path('elsewhere/u.json'), b.path(acct(BOB) + '/usage.json'))
    os.symlink(b.path('elsewhere/w.json'), b.path(acct(BOB) + '/usage-weeks.json'))


@render('save-cannot-write-either-file', mode='json', mask=ALL)
def _(b):
    # 两个文件都是指向名字很长的文件的链接：旁边的临时文件名字超长，写不进去（OSError），照样往下走
    world(b)
    claude_login(b, BOB)
    result(b, 0, usage(rl(), 'pro'))
    for name in ('usage.json', 'usage-weeks.json'):
        long_name = name[0] * 250
        b.write('elsewhere/' + long_name, '{}')
        os.symlink(b.path('elsewhere/' + long_name), b.path(acct(BOB) + '/' + name))


@render('save-account-dir-is-a-file', mode='json', mask=ALL)
def _(b):
    world(b)
    result(b, 0, usage(rl()))
    b.write(acct(BOB), 'not a directory')


@render('save-two-accounts', (ALICE, BOB), mask=LIVE)
def _(b):
    world(b)
    claude_login(b, ALICE, organizationRateLimitTier='default_claude_max_20x')
    claude_login(b, BOB, organizationType='claude_team', organizationRateLimitTier='default_claude_team_5x')
    result(b, 0, usage(rl(5, 6)))
    result(b, 1, usage(rl(7.5, 8.5), 'team'))


# --- 后台的两件事：同步记下备查的 API key（开着设备统计时）、统计折合金额 ---
# 假的 cca 把自己被怎么调的记下来。两件事是紧挨着起的两个后台进程，谁先写完说不准：各记各的文件（calls.key-sync.log、calls.cost-sync.log）
KICKED = 'printf \'%s TOOL=[%s] RO=[%s] EVENTS=[%s]\\n\' "cca $*" "$CCA_TOOL" "$CCA_RO" "$CCA_EVENTS" >> "$HOME/calls.${1#_}.log"\n'
PIN = 'sha256//' + 'A' * 43 + '='
PRESENCE = {'url': 'https://192.0.2.10:18443/', 'pins': [PIN], 'salt': 'ab' * 16}
PUB = 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f cca-policy\n'


def kicks(b, cfg=None, presence=False, keys_json=None, last=None):
    world(b, cfg=cfg, policy={'version': 1, 'claude': {'bob@example.com': 'US-B'}, 'presence': PRESENCE} if presence else None)
    claude_login(b, BOB)
    result(b, 0, usage(rl()))
    b.write('.cca/bin/cca', '#!/bin/sh\n' + KICKED, 0o755)
    if presence:
        b.write('.cca/policy/id_ed25519.pub', PUB, 0o644)
        b.write('.cca/policy/id_ed25519', 'PRIVATE KEY PLACEHOLDER\n')
    if keys_json is not None:
        b.write('.cca/presence/keys.json', json.dumps(keys_json(time.time())))
    if last is not None:
        p = b.write('.cca/cost/last', '')
        os.utime(p, (time.time() - last, time.time() - last))


KICKS = [
    ('cost-default', {}, {}), ('cost-off-by-env', {}, {'CCA_COST': 'off'}), ('cost-off-in-config', {'cfg': {'cost': False}}, {}), ('cost-zero-in-config-is-on', {'cfg': {'cost': 0}}, {}),
    ('cost-ran-a-minute-ago', {'last': 60}, {}), ('cost-ran-long-ago', {'last': 900}, {}), ('cost-stamp-in-the-future', {'last': -900}, {}),
    ('events-not-passed-on', {}, {'CCA_EVENTS': '/dev/null', 'CCA_RO': ''}),
    ('keys-presence-on', {'presence': True}, {'CCA_COST': 'off'}), ('keys-synced-recently', {'presence': True, 'keys_json': lambda now: {'v': 1, 'acc': {}, 'synced': now - 60}}, {'CCA_COST': 'off'}),
    ('keys-synced-long-ago', {'presence': True, 'keys_json': lambda now: {'v': 1, 'acc': {}, 'synced': now - 6000}}, {'CCA_COST': 'off'}),
    ('keys-retry-later', {'presence': True, 'keys_json': lambda now: {'v': 1, 'acc': {}, 'synced': 0, 'retry': now + 6000}}, {'CCA_COST': 'off'}),
    ('keys-and-cost', {'presence': True}, {}),
]
for name, fixture, env in KICKS:
    @case('usage-render/kick-%s' % name, ['usage-render', OUT, 'table', '100', '', BOB], mask=LIVE, env=env, settle=0.8)
    def _(b, fixture=fixture):
        kicks(b, **fixture)


@case('usage-render/kick-codex', ['usage-render', OUT, 'json', '100', '', 'erin@example.com'], tool='codex', mask=ALL, settle=0.8)
def _(b):
    world(b, codex={'erin@example.com': 'HK-A'})
    b.write('.cca/bin/cca', '#!/bin/sh\n' + KICKED, 0o755)
    codex_login(b, 'erin@example.com')
    result(b, 0, usage(crl(), 'plus'))


@case('usage-render/kick-agy-has-no-cost', ['usage-render', OUT, 'json', '100', '', 'gina@example.com'], tool='agy', mask=ALL, settle=0.8)
def _(b):
    world(b, agy={'gina@example.com': 'US-A'})
    b.write('.cca/bin/cca', '#!/bin/sh\n' + KICKED, 0o755)
    agy_login(b, 'gina@example.com')
    result(b, 0, usage(rl()))


@case('usage-render/kick-no-config-file', ['usage-render', OUT, 'json', '100', ''], mask=GEN, settle=0.5)
def _(b):
    b.write('.cca/bin/cca', '#!/bin/sh\n' + KICKED, 0o755)


# ================================================================================================================
# usage-render：Codex、agy
# ================================================================================================================
ERIN, FRED = 'erin@example.com', '备用(fred@example.com)'
CODEX_TABLE = {ERIN: 'HK-A', FRED: 'US-A', 'kimi': {'base_url': 'https://api.example.com/v1'}}


def crl(five=13, week=41, now=None, plan='pro', **extra):
    """codex 查到的用量（usage-query 整理过的样子）：重置时间是整数秒"""
    now = now or time.time()
    d = {'five_hour': {'utilization': five, 'resets_at': int(now + 2 * 3600 + 13 * 60 + 30)}, 'seven_day': {'utilization': week, 'resets_at': int(now + 3 * 86400 + 5 * 3600)}}
    if plan:
        d['plan'] = plan
    d.update(extra)
    return d


def credits(now, *expires, count=None, **more):
    """还能重置几次：每一次什么时候过期（秒，None 是不过期）"""
    cs = [{'id': 'credit-%d' % i, 'status': 'available', 'title': None, 'expiresAt': None if e is None else iso(now + e)} for i, e in enumerate(expires)]
    d = {'availableCount': len(expires) if count is None else count, 'credits': cs}
    d.update(more)
    return d


def codex_world(b):
    world(b, codex=CODEX_TABLE)
    codex_login(b, ERIN, 'plus')
    codex_login(b, FRED, 'free')


def codex_rich(b):
    now = time.time()
    codex_world(b)
    other = [{'utilization': 30, 'resets_at': int(now + 86400 * 2), 'display_name': 'GPT-5-Pro'}]
    result(b, 0, usage(crl(13, 41, now, model_scoped=other, credits={'hasCredits': True, 'unlimited': False, 'balance': '12.5'},
                           reset_credits=credits(now, 20 * 86400, 5 * 86400)), 'pro'))
    skip(b, 1, 'port', '7903')
    cache(b, FRED, crl(7, 33, now, 'prolite'), now - 23 * 60 - 30, 'codex', uuid='fred@example.com|ws-1')


for cols in (40, 48, 56, 64, 72, 80, 88, 96, 104, 112, 120, 140, 160, 200):
    render('codex-rich-cols-%d' % cols, (ERIN, FRED), cols=str(cols), mask=LIVE, tool='codex')(codex_rich)
render('codex-rich-color', (ERIN, FRED), color='1', mask=LIVE, tool='codex')(codex_rich)
render('codex-rich-json', (ERIN, FRED), 'json', mask=ALL, tool='codex')(codex_rich)
render('codex-rich-subset-json', (FRED,), 'json', mask=ALL, tool='codex')(codex_rich)      # 结果文件按位置认：0.json 成了 fred 的

# (名字, 照当时的时间做出用量的函数, 查询带回来的套餐, 登录凭证里的套餐)
CODEX_ROWS = [
    ('plan-from-query', lambda now: crl(now=now, plan=None), 'promax', 'plus'), ('plan-from-login', lambda now: crl(now=now, plan=None), None, 'prolite'),
    ('plan-unknown-capitalized', lambda now: crl(now=now, plan=None), 'brand_new plan', 'plus'), ('plan-padded-uppercase', lambda now: crl(now=now, plan=None), '  TEAM ', 'plus'),
    ('plan-number', lambda now: crl(now=now, plan=None), 5, 'plus'), ('plan-list', lambda now: crl(now=now, plan=None), ['pro'], 'plus'), ('plan-nowhere', lambda now: crl(now=now, plan=None), None, ''),
    ('plan-business-premium', lambda now: crl(now=now, plan=None), 'self_serve_business_prolite', 'plus'), ('plan-enterprise-automation', lambda now: crl(now=now, plan=None), 'enterprise_cbp_automation', 'plus'),
    ('reset-count-zero', lambda now: crl(now=now, reset_credits={'availableCount': 0, 'credits': []}), 'pro', 'plus'),
    ('reset-count-one', lambda now: crl(now=now, reset_credits=credits(now, 5 * 86400)), 'pro', 'plus'),
    ('reset-count-one-no-expiry', lambda now: crl(now=now, reset_credits=credits(now, None)), 'pro', 'plus'),
    ('reset-count-three-mixed', lambda now: crl(now=now, reset_credits=credits(now, None, 9 * 86400, 2 * 86400)), 'pro', 'plus'),
    ('reset-count-without-details', lambda now: crl(now=now, reset_credits={'availableCount': 3}), 'pro', 'plus'),
    ('reset-count-more-than-details', lambda now: crl(now=now, reset_credits=credits(now, 5 * 86400, count=4)), 'pro', 'plus'),
    ('reset-count-float', lambda now: crl(now=now, reset_credits={'availableCount': 2.9}), 'pro', 'plus'),
    ('reset-count-float-under-one', lambda now: crl(now=now, reset_credits={'availableCount': 0.9}), 'pro', 'plus'),
    ('reset-count-negative', lambda now: crl(now=now, reset_credits={'availableCount': -2}), 'pro', 'plus'),
    ('reset-count-negative-float', lambda now: crl(now=now, reset_credits={'availableCount': -0.5}), 'pro', 'plus'),
    ('reset-count-bool', lambda now: crl(now=now, reset_credits={'availableCount': True}), 'pro', 'plus'),
    ('reset-count-string', lambda now: crl(now=now, reset_credits={'availableCount': '2'}), 'pro', 'plus'),
    ('reset-count-huge', lambda now: crl(now=now, reset_credits={'availableCount': 10 ** 30}), 'pro', 'plus'),
    ('reset-count-huge-float', lambda now: crl(now=now, reset_credits={'availableCount': 1e30}), 'pro', 'plus'),
    ('reset-count-huge-negative', lambda now: crl(now=now, reset_credits={'availableCount': -10 ** 30}), 'pro', 'plus'),
    ('reset-credits-not-an-object', lambda now: crl(now=now, reset_credits=[2]), 'pro', 'plus'),
    ('reset-credits-expired-only', lambda now: crl(now=now, reset_credits=credits(now, -86400, count=1)), 'pro', 'plus'),
    ('reset-credits-odd-details', lambda now: crl(now=now, reset_credits={'availableCount': 2, 'credits': [
        5, None, {'id': 'a', 'status': 'used', 'expiresAt': iso(now + 86400)}, {'id': '', 'status': 'available'}, {'id': 7, 'status': 'available'}, {'status': 'available'},
        {'id': 'late', 'status': 'available', 'expiresAt': int(now + 9 * 86400), 'title': ' 周年礼物 ', 'expires_epoch': 'overwritten'},
        {'id': 'soon', 'status': 'available', 'expiresAt': iso(now + 2 * 86400)}, {'id': 'never', 'status': 'available', 'expiresAt': 'whenever'}]}), 'pro', 'plus'),
    ('reset-credits-details-not-a-list', lambda now: crl(now=now, reset_credits={'availableCount': 2, 'credits': {'id': 'a'}}), 'pro', 'plus'),
    ('reset-credits-bad-date', lambda now: crl(now=now, reset_credits={'availableCount': 2, 'credits': [{'id': 'a', 'status': 'available', 'expiresAt': '2026-13-01T00:00:00Z'}]}), 'pro', 'plus'),
    ('reset-credits-expiry-year-10000', lambda now: crl(now=now, reset_credits={'availableCount': 2, 'credits': [{'id': 'a', 'status': 'available', 'expiresAt': YEAR_10000 * 1000}]}), 'pro', 'plus'),
    ('reset-credits-expiry-huge', lambda now: crl(now=now, reset_credits={'availableCount': 2, 'credits': [{'id': 'a', 'status': 'available', 'expiresAt': 1e300}]}), 'pro', 'plus'),
    ('credits-unlimited', lambda now: crl(now=now, credits={'unlimited': True}), 'pro', 'plus'),
    ('windows-missing-seven-day', lambda now: crl(now=now, seven_day=None), 'pro', 'plus'),
]
for name, make, sub, login_plan in CODEX_ROWS:
    @both('codex-%s' % name, (ERIN,), mask=LIVE, tool='codex', cols='160')
    def _(b, make=make, sub=sub, login_plan=login_plan):
        world(b, codex=CODEX_TABLE)
        codex_login(b, ERIN, login_plan)
        result(b, 0, usage(make(time.time()), sub))

for name, text in [('nan', 'NaN'), ('inf', 'Infinity'), ('neg-inf', '-Infinity')]:
    @both('codex-reset-count-%s' % name, (ERIN,), mask=LIVE, tool='codex')
    def _(b, text=text):
        codex_world(b)
        result(b, 0, raw_number(usage(crl(reset_credits={'availableCount': '@@'}), 'pro'), text))

    @both('codex-reset-credit-expires-%s' % name, (ERIN,), mask=LIVE, tool='codex')
    def _(b, text=text):
        codex_world(b)
        result(b, 0, raw_number(usage(crl(reset_credits={'availableCount': 1, 'credits': [{'id': 'a', 'status': 'available', 'expiresAt': '@@'}]}), 'pro'), text))

CODEX_CACHED = [
    ('plan-from-cache', 'promax', 'plus', {}), ('plan-in-cache-empty-uses-login', '', 'prolite', {}), ('other-workspace-not-used', 'pro', 'plus', {'uuid': 'erin@example.com|ws-other'}),
    ('other-email-not-used', 'pro', 'plus', {'uuid': 'someone@example.com|ws-1'}), ('uuid-missing-used', 'pro', 'plus', {'uuid': None}),
]
for name, cached_plan, login_plan, more in CODEX_CACHED:
    @both('codex-cached-%s' % name, (ERIN,), mask=AGE, tool='codex')
    def _(b, cached_plan=cached_plan, login_plan=login_plan, more=more):
        world(b, codex=CODEX_TABLE)
        codex_login(b, ERIN, login_plan)
        skip(b, 0, 'timeout')
        cache(b, ERIN, crl(plan=cached_plan), time.time() - 1830, 'codex', **more)


@both('codex-cached-not-logged-in', (ERIN,), mask=AGE, tool='codex')
def _(b):
    world(b, codex=CODEX_TABLE)
    skip(b, 0, 'auth')
    cache(b, ERIN, crl(), time.time() - 1830, 'codex')


@both('codex-login-plan-not-recorded', (ERIN,), mask=LIVE, tool='codex')
def _(b):
    codex_world(b)
    result(b, 0, usage(crl(), 'pro'))      # Codex 不记 login_plan


@both('codex-why-reasons', (ERIN, FRED), tool='codex')
def _(b):
    codex_world(b)
    skip(b, 0, 'login')
    result(b, 1, {'ok': False, 'why': 'unsupported', 'msg': 'x'})


AGY_TABLE = {'gina@example.com': 'US-A', '备用(hank@example.com)': 'US-B'}
for name, tier in [('pro', 'g1-pro-tier'), ('ultra', 'g1-ultra-tier'), ('free', 'free-tier'), ('unknown', 'brand-new-tier'), ('number', 5), ('empty', '')]:
    @both('agy-tier-%s' % name, ('gina@example.com', '备用(hank@example.com)'), tool='agy')
    def _(b, tier=tier):
        world(b, agy=AGY_TABLE)
        agy_login(b, 'gina@example.com', tier)
        result(b, 0, usage(rl(), None))      # agy 没有认得的窗口：查到了也算没有订阅用量
        skip(b, 1, 'login')

for name, sub in [('sub-wins', 'legacy-tier'), ('sub-unknown', 'x'), ('sub-number', 5), ('sub-list', ['g1-pro-tier']), ('sub-object', {'tier': 'x'})]:
    @both('agy-%s' % name, ('gina@example.com',), tool='agy')
    def _(b, sub=sub):
        world(b, agy=AGY_TABLE)
        agy_login(b, 'gina@example.com')
        result(b, 0, usage(rl(), sub))


@both('agy-not-logged-in', ('gina@example.com',), tool='agy')
def _(b):
    world(b, agy=AGY_TABLE)
    skip(b, 0, 'timeout')
    cache(b, 'gina@example.com', rl(), time.time() - 1830, 'agy')      # agy 不看缓存


# ================================================================================================================
# login-plan-forget
# ================================================================================================================
FORGET = [
    ('has-login-plan', {'fetchedAtMs': 1795000000000, 'accountUuid': 'uuid-bob', 'utilization': {'five_hour': {'utilization': 1.5, 'resets_at': None}}, 'login_plan': 'pro'}),
    ('login-plan-null', {'fetchedAtMs': 1, 'login_plan': None, 'utilization': {}}), ('no-login-plan', {'fetchedAtMs': 1, 'utilization': {}}),
    ('only-login-plan', {'login_plan': 'max'}), ('not-an-object', ['login_plan']), ('not-json', '{"login_plan": '), ('missing', None), ('empty', ''),
    ('one-line-file-gets-reformatted', '{"login_plan":"pro","b":[1,2.50,"中"],"a":{}}'),
]
for name, j in FORGET:
    for tool, key in (('claude', BOB), ('codex', ERIN)):
        @case('login-plan-forget/%s%s' % (name, '' if tool == 'claude' else '-' + tool), ['login-plan-forget', key], tool=tool)
        def _(b, j=j, tool=tool, key=key):
            world(b, codex=CODEX_TABLE)
            if j is not None:
                b.write(acct(key, tool) + '/usage.json', j if isinstance(j, str) else json.dumps(j), 0o644)


@case('login-plan-forget/file-is-a-link', ['login-plan-forget', BOB])
def _(b):
    world(b)
    b.write('elsewhere/u.json', '{"login_plan": "pro", "x": 1}', 0o644)
    os.makedirs(b.path(acct(BOB)))
    os.symlink(b.path('elsewhere/u.json'), b.path(acct(BOB) + '/usage.json'))


@case('login-plan-forget/cannot-write', ['login-plan-forget', BOB])
def _(b):
    world(b)
    b.write('elsewhere/' + 'u' * 250, '{"login_plan": "pro", "x": 1}', 0o644)
    os.makedirs(b.path(acct(BOB)))
    os.symlink(b.path('elsewhere/' + 'u' * 250), b.path(acct(BOB) + '/usage.json'))


@case('login-plan-forget/unknown-account', ['login-plan-forget', 'ghost@example.com'])
def _(b):
    world(b)


@case('login-plan-forget/agy', ['login-plan-forget', 'gina@example.com'], tool='agy')
def _(b):
    world(b, agy={'gina@example.com': 'US-A'})
    b.write(acct('gina@example.com', 'agy') + '/usage.json', '{"login_plan": "pro", "x": 1}')


@case('login-plan-forget/no-args', ['login-plan-forget'])
def _(b):
    world(b)


# ================================================================================================================
# reset-plan / reset-done（cca codex reset 的头尾两步）
# ================================================================================================================
def rcase(cmd, name, argv=(), tool='codex', mask=(), key=ERIN, **kw):
    return case('%s/%s' % (cmd, name), [cmd, OUT, key] + list(argv), tool=tool, mask=list(mask), env=OFF, **kw)


PLAN_OK = [
    ('two-credits', lambda now: crl(reset_credits=credits(now, 20 * 86400, 5 * 86400))),
    ('one-credit-no-expiry-with-title', lambda now: crl(reset_credits={'availableCount': 1, 'credits': [{'id': 'c-1', 'status': 'available', 'title': '  周年礼物  ', 'expiresAt': None}]})),
    ('title-blank', lambda now: crl(reset_credits={'availableCount': 1, 'credits': [{'id': 'c-1', 'status': 'available', 'title': '   ', 'expiresAt': int(now + 86400)}]})),
    ('title-number', lambda now: crl(reset_credits={'availableCount': 1, 'credits': [{'id': 'c-1', 'status': 'available', 'title': 5, 'expiresAt': None}]})),
    ('title-with-quote-and-newline', lambda now: crl(reset_credits={'availableCount': 1, 'credits': [{'id': "c'1 $(x)", 'status': 'available', 'title': "it's\na gift", 'expiresAt': None}]})),
    ('count-without-details', lambda now: crl(reset_credits={'availableCount': 3})),
    ('count-zero', lambda now: crl(reset_credits={'availableCount': 0, 'credits': []})),
    ('count-zero-with-details', lambda now: crl(reset_credits=credits(now, 86400, count=0))),
    ('count-float', lambda now: crl(reset_credits={'availableCount': 2.9})),
    ('count-negative', lambda now: crl(reset_credits={'availableCount': -1})),
    ('count-huge', lambda now: crl(reset_credits={'availableCount': 10 ** 30})),
    ('count-huge-float', lambda now: crl(reset_credits={'availableCount': 1e30})),
    ('sorted-by-expiry', lambda now: crl(reset_credits=credits(now, None, 9 * 86400, 2 * 86400, 2 * 86400, -5))),
    ('skips-used-and-nameless', lambda now: crl(reset_credits={'availableCount': 5, 'credits': [
        {'id': 'used', 'status': 'used', 'expiresAt': iso(now + 60)}, {'id': '', 'status': 'available'}, {'id': 9, 'status': 'available'}, 'x',
        {'id': 'ok', 'status': 'available', 'expiresAt': 'not a date'}, {'id': 'ok-2', 'status': 'available', 'expiresAt': int(now + 86400) * 1000}]})),
    ('old-codex-no-reset-credits', lambda now: crl()),
    ('reset-credits-not-an-object', lambda now: crl(reset_credits='2')),
    ('count-bool', lambda now: crl(reset_credits={'availableCount': False})),
    ('count-null', lambda now: crl(reset_credits={'availableCount': None, 'credits': []})),
    ('details-not-a-list', lambda now: crl(reset_credits={'availableCount': 2, 'credits': 'x'})),
    ('bad-date', lambda now: crl(reset_credits={'availableCount': 1, 'credits': [{'id': 'a', 'status': 'available', 'expiresAt': '2026-00-01T00:00:00Z'}]})),
    ('expiry-year-10000', lambda now: crl(reset_credits={'availableCount': 1, 'credits': [{'id': 'a', 'status': 'available', 'expiresAt': YEAR_10000 * 1000}]})),
    ('expiry-huge', lambda now: crl(reset_credits={'availableCount': 1, 'credits': [{'id': 'a', 'status': 'available', 'expiresAt': 1e300}]})),
    ('expiry-far-beyond-time-t', lambda now: crl(reset_credits={'availableCount': 1, 'credits': [{'id': 'a', 'status': 'available', 'expiresAt': 1e17}]})),
]
for name, make in PLAN_OK:
    @rcase('reset-plan', name, mask=LIVE)
    def _(b, make=make):
        codex_world(b)
        result(b, 0, usage(make(time.time()), 'pro'))

for name, text in [('nan', 'NaN'), ('inf', 'Infinity')]:
    @rcase('reset-plan', 'count-%s' % name, mask=LIVE)
    def _(b, text=text):
        codex_world(b)
        result(b, 0, raw_number(usage(crl(reset_credits={'availableCount': '@@'}), 'pro'), text))

    @rcase('reset-plan', 'expiry-%s' % name, mask=LIVE)
    def _(b, text=text):
        codex_world(b)
        result(b, 0, raw_number(usage(crl(reset_credits={'availableCount': 1, 'credits': [{'id': 'a', 'status': 'available', 'expiresAt': '@@'}]}), 'pro'), text))

for why, msg in WHYS:
    @rcase('reset-plan', 'why-%s%s' % (why or 'empty', '-msg' if msg else ''))
    def _(b, why=why, msg=msg):
        codex_world(b)
        skip(b, 0, why, msg)
        cache(b, ERIN, crl(), time.time() - 1830, 'codex')      # 这里不看缓存

PLAN_BAD = [('no-result', None), ('not-available', lambda: usage(crl(), available=False)), ('no-windows', lambda: usage({'plan': 'pro', 'reset_credits': {'availableCount': 2}})),
            ('usage-missing', lambda: {'ok': True}), ('why-number', lambda: {'ok': False, 'why': 5}), ('why-list', lambda: {'ok': False, 'why': ['x']}),
            ('msg-number', lambda: {'ok': False, 'why': 'port', 'msg': 7903}), ('ok-false-with-usage', lambda: {'ok': False, 'usage': usage(crl())['usage']}),
            ('why-unsupported-as-json', lambda: {'ok': False, 'why': 'unsupported', 'msg': 'Method not found'})]
for name, make in PLAN_BAD:
    @rcase('reset-plan', name)
    def _(b, make=make):
        codex_world(b)
        if make is not None:
            result(b, 0, make())


@rcase('reset-plan', 'not-logged-in-email-from-key', mask=LIVE)
def _(b):
    world(b, codex=CODEX_TABLE)
    result(b, 0, usage(crl(reset_credits={'availableCount': 1}), 'pro'))


@rcase('reset-plan', 'alias-key', key=FRED)
def _(b):
    codex_world(b)
    skip(b, 0, 'mismatch', 'someone@example.com')


@rcase('reset-plan', 'claude-tool', tool='claude', key=BOB, mask=LIVE)
def _(b):
    world(b)
    claude_login(b, BOB)
    result(b, 0, usage(rl(reset_credits=credits(time.time(), 86400))))      # 不看是哪个工具：照样算；Claude 的 login_plan 这里不记


@rcase('reset-plan', 'claude-tool-unsupported', tool='claude', key=BOB)
def _(b):
    world(b)
    result(b, 0, {'ok': False, 'why': 'unsupported'})


@rcase('reset-plan', 'agy-tool', tool='agy', key='gina@example.com')
def _(b):
    world(b, agy=AGY_TABLE)
    result(b, 0, usage(rl(reset_credits={'availableCount': 1})))


@case('reset-plan/no-args', ['reset-plan', OUT], tool='codex')
def _(b):
    codex_world(b)


def consumed(outcome, u=Ellipsis, **more):
    """reset-consume 打印的那一行"""
    r = {'ok': True, 'outcome': outcome, 'usage': usage(crl(0, 0, reset_credits={'availableCount': 1}), 'pro')['usage'] if u is Ellipsis else u}
    r.update(more)
    return r


# (名字, 现做结果的函数, 后面的参数)
DONE = [
    ('reset', lambda: consumed('reset'), []), ('reset-json', lambda: consumed('reset'), ['json', 'credit-0']), ('reset-no-usage', lambda: consumed('reset', None), ['json']),
    ('reset-usage-without-windows', lambda: consumed('reset', {'rate_limits_available': True, 'rate_limits': {}}), ['json']),
    ('reset-usage-not-available', lambda: consumed('reset', dict(usage(crl())['usage'], rate_limits_available=False)), ['json']),
    ('already-redeemed', lambda: consumed('already_redeemed'), []), ('already-redeemed-json', lambda: consumed('already_redeemed'), ['json']),
    ('nothing-to-reset', lambda: consumed('nothing_to_reset'), []), ('nothing-to-reset-json', lambda: consumed('nothing_to_reset'), ['json', '']),
    ('no-credit', lambda: consumed('no_credit'), []), ('no-credit-picked-one', lambda: consumed('no_credit'), ['', 'credit-0']), ('no-credit-json', lambda: consumed('no_credit'), ['json']),
    ('unknown-outcome', lambda: consumed('exploded'), ['json']), ('outcome-missing', lambda: {'ok': True}, []), ('outcome-number', lambda: consumed(5), []), ('outcome-null', lambda: consumed(None), []),
    ('outcome-list', lambda: consumed(['reset']), []), ('outcome-object', lambda: consumed({'reset': 1}), []), ('outcome-camel-case-not-ours', lambda: consumed('alreadyRedeemed'), []),
    ('timeout', lambda: {'ok': False, 'why': 'timeout', 'msg': ''}, []), ('error', lambda: {'ok': False, 'why': 'error', 'msg': 'boom'}, ['json']), ('error-no-msg', lambda: {'ok': False, 'why': 'error'}, []),
    ('unsupported', lambda: {'ok': False, 'why': 'unsupported', 'msg': 'Method not found'}, []), ('auth', lambda: {'ok': False, 'why': 'auth', 'msg': '401 Unauthorized'}, []),
    ('why-unknown', lambda: {'ok': False, 'why': 'weird'}, []), ('why-list', lambda: {'ok': False, 'why': ['x']}, []), ('ok-false-with-outcome', lambda: {'ok': False, 'outcome': 'reset', 'why': 'timeout'}, []),
    ('no-result', lambda: None, []), ('mode-spelled-differently', lambda: consumed('reset'), ['JSON']),
    ('reset-usage-with-credits-left', lambda: consumed('reset', usage(crl(0, 0, reset_credits=credits(time.time(), 5 * 86400, None), credits={'hasCredits': True, 'balance': '3'}), 'pro')['usage']), ['json']),
    ('reset-usage-half-used', lambda: consumed('reset', usage(crl(12.5, 99.5), 'pro')['usage']), []),
]
for name, make, argv in DONE:
    @rcase('reset-done', name, argv, mask=ALL)
    def _(b, make=make):
        codex_world(b)
        skip(b, 0, 'port', 'this is step 0 and is not read')
        if make() is not None:
            result(b, 1, make())

for name, why, msg in [('skip-port', 'port', '7903'), ('skip-login', 'login', ''), ('skip-route', 'route', ''), ('skip-region', 'region', '出口在受限地区'), ('skip-trace', 'trace', ''), ('skip-mismatch', 'mismatch', 'x@example.com')]:
    @rcase('reset-done', name)
    def _(b, why=why, msg=msg):
        codex_world(b)
        skip(b, 1, why, msg)


@rcase('reset-done', 'stale-cache-in-the-message', mask=ALL)
def _(b):
    # 重置了但没带回用量：那句话里不写现在的用量（哪怕有缓存）
    codex_world(b)
    result(b, 1, consumed('reset', None))
    cache(b, ERIN, crl(50, 60), time.time() - 1830, 'codex')


@rcase('reset-done', 'reset-windows-already-over', ['json'], mask=ALL)
def _(b):
    codex_world(b)
    now = time.time()
    u = {'five_hour': {'utilization': 0, 'resets_at': int(now - 60)}, 'seven_day': {'utilization': 12.5, 'resets_at': None}, 'plan': 'pro'}
    result(b, 1, consumed('reset', usage(u, 'pro')['usage']))


@rcase('reset-done', 'alias-key', ['json'], key=FRED, mask=ALL)
def _(b):
    codex_world(b)
    result(b, 1, consumed('reset'))


@rcase('reset-done', 'claude-tool', ['json'], tool='claude', key=BOB, mask=ALL)
def _(b):
    world(b)
    claude_login(b, BOB)
    result(b, 1, {'ok': True, 'outcome': 'reset', 'usage': usage(rl(0, 0), 'pro')['usage']})


@rcase('reset-done', 'not-logged-in', ['json'], mask=ALL)
def _(b):
    world(b, codex=CODEX_TABLE)
    result(b, 1, consumed('reset'))


@rcase('reset-done', 'agy-tool', ['json'], tool='agy', key='gina@example.com', mask=ALL)
def _(b):
    world(b, agy=AGY_TABLE)
    agy_login(b, 'gina@example.com')
    result(b, 1, consumed('reset'))      # agy 没有认得的窗口：带回来的用量不算数，那句话照说


@case('reset-done/no-args', ['reset-done', OUT], tool='codex')
def _(b):
    codex_world(b)


@case('reset-done/too-many-args', ['reset-done', OUT, ERIN, 'json', 'c', 'x'], tool='codex')
def _(b):
    codex_world(b)
