#!/usr/bin/env python3
"""cca usage 的表格（一个账号一行、不超过终端的宽度）、周结算提醒（use_first）、API key 的掩码、每周用量（usage_weeks）：
临时 HOME、cca 的辅助程序（cca-core usage-render / secret-set / gui-state），不联网，不碰钥匙串。

只能在进程内测的几处在辅助程序自己的单元测试里（cargo test），下面各留了一行说它在哪。"""
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import unicodedata
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from helper_bin import HELPER

FAR = '2099-01-01T00:00:00Z'
WEEK = 7 * 86400   # cli/src/cmd/usage.rs 的同名常量


def dw(s):
    """在终端里占几格（中文、全角字符算 2 格）"""
    return sum(2 if unicodedata.east_asian_width(ch) in 'WF' else 1 for ch in s)


def ident(email):
    """账号目录的名字：邮箱里字母数字以外的换成 -（最多 20 个字符），加上邮箱摘要的前 6 位"""
    s = re.sub('[^a-z0-9]+', '-', email.lower()).strip('-')[:20].strip('-') or 'acct'
    return '%s-%s' % (s, hashlib.sha1(email.encode()).hexdigest()[:6])


def email_of(key):
    return key.split('(')[-1].rstrip(')')


def id_token(claims):
    """Codex 的 auth.json 里的 id_token（只有中间那一段声明有内容；编的，不是真的 token）"""
    return 'e30.%s.sig' % base64.urlsafe_b64encode(json.dumps(claims).encode()).decode().rstrip('=')


class UsageView(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-usage-')
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.cca = self.home / '.cca'
        self.cca.mkdir()
        self.out = self.home / 'out'   # 后台查询的结果（bash 的 cmd_usage 放在一个临时目录里）：第 N 个账号的在 N.json / N.skip
        self.out.mkdir()
        (self.home / 'bin').mkdir()
        # _gui state 要看进程：一个都没有。真的钥匙串绝不能碰：key 存成文件（CCA_KEYSTORE=file），security 换成只会报错的
        self.fake('ps', 'exit 0\n')
        self.fake('security', 'echo "测试不碰钥匙串：security $*" >> "$HOME/security.log"\nexit 1\n')
        self.write(self.cca / 'config.json', {})
        self.env = {'HOME': str(self.home), 'CCA_HOME': str(self.cca), 'CCA_TOOL': 'claude', 'PATH': '%s:/usr/bin:/bin' % (self.home / 'bin'),
                    'CCA_KEYSTORE': 'file', 'CCA_COST': 'off'}
        if 'TZ' in os.environ:
            self.env['TZ'] = os.environ['TZ']
        self.now = time.time()

    def tearDown(self):
        self.assertFalse((self.home / 'security.log').exists(), '调到了 security（钥匙串）')

    def fake(self, name, script):
        p = self.home / 'bin' / name
        p.write_text('#!/bin/sh\n' + script)
        p.chmod(0o755)

    def write(self, path, data, mode=None):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(data if isinstance(data, str) else json.dumps(data, ensure_ascii=False))
        if mode is not None:
            path.chmod(mode)
        return path

    def run_helper(self, *argv, stdin='', **env):
        return subprocess.run([HELPER] + [str(a) for a in argv], env=dict(self.env, **env), input=stdin, capture_output=True, text=True, timeout=60)

    def acct(self, key, tool='claude'):
        """账号目录"""
        return self.cca / {'claude': 'accounts', 'codex': 'codex/accounts'}[tool] / ident(email_of(key))

    def iso(self, secs):
        return time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime(self.now + secs))

    def rl(self, five, week, **extra):
        """一份用量：5 小时的窗口 2 小时 13 分半后重置，本周的 3 天 5 小时后"""
        rl = {'five_hour': {'utilization': five, 'resets_at': self.iso(2 * 3600 + 13 * 60 + 30)},
              'seven_day': {'utilization': week, 'resets_at': self.iso(3 * 86400 + 5 * 3600)}}
        rl.update(extra)
        return rl

    def login(self, key, uuid=None, cached=None, **oa):
        """账号目录里 Claude Code 的 .claude.json：登录的是谁、什么套餐；cached 是 Claude Code 自己存的用量（cachedUsageUtilization）"""
        who = {'emailAddress': email_of(key), 'accountUuid': uuid or 'uuid-' + email_of(key).split('@')[0], 'organizationType': 'claude_max',
               'organizationRateLimitTier': 'default_claude_max_5x'}
        who.update(oa)
        j = {'oauthAccount': {k: v for k, v in who.items() if v is not None}}
        if cached is not None:
            j['cachedUsageUtilization'] = cached
        return self.write(self.acct(key) / 'claude' / '.claude.json', j)

    def result(self, i, rate_limits, sub='max'):
        """后台查到了：usage-query 打印的那一行"""
        (self.out / ('%d.skip' % i)).unlink(missing_ok=True)
        self.write(self.out / ('%d.json' % i), json.dumps({'ok': True, 'usage': {'subscription_type': sub, 'rate_limits_available': True,
                                                                                 'rate_limits': rate_limits}}) + '\n')

    def skip(self, i, why, msg=''):
        """没查（bash 跳过了）、没查成：原因"""
        (self.out / ('%d.json' % i)).unlink(missing_ok=True)
        self.write(self.out / ('%d.skip' % i), '%s\t%s\n' % (why, msg))

    def cache(self, key, cache, tool='claude'):
        """cca 上次存的 usage.json（None：没有）"""
        p = self.acct(key, tool) / 'usage.json'
        if cache is None:
            p.unlink(missing_ok=True)
        else:
            self.write(p, cache)

    def render(self, keys, mode='table', cols=120, rc=0):
        """cca usage 最后那一步：把后台查询的结果整理成表格（各行）或 JSON（各账号的条目）"""
        r = self.run_helper('usage-render', self.out, mode, cols, '', *keys)
        self.assertEqual(r.returncode, rc, r.stderr)
        self.assertEqual(r.stderr, '')
        return json.loads(r.stdout)['accounts'] if mode == 'json' else r.stdout.splitlines()

    # --- 表格 ---
    RICH = [('工作', 'bob@example.com'), ('', 'alice@example.com'), ('副号', 'carol@example.com'), ('', 'dave@example.com')]

    def rich(self):
        """四个官方账号：查到了的（Ultra，备注两项）、查到了但 Claude Code 记的套餐过时的（Max，记的是 Pro）、出口端口不通拿 23 分钟前的
        缓存顶上的（Pro）、没登录的；外加一个不查的第三方 API 账号。返回这四个账号的键"""
        self.now = time.time()
        keys = ['%s(%s)' % (a, e) if a else e for a, e in self.RICH]
        self.write(self.cca / 'accounts.json', dict(dict.fromkeys(keys, 'US-A'), kimi={'base_url': 'https://api.example.com/anthropic'}))
        weekly = {'seven_day_sonnet': {'utilization': 60, 'resets_at': FAR},
                  'model_scoped': [{'display_name': 'Fable', 'utilization': 34, 'resets_at': FAR}]}
        self.login(keys[0], organizationRateLimitTier='default_claude_max_20x')
        self.result(0, self.rl(13, 41, **weekly))
        self.login(keys[1])
        self.result(1, self.rl(98, 72), sub='pro')
        self.login(keys[2], organizationType='claude_pro', organizationRateLimitTier=None)
        self.skip(2, 'port', '7902')
        self.cache(keys[2], {'fetchedAtMs': int((self.now - 23 * 60 - 30) * 1000), 'accountUuid': 'uuid-carol', 'utilization': self.rl(7, 33)})
        self.skip(3, 'login')
        return keys

    def table(self, cols):
        return self.render(self.rich(), cols=cols)

    def test_one_line_per_account_never_wider_than_terminal(self):
        keys = self.rich()
        for cols in range(40, 201):
            with self.subTest(cols=cols):
                lines = self.render(keys, cols=cols)
                self.assertLessEqual(max(dw(l) for l in lines), max(cols, 40) - 2)
                for alias, email in self.RICH:   # 每个账号正好一行（别名、没有别名的邮箱总有一个看得到）
                    self.assertEqual(sum(1 for l in lines if (alias or email) in l and not l.lstrip().startswith('!')), 1)

    def test_wide_terminal_keeps_everything(self):
        lines = self.table(160)
        self.assertRegex(lines[0], r'^  账号 +套餐 +5 小时/重置 +本周/重置 +备注$')
        self.assertRegex(lines[1], r'^  工作  bob@example.com +Ultra +13% 2小时13分后 +41% 周. \d\d:\d\d  Sonnet 本周 60%；Fable 本周 34%$')
        self.assertRegex(lines[3], r'出口端口 7902 不通；缓存（23 分钟前）$')
        # 没登录的账号套餐那一列是 —（usage_table 直接拿到套餐是空的一行时那一列空着：cli/src/cmd/usage.rs 的单元测试 table_folds_columns_in_order）
        self.assertRegex(lines[4], r'^ {8}dave@example.com +— +还没登录（cca @dave@example.com 登录）$')
        self.assertEqual(lines[5], '  ! alice@example.com：Claude Code 里记的套餐还是 Pro（登录时记下的，不会自己更新），'
                                   '账号现在是 Max：重新登录一次就会更新（cca login alice@example.com）')

    def test_narrow_terminal_folds_columns_in_order(self):
        lines = self.table(80)
        self.assertRegex(lines[0], r'^  账号 +5 小时/重置 +本周/重置 +备注$')   # 先收起有别名的账号的邮箱，再收套餐
        self.assertRegex(lines[1], r'^  工作 +13% 2小时13分后 +41% 周. \d\d:\d\d  Sonnet 本周 60%；…$')
        self.assertRegex(lines[3], r'出口端口 7902 不通；…$')   # 原因在前：窄了先保住它
        self.assertEqual(lines[5], '  ! 套餐过时（记的是 Pro，现在是 Max）：cca login alice@example.com')
        lines = self.table(40)
        self.assertRegex(lines[0], r'^  账号 +5 小时 +本周$')   # 最窄时连重置时间也收起，不写备注
        self.assertTrue(any('终端太窄，备注没显示' in l for l in lines))

    # test_fit_note（备注放不下时的取舍）：cli/src/cmd/usage.rs 的单元测试 fit_note_like_python_test
    # test_reset_short（表格里的重置时间）：cli/src/cmd/usage.rs 的单元测试 reset_short_like_python_test

    # --- 周结算提醒 ---
    def use_first(self, rl, why=None):
        """cca usage --json 里这个账号的 use_first。why 是 None：rl 是这次查到的；不是 None：这次没查成（why 是原因），rl 是缓存里的"""
        key = 'bob@example.com'
        self.write(self.cca / 'accounts.json', {key: 'US-A'})
        if why is None:
            self.result(0, rl)
        else:
            self.skip(0, why)
            self.cache(key, {'fetchedAtMs': int(self.now * 1000) - 5000, 'utilization': rl})
        [a] = self.render([key], 'json')
        self.assertEqual(a['source'], 'live' if why is None else 'cache')
        return a['use_first']

    def test_use_first(self):
        use_first, iso = self.use_first, self.iso
        at = float(int(self.now + 10 * 3600))   # 重置时间写到秒
        rl = lambda pct, secs=10 * 3600: {'seven_day': {'utilization': pct, 'resets_at': iso(secs)}}
        self.assertEqual(use_first(rl(41)), {'left_percent': 59, 'from_epoch': at - 86400, 'until_epoch': at})
        self.assertEqual(use_first(rl(80.4))['left_percent'], 20)   # 按四舍五入后显示的百分比（80%）
        self.assertIsNone(use_first(rl(80.5)))   # 显示 81%：还剩 19%
        self.assertIsNone(use_first(rl(41, -60)))   # 重置时间已经过了：缓存里的数不算数
        self.assertIsNone(use_first({'seven_day': {'utilization': 41}}))   # 不知道什么时候重置
        self.assertIsNone(use_first({}, 'timeout'))   # 没有本周的窗口（一个窗口都没有的算不上查到了：拿缓存里空的试）
        self.assertIsNone(use_first({'five_hour': {'utilization': 5, 'resets_at': iso(3600)}}))
        self.assertIsNone(use_first(rl(41), 'port'))   # 这次查询说明账号现在用不了
        self.assertIsNotNone(use_first(rl(41), 'timeout'))   # 只是没查成：照缓存里的数

    # --- API key 的掩码 ---
    def hint(self, secret):
        """记下这把 key（cca key 账号，key 从 stdin 给）以后 config.json 里留的掩码：App 默认显示它"""
        key = 'bob@example.com'
        self.write(self.cca / 'accounts.json', {key: 'US-A'})
        r = self.run_helper('secret-set', key, stdin=secret)
        self.assertEqual(r.returncode, 0, r.stderr)
        return json.loads((self.cca / 'config.json').read_text())['key_hints'][ident(key)]['hint']

    def test_key_hint(self):
        key_hint = self.hint
        self.assertEqual(key_hint('sk-ant-api03-0123456789abcdefghijWXYZ'), 'sk-ant-…WXYZ')
        self.assertEqual(key_hint('sk-proj-0123456789abcdefghijEFGH'), 'sk-proj-…EFGH')
        self.assertEqual(key_hint('abcdefghijklmnopqrstuvwxyz'), '…wxyz')
        self.assertEqual(key_hint('sk-short-1234'), '')   # 太短：露出的比例太大
        # 带空白的不算 key，记不下来（它的掩码是空的：cli/src/core/model.rs 的单元测试 key_hint_like_python_test）
        r = self.run_helper('secret-set', 'bob@example.com', stdin='has space in the key 0123456789')
        self.assertEqual((r.returncode, r.stdout), (1, ''))
        self.assertEqual(json.loads((self.cca / 'config.json').read_text())['key_hints'][ident('bob@example.com')]['hint'], '', '上一把的记录没动')
        for k in ('sk-ant-api03-0123456789abcdefghijWXYZ', 'abcdefghijklmnopqrstuvwxyz'):
            self.assertRegex(key_hint(k), r'^[!-~]{0,8}…[!-~]{4}$')

    # --- 每周用量 ---
    # test_weeks_add（按重置时间认是哪一周，记最高的百分比；掉下去一大截是中间重置过）：cli/src/cmd/usage.rs 的单元测试 weeks_add_like_python_test

    def weeks(self, key, tool='claude'):
        return json.loads((self.acct(key, tool) / 'usage-weeks.json').read_text())['weeks']

    def test_weeks_record_file(self):
        """查到用量时记进账号目录的 usage-weeks.json（600）；坏的项读的时候不要。
        同一次查询不重写：cli/src/cmd/usage.rs 的单元测试 weeks_record_file_like_python_test"""
        key = 'bob@example.com'
        self.write(self.cca / 'accounts.json', {key: 'US-A'})
        p = self.acct(key) / 'usage-weeks.json'

        def query():
            """查到一次用量；返回查到的时间在哪两个时刻之间"""
            self.result(0, {'seven_day': {'utilization': 12.5, 'resets_at': self.now + 86400}})
            t0 = time.time()
            self.render([key], 'json')
            return t0 - 0.001, time.time()   # 记下的时间是毫秒取整的

        self.assertFalse(p.exists())   # 没查过的账号没有记录
        t0, t1 = query()
        self.assertEqual(os.stat(p).st_mode & 0o777, 0o600)
        [w] = self.weeks(key)
        self.assertEqual(w['peak'], 12.5)
        self.assertTrue(t0 <= w['first'] <= t1 and w['last'] == w['first'], (t0, w, t1))
        # 文件里坏的项：下一次查到时读进来的只有好的那些（缺的 carried、resets 补上 0），新的一周接在后面
        self.write(p, {'version': 1, 'weeks': [{'end': 'x'}, 'bad', {'end': 1, 'peak': 2, 'first': 3, 'last': 4}]}, 0o600)
        query()
        self.assertEqual(self.weeks(key)[:-1], [{'end': 1, 'peak': 2, 'first': 3, 'last': 4, 'carried': 0, 'resets': 0}])
        nan, inf = float('nan'), float('inf')
        self.write(p, {'version': 1, 'weeks': [{'end': 1, 'peak': nan, 'first': 3, 'last': 4},
                                               {'end': 5, 'peak': 6, 'first': 7, 'last': 8, 'carried': inf, 'resets': nan}]}, 0o600)
        query()
        self.assertEqual(self.weeks(key)[:-1], [{'end': 5, 'peak': 6, 'first': 7, 'last': 8, 'carried': 0, 'resets': 0}])
        self.assertEqual((len(self.weeks(key)), self.weeks(key)[-1]['peak']), (2, 12.5))

    def gui_account(self, tool, key):
        """cca _gui state（App 的只读快照；bash 带着 CCA_RO=1 跑它）里的这个账号"""
        r = self.run_helper('gui-state', CCA_RO='1')
        self.assertEqual((r.returncode, r.stderr), (0, ''))
        return next(a for t in json.loads(r.stdout)['tools'] if t['id'] == tool for a in t['accounts'] if a['key'] == key)

    def test_usage_weeks_view(self):
        """App 看的每周用量：最近 12 周，标出还没过完的、结束前一天没查到的；统计只算过完的几周（四舍五入后的百分比）。
        拿 Codex 账号看（每周用量两种工具是同一套）：_gui state 里 Claude 官方账号那一节还要等邮箱助手移植完"""
        key, W, D = 'alice@example.com', WEEK, 86400
        self.write(self.cca / 'codex.json', {key: 'US-A'})
        acct = self.acct(key, 'codex')
        save = lambda weeks: self.write(acct / 'usage-weeks.json', {'version': 1, 'weeks': weeks}, 0o600)
        view = lambda: self.gui_account('codex', key)['usage_weeks']
        now = time.time()

        def week(end, peak, carried=0, resets=0, first=None, last=None):
            return {'end': end, 'peak': peak, 'carried': carried, 'resets': resets,
                    'first': end - W + 3600 if first is None else first, 'last': end - 600 if last is None else last}

        self.assertIsNone(view())   # 还没有记录
        weeks = [week(now - 13 * W, 90),                         # 13 周前结束：不显示，只算进从什么时候开始记
                 week(now - 3 * W, 99.5),                        # 显示成 100%：算用完
                 week(now - 2 * W, 30, last=now - 2 * W - 2 * D),   # 结束前两天就没再查到：实际可能更多
                 week(now - W, 60, carried=70, resets=1),        # 中间重置过一次：一共 130%
                 week(now + 2 * D, 20, last=now - 60)]           # 这一周还没过完
        save(weeks)
        v = view()
        self.assertEqual(v['since_epoch'], weeks[0]['first'])
        self.assertEqual([(w['start_epoch'], w['end_epoch']) for w in v['weeks']], [(w['end'] - W, w['end']) for w in weeks[1:]])
        self.assertEqual([w['used_percent'] for w in v['weeks']], [99.5, 30, 130, 20])
        self.assertEqual([w['current'] for w in v['weeks']], [False, False, False, True])
        self.assertEqual([w['partial'] for w in v['weeks']], [False, True, False, False])
        self.assertEqual([w['resets'] for w in v['weeks']], [0, 0, 1, 0])
        self.assertEqual(v['weeks'][-1]['last_epoch'], now - 60)
        self.assertEqual(v['summary'], {'weeks': 3, 'average_percent': 87, 'maxed': 2, 'under_half': 1})
        # 缓存的用量比记下的每周用量新（Claude 账号：Claude Code 自己的缓存比 cca 记的新）：只在内存里并进来，文件不动
        claims = {'email': key, 'https://api.openai.com/auth': {'chatgpt_account_id': 'ws-1', 'chatgpt_user_id': 'user-0123456789'}}
        self.write(acct / 'codex' / 'auth.json', {'tokens': {'id_token': id_token(claims)}})   # 登录着：缓存的用量才算数
        self.cache(key, {'fetchedAtMs': now * 1000, 'utilization': {'seven_day': {'utilization': 35, 'resets_at': now + 2 * D}}}, 'codex')
        self.assertEqual(view()['weeks'][-1]['used_percent'], 35)
        self.assertEqual(self.weeks(key, 'codex')[-1]['peak'], 20)
        self.cache(key, None, 'codex')
        # 中间重置后重新计时（下一周提前开始）：上一周到那时为止，不算还没过完
        save([week(now + 3 * D, 80, first=now - 4 * D, last=now - 2 * D), week(now + 6 * D, 5, first=now - D, last=now - 60)])
        v = view()
        self.assertEqual([(w['end_epoch'], w['current'], w['partial']) for w in v['weeks']], [(now - D, False, False), (now + 6 * D, True, False)])
        self.assertEqual(v['summary'], {'weeks': 1, 'average_percent': 80, 'maxed': 0, 'under_half': 0})
        # 最近 12 周都没有记录
        save([week(now - 20 * W, 50)])
        self.assertEqual(view()['weeks'], [])
        self.assertEqual(view()['summary'], {'weeks': 0, 'average_percent': None, 'maxed': 0, 'under_half': 0})

    def test_usage_cached_keeps_model_scoped(self):
        """Claude Code 的用量缓存（cachedUsageUtilization）比 usage.json 新时：它没有 model_scoped，照 usage.json 有哪几个模型从它的 limits 补上
        （数字取新的；limits 被拿掉了照搬旧的），Fable 那几行不会一闪没了；usage.json 里没有的模型不加，账号对不上的不算"""
        key, ends = 'bob@example.com', '2099-01-01T00:00:00Z'
        self.write(self.cca / 'accounts.json', {key: 'US-A'})
        self.skip(0, 'timeout')   # 这次没查成：表格、JSON 里用的是缓存

        def mine(at, rl, uuid='u-1'):
            return {'fetchedAtMs': at * 1000, 'accountUuid': uuid, 'utilization': rl}

        weekly = {'kind': 'weekly_all', 'percent': 61, 'resets_at': ends, 'scope': None}
        fable = {'kind': 'weekly_scoped', 'percent': 44, 'resets_at': ends, 'scope': {'model': {'id': None, 'display_name': 'Fable'}}}
        ours = {'five_hour': None, 'seven_day': {'utilization': 60, 'resets_at': ends},
                'limits': [dict(weekly, percent=60), dict(fable, percent=43)],
                'model_scoped': [{'display_name': 'Fable', 'utilization': 43, 'resets_at': ends}],
                'weekly_scoped_shares': [{'limit_index': 1, 'used_percent_of_weekly': 21, 'allowance_percent_of_weekly': 50}]}
        theirs = {'five_hour': None, 'seven_day': {'utilization': 61, 'resets_at': ends}, 'limits': [weekly, fable],
                  'weekly_scoped_shares': [{'limit_index': 1, 'used_percent_of_weekly': 22, 'allowance_percent_of_weekly': 50}]}
        at = lambda cache: cache['fetchedAtMs'] / 1000.0   # 取到它的时间（秒）
        scoped = lambda e: None if e['model_scoped'] is None else [{'display_name': m['name'], 'utilization': m['utilization'], 'resets_at': m['resets_at']}
                                                                    for m in e['model_scoped']]

        def pick(caches):
            """caches：[cca 存的 usage.json, Claude Code 的 cachedUsageUtilization]（None：没有）→ cca usage --json 里这个账号的条目"""
            self.cache(key, caches[0])
            claude_json = self.login(key, uuid='u-1', cached=caches[1])
            before = claude_json.read_text()
            [e] = self.render([key], 'json', rc=0 if any(caches) else 1)
            self.assertEqual(claude_json.read_text(), before)   # 只在给出去的那份里补，读到的不改
            return e

        newer = mine(self.now - 2, theirs)
        e = pick([mine(self.now - 10, ours), newer])
        self.assertEqual((e['source'], e['fetched_epoch']), ('cache', at(newer)))
        self.assertEqual(e['seven_day']['utilization'], 61)
        self.assertEqual(scoped(e), [{'display_name': 'Fable', 'utilization': 44, 'resets_at': ends}])
        self.assertNotIn('model_scoped', newer['utilization'])
        self.assertIsNone(e['five_hour'])
        self.assertEqual([(m['name'], m['utilization'], m.get('allowance_percent_of_weekly'), m.get('used_percent_of_weekly'))
                          for m in e['model_scoped']], [('Fable', 44, 50, 22)])
        # usage.json 是新的那份：原样
        mine_newer = mine(self.now - 1, ours)
        e = pick([mine_newer, mine(self.now - 2, theirs)])
        self.assertEqual((e['fetched_epoch'], e['seven_day']['utilization'], scoped(e)), (at(mine_newer), 60, ours['model_scoped']))
        self.assertEqual([(m['name'], m.get('used_percent_of_weekly')) for m in e['model_scoped']], [('Fable', 21)])
        # 一样新：取 usage.json
        e = pick([mine(self.now - 2, ours), mine(self.now - 2, theirs)])
        self.assertEqual((e['seven_day']['utilization'], scoped(e)), (60, ours['model_scoped']))
        # limits 被拿掉了（Claude Code 查不到时拿缓存顶上的那种）：照搬 usage.json 的那一项
        bare = {k: v for k, v in theirs.items() if k != 'limits'}
        self.assertEqual(scoped(pick([mine(self.now - 10, ours), mine(self.now - 2, bare)])), ours['model_scoped'])
        # limits 里没有这个模型了：也照搬，哪一行都不少（下一次查到时以查到的为准）
        gone = dict(theirs, limits=[weekly])
        self.assertEqual(scoped(pick([mine(self.now - 10, ours), mine(self.now - 2, gone)])), ours['model_scoped'])
        # usage.json 里没有分模型的额度（[]）：不照 limits 加，cca 不猜 get_usage 给哪几个模型
        none = dict(ours, model_scoped=[])
        self.assertEqual(scoped(pick([mine(self.now - 10, none), mine(self.now - 2, theirs)])), [])
        # 只有 Claude Code 的缓存（cca 还没查过）：不补
        self.assertIsNone(scoped(pick([None, mine(self.now - 2, theirs)])))
        # usage.json 是别的账号的（换过登录）：不拿它补
        self.assertIsNone(scoped(pick([mine(self.now - 10, ours, uuid='u-2'), mine(self.now - 2, theirs)])))
        # 名字重复的只留一个；limits 里的不是数字时照搬旧的
        dup = dict(ours, model_scoped=ours['model_scoped'] * 2)
        odd = dict(theirs, limits=[weekly, dict(fable, percent=None)])
        self.assertEqual(scoped(pick([mine(self.now - 10, dup), mine(self.now - 2, odd)])), ours['model_scoped'])
        e = pick([None, None])   # 哪边都没有：没有用量可给
        self.assertEqual((e['source'], e['fetched_epoch'], e['seven_day']), ('none', None, None))


if __name__ == '__main__':
    unittest.main()
