// cca 远程会话的网页端：协议和加密这一层（没有界面）。格式照 server/README.md「远程会话的中转」「网页端」，
// 和 cca 的辅助程序（cli/src/cmd/remote/）、tests/kit/spec.rs 是同一套——三份实现要能互相配对、握手、收发。
// 只用浏览器自带的：WebCrypto（PBKDF2、Ed25519、SHA-512）、fetch、WebSocket；HMAC-SHA256 自己算（加密记录要一块块地出密钥流，
// WebCrypto 的每次调用都是异步的，太慢）。Node 里也能原样跑（tests/web_client.mjs 拿它和真的服务器、真的 cca 对着测）。

const te = new TextEncoder();
const td = new TextDecoder('utf-8', { fatal: false });

// ---------------------------------------------------------------- 字节
export const utf8 = (s) => te.encode(s);
export const text = (b) => td.decode(b);

export function concat(...parts) {
  let n = 0;
  for (const p of parts) n += p.length;
  const out = new Uint8Array(n);
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

export function hex(b) {
  let s = '';
  for (const x of b) s += (x < 16 ? '0' : '') + x.toString(16);
  return s;
}

export function unhex(s) {
  if (typeof s !== 'string' || s.length % 2 || /[^0-9a-f]/.test(s)) return null;
  const out = new Uint8Array(s.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(s.substr(2 * i, 2), 16);
  return out;
}

export function b64(b) {
  let s = '';
  for (let i = 0; i < b.length; i += 0x8000) s += String.fromCharCode.apply(null, b.subarray(i, i + 0x8000));
  return btoa(s);
}

export function unb64(s) {
  if (typeof s !== 'string' || !/^[A-Za-z0-9+/]*={0,2}$/.test(s)) return null;
  try {
    const raw = atob(s);
    const out = new Uint8Array(raw.length);
    for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
    return out;
  } catch {
    return null;
  }
}

const b64url = (b) => b64(b).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');

function u64(n) {
  const out = new Uint8Array(8);
  new DataView(out.buffer).setUint32(4, n >>> 0);
  new DataView(out.buffer).setUint32(0, Math.floor(n / 0x100000000));
  return out;
}

/** 比两段字节：不因为前几个字节不一样就早回（认证码的比较） */
export function same(a, b) {
  if (a.length !== b.length) return false;
  let d = 0;
  for (let i = 0; i < a.length; i++) d |= a[i] ^ b[i];
  return d === 0;
}

export function random(n) {
  return crypto.getRandomValues(new Uint8Array(n));
}

// ---------------------------------------------------------------- SHA-256、HMAC（同步的）
const K = new Int32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be,
  0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa,
  0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85,
  0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
  0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f,
  0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);
const IV = new Int32Array([0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]);
const W = new Int32Array(64);

function compress(h, p, off) {
  for (let i = 0; i < 16; i++, off += 4) W[i] = (p[off] << 24) | (p[off + 1] << 16) | (p[off + 2] << 8) | p[off + 3];
  for (let i = 16; i < 64; i++) {
    const x = W[i - 15];
    const y = W[i - 2];
    W[i] = (W[i - 16] + (((x >>> 7) | (x << 25)) ^ ((x >>> 18) | (x << 14)) ^ (x >>> 3)) + W[i - 7]
      + (((y >>> 17) | (y << 15)) ^ ((y >>> 19) | (y << 13)) ^ (y >>> 10))) | 0;
  }
  let a = h[0], b = h[1], c = h[2], d = h[3], e = h[4], f = h[5], g = h[6], k = h[7];
  for (let i = 0; i < 64; i++) {
    const t1 = (k + (((e >>> 6) | (e << 26)) ^ ((e >>> 11) | (e << 21)) ^ ((e >>> 25) | (e << 7))) + ((e & f) ^ (~e & g)) + K[i] + W[i]) | 0;
    const t2 = ((((a >>> 2) | (a << 30)) ^ ((a >>> 13) | (a << 19)) ^ ((a >>> 22) | (a << 10))) + ((a & b) ^ (a & c) ^ (b & c))) | 0;
    k = g; g = f; f = e; e = (d + t1) | 0; d = c; c = b; b = a; a = (t1 + t2) | 0;
  }
  h[0] = (h[0] + a) | 0; h[1] = (h[1] + b) | 0; h[2] = (h[2] + c) | 0; h[3] = (h[3] + d) | 0;
  h[4] = (h[4] + e) | 0; h[5] = (h[5] + f) | 0; h[6] = (h[6] + g) | 0; h[7] = (h[7] + k) | 0;
}

/** 从状态 h0（前面已经算过 before 个字节，是 64 的整数倍）接着算 msg，出 32 个字节 */
function finish(h0, before, msg) {
  const h = new Int32Array(h0);
  const full = msg.length - (msg.length % 64);
  for (let off = 0; off < full; off += 64) compress(h, msg, off);
  const rest = msg.length - full;
  const tail = new Uint8Array(rest < 56 ? 64 : 128);
  tail.set(msg.subarray(full));
  tail[rest] = 0x80;
  const bits = (before + msg.length) * 8;
  const view = new DataView(tail.buffer);
  view.setUint32(tail.length - 8, Math.floor(bits / 0x100000000));
  view.setUint32(tail.length - 4, bits >>> 0);
  for (let off = 0; off < tail.length; off += 64) compress(h, tail, off);
  const out = new Uint8Array(32);
  const ov = new DataView(out.buffer);
  for (let i = 0; i < 8; i++) ov.setInt32(4 * i, h[i]);
  return out;
}

export const sha256 = (msg) => finish(IV, 0, msg);

/** 一把 HMAC-SHA256 的密钥：两个垫块先算好，之后每次只算消息 */
export class Hmac {
  constructor(key) {
    const k = new Uint8Array(64);
    k.set(key.length > 64 ? sha256(key) : key);
    const pad = new Uint8Array(64);
    this.inner = new Int32Array(IV);
    for (let i = 0; i < 64; i++) pad[i] = k[i] ^ 0x36;
    compress(this.inner, pad, 0);
    this.outer = new Int32Array(IV);
    for (let i = 0; i < 64; i++) pad[i] = k[i] ^ 0x5c;
    compress(this.outer, pad, 0);
  }

  mac(msg) {
    return finish(this.outer, 64, finish(this.inner, 64, msg));
  }
}

export const hmac = (key, msg) => new Hmac(key).mac(msg);

/** 密钥流：HMAC(enc, nonce + 8 字节大端的块号)，一块 32 个字节，和 data 异或（原地改） */
function xorStream(enc, nonce, data) {
  const block = new Uint8Array(nonce.length + 8);
  block.set(nonce);
  const view = new DataView(block.buffer);
  for (let i = 0, at = 0; at < data.length; i++, at += 32) {
    view.setUint32(nonce.length + 4, i);
    const ks = enc.mac(block);
    const n = Math.min(32, data.length - at);
    for (let j = 0; j < n; j++) data[at + j] ^= ks[j];
  }
  return data;
}

// ---------------------------------------------------------------- 封好的一份（{n, c, t}：配对时交换的、会话的说明）
function sealTag(label, mac, ad, n, c) {
  return mac.mac(concat(utf8([label, ...ad].map((x) => x + '\0').join('')), n, c));
}

export function seal(label, keys, ad, pt) {
  const n = random(16);
  const c = xorStream(keys.enc, n, pt.slice());
  return { n: b64(n), c: b64(c), t: b64(sealTag(label, keys.mac, ad, n, c)) };
}

/** 认证过的明文；认证不过、样子不对是 null */
export function unseal(label, keys, ad, blob) {
  if (!blob || typeof blob !== 'object') return null;
  const n = unb64(blob.n);
  const c = unb64(blob.c);
  const t = unb64(blob.t);
  if (!n || !c || !t || n.length !== 16 || !same(t, sealTag(label, keys.mac, ad, n, c))) return null;
  return xorStream(keys.enc, n, c);
}

function padded(obj) {
  const pt = utf8(JSON.stringify(obj));
  const out = new Uint8Array(pt.length + ((256 - (pt.length % 256)) % 256)).fill(0x20);
  out.set(pt);
  return out;
}

function parsed(pt) {
  try {
    const j = JSON.parse(text(pt));
    return j && typeof j === 'object' && !Array.isArray(j) ? j : null;
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------- 配对码、配对
const B32 = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';   // Crockford base32

/** 输入的配对码（大小写、横线、空格随意；I L 当 1、O 当 0）→ 10 个字节；不对是 null */
export function codeBytes(input) {
  const s = String(input).toUpperCase().replace(/[-\s]/g, '').replace(/[IL]/g, '1').replace(/O/g, '0');
  if (s.length !== 16) return null;
  let n = 0n;
  for (const ch of s) {
    const v = B32.indexOf(ch);
    if (v < 0) return null;
    n = n * 32n + BigInt(v);
  }
  const out = new Uint8Array(10);
  for (let i = 9; i >= 0; i--, n >>= 8n) out[i] = Number(n & 0xffn);
  return out;
}

export function codeText(c) {
  let n = 0n;
  for (const x of c) n = (n << 8n) | BigInt(x);
  let s = '';
  for (let i = 15; i >= 0; i--) s += B32[Number((n >> BigInt(5 * i)) & 31n)];
  return s.replace(/(.{4})(?=.)/g, '$1-');
}

/** 配对码 → {kc, keys, rid}。配对码只有 80 位，所以这一步故意算得慢（PBKDF2 65536 轮） */
export async function pairKeys(code) {
  const base = await crypto.subtle.importKey('raw', code, 'PBKDF2', false, ['deriveBits']);
  const kc = new Uint8Array(await crypto.subtle.deriveBits(
    { name: 'PBKDF2', hash: 'SHA-256', salt: utf8('cca-remote/v1/pair'), iterations: 65536 }, base, 256));
  const k = new Hmac(kc);
  return { kc: k, keys: { enc: new Hmac(k.mac(utf8('enc'))), mac: new Hmac(k.mac(utf8('mac'))) }, rid: hex(k.mac(utf8('rid'))).slice(0, 32) };
}

/** 配对时放到服务器上的那一份。role："a" 显示配对码的 / "b" 输入的 */
export function pairMsg(keys, rid, role, fp, nonce, name) {
  return seal('cca-remote-pair', keys, [rid, role], padded({ v: 1, fp, n: hex(nonce), name }));
}

export function pairOpen(keys, rid, role, blob) {
  const pt = unseal('cca-remote-pair', keys, [rid, role], blob);
  const j = pt && parsed(pt);
  const n = j && unhex(j.n);
  if (!j || j.v !== 1 || typeof j.fp !== 'string' || !n || n.length !== 32) return null;
  return { fp: j.fp, n, name: typeof j.name === 'string' ? j.name : '' };
}

/** 两台设备之间的密钥 P 和它的编号（服务器只知道编号）。a 是显示配对码的那台 */
export function pairSecret(kc, na, nb, fpa, fpb) {
  const p = kc.mac(concat(utf8('secret\0'), na, nb, utf8(fpa + '\0' + fpb + '\0')));
  return { p, pid: hex(hmac(p, utf8('cca-remote/v1/pid'))).slice(0, 32) };
}

/** 对方写的、服务器转交的文字，显示之前：去掉控制字符，限制长度（和辅助程序的 far::clean 一样的用意） */
export function clean(s, max) {
  if (typeof s !== 'string') return '';
  // eslint-disable-next-line no-control-regex
  const out = Array.from(s.replace(/[\u0000-\u001f\u007f-\u009f‎‏‪-‮⁦-⁩]/g, ''));
  return out.slice(0, max).join('').trim();
}

/** 会话的说明（宿主封给这个配对的）；打不开是 null */
export function dirOpen(kd, pid, sid, blob) {
  const pt = unseal('cca-remote-dir', kd, [pid, sid], blob);
  const j = pt && parsed(pt);
  if (!j || j.v !== 1) return null;
  return {
    machine: clean(j.machine, 48), tool: clean(j.tool, 16), name: clean(j.name, 48), cwd: clean(j.cwd, 200),
    since: Number.isFinite(j.since) ? j.since : 0, access: j.access === 'view' ? 'view' : 'control',
  };
}

// ---------------------------------------------------------------- 两头之间的通道（这一头总是接入方）
const VER = 1;
export const RECORD_MAX = 16384;
const pidBytes = (s) => unhex(s);

export class Channel {
  /** ka：HMAC(P, "cca-remote/v1/auth")（32 个字节）；fpH、fpV：配对时记下的宿主、自己的指纹 */
  constructor(ka, pid, sid, fpH, fpV) {
    this.ka = new Hmac(ka);
    this.pid = pidBytes(pid);
    this.sid = pidBytes(sid);
    this.fps = utf8(fpH + '\0' + fpV + '\0');
    this.sendSeq = 0;
    this.recvSeq = 0;
  }

  /** 接入方 → 宿主（52 个字节） */
  hello() {
    this.nv = random(32);
    return concat(utf8('CR'), Uint8Array.of(1, VER), this.nv, this.pid);
  }

  /** 宿主回的 68 个字节 → 该发的确认（35 个字节）；宿主的证明不对是 null（之后什么都不该再发） */
  confirm(reply) {
    if (reply.length !== 68 || text(reply.subarray(0, 2)) !== 'CR' || reply[2] !== 2 || reply[3] !== VER) return null;
    const nh = reply.subarray(4, 36);
    const tagH = reply.subarray(36);
    const ctx = concat(Uint8Array.of(VER), this.pid, this.sid, this.nv, nh, this.fps);
    if (!same(tagH, this.ka.mac(concat(utf8('host\0'), ctx)))) return null;
    const tagV = this.ka.mac(concat(utf8('viewer\0'), ctx, tagH));
    const ks = new Hmac(this.ka.mac(concat(utf8('session\0'), ctx, tagH, tagV)));
    const key = (name) => new Hmac(ks.mac(utf8(name)));
    this.out = { enc: key('v2h-enc'), mac: key('v2h-mac') };
    this.inn = { enc: key('h2v-enc'), mac: key('h2v-mac') };
    return concat(utf8('CR'), Uint8Array.of(3), tagV);
  }

  /** 一条记录：序号(8) 密文长度(2) 密文 认证(16)。明文是 长度(2) 内容 零补齐到 32 的整数倍 */
  seal(data) {
    if (data.length > RECORD_MAX) throw new Error('record too long');
    const pt = new Uint8Array(Math.ceil((2 + data.length) / 32) * 32);
    pt[0] = data.length >> 8;
    pt[1] = data.length & 0xff;
    pt.set(data, 2);
    const seq = u64(this.sendSeq++);
    xorStream(this.out.enc, seq, pt);
    const head = concat(seq, Uint8Array.of(pt.length >> 8, pt.length & 0xff));
    return concat(head, pt, this.out.mac.mac(concat(head, pt)).subarray(0, 16));
  }

  /** buf 开头的一条记录 → {data, used}；还不够一条是 null；序号、认证、补齐不对抛错（断开） */
  open(buf) {
    if (buf.length < 10) return null;
    const n = (buf[8] << 8) | buf[9];
    if (n > RECORD_MAX + 2 + 31 || n % 32 || n === 0) throw new Error('record length');
    if (buf.length < 26 + n) return null;
    const head = buf.subarray(0, 10);
    if (!same(head.subarray(0, 8), u64(this.recvSeq))
        || !same(buf.subarray(10 + n, 26 + n), this.inn.mac.mac(buf.subarray(0, 10 + n)).subarray(0, 16))) {
      throw new Error('record auth');
    }
    this.recvSeq++;
    const pt = xorStream(this.inn.enc, head.subarray(0, 8), buf.slice(10, 10 + n));
    const size = (pt[0] << 8) | pt[1];
    if (size > n - 2) throw new Error('record pad');
    for (let i = 2 + size; i < n; i++) if (pt[i]) throw new Error('record pad');
    return { data: pt.subarray(2, 2 + size), used: 26 + n };
  }
}

// ---------------------------------------------------------------- 这台设备的身份：一把 Ed25519，请求照 SSHSIG 签名
const sshString = (b) => {
  const out = new Uint8Array(4 + b.length);
  new DataView(out.buffer).setUint32(0, b.length);
  out.set(b, 4);
  return out;
};
const NS = 'cca-presence';

export class Identity {
  /** privateKey：WebCrypto 的 Ed25519 私钥（不可导出）；pub：32 个字节的公钥 */
  constructor(privateKey, pub) {
    this.privateKey = privateKey;
    this.blob = concat(sshString(utf8('ssh-ed25519')), sshString(pub));
    this.pub = 'ssh-ed25519 ' + b64(this.blob);
    this.fp = 'SHA256:' + b64(sha256(this.blob)).replace(/=+$/, '');
  }

  static async create() {
    const pair = await crypto.subtle.generateKey({ name: 'Ed25519' }, false, ['sign', 'verify']);
    return { privateKey: pair.privateKey, pub: new Uint8Array(await crypto.subtle.exportKey('raw', pair.publicKey)) };
  }

  /** ssh-keygen -Y sign 那种签名（OpenSSH 的 PROTOCOL.sshsig），命名空间 cca-presence */
  async sign(data) {
    const digest = new Uint8Array(await crypto.subtle.digest('SHA-512', data));
    const rest = concat(sshString(utf8(NS)), sshString(new Uint8Array(0)), sshString(utf8('sha512')));
    const sig = new Uint8Array(await crypto.subtle.sign({ name: 'Ed25519' }, this.privateKey, concat(utf8('SSHSIG'), rest, sshString(digest))));
    const blob = concat(utf8('SSHSIG'), Uint8Array.of(0, 0, 0, 1), sshString(this.blob), rest,
      sshString(concat(sshString(utf8('ssh-ed25519')), sshString(sig))));
    const lines = b64(blob).match(/.{1,70}/g).join('\n');
    return `-----BEGIN SSH SIGNATURE-----\n${lines}\n-----END SSH SIGNATURE-----\n`;
  }
}

// ---------------------------------------------------------------- 出了什么事：界面照 code 说话
export class Failure extends Error {
  /** code：给界面分情况的词；status：服务器回的状态码（没问到是 0） */
  constructor(code, status = 0, more = '') {
    super(more ? `${code}: ${more}` : code);
    this.code = code;
    this.status = status;
  }
}

// ---------------------------------------------------------------- 帧（记录里装的，和 cca 在一台机器上用的同一套）
export const F = { HELLO: 0x01, INPUT: 0x02, RESIZE: 0x03, DETACH: 0x04, OUTPUT: 0x12, EXIT: 0x13 };
const FRAME_MAX = 1 << 20;

function frame(kind, payload) {
  const out = new Uint8Array(5 + payload.length);
  out[0] = kind;
  new DataView(out.buffer).setUint32(1, payload.length);
  out.set(payload, 5);
  return out;
}

const sizeBytes = (cols, rows) => Uint8Array.of(cols >> 8, cols & 0xff, rows >> 8, rows & 0xff);
const validSize = (cols, rows) => Number.isInteger(cols) && Number.isInteger(rows) && cols >= 2 && rows >= 2 && cols <= 1000 && rows <= 1000;

/** 攒着收到的字节，从头取 */
class Buffer {
  constructor() {
    this.b = new Uint8Array(0);
  }

  push(data) {
    this.b = this.b.length ? concat(this.b, data) : data;
  }

  take(n) {
    const out = this.b.subarray(0, n);
    this.b = this.b.subarray(n);
    return out;
  }

  get length() {
    return this.b.length;
  }
}

/** 一处接入：一条 WebSocket，从开场白、握手到收发。事件经 on 给的几个回调出来：
 *  open()、output(字节)、exit(退出码)、close(原因)。原因是一个词：
 *  detached（自己断开的）、ended（会话结束了）、lost（连接断了）、refused（宿主不认这个配对）、version、
 *  auth（对面证明不了它有密钥：什么都没发给它）、nohost（那台机器这会儿没连着服务器）、busy、ticket、error */
export class Link {
  constructor({ url, WebSocket: WS, ticket, channel, cols, rows, readOnly, on }) {
    this.on = on;
    this.channel = channel;
    this.buf = new Buffer();
    this.plain = new Buffer();
    this.phase = 'line';
    this.done = false;
    this.heard = Date.now();
    this.hello = { cols, rows, readOnly: !!readOnly };
    const cn = hex(random(8));
    const tkey = new Hmac(ticket.tkey);
    this.line = `CCA-RELAY/1 view ${ticket.tid} ${cn} ${b64url(tkey.mac(utf8('cview' + ticket.tid + cn)))}\n`;
    this.expect = `CCA-RELAY/1 OK ${b64url(tkey.mac(utf8('s' + ticket.tid + cn)))}`;
    const ws = this.ws = new WS(url);
    ws.binaryType = 'arraybuffer';
    ws.onopen = () => ws.send(utf8(this.line));
    ws.onmessage = (e) => this.guard(() => this.feed(new Uint8Array(e.data)));
    ws.onclose = () => this.finish(this.phase === 'line' ? 'unreachable' : 'lost');
    ws.onerror = () => {};
    // 等宿主那头连上来（服务器最多等它 10 秒）、握手：太久就算了
    this.timer = setTimeout(() => this.finish(this.phase === 'line' ? 'nohost' : 'lost'), 25000);
  }

  guard(f) {
    try {
      f();
    } catch (e) {
      this.finish(e instanceof Failure ? e.code : 'error');
    }
  }

  feed(data) {
    if (this.done) return;
    this.heard = Date.now();
    this.buf.push(data);
    if (this.phase === 'line') {
      const nl = this.buf.b.indexOf(10);
      if (nl < 0) {
        if (this.buf.length > 200) throw new Failure('error');
        return;
      }
      const line = text(this.buf.take(nl + 1)).trimEnd();
      if (line !== this.expect) {
        // 服务器只会说这几个词；别的不原样往外传
        const why = /^CCA-RELAY\/1 ERR (ticket|bad|nohost|taken|sessions|slow)$/.exec(line);
        throw new Failure(why ? ({ nohost: 'nohost', sessions: 'busy' }[why[1]] || 'ticket') : 'error');
      }
      this.phase = 'reply';
      this.ws.send(this.channel.hello());
    }
    if (this.phase === 'reply') {
      // 宿主不认这个配对、版本不一样：回 4 个字节 "CR" 7F 原因 就断开
      if (this.buf.length >= 4 && this.buf.b[2] === 0x7f) throw new Failure(this.buf.b[3] === 4 ? 'version' : 'refused');
      if (this.buf.length < 68) return;
      const confirm = this.channel.confirm(this.buf.take(68));
      if (!confirm) throw new Failure('auth');
      this.ws.send(confirm);
      this.phase = 'open';
      clearTimeout(this.timer);
      const h = this.hello;
      this.write(frame(F.HELLO, concat(Uint8Array.of(1, h.readOnly ? 1 : 0), sizeBytes(h.cols, h.rows))));
      // 两头每 20 秒说一声（空的记录），60 秒没听到对面就算断了
      this.beat = setInterval(() => {
        if (Date.now() - this.heard > 60000) this.finish('lost');
        else this.ws.send(this.channel.seal(new Uint8Array(0)));
      }, 20000);
      this.on.open?.();
    }
    if (this.phase === 'open') this.records();
  }

  records() {
    for (;;) {
      const rec = this.channel.open(this.buf.b);
      if (!rec) break;
      this.buf.take(rec.used);
      if (rec.data.length) this.plain.push(rec.data.slice());
    }
    while (!this.done && this.plain.length >= 5) {
      const n = new DataView(this.plain.b.buffer, this.plain.b.byteOffset + 1, 4).getUint32(0);
      if (n > FRAME_MAX) throw new Failure('error');
      if (this.plain.length < 5 + n) break;
      const kind = this.plain.b[0];
      const payload = this.plain.take(5 + n).subarray(5);
      if (kind === F.OUTPUT) this.on.output?.(payload);
      else if (kind === F.EXIT) {
        this.on.exit?.(payload.length >= 4 ? new DataView(payload.buffer, payload.byteOffset, 4).getInt32(0) : 0);
        this.finish('ended');
      }
    }
  }

  /** 帧的字节切成记录发出去 */
  write(bytes) {
    if (this.phase !== 'open' || this.done) return;
    for (let at = 0; at < bytes.length; at += RECORD_MAX) this.ws.send(this.channel.seal(bytes.subarray(at, at + RECORD_MAX)));
  }

  input(data) {
    if (this.hello.readOnly || !data.length) return;
    for (let at = 0; at < data.length; at += 8192) this.write(frame(F.INPUT, data.subarray(at, at + 8192)));
  }

  resize(cols, rows) {
    if (!validSize(cols, rows)) return;
    this.hello.cols = cols;
    this.hello.rows = rows;
    this.write(frame(F.RESIZE, sizeBytes(cols, rows)));
  }

  /** 自己断开（会话接着运行） */
  detach() {
    this.write(frame(F.DETACH, new Uint8Array(0)));
    this.finish('detached');
  }

  finish(why) {
    if (this.done) return;
    this.done = true;
    clearTimeout(this.timer);
    clearInterval(this.beat);
    try {
      this.ws.close();
    } catch { /* 已经关了 */ }
    this.on.close?.(why);
  }
}

// ---------------------------------------------------------------- 客户端：身份、配对表、请求
const PEER_NAME = 48;

/** store：{get(key), set(key, value), del(key)}，都是异步的；浏览器里是 IndexedDB（存得下不可导出的密钥），测试里是内存。
 *  base：服务器的地址（页面就是它给的：同一个源，留空）；name：这台设备叫什么（配对时告诉对方） */
export class Client {
  constructor({ base = '', store, fetch: f, WebSocket: WS, name = '手机' }) {
    this.base = base.replace(/\/$/, '');
    this.store = store;
    this.fetch = f || ((...a) => fetch(...a));
    this.WS = WS || WebSocket;
    this.name = name;
    this.skew = 0;
  }

  /** 这台设备的密钥：第一次用时生成，之后一直是它（清掉网站数据就没了，要重新配对） */
  async identity() {
    if (!this.id) {
      let saved = await this.store.get('identity');
      if (!saved) {
        saved = await Identity.create();
        await this.store.set('identity', saved);
      }
      this.id = new Identity(saved.privateKey, saved.pub);
    }
    return this.id;
  }

  /** 发一个签了名的请求。回服务器的 JSON；不是 200 抛 Failure（code 是服务器说的 error，没问到是 unreachable） */
  async post(op, extra = {}, again = true) {
    const id = await this.identity();
    const body = utf8(JSON.stringify({ ...extra, v: 1, op, key: id.fp, ts: Date.now() / 1000 + this.skew, nonce: hex(random(16)) }));
    const sig = await id.sign(body);
    let r;
    let j;
    // 有期限：手机的网络半死不活时请求会一直挂着（服务器替 pair_get 最多等 10 秒，所以给得比它长）
    const abort = new AbortController();
    const timer = setTimeout(() => abort.abort(), 25000);
    try {
      r = await this.fetch(`${this.base}/v1/${op}`, {
        method: 'POST', body, cache: 'no-store', credentials: 'omit', signal: abort.signal,
        headers: { 'Content-Type': 'application/json', 'X-Cca-Sig': b64(utf8(sig)) },
      });
      j = await r.json();
    } catch {
      throw new Failure('unreachable');
    } finally {
      clearTimeout(timer);
    }
    if (r.status === 200 && j && j.ok) return j;
    const code = j && typeof j.error === 'string' && /^[a-z_]{1,32}$/.test(j.error) ? j.error : 'error';
    if (code === 'clock' && again && Number.isFinite(j.now)) {   // 这台设备的钟不准：照服务器的时间再发一次
      this.skew = j.now - Date.now() / 1000;
      return this.post(op, extra, false);
    }
    throw new Failure(code, r.status);
  }

  async peers() {
    return (await this.store.get('peers')) || [];
  }

  /** 输入另一台机器显示的配对码。progress(步骤)：'deriving' | 'asking' | 'confirming'。回配对表里新的那一项 */
  async pair(input, progress = () => {}) {
    const code = codeBytes(input);
    if (!code) throw new Failure('code');
    const id = await this.identity();
    progress('deriving');
    const k = await pairKeys(code);
    const nb = random(32);
    progress('asking');
    try {
      // 带上公钥：这台设备不在服务器的设备清单里，凭这次配对（知道配对码才知道 rid）让服务器认它
      await this.post('pair_put', { rid: k.rid, pub: id.pub, m: pairMsg(k.keys, k.rid, 'b', id.fp, nb, this.name) });
    } catch (e) {
      // 没有这个配对码（输错了、过期了）、它不是开给手机的、已经有人输入过：服务器对不认识的设备回的都一样
      if (e.status === 403 || e.status === 404 || e.status === 409 || e.status === 400) throw new Failure('no_pairing', e.status);
      throw e;
    }
    // 从这里起没成（没等到、对不上、网络断了）都告诉服务器这次配对作废：电脑那一头马上知道，不用干等；配对码也不留着
    let committed = null;
    try {
      let theirs = null;
      for (let i = 0; i < 4 && !theirs; i++) {
        const j = await this.post('pair_get', { rid: k.rid, wait: 10 });
        if (j.m) theirs = { m: j.m, fp: j.fp, listed: typeof j.name === 'string' ? j.name : '' };
      }
      if (!theirs) throw new Failure('no_answer');
      const opened = pairOpen(k.keys, k.rid, 'a', theirs.m);
      if (!opened) throw new Failure('bad_peer');
      if (opened.fp !== theirs.fp || opened.fp === id.fp) throw new Failure('bad_peer');
      const { p, pid } = pairSecret(k.kc, opened.n, nb, opened.fp, id.fp);
      progress('confirming');
      let done = (await this.post('pair_commit', { rid: k.rid, pid })).done === true;
      for (let i = 0; i < 6 && !done; i++) done = (await this.post('pair_get', { rid: k.rid, wait: 10, until: 'done' })).done === true;
      if (!done) throw new Failure('no_answer');
      committed = pid;
      // 密钥存成不可导出的：页面以后只能拿它算，取不出原样的字节
      const key = await crypto.subtle.importKey('raw', p, { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
      p.fill(0);
      // 名字先用服务器的设备清单里那台电脑叫什么（cca 里各处看到的都是它，辅助程序也是这么取的），没有才用它自己说的机器名。
      // 只是给人看的：认设备靠的是上面核对过的指纹
      const peer = { pid, fp: opened.fp, name: clean(theirs.listed, PEER_NAME) || clean(opened.name, PEER_NAME) || `电脑 ${pid.slice(0, 6)}`, at: Date.now(), key };
      // 同一台机器以前配过的换掉（服务器上也是换掉）
      const all = (await this.peers()).filter((x) => x.fp !== peer.fp && x.pid !== pid);
      all.push(peer);
      await this.store.set('peers', all);
      return peer;
    } catch (e) {
      // 服务器已经记下了、这边却没存成：把那条配对删掉，不留一条这台设备用不了的
      await (committed ? this.post('pair_drop', { pid: committed }) : this.post('pair_close', { rid: k.rid })).catch(() => {});
      throw e;
    }
  }

  /** 取消配对：服务器上的记录删掉（删不成也不拦着），这台设备上的密钥删掉 */
  async unpair(pid) {
    let told = true;
    try {
      await this.post('pair_drop', { pid });
    } catch {
      told = false;
    }
    await this.store.set('peers', (await this.peers()).filter((x) => x.pid !== pid));
    return told;
  }

  /** P 派生出来的一把密钥（原样的 32 个字节，只在内存里） */
  async derived(peer, label) {
    return new Uint8Array(await crypto.subtle.sign('HMAC', peer.key, utf8(label)));
  }

  /** 配对过的机器和它们上面可以接入的会话。回 [{peer, gone, sessions: [...]}]；gone：对方已经取消了配对 */
  async list() {
    const all = await this.peers();
    if (!all.length) return [];
    const j = await this.post('relay_list', { pids: all.map((p) => p.pid) });
    const out = [];
    for (const peer of all) {
      const known = j.peers && Object.prototype.hasOwnProperty.call(j.peers, peer.pid);
      const rows = (j.sessions && j.sessions[peer.pid]) || [];
      const kd = new Hmac(await this.derived(peer, 'cca-remote/v1/dir'));
      const keys = { enc: new Hmac(kd.mac(utf8('enc'))), mac: new Hmac(kd.mac(utf8('mac'))) };
      const sessions = [];
      for (const r of Array.isArray(rows) ? rows : []) {
        if (!r || typeof r.sid !== 'string' || !/^[0-9a-f]{32}$/.test(r.sid)) continue;
        const info = dirOpen(keys, peer.pid, r.sid, r.blob);
        if (!info) continue;   // 不是这个配对的密钥封的（服务器、别人塞进来的）：不显示
        sessions.push({ ...info, sid: r.sid, online: r.online === true, viewers: Number.isInteger(r.viewers) ? r.viewers : 0 });
      }
      sessions.sort((a, b) => b.since - a.since);
      out.push({ peer, gone: !known, sessions });
    }
    return out;
  }

  /** 接入一个会话（一次连接；断了要不要重新接上由调用方定，见 Attachment） */
  async link(peer, sid, { cols, rows, readOnly = false, on }) {
    if (!validSize(cols, rows)) throw new Failure('error');
    const id = await this.identity();
    const t = await this.post('relay_attach', { sid, pid: peer.pid });
    const tkey = unb64(t.tkey);
    if (!tkey || tkey.length !== 32 || !/^[0-9a-f]{32}$/.test(t.tid || '')) throw new Failure('error');
    const channel = new Channel(await this.derived(peer, 'cca-remote/v1/auth'), peer.pid, sid, peer.fp, id.fp);
    const origin = this.base || (typeof location !== 'undefined' ? location.origin : '');
    // 网址里带着票据的编号：服务器先看它在不在，才让这条 WebSocket 进来（编号不是秘密；进中转靠的是开场白里证明有票据的密钥）
    return new Link({ url: `${origin.replace(/^http/, 'ws')}/v1/relay/ws?t=${t.tid}`, WebSocket: this.WS, ticket: { tid: t.tid, tkey }, channel, cols, rows, readOnly, on });
  }
}

// ---------------------------------------------------------------- 连接断了自己重新接上
const RETRY_GAP = [1, 2, 4, 8, 15];   // 秒；之后一直 15
const RETRY_FOR = 300;                // 一共试这么久
const MISSING_FOR = 45;               // 服务器说没有这个会话：多半是结束了，再等这么久就不等了
const FLAPS = 5;                      // 接上不到 10 秒又断，连着这么多次就不再试

/** 一个会话在这台设备上的接入：断了（网络断了、页面被切到后台、服务器重启）照辅助程序的办法重新接上。
 *  on：output(字节)、state(状态, 说明)、exit(退出码)。状态：connecting、open、retrying、closed；
 *  closed 的说明是 Link 的原因，再加 gone（会话不在了）、unpaired（配对没了）、gave_up（一直没接上） */
export class Attachment {
  constructor(client, peer, sid, { cols, rows, readOnly = false, on, timers = globalThis }) {
    Object.assign(this, { client, peer, sid, cols, rows, readOnly, on, timers });
    this.state = 'connecting';
    this.flaps = 0;
    this.connect(false);
  }

  set(state, why = '') {
    this.state = state;
    this.on.state?.(state, why);
  }

  async connect(retrying) {
    const me = (this.attempt = {});
    try {
      const link = await this.client.link(this.peer, this.sid, {
        cols: this.cols, rows: this.rows, readOnly: this.readOnly,
        on: {
          open: () => {
            if (this.attempt !== me) return;
            this.openedAt = Date.now();
            this.since = null;
            this.missing = null;
            this.set('open');
          },
          output: (b) => this.attempt === me && this.on.output?.(b),
          exit: (code) => this.attempt === me && this.on.exit?.(code),
          close: (why) => this.attempt === me && this.closed(why),
        },
      });
      if (this.attempt !== me) return link.finish('detached');
      this.link = link;
    } catch (e) {
      if (this.attempt !== me) return undefined;
      const code = e instanceof Failure ? e.code : 'error';
      if (code === 'no_session') {
        // 第一次就没有：会话不在了。重新接的时候：多半是结束了，也可能是那头正在重连，留一阵
        if (!retrying) return this.stop('gone');
        this.missing = this.missing || Date.now();
        if (Date.now() - this.missing > MISSING_FOR * 1000) return this.stop('gone');
        return this.later();
      }
      if (code === 'denied' || code === 'unknown_device') return this.stop('unpaired');
      if (!retrying && code !== 'unreachable' && code !== 'rate' && code !== 'busy') return this.stop(code);
      this.missing = null;
      return this.later();
    }
    return undefined;
  }

  closed(why) {
    this.link = null;
    if (why === 'detached' || why === 'ended' || why === 'refused' || why === 'version' || why === 'auth') return this.stop(why);
    if (this.state === 'open') {
      this.flaps = Date.now() - this.openedAt < 10000 ? this.flaps + 1 : 0;
      if (this.flaps >= FLAPS) return this.stop('gave_up');
      this.tries = this.flaps;   // 刚接上就断的：间隔从长一点的起步，别一秒一次地问服务器
    }
    return this.later();
  }

  later() {
    this.since = this.since || Date.now();
    if (Date.now() - this.since > RETRY_FOR * 1000) return this.stop('gave_up');
    const gap = RETRY_GAP[Math.min(this.tries || 0, RETRY_GAP.length - 1)];
    this.tries = (this.tries || 0) + 1;
    this.set('retrying');
    this.timer = this.timers.setTimeout(() => this.connect(true), gap * 1000);
    return undefined;
  }

  /** 页面回到前台：断着的马上再试，不等退避的那几秒；开着却好一阵没听到对面的（手机把页面挂起过，连接多半已经死了）
   *  当成断了，不等 60 秒的那次检查 */
  nudge() {
    if (this.state === 'open' && this.link && Date.now() - this.link.heard > 25000) this.link.finish('lost');
    if (this.state !== 'retrying') return;
    this.timers.clearTimeout(this.timer);
    this.connect(true);
  }

  stop(why) {
    this.attempt = null;
    this.timers.clearTimeout(this.timer);
    this.set('closed', why);
  }

  /** 敲的键、粘贴的字。回：送出去了没有（断着、还没接上时不送，也不攒着等接上——迟到的按键比丢了的更糟） */
  input(data) {
    if (this.state !== 'open' || !this.link) return false;
    this.link.input(typeof data === 'string' ? utf8(data) : data);
    return true;
  }

  resize(cols, rows) {
    if (!validSize(cols, rows)) return;
    this.cols = cols;
    this.rows = rows;
    this.link?.resize(cols, rows);
  }

  /** 自己断开（会话接着运行） */
  detach() {
    if (this.state === 'closed') return;
    const link = this.link;
    this.attempt = null;
    this.timers.clearTimeout(this.timer);
    if (link) link.detach();
    this.set('closed', 'detached');
  }
}
