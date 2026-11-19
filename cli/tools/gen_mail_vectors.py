#!/usr/bin/env python3
"""生成 cli/tests/vectors/mail.json：邮箱助手（cli/src/cmd/mail.rs 和 mail/ 下各文件）的对照数据。原来的实现靠 Python 标准库的
email、html.parser、imaplib、codecs、fnmatch，Rust 那边是照它们的行为手写的：这里把一批输入交给真的 Python，结果存下来，
cargo test 照着核对（不用装 Python）。

    python3 -I cli/tools/gen_mail_vectors.py cli/tests/vectors                      写 mail.json（提交）
    python3 -I cli/tools/gen_mail_vectors.py --fuzz 20 --seed 7 /tmp/extra.json     另外一批随机输入（量是平时的 20 倍，不提交）：
    CCA_MAIL_VECTORS_EXTRA=/tmp/extra.json cargo test mail                          单元测试把它接在 mail.json 后面一起核对

「参照实现」一节是从 cca 的 PY_SRC 里原样抄来的函数（邮箱助手里不碰网络的那些，加上 MailBox、盯邮箱的 mail_hunt 和它记
打开过哪些邮件的那几个——那几个写的是临时目录）：cca 里还有 PY_SRC 的时候，每次运行都核对抄的和原文一字不差（主干改了原文，
这里要跟着改）。切换到只有 Rust 以后它们就是规格的存档。以后并进 gen_vectors.py。

不碰真东西：只连本机上自己起的假 IMAP 服务器（imap 一节），写的文件都在临时目录里；limit_paths 一节把 PY_SRC 拷到临时目录、
在假的 HOME 里起它（cca 里没有 PY_SRC 了就沿用上一次量的）。整份跑一遍一分钟左右。

字节在 JSON 里写成 Latin-1 的文字（一个字符一个字节）。结果随 Python 的版本变（html.parser、email 各版本改过）：
Rust 这边固定成生成这份文件的那个版本。"""
import codecs
import encodings
import encodings.aliases
import fcntl
import fnmatch
import hashlib
import inspect
import json
import os
import pkgutil
import random
import re
import shutil
import socket
import sys
import tempfile
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))


class lazy_re:
    """PY_SRC 里的同名小类：正则用到时才编译"""
    __slots__ = ('args', 'rx')

    def __init__(self, *args):
        self.args, self.rx = args, None

    def __getattr__(self, name):
        if self.rx is None:
            self.rx = re.compile(*self.args)
        return getattr(self.rx, name)


# --- 参照实现：原样抄自 cca 的 PY_SRC（常量在前，函数照原文的先后）-----------------------------------------------------
MAIL_HOST, MAIL_PORT = 'imap.gmail.com', 993
MAIL_SKEW = 60        # 两边时钟的误差：登录开始前一分钟内收到的也算
MAIL_FETCH_MAX = 262144   # 一封邮件最多取这么多字节（登录邮件只有几十 KB）
MAIL_DOMAIN_RE = lazy_re(r'^[a-z0-9-]+(\.[a-z0-9-]+)+$')
MAIL_ADDR_RE = lazy_re(r'^[a-z0-9._%+-]+@[a-z0-9-]+(\.[a-z0-9-]+)+$')   # Gmail 地址（小写；也是钥匙串的账户名，交给 security -i）
MAIL_HDRS = ('FROM', 'TO', 'SUBJECT', 'REPLY-TO', 'MESSAGE-ID', 'AUTHENTICATION-RESULTS', 'DKIM-SIGNATURE')   # 先只取这些头来核对（Reply-To 给信任中转的判定：中转标注原件来自 Anthropic）
MAIL_SIGNED = ('from', 'to', 'subject')   # 这几个头必须在 Anthropic 的签名里、各只有一个：签名之外的头谁都能改、能加
MAIL_Q = '\x22'       # IMAP 里的双引号
MAIL_MONTHS = ('Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec')
MAIL_ORDER = ('old', 'subject', 'dup', 'from', 'dkim', 'unsigned', 'to', 'nolink')
MAIL_LIST_RE = lazy_re(r'^\(([^)]*)\) (?:\x22(?:[^\x22\\]|\\.)*\x22|NIL) (\x22(?:[^\x22\\]|\\.)*\x22|[^\s\x22()]+)$')
MAIL_WHY = {   # 没通过核对的原因（按核对的先后）
    'old': '是这次登录开始以前收到的',
    'subject': '标题不像登录邮件',
    'dup': '发件人、收件人或标题的邮件头有好几个（邮件被加过东西）',
    'from': '发件人不是 Anthropic',
    'dkim': 'Gmail 的验证结果里没有 Anthropic 的签名通过（转发时邮件被改过，或者不是 Anthropic 发的）',
    'unsigned': '发件人、收件人、标题不全在 Anthropic 的签名里，认不出是不是发给这个账号的',
    'to': '收件人不是这个账号的邮箱',
    'nolink': '邮件里没找到登录链接（邮件的格式变了？）',
    'refused': '打开之前再核对一遍链接时没通过',   # cca _mail-open 不肯打开（不是 mail_judge 的结果，不进 MAIL_ORDER）
}
EMAIL_RE = lazy_re(r'^[^@\s"\\()（）]+@[^@\s"\\()（）]+\.[^@\s"\\()（）]+$')
PAIR_RE = lazy_re(r'^(.*?)[(（]([^()（）]*)[)）]$')


class MailRule:
    """一类登录邮件的认法：MAIL_RULES 里一条一项（以后加 Console 的登录链接也是加一条规则；只认链接，邮件里的验证码装不下）"""

    def __init__(self, name, what, domains, subjects, search, hosts, paths):
        self.name, self.what = name, what
        self.domains = domains     # 发件人和 DKIM 签名的域名（按标签认后缀：mail.anthropic.com 算，evilanthropic.com 不算）
        self.subjects = subjects   # 标题（正则，不分大小写）
        self.search = search       # 在 Gmail 上粗筛的 IMAP 搜索条件（只为少取几封，本地照样逐项核对）
        self.hosts, self.paths = hosts, paths   # 链接的主机（原样相等）和路径（原样相等；/ 结尾的按前缀）

    def domain_ok(self, d):
        d = (d or '').lower()
        return any(d == x or d.endswith('.' + x) for x in self.domains)

    def subject_ok(self, s):
        return any(re.search(p, s or '', re.I) for p in self.subjects)

    def link_ok(self, url):
        """是不是这一类的登录链接：https、主机原样相等（挡住 claude.ai.evil.com、claude.ai@evil.com、带端口、大写这些写法）、
        路径在名单里；只有可见的 ASCII，没有引号、尖括号、反斜杠，不超过 2048 个字符"""
        from urllib.parse import urlsplit
        if (not isinstance(url, str) or len(url) > 2048 or not re.match(r'^[\x21-\x7e]+$', url)
                or re.search(r'[\x22\x27<>\\]', url)):
            return False
        try:
            u = urlsplit(url)
        except ValueError:
            return False
        return (u.scheme == 'https' and u.netloc in self.hosts
                and any(u.path == p or (p.endswith('/') and u.path.startswith(p)) for p in self.paths))


class MailError(Exception):
    """连 Gmail 出的错：code 是 net、proxy、tls、auth、imap、login、proto 之一，text 给人看"""

    def __init__(self, code, text):
        Exception.__init__(self, text)
        self.code, self.text = code, text


class MailRetry(MailError):
    """过一会儿可能就好了（网络、代理、Gmail 那边断开）：整个会话丢掉，等一会儿重连"""


class MailFatal(MailError):
    """不会自己好：密码不对、没开 IMAP、证书校验不过、连接方式写错了"""


def mail_clean(s):
    """别人写的文字（邮件头、Gmail 的回话）打到终端以前：控制字符（C0、DEL、C1）和看不见的、改变显示顺序的格式字符换成空格，
    免得里面的转义序列擦掉、改写前面的输出"""
    return re.sub(r'[\x00-\x1f\x7f-\x9f​-‏‪-‮⁠-⁩﻿]', ' ', str(s))


def mail_oserr(e):
    """出错的原因（给人看的一小段：一行、不带控制字符，imaplib 的 b'…' 去掉）"""
    s = str(getattr(e, 'strerror', None) or e or type(e).__name__)
    s = re.sub(r'^b[\x22\x27](.*)[\x22\x27]$', r'\1', s)
    return ' '.join(mail_clean(s).split())[:160] or type(e).__name__


def mail_proxy_parse(text):
    """scutil --proxy 的输出 → (顶层的键值, 例外列表)：只认两格缩进的顶层「键 : 值」（__SCOPED__ 里按网卡分的不管）"""
    kv, exc, inexc = {}, [], False
    for line in text.splitlines():
        if inexc:
            m = re.match(r'^    \d+ : (.+)$', line)
            if m:
                exc.append(m.group(1).strip())
                continue
            inexc = False
        m = re.match(r'^  (\w+) : (.*)$', line)
        if m:
            if m.group(1) == 'ExceptionsList':
                inexc = True
            else:
                kv[m.group(1)] = m.group(2).strip()
    return kv, exc


def mail_bypass(host, pats):
    """系统代理的例外里有没有这个主机（*.gmail.com、.gmail.com、imap.gmail.com 这些写法）"""
    import fnmatch
    for p in pats:
        p = p.strip().lower()
        if p and (fnmatch.fnmatchcase(host, p) or (p.startswith('.') and host.endswith(p))):
            return True
    return False


def mail_route_note(r):
    return '直连' if r[0] == 'direct' else '经代理 %s:%d，%s' % (r[1], r[2], 'SOCKS5' if r[0] == 'socks5' else 'HTTP CONNECT')


def mail_epoch(s):
    """INTERNALDATE（15-Nov-2026 10:00:00 +0800：Gmail 收到这封邮件的时间，发件人写不了）→ 时间戳；认不出是 None"""
    import calendar
    m = re.match(r'^\s*(\d{1,2})-([A-Za-z]{3})-(\d{4}) (\d\d):(\d\d):(\d\d) ([+-])(\d\d)(\d\d)$', s)
    if not m or m.group(2).title() not in MAIL_MONTHS:
        return None
    t = calendar.timegm((int(m.group(3)), MAIL_MONTHS.index(m.group(2).title()) + 1, int(m.group(1)),
                         int(m.group(4)), int(m.group(5)), int(m.group(6))))
    off = (int(m.group(8)) * 60 + int(m.group(9))) * 60
    return t - off if m.group(7) == '+' else t + off


def mail_since(t):
    """IMAP 的 SINCE（只到天、不看时区）：往前多算一天，本地再按 INTERNALDATE 细比"""
    g = time.gmtime(t - 86400)
    return '%02d-%s-%d' % (g.tm_mday, MAIL_MONTHS[g.tm_mon - 1], g.tm_year)


def mail_crit(rules, since):
    """Gmail 上粗筛的搜索条件：SINCE 加上各条规则的条件（好几条时 OR 起来）"""
    terms = [list(r.search) for r in rules]
    while len(terms) > 1:
        a, b = terms.pop(0), terms.pop(0)
        terms.insert(0, ['OR', '(%s)' % ' '.join(a), '(%s)' % ' '.join(b)])
    return ['SINCE', mail_since(since)] + (terms[0] if terms else [])


def mail_fetch_parse(data):
    """UID FETCH 的回应 → {uid: (INTERNALDATE 的时间戳, literal 的字节)}：各项的先后不定，UID 也可能写在 literal 后面"""
    recs, out = [], {}
    for item in data or []:
        if isinstance(item, tuple) and len(item) == 2:
            recs.append([item[0].decode('latin-1', 'replace'), item[1]])
        elif recs and isinstance(item, bytes) and not re.match(rb'^\d+ \(', item):
            recs[-1][0] += ' ' + item.decode('latin-1', 'replace')
    for meta, lit in recs:
        u = re.search(r'\bUID (\d+)', meta)
        d = re.search(r'\bINTERNALDATE \x22([^\x22]+)\x22', meta)
        t = mail_epoch(d.group(1)) if d else None
        if u and t is not None and isinstance(lit, bytes):
            out[int(u.group(1))] = (t, lit)
    return out


def mail_parse(raw):
    from email import message_from_bytes
    return message_from_bytes(raw or b'')


def mail_text(v):
    """邮件头的值（可能是 =?utf-8?…?= 编码的）→ 文字，一行、不带控制字符（mail_clean）"""
    from email.header import decode_header, make_header
    try:
        s = str(make_header(decode_header(str(v or ''))))
    except Exception:   # 头是别人写的：解不开就用原样的
        s = str(v or '')
    return ' '.join(mail_clean(s).split())


def mail_addrs(msg, name):
    from email.utils import getaddresses
    return [a.lower() for _, a in getaddresses([str(x) for x in (msg.get_all(name) or [])]) if '@' in a]


def mail_tags(v):
    """DKIM-Signature 的 tag=value（值去掉空白）"""
    out = {}
    for part in str(v or '').split(';'):
        k, _, val = part.partition('=')
        k = k.strip().lower()
        if k:
            out.setdefault(k, re.sub(r'\s+', '', val))
    return out


def mail_ar_items(v):
    """Authentication-Results 的值按 RFC 8601 切开：(authserv-id, [[(名字, 值), …], …])，每项结果的第一对是 method=result，
    后面是 ptype.property=value（名字小写）。注释（可以嵌套、带反斜杠转义）去掉；引号里的整段算值的一部分，里面的分号、等号、
    括号都不算（smtp.mailfrom 的本地部分是发件人写的，Gmail 照抄）。写法不对（引号、括号不配对，名字=值对不上）是 None：整条不信"""
    segs, cur, tok, depth, i = [], [], '', 0, 0
    while i < len(v):
        ch = v[i]
        if depth:   # 注释里
            if ch == '\\':
                i += 1
            depth += {'(': 1, ')': -1}.get(ch, 0)
        elif ch == '\x22':
            j = i + 1
            while j < len(v) and v[j] != '\x22':
                j += 2 if v[j] == '\\' else 1
            if j >= len(v):
                return None
            tok += v[i:j + 1]
            i = j
        elif ch == '(' or ch in ';=' or ch.isspace():
            if tok:
                cur.append(tok)
                tok = ''
            if ch == '(':
                depth = 1
            elif ch == '=':
                cur.append('=')
            elif ch == ';':
                segs.append(cur)
                cur = []
        else:
            tok += ch
        i += 1
    if depth:
        return None
    if tok:
        cur.append(tok)
    segs.append(cur)
    if not segs[0] or '=' in segs[0]:
        return None
    out = []
    for seg in segs[1:]:
        if not seg or seg == ['none']:
            continue
        if len(seg) % 3 or any(seg[k + 1] != '=' or '=' in (seg[k], seg[k + 2]) for k in range(0, len(seg), 3)):
            return None
        out.append([(seg[k].lower(), seg[k + 2]) for k in range(0, len(seg), 3)])
    return segs[0][0].lower(), out


def mail_ar(msg):
    """最上面那一条 Authentication-Results 里的 DKIM 结果 [(结果, 域名, header.b)]（Gmail 收信时把它加在最上面，邮件里原有的、
    发件人写的都在它下面）：它不是 mx.google.com 写的、写法不对时是 None。域名取 header.d，没有时取 header.i 的 @ 后面；
    名字重复、域名或 header.b 的样子不对（header.b 至少 8 个 base64 字符）的那一项记成 ?，不算通过"""
    ars = msg.get_all('Authentication-Results') or []
    items = mail_ar_items(' '.join(str(ars[0]).split())) if ars else None
    if not items or items[0] != 'mx.google.com':
        return None
    out = []
    for pairs in items[1]:
        if pairs[0][0] != 'dkim':
            continue
        p = dict(pairs[1:])
        i = re.match(r'^[A-Za-z0-9._+-]*@(.+)$', p.get('header.i', ''))
        d = (p.get('header.d') or (i.group(1) if i else '')).lower()
        b = p.get('header.b', '')
        ok = len(p) == len(pairs) - 1 and MAIL_DOMAIN_RE.match(d) and re.match(r'^[A-Za-z0-9+/]{8,}$', b)
        out.append((pairs[0][1].lower(), d, b) if ok else ('?', '', ''))
    return out


def mail_fwd_ok(d, forwarder):
    """域名 d 是不是信任的中转 forwarder（按标签认后缀，和 domain_ok 一样）"""
    d = (d or '').lower()
    f = (forwarder or '').lower()
    return bool(f) and (d == f or d.endswith('.' + f))


def mail_from_alias(msg):
    """From 的显示名里的邮箱（信任中转把原始别名放在显示名里，像 "alias@x" <noreply@fwd>，用来认是哪个账号）；没有是空字符串"""
    from email.utils import parseaddr
    name = parseaddr(str(msg.get('From') or ''))[0].strip().lower()
    return name if MAIL_ADDR_RE.match(name) else ''


def mail_signed_for(msg, dom_ok):
    """Gmail 验过的、dom_ok(域名) 为真的那个 DKIM 签名覆盖了哪些头（小写的集合）；没有是 None。按 header.b（签名的开头几个字符）
    认是哪个签名；对上好几个时分不清，不认"""
    for res, dom, b in mail_ar(msg) or []:
        if res != 'pass' or not dom_ok(dom):
            continue
        sigs = [tg for tg in (mail_tags(v) for v in msg.get_all('DKIM-Signature') or [])
                if dom_ok(tg.get('d')) and (dom == (tg.get('d') or '').lower() or dom.endswith('.' + (tg.get('d') or '').lower()))
                and tg.get('b', '').startswith(b)]
        if len(sigs) == 1:
            return {h.strip().lower() for h in sigs[0].get('h', '').split(':') if h.strip()}
    return None


def mail_signed(msg, rule):
    return mail_signed_for(msg, rule.domain_ok)


def mail_judge(rule, msg, got, acct, since, forwarder=None):
    """这封邮件是不是 acct（小写的邮箱）这一次登录的登录邮件：是返回 None，不是返回原因（MAIL_WHY 的键）。got 是 Gmail 收到它的时间。
    forwarder 给定时（这个账号开了「信任转发中转」，默认不开）：From 是 Anthropic 时照严格路走，From 是这个中转时走 mail_judge_fwd"""
    if got < since - MAIL_SKEW:
        return 'old'
    if not rule.subject_ok(mail_text(msg.get('Subject'))):
        return 'subject'
    if any(len(msg.get_all(h) or []) > 1 for h in MAIL_SIGNED):
        return 'dup'
    frm = mail_addrs(msg, 'From')
    if len(frm) != 1:
        return 'from'
    dom = frm[0].rpartition('@')[2]
    if rule.domain_ok(dom):
        signed = mail_signed(msg, rule)
        if signed is None:
            return 'dkim'
        if not set(MAIL_SIGNED) <= signed:
            return 'unsigned'
        if not acct or acct not in mail_addrs(msg, 'To'):
            return 'to'
        return None
    if forwarder and mail_fwd_ok(dom, forwarder):
        return mail_judge_fwd(rule, msg, acct, forwarder)
    return 'from'


def mail_judge_fwd(rule, msg, acct, forwarder):
    """信任的中转 forwarder 转发的登录邮件（账号显式开了「信任转发中转」，不安全、默认关）：中转没有 Anthropic 的直签了，所以靠
    「中转的 DKIM 覆盖 From/To/Subject/Reply-To（中转之后改不了这几项）+ From 显示名是这个账号的别名（路由）+ Reply-To 是单个
    Anthropic 地址（中转标注原件来自 Anthropic）」。安全边界只到「信任这个中转 + 别名不泄露」：别名泄露，知道的人可经中转注入登录链接。
    返回 None 放行，否则原因（MAIL_WHY 的键）"""
    signed = mail_signed_for(msg, lambda d: mail_fwd_ok(d, forwarder))
    if signed is None or not {'from', 'to', 'subject', 'reply-to'} <= signed:
        return 'dkim'
    if not acct or mail_from_alias(msg) != acct:
        return 'to'
    rt = mail_addrs(msg, 'Reply-To')
    if len(rt) != 1 or not rule.domain_ok(rt[0].rpartition('@')[2]):
        return 'from'
    return None


def mail_judge_any(rules, msg, got, acct, since, forwarder=None):
    """(过了的那条规则, None)；都没过时 (核对得最远的那条, 原因)"""
    best = (rules[0] if rules else None, 'subject')
    for r in rules:
        why = mail_judge(r, msg, got, acct, since, forwarder)
        if why is None:
            return r, None
        if MAIL_ORDER.index(why) > MAIL_ORDER.index(best[1]):
            best = (r, why)
    return best


def mail_links(msg):
    """正文里的链接（HTML 的 <a href>、纯文本里的网址），按出现的先后；附件里的不算"""
    from html.parser import HTMLParser
    links = []

    class Anchors(HTMLParser):
        def handle_starttag(self, tag, attrs):
            if tag == 'a':
                links.extend(v for k, v in attrs if k == 'href' and v)

    for part in msg.walk():
        if part.is_multipart() or part.get_content_disposition() == 'attachment':
            continue
        ct = part.get_content_type()
        if ct not in ('text/html', 'text/plain'):
            continue
        raw = part.get_payload(decode=True) or b''
        try:
            text = raw.decode(part.get_content_charset() or 'utf-8', 'replace')
        except LookupError:
            text = raw.decode('utf-8', 'replace')
        try:
            if ct == 'text/html':
                p = Anchors()
                p.feed(text)
                p.close()
            else:
                links.extend(re.findall(r'https://[^\s<>\x22\x27\\]+', text))
        except Exception:   # 正文是别人写的：解析不了就当没有链接
            pass
    return [l.strip() for l in links]


def mail_extract(rule, msg):
    return next((u for u in mail_links(msg) if rule.link_ok(u)), None)


def mail_shape(url):
    """链接的样子（给 cca mail test 看）：协议、主机和路径，后面的部分只说有多长，不打出来"""
    from urllib.parse import urlsplit
    try:
        u = urlsplit(url)
    except ValueError:
        return '（认不出的链接，%d 个字符）' % len(url)
    rest = '、'.join('%s 后面 %d 个字符' % (k, len(v)) for k, v in (('?', u.query), ('#', u.fragment)) if v)
    return '%s://%s%s%s' % (u.scheme, u.netloc, u.path, '（%s，不显示）' % rest if rest else '')


def mail_digest(msg, cand):
    mid = ' '.join(str(msg.get('Message-ID') or '').split()) or '%s|%s' % (cand['folder'], cand['uid'])
    return hashlib.sha256(('%s\n%d' % (mid, int(cand['got']))).encode('utf-8', 'replace')).hexdigest()[:16]


def mail_handle(s):
    """--msg 的「文件夹:uid:uidvalidity」→ (folder, uid, uidvalidity)：文件夹名不含冒号（Gmail 的 INBOX、[Gmail]/All Mail 都不含），从右边切两刀"""
    parts = s.rsplit(':', 2)
    if len(parts) != 3 or not parts[0] or not parts[1].isdigit() or not parts[2].isdigit():
        raise ValueError(s)
    return parts[0], int(parts[1]), int(parts[2])


def mail_imap(sock):
    """imaplib 的会话，用连好、做好 TLS 的套接字（3.8 到 3.14 的 IMAP4 都经 _create_socket 拿套接字，再按各自的版本包装）"""
    import imaplib

    class Imap(imaplib.IMAP4):
        def _create_socket(self, timeout=None):
            return sock
    return Imap(MAIL_HOST, MAIL_PORT)


class MailBox:
    """登录好的 Gmail 会话，只读：只用 LOGIN、CAPABILITY、LIST、EXAMINE、UID SEARCH、UID FETCH 的 BODY.PEEK（不标已读）、LOGOUT。
    不 SELECT、不 STORE、不删不移，也没有通用的命令入口。出错时整个会话丢掉（超时以后 imaplib 读不回来），下次重连"""

    def __init__(self, user, pw, route, timeout=20):
        self.user, self.pw, self.route, self.timeout = user, pw, route, timeout
        self.im = None

    def open(self):
        import imaplib
        s = mail_tls(mail_dial(self.route), self.timeout)
        try:
            self.im = mail_imap(s)
        except (OSError, imaplib.IMAP4.error) as e:
            s.close()
            raise MailRetry('net', 'Gmail 没有正常应答（%s）' % mail_oserr(e))
        try:
            self.im.login(self.user, self.pw)
        except (imaplib.IMAP4.abort, OSError) as e:   # abort 是 error 的子类：先认
            self.close()
            raise MailRetry('net', '和 Gmail 的连接断了（%s）' % mail_oserr(e))
        except imaplib.IMAP4.error as e:   # NO：按 Gmail 的原话分，认不出的当作过一会儿再试（临时出错、同时连得太多……）
            self.close()
            why = mail_oserr(e)
            if re.search(r'not enabled for IMAP|IMAP (access )?is disabled', why, re.I):
                raise MailFatal('imap', '这个 Gmail 没开 IMAP：Gmail 的设置 → 转发和 POP/IMAP → 启用 IMAP（公司的邮箱要管理员允许）')
            if re.search(r'AUTHENTICATIONFAILED|Invalid credentials|Application-specific password required', why, re.I):
                raise MailFatal('auth', 'Gmail 不认这个密码：要用应用专用密码（16 个字母），不是 Gmail 的登录密码；没开两步验证的账号建不了应用专用密码')
            raise MailRetry('login', 'Gmail 这次没让登录（%s）' % why)

    def cmd(self, fn, *args):
        import imaplib
        if self.im is None:
            raise MailRetry('net', '和 Gmail 的连接断了')
        try:
            typ, data = fn(*args)
        except (imaplib.IMAP4.abort, OSError) as e:   # abort 是 error 的子类：先认
            self.close()
            raise MailRetry('net', '和 Gmail 的连接断了（%s）' % mail_oserr(e))
        except imaplib.IMAP4.error as e:   # BAD：Gmail 不认这个请求
            self.close()
            raise MailRetry('proto', 'Gmail 不认 cca 的请求（%s）' % mail_oserr(e))
        if typ != 'OK':
            self.close()
            raise MailRetry('proto', 'Gmail 不认 cca 的请求（%s）' % mail_oserr(data[-1] if data else typ))
        return data

    def folders(self):
        """要看的邮件夹 [(给人看的名字, IMAP 里的名字)]：所有邮件和垃圾邮件，按 Gmail 的特殊用途标记（\\All、\\Junk）认，不认名字
        （名字会本地化）；没有 \\All（Gmail 的设置里没让它在 IMAP 里显示）时看收件箱"""
        rows = []
        for item in self.cmd(self.im.list):
            m = MAIL_LIST_RE.match(item.decode('latin-1', 'replace')) if isinstance(item, bytes) else None
            if m:
                rows.append((m.group(1).lower().split(), m.group(2)))
        allm = next((n for a, n in rows if '\\all' in a), None)
        junk = next((n for a, n in rows if '\\junk' in a), None)
        return [('所有邮件', allm) if allm else ('收件箱', 'INBOX')] + ([('垃圾邮件', junk)] if junk else [])

    def examine(self, folder):
        """只读地打开一个邮件夹（EXAMINE），返回 (UIDVALIDITY, UIDNEXT)：没给的是 None"""
        self.cmd(self.im.select, folder, True)
        vals = []
        for code in ('UIDVALIDITY', 'UIDNEXT'):
            d = self.im.response(code)[1]
            v = d[-1] if d else None
            vals.append(int(v) if isinstance(v, bytes) and v.isdigit() else None)
        return tuple(vals)

    def search(self, crit):
        data = self.cmd(self.im.uid, 'SEARCH', *crit)
        return [int(x) for x in b' '.join(d for d in data if isinstance(d, bytes)).split() if x.isdigit()]

    def headers(self, uids):
        """{uid: (Gmail 收到的时间, 头的字节)}；uids 是 UID 的列表，或者 (起, 止) 的区间（含两头）"""
        if isinstance(uids, tuple):
            spec = '%d:%d' % uids
        elif uids:
            spec = ','.join(str(u) for u in uids)
        else:
            return {}
        return mail_fetch_parse(self.cmd(self.im.uid, 'FETCH', spec,
                                         '(UID INTERNALDATE BODY.PEEK[HEADER.FIELDS (%s)])' % ' '.join(MAIL_HDRS)))

    def message(self, uid):
        got = mail_fetch_parse(self.cmd(self.im.uid, 'FETCH', str(uid), '(UID INTERNALDATE BODY.PEEK[]<0.%d>)' % MAIL_FETCH_MAX)).get(uid)
        if not got:
            raise MailRetry('proto', 'Gmail 没给出这封邮件')
        return got[1]

    def close(self, polite=False):
        import imaplib
        im, self.im = self.im, None
        if im is None:
            return
        try:
            if polite:
                im.logout()
            else:
                im.shutdown()
        except (OSError, imaplib.IMAP4.error):
            pass


def mail_knob(name, default):
    """测试用的时间开关（CCA_MAIL_POLL 几秒看一次邮箱、CCA_MAIL_RETRY 连不上 Gmail、浏览器没打开时先等多久）：不是正数时用默认值"""
    try:
        v = float(os.environ.get(name) or default)
    except ValueError:
        v = default
    return v if v > 0 else default


def mail_used(fn):
    os.makedirs(MAIL_DIR, mode=0o700, exist_ok=True)
    with open(os.path.join(MAIL_DIR, 'used.lock'), 'a') as f:
        fcntl.flock(f, fcntl.LOCK_EX)
        p = os.path.join(MAIL_DIR, 'used.json')
        d = load_quiet(p)
        now = time.time()
        d = {k: v for k, v in (d if isinstance(d, dict) else {}).items()
             if isinstance(v, dict) and isinstance(v.get('at'), (int, float)) and now - v['at'] < 2 * 86400}
        before = json.dumps(d, sort_keys=True)
        r = fn(d, now)
        if json.dumps(d, sort_keys=True) != before:
            save_json(p, d, 0o600)
        return r


def mail_claim(dg, again=False):
    """认领这封邮件：返回 False 是不该再打开（打开过、别的 cca 正在打开它），否则是原来的状态（没打开时照它还原）"""
    def f(d, now):
        v = d.get(dg) or {}
        st = v.get('st')
        if st == 'done' and not again:
            return False
        if st == 'claim' and now - v['at'] < 120 and v.get('pid') != os.getpid() and mail_pid_alive(v.get('pid')):
            return False
        d[dg] = {'at': now, 'st': 'claim', 'pid': os.getpid()}
        return st if st == 'done' else None
    return mail_used(f)


def mail_mark(dg, st):
    def f(d, now):
        if st:
            d[dg] = {'at': now, 'st': st}
        else:
            d.pop(dg, None)
    mail_used(f)


def mail_pid_alive(pid):
    try:
        os.kill(int(pid), 0)
    except (OSError, TypeError, ValueError):
        return False
    return True


def mail_nap(secs, end, alive, now, sleep):
    """睡 secs 秒（到 end 为止），每半秒看一次 alive()"""
    until = min(now() + secs, end)
    while now() < until and alive():
        sleep(max(0.01, min(0.5, until - now())))


def mail_verdict(rule, raw, got, acct, since, forwarder=None):
    """取来的全文照样再核对一遍、取出链接：(没通过的原因或 None, 链接或 None, 解析好的邮件)"""
    msg = mail_parse(raw)
    why = mail_judge(rule, msg, got, acct, since, forwarder)
    return why, None if why else mail_extract(rule, msg), msg


def mail_take(src, p, acct, since, deliver, again, forwarder=None):
    """认领、取全文、照全文再核对一遍、取出链接、打开：返回 ok、used（打开过了）、fail（浏览器没打开，链接还没用掉）、refused，
    或者没通过核对的原因。只有打开了才记下（别的照原来的状态还原：下次照样核对、照实说原因）；Gmail 那边出错时往外抛（下一轮再来）"""
    prev = mail_claim(p['dg'], again)
    if prev is False:
        return 'used'
    st = 'fail'
    try:
        why, link, _ = mail_verdict(p['rule'], src.full(p), p['got'], acct, since, forwarder)
        st = why or ('nolink' if not link else deliver(link))
        return st
    finally:
        mail_mark(p['dg'], 'done' if st == 'ok' else prev)


def mail_hunt(src, rules, acct, since, end, deliver, once=False, again=False, alive=None, say=None, hello=None, idle=None,
              now=time.time, sleep=time.sleep, forwarder=None):
    """找 acct 的登录邮件，找到就交给 deliver(链接)（返回 ok、refused、fail）。once：手动（cca mail open），打开了、浏览器打不开就停，
    第一次就连不上马上停；否则一直盯到 end、alive() 为假（登录结束了）。来了好几封只开最新的一封（之前的多半已经失效）；浏览器
    没打开的过一会儿（CCA_MAIL_RETRY，默认 2 秒）再试，一共三次（到了 end 也试完）；这一轮从取头到打开都成了才算连上了（取不到全文的
    那封照连不上退避）。hello：第一次连上时调；idle：第一轮看完还没有能打开的时调（cca mail open
    说要等）。返回 {code: opened | none | fail | 最后连不上时 MailError 的 code, text, opened, used, rejected: {原因: 封数}}"""
    alive = alive or (lambda: True)
    say = say or (lambda s: None)
    poll, base = mail_knob('CCA_MAIL_POLL', 3), mail_knob('CCA_MAIL_RETRY', 2)
    res = {'code': 'none', 'text': '', 'opened': 0, 'used': 0, 'rejected': {}}
    wait, fails, pend, polled = base, 0, None, False

    def reject(why):
        res['rejected'][why] = res['rejected'].get(why, 0) + 1
        if not once and why not in ('old', 'subject', 'to'):   # 别的账号的登录邮件（同时在登录几个账号）不说
            say('! 邮箱助手：收到一封像是登录邮件的邮件，没通过核对（%s），没有打开：要用的话自己点里面的链接' % MAIL_WHY[why])

    while alive():
        try:
            new = src.poll(since)
            polled = True
            if hello:
                hello()
                hello = None
            for c in new:
                msg = mail_parse(c['hdr'])
                rule, why = mail_judge_any(rules, msg, c['got'], acct, since, forwarder)
                if why:
                    reject(why)
                elif pend is None or (c['got'], c['uid']) > (pend['got'], pend['uid']):
                    pend = dict(c, rule=rule, dg=mail_digest(msg, c), tries=0, at=0.0)
            if pend is not None and now() >= pend['at']:
                st = mail_take(src, pend, acct, since, deliver, again, forwarder)
                if st == 'ok':
                    res['opened'] += 1
                    res['code'], pend = 'opened', None
                    if once:
                        return res
                elif st == 'fail' and pend['tries'] < 2:
                    pend['tries'] += 1
                    pend['at'] = now() + base
                else:
                    pend = None
                    if st == 'used':
                        res['used'] += 1
                    elif st == 'fail':
                        res['code'] = 'fail'
                        if once:
                            return res
                        say('! 邮箱助手：收到登录邮件了，但没能打开浏览器（原因见上面）：链接没有用掉，处理好后运行 cca mail open %s' % acct)
                    elif st in MAIL_WHY:
                        reject(st)
            if idle and pend is None:
                idle()
            idle = None
            fails, wait = 0, base
        except MailFatal as e:
            res.update(code=e.code, text=e.text)
            return res
        except MailRetry as e:
            if once and not polled:
                res.update(code=e.code, text=e.text)
                return res
            fails += 1
            if fails == 3 and not once:
                say('! 邮箱助手：连不上 Gmail（%s），还在重试；登录照常进行，邮件到了也可以自己点里面的链接' % e.text)
            if now() >= end:
                res.update(code=e.code, text=e.text)
                break
            mail_nap(min(wait, 30), end, alive, now, sleep)
            wait = min(wait * 2, 30)
            continue
        if pend is not None:   # 浏览器没打开的那一封：到点再试
            mail_nap(pend['at'] - now(), pend['at'], alive, now, sleep)
        elif now() >= end:
            break
        else:
            mail_nap(poll, end, alive, now, sleep)
    return res


def mail_one(m, auto_default=True, proxy_default='system'):
    """config.json 的 mail 或 mail.accounts[*] 一项 → {user, auto, proxy}；地址不对是 None。
    没写 auto / proxy 的按给的默认值（账号覆盖没写时继承默认那一项的）"""
    if not isinstance(m, dict) or not isinstance(m.get('user'), str) or not MAIL_ADDR_RE.match(m['user'].lower()):
        return None
    p = m.get('proxy')
    return {'user': m['user'].lower(),
            'auto': (m.get('auto') is not False) if 'auto' in m else auto_default,
            'proxy': p if isinstance(p, str) and p else proxy_default}


def mail_cfg(c, key=None):
    """默认的邮箱助手设置（key 是 None），或某账号解析后的设置（覆盖 config.json 的 mail.accounts[acct_id] → 默认 mail）。
    返回 {user, auto, proxy, source: 'account'|'default', forwarder} 或 None（没有可连的 Gmail）。
    forwarder 是这个账号单独登记的「信任转发中转」域名（不安全、默认不登记；不继承默认；只在 key 给定时有意义），没有是 None"""
    m = c.get('mail')
    if not isinstance(m, dict):
        return None
    default = mail_one(m)
    ov = None
    if key is not None:
        accts = m.get('accounts')
        ov = accts.get(acct_id(key)) if isinstance(accts, dict) else None
    one = mail_one(ov, default['auto'] if default else True, default['proxy'] if default else 'system') if isinstance(ov, dict) else None
    if one:
        one['source'] = 'account'
    elif default:
        one = dict(default, source='default')
    if one is None:
        return None
    fwd = ov.get('forwarder') if isinstance(ov, dict) else None
    one['forwarder'] = fwd.lower() if isinstance(fwd, str) and MAIL_DOMAIN_RE.match(fwd.lower()) else None
    return one


def mail_all_addrs(c):
    """config.json 里引用到的所有 Gmail 地址（默认 + 各账号覆盖），小写：换地址、移除时据此清钥匙串"""
    m = c.get('mail')
    out = set()
    if isinstance(m, dict):
        one = mail_one(m)
        if one:
            out.add(one['user'])
        accts = m.get('accounts')
        if isinstance(accts, dict):
            for ov in accts.values():
                o = mail_one(ov)
                if o:
                    out.add(o['user'])
    return out


def mail_view(c, key):
    """账号页的邮箱助手状态（_gui state；只读 config.json，不碰钥匙串）：{configured, user, source, auto, forwarder}。
    source：account（这个账号单独配的 Gmail）| default（用默认那个）| None（没配置）。
    forwarder：这个账号登记的「信任转发中转」域名（不安全开关，默认 null）"""
    m = mail_cfg(c, key)
    return {'configured': bool(m), 'user': m['user'] if m else None,
            'source': m.get('source') if m else None, 'auto': m['auto'] if m else True,
            'forwarder': m.get('forwarder') if m else None}


def load_quiet(path):
    """读不了、格式不对都当作没有（缓存之类，不值得为它报错退出）"""
    try:
        with open(path, encoding='utf-8') as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def save_json(path, data, mode=None, follow_symlinks=True):
    path = os.path.realpath(path) if follow_symlinks else os.path.abspath(path)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    if mode is None:
        try:
            mode = os.stat(path).st_mode & 0o777
        except FileNotFoundError:
            mode = 0o600
    tmp = '%s.cca-%d.tmp' % (path, os.getpid())
    with open(tmp, 'w', encoding='utf-8') as f:
        json.dump(data, f, ensure_ascii=False, indent=2)
        f.write('\n')
    os.chmod(tmp, mode)
    os.replace(tmp, path)


def parse_key(key):
    """账号表的键 → (别名, 邮箱)。键可以是「别名(邮箱)」、只写邮箱，或只写别名（登录页不预填邮箱）"""
    m = PAIR_RE.match(key)
    if m and EMAIL_RE.match(m.group(2)):
        return m.group(1), m.group(2)
    return ('', key) if EMAIL_RE.match(key) else (key, '')


def ident(k):
    s = re.sub(r'[^a-z0-9]+', '-', k.lower()).strip('-')[:20].strip('-') or 'acct'
    return '%s-%s' % (s, hashlib.sha1(k.encode('utf-8')).hexdigest()[:6])


def acct_id(key, tool=None):
    # 有邮箱就按邮箱认账号：改别名不换账号目录，登录和浏览器资料都还在；第三方 API 账号按 id 认。
    # 各工具的账号目录在不同的地方（sroot），同一个邮箱的 Claude 账号和 Codex 账号不会撞
    return ident(parse_key(key)[1] or (tool or T).ids.get(key) or key)


MAIL_RULES = {
    # claude.ai 用邮箱登录时发的登录邮件。真邮件（2026-11-16 核对）标题是 Your secure link to Claude is here，发件人
    # no-reply-<随机>@mail.anthropic.com，Gmail 验 dkim=pass mail.anthropic.com。帮助中心写的是 Secure link to log in to
    # Claude.ai、也有人收到过 Your secure link to Claude.ai is here，一并认。标题里 claude 后面那个词是应用名（attacker 能把 OAuth
    # 应用取名叫别的，比如见过 Your secure link to Verification Portal is here）：所以只认应用名是 Claude / Claude.ai 的，别的不开
    'claude-login': MailRule('claude-login', 'claude.ai 的登录链接', ('anthropic.com',),
                             (r'\bsecure link to (log in to )?claude(\.ai)?(?![\w.-])',),
                             ('OR', 'FROM', MAIL_Q + 'anthropic.com' + MAIL_Q, 'SUBJECT', MAIL_Q + 'secure link' + MAIL_Q),
                             ('claude.ai',), ('/magic-link',)),
}
REFERENCE = (MailRule, MailError, MailRetry, MailFatal, mail_clean, mail_oserr, mail_proxy_parse, mail_bypass, mail_route_note, mail_epoch,
             mail_since, mail_crit, mail_fetch_parse, mail_parse, mail_text, mail_addrs, mail_tags, mail_ar_items, mail_ar, mail_fwd_ok,
             mail_from_alias, mail_signed_for, mail_signed, mail_judge, mail_judge_fwd, mail_judge_any, mail_links, mail_extract, mail_shape,
             mail_digest, mail_handle, mail_imap, MailBox, mail_knob, mail_used, mail_claim, mail_mark, mail_pid_alive, mail_nap, mail_verdict, mail_take,
             mail_hunt, mail_one, mail_cfg, mail_all_addrs, mail_view, load_quiet, save_json, parse_key, ident, acct_id)
RULE = MAIL_RULES['claude-login']


def check_reference():
    """抄来的函数和 cca 里 PY_SRC 的原文一字不差（cca 里已经没有 PY_SRC 时不查）"""
    try:
        with open(os.path.join(REPO, 'cca'), encoding='utf-8') as f:
            src = f.read()
    except OSError:
        return
    if "PY_SRC=$(cat <<'PY'\n" not in src:
        return
    py = src.split("PY_SRC=$(cat <<'PY'\n", 1)[1].split('\nPY\n)', 1)[0]
    bad = []
    for obj in REFERENCE:
        m = re.search(r'^(?:def|class) %s\b.*?(?=^\S|\Z)' % re.escape(obj.__name__), py, re.M | re.S)
        if not m or m.group(0).rstrip() != inspect.getsource(obj).rstrip():
            bad.append(obj.__name__)
    with open(os.path.abspath(__file__), encoding='utf-8') as f:
        me = f.read()
    consts = [l for l in me.split('# --- 参照实现', 1)[1].split('\n\n\n', 1)[0].splitlines() if l.startswith(('MAIL_', 'EMAIL_RE', 'PAIR_RE'))]
    bad += [l.split(' ', 1)[0] for l in consts if l not in py.splitlines()]
    for table in ('MAIL_RULES', 'MAIL_WHY'):
        if '\n%s = {' % table + me.split('\n%s = {' % table, 1)[1].split('\n}\n', 1)[0] + '\n}\n' not in py:
            bad.append(table)
    if bad:
        sys.exit('参照实现和 cca 里的原文对不上了（主干改过？照原文重抄）：%s' % '、'.join(bad))


# --- 小工具 --------------------------------------------------------------------------------------------------------
def b2s(b):
    """字节 → JSON 里的写法（Latin-1 的文字）"""
    return b.decode('latin-1')


def cls(e):
    return type(e).__name__


MINE = {'utf-8': 'utf-8', 'ascii': 'ascii', 'iso8859-1': 'iso8859-1', 'cp1252': 'cp1252'}   # Rust 那边认的四个字符集（codecs 里的名字）


def codec_of(name):
    """这个字符集的名字在 bytes.decode 里落到哪：四个之一 / other（Python 认识的别的）/ LookupError / ValueError（名字里有 NUL）"""
    try:
        b'a'.decode(name, 'replace')
    except LookupError:
        return 'LookupError'
    except ValueError as e:
        return 'ValueError' if 'embedded null' in str(e) else 'other'
    return MINE.get(codecs.lookup(name).name, 'other')


def pick(rng, pool, lo, hi):
    return [rng.choice(pool) for _ in range(rng.randint(lo, hi))]


def rep_encode(s):
    """长串里连着重复的段写成 [段, 次数]（4301 个 9、八千个 a 这种）；没有可压的返回 None"""
    out, lit, i, n, hit = [], [], 0, len(s), False
    while i < n:
        best = (0, 1, 1)   # 这一段一共多长, 段长, 次数
        for size in range(1, 9):
            unit = s[i:i + size]
            if len(unit) < size:
                break
            k = 1
            while s.startswith(unit, i + k * size):
                k += 1
            if k >= 2 and size * k >= 48 and size * k > best[0]:
                best = (size * k, size, k)
        if best[0]:
            if lit:
                out.append([''.join(lit), 1])
                lit = []
            out.append([s[i:i + best[1]], best[2]])
            i += best[0]
            hit = True
        else:
            lit.append(s[i])
            i += 1
    if lit:
        out.append([''.join(lit), 1])
    return out if hit else None


def compact(x):
    """写进 JSON 之前：长的、有重复段的字符串换成 {"rep": [[段, 次数], …]}（Rust 的测试读的时候展开）"""
    if isinstance(x, str) and len(x) >= 64:
        rep = rep_encode(x)
        return {'rep': rep} if rep else x
    if isinstance(x, list):
        return [compact(v) for v in x]
    if isinstance(x, dict):
        return {k: compact(v) for k, v in x.items()}
    return x


# --- 字符集、传输编码 ----------------------------------------------------------------------------------------------
CODEC_NAMES = [
    'utf-8', 'UTF-8', 'utf8', 'Utf_8', 'utf 8', ' utf-8', 'utf-8 ', 'utf--8', 'utf-8;', '"utf-8"', 'u8', 'UTF', 'utf-8-sig', 'utf_8_sig', 'cp65001',
    'ascii', 'us-ascii', 'US_ASCII', 'us ascii', 'latin-1', 'latin1', 'LATIN_1', 'iso-8859-1', 'iso8859-1', 'ISO_8859-1', 'iso-8859-1:1987', 'l1',
    '8859', 'cp819', 'windows-1252', 'cp1252', 'Windows_1252', '1252', 'x-cp1252', '', ' ', 'a\x00b', '\x00', 'utf-8\x00', 'gbk', 'gb2312', 'gb18030',
    'big5', 'iso-2022-jp', 'shift_jis', 'euc-jp', 'euc-kr', 'koi8-r', 'iso-8859-2', 'iso-8859-15', 'windows-1251', 'utf-16', 'utf-16le', 'utf-32', 'utf-7',
    'rot13', 'hex', 'base64', 'idna', 'punycode', 'undefined', 'unicode_escape', 'raw_unicode_escape', 'aliases', 'utf.8', 'utf_8.', '.utf_8',
    'utf\u20138', 'utf-\uff18', '\uff35\uff34\uff26-8', '\u00e9', 'utf-8\u00e9', 'mbcs', 'oem', 'charmap', 'unknown-8bit', 'x' * 300, 'latin', 'iso_8859_1',
    'iso-ir-100', 'ansi_x3.4-1968', 'ansi_x3_4_1968', 'ansi_x3.4_1986', 'iso_646.irv_1991', 'iso_646.irv:1991', 'iso-646.irv-1991', '646', 'us',
    'utf8_ucs2', 'utf8-ucs4', 'cp_1252', 'utf-8/', 'encodings.utf_8', '__init__', 'utf_8\n', '\tutf-8', 'UTF-8\r\n', 'csascii', 'csISOLatin1', 'ibm367',
    'IBM819', 'iso646-us', 'iso-ir-6', 'iso_8859-1:1987', 'iso8859', 'cp367', 'utf', 'utf_8_', '_utf_8', 'u t f 8', 'utf8\u00e9', '\u00e9utf8',
    'utf-\uff188', 'utf\u00b28', 'iso-8859-1\u0661', '\u0130so-8859-1', 'UTF-8K', 'utf-8\u212a', 'none', 'None', 'ascii.', 'a.scii', 'latin.1', 'us.ascii',
    'ansi.x3.4.1968', 'ansi_x3.4.1968', 'iso.646_irv.1991', 'utf_8.py', 'cp1252 ', 'CP-1252', 'cp 1252', 'windows1252', 'win1252', 'ms-ansi', 'x-user-defined',
    'binary', '8bit', 'us-ascii ', '"us-ascii"', "'utf-8'", 'utf-8,utf-8', 'utf-8 utf-8', 'UTF8;format=flowed',
]
BYTES = [
    b'', b'abc', bytes(range(256)), b'caf\xc3\xa9', b'caf\xe9', b'\xe4\xb8\xad\xe6\x96\x87', b'\xf0\x9f\x98\x80', b'\xf0\x9f\x98', b'\xf0\x9f', b'\xf0',
    b'\xe4\xb8', b'\xe4', b'\xc3', b'\xc3(', b'a\x80b', b'\x80\x80', b'\xc0\xaf', b'\xc0\x80', b'\xc1\xbf', b'\xe0\x80\x80', b'\xe0\x9f\xbf', b'\xe0\xa0\x80',
    b'\xed\x9f\xbf', b'\xed\xa0\x80', b'\xed\xbf\xbf', b'\xee\x80\x80', b'\xef\xbf\xbd', b'\xef\xbf\xbe', b'\xef\xbb\xbfbom', b'\xf0\x80\x80\x80',
    b'\xf0\x8f\xbf\xbf', b'\xf0\x90\x80\x80', b'\xf4\x8f\xbf\xbf', b'\xf4\x90\x80\x80', b'\xf5\x80\x80\x80', b'\xf8\x88\x80\x80\x80', b'\xfc\x84\x80\x80\x80\x80',
    b'\xfe', b'\xff', b'\xfe\xff', b'\xff\xfe', b'\xe4\xb8\x41', b'\xf0\x9f\x41\x80', b'\xf0\x41', b'\xe4\xb8\xe4\xb8\xad', b'\xc3\xc3\xa9', b'\xf0\x9f\x98\xf0\x9f\x98\x80',
    b'\x00\x01\x7f', b'"quoted"', b"it's", b'both \' and "', b'back\\slash', b'tab\there\nnew\rline', b'\x1b[2K', b'\x81\x8d\x8f\x90\x9d', b'\x80\x82\x9f\xa0',
    b'https://claude.ai/magic-link#x\xa1\xa1', b'\xd6\xd0\xce\xc4', b'=?utf-8?q?x?=', b'a' * 300,
]


def codec_lookup_cases(rng, extra):
    names = list(CODEC_NAMES)
    names += sorted(encodings.aliases.aliases) + sorted(m.name for m in pkgutil.iter_modules(encodings.__path__))
    pool = ['utf', 'UTF', '8', '-', '_', ' ', '.', 'latin', '1', 'iso', '8859', 'ascii', 'us', 'cp', '1252', 'windows', ';', '"', '\u00e9', '\uff18', 'x',
            'ansi', 'x3', '4', '1968', '646', 'irv', '1991', 'ucs2', 'sig', '\x00', 'L', 'u']
    for _ in range(300 * extra):
        names.append(''.join(pick(rng, pool, 1, 5)))
    return [{'name': n, 'py': codec_of(n)} for n in dict.fromkeys(names)]


def decode_case(data):
    out = {'data': b2s(data), 'repr': str(data)}
    for name in ('utf-8', 'ascii', 'latin-1', 'cp1252'):
        d = {'replace': data.decode(name, 'replace')}
        try:
            d['strict'] = data.decode(name)
        except UnicodeDecodeError:
            pass
        out[name] = d
    return out


def codec_decode_cases(rng, extra):
    datas = list(BYTES)
    lead = [b'\xc2', b'\xc3', b'\xdf', b'\xe0', b'\xe1', b'\xec', b'\xed', b'\xee', b'\xef', b'\xf0', b'\xf1', b'\xf3', b'\xf4', b'\xf5', b'\xc0', b'\xc1',
            b'\x80', b'\x8f', b'\x90', b'\x9f', b'\xa0', b'\xbf', b'a', b'"', b"'", b'\\', b'\n', b'\xff', b'\xfe']
    for _ in range(80 * extra):
        datas.append(b''.join(pick(rng, lead, 1, 7)))
    for _ in range(20 * extra):
        datas.append(bytes(rng.randrange(256) for _ in range(rng.randint(1, 12))))
    return [decode_case(d) for d in dict.fromkeys(datas)]


def raw_escape_cases():
    ss = ['', 'abc', 'caf\u00e9', '\u00ff\u0100', '\ufffd', '\u4e2d\u6587', '\U0001f600', 'a\\u1234', '\x00\x7f\x80', '\uffff\U00010000\U0010ffff']
    return [{'s': s, 'out': b2s(s.encode('raw-unicode-escape'))} for s in ss]


TRANSFER = [
    b'', b'plain text\r\n', b'caf=C3=A9\r\n', b'soft=\r\nbreak', b'soft=\nbreak', b'soft=\rbreak', b'soft= \t\r\nbreak', b'soft=   x\nbreak', b'end=',
    b'=', b'==', b'===', b'=4', b'=4g', b'=g4', b'=41=4a=4A', b'a=3D"b"', b'under_score', b'=0D=0A', b'=\r', b'=\n', b'=\r\n', b'= \r\n', b'x=\r\r\ny',
    b'<a href=3D"https://claude.ai/magic-link#tok">x</a>=\r\n<br>', b'=E4=B8=AD=E6=96=87', b'=e4=b8=ad', b'a=\x00b', b'a=\xe4b', b'=3d=3D',
    b'aGVsbG8=', b'aGVsbG8', b'aGVsbG8==', b'aGVs\r\nbG8=\r\n', b'aGVs bG8=', b'aGVs*bG8=', b'a', b'aG', b'aGV', b'aGVsb', b'====', b'=aGVsbG8=', b'aGVsbG8=aGVsbG8=',
    b'aGVsbG8=\r\naGVsbG8=\r\n', b'PGEgaHJlZj0iaHR0cHM6Ly9jbGF1ZGUuYWkvbWFnaWMtbGluayN0b2siPng8L2E+\r\n', b'aGVs\nbG8=\n', b'aGVs\rbG8=', b'aGVsbG8=\x00', b'\xff\xfeaGVsbG8=',
    b'YQ==YQ==', b'YQ=', b'YQ', b'Y', b'YQ===', b'Y=Q=', b'!!!!', b'!!!!!', b'a\r\n', b'ab\r\n', b'abcde',
    b'begin 644 x\r\n#:&D*\r\n`\r\nend\r\n', b'begin 644 x\n#:&D*\n`\nend\n', b'begin 644 x\n#:&D*\n \nend\n', b'begin 644 x\n#:&D*\n', b'begin 644 x\n\nend\n',
    b'begin 0o644 x\n#:&D*\nend\n', b'begin  644 x\n#:&D*\nend\n', b'begin 9 x\nbegin 7 y\n#:&D*\nend\n', b'begin x\n#:&D*\nend\n', b'begin 6_4 x\n#:&D*\nend\n',
    b'begin +7\n#:&D*\nend\n', b'begin\n#:&D*\nend\n', b'no begin\n#:&D*\nend\n', b'begin 644 x\n#:&D*junk\nend\n', b'begin 644 x\n#:&D\x01\nend\n',
    b'begin 644 x\nM' + b'A' * 60 + b'\n \t end \r\n', b'begin 644 x\n#:&D*\n  end\ntrailing', b'begin 644 x\n!:&D*\nend\n', b'begin 644 x\n$:&D*\nend\n',
    b'begin 644 x\n`\nend\n', b'begin 644 x\n#\nend\n', b'begin 644 x\n#:&\nend\n', b'begin 644 x\n#:&D*\x80\nend\n', b'begin 644 x\n\x7f:&D*\nend\n',
    b'begin 644 x\n#:&D*   \nend\n', b'begin 644 x\n#:&D*``\nend\n', b'begin 644 x\n#:&D*a\nend\n', b'begin 0O7 x\n#:&D*\nend\n', b'begin 0x7 x\n#:&D*\nend\n',
    b'begin -7 x\n#:&D*\nend\n', b'begin 7_ x\n#:&D*\nend\n', b'begin _7 x\n#:&D*\nend\n', b'begin 0o_7 x\n#:&D*\nend\n', b'begin 0o x\n#:&D*\nend\n',
    b'begin \t7 x\n#:&D*\nend\n', b'begin 7\x0b x\n#:&D*\nend\n', b'BEGIN 644 x\n#:&D*\nend\n', b'begin 644 x\r#:&D*\rend\r',
]


def transfer_case(data):
    from email import message_from_bytes
    out = {'data': b2s(data)}
    for cte in ('quoted-printable', 'base64', 'x-uuencode', '8bit'):
        m = message_from_bytes(b'Content-Transfer-Encoding: ' + cte.encode() + b'\r\n\r\n' + data)
        out[cte] = b2s(m.get_payload(decode=True))
    return out


def transfer_cases(rng, extra):
    datas = list(TRANSFER)
    qp = [b'=', b'=', b'4', b'1', b'a', b'F', b'g', b'\r', b'\n', b'\r\n', b' ', b'\t', b'_', b'x', b'=\r\n', b'=3D', b'\xe4']
    b64 = [b'a', b'G', b'V', b's', b'b', b'8', b'=', b'=', b'+', b'/', b'\r\n', b'\n', b' ', b'*', b'-', b'_', b'\x00', b'\xff', b'YQ==', b'aGVsbG8=']
    uu = [b'begin 644 x\n', b'begin 7 y\r\n', b'#:&D*\n', b'#:&D*', b'`\n', b' \n', b'end\n', b'end', b'\n', b'\r\n', b'M', b'A', b'!', b'"', b'$', b'`', b' ', b'a',
          b'\x7f', b'junk', b'begin x\n', b'0o', b'_']
    for pool in (qp, b64, uu):
        for _ in range(60 * extra):
            datas.append(b''.join(pick(rng, pool, 1, 9)))
    return [transfer_case(d) for d in dict.fromkeys(datas)]


# --- 邮件的结构 ----------------------------------------------------------------------------------------------------
def crlf(s):
    return s.replace('\n', '\r\n').encode('latin-1')


MESSAGES = [
    b'', b'\r\n', b'body only\r\n', b'\r\nbody after blank\r\n', b'Subject: hi\r\n\r\nbody\r\n', b'Subject: hi\r\n', b'Subject: hi', b'Subject:hi\n\nbody\n',
    b'Subject: hi\r\rbody\r', b'Subject: folded\r\n line two\r\n\tline three\r\n\r\nbody', b'Subject:\r\n  only continuation\r\n\r\n', b' leading continuation\r\nSubject: x\r\n\r\n',
    b'A: 1\r\nA: 2\r\na: 3\r\nB: x\r\n\r\n', b'From alice Sat Nov 15\r\nSubject: unix from\r\n\r\nbody', b'Subject: x\r\nFrom bob Sat\r\nTo: y\r\n\r\nbody',
    b'Subject: x\r\nFrom last line\r\n', b'Subject: x\r\nFrom last line\r\n\r\nbody', b'Subject: x\r\nFrom mid\r\n continued\r\nTo: y\r\n\r\n', b': no name\r\nSubject: x\r\n\r\n',
    b'Subject: x\r\n: no name\r\n cont\r\nTo: y\r\n\r\n', b'Bad Header: x\r\nSubject: y\r\n\r\nbody', b'Subject: x\r\nnot a header line\r\nTo: y\r\n\r\nbody',
    b'Subject: x\r\n\xe4\xb8\xad: y\r\n\r\nbody', b'Subject: caf\xc3\xa9\r\nX-Bin: \xff\xfe\r\n\r\n\xff body', b'Subject : space before colon\r\n\r\n', b'Subject:\r\n\r\nbody',
    b'Subject:   \t padded \t \r\n\r\n', b'Subject: trailing cr\r\r\n\r\nbody', b'Subject: a\r\n\r\n\r\nbody after two blanks\r\n\r\n', b'S\x00: nul in name\r\n\r\n',
    b'Subject: nul \x00 in value\r\n\r\n', b'Subject: x\n continued lf\n\nbody', b'Subject: x\r continued cr\r\rbody', b'X: 1\r\n\r\nFrom not a header\r\n',
    b'X-Long: ' + b'a' * 9000 + b'\r\n\r\n' + b'b' * 9000, (b'A: ' + b'x' * 100 + b'\r\n') * 90 + b'\r\nbody',
    crlf('Content-Type: text/plain\n\nplain'), crlf('Content-Type: TEXT/HTML; charset="UTF-8"\n\n<a href="https://x">'), crlf('Content-Type: text\n\nno slash'),
    crlf('Content-Type: a/b/c\n\ntwo slashes'), crlf('Content-Type:\n\nempty'), crlf('Content-Type: ;charset=utf-8\n\nx'), crlf('Content-Type: text/html ; charset = utf-8 \n\nx'),
    crlf('Content-Type: text/html;charset=utf-8;charset=latin-1\n\nx'), crlf('Content-Type: text/html; CHARSET=UTF-8\n\nx'), crlf("Content-Type: text/html; charset='utf-8'\n\nx"),
    crlf('Content-Type: text/html; charset="utf-8\n\nx'), crlf('Content-Type: text/html; charset=utf-8"\n\nx'), crlf('Content-Type: text/html; charset=<utf-8>\n\nx'),
    crlf('Content-Type: text/html; charset="\\"utf-8\\""\n\nx'), crlf('Content-Type: text/html; name="a;b"; charset=latin-1\n\nx'),
    crlf('Content-Type: text/html; name="a\\";b"; charset=latin-1\n\nx'), crlf('Content-Type: text/html; name="a;b; charset=latin-1\n\nx'), crlf('Content-Type: text/html; charset\n\nx'),
    crlf('Content-Type: text/html; =utf-8\n\nx'), crlf('Content-Type: text/html; charset=\n\nx'), crlf('Content-Type: text/html; charset=""\n\nx'),
    crlf('Content-Type: text/html; charset=caf\u00e9\n\nx'), crlf('Content-Type: charset=utf-8\n\nx'), crlf('Content-Type: text/html; charset=utf-8; charset*=latin-1\n\nx'),
    crlf("Content-Type: text/html; charset*=utf-8''utf-8\n\nx"), crlf("Content-Type: text/html; charset*=us-ascii'en'utf-8\n\nx"), crlf("Content-Type: text/html; charset*=''latin-1\n\nx"),
    crlf("Content-Type: text/html; charset*=utf-8\n\nx"), crlf("Content-Type: text/html; charset*='utf-8\n\nx"), crlf("Content-Type: text/html; charset*=utf-8''%75%74%66-8\n\nx"),
    crlf("Content-Type: text/html; charset*=utf-8''%c3%a9\n\nx"), crlf("Content-Type: text/html; charset*=latin-1''%e9\n\nx"), crlf("Content-Type: text/html; charset*=us-ascii''%e9\n\nx"),
    crlf("Content-Type: text/html; charset*=nosuchcodec''utf-8\n\nx"), crlf("Content-Type: text/html; charset*=nosuchcodec''%e9\n\nx"), crlf("Content-Type: text/html; charset*=utf-8''%ff\n\nx"),
    crlf("Content-Type: text/html; charset*0=ut; charset*1=f-8\n\nx"), crlf("Content-Type: text/html; charset*1=f-8; charset*0=ut\n\nx"), crlf("Content-Type: text/html; charset*0*=utf-8''ut; charset*1=f%2D8\n\nx"),
    crlf("Content-Type: text/html; charset*0*=utf-8''ut; charset*1*=f%2D8\n\nx"), crlf("Content-Type: text/html; charset*=x; charset*0=y; charset*1=z\n\nx"),
    crlf("Content-Type: text/html; charset*0=a; charset*=b\n\nx"), crlf("Content-Type: text/html; charset*00=ut; charset*01=f-8\n\nx"), crlf("Content-Type: text/html; charset*2=c; charset*10=d; charset*1=b\n\nx"),
    crlf("Content-Type: text/html; charset**=x\n\nx"), crlf("Content-Type: text/html; charset*x=x\n\nx"), crlf("Content-Type: text/html; char set*=x\n\nx"), crlf("Content-Type: text/html; ch-arset*=x; charset=utf-8\n\nx"),
    crlf("Content-Type: text/html; charset*=\"utf-8''utf-8\"\n\nx"), crlf("Content-Type: text/html; charset*=utf-8'\\'utf-8\n\nx"), crlf("Content-Type: text/html; charset*=a'b'c'd\n\nx"),
    crlf("Content-Type: text/html; charset*=%75tf-8''latin-1\n\nx"), crlf("Content-Type: text/html; x*=a%00b''c; charset=utf-8\n\nx"), crlf("Content-Type: text/html; charset*=a%00b''utf-8\n\nx"),
    crlf('Content-Type: text/html; charset="a\x00b"\n\nx'), crlf("Content-Type: text/html; x*" + "9" * 4301 + "*=y; charset=utf-8\n\nx"), crlf("Content-Type: text/html; x*" + "9" * 4300 + "*=y; charset=utf-8\n\nx"),
    crlf("Content-Type: text/html; x*" + "0" * 4301 + "=y\n\nx"), crlf('Content-Type: text/html; charset=utf-8\nContent-Type: text/plain; charset=latin-1\n\nx'),
    crlf('Content-Disposition: attachment; filename="x.html"\nContent-Type: text/html\n\nx'), crlf('Content-Disposition: ATTACHMENT\n\nx'), crlf('Content-Disposition:  inline ; x\n\nx'),
    crlf('Content-Disposition:\n\nx'), crlf('Content-Disposition: ;attachment\n\nx'), crlf('Content-Disposition: attachment\nContent-Disposition: inline\n\nx'),
    b'Content-Disposition: attach\xffment\r\n\r\nx', b'Content-Type: text/h\xfftml\r\n\r\nx', b'Content-Type: text/html; charset=utf\xff-8\r\n\r\nx',
    crlf('Content-Transfer-Encoding: BASE64\n\naGVsbG8=\n'), crlf('Content-Transfer-Encoding:  base64 \n\naGVsbG8=\n'), crlf('Content-Transfer-Encoding: base64; x\n\naGVsbG8=\n'),
    crlf('Content-Transfer-Encoding: Quoted-Printable\n\ncaf=C3=A9=\n!\n'), crlf('Content-Transfer-Encoding: 7bit\n\ncaf=C3=A9\n'), crlf('Content-Transfer-Encoding: x-uue\n\nbegin 644 x\n#:&D*\nend\n'),
    crlf('Content-Transfer-Encoding: uuencode\n\nbegin 644 x\n#:&D*\nend\n'), crlf('Content-Transfer-Encoding: uue\n\nnot uu\n'), crlf('Content-Transfer-Encoding: binary\n\n=41\n'),
    b'Content-Transfer-Encoding: base\xff64\r\n\r\naGVsbG8=\r\n', crlf('Content-Transfer-Encoding: base64\nContent-Transfer-Encoding: 7bit\n\naGVsbG8=\n'),
    # multipart
    crlf('Content-Type: multipart/alternative; boundary="b"\n\npreamble\n--b\nContent-Type: text/plain\n\nplain part\n--b\nContent-Type: text/html\n\n<b>html</b>\n--b--\nepilogue\n'),
    crlf('Content-Type: multipart/mixed; boundary=b\n\n--b\n\nfirst\n--b\n\nsecond\n--b--\n'), crlf('Content-Type: multipart/mixed; boundary=b\n\n--b\n\nfirst\n\n--b\n\n\nsecond\n\n\n--b--\n'),
    crlf('Content-Type: multipart/mixed; boundary=b\n\n--b\nX: 1\n\nno close\n'), crlf('Content-Type: multipart/mixed; boundary=b\n\n--b\nX: 1\n\nno trailing newline'),
    crlf('Content-Type: multipart/mixed; boundary=b\n\nno boundary at all\nline two\n'), crlf('Content-Type: multipart/mixed; boundary=b\n\npre\n--b--\npost\n'),
    crlf('Content-Type: multipart/mixed; boundary=b\n\n--b--\n'), crlf('Content-Type: multipart/mixed; boundary=b\n\n--b\n'), crlf('Content-Type: multipart/mixed; boundary=b\n\n--b'),
    crlf('Content-Type: multipart/mixed; boundary=b\n\n--b\n--b\n--b\nX: 1\n\nafter three\n--b--\n'), crlf('Content-Type: multipart/mixed; boundary=b\n\n--b\n--b--\nX: 1\n\nswallowed close\n--b--\n'),
    crlf('Content-Type: multipart/mixed; boundary=b\n\n--b\n--b--\n'), crlf('Content-Type: multipart/mixed; boundary=b\n\n--b  \t\nX: 1\n\ntrailing ws on boundary\n--b-- \t \n'),
    crlf('Content-Type: multipart/mixed; boundary=b\n\n--bx\nX: 1\n\nnot a boundary\n--b\nX: 2\n\nreal\n--b--x\nstill inside\n--b--\n'),
    crlf('Content-Type: multipart/mixed; boundary=b\n\n--b - -\nX: 1\n\nx\n--b--\n'), crlf('Content-Type: multipart/mixed; boundary=b\n\n --b\nX: 1\n\nindented\n--b--\n'),
    crlf('Content-Type: multipart/mixed; boundary="b b"\n\n--b b\nX: 1\n\nspace in boundary\n--b b--\n'), crlf('Content-Type: multipart/mixed; boundary="b "\n\n--b\nX: 1\n\ntrailing space stripped\n--b--\n'),
    crlf('Content-Type: multipart/mixed; boundary=" b"\n\n-- b\nX: 1\n\nleading space kept\n-- b--\n'), crlf('Content-Type: multipart/mixed; boundary=<b>\n\n--b\nX: 1\n\nangle\n--b--\n'),
    crlf('Content-Type: multipart/mixed; boundary=""\n\n--\nX: 1\n\nempty boundary\n----\n'), crlf('Content-Type: multipart/mixed; boundary=\n\n--\nX: 1\n\nempty boundary\n----\n'),
    crlf('Content-Type: multipart/mixed\n\n--b\nX: 1\n\nno boundary param\n--b--\n'), crlf('Content-Type: multipart/mixed; boundary\n\n--\nx\n'), crlf('Content-Type: Multipart/Mixed; BOUNDARY=b\n\n--b\n\nupper\n--b--\n'),
    crlf('Content-Type: multipart/mixed; boundary=b; boundary=c\n\n--c\n\nsecond wins?\n--c--\n--b\n\nfirst wins?\n--b--\n'),
    crlf('Content-Type: multipart/mixed; boundary="a\\"b"\n\n--a"b\n\nescaped quote\n--a"b--\n'), crlf('Content-Type: multipart/mixed; boundary="a;b"\n\n--a;b\n\nsemicolon\n--a;b--\n'),
    crlf("Content-Type: multipart/mixed; boundary*=utf-8''b%2Db\n\n--b-b\n\nrfc2231\n--b-b--\n"), crlf("Content-Type: multipart/mixed; boundary*0=b; boundary*1=c\n\n--bc\n\ncontinued\n--bc--\n"),
    crlf("Content-Type: multipart/mixed; boundary*=us-ascii'en'b\n\n--b\n\nlang\n--b--\n"), crlf("Content-Type: multipart/mixed; boundary*=''b\n\n--b\n\nno charset\n--b--\n"),
    crlf("Content-Type: multipart/mixed; boundary*=b\n\n--b\n\nno ticks\n--b--\n"), crlf("Content-Type: multipart/mixed; boundary*=nosuchcodec''b\n\n--b\n\nunknown codec\n--b--\n"),
    crlf("Content-Type: multipart/mixed; boundary*=nosuchcodec''%22b%22\n\n--b\n\nunknown codec, quoted twice\n--b--\n"), crlf("Content-Type: multipart/mixed; boundary*=utf-8''%c3%a9\n\n--\u00e9\n\nnon-ascii\n"),
    crlf("Content-Type: multipart/mixed; boundary*=latin-1''b%e9\n\n--b\u00e9\n\nlatin\n--b\u00e9--\n"), crlf("Content-Type: multipart/mixed; boundary*=us-ascii''b%e9\n\n--b\n\nreplaced\n--b--\n"),
    crlf("Content-Type: multipart/mixed; boundary*=a%00b''c\n\n--c\n\nnul in charset\n"), crlf("Content-Type: multipart/mixed; boundary*" + "9" * 4301 + "*=x\n\n--x\n"),
    b'Content-Type: multipart/mixed; boundary=b\xe9\r\n\r\n--b\xe9\r\n\r\n8bit boundary never matches\r\n--b\xe9--\r\n',
    crlf('Content-Type: multipart/mixed; boundary=b\nContent-Transfer-Encoding: base64\n\n--b\n\naGVsbG8=\n--b--\n'),
    crlf('Content-Type: multipart/mixed; boundary=o\n\n--o\nContent-Type: multipart/alternative; boundary=i\n\n--i\nContent-Type: text/plain\n\ninner plain\n--i\n'
         'Content-Type: text/html\n\ninner html\n--i--\ninner epilogue\n--o\nContent-Type: text/plain\n\nouter second\n--o--\n'),
    crlf('Content-Type: multipart/mixed; boundary=o\n\n--o\nContent-Type: multipart/alternative; boundary=i\n\n--i\n\ninner never closed\n--o\n\nouter second\n--o--\n'),
    crlf('Content-Type: multipart/mixed; boundary=o\n\n--o\nContent-Type: multipart/alternative; boundary=i\n\n--i\n--o\n\nouter boundary right after inner\n--o--\n'),
    crlf('Content-Type: multipart/mixed; boundary=o\n\n--o\nContent-Type: multipart/alternative; boundary=i\n\nno inner boundary\n--o\n\nsecond\n--o--\n'),
    crlf('Content-Type: multipart/mixed; boundary=o\n\n--o\nContent-Type: multipart/alternative\n\ninner without param\n--o\n\nsecond\n--o--\n'),
    crlf('Content-Type: multipart/mixed; boundary=b\n\n--b\nContent-Type: multipart/mixed; boundary=b\n\n--b\n\nsame boundary nested\n--b--\n--b--\n'),
    crlf('Content-Type: multipart/mixed; boundary=b\n\n--b\nContent-Type: message/rfc822\n\nSubject: inner\nContent-Type: text/html\n\n<a href="https://x">inner body</a>\n--b--\n'),
    crlf('Content-Type: multipart/mixed; boundary=b\n\n--b\nContent-Type: message/rfc822\n\nContent-Type: multipart/mixed; boundary=c\n\n--c\n\ndeep\n--c--\n\n--b--\n'),
    crlf('Content-Type: message/rfc822\n\nSubject: wrapped\n\nwrapped body\n'), crlf('Content-Type: message/rfc822\n\n'), crlf('Content-Type: message/rfc822'),
    crlf('Content-Type: message/rfc822\n\nContent-Type: message/rfc822\n\nSubject: twice\n\nbody\n'), crlf('Content-Type: message/partial\n\nSubject: partial\n\nbody\n'),
    crlf('Content-Type: multipart/digest; boundary=b\n\n--b\n\nSubject: digest one\n\nbody one\n--b\nContent-Type: text/plain\n\nexplicit\n--b--\n'),
    crlf('Content-Type: multipart/digest; boundary=b\n\n--b\nSubject: has a header\n\nSubject: inner?\n\nbody\n--b--\n'),
    crlf('Content-Type: message/delivery-status\n\nReporting-MTA: dns; x\n\nFinal-Recipient: rfc822; a@example.com\nAction: failed\n\nFinal-Recipient: rfc822; b@example.com\n'),
    crlf('Content-Type: message/delivery-status\n\n'), crlf('Content-Type: message/delivery-status\n\n\n\nA: 1\n\n\nB: 2\n'), crlf('Content-Type: message/delivery-status\n\nA: 1\nnot a header\n\nB: 2\n'),
    crlf('Content-Type: multipart/report; boundary=b\n\n--b\nContent-Type: message/delivery-status\n\nA: 1\n\nB: 2\n\n--b\nContent-Type: text/plain\n\ntext\n--b--\n'),
    crlf('Content-Type: multipart/mixed; boundary=b\n\n--b\nContent-Type: text/html; charset=latin-1\nContent-Transfer-Encoding: quoted-printable\n\n<a href=3D"https://x/=E9">y</a>=\n\n--b\n'
         'Content-Type: text/plain; charset=utf-8\nContent-Transfer-Encoding: base64\n\naHR0cHM6Ly9jbGF1ZGUuYWkvbWFnaWMtbGluayN0b2s=\n--b--\n'),
    crlf('Content-Type: multipart/mixed; boundary=b\n\n--b\rX: 1\r\rcr only\r--b--\r'), b'Content-Type: multipart/mixed; boundary=b\n\n--b\nX: 1\n\nlf only\n--b--\n',
    b'Content-Type: multipart/mixed; boundary=b\r\n\r\n--b\nX: 1\n\nmixed endings\r--b--',
    crlf('Content-Type: multipart/mixed; boundary=b\n\n--b\nFrom here\n\nfrom line as last header\n--b--\n'), crlf('Content-Type: multipart/mixed; boundary=b\n\n--b\nX: 1\nFrom x\n--b--\n'),
    crlf('Content-Type: multipart/mixed; boundary=b\n\n--b\nno header just text\n--b\n\n--b--\n'), crlf('Content-Type: multipart/mixed; boundary=--\n\n----\n\ndashes\n------\n'),
    crlf('Content-Type: multipart/mixed; boundary=b\n\n--b\n\n\n--b\n\n\r--b--'), crlf('MIME-Version: 1.0\nContent-Type: multipart/mixed;\n boundary="folded"\n\n--folded\n\nfolded param\n--folded--\n'),
    crlf('Content-Type: multipart/mixed;\n\tboundary="tab";\n\tcharset=utf-8\n\n--tab\n\ntab folded\n--tab--\n'),
]


def message_case(raw):
    from email import message_from_bytes
    out = {'raw': b2s(raw)}
    try:
        m = message_from_bytes(raw)
    except (RecursionError, ValueError, AssertionError) as e:
        out['error'] = cls(e)
        return out
    parts = []
    for part in m.walk():
        try:
            charset = part.get_content_charset()
        except ValueError as e:
            charset = '!' + cls(e)
        payload = part.get_payload(decode=True)
        parts.append({'multipart': part.is_multipart(), 'type': part.get_content_type(), 'disposition': part.get_content_disposition(), 'charset': charset,
                      'payload': None if payload is None else b2s(payload)})
    out['parts'] = parts
    out['headers'] = [[k, str(m.policy.header_fetch_parse(k, v))] for k, v in m._headers]
    return out


def chunk_edges(extra):
    """折行、空行、分隔行正好跨在 8192 个字符的边上：Parser.parse 一次喂 8192 个字符，CR 在这一块的最后、LF 在下一块的开头时
    也要当成一个换行"""
    out = []
    head = b'Subject: x\r\n\r\n'
    for tail, shift in ((b'\r\n', 1), (b'\rX', 1), (b'\r\n', 0), (b'\r', 1), (b'\n', 1), (b'\r\n\r\n', 2), (b'\r\n', 2), (b'\rX', 0))[:2 if extra == 1 else 8]:
        out.append(head + b'a' * (8192 - len(head) - shift) + tail + b'line after\r\nlast')
    mp = b'Content-Type: multipart/mixed; boundary=b\r\n\r\n'
    for shift in (3, 1, 0, 2, 4, 5, 6)[:2 if extra == 1 else 7]:
        out.append(mp + b'p' * (8192 - len(mp) - shift) + b'\r\n--b\r\nX: 1\r\n\r\npart\r\n--b--\r\n')
    return out


def message_cases(rng, extra):
    raws = list(MESSAGES) + chunk_edges(extra)
    pool = [b'--b\r\n', b'--b--\r\n', b'--b\n', b'--b', b'--c\r\n', b'--c--\r\n', b'\r\n', b'\n', b'\r', b'text line\r\n', b'X: 1\r\n', b' cont\r\n', b'From x\r\n',
            b'Content-Type: multipart/mixed; boundary=b\r\n', b'Content-Type: multipart/alternative; boundary=c\r\n', b'Content-Type: message/rfc822\r\n',
            b'Content-Type: text/html; charset=latin-1\r\n', b'Content-Type: message/delivery-status\r\n', b'Content-Type: multipart/digest; boundary=c\r\n',
            b'Content-Transfer-Encoding: base64\r\n', b'Content-Transfer-Encoding: quoted-printable\r\n', b'Content-Disposition: attachment\r\n', b'aGVsbG8=\r\n', b'=41=\r\n',
            b': x\r\n', b'--b \r\n', b'--bx\r\n', b'not: header\r\n', b'a b: c\r\n', b'\xe4\xb8\xad\r\n']
    for _ in range(200 * extra):
        raws.append(b''.join(pick(rng, pool, 1, 14)))
    return [message_case(r) for r in dict.fromkeys(raws)]


# --- 头：编码词、地址 ----------------------------------------------------------------------------------------------
TEXTS = [
    '', 'plain', '  spaced   out \t here  ', 'Your secure link to Claude is here', '=?utf-8?q?Secure_link_to_log_in_to_Claude=2Eai?=', '=?UTF-8?Q?caf=C3=A9?=', '=?utf-8?b?Y2Fmw6k=?=',
    '=?utf-8?B?Y2Fmw6k?=', '=?utf-8?B?Y2Fmw6k==?=', '=?utf-8?B?Y2Fmw6k===?=', '=?utf-8?b?Y?=', '=?utf-8?b??=', '=?utf-8?q??=', '=?utf-8?b?!!!!?=', '=?utf-8?b?Y2Fm*w6k=?=',
    '=?iso-8859-1?q?caf=E9?=', '=?latin-1?q?caf=E9?=', '=?latin1?q?caf=E9?=', '=?LATIN_1?Q?caf=E9?=', '=?windows-1252?q?=80uro?=', '=?cp1252?q?=80uro?=', '=?windows-1252?q?=81?=',
    '=?us-ascii?q?plain?=', '=?ascii?q?plain?=', '=?us-ascii?q?caf=E9?=', '=?utf-8?q?bad=FF?=', '=?utf-8?q?=E4=B8?=', '=?nosuchcodec?q?x?=', '=?nosuchcodec?q??=', '=??q?x?=', '=??q?caf=E9?=',
    '=? ?q?x?=', '=?utf-8 ?q?x?=', '=?caf\u00e9?q?x?=', '=?unknown-8bit?q?caf=E9?=', '=?unknown-8bit?q?a?= =?unknown-8bit?q?=FF?=', '=?UNKNOWN-8BIT?b?/w==?= tail', '=?none?q?x?=',
    '=?gbk?b?1tDOxA==?=', '=?gb2312?b?1tDOxA==?=', '=?gb18030?q?=D6=D0?=', '=?big5?b?pKSk5Q==?=', '=?iso-2022-jp?b?GyRCRnxLXDhsGyhC?=', '=?shift_jis?b?k/qWe4zq?=', '=?euc-jp?b?xvzL3A==?=',
    '=?koi8-r?q?=D2=D5=D3?=', '=?iso-8859-2?q?=B1?=', '=?utf-16?b?/v9OLQ==?=', '=?utf-7?q?+AGE-?=', '=?gbk?q??=', '=?gbk?q?ascii?=', '=?utf-8?q?ok?= =?gbk?b?1tA=?=',
    'Re: =?utf-8?q?caf=C3=A9?= and more', '=?utf-8?q?a?= =?utf-8?q?b?=', '=?utf-8?q?a?=  \t =?utf-8?q?b?=', '=?utf-8?q?a?==?utf-8?q?b?=', '=?utf-8?q?a?= x =?utf-8?q?b?=',
    '=?utf-8?q?a?= =?latin-1?q?b?=', '=?latin-1?q?a?= =?iso-8859-1?q?b?=', '=?utf-8?q?a?=\r\n =?utf-8?q?b?=', '=?utf-8?q?a?=\n\t=?utf-8?q?b?=', 'x\r\n y\r\n =?utf-8?q?z?=',
    '=?utf-8?q?a?=\r\n plain\r\n =?utf-8?q?b?=', 'tail =?utf-8?q?a?=', '=?utf-8?q?a?= tail', '=?utf-8?q?a?=tail', 'head=?utf-8?q?a?=', '(=?utf-8?q?a?=)', '=?utf-8?q?a?=(c)', 'x\\=?utf-8?q?a?=',
    '=?utf-8?q?a_b?=', '=?utf-8?q?a=20b?=', '=?utf-8?q?=41=4a=4A?=', '=?utf-8?q?=4?=', '=?utf-8?q?=?=', '=?utf-8?q?==41?=', '=?utf-8?q?=G1?=', '=?utf-8?Q?a?b?=', '=?utf-8?q?a?b?= =?utf-8?q?c?=',
    '=?utf-8?x?a?=', '=?utf-8?q?unterminated', '=?utf-8?q?a?', '=?utf-8?q', '=?', '?=', '=?utf-8?q?a?= =?', '=?utf-8?=?q?a?=', '=?a?b?c?d?e?=', '=?utf-8*en?q?lang?=', '=?utf-8?q?\u4e2d?=',
    '=?utf-8?q?caf\ufffd?=', '=?utf-8?b?Y2Fm\ufffd?=', 'caf\ufffd =?utf-8?q?x?=', 'caf\u00e9 =?utf-8?q?x?=', 'caf\ufffd', 'caf\u00e9', '\u4e2d\u6587 only', '\u8f49\u5bc4: Your secure link to Claude is here',
    '=?utf-8?b?6L2J5a+EOiBZb3VyIHNlY3VyZSBsaW5rIHRvIENsYXVkZSBpcyBoZXJl?=', '=?utf-8?q?Your_secure_link_to_Claude_is_here?= | 2026-11-16', '=?utf-8?q?=1B=5B2K=1B=5B1A=E2=9C=93_=E9=80=9A=E8=BF=87?=',
    '=?utf-8?q?bidi=E2=80=AE?=', '=?utf-8?q?zero=E2=80=8Bwidth?=', '=?utf-8?q?c1=C2=9B31m?=', 'ctl\x07\x1b[31m\x9b raw', 'nbsp\u00a0here', 'line\u2028sep\u2029para', 'vt\x0bff\x0cfs\x1cus\x1f',
    '=?utf-8?q?a?=\x0b=?utf-8?q?b?=', '=?utf-8?q?a?=\x1c=?utf-8?q?b?=', '=?utf-8?q?a?=\u2028=?utf-8?q?b?=', '=?utf-8?q?a?=\u00a0=?utf-8?q?b?=', '=?utf-8?q?a?=\u200b=?utf-8?q?b?=',
    '=?utf-8\n?q?x?=', '=?utf-8?q?x\ny?=', 'a\n=?utf-8?q?b?=', ' \n \n=?utf-8?q?b?=', '=?utf-8?q?a?= \n', '=?utf-8?q?a?=\n\n=?utf-8?q?b?=', '=?utf-8?q?=00?=', '=?utf-8?q? leading?=', '=?utf-8?q?trailing ?=',
    'x =?utf-8?q? leading?=', '=?utf-8?q?trailing ?= x', '=?us-ascii?q?a?= b', 'a =?us-ascii?q?b?=', '=?ascii?q?a?= =?us-ascii?q?b?=', 'a =?ascii?q?b?= c =?utf-8?q?d?=', '=?utf-8?q?a?= =?ascii?q?b?=',
    '=?utf-8?q?(?=x', '=?utf-8?q?a?=)x', 'x(=?utf-8?q?a?=', 'x\\ =?utf-8?q?a?=', '=?utf-8?q?a\\?=x', '=?utf-8?q??= =?utf-8?q?b?=', 'a =?utf-8?q??= b', '=?latin-1?q?=A0?=x',
    '=?utf-8?q?' + 'a' * 200 + '?=', '=?utf-8?b?' + 'QUJD' * 60 + '?=', ' '.join(['=?utf-8?q?w%d?=' % i for i in range(12)]),
]


def text_case(s):
    from email.header import decode_header
    from email.charset import Charset
    out = {'in': s, 'out': mail_text(s)}
    try:   # 用到了 Python 认识、Rust 不认识的字符集：Rust 那边照「解不开用原样的」
        other = False
        for _, cs in decode_header(s):
            if cs is not None:
                codec = Charset(cs).input_codec or 'us-ascii'
                if codec != 'unknown-8bit' and codec_of(codec) == 'other':
                    other = True
        if other:
            out['out_unknown'] = ' '.join(mail_clean(s).split())
    except Exception:
        pass
    return out


def text_cases(rng, extra):
    ss = list(TEXTS)
    pool = ['=?utf-8?q?', '=?UTF-8?B?', '=?latin-1?q?', '=?us-ascii?q?', '=?unknown-8bit?q?', '=?x?q?', '=??b?', '?=', '?=', '?=', ' ', ' ', '\r\n ', '\n', '\t', 'a', 'Y2Fmw6k=', '=C3=A9',
            '=E9', '_', '=', '?', 'x y', '(', ')', '\\', '\ufffd', '\u00e9', 'caf', '=FF', 'QQ==', '\x0b', '\u2028']
    for _ in range(320 * extra):
        ss.append(''.join(pick(rng, pool, 1, 10)))
    return [text_case(s) for s in dict.fromkeys(ss)]


ADDRS = [
    '', ' ', 'alice@example.com', 'Alice@Example.COM', 'Alice <alice@example.com>', '"Alice A." <alice@example.com>', 'alice@example.com (Alice)', '(c) alice@example.com',
    'a (c) @example.com', 'a@(c)example.com', 'a@example.com(c)', 'a@ example.com', 'a @example.com', 'a . b @ example . com', '"a b"@example.com', '"a\\"b"@example.com', '"a@b"@example.com',
    'a@b@example.com', 'a@malicious.example@important.example', '@example.com', 'a@', '@', 'a', 'a b', 'a.b', '.a@example.com', 'a.@example.com', 'a..b@example.com', '<alice@example.com>',
    '<alice@example.com', 'alice@example.com>', '<>', '< >', '<@>', 'x <>', 'Alice <>', '<a@b> <c@d>', 'a@b <c@d>', 'alice@example.com <bob@example.com>', 'a@b, c@d', 'a@b,c@d', 'a@b,,c@d',
    ',a@b', 'a@b,', ',', ',,', 'a@b;c@d', 'a@b; c@d', ';', 'group: a@b, c@d;', 'group:;', 'group: ;', 'group:', 'group: a@b', 'g1: g2: a@b;;', 'undisclosed-recipients:;', ': a@b;', 'a:b:c@d;',
    '"quoted, comma" <a@b>', '"quoted, comma" <a@b>, c@d', '"unterminated <a@b>', 'unterminated" <a@b>', '"a" "b" <c@d>', '"a"b"c" <c@d>', '"\\"" <a@b>', '"\\\\" <a@b>', '"a\\' , '\\',
    'a\\@b@example.com', 'x\\, y <a@b>', '(unbalanced a@b', 'unbalanced) a@b', '((nested) (comments)) a@b', '(a(b(c)d)e) a@b', '(\\() a@b', '(\\)) a@b', '(") a@b', '"(" a@b', '")" <a@b>', '(a) (b) <c@d>',
    'x (a) y (b) <c@d>', '<(c)a@b>', '<a@b(c)>', '<a(c)@b>', '<a@(c)b>', '<@route1,@route2:a@b>', '<@r:a@b>', '<:a@b>', '<a@b>junk', '<a@b> junk@c', 'x <a@b> y', 'x <a@b>, y <c@d>',
    'a@[127.0.0.1]', 'a@[::1]', 'a@[x', 'a@[x]y', 'a@[x].y', '<a@[1.2.3.4]>', 'a@[\\]]', '[a]@b', 'a[@b', 'x [y] <a@b>', 'a@b.', 'a@.b', 'a@b..c', 'a@b c', 'a @ b', 'a@b\r\n c@d', 'a@b\r\n ,c@d',
    'a@b\r\n\t<c@d>', 'x\r\n <a@b>', '"x\r\n y" <a@b>', '(x\r y) a@b', 'a@b\rc', 'a\n@b', ' \t a@b \t ', 'a@b\x00c', 'a\x00@b', 'caf\ufffd <a@b>', 'a\ufffd@b.example.com', 'a@\ufffd.example.com',
    '\ufffd', 'a@b ' * 3, 'no-reply@mail.anthropic.com', 'Anthropic <no-reply@mail.anthropic.com>', '"alice@example.com" <noreply@relay.example.com>', 'alice@example.com <noreply@relay.example.com>',
    '"Alice@Example.com" <noreply@relay.example.com>', '" alice@example.com " <noreply@relay.example.com>', "'alice@example.com' <noreply@relay.example.com>",
    '"alice@example.com" (c) <noreply@relay.example.com>', 'alice@example.com" <noreply@relay.example.com>', '=?utf-8?q?alice=40example.com?= <noreply@relay.example.com>',
    'a@mail.anthropic.com, b@mail.anthropic.com', 'x@evil.example.com (no-reply@mail.anthropic.com)', '"no-reply@mail.anthropic.com" <x@evil.example.com>',
    'no-reply@mail.anthropic.com <x@evil.example.com>', '<no-reply@mail.anthropic.com> x@evil.example.com', 'no-reply@mail.anthropic.com\x00@evil.example.com',
    'no-reply@mail.anthropic.com@evil.example.com', 'no-reply@mail.anthropic.com.evil.example.com', '"no-reply@mail.anthropic.com"@evil.example.com', 'x@evil.example.com, no-reply@mail.anthropic.com',
    'x: no-reply@mail.anthropic.com;', '<x@evil.example.com>no-reply@mail.anthropic.com', 'a@b' + ', c@d' * 40, '(' * 30 + ')' * 30 + ' a@b', '"' + 'x' * 500 + '" <a@b>', 'a' * 500 + '@b',
]
ADDR_LISTS = [
    [], [''], ['', ''], ['a@b', 'c@d'], ['a@b', ''], ['', 'a@b'], ['a@b', '(unbalanced'], ['"x, y" <a@b>', 'c@d'], ['a@b,', 'c@d'], ['a@b', ',c@d'], ['group: a@b;', 'c@d'],
    ['a@b', 'c@d', 'e@f'], ['Alice <alice@example.com>', 'bob@example.com'], ['a@b <c@d>', 'e@f'], ["('', '')"], ['a@b', "('', '')"],
]


def addr_case(values):
    from email.utils import getaddresses, parseaddr
    out = {'values': values}
    try:
        out['pairs'] = [list(p) for p in getaddresses(values)]
        # \u4e0d\u505a\u4e25\u683c\u68c0\u67e5\u65f6\u89e3\u6790\u5668\u81ea\u5df1\u5207\u51fa\u6765\u7684\uff08\u4e25\u683c\u7684\u90a3\u4e00\u5957\u591a\u534a\u6574\u6761\u4e0d\u8ba4\uff0c\u5149\u770b\u5b83\u770b\u4e0d\u51fa\u89e3\u6790\u5668\u5bf9\u4e0d\u5bf9\uff09
        out['loose'] = [list(p) for p in getaddresses(values, strict=False)]
    except RecursionError as e:
        out['error'] = cls(e)
    if len(values) == 1:
        try:
            out['parseaddr'] = list(parseaddr(values[0]))
        except RecursionError:
            out['parseaddr'] = ['', '']
    return out


def addr_cases(rng, extra):
    lists = [[a] for a in ADDRS] + ADDR_LISTS
    pool = ['a', 'b@c', '@', '.', ',', ';', ':', '<', '>', '(', ')', '"', '\\', '[', ']', ' ', ' ', '\t', '\r\n ', 'x y', 'alice@example.com', '<bob@example.com>', '"q"', '(c)', 'g:',
            '\ufffd', 'no-reply@mail.anthropic.com', '\\"', '\\(', "'"]
    for _ in range(150 * extra):
        lists.append([''.join(pick(rng, pool, 1, 9))])
    # \u50cf\u6837\u7684\u5730\u5740\uff0c\u4e00\u5904\u4e00\u5904\u5730\u6539\u574f
    good = ['alice@example.com', 'Alice <alice@example.com>', '"Alice, A." <alice@example.com>', 'alice@example.com (Alice)', '(c) a.b@example.com', 'g: a@b.example, c@d.example;',
            '<@r.example:alice@example.com>', '"a b"@example.com', 'a@[192.0.2.10]', 'Alice (x) <alice@example.com>', '"alice@example.com" <noreply@relay.example.com>', 'a @ b . c']
    junk = ['@', '<', '>', '(', ')', '"', ',', ';', ':', '\\', '[', ']', '.', ' ', 'x', '\ufffd', '\r\n ']
    for _ in range(120 * extra):
        s = ', '.join(pick(rng, good, 1, 3))
        for _ in range(rng.randint(0, 2)):
            at = rng.randrange(len(s) + 1)
            s = s[:at] + rng.choice(junk) + s[at + rng.randint(0, 1):]
        lists.append([s])
    for _ in range(40 * extra):
        lists.append([''.join(pick(rng, pool, 0, 5)) if rng.random() < 0.5 else rng.choice(good) for _ in range(rng.randint(2, 3))])
    seen, out = set(), []
    for v in lists:
        if tuple(v) not in seen:
            seen.add(tuple(v))
            out.append(addr_case(v))
    return out


# --- HTML ----------------------------------------------------------------------------------------------------------
def html_hrefs(text):
    """mail_links 里对 text/html 的那一段（收下的链接还没 strip）"""
    from html.parser import HTMLParser
    links = []

    class Anchors(HTMLParser):
        def handle_starttag(self, tag, attrs):
            if tag == 'a':
                links.extend(v for k, v in attrs if k == 'href' and v)

    try:
        p = Anchors()
        p.feed(text)
        p.close()
    except Exception:
        pass
    return links


BIG = '9' * 4301
HTMLS = [
    '', 'plain text', '<a href="https://claude.ai/magic-link#tok">x</a>', "<a href='https://x/1'>x</a>", '<a href=https://x/2>x</a>', '<A HREF="https://x/3">x</A>', '<a  href = "https://x/4" >x</a>',
    '<a\nhref\n=\n"https://x/5"\n>x</a>', '<a\thref="https://x/6">', '<a\x0chref="https://x/7">', '<a\x0bhref="https://x/8">', '<a\u00a0href="https://x/9">', '<a href="https://x/10"/>', '<a href="https://x/11" />',
    '<a href=https://x/12/>', '<a href=https://x/13 />', '<a/href="https://x/14">', '<a / href="https://x/15">', '<a href="">', '<a href>', '<a href=>', '<a href= >', '<a href="a" href="b">', '<a href="a" HREF="b" hRef=c>',
    '<a class="c" href="https://x/16" style="s">', '<a href="https://x/17" class="c">', '<a href="https://x/18"class="c">', "<a href='https://x/19'class='c'>", '<a href="https://x/20"x>',
    '<a href="x&amp;y">', '<a href="x&amp">', '<a href="x&ampy">', '<a href="x&amp=y">', '<a href="x&amp;=y">', '<a href="x&AMP;y">', '<a href="x&lt;y&gt;">', '<a href="x&numsp;y">', '<a href="x&numsp y">',
    '<a href="x&notit;y">', '<a href="x&not;y">', '<a href="x&noty">', '<a href="x&copy">', '<a href="x&copy;">', '<a href="x&copyright;">', '<a href="x&nosuch;y">', '<a href="x&;y">', '<a href="x&">',
    '<a href="x& y">', '<a href="x&&amp;y">', '<a href="x&amp;amp;y">', '<a href="x&#38;y">', '<a href="x&#38y">', '<a href="x&#38=y">', '<a href="x&#x26;y">', '<a href="x&#X26;y">', '<a href="x&#x26y">',
    '<a href="x&#x;y">', '<a href="x&#;y">', '<a href="x&#xg;y">', '<a href="x&#0;y">', '<a href="x&#13;y">', '<a href="x&#128;y">', '<a href="x&#129;y">', '<a href="x&#159;y">', '<a href="x&#1;y">',
    '<a href="x&#11;y">', '<a href="x&#127;y">', '<a href="x&#xfdd0;y">', '<a href="x&#xfffe;y">', '<a href="x&#x10ffff;y">', '<a href="x&#x110000;y">', '<a href="x&#xd800;y">', '<a href="x&#xdfff;y">',
    '<a href="x&#55296;y">', '<a href="x&#65;&#x41;&#0065;&#x0041;y">', '<a href="x&#99999999999999999999;y">', '<a href="x&#xffffffffffffffffffff;y">', '<a href="x&#%s;y">' % ('9' * 4300),
    '<a href="first"><a href="x&#%s;y"><a href="z">' % BIG, '<a href="first"><b title="&#%s;"><a href="z">' % BIG, '<a href="first">text &#%s; <a href="z">' % BIG,
    '<a href="first">&#x%s; <a href="z">' % ('f' * 500), '<a href="first"><script>&#%s;</script><a href="z">' % BIG,
    '<a href="first"><title>&#%s;</title><a href="z">' % BIG, '<a href="first"><style>&#%s;' % BIG, '<a href="first">&#%s' % ('0' * 4301), '<a href="x&CounterClockwiseContourIntegral;y">',
    '<a href="x&NotEqualTilde;y">', '<a href="x&acE;y">', '<a href="x&hellip;&nbsp;&quot;&apos;y">', '<a href="&#104;ttps://claude.ai/magic-link">', '<a href="https://claude.ai/magic-link?a=1&amp;b=2#tok">',
    '<a href="https://claude.ai/magic-link?a=1&b=2#tok">', '<a href="https://claude.ai/magic-link?a=1&lang=zh">', '<a href="https://claude.ai/magic-link?a=1&para=2">', '<a href="https://claude.ai/magic-link?a=1&para;=2">',
    '<a href="unterminated>x</a><a href="https://x/21">', "<a href='unterminated>x</a><a href='https://x/22'>", '<a href="unterminated>x</a>', "<a href= 'unterminated>x</a><a href=https://x/23>", '<a href= "unterminated>',
    '<a href ="unterminated>x<a href=y>', '<a href="a>b">', "<a href='a>b'>", '<a href=a>b>', '<a href="a"b">', '<a href=a"b>', "<a href=a'b>", '<a href=a=b>', '<a href==a>', '<a =href="x">', '<a "href"="x">',
    '<a href="x"">', "<a 'href='x>", '<a href=x"y"z>', '<a x="1"href="https://x/24">', "<a x='1'href='https://x/25'>", '<a x=1href="y">', '<a/>', '<a>', '<a', '<a ', '<a href', '<a href=', '<a href="x"',
    '<a href="x" ', '<a href=x', '<', '<<a href=x>>', '< a href=x>', '<a\nhref=x', 'a < b <a href="https://x/26">', '1<2 and 3>2 <a href="https://x/27">', '<1a href=x>', '<a1 href=x>', '<ab href=x>', '<a:b href=x>',
    '<a-b href=x>', '<a.b href=x>', '<\u00e9 href=x><a href=y>', '<a\u00e9 href=x>', '<A\u0130 href=x>', '<\u212a href=x>', '<a \u212aref=x href=y>', '<a hre\u017f=x>', '<a HREF\u0130=x>', '</a href=x>', '</ a><a href=y>',
    '</><a href=y>', '</', '</a', '</a <a href=y>', '</a x="<a href=y>"><a href=z>', '</a "x>y" <a href=z>', '</1><a href=y>', '</1 <a href=y>', '</>x</ ><a href=y>',
    '<!-- <a href=x> --><a href=y>', '<!-- <a href=x> --!><a href=y>', '<!--><a href=y>', '<!---><a href=y>', '<!----><a href=y>', '<!-- unterminated <a href=x>', '<!-- a -- b --><a href=y>', '<!-- a --!> b --><a href=y>',
    '<!-- a - -><a href=x> --><a href=y>', '<!--', '<!-', '<!', '<!-->', '<!--a-->b--><a href=y>', '<!--a--!', '<!--a--', '<!--a-', '<!DOCTYPE html><a href=y>', '<!doctype <a href=x>><a href=y>', '<!DOCTYPE <a href=x>',
    '<!foo <a href=x>><a href=y>', '<!><a href=y>', '<![CDATA[ <a href=x> ]]><a href=y>', '<![CDATA[ <a href=x> ]]', '<![CDATA[]]]><a href=y>', '<![cdata[ <a href=x> ]]><a href=y>', '<![CDATA <a href=x>><a href=y>',
    '<? <a href=x> ?><a href=y>', '<?php echo "<a href=x>"; ?><a href=y>', '<? unterminated <a href=x', '<?><a href=y>', '<script><a href=x></script><a href=y>', '<SCRIPT><a href=x></SCRIPT><a href=y>',
    '<script><a href=x></scripty><a href=y></script><a href=z>', '<script><a href=x></script', '<script><a href=x></script ><a href=y>', '<script><a href=x></script/><a href=y>', '<script><a href=x></script\n><a href=y>',
    '<script><a href=x></script\x0b><a href=y>', '<script><a href=x></scr\u0131pt><a href=y>', '<script><a href=x></\u017fcript><a href=y>', '<script/><a href=y>', '<script /><a href=y>', '<script x="/>"><a href=x></script><a href=y>',
    '<script><a href=x>', '<script>', '<script><!-- </script><a href=y> --><a href=z>', '<script></style><a href=x></script><a href=y>', '<style><a href=x></style><a href=y>', '<style><a href=x></STYLE ><a href=y>',
    '<textarea><a href=x></textarea><a href=y>', '<title><a href=x></title><a href=y>', '<title><a href=x>', '<title><a href=x></title', '<TITLE><a href=x></title x="y"><a href=y>', '<title><a href=x></title x="><a href=y>',
    '<title><a href=x></title "><a href=y>"><a href=z>', '<xscript><a href=x></xscript>', '<scriptx><a href=x></scriptx>', '<script x><a href=x></script x><a href=y>', '<script><a href=x></script><script><a href=y></script><a href=z>',
    '<script></script x="\'><a href=y>', '<plaintext><a href=x>', '<xmp><a href=x></xmp>', '<noscript><a href=x></noscript>', '<iframe><a href=x></iframe>', '<a href=x><script>', '<a href="x" script>',
    '<p><a href="https://www.anthropic.com/help">help</a><br><a href=3D"https://claude.ai/magic-link#tok">go</a></p>', '<html><body><a href="https://claude.ai/magic-link#tok-A1:YWxpY2U" style="color:#fff">Sign in</a></body></html>',
    '<a href=" https://x/28 ">', '<a href="\nhttps://x/29\n">', '<a href="https://x/30\x00">', '<a href="https://x/31\u00a0">', '<a href="javascript:alert(1)">', '<a href="  ">', '<a href="\x0b">',
    '<area href="https://x/32"><link href="https://x/33"><base href="https://x/34"><a href="https://x/35">', '<a name="n">no href</a><a href="https://x/36">', '<a href="1"><a href="2"><a href="3">',
    '&lt;a href="x"&gt;<a href="y">', '&#60;a href=x&#62;<a href="y">', 'x & y <a href="z">', 'x &amp <a href="z">', 'x &#38 <a href="z">', 'x &#x26 <a href="z">', 'tail &', 'tail &#', 'tail &#3', 'tail &am', 'tail &amp;x',
    '<a href="x">tail &', '<a href="x">tail <', '<a href="x">tail </', '<a href="x">tail <!', '<a href="x">tail <?', '<a href="x">tail <a', '<a href="x">tail <a href="y',
    '<a href="x"' * 3 + '>', '<a href="x">' * 50, '<a ' + 'x ' * 200 + 'href="deep">', '<a href="' + 'x' * 500 + '">', '<div ' * 20 + '><a href=x>', '<a href=x ' + '/' * 10 + '>', '<a href=x//>', '<a href=x / / >',
    '<a href="x"/ >', '<a/ /href=x>', '<a//href=x>', '<a href=/>', '<a href=/x/>', '<a href= />', "<a href='x' / y='z'>", '<a href=x>>', '<a href=x> >', '<a href=x\n\n>', '<a href=x\r>', '<a href=x\x0c>',
    '<a href=x\x0b>', '<a href=x\u00a0>', '<a href=x \u00a0>', '<a href=x\u2028>', '<a href=x y=\u00a0>', '<a href=x y= \u00a0 >',
]
ATTRS = [
    '', 'plain', '&', '&&', '&amp;', '&amp', '&ampx', '&amp=', '&amp;=', '&AMP;', '&Amp;', '&lt;&gt;&quot;&apos;', '&numsp;', '&numsp', '&notit;', '&not;it', '&not', '&notin;', '&copy', '&copy;', '&copyx', '&copy1',
    '&nosuch;', '&;', '& ', '&a', '&a;', '&1;', '&#', '&#;', '&#x', '&#x;', '&#X;', '&#38', '&#38;', '&#38=', '&#38;=', '&#038;', '&#x26', '&#x26;', '&#X26=', '&#x026;', '&#xG;', '&#3x;', '&#x3G;', '&#0;',
    '&#13;', '&#128;', '&#129;', '&#141;', '&#159;', '&#160;', '&#1;', '&#8;', '&#9;', '&#10;', '&#11;', '&#12;', '&#14;', '&#31;', '&#32;', '&#127;', '&#xfdd0;', '&#xfdef;', '&#xfdf0;', '&#xfffe;', '&#xffff;',
    '&#x1fffe;', '&#x10fffe;', '&#x10ffff;', '&#x110000;', '&#xd7ff;', '&#xd800;', '&#xdfff;', '&#xe000;', '&#55296;', '&#1114111;', '&#1114112;', '&#99999999999999999999;', '&#xffffffffffffffffffff;',
    '&#00000000000000000000000000000065;', '&#x000000000000000000000000000041;', '&#' + '9' * 4300 + ';', '&#' + '9' * 4301 + ';', '&#' + '0' * 4301 + ';', '&#' + '0' * 4298 + '65;',
    '&#x' + 'f' * 5000 + ';', '&#x' + '0' * 500 + '41;', '&CounterClockwiseContourIntegral;', '&CounterClockwiseContourIntegralx;', '&NotEqualTilde;', '&acE;', '&bne;', '&fjlig;', '&ThickSpace;',
    'a&amp;b&lt;c', 'a=1&b=2', 'a=1&amp;b=2', 'a=1&lang=zh', 'a=1&para=2', 'a=1&para;=2', 'a=1&para', 'a=1&sect=2&copy=3', '&amp;amp;', '&amp;#38;', '&#38;amp;', '&&amp;&', '&amp&amp', '&\u00e9;', '&a\u00e9;',
    '&amp\u00e9', '&#\u0661;', '&#x\uff21;', 'https://claude.ai/magic-link?x=1&amp;y=2#tok', '&gt', '&GT', '&gt;', '&gT;', '&LT', '&QUOT', '&quot', '&apos', '&apos;', '&nbsp', '&nbsp;', '&shy', '&reg', '&REG', '&times',
    '&timesb;', '&timesbar;', '&divide', '&micro', '&middot', '&cedil', '&ordf', '&ordm', '&frac12', '&frac12;', '&frac13;', '&sup1', '&sup2', '&sup3', '&yen', '&yuml', '&Yuml;', '&ETH', '&eth', '&THORN', '&thorn',
    '&szlig', '&uml', '&macr', '&iexcl', '&iquest', '&laquo', '&raquo', '&curren', '&brvbar', '&cent', '&pound', '&deg', '&plusmn', '&acute', '&para', '&sect', '&AElig', '&aelig', '&Aring', '&aring',
]


def html_cases(rng, extra):
    docs = list(HTMLS)
    pool = ['<a', '<a ', '</a>', '</a', '<', '>', '>', '/', '/>', ' ', ' ', '\n', '\t', 'href', 'href=', '=', '"', "'", 'https://x/y', 'x', '&amp;', '&#38;', '&', '<!--', '-->', '--!>', '<!', '<?', '?>',
            '<![CDATA[', ']]>', '<script>', '</script>', '<script', '<style>', '</style>', '<title>', '</title>', '<textarea>', '</textarea ', '<b>', '<p class="c">', 'text', ' y=z', ' y="z"', " y='z'",
            '\u00a0', '\x0b', '\x0c', '\r', '<A HREF=', '</', '<a href="1">', "<a href='2'>", '<a href=3>', '-', '!', '<!DOCTYPE', 'href="u', "href='v", '\u00e9']
    for _ in range(420 * extra):
        docs.append(''.join(pick(rng, pool, 1, 12)))
    return [{'in': d, 'hrefs': html_hrefs(d)} for d in dict.fromkeys(docs)]


def attr_cases(rng, extra):
    from html.parser import _unescape_attrvalue
    vals = list(ATTRS)
    pool = ['&', '&', '#', '#x', '#X', ';', ';', '=', 'amp', 'lt', 'not', 'notin', 'numsp', 'copy', 'x', '3', '8', '26', 'g', ' ', '0', '65', 'fffe', 'd800', '\u00e9', 'para', 'AMP', 'gt']
    for _ in range(160 * extra):
        vals.append(''.join(pick(rng, pool, 1, 8)))
    out = []
    for v in dict.fromkeys(vals):
        c = {'in': v}
        try:
            c['out'] = _unescape_attrvalue(v)
        except ValueError:
            pass   # 数字太长：Rust 那边是 None
        out.append(c)
    return out


# --- 核对 ----------------------------------------------------------------------------------------------------------
TAGS = [
    '', 'v=1', 'v=1; a=rsa-sha256; d=mail.anthropic.com; s=s1; h=from:to:subject; bh=abc=; b=AbCd+/==', 'V=1; D=Example.COM', ' v = 1 ; d = x . y ', 'd=a; d=b', 'd=;d=b', '=x; d=a', ';;d=a;;',
    'd', 'd=a=b', 'h=from:\r\n\tto:\r\n\tsubject', 'b=AAAA\r\n\t BBBB\r\n\t CCCC', 'b=A\u00a0B\u2028C\x0bD\x1cE', 'd=x\ufffdy', '\u00a0d\u00a0=a', 'D\u0130=a', '\u212a=a', 'k=v;K=w', 'a=1;\r\n b=2;\r\n', 'h=From : To',
    'x=\x00y', 'a==', '=', '==', ';', ' ; ', 'a=b;c', 'a;b=c', 'h=:from::to:', 'b=', 'b= ', 'i=@mail.anthropic.com; d=mail.anthropic.com', 'd="mail.anthropic.com"', "d='x'", 'd=x;d =y;d= z',
]
AR_ITEMS = [
    '', ' ', 'mx.google.com', 'mx.google.com;', 'MX.Google.Com; dkim=pass', 'mx.google.com; dkim=pass header.i=@mail.anthropic.com header.s=s1 header.b=AbCdEfGh',
    'mx.google.com; dkim=pass header.i=@mail.anthropic.com header.s=s1 header.b=AbCdEfGh; spf=pass (google.com: domain of x designates 192.0.2.10 as permitted sender) smtp.mailfrom=x@y; dmarc=pass (p=REJECT) header.from=y',
    'mx.google.com; none', 'mx.google.com; none; dkim=pass', 'mx.google.com; None', 'mx.google.com; none none', 'mx.google.com;;dkim=pass', 'mx.google.com; ; ;', 'mx.google.com 1; dkim=pass', 'mx.google.com (c); dkim=pass',
    '(c) mx.google.com; dkim=pass', 'mx.google.com; dkim=pass (ok)', 'mx.google.com; dkim=pass (ok (nested)) header.b=x', 'mx.google.com; dkim=pass (ok \\) still) header.b=x', 'mx.google.com; dkim=pass (ok \\( still) header.b=x',
    'mx.google.com; dkim=pass (unterminated header.b=x', 'mx.google.com; dkim=pass unbalanced) header.b=x', 'mx.google.com; dkim=pass (a)) header.b=x', 'mx.google.com; dkim=pass ((a) header.b=x',
    'mx.google.com; dkim=pass (a\\', 'mx.google.com; dkim=pass (a\\)', 'mx.google.com; dkim=pass \\(a) header.b=x', 'mx.google.com; dkim=pass "quoted"', 'mx.google.com; dkim="pass"', 'mx.google.com; dkim=pass header.i="a b"@x',
    'mx.google.com; dkim=pass header.i="a;b=c(d)"@x header.b=y', 'mx.google.com; dkim=pass header.i="a\\"b"@x', 'mx.google.com; dkim=pass header.i="a\\\\"@x', 'mx.google.com; dkim=pass "unterminated',
    'mx.google.com; dkim=pass "a\\', 'mx.google.com; dkim=pass "a\\"', 'mx.google.com; dkim=pass ""', 'mx.google.com; dkim=pass "" x=y', 'mx.google.com; dkim=pass ""=y', 'mx.google.com; dkim=pass x=""',
    'mx.google.com; dkim=pass x="="', 'mx.google.com; "dkim"=pass', 'mx.google.com; dkim pass', 'mx.google.com; dkim= pass', 'mx.google.com; dkim =pass', 'mx.google.com; dkim = pass', 'mx.google.com; dkim==pass',
    'mx.google.com; dkim=pass=x', 'mx.google.com; dkim=', 'mx.google.com; =pass', 'mx.google.com; dkim', 'mx.google.com; dkim=pass x', 'mx.google.com; dkim=pass x=', 'mx.google.com; dkim=pass x=y z',
    'mx.google.com; dkim=pass x=y=z', 'mx.google.com; DKIM=PASS Header.I=@X', 'mx.google.com; dkim=pass header.d=a header.d=b', 'mx.google.com; a=b; c=d; e=f', 'mx.google.com=x; dkim=pass', '=; dkim=pass',
    '; dkim=pass', ';', ';;', '=', 'a b; dkim=pass', '"mx.google.com"; dkim=pass', 'mx.google.com\t;\tdkim=pass', 'mx.google.com; dkim=pass\u00a0header.b=x', 'mx.google.com; dkim=pass\x1cheader.b=x',
    'mx.google.com; dkim=pass\u2028header.b=x', 'mx.google.com; dkim=pass\x00header.b=x', 'mx.google.com; dkim=pass header.b=\ufffd', 'mx.google.com; dkim=pass (c1)(c2) header.b=x', 'mx.google.com; dkim=pass(c)header.b=x',
    'mx.google.com; dkim(c)=(c)pass', 'mx.google.com; d(k)im=pass', 'mx.google.com; arc=pass (i=1 spf=pass spfdomain=x dkim=pass dkdomain=y); dkim=fail', 'mx.google.com; dkim=pass ()', 'mx.google.com; dkim=pass (())',
    'mx.google.com; dkim=pass ("); x=y', 'mx.google.com; dkim=pass "(" x=y', 'mx.google.com; dkim=pass ")" x=y', 'mx.google.com; dkim=pass ";" x=y', 'mx.google.com; dkim=pass x=";"; y=z',
    'spf.protection.outlook.com; dkim=pass header.d=mail.anthropic.com', 'mx.google.com.evil.example.com; dkim=pass', 'mx.google.com ; dkim=pass', ' mx.google.com; dkim=pass', 'mx.google.com;dkim=pass;spf=pass',
]


def ar_cases(rng, extra):
    vs = list(AR_ITEMS)
    pool = ['mx.google.com', ';', ';', '=', '=', '(', ')', '"', '\\', ' ', ' ', 'dkim', 'pass', 'fail', 'none', 'header.i', 'header.b', 'header.d', '@mail.anthropic.com', 'AbCdEfGh', 'x', 'spf', '\u00a0', '\t']
    for _ in range(160 * extra):
        vs.append(''.join(pick(rng, pool, 1, 12)))
    return [{'in': v, 'out': (lambda r: None if r is None else [r[0], [[list(p) for p in pairs] for pairs in r[1]]])(mail_ar_items(v))} for v in dict.fromkeys(vs)]


SUBJECTS = [
    '', 'Your secure link to Claude is here', 'Your secure link to Claude is here | 2026-11-16 10:36:30', '\u8f49\u5bc4: Your secure link to Claude is here', 'Secure link to log in to Claude.ai | 2026-11-15',
    'Your secure link to Claude.ai is here', 'Secure link to log in to Claude.ai', 'Your secure link to Verification Portal is here', 'Your receipt from Anthropic', 'Secure link to log in to Claude-ai',
    'Your secure link to Claudexyz is here', 'SECURE LINK TO CLAUDE', 'secure link to claude', 'secure link to claude.', 'secure link to claude.ai.', 'secure link to claude.ai-x', 'secure link to claude.aix',
    'secure link to claude.a', 'secure link to claude.ai', 'secure link to claude.AI!', 'secure link to claude_', 'secure link to claude-', 'secure link to claude1', 'secure link to claude\u00e9', 'secure link to claude\u4e2d',
    'secure link to claude\u00b2', 'secure link to claude\u00bd', 'secure link to claude\u0301', 'secure link to claude\u200b', 'secure link to claude\u00a0', 'secure link to claude\n', 'secure link to claude,',
    'secure link to claude)', 'secure link to claude.ai\u00e9', 'secure link to claude.ai_', 'secure link to claude.ai1', 'secure link to claude.ai ', 'secure link to claude .ai', 'secure link to  claude',
    'secure  link to claude', 'secure link to\tclaude', 'secure link to\u00a0claude', 'secure link toclaude', 'securelink to claude', 'insecure link to claude', '_secure link to claude', '1secure link to claude',
    '\u00e9secure link to claude', '\u4e2dsecure link to claude', '\u00b2secure link to claude', '\u00bdsecure link to claude', '\u0301secure link to claude', '-secure link to claude', '.secure link to claude',
    ' secure link to claude', '\u200bsecure link to claude', '(secure link to claude)', 'x secure link to log in to claude', 'secure link to log in to claude', 'secure link to log in to claude.ai',
    'secure link to log in to  claude', 'secure link to log in to', 'secure link to log in to claudex', 'secure link to log in to log in to claude', 'secure link to log in claude', 'secure link to login to claude',
    'secure link to claude secure link to claude.aix', 'secure link to claudex secure link to claude', 'secure link to claude.aix secure link to claude.ai', 'secure link to secure link to claude',
    '\u017fecure link to claude', 'SECURE L\u0130NK TO CLAUDE', 'secure l\u0131nk to claude', 'secure lin\u212a to claude', 'secure link to c\u0142aude', 'secure link to \u0441laude', 'secure link to claud\u0435',
    's\u00e9cure link to claude', 'secure link to cla\u00fcde', 'Secure Link To Claude.Ai', 'sEcUrE lInK tO cLaUdE', 'secure link to CLAUDE.AI', 'secure link to claude.Ai', 'secure link to claude.aI',
    'secure link to claude.\u0131', 'secure link to claude.a\u0131', 'secure link to claude.a\u0130', 'secure link to c\u217cAUDE', 'secure\u00a0link to claude', 'secure link\u2003to claude',
    'your secure link to claude is here ' * 3, 'x' * 300 + ' secure link to claude', 'secure link to claud', 'secure link to', 'secure link', 'claude', 'claude.ai',
]


def subject_cases(rng, extra):
    ss = list(SUBJECTS)
    pool = ['secure link to ', 'Secure Link To ', 'log in to ', 'claude', 'Claude', 'CLAUDE', '.ai', '.AI', ' ', ' ', '.', '-', '_', 'x', '1', '\u00e9', '\u017f', '\u0130', '\u0131', '\u212a', 'secure', 'link', 'to',
            '\u4e2d', '!', '\u00b2', '\u0301', '\n', 'ecure link to claude', 's']
    for _ in range(320 * extra):
        ss.append(''.join(pick(rng, pool, 1, 8)))
    return [{'in': s, 'ok': RULE.subject_ok(s)} for s in dict.fromkeys(ss)]


LINKS = [
    'https://claude.ai/magic-link#tok-abc:YWxpY2U', 'https://claude.ai/magic-link?x=1#t', 'http://claude.ai/magic-link#t', 'https://claude.ai.evil.com/magic-link#t', 'https://claude.ai@evil.com/magic-link#t',
    'https://evil.com/claude.ai/magic-link', 'https://claude.ai:443/magic-link#t', 'https://Claude.ai/magic-link#t', 'https://claude.ai/magic-linkx#t', 'https://claude.ai/magic-link/../logout',
    'https://claude.ai/login#t', 'https://claude.ai/magic-link#t"onload', "https://claude.ai/magic-link#t'x", 'https://claude.ai/magic-link#a<b', 'https://claude.ai/magic-link#a\\b', 'https://claude.ai/magic-link# t',
    'https://claude.ai/magic-link#\u4e2d', 'https://claude.ai/magic-link#' + 'a' * 2048, 'https://claude.ai/magic-link#' + 'a' * 2020, 'javascript:alert(1)', '', 'https://claude.ai/magic-link',
    'https://claude.ai/magic-link\n', 'https://claude.ai/magic-link#t\n', 'https://claude.ai/magic-link#t\n\n', '\nhttps://claude.ai/magic-link', 'https://claude.ai/magic-link\r', 'https://claude.ai/magic-link\t',
    ' https://claude.ai/magic-link', 'HTTPS://claude.ai/magic-link', 'Https://claude.ai/magic-link#t', 'https:/claude.ai/magic-link', 'https:claude.ai/magic-link', 'https:///claude.ai/magic-link', '//claude.ai/magic-link',
    'https://claude.ai./magic-link', 'https://claude.ai/magic-link/', 'https://claude.ai//magic-link', 'https://claude.ai/Magic-Link', 'https://claude.ai/magic-link;x', 'https://claude.ai/magic-link%23t',
    'https://claude.ai/magic-link?', 'https://claude.ai/magic-link#', 'https://claude.ai/magic-link?#', 'https://claude.ai/magic-link?a=b&c=d#e=f&g', 'https://claude.ai?x/magic-link', 'https://claude.ai#/magic-link',
    'https://claude.ai:/magic-link', 'https://:claude.ai/magic-link', 'https://user@claude.ai/magic-link', 'https://claude.ai:80/magic-link', 'https://[claude.ai]/magic-link', 'https://[::1]/magic-link',
    'https://claude.ai[/magic-link', 'https://claude.ai]/magic-link', 'https://xn--claude-.ai/magic-link', 'https://www.claude.ai/magic-link', 'https://claude.ai%2emagic-link', 'https://claude.ai\\@evil.com/magic-link',
    'https://claude.ai/magic-link#\x7f', 'https://claude.ai/magic-link#\x80', 'https://claude.ai/magic-link#~!$&()*+,;=:@/?', 'https://claude.ai/magic-link#`{|}^', 'https://claude.ai/magic-link#[x]',
    'https://anthropic.com/magic-link', 'https://console.anthropic.com/magic-link', 'ftp://claude.ai/magic-link', 'https+x://claude.ai/magic-link', 'h\ttps://claude.ai/magic-link', 'https://cla\tude.ai/magic-link',
    'https://claude.ai/magic-link?a=' + 'b' * 3000, 'https://ex\u2100mple.com/', 'https://[v1.x]/magic-link', 'https://claude.ai/magic-link#tok-A1:YWxpY2VAZXhhbXBsZS5jb20', 'not a url', 'mailto:a@b', '#frag', '?q',
    'https://links.example.com/ls/click?u=https%3A%2F%2Fclaude.ai', 'https://www.anthropic.com/help',
]
DOMAINS = ['anthropic.com', 'mail.anthropic.com', 'MAIL.Anthropic.COM', 'evilanthropic.com', 'anthropic.com.evil.com', '', 'a.mail.anthropic.com', '.anthropic.com', 'anthropic.com.', 'anthropic', 'com',
           'relay.example.com', 'sub.relay.example.com', 'RELAY.EXAMPLE.COM', 'xrelay.example.com', 'relay.example.com.evil.com', 'example.com', '.relay.example.com', 'anthropic.com\n', '\ufffd.anthropic.com',
           'mail.anthropic.com\u212a', 'MAIL.ANTHROP\u0130C.COM', 'mail.\u0131anthropic.com']


def judge_emails():
    """(名字, 邮件, [(got, since, acct, forwarder)…])：tests/mail_view.py 里核对用的那些邮件，加上各种边角"""
    sys.path.insert(0, os.path.join(REPO, 'tests'))
    import mail_fake as mf
    mf.email.utils.formatdate = lambda *a, **kw: 'Sun, 15 Nov 2026 10:00:00 -0000'   # 生成的邮件每次都一样
    random.seed(20261116)
    out = []
    now = 1794708000
    std = [(now, now - 60, 'alice@example.com', None)]
    fwd = [(now, now - 60, 'alice@example.com', 'relay.example.com')]

    def add(name, raw, runs=std):
        out.append((name, raw, runs))

    def E(to, **kw):   # 只看头的用例：正文只留纯文本那一半（对照数据小一些）
        kw.setdefault('html', False)
        return mf.build_eml(to, **kw)

    good = E('Alice@Example.com')
    add('good', good, [(now, now - 60, 'alice@example.com', None), (now - 200, now - 60, 'alice@example.com', None), (now - 100, now - 60, 'alice@example.com', None),
                       (now - 120, now - 60, 'alice@example.com', None), (now - 121, now - 60, 'alice@example.com', None), (now, now - 60, '', None), (now, now - 60, 'bob@example.com', None),
                       (now, now - 60, 'alice@example.com', 'relay.example.com'), (now, now - 60, 'alice@example.com', ''), (now, now - 60.5, 'alice@example.com', None)])
    add('subject receipt', E('alice@example.com', subject='Your receipt'))
    add('subject portal', E('alice@example.com', subject='Your secure link to Verification Portal is here'))
    add('subject encoded', E('alice@example.com', subject='=?utf-8?q?Secure_link_to_log_in_to_Claude=2Eai?='))
    add('subject folded', E('alice@example.com', subject='Your secure link to\r\n Claude is here'))
    add('subject missing', E('alice@example.com').replace(b'\r\nSubject: Your secure link to Claude is here', b'', 1))
    add('dup from', E('alice@example.com', extra=['From: Anthropic <no-reply@mail.anthropic.com>']))
    add('dup to', E('alice@example.com', extra=['To: alice@example.com']))
    add('dup subject', E('alice@example.com', extra=['Subject: Your secure link to Claude is here']))
    add('dup subject case', E('alice@example.com', extra=['SUBJECT: other']))
    add('from evil', E('alice@example.com', frm='Anthropic <no-reply@evilanthropic.com>'))
    add('from two', E('alice@example.com', frm='a@mail.anthropic.com, b@mail.anthropic.com'))
    add('from none', E('alice@example.com', frm='Anthropic'))
    add('from empty', E('alice@example.com', frm=''))
    add('from display trick', E('alice@example.com', frm='"no-reply@mail.anthropic.com" <x@evil.example.com>'))
    add('from comment trick', E('alice@example.com', frm='x@evil.example.com (no-reply@mail.anthropic.com)'))
    add('from two ats', E('alice@example.com', frm='no-reply@mail.anthropic.com@evil.example.com'))
    add('from angle after', E('alice@example.com', frm='no-reply@mail.anthropic.com <x@evil.example.com>'))
    add('from group', E('alice@example.com', frm='g: no-reply@mail.anthropic.com;'))
    add('from upper', E('alice@example.com', frm='NO-REPLY@MAIL.ANTHROPIC.COM'))
    add('from apex', E('alice@example.com', frm='no-reply@anthropic.com', dkim_d='anthropic.com'))
    add('from 8bit', E('alice@example.com').replace(b'From: Anthropic <', b'From: Anthr\xf6pic <', 1))
    add('from deep comment', E('alice@example.com', frm='(' * 600 + ')' * 600 + ' no-reply@mail.anthropic.com'))
    add('to deep comment', E('(' * 600 + ')' * 600 + ' alice@example.com'))
    add('ar fail', E('alice@example.com', ar='fail'))
    add('ar none', E('alice@example.com', ar='none'))
    add('ar missing', E('alice@example.com', ar='missing'))
    add('dkim other domain', E('alice@example.com', dkim_d='evil.com'))
    add('dkim subdomain signs', E('alice@example.com', dkim_d='anthropic.com'))
    add('unsigned to', E('alice@example.com', sign='from:subject:date'))
    add('unsigned from', E('alice@example.com', sign='to:subject:date'))
    add('unsigned subject', E('alice@example.com', sign='from:to:date'))
    add('signed upper', E('alice@example.com', sign='From : TO:Subject'))
    add('signed empty', E('alice@example.com', sign=''))
    add('to bob', E('bob@example.com'))
    add('to list', E('bob@example.com, Alice <alice@example.com>'))
    add('to display trick', E('"alice@example.com" <bob@example.com>'))
    add('to malformed', E('alice@example.com <bob@example.com>'))
    add('to group', E('team: alice@example.com, bob@example.com;'))
    forged = 'Authentication-Results: mx.google.com;\r\n       dkim=pass header.i=@mail.anthropic.com header.s=s1 header.b=AAAA'
    add('forged below', E('alice@example.com', ar='fail', extra=[forged]))
    add('other server on top', E('alice@example.com', above=['Authentication-Results: spf.protection.outlook.com; dkim=pass header.d=mail.anthropic.com header.b=AAAA']))
    add('comment pass', E('alice@example.com', ar='missing', above=['Authentication-Results: mx.google.com;\r\n       arc=pass (i=1 dkim=pass dkdomain=mail.anthropic.com);\r\n'
                                                                            '       spf=pass smtp.mailfrom=bounce@mail.anthropic.com']))

    def ar_mail(*results, **kw):
        raw = E('alice@example.com', ar='missing', **kw)
        b8 = mail_tags(mail_parse(raw)['DKIM-Signature'])['b'][:8]
        head = 'Authentication-Results: mx.google.com;\r\n       ' + ';\r\n       '.join(r.format(b=b8) for r in results)
        return head.encode() + b'\r\n' + raw

    add('ar control', ar_mail('dkim=pass header.i=@mail.anthropic.com header.s=s1 header.b={b}'))
    add('ar nested comment', ar_mail('dkim=pass (ok \\) (nested)) header.i=@mail.anthropic.com header.s=s1 header.b={b}'))
    for i, res in enumerate((
            'dkim=fail header.i=@mail.anthropic.com header.b={b}; spf=pass smtp.mailfrom="x;dkim=pass header.i=@mail.anthropic.com header.b={b}"@evil.example.com',
            'dkim=pass header.i="a header.d=mail.anthropic.com"@evil.example.com header.s=s1 header.b={b}',
            'dkim=pass header.i=@evil.example.com header.i=@mail.anthropic.com header.b={b}',
            'dkim=pass header.i=@mail.anthropic.com header.s=s1 header.b=AbC',
            'dkim=pass header.i=@mail.anthropic.com header.s=s1',
            'dkim=pass header.i=@mail.anthropic.com header.b={b} "unterminated',
            'dkim=pass header.i=@mail.anthropic.com (unterminated header.b={b}',
            'dkim pass header.i=@mail.anthropic.com header.b={b}')):
        add('ar not gmail-written %d' % i, ar_mail(res))
    add('ar header.d', ar_mail('dkim=pass header.d=mail.anthropic.com header.b={b}'))
    add('ar header.d wins over i', ar_mail('dkim=pass header.d=evil.example.com header.i=@mail.anthropic.com header.b={b}'))
    add('ar header.d empty', ar_mail('dkim=pass header.d="" header.i=@mail.anthropic.com header.b={b}'))
    add('ar header.i local', ar_mail('dkim=pass header.i=no-reply@mail.anthropic.com header.b={b}'))
    add('ar header.i odd local', ar_mail('dkim=pass header.i=a!b@mail.anthropic.com header.b={b}'))
    add('ar upper', ar_mail('DKIM=PASS HEADER.I=@MAIL.ANTHROPIC.COM HEADER.B={b}'))
    add('ar two dkim, second passes', ar_mail('dkim=fail header.i=@evil.example.com header.b=ZZZZZZZZ', 'dkim=pass header.i=@mail.anthropic.com header.b={b}'))
    add('ar pass for other domain only', ar_mail('dkim=pass header.i=@evil.example.com header.b={b}'))
    add('ar b prefix longer than sig', ar_mail('dkim=pass header.i=@mail.anthropic.com header.b={b}' + 'x' * 400))
    add('ar b with slash', ar_mail('dkim=pass header.i=@mail.anthropic.com header.b=+/+/+/+/'))
    add('ar b bad chars', ar_mail('dkim=pass header.i=@mail.anthropic.com header.b={b}-'))
    add('ar domain bad', ar_mail('dkim=pass header.i=@mail_anthropic.com header.b={b}'))
    add('ar authserv upper', ar_mail('dkim=pass header.i=@mail.anthropic.com header.b={b}').replace(b'mx.google.com', b'MX.GOOGLE.COM', 1))
    add('ar authserv with version', ar_mail('dkim=pass header.i=@mail.anthropic.com header.b={b}').replace(b'mx.google.com;', b'mx.google.com 1;', 1))
    add('ar none then dkim', ar_mail('none', 'dkim=pass header.i=@mail.anthropic.com header.b={b}'))
    add('ar 8bit', ar_mail('dkim=pass header.i=@mail.anthropic.com header.b={b}').replace(b'header.s=s1', b'header.s=\xe9', 1))
    add('ar policy', ar_mail('dkim=policy header.i=@mail.anthropic.com header.b={b}'))
    add('ar temperror', ar_mail('dkim=temperror (no key) header.i=@mail.anthropic.com header.b={b}'))
    raw = E('alice@example.com')
    sig = mail_parse(raw)['DKIM-Signature']
    twin = 'DKIM-Signature: ' + ' '.join(str(sig).split()).replace('h=from:to:subject', 'h=date')
    add('twin signature', raw.replace(b'\r\nFrom:', ('\r\n' + twin + '\r\nFrom:').encode(), 1))
    other = 'DKIM-Signature: v=1; a=rsa-sha256; d=mail.anthropic.com; s=s2; h=date; bh=x=; b=ZZZZZZZZZZZZ'
    add('other signature', raw.replace(b'\r\nFrom:', ('\r\n' + other + '\r\nFrom:').encode(), 1))
    evil = 'DKIM-Signature: v=1; a=rsa-sha256; d=evil.example.com; s=s2; h=from:to:subject; bh=x=; b=' + mail_tags(sig)['b']
    add('same b, other domain signature', raw.replace(b'\r\nFrom:', ('\r\n' + evil + '\r\nFrom:').encode(), 1))
    add('no signature header', re.sub(rb'DKIM-Signature: .*?\r\n(?=[A-Z])', b'', raw, count=1, flags=re.S))

    def fw(alias='alice@example.com', frm=None, dkim_d='relay.example.com', sign='from:to:subject:reply-to:date', ar='pass', reply_to='Anthropic <no-reply@mail.anthropic.com>',
           subject='Your secure link to Claude is here'):
        return E('collect@gmail.com', frm=frm if frm is not None else '"%s" <noreply@relay.example.com>' % alias, dkim_d=dkim_d, sign=sign, ar=ar, reply_to=reply_to, subject=subject)

    add('fwd good', fw(), fwd + std + [(now, now - 60, 'alice@example.com', 'example.com'), (now, now - 60, 'alice@example.com', 'evil.com'), (now, now - 60, 'bob@example.com', 'relay.example.com'),
                                       (now, now - 60, 'alice@example.com', 'RELAY.example.com'), (now, now - 60, 'alice@example.com', 'sub.relay.example.com'), (now - 200, now - 60, 'alice@example.com', 'relay.example.com')])
    add('fwd reply-to evil', fw(reply_to='X <a@evil.com>'), fwd)
    add('fwd reply-to missing', fw(reply_to=None), fwd)
    add('fwd reply-to two', fw(reply_to='no-reply@mail.anthropic.com, x@mail.anthropic.com'), fwd)
    add('fwd reply-to twice', fw().replace(b'\r\nReply-To:', b'\r\nReply-To: x@mail.anthropic.com\r\nReply-To:', 1), fwd)
    add('fwd alias bob', fw(frm='"bob@example.com" <noreply@relay.example.com>'), fwd)
    add('fwd no alias', fw(frm='noreply@relay.example.com'), fwd)
    add('fwd alias unquoted', fw(frm='alice@example.com <noreply@relay.example.com>'), fwd)
    add('fwd alias upper', fw(frm='"Alice@Example.COM" <noreply@relay.example.com>'), fwd)
    add('fwd alias padded', fw(frm='" alice@example.com " <noreply@relay.example.com>'), fwd)
    add('fwd alias with comment', fw(frm='"alice@example.com" (x) <noreply@relay.example.com>'), fwd)
    add('fwd sub relay', fw(frm='"alice@example.com" <noreply@mx.relay.example.com>', dkim_d='mx.relay.example.com'), fwd)
    add('fwd unsigned reply-to', fw(sign='from:to:subject:date'), fwd)
    add('fwd ar fail', fw(ar='fail'), fwd)
    add('fwd signed by evil', fw(dkim_d='evil.com'), fwd)
    add('fwd signed by anthropic', fw(dkim_d='mail.anthropic.com'), fwd)
    add('fwd subject portal', fw(subject='Your secure link to Verification Portal is here'), fwd)
    add('fwd subject suffix', fw(subject='Your secure link to Claude is here | 2026-11-16 10:36:30'), fwd)
    # 正文
    add('body two links', mf.build_eml('alice@example.com', body_links=['https://www.anthropic.com/help', 'https://claude.ai/magic-link#tok-X']))
    add('body text only', mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-T', html=False))
    add('body html only', mf.build_eml('alice@example.com', link='https://claude.ai/magic-link#tok-H', text=False))
    add('body tracking link', mf.build_eml('alice@example.com', body_links=['https://links.example.com/ls/click?u=https%3A%2F%2Fclaude.ai']))
    add('body no parts', mf.build_eml('alice@example.com', html=False, text=False))
    add('body query link', mf.build_eml('alice@example.com', link='https://claude.ai/magic-link?a=1&b=2#tok'))
    head = b'From: no-reply@mail.anthropic.com\r\n'
    add('attachment ignored', head + b'Content-Type: multipart/mixed; boundary="b"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nhello\r\n--b\r\nContent-Type: text/html\r\n'
        b'Content-Disposition: attachment; filename=x.html\r\n\r\n<a href="https://claude.ai/magic-link#tok-att">x</a>\r\n--b--\r\n')
    add('inline disposition', head + b'Content-Type: text/html\r\nContent-Disposition: inline\r\n\r\n<a href="https://claude.ai/magic-link#tok-in">x</a>\r\n')
    add('plain html top level', head + b'Content-Type: text/html\r\n\r\n<a href="https://claude.ai/magic-link#tok-top">x</a>\r\n')
    add('no content type', head + b'\r\nvisit https://claude.ai/magic-link#tok-plain now\r\n')
    add('text punctuation', head + b'\r\n(https://claude.ai/magic-link#a) <https://claude.ai/magic-link#b> "https://claude.ai/magic-link#c" https://claude.ai/magic-link#d. https://x/y\\z\r\n'
        b'https://claude.ai/magic-link#e,f;g\r\nhttps://claude.ai/magic-link#h\tHTTPS://claude.ai/magic-link#i http://claude.ai/magic-link#j https://\r\n')
    add('text nbsp ends link', head + b'Content-Type: text/plain; charset=utf-8\r\n\r\nhttps://claude.ai/magic-link#k\xc2\xa0next https://claude.ai/magic-link#l\xe3\x80\x80x https://claude.ai/magic-link#m\xe4\xb8\xad\r\n')
    add('text latin-1', head + b'Content-Type: text/plain; charset=iso-8859-1\r\n\r\nhttps://claude.ai/magic-link#n\xa0x https://claude.ai/magic-link#o\xe9\r\n')
    add('text cp1252', head + b'Content-Type: text/plain; charset=windows-1252\r\n\r\nhttps://claude.ai/magic-link#p\x85 https://claude.ai/magic-link#q\x81 \x93https://claude.ai/magic-link#r\x94\r\n')
    add('text ascii 8bit', head + b'Content-Type: text/plain; charset=us-ascii\r\n\r\nhttps://claude.ai/magic-link#s\xe9 x\r\n')
    add('text bad utf8', head + b'Content-Type: text/plain; charset=utf-8\r\n\r\nhttps://claude.ai/magic-link#t\xff\xfe x https://claude.ai/magic-link#u\xe4\xb8 y\r\n')
    add('text unknown charset', head + b'Content-Type: text/plain; charset=x-nosuch\r\n\r\nhttps://claude.ai/magic-link#v\xc2\xa0x\r\n')
    add('text gbk', head + b'Content-Type: text/plain; charset=gbk\r\n\r\n\xb5\xc7\xc2\xbc https://claude.ai/magic-link#w\xa1\xa1\xd6\xd0 https://claude.ai/magic-link#x \xd6\xd0\r\n')
    add('html gbk', head + b'Content-Type: text/html; charset=gb2312\r\n\r\n<p>\xb5\xc7\xc2\xbc</p><a href="https://claude.ai/magic-link#y">\xd6\xd0</a>\r\n')
    add('html utf-16', head + b'Content-Type: text/html; charset=utf-16\r\n\r\n' + '<a href="https://claude.ai/magic-link#z">x</a>'.encode('utf-16'))
    add('html qp', head + b'Content-Type: text/html; charset="utf-8"\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n<a href=3D"https://claude.ai/magic-link#tok=3Dqp" style=3D"color:#fff">Sign=\r\n in</a>\r\n')
    add('html base64', head + b'Content-Type: text/html\r\nContent-Transfer-Encoding: base64\r\n\r\nPGEgaHJlZj0iaHR0cHM6Ly9jbGF1ZGUuYWkvbWFn\r\naWMtbGluayN0b2stYjY0Ij54PC9hPg==\r\n')
    add('html entity link', head + b'Content-Type: text/html\r\n\r\n<a href="https://claude.ai/magic-link?a=1&amp;b=2#tok">x</a><a href="&#104;ttps://claude.ai/magic-link#ent">y</a>\r\n')
    add('html spaces around', head + b'Content-Type: text/html\r\n\r\n<a href="  https://claude.ai/magic-link#sp \r\n">x</a>\r\n')
    add('html in rfc822', head + b'Content-Type: message/rfc822\r\n\r\nContent-Type: text/html\r\n\r\n<a href="https://claude.ai/magic-link#inner">x</a>\r\n')
    add('html in script', head + b'Content-Type: text/html\r\n\r\n<script>var a = \'<a href="https://claude.ai/magic-link#js">\';</script><a href="https://claude.ai/magic-link#real">x</a>\r\n')
    add('html in comment', head + b'Content-Type: text/html\r\n\r\n<!-- <a href="https://claude.ai/magic-link#hidden"> --><a href="https://claude.ai/magic-link#shown">x</a>\r\n')
    add('html huge charref', head + b'Content-Type: text/html\r\n\r\n<a href="https://claude.ai/magic-link#before">x</a>&#' + b'9' * 4301 + b';<a href="https://claude.ai/magic-link#after">y</a>\r\n')
    add('text/HTML upper', head + b'Content-Type: TEXT/HTML; CHARSET=UTF-8\r\n\r\n<a href="https://claude.ai/magic-link#up">x</a>\r\n')
    add('text/xml skipped', head + b'Content-Type: text/xml\r\n\r\n<a href="https://claude.ai/magic-link#xml">x</a> https://claude.ai/magic-link#xml2\r\n')
    add('charset nul', head + b'Content-Type: text/html; charset="a\x00b"\r\n\r\n<a href="https://claude.ai/magic-link#nul">x</a>\r\n')
    add('param number too long', head + b'Content-Type: text/html; x*' + b'9' * 4301 + b'*=y\r\n\r\n<a href="https://claude.ai/magic-link#big">x</a>\r\n')
    add('charset rfc2231', head + b"Content-Type: text/plain; charset*=utf-8''iso-8859-1\r\n\r\nhttps://claude.ai/magic-link#r\xa0x\r\n")
    add('multipart no boundary', head + b'Content-Type: multipart/alternative\r\n\r\n--b\r\nContent-Type: text/html\r\n\r\n<a href="https://claude.ai/magic-link#nb">x</a>\r\n--b--\r\n')
    add('message-id missing', head + b'\r\nx\r\n')
    add('message-id folded', head + b'Message-ID:\r\n <abc@mail.anthropic.com>\r\n\r\nx\r\n')
    add('message-id 8bit', head + b'Message-ID: <ab\xe9c@mail.anthropic.com>\r\n\r\nx\r\n')
    add('empty', b'')
    add('headers only', head)
    return out


def judge_case(name, raw, runs):
    out = {'name': name, 'raw': b2s(raw)}
    try:
        msg = mail_parse(raw)
    except (RecursionError, ValueError) as e:
        out['error'] = cls(e)
        return out
    ar = mail_ar(msg)
    out['ar'] = None if ar is None else [list(x) for x in ar]
    signed = mail_signed(msg, RULE)
    out['signed'] = None if signed is None else sorted(signed)
    out['subject'] = mail_text(msg.get('Subject'))
    for key, hdr in (('from', 'From'), ('to', 'To'), ('reply_to', 'Reply-To')):
        try:
            out[key] = mail_addrs(msg, hdr)
        except RecursionError as e:
            out[key] = cls(e)
    try:
        out['alias'] = mail_from_alias(msg)
    except RecursionError:
        out['alias'] = ''
    out['judge'] = []
    for got, since, acct, fwd in runs:
        try:
            why = mail_judge(RULE, msg, got, acct, since, fwd)
            any_rule, any_why = mail_judge_any([RULE], msg, got, acct, since, fwd)
            none_rule, none_why = mail_judge_any([], msg, got, acct, since, fwd)
            assert any_rule is RULE and none_rule is None and none_why == 'subject'
        except RecursionError as e:
            why = any_why = '!' + cls(e)
        out['judge'].append({'got': got, 'since': since, 'acct': acct, 'forwarder': fwd, 'why': why, 'any': any_why})
    try:
        out['links'] = mail_links(msg)
        out['extract'] = mail_extract(RULE, msg)
    except ValueError as e:
        out['links'] = '!' + cls(e)
    if isinstance(out['links'], list):
        alt = links_like_rust(msg)   # 正文的字符集 Python 认识、Rust 不认识时，Rust 照 UTF-8 解：链接可能不一样
        if alt != out['links']:
            out['links_unknown'] = alt
            out['extract_unknown'] = next((u for u in alt if RULE.link_ok(u)), None)
    out['digest'] = mail_digest(msg, {'folder': '[Gmail]/All Mail', 'uid': 7, 'got': 1794708000})
    return out


def links_like_rust(msg):
    """mail_links，只是 Python 认识、Rust 不认识的字符集照「不认识」处理（按 UTF-8 解）"""
    links = []
    for part in msg.walk():
        if part.is_multipart() or part.get_content_disposition() == 'attachment':
            continue
        ct = part.get_content_type()
        if ct not in ('text/html', 'text/plain'):
            continue
        raw = part.get_payload(decode=True) or b''
        cs = part.get_content_charset() or 'utf-8'
        text = raw.decode(cs if codec_of(cs) in MINE.values() else 'utf-8', 'replace')
        if ct == 'text/html':
            links.extend(html_hrefs(text))
        else:
            links.extend(re.findall(r'https://[^\s<>\x22\x27\\]+', text))
    return [l.strip() for l in links]


OSERR = [
    ('Connection refused', 'ConnectionRefusedError'), ('timed out', 'TimeoutError'), ('', 'error'), ('   ', 'abort'), ("b'[AUTHENTICATIONFAILED] Invalid credentials (Failure)'", 'error'),
    ('b"it\'s quoted"', 'error'), ("b'it\\'s \"both\"'", 'error'), ("b''", 'error'), ('b""', 'error'), ("b'unterminated", 'error'), ("xb'not at start'", 'error'), ("b'a' and b'b'", 'error'), ("B'upper'", 'error'),
    ("b'with\\nescape\\x1b[2K'", 'error'), ("b'line one\nline two'", 'error'), ("b'trailing newline'\n", 'error'), ("b'x'\n\n", 'error'), ('a\x1b]52;c;ZXZpbA==\x07b\x9d', 'Exception'),
    ('\x1b[2K\x1b[1A\u2713 \u901a\u8fc7\u6838\u5bf9\x9b31m\u202e\x07', 'Exception'), ('zero\u200bwidth\u200fmarks\u202a\u202e\u2060\u2069\ufeff', 'Exception'), ('keep\u2010\u2011\u2028\u2029\u00a0\u3000', 'Exception'),
    ('  many   spaces\t\tand\ttabs\n\nnewlines  ', 'OSError'), ('x' * 159, 'OSError'), ('x' * 160, 'OSError'), ('x' * 161, 'OSError'), ('\u4e2d' * 200, 'OSError'), ('a ' * 100, 'OSError'), (' ' * 200 + 'x', 'OSError'),
    ('\x00\x01\x1f\x7f\x80\x9f', 'OSError'), ('\u200b', 'OSError'), ('command: LOGIN => socket error: EOF', 'abort'), ("UID command error: BAD [b'Could not parse command']", 'error'),
    ("unexpected response: b'garbage \\xff'", 'abort'), ('NoneType', 'NoneType'), ('bytes', 'bytes'), ("(b'1 (UID 5 {3}', b'abc')", 'tuple'), ('NO', 'str'), ('[Errno 32] Broken pipe', 'BrokenPipeError'),
    ('nodename nor servname provided, or not known', 'gaierror'), ('The read operation timed out', 'TimeoutError'), ("b'\\x1b[31m'", 'error'), ("b'caf\\xc3\\xa9'", 'error'), ("b'' ", 'error'), ("b'a'b'", 'error'),
    ('b\'"\'', 'error'), ('b"\'"', 'error'), ("b'\\\\'", 'error'), ('certificate verify failed: unable to get local issuer certificate', 'SSLCertVerificationError'),
]


def oserr_case(text, klass):
    """mail_oserr(e)：e 是 str(e) 等于 text、类名是 klass 的异常"""
    e = type(klass, (Exception,), {'__str__': lambda self: text})()
    return {'text': text, 'class': klass, 'clean': mail_clean(text), 'oserr': mail_oserr(e)}


# --- 路：系统代理、通配符 --------------------------------------------------------------------------------------------
SCUTILS = [
    '', '<dictionary> {\n}\n', 'No proxy settings\n',
    ('<dictionary> {\n  ExceptionsList : <array> {\n    0 : *.local\n    1 : 169.254/16\n  }\n  FTPPassive : 1\n  HTTPEnable : 1\n  HTTPPort : 7890\n  HTTPProxy : 127.0.0.1\n'
     '  HTTPSEnable : 1\n  HTTPSPort : 7891\n  HTTPSProxy : 127.0.0.1\n  SOCKSEnable : 1\n  SOCKSPort : 7892\n  SOCKSProxy : 127.0.0.1\n'
     '  __SCOPED__ : <dictionary> {\n    en0 : <dictionary> {\n      SOCKSPort : 1\n    }\n  }\n}\n'),
    '  SOCKSEnable : 1\n  SOCKSEnable : 0\n  SOCKSProxy :   spaced host  \n  SOCKSPort :\n', '  A : 1\r\n  B : 2\r  C : 3\x0b  D : 4\x1c  E : 5   F : 6\n',
    '  ExceptionsList : <array> {\n    0 : a\n    10 : b\n     2 : too deep\n    x : not a number\n  After : 1\n    3 : not in the list any more\n',
    '  ExceptionsList : x\n    0 : first\n  ExceptionsList : y\n    1 : second\n  K : v\n', ' K : one space\n   K : three spaces\n\tK : tab\n  K: no space\n  K :no space\n  K  : two\n  K : \n  K : a : b\n',
    '  中文 : v\n  kéy : v\n  k-y : v\n  k.y : v\n  k_y1 : v\n  ١ : v\n', '  ExceptionsList : <array> {\n    ١ : arabic digit index\n    0 :  padded  \n    1 : \n  }\n',
    '  HTTPProxy : 127.0.0.1\n  HTTPProxy : 192.0.2.10\n', '  ProxyAutoConfigEnable : 1\n  ProxyAutoConfigURLString : http://example.com/proxy.pac\n',
]
FN_PATS = [
    '*.gmail.com', '.gmail.com', 'imap.gmail.com', 'IMAP.GMAIL.COM', '*.google.com', '*', '', '?', '*.com', 'imap.*', 'imap.gmail.co?', 'imap.gmail.com?', '?map.gmail.com', 'imap[.]gmail[.]com', 'imap.gmail.co[a-z]',
    'imap.gmail.co[!m]', 'imap.gmail.co[!a-l]', 'imap.gmail.co[m-a]', 'imap.gmail.co[]m]', 'imap.gmail.co[!]]', 'imap.gmail.co[', 'imap.gmail.co[m', 'imap.gmail.co[]', 'imap.gmail.co[!]', 'imap.gmail.co[^m]',
    'imap.gmail.co[m^]', 'imap.gmail.co[\\m]', 'imap.gmail.co[-m]', 'imap.gmail.co[m-]', 'imap.gmail.co[a-c-m]', 'imap.gmail.co[a-cl-n]', 'imap.gmail.co[a--]', 'imap.gmail.co[--m]', 'imap.gmail.co[m&&m]',
    'imap.gmail.co[m||x]', 'imap.gmail.co[~~m]', 'imap.gmail.co[[m]', 'imap.gmail.co[[:alpha:]]', 'i*p.g*l.c*m', 'i**p***', '*i*m*a*p*', '*imap.gmail.com*', 'imap.gmail.com*', '*imap.gmail.comx', '*x*', '*.', '.*',
    'imap\\.gmail\\.com', 'imap.gmail.com\n', '169.254/16', '*.local', 'localhost', '127.0.0.1', '10.0.0.0/8', '<local>', 'imap.gmail.c[n-l]m', 'imap.gmail.c[o-o]m', 'imap.gmail.c[a-co-o]m', 'imap.gmail.c[!!]m',
    'imap.gmail.c[!-]m', 'imap.gmail.c[-!]m', 'imap.gmail.c[!--o]m', 'imap.gmail.c[n-o-p]m', 'imap.gmail.c[o-n-p]m', 'imap.gmail.c[a-b-c-d-o]m', '[i][m][a][p].gmail.com', '[a-z]*[!a-z]*', '*[.]*[.]*', '[!.]*',
]


def fnmatch_cases(rng, extra):
    pairs = [('imap.gmail.com', p) for p in FN_PATS] + [(n, p) for n in ('', 'a', 'ab', 'a-b', 'a]b', 'a\\b', 'a^b', 'a!b', 'A', 'é', '\n', '-', ']', '[', '&', '~', '|')
                                                        for p in ('?', '*', '[a]', '[!a]', '[a-z]', '[]]', '[!]]', '[-]', '[\\]', '[\\\\]', '[^]', '[!^]', '[a-]', '[-a]', '[&]', '[~]', '[|]', '[[]',
                                                                  'a[]]b', 'a[-]b', 'a[\\]b', 'a[!-]b', 'a[^]b', 'a[!]b', 'a[]b', '[a-b-c]', '[z-a]', '[!z-a]', '[a-a]', 'é', '[à-ÿ]')]
    alpha = 'aimp.gl*?[]!-^\\&~|c'
    for _ in range(400 * extra):
        pairs.append((rng.choice(('imap.gmail.com', 'a', 'ab-c', 'a.b', 'm]', 'x!y^', 'i\\p', 'a&b|c~')), ''.join(pick(rng, alpha, 0, 8))))
    out = []
    for name, pat in dict.fromkeys(pairs):
        out.append({'name': name, 'pat': pat, 'ok': fnmatch.fnmatchcase(name, pat)})
    return out


# --- IMAP：时间、搜索条件、FETCH 的回应 --------------------------------------------------------------------------------
EPOCHS = [
    '15-Nov-2026 10:00:00 +0800', '15-Nov-2026 02:00:00 +0000', ' 5-Nov-2026 02:00:00 -0130', '05-Nov-2026 03:30:00 +0000', '15-Foo-2026 02:00:00 +0000', '2026-11-15 02:00:00', '', '15-nov-2026 02:00:00 +0000',
    '15-NOV-2026 02:00:00 +0000', '15-nOV-2026 02:00:00 +0000', '5-Nov-2026 02:00:00 +0000', '  5-Nov-2026 02:00:00 +0000', '\t5-Nov-2026 02:00:00 +0000', ' 5-Nov-2026 02:00:00 +0000', '005-Nov-2026 02:00:00 +0000',
    '15-Nov-26 02:00:00 +0000', '15-Nov-02026 02:00:00 +0000', '15-Nov-2026  02:00:00 +0000', '15-Nov-2026 2:00:00 +0000', '15-Nov-2026 02:00:00 0000', '15-Nov-2026 02:00:00 +000', '15-Nov-2026 02:00:00 +00000',
    '15-Nov-2026 02:00:00 +0000\n', '15-Nov-2026 02:00:00 +0000\n\n', '15-Nov-2026 02:00:00 +0000 ', '15-Nov-2026 02:00:00 UTC', '15-Nov-2026 02:00:00 +9999', '15-Nov-2026 02:00:00 -9999', '00-Jan-1970 00:00:00 +0000',
    '99-Dec-9999 99:99:99 +0000', '01-Jan-0001 00:00:00 +0000', '01-Jan-0000 00:00:00 +0000', '31-Feb-2026 25:61:61 +0000', '29-Feb-2028 00:00:00 +0530', '01-Jan-1970 00:00:00 +0000', '31-Dec-1969 23:59:59 +0000',
    '١٥-Nov-2026 02:00:00 +0000', '15-Nov-٢٠٢٦ 02:00:00 +0000', '15-Növ-2026 02:00:00 +0000', '15-Sept-2026 02:00:00 +0000', '15-May-2026 02:00:00 +0000', '15-may-2026 02:00:00 -0000',
]
SINCES = [0, 1, 86399, 86400, 86401, 1794708000, 1794708000.9, 1794787199, 1794787200, 951782400, 951868800, 946684800, 978307199.5, 4102444800, 1740787200, 1709251200, 1.5e9, 2e9, 1794700800 + 86400 * 46]
FETCHES = [
    None, [], [None], ['1 (UID 5 INTERNALDATE "15-Nov-2026 02:00:00 +0000")'], [['1 FETCH (UID 5 INTERNALDATE "15-Nov-2026 02:00:00 +0000" BODY[HEADER.FIELDS (FROM)] {9}', 'From: x\r\n'], ')'],
    [['1 FETCH (UID 5 INTERNALDATE "15-Nov-2026 02:00:00 +0000" BODY[HEADER.FIELDS (FROM)] {9}', 'From: x\r\n'], ')',
     ['2 FETCH (INTERNALDATE "15-Nov-2026 02:00:00 +0000" BODY[HEADER.FIELDS (FROM)] {9}', 'From: y\r\n'], ' UID 7)', ['3 FETCH (UID 8 BODY[] {1}', 'z'], ')'],
    [['1 (UID 5 INTERNALDATE "15-Nov-2026 02:00:00 +0000" BODY[] {3}', 'abc'], ')', '2 (UID 6 FLAGS (\\Seen))', ' UID 9'],
    [['1 (INTERNALDATE "15-Nov-2026 02:00:00 +0000" BODY[] {3}', 'abc'], 'UID 6)'], [['1 (INTERNALDATE "15-Nov-2026 02:00:00 +0000" BODY[] {3}', 'abc'], '12 (UID 6)'],
    [['1 (INTERNALDATE "15-Nov-2026 02:00:00 +0000" BODY[] {3}', 'abc'], '12 UID 6)'], [['1 (INTERNALDATE "15-Nov-2026 02:00:00 +0000" BODY[] {3}', 'abc'], '(UID 6)'],
    [['1 (uid 5 INTERNALDATE "15-Nov-2026 02:00:00 +0000" {1}', 'a'], ')'], [['1 (XUID 5 INTERNALDATE "15-Nov-2026 02:00:00 +0000" {1}', 'a'], ')'], [['1 (_UID 5 INTERNALDATE "15-Nov-2026 02:00:00 +0000" {1}', 'a'], ')'],
    [['1 (½UID 5 INTERNALDATE "15-Nov-2026 02:00:00 +0000" {1}', 'a'], ')'], [['1 (éUID 5 INTERNALDATE "15-Nov-2026 02:00:00 +0000" {1}', 'a'], ')'], [['1 ( UID 5 INTERNALDATE "15-Nov-2026 02:00:00 +0000" {1}', 'a'], ')'],
    [['1 (UID x UID 6 INTERNALDATE "15-Nov-2026 02:00:00 +0000" {1}', 'a'], ')'], [['1 (UID 5 UID 6 INTERNALDATE "15-Nov-2026 02:00:00 +0000" {1}', 'a'], ')'], [['1 (UID  5 INTERNALDATE "15-Nov-2026 02:00:00 +0000" {1}', 'a'], ')'],
    [['1 (UID 5 XINTERNALDATE "15-Nov-2026 02:00:00 +0000" {1}', 'a'], ')'], [['1 (UID 5 INTERNALDATE "" INTERNALDATE "15-Nov-2026 02:00:00 +0000" {1}', 'a'], ')'], [['1 (UID 5 INTERNALDATE "15-Nov-2026 02:00:00 +0000 {1}', 'a'], ')'],
    [['1 (UID 5 INTERNALDATE "bad" {1}', 'a'], ')'], [['1 (UID 5 INTERNALDATE "bad" INTERNALDATE "15-Nov-2026 02:00:00 +0000" {1}', 'a'], ')'], [['1 (UID 5 internaldate "15-Nov-2026 02:00:00 +0000" {1}', 'a'], ')'],
    [['1 (UID 5 INTERNALDATE "01-Jan-0000 00:00:00 +0000" {1}', 'a'], ')'], [['1 (UID ' + '9' * 4301 + ' INTERNALDATE "15-Nov-2026 02:00:00 +0000" {1}', 'a'], ')'], [['1 (UID 005 INTERNALDATE "15-Nov-2026 02:00:00 +0000" {0}', ''], ')'],
    [['1 (UID 5 INTERNALDATE "15-Nov-2026 02:00:00 +0000" {1}', 'a'], ')', ['2 (UID 5 INTERNALDATE "16-Nov-2026 02:00:00 +0000" {1}', 'b'], ')'],
    [['1 (UID 9 INTERNALDATE "15-Nov-2026 02:00:00 +0000" {1}', 'a'], ')', ['2 (UID 3 INTERNALDATE "16-Nov-2026 02:00:00 +0000" {1}', 'b'], ')'],
    [')', ['1 (UID 5 INTERNALDATE "15-Nov-2026 02:00:00 +0000" {1}', 'a'], None, ')', '', ' ', 'UID 77'], [['1 (UID 5 INTERNALDATE "15-Nov-2026 02:00:00 +0000" {1}', 'a'], '1 (not a new one', '1x (UID 8)'],
    [['\xff\xfe (UID 5 INTERNALDATE "15-Nov-2026 02:00:00 +0000" {1}', '\xff'], '\xff)'],
]
LIST_ROWS = [
    '(\\HasNoChildren) "/" "INBOX"', '(\\All \\HasNoChildren) "/" "[Gmail]/All Mail"', '(\\HasNoChildren \\Junk) "/" "[Gmail]/Spam"', '(\\HasChildren \\Noselect) "/" "[Gmail]"', '() "/" INBOX', '() NIL INBOX',
    '() nil INBOX', '() "/" ""', '() "/" "a\\"b"', '() "/" "a\\\\"', '() "/" "a\\"', '() "/" "a"b"', '() "." "a b"', '() "/" a b', '() "/" a(b', '() "/" a)b', '() "/" a"b', '(\\All) "/" &UXZO1mWHTvZZOQ-',
    '(\\ALL \\junk) "/" x', '(a)b) "/" x', '((nested)) "/" x', '(\\All)  "/" x', '(\\All) "/"  x', '(\\All) "/" x ', ' (\\All) "/" x', '(\\All) "/" x\n', '(\\All) "/" x\n\n', '(\\All) "\\"" x', '(\\All) "\\\\" x',
    '(\\All) "//" x', '(\\All) / x', '(\\All) "" x', '(\\All) "/" x y', '(\\All) "/" xéy', '(\\All) "/" "xéy"', '(\\All) "/" "a\nb"', '(\\All) "/" "a\\\nb"', '(\\All\n) "/" x', '\\All "/" x', '',
    '(\\All) "/"', '(\\All) "/" {5}', '(\\All) "/" x\x1cy', '(\\All) "/" x\x0by', '(\\All \\Junk \\Trash) "/" "[Gmail]/&V4NXPpCuTvY-"',
]


def fetch_case(data):
    def item(x):
        if x is None or isinstance(x, str):
            return None if x is None else x.encode('latin-1')
        return (x[0].encode('latin-1'), x[1].encode('latin-1'))
    arg = None if data is None else [item(x) for x in data]
    out = {'data': data}
    try:
        got = mail_fetch_parse(arg)
        out['out'] = [[u, t, b2s(lit)] for u, (t, lit) in sorted(got.items())]
    except ValueError as e:
        out['error'] = cls(e)
    return out


def epoch_case(s):
    try:
        return {'in': s, 'out': mail_epoch(s)}
    except ValueError as e:
        return {'in': s, 'error': cls(e)}


def list_row_case(s):
    m = MAIL_LIST_RE.match(s)
    return {'in': s, 'out': None if not m else [m.group(1), m.group(2)]}


# --- IMAP：照剧本回答的假服务器（明文，不经 TLS），MailBox 的各个方法走一遍 ----------------------------------------------------
class LikeTls:
    """明文的套接字装成 ssl 模块的套接字的样子：超时的说法照 TLS 的（原来的连接总是经 TLS 的）"""

    def __init__(self, s):
        self.s = s

    def recv(self, n):
        try:
            return self.s.recv(n)
        except TimeoutError:
            raise TimeoutError('The read operation timed out') from None

    def sendall(self, data):
        try:
            return self.s.sendall(data)
        except TimeoutError:
            raise TimeoutError('The write operation timed out') from None

    def __getattr__(self, name):
        return getattr(self.s, name)


class Script(threading.Thread):
    """一个剧本：conns 是先后各次连接怎么演。每次连接：greet（连上就发的）、steps（每收到一条命令回一段，None 是不回）、
    end（剧本演完以后：close 关掉、hang 接着读不回、rst 马上重置、line-rst 再读一条命令才重置）。回答里的 {TAG} 换成这条命令的
    tag。收到的命令（tag 换成 TAG）记下来"""

    def __init__(self, conns):
        threading.Thread.__init__(self, daemon=True)
        self.conns, self.sent = conns, []
        self.ls = socket.socket()
        self.ls.bind(('127.0.0.1', 0))
        self.ls.listen(4)
        self.port = self.ls.getsockname()[1]

    def run(self):
        for conn in self.conns:
            try:
                s, _ = self.ls.accept()
            except OSError:
                return
            try:
                self.play(s, conn)
            except OSError:
                pass

    def play(self, s, conn):
        import struct
        f = s.makefile('rb')
        if conn.get('greet') is not None:
            s.sendall(conn['greet'].encode('latin-1'))
        for reply in conn.get('steps', []):
            line = f.readline()
            if not line:
                break
            tag = line.split(b' ', 1)[0]
            self.sent.append(b2s(b'TAG' + line[len(tag):]))
            if reply is not None:
                s.sendall(reply.encode('latin-1').replace(b'{TAG}', tag))
        end = conn.get('end', 'close')
        if end == 'hang':
            while True:
                line = f.readline()
                if not line:
                    break
                tag = line.split(b' ', 1)[0]
                self.sent.append(b2s(b'TAG' + line[len(tag):]))
        elif end in ('rst', 'line-rst'):
            if end == 'line-rst':
                line = f.readline()
                if line:
                    self.sent.append(b2s(b'TAG' + line[len(line.split(b' ', 1)[0]):]))
            s.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack('ii', 1, 0))
        f.close()
        s.close()


CAPA = '* CAPABILITY IMAP4rev1 UNSELECT\r\n{TAG} OK done\r\n'
HELLO = {'greet': '* OK Gimap ready\r\n'}
LOGIN_OK = '{TAG} OK authenticated\r\n'
LIST3 = ('* LIST (\\HasNoChildren) "/" "INBOX"\r\n* LIST (\\All \\HasNoChildren) "/" "[Gmail]/All Mail"\r\n* LIST (\\HasNoChildren \\Junk) "/" "[Gmail]/Spam"\r\n{TAG} OK Success\r\n')
EXAM = ('* FLAGS (\\Seen)\r\n* OK [PERMANENTFLAGS ()] Flags permitted.\r\n* OK [UIDVALIDITY 11] UIDs valid.\r\n* 3 EXISTS\r\n* 0 RECENT\r\n* OK [UIDNEXT 4] Predicted next UID.\r\n'
        '{TAG} OK [READ-ONLY] selected. (Success)\r\n')
DATE = '"15-Nov-2026 02:00:00 +0000"'
FETCH1 = '* 1 FETCH (UID 1 INTERNALDATE %s BODY[HEADER.FIELDS (FROM)] {9}\r\nFrom: x\r\n)\r\n{TAG} OK Success\r\n' % DATE
BYE = '* BYE LOGOUT Requested\r\n{TAG} OK 73 good day\r\n'


def session(*steps, greet=HELLO['greet'], end='close'):
    """一次连接：欢迎、CAPABILITY、LOGIN 照常，后面接这些回答"""
    return {'greet': greet, 'steps': [CAPA, LOGIN_OK] + list(steps), 'end': end}


def imap_scripts():
    """(名字, 各次连接的剧本, 这边依次做什么)"""
    full = ['open', 'folders', ['examine', '"[Gmail]/All Mail"'], 'search', ['headers', [1, 2]], ['range', 1, 3], ['message', 1], 'logout']
    out = [
        ('full session', [session(LIST3, EXAM, '* SEARCH 1 2 3\r\n{TAG} OK SEARCH completed\r\n', FETCH1, FETCH1, FETCH1.replace('BODY[HEADER.FIELDS (FROM)]', 'BODY[]<0>'), BYE)], full),
        ('ops on a box that was never opened', [], ['folders', ['examine', 'INBOX'], 'search', ['headers', [1]], ['headers', []], ['message', 1], 'logout', 'shutdown']),
    ]
    # 欢迎的那一行
    for name, greet in (('preauth', '* PREAUTH ready\r\n'), ('bye', '* BYE Temporary failure\r\n'), ('garbage', 'hello\r\n'), ('empty line', '\r\n'), ('lf only', '* OK x\n'), ('no newline', '* OK x'),
                        ('continuation', '+ go ahead\r\n'), ('lowercase', '* ok ready\r\n'), ('ok no space', '* OK[x]\r\n'), ('no', '* NO go away\r\n'), ('bad', '* BAD go away\r\n'),
                        ('quotes', '* BYE it\'s "over" \\ \x1b[2K\xff\r\n'), ('status form', '* 5 OK x\r\n'), ('tagged', 'A1 OK x\r\n'), ('ok empty', '* OK\r\n'), ('ok with code', '* OK [ALERT] hi [x]\r\n'),
                        ('long bye', '* BYE ' + 'x' * 300 + '\r\n'), ('cr in line', '* OK a\rb\r\n'), ('nul', '* OK a\x00b\r\n')):
        out.append(('greeting ' + name, [{'greet': greet, 'steps': [CAPA, LOGIN_OK, BYE]}], ['open', 'logout']))
    out.append(('greeting eof', [{'greet': None, 'steps': []}], ['open', 'folders']))
    out.append(('greeting stall', [{'greet': None, 'steps': [], 'end': 'hang'}], ['open']))
    out.append(('greeting rst', [{'greet': None, 'steps': [], 'end': 'rst'}], ['open']))
    out.append(('capability stall', [{'greet': HELLO['greet'], 'steps': [], 'end': 'hang'}], ['open']))
    out.append(('greeting too long', [{'greet': '* OK ' + 'x' * 1000001 + '\r\n', 'steps': [CAPA]}], ['open']))
    # CAPABILITY
    for name, capa in (('none', '{TAG} OK done\r\n'), ('no', '{TAG} NO nope\r\n'), ('no with imap4', '{TAG} NO try IMAP4rev1 later\r\n'), ('bad', '{TAG} BAD nope\r\n'), ('not imap4', '* CAPABILITY POP3\r\n{TAG} OK done\r\n'),
                       ('imap4 only', '* CAPABILITY IMAP4\r\n{TAG} OK done\r\n'), ('lowercase', '* CAPABILITY imap4rev1\r\n{TAG} OK done\r\n'), ('8bit', '* CAPABILITY IMAP4rev1 \xff\r\n{TAG} OK done\r\n'),
                       ('literal', '* CAPABILITY {8}\r\nIMAP4rev1\r\n{TAG} OK done\r\n'), ('literal then text', '* CAPABILITY {3}\r\nabc IMAP4rev1\r\n{TAG} OK done\r\n'),
                       ('twice', '* CAPABILITY POP3\r\n* CAPABILITY IMAP4rev1\r\n{TAG} OK done\r\n'), ('wrong tag', '* CAPABILITY IMAP4rev1\r\nZZZZ9 OK done\r\n'), ('bye', '* BYE shutting down\r\n{TAG} OK done\r\n'),
                       ('empty', '* CAPABILITY\r\n{TAG} OK done\r\n'), ('tagged odd type', '* CAPABILITY IMAP4rev1\r\n{TAG} PREAUTH x\r\n'), ('tagged no data', '* CAPABILITY IMAP4rev1\r\n{TAG} OK\r\n'),
                       ('tabs', '* CAPABILITY\tIMAP4rev1\r\n{TAG} OK done\r\n'), ('tab inside', '* CAPABILITY X\tIMAP4rev1\x0bY\r\n{TAG} OK done\r\n'), ('eof', None)):
        out.append(('capability ' + name, [{'greet': HELLO['greet'], 'steps': [capa, LOGIN_OK, BYE] if capa else [None]}], ['open', 'logout']))
    # LOGIN
    for name, reply in (('auth failed', '{TAG} NO [AUTHENTICATIONFAILED] Invalid credentials (Failure)\r\n'), ('imap disabled', '{TAG} NO [ALERT] Your account is not enabled for IMAP use. (Failure)\r\n'),
                        ('imap disabled 2', '{TAG} NO IMAP access is disabled for your domain\r\n'), ('imap disabled 3', '{TAG} NO imap IS DISABLED\r\n'),
                        ('app password', '{TAG} NO [ALERT] Application-specific password required: https://support.google.com/accounts/answer/185833 (Failure)\r\n'),
                        ('unavailable', '{TAG} NO [UNAVAILABLE] Temporary System Error\r\n'), ('no empty', '{TAG} NO \r\n'), ('no quotes', '{TAG} NO it\'s "bad" \\ \x1b[2K\xff\r\n'), ('no long', '{TAG} NO ' + 'x' * 300 + '\r\n'),
                        ('no single quote', "{TAG} NO it's bad\r\n"), ('bad', '{TAG} BAD Invalid credentials\r\n'), ('bye first', '* BYE Too many connections\r\n{TAG} NO x\r\n'), ('odd type', '{TAG} FOO bar\r\n'),
                        ('read-only code', '* OK [READ-ONLY] odd\r\n{TAG} OK authenticated\r\n'), ('capability inside', '* CAPABILITY IMAP4rev1 X\r\n{TAG} OK [CAPABILITY IMAP4rev1] ok\r\n'),
                        ('invalid credentials lower', '{TAG} NO invalid CREDENTIALS\r\n'), ('unexpected', 'what\r\n'), ('continuation', '+ send more\r\n{TAG} OK authenticated\r\n'), ('eof', None)):
        steps = [CAPA, reply, LIST3, BYE] if reply else [CAPA]
        out.append(('login ' + name, [{'greet': HELLO['greet'], 'steps': steps}], ['open', 'folders', 'logout']))
    out.append(('login stall', [{'greet': HELLO['greet'], 'steps': [CAPA], 'end': 'hang'}], ['open', 'folders']))
    out.append(('login rst', [{'greet': HELLO['greet'], 'steps': [CAPA], 'end': 'line-rst'}], ['open']))
    # LIST
    for name, reply in (('no all', '* LIST (\\HasNoChildren) "/" "INBOX"\r\n* LIST (\\HasNoChildren \\Junk) "/" "[Gmail]/Spam"\r\n{TAG} OK Success\r\n'), ('only inbox', '* LIST () "/" INBOX\r\n{TAG} OK Success\r\n'),
                        ('none', '{TAG} OK Success\r\n'), ('no', '{TAG} NO [SERVERBUG] try again\r\n'), ('bad', '{TAG} BAD Could not parse command\r\n'), ('odd type', '{TAG} FOO x\r\n'),
                        ('odd type with rows', '* LIST (\\All) "/" x\r\n{TAG} FOO x\r\n'), ('upper flags', '* LIST (\\ALL) "/" a\r\n* LIST (\\JUNK \\All) "/" b\r\n{TAG} OK Success\r\n'),
                        ('two all', '* LIST (\\All) "/" first\r\n* LIST (\\All) "/" second\r\n{TAG} OK Success\r\n'), ('quoted names', '* LIST (\\All) "/" "a \\"q\\" b"\r\n* LIST (\\Junk) NIL "x\\\\y"\r\n{TAG} OK Success\r\n'),
                        ('literal name', '* LIST (\\All) "/" {7}\r\nAll Box\r\n* LIST (\\Junk) "/" J\r\n{TAG} OK Success\r\n'), ('8bit name', '* LIST (\\All) "/" "\xe9\xff"\r\n{TAG} OK Success\r\n'),
                        ('garbage rows', '* LIST\r\n* LIST x\r\n* LIST (\\All)\r\n* LSUB (\\All) "/" lsub\r\n* 3 EXISTS\r\n{TAG} OK Success\r\n'), ('bye', '* BYE going down\r\n'), ('eof mid', '* LIST (\\All) "/" a\r\n'),
                        ('unterminated', '* LIST (\\All) "/" a'), ('unexpected', 'garbage \xff\r\n'), ('wrong tag', 'ZZZZ1 OK x\r\n'), ('too long', '* LIST ' + 'x' * 1000001 + '\r\n{TAG} OK\r\n'),
                        ('ok with rows after', '{TAG} OK Success\r\n* LIST (\\All) "/" late\r\n'), ('same name', '* LIST (\\All \\Junk) "/" both\r\n{TAG} OK Success\r\n')):
        out.append(('list ' + name, [session(reply, EXAM, BYE)], ['open', 'folders', ['examine', 'INBOX'], 'logout']))
    out.append(('list stall', [session(end='hang')], ['open', 'folders', 'folders']))
    out.append(('list rst', [session(end='line-rst')], ['open', 'folders']))
    # EXAMINE
    for name, reply in (('no uidnext', '* OK [UIDVALIDITY 11] ok\r\n{TAG} OK [READ-ONLY] done\r\n'), ('nothing', '{TAG} OK done\r\n'), ('not numbers', '* OK [UIDVALIDITY x] ok\r\n* OK [UIDNEXT 4 5] ok\r\n{TAG} OK done\r\n'),
                        ('twice', '* OK [UIDVALIDITY 1] a\r\n* OK [UIDVALIDITY 2] b\r\n* OK [UIDNEXT 9] c\r\n* OK [UIDNEXT 10] d\r\n{TAG} OK done\r\n'), ('greedy bracket', '* OK [UIDNEXT 4] Predicted] next\r\n{TAG} OK done\r\n'),
                        ('no bracket end', '* OK [UIDNEXT 4 next\r\n{TAG} OK done\r\n'), ('code only', '* OK [UIDNEXT]\r\n* OK [UIDVALIDITY ]\r\n{TAG} OK done\r\n'), ('in no and bad', '* NO [UIDNEXT 7] x\r\n* BAD [UIDVALIDITY 8] y\r\n{TAG} OK done\r\n'),
                        ('in other type', '* FOO [UIDNEXT 7] x\r\n{TAG} OK [UIDVALIDITY 3] done\r\n'), ('leading zeros', '* OK [UIDVALIDITY 0011] ok\r\n* OK [UIDNEXT 0] ok\r\n{TAG} OK done\r\n'),
                        ('too many digits', '* OK [UIDVALIDITY ' + '1' * 4301 + '] ok\r\n{TAG} OK done\r\n'), ('no', '{TAG} NO [NONEXISTENT] Unknown Mailbox: INBOX (Failure)\r\n'), ('bad', '{TAG} BAD nope\r\n'),
                        ('read-write', '* OK [UIDVALIDITY 11] ok\r\n{TAG} OK [READ-WRITE] done\r\n'), ('lower code', '* OK [uidnext 4] ok\r\n{TAG} OK done\r\n'), ('spaces', '* OK  [UIDNEXT 4] ok\r\n* OK [UIDVALIDITY  5] ok\r\n{TAG} OK done\r\n'),
                        ('bye', '* BYE bye\r\n{TAG} OK done\r\n'), ('eof', None)):
        steps = [reply, '* SEARCH\r\n{TAG} OK done\r\n', BYE] if reply else []
        out.append(('examine ' + name, [session(*steps)], ['open', ['examine', 'INBOX'], 'search', 'logout']))
    out.append(('examine quoted and odd names', [session(EXAM, EXAM, EXAM, BYE)], ['open', ['examine', '"[Gmail]/All Mail"'], ['examine', 'a b'], ['examine', ''], 'logout']))
    out.append(('examine not ascii', [session(EXAM, BYE)], ['open', ['examine', 'café'], ['examine', 'INBOX'], 'logout']))
    # UID SEARCH
    for name, reply in (('hits', '* SEARCH 1 2 3\r\n{TAG} OK done\r\n'), ('empty', '* SEARCH\r\n{TAG} OK done\r\n'), ('none', '{TAG} OK done\r\n'), ('two lines', '* SEARCH 1 2\r\n* SEARCH 3\r\n{TAG} OK done\r\n'),
                        ('junk', '* SEARCH 1 x 2a -3 4.5 +6 07 \t8\x0b9\r\n{TAG} OK done\r\n'), ('literal', '* SEARCH {3}\r\n1 2 5\r\n{TAG} OK done\r\n'), ('no', '{TAG} NO [CANNOT] x\r\n'),
                        ('bad', '{TAG} BAD Could not parse command\r\n'), ('too many digits', '* SEARCH ' + '7' * 4301 + '\r\n{TAG} OK done\r\n'), ('esearch', '* ESEARCH (TAG "x") UID ALL 1:3\r\n{TAG} OK done\r\n'),
                        ('bye after', '{TAG} OK done\r\n* BYE later\r\n'), ('eof', None)):
        steps = [EXAM, reply, FETCH1, BYE] if reply else [EXAM]
        out.append(('search ' + name, [session(*steps)], ['open', ['examine', 'INBOX'], 'search', ['headers', [1]], 'logout']))
    out.append(('search before examine', [session(BYE)], ['open', 'search', 'logout']))
    out.append(('search after failed examine', [session('{TAG} NO nope\r\n', BYE)], ['open', ['examine', 'x'], 'search', 'logout']))
    # UID FETCH
    two = ('* 1 FETCH (UID 1 INTERNALDATE %s BODY[HEADER.FIELDS (FROM)] {9}\r\nFrom: x\r\n)\r\n* 2 FETCH (INTERNALDATE %s BODY[HEADER.FIELDS (FROM)] {9}\r\nFrom: y\r\n UID 2)\r\n{TAG} OK Success\r\n' % (DATE, DATE))
    for name, reply in (('two, uid after literal', two), ('no date', '* 1 FETCH (UID 1 BODY[] {1}\r\nz)\r\n{TAG} OK done\r\n'), ('no literal', '* 1 FETCH (UID 1 INTERNALDATE %s)\r\n{TAG} OK done\r\n' % DATE),
                        ('zero literal', '* 1 FETCH (UID 1 INTERNALDATE %s BODY[] {0}\r\n)\r\n{TAG} OK done\r\n' % DATE), ('two literals', '* 1 FETCH (UID 1 INTERNALDATE %s A {2}\r\nab B {3}\r\ncde)\r\n{TAG} OK done\r\n' % DATE),
                        ('literal with crlf inside', '* 1 FETCH (UID 1 INTERNALDATE %s BODY[] {12}\r\na\r\n{TAG} OK\r\n)\r\n{TAG} OK done\r\n' % DATE), ('short literal then eof', '* 1 FETCH (UID 1 INTERNALDATE %s BODY[] {50}\r\nshort' % DATE),
                        ('literal then eof', '* 1 FETCH (UID 1 INTERNALDATE %s BODY[] {5}\r\nshort' % DATE), ('not a literal', '* 1 FETCH (UID 1 INTERNALDATE %s BODY[] {x}\r\n{TAG} OK done\r\n' % DATE),
                        ('brace in middle', '* 1 FETCH (UID 1 {3} INTERNALDATE %s)\r\n{TAG} OK done\r\n' % DATE), ('none', '{TAG} OK done\r\n'), ('no', '{TAG} NO x\r\n'), ('bad', '{TAG} BAD Would mark as read\r\n'),
                        ('other uid', '* 1 FETCH (UID 9 INTERNALDATE %s BODY[] {1}\r\nz)\r\n{TAG} OK done\r\n' % DATE), ('expunge between', '* 5 EXPUNGE\r\n* 1 FETCH (UID 1 INTERNALDATE %s BODY[] {1}\r\nz)\r\n* 2 EXISTS\r\n{TAG} OK done\r\n' % DATE),
                        ('8bit literal', '* 1 FETCH (UID 1 INTERNALDATE %s BODY[] {4}\r\n\xff\x00\r\n)\r\n{TAG} OK done\r\n' % DATE), ('huge size', '* 1 FETCH (UID 1 INTERNALDATE %s BODY[] {99999999999999999999999}\r\nabc' % DATE),
                        ('too many digits', '* 1 FETCH (UID 1 BODY[] {' + '1' * 4301 + '}\r\nabc'), ('bad date year 0', '* 1 FETCH (UID 1 INTERNALDATE "01-Jan-0000 00:00:00 +0000" BODY[] {1}\r\nz)\r\n{TAG} OK done\r\n'),
                        ('status without data', '* 1 FETCH\r\n{TAG} OK done\r\n'), ('fetch not status form', '* FETCH (UID 1 INTERNALDATE %s BODY[] {1}\r\nz)\r\n{TAG} OK done\r\n' % DATE)):
        out.append(('fetch ' + name, [session(EXAM, reply, reply, BYE)], ['open', ['examine', 'INBOX'], ['headers', [1, 2]], ['message', 1], 'logout']))
    out.append(('fetch stall mid literal', [session(EXAM, '* 1 FETCH (UID 1 INTERNALDATE %s BODY[] {50}\r\nshort' % DATE, end='hang')], ['open', ['examine', 'INBOX'], ['message', 1], ['message', 1]]))
    # 断线重连、LOGOUT
    out.append(('reconnect after drop', [session(EXAM), session(EXAM, FETCH1, BYE)], ['open', ['examine', 'INBOX'], ['message', 1], 'open', ['examine', 'INBOX'], ['message', 1], 'logout']))
    out.append(('logout only ok', [session('{TAG} OK bye\r\n')], ['open', 'logout', 'folders']))
    out.append(('logout eof', [session()], ['open', 'logout', 'logout']))
    out.append(('logout stall', [session(end='hang')], ['open', 'logout']))
    out.append(('logout bad', [session('{TAG} BAD no\r\n')], ['open', 'logout']))
    out.append(('logout bye without tag', [session('* BYE see you\r\n')], ['open', 'logout']))
    out.append(('shutdown without logout', [session(end='hang')], ['open', 'shutdown', 'folders']))
    out.append(('password is quoted', [{'greet': HELLO['greet'], 'steps': [CAPA, LOGIN_OK, BYE]}], [['open_as', 'inbox@example.com', 'ab"cd\\ef gh(ij)kl{m}n*%'], 'logout']))
    out.append(('user not ascii', [{'greet': HELLO['greet'], 'steps': [CAPA, LOGIN_OK, BYE]}], [['open_as', 'café@example.com', 'abcdefghijklmnop'], 'logout']))
    return out


def imap_case(name, conns, ops, timeout=0.4):
    srv = Script(conns)
    srv.start()
    IMAP_PORT[0] = srv.port   # mail_dial 连到这个剧本
    env = {'box': None}

    def make(user='inbox@example.com', pw='abcdefghijklmnop'):
        return MailBox(user, pw, ('direct', '', 0), timeout)

    env['box'] = make()
    results = []
    for op in ops:
        kind = op if isinstance(op, str) else op[0]
        box = env['box']
        try:
            if kind == 'open':
                r = box.open()
            elif kind == 'open_as':
                box = env['box'] = make(op[1], op[2])
                r = box.open()
            elif kind == 'folders':
                r = [list(x) for x in box.folders()]
            elif kind == 'examine':
                r = list(box.examine(op[1]))
            elif kind == 'search':
                r = box.search(mail_crit([RULE], 1794708000))
            elif kind == 'headers':
                r = [[u, t, b2s(h)] for u, (t, h) in sorted(box.headers(op[1]).items())]
            elif kind == 'range':
                r = [[u, t, b2s(h)] for u, (t, h) in sorted(box.headers((op[1], op[2])).items())]
            elif kind == 'message':
                r = b2s(box.message(op[1]))
            elif kind == 'logout':
                r = box.close(True)
            elif kind == 'shutdown':
                r = box.close()
            else:
                raise KeyError(kind)
            res = {'ok': r}
        except MailFatal as e:
            res = {'fatal': [e.code, e.text]}
        except MailRetry as e:   # 说明里带着这次随机的 tag 的（服务器原样回了它）：换成 TAG
            res = {'retry': [e.code, re.sub(r'\b[A-P]{4}(\d+ )', r'TAG\1', e.text)]}
        except Exception as e:   # 原来没有人接的异常
            res = {'py': cls(e)}
        res['alive'] = env['box'].im is not None
        results.append(res)
    env['box'].close()   # 还开着的会话关掉（不算在剧本里）：假服务器才演得完
    srv.ls.close()
    srv.join(3)
    return {'name': name, 'conns': conns, 'ops': ops, 'timeout': timeout, 'results': results, 'sent': srv.sent}


def mail_dial(r, timeout=15):
    """参照实现里的 MailBox 要的：这里连到 Script（IMAP_PORT 是当前剧本的端口）"""
    return socket.create_connection(('127.0.0.1', IMAP_PORT[0]), 5)


def mail_tls(s, timeout=20):
    s.settimeout(timeout)
    return LikeTls(s)


IMAP_PORT = [0]


def imap_cases():
    return [imap_case(name, conns, ops) for name, conns, ops in imap_scripts()]


# --- 设置：config.json 的 mail 一节 ------------------------------------------------------------------------------------
class T:
    """参照实现里的 acct_id 要的「当前工具」：只用到 ids（第三方 API 账号的键 → id；这里没有）"""
    ids = {}


MAIL_DIR = None   # 参照实现里 mail_used 用的目录：hunt_case 每个用例换一个临时目录


def cfg_inputs():
    a, b = ident('alice@example.com'), ident('bob@example.com')   # mail.accounts 的键是账号目录名
    base = {'user': 'Inbox@Example.com', 'proxy': 'socks5://127.0.0.1:7897'}
    mails = [
        None, 'x', [], {}, {'user': 'inbox@example.com'}, base, dict(base, auto=False), dict(base, auto=0), dict(base, auto=None), dict(base, auto='false'), dict(base, proxy=''),
        dict(base, proxy=5), {'user': 'not an address'}, {'user': 5}, {'user': 'inbox@example.com\n'}, {'user': 'İnbox@example.com'},
        {'accounts': {a: {'user': 'Alice-Box@example.com'}}}, {'accounts': 'x'}, {'accounts': {a: 'x'}},
        dict(base, accounts={a: {'user': 'alice-box@example.com'}, b: {'forwarder': 'Relay.Example.com'}}),
        dict(base, auto=False, accounts={a: {'user': 'alice-box@example.com'}}),
        dict(base, auto=False, accounts={a: {'user': 'alice-box@example.com', 'auto': True, 'proxy': 'direct'}}),
        dict(base, accounts={a: {'user': 'bad address', 'forwarder': 'relay.example.com'}}),
        dict(base, accounts={a: {'forwarder': 'relay.example.com\n'}}), dict(base, accounts={a: {'forwarder': 'no-dot'}}), dict(base, accounts={a: {'forwarder': 5}}),
        dict(base, accounts={a: {'forwarder': ''}}), dict(base, accounts={a: {'user': 'x@example.com', 'forwarder': 'a.b', 'auto': False}}),
        {'accounts': {a: {'forwarder': 'relay.example.com'}}}, {'user': 'bad', 'accounts': {a: {'user': 'alice-box@example.com', 'proxy': ''}}},
        dict(base, accounts={a: {'user': 'inbox@example.com'}, b: {'user': 'INBOX@example.com'}, 'zz': {'user': 'other@example.com'}, 'yy': 5}),
        dict(base, accounts={ident('只有别名'): {'user': 'alias-box@example.com'}, ident(''): {'user': 'empty-key@example.com'}}),
    ]
    return [{'version': 2} if m is None else {'version': 2, 'mail': m} for m in mails]


CFG_KEYS = [None, 'alice@example.com', '主号(alice@example.com)', '主号（Alice@Example.com）', 'bob@example.com', '只有别名', '', 'nobody@example.com']


def cfg_case(c):
    """一份 config.json：各个账号（keys 的每一项是 [键, mail_cfg, mail_view]；键是 None 的是默认那一项）、引用到的地址"""
    return {'c': c, 'keys': [[k, mail_cfg(c, k), mail_view(c, k)] for k in CFG_KEYS], 'addrs': sorted(mail_all_addrs(c)), 'one': mail_one(c.get('mail')),
            'one_off': mail_one(c.get('mail'), False, 'direct')}


# --- cca mail open 的 --msg、--wait 和小时数 -----------------------------------------------------------------------------
HANDLES = ['INBOX:5:11', '"[Gmail]/All Mail":42:11', 'a:b:007:0', ':5:11', 'INBOX:5', 'INBOX', '', 'INBOX:x:11', 'INBOX:5:', 'INBOX::11', 'INBOX:-5:11', 'INBOX:5:+11', 'INBOX:5 :11',
           'INBOX:²:11', 'INBOX:٥:١١', 'INBOX:５:１１', 'INBOX:' + '9' * 30 + ':1', 'INBOX:1:' + '9' * 4300, 'INBOX:1:' + '9' * 4301, 'INBOX:' + '0' * 4301 + ':1',
           'INBOX:1_0:2', 'INBOX: 5:11', '::', ':::', 'x:0:0', '收件箱:5:11', 'INBOX:5:11\n', 'INBOX:①:11', 'INBOX:٣٤:٥٦', 'INBOX:1٣:2', 'INBOX:5:1 1']
INTS = ['24', '0', '-1', '+5', ' 7 ', '1_0', '_1', '1_', '1__0', '', ' ', 'x', '1.5', '1e3', '0x10', '336', '337', '900', '901', '99999999999999999999999999', '-99999999999999999999999999',
        '٣', '３３', '²', '1 2', '- 1', '--1', '+-1', '0' * 50 + '9', '9' * 4300, '9' * 4301, '0' * 4301, '-' + '9' * 4301, '\t12\n', '　1　', '1\x00', 'ⅷ', '1٣',
        '-0', '+0', '00', '1_000', '\x1c5\x1f', '\x855\xa0']


def handle_case(s):
    try:
        folder, uid, valid = mail_handle(s)
    except ValueError:
        return {'in': s, 'out': None}
    return {'in': s, 'out': [folder, str(uid), str(valid)]}


def int_case(s):
    try:
        n = int(s)
    except ValueError:
        return {'in': s, 'hours': None, 'secs': None}
    return {'in': s, 'hours': max(1, min(n, 336)), 'secs': max(0, min(n, 900))}


# --- 盯邮箱：mail_hunt 配假的邮件来源、假的时钟、假的「打开」 --------------------------------------------------------------
T0 = 1794708000
BOX = '\x22[Gmail]/All Mail\x22'


class HuntSource:
    """rounds：每看一次邮箱给什么（{'mails': [[uid, 邮件, 几秒前收到的, 取全文时给哪一封（gone 是取不到）]…]}、{'retry': [code, text]}、
    {'fatal': [code, text]}），用完了以后每次都是没有新邮件"""

    def __init__(self, rounds, emails):
        self.rounds, self.emails, self.polls = [dict(r) for r in rounds], emails, 0

    def poll(self, since):
        self.polls += 1
        r = self.rounds.pop(0) if self.rounds else {'mails': []}
        if 'retry' in r:
            raise MailRetry(*r['retry'])
        if 'fatal' in r:
            raise MailFatal(*r['fatal'])
        return [{'folder': BOX, 'label': '所有邮件', 'uid': m[0], 'got': T0 - m[2], 'hdr': self.emails[m[1]], 'seen': (BOX, 1, m[0]),
                 'full': m[3] if len(m) > 3 else m[1]} for m in r['mails']]

    def full(self, cand):
        if cand['full'] == 'gone':
            raise MailRetry('proto', 'Gmail 没给出这封邮件')
        return self.emails[cand['full']]


def hunt_case(emails, name, rounds, deliver=(), secs=30, once=False, again=False, alive=None, since_ago=10, forwarder=None, rules=1, used=()):
    """used：事先记在 used.json 里的 [[邮件, uid, 几秒前收到的], 状态, 记下是几秒前, 谁认领的（me / parent / dead）]"""
    global MAIL_DIR
    MAIL_DIR = tempfile.mkdtemp(prefix='cca-mail-vectors.')
    try:
        if used:
            pids = {'me': os.getpid(), 'parent': os.getppid(), 'dead': 2147483600}
            d = {}
            for (mail, uid, ago), st, age, who in used:
                dg = mail_digest(mail_parse(emails[mail]), {'folder': BOX, 'uid': uid, 'got': T0 - ago})
                d[dg] = dict({'at': time.time() - age, 'st': st}, **({'pid': pids[who]} if who else {}))
            save_json(os.path.join(MAIL_DIR, 'used.json'), d, 0o600)
        clock, said, opened, calls, results = [float(T0)], [], [], {'hello': 0, 'idle': 0, 'alive': 0}, list(deliver)

        def open_it(link):
            opened.append([round(clock[0] - T0, 3), link])
            return results.pop(0) if results else 'ok'

        def is_alive():
            calls['alive'] += 1
            return alive is None or calls['alive'] <= alive

        def count(what):
            calls[what] += 1
        src = HuntSource(rounds, emails)
        try:
            res = mail_hunt(src, [RULE] * rules, 'alice@example.com', T0 - since_ago, T0 + secs, open_it, once=once, again=again, alive=is_alive, say=said.append,
                            hello=lambda: count('hello'), idle=lambda: count('idle'), now=lambda: clock[0], sleep=lambda x: clock.__setitem__(0, clock[0] + x), forwarder=forwarder)
            res = dict(res, rejected=[list(kv) for kv in res['rejected'].items()])
        except Exception as e:   # 原来没有人接的异常
            res = {'error': cls(e)}
        after = load_quiet(os.path.join(MAIL_DIR, 'used.json')) or {}
        return {'name': name, 'rounds': rounds, 'deliver': list(deliver), 'secs': secs, 'once': once, 'again': again, 'alive': alive, 'since_ago': since_ago, 'forwarder': forwarder,
                'rules': rules, 'used': [list(u) for u in used], 'res': res, 'said': said, 'opened': opened, 'polls': src.polls, 'clock': round(clock[0] - T0, 3),
                'hello': calls['hello'], 'idle': calls['idle'], 'alive_calls': calls['alive'], 'used_after': sorted([k, v.get('st')] for k, v in after.items())}
    finally:
        shutil.rmtree(MAIL_DIR, True)
        MAIL_DIR = None


def hunt_cases():
    emails = {name: raw for name, raw, _ in judge_emails()}
    M = lambda *mails: {'mails': [list(m) for m in mails]}
    net = lambda t: {'retry': ['net', t]}
    A, H, T = 'good', 'body html only', 'body text only'
    S = [
        # tests/mail_view.py 的 Hunt 里那些
        ('newest of two, keeps watching', [M((1, H, 5), (2, T, 1))], {}),
        ('browser fails twice then works', [M((1, A, 0))], dict(deliver=['fail', 'fail', 'ok'])),
        ('browser fails three times', [M((2, 'subject encoded', 0))], dict(deliver=['fail', 'fail', 'fail'])),
        ('retries finish past the end', [M((1, A, 0))], dict(deliver=['fail', 'fail', 'ok'], secs=1)),
        ('once stops on browser failure', [M((1, A, 0))], dict(deliver=['fail', 'fail', 'fail'], secs=600, once=True)),
        ('trouble four times then fine', [net('x1'), net('x2'), net('x3'), net('x4'), M()], dict(secs=60)),
        ('down all the time', [net('down')] * 50, dict(secs=20)),
        ('once fails fast before the first poll', [{'retry': ['proxy', '代理断开了连接']}], dict(once=True)),
        ('once keeps waiting after a blip', [M(), net('blip'), M((3, A, 0))], dict(secs=60, once=True)),
        ('fatal stops at once', [{'fatal': ['auth', 'Gmail 不认这个密码']}], {}),
        ('once idle when nothing', [M()], dict(secs=10, once=True)),
        ('once no idle when opening', [M((1, A, 0))], dict(secs=10, once=True)),
        ('used is not opened again', [M((1, A, 0))], dict(secs=5, once=True, used=[[[A, 1, 0], 'done', 30, None]])),
        ('used is opened with again', [M((1, A, 0))], dict(once=True, again=True, used=[[[A, 1, 0], 'done', 30, None]])),
        ('rejections quiet or said', [M((1, 'to bob', 0), (2, 'subject receipt', 0), (3, 'from evil', 0), (4, 'ar fail', 0))], dict(secs=5)),
        ('rejections never said when once', [M((1, 'to bob', 0), (2, 'subject receipt', 0), (3, 'from evil', 0), (4, 'ar fail', 0))], dict(secs=5, once=True)),
        ('stops when login is over', [], dict(secs=600, alive=3)),
        ('mail that cannot be fetched backs off', [M((1, A, 0, 'gone'))], dict(secs=120)),
        ('no link is checked again next time', [M((1, 'body tracking link', 0))], dict(secs=5, once=True)),
        ('refused counts as rejected', [M((1, A, 0))], dict(deliver=['refused'], secs=5)),
        # 别的路
        ('refused when once', [M((1, A, 0))], dict(deliver=['refused'], secs=5, once=True)),
        ('newer mail replaces the one being retried', [M((1, A, 5)), M((2, T, 0))], dict(deliver=['fail', 'ok'])),
        ('older mail does not replace it', [M((2, T, 0)), M((1, A, 5))], dict(deliver=['fail', 'ok'])),
        ('same second, higher uid wins', [M((7, A, 3), (9, T, 3), (8, H, 3))], dict(once=True)),
        ('same second, later round lower uid loses', [M((9, T, 3)), M((8, H, 3))], dict(deliver=['fail', 'ok'])),
        ('old mail is ignored quietly', [M((1, A, 200))], dict(secs=5)),
        ('just inside the clock skew', [M((1, A, 70))], dict(secs=5, once=True)),
        ('just outside the clock skew', [M((1, A, 70.5))], dict(secs=5, once=True)),
        ('forwarded mail with a trusted forwarder', [M((1, 'fwd good', 0))], dict(once=True, forwarder='relay.example.com')),
        ('forwarded mail without one', [M((1, 'fwd good', 0))], dict(secs=5)),
        ('forwarded mail for another alias', [M((1, 'fwd alias bob', 0))], dict(secs=5, forwarder='relay.example.com')),
        ('full message is for someone else', [M((1, A, 0, 'to bob'))], dict(secs=5)),
        ('full message fails dkim', [M((1, A, 0, 'ar fail'))], dict(secs=5)),
        ('full message has no link', [M((1, A, 0, 'body no parts'))], dict(secs=5)),
        ('full message is empty', [M((1, A, 0, 'empty'))], dict(secs=5)),
        ('blip after polling, once, time is up', [M(), net('blip')], dict(secs=2, once=True)),
        ('fatal after something was opened', [M((1, A, 0)), {'fatal': ['auth', 'Gmail 不认这个密码']}], {}),
        ('second mail fails after the first opened', [M((1, A, 5)), M((2, T, 0))], dict(deliver=['ok', 'fail', 'fail', 'fail'])),
        ('second mail opens too', [M((1, A, 5)), M(), M((2, T, 0))], {}),
        ('claimed by a live process', [M((1, A, 0))], dict(secs=5, used=[[[A, 1, 0], 'claim', 10, 'parent']])),
        ('claimed by a dead process', [M((1, A, 0))], dict(secs=5, once=True, used=[[[A, 1, 0], 'claim', 10, 'dead']])),
        ('claimed long ago by a live process', [M((1, A, 0))], dict(secs=5, once=True, used=[[[A, 1, 0], 'claim', 300, 'parent']])),
        ('claimed by this process', [M((1, A, 0))], dict(secs=5, once=True, used=[[[A, 1, 0], 'claim', 10, 'me']])),
        ('again but the browser fails: stays done', [M((1, A, 0))], dict(deliver=['fail', 'fail', 'fail'], once=True, again=True, used=[[[A, 1, 0], 'done', 30, None]])),
        ('no time at all', [M((1, A, 0))], dict(secs=0)),
        ('no time at all, nothing there', [M()], dict(secs=0)),
        ('trouble exactly at the end', [net('late')], dict(secs=0)),
        ('backoff doubles up to 30 seconds', [net('n%d' % i) for i in range(12)], dict(secs=200)),
        ('no rules at all', [M((1, A, 0))], dict(secs=5, rules=0)),
        ('two identical rules', [M((1, A, 0), (2, 'to bob', 0))], dict(secs=5, once=True, rules=2)),
        ('opener answers something odd', [M((1, A, 0))], dict(deliver=['weird'], secs=5)),
        ('opener answers a reason', [M((1, A, 0))], dict(deliver=['dkim'], secs=5)),
        ('login already over', [M((1, A, 0))], dict(alive=0)),
        ('login ends while retrying the browser', [M((1, A, 0))], dict(deliver=['fail', 'fail', 'fail'], alive=4)),
        ('login ends while backing off', [net('x')] * 9, dict(secs=600, alive=6)),
        ('several rejected then one good', [M((1, 'ar fail', 9), (2, 'from evil', 8)), M((3, 'dup to', 7), (4, T, 6))], {}),
        ('same reason twice is counted', [M((1, 'ar fail', 9), (2, 'ar fail', 8), (3, 'to bob', 7), (4, 'to bob', 6))], dict(secs=5, once=True)),
        ('hostile address is an uncaught error', [M((1, 'from deep comment', 0))], dict(secs=5)),
        ('hostile full message is an uncaught error', [M((1, A, 0, 'from deep comment'))], dict(secs=5)),
        ('fetch trouble three times then it works', [M((1, A, 0, 'gone')), M(), M(), M((1, A, 0))], dict(secs=120)),
        ('cannot fetch, once', [M((1, A, 0, 'gone'))], dict(secs=30, once=True)),
    ]
    return [hunt_case(emails, name, rounds, **kw) for name, rounds, kw in S]


# --- 各条路上的上限：真的 PY_SRC 后面接上几个量层数用的子命令，一个数起一个进程量 ------------------------------------------------
# 一层套一层到 Python 调用栈的一千层为止，落在第几层看解析的那个函数自己在第几层（depth：它和子命令的函数之间隔着几层函数），
# 还看是不是这个进程里头一次解析地址（first：见 cli/src/cmd/mail/header.rs 的 first_use）。上面 measure_limits 在一个进程里
# 反复试，量到的是「不是头一次」的数
PROBES = r"""
def probe_nested(kind, n, header):
    v = {'comment': '(' * n + ')' * n + ' no-reply@mail.anthropic.com', 'group': 'a:' * n}[kind]
    return ('%s: %s\r\nSubject: Your secure link to Claude is here\r\n\r\n' % (header, v)).encode()


def probe_warm(addrs, aliases):
    plain = mail_parse(b'From: \x22x@example.com\x22 <a@example.com>\r\nTo: a@example.com\r\n\r\n')
    for _ in range(int(addrs)):
        mail_addrs(plain, 'To')
    for _ in range(int(aliases)):
        mail_from_alias(plain)


def cmd_probe_addr(kind, n, addrs='0', aliases='0'):
    # 子命令里直接调 mail_addrs（cmd_mail_test 取收件人那一处）；addrs / aliases：先解析过几次（getaddresses / parseaddr 各算各的）
    probe_warm(addrs, aliases)
    msg = mail_parse(probe_nested(kind, int(n), 'To'))
    try:
        mail_addrs(msg, 'To')
        print('ok')
    except RecursionError:
        print('RecursionError')


def cmd_probe_alias(kind, n, addrs='0', aliases='0'):
    # 子命令里直接调 mail_from_alias（parseaddr 那条路）
    probe_warm(addrs, aliases)
    msg = mail_parse(probe_nested(kind, int(n), 'From'))
    try:
        mail_from_alias(msg)
        print('ok')
    except RecursionError:
        print('RecursionError')


def cmd_probe_judge(kind, n):
    # 子命令 → mail_judge_any → mail_judge → mail_addrs
    msg = mail_parse(probe_nested(kind, int(n), 'From'))
    try:
        mail_judge_any(mail_rules(), msg, time.time(), 'a@example.com', 0)
        print('ok')
    except RecursionError:
        print('RecursionError')


class ProbeSource:
    def __init__(self, hdr):
        self.hdr = hdr

    def poll(self, since):
        return [{'folder': 'a', 'label': 'a', 'uid': 1, 'got': time.time(), 'hdr': self.hdr, 'seen': 1}]


def cmd_probe_open(kind, n):
    # cca mail open：子命令 → mail_hunt → mail_judge_any → mail_judge → mail_addrs
    src = ProbeSource(probe_nested(kind, int(n), 'From'))
    try:
        mail_hunt(src, mail_rules(), 'a@example.com', 0, 0, lambda link: 'ok', once=True)
        print('ok')
    except RecursionError:
        print('RecursionError')


def cmd_probe_watch(kind, n):
    # 登录时：子命令 → mail_watch 那一层 → mail_hunt → mail_judge_any → mail_judge → mail_addrs
    src = ProbeSource(probe_nested(kind, int(n), 'From'))

    def like_mail_watch():
        return mail_hunt(src, mail_rules(), 'a@example.com', 0, 0, lambda link: 'ok', alive=lambda: True)
    try:
        like_mail_watch()
        print('ok')
    except RecursionError:
        print('RecursionError')


def probe_message(kind, n):
    unit, leaf = {'rfc822': ('Content-Type: message/rfc822\r\n\r\n', 'Subject: x\r\n\r\nhi\r\n'),
                  'multipart': ('Content-Type: multipart/mixed; boundary=b%d\r\n\r\n--b%d\r\n', '\r\nhi\r\n')}[kind]
    return (''.join(unit % ((i, i) if '%d' in unit else ()) for i in range(n)) + leaf).encode()


def cmd_probe_parse(kind, n):
    # 子命令里直接调 mail_parse
    raw = probe_message(kind, int(n))
    try:
        mail_parse(raw)
        print('ok')
    except RecursionError:
        print('RecursionError')


def cmd_probe_verdict(kind, n, extra='0'):
    # 子命令 → mail_verdict → mail_parse（cmd_mail_test 取全文那一处）；extra：中间再隔几层（3 层相当于登录时 mail_watch → mail_hunt → mail_take）
    raw = probe_message(kind, int(n))

    def deeper(k):
        return deeper(k - 1) if k > 1 else mail_verdict(mail_rules()[0], raw, time.time(), 'a@example.com', 0)
    try:
        deeper(int(extra)) if int(extra) else mail_verdict(mail_rules()[0], raw, time.time(), 'a@example.com', 0)
        print('ok')
    except RecursionError:
        print('RecursionError')


CMDS.update({'probe-addr': cmd_probe_addr, 'probe-alias': cmd_probe_alias, 'probe-judge': cmd_probe_judge, 'probe-open': cmd_probe_open, 'probe-watch': cmd_probe_watch,
             'probe-parse': cmd_probe_parse, 'probe-verdict': cmd_probe_verdict})
"""
# (这一处是什么, 起哪个量层数的子命令和它后面的参数, 记进对照数据的：解析的是地址还是邮件、经 getaddresses 还是 parseaddr、隔着几层、是不是头一次)
PATHS = (
    ('cmd_mail_test 里头一次解析地址', ['probe-addr'], dict(what='address', via='getaddresses', depth=0, first=True)),
    ('同一处，第二次起', ['probe-addr', '1', '0'], dict(what='address', via='getaddresses', depth=0, first=False)),
    ('同一处，先用过 parseaddr 不算', ['probe-addr', '0', '3'], dict(what='address', via='getaddresses', depth=0, first=True)),
    ('子命令里头一次 mail_from_alias', ['probe-alias', '3', '0'], dict(what='address', via='parseaddr', depth=0, first=True)),
    ('同一处，第二次起', ['probe-alias', '0', '1'], dict(what='address', via='parseaddr', depth=0, first=False)),
    ('经 mail_judge_any → mail_judge', ['probe-judge'], dict(what='address', via='getaddresses', depth=2, first=True)),
    ('cca mail open（经 mail_hunt）', ['probe-open'], dict(what='address', via='getaddresses', depth=3, first=True)),
    ('登录时（mail_watch → mail_hunt）', ['probe-watch'], dict(what='address', via='getaddresses', depth=4, first=True)),
    ('子命令里直接 mail_parse', ['probe-parse'], dict(what='message', depth=0)),
    ('cmd_mail_test 取全文（经 mail_verdict）', ['probe-verdict'], dict(what='message', depth=1)),
    ('cca mail open 取全文', ['probe-verdict', '2'], dict(what='message', depth=3)),
    ('登录时取全文', ['probe-verdict', '3'], dict(what='message', depth=4)),
)


def path_limits(old):
    """各条路上最多套几层。cca 里已经没有 PY_SRC 时沿用 old（上一次生成的那一节）"""
    import subprocess
    try:
        with open(os.path.join(REPO, 'cca'), encoding='utf-8') as f:
            src = f.read()
    except OSError:
        return old
    if "PY_SRC=$(cat <<'PY'\n" not in src:
        return old
    py = src.split("PY_SRC=$(cat <<'PY'\n", 1)[1].split('\nPY\n)', 1)[0]
    check = 'if len(sys.argv) < 2 or sys.argv[1] not in CMDS:\n'
    if py.count(check) != 1:
        sys.exit('PY_SRC 最后分派子命令的写法变了：path_limits 要跟着改')
    d = tempfile.mkdtemp(prefix='cca-mail-limits.')
    try:
        probe = os.path.join(d, 'py_src.py')
        with open(probe, 'w', encoding='utf-8') as f:
            f.write(py.replace(check, PROBES + check) + '\n')
        env = {'HOME': os.path.join(d, 'home'), 'PATH': '/usr/bin:/bin', 'LANG': 'en_US.UTF-8'}   # 沙箱里的 HOME：不碰真的 ~/.cca
        os.makedirs(env['HOME'])

        def fits(argv, kind, n):
            r = subprocess.run([sys.executable, '-I', '-X', 'utf8', probe, argv[0], kind, str(n)] + argv[1:], capture_output=True, text=True, env=env, timeout=120)
            last = (r.stdout.strip().splitlines() or [''])[-1]
            if last not in ('ok', 'RecursionError'):
                sys.exit('量层数的子命令没跑成：%s %s %d\n%s' % (argv, kind, n, r.stderr[-800:]))
            return last == 'ok'
        out = []
        for name, argv, meta in PATHS:
            for kind in (('comment', 'group') if meta['what'] == 'address' else ('rfc822', 'multipart')):
                lo, hi = 400, 1100
                assert fits(argv, kind, lo) and not fits(argv, kind, hi)
                while hi - lo > 1:
                    mid = (lo + hi) // 2
                    if fits(argv, kind, mid):
                        lo = mid
                    else:
                        hi = mid
                out.append(dict(meta, path=name, kind=kind, max=lo))
        return out
    finally:
        shutil.rmtree(d, True)


ADDR_NESTS = (('comment', '', '(', ')', ' a@example.com'), ('domain comment', 'a@', '(', ')', 'b.example.com'), ('phrase comment', 'x ', '(', ')', ' <a@example.com>'),
              ('route comment', '<', '(', ')', 'a@example.com>'), ('group comment', 'g: ', '(', ')', 'a@example.com;'), ('group', '', 'a:', '', ''))
MSG_NESTS = (('rfc822', 'Content-Type: message/rfc822\r\n\r\n', 'Subject: x\r\n\r\nhi\r\n'), ('multipart', 'Content-Type: multipart/mixed; boundary=b%d\r\n\r\n--b%d\r\n', '\r\nhi\r\n'))


def nest_addr(kind, n):
    _, pre, a, b, post = next(x for x in ADDR_NESTS if x[0] == kind)
    return pre + a * n + b * n + post


def nest_msg(kind, n):
    _, unit, leaf = next(x for x in MSG_NESTS if x[0] == kind)
    return (''.join(unit % ((i, i) if '%d' in unit else ()) for i in range(n)) + leaf).encode()


def parse_at_3(raw):
    """相当于 mail_verdict：它在第 3 层，mail_parse 在第 4 层"""
    return mail_parse(raw)


def measure_limits():
    """Python 的调用栈上限（一千层）落在哪。要在和 cca 一样深的调用栈上量：这个函数从模块的最外层调（第 2 层，相当于
    cmd_mail_test），下面直接调 mail_addrs（第 3 层）和 parse_at_3（第 3 层），中间不能再隔着别的函数。
    地址的各种写法都在这里量；在一个进程里反复试，量到的是「不是头一次解析地址」的数（头一次多一层，别的调用深度见 path_limits）"""
    out = []
    for kind in [x[0] for x in ADDR_NESTS]:
        lo, hi = 1, 3000
        while hi - lo > 1:
            mid = (lo + hi) // 2
            msg = mail_parse(('To: %s\r\n\r\n' % nest_addr(kind, mid)).encode())
            try:
                mail_addrs(msg, 'To')
                lo = mid
            except RecursionError:
                hi = mid
        out.append({'what': 'address', 'kind': kind, 'max': lo})
    for kind in [x[0] for x in MSG_NESTS]:
        lo, hi = 1, 3000
        while hi - lo > 1:
            mid = (lo + hi) // 2
            raw = nest_msg(kind, mid)
            try:
                parse_at_3(raw)
                lo = mid
            except RecursionError:
                hi = mid
        out.append({'what': 'message', 'kind': kind, 'max': lo})
    return out


def vectors(seed, extra, old_paths):
    rng = random.Random(seed)
    from urllib.parse import urlsplit

    def host(u):
        try:
            return urlsplit(u).hostname or ''
        except ValueError:
            return ''
    cases = {
        'codec_lookup': codec_lookup_cases(rng, extra),
        'codec_decode': codec_decode_cases(rng, extra),
        'raw_unicode_escape': raw_escape_cases(),
        'transfer': transfer_cases(rng, extra),
        'message': message_cases(rng, extra),
        'text': text_cases(rng, extra),
        'addresses': addr_cases(rng, extra),
        'html': html_cases(rng, extra),
        'html_attr': attr_cases(rng, extra),
        'tags': [{'in': t, 'out': [list(kv) for kv in mail_tags(t).items()]} for t in TAGS],
        'ar_items': ar_cases(rng, extra),
        'subject': subject_cases(rng, extra),
        'link': [{'in': u, 'ok': RULE.link_ok(u), 'shape': mail_shape(u), 'host': host(u)} for u in LINKS],
        'domain': [{'in': d, 'ok': RULE.domain_ok(d), 'fwd': mail_fwd_ok(d, 'relay.example.com')} for d in DOMAINS],
        'oserr': [oserr_case(t, k) for t, k in OSERR],
        'judge': [judge_case(*j) for j in judge_emails()],
        'limits': LIMITS,
        'limit_paths': old_paths if extra != 1 else path_limits(old_paths),
        'proxy_parse': [(lambda r: {'in': x, 'kv': r[0], 'exc': r[1]})(mail_proxy_parse(x)) for x in SCUTILS],
        'fnmatch': fnmatch_cases(rng, extra),
        'epoch': [epoch_case(x) for x in EPOCHS],
        'since': [{'t': t, 'since': mail_since(t), 'crit': mail_crit([RULE], t), 'crit2': mail_crit([RULE, RULE], t), 'crit3': mail_crit([RULE, RULE, RULE], t), 'crit0': mail_crit([], t)} for t in SINCES],
        'fetch_parse': [fetch_case(x) for x in FETCHES],
        'list_row': [list_row_case(x) for x in LIST_ROWS],
        'imap': imap_cases(),
        'cfg': [cfg_case(c) for c in cfg_inputs()],
        'handle': [handle_case(x) for x in HANDLES],
        'int': [int_case(x) for x in INTS],
        'hunt': hunt_cases(),
    }
    return cases


def main():
    args = sys.argv[1:]
    extra, seed = 1, 20261119
    if '--fuzz' in args:
        extra = int(args[args.index('--fuzz') + 1])
        del args[args.index('--fuzz'):args.index('--fuzz') + 2]
    if '--seed' in args:
        seed = int(args[args.index('--seed') + 1])
        del args[args.index('--seed'):args.index('--seed') + 2]
    check_reference()
    out = args[0]
    if not out.endswith('.json'):
        os.makedirs(out, exist_ok=True)
        out = os.path.join(out, 'mail.json')
    try:   # 上一次量的各条路的上限：cca 里没有 PY_SRC 了、或者只是多跑一批随机输入时沿用
        with open(out, encoding='utf-8') as f:
            old_paths = json.load(f)['cases'].get('limit_paths') or []
    except (OSError, ValueError, KeyError):
        old_paths = []
    cases = vectors(seed, extra, old_paths)
    with open(out, 'w', encoding='utf-8') as f:   # 一个用例一行：文件小一些，改了哪个用例 git diff 里也看得清
        f.write('{\n"python": "%d.%d",\n"cases": {\n' % sys.version_info[:2])
        for i, (name, items) in enumerate(cases.items()):
            f.write('"%s": [\n' % name)
            f.write(',\n'.join(json.dumps(compact(c), ensure_ascii=False, separators=(',', ':')) for c in items))
            f.write('\n]%s\n' % (',' if i + 1 < len(cases) else ''))
        f.write('}\n}\n')


LIMITS = measure_limits()   # 要从最外层调：见它的说明
main()
