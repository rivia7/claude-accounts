//! 假的推送服务（苹果的 APNs）：给推送网关（`cca-core team-push`，docs/dev/push.md）的测试用。HTTP/2（RFC 9113）和 HPACK
//! （RFC 7541）照规范自己写的服务端那一头（不用辅助程序里的 h2.rs），每个请求照苹果的文档（Sending notification requests to
//! APNs、Establishing a token-based connection to APNs、Handling notification responses from APNs）核对：
//! - TLS 的 ALPN 要是 h2；连接开头那一句、SETTINGS（客户端的收到就 ACK；它说动态表是 0 的话，下一个回的头开头带上大小的更新）；
//!   流的编号是单数、一个比一个大；DATA 不能超过给它的窗口（客户端 ACK 了这边的 SETTINGS 以后，新开的流照这边说的初始窗口算）
//! - 请求头：`:method` POST、`:scheme` https、`:authority`、`:path` 是 `/3/device/<十六进制的令牌>`、
//!   `authorization: bearer <JWT>`、`apns-push-type` alert、`apns-topic`（App 的 bundle id）、`apns-priority` 10 或 5、
//!   `apns-expiration` 是数、`apns-collapse-id` 不超过 64 个字节；同一个名字不能有两个
//! - JWT：头是 `{"alg":"ES256","kid":<Key ID>}`、内容是 `{"iss":<Team ID>,"iat":<签发的时间>}`，签名拿 .p8 的公钥验
//!   （ES256：r‖s）；签发超过一小时的是 ExpiredProviderToken
//! - 内容：不超过 4096 个字节（[`PAYLOAD_MAX`]）的 JSON，`aps.alert` 有、`aps.mutable-content` 是 1
//!
//! 不对的照苹果的说法回（403 InvalidProviderToken、400 BadTopic……；内容不是那个样子的回 400 BadPayload：这一句是这里自己起的，
//! 苹果的文档里没有）。照规矩的请求回什么由令牌的最后两个十六进制字符定（[`by_tail`]），每个请求记一条 [`Seen`]；
//! 客户端不照 HTTP/2 来的记在 [`FakeApns::complaints`]（用例最后看它是空的）。回的头故意用几种写法（静态表的编号、
//! 带索引 / 不进表 / 永不进表的原文、压缩过的数字、名字也是原文的、不在静态表里的 `apns-id`），客户端都要认得。
//! 压缩过（Huffman）的请求头这里不解：推送网关从来不压缩，收到了说明它改了写法（记一条，连接关掉）。
//!
//! - [`auth_key`]：造一把苹果那种样子的 .p8（PKCS#8，里面的 ECPrivateKey 带着曲线的参数和公钥），给出公钥
//! - [`FakeApns::start`]：在 127.0.0.1 的空闲端口上起，用给的证书（测试的证书机构签的：kit::pki::make_ca_cert）
//! - [`Gateway::start`]：两个假推送服务（正式、开发环境）、推送网关自己的证书（自签的 P-256：Team 的服务器钉住它）和 .p8，
//!   起好 `cca-core team-push`（`--ca` 认测试的证书机构）；[`Gateway::post`] 像 Team 的服务器那样交一条

use crate::fleet::ServerProc;
use crate::http::{Request, Response};
use crate::json::{self, Style, Value};
use crate::{crypto, fsx, pki, server, tls, tlsd};
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, UnparsedPublicKey, ECDSA_P256_SHA256_FIXED, ECDSA_P256_SHA256_FIXED_SIGNING};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use std::collections::{HashMap, VecDeque};
use std::io::{self, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 推送服务一条通知最多多少个字节（苹果的文档：4 KB）
pub const PAYLOAD_MAX: usize = 4096;

const PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
// 帧的种类（RFC 9113 6）
const DATA: u8 = 0x0;
const HEADERS: u8 = 0x1;
const PRIORITY: u8 = 0x2;
const RST_STREAM: u8 = 0x3;
const SETTINGS: u8 = 0x4;
const PUSH_PROMISE: u8 = 0x5;
const PING: u8 = 0x6;
const GOAWAY: u8 = 0x7;
const WINDOW_UPDATE: u8 = 0x8;
const CONTINUATION: u8 = 0x9;
// 标志
const END_STREAM: u8 = 0x1;
const ACK: u8 = 0x1;
const END_HEADERS: u8 = 0x4;
const PADDED: u8 = 0x8;
const PRIORITY_FLAG: u8 = 0x20;
// 错误码（RFC 9113 7）
const NO_ERROR: u32 = 0x0;
const PROTOCOL_ERROR: u32 = 0x1;
const REFUSED_STREAM: u32 = 0x7;
/// 这边收的一帧最长多少（没另外说：SETTINGS_MAX_FRAME_SIZE 的默认值）
const MAX_FRAME: usize = 16_384;
/// 窗口的默认值（RFC 9113 6.9.2）
const DEFAULT_WINDOW: i64 = 65_535;
/// 动态表的默认大小（RFC 7541 4.2：SETTINGS_HEADER_TABLE_SIZE 的默认值）
const TABLE_MAX: usize = 4096;
/// 连接上多久没有动静就关
const IDLE: Duration = Duration::from_secs(60);

/// HPACK 的静态表（RFC 7541 附录 A），第 1 项起
const STATIC: [(&str, &str); 61] = [
    (":authority", ""),
    (":method", "GET"),
    (":method", "POST"),
    (":path", "/"),
    (":path", "/index.html"),
    (":scheme", "http"),
    (":scheme", "https"),
    (":status", "200"),
    (":status", "204"),
    (":status", "206"),
    (":status", "304"),
    (":status", "400"),
    (":status", "404"),
    (":status", "500"),
    ("accept-charset", ""),
    ("accept-encoding", "gzip, deflate"),
    ("accept-language", ""),
    ("accept-ranges", ""),
    ("accept", ""),
    ("access-control-allow-origin", ""),
    ("age", ""),
    ("allow", ""),
    ("authorization", ""),
    ("cache-control", ""),
    ("content-disposition", ""),
    ("content-encoding", ""),
    ("content-language", ""),
    ("content-length", ""),
    ("content-location", ""),
    ("content-range", ""),
    ("content-type", ""),
    ("cookie", ""),
    ("date", ""),
    ("etag", ""),
    ("expect", ""),
    ("expires", ""),
    ("from", ""),
    ("host", ""),
    ("if-match", ""),
    ("if-modified-since", ""),
    ("if-none-match", ""),
    ("if-range", ""),
    ("if-unmodified-since", ""),
    ("last-modified", ""),
    ("link", ""),
    ("location", ""),
    ("max-forwards", ""),
    ("proxy-authenticate", ""),
    ("proxy-authorization", ""),
    ("range", ""),
    ("referer", ""),
    ("refresh", ""),
    ("retry-after", ""),
    ("server", ""),
    ("set-cookie", ""),
    ("strict-transport-security", ""),
    ("transfer-encoding", ""),
    ("user-agent", ""),
    ("vary", ""),
    ("via", ""),
    ("www-authenticate", ""),
];

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// 路径当参数用（临时目录都是 UTF-8 的）
fn text(path: &Path) -> String {
    path.to_str().unwrap_or_else(|| panic!("路径不是 UTF-8：{}", path.display())).to_string()
}

// ---- 推送服务的密钥 ----

/// 造一把苹果那种样子的推送服务的密钥写到 path（权限 600），给出它的公钥（65 个字节：04 X Y）。开发者账号里下载的 .p8 是
/// PKCS#8，里面的 ECPrivateKey 带着曲线的参数 [0] 和公钥 [1]（`MIGTAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBHkwdwIBAQQg` 开头）；ring 生成的
/// 没有 [0]，这里拿它的私钥和公钥照苹果的样子重新拼
pub fn auth_key(path: &Path) -> Vec<u8> {
    let rng = SystemRandom::new();
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap_or_else(|e| panic!("造不了私钥：{e}"));
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
        .unwrap_or_else(|e| panic!("ring 自己造的私钥读不回来：{e}"));
    let public = pair.public_key().as_ref().to_vec();
    // ring 的：30 81 87 02 01 00 <算法> 04 6d 30 6b 02 01 01 04 20 <私钥> a1 44 03 42 00 <公钥>，私钥从第 36 个字节起
    let der = pkcs8.as_ref();
    assert_eq!((der.len(), public.len()), (138, 65), "ring 生成的 PKCS#8 不是认得的样子");
    let scalar = &der[36..68];
    let curve = [0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
    let mut ec = vec![0x30, 0x77, 0x02, 0x01, 0x01, 0x04, 0x20];
    ec.extend_from_slice(scalar);
    ec.extend_from_slice(&[0xa0, 0x0a]);
    ec.extend_from_slice(&curve);
    ec.extend_from_slice(&[0xa1, 0x44, 0x03, 0x42, 0x00]);
    ec.extend_from_slice(&public);
    let mut out = vec![0x30, 0x81, 0x93, 0x02, 0x01, 0x00, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
    out.extend_from_slice(&curve);
    out.extend_from_slice(&[0x04, 0x79]);
    out.extend_from_slice(&ec);
    assert_eq!(out.len(), 150);
    let b64 = crypto::b64(&out);
    let lines: Vec<&str> = b64.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap_or_default()).collect();
    fsx::write_mode(path, format!("-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n", lines.join("\n")), 0o600);
    public
}

/// JWT（`bearer ` 后面那一段，带不带 `bearer ` 都行）的头和内容；不成样子就 panic
pub fn jwt_parts(jwt: &str) -> (Value, Value) {
    let jwt = jwt.strip_prefix("bearer ").unwrap_or(jwt);
    let part = |i: usize| {
        let p = jwt.split('.').nth(i).unwrap_or_else(|| panic!("JWT 不是三段：{jwt}"));
        let raw = crypto::b64url_nopad_decode(p).unwrap_or_else(|e| panic!("JWT 的第 {i} 段不是 base64url（{e}）：{jwt}"));
        json::parse_bytes(&raw).unwrap_or_else(|e| panic!("JWT 的第 {i} 段不是 JSON（{e}）：{jwt}"))
    };
    (part(0), part(1))
}

// ---- HPACK ----

/// 带前缀的整数（RFC 7541 5.1）：b[at] 的低 prefix 位起；(值, 后面从哪起)
fn read_int(b: &[u8], at: usize, prefix: u32) -> Result<(usize, usize), String> {
    let max = (1usize << prefix) - 1;
    let first = usize::from(*b.get(at).ok_or("整数没写完")?) & max;
    if first < max {
        return Ok((first, at + 1));
    }
    let (mut value, mut shift, mut i) = (max, 0u32, at + 1);
    loop {
        let byte = *b.get(i).ok_or("整数没写完")?;
        value += usize::from(byte & 0x7f) << shift;
        i += 1;
        if byte & 0x80 == 0 {
            return Ok((value, i));
        }
        shift += 7;
        if shift > 28 {
            return Err("整数太大".into());
        }
    }
}

fn put_int(out: &mut Vec<u8>, first: u8, prefix: u32, value: usize) {
    let max = (1usize << prefix) - 1;
    if value < max {
        out.push(first | value as u8);
        return;
    }
    out.push(first | max as u8);
    let mut rest = value - max;
    while rest >= 0x80 {
        out.push((rest & 0x7f) as u8 | 0x80);
        rest >>= 7;
    }
    out.push(rest as u8);
}

/// 不压缩的字符串
fn put_raw(out: &mut Vec<u8>, s: &[u8]) {
    put_int(out, 0x00, 7, s.len());
    out.extend_from_slice(s);
}

/// 压缩过的一串数字（RFC 7541 附录 B：'0'–'2' 是 5 位的 00000–00010，'3'–'9' 是 6 位的 011001–011111），最后用 1 补满一个字节
fn put_huffman_digits(out: &mut Vec<u8>, digits: &str) {
    let (mut bits, mut n) = (0u64, 0u32);
    for c in digits.bytes() {
        assert!(c.is_ascii_digit(), "只会压缩数字：{digits}");
        let (code, len) = if c <= b'2' { (u64::from(c - b'0'), 5) } else { (0x19 + u64::from(c - b'3'), 6) };
        bits = bits << len | code;
        n += len;
    }
    let pad = (8 - n % 8) % 8;
    bits = bits << pad | ((1u64 << pad) - 1);
    n += pad;
    let bytes: Vec<u8> = (0..n / 8).rev().map(|i| (bits >> (i * 8)) as u8).collect();
    put_int(out, 0x80, 7, bytes.len());
    out.extend(bytes);
}

/// HPACK 的解码这一头（RFC 7541 3、6）：静态表、动态表（带索引的原文进表，超了从最旧的丢）、开头的大小更新
struct Decoder {
    table: VecDeque<(String, String)>,
    size: usize,
    max: usize,
}

impl Decoder {
    fn new() -> Decoder {
        Decoder { table: VecDeque::new(), size: 0, max: TABLE_MAX }
    }

    fn entry(&self, index: usize) -> Result<(String, String), String> {
        match index {
            0 => Err("编号是 0".into()),
            1..=61 => Ok((STATIC[index - 1].0.to_string(), STATIC[index - 1].1.to_string())),
            _ => self.table.get(index - 62).cloned().ok_or_else(|| format!("表里没有第 {index} 项")),
        }
    }

    fn evict(&mut self) {
        while self.size > self.max {
            let Some((n, v)) = self.table.pop_back() else { break };
            self.size -= n.len() + v.len() + 32;
        }
    }

    fn string(b: &[u8], at: usize) -> Result<(String, usize), String> {
        let huffman = *b.get(at).ok_or("字符串没写完")? & 0x80 != 0;
        let (len, at) = read_int(b, at, 7)?;
        let raw = b.get(at..at + len).ok_or("字符串没写完")?;
        if huffman {
            return Err("请求头里有压缩过（Huffman）的字符串：推送网关不该压缩".into());
        }
        Ok((String::from_utf8(raw.to_vec()).map_err(|_| "字符串不是 UTF-8")?, at + len))
    }

    fn literal(&self, b: &[u8], index: usize, at: usize) -> Result<((String, String), usize), String> {
        let (name, at) = match index {
            0 => Decoder::string(b, at)?,
            i => (self.entry(i)?.0, at),
        };
        let (value, at) = Decoder::string(b, at)?;
        Ok(((name, value), at))
    }

    fn decode(&mut self, b: &[u8]) -> Result<Vec<(String, String)>, String> {
        let (mut out, mut at) = (Vec::new(), 0);
        while at < b.len() {
            let first = b[at];
            if first & 0x80 != 0 {
                let (i, next) = read_int(b, at, 7)?;
                out.push(self.entry(i)?);
                at = next;
            } else if first & 0x40 != 0 {
                let (i, next) = read_int(b, at, 6)?;
                let (field, next) = self.literal(b, i, next)?;
                self.size += field.0.len() + field.1.len() + 32;
                self.table.push_front(field.clone());
                self.evict();
                out.push(field);
                at = next;
            } else if first & 0x20 != 0 {
                if !out.is_empty() {
                    return Err("动态表大小的更新不在头的开头（RFC 7541 4.2）".into());
                }
                let (size, next) = read_int(b, at, 5)?;
                if size > TABLE_MAX {
                    return Err(format!("动态表改成 {size}：比这边说的大"));
                }
                self.max = size;
                self.evict();
                at = next;
            } else {
                // 0000xxxx 不进表、0001xxxx 永不进表
                let (i, next) = read_int(b, at, 4)?;
                let (field, next) = self.literal(b, i, next)?;
                out.push(field);
                at = next;
            }
        }
        Ok(out)
    }
}

/// 回的头：:status 照状态码换着几种写法，再加一个 apns-id。resize：开头先说动态表改成 0（客户端在 SETTINGS 里说的）
fn reply_block(status: u16, resize: bool) -> Vec<u8> {
    let mut out = Vec::new();
    if resize {
        out.push(0x20);
    }
    let digits = status.to_string();
    match status {
        200 => out.push(0x88),
        400 => out.push(0x8c),
        404 => out.push(0x8d),
        500 => out.push(0x8e),
        // 带索引的原文、名字是静态表的第 8 项、值不压缩（客户端的表是 0：进了就丢）
        403 => {
            out.push(0x48);
            put_raw(&mut out, digits.as_bytes());
        }
        // 不进表的原文，值压缩过
        410 | 413 => {
            out.push(0x08);
            put_huffman_digits(&mut out, &digits);
        }
        // 永不进表的原文
        429 => {
            out.push(0x18);
            put_raw(&mut out, digits.as_bytes());
        }
        // 名字也是原文，值压缩过
        _ => {
            out.push(0x00);
            put_raw(&mut out, b":status");
            put_huffman_digits(&mut out, &digits);
        }
    }
    out.push(0x00);
    put_raw(&mut out, b"apns-id");
    put_raw(&mut out, crypto::uuid4().to_uppercase().as_bytes());
    out
}

// ---- 推送服务 ----

/// .p8 的那一套：Key ID、Team ID、App 的 bundle id、公钥（65 个字节）
#[derive(Clone, Debug)]
pub struct Keys {
    pub key_id: String,
    pub team_id: String,
    pub topic: String,
    pub public: Vec<u8>,
}

/// 开关
#[derive(Clone, Copy, Debug)]
pub struct Knobs {
    /// 在 SETTINGS 里给客户端的流的初始窗口（默认 65535）：小了的话客户端要一段段地发、等 WINDOW_UPDATE
    pub window: u32,
}

impl Default for Knobs {
    fn default() -> Knobs {
        Knobs { window: 65_535 }
    }
}

/// 收到的一个请求和回的
#[derive(Clone, Debug)]
pub struct Seen {
    /// 第几条连接（从 1 数）
    pub conn: usize,
    /// 流的编号
    pub stream: u32,
    /// 请求头，照收到的先后
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// 内容分成了几个 DATA 帧
    pub data_frames: usize,
    /// 回的状态码和原因（没处理就叫它再发的：0 和 REFUSED_STREAM）
    pub status: u16,
    pub reason: String,
}

impl Seen {
    /// 这个名字的头（没有是空的）
    pub fn header(&self, name: &str) -> &str {
        self.headers.iter().find(|(n, _)| n == name).map_or("", |(_, v)| v.as_str())
    }

    /// :path 里的设备令牌
    pub fn token(&self) -> &str {
        self.header(":path").strip_prefix("/3/device/").unwrap_or_default()
    }

    /// 内容（JSON）；不是 JSON 就 panic
    pub fn json(&self) -> Value {
        json::parse_bytes(&self.body)
            .unwrap_or_else(|e| panic!("推送服务收到的不是 JSON（{e}）：{}", String::from_utf8_lossy(&self.body)))
    }
}

/// 照规矩的请求怎么回
#[derive(Clone, Debug, PartialEq)]
enum Answer {
    Status(u16, &'static str),
    /// RST_STREAM REFUSED_STREAM：没处理，可以再发
    Refuse,
}

/// 回完以后这条连接怎么办
#[derive(Clone, Copy, Debug, PartialEq)]
enum After {
    Stay,
    /// 先说 GOAWAY（这条连接不再收新的流；最后一个是这一个），再回，等对面关
    GoAway,
    /// 直接把连接断了（没说 GOAWAY：像闲了太久被掐掉的那样）
    Close,
}

struct Shared {
    keys: Keys,
    knobs: Knobs,
    seen: Mutex<Vec<Seen>>,
    complaints: Mutex<Vec<String>>,
    /// 每个令牌来过几回
    tries: Mutex<HashMap<String, usize>>,
    conns: AtomicUsize,
}

impl Shared {
    fn complain(&self, why: String) {
        locked(&self.complaints).push(why);
    }

    /// 令牌的最后两个十六进制字符定回什么（每个令牌第几回由 tries 数着）：
    /// - `bb` 410 Unregistered；`cc` 400 BadDeviceToken；`ee` 429 TooManyRequests；`ff` 503 ServiceUnavailable
    /// - `dd`：这个令牌头一回 403 ExpiredProviderToken，以后 200
    /// - `66`：这个令牌头一回 RST_STREAM（REFUSED_STREAM），以后 200
    /// - `77`：先说 GOAWAY（最后一个流就是这一个）再回 200；`88`：200，回完把连接断了
    /// - 别的（`aa`）：200
    fn by_tail(&self, token: &str) -> (Answer, After) {
        let n = {
            let mut tries = locked(&self.tries);
            let n = tries.entry(token.to_string()).or_insert(0);
            *n += 1;
            *n
        };
        match token.get(token.len().saturating_sub(2)..).unwrap_or_default() {
            "bb" => (Answer::Status(410, "Unregistered"), After::Stay),
            "cc" => (Answer::Status(400, "BadDeviceToken"), After::Stay),
            "dd" if n == 1 => (Answer::Status(403, "ExpiredProviderToken"), After::Stay),
            "ee" => (Answer::Status(429, "TooManyRequests"), After::Stay),
            "ff" => (Answer::Status(503, "ServiceUnavailable"), After::Stay),
            "66" if n == 1 => (Answer::Refuse, After::Stay),
            "77" => (Answer::Status(200, ""), After::GoAway),
            "88" => (Answer::Status(200, ""), After::Close),
            _ => (Answer::Status(200, ""), After::Stay),
        }
    }

    /// JWT 验得过没有：不过是苹果的那句原因
    fn jwt_ok(&self, jwt: &str, now: i64) -> Result<(), &'static str> {
        let parts: Vec<&str> = jwt.split('.').collect();
        let &[head, claims, sig] = parts.as_slice() else { return Err("InvalidProviderToken") };
        let part = |p: &str| crypto::b64url_nopad_decode(p).ok().and_then(|raw| json::parse_bytes(&raw).ok());
        let (Some(h), Some(c), Ok(sig)) = (part(head), part(claims), crypto::b64url_nopad_decode(sig)) else {
            return Err("InvalidProviderToken");
        };
        if h.get("alg").and_then(Value::as_str) != Some("ES256") || h.get("kid").and_then(Value::as_str) != Some(&self.keys.key_id)
        {
            return Err("InvalidProviderToken");
        }
        if c.get("iss").and_then(Value::as_str) != Some(&self.keys.team_id) {
            return Err("InvalidProviderToken");
        }
        let Some(iat) = c.get("iat").and_then(Value::as_i64) else { return Err("InvalidProviderToken") };
        let signed = format!("{head}.{claims}");
        if UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &self.keys.public).verify(signed.as_bytes(), &sig).is_err() {
            return Err("InvalidProviderToken");
        }
        if now - iat > 3600 {
            return Err("ExpiredProviderToken");
        }
        Ok(())
    }

    /// 一个收齐了的请求：照苹果的文档核对，再照令牌定回什么
    fn judge(&self, h: &[(String, String)], body: &[u8], now: i64) -> (Answer, After) {
        let fail = |status: u16, reason: &'static str| (Answer::Status(status, reason), After::Stay);
        let mut names: Vec<&str> = h.iter().map(|(n, _)| n.as_str()).collect();
        names.sort_unstable();
        if names.windows(2).any(|w| w[0] == w[1]) {
            return fail(400, "DuplicateHeaders");
        }
        let one = |name: &str| h.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str());
        if one(":method") != Some("POST") {
            return fail(405, "MethodNotAllowed");
        }
        if one(":scheme") != Some("https") || one(":authority").is_none_or(str::is_empty) {
            return fail(400, "BadPath");
        }
        let Some(token) = one(":path").and_then(|p| p.strip_prefix("/3/device/")) else { return fail(404, "BadPath") };
        if !(64..=200).contains(&token.len()) || !token.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return fail(400, "BadDeviceToken");
        }
        let Some(auth) = one("authorization") else { return fail(403, "MissingProviderToken") };
        let Some(jwt) = auth.strip_prefix("bearer ") else { return fail(403, "InvalidProviderToken") };
        if let Err(reason) = self.jwt_ok(jwt, now) {
            return fail(403, reason);
        }
        match one("apns-topic") {
            None => return fail(400, "MissingTopic"),
            Some(topic) if topic != self.keys.topic => return fail(400, "BadTopic"),
            Some(_) => {}
        }
        if one("apns-push-type") != Some("alert") {
            return fail(400, "InvalidPushType");
        }
        if !matches!(one("apns-priority"), None | Some("10" | "5")) {
            return fail(400, "BadPriority");
        }
        if one("apns-expiration").is_some_and(|e| e.is_empty() || !e.bytes().all(|b| b.is_ascii_digit())) {
            return fail(400, "BadExpirationDate");
        }
        if one("apns-collapse-id").is_some_and(|c| c.len() > 64) {
            return fail(400, "BadCollapseId");
        }
        if body.is_empty() {
            return fail(400, "PayloadEmpty");
        }
        if body.len() > PAYLOAD_MAX {
            return fail(413, "PayloadTooLarge");
        }
        let Ok(doc) = json::parse_bytes(body) else { return fail(400, "BadPayload") };
        let aps = doc.get("aps");
        if aps.and_then(|a| a.get("alert")).is_none()
            || aps.and_then(|a| a.get("mutable-content")).and_then(Value::as_i64) != Some(1)
        {
            return fail(400, "BadPayload");
        }
        self.by_tail(token)
    }
}

/// 收到的一帧
struct Frame {
    kind: u8,
    flags: u8,
    stream: u32,
    payload: Vec<u8>,
}

/// 正在收的那个请求
struct Open {
    id: u32,
    /// 头还没收齐时攒着的
    block: Vec<u8>,
    headers: Option<Vec<(String, String)>>,
    body: Vec<u8>,
    data_frames: usize,
    /// 这个流还能收多少
    window: i64,
    ended: bool,
}

/// 去掉 PADDED 的填充（第一个字节是填充的长度）
fn unpad(p: &[u8], flags: u8) -> Result<&[u8], String> {
    if flags & PADDED == 0 {
        return Ok(p);
    }
    let (&pad, rest) = p.split_first().ok_or("PADDED 的帧是空的")?;
    rest.len().checked_sub(usize::from(pad)).map(|end| &rest[..end]).ok_or_else(|| "填充比帧还长".to_string())
}

/// 一条连接（握完手以后）
struct H2<'a> {
    s: StreamOwned<ServerConnection, TcpStream>,
    sh: &'a Shared,
    conn: usize,
    dec: Decoder,
    /// 下一个回的头开头带上动态表大小的更新
    resize: bool,
    /// 最近开的流
    last: u32,
    open: Option<Open>,
    /// 新开的流的初始窗口：客户端 ACK 了这边的 SETTINGS 以后才是 knobs.window
    stream_init: i64,
    /// 连接这一级还能收多少
    conn_window: i64,
}

impl H2<'_> {
    fn read_full(&mut self, n: usize) -> Result<Option<Vec<u8>>, String> {
        let mut buf = vec![0u8; n];
        let mut got = 0;
        while got < n {
            match self.s.read(&mut buf[got..]) {
                Ok(0) if got == 0 => return Ok(None),
                Ok(0) => return Err("帧没读完对面就关了".into()),
                Ok(k) => got += k,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                // 对面关连接时不说 close_notify、断开、闲太久：在帧和帧之间的话就当它关了
                Err(e) if got == 0 => {
                    let _ = e;
                    return Ok(None);
                }
                Err(e) => return Err(format!("读不了：{e}")),
            }
        }
        Ok(Some(buf))
    }

    fn frame(&mut self) -> Result<Option<Frame>, String> {
        let Some(h) = self.read_full(9)? else { return Ok(None) };
        let len = usize::from(h[0]) << 16 | usize::from(h[1]) << 8 | usize::from(h[2]);
        if len > MAX_FRAME {
            return Err(format!("帧有 {len} 个字节：超过了 {MAX_FRAME}（FRAME_SIZE_ERROR）"));
        }
        let stream = u32::from_be_bytes([h[5] & 0x7f, h[6], h[7], h[8]]);
        let payload = if len == 0 { Vec::new() } else { self.read_full(len)?.ok_or("帧没读完对面就关了")? };
        Ok(Some(Frame { kind: h[3], flags: h[4], stream, payload }))
    }

    fn send(&mut self, kind: u8, flags: u8, stream: u32, payload: &[u8]) -> Result<(), String> {
        let mut out = Vec::with_capacity(9 + payload.len());
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes()[1..]);
        out.extend_from_slice(&[kind, flags]);
        out.extend_from_slice(&stream.to_be_bytes());
        out.extend_from_slice(payload);
        self.s.write_all(&out).and_then(|()| self.s.flush()).map_err(|e| format!("写不了：{e}"))
    }

    fn goaway(&mut self, code: u32) {
        let mut p = self.last.to_be_bytes().to_vec();
        p.extend_from_slice(&code.to_be_bytes());
        let _ = self.send(GOAWAY, 0, 0, &p);
    }

    fn run(&mut self) -> Result<(), String> {
        match self.read_full(PREFACE.len())? {
            Some(p) if p == PREFACE => {}
            Some(p) => return Err(format!("开头那一句不对：{:?}", String::from_utf8_lossy(&p))),
            None => return Ok(()),
        }
        let mut mine = Vec::new();
        for (key, value) in [(0x3u16, 1000u32), (0x4, self.sh.knobs.window), (0x6, 8000)] {
            mine.extend_from_slice(&key.to_be_bytes());
            mine.extend_from_slice(&value.to_be_bytes());
        }
        self.send(SETTINGS, 0, 0, &mine)?;
        loop {
            let Some(f) = self.frame()? else { return Ok(()) };
            if let Some(open) = &self.open {
                if open.headers.is_none() && !(f.kind == CONTINUATION && f.stream == open.id) {
                    return Err("头没收齐（没有 END_HEADERS）就来了别的帧".into());
                }
            }
            let after = match f.kind {
                SETTINGS if f.flags & ACK != 0 => {
                    let delta = i64::from(self.sh.knobs.window) - self.stream_init;
                    self.stream_init = i64::from(self.sh.knobs.window);
                    if let Some(open) = self.open.as_mut() {
                        open.window += delta;
                    }
                    After::Stay
                }
                SETTINGS => {
                    self.settings(&f.payload)?;
                    self.send(SETTINGS, ACK, 0, &[])?;
                    After::Stay
                }
                PING if f.flags & ACK == 0 => {
                    self.send(PING, ACK, 0, &f.payload)?;
                    After::Stay
                }
                HEADERS => self.headers(f)?,
                CONTINUATION => self.continuation(f)?,
                DATA => self.data(f)?,
                RST_STREAM => {
                    if self.open.as_ref().is_some_and(|o| o.id == f.stream) {
                        self.open = None;
                    }
                    After::Stay
                }
                GOAWAY => return Ok(()),
                PUSH_PROMISE => return Err("客户端发了 PUSH_PROMISE".into()),
                // PING 的 ACK、WINDOW_UPDATE、PRIORITY、不认识的种类（RFC 9113 4.1：不认识的不管）
                _ => {
                    let _ = PRIORITY;
                    After::Stay
                }
            };
            match after {
                After::Stay => {}
                After::GoAway => {
                    // GOAWAY 在回复的前面就说了（finish）：客户端读到回复以前一定先读到它。等它关（最多 5 秒）；
                    // 它要是还在这条连接上开新的流，记下来
                    let _ = self.s.sock.set_read_timeout(Some(Duration::from_secs(5)));
                    while let Ok(Some(f)) = self.frame() {
                        if f.kind == HEADERS {
                            return Err("说了 GOAWAY 以后还开新的流".into());
                        }
                    }
                    return Ok(());
                }
                After::Close => {
                    let _ = self.s.sock.shutdown(Shutdown::Both);
                    return Ok(());
                }
            }
        }
    }

    fn settings(&mut self, p: &[u8]) -> Result<(), String> {
        if !p.len().is_multiple_of(6) {
            return Err("SETTINGS 的长度不是 6 的倍数".into());
        }
        for e in p.chunks(6) {
            let key = u16::from_be_bytes([e[0], e[1]]);
            let value = u32::from_be_bytes([e[2], e[3], e[4], e[5]]);
            match key {
                0x1 if (value as usize) < TABLE_MAX => self.resize = true,
                0x2 if value > 1 => return Err(format!("ENABLE_PUSH 是 {value}")),
                _ => {}
            }
        }
        Ok(())
    }

    fn headers(&mut self, f: Frame) -> Result<After, String> {
        if f.stream.is_multiple_of(2) || f.stream <= self.last {
            return Err(format!("流的编号 {} 不对（上一个是 {}）", f.stream, self.last));
        }
        if self.open.is_some() {
            return Err("上一个请求还没发完就开了下一个流".into());
        }
        self.last = f.stream;
        let mut p = unpad(&f.payload, f.flags)?;
        if f.flags & PRIORITY_FLAG != 0 {
            p = p.get(5..).ok_or("PRIORITY 的那 5 个字节不全")?;
        }
        let mut open = Open {
            id: f.stream,
            block: p.to_vec(),
            headers: None,
            body: Vec::new(),
            data_frames: 0,
            window: self.stream_init,
            ended: f.flags & END_STREAM != 0,
        };
        if f.flags & END_HEADERS != 0 {
            open.headers = Some(self.dec.decode(&open.block)?);
        }
        self.open = Some(open);
        self.finish()
    }

    fn continuation(&mut self, f: Frame) -> Result<After, String> {
        let mut open =
            self.open.take().filter(|o| o.id == f.stream && o.headers.is_none()).ok_or("CONTINUATION 前面没有没收齐的头")?;
        open.block.extend_from_slice(&f.payload);
        if f.flags & END_HEADERS != 0 {
            open.headers = Some(self.dec.decode(&open.block)?);
        }
        self.open = Some(open);
        self.finish()
    }

    fn data(&mut self, f: Frame) -> Result<After, String> {
        let mut open = self
            .open
            .take()
            .filter(|o| o.id == f.stream && o.headers.is_some() && !o.ended)
            .ok_or_else(|| format!("流 {} 上来了 DATA：它没开着", f.stream))?;
        let len = f.payload.len() as i64;
        if len > open.window || len > self.conn_window {
            return Err(format!(
                "DATA 有 {len} 个字节：超过了窗口（流 {}、连接 {}；FLOW_CONTROL_ERROR）",
                open.window, self.conn_window
            ));
        }
        open.window -= len;
        self.conn_window -= len;
        open.body.extend_from_slice(unpad(&f.payload, f.flags)?);
        open.data_frames += 1;
        open.ended = f.flags & END_STREAM != 0;
        // 窗口还回去：连接的每次都还；流的只在流还没完、窗口给得小的时候还（不然它一直等）
        if len > 0 {
            self.send(WINDOW_UPDATE, 0, 0, &(len as u32).to_be_bytes())?;
            self.conn_window += len;
            if !open.ended && i64::from(self.sh.knobs.window) < DEFAULT_WINDOW {
                self.send(WINDOW_UPDATE, 0, open.id, &(len as u32).to_be_bytes())?;
                open.window += len;
            }
        }
        self.open = Some(open);
        self.finish()
    }

    /// 请求收齐了的话：核对、回、记下
    fn finish(&mut self) -> Result<After, String> {
        if !self.open.as_ref().is_some_and(|o| o.ended && o.headers.is_some()) {
            return Ok(After::Stay);
        }
        let Some(Open { id, headers: Some(headers), body, data_frames, .. }) = self.open.take() else { return Ok(After::Stay) };
        let (answer, after) = self.sh.judge(&headers, &body, unix_now());
        if after == After::GoAway {
            // 最后一个流就是这一个：它照样回完，以后的不收
            self.goaway(NO_ERROR);
        }
        let (status, reason) = match &answer {
            Answer::Refuse => {
                self.send(RST_STREAM, 0, id, &REFUSED_STREAM.to_be_bytes())?;
                (0, "REFUSED_STREAM")
            }
            Answer::Status(status, reason) => {
                let block = reply_block(*status, std::mem::take(&mut self.resize));
                let text = match (*status, *reason) {
                    (_, "") => String::new(),
                    (410, reason) => format!(r#"{{"reason":"{reason}","timestamp":{}}}"#, unix_now() * 1000),
                    (_, reason) => format!(r#"{{"reason":"{reason}"}}"#),
                };
                self.send(HEADERS, END_HEADERS | if text.is_empty() { END_STREAM } else { 0 }, id, &block)?;
                if !text.is_empty() {
                    self.send(DATA, END_STREAM, id, text.as_bytes())?;
                }
                (*status, *reason)
            }
        };
        let seen = Seen { conn: self.conn, stream: id, headers, body, data_frames, status, reason: reason.to_string() };
        locked(&self.sh.seen).push(seen);
        Ok(after)
    }
}

/// 一个假推送服务
pub struct FakeApns {
    pub port: u16,
    sh: Arc<Shared>,
    stop: Arc<AtomicBool>,
}

impl FakeApns {
    /// 用 certs 里的 cert.pem、key.pem（kit::pki::make_ca_cert 造的：给 127.0.0.1、测试的证书机构签的），在 127.0.0.1 的空闲端口上起
    pub fn start(certs: &Path, keys: &Keys, knobs: Knobs) -> FakeApns {
        let acceptor = tlsd::Acceptor::from_pem_files(&certs.join("cert.pem"), &certs.join("key.pem"))
            .unwrap_or_else(|e| panic!("假推送服务的证书：{e}"));
        let mut cfg: ServerConfig = (*acceptor.config()).clone();
        cfg.alpn_protocols = vec![b"h2".to_vec()];
        let cfg = Arc::new(cfg);
        let l = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("假推送服务听不了：{e}"));
        let port = l.local_addr().unwrap_or_else(|e| panic!("{e}")).port();
        let sh = Arc::new(Shared {
            keys: keys.clone(),
            knobs,
            seen: Mutex::new(Vec::new()),
            complaints: Mutex::new(Vec::new()),
            tries: Mutex::new(HashMap::new()),
            conns: AtomicUsize::new(0),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let (sh2, stop2) = (Arc::clone(&sh), Arc::clone(&stop));
        std::thread::spawn(move || {
            for sock in l.incoming() {
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(sock) = sock else { continue };
                let n = sh2.conns.fetch_add(1, Ordering::SeqCst) + 1;
                let (sh, cfg) = (Arc::clone(&sh2), Arc::clone(&cfg));
                std::thread::spawn(move || serve(&sh, cfg, sock, n));
            }
        });
        FakeApns { port, sh, stop }
    }

    /// 推送网关的 --apns / --apns-sandbox 填的
    pub fn url(&self) -> String {
        format!("https://127.0.0.1:{}", self.port)
    }

    /// 到现在为止收到的请求
    pub fn seen(&self) -> Vec<Seen> {
        locked(&self.sh.seen).clone()
    }

    /// 等到收到 n 个请求（最多 limit），给出全部；没等到就 panic
    pub fn wait_seen(&self, n: usize, limit: Duration) -> Vec<Seen> {
        let end = Instant::now() + limit;
        loop {
            let seen = self.seen();
            if seen.len() >= n {
                return seen;
            }
            assert!(Instant::now() < end, "假推送服务等了 {limit:?} 只收到 {} 个请求（要 {n} 个）：{seen:?}", seen.len());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// 客户端不照 HTTP/2、HPACK 来的那些（空的才对）
    pub fn complaints(&self) -> Vec<String> {
        locked(&self.sh.complaints).clone()
    }

    /// 接进来过几条连接
    pub fn connections(&self) -> usize {
        self.sh.conns.load(Ordering::SeqCst)
    }
}

impl Drop for FakeApns {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // 叫醒等着接连接的那条线
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

/// 一条连接：握手（ALPN 要是 h2）、说 HTTP/2
fn serve(sh: &Shared, cfg: Arc<ServerConfig>, mut sock: TcpStream, n: usize) {
    let _ = sock.set_read_timeout(Some(IDLE));
    let _ = sock.set_nodelay(true);
    let Ok(mut conn) = ServerConnection::new(cfg) else { return };
    while conn.is_handshaking() {
        if conn.complete_io(&mut sock).is_err() {
            return;
        }
    }
    if conn.alpn_protocol() != Some(b"h2") {
        sh.complain(format!("第 {n} 条连接：ALPN 不是 h2（{:?}）", conn.alpn_protocol()));
        return;
    }
    let mut h2 = H2 {
        s: StreamOwned::new(conn, sock),
        sh,
        conn: n,
        dec: Decoder::new(),
        resize: false,
        last: 0,
        open: None,
        stream_init: DEFAULT_WINDOW,
        conn_window: DEFAULT_WINDOW,
    };
    if let Err(why) = h2.run() {
        h2.goaway(PROTOCOL_ERROR);
        sh.complain(format!("第 {n} 条连接：{why}"));
    }
}

// ---- 推送网关 ----

/// 两个假推送服务和一个起好的推送网关
pub struct Gateway {
    pub d: PathBuf,
    pub keys: Keys,
    /// 正式环境（--apns）
    pub production: FakeApns,
    /// 开发环境（--apns-sandbox）
    pub sandbox: FakeApns,
    pub proc: ServerProc,
    pub port: u16,
    /// 网关证书的公钥（Team 的服务器的 --push-pin）
    pub pin: String,
}

impl Gateway {
    pub const KEY_ID: &'static str = "KEYID12345";
    pub const TEAM_ID: &'static str = "TEAMID1234";
    pub const TOPIC: &'static str = "com.cca.remote";

    /// 在 d 里（已经有、空的）造好：apns/（假推送服务的证书和测试的证书机构 ca.pem）、gw/（网关自己的证书：自签的 P-256）、
    /// AuthKey_<Key ID>.p8（苹果那种样子，600），起两个假推送服务和推送网关（监听 127.0.0.1 的空闲端口）
    pub fn start(d: &Path, knobs: Knobs) -> Gateway {
        Gateway::start_with(d, knobs, &[])
    }

    /// 同 [`Gateway::start`]，网关多带 extra 这几个参数
    pub fn start_with(d: &Path, knobs: Knobs, extra: &[&str]) -> Gateway {
        let (apns, gw) = (d.join("apns"), d.join("gw"));
        for dir in [&apns, &gw] {
            std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("建 {}：{e}", dir.display()));
        }
        pki::make_ca_cert(&apns);
        pki::make_cert(&gw);
        let p8 = d.join(format!("AuthKey_{}.p8", Gateway::KEY_ID));
        let public = auth_key(&p8);
        let keys = Keys { key_id: Gateway::KEY_ID.into(), team_id: Gateway::TEAM_ID.into(), topic: Gateway::TOPIC.into(), public };
        let production = FakeApns::start(&apns, &keys, knobs);
        let sandbox = FakeApns::start(&apns, &keys, knobs);
        let port_file = d.join("gw.port");
        let mut args = vec![
            "--cert".to_string(),
            text(&gw.join("cert.pem")),
            "--key".into(),
            text(&gw.join("key.pem")),
            "--apns-key".into(),
            text(&p8),
            "--key-id".into(),
            Gateway::KEY_ID.into(),
            "--team-id".into(),
            Gateway::TEAM_ID.into(),
            "--listen".into(),
            "127.0.0.1:0".into(),
            "--port-file".into(),
            text(&port_file),
            "--apns".into(),
            production.url(),
            "--apns-sandbox".into(),
            sandbox.url(),
            "--ca".into(),
            text(&apns.join("ca.pem")),
        ];
        args.extend(extra.iter().map(|a| a.to_string()));
        let mut proc = ServerProc::spawn_with(server::push_argv(), &args, &[]);
        let written = proc.wait_file(&port_file, "推送网关起来", Duration::from_secs(30));
        let port = written.trim().parse().unwrap_or_else(|_| panic!("端口文件里写的不是端口：{written:?}"));
        let pin = pki::openssl_pin(gw.join("cert.pem"));
        Gateway { d: d.to_path_buf(), keys, production, sandbox, proc, port, pin }
    }

    /// Team 的服务器的 --push 填的
    pub fn url(&self) -> String {
        format!("https://127.0.0.1:{}", self.port)
    }

    /// 连网关用的 TLS 客户端：钉住它的公钥（和 Team 的服务器一样）
    pub fn client(&self) -> tls::Client {
        tls::Client::insecure().pin(&[&self.pin]).unwrap_or_else(|e| panic!("{e}"))
    }

    fn reply(r: &Response) -> (u16, Value) {
        let body: &[u8] = if r.body.is_empty() { b"{}" } else { &r.body };
        (r.status, json::parse_bytes(body).unwrap_or_else(|e| panic!("网关回的不是 JSON（{e}）：{}", r.text())))
    }

    /// 像 Team 的服务器那样 POST /v1/push：(状态码, 回的 JSON)
    pub fn post(&self, body: &[u8]) -> (u16, Value) {
        let r = Request::post(&format!("{}/v1/push", self.url()))
            .header("Content-Type", "application/json")
            .body(body.to_vec())
            .tls(self.client())
            .timeout(Duration::from_secs(60))
            .send()
            .unwrap_or_else(|e| panic!("{e}"));
        Gateway::reply(&r)
    }

    /// GET path
    pub fn get(&self, path: &str) -> (u16, Value) {
        let r = Request::get(&format!("{}{path}", self.url())).tls(self.client()).timeout(Duration::from_secs(30)).send();
        Gateway::reply(&r.unwrap_or_else(|e| panic!("{e}")))
    }

    /// 原样发一段请求（头写成什么样都行），读回它的回复
    pub fn raw(&self, request: &[u8]) -> (u16, Value) {
        let mut s = self.client().connect("127.0.0.1", self.port, Duration::from_secs(30)).unwrap_or_else(|e| panic!("{e}"));
        s.write_all(request).unwrap_or_else(|e| panic!("{e}"));
        let r = Response::read_from(&mut BufReader::new(s), "POST").unwrap_or_else(|e| panic!("网关的回复读不了：{e}"));
        Gateway::reply(&r)
    }

    /// Team 的服务器交来的那样的一条（cli/src/server/push.rs 的 run）
    pub fn body(token: &str, env: &str, id: &str, note: &Value) -> Vec<u8> {
        let doc = crate::obj! { "v" => 1, "token" => token, "env" => env, "id" => id, "note" => note.clone() };
        json::dumps(&doc, &Style::compact()).into_bytes()
    }

    /// 一个设备令牌：64 个十六进制字符，最后两个是 tail（定假推送服务回什么：见 Shared::by_tail）
    pub fn token(tail: &str) -> String {
        format!("{}{tail}", crypto::token_hex(31))
    }

    /// 它在 stderr 上说的话
    pub fn said(&self) -> String {
        self.proc.said()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        crypto::unhex(s).unwrap_or_else(|e| panic!("{e}"))
    }

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect()
    }

    /// RFC 7541 C.3：同一个解码器接连解三个请求（不压缩的那一组），第二、三个用到前面进了动态表的
    #[test]
    fn rfc7541_requests() {
        let mut d = Decoder::new();
        let first = d.decode(&unhex("828684410f7777772e6578616d706c652e636f6d")).unwrap();
        assert_eq!(first, pairs(&[(":method", "GET"), (":scheme", "http"), (":path", "/"), (":authority", "www.example.com")]));
        let second = d.decode(&unhex("828684be58086e6f2d6361636865")).unwrap();
        assert_eq!(
            second,
            pairs(&[
                (":method", "GET"),
                (":scheme", "http"),
                (":path", "/"),
                (":authority", "www.example.com"),
                ("cache-control", "no-cache")
            ])
        );
        let third = d.decode(&unhex("828785bf400a637573746f6d2d6b65790c637573746f6d2d76616c7565")).unwrap();
        assert_eq!(
            third,
            pairs(&[
                (":method", "GET"),
                (":scheme", "https"),
                (":path", "/index.html"),
                (":authority", "www.example.com"),
                ("custom-key", "custom-value")
            ])
        );
        assert_eq!(d.size, 164, "RFC 7541 C.3.3 里表的大小");
        // 大小的更新只能在开头；压缩过的不认
        assert!(d.decode(&[0x82, 0x20]).is_err());
        assert!(Decoder::new().decode(&unhex("418cf1e3c2e5f23a6ba0ab90f4ff")).is_err());
    }

    /// 压缩过的数字照 RFC 7541 C.6.1、C.6.2 的例子（302、307）；回的头这边自己也解得回来（名字是原文的那种除外）
    #[test]
    fn huffman_digits_and_reply_blocks() {
        let mut out = Vec::new();
        put_huffman_digits(&mut out, "302");
        assert_eq!(out, [0x82, 0x64, 0x02]);
        out.clear();
        put_huffman_digits(&mut out, "307");
        assert_eq!(out, [0x83, 0x64, 0x0e, 0xff]);
        for status in [200, 400, 403, 404, 429, 500] {
            let block = reply_block(status, true);
            let got = Decoder::new().decode(&block).unwrap();
            assert_eq!(
                (got[0].0.as_str(), got[0].1.as_str(), got[1].0.as_str()),
                (":status", status.to_string().as_str(), "apns-id")
            );
        }
    }

    /// 造出来的 .p8 是苹果那个样子，ring 读得回来、拿它签的名用给出的公钥验得过
    #[test]
    fn auth_keys_look_like_apples() {
        let d = crate::fsx::TempDir::new("cca-apnsfake-");
        let path = d.path().join("AuthKey_TEST.p8");
        let public = auth_key(&path);
        let pem = fsx::read_text(&path);
        assert!(pem.starts_with("-----BEGIN PRIVATE KEY-----\nMIGTAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBHkwdwIBAQQg"), "{pem}");
        assert_eq!(fsx::mode(&path), 0o600);
        let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
        let der = crypto::b64_decode(&body).unwrap();
        let rng = SystemRandom::new();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &rng).unwrap();
        assert_eq!(pair.public_key().as_ref(), public.as_slice());
        let sig = pair.sign(&rng, b"x.y").unwrap();
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &public).verify(b"x.y", sig.as_ref()).unwrap();
    }
}
