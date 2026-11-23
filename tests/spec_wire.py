"""照 server/README.md 写的格式独立实现一遍（不用 cca 的代码），核对 cca 封的、解的：记下的 API key、折合金额两种密文，
和设备拿它在 Team 里的密钥签名的请求。tests/presence.py、tests/cost_view.py、regress 的 dkey / dcost（扮演另一台设备 dan-air）共用"""
import base64
import hashlib
import hmac
import http.client
import json
import secrets
import ssl
import subprocess
import tempfile
import time
import zlib

KEYSHARE, COST = 'cca-keyshare-v1', 'cca-cost-v1'


def derive(label, salt, tool, email, ident):
    """两把 HMAC 密钥（加密、认证）：由 Team 的配置里的盐（十六进制）、工具、邮箱（小写）和登录身份派生"""
    root = hmac.new(bytes.fromhex(salt), ('%s\0%s\0%s\0%s' % (label, tool, email.lower(), ident)).encode(), hashlib.sha256).digest()
    return hmac.new(root, b'enc', hashlib.sha256).digest(), hmac.new(root, b'mac', hashlib.sha256).digest()


def stream(enc, n, size):
    """密钥流：HMAC(enc, nonce + 8 字节大端的块号)，每块 32 字节"""
    return b''.join(hmac.new(enc, n + i.to_bytes(8, 'big'), hashlib.sha256).digest() for i in range((size + 31) // 32))


def tag(label, mac, ad, n, c):
    """认证码：标签和 ad（账号摘要；折合金额还有设备指纹）各跟一个 \\0，接着 nonce 和密文"""
    return hmac.new(mac, ''.join(x + '\0' for x in [label] + list(ad)).encode() + n + c, hashlib.sha256).digest()


def seal(label, keys, ad, pt):
    n = secrets.token_bytes(16)
    c = bytes(x ^ y for x, y in zip(pt, stream(keys[0], n, len(pt))))
    return {k: base64.b64encode(v).decode() for k, v in (('n', n), ('c', c), ('t', tag(label, keys[1], ad, n, c)))}


def unseal(label, keys, ad, blob):
    """认证过的明文；认证不过是 None"""
    n, c, t = (base64.b64decode(blob[k]) for k in 'nct')
    if not hmac.compare_digest(t, tag(label, keys[1], ad, n, c)):
        return None
    return bytes(x ^ y for x, y in zip(c, stream(keys[0], n, len(c))))


def keyshare_seal(keys, aid, doc):
    """记下的 API key：明文是 JSON，空格补齐到 256 字节的整数倍；认证带着账号摘要"""
    pt = json.dumps(doc).encode()
    return seal(KEYSHARE, keys, [aid], pt + b' ' * (-len(pt) % 256))


def keyshare_open(keys, aid, blob):
    pt = unseal(KEYSHARE, keys, [aid], blob)
    return None if pt is None else json.loads(pt)


def cost_plain(z):
    """折合金额的明文：4 字节大端的长度 + zlib 压缩的 JSON，零补齐到 1024 字节的整数倍"""
    pt = len(z).to_bytes(4, 'big') + z
    return pt + b'\0' * (-len(pt) % 1024)


def cost_seal(keys, aid, fp, doc, raw=None):
    """认证带着账号摘要和设备指纹；raw：直接给压缩好的（测解压炸弹之类）"""
    return seal(COST, keys, [aid, fp], cost_plain(raw if raw is not None else zlib.compress(json.dumps(doc).encode())))


def cost_open(keys, aid, fp, blob):
    pt = unseal(COST, keys, [aid, fp], blob)
    if pt is None:
        return None
    size = int.from_bytes(pt[:4], 'big')
    assert len(pt) % 1024 == 0 and set(pt[4 + size:]) <= {0}, '补齐到 1024 字节的整数倍，补的是零'
    return json.loads(zlib.decompress(pt[4:4 + size]))


def aid_of(salt, tool, email):
    """上报用的账号摘要：HMAC(盐, 工具 \\0 邮箱) 的前 32 个十六进制字符"""
    return hmac.new(bytes.fromhex(salt), ('%s\0%s' % (tool, email)).encode(), hashlib.sha256).hexdigest()[:32]


def fp_of(pubfile):
    """设备在 Team 里的那把密钥（公钥文件）的指纹 SHA256:…"""
    with open(pubfile) as f:
        pub = f.read().split()[1]
    return 'SHA256:' + base64.b64encode(hashlib.sha256(base64.b64decode(pub)).digest()).decode().rstrip('=')


# ---------------------------------------------------------------- 远程会话（server/README.md「远程会话的中转」）
B32 = '0123456789ABCDEFGHJKMNPQRSTVWXYZ'   # Crockford base32


def H(key, msg):
    return hmac.new(key, msg, hashlib.sha256).digest()


def code_text(c):
    """配对码：10 个字节写成 16 个字符，四个一组"""
    n = int.from_bytes(c, 'big')
    s = ''.join(B32[(n >> (5 * i)) & 31] for i in range(15, -1, -1))
    return '-'.join(s[i:i + 4] for i in range(0, 16, 4))


def code_bytes(text):
    """输入的配对码（大小写、横线、空格随意；I L 当 1、O 当 0）；不对是 None"""
    s = text.upper().replace('-', '').replace(' ', '').replace('I', '1').replace('L', '1').replace('O', '0')
    if len(s) != 16 or any(ch not in B32 for ch in s):
        return None
    n = 0
    for ch in s:
        n = n * 32 + B32.index(ch)
    return n.to_bytes(10, 'big')


def pair_keys(c):
    """配对码 → (配对消息的两把密钥, 服务器上认这次配对的编号)。配对码只有 80 位，所以这一步故意算得慢"""
    kc = hashlib.pbkdf2_hmac('sha256', c, b'cca-remote/v1/pair', 65536)
    return kc, (H(kc, b'enc'), H(kc, b'mac')), H(kc, b'rid').hex()[:32]


def pair_msg(keys, rid, role, fp, nonce, name):
    """配对时放到服务器上的那一份：role 是 "a"（显示配对码的）/ "b"（输入的）"""
    pt = json.dumps({'v': 1, 'fp': fp, 'n': nonce.hex(), 'name': name}, ensure_ascii=False, separators=(',', ':')).encode()
    return seal('cca-remote-pair', keys, [rid, role], pt + b' ' * (-len(pt) % 256))


def pair_open(keys, rid, role, blob):
    pt = unseal('cca-remote-pair', keys, [rid, role], blob)
    return None if pt is None else json.loads(pt)


def pair_secret(kc, na, nb, fpa, fpb):
    """两台机器之间的密钥，和它的编号（服务器只知道编号）"""
    p = H(kc, b'secret\0' + na + nb + fpa.encode() + b'\0' + fpb.encode() + b'\0')
    return p, H(p, b'cca-remote/v1/pid').hex()[:32]


def dir_keys(p):
    k = H(p, b'cca-remote/v1/dir')
    return H(k, b'enc'), H(k, b'mac')


def dir_seal(p, pid, sid, doc):
    pt = json.dumps(doc, ensure_ascii=False, separators=(',', ':')).encode()
    return seal('cca-remote-dir', dir_keys(p), [pid, sid], pt + b' ' * (-len(pt) % 256))


def dir_open(p, pid, sid, blob):
    pt = unseal('cca-remote-dir', dir_keys(p), [pid, sid], blob)
    return None if pt is None else json.loads(pt)


class Channel:
    """两头之间的加密通道。握手：接入方发 hello()，宿主回 reply()，接入方再发 confirm()；之后两个方向各用各的密钥、
    各从 0 数序号。role 是 "host" / "viewer"；fph、fpv 是各自记着的宿主、接入方的指纹"""
    VER = 1

    def __init__(self, role, p, pid, sid, fph, fpv):
        self.role, self.ka, self.pid, self.sid, self.fph, self.fpv = role, H(p, b'cca-remote/v1/auth'), pid, sid, fph, fpv
        self.send_seq = self.recv_seq = 0

    def ctx(self):
        return bytes([self.VER]) + bytes.fromhex(self.pid) + bytes.fromhex(self.sid) + self.nv + self.nh \
            + self.fph.encode() + b'\0' + self.fpv.encode() + b'\0'

    def keys(self):
        ks = H(self.ka, b'session\0' + self.ctx() + self.tagh + self.tagv)
        k = {n: H(ks, n.encode()) for n in ('h2v-enc', 'h2v-mac', 'v2h-enc', 'v2h-mac')}
        mine, theirs = ('h2v', 'v2h') if self.role == 'host' else ('v2h', 'h2v')
        self.out, self.inn = (k[mine + '-enc'], k[mine + '-mac']), (k[theirs + '-enc'], k[theirs + '-mac'])

    def hello(self, nv=None):   # 接入方 → 宿主（52 字节）
        self.nv = nv or secrets.token_bytes(32)
        return b'CR\x01' + bytes([self.VER]) + self.nv + bytes.fromhex(self.pid)

    def reply(self, hello, nh=None):   # 宿主 → 接入方（68 字节）；hello 不对是 None
        if len(hello) != 52 or hello[:4] != b'CR\x01' + bytes([self.VER]) or hello[36:] != bytes.fromhex(self.pid):
            return None
        self.nv, self.nh = hello[4:36], nh or secrets.token_bytes(32)
        self.tagh = H(self.ka, b'host\0' + self.ctx())
        return b'CR\x02' + bytes([self.VER]) + self.nh + self.tagh

    def confirm(self, reply):   # 接入方 → 宿主（35 字节）；宿主的证明不对是 None
        if len(reply) != 68 or reply[:4] != b'CR\x02' + bytes([self.VER]):
            return None
        self.nh, self.tagh = reply[4:36], reply[36:]
        if not hmac.compare_digest(self.tagh, H(self.ka, b'host\0' + self.ctx())):
            return None
        self.tagv = H(self.ka, b'viewer\0' + self.ctx() + self.tagh)
        self.keys()
        return b'CR\x03' + self.tagv

    def accept(self, confirm):   # 宿主收到接入方的证明：对不对
        if len(confirm) != 35 or confirm[:3] != b'CR\x03':
            return False
        self.tagv = confirm[3:]
        if not hmac.compare_digest(self.tagv, H(self.ka, b'viewer\0' + self.ctx() + self.tagh)):
            return False
        self.keys()
        return True

    def seal(self, data):
        """一条记录：序号(8) 密文长度(2) 密文 认证(16)。明文是 长度(2) 内容 零补齐到 32 的整数倍"""
        assert len(data) <= 16384
        pt = len(data).to_bytes(2, 'big') + data
        pt += b'\0' * (-len(pt) % 32)
        seq = self.send_seq.to_bytes(8, 'big')
        self.send_seq += 1
        ct = bytes(x ^ y for x, y in zip(pt, stream(self.out[0], seq, len(pt))))
        head = seq + len(ct).to_bytes(2, 'big')
        return head + ct + H(self.out[1], head + ct)[:16]

    def open(self, buf):
        """buf（bytearray）开头的一条记录：内容；还不够一条是 None；序号、认证不对抛 ValueError"""
        if len(buf) < 10:
            return None
        n = int.from_bytes(buf[8:10], 'big')
        if n > 16384 + 2 + 31 or n % 32 or n == 0:
            raise ValueError('len')
        if len(buf) < 10 + n + 16:
            return None
        head, ct, tag_ = bytes(buf[:10]), bytes(buf[10:10 + n]), bytes(buf[10 + n:26 + n])
        if head[:8] != self.recv_seq.to_bytes(8, 'big') or not hmac.compare_digest(tag_, H(self.inn[1], head + ct)[:16]):
            raise ValueError('auth')
        self.recv_seq += 1
        del buf[:26 + n]
        pt = bytes(x ^ y for x, y in zip(ct, stream(self.inn[0], head[:8], n)))
        size = int.from_bytes(pt[:2], 'big')
        if size > n - 2 or any(pt[2 + size:]):
            raise ValueError('pad')
        return pt[2:2 + size]


def relay_line(role, ident, key, cn=None):
    """连数据端口时说的那一行（role：host / view 用票据的编号和密钥，join 用令牌和控制连接那张票据的密钥），和该回的那一行"""
    cn = cn or secrets.token_hex(8)
    b64u = lambda b: base64.urlsafe_b64encode(b).rstrip(b'=').decode()
    lead = 'j' if role == 'join' else 'c' + role
    line = 'CCA-RELAY/1 %s %s %s %s\n' % (role, ident, cn, b64u(H(key, (lead + ident + cn).encode())))
    return line.encode(), ('CCA-RELAY/1 OK %s\n' % b64u(H(key, ('s' + ident + cn).encode()))).encode()


# ---- Team（README「Team 的同步服务器」）----
TEAM_LINK = 'cca-team1.'
TEAM_DOC = 'cca-team-doc-v1'


def team_id(root_pub):
    """Team 的编号：根公钥（'ssh-ed25519 AAAA…' 里 base64 那一段解出来的字节）的 SHA-256 的前 32 个十六进制字符"""
    return hashlib.sha256(base64.b64decode(root_pub.split()[1])).hexdigest()[:32]


def invite_parts(secret):
    """邀请里的那串随机数（32 个小写十六进制字符，解出来的 16 个字节当 HMAC 的密钥）派生出来的三样：加入时交给服务器的令牌、
    这台设备在 Team 里的编号、它那份配置的两把密钥（加密、认证）"""
    if len(secret) != 32 or set(secret) - set('0123456789abcdef'):
        raise ValueError('那串随机数要是 32 个小写十六进制字符')
    k = bytes.fromhex(secret)
    root = H(k, b'cca-team-key-v1')
    return H(k, b'cca-team-token-v1')[:16].hex(), H(k, b'cca-team-dev-v1')[:8].hex(), (H(root, b'enc'), H(root, b'mac'))


def team_pub_proof(secret, pub):
    """设备加入时给自己的公钥（'ssh-ed25519 AAAA…' 那一行的前两段）算的认证码：HMAC(HMAC(k, "cca-team-pub-v1"), 那一行)，base64"""
    key = H(bytes.fromhex(secret), b'cca-team-pub-v1')
    return base64.b64encode(H(key, ' '.join(pub.split()[:2]).encode())).decode()


def invite_encode(url, pins, root_pub, secret, name, exp=None, device=None):
    """邀请链接：cca-team1.<base64url(紧凑的 JSON)，不带 =>.<前面这一段的 SHA-256 的前 4 个十六进制字符>；device 是管理员给这台设备起的名字（d）"""
    doc = {'u': url, 'p': pins, 'r': root_pub.split()[1], 'k': secret, 'n': name}
    if exp is not None:
        doc['e'] = exp
    if device:
        doc['d'] = device
    raw = json.dumps(doc, separators=(',', ':'), ensure_ascii=False).encode()
    head = TEAM_LINK + base64.urlsafe_b64encode(raw).decode().rstrip('=')
    return head + '.' + hashlib.sha256(head.encode()).hexdigest()[:4]


def invite_decode(link):
    """读邀请链接（中间的空白不算）：{"u", "p", "r", "k", "n"[, "e"][, "d"]}；不对是 ValueError"""
    s = ''.join(link.split())
    head, _, check = s.rpartition('.')
    if not head.startswith(TEAM_LINK) or hashlib.sha256(head.encode()).hexdigest()[:4] != check:
        raise ValueError('不是邀请链接，或者抄错了')
    body = head[len(TEAM_LINK):]
    return json.loads(base64.urlsafe_b64decode(body + '=' * (-len(body) % 4)))


def team_doc_seal(keys, tid, dev, seq, doc):
    """一台设备的配置：明文是紧凑的 JSON（UTF-8），空格补齐到 1024 字节的整数倍；认证带着 Team 的编号、设备的编号、序号"""
    pt = json.dumps(doc, separators=(',', ':'), ensure_ascii=False).encode()
    return seal(TEAM_DOC, keys, [tid, dev, str(seq)], pt + b' ' * (-len(pt) % 1024))


def team_doc_open(keys, tid, dev, seq, ct):
    """认证过的配置；认证不过（别的设备的、别的序号的、改过的）是 None"""
    pt = unseal(TEAM_DOC, keys, [tid, dev, str(seq)], ct)
    return None if pt is None else json.loads(pt)


def team_doc_signed(tid, dev, seq, ct):
    """根密钥签的那一段字节：标签、Team 的编号、设备的编号、序号各跟一个 \0，接着 nonce、密文、认证码（解出来的字节）"""
    return ''.join(x + '\0' for x in (TEAM_DOC, tid, dev, str(seq))).encode() + b''.join(base64.b64decode(ct[k]) for k in 'nct')


def team_doc_sign(rootkey, tid, dev, seq, ct):
    """根密钥（私钥文件）签名，命名空间 cca-team；传的是 armored 签名的 base64"""
    sig = subprocess.run(['ssh-keygen', '-Y', 'sign', '-q', '-n', 'cca-team', '-f', rootkey], input=team_doc_signed(tid, dev, seq, ct),
                         stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=True).stdout
    return base64.b64encode(sig).decode()


def team_doc_verify(root_pub, tid, dev, seq, ct, sig):
    """是不是这把根公钥签的（ssh-keygen -Y verify；allowed_signers 里只有它）"""
    with tempfile.TemporaryDirectory() as d:
        with open(d + '/signers', 'w') as f:
            f.write('cca-team %s\n' % ' '.join(root_pub.split()[:2]))
        with open(d + '/sig', 'wb') as f:
            f.write(base64.b64decode(sig))
        r = subprocess.run(['ssh-keygen', '-Y', 'verify', '-f', d + '/signers', '-I', 'cca-team', '-n', 'cca-team', '-s', d + '/sig'],
                           input=team_doc_signed(tid, dev, seq, ct), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        return r.returncode == 0


def post(keyfile, port, op, extra):
    """用 keyfile（设备在 Team 里的密钥，或者管理员的根密钥）签名，向 127.0.0.1:port 的测试服务器发 op 请求（extra 是 put / get 这些）；限流（429）时等一会儿再试。
    回 (状态码, JSON)"""
    body = dict(extra, v=1, op=op, key=fp_of(keyfile + '.pub'), ts=time.time(), nonce=secrets.token_hex(16))
    raw = json.dumps(body, separators=(',', ':')).encode()
    sig = subprocess.run(['ssh-keygen', '-Y', 'sign', '-q', '-n', 'cca-presence', '-f', keyfile], input=raw,
                         stdout=subprocess.PIPE, check=True).stdout
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    ctx.check_hostname, ctx.verify_mode = False, ssl.CERT_NONE
    for _ in range(6):   # 每台设备每 2 秒一个请求，攒得下 30 个
        conn = http.client.HTTPSConnection('127.0.0.1', port, context=ctx, timeout=10)
        conn.request('POST', '/v1/' + op, body=raw, headers={'Content-Type': 'application/json', 'X-Cca-Sig': base64.b64encode(sig).decode()})
        r = conn.getresponse()
        j = json.loads(r.read() or b'{}')
        if r.status != 429:
            return r.status, j
        time.sleep(2)
    return r.status, j
