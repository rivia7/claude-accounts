#!/usr/bin/env python3
"""cca usage 的表格（一个账号一行、不超过终端的宽度）和周结算提醒（use_first）：临时 HOME、内嵌 Python，不联网。"""
import contextlib
import io
import os
from pathlib import Path
import tempfile
import time
import unittest
from unittest import mock


CCA = Path(__file__).resolve().parents[1] / 'cca'
SOURCE = CCA.read_text().split("PY_SRC=$(cat <<'PY'\n", 1)[1].split('\nPY\n)', 1)[0]
DEFINITIONS = SOURCE.split('\nif len(sys.argv) < 2', 1)[0]


class UsageView(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-usage-')
        self.addCleanup(self.temp.cleanup)
        home = Path(self.temp.name)
        (home / '.cca').mkdir()
        with mock.patch.dict(os.environ, {'HOME': str(home), 'CCA_HOME': str(home / '.cca'), 'CCA_TOOL': 'claude'}, clear=True):
            self.ns = {}
            exec(compile(DEFINITIONS, str(CCA), 'exec'), self.ns)
        self.now = time.time()

    def iso(self, secs):
        return time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime(self.now + secs))

    def row(self, alias, email, plan, five, week, source='live', why=None, msg='', extra=None, old=''):
        rl = {'five_hour': {'utilization': five, 'resets_at': self.iso(2 * 3600 + 13 * 60 + 30)},
              'seven_day': {'utilization': week, 'resets_at': self.iso(3 * 86400 + 5 * 3600)}}
        rl.update(extra or {})
        return {'key': '%s(%s)' % (alias, email) if alias else email, 'name': alias or email, 'alias': alias, 'email': email,
                'plan': plan, 'source': source, 'at': self.now - (23 * 60 if source == 'cache' else 0),
                'rl': rl if source != 'none' else {}, 'why': why, 'msg': msg, 'outdated_login_plan': old}

    def rows(self):
        weekly = {'seven_day_sonnet': {'utilization': 60, 'resets_at': '2099-01-01T00:00:00Z'},
                  'model_scoped': [{'display_name': 'Fable', 'utilization': 34, 'resets_at': '2099-01-01T00:00:00Z'}]}
        return [self.row('工作', 'bob@example.com', 'Ultra', 13, 41, extra=weekly),
                self.row('', 'alice@example.com', 'Max', 98, 72, old='Pro'),
                self.row('副号', 'carol@example.com', 'Pro', 7, 33, source='cache', why='port', msg='7902'),
                self.row('', 'dave@example.com', '', 0, 0, source='none', why='login')]

    def table(self, rows, cols, api=('kimi',)):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.ns['usage_table'](rows, self.now, str(cols), False, list(api))
        return out.getvalue().splitlines()

    def test_one_line_per_account_never_wider_than_terminal(self):
        rows, dw = self.rows(), self.ns['dw']
        for cols in range(40, 201):
            with self.subTest(cols=cols):
                lines = self.table(rows, cols)
                self.assertLessEqual(max(dw(l) for l in lines), max(cols, 40) - 2)
                for r in rows:   # 每个账号正好一行（别名、没有别名的邮箱总有一个看得到）
                    self.assertEqual(sum(1 for l in lines if (r['alias'] or r['email']) in l and not l.lstrip().startswith('!')), 1)

    def test_wide_terminal_keeps_everything(self):
        lines = self.table(self.rows(), 160)
        self.assertRegex(lines[0], r'^  账号 +套餐 +5 小时/重置 +本周/重置 +备注$')
        self.assertRegex(lines[1], r'^  工作  bob@example.com +Ultra +13% 2小时13分后 +41% 周. \d\d:\d\d  Sonnet 本周 60%；Fable 本周 34%$')
        self.assertRegex(lines[3], r'出口端口 7902 不通；缓存（23 分钟前）$')
        self.assertRegex(lines[4], r'^ {8}dave@example.com +还没登录（cca @dave@example.com 登录）$')
        self.assertEqual(lines[5], '  ! alice@example.com：Claude Code 里记的套餐还是 Pro（登录时记下的，不会自己更新），'
                                   '账号现在是 Max：重新登录一次就会更新（cca login alice@example.com）')

    def test_narrow_terminal_folds_columns_in_order(self):
        lines = self.table(self.rows(), 80)
        self.assertRegex(lines[0], r'^  账号 +5 小时/重置 +本周/重置 +备注$')   # 先收起有别名的账号的邮箱，再收套餐
        self.assertRegex(lines[1], r'^  工作 +13% 2小时13分后 +41% 周. \d\d:\d\d  Sonnet 本周 60%；…$')
        self.assertRegex(lines[3], r'出口端口 7902 不通；…$')   # 原因在前：窄了先保住它
        self.assertEqual(lines[5], '  ! 套餐过时（记的是 Pro，现在是 Max）：cca login alice@example.com')
        lines = self.table(self.rows(), 40)
        self.assertRegex(lines[0], r'^  账号 +5 小时 +本周$')   # 最窄时连重置时间也收起，不写备注
        self.assertTrue(any('终端太窄，备注没显示' in l for l in lines))

    def test_fit_note(self):
        fit_note = self.ns['fit_note']
        parts = ['剩余重置 2 次（最早 12 月 5 日过期）', 'GPT-5-Pro 本周 30%', 'credits 余额 12.5']
        self.assertEqual(fit_note(parts, 200), '；'.join(parts))
        self.assertEqual(fit_note(parts, 60), '剩余重置 2 次（最早 12 月 5 日过期）；GPT-5-Pro 本周 30%；…')
        self.assertEqual(fit_note(parts, 20), '剩余重置 2 次…')   # 第一项都放不下：先去掉括号里的补充
        self.assertEqual(fit_note(['出口端口 7902 不通'], 10), '出口端口 …')

    def test_reset_short(self):
        reset_short = self.ns['reset_short']
        self.assertEqual(reset_short(self.now + 2 * 3600 + 13 * 60 + 5, self.now), '2小时13分后')
        self.assertEqual(reset_short(self.now + 30, self.now), '不到1分钟后')
        self.assertEqual(reset_short(self.now + 25 * 60 + 5, self.now, True), '25分钟后')   # 本周的一小时以内也写还有多久
        lt = time.localtime(self.now + 3 * 86400)
        self.assertEqual(reset_short(self.now + 3 * 86400, self.now, True), '周%s %02d:%02d' % ('一二三四五六日'[lt.tm_wday], lt.tm_hour, lt.tm_min))

    def test_use_first(self):
        use_first, iso = self.ns['use_first'], self.iso
        at = self.ns['parse_ts'](iso(10 * 3600))
        rl = lambda pct, secs=10 * 3600: {'seven_day': {'utilization': pct, 'resets_at': iso(secs)}}
        self.assertEqual(use_first(rl(41), self.now), {'left_percent': 59, 'from_epoch': at - 86400, 'until_epoch': at})
        self.assertEqual(use_first(rl(80.4), self.now)['left_percent'], 20)   # 按四舍五入后显示的百分比（80%）
        self.assertIsNone(use_first(rl(80.5), self.now))   # 显示 81%：还剩 19%
        self.assertIsNone(use_first(rl(41, -60), self.now))   # 重置时间已经过了：缓存里的数不算数
        self.assertIsNone(use_first({'seven_day': {'utilization': 41}}, self.now))   # 不知道什么时候重置
        self.assertIsNone(use_first({}, self.now))
        self.assertIsNone(use_first(rl(41), self.now, 'port'))   # 这次查询说明账号现在用不了
        self.assertIsNotNone(use_first(rl(41), self.now, 'timeout'))   # 只是没查成：照缓存里的数

    def test_key_hint(self):
        key_hint = self.ns['key_hint']
        self.assertEqual(key_hint('sk-ant-api03-0123456789abcdefghijWXYZ'), 'sk-ant-…WXYZ')
        self.assertEqual(key_hint('sk-proj-0123456789abcdefghijEFGH'), 'sk-proj-…EFGH')
        self.assertEqual(key_hint('abcdefghijklmnopqrstuvwxyz'), '…wxyz')
        self.assertEqual(key_hint('sk-short-1234'), '')   # 太短：露出的比例太大
        self.assertEqual(key_hint('has space in the key 0123456789'), '')
        for k in ('sk-ant-api03-0123456789abcdefghijWXYZ', 'abcdefghijklmnopqrstuvwxyz'):
            self.assertRegex(key_hint(k), r'^[!-~]{0,8}…[!-~]{4}$')


if __name__ == '__main__':
    unittest.main()
