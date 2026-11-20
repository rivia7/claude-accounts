#!/usr/bin/env python3
"""远程会话经服务器中转（docs/dev/remote.md、server/README.md「远程会话的中转」）：配对、会话的列表、端到端加密的通道。

真的服务器进程（server/cca-presence.py，127.0.0.1、随机端口，开着中转）、真的 ssh-keygen；每台设备一个临时 HOME 和自己的
deploy key（tests/presence.py 的 Device），跑的是 cca 的辅助程序，会话在真的伪终端上（tests/remote_host.py 的 Term 和 CLIENT）。
设备和数据端口之间隔着一个转发器（Sniffer），经过的字节都记下来：服务器看得到的就是这些。
另一头有时由 tests/spec_wire.py（照文档独立实现的那一份）扮演：两份实现要能互相配对、握手、收发。不联网。"""
import base64
import http.client
import json
import os
from pathlib import Path
import re
import secrets
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from helper_bin import HELPER   # noqa: E402
from presence import ROOT, SERVER, Device, keygen, make_cert, openssl_pin, sign, srvmod, wait_for   # noqa: E402
from remote_host import CLIENT, Term   # noqa: E402
import spec_wire as sw   # noqa: E402

F_JOIN, F_PING, F_PONG, F_ANNOUNCE, F_BYE = 0x02, 0x06, 0x07, 0x08, 0x09
HELLO, INPUT, RESIZE, DETACH, WELCOME, OUTPUT, EXIT = 0x01, 0x02, 0x03, 0x04, 0x11, 0x12, 0x13
# cli/src/cmd/remote/relay.rs 的同名常量
RELAY_OLD = '维护者的服务器还不支持远程会话'


# 设备和数据端口之间的转发器，自己一个进程（测试进程里不留线程：开伪终端要 fork）：两个方向经过的字节都追加到一个文件里。
# 参数：服务器的数据端口写在哪个文件里（接进来一条连接时才读）、自己监听的端口写到哪、经过的字节记到哪
SNIFFER = r'''
import socket, sys, threading
rport_file, port_file, capture = sys.argv[1:4]
lock = threading.Lock()
lsock = socket.socket()
lsock.bind(('127.0.0.1', 0))
lsock.listen(32)
with open(port_file + '.tmp', 'w') as f:
    f.write('%d' % lsock.getsockname()[1])
__import__('os').replace(port_file + '.tmp', port_file)
out = open(capture, 'ab', buffering=0)

def pump(a, b):
    try:
        while True:
            data = a.recv(65536)
            if not data:
                break
            with lock:
                out.write(data)
            b.sendall(data)
    except OSError:
        pass
    for s, how in ((b, socket.SHUT_WR), (a, socket.SHUT_RD)):
        try:
            s.shutdown(how)
        except OSError:
            pass

def pair(c, u):
    back = threading.Thread(target=pump, args=(u, c), daemon=True)
    back.start()
    pump(c, u)
    back.join()
    c.close()
    u.close()

while True:
    c, _ = lsock.accept()
    try:
        u = socket.create_connection(('127.0.0.1', int(open(rport_file).read())))
    except (OSError, ValueError):
        c.close()
        continue
    threading.Thread(target=pair, args=(c, u), daemon=True).start()
'''


class Sniffer:
    """起转发器；snapshot() 是到现在为止经过的全部字节"""
    def __init__(self, d):
        self.capture = d / 'sniffed'
        self.capture.write_bytes(b'')
        self.proc = subprocess.Popen([sys.executable, '-I', '-c', SNIFFER, str(d / 'rport'), str(d / 'sport'), str(self.capture)],
                                     stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            self.port = int(wait_for('转发器起来', lambda: (d / 'sport').exists() and (d / 'sport').read_text()))
        except BaseException:
            self.close()
            raise

    def snapshot(self):
        return self.capture.read_bytes()

    def close(self):
        self.proc.kill()
        self.proc.wait()


class RelayServer:
    """一个真的服务器进程。relay=False：没开中转（和旧版服务器一样）。sniff：告诉客户端连转发器的端口。
    port：重启时用回原来的端口（设备的映射里写着它；数据端口换了没关系，转发器每次现读）"""
    def __init__(self, d, relay=True, sniff=None, port=0):
        self.d = d
        for name in ('port', 'rport'):
            (d / name).unlink(missing_ok=True)
        args = [sys.executable, '-I', str(SERVER), '--listen', '127.0.0.1:%d' % port, '--port-file', str(d / 'port'),
                '--cert', str(d / 'cert.pem'), '--key', str(d / 'key.pem'), '--release-key', str(d / 'release.pub'),
                '--state', str(d / 'state.json'), '--ttl', '60', '--idle', '600']
        if relay:
            args += ['--relay-listen', '127.0.0.1:0', '--relay-port-file', str(d / 'rport')]
        if sniff:
            args += ['--relay-port', str(sniff.port)]
        self.proc = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            self.port = int(wait_for('服务器起来', lambda: (d / 'port').exists() and (d / 'port').read_text()))
            self.rport = int(wait_for('数据端口起来', lambda: (d / 'rport').exists() and (d / 'rport').read_text())) if relay else None
        except BaseException:
            self.proc.kill()
            self.proc.wait()
            raise

    def stop(self):
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        if not self.proc.stderr.closed:
            self.proc.stderr.close()


def read_line(s):
    out = b''
    while not out.endswith(b'\n'):
        c = s.recv(1)
        if not c:
            break
        out += c
    return out


def read_exact(s, n):
    out = b''
    while len(out) < n:
        c = s.recv(n - len(out))
        if not c:
            break
        out += c
    return out


def read_all(s):
    out = b''
    while True:
        c = s.recv(65536)
        if not c:
            return out
        out += c


def ctl(kind, payload=b''):
    return bytes([kind]) + len(payload).to_bytes(2, 'big') + payload


def read_ctl(s):
    head = read_exact(s, 3)
    if len(head) < 3:
        return None, b''
    return head[0], read_exact(s, int.from_bytes(head[1:], 'big'))


SOCKS = []   # 用例里开的连接：收尾时都关上


def close_socks():
    while SOCKS:
        SOCKS.pop().close()


def dial(port, role, ident, key):
    """连数据端口、说开场白。返回 (连接, 该回的那一行)"""
    s = socket.create_connection(('127.0.0.1', port), timeout=20)
    SOCKS.append(s)
    line, ok = sw.relay_line(role, ident, key)
    s.sendall(line)
    return s, ok


def frame(kind, payload=b''):
    return bytes([kind]) + len(payload).to_bytes(4, 'big') + payload


class Base(unittest.TestCase):
    relay = True

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='cca-relay-')
        self.addCleanup(self.tmp.cleanup)
        d = self.d = Path(self.tmp.name).resolve()
        make_cert(d)
        self.relpub = keygen(d / 'rel', 'test-release')
        (d / 'release.pub').write_text(self.relpub + ' test-release\n')
        self.sniff = Sniffer(d)
        self.addCleanup(self.sniff.close)
        self.srv = RelayServer(d, relay=self.relay, sniff=self.sniff)
        self.addCleanup(lambda: self.srv.stop())
        self.pin = openssl_pin(d / 'cert.pem')
        self.terms = []
        self.addCleanup(lambda: [t.close() for t in self.terms])
        self.addCleanup(close_socks)

    def device(self, name):
        dev = Device(self.d / name, name, {Device.KEY: 'US-A'})
        dev.policy({'url': 'https://127.0.0.1:%d' % self.srv.port, 'pins': [self.pin], 'salt': '00' * 16})
        dev.env['TERM'] = 'xterm-256color'
        dev.work = dev.home / 'work'
        dev.work.mkdir()
        (dev.home / 'client.py').write_text(CLIENT)
        return dev

    def machines(self, *devs):
        doc = json.dumps({'version': 1, 'seq': int(time.time() * 1000), 'machines': [{'key': d.pub, 'name': d.name} for d in devs]}).encode()
        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        ctx.check_hostname, ctx.verify_mode = False, ssl.CERT_NONE
        c = http.client.HTTPSConnection('127.0.0.1', self.srv.port, context=ctx, timeout=10)
        c.request('POST', '/v1/machines', body=doc, headers={'X-Cca-Sig': base64.b64encode(sign(self.d / 'rel', 'cca-devices', doc)).decode()})
        self.assertEqual(c.getresponse().status, 200)
        c.close()

    def call(self, dev, op, **extra):
        return dev.post(self.srv.port, op, extra)

    def good(self, dev, op, **extra):
        st, j = self.call(dev, op, **extra)
        self.assertEqual(st, 200, (op, j))
        return j

    def py_pair(self, a, b):
        """两台设备照文档配对（a 显示配对码）。返回 (密钥, 配对编号)"""
        c = secrets.token_bytes(10)
        kc, keys, rid = sw.pair_keys(c)
        na, nb = secrets.token_bytes(32), secrets.token_bytes(32)
        self.good(a, 'pair_open', rid=rid)
        self.good(a, 'pair_put', rid=rid, m=sw.pair_msg(keys, rid, 'a', a.fp, na, a.name))
        self.good(b, 'pair_put', rid=rid, m=sw.pair_msg(keys, rid, 'b', b.fp, nb, b.name))
        ja, jb = self.good(a, 'pair_get', rid=rid), self.good(b, 'pair_get', rid=rid)
        self.assertEqual((ja['fp'], ja['name'], jb['fp'], jb['name']), (b.fp, b.name, a.fp, a.name))
        self.assertEqual(sw.pair_open(keys, rid, 'b', ja['m'])['fp'], b.fp)
        self.assertEqual(sw.pair_open(keys, rid, 'a', jb['m'])['n'], na.hex())
        p, pid = sw.pair_secret(kc, na, nb, a.fp, b.fp)
        self.assertEqual(self.good(a, 'pair_commit', rid=rid, pid=pid)['done'], False)
        self.assertEqual(self.good(b, 'pair_commit', rid=rid, pid=pid)['done'], True)
        self.assertEqual(self.good(a, 'pair_get', rid=rid, until='done')['done'], True)
        return p, pid

    def edges(self):
        p = self.d / 'remote.json'
        return json.loads(p.read_text())['edges'] if p.exists() else {}


class ServerTest(Base):
    """服务器这一头：两边都照文档自己说话（不经 cca）"""
    def setUp(self):
        super().setUp()
        self.a, self.b, self.c = self.device('box'), self.device('laptop'), self.device('other')
        self.machines(self.a, self.b, self.c)

    def test_pairing(self):
        a, b, c = self.a, self.b, self.c
        kc, keys, rid = sw.pair_keys(secrets.token_bytes(10))
        na, nb = secrets.token_bytes(32), secrets.token_bytes(32)
        ma, mb = sw.pair_msg(keys, rid, 'a', a.fp, na, 'box'), sw.pair_msg(keys, rid, 'b', b.fp, nb, 'laptop')
        # 没开的编号：不认；编号的样子不对：拒
        self.assertEqual(self.call(b, 'pair_put', rid=rid, m=mb), (404, {'error': 'no_pairing'}))
        self.assertEqual(self.call(a, 'pair_open', rid='xyz')[0], 400)
        self.good(a, 'pair_open', rid=rid)
        self.assertEqual(self.call(c, 'pair_open', rid=rid), (409, {'error': 'taken'}), '同一个编号不能再开一次')
        self.assertEqual(self.good(a, 'pair_get', rid=rid), {'v': 1, 'ok': True, 'm': None, 'fp': None, 'name': None, 'done': False})
        self.good(a, 'pair_put', rid=rid, m=ma)
        self.assertEqual(self.call(a, 'pair_put', rid=rid, m=ma)[0], 400, '一边只能放一次')
        self.assertEqual(self.call(a, 'pair_put', rid=rid, m={'n': 'x'})[0], 400)
        # 还没人输入配对码：不知道配对码的设备拿不到显示方放的那一份（它不是这次配对的哪一边）
        self.assertEqual(self.call(c, 'pair_get', rid=rid), (409, {'error': 'taken'}))
        self.assertEqual(self.call(a, 'pair_commit', rid=rid, pid='0' * 32)[0], 400, '对方还没来')
        self.good(b, 'pair_put', rid=rid, m=mb)
        self.assertEqual(self.call(c, 'pair_put', rid=rid, m=mb), (409, {'error': 'taken'}), '第三台进不来')
        self.assertEqual(self.good(b, 'pair_get', rid=rid)['m'], ma)
        # 等对方：b 还没报，a 等 1 秒等不到；b 报了以后 a 等的那一次马上回来
        p, pid = sw.pair_secret(kc, na, nb, a.fp, b.fp)
        self.assertEqual(self.good(a, 'pair_commit', rid=rid, pid=pid)['done'], False)
        t0 = time.monotonic()
        self.assertEqual(self.good(a, 'pair_get', rid=rid, wait=1, until='done')['done'], False)
        self.assertGreater(time.monotonic() - t0, 0.9)
        got = {}
        waiter = threading.Thread(target=lambda: got.update(self.good(a, 'pair_get', rid=rid, wait=10, until='done')))
        waiter.start()
        time.sleep(0.3)
        t0 = time.monotonic()
        self.assertEqual(self.good(b, 'pair_commit', rid=rid, pid=pid)['done'], True)
        waiter.join(8)
        self.assertFalse(waiter.is_alive())
        self.assertEqual(got.get('done'), True)
        self.assertLess(time.monotonic() - t0, 5, '不用等满 10 秒')
        e = self.edges()
        self.assertEqual((list(e), e[pid]['a'], e[pid]['b']), ([pid], a.fp, b.fp))
        self.assertNotIn(p.hex(), (self.d / 'remote.json').read_text() + (self.d / 'state.json').read_text())

        # 两边报的编号不一样：不记，这次配对作废
        p2, pid2 = self.py_pair_until_commit(a, c)
        self.good(a, 'pair_commit', rid=p2, pid=pid2)
        self.assertEqual(self.call(c, 'pair_commit', rid=p2, pid='f' * 32), (409, {'error': 'mismatch'}))
        self.assertEqual(self.call(a, 'pair_get', rid=p2), (404, {'error': 'no_pairing'}))
        self.assertEqual(list(self.edges()), [pid])

        # 显示配对码的那一边中途不做了：这次配对作废，对方马上知道；不是这次配对的设备关不掉它
        rid4, _ = self.py_pair_until_commit(a, c)
        self.assertEqual(self.call(b, 'pair_close', rid=rid4), (409, {'error': 'taken'}))
        self.good(a, 'pair_close', rid=rid4)
        self.assertEqual(self.call(c, 'pair_get', rid=rid4, wait=5, until='done'), (404, {'error': 'no_pairing'}))
        self.assertEqual(list(self.edges()), [pid], '已经记下的配对不受影响')

        # 同两台设备再配一次：换掉以前的
        _, pid3 = self.py_pair(b, a)
        self.assertEqual(list(self.edges()), [pid3])
        # 不是这个配对的设备取消不了；是的那两台谁取消都行
        self.good(c, 'pair_drop', pid=pid3)
        self.assertEqual(list(self.edges()), [pid3])
        self.good(a, 'pair_drop', pid=pid3)
        self.assertEqual(self.edges(), {})

    def py_pair_until_commit(self, a, b):
        """配对到两边都放了密文为止。返回 (这次配对的编号, 该报的配对编号)"""
        kc, keys, rid = sw.pair_keys(secrets.token_bytes(10))
        na, nb = secrets.token_bytes(32), secrets.token_bytes(32)
        self.good(a, 'pair_open', rid=rid)
        self.good(a, 'pair_put', rid=rid, m=sw.pair_msg(keys, rid, 'a', a.fp, na, a.name))
        self.good(b, 'pair_put', rid=rid, m=sw.pair_msg(keys, rid, 'b', b.fp, nb, b.name))
        return rid, sw.pair_secret(kc, na, nb, a.fp, b.fp)[1]

    def host(self, dev, sid, pids):
        """dev 的会话 sid 连上控制连接（给 pids 这几个配对看）。返回 (连接, 票据的密钥)"""
        t = self.good(dev, 'relay_host', sid=sid, pids=pids)
        key = base64.b64decode(t['tkey'])
        s, ok = dial(self.srv.rport, 'host', t['tid'], key)
        self.assertEqual(read_line(s), ok)
        return s, key

    def test_sessions_tickets_and_the_pipe(self):
        a, b, c = self.a, self.b, self.c
        p, pid = self.py_pair(a, b)
        sid = secrets.token_hex(16)
        # 不是这个配对的设备开不了这个配对的会话；没有会话时接不了
        self.assertEqual(self.call(c, 'relay_host', sid=sid, pids=[pid]), (404, {'error': 'no_pairing'}))
        self.assertEqual(self.call(b, 'relay_attach', sid=sid, pid=pid), (404, {'error': 'no_session'}))
        t = self.good(a, 'relay_host', sid=sid, pids=[pid])
        self.assertEqual((t['port'], t['ttl']), (self.sniff.port, 30), '告诉客户端的是 --relay-port 给的端口')
        key = base64.b64decode(t['tkey'])
        # 开场白：认证码不对、乱说的都进不来；票据只能用一次
        for line in (sw.relay_line('host', t['tid'], b'x' * 32)[0], b'GET / HTTP/1.0\r\n\r\n', b'CCA-RELAY/1 host zz 00 00\n'):
            s = socket.create_connection(('127.0.0.1', self.srv.rport), timeout=10)
            SOCKS.append(s)
            s.sendall(line)
            self.assertIn(read_line(s), (b'CCA-RELAY/1 ERR ticket\n', b'CCA-RELAY/1 ERR bad\n'))
            self.assertEqual(s.recv(1), b'')
            s.close()
        host, ok = dial(self.srv.rport, 'host', t['tid'], key)
        self.assertEqual(read_line(host), ok, '服务器也证明它认得这张票据')
        again, _ = dial(self.srv.rport, 'host', t['tid'], key)
        self.assertEqual(read_line(again), b'CCA-RELAY/1 ERR ticket\n')
        again.close()

        blob = sw.dir_seal(p, pid, sid, {'v': 1, 'machine': 'box', 'tool': 'claude', 'name': '工作', 'cwd': '~/项目', 'since': 1, 'access': 'control'})
        host.sendall(ctl(F_ANNOUNCE, json.dumps({pid: blob}).encode()) + ctl(F_PING, b'hi'))
        self.assertEqual(read_ctl(host), (F_PONG, b'hi'))
        j = self.good(b, 'relay_list', pids=[pid, 'ab' * 16])
        self.assertEqual(j['peers'], {pid: {'fp': a.fp, 'name': 'box'}})
        (row,) = j['sessions'][pid]
        self.assertEqual((row['sid'], row['blob'], row['viewers'], row['online']), (sid, blob, 0, True))
        self.assertEqual(sw.dir_open(p, pid, sid, row['blob'])['cwd'], '~/项目')
        self.assertNotIn('项目', json.dumps(j, ensure_ascii=False), '说明是密文')
        self.assertEqual(self.good(a, 'relay_list', pids=[pid])['sessions'], {pid: []}, '自己的会话不列给自己')
        self.assertEqual(self.good(c, 'relay_list', pids=[pid]), {'v': 1, 'ok': True, 'peers': {}, 'sessions': {}})
        self.assertEqual(self.call(c, 'relay_attach', sid=sid, pid=pid), (404, {'error': 'no_session'}))
        self.assertEqual(self.call(a, 'relay_attach', sid=sid, pid=pid), (404, {'error': 'no_session'}), '宿主自己不经中转接自己')

        # 接入：服务器在控制连接上说 JOIN，宿主拿令牌另连一条，两条对接
        tv = self.good(b, 'relay_attach', sid=sid, pid=pid)
        view, vok = dial(self.srv.rport, 'view', tv['tid'], base64.b64decode(tv['tkey']))
        kind, payload = read_ctl(host)
        self.assertEqual(kind, F_JOIN)
        j = json.loads(payload)
        self.assertEqual(j['pid'], pid)
        bad, _ = dial(self.srv.rport, 'join', j['token'], b'y' * 32)
        self.assertEqual(read_line(bad), b'CCA-RELAY/1 ERR ticket\n', '令牌要配控制连接那张票据的密钥')
        bad.close()
        join, jok = dial(self.srv.rport, 'join', j['token'], key)
        self.assertEqual((read_line(join), read_line(view)), (jok, vok))
        self.assertEqual(self.good(b, 'relay_list', pids=[pid])['sessions'][pid][0]['viewers'], 1)
        view.sendall(b'from the viewer')
        self.assertEqual(read_exact(join, 15), b'from the viewer')
        # 大块的、一头读得慢：一个字节不丢；宿主说完就关，最后那几个字节也到
        big = os.urandom(3 * 1024 * 1024)
        sender = threading.Thread(target=lambda: (join.sendall(big + b'<the end>'), join.close()))
        sender.start()
        time.sleep(0.5)
        got = read_all(view)
        sender.join(10)
        self.assertEqual((len(got), got[:64], got[-9:]), (len(big) + 9, big[:64], b'<the end>'))
        self.assertEqual(got[:-9], big)
        view.close()
        wait_for('这一处接入算结束', lambda: self.good(b, 'relay_list', pids=[pid])['sessions'][pid][0]['viewers'] == 0)

        # 宿主那头不来接：接入方等到服务器说连不上
        tv = self.good(b, 'relay_attach', sid=sid, pid=pid)
        view, _ = dial(self.srv.rport, 'view', tv['tid'], base64.b64decode(tv['tkey']))
        self.assertEqual(read_ctl(host)[0], F_JOIN)
        t0 = time.monotonic()
        self.assertEqual(read_line(view), b'CCA-RELAY/1 ERR nohost\n')
        self.assertLess(time.monotonic() - t0, 19)
        view.close()

        # 会话结束（BYE）：不再列出来、接不了
        host.sendall(ctl(F_BYE))
        self.assertEqual(host.recv(1), b'')
        self.assertEqual(self.good(b, 'relay_list', pids=[pid])['sessions'], {pid: []})
        self.assertEqual(self.call(b, 'relay_attach', sid=sid, pid=pid), (404, {'error': 'no_session'}))

    def test_one_bad_connection_does_not_stop_the_relay(self):
        a, b = self.a, self.b
        p, pid = self.py_pair(a, b)
        sid = secrets.token_hex(16)
        blob = sw.dir_seal(p, pid, sid, {'v': 1})
        # 嵌套几万层的 JSON（解析时是 RecursionError）、不是 JSON、密文的样子不对：只断开这一条
        for bad in (b'[' * 30000, b'\xff\xfe', json.dumps({pid: {'n': 'x'}}).encode(), b'[]'):
            host, key = self.host(a, sid, [pid])
            host.sendall(ctl(F_ANNOUNCE, bad))
            self.assertEqual(host.recv(1), b'', bad[:20])
        # 只发 PING、不收 PONG：服务器不替它攒着，攒到上限就断开。要先把内核的缓冲区填满，服务器那头才攒得起来——回环上
        # macOS 只有几百 KB，Linux 能到十几兆（一次 7.5 MB 的 PONG 它全收得下，服务器那头一点没攒）：一直发到它断开为止
        host, key = self.host(a, sid, [pid])
        host.settimeout(20)
        try:
            for _ in range(40):
                host.sendall(ctl(F_PING) * 1500000)
        except OSError:
            pass
        try:
            while host.recv(65536):
                pass
            closed = True
        except ConnectionResetError:
            closed = True
        except OSError:
            closed = False
        self.assertTrue(closed, '只发不收的控制连接没有被断开')
        # 中转还在：会话照样开得了、列得出、接得上
        host, key = self.host(a, sid, [pid])
        host.sendall(ctl(F_ANNOUNCE, json.dumps({pid: blob}).encode()) + ctl(F_PING, b'ok'))
        self.assertEqual(read_ctl(host), (F_PONG, b'ok'))
        self.assertEqual(self.good(b, 'relay_list', pids=[pid])['sessions'][pid][0]['online'], True)
        self.assertEqual(self.srv.proc.poll(), None)

        # 控制连接断了（宿主那头网络断了一下）：会话留着、标成连不上，这时接入马上被告知；重连以后原来的说明还在
        host.close()
        wait_for('会话标成连不上', lambda: self.good(b, 'relay_list', pids=[pid])['sessions'][pid][0]['online'] is False)
        tv = self.good(b, 'relay_attach', sid=sid, pid=pid)
        view, _ = dial(self.srv.rport, 'view', tv['tid'], base64.b64decode(tv['tkey']))
        self.assertEqual(read_line(view), b'CCA-RELAY/1 ERR nohost\n')
        host, key = self.host(a, sid, [pid])
        (row,) = self.good(b, 'relay_list', pids=[pid])['sessions'][pid]
        self.assertEqual((row['online'], row['blob']), (True, blob))

    def test_revoked_device_and_dropped_pairing(self):
        a, b, c = self.a, self.b, self.c
        p, pid = self.py_pair(a, b)
        p2, pid2 = self.py_pair(a, c)
        sid = secrets.token_hex(16)
        host, key = self.host(a, sid, [pid, pid2])
        host.sendall(ctl(F_ANNOUNCE, json.dumps({q: sw.dir_seal(s, q, sid, {'v': 1}) for q, s in ((pid, p), (pid2, p2))}).encode()))
        wait_for('两个配对都看得到', lambda: all(self.good(d, 'relay_list', pids=[q])['sessions'][q] for d, q in ((b, pid), (c, pid2))))
        pipes = {}
        for dev, q in ((b, pid), (c, pid2)):   # 两台都接着
            tv = self.good(dev, 'relay_attach', sid=sid, pid=q)
            view, vok = dial(self.srv.rport, 'view', tv['tid'], base64.b64decode(tv['tkey']))
            join, jok = dial(self.srv.rport, 'join', json.loads(read_ctl(host)[1])['token'], key)
            self.assertEqual((read_line(join), read_line(view)), (jok, vok))
            pipes[q] = (view, join)
        # 取消一个配对：那台设备马上看不到、接不了，正接着的那一处两头都被断开；另一台照旧
        self.good(c, 'pair_drop', pid=pid2)
        self.assertEqual([s.recv(1) for s in pipes[pid2]], [b'', b''])
        pipes[pid][0].sendall(b'still piped')
        self.assertEqual(read_exact(pipes[pid][1], 11), b'still piped')
        self.assertEqual(self.good(c, 'relay_list', pids=[pid2]), {'v': 1, 'ok': True, 'peers': {}, 'sessions': {}})
        self.assertEqual(self.call(c, 'relay_attach', sid=sid, pid=pid2), (404, {'error': 'no_session'}))
        self.assertEqual(len(self.good(b, 'relay_list', pids=[pid])['sessions'][pid]), 1)
        # 维护者撤销了一台设备：它的配对、会话都没了（重启服务器以后也一样：配对记在文件里）
        self.machines(b, c)
        self.assertEqual(self.edges(), {})
        self.assertEqual(self.call(a, 'relay_list', pids=[pid])[0], 403)
        self.assertEqual(self.good(b, 'relay_list', pids=[pid]), {'v': 1, 'ok': True, 'peers': {}, 'sessions': {}})
        self.assertEqual([s.recv(1) for s in pipes[pid]], [b'', b''], '被撤销的设备正接着的也断开')
        self.assertEqual(host.recv(1), b'', '它的控制连接也断开')

    def test_pairings_survive_a_restart(self):
        p, pid = self.py_pair(self.a, self.b)
        self.srv.stop()
        self.srv = RelayServer(self.d, sniff=self.sniff)
        self.assertEqual(self.good(self.b, 'relay_list', pids=[pid])['peers'], {pid: {'fp': self.a.fp, 'name': 'box'}})
        self.assertEqual(oct((self.d / 'remote.json').stat().st_mode & 0o777), '0o600')


class LimitsTest(unittest.TestCase):
    """服务器的限额和记账：直接调 Remote（不经网络、不签名，时间由用例给）"""
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='cca-relay-limits-')
        self.addCleanup(self.tmp.cleanup)
        self.path = os.path.join(self.tmp.name, 'remote.json')
        self.r = srvmod.Remote(self.path, 18444)
        self.now = 1794000000.0

    @staticmethod
    def fp(i):
        return 'SHA256:' + ('%043d' % i)

    def op(self, op, dev, **j):
        return self.r.op(op, self.fp(dev), j, self.now, lambda fp: 'dev')

    def pair(self, a, b, n):
        """设备 a、b 配对，配对编号由 n 定。返回 (pair_commit 的结果, 配对编号)"""
        rid, pid = '%032x' % (1000 + n), '%032x' % n
        m = {'n': 'A' * 22 + '==', 'c': 'AAAA', 't': 'A' * 43 + '='}
        for st, j in (self.op('pair_open', a, rid=rid), self.op('pair_put', a, rid=rid, m=m), self.op('pair_put', b, rid=rid, m=m)):
            self.assertEqual(st, 200, j)
        self.assertEqual(self.op('pair_commit', a, rid=rid, pid=pid)[0], 200)
        done = self.op('pair_commit', b, rid=rid, pid=pid)
        self.now += 61   # 配好的那一次留一分钟就清掉：不占“同时开着的配对”的名额
        return done, pid

    def test_limits(self):
        # 同时开着的配对：一台设备 4 个
        for i in range(4):
            self.assertEqual(self.op('pair_open', 1, rid='%032x' % i)[0], 200)
        self.assertEqual(self.op('pair_open', 1, rid='%032x' % 9), (429, {'error': 'rate'}))
        self.assertEqual(self.op('pair_open', 2, rid='%032x' % 9)[0], 200, '别的设备不受影响')
        self.now += 601   # 过期了的不算
        self.assertEqual(self.op('pair_open', 1, rid='%032x' % 10)[0], 200)
        # 一台设备最多和 16 台配对
        for i in range(16):
            self.assertEqual(self.pair(100, 200 + i, i)[0], (200, {'v': 1, 'ok': True, 'done': True}))
        (st, j), _ = self.pair(100, 300, 99)
        self.assertEqual((st, j), (429, {'error': 'edges'}))
        self.assertEqual(len(self.r.edges), 16)
        # 没用掉的票据：16 张；过期了又能换
        pid = '%032x' % 0
        for i in range(16):
            self.assertEqual(self.op('relay_host', 100, sid='%032x' % (500 + i), pids=[pid])[0], 200)
        self.assertEqual(self.op('relay_host', 100, sid='%032x' % 600, pids=[pid]), (429, {'error': 'rate'}))
        self.now += 31
        self.assertEqual(self.op('relay_host', 100, sid='%032x' % 600, pids=[pid])[0], 200)
        # 开着的会话：8 个；别人的会话编号不能拿来用；接入的满 4 处就不给票据
        for i in range(8):
            self.r.sessions['%032x' % (700 + i)] = {'fp': self.fp(100), 'pids': [pid], 'blobs': {}, 'conn': object(), 'lost': None,
                                                    'viewers': 0, 'since': self.now}
        self.now += 31
        self.assertEqual(self.op('relay_host', 100, sid='%032x' % 800, pids=[pid]), (429, {'error': 'sessions'}))
        self.assertEqual(self.op('relay_host', 100, sid='%032x' % 700, pids=[pid])[0], 200, '已经开着的会话换票据（重连）不算新的')
        self.assertEqual(self.op('relay_host', 200, sid='%032x' % 700, pids=[pid]), (409, {'error': 'taken'}))
        self.assertEqual(self.op('relay_attach', 200, sid='%032x' % 700, pid=pid)[0], 200)
        self.r.sessions['%032x' % 700]['viewers'] = 4
        self.assertEqual(self.op('relay_attach', 200, sid='%032x' % 700, pid=pid), (429, {'error': 'busy'}))

    def test_idle_pairings_are_dropped_but_used_ones_stay(self):
        (_, _), used = self.pair(1, 2, 1)
        (_, _), idle = self.pair(1, 3, 2)
        self.r.sessions['%032x' % 50] = {'fp': self.fp(1), 'pids': [used], 'blobs': {}, 'conn': object(), 'lost': None, 'viewers': 0,
                                         'since': self.now}
        # 用过（接入过）的记下时间、写进文件：一天最多写一次；重启以后还记得
        self.now += 100 * 86400
        self.assertEqual(self.op('relay_attach', 2, sid='%032x' % 50, pid=used)[0], 200)
        saved = os.stat(self.path).st_mtime_ns
        self.now += 3600
        self.assertEqual(self.op('relay_attach', 2, sid='%032x' % 50, pid=used)[0], 200)
        self.assertEqual(os.stat(self.path).st_mtime_ns, saved, '一小时以后又用：不用再写一次盘')
        self.r = srvmod.Remote(self.path, 18444)
        self.now += 100 * 86400   # 配对以后 200 天：没用过的那个删掉，100 天前用过的留着
        self.assertEqual(self.op('relay_list', 1, pids=[used, idle])[1]['peers'], {used: {'fp': self.fp(2), 'name': 'dev'}})
        self.assertEqual(list(json.loads(Path(self.path).read_text())['edges']), [used])


class OldServerTest(Base):
    """没开中转的服务器（和旧版一样）：那几个接口不认"""
    relay = False

    def test_relay_is_off_unless_asked(self):
        a = self.device('box')
        self.machines(a)
        for op in ('pair_open', 'relay_list', 'relay_host'):
            self.assertEqual(self.call(a, op, rid='0' * 32, pids=[]), (404, {'error': 'not_found'}))
        self.assertFalse((self.d / 'remote.json').exists())
        r = a.run('remote-pair-show')
        self.assertEqual((r.returncode, r.stdout), (1, ''), '服务器不支持时不给配对码')
        self.assertIn(RELAY_OLD, r.stderr)
        # 没配置映射的机器：说清楚原因，会话照常能在这台机器上用
        (a.cca / 'policy' / 'policy.json').unlink()
        r = a.run('remote-pair-join', stdin='0000-0000-0000-0000\n')
        self.assertEqual(r.returncode, 1)
        self.assertIn('没有配置维护者的映射', r.stderr)


class PyPeer:
    """照文档自己说话的一台设备（不经 cca）：配对、接入"""
    def __init__(self, test, dev):
        self.t, self.dev = test, dev

    def join(self, code):
        """输入配对码（对面是真的 cca）"""
        t, dev = self.t, self.dev
        kc, keys, rid = sw.pair_keys(sw.code_bytes(code))
        nb = secrets.token_bytes(32)
        t.good(dev, 'pair_put', rid=rid, m=sw.pair_msg(keys, rid, 'b', dev.fp, nb, dev.name))
        j = t.good(dev, 'pair_get', rid=rid)
        theirs = sw.pair_open(keys, rid, 'a', j['m'])
        t.assertEqual(theirs['fp'], j['fp'])
        self.peer_fp = theirs['fp']
        self.p, self.pid = sw.pair_secret(kc, bytes.fromhex(theirs['n']), nb, theirs['fp'], dev.fp)
        t.good(dev, 'pair_commit', rid=rid, pid=self.pid)

    def sessions(self):
        j = self.t.good(self.dev, 'relay_list', pids=[self.pid])
        return [dict(sw.dir_open(self.p, self.pid, r['sid'], r['blob']), sid=r['sid']) for r in j['sessions'].get(self.pid, [])]

    def attach(self, sid, p=None):
        """接到会话上，握手。返回 (连接, 通道)；宿主的证明不对时通道是 None"""
        t = self.t.good(self.dev, 'relay_attach', sid=sid, pid=self.pid)
        s, ok = dial(t['port'], 'view', t['tid'], base64.b64decode(t['tkey']))
        self.t.assertEqual(read_line(s), ok)
        ch = sw.Channel('viewer', p or self.p, self.pid, sid, self.peer_fp, self.dev.fp)
        s.sendall(ch.hello())
        confirm = ch.confirm(read_exact(s, 68))
        if confirm is None:
            return s, None
        s.sendall(confirm)
        return s, ch


class Reader:
    """照文档自己说话的接入方：从通道上读帧。out 是收到的输出（会话结束记成 <EXIT 码>），kinds 是收到过的帧的类型，
    raw 是解出来的全部明文"""
    def __init__(self, sock, ch):
        self.s, self.ch, self.buf, self.plain, self.out, self.kinds, self.raw = sock, ch, bytearray(), bytearray(), b'', set(), b''

    def until(self, want, secs=15):
        """读到 want(out) 为真，返回 out。连接断了、等不到都算失败"""
        self.s.settimeout(secs)
        while not want(self.out):
            data = self.s.recv(65536)
            if not data:
                raise AssertionError('连接断了，已收到的输出：%r' % self.out[-300:])
            self.buf += data
            while True:
                rec = self.ch.open(self.buf)
                if rec is None:
                    break
                self.plain += rec
                self.raw += rec
            while len(self.plain) >= 5 and len(self.plain) >= 5 + int.from_bytes(self.plain[1:5], 'big'):
                n = int.from_bytes(self.plain[1:5], 'big')
                kind, payload = self.plain[0], bytes(self.plain[5:5 + n])
                del self.plain[:5 + n]
                self.kinds.add(kind)
                if kind == OUTPUT:
                    self.out += payload
                elif kind == EXIT:
                    self.out += b'<EXIT %d>' % int.from_bytes(payload[:4], 'big')
        return self.out


class ClientTest(Base):
    """两台设备上都是真的 cca（辅助程序）：box 上跑会话，laptop 接入"""
    def setUp(self):
        super().setUp()
        self.a, self.b = self.device('box'), self.device('laptop')
        self.machines(self.a, self.b)

    def term(self, dev, *argv, **kw):
        t = Term([HELPER, *argv], dev.env, str(dev.work), **kw)
        self.terms.append(t)
        return t

    def session(self, dev, *args):
        """dev 上起一个在宿主里运行的会话"""
        h = self.term(dev, 'remote-host', 'claude', Device.KEY, 'alice', '--', sys.executable, str(dev.home / 'client.py'), *args)
        h.expect('<ready 24 80')
        return h

    def pair(self, shower, joiner):
        """shower 显示配对码，joiner 输入。返回 (配对码, 配对编号)"""
        p = subprocess.Popen([HELPER, 'remote-pair-show'], env=shower.env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, text=True)
        try:
            line = p.stdout.readline()   # 配对码马上写出来（调用方拿去显示），配好以后才是对方的名字
            self.assertRegex(line, r'^code [0-9A-Z]{4}(-[0-9A-Z]{4}){3}$')
            code = line.split()[1]
            r = joiner.run('remote-pair-join', stdin=code.lower().replace('-', ' ') + '\n')
            self.assertEqual((r.returncode, r.stdout, r.stderr), (0, shower.name + '\n', ''))
            out, err = p.communicate(timeout=60)
        finally:
            p.kill()
        self.assertEqual((p.returncode, out, err), (0, 'peer %s\n' % joiner.name, ''))
        (pid,) = self.peers(joiner)
        return code, pid

    @staticmethod
    def peers(dev):
        p = dev.cca / 'remote' / 'peers.json'
        return json.loads(p.read_text())['peers'] if p.exists() else {}

    @staticmethod
    def secret(dev, pid):
        p = dev.cca / 'secrets' / ('cca-remote-' + pid)
        return json.loads(p.read_text())['key'] if p.exists() else None

    def far(self, dev):
        """dev 看到的别的机器上的会话：一行一个。服务器给每台设备的请求限了流（每 2 秒一个、攒得下 30 个）：
        等某个状态时一圈圈地问，两次之间隔开一秒，别把配额问光（之后那台设备的请求会被拒）"""
        asked = self.__dict__.setdefault('asked', {})
        gap = 1.0 - (time.monotonic() - asked.get(dev.name, 0))
        if gap > 0:
            time.sleep(gap)
        asked[dev.name] = time.monotonic()
        r = dev.run('remote-list', '--far')
        self.assertEqual(r.returncode, 0, r.stderr)
        return r.stdout.splitlines()

    def test_pair_list_attach(self):
        a, b = self.a, self.b
        self.assertEqual(self.far(b), [], '没配对过：不问服务器')
        self.assertEqual(b.sent(), [])
        h = self.session(a)   # 配对之前就开着的会话，配对以后也接得上
        code, pid = self.pair(a, b)

        # 两边各存了同一把密钥（密钥文件，600），配对表里是对方的指纹和名字；默认只有显示配对码的那台让对方接入
        pa, pb = self.peers(a), self.peers(b)
        self.assertEqual(list(pa), [pid])
        self.assertEqual((pa[pid]['fp'], pa[pid]['name'], pa[pid]['incoming'], pa[pid]['shown']), (b.fp, 'laptop', 'control', True))
        self.assertEqual((pb[pid]['fp'], pb[pid]['name'], pb[pid]['incoming'], pb[pid]['shown']), (a.fp, 'box', 'none', False))
        key = self.secret(a, pid)
        self.assertRegex(key, '^[0-9a-f]{64}$')
        self.assertEqual(self.secret(b, pid), key)
        for dev in (a, b):
            self.assertEqual(oct((dev.cca / 'secrets' / ('cca-remote-' + pid)).stat().st_mode & 0o777), '0o600')
            self.assertEqual(oct((dev.cca / 'remote' / 'peers.json').stat().st_mode & 0o777), '0o600')
            self.assertNotIn(key, (dev.cca / 'remote' / 'peers.json').read_text())
        # 配对码、密钥都没有发给服务器（请求里只有从配对码派生的编号和密文）
        raw = ''.join(p.read_text() for dev in (a, b) for p in (dev.home / 'sent').glob('*.json'))
        self.assertIn('pair_commit', raw)
        for secret in (code, code.replace('-', ''), key, sw.pair_keys(sw.code_bytes(code))[0].hex()):
            self.assertNotIn(secret, raw)
        self.assertEqual(sw.pair_keys(sw.code_bytes(code))[2], [r for r in a.sent('pair_open')][0]['rid'])
        self.assertEqual(list(self.edges()), [pid])
        rows = a.run('remote-peers', '--check').stdout
        self.assertRegex(rows, r'^%s  laptop  对方可以看和操作这台机器的会话  \d{4}-\d\d-\d\d 配对\n$' % pid[:8])
        self.assertRegex(b.run('remote-peers').stdout, r'^%s  box  对方不能接入这台机器  ' % pid[:8])

        # laptop 看得到 box 上的会话（box 看不到 laptop 的：没有，也不让）
        row = wait_for('laptop 列出 box 上的会话', lambda: self.far(b), secs=30)[0]
        self.assertRegex(row, r'^[0-9a-f]{8}  claude  alice  ~/work  \d\d-\d\d \d\d:\d\d 开始  在「box」上$')
        sid = row[:8]
        self.assertEqual(self.far(a), [])
        self.assertEqual(b.run('remote-list', '--local').stdout, '')
        self.assertEqual(b.run('remote-list').stdout, row + '\n')
        # 给界面 App 的那一份（PROTOCOL 9.1）：laptop 上是 box 的会话和“对方接不进来”，box 上是自己的会话和“对方能操作”
        doc = json.loads(b.ok('remote-list', '--json'))
        self.assertEqual((doc['version'], doc['kind'], doc['enabled'], doc['relay'], doc['errors']),
                         (1, 'remote', False, {'available': True, 'message': None}, []))
        (s,) = doc['sessions']
        self.assertEqual((s['short'], s['id'][:8], len(s['id']), s['where'], s['peer'], s['machine'], s['tool'], s['name'], s['cwd'],
                          s['state'], s['access'], s['local_terminal']),
                         (sid, sid, 32, 'peer', pid, 'box', 'claude', 'alice', '~/work', 'ok', 'control', None))
        self.assertLess(abs(s['since_epoch'] - time.time()), 600)
        self.assertEqual([(p['id'], p['short'], p['name'], p['incoming'], p['shown'], p['state']) for p in doc['peers']],
                         [(pid, pid[:8], 'box', 'none', False, 'ok')])
        doc = json.loads(a.ok('remote-list', '--json'))
        (s,) = doc['sessions']
        self.assertEqual((s['short'], s['where'], s['peer'], s['machine'], s['state'], s['access'], s['local_terminal'], s['viewers']),
                         (sid, 'local', None, None, 'ok', 'control', True, 0))
        self.assertEqual([(p['name'], p['incoming'], p['shown'], p['state']) for p in doc['peers']], [('laptop', 'control', True, 'ok')])
        self.assertNotIn('example.com', json.dumps(doc), '给 App 的也不带邮箱')
        doc = json.loads(a.ok('remote-list', '--json', '--local'))
        self.assertEqual((len(doc['sessions']), [p['state'] for p in doc['peers']]), (1, ['unknown']), '--local 不问服务器')

        # 接入：先收到回放，之后两边同步；按键到会话里；Ctrl-\ d 断开，会话接着运行
        v = self.term(b, 'remote-attach', sid, rows=30, cols=100)
        v.expect('正在接入「box」上的会话 %s' % sid)
        v.expect('<ready 24 80')
        h.expect('<size 24 80>|<ready')   # 两个终端都画得下的大小还是 24x80
        v.send('say typed-on-the-laptop\n')
        h.expect('<typed-on-the-laptop>')
        v.expect('<typed-on-the-laptop>')
        h.send('say typed-on-the-box\n')
        v.expect('<typed-on-the-box>')
        self.assertIn('1 处接入', a.run('remote-list', '--local').stdout)
        wait_for('列表里看得到有人接着', lambda: '，1 处接入' in ''.join(self.far(b)))
        v.send(b'\x1cd')
        self.assertEqual(v.wait(), 0)
        v.expect('已断开，会话还在运行（再接入：cca remote attach %s）' % sid)
        h.send('say still-here\n')
        h.expect('<still-here>')

        # 再接入；会话结束时两边都拿到客户端的退出码，列表里没有了
        v = self.term(b, 'remote-attach')   # 只有这一个：不用写编号
        v.expect('<still-here>')
        v.send('exit 7\n')
        self.assertEqual((h.wait(), v.wait()), (7, 7))
        v.expect('会话已经结束（退出码 7）')
        wait_for('会话从列表里消失', lambda: self.far(b) == [], secs=20)
        r = b.run('remote-attach', sid)
        self.assertEqual(r.returncode, 65)
        self.assertIn('没有编号以 %s 开头的会话' % sid, r.stderr)

        # 服务器（和路上的人）看到的：开场白、密文。终端里的字、按键一个都没有明文
        seen = self.sniff.snapshot()
        self.assertIn(b'CCA-RELAY/1 view ', seen)
        self.assertIn(b'CCA-RELAY/1 join ', seen)
        self.assertGreater(len(seen), 2000)
        for word in (b'typed-on-the-laptop', b'typed-on-the-box', b'still-here', b'ready 24 80', b'alice', b'work', key.encode()):
            self.assertNotIn(word, seen)
        state = ''.join(p.read_text() for p in self.d.glob('*.json'))
        for word in ('typed-on', 'alice@example.com', '/work', key):
            self.assertNotIn(word, state)

    def test_what_the_other_side_may_do(self):
        a, b = self.a, self.b
        code, pid = self.pair(a, b)
        h = self.session(a)
        sid = wait_for('列出会话', lambda: self.far(b), secs=30)[0][:8]
        v = self.term(b, 'remote-attach', sid)
        v.expect('<ready 24 80')
        v.send('say before\n')
        h.expect('<before>')

        # 改成只让看：正操作着的那一处被断开。它自己重新接上，这回是只看（说一句），按键到不了会话
        self.assertEqual(a.ok('remote-allow', 'laptop', 'view'), 'laptop\n')
        mark = len(v.out)
        v.expect('连接断了，正在重新接上', timeout=20, since=mark)
        v.expect('「box」现在只让这台机器看这个会话，不能操作', timeout=40, since=mark)
        v.expect('<before>', since=mark)   # 接上以后的回放
        row = wait_for('列表里说只能看', lambda: [r for r in self.far(b) if '只能看' in r], secs=30)[0]
        self.assertIn('在「box」上，只能看', row)
        mark = len(h.out)
        v.send('say sneaky\n')
        h.send('say from-the-box\n')
        v.expect('<from-the-box>')
        self.assertNotIn(b'sneaky', h.out[mark:])
        self.assertIn('对方只能看这台机器的会话', a.run('remote-peers').stdout)
        v.send(b'\x1cd')
        self.assertEqual(v.wait(), 0)
        # 新接入的一开始就说
        v = self.term(b, 'remote-attach', sid)
        v.expect('「box」只让这台机器看这个会话，不能操作')
        v.expect('<from-the-box>')
        v.send(b'\x1cd')
        self.assertEqual(v.wait(), 0)

        # 不让接入了：断开，对方的列表里没有这个会话了，也接不上。正接着的那一处试一阵（这里调成 3 秒）以后说没接上
        b.env['CCA_REMOTE_RETRY'] = '3'
        v = self.term(b, 'remote-attach', '-r', sid)
        v.expect('<from-the-box>')
        del b.env['CCA_REMOTE_RETRY']
        self.assertEqual(a.ok('remote-allow', pid[:6], 'none'), 'laptop\n')
        self.assertEqual(v.wait(30), 68)
        v.expect('连接断了，正在重新接上')
        v.expect('没重新接上：那台机器上(现在|已经)没有这个会话')
        wait_for('会话从 laptop 的列表里消失', lambda: self.far(b) == [], secs=30)
        self.assertEqual(b.run('remote-attach', sid).returncode, 65)
        # 再让：又看得到、能操作
        a.ok('remote-allow', 'laptop', 'control')
        wait_for('又列出来了', lambda: self.far(b), secs=30)
        v = self.term(b, 'remote-attach', sid)
        v.expect('<from-the-box>')
        v.send('say back-again\n')
        h.expect('<back-again>')
        v.send(b'\x1cd')
        self.assertEqual(v.wait(), 0)

        # 取消配对（laptop 这边）：密钥删掉、告诉服务器；box 那边看得出对方取消了，会话不再给它
        self.assertEqual(b.run('remote-unpair', 'box').stdout, 'box\n')
        self.assertEqual((self.peers(b), self.secret(b, pid), self.edges()), ({}, None, {}))
        self.assertIn('（对方已经取消了配对：cca remote unpair 把它删掉）', a.run('remote-peers', '--check').stdout)
        self.assertEqual(self.far(b), [])
        r = b.run('remote-unpair', 'box')
        self.assertEqual(r.returncode, 1)
        self.assertIn('没有叫「box」的配对', r.stderr)
        # cca restore 用的：全部取消，密钥都删掉
        self.assertEqual(a.ok('remote-forget'), '')
        self.assertEqual((self.peers(a), self.secret(a, pid)), ({}, None))
        h.send('exit 0\n')
        self.assertEqual(h.wait(), 0)

    def test_the_other_implementation_and_what_the_host_enforces(self):
        """另一头是照文档独立实现的那一份：能和 cca 配对、握手、收发；宿主那头的规矩不靠接入方自觉"""
        a = self.a
        other = self.device('other')
        self.machines(a, self.b, other)
        peer = PyPeer(self, other)
        p = subprocess.Popen([HELPER, 'remote-pair-show'], env=a.env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, text=True)
        try:
            peer.join(p.stdout.readline().split()[1])
            out, err = p.communicate(timeout=60)
        finally:
            p.kill()
        self.assertEqual((p.returncode, out), (0, 'peer other\n'), err)
        self.assertEqual(self.secret(a, peer.pid), peer.p.hex())

        h = self.session(a)
        (s,) = wait_for('会话的说明', peer.sessions, secs=30)
        self.assertEqual((s['machine'] != '', s['tool'], s['name'], s['cwd'], s['access']), (True, 'claude', 'alice', '~/work', 'control'))
        self.assertNotIn('example.com', json.dumps(s), '账号只给短名字')
        sid = s['sid']
        hello = frame(HELLO, bytes([1, 0, 0, 80, 0, 24]))

        # 握手过了才有输出；按键到会话里；能操作的也能改窗口大小（取都画得下的）
        sock, ch = peer.attach(sid)
        r = Reader(sock, ch)
        sock.sendall(ch.seal(hello))
        r.until(lambda out: b'<ready 24 80' in out)
        sock.sendall(ch.seal(frame(INPUT, b'say from-the-other-implementation\n')))
        h.expect('<from-the-other-implementation>')
        r.until(lambda out: b'<from-the-other-implementation>' in out)
        sock.sendall(ch.seal(frame(RESIZE, bytes([0, 70, 0, 20]))))
        h.expect('<size 20 70>')
        sock.sendall(ch.seal(frame(RESIZE, bytes([0, 80, 0, 24]))))
        h.expect('<size 24 80>')
        # 宿主的状态（里面有账号的邮箱、完整的路径、进程号）：问了不替它转，宿主自己发的欢迎帧也不转——到接入方的只有输出。
        # “结束这个会话”（0x07，cca remote end）只有那台机器上说得了：能操作的接入方发了也不转，会话照常
        sock.sendall(ch.seal(frame(0x06)))
        sock.sendall(ch.seal(frame(0x07)))
        sock.sendall(ch.seal(frame(HELLO, bytes([1, 0, 0, 80, 0, 24]))))   # 再说一遍 HELLO 也不转
        sock.sendall(ch.seal(frame(INPUT, b'say after-status\n')))
        r.until(lambda out: b'<after-status>' in out)
        self.assertEqual(r.kinds, {OUTPUT})
        for word in (b'example.com', str(a.work).encode(), b'"pid"'):
            self.assertNotIn(word, r.raw)
        # 改过的记录、重放的记录：宿主那头马上断开，不往会话里送
        rec = bytearray(ch.seal(frame(INPUT, b'say tampered\n')))
        rec[12] ^= 1
        mark = len(h.out)
        sock.sendall(bytes(rec))
        read_all(sock)   # 到宿主那头把连接关上为止
        sock.close()
        h.send('say host-still-fine\n')
        h.expect('<host-still-fine>')
        self.assertNotIn(b'tampered', h.out[mark:])

        # 没有配对密钥的（服务器自己、偷了设备 deploy key 的人）：接入方那头看得出宿主的证明不对；
        # 硬着头皮发一个证明过去，宿主不认、什么都不给
        sock, ch = peer.attach(sid, p=os.urandom(32))
        self.assertIsNone(ch)
        sock.sendall(b'CR\x03' + os.urandom(32))
        self.assertEqual(read_all(sock), b'')
        sock.close()

        # 只让看：接入方不守规矩（HELLO 里不说只看、报一个很小的窗口、照发按键、改窗口大小）也没用：
        # 按键到不了会话，会话的窗口大小不跟着它变（接入时让客户端重画的那一下是 23 行再回到 24 行）
        a.ok('remote-allow', 'other', 'view')
        wait_for('说明里改成只看', lambda: [s for s in peer.sessions() if s['access'] == 'view'], secs=30)
        mark = len(h.out)
        sock, ch = peer.attach(sid)
        r = Reader(sock, ch)
        sock.sendall(ch.seal(frame(HELLO, bytes([1, 0, 0, 20, 0, 5]))))
        r.until(lambda out: b'<host-still-fine>' in out)
        sock.sendall(ch.seal(frame(INPUT, b'say forced-input\n')))
        sock.sendall(ch.seal(frame(RESIZE, bytes([0, 2, 0, 2]))))
        h.send('say host-typed\n')
        r.until(lambda out: b'<host-typed>' in out)
        h.expect('<host-typed>', since=mark)
        self.assertNotIn(b'forced-input', h.out[mark:])
        self.assertLessEqual(set(re.findall(rb'<size (\d+ \d+)>', h.out[mark:])), {b'23 80', b'24 80'})
        self.assertIn('另有 1 处接入', a.run('remote-list', '--local').stdout)
        # 会话结束：接入方收到退出码，连接由宿主那头关上
        h.send('exit 3\n')
        self.assertEqual(h.wait(), 3)
        self.assertIn(b'<EXIT 3>', r.until(lambda out: b'<EXIT' in out))
        self.assertEqual(r.kinds, {OUTPUT, EXIT})
        sock.close()

    def test_the_link_process_is_killed(self):
        a, b = self.a, self.b
        self.pair(a, b)
        h = self.session(a)
        sid = wait_for('列出会话', lambda: self.far(b), secs=30)[0][:8]
        v = self.term(b, 'remote-attach', sid)
        v.expect('<ready 24 80')
        links = subprocess.run(['pgrep', '-f', 'remote-link %s' % sid], capture_output=True, text=True).stdout.split()
        self.assertEqual(len(links), 2, '转接进程，和它为这个接入方分出来的那一个')
        link = min(links, key=lambda p: subprocess.run(['ps', '-o', 'ppid=', '-p', p], capture_output=True, text=True).stdout.strip()
                   in links)   # 父进程不在这两个里的那个
        os.kill(int(link), 9)
        # 没人管着的那个子进程自己走（不然配对表变了它也照转）：接入方那头断开，说一句、自己接着试；会话不受影响
        mark = len(v.out)
        v.expect('连接断了，正在重新接上', timeout=20, since=mark)
        wait_for('子进程都走了', lambda: not subprocess.run(['pgrep', '-f', 'remote-link %s' % sid], capture_output=True).stdout
                 or len(subprocess.run(['pgrep', '-f', 'remote-link %s' % sid], capture_output=True).stdout.split()) == 1, secs=20)
        h.send('say link-was-killed\n')
        h.expect('<link-was-killed>')
        v.send('say typed-while-cut-off\n')   # 断着的时候敲的不攒着：接上以后不会迟到
        # 宿主过一会儿再起一个转接进程：接入方自己接上了，断着的时候那边的输出在回放里，又能操作
        v.expect('还没接上：那台机器暂时没连着服务器。接着试…', timeout=30, since=mark)
        v.expect('<link-was-killed>', timeout=90, since=mark)
        v.send('say back-again\n')
        h.expect('<back-again>')
        self.assertNotIn(b'typed-while-cut-off', h.out)
        v.send('exit 0\n')
        self.assertEqual((h.wait(), v.wait()), (0, 0))

    def test_the_server_restarts_and_the_session_ends_meanwhile(self):
        a, b = self.a, self.b
        self.pair(a, b)
        h = self.session(a)
        sid = wait_for('列出会话', lambda: self.far(b), secs=30)[0][:8]
        v = self.term(b, 'remote-attach', sid)
        v.expect('<ready 24 80')
        v.send('alt\n')   # 会话打开了备用屏幕、藏了光标、开了括号粘贴
        v.expect('<alt>')
        # 服务器重启（两头的连接都断了）：等的时候先把会话在这个终端上打开的模式关掉再说话；两头各自连回来，自己接上，
        # 模式由那边照新接入的补上
        port = self.srv.port
        mark = len(v.out)
        self.srv.stop()
        v.expect('连接断了，正在重新接上', timeout=20, since=mark)
        cut = v.out[mark:]
        self.assertLess(cut.index(b'\x1b[?1049l'), cut.index('连接断了'.encode()))
        self.assertIn(b'\x1b[?2004l', cut)
        self.srv = RelayServer(self.d, sniff=self.sniff, port=port)
        again = v.expect(r'\x1b\[\?1049h', timeout=90, since=mark + len(cut))
        v.send('say after-the-restart\n')
        h.expect('<after-the-restart>')
        v.expect('<after-the-restart>', since=again)

        # 等的时候断开的键照样管用
        self.srv.stop()
        v.expect('连接断了，正在重新接上', timeout=20, since=again)
        v.send(b'\x1cd')
        self.assertEqual(v.wait(), 0)
        v.expect('已断开，会话还在运行')
        # 断着的时候会话结束了：服务器回来以后说没有这个会话了，试一阵（这里调成 10 秒：第 1、3、7、15 秒各试一次，
        # 机器慢的时候服务器晚几秒回来也赶得上最后那一次）说没接上，退出码 68
        self.srv = RelayServer(self.d, sniff=self.sniff, port=port)
        b.env['CCA_REMOTE_RETRY'] = '10'
        v = None
        for _ in range(60):   # 宿主那头的转接要过一阵才连回来
            t = self.term(b, 'remote-attach', sid)
            try:
                t.expect('<after-the-restart>', timeout=3)
                v = t
                break
            except AssertionError:
                t.close()
                time.sleep(1)
        self.assertIsNotNone(v, '服务器重启以后会话没有再登记上')
        self.srv.stop()
        v.expect('连接断了，正在重新接上', timeout=20)
        h.send('exit 0\n')
        self.assertEqual(h.wait(), 0)
        self.srv = RelayServer(self.d, sniff=self.sniff, port=port)   # 马上回来：最后那一次问得到它，原因是“没有这个会话”
        self.assertEqual(v.wait(60), 68)
        v.expect('没重新接上：那台机器上(现在|已经)没有这个会话')
        self.assertIn('cca remote attach %s' % sid, v.out.decode(errors='replace'))

    def test_local_sessions_never_touch_the_network(self):
        a, b = self.a, self.b
        self.pair(a, b)
        h = self.session(a)
        before = len(a.sent())
        wait_for('转接进程连上了服务器', lambda: a.sent('relay_host'), secs=30)
        n = len(a.sent())
        self.assertGreater(n, before)
        # 这台机器上的会话：列出来、接进去都不问服务器
        self.assertEqual(len(a.run('remote-list', '--local').stdout.splitlines()), 1)
        v = self.term(a, 'remote-attach')
        v.expect('<ready 24 80')
        v.send(b'\x1cd')
        self.assertEqual(v.wait(), 0)
        self.assertEqual([r['op'] for r in a.sent()[n:]], [])
        # 服务器停了：会话照常，这台机器上照样接得进去；laptop 那边说清楚没查到
        self.srv.stop()
        h.send('say server-is-down\n')
        h.expect('<server-is-down>')
        v = self.term(a, 'remote-attach')
        v.expect('<server-is-down>')
        v.send(b'\x1cd')
        self.assertEqual(v.wait(), 0)
        r = b.run('remote-list')
        self.assertEqual((r.returncode, r.stdout), (0, ''))
        self.assertIn('别的机器上的会话没查到：连不上', r.stderr)
        doc = json.loads(a.ok('remote-list', '--json'))   # 给 App 的：没问到不算失败，这台机器上的照给，原因在 errors 里
        self.assertEqual((len(doc['sessions']), [e['code'] for e in doc['errors']], [p['state'] for p in doc['peers']]),
                         (1, ['relay_unreachable'], ['unknown']))
        h.send('exit 0\n')
        self.assertEqual(h.wait(), 0)

    def test_pairing_errors(self):
        a, b = self.a, self.b
        for bad in ('', 'hello', '0000-0000-0000-000', '0000-0000-0000-0000-0'):
            r = b.run('remote-pair-join', stdin=bad + '\n')
            self.assertEqual(r.returncode, 1, bad)
            self.assertIn('配对码不对', r.stderr)
        self.assertEqual(b.sent(), [], '样子不对的配对码不发请求')
        r = b.run('remote-pair-join', stdin='0000-0000-0000-0000\n')
        self.assertEqual(r.returncode, 1)
        self.assertIn('没有这个配对码', r.stderr)
        # 自己显示的配对码自己输入：不行；配对码不能用第二次
        p = subprocess.Popen([HELPER, 'remote-pair-show'], env=a.env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, text=True)
        try:
            code = p.stdout.readline().split()[1]
            r = a.run('remote-pair-join', stdin=code + '\n')
            self.assertEqual(r.returncode, 1)
            self.assertIn('这个配对码用不了', r.stderr)
            self.assertEqual(b.run('remote-pair-join', stdin=code + '\n').returncode, 0)
            self.assertEqual(p.wait(60), 0)
        finally:
            p.kill()
            p.communicate()
        r = b.run('remote-pair-join', stdin=code + '\n')
        self.assertEqual(r.returncode, 1)
        self.assertEqual(len(self.peers(a)), 1)
        # 反过来再配一次（laptop 显示）：换成新的一把密钥，以前让的照旧让，现在两边都能接入对方
        (old,) = self.peers(a)
        code, pid = self.pair(b, a)
        self.assertNotEqual(pid, old)
        self.assertEqual((self.secret(a, old), self.secret(b, old), list(self.edges())), (None, None, [pid]))
        self.assertEqual(([p['incoming'] for p in self.peers(a).values()], [p['incoming'] for p in self.peers(b).values()]),
                         (['control'], ['control']))


class ShellTest(Base):
    """bash 这一层：真的 cca 脚本（仓库里的那一份，辅助程序照 helper_bin 找到的），两台设备。终端里的提问用答案文件（CCA_TTY），
    界面 App 的提问会话由 tests/gui_session.py 扮演（事件文件、答案管道）"""
    def setUp(self):
        super().setUp()
        self.a, self.b = self.device('box'), self.device('laptop')
        self.machines(self.a, self.b)
        self.n = 0

    def env(self, dev, **more):
        return dict(dev.env, CCA_HELPER=HELPER, LANG='en_US.UTF-8', TERM='dumb', **more)

    def answers(self, *lines):
        self.n += 1
        f = self.d / ('answers.%d' % self.n)
        f.write_text(''.join(line + '\n' for line in lines))
        return str(f)

    def cca(self, dev, *argv, answers=None, events=None):
        """运行 cca 参数…：返回 (退出码, stdout, stderr)。answers：终端里的回答（一行一个）；events：事件文件"""
        env = self.env(dev)
        if answers is not None:
            env['CCA_TTY'] = self.answers(*answers)
        if events is not None:
            Path(events).write_text('')
            env['CCA_EVENTS'] = str(events)
        r = subprocess.run(['/bin/bash', str(ROOT / 'cca'), *argv], env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=120)
        return r.returncode, r.stdout, r.stderr

    def session(self, dev, table, *argv, opts=()):
        """界面 App 那样开一条提问会话：返回 (进程, 事件文件)"""
        self.n += 1
        ev, ans = self.d / ('ev.%d.jsonl' % self.n), self.d / ('table.%d.json' % self.n)
        ev.write_text('')
        ans.write_text(json.dumps(table))
        cmd = [sys.executable, '-I', str(ROOT / 'tests' / 'gui_session.py'), str(ev), str(ans), *opts, '--', '/bin/bash', str(ROOT / 'cca'), *argv]
        p = subprocess.Popen(cmd, env=self.env(dev, CCA_EVENTS=str(ev), CCA_PROMPT='1'), stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, text=True)
        self.addCleanup(lambda: (p.poll() is None and p.kill(), p.communicate()))
        return p, ev

    @staticmethod
    def events(path):
        return [json.loads(line) for line in Path(path).read_text().splitlines() if line.strip()]

    def test_terminal(self):
        a, b = self.a, self.b
        # 不同意就不出配对码
        rc, out, err = self.cca(a, 'remote', 'pair', answers=['n'])
        self.assertEqual((rc, out), (1, ''))
        self.assertIn('那台机器可以看和操作这台机器上开着远程会话的会话', err)
        self.assertIn('已取消', err)
        self.assertNotIn('配对码：', err)
        self.assertEqual(a.sent(), [], '没同意：什么都没发给服务器')
        # 同意：打印配对码、等对方；另一台输入（小写、空格也行）
        p = subprocess.Popen(['/bin/bash', str(ROOT / 'cca'), 'remote', 'pair'], env=self.env(a, CCA_TTY=self.answers('y')),
                             stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.addCleanup(lambda: (p.poll() is None and p.kill()))
        code = None
        while code is None:
            line = p.stderr.readline()
            self.assertTrue(line, '没等到配对码')
            if '配对码：' in line:
                code = line.strip().split('：')[1]
        self.assertRegex(code, '^[0-9A-Z]{4}(-[0-9A-Z]{4}){3}$')
        rc, out, err = self.cca(b, 'remote', 'join', answers=[code.lower().replace('-', ' ')])
        self.assertEqual((rc, out), (0, ''), err)
        self.assertIn('已经和「box」配对：这台机器可以接入它上面的会话', err)
        out, err = p.communicate(timeout=60)
        self.assertEqual((p.returncode, out), (0, ''), err)
        self.assertIn('已经和「laptop」配对：它可以接入这台机器上开着远程会话的会话', err)
        self.assertIn('cca remote allow laptop view', err)
        self.assertEqual(list((self.d).glob('**/cca-pair.*')), [])
        # 看、改、取消：stdout 只有数据（peers 的行），说明都在 stderr
        rc, out, err = self.cca(a, 'remote', 'peers')
        self.assertEqual(rc, 0, err)
        self.assertRegex(out, r'^[0-9a-f]{8}  laptop  对方可以看和操作这台机器的会话  ')
        rc, out, err = self.cca(a, 'remote', 'allow', 'laptop', 'view')
        self.assertEqual((rc, out), (0, ''), err)
        self.assertIn('「laptop」只能看这台机器上的会话', err)
        rc, out, err = self.cca(a, 'remote', 'allow', 'nobody', 'view')
        self.assertEqual((rc, out), (1, ''))
        self.assertIn('没有叫「nobody」的配对', err)
        rc, out, err = self.cca(a, 'remote')
        self.assertIn('配对过 1 台机器', err)
        rc, out, err = self.cca(b, 'remote', 'list', '--json')
        self.assertEqual(rc, 0, err)
        self.assertEqual([p['name'] for p in json.loads(out)['peers']], ['box'])
        rc, out, err = self.cca(b, 'remote', 'unpair', 'box')
        self.assertEqual((rc, out), (0, ''), err)
        self.assertIn('已经取消和「box」的配对', err)
        self.assertEqual(self.edges(), {})
        rc, out, err = self.cca(b, 'remote', 'join', answers=['0000-0000-0000-0000'])
        self.assertEqual((rc, out), (1, ''))
        self.assertIn('没有这个配对码', err)
        self.assertEqual(self.cca(b, 'remote', 'join', answers=[''])[0], 1)

    def test_in_the_app(self):
        a, b = self.a, self.b
        hello = json.loads(self.cca(a, '_gui', 'hello')[1])
        self.assertEqual(hello.get('remote'), 1)
        # 同意在会话里当场问；答否：按取消收场，不出配对码
        p, ev = self.session(a, {'consent.remote_pair': [{'v': 'n'}]}, 'remote', 'pair')
        self.assertNotEqual(p.wait(60), 0)
        es = self.events(ev)
        self.assertEqual([e['q'] for e in es if e['ev'] == 'ask'], ['consent.remote_pair'])
        self.assertEqual([e['code'] for e in es if e['ev'] == 'fail'], ['cancelled'])
        # 显示配对码的那一页上取消（App 关掉答案管道）：辅助程序跟着停下——不然它会在后台把配对做完
        p, ev = self.session(a, {'consent.remote_pair': [{'v': 'y'}]}, 'remote', 'pair', opts=('--eof-at', 'remote.pair'))
        self.assertNotEqual(p.wait(60), 0)
        es = self.events(ev)
        self.assertEqual([e['q'] for e in es if e['ev'] == 'ask'], ['consent.remote_pair', 'remote.pair'])
        self.assertEqual([e['code'] for e in es if e['ev'] == 'fail'][-1], 'cancelled')
        (page,) = [e for e in es if e.get('q') == 'remote.pair']
        (field,) = page['fields']
        self.assertEqual((field['kind'], field['id']), ('code', 'code'))
        cancelled = field['value']
        wait_for('辅助程序停下', lambda: not subprocess.run(['pgrep', '-f', 'remote-pair-show'], capture_output=True).stdout, secs=10)
        t0 = time.monotonic()
        rc, out, err = self.cca(b, 'remote', 'join', answers=[cancelled])   # 那个配对码作废了：输进去马上知道，不用干等
        self.assertNotEqual(rc, 0)
        self.assertIn('没有这个配对码', err)
        self.assertLess(time.monotonic() - t0, 20)
        self.assertEqual((ClientTest.peers(a), ClientTest.peers(b), self.edges()), ({}, {}, {}))
        # 配好：一页显示配对码（不用回答），另一台在它自己的会话里输入；两边各有一条结果
        p, ev = self.session(a, {'consent.remote_pair': [{'v': 'y'}], 'remote.pair': [None]}, 'remote', 'pair')
        code = wait_for('显示配对码的那一页', lambda: [e['fields'][0]['value'] for e in self.events(ev) if e.get('q') == 'remote.pair'], secs=30)[0]
        self.assertRegex(code, '^[0-9A-Z]{4}(-[0-9A-Z]{4}){3}$')
        p2, ev2 = self.session(b, {'remote.code': [{'v': code}]}, 'remote', 'join')
        self.assertEqual((p2.wait(60), p.wait(60)), (0, 0), (p2.stderr.read(), p.stderr.read()))
        (res,) = [e for e in self.events(ev) if e['ev'] == 'result']
        self.assertEqual((res['code'], '「laptop」' in res['message']), ('remote_paired', True))
        (res,) = [e for e in self.events(ev2) if e['ev'] == 'result']
        self.assertEqual((res['code'], '「box」' in res['message']), ('remote_paired', True))
        (ask,) = [e for e in self.events(ev2) if e['ev'] == 'ask']
        self.assertEqual((ask['q'], [f['kind'] for f in ask['fields']]), ('remote.code', ['text']))
        self.assertEqual(len(self.edges()), 1)
        # 不用问的命令：做完有结果事件，没做成有原因
        ev3 = self.d / 'ev.cmd.jsonl'
        rc, out, err = self.cca(a, 'remote', 'allow', 'laptop', 'none', events=ev3)
        self.assertEqual(rc, 0, err)
        self.assertEqual([(e['ev'], e['code']) for e in self.events(ev3)], [('result', 'remote_updated')])
        rc, out, err = self.cca(a, 'remote', 'allow', 'nobody', 'view', events=ev3)
        self.assertEqual(rc, 1)
        (fail,) = self.events(ev3)
        self.assertEqual((fail['ev'], fail['code'], '没有叫「nobody」的配对' in fail['message']), ('fail', 'error', True))
        for argv in (('remote', 'on'), ('remote', 'off'), ('remote', 'unpair', 'laptop')):
            rc, out, err = self.cca(a, *argv, events=ev3)
            self.assertEqual(rc, 0, err)
            self.assertEqual([(e['ev'], e['code']) for e in self.events(ev3)], [('result', 'remote_updated')], argv)
        # 配对码不对：会话里说清楚原因
        p, ev = self.session(b, {'remote.code': [{'v': '0000-0000-0000-0000'}]}, 'remote', 'join')
        self.assertNotEqual(p.wait(60), 0)
        self.assertIn('没有这个配对码', [e for e in self.events(ev) if e['ev'] == 'fail'][-1]['message'])


if __name__ == '__main__':
    unittest.main()
