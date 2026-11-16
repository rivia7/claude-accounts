#!/usr/bin/env python3
"""邮箱助手（cca mail）：认登录邮件的规则、连 Gmail 的路（照系统代理、不经账号的出口）、只读的 IMAP、核对（只信 Gmail 写的
验证结果和 Anthropic 签过名的头）、盯邮箱（最新的一封、浏览器打不开时重试、断线重连不漏邮件）、打开过的不再打开；
还有经本机的假 Gmail（tests/mail_fake.py：真的 TLS、只读 IMAP）走一遍 setup / test / open / 登录时盯邮箱。临时 HOME，不联网。"""
import ast
import contextlib
import io
import json
import os
from pathlib import Path
import re
import stat
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import mail_fake as mf  # noqa: E402

CCA = Path(__file__).resolve().parents[1] / 'cca'
SOURCE = CCA.read_text().split("PY_SRC=$(cat <<'PY'\n", 1)[1].split('\nPY\n)', 1)[0]
DEFINITIONS = SOURCE.split('\nif len(sys.argv) < 2', 1)[0]
PW = 'abcdefghijklmnop'
ALL = '"[Gmail]/All Mail"'


class Base(unittest.TestCase):
    """每个用例一个临时 HOME 和一份新执行的 PY_SRC（全局变量按这个 HOME 算）"""
    extra_env = {}

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-mail-')
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name) / 'home'
        (self.home / '.cca').mkdir(parents=True)
        env = {'HOME': str(self.home), 'CCA_HOME': str(self.home / '.cca'), 'CCA_TOOL': 'claude', 'CCA_KEYSTORE': 'file',
               'PATH': os.environ.get('PATH', '/usr/bin:/bin')}
        env.update(self.extra_env)
        p = mock.patch.dict(os.environ, env, clear=True)
        p.start()
        self.addCleanup(p.stop)
        self.ns = {}
        exec(compile(DEFINITIONS, str(CCA), 'exec'), self.ns)
        self.rule = self.ns['MAIL_RULES']['claude-login']

    def msg(self, raw):
        return self.ns['mail_parse'](raw)


class Rules(Base):
    def test_link_must_be_exactly_a_claude_login_link(self):
        ok = self.rule.link_ok
        self.assertTrue(ok('https://claude.ai/magic-link#tok-abc:YWxpY2U'))
        self.assertTrue(ok('https://claude.ai/magic-link?x=1#t'))
        for bad in ('http://claude.ai/magic-link#t', 'https://claude.ai.evil.com/magic-link#t', 'https://claude.ai@evil.com/magic-link#t',
                    'https://evil.com/claude.ai/magic-link', 'https://claude.ai:443/magic-link#t', 'https://Claude.ai/magic-link#t',
                    'https://claude.ai/magic-linkx#t', 'https://claude.ai/magic-link/../logout', 'https://claude.ai/login#t',
                    'https://claude.ai/magic-link#t"onload', "https://claude.ai/magic-link#t'x", 'https://claude.ai/magic-link#a<b',
                    'https://claude.ai/magic-link#a\\b', 'https://claude.ai/magic-link# t', 'https://claude.ai/magic-link#\u4e2d',
                    'https://claude.ai/magic-link#' + 'a' * 2048, 'javascript:alert(1)', '', None, 7):
            self.assertFalse(ok(bad), bad)

    def test_sender_domain_by_label_suffix(self):
        d = self.rule.domain_ok
        self.assertTrue(d('anthropic.com') and d('mail.anthropic.com') and d('MAIL.Anthropic.COM'))
        self.assertFalse(d('evilanthropic.com') or d('anthropic.com.evil.com') or d('') or d(None))

    def test_subject(self):
        s = lambda v: self.rule.subject_ok(self.ns['mail_text'](v))
        self.assertTrue(s('Your secure link to Claude is here'))   # 真邮件（2026-11-16 核对）；转发会加后缀
        self.assertTrue(s('Your secure link to Claude is here | 2026-11-16 10:36:30'))
        self.assertTrue(s('轉寄: Your secure link to Claude is here'))
        self.assertTrue(s('Secure link to log in to Claude.ai | 2026-11-15'))   # 帮助中心写的说法
        self.assertTrue(s('Your secure link to Claude.ai is here'))
        self.assertTrue(s('=?utf-8?q?Secure_link_to_log_in_to_Claude=2Eai?='))
        # 应用名不是 Claude 的不认：Verification Portal 是真从 mail.anthropic.com 发出、dkim 也过的（见过），只靠签名挡不住
        self.assertFalse(s('Your secure link to Verification Portal is here'))
        self.assertFalse(s('Your receipt from Anthropic') or s('Secure link to log in to Claude-ai')
                         or s('Your secure link to Claudexyz is here'))


class Route(Base):
    SCUTIL = ('<dictionary> {\n  ExceptionsList : <array> {\n    0 : *.local\n    1 : 169.254/16\n  }\n  FTPPassive : 1\n'
              '  HTTPEnable : 1\n  HTTPPort : 7890\n  HTTPProxy : 127.0.0.1\n  HTTPSEnable : 1\n  HTTPSPort : 7891\n'
              '  HTTPSProxy : 127.0.0.1\n  SOCKSEnable : 1\n  SOCKSPort : 7892\n  SOCKSProxy : 127.0.0.1\n'
              '  __SCOPED__ : <dictionary> {\n    en0 : <dictionary> {\n      SOCKSPort : 1\n    }\n  }\n}\n')

    def route(self, spec, kv=None, exc=None, c=None):
        sysp = self.ns['mail_proxy_parse'](self.SCUTIL) if kv is None else (kv, exc or [])
        return self.ns['mail_route'](spec, c or {}, sysp)

    def test_scutil_parse_top_level_only(self):
        kv, exc = self.ns['mail_proxy_parse'](self.SCUTIL)
        self.assertEqual((kv['SOCKSPort'], kv['HTTPSProxy'], exc), ('7892', '127.0.0.1', ['*.local', '169.254/16']))

    def test_system_proxy_socks_first_then_https_then_http(self):
        self.assertEqual(self.route('system'), ('socks5', '127.0.0.1', 7892))
        kv, _ = self.ns['mail_proxy_parse'](self.SCUTIL)
        kv['SOCKSEnable'] = '0'
        self.assertEqual(self.route('', kv), ('http', '127.0.0.1', 7891))
        kv['HTTPSEnable'] = '0'
        self.assertEqual(self.route(None, kv), ('http', '127.0.0.1', 7890))
        kv['HTTPEnable'] = '0'
        self.assertEqual(self.route('system', kv), ('direct', '', 0))
        self.assertEqual(self.route('system', {'ProxyAutoConfigEnable': '1'}), ('direct', '', 0), '只有 PAC：直连')

    def test_system_proxy_exceptions(self):
        kv, _ = self.ns['mail_proxy_parse'](self.SCUTIL)
        for pat in ('*.gmail.com', '.gmail.com', 'imap.gmail.com', 'IMAP.GMAIL.COM'):
            self.assertEqual(self.route('system', kv, [pat]), ('direct', '', 0), pat)
        self.assertEqual(self.route('system', kv, ['*.google.com']), ('socks5', '127.0.0.1', 7892))

    def test_explicit_specs(self):
        self.assertEqual(self.route('direct'), ('direct', '', 0))
        self.assertEqual(self.route('socks5://127.0.0.1:7897'), ('socks5', '127.0.0.1', 7897))
        self.assertEqual(self.route('socks5h://proxy.example.com:1080/'), ('socks5', 'proxy.example.com', 1080))
        self.assertEqual(self.route('http://[::1]:8080'), ('http', '::1', 8080))
        for bad in ('socks4://127.0.0.1:1', 'http://127.0.0.1', 'http://127.0.0.1:0', 'http://127.0.0.1:70000', 'http://a b:1',
                    'https://127.0.0.1:1', 'system2'):
            with self.assertRaises(self.ns['MailFatal'], msg=bad):
                self.route(bad)

    def test_never_through_an_account_exit(self):
        c = {'ports': {'alice@example.com': 7901}, 'proxy_host': '192.0.2.10', 'codex': {'ports': {'dave@example.com': 7905}}}
        for spec in ('socks5://127.0.0.1:7901', 'http://192.0.2.10:7901', 'http://localhost:7905'):
            with self.assertRaises(self.ns['MailFatal'], msg=spec):
                self.route(spec, c=c)
        self.assertEqual(self.route('socks5://127.0.0.1:7897', c=c), ('socks5', '127.0.0.1', 7897))
        with mock.patch.dict(os.environ, {'CCA_PROXY': 'http://127.0.0.1:7897'}):
            with self.assertRaises(self.ns['MailFatal']):
                self.route('socks5://127.0.0.1:7897', c=c)


class Parse(Base):
    def test_internaldate(self):
        e = self.ns['mail_epoch']
        self.assertEqual(e('15-Nov-2026 10:00:00 +0800'), e('15-Nov-2026 02:00:00 +0000'))
        self.assertEqual(e(' 5-Nov-2026 02:00:00 -0130'), e('05-Nov-2026 03:30:00 +0000'))
        self.assertEqual(e('15-Nov-2026 02:00:00 +0000'), 1794708000)
        for bad in ('15-Foo-2026 02:00:00 +0000', '2026-11-15 02:00:00', ''):
            self.assertIsNone(e(bad))

    def test_search_criteria(self):
        t = self.ns['mail_epoch']('15-Nov-2026 00:30:00 +0000')
        self.assertEqual(self.ns['mail_since'](t), '14-Nov-2026')
        crit = self.ns['mail_crit']([self.rule], t)
        self.assertEqual(crit, ['SINCE', '14-Nov-2026', 'OR', 'FROM', '"anthropic.com"', 'SUBJECT', '"secure link"'])
        two = self.ns['mail_crit']([self.rule, self.rule], t)
        self.assertEqual(two[:3], ['SINCE', '14-Nov-2026', 'OR'])
        self.assertTrue(two[3].startswith('(OR FROM') and two[4].startswith('(OR FROM'))

    def test_fetch_parse_any_order(self):
        p = self.ns['mail_fetch_parse']
        d = '"15-Nov-2026 02:00:00 +0000"'
        a = [(('1 FETCH (UID 5 INTERNALDATE %s BODY[HEADER.FIELDS (FROM)] {9}' % d).encode(), b'From: x\r\n'), b')',
             (('2 FETCH (INTERNALDATE %s BODY[HEADER.FIELDS (FROM)] {9}' % d).encode(), b'From: y\r\n'), b' UID 7)',
             (b'3 FETCH (UID 8 BODY[] {1}', b'z'), b')']
        out = p(a)
        self.assertEqual(sorted(out), [5, 7], '没有 INTERNALDATE 的不算')
        self.assertEqual(out[7][1], b'From: y\r\n')
        self.assertEqual(p(None), {})


class Judge(Base):
    def judge(self, raw, acct='alice@example.com', got=None, since=None):
        now = time.time()
        return self.ns['mail_judge'](self.rule, self.msg(raw), now if got is None else got, acct, now - 60 if since is None else since)

    def test_good_and_each_failure(self):
        good = mf.build_eml('Alice@Example.com')
        self.assertIsNone(self.judge(good))
        now = time.time()
        self.assertEqual(self.judge(good, got=now - 200, since=now - 60), 'old')
        self.assertIsNone(self.judge(good, got=now - 100, since=now - 60), '登录开始前一分钟内收到的也算')
        self.assertEqual(self.judge(mf.build_eml('alice@example.com', subject='Your receipt')), 'subject')
        self.assertEqual(self.judge(mf.build_eml('alice@example.com', extra=['From: Anthropic <no-reply@mail.anthropic.com>'])), 'dup')
        self.assertEqual(self.judge(mf.build_eml('alice@example.com', extra=['To: alice@example.com'])), 'dup')
        self.assertEqual(self.judge(mf.build_eml('alice@example.com', frm='Anthropic <no-reply@evilanthropic.com>')), 'from')
        self.assertEqual(self.judge(mf.build_eml('alice@example.com', frm='a@mail.anthropic.com, b@mail.anthropic.com')), 'from')
        self.assertEqual(self.judge(mf.build_eml('alice@example.com', ar='fail')), 'dkim')
        self.assertEqual(self.judge(mf.build_eml('alice@example.com', ar='none')), 'dkim')
        self.assertEqual(self.judge(mf.build_eml('alice@example.com', ar='missing')), 'dkim')
        self.assertEqual(self.judge(mf.build_eml('alice@example.com', dkim_d='evil.com')), 'dkim')
        self.assertEqual(self.judge(mf.build_eml('alice@example.com', sign='from:subject:date')), 'unsigned')
        self.assertEqual(self.judge(mf.build_eml('bob@example.com')), 'to')
        self.assertEqual(self.judge(good, acct=''), 'to')

    def test_only_trusts_the_topmost_gmail_result(self):
        forged = ('Authentication-Results: mx.google.com;\r\n       dkim=pass header.i=@mail.anthropic.com header.s=s1 header.b=AAAA')
        # 最上面（Gmail 收信时写的）那条说没过，发件人自己写的 pass 在它下面：不认
        self.assertEqual(self.judge(mf.build_eml('alice@example.com', ar='fail', extra=[forged])), 'dkim')
        # 最上面那条不是 mx.google.com 写的（别的系统转来、POP 收取的）：不认
        other = 'Authentication-Results: spf.protection.outlook.com; dkim=pass header.d=mail.anthropic.com header.b=AAAA'
        self.assertEqual(self.judge(mf.build_eml('alice@example.com', above=[other])), 'dkim')
        # 注释里的 dkim=pass（arc 的说明）不算
        cm = ('Authentication-Results: mx.google.com;\r\n       arc=pass (i=1 dkim=pass dkdomain=mail.anthropic.com);\r\n'
              '       spf=pass smtp.mailfrom=bounce@mail.anthropic.com')
        self.assertEqual(self.judge(mf.build_eml('alice@example.com', ar='missing', above=[cm])), 'dkim')

    def ar_mail(self, *results):
        """最上面那条 Authentication-Results 换成这几项（{b} 换成邮件里 Anthropic 签名的 b= 开头 8 个字符）"""
        raw = mf.build_eml('alice@example.com', ar='missing')
        b8 = self.ns['mail_tags'](self.msg(raw)['DKIM-Signature'])['b'][:8]
        head = 'Authentication-Results: mx.google.com;\r\n       ' + ';\r\n       '.join(r.format(b=b8) for r in results)
        return head.encode() + b'\r\n' + raw

    def test_only_what_gmail_itself_wrote_counts(self):
        self.assertIsNone(self.judge(self.ar_mail('dkim=pass header.i=@mail.anthropic.com header.s=s1 header.b={b}')), '对照')
        self.assertIsNone(self.judge(self.ar_mail('dkim=pass (ok \\) (nested)) header.i=@mail.anthropic.com header.s=s1 header.b={b}')),
                          '注释可以嵌套、带转义')
        for res in (
                # 发件人写的 smtp.mailfrom（带引号的本地部分里能有分号、等号），Gmail 照抄进来：不当成一项结果
                'dkim=fail header.i=@mail.anthropic.com header.b={b}; '
                'spf=pass smtp.mailfrom="x;dkim=pass header.i=@mail.anthropic.com header.b={b}"@evil.example.com',
                'dkim=pass header.i="a header.d=mail.anthropic.com"@evil.example.com header.s=s1 header.b={b}',   # 藏在引号里
                'dkim=pass header.i=@evil.example.com header.i=@mail.anthropic.com header.b={b}',   # 同一个名字两次：分不清
                'dkim=pass header.i=@mail.anthropic.com header.s=s1 header.b=AbC',   # header.b 太短：哪个签名都对得上
                'dkim=pass header.i=@mail.anthropic.com header.s=s1',
                'dkim=pass header.i=@mail.anthropic.com header.b={b} "unterminated',   # 写法不对：整条不信
                'dkim=pass header.i=@mail.anthropic.com (unterminated header.b={b}',
                'dkim pass header.i=@mail.anthropic.com header.b={b}'):
            self.assertEqual(self.judge(self.ar_mail(res)), 'dkim', res)

    def test_signature_matched_by_header_b(self):
        raw = mf.build_eml('alice@example.com')
        m = self.msg(raw)
        sig = m['DKIM-Signature']
        # 再加一个 b 开头一样的签名：分不清是哪个，不认
        twin = 'DKIM-Signature: ' + ' '.join(str(sig).split()).replace('h=from:to:subject', 'h=date')
        self.assertEqual(self.judge(raw.replace(b'\r\nFrom:', ('\r\n' + twin + '\r\nFrom:').encode(), 1)), 'dkim')
        # b 开头不一样的另一个签名不影响
        other = 'DKIM-Signature: v=1; a=rsa-sha256; d=mail.anthropic.com; s=s2; h=date; bh=x=; b=ZZZZZZZZZZZZ'
        self.assertIsNone(self.judge(raw.replace(b'\r\nFrom:', ('\r\n' + other + '\r\nFrom:').encode(), 1)))

    def test_judge_any_reports_the_furthest_reason(self):
        r, why = self.ns['mail_judge_any']([self.rule], self.msg(mf.build_eml('bob@example.com')), time.time(), 'alice@example.com', 0)
        self.assertEqual((r is self.rule, why), (True, 'to'))


class Clean(Base):
    def test_text_written_by_others_cannot_drive_the_terminal(self):
        from email.header import Header
        t = self.ns['mail_text'](Header('\x1b[2K\x1b[1A✓ 通过核对\x9b31m\u202e\x07', 'utf-8').encode())
        self.assertIn('通过核对', t)
        self.assertIsNone(re.search('[\x00-\x1f\x7f-\x9f\u202e]', t), repr(t))
        e = self.ns['mail_oserr'](Exception('a\x1b]52;c;ZXZpbA==\x07b\x9d'))
        self.assertIsNone(re.search('[\x00-\x1f\x7f-\x9f]', e), repr(e))


class Links(Base):
    def test_extracts_first_login_link_from_html_or_text(self):
        raw = mf.build_eml('alice@example.com', body_links=['https://www.anthropic.com/help', 'https://claude.ai/magic-link#tok-X'])
        self.assertEqual(self.ns['mail_extract'](self.rule, self.msg(raw)), 'https://claude.ai/magic-link#tok-X')
        txt = mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-T', html=False)
        self.assertEqual(self.ns['mail_extract'](self.rule, self.msg(txt)), 'https://claude.ai/magic-link#tok-T')
        none = mf.build_eml('alice@example.com', body_links=['https://links.example.com/ls/click?u=https%3A%2F%2Fclaude.ai'])
        self.assertIsNone(self.ns['mail_extract'](self.rule, self.msg(none)))

    def test_attachments_are_ignored(self):
        raw = (b'From: no-reply@mail.anthropic.com\r\nContent-Type: multipart/mixed; boundary="b"\r\n\r\n--b\r\n'
               b'Content-Type: text/plain\r\n\r\nhello\r\n--b\r\nContent-Type: text/html\r\nContent-Disposition: attachment; filename=x.html\r\n\r\n'
               b'<a href="https://claude.ai/magic-link#tok-att">x</a>\r\n--b--\r\n')
        self.assertIsNone(self.ns['mail_extract'](self.rule, self.msg(raw)))

    def test_shape_never_shows_the_token(self):
        s = self.ns['mail_shape']('https://claude.ai/magic-link?a=1#tok-secret')
        self.assertNotIn('secret', s)
        self.assertIn('https://claude.ai/magic-link', s)


class Used(Base):
    def test_claim_once_across_processes(self):
        claim, mark = self.ns['mail_claim'], self.ns['mail_mark']
        self.assertIsNone(claim('d1'))
        self.assertIsNone(claim('d1'), '自己认领过的可以再认领')
        with mock.patch.object(self.ns['os'], 'getpid', return_value=1):   # 另一个进程（pid 1 一直在）：120 秒内不抢
            self.assertIs(claim('d1'), False)
        mark('d1', 'done')
        self.assertIs(claim('d1'), False)
        self.assertEqual(claim('d1', again=True), 'done', '--again：照样打开，失败时照原来的状态还原')
        mark('d1', 'done')
        self.assertIsNone(claim('d3'))
        mark('d3', None)
        self.assertIsNone(claim('d3'), '没打开时去掉认领，下次照样能开')
        used = json.loads(Path(self.home / '.cca' / 'mail' / 'used.json').read_text())
        self.assertEqual(set(used), {'d1', 'd3'})
        self.assertEqual(stat.S_IMODE(os.stat(self.home / '.cca' / 'mail').st_mode), 0o700)
        self.assertEqual(stat.S_IMODE(os.stat(self.home / '.cca' / 'mail' / 'used.json').st_mode), 0o600)
        self.assertNotIn('magic', json.dumps(used))

    def test_old_records_are_dropped(self):
        p = self.home / '.cca' / 'mail'
        p.mkdir(mode=0o700)
        (p / 'used.json').write_text(json.dumps({'old': {'at': time.time() - 3 * 86400, 'st': 'done'}, 'junk': 5}))
        self.assertIsNone(self.ns['mail_claim']('new'))
        self.assertEqual(set(json.loads(Path(p / 'used.json').read_text())), {'new'})


class FakeSource:
    def __init__(self, rounds, msgs):
        self.rounds, self.msgs = list(rounds), msgs
        self.polls = 0

    def poll(self, since):
        self.polls += 1
        r = self.rounds.pop(0) if self.rounds else []
        if isinstance(r, Exception):
            raise r
        return r

    def full(self, cand):
        return self.msgs[cand['uid']]


class Hunt(Base):
    extra_env = {'CCA_MAIL_POLL': '3', 'CCA_MAIL_RETRY': '2'}

    def setUp(self):
        Base.setUp(self)
        self.clock = [1000000.0]
        self.said = []
        self.opened = []

    def now(self):
        return self.clock[0]

    def sleep(self, s):
        self.clock[0] += s

    def cand(self, uid, raw, ago=0):
        return {'folder': ALL, 'label': '所有邮件', 'uid': uid, 'got': 1000000.0 - ago, 'hdr': raw, 'seen': (ALL, 1, uid)}

    def deliver(self, results):
        res = list(results)

        def d(link):
            self.opened.append((round(self.clock[0] - 1000000.0, 1), link))
            return res.pop(0) if res else 'ok'
        return d

    def hunt(self, src, results=(), secs=30, **kw):
        return self.ns['mail_hunt'](src, [self.rule], 'alice@example.com', self.clock[0] - 10, self.clock[0] + secs, self.deliver(results),
                                    say=self.said.append, now=self.now, sleep=self.sleep, **kw)

    def test_opens_only_the_newest_and_keeps_watching(self):
        a = mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-old')
        b = mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-new')
        src = FakeSource([[self.cand(1, a, 5), self.cand(2, b, 1)]], {1: a, 2: b})
        res = self.hunt(src)
        self.assertEqual([l for _, l in self.opened], ['https://claude.ai/magic-link#tok-new'])
        self.assertEqual((res['code'], res['opened']), ('opened', 1))
        self.assertGreater(src.polls, 5, '打开以后接着看（登录结束才停）')
        self.assertEqual(self.said, [])

    def test_browser_failure_retries_three_times_then_says_so(self):
        a = mf.build_eml('alice@example.com')
        res = self.hunt(FakeSource([[self.cand(1, a)]], {1: a}), ['fail', 'fail', 'ok'])
        self.assertEqual([t for t, _ in self.opened], [0.0, 2.0, 4.0])
        self.assertEqual(res['code'], 'opened')
        self.opened, self.said = [], []
        res = self.hunt(FakeSource([[self.cand(2, mf.build_eml('alice@example.com'))]], {2: mf.build_eml('alice@example.com')}),
                        ['fail', 'fail', 'fail'])
        self.assertEqual((len(self.opened), res['code']), (3, 'fail'))
        self.assertEqual(len([s for s in self.said if '没能打开浏览器' in s]), 1)

    def test_retries_finish_even_past_the_end(self):
        a = mf.build_eml('alice@example.com')
        res = self.hunt(FakeSource([[self.cand(1, a)]], {1: a}), ['fail', 'fail', 'ok'], secs=1)
        self.assertEqual((len(self.opened), res['code']), (3, 'opened'))

    def test_once_stops_on_browser_failure(self):
        a = mf.build_eml('alice@example.com')
        res = self.hunt(FakeSource([[self.cand(1, a)]], {1: a}), ['fail'] * 3, secs=600, once=True)
        self.assertEqual((res['code'], len(self.opened)), ('fail', 3))
        self.assertLess(self.clock[0] - 1000000.0, 10)
        self.assertEqual(self.said, [], 'cca mail open 自己说结果')

    def test_connection_trouble_backs_off_and_says_once(self):
        R = self.ns['MailRetry']
        src = FakeSource([R('net', 'x1'), R('net', 'x2'), R('net', 'x3'), R('net', 'x4'), []], {})
        res = self.hunt(src, secs=60)
        self.assertEqual(len([s for s in self.said if '连不上 Gmail' in s]), 1)
        self.assertEqual(res['code'], 'none', '后来连上了')
        res = self.hunt(FakeSource([R('net', 'down')] * 50, {}), secs=20)
        self.assertEqual((res['code'], res['text']), ('net', 'down'), '到最后也没连上：说最后的原因')

    def test_once_fails_fast_only_before_the_first_poll(self):
        R = self.ns['MailRetry']
        res = self.hunt(FakeSource([R('proxy', '代理断开了连接')], {}), once=True)
        self.assertEqual((res['code'], self.clock[0]), ('proxy', 1000000.0))
        a = mf.build_eml('alice@example.com')
        res = self.hunt(FakeSource([[], R('net', 'blip'), [self.cand(3, a)]], {3: a}), once=True, secs=60)
        self.assertEqual(res['code'], 'opened', '中途断一下接着等')

    def test_fatal_stops_at_once(self):
        res = self.hunt(FakeSource([self.ns['MailFatal']('auth', 'Gmail 不认这个密码')], {}))
        self.assertEqual(res['code'], 'auth')

    def test_idle_called_only_when_nothing_to_open(self):
        seen = []
        a = mf.build_eml('alice@example.com')
        self.hunt(FakeSource([[]], {}), once=True, secs=10, idle=lambda: seen.append('idle'))
        self.hunt(FakeSource([[self.cand(1, a)]], {1: a}), once=True, secs=10, idle=lambda: seen.append('bad'))
        self.assertEqual(seen, ['idle'])

    def test_used_mails_are_not_opened_again_unless_again(self):
        a = mf.build_eml('alice@example.com')
        self.hunt(FakeSource([[self.cand(1, a)]], {1: a}), once=True)
        res = self.hunt(FakeSource([[self.cand(1, a)]], {1: a}), once=True, secs=5)
        self.assertEqual((len(self.opened), res['used'], res['code']), (1, 1, 'none'))
        res = self.hunt(FakeSource([[self.cand(1, a)]], {1: a}), once=True, again=True)
        self.assertEqual((len(self.opened), res['code']), (2, 'opened'))

    def test_rejections_quiet_or_said(self):
        mails = {1: mf.build_eml('bob@example.com'), 2: mf.build_eml('alice@example.com', subject='Receipt'),
                 3: mf.build_eml('alice@example.com', frm='x@evil.example.com'), 4: mf.build_eml('alice@example.com', ar='fail')}
        res = self.hunt(FakeSource([[self.cand(u, r) for u, r in mails.items()]], mails), secs=5)
        self.assertEqual(res['rejected'], {'to': 1, 'subject': 1, 'from': 1, 'dkim': 1})
        self.assertEqual(len(self.said), 2, '别的账号的、标题不像的不说；发件人、签名不对的说')
        self.assertEqual(self.opened, [])

    def test_stops_when_login_is_over(self):
        alive = iter([True, True, True, False])
        src = FakeSource([], {})
        self.hunt(src, secs=600, alive=lambda: next(alive, False))
        self.assertLessEqual(src.polls, 2)

    def test_mail_that_cannot_be_fetched_backs_off(self):
        R = self.ns['MailRetry']
        a = mf.build_eml('alice@example.com')

        class Gone(FakeSource):
            def full(self, cand):
                raise R('proto', 'Gmail 没给出这封邮件')
        src = Gone([[self.cand(1, a)]], {})
        res = self.hunt(src, secs=120)
        self.assertEqual(len([s for s in self.said if 'Gmail 没给出这封邮件' in s]), 1)
        self.assertLess(src.polls, 15, '取不到全文的那封照连不上退避，不是每两秒取一次')
        self.assertEqual(res['code'], 'proto')

    def test_mails_not_opened_are_checked_again_next_time(self):
        a = mf.build_eml('alice@example.com', body_links=['https://www.anthropic.com/help'])
        for _ in range(2):
            res = self.hunt(FakeSource([[self.cand(1, a)]], {1: a}), once=True, secs=5)
            self.assertEqual((res['rejected'], res['used']), ({'nolink': 1}, 0), '没打开的不记下：下次照样核对、照实说原因')

    def test_refused_counts_as_rejected(self):
        a = mf.build_eml('alice@example.com')
        res = self.hunt(FakeSource([[self.cand(1, a)]], {1: a}), ['refused'], secs=5)
        self.assertEqual((res['rejected'], len(self.opened)), ({'refused': 1}, 1))
        self.assertTrue(any('打开之前再核对' in s for s in self.said))


class Keystore(Base):
    """ks_read / ks_write / ks_del 走钥匙串的那一支：假的 security 把条目放在字典里（和 tests/setup.sh 的一样认 -a、-s、-X）"""

    def setUp(self):
        Base.setUp(self)
        self.kc, self.calls = {}, []
        self.ns['keyfile_store'] = lambda: False

        def one(w):
            o = {w[i]: w[i + 1] for i in range(1, len(w) - 1) if w[i] in ('-a', '-s', '-X')}
            hit = [k for k in self.kc if k[0] == o.get('-s') and o.get('-a', k[1]) == k[1]]
            if w[0] == 'add-generic-password':
                self.kc[(o['-s'], o['-a'])] = bytes.fromhex(o['-X']).decode()
                return 0, b''
            if not hit:
                return 44, b''
            if w[0] == 'find-generic-password':
                return 0, (self.kc[hit[0]] + '\n').encode()
            del self.kc[hit[0]]
            return 0, b''

        def security(*args, data=None):
            self.calls.append(args)
            if args[0] != '-i':
                return one(list(args))
            for line in data.decode().splitlines():
                one(line.split())
            return 0, b''
        self.ns['security'] = security

    def test_write_replaces_the_same_account_only_and_delete_can_pick_one(self):
        ks, a, b = self.ns, 'abcdefghijklmnop', 'ponmlkjihgfedcba'
        self.assertTrue(ks['ks_write']('svc', 'a@example.com', a))
        self.assertTrue(ks['ks_write']('svc', 'b@example.com', b))
        self.assertTrue(ks['ks_write']('svc', 'b@example.com', a))
        self.assertEqual((ks['ks_read']('svc', 'a@example.com'), ks['ks_read']('svc', 'b@example.com')), ((0, a), (0, a)))
        ks['ks_del']('svc', 'a@example.com')
        self.assertEqual(set(self.kc), {('svc', 'b@example.com')}, '只删这个账户名的')
        ks['ks_del']('svc')
        self.assertEqual(self.kc, {})
        self.assertFalse([c for c in self.calls if c[0] != '-i' and (a in c or b in c)], '秘密不进命令行')


class FakeBox:
    """MailSource 用的假 MailBox：各邮件夹的 (UIDVALIDITY, UIDNEXT) 和邮件；记下每次调用"""

    def __init__(self):
        self.im = None
        self.state = {'A': (1, 10), 'J': (5, 3)}
        self.msgs = {'A': {u: (u * 1.0, b'Subject: %d\r\n' % u) for u in range(1, 10)}, 'J': {1: (1.0, b'x'), 2: (2.0, b'y')}}
        self.hits = {'A': [2, 4], 'J': [1]}
        self.calls, self.cur, self.fail = [], None, None

    def open(self):
        self.im = object()
        self.calls.append('open')

    def folders(self):
        return [('所有邮件', 'A'), ('垃圾邮件', 'J')]

    def examine(self, name):
        self.cur = name
        self.calls.append(('examine', name))
        if self.fail == name:
            self.fail, self.im = None, None
            raise MailRetryBox('net', 'gone')
        return self.state[name]

    def search(self, crit):
        self.calls.append(('search', self.cur))
        return list(self.hits[self.cur])

    def headers(self, uids):
        self.calls.append(('headers', self.cur, uids))
        if isinstance(uids, tuple):
            uids = range(uids[0], uids[1] + 1)
        return {u: self.msgs[self.cur][u] for u in uids if u in self.msgs[self.cur]}

    def message(self, uid):
        return self.msgs[self.cur][uid][1]

    def close(self, polite=False):
        self.im = None


MailRetryBox = None


class Source(Base):
    def setUp(self):
        Base.setUp(self)
        global MailRetryBox
        MailRetryBox = self.ns['MailRetry']
        self.src = self.ns['MailSource']('u@example.com', PW, ('direct', '', 0), [self.rule])
        self.box = self.src.box = FakeBox()

    def uids(self, out):
        return sorted((c['folder'], c['uid']) for c in out)

    def test_first_round_searches_and_takes_the_latest_burst(self):
        out = self.src.poll(0)
        self.assertEqual(self.uids(out), [('A', u) for u in range(1, 10)] + [('J', 1), ('J', 2)])
        self.assertIn(('search', 'A'), self.box.calls)
        self.assertIn(('headers', 'A', (1, 9)), self.box.calls)

    def test_later_rounds_fetch_new_uids_without_searching(self):
        self.src.poll(0)
        self.box.calls = []
        self.assertEqual(self.src.poll(0), [])
        self.assertFalse([c for c in self.box.calls if c[0] in ('search', 'headers')], 'UIDNEXT 没变：不搜索也不取')
        self.box.msgs['A'][10] = (10.0, b'Subject: new\r\n')
        self.box.state['A'] = (1, 11)
        self.box.calls = []
        out = self.src.poll(0)
        self.assertEqual(self.uids(out), [('A', 10)])
        self.assertEqual([c for c in self.box.calls if c[0] in ('search', 'headers')], [('headers', 'A', (10, 10))])

    def test_uidvalidity_change_searches_again(self):
        self.src.poll(0)
        self.box.state['A'] = (2, 10)
        self.box.calls = []
        out = self.src.poll(0)
        self.assertIn(('search', 'A'), self.box.calls)
        self.assertEqual(len(out), 9, '换了 UIDVALIDITY：同样的 UID 是另外的邮件')

    def test_a_round_cut_short_loses_nothing(self):
        self.box.fail = 'J'
        with self.assertRaises(self.ns['MailRetry']):
            self.src.poll(0)
        out = self.src.poll(0)
        self.assertIn(('A', 4), self.uids(out), '断在第二个邮件夹：第一个邮件夹里的下一轮照样给')
        self.assertEqual(self.box.calls.count('open'), 2)


class Lint(Base):
    def test_python38_syntax_and_lazy_imports(self):
        ast.parse(SOURCE, feature_version=(3, 8))
        tree = ast.parse(SOURCE)
        top = {a.name for n in tree.body if isinstance(n, ast.Import) for a in n.names}
        top |= {n.module for n in tree.body if isinstance(n, ast.ImportFrom)}
        self.assertFalse(top & {'imaplib', 'ssl', 'socket', 'email', 'html', 'subprocess'}, top)

    def test_every_rule_is_known(self):
        for tool in self.ns['TOOLS']:
            for r in tool.mail_rules:
                self.assertIn(r, self.ns['MAIL_RULES'])
        self.assertEqual(self.ns['CodexTool'].mail_rules, ())


class WireBase(Base):
    """经本机的假 Gmail：真的 TLS（测试的 CA，CCA_MAIL_CAFILE）、经假代理（SOCKS5 / HTTP CONNECT）。每个子类一个假 Gmail"""

    @classmethod
    def setUpClass(cls):
        cls.dir = tempfile.mkdtemp(prefix='cca-mail-wire-')
        cls.ca = mf.make_pki(os.path.join(cls.dir, 'pki'))
        for k, _, _ in mf.FOLDERS:
            os.makedirs(os.path.join(cls.dir, 'box', k), exist_ok=True)
        cls.srv = mf.Server(cls.dir)
        cls.srv.start()
        cls.px = mf.Proxy(cls.srv)
        cls.px.start()

    @classmethod
    def tearDownClass(cls):
        import shutil
        cls.srv.ls.close()
        cls.px.ls.close()
        shutil.rmtree(cls.dir, ignore_errors=True)

    def setUp(self):
        self.extra_env = {'CCA_MAIL_CAFILE': self.ca, 'CCA_MAIL_POLL': '0.1', 'CCA_MAIL_RETRY': '0.1'}
        Base.setUp(self)
        for k, _, _ in mf.FOLDERS:
            for f in Path(self.dir, 'box', k).glob('*.eml'):
                f.unlink()
        self.ctl({})
        open(os.path.join(self.dir, 'imap.log'), 'w').close()
        self.socks = ('socks5', '127.0.0.1', self.px.port)
        self.http = ('http', '127.0.0.1', self.px.port)

    def ctl(self, d):
        with open(os.path.join(self.dir, 'ctl.json'), 'w') as f:
            json.dump(d, f)

    def put(self, uid, raw, ago=0, folder='all'):
        mf.put_eml(os.path.join(self.dir, 'box', folder, '%d.eml' % uid), raw, time.time() - ago)

    def log(self):
        with open(os.path.join(self.dir, 'imap.log')) as f:
            return f.read()

    def box(self, route=None, pw=PW):
        b = self.ns['MailBox']('inbox@example.com', pw, route or self.socks)
        self.addCleanup(b.close)
        return b



class Wire(WireBase):
    def test_read_only_session_through_socks_and_http(self):
        self.put(3, mf.build_eml('alice@example.com'), 60)
        for route in (self.socks, self.http):
            b = self.box(route)
            b.open()
            self.assertEqual(b.folders(), [('所有邮件', ALL), ('垃圾邮件', '"[Gmail]/Spam"')])
            self.assertEqual(b.examine(ALL), (11, 4))
            self.assertEqual(b.search(self.ns['mail_crit']([self.rule], time.time() - 3600)), [3])
            self.assertIn(b'From:', b.headers([3])[3][1])
            self.assertIn(b'magic-link', b.message(3))
            b.close(True)
        log = self.log()
        self.assertIn('PROXY socks5 imap.gmail.com:993', log)
        self.assertIn('PROXY http imap.gmail.com:993', log)
        self.assertIn('BODY.PEEK[HEADER.FIELDS', log)
        self.assertNotIn('VIOLATION', log)
        self.assertNotIn(PW, log)

    def test_login_errors(self):
        with self.assertRaises(self.ns['MailFatal']) as e:
            self.box(pw='wrongwrongwrongw').open()
        self.assertEqual(e.exception.code, 'auth')
        self.ctl({'imap_disabled': True})
        with self.assertRaises(self.ns['MailFatal']) as e:
            self.box().open()
        self.assertEqual(e.exception.code, 'imap')
        self.ctl({'login_no': '[ALERT] Application-specific password required: https://support.google.com/accounts/answer/185833 (Failure)'})
        with self.assertRaises(self.ns['MailFatal']) as e:
            self.box().open()
        self.assertEqual(e.exception.code, 'auth')
        self.ctl({'login_no': '[UNAVAILABLE] Temporary System Error'})   # 别的 NO：过一会儿再试，带着 Gmail 的原话
        with self.assertRaises(self.ns['MailRetry']) as e:
            self.box().open()
        self.assertEqual(e.exception.code, 'login')
        self.assertIn('Temporary System Error', e.exception.text)

    def test_tls_is_always_verified(self):
        with mock.patch.dict(os.environ, {'CCA_MAIL_CAFILE': ''}):
            with self.assertRaises(self.ns['MailFatal']) as e:
                self.box().open()
        self.assertEqual(e.exception.code, 'tls')
        with mock.patch.dict(self.ns, {'home_swapped': lambda: False}):   # HOME 是用户真正的主目录：只给测试的 CA 不认
            with self.assertRaises(self.ns['MailFatal']) as e:
                self.box().open()
        self.assertEqual(e.exception.code, 'tls')

    def test_proxy_errors_are_retryable(self):
        for mode in ('refuse', 'auth'):
            self.ctl({'proxy': mode})
            with self.assertRaises(self.ns['MailRetry']) as e:
                self.box(self.socks if mode == 'auth' else self.http).open()
            self.assertEqual(e.exception.code, 'proxy', mode)
        self.ctl({})
        with self.assertRaises(self.ns['MailRetry']) as e:
            self.box(('socks5', '127.0.0.1', 9)).open()
        self.assertEqual(e.exception.code, 'net')

    def test_no_all_mail_falls_back_to_inbox_and_odd_fetch_order(self):
        self.ctl({'no_all': True, 'uid_after_literal': True})
        self.put(2, mf.build_eml('alice@example.com'), 10, folder='inbox')
        b = self.box()
        b.open()
        self.assertEqual(b.folders(), [('收件箱', 'INBOX'), ('垃圾邮件', '"[Gmail]/Spam"')])
        b.examine('INBOX')
        self.assertEqual(sorted(b.headers((1, 5))), [2])

    def test_new_mail_found_even_if_gmail_search_lags(self):
        self.ctl({'index_lag': 3600})   # 刚到的邮件 SEARCH 一律搜不到
        src = self.ns['MailSource']('inbox@example.com', PW, self.socks, [self.rule])
        self.addCleanup(src.close)
        self.assertEqual(src.poll(time.time() - 60), [])
        self.put(1, mf.build_eml('alice@example.com'))
        out = src.poll(time.time() - 60)
        self.assertEqual([c['uid'] for c in out], [1])
        self.assertNotIn('VIOLATION', self.log())

    def test_dropped_connection_reconnects(self):
        self.put(1, mf.build_eml('alice@example.com'), 5)
        self.ctl({'drop': 'UID SEARCH'})
        src = self.ns['MailSource']('inbox@example.com', PW, self.socks, [self.rule])
        self.addCleanup(src.close)
        with self.assertRaises(self.ns['MailRetry']):
            src.poll(time.time() - 60)
        self.ctl({})
        self.assertEqual([c['uid'] for c in src.poll(time.time() - 60)], [1])


class Commands(WireBase):
    """cca mail setup / test / open 和登录时盯邮箱（在这个进程里调 PY_SRC 的命令；打开链接的 cca _mail-open 换成记下链接的小脚本）"""

    def setUp(self):
        WireBase.setUp(self)
        self.opened = Path(self.temp.name) / 'opened.log'
        fake = Path(self.temp.name) / 'fake-cca'
        fake.write_text('#!/bin/sh\n[ "$1" = _mail-open ] || exit 9\nIFS= read -r url\nprintf "%%s %%s\\n" "$2" "$url" >> %s\n'
                        'exit "$(cat %s 2>/dev/null || echo 0)"\n' % (self.opened, Path(self.temp.name) / 'rc'))
        fake.chmod(0o755)
        os.environ['CCA_SELF'] = str(fake)
        self.ns['SELF_PATH'] = str(fake)
        (self.home / '.cca' / 'accounts.json').write_text(json.dumps({'alice@example.com': 'US-A', '工作(bob@example.com)': 'US-B'}))

    def run_cmd(self, name, *args, stdin=''):
        out, err = io.StringIO(), io.StringIO()
        code = 0
        with mock.patch.object(sys, 'stdin', io.StringIO(stdin)), contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                self.ns[name](*args)
            except SystemExit as e:
                code = e.code if isinstance(e.code, int) else 1
        return code, out.getvalue(), err.getvalue()

    def setup_ok(self):
        code, _, err = self.run_cmd('cmd_mail_setup', 'Inbox@Example.com', 'socks5://127.0.0.1:%d' % self.px.port, stdin='abcd efgh ijkl mnop\n')
        self.assertEqual(code, 0, err)
        return err

    def test_setup_checks_then_saves(self):
        code, _, err = self.run_cmd('cmd_mail_setup', 'inbox@example.com', 'socks5://127.0.0.1:%d' % self.px.port, stdin='wrongwrongwrongw')
        self.assertEqual(code, 3)
        self.assertIn('应用专用密码', err)
        self.assertFalse((self.home / '.cca' / 'secrets' / 'cca-mail').exists(), '没登录上不存')
        self.assertEqual(self.run_cmd('cmd_mail_setup', 'not-an-address', '', stdin=PW)[0], 2)
        self.assertEqual(self.run_cmd('cmd_mail_setup', 'a;b@example.com', '', stdin=PW)[0], 2)
        self.assertEqual(self.run_cmd('cmd_mail_setup', 'inbox@example.com', 'socks5://127.0.0.1:9', stdin=PW)[0], 4)
        err = self.setup_ok()
        self.assertIn('所有邮件、垃圾邮件', err)
        c = json.loads(Path(self.home / '.cca' / 'config.json').read_text())
        self.assertEqual(c['mail'], {'user': 'inbox@example.com', 'proxy': 'socks5://127.0.0.1:%d' % self.px.port})
        f = self.home / '.cca' / 'secrets' / 'cca-mail'
        self.assertEqual(stat.S_IMODE(f.stat().st_mode), 0o600)
        self.assertEqual(json.loads(Path(f).read_text())['key'], PW, '空格去掉了')
        self.assertEqual(self.ns['ks_read']('cca-mail', 'inbox@example.com'), (0, PW))
        self.assertEqual(self.ns['ks_read']('cca-mail', 'other@example.com')[1], '', '换了地址取不到旧的')
        code, out, _ = self.run_cmd('cmd_mail_status')
        self.assertIn('inbox@example.com', out)
        self.assertIn('自动打开', out)
        self.assertEqual(self.run_cmd('cmd_mail_get_user')[1], 'inbox@example.com\n')

    def test_new_address_replaces_the_old_only_once_saved(self):
        self.setup_ok()
        read = lambda u: self.ns['ks_read']('cca-mail', u)[1]
        user = lambda: self.ns['mail_cfg'](self.ns['cfg_load']())['user']
        self.ctl({'user': 'new@example.com', 'password': PW})
        with mock.patch.dict(self.ns, {'ks_write': lambda *a: False}):   # 存不进去（钥匙串锁着、磁盘满了）
            code, _, err = self.run_cmd('cmd_mail_setup', 'new@example.com', '', stdin=PW)
        self.assertEqual(code, 5, err)
        self.assertEqual((read('inbox@example.com'), user()), (PW, 'inbox@example.com'), '原来的密码、设置都还在，照样能用')
        self.assertEqual(self.run_cmd('cmd_mail_setup', 'new@example.com', '', stdin=PW)[0], 0)
        self.assertEqual((read('new@example.com'), read('inbox@example.com'), user()), (PW, '', 'new@example.com'))

    def test_setup_when_gmail_refuses_for_now(self):
        self.ctl({'login_no': '[UNAVAILABLE] Temporary System Error'})
        code, _, err = self.run_cmd('cmd_mail_setup', 'inbox@example.com', 'socks5://127.0.0.1:%d' % self.px.port, stdin=PW)
        self.assertEqual(code, 4, '不是密码不对：不重问密码')
        self.assertIn('Gmail 这次没让登录（[UNAVAILABLE] Temporary System Error）', err)
        self.assertNotIn('连 Gmail 的方式可以指定', err)

    def test_setup_keeps_auto_off(self):
        c = self.ns['cfg_load']()
        c['mail'] = {'user': 'inbox@example.com', 'auto': False}
        self.ns['cfg_save'](c)
        self.setup_ok()
        self.assertIs(json.loads(Path(self.home / '.cca' / 'config.json').read_text())['mail']['auto'], False)

    def test_mail_test_lists_without_showing_links(self):
        self.setup_ok()
        self.put(5, mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-SECRET1'), 120)
        self.put(6, mf.build_eml('carol@example.com', link='https://claude.ai/magic-link#tok-SECRET2'), 60)
        self.put(7, mf.build_eml('alice@example.com', ar='fail', link='https://claude.ai/magic-link#tok-SECRET3'), 30)
        self.put(8, mf.build_eml('alice@example.com', subject='Secure link to log in to Claude.ai \x1b[2K\x1b[1A', ar='fail',
                                 frm='"\x1b]52;c;eA==\x07" <x@mail.anthropic.com>'), 20)
        code, out, err = self.run_cmd('cmd_mail_test', '2')
        self.assertEqual(code, 0, err)
        self.assertNotIn('SECRET', out + err)
        self.assertNotIn('\x1b', out, '发件人写的转义序列不原样打到终端')
        self.assertIn('有 4 封', out)
        self.assertIn('通过核对，链接 https://claude.ai/magic-link（# 后面', out)
        self.assertIn('不是账号表里的邮箱', out)
        self.assertIn('Gmail 的验证结果里没有 Anthropic 的签名通过', out)
        self.assertNotIn('VIOLATION', self.log())

    def test_open_hands_the_newest_link_over_stdin_once(self):
        self.setup_ok()
        self.put(5, mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-A1'), 600)
        self.put(6, mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-A2'), 300)
        self.put(7, mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-OLD'), 3 * 3600)
        code, _, err = self.run_cmd('cmd_mail_open', 'alice@example.com', '0')
        self.assertEqual(code, 0, err)
        self.assertEqual(self.opened.read_text(), 'alice@example.com https://claude.ai/magic-link#tok-A2\n')
        code, _, err = self.run_cmd('cmd_mail_open', 'alice@example.com', '0')
        self.assertEqual(code, 1)
        self.assertIn('已经打开过了', err)
        code, _, err = self.run_cmd('cmd_mail_open', 'alice@example.com', '0', 'again')
        self.assertEqual(code, 0, err)
        self.assertEqual(self.opened.read_text().count('tok-A2'), 2)
        self.assertNotIn('tok-', err)
        code, _, err = self.run_cmd('cmd_mail_open', '工作(bob@example.com)', '0')
        self.assertEqual(code, 1)
        self.assertIn('最近 30 分钟里没有 bob@example.com 的登录邮件', err)
        self.assertNotIn('VIOLATION', self.log())

    def test_open_waits_for_a_new_mail(self):
        self.setup_ok()
        threading.Timer(0.6, lambda: self.put(9, mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-W'))).start()
        code, _, err = self.run_cmd('cmd_mail_open', 'alice@example.com', '20')
        self.assertEqual(code, 0, err)
        self.assertIn('等 alice@example.com 的登录邮件', err)
        self.assertIn('tok-W', self.opened.read_text())

    def test_open_reports_browser_failure(self):
        self.setup_ok()
        (Path(self.temp.name) / 'rc').write_text('3')
        self.put(5, mf.build_eml('alice@example.com'), 10)
        code, _, err = self.run_cmd('cmd_mail_open', 'alice@example.com', '0')
        self.assertEqual(code, 1)
        self.assertIn('没能打开浏览器', err)
        self.assertEqual(len(self.opened.read_text().splitlines()), 3)
        (Path(self.temp.name) / 'rc').write_text('0')
        self.assertEqual(self.run_cmd('cmd_mail_open', 'alice@example.com', '0')[0], 0, '没用掉：修好后照样能开')

    def test_watch_opens_the_mail_that_arrives_during_login(self):
        self.setup_ok()
        self.put(4, mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-before'), 120)
        done = threading.Event()
        threading.Timer(0.5, lambda: self.put(5, mf.build_eml('bob@example.com', link='https://claude.ai/magic-link#tok-bob'))).start()
        threading.Timer(0.8, lambda: self.put(6, mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-login'))).start()

        def alive():
            if self.opened.exists() and 'tok-login' in self.opened.read_text():
                threading.Timer(0.5, done.set).start()
            return not done.is_set()
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            self.ns['mail_watch']('alice@example.com', alive)
        self.assertEqual(self.opened.read_text(), 'alice@example.com https://claude.ai/magic-link#tok-login\n',
                         '登录开始前就到了的、别的账号的都不开')
        self.assertIn('已连上 inbox@example.com', err.getvalue())
        self.assertNotIn('没通过核对', err.getvalue(), '别的账号的登录邮件不说')
        self.assertIn('打开了登录邮件里的链接', json.loads(Path(self.home / '.cca' / 'mail' / 'last.json').read_text())['text'])

    def test_watch_does_nothing_without_config_or_when_auto_is_off(self):
        self.ns['mail_watch']('alice@example.com', lambda: True)   # 没配置：马上返回
        self.setup_ok()
        c = self.ns['cfg_load']()
        c['mail']['auto'] = False
        self.ns['cfg_save'](c)
        self.ns['mail_watch']('alice@example.com', lambda: True)
        self.assertNotIn('LOGIN', self.log().split('IMAP LOGOUT')[-1])

    def test_forget_removes_everything(self):
        self.setup_ok()
        self.ns['mail_claim']('x')
        self.run_cmd('cmd_mail_forget')
        self.assertFalse((self.home / '.cca' / 'secrets' / 'cca-mail').exists())
        self.assertFalse((self.home / '.cca' / 'mail').exists())
        self.assertNotIn('mail', json.loads(Path(self.home / '.cca' / 'config.json').read_text()))
        self.assertEqual(self.run_cmd('cmd_mail_forget')[0], 0, '没配置过也照样能跑')


if __name__ == '__main__':
    unittest.main()
