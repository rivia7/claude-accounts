#!/usr/bin/env python3
"""cca 的设备在线服务（presence）：各台 Mac 报告哪些映射里的账号正经 cca 在用，账号页据此显示连着哪些设备。

只用标准库（Python 3.6 以上），不调外部命令：签名是 ssh-keygen -Y sign 的 SSHSIG 格式（Ed25519），这里照 RFC 8032
直接验，所以容器用官方的 python 镜像就行，不用另装 OpenSSH。部署、更新：./policy.sh presence deploy（docker compose）。

- 设备：每台 Mac 拉映射用的 deploy key（指纹 SHA256:…）。服务器只认维护者用发布密钥签过的设备清单（命名空间
  cca-devices，POST /v1/machines，序号只增不减），名字是 GitHub 上 deploy key 的标题。
- 请求：body 是紧凑 JSON，带 op、key（设备指纹）、ts、nonce，设备用 deploy key 签（命名空间 cca-presence），
  签名放在 X-Cca-Sig 头里（armored 签名的 base64）。ts 和服务器的时间差 5 分钟以内、nonce 不能重复。
- 账号：HMAC(盐, 工具\\0邮箱) 的前 32 位十六进制，盐在签名的映射里，服务器看不到邮箱。
- 上报（report）是这台设备现在开着的全部会话（快照，不是开始 / 结束事件）：命令行会话 [开了多久, 闲置多久]，
  桌面 App 同样一对；end 是 "none"（什么都没开了，之后不再心跳）或 "down"（上报进程被停掉）。
- 状态在读的时候按存下的数据和现在的时间算，没有清理线程：开着会话却 ttl 秒没心跳、或者报过 end=down → offline；
  没开会话 → none；有会话 idle 秒内有动静 → in_use，否则 idle。只有相对秒数上线，两边的时钟对不对不影响状态。
- API key（keys）：映射里官方账号记下备查的 API key，经这里同步给登录了这个账号的其他设备。服务器只存设备传上来的密文
  （每个账号一份，新的整个换掉旧的），看不到 key，也分不出是 key 还是“删掉了”：密钥由盐、邮箱和账号的登录身份派生，只有
  登录过这个账号的设备算得出来；谁新谁旧、真的假的都由设备自己按密文里认证过的时间判断。存在状态文件旁边的 keys.json。
- 折合金额（op ledger）：映射里官方账号的用量按官方 API 标价折成的金额，各台设备算自己的，经这里交换。服务器只存设备传上来的密文
  （每个账号、每台设备一份，设备只能写自己的那一份；撤销设备后不删，是账号的用量，cost_keep 天没再传就删掉），密钥和 API key
  一样只有登录过这个账号的设备算得出来。回的时候顺带说哪些设备 30 天内用过这个账号、却没传过（多半是 cca 太旧）。存在 cost-slots.json。
  早先试过的 /v1/cost 是另一种格式，这一版不认（回 404：那时的客户端照旧版服务器处理，过几个小时再试）。
- 连接：同一个 IP 最多同时 8 个，每个连接从接进来起最多 15 秒（TLS 握手 5 秒），慢吞吞、不说话的连接占不满服务器；
  POST 按 IP 限流，验签通过之后再按设备限流（别的设备拿你的指纹乱发请求，扣不到你的配额）。
- 证书公钥的指纹（映射里的 presence.pins）在服务器上用 openssl 算：./policy.sh presence deploy 打印、写进映射。

自己的同步服务器（--mode own，用户自己部署：cca hub）：上面这些照旧，只是设备不来自维护者签名的设备清单（没有 /v1/machines），
而是拿着加入链接自己加入（op join：链接里的入伙口令，服务器只存它的 SHA-256；盐的校验值，第一台设备加入时登记，拿着另一份盐的
加入不了），最多 MAX_MEMBERS 台；成员能列出（members）、移除（remove）设备，移除自己就是退出。设备不在名单里回
403 unknown_device（被移除了、服务器的记录清掉了：设备拿着链接重新加入），签名不对回 403 denied。启动时配置的入伙口令的哈希
和上次的不一样（用新的加入链接重新部署了）就从头开始：以前的设备、记录、密文都不要了。格式见 README.md「自己的同步服务器」。
"""
import argparse
import base64
import hashlib
import hmac
import http.server
import json
import os
import re
import selectors
import socket
import socketserver
import ssl
import sys
import threading
import time

API = 1
NS_REPORT = 'cca-presence'
NS_MACHINES = 'cca-devices'
MAX_BODY = 64 * 1024
MAX_SIG = 4096
TS_WINDOW = 300            # 请求里的时间和服务器差多少秒以内
MAX_ACCOUNTS = 64          # 一次上报 / 查询最多几个账号
MAX_UNITS = 64             # 一台设备上一个账号最多几个会话
MAX_MACHINES = 1000
MAX_CONN = 64              # 同时处理的连接
MAX_CONN_IP = 8            # 同一个 IP 同时几个连接
CONN_DEADLINE = 15         # 一个连接从接进来起最多多少秒（到点就断）
HANDSHAKE_TIMEOUT = 5
MAX_AGE = 10 * 365 * 86400
MAX_SYNC_BODY = 1 << 20    # 交换密文的请求（/v1/keys、/v1/ledger）一次最多这么大
MAX_KEYS = 4096            # API key 的密文最多存几个账号
MAX_KEY_BLOB = 8192        # 一份密文（base64）最长多少个字符（key 最长 4096 个字符，补齐、加上时间后不到 6000）
MAX_COST_BLOB = 98304      # 一份密文（base64）最长多少个字符（一台设备一个账号 14 周的小时数据，压缩后一般几十 KB）
COST_SLOTS = 128           # 一台设备最多存几个账号的
COST_DEVICE_BYTES = 4 << 20    # 一台设备存的密文加起来最多这么大
COST_TOTAL_BYTES = 32 << 20    # 所有设备加起来最多这么大
COST_RESP = 512 * 1024     # 一次回的密文最多这么多：多了的只带认证码不带密文，设备下次再取
SYNC_OPS = {'keys': MAX_KEY_BLOB, 'ledger': MAX_COST_BLOB}   # 交换密文的 op → 一份密文最长多少个字符

FP_RE = re.compile(r'^SHA256:[A-Za-z0-9+/]{43}$')
AID_RE = re.compile(r'^[0-9a-f]{32}$')
NONCE_RE = re.compile(r'^[0-9a-f]{32}$')
KEY_RE = re.compile(r'^ssh-ed25519 (AAAAC3NzaC1lZDI1NTE5AAAAI[A-Za-z0-9+/]{43})$')
NAME_BAD = re.compile(r'[\x00-\x1f\x7f]')
B64_RE = re.compile(r'^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$')
TAG_RE = re.compile(r'^[A-Za-z0-9+/]{43}=$')   # 认证码（32 字节的 base64）
HASH_RE = re.compile(r'^[0-9a-f]{64}$')      # 入伙口令的 SHA-256（--admit-hash）
ADMIT_RE = re.compile(r'^[0-9a-f]{32}$')     # 入伙口令
CHECK_RE = re.compile(r'^[0-9a-f]{32}$')     # 盐的校验值：HMAC(盐, "cca-hub-check-v1") 的前 16 个字节
MAX_MEMBERS = 64           # 自己的服务器最多几台设备
OWN_OPS = ('join', 'members', 'remove')     # 只有自己的服务器有的 op

VERIFY_SLOTS = threading.BoundedSemaphore(4)   # 同时验几个签名（一个十几毫秒 CPU）


def key_fp(b64):
    """公钥（base64 那一段）的指纹，和 ssh-keygen -l 的写法一样"""
    return 'SHA256:' + base64.b64encode(hashlib.sha256(base64.b64decode(b64)).digest()).decode('ascii').rstrip('=')


# --- Ed25519 验签（RFC 8032 第 5.1.7 节，照第 6 节的参考实现；只验不签，不碰私钥）-----------------------------
EP = 2 ** 255 - 19
EL = 2 ** 252 + 27742317777372353535851937790883648493   # 基点的阶
ED = -121665 * pow(121666, EP - 2, EP) % EP
EI = pow(2, (EP - 1) // 4, EP)                              # sqrt(-1)


def ed_x(y, sign):
    """曲线上纵坐标是 y 的点的横坐标（sign 是最低位）；不在曲线上是 None"""
    if y >= EP:
        return None
    x2 = (y * y - 1) * pow(ED * y * y + 1, EP - 2, EP) % EP
    if x2 == 0:
        return None if sign else 0
    x = pow(x2, (EP + 3) // 8, EP)
    if (x * x - x2) % EP:
        x = x * EI % EP
    if (x * x - x2) % EP:
        return None
    if (x & 1) != sign:
        x = EP - x
    return x


def ed_add(p, q):   # 扩展坐标 (X, Y, Z, T) 的加法
    a = (p[1] - p[0]) * (q[1] - q[0]) % EP
    b = (p[1] + p[0]) * (q[1] + q[0]) % EP
    c = 2 * p[3] * q[3] * ED % EP
    d = 2 * p[2] * q[2] % EP
    e, f, g, h = b - a, d - c, d + c, b + a
    return e * f % EP, g * h % EP, f * g % EP, e * h % EP


def ed_mul(s, p):
    q = (0, 1, 1, 0)
    while s:
        if s & 1:
            q = ed_add(q, p)
        p = ed_add(p, p)
        s >>= 1
    return q


def ed_point(b):
    """32 字节的压缩点；不是合法的点是 None"""
    if len(b) != 32:
        return None
    y = int.from_bytes(b, 'little')
    x = ed_x(y & ((1 << 255) - 1), y >> 255)
    return None if x is None else (x, y & ((1 << 255) - 1), 1, x * (y & ((1 << 255) - 1)) % EP)


EG_Y = 4 * pow(5, EP - 2, EP) % EP
EG = (ed_x(EG_Y, 0), EG_Y, 1, ed_x(EG_Y, 0) * EG_Y % EP)


def ed25519_verify(pub, msg, sig):
    if len(pub) != 32 or len(sig) != 64:
        return False
    a, r = ed_point(pub), ed_point(sig[:32])
    s = int.from_bytes(sig[32:], 'little')
    if a is None or r is None or s >= EL:
        return False
    h = int.from_bytes(hashlib.sha512(sig[:32] + pub + msg).digest(), 'little') % EL
    lhs, rhs = ed_mul(s, EG), ed_add(r, ed_mul(h, a))
    return (lhs[0] * rhs[2] - rhs[0] * lhs[2]) % EP == 0 and (lhs[1] * rhs[2] - rhs[1] * lhs[2]) % EP == 0


def ssh_string(b, i):
    """SSH 线上格式的 string（4 字节长度 + 内容）：(内容, 下一个位置)；越界抛 ValueError"""
    if i + 4 > len(b):
        raise ValueError('short')
    n = int.from_bytes(b[i:i + 4], 'big')
    if i + 4 + n > len(b):
        raise ValueError('short')
    return b[i + 4:i + 4 + n], i + 4 + n


def ssh_verify(pubkey, namespace, data, sig):
    """sig 是 ssh-keygen -Y sign 的 armored 输出（SSHSIG，见 OpenSSH 的 PROTOCOL.sshsig）；pubkey 是 'ssh-ed25519 AAAA…'。
    签名里的公钥必须就是这把，命名空间必须一样"""
    try:
        text = sig.decode('ascii').strip().splitlines()
        if len(text) < 3 or text[0] != '-----BEGIN SSH SIGNATURE-----' or text[-1] != '-----END SSH SIGNATURE-----':
            return False
        blob = base64.b64decode(''.join(text[1:-1]), validate=True)
        if blob[:6] != b'SSHSIG' or blob[6:10] != b'\x00\x00\x00\x01':
            return False
        i = 10
        pk, i = ssh_string(blob, i)
        ns, i = ssh_string(blob, i)
        reserved, i = ssh_string(blob, i)
        halg, i = ssh_string(blob, i)
        sb, i = ssh_string(blob, i)
        if i != len(blob) or pk != base64.b64decode(pubkey.split()[1]) or ns != namespace.encode() or halg not in (b'sha512', b'sha256'):
            return False
        kt, j = ssh_string(pk, 0)
        a, j2 = ssh_string(pk, j)
        st, k = ssh_string(sb, 0)
        raw, k2 = ssh_string(sb, k)
        if kt != b'ssh-ed25519' or st != b'ssh-ed25519' or j2 != len(pk) or k2 != len(sb):
            return False
        pre = lambda x: len(x).to_bytes(4, 'big') + x
        signed = b'SSHSIG' + pre(ns) + pre(reserved) + pre(halg) + pre(hashlib.new(halg.decode(), data).digest())
        with VERIFY_SLOTS:
            return ed25519_verify(a, signed, raw)
    except (ValueError, IndexError, UnicodeDecodeError):
        return False


def self_test():
    """验签自己验一遍（RFC 8032 的测试向量 1，和改了一个字节的）：结果在 /v1/health 的 sig_ok"""
    pub = bytes.fromhex('d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a')
    sig = bytes.fromhex('e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b')
    return ed25519_verify(pub, b'', sig) and not ed25519_verify(pub, b'x', sig) and not ed25519_verify(pub, b'', sig[:63] + b'\x0c')


def num_ok(x):
    return isinstance(x, (int, float)) and not isinstance(x, bool) and 0 <= x <= MAX_AGE


def is_time(x):
    """服务器记的时间（时间戳）"""
    return isinstance(x, (int, float)) and not isinstance(x, bool) and x > 0


def pair(p):
    """[开了多久, 闲置多久]（秒）；不合格是 None"""
    if isinstance(p, list) and len(p) == 2 and num_ok(p[0]) and num_ok(p[1]):
        return [float(p[0]), float(p[1])]
    return None


def parse_units(u):
    """report 的 units：{账号摘要: {"cli": [[开了多久, 闲置多久], …], "app": [开了多久, 闲置多久] | null}}"""
    if not isinstance(u, dict) or len(u) > MAX_ACCOUNTS:
        return None
    out = {}
    for aid, x in u.items():
        if not (isinstance(aid, str) and AID_RE.match(aid) and isinstance(x, dict)):
            return None
        cli = x.get('cli') or []
        if not isinstance(cli, list) or len(cli) > MAX_UNITS:
            return None
        pairs = [pair(p) for p in cli]
        app = x.get('app')
        appp = pair(app) if app is not None else None
        if None in pairs or (app is not None and appp is None):
            return None
        out[aid] = {'cli': pairs, 'app': appp}
    return out


def parse_blob(b, limit=MAX_KEY_BLOB):
    """keys、cost 里设备传上来的一份密文：{"n": nonce, "c": 密文, "t": 校验}，都是 base64（服务器不懂里面是什么，只查形状和长度）"""
    if not (isinstance(b, dict) and set(b) == {'n', 'c', 't'} and all(isinstance(b[k], str) and B64_RE.match(b[k]) for k in b)):
        return None
    if len(b['n']) != 24 or len(b['t']) != 44 or not 0 < len(b['c']) <= limit:
        return None
    return {'n': b['n'], 'c': b['c'], 't': b['t']}


def parse_sync(j, limit):
    """keys、ledger 的 put（{账号摘要: 密文}）、get（[账号摘要…]）和 have（{账号摘要: {设备指纹: 认证码}}：设备手里已经有的，
    没变的不回密文；只有 ledger 用）；不合格是 None"""
    put, get, have = j.get('put') or {}, j.get('get') or [], j.get('have') or {}
    if not (isinstance(put, dict) and isinstance(get, list) and isinstance(have, dict) and max(len(put), len(get), len(have)) <= MAX_ACCOUNTS):
        return None
    if not all(isinstance(a, str) and AID_RE.match(a) for a in list(put) + get + list(have)):
        return None
    if not all(isinstance(h, dict) and len(h) <= 256 and all(isinstance(f, str) and FP_RE.match(f) and isinstance(t, str) and TAG_RE.match(t)
                                                            for f, t in h.items()) for h in have.values()):
        return None
    blobs = {aid: parse_blob(b, limit) for aid, b in put.items()}
    if None in blobs.values():
        return None
    return blobs, list(dict.fromkeys(get)), have


class Buckets:
    """令牌桶限流：每个键每秒 rate 个，最多攒 burst 个"""
    def __init__(self, rate, burst):
        self.rate, self.burst, self.b, self.lock = rate, burst, {}, threading.Lock()

    def take(self, key):
        now = time.monotonic()
        with self.lock:
            if len(self.b) > 4096:
                self.b = {k: v for k, v in self.b.items() if now - v[1] < 600}
            tokens, last = self.b.get(key, (self.burst, now))
            tokens = min(self.burst, tokens + (now - last) * self.rate)
            if tokens < 1:
                self.b[key] = (tokens, now)
                return False
            self.b[key] = (tokens - 1, now)
            return True


class Nonces:
    """用过的 nonce（只在内存里；重启后靠 ts 的 5 分钟窗口挡重放）"""
    def __init__(self):
        self.seen, self.lock = {}, threading.Lock()

    def add(self, nonce, now):
        with self.lock:
            if len(self.seen) > 20000:
                self.seen = {k: t for k, t in self.seen.items() if t > now}
            if self.seen.get(nonce, 0) > now:
                return False
            self.seen[nonce] = now + 2 * TS_WINDOW + 60
            return True


def sessions(e):
    """一台设备上一个账号开着的会话：命令行会话加桌面 App，每个是 [开始, 最后动静]（服务器时间）"""
    return list(e.get('cli') or []) + ([e['app']] if e.get('app') else [])


def last_used(e):
    """一台设备上一个账号最后一次有动静：记下的上次使用和开着的会话里最晚的"""
    return max([e.get('used') or 0] + [x[1] for x in sessions(e)])


def ago(now, t):
    """t 以来多少秒（取整，不为负）"""
    return round(max(now - t, 0))


def load_doc(path, keys):
    """读状态文件：没有、读不了、格式不对是 None（读不了的在日志里说一声）"""
    try:
        with open(path, encoding='utf-8') as f:
            d = json.load(f)
        if isinstance(d, dict) and all(isinstance(d.get(k), dict) for k in keys):
            return d
    except FileNotFoundError:
        return None
    except (OSError, ValueError) as e:
        sys.stderr.write('cca-presence: %s 读不了（%s），从空的开始\n' % (path, e))
    return None


def save_doc(path, data):
    tmp = '%s.%d.tmp' % (path, os.getpid())
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, 'w', encoding='utf-8') as f:
        json.dump(data, f, ensure_ascii=False, separators=(',', ':'))
    os.replace(tmp, path)


class Store:
    """state.json：{"seq": 设备清单的序号, "machines": {指纹: {"name", "key"}}, "beats": {指纹: {"seen", "end"}},
    "units": {指纹: {账号摘要: {"cli": [[开始, 最后动静]…], "app": [开始, 最后动静] | null, "used": 上次使用}}}}，时间都是服务器的。
    自己的服务器（mode own）：machines 的每一台多一项 "joined"（什么时候加入的），另有 "own": {"seed": 启动时配置的入伙口令的哈希,
    "admit": 现在认的入伙口令的哈希, "check": 登记的盐的校验值 | null}。
    keys.json（另一个文件：心跳不用跟着重写它）：{"acc": {账号摘要: {"blob": 密文, "fp": 传的设备, "name": 它那时的名字,
    "at": 什么时候传的}}}（撤销设备不删：是账号的）。cost-slots.json：{"acc": {账号摘要: {设备指纹: {"blob", "name", "at"}}}}
    （撤销设备不删，cost_keep 秒没再传的才删）"""
    def __init__(self, path, ttl, idle, retain, keys_path=None, cost_path=None, cost_keep=100 * 86400, mode='maintainer', admit_hash=None):
        self.path, self.ttl, self.idle, self.retain, self.cost_keep, self.mode = path, ttl, idle, retain, cost_keep, mode
        self.seen = {}   # 自己的服务器：各台设备最近一次验过签的请求（只在内存里：不为它每个请求都重写状态文件）
        self.keys_path = keys_path or os.path.join(os.path.dirname(path) or '.', 'keys.json')
        self.cost_path = cost_path or os.path.join(os.path.dirname(path) or '.', 'cost-slots.json')
        self.lock = threading.Lock()
        self.data = {'v': 1, 'seq': 0, 'machines': {}, 'beats': {}, 'units': {}}
        self.data.update(load_doc(path, ('machines', 'beats', 'units')) or {})
        self.keys = {'v': 1, 'acc': {}}
        self.keys.update(load_doc(self.keys_path, ('acc',)) or {})
        self.cost = {'v': 1, 'acc': {}}
        self.cost.update(load_doc(self.cost_path, ('acc',)) or {})
        self.cost['acc'] = {aid: {f: e for f, e in slots.items() if isinstance(e, dict) and parse_blob(e.get('blob'), MAX_COST_BLOB)}
                            for aid, slots in self.cost['acc'].items() if isinstance(aid, str) and isinstance(slots, dict)}
        if mode == 'own':
            self.own_setup(admit_hash)
        elif 'own' in self.data:   # 以前是自己的服务器：那时加入的设备不是维护者清单里的，不认
            sys.stderr.write('cca-presence: 状态文件是自己的服务器（--mode own）的：%d 台设备不要了，等维护者同步设备清单\n' % len(self.data['machines']))
            self.data.pop('own')
            self.data.update({'seq': 0, 'machines': {}, 'beats': {}, 'units': {}})
            self.save()

    def own_setup(self, admit_hash):
        """自己的服务器：配置的入伙口令的哈希（seed）和上次的不一样（第一次启动、用新的加入链接重新部署了、以前是维护者的服务器）就从头
        开始：以前的设备、记录、密文都不要了（新的盐算出来的账号摘要和密钥都不一样，旧的谁也用不上）"""
        own = self.data.get('own')
        check = own.get('check') if isinstance(own, dict) else None
        if (isinstance(own, dict) and own.get('seed') == admit_hash and HASH_RE.match(own.get('admit') or '')
                and (check is None or (isinstance(check, str) and CHECK_RE.match(check)))):
            return
        if self.data['machines'] or self.keys['acc'] or self.cost['acc']:
            sys.stderr.write('cca-presence: 入伙口令换了（用新的加入链接重新部署了）：以前的 %d 台设备和存的密文都不要了\n' % len(self.data['machines']))
        self.data.update({'seq': 0, 'machines': {}, 'beats': {}, 'units': {}, 'own': {'seed': admit_hash, 'admit': admit_hash, 'check': None}})
        self.keys, self.cost = {'v': 1, 'acc': {}}, {'v': 1, 'acc': {}}
        self.save()
        save_doc(self.keys_path, self.keys)
        save_doc(self.cost_path, self.cost)
    def save(self):
        save_doc(self.path, self.data)

    def machine(self, fp):
        with self.lock:
            return self.data['machines'].get(fp) if isinstance(fp, str) else None

    def name(self, fp):
        """设备清单里这台设备的名字（没有是 None）"""
        m = self.machine(fp)
        return m['name'] if m else None

    def fps(self):
        with self.lock:
            return set(self.data['machines'])

    def health(self, sig_ok, relay=None):
        with self.lock:
            out = {'v': API, 'ok': True, 'sig_ok': sig_ok, 'machines': len(self.data['machines']), 'seq': self.data['seq'], 'mode': self.mode,
                   'ops': ['report', 'view'] + list(SYNC_OPS) + (list(OWN_OPS) if self.mode == 'own' else []) + (list(RELAY_OPS) if relay else [])}
            if relay:
                out['relay'] = {'port': relay}
            return out

    def set_machines(self, body, sig, release_key):
        if not ssh_verify(release_key, NS_MACHINES, body, sig):
            return 403, {'error': 'denied'}
        try:
            doc = json.loads(body.decode('utf-8'))
        except ValueError:
            return 400, {'error': 'bad'}
        if not isinstance(doc, dict):
            return 400, {'error': 'bad'}
        ms, seq = doc.get('machines'), doc.get('seq')
        if doc.get('version') != 1 or not isinstance(seq, int) or isinstance(seq, bool) or not isinstance(ms, list) or len(ms) > MAX_MACHINES:
            return 400, {'error': 'bad'}
        machines, names = {}, set()
        for m in ms:
            k = KEY_RE.match(m.get('key') or '') if isinstance(m, dict) and isinstance(m.get('key'), str) else None
            name = m.get('name') if isinstance(m, dict) else None
            if not k or not isinstance(name, str) or not name.strip() or len(name) > 64 or NAME_BAD.search(name):
                return 400, {'error': 'bad'}
            fp = key_fp(k.group(1))
            if fp in machines or name in names:
                return 400, {'error': 'bad'}
            machines[fp] = {'name': name, 'key': 'ssh-ed25519 ' + k.group(1)}
            names.add(name)
        with self.lock:
            if seq < self.data['seq'] or (seq == self.data['seq'] and machines != self.data['machines']):
                return 409, {'error': 'seq', 'seq': self.data['seq']}
            gone = set(self.data['machines']) - set(machines)
            self.data['machines'], self.data['seq'] = machines, seq
            for fp in gone:   # 撤销的设备：它的记录一起删掉
                self.data['beats'].pop(fp, None)
                self.data['units'].pop(fp, None)
            self.save()
        if gone or machines:
            sys.stderr.write('cca-presence: 设备清单 #%d，%d 台%s\n' % (seq, len(machines), '，撤销 %d 台' % len(gone) if gone else ''))
        return 200, {'v': API, 'ok': True, 'seq': seq, 'machines': len(machines)}

    def report(self, fp, units, end, now):
        with self.lock:
            self.data['beats'][fp] = {'seen': now, 'end': end}
            cur = self.data['units'].setdefault(fp, {})
            for aid in cur:
                if aid not in units:   # 这次没报的账号：会话都关了（上次使用照留）
                    cur[aid]['cli'], cur[aid]['app'] = [], None
            for aid, u in units.items():
                e = cur.setdefault(aid, {'cli': [], 'app': None, 'used': 0})
                e['cli'] = [[now - age, now - idle] for age, idle in u['cli']]
                e['app'] = [now - u['app'][0], now - u['app'][1]] if u['app'] else None
                e['used'] = last_used(e)
            self.prune(now)
            self.save()

    def prune(self, now):
        d, old = self.data, now - self.retain
        for fp in list(d['beats']):
            if fp not in d['machines'] or d['beats'][fp].get('seen', 0) < old:
                d['beats'].pop(fp)
                d['units'].pop(fp, None)
        for fp in list(d['units']):
            if fp not in d['machines'] or fp not in d['beats']:
                d['units'].pop(fp)
                continue
            for aid in list(d['units'][fp]):
                e = d['units'][fp][aid]
                if not sessions(e) and (e.get('used') or 0) < old:
                    d['units'][fp].pop(aid)

    def key_sync(self, fp, put, get, now):
        """API key 的密文：先存这台设备传的（整个换掉），再回问的和传的那些账号现在的（没有的不回）。账号太多时不收新的"""
        with self.lock:
            acc = self.keys['acc']
            if len(set(acc) | set(put)) > MAX_KEYS:
                return 507, {'error': 'full'}
            m = self.data['machines'].get(fp)
            if not m:   # 验签之后、拿到锁之前设备清单换了，这台被撤销了
                return 403, {'error': 'denied'}
            name = m['name']
            for aid, blob in put.items():
                acc[aid] = {'blob': blob, 'fp': fp, 'name': name, 'at': now}
            if put:
                save_doc(self.keys_path, self.keys)
            out = {}
            for aid in list(dict.fromkeys(get + list(put))):
                e = acc.get(aid)
                if isinstance(e, dict) and isinstance(e.get('blob'), dict):
                    out[aid] = {'blob': e['blob'], 'name': e.get('name') if isinstance(e.get('name'), str) else None,
                                'me': e.get('fp') == fp, 'age': ago(now, e.get('at') or 0)}
            return 200, {'v': API, 'ok': True, 'me_name': name, 'keys': out}

    def cost_sync(self, fp, put, get, have, now):
        """折合金额的密文：先存这台设备传的（每个账号一份、只写自己的那一份），再回问的那些账号各台设备的（have 里认证码一样的、
        这台设备自己的不带密文；撤销了的设备照样回，标上 revoked），和 30 天内经 cca 用过这个账号、却没传过的设备（missing）"""
        with self.lock:
            m = self.data['machines'].get(fp)
            if not m:
                return 403, {'error': 'denied'}
            acc = self.cost['acc']
            if put:
                self.cost_prune(now)   # 先清掉很久没再传的：不然存满了以后谁都传不上来，也就一直清不掉
                size = lambda e: len(e['blob']['c'])
                mine = {aid: size(e[fp]) for aid, e in acc.items() if fp in e}
                mine.update({aid: len(b['c']) for aid, b in put.items()})
                total = sum(size(x) for aid, e in acc.items() for f, x in e.items() if not (f == fp and aid in put))
                if len(mine) > COST_SLOTS or sum(mine.values()) > COST_DEVICE_BYTES or total + sum(len(b['c']) for b in put.values()) > COST_TOTAL_BYTES:
                    return 507, {'error': 'full'}
                for aid, blob in put.items():
                    acc.setdefault(aid, {})[fp] = {'blob': blob, 'name': m['name'], 'at': now}
                save_doc(self.cost_path, self.cost)
            out, missing, budget = {}, {}, COST_RESP
            for aid in get:
                rows = []
                for f, e in sorted((acc.get(aid) or {}).items(), key=lambda x: -(x[1].get('at') or 0)):   # 最近传过的先给密文
                    cur = self.data['machines'].get(f)
                    name = cur['name'] if cur else e.get('name')
                    r = {'fp': f, 'name': name if isinstance(name, str) else None, 'me': f == fp, 'revoked': cur is None,
                         'tag': e['blob']['t'], 'age': ago(now, e.get('at') or 0)}
                    if f != fp and (have.get(aid) or {}).get(f) != r['tag'] and len(e['blob']['c']) <= budget:
                        r['blob'] = e['blob']
                        budget -= len(e['blob']['c'])
                    rows.append(r)
                out[aid] = rows
                missing[aid] = self.cost_missing(aid, acc.get(aid) or {}, now, fp)
            return 200, {'v': API, 'ok': True, 'me_name': m['name'], 'cost': out, 'missing': missing}

    def cost_missing(self, aid, slots, now, me):
        """retain 秒内经 cca 用过这个账号（上报的记录）、却从来没传过折合金额的设备；问的这台（me）不算：它先问、接着就传"""
        out = []
        for f, accts in self.data['units'].items():
            e = accts.get(aid)
            if f == me or f in slots or f not in self.data['machines'] or not isinstance(e, dict):
                continue
            used = last_used(e)
            if used and now - used < self.retain:
                out.append({'name': self.data['machines'][f]['name'], 'used_age': ago(now, used)})
        return out

    def cost_prune(self, now):
        """cost_keep 秒没再传过的不要了（那台设备早就不用 cca 了）"""
        acc = self.cost['acc']
        for aid in list(acc):
            for f in [f for f, e in acc[aid].items() if now - (e.get('at') or 0) > self.cost_keep]:
                del acc[aid][f]
            if not acc[aid]:
                del acc[aid]

    def admit_ok(self, admit):
        """自己的服务器：入伙口令对不对（只比哈希，用不随内容变的时间比）"""
        if not (isinstance(admit, str) and ADMIT_RE.match(admit)):
            return False
        with self.lock:
            want = self.data['own']['admit']
        return hmac.compare_digest(hashlib.sha256(admit.encode('ascii')).hexdigest(), want)

    def free_name(self, name, fp):
        """名字被别的设备用了就在后面加 -2、-3……（最长 64 个字符）。拿着锁调"""
        taken = {m['name'] for f, m in self.data['machines'].items() if f != fp}
        for i in range(1, MAX_MEMBERS + 2):
            tail = '' if i == 1 else '-%d' % i
            cand = name[:64 - len(tail)] + tail
            if cand not in taken:
                return cand
        return name

    def join(self, fp, key, name, check, now):
        """自己的服务器：加入（入伙口令、签名在 Handler 里验过）。盐的校验值第一台设备登记，以后的要一样；同一把密钥再加入是改名字"""
        with self.lock:
            own, ms = self.data['own'], self.data['machines']
            if own['check'] is not None and not hmac.compare_digest(own['check'], check):
                return 409, {'error': 'salt_mismatch'}
            if fp not in ms and len(ms) >= MAX_MEMBERS:
                return 507, {'error': 'full'}
            own['check'] = own['check'] or check
            old = ms.get(fp) or {}
            name = self.free_name(name, fp)
            ms[fp] = {'name': name, 'key': key, 'joined': old['joined'] if is_time(old.get('joined')) else now}
            self.seen[fp] = now
            self.save()
            if not old:
                sys.stderr.write('cca-presence: 加入了一台设备，现在 %d 台\n' % len(ms))
            return 200, {'v': API, 'ok': True, 'name': name, 'members': len(ms)}

    def members(self, me, now):
        """自己的服务器：现在有哪些设备（最近一次请求：内存里记的、上报的心跳，取新的）"""
        with self.lock:
            rows = []
            for f, m in self.data['machines'].items():
                seen = max(self.seen.get(f) or 0, (self.data['beats'].get(f) or {}).get('seen') or 0)
                rows.append({'fp': f, 'name': m['name'], 'me': f == me, 'joined_age': ago(now, m['joined']) if is_time(m.get('joined')) else None,
                             'seen_age': ago(now, seen) if seen else None})
            return 200, {'v': API, 'ok': True, 'me_name': self.data['machines'][me]['name'] if me in self.data['machines'] else None,
                         'members': rows}

    def remove(self, target):
        """自己的服务器：移除一台设备（它的密钥就此不认；是发请求的设备自己就是退出）。它的上报记录跟着删掉，传过的密文是账号的，
        不删（折合金额里标成撤销了）"""
        with self.lock:
            if target not in self.data['machines']:
                return 404, {'error': 'no_device'}
            del self.data['machines'][target]
            self.data['beats'].pop(target, None)
            self.data['units'].pop(target, None)
            self.seen.pop(target, None)
            self.save()
            n = len(self.data['machines'])
        sys.stderr.write('cca-presence: 移除了一台设备，现在 %d 台\n' % n)
        return 200, {'v': API, 'ok': True, 'members': n}

    def row(self, fp, e, now):
        beat = self.data['beats'].get(fp) or {}
        seen, end = beat.get('seen') or 0, beat.get('end')
        units = sessions(e)
        age = lambda t: ago(now, t) if t else None
        last = max((u[1] for u in units), default=0)   # 开着的会话最后一次有动静
        if units and (end == 'down' or now - seen > self.ttl):
            state = 'offline'
        elif not units:
            state = 'none'
        else:
            state = 'in_use' if now - last < self.idle else 'idle'
        return {'fp': fp, 'name': self.data['machines'][fp]['name'], 'state': state, 'cli': len(e.get('cli') or []),
                'app': bool(e.get('app')), 'since_age': age(min((u[0] for u in units), default=0)), 'idle_age': age(last),
                'used_age': age(e.get('used')), 'seen_age': age(seen)}

    def view(self, me, aids, now):
        with self.lock:
            out = {}
            for aid in aids:
                rows = [self.row(fp, accts[aid], now) for fp, accts in self.data['units'].items()
                        if aid in accts and fp in self.data['machines']]
                out[aid] = [r for r in rows if r['state'] != 'none' or (r['used_age'] or 0) < self.retain]
            return {'v': API, 'me_name': (self.data['machines'].get(me) or {}).get('name'),
                    'ttl': self.ttl, 'idle': self.idle, 'accounts': out}


# ---------------------------------------------------------------- 远程会话的中转（--relay-listen 才开；格式见 README.md）
RELAY_OPS = ('pair_open', 'pair_put', 'pair_get', 'pair_commit', 'pair_close', 'pair_drop', 'relay_host', 'relay_attach', 'relay_list')
PAIR_TTL = 600             # 配对码多久过期
PAIR_SLOTS = 4             # 一台设备同时开着几个配对
PAIR_WAIT = 10             # pair_get 最多等对方多久（要比 CONN_DEADLINE 短）
PAIR_WAITERS = threading.BoundedSemaphore(16)   # 同时等着的 pair_get（多了的不等、马上回）
PAIR_MSG = 2048            # 配对时交换的一份密文（base64）最长多少个字符
EDGES_DEV = 16             # 一台设备最多和几台配对
EDGES_MAX = 4096
EDGE_IDLE = 180 * 86400    # 这么久没用过的配对删掉
TICKET_TTL = 30            # 票据多久过期（只能用一次）
TICKETS_DEV = 16           # 一台设备手里没用掉的票据
SESSIONS_DEV = 8           # 一台设备同时开着几个可以接入的会话
VIEWERS = 4                # 一个会话同时几处接入
DIR_BLOB = 4096            # 会话的说明（密文，base64）最长多少个字符
RELAY_CONN = 256           # 数据端口同时几个连接
RELAY_CONN_IP = 32
RELAY_QUEUE = 256 * 1024   # 一头还没转出去的超过这么多，就先不从另一头读
RELAY_IDLE = 90            # 这么久一个字节都没有的连接断开（两头各自每 20 秒会说一声）
RELAY_GRACE = 60           # 会话的控制连接断了以后，会话留这么久等它重连
PREAMBLE_WAIT = 5          # 连上以后这么久还没说完开场白就断开
JOIN_WAIT = 10             # 接入方等宿主那一头连上来最多这么久
CTL_FRAME = 32 * 1024      # 控制连接上一帧最长
F_JOIN, F_PING, F_PONG, F_ANNOUNCE, F_BYE = 0x02, 0x06, 0x07, 0x08, 0x09
RELAY_LINE = re.compile(rb'^CCA-RELAY/1 (host|view|join) ([0-9a-f]{32}) ([0-9a-f]{16}) ([A-Za-z0-9_-]{43})$')


def b64u(b):
    return base64.urlsafe_b64encode(b).rstrip(b'=')


def hmac256(key, msg):
    return hmac.new(key, msg, hashlib.sha256).digest()


class Remote:
    """远程会话的中转记着的：
    - edges（remote.json，撤销设备、180 天没用过才删）：{配对编号: {"a": 显示配对码的设备, "b": 输入配对码的设备, "at", "used"}}。
      配对编号是两台设备从配对出来的密钥算的，服务器只知道是哪两台设备配了对
    - 下面的只在内存里：slots（正在进行的配对：两边各放一份密文、各报一次配对编号）、tickets（进数据端口的票据）、
      sessions（开着的会话：哪台设备的、给哪几个配对看、每个配对一份说明的密文、控制连接）"""
    def __init__(self, path, port):
        self.path, self.port = path, port
        self.lock = threading.RLock()
        self.cond = threading.Condition(self.lock)
        self.doc = {'v': 1, 'edges': {}}
        self.doc.update(load_doc(path, ('edges',)) or {})
        self.doc['edges'] = {p: e for p, e in self.doc['edges'].items()
                             if isinstance(p, str) and AID_RE.match(p) and isinstance(e, dict)
                             and all(isinstance(e.get(k), str) and FP_RE.match(e[k]) for k in ('a', 'b'))}
        self.slots, self.tickets, self.sessions = {}, {}, {}

    @property
    def edges(self):
        return self.doc['edges']

    def save(self):
        save_doc(self.path, self.doc)

    def member(self, pid, fp):
        e = self.edges.get(pid) if isinstance(pid, str) else None
        return bool(e) and fp in (e['a'], e['b'])

    def other(self, pid, fp):
        e = self.edges[pid]
        return e['b'] if e['a'] == fp else e['a']

    def keep_only(self, fps):
        """设备清单换了：撤销的设备的配对、票据、会话都删掉（它的控制连接由数据端口那边发现会话没了时断开）"""
        with self.lock:
            gone = [p for p, e in self.edges.items() if e['a'] not in fps or e['b'] not in fps]
            for p in gone:
                self.drop_edge(p)
            self.tickets = {t: k for t, k in self.tickets.items() if k['fp'] in fps}
            for sid in [s for s, x in self.sessions.items() if x['fp'] not in fps]:
                self.sessions.pop(sid)
            if gone:
                self.save()

    def drop_edge(self, pid):
        self.edges.pop(pid, None)
        for sid in list(self.sessions):
            x = self.sessions[sid]
            if pid in x['pids']:
                x['pids'] = [p for p in x['pids'] if p != pid]
                x['blobs'].pop(pid, None)

    def sweep(self, now):
        """过期的配对、票据，很久没用的配对"""
        for rid in [r for r, s in self.slots.items() if s['exp'] < now]:
            self.slots.pop(rid)
        for tid in [t for t, k in self.tickets.items() if k['exp'] < now]:
            self.tickets.pop(tid)
        old = [p for p, e in self.edges.items() if max(e.get('used') or 0, e.get('at') or 0) < now - EDGE_IDLE]
        for p in old:
            self.drop_edge(p)
        if old:
            self.save()

    def ticket(self, now, **what):
        if sum(1 for k in self.tickets.values() if k['fp'] == what['fp']) >= TICKETS_DEV:
            return 429, {'error': 'rate'}
        tid, key = os.urandom(16).hex(), os.urandom(32)
        self.tickets[tid] = dict(what, key=key, exp=now + TICKET_TTL)
        return 200, {'v': API, 'ok': True, 'tid': tid, 'tkey': base64.b64encode(key).decode(), 'port': self.port, 'ttl': TICKET_TTL}

    def take_ticket(self, tid, role, now, proof):
        """用掉一张票据。proof(票据的密钥) 核对来的人确实有这把密钥：对不上的不算用掉（光知道编号的人废不了别人的票据）"""
        with self.lock:
            k = self.tickets.get(tid)
            if not k or k['role'] != role or k['exp'] < now or not proof(k['key']):
                return None
            return self.tickets.pop(tid)

    def op(self, op, fp, j, now, name_of):
        """签名、时间、nonce 都验过了的请求。返回 (状态码, 回的内容)"""
        hex32 = lambda x: isinstance(x, str) and bool(AID_RE.match(x))
        with self.lock:
            self.sweep(now)
            if op.startswith('pair_') and op != 'pair_drop':
                rid = j.get('rid')
                if not hex32(rid):
                    return 400, {'error': 'bad'}
                s = self.slots.get(rid)
                if op == 'pair_open':
                    if s is not None:
                        return 409, {'error': 'taken'}
                    if sum(1 for x in self.slots.values() if x['owner'] == fp) >= PAIR_SLOTS:
                        return 429, {'error': 'rate'}
                    self.slots[rid] = {'owner': fp, 'joiner': None, 'msgs': {}, 'commit': {}, 'done': False, 'exp': now + PAIR_TTL}
                    return 200, {'v': API, 'ok': True, 'ttl': PAIR_TTL}
                if s is None:
                    return 404, {'error': 'no_pairing'}
                if fp != s['owner'] and s['joiner'] is None and op == 'pair_put':
                    s['joiner'] = fp   # 头一个放密文进来的别的设备
                if fp not in (s['owner'], s['joiner']):
                    return 409, {'error': 'taken'}
                peer = s['joiner'] if fp == s['owner'] else s['owner']
                if op == 'pair_close':   # 这一边中途不做了：这次配对作废（已经记下的配对不受影响），等着的那一边马上知道
                    self.slots.pop(rid)
                    self.cond.notify_all()
                    return 200, {'v': API, 'ok': True}
                if op == 'pair_put':
                    m = parse_blob(j.get('m'), PAIR_MSG)
                    if m is None or fp in s['msgs']:
                        return 400, {'error': 'bad'}
                    s['msgs'][fp] = m
                    self.cond.notify_all()
                    return 200, {'v': API, 'ok': True}
                if op == 'pair_get':
                    # 等对方放密文进来（until 是 "done" 时等两边都报了配对编号），最多 wait 秒；等不到照现在的回
                    wait = j.get('wait')
                    wait = min(float(wait), PAIR_WAIT) if num_ok(wait) else 0
                    done_only = j.get('until') == 'done'

                    def theirs():
                        p = s['joiner'] if fp == s['owner'] else s['owner']
                        return p, (s['msgs'].get(p) if p else None)
                    ready = lambda: s['done'] or (not done_only and theirs()[1] is not None) or self.slots.get(rid) is not s
                    if wait > 0 and not ready() and PAIR_WAITERS.acquire(blocking=False):
                        try:
                            end = time.monotonic() + wait
                            while not ready() and end - time.monotonic() > 0:
                                self.cond.wait(end - time.monotonic())
                        finally:
                            PAIR_WAITERS.release()
                    peer, m = theirs()
                    return 200, {'v': API, 'ok': True, 'm': m, 'fp': peer if m else None, 'name': name_of(peer) if m else None,
                                 'done': s['done']}
                if op == 'pair_commit':
                    pid = j.get('pid')
                    if not hex32(pid) or s['joiner'] is None:
                        return 400, {'error': 'bad'}
                    s['commit'][fp] = pid
                    both = [s['commit'].get(s['owner']), s['commit'].get(s['joiner'])]
                    if None not in both and not s['done']:
                        if both[0] != both[1] or pid in self.edges:
                            self.slots.pop(rid)
                            self.cond.notify_all()
                            return 409, {'error': 'mismatch'}
                        for who in (s['owner'], s['joiner']):
                            if sum(1 for e in self.edges.values() if who in (e['a'], e['b'])) >= EDGES_DEV:
                                self.slots.pop(rid)
                                self.cond.notify_all()
                                return 429, {'error': 'edges'}
                        if len(self.edges) >= EDGES_MAX:
                            return 507, {'error': 'full'}
                        # 同两台设备以前配过的换掉（重新配对）
                        for old in [p for p, e in self.edges.items() if {e['a'], e['b']} == {s['owner'], s['joiner']}]:
                            self.drop_edge(old)
                        self.edges[pid] = {'a': s['owner'], 'b': s['joiner'], 'at': now, 'used': now}
                        s['done'] = True
                        s['exp'] = min(s['exp'], now + 60)   # 留一会儿，先报的那一边还要来问一次
                        self.save()
                        self.cond.notify_all()
                    return 200, {'v': API, 'ok': True, 'done': s['done']}
            if op == 'pair_drop':
                pid = j.get('pid')
                if not hex32(pid):
                    return 400, {'error': 'bad'}
                if self.member(pid, fp):
                    self.drop_edge(pid)
                    self.save()
                return 200, {'v': API, 'ok': True}
            if op == 'relay_host':
                sid, pids = j.get('sid'), j.get('pids')
                if not (hex32(sid) and isinstance(pids, list) and 0 < len(pids) <= EDGES_DEV and all(hex32(p) for p in pids)):
                    return 400, {'error': 'bad'}
                pids = [p for p in dict.fromkeys(pids) if self.member(p, fp)]
                if not pids:
                    return 404, {'error': 'no_pairing'}
                cur = self.sessions.get(sid)
                if cur and cur['fp'] != fp:
                    return 409, {'error': 'taken'}
                if not cur and sum(1 for x in self.sessions.values() if x['fp'] == fp) >= SESSIONS_DEV:
                    return 429, {'error': 'sessions'}
                return self.ticket(now, role='host', fp=fp, sid=sid, pids=pids)
            if op == 'relay_attach':
                sid, pid = j.get('sid'), j.get('pid')
                if not (hex32(sid) and hex32(pid)):
                    return 400, {'error': 'bad'}
                x = self.sessions.get(sid)
                if not self.member(pid, fp) or not x or pid not in x['pids'] or x['fp'] != self.other(pid, fp):
                    return 404, {'error': 'no_session'}
                if x['viewers'] >= VIEWERS:
                    return 429, {'error': 'busy'}
                if now - (self.edges[pid].get('used') or 0) > 86400:   # 最近用过：一天最多记一次、写一次盘（180 天没用过的配对会删掉）
                    self.edges[pid]['used'] = now
                    self.save()
                return self.ticket(now, role='view', fp=fp, sid=sid, pid=pid)
            if op == 'relay_list':
                pids = j.get('pids')
                if not (isinstance(pids, list) and len(pids) <= EDGES_DEV and all(hex32(p) for p in pids)):
                    return 400, {'error': 'bad'}
                out, peers = {}, {}
                for pid in dict.fromkeys(pids):
                    if not self.member(pid, fp):
                        continue   # 对方取消了配对（或者从来没有）：不在回的里面
                    peer = self.other(pid, fp)
                    peers[pid] = {'fp': peer, 'name': name_of(peer)}
                    out[pid] = [{'sid': sid, 'blob': x['blobs'][pid], 'age': ago(now, x['since']), 'viewers': x['viewers'],
                                 'online': x['conn'] is not None}
                                for sid, x in self.sessions.items() if x['fp'] == peer and pid in x['blobs']]
                return 200, {'v': API, 'ok': True, 'peers': peers, 'sessions': out}
        return 404, {'error': 'not_found'}


class RelayConn:
    __slots__ = ('sock', 'ip', 'state', 'inb', 'out', 'peer', 'born', 'seen', 'sid', 'pid', 'key', 'reply', 'paused', 'events')

    def __init__(self, sock, ip, now):
        self.sock, self.ip, self.state, self.inb, self.out = sock, ip, 'pre', bytearray(), bytearray()
        self.peer, self.born, self.seen, self.sid, self.key, self.reply, self.paused, self.events = None, now, now, None, None, None, False, 0
        self.pid = None   # 接入方的连接、和它对接的那一条：是哪个配对的（配对没了就断开）


class Relay(threading.Thread):
    """数据端口：一条线程用 selectors 管所有连接。连上来先说一行开场白（可见的 ASCII），凭票据进来：
    - host：会话的控制连接（一个会话一条）。之后是帧：类型(1) 长度(2) 内容——宿主说 ANNOUNCE（各个配对的说明密文）、PING、BYE，
      服务器说 JOIN（有接入方来了：带一个令牌）、PONG
    - view：接入方。服务器给宿主的控制连接发 JOIN，等宿主拿着令牌另连一条（join）上来，再回 OK
    - join：宿主为这个接入方连上来的那一条。对上以后两条连接原样对接：服务器只转字节，里面是两头之间端到端加密的内容"""
    daemon = True

    def __init__(self, sock, remote):
        super().__init__(name='relay')
        self.lsock, self.remote = sock, remote
        self.sel = selectors.DefaultSelector()
        self.conns, self.per_ip, self.waiting = set(), {}, {}   # waiting：令牌 → (接入方的连接, 宿主控制连接的票据密钥, 什么时候)

    def want(self, c):
        ev = (0 if c.paused or c.state == 'wait' else selectors.EVENT_READ) | (selectors.EVENT_WRITE if c.out else 0)
        if ev == c.events:
            return
        if c.events and ev:
            self.sel.modify(c.sock, ev, c)
        elif ev:
            self.sel.register(c.sock, ev, c)
        else:
            self.sel.unregister(c.sock)
        c.events = ev

    def send(self, c, data):
        c.out += data
        if c.state == 'ctl' and len(c.out) > RELAY_QUEUE:   # 控制连接上只发不收（一个 PING 换一个 PONG）：不替它攒着
            return self.close(c)
        self.want(c)

    def close(self, c, why=None):
        if c not in self.conns:
            return
        self.conns.discard(c)
        if why and c.state in ('pre', 'wait'):
            try:
                c.sock.send(b'CCA-RELAY/1 ERR %s\n' % why)
            except OSError:
                pass
        if c.events:
            self.sel.unregister(c.sock)
            c.events = 0
        try:
            c.sock.close()
        except OSError:
            pass
        n = self.per_ip.get(c.ip, 1) - 1
        if n > 0:
            self.per_ip[c.ip] = n
        else:
            self.per_ip.pop(c.ip, None)
        with self.remote.lock:
            x = self.remote.sessions.get(c.sid) if c.sid else None
            if c.state == 'ctl' and x and x['conn'] is c:
                x['conn'], x['lost'] = None, time.monotonic()
            elif c.state == 'pipe' and x:   # 对接着的一对断了一头：这一处接入算结束（另一头下面改成 drain，不会再算一次）
                x['viewers'] = max(x['viewers'] - 1, 0)
        for tok in [t for t, w in self.waiting.items() if w[0] is c]:
            self.waiting.pop(tok)
        peer, c.peer = c.peer, None
        if peer is not None:
            # 另一头：还没转给它的先写完再关（会话结束时宿主最后说的那一句“结束了”不能丢），最多再等 5 秒
            peer.peer, peer.state, peer.paused, peer.born = None, 'drain', True, time.monotonic()
            if peer.out and peer in self.conns:
                self.want(peer)
            else:
                self.close(peer)

    def frame(self, kind, payload=b''):
        return bytes([kind]) + len(payload).to_bytes(2, 'big') + payload

    def preamble(self, c, line, now):
        m = RELAY_LINE.match(line)
        if not m:
            return self.close(c, b'bad')
        role, ident, cn, mac = m.group(1).decode(), m.group(2).decode(), m.group(3), m.group(4)
        if role == 'join':
            w = self.waiting.get(ident)
            if not w or not hmac.compare_digest(mac, b64u(hmac256(w[1], b'j' + ident.encode() + cn))):
                return self.close(c, b'ticket')
            v = w[0]
            self.waiting.pop(ident)
            c.peer, v.peer, c.state, v.state, c.sid, c.pid, c.reply = v, c, 'pipe', 'pipe', v.sid, v.pid, 'join'
            with self.remote.lock:
                x = self.remote.sessions.get(v.sid)
                if x:
                    x['viewers'] += 1
            self.send(c, b'CCA-RELAY/1 OK %s\n' % b64u(hmac256(w[1], b's' + ident.encode() + cn)))
            self.send(v, v.key)   # 接入方那句 OK 是早先备好的
            v.key = None
            self.want(v)
            return None
        k = self.remote.take_ticket(ident, role, time.time(),
                                    lambda key: hmac.compare_digest(mac, b64u(hmac256(key, b'c' + role.encode() + ident.encode() + cn))))
        if not k:
            return self.close(c, b'ticket')
        ok = b'CCA-RELAY/1 OK %s\n' % b64u(hmac256(k['key'], b's' + ident.encode() + cn))
        c.sid = k['sid']
        with self.remote.lock:
            x = self.remote.sessions.get(k['sid'])
            if role == 'host':
                if x and x['fp'] != k['fp']:
                    return self.close(c, b'taken')
                old = x['conn'] if x else None
                if x:
                    x['conn'], x['pids'], x['lost'] = c, k['pids'], None
                    x['blobs'] = {p: b for p, b in x['blobs'].items() if p in k['pids']}
                elif sum(1 for s in self.remote.sessions.values() if s['fp'] == k['fp']) >= SESSIONS_DEV:
                    return self.close(c, b'sessions')   # 换票据时数过一次；攒着票据一起连上来的在这里拦住
                else:
                    self.remote.sessions[k['sid']] = {'fp': k['fp'], 'pids': k['pids'], 'blobs': {}, 'conn': c, 'lost': None,
                                                      'viewers': 0, 'since': time.time()}
                c.state, c.key = 'ctl', k['key']
                self.send(c, ok)
                if old is not None and old is not c:
                    old.state = 'gone'   # 被新的控制连接顶掉了：关它的时候不动会话
                    self.close(old)
                return None
            host = x['conn'] if x else None
            if not x or host is None or k['pid'] not in x['pids']:
                return self.close(c, b'nohost')
            token = os.urandom(16).hex()
            self.waiting[token] = (c, host.key, now)
            c.state, c.key, c.reply, c.pid = 'wait', ok, 'view', k['pid']
            self.want(c)
            self.send(host, self.frame(F_JOIN, json.dumps({'token': token, 'pid': k['pid']}, separators=(',', ':')).encode()))
        return None

    def control(self, c):
        """宿主的控制连接上的帧：读到的这一块里有几帧就处理几帧，处理过的最后一起从缓冲区里拿掉（一块里全是 3 个字节的
        小帧时，每帧挪一次缓冲区是平方级的，而这条线程管着所有连接）"""
        buf, i = c.inb, 0
        while c in self.conns and len(buf) - i >= 3:
            n = int.from_bytes(buf[i + 1:i + 3], 'big')
            if n > CTL_FRAME:
                self.close(c)
                break
            if len(buf) - i < 3 + n:
                break
            kind, payload = buf[i], bytes(buf[i + 3:i + 3 + n])
            i += 3 + n
            self.control_frame(c, kind, payload)
        del buf[:i]

    def control_frame(self, c, kind, payload):
        """控制连接上的一帧。不对的就把这条连接关掉（control 看到它不在了就停）"""
        if kind == F_PING:
            self.send(c, self.frame(F_PONG, payload[:16]))
        elif kind == F_BYE:
            with self.remote.lock:
                self.remote.sessions.pop(c.sid, None)
            c.state = 'gone'
            self.close(c)
        elif kind == F_ANNOUNCE:
            try:
                blobs = json.loads(payload.decode('utf-8'))
            except (ValueError, RecursionError):   # 嵌套几万层的 JSON 是 RecursionError，不是 ValueError
                return self.close(c)
            with self.remote.lock:
                x = self.remote.sessions.get(c.sid)
                if not x or x['conn'] is not c or not isinstance(blobs, dict):
                    return self.close(c)
                good = {p: parse_blob(b, DIR_BLOB) for p, b in blobs.items() if p in x['pids']}
                if None in good.values():
                    return self.close(c)
                x['blobs'] = good
        return None

    def readable(self, c, now):
        try:
            data = c.sock.recv(65536)
        except (BlockingIOError, InterruptedError):
            return
        except OSError:
            return self.close(c)
        if not data:
            return self.close(c)
        c.seen = now
        if c.state == 'pipe':
            p = c.peer
            if p is None:
                return self.close(c)
            p.seen = now
            self.send(p, data)
            if len(p.out) > RELAY_QUEUE:   # 对面写不动：先不读这一头（让发的那一头自己慢下来）
                c.paused = True
                self.want(c)
            return None
        c.inb += data
        if c.state == 'pre':
            i = c.inb.find(b'\n')
            if i < 0:
                return self.close(c, b'bad') if len(c.inb) > 256 else None
            line = bytes(c.inb[:i])
            del c.inb[:i + 1]
            self.preamble(c, line, now)
            if c not in self.conns:
                return None
        if c.state == 'ctl':
            self.control(c)
        elif c.state in ('wait', 'pipe') and c.inb:
            return self.close(c)   # 回 OK 之前不该有别的话
        return None

    def writable(self, c):
        try:
            n = c.sock.send(c.out)
        except (BlockingIOError, InterruptedError):
            return
        except OSError:
            return self.close(c)
        del c.out[:n]
        if c.state == 'drain' and not c.out:
            return self.close(c)
        p = c.peer
        if p is not None and p.paused and len(c.out) < RELAY_QUEUE // 2:
            p.paused = False
            self.want(p)
        self.want(c)
        return None

    def sweep(self, now):
        for c in list(self.conns):
            if c.state == 'pre' and now - c.born > PREAMBLE_WAIT:
                self.close(c, b'slow')
            elif c.state == 'wait' and now - c.born > PREAMBLE_WAIT + JOIN_WAIT:
                self.close(c, b'nohost')
            elif c.state == 'drain' and now - c.born > 5:
                self.close(c)
            elif c.state in ('ctl', 'pipe') and now - c.seen > RELAY_IDLE:
                self.close(c)
        with self.remote.lock:
            for sid in [s for s, x in self.remote.sessions.items() if x['conn'] is None and now - (x['lost'] or now) > RELAY_GRACE]:
                self.remote.sessions.pop(sid)
            # 配对取消了、设备撤销了：会话不在了的控制连接断开，那个配对正接着的（和等着接的）也断开
            for c in [c for c in self.conns if c.state == 'ctl' and self.remote.sessions.get(c.sid, {}).get('conn') is not c]:
                c.state = 'gone'
                self.close(c)
            for c in [c for c in self.conns if c.state in ('pipe', 'wait') and c.pid not in self.remote.edges]:
                self.close(c)

    def accept(self, now):
        try:
            sock, addr = self.lsock.accept()
        except OSError:
            return
        ip = addr[0]
        if len(self.conns) >= RELAY_CONN or self.per_ip.get(ip, 0) >= RELAY_CONN_IP:
            sock.close()
            return
        sock.setblocking(False)
        try:
            sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        except OSError:
            pass
        c = RelayConn(sock, ip, now)
        self.conns.add(c)
        self.per_ip[ip] = self.per_ip.get(ip, 0) + 1
        self.want(c)

    def oops(self, what, c=None):
        """这条线程管着所有连接：哪一步出了没料到的错，都只断开那一条、记一行（只记是哪种错，不记内容），接着转别的"""
        sys.stderr.write('cca-presence: 中转%s出错（%s），接着运行\n' % (what, type(sys.exc_info()[1]).__name__))
        sys.stderr.flush()
        if c is not None:
            try:
                self.close(c)
            except Exception:
                self.conns.discard(c)

    def run(self):
        self.sel.register(self.lsock, selectors.EVENT_READ, None)
        swept = 0
        while True:
            try:
                events = self.sel.select(1.0)
            except Exception:
                self.oops('等连接的动静时')
                time.sleep(0.2)
                continue
            for key, ev in events:
                now = time.monotonic()
                c = key.data
                try:
                    if c is None:
                        self.accept(now)
                        continue
                    if ev & selectors.EVENT_WRITE and c in self.conns:
                        self.writable(c)
                    if ev & selectors.EVENT_READ and c in self.conns:
                        self.readable(c, now)
                except Exception:
                    self.oops('的一条连接', c)
            now = time.monotonic()
            if now - swept >= 1:
                swept = now
                try:
                    self.sweep(now)
                except Exception:
                    self.oops('清理过期的连接时')


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    server_version = 'cca-presence/1'
    sys_version = ''
    timeout = 15

    def log_message(self, fmt, *args):   # 不记每个请求：谁在什么时候用了哪个账号不写进日志
        pass

    def send(self, code, obj):
        body = json.dumps(obj, ensure_ascii=False, separators=(',', ':')).encode('utf-8')
        self.send_response(code)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        self.send_header('Cache-Control', 'no-store')
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == '/v1/health':
            return self.send(200, self.server.store.health(self.server.sig_ok, self.server.remote and self.server.remote.port))
        return self.send(404, {'error': 'not_found'})

    def do_POST(self):
        srv = self.server
        if not srv.ip_bucket.take(self.client_address[0]):
            return self.send(429, {'error': 'rate'})
        try:
            n = int(self.headers.get('Content-Length') or '')
        except ValueError:
            return self.send(411, {'error': 'bad'})
        op = self.path[len('/v1/'):] if self.path.startswith('/v1/') else ''
        if n < 0 or n > (MAX_SYNC_BODY if op in SYNC_OPS else MAX_BODY):
            return self.send(413, {'error': 'bad'})
        body = self.rfile.read(n)
        try:
            sig = base64.b64decode(self.headers.get('X-Cca-Sig') or '', validate=True)
        except ValueError:
            sig = b''
        if len(body) != n or not sig or len(sig) > MAX_SIG:
            return self.send(400, {'error': 'bad'})
        now = time.time()
        own = srv.store.mode == 'own'
        if op == 'machines' and not own:   # 自己的服务器没有维护者的设备清单：设备经 join 加入（404）
            code, reply = srv.store.set_machines(body, sig, srv.release_key)
            if code == 200 and srv.remote:
                srv.remote.keep_only(srv.store.fps())
            return self.send(code, reply)
        relay = op in RELAY_OPS and srv.remote is not None   # 没开中转时和旧版服务器一样：不认这几个 op（404）
        if op not in ('report', 'view') and op not in SYNC_OPS and not relay and not (own and op in OWN_OPS):
            return self.send(404, {'error': 'not_found'})
        try:
            j = json.loads(body.decode('utf-8'))
        except ValueError:
            return self.send(400, {'error': 'bad'})
        if not (isinstance(j, dict) and j.get('v') == API and j.get('op') == op):
            return self.send(400, {'error': 'bad'})
        fp = j.get('key')
        if not (isinstance(fp, str) and FP_RE.match(fp)):
            return self.send(400, {'error': 'bad'})
        if op == 'join':
            # 加入：拿要加入的这把密钥签名。入伙口令不对和签名不对回一样的 403（不留判断口）；口令先比（便宜），对了才验签
            pub = j.get('pub')
            k = KEY_RE.match(pub) if isinstance(pub, str) else None
            if not (k and key_fp(k.group(1)) == fp and srv.store.admit_ok(j.get('admit'))
                    and ssh_verify('ssh-ed25519 ' + k.group(1), NS_REPORT, body, sig)):
                return self.send(403, {'error': 'denied'})
        else:
            m = srv.store.machine(fp)
            if not m:   # 不在设备清单里（维护者还没同步、撤销了；自己的服务器上是被移除了、记录清掉了：设备重新加入）
                return self.send(403, {'error': 'unknown_device' if own else 'denied'})
            if not ssh_verify(m['key'], NS_REPORT, body, sig):
                return self.send(403, {'error': 'denied'})
        if not srv.fp_bucket.take(fp):   # 验签之后才扣：别人拿这台设备的指纹乱发，扣不到它的配额
            return self.send(429, {'error': 'rate'})
        ts = j.get('ts')
        if not isinstance(ts, (int, float)) or isinstance(ts, bool) or abs(ts - now) > TS_WINDOW:
            return self.send(400, {'error': 'clock', 'now': now})
        nonce = j.get('nonce')
        if not (isinstance(nonce, str) and NONCE_RE.match(nonce)):
            return self.send(400, {'error': 'bad'})
        if not srv.nonces.add(nonce, now):
            return self.send(409, {'error': 'replay'})
        if own and op != 'join':   # 加入的那一次在 join 里记（没加入成的不记）
            srv.store.seen[fp] = now
        if op == 'join':
            name, check = j.get('name'), j.get('check')
            if not (isinstance(name, str) and 0 < len(name) <= 64 and name.strip() == name and not NAME_BAD.search(name)
                    and isinstance(check, str) and CHECK_RE.match(check)):
                return self.send(400, {'error': 'bad'})
            return self.send(*srv.store.join(fp, 'ssh-ed25519 ' + k.group(1), name, check, now))
        if op == 'members':
            return self.send(*srv.store.members(fp, now))
        if op == 'remove':
            target = j.get('target')
            if not (isinstance(target, str) and FP_RE.match(target)):
                return self.send(400, {'error': 'bad'})
            code, reply = srv.store.remove(target)
            if code == 200 and srv.remote:
                srv.remote.keep_only(srv.store.fps())
            return self.send(code, reply)
        if relay:
            return self.send(*srv.remote.op(op, fp, j, now, srv.store.name))
        if op == 'report':
            units, end = parse_units(j.get('units')), j.get('end')
            if units is None or end not in (None, 'none', 'down') or (end == 'none' and units):
                return self.send(400, {'error': 'bad'})
            srv.store.report(fp, units, end, now)
            return self.send(200, {'v': API, 'ok': True, 'ttl': srv.store.ttl, 'idle': srv.store.idle})
        if op in SYNC_OPS:
            parsed = parse_sync(j, SYNC_OPS[op])
            if parsed is None:
                return self.send(400, {'error': 'bad'})
            put, get, have = parsed
            if op == 'keys':
                return self.send(*srv.store.key_sync(fp, put, get, now))
            return self.send(*srv.store.cost_sync(fp, put, get, have, now))
        aids = j.get('accounts')
        if not (isinstance(aids, list) and len(aids) <= MAX_ACCOUNTS and all(isinstance(a, str) and AID_RE.match(a) for a in aids)):
            return self.send(400, {'error': 'bad'})
        return self.send(200, srv.store.view(fp, list(dict.fromkeys(aids)), now))


class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):   # 就是 3.7 起的 http.server.ThreadingHTTPServer（3.6 上没有）
    daemon_threads = True
    allow_reuse_address = True
    request_queue_size = 64

    def __init__(self, addr, ctx, store, release_key, sig_ok, remote=None):
        self.ctx, self.store, self.release_key, self.sig_ok, self.remote = ctx, store, release_key, sig_ok, remote
        self.ip_bucket, self.fp_bucket, self.nonces = Buckets(2, 120), Buckets(0.5, 30), Nonces()
        self.slots = threading.BoundedSemaphore(MAX_CONN)
        self.per_ip, self.per_ip_lock = {}, threading.Lock()
        super().__init__(addr, Handler)

    def take_slot(self, ip):
        """这个连接能不能处理：总数、同一个 IP 的数都没到上限"""
        with self.per_ip_lock:
            if self.per_ip.get(ip, 0) >= MAX_CONN_IP:
                return False
            if not self.slots.acquire(blocking=False):
                return False
            self.per_ip[ip] = self.per_ip.get(ip, 0) + 1
            return True

    def free_slot(self, ip):
        with self.per_ip_lock:
            n = self.per_ip.get(ip, 1) - 1
            if n > 0:
                self.per_ip[ip] = n
            else:
                self.per_ip.pop(ip, None)
            self.slots.release()

    def process_request(self, request, client_address):
        if not self.take_slot(client_address[0]):   # 连接太多：直接断开，不排队
            self.shutdown_request(request)
            return
        try:
            super().process_request(request, client_address)
        except Exception:
            self.free_slot(client_address[0])
            raise

    def process_request_thread(self, request, client_address):
        try:
            super().process_request_thread(request, client_address)
        finally:
            self.free_slot(client_address[0])

    def handle_error(self, request, client_address):   # 一行就够：不打印整段 traceback
        sys.stderr.write('cca-presence: 处理请求出错：%s\n' % (sys.exc_info()[1],))

    def finish_request(self, request, client_address):
        """TLS 握手放在处理这个连接的线程里：握手最多 5 秒，整个连接从接进来起最多 15 秒（到点从别的线程把它断掉，
        一个字节一个字节慢慢发也占不住）"""
        def cut():
            try:
                request.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
        timer = threading.Timer(CONN_DEADLINE, cut)
        timer.daemon = True
        timer.start()
        try:
            request.settimeout(HANDSHAKE_TIMEOUT)
            try:
                tls = self.ctx.wrap_socket(request, server_side=True)
            except (OSError, ValueError):
                return
            try:
                self.RequestHandlerClass(tls, client_address, self)
            finally:
                try:
                    tls.close()
                except OSError:
                    pass
        finally:
            timer.cancel()


def main():
    ap = argparse.ArgumentParser(description='cca 的设备在线服务')
    ap.add_argument('--listen', default='0.0.0.0:18443', help='监听的地址:端口（端口 0：随便挑一个，写进 --port-file）')
    ap.add_argument('--cert', required=True, help='TLS 证书（自签的 P-256；macOS 的 curl 只给 RSA 和 P-256/P-384 做公钥钉住）')
    ap.add_argument('--key', required=True, help='TLS 私钥')
    ap.add_argument('--mode', choices=('maintainer', 'own'), default='maintainer',
                    help='maintainer：维护者的服务器（设备来自签名的设备清单）；own：自己的同步服务器（设备拿加入链接加入）')
    ap.add_argument('--release-key', help='发布公钥（ssh-ed25519 …）：只认它签的设备清单（维护者的服务器必须给）')
    ap.add_argument('--admit-hash', default=os.environ.get('CCA_HUB_ADMIT_HASH'),
                    help='自己的同步服务器：入伙口令的 SHA-256（64 位十六进制；也可以用环境变量 CCA_HUB_ADMIT_HASH）')
    ap.add_argument('--state', required=True, help='状态文件')
    ap.add_argument('--ttl', type=float, default=150, help='开着会话却这么多秒没心跳就算离线')
    ap.add_argument('--idle', type=float, default=600, help='这么多秒没动静算闲置')
    ap.add_argument('--retain-days', type=float, default=30, help='上次使用的记录留几天')
    ap.add_argument('--keys', help='API key 的密文（默认在状态文件旁边的 keys.json）')
    ap.add_argument('--cost', help='折合金额的密文（默认在状态文件旁边的 cost-slots.json）')
    ap.add_argument('--cost-days', type=float, default=100, help='折合金额的密文这么多天没再传就删掉')
    ap.add_argument('--port-file', help='实际监听的端口写到这里（测试用）')
    ap.add_argument('--relay-listen', help='远程会话的中转：数据端口监听的地址:端口（不给就不开中转）')
    ap.add_argument('--relay-port', type=int, help='客户端连数据端口用的端口（容器里外端口不一样时给；默认就是监听的那个）')
    ap.add_argument('--relay-port-file', help='数据端口实际监听的端口写到这里（测试用）')
    ap.add_argument('--remote', help='配对的记录（默认在状态文件旁边的 remote.json）')
    a = ap.parse_args()
    release = None
    if a.mode == 'own':
        if not HASH_RE.match(a.admit_hash or ''):
            ap.error('自己的同步服务器要给入伙口令的哈希（--admit-hash 或者环境变量 CCA_HUB_ADMIT_HASH，64 位小写十六进制）')
    else:
        if not a.release_key:
            ap.error('维护者的服务器要给 --release-key')
        with open(a.release_key) as f:
            k = KEY_RE.match(' '.join(f.read().split()[:2]))
        if not k:
            ap.error('%s 不是 ssh-ed25519 公钥' % a.release_key)
        release = 'ssh-ed25519 ' + k.group(1)
    host, _, port = a.listen.rpartition(':')
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    if hasattr(ssl, 'TLSVersion'):
        ctx.minimum_version = ssl.TLSVersion.TLSv1_2
    else:   # Python 3.6（CentOS 7、一些云厂商的系统镜像里的 python3）
        ctx.options |= ssl.OP_NO_TLSv1 | ssl.OP_NO_TLSv1_1
    ctx.load_cert_chain(a.cert, a.key)
    sig_ok = self_test()
    store = Store(a.state, a.ttl, a.idle, a.retain_days * 86400, a.keys, a.cost, a.cost_days * 86400, a.mode, a.admit_hash)
    remote = None
    if a.relay_listen:
        rhost, _, rport = a.relay_listen.rpartition(':')
        rsock = socket.socket(socket.AF_INET6 if ':' in rhost else socket.AF_INET)
        rsock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        rsock.bind((rhost.strip('[]') or '0.0.0.0', int(rport)))
        rsock.listen(64)
        rsock.setblocking(False)
        remote = Remote(a.remote or os.path.join(os.path.dirname(a.state) or '.', 'remote.json'), a.relay_port or rsock.getsockname()[1])
        remote.keep_only(store.fps())
        Relay(rsock, remote).start()
        if a.relay_port_file:
            with open(a.relay_port_file + '.tmp', 'w') as f:
                f.write('%d\n' % rsock.getsockname()[1])
            os.replace(a.relay_port_file + '.tmp', a.relay_port_file)
    srv = Server((host.strip('[]') or '0.0.0.0', int(port)), ctx, store, release, sig_ok, remote)
    if a.port_file:
        with open(a.port_file + '.tmp', 'w') as f:
            f.write('%d\n' % srv.server_address[1])
        os.replace(a.port_file + '.tmp', a.port_file)
    sys.stderr.write('cca-presence: %s监听 %s:%d，设备 %d 台，签名自检%s%s\n' % (
        '自己的同步服务器，' if a.mode == 'own' else '', srv.server_address[0], srv.server_address[1], len(store.data['machines']),
        '通过' if sig_ok else '没通过',
        '，中转的数据端口 %d（%d 个配对）' % (rsock.getsockname()[1], len(remote.edges)) if remote else ''))
    sys.stderr.flush()
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == '__main__':
    main()
