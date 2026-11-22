#!/usr/bin/env python3
"""假的证书机构（ACME，RFC 8555 + RFC 8738 + 证书种类 profile），给 tests/acme.py 测 server/cca-acme.py 用。不是随便回几句的桩：
客户端的每个请求都照规范核对——JWS 的签名（拿 openssl 验）、nonce 只能用一次、url 要对、开账户用 jwk 之后用 kid、单子里的地址和证书种类、
http-01 是真的去取那个文件比对内容、证书请求里的地址要和单子上的一样而且没有 Common Name——不对就照规范回错误。签出来的证书由
临时的测试机构签（证书里的地址照单子上的）。

    python3 acme_fake.py 目录 应答端口 [--bad-nonce N] [--no-shortlived]
目录里要有 ca.pem、ca.key（签证书的）、cert.pem、key.pem（这个服务自己的 HTTPS 证书）；实际监听的端口写到 目录/port，
做过的事一行一条 JSON 追加到 目录/log（测试读它）。应答端口：去哪个端口取 http-01 的文件（真的证书机构是 80）。
--bad-nonce N：每 N 个请求说一次 nonce 不对（客户端应该自己再发）；--no-shortlived：目录里不列 shortlived 这一种。"""
import base64
import hashlib
import http.client
import http.server
import json
import os
import re
import secrets
import socketserver
import ssl
import subprocess
import sys
import tempfile
import threading

P256_SPKI = bytes.fromhex('3059301306072a8648ce3d020106082a8648ce3d030107034200')
b64u = lambda b: base64.urlsafe_b64encode(b).rstrip(b'=').decode()


def unb64u(s):
    if not isinstance(s, str) or not re.match(r'^[A-Za-z0-9_-]*$', s):
        raise ValueError('不是 base64url')
    return base64.urlsafe_b64decode(s + '=' * (-len(s) % 4))


def openssl(*args, data=None):
    return subprocess.run(('openssl',) + args, input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)


class Refuse(Exception):
    def __init__(self, status, kind, detail):
        self.status, self.kind, self.detail = status, kind, detail


class CA:
    def __init__(self, d, http_port, bad_nonce, shortlived):
        self.d, self.http_port, self.bad_nonce, self.shortlived = d, http_port, bad_nonce, shortlived
        self.lock = threading.Lock()
        self.nonces, self.accounts, self.orders, self.authz, self.certs = set(), {}, {}, {}, {}
        self.requests = 0

    def log(self, **what):
        with self.lock, open(os.path.join(self.d, 'log'), 'a') as f:
            f.write(json.dumps(what, ensure_ascii=False) + '\n')

    def nonce(self):
        n = secrets.token_urlsafe(16)
        with self.lock:
            self.nonces.add(n)
        return n

    def verify(self, url, body):
        """核对一个 JWS：返回 (账户, 内容)；内容是 None 表示 POST-as-GET"""
        try:
            j = json.loads(body)
            protected = json.loads(unb64u(j['protected']))
            sig = unb64u(j['signature'])
            payload = j['payload']
            data = None if payload == '' else json.loads(unb64u(payload))
        except (ValueError, KeyError, TypeError):
            raise Refuse(400, 'malformed', 'JWS 看不懂')
        if protected.get('alg') != 'ES256' or protected.get('url') != url:
            raise Refuse(400, 'malformed', 'alg 或者 url 不对')
        with self.lock:
            self.requests += 1
            fresh = protected.get('nonce') in self.nonces
            self.nonces.discard(protected.get('nonce'))
            flaky = self.bad_nonce and self.requests % self.bad_nonce == 0
        if not fresh or flaky:
            raise Refuse(400, 'badNonce', 'nonce 不对')
        if ('jwk' in protected) == ('kid' in protected):
            raise Refuse(400, 'malformed', 'jwk 和 kid 要有且只有一个')
        if 'jwk' in protected:
            jwk, acct = protected['jwk'], None
        else:
            acct = self.accounts.get(protected['kid'])
            if not acct:
                raise Refuse(400, 'accountDoesNotExist', '没有这个账户')
            jwk = acct['jwk']
        try:
            if (jwk.get('kty'), jwk.get('crv')) != ('EC', 'P-256'):
                raise ValueError
            x, y = unb64u(jwk['x']), unb64u(jwk['y'])
            if (len(x), len(y), len(sig)) != (32, 32, 64):
                raise ValueError
        except (ValueError, KeyError, AttributeError):
            raise Refuse(400, 'badPublicKey', '公钥或者签名的样子不对')
        ints = [sig[:32].lstrip(b'\0') or b'\0', sig[32:].lstrip(b'\0') or b'\0']
        der = b''.join(b'\x02' + bytes([len(i) + (i[0] >> 7)]) + (b'\0' if i[0] >> 7 else b'') + i for i in ints)
        der = b'\x30' + bytes([len(der)]) + der
        with tempfile.TemporaryDirectory() as tmp:
            pub, sigf = os.path.join(tmp, 'pub.der'), os.path.join(tmp, 'sig')
            with open(pub, 'wb') as f:
                f.write(P256_SPKI + b'\x04' + x + y)
            with open(sigf, 'wb') as f:
                f.write(der)
            ok = openssl('dgst', '-sha256', '-verify', pub, '-keyform', 'DER', '-signature', sigf,
                         data=('%s.%s' % (j['protected'], j['payload'])).encode()).returncode == 0
        if not ok:
            raise Refuse(403, 'unauthorized', '签名不对')
        return (acct, jwk, protected.get('kid')), data


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def log_message(self, fmt, *args):
        pass

    @property
    def base(self):
        return 'https://127.0.0.1:%d' % self.server.server_address[1]

    def reply(self, status, obj=None, headers=None, raw=None, ctype='application/json'):
        body = raw if raw is not None else b'' if obj is None else json.dumps(obj).encode()
        self.send_response(status)
        self.send_header('Replay-Nonce', self.server.ca.nonce())
        self.send_header('Content-Type', ctype)
        self.send_header('Content-Length', str(len(body)))
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        if self.command != 'HEAD':
            self.wfile.write(body)

    def do_HEAD(self):
        self.reply(200 if self.path == '/nonce' else 404)

    def do_GET(self):
        ca = self.server.ca
        if self.path == '/dir':
            profiles = {'classic': self.base + '/docs#classic'}
            if ca.shortlived:
                profiles['shortlived'] = self.base + '/docs#shortlived'
            return self.reply(200, {'newNonce': self.base + '/nonce', 'newAccount': self.base + '/acct', 'newOrder': self.base + '/order',
                                    'meta': {'profiles': profiles}})
        self.reply(204 if self.path == '/nonce' else 404)

    def do_POST(self):
        ca = self.server.ca
        try:
            if self.headers.get('Content-Type') != 'application/jose+json':
                raise Refuse(415, 'malformed', 'Content-Type 要是 application/jose+json')
            body = self.rfile.read(int(self.headers.get('Content-Length') or 0))
            (acct, jwk, kid), data = ca.verify(self.base + self.path, body)
            self.route(ca, acct, jwk, kid, data)
        except Refuse as e:
            ca.log(refused=e.kind, path=self.path)
            self.reply(e.status, {'type': 'urn:ietf:params:acme:error:' + e.kind, 'detail': e.detail}, ctype='application/problem+json')

    def route(self, ca, acct, jwk, kid, data):
        path = self.path
        if path == '/acct':
            if acct or not isinstance(data, dict) or data.get('termsOfServiceAgreed') is not True:
                raise Refuse(400, 'malformed', '开账户要用 jwk，并且同意条款')
            thumb = b64u(hashlib.sha256(json.dumps({k: jwk[k] for k in ('crv', 'kty', 'x', 'y')}, sort_keys=True, separators=(',', ':')).encode()).digest())
            url = self.base + '/acct/' + thumb[:16]
            with ca.lock:
                new = url not in ca.accounts
                ca.accounts[url] = {'jwk': jwk, 'thumb': thumb}
            ca.log(account=thumb, new=new)
            return self.reply(201 if new else 200, {'status': 'valid'}, {'Location': url})
        if not acct:
            raise Refuse(400, 'malformed', '这个请求要用 kid')
        if path == '/order':
            ids = data.get('identifiers') if isinstance(data, dict) else None
            if not (isinstance(ids, list) and ids and all(isinstance(i, dict) and i.get('type') in ('ip', 'dns') and isinstance(i.get('value'), str) for i in ids)):
                raise Refuse(400, 'malformed', '单子上的地址不对')
            profile = data.get('profile')
            if profile is not None and (profile not in ('classic', 'shortlived') or (profile == 'shortlived' and not ca.shortlived)):
                raise Refuse(400, 'invalidProfile', '没有这一种证书')
            if any(i['type'] == 'ip' for i in ids) and profile != 'shortlived':
                raise Refuse(400, 'rejectedIdentifier', '给 IP 地址签的只有 shortlived 这一种')
            with ca.lock:
                n = str(len(ca.orders) + 1)
                authz = []
                for i in ids:
                    a = '%s-%d' % (n, len(authz))
                    ca.authz[a] = {'identifier': i, 'status': 'pending', 'token': secrets.token_urlsafe(24), 'kid': kid, 'error': None}
                    authz.append(a)
                ca.orders[n] = {'ids': ids, 'authz': authz, 'profile': profile, 'status': 'pending', 'kid': kid}
            ca.log(order=n, ids=ids, profile=profile)
            return self.reply(201, self.order_doc(ca, n), {'Location': self.base + '/o/' + n})
        kind, _, n = path.strip('/').partition('/')
        if kind == 'o' and n in ca.orders and ca.orders[n]['kid'] == kid and data is None:
            return self.reply(200, self.order_doc(ca, n))
        if kind in ('authz', 'chall') and n in ca.authz and ca.authz[n]['kid'] == kid:
            a = ca.authz[n]
            if kind == 'chall':
                if data != {}:
                    raise Refuse(400, 'malformed', '开始验证要发 {}')
                if a['status'] == 'pending':
                    self.validate(ca, a, acct['thumb'])
            elif data is not None:
                raise Refuse(400, 'malformed', '取验证的要求是 POST-as-GET')
            ch = {'type': 'http-01', 'url': self.base + '/chall/' + n, 'token': a['token'], 'status': a['status']}
            if a['error']:
                ch['error'] = {'type': 'urn:ietf:params:acme:error:unauthorized', 'detail': a['error']}
            return self.reply(200, ch if kind == 'chall' else {'status': a['status'], 'identifier': a['identifier'], 'challenges': [ch]})
        if kind == 'fin' and n in ca.orders and ca.orders[n]['kid'] == kid:
            return self.finalize(ca, n, data)
        if kind == 'cert' and n in ca.certs and ca.orders[n]['kid'] == kid and data is None:
            return self.reply(200, raw=ca.certs[n], ctype='application/pem-certificate-chain')
        raise Refuse(404, 'malformed', '没有这个地址')

    def order_doc(self, ca, n):
        o = ca.orders[n]
        if o['status'] == 'pending' and all(ca.authz[a]['status'] == 'valid' for a in o['authz']):
            o['status'] = 'ready'
        if any(ca.authz[a]['status'] == 'invalid' for a in o['authz']):
            o['status'] = 'invalid'
        doc = {'status': o['status'], 'identifiers': o['ids'], 'authorizations': [self.base + '/authz/' + a for a in o['authz']],
               'finalize': self.base + '/fin/' + n}
        if o['profile']:
            doc['profile'] = o['profile']
        if o['status'] == 'valid':
            doc['certificate'] = self.base + '/cert/' + n
        return doc

    def validate(self, ca, a, thumb):
        """http-01：从“公网”（这里是本机的应答端口）取那个文件，Host 是要验证的地址，内容要是 令牌.账户公钥的摘要"""
        want = ('%s.%s' % (a['token'], thumb)).encode()
        try:
            c = http.client.HTTPConnection('127.0.0.1', ca.http_port, timeout=5)
            c.request('GET', '/.well-known/acme-challenge/' + a['token'], headers={'Host': a['identifier']['value']})
            r = c.getresponse()
            got = r.read(4096)
            c.close()
            ok = r.status == 200 and got.strip() == want
            why = None if ok else '取到的内容不对（状态码 %d）' % r.status
        except OSError as e:
            ok, why = False, '连不上 80 端口（%s）' % type(e).__name__
        a['status'], a['error'] = ('valid' if ok else 'invalid'), why
        ca.log(validated=a['identifier']['value'], ok=ok)

    def finalize(self, ca, n, data):
        o = ca.orders[n]
        self.order_doc(ca, n)
        if o['status'] != 'ready':
            raise Refuse(403, 'orderNotReady', '地址还没验证完')
        try:
            csr = unb64u(data['csr'])
        except (ValueError, KeyError, TypeError):
            raise Refuse(400, 'malformed', '没有证书请求')
        text = openssl('req', '-inform', 'DER', '-noout', '-text', '-verify', data=csr)
        if text.returncode != 0:
            raise Refuse(400, 'badCSR', '证书请求看不懂，或者它自己的签名不对')
        out = text.stdout.decode('utf-8', 'replace')
        subject = re.search(r'^\s*Subject:(.*)$', out, re.M)
        if subject and 'CN' in subject.group(1):
            raise Refuse(400, 'badCSR', '证书请求里不该有 Common Name')
        have = sorted(('ip', v) if k == 'IP Address' else ('dns', v.lower()) for k, v in re.findall(r'(IP Address|DNS):([^,\s]+)', out))
        if have != sorted((i['type'], i['value']) for i in o['ids']):
            raise Refuse(400, 'badCSR', '证书请求里的地址和单子上的不一样')
        with tempfile.TemporaryDirectory() as tmp:
            req, ext, crt = (os.path.join(tmp, x) for x in ('csr.der', 'ext.cnf', 'cert.pem'))
            with open(req, 'wb') as f:
                f.write(csr)
            with open(ext, 'w') as f:
                f.write('subjectAltName=critical,%s\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n'
                        % ','.join(('IP:' if t == 'ip' else 'DNS:') + v for t, v in have))
            r = openssl('x509', '-req', '-inform', 'DER', '-in', req, '-CA', os.path.join(ca.d, 'ca.pem'), '-CAkey', os.path.join(ca.d, 'ca.key'),
                        '-set_serial', '0x' + secrets.token_hex(8), '-days', '7' if o['profile'] == 'shortlived' else '90', '-extfile', ext, '-out', crt)
            if r.returncode != 0:
                raise Refuse(500, 'serverInternal', '签不出来')
            with open(crt, 'rb') as f, open(os.path.join(ca.d, 'ca.pem'), 'rb') as g:
                ca.certs[n] = f.read() + g.read()
        o['status'] = 'valid'
        ca.log(issued=n, ids=o['ids'], profile=o['profile'])
        self.reply(200, self.order_doc(ca, n))


class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True


def main():
    d, http_port = sys.argv[1], int(sys.argv[2])
    rest = sys.argv[3:]
    bad = int(rest[rest.index('--bad-nonce') + 1]) if '--bad-nonce' in rest else 0
    srv = Server(('127.0.0.1', 0), Handler)
    srv.ca = CA(d, http_port, bad, '--no-shortlived' not in rest)
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(os.path.join(d, 'cert.pem'), os.path.join(d, 'key.pem'))
    srv.socket = ctx.wrap_socket(srv.socket, server_side=True)
    with open(os.path.join(d, 'port.tmp'), 'w') as f:
        f.write('%d\n' % srv.server_address[1])
    os.replace(os.path.join(d, 'port.tmp'), os.path.join(d, 'port'))
    srv.serve_forever()


if __name__ == '__main__':
    main()
