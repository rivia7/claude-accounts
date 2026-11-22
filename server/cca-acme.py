#!/usr/bin/env python3
"""cca 的证书助手：给远程会话的网页端（手机浏览器里的那一页，README「网页端」）申请、续上浏览器信得过的证书。

手机的浏览器只认证书机构签的证书。这个程序照 ACME（RFC 8555）向 Let's Encrypt 要一张——给 IP 地址签的（RFC 8738：不用域名，
只有约 6 天的短期证书这一种）或者给域名签的——放到一个目录里（fullchain.pem、privkey.pem），到期之前自己续上。服务端
（cca-presence.py 的 --web-cert / --web-key）发现文件换了会自己重新读，不用重启。

验证只用 http-01：证书机构从公网连这台机器的 **80 端口**，取 /.well-known/acme-challenge/<令牌>（给 IP 签的证书不能用 DNS 验证）。
80 端口要从公网连得上。两种办法应答：
- 这个程序自己一直听着一个端口（--listen，部署时把 80 端口指到它），别的路径一律 404
- 80 端口上已经有别的网站服务（nginx、caddy……）：--webroot 目录，验证的文件写到 目录/.well-known/acme-challenge/ 里，让那个服务
  把这一段路径指到这个目录。nginx 的写法（^~ 让它排在别的按正则匹配的 location 前面，比如拦 “.” 开头的路径的那种）：
      location ^~ /.well-known/acme-challenge/ { root 目录; default_type text/plain; }

只用 Python 标准库和系统的 openssl 命令（生成密钥、签名、做证书请求）：标准库里没有签名。账户的私钥、证书的私钥都只在这台机器上，
证书的私钥续证书时不换（服务端、别的东西认的公钥不变）。

    python3 cca-acme.py --id 192.0.2.10 --out /cert --account /account --listen 0.0.0.0:18080

一台服务器上装了几个 Team 的同步服务器（server/hub-install.sh）时，80 端口只有一个：几个 Team 共用一个证书助手（cca-team-acme）——

    python3 cca-acme.py --teams /teams --store /certs --out /webcert --account /acme --listen 0.0.0.0:18080

--teams 目录里一个 Team 一个文件（文件名是它的服务名，内容是它的网页端的地址，安装脚本写的），每个地址要一张证书（几个 Team 用同一个
地址只要一张：同样的证书 7 天最多发 5 张），存在 --store/<种类>-<地址>/，再放进 --out/<服务名>/（安装脚本建好的这个 Team 的目录：
它的组、带 setgid，放进去的文件跟着它的组，只有这个 Team 的服务端读得了私钥）。目录里的文件变了过几秒就照新的来，不用重启。
手里没有用得了的证书、这个 Team 的目录里却有（以前它自己的证书助手要的，安装脚本搬过来的）时接着用它，不再要一张。

--once：现在该要、该续就做一次然后退出（退出码 0 是手里有一张用得了的证书；--teams 时是每个地址都有）。不带就一直运行：每小时看一眼，
证书过了一半有效期（6 天的证书是 80 小时：Let's Encrypt 对 10 天以内的证书的建议）就续；没成的隔 1 分钟、10 分钟、1 小时、6 小时
再试——同一个地址一小时最多验证失败 5 次、同样的证书 7 天最多发 5 张，所以不能试得更勤。"""
import argparse
import base64
import hashlib
import http.server
import ipaddress
import json
import os
import re
import socketserver
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request

LETSENCRYPT = 'https://acme-v02.api.letsencrypt.org/directory'
AGENT = 'cca-acme/1'
TOKEN_RE = re.compile(r'^[A-Za-z0-9_-]{1,256}\Z')   # \Z：$ 会放过结尾的换行（令牌要当文件名用：--webroot）
NAME_RE = re.compile(r'^(?=.{1,253}$)[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?(\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)+$')
TEAM_RE = re.compile(r'^[a-z0-9][a-z0-9-]{0,62}\Z')   # --teams 里的文件名：Team 的服务名（cca-team-<编号的前 8 位>）
RETRY = (60, 600, 3600, 6 * 3600)     # 没成以后隔多久再试（最后一个一直用）
POLL = 90                             # 等证书机构验证、签发最多多少秒
LOOK = 5                              # --teams：等着的时候隔几秒看一眼目录变了没有


class Problem(Exception):
    """没办成：原因是给人看的一句话"""


def log(text):
    sys.stderr.write('cca-acme: %s\n' % text)
    sys.stderr.flush()


def b64u(data):
    return base64.urlsafe_b64encode(data).rstrip(b'=').decode()


def openssl(*args, data=None):
    """运行 openssl，返回它的输出；没成抛 Problem（只说是哪一步，不带它打出来的内容：里面可能有路径）"""
    try:
        r = subprocess.run(('openssl',) + args, input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60)
    except (OSError, subprocess.TimeoutExpired) as e:
        raise Problem('运行不了 openssl（%s）：这台机器上要有 openssl 命令' % type(e).__name__)
    if r.returncode != 0:
        raise Problem('openssl %s 没成' % args[0])
    return r.stdout


def identifier(text):
    """命令行里写的一个地址 → ACME 的 identifier。IP 地址是 {"type": "ip"}（写成规范的样子），别的当域名"""
    try:
        ip = ipaddress.ip_address(text.strip('[]'))
    except ValueError:
        name = text.lower().rstrip('.')
        if not NAME_RE.match(name):
            raise Problem('%s 不是 IP 地址，也不是域名' % text)
        return {'type': 'dns', 'value': name}
    if ip.is_private or ip.is_loopback or ip.is_link_local or ip.is_multicast or ip.is_unspecified or ip.is_reserved:
        # 证书机构只给公网的地址签；测试时对着本机的假证书机构要签 127.0.0.1，由 --any-ip 放开
        if not identifier.any_ip:
            raise Problem('%s 不是公网的地址：证书机构只给从公网连得上的地址签证书' % ip)
    return {'type': 'ip', 'value': str(ip)}


identifier.any_ip = False


def put_file(path, data, mode):
    """换上一个文件：先写到旁边、改好权限，再换过去（读它的程序读不到半截）"""
    tmp = '%s.%d.tmp' % (path, os.getpid())
    try:
        with open(tmp, 'wb') as f:
            f.write(data)
        os.chmod(tmp, mode)
        os.replace(tmp, path)
    except OSError:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def read(path):
    try:
        with open(path, 'rb') as f:
            return f.read(1 << 20)
    except OSError:
        return None


def when(t):
    return time.strftime('%Y-%m-%d %H:%M UTC', time.gmtime(t))


class Key:
    """一把 P-256 的私钥（PEM 文件，只有自己读得了）。没有就生成（make 是 False 时不生成：没有就是 Problem）"""
    def __init__(self, path, make=True):
        self.path = path
        if not make and not os.path.isfile(path):
            raise Problem('没有 %s' % path)
        if not os.path.exists(path):
            old = os.umask(0o077)
            try:
                pem = openssl('genpkey', '-algorithm', 'EC', '-pkeyopt', 'ec_paramgen_curve:P-256')
                tmp = '%s.%d.tmp' % (path, os.getpid())
                with open(tmp, 'wb') as f:
                    f.write(pem)
                os.replace(tmp, path)
            finally:
                os.umask(old)
        # 公钥的那个点：DER 的最后 65 个字节（04 X Y）
        der = openssl('pkey', '-in', path, '-pubout', '-outform', 'DER')
        if len(der) < 65 or der[-65] != 4:
            raise Problem('%s 不是 P-256 的私钥' % path)
        self.x, self.y = der[-64:-32], der[-32:]
        self.spki = der

    def jwk(self):
        return {'crv': 'P-256', 'kty': 'EC', 'x': b64u(self.x), 'y': b64u(self.y)}

    def thumbprint(self):
        """RFC 7638：这几项按键名排好、不带空白的 JSON 的 SHA-256"""
        return b64u(hashlib.sha256(json.dumps(self.jwk(), sort_keys=True, separators=(',', ':')).encode()).digest())

    def sign(self, data):
        """ES256：openssl 出的是 DER（SEQUENCE { r, s }），JWS 要的是 r、s 各 32 个字节接在一起"""
        der = openssl('dgst', '-sha256', '-sign', self.path, data=data)
        try:
            if der[0] != 0x30:
                raise ValueError
            at = 2 + (der[1] & 0x7f if der[1] & 0x80 else 0)
            out = b''
            for _ in range(2):
                if der[at] != 0x02:
                    raise ValueError
                n = der[at + 1]
                out += der[at + 2:at + 2 + n].lstrip(b'\0').rjust(32, b'\0')
                at += 2 + n
            if len(out) != 64:
                raise ValueError
        except (IndexError, ValueError):
            raise Problem('openssl 签出来的看不懂')
        return out


class Responder(socketserver.ThreadingMixIn, http.server.HTTPServer):
    """http-01 的应答：证书机构来取 /.well-known/acme-challenge/<令牌> 时回“令牌.账户公钥的摘要”。别的一律 404，不记日志"""
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, addr):
        self.tokens = {}
        super().__init__(addr, Challenge)

    def put(self, token, body):
        self.tokens[token] = body

    def drop(self, token):
        self.tokens.pop(token, None)


class Webroot:
    """http-01 的文件放进一个目录（80 端口上已经有别的网站服务时，由它把 /.well-known/acme-challenge/ 指到这里）。
    文件只在验证的那一会儿在：验完就删"""
    def __init__(self, root):
        self.dir = os.path.join(root, '.well-known', 'acme-challenge')
        os.makedirs(self.dir, mode=0o755, exist_ok=True)

    def put(self, token, body):
        tmp = os.path.join(self.dir, '.%s.%d.tmp' % (token[:16], os.getpid()))
        with open(tmp, 'wb') as f:
            f.write(body)
        os.chmod(tmp, 0o644)   # 网站服务多半是另一个用户在跑：要它读得了
        os.replace(tmp, os.path.join(self.dir, token))

    def drop(self, token):
        try:
            os.unlink(os.path.join(self.dir, token))
        except OSError:
            pass


class Challenge(http.server.BaseHTTPRequestHandler):
    server_version = 'cca-acme'
    sys_version = ''
    timeout = 15

    def log_message(self, fmt, *args):
        pass

    def do_GET(self):
        prefix = '/.well-known/acme-challenge/'
        token = self.path[len(prefix):] if self.path.startswith(prefix) else ''
        body = self.server.tokens.get(token) if TOKEN_RE.match(token) else None
        self.send_response(200 if body else 404)
        self.send_header('Content-Type', 'text/plain')
        self.send_header('Content-Length', str(len(body or b'')))
        self.end_headers()
        self.wfile.write(body or b'')


class Acme:
    """和证书机构说话（RFC 8555）：每个请求都是拿账户的私钥签的 JWS"""
    def __init__(self, directory, key, ca=None):
        self.key, self.kid, self.nonce = key, None, None
        self.ctx = ssl.create_default_context(cafile=ca)
        status, _, self.dir = self.http(directory)
        if status != 200 or not isinstance(self.dir, dict) or not all(isinstance(self.dir.get(k), str) for k in ('newNonce', 'newAccount', 'newOrder')):
            raise Problem('证书机构的目录读不了（%s）' % status)

    def http(self, url, data=None, method=None):
        req = urllib.request.Request(url, data=data, method=method, headers={'User-Agent': AGENT, 'Content-Type': 'application/jose+json'})
        try:
            with urllib.request.urlopen(req, timeout=30, context=self.ctx) as r:
                status, headers, body = r.status, r.headers, r.read(1 << 20)
        except urllib.error.HTTPError as e:
            status, headers, body = e.code, e.headers, e.read(1 << 20)
        except (OSError, ValueError) as e:
            raise Problem('连不上证书机构（%s）' % type(e).__name__)
        if headers.get('Replay-Nonce'):
            self.nonce = headers['Replay-Nonce']
        try:
            parsed = json.loads(body.decode('utf-8')) if 'json' in (headers.get('Content-Type') or '') else body
        except ValueError:
            parsed = body
        return status, headers, parsed

    def post(self, url, payload):
        """payload 是 None：只是取（POST-as-GET）。nonce 过期了（badNonce）照新给的再发一次"""
        for attempt in range(3):
            if not self.nonce:
                self.http(self.dir['newNonce'], method='HEAD')
                if not self.nonce:
                    raise Problem('证书机构没给 nonce')
            protected = {'alg': 'ES256', 'nonce': self.nonce, 'url': url}
            protected.update({'kid': self.kid} if self.kid else {'jwk': self.key.jwk()})
            self.nonce = None
            p = b64u(json.dumps(protected).encode())
            d = '' if payload is None else b64u(json.dumps(payload).encode())
            body = json.dumps({'protected': p, 'payload': d, 'signature': b64u(self.key.sign(('%s.%s' % (p, d)).encode()))}).encode()
            status, headers, j = self.http(url, data=body)
            if status == 400 and isinstance(j, dict) and str(j.get('type', '')).endswith(':badNonce') and attempt < 2:
                continue
            return status, headers, j
        raise Problem('证书机构一直说 nonce 不对')

    @staticmethod
    def why(status, j):
        """证书机构说的原因：只取它的类型和 detail（给人看的那一句），限制长度、去掉控制字符"""
        if isinstance(j, dict):
            kind = str(j.get('type', '')).rsplit(':', 1)[-1]
            text = re.sub(r'[\x00-\x1f\x7f]', ' ', '%s %s' % (kind, j.get('detail', '')))[:300].strip()
            if text:
                return '%s（%s）' % (text, status)
        return '状态码 %s' % status

    def account(self):
        status, headers, j = self.post(self.dir['newAccount'], {'termsOfServiceAgreed': True})
        if status not in (200, 201) or not headers.get('Location'):
            raise Problem('证书机构没给开账户：' + self.why(status, j))
        self.kid = headers['Location']

    def wait(self, url, done, what):
        """一遍遍取 url，到 done(状态) 为止；状态成了 invalid 就是没成"""
        end = time.monotonic() + POLL
        while True:
            status, headers, j = self.post(url, None)
            if status != 200 or not isinstance(j, dict):
                raise Problem('%s：%s' % (what, self.why(status, j)))
            if j.get('status') == 'invalid':
                errs = [c.get('error') for c in j.get('challenges', []) if isinstance(c, dict) and c.get('error')] or [j.get('error')]
                raise Problem('%s没过：%s' % (what, self.why(status, errs[0]) if errs[0] else '证书机构没说原因'))
            if done(j.get('status')):
                return j
            if time.monotonic() > end:
                raise Problem('%s等了 %d 秒还没好' % (what, POLL))
            try:
                pause = float(headers.get('Retry-After') or 2)
            except ValueError:
                pause = 2
            time.sleep(min(max(pause, 1), 10))

    def issue(self, ids, profile, cert_key, responder):
        """要一张证书：返回 PEM（证书连同中间证书）"""
        if not self.kid:
            self.account()
        order = {'identifiers': ids}
        profiles = (self.dir.get('meta') or {}).get('profiles') if isinstance(self.dir.get('meta'), dict) else None
        if profile and isinstance(profiles, dict) and profile in profiles:
            order['profile'] = profile   # 证书机构说它有这种证书才写（没列出来的写了会被拒）
        elif any(i['type'] == 'ip' for i in ids):
            raise Problem('这个证书机构没有给 IP 地址签的那种证书（%s）' % profile)
        status, headers, j = self.post(self.dir['newOrder'], order)
        if status != 201 or not isinstance(j, dict) or not headers.get('Location'):
            raise Problem('证书机构没接这张单子：' + self.why(status, j))
        order_url = headers['Location']
        added = []
        try:
            for url in j.get('authorizations') or []:
                status, _, a = self.post(url, None)
                if status != 200 or not isinstance(a, dict):
                    raise Problem('验证的要求取不到：' + self.why(status, a))
                if a.get('status') == 'valid':
                    continue
                ch = next((c for c in a.get('challenges') or [] if isinstance(c, dict) and c.get('type') == 'http-01'), None)
                if not ch or not TOKEN_RE.match(str(ch.get('token', ''))) or not isinstance(ch.get('url'), str):
                    raise Problem('证书机构没给 http-01 这种验证办法')
                responder.put(ch['token'], ('%s.%s' % (ch['token'], self.key.thumbprint())).encode())
                added.append(ch['token'])
                status, _, c = self.post(ch['url'], {})
                if status != 200:
                    raise Problem('证书机构没开始验证：' + self.why(status, c))
                self.wait(url, lambda s: s == 'valid', '验证这个地址是你的（证书机构从公网连 80 端口）')
            with tempfile.TemporaryDirectory() as tmp:
                san = ', '.join(('IP:' if i['type'] == 'ip' else 'DNS:') + i['value'] for i in ids)
                with open(os.path.join(tmp, 'req.cnf'), 'w') as f:
                    # 证书里不要 Common Name（给 IP 签的不许有）：地址只写在 subjectAltName 里
                    f.write('[req]\ndistinguished_name = dn\nreq_extensions = ext\nprompt = no\n[dn]\n[ext]\nsubjectAltName = %s\n' % san)
                csr = openssl('req', '-new', '-key', cert_key.path, '-subj', '/', '-config', os.path.join(tmp, 'req.cnf'), '-outform', 'DER')
            status, _, f = self.post(j['finalize'], {'csr': b64u(csr)})
            if status != 200:
                raise Problem('证书机构没收证书请求：' + self.why(status, f))
            done = self.wait(order_url, lambda s: s == 'valid', '签发证书')
            status, _, pem = self.post(done.get('certificate') or '', None)
            if status != 200 or not isinstance(pem, bytes) or b'BEGIN CERTIFICATE' not in pem:
                raise Problem('证书取不下来：' + self.why(status, pem))
            return pem
        finally:
            for token in added:
                responder.drop(token)


def cert_state(path, ids, key):
    """手里的证书：(什么时候该续, 到期时间)；没有、看不懂、不是给这几个地址的、不是这把私钥的，是 None"""
    try:
        text = openssl('x509', '-in', path, '-noout', '-startdate', '-enddate', '-ext', 'subjectAltName', '-pubkey').decode('ascii', 'replace')
    except Problem:
        return None
    try:
        when = lambda k: time.mktime(time.strptime(re.search(r'^%s=(.+?) GMT$' % k, text, re.M).group(1), '%b %d %H:%M:%S %Y')) - time.timezone
        start, end = when('notBefore'), when('notAfter')
    except (AttributeError, ValueError, OverflowError):
        return None
    have = set()
    for kind, value in re.findall(r'(IP Address|DNS):([^,\s]+)', text):
        try:
            have.add(('ip', str(ipaddress.ip_address(value))) if kind == 'IP Address' else ('dns', value.lower()))
        except ValueError:
            return None
    if have != {(i['type'], i['value']) for i in ids}:
        return None
    pub = openssl('pkey', '-pubin', '-outform', 'DER', data=text[text.index('-----BEGIN PUBLIC KEY-----'):].encode())
    if pub != key.spki:
        return None
    return start + (end - start) * cert_state.fraction, end


cert_state.fraction = 0.5   # 过了有效期的这一半就续（10 天以内的证书 Let's Encrypt 建议的是一半）


def usable(chain, keyfile, ids):
    """这一对证书、私钥是给这几个地址的、还没过期：(什么时候该续, 到期时间)；不是（或者不在）是 None。私钥不在时不生成"""
    try:
        state = cert_state(chain, ids, Key(keyfile, make=False))
    except Problem:
        return None
    return state if state and time.time() < state[1] else None


def renew(acme, ids, profile, chain, cert_key, responder):
    """要一张新的、换上（fullchain.pem 别人读得了）：返回它的 (什么时候该续, 到期时间)"""
    pem = acme.issue(ids, profile, cert_key, responder)
    put_file(chain, pem, 0o644)
    names = '、'.join(i['value'] for i in ids)
    got = cert_state(chain, ids, cert_key)
    if not got:
        raise Problem('证书机构给的证书不是给 %s 的，或者不是这把私钥的' % names)
    log('拿到 %s 的证书，%s 到期' % (names, when(got[1])))
    return got


def serve_one(a, ids, account_key, responder):
    """--id：一张证书（上面是这几个地址），放在 --out 里"""
    os.makedirs(a.out, mode=0o700, exist_ok=True)
    cert_key = Key(os.path.join(a.out, 'privkey.pem'))
    chain = os.path.join(a.out, 'fullchain.pem')
    names = '、'.join(i['value'] for i in ids)
    fails, acme = 0, None
    while True:
        state = cert_state(chain, ids, cert_key)
        now = time.time()
        if state and now < state[0]:
            if a.once:
                return 0
            time.sleep(min(3600, max(60, state[0] - now)))
            continue
        try:
            acme = acme or Acme(a.directory, account_key, a.ca)
            renew(acme, ids, a.profile, chain, cert_key, responder)
            fails = 0
            if a.once:
                return 0
        except Problem as e:
            fails += 1
            pause = RETRY[min(fails, len(RETRY)) - 1]
            left = '，手里的那张 %s 到期' % when(state[1]) if state and now < state[1] else '，手里没有用得了的证书'
            log('%s 的证书没要到：%s%s。%d 分钟后再试' % (names, e, left, pause // 60))
            acme = None   # 账户、nonce 从头来
            if a.once:
                return 1
            time.sleep(pause)


class Slot:
    """--teams 时的一个地址：它的证书、私钥存在 --store/<种类>-<地址>/（IPv6 的冒号写成下划线），没成了几次、什么时候再试"""
    def __init__(self, store, ident):
        self.ident = ident
        self.dir = os.path.join(store, '%s-%s' % (ident['type'], ident['value'].replace(':', '_')))
        self.chain, self.key = os.path.join(self.dir, 'fullchain.pem'), os.path.join(self.dir, 'privkey.pem')
        self.fails, self.retry = 0, 0.0


def registry(path, said):
    """--teams 目录里登记的：{Team: identifier}。看不懂的文件说一次（said 记着说过的）、跳过"""
    teams = {}
    for name in sorted(os.listdir(path)):
        p = os.path.join(path, name)
        if not TEAM_RE.match(name) or not os.path.isfile(p):
            continue   # 安装脚本正写到一半的（.new.<进程号>）这些
        try:
            with open(p, encoding='utf-8') as f:
                teams[name] = identifier(f.read(512).strip())
            said.pop(name, None)
        except (OSError, UnicodeDecodeError, Problem) as e:
            if said.get(name) != str(e):
                said[name] = str(e)
                log('%s 登记的地址用不了：%s' % (name, e))
    return teams


def stamp(path):
    """--teams 目录现在的样子（文件名、修改时间、大小）：变了就照新的来"""
    try:
        return tuple(sorted((n, s.st_mtime_ns, s.st_size) for n in os.listdir(path) for s in [os.stat(os.path.join(path, n))]))
    except OSError:
        return None


def give(d, slot):
    """这个地址的证书、私钥放进一个 Team 的目录：不一样才换，私钥先换（服务端看到两个都换了才重新读）。目录不在（安装脚本还没建，
    或者已经去掉了这个 Team）返回 False"""
    if not os.path.isdir(d):
        return False
    for name, mode in (('privkey.pem', 0o640), ('fullchain.pem', 0o644)):
        want = read(os.path.join(slot.dir, name))
        if want is not None and read(os.path.join(d, name)) != want:
            put_file(os.path.join(d, name), want, mode)
    return True


def serve_teams(a, account_key, responder):
    """--teams：照目录里登记的 Team，每个地址一张证书，放进用它的那几个 Team 的目录；目录变了过几秒就照新的来"""
    slots, said, missing, acme = {}, {}, set(), None
    os.makedirs(a.store, mode=0o700, exist_ok=True)
    while True:
        seen = stamp(a.teams)
        try:
            teams = registry(a.teams, said)
        except OSError as e:
            log('读不了 %s（%s）' % (a.teams, e.strerror or e))
            teams = {}
        users = {}   # 地址 → 用它的那几个 Team
        for team, ident in teams.items():
            users.setdefault((ident['type'], ident['value']), []).append(team)
        wake, ready = time.time() + 3600, True
        for k, names in sorted(users.items()):
            s = slots.get(k) or slots.setdefault(k, Slot(a.store, teams[names[0]]))
            ids, value = [s.ident], s.ident['value']
            try:
                os.makedirs(s.dir, mode=0o700, exist_ok=True)
                got = usable(s.chain, s.key, ids)
                if not got:
                    # 手里没有用得了的：这几个 Team 的目录里有（以前它自己的证书助手要的，安装脚本搬过来的）就接着用，不再要一张
                    for team in names:
                        d = os.path.join(a.out, team)
                        got = usable(os.path.join(d, 'fullchain.pem'), os.path.join(d, 'privkey.pem'), ids)
                        if got:
                            put_file(s.key, read(os.path.join(d, 'privkey.pem')), 0o600)
                            put_file(s.chain, read(os.path.join(d, 'fullchain.pem')), 0o644)
                            log('%s 接着用 %s 原来的证书（%s 到期）' % (value, team, when(got[1])))
                            break
                now = time.time()
                if (not got or now >= got[0]) and now >= s.retry:
                    try:
                        acme = acme or Acme(a.directory, account_key, a.ca)
                        got = renew(acme, ids, a.profile, s.chain, Key(s.key), responder)
                        s.fails = 0
                    except Problem as e:
                        s.fails += 1
                        pause = RETRY[min(s.fails, len(RETRY)) - 1]
                        s.retry = time.time() + pause
                        left = '，手里的那张 %s 到期' % when(got[1]) if got else '，手里没有用得了的证书'
                        log('%s 的证书没要到（%s 用）：%s%s。%d 分钟后再试' % (value, '、'.join(names), e, left, pause // 60))
                        acme = None   # 账户、nonce 从头来
                due = got[0] if got and time.time() < got[0] else s.retry
                wake = min(wake, max(due, time.time() + 1))
                if not got:
                    ready = False
                    continue
                for team in names:
                    if give(os.path.join(a.out, team), s):
                        missing.discard(team)
                        continue
                    ready = False
                    if team not in missing:
                        missing.add(team)
                        log('%s 的目录 %s 不在：证书放不进去（安装脚本建它）' % (team, os.path.join(a.out, team)))
            except (OSError, Problem) as e:
                ready = False
                wake = min(wake, time.time() + 60)
                log('%s 的证书放不好：%s' % (value, e))
        if a.once:
            return 0 if ready else 1
        # 睡到该续、该再试的时候（最多一小时）；登记的变了（安装脚本加了、去掉了一个 Team）过几秒就醒
        while time.time() < wake and stamp(a.teams) == seen:
            time.sleep(min(LOOK, max(0.2, wake - time.time())))


def main():
    ap = argparse.ArgumentParser(description='给 cca 的网页端申请、续上浏览器信得过的证书（ACME，http-01）')
    ap.add_argument('--id', action='append', help='给哪个地址签：公网的 IP 地址，或者域名（可以写几次：一张证书上几个地址）')
    ap.add_argument('--teams', help='几个 Team 共用这一个证书助手：这个目录里一个 Team 一个文件（文件名是服务名，内容是它的网页端的地址），'
                    '每个地址一张证书，放进 --out/<服务名>/；和 --id 只能用一种')
    ap.add_argument('--store', help='--teams 时每个地址的证书、私钥存在这里（一个地址一个子目录）')
    ap.add_argument('--out', required=True, help='证书放在这个目录里：fullchain.pem、privkey.pem（--teams 时这里每个 Team 一个子目录）')
    ap.add_argument('--account', required=True, help='账户的私钥放在这个目录里（account.key；留着它，续证书时用的是同一个账户）')
    ap.add_argument('--listen', default='0.0.0.0:18080', help='应答证书机构验证的端口（公网的 80 端口要指到它；默认 0.0.0.0:18080）')
    ap.add_argument('--webroot', help='不自己听端口：验证的文件写到 这个目录/.well-known/acme-challenge/ 里，由 80 端口上已有的网站服务给出去')
    ap.add_argument('--directory', default=LETSENCRYPT, help='证书机构的 ACME 目录（默认是 Let\'s Encrypt）')
    ap.add_argument('--profile', default='shortlived', help='要哪一种证书（默认 shortlived：约 6 天，给 IP 地址签的只有这一种）；空的是证书机构默认的')
    ap.add_argument('--once', action='store_true', help='该要、该续就做一次，然后退出')
    ap.add_argument('--ca', help='测试用：照这个证书机构验 ACME 服务器的证书')
    ap.add_argument('--any-ip', action='store_true', help='测试用：不是公网的地址也去要')
    ap.add_argument('--port-file', help='测试用：实际监听的端口写到这里')
    ap.add_argument('--renew-at', type=float, default=0.5, help='测试用：证书过了有效期的这么多（0～1）就续，默认 0.5')
    a = ap.parse_args()
    if bool(a.id) == bool(a.teams) or bool(a.teams) != bool(a.store):
        ap.error('要给 --id（一张证书），或者 --teams 连同 --store（几个 Team 共用）')
    identifier.any_ip = a.any_ip
    cert_state.fraction = min(max(a.renew_at, 0.0), 0.9)
    try:
        ids = [identifier(x) for x in dict.fromkeys(a.id or [])]
        os.makedirs(a.account, mode=0o700, exist_ok=True)
        account_key = Key(os.path.join(a.account, 'account.key'))
        if a.teams and not os.path.isdir(a.teams):
            raise Problem('没有 %s' % a.teams)
        if a.webroot:
            responder = Webroot(a.webroot)
        else:
            host, _, port = a.listen.rpartition(':')
            responder = Responder((host.strip('[]') or '0.0.0.0', int(port)))
    except (Problem, OSError, ValueError) as e:
        log('起不来：%s' % e)
        return 2
    if not a.webroot:
        if a.port_file:
            with open(a.port_file + '.tmp', 'w') as f:
                f.write('%d\n' % responder.server_address[1])
            os.replace(a.port_file + '.tmp', a.port_file)
        threading.Thread(target=responder.serve_forever, name='http-01', daemon=True).start()
    try:
        return serve_teams(a, account_key, responder) if a.teams else serve_one(a, ids, account_key, responder)
    except (Problem, OSError) as e:
        log('起不来：%s' % e)
        return 2


if __name__ == '__main__':
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
