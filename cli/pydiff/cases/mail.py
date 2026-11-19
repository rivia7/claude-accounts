"""邮箱助手（cli/src/cmd/mail.rs 和 mail/ 下各份）：mail-setup / status / ready / get-user / forget / trust-forwarder / test / open / watch /
link-check、gui-mail-list。

真的 Gmail、真的钥匙串、真的浏览器都绝不能碰，这里全是假的：

- 假 Gmail 加假代理（下面的 Fake）：在跑对拍的这个进程里起一条线程，只听 127.0.0.1 上的一个端口。它先当代理（SOCKS5 和 HTTP
  CONNECT 都认，要连哪个主机都接到自己身上），再在同一个连接上做真的 TLS（证书是当场用 openssl 生成的一次性 CA 签的
  imap.gmail.com，cca 经 CCA_MAIL_CAFILE 信任它：这个变量只在 HOME 换过、对方是本机时才算数），然后说 IMAP。
  邮箱、开关、日志都在沙箱里：~/gmail/box/<all|spam|inbox>/<UID>.eml（Gmail 收到的时间取文件的修改时间）、~/gmail/ctl.json、
  ~/gmail/imap.log。收到的每条命令都记进日志（tag 开头的四个随机字母换成 TAG，SINCE 的日期写成离今天几天；LOGIN 那一行的假密码
  照记），对拍连沙箱里的文件一起比，所以两边发给 Gmail 的每个字节、先后和次数差一点都看得出来。
- 连接方式一律写成经这个假代理（socks5:// 或 http://127.0.0.1:端口，或者假的 scutil 说系统代理是它）。直连那条路会去连真的
  imap.gmail.com，只在不联网的命令里出现（mail-status、mail-ready 只是把路算出来）。
- 钥匙串是 cases/secrets.py 的假 security；打开链接的 cca _mail-open 是沙箱里 ~/.cca/bin/cca 这个假脚本（收到的链接记在
  ~/opened.log，看得到的 CCA_ 变量记在 ~/open-env.log，退出码照 ~/open.rc）。
- 密码是 abcdefghijklmnop，邮箱都在 example.com，链接是编的。

邮件的时间是登记夹具时定死的（「刚才」、「两小时前」、「一会儿以后」），两边看到的是同一批文件：先跑的 Python 和后跑的 Rust 隔了
几十秒也一样。命令跑着的时候才来的邮件先放在 ~/gmail/stash/ 里，during 里挪进邮箱。

ctl.json 的开关（每条命令都重新读）：user、password、imap_disabled、login_no（LOGIN 回 NO 和这段话）、no_all（没有 \\All）、
no_uidvalidity、no_uidnext（EXAMINE 的回应里不给这一项）、
uid_after_literal（FETCH 的 UID 写在 literal 后面）、search_blind（SEARCH 什么都搜不到：Gmail 的索引还没跟上）、greet（欢迎的那一行
换成这个；空串是不说话）、proxy（refuse | auth | close | garbage）、tls（badcert：别的 CA 签的证书 | wrongname：给别的域名的证书 |
plain：不说 TLS | close：直接断开）、faults（[{on: 命令, has: 这一行里要有的字, do: drop | bad | no | bye | stall | reply, text, times,
skip, close}]：第 skip+1 次起、一共 times 次，照 do 做；次数记在 ~/gmail/state.json）。

等邮件的用例要两边看邮箱的次数一样才比得上：看一次邮箱的间隔（CCA_MAIL_POLL）调得比「during 里放邮件、发信号」的那点时间长得多，
让该发生的事都落在两次之间。"""
import atexit
import calendar
import hashlib
import json
import os
import re
import shutil
import signal
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time

from run import REPO, case
from cases.secrets import SECURITY, ident

sys.path.insert(0, os.path.join(REPO, 'tests'))
import mail_fake as mf   # noqa: E402  仓库里回归用的那份替身：证书、登录邮件、IMAP 的几个小函数从它拿

PW = 'abcdefghijklmnop'
USER = 'inbox@example.com'
GREETING = '* OK Gimap ready for requests from 127.0.0.1 x1mb12345\r\n'
CAPA = ('* CAPABILITY IMAP4rev1 UNSELECT IDLE NAMESPACE QUOTA ID XLIST CHILDREN X-GM-EXT-1 XYZZY SASL-IR AUTH=XOAUTH2 AUTH=PLAIN '
        'AUTH=PLAIN-CLIENTTOKEN AUTH=OAUTHBEARER\r\n')


def recv_n(s, n):
    buf = b''
    while len(buf) < n:
        b = s.recv(n - len(buf))
        if not b:
            raise OSError('closed')
        buf += b
    return buf


def drain(s):
    """什么都不回，等对方自己走（超时、断开）"""
    try:
        while s.recv(65536):
            pass
    except OSError:
        pass


class Fake(threading.Thread):
    """假代理 + 假 Gmail（见文件开头）。home 是当前用例的沙箱，arm() 里指过来"""

    def __init__(self):
        threading.Thread.__init__(self, daemon=True)
        self.home = None
        self.pki = None
        self.ctxs = {}
        self.lk = threading.Lock()
        self.ls = socket.socket()
        self.ls.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.ls.bind(('127.0.0.1', 0))
        self.ls.listen(64)
        self.port = self.ls.getsockname()[1]
        # 连得上、但永远没人理的端口（握手一直等到超时）
        self.mute = socket.socket()
        self.mute.bind(('127.0.0.1', 0))
        self.mute.listen(64)
        self.mute_port = self.mute.getsockname()[1]

    # --- 证书：这一趟对拍用的一次性 CA（good）、另一个 CA（bad）、good 签给别的域名的（wrongname）
    def ensure_pki(self):
        if self.pki:
            return
        self.pki = tempfile.mkdtemp(prefix='cca-pydiff-mail-pki.')
        atexit.register(shutil.rmtree, self.pki, True)
        good, bad = os.path.join(self.pki, 'good'), os.path.join(self.pki, 'bad')
        mf.make_pki(good)
        mf.make_pki(bad)
        q = dict(stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
        p = lambda n: os.path.join(good, n)
        ext = p('other.cnf')
        with open(ext, 'w') as f:
            f.write('[leaf]\nbasicConstraints = critical, CA:FALSE\nkeyUsage = critical, digitalSignature\nextendedKeyUsage = serverAuth\n'
                    'subjectAltName = DNS:imap.example.com\nsubjectKeyIdentifier = hash\nauthorityKeyIdentifier = keyid:always\n')
        subprocess.run(['openssl', 'req', '-new', '-key', p('key.pem'), '-subj', '/CN=imap.example.com', '-out', p('other.csr')], **q)
        subprocess.run(['openssl', 'x509', '-req', '-in', p('other.csr'), '-CA', p('ca.pem'), '-CAkey', p('ca.key'), '-CAcreateserial',
                        '-days', '2', '-extfile', ext, '-extensions', 'leaf', '-out', p('other.pem')], **q)
        shutil.copy(p('ca.pem'), CA_FILE)

    def context(self, kind):
        if kind not in self.ctxs:
            cert, key = {'good': ('good/cert.pem', 'good/key.pem'), 'bad': ('bad/cert.pem', 'bad/key.pem'),
                         'wrongname': ('good/other.pem', 'good/key.pem')}[kind]
            ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            ctx.load_cert_chain(os.path.join(self.pki, cert), os.path.join(self.pki, key))
            self.ctxs[kind] = ctx
        return self.ctxs[kind]

    # --- 沙箱里的状态
    def path(self, *rel):
        return os.path.join(self.home, 'gmail', *rel)

    def log(self, line):
        with self.lk:
            try:
                with open(self.path('imap.log'), 'a', encoding='utf-8') as f:
                    f.write(line + '\n')
            except OSError:
                pass

    def ctl(self):
        try:
            with open(self.path('ctl.json'), encoding='utf-8') as f:
                c = json.load(f)
        except (OSError, ValueError):
            c = {}
        c.setdefault('user', USER)
        c.setdefault('password', PW)
        return c

    def fault(self, c, verb, line):
        """这条命令要不要出岔子（ctl.json 的 faults），次数记在 state.json"""
        for i, f in enumerate(c.get('faults') or []):
            if f.get('on') != verb or (f.get('has') and f['has'] not in line):
                continue
            with self.lk:
                try:
                    with open(self.path('state.json')) as fh:
                        st = json.load(fh)
                except (OSError, ValueError):
                    st = {}
                n = st.get(str(i), 0)
                st[str(i)] = n + 1
                with open(self.path('state.json'), 'w') as fh:
                    json.dump(st, fh)
            if n >= f.get('skip', 0) and ('times' not in f or n < f.get('skip', 0) + f['times']):
                return f
        return None

    def run(self):
        while True:
            try:
                s, _ = self.ls.accept()
            except OSError:
                return
            threading.Thread(target=self.serve, args=(s,), daemon=True).start()

    # --- 代理
    def serve(self, s):
        c = self.ctl()
        mode = c.get('proxy')
        try:
            s.settimeout(120)
            first = recv_n(s, 1)
            if first == b'\x05':
                methods = recv_n(s, recv_n(s, 1)[0])
                if mode == 'auth':
                    self.log('PROXY socks5 要认证（它说能用的办法 %s）' % methods.hex())
                    return s.sendall(b'\x05\xff')
                s.sendall(b'\x05\x00')
                ver, cmd, rsv, atyp = recv_n(s, 4)
                if atyp == 3:
                    host = recv_n(s, recv_n(s, 1)[0]).decode('latin-1')
                elif atyp == 1:
                    host = socket.inet_ntoa(recv_n(s, 4))
                else:
                    host = socket.inet_ntop(socket.AF_INET6, recv_n(s, 16))
                port = int.from_bytes(recv_n(s, 2), 'big')
                self.log('PROXY socks5 办法=%s 版本=%d 命令=%d 保留=%d 地址类型=%d %s:%d' % (methods.hex(), ver, cmd, rsv, atyp, host, port))
                if mode == 'refuse':
                    return s.sendall(b'\x05\x05\x00\x01\x00\x00\x00\x00\x00\x00')
                if mode == 'close':
                    return None
                if mode == 'garbage':
                    return s.sendall(b'\x05\x00\x00\x09')
                s.sendall(b'\x05\x00\x00\x01\x7f\x00\x00\x01' + self.port.to_bytes(2, 'big'))
            else:
                buf = first
                while not buf.endswith(b'\r\n\r\n'):
                    buf += recv_n(s, 1)
                self.log('PROXY http %r' % buf.decode('latin-1'))
                if mode == 'refuse':
                    return s.sendall(b'HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n')
                if mode == 'close':
                    return None
                if mode == 'garbage':
                    return s.sendall(b'ICY 200 OK\x01\xff' + b'x' * 100 + b'\r\n\r\n')
                s.sendall(b'HTTP/1.1 200 Connection established\r\n\r\n')
            self.tls(s, c)
        except OSError:
            pass
        finally:
            try:
                s.close()
            except OSError:
                pass

    # --- TLS
    def tls(self, s, c):
        mode = c.get('tls')
        if mode == 'close':
            return
        if mode == 'plain':
            s.sendall(b'* OK this is not TLS\r\n')
            return drain(s)
        try:
            t = self.context({'badcert': 'bad', 'wrongname': 'wrongname'}.get(mode, 'good')).wrap_socket(s, server_side=True)
        except (OSError, ssl.SSLError):
            return   # 它不认这张证书（握手没成的不记日志：它什么时候走的说不准）
        f = t.makefile('rwb')
        try:
            self.imap(f, c)
        except (OSError, ssl.SSLError, ValueError):
            pass
        finally:
            for x in (f, t):
                try:
                    x.close()
                except (OSError, ssl.SSLError):
                    pass

    # --- IMAP（照 tests/mail_fake.py 的 Conn，加上出岔子的开关）
    def imap(self, f, c):
        def send(x):
            f.write(x if isinstance(x, bytes) else x.encode('utf-8'))
            f.flush()

        greet = c.get('greet', GREETING)
        if greet:
            send(greet)
        box = mf.Box(os.path.join(self.home, 'gmail'))
        state = {'folder': None, 'authed': False}
        while True:
            line = f.readline()
            if not line:
                return
            line = line.rstrip(b'\r\n').decode('utf-8', 'replace')
            if not self.command(line, send, box, state):
                return

    def command(self, line, send, box, state):
        c = self.ctl()
        tag, _, rest = line.partition(' ')
        verb, _, args = rest.partition(' ')
        verb = verb.upper()
        if verb == 'UID':
            sub, _, args = args.partition(' ')
            verb = 'UID ' + sub.upper()
        today = int(time.time()) // 86400

        def days(m):
            return 'SINCE <今天%+d天>' % (calendar.timegm(time.strptime(m.group(1), '%d-%b-%Y')) // 86400 - today)
        self.log('IMAP %s %s' % (re.sub(r'^[A-P]{4}', 'TAG', tag), re.sub(r'SINCE (\d{1,2}-[A-Za-z]{3}-\d{4})', days, rest)))
        if verb in mf.WRITES or verb.split(' ')[-1] in mf.WRITES:
            self.log('VIOLATION %s %s' % (verb, args))
            send('%s BAD Command not allowed in the test\r\n' % tag)
            return True
        f = self.fault(c, verb, line)
        if f:
            do, text = f.get('do'), f.get('text', '')
            if do == 'drop':
                return False
            if do == 'stall':
                return True
            if do == 'bad':
                send('%s BAD %s\r\n' % (tag, text or 'Could not parse command'))
                return True
            if do == 'no':
                send('%s NO %s\r\n' % (tag, text or '[UNAVAILABLE] Temporary System Error'))
                return True
            if do == 'bye':
                send('* BYE %s\r\n' % (text or 'Session expired, please login again.'))
                return False
            if do == 'reply':   # 照这段话回（close：回完就断开）
                send(text.replace('{TAG}', tag))
                return not f.get('close')
        if verb == 'CAPABILITY':
            send('%s%s OK Thats all she wrote!\r\n' % (CAPA, tag))
        elif verb == 'LOGIN':
            parts = mf.tokens(args)
            val = lambda x: x[1] if isinstance(x, tuple) else x
            user, pw = val(parts[0]) if parts else '', val(parts[1]) if len(parts) > 1 else ''
            if c.get('imap_disabled'):
                send('%s NO [ALERT] Your account is not enabled for IMAP use. Please visit your Gmail settings page '
                     'and enable your account for IMAP access. (Failure)\r\n' % tag)
            elif c.get('login_no'):
                send('%s NO %s\r\n' % (tag, c['login_no']))
            elif user == c['user'] and pw == c['password']:
                state['authed'] = True
                send('* CAPABILITY IMAP4rev1 UNSELECT IDLE NAMESPACE QUOTA ID XLIST CHILDREN X-GM-EXT-1 UIDPLUS COMPRESS=DEFLATE ENABLE MOVE '
                     'CONDSTORE ESEARCH UTF8=ACCEPT LIST-EXTENDED LIST-STATUS LITERAL- SPECIAL-USE APPENDLIMIT=35651584\r\n'
                     '%s OK %s authenticated (Success)\r\n' % (tag, user))
            else:
                send('%s NO [AUTHENTICATIONFAILED] Invalid credentials (Failure)\r\n' % tag)
        elif verb == 'LOGOUT':
            send('* BYE LOGOUT Requested\r\n%s OK 73 good day (Success)\r\n' % tag)
            return False
        elif not state['authed']:
            send('%s BAD Command not valid in this state\r\n' % tag)
        elif verb == 'LIST':
            for key, name, attrs in mf.FOLDERS:
                if key == 'all' and c.get('no_all'):
                    continue
                send('* LIST (%s) "/" "%s"\r\n' % (attrs, name))
            send('* LIST (\\HasChildren \\Noselect) "/" "[Gmail]"\r\n* LIST (\\HasNoChildren \\Sent) "/" "[Gmail]/Sent Mail"\r\n'
                 '%s OK Success\r\n' % tag)
        elif verb == 'EXAMINE':
            key = box.folder_dir(args.strip())
            if key is None or (key == 'all' and c.get('no_all')):
                send('%s NO [NONEXISTENT] Unknown Mailbox: %s (Failure)\r\n' % (tag, args))
                return True
            state['folder'] = key
            ms = box.msgs(key)
            nxt = max([u for u, _, _ in ms] + [0]) + 1
            valid = '' if c.get('no_uidvalidity') else '* OK [UIDVALIDITY %d] UIDs valid.\r\n' % box.uidvalidity(key)
            unext = '' if c.get('no_uidnext') else '* OK [UIDNEXT %d] Predicted next UID.\r\n' % nxt
            send('* FLAGS (\\Answered \\Flagged \\Draft \\Deleted \\Seen $NotPhishing $Phishing)\r\n'
                 '* OK [PERMANENTFLAGS ()] Flags permitted.\r\n%s* %d EXISTS\r\n'
                 '* 0 RECENT\r\n%s* OK [HIGHESTMODSEQ 1234]\r\n'
                 '%s OK [READ-ONLY] %s selected. (Success)\r\n' % (valid, len(ms), unext, tag, args))
        elif verb == 'UID SEARCH':
            if not state['folder']:
                send('%s BAD No mailbox selected\r\n' % tag)
                return True
            try:
                toks = mf.tokens(args)
                hits = [] if c.get('search_blind') else [m[0] for m in box.msgs(state['folder']) if mf.search_eval(toks, m)]
            except (ValueError, StopIteration, AttributeError) as e:
                send('%s BAD Could not parse command (%s)\r\n' % (tag, e))
                return True
            send('* SEARCH%s\r\n%s OK SEARCH completed (Success)\r\n' % (''.join(' %d' % u for u in hits), tag))
        elif verb == 'UID FETCH':
            if not state['folder']:
                send('%s BAD No mailbox selected\r\n' % tag)
                return True
            spec, _, items = args.partition(' ')
            up = items.upper()
            if re.search(r'\bBODY\[', up) or re.search(r'\bRFC822(?![.](HEADER|SIZE))\b', up):
                self.log('VIOLATION UID FETCH %s' % items)   # 不带 PEEK：会标已读
                send('%s BAD Would mark as read\r\n' % tag)
                return True
            ms = box.msgs(state['folder'])
            try:
                want = mf.uid_set(spec, [u for u, _, _ in ms])
            except ValueError:
                send('%s BAD Could not parse command\r\n' % tag)
                return True
            hf = re.search(r'BODY\.PEEK\[HEADER\.FIELDS \(([^)]*)\)\]', items, re.I)
            part = re.search(r'BODY\.PEEK\[\]<0\.(\d+)>', items, re.I)
            for seq, (u, t, raw) in enumerate(ms, 1):
                if u not in want:
                    continue
                if hf:
                    data, label = mf.header_fields(raw, {n.upper() for n in hf.group(1).split()}), 'BODY[HEADER.FIELDS (%s)]' % hf.group(1)
                elif part:
                    try:   # <UID>.full：取全文时给这一份（头和全文不是一回事的邮件）
                        with open(self.path('box', state['folder'], '%d.full' % u), 'rb') as fh:
                            raw = fh.read()
                    except OSError:
                        pass
                    data, label = raw[:int(part.group(1))], 'BODY[]<0>'
                else:
                    data, label = b'', None
                if label is None:
                    send('* %d FETCH (UID %d INTERNALDATE "%s")\r\n' % (seq, u, mf.idate(t)))
                elif c.get('uid_after_literal'):
                    send(('* %d FETCH (INTERNALDATE "%s" %s {%d}\r\n' % (seq, mf.idate(t), label, len(data))).encode() + data + (' UID %d)\r\n' % u).encode())
                else:
                    send(('* %d FETCH (UID %d INTERNALDATE "%s" %s {%d}\r\n' % (seq, u, mf.idate(t), label, len(data))).encode() + data + b')\r\n')
            send('%s OK Success\r\n' % tag)
        else:
            self.log('VIOLATION %s %s' % (verb, args))
            send('%s BAD Unknown command\r\n' % tag)
        return True


FAKE = Fake()
FAKE.start()
PORT = FAKE.port
SOCKS, HTTP = 'socks5://127.0.0.1:%d' % PORT, 'http://127.0.0.1:%d' % PORT
REFUSED = 'socks5://127.0.0.1:9'                    # 没人听的端口：连不上
MUTE = 'socks5://127.0.0.1:%d' % FAKE.mute_port     # 连得上、没人理：等到超时
CA_FILE = os.path.join(tempfile.gettempdir(), 'cca-pydiff-mail-ca-%d.pem' % os.getpid())
atexit.register(lambda: os.path.exists(CA_FILE) and os.remove(CA_FILE))
ENV = {'CCA_MAIL_CAFILE': CA_FILE, 'CCA_MAIL_POLL': '0.3', 'CCA_MAIL_RETRY': '0.2'}
FILE = {'CCA_KEYSTORE': 'file'}

# 每次不一样的：记下的时间、进程号、界面要的生成时间；TLS 握手出错时括号里那一小段是各自的 TLS 库的说法（原来是 OpenSSL 的，
# cca-core 是 rustls 的），只比前后的字。证书校验没通过那一句后半句原来说“这个 Python 缺根证书”，cca-core 改口说系统的根证书
MASK = [(r'"at": [\d.]+', '"at": <时间>'), (r'"pid": \d+', '"pid": <进程号>'), (r'"generated_at": \d+', '"generated_at": <时间>'),
        (r'(Gmail 的证书校验没通过|和 Gmail 的 TLS 握手没成)（[^）]*）', r'\1（<TLS 库的说法>）'),
        (r'或者(这个 Python 缺根证书|系统的根证书（/etc/ssl/cert.pem）不全)。', '或者<缺根证书>。')]

A, B = '主号(alice@example.com)', 'bob@example.com'
ALICE, BOB = ident('alice@example.com'), ident('bob@example.com')
TABLE = {A: 'US-A', B: 'US-B', '直连': {'base_url': 'https://api.example.com/anthropic'}, '只有别名': 'US-A'}
CODEX = {'erin@example.com': 'US-A', '主号(alice@example.com)': 'US-B'}
MAIL = {'user': USER, 'proxy': SOCKS}
LINK = 'https://claude.ai/magic-link#tok-%s:YWxpY2VAZXhhbXBsZS5jb20'

# 假的 cca：邮箱助手只会起「cca _mail-open 账号」（链接从 stdin 来）
OPENER = r'''#!/bin/sh
[ "$1" = _mail-open ] || { printf 'cca %s\n' "$*" >> "$HOME/cca-other.log"; exit 9; }
IFS= read -r url
printf '%s|%s|参数 %d 个\n' "$2" "$url" "$#" >> "$HOME/opened.log"
env | grep '^CCA_' | sort > "$HOME/open-env.log"
[ -f "$HOME/open.sleep" ] && sleep "$(cat "$HOME/open.sleep")"
rc=0
if [ -s "$HOME/open.rc" ]; then
	rc=$(head -n 1 "$HOME/open.rc")
	tail -n +2 "$HOME/open.rc" > "$HOME/open.rc.new"; mv "$HOME/open.rc.new" "$HOME/open.rc"
fi
if [ "$rc" = 0 ]; then echo "✓ 假的 cca：在 $2 专属的浏览器里打开了" >&2; else echo "! 假的 cca：没打开（退出码 ${rc}）" >&2; fi
exit "$rc"
'''


def scutil(*lines):
    """假的 scutil --proxy：照这几行说（每次被调都记一笔）"""
    return ('printf \'scutil %%s\\n\' "$*" >> "$HOME/scutil.log"\n[ "$1" = --proxy ] || exit 64\ncat <<\'EOF\'\n<dictionary> {\n%s\n}\nEOF\n'
            % '\n'.join('  ' + l for l in lines))


SYS_SOCKS = scutil('ExceptionsList : <array> {', '  0 : *.local', '  1 : 169.254/16', '}', 'HTTPEnable : 0', 'SOCKSEnable : 1',
                   'SOCKSPort : %d' % PORT, 'SOCKSProxy : 127.0.0.1')
SYS_HTTPS = scutil('HTTPEnable : 1', 'HTTPPort : 1', 'HTTPProxy : 192.0.2.99', 'HTTPSEnable : 1', 'HTTPSPort : %d' % PORT, 'HTTPSProxy : 127.0.0.1')
SYS_NONE = scutil('FTPPassive : 1')
SYS_BYPASS = scutil('ExceptionsList : <array> {', '  0 : *.gmail.com', '}', 'SOCKSEnable : 1', 'SOCKSPort : %d' % PORT, 'SOCKSProxy : 127.0.0.1')


def world(b, mail=MAIL, table=None, cfg=None, pw=PW, codex=None, agy=None, sysproxy=SYS_SOCKS, keyfile=False):
    """一套夹具：config.json（mail 一节照给的）、账号表、假钥匙串里的应用专用密码、假的 cca、指好了沙箱的假 Gmail。
    假的 scutil 默认说系统代理就是假代理：哪个用例不小心走了「照系统代理」，也连不到真的 Gmail 上去"""
    c = {'version': 2, 'router': {'host': '192.0.2.10', 'port': 22, 'user': 'root', 'key': '~/.cca/router_ed25519'},
         'proxy_host': '192.0.2.10', 'nodes': ['US-A', 'US-B'], 'min_trust': 50}
    if mail is not None:
        c['mail'] = json.loads(json.dumps(mail))
    c.update(cfg or {})
    b.cfg(c)
    b.table(TABLE if table is None else table)
    if codex is not None:
        b.table(codex, 'codex')
    if agy is not None:
        b.table(agy, 'agy')
    b.fake('security', SECURITY)
    if sysproxy is not None:
        b.fake('scutil', sysproxy)
    if pw:
        store(b, USER, pw, keyfile)
    b.write('.cca/bin/cca', OPENER, 0o755)
    arm(b)


def store(b, user, pw=PW, keyfile=False):
    """钥匙串（或者密钥文件）里这个 Gmail 的应用专用密码"""
    if keyfile:
        p = b.write('.cca/secrets/cca-mail', json.dumps({'host': user, 'key': pw}) + '\n')
        os.chmod(os.path.dirname(p), 0o700)
    else:
        b.write('keychain/cca-mail/%s' % user, pw)


def arm(b):
    """假 Gmail 指到这个沙箱"""
    FAKE.ensure_pki()
    FAKE.home = b.home
    for key, _, _ in mf.FOLDERS:
        os.makedirs(b.path('gmail/box/' + key), exist_ok=True)


def ctl(b, **kw):
    b.json('gmail/ctl.json', kw)


def eml(to='alice@example.com', tok='A1', **kw):
    kw.setdefault('link', LINK % tok)
    return mf.build_eml(to, **kw)


def put(b, uid, data, ago=120, folder='all', full=None, stash=False):
    """往邮箱里放一封：ago 秒以前收到的（负的是一会儿以后）。stash：先放在一边，during 里再挪进去"""
    at = time.time() - ago
    mf.put_eml(b.path('gmail/%s/%s/%d.eml' % ('stash' if stash else 'box', folder, uid)), data, at)
    if full is not None:
        mf.put_eml(b.path('gmail/box/%s/%d.full' % (folder, uid)), full, at)


def log_has(b, pattern, n=1):
    try:
        with open(b.path('gmail/imap.log'), encoding='utf-8') as f:
            return len(re.findall(pattern, f.read())) >= n
    except OSError:
        return False


# during（命令跑着的时候另一条线程里做的事：放邮件、发信号）整个最多 9 秒：run.py 只等这条线程 10 秒，等不到就往下跑。
# 过了这个时间它什么都不再做，不然上一遍留下的线程会把信号发给下一遍的命令
SIDE = threading.local()


def in_time():
    return time.time() < getattr(SIDE, 'until', float('inf'))


def timed(fn):
    def during(b):
        SIDE.until = time.time() + 9
        fn(b)
    return during


def wait_for(cond):
    while in_time():
        if cond():
            return True
        time.sleep(0.02)
    return False


def deliver(b):
    """stash 里的邮件挪进邮箱（改名：收到的时间不变）"""
    root = b.path('gmail/stash')
    if not in_time():
        return
    for folder in sorted(os.listdir(root)):
        for n in sorted(os.listdir(os.path.join(root, folder))):
            os.replace(os.path.join(root, folder, n), b.path('gmail/box/%s/%s' % (folder, n)))


def signal_child(sig):
    """给正在跑的那个命令（对拍这个进程的子进程）发信号"""
    if not in_time():
        return
    for pid in subprocess.run(['pgrep', '-P', str(os.getpid())], capture_output=True, text=True).stdout.split():
        try:
            os.kill(int(pid), sig)
        except OSError:
            pass


def mcase(name, argv, **kw):
    """和 run.case 一样，带上邮箱助手的环境变量和要遮掉的那几样"""
    kw['env'] = dict(ENV, **(kw.get('env') or {}))
    kw['mask'] = MASK + (kw.get('mask') or [])
    if kw.get('during'):
        kw['during'] = timed(kw['during'])
    return case('mail/' + name, argv, **kw)


# --- mail-link-check -------------------------------------------------------------------------------------------------
for name, data in [
    ('ok', 'https://claude.ai/magic-link#tok-abc:YWxpY2U\n'), ('ok-crlf-padded', '  https://claude.ai/magic-link?x=1#t \r\n'),
    ('ok-no-newline', 'https://claude.ai/magic-link#t'), ('second-line-ignored', 'https://claude.ai/magic-link#t\nhttps://evil.example.com/\n'),
    ('first-line-bad', 'https://evil.example.com/\nhttps://claude.ai/magic-link#t\n'), ('empty', ''), ('blank-line', '\n'),
    ('http', 'http://claude.ai/magic-link#t\n'), ('other-host', 'https://claude.ai.evil.example.com/magic-link#t\n'),
    ('userinfo', 'https://claude.ai@evil.example.com/magic-link#t\n'), ('port', 'https://claude.ai:443/magic-link#t\n'),
    ('upper-host', 'https://Claude.ai/magic-link#t\n'), ('other-path', 'https://claude.ai/login#t\n'), ('path-suffix', 'https://claude.ai/magic-linkx#t\n'),
    ('quote', 'https://claude.ai/magic-link#t"x\n'), ('space-inside', 'https://claude.ai/magic-link# t\n'), ('chinese', 'https://claude.ai/magic-link#中\n'),
    ('not-utf8', b'https://claude.ai/magic-link#\xff\n'), ('too-long', 'https://claude.ai/magic-link#' + 'a' * 2048 + '\n'),
    ('longest', 'https://claude.ai/magic-link#' + 'a' * 2019 + '\n'), ('nul', 'https://claude.ai/magic-link#a\x00b\n'),
    ('bracket-host', 'https://[claude.ai]/magic-link\n'),
]:
    @mcase('link-check/' + name, ['mail-link-check'], stdin=data)
    def _(b):
        pass


@mcase('link-check/codex-has-no-rules', ['mail-link-check'], stdin='https://claude.ai/magic-link#t\n', tool='codex')
def _(b):
    pass


@mcase('link-check/agy-has-no-rules', ['mail-link-check'], stdin='https://claude.ai/magic-link#t\n', tool='agy')
def _(b):
    pass


@mcase('link-check/extra-arg', ['mail-link-check', 'x'], stdin='https://claude.ai/magic-link#t\n')
def _(b):
    pass


# --- mail-get-user、mail-ready ---------------------------------------------------------------------------------------
for name, mail in [
    ('configured', MAIL), ('none', None), ('upper', {'user': 'InBox@Example.COM'}), ('bad-address', {'user': 'not an address'}),
    ('user-not-string', {'user': 5}), ('mail-not-dict', 'inbox@example.com'), ('only-overrides', {'accounts': {ALICE: {'user': 'a2@example.com'}}}),
    ('trailing-newline', {'user': 'inbox@example.com\n'}),
]:
    @mcase('get-user/' + name, ['mail-get-user'])
    def _(b, mail=mail):
        world(b, mail=mail)

    @mcase('get-user/codex-' + name, ['mail-get-user'], tool='codex')
    def _(b, mail=mail):
        world(b, mail=mail, codex=CODEX)


@mcase('get-user/extra-arg', ['mail-get-user', 'x'])
def _(b):
    world(b)


@mcase('get-user/agy', ['mail-get-user'], tool='agy')
def _(b):
    world(b, agy={'gina@example.com': 'US-A'})


@mcase('get-user/broken-config', ['mail-get-user'])
def _(b):
    world(b)
    b.write('.cca/config.json', '{oops')


OVERRIDES = {'user': USER, 'proxy': SOCKS, 'accounts': {ALICE: {'user': 'alice-box@example.com'}, BOB: {'forwarder': 'Relay.Example.com'}}}
for name, argv, kw in [
    ('default', ['mail-ready'], {}), ('empty-key', ['mail-ready', ''], {}), ('not-configured', ['mail-ready'], dict(mail=None)),
    ('no-password', ['mail-ready'], dict(pw=None)), ('bad-route', ['mail-ready'], dict(mail={'user': USER, 'proxy': 'ftp://127.0.0.1:1'})),
    ('route-port-0', ['mail-ready'], dict(mail={'user': USER, 'proxy': 'socks5://127.0.0.1:0'})),
    ('route-port-65536', ['mail-ready'], dict(mail={'user': USER, 'proxy': 'http://127.0.0.1:65536'})),
    ('direct', ['mail-ready'], dict(mail={'user': USER, 'proxy': 'direct'})),
    ('system-no-proxy', ['mail-ready'], dict(mail={'user': USER}, sysproxy=SYS_NONE)),
    ('system-scutil-fails', ['mail-ready'], dict(mail={'user': USER}, sysproxy='exit 1\n')),
    ('override-needs-its-own-password', ['mail-ready', A], dict(mail=OVERRIDES)),
    ('override-falls-back-to-default', ['mail-ready', B], dict(mail=OVERRIDES)),
    ('key-not-in-table', ['mail-ready', 'nobody@example.com'], dict(mail=OVERRIDES)),
    ('exit-port-local', ['mail-ready'], dict(mail={'user': USER, 'proxy': 'socks5://127.0.0.1:7901'}, cfg={'ports': {A: 7901}})),
    ('exit-port-router', ['mail-ready'], dict(mail={'user': USER, 'proxy': 'http://192.0.2.10:7902'}, cfg={'ports': {A: 7901, B: 7902}})),
    ('exit-port-other-host-is-fine', ['mail-ready'], dict(mail={'user': USER, 'proxy': 'http://192.0.2.77:7902'}, cfg={'ports': {A: 7901, B: 7902}})),
    ('exit-port-codex-pool', ['mail-ready'], dict(mail={'user': USER, 'proxy': 'socks5://localhost:7950'}, cfg={'codex': {'ports': {'erin@example.com': 7950}}})),
    ('ports-not-a-dict', ['mail-ready'], dict(mail={'user': USER, 'proxy': 'socks5://127.0.0.1:7901'}, cfg={'ports': [7901]})),
]:
    @mcase('ready/' + name, argv)
    def _(b, kw=kw):
        world(b, **kw)


@mcase('ready/exit-is-this-runs-proxy', ['mail-ready'], env={'CCA_PROXY': 'http://127.0.0.1:%d' % PORT})
def _(b):
    world(b)


@mcase('ready/keychain-unreadable', ['mail-ready'])
def _(b):
    world(b)
    b.write('keychain_unreadable', '')


@mcase('ready/keyfile', ['mail-ready'], env=FILE)
def _(b):
    world(b, keyfile=True)


@mcase('ready/keyfile-missing', ['mail-ready'], env=FILE)
def _(b):
    world(b, pw=None)


@mcase('ready/codex', ['mail-ready', 'erin@example.com'], tool='codex')
def _(b):
    world(b, codex=CODEX)


@mcase('ready/codex-override-by-codex-key', ['mail-ready', 'erin@example.com'], tool='codex')
def _(b):
    world(b, mail=dict(MAIL, accounts={ident('erin@example.com'): {'user': 'erin-box@example.com'}}), codex=CODEX)


@mcase('ready/agy', ['mail-ready', 'gina@example.com'], tool='agy')
def _(b):
    world(b, agy={'gina@example.com': 'US-A'})


@mcase('ready/agy-no-password', ['mail-ready'], tool='agy')
def _(b):
    world(b, pw=None, agy={'gina@example.com': 'US-A'})


@mcase('ready/too-many-args', ['mail-ready', A, 'x'])
def _(b):
    world(b)


# --- mail-status -----------------------------------------------------------------------------------------------------
LAST = {'at': 1794708000, 'name': '主号', 'text': '打开了登录邮件里的链接'}
for name, kw, last in [
    ('not-configured', dict(mail=None), None), ('mail-not-dict', dict(mail=['x']), None), ('default', {}, None),
    ('no-password', dict(pw=None), None), ('auto-off', dict(mail=dict(MAIL, auto=False)), None), ('auto-zero-is-on', dict(mail=dict(MAIL, auto=0)), None),
    ('http-proxy', dict(mail={'user': USER, 'proxy': HTTP}), None), ('direct', dict(mail={'user': USER, 'proxy': 'direct'}), None),
    ('system-socks', dict(mail={'user': USER}, sysproxy=SYS_SOCKS), None), ('system-https', dict(mail={'user': USER, 'proxy': 'system'}, sysproxy=SYS_HTTPS), None),
    ('system-none', dict(mail={'user': USER}, sysproxy=SYS_NONE), None), ('system-bypass', dict(mail={'user': USER}, sysproxy=SYS_BYPASS), None),
    ('bad-route', dict(mail={'user': USER, 'proxy': 'socks4://127.0.0.1:1'}), None),
    ('exit-route', dict(mail={'user': USER, 'proxy': 'socks5://127.0.0.1:7901'}, cfg={'ports': {A: 7901}}), None),
    ('overrides', dict(mail=OVERRIDES), None), ('only-overrides', dict(mail={'accounts': {ALICE: {'user': 'alice-box@example.com'}, 'zz-gone-000000': {'user': 'gone@example.com'}}}), None),
    ('overrides-not-dict', dict(mail=dict(MAIL, accounts=['x'])), None), ('override-bad-address', dict(mail=dict(MAIL, accounts={ALICE: {'user': 'x'}, BOB: 'y'})), None),
    ('last', {}, LAST), ('last-no-name', {}, {'at': 1794708000, 'text': 'x'}), ('last-name-number', {}, {'at': 1794708000, 'name': 5, 'text': 'x'}),
    ('last-at-float', {}, dict(LAST, at=1794708000.5)), ('last-at-true', {}, dict(LAST, at=True)), ('last-at-negative', {}, dict(LAST, at=-1)),
    ('last-text-not-string', {}, dict(LAST, text=None)), ('last-not-dict', {}, [1]), ('last-at-huge', {}, dict(LAST, at=10 ** 30)),
    ('last-control-chars', {}, dict(LAST, text='连不上 Gmail（x\x1b[2Jy）')), ('last-at-out-of-range', {}, dict(LAST, at=2 ** 62)),
    ('last-at-far-future', {}, dict(LAST, at=253402300800)), ('last-at-false', {}, dict(LAST, at=False)), ('last-name-empty', {}, dict(LAST, name='')),
    ('last-name-list', {}, dict(LAST, name=['x', 1])),
]:
    @mcase('status/' + name, ['mail-status'])
    def _(b, kw=kw, last=last):
        world(b, **kw)
        if last is not None:
            b.json('.cca/mail/last.json', last)
        if 'accounts' in (kw.get('mail') or {}) and isinstance(kw['mail']['accounts'], dict):
            store(b, 'alice-box@example.com')


@mcase('status/last-broken-json', ['mail-status'])
def _(b):
    world(b)
    b.write('.cca/mail/last.json', '{oops')


@mcase('status/keychain-unreadable', ['mail-status'])
def _(b):
    world(b, mail=OVERRIDES)
    b.write('keychain_unreadable', '')


@mcase('status/keyfile', ['mail-status'], env=FILE)
def _(b):
    world(b, mail=OVERRIDES, keyfile=True)


@mcase('status/codex-names-from-codex-table', ['mail-status'], tool='codex')
def _(b):
    world(b, mail=dict(MAIL, accounts={ident('erin@example.com'): {'user': 'erin-box@example.com'}, ALICE: {'user': 'alice-box@example.com'}}), codex=CODEX)


@mcase('status/agy', ['mail-status'], tool='agy')
def _(b):
    world(b, mail=OVERRIDES, agy={'gina@example.com': 'US-A'})


@mcase('status/table-not-dict', ['mail-status'])
def _(b):
    world(b, mail=OVERRIDES)
    b.write('.cca/accounts.json', '[1]\n')


@mcase('status/long-names', ['mail-status'])
def _(b):
    world(b, mail=dict(MAIL, accounts={ident('carol@example.com'): {'user': 'c@example.com'}, ident('dave@example.com'): {'user': 'd@example.com'}}),
          table={'一个很长很长的别名(carol@example.com)': 'US-A', 'dave@example.com': 'US-B'})


@mcase('status/extra-arg', ['mail-status', 'x'])
def _(b):
    world(b)


# --- mail-trust-forwarder --------------------------------------------------------------------------------------------
for name, argv, mail in [
    ('set', [A, 'relay.example.com'], MAIL), ('set-upper-padded', [A, '  Relay.Example.COM \n'], MAIL), ('set-no-mail-yet', [A, 'relay.example.com'], None),
    ('set-keeps-override', [A, 'relay.example.com'], OVERRIDES), ('replace', [B, 'other.example.com'], OVERRIDES), ('off', [B, 'off'], OVERRIDES),
    ('off-empty', [B, ''], OVERRIDES), ('off-nothing-there', [A, 'off'], MAIL), ('off-no-mail', [A, 'off'], None),
    ('off-last-one', [B, 'off'], {'accounts': {BOB: {'forwarder': 'relay.example.com'}}}), ('off-keeps-user', [A, 'off'], {'accounts': {ALICE: {'user': 'a@example.com', 'forwarder': 'r.example.com'}}}),
    ('off-padded-is-a-domain', [A, ' off'], MAIL), ('bad-at', [A, 'x@relay.example.com'], MAIL), ('bad-url', [A, 'http://relay.example.com'], MAIL),
    ('bad-port', [A, 'relay.example.com:25'], MAIL), ('bad-no-dot', [A, 'relay'], MAIL), ('bad-space', [A, 'relay example.com'], MAIL),
    ('mail-not-dict', [A, 'relay.example.com'], 'x'), ('mail-not-dict-off', [A, 'off'], 'x'), ('accounts-not-dict', [A, 'relay.example.com'], dict(MAIL, accounts=[1])),
    ('override-not-dict', [A, 'relay.example.com'], dict(MAIL, accounts={ALICE: 'x'})), ('api-account', ['直连', 'relay.example.com'], MAIL),
    ('alias-only', ['只有别名', 'relay.example.com'], MAIL), ('not-in-table', ['nobody@example.com', 'relay.example.com'], MAIL),
]:
    @mcase('trust-forwarder/' + name, ['mail-trust-forwarder'] + argv)
    def _(b, mail=mail):
        world(b, mail=mail)


@mcase('trust-forwarder/codex', ['mail-trust-forwarder', 'erin@example.com', 'relay.example.com'], tool='codex')
def _(b):
    world(b, codex=CODEX)


@mcase('trust-forwarder/agy', ['mail-trust-forwarder', 'gina@example.com', 'relay.example.com'], tool='agy')
def _(b):
    world(b, agy={'gina@example.com': 'US-A'})


@mcase('trust-forwarder/one-arg', ['mail-trust-forwarder', A])
def _(b):
    world(b)


# --- mail-forget -----------------------------------------------------------------------------------------------------
BOXES = {'user': USER, 'auto': False, 'proxy': SOCKS, 'accounts': {ALICE: {'user': 'alice-box@example.com'}, BOB: {'user': USER, 'forwarder': 'relay.example.com'}}}
for name, argv, mail in [
    ('all', [], BOXES), ('all-nothing-configured', [], None), ('all-mail-not-dict', [], 'x'),
    ('default', ['default'], BOXES), ('default-only', ['default'], MAIL), ('default-keeps-shared-address', ['default'], BOXES),
    ('default-nothing', ['default'], None), ('default-mail-not-dict', ['default'], 'x'), ('default-accounts-not-dict', ['default'], dict(MAIL, accounts='x')),
    ('account', ['account', A], BOXES), ('account-shares-default-address', ['account', B], BOXES), ('account-not-there', ['account', 'nobody@example.com'], BOXES),
    ('account-last-override', ['account', A], {'accounts': {ALICE: {'user': 'alice-box@example.com'}}}), ('account-no-key', ['account'], BOXES),
    ('account-nothing', ['account', A], None), ('unknown-mode-is-all', ['everything'], BOXES),
]:
    @mcase('forget/' + name, ['mail-forget'] + argv)
    def _(b, mail=mail):
        world(b, mail=mail)
        store(b, 'alice-box@example.com')
        b.json('.cca/mail/used.json', {'d1': {'at': 1, 'st': 'done'}})
        b.json('.cca/mail/last.json', LAST)


@mcase('forget/all-keyfile', ['mail-forget'], env=FILE)
def _(b):
    world(b, mail=BOXES, keyfile=True)


@mcase('forget/default-keyfile', ['mail-forget', 'default'], env=FILE)
def _(b):
    world(b, mail=MAIL, keyfile=True)


@mcase('forget/all-mail-dir-is-a-link', ['mail-forget'])
def _(b):
    world(b)
    b.write('elsewhere/used.json', '{}')
    os.symlink(b.path('elsewhere'), b.path('.cca/mail'))


@mcase('forget/all-sticky-keychain', ['mail-forget'])
def _(b):
    world(b, mail=BOXES)
    b.write('keychain_sticky', '')


@mcase('forget/codex-account', ['mail-forget', 'account', 'erin@example.com'], tool='codex')
def _(b):
    world(b, mail=dict(MAIL, accounts={ident('erin@example.com'): {'user': 'erin-box@example.com'}}), codex=CODEX)
    store(b, 'erin-box@example.com')


@mcase('forget/agy-all', ['mail-forget'], tool='agy')
def _(b):
    world(b, mail=BOXES)


@mcase('forget/too-many-args', ['mail-forget', 'account', A, 'x'])
def _(b):
    world(b, mail=BOXES)


# --- mail-setup ------------------------------------------------------------------------------------------------------
for name, argv, stdin in [
    ('bad-address', ['not-an-address'], PW), ('bad-address-semicolon', ['a;b@example.com'], PW), ('bad-address-no-dot', ['a@example'], PW),
    ('bad-address-chinese', ['中@example.com'], PW), ('bad-address-space-inside', ['in box@example.com'], PW), ('bad-address-empty', [''], PW),
    ('password-short', [USER], 'abcdefg'), ('password-empty', [USER], ''), ('password-chinese', [USER], '密码密码密码密码密码'),
    ('password-not-utf8', [USER], b'abcdefgh\xff\xfeijklmnop'), ('password-nul', [USER], 'abcdefgh\x00ijklmnop'),
]:
    @mcase('setup/' + name, ['mail-setup'] + argv + [SOCKS], stdin=stdin)
    def _(b):
        world(b, mail=None, pw=None)


for name, argv, stdin, kw in [
    ('ok', [USER, SOCKS], PW, {}), ('ok-http', ['Inbox@Example.com', HTTP], 'abcd efgh ijkl mnop\n', {}),
    ('ok-address-padded', ['  %s\n' % USER, SOCKS], '\tabcd\u3000efgh\xa0ijkl\r\nmnop  ', {}),
    ('ok-system-proxy-not-saved', [USER], PW, dict(mail=None, sysproxy=SYS_SOCKS)), ('ok-system-https-drops-saved-proxy', [USER, 'system'], PW, dict(sysproxy=SYS_HTTPS)),
    ('ok-socks5h', [USER, 'socks5h://127.0.0.1:%d/' % PORT], PW, {}),
    ('ok-reuses-saved-proxy', [USER], PW, dict(mail={'user': 'old@example.com', 'proxy': HTTP})),
    ('ok-keeps-auto-off', [USER, SOCKS], PW, dict(mail={'user': 'old@example.com', 'auto': False})),
    ('ok-auto-zero-is-dropped', [USER, SOCKS], PW, dict(mail={'user': 'old@example.com', 'auto': 0})),
    ('ok-keeps-overrides', [USER, SOCKS], PW, dict(mail=OVERRIDES)), ('ok-drops-empty-overrides', [USER, SOCKS], PW, dict(mail=dict(MAIL, accounts={}))),
    ('ok-old-address-forgotten', [USER, SOCKS], PW, dict(mail={'user': 'old@example.com'})),
    ('ok-old-address-still-used', [USER, SOCKS], PW, dict(mail={'user': 'old@example.com', 'accounts': {ALICE: {'user': 'OLD@example.com'}}})),
    ('ok-mail-not-dict', [USER, SOCKS], PW, dict(mail='x')),
    ('account', [USER, SOCKS, A], PW, {}), ('account-first-thing', [USER, SOCKS, B], PW, dict(mail=None)),
    ('account-inherits-default-proxy', ['alice-box@example.com', '', A], PW, dict(mail=MAIL)),
    ('account-replaces-override', ['new@example.com', SOCKS, A], PW, dict(mail=OVERRIDES)),
    ('account-keeps-auto-off-drops-forwarder', ['new@example.com', SOCKS, B], PW, dict(mail=dict(MAIL, accounts={BOB: {'user': 'bob-box@example.com', 'auto': False, 'forwarder': 'relay.example.com'}}))),
    ('account-accounts-not-dict', [USER, SOCKS, A], PW, dict(mail=dict(MAIL, accounts='x'))),
    ('account-alias-only', [USER, SOCKS, '只有别名'], PW, {}), ('account-api', [USER, SOCKS, '直连'], PW, {}),
    ('account-system-proxy-not-saved', [USER, 'system', A], PW, dict(mail=dict(MAIL, accounts={ALICE: {'user': 'alice-box@example.com', 'proxy': HTTP}}))),
]:
    @mcase('setup/' + name, ['mail-setup'] + argv, stdin=stdin)
    def _(b, kw=kw, argv=argv):
        world(b, **dict(dict(pw=None), **kw))
        for old in ('old@example.com', 'alice-box@example.com', 'bob-box@example.com'):
            store(b, old, 'oldoldoldoldoldo')
        ctl(b, user=argv[0].strip().lower())


for name, argv, stdin, switches in [
    ('wrong-password', [USER, SOCKS], 'wrongwrongwrongw', {}), ('imap-disabled', [USER, SOCKS], PW, dict(imap_disabled=True)),
    ('login-no-for-now', [USER, SOCKS], PW, dict(login_no='[UNAVAILABLE] Temporary System Error')),
    ('login-no-app-password', [USER, SOCKS], PW, dict(login_no='[ALERT] Application-specific password required: https://support.google.com/accounts/answer/185833 (Failure)')),
    ('login-no-control-chars', [USER, SOCKS], PW, dict(login_no='[ALERT] \x1b[2Jwiped\x07 \u202egnp.exe')),
    ('proxy-refuses-socks', [USER, SOCKS], PW, dict(proxy='refuse')), ('proxy-refuses-http', [USER, HTTP], PW, dict(proxy='refuse')),
    ('proxy-wants-auth', [USER, SOCKS], PW, dict(proxy='auth')), ('proxy-closes-socks', [USER, SOCKS], PW, dict(proxy='close')),
    ('proxy-closes-http', [USER, HTTP], PW, dict(proxy='close')), ('proxy-garbage-socks', [USER, SOCKS], PW, dict(proxy='garbage')),
    ('proxy-garbage-http', [USER, HTTP], PW, dict(proxy='garbage')),
    ('cert-from-another-ca', [USER, SOCKS], PW, dict(tls='badcert')), ('cert-for-another-name', [USER, HTTP], PW, dict(tls='wrongname')),
    ('peer-does-not-speak-tls', [USER, SOCKS], PW, dict(tls='plain')), ('closed-before-tls', [USER, SOCKS], PW, dict(tls='close')),
    ('greeting-bye', [USER, SOCKS], PW, dict(greet='* BYE Too many connections\r\n')), ('greeting-garbage', [USER, SOCKS], PW, dict(greet='220 smtp ready\r\n')),
    ('capability-bad', [USER, SOCKS], PW, dict(faults=[{'on': 'CAPABILITY', 'do': 'bad'}])), ('capability-drop', [USER, SOCKS], PW, dict(faults=[{'on': 'CAPABILITY', 'do': 'drop'}])),
    ('login-drop', [USER, SOCKS], PW, dict(faults=[{'on': 'LOGIN', 'do': 'drop'}])), ('login-bad', [USER, SOCKS], PW, dict(faults=[{'on': 'LOGIN', 'do': 'bad', 'text': 'Invalid arguments'}])),
    ('login-bye', [USER, SOCKS], PW, dict(faults=[{'on': 'LOGIN', 'do': 'bye', 'text': 'System Error'}])),
    ('list-bad', [USER, SOCKS], PW, dict(faults=[{'on': 'LIST', 'do': 'bad'}])), ('list-no', [USER, SOCKS], PW, dict(faults=[{'on': 'LIST', 'do': 'no', 'text': '[SERVERBUG] oops'}])),
    ('list-drop', [USER, SOCKS], PW, dict(faults=[{'on': 'LIST', 'do': 'drop'}])), ('list-bye', [USER, SOCKS], PW, dict(faults=[{'on': 'LIST', 'do': 'bye'}])),
    ('no-all-mail-folder', [USER, SOCKS], PW, dict(no_all=True)), ('logout-drop', [USER, SOCKS], PW, dict(faults=[{'on': 'LOGOUT', 'do': 'drop'}])),
    ('logout-bad', [USER, SOCKS], PW, dict(faults=[{'on': 'LOGOUT', 'do': 'bad'}])),
]:
    @mcase('setup/' + name, ['mail-setup'] + argv, stdin=stdin)
    def _(b, switches=switches):
        world(b, mail={'user': 'old@example.com'}, pw=None)
        store(b, 'old@example.com', 'oldoldoldoldoldo')
        ctl(b, **switches)


for name, proxy in [('cannot-connect', REFUSED), ('route-unknown', 'ftp://127.0.0.1:21'), ('route-bad-host', 'socks5://exa_mple.com:1080'),
                    ('route-is-an-account-exit', 'socks5://127.0.0.1:7901'), ('route-empty-label', 'http://a..b:8080')]:
    @mcase('setup/' + name, ['mail-setup', USER, proxy], stdin=PW)
    def _(b):
        world(b, mail=None, pw=None, cfg={'ports': {A: 7901}})


@mcase('setup/proxy-never-answers', ['mail-setup', USER, MUTE], stdin=PW, timeout=90)
def _(b):
    world(b, mail=None, pw=None)


@mcase('setup/gmail-never-answers-login', ['mail-setup', USER, SOCKS], stdin=PW, timeout=90)
def _(b):
    world(b, mail=None, pw=None)
    ctl(b, faults=[{'on': 'LOGIN', 'do': 'stall'}])


@mcase('setup/keychain-locked', ['mail-setup', 'new@example.com', SOCKS], stdin=PW)
def _(b):
    world(b)
    ctl(b, user='new@example.com')
    b.write('keychain_locked', '')


@mcase('setup/keychain-mangles', ['mail-setup', USER, SOCKS], stdin=PW)
def _(b):
    world(b, mail=None, pw=None)
    b.write('keychain_mangle', '')


@mcase('setup/keyfile', ['mail-setup', USER, HTTP], stdin=PW, env=FILE)
def _(b):
    world(b, mail=None, pw=None)


@mcase('setup/keyfile-new-address-replaces-the-file', ['mail-setup', 'new@example.com', SOCKS], stdin=PW, env=FILE)
def _(b):
    world(b, keyfile=True)
    ctl(b, user='new@example.com')


@mcase('setup/keyfile-cannot-write', ['mail-setup', USER, SOCKS], stdin=PW, env=FILE)
def _(b):
    world(b, mail=None, pw=None)
    b.write('.cca/secrets', 'a file where the directory should be')


@mcase('setup/codex', ['mail-setup', USER, SOCKS, 'erin@example.com'], stdin=PW, tool='codex')
def _(b):
    world(b, mail=None, pw=None, codex=CODEX)


@mcase('setup/agy', ['mail-setup', USER, SOCKS], stdin=PW, tool='agy')
def _(b):
    world(b, mail=None, pw=None, agy={'gina@example.com': 'US-A'})


@mcase('setup/password-with-quotes', ['mail-setup', USER, SOCKS], stdin='ab"cd\\ef(gh)ij{k}l*%')
def _(b):
    world(b, mail=None, pw=None)
    ctl(b, password='ab"cd\\ef(gh)ij{k}l*%')


@mcase('setup/no-args', ['mail-setup'], stdin=PW)
def _(b):
    world(b, mail=None, pw=None)


# --- mail-test -------------------------------------------------------------------------------------------------------
RELAYED = dict(frm='"alice@example.com" <noreply@relay.example.com>', dkim_d='relay.example.com', sign='from:to:subject:reply-to:date',
               reply_to='Anthropic <no-reply@mail.anthropic.com>')
TRUSTING = dict(MAIL, accounts={ALICE: {'forwarder': 'relay.example.com'}})


def mixed_box(b):
    """各种各样的登录邮件：好的、别的账号的、不是账号表里的、签名没过的、标题带转义序列的、垃圾邮件夹里的、太旧的、没有链接的"""
    put(b, 5, eml(tok='SECRET1'), 300)
    put(b, 6, eml('carol@example.com', tok='SECRET2'), 240)
    put(b, 7, eml(ar='fail', tok='SECRET3'), 180)
    put(b, 8, eml(subject='Secure link to log in to Claude.ai \x1b[2K\x1b[1A', ar='fail', frm='"\x1b]52;c;eA==\x07" <x@mail.anthropic.com>'), 150)
    put(b, 9, eml('Bob@Example.com', tok='SECRET4'), 120)
    put(b, 10, eml(body_links=['https://links.example.com/ls/click?u=x', 'https://EVIL.example.com/magic-link', 'not a url']), 90)
    put(b, 11, eml(subject='Your receipt from Anthropic'), 60)
    put(b, 12, eml(tok='OLD'), 9 * 86400)
    put(b, 13, eml(ar='missing', tok='NOAR'), 45)    # 没有 Gmail 写的验证结果
    put(b, 14, eml(ar='none', tok='NODKIM'), 44)     # 验证结果里没有 DKIM
    put(b, 2, eml(tok='SPAM1'), 200, folder='spam')
    put(b, 3, eml(frm='Anthropic <no-reply@evilanthropic.com>', subject='Your secure link to Claude is here', dkim_d='evilanthropic.com'), 100, folder='spam')
    put(b, 4, eml('nobody@example.com', subject='=?utf-8?b?5L2g55qE?= secure link to Claude', ar='none'), 50, folder='inbox')


for name, argv, kw in [
    ('mixed', ['mail-test'], {}), ('mixed-2-hours', ['mail-test', '2'], {}), ('mixed-two-weeks', ['mail-test', '336'], {}),
    ('hours-clamped-high', ['mail-test', '99999999999999999999999999'], {}), ('hours-clamped-low', ['mail-test', '-3'], {}),
    ('hours-padded-plus', ['mail-test', ' +0_2 '], {}), ('hours-not-a-number', ['mail-test', '2h'], {}), ('hours-float', ['mail-test', '1.5'], {}),
    ('hours-empty', ['mail-test', ''], {}), ('hours-too-many-digits', ['mail-test', '9' * 4301], {}),
    ('http-proxy', ['mail-test'], dict(mail={'user': USER, 'proxy': HTTP})), ('system-proxy', ['mail-test'], dict(mail={'user': USER}, sysproxy=SYS_SOCKS)),
    ('trusted-forwarder', ['mail-test'], dict(mail=TRUSTING)), ('not-configured', ['mail-test'], dict(mail=None)), ('no-password', ['mail-test'], dict(pw=None)),
    ('bad-route', ['mail-test'], dict(mail={'user': USER, 'proxy': 'nope'})),
]:
    @mcase('test/' + name, argv)
    def _(b, kw=kw):
        world(b, **kw)
        mixed_box(b)
        put(b, 20, eml('collect@example.com', tok='FWD1', **RELAYED), 30)
        put(b, 21, eml('collect@example.com', tok='FWD2', **dict(RELAYED, frm='"bob@example.com" <noreply@relay.example.com>')), 20)
        put(b, 22, eml('collect@example.com', tok='FWD3', **dict(RELAYED, reply_to='x@evil.example.com')), 10)


for name, switches in [
    ('wrong-password', dict(password='somethingelse1234')), ('imap-disabled', dict(imap_disabled=True)), ('cert-from-another-ca', dict(tls='badcert')),
    ('proxy-refuses', dict(proxy='refuse')), ('no-all-mail-folder', dict(no_all=True)), ('uid-after-literal', dict(uid_after_literal=True)),
    ('search-finds-nothing', dict(search_blind=True)), ('no-uidvalidity', dict(no_uidvalidity=True)), ('no-uidnext', dict(no_uidnext=True)),
    ('examine-no', dict(faults=[{'on': 'EXAMINE', 'do': 'no', 'text': '[NONEXISTENT] Unknown Mailbox (Failure)'}])),
    ('examine-drop-second-folder', dict(faults=[{'on': 'EXAMINE', 'do': 'drop', 'skip': 1, 'times': 1}])),
    ('search-bad', dict(faults=[{'on': 'UID SEARCH', 'do': 'bad', 'text': 'Could not parse command'}])),
    ('search-bye', dict(faults=[{'on': 'UID SEARCH', 'do': 'bye', 'text': 'Session expired, please login again.'}])),
    ('headers-drop', dict(faults=[{'on': 'UID FETCH', 'has': 'HEADER.FIELDS', 'do': 'drop'}])),
    ('headers-bad', dict(faults=[{'on': 'UID FETCH', 'has': 'HEADER.FIELDS', 'do': 'bad', 'text': 'Could not parse command'}])),
    ('message-drop', dict(faults=[{'on': 'UID FETCH', 'has': 'BODY.PEEK[]', 'do': 'drop'}])),
    ('message-no', dict(faults=[{'on': 'UID FETCH', 'has': 'BODY.PEEK[]', 'do': 'no', 'text': 'Some messages could not be FETCHed (Failure)'}])),
    ('message-missing', dict(faults=[{'on': 'UID FETCH', 'has': 'BODY.PEEK[]', 'do': 'reply', 'text': '{TAG} OK Success\r\n'}])),
    ('message-unterminated', dict(faults=[{'on': 'UID FETCH', 'has': 'BODY.PEEK[]', 'do': 'reply', 'text': '* 1 FETCH (UID 5 no end of line', 'close': True}])),
    ('second-examine-no', dict(faults=[{'on': 'EXAMINE', 'do': 'no', 'skip': 2}])),
]:
    @mcase('test/' + name, ['mail-test', '2'])
    def _(b, switches=switches):
        world(b)
        mixed_box(b)
        ctl(b, **switches)


@mcase('test/nothing-recent', ['mail-test', '1'])
def _(b):
    world(b)
    put(b, 5, eml(), 2 * 3600)


@mcase('test/empty-mailbox', ['mail-test'])
def _(b):
    world(b)


@mcase('test/more-than-20', ['mail-test', '3'])
def _(b):
    world(b)
    for i in range(1, 26):
        put(b, i, eml(tok='N%d' % i, html=False), 3000 - i * 60, folder='all' if i % 5 else 'spam')


@mcase('test/full-message-differs-from-headers', ['mail-test', '2'])
def _(b):
    world(b)
    good = eml(tok='HEAD')
    put(b, 5, good, 60, full=eml('carol@example.com', tok='BODY'))          # 取全文时收件人变了：照全文再核对
    put(b, 6, eml(tok='HEAD2'), 50, full=eml(tok='BODY2', html=False, text=False))   # 全文里没有正文
    put(b, 7, eml(tok='HEAD3'), 40, full=b'')                               # 全文是空的


@mcase('test/hostile-headers', ['mail-test', '2'])
def _(b):
    world(b)
    put(b, 5, eml(frm='(' * 30 + ')' * 30 + ' no-reply@mail.anthropic.com', subject='=?utf-8?q?Your_secure_link_to_Claude?= \u202eis here'), 60)
    put(b, 6, eml('"Alice" <alice@example.com>, bob@example.com', subject='x' * 200 + ' secure link to Claude'), 50)
    put(b, 7, eml().replace(b'From: Anthropic <', b'From: Anthr\xf6pic <', 1), 40)
    put(b, 8, eml(frm='', subject=''), 30)
    put(b, 9, eml(frm='a@mail.anthropic.com, b@mail.anthropic.com', to='undisclosed-recipients:;'), 20)


@mcase('test/deep-comment-is-an-uncaught-error', ['mail-test', '2'])
def _(b):
    world(b)
    put(b, 5, eml(), 60)
    put(b, 6, eml(frm='(' * 1200 + ')' * 1200 + ' no-reply@mail.anthropic.com'), 50)


# 一层套一层的上限：原来是到 Python 调用栈的一千层为止（没人接的 RecursionError），落在第几层看解析的那个函数自己在第几层、
# 是不是这个进程里头一次解析地址（CPython 3.14.0 上各条路实测过，见 cli/src/cmd/mail/header.rs 的 ADDR_FRAMES）。
# 每条路的上限两边各一个用例：at 是最多的那一层（照常往下走），over 是再多一层（出错）
def comments(n, addr):
    return '(' * n + ')' * n + ' ' + addr


def nested_message(n):
    """message/rfc822 套 n 层的邮件（最外面没有标题：解析得了的话核对到标题就停）"""
    return ('Content-Type: message/rfc822\r\n\r\n' * n + 'Subject: x\r\n\r\nhi\r\n').encode()


NOT_LOGIN = dict(subject='About your secure link')   # 搜得到、标题不像登录邮件：核对到标题就停，不会在更深的地方再解析一遍地址
for name, kw in [
    # cmd_mail_test 里直接解析：收件人是这个进程里头一次解析地址（多一层），发件人是第二次
    ('to-comments-at', dict(NOT_LOGIN, to=comments(496, 'alice@example.com'))), ('to-comments-over', dict(NOT_LOGIN, to=comments(497, 'alice@example.com'))),
    ('to-groups-at', dict(NOT_LOGIN, to='a:' * 992)), ('to-groups-over', dict(NOT_LOGIN, to='a:' * 993)),
    ('from-comments-at', dict(NOT_LOGIN, frm=comments(495, 'no-reply@mail.anthropic.com'))),
    ('from-comments-over', dict(NOT_LOGIN, frm=comments(496, 'no-reply@mail.anthropic.com'))),
    ('from-groups-at', dict(NOT_LOGIN, frm='a:' * 991)), ('from-groups-over', dict(NOT_LOGIN, frm='a:' * 992)),
    # 好的登录邮件：mail_judge_any → mail_judge 里再解析一遍收件人，深两层
    ('judge-to-comments-at', dict(to=comments(494, 'alice@example.com'))), ('judge-to-comments-over', dict(to=comments(495, 'alice@example.com'))),
]:
    @mcase('test/nested-' + name, ['mail-test', '2'])
    def _(b, kw=kw):
        world(b)
        put(b, 5, eml(**kw), 60)

    @mcase('gui-list/nested-' + name, ['gui-mail-list'], env={'CCA_RO': '1'})
    def _(b, kw=kw):
        world(b)
        put(b, 5, eml(**kw), 60)


for name, n in [('at', 983), ('over', 984)]:
    @mcase('test/nested-message-' + name, ['mail-test', '2'])   # cmd_mail_test → mail_verdict → mail_parse
    def _(b, n=n):
        world(b)
        put(b, 5, eml(), 60, full=nested_message(n))

    @mcase('gui-list/nested-message-' + name, ['gui-mail-list'], env={'CCA_RO': '1'})
    def _(b, n=n):
        world(b)
        put(b, 5, eml(), 60, full=nested_message(n))


def signed_nested(n, leaf=b'Content-Type: text/plain\r\n\r\nhttps://claude.ai/magic-link#tok-DEEP\r\n'):
    """头是好的登录邮件的头（核对通得过），里面 message/rfc822 套 n 层，最里面才是带链接的正文：取链接要一层层走到底（msg.walk()）"""
    head = eml().split(b'\r\n\r\n', 1)[0]
    head = b'\r\n'.join(l for l in head.split(b'\r\n') if not l.lower().startswith(b'content-type:'))
    return head + b'\r\n' + b'Content-Type: message/rfc822\r\n\r\n' * n + leaf


for name, n in [('at', 983), ('over', 984), ('shallow', 30)]:
    @mcase('test/nested-signed-message-' + name, ['mail-test', '2'])
    def _(b, n=n):
        world(b)
        put(b, 5, eml(), 60, full=signed_nested(n))

    @mcase('open/nested-signed-message-' + name, ['mail-open', A, '0'])
    def _(b, n=n):
        world(b)
        put(b, 5, eml(), 60, full=signed_nested(n - 2 if n > 900 else n))   # 取全文深两层：981 / 982


# cca mail open：mail_hunt → mail_judge_any → mail_judge 里头一次解析地址（发件人）；取全文是 mail_hunt → mail_take → mail_verdict → mail_parse
for name, kw, full in [
    ('from-comments-at', dict(frm=comments(494, 'no-reply@mail.anthropic.com'), ar='fail'), None),
    ('from-comments-over', dict(frm=comments(495, 'no-reply@mail.anthropic.com'), ar='fail'), None),
    ('verdict-from-comments-at', dict(frm=comments(493, 'no-reply@mail.anthropic.com')), None),   # 头通过了：取来全文再核对一遍，又深一层
    ('verdict-from-comments-over', dict(frm=comments(494, 'no-reply@mail.anthropic.com')), None),
    ('from-groups-at', dict(frm='a:' * 989), None), ('from-groups-over', dict(frm='a:' * 990), None),
    ('message-at', {}, 981), ('message-over', {}, 982),
]:
    @mcase('open/nested-' + name, ['mail-open', A, '0'])
    def _(b, kw=kw, full=full):
        world(b)
        put(b, 5, eml(**kw), 60, full=None if full is None else nested_message(full))

    @mcase('open/msg-nested-' + name, ['mail-open', A, '0', '', '"[Gmail]/All Mail":5:11'])   # mail_open_one 和 mail_hunt 在同一层
    def _(b, kw=kw, full=full):
        world(b)
        put(b, 5, eml(**kw), 60, full=None if full is None else nested_message(full))


# 登录时盯邮箱：比 cca mail open 多隔一层（cmd_mail_watch → mail_watch → mail_hunt）
for name, kw, full in [
    ('from-comments-at', dict(frm=comments(494, 'no-reply@mail.anthropic.com'), ar='fail'), None),
    ('from-comments-over', dict(frm=comments(495, 'no-reply@mail.anthropic.com'), ar='fail'), None),
    ('verdict-from-comments-at', dict(frm=comments(493, 'no-reply@mail.anthropic.com')), None),   # 头通过了：取来全文再核对一遍，又深一层
    ('verdict-from-comments-over', dict(frm=comments(494, 'no-reply@mail.anthropic.com')), None),
    ('from-groups-at', dict(frm='a:' * 988), None), ('from-groups-over', dict(frm='a:' * 989), None),
    ('message-at', {}, 980), ('message-over', {}, 981),
]:
    @mcase('watch/nested-' + name, ['mail-watch', A], env={'CCA_MAIL_POLL': '8'},
           during=lambda b: (wait_for(lambda: log_has(b, r'UID SEARCH', 2)), time.sleep(1.2), signal_child(signal.SIGTERM)))
    def _(b, kw=kw, full=full):
        world(b)
        put(b, 5, eml(**kw), -600, full=None if full is None else nested_message(full))


@mcase('test/keyfile', ['mail-test', '2'], env=FILE)
def _(b):
    world(b, keyfile=True)
    put(b, 5, eml(), 60)


@mcase('test/codex-has-no-rules', ['mail-test', '2'], tool='codex')
def _(b):
    world(b, codex=CODEX)
    mixed_box(b)


@mcase('test/agy-has-no-rules', ['mail-test', '2'], tool='agy')
def _(b):
    world(b, agy={'alice@example.com': 'US-A'})
    put(b, 5, eml(), 60)


@mcase('test/table-not-dict', ['mail-test', '2'])
def _(b):
    world(b)
    b.write('.cca/accounts.json', '"x"\n')


@mcase('test/too-many-args', ['mail-test', '2', 'x'])
def _(b):
    world(b)


# --- gui-mail-list ---------------------------------------------------------------------------------------------------
RO = {'CCA_RO': '1'}
for name, argv, kw in [
    ('all-accounts', ['gui-mail-list'], {}), ('one-account', ['gui-mail-list', A], {}), ('other-account', ['gui-mail-list', B, '2'], {}),
    ('alias-only-is-not-scoped', ['gui-mail-list', '只有别名'], {}), ('not-in-table', ['gui-mail-list', 'nobody@example.com'], {}),
    ('hours-bad-is-24', ['gui-mail-list', '', 'x'], {}), ('hours-clamped', ['gui-mail-list', '', '0'], {}), ('hours-336', ['gui-mail-list', A, '100000'], {}),
    ('trusted-forwarder', ['gui-mail-list'], dict(mail=TRUSTING)), ('trusted-forwarder-scoped', ['gui-mail-list', A], dict(mail=TRUSTING)),
    ('not-configured', ['gui-mail-list'], dict(mail=None)), ('not-configured-account', ['gui-mail-list', A], dict(mail=None)),
    ('no-password', ['gui-mail-list'], dict(pw=None)), ('bad-route', ['gui-mail-list'], dict(mail={'user': USER, 'proxy': 'nope'})),
    ('override-box', ['gui-mail-list', A], dict(mail=dict(MAIL, accounts={ALICE: {'user': USER, 'proxy': HTTP}}))),
]:
    @mcase('gui-list/' + name, argv, env=RO)
    def _(b, kw=kw):
        world(b, **kw)
        mixed_box(b)
        put(b, 20, eml('collect@example.com', tok='FWD1', **RELAYED), 30)
        put(b, 21, eml('collect@example.com', tok='FWD2', **dict(RELAYED, frm='"bob@example.com" <noreply@relay.example.com>')), 20)


for name, switches in [
    ('wrong-password', dict(password='somethingelse1234')), ('cert-from-another-ca', dict(tls='badcert')), ('proxy-wants-auth', dict(proxy='auth')),
    ('list-drop', dict(faults=[{'on': 'LIST', 'do': 'drop'}])), ('search-bad', dict(faults=[{'on': 'UID SEARCH', 'do': 'bad'}])),
    ('message-drop-after-folders-known', dict(faults=[{'on': 'UID FETCH', 'has': 'BODY.PEEK[]', 'do': 'drop'}])),
    ('uid-after-literal', dict(uid_after_literal=True)), ('no-all-mail-folder', dict(no_all=True)), ('no-uidvalidity', dict(no_uidvalidity=True)),
]:
    @mcase('gui-list/' + name, ['gui-mail-list'], env=RO)
    def _(b, switches=switches):
        world(b)
        mixed_box(b)
        ctl(b, **switches)


@mcase('gui-list/truncated', ['gui-mail-list', '', '3'], env=RO)
def _(b):
    world(b)
    for i in range(1, 54):
        put(b, i, eml(tok='N%d' % i, html=False), 4000 - i * 60, folder='all' if i % 7 else 'spam')


@mcase('gui-list/full-message-differs-from-headers', ['gui-mail-list'], env=RO)
def _(b):
    world(b)
    put(b, 5, eml(tok='HEAD'), 60, full=eml('carol@example.com', tok='BODY'))
    put(b, 6, eml(tok='HEAD2'), 50, full=eml(tok='BODY2', html=False, text=False))
    put(b, 7, eml(tok='HEAD3'), 40)


@mcase('gui-list/hostile-mail-is-an-uncaught-error', ['gui-mail-list'], env=RO)
def _(b):
    world(b)
    put(b, 5, eml(), 60)
    put(b, 6, eml('(' * 1200 + ')' * 1200 + ' alice@example.com'), 50)


def hostile_box(b):
    """头里的怪东西：不是 UTF-8 的字节、编码过的字（解不出来的、解出来是控制字符的）、JSON 里要转义的字符、折行"""
    put(b, 5, eml(subject='Your secure link to Claude is here "q" \\ / </script>    \U0001F600 \x7f\x01'), 60)
    put(b, 6, eml().replace(b'Subject: Your', b'Subject: \xff\xfe Y\xc3\xa9 \xed\xa0\x80 Your', 1), 55)
    put(b, 7, eml(subject='=?utf-8?b?/v8=?= =?iso-8859-1?q?caf=E9?= =?utf-8?q?=ED=A0=80_=F0=9F=98=80?= =?us-ascii?q?=FF?= Your secure link to Claude is here'), 50)
    put(b, 8, eml().replace(b'From: Anthropic <', b'From: "Anthr\xf6pic \\"x\\"" <', 1), 45)
    put(b, 9, eml('"Al\\"ice" <ALICE@Example.COM>, =?utf-8?q?B=C3=B6b?= <bob@example.com>'), 40)
    put(b, 10, eml().replace(b'To: alice@example.com', b'To: al\xefce@example.com, alice@example.com', 1), 35)
    put(b, 11, eml(subject='=?windows-1252?q?=80uro_=81?= =?latin1?b?4A==?= =?UTF8?Q?secure_link_to_Claude?=\x00x'), 30)
    put(b, 12, eml(subject='Your secure link\r\n to Claude\r\n\tis here'), 25)
    put(b, 13, eml('alice@example.com (\xe4 comment), "b\xf6b" <bob@example.com>'), 20)
    put(b, 14, eml(frm='=?utf-8?q?=22=5C?= <no-reply@mail.anthropic.com>', subject=' Your secure link to Claude is here　'), 15)


for name, argv, env in [('test/hostile-bytes', ['mail-test', '2'], {}), ('gui-list/hostile-bytes', ['gui-mail-list', '', '2'], RO),
                        ('gui-list/hostile-bytes-one-account', ['gui-mail-list', A, '2'], RO)]:
    @mcase(name, argv, env=env)
    def _(b):
        world(b)
        hostile_box(b)


@mcase('gui-list/empty', ['gui-mail-list'], env=RO)
def _(b):
    world(b)


@mcase('gui-list/codex', ['gui-mail-list', 'erin@example.com'], env=RO, tool='codex')
def _(b):
    world(b, codex=CODEX)
    put(b, 5, eml('erin@example.com'), 60)


@mcase('gui-list/agy', ['gui-mail-list'], env=RO, tool='agy')
def _(b):
    world(b, agy={'alice@example.com': 'US-A'})
    put(b, 5, eml(), 60)


@mcase('gui-list/too-many-args', ['gui-mail-list', A, '2', 'x'], env=RO)
def _(b):
    world(b)


# --- mail-open -------------------------------------------------------------------------------------------------------
def open_box(b):
    """最近的两封（新的那封该打开）、三小时前的一封、别的账号的一封"""
    put(b, 5, eml(tok='A1'), 600)
    put(b, 6, eml(tok='A2'), 300)
    put(b, 7, eml(tok='OLD'), 3 * 3600)
    put(b, 8, eml('bob@example.com', tok='BOB'), 100)


for name, argv, kw, rc in [
    ('newest', [A, '0'], {}, None), ('by-email-key', ['alice@example.com', '0'], {}, None), ('other-account', [B, '0'], {}, None),
    ('default-wait-finds-it-at-once', [A], {}, None), ('http-proxy', [A, '0'], dict(mail={'user': USER, 'proxy': HTTP}), None),
    ('system-proxy', [A, '0'], dict(mail={'user': USER}, sysproxy=SYS_HTTPS), None),
    ('browser-fails-three-times', [A, '0'], {}, '3\n3\n3\n'), ('browser-works-the-third-time', [A, '0'], {}, '1\n1\n0\n'),
    ('refused-by-mail-open', [A, '0'], {}, '2\n'), ('opener-killed', [A, '0'], {}, '137\n137\n137\n'),
    ('nothing-for-this-account', ['carol@example.com', '0'], {}, None), ('alias-only-account', ['只有别名', '0'], {}, None),
    ('wait-not-a-number', [A, 'soon'], {}, None), ('wait-negative-is-0', [A, '-5'], {}, None), ('wait-empty', [A, ''], {}, None),
    ('not-configured', [A, '0'], dict(mail=None), None), ('no-password', [A, '0'], dict(pw=None), None),
    ('bad-route', [A, '0'], dict(mail={'user': USER, 'proxy': 'nope'}), None),
    ('msg-bad-format', [A, '0', '', 'INBOX:5'], {}, None), ('msg-bad-uid', [A, '0', '', '"[Gmail]/All Mail":x:11'], {}, None),
    ('msg-empty-folder', [A, '0', '', ':5:11'], {}, None), ('msg-superscript-digits', [A, '0', '', 'INBOX:\u00b2:11'], {}, None),
    ('msg-ok', [A, 'soon', '', '"[Gmail]/All Mail":5:11'], {}, None), ('msg-ok-validity-0-not-checked', [A, '0', '', '"[Gmail]/All Mail":6:0'], {}, None),
    ('msg-ok-arabic-digits', [A, '0', '', '"[Gmail]/All Mail":\u0665:\u0661\u0661'], {}, None),
    ('msg-old-one-is-fine-when-picked', [A, '0', '', '"[Gmail]/All Mail":7:11'], {}, None),
    ('msg-validity-changed', [A, '0', '', '"[Gmail]/All Mail":5:12'], {}, None), ('msg-not-there', [A, '0', '', '"[Gmail]/All Mail":99:11'], {}, None),
    ('msg-huge-uid', [A, '0', '', '"[Gmail]/All Mail":99999999999999999999999999:11'], {}, None),
    ('msg-huge-validity', [A, '0', '', '"[Gmail]/All Mail":5:99999999999999999999999999'], {}, None),
    ('msg-other-accounts-mail', [A, '0', '', '"[Gmail]/All Mail":8:11'], {}, None), ('msg-unknown-folder', [A, '0', '', 'Nope:5:11'], {}, None),
    ('msg-folder-with-colons', [A, '0', '', 'a:b:5:11'], {}, None), ('msg-browser-fails', [A, '0', '', '"[Gmail]/All Mail":5:11'], {}, '3\n'),
    ('msg-refused', [A, '0', '', '"[Gmail]/All Mail":5:11'], {}, '2\n'), ('msg-folder-not-ascii', [A, '0', '', '收件箱:5:11'], {}, None),
    # 邮件夹名原样拼进 EXAMINE（imaplib 不加引号、不查控制字符）：带空格的成了两个参数，带 CR LF 的后面那一段是另一条命令
    ('msg-folder-with-space', [A, '0', '', '[Gmail]/All Mail:5:11'], {}, None),
    ('msg-folder-with-crlf', [A, '0', '', 'INBOX\r\nZZZZ9 NOOP:5:11'], {}, None),
    ('msg-uid-0', [A, '0', '', '"[Gmail]/All Mail":0:11'], {}, None), ('msg-leading-zeros', [A, '0', '', '"[Gmail]/All Mail":005:0011'], {}, None),
    ('msg-validity-00-not-checked', [A, '0', '', '"[Gmail]/All Mail":5:00'], {}, None),
    ('msg-uid-too-many-digits', [A, '0', '', '"[Gmail]/All Mail":%s:11' % ('5' * 4301)], {}, None),
    ('msg-padded-digits-are-not-digits', [A, '0', '', '"[Gmail]/All Mail": 5:11'], {}, None),
]:
    @mcase('open/' + name, ['mail-open'] + argv)
    def _(b, kw=kw, rc=rc):
        world(b, **kw)
        open_box(b)
        if rc:
            b.write('open.rc', rc)


for name, argv, used in [
    ('already-opened', [A, '0'], 'done'), ('again', [A, '0', 'again'], 'done'), ('again-browser-fails-stays-done', [A, '0', 'again'], 'done'),
    ('claimed-by-a-dead-process', [A, '0'], 'dead'), ('claimed-long-ago', [A, '0'], 'stale'), ('claimed-by-a-live-process', [A, '0'], 'live'),
    ('msg-already-opened', [A, '0', '', '"[Gmail]/All Mail":6:11'], 'done'), ('msg-again', [A, '0', 'again', '"[Gmail]/All Mail":6:11'], 'done'),
    ('again-word-must-be-exact', [A, '0', 'Again'], 'done'),
]:
    @mcase('open/' + name, ['mail-open'] + argv)
    def _(b, used=used, name=name):
        world(b)
        open_box(b)
        raw = eml(tok='A2', msgid='<fixed-a2@mail.anthropic.com>')
        put(b, 6, raw, 300)
        got = int(os.stat(b.path('gmail/box/all/6.eml')).st_mtime)
        dg = hashlib.sha256(('<fixed-a2@mail.anthropic.com>\n%d' % got).encode()).hexdigest()[:16]
        entry = {'done': {'at': time.time() - 30, 'st': 'done'}, 'dead': {'at': time.time() - 5, 'st': 'claim', 'pid': 2147483600},
                 'stale': {'at': time.time() - 600, 'st': 'claim', 'pid': os.getpid()}, 'live': {'at': time.time() + 600, 'st': 'claim', 'pid': os.getpid()}}[used]
        b.json('.cca/mail/used.json', {dg: entry, 'two-days-old': {'at': time.time() - 3 * 86400, 'st': 'done'}, 'junk': 5})
        if 'browser-fails' in name:
            b.write('open.rc', '1\n1\n1\n')


# used.json 里手改出来的怪东西：at、pid 不是该有的类型。NOW 换成现在的时间，ME 换成跑对拍的这个进程（一个还在的、别的进程）
for name, entry in [
    ('at-true-is-1970', {'at': True, 'st': 'done'}), ('at-string-is-dropped', {'at': 'NOW', 'st': 'done'}), ('at-null-is-dropped', {'at': None, 'st': 'done'}),
    ('at-huge-float', {'at': 1e300, 'st': 'done'}), ('at-too-big-for-a-float', {'at': 10 ** 400, 'st': 'done'}), ('st-unknown', {'at': 'now', 'st': 'opened'}),
    ('st-missing', {'at': 'now'}), ('pid-missing', {'at': 'now', 'st': 'claim'}), ('pid-string-digits', {'at': 'now', 'st': 'claim', 'pid': 'ME'}),
    ('pid-string-junk', {'at': 'now', 'st': 'claim', 'pid': 'x'}), ('pid-float', {'at': 'now', 'st': 'claim', 'pid': 'ME.0'}), ('pid-list', {'at': 'now', 'st': 'claim', 'pid': [1]}),
    ('pid-true', {'at': 'now', 'st': 'claim', 'pid': True}), ('pid-zero-is-this-process-group', {'at': 'now', 'st': 'claim', 'pid': 0}),
    ('pid-too-big', {'at': 'now', 'st': 'claim', 'pid': 10 ** 30}), ('pid-bigger-than-int', {'at': 'now', 'st': 'claim', 'pid': 2 ** 40}),
    ('pid-negative', {'at': 'now', 'st': 'claim', 'pid': -2147483600}), ('entry-is-a-list', [1, 2]),
]:
    @mcase('open/used-' + name, ['mail-open', A, '0'])
    def _(b, entry=entry):
        world(b)
        put(b, 6, eml(tok='A2', msgid='<fixed-a2@mail.anthropic.com>'), 300)
        got = int(os.stat(b.path('gmail/box/all/6.eml')).st_mtime)
        dg = hashlib.sha256(('<fixed-a2@mail.anthropic.com>\n%d' % got).encode()).hexdigest()[:16]
        if isinstance(entry, dict):
            entry = dict(entry)
            if entry.get('at') == 'now':
                entry['at'] = time.time() + 60   # 两边先后跑，隔几十秒也还算「刚才」
            if entry.get('at') == 'NOW':
                entry['at'] = str(time.time())
            if entry.get('pid') == 'ME':
                entry['pid'] = str(os.getpid())
            if entry.get('pid') == 'ME.0':
                entry['pid'] = float(os.getpid())
        b.json('.cca/mail/used.json', {dg: entry})


for name, how in [('missing', 'rm'), ('not-executable', 'chmod'), ('dies-by-signal', 'kill')]:
    @mcase('open/opener-' + name, ['mail-open', A, '0'])
    def _(b, how=how):
        world(b)
        open_box(b)
        if how == 'rm':
            os.remove(b.path('.cca/bin/cca'))
        elif how == 'chmod':
            os.chmod(b.path('.cca/bin/cca'), 0o644)
        else:
            b.write('.cca/bin/cca', '#!/bin/sh\nIFS= read -r url\nprintf \'%s\\n\' "$2" >> "$HOME/opened.log"\nkill -9 $$\n', 0o755)


for name, switches in [
    ('wrong-password', dict(password='somethingelse1234')), ('imap-disabled', dict(imap_disabled=True)), ('cert-from-another-ca', dict(tls='badcert')),
    ('proxy-refuses', dict(proxy='refuse')), ('login-no-for-now', dict(login_no='[UNAVAILABLE] Temporary System Error')),
    ('first-poll-dropped', dict(faults=[{'on': 'UID SEARCH', 'do': 'drop'}])), ('uid-after-literal', dict(uid_after_literal=True)),
    ('search-finds-nothing-latest-burst-does', dict(search_blind=True)), ('no-all-mail-folder', dict(no_all=True)),
    ('no-uidvalidity', dict(no_uidvalidity=True)), ('no-uidnext', dict(no_uidnext=True)), ('no-uidnext-search-finds-nothing', dict(no_uidnext=True, search_blind=True)),
    ('message-dropped-every-time', dict(faults=[{'on': 'UID FETCH', 'has': 'BODY.PEEK[]', 'do': 'drop'}])),
    ('message-missing-every-time', dict(faults=[{'on': 'UID FETCH', 'has': 'BODY.PEEK[]', 'do': 'reply', 'text': '{TAG} OK Success\r\n'}])),
]:
    @mcase('open/' + name, ['mail-open', A, '0'])
    def _(b, switches=switches):
        world(b)
        open_box(b)
        ctl(b, **switches)

    @mcase('open/msg-' + name, ['mail-open', A, '0', '', '"[Gmail]/All Mail":6:11'])
    def _(b, switches=switches):
        world(b)
        open_box(b)
        ctl(b, **switches)


for name, mails in [
    ('rejected-ones-are-counted', [(5, eml(ar='fail'), 300), (6, eml(frm='Anthropic <no-reply@evilanthropic.com>'), 200), (7, eml(sign='from:subject'), 100),
                                   (8, eml(tok='X', extra=['To: alice@example.com']), 50), (9, eml(ar='fail', tok='Y'), 40)]),
    ('no-link-in-the-newest', [(5, eml(tok='A1'), 300), (6, eml(body_links=['https://links.example.com/x']), 100)]),
    ('only-others-and-old', [(5, eml('bob@example.com'), 300), (6, eml(tok='OLD'), 4000), (7, eml(subject='Your receipt'), 100)]),
    ('same-second-higher-uid-wins', [(5, eml(tok='A1'), 300), (6, eml(tok='A2'), 300)]),
    ('spam-folder-counts-too', [(5, eml(tok='A1'), 300)]),
]:
    @mcase('open/' + name, ['mail-open', A, '0'])
    def _(b, mails=mails, name=name):
        world(b)
        at = time.time()
        for uid, raw, ago in mails:
            mf.put_eml(b.path('gmail/box/all/%d.eml' % uid), raw, at - ago)
        if name.startswith('spam'):
            put(b, 3, eml(tok='SPAM'), 60, folder='spam')


@mcase('open/full-message-differs-from-headers', ['mail-open', A, '0'])
def _(b):
    world(b)
    put(b, 5, eml(tok='HEAD'), 60, full=eml('carol@example.com', tok='BODY'))


@mcase('open/trusted-forwarder', ['mail-open', A, '0'])
def _(b):
    world(b, mail=TRUSTING)
    put(b, 5, eml('collect@example.com', tok='FWD1', **RELAYED), 60)


@mcase('open/forwarder-not-trusted-for-this-account', ['mail-open', B, '0'])
def _(b):
    world(b, mail=TRUSTING)
    put(b, 5, eml('collect@example.com', tok='FWD1', **dict(RELAYED, frm='"bob@example.com" <noreply@relay.example.com>')), 60)


@mcase('open/env-kept-and-dropped', ['mail-open', A, '0'],
       env={'CCA_PROXY': 'http://127.0.0.1:7901', 'CCA_EVENTS': '/dev/null', 'CCA_GRANT': 'x', 'CCA_PROMPT': 'x', 'CCA_TTY': '/dev/null',
            'CCA_YES': '1', 'CCA_RO': '0', 'KEEP_ME': 'yes'})
def _(b):
    world(b)
    open_box(b)


@mcase('open/keyfile', ['mail-open', A, '0'], env=FILE)
def _(b):
    world(b, keyfile=True)
    open_box(b)


@mcase('open/override-box', ['mail-open', A, '0'])
def _(b):
    world(b, mail=dict(MAIL, accounts={ALICE: {'user': 'alice-box@example.com', 'proxy': HTTP}}))
    store(b, 'alice-box@example.com')
    ctl(b, user='alice-box@example.com')
    open_box(b)


@mcase('open/codex-has-no-rules', ['mail-open', 'erin@example.com', '0'], tool='codex')
def _(b):
    world(b, codex=CODEX)
    put(b, 5, eml('erin@example.com'), 60)


@mcase('open/codex-msg-has-no-rules', ['mail-open', 'erin@example.com', '0', '', '"[Gmail]/All Mail":5:11'], tool='codex')
def _(b):
    world(b, codex=CODEX)
    put(b, 5, eml('erin@example.com'), 60)


@mcase('open/agy-has-no-rules', ['mail-open', 'gina@example.com', '0'], tool='agy')
def _(b):
    world(b, agy={'gina@example.com': 'US-A'})
    put(b, 5, eml('gina@example.com'), 60)


@mcase('open/agy-not-configured', ['mail-open', 'gina@example.com', '0'], tool='agy')
def _(b):
    world(b, mail=None, agy={'gina@example.com': 'US-A'})


@mcase('open/hostile-mail-is-an-uncaught-error', ['mail-open', A, '0'])
def _(b):
    world(b)
    open_box(b)
    put(b, 9, eml(frm='(' * 1200 + ')' * 1200 + ' no-reply@mail.anthropic.com'), 30)


@mcase('open/mail-dir-is-a-file', ['mail-open', A, '0'])
def _(b):
    world(b)
    open_box(b)
    b.write('.cca/mail', 'not a directory')


@mcase('open/no-args', ['mail-open'])
def _(b):
    world(b)


# 等邮件的：两次看邮箱之间隔 1.5 秒（要不隔 8 秒、等着发信号），第一次看完以后 during 才放邮件，下一次看的时候两边都看得到
SLOW, IDLE = {'CCA_MAIL_POLL': '1.5'}, {'CCA_MAIL_POLL': '8'}


def first_look_done(b):
    """第一次看邮箱看完了（两个邮件夹各搜了一遍）"""
    wait_for(lambda: log_has(b, r'UID SEARCH', 2))
    time.sleep(0.2)


def arrives(b):
    first_look_done(b)
    deliver(b)


@mcase('open/waits-for-a-new-mail', ['mail-open', A, '30'], env=SLOW, during=arrives)
def _(b):
    world(b)
    put(b, 4, eml(tok='OLD'), 3 * 3600)
    put(b, 9, eml(tok='W'), 20, stash=True)


@mcase('open/waits-new-mail-search-lags', ['mail-open', A, '30'], env=SLOW, during=arrives)
def _(b):
    world(b)
    ctl(b, search_blind=True)
    put(b, 9, eml(tok='W'), 20, stash=True)


@mcase('open/waits-new-mail-in-spam', ['mail-open', A, '30'], env=SLOW, during=arrives)
def _(b):
    world(b)
    put(b, 2, eml(tok='S'), 20, folder='spam', stash=True)


def arrives_uidvalidity_changes(b):
    first_look_done(b)
    deliver(b)
    if in_time():
        b.write('gmail/box/all/.uidvalidity', '12\n')   # 邮件夹重建了：同样的 UID 是另外的邮件，要重新搜


@mcase('open/waits-uidvalidity-changes', ['mail-open', A, '30'], env=SLOW, during=arrives_uidvalidity_changes)
def _(b):
    world(b)
    put(b, 4, eml(tok='OLD'), 3 * 3600)
    put(b, 9, eml(tok='W'), 20, stash=True)


for name, switches in [('no-uidvalidity', dict(no_uidvalidity=True)), ('no-uidnext', dict(no_uidnext=True))]:
    @mcase('open/waits-' + name, ['mail-open', A, '30'], env=SLOW, during=arrives)   # Gmail 不给这一项：每一轮都搜
    def _(b, switches=switches):
        world(b)
        ctl(b, **switches)
        put(b, 4, eml(tok='OLD'), 3 * 3600)
        put(b, 9, eml(tok='W'), 20, stash=True)


@mcase('open/waits-in-vain', ['mail-open', A, '3'], env=IDLE)
def _(b):
    world(b)
    put(b, 4, eml(tok='OLD'), 3 * 3600)


# CCA_MAIL_POLL 写得不对时用默认的 3 秒（float() 认不出的、不是正数的），inf 是一直等到头：等 3 秒的话都正好看两次邮箱
for name, poll in [('not-a-number', '0.1s'), ('control-char-is-not-blank', '\x1c0.1'), ('negative', '-0.1'), ('nan', 'nan'), ('inf', 'inf'),
                   ('underscore-between-digits', '1_0'), ('fullwidth-digits', '１０')]:
    @mcase('open/poll-knob-' + name, ['mail-open', A, '3'], env={'CCA_MAIL_POLL': poll})
    def _(b):
        world(b)
        put(b, 4, eml(tok='OLD'), 3 * 3600)


@mcase('open/waits-in-vain-others-arrive', ['mail-open', A, '3'], env=SLOW, during=arrives)
def _(b):
    world(b)
    put(b, 9, eml('bob@example.com', tok='BOB'), 20, stash=True)
    put(b, 10, eml(ar='fail', tok='BAD'), 10, stash=True)


@mcase('open/connection-drops-while-waiting', ['mail-open', A, '30'], env=SLOW, during=arrives)
def _(b):
    world(b)
    ctl(b, faults=[{'on': 'EXAMINE', 'do': 'drop', 'skip': 2, 'times': 1}])
    put(b, 9, eml(tok='W'), 20, stash=True)


def ctrl_c(b):
    first_look_done(b)
    time.sleep(0.3)
    signal_child(signal.SIGINT)


@mcase('open/ctrl-c-while-waiting', ['mail-open', A, '30'], env=IDLE, during=ctrl_c)
def _(b):
    world(b)


def ctrl_c_opening(b):
    wait_for(lambda: os.path.exists(b.path('opened.log')))
    time.sleep(0.3)
    signal_child(signal.SIGINT)


@mcase('open/ctrl-c-while-the-browser-opens', ['mail-open', A, '0'], during=ctrl_c_opening)
def _(b):
    world(b)
    open_box(b)
    b.write('open.sleep', '3')


@mcase('open/msg-ctrl-c-while-the-browser-opens', ['mail-open', A, '0', 'again', '"[Gmail]/All Mail":6:11'], during=ctrl_c_opening)
def _(b):
    world(b)
    open_box(b)
    b.write('open.sleep', '3')



# --- mail-watch ------------------------------------------------------------------------------------------------------
def lines(b, rel):
    try:
        with open(b.path(rel), encoding='utf-8') as f:
            return len(f.read().splitlines())
    except OSError:
        return 0


def term_when(cond, pause=0.5):
    """等到 cond(b)，再过一会儿（它这时在两次看邮箱之间歇着）发 TERM：登录结束了"""
    def during(b):
        wait_for(lambda: cond(b))
        time.sleep(pause)
        signal_child(signal.SIGTERM)
    return during


def arrives_then_term(done, pause=0.7):
    """第一次看完邮箱以后来了邮件；等到 done(b)（它把这封处理完了），再过一会儿发 TERM"""
    def during(b):
        first_look_done(b)
        deliver(b)
        wait_for(lambda: done(b))
        time.sleep(pause)
        signal_child(signal.SIGTERM)
    return during


OPENED = arrives_then_term(lambda b: lines(b, 'opened.log') >= 1)
FAILED3 = arrives_then_term(lambda b: lines(b, 'opened.log') >= 3)
FETCHED = arrives_then_term(lambda b: log_has(b, r'BODY\.PEEK\[\]'))
WATCH = {'CCA_MAIL_POLL': '1.5', 'CCA_MAIL_RETRY': '0.2'}
for name, argv, kw in [
    ('not-configured', [A], dict(mail=None)), ('auto-off', [A], dict(mail=dict(MAIL, auto=False))), ('alias-only-account', ['只有别名'], {}),
    ('no-password', [A], dict(pw=None)), ('bad-route', [A], dict(mail={'user': USER, 'proxy': 'nope'})),
    ('exit-route', [A], dict(mail={'user': USER, 'proxy': 'socks5://127.0.0.1:7901'}, cfg={'ports': {A: 7901}})),
    ('override-auto-off', [A], dict(mail=dict(MAIL, accounts={ALICE: {'user': USER, 'auto': False}}))),
    ('ports-not-a-dict-is-an-error-said-once', [A], dict(mail={'user': USER, 'proxy': 'socks5://127.0.0.1:7901'}, cfg={'ports': [7901]})),
]:
    @mcase('watch/' + name, ['mail-watch'] + argv, env=WATCH)
    def _(b, kw=kw):
        world(b, **kw)
        put(b, 5, eml(), -600)


@mcase('watch/codex-does-nothing', ['mail-watch', 'erin@example.com'], env=WATCH, tool='codex')
def _(b):
    world(b, codex=CODEX)


@mcase('watch/agy-does-nothing', ['mail-watch', 'gina@example.com'], env=WATCH, tool='agy')
def _(b):
    world(b, agy={'gina@example.com': 'US-A'})


@mcase('watch/broken-config-is-said-and-still-0', ['mail-watch', A], env=WATCH)
def _(b):
    world(b)
    b.write('.cca/config.json', '{oops')


@mcase('watch/keychain-unreadable', ['mail-watch', A], env=WATCH)
def _(b):
    world(b)
    b.write('keychain_unreadable', '')


@mcase('watch/no-args', ['mail-watch'], env=WATCH)
def _(b):
    world(b)


@mcase('watch/opens-the-mail-that-arrives', ['mail-watch', A], env=WATCH, during=OPENED)
def _(b):
    world(b)
    put(b, 4, eml(tok='BEFORE'), 300)                       # 登录开始以前就到了的：不开
    put(b, 5, eml('bob@example.com', tok='BOB'), -600, stash=True)   # 别的账号的：不开、不说
    put(b, 6, eml(tok='LOGIN'), -600, stash=True)


@mcase('watch/opens-via-http-proxy-and-override', ['mail-watch', A], env=WATCH, during=OPENED)
def _(b):
    world(b, mail=dict(MAIL, accounts={ALICE: {'user': 'alice-box@example.com', 'proxy': HTTP, 'forwarder': 'relay.example.com'}}))
    store(b, 'alice-box@example.com')
    ctl(b, user='alice-box@example.com')
    put(b, 6, eml('collect@example.com', tok='FWD', **RELAYED), -600, stash=True)


@mcase('watch/opens-newest-of-several', ['mail-watch', A], env=WATCH, during=OPENED)
def _(b):
    world(b)
    put(b, 6, eml(tok='FIRST'), -500, stash=True)
    put(b, 7, eml(tok='SECOND'), -600, stash=True)
    put(b, 3, eml(tok='SPAM'), -550, folder='spam', stash=True)


@mcase('watch/says-why-not-opened', ['mail-watch', A], env=WATCH, during=FETCHED)
def _(b):
    world(b)
    put(b, 5, eml(ar='fail', tok='X1'), -600, stash=True)
    put(b, 6, eml(frm='Anthropic <no-reply@evilanthropic.com>', tok='X2'), -610, stash=True)
    put(b, 7, eml(subject='Your receipt'), -620, stash=True)
    put(b, 8, eml(body_links=['https://links.example.com/x']), -630, stash=True)


@mcase('watch/browser-fails-three-times-then-says-so', ['mail-watch', A], env=WATCH, during=FAILED3)
def _(b):
    world(b)
    b.write('open.rc', '1\n1\n1\n')
    put(b, 6, eml(tok='LOGIN'), -600, stash=True)


@mcase('watch/refused-is-said', ['mail-watch', A], env=WATCH, during=OPENED)
def _(b):
    world(b)
    b.write('open.rc', '2\n')
    put(b, 6, eml(tok='LOGIN'), -600, stash=True)


@mcase('watch/full-message-differs-from-headers', ['mail-watch', A], env=WATCH, during=FETCHED)
def _(b):
    world(b)
    put(b, 6, eml(tok='HEAD'), -600, stash=True)
    mf.put_eml(b.path('gmail/box/all/6.full'), eml('carol@example.com', tok='BODY'), time.time())


@mcase('watch/already-opened-by-another-cca', ['mail-watch', A], env=WATCH,
       during=term_when(lambda b: log_has(b, r'UID SEARCH', 2)))
def _(b):
    world(b)
    raw = eml(tok='LOGIN', msgid='<fixed-login@mail.anthropic.com>')
    put(b, 6, raw, -600)
    got = int(os.stat(b.path('gmail/box/all/6.eml')).st_mtime)
    dg = hashlib.sha256(('<fixed-login@mail.anthropic.com>\n%d' % got).encode()).hexdigest()[:16]
    b.json('.cca/mail/used.json', {dg: {'at': time.time(), 'st': 'done'}})


@mcase('watch/wrong-password-stops-for-good', ['mail-watch', A], env=WATCH)
def _(b):
    world(b)
    ctl(b, password='somethingelse1234')


@mcase('watch/cert-from-another-ca-stops-for-good', ['mail-watch', A], env=WATCH)
def _(b):
    world(b)
    ctl(b, tls='badcert')


@mcase('watch/cannot-connect-says-once-and-keeps-trying', ['mail-watch', A], env=dict(WATCH, CCA_MAIL_RETRY='0.1'),
       during=term_when(lambda b: log_has(b, r'PROXY', 4), 0.3))
def _(b):
    world(b)
    ctl(b, proxy='refuse')


@mcase('watch/reconnects-after-a-drop', ['mail-watch', A], env=WATCH, during=OPENED)
def _(b):
    world(b)
    ctl(b, faults=[{'on': 'EXAMINE', 'do': 'drop', 'skip': 1, 'times': 2}])
    put(b, 6, eml(tok='LOGIN'), -600, stash=True)


@mcase('watch/term-while-idle', ['mail-watch', A], env=dict(WATCH, CCA_MAIL_POLL='5'), during=term_when(lambda b: log_has(b, r'UID SEARCH', 2)))
def _(b):
    world(b)


def ctrl_c_then_term(b):
    first_look_done(b)
    signal_child(signal.SIGINT)   # Ctrl-C 是给登录的：不理
    time.sleep(0.3)
    deliver(b)
    wait_for(lambda: os.path.exists(b.path('opened.log')))
    time.sleep(0.7)
    signal_child(signal.SIGTERM)


@mcase('watch/ctrl-c-is-ignored', ['mail-watch', A], env=WATCH, during=ctrl_c_then_term)
def _(b):
    world(b)
    put(b, 6, eml(tok='LOGIN'), -600, stash=True)


def term_while_opening(b):
    wait_for(lambda: os.path.exists(b.path('opened.log')))
    time.sleep(0.3)
    signal_child(signal.SIGTERM)


@mcase('watch/term-while-the-browser-opens', ['mail-watch', A], env=WATCH, during=term_while_opening)
def _(b):
    world(b)
    b.write('open.sleep', '2')
    put(b, 6, eml(tok='LOGIN'), -600)


@mcase('watch/hostile-mail-is-an-error-said-once', ['mail-watch', A], env=WATCH)
def _(b):
    world(b)
    put(b, 6, eml(frm='(' * 1200 + ')' * 1200 + ' no-reply@mail.anthropic.com'), -600)


@mcase('watch/mail-dir-is-a-file', ['mail-watch', A], env=WATCH)
def _(b):
    world(b)
    put(b, 6, eml(tok='LOGIN'), -600)
    b.write('.cca/mail', 'not a directory')
