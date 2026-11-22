#!/usr/bin/env python3
"""网页端（手机浏览器接入远程会话：docs/dev/remote.md「网页端」、server/README.md「网页端」）在服务器这一头：
不在设备清单里的访客拿配对码配对、它能发和不能发的请求、WebSocket 进中转、页面的文件、另开的那个带证书机构签的证书的端口。

真的服务器进程；扮演手机的是这里照文档写的一份（Guest、WsSock：ssh-keygen 签名、自己说 WebSocket），另一头是设备清单里的
设备（照文档自己说话，或者真的 cca：见 ClientTest）。浏览器里跑的那一份（server/web/core.js）由 tests/web_client.mjs 测。不联网。"""
import base64
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import socket
import ssl
import struct
import subprocess
import sys
import tempfile
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from presence import ROOT, keygen, make_ca_cert, sign, srvmod, wait_for   # noqa: E402
import remote_relay as rr   # noqa: E402
from remote_relay import Base, F_ANNOUNCE, F_JOIN, SOCKS, ctl, dial, read_ctl, read_exact, read_line   # noqa: E402
import spec_wire as sw   # noqa: E402

WEB = ROOT / 'server' / 'web'


def ca_ctx(ca):
    """照测试的证书机构验证书（名字、链都要对）。测试的机构证书是 openssl 一条命令出的、没有“密钥用途”那一项：
    新的 Python 默认连这个也查（VERIFY_X509_STRICT），这里只放开这一条"""
    ctx = ssl.create_default_context(cafile=str(ca))
    ctx.verify_flags &= ~getattr(ssl, 'VERIFY_X509_STRICT', 0)
    return ctx


class WsSock:
    """照 RFC 6455 自己说的 WebSocket 客户端，用起来像一条连接（sendall / recv）：发的装成加了掩码的二进制帧，
    收的把帧里的内容接成字节流。closed：服务器发了关闭帧、或者连接断了"""
    def __init__(self, port, ca, origin=None, path='/v1/relay/ws', headers=None):
        ctx = ca_ctx(ca)
        raw = socket.create_connection(('127.0.0.1', port), timeout=20)
        self.s = ctx.wrap_socket(raw, server_hostname='127.0.0.1')
        SOCKS.append(self.s)
        key = base64.b64encode(os.urandom(16)).decode()
        h = {'Host': '127.0.0.1:%d' % port, 'Upgrade': 'websocket', 'Connection': 'Upgrade', 'Sec-WebSocket-Key': key,
             'Sec-WebSocket-Version': '13'}
        if origin:
            h['Origin'] = origin
        h.update(headers or {})
        self.s.sendall(('GET %s HTTP/1.1\r\n' % path + ''.join('%s: %s\r\n' % kv for kv in h.items() if kv[1] is not None) + '\r\n').encode())
        head = b''
        while b'\r\n\r\n' not in head:
            c = self.s.recv(1)
            if not c:
                break
            head += c
        self.status = int(head.split(b' ', 2)[1]) if head.count(b' ') >= 2 else 0
        self.head = head.decode('latin-1')
        self.accept_ok = ('Sec-WebSocket-Accept: %s' % base64.b64encode(
            hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()) in self.head
        self.buf, self.closed, self.pongs = bytearray(), False, []

    def frame(self, op, data, mask=True, fin=True):
        n = len(data)
        head = bytes([(0x80 if fin else 0) | op])
        m = 0x80 if mask else 0
        head += bytes([m | n]) if n < 126 else bytes([m | 126]) + struct.pack('>H', n) if n < 65536 else bytes([m | 127]) + struct.pack('>Q', n)
        if not mask:
            return head + data
        k = os.urandom(4)
        return head + k + bytes(b ^ k[i % 4] for i, b in enumerate(data)) if n < 4096 else \
            head + k + (int.from_bytes(data, 'big') ^ int.from_bytes((k * (n // 4 + 1))[:n], 'big')).to_bytes(n, 'big')

    def sendall(self, data):
        self.s.sendall(self.frame(2, data))

    def settimeout(self, t):
        self.s.settimeout(t)

    def _exact(self, n):
        out = b''
        while len(out) < n:
            c = self.s.recv(n - len(out))
            if not c:
                raise EOFError
            out += c
        return out

    def recv(self, n):
        """和 socket 的一样：有多少给多少（最多 n），那头关了是 b''"""
        while not self.buf and not self.closed:
            try:
                b0, b1 = self._exact(2)
                size = b1 & 0x7f
                if b1 & 0x80:
                    raise AssertionError('服务器发的帧不该加掩码')
                if size == 126:
                    size = struct.unpack('>H', self._exact(2))[0]
                elif size == 127:
                    size = struct.unpack('>Q', self._exact(8))[0]
                data = self._exact(size)
            except (EOFError, OSError):
                self.closed = True
                break
            op = b0 & 0x0f
            if op == 8:
                self.closed = True
            elif op == 10:
                self.pongs.append(data)
            elif op == 2:
                self.buf += data
        out = bytes(self.buf[:n])
        del self.buf[:n]
        return out

    def close(self):
        self.s.close()


class Guest:
    """扮演手机上的那一页：一把自己生成的密钥（不在设备清单里），请求照文档签名，发到网页端的那个端口（照证书机构验证书）"""
    def __init__(self, test, name='iPhone'):
        self.t, self.name = test, name
        self.keyfile = test.d / ('guest-' + secrets.token_hex(4))
        self.pub = keygen(self.keyfile, 'web')
        self.fp = sw.fp_of(str(self.keyfile) + '.pub')

    def post(self, op, extra, key=None, port=None):
        body = dict(extra, v=1, op=op, key=self.fp, ts=time.time(), nonce=secrets.token_hex(16))
        raw = json.dumps(body, separators=(',', ':')).encode()
        sig = sign(key or self.keyfile, 'cca-presence', raw)
        for _ in range(6):
            c = http.client.HTTPSConnection('127.0.0.1', port or self.t.wport, context=ca_ctx(self.t.ca), timeout=20)
            c.request('POST', '/v1/' + op, body=raw, headers={'Content-Type': 'application/json', 'X-Cca-Sig': base64.b64encode(sig).decode()})
            r = c.getresponse()
            j = json.loads(r.read() or b'{}')
            c.close()
            if r.status != 429 or j.get('error') != 'rate':
                break
            time.sleep(2)
        return r.status, j

    def good(self, op, **extra):
        st, j = self.post(op, extra)
        self.t.assertEqual(st, 200, (op, j))
        return j

    def join(self, code_bytes, owner_fp=None):
        """拿配对码配对（对面先开了给网页端的配对、放了它那一份）。返回 (密钥, 配对编号)，记在自己身上"""
        kc, keys, rid = sw.pair_keys(code_bytes)
        nb = secrets.token_bytes(32)
        self.good('pair_put', rid=rid, pub=self.pub, m=sw.pair_msg(keys, rid, 'b', self.fp, nb, self.name))
        j = self.good('pair_get', rid=rid, wait=10)
        theirs = sw.pair_open(keys, rid, 'a', j['m'])
        self.t.assertEqual(theirs['fp'], j['fp'])
        if owner_fp:
            self.t.assertEqual(j['fp'], owner_fp)
        self.peer_fp, self.rid = theirs['fp'], rid
        self.p, self.pid = sw.pair_secret(kc, bytes.fromhex(theirs['n']), nb, theirs['fp'], self.fp)
        self.good('pair_commit', rid=rid, pid=self.pid)
        return self.p, self.pid

    def ws(self, **kw):
        return WsSock(self.t.wport, self.t.ca, origin=self.t.origin, **kw)

    def attach(self, sid):
        """换票据、经 WebSocket 进中转、说开场白。返回 (连接, 服务器该回的那一行)"""
        t = self.good('relay_attach', sid=sid, pid=self.pid)
        s = self.ws()
        self.t.assertEqual((s.status, s.accept_ok), (101, True), s.head)
        line, ok = sw.relay_line('view', t['tid'], base64.b64decode(t['tkey']))
        s.sendall(line)
        return s, ok


class WebBase(Base):
    """开着网页端的服务器：页面的文件在临时目录里（和真的那一页无关），另开一个端口、证书是测试的证书机构签的"""
    PAGE = b'<!doctype html><title>cca</title><script type="module" src="app.js"></script>\n'
    SCRIPT = b'// app\n' + b'console.log("x");\n' * 400

    def server_extra(self, d):
        tls = d / 'web-tls'
        tls.mkdir()
        make_ca_cert(tls)
        self.ca = tls / 'ca.pem'
        root = d / 'web-root'
        root.mkdir()
        (root / 'index.html').write_bytes(self.PAGE)
        (root / 'app.js').write_bytes(self.SCRIPT)
        (root / 'icon.png').write_bytes(b'\x89PNG\r\n\x1a\n' + os.urandom(64))
        (root / 'README.md').write_text('不是页面的一部分\n')
        (root / '.secret.js').write_text('x')
        (root / 'sub').mkdir()
        (root / 'sub' / 'x.js').write_text('x')
        self.origin = 'https://relay.example.com:8443'
        return ['--web-url', self.origin + '/', '--web-listen', '127.0.0.1:0', '--web-port-file', str(d / 'wport'),
                '--web-cert', str(tls / 'cert.pem'), '--web-key', str(tls / 'key.pem'), '--web-root', str(root)]

    def setUp(self):
        super().setUp()
        self.wport = int(wait_for('网页端的端口起来', lambda: (self.d / 'wport').exists() and (self.d / 'wport').read_text()))
        self.a, self.b = self.device('box'), self.device('laptop')
        self.machines(self.a, self.b)

    def get(self, path, headers=None, port=None, ca=True):
        ctx = ca_ctx(self.ca) if ca else ssl._create_unverified_context()
        c = http.client.HTTPSConnection('127.0.0.1', port or self.wport, context=ctx, timeout=20)
        c.request('GET', path, headers=headers or {})
        r = c.getresponse()
        out = r.status, dict((k.lower(), v) for k, v in r.getheaders()), r.read()
        c.close()
        return out

    def open_web(self, dev, code=None):
        """dev 开一次给网页端的配对、放上它那一份。返回 (配对码的字节, rid, 密钥对, 它的随机数)"""
        code = code or secrets.token_bytes(10)
        kc, keys, rid = sw.pair_keys(code)
        na = secrets.token_bytes(32)
        j = self.good(dev, 'pair_open', rid=rid, web=True)
        self.assertEqual(j['web'], self.origin + '/')
        self.good(dev, 'pair_put', rid=rid, m=sw.pair_msg(keys, rid, 'a', dev.fp, na, dev.name))
        return code, rid, kc, na

    def pair_guest(self, dev, g):
        """dev 和访客 g 配好对。返回 (密钥, 配对编号)"""
        code, rid, kc, na = self.open_web(dev)
        p, pid = g.join(code, dev.fp)
        j = self.good(dev, 'pair_get', rid=rid)
        self.assertEqual((j['fp'], j['name']), (g.fp, None), '访客不在设备清单里：服务器不知道它叫什么')
        self.assertEqual(sw.pair_open(sw.pair_keys(code)[1], rid, 'b', j['m'])['name'], g.name)
        self.assertEqual(self.good(dev, 'pair_commit', rid=rid, pid=pid)['done'], True)
        self.assertEqual(g.good('pair_get', rid=rid, until='done')['done'], True)
        return p, pid

    def remote(self):
        p = self.d / 'remote.json'
        return json.loads(p.read_text()) if p.exists() else {'edges': {}, 'guests': {}}


class ServerTest(WebBase):
    def test_page_files(self):
        st, h, body = self.get('/')
        self.assertEqual((st, body, h['content-type']), (200, self.PAGE, 'text/html; charset=utf-8'))
        self.assertEqual(h['cache-control'], 'no-cache')
        csp = h['content-security-policy']
        for want in ("default-src 'none'", "script-src 'self'", "connect-src 'self' wss://relay.example.com:8443", "frame-ancestors 'none'"):
            self.assertIn(want, csp)
        self.assertNotIn('unsafe-eval', csp)
        self.assertEqual((h['x-content-type-options'], h['referrer-policy']), ('nosniff', 'no-referrer'))
        self.assertEqual(self.get('/index.html')[2], self.PAGE)
        self.assertEqual(self.get('/?from=home')[2], self.PAGE)
        # 没变：304，不再发内容；压缩的那一份解出来一样
        self.assertEqual(self.get('/', {'If-None-Match': h['etag']})[::2], (304, b''))
        st, h, body = self.get('/app.js', {'Accept-Encoding': 'gzip, br'})
        self.assertEqual((st, h['content-encoding'], h['content-type']), (200, 'gzip', 'text/javascript; charset=utf-8'))
        self.assertLess(len(body), len(self.SCRIPT) // 4)
        self.assertEqual(__import__('gzip').decompress(body), self.SCRIPT)
        self.assertEqual(self.get('/app.js')[2], self.SCRIPT)
        st, h, body = self.get('/icon.png', {'Accept-Encoding': 'gzip'})
        self.assertEqual((st, h['content-type'], 'content-encoding' in h, len(body)), (200, 'image/png', False, 72))
        # 不是页面的文件、目录里别的东西、往外走的路径：都没有
        for path in ('/README.md', '/.secret.js', '/sub/x.js', '/sub', '/../state.json', '/%2e%2e/state.json', '/app.js/',
                     '/state.json', '/remote.json', '/nope.js'):
            self.assertEqual(self.get(path)[0], 404, path)
        j = json.loads(self.get('/v1/health')[2])
        self.assertEqual((j['web'], j['ok']), (self.origin + '/', True))
        # 原来那个端口（自签的证书，给 cca 的设备）照旧；浏览器不认它的证书
        self.assertEqual(json.loads(self.get('/v1/health', port=self.srv.port, ca=False)[2])['web'], self.origin + '/')
        with self.assertRaises(ssl.SSLError):
            self.get('/v1/health', port=self.srv.port)

    def test_guest_pairing_and_what_a_guest_may_do(self):
        a, b = self.a, self.b
        g, other = Guest(self), Guest(self, 'iPad')
        code, rid, kc, na = self.open_web(a)
        keys = sw.pair_keys(code)[1]
        m = sw.pair_msg(keys, rid, 'b', g.fp, secrets.token_bytes(32), g.name)
        # 不在设备清单里的：没带公钥、公钥和指纹对不上、签名不是这把密钥签的，都进不来；回的都一样
        denied = (403, {'error': 'denied'})
        self.assertEqual(g.post('pair_put', {'rid': rid, 'm': m}), denied)
        self.assertEqual(g.post('pair_put', {'rid': rid, 'm': m, 'pub': other.pub}), denied)
        self.assertEqual(g.post('pair_put', {'rid': rid, 'm': m, 'pub': g.pub}, key=other.keyfile), denied)
        self.assertEqual(g.post('pair_get', {'rid': rid, 'pub': g.pub}), denied, '还没放过密文的：别的请求不行')
        self.assertEqual(g.post('pair_put', {'rid': secrets.token_hex(16), 'm': m, 'pub': g.pub}), denied, '不知道配对码（rid 对不上）')
        # 不是开给网页端的配对：访客进不去
        code2 = secrets.token_bytes(10)
        kc2, keys2, rid2 = sw.pair_keys(code2)
        self.good(a, 'pair_open', rid=rid2)
        self.assertEqual(g.post('pair_put', {'rid': rid2, 'pub': g.pub, 'm': sw.pair_msg(keys2, rid2, 'b', g.fp, os.urandom(32), 'x')}), denied)
        self.good(a, 'pair_close', rid=rid2)
        self.assertEqual(self.remote()['guests'], {})

        p, pid = g.join(code, a.fp)
        self.assertEqual(other.post('pair_put', {'rid': rid, 'pub': other.pub, 'm': m}), denied, '已经有人输入了：第二个访客进不来')
        self.assertEqual(self.remote()['guests'], {}, '还没配成：不记')
        self.assertEqual(self.good(a, 'pair_get', rid=rid)['fp'], g.fp)
        self.assertEqual(self.good(a, 'pair_commit', rid=rid, pid=pid)['done'], True)
        doc = self.remote()
        self.assertEqual(doc['guests'][g.fp]['key'], g.pub)
        self.assertEqual((doc['edges'][pid]['a'], doc['edges'][pid]['b']), (a.fp, g.fp))
        self.assertNotIn(p.hex(), json.dumps(doc))

        # 访客能做的：看配对的设备上的会话、接入、取消配对。别的都不行
        self.assertEqual(g.good('relay_list', pids=[pid]), {'v': 1, 'ok': True, 'peers': {pid: {'fp': a.fp, 'name': 'box'}}, 'sessions': {pid: []}})
        self.assertEqual(g.post('relay_attach', {'sid': 'ab' * 16, 'pid': pid}), (404, {'error': 'no_session'}))
        for op, extra in (('pair_open', {'rid': secrets.token_hex(16)}), ('pair_open', {'rid': secrets.token_hex(16), 'web': True}),
                          ('relay_host', {'sid': secrets.token_hex(16), 'pids': [pid]}), ('report', {'sessions': []}), ('view', {'accounts': []}),
                          ('keys', {}), ('ledger', {})):
            self.assertEqual(g.post(op, extra), denied, op)
        # 已经是访客了：再和别的设备配对，也只进得了开给网页端的
        kc3, keys3, rid3 = sw.pair_keys(secrets.token_bytes(10))
        self.good(b, 'pair_open', rid=rid3)
        self.assertEqual(g.post('pair_put', {'rid': rid3, 'm': sw.pair_msg(keys3, rid3, 'b', g.fp, os.urandom(32), 'x')}), denied)
        self.good(b, 'pair_close', rid=rid3)
        p2, pid2 = self.pair_guest(b, g)
        self.assertEqual(sorted(self.remote()['edges']), sorted([pid, pid2]))
        # 设备清单里的设备也可以输入开给网页端的配对码（它不是访客）
        code4, rid4, kc4, na4 = self.open_web(a)
        self.good(b, 'pair_put', rid=rid4, m=sw.pair_msg(sw.pair_keys(code4)[1], rid4, 'b', b.fp, os.urandom(32), 'laptop'))
        self.good(a, 'pair_close', rid=rid4)
        self.assertEqual(list(self.remote()['guests']), [g.fp])

        # 重启以后访客还在；取消一个配对它还在，最后一个也取消了就删掉，之后它的请求不认
        self.srv.stop()
        self.srv = rr.RelayServer(self.d, sniff=self.sniff, port=self.srv.port, extra=self.restart_args())
        self.wport = int(wait_for('网页端的端口起来', lambda: (self.d / 'wport').exists() and (self.d / 'wport').read_text()))
        self.assertEqual(list(g.good('relay_list', pids=[pid, pid2])['peers']), [pid, pid2])
        self.good(a, 'pair_drop', pid=pid)
        self.assertEqual(list(self.remote()['guests']), [g.fp])
        self.assertEqual(list(g.good('relay_list', pids=[pid, pid2])['peers']), [pid2])
        g.good('pair_drop', pid=pid2)
        self.assertEqual((self.remote()['guests'], self.remote()['edges']), ({}, {}))
        self.assertEqual(g.post('relay_list', {'pids': [pid2]}), denied)

    def restart_args(self):
        d, tls = self.d, self.d / 'web-tls'
        return ['--web-url', self.origin + '/', '--web-listen', '127.0.0.1:0', '--web-port-file', str(d / 'wport'),
                '--web-cert', str(tls / 'cert.pem'), '--web-key', str(tls / 'key.pem'), '--web-root', str(d / 'web-root')]

    def test_the_sponsor_is_revoked(self):
        a, b = self.a, self.b
        g = Guest(self)
        p, pid = self.pair_guest(a, g)
        self.assertEqual(list(self.remote()['guests']), [g.fp])
        self.machines(b)   # 领它进来的那台设备撤销了：配对没了，访客也就没了
        doc = self.remote()
        self.assertEqual((doc['edges'], doc['guests']), ({}, {}))
        self.assertEqual(g.post('relay_list', {'pids': [pid]}), (403, {'error': 'denied'}))

    def test_attach_over_websocket(self):
        a = self.a
        g = Guest(self)
        p, pid = self.pair_guest(a, g)
        sid = secrets.token_hex(16)
        t = self.good(a, 'relay_host', sid=sid, pids=[pid])
        key = base64.b64decode(t['tkey'])
        host, ok = dial(self.srv.rport, 'host', t['tid'], key)
        self.assertEqual(read_line(host), ok)
        blob = sw.dir_seal(p, pid, sid, {'v': 1, 'machine': 'box', 'tool': 'claude', 'name': '工作', 'cwd': '~/项目', 'since': 1, 'access': 'control'})
        host.sendall(ctl(F_ANNOUNCE, json.dumps({pid: blob}).encode()))
        (row,) = wait_for('访客看到会话', lambda: g.good('relay_list', pids=[pid])['sessions'][pid])
        self.assertEqual(sw.dir_open(p, pid, sid, row['blob'])['cwd'], '~/项目')

        # 握手不对的、不是自己那一页发起的：进不来
        self.assertEqual(g.ws(headers={'Sec-WebSocket-Version': '8'}).status, 400)
        self.assertEqual(g.ws(headers={'Upgrade': None}).status, 400)
        self.assertEqual(WsSock(self.wport, self.ca, origin='https://evil.example.com').status, 403)
        self.assertEqual(self.get('/v1/relay/ws')[0], 400)

        view, vok = g.attach(sid)
        kind, payload = read_ctl(host)
        self.assertEqual(kind, F_JOIN)
        j = json.loads(payload)
        join, jok = dial(self.srv.rport, 'join', j['token'], key)
        self.assertEqual((read_line(join), read_line(view)), (jok, vok))
        # 两头握手、收发（宿主那头是普通的 TCP，访客那头经 WebSocket）
        hc, vc = sw.Channel('host', p, pid, sid, a.fp, g.fp), sw.Channel('viewer', p, pid, sid, a.fp, g.fp)
        view.sendall(vc.hello())
        reply = hc.reply(read_exact(join, 52))
        join.sendall(reply)
        confirm = vc.confirm(read_exact(view, 68))
        self.assertIsNotNone(confirm)
        view.sendall(confirm)
        self.assertTrue(hc.accept(read_exact(join, 35)))
        view.sendall(vc.seal(b'keys from the phone'))
        buf = bytearray(read_exact(join, 8 + 2 + 32 + 16))
        self.assertEqual(hc.open(buf), b'keys from the phone')
        # 大块的、切成碎帧的、几帧并成一次发的：一个字节不丢、不乱
        big = os.urandom(700 * 1024)
        join.sendall(big)
        self.assertEqual(read_exact(view, len(big)), big)
        up = os.urandom(300 * 1024)
        view.s.sendall(view.frame(2, up[:10], fin=False) + view.frame(0, up[10:5000], fin=False) + view.frame(0, up[5000:70000])
                       + view.frame(9, b'ping me') + view.frame(2, up[70000:]))
        self.assertEqual(read_exact(join, len(up)), up)
        join.sendall(b'!')
        self.assertEqual(view.recv(1), b'!')
        self.assertEqual(view.pongs, [b'ping me'])
        self.assertEqual(g.good('relay_list', pids=[pid])['sessions'][pid][0]['viewers'], 1)
        # 宿主那头断了：访客这边收到关闭
        join.close()
        self.assertEqual(view.recv(1), b'')
        self.assertTrue(view.closed)
        wait_for('这一处接入算结束', lambda: g.good('relay_list', pids=[pid])['sessions'][pid][0]['viewers'] == 0)

        # 不合规矩的帧（没加掩码、文本的）：断开这一条，服务器照常
        for bad in (lambda w: w.frame(2, b'x', mask=False), lambda w: w.frame(1, b'text')):
            w = g.ws()
            self.assertEqual(w.status, 101)
            w.s.sendall(bad(w))
            self.assertEqual(w.recv(1), b'')
        # 开场白不对：中转回一行原因就关
        w = g.ws()
        w.sendall(b'CCA-RELAY/1 view ' + b'0' * 32 + b' ' + b'0' * 16 + b' ' + b'A' * 43 + b'\n')
        self.assertEqual(read_line(w), b'CCA-RELAY/1 ERR ticket\n')
        self.assertEqual(w.recv(1), b'')
        # 连上不说话：5 秒后被中转断开（和数据端口上一样，先说一行原因）
        w = g.ws()
        t0 = time.monotonic()
        self.assertEqual(read_line(w), b'CCA-RELAY/1 ERR slow\n')
        self.assertEqual(w.recv(1), b'')
        self.assertLess(time.monotonic() - t0, 12)
        # 之后还接得上
        view, vok = g.attach(sid)
        self.assertEqual(read_ctl(host)[0], F_JOIN)

    def test_many_websockets(self):
        """同一个地址开得了的 WebSocket 有上限；占着它们不影响普通请求"""
        g = Guest(self)
        socks = [g.ws() for _ in range(9)]
        self.assertEqual([w.status for w in socks], [101] * 8 + [429])
        self.assertEqual(self.get('/')[0], 200)
        self.assertEqual(json.loads(self.get('/v1/health')[2])['ok'], True)
        for w in socks:
            w.close()
        wait_for('名额还了', lambda: g.ws().status == 101, 20)


class OffTest(Base):
    """没开网页端的服务器（和以前一样）：没有那一页、没有 WebSocket，设备开不了给网页端的配对，不在清单里的一律不认"""
    def test_off(self):
        a = self.device('box')
        self.machines(a)
        ctx = ssl._create_unverified_context()
        for path in ('/', '/index.html', '/v1/relay/ws'):
            c = http.client.HTTPSConnection('127.0.0.1', self.srv.port, context=ctx, timeout=10)
            c.request('GET', path, headers={'Upgrade': 'websocket', 'Connection': 'Upgrade', 'Sec-WebSocket-Version': '13',
                                            'Sec-WebSocket-Key': base64.b64encode(os.urandom(16)).decode()})
            self.assertEqual(c.getresponse().status, 404, path)
            c.close()
        kc, keys, rid = sw.pair_keys(secrets.token_bytes(10))
        self.assertEqual(self.call(a, 'pair_open', rid=rid, web=True), (404, {'error': 'no_web'}))
        self.good(a, 'pair_open', rid=rid)
        self.d.joinpath('web-tls').mkdir()
        self.ca = None
        g = Guest.__new__(Guest)
        g.t, g.name, g.keyfile = self, 'x', self.d / 'gk'
        g.pub = keygen(g.keyfile, 'web')
        g.fp = sw.fp_of(str(g.keyfile) + '.pub')
        body = dict(rid=rid, pub=g.pub, m=sw.pair_msg(keys, rid, 'b', g.fp, os.urandom(32), 'x'), v=1, op='pair_put', key=g.fp,
                    ts=time.time(), nonce=secrets.token_hex(16))
        raw = json.dumps(body, separators=(',', ':')).encode()
        c = http.client.HTTPSConnection('127.0.0.1', self.srv.port, context=ctx, timeout=10)
        c.request('POST', '/v1/pair_put', body=raw, headers={'X-Cca-Sig': base64.b64encode(sign(g.keyfile, 'cca-presence', raw)).decode()})
        r = c.getresponse()
        self.assertEqual((r.status, json.loads(r.read())), (403, {'error': 'denied'}))
        c.close()


NODE = shutil.which('node')


def node_ok():
    """有 Node 22 以上（自带 WebSocket、WebCrypto 的 Ed25519）才测浏览器里跑的那一份；没有就跳过（回归不要求装 Node）"""
    if not NODE:
        return False
    try:
        v = subprocess.run([NODE, '--version'], capture_output=True, text=True, timeout=20).stdout.strip().lstrip('v')
        return int(v.split('.')[0]) >= 22
    except (OSError, ValueError, subprocess.TimeoutExpired):
        return False


class Node:
    """跑着 tests/web_client.mjs 的一个 Node 进程：call(命令, 参数…) 发一条、等它回一条。ca：照这个证书机构认服务器的证书"""
    def __init__(self, test, ca=None):
        env = dict(os.environ, NODE_NO_WARNINGS='1')
        for k in ('HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'http_proxy', 'https_proxy', 'all_proxy', 'NODE_USE_ENV_PROXY'):
            env.pop(k, None)   # 只连本机：不经这台机器上设的代理
        if ca:
            env['NODE_EXTRA_CA_CERTS'] = str(ca)
        self.p = subprocess.Popen([NODE, str(ROOT / 'tests' / 'web_client.mjs')], env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, text=True)
        test.addCleanup(self.close)

    def call(self, cmd, **args):
        self.p.stdin.write(json.dumps(dict(args, cmd=cmd)) + '\n')
        self.p.stdin.flush()
        line = self.p.stdout.readline()
        if not line:
            raise AssertionError('Node 那一头退出了：%s' % self.p.stderr.read()[-2000:])
        return json.loads(line)

    def good(self, cmd, **args):
        j = self.call(cmd, **args)
        if not j.get('ok'):
            raise AssertionError('%s 没成：%s' % (cmd, j))
        return j

    def close(self):
        if self.p.poll() is None:
            self.p.kill()
        self.p.wait()
        for f in (self.p.stdin, self.p.stdout, self.p.stderr):
            f.close()


@unittest.skipUnless(node_ok(), '没有 Node 22 以上：浏览器里跑的那一份（server/web/core.js）这次没测')
class CoreJsTest(unittest.TestCase):
    """浏览器里跑的那一份（server/web/core.js）和照文档独立实现的 spec_wire.py 对：同样的输入算出同样的东西，互相握得上手、
    解得开对方的记录。不起服务器"""
    def setUp(self):
        self.node = Node(self)

    def test_the_same_numbers(self):
        b = lambda x: base64.b64encode(x).decode()
        code = secrets.token_bytes(10)
        kc, keys, rid = sw.pair_keys(code)
        na, nb = secrets.token_bytes(32), secrets.token_bytes(32)
        fpa, fpb = 'SHA256:' + b(os.urandom(32)).rstrip('='), 'SHA256:' + b(os.urandom(32)).rstrip('=')
        p, pid = sw.pair_secret(kc, na, nb, fpa, fpb)
        sid = secrets.token_hex(16)
        info = {'v': 1, 'machine': '书房的 Mac', 'tool': 'claude', 'name': '工作\x1b[31m', 'cwd': '~/项目/' + 'x' * 300, 'since': 1794000000,
                'access': 'view'}
        blob, key = os.urandom(1000), os.urandom(32)
        text = sw.code_text(code)
        j = self.node.good('vectors', code=text.lower().replace('-', ' ').replace('1', 'l').replace('0', 'o'), na=b(na), nb=b(nb),
                           fpa=fpa, fpb=fpb, msg_a=sw.pair_msg(keys, rid, 'a', fpa, na, '书房的 Mac'), name_b='我的 iPhone',
                           sid=sid, dir=sw.dir_seal(p, pid, sid, info), blob=b(blob), key=b(key),
                           bad_codes=['', text[:-1], text + 'A', text.replace(text[0], 'U'), 'u' * 16])
        self.assertEqual((j['code_text'], j['rid'], j['p'], j['pid']), (text, rid, p.hex(), pid))
        self.assertEqual((j['sha'], j['mac']), (hashlib.sha256(blob).hexdigest(), sw.H(key, blob).hex()))
        self.assertEqual(j['opened'], {'fp': fpa, 'n': na.hex(), 'name': '书房的 Mac'})
        self.assertEqual(sw.pair_open(keys, rid, 'b', j['msg_b']), {'v': 1, 'fp': fpb, 'n': nb.hex(), 'name': '我的 iPhone'})
        self.assertEqual(len(base64.b64decode(j['msg_b']['c'])), 256, '补齐到 256 的整数倍（按字节算，不是按字）')
        # 会话的说明：控制字符去掉、太长的截掉；换一个会话编号就打不开
        self.assertEqual(j['dir'], {'machine': '书房的 Mac', 'tool': 'claude', 'name': '工作[31m', 'cwd': '~/项目/' + 'x' * 195, 'since': 1794000000,
                                    'access': 'view'})
        self.assertIsNone(j['dir_bad'])
        self.assertEqual(j['bad_codes'], [True] * 5)

    def test_handshake_and_records(self):
        b, ub = lambda x: base64.b64encode(x).decode(), base64.b64decode
        p, pid, sid = os.urandom(32), secrets.token_hex(16), secrets.token_hex(16)
        fph, fpv = 'SHA256:host', 'SHA256:viewer'
        host = sw.Channel('host', p, pid, sid, fph, fpv)
        hello = ub(self.node.good('channel', p=b(p), pid=pid, sid=sid, fph=fph, fpv=fpv)['hello'])
        reply = host.reply(hello)
        self.assertIsNotNone(reply)
        confirm = self.node.good('confirm', reply=b(reply))['confirm']
        self.assertTrue(host.accept(ub(confirm)))
        # 两个方向各发几条：空的、一个字节、正好一条的上限
        sizes = [0, 1, 30, 31, 1000, 16384]
        data = [os.urandom(n) for n in sizes]
        recs = self.node.good('seal', data=[b(d) for d in data])['records']
        buf = bytearray(b''.join(ub(r) for r in recs))
        self.assertEqual([host.open(buf) for _ in sizes], data)
        self.assertEqual(len(buf), 0)
        sent = b''.join(host.seal(d) for d in data)
        j = self.node.good('open', bytes=b(sent + host.seal(b'tail')[:20]))
        self.assertEqual(([ub(x) for x in j['got']], j['left']), (data, 20), '最后半条留着等后面的')
        # 改一个字节、漏一条（序号对不上）：认证不过
        for bad in (lambda r: r[:12] + bytes([r[12] ^ 1]) + r[13:], lambda r: host.seal(b'next')):
            j = self.node.good('open', bytes=b(bad(host.seal(b'skipped'))))
            self.assertEqual((j['got'], j.get('error')), ([], 'record auth'))
        # 宿主的证明不对（它没有这把密钥）：不发确认
        other = sw.Channel('host', os.urandom(32), pid, sid, fph, fpv)
        hello = ub(self.node.good('channel', p=b(p), pid=pid, sid=sid, fph=fph, fpv=fpv)['hello'])
        self.assertIsNone(self.node.good('confirm', reply=b(other.reply(hello)))['confirm'])

    def test_signature(self):
        """页面签的请求：服务器验得过（它自己的验签），ssh-keygen 也认"""
        data = os.urandom(300)
        j = self.node.good('sign', data=base64.b64encode(data).decode())
        self.assertRegex(j['pub'], r'^ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI[A-Za-z0-9+/]{43}$')
        self.assertEqual(j['fp'], 'SHA256:' + srvmod.key_fp(j['pub'].split()[1]).split(':', 1)[1])
        self.assertTrue(srvmod.ssh_verify(j['pub'], 'cca-presence', data, j['sig'].encode()))
        self.assertFalse(srvmod.ssh_verify(j['pub'], 'cca-presence', data + b'x', j['sig'].encode()))
        self.assertFalse(srvmod.ssh_verify(j['pub'], 'cca-devices', data, j['sig'].encode()))
        with tempfile.TemporaryDirectory() as d:
            Path(d, 'signers').write_text('web %s\n' % j['pub'])
            Path(d, 'sig').write_text(j['sig'])
            r = subprocess.run(['ssh-keygen', '-Y', 'verify', '-f', str(Path(d, 'signers')), '-I', 'web', '-n', 'cca-presence', '-s',
                                str(Path(d, 'sig'))], input=data, capture_output=True)
            self.assertEqual(r.returncode, 0, r.stderr)


@unittest.skipUnless(node_ok(), '没有 Node 22 以上：浏览器里跑的那一份（server/web/core.js）这次没测')
class ClientTest(WebBase):
    """整条路：box 上是真的 cca（辅助程序：在宿主里运行的会话、cca remote pair --web），手机那一头是浏览器里跑的那一份
    （在 Node 里：自己生成密钥、输入配对码、列会话、经 WebSocket 接入、敲键、断了自己接上）"""
    def setUp(self):
        super().setUp()
        self.node = Node(self, ca=self.ca)
        # 网页端的地址照实际的端口：页面连的是它
        self.base = 'https://127.0.0.1:%d' % self.wport

    def server_extra(self, d):
        args = super().server_extra(d)
        self.origin = None   # Node 里的 WebSocket 不带 Origin（浏览器才带）；真的浏览器里对不对由界面那一轮看
        return args

    def session(self, dev, *args):
        h = rr.Term([rr.HELPER, 'remote-host', 'claude', rr.Device.KEY, 'alice', '--', sys.executable, str(dev.home / 'client.py'), *args],
                    dev.env, str(dev.work))
        self.terms.append(h)
        h.expect('<ready 24 80')
        return h

    def pair_web(self, dev, code_to=None):
        """dev 上 cca remote pair --web（辅助程序的那一步），手机输入配对码。返回 (那一头回的, 辅助程序说的对方的名字)"""
        p = subprocess.Popen([rr.HELPER, 'remote-pair-show', '--web'], env=dev.env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, text=True)
        try:
            line, web = p.stdout.readline(), p.stdout.readline()
            self.assertRegex(line, r'^code [0-9A-Z]{4}(-[0-9A-Z]{4}){3}$')
            self.assertEqual(web, 'web https://relay.example.com:8443/\n', '手机上打开的地址：服务器配置里写的那个')
            j = self.node.call('pair', code=line.split()[1] if code_to is None else code_to)
            if not j['ok']:
                return j, None
            out, err = p.communicate(timeout=60)
        finally:
            p.kill()
            p.stdout.close()
            p.stderr.close()
        self.assertEqual((p.returncode, err), (0, ''))
        self.assertRegex(out, r'^peer .+\n$')
        return j, out[5:-1]

    def test_pair_list_attach_type(self):
        a = self.a
        me = self.node.good('start', base=self.base, name='我的 iPhone')
        self.assertEqual(self.node.good('list')['rows'], [], '没配对过：不问服务器')
        h = self.session(a)
        # 配对码输错了、输的是别的码：没配成，说得出是哪种
        self.assertEqual(self.node.call('pair', code='not a code')['error'], 'code')
        self.assertEqual(self.node.call('pair', code=sw.code_text(secrets.token_bytes(10)))['error'], 'no_pairing')
        j, said = self.pair_web(a)
        self.assertEqual((j['fp'], j['name'], j['steps'], j['extractable']), (a.fp, 'box', ['deriving', 'asking', 'confirming'], False))
        self.assertEqual(said, '我的 iPhone')
        pid = j['pid']
        # box 的配对表：记着它是网页端、让它看和操作；密钥两边是同一把（box 存在密钥文件里，手机那头取不出来所以比不了，靠后面握手成功）
        row = json.loads((a.cca / 'remote' / 'peers.json').read_text())['peers'][pid]
        self.assertEqual((row['fp'], row['name'], row['incoming'], row['shown'], row['web']), (me['fp'], '我的 iPhone', 'control', True, True))
        self.assertIn('我的 iPhone（手机上的网页端）  对方可以看和操作这台机器的会话', a.ok('remote-peers'))
        doc = self.remote()
        self.assertEqual((list(doc['guests']), doc['guests'][me['fp']]['key']), ([me['fp']], me['pub']))

        # 列会话：说明是宿主封给这个配对的（工具、账号的短名字、文件夹；没有邮箱）
        rows = wait_for('手机看到会话', lambda: (lambda r: r if r[0]['sessions'] else None)(self.node.good('list')['rows']) or time.sleep(1.2))
        self.assertEqual((rows[0]['pid'], rows[0]['name'], rows[0]['gone']), (pid, 'box', False))
        (s,) = rows[0]['sessions']
        self.assertEqual((s['tool'], s['name'], s['access'], s['online'], bool(s['machine'])), ('claude', 'alice', 'control', True, True))
        self.assertNotIn('example.com', json.dumps(rows))
        sid = s['sid']

        # 接入：先收到回放，敲的键到了会话里，会话的输出回到手机上；窗口大小跟着最小的那一处
        self.node.good('attach', pid=pid, sid=sid, cols=60, rows=20)
        self.assertTrue(self.node.good('wait', state='open')['ok'])
        self.assertTrue(self.node.good('wait', text='<ready 24 80')['ok'], '回放里有接入之前的输出')
        h.expect('<size 20 60>')
        self.node.good('input', text='say hello from the phone\r')
        j = self.node.good('wait', text='<hello from the phone>')
        self.assertTrue(j['ok'], j['tail'])
        h.expect('<hello from the phone>')
        self.node.good('resize', cols=50, rows=18)
        h.expect('<size 18 50>')
        # 一大段输出（几百 KB，切成很多条记录）：一个字节不丢
        self.node.good('clear')
        self.node.good('input', text='flood\r')
        j = self.node.good('wait', text='<flooded>\r', secs=40, count=' of the flood\r')
        self.assertTrue(j['ok'], j['tail'][-200:])
        self.assertEqual(j['count'], 20000, '两万行一行不少')
        # 连接从底下断了：自己接回来，接着用
        self.node.good('clear')
        self.node.good('cut')
        j = self.node.good('wait', state='retrying')
        self.assertTrue(j['ok'], j['events'])
        self.node.good('input', text='say typed while offline\r')   # 断着的时候敲的：不送（不攒着等接上）
        self.assertTrue(wait_for('重新接上', lambda: self.node.good('wait', state='open', secs=1)['events'][-1][0] == 'open' or None, 30))
        self.node.good('input', text='say back again\r')
        self.assertTrue(self.node.good('wait', text='<back again>')['ok'])
        h.expect('<back again>')
        self.assertNotIn(b'typed while offline', h.out)
        # 自己断开：会话接着运行，窗口大小回到原来的
        ev = self.node.good('detach')['events']
        self.assertEqual(ev[-1], ['closed', 'detached'])
        h.expect('<size 24 80>')
        # 会话结束：接着的那一处收到退出码
        self.node.good('attach', pid=pid, sid=sid, cols=80, rows=24)
        self.assertTrue(self.node.good('wait', state='open')['ok'])
        h.send('exit 7\r')
        j = self.node.good('wait', state='closed', why='ended')
        self.assertTrue(j['ok'], j['events'])
        self.assertIn(['exit', 7], j['events'])

    def test_view_only_unpair_and_no_session(self):
        a = self.a
        self.node.good('start', base=self.base, name='iPad')
        h = self.session(a)
        j, said = self.pair_web(a)
        pid = j['pid']
        rows = wait_for('手机看到会话', lambda: (lambda r: r if r[0]['sessions'] else None)(self.node.good('list')['rows']) or time.sleep(1.2))
        sid = rows[0]['sessions'][0]['sid']
        # 只让看：宿主那头把关，手机敲的键进不去（页面自己不发，发了也不转）
        a.ok('remote-allow', 'iPad', 'view')
        rows = wait_for('说明改成只看', lambda: (lambda r: r if r[0]['sessions'] and r[0]['sessions'][0]['access'] == 'view' else None)(
            self.node.good('list')['rows']) or time.sleep(1.2))
        self.node.good('attach', pid=pid, sid=sid, cols=70, rows=22)
        self.assertTrue(self.node.good('wait', state='open')['ok'])
        self.node.good('input', text='say must not arrive\r')
        h.send('say typed on the box\r')
        self.assertTrue(self.node.good('wait', text='<typed on the box>')['ok'])
        h.expect('<typed on the box>')
        self.assertNotIn(b'must not arrive', h.out)
        self.assertNotIn(b'<size 22 70>', h.out, '只看的不算进窗口大小')
        # 那边不让接入了：正接着的被断开，再也接不上（过一阵说会话不在了）；没有这个会话的直接说
        a.ok('remote-allow', 'iPad', 'none')
        j = self.node.good('wait', state='retrying', secs=30)
        self.assertTrue(j['ok'], j['events'])
        self.node.good('detach')
        self.node.good('attach', pid=pid, sid='ab' * 16, cols=80, rows=24)
        self.assertTrue(self.node.good('wait', state='closed', why='gone')['ok'])
        # 手机上取消配对：服务器上的记录、访客都删掉；box 那边再看是“对方已经取消了”
        self.assertEqual(self.node.good('unpair', pid=pid)['told'], True)
        self.assertEqual(self.node.good('list')['rows'], [])
        doc = self.remote()
        self.assertEqual((doc['edges'], doc['guests']), ({}, {}))
        self.assertIn('对方已经取消了配对', a.ok('remote-peers', '--check'))


class ShellTest(WebBase):
    """bash 这一层：cca remote pair --web（真的 cca 脚本），终端里和界面 App 的提问会话里；手机那一头照文档自己说话（Guest）"""
    env, answers, cca, session, events = (rr.ShellTest.env, rr.ShellTest.answers, rr.ShellTest.cca, rr.ShellTest.session,
                                          staticmethod(rr.ShellTest.events))

    def setUp(self):
        super().setUp()
        self.n = 0

    def test_terminal(self):
        a = self.a
        self.assertEqual(json.loads(self.cca(a, '_gui', 'hello')[1]).get('remote_web'), 1)
        rc, out, err = self.cca(a, 'remote', 'pair', '--phone')
        self.assertEqual((rc, out), (1, ''))
        self.assertIn('用法：cca remote pair [--web]', err)
        # 不同意就不出配对码；问之前说清楚：是手机、那一页是服务器给的
        rc, out, err = self.cca(a, 'remote', 'pair', '--web', answers=['n'])
        self.assertEqual((rc, out), (1, ''))
        self.assertIn('那台手机可以在浏览器里看和操作这台机器上开着远程会话的会话', err)
        self.assertIn('管着服务器的人能换掉那一页的代码', err)
        self.assertNotIn('配对码：', err)
        self.assertEqual(a.sent(), [], '没同意：什么都没发给服务器')
        p = subprocess.Popen(['/bin/bash', str(ROOT / 'cca'), 'remote', 'pair', '--web'], env=self.env(a, CCA_TTY=self.answers('y')),
                             stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.addCleanup(lambda: (p.poll() is None and p.kill(), p.stdout.close(), p.stderr.close()))
        code, seen = None, ''
        while code is None:
            line = p.stderr.readline()
            self.assertTrue(line, '没等到配对码：' + seen)
            seen += line
            if '配对码：' in line:
                code = line.strip().split('：')[1]
        self.assertIn('手机上打开：%s/' % self.origin, seen, '先说地址，再说配对码')
        self.assertIn('添加到主屏幕', seen)
        g = Guest(self, '我的 iPhone')
        g.join(sw.code_bytes(code), a.fp)
        out, err = p.communicate(timeout=60)
        self.assertEqual((p.returncode, out), (0, ''), err)
        self.assertIn('已经和「我的 iPhone」配对：它可以接入这台机器上开着远程会话的会话', err)
        self.assertEqual(list(self.d.glob('**/cca-pair.*')), [])
        rc, out, err = self.cca(a, 'remote', 'peers')
        self.assertRegex(out, r'^[0-9a-f]{8}  我的 iPhone（手机上的网页端）  对方可以看和操作这台机器的会话  ')
        rc, out, err = self.cca(a, 'remote', 'list', '--json')
        self.assertEqual([(x['name'], x['web'], x['incoming'], x['state']) for x in json.loads(out)['peers']],
                         [('我的 iPhone', True, 'control', 'ok')])
        # 改成只看、取消配对照旧按名字；取消以后服务器上访客也没了
        rc, out, err = self.cca(a, 'remote', 'allow', '我的 iPhone', 'view')
        self.assertEqual(rc, 0, err)
        rc, out, err = self.cca(a, 'remote', 'unpair', '我的 iPhone')
        self.assertEqual(rc, 0, err)
        doc = self.remote()
        self.assertEqual((doc['edges'], doc['guests']), ({}, {}))

    def test_in_the_app(self):
        a = self.a
        p, ev = self.session(a, {'consent.remote_pair': [{'v': 'n'}]}, 'remote', 'pair', '--web')
        self.assertNotEqual(p.wait(60), 0)
        (ask,) = [e for e in self.events(ev) if e['ev'] == 'ask']
        self.assertEqual((ask['q'], ask.get('scope', 'web')), ('consent.remote_pair', 'web'))
        # 显示的那一页：先是手机上打开的地址（App 画成二维码），再是配对码；都不用回答
        p, ev = self.session(a, {'consent.remote_pair': [{'v': 'y'}], 'remote.pair_web': [None]}, 'remote', 'pair', '--web')
        (page,) = wait_for('显示配对码的那一页', lambda: [e for e in self.events(ev) if e.get('q') == 'remote.pair_web'], secs=30)
        self.assertEqual([(f['id'], f['kind']) for f in page['fields']], [('url', 'qr'), ('code', 'code')])
        self.assertEqual((page['title'], page['fields'][0]['value']), ('和手机配对', self.origin + '/'))
        code = page['fields'][1]['value']
        self.assertRegex(code, '^[0-9A-Z]{4}(-[0-9A-Z]{4}){3}$')
        Guest(self, 'iPad').join(sw.code_bytes(code), a.fp)
        self.assertEqual(p.wait(60), 0, p.stderr.read())
        (res,) = [e for e in self.events(ev) if e['ev'] == 'result']
        self.assertEqual((res['code'], '「iPad」' in res['message']), ('remote_paired', True))


class ShellOffTest(Base):
    """服务器没开网页端（或者是旧版）：cca remote pair --web 说清楚，不留下一个手机输了也没用的配对码"""
    env, answers, cca = rr.ShellTest.env, rr.ShellTest.answers, rr.ShellTest.cca

    def test_no_web(self):
        self.n = 0
        a = self.device('box')
        self.machines(a)
        rc, out, err = self.cca(a, 'remote', 'pair', '--web', answers=['y'])
        self.assertEqual((rc, out), (1, ''))
        self.assertIn('维护者的服务器还没有开网页端，手机上接入不了', err)
        self.assertNotIn('配对码：', err)
        # 没开网页端的服务器照样配得了两台机器
        r = a.run('remote-pair-show', '--web')
        self.assertEqual((r.returncode, r.stdout), (1, ''))
        self.assertEqual(self.call(a, 'pair_get', rid='0' * 32)[0], 404)


class FilesTest(unittest.TestCase):
    """页面的文件（server/web/）：服务器读得进去、只发该发的；别人的代码（xterm.js）和说明里记的校验和一样；
    页面自己不引别处的东西、没有内联的脚本（内容安全策略不许，写了也不会跑）"""
    def test_the_real_page(self):
        web = srvmod.Web('https://relay.example.com/', str(WEB), None)
        need = {'index.html', 'app.js', 'app.css', 'core.js', 'xterm.js', 'xterm.css', 'xterm-addon-fit.js', 'logo.svg',
                'manifest.webmanifest', 'icon-180.png', 'icon-192.png', 'icon-512.png'}
        self.assertEqual(set(web.files), need, '多了、少了文件：页面目录里新加的东西要想清楚该不该发给浏览器')
        self.assertLess(sum(len(f[2] or f[1]) for f in web.files.values()), 400 * 1024, '压缩以后整页不到 400 KB')
        table = dict(re.findall(r'^\| `([^`]+)` \|[^|]+\|[^|]+\| `([0-9a-f]{64})` \|$', (WEB / 'README.md').read_text(), re.M))
        self.assertEqual(set(table), {'xterm.js', 'xterm.css', 'xterm-addon-fit.js', 'xterm-LICENSE.txt'})
        for name, want in table.items():
            self.assertEqual(hashlib.sha256((WEB / name).read_bytes()).hexdigest(), want, name + '：和说明里记的不一样（被改过？）')
        html = (WEB / 'index.html').read_text()
        self.assertEqual(re.findall(r'(?:src|href)="(?![a-z0-9][a-z0-9._-]*")[^"]*"', html), [], '只引同一个目录里的文件')
        self.assertNotRegex(html, r'<script(?![^>]*\bsrc=)|\son[a-z]+\s*=|javascript:|<style', '没有内联的脚本、样式、事件')
        for name in ('app.js', 'core.js'):
            src = (WEB / name).read_text()
            self.assertNotRegex(src, r'\beval\(|new Function|innerHTML|outerHTML|insertAdjacentHTML|document\.write', name)
            self.assertNotRegex(src, r'https?://(?!localhost)', name + '：不连别处')
        m = json.loads((WEB / 'manifest.webmanifest').read_text())
        self.assertEqual((m['display'], m['start_url'], [i['src'] for i in m['icons']]), ('standalone', './', ['icon-192.png', 'icon-512.png']))
        for n in (180, 192, 512):
            png = (WEB / ('icon-%d.png' % n)).read_bytes()
            self.assertEqual((png[:8], struct.unpack('>II', png[16:24])), (b'\x89PNG\r\n\x1a\n', (n, n)))


CHROME = os.environ.get('CCA_CHROME') or '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'


@unittest.skipUnless(node_ok() and os.path.exists(CHROME), '没有 Node 22 以上或者没有 Chrome：页面这次没在浏览器里走')
class PageTest(Base):
    """真的那一页在真的浏览器里（无界面的 Chrome，照 iPhone 的屏幕、触摸；tests/web_page.mjs）：对面是真的服务器、真的 cca 会话。
    网页端开在 http://localhost（浏览器把它当安全的来源，不用证书）。截图留在 CCA_WEB_SHOTS 指的目录里（没给就不留）"""
    def server_extra(self, d):
        s = socket.socket()
        s.bind(('127.0.0.1', 0))
        self.wport = s.getsockname()[1]
        s.close()
        return ['--web-url', 'http://localhost:%d/' % self.wport, '--web-listen', '127.0.0.1:%d' % self.wport]

    def test_the_page_in_a_browser(self):
        box = self.device('box')
        self.machines(box)
        h = rr.Term([rr.HELPER, 'remote-host', 'claude', rr.Device.KEY, 'alice', '--', sys.executable, str(box.home / 'client.py')],
                    box.env, str(box.work))
        self.terms.append(h)
        h.expect('<ready 24 80')
        p = subprocess.Popen([rr.HELPER, 'remote-pair-show', '--web'], env=box.env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, text=True)
        self.addCleanup(lambda: (p.kill(), p.wait(), p.stdout.close(), p.stderr.close()))
        code, web = p.stdout.readline().split()[1], p.stdout.readline()
        url = 'http://localhost:%d/' % self.wport
        self.assertEqual(web, 'web %s\n' % url)
        shots = os.environ.get('CCA_WEB_SHOTS')
        tmp = None if shots else tempfile.TemporaryDirectory(prefix='cca-shots-')
        env = {k: v for k, v in os.environ.items() if k.lower() not in ('http_proxy', 'https_proxy', 'all_proxy')}
        try:
            r = subprocess.run([NODE, str(ROOT / 'tests' / 'web_page.mjs'), url, code, shots or tmp.name], env=dict(env, CCA_CHROME=CHROME),
                               capture_output=True, text=True, timeout=240)
        finally:
            if tmp:
                tmp.cleanup()
        try:
            j = json.loads(r.stdout)
        except ValueError:
            self.fail('浏览器那一头没给结果：%s %s' % (r.stdout[-500:], r.stderr[-1500:]))
        self.assertTrue(j['ok'], '%s\n页面上：%s\n控制台：%s' % (j.get('error'), j.get('dom'), j.get('logs')))
        self.assertEqual(j['steps'], ['welcome', 'paired', 'typed', 'keybar', 'compose', 'menu', 'reattach', 'ended', 'settings'])
        self.assertEqual(j['logs'], [], '页面的控制台里不该有报错（被内容安全策略拦下的也算）')
        self.assertEqual(j['codeShown'], code, '输入的配对码自动分成四个一组')
        self.assertIn('alice', j['home'])
        self.assertIn('会话结束了（退出码 3）', j['ended'])
        self.assertLess(j['sizeAfterFont'], j['size'][0], '字调大以后行数变少（终端跟着重新算了大小）')
        self.assertEqual(p.wait(30), 0)
        self.assertEqual(p.stdout.read(), 'peer iPhone\n')


if __name__ == '__main__':
    unittest.main(verbosity=2)
