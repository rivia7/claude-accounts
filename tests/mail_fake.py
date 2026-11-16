#!/usr/bin/env python3
"""邮箱助手（cca mail）的测试替身，不联网：

- 测试用的证书（make_pki）：一个 CA 和它签的 imap.gmail.com 的证书（EC P-256，openssl 生成）
- 假的 Gmail（FakeGmail）：真的 TLS，只实现 cca 用得到的那一点 IMAP（CAPABILITY、LOGIN、LIST、EXAMINE、UID SEARCH、UID FETCH、
  LOGOUT、NOOP）；会改邮箱的命令（SELECT、STORE、COPY、MOVE、EXPUNGE、APPEND、CREATE……）和不带 PEEK 的 BODY[]（会标已读）
  一律回 BAD，并在日志里记一行 VIOLATION
- 假的代理（FakeProxy）：一个端口同时认 SOCKS5 和 HTTP CONNECT，不管要连哪个主机都接到假 Gmail 上，日志里记下要连的目标
- 登录邮件（build_eml）：Anthropic 的样子（DKIM 签名、Gmail 写的 Authentication-Results、HTML 和纯文本），各处都能改

邮箱放在 目录/box/<邮件夹>/<UID>.eml（all、spam、inbox），Gmail 收到的时间（INTERNALDATE）取文件的修改时间；每次 EXAMINE、SEARCH、
FETCH 都重新读目录，测试往里放文件就是来了新邮件。目录/ctl.json 改假 Gmail 的行为（每个命令都重新读）：
  user、password（默认 inbox@example.com / abcdefghijklmnop）、imap_disabled、login_no（LOGIN 一律回 NO 和这段话）、
  no_all（没有 \\All）、index_lag（这么多秒以内收到的 SEARCH 搜不到：模拟 Gmail 的索引还没跟上）、
  uid_after_literal（FETCH 的 UID 写在 literal 后面）、drop（收到的命令里有这段文字时直接断开连接）、
  proxy（refuse：代理不肯连；auth：SOCKS5 要认证）

命令行（regress 用）：
  python3 mail_fake.py serve 目录      起假 Gmail 和代理，端口写进 目录/ports.json（{"imap":…, "proxy":…}），日志 目录/imap.log
  python3 mail_fake.py eml 文件 --to 地址 [--link 网址] [--subject …] [--sign from:to:subject] [--ar pass|fail|none]
                                 [--from 地址] [--dkim-d 域名] [--at 时间戳] [--msgid …]
"""
import argparse
import email.utils
import json
import os
import random
import re
import socket
import ssl
import string
import subprocess
import sys
import threading
import time
from email.header import decode_header, make_header

GOOD_LINK = 'https://claude.ai/magic-link#tok-%s:YWxpY2VAZXhhbXBsZS5jb20'
MONTHS = ('Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec')
FOLDERS = (('inbox', 'INBOX', r'\HasNoChildren'),
           ('all', '[Gmail]/All Mail', r'\All \HasNoChildren'),
           ('spam', '[Gmail]/Spam', r'\HasNoChildren \Junk'))
WRITES = ('SELECT', 'STORE', 'COPY', 'MOVE', 'EXPUNGE', 'APPEND', 'CREATE', 'DELETE', 'RENAME', 'SUBSCRIBE', 'UNSUBSCRIBE',
          'CLOSE', 'IDLE', 'AUTHENTICATE', 'STARTTLS')


# --- 证书
def make_pki(d):
    """d/ca.pem、d/cert.pem、d/key.pem（imap.gmail.com，CA 签的；Python 3.13 起默认的严格校验也过）"""
    os.makedirs(d, exist_ok=True)
    cnf = os.path.join(d, 'pki.cnf')
    with open(cnf, 'w') as f:
        f.write('[req]\ndistinguished_name = dn\nprompt = no\n[dn]\nCN = cca test mail CA\n'
                '[v3_ca]\nbasicConstraints = critical, CA:TRUE\nkeyUsage = critical, keyCertSign, cRLSign\n'
                'subjectKeyIdentifier = hash\n'
                '[leaf]\nbasicConstraints = critical, CA:FALSE\nkeyUsage = critical, digitalSignature\n'
                'extendedKeyUsage = serverAuth\nsubjectAltName = DNS:imap.gmail.com\nsubjectKeyIdentifier = hash\n'
                'authorityKeyIdentifier = keyid:always\n')
    q = dict(stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
    p = lambda n: os.path.join(d, n)
    subprocess.run(['openssl', 'ecparam', '-name', 'prime256v1', '-genkey', '-noout', '-out', p('ca.key')], **q)
    subprocess.run(['openssl', 'req', '-new', '-x509', '-key', p('ca.key'), '-config', cnf, '-extensions', 'v3_ca',
                    '-days', '3', '-out', p('ca.pem')], **q)
    subprocess.run(['openssl', 'ecparam', '-name', 'prime256v1', '-genkey', '-noout', '-out', p('key.pem')], **q)
    subprocess.run(['openssl', 'req', '-new', '-key', p('key.pem'), '-subj', '/CN=imap.gmail.com', '-out', p('leaf.csr')], **q)
    subprocess.run(['openssl', 'x509', '-req', '-in', p('leaf.csr'), '-CA', p('ca.pem'), '-CAkey', p('ca.key'),
                    '-CAcreateserial', '-days', '2', '-extfile', cnf, '-extensions', 'leaf', '-out', p('cert.pem')], **q)
    return p('ca.pem')


# --- 登录邮件
def rand(n, chars=string.ascii_letters + string.digits):
    return ''.join(random.choice(chars) for _ in range(n))


def build_eml(to, link=None, subject='Your secure link to Claude is here', frm='Anthropic <no-reply@mail.anthropic.com>',
              sign='from:to:subject:date:message-id:mime-version:content-type', ar='pass', dkim_d='mail.anthropic.com',
              msgid=None, extra=(), above=(), body_links=None, html=True, text=True):
    """一封登录邮件（字节）。sign 是 DKIM 签名的 h=；ar 是最上面那条（Gmail 写的）Authentication-Results 里 DKIM 的结果：
    pass、fail、none（没有 DKIM 结果）、missing（没有这一条）；above 是插在最上面的头（模拟别人加的）、extra 加在后面"""
    link = link or GOOD_LINK % rand(12)
    b = rand(340, string.ascii_letters + string.digits + '+/') + '=='
    msgid = msgid or '<%s@mail.anthropic.com>' % rand(20)
    lines = list(above)
    lines += ['Delivered-To: inbox@example.com',
              'Received: by 2002:a05:6000:1:b0:1 with SMTP id x; Sat, 15 Nov 2026 02:00:00 -0800 (PST)']
    if ar != 'missing':
        res = {'pass': 'dkim=pass header.i=@%s header.s=s1 header.b=%s' % (dkim_d, b[:8]),
               'fail': 'dkim=fail header.i=@%s header.s=s1 header.b=%s' % (dkim_d, b[:8]),
               'none': 'dkim=none'}[ar]
        lines += ['Authentication-Results: mx.google.com;',
                  '       %s;' % res,
                  '       arc=pass (i=1 spf=pass spfdomain=mail.anthropic.com dkim=pass dkdomain=mail.anthropic.com);',
                  '       spf=pass (google.com: domain of bounce@mail.anthropic.com designates 192.0.2.10 as permitted sender) smtp.mailfrom=bounce@mail.anthropic.com;',
                  '       dmarc=pass (p=REJECT sp=REJECT dis=NONE) header.from=mail.anthropic.com']
    lines += ['DKIM-Signature: v=1; a=rsa-sha256; c=relaxed/relaxed; d=%s; s=s1;' % dkim_d,
              '\tt=1763200800; h=%s;' % sign,
              '\tbh=%s=;' % rand(43),
              '\tb=%s' % b[:76]] + ['\t %s' % b[i:i + 76] for i in range(76, len(b), 76)]
    lines += list(extra)
    lines += ['From: %s' % frm, 'To: %s' % to, 'Subject: %s' % subject,
              'Date: %s' % email.utils.formatdate(localtime=False), 'Message-ID: %s' % msgid, 'MIME-Version: 1.0']
    links = body_links if body_links is not None else [link]
    boundary = 'b-' + rand(16)
    parts = []
    if text:
        parts.append('Content-Type: text/plain; charset="utf-8"\nContent-Transfer-Encoding: 7bit\n\n'
                     'Click the link below to log in to Claude.ai:\n\n%s\n\nThis link expires in 1 hour.\n'
                     % '\n'.join(links))
    if html:
        anchors = ''.join('<a href=3D"%s" style=3D"color:#fff">Sign in to Claude.ai</a><br>' % l.replace('=', '=3D')
                          for l in links)
        parts.append('Content-Type: text/html; charset="utf-8"\nContent-Transfer-Encoding: quoted-printable\n\n'
                     '<html><body><p>Click the button to log in:</p>%s<p>If you did not request this, ignore it.</p></body></html>\n'
                     % anchors)
    lines.append('Content-Type: multipart/alternative; boundary="%s"' % boundary)
    body = ''.join('--%s\n%s' % (boundary, p) for p in parts) + '--%s--\n' % boundary
    return ('\r\n'.join(lines) + '\r\n\r\n' + body.replace('\n', '\r\n')).encode('utf-8')


def put_eml(path, data, at=None):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = path + '.tmp'
    with open(tmp, 'wb') as f:
        f.write(data)
    at = time.time() if at is None else at
    os.utime(tmp, (at, at))
    os.replace(tmp, path)


# --- 假 Gmail
class Box:
    def __init__(self, d):
        self.d = d

    def ctl(self):
        try:
            with open(os.path.join(self.d, 'ctl.json')) as f:
                c = json.load(f)
        except (OSError, ValueError):
            c = {}
        c.setdefault('user', 'inbox@example.com')
        c.setdefault('password', 'abcdefghijklmnop')
        return c

    def folder_dir(self, name):
        for key, imap_name, _ in FOLDERS:
            if name == imap_name or name == '"%s"' % imap_name:
                return key
        return None

    def msgs(self, key):
        """[(uid, internaldate 时间戳, 字节)]，按 UID 排"""
        d = os.path.join(self.d, 'box', key)
        out = []
        try:
            names = os.listdir(d)
        except OSError:
            return out
        for n in names:
            m = re.match(r'^(\d+)\.eml$', n)
            if not m:
                continue
            p = os.path.join(d, n)
            try:
                with open(p, 'rb') as f:
                    out.append((int(m.group(1)), os.stat(p).st_mtime, f.read()))
            except OSError:
                pass
        return sorted(out)

    def uidvalidity(self, key):
        try:
            with open(os.path.join(self.d, 'box', key, '.uidvalidity')) as f:
                return int(f.read().strip())
        except (OSError, ValueError):
            return 11


def idate(t):
    g = time.gmtime(t)
    return '%02d-%s-%d %02d:%02d:%02d +0000' % (g.tm_mday, MONTHS[g.tm_mon - 1], g.tm_year, g.tm_hour, g.tm_min, g.tm_sec)


def tokens(s):
    """IMAP 参数 → 记号：原子、带引号的字符串（去掉引号）、括号"""
    out, i = [], 0
    while i < len(s):
        c = s[i]
        if c.isspace():
            i += 1
        elif c in '()':
            out.append(c)
            i += 1
        elif c == '"':
            j, buf = i + 1, ''
            while j < len(s) and s[j] != '"':
                if s[j] == '\\' and j + 1 < len(s):
                    j += 1
                buf += s[j]
                j += 1
            out.append(('q', buf))
            i = j + 1
        else:
            j = i
            while j < len(s) and not s[j].isspace() and s[j] not in '()':
                j += 1
            out.append(s[i:j])
            i = j
    return out


def hdr_text(raw, name):
    from email import message_from_bytes
    m = message_from_bytes(raw)
    vals = []
    for v in m.get_all(name) or []:
        try:
            vals.append(str(make_header(decode_header(str(v)))))
        except Exception:
            vals.append(str(v))
    return ' '.join(vals)


def search_eval(toks, msg):
    """SEARCH 条件（记号列表）对这封邮件成不成立：只认 cca 用的 ALL、SINCE、FROM、SUBJECT、OR、括号；别的报 ValueError（回 BAD）"""
    _, t, raw = msg
    pos = [0]

    def take():
        if pos[0] >= len(toks):
            raise ValueError('truncated')
        pos[0] += 1
        return toks[pos[0] - 1]

    def one():
        k = take()
        if k == '(':
            ok = True
            while True:
                if pos[0] >= len(toks):
                    raise ValueError('unclosed')
                if toks[pos[0]] == ')':
                    pos[0] += 1
                    return ok
                ok = one() and ok   # 先解析完再算
        if isinstance(k, tuple) or k == ')':
            raise ValueError('unexpected %r' % (k,))
        u = k.upper()
        if u == 'ALL':
            return True
        if u == 'OR':
            a = one()
            b = one()
            return a or b
        arg = take()
        arg = arg[1] if isinstance(arg, tuple) else arg
        if u == 'SINCE':
            m = re.match(r'^(\d{1,2})-([A-Za-z]{3})-(\d{4})$', arg)
            if not m:
                raise ValueError('bad date')
            day = (int(m.group(3)), MONTHS.index(m.group(2).title()) + 1, int(m.group(1)))
            g = time.gmtime(t)
            return (g.tm_year, g.tm_mon, g.tm_mday) >= day
        if u == 'FROM':
            return arg.lower() in hdr_text(raw, 'From').lower()
        if u == 'SUBJECT':
            return arg.lower() in hdr_text(raw, 'Subject').lower()
        raise ValueError('unsupported search key %s' % k)

    ok = True
    while pos[0] < len(toks):
        ok = one() and ok
    return ok


def uid_set(spec, uids):
    want = set()
    top = max(uids) if uids else 0
    for part in spec.split(','):
        if ':' in part:
            a, b = part.split(':', 1)
            a = top if a == '*' else int(a)
            b = top if b == '*' else int(b)
            lo, hi = min(a, b), max(a, b)
            want.update(u for u in uids if lo <= u <= hi)
        else:
            want.add(top if part == '*' else int(part))
    return want


def header_fields(raw, names):
    head = raw.split(b'\r\n\r\n', 1)[0].split(b'\r\n')
    out, keep = [], False
    for line in head:
        if line[:1] in (b' ', b'\t'):
            if keep:
                out.append(line)
            continue
        keep = line.split(b':', 1)[0].strip().upper().decode('latin-1') in names
        if keep:
            out.append(line)
    return b'\r\n'.join(out) + b'\r\n\r\n'


class Conn(threading.Thread):
    def __init__(self, srv, sock):
        threading.Thread.__init__(self, daemon=True)
        self.srv, self.sock, self.f = srv, sock, None
        self.folder = None
        self.authed = False

    def send(self, s):
        self.f.write(s if isinstance(s, bytes) else s.encode('utf-8'))
        self.f.flush()

    def run(self):
        try:
            self.sock = self.srv.ctx.wrap_socket(self.sock, server_side=True)
        except (OSError, ssl.SSLError):
            self.sock.close()
            return
        self.f = self.sock.makefile('rwb')
        try:
            self.send('* OK Gimap ready for requests from 127.0.0.1 x1mb12345\r\n')
            while True:
                line = self.f.readline()
                if not line:
                    break
                line = line.rstrip(b'\r\n').decode('utf-8', 'replace')
                if not self.handle(line):
                    break
        except (OSError, ssl.SSLError, ValueError):
            pass
        finally:
            for x in (self.f, self.sock):
                try:
                    x.close()
                except (OSError, ssl.SSLError):
                    pass

    def handle(self, line):
        c = self.srv.box.ctl()
        tag, _, rest = line.partition(' ')
        verb, _, args = rest.partition(' ')
        verb = verb.upper()
        if verb == 'UID':
            sub, _, args = args.partition(' ')
            verb = 'UID ' + sub.upper()
        self.srv.log('IMAP %s %s' % (verb, '%s ***' % args.split(' ')[0] if verb == 'LOGIN' else args))
        if c.get('drop') and c['drop'] in line:
            self.srv.log('DROP %s' % verb)
            return False
        if verb in WRITES or verb.split(' ')[-1] in WRITES:
            self.srv.log('VIOLATION %s %s' % (verb, args))
            self.send('%s BAD Command not allowed in the test\r\n' % tag)
            return True
        if verb == 'CAPABILITY':
            self.send('* CAPABILITY IMAP4rev1 UNSELECT IDLE NAMESPACE QUOTA ID XLIST CHILDREN X-GM-EXT-1 XYZZY SASL-IR '
                      'AUTH=XOAUTH2 AUTH=PLAIN AUTH=PLAIN-CLIENTTOKEN AUTH=OAUTHBEARER\r\n%s OK Thats all she wrote!\r\n' % tag)
        elif verb == 'LOGIN':
            parts = tokens(args)
            user = parts[0][1] if isinstance(parts[0], tuple) else parts[0]
            pw = parts[1][1] if len(parts) > 1 and isinstance(parts[1], tuple) else (parts[1] if len(parts) > 1 else '')
            if c.get('imap_disabled'):
                self.send('%s NO [ALERT] Your account is not enabled for IMAP use. Please visit your Gmail settings page '
                          'and enable your account for IMAP access. (Failure)\r\n' % tag)
            elif c.get('login_no'):
                self.send('%s NO %s\r\n' % (tag, c['login_no']))
            elif user == c['user'] and pw == c['password']:
                self.authed = True
                self.send('* CAPABILITY IMAP4rev1 UNSELECT IDLE NAMESPACE QUOTA ID XLIST CHILDREN X-GM-EXT-1 UIDPLUS '
                          'COMPRESS=DEFLATE ENABLE MOVE CONDSTORE ESEARCH UTF8=ACCEPT LIST-EXTENDED LIST-STATUS LITERAL- '
                          'SPECIAL-USE APPENDLIMIT=35651584\r\n%s OK %s authenticated (Success)\r\n' % (tag, user))
            else:
                self.send('%s NO [AUTHENTICATIONFAILED] Invalid credentials (Failure)\r\n' % tag)
        elif not self.authed and verb not in ('LOGOUT', 'NOOP'):
            self.send('%s BAD Command not valid in this state\r\n' % tag)
        elif verb == 'LIST':
            for key, name, attrs in FOLDERS:
                if key == 'all' and c.get('no_all'):
                    continue
                self.send('* LIST (%s) "/" "%s"\r\n' % (attrs, name))
            self.send('* LIST (\\HasChildren \\Noselect) "/" "[Gmail]"\r\n* LIST (\\HasNoChildren \\Sent) "/" "[Gmail]/Sent Mail"\r\n'
                      '%s OK Success\r\n' % tag)
        elif verb == 'EXAMINE':
            key = self.srv.box.folder_dir(args.strip())
            if key is None or (key == 'all' and c.get('no_all')):
                self.send('%s NO [NONEXISTENT] Unknown Mailbox: %s (Failure)\r\n' % (tag, args))
                return True
            self.folder = key
            ms = self.srv.box.msgs(key)
            nxt = max([u for u, _, _ in ms] + [0]) + 1
            self.send('* FLAGS (\\Answered \\Flagged \\Draft \\Deleted \\Seen $NotPhishing $Phishing)\r\n'
                      '* OK [PERMANENTFLAGS ()] Flags permitted.\r\n* OK [UIDVALIDITY %d] UIDs valid.\r\n* %d EXISTS\r\n'
                      '* 0 RECENT\r\n* OK [UIDNEXT %d] Predicted next UID.\r\n* OK [HIGHESTMODSEQ 1234]\r\n'
                      '%s OK [READ-ONLY] %s selected. (Success)\r\n' % (self.srv.box.uidvalidity(key), len(ms), nxt, tag, args))
        elif verb == 'UID SEARCH':
            if not self.folder:
                self.send('%s BAD No mailbox selected\r\n' % tag)
                return True
            toks = tokens(args)
            lag = float(c.get('index_lag') or 0)
            try:
                hits = [m[0] for m in self.srv.box.msgs(self.folder)
                        if time.time() - m[1] >= lag and search_eval(toks, m)]
            except (ValueError, StopIteration, AttributeError) as e:
                self.send('%s BAD Could not parse command (%s)\r\n' % (tag, e))
                return True
            self.send('* SEARCH%s\r\n%s OK SEARCH completed (Success)\r\n' % (''.join(' %d' % u for u in hits), tag))
        elif verb == 'UID FETCH':
            if not self.folder:
                self.send('%s BAD No mailbox selected\r\n' % tag)
                return True
            spec, _, items = args.partition(' ')
            up = items.upper()
            if re.search(r'\bBODY\[', up) or re.search(r'\bRFC822(?![.](HEADER|SIZE))\b', up):
                self.srv.log('VIOLATION UID FETCH %s' % items)   # 不带 PEEK：会标已读
                self.send('%s BAD Would mark as read\r\n' % tag)
                return True
            ms = self.srv.box.msgs(self.folder)
            want = uid_set(spec, [u for u, _, _ in ms])
            hf = re.search(r'BODY\.PEEK\[HEADER\.FIELDS \(([^)]*)\)\]', items, re.I)
            part = re.search(r'BODY\.PEEK\[\]<0\.(\d+)>', items, re.I)
            for seq, (u, t, raw) in enumerate(ms, 1):
                if u not in want:
                    continue
                if hf:
                    names = {n.upper() for n in hf.group(1).split()}
                    data, label = header_fields(raw, names), 'BODY[HEADER.FIELDS (%s)]' % hf.group(1)
                elif part:
                    data, label = raw[:int(part.group(1))], 'BODY[]<0>'
                else:
                    data, label = b'', None
                if label is None:
                    self.send('* %d FETCH (UID %d INTERNALDATE "%s")\r\n' % (seq, u, idate(t)))
                elif c.get('uid_after_literal'):
                    self.send(('* %d FETCH (INTERNALDATE "%s" %s {%d}\r\n' % (seq, idate(t), label, len(data))).encode() + data +
                              (' UID %d)\r\n' % u).encode())
                else:
                    self.send(('* %d FETCH (UID %d INTERNALDATE "%s" %s {%d}\r\n' % (seq, u, idate(t), label, len(data))).encode()
                              + data + b')\r\n')
            self.send('%s OK Success\r\n' % tag)
        elif verb == 'NOOP':
            self.send('%s OK Success\r\n' % tag)
        elif verb == 'LOGOUT':
            self.send('* BYE LOGOUT Requested\r\n%s OK 73 good day (Success)\r\n' % tag)
            return False
        else:
            self.srv.log('VIOLATION %s %s' % (verb, args))
            self.send('%s BAD Unknown command\r\n' % tag)
        return True


class Server(threading.Thread):
    def __init__(self, d, port=0):
        threading.Thread.__init__(self, daemon=True)
        self.d = d
        self.box = Box(d)
        self.ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        self.ctx.load_cert_chain(os.path.join(d, 'pki', 'cert.pem'), os.path.join(d, 'pki', 'key.pem'))
        self.lk = threading.Lock()
        self.ls = socket.socket()
        self.ls.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.ls.bind(('127.0.0.1', port))
        self.ls.listen(16)
        self.port = self.ls.getsockname()[1]

    def log(self, s):
        with self.lk, open(os.path.join(self.d, 'imap.log'), 'a') as f:
            f.write(s + '\n')

    def run(self):
        while True:
            try:
                s, _ = self.ls.accept()
            except OSError:
                return
            Conn(self, s).start()


# --- 假代理
def pipe(a, b):
    try:
        while True:
            data = a.recv(65536)
            if not data:
                break
            b.sendall(data)
    except OSError:
        pass
    for s in (a, b):
        try:
            s.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass


def recv_n(s, n):
    buf = b''
    while len(buf) < n:
        b = s.recv(n - len(buf))
        if not b:
            raise OSError('closed')
        buf += b
    return buf


class Proxy(threading.Thread):
    def __init__(self, srv, port=0):
        threading.Thread.__init__(self, daemon=True)
        self.srv = srv
        self.ls = socket.socket()
        self.ls.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.ls.bind(('127.0.0.1', port))
        self.ls.listen(16)
        self.port = self.ls.getsockname()[1]

    def run(self):
        while True:
            try:
                s, _ = self.ls.accept()
            except OSError:
                return
            threading.Thread(target=self.serve, args=(s,), daemon=True).start()

    def serve(self, s):
        mode = self.srv.box.ctl().get('proxy')
        try:
            first = recv_n(s, 1)
            if first == b'\x05':
                n = recv_n(s, 1)[0]
                recv_n(s, n)
                if mode == 'auth':
                    s.sendall(b'\x05\xff')
                    self.srv.log('PROXY socks5 auth-required')
                    return s.close()
                s.sendall(b'\x05\x00')
                ver, cmd, _, atyp = recv_n(s, 4)
                if atyp == 3:
                    host = recv_n(s, recv_n(s, 1)[0]).decode('ascii')
                elif atyp == 1:
                    host = socket.inet_ntoa(recv_n(s, 4))
                else:
                    host = socket.inet_ntop(socket.AF_INET6, recv_n(s, 16))
                port = int.from_bytes(recv_n(s, 2), 'big')
                self.srv.log('PROXY socks5 %s:%d' % (host, port))
                if mode == 'refuse':
                    s.sendall(b'\x05\x05\x00\x01\x00\x00\x00\x00\x00\x00')
                    return s.close()
                up = socket.create_connection(('127.0.0.1', self.srv.port))
                s.sendall(b'\x05\x00\x00\x01\x7f\x00\x00\x01' + self.port.to_bytes(2, 'big'))
            else:
                buf = first
                while not buf.endswith(b'\r\n\r\n'):
                    buf += recv_n(s, 1)
                m = re.match(rb'^CONNECT (\S+) HTTP/1\.[01]\r\n', buf)
                self.srv.log('PROXY http %s' % (m.group(1).decode('ascii', 'replace') if m else '?'))
                if not m or mode == 'refuse':
                    s.sendall(b'HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n')
                    return s.close()
                up = socket.create_connection(('127.0.0.1', self.srv.port))
                s.sendall(b'HTTP/1.1 200 Connection established\r\n\r\n')
            t = threading.Thread(target=pipe, args=(up, s), daemon=True)
            t.start()
            pipe(s, up)
            t.join(5)
            up.close()
        except OSError:
            pass
        s.close()


def serve(d):
    """起假 Gmail 和代理（regress 用），一直跑到被 kill"""
    import signal
    if not os.path.exists(os.path.join(d, 'pki', 'cert.pem')):
        make_pki(os.path.join(d, 'pki'))
    for key, _, _ in FOLDERS:
        os.makedirs(os.path.join(d, 'box', key), exist_ok=True)
    srv = Server(d)
    srv.start()
    px = Proxy(srv)
    px.start()
    tmp = os.path.join(d, 'ports.json.tmp')
    with open(tmp, 'w') as f:
        json.dump({'imap': srv.port, 'proxy': px.port}, f)
    os.replace(tmp, os.path.join(d, 'ports.json'))
    signal.signal(signal.SIGTERM, lambda *_: os._exit(0))
    while True:
        time.sleep(3600)


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest='cmd')
    s = sub.add_parser('serve')
    s.add_argument('dir')
    e = sub.add_parser('eml')
    e.add_argument('path')
    e.add_argument('--to', required=True)
    e.add_argument('--link')
    e.add_argument('--subject', default='Your secure link to Claude is here')
    e.add_argument('--sign', default='from:to:subject:date:message-id:mime-version:content-type')
    e.add_argument('--ar', default='pass')
    e.add_argument('--from', dest='frm', default='Anthropic <no-reply@mail.anthropic.com>')
    e.add_argument('--dkim-d', default='mail.anthropic.com')
    e.add_argument('--at', type=float)
    e.add_argument('--msgid')
    a = ap.parse_args()
    if a.cmd == 'serve':
        serve(a.dir)
    elif a.cmd == 'eml':
        put_eml(a.path, build_eml(a.to, a.link, a.subject, a.frm, a.sign, a.ar, a.dkim_d, a.msgid), a.at)
    else:
        ap.print_help()
        sys.exit(2)


if __name__ == '__main__':
    main()
