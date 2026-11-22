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
403 unknown_device（带着这一份记录的编号 gen：和设备加入时记下的一样是被移除了，不一样是服务器的记录没了，设备自己重新加入），
签名不对回 403 denied。成员能换入伙口令（relink：服务器只拿到新口令的哈希和一份它解不开的密文，成员拿盐解开它，换上新的加入链接；
已经加入的设备照旧）。启动时配置的入伙口令的哈希和上次的不一样（用新的加入链接重新部署了）就从头开始：以前的设备、记录、密文都不要了。
格式见 README.md「自己的同步服务器」。

Team 的同步服务器（--mode team --team-root 根公钥：cca team）：设备统计、API key、折合金额照旧，另外存着管理员给每台设备的配置
（账号走哪个节点、节点的定义：管理员的设备加密、拿 Team 的根密钥签名，服务器解不开也改不了）。管理员的请求拿根密钥签名（key 是
根公钥的指纹）：直接登记设备的公钥（team_enroll）、发只能用一次的邀请（team_invite：服务器只存令牌的 SHA-256）、存各台设备的
配置（team_put：每台一份，序号只增不减）、列出设备（team_members）、移除设备（team_revoke）。设备拿邀请里的令牌加入
（team_join）、取自己的那一份配置（team_get）、退出（team_leave）；不在名单里回 403 unknown_device（带着 gen，和自己的同步
服务器一样）。配置的根公钥和上次的不一样就从头开始。格式见 README.md「Team 的同步服务器」。

在托管平台、反向代理后面（--behind-proxy）：只说 HTTP，HTTPS 由平台做（设备照证书机构验它的证书，加入链接里没有指纹）；
每个 IP 的限额按 X-Forwarded-For 的最后一项算（平台的代理加的那一项，客户端改不了），同一个 IP 同时几个连接不限（连过来的都是代理）。
环境变量 CCA_HUB_HOSTED=1（cca hub hosted 生成的部署设置）等于 --mode own --behind-proxy --listen 0.0.0.0:8080 --state /data/state.json，
平台的表单里只填一条命令 python3 -I /app/cca-presence.py；/data 不在（没挂存储）时不启动。
"""
import argparse
import base64
import collections
import gzip
import hashlib
import hmac
import http.server
import ipaddress
import json
import os
import re
import secrets
import select
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
GEN_RE = re.compile(r'^[0-9a-f]{32}$')       # 这一份记录的编号（从头开始时随机生成）
MAX_MEMBERS = 64           # 自己的服务器最多几台设备
MAX_LINK_BLOB = 512        # 换入伙口令时成员存的那份密文（base64）最长多少个字符（里面是两个口令，不到 100 个字节）
OWN_OPS = ('join', 'members', 'remove', 'relink')     # 只有自己的服务器有的 op
DEV_RE = re.compile(r'^[0-9a-f]{16}$')       # 设备在 Team 里的编号（管理员发邀请时定的）
MAX_TEAM_MEMBERS = 256     # 一个 Team 最多几台设备
MAX_INVITES = 64           # 还没用掉的邀请最多几个
INVITE_TTL = (60, 30 * 86400)   # 邀请多久过期：最短、最长（秒）
INVITE_KEEP = 30 * 86400   # 过期的、用掉的邀请记多久（这段时间里拿它来加入，说得出是过期了还是用过了）
SEQ_MAX = 2 ** 53          # 配置的序号比它小（各种语言的 JSON 都读得准的整数）
MAX_TEAM_DOC = 131072      # 一台设备的配置（密文，base64）最长多少个字符
MAX_TEAM_SIG = 1024        # 配置的签名（armored 签名的 base64）最长多少个字符
MAX_TEAM_BATCH = 64        # team_enroll、team_put 一次最多几台设备
TEAM_DOCS_BYTES = 16 << 20     # 所有设备的配置（密文，base64）加起来最多这么多个字符
TEAM_ADMIN_OPS = ('team_enroll', 'team_invite', 'team_put', 'team_members', 'team_revoke')   # 要 Team 的根密钥签名
TEAM_OPS = TEAM_ADMIN_OPS + ('team_join', 'team_get', 'team_leave')     # 只有 Team 的服务器有的 op

VERIFY_SLOTS = threading.BoundedSemaphore(4)   # 同时验几个签名（一个十几毫秒 CPU）


def key_fp(b64):
    """公钥（base64 那一段）的指纹，和 ssh-keygen -l 的写法一样"""
    return 'SHA256:' + base64.b64encode(hashlib.sha256(base64.b64decode(b64)).digest()).decode('ascii').rstrip('=')


def team_id(pubkey):
    """Team 的编号：根公钥（'ssh-ed25519 AAAA…' 里 base64 那一段解出来的字节）的 SHA-256 的前 32 个十六进制字符"""
    return hashlib.sha256(base64.b64decode(pubkey.split()[1])).hexdigest()[:32]


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


def is_count(x):
    """从 1 起数的次数"""
    return isinstance(x, int) and not isinstance(x, bool) and x > 0


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


def name_ok(name):
    """设备的名字：1～64 个字符，没有控制字符，前后没有空白"""
    return isinstance(name, str) and 0 < len(name) <= 64 and name.strip() == name and not NAME_BAD.search(name)


def dev_ok(dev):
    """设备在 Team 里的编号"""
    return isinstance(dev, str) and bool(DEV_RE.match(dev))


def seq_ok(seq):
    """配置的序号：从 1 起，比 SEQ_MAX 小"""
    return is_count(seq) and seq < SEQ_MAX


def parse_enroll(devs):
    """team_enroll 的 devices：{设备编号: {"pub": 'ssh-ed25519 …', "name": 名字}}，一把公钥只能是一台设备；
    回 {设备编号: (指纹, 公钥, 名字)}，不合格是 None"""
    if not (isinstance(devs, dict) and 0 < len(devs) <= MAX_TEAM_BATCH):
        return None
    out = {}
    for dev, x in devs.items():
        k = KEY_RE.match(x['pub']) if isinstance(x, dict) and isinstance(x.get('pub'), str) else None
        if not (dev_ok(dev) and k and name_ok(x.get('name'))):
            return None
        out[dev] = (key_fp(k.group(1)), 'ssh-ed25519 ' + k.group(1), x['name'])
    return out if len({v[0] for v in out.values()}) == len(out) else None


def parse_team_docs(docs):
    """team_put 的 docs：{设备编号: {"seq": 序号, "ct": 密文, "sig": 签名}}（服务器不懂里面是什么，只查形状和长度）；不合格是 None"""
    if not (isinstance(docs, dict) and 0 < len(docs) <= MAX_TEAM_BATCH):
        return None
    out = {}
    for dev, x in docs.items():
        if not (dev_ok(dev) and isinstance(x, dict) and set(x) == {'seq', 'ct', 'sig'} and seq_ok(x['seq'])):
            return None
        ct, sig = parse_blob(x['ct'], MAX_TEAM_DOC), x['sig']
        if not (ct and isinstance(sig, str) and 0 < len(sig) <= MAX_TEAM_SIG and B64_RE.match(sig)):
            return None
        out[dev] = {'seq': x['seq'], 'ct': ct, 'sig': sig}
    return out


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
    "admit": 现在认的入伙口令的哈希, "check": 登记的盐的校验值 | null, "gen": 这一份记录的编号, "link": {"n": 换过几次入伙口令,
    "blob": 换的那台设备存的密文} | 没有}。
    keys.json（另一个文件：心跳不用跟着重写它）：{"acc": {账号摘要: {"blob": 密文, "fp": 传的设备, "name": 它那时的名字,
    "at": 什么时候传的}}}（撤销设备不删：是账号的）。cost-slots.json：{"acc": {账号摘要: {设备指纹: {"blob", "name", "at"}}}}
    （撤销设备不删，cost_keep 秒没再传的才删）。
    Team 的服务器（mode team）：machines 的每一台多 "joined" 和 "dev"（它在 Team 里的编号），另有 "team": {"root": 配置的根公钥,
    "gen": 这一份记录的编号, "invites": {令牌的 SHA-256: {"dev", "exp": 什么时候过期, "at"}}, "spent": {用掉的令牌的 SHA-256: 什么时候}}。
    team-docs.json：{"docs": {设备编号: {"seq", "ct": 密文, "sig": 签名, "at"}}}（管理员给各台设备的配置）"""
    def __init__(self, path, ttl, idle, retain, keys_path=None, cost_path=None, cost_keep=100 * 86400, mode='maintainer', admit_hash=None,
                 team_root=None):
        self.path, self.ttl, self.idle, self.retain, self.cost_keep, self.mode = path, ttl, idle, retain, cost_keep, mode
        self.seen = {}   # 自己的、Team 的服务器：各台设备最近一次验过签的请求（只在内存里：不为它每个请求都重写状态文件）
        self.have = {}   # Team 的服务器：各台设备取配置时说的手里那一份的序号（只在内存里）
        self.docs_path = os.path.join(os.path.dirname(path) or '.', 'team-docs.json')
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
        self.docs = {'v': 1, 'docs': {}}
        self.docs.update(load_doc(self.docs_path, ('docs',)) or {})
        self.docs['docs'] = {dev: e for dev, e in self.docs['docs'].items() if isinstance(e, dict) and is_count(e.get('seq'))
                             and parse_blob(e.get('ct'), MAX_TEAM_DOC) and isinstance(e.get('sig'), str)}
        if mode == 'own':
            self.own_setup(admit_hash)
        elif mode == 'team':
            self.team_setup(team_root)
        elif 'own' in self.data or 'team' in self.data:   # 以前是自己的、Team 的服务器：那时加入的设备不是维护者清单里的，不认
            sys.stderr.write('cca-presence: 状态文件是自己的、Team 的服务器的：%d 台设备不要了，等维护者同步设备清单\n' % len(self.data['machines']))
            self.reset()

    def reset(self, kind=None, entry=None):
        """从头开始：以前的设备、上报的记录、各台设备的配置都不要了。kind（'own' | 'team'）是这种服务器在状态文件里的那一项，
        entry 是它的内容，这时存的密文（API key、折合金额）也不要了；维护者的服务器（不带 kind）不动它们"""
        self.data.pop('own', None)
        self.data.pop('team', None)
        self.data.update({'seq': 0, 'machines': {}, 'beats': {}, 'units': {}})
        if kind:
            self.data[kind] = entry
            self.keys, self.cost = {'v': 1, 'acc': {}}, {'v': 1, 'acc': {}}
            save_doc(self.keys_path, self.keys)
            save_doc(self.cost_path, self.cost)
        self.save()
        if self.docs['docs'] or os.path.exists(self.docs_path):   # 没有就不写：别的模式的服务器旁边不留一个空的 team-docs.json
            self.docs = {'v': 1, 'docs': {}}
            save_doc(self.docs_path, self.docs)

    def own_setup(self, admit_hash):
        """自己的服务器：配置的入伙口令的哈希（seed）和上次的不一样（第一次启动、用新的加入链接重新部署了、以前是维护者的服务器）就从头
        开始：以前的设备、记录、密文都不要了（新的盐算出来的账号摘要和密钥都不一样，旧的谁也用不上）"""
        own = self.data.get('own')
        check = own.get('check') if isinstance(own, dict) else None
        if (isinstance(own, dict) and own.get('seed') == admit_hash and HASH_RE.match(own.get('admit') or '')
                and (check is None or (isinstance(check, str) and CHECK_RE.match(check)))):
            changed = False
            if not GEN_RE.match(str(own.get('gen') or '')):   # 以前的版本没有 gen
                own['gen'], changed = secrets.token_hex(16), True
            link = own.get('link')
            if 'link' in own and not (isinstance(link, dict) and is_count(link.get('n')) and parse_blob(link.get('blob'), MAX_LINK_BLOB)):
                own.pop('link')   # 坏了的不要：成员照旧用手里的链接
                changed = True
            if changed:
                self.save()
            return
        if self.data['machines'] or self.keys['acc'] or self.cost['acc'] or self.docs['docs']:
            why = '以前是 Team 的服务器' if 'team' in self.data else '入伙口令换了（用新的加入链接重新部署了）'
            sys.stderr.write('cca-presence: %s：以前的 %d 台设备和存的密文都不要了\n' % (why, len(self.data['machines'])))
        self.reset('own', {'seed': admit_hash, 'admit': admit_hash, 'check': None, 'gen': secrets.token_hex(16)})

    def team_setup(self, root):
        """Team 的服务器：配置的根公钥和上次的不一样（第一次启动、换了一个 Team、以前是别的模式）就从头开始：以前的设备、邀请、
        记录、密文都不要了（另一个 Team 的盐算出来的账号摘要和密钥都不一样，旧的谁也用不上）。接着用时，状态文件里样子不对的
        邀请、设备不要（手改坏了的：留着的话以后每个请求都出错）"""
        team = self.data.get('team')
        if isinstance(team, dict) and team.get('root') == root and GEN_RE.match(str(team.get('gen') or '')):
            entries = lambda k: team[k].items() if isinstance(team.get(k), dict) else ()
            before = json.dumps([team.get('invites'), team.get('spent'), self.data['machines']], sort_keys=True)
            team['invites'] = {h: i for h, i in entries('invites') if isinstance(i, dict) and is_time(i.get('exp')) and dev_ok(i.get('dev'))}
            team['spent'] = {h: t for h, t in entries('spent') if is_time(t)}
            self.data['machines'] = {f: m for f, m in self.data['machines'].items() if isinstance(m, dict) and dev_ok(m.get('dev'))
                                     and name_ok(m.get('name')) and isinstance(m.get('key'), str) and KEY_RE.match(m['key'])}
            if json.dumps([team['invites'], team['spent'], self.data['machines']], sort_keys=True) != before:
                self.save()
            self.docs_prune()
            return
        if self.data['machines'] or self.keys['acc'] or self.cost['acc'] or self.docs['docs']:
            sys.stderr.write('cca-presence: 换了一个 Team（根公钥不一样）：以前的 %d 台设备和存的密文都不要了\n' % len(self.data['machines']))
        self.reset('team', {'root': root, 'gen': secrets.token_hex(16), 'invites': {}, 'spent': {}})

    def save(self):
        save_doc(self.path, self.data)

    def machine(self, fp):
        with self.lock:
            return self.data['machines'].get(fp) if isinstance(fp, str) else None

    def gen(self):
        """自己的、Team 的服务器：这一份记录的编号"""
        with self.lock:
            return self.data['team' if self.mode == 'team' else 'own']['gen']

    def team_root(self):
        """Team 的服务器：配置的根公钥"""
        return self.data['team']['root']

    def root_fp(self):
        """Team 的服务器：根公钥的指纹（管理员的请求的 key；它不是设备，登记、加入都不收）"""
        return key_fp(self.team_root().split()[1])

    def name(self, fp):
        """设备清单里这台设备的名字（没有是 None）"""
        m = self.machine(fp)
        return m['name'] if m else None

    def fps(self):
        with self.lock:
            return set(self.data['machines'])

    def health(self, sig_ok, relay=None):
        with self.lock:
            extra = {'own': OWN_OPS, 'team': TEAM_OPS}.get(self.mode, ())
            out = {'v': API, 'ok': True, 'sig_ok': sig_ok, 'machines': len(self.data['machines']), 'seq': self.data['seq'], 'mode': self.mode,
                   'ops': ['report', 'view'] + list(SYNC_OPS) + list(extra) + (list(RELAY_OPS) if relay else [])}
            if self.mode == 'team':
                out['team'] = team_id(self.data['team']['root'])
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
        for i in range(1, len(taken) + 2):
            tail = '' if i == 1 else '-%d' % i
            cand = name[:64 - len(tail)] + tail
            if cand not in taken:
                return cand
        return name

    def join(self, fp, key, name, check, admit, now):
        """自己的服务器：加入（入伙口令、签名在 Handler 里验过）。盐的校验值第一台设备登记，以后的要一样；同一把密钥再加入是改名字。
        回的 gen 是这一份记录的编号（设备记下，以后不认它时据此分清是被移除了还是服务器的记录没了），seed：用的是不是部署时配置的
        那个口令（设备记下它：服务器的记录没了以后认的是它）"""
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
            seed = hmac.compare_digest(hashlib.sha256(admit.encode('ascii')).hexdigest(), own['seed'])
            return 200, {'v': API, 'ok': True, 'name': name, 'members': len(ms), 'gen': own['gen'], 'seed': seed}

    def member_row(self, f, m, now):
        """名单里的一台设备：什么时候加入的、最近一次请求（内存里记的、上报的心跳，取新的）。拿着锁调"""
        seen = max(self.seen.get(f) or 0, (self.data['beats'].get(f) or {}).get('seen') or 0)
        return {'fp': f, 'name': m['name'], 'joined_age': ago(now, m['joined']) if is_time(m.get('joined')) else None,
                'seen_age': ago(now, seen) if seen else None}

    def members(self, me, now):
        """自己的服务器：现在有哪些设备（最近一次请求：内存里记的、上报的心跳，取新的）"""
        with self.lock:
            rows = [dict(self.member_row(f, m, now), me=f == me) for f, m in self.data['machines'].items()]
            own = self.data['own']
            return 200, {'v': API, 'ok': True, 'me_name': self.data['machines'][me]['name'] if me in self.data['machines'] else None,
                         'members': rows, 'gen': own['gen'], 'link': own.get('link')}

    def relink(self, admit_hash, blob, now):
        """自己的服务器：换入伙口令（成员发的）。以后只认新的口令（旧的加入链接不能再用来加入），已经加入的设备照旧；blob 是换的那台
        设备拿盐加密的新链接（服务器解不开），members 回给成员，它们换上新的。n 每换一次加一"""
        with self.lock:
            own = self.data['own']
            n = (own.get('link') or {}).get('n', 0) + 1
            own['admit'], own['link'] = admit_hash, {'n': n, 'blob': blob, 'at': now}
            self.save()
        sys.stderr.write('cca-presence: 换了入伙口令（第 %d 次）\n' % n)
        return 200, {'v': API, 'ok': True, 'n': n}

    def remove(self, target):
        """自己的服务器：移除一台设备（它的密钥就此不认；是发请求的设备自己就是退出）。它的上报记录跟着删掉，传过的密文是账号的，
        不删（折合金额里标成撤销了）"""
        with self.lock:
            if target not in self.data['machines']:
                return 404, {'error': 'no_device'}
            self.drop_machine(target)
            self.save()
            n = len(self.data['machines'])
        sys.stderr.write('cca-presence: 移除了一台设备，现在 %d 台\n' % n)
        return 200, {'v': API, 'ok': True, 'members': n}

    def drop_machine(self, fp):
        """一台设备的密钥就此不认，它上报的记录跟着删掉（传过的 API key、折合金额的密文是账号的，不删）。拿着锁调"""
        self.data['machines'].pop(fp, None)
        self.data['beats'].pop(fp, None)
        self.data['units'].pop(fp, None)
        self.seen.pop(fp, None)
        self.have.pop(fp, None)

    # ---- Team 的服务器（mode team；格式见 README.md「Team 的同步服务器」）。管理员的请求（根密钥签的）、设备加入时的签名都在 Handler 里验过

    def team_devs(self):
        """认得的设备编号：加入了的、有邀请的（过期了、还没清掉的也算）。拿着锁调"""
        return {m.get('dev') for m in self.data['machines'].values()} | {i['dev'] for i in self.data['team']['invites'].values()}

    def docs_prune(self):
        """不认得的编号的配置不要了（邀请过期很久被清掉了；移除设备时写了状态文件、还没写这一份就停了）。拿着锁调（启动时不用）"""
        known = self.team_devs()
        gone = [dev for dev in self.docs['docs'] if dev not in known]
        for dev in gone:
            del self.docs['docs'][dev]
        if gone:
            save_doc(self.docs_path, self.docs)

    def invite_prune(self, now):
        """过期了、用掉了很久的邀请不记了，给它们存的配置跟着删掉。拿着锁调"""
        team = self.data['team']
        inv = {h: i for h, i in team['invites'].items() if now - i['exp'] < INVITE_KEEP}
        spent = {h: t for h, t in team['spent'].items() if now - t < INVITE_KEEP}
        if len(inv) != len(team['invites']) or len(spent) != len(team['spent']):
            team['invites'], team['spent'] = inv, spent
            self.save()
            self.docs_prune()

    def invite_state(self, token, now):
        """邀请里的令牌：'ok' | 'expired' | 'used'；没有这个邀请（或者不是令牌的样子）是 None"""
        if not (isinstance(token, str) and ADMIT_RE.match(token)):
            return None
        h = hashlib.sha256(token.encode('ascii')).hexdigest()
        with self.lock:
            team = self.data['team']
            inv = team['invites'].get(h)
            if inv:
                return 'ok' if now < inv['exp'] else 'expired'
            return 'used' if h in team['spent'] else None

    def team_enroll(self, devs, now):
        """管理员直接登记设备的公钥（它自己的那台；服务器的记录没了以后把各台加回去）：devs 是 parse_enroll 的结果。这个编号原来的
        那把密钥就此不认（换了密钥），还没用掉的邀请作废；公钥已经是别的编号的设备是 409"""
        root = self.root_fp()
        if any(fp == root for fp, _, _ in devs.values()):   # 根密钥不是设备
            return 400, {'error': 'bad'}
        with self.lock:
            ms, team = self.data['machines'], self.data['team']
            if any(fp in ms and ms[fp].get('dev') != dev for dev, (fp, _, _) in devs.items()):
                return 409, {'error': 'taken'}
            if len({m.get('dev') for m in ms.values()} | set(devs)) > MAX_TEAM_MEMBERS:
                return 507, {'error': 'full'}
            names = {}
            for dev, (fp, key, name) in devs.items():
                for f in [f for f, m in ms.items() if m.get('dev') == dev and f != fp]:
                    self.drop_machine(f)
                old = ms.get(fp) or {}
                names[dev] = self.free_name(name, fp)
                ms[fp] = {'name': names[dev], 'key': key, 'joined': old['joined'] if is_time(old.get('joined')) else now, 'dev': dev}
            team['invites'] = {h: i for h, i in team['invites'].items() if i['dev'] not in devs}
            self.save()
            return 200, {'v': API, 'ok': True, 'names': names, 'members': len(ms), 'gen': team['gen']}

    def team_invite(self, token_hash, dev, ttl, now):
        """管理员发一个邀请：服务器只存令牌的 SHA-256，ttl 秒后过期，只能用一次。同一个编号再发，前一个作废；这个编号的设备已经
        加入了是 409（先移除它）"""
        with self.lock:
            ms, team = self.data['machines'], self.data['team']
            self.invite_prune(now)
            if any(m.get('dev') == dev for m in ms.values()):
                return 409, {'error': 'taken'}
            if token_hash in team['spent']:   # 用过的令牌不能再发一次
                return 409, {'error': 'used'}
            inv = {h: i for h, i in team['invites'].items() if i['dev'] != dev}
            if sum(1 for i in inv.values() if now < i['exp']) >= MAX_INVITES:
                return 507, {'error': 'full'}
            inv[token_hash] = {'dev': dev, 'exp': now + ttl, 'at': now}
            team['invites'] = inv
            self.save()
            return 200, {'v': API, 'ok': True, 'invites': sum(1 for i in inv.values() if now < i['exp']), 'gen': team['gen']}

    def team_join(self, fp, key, name, token, now, proof=None):
        """设备拿邀请里的令牌加入：令牌就此用掉，这把密钥记成邀请里的那个编号。proof 是设备拿邀请里那串随机数给自己的公钥算的认证码
        （服务器不懂，原样存着、回给管理员：管理员据此知道这把公钥真是拿着邀请的那台设备的，不是服务器换的）"""
        h = hashlib.sha256(token.encode('ascii')).hexdigest()
        with self.lock:
            ms, team = self.data['machines'], self.data['team']
            inv = team['invites'].get(h)
            if not inv or now >= inv['exp']:   # 验签之后、拿到锁之前被别的设备用掉了、过期了
                return 403, {'error': 'expired' if inv else 'used'}
            if fp in ms:   # 这把密钥已经是一台设备了（管理员登记的、用过别的邀请）：先让管理员移除它
                return 409, {'error': 'taken'}
            if len(ms) >= MAX_TEAM_MEMBERS:
                return 507, {'error': 'full'}
            name = self.free_name(name, fp)
            ms[fp] = {'name': name, 'key': key, 'joined': now, 'dev': inv['dev']}
            if proof:
                ms[fp]['proof'] = proof
            del team['invites'][h]
            team['spent'][h] = now
            self.seen[fp] = now
            self.save()
            sys.stderr.write('cca-presence: 加入了一台设备，现在 %d 台\n' % len(ms))
            return 200, {'v': API, 'ok': True, 'name': name, 'dev': inv['dev'], 'members': len(ms), 'gen': team['gen']}

    def team_put(self, docs, now):
        """管理员存各台设备的配置（parse_team_docs 的结果）：序号要比存着的大（一样、认证码也一样的算存过了：没收到回复再发一次）；
        不认得的编号（被移除了、邀请过期很久了）、序号不比存着的大的不存，回给管理员"""
        with self.lock:
            self.invite_prune(now)
            cur, known = self.docs['docs'], self.team_devs()
            stored, stale, unknown = [], {}, []
            for dev, d in docs.items():
                have = cur.get(dev)
                if dev not in known:
                    unknown.append(dev)
                elif have and (d['seq'] < have['seq'] or (d['seq'] == have['seq'] and d['ct']['t'] != have['ct']['t'])):
                    stale[dev] = have['seq']
                else:
                    stored.append(dev)
            size = lambda e: len(e['ct']['c'])
            if sum(size(e) for dev, e in cur.items() if dev not in stored) + sum(size(docs[dev]) for dev in stored) > TEAM_DOCS_BYTES:
                return 507, {'error': 'full'}
            for dev in stored:
                cur[dev] = dict(docs[dev], at=now)
            if stored:
                save_doc(self.docs_path, self.docs)
            return 200, {'v': API, 'ok': True, 'stored': stored, 'stale': stale, 'unknown': unknown, 'gen': self.data['team']['gen']}

    def team_get(self, fp, have):
        """设备取自己的那一份配置：have 是它手里那一份的序号（记下来，管理员看名单时看得到各台用上了没有），一样就不带（doc 是 null）；
        seq 是存着的序号（还没有是 0）"""
        with self.lock:
            m = self.data['machines'].get(fp)
            if not m:   # 验签之后、拿到锁之前被移除了
                return 403, {'error': 'unknown_device', 'gen': self.data['team']['gen']}
            self.have[fp] = have
            e = self.docs['docs'].get(m.get('dev'))
            doc = {k: e[k] for k in ('seq', 'ct', 'sig')} if e and e['seq'] != have else None
            return 200, {'v': API, 'ok': True, 'dev': m.get('dev'), 'me_name': m['name'], 'seq': e['seq'] if e else 0, 'doc': doc,
                         'gen': self.data['team']['gen']}

    def team_members(self, now):
        """管理员看现在有哪些设备（公钥也带着：服务器的记录没了以后照它登记回去）、它们各自存着的配置的序号（seq）和设备取配置时
        说的手里那一份的序号（have：重启以后、还没取过是 null）、还没用掉的邀请"""
        with self.lock:
            self.invite_prune(now)
            team, docs = self.data['team'], self.docs['docs']
            seq = lambda dev: (docs.get(dev) or {}).get('seq', 0)
            rows = [dict(self.member_row(f, m, now), dev=m.get('dev'), pub=m['key'], proof=m.get('proof'), seq=seq(m.get('dev')),
                         have=self.have.get(f))
                    for f, m in self.data['machines'].items()]
            invites = [{'dev': i['dev'], 'expires_in': round(i['exp'] - now), 'seq': seq(i['dev'])} for i in team['invites'].values() if now < i['exp']]
            return 200, {'v': API, 'ok': True, 'members': rows, 'invites': invites, 'gen': team['gen']}

    def team_revoke(self, dev):
        """管理员移除一台设备：它的密钥就此不认，给它的配置、还没用掉的邀请一起删掉"""
        with self.lock:
            team = self.data['team']
            fps = [f for f, m in self.data['machines'].items() if m.get('dev') == dev]
            pending = [h for h, i in team['invites'].items() if i['dev'] == dev]
            if not fps and not pending and dev not in self.docs['docs']:
                return 404, {'error': 'no_device'}
            for f in fps:
                self.drop_machine(f)
            for h in pending:
                del team['invites'][h]
            self.save()
            self.docs_prune()
            n = len(self.data['machines'])
        if fps:
            sys.stderr.write('cca-presence: 移除了一台设备，现在 %d 台\n' % n)
        return 200, {'v': API, 'ok': True, 'members': n}

    def team_leave(self, fp):
        """设备自己退出：和被管理员移除一样"""
        with self.lock:
            dev = (self.data['machines'].get(fp) or {}).get('dev')
        if not dev:   # 验签之后、拿到锁之前被移除了
            return 403, {'error': 'unknown_device', 'gen': self.gen()}
        return self.team_revoke(dev)

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
# 访客（手机上的网页端：不在设备清单里，是和它配对的那台设备领进来的）能发的：输入配对码、看配对的设备上有哪些会话、接入。
# 开配对（pair_open）、当宿主（relay_host）和中转以外的请求都不行
GUEST_OPS = ('pair_put', 'pair_get', 'pair_commit', 'pair_close', 'pair_drop', 'relay_attach', 'relay_list')
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
WS_CONN = 64               # 网页端同时几条 WebSocket
WS_CONN_IP = 8
WS_FRAME = 1 << 20         # WebSocket 一帧最长（里面是加密的记录，一条不到 17 KB；浏览器可能把攒着的几条并成一帧）
WS_STALL = 20              # 一头这么久写不动、一帧这么久收不齐：断开
WS_IDLE = 100              # 两头这么久都没有动静：断开（比 RELAY_IDLE 长：闲着的由中转那头先断）
WS_GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11'
WS_KEY_RE = re.compile(r'^[A-Za-z0-9+/]{22}==$')
WEB_URL_RE = re.compile(r'^(https://[A-Za-z0-9.-]+(?::\d{1,5})?|http://(?:localhost|127\.0\.0\.1)(?::\d{1,5})?)/?$')
WEB_NAME_RE = re.compile(r'^[a-z0-9][a-z0-9._-]{0,63}$')
WEB_TYPES = {'.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8',
             '.png': 'image/png', '.svg': 'image/svg+xml', '.webmanifest': 'application/manifest+json'}
WEB_FILE = 2 << 20         # 网页的一个文件最长
WAKE = object()            # 中转的 selector 里“别的线程交了连接过来”的那一项
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
    def __init__(self, path, port, web=None):
        """web：网页端的地址（开着网页端时）。给了才让设备开“给网页端的配对”、才有访客"""
        self.path, self.port, self.web = path, port, web
        self.lock = threading.RLock()
        self.cond = threading.Condition(self.lock)
        self.doc = {'v': 1, 'edges': {}, 'guests': {}}
        self.doc.update(load_doc(path, ('edges',)) or {})
        self.doc['edges'] = {p: e for p, e in self.doc['edges'].items()
                             if isinstance(p, str) and AID_RE.match(p) and isinstance(e, dict)
                             and all(isinstance(e.get(k), str) and FP_RE.match(e[k]) for k in ('a', 'b'))}
        g = self.doc.get('guests')
        self.doc['guests'] = {fp: {'key': x['key'], 'at': x.get('at')} for fp, x in (g.items() if isinstance(g, dict) else ())
                              if isinstance(fp, str) and FP_RE.match(fp) and isinstance(x, dict) and isinstance(x.get('key'), str)
                              and KEY_RE.match(x['key']) and key_fp(x['key'].split()[1]) == fp}
        self.slots, self.tickets, self.sessions = {}, {}, {}
        self.prune_guests()

    @property
    def edges(self):
        return self.doc['edges']

    @property
    def guests(self):
        """访客（remote.json 的 guests）：{指纹: {"key": 公钥, "at"}}。手机上的网页端没有 deploy key、不在设备清单里：它拿着一台设备
        显示的配对码配上对，服务器才记下它的公钥；只能发 GUEST_OPS，一个配对都不剩时就删掉"""
        return self.doc['guests']

    def prune_guests(self):
        """一个配对都不剩的访客不留（它的票据也作废）。有删掉的回 True"""
        used = set()
        for e in self.edges.values():
            used.update((e['a'], e['b']))
        gone = [fp for fp in self.guests if fp not in used]
        for fp in gone:
            self.guests.pop(fp)
        if gone:
            self.tickets = {t: k for t, k in self.tickets.items() if k['fp'] not in gone}
        return bool(gone)

    def guest_key(self, fp, op, j, now):
        """不在设备清单里的这把密钥能不能发这个请求：能的话回验签名用的公钥，不能是 None。两种：已经配了对的访客；
        正拿着配对码来的——那次配对是开给网页端的（pair_open 带 web）、还没有别的设备放过密文，公钥在请求里（pub，指纹要对得上）。
        知道 rid 就是知道配对码：不知道的人到不了验签名那一步"""
        if op not in GUEST_OPS:
            return None
        with self.lock:
            g = self.guests.get(fp)
            if g:
                return g['key']
            rid = j.get('rid')
            s = self.slots.get(rid) if isinstance(rid, str) else None
            if not s or not s['web'] or s['exp'] < now:
                return None
            if s['joiner'] == fp:
                return s['gkey']
            pub = j.get('pub')
            k = KEY_RE.match(pub) if isinstance(pub, str) else None
            if op == 'pair_put' and s['joiner'] is None and k and key_fp(k.group(1)) == fp:
                return 'ssh-ed25519 ' + k.group(1)
        return None

    def save(self):
        save_doc(self.path, self.doc)

    def member(self, pid, fp):
        e = self.edges.get(pid) if isinstance(pid, str) else None
        return bool(e) and fp in (e['a'], e['b'])

    def other(self, pid, fp):
        e = self.edges[pid]
        return e['b'] if e['a'] == fp else e['a']

    def keep_only(self, fps):
        """设备清单换了：撤销的设备的配对、票据、会话都删掉（它的控制连接由数据端口那边发现会话没了时断开）。
        访客不在清单里：领它进来的设备撤销了，它的配对跟着没有，它也就删掉了"""
        with self.lock:
            ok = set(fps) | set(self.guests)
            gone = [p for p, e in self.edges.items() if e['a'] not in ok or e['b'] not in ok]
            for p in gone:
                self.drop_edge(p)
            self.tickets = {t: k for t, k in self.tickets.items() if k['fp'] in fps or k['fp'] in self.guests}
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
        self.prune_guests()

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

    def op(self, op, fp, j, now, name_of, gkey=None):
        """签名、时间、nonce 都验过了的请求。返回 (状态码, 回的内容)。gkey：发请求的是访客（不在设备清单里），这是验它签名用的公钥"""
        hex32 = lambda x: isinstance(x, str) and bool(AID_RE.match(x))
        if gkey and op not in GUEST_OPS:
            return 403, {'error': 'denied'}
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
                    web = j.get('web') is True   # 这次配对开给网页端：输入配对码的可以是不在设备清单里的（手机上的那一页）
                    if web and not self.web:
                        return 404, {'error': 'no_web'}
                    self.slots[rid] = {'owner': fp, 'joiner': None, 'msgs': {}, 'commit': {}, 'done': False, 'exp': now + PAIR_TTL,
                                       'web': web, 'gkey': None}
                    return 200, dict({'v': API, 'ok': True, 'ttl': PAIR_TTL}, **({'web': self.web} if web else {}))
                if s is None:
                    return 404, {'error': 'no_pairing'}
                if gkey and not s['web']:   # 访客只进得了开给网页端的配对
                    return 403, {'error': 'denied'}
                if fp != s['owner'] and s['joiner'] is None and op == 'pair_put':
                    s['joiner'], s['gkey'] = fp, gkey   # 头一个放密文进来的别的设备（是访客的话记下它的公钥：配成了才留下）
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
                        if s['gkey']:   # 输入配对码的是访客：两边算出了同一把密钥（它确实知道配对码），这才记下它
                            self.guests[s['joiner']] = {'key': s['gkey'], 'at': now}
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
        # 网页端的连接不是从数据端口进来的：处理 WebSocket 的线程把 socketpair 的一头交过来（adopt），这条线程被叫醒后收下
        self.handed = collections.deque()
        self.wake_r, self.wake_w = socket.socketpair()
        self.wake_r.setblocking(False)
        self.wake_w.setblocking(False)

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
        self.admit(sock, addr[0], now)

    def adopt(self, sock, ip):
        """别的线程交过来一条已经连好的连接（网页端的 WebSocket 转成的 socketpair 的一头；ip 是浏览器的地址）：
        之后和数据端口上接进来的一样——先说开场白，同样的限额"""
        self.handed.append((sock, ip))
        try:
            self.wake_w.send(b'\0')
        except OSError:   # 叫醒用的那一对满了：已经有没读走的，照样会醒
            pass

    def admit(self, sock, ip, now):
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
        self.sel.register(self.wake_r, selectors.EVENT_READ, WAKE)
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
                    if c is WAKE:
                        try:
                            self.wake_r.recv(4096)
                        except OSError:
                            pass
                        while self.handed:
                            sock, ip = self.handed.popleft()
                            self.admit(sock, ip, now)
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


class Web:
    """网页端（手机浏览器里的那一页，README「网页端」）：页面的文件（启动时读进内存）、页面的地址、WebSocket 交给哪个中转"""
    def __init__(self, url, root, relay):
        self.url = url.rstrip('/') + '/'
        self.origin = self.url[:-1]
        self.relay = relay
        host = self.origin.split('://', 1)[1]
        # 页面只连自己这一个地址：脚本、样式、图片、请求、WebSocket 都只认同一个源（xterm.js 画界面要动态加样式，所以样式放行内联的）
        self.headers = (
            ('Content-Security-Policy', "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; "
             "font-src 'self'; manifest-src 'self'; connect-src 'self' %s://%s; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
             % ('wss' if self.origin.startswith('https:') else 'ws', host)),
            ('X-Content-Type-Options', 'nosniff'), ('X-Frame-Options', 'DENY'), ('Referrer-Policy', 'no-referrer'),
            ('Cross-Origin-Opener-Policy', 'same-origin'), ('Strict-Transport-Security', 'max-age=31536000'))
        self.files = {}
        for name in sorted(os.listdir(root)):
            ctype = WEB_TYPES.get(os.path.splitext(name)[1])
            path = os.path.join(root, name)
            if not (ctype and WEB_NAME_RE.match(name) and os.path.isfile(path)):
                continue   # 说明、校验和这些不是页面的一部分：不给
            with open(path, 'rb') as f:
                raw = f.read(WEB_FILE + 1)
            if len(raw) > WEB_FILE:
                raise ValueError('%s 太大' % path)
            gz = gzip.compress(raw, 9) if not name.endswith('.png') else None
            self.files[name] = (ctype, raw, gz if gz and len(gz) < len(raw) else None, '"%s"' % hashlib.sha256(raw).hexdigest()[:32])
        if 'index.html' not in self.files:
            raise ValueError('%s 里没有 index.html' % root)
        self.lock, self.conns, self.per_ip = threading.Lock(), 0, {}

    def take(self, ip):
        with self.lock:
            if self.conns >= WS_CONN or self.per_ip.get(ip, 0) >= WS_CONN_IP:
                return False
            self.conns += 1
            self.per_ip[ip] = self.per_ip.get(ip, 0) + 1
            return True

    def free(self, ip):
        with self.lock:
            self.conns -= 1
            n = self.per_ip.get(ip, 1) - 1
            if n > 0:
                self.per_ip[ip] = n
            else:
                self.per_ip.pop(ip, None)


def tls_context(cert, key):
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    if hasattr(ssl, 'TLSVersion'):
        ctx.minimum_version = ssl.TLSVersion.TLSv1_2
    else:   # Python 3.6（CentOS 7、一些云厂商的系统镜像里的 python3）
        ctx.options |= ssl.OP_NO_TLSv1 | ssl.OP_NO_TLSv1_1
    ctx.load_cert_chain(cert, key)
    return ctx


class CertCtx:
    """浏览器信得过的那张证书（--web-cert）：证书机构签的会到期、会被续上（90 天一换，给 IP 地址签的 6 天一换），
    所以文件变了就重新读（最多一分钟看一次），不用重启；新的读不了时接着用旧的、在日志里说一声"""
    def __init__(self, cert, key):
        self.cert, self.key, self.lock = cert, key, threading.Lock()
        self.checked, self.warned = time.monotonic(), None
        self.seen = self.stamp()
        self.ctx = tls_context(cert, key)

    def stamp(self):
        return tuple((s.st_mtime, s.st_size) for s in (os.stat(self.cert), os.stat(self.key)))

    def get(self):
        with self.lock:
            now = time.monotonic()
            if now - self.checked >= 60:
                self.checked = now
                try:
                    stamp = self.stamp()
                    if stamp != self.seen:
                        self.ctx, self.seen, self.warned = tls_context(self.cert, self.key), stamp, None
                        sys.stderr.write('cca-presence: 网页端的证书换了，已经重新读\n')
                        sys.stderr.flush()
                except (OSError, ssl.SSLError, ValueError) as e:
                    if self.warned != str(e):   # 续证书的程序正写到一半：下一分钟再看；同一个错只说一次
                        self.warned = str(e)
                        sys.stderr.write('cca-presence: 网页端的新证书读不了（%s），接着用原来的\n' % e)
                        sys.stderr.flush()
            return self.ctx


def ws_frame(op, data):
    """服务器发的一帧（不加掩码）"""
    n = len(data)
    head = bytes([0x80 | op]) + (bytes([n]) if n < 126 else b'\x7e' + n.to_bytes(2, 'big') if n < 65536 else b'\x7f' + n.to_bytes(8, 'big'))
    return head + data


def ws_frames(buf):
    """buf（bytearray）开头收齐了的帧一个个取出来：(操作码, 内容)；还不够一帧就停。不合规矩的抛 ValueError：
    用了扩展位、没加掩码（浏览器发的一定加）、太长"""
    while len(buf) >= 2:
        b0, b1 = buf[0], buf[1]
        if b0 & 0x70 or not b1 & 0x80:
            raise ValueError('frame')
        n, at = b1 & 0x7f, 2
        if n == 126:
            if len(buf) < 4:
                return
            n, at = int.from_bytes(buf[2:4], 'big'), 4
        elif n == 127:
            if len(buf) < 10:
                return
            n, at = int.from_bytes(buf[2:10], 'big'), 10
        if n > WS_FRAME or (b0 & 0x08 and n > 125):
            raise ValueError('long')
        if len(buf) < at + 4 + n:
            return
        mask, data = bytes(buf[at:at + 4]), bytes(buf[at + 4:at + 4 + n])
        del buf[:at + 4 + n]
        if n:   # 去掩码：一个字节一个字节异或太慢，当成两个大整数异或
            data = (int.from_bytes(data, 'big') ^ int.from_bytes((mask * (n // 4 + 1))[:n], 'big')).to_bytes(n, 'big')
        yield b0 & 0x0f, data


def ws_pump(conn, early, relay, ip):
    """网页端的一条 WebSocket。浏览器开不了普通的 TCP 连接，所以它把本该发到数据端口的字节装在二进制帧里发过来：这里拆出来，
    当成一条字节流交给中转（socketpair 的一头交给中转的线程，和数据端口上接进来的连接一样对待），中转回的装成帧发回去。
    里面照旧是开场白和两头之间的密文，这里一个字节都不看。early：握手时多读进来的"""
    a, b = socket.socketpair()
    try:
        relay.adopt(a, ip)
        conn.settimeout(WS_STALL)
        b.settimeout(WS_STALL)
        buf = bytearray(early)
        while True:
            for op, data in ws_frames(buf):
                if op in (0, 2):   # 二进制的（和它的后续帧）：内容接起来就是那条字节流
                    if data:
                        b.sendall(data)
                elif op == 9:
                    conn.sendall(ws_frame(10, data))
                elif op == 8:
                    return
                elif op != 10:   # 文本的、不认识的
                    raise ValueError('op')
            pending = bool(getattr(conn, 'pending', lambda: 0)())   # TLS 已经解出来、还没取走的：select 看不到
            ready = select.select([conn, b], [], [], 0 if pending else WS_IDLE)[0]
            if not ready and not pending:
                return
            if b in ready:
                data = b.recv(65536)
                if not data:   # 中转那头关了（票据不对、会话没了、对方断开、闲得太久）
                    return
                conn.sendall(ws_frame(2, data))
            if pending or conn in ready:
                data = conn.recv(65536)
                if not data:
                    return
                buf += data
    except (OSError, ValueError):
        pass
    finally:
        try:
            conn.sendall(ws_frame(8, b''))
        except (OSError, ValueError):
            pass
        b.close()


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
        srv = self.server
        path = self.path.split('?', 1)[0]
        if path == '/v1/health':
            health = srv.store.health(srv.sig_ok, srv.remote and srv.remote.port)
            if srv.web:
                health['web'] = srv.web.url
            return self.send(200, health)
        if srv.web and path == '/v1/relay/ws':
            return self.websocket(srv.web)
        f = srv.web and srv.web.files.get('index.html' if path == '/' else path[1:])
        if f:
            return self.static(srv.web, f)
        return self.send(404, {'error': 'not_found'})

    def static(self, web, f):
        """网页端的一个文件。每次都问一下变了没有（no-cache + ETag）：页面更新了手机上马上就是新的"""
        if not self.server.ip_bucket.take(self.client_ip()):
            return self.send(429, {'error': 'rate'})
        ctype, raw, gz, etag = f
        same = self.headers.get('If-None-Match') == etag
        body = b'' if same else gz if gz and 'gzip' in (self.headers.get('Accept-Encoding') or '') else raw
        self.send_response(304 if same else 200)
        if not same:
            self.send_header('Content-Type', ctype)
            self.send_header('Content-Length', str(len(body)))
            if body is gz:
                self.send_header('Content-Encoding', 'gzip')
        self.send_header('ETag', etag)
        self.send_header('Cache-Control', 'no-cache')
        self.send_header('Vary', 'Accept-Encoding')
        for k, v in web.headers:
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)

    def websocket(self, web):
        """GET /v1/relay/ws：网页端进数据端口的路（见 ws_pump）。只认自己那一页发起的（Origin）；进得了中转靠的还是开场白里的票据"""
        h, srv = self.headers, self.server
        key = h.get('Sec-WebSocket-Key') or ''
        if (h.get('Upgrade') or '').lower() != 'websocket' or 'upgrade' not in (h.get('Connection') or '').lower() \
                or h.get('Sec-WebSocket-Version') != '13' or not WS_KEY_RE.match(key):
            return self.send(400, {'error': 'bad'})
        if h.get('Origin') not in (None, web.origin):
            return self.send(403, {'error': 'denied'})
        ip = self.client_ip()
        if not srv.ip_bucket.take(ip):
            return self.send(429, {'error': 'rate'})
        if not web.take(ip):
            return self.send(429, {'error': 'busy'})
        try:
            accept = base64.b64encode(hashlib.sha1((key + WS_GUID).encode()).digest()).decode()
            self.wfile.write(('HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n'
                              'Sec-WebSocket-Accept: %s\r\n\r\n' % accept).encode())
            self.wfile.flush()
            srv.long_lived(self.client_address[0])   # 这条连接要开很久：不受 15 秒的期限管，也不占处理请求的名额
            # 读请求头时可能多读了几个字节进缓冲区（浏览器等到回了 101 才发帧，一般没有）：取出来
            self.connection.settimeout(0)
            try:
                early = self.rfile.peek()
            except (OSError, ValueError):
                early = b''
            ws_pump(self.connection, early or b'', web.relay, ip)
        except OSError:
            pass
        finally:
            web.free(ip)
            self.close_connection = True

    def client_ip(self):
        """按哪个 IP 限流：--behind-proxy 时是 X-Forwarded-For 最后一项（平台的代理加的；前面的是客户端自己写的，能乱填），
        没有、不像 IP 时和平时一样是连过来的地址"""
        if self.server.proxied:
            last = ','.join(self.headers.get_all('X-Forwarded-For') or []).split(',')[-1].strip()
            try:
                return str(ipaddress.ip_address(last))
            except ValueError:
                pass
        return self.client_address[0]

    def do_POST(self):
        srv = self.server
        if not srv.ip_bucket.take(self.client_ip()):
            return self.send(429, {'error': 'rate'})
        try:
            n = int(self.headers.get('Content-Length') or '')
        except ValueError:
            return self.send(411, {'error': 'bad'})
        op = self.path[len('/v1/'):] if self.path.startswith('/v1/') else ''
        big = op in SYNC_OPS or (op == 'team_put' and srv.store.mode == 'team')   # 交换密文的、管理员存各台设备的配置的
        if n < 0 or n > (MAX_SYNC_BODY if big else MAX_BODY):
            return self.send(413, {'error': 'bad'})
        body = self.rfile.read(n)
        try:
            sig = base64.b64decode(self.headers.get('X-Cca-Sig') or '', validate=True)
        except ValueError:
            sig = b''
        if len(body) != n or not sig or len(sig) > MAX_SIG:
            return self.send(400, {'error': 'bad'})
        now = time.time()
        own, team = srv.store.mode == 'own', srv.store.mode == 'team'
        if op == 'machines' and not (own or team):   # 自己的、Team 的服务器没有维护者的设备清单：设备自己加入（404）
            code, reply = srv.store.set_machines(body, sig, srv.release_key)
            if code == 200 and srv.remote:
                srv.remote.keep_only(srv.store.fps())
            return self.send(code, reply)
        relay = op in RELAY_OPS and srv.remote is not None   # 没开中转时和旧版服务器一样：不认这几个 op（404）
        if op not in ('report', 'view') and op not in SYNC_OPS and not relay and not (own and op in OWN_OPS) and not (team and op in TEAM_OPS):
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
        gkey = None
        if op == 'join':
            # 加入：拿要加入的这把密钥签名。入伙口令不对和签名不对回一样的 403（不留判断口）；口令先比（便宜），对了才验签
            pub = j.get('pub')
            k = KEY_RE.match(pub) if isinstance(pub, str) else None
            if not (k and key_fp(k.group(1)) == fp and srv.store.admit_ok(j.get('admit'))
                    and ssh_verify('ssh-ed25519 ' + k.group(1), NS_REPORT, body, sig)):
                return self.send(403, {'error': 'denied'})
        elif op == 'team_join':
            # 拿邀请加入：和 join 一样拿要加入的这把密钥签名；没有这个邀请和签名不对回一样的 403。签名对了、邀请过期了或者用过了才说
            # 是哪一种（拿着真的邀请的人才问得出来）
            pub = j.get('pub')
            k = KEY_RE.match(pub) if isinstance(pub, str) else None
            state = srv.store.invite_state(j.get('token'), now) if k and key_fp(k.group(1)) == fp != srv.store.root_fp() else None
            if not (state and ssh_verify('ssh-ed25519 ' + k.group(1), NS_REPORT, body, sig)):
                return self.send(403, {'error': 'denied'})
            if state != 'ok':
                return self.send(403, {'error': state})
        elif op in TEAM_ADMIN_OPS:   # 管理员的：拿 Team 的根密钥签名（key 是根公钥的指纹）
            if fp != srv.store.root_fp() or not ssh_verify(srv.store.team_root(), NS_REPORT, body, sig):
                return self.send(403, {'error': 'denied'})
        else:
            m = srv.store.machine(fp)
            # 不在设备清单里、发的是中转的请求：可能是访客（手机上的网页端，和哪台设备配了对、或者正拿着配对码来配对）
            gkey = srv.remote.guest_key(fp, op, j, now) if relay and not m else None
            if not m and not gkey:   # 不在设备清单里（维护者还没同步、撤销了；自己的、Team 的服务器上是被移除了、记录清掉了：设备看 gen 分清是哪种）
                return self.send(403, {'error': 'unknown_device', 'gen': srv.store.gen()} if own or team else {'error': 'denied'})
            if not ssh_verify(m['key'] if m else gkey, NS_REPORT, body, sig):
                return self.send(403, {'error': 'denied'})
        bucket = srv.root_bucket if team and op in TEAM_ADMIN_OPS else srv.fp_bucket
        if not bucket.take(fp):   # 验签之后才扣：别人拿这台设备的指纹乱发，扣不到它的配额
            return self.send(429, {'error': 'rate'})
        ts = j.get('ts')
        if not isinstance(ts, (int, float)) or isinstance(ts, bool) or abs(ts - now) > TS_WINDOW:
            return self.send(400, {'error': 'clock', 'now': now})
        nonce = j.get('nonce')
        if not (isinstance(nonce, str) and NONCE_RE.match(nonce)):
            return self.send(400, {'error': 'bad'})
        if not srv.nonces.add(nonce, now):
            return self.send(409, {'error': 'replay'})
        if (own or team) and op not in ('join', 'team_join') + TEAM_ADMIN_OPS and not gkey:   # 设备的请求。加入的那一次在加入时记（没加入成的不记）；访客不是这里的设备
            srv.store.seen[fp] = now
        if op == 'join':
            name, check = j.get('name'), j.get('check')
            if not (name_ok(name) and isinstance(check, str) and CHECK_RE.match(check)):
                return self.send(400, {'error': 'bad'})
            return self.send(*srv.store.join(fp, 'ssh-ed25519 ' + k.group(1), name, check, j['admit'], now))
        if op == 'team_join':
            proof = j.get('proof')
            if not (name_ok(j.get('name')) and (proof is None or (isinstance(proof, str) and TAG_RE.match(proof)))):
                return self.send(400, {'error': 'bad'})
            return self.send(*srv.store.team_join(fp, 'ssh-ed25519 ' + k.group(1), j['name'], j['token'], now, proof))
        if op in TEAM_OPS:
            return self.team_op(op, fp, j, now)
        if op == 'members':
            return self.send(*srv.store.members(fp, now))
        if op == 'relink':
            admit, blob = j.get('admit'), parse_blob(j.get('blob'), MAX_LINK_BLOB)
            if not (isinstance(admit, str) and HASH_RE.match(admit) and blob):
                return self.send(400, {'error': 'bad'})
            return self.send(*srv.store.relink(admit, blob, now))
        if op == 'remove':
            target = j.get('target')
            if not (isinstance(target, str) and FP_RE.match(target)):
                return self.send(400, {'error': 'bad'})
            code, reply = srv.store.remove(target)
            if code == 200 and srv.remote:
                srv.remote.keep_only(srv.store.fps())
            return self.send(code, reply)
        if relay:
            return self.send(*srv.remote.op(op, fp, j, now, srv.store.name, gkey))
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

    def team_op(self, op, fp, j, now):
        """Team 的服务器上除了加入以外的 op（谁发的、签名在 do_POST 里验过）"""
        srv, st = self.server, self.server.store
        code, reply = 400, {'error': 'bad'}
        if op == 'team_members':
            code, reply = st.team_members(now)
        elif op == 'team_get':
            have = j.get('have')
            if have is None or seq_ok(have):
                code, reply = st.team_get(fp, have)
        elif op == 'team_invite':
            token, dev, ttl = j.get('token'), j.get('dev'), j.get('ttl')
            if (isinstance(token, str) and HASH_RE.match(token) and dev_ok(dev) and isinstance(ttl, int) and not isinstance(ttl, bool)
                    and INVITE_TTL[0] <= ttl <= INVITE_TTL[1]):
                code, reply = st.team_invite(token, dev, ttl, now)
        elif op == 'team_put':
            docs = parse_team_docs(j.get('docs'))
            if docs:
                code, reply = st.team_put(docs, now)
        elif op == 'team_enroll':
            devs = parse_enroll(j.get('devices'))
            if devs:
                code, reply = st.team_enroll(devs, now)
        elif op == 'team_revoke':
            if dev_ok(j.get('dev')):
                code, reply = st.team_revoke(j['dev'])
        elif op == 'team_leave':
            code, reply = st.team_leave(fp)
        if code == 200 and srv.remote and op in ('team_enroll', 'team_revoke', 'team_leave'):   # 有设备的密钥不再认了：它的配对、会话跟着清掉
            srv.remote.keep_only(st.fps())
        return self.send(code, reply)


class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):   # 就是 3.7 起的 http.server.ThreadingHTTPServer（3.6 上没有）
    daemon_threads = True
    allow_reuse_address = True
    request_queue_size = 64

    def __init__(self, addr, ctx, store, release_key, sig_ok, remote=None, web=None, share=None):
        """ctx 是 None：--behind-proxy（只说 HTTP，HTTPS 由前面的代理做）；是 CertCtx：网页端的那个监听（证书会换）。
        share：另一个监听（网页端的监听和原来的是同一个服务：限流、用过的 nonce 是同一份，不然同一个请求能在两个端口上各用一次）"""
        self.ctx, self.store, self.release_key, self.sig_ok, self.remote, self.web = ctx, store, release_key, sig_ok, remote, web
        self.proxied = ctx is None
        if share:
            self.ip_bucket, self.fp_bucket, self.nonces, self.root_bucket = share.ip_bucket, share.fp_bucket, share.nonces, share.root_bucket
        else:
            self.ip_bucket, self.fp_bucket, self.nonces = Buckets(2, 120), Buckets(0.5, 30), Nonces()
            self.root_bucket = Buckets(2, 120)   # Team 的管理员（根密钥签的）：一条命令要问两三次，设备的那个限额不够用
        self.slots = threading.BoundedSemaphore(MAX_CONN)
        self.per_ip, self.per_ip_lock = {}, threading.Lock()
        self.tl = threading.local()   # 处理这条连接的线程自己的：到期限断开它的定时器、名额还了没有
        super().__init__(addr, Handler)

    def long_lived(self, ip):
        """这条连接成了 WebSocket，要开很久：不再受 15 秒的期限管，处理请求的名额现在就还（WebSocket 有自己的上限，Web.take）"""
        self.tl.timer.cancel()
        self.free_slot(ip)
        self.tl.freed = True

    def take_slot(self, ip):
        """这个连接能不能处理：总数、同一个 IP 的数都没到上限（--behind-proxy 时只看总数：连过来的都是代理）"""
        with self.per_ip_lock:
            if self.per_ip.get(ip, 0) >= MAX_CONN_IP and not self.proxied:
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
            if not getattr(self.tl, 'freed', False):
                self.free_slot(client_address[0])

    def handle_error(self, request, client_address):   # 一行就够：不打印整段 traceback
        sys.stderr.write('cca-presence: 处理请求出错：%s\n' % (sys.exc_info()[1],))

    def finish_request(self, request, client_address):
        """TLS 握手放在处理这个连接的线程里：握手最多 5 秒，整个连接从接进来起最多 15 秒（到点从别的线程把它断掉，
        一个字节一个字节慢慢发也占不住）。--behind-proxy 时不握手，照样最多 15 秒"""
        def cut():
            try:
                request.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
        timer = self.tl.timer = threading.Timer(CONN_DEADLINE, cut)
        timer.daemon = True
        timer.start()
        try:
            request.settimeout(HANDSHAKE_TIMEOUT)
            if self.proxied:
                self.RequestHandlerClass(request, client_address, self)
                return
            try:
                ctx = self.ctx.get() if isinstance(self.ctx, CertCtx) else self.ctx
                tls = ctx.wrap_socket(request, server_side=True)
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


HOSTED_STATE = '/data/state.json'   # 托管平台上（CCA_HUB_HOSTED=1）状态文件的位置：/data 要挂一块存储


def main():
    # 托管平台上（CCA_HUB_HOSTED=1，cca hub hosted 生成的设置）：平台的表单里只填得了环境变量、一条命令，所以不带参数也能跑：等于
    # --mode own --behind-proxy --listen 0.0.0.0:8080 --state /data/state.json（命令行里写了的照命令行的）。端口固定是 8080，
    # 不看平台给的 PORT：公网地址指向哪个端口是部署时填的（railway domain --port 8080、Sealos 表单里的容器端口）
    hosted = os.environ.get('CCA_HUB_HOSTED') == '1'
    ap = argparse.ArgumentParser(description='cca 的设备在线服务')
    ap.add_argument('--listen', help='监听的地址:端口（端口 0：随便挑一个，写进 --port-file；默认 0.0.0.0:18443，'
                    'CCA_HUB_HOSTED=1 时是 0.0.0.0:8080）')
    ap.add_argument('--cert', help='TLS 证书（自签的 P-256；macOS 的 curl 只给 RSA 和 P-256/P-384 做公钥钉住）')
    ap.add_argument('--key', help='TLS 私钥')
    ap.add_argument('--behind-proxy', action='store_true', default=hosted,
                    help='在托管平台、反向代理后面：只说 HTTP（HTTPS 由平台做，不用 --cert、--key），按 X-Forwarded-For 的最后一项限流')
    # 托管平台上给了 Team 的根公钥（CCA_TEAM_ROOT）就是 Team 的服务器，不然是自己的同步服务器
    hosted_mode = 'team' if os.environ.get('CCA_TEAM_ROOT') else 'own'
    ap.add_argument('--mode', choices=('maintainer', 'own', 'team'), default=hosted_mode if hosted else 'maintainer',
                    help='maintainer：维护者的服务器（设备来自签名的设备清单）；own：自己的同步服务器（设备拿加入链接加入）；'
                    'team：Team 的同步服务器（管理员拿根密钥登记、邀请设备，存着给各台设备的配置）')
    ap.add_argument('--release-key', help='发布公钥（ssh-ed25519 …）：只认它签的设备清单（维护者的服务器必须给）')
    ap.add_argument('--admit-hash', default=os.environ.get('CCA_HUB_ADMIT_HASH'),
                    help='自己的同步服务器：入伙口令的 SHA-256（64 位十六进制；也可以用环境变量 CCA_HUB_ADMIT_HASH）')
    ap.add_argument('--team-root', default=os.environ.get('CCA_TEAM_ROOT'),
                    help='Team 的同步服务器：Team 的根公钥（ssh-ed25519 AAAA…，是公开的；也可以用环境变量 CCA_TEAM_ROOT；'
                    '只写后面那一段 AAAA… 也行：安装脚本、托管平台的环境变量里没有空格好写）')
    ap.add_argument('--state', default=HOSTED_STATE if hosted else None, help='状态文件（CCA_HUB_HOSTED=1 时默认是 /data/state.json）')
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
    ap.add_argument('--web-url', default=os.environ.get('CCA_WEB_URL'),
                    help='网页端（手机浏览器接入远程会话的那一页）：手机上打开的地址，https://域名[:端口]/。给了才开网页端（要和 '
                    '--relay-listen 一起；也可以用环境变量 CCA_WEB_URL）。浏览器只认信得过的证书：用 --web-listen 另开一个端口，'
                    '或者放在做 HTTPS 的代理后面（--behind-proxy）')
    ap.add_argument('--web-listen', help='网页端另开的监听 地址:端口，用 --web-cert、--web-key 的证书（原来那个端口上自签的证书照旧给 cca 的设备用）')
    ap.add_argument('--web-cert', help='浏览器信得过的证书（连同中间证书；证书机构签的）。文件换了会自己重新读')
    ap.add_argument('--web-key', help='--web-cert 的私钥')
    ap.add_argument('--web-root', help='网页端的文件在哪（默认是这个程序旁边的 web/）')
    ap.add_argument('--web-port-file', help='网页端实际监听的端口写到这里（测试用）')
    a = ap.parse_args()
    a.listen = a.listen or ('0.0.0.0:8080' if hosted else '0.0.0.0:18443')
    if not a.state:
        ap.error('要给 --state（状态文件）')
    if hosted and a.state == HOSTED_STATE and not os.path.isdir('/data'):
        ap.error('没有 /data：在平台上给这个服务挂一块存储，挂载路径 /data（设备、记录、密文存在那里，不然重启一次就全没了）')
    if a.behind_proxy and (a.cert or a.key):
        ap.error('--behind-proxy 时 HTTPS 由平台做：不用 --cert、--key')
    if not a.behind_proxy and not (a.cert and a.key):
        ap.error('要给 --cert 和 --key（在托管平台、反向代理后面运行时用 --behind-proxy）')
    if a.web_url and not WEB_URL_RE.match(a.web_url):
        ap.error('--web-url 要写成 https://域名[:端口]/（手机上打开的就是它；本机试的时候可以是 http://localhost:端口/）')
    if a.web_url and not a.relay_listen:
        ap.error('网页端是经中转接入会话的：--web-url 要和 --relay-listen 一起给')
    if a.web_listen or a.web_cert or a.web_key:
        if not (a.web_url and a.web_listen and a.web_cert and a.web_key):
            ap.error('--web-listen、--web-cert、--web-key 要一起给，还要有 --web-url')
        if a.behind_proxy:
            ap.error('--behind-proxy 时 HTTPS 由平台做：网页端就在同一个端口上，不用 --web-listen')
    release = root = None
    if a.mode == 'own':
        if not HASH_RE.match(a.admit_hash or ''):
            ap.error('自己的同步服务器要给入伙口令的哈希（--admit-hash 或者环境变量 CCA_HUB_ADMIT_HASH，64 位小写十六进制）')
    elif a.mode == 'team':
        given = (a.team_root or '').split()[:2]
        if len(given) == 1 and given[0] != 'ssh-ed25519':
            given = ['ssh-ed25519'] + given   # 只写了后面那一段
        k = KEY_RE.match(' '.join(given))
        if not k:
            ap.error('Team 的同步服务器要给根公钥（--team-root 或者环境变量 CCA_TEAM_ROOT：ssh-ed25519 AAAA…）')
        root = 'ssh-ed25519 ' + k.group(1)
    else:
        if not a.release_key:
            ap.error('维护者的服务器要给 --release-key')
        with open(a.release_key) as f:
            k = KEY_RE.match(' '.join(f.read().split()[:2]))
        if not k:
            ap.error('%s 不是 ssh-ed25519 公钥' % a.release_key)
        release = 'ssh-ed25519 ' + k.group(1)
    host, _, port = a.listen.rpartition(':')
    ctx = None if a.behind_proxy else tls_context(a.cert, a.key)
    sig_ok = self_test()
    store = Store(a.state, a.ttl, a.idle, a.retain_days * 86400, a.keys, a.cost, a.cost_days * 86400, a.mode, a.admit_hash, root)
    remote = web = None
    if a.relay_listen:
        rhost, _, rport = a.relay_listen.rpartition(':')
        rsock = socket.socket(socket.AF_INET6 if ':' in rhost else socket.AF_INET)
        rsock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        rsock.bind((rhost.strip('[]') or '0.0.0.0', int(rport)))
        rsock.listen(64)
        rsock.setblocking(False)
        remote = Remote(a.remote or os.path.join(os.path.dirname(a.state) or '.', 'remote.json'), a.relay_port or rsock.getsockname()[1],
                        a.web_url and a.web_url.rstrip('/') + '/')
        remote.keep_only(store.fps())
        relay = Relay(rsock, remote)
        relay.start()
        if a.web_url:
            try:
                web = Web(a.web_url, a.web_root or os.path.join(os.path.dirname(os.path.abspath(__file__)), 'web'), relay)
            except (OSError, ValueError) as e:
                ap.error('网页端的文件读不了：%s' % e)
        if a.relay_port_file:
            with open(a.relay_port_file + '.tmp', 'w') as f:
                f.write('%d\n' % rsock.getsockname()[1])
            os.replace(a.relay_port_file + '.tmp', a.relay_port_file)
    srv = Server((host.strip('[]') or '0.0.0.0', int(port)), ctx, store, release, sig_ok, remote, web)
    if a.port_file:
        with open(a.port_file + '.tmp', 'w') as f:
            f.write('%d\n' % srv.server_address[1])
        os.replace(a.port_file + '.tmp', a.port_file)
    wsrv = None
    if a.web_listen:   # 网页端另开的端口：同一个服务（同样的接口、同一份限流），只是证书是浏览器信得过的那一张
        whost, _, wport = a.web_listen.rpartition(':')
        try:
            wctx = CertCtx(a.web_cert, a.web_key)
        except (OSError, ssl.SSLError, ValueError) as e:
            ap.error('网页端的证书读不了：%s' % e)
        wsrv = Server((whost.strip('[]') or '0.0.0.0', int(wport)), wctx, store, release, sig_ok, remote, web, share=srv)
        threading.Thread(target=wsrv.serve_forever, name='web', daemon=True).start()
        if a.web_port_file:
            with open(a.web_port_file + '.tmp', 'w') as f:
                f.write('%d\n' % wsrv.server_address[1])
            os.replace(a.web_port_file + '.tmp', a.web_port_file)
    sys.stderr.write('cca-presence: %s监听 %s:%d%s，设备 %d 台，签名自检%s%s\n' % (
        {'own': '自己的同步服务器，', 'team': 'Team 的同步服务器，'}.get(a.mode, ''), srv.server_address[0], srv.server_address[1],
        '（HTTP，HTTPS 由前面的代理做）' if a.behind_proxy else '', len(store.data['machines']),
        '通过' if sig_ok else '没通过',
        ('，中转的数据端口 %d（%d 个配对）' % (rsock.getsockname()[1], len(remote.edges)) if remote else '')
        + ('，网页端 %s（%d 个文件%s）' % (web.url, len(web.files), '，另开的端口 %d' % wsrv.server_address[1] if wsrv else '') if web else '')))
    sys.stderr.flush()
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == '__main__':
    main()
