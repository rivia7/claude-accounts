//! 共享记下的 API key（`keyshare_on` 到 `cmd_key_sync`）：key-sync。
//!
//! 子命令还没移植。记下、删掉 key 时要用的这几样已经移植（secrets 模块调）：`keyshare_on`、`keyshare_edit`（keys.json 的
//! 读改写）、`keyshare_kick`。

use super::presence::{presence_load, presence_mapped, spawn_background, PRESENCE_DIR};
use super::Cmd;
use crate::core::model::{acct_id, is_api};
use crate::core::policy::policy_raw;
use crate::core::tool::{t, Kind};
use crate::dict;
use crate::py::base64::{b64decode, b64encode};
use crate::py::fs::{load_quiet, save_json};
use crate::py::hash::{compare_digest, hmac_sha256, sha256_hex};
use crate::py::json;
use crate::py::rand::urandom;
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::time::time;
use crate::py::R;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::io::AsRawFd;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[];

/// PRESENCE_DIR 里：{"v": 1, "acc": {工具:账号目录名: {"at": 时间, "sum": key 的摘要 | null}},
/// "synced": 上次同步, "retry": 旧的服务器什么时候再试, "error": 没同步成的原因}
pub const KEYSHARE_FILE: &str = "keys.json";
/// 后台最多隔这么久同步一次（在这台设备上记下、删掉时马上）
pub const KEYSHARE_EVERY: f64 = 600.0;

/// `isinstance(x, (int, float)) and not isinstance(x, bool)`
pub fn is_num(v: &V) -> bool {
    matches!(v, V::Int(_) | V::Big(_) | V::Float(_))
}

/// 这个账号（当前工具的）记下备查的 key 会不会同步：映射里的 Claude、Codex 官方账号，这台设备开着设备统计
pub fn keyshare_on(key: &str, v: &V) -> R<bool> {
    Ok(matches!(t().kind, Kind::Claude | Kind::Codex) && !is_api(v) && presence_mapped(&policy_raw(t()), key, v) && presence_load()?.1.is_some())
}

/// key 的摘要（没有 key、删掉了是 None）
pub fn keyshare_sum(s: Option<&str>) -> V {
    match s.filter(|s| !s.is_empty()) {
        Some(s) => V::from(&sha256_hex(s)[..16]),
        None => V::None,
    }
}

/// keys.json 里这个账号（当前工具的）的位置：按这台设备上的账号目录名，和盐无关（换了盐也认得出谁新）
pub fn keyshare_slot(key: &str) -> String {
    format!("{}:{}", t().name, acct_id(key, t()))
}

/// 建 PRESENCE_DIR，但不建 ~/.cca 本身：cca restore 刚删掉它时，后台还没跑完的同步不该把它建回来（没有时 OSError）
pub fn presence_dir() -> std::io::Result<()> {
    if !path::isdir(&PRESENCE_DIR) {
        match std::fs::DirBuilder::new().mode(0o700).create(&*PRESENCE_DIR) {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// keys.json 的读改写（keys.lock：在这台设备上记下、删掉 key 和同步时照服务器上的改 key 互斥，拿着的时间很短）。
/// `enter` 拿锁、读出来；改完 `exit(true)` 写回去（出了错的 `exit(false)` 不写）；锁在这个值没了的时候放开
pub struct KeyshareFile {
    _lock: std::fs::File,
    pub j: Dict,
}

impl KeyshareFile {
    pub fn enter() -> R<KeyshareFile> {
        presence_dir()?;
        let lock = std::fs::OpenOptions::new().append(true).create(true).open(path::join(&PRESENCE_DIR, "keys.lock"))?;
        // SAFETY: lock 开着，fd 有效
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let j = load_quiet(&path::join(&PRESENCE_DIR, KEYSHARE_FILE)).and_then(|j| j.as_dict());
        let j = j.filter(|j| j.get("v").is_some_and(|v| v.py_eq(&V::Int(1))) && j.dict("acc").is_some());
        Ok(KeyshareFile { _lock: lock, j: j.unwrap_or_else(|| dict! { "v" => 1, "acc" => Dict::new() }) })
    }

    pub fn exit(self, ok: bool) -> R<()> {
        if ok {
            save_json(&path::join(&PRESENCE_DIR, KEYSHARE_FILE), &V::Dict(self.j.clone()), Some(0o600), true)?;
        }
        Ok(())
    }
}

/// `with keyshare_edit(键, 时间) as ed`：在这台设备上记下（换掉）、删掉这个账号（当前工具的）记下备查的 key，改完
/// `ed.mark(key 或 None)` 记下时间（同步时按它比新旧）。拿着 keys.lock：同步不会在这中间拿服务器上旧的盖掉刚记下的。
/// 没开设备统计时不记（以后开了，记下的按 key_hints 里的时间算，删掉的不同步）；出错不影响记下、删掉本身
pub struct KeyshareEdit {
    key: String,
    at: f64,
    f: Option<KeyshareFile>,
}

/// `keyshare_edit(key, at).__enter__()`
pub fn keyshare_edit(key: &str, at: f64) -> KeyshareEdit {
    let open = || -> R<Option<KeyshareFile>> {
        if presence_load()?.1.is_some() {
            return Ok(Some(KeyshareFile::enter()?));
        }
        Ok(None)
    };
    let f = match open() {
        Ok(f) => f,
        Err(e) if e.is_os_or_value() => None,
        Err(e) => panic!("keyshare_edit：{e}"),
    };
    KeyshareEdit { key: key.to_string(), at, f }
}

impl KeyshareEdit {
    /// `ed.mark(s)`
    pub fn mark(&mut self, s: Option<&str>) {
        if let Some(f) = &self.f {
            f.j.dict("acc").expect("enter 保证了 acc 是字典").set(keyshare_slot(&self.key), dict! { "at" => self.at, "sum" => keyshare_sum(s) });
        }
    }

    /// `__exit__`；ok：with 里面没出异常（出了就不写 keys.json）
    pub fn exit(self, ok: bool) {
        if let Some(f) = self.f {
            match f.exit(ok) {
                Err(e) if !e.is_os_or_value() => panic!("keyshare_edit：{e}"),
                _ => {}
            }
        }
    }
}

/// 在后台跑一次 cca _key-sync（同步记下备查的 API key）；force 以外还不到时候就不起进程。静默
pub fn keyshare_kick(force: bool) {
    let kick = || -> R<()> {
        if presence_load()?.1.is_none() {
            return Ok(());
        }
        let j = load_quiet(&path::join(&PRESENCE_DIR, KEYSHARE_FILE)).and_then(|j| j.as_dict()).unwrap_or_default();
        let last = |k: &str| j.get(k).filter(is_num).and_then(|v| v.as_num()).unwrap_or(0.0);
        if !force && (time() - last("synced") < KEYSHARE_EVERY || time() < last("retry")) {
            return Ok(());
        }
        if force {
            spawn_background(&["_key-sync", "force"]);
        } else {
            spawn_background(&["_key-sync"]);
        }
        Ok(())
    };
    match kick() {
        Err(e) if !e.is_os_or_value() => panic!("keyshare_kick：{e}"),
        _ => {}
    }
}

// --- 服务器存的密文（共享 API key、折合金额共用，格式在 server/README.md）：{"n": nonce, "c": 密文, "t": 认证}（base64）。
// HMAC-SHA256(加密密钥, nonce + 块号) 按块（32 字节）当密钥流异或加密，再用另一把密钥认证 ad + nonce + 密文（先加密后认证；
// ad 是标签、账号摘要这些，挪不到别的用途、别的账号上）。明文由调用的一方补齐。

/// 密钥、认证的标签（折合金额的是 COST_LABEL）
pub const KEYSHARE_LABEL: &str = "cca-keyshare-v1";
/// 明文补齐到这么长的整数倍：从密文的长短看不出是 key 还是删除、key 有多长
pub const KEYSHARE_PAD: usize = 256;

/// (加密, 认证) 两把密钥
pub type Keys = ([u8; 32], [u8; 32]);

/// `bytes.fromhex(s)`：字节之间的空白不算；不是十六进制是 ValueError
fn bytes_fromhex(s: &str) -> R<Vec<u8>> {
    let compact: String = s.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    crate::py::hash::from_hex(&compact).ok_or_else(|| crate::py::Fail::Value("non-hexadecimal number found in fromhex() arg".into()))
}

/// 这个账号的两把密钥（加密、认证）：盐、工具、邮箱、登录身份一起派生；label 分开用途（折合金额的是 COST_LABEL）
pub fn keyshare_keys(salt: &str, tool: &str, email: &str, ident: &str, label: &str) -> R<Keys> {
    let root = hmac_sha256(&bytes_fromhex(salt)?, format!("{}\0{}\0{}\0{}", label, tool, email.to_lowercase(), ident).as_bytes());
    Ok((hmac_sha256(&root, b"enc"), hmac_sha256(&root, b"mac")))
}

/// 认证里带着的：每一项后面一个 \0（都得是 ASCII，不是的话原来是没人接的 UnicodeEncodeError）
pub fn blob_ad(parts: &[&str]) -> R<Vec<u8>> {
    let joined: String = parts.iter().map(|x| format!("{x}\0")).collect();
    if !joined.is_ascii() {
        return Err(crate::py::Fail::Value("'ascii' codec can't encode characters: ordinal not in range(128)".into()));
    }
    Ok(joined.into_bytes())
}

pub fn blob_xor(enc: &[u8], nonce: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for (i, chunk) in data.chunks(32).enumerate() {
        let mut block = nonce.to_vec();
        block.extend_from_slice(&(i as u64).to_be_bytes());
        let stream = hmac_sha256(enc, &block);
        out.extend(chunk.iter().zip(stream.iter()).map(|(a, b)| a ^ b));
    }
    out
}

pub fn blob_tag(mac: &[u8], ad: &[u8], nonce: &[u8], ct: &[u8]) -> [u8; 32] {
    hmac_sha256(mac, &[ad, nonce, ct].concat())
}

fn seal_with(keys: &Keys, ad: &[u8], pt: &[u8], nonce: &[u8]) -> Dict {
    let ct = blob_xor(&keys.0, nonce, pt);
    dict! { "n" => b64encode(nonce), "c" => b64encode(&ct), "t" => b64encode(&blob_tag(&keys.1, ad, nonce, &ct)) }
}

/// 补齐好的明文 → {"n", "c", "t"}；keys 是 (加密, 认证) 两把密钥
pub fn blob_seal(keys: &Keys, ad: &[u8], pt: &[u8]) -> Dict {
    seal_with(keys, ad, pt, &urandom(16))
}

/// blob_seal 反过来：明文（带着补齐）；格式不对、认证不过（伪造的、别的账号的、坏了）是 None
pub fn blob_open(keys: &Keys, ad: &[u8], blob: &V) -> Option<Vec<u8>> {
    let blob = blob.as_dict()?;
    let part = |k: &str| b64decode(blob.get(k)?.as_str()?, true).ok();
    let (nonce, ct, tag) = (part("n")?, part("c")?, part("t")?);
    if nonce.len() != 16 || !compare_digest(&tag, &blob_tag(&keys.1, ad, &nonce, &ct)) {
        return None;
    }
    Some(blob_xor(&keys.0, &nonce, &ct))
}

/// key（删掉时是 None）和时间 → 服务器存的那一份（blob_seal；明文用空格补齐到 KEYSHARE_PAD 的整数倍）
pub fn keyshare_seal(keys: &Keys, aid: &str, s: Option<&str>, at: &V) -> R<Dict> {
    Ok(blob_seal(keys, &blob_ad(&[KEYSHARE_LABEL, aid])?, &keyshare_plain(s, at)))
}

fn keyshare_plain(s: Option<&str>, at: &V) -> Vec<u8> {
    let doc = dict! { "v" => 1, "key" => s.map_or(V::None, V::from), "at" => at };
    let mut pt = json::dumps(&V::Dict(doc), json::Fmt::new().compact()).into_bytes();
    pt.resize(pt.len().div_ceil(KEYSHARE_PAD) * KEYSHARE_PAD, b' ');
    pt
}

/// 服务器上的那一份解开：{"key": key | None, "at": 时间}；认证不过、格式不对（伪造的、别的账号的、坏了）是 None
pub fn keyshare_open(keys: &Keys, aid: &str, blob: &V) -> R<Option<Dict>> {
    let Some(pt) = blob_open(keys, &blob_ad(&[KEYSHARE_LABEL, aid])?, blob) else { return Ok(None) };
    let Some(j) = std::str::from_utf8(&pt).ok().and_then(|text| json::loads(text).ok()).and_then(|j| j.as_dict()) else { return Ok(None) };
    let at = j.get("at").filter(is_num).and_then(|at| at.as_num());
    let key_ok = j.get("key").is_none_or(|k| k.is_none() || crate::core::model::secret_ok(&k));
    if !(j.get("v").is_some_and(|v| v.py_eq(&V::Int(1))) && at.is_some_and(|at| at > 0.0) && key_ok) {
        return Ok(None);
    }
    Ok(Some(dict! { "key" => j.at("key"), "at" => at.expect("上面核对过") }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::py::hash::{sha256_hex, to_hex};

    /// 期望值是把原来的 Python 函数抄出去、用 CPython 3.14 算的
    #[test]
    fn keys_and_blobs_match_python() {
        let salt = "ab".repeat(16);
        let keys = keyshare_keys(&salt, "claude", "Alice@Example.com", "user-uuid-1", KEYSHARE_LABEL).unwrap();
        assert_eq!(to_hex(&keys.0), "e069f911df4152daad3f9a6507058688278a09a21ccc190fc70bce03f1416050");
        assert_eq!(to_hex(&keys.1), "ff5c832f0147ecbf10eb5cdb9bd9c72a844ca4776bf60fd44ebc03dcbdaefed6");
        let cost = keyshare_keys(&format!("{salt}\n"), "codex", "erin@example.com", "acct", "cca-cost-v1").unwrap();
        assert_eq!(to_hex(&cost.0), "c2ae742d849d173c0e5c9d82ef6c457fe409ea7d764aa705fc66c283497a6a34");
        assert!(keyshare_keys("zz", "claude", "a@example.com", "x", KEYSHARE_LABEL).is_err());
        assert_eq!(blob_ad(&[KEYSHARE_LABEL, "abc123"]).unwrap(), b"cca-keyshare-v1\0abc123\0");
        assert!(blob_ad(&["中"]).is_err());
        let nonce: Vec<u8> = (0..16).collect();
        let ad = blob_ad(&[KEYSHARE_LABEL, "aid-1"]).unwrap();
        let long = "k".repeat(300);
        for (s, at, len, c_sha, t) in [
            (Some("sk-ant-api03-REFtest0123456789abcdWXYZ"), V::Float(1794024000.5), 256, "a6bee9f921d43b9308e981b6bce9f884e24cf8a340220d213ad0977c586c97b6", "VS5977i4HFuFYk+4BKwHQHEI4J9jECiVXRncLUscfx0="),
            (None, V::Int(1794024000), 256, "91c0321fe754e28dd380e794737d910f5cd2c9051502fc96b9ceadd714a307d6", "VWt1VFY3ERfSKhyDnCNmIumImPzca1BYm8WHyTBkZZE="),
            (Some(long.as_str()), V::Float(1.25), 512, "42256c6a402f01a38694a90d38f1b25ae7801189a0a8563cdd75fa664084a201", "UvvAP6nztA+iqCq98rhItM7UoYFGlvHnaGH3lQs06i4="),
        ] {
            let pt = keyshare_plain(s, &at);
            assert_eq!(pt.len(), len);
            let blob = seal_with(&keys, &ad, &pt, &nonce);
            assert_eq!(sha256_hex(b64decode(&blob.at("c").py_str(), true).unwrap()), c_sha);
            assert_eq!(blob.at("t").py_str(), t);
            assert_eq!(blob_open(&keys, &ad, &V::Dict(blob.clone())), Some(pt));
            let opened = keyshare_open(&keys, "aid-1", &V::Dict(blob.clone())).unwrap().unwrap();
            assert_eq!(opened.at("key").as_str(), s);
            // 认证不过的：换了账号、改了密文、换了密钥
            assert!(keyshare_open(&keys, "aid-2", &V::Dict(blob.clone())).unwrap().is_none());
            let tampered = blob.copy();
            tampered.set("c", b64encode(&[0u8; 256]));
            assert!(blob_open(&keys, &ad, &V::Dict(tampered)).is_none());
            assert!(blob_open(&cost, &ad, &V::Dict(blob)).is_none());
        }
        assert_eq!(to_hex(&blob_xor(&keys.0, &nonce, &[b'x'; 33])), "7276d4c21e054103513f4f20e5c7496535e15e7980d0b916abc7f6c0f8057b2f48");
        assert!(blob_xor(&keys.0, &nonce, b"").is_empty());
        for bad in [V::None, V::from("text"), V::from(5), V::Dict(dict! { "n" => "AA==", "c" => "AA==" }), V::Dict(dict! { "n" => 5, "c" => "", "t" => "" }), V::Dict(dict! { "n" => "!!", "c" => "", "t" => "" })] {
            assert!(blob_open(&keys, &ad, &bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn presence_aid_matches_python() {
        let aid = crate::cmd::presence::presence_aid(&"ab".repeat(16), "claude", "Alice@Example.com").unwrap();
        assert_eq!(aid, "88f5ce63b50ec830552331fbc5afe3e5");
    }

    #[test]
    fn seal_then_open_round_trips_with_a_random_nonce() {
        let keys = keyshare_keys(&"cd".repeat(16), "codex", "erin@example.com", "acct-1", KEYSHARE_LABEL).unwrap();
        let (a, b) = (keyshare_seal(&keys, "aid", Some("sk-test-AAAA1111"), &V::Float(5.5)).unwrap(), keyshare_seal(&keys, "aid", Some("sk-test-AAAA1111"), &V::Float(5.5)).unwrap());
        assert_ne!(a.at("n").py_str(), b.at("n").py_str(), "每次的 nonce 都不一样");
        let opened = keyshare_open(&keys, "aid", &V::Dict(a)).unwrap().unwrap();
        assert_eq!((opened.at("key").py_str(), opened.at("at").as_num()), ("sk-test-AAAA1111".to_string(), Some(5.5)));
        let gone = keyshare_open(&keys, "aid", &V::Dict(keyshare_seal(&keys, "aid", None, &V::Int(7)).unwrap())).unwrap().unwrap();
        assert!(gone.at("key").is_none() && gone.at("at").as_num() == Some(7.0));
    }
}
