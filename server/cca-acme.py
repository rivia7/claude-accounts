#!/usr/bin/env python3
"""cca 的证书助手：给远程会话的网页端（手机浏览器里的那一页，README「网页端」）申请、续上浏览器信得过的证书。

手机的浏览器只认证书机构签的证书。这个程序照 ACME（RFC 8555）向 Let's Encrypt 要一张——给 IP 地址签的（RFC 8738：不用域名，
只有约 6 天的短期证书这一种）或者给域名签的——放到一个目录里（fullchain.pem、privkey.pem），到期之前自己续上。服务端
（cca-presence.py 的 --web-cert / --web-key）发现文件换了会自己重新读，不用重启。

验证只用 http-01：证书机构从公网连这台机器的 **80 端口**，取 /.well-known/acme-challenge/<令牌>（给 IP 签的证书不能用 DNS 验证）。
所以这个程序自己一直听着一个端口（--listen，部署时把 80 端口指到它），别的路径一律 404；80 端口要从公网连得上。

只用 Python 标准库和系统的 openssl 命令（生成密钥、签名、做证书请求）：标准库里没有签名。账户的私钥、证书的私钥都只在这台机器上，
证书的私钥续证书时不换（服务端、别的东西认的公钥不变）。

    python3 cca-acme.py --id 192.0.2.10 --out /cert --account /account --listen 0.0.0.0:18080

--once：现在该要、该续就做一次然后退出（退出码 0 是手里有一张用得了的证书）。不带就一直运行：每小时看一眼，证书过了一半有效期
（6 天的证书是 80 小时：Let's Encrypt 对 10 天以内的证书的建议）就续；没成的隔 1 分钟、10 分钟、1 小时、6 小时再试——
同一个地址一小时最多验证失败 5 次、同样的证书 7 天最多发 5 张，所以不能试得更勤。"""
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
TOKEN_RE = re.compile(r'^[A-Za-z0-9_-]{1,256}$')
NAME_RE = re.compile(r'^(?=.{1,253}$)[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?(\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)+$')
RETRY = (60, 600, 3600, 6 * 3600)     # 没成以后隔多久再试（最后一个一直用）
POLL = 90                             # 等证书机构验证、签发最多多少秒


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


class Key:
    """一把 P-256 的私钥（PEM 文件，只有自己读得了）。没有就生成"""
    def __init__(self, path):
        self.path = path
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
                responder.tokens[ch['token']] = ('%s.%s' % (ch['token'], self.key.thumbprint())).encode()
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
                responder.tokens.pop(token, None)


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


def main():
    ap = argparse.ArgumentParser(description='给 cca 的网页端申请、续上浏览器信得过的证书（ACME，http-01）')
    ap.add_argument('--id', action='append', required=True, help='给哪个地址签：公网的 IP 地址，或者域名（可以写几次：一张证书上几个地址）')
    ap.add_argument('--out', required=True, help='证书放在这个目录里：fullchain.pem、privkey.pem')
    ap.add_argument('--account', required=True, help='账户的私钥放在这个目录里（account.key；留着它，续证书时用的是同一个账户）')
    ap.add_argument('--listen', default='0.0.0.0:18080', help='应答证书机构验证的端口（公网的 80 端口要指到它；默认 0.0.0.0:18080）')
    ap.add_argument('--directory', default=LETSENCRYPT, help='证书机构的 ACME 目录（默认是 Let\'s Encrypt）')
    ap.add_argument('--profile', default='shortlived', help='要哪一种证书（默认 shortlived：约 6 天，给 IP 地址签的只有这一种）；空的是证书机构默认的')
    ap.add_argument('--once', action='store_true', help='该要、该续就做一次，然后退出')
    ap.add_argument('--ca', help='测试用：照这个证书机构验 ACME 服务器的证书')
    ap.add_argument('--any-ip', action='store_true', help='测试用：不是公网的地址也去要')
    ap.add_argument('--port-file', help='测试用：实际监听的端口写到这里')
    ap.add_argument('--renew-at', type=float, default=0.5, help='测试用：证书过了有效期的这么多（0～1）就续，默认 0.5')
    a = ap.parse_args()
    identifier.any_ip = a.any_ip
    cert_state.fraction = min(max(a.renew_at, 0.0), 0.9)
    try:
        ids = [identifier(x) for x in dict.fromkeys(a.id)]
        for d in (a.out, a.account):
            os.makedirs(d, mode=0o700, exist_ok=True)
        account_key, cert_key = Key(os.path.join(a.account, 'account.key')), Key(os.path.join(a.out, 'privkey.pem'))
        host, _, port = a.listen.rpartition(':')
        responder = Responder((host.strip('[]') or '0.0.0.0', int(port)))
    except (Problem, OSError, ValueError) as e:
        log('起不来：%s' % e)
        return 2
    if a.port_file:
        with open(a.port_file + '.tmp', 'w') as f:
            f.write('%d\n' % responder.server_address[1])
        os.replace(a.port_file + '.tmp', a.port_file)
    threading.Thread(target=responder.serve_forever, name='http-01', daemon=True).start()
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
            pem = acme.issue(ids, a.profile, cert_key, responder)
            tmp = '%s.%d.tmp' % (chain, os.getpid())
            with open(tmp, 'wb') as f:
                f.write(pem)
            os.chmod(tmp, 0o644)
            os.replace(tmp, chain)
            got = cert_state(chain, ids, cert_key)
            if not got:
                raise Problem('证书机构给的证书不是给 %s 的，或者不是这把私钥的' % names)
            log('拿到 %s 的证书，%s 到期' % (names, time.strftime('%Y-%m-%d %H:%M', time.gmtime(got[1])) + ' UTC'))
            fails = 0
            if a.once:
                return 0
        except Problem as e:
            fails += 1
            pause = RETRY[min(fails, len(RETRY)) - 1]
            left = '，手里的那张 %s 到期' % time.strftime('%Y-%m-%d %H:%M UTC', time.gmtime(state[1])) if state and now < state[1] else '，手里没有用得了的证书'
            log('%s 的证书没要到：%s%s。%d 分钟后再试' % (names, e, left, pause // 60))
            acme = None   # 账户、nonce 从头来
            if a.once:
                return 1
            time.sleep(pause)


if __name__ == '__main__':
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
