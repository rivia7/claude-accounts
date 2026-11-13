"""照 server/README.md 写的格式独立实现一遍（不用 cca 的代码），核对 cca 封的、解的：记下的 API key、折合金额两种密文，
和设备用 deploy key 签名的请求。tests/presence.py、tests/cost_view.py、regress 的 dkey / dcost（扮演另一台设备 dan-air）共用"""
import base64
import hashlib
import hmac
import http.client
import json
import secrets
import ssl
import subprocess
import time
import zlib

KEYSHARE, COST = 'cca-keyshare-v1', 'cca-cost-v1'


def derive(label, salt, tool, email, ident):
    """两把 HMAC 密钥（加密、认证）：由映射的盐（十六进制）、工具、邮箱（小写）和登录身份派生"""
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
    """deploy key 的指纹 SHA256:…"""
    with open(pubfile) as f:
        pub = f.read().split()[1]
    return 'SHA256:' + base64.b64encode(hashlib.sha256(base64.b64decode(pub)).digest()).decode().rstrip('=')


def post(keyfile, port, op, extra):
    """用 deploy key（keyfile）签名，向 127.0.0.1:port 的测试服务器发 op 请求（extra 是 put / get 这些）；限流（429）时等一会儿再试。
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
