//! Team 的线上格式（server/README.md「Team 的同步服务器」的「邀请链接」「设备的配置」）：Team 的编号、邀请链接、从邀请里那串随机数
//! 派生的令牌 / 设备编号 / 密钥、每台设备的配置（密文 + 根密钥的签名）和配置明文的样子。tests/kit/spec.rs 里有照文档独立写的一份。
//! 这里单元测试的对照数据是固定的，没有重新生成它的命令：改了格式，两边的代码和这些数据一起换。

use crate::cmd::hub::{hex32, link_sum};
use crate::cmd::keyshare::{blob_ad, blob_open, blob_seal, Keys};
use crate::cmd::presence::{url_ok, PRESENCE_PIN_RE, PRESENCE_SALT_RE};
use crate::core::policy::{alias_ok, fill_problems};
use crate::core::tool::tools;
use crate::dict;
use crate::py::base64::{b64decode, b64encode, urlsafe_b64decode, urlsafe_b64encode};
use crate::py::fs::TempDir;
use crate::py::hash::{from_hex, hmac_sha256, sha256, sha256_hex, to_hex};
use crate::py::obj::{Dict, V};
use crate::py::proc::{Cmd as Proc, Out};
use crate::py::{json, path, signal, R};

/// 邀请链接的开头（旧版本必须认得才安全的改动升到 cca-team2.；旧版本不认得也不要紧的字段直接加）
pub const LINK_PREFIX: &str = "cca-team1.";
/// 配置的密文的标签（认证、签名里都带着）
const DOC_LABEL: &str = "cca-team-doc-v1";
/// 根密钥给配置签名的命名空间
const SIGN_NS: &str = "cca-team";
/// 明文补齐到这么长的整数倍
const DOC_PAD: usize = 1024;
/// 服务器收的一份密文（base64）最长这么多个字符（cli/src/server/mod.rs 的 MAX_TEAM_DOC）
pub const DOC_MAX: usize = 131072;
/// 序号比它小（服务器的 SEQ_MAX）
pub const SEQ_MAX: i64 = 1 << 53;

/// 根公钥写成 `ssh-ed25519 <base64>`（只留前两段）；不是 ed25519 公钥是 None
pub fn root_line(s: &str) -> Option<String> {
    let mut f = s.split_whitespace();
    let (kind, b64) = (f.next()?, f.next()?);
    let raw = b64decode(b64, true).ok()?;
    // ed25519 公钥的线上格式：string "ssh-ed25519" + string 32 个字节
    (kind == "ssh-ed25519" && raw.len() == 51 && raw.starts_with(b"\0\0\0\x0bssh-ed25519\0\0\0\x20"))
        .then(|| format!("ssh-ed25519 {b64}"))
}

/// Team 的编号：根公钥（base64 那一段解出来的字节）的 SHA-256 的前 32 个十六进制字符
pub fn team_id(root: &str) -> Option<String> {
    let line = root_line(root)?;
    let raw = b64decode(line.split(' ').nth(1)?, true).ok()?;
    Some(sha256_hex(raw)[..32].to_string())
}

/// 邀请链接里的：服务器的地址、证书公钥的指纹（没有是照证书机构验）、根公钥、那串随机数、Team 的名字、什么时候过期（给人看的）、
/// 管理员给这台设备起的名字（3.5.3-beta.7 起；没有是空的：加入时没写 --name 就用它，旧版本不认得也不要紧）
#[derive(Clone, Debug, PartialEq)]
pub struct Invite {
    pub url: String,
    pub pins: Vec<String>,
    pub root: String,
    pub secret: String,
    pub name: String,
    pub exp: Option<i64>,
    pub device: String,
}

/// Team 的名字：1～64 个字符，没有控制字符，前后没有空白
pub fn team_name_ok(n: &str) -> bool {
    !n.is_empty() && n.trim() == n && n.chars().count() <= 64 && !n.chars().any(|c| c.is_control())
}

pub fn invite_encode(i: &Invite) -> String {
    let doc = dict! {
        "u" => i.url.as_str(), "p" => i.pins.iter().map(|p| V::from(p.as_str())).collect::<Vec<V>>(),
        "r" => i.root.split(' ').nth(1).unwrap_or(""), "k" => i.secret.as_str(), "n" => i.name.as_str(),
    };
    if let Some(e) = i.exp {
        doc.set("e", e);
    }
    if !i.device.is_empty() {
        doc.set("d", i.device.as_str());
    }
    let raw = json::dumps(&V::Dict(doc), json::Fmt::new().utf8().compact());
    let head = format!("{LINK_PREFIX}{}", urlsafe_b64encode(raw.as_bytes()).trim_end_matches('='));
    let sum = link_sum(&head);
    format!("{head}.{sum}")
}

/// 读邀请链接：中间的空白不算（聊天软件里贴过来可能折了行）；不对是给人看的原因
pub fn invite_decode(raw: &str) -> Result<Invite, String> {
    let s: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    if s.is_empty() {
        return Err("没有输入邀请链接".into());
    }
    let Some(rest) = s.strip_prefix(LINK_PREFIX) else {
        if s.starts_with("cca-team") {
            return Err("这个邀请链接是更新版本的 cca 生成的：先更新 cca（cca update）再加入".into());
        }
        if s.starts_with("cca-hub") {
            return Err("这是自己的同步服务器的加入链接（cca 3.5.3 起没有自己的同步服务器了），不是 Team 的邀请链接：向 Team 的管理员要邀请链接".into());
        }
        return Err("这不是邀请链接（邀请链接以 cca-team1. 开头）".into());
    };
    let broken = || "邀请链接不完整或者抄错了（复制时少了一截？）：重新复制一次".to_string();
    let Some((body, sum)) = rest.split_once('.') else { return Err(broken()) };
    if body.is_empty() || !body.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(broken());
    }
    if link_sum(&format!("{LINK_PREFIX}{body}")) != sum {
        return Err(broken());
    }
    let pad = "=".repeat((4 - body.len() % 4) % 4);
    let doc = urlsafe_b64decode(&format!("{body}{pad}"))
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|t| json::loads(&t).ok())
        .and_then(|j| j.as_dict())
        .ok_or_else(broken)?;
    let s = |k: &str| doc.str(k).map(|v| v.to_string()).unwrap_or_default();
    let pins: Vec<String> = doc.list("p").map(|p| p.items().iter().map(|x| x.py_str()).collect()).unwrap_or_default();
    let url = s("u").trim_end_matches('/').to_string();
    let why = if !url_ok(&url) {
        "服务器的地址不对"
    } else if pins.len() > 4 || !pins.iter().all(|p| PRESENCE_PIN_RE.fullmatch(p).is_some()) {
        "服务器证书的指纹不对"
    } else if !hex32(&s("k")) {
        "那串随机数不对"
    } else if !team_name_ok(&s("n")) {
        "Team 的名字不对"
    } else {
        ""
    };
    let root = root_line(&format!("ssh-ed25519 {}", s("r")));
    match (why, root) {
        ("", Some(root)) => {
            let exp = doc.get("e").and_then(|e| e.as_int()).filter(|e| *e > 0);
            // 设备的名字不对不算链接不对：加入时不用它就是了（照主机名）
            Ok(Invite { url, pins, root, secret: s("k"), name: s("n"), exp, device: s("d") })
        }
        (why, _) => {
            let why = if why.is_empty() { "根公钥不对" } else { why };
            Err(format!("邀请链接里{why}：向 Team 的管理员要一个新的"))
        }
    }
}

/// 邀请里那串随机数派生出来的：加入时交给服务器的令牌、这台设备在 Team 里的编号、它那份配置的两把密钥
pub struct Parts {
    pub token: String,
    pub dev: String,
    pub keys: Keys,
}

/// HMAC 的密钥是那串随机数解出来的 16 个字节；不是 32 个小写十六进制字符是 None
pub fn invite_parts(secret: &str) -> Option<Parts> {
    let k = from_hex(secret).filter(|_| hex32(secret))?;
    let root = hmac_sha256(&k, b"cca-team-key-v1");
    Some(Parts {
        token: to_hex(&hmac_sha256(&k, b"cca-team-token-v1")[..16]),
        dev: to_hex(&hmac_sha256(&k, b"cca-team-dev-v1")[..8]),
        keys: (hmac_sha256(&root, b"enc"), hmac_sha256(&root, b"mac")),
    })
}

/// 设备加入时给自己的公钥算的认证码：HMAC(HMAC(k, "cca-team-pub-v1"), 公钥那一行的前两段)，base64。管理员照自己算的 k 核对：
/// 对得上，这把公钥就真是拿着邀请的那台设备的（服务器不知道 k，换不了）
pub fn pub_proof(secret: &str, pub_line: &str) -> Option<String> {
    let k = from_hex(secret).filter(|_| hex32(secret))?;
    let line = root_line(pub_line)?;
    Some(b64encode(&hmac_sha256(&hmac_sha256(&k, b"cca-team-pub-v1"), line.as_bytes())))
}

/// 服务器存的令牌的哈希：那 32 个字符的 SHA-256
pub fn token_hash(token: &str) -> String {
    sha256_hex(token)
}

fn doc_ad(tid: &str, dev: &str, seq: i64) -> R<Vec<u8>> {
    blob_ad(&[DOC_LABEL, tid, dev, &seq.to_string()])
}

/// 一台设备的配置 → 服务器存的密文：明文是紧凑的 JSON（UTF-8），空格补齐到 1024 字节的整数倍；认证带着 Team、设备、序号
pub fn doc_seal(keys: &Keys, tid: &str, dev: &str, seq: i64, doc: &Dict) -> R<Dict> {
    let mut pt = json::dumps(&V::Dict(doc.copy()), json::Fmt::new().utf8().compact()).into_bytes();
    pt.resize(pt.len().div_ceil(DOC_PAD) * DOC_PAD, b' ');
    Ok(blob_seal(keys, &doc_ad(tid, dev, seq)?, &pt))
}

/// doc_seal 反过来；认证不过（别的设备的、别的序号的、改过的）、不是 JSON 对象是 None
pub fn doc_open(keys: &Keys, tid: &str, dev: &str, seq: i64, ct: &V) -> Option<Dict> {
    let pt = blob_open(keys, &doc_ad(tid, dev, seq).ok()?, ct)?;
    json::loads(std::str::from_utf8(&pt).ok()?.trim_end_matches(' ')).ok()?.as_dict()
}

/// 根密钥签的那一段字节：标签、Team、设备、序号各跟一个 \0，接着 nonce（16 个字节）、密文、认证码（32 个字节）；长度不对是 None
pub fn doc_signed(tid: &str, dev: &str, seq: i64, ct: &V) -> Option<Vec<u8>> {
    let ct = ct.as_dict()?;
    let part = |k: &str| b64decode(ct.get(k)?.as_str()?, true).ok();
    let (n, c, t) = (part("n")?, part("c")?, part("t")?);
    if n.len() != 16 || t.len() != 32 || c.is_empty() {
        return None;
    }
    let mut msg = doc_ad(tid, dev, seq).ok()?;
    msg.extend_from_slice(&n);
    msg.extend_from_slice(&c);
    msg.extend_from_slice(&t);
    Some(msg)
}

/// 拿根密钥（私钥文件）签名：armored 的输出再 base64；签不了是 None
pub fn doc_sign(root_key: &str, msg: &[u8]) -> R<Option<String>> {
    let done = Proc::new(["ssh-keygen", "-Y", "sign", "-q", "-n", SIGN_NS, "-f", root_key])
        .input(msg)
        .stdout(Out::Capture)
        .stderr(Out::Null)
        .timeout(10.0)
        .run();
    signal::check()?;
    Ok(done.ok().filter(|d| d.rc == 0 && !d.stdout.is_empty()).map(|d| b64encode(&d.stdout)))
}

/// 是不是这把公钥（根公钥，或者管理员名单里的一把）签的（ssh-keygen -Y verify：allowed_signers 里只有它，命名空间 cca-team）。
/// None：验不了（ssh-keygen 起不来、超时）——和验不过分开说，免得把“这台设备的 ssh-keygen 用不了”说成有人冒充服务器；两种都不用这份配置
pub fn doc_verify(root: &str, msg: &[u8], sig: &str) -> R<Option<bool>> {
    let (Some(root), Ok(raw)) = (root_line(root), b64decode(sig, true)) else { return Ok(Some(false)) };
    let tmp = TempDir::new()?;
    let (signers, sigfile) = (path::join(&tmp.path, "signers"), path::join(&tmp.path, "sig"));
    crate::py::fs::put(&signers, format!("{SIGN_NS} {root}\n").as_bytes(), 0o600)?;
    crate::py::fs::put(&sigfile, &raw, 0o600)?;
    let done =
        Proc::new(["ssh-keygen", "-Y", "verify", "-f", signers.as_str(), "-I", SIGN_NS, "-n", SIGN_NS, "-s", sigfile.as_str()])
            .input(msg)
            .stdout(Out::Null)
            .stderr(Out::Null)
            .timeout(10.0)
            .run();
    signal::check()?;
    Ok(done.ok().map(|d| d.rc == 0))
}

/// 解开的一份配置：序号、配置，和签名的是不是根密钥（不是的话是哪一把：管理员名单里的）
pub struct Opened {
    pub seq: i64,
    pub doc: Dict,
    /// 签名的那把公钥那一行；None 是根密钥签的
    pub by: Option<String>,
}

/// 设备取回来的那一份（{"seq", "ct", "sig", "by"}）：验过签名和认证、解开的配置；不对是给人看的原因。by（3.6.15 起）是签名的管理员的
/// 公钥：要在 admins（验过根密钥签名的管理员名单，调用方给）里，名单的纪元不比配置里说的 admins_epoch 小
pub fn envelope_open(
    root: &str,
    keys: &Keys,
    tid: &str,
    dev: &str,
    env: &Dict,
    admins: Option<&AdminList>,
) -> R<Result<Opened, String>> {
    let Some(seq) = env.get("seq").and_then(|s| s.as_int()).filter(|s| (1..SEQ_MAX).contains(s)) else {
        return Ok(Err("服务器给的配置样子不对".into()));
    };
    let (ct, sig) = (env.at("ct"), env.at("sig").py_str());
    let Some(msg) = doc_signed(tid, dev, seq, &ct) else { return Ok(Err("服务器给的配置样子不对".into())) };
    let by = match env.get("by") {
        None | Some(V::None) => None,
        Some(b) => match b.as_str().and_then(root_line) {
            Some(line) if root_line(root).as_deref() == Some(line.as_str()) => None,
            Some(line) => Some(line),
            None => return Ok(Err("服务器给的配置样子不对".into())),
        },
    };
    let signer = match &by {
        None => root.to_string(),
        Some(line) => match admins {
            Some(list) if list.has(line) => line.clone(),
            Some(_) => {
                return Ok(Err("配置不是这个 Team 现在的管理员签的（管理员身份被收回了？可能有人冒充服务器）：不用它".into()))
            }
            None => return Ok(Err("配置是另一位管理员签的，服务器却没给根密钥签过的管理员名单：不用它".into())),
        },
    };
    match doc_verify(&signer, &msg, &sig)? {
        Some(true) => {}
        Some(false) if by.is_none() => {
            return Ok(Err("配置的签名不是这个 Team 的根密钥签的（可能有人冒充服务器）：不用它".into()));
        }
        Some(false) => return Ok(Err("配置的签名不对（可能有人冒充服务器）：不用它".into())),
        None => return Ok(Err("验不了配置的签名（这台设备上的 ssh-keygen 用不了：要 OpenSSH 8.0 以上）：先不用它".into())),
    }
    let Some(doc) = doc_open(keys, tid, dev, seq, &ct) else {
        return Ok(Err("配置解不开（不是给这台设备的这一份）：不用它".into()));
    };
    if let Some(why) = doc_problems(&doc).into_iter().next() {
        return Ok(Err(format!("配置的内容不对（{why}）：不用它")));
    }
    // 别的管理员签的：照的名单不能比配置里说的旧（服务器拿旧的名单配一份被收回了的管理员签的配置）
    if by.is_some() && admins.is_some_and(|l| doc_admins_epoch(&doc) > l.epoch) {
        return Ok(Err("服务器给的管理员名单比配置说的旧（可能有人冒充服务器）：不用它".into()));
    }
    Ok(Ok(Opened { seq, doc, by }))
}

/// 配置里说的管理员名单的纪元（admins_epoch，3.6.15 起；没有是 0）
pub fn doc_admins_epoch(doc: &Dict) -> i64 {
    doc.get("admins_epoch").and_then(|e| e.as_int()).unwrap_or(0)
}

/// 签名的那一把：私钥文件，和不是根密钥时它的公钥那一行（放进配置的 by）
pub struct Signer {
    pub key: String,
    pub by: Option<String>,
}

/// 管理员给一台设备封好、签好的那一份（{"seq", "ct", "sig"}，别的管理员签的另有 "by"）；太大、签不了是给人看的原因
pub fn envelope_seal(signer: &Signer, keys: &Keys, tid: &str, dev: &str, seq: i64, doc: &Dict) -> R<Result<Dict, String>> {
    let ct = V::Dict(doc_seal(keys, tid, dev, seq, doc)?);
    if ct.as_dict().is_some_and(|c| c.at("c").py_str().len() > DOC_MAX) {
        return Ok(Err("配置太大了（节点太多？一台设备的配置加密后最多 128 KB）".into()));
    }
    let msg = doc_signed(tid, dev, seq, &ct).expect("刚封好的密文样子是对的");
    match doc_sign(&signer.key, &msg)? {
        Some(sig) => {
            let env = dict! { "seq" => seq, "ct" => ct, "sig" => sig };
            if let Some(by) = &signer.by {
                env.set("by", by.as_str());
            }
            Ok(Ok(env))
        }
        None if signer.by.is_none() => Ok(Err("没法用 Team 的根密钥签名（ssh-keygen）".into())),
        None => Ok(Err("没法用这台设备的密钥签名（ssh-keygen）".into())),
    }
}

// --- 管理员名单、几位管理员共用的状态（3.6.15 起） ------------------------------------------------------------------------

/// 管理员名单签名时前面的标签
const ADMINS_LABEL: &str = "cca-team-admins-v1";
/// 共用的状态的密文的标签
const ASTATE_LABEL: &str = "cca-team-astate-v1";

/// 验过根密钥签名的管理员名单
#[derive(Clone, Debug)]
pub struct AdminList {
    /// 名单原样（紧凑的 JSON 文字）和根密钥的签名
    pub list: String,
    pub sig: String,
    pub epoch: i64,
    /// (公钥那一行, 名字)
    pub admins: Vec<(String, String)>,
}

impl AdminList {
    pub fn has(&self, line: &str) -> bool {
        self.admins.iter().any(|(p, _)| p == line)
    }
}

/// 根密钥签的那一段：标签 \0 名单的文字
fn admins_signed(list: &str) -> Vec<u8> {
    let mut msg = format!("{ADMINS_LABEL}\0").into_bytes();
    msg.extend_from_slice(list.as_bytes());
    msg
}

/// 名单的文字：{"v": 1, "team", "epoch", "admins": [{"pub", "name"}…]}，紧凑的 JSON
pub fn admins_text(tid: &str, epoch: i64, admins: &[(String, String)]) -> String {
    let rows: Vec<V> = admins.iter().map(|(p, n)| V::Dict(dict! { "pub" => p.as_str(), "name" => n.as_str() })).collect();
    let doc = dict! { "v" => 1, "team" => tid, "epoch" => epoch, "admins" => rows };
    json::dumps(&V::Dict(doc), json::Fmt::new().utf8().compact())
}

/// 拿根密钥签一份名单：{"list", "sig"}；签不了是 None
pub fn admins_sign(root_key: &str, list: &str) -> R<Option<Dict>> {
    Ok(doc_sign(root_key, &admins_signed(list))?.map(|sig| dict! { "list" => list, "sig" => sig }))
}

/// 名单的文字里的纪元、各位管理员（不验签名：先看要不要验）；样子不对是 None
fn admins_parse(tid: &str, list: &str) -> Option<(i64, Vec<(String, String)>)> {
    let d = json::loads(list).ok()?.as_dict()?;
    let epoch = d.get("epoch")?.as_int().filter(|e| (1..SEQ_MAX).contains(e))?;
    if !d.get("v").is_some_and(|v| v.py_eq(&V::Int(1))) || d.str("team").as_deref() != Some(tid) {
        return None;
    }
    let mut out = Vec::new();
    for a in d.list("admins")?.items() {
        let a = a.as_dict()?;
        out.push((root_line(&a.str("pub")?)?, a.str("name").filter(|n| team_name_ok(n))?.to_string()));
    }
    Some((epoch, out))
}

/// 服务器给的 {"list", "sig"}：纪元比 at_least 小的不要（None，不验）；别的照根公钥验过签名。Err 是给人看的原因
pub fn admins_open(root: &str, tid: &str, v: &Dict, at_least: i64) -> R<Result<Option<AdminList>, String>> {
    let (list, sig) = (v.at("list").py_str(), v.at("sig").py_str());
    let Some((epoch, admins)) = admins_parse(tid, &list) else {
        return Ok(Err("服务器给的管理员名单样子不对".into()));
    };
    if epoch < at_least {
        return Ok(Ok(None));
    }
    match doc_verify(root, &admins_signed(&list), &sig)? {
        Some(true) => Ok(Ok(Some(AdminList { list, sig, epoch, admins }))),
        Some(false) => Ok(Err("管理员名单不是这个 Team 的根密钥签的（可能有人冒充服务器）".into())),
        None => Ok(Err("验不了管理员名单的签名（这台设备上的 ssh-keygen 用不了：要 OpenSSH 8.0 以上）".into())),
    }
}

/// 共用的状态的两把密钥：由管理员密钥（64 个十六进制字符）派生，root = HMAC(它解出来的 32 个字节, "cca-team-astate-v1")
pub fn astate_keys(akey: &str) -> Option<Keys> {
    let k = from_hex(akey).filter(|k| k.len() == 32)?;
    let root = hmac_sha256(&k, ASTATE_LABEL.as_bytes());
    Some((hmac_sha256(&root, b"enc"), hmac_sha256(&root, b"mac")))
}

fn astate_ad(tid: &str, rev: i64) -> R<Vec<u8>> {
    blob_ad(&[ASTATE_LABEL, tid, &rev.to_string()])
}

/// 共用的状态 → 服务器存的密文：明文是紧凑的 JSON，空格补齐到 1024 字节的整数倍；认证带着 Team、版本号
pub fn astate_seal(keys: &Keys, tid: &str, rev: i64, doc: &Dict) -> R<Dict> {
    let mut pt = json::dumps(&V::Dict(doc.copy()), json::Fmt::new().utf8().compact()).into_bytes();
    pt.resize(pt.len().div_ceil(DOC_PAD) * DOC_PAD, b' ');
    Ok(blob_seal(keys, &astate_ad(tid, rev)?, &pt))
}

/// astate_seal 反过来；认证不过、不是 JSON 对象是 None
pub fn astate_open(keys: &Keys, tid: &str, rev: i64, ct: &V) -> Option<Dict> {
    let pt = blob_open(keys, &astate_ad(tid, rev).ok()?, ct)?;
    json::loads(std::str::from_utf8(&pt).ok()?.trim_end_matches(' ')).ok()?.as_dict()
}

/// 配置的明文哪里不对（管理员发布前、设备用之前都查；不认得的项不管：以后加的旧版本照样用）：
/// {"v": 1, "at", "name", "salt", "server": {"u", "p"}, "accounts": {工具: {邮箱: 节点}}, "aliases": {邮箱: 别名},
/// "tool_aliases": {工具: {邮箱: 别名}}（3.5.3-beta.6 起；设备有它时照它，aliases 给旧版本用）, "alias_lock": true（3.5.3-beta.7 起，
/// 管理员锁定了别名：设备上不能自己改，改过的换回来；没锁定时没有这一项）, "install_node", "nodes": {"group", "proxies"} | null,
/// "mail_forwarders": [域名…]（Team 给的 Claude 账号信任的转发中转：设了、这台设备又拿到了 Claude 账号才有这一项；以前的版本不认它）}
pub fn doc_problems(doc: &Dict) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if !doc.get("v").is_some_and(|v| v.py_eq(&V::Int(1))) {
        out.push("v 不是 1".into());
    }
    if !doc.str("name").is_some_and(|n| team_name_ok(&n)) {
        out.push("name 不对".into());
    }
    if !doc.str("salt").is_some_and(|s| PRESENCE_SALT_RE.fullmatch(&s).is_some()) {
        out.push("salt 不对".into());
    }
    let server_ok = doc.dict("server").is_some_and(|s| {
        let pins = s.list("p").map(|p| p.items()).unwrap_or_default();
        s.str("u").is_some_and(|u| url_ok(&u))
            && pins.len() <= 4
            && pins.iter().all(|p| p.as_str().is_some_and(|p| PRESENCE_PIN_RE.fullmatch(p).is_some()))
    });
    if !server_ok {
        out.push("server 不对".into());
    }
    let email_ok = |e: &str| e.contains('@') && e == e.to_lowercase() && e.trim() == e && !e.chars().any(|c| c.is_control());
    match doc.dict("accounts") {
        None => out.push("accounts 不对".into()),
        Some(accts) => {
            for (tool, m) in accts.items() {
                let Some(m) = m.as_dict().filter(|_| tools().iter().any(|t| t.name == &*tool)) else {
                    out.push(format!("accounts 里的 {tool} 不对"));
                    continue;
                };
                for (e, n) in m.items() {
                    if !email_ok(&e) || !n.as_str().is_some_and(|n| !n.is_empty() && n.trim() == n) {
                        out.push(format!("accounts.{tool} 里的 {e} 不对"));
                    }
                }
            }
        }
    }
    let names_ok = |a: &V| a.as_dict().is_some_and(|a| a.items().iter().all(|(e, v)| email_ok(e) && alias_ok(v)));
    if doc.get("aliases").is_some_and(|a| !names_ok(&a)) {
        out.push("aliases 不对".into());
    }
    // 不认得的工具不管（以后加的工具：accounts 里那一节旧版本本来就不认）
    if doc.get("tool_aliases").is_some_and(|a| !a.as_dict().is_some_and(|a| a.values().iter().all(names_ok))) {
        out.push("tool_aliases 不对".into());
    }
    if doc.get("alias_lock").is_some_and(|l| !matches!(l, V::Bool(_))) {
        out.push("alias_lock 不对".into());
    }
    if doc.get("install_node").is_some_and(|n| !n.is_str()) {
        out.push("install_node 不对".into());
    }
    // 信任的转发中转：只查样子（最多 64 个字符串）。内容设备上读的时候一项项过（mail::doc_forwarders：公共邮箱的域名这些跳过）：以后
    // 那份名单变长了，也不会让整份配置作废
    let fwd_ok =
        |l: &V| l.as_list().is_some_and(|l| l.len() <= 64 && l.items().iter().all(|x| x.as_str().is_some_and(|s| s.len() <= 253)));
    if doc.get("mail_forwarders").is_some_and(|l| !fwd_ok(&l)) {
        out.push("mail_forwarders 不对".into());
    }
    if doc.get("admins_epoch").is_some_and(|e| !e.as_int().is_some_and(|e| (0..SEQ_MAX).contains(&e))) {
        out.push("admins_epoch 不对".into());
    }
    if doc.get("admin").is_some_and(|a| !a.as_dict().is_some_and(|a| a.str("key").is_some_and(|k| astate_keys(&k).is_some()))) {
        out.push("admin 不对".into());
    }
    if let Some(n) = doc.get("nodes").filter(|n| !n.is_none()) {
        let shaped = n.as_dict().map(|n| V::Dict(dict! { "version" => 1, "group" => n.at("group"), "proxies" => n.at("proxies") }));
        match shaped {
            None => out.push("nodes 不对".into()),
            Some(s) => out.extend(fill_problems(&s).into_iter().map(|p| format!("nodes：{p}"))),
        }
    }
    out
}

/// 配置的内容的摘要（管理员据此看要不要重新发布：发布的时间 at 不算）
pub fn doc_digest(doc: &Dict) -> String {
    let d = doc.copy();
    d.pop("at");
    to_hex(&sha256(json::dumps(&V::Dict(d), json::Fmt::new().utf8().compact())))
}

#[cfg(test)]
mod tests {
    use super::*;

    // 对照数据是照文档独立实现的那一份算的，固定的（team_id、invite_parts、invite_encode、team_doc_seal 固定 nonce 0..15、team_doc_signed）
    const ROOT: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPUL7hSEVHXfYDWvDx/ozOeLVzcLMWYwN8Ru2CqCdG8d";
    const TID: &str = "45391bb0e9129a8a817e3e03219a08a8";
    const SECRET: &str = "abababababababababababababababab";
    const PROOF: &str = "B1rh0sV/O5Lm9pwKjFLyudL8kwNASfoJY+rd9ZBjYvY=";
    const LINK: &str = "cca-team1.eyJ1IjoiaHR0cHM6Ly8xOTIuMC4yLjEwOjE4NDQ1IiwicCI6WyJzaGEyNTYvL0FBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE9Il0sInIiOiJBQUFBQzNOemFDMWxaREkxTlRFNUFBQUFJUFVMN2hTRVZIWGZZRFd2RHgvb3pPZUxWemNMTVdZd044UnUyQ3FDZEc4ZCIsImsiOiJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYiIsIm4iOiLmtYvor5Xnu4QiLCJlIjoxODAwMDAwMDAwfQ.169a";
    // 同上，加上 d（invite_encode 的 device="小王的 MacBook"）
    const LINK_D: &str = "cca-team1.eyJ1IjoiaHR0cHM6Ly8xOTIuMC4yLjEwOjE4NDQ1IiwicCI6WyJzaGEyNTYvL0FBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE9Il0sInIiOiJBQUFBQzNOemFDMWxaREkxTlRFNUFBQUFJUFVMN2hTRVZIWGZZRFd2RHgvb3pPZUxWemNMTVdZd044UnUyQ3FDZEc4ZCIsImsiOiJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYiIsIm4iOiLmtYvor5Xnu4QiLCJlIjoxODAwMDAwMDAwLCJkIjoi5bCP546L55qEIE1hY0Jvb2sifQ.9710";
    const CT: &str = include_str!("../../../tests/vectors/team_doc_ct.json");

    fn invite() -> Invite {
        Invite {
            url: "https://192.0.2.10:18445".into(),
            pins: vec![format!("sha256//{}=", "A".repeat(43))],
            root: ROOT.into(),
            secret: SECRET.into(),
            name: "测试组".into(),
            exp: Some(1_800_000_000),
            device: String::new(),
        }
    }

    #[test]
    fn team_id_and_root_line() {
        assert_eq!(team_id(ROOT).as_deref(), Some(TID));
        assert_eq!(team_id(&format!("{ROOT}  备注 在后面")).as_deref(), Some(TID));
        assert_eq!(team_id("ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ"), None);
        assert_eq!(team_id("ssh-ed25519 AAAA"), None);
        assert_eq!(root_line("nope"), None);
    }

    #[test]
    fn invite_link_matches_the_spec() {
        assert_eq!(invite_encode(&invite()), LINK);
        assert_eq!(invite_decode(LINK), Ok(invite()));
        let folded = format!("  {}\n {}  ", &LINK[..40], &LINK[40..]);
        assert_eq!(invite_decode(&folded), Ok(invite()), "中间的空白不算");
        let no_exp = Invite { exp: None, pins: vec![], ..invite() };
        assert_eq!(invite_decode(&invite_encode(&no_exp)), Ok(no_exp));
        // 带着设备的名字（d）：没有时不写这一项，上面的链接照旧；旧版本只是不认得它
        let named = Invite { device: "小王的 MacBook".into(), ..invite() };
        assert_eq!(invite_encode(&named), LINK_D);
        assert_eq!(invite_decode(LINK_D), Ok(named));
        for bad in ["", "cca-team1.", "cca-team1.abc", &LINK[..LINK.len() - 1], &LINK.replacen("eyJ1", "eyJ2", 1)] {
            assert!(invite_decode(bad).is_err(), "{bad}");
        }
        assert!(invite_decode("cca-team2.abc.0000").unwrap_err().contains("更新"));
        assert!(invite_decode("cca-hub1.abc.0000").unwrap_err().contains("向 Team 的管理员要邀请链接"));
        for broken in [
            Invite { secret: "AB".repeat(16), ..invite() },
            Invite { url: "http://192.0.2.10:1".into(), ..invite() },
            Invite { root: "ssh-ed25519 AAAA".into(), ..invite() },
            Invite { name: " x".into(), ..invite() },
        ] {
            assert!(invite_decode(&invite_encode(&broken)).is_err(), "{broken:?}");
        }
    }

    #[test]
    fn parts_match_the_spec() {
        let p = invite_parts(SECRET).expect("合格的随机数");
        assert_eq!((p.token.as_str(), p.dev.as_str()), ("a09e1146d2295e068a3aaa064cb6859e", "8e4122911eba7cc8"));
        assert_eq!(to_hex(&p.keys.0), "a4497ef694250db52b8fee5e62574680df5e0b67daaeb33cb51328ad0220db3c");
        assert_eq!(to_hex(&p.keys.1), "ffa8807e6363e23747849e1972c94307fc2786a04860d0b3f3a87a9ceec7ca59");
        assert!(invite_parts(&"AB".repeat(16)).is_none(), "只认小写");
        assert!(invite_parts("abab").is_none());
        // 独立实现的那一份的 team_pub_proof 算的
        assert_eq!(pub_proof(SECRET, &format!("{ROOT} 备注")).as_deref(), Some(PROOF));
        assert!(pub_proof(SECRET, "ssh-rsa AAAA").is_none());
    }

    #[test]
    fn doc_opens_what_the_spec_sealed_and_signs_the_same_bytes() {
        let p = invite_parts(SECRET).expect("合格的随机数");
        let ct = json::loads(CT).expect("对照数据是 JSON");
        let doc = doc_open(&p.keys, TID, &p.dev, 7, &ct).expect("认证得过");
        assert_eq!((doc.at("name").py_str(), doc.at("at").as_int()), ("测试组".to_string(), Some(1_790_000_000)));
        assert_eq!(
            doc.dict("accounts").and_then(|a| a.dict("claude")).map(|c| c.at("alice@example.com").py_str()).as_deref(),
            Some("US-A")
        );
        assert!(doc_open(&p.keys, TID, &p.dev, 8, &ct).is_none(), "换个序号认证不过");
        assert!(doc_open(&p.keys, TID, "0000000000000000", 7, &ct).is_none(), "挪到别的设备上认证不过");
        assert!(doc_open(&p.keys, &"0".repeat(32), &p.dev, 7, &ct).is_none(), "挪到别的 Team 上认证不过");
        let msg = doc_signed(TID, &p.dev, 7, &ct).expect("样子对");
        assert_eq!(sha256_hex(&msg), "ff275f4352b321c66899ffcb279ff79915ed0f96bf50a090ee0c8560013a54da");
        let short = dict! { "n" => "AAAA", "c" => ct.as_dict().expect("对象").at("c"), "t" => ct.as_dict().expect("对象").at("t") };
        assert!(doc_signed(TID, &p.dev, 7, &V::Dict(short)).is_none(), "nonce 的长度不对");
    }

    #[test]
    fn seal_round_trips_and_pads() {
        let p = invite_parts(SECRET).expect("合格的随机数");
        let doc = dict! { "v" => 1, "name" => "组" };
        let ct = doc_seal(&p.keys, TID, &p.dev, 1, &doc).expect("封得上");
        assert_eq!(b64decode(&ct.at("c").py_str(), true).expect("base64").len(), DOC_PAD);
        let back = doc_open(&p.keys, TID, &p.dev, 1, &V::Dict(ct)).expect("解得开");
        assert_eq!(back.at("name").py_str(), "组");
    }

    fn good_doc() -> Dict {
        dict! {
            "v" => 1, "at" => 1, "name" => "组", "salt" => "ab".repeat(16),
            "server" => dict! { "u" => "https://192.0.2.10:18445", "p" => crate::list![] },
            "accounts" => dict! { "claude" => dict! { "alice@example.com" => "US-A" } },
            "aliases" => dict! { "alice@example.com" => "alice" }, "install_node" => "US-A", "nodes" => V::None,
        }
    }

    #[test]
    fn doc_shape() {
        assert_eq!(doc_problems(&good_doc()), Vec::<String>::new());
        let with = |k: &str, v: V| {
            let d = good_doc();
            d.set(k, v);
            doc_problems(&d).len()
        };
        assert_eq!(with("v", V::Int(2)), 1);
        assert_eq!(with("salt", V::from("xyz")), 1);
        assert_eq!(with("server", V::Dict(dict! { "u" => "http://192.0.2.10:1" })), 1);
        assert_eq!(with("accounts", V::Dict(dict! { "nope" => dict! {} })), 1);
        assert_eq!(with("accounts", V::Dict(dict! { "claude" => dict! { "Alice@Example.com" => "US-A" } })), 1);
        assert_eq!(with("accounts", V::Dict(dict! { "codex" => dict! { "bob@example.com" => "" } })), 1);
        assert_eq!(with("aliases", V::Dict(dict! { "alice@example.com" => "a b" })), 1);
        let by_tool = |m: Dict| V::Dict(dict! { "claude" => dict! { "alice@example.com" => "alice" }, "codex" => m });
        assert_eq!(
            with("tool_aliases", by_tool(dict! { "alice@example.com" => "team-1" })),
            0,
            "同一个邮箱在两个工具里可以叫得不一样"
        );
        assert_eq!(with("tool_aliases", by_tool(dict! { "Alice@Example.com" => "team-1" })), 1);
        assert_eq!(with("tool_aliases", by_tool(dict! { "alice@example.com" => "a b" })), 1);
        assert_eq!(with("tool_aliases", V::Dict(dict! { "later" => dict! { "alice@example.com" => "x" } })), 0, "不认得的工具不管");
        assert_eq!(with("tool_aliases", V::Dict(dict! { "claude" => "x" })), 1);
        assert_eq!(with("tool_aliases", V::from("x")), 1);
        assert_eq!((with("alias_lock", V::Bool(true)), with("alias_lock", V::Bool(false))), (0, 0));
        assert_eq!((with("alias_lock", V::from("yes")), with("alias_lock", V::Int(1))), (1, 1));
        assert_eq!(with("nodes", V::from("x")), 1);
        let fwds = |v: Vec<V>| V::from(v);
        assert_eq!(with("mail_forwarders", fwds(vec![V::from("relay.example.com")])), 0);
        assert_eq!(with("mail_forwarders", fwds(vec![V::from("gmail.com")])), 0, "内容读的时候才查（跳过），整份配置照样用");
        assert_eq!((with("mail_forwarders", V::from("relay.example.com")), with("mail_forwarders", fwds(vec![V::Int(1)]))), (1, 1));
        assert_eq!(with("mail_forwarders", fwds((0..65).map(|i| V::from(format!("r{i}.example.com"))).collect())), 1);
        assert_eq!(with("later", V::from("旧版本不认得的项不管")), 0);
        let d = good_doc();
        let before = doc_digest(&d);
        d.set("at", 2);
        assert_eq!(doc_digest(&d), before, "发布的时间不算进摘要");
        d.set("name", "别的");
        assert_ne!(doc_digest(&d), before);
    }
}
