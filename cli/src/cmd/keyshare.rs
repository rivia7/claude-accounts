//! 共享记下的 API key（`keyshare_on` 到 `cmd_key_sync`）：key-sync。
//!
//! 记下、删掉 key 时用的（secrets 模块调）：`keyshare_on`、`keyshare_edit`（keys.json 的读改写）、`keyshare_kick`；
//! 密文的格式（`blob_*`，折合金额也用）；同步本身（`keyshare_sync`、`keyshare_round`：cca _key-sync 和上报进程顺带做）。

use super::presence::{presence_aid, presence_load, presence_mapped, presence_post, presence_targets, presence_why, spawn_background, PRESENCE_DIR};
use super::secrets::{kc_del, kc_hints, kc_put, kc_read, kc_svc, kc_unbook, keyfile_store, keystore_name, py_set, security, SECRETS_DIR};
use super::{arg, arity, Cmd};
use crate::core::base::CFG;
use crate::core::model::{acct_id, cdir, cfg_load, is_api, parse_key, REF_ACCT};
use crate::core::policy::policy_raw;
use crate::core::tool::{t, Kind, Tool, Using};
use crate::py::base64::{b64decode, b64encode};
use crate::py::fs::{load_quiet, save_json};
use crate::py::hash::{compare_digest, hmac_sha256, sha256_hex};
use crate::py::obj::{Dict, V};
use crate::py::rand::urandom;
use crate::py::time::{monotonic, time, timegm};
use crate::py::R;
use crate::py::{json, path, shutil, signal, text};
use crate::{dict, re};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::io::AsRawFd;
use std::rc::Rc;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[Cmd::all("key-sync", cmd_key_sync)];

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
        // str(UnicodeEncodeError)：第一段连着的非 ASCII 字符在哪（按字符数）
        let chars: Vec<char> = joined.chars().collect();
        let start = chars.iter().position(|c| !c.is_ascii()).expect("刚核对过有");
        let end = start + chars[start..].iter().take_while(|c| !c.is_ascii()).count();
        let what = if end - start == 1 {
            let c = chars[start] as u32;
            let esc = if c < 0x100 { format!("\\x{c:02x}") } else if c < 0x10000 { format!("\\u{c:04x}") } else { format!("\\U{c:08x}") };
            format!("character '{esc}' in position {start}")
        } else {
            format!("characters in position {}-{}", start, end - 1)
        };
        return Err(crate::py::Fail::Value(format!("'ascii' codec can't encode {what}: ordinal not in range(128)")));
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

// --- 同步（cca _key-sync、上报进程顺带）-----------------------------------------------------------------------------------
// 明文里有记下 / 删掉的时间，新的胜出（keyshare_round）：旧的、认证不过的（伪造、重放、坏了）不认；服务器上没有、认不出、
// 比这台设备的旧时，这台设备把自己的传上去（自己修好）。只有在这台设备上记下、删掉才算改了（keyshare_edit）：删账号
// （cca rm、cca edit）、cca restore 只删这台设备上的，不同步；钥匙串里的被删了，下次同步从服务器取回来。
// 什么时候同步：记下、删掉之后马上（后台）；cca usage、cca devices 之后，和上报进程开着时，最多 KEYSHARE_EVERY 一次。

/// 旧的服务器（404：还不收 key）：隔这么久再试
pub const KEYSHARE_RETRY: f64 = 6.0 * 3600.0;
pub const KEYSHARE_OLD: &str = "设备统计的服务器还不收 API key（维护者要更新服务器：./policy.sh presence deploy）";

/// 只有登录了这个账号的设备才知道的身份（共享 API key 的密钥由它派生，见 keyshare_keys；原来是各工具的 `share_ident`，
/// 调用时当前工具就是 tool）。Claude：accountUuid，每次现读，读不出（.claude.json 半写、手改坏了）当作没登录，只是这一轮
/// 不同步这个账号——不经 acct_json：它读不了时退出（会停掉上报进程），一个进程里又只读一次（上报进程一开好几天，看不到
/// 重新登录、退出登录）。Codex：id_token 的 chatgpt_user_id（人，不分工作空间）。Antigravity 没有只有登录了才知道的身份
/// （token 文件里只读邮箱和档位）：API key 不同步
pub fn share_ident(tool: &Tool, key: &str) -> Option<String> {
    let u = match tool.kind {
        Kind::Claude => load_quiet(&path::join(&cdir(key), ".claude.json"))?.as_dict()?.dict("oauthAccount")?.str("accountUuid")?,
        Kind::Codex => codex_claims_user(key)?,
        Kind::Agy => return None,
    };
    (text::len(&u) >= 8).then(|| u.to_string())
}

/// `codex_claims(key).get('user')`（是字符串才算）。codex_claims 在 usage+codex 那一批里移植，合进来以后换成它
fn codex_claims_user(_key: &str) -> Option<Rc<str>> {
    unimplemented!("codex_claims 还没合进来（usage+codex 那一批）：Codex 账号的 API key 同步先用不了")
}

/// `%Y%m%d%H%M%S`（14 位数字，UTC）→ 时间戳；不是一个真的时刻是 None（原来是 time.strptime 的 ValueError）
fn stamp14(s: &str) -> Option<f64> {
    if s.len() != 14 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n = |a: usize, b: usize| s[a..b].parse::<i64>().expect("都是数字");
    let (y, mo, d, h, mi, sec) = (n(0, 4), n(4, 6), n(6, 8), n(8, 10), n(10, 12), n(12, 14));
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    // 秒数 strptime 认到 61；年份 0 不是一个日期
    if y < 1 || !(1..=12).contains(&mo) || d < 1 || d > days[mo as usize - 1] || h > 23 || mi > 59 || sec > 61 {
        return None;
    }
    Some(timegm(y, mo, d, h, mi, sec) as f64)
}

/// 钥匙串里（密钥文件）这个账号记下备查的那一条最后改的时间；读不出来是 None
pub fn keystore_mtime(aid: &str) -> Option<f64> {
    if keyfile_store() {
        return shutil::getmtime(&path::join(&SECRETS_DIR, &kc_svc(aid, true))).ok();
    }
    // 不带 -w、-g：只有属性
    let (rc, out) = security(&["find-generic-password", "-a", REF_ACCT, "-s", &kc_svc(aid, true)], None);
    if rc != 0 {
        return None;
    }
    let out = String::from_utf8_lossy(&out);
    let m = re!(r"\x22mdat\x22<timedate>=[^\x22\n]*\x22([0-9]{14})Z").search(&out)?;
    stamp14(m.g(1))
}

/// 这台设备上这个账号（当前工具的）现在的 (at, sum)：e 是 keys.json 里记的，booked 是 config.json 里记着有没有 key，
/// hint 是它的 key_hints（at：记下的时间）。at 是 0：没有过（不传也不比）；sum 是 ?：没开设备统计时（或者旧版本）记下的，
/// 不知道是哪一把，按记下的时间算（key_hints 里没有就看钥匙串里这一条的修改时间，都读不出来当作很早以前）
pub fn keyshare_local(key: &str, e: Option<&V>, booked: bool, hint: Option<&V>) -> (f64, V) {
    let e = e.and_then(|e| e.as_dict()).filter(|e| e.get("at").is_some_and(|at| is_num(&at)));
    let e_at = e.as_ref().map(|e| e.at("at").as_num().unwrap_or(0.0));
    let e_sum = e.as_ref().and_then(|e| e.get("sum")).unwrap_or(V::None);
    let h_at = hint.and_then(|h| h.as_dict()).and_then(|h| h.get("at")).filter(is_num).and_then(|at| at.as_num()).filter(|at| *at > 0.0);
    if booked && (e.is_none() || !e_sum.truthy() || h_at.is_some_and(|h| h > e_at.unwrap_or(0.0))) {
        let at = h_at.or_else(|| keystore_mtime(&acct_id(key, t())).filter(|m| *m != 0.0)).unwrap_or(1.0);
        return (at, V::from("?"));
    }
    if e.is_none() || (!booked && e_sum.truthy()) {
        // 没有过；或者记下过、这台设备上又没有了：服务器上有的取回来
        return (0.0, V::None);
    }
    (e_at.unwrap_or(0.0), e_sum)
}

fn keyshare_note(code: i64, cfg: &Dict, j: &Dict) -> R<String> {
    Ok(if code == 404 {
        KEYSHARE_OLD.to_string()
    } else if code != 200 {
        presence_why(cfg, code, j)?
    } else {
        "设备统计的服务器回的内容看不懂".to_string()
    })
}

/// 同步这台设备上映射里账号记下备查的 API key（见上面这一节的说明）。同时只有一个（keys-sync.lock）：别的正在同步时，force 的
/// （记下、删掉之后，--show 现取）等它跑完再同步（不然这次的改动要等到下一轮），别的就走；force 以外最多 KEYSHARE_EVERY 一次；
/// timeout 是每个请求最多等多久。返回没同步成的原因：成了是空的，没开设备统计、还不到时候、等不到是 None
pub fn keyshare_sync(force: bool, timeout: i64) -> R<Option<String>> {
    let Some(cfg) = presence_load()?.1 else { return Ok(None) };
    presence_dir()?;
    let run = std::fs::OpenOptions::new().append(true).create(true).open(path::join(&PRESENCE_DIR, "keys-sync.lock"))?;
    // 正在跑的那一轮最多两个请求
    let until = monotonic() + if force { (2 * timeout + 5) as f64 } else { 0.0 };
    // SAFETY: run 开着，fd 有效
    while unsafe { libc::flock(run.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        if monotonic() >= until {
            return Ok(None);
        }
        signal::sleep(0.2)?;
    }
    let f = KeyshareFile::enter()?;
    let now = time();
    let num = |j: &Dict, k: &str| j.get(k).filter(is_num).and_then(|v| v.as_num()).unwrap_or(0.0);
    if !force && (now - num(&f.j, "synced") < KEYSHARE_EVERY || now < num(&f.j, "retry")) {
        f.exit(true)?;
        return Ok(None);
    }
    f.j.set("synced", now);
    // 开始时 keys.json 里记的（每一项各拷一份：这一轮里别人改了 keys.json 才看得出来）
    let acc = Dict::new();
    for (k, v) in f.j.dict("acc").expect("enter 保证了 acc 是字典").items() {
        if let Some(v) = v.as_dict() {
            acc.set(k, v.copy());
        }
    }
    f.exit(true)?;
    let err = keyshare_round(&cfg, &acc, timeout)?;
    let f = KeyshareFile::enter()?;
    if err.is_empty() {
        f.j.pop("error");
    } else {
        f.j.set("error", err.as_str());
    }
    if err == KEYSHARE_OLD {
        f.j.set("retry", time() + KEYSHARE_RETRY);
    } else {
        f.j.pop("retry");
    }
    f.exit(true)?;
    Ok(Some(err))
}

/// 同步一轮里的一个账号
struct Item {
    tool: &'static Tool,
    key: String,
    slot: String,
    /// 开始时 keys.json 里这一项（没有、不是字典是 None）
    raw: Option<V>,
    aid: String,
    keys: Keys,
    local: (f64, V),
    booked: bool,
}

/// `with KeyshareFile() as j:`——body 正常做完（包括里面的 continue）就写回 keys.json，出了错的不写
fn with_keys<T>(body: impl FnOnce(&Dict) -> R<T>) -> R<T> {
    let f = KeyshareFile::enter()?;
    let r = body(&f.j);
    f.exit(r.is_ok())?;
    r
}

/// keys.json 里这一项和开始时记的一样吗（`j['acc'].get(slot) == raw`）
fn slot_is(j: &Dict, slot: &str, raw: &Option<V>) -> bool {
    let now = j.dict("acc").expect("enter 保证了 acc 是字典").get(slot);
    match (&now, raw) {
        (None, None) => true,
        (Some(a), Some(b)) => a.py_eq(b),
        // 没有这一项和 None 是一回事（.get 给的都是 None）
        (Some(a), None) | (None, Some(a)) => a.is_none(),
    }
}

/// 同步一轮：问服务器这些账号现在的，比新旧，取回来新的、传上去新的。acc 是开始时 keys.json 里记的：照服务器上的改、
/// 传上去以后，只有这期间这台设备上没再记下、删掉（keys.json 里这一项没变）才算数，不然下一轮再比。返回没成的原因
pub fn keyshare_round(cfg: &Dict, acc: &Dict, timeout: i64) -> R<String> {
    let Some(targets) = presence_targets() else { return Ok("读不了账号表".into()) };
    let salt = cfg.at("salt").py_str();
    let mut items: Vec<Item> = Vec::new();
    // 各工具 config.json 里记着的：(有 key 的账号目录名, key_hints)
    let mut books: Vec<(&'static str, Vec<V>, Dict)> = Vec::new();
    for (tool, key, _) in &targets {
        if !matches!(tool.kind, Kind::Claude | Kind::Codex) {
            continue;
        }
        let _using = Using::tool(tool);
        let Some(ident) = share_ident(tool, key) else {
            continue; // 这台设备没登录这个账号：解不开，也不传
        };
        if !books.iter().any(|(name, _, _)| *name == tool.name) {
            let st = t().state(&cfg_load()?, false);
            books.push((tool.name, py_set(st.get("ref_keys"))?, kc_hints(&st)));
        }
        let (_, booked, hints) = books.iter().find(|(name, _, _)| *name == tool.name).expect("刚放进去的");
        let (email, slot, id) = (parse_key(key).1, keyshare_slot(key), acct_id(key, t()));
        let booked = booked.iter().any(|b| b.as_str() == Some(id.as_str()));
        let raw = acc.get(&slot);
        items.push(Item {
            tool,
            key: key.clone(),
            aid: presence_aid(&salt, tool.name, &email)?,
            keys: keyshare_keys(&salt, tool.name, &email, &ident, KEYSHARE_LABEL)?,
            local: keyshare_local(key, raw.as_ref(), booked, hints.get(&id).as_ref()),
            raw,
            slot,
            booked,
        });
    }
    if items.is_empty() {
        return Ok(String::new());
    }
    let get: Vec<V> = items.iter().map(|x| V::from(x.aid.as_str())).collect();
    let (code, r) = presence_post(cfg, "keys", &dict! { "get" => get }, timeout)?;
    let Some(remote_keys) = r.dict("keys").filter(|_| code == 200) else {
        return keyshare_note(code, cfg, &r);
    };
    let puts = Dict::new();
    let mut sent: Vec<(String, Option<V>, Dict)> = Vec::new();
    let mut err = String::new();
    for x in &items {
        let rec = remote_keys.dict(&x.aid).unwrap_or_default();
        let remote = if rec.is_empty() { None } else { keyshare_open(&x.keys, &x.aid, &rec.get("blob").unwrap_or(V::None))? };
        let (local_at, local_sum) = (x.local.0, &x.local.1);
        let _using = Using::tool(x.tool);
        let aid = acct_id(&x.key, t());
        let remote_at = remote.as_ref().map(|r| r.at("at").as_num().unwrap_or(0.0));
        if let Some(remote) = remote.as_ref().filter(|_| remote_at.is_some_and(|at| at > local_at)) {
            // 服务器上的新：照它改这台设备上的
            let remote_key = remote.at("key");
            let new = dict! { "at" => remote.at("at"), "sum" => keyshare_sum(remote_key.as_str()) };
            let from = rec.str("name").filter(|n| !n.is_empty() && !matches!(rec.get("me"), Some(V::Bool(true))));
            let said = with_keys(|j| -> R<Option<String>> {
                if !slot_is(j, &x.slot, &x.raw) || !path::isfile(&CFG) {
                    // 这期间在这台设备上又记下、删掉了：下一轮再比（config.json 没了：cca restore 刚删掉 ~/.cca，什么都不写）
                    return Ok(None);
                }
                match remote_key.as_str() {
                    None => {
                        kc_del(&aid, true)?;
                        signal::check()?;
                        if kc_read(&aid, REF_ACCT, true).0 != 44 {
                            // 没删掉（钥匙串锁着、读不了）：先不记，下一轮再删
                            signal::check()?;
                            return Ok(Some(format!("其他设备上删掉的 API key 没能从这台设备的{}里删掉", keystore_name())));
                        }
                    }
                    Some(s) if !local_sum.py_eq(&new.at("sum")) || !x.booked => {
                        let extra = dict! { "at" => remote.at("at") };
                        if let Some(from) = &from {
                            extra.set("from", text::head(from, 64));
                        }
                        let put = kc_put(&aid, REF_ACCT, s, true, Some(&extra))?;
                        signal::check()?;
                        if !put {
                            return Ok(Some(format!("API key 存不进这台设备的{}", keystore_name())));
                        }
                    }
                    Some(_) => {}
                }
                j.dict("acc").expect("enter 保证了 acc 是字典").set(x.slot.as_str(), new.clone());
                Ok(None)
            })?;
            if let Some(said) = said {
                err = said;
            }
        } else if local_at > 0.0 && remote_at.is_none_or(|at| at < local_at) {
            // 这台设备上的新（或者服务器上没有、认不出）
            let mut s: Option<String> = None;
            if !local_sum.is_none() {
                let (rc, got) = kc_read(&aid, REF_ACCT, true);
                signal::check()?;
                if got.is_empty() {
                    if rc == 44 {
                        kc_unbook(&aid, true)?; // 钥匙串里已经没有了：下一轮取服务器上的
                    } else if rc != 0 {
                        err = format!("读不了这台设备的{}", keystore_name());
                    }
                    continue;
                }
                s = Some(got);
            }
            let at = V::from(local_at);
            puts.set(x.aid.as_str(), keyshare_seal(&x.keys, &x.aid, s.as_deref(), &at)?);
            sent.push((x.slot.clone(), x.raw.clone(), dict! { "at" => at, "sum" => keyshare_sum(s.as_deref()) }));
        }
    }
    if !puts.is_empty() {
        let (code, r) = presence_post(cfg, "keys", &dict! { "put" => &puts }, timeout)?;
        if code != 200 {
            return keyshare_note(code, cfg, &r);
        }
        with_keys(|j| {
            for (slot, raw, new) in &sent {
                if slot_is(j, slot, raw) {
                    j.dict("acc").expect("enter 保证了 acc 是字典").set(slot.as_str(), new.clone());
                }
            }
            Ok(())
        })?;
    }
    Ok(err)
}

/// 最近一次没同步成的原因（cca devices、_gui state 显示）；成了、没同步过是 None
pub fn keyshare_error() -> Option<String> {
    let e = load_quiet(&path::join(&PRESENCE_DIR, KEYSHARE_FILE))?.as_dict()?.str("error")?;
    (!e.is_empty()).then(|| e.to_string())
}

/// cca _key-sync [force]：同步一次记下备查的 API key（keyshare_kick 在后台起的）；py key-sync now：cca key 账号 --show 这台设备上
/// 没有时先同步一次（有人等着：每个请求最多等 8 秒）。没开设备统计、没同步成都是退出码 0：key 本身在不在由调用的地方再看
pub fn cmd_key_sync(args: &[String]) -> R<()> {
    arity("cmd_key_sync", args, 0, 1)?;
    let mode = arg(args, 0, "");
    match keyshare_sync(mode == "force" || mode == "now", if mode == "now" { 8 } else { 15 }) {
        // SystemExit：config.json 读不了（die 已经说了原因）
        Err(e) if e.is_os_or_value() || e.is_exit() => {
            e.swallowed();
            Ok(())
        }
        Err(e) => Err(e),
        Ok(_) => Ok(()),
    }
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
