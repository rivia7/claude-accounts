"""折合金额（cli/src/cmd/cost.rs）：cost-worker、cost-sync、cost-stop。

会话记录都是编的（照 Claude Code、Codex 写的格式，只有统计要看的那几样），放在沙箱的 ~/.claude/projects、~/.codex/sessions 里。
时间都是相对现在往前推的（统计按“最近多少天、多少周”取舍），离各个界线都远，两边先后跑差的那几秒不影响结果。
文件建立的先后决定读的先后：夹具给每个文件一个过去的修改时间（macOS 上修改时间早于建立时间时，建立时间跟着改），拷出来的
沙箱里先后就是定的。scan.json 里记的 inode 每个沙箱不一样，比之前换掉。

接着上次读（增量）的用例用 before：命令开始前先拿原来的 Python 跑一遍 cost-worker（两边都是它跑的，起点一样），再往会话记录里
添东西。和别的设备同步的用例借 cases/presence.py 的假 curl、假 ssh-keygen；别的设备传来的密文拿 PY_SRC 自己的 cost_seal 现算。
Codex 官方账号的身份（codex_claims）在 usage+codex 那一批里：合进来以前这里的 Codex 只有会话记录，没有账号。"""
import base64
import calendar
import hashlib
import json
import os
import subprocess
import sys
import time

import run
from run import case
from cases.presence import CURL, KEYGEN, MASK, aid_of, reply as server, src
from cases.secrets import ALICE, BOB, PRESENCE, PUB, SHARING, world
from cases.session import PS, T0_EPOCH

NOW = time.time()
H = 3600
D = 86400
W = 7 * D
U1, U2 = '0b0c0d0e-aaaa-4bbb-8ccc-0123456789ab', '11111111-2222-4333-8444-555555555555'
CX = 'user-AbCdEfGhIjKlMnOp'


def near_now(m):
    """同步时记下的时间（现在，或者 6 小时以后再试）换掉；夹具里给的时间离现在都远，原样留着比"""
    v = float(m.group(2))
    return '"%s":<NOW>' % m.group(1) if any(abs(v - (time.time() + off)) < 900 for off in (0, 6 * 3600)) else m.group(0)


CMASK = MASK + [
    (r'"ino":\d+', '"ino":<I>'), (r'"(updated_epoch|generated_epoch)": \d+\.\d+', r'"\1": <T>'),
    (r'"(at|ok_at|up_at|checked|retry)":(\d+\.\d+)', near_now), (r'"(checked_epoch)": (\d+\.\d+)', near_now),
    (r':"[A-Za-z0-9+/]{43}="', ':"<TAG>"'),                                   # 这台设备自己传上去的那份的认证码（每次不一样）
    (r'"used_epoch":\d+', '"used_epoch":<T>'),                               # 现在减去服务器说的“多久以前用过”
    (r'"n": "[^"]+", "c": "[^"]+", "t": "[^"]+"', '<BLOB>'),      # 夹具里别的设备的密文（before 里现算的，两边各算一遍）
]


def iso(t, frac='.123'):
    return time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(t)) + frac + 'Z'


def dumps(d):
    return json.dumps(d, separators=(',', ':'), ensure_ascii=False)


def owner(uuid=U1, **more):
    return dumps(dict({'type': 'session-owner', 'accountUuid': uuid}, **more))


USAGE = {'input_tokens': 1200, 'output_tokens': 345, 'cache_read_input_tokens': 50000,
         'cache_creation': {'ephemeral_5m_input_tokens': 800, 'ephemeral_1h_input_tokens': 200}}


def msg(t, mid, model='claude-opus-5-5', usage=None, rid='req_011', **top):
    """一条回复（一行）；t 是时间戳，mid 是消息 id（None：没有 id）"""
    m = {'model': model, 'role': 'assistant', 'content': [{'type': 'text', 'text': '好的。'}], 'usage': USAGE if usage is None else usage}
    if mid is not None:
        m['id'] = mid
    d = {'parentUuid': 'p', 'type': 'assistant', 'timestamp': iso(t) if isinstance(t, (int, float)) else t, 'requestId': rid, 'message': m}
    d.update(top)
    return dumps(d)


def lines(*rows):
    return ''.join(r + '\n' for r in rows)


def put(b, rel, text, age):
    """写一个会话记录，修改时间（也就是建立时间）是 age 秒以前"""
    p = b.write(rel, text, 0o644)
    os.utime(p, (NOW - age, NOW - age))
    return p


def main(b, sid, text, age=3 * D, proj='-Users-me-proj'):
    return put(b, '.claude/projects/%s/%s.jsonl' % (proj, sid), text, age)


def sub(b, sid, name, text, age=3 * D, proj='-Users-me-proj'):
    return put(b, '.claude/projects/%s/%s/subagents/%s.jsonl' % (proj, sid, name), text, age)


def rollout(b, name, text, age=3 * D, where='sessions/2026/11/13'):
    return put(b, '.codex/%s/%s.jsonl' % (where, name), text, age)


def cx(kind, t, payload):
    return dumps({'timestamp': iso(t) if isinstance(t, (int, float)) else t, 'type': kind, 'payload': payload})


def meta(t, user=CX, provider='openai', **more):
    return cx('session_meta', t, dict({'id': 'sess-1', 'creator_user_id': user, 'model_provider': provider}, **more))


def turn(t, model='gpt-6.1-sol'):
    return cx('turn_context', t, {'cwd': '/x', 'model': model})


def settings(t, **s):
    return cx('event_msg', t, {'type': 'thread_settings_applied', 'thread_settings': s})


def count(t, total, last=None):
    """token_count 事件：total 是累计的输入（别的照比例编），last 是这一次的"""
    tu = lambda n: {'input_tokens': n, 'cached_input_tokens': n // 2, 'output_tokens': n // 10, 'reasoning_output_tokens': n // 20, 'total_tokens': n + n // 10}
    info = {'total_token_usage': tu(total) if isinstance(total, int) else total, 'last_token_usage': tu(last if last is not None else total) if not isinstance(last, dict) else last}
    return cx('event_msg', t, {'type': 'token_count', 'info': info, 'rate_limits': {'x': 1}})


def base(b, cfg=None, logged=True, master=True, **kw):
    """两个 Claude 官方账号（alice 登录着、身份是 U1），没有设备统计"""
    world(b, cfg=cfg, **kw)
    if logged:
        b.write('.cca/accounts/%s/claude/.claude.json' % ALICE, json.dumps({'oauthAccount': {'accountUuid': U1, 'emailAddress': 'Alice@Example.com'}}))
    if master:
        b.write('.claude.json', json.dumps({'oauthAccount': {'accountUuid': U2, 'emailAddress': 'bob@example.com'}}))


def ccase(name, argv, **kw):
    kw.setdefault('mask', CMASK)
    return case('cost/' + name, argv, **kw)


PRIME_ENV = {'LANG': 'en_US.UTF-8', 'TZ': 'Asia/Shanghai', 'CCA_TOOL': 'claude', 'CCA_BIN': ''}


def prime(b, *argv):
    """拿原来的 Python 在这个沙箱里先跑一遍（before 里用：两边的起点都是它跑出来的）"""
    p = b.path('../prime_src.py')
    if not os.path.exists(p):
        with open(p, 'w', encoding='utf-8') as f:
            f.write(run.py_src())
    env = dict(PRIME_ENV, HOME=b.home, PATH='%s/bin:/usr/bin:/bin:/usr/sbin:/sbin' % b.home, CCA_SELF=b.path('.cca/bin/cca'))
    subprocess.run([sys.executable, '-I', '-X', 'utf8', p] + list(argv or ['cost-worker']), env=env, cwd=b.home, stdin=subprocess.DEVNULL,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=60)


def append(p, text, age=None):
    with open(p, 'a', encoding='utf-8') as f:
        f.write(text)
    if age is not None:
        os.utime(p, (NOW - age, NOW - age))


# --- 用不了、没有可统计的 ---------------------------------------------------------------------------------------------
@ccase('sync/cost-off', ['cost-sync'])
def _(b):
    base(b, cfg={'cost': False})
    main(b, 's1', lines(owner(), msg(NOW - 2 * H, 'msg_1')))


@ccase('worker/cost-off', ['cost-worker'])
def _(b):
    base(b, cfg={'cost': False})
    main(b, 's1', lines(owner(), msg(NOW - 2 * H, 'msg_1')))


@ccase('sync/no-official-accounts', ['cost-sync', '--json'])
def _(b):
    base(b, table={'直连': {'base_url': 'https://api.example.com/anthropic'}}, logged=False)
    main(b, 's1', lines(owner(), msg(NOW - 2 * H, 'msg_1')))


@ccase('sync/no-tables', ['cost-sync'])
def _(b):
    b.cfg({'version': 2})


@ccase('sync/table-unreadable', ['cost-sync'])
def _(b):
    base(b)
    b.write('.cca/accounts.json', '{oops')


@ccase('worker/no-config', ['cost-worker'])
def _(b):
    base(b)
    os.unlink(b.path('.cca/config.json'))
    main(b, 's1', lines(owner(), msg(NOW - 2 * H, 'msg_1')))


@ccase('sync/no-config', ['cost-sync'])   # config.json 没了（cca restore 刚删掉）：不写任何东西
def _(b):
    base(b)
    os.unlink(b.path('.cca/config.json'))
    main(b, 's1', lines(owner(), msg(NOW - 2 * H, 'msg_1')))


@ccase('sync/config-unreadable', ['cost-sync'])
def _(b):
    base(b)
    b.write('.cca/config.json', '{oops')
    main(b, 's1', lines(owner(), msg(NOW - 2 * H, 'msg_1')))


@ccase('sync/cost-dir-is-a-file', ['cost-sync'])
def _(b):
    base(b)
    b.write('.cca/cost', 'not a directory')


def hold(name):
    def before(b):
        import fcntl
        os.makedirs(b.path('.cca/cost'), mode=0o700, exist_ok=True)
        f = open(b.path('.cca/cost/' + name), 'a')
        fcntl.flock(f, fcntl.LOCK_EX)
        return f.close
    return before


@ccase('worker/busy', ['cost-worker'], before=hold('cost.lock'))   # 另一个统计在跑：走
def _(b):
    base(b)
    main(b, 's1', lines(owner(), msg(NOW - 2 * H, 'msg_1')))


# --- 读会话记录 -------------------------------------------------------------------------------------------------------
for name, argv in [('worker', ['cost-worker']), ('sync-text', ['cost-sync']), ('sync-json', ['cost-sync', '--json']), ('sync-now', ['cost-sync', '--now', '--json']),
                   ('worker-extra-args', ['cost-worker', 'x', 'y'])]:
    @ccase('scan/basic-' + name, argv)
    def _(b):
        base(b)
        main(b, 's1', lines(dumps({'type': 'summary', 'summary': 'x'}), owner(), msg(NOW - 5 * H, 'msg_1'), msg(NOW - 5 * H + 60, 'msg_2', 'claude-sonnet-5-5'),
                            dumps({'type': 'user', 'message': {'role': 'user', 'content': 'usage of "assistant"?'}}), msg(NOW - 2 * H, 'msg_3')))


MODELS = [
    'claude-fable-5-2', 'claude-mythos-5-2', 'claude-fable-5-1', 'claude-fable-5', 'claude-opus-5-5', 'claude-opus-5', 'claude-opus-4-8', 'claude-opus-4-5', 'claude-opus-4-1',
    'claude-sonnet-5-5', 'claude-sonnet-5', 'claude-sonnet-4-6', 'claude-sonnet-4', 'claude-haiku-5-5', 'claude-haiku-4-5',
    'Claude-Opus-5-5[1m]', ' claude-sonnet-4-5-20250929 ', 'claude-haiku-4-5-2025-10-01', 'claude-opus-5-5-20261101[1m]', 'claude-next-9', 'claude-opus-5-5-preview',
    'gpt-6.1-sol', '', 'x' * 80, 'CLAUDE-ΣΟΦΟΣ-1', 'claude-opus-4-20250514',
]
USAGES = [
    USAGE,
    {'input_tokens': 7, 'output_tokens': 3},
    {'input_tokens': 120000, 'output_tokens': 4000, 'cache_read_input_tokens': 300, 'cache_creation_input_tokens': 999},          # 旧写法的缓存写
    {'input_tokens': 10, 'output_tokens': 20, 'cache_creation': {'ephemeral_5m_input_tokens': 0, 'ephemeral_1h_input_tokens': 0}, 'cache_creation_input_tokens': 55},
    {'input_tokens': 10, 'output_tokens': 20, 'cache_creation': 'x', 'cache_creation_input_tokens': 55},
    {'input_tokens': 99000, 'output_tokens': 100, 'cache_read_input_tokens': 900, 'cache_creation': {'ephemeral_5m_input_tokens': 101}},   # 超过 10 万：haiku 5.5 的长上下文价
    {'input_tokens': 99000, 'output_tokens': 100, 'cache_read_input_tokens': 900, 'cache_creation': {'ephemeral_5m_input_tokens': 100}},   # 正好 10 万：不算
    dict(USAGE, speed='fast'), dict(USAGE, speed='standard'), dict(USAGE, inference_geo='us'), dict(USAGE, inference_geo='eu'), dict(USAGE, speed='fast', inference_geo='us'),
    dict(USAGE, server_tool_use={'web_search_requests': 3}), dict(USAGE, server_tool_use={'web_search_requests': '3'}), dict(USAGE, server_tool_use=[1]),
    {'input_tokens': True, 'output_tokens': 1.5, 'cache_read_input_tokens': -4, 'cache_creation': {'ephemeral_5m_input_tokens': 10 ** 12, 'ephemeral_1h_input_tokens': 10 ** 12 - 1}},
    {'input_tokens': '12', 'output_tokens': None, 'cache_read_input_tokens': [1]},
    {},
    {'input_tokens': 333333, 'output_tokens': 77777, 'cache_read_input_tokens': 1234567, 'cache_creation': {'ephemeral_5m_input_tokens': 13, 'ephemeral_1h_input_tokens': 17}},
    {'input_tokens': 1, 'output_tokens': 0},
    {'input_tokens': 10 ** 30, 'output_tokens': 5},
]


@ccase('scan/every-model-and-usage', ['cost-sync', '--json'])   # 价格表的每一种算法：模型 × 用量各算一条，每条落在自己的小时里
def _(b):
    base(b)
    rows, n = [owner()], 0
    for m in MODELS:
        for u in USAGES:
            n += 1
            rows.append(msg(NOW - 10 * D + n * H / 3, 'msg_%d' % n, m, u))
    main(b, 's1', lines(*rows))


@ccase('scan/rounding', ['cost-worker'])   # 同一个小时里很多条，各自四舍五入再加
def _(b):
    base(b)
    rows = [owner()]
    for n in range(1, 60):
        rows.append(msg(NOW - 3 * H + n, 'msg_%d' % n, 'claude-haiku-5-5', {'input_tokens': n, 'output_tokens': n * 3 + 1, 'cache_read_input_tokens': n * 7}))
        rows.append(msg(NOW - 3 * H + n, 'gsg_%d' % n, 'claude-sonnet-4-5', {'input_tokens': n * 11, 'output_tokens': 1, 'cache_read_input_tokens': n * 5, 'inference_geo': 'us'}))
    main(b, 's1', lines(*rows))


@ccase('scan/dedupe', ['cost-sync', '--json'])   # 一条回复的每一块各写一行、复制的会话里也有：按消息 id 和 requestId 只算一次
def _(b):
    base(b)
    t = NOW - 6 * H
    main(b, 's1', lines(owner(), msg(t, 'msg_1'), msg(t, 'msg_1'), msg(t + 1, 'msg_1', rid='req_other'), msg(t + 2, 'msg_2', rid=None), msg(t + 3, 'msg_2', rid=None),
                        msg(t + 4, None), msg(t + 4, None), msg(t + 5, None), msg(t + 6, 5), msg(t + 7, 'msg_3', rid={'a': [1, None, True, 1.5, 'x']})), age=4 * D)
    main(b, 's2', lines(owner(U2), msg(t, 'msg_1'), msg(t + 9, 'msg_9')), age=3 * D)   # 复制出来的会话：msg_1 算在先建的 s1 上
    main(b, 's0', lines(owner(U2), msg(t + 7, 'msg_3', rid={'a': [1, None, True, 1.5, 'x']})), age=2 * D)


@ccase('scan/owners-and-subagents', ['cost-sync', '--json'])   # 换了账号续接；子代理的记录按所在会话、按时间认账号
def _(b):
    base(b)
    t = NOW - 30 * H
    main(b, 's1', lines(msg(t, 'a0'), owner(U1), msg(t + 100, 'a1'), owner(U1), msg(t + 200, 'a2'), owner(U2), msg(t + 300, 'a3'), owner('short'), msg(t + 400, 'a4'),
                        owner(U1), msg(t + 500, 'a5')), age=5 * D)
    sub(b, 's1', 'agent-1', lines(msg(t + 50, 'b0'), msg(t + 150, 'b1'), msg(t + 250, 'b2'), msg(t + 350, 'b3'), msg(t + 450, 'b4'), msg(t + 550, 'b5')), age=4 * D)
    sub(b, 's1', 'agent-2', lines(owner(U2), msg(t + 150, 'c1'), msg(t + 250, 'c2')), age=3 * D)     # 子代理自己写了账号
    sub(b, 'sX', 'agent-3', lines(msg(t + 150, 'd1')), age=3 * D)                                       # 所在的会话没有记录：认不出是谁的
    main(b, 's2', lines(dumps({'type': 'session-owner', 'accountUuid': U2, 'pad': 'x' * 600}), msg(t + 700, 'e1')), age=2 * D)   # 这一行太长：不认
    main(b, 's3', lines('{"type":"session-owner","accountUuid":"' + U2 + '"', msg(t + 800, 'f1')), age=2 * D)   # 写坏了
    main(b, 's4', lines(dumps({'type': 'session-owner', 'accountUuid': 12345678}), msg(t + 900, 'g1'), owner(U2, extra=1), msg(t + 950, 'g2')), age=2 * D)
    main(b, 's5', lines(' ' + owner(U2), msg(t + 990, 'h1')), age=2 * D)   # 不在行首：不认


@ccase('scan/timestamps', ['cost-sync', '--json'])
def _(b):
    base(b)
    t = int(NOW - 40 * H)
    stamps = [iso(t), iso(t, ''), iso(t, '.5'), time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(t)) + '+00:00', time.strftime('%Y-%m-%d %H:%M:%S', time.gmtime(t)),
              time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(t + 8 * H)) + '+08:00', t, t + 0.5, t * 1000, str(t), None, True, '', 'yesterday', [t],
              time.strftime('%Y-13-%dT%H:%M:%S', time.gmtime(t)) + '.000Z', time.strftime('0000-%m-%dT%H:%M:%S', time.gmtime(t)) + '.000Z',
              time.strftime('%Y-%m-45T%H:%M:%S', time.gmtime(t - 30 * D)) + '.000Z', time.strftime('%Y-%m-%dT25:61:61', time.gmtime(t - 2 * D)) + '.000Z',
              time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(t)) + 'Z', time.strftime('%Y-%m-%dT%H:%M', time.gmtime(t)) + 'Z', iso(NOW + D), iso(NOW + 3 * D),
              '1999-01-01T00:00:00.000Z', '2001-09-09T01:46:39.000Z', '2001-09-09T01:46:40.000Z', time.strftime(' %Y-%m-%dT%H:%M:%S', time.gmtime(t)) + '.000Z ',
              time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(t)).replace('-', '－', 1) + '.000Z', time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(t)) + '.000z',
              time.strftime('+%y0-%m-%dT%H:%M:%S', time.gmtime(t)) + '.000Z', time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(t)) + '.١٢٣Z',
              time.strftime('%Y-%m-%dT', time.gmtime(t)) + ' 5: 6: 7.000Z']
    main(b, 's1', lines(owner(), *[msg(s, 'msg_%d' % n) for n, s in enumerate(stamps)]))


BAD_LINES = [
    '{not json "usage" "assistant"}', '[1, "usage", "assistant"]', '"usage" "assistant"', 'null', '{"type":"assistant","message":"usage"}',
    dumps({'type': 'assistant', 'message': {'usage': [1], 'id': 'm1'}, 'timestamp': iso(NOW - 3 * H)}),
    dumps({'type': 'user', 'message': {'usage': USAGE, 'id': 'm2', 'role': 'assistant'}, 'timestamp': iso(NOW - 3 * H)}),
    dumps({'type': 'assistant', 'message': {'usage': USAGE, 'id': 'm3', 'model': '<synthetic>'}, 'timestamp': iso(NOW - 3 * H)}),
    dumps({'type': 'assistant', 'message': {'usage': USAGE, 'id': 'm4', 'model': 5}, 'timestamp': iso(NOW - 3 * H)}),
    dumps({'type': 'assistant', 'message': {'usage': USAGE, 'id': 'm5'}, 'timestamp': iso(NOW - 3 * H)}),
    dumps({'type': 'assistant', 'message': {'usage': USAGE, 'id': 'm6', 'model': 'claude-opus-5-5'}}),
    b'{"type":"assistant","timestamp":"' + iso(NOW - 3 * H).encode() + b'","message":{"id":"m7\xff","model":"claude-opus-5-5","usage":{"input_tokens":5}}}',
    '﻿' + msg(NOW - 3 * H, 'm8'),
    msg(NOW - 3 * H, 'm9') + '   \t',
    msg(NOW - 3 * H, 'm10') + ' trailing',
    '',
    '   ',
    msg(NOW - 3 * H, 'm11').replace('"type":"assistant"', '"type":"assistant","type":"other"'),
    msg(NOW - 3 * H, 'm12', usage=dict(USAGE, input_tokens=float('nan'))).replace('NaN', 'NaN'),
    msg(NOW - 3 * H, 'm13') .replace('好的。', '\\ud83d'),
    '[' * 2000 + '"usage","assistant"' + ']' * 2000,
    msg(NOW - 3 * H, 'm14'),
]


@ccase('scan/bad-lines', ['cost-sync', '--json'])   # 坏的行一行一行跳过，不影响后面的
def _(b):
    base(b)
    body = b''.join((x if isinstance(x, bytes) else x.encode('utf-8')) + b'\n' for x in [owner()] + BAD_LINES)
    main(b, 's1', body)


@ccase('scan/partial-last-line', ['cost-sync', '--json'])   # 还没写完的一行：这次不读
def _(b):
    base(b)
    main(b, 's1', lines(owner(), msg(NOW - 5 * H, 'msg_1')) + msg(NOW - 4 * H, 'msg_2'))
    main(b, 's2', msg(NOW - 4 * H, 'msg_3'))
    main(b, 's3', '')


@ccase('scan/crlf', ['cost-sync', '--json'])
def _(b):
    base(b)
    main(b, 's1', (owner() + '\r\n' + msg(NOW - 5 * H, 'msg_1') + '\r\n' + msg(NOW - 4 * H, 'msg_2') + '\r').encode())


@ccase('scan/which-files', ['cost-sync', '--json'])   # 哪些文件算会话记录：点开头的、不是 .jsonl 的、放错层的都不读
def _(b):
    base(b)
    t, n = NOW - 5 * H, [0]

    def one(who=U1):
        n[0] += 1
        return lines(owner(who), msg(t + n[0], 'f%d' % n[0]))
    main(b, 's1', one())
    main(b, '.hidden', one())
    main(b, 's2', one(), proj='.hidden-proj')
    put(b, '.claude/projects/-p/s3.JSONL', one(), 3 * D)
    put(b, '.claude/projects/-p/s4.jsonl.bak', one(), 3 * D)
    put(b, '.claude/projects/top.jsonl', one(), 3 * D)
    put(b, '.claude/projects/-p/deep/s5.jsonl', one(), 3 * D)
    put(b, '.claude/projects/-p/s6/subagents/.agent.jsonl', one(), 3 * D)
    put(b, '.claude/projects/-p/s6/subagents/deep/agent.jsonl', one(), 3 * D)
    put(b, '.claude/projects/-p/s6/other/agent.jsonl', one(), 3 * D)
    put(b, '.claude/projects/-p/.s7/subagents/agent.jsonl', one(), 3 * D)
    put(b, '.claude/projects/-p/名字 带空格.jsonl', one(), 3 * D)
    os.makedirs(b.path('.claude/projects/-p/dir.jsonl'))
    os.symlink(b.path('.claude/projects/-p/名字 带空格.jsonl'), b.path('.claude/projects/-p/link.jsonl'))
    os.symlink(b.path('nowhere'), b.path('.claude/projects/-p/broken.jsonl'))
    os.symlink(b.path('.claude/projects/-p'), b.path('.claude/projects/-linked-proj'))
    put(b, '.claude/projects/-p/subagents/agent.jsonl', one(), 3 * D)
    b.write('.claude/projects/-q', 'a file, not a project directory')
    rollout(b, 'rollout-a', lines(meta(t), turn(t), count(t + 50, 1000)))
    rollout(b, 'rollout-top', lines(meta(t), turn(t), count(t + 51, 2000)), where='sessions')
    rollout(b, 'rollout-arch', lines(meta(t), turn(t), count(t + 52, 3000)), where='archived_sessions')
    rollout(b, 'rollout-deep', lines(meta(t), turn(t), count(t + 53, 4000)), where='sessions/a/b/c/d/e')
    rollout(b, '.rollout-hidden', lines(meta(t), turn(t), count(t + 54, 5000)))
    rollout(b, 'rollout-in-hidden', lines(meta(t), turn(t), count(t + 55, 6000)), where='sessions/.trash')
    rollout(b, 'rollout-elsewhere', lines(meta(t), turn(t), count(t + 56, 7000)), where='other')
    put(b, '.codex/sessions/2026/notes.txt', 'x', 3 * D)
    os.symlink(b.path('.codex/sessions/2026'), b.path('.codex/sessions/linked'))


@ccase('scan/no-session-dirs', ['cost-sync', '--json'])
def _(b):
    base(b)


@ccase('scan/file-order-by-birth', ['cost-worker'])   # 先建的先读：一样的回复算在先建的会话上
def _(b):
    base(b)
    t = NOW - 6 * H
    main(b, 'zz-first', lines(owner(U1), msg(t, 'dup')), age=9 * D)
    main(b, 'aa-second', lines(owner(U2), msg(t, 'dup')), age=8 * D)
    main(b, 'mm-same-time-1', lines(owner(U2), msg(t + 1, 'dup2')), age=7 * D)
    main(b, 'mm-same-time-0', lines(owner(U1), msg(t + 1, 'dup2')), age=7 * D)


@ccase('scan/codex', ['cost-sync', '--json'])
def _(b):
    base(b)
    t = NOW - 26 * H
    rollout(b, 'rollout-1', lines(
        meta(t), count(t + 1, 500),                                            # 还不知道模型
        turn(t + 2), count(t + 3, 1000, 500), count(t + 4, 1000, 500),         # 累计没变：同一次请求又报了一遍
        count(t + 5, 300000, 299000),                                          # 一次请求的输入超过 272000：长上下文价
        settings(t + 6, model='gpt-6-astra', service_tier='fast'), count(t + 7, 301000, 1000),
        settings(t + 8, service_tier='ultrafast'), count(t + 9, 302000, 1000),
        settings(t + 10, model='gpt-5.6-luna', service_tier='ultrafast'), count(t + 11, 303000, 1000),
        settings(t + 12, model='gpt-5.6-luna', service_tier='flex'), count(t + 13, 304000, 1000),
        settings(t + 14, model='gpt-5.6-luna', service_tier='priority'), count(t + 15, 305000, 1000),
        settings(t + 16, model='gpt-5.6-luna', service_tier=5), count(t + 17, 306000, 1000),
        turn(t + 18, 'GPT-5.3-Codex'), count(t + 19, 307000, {'input_tokens': 1000, 'cached_input_tokens': 2000, 'cache_write_input_tokens': 300, 'output_tokens': 50}),
        count(t + 20, 308000, {'input_tokens': 1000, 'cached_input_tokens': 200, 'cache_write_input_tokens': 900, 'output_tokens': 50}),
        turn(t + 21, 'gpt-unknown-9'), count(t + 22, 309000, 1000),
        turn(t + 23, 5), count(t + 24, 310000, 1000),
        meta(t + 25, 'user-someone-else-000'), count(t + 26, 311000, 1000),    # 后面的 session_meta 是复制进来的历史：不认
        cx('event_msg', t + 27, {'type': 'token_count', 'info': None}), cx('event_msg', t + 28, {'type': 'token_count', 'info': {'total_token_usage': None, 'last_token_usage': None}}),
        cx('event_msg', t + 29, {'type': 'other', 'info': {'total_token_usage': {'x': 1}, 'last_token_usage': {'input_tokens': 5}}}),
        cx('response_item', t + 30, {'type': 'message', 'content': 'the "token_count" is far away' + 'x' * 200}),
        count('nonsense', 312000, 1000), count(None, 313000, 1000),
    ))
    rollout(b, 'rollout-2', lines(meta(t, provider='cca'), turn(t), count(t + 40, 1000)))                    # 第三方的接口：不算
    rollout(b, 'rollout-3', lines(meta(t, provider=''), turn(t), count(t + 41, 1000)))
    rollout(b, 'rollout-4', lines(meta(t, provider=None), turn(t), count(t + 42, 1000)))
    rollout(b, 'rollout-5', lines(meta(t, user='short'), turn(t), count(t + 43, 1000)))                      # 认不出是谁的
    rollout(b, 'rollout-6', lines(turn(t), count(t + 44, 1000), meta(t + 45, 'user-late-meta-00001'), count(t + 46, 2000)))
    rollout(b, 'rollout-7', lines(meta(t), turn(t), settings(t, model_provider_id='azure'), count(t + 47, 1000), settings(t, model_provider_id='openai'), count(t + 48, 2000),
                                  settings(t, model_provider_id=''), count(t + 49, 3000)))
    rollout(b, 'rollout-8', lines(meta(t, provider='cca'), turn(t), settings(t, model_provider_id='openai'), count(t + 50, 1000)))
    rollout(b, 'rollout-9', lines(dumps({'timestamp': iso(t), 'type': 'other', 'payload': {'x': '"session_meta"'}}), meta(t), turn(t), count(t + 51, 1000)))
    rollout(b, 'rollout-a', lines('{"session_meta" oops', '["turn_context"]', '"token_count"', meta(t), cx('turn_context', t, 'x'), cx('turn_context', t, {'model': None}),
                                  dumps({'type': 'turn_context', 'timestamp': iso(t)}), turn(t), count(t + 52, 1000), cx('event_msg', t, {'type': 'thread_settings_applied', 'thread_settings': 5})))
    rollout(b, 'rollout-b', lines(meta(t), turn(t), count(t + 60, 1000)), where='archived_sessions')         # 归档的照样读
    rollout(b, 'rollout-c', lines(meta(t), turn(t), count(t + 60, 1000)))                                    # 归档前后是同一条：只算一次


# --- 接着上次读 -------------------------------------------------------------------------------------------------------
def grown(b):
    prime(b)
    append(b.path('.claude/projects/-Users-me-proj/s1.jsonl'), lines(msg(NOW - 1 * H, 'msg_new'), msg(NOW - 5 * H, 'msg_1')), age=1 * D)
    append(b.path('.codex/sessions/2026/11/13/rollout-1.jsonl'), lines(count(NOW - 1 * H, 3000, 1000), count(NOW - 1 * H + 5, 3000, 1000), count(NOW - 1 * H + 9, 4000, 1000)), age=1 * D)
    main(b, 's9', lines(owner(U2), msg(NOW - 1 * H + 30, 'msg_other')), age=60)


def increment(b):
    base(b)
    main(b, 's1', lines(owner(), msg(NOW - 5 * H, 'msg_1'), msg(NOW - 4 * H, 'msg_2')) + msg(NOW - 3 * H, 'msg_cut')[:40])
    main(b, 's2', lines(owner(U2), msg(NOW - 4 * H, 'msg_3')))
    sub(b, 's1', 'agent-1', lines(msg(NOW - 4 * H + 5, 'sub_1')))
    rollout(b, 'rollout-1', lines(meta(NOW - 9 * H), turn(NOW - 9 * H), count(NOW - 8 * H, 2000, 2000)))


@ccase('resume/nothing-new', ['cost-sync', '--json'], before=prime)
def _(b):
    increment(b)


@ccase('resume/appended', ['cost-sync', '--json'], before=grown)
def _(b):
    increment(b)


def finished_line(b):
    prime(b)
    append(b.path('.claude/projects/-Users-me-proj/s1.jsonl'), msg(NOW - 3 * H, 'msg_cut')[40:] + '\n', age=1 * D)


@ccase('resume/partial-line-completed', ['cost-sync', '--json'], before=finished_line)
def _(b):
    increment(b)


def rewritten(b):
    prime(b)
    p = b.path('.claude/projects/-Users-me-proj/s1.jsonl')
    with open(p, 'r+', encoding='utf-8') as f:   # 同一个文件原地改写：长度一样，末尾那几十个字节变了
        text = f.read()
        f.seek(0)
        f.write(text.replace('msg_2', 'msg_X'))
    os.utime(p, (NOW - D, NOW - D))


@ccase('resume/rewritten-in-place', ['cost-sync', '--json'], before=rewritten)
def _(b):
    increment(b)


def truncated(b):
    prime(b)
    p = b.path('.claude/projects/-Users-me-proj/s1.jsonl')
    with open(p, 'r+', encoding='utf-8') as f:
        f.truncate(len(owner()) + 1)
    append(p, lines(msg(NOW - 2 * H, 'msg_after_cut')), age=1 * D)


@ccase('resume/truncated', ['cost-sync', '--json'], before=truncated)
def _(b):
    increment(b)


def replaced(b):
    prime(b)
    p = b.path('.claude/projects/-Users-me-proj/s2.jsonl')
    os.unlink(p)
    main(b, 's2', lines(owner(U2), msg(NOW - 4 * H, 'msg_3'), msg(NOW - 2 * H, 'msg_4')), age=1 * D)   # 换了一个文件（inode 不一样）：从头读，读过的不重复计
    os.unlink(b.path('.claude/projects/-Users-me-proj/s1/subagents/agent-1.jsonl'))                    # 删掉的：scan.json 里不再记它
    os.rename(b.path('.codex/sessions/2026/11/13/rollout-1.jsonl'), b.path('.codex/sessions/2026/11/13/rollout-moved.jsonl'))   # 改了名：重读，不重复计


@ccase('resume/replaced-removed-renamed', ['cost-sync', '--json'], before=replaced)
def _(b):
    increment(b)


def session_gone(b):
    prime(b)
    os.unlink(b.path('.claude/projects/-Users-me-proj/s1.jsonl'))   # 主会话没了：owners 里也不留；它的子代理这以后认不出是谁的
    append(b.path('.claude/projects/-Users-me-proj/s1/subagents/agent-1.jsonl'), lines(msg(NOW - 1 * H, 'sub_2')), age=1 * D)


@ccase('resume/main-session-removed', ['cost-sync', '--json'], before=session_gone)
def _(b):
    increment(b)


def twice(b):
    prime(b)
    prime(b)


@ccase('resume/local-json-unchanged', ['cost-worker'], before=twice)   # 内容没变：local.json 不重写
def _(b):
    increment(b)


def rust_first(b):
    """先拿 cca-core 跑一遍，再让两边接着读：Python 读得懂 cca-core 写的 scan.json"""
    env = dict(PRIME_ENV, HOME=b.home, PATH='%s/bin:/usr/bin:/bin:/usr/sbin:/sbin' % b.home, CCA_SELF=b.path('.cca/bin/cca'))
    subprocess.run([run.CORE, 'cost-worker'], env=env, cwd=b.home, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=60)
    append(b.path('.claude/projects/-Users-me-proj/s1.jsonl'), lines(msg(NOW - 1 * H, 'msg_new')), age=1 * D)


@ccase('resume/after-a-rust-run', ['cost-sync', '--json'], before=rust_first)
def _(b):
    increment(b)


# --- 旧的记录：第一次全读不管多早，以后比 35 天更早的不算；60 周以前的不留 ---------------------------------------------
OLD_TIMES = [NOW - 2 * H, NOW - 20 * D, NOW - 34 * D, NOW - 36 * D, NOW - 10 * W, NOW - 15 * W, NOW - 59 * W, NOW - 61 * W, NOW - 3 * 365 * D]


@ccase('old/first-read', ['cost-sync', '--json'])
def _(b):
    base(b)
    main(b, 's1', lines(owner(), *[msg(t, 'old_%d' % n) for n, t in enumerate(OLD_TIMES)], *[msg(t + 60, 'unp_%d' % n, 'claude-next-9') for n, t in enumerate(OLD_TIMES)]))
    main(b, 's2', lines(*[msg(t + 120, 'orph_%d' % n) for n, t in enumerate(OLD_TIMES)]))


def old_appended(b):
    prime(b)
    append(b.path('.claude/projects/-Users-me-proj/s1.jsonl'), lines(*[msg(t + 300, 'late_%d' % n) for n, t in enumerate(OLD_TIMES)]), age=1 * D)


@ccase('old/later-read', ['cost-sync', '--json'], before=old_appended)   # 以后才读到的更早的回复（改了名、复制出来的文件）：35 天以前的不算
def _(b):
    base(b)
    main(b, 's1', lines(owner(), msg(NOW - 3 * H, 'msg_1')))


def scan_json(b, **over):
    j = {'v': 1, 'build': '2026-11-13/1', 'done': True, 'files': {}, 'seen': {}, 'owners': {}, 'h': {}, 'orphan': {}, 'um': {}, 'who': {}}
    j.update(over)
    for k in [k for k, v in j.items() if v is Ellipsis]:
        del j[k]
    b.write('.cca/cost/scan.json', dumps(j))
    os.chmod(b.path('.cca/cost'), 0o700)


def hour(t):
    return str(int(t // 3600 * 3600))


ROW = [1500000, 3, 100, 200, 300, 400, 0]
KEPT = {'claude:' + U1: {hour(NOW - 2 * D): ROW, hour(NOW - 9 * D): ROW, hour(NOW - 50 * D): [9, 9, 9, 9, 9, 9, 9], hour(NOW - 61 * W): ROW},
        'claude:' + U2: {hour(NOW - 50 * D): ROW}, 'codex:' + CX: {hour(NOW - 50 * D): ROW, hour(NOW - 2 * D): ROW}, 'bad ident': {hour(NOW - 50 * D): ROW},
        'claude:short': {hour(NOW - 50 * D): ROW}}
SEEN = {'c0123456789abcdef': int(hour(NOW - 2 * D)), 'cfedcba9876543210': int(hour(NOW - 34 * D)), 'x0000000000000000': int(hour(NOW - 37 * D)), 'odd key': int(hour(NOW - 1 * D))}

for name, over in [
    ('ok-kept', dict(h=KEPT, seen=SEEN, orphan={hour(NOW - 3 * D): ROW, hour(NOW - 61 * W): ROW}, um={'claude:' + U1: {'claude-next-9': int(hour(NOW - 2 * D)), 'old-model': int(hour(NOW - 61 * W))}},
                     who={'claude:' + U1: 'Old@Example.com', 'codex:' + CX: 'carol@example.com', 'bad': 'x@example.com', 'claude:' + U2: 'not an email'})),
    ('not-done', dict(done=False, h=KEPT, seen=SEEN)),
    ('other-build', dict(build='2026-10-01/3', h=KEPT, seen=SEEN, orphan={hour(NOW - 50 * D): ROW, hour(NOW - 2 * D): ROW}, who={'claude:' + U1: 'old@example.com', 'codex:' + CX: 'carol@example.com'})),
    ('other-build-with-old', dict(build='x', h={'claude:' + U1: {hour(NOW - 2 * D): ROW}}, old={'h': KEPT, 'orphan': {hour(NOW - 50 * D): ROW}})),
    ('other-build-empty-h', dict(build='x')),
    ('v-2', dict(v=2, h=KEPT)), ('v-true', dict(v=True, h=KEPT)), ('v-float', dict(v=1.0, h=KEPT)), ('no-done', dict(done=..., h=KEPT)), ('done-int', dict(done=1, h=KEPT)),
    ('files-list', dict(files=[], h=KEPT)), ('files-entry-not-dict', dict(files={'/x': 5}, h=KEPT)), ('seen-not-int', dict(seen={'c1': 1.5}, h=KEPT)),
    ('seen-bool', dict(seen={'c1': True}, h=KEPT)), ('seen-big', dict(seen={'c1': 10 ** 30}, h=KEPT)), ('owners-entry-not-list', dict(owners={'s1': {}}, h=KEPT)),
    ('orphan-bad-row', dict(orphan={hour(NOW - 2 * D): [1, 2, 3]}, h=KEPT)), ('h-bad-hour', dict(h={'claude:' + U1: {'12345678': ROW}})),
    ('h-row-negative', dict(h={'claude:' + U1: {hour(NOW - 2 * D): [-1, 0, 0, 0, 0, 0, 0]}})), ('h-row-too-big', dict(h={'claude:' + U1: {hour(NOW - 2 * D): [10 ** 15 + 1, 0, 0, 0, 0, 0, 0]}})),
    ('h-row-max', dict(h={'claude:' + U1: {hour(NOW - 2 * D): [10 ** 15, 0, 0, 0, 0, 0, 0]}})), ('h-row-bool', dict(h={'claude:' + U1: {hour(NOW - 2 * D): [True, 0, 0, 0, 0, 0, 0]}})),
    ('h-row-float', dict(h={'claude:' + U1: {hour(NOW - 2 * D): [1.0, 0, 0, 0, 0, 0, 0]}})), ('h-not-dict', dict(h={'claude:' + U1: []})), ('um-not-dict', dict(um={'x': []}, h=KEPT)),
    ('um-value-not-int', dict(um={'x': {'m': 'y'}}, h=KEPT)), ('who-list', dict(who=[], h=KEPT)), ('build-missing', dict(build=..., h=KEPT)),
]:
    @ccase('old/scan-json-' + name, ['cost-sync', '--json'])
    def _(b, over=over):
        base(b)
        scan_json(b, **over)
        main(b, 's1', lines(owner(), msg(NOW - 3 * H, 'msg_1'), msg(NOW - 40 * D, 'msg_old'), msg(NOW - 3 * H, 'msg_unpriced', 'claude-next-9')), age=30 * D)
        rollout(b, 'rollout-1', lines(meta(NOW - 9 * H), turn(NOW - 9 * H), count(NOW - 8 * H, 2000, 2000)), age=45 * D)


for name, text in [('not-json', '{oops'), ('list', '[]'), ('empty', ''), ('not-utf8', b'\xff\xfe')]:
    @ccase('old/scan-json-file-' + name, ['cost-sync', '--json'])
    def _(b, text=text):
        base(b)
        b.write('.cca/cost/scan.json', text)
        os.chmod(b.path('.cca/cost'), 0o700)
        main(b, 's1', lines(owner(), msg(NOW - 3 * H, 'msg_1')))


@ccase('old/rebuild-from-local-json', ['cost-sync', '--json'])   # scan.json 没了（restore 的备份不带它）：以前算好的从 local.json 接上
def _(b):
    base(b)
    b.write('.cca/cost/local.json', dumps({'v': 1, 'build': 'x', 'who': {'claude:' + U1: 'old@example.com', 'codex:' + CX: 'Carol@Example.com'}, 'acc': KEPT,
                                           'um': {'claude:' + U1: ['m1']}, 'orphan': {hour(NOW - 50 * D): ROW, hour(NOW - 20 * D): ROW}}))
    os.chmod(b.path('.cca/cost'), 0o700)
    main(b, 's1', lines(owner(), msg(NOW - 3 * H, 'msg_1')), age=30 * D)


@ccase('old/rebuild-no-sessions-left', ['cost-sync', '--json'])   # 会话记录一个都没了：以前算的全留着
def _(b):
    base(b)
    scan_json(b, build='x', h=KEPT, orphan={hour(NOW - 50 * D): ROW, hour(NOW - 2 * D): ROW})


# --- 认身份：这台设备上登录过的账号 → 邮箱 ----------------------------------------------------------------------------
for name, acct, master in [
    ('both', {'oauthAccount': {'accountUuid': U1, 'emailAddress': 'Alice@Example.com'}}, {'oauthAccount': {'accountUuid': U2, 'emailAddress': 'bob@example.com'}}),
    ('none', None, None), ('bad-email', {'oauthAccount': {'accountUuid': U1, 'emailAddress': 'not an email'}}, {'oauthAccount': {'accountUuid': U2}}),
    ('short-ident', {'oauthAccount': {'accountUuid': 'short', 'emailAddress': 'a@example.com'}}, {'oauthAccount': {'accountUuid': 'x' * 65, 'emailAddress': 'b@example.com'}}),
    ('not-objects', {'oauthAccount': [1]}, [1]), ('ident-number', {'oauthAccount': {'accountUuid': 12345678, 'emailAddress': 'a@example.com'}}, {'oauthAccount': {'accountUuid': U2, 'emailAddress': 5}}),
    ('same-ident', {'oauthAccount': {'accountUuid': U1, 'emailAddress': 'first@example.com'}}, {'oauthAccount': {'accountUuid': U1, 'emailAddress': 'second@example.com'}}),
]:
    @ccase('who/' + name, ['cost-worker'])
    def _(b, acct=acct, master=master):
        base(b, logged=False, master=False)
        if acct is not None:
            b.write('.cca/accounts/%s/claude/.claude.json' % ALICE, json.dumps(acct))
        if master is not None:
            b.write('.claude.json', json.dumps(master))
        scan_json(b, who={'claude:' + U2: 'kept@example.com'})
        main(b, 's1', lines(owner(), msg(NOW - 3 * H, 'msg_1')))


# --- cost-stop --------------------------------------------------------------------------------------------------------
for name, rec in [('no-record', None), ('not-json', '{oops'), ('wrong-v', {'v': 2, 'pid': 77, 'started': T0_EPOCH}), ('pid-not-int', {'v': 1, 'pid': '77', 'started': T0_EPOCH}),
                  ('started-zero', {'v': 1, 'pid': 77, 'started': 0}), ('started-float', {'v': 1, 'pid': 77, 'started': 1.5}), ('process-gone', {'v': 1, 'pid': 77, 'started': T0_EPOCH}),
                  ('pid-reused', {'v': 1, 'pid': 4242, 'started': T0_EPOCH - 500}), ('pid-bool', {'v': 1, 'pid': True, 'started': T0_EPOCH}), ('pid-float', {'v': 1, 'pid': 77.0, 'started': T0_EPOCH})]:
    @ccase('stop/' + name, ['cost-stop'])
    def _(b, rec=rec):
        base(b)
        b.fake('ps', PS)
        b.write('ps-lstart.txt', ' 4242 Sat Nov  7 12:08:55 2026\n')
        if rec is not None:
            b.write('.cca/cost/worker.json', rec if isinstance(rec, str) else json.dumps(rec))


@ccase('stop/extra-arg', ['cost-stop', 'x'])
def _(b):
    base(b)


def sleeper(b):
    p = subprocess.Popen(['/bin/sleep', '30'], start_new_session=True, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    out = subprocess.run(['/bin/ps', '-o', 'lstart=', '-p', str(p.pid)], capture_output=True, text=True, env={'LC_ALL': 'C', 'LANG': 'C', 'TZ': 'Asia/Shanghai'}).stdout.strip()
    started = calendar.timegm(time.strptime(' '.join(out.split()), '%a %b %d %H:%M:%S %Y')) - 8 * 3600   # ps 是按 Asia/Shanghai 说的
    b.write('.cca/cost/worker.json', json.dumps({'v': 1, 'pid': p.pid, 'started': started}))

    def cleanup():
        alive = p.poll() is None
        p.kill()
        p.wait()
        b.write('.cca/cost/worker.json', 'was alive' if alive else 'stopped')   # 进程号每次不一样：换成“停掉了没有”再比
    return cleanup


@ccase('stop/running-worker', ['cost-stop'], before=sleeper, env={'PATH': '/usr/bin:/bin:/usr/sbin:/sbin'})   # 真的进程：发 SIGTERM，等它退出
def _(b):
    base(b)


# --- 和别的设备同步（op ledger）----------------------------------------------------------------------------------------
SALT = PRESENCE['salt']
FP_ME = 'SHA256:' + base64.b64encode(hashlib.sha256(base64.b64decode(PUB.split()[1])).digest()).decode().rstrip('=')   # 夹具里那把公钥的指纹
FP_A, FP_B = 'SHA256:' + 'A' * 43, 'SHA256:' + 'B' * 43


def remote_blob(email, fp, doc, ident=U1, tool='CLAUDE'):
    """别的设备传到服务器上的那一份"""
    ns = src()
    return ns['cost_seal'](ns['keyshare_keys'](SALT, ns[tool], email, ident, ns['COST_LABEL']), aid_of(email, tool), fp, doc)


def shared(b, cfg=None, uuid=U1, **kw):
    """alice 是映射里的账号、开着设备统计、这台设备登录着"""
    world(b, policy=SHARING, cfg=cfg, **kw)
    b.write('.cca/policy/id_ed25519.pub', PUB, 0o644)
    b.write('.cca/policy/id_ed25519', 'PRIVATE KEY PLACEHOLDER\n')
    b.write('.cca/bin/cca', '#!/bin/sh\nexit 0\n', 0o755)
    for n, s in (('ssh-keygen', KEYGEN), ('curl', CURL)):
        b.fake(n, s)
    if uuid:
        b.write('.cca/accounts/%s/claude/.claude.json' % ALICE, json.dumps({'oauthAccount': {'accountUuid': uuid, 'emailAddress': 'alice@example.com'}}))
    main(b, 's1', lines(owner(U1), msg(NOW - 5 * H, 'msg_1'), msg(NOW - 30 * H, 'msg_2'), msg(NOW - 5 * H, 'msg_3', 'claude-next-9')))


AID = None


def ledger(b, rows=None, nth=1, **top):
    global AID
    AID = AID or aid_of('alice@example.com')
    server(b, 'ledger', dict({'cost': {AID: rows or []}}, **top), nth=nth)


DOC_A = {'v': 1, 'at': NOW - 2 * H, 'h': {hour(NOW - 3 * D): ROW, hour(NOW - 5 * H): ROW}, 'um': ['m-x', 5, 'bad\nname', 'y' * 65]}
DOC_B = {'v': 1, 'at': NOW - 50 * H, 'h': {hour(NOW - 10 * D): ROW}, 'um': []}


@ccase('share/first-sync', ['cost-sync', '--json'])   # 第一次：问服务器（别的设备的取回来）、把这台的传上去
def _(b):
    shared(b)
    ledger(b, [{'fp': FP_A, 'tag': 'tag-a-1', 'name': '书房的 Mac', 'blob': remote_blob('alice@example.com', FP_A, DOC_A)},
               {'fp': FP_B, 'tag': 'tag-b-1', 'name': '旧电脑', 'revoked': True, 'blob': remote_blob('alice@example.com', FP_B, DOC_B)}],
           me_name='这台 Mac', missing={aid_of('alice@example.com'): [{'name': '没传过的设备', 'used_age': 3600.4}, {'name': 5, 'used_age': 1}, {'name': 'x', 'used_age': 'y'}, 'junk']})
    ledger(b, nth=2)


for name, argv in [('text', ['cost-sync']), ('text-now', ['cost-sync', '--now']), ('worker', ['cost-worker'])]:
    @ccase('share/first-sync-' + name, argv)
    def _(b):
        shared(b)
        ledger(b)
        ledger(b, nth=2)


ROWS = [
    ('forged', lambda: {'fp': FP_A, 'tag': 't', 'name': 'x', 'blob': dict(remote_blob('alice@example.com', FP_A, DOC_A), t='AAAA')}),
    ('other-device-fp', lambda: {'fp': FP_B, 'tag': 't', 'name': 'x', 'blob': remote_blob('alice@example.com', FP_A, DOC_A)}),          # 别的设备的密文挪过来：认证不过
    ('other-account', lambda: {'fp': FP_A, 'tag': 't', 'name': 'x', 'blob': remote_blob('bob@example.com', FP_A, DOC_A)}),
    ('other-ident', lambda: {'fp': FP_A, 'tag': 't', 'name': 'x', 'blob': remote_blob('alice@example.com', FP_A, DOC_A, ident=U2)}),
    ('no-blob-no-prev', lambda: {'fp': FP_A, 'tag': 't', 'name': 'x'}),
    ('blob-not-dict', lambda: {'fp': FP_A, 'tag': 't', 'name': 'x', 'blob': 'zzz'}),
    ('no-tag', lambda: {'fp': FP_A, 'name': 'x', 'blob': remote_blob('alice@example.com', FP_A, DOC_A)}),
    ('fp-not-str', lambda: {'fp': 5, 'tag': 't', 'blob': remote_blob('alice@example.com', FP_A, DOC_A)}),
    ('not-a-row', lambda: 'junk'),
    ('name-missing', lambda: {'fp': FP_A, 'tag': 't', 'blob': remote_blob('alice@example.com', FP_A, DOC_A)}),
    ('name-long', lambda: {'fp': FP_A, 'tag': 't', 'name': '很长的名字' * 20, 'blob': remote_blob('alice@example.com', FP_A, DOC_A)}),
    ('name-unprintable', lambda: {'fp': FP_A, 'tag': 't', 'name': 'a\x07b', 'blob': remote_blob('alice@example.com', FP_A, DOC_A)}),
    ('name-empty', lambda: {'fp': FP_A, 'tag': 't', 'name': '', 'blob': remote_blob('alice@example.com', FP_A, DOC_A)}),
    ('revoked-not-true', lambda: {'fp': FP_A, 'tag': 't', 'name': 'x', 'revoked': 1, 'blob': remote_blob('alice@example.com', FP_A, DOC_A)}),
    ('doc-v-2', lambda: {'fp': FP_A, 'tag': 't', 'name': 'x', 'blob': remote_blob('alice@example.com', FP_A, dict(DOC_A, v=2))}),
    ('doc-at-future', lambda: {'fp': FP_A, 'tag': 't', 'name': 'x', 'blob': remote_blob('alice@example.com', FP_A, dict(DOC_A, at=NOW + 3 * D))}),
    ('doc-at-zero', lambda: {'fp': FP_A, 'tag': 't', 'name': 'x', 'blob': remote_blob('alice@example.com', FP_A, dict(DOC_A, at=0))}),
    ('doc-at-int', lambda: {'fp': FP_A, 'tag': 't', 'name': 'x', 'blob': remote_blob('alice@example.com', FP_A, dict(DOC_A, at=int(NOW - 99)))}),
    ('doc-h-bad', lambda: {'fp': FP_A, 'tag': 't', 'name': 'x', 'blob': remote_blob('alice@example.com', FP_A, dict(DOC_A, h={'x': ROW}))}),
    ('doc-um-not-list', lambda: {'fp': FP_A, 'tag': 't', 'name': 'x', 'blob': remote_blob('alice@example.com', FP_A, dict(DOC_A, um='m'))}),
    ('doc-too-many-hours', lambda: {'fp': FP_A, 'tag': 't', 'name': 'x', 'blob': remote_blob('alice@example.com', FP_A, dict(DOC_A, h={str(1700000000 + i * 3600): ROW for i in range(24 * 7 * 20 + 1)}))}),
    ('doc-many-um', lambda: {'fp': FP_A, 'tag': 't', 'name': 'x', 'blob': remote_blob('alice@example.com', FP_A, dict(DOC_A, um=['m%d' % i for i in range(50)]))}),
    ('doc-is-list', lambda: {'fp': FP_A, 'tag': 't', 'name': 'x', 'blob': remote_blob('alice@example.com', FP_A, [1, 2])}),
    ('mine-by-fp', lambda: {'fp': FP_ME, 'tag': 'tag-mine', 'name': 'me'}),
    ('mine-by-flag', lambda: {'fp': FP_A, 'tag': 'tag-mine', 'name': 'me', 'me': True}),
    ('fp-not-ascii', lambda: {'fp': 'SHA256:中文', 'tag': 't', 'name': 'x', 'blob': remote_blob('alice@example.com', FP_A, DOC_A)}),   # 认证里带不了：没人接的 ValueError
]
for name, row in ROWS:
    @ccase('share/row-' + name, ['cost-sync', '--json'])
    def _(b, row=row):
        shared(b)
        ledger(b, [row()])
        ledger(b, nth=2)


# 服务器没成、回的看不懂
for name, kw in [
    ('unreachable', None), ('404-old-server', dict(body={'error': 'no such op'}, code=404)), ('413-old-server', dict(body='', code=413)), ('403', dict(body={}, code=403)),
    ('500', dict(body='oops', code=500)), ('pin-mismatch', dict(body='', rc=90)), ('200-not-json', dict(body='hello')), ('200-cost-not-dict', dict(body={'cost': []})),
    ('200-missing-not-dict', dict(body={'cost': {}, 'missing': [], 'me_name': 5})), ('200-me-name-unprintable', dict(body={'cost': {}, 'me_name': 'a\tb'})),
    ('200-me-name-long', dict(body={'cost': {}, 'me_name': '名' * 100})), ('200-me-name-empty', dict(body={'cost': {}, 'me_name': ''})),
]:
    @ccase('share/server-' + name, ['cost-sync', '--json'])
    def _(b, kw=kw):
        shared(b)
        if kw:
            server(b, 'ledger', nth=1, **kw)
        ledger(b, nth=2)


for name, kw in [('unreachable', None), ('404', dict(body={}, code=404)), ('500', dict(body={}, code=500)), ('429', dict(body={}, code=429))]:
    @ccase('share/put-' + name, ['cost-sync'])
    def _(b, kw=kw):
        shared(b)
        ledger(b)
        if kw:
            server(b, 'ledger', nth=2, **kw)


@ccase('share/sign-fails', ['cost-sync'])
def _(b):
    shared(b)
    b.write('sign_fails', '')


for name, kw in [('not-logged-in', dict(uuid=None)), ('presence-off', dict(cfg={'presence': False})), ('table-unreadable', dict())]:
    @ccase('share/skipped-' + name, ['cost-sync', '--json'])
    def _(b, name=name, kw=kw):
        shared(b, **kw)
        ledger(b)
        if name == 'table-unreadable':
            b.table({'gina@example.com': 'US-B'}, 'agy')
            b.write('.cca/agy.json', '{oops')


def synced_then(change=None, **sync):
    """先同步成一次（Python 跑的），再按 change 改一改，或者把 sync.json 里的时间往回拨"""
    def before(b):
        ledger(b, [{'fp': FP_A, 'tag': 'tag-a-1', 'name': '书房的 Mac', 'blob': remote_blob('alice@example.com', FP_A, DOC_A)}])
        ledger(b, nth=2)
        prime(b, 'cost-sync', '--now')
        for n in os.listdir(b.path('server')):
            os.unlink(b.path('server/' + n))
        for n in ('curl.ledger.n', 'curl.log', 'keygen.log'):
            os.unlink(b.path(n))
        p = b.path('.cca/cost/sync.json')
        with open(p) as f:
            st = json.load(f)
        for k, v in sync.items():
            if k in ('up_at', 'checked', 'tag', 'sum'):
                st['acc']['claude:alice@example.com'][k] = v
            elif v is None:
                st.pop(k, None)
            else:
                st[k] = v
        with open(p, 'w') as f:
            f.write(dumps(st))
        if change:
            change(b)
    return before


def again(b, rows=None):
    ledger(b, rows if rows is not None else [{'fp': FP_A, 'tag': 'tag-a-1', 'name': '书房的 Mac（改名了）'}, {'fp': FP_ME, 'tag': 'PLACEHOLDER', 'name': 'me', 'me': True}])
    ledger(b, nth=2)


for name, argv, before in [
    ('too-soon', ['cost-sync', '--json'], synced_then()),                                             # 刚同步过：不问服务器
    ('too-soon-text', ['cost-sync'], synced_then()),
    ('too-soon-now', ['cost-sync', '--now', '--json'], synced_then(again)),                           # --now 不等
    ('due', ['cost-sync', '--json'], synced_then(again, at=NOW - 700)),                               # 到时候了：带着已经有的认证码问，没变的不回密文
    ('due-clock-went-back', ['cost-sync', '--json'], synced_then(again, at=NOW + 5000)),              # 上次同步在“未来”：不当真
    ('due-beat', ['cost-sync', '--json'], synced_then(again, at=NOW - 700, up_at=NOW - 13 * H)),      # 内容没变也隔 12 小时重传一次
    ('due-new-usage', ['cost-sync', '--json'], synced_then(lambda b: (again(b), append(b.path('.claude/projects/-Users-me-proj/s1.jsonl'), lines(msg(NOW - 1 * H, 'msg_9')), age=D)), at=NOW - 700)),
    ('due-device-gone', ['cost-sync', '--json'], synced_then(lambda b: again(b, []), at=NOW - 700)),   # 服务器上没有别的设备了：remote.json 里也去掉
    ('due-server-lost-mine', ['cost-sync', '--json'], synced_then(lambda b: again(b, [{'fp': FP_A, 'tag': 'tag-a-1', 'name': 'x'}]), at=NOW - 700)),
    ('due-device-updated', ['cost-sync', '--json'], synced_then(lambda b: again(b, [{'fp': FP_A, 'tag': 'tag-a-2', 'name': 'x', 'blob': remote_blob('alice@example.com', FP_A, DOC_B)}]), at=NOW - 700)),
    ('error-waiting', ['cost-sync', '--json'], synced_then(state='error', error='上次没成的原因')),
    ('old-server-waiting', ['cost-sync', '--json'], synced_then(state='old_server', error='x', at=NOW - 3 * H, retry=NOW + 3 * H)),
    ('old-server-retry-due', ['cost-sync', '--json'], synced_then(again, state='old_server', error='x', at=NOW - 7 * H, retry=NOW - 1 * H)),
    ('retry-too-far', ['cost-sync', '--json'], synced_then(again, state='old_server', error='x', at=NOW - 7 * H, retry=NOW + 30 * H)),   # 再试的时间在 6 小时以外（时钟改过）：不等
    ('never-ok', ['cost-sync', '--json'], synced_then(ok_at=None)),
]:
    @ccase('share/again-' + name, argv, before=before)
    def _(b):
        shared(b)


@ccase('share/big-ledger', ['cost-sync'], timeout=120)   # 小时很多：密文超过上限就去掉最早的四分之一，直到放得下
def _(b):
    shared(b)
    rows = {}
    import random
    rnd = random.Random(7)
    for i in range(14 * 7 * 24 - 30):
        rows[str(int((NOW - 14 * W) // 3600 * 3600) + (i + 30) * 3600)] = [rnd.randrange(10 ** 12), rnd.randrange(10 ** 6), rnd.randrange(10 ** 9), rnd.randrange(10 ** 9), rnd.randrange(10 ** 9), rnd.randrange(10 ** 9), 0]
    b.write('.cca/cost/scan.json', dumps({'v': 1, 'build': '2026-11-13/1', 'done': True, 'files': {}, 'seen': {}, 'owners': {}, 'h': {'claude:' + U1: rows}, 'orphan': {}, 'um': {}, 'who': {}}))
    os.chmod(b.path('.cca/cost'), 0o700)
    ledger(b)
    ledger(b, nth=2)
