#!/usr/bin/env python3
"""生成 cli/tests/vectors/usage.json：cca 里的 PY_SRC（用量、套餐、Codex 登录身份那一块）对一批输入给出的结果，
cli/src/cmd/usage.rs、codex.rs 的单元测试照着核对（cargo test 读这个文件，不用装 Python）。

    python3 -I cli/tools/gen_usage_vectors.py cli/tests/vectors

只能调内部函数、对拍（pydiff）够不着的逻辑才放这里：时间的写法、套餐的叫法和档位、取整、每周用量的统计、菜单里的一格、
id_token 的声明、json.loads(字节) 认编码。结果和时区无关（要本地时间的函数这里不调）。切换到只有 Rust 以后 PY_SRC 就没有了，
这份数据留作对照。"""
import base64
import json
import os
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
with open(os.path.join(REPO, 'cca'), encoding='utf-8') as f:
    DEFINITIONS = f.read().split("PY_SRC=$(cat <<'PY'\n", 1)[1].split('\nPY\n)', 1)[0].split('\nif len(sys.argv) < 2', 1)[0]
home = tempfile.mkdtemp(prefix='cca-vectors-')
os.makedirs(os.path.join(home, '.cca'))
os.environ.update({'HOME': home, 'CCA_HOME': os.path.join(home, '.cca'), 'CCA_TOOL': 'claude'})
ns = {}
exec(compile(DEFINITIONS, 'cca', 'exec'), ns)
NAN, INF = float('nan'), float('inf')


def attempt(f, *a):
    """调用的结果；出了异常记下是哪一类"""
    try:
        return f(*a)
    except Exception as e:   # noqa: BLE001
        return {'raises': type(e).__name__}


def number(x):
    """数字的写法（NaN、无穷大 JSON 里写不了，用文字）"""
    return repr(x)


out = {}
secs = [-5, 0, 0.0, -0.0, 59.9, 60, 61, 119.99, 120.0, 3599, 3600, 3660, 7199.5, 86399, 86400, 90000, 3 * 86400 + 4 * 3600 + 59, 7 * 86400, 1e9, 59.999999999999, 1e15, 5e18,
        119.99999999999999, 7199.999999999999, 179.99999999999997, 5.3e20, 5.4e20, 5.6e20, 1e25, 86400e22, 1e300, 1.7976931348623157e308, NAN, INF, -INF]
out['span'] = [[number(s), attempt(ns['span'], s), attempt(ns['span_short'], s), attempt(ns['ago'], s)] for s in secs]
stamps = [0, 1795058400, 1795058400.9, -1, -0.5, 253402300799, 253402300800, 1e12, 1e17, 1e19, NAN, INF]
out['iso'] = [[number(s), attempt(ns['iso'], s)] for s in stamps]
out['weekday_clock'] = [[wd, h, m, ns['weekday_clock'](time.struct_time((2026, 11, 19, h, m, 0, wd, 323, 0)))] for wd, h, m in [(0, 0, 0), (1, 9, 5), (2, 23, 59), (3, 12, 30), (4, 1, 1), (5, 10, 10), (6, 7, 0)]]
out['reset_short'] = [[number(at), number(now), attempt(ns['reset_short'], at, now)] for at, now in
                      [(7985, 0), (30, 0), (1795058400 + 1505, 1795058400), (0, 100), (86400 * 3 + 5, 0), (3600, 0), (3599.9, 0), (NAN, 0), (INF, 0), (-INF, 0), (5e18, 0)]]

oas = [
    {}, {'planDisplayName': 'Max 20x'}, {'planDisplayName': 'Claude Max  5x', 'organizationType': 'claude_max'}, {'planDisplayName': 'claude team premium', 'organizationType': 'claude_team'},
    {'planDisplayName': 'Ultra', 'organizationType': 'claude_max'}, {'planDisplayName': 'Max\n'}, {'planDisplayName': '_x'}, {'planDisplayName': 5}, {'planDisplayName': 'A' + 'b' * 39},
    {'planDisplayName': 'A' + 'b' * 40}, {'planDisplayName': 'x+'}, {'planDisplayName': 'x.'}, {'planDisplayName': '专业版'}, {'planDisplayName': 'é'}, {'planDisplayName': '٣x'},
    {'organizationType': 'claude_max', 'organizationRateLimitTier': 'default_claude_max_20x'}, {'organizationType': 'claude_max', 'organizationRateLimitTier': 'default_claude_max_5x'},
    {'organizationType': 'claude_max'}, {'organizationType': 'claude_max', 'userRateLimitTier': 'x 100x'}, {'organizationType': 'claude_max', 'organizationRateLimitTier': '20xy'},
    {'organizationType': 'claude_max', 'organizationRateLimitTier': 'a 5x b', 'userRateLimitTier': '20x'}, {'organizationType': 'claude_max', 'organizationRateLimitTier': '0x'},
    {'organizationType': 'claude_max', 'organizationRateLimitTier': '19x'}, {'organizationType': 'claude_max', 'organizationRateLimitTier': '9' * 25 + 'x'},
    {'organizationType': 'claude_team', 'organizationRateLimitTier': 'default_claude_team_5x'}, {'organizationType': 'claude_team'}, {'organizationType': 'claude_team', 'organizationRateLimitTier': '4x'},
    {'organizationType': 'claude_team', 'planDisplayName': '!bad', 'organizationRateLimitTier': 'x'}, {'organizationType': 'claude_pro'}, {'organizationType': 'claude_enterprise'},
    {'organizationType': 'claude_free'}, {'organizationType': 'claude_edu'}, {'organizationType': 'claude_education_plus'}, {'organizationType': 'CLAUDE_MAX'}, {'organizationType': 'claude_'},
    {'organizationType': 'claude_new thing'}, {'organizationType': 'api'}, {'organizationType': 5}, {'organizationType': True}, {'organizationType': ['claude_max']},
    {'organizationType': 'claude_ßtrasse'}, {'organizationType': 'claude_ǆungla'}, {'organizationType': 'claude_ქართული'}, {'organizationType': 'claude_σίσυφος'},
    {'planDisplayName': '  MAX   20X ', 'organizationType': 'claude_max'}, {'planDisplayName': 'Team', 'organizationType': 'claude_weird'}, {'planDisplayName': 'Claude Free'},
    {'planDisplayName': 'Max 5x'}, {'planDisplayName': 'Team Premium'}, {'planDisplayName': 'Enterprise', 'organizationType': None},
]
out['plans'] = []
for oa in oas:
    for sub in [None, 'pro', 'MAX', 5, 'ΑΣ']:
        kind, x = ns['claude_plan_parts'](oa, sub)
        # 倍数的位数多到放不下 64 位时 Rust 这边记成最大的那个数：只拿来比大小
        out['plans'].append([oa, sub, ns['plan_label'](oa, sub), kind, min(x, 2 ** 63 - 1), ns['claude_plan_tier'](oa, sub)])
out['outdated'] = [[oa, rec, ns['claude_login_plan_outdated'](oa, rec)] for oa in oas if 'organizationType' in oa for rec in ['pro', 'max', 'team', 'enterprise', 'free', 'PRO', '']]
plans = sorted(ns['CODEX_PLANS']) + ['', None, 'Pro', '  PLUS ', 'brand_new', 'ßx', 'ǆ', 5, 0, True, 'ΑΣ', 'ქართული', 'x y', 'İstanbul']
out['codex_plans'] = [[p, ns['codex_plan_label'](p), ns['codex_plan_tier'](p)] for p in plans]
out['agy_tiers'] = {k: list(v) for k, v in ns['AGY_TIERS'].items()}
out['capitalize'] = [[chr(c), chr(c).capitalize()] for c in range(0x110000) if not 0xd800 <= c < 0xe000 and chr(c).capitalize() != chr(c).upper()]
out['capitalize_words'] = [[w, w.capitalize()] for w in ['', 'a', 'abc', 'ABC', 'aBC dEF', 'éa', '1abc', 'ßa', 'ǆUNGLA', 'ΑΣ', 'ΣΑΣ', 'ΑΣ Σ', 'İSTANBUL', 'ﬁne', 'ქართული', '中文', 'x_y', 'ǅX', 'ᾳ', 'ŉa']]

lines = [b'{"a": 1}', b'\xef\xbb\xbf{"a": 1}', b'\xef\xbb\xbf\xef\xbb\xbf{"a": 1}', b'', b'  ', b'[1]', b'x', b'{"a": "\xff"}', '{"中": "文"}'.encode(), '{"a": 1}'.encode('utf-16-le'), '{"a": 1}'.encode('utf-16-be'),
         '{"a": 1}'.encode('utf-16'), b'\xfe\xff' + '{"a": 1}'.encode('utf-16-be'), '{"a": 1}'.encode('utf-32-le'), '{"a": 1}'.encode('utf-32-be'), '{"a": 1}'.encode('utf-32'),
         b'\x00\x00\xfe\xff' + '{"a": 1}'.encode('utf-32-be'), '{"a": 1}\n'.encode('utf-16-be'), '{"a": 1}\n'.encode('utf-16-le'), b'1\x00', b'\x001', b'12', b'1', b'1\x00\x00\x00', b'\x00\x00\x001',
         b'{"a": 1}\n', b'{"a": 1}\r\n', b'\x00{"a": 1}', b'{\x00"a": 1}', '"\U0001f600"'.encode('utf-16-be'), '"\U0001f600"'.encode('utf-16-le')[:-3] + b'\x00', b'\xff\xfe', b'\xfe\xff\x00', b'null', b'NaN',
         b'{"a": 1} ', b'\t{"a": 1}', b'1\x002', b'\xff\xfe\x00\x00', b'\xff\xfe1\x00', b'\x00\x00\xfe\xff\x00\x00\x00', b'1\x00\x00', b'\x00\x00\x00']


def loads(b):
    """json.loads(字节)：值再写成文字，或者 ValueError"""
    try:
        return json.dumps(json.loads(b), ensure_ascii=False)
    except ValueError:
        return None


out['json_bytes'] = [[list(b), loads(b)] for b in lines]

rounds = [0, 1, -1, 0.5, 1.5, 2.5, -0.5, -1.5, 13.4, 13.5, 14.5, 69.5, 89.5, 99.5, 100, 1e30, -1e30, 12345678901234567890123, 9007199254740993, 8.999e18, 9.1e18, 9.3e18, -9.3e18, 1e300, 2.5e15,
          4503599627370497.5, -0.4]
out['round'] = [[json.dumps(x), '%d' % round(x), '%3d%%' % round(x), round(x) >= 90, round(x) >= 70] for x in rounds]
counts = [0, 1, 2, -1, 2.9, 0.9, -0.5, 1e30, -1e30, 10 ** 30, -10 ** 30, True, '2', None, 1.0, 9.3e18, -9.3e18]
out['reset_count'] = [[json.dumps(n), (lambda r: None if r is None else [str(r), not r, r > 1])(ns['reset_count']({'reset_credits': {'availableCount': n}}))] for n in counts]
out['floordiv'] = [[number(x), number(x // 60)] for x in [0.0, 59.9, 60.0, 119.99999999999999, 120.0, 1e18, 7199.999999999999, 3.0e-320, 1e308, 59.99999999999999, 179.99999999999997, 6e15 + 0.5]]
# 两个整数的真除法（算平均用量）：被除数很大时也是离真值最近的那个小数
pairs = [(0, 3), (7, 2), (-7, 2), (100, 3), (200000000000000396, 3), (2 ** 53, 3), (2 ** 53 + 1, 3), (2 ** 53 + 1, 1), (2 ** 54 + 2, 1), (2 ** 54 + 6, 1), (2 ** 54 + 3, 1), (2 ** 60 + 1, 7),
         (-(2 ** 62 + 3), 5), (12 * 10 ** 18, 12), (2 ** 64 + 2 ** 11, 2), (2 ** 64 + 2 ** 11 + 1, 2), (24 * 10 ** 18 - 1, 12), (2 ** 55 + 4, 8), (2 ** 55 + 12, 8), (2 ** 55 + 5, 8),
         (3 * 2 ** 53 + 3, 3), (9007199254740993 * 11, 11), (9007199254740995 * 7 + 3, 7), (2 ** 53 - 1, 12), (10 ** 17, 9)]
out['true_div'] = [[str(n), d, number(n / d)] for n, d in pairs]
# 登录等了几分钟（login-flow 超时的那句话）：'%d' % (int(float(超时)) // 60)
out['whole_minutes'] = [[number(x), attempt(lambda x: '%d' % (int(x) // 60), x)] for x in
                        [900.0, 0.5, -0.5, 59.9, 60.0, 125.9, -60.0, -61.0, -90.0, 8.9e18, -8.9e18, 9.1e18, -9.1e18, 9.3e18, -9.3e18, 1e300, -1e300, -6e19, -5.99999e19, 1.2e21, -1.2e21,
                         -(2.0 ** 70) * 60, -(2.0 ** 70) * 60 - 2.0 ** 30, NAN, INF, -INF]]
ts = ['2026-11-19T08:00:00Z', '2026-11-19 08:00', ' 2026-11-19T08:00:00.25+08:00 ', '2026-11-19T08:00:00-0530', 1795075200, 1795075200123, 1.5, True, None, 'x', [1], 1e11, 1e11 + 1, -5, 0]
out['win'] = []
for now in (0, 1795075200, 1795075201):
    for w in [None, 5, {}, {'utilization': 5}, {'utilization': True}, {'utilization': '5'}, {'utilization': 5.5, 'resets_at': None}] + [{'utilization': 1, 'resets_at': t} for t in ts]:
        out['win'].append([w, now, ns['win'](w, now)])

# 每周用量给 App 的样子（usage_weeks_view）：weeks 是 usage-weeks.json 的内容，cached 是 usage_cached 的结果
KEY, W, D, NOW = 'alice@example.com', ns['WEEK'], 86400, 1_800_000_000.0


def week(end, peak, carried=0, resets=0, first=None, last=None):
    return {'end': end, 'peak': peak, 'carried': carried, 'resets': resets, 'first': end - W + 3600 if first is None else first, 'last': end - 600 if last is None else last}


def weeks_view(doc, now, cached):
    p = ns['weeks_file'](KEY)
    if doc is None:
        if os.path.exists(p):
            os.remove(p)
    else:
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, 'w') as f:
            json.dump(doc, f)
    v = attempt(ns['usage_weeks_view'], KEY, now, cached)
    return v if isinstance(v, dict) and 'raises' in v else json.dumps(v)


int_now = int(NOW)
views = [
    (None, NOW, None), ({'version': 1, 'weeks': []}, NOW, None),
    ({'weeks': [week(NOW - 13 * W, 90), week(NOW - 3 * W, 99.5), week(NOW - 2 * W, 30, last=NOW - 2 * W - 2 * D), week(NOW - W, 60, carried=70, resets=1), week(NOW + 2 * D, 20, last=NOW - 60)]}, NOW, None),
    ({'weeks': [week(NOW - 13 * W, 90), week(NOW + 2 * D, 20, last=NOW - 60)]}, NOW, {'at': NOW, 'rl': {'seven_day': {'utilization': 35, 'resets_at': NOW + 2 * D}}}),
    ({'weeks': [week(NOW + 3 * D, 80, first=NOW - 4 * D, last=NOW - 2 * D), week(NOW + 6 * D, 5, first=NOW - D, last=NOW - 60)]}, NOW, None),
    ({'weeks': [week(NOW - 20 * W, 50)]}, NOW, None),
    (None, NOW, {'at': NOW - 60, 'rl': {'seven_day': {'utilization': 12.34, 'resets_at': NOW + D}}}),
    (None, NOW, {'at': NOW - 60, 'rl': {'seven_day': {'utilization': 12, 'resets_at': NOW - D}}}),
    (None, NOW, {'at': NOW - 60, 'rl': {}}),
    ({'weeks': [week(int_now - 3 * W, 99, 1, 2), week(int_now - 2 * W, 49), week(int_now - W, 50), week(int_now + D, 100, last=int_now - 5)]}, NOW, None),
    ({'weeks': [week(int_now - W, 99.44), week(int_now - 2 * W, 99.45), week(int_now - 3 * W, 0.5), week(int_now - 4 * W, 1.5), week(int_now - 5 * W, 2.5), week(int_now - 6 * W, 12.25, 0.1)]}, NOW, None),
    ({'weeks': [week(NOW - W, 100, last=NOW - W - 2 * D), week(NOW - 2 * W, 99.9, last=NOW - 2 * W - 2 * D), week(NOW - 3 * W, 50, last=NOW - 3 * W - D), week(NOW - 4 * W, 50, last=NOW - 4 * W - D - 1)]}, NOW, None),
    ({'weeks': [week(NOW - 12 * W, 10), week(NOW - 12 * W + 1, 11), week(NOW - 12 * W - 1, 12)]}, NOW, None),
    ({'weeks': [week(NOW + D, 10, first=NOW - 3 * D, last=NOW - D), week(NOW + 2 * D, 20, first=NOW - 2 * D, last=NOW - 60), week(NOW + 9 * D, 30, first=NOW - 90, last=NOW - 30)]}, NOW, None),
    ({'weeks': [week(NOW + D, 10, first=NOW - 3 * D, last=NOW + 5 * D), week(NOW + 2 * D, 20, first=NOW - 2 * D, last=NOW - 60)]}, NOW, None),
    ({'weeks': [week(int_now + D, 10, first=int_now - 3 * D, last=int_now - D), week(int_now + 2 * D, 20, first=int_now - 2 * D, last=int_now - 60)]}, NOW, None),
    ({'weeks': [week(NOW - W, 150, carried=250.26, resets=3), week(NOW - 2 * W, -5), week(NOW - 3 * W, 1e17, 1e17)]}, NOW, None),
    ({'weeks': [week(NOW + D, 10)]}, NOW + D, None), ({'weeks': [week(NOW + D, 10)]}, int_now + D + 1, None),
    ({'weeks': [{'end': NOW + D, 'peak': 5, 'first': 7, 'last': 9}, 'x', {'end': NOW, 'peak': True, 'first': 1, 'last': 1}, {'end': NOW - 5, 'peak': 2, 'first': 3.5, 'last': 4, 'carried': 'x', 'resets': 2.9}]}, NOW, None),
    ([1], NOW, None), ({'weeks': 'x'}, NOW, None),
    ({'weeks': [week(NOW + D, 30.0, last=NOW - 100)]}, NOW, {'at': NOW - 50, 'rl': {'seven_day': {'utilization': 20, 'resets_at': NOW + D + 10}}}),
    ({'weeks': [week(NOW + D, 30.0, last=NOW - 100)]}, NOW, {'at': NOW - 500, 'rl': {'seven_day': {'utilization': 99, 'resets_at': NOW + D}}}),
    ({'weeks': [week(NOW + D, 30.0, last=NOW - 100)]}, NOW, {'at': NOW - 50, 'rl': {'seven_day': {'utilization': 24.9, 'resets_at': NOW + D}}}),
    ({'weeks': [week(NOW + D, 30.0, last=NOW - 100)]}, NOW, {'at': NOW - 50, 'rl': {'seven_day': {'utilization': 25, 'resets_at': NOW + D}}}),
    ({'weeks': [week(NOW + D, 30, last=NOW - 100)]}, NOW, {'at': NOW - 50, 'rl': {'seven_day': {'utilization': 30, 'resets_at': NOW + D}}}),
]
out['weeks_view'] = [[doc, number(now), cached, weeks_view(doc, now, cached)] for doc, now, cached in views]

# 菜单里的一格（usage_cell）：u 是 usage_cached 的结果
cells = [
    (None, 100.0), ({'at': 1000.0, 'rl': {}}, 1000.0), ({'at': 1000.0, 'rl': {'five_hour': {'utilization': 13.4, 'resets_at': 5000}, 'seven_day': {'utilization': 41, 'resets_at': None}}}, 1000.0),
    ({'at': 1000.0, 'rl': {'five_hour': {'utilization': 13.5, 'resets_at': 900}, 'seven_day': {'utilization': 14.5, 'resets_at': 5000}}}, 1599.9),
    ({'at': 1000.0, 'rl': {'five_hour': {'utilization': 0.5, 'resets_at': 5000}}}, 1600.0), ({'at': 1000.0, 'rl': {'seven_day': {'utilization': 100, 'resets_at': 1e12}}}, 1000.0 + 3 * 3600 + 59),
    ({'at': 1000.0, 'rl': {'seven_day': {'utilization': 250, 'resets_at': 1e12}}}, 1000.0 + 3 * 86400), ({'at': 5000.0, 'rl': {'seven_day': {'utilization': -3, 'resets_at': 1e12}}}, 1000.0),
    ({'at': 1000.0, 'rl': {'five_hour': {'utilization': True}, 'seven_day': 'x'}}, 9000.0), ({'at': 1000.0, 'rl': {'five_hour': {'utilization': 1e30, 'resets_at': 1e12}}}, 1000.0),
    ({'at': 1000.0, 'rl': {'five_hour': {'utilization': NAN, 'resets_at': 1e12}}}, 1000.0), ({'at': 1000.0, 'rl': {'five_hour': {'utilization': 1, 'resets_at': '2026-13-01T00:00:00Z'}}}, 1000.0),
    ({'at': NAN, 'rl': {'five_hour': {'utilization': 1}}}, 1000.0), ({'at': -INF, 'rl': {'five_hour': {'utilization': 1}}}, 1000.0), ({'at': 1000.0, 'rl': {'five_hour': {'utilization': 1}}}, 1000.0 + 600),
]


def cell(u, now):
    ns['usage_cached'] = lambda key: u
    return attempt(ns['usage_cell'], KEY, now)


out['usage_cell'] = [[json.dumps(u), number(now), cell(u, now)] for u, now in cells]


# Codex 登录的身份（codex_claims）：auth.json 的内容 → 声明
def b64(d):
    return base64.urlsafe_b64encode(d if isinstance(d, bytes) else json.dumps(d).encode()).decode().rstrip('=')


def claims(doc):
    p = os.path.join(home, 'auth.json')
    with open(p, 'w') as f:
        json.dump(doc, f)
    return ns['codex_claims'](None, p)


auth = {'chatgpt_plan_type': 'pro', 'chatgpt_account_id': 'ws-1', 'chatgpt_user_id': 'user-1', 'user_id': 'u-2'}
payloads = [
    {'email': 'a@example.com', 'https://api.openai.com/auth': auth}, {'https://api.openai.com/profile': {'email': 'p@example.com'}, 'https://api.openai.com/auth': {'user_id': 'u-2'}},
    {'email': '', 'https://api.openai.com/profile': {'email': 7}}, {'email': 5}, {'email': ['x']}, {}, [1], 'text', None, {'https://api.openai.com/auth': 'x', 'https://api.openai.com/profile': [1]},
    {'email': 'a@example.com', 'https://api.openai.com/auth': {'chatgpt_plan_type': 5, 'chatgpt_account_id': ['w'], 'chatgpt_user_id': 0, 'user_id': True}},
    {'email': 'a@example.com', 'https://api.openai.com/auth': {'chatgpt_plan_type': '', 'chatgpt_account_id': None, 'chatgpt_user_id': '', 'user_id': ''}}, {'email': '用户@example.com'},
]
tokens = [{'tokens': {'id_token': 'h.%s.s' % b64(p)}} for p in payloads]
tokens += [{'tokens': {'id_token': t}} for t in ['', 'a.b', 'a.b.c.d', 'h.e30.s', 'h..s', 'h.e30x.s', 'h.e.s', 'h.!!!.s', 'h.邮箱.s', 5, None, ['h.e30.s'],
                                               'h.%s.s' % b64(b'not json'), 'h.%s.s' % b64(b'{"email": "\xff"}'), 'h.%s.s' % b64('﻿{"email": "x@example.com"}'.encode()),
                                               'h.%s==.s' % b64({'email': 'padded@example.com'}), 'h.%s.s' % '*'.join(b64({'email': 'junk@example.com'})), 'h.%s.s' % b64({'email': 'safe@example.com', 'p': '>>>???~~~'})]]
tokens += [{}, [1], 'x', None, {'tokens': None}, {'tokens': [1]}, {'tokens': {}}, {'OPENAI_API_KEY': 'not-a-real-key', 'tokens': None}]
out['codex_claims'] = [[doc, claims(doc)] for doc in tokens]

target = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(HERE), 'tests', 'vectors')
with open(os.path.join(target, 'usage.json'), 'w', encoding='utf-8') as f:
    json.dump(out, f, ensure_ascii=False, separators=(',', ':'))
    f.write('\n')
print({k: len(v) for k, v in out.items()})
