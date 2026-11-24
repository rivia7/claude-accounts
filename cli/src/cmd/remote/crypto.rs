//! 远程会话里的密码学（格式在 server/README.md「远程会话的中转」；tests/kit/spec.rs 照那一份独立实现了一遍；
//! 下面的测试向量是固定的数据）。只用 HMAC-SHA256（辅助程序已有的 sha2）：
//!
//! - 配对码：10 个随机字节，写成 16 个字符。从它派生配对时交换的那两份密文的密钥，和服务器上认这次配对的编号（rid）；
//!   服务器只见得到 rid 和密文，不知道配对码
//! - 配对出来的密钥 P：两台机器各存一份（钥匙串 / 密钥文件）。服务器只知道它的编号（pid）
//! - 会话的说明（目录）：用 P 派生的密钥封起来，服务器存着转交、看不到是哪个账号、哪个文件夹
//! - 通道：握手时两头各出 32 个字节的随机数、用 P 互相证明，派生这一次连接的四把密钥（两个方向各一对），之后是带序号、
//!   带认证的记录。没有前向保密：P 日后泄露的话，之前被录下的流量可以解开
//!
//! 密文的格式和 API key 同步用的是同一种（keyshare.rs 的 `blob_seal` / `blob_open`）。

use crate::cmd::keyshare::{blob_ad, blob_open, blob_seal, blob_xor, Keys};
use crate::py::base64::urlsafe_b64encode;
use crate::py::hash::{compare_digest, from_hex, hmac_sha256, pbkdf2_sha256, to_hex};
use crate::py::json;
use crate::py::obj::{Dict, V};
use crate::py::rand::urandom;
use crate::py::R;

/// Crockford base32：没有 I、L、O、U
const B32: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
pub const CODE_LEN: usize = 10;

/// 配对码写成 `XXXX-XXXX-XXXX-XXXX`
pub fn code_text(c: &[u8; CODE_LEN]) -> String {
    let n = c.iter().fold(0u128, |n, b| (n << 8) | *b as u128);
    let s: Vec<u8> = (0..16).rev().map(|i| B32[((n >> (5 * i)) & 31) as usize]).collect();
    s.chunks(4).map(|g| String::from_utf8_lossy(g).into_owned()).collect::<Vec<_>>().join("-")
}

/// 输入的配对码（大小写、横线、空格随意；I、L 当 1，O 当 0）；不对是 None
pub fn code_bytes(text: &str) -> Option<[u8; CODE_LEN]> {
    let mut n = 0u128;
    let mut count = 0;
    for ch in text.chars().filter(|c| !matches!(c, '-' | ' ' | '\t' | '\r' | '\n')) {
        let ch = match ch.to_ascii_uppercase() {
            'I' | 'L' => '1',
            'O' => '0',
            c => c,
        };
        n = (n << 5) | B32.iter().position(|b| *b as char == ch)? as u128;
        count += 1;
        if count > 16 {
            return None;
        }
    }
    (count == 16).then(|| {
        let mut out = [0u8; CODE_LEN];
        out.copy_from_slice(&n.to_be_bytes()[16 - CODE_LEN..]);
        out
    })
}

pub fn code_new() -> [u8; CODE_LEN] {
    let mut c = [0u8; CODE_LEN];
    c.copy_from_slice(&urandom(CODE_LEN));
    c
}

/// 从配对码派生的
pub struct PairKeys {
    pub kc: [u8; 32],
    /// 配对时交换的两份密文用的
    pub keys: Keys,
    /// 服务器上认这次配对的编号
    pub rid: String,
}

/// 配对码只有 80 位：从它派生密钥时故意算得慢（PBKDF2，这么多轮），拿着服务器上的记录一个个去试配对码的代价翻这么多倍
pub const CODE_ROUNDS: u32 = 65536;

pub fn pair_keys(code: &[u8]) -> PairKeys {
    let kc = pbkdf2_sha256(code, b"cca-remote/v1/pair", CODE_ROUNDS);
    PairKeys {
        kc,
        keys: (hmac_sha256(&kc, b"enc"), hmac_sha256(&kc, b"mac")),
        rid: to_hex(&hmac_sha256(&kc, b"rid"))[..32].to_string(),
    }
}

/// 空格补齐到 256 个字节的整数倍（从密文的长短看不出里面多长）
fn padded(doc: &Dict) -> Vec<u8> {
    let mut pt = json::dumps(&V::Dict(doc.clone()), json::Fmt::new().utf8().compact()).into_bytes();
    pt.resize(pt.len().div_ceil(256) * 256, b' ');
    pt
}

fn parse(pt: &[u8]) -> Option<Dict> {
    json::loads(std::str::from_utf8(pt).ok()?.trim_end_matches(' ')).ok()?.as_dict()
}

/// 配对时放到服务器上的那一份。role："a" 是显示配对码的那台，"b" 是输入的那台
pub fn pair_msg(keys: &Keys, rid: &str, role: &str, fp: &str, nonce: &[u8], name: &str) -> R<Dict> {
    let doc = crate::dict! { "v" => 1, "fp" => fp, "n" => to_hex(nonce), "name" => name };
    Ok(blob_seal(keys, &blob_ad(&["cca-remote-pair", rid, role])?, &padded(&doc)))
}

/// 对方放的那一份：(它的指纹, 它的随机数, 它的名字)；认证不过（不知道配对码的人放的）、格式不对是 None
pub fn pair_open(keys: &Keys, rid: &str, role: &str, blob: &V) -> Option<(String, Vec<u8>, String)> {
    let doc = parse(&blob_open(keys, &blob_ad(&["cca-remote-pair", rid, role]).ok()?, blob)?)?;
    let nonce = from_hex(&doc.str("n")?).filter(|n| n.len() == 32)?;
    (doc.at("v").as_int() == Some(1)).then_some((doc.str("fp")?.to_string(), nonce, doc.str("name")?.to_string()))
}

/// 两台机器之间的密钥。a 是显示配对码的那台
pub fn pair_secret(kc: &[u8], na: &[u8], nb: &[u8], fpa: &str, fpb: &str) -> [u8; 32] {
    hmac_sha256(kc, &[b"secret\0", na, nb, fpa.as_bytes(), b"\0", fpb.as_bytes(), b"\0"].concat())
}

/// 这把密钥的编号（服务器只知道它）
pub fn pid_of(p: &[u8]) -> String {
    to_hex(&hmac_sha256(p, b"cca-remote/v1/pid"))[..32].to_string()
}

fn dir_keys(p: &[u8]) -> Keys {
    let k = hmac_sha256(p, b"cca-remote/v1/dir");
    (hmac_sha256(&k, b"enc"), hmac_sha256(&k, b"mac"))
}

/// 会话的说明，封给这个配对的另一台机器看
pub fn dir_seal(p: &[u8], pid: &str, sid: &str, doc: &Dict) -> R<Dict> {
    Ok(blob_seal(&dir_keys(p), &blob_ad(&["cca-remote-dir", pid, sid])?, &padded(doc)))
}

pub fn dir_open(p: &[u8], pid: &str, sid: &str, blob: &V) -> Option<Dict> {
    parse(&blob_open(&dir_keys(p), &blob_ad(&["cca-remote-dir", pid, sid]).ok()?, blob)?)
}

pub const VER: u8 = 1;
pub const HELLO_LEN: usize = 52;
pub const REPLY_LEN: usize = 68;
pub const CONFIRM_LEN: usize = 35;
/// 一条记录里内容最长这么多
pub const RECORD_MAX: usize = 16384;

#[derive(Clone, Copy, PartialEq)]
pub enum Role {
    Host,
    Viewer,
}

/// 两头之间的加密通道。握手：接入方发 `hello`，宿主回 `reply`，接入方再发 `confirm`（宿主 `accept`）。
/// 之后 `seal` / `open`：两个方向各用各的密钥、各从 0 数序号
pub struct Channel {
    role: Role,
    ka: [u8; 32],
    pid: Vec<u8>,
    sid: Vec<u8>,
    /// 各自记着的宿主、接入方的指纹（配对时存下的，不是服务器说的）
    fph: String,
    fpv: String,
    nv: Vec<u8>,
    nh: Vec<u8>,
    tagh: [u8; 32],
    out: Keys,
    inn: Keys,
    send_seq: u64,
    recv_seq: u64,
}

impl Channel {
    /// pid、sid 是 32 个十六进制字符；不是的话 None
    pub fn new(role: Role, p: &[u8], pid: &str, sid: &str, fph: &str, fpv: &str) -> Option<Channel> {
        let (pid, sid) = (from_hex(pid).filter(|x| x.len() == 16)?, from_hex(sid).filter(|x| x.len() == 16)?);
        Some(Channel {
            role,
            ka: hmac_sha256(p, b"cca-remote/v1/auth"),
            pid,
            sid,
            fph: fph.to_string(),
            fpv: fpv.to_string(),
            nv: Vec::new(),
            nh: Vec::new(),
            tagh: [0; 32],
            out: ([0; 32], [0; 32]),
            inn: ([0; 32], [0; 32]),
            send_seq: 0,
            recv_seq: 0,
        })
    }

    /// 握手的两头都认的那一串：版本、配对、会话、两个随机数、两头的指纹。换一个会话、换一对机器、重放旧的都对不上
    fn ctx(&self) -> Vec<u8> {
        [&[VER][..], &self.pid, &self.sid, &self.nv, &self.nh, self.fph.as_bytes(), b"\0", self.fpv.as_bytes(), b"\0"].concat()
    }

    fn tag_host(&self) -> [u8; 32] {
        hmac_sha256(&self.ka, &[b"host\0", &self.ctx()[..]].concat())
    }

    fn tag_viewer(&self) -> [u8; 32] {
        hmac_sha256(&self.ka, &[b"viewer\0", &self.ctx()[..], &self.tagh].concat())
    }

    fn derive(&mut self, tagv: &[u8]) {
        let ks = hmac_sha256(&self.ka, &[b"session\0", &self.ctx()[..], &self.tagh, tagv].concat());
        let pair =
            |dir: &str| (hmac_sha256(&ks, format!("{dir}-enc").as_bytes()), hmac_sha256(&ks, format!("{dir}-mac").as_bytes()));
        let (mine, theirs) = if self.role == Role::Host { ("h2v", "v2h") } else { ("v2h", "h2v") };
        self.out = pair(mine);
        self.inn = pair(theirs);
    }

    /// 接入方 → 宿主
    pub fn hello(&mut self) -> Vec<u8> {
        self.hello_with(urandom(32))
    }

    fn hello_with(&mut self, nv: Vec<u8>) -> Vec<u8> {
        self.nv = nv;
        [b"CR\x01", &[VER][..], &self.nv, &self.pid].concat()
    }

    /// hello 里说的是哪个配对（宿主先看它，才知道用哪把密钥）
    pub fn hello_pid(hello: &[u8]) -> Option<String> {
        (hello.len() == HELLO_LEN && hello[..4] == [b'C', b'R', 1, VER]).then(|| to_hex(&hello[36..]))
    }

    /// 宿主 → 接入方；hello 不对是 None
    pub fn reply(&mut self, hello: &[u8]) -> Option<Vec<u8>> {
        self.reply_with(hello, urandom(32))
    }

    fn reply_with(&mut self, hello: &[u8], nh: Vec<u8>) -> Option<Vec<u8>> {
        if Channel::hello_pid(hello)? != to_hex(&self.pid) {
            return None;
        }
        self.nv = hello[4..36].to_vec();
        self.nh = nh;
        self.tagh = self.tag_host();
        Some([b"CR\x02", &[VER][..], &self.nh, &self.tagh].concat())
    }

    /// 接入方 → 宿主；宿主的证明不对（它没有这把密钥）是 None：之后什么都不发给它
    pub fn confirm(&mut self, reply: &[u8]) -> Option<Vec<u8>> {
        if reply.len() != REPLY_LEN || reply[..4] != [b'C', b'R', 2, VER] {
            return None;
        }
        self.nh = reply[4..36].to_vec();
        self.tagh.copy_from_slice(&reply[36..]);
        if !compare_digest(&self.tagh, &self.tag_host()) {
            return None;
        }
        let tagv = self.tag_viewer();
        self.derive(&tagv);
        Some([&b"CR\x03"[..], &tagv].concat())
    }

    /// 宿主收到接入方的证明：对不对
    pub fn accept(&mut self, confirm: &[u8]) -> bool {
        if confirm.len() != CONFIRM_LEN || confirm[..3] != [b'C', b'R', 3] || !compare_digest(&confirm[3..], &self.tag_viewer()) {
            return false;
        }
        self.derive(&confirm[3..]);
        true
    }

    /// 一条记录：`序号(8) 密文长度(2) 密文 认证(16)`；明文是 `长度(2) 内容 零补齐到 32 的整数倍`。data 最长 RECORD_MAX
    pub fn seal(&mut self, data: &[u8]) -> Vec<u8> {
        debug_assert!(data.len() <= RECORD_MAX);
        let mut pt = (data.len() as u16).to_be_bytes().to_vec();
        pt.extend_from_slice(data);
        pt.resize(pt.len().div_ceil(32) * 32, 0);
        let seq = self.send_seq.to_be_bytes();
        self.send_seq += 1;
        let ct = blob_xor(&self.out.0, &seq, &pt);
        let mut rec = [&seq[..], &(ct.len() as u16).to_be_bytes(), &ct].concat();
        let tag = hmac_sha256(&self.out.1, &rec);
        rec.extend_from_slice(&tag[..16]);
        rec
    }

    /// buf 开头的一条记录：内容（从 buf 里拿掉）；还不够一条是 Ok(None)；序号、认证、长度不对是 Err（断开）
    pub fn open(&mut self, buf: &mut Vec<u8>) -> Result<Option<Vec<u8>>, ()> {
        if buf.len() < 10 {
            return Ok(None);
        }
        let n = u16::from_be_bytes([buf[8], buf[9]]) as usize;
        if n == 0 || !n.is_multiple_of(32) || n > RECORD_MAX + 2 + 31 {
            return Err(());
        }
        if buf.len() < 10 + n + 16 {
            return Ok(None);
        }
        let tag = hmac_sha256(&self.inn.1, &buf[..10 + n]);
        if buf[..8] != self.recv_seq.to_be_bytes() || !compare_digest(&buf[10 + n..26 + n], &tag[..16]) {
            return Err(());
        }
        let pt = blob_xor(&self.inn.0, &buf[..8], &buf[10..10 + n]);
        self.recv_seq += 1;
        buf.drain(..26 + n);
        let size = u16::from_be_bytes([pt[0], pt[1]]) as usize;
        if size > n - 2 || pt[2 + size..].iter().any(|b| *b != 0) {
            return Err(());
        }
        Ok(Some(pt[2..2 + size].to_vec()))
    }
}

/// 连数据端口时说的那一行，和服务器该回的那一行。role：`host` / `view` 用票据的编号和密钥，`join` 用令牌和控制连接
/// 那张票据的密钥。第一行是可见的 ASCII（有的网络会掐掉一上来就全是随机字节的连接）
pub fn relay_line(role: &str, ident: &str, key: &[u8], cn: &str) -> (String, String) {
    let mac = |lead: &str| {
        urlsafe_b64encode(&hmac_sha256(key, format!("{lead}{ident}{cn}").as_bytes())).trim_end_matches('=').to_string()
    };
    let lead = if role == "join" { "j".to_string() } else { format!("c{role}") };
    (format!("CCA-RELAY/1 {role} {ident} {cn} {}\n", mac(&lead)), format!("CCA-RELAY/1 OK {}\n", mac("s")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FPA: &str = "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    const FPB: &str = "SHA256:BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
    const SID: &str = "00112233445566778899aabbccddeeff";

    fn code() -> [u8; 10] {
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
    }

    /// 从配对码派生是故意算得慢的：测试里各算一次
    fn keys() -> &'static PairKeys {
        static K: std::sync::LazyLock<PairKeys> = std::sync::LazyLock::new(|| pair_keys(&code()));
        &K
    }

    fn other_keys() -> &'static PairKeys {
        static K: std::sync::LazyLock<PairKeys> = std::sync::LazyLock::new(|| pair_keys(&[9; 10]));
        &K
    }

    fn secret() -> [u8; 32] {
        pair_secret(&keys().kc, &[0x11; 32], &[0x22; 32], FPA, FPB)
    }

    /// 照文档独立实现的那一份算出来的（固定的数据）：两边要逐字节一样
    #[test]
    fn matches_the_reference_vectors() {
        assert_eq!(code_text(&code()), "0410-6105-0R3G-G28A");
        let k = keys();
        assert_eq!(to_hex(&k.kc), "ff1325fce67621a93b40804d02cc5b920c79bc5bd6c2fd56612071065da2427d");
        assert_eq!(to_hex(&k.keys.0), "cb0df8f9f466f78152777f2a9ed109dc46338bcd27e97480346c147f4183d425");
        assert_eq!(to_hex(&k.keys.1), "03e81e7250d7236679675169c168b6a0e211f6e316295b9749502cf34cf5ac93");
        assert_eq!(k.rid, "581cbbde70977fb6ad7e5d04762f73b5");
        let p = secret();
        assert_eq!(to_hex(&p), "7adfdd29f6b23870c135b81e6494fb6c8d24e0af30f50632ca858c5129c3c58a");
        let pid = pid_of(&p);
        assert_eq!(pid, "36ac6aaa256ba92dfa89065e98f1c4f3");
        let dk = dir_keys(&p);
        assert_eq!(to_hex(&dk.0), "2b41224850f5c0cbee2ae495409489722980a9f539333ef5267740d13b00fb4d");
        assert_eq!(to_hex(&dk.1), "5fb66881d76be202cbc0f5701a371a05034e6c103522832cd5d4e32a1f78e40e");

        let mut v = Channel::new(Role::Viewer, &p, &pid, SID, FPA, FPB).unwrap();
        let mut h = Channel::new(Role::Host, &p, &pid, SID, FPA, FPB).unwrap();
        let hello = v.hello_with(vec![0x33; 32]);
        assert_eq!(
            to_hex(&hello),
            "43520101333333333333333333333333333333333333333333333333333333333333333336ac6aaa256ba92dfa89065e98f1c4f3"
        );
        let reply = h.reply_with(&hello, vec![0x44; 32]).unwrap();
        assert_eq!(
            to_hex(&reply),
            "4352020144444444444444444444444444444444444444444444444444444444444444445ff40933b629041ad83cd1e0f0d7aef42340afb54c499460a00f04071be0848e"
        );
        let confirm = v.confirm(&reply).unwrap();
        assert_eq!(to_hex(&confirm), "435203f866ad6f9b8eadb30965822033478cbb5c0ef01f18340d51c58e2c556cbc0606");
        assert!(h.accept(&confirm));
        assert_eq!(
            to_hex(&h.seal("你好 hello".as_bytes())),
            "00000000000000000020492a06ee59af5627d4297055e19fe2fa6ac2e07de7569ad550c6da5b61c398523a84149581e28d934f9e90bc6b46baef"
        );
        assert_eq!(
            to_hex(&h.seal(b"")),
            "00000000000000010020734affce5b7ab7817c1d81c42f9b8693ec966bb1142ce7f675cdad4f011d3cd98381ae4470bbd55dfd28dd1ac32a2078"
        );
        assert_eq!(
            to_hex(&v.seal(&[b'k'; 100])),
            "00000000000000000080651b27b6b0545df2e72c30dbabc7edfbfd9225699ed0276cab6d14ea7a494f91446095a754c9bdb1b25fee2b972acace976b4517a911852680dd4fa4d72437111571b6b2f68c444d84600c4d69327484f667019f66f957086cc7ff338473cf706c7fe3895570ac2b60f1ea330037c4dd12c2e36c30193a0b9c37ae17b791bd473ea00771f42b6414b6490473b38c9f7b"
        );
        let (line, ok) = relay_line("host", &"ab".repeat(16), &[7; 32], "0123456789abcdef");
        assert_eq!(
            line,
            "CCA-RELAY/1 host abababababababababababababababab 0123456789abcdef gLe_8CQuzYdrXbCC4GE_-a3pjz7bYwCZ_jvmEMJ1HfY\n"
        );
        assert_eq!(ok, "CCA-RELAY/1 OK WM1jPui86RX1yQXMbJMbO9Awmn04Y80TpSgn3DhFQRA\n");
        let (line, ok) = relay_line("join", &"cd".repeat(16), &[7; 32], "0123456789abcdef");
        assert_eq!(
            line,
            "CCA-RELAY/1 join cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd 0123456789abcdef bfdoPmRWMZnFPMOjFkxZv-H6gfMKherzxOgNV6krD6s\n"
        );
        assert_eq!(ok, "CCA-RELAY/1 OK kGt4eo8Pp0W6i0ECgylnJCowEO__254sY0NtEEhgCRU\n");
    }

    #[test]
    fn codes() {
        for _ in 0..50 {
            let c = code_new();
            let text = code_text(&c);
            assert_eq!(text.len(), 19);
            assert_eq!(code_bytes(&text), Some(c));
            assert_eq!(code_bytes(&text.to_lowercase().replace('-', " ")), Some(c));
        }
        // I、L 当 1，O 当 0；长度不对、有别的字符不认
        assert_eq!(code_bytes("O41O-61O5-OR3G-G28A"), Some(code()));
        assert_eq!(code_bytes("0410-6l05-0R3G-G28A").map(|c| code_text(&c)), Some("0410-6105-0R3G-G28A".into()));
        for bad in ["", "0410-6105-0R3G-G28", "0410-6105-0R3G-G28AA", "0410-6105-0R3G-G28U", "0410-6105-0R3G-G2!A"] {
            assert_eq!(code_bytes(bad), None, "{bad}");
        }
    }

    fn paired() -> (Channel, Channel) {
        let p = secret();
        let pid = pid_of(&p);
        let mut v = Channel::new(Role::Viewer, &p, &pid, SID, FPA, FPB).unwrap();
        let mut h = Channel::new(Role::Host, &p, &pid, SID, FPA, FPB).unwrap();
        let hello = v.hello();
        let reply = h.reply(&hello).unwrap();
        assert!(h.accept(&v.confirm(&reply).unwrap()));
        (h, v)
    }

    #[test]
    fn records_round_trip_and_reject_tampering() {
        let (mut h, mut v) = paired();
        let mut wire = Vec::new();
        let big = vec![0xa5u8; RECORD_MAX];
        for data in [&b"x"[..], b"", "多字节 ✓".as_bytes(), &big] {
            wire.extend(h.seal(data));
        }
        // 一个字节一个字节地到
        let (mut buf, mut got) = (Vec::new(), Vec::new());
        for b in &wire {
            buf.push(*b);
            while let Some(data) = v.open(&mut buf).unwrap() {
                got.push(data);
            }
        }
        assert_eq!(got, vec![b"x".to_vec(), b"".to_vec(), "多字节 ✓".as_bytes().to_vec(), big]);
        assert!(buf.is_empty());
        // 两个方向的密钥不一样：自己发的记录自己收不认（不能把话原样弹回去）
        let mut echo = v.seal(b"hi");
        assert!(v.open(&mut echo.clone()).is_err());
        // 改一个字节、重放、跳号、长度离谱：都断开
        let rec = h.seal(b"secret");
        for i in [0, 9, 12, rec.len() - 1] {
            let mut bad = rec.clone();
            bad[i] ^= 1;
            assert!(v.clone_state().open(&mut bad).is_err(), "改了第 {i} 个字节");
        }
        let mut twice = [rec.clone(), rec.clone()].concat();
        assert_eq!(v.open(&mut twice).unwrap(), Some(b"secret".to_vec()));
        assert!(v.open(&mut twice).is_err(), "同一条再来一遍");
        let _skipped = h.seal(b"one");
        let mut later = h.seal(b"two");
        assert!(v.clone_state().open(&mut later).is_err(), "中间少了一条");
        assert!(h.open(&mut echo).is_ok());
        assert!(h.open(&mut vec![0, 0, 0, 0, 0, 0, 0, 1, 0xff, 0xff, 0, 0]).is_err());
    }

    #[test]
    fn handshake_needs_the_same_secret_session_and_peers() {
        let p = secret();
        let pid = pid_of(&p);
        let other = pair_secret(&other_keys().kc, &[0x11; 32], &[0x22; 32], FPA, FPB);
        let host = |p: &[u8], sid: &str, fph: &str| Channel::new(Role::Host, p, &pid, sid, fph, FPB).unwrap();
        let mut v = Channel::new(Role::Viewer, &p, &pid, SID, FPA, FPB).unwrap();
        let hello = v.hello();
        assert_eq!(Channel::hello_pid(&hello), Some(pid.clone()));
        // 没有这把密钥的“宿主”、另一个会话的、指纹不是这一台的：接入方不认它的证明，什么都不再发
        for mut fake in [
            host(&other, SID, FPA),
            host(&p, "ffffffffffffffffffffffffffffffff", FPA),
            host(&p, SID, "SHA256:CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC"),
        ] {
            let reply = fake.reply(&hello).unwrap();
            assert!(v.confirm(&reply).is_none());
        }
        // 别的配对的 hello：宿主不回
        let mut h = host(&p, SID, FPA);
        let mut wrong = hello.clone();
        wrong[40] ^= 1;
        assert!(h.reply(&wrong).is_none());
        assert!(h.reply(&hello[..51]).is_none());
        // 没有这把密钥的“接入方”：宿主不认；录下来的旧证明对新的握手没用（随机数不一样）
        let reply = h.reply(&hello).unwrap();
        let good = v.confirm(&reply).unwrap();
        let mut fake = Channel::new(Role::Viewer, &other, &pid, SID, FPA, FPB).unwrap();
        fake.hello_with(hello[4..36].to_vec());
        fake.nh = reply[4..36].to_vec();
        fake.tagh.copy_from_slice(&reply[36..]);
        let forged = [&b"CR\x03"[..], &fake.tag_viewer()].concat();
        assert!(!h.accept(&forged));
        let mut again = host(&p, SID, FPA);
        again.reply(&hello).unwrap();
        assert!(!again.accept(&good));
        assert!(h.accept(&good));
    }

    #[test]
    fn pairing_messages_and_directory() {
        let k = keys();
        let m = pair_msg(&k.keys, &k.rid, "b", FPB, &[0x22; 32], "办公室的 Mac").unwrap();
        assert_eq!(m.str("c").unwrap().len() % 4, 0);
        let got = pair_open(&k.keys, &k.rid, "b", &V::Dict(m.clone())).unwrap();
        assert_eq!(got, (FPB.to_string(), vec![0x22; 32], "办公室的 Mac".to_string()));
        // 角色、这次配对的编号、配对码有一样不对：不认
        assert!(pair_open(&k.keys, &k.rid, "a", &V::Dict(m.clone())).is_none());
        assert!(pair_open(&k.keys, "00000000000000000000000000000000", "b", &V::Dict(m.clone())).is_none());
        assert!(pair_open(&other_keys().keys, &k.rid, "b", &V::Dict(m)).is_none());
        // 换了先后（谁显示的码）密钥就不一样
        assert_ne!(pair_secret(&k.kc, &[1; 32], &[2; 32], FPA, FPB), pair_secret(&k.kc, &[2; 32], &[1; 32], FPB, FPA));

        let p = secret();
        let pid = pid_of(&p);
        let doc = crate::dict! { "v" => 1, "machine" => "服务器", "tool" => "claude", "name" => "工作", "cwd" => "~/项目", "since" => 1794000000 };
        let blob = dir_seal(&p, &pid, SID, &doc).unwrap();
        let got = dir_open(&p, &pid, SID, &V::Dict(blob.clone())).unwrap();
        assert_eq!(
            (got.str("machine").unwrap().to_string(), got.str("cwd").unwrap().to_string()),
            ("服务器".into(), "~/项目".into())
        );
        assert!(dir_open(&p, &pid, "ffffffffffffffffffffffffffffffff", &V::Dict(blob.clone())).is_none(), "挪到别的会话上不认");
        assert!(dir_open(&[0; 32], &pid, SID, &V::Dict(blob)).is_none());
    }

    impl Channel {
        /// 测试里从同一个状态试几种坏的记录
        fn clone_state(&self) -> Channel {
            Channel {
                role: self.role,
                ka: self.ka,
                pid: self.pid.clone(),
                sid: self.sid.clone(),
                fph: self.fph.clone(),
                fpv: self.fpv.clone(),
                nv: self.nv.clone(),
                nh: self.nh.clone(),
                tagh: self.tagh,
                out: self.out,
                inn: self.inn,
                send_seq: self.send_seq,
                recv_seq: self.recv_seq,
            }
        }
    }
}
