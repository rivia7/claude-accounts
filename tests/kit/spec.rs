//! 照 server/README.md 独立实现的那一头（原来的 tests/spec_wire.py）：密钥派生、封装的密文、配对码和配对的消息、会话通道、
//! Team 的邀请链接和配置、带签名的请求。测试拿它扮演别的设备和管理员；不看、不用辅助程序里对应的代码，两边对得上才算对。
//!
//! 函数名、参数的先后都照原来的 Python（Python 里有默认值的参数这里要写出来：标签写 [`COST`] 这样的常量）。几处写法不一样：
//! - 随机的那几样（nonce、握手的随机数、cn）另有一个 `_with` 的写法把它交进来：对照数据（tests/vectors/spec_wire.json）靠它逐字节比
//! - Python 里抛异常的地方：文档写明“不对是 ValueError / None”的返回 `Result` / `Option`；别的（十六进制、base64 不成样子，
//!   密文少了键，明文不是 JSON）在这里 panic——测试里那就是这个用例失败
//! - base64 解得比 Python 严（不跳过字母表以外的字符，= 要带齐）：被测的程序写得不规矩应该看得出来
//! - 折合金额的压缩用 miniz_oxide：压出来的字节和 Python 的 zlib 不一样（解出来一样），所以比的都是明文

use crate::crypto::{
    b64, b64_decode, b64_nopad, b64url_nopad, b64url_nopad_decode, ct_eq, hex, hmac_sha256, pbkdf2_hmac_sha256, random, sha256,
    sha256_hex, token_hex, unhex,
};
use crate::fsx::TempDir;
use crate::json::{self, Style, Value};
use crate::proc::{Cmd, Stdio};
use crate::{http, obj, py, tls};
use std::io;
use std::path::Path;
use std::time::Duration;

/// 记下的 API key 的标签
pub const KEYSHARE: &str = "cca-keyshare-v1";
/// 折合金额的标签（3.6.6 起：密钥整个 Team 共用）
pub const COST: &str = "cca-cost-v2";
/// 3.6.5 及以前的折合金额的标签（密钥照 [`derive`]）
pub const COST_V1: &str = "cca-cost-v1";
/// 折合金额里认不出是哪个账号的那一份：账号摘要里“工具”的位置写它
pub const ORPHAN: &str = "cca-cost-orphan";
/// Crockford base32 的字母表（配对码）
pub const B32: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// 邀请链接的开头
pub const TEAM_LINK: &str = "cca-team1.";
/// 设备的配置的标签
pub const TEAM_DOC: &str = "cca-team-doc-v1";
/// 管理员名单签名时前面的标签（3.6.15 起）
pub const TEAM_ADMINS: &str = "cca-team-admins-v1";
/// 几位管理员共用的状态的标签（3.6.15 起）
pub const TEAM_ASTATE: &str = "cca-team-astate-v1";
/// 配对时放到服务器上的那一份的标签
pub const PAIR: &str = "cca-remote-pair";
/// 会话的说明的标签
pub const DIR: &str = "cca-remote-dir";
/// 推送的通知的标签
pub const NOTE: &str = "cca-remote-note";
/// 账号的状态的标签（手机的「账号」页，3.7.5 起）
pub const STATUS: &str = "cca-remote-status";
/// 启动器的说明的标签（手机上用模板新建会话，3.7.6 起）
pub const LAUNCH: &str = "cca-remote-launch";

/// 两把密钥：（加密, 认证）
pub type Keys = ([u8; 32], [u8; 32]);

/// `bytes.fromhex`：不是十六进制就 panic
fn fromhex(text: &str) -> Vec<u8> {
    unhex(text).unwrap_or_else(|e| panic!("{text:?} 不是十六进制：{e}"))
}

/// `base64.b64decode`：不是规矩的 base64 就 panic
fn unb64(text: &str) -> Vec<u8> {
    b64_decode(text).unwrap_or_else(|e| panic!("{text:?} 不是 base64：{e}"))
}

/// 补齐到 block 的整数倍
fn pad(mut data: Vec<u8>, block: usize, with: u8) -> Vec<u8> {
    let short = (block - data.len() % block) % block;
    data.resize(data.len() + short, with);
    data
}

/// 两段字节逐个异或（长的那段多出来的不要）
fn xor(a: &[u8], b: &[u8]) -> Vec<u8> {
    a.iter().zip(b).map(|(x, y)| x ^ y).collect()
}

/// 明文读成 JSON（后面补的空格不碍事）；读不出来就 panic
fn loads(plain: &[u8]) -> Value {
    json::parse_bytes(plain).unwrap_or_else(|e| panic!("认证过的明文不是 JSON：{e}"))
}

/// 公钥那一行（`ssh-ed25519 AAAA… 备注`）里 base64 的那一段解出来的字节
fn pub_blob(line: &str) -> Vec<u8> {
    let parts: Vec<&str> = py::split(line).collect();
    let Some(body) = parts.get(1) else { panic!("不是公钥的那一行（要“类型 base64 [备注]”）：{line:?}") };
    unb64(body)
}

/// 公钥那一行（`ssh-ed25519 AAAA… 备注`）的前两段，中间一个空格：加入 Team 时交的、allowed_signers 里写的都是这个样子
pub fn pub_line(line: &str) -> String {
    py::split(line).take(2).collect::<Vec<_>>().join(" ")
}

/// HMAC-SHA256
pub fn h(key: &[u8], msg: &[u8]) -> [u8; 32] {
    hmac_sha256(key, msg)
}

/// 两把 HMAC 密钥（加密、认证）：root = HMAC(盐（十六进制解出来的字节）, info)，再各 HMAC 一次 enc、mac
pub fn keys_of(salt: &str, info: &[u8]) -> Keys {
    let root = h(&fromhex(salt), info);
    (h(&root, b"enc"), h(&root, b"mac"))
}

/// 由 Team 的配置里的盐、工具、邮箱（小写）和登录身份派生（记下的 API key；3.6.5 及以前的折合金额，标签是 [`COST_V1`]）
pub fn derive(label: &str, salt: &str, tool: &str, email: &str, ident: &str) -> Keys {
    keys_of(salt, format!("{label}\0{tool}\0{}\0{ident}", email.to_lowercase()).as_bytes())
}

/// 整个 Team 共用的两把密钥：只由盐派生（3.6.6 起的折合金额，标签是 [`COST`]）
pub fn team_keys(salt: &str, label: &str) -> Keys {
    keys_of(salt, label.as_bytes())
}

/// 密钥流：HMAC(enc, nonce + 8 字节大端的块号)，每块 32 字节（给的是整块的，可能比 size 长）
pub fn stream(enc: &[u8], n: &[u8], size: usize) -> Vec<u8> {
    let blocks = size.div_ceil(32);
    let mut out = Vec::with_capacity(blocks * 32);
    for i in 0..blocks as u64 {
        out.extend_from_slice(&h(enc, &[n, &i.to_be_bytes()].concat()));
    }
    out
}

/// 认证码：标签和 ad（账号摘要；折合金额还有设备指纹）各跟一个 \0，接着 nonce 和密文
pub fn tag(label: &str, mac: &[u8], ad: &[&str], n: &[u8], c: &[u8]) -> [u8; 32] {
    let mut msg = Vec::new();
    for part in std::iter::once(&label).chain(ad) {
        msg.extend_from_slice(part.as_bytes());
        msg.push(0);
    }
    msg.extend_from_slice(n);
    msg.extend_from_slice(c);
    h(mac, &msg)
}

/// 封一份：`{"n": nonce, "c": 密文, "t": 认证码}`（base64），nonce 是 16 个随机字节
pub fn seal(label: &str, keys: &Keys, ad: &[&str], pt: &[u8]) -> Value {
    seal_with(label, keys, ad, pt, &random(16))
}

/// 同 [`seal`]，nonce 是给的（对照数据用）
pub fn seal_with(label: &str, keys: &Keys, ad: &[&str], pt: &[u8], n: &[u8]) -> Value {
    let c = xor(pt, &stream(&keys.0, n, pt.len()));
    obj! { "n" => b64(n), "c" => b64(&c), "t" => b64(&tag(label, &keys.1, ad, n, &c)) }
}

/// 认证过的明文；认证不过是 None
pub fn unseal(label: &str, keys: &Keys, ad: &[&str], blob: &Value) -> Option<Vec<u8>> {
    let [n, c, t] = ["n", "c", "t"].map(|k| unb64(blob[k].str()));
    if !ct_eq(&t, &tag(label, &keys.1, ad, &n, &c)) {
        return None;
    }
    Some(xor(&c, &stream(&keys.0, &n, c.len())))
}

/// 记下的 API key：明文是 JSON，空格补齐到 256 字节的整数倍；认证带着账号摘要
pub fn keyshare_seal(keys: &Keys, aid: &str, doc: &Value) -> Value {
    seal(KEYSHARE, keys, &[aid], &keyshare_plain(doc))
}

/// 记下的 API key 的明文（`json.dumps` 默认的写法，补齐）
pub fn keyshare_plain(doc: &Value) -> Vec<u8> {
    pad(json::dumps(doc, &Style::default()).into_bytes(), 256, b' ')
}

/// 解开记下的 API key；认证不过是 None
pub fn keyshare_open(keys: &Keys, aid: &str, blob: &Value) -> Option<Value> {
    unseal(KEYSHARE, keys, &[aid], blob).map(|pt| loads(&pt))
}

/// 折合金额的明文：4 字节大端的长度 + zlib 压缩的 JSON，零补齐到 1024 字节的整数倍
pub fn cost_plain(z: &[u8]) -> Vec<u8> {
    let len = u32::try_from(z.len()).unwrap_or_else(|_| panic!("压缩后的内容太长（{} 字节）", z.len()));
    pad([&len.to_be_bytes()[..], z].concat(), 1024, 0)
}

/// 封一份折合金额：认证带着标签、账号摘要和设备指纹（3.6.5 及以前的标签是 [`COST_V1`]，密钥照 [`derive`]）
pub fn cost_seal(keys: &Keys, aid: &str, fp: &str, doc: &Value, label: &str) -> Value {
    let text = json::dumps(doc, &Style::default());
    cost_seal_raw(keys, aid, fp, &miniz_oxide::deflate::compress_to_vec_zlib(text.as_bytes(), 6), label)
}

/// 同 [`cost_seal`]，直接给压缩好的（测解压炸弹之类；原来的 `raw=`）
pub fn cost_seal_raw(keys: &Keys, aid: &str, fp: &str, raw: &[u8], label: &str) -> Value {
    seal(label, keys, &[aid, fp], &cost_plain(raw))
}

/// 折合金额的明文读成文档：没补齐到 1024 字节的整数倍、补的不是零、解压不了、不是 JSON 都说出来
pub fn cost_unpack(pt: &[u8]) -> Result<Value, String> {
    if pt.len() < 4 {
        return Err(format!("明文只有 {} 个字节：连长度都放不下", pt.len()));
    }
    let size = u32::from_be_bytes([pt[0], pt[1], pt[2], pt[3]]) as usize;
    let end = pt.len().min(4usize.saturating_add(size));
    if !pt.len().is_multiple_of(1024) || pt[end..].iter().any(|b| *b != 0) {
        return Err("补齐到 1024 字节的整数倍，补的是零".to_string());
    }
    let text = miniz_oxide::inflate::decompress_to_vec_zlib(&pt[4..end]).map_err(|e| format!("解压不了：{e}"))?;
    json::parse_bytes(&text).map_err(|e| format!("解压出来的不是 JSON：{e}"))
}

/// 解开一份折合金额；认证不过是 None。明文不成样子（见 [`cost_unpack`]）就 panic
pub fn cost_open(keys: &Keys, aid: &str, fp: &str, blob: &Value, label: &str) -> Option<Value> {
    unseal(label, keys, &[aid, fp], blob).map(|pt| cost_unpack(&pt).unwrap_or_else(|e| panic!("{e}")))
}

/// 上报用的账号摘要：HMAC(盐, 工具 \0 邮箱) 的前 32 个十六进制字符
pub fn aid_of(salt: &str, tool: &str, email: &str) -> String {
    hex(&h(&fromhex(salt), format!("{tool}\0{email}").as_bytes()))[..32].to_string()
}

/// 折合金额里认不出是哪个账号的那一份的账号摘要：HMAC(盐, "cca-cost-orphan" \0) 的前 32 个十六进制字符
/// （和 [`aid_of`] 的工具写它、邮箱是空的一样）
pub fn orphan_aid(salt: &str) -> String {
    aid_of(salt, ORPHAN, "")
}

/// 公钥那一行的指纹 `SHA256:…`（base64 那一段解出来的字节的 SHA-256，base64 不带 =）
pub fn fp_of_line(line: &str) -> String {
    format!("SHA256:{}", b64_nopad(&sha256(&pub_blob(line))))
}

/// 设备在 Team 里的那把密钥（公钥文件）的指纹 `SHA256:…`
pub fn fp_of(pubfile: impl AsRef<Path>) -> String {
    fp_of_line(&crate::fsx::read_text(pubfile))
}

// ---------------------------------------------------------------- 远程会话（server/README.md「远程会话的中转」）

/// 配对码：10 个字节写成 16 个字符，四个一组
pub fn code_text(c: &[u8]) -> String {
    // 当成一个大端的整数，取低 80 位，5 位一个字符
    let mut n = [0u8; 10];
    let take = c.len().min(10);
    n[10 - take..].copy_from_slice(&c[c.len() - take..]);
    let n = n.iter().fold(0u128, |acc, b| acc << 8 | *b as u128);
    let chars: Vec<char> = (0..16).rev().map(|i| B32.as_bytes()[(n >> (5 * i) & 31) as usize] as char).collect();
    chars.chunks(4).map(|group| group.iter().collect::<String>()).collect::<Vec<_>>().join("-")
}

/// 输入的配对码（大小写、横线、空格随意；I L 当 1、O 当 0）；不对是 None
pub fn code_bytes(text: &str) -> Option<Vec<u8>> {
    let s: Vec<char> = text
        .to_uppercase()
        .chars()
        .filter(|c| *c != '-' && *c != ' ')
        .map(|c| match c {
            'I' | 'L' => '1',
            'O' => '0',
            c => c,
        })
        .collect();
    if s.len() != 16 {
        return None;
    }
    let mut n = 0u128;
    for ch in s {
        n = n * 32 + B32.chars().position(|b| b == ch)? as u128;
    }
    Some(n.to_be_bytes()[6..].to_vec())
}

/// 配对码 → (Kc, 配对消息的两把密钥, 服务器上认这次配对的编号)。配对码只有 80 位，所以这一步故意算得慢
pub fn pair_keys(c: &[u8]) -> ([u8; 32], Keys, String) {
    let mut kc = [0u8; 32];
    kc.copy_from_slice(&pbkdf2_hmac_sha256(c, b"cca-remote/v1/pair", 65536, 32));
    let rid = hex(&h(&kc, b"rid"))[..32].to_string();
    (kc, (h(&kc, b"enc"), h(&kc, b"mac")), rid)
}

/// 配对时放到服务器上的那一份：role 是 "a"（显示配对码的）/ "b"（输入的）
pub fn pair_msg(keys: &Keys, rid: &str, role: &str, fp: &str, nonce: &[u8], name: &str) -> Value {
    seal(PAIR, keys, &[rid, role], &pair_plain(fp, nonce, name))
}

/// 配对的那一份的明文：紧凑的 JSON（非 ASCII 原样写），空格补齐到 256 字节的整数倍
pub fn pair_plain(fp: &str, nonce: &[u8], name: &str) -> Vec<u8> {
    pad(obj! { "v" => 1, "fp" => fp, "n" => hex(nonce), "name" => name }.compact().into_bytes(), 256, b' ')
}

/// 解开对方放的那一份；认证不过（不知道配对码的人放的）是 None
pub fn pair_open(keys: &Keys, rid: &str, role: &str, blob: &Value) -> Option<Value> {
    unseal(PAIR, keys, &[rid, role], blob).map(|pt| loads(&pt))
}

/// 两台机器之间的密钥 P，和它的编号（服务器只知道编号）
pub fn pair_secret(kc: &[u8], na: &[u8], nb: &[u8], fpa: &str, fpb: &str) -> ([u8; 32], String) {
    let p = h(kc, &[b"secret\0", na, nb, fpa.as_bytes(), b"\0", fpb.as_bytes(), b"\0"].concat());
    let pid = hex(&h(&p, b"cca-remote/v1/pid"))[..32].to_string();
    (p, pid)
}

/// 封会话的说明用的两把密钥
pub fn dir_keys(p: &[u8]) -> Keys {
    let k = h(p, b"cca-remote/v1/dir");
    (h(&k, b"enc"), h(&k, b"mac"))
}

/// 会话的说明的明文：紧凑的 JSON（非 ASCII 原样写），空格补齐到 256 字节的整数倍
pub fn dir_plain(doc: &Value) -> Vec<u8> {
    pad(doc.compact().into_bytes(), 256, b' ')
}

/// 封一份会话的说明：认证带着配对的编号和会话的编号
pub fn dir_seal(p: &[u8], pid: &str, sid: &str, doc: &Value) -> Value {
    seal(DIR, &dir_keys(p), &[pid, sid], &dir_plain(doc))
}

/// 解开会话的说明；认证不过是 None
pub fn dir_open(p: &[u8], pid: &str, sid: &str, blob: &Value) -> Option<Value> {
    unseal(DIR, &dir_keys(p), &[pid, sid], blob).map(|pt| loads(&pt))
}

/// 封启动器的说明用的两把密钥（启动器：设备上能远程起会话的那个常驻进程，在服务器上登记成一个会话）
pub fn launch_keys(p: &[u8]) -> Keys {
    let k = h(p, b"cca-remote/v1/launch");
    (h(&k, b"enc"), h(&k, b"mac"))
}

/// 启动器的说明的明文：紧凑的 JSON（非 ASCII 原样写），空格补齐到 256 字节的整数倍
pub fn launch_plain(doc: &Value) -> Vec<u8> {
    pad(doc.compact().into_bytes(), 256, b' ')
}

/// 封一份启动器的说明：认证带着配对的编号和启动器的编号（它登记成的那个会话的）
pub fn launch_seal(p: &[u8], pid: &str, sid: &str, doc: &Value) -> Value {
    seal(LAUNCH, &launch_keys(p), &[pid, sid], &launch_plain(doc))
}

/// 解开启动器的说明；认证不过（包括拿会话的说明来解的）是 None
pub fn launch_open(p: &[u8], pid: &str, sid: &str, blob: &Value) -> Option<Value> {
    unseal(LAUNCH, &launch_keys(p), &[pid, sid], blob).map(|pt| loads(&pt))
}

/// 封推送的通知用的两把密钥（手机上解它的通知扩展只拿得到这两把，拿不到 P）
pub fn note_keys(p: &[u8]) -> Keys {
    let k = h(p, b"cca-remote/v1/note");
    (h(&k, b"enc"), h(&k, b"mac"))
}

/// 推送的通知的明文：紧凑的 JSON（非 ASCII 原样写），空格补齐到 256 字节的整数倍
pub fn note_plain(doc: &Value) -> Vec<u8> {
    pad(doc.compact().into_bytes(), 256, b' ')
}

/// 封一条推送的通知：认证带着配对的编号
pub fn note_seal(p: &[u8], pid: &str, doc: &Value) -> Value {
    seal(NOTE, &note_keys(p), &[pid], &note_plain(doc))
}

/// 解开推送的通知；认证不过是 None
pub fn note_open(p: &[u8], pid: &str, blob: &Value) -> Option<Value> {
    unseal(NOTE, &note_keys(p), &[pid], blob).map(|pt| loads(&pt))
}

/// 封账号的状态用的两把密钥（宿主放到服务器上、手机查会话列表时取走的那一份）
pub fn status_keys(p: &[u8]) -> Keys {
    let k = h(p, b"cca-remote/v1/status");
    (h(&k, b"enc"), h(&k, b"mac"))
}

/// 账号的状态的明文：紧凑的 JSON（非 ASCII 原样写），空格补齐到 256 字节的整数倍
pub fn status_plain(doc: &Value) -> Vec<u8> {
    pad(doc.compact().into_bytes(), 256, b' ')
}

/// 封一份账号的状态：认证带着配对的编号
pub fn status_seal(p: &[u8], pid: &str, doc: &Value) -> Value {
    seal(STATUS, &status_keys(p), &[pid], &status_plain(doc))
}

/// 解开账号的状态；认证不过是 None
pub fn status_open(p: &[u8], pid: &str, blob: &Value) -> Option<Value> {
    unseal(STATUS, &status_keys(p), &[pid], blob).map(|pt| loads(&pt))
}

/// 两头之间的加密通道。握手：接入方发 hello()，宿主回 reply()，接入方再发 confirm()，宿主 accept()；之后两个方向各用各的密钥、
/// 各从 0 数序号。role 是 "host" / "viewer"；fph、fpv 是各自记着的宿主、接入方的指纹
pub struct Channel {
    role: String,
    ka: [u8; 32],
    pid: String,
    sid: String,
    fph: String,
    fpv: String,
    send_seq: u64,
    recv_seq: u64,
    nv: Option<Vec<u8>>,
    nh: Option<Vec<u8>>,
    tagh: Option<Vec<u8>>,
    tagv: Option<Vec<u8>>,
    /// 自己发的那个方向的（加密, 认证）
    out: Option<Keys>,
    /// 对方发的那个方向的
    inn: Option<Keys>,
}

impl Channel {
    /// 握手的版本
    pub const VER: u8 = 1;

    pub fn new(role: &str, p: &[u8], pid: &str, sid: &str, fph: &str, fpv: &str) -> Channel {
        Channel {
            role: role.to_string(),
            ka: h(p, b"cca-remote/v1/auth"),
            pid: pid.to_string(),
            sid: sid.to_string(),
            fph: fph.to_string(),
            fpv: fpv.to_string(),
            send_seq: 0,
            recv_seq: 0,
            nv: None,
            nh: None,
            tagh: None,
            tagv: None,
            out: None,
            inn: None,
        }
    }

    /// 握手里那几样还没有（步骤的先后不对）就 panic
    fn part<'a>(slot: &'a Option<Vec<u8>>, what: &str) -> &'a [u8] {
        slot.as_deref().unwrap_or_else(|| panic!("握手的先后不对：还没有 {what}"))
    }

    fn ctx(&self) -> Vec<u8> {
        [
            &[Self::VER][..],
            &fromhex(&self.pid),
            &fromhex(&self.sid),
            Self::part(&self.nv, "nV"),
            Self::part(&self.nh, "nH"),
            self.fph.as_bytes(),
            b"\0",
            self.fpv.as_bytes(),
            b"\0",
        ]
        .concat()
    }

    fn keys(&mut self) {
        let ks =
            h(&self.ka, &[b"session\0", &self.ctx()[..], Self::part(&self.tagh, "tagH"), Self::part(&self.tagv, "tagV")].concat());
        let pair = |dir: &str| (h(&ks, format!("{dir}-enc").as_bytes()), h(&ks, format!("{dir}-mac").as_bytes()));
        let (mine, theirs) = if self.role == "host" { ("h2v", "v2h") } else { ("v2h", "h2v") };
        self.out = Some(pair(mine));
        self.inn = Some(pair(theirs));
    }

    fn tag_host(&self) -> [u8; 32] {
        h(&self.ka, &[b"host\0", &self.ctx()[..]].concat())
    }

    fn tag_viewer(&self) -> [u8; 32] {
        h(&self.ka, &[b"viewer\0", &self.ctx()[..], Self::part(&self.tagh, "tagH")].concat())
    }

    /// 接入方 → 宿主（52 字节）
    pub fn hello(&mut self) -> Vec<u8> {
        self.hello_with(&random(32))
    }

    /// 同 [`Channel::hello`]，随机数是给的
    pub fn hello_with(&mut self, nv: &[u8]) -> Vec<u8> {
        self.nv = Some(nv.to_vec());
        [b"CR\x01", &[Self::VER][..], nv, &fromhex(&self.pid)].concat()
    }

    /// 宿主 → 接入方（68 字节）；hello 不对是 None
    pub fn reply(&mut self, hello: &[u8]) -> Option<Vec<u8>> {
        self.reply_with(hello, &random(32))
    }

    /// 同 [`Channel::reply`]，随机数是给的
    pub fn reply_with(&mut self, hello: &[u8], nh: &[u8]) -> Option<Vec<u8>> {
        if hello.len() != 52 || hello[..4] != [b'C', b'R', 1, Self::VER] || hello[36..] != fromhex(&self.pid)[..] {
            return None;
        }
        self.nv = Some(hello[4..36].to_vec());
        self.nh = Some(nh.to_vec());
        let tagh = self.tag_host();
        self.tagh = Some(tagh.to_vec());
        Some([b"CR\x02", &[Self::VER][..], nh, &tagh].concat())
    }

    /// 接入方 → 宿主（35 字节）；宿主的证明不对是 None
    pub fn confirm(&mut self, reply: &[u8]) -> Option<Vec<u8>> {
        if reply.len() != 68 || reply[..4] != [b'C', b'R', 2, Self::VER] {
            return None;
        }
        self.nh = Some(reply[4..36].to_vec());
        self.tagh = Some(reply[36..].to_vec());
        if !ct_eq(&reply[36..], &self.tag_host()) {
            return None;
        }
        let tagv = self.tag_viewer();
        self.tagv = Some(tagv.to_vec());
        self.keys();
        Some([b"CR\x03", &tagv[..]].concat())
    }

    /// 宿主收到接入方的证明：对不对
    pub fn accept(&mut self, confirm: &[u8]) -> bool {
        if confirm.len() != 35 || confirm[..3] != *b"CR\x03" {
            return false;
        }
        self.tagv = Some(confirm[3..].to_vec());
        if !ct_eq(&confirm[3..], &self.tag_viewer()) {
            return false;
        }
        self.keys();
        true
    }

    /// 一个方向的两把密钥；握手还没做完就 panic
    fn side(slot: &Option<Keys>) -> &Keys {
        slot.as_ref().unwrap_or_else(|| panic!("握手还没做完：这个方向的密钥还没有"))
    }

    /// 一条记录：序号(8) 密文长度(2) 密文 认证(16)。明文是 长度(2) 内容 零补齐到 32 的整数倍；内容超过 16384 字节就 panic
    pub fn seal(&mut self, data: &[u8]) -> Vec<u8> {
        assert!(data.len() <= 16384, "一条记录的内容最长 16384 字节（给的是 {}）", data.len());
        let (enc, mac) = Self::side(&self.out);
        let pt = pad([&(data.len() as u16).to_be_bytes()[..], data].concat(), 32, 0);
        let seq = self.send_seq.to_be_bytes();
        let ct = xor(&pt, &stream(enc, &seq, pt.len()));
        let record = [&seq[..], &(ct.len() as u16).to_be_bytes(), &ct].concat();
        let tag = h(mac, &record);
        self.send_seq += 1;
        [&record[..], &tag[..16]].concat()
    }

    /// buf 开头的一条记录：内容（从 buf 里拿走这一条）；还不够一条是 Ok(None)；长度、序号和认证、补齐不对是
    /// Err("len" / "auth" / "pad")（原来抛 ValueError；"pad" 时这一条已经拿走、序号已经往前数了）
    pub fn open(&mut self, buf: &mut Vec<u8>) -> Result<Option<Vec<u8>>, &'static str> {
        if buf.len() < 10 {
            return Ok(None);
        }
        let n = u16::from_be_bytes([buf[8], buf[9]]) as usize;
        if n > 16384 + 2 + 31 || !n.is_multiple_of(32) || n == 0 {
            return Err("len");
        }
        if buf.len() < 10 + n + 16 {
            return Ok(None);
        }
        let (enc, mac) = Self::side(&self.inn);
        let (head, ct, tag) = (&buf[..10], &buf[10..10 + n], &buf[10 + n..26 + n]);
        if head[..8] != self.recv_seq.to_be_bytes() || !ct_eq(tag, &h(mac, &buf[..10 + n])[..16]) {
            return Err("auth");
        }
        let pt = xor(ct, &stream(enc, &head[..8], n));
        self.recv_seq += 1;
        buf.drain(..26 + n);
        let size = u16::from_be_bytes([pt[0], pt[1]]) as usize;
        if size > n - 2 || pt[2 + size..].iter().any(|b| *b != 0) {
            return Err("pad");
        }
        Ok(Some(pt[2..2 + size].to_vec()))
    }
}

/// 连数据端口时说的那一行（role：host / view 用票据的编号和密钥，join 用令牌和控制连接那张票据的密钥），和该回的那一行
pub fn relay_line(role: &str, ident: &str, key: &[u8]) -> (Vec<u8>, Vec<u8>) {
    relay_line_with(role, ident, key, &token_hex(8))
}

/// 同 [`relay_line`]，cn（客户端出的 16 个十六进制字符）是给的
pub fn relay_line_with(role: &str, ident: &str, key: &[u8], cn: &str) -> (Vec<u8>, Vec<u8>) {
    let lead = if role == "join" { "j".to_string() } else { format!("c{role}") };
    let mac = b64url_nopad(&h(key, format!("{lead}{ident}{cn}").as_bytes()));
    let back = b64url_nopad(&h(key, format!("s{ident}{cn}").as_bytes()));
    (format!("CCA-RELAY/1 {role} {ident} {cn} {mac}\n").into_bytes(), format!("CCA-RELAY/1 OK {back}\n").into_bytes())
}

// ---------------------------------------------------------------- Team（server/README.md「Team 的同步服务器」）

/// Team 的编号：根公钥（'ssh-ed25519 AAAA…' 里 base64 那一段解出来的字节）的 SHA-256 的前 32 个十六进制字符
pub fn team_id(root_pub: &str) -> String {
    sha256_hex(&pub_blob(root_pub))[..32].to_string()
}

/// 邀请里的那串随机数（32 个小写十六进制字符，解出来的 16 个字节当 HMAC 的密钥）派生出来的三样：加入时交给服务器的令牌、
/// 这台设备在 Team 里的编号、它那份配置的两把密钥（加密、认证）。不是那个样子是 Err（原来的 ValueError）
pub fn invite_parts(secret: &str) -> Result<(String, String, Keys), String> {
    if secret.chars().count() != 32 || !secret.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
        return Err("那串随机数要是 32 个小写十六进制字符".to_string());
    }
    let k = fromhex(secret);
    let root = h(&k, b"cca-team-key-v1");
    let token = hex(&h(&k, b"cca-team-token-v1")[..16]);
    let dev = hex(&h(&k, b"cca-team-dev-v1")[..8]);
    Ok((token, dev, (h(&root, b"enc"), h(&root, b"mac"))))
}

/// 设备加入时给自己的公钥（'ssh-ed25519 AAAA…' 那一行的前两段）算的认证码：HMAC(HMAC(k, "cca-team-pub-v1"), 那一行)，base64
pub fn team_pub_proof(secret: &str, public: &str) -> String {
    let key = h(&fromhex(secret), b"cca-team-pub-v1");
    b64(&h(&key, pub_line(public).as_bytes()))
}

/// 邀请链接：`cca-team1.<base64url(紧凑的 JSON)，不带 =>.<前面这一段的 SHA-256 的前 4 个十六进制字符>`。exp 是什么时候过期
/// （JSON 的数，可以没有），device 是管理员给这台设备起的名字（空的和没有一样：不写 d）
pub fn invite_encode(
    url: &str,
    pins: &[&str],
    root_pub: &str,
    secret: &str,
    name: &str,
    exp: Option<&Value>,
    device: Option<&str>,
) -> String {
    let parts: Vec<&str> = py::split(root_pub).collect();
    let Some(root) = parts.get(1) else { panic!("不是公钥的那一行（要“类型 base64 [备注]”）：{root_pub:?}") };
    let mut doc = obj! { "u" => url, "p" => pins.to_vec(), "r" => *root, "k" => secret, "n" => name };
    let fields = doc.as_obj_mut().expect("obj! 拼的是对象");
    if let Some(exp) = exp {
        fields.insert("e", exp);
    }
    if let Some(device) = device.filter(|d| !d.is_empty()) {
        fields.insert("d", device);
    }
    let head = format!("{TEAM_LINK}{}", b64url_nopad(doc.compact().as_bytes()));
    format!("{head}.{}", &sha256_hex(head.as_bytes())[..4])
}

/// 读邀请链接（中间的空白不算）：`{"u", "p", "r", "k", "n"[, "e"][, "d"]}`；不对是 Err（原来的 ValueError）
pub fn invite_decode(link: &str) -> Result<Value, String> {
    let s: String = link.chars().filter(|c| !py::isspace(*c)).collect();
    let (head, check) = s.rsplit_once('.').unwrap_or(("", &s));
    if !head.starts_with(TEAM_LINK) || &sha256_hex(head.as_bytes())[..4] != check {
        return Err("不是邀请链接，或者抄错了".to_string());
    }
    let raw = b64url_nopad_decode(&head[TEAM_LINK.len()..]).map_err(|e| format!("邀请链接里的 base64 解不开：{e}"))?;
    json::parse_bytes(&raw).map_err(|e| format!("邀请链接里的不是 JSON：{e}"))
}

/// 一台设备的配置的明文：紧凑的 JSON（UTF-8，非 ASCII 原样写），空格补齐到 1024 字节的整数倍
pub fn team_doc_plain(doc: &Value) -> Vec<u8> {
    pad(doc.compact().into_bytes(), 1024, b' ')
}

/// 封一台设备的配置：认证带着 Team 的编号、设备的编号、序号（十进制的数字）
pub fn team_doc_seal(keys: &Keys, tid: &str, dev: &str, seq: i64, doc: &Value) -> Value {
    seal(TEAM_DOC, keys, &[tid, dev, &seq.to_string()], &team_doc_plain(doc))
}

/// 认证过的配置；认证不过（别的设备的、别的序号的、改过的）是 None
pub fn team_doc_open(keys: &Keys, tid: &str, dev: &str, seq: i64, ct: &Value) -> Option<Value> {
    unseal(TEAM_DOC, keys, &[tid, dev, &seq.to_string()], ct).map(|pt| loads(&pt))
}

/// 根密钥签的那一段字节：标签、Team 的编号、设备的编号、序号各跟一个 \0，接着 nonce、密文、认证码（解出来的字节）
pub fn team_doc_signed(tid: &str, dev: &str, seq: i64, ct: &Value) -> Vec<u8> {
    let mut out = format!("{TEAM_DOC}\0{tid}\0{dev}\0{seq}\0").into_bytes();
    for k in ["n", "c", "t"] {
        out.extend_from_slice(&unb64(ct[k].str()));
    }
    out
}

/// `ssh-keygen -Y sign`：拿这把私钥（文件）给 data 签名，命名空间 ns；给的是 armored 的签名。ssh-keygen 只带着 PATH：
/// 不碰 ssh-agent，也不读真的 HOME。quiet：它在 stderr 上说的不要（不然原样说出来）
pub fn ssh_sign(keyfile: &Path, ns: &str, data: &[u8], quiet: bool) -> io::Result<Vec<u8>> {
    let mut cmd = Cmd::new("ssh-keygen");
    cmd.args(["-Y", "sign", "-q", "-n", ns, "-f"]).arg(keyfile).pass_env(&["PATH"]).input(data);
    cmd.stderr(if quiet { Stdio::Null } else { Stdio::Inherit });
    let out = cmd.output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!("ssh-keygen -Y sign（{}）没成：{}", keyfile.display(), out.status)));
    }
    Ok(out.stdout)
}

/// `ssh-keygen -Y verify`：sig（armored 的签名）是不是这把公钥（'ssh-ed25519 AAAA…'）在命名空间 ns 里给 data 签的
/// （allowed_signers 里只有它）
pub fn ssh_verify(public: &str, ns: &str, data: &[u8], sig: &[u8]) -> bool {
    let dir = TempDir::new("cca-spec-sig-");
    crate::fsx::write(dir.join("signers"), format!("{ns} {}\n", pub_line(public)));
    crate::fsx::write(dir.join("sig"), sig);
    let mut cmd = Cmd::new("ssh-keygen");
    cmd.args(["-Y", "verify", "-f"]).arg(dir.join("signers")).args(["-I", ns, "-n", ns, "-s"]).arg(dir.join("sig"));
    cmd.pass_env(&["PATH"]).input(data).stdout(Stdio::Null).stderr(Stdio::Null);
    cmd.output().unwrap_or_else(|e| panic!("{e}")).status.success()
}

/// 根密钥（私钥文件）签名，命名空间 cca-team；传的是 armored 签名的 base64。签不了就 panic
pub fn team_doc_sign(rootkey: impl AsRef<Path>, tid: &str, dev: &str, seq: i64, ct: &Value) -> String {
    let sig = ssh_sign(rootkey.as_ref(), "cca-team", &team_doc_signed(tid, dev, seq, ct), true).unwrap_or_else(|e| panic!("{e}"));
    b64(&sig)
}

/// 是不是这把根公钥签的（`ssh-keygen -Y verify`；allowed_signers 里只有它）
pub fn team_doc_verify(root_pub: &str, tid: &str, dev: &str, seq: i64, ct: &Value, sig: &str) -> bool {
    ssh_verify(root_pub, "cca-team", &team_doc_signed(tid, dev, seq, ct), &unb64(sig))
}

/// 管理员名单的文字：`{"v": 1, "team", "epoch", "admins": [{"pub", "name"}…]}`，紧凑的 JSON（非 ASCII 原样写）
pub fn team_admins_text(tid: &str, epoch: i64, admins: &[(&str, &str)]) -> String {
    let rows: Vec<Value> = admins.iter().map(|(p, n)| crate::obj! { "pub" => pub_line(p), "name" => *n }).collect();
    crate::obj! { "v" => 1, "team" => tid, "epoch" => epoch, "admins" => Value::List(rows) }.compact()
}

/// 根密钥给名单签名（命名空间 cca-team，签的是 `"cca-team-admins-v1\0"` 接着名单的文字）：armored 签名的 base64
pub fn team_admins_sign(rootkey: impl AsRef<Path>, list: &str) -> String {
    let data = [format!("{TEAM_ADMINS}\0").as_bytes(), list.as_bytes()].concat();
    b64(&ssh_sign(rootkey.as_ref(), "cca-team", &data, true).unwrap_or_else(|e| panic!("{e}")))
}

/// 名单是不是这把根公钥签的
pub fn team_admins_verify(root_pub: &str, list: &str, sig: &str) -> bool {
    let data = [format!("{TEAM_ADMINS}\0").as_bytes(), list.as_bytes()].concat();
    ssh_verify(root_pub, "cca-team", &data, &unb64(sig))
}

/// 共用的状态的两把密钥：root = HMAC(管理员密钥（64 个十六进制字符解出来的 32 个字节）, "cca-team-astate-v1")，再各 HMAC 一次 enc、mac
pub fn astate_keys(akey: &str) -> Keys {
    let root = h(&fromhex(akey), TEAM_ASTATE.as_bytes());
    (h(&root, b"enc"), h(&root, b"mac"))
}

/// 封共用的状态：明文是紧凑的 JSON，空格补齐到 1024 字节的整数倍；认证带着 Team 的编号、版本号（十进制）
pub fn astate_seal(keys: &Keys, tid: &str, rev: i64, doc: &Value) -> Value {
    seal(TEAM_ASTATE, keys, &[tid, &rev.to_string()], &pad(doc.compact().into_bytes(), 1024, b' '))
}

/// 认证过的共用的状态；认证不过（别的 Team、别的版本号、改过的、密钥换了）是 None
pub fn astate_open(keys: &Keys, tid: &str, rev: i64, ct: &Value) -> Option<Value> {
    unseal(TEAM_ASTATE, keys, &[tid, &rev.to_string()], ct).map(|pt| loads(&pt))
}

/// 签过名的请求的 body：extra（put / get 这些）后面接上 v、op、key（签名的那把密钥的指纹）、ts、nonce——extra 里已经有的键
/// 留在原来的位置、换成这里的值（`dict(extra, v=1, …)`）；紧凑的 JSON，非 ASCII 转义
pub fn post_body(extra: &Value, op: &str, key_fp: &str, ts: f64, nonce: &str) -> Vec<u8> {
    let mut body = extra.obj().clone();
    body.insert("v", 1);
    body.insert("op", op);
    body.insert("key", key_fp);
    body.insert("ts", ts);
    body.insert("nonce", nonce);
    json::dumps(&Value::Obj(body), &Style { compact: true, ..Style::default() }).into_bytes()
}

/// 用 keyfile（设备在 Team 里的密钥，或者管理员的根密钥）签名，向 127.0.0.1:port 的测试服务器发 op 请求（extra 是 put / get 这些）；
/// 限流（429）时等 2 秒再试，一共试 6 次。回 (状态码, JSON)；空的回应当成 `{}`。服务器的证书不验（测试证书是现造的自签证书）。
/// 连不上、签不了名、回的不是 JSON 是 Err
pub fn try_post(keyfile: impl AsRef<Path>, port: u16, op: &str, extra: &Value) -> io::Result<(u16, Value)> {
    // 每台设备每 2 秒一个请求，攒得下 30 个
    post_retrying(keyfile.as_ref(), port, op, extra, Duration::from_secs(2))
}

/// [`try_post`] 的本体；pause 是 429 之后等多久（单元测试里调短）
fn post_retrying(keyfile: &Path, port: u16, op: &str, extra: &Value, pause: Duration) -> io::Result<(u16, Value)> {
    let mut pubfile = keyfile.as_os_str().to_os_string();
    pubfile.push(".pub");
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let raw = post_body(extra, op, &fp_of(&pubfile), ts, &token_hex(16));
    let sig = ssh_sign(keyfile, "cca-presence", &raw, false)?;
    let request = http::Request::new("POST", &format!("https://127.0.0.1:{port}/v1/{op}"))
        .header("Content-Type", "application/json")
        .header("X-Cca-Sig", &b64(&sig))
        .body(raw)
        .tls(tls::Client::insecure())
        .timeout(Duration::from_secs(10));
    let mut tries = 0;
    loop {
        let reply = request.send()?;
        let body: &[u8] = if reply.body.is_empty() { b"{}" } else { &reply.body };
        let j = json::parse_bytes(body).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("{op}：服务器回的（{}）不是 JSON：{e}", reply.status))
        })?;
        tries += 1;
        if reply.status != 429 || tries == 6 {
            return Ok((reply.status, j));
        }
        std::thread::sleep(pause);
    }
}

/// 同 [`try_post`]；连不上、签不了名、回的不是 JSON 就 panic（原来是抛异常）
pub fn post(keyfile: impl AsRef<Path>, port: u16, op: &str, extra: &Value) -> (u16, Value) {
    try_post(keyfile, port, op, extra).unwrap_or_else(|e| panic!("{e}"))
}

#[cfg(test)]
mod vectors;
