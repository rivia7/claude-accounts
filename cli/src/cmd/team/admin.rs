//! Team 的管理员这一头（拿着根密钥的设备）：建 Team、接上服务器、给设备发邀请、定每台设备能用哪些账号、维护账号 → 节点和节点的定义、
//! 把每台设备的那一份配置封好签好存到服务器上。
//!
//! admin.json（600，只有管理员的设备有）：
//! ```text
//! {"v": 1, "master": 主密钥（64 个十六进制字符）, "salt": 这个 Team 的盐, "next": 下一台设备的序数, "gen": 服务器上这一份记录的编号,
//!  "alias_n": {工具: 默认的别名编到了第几号（「Team 的名字-N」的 N）}（各工具各编各的，用过的号不再用；3.5.3-beta.5 是一个数）,
//!  "config": {"accounts": {工具: {邮箱: 节点}}, "tool_aliases": {工具: {邮箱: 别名}}, "aliases": {邮箱: 别名}（照 tool_aliases 算的，
//!             给旧版本用：每个邮箱用它最前面那个工具里的）, "alias_lock": true（锁定了别名；没锁定时没有）, "install_node": 节点,
//!             "nodes": {"group", "proxies"} | null},
//!  "devices": {设备编号: {"n": 序数, "label": 管理员起的名字（设备的名字以它为准：align_names）, "state": local | invited | joined |
//!              expired（3.6.6 起移除了的、自己退出了的不记：以前记着的 revoked、left 下次和服务器对一遍时删掉）, "pub": 它的公钥, "name": 它在服务器上的名字, "grants": "all" | {工具: [邮箱…]}, "seq": 发布到第几版,
//!              "sha": 那一版内容的摘要, "exp": 邀请什么时候过期}},
//!  "deploy": {"target": [用户@]主机, "ssh_port": SSH 端口（没写是空的）, "dir": 目录, "identity": 私钥（-i 的绝对路径，没写是空的）}
//!            （cca team deploy 装到了哪台、用哪把私钥登录的：删除 Team 时照它卸载；手动 cca team attach 换到别的地址时去掉）}
//! ```
//! 每台设备那串随机数（邀请里的 k）不存：是 HMAC(主密钥, "cca-team-k-v1\0" 序数) 的前 16 个字节，随时重算。所以备份根密钥和主密钥
//! 就够了（管理员的本机状态丢了，各台设备的密钥还算得出来）。

pub mod config;
pub mod nodes;

use super::doc::{
    admins_sign, admins_text, astate_keys, astate_open, astate_seal, doc_digest, doc_problems, envelope_seal, invite_encode,
    invite_parts, pub_proof, root_line, team_id, team_name_ok, token_hash, Invite, Signer, SEQ_MAX,
};
use super::{cfg_of, fail, fault, find_team, index_add, now, pub_line, save_team, sync_locked, team_lock};
use crate::cmd::hub::deploy::{team_install, team_uninstall, url_parts};
use crate::cmd::hub::hosted::team_files;
use crate::cmd::hub::own_account;
use crate::cmd::hub::{hex32, host_name, host_of, keygen, name_ok, with_https_port, NAME_BAD};
use crate::cmd::mail::{doc_forwarders, fwd_norm, fwd_widens, team_fwd_check};
use crate::cmd::presence::{key_at, presence_post, url_ok, PRESENCE_PIN_RE};
use crate::cmd::{arity, keyshare::is_num};
use crate::core::base::sh;
use crate::core::model::{is_api, node_of, parse_key};
use crate::core::policy::{alias_ok, legacy_n, policy_sources, team_default_alias, team_default_n};
use crate::core::team::{load_team, team_dir, team_path, teams, Role, Team, TEAMS_DIR};
use crate::core::tool::{claude, tools};
use crate::py::fs::{self, load_quiet, save_json};
use crate::py::hash::{from_hex, hmac_sha256, sha256_hex, to_hex};
use crate::py::obj::{str_list, Dict, List, V};
use crate::py::rand::token_hex;
use crate::py::{json, path, Fail, R};
use crate::{dict, out};
use std::collections::{BTreeSet, HashMap, HashSet};

/// 邀请默认多久过期（秒）；服务器收 60 秒～30 天
const INVITE_TTL: i64 = 7 * 86400;
/// team_enroll、team_put 一次最多几台设备（服务器的 MAX_TEAM_BATCH 是 64；存配置的请求最多 1 MB，所以按大小再分）
const BATCH: usize = 32;
const PUT_CHARS: usize = 600_000;

fn admin_path(tid: &str) -> String {
    team_path(tid, "admin.json")
}

/// 管理员的状态；这台设备不是这个 Team 的管理员是给人看的原因
fn admin_load(t: &Team) -> Result<Dict, String> {
    let j = load_quiet(&admin_path(&t.id)).and_then(|j| j.as_dict());
    let ok = |j: &Dict| {
        j.str("master").is_some_and(|m| m.len() == 64 && from_hex(&m).is_some())
            && j.dict("devices").is_some()
            && j.dict("config").is_some()
    };
    match j.filter(|j| (t.role == Role::Admin || t.coadmin()) && ok(j)) {
        Some(j) => Ok(j),
        None => Err(format!("这台设备不是「{}」的管理员（建这个 Team 的设备，和它授予了管理员身份的设备才能管理它）", t.name)),
    }
}

/// 存 admin.json；几位管理员共用着（有管理员的密钥 akey）时也存到服务器上（内容变了才存）
fn admin_save(tid: &str, a: &Dict) -> R<()> {
    save_json(&admin_path(tid), &V::Dict(a.copy()), Some(0o600), true)?;
    if a.str("akey").is_some() {
        shared_push(tid, a)?;
    }
    Ok(())
}

// --- 几位管理员共用的状态（3.6.15 起） ---------------------------------------------------------------------------------------
//
// 建 Team 的设备（拿着根密钥）给别的设备管理员身份时，admin.json 除了下面这几项都封好存到服务器上（team_astate_put，密钥由 akey 派生，
// 服务器只存密文和版本号）：每条管理员的命令开头取回来（版本号比记着的新就换上），改了就存回去（照着改的那一版要还是服务器上的那一版，
// 不然是别的管理员刚改过：这一次不算，重新运行）。akey 经根密钥签的配置（admin.key）交给别的管理员的设备

/// 只在这台设备上的：部署记下的 SSH 登录（只有建 Team 的设备部署、删除）、管理员的密钥、服务器上的版本号、上次存上去的内容的摘要
const LOCAL_KEYS: [&str; 4] = ["deploy", "akey", "arev", "asha"];

/// 共用的那一份：admin.json 去掉只在这台设备上的
fn shared_doc(a: &Dict) -> Dict {
    let d = a.copy();
    for k in LOCAL_KEYS {
        d.pop(k);
    }
    d
}

fn shared_sha(d: &Dict) -> String {
    to_hex(&crate::py::hash::sha256(json::dumps(&V::Dict(d.copy()), json::Fmt::new().utf8().compact())))
}

/// 存到服务器上：照着改的是 arev 那一版，存上了记下新的版本号；别的管理员刚改过（409 stale）是 fail busy
fn shared_push(tid: &str, a: &Dict) -> R<()> {
    let doc = shared_doc(a);
    let sha = shared_sha(&doc);
    if a.str("asha").as_deref() == Some(sha.as_str()) {
        return Ok(());
    }
    let Some(t) = load_team(tid) else { return Ok(()) };
    let Some(keys) = a.str("akey").and_then(|k| astate_keys(&k)) else { return fail("server", "管理员的密钥不对") };
    let cfg = match admin_cfg(&t)? {
        Ok(c) => c,
        Err(why) => return fail("not_attached", &why),
    };
    let base = a.get("arev").and_then(|r| r.as_int()).unwrap_or(0);
    let ct = astate_seal(&keys, tid, base + 1, &doc)?;
    let (code, j) = presence_post(&cfg, "team_astate_put", &dict! { "base" => base, "ct" => ct }, 30)?;
    match (code, j.str("error").as_deref()) {
        (200, _) => {
            a.set("arev", j.get("rev").and_then(|r| r.as_int()).unwrap_or(base + 1));
            a.set("asha", sha);
            save_json(&admin_path(tid), &V::Dict(a.copy()), Some(0o600), true)
        }
        (409, Some("stale")) => fail(
            "busy",
            &format!("另一位管理员刚改过「{}」的设置：这一次的改动没存上（服务器上照旧是他改的那一版），重新运行这条命令", t.name),
        ),
        _ => {
            let (c, why) = admin_fault(&t, code, &j);
            fail(c, &why)
        }
    }
}

/// 这台设备上管理员的密钥：建 Team 的设备记在 admin.json 的 akey，别的管理员的在 admin_key（同步配置时记下的）
fn akey_of(t: &Team, local: Option<&Dict>) -> Option<String> {
    if t.coadmin() {
        return fs::read_text(&t.path("admin_key")).ok().map(|s| s.trim().to_string()).filter(|k| astate_keys(k).is_some());
    }
    local.and_then(|a| a.str("akey")).map(|k| k.to_string())
}

/// 取回服务器上共用的那一份：比这台设备上记着的新就换上（只在这台设备上的几项照旧）。不用共用的（还没给过别的设备管理员身份）什么都不做
fn shared_pull(t: &Team) -> R<Result<(), String>> {
    let local = load_quiet(&admin_path(&t.id)).and_then(|j| j.as_dict());
    let Some(akey) = akey_of(t, local.as_ref()) else {
        return Ok(if t.coadmin() {
            Err(format!("这台设备上没有「{}」的管理员密钥：先 cca team sync", t.name))
        } else {
            Ok(())
        });
    };
    let cfg = match admin_cfg(t)? {
        Ok(c) => c,
        Err(why) => return Ok(Err(why)),
    };
    let (code, j) = presence_post(&cfg, "team_astate_get", &dict! {}, 20)?;
    if code != 200 {
        return Ok(Err(admin_fault(t, code, &j).1));
    }
    let rev = j.get("rev").and_then(|r| r.as_int()).unwrap_or(0);
    let mine = local.as_ref().and_then(|a| a.get("arev")).and_then(|r| r.as_int()).unwrap_or(0);
    if let Some(a) = local.as_ref().filter(|a| t.coadmin() && a.str("akey").as_deref() != Some(akey.as_str())) {
        a.set("akey", akey.as_str());
        save_json(&admin_path(&t.id), &V::Dict(a.copy()), Some(0o600), true)?;
    }
    if rev == 0 || rev <= mine {
        if rev == 0 && t.coadmin() {
            return Ok(Err(format!(
                "建「{}」的设备还没把管理员的设置传到服务器上：请它运行一次 cca team devices {}",
                t.name, t.name
            )));
        }
        // 服务器上的比记着的旧（记录回退了）：照这台设备的，下次存的时候照服务器的版本号接着往上加
        if let Some(a) = local.filter(|_| rev < mine) {
            a.set("arev", rev);
            a.pop("asha");
            save_json(&admin_path(&t.id), &V::Dict(a), Some(0o600), true)?;
        }
        return Ok(Ok(()));
    }
    let keys = astate_keys(&akey).expect("akey_of 查过样子");
    let Some(doc) = astate_open(&keys, &t.id, rev, &j.at("ct")) else {
        return Ok(Err(format!("服务器上「{}」的管理员设置解不开（管理员的密钥换了？先 cca team sync）", t.name)));
    };
    if !doc.str("master").is_some_and(|m| m.len() == 64 && from_hex(&m).is_some()) || doc.dict("devices").is_none() {
        return Ok(Err(format!("服务器上「{}」的管理员设置样子不对", t.name)));
    }
    let sha = shared_sha(&doc);
    for k in LOCAL_KEYS {
        if let Some(v) = local.as_ref().and_then(|a| a.get(k)) {
            doc.set(k, v);
        }
    }
    // 别的管理员的密钥以 admin_key 为准（同步配置时记下的），admin.json 里也记一份：admin_save 照它存回服务器
    doc.set("akey", akey.as_str());
    doc.set("arev", rev);
    doc.set("asha", sha);
    save_json(&admin_path(&t.id), &V::Dict(doc), Some(0o600), true)?;
    Ok(Ok(()))
}

/// 签配置的那一把：建 Team 的设备是根密钥，别的管理员是它自己这台设备的密钥（配置里带着它的公钥：by）
fn signer(t: &Team) -> R<Signer> {
    if t.coadmin() {
        let key = t.path("id_ed25519");
        return Ok(Signer { by: Some(pub_line(&key)?), key });
    }
    Ok(Signer { key: t.path("root_ed25519"), by: None })
}

/// 管理员名单（admin.json 的 admins、admins_epoch）照根密钥签好放到服务器上。服务器上已经是这一版或者更新的（409 stale）也算放好了
fn admins_post(t: &Team, a: &Dict) -> R<Result<(), (&'static str, String)>> {
    let cfg = match admin_cfg(t)? {
        Ok(c) => c,
        Err(why) => return Ok(Err(("not_attached", why))),
    };
    let epoch = a.get("admins_epoch").and_then(|e| e.as_int()).unwrap_or(0);
    let Some(text) = admins_list(t, a, epoch) else { return Ok(Err(("server", "管理员名单的样子不对".into()))) };
    let Some(body) = admins_sign(&t.path("root_ed25519"), &text)? else {
        return Ok(Err(("sign", "没法用 Team 的根密钥签名（ssh-keygen）".into())));
    };
    let (code, j) = presence_post(&cfg, "team_admins", &body, 20)?;
    match (code, j.str("error").as_deref()) {
        (200, _) => Ok(Ok(())),
        (409, Some("stale")) if j.get("epoch").and_then(|e| e.as_int()).is_some_and(|e| e >= epoch) => Ok(Ok(())),
        _ => Ok(Err(admin_fault(t, code, &j))),
    }
}

/// 名单的文字：admins 里各台设备的公钥和名字（照设备编号的先后）
fn admins_list(t: &Team, a: &Dict, epoch: i64) -> Option<String> {
    let devs = a.dict("devices").unwrap_or_default();
    let mut rows: Vec<(String, String)> = Vec::new();
    for dev in a.dict("admins").unwrap_or_default().keys() {
        let d = devs.dict(&dev)?;
        let line = root_line(&d.str("pub")?)?;
        let name = d.str("label").filter(|n| team_name_ok(n)).map_or_else(|| "device".to_string(), |n| n.to_string());
        rows.push((line, name));
    }
    Some(admins_text(&t.id, epoch, &rows))
}

/// 只有建 Team 的设备（拿着根密钥）能做的：别的管理员是 fail owner
fn owner_only(t: &Team, what: &str) -> R<()> {
    if t.coadmin() {
        return fail("owner", &format!("只有建「{}」的设备（拿着根密钥）能{what}", t.name));
    }
    Ok(())
}

/// 第 n 台设备那串随机数（邀请里的 k）：HMAC(主密钥, "cca-team-k-v1\0" n) 的前 16 个字节
fn device_secret(master: &str, n: i64) -> String {
    let key = from_hex(master).unwrap_or_default();
    to_hex(&hmac_sha256(&key, format!("cca-team-k-v1\0{n}").as_bytes())[..16])
}

/// 管理员的请求用的配置（presence_post 用）：建 Team 的设备拿根密钥签名，别的管理员拿它自己这台设备的密钥（服务器照管理员名单认）；
/// 没接上服务器、密钥不见了是给人看的原因
fn admin_cfg(t: &Team) -> R<Result<Dict, String>> {
    if !t.attached() {
        return Ok(Err(format!("「{}」还没有接上同步服务器：先 cca team attach {} https://服务器:端口", t.name, &t.id[..8])));
    }
    if t.coadmin() {
        return Ok(match key_at(&t.path("id_ed25519"))? {
            Some((key, fp)) => Ok(cfg_of(&t.url, &t.pins, key, fp)),
            None => Err(format!("这台设备在「{}」里的密钥不见了（~/.cca/teams/{}/id_ed25519）", t.name, t.id)),
        });
    }
    Ok(match key_at(&t.path("root_ed25519"))? {
        Some((key, fp)) => Ok(cfg_of(&t.url, &t.pins, key, fp)),
        None => Err(format!("「{}」的根密钥不见了（~/.cca/teams/{}/root_ed25519）", t.name, t.id)),
    })
}

/// 管理员的请求没成的原因
fn admin_fault(t: &Team, code: i64, j: &Dict) -> (&'static str, String) {
    match code {
        403 if t.coadmin() => (
            "denied",
            format!("{} 不认这台设备是「{}」的管理员（管理员身份被收回了？服务器不是这个 Team 的？）", host_of(&t.url), t.name),
        ),
        403 => {
            ("denied", format!("{} 配置的不是这个 Team 的根公钥（服务器要用 --team-root 这个 Team 的根公钥运行）", host_of(&t.url)))
        }
        404 if j.str("error").as_deref() == Some("not_found") => (
            "old",
            format!("{} 上的服务器版本太旧，不支持几位管理员：先更新服务器（cca team deploy {} …）", host_of(&t.url), t.name),
        ),
        _ => fault(&t.url, code, j),
    }
}

/// 命令里写的是哪台设备：编号、编号的开头（至少 4 个字符）、管理员起的名字或者它在服务器上的名字；找不到、不止一台是给人看的原因
fn find_device(a: &Dict, sel: &str) -> Result<String, String> {
    let devs = a.dict("devices").unwrap_or_default();
    if devs.has(sel) {
        return Ok(sel.to_string()); // 完整的编号：就是它（别的设备起了同样的名字也不算重）
    }
    let hits: Vec<String> = devs
        .items()
        .iter()
        .filter(|(dev, d)| {
            let d = d.as_dict().unwrap_or_default();
            (sel.len() >= 4 && dev.starts_with(sel))
                || d.str("label").as_deref() == Some(sel)
                || d.str("name").as_deref() == Some(sel)
        })
        .map(|(dev, _)| dev.to_string())
        .collect();
    match hits.len() {
        1 => Ok(hits.into_iter().next().expect("刚数过有一个")),
        0 => Err(format!("没有叫「{sel}」的设备（cca team devices 看有哪些）")),
        _ => Err(format!("「{sel}」不止一台设备，写编号的开头")),
    }
}

// --- 每台设备的那一份配置 ---------------------------------------------------------------------------------------------------

/// 各工具的别名算出来是什么样（alias_plan）：tool_aliases、按邮箱的 aliases、alias_n
struct AliasPlan {
    by_tool: Dict,
    flat: Dict,
    counters: Dict,
}

/// 配置里每个账号都有别名，按工具记（{工具: {邮箱: 别名}}）：没有的（不合规的、在这个工具里和前面的账号撞了的）编成「Team 的名字-1」
/// 「Team 的名字-2」……（team_default_alias；书房组-1）——各工具（Claude、Codex、Antigravity）各编各的，各自的第一个账号都是 1 号；
/// 同一个邮箱在两个工具里是两个账号，各有各的别名。以前（3.7.8 及以前）默认编的 team-N 换成「Team 的名字-N」（号不变；换成的那个被这个
/// 工具里别的账号占着就留着 team-N）：换过一次以后再算还是一样，所以不用记换没换过，下一次发布时各台设备跟着换（team_alias_follow）。
/// alias_n（{工具: 编到第几}）记着各工具编到第几了，用过的号不再用（去掉的账号在各台设备上收起来了，再给回来时照原来的键：号给了别的账号
/// 就撞了）；已经不在 Team 里的账号的别名去掉。照工具的先后、各工具里账号加进来的先后编，别名也照这个先后排（每一次算出来的一样：配置的
/// 摘要不会白变）。按邮箱、各工具通用的 aliases 照它算：每个邮箱用它最前面那个工具里的（给 3.5.3-beta.5 及以前的 cca：旧版本的设备、
/// 管理员这台换回旧版本时）。没有 tool_aliases 的（3.5.3-beta.5 及以前的 Team）照 aliases 给它在的每个工具；3.5.3-beta.5 的 alias_n 是
/// 一个数（那时各工具一起编）：那时编的 team-N 都去掉，照各工具重新编。team：这个 Team 现在的名字。只算，不改 admin.json
fn alias_plan(a: &Dict, team: &str) -> AliasPlan {
    let config = a.dict("config").unwrap_or_default();
    let accounts = config.dict("accounts").unwrap_or_default();
    let legacy = a.get("alias_n").is_some_and(|v| v.as_dict().is_none());
    let counters = if legacy { Dict::new() } else { a.dict("alias_n").unwrap_or_default() };
    let (have, flat) = (config.dict("tool_aliases"), config.dict("aliases").unwrap_or_default());
    let plan = AliasPlan { by_tool: Dict::new(), flat: Dict::new(), counters: Dict::new() };
    for tool in tools().iter() {
        let emails: Vec<String> = accounts.dict(tool.name).unwrap_or_default().keys().iter().map(|e| e.to_string()).collect();
        let old = match &have {
            Some(h) => h.dict(tool.name).unwrap_or_default(),
            None => flat.clone(),
        };
        let mut used: HashSet<String> = HashSet::new();
        let mut kept: HashMap<&str, String> = HashMap::new();
        for e in &emails {
            let Some(x) = old.get(e).filter(alias_ok).map(|x| x.py_str()) else { continue };
            if !(have.is_none() && legacy && legacy_n(&x).is_some()) && used.insert(x.clone()) {
                kept.insert(e, x);
            }
        }
        for e in &emails {
            let Some(k) = kept.get(e.as_str()).and_then(|x| legacy_n(x)) else { continue };
            let y = team_default_alias(team, k);
            if !used.contains(&y) {
                used.remove(&kept[e.as_str()]);
                used.insert(y.clone());
                kept.insert(e, y);
            }
        }
        let mut n = counters.get(tool.name).and_then(|v| v.as_int()).filter(|n| *n >= 0).unwrap_or(0);
        n = kept.values().filter_map(|x| team_default_n(x, team)).fold(n, i64::max);
        let m = Dict::new();
        for e in &emails {
            let x = kept.remove(e.as_str()).unwrap_or_else(|| loop {
                n += 1;
                let cand = team_default_alias(team, n);
                if used.insert(cand.clone()) {
                    break cand;
                }
            });
            if !plan.flat.has(e) {
                plan.flat.set(e.as_str(), x.as_str());
            }
            m.set(e.as_str(), x);
        }
        if !m.is_empty() {
            plan.by_tool.set(tool.name, m);
        }
        if n > 0 {
            plan.counters.set(tool.name, n);
        }
    }
    plan
}

/// 照 alias_plan 改 admin.json 里的 config 和 alias_n（没存盘）
fn default_aliases(a: &Dict, team: &str) {
    let plan = alias_plan(a, team);
    let config = a.dict("config").unwrap_or_default();
    config.set("tool_aliases", plan.by_tool);
    config.set("aliases", plan.flat);
    a.set("config", config);
    a.set("alias_n", plan.counters);
}

/// Team 改了名字：照原来的名字默认编的别名（「旧名字-N」）换成「新名字-N」（号不变）；换成的那个被这个工具里别的账号占着就留着原来的
/// （以后它看起来就像管理员起的，不再跟着改名）。以前的 team-N 不在这里换，之后的 default_aliases 照新名字换：先换「旧名字-N」，原来占着
/// 第几号的账号改了名字以后还是它占着。只改 tool_aliases（没存盘）：之后要 default_aliases
fn rename_defaults(a: &Dict, old: &str, new: &str) {
    let config = a.dict("config").unwrap_or_default();
    let by_tool = config.dict("tool_aliases").unwrap_or_default();
    for (tool, names) in by_tool.items() {
        let Some(names) = names.as_dict() else { continue };
        for (e, x) in names.items() {
            let Some(k) = x.as_str().filter(|x| legacy_n(x).is_none()).and_then(|x| team_default_n(x, old)) else { continue };
            let y = team_default_alias(new, k);
            if x.as_str() != Some(y.as_str()) && alias_holder(&names, &e, &y).is_none() {
                names.set(&*e, y.as_str());
            }
        }
        by_tool.set(&*tool, names);
    }
    config.set("tool_aliases", by_tool);
    a.set("config", config);
}

/// 这个工具（tool_aliases 里的那一节）里叫 alias 的另一个账号的邮箱
fn alias_holder(names: &Dict, email: &str, alias: &str) -> Option<String> {
    names.items().iter().find(|(e, x)| **e != *email && x.as_str() == Some(alias)).map(|(e, _)| e.to_string())
}

/// 工具:邮箱（邮箱小写）；不对是 None
fn account_spec(s: &str) -> Option<(String, String)> {
    let (tool, email) = s.split_once(':')?;
    let email = email.trim().to_lowercase();
    (tools().iter().any(|t| t.name == tool) && email.contains('@')).then(|| (tool.to_string(), email))
}

/// 这台设备能用的账号：{工具: {邮箱: 节点}}（grants 是 "all" 时就是全部）
fn granted(config: &Dict, grants: &V) -> Dict {
    let all = config.dict("accounts").unwrap_or_default();
    let out = Dict::new();
    for (tool, m) in all.items() {
        let Some(m) = m.as_dict() else { continue };
        let pick = Dict::new();
        match grants.as_dict() {
            None if grants.as_str() == Some("all") => pick.update(&m),
            None => {}
            Some(g) => {
                for e in g.list(&tool).map(|l| l.strs()).unwrap_or_default() {
                    if let Some(n) = m.get(&e) {
                        pick.set(&*e, n);
                    }
                }
            }
        }
        if !pick.is_empty() {
            out.set(&*tool, pick);
        }
    }
    out
}

/// 参考节点里这台设备用得着的：账号走的节点、安装客户端用的节点，和它们经 dialer-proxy 连着的
fn needed_nodes(nodes: Option<Dict>, used: &BTreeSet<String>) -> V {
    let Some(nodes) = nodes else { return V::None };
    let proxies: Vec<Dict> =
        nodes.list("proxies").map(|l| l.items().iter().filter_map(|p| p.as_dict()).collect()).unwrap_or_default();
    let mut want = used.clone();
    loop {
        let more: Vec<String> = proxies
            .iter()
            .filter(|p| p.str("name").is_some_and(|n| want.contains(&*n)))
            .filter_map(|p| p.str("dialer-proxy").map(|d| d.to_string()))
            .filter(|d| !want.contains(d))
            .collect();
        if more.is_empty() {
            break;
        }
        want.extend(more);
    }
    let keep: Vec<V> = proxies.into_iter().filter(|p| p.str("name").is_some_and(|n| want.contains(&*n))).map(V::Dict).collect();
    if keep.is_empty() {
        V::None
    } else {
        V::Dict(dict! { "group" => nodes.at("group"), "proxies" => keep })
    }
}

/// 给一台设备的配置的明文（格式见 doc.rs 的 doc_problems）
fn build_doc(t: &Team, a: &Dict, id: &str, dev: &Dict) -> Dict {
    let config = a.dict("config").unwrap_or_default();
    // 拿别的密钥用了这个邀请的（reconcile 记的 mismatch）：一个账号都不给
    let grants = if dev.get("mismatch").is_some_and(|m| m.truthy()) { V::Dict(Dict::new()) } else { dev.at("grants") };
    let accounts = granted(&config, &grants);
    let mut used: BTreeSet<String> = BTreeSet::new();
    for m in accounts.values().iter().filter_map(|m| m.as_dict()) {
        for n in m.values() {
            used.insert(n.py_str());
        }
    }
    // 给的账号的别名：按工具的（tool_aliases），和按邮箱的（aliases，每个邮箱用它最前面那个工具里的：给 3.5.3-beta.5 及以前的 cca）
    let (by_tool, aliases, names) = (Dict::new(), Dict::new(), config.dict("tool_aliases").unwrap_or_default());
    for tool in tools().iter() {
        let (Some(m), Some(src)) = (accounts.dict(tool.name), names.dict(tool.name)) else { continue };
        let got = Dict::new();
        for e in m.keys() {
            if let Some(x) = src.get(&e) {
                if !aliases.has(&e) {
                    aliases.set(&*e, x.clone());
                }
                got.set(&*e, x);
            }
        }
        if !got.is_empty() {
            by_tool.set(tool.name, got);
        }
    }
    let install = config.str("install_node").map(|n| n.to_string()).unwrap_or_default();
    if !install.is_empty() {
        used.insert(install.clone());
    }
    let has_claude = accounts.dict(claude().name).is_some_and(|m| !m.is_empty());
    let doc = dict! {
        "v" => 1, "at" => now(), "name" => t.name.as_str(), "salt" => a.at("salt"),
        "server" => dict! { "u" => t.url.as_str(), "p" => str_list(&t.pins) },
        "accounts" => accounts, "aliases" => aliases, "tool_aliases" => by_tool, "install_node" => install,
        "nodes" => needed_nodes(config.dict("nodes"), &used),
    };
    // 锁定了别名才有这一项（没锁定的 Team 的配置和以前一样，不用重新发布）
    if alias_locked(a) {
        doc.set("alias_lock", true);
    }
    // 信任的转发中转（cca team forwarder）：设了、这台设备又拿到了 Claude 账号才有这一项（没设的 Team、没有 Claude 账号的设备，配置和以前
    // 一样，不用重新发布）。设备上照它，这台设备给这些账号登记的不算（mail::mail_forwarders）
    let fw = doc_forwarders(&config);
    if !fw.is_empty() && has_claude {
        doc.set("mail_forwarders", str_list(&fw));
    }
    // 给过别的设备管理员身份才有这两项（3.6.15 起）：管理员名单到第几版了（设备照它不认更旧的名单），给管理员的那几台带着管理员的
    // 密钥（只认根密钥签的这一项：别的管理员签的那一份也带着同一把，免得内容变来变去）
    if let Some(e) = a.get("admins_epoch").and_then(|e| e.as_int()).filter(|e| *e > 0) {
        doc.set("admins_epoch", e);
    }
    if let Some(k) = a.str("akey").filter(|_| a.dict("admins").is_some_and(|m| m.has(id))) {
        doc.set("admin", dict! { "key" => &*k });
    }
    doc
}

/// 管理员锁定了这个 Team 里账号的别名（admin.json 的 config.alias_lock）：各台设备上不能自己改，改过的换回 Team 给的
fn alias_locked(a: &Dict) -> bool {
    a.dict("config").is_some_and(|c| matches!(c.get("alias_lock"), Some(V::Bool(true))))
}

/// 发布的结果：存上了几台、每台没存上的原因
#[derive(Default)]
struct Published {
    stored: usize,
    problems: Vec<String>,
}

/// team_put 各个请求合起来的结果：存上了的编号、服务器说序号不比它存着的大的（{编号: 它存着的序号}）、它不认得的编号、
/// 它哪一种都没说的（回话不对）
#[derive(Default)]
struct Put {
    stored: Vec<String>,
    stale: Dict,
    unknown: Vec<String>,
    silent: Vec<String>,
}

/// 把封好的配置存到服务器上：一个请求最多 BATCH 台、密文加起来不超过 PUT_CHARS。返回已经问到的结果，和中途没成的那个请求
/// （状态码、回的；前面几个请求存上的照样算）
fn put_docs(cfg: &Dict, envs: &[(String, Dict)]) -> R<(Put, Option<(i64, Dict)>)> {
    let mut out = Put::default();
    let mut i = 0;
    while i < envs.len() {
        let (mut j, mut size) = (i, 0usize);
        while j < envs.len() && j - i < BATCH {
            let len = envs[j].1.dict("ct").map_or(0, |c| c.at("c").py_str().len());
            if j > i && size + len > PUT_CHARS {
                break;
            }
            size += len;
            j += 1;
        }
        let docs = Dict::new();
        for (dev, env) in &envs[i..j] {
            docs.set(dev.as_str(), env.copy());
        }
        let (code, reply) = presence_post(cfg, "team_put", &dict! { "docs" => docs }, 30)?;
        if code != 200 {
            return Ok((out, Some((code, reply))));
        }
        let (stored, stale) = (reply.list("stored").unwrap_or_default(), reply.dict("stale").unwrap_or_default());
        let unknown = reply.list("unknown").unwrap_or_default();
        for (dev, _) in &envs[i..j] {
            if stored.has_str(dev) {
                out.stored.push(dev.clone());
            } else if let Some(cur) = stale.get(dev) {
                out.stale.set(dev.as_str(), cur);
            } else if unknown.has_str(dev) {
                out.unknown.push(dev.clone());
            } else {
                out.silent.push(dev.clone());
            }
        }
        i = j;
    }
    Ok((out, None))
}

fn label_of(d: &Dict, dev: &str) -> String {
    d.str("label").map_or_else(|| dev.to_string(), |l| l.to_string())
}

/// 把内容变了的配置封好、签好存到服务器上（only：只管这一台）。存上了的记下序号和摘要；服务器上的序号比记着的大（管理员的状态是从
/// 备份里恢复的）就接着它的往上加再存一次——它说的数要像话（不比记着的小、没到上限），不然不信；服务器不认得的编号：加入过的（和服务器
/// 对过以后才退出的）不再记，别的（邀请过期很久了）记成 expired。改了 admin.json
fn publish(t: &Team, a: &Dict, only: Option<&str>) -> R<Result<Published, String>> {
    let cfg = match admin_cfg(t)? {
        Ok(c) => c,
        Err(why) => return Ok(Err(why)),
    };
    default_aliases(a, &t.name); // 以前加进来、还没有别名的账号（3.5.3-beta.5 以前的 Team）也编上，以前默认的 team-N 换成「Team 的名字-N」
    let devs = a.dict("devices").unwrap_or_default();
    let (master, sign) = (a.at("master").py_str(), signer(t)?);
    let seq_of = |d: &Dict| d.get("seq").and_then(|s| s.as_int()).filter(|s| *s >= 0).unwrap_or(0);
    let mut out = Published::default();
    let mut again: Vec<String> = Vec::new(); // 第二轮：服务器说序号不够大的那些
    for round in 0..2 {
        let mut todo: Vec<(String, Dict)> = Vec::new();
        let shas = Dict::new();
        for (dev, d) in devs.items() {
            let Some(d) = d.as_dict() else { continue };
            let live = matches!(d.str("state").as_deref(), Some("invited" | "joined"));
            if !live || only.is_some_and(|o| o != &*dev) || (round == 1 && !again.iter().any(|x| **x == *dev)) {
                continue;
            }
            let doc = build_doc(t, a, &dev, &d);
            let sha = doc_digest(&doc);
            if round == 0 && d.str("sha").as_deref() == Some(sha.as_str()) {
                continue;
            }
            let label = label_of(&d, &dev);
            if let Some(why) = doc_problems(&doc).into_iter().next() {
                out.problems.push(format!("{label}：配置的内容不对（{why}）"));
                continue;
            }
            let seq = seq_of(&d) + 1;
            let Some(parts) = invite_parts(&device_secret(&master, d.get("n").and_then(|n| n.as_int()).unwrap_or(0))) else {
                continue;
            };
            if seq >= SEQ_MAX {
                out.problems.push(format!("{label}：配置的序号用完了"));
                continue;
            }
            match envelope_seal(&sign, &parts.keys, &t.id, &dev, seq, &doc)? {
                Ok(env) => {
                    shas.set(&*dev, sha);
                    todo.push((dev.to_string(), env));
                }
                Err(why) => out.problems.push(format!("{label}：{why}")),
            }
        }
        if todo.is_empty() {
            break;
        }
        let (put, failed) = put_docs(&cfg, &todo)?;
        again.clear();
        for (dev, env) in &todo {
            let Some(d) = devs.dict(dev) else { continue };
            let label = label_of(&d, dev);
            if put.stored.contains(dev) {
                d.set("seq", env.at("seq"));
                d.set("sha", shas.at(dev));
                out.stored += 1;
            } else if let Some(cur) = put.stale.get(dev) {
                match cur.as_int().filter(|c| round == 0 && *c >= seq_of(&d) && *c < SEQ_MAX - 1) {
                    Some(c) => {
                        d.set("seq", c);
                        again.push(dev.clone());
                    }
                    None => out.problems.push(format!("{label}：服务器说它存着的配置的序号是 {}，对不上，没存上", cur.py_str())),
                }
            } else if put.unknown.contains(dev) {
                if d.str("state").as_deref() == Some("joined") {
                    devs.pop(dev); // 加入过、服务器却不认得了：它刚退出了（和服务器对过以后），这边也不记了
                } else {
                    d.set("state", "expired");
                    out.problems.push(format!("{label}：服务器不认得这台设备（邀请过期很久了，或者被移除了）"));
                }
            } else if put.silent.contains(dev) {
                out.problems.push(format!("{label}：服务器没说这一份存上了没有"));
            }
        }
        if let Some((code, j)) = failed {
            admin_save(&t.id, a)?;
            return Ok(Err(admin_fault(t, code, &j).1));
        }
    }
    admin_save(&t.id, a)?;
    Ok(Ok(out))
}

/// 换了一台服务器以后，在原来那台上收尾（admin.json 的 moved_from 记着它：没做完时下次接着做）：
/// - 还没用掉的邀请写的是原来那台的地址：到原来那台上撤回（不然拿着它的人会加入一台没人管的服务器），它们在这边记成 expired，要重新发
/// - 各台设备现在的配置（里面是新的地址）照同一个序号也存到原来那台上：还连着它的设备取到以后自己换过去
///
/// 原来那台连不上、不认了是给人看的原因（那些设备要重新邀请），成了是空的。做完去掉 moved_from
fn finish_move(t: &Team, a: &Dict) -> R<String> {
    let Some(from) = a.dict("moved_from") else { return Ok(String::new()) };
    let pins: Vec<String> = from.list("pins").map(|p| p.strs().iter().map(|s| s.to_string()).collect()).unwrap_or_default();
    let old = Team { url: from.at("url").py_str(), pins, ..t.clone() };
    let note = if old.url == t.url || !old.attached() {
        String::new()
    } else {
        forward(&old, t, a, &from.list("pending").unwrap_or_default())?
    };
    a.pop("moved_from");
    admin_save(&t.id, a)?;
    Ok(note)
}

fn forward(old: &Team, t: &Team, a: &Dict, pending: &List) -> R<String> {
    let cfg = match admin_cfg(old)? {
        Ok(c) => c,
        Err(why) => return Ok(why),
    };
    for dev in pending.strs() {
        let (code, j) = presence_post(&cfg, "team_revoke", &dict! { "dev" => &*dev }, 15)?;
        if code != 200 && code != 404 {
            return Ok(admin_fault(old, code, &j).1);
        }
    }
    let (master, sign) = (a.at("master").py_str(), signer(t)?);
    let mut envs: Vec<(String, Dict)> = Vec::new();
    for (dev, d) in a.dict("devices").unwrap_or_default().items() {
        let Some(d) = d.as_dict().filter(|d| d.str("state").as_deref() == Some("joined")) else { continue };
        let (n, seq) = (d.get("n").and_then(|n| n.as_int()).unwrap_or(0), d.get("seq").and_then(|s| s.as_int()).unwrap_or(0));
        let Some(parts) = invite_parts(&device_secret(&master, n)).filter(|_| (1..SEQ_MAX).contains(&seq)) else { continue };
        if let Ok(env) = envelope_seal(&sign, &parts.keys, &t.id, &dev, seq, &build_doc(t, a, &dev, &d))? {
            envs.push((dev.to_string(), env));
        }
    }
    Ok(match put_docs(&cfg, &envs)?.1 {
        Some((code, j)) => admin_fault(old, code, &j).1,
        None => String::new(),
    })
}

// --- 和服务器对一遍 -----------------------------------------------------------------------------------------------------------

/// 向服务器要名单，和 admin.json 对一遍。服务器说的都没有签名，所以只拿它当线索：
/// - 设备的公钥：第一次见到时要过认证码（设备加入时拿邀请里的 k 算的 proof：服务器不知道 k，换不了公钥）才记下；记下以后服务器
///   再报别的公钥不照它的。管理员自己这台的是本机的。没过认证码的（旧版本、照文档自己实现又没带 proof 的）照样发布配置，只是不记公钥
/// - 服务器的记录换了编号（记录没了、换了一台服务器）：照**记下的**公钥把加入过的设备登记回去，配置都重新发布一次
/// - 加入了的设备 invited → joined；邀请不在了、设备也没加入的记成 expired；编号没变、服务器上却没有了的是它自己退出了：不登记回去，
///   这边也不记了（3.6.5 及以前记成 left、移除了的记成 revoked：这样的也删掉，只有 left 的又出现在服务器上时恢复成 joined）；
///   服务器上存着的那一版比发布过的旧就重新发布
///
/// 返回 {"gen", "by_dev": {设备编号: 服务器的那一行}, "pending": {设备编号: 邀请的那一行}, "problems": {设备编号: 要告诉管理员的}}；
/// 没成是给人看的原因。改了 admin.json
fn reconcile(t: &Team, a: &Dict) -> R<Result<Dict, String>> {
    let cfg = match admin_cfg(t)? {
        Ok(c) => c,
        Err(why) => return Ok(Err(why)),
    };
    let devs = a.dict("devices").unwrap_or_default();
    let master = a.at("master").py_str();
    let mut rows = match members(t, &cfg)? {
        Ok(r) => r,
        Err(why) => return Ok(Err(why)),
    };
    let moved = a.str("gen").is_some_and(|g| rows.str("gen").as_deref() != Some(&*g));
    if moved {
        let known = rows.dict("by_dev").unwrap_or_default();
        let lost: Vec<(String, Dict)> = devs
            .items()
            .iter()
            .filter_map(|(dev, d)| Some((dev.to_string(), d.as_dict()?)))
            .filter(|(dev, d)| {
                d.str("state").as_deref() == Some("joined")
                    && d.str("pub").is_some_and(|p| root_line(&p).is_some())
                    && !known.has(dev)
            })
            .collect();
        for chunk in lost.chunks(BATCH) {
            let payload = Dict::new();
            for (dev, d) in chunk {
                let name = d.str("name").filter(|n| name_ok(n)).or_else(|| d.str("label").filter(|n| name_ok(n)));
                payload.set(
                    dev.as_str(),
                    dict! { "pub" => d.at("pub"), "name" => name.map_or("device".to_string(), |n| n.to_string()) },
                );
            }
            let (code, j) = presence_post(&cfg, "team_enroll", &dict! { "devices" => payload }, 20)?;
            if code != 200 {
                return Ok(Err(admin_fault(t, code, &j).1));
            }
        }
        for (_, d) in devs.items() {
            if let Some(d) = d.as_dict() {
                d.pop("sha"); // 服务器上存的配置也没了：都重新发布
            }
        }
        rows = match members(t, &cfg)? {
            Ok(r) => r,
            Err(why) => return Ok(Err(why)),
        };
    }
    if let Some(g) = rows.str("gen").filter(|g| hex32(g)) {
        a.set("gen", &*g);
    }
    let (by_dev, pending, problems) =
        (rows.dict("by_dev").unwrap_or_default(), rows.dict("pending").unwrap_or_default(), Dict::new());
    let older = |r: &Dict, d: &Dict| {
        r.get("seq").and_then(|s| s.as_int()).unwrap_or(0) < d.get("seq").and_then(|s| s.as_int()).unwrap_or(0)
    };
    let mut gone: Vec<String> = Vec::new(); // 不再记着的：退出了的、移除了的
    for (dev, d) in devs.items() {
        let Some(d) = d.as_dict() else { continue };
        let state = d.str("state").map(|s| s.to_string()).unwrap_or_default();
        if state == "revoked" {
            gone.push(dev.to_string());
            continue;
        }
        if let Some(r) = by_dev.dict(&dev) {
            let reported = r.str("pub").and_then(|p| root_line(&p));
            match d.str("pub") {
                Some(known) if reported.as_deref() != Some(&*known) => {
                    problems.set(
                        &*dev,
                        "服务器报的公钥和这台设备加入时的不一样（服务器被人动过？）：不照它的；要紧的话移除这台设备、重新邀请",
                    );
                }
                Some(_) => {}
                None if *dev == *t.dev => {} // 管理员自己这台：公钥是本机的（接上服务器时记下）
                // 这个邀请是给某一把公钥的（cca team invite --key 指定的），服务器报的却是别的：邀请被别的机器用了。它已经
                // 取走的那一份收不回来，以后不给它账号（build_doc 看 mismatch）。服务器没报公钥不算；后来报的对上了就不再这样记
                // （服务器一时说错，不该把一台好好的设备的账号永久拿掉）
                None if d.str("expect").is_some_and(|e| reported.as_deref().is_some_and(|p| p != &*e)) => {
                    d.set("mismatch", true);
                    problems.set(
                        &*dev,
                        format!(
                            "拿这个邀请加入的不是指定的那把密钥（邀请被别的机器用了）：不给它账号；移除它（cca team revoke {} {}），\
                             再给那台机器发一个新的邀请（cca team invite … --key 它的公钥）",
                            &t.id[..8],
                            &dev[..8.min(dev.len())]
                        ),
                    );
                }
                None => {
                    d.pop("mismatch");
                    let secret = device_secret(&master, d.get("n").and_then(|n| n.as_int()).unwrap_or(0));
                    let proved = reported
                        .as_ref()
                        .is_some_and(|p| r.str("proof").is_some_and(|x| pub_proof(&secret, p).as_deref() == Some(&*x)));
                    if proved {
                        d.set("pub", reported.expect("刚核对过有"));
                    } else {
                        problems.set(&*dev, "它加入时的公钥没有带对认证码（旧版本的 cca，或者服务器换了它的公钥）：配置照样发布给它，但服务器的记录没了以后登记不回去");
                    }
                }
            }
            if let Some(n) = r.str("name").filter(|n| name_ok(n)) {
                d.set("name", &*n);
            }
            if matches!(state.as_str(), "invited" | "local" | "expired" | "left") {
                d.set("state", "joined");
            }
            if older(&r, &d) {
                d.pop("sha"); // 服务器上的那一版比发布过的旧（它的记录回滚了、丢了）：重新发布
            }
        } else if let Some(r) = pending.dict(&dev) {
            if older(&r, &d) {
                d.pop("sha");
            }
        } else if state == "invited" {
            d.set("state", "expired");
        } else if matches!(state.as_str(), "joined" | "left") && *dev != *t.dev {
            // 服务器上没有它了：它自己退出了（记录没了的那种，上面已经登记回去）。管理员自己这台不会退出，照旧记着
            gone.push(dev.to_string());
        }
    }
    for dev in &gone {
        devs.pop(dev);
    }
    align_names(t, &cfg, &devs, &by_dev, &problems)?;
    admin_save(&t.id, a)?;
    rows.set("problems", problems);
    Ok(Ok(rows))
}

/// 这个 Team 里还在的别的设备（except 以外）已经叫这个名字（管理员起的，或者它在服务器上的）：发邀请、改名字时不行，不然按名字就分不清
fn name_taken(a: &Dict, except: &str, name: &str) -> bool {
    a.dict("devices").unwrap_or_default().items().iter().any(|(dev, d)| {
        **dev != *except
            && d.as_dict().is_some_and(|d| {
                matches!(d.str("state").as_deref(), Some("local" | "invited" | "joined"))
                    && (d.str("label").as_deref() == Some(name) || d.str("name").as_deref() == Some(name))
            })
    })
}

/// 设备只有一个名字：管理员起的（label）。加入了的设备在服务器上的名字（它加入时报的：旧版本发的邀请里没有名字，它报的是主机名；
/// 或者加入时写了 --name）不一样时，照管理员起的再登记一次（同 cca team rename：同一把公钥、新名字），它下次同步配置时跟着改。
/// 管理员起的不合规、这个 Team 里还在的别的设备用着（管理员给它起的，或者不跟着改的那些在服务器上的名字）时反过来，这边记着的换成
/// 服务器上的；服务器给改成了别的（服务器上有这边没记着的设备叫这个名字时加了 -2）也照服务器的记。公钥对不上的（problems 里有它的）不碰。
/// 没登记成不耽误别的：原因记进 problems，下次对账再试。管理员自己这台不在这里管（它的名字照 team.json 的 device，接上服务器、改名时都登记）
fn align_names(t: &Team, cfg: &Dict, devs: &Dict, by_dev: &Dict, problems: &Dict) -> R<()> {
    let still = |d: &Dict| matches!(d.str("state").as_deref(), Some("local" | "invited" | "joined"));
    let mut todo: Vec<(String, Dict, String)> = Vec::new(); // (设备编号, 那一项, 公钥)
    for (dev, d) in devs.items() {
        let Some(d) = d.as_dict() else { continue };
        if *dev == *t.dev || d.str("state").as_deref() != Some("joined") || problems.has(&dev) {
            continue;
        }
        if d.str("name").is_none() || d.str("name") == d.str("label") {
            continue;
        }
        let reported = by_dev.dict(&dev).and_then(|r| r.str("pub")).and_then(|p| root_line(&p));
        let key = match (d.str("pub").map(|p| p.to_string()), reported) {
            (Some(k), Some(r)) if k == r => k,
            (None, Some(r)) => r,
            _ => continue,
        };
        todo.push((dev.to_string(), d, key));
    }
    let moving: HashSet<String> = todo.iter().map(|(dev, _, _)| dev.clone()).collect();
    let held = |dev: &str, want: &str| {
        devs.items().iter().any(|(o, x)| {
            **o != *dev
                && x.as_dict().is_some_and(|x| {
                    still(&x)
                        && (x.str("label").as_deref() == Some(want)
                            || (!moving.contains(&**o) && x.str("name").as_deref() == Some(want)))
                })
        })
    };
    let mut keep: Vec<(String, Dict, String)> = Vec::new();
    for (dev, d, key) in todo {
        let label = d.str("label").map(|l| l.to_string()).unwrap_or_default();
        if name_ok(&label) && !held(&dev, &label) {
            keep.push((dev, d, key));
        } else {
            d.set("label", d.at("name"));
        }
    }
    // 先登记要让出名字的（它现在的名字是别的设备要的）：服务器照先后查重名
    let wanted: HashSet<String> = keep.iter().filter_map(|(_, d, _)| d.str("label").map(|l| l.to_string())).collect();
    keep.sort_by_key(|(_, d, _)| !d.str("name").is_some_and(|n| wanted.contains(&*n)));
    for chunk in keep.chunks(BATCH) {
        let payload = Dict::new();
        for (dev, d, key) in chunk {
            payload.set(dev.as_str(), dict! { "pub" => key.as_str(), "name" => d.at("label") });
        }
        let (code, j) = presence_post(cfg, "team_enroll", &dict! { "devices" => payload }, 20)?;
        if code != 200 {
            let why = admin_fault(t, code, &j).1;
            for (dev, d, _) in chunk {
                problems.set(dev.as_str(), format!("服务器上的名字没改成「{}」（{why}）：下次对账再试", d.at("label").py_str()));
            }
            continue;
        }
        for (dev, d, _) in chunk {
            if let Some(done) = j.dict("names").and_then(|n| n.str(dev)).filter(|n| name_ok(n)) {
                d.set("name", &*done);
                d.set("label", &*done);
            }
        }
    }
    Ok(())
}

/// team_members 的回复整理成 {"gen", "by_dev": {设备编号: 那一行}, "pending": {设备编号: 邀请的那一行}}
fn members(t: &Team, cfg: &Dict) -> R<Result<Dict, String>> {
    let (code, j) = presence_post(cfg, "team_members", &dict! {}, 15)?;
    if code != 200 {
        return Ok(Err(admin_fault(t, code, &j).1));
    }
    let (by_dev, pending) = (Dict::new(), Dict::new());
    for r in j.list("members").unwrap_or_default().items().iter().filter_map(|r| r.as_dict()) {
        if let Some(dev) = r.str("dev") {
            by_dev.set(&*dev, r.copy());
        }
    }
    for r in j.list("invites").unwrap_or_default().items().iter().filter_map(|r| r.as_dict()) {
        if let Some(dev) = r.str("dev") {
            pending.set(&*dev, r.copy());
        }
    }
    Ok(Ok(dict! { "gen" => j.at("gen"), "by_dev" => by_dev, "pending" => pending }))
}

// --- 子命令 ----------------------------------------------------------------------------------------------------------------

/// 找到 Team、拿着它的锁读出管理员的状态（整条命令都拿着：读、联网、写回这一段里别的命令插不进来——不然两个邀请会用同一个序数）；
/// 没有这个 Team、这台设备不是管理员就 fail
fn admin_of(sel: &str) -> R<(Team, Dict, std::fs::File)> {
    let t = match find_team(Some(sel)) {
        Ok(t) => t,
        Err(why) => return fail("no_team", &why),
    };
    if !(t.role == Role::Admin || t.coadmin()) {
        return fail("not_admin", &admin_load(&t).err().unwrap_or_default());
    }
    let lock = team_lock(&t.id)?;
    // 等锁的时候别的命令可能改过：拿到以后现读
    let reload = |id: &str| load_team(id).ok_or_else(|| Fail::Die("这个 Team 在本机的记录不见了".into()));
    let mut t = reload(&t.id)?;
    if t.coadmin() {
        // 别的管理员：先同步一次自己的配置（管理员的密钥换了、身份被收回了都在这里知道）
        sync_locked(&t)?;
        t = reload(&t.id)?;
        if !t.coadmin() {
            return fail("not_admin", &format!("这台设备在「{}」里的管理员身份被收回了", t.name));
        }
    }
    if let Err(why) = shared_pull(&t)? {
        return fail("server", &why);
    }
    match admin_load(&t) {
        Ok(a) => Ok((t, a, lock)),
        Err(why) => fail("not_admin", &why),
    }
}

/// 先和服务器对一遍（顺带记下新加入的设备的公钥：服务器的记录没了时照它登记回去）再发布，返回存上了几台；没成是 fail
fn settle(t: &Team, a: &Dict, only: Option<&str>) -> R<usize> {
    if let Err(why) = reconcile(t, a)? {
        return fail("server", &why);
    }
    match publish(t, a, only)? {
        Ok(p) if p.problems.is_empty() => Ok(p.stored),
        Ok(p) => fail("publish", &p.problems.join("；")),
        Err(why) => fail("publish", &why),
    }
}

/// 这台设备上能放进 Team 的一个官方账号（team_picks）
struct Pick {
    tool: &'static str,
    label: &'static str,
    email: String,
    /// 这台设备的账号表里的别名（没起过是空的）
    alias: String,
    node: String,
    /// 哪个 Team 给的（这台设备是它的管理员）：它的名字；自己加的是 None
    from: Option<String>,
    /// 那个 Team 带着的、这个账号的节点的定义（needed_nodes：连同经 dialer-proxy 连着的）；没有是 null
    defs: V,
}

/// 这台设备上能放进 Team 的官方账号（cca team new 选的、cca team import --mine 放的）：账号表里有邮箱、有节点的官方账号——自己加的
/// （不是谁给的），或者这台设备当管理员的 Team 给的（账号和节点的定义本来就归这台设备的人管）。这台设备只是成员的 Team 给的不算
/// （是别人的，不能转手给别的设备），第三方 API 账号也不算（Team 的配置里只有官方账号）。照工具的先后、各工具账号表里的先后
fn team_picks() -> Vec<Pick> {
    let admins: HashMap<String, (String, Option<Dict>)> = teams()
        .into_iter()
        .filter_map(|t| {
            // 别的管理员（3.6.15 起）不算：那个 Team 的账号不是这台设备的人的
            let nodes = admin_load(&t).ok().filter(|_| t.role == Role::Admin)?.dict("config").and_then(|c| c.dict("nodes"));
            Some((t.id, (t.name, nodes)))
        })
        .collect();
    let mut out = Vec::new();
    for tool in tools().iter() {
        let Some(table) = load_quiet(&tool.table).and_then(|j| j.as_dict()) else { continue };
        let src = policy_sources(tool);
        for (k, v) in table.items() {
            let (alias, email) = parse_key(&k);
            let (email, node) = (email.to_lowercase(), node_of(&v));
            if node.is_empty() {
                continue;
            }
            let (from, defs) = match src.str(&email) {
                _ if own_account(&src, &k, &v) => (None, V::None),
                // 这台设备只是成员的 Team 给的：不能放；第三方 API 账号、没有邮箱的也不能
                Some(tid) if !is_api(&v) => match admins.get(&*tid) {
                    Some((name, nodes)) => (Some(name.clone()), needed_nodes(nodes.clone(), &BTreeSet::from([node.to_string()]))),
                    None => continue,
                },
                _ => continue,
            };
            out.push(Pick { tool: tool.name, label: tool.label, email, alias, node: node.to_string(), from, defs });
        }
    }
    out
}

/// 把这几个账号连同节点、别名放进这个 Team（名字是 team）的配置（admin.json 的 config，没存盘；之后要 default_aliases），同一个账号照这台
/// 设备上的换。这台设备上起过的别名带进去：Team 里原来是默认编的（或者还没有）、这个工具里也没有别的账号叫它时；默认样子的不带（以前的
/// team-N、这个 Team 或者给这个账号的那个 Team 编的「名字-N」：是 Team 编的号，这里另编）。别的 Team 给的账号，那个 Team 带着它的节点的定义时一起带过来（别的设备照这一份连：它们不一定有这个节点），
/// 这个 Team 已经有了同名的节点的不换。节点的定义不合格（fill_problems）时什么都不改，Err 是给人看的原因
fn put_accounts(a: &Dict, team: &str, picks: &[Pick]) -> Result<(), String> {
    let config = a.dict("config").unwrap_or_default();
    let nodes = config.dict("nodes");
    let mut px: Vec<Dict> = nodes
        .as_ref()
        .and_then(|n| n.list("proxies"))
        .map(|l| l.items().iter().filter_map(|p| p.as_dict()).collect())
        .unwrap_or_default();
    let mut names: HashSet<String> = px.iter().filter_map(|p| p.str("name").map(|n| n.to_string())).collect();
    let mut group = nodes.as_ref().map(|n| n.at("group"));
    let before = px.len();
    for defs in picks.iter().filter_map(|p| p.defs.as_dict()) {
        group.get_or_insert_with(|| defs.at("group"));
        for p in defs.list("proxies").map(|l| l.items()).unwrap_or_default().iter().filter_map(|p| p.as_dict()) {
            if p.str("name").is_some_and(|n| names.insert(n.to_string())) {
                px.push(p);
            }
        }
    }
    if px.len() > before {
        let probe = dict! { "config" => dict! { "nodes" => dict! { "group" => group.unwrap_or(V::None) } } };
        nodes::set_proxies(&probe, px)?;
        config.set("nodes", probe.dict("config").unwrap_or_default().at("nodes"));
    }
    let (accounts, by_tool) = (config.dict("accounts").unwrap_or_default(), config.dict("tool_aliases").unwrap_or_default());
    for p in picks {
        let (m, names) = (accounts.dict(p.tool).unwrap_or_default(), by_tool.dict(p.tool).unwrap_or_default());
        m.set(p.email.as_str(), p.node.as_str());
        let replaceable = names.str(&p.email).is_none_or(|x| team_default_n(&x, team).is_some());
        if team_default_n(&p.alias, team).is_none()
            && p.from.as_deref().is_none_or(|f| team_default_n(&p.alias, f).is_none())
            && alias_ok(&V::from(p.alias.as_str()))
            && replaceable
            && alias_holder(&names, &p.email, &p.alias).is_none()
        {
            names.set(p.email.as_str(), p.alias.as_str());
        }
        accounts.set(p.tool, m);
        by_tool.set(p.tool, names);
    }
    config.set("accounts", accounts);
    config.set("tool_aliases", by_tool);
    a.set("config", config);
    Ok(())
}

/// py team-candidates：这台设备上能放进 Team 的官方账号（team_picks），一行一个：工具、工具的名字、邮箱、别名、已经在哪个 Team 里
/// （这台设备当管理员的那个，它的名字）、节点，制表符分开，没有的栏写 -（bash 按制表符读，空的栏会被并掉）。cca team new 问放进哪些账号时用
pub fn cmd_team_candidates(args: &[String]) -> R<()> {
    arity("team-candidates", args, 0, 0)?;
    let col = |s: &str| if s.is_empty() { "-".to_string() } else { s.replace(['\t', '\n', '\r'], " ") };
    for p in team_picks() {
        out!("{}\t{}\t{}\t{}\t{}\t{}", p.tool, p.label, p.email, col(&p.alias), col(p.from.as_deref().unwrap_or("")), col(&p.node));
    }
    Ok(())
}

/// --accounts 写的（grants_spec 的结果：all，或者 {工具: [邮箱…]}）是 picks 里的哪几个，照 picks 的先后；有不在里面的是给人看的原因
fn chosen_picks(picks: Vec<Pick>, want: &V) -> Result<Vec<Pick>, String> {
    let Some(want) = want.as_dict() else { return Ok(picks) }; // all
    let mut keep = vec![false; picks.len()];
    for (tool, emails) in want.items() {
        for email in emails.as_list().map(|l| l.strs()).unwrap_or_default() {
            match picks.iter().position(|p| *p.tool == *tool && *p.email == *email) {
                Some(i) => keep[i] = true,
                None => {
                    return Err(format!(
                        "这台设备上没有 {tool}:{email} 这个账号，或者它不能放进 Team（第三方 API 账号、这台设备不是管理员的 Team 给的都不能放）"
                    ))
                }
            }
        }
    }
    Ok(picks.into_iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| p).collect())
}

/// py team-new 名字 [--device 这台设备的名字] [--accounts all|none|工具:邮箱,…]：在这台设备上建一个 Team（还没有服务器）：生成根密钥、
/// 主密钥和盐，这台设备是管理员、也是第一台设备。--accounts：这台设备上的哪些账号连同节点、别名放进去（team-candidates 列出的那些，
/// all 是全部，见 put_accounts）；不写是一个都不放。stdout：Team 的编号、根公钥（部署服务器时的 --team-root）、放进了几个账号。
/// 原因码：name、usage、accounts（写的账号这台设备上没有、不能放，带过来的节点的定义不合格）
pub fn cmd_team_new(args: &[String]) -> R<()> {
    const USAGE: &str = "用法: py team-new 名字 [--device 这台设备的名字] [--accounts all|none|工具:邮箱,…]";
    let Some((name, rest)) = args.split_first() else { return Err(Fail::Die(USAGE.into())) };
    let (name, mut device, mut spec) = (name.trim().to_string(), host_name(), None);
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        match (flag.as_str(), it.next()) {
            ("--device", Some(d)) => device = d.trim().to_string(),
            ("--accounts", Some(s)) => spec = Some(s.as_str()),
            _ => return Err(Fail::Die(USAGE.into())),
        }
    }
    if !team_name_ok(&name) {
        return fail("name", "Team 的名字要 1～64 个字符，不能有控制字符");
    }
    if !name_ok(&device) {
        return fail("name", NAME_BAD);
    }
    // 放进的账号先算好：写错了、节点的定义不合格时什么都还没建
    let config = dict! {
        "accounts" => Dict::new(), "tool_aliases" => Dict::new(), "aliases" => Dict::new(), "install_node" => "", "nodes" => V::None,
    };
    let held = dict! { "config" => config };
    let chosen = match spec.map(grants_spec) {
        None => Vec::new(),
        Some(None) => return fail("usage", "--accounts 写成 all、none，或者 工具:邮箱,工具:邮箱（工具是 claude、codex、agy）"),
        Some(Some(want)) => match chosen_picks(team_picks(), &want) {
            Ok(chosen) => chosen,
            Err(why) => return fail("accounts", &why),
        },
    };
    if !chosen.is_empty() {
        if let Err(why) = put_accounts(&held, &name, &chosen) {
            return fail("accounts", &format!("带过来的节点的定义不对：{why}"));
        }
        default_aliases(&held, &name);
    }
    fs::makedirs_mode(&TEAMS_DIR, 0o700)?;
    // 以前建到一半被打断留下的（里面有根密钥）：过了一个小时的清掉
    for e in std::fs::read_dir(&*TEAMS_DIR).into_iter().flatten().flatten() {
        let old =
            e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|m| m.elapsed().ok()).is_some_and(|d| d.as_secs() > 3600);
        if old && e.file_name().to_string_lossy().starts_with(".new.") {
            let _ = fs::rmtree(&e.path().to_string_lossy());
        }
    }
    // 先在旁边建好再改名：编号要有了根公钥才知道
    let tmp = path::join(&TEAMS_DIR, &format!(".new.{}", std::process::id()));
    if path::isdir(&tmp) {
        fs::rmtree(&tmp)?;
    }
    fs::makedirs_mode(&tmp, 0o700)?;
    let made = (|| -> R<Option<String>> {
        let root_key = path::join(&tmp, "root_ed25519");
        keygen(&root_key, "cca-team-root")?;
        keygen(&path::join(&tmp, "id_ed25519"), "cca-team")?;
        if key_at(&root_key)?.is_none() || key_at(&path::join(&tmp, "id_ed25519"))?.is_none() {
            return Ok(None);
        }
        let Some(root) = root_line(&pub_line(&root_key)?) else { return Ok(None) };
        let Some(tid) = team_id(&root) else { return Ok(None) };
        let master = token_hex(32);
        let secret = device_secret(&master, 0);
        let dev = invite_parts(&secret).expect("刚算出来的是 32 个十六进制字符").dev;
        fs::put(&path::join(&tmp, "secret"), format!("{secret}\n").as_bytes(), 0o600)?;
        let team = dict! {
            "v" => 1, "id" => tid.as_str(), "name" => name.as_str(), "url" => "", "pins" => List::new(), "root" => root.as_str(),
            "role" => Role::Admin.name(), "dev" => dev.as_str(), "device" => device.as_str(), "joined_at" => now(),
        };
        save_json(&path::join(&tmp, "team.json"), &V::Dict(team), Some(0o600), true)?;
        let me = dict! { "n" => 0, "label" => device.as_str(), "state" => "local", "grants" => "all", "seq" => 0 };
        let admin = dict! {
            "v" => 1, "master" => master, "salt" => token_hex(16), "next" => 1, "config" => held.at("config"),
            "devices" => dict! { dev.as_str() => me },
        };
        if let Some(n) = held.get("alias_n") {
            admin.set("alias_n", n);
        }
        save_json(&path::join(&tmp, "admin.json"), &V::Dict(admin), Some(0o600), true)?;
        Ok(Some(tid))
    })();
    let tid = match made {
        Ok(Some(tid)) => tid,
        other => {
            let _ = fs::rmtree(&tmp);
            other?;
            return fail("sign", "生成不了 Team 的密钥（ssh-keygen）");
        }
    };
    std::fs::rename(&tmp, team_dir(&tid))?;
    index_add(&tid)?;
    let root = load_team(&tid).map(|t| t.root).unwrap_or_default();
    out!("{tid}\n{root}\n{}", chosen.len());
    Ok(())
}

/// py team-attach Team https://服务器:端口 [证书公钥的指纹…]：接上（换一台）同步服务器（没写端口是 443：托管平台给的地址）。服务器要已经用
/// 这个 Team 的根公钥运行着（--team-root）。把这台设备登记上去，别的加入过的设备也登记回去，再发布各台的配置；换了一台的话，新的配置也存到原来那台上
/// （还连着它的设备取到以后自己换过来）。stdout：服务器的地址:端口、这台设备在服务器上的名字、现在几台设备、存上了几台的配置、
/// 换服务器时原来那台没存上的原因（存上了、不是换服务器是空行）。原因码：no_team、not_admin、url、pin、denied（服务器配置的不是这个 Team 的根公钥）、
/// old、unreachable、cert、clock、busy、server、publish
pub fn cmd_team_attach(args: &[String]) -> R<()> {
    let [sel, url, pins @ ..] = args else {
        return Err(Fail::Die("用法: py team-attach Team https://服务器:端口 [证书公钥的指纹…]".into()));
    };
    let (t, a, _lk) = admin_of(sel)?;
    owner_only(&t, "换同步服务器")?;
    let url = with_https_port(url);
    if !url_ok(&url) {
        return fail("url", "服务器的地址要写成 https://地址:端口（托管平台给的 https://名字.平台的域名 不用写端口）");
    }
    if pins.len() > 4 || !pins.iter().all(|p| PRESENCE_PIN_RE.fullmatch(p).is_some()) {
        return fail("pin", "证书公钥的指纹要写成 sha256//…（最多 4 个）");
    }
    let (tid, moving) = (t.id.clone(), t.attached() && t.url != url);
    match attach(t, &a, url, pins)? {
        Ok(x) => {
            // 从接着的一台换到了别的地址：新的那台不知道是怎么装的（手动、托管平台），删除 Team 时不再照原来记下的那台去卸载（没接上的
            // 话还连着原来那台：照旧）。原来没接上的（cca team deploy 装好了、没接上，再手动接上）照旧：多半接的就是装好的那台
            if moving && a.pop("deploy").is_some() {
                admin_save(&tid, &a)?;
            }
            out!("{}\n{}\n{}\n{}\n{}", x.host, x.device, x.members, x.stored, x.note);
            Ok(())
        }
        Err((c, why)) => fail(&c, &why),
    }
}

/// 接上了以后要说的：服务器的地址:端口、这台设备在服务器上的名字、现在几台设备、存上了几台的配置、换服务器时原来那台没存上的原因
struct Attached {
    host: String,
    device: String,
    members: usize,
    stored: usize,
    note: String,
}

/// 接上（换一台）同步服务器（cmd_team_attach、cmd_team_deploy 共用；地址和指纹调用方核对过样子）：把这台设备登记上去，别的加入过的
/// 设备也登记回去，再发布各台的配置；换了一台的话新的配置也存到原来那台上。服务器不认（没连上、配置的不是这个 Team 的根公钥……）
/// 是（原因码, 原因）
fn attach(t: Team, a: &Dict, url: String, pins: &[String]) -> R<Result<Attached, (String, String)>> {
    let devs = a.dict("devices").unwrap_or_default();
    // 换一台：记下原来是哪台、哪些邀请还没用掉（写的是它的地址），新的那台弄好以后去原来那台上收尾（finish_move）。
    // 上一次换到一半没做完的照旧记着最早的那台
    if t.attached() && t.url != url && !a.has("moved_from") {
        let pending: Vec<String> = devs
            .items()
            .iter()
            .filter(|(_, d)| d.as_dict().is_some_and(|d| d.str("state").as_deref() == Some("invited")))
            .map(|(dev, _)| dev.to_string())
            .collect();
        a.set("moved_from", dict! { "url" => t.url.as_str(), "pins" => str_list(&t.pins), "pending" => str_list(&pending) });
        admin_save(&t.id, a)?;
    }
    let raw = t.raw.copy();
    raw.set("url", url.as_str());
    raw.set("pins", str_list(pins));
    let t = Team { url, pins: pins.to_vec(), raw: raw.copy(), ..t };
    let cfg = match admin_cfg(&t)? {
        Ok(c) => c,
        Err(why) => return Ok(Err(("key".into(), why))),
    };
    // 先登记这台设备：服务器认不认这把根密钥、是不是 Team 的服务器，这一下就知道。认了才把新的地址写进 team.json
    let me = devs.dict(&t.dev).unwrap_or_default();
    let my_pub = pub_line(&t.path("id_ed25519"))?;
    let payload = dict! { t.dev.as_str() => dict! { "pub" => my_pub.as_str(), "name" => t.device.as_str() } };
    let (code, j) = presence_post(&cfg, "team_enroll", &dict! { "devices" => payload }, 20)?;
    if code != 200 {
        if !load_team(&t.id).is_some_and(|cur| cur.attached()) {
            a.pop("moved_from"); // 第一次接就没接上：没有原来那台
            admin_save(&t.id, a)?;
        }
        let (c, why) = admin_fault(&t, code, &j);
        return Ok(Err((c.to_string(), why)));
    }
    let device = j.dict("names").and_then(|n| n.str(&t.dev)).filter(|n| name_ok(n)).map_or(t.device.clone(), |n| n.to_string());
    raw.set("device", device.as_str());
    save_team(&t.id, &raw)?;
    // 给过别的设备管理员身份的（3.6.15 起）：管理员名单、共用的状态也放到这一台上（换了一台的话它上面还没有；从头存一份）
    if a.get("admins_epoch").and_then(|e| e.as_int()).is_some_and(|e| e > 0) {
        if let Some(cur) = load_team(&t.id) {
            if let Err((c, why)) = admins_post(&cur, a)? {
                return Ok(Err((c.to_string(), why)));
            }
        }
        // 共用的状态：照这一台上的版本号接着存（换了一台的话它上面还没有，是 0），下面的 admin_save 就存上去了
        if let Some(cfg) = load_team(&t.id).and_then(|cur| admin_cfg(&cur).ok()?.ok()) {
            let (code, j) = presence_post(&cfg, "team_astate_get", &dict! {}, 20)?;
            let rev = j.get("rev").and_then(|r| r.as_int()).filter(|_| code == 200);
            if let Some(rev) = rev.filter(|r| a.get("arev").and_then(|x| x.as_int()) != Some(*r)) {
                a.set("arev", rev);
                a.pop("asha");
            }
        }
    }
    me.set("state", "joined");
    me.set("name", device.as_str());
    me.set("pub", my_pub);
    devs.set(t.dev.as_str(), me);
    admin_save(&t.id, a)?;
    let t = load_team(&t.id).ok_or_else(|| Fail::Die("刚写好的 team.json 读不回来".into()))?;
    let stored = settle(&t, a, None)?;
    sync_locked(&t)?;
    let members = devs.values().iter().filter(|d| d.as_dict().is_some_and(|d| d.str("state").as_deref() == Some("joined"))).count();
    let note = finish_move(&t, a)?;
    Ok(Ok(Attached { host: host_of(&t.url), device, members, stored, note }))
}

/// py team-deploy Team [用户@]主机 [hub-deploy 那一套参数，没有 --reset、--name]：经 SSH 把这个 Team 的同步服务器装到一台 Linux
/// 服务器上（一台服务器上每个 Team 一个服务：cca-team-<编号的前 8 位>，装在 /opt/<服务名>；再运行是更新），装好了接上（attach）。
/// --check：只看登录得上不（什么都不装）。stdout 是 sh 的 键=值：mode、state（new | update）、port、relay（中转的数据端口，没开是空的）、
/// dir、lan、host（地址:端口）、device、members、stored、note（换服务器时原来那台没存上的原因）、svc（服务名）、web（手机的网页端的网址，
/// 没开是空的）、web_cert（ok | wait：证书拿到了没有）、web_off（这一次没开网页端的原因：port80 | acme_old | lan | host | web_port；
/// 自己关的是空的）、push（推送网关的地址：会话里的通知经它推到手机上；没用推送是空的）。
/// 原因码：安装那一步的那些（ssh…、root、deps、image、port、taken、start、host、install、usage），加上 no_team、not_admin、key，
/// 装好了没接上的是 attach 的那些
pub fn cmd_team_deploy(args: &[String]) -> R<()> {
    let [sel, rest @ ..] = args else {
        return Err(Fail::Die("用法: py team-deploy Team [用户@]主机 [选项]".into()));
    };
    // 装的那一段经 SSH，网慢时要一会儿（要把约 10 MB 的安装脚本传过去：里面带着服务端程序）：这段时间不拿着这个 Team 的锁（不然这台设备上同步配置、别的管理命令
    // 都卡着等）。装好了再取锁、重新读一遍（等的时候可能被别的命令改过），接上
    let (t, _a, lk) = admin_of(sel)?;
    owner_only(&t, "部署同步服务器")?;
    let (tid, root, svc, name) = (t.id.clone(), t.root.clone(), format!("cca-team-{}", &t.id[..8]), t.name.clone());
    let old = t.attached().then(|| (t.url.clone(), t.pins.clone()));
    drop(lk);
    let Some(inst) = team_install(rest, &root, &svc, &name, old.as_ref().map(|(u, p)| (u.as_str(), p.as_slice())))? else {
        return Ok(());
    };
    let (t, a, _lk) = admin_of(&tid)?;
    // 记下装到了哪台、用哪把私钥登录的（接没接上都是装在那里了）：删除 Team 时照它卸载（undeploy_plan），再部署到这一台时照它登录
    // （bash 的 team_deploy：没写 -i 时传进来的就是记下的那把）。这一次没有私钥的就没有：照 ssh 的配置、密码
    a.set(
        "deploy",
        dict! {
            "target" => inst.target.as_str(),
            "ssh_port" => inst.ssh_port.as_str(),
            "dir" => inst.dir.as_str(),
            "identity" => inst.identity.as_str(),
        },
    );
    admin_save(&tid, &a)?;
    let x = match attach(t, &a, inst.url.clone(), std::slice::from_ref(&inst.pin))? {
        Ok(x) => x,
        Err((c, why)) => {
            let fw = if c == "unreachable" {
                format!("。云服务商的安全组（或者轻量应用服务器的防火墙）里要放行 TCP {}，放行以后再运行一次", inst.port)
            } else {
                String::new()
            };
            return fail(&c, &format!("服务器上装好了（{}），但没接上：{why}{fw}", host_of(&inst.url)));
        }
    };
    let (port, members, stored) = (inst.port.to_string(), x.members.to_string(), x.stored.to_string());
    let relay = if inst.relay > 0 { inst.relay.to_string() } else { String::new() };
    sh(&[
        ("mode", inst.mode.as_str()),
        ("state", inst.state.as_str()),
        ("port", port.as_str()),
        ("relay", relay.as_str()),
        ("dir", inst.dir.as_str()),
        ("lan", if inst.lan { "1" } else { "" }),
        ("host", x.host.as_str()),
        ("device", x.device.as_str()),
        ("members", members.as_str()),
        ("stored", stored.as_str()),
        ("note", x.note.as_str()),
        ("svc", svc.as_str()),
        ("web", inst.web.as_str()),
        ("web_cert", inst.web_cert.as_str()),
        ("web_off", inst.web_off.as_str()),
        ("push", inst.push.as_str()),
    ]);
    Ok(())
}

/// py team-undeploy Team [用户@]主机 [--ssh-port 端口] [--identity 私钥] [--dir 目录] [--batch] [--askpass] [--check]：经 SSH 卸掉这个
/// Team 的同步服务器（deploy.rs 的 team_uninstall；带着根公钥，安装脚本核对目录里装的是这个 Team 的）。删除 Team（管理员 cca team leave）
/// 时 bash 先调它，卸好了才删这台设备上的（team-leave apply --server-gone）。--check：只看登录得上不。stdout：卸掉的目录（--check 时没有）。
/// 原因码：no_team、not_admin、usage、key，SSH 和安装脚本的那些（ssh…、root、deps、taken、install：目录里没装它也是 install）
pub fn cmd_team_undeploy(args: &[String]) -> R<()> {
    let [sel, rest @ ..] = args else {
        return Err(Fail::Die("用法: py team-undeploy Team [用户@]主机 [选项]".into()));
    };
    // 经 SSH 可能要一会儿：不拿着这个 Team 的锁
    let (t, _a, lk) = admin_of(sel)?;
    owner_only(&t, "卸载同步服务器")?;
    let (root, svc) = (t.root.clone(), format!("cca-team-{}", &t.id[..8]));
    drop(lk);
    if let Some(dir) = team_uninstall(rest, &root, &svc)? {
        out!("{dir}");
    }
    Ok(())
}

/// 删除 Team 时去哪卸载它的同步服务器（undeploy_plan，team-leave plan 打给 bash）
pub struct UndeployPlan {
    /// [用户@]主机
    pub target: String,
    /// SSH 端口：空的是照 ssh 的配置
    pub ssh_port: String,
    pub dir: String,
    /// 是部署时记下的（不是照部署的默认猜的）
    pub saved: bool,
    /// 部署时写的私钥（-i）：记下的、文件还在才有（不在了照 ssh 的配置登录）；空的是没写
    pub identity: String,
}

/// 删除 Team 时去哪卸载它的同步服务器（team-leave plan）。cca team deploy 记下的照它（装好了、没接上的也是：服务器上装着），连同部署时
/// 写的私钥（App 里删除时没法写 -i）；没记下的（以前的版本部署的、手动装的）照部署的默认猜：root@服务器的地址、22、
/// /opt/cca-team-<编号的前 8 位>（猜错了卸不成，bash 那边问要不要只删这台设备上的；安装脚本核对目录里装的是这个 Team 的，别的不动）。
/// 证书照证书机构验的（没钉指纹：托管平台、自己配的反向代理）不是安装脚本直接装的，不猜。不是管理员、没有服务器是 None
pub fn undeploy_plan(t: &Team) -> Option<UndeployPlan> {
    let a = admin_load(t).ok()?;
    let def_dir = format!("/opt/cca-team-{}", &t.id[..8]);
    if let Some(d) = a.dict("deploy") {
        if let Some(target) = d.str("target").filter(|x| !x.is_empty()) {
            return Some(UndeployPlan {
                target: target.to_string(),
                ssh_port: d.str("ssh_port").map(|x| x.to_string()).unwrap_or_default(),
                dir: d.str("dir").filter(|x| !x.is_empty()).map_or(def_dir, |x| x.to_string()),
                saved: true,
                identity: d.str("identity").filter(|k| !k.is_empty() && path::isfile(k)).map(|k| k.to_string()).unwrap_or_default(),
            });
        }
    }
    if !t.attached() || t.pins.is_empty() {
        return None;
    }
    let (host, _) = url_parts(&t.url);
    (!host.is_empty()).then(|| UndeployPlan {
        target: format!("root@{host}"),
        ssh_port: "22".into(),
        dir: def_dir,
        saved: false,
        identity: String::new(),
    })
}

/// py team-hosted Team [railway] [--dir 目录]：没有服务器时把这个 Team 的同步服务器部署到托管平台（只有 Railway，平台可以不写）：
/// 生成部署文件（team_files）。这个 Team 已经接着一台照证书机构验的服务器（托管平台上的：没有指纹）时是更新。stdout 是 sh 的 键=值：
/// dir、steps、tid、name、svc。原因码：no_team、not_admin、usage（写了别的平台；sealos 说不再支持）、dir、key、
/// install（这一份 cca 没带 Linux 的服务端程序：payload.rs 说了为什么）
pub fn cmd_team_hosted(args: &[String]) -> R<()> {
    const USAGE: &str = "用法: py team-hosted Team [railway] [--dir 目录]";
    let [sel, rest @ ..] = args else { return Err(Fail::Die(USAGE.into())) };
    let rest = match rest {
        [p, more @ ..] if !p.starts_with('-') => match p.as_str() {
            "railway" => more,
            // Sealos 的表单只能把服务端程序当成文本的配置文件挂进容器：换成编译好的程序以后放不进去（docs/dev/team.md「托管平台」）
            "sealos" => return fail("usage", "不再支持 Sealos：部署到 Railway，或者自己的服务器（cca team deploy）"),
            _ => return fail("usage", "托管平台只有 Railway：写 railway，或者不写"),
        },
        _ => rest,
    };
    let dir = match rest {
        [] => None,
        [flag, d] if flag == "--dir" && !d.is_empty() => Some(d.as_str()),
        _ => return Err(Fail::Die(USAGE.into())),
    };
    let (t, _a, _lk) = admin_of(sel)?;
    owner_only(&t, "部署同步服务器")?;
    let Some(key) = t.root.split(' ').nth(1) else { return fail("key", "这个 Team 的根公钥读不出来") };
    let svc = format!("cca-team-{}", &t.id[..8]);
    let update = t.attached() && t.pins.is_empty();
    match team_files(dir, key, &svc, &t.name, update)? {
        Ok((dir, steps)) => {
            sh(&[
                ("dir", dir.as_str()),
                ("steps", steps.as_str()),
                ("tid", t.id.as_str()),
                ("name", t.name.as_str()),
                ("svc", svc.as_str()),
            ]);
            Ok(())
        }
        Err((c, why)) => fail(&c, &why),
    }
}

/// 命令里写的授权：all | none | 工具:邮箱[,工具:邮箱…]；不对是 None
fn grants_spec(spec: &str) -> Option<V> {
    match spec {
        "all" => Some(V::from("all")),
        "none" | "" => Some(V::Dict(Dict::new())),
        _ => {
            let g = Dict::new();
            for item in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                let (tool, email) = account_spec(item)?;
                let list = g.list(&tool).unwrap_or_default();
                if !list.has_str(&email) {
                    list.push(email);
                }
                g.set(tool, list);
            }
            Some(V::Dict(g))
        }
    }
}

/// py team-invite Team 给这台设备起的名字 [--accounts all|工具:邮箱,…] [--ttl 秒]：给一台设备发邀请（邀请里带着这个名字：它加入时用它）。
/// stdout：邀请链接（只这一次，它是凭证）、设备编号、配置没存上时的原因（存上了是空行）。原因码：no_team、not_admin、usage、
/// name（名字不合规、这个 Team 里还在的别的设备已经叫它）、full（没用掉的邀请太多）、以及请求没成的那些
pub fn cmd_team_invite(args: &[String]) -> R<()> {
    const USAGE: &str = "用法: cca team invite Team 名字 [--accounts all|工具:邮箱,…] [--ttl 秒] [--key '公钥']";
    let [sel, label, rest @ ..] = args else {
        return Err(Fail::Die("用法: py team-invite Team 名字 [--accounts all|工具:邮箱,…] [--ttl 秒] [--key 公钥]".into()));
    };
    let (mut grants, mut ttl, mut expect) = (V::from("all"), INVITE_TTL, None);
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        match (flag.as_str(), it.next()) {
            ("--accounts", Some(s)) => match grants_spec(s) {
                Some(g) => grants = g,
                None => return fail("usage", "--accounts 写成 all，或者 工具:邮箱,工具:邮箱（工具是 claude、codex、agy）"),
            },
            ("--ttl", Some(s)) => match s.parse::<i64>() {
                Ok(n) if (60..=30 * 86400).contains(&n) => ttl = n,
                _ => return fail("usage", "--ttl 是多少秒以后过期：60 秒到 30 天"),
            },
            ("--key", Some(s)) => match root_line(s) {
                Some(k) => expect = Some(k),
                None => return fail("usage", "--key 后面是那台设备要拿来加入的公钥（ssh-ed25519 AAAA…）"),
            },
            _ => return fail("usage", USAGE),
        }
    }
    let label = label.trim().to_string();
    if !name_ok(&label) {
        return fail("name", NAME_BAD);
    }
    let (t, a, _lk) = admin_of(sel)?;
    if name_taken(&a, "", &label) {
        return fail("name", &format!("这个 Team 里已经有一台设备叫「{label}」：换一个名字"));
    }
    let cfg = match admin_cfg(&t)? {
        Ok(c) => c,
        Err(why) => return fail("not_attached", &why),
    };
    let (link, dev) = match issue(&t, &a, &cfg, &label, grants, ttl, expect.as_deref())? {
        Ok(x) => x,
        Err((c, why)) => return fail(&c, &why),
    };
    let problem = match publish(&t, &a, Some(&dev))? {
        Ok(p) => p.problems.join("；"),
        Err(why) => why,
    };
    out!("{link}\n{dev}\n{problem}");
    Ok(())
}

/// 发一个邀请：在服务器上登记、记进 admin.json（还没发布这台设备的配置）。expect：这个邀请是给这把公钥的（对账时核对拿它加入的
/// 是不是这把：reconcile）。返回（邀请链接, 设备编号），没成是（原因码, 原因）
fn issue(
    t: &Team,
    a: &Dict,
    cfg: &Dict,
    label: &str,
    grants: V,
    ttl: i64,
    expect: Option<&str>,
) -> R<Result<(String, String), (String, String)>> {
    let n = a.get("next").and_then(|n| n.as_int()).filter(|n| *n > 0).unwrap_or(1);
    let secret = device_secret(&a.at("master").py_str(), n);
    let parts = invite_parts(&secret).expect("刚算出来的是 32 个十六进制字符");
    a.set("next", n + 1);
    admin_save(&t.id, a)?; // 序数先记下：这一次没成也不再用它
    let payload = dict! { "token" => token_hash(&parts.token), "dev" => parts.dev.as_str(), "ttl" => ttl };
    let (code, j) = presence_post(cfg, "team_invite", &payload, 15)?;
    if code != 200 {
        let (c, why) = match code {
            507 => ("full", "还没用掉的邀请太多了（最多 64 个）：等它们用掉、过期，或者移除几个（cca team revoke）".to_string()),
            _ => admin_fault(t, code, &j),
        };
        return Ok(Err((c.to_string(), why)));
    }
    let exp = now() + ttl;
    let dev = dict! { "n" => n, "label" => label, "state" => "invited", "grants" => grants, "seq" => 0, "exp" => exp };
    if let Some(k) = expect {
        dev.set("expect", k);
    }
    a.dict("devices").unwrap_or_default().set(parts.dev.as_str(), dev);
    admin_save(&t.id, a)?;
    Ok(Ok((invite_link(t, &secret, exp, label), parts.dev)))
}

/// 邀请链接（那串随机数是照主密钥和序数算的，所以发过的链接能原样再算出来）。带着给这台设备起的名字：它加入时用这个名字
fn invite_link(t: &Team, secret: &str, exp: i64, device: &str) -> String {
    invite_encode(&Invite {
        url: t.url.clone(),
        pins: t.pins.clone(),
        root: t.root.clone(),
        secret: secret.to_string(),
        name: t.name.clone(),
        exp: Some(exp),
        device: device.to_string(),
    })
}

/// 授权给人看的样子：全部 / 几个账号
fn grants_text(g: &V) -> String {
    match g.as_dict() {
        None => "全部账号".into(),
        Some(d) => format!("{} 个账号", d.values().iter().filter_map(|l| l.as_list()).map(|l| l.len()).sum::<usize>()),
    }
}

/// py team-devices Team [--json]：这个 Team 的设备（先和服务器对一遍，见 reconcile）。--json：{"version": 1, "kind": "team_devices",
/// "team": 编号, "alias_lock": 别名锁定着没有, "mail_forwarders": 信任的转发中转（没有是 []）, "devices": [{dev, label, name, state, me, grants, seq, have: 它手里是第几版（没说过是 null）,
/// seen_age, expires_epoch: 邀请什么时候过期, problem: 对一遍时发现的、要告诉管理员的（没有是 null）, role: owner（建 Team 的那台）|
/// admin（别的管理员，3.6.15 起）| member}]}；不带时第一行是别名锁定着没有（1 | 0），后面一台一行：设备编号\t名字\t状态\t授权\t
/// 发布到第几版\t它手里第几版\t要告诉管理员的\t角色（没有的写 -）
pub fn cmd_team_devices(args: &[String]) -> R<()> {
    let (sel, as_json) = match args {
        [s] => (s, false),
        [s, f] if f == "--json" => (s, true),
        _ => return Err(Fail::Die("用法: py team-devices Team [--json]".into())),
    };
    let (t, a, _lk) = admin_of(sel)?;
    let rows = match reconcile(&t, &a)? {
        Ok(r) => r,
        Err(why) => return fail("server", &why),
    };
    let (by_dev, problems) = (rows.dict("by_dev").unwrap_or_default(), rows.dict("problems").unwrap_or_default());
    let mut list: Vec<V> = Vec::new();
    for (dev, d) in a.dict("devices").unwrap_or_default().items() {
        let Some(d) = d.as_dict() else { continue };
        let r = by_dev.dict(&dev).unwrap_or_default();
        let state = d.str("state").map(|s| s.to_string()).unwrap_or_default();
        let row = dict! {
            "dev" => &*dev, "label" => d.at("label"), "name" => d.get("name").unwrap_or(V::None), "state" => state.as_str(),
            "me" => *dev == *t.dev, "grants" => d.at("grants"), "seq" => d.get("seq").unwrap_or(V::Int(0)),
            "have" => r.get("have").unwrap_or(V::None), "seen_age" => r.get("seen_age").unwrap_or(V::None),
            "expires_epoch" => if state == "invited" { d.get("exp").filter(is_num).unwrap_or(V::None) } else { V::None },
            "problem" => problems.get(&dev).unwrap_or(V::None),
            "role" => match d.get("n").and_then(|n| n.as_int()) {
                Some(0) => "owner",
                _ if a.dict("admins").is_some_and(|m| m.has(&dev)) => "admin",
                _ => "member",
            },
        };
        list.push(V::Dict(row));
    }
    if as_json {
        // 管理页要的都在这一份里：Team 的名字、配置里的账号 → 节点（管理员定的）、别名（按工具的，和按邮箱的：照 alias_plan 算，
        // 还没发布过的旧 Team 也是按工具编好的样子）、带着定义的节点的名字
        let config = a.dict("config").unwrap_or_default();
        let plan = alias_plan(&a, &t.name);
        let nodes: Vec<V> = config
            .dict("nodes")
            .and_then(|n| n.list("proxies"))
            .map(|l| l.items().iter().filter_map(|p| p.as_dict()?.get("name")).collect())
            .unwrap_or_default();
        let doc = dict! {
            "version" => 1, "kind" => "team_devices", "team" => t.id.as_str(), "name" => t.name.as_str(),
            "accounts" => config.get("accounts").filter(|x| x.is_dict()).unwrap_or_else(|| V::Dict(Dict::new())),
            "aliases" => plan.flat, "tool_aliases" => plan.by_tool, "alias_lock" => alias_locked(&a), "nodes" => nodes,
            "mail_forwarders" => str_list(doc_forwarders(&config)), "devices" => list,
        };
        out!("{}", json::dumps(&V::Dict(doc), json::Fmt::new().utf8().indent(2)));
        return Ok(());
    }
    out!("{}", if alias_locked(&a) { 1 } else { 0 }); // 第一行：别名锁定着没有
    for r in list.iter().filter_map(|r| r.as_dict()) {
        // 没有的写 -：bash 按制表符读，空的栏会被并掉
        let s = |k: &str| r.get(k).filter(|v| !v.is_none()).map_or("-".to_string(), |v| v.py_str());
        let grants = grants_text(&r.at("grants"));
        out!(
            "{}\t{}\t{}\t{grants}\t{}\t{}\t{}\t{}",
            s("dev"),
            s("label"),
            s("state"),
            s("seq"),
            s("have"),
            s("problem"),
            s("role")
        );
    }
    Ok(())
}

/// py team-revoke plan | apply Team 设备：移除一台设备（或者撤回还没用掉的邀请）。plan（只读）：stdout 是「Team 的编号\t设备编号\t给它
/// 起的名字\t状态」（bash 据此问同意）。apply：服务器就此不认它的密钥，给它的配置、它传过的折合金额删掉（它已经拿到的节点凭证收
/// 不回来），这边也不再记着它；stdout：给它起的名字。原因码：no_team、not_admin、no_device、self（管理员移除不了自己这台）、not_attached、以及请求没成的那些
pub fn cmd_team_revoke(args: &[String]) -> R<()> {
    let [mode, sel, device] = args else {
        return Err(Fail::Die("用法: py team-revoke plan | apply Team 设备".into()));
    };
    if !matches!(mode.as_str(), "plan" | "apply") {
        return Err(Fail::Die("用法: py team-revoke plan | apply Team 设备".into()));
    }
    let (t, a, _lk) = admin_of(sel)?;
    let dev = match find_device(&a, device) {
        Ok(d) => d,
        Err(why) => return fail("no_device", &why),
    };
    if dev == t.dev {
        return fail("self", "这是管理员自己的这台设备：移除不了（不要这个 Team 了用 cca team leave）");
    }
    let d = a.dict("devices").and_then(|ds| ds.dict(&dev)).unwrap_or_default();
    // 几位管理员（3.6.15 起）：建 Team 的那台谁也移除不了；别的管理员的设备先收回它的管理员身份（只有建 Team 的设备能）
    if d.get("n").and_then(|n| n.as_int()) == Some(0) {
        return fail("owner", "这是建这个 Team 的设备：移除不了");
    }
    if a.dict("admins").is_some_and(|m| m.has(&dev)) {
        return fail(
            "admin_dev",
            &format!("「{}」是这个 Team 的管理员：先收回它的管理员身份（cca team admin {} remove …）", label_of(&d, &dev), t.name),
        );
    }
    if mode == "plan" {
        out!("{}\t{dev}\t{}\t{}", t.id, d.at("label").py_str(), d.at("state").py_str());
        return Ok(());
    }
    let cfg = match admin_cfg(&t)? {
        Ok(c) => c,
        Err(why) => return fail("not_attached", &why),
    };
    let (code, j) = presence_post(&cfg, "team_revoke", &dict! { "dev" => dev.as_str() }, 15)?;
    if code != 200 && !(code == 404 && j.str("error").as_deref() == Some("no_device")) {
        let (c, why) = admin_fault(&t, code, &j);
        return fail(c, &why);
    }
    if let Some(ds) = a.dict("devices") {
        ds.pop(&dev);
    }
    admin_save(&t.id, &a)?;
    out!("{}", d.at("label").py_str());
    Ok(())
}

/// py team-grant Team 设备 all | none | +工具:邮箱 | -工具:邮箱 …：改一台设备能用哪些账号，改完发布它的配置。stdout：现在的授权
/// （全部账号 / 几个账号）。原因码：no_team、not_admin、no_device、usage、publish
pub fn cmd_team_grant(args: &[String]) -> R<()> {
    let [sel, device, changes @ ..] = args else {
        return Err(Fail::Die("用法: py team-grant Team 设备 all | none | +工具:邮箱 | -工具:邮箱 …".into()));
    };
    if changes.is_empty() {
        return fail("usage", "用法: cca team grant Team 设备 all | none | +工具:邮箱 | -工具:邮箱 …");
    }
    let (t, a, _lk) = admin_of(sel)?;
    let dev = match find_device(&a, device) {
        Ok(d) => d,
        Err(why) => return fail("no_device", &why),
    };
    let d = a.dict("devices").and_then(|ds| ds.dict(&dev)).unwrap_or_default();
    let config = a.dict("config").unwrap_or_default();
    for c in changes {
        let cur = d.at("grants");
        match c.as_str() {
            "all" | "none" => d.set("grants", grants_spec(c).expect("all、none 都认")),
            _ => {
                let (add, spec) = (c.starts_with('+'), c.trim_start_matches(['+', '-']));
                let Some((tool, email)) = account_spec(spec).filter(|_| c.starts_with(['+', '-'])) else {
                    return fail("usage", &format!("「{c}」写成 +工具:邮箱 或者 -工具:邮箱（工具是 claude、codex、agy）"));
                };
                // 全部账号里去掉一个：先展开成现在的全部
                let g = cur.as_dict().unwrap_or_else(|| {
                    let all = Dict::new();
                    for (tool, m) in granted(&config, &V::from("all")).items() {
                        all.set(&*tool, str_list(m.as_dict().unwrap_or_default().keys()));
                    }
                    all
                });
                let list: Vec<String> = g.list(&tool).map(|l| l.strs().iter().map(|s| s.to_string()).collect()).unwrap_or_default();
                let mut list: Vec<String> = list.into_iter().filter(|e| *e != email).collect();
                if add {
                    list.push(email);
                }
                g.set(tool, str_list(&list));
                d.set("grants", g);
            }
        }
    }
    admin_save(&t.id, &a)?;
    if t.attached() {
        settle(&t, &a, Some(&dev))?;
    }
    out!("{}", grants_text(&d.at("grants")));
    Ok(())
}

/// py team-rename Team 设备 新名字：给一台设备改名字。管理员这边记着的名字（label）改成它；加入了的设备在服务器上的名字也跟着改
/// （同一把公钥照它的编号再登记一次：team_enroll，加入的时间、远程会话的配对都不变；名字被别的设备用了时服务器在后面加 -2，这边也记成
/// 服务器的：一台设备只有一个名字），它自己下次同步配置时跟着改（team.json 的 device：follow）。还没加入的设备加入以后照这个名字改
/// （align_names）。登记之前先和服务器对一遍：自己退出了的不登记回去（对的时候这边就不记它了：no_device）；服务器报的公钥和记下的
/// 对不上时不登记（只改这边记着的）。
/// 管理员自己这台还没接上服务器时改的是接上时用的名字。还在这个 Team 里的别的设备已经叫这个名字的不行（name_taken）。
/// stdout：原来的名字、改好的名字、服务器上的名字没跟着改的原因（改了是空行）。原因码：no_team、not_admin、no_device、name、server、
/// 以及请求没成的那些
pub fn cmd_team_rename(args: &[String]) -> R<()> {
    let [sel, device, name] = args else {
        return Err(Fail::Die("用法: py team-rename Team 设备 新名字".into()));
    };
    let name = name.trim().to_string();
    if !name_ok(&name) {
        return fail("name", NAME_BAD);
    }
    let (t, a, _lk) = admin_of(sel)?;
    let dev = match find_device(&a, device) {
        Ok(d) => d,
        Err(why) => return fail("no_device", &why),
    };
    let devs = a.dict("devices").unwrap_or_default();
    let still = |d: &Dict| matches!(d.str("state").as_deref(), Some("local" | "invited" | "joined"));
    if name_taken(&a, &dev, &name) {
        return fail("name", &format!("这个 Team 里已经有一台设备叫「{name}」：换一个名字"));
    }
    let d = devs.dict(&dev).unwrap_or_default();
    let (old, me) = (label_of(&d, &dev), dev == t.dev);
    let mut done = name.clone();
    let why = if !t.attached() {
        if me {
            "还没接上服务器：接上时用这个名字".to_string()
        } else {
            "还没接上服务器：只改了这边记着的名字".to_string()
        }
    } else if d.str("state").as_deref() != Some("joined") {
        let gone = !still(&d);
        if gone {
            "它已经不在这个 Team 里了：只改了这边记着的名字".to_string()
        } else {
            "它还没加入：加入以后服务器上也用这个名字".to_string()
        }
    } else {
        let rows = match reconcile(&t, &a)? {
            Ok(r) => r,
            Err(why) => return fail("server", &why),
        };
        if !a.dict("devices").is_some_and(|ds| ds.has(&dev)) {
            return fail("no_device", &format!("「{old}」已经退出了这个 Team"));
        }
        let reported = rows.dict("by_dev").and_then(|b| b.dict(&dev)).and_then(|r| r.str("pub")).and_then(|p| root_line(&p));
        // 记下的公钥（管理员自己这台是本机的）；没记下（加入时没带对认证码）时用服务器报的：它本来就存着这一把，再登记一次不多给谁什么
        let known = if me { Some(pub_line(&t.path("id_ed25519"))?) } else { d.str("pub").map(|p| p.to_string()) };
        let key = match (known, reported) {
            (Some(k), Some(r)) if k == r => Some(k),
            (None, Some(r)) => Some(r),
            _ => None,
        };
        match key {
            None => "服务器报的公钥和这台设备加入时的不一样：服务器上的名字没改（cca team devices 看看）".to_string(),
            Some(key) => {
                let cfg = match admin_cfg(&t)? {
                    Ok(c) => c,
                    Err(why) => return fail("not_attached", &why),
                };
                let payload = dict! { dev.as_str() => dict! { "pub" => key.as_str(), "name" => name.as_str() } };
                let (code, j) = presence_post(&cfg, "team_enroll", &dict! { "devices" => payload }, 20)?;
                if code != 200 {
                    let (c, why) = admin_fault(&t, code, &j);
                    return fail(c, &why);
                }
                done = j.dict("names").and_then(|n| n.str(&dev)).filter(|n| name_ok(n)).map_or(name.clone(), |n| n.to_string());
                d.set("name", done.as_str());
                String::new()
            }
        }
    };
    d.set("label", done.as_str());
    admin_save(&t.id, &a)?;
    if me && (why.is_empty() || !t.attached()) {
        let raw = t.raw.copy();
        raw.set("device", done.as_str());
        save_team(&t.id, &raw)?;
    }
    out!("{old}\n{done}\n{why}");
    Ok(())
}

/// py team-import Team --mine：把这台设备上**自己的**官方账号（不是哪个 Team 给的；第三方 API 账号不算）连同现在走的节点、别名加进
/// 这个 Team 的配置（原来有的留着，同一个账号照这台设备上的换；别名照 put_accounts：这台设备上起过的带进去——Team 里原来是默认编的、
/// 也没被别的账号用着时换成它，没起过的编成「Team 的名字-N」），再发布——把自己的几台设备并成一个 Team 时用。stdout：几个账号、
/// 0（节点各台设备自己有）、存上了几台的配置。原因码：no_team、not_admin、usage、file（没有可加的账号）、publish。
/// 3.6.0 起没有维护者的映射了：导入映射文件的那一种（team-import Team policy.json [nodes.json]）去掉了，照旧写是 usage
pub fn cmd_team_import(args: &[String]) -> R<()> {
    arity("team-import", args, 2, 3)?;
    let (t, a, _lk) = admin_of(&args[0])?;
    if args[1] != "--mine" || args.len() != 2 {
        return fail("usage", "用法: cca team import Team --mine");
    }
    default_aliases(&a, &t.name); // 原来的先换成按工具记的（3.5.3-beta.5 及以前的 Team）
    let mine: Vec<Pick> = team_picks().into_iter().filter(|p| p.from.is_none()).collect();
    let count = mine.len();
    if count == 0 {
        return fail("file", "这台设备上没有自己的官方账号可加（别的 Team 给的、第三方 API 账号都不算）");
    }
    if let Err(why) = put_accounts(&a, &t.name, &mine) {
        return fail("file", &why); // 自己的账号不带节点的定义：到不了这里
    }
    default_aliases(&a, &t.name);
    admin_save(&t.id, &a)?;
    let stored = if t.attached() { settle(&t, &a, None)? } else { 0 };
    out!("{count}\n0\n{stored}");
    Ok(())
}

/// py team-map Team 工具 邮箱 节点|-：定一个账号走哪个节点（- 是从这个 Team 里去掉这个账号），再发布。stdout：存上了几台的配置。
/// 原因码：no_team、not_admin、usage、publish
pub fn cmd_team_map(args: &[String]) -> R<()> {
    arity("team-map", args, 4, 4)?;
    let (t, a, _lk) = admin_of(&args[0])?;
    let Some((tool, email)) = account_spec(&format!("{}:{}", args[1], args[2])) else {
        return fail("usage", "用法: cca team map Team 工具 邮箱 节点（工具是 claude、codex、agy；节点写 - 是去掉这个账号）");
    };
    let node = args[3].trim();
    let accounts = a.dict("config").unwrap_or_default().dict("accounts").unwrap_or_default();
    let m = accounts.dict(&tool).unwrap_or_default();
    if node == "-" {
        m.pop(&email);
    } else if node.is_empty() {
        return fail("usage", "节点的名字不能是空的");
    } else {
        m.set(email, node);
    }
    accounts.set(tool, m);
    let config = a.dict("config").unwrap_or_default();
    config.set("accounts", accounts);
    a.set("config", config);
    default_aliases(&a, &t.name); // 新加的账号编成「Team 的名字-N」；去掉的账号（别的工具里也没有了）的别名去掉
    admin_save(&t.id, &a)?;
    let stored = if t.attached() { settle(&t, &a, None)? } else { 0 };
    out!("{stored}");
    Ok(())
}

/// py team-alias Team 账号 新别名：改一个账号在这个 Team 里的别名，再发布。别名按工具记：账号写 工具:邮箱 只改这个工具里的，只写邮箱
/// 改它在的每个工具里的。各台设备上照 Team 给的用着的下次同步配置时跟着改，自己改过的不动（team_alias_follow）；管理员这台设备上的账号表
/// 不跟着改。同一个工具里别的账号已经叫这个的不行（别的工具里的可以：Claude、Codex 各有各的 1 号）；team-数字 不行（以前默认编号的
/// 写法：发布时会换成「Team 的名字-数字」，名字里一个能用的字都没有的 Team 除外）。stdout：原来的别名（几个工具里
/// 不一样时用「、」连起来）、存上了几台的配置。原因码：no_team、not_admin、alias、no_account、publish
pub fn cmd_team_alias(args: &[String]) -> R<()> {
    arity("team-alias", args, 3, 3)?;
    let alias = args[2].trim().to_string();
    if !alias_ok(&V::from(alias.as_str())) {
        return fail("alias", "别名不能是空的，不能有空格、引号、括号、反斜杠或 @，最多 12 个汉字或 24 个字母");
    }
    let (t, a, _lk) = admin_of(&args[0])?;
    if let Some(k) = legacy_n(&alias).filter(|k| team_default_alias(&t.name, *k) != alias) {
        let want = team_default_alias(&t.name, k);
        return fail("alias", &format!("「{alias}」是以前默认编号的写法，发布时会换成「{want}」：直接写「{want}」，或者换一个"));
    }
    let sel = args[1].trim().to_lowercase();
    let (only, email) = match sel.split_once(':') {
        Some((tool, e)) if tools().iter().any(|x| x.name == tool) => (Some(tool.to_string()), e.trim().to_string()),
        _ => (None, sel.clone()),
    };
    default_aliases(&a, &t.name);
    let config = a.dict("config").unwrap_or_default();
    let (accounts, by_tool) = (config.dict("accounts").unwrap_or_default(), config.dict("tool_aliases").unwrap_or_default());
    let targets: Vec<_> = tools()
        .iter()
        .filter(|x| only.as_deref().is_none_or(|o| o == x.name) && accounts.dict(x.name).is_some_and(|m| m.has(&email)))
        .collect();
    if targets.is_empty() {
        return fail(
            "no_account",
            &format!("「{}」里没有 {sel} 这个账号（cca team devices {} --json 看有哪些）", t.name, &t.id[..8]),
        );
    }
    let mut old: Vec<String> = Vec::new();
    for x in &targets {
        let names = by_tool.dict(x.name).unwrap_or_default();
        if let Some(other) = alias_holder(&names, &email, &alias) {
            return fail("alias", &format!("「{}」里 {} 账号 {other} 已经叫「{alias}」：换一个", t.name, x.label));
        }
        if let Some(o) = names.str(&email).map(|o| o.to_string()).filter(|o| !old.contains(o)) {
            old.push(o);
        }
    }
    for x in &targets {
        let names = by_tool.dict(x.name).unwrap_or_default();
        names.set(email.as_str(), alias.as_str());
        by_tool.set(x.name, names);
    }
    config.set("tool_aliases", by_tool);
    a.set("config", config);
    default_aliases(&a, &t.name); // 按邮箱的 aliases 跟着算
    admin_save(&t.id, &a)?;
    let stored = if t.attached() { settle(&t, &a, None)? } else { 0 };
    out!("{}\n{stored}", old.join("、"));
    Ok(())
}

/// py team-alias-lock Team on|off：锁定（解开）这个 Team 里账号的别名，再发布。锁定了：各台设备上照 Team 给的别名用，自己改过的
/// 换回来（team_alias_follow），cca rename 改不了（alias_locked）；管理员这台设备的账号表照旧不管。改别名（cca team alias）照样能改，
/// 各台设备跟着换。stdout 一行一项：Team 的名字、存上了几台的配置。原因码：no_team、not_admin、server、publish
pub fn cmd_team_alias_lock(args: &[String]) -> R<()> {
    arity("team-alias-lock", args, 2, 2)?;
    let on = match args[1].as_str() {
        "on" => true,
        "off" => false,
        _ => return Err(Fail::Die("用法: py team-alias-lock Team on|off".into())),
    };
    let (t, a, _lk) = admin_of(&args[0])?;
    let config = a.dict("config").unwrap_or_default();
    if on {
        config.set("alias_lock", true);
    } else {
        config.pop("alias_lock");
    }
    a.set("config", config);
    admin_save(&t.id, &a)?;
    let stored = if t.attached() { settle(&t, &a, None)? } else { 0 };
    out!("{}\n{stored}", t.name);
    Ok(())
}

/// 命令行上写的转发中转（每个参数也可以用逗号分开写好几个；只写 off 是一个都不信任）→ 规整好的名单；不对的说出第一处（原因码 domain）
fn fwd_args(words: &[String]) -> Result<Vec<String>, String> {
    if matches!(words, [w] if w == "off") {
        return Ok(Vec::new());
    }
    let mut got = Vec::new();
    for w in words.iter().flat_map(|w| w.split(',')).filter(|w| !w.trim().is_empty()) {
        got.push(team_fwd_check(w)?);
    }
    if got.is_empty() {
        return Err("要写中转的域名（像 relay.example.com），不信任写 off".into());
    }
    Ok(fwd_norm(got))
}

/// 问同意时的范围：新名单的摘要（App 里同意的是这一份；改的时候在锁里照它核对）
pub(super) fn fwd_token(list: &[String]) -> String {
    sha256_hex(list.join("\n"))[..16].to_string()
}

/// py team-forwarder Team [check|set 域名…|off [--ok 摘要]]：这个 Team 给的 Claude 账号在各台设备上信任哪些转发中转（不安全；admin.json 的
/// config.mail_forwarders，发给拿到 Claude 账号的设备：build_doc；设备上照它，这台设备给这些账号登记的不算：mail::mail_forwarders）。
/// 不写：stdout 一行一项：Team 的编号、名字、现在的名单（逗号连起来，没有是空行）。check（只看）：编号、名字、现在的名单、新名单、
/// 比现在宽没有（1 | 0：多了现在的名单盖不住的域名，要先问同意）、新名单的摘要（同意的范围）。set：在锁里照现在的名单再算一遍，
/// 宽了而 --ok 和新名单的摘要对不上就不改（consent）；改了接上了服务器就发布，stdout：名字、存上了几台的配置。
/// 原因码：no_team、not_admin、usage、domain（不是域名、公共邮箱的域名、公共的后缀）、consent、server、publish
pub fn cmd_team_forwarder(args: &[String]) -> R<()> {
    const USE: &str = "用法: cca team forwarder Team [域名… | off]";
    let (sel, mode, rest) = match args {
        [s] => (s, "show", &args[1..]),
        [s, m, ..] if m == "check" || m == "set" => (s, m.as_str(), &args[2..]),
        _ => return fail("usage", USE),
    };
    let (words, ok) = match rest {
        [w @ .., flag, h] if flag == "--ok" && mode == "set" => (w, Some(h.as_str())),
        w => (w, None),
    };
    let new = if mode == "show" {
        Vec::new()
    } else if words.is_empty() {
        return fail("usage", USE);
    } else {
        match fwd_args(words) {
            Ok(v) => v,
            Err(why) => return fail("domain", &why),
        }
    };
    let (t, a, _lk) = admin_of(sel)?;
    let config = a.dict("config").unwrap_or_default();
    let old = doc_forwarders(&config);
    let widen = fwd_widens(&old, &new);
    match mode {
        "show" => out!("{}\n{}\n{}", t.id, t.name, old.join(",")),
        "check" => out!("{}\n{}\n{}\n{}\n{}\n{}", t.id, t.name, old.join(","), new.join(","), u8::from(widen), fwd_token(&new)),
        _ => {
            if widen && ok != Some(fwd_token(&new).as_str()) {
                return fail("consent", "名单比现在宽了，要先同意（也可能刚在别处改过）：重新运行 cca team forwarder，会先问你");
            }
            if new.is_empty() {
                config.pop("mail_forwarders");
            } else {
                config.set("mail_forwarders", str_list(&new));
            }
            a.set("config", config);
            admin_save(&t.id, &a)?;
            let stored = if t.attached() { settle(&t, &a, None)? } else { 0 };
            out!("{}\n{stored}", t.name);
        }
    }
    Ok(())
}

/// py team-name Team 新名字：改这个 Team 的名字（team.json 的 name），再发布：各台设备下次同步配置时跟着改（配置里的 name：follow）。
/// 照原来的名字默认编的别名（「旧名字-N」）跟着换成「新名字-N」（rename_defaults），各台设备上照 Team 给的用着的一起跟着换。
/// 已经发出去的邀请链接里带着的还是原来的名字，拿它加入的设备第一次同步到配置时换成新的。手机网页端的标题是部署时写进服务器的（app/name），
/// 下次部署（cca team deploy）时才换。这台设备上别的 Team 已经叫这个名字的不行（按名字选 Team 就分不清了）。先和服务器对一遍：连不上、
/// 对不上（server）时名字还没改，照原来的命令再来一次就行；发布没成（publish）时名字已经改了，原因里说照新名字 cca team publish。
/// stdout：原来的名字、新的名字、存上了几台的配置。原因码：no_team、not_admin、name、server、publish
pub fn cmd_team_name(args: &[String]) -> R<()> {
    arity("team-name", args, 2, 2)?;
    let name = args[1].trim().to_string();
    if !team_name_ok(&name) {
        return fail("name", "Team 的名字要 1～64 个字符，不能有控制字符");
    }
    let (t, a, _lk) = admin_of(&args[0])?;
    if let Some(o) = teams().iter().find(|o| o.id != t.id && o.name == name) {
        return fail("name", &format!("这台设备上已经有一个叫「{name}」的 Team（{}）：换一个名字", &o.id[..8]));
    }
    if t.attached() {
        if let Err(why) = reconcile(&t, &a)? {
            return fail("server", &why);
        }
    }
    let old = t.name.clone();
    if old != name {
        let raw = t.raw.copy();
        raw.set("name", name.as_str());
        save_team(&t.id, &raw)?;
    }
    let t = load_team(&t.id).ok_or_else(|| Fail::Die("刚写好的 team.json 读不回来".into()))?;
    if old != name {
        rename_defaults(&a, &old, &name);
        default_aliases(&a, &name);
        admin_save(&t.id, &a)?;
    }
    // 发布没成时名字已经改了：照原来的名字再来一次找不到这个 Team，所以说清楚
    let later = |why: String| {
        if old == name {
            why
        } else {
            format!("名字已经改成「{name}」，但配置没全发布成（{why}）：修好后运行 cca team publish {name}")
        }
    };
    let stored = if !t.attached() {
        0
    } else {
        match publish(&t, &a, None)? {
            Ok(p) if p.problems.is_empty() => p.stored,
            Ok(p) => return fail("publish", &later(p.problems.join("；"))),
            Err(why) => return fail("publish", &later(why)),
        }
    };
    out!("{old}\n{name}\n{stored}");
    Ok(())
}

/// py team-publish Team：和服务器对一遍（见 reconcile），把内容变了的配置都发布。stdout：存上了几台的配置、换服务器时原来那台上
/// 没收尾成的原因（见 finish_move；没有、成了是空行）。
/// 原因码：no_team、not_admin、server、publish
pub fn cmd_team_publish(args: &[String]) -> R<()> {
    arity("team-publish", args, 1, 1)?;
    let (t, a, _lk) = admin_of(&args[0])?;
    let stored = settle(&t, &a, None)?;
    sync_locked(&t)?;
    let note = finish_move(&t, &a)?; // 上次换服务器没做完的收尾
    out!("{stored}\n{note}");
    Ok(())
}

/// py team-admin ls | plan | add | remove Team [设备]：几位管理员（3.6.15 起；docs/dev/team.md「几位管理员」）。
/// - ls：一行一位：设备编号\t名字\towner | admin（owner 是建 Team 的那台，拿着根密钥；别的管理员也能看）
/// - plan add | remove Team 设备（只读）：「Team 的编号\t设备编号\t名字」（bash 据此问同意）
/// - add：给一台加入了的设备管理员身份：管理员名单换成新的一版（根密钥签），这个 Team 的管理员设置存到服务器上，配置里带上
///   管理员的密钥发布给它（它下次同步配置就是管理员了：能邀请、移除设备，改账号、节点，发布）
/// - remove：收回：名单换成新的一版，管理员的密钥换一把（它手里那把解不开以后的设置），各台设备的配置重新发布
///
/// add、remove 的 stdout：存上了几台的配置。只有建 Team 的设备能 add、remove。原因码：no_team、not_admin、owner、no_device、
/// state（还没加入、公钥没核对过的设备）、self、already、not_admin_dev、old（服务器太旧）、sign、busy、server、publish
pub fn cmd_team_admin(args: &[String]) -> R<()> {
    const USAGE: &str = "用法: py team-admin ls Team | plan add|remove Team 设备 | add|remove Team 设备";
    let (mode, plan, sel, device) = match args {
        [m, s] if m == "ls" => ("ls", false, s, None),
        [p, m, s, d] if p == "plan" && matches!(m.as_str(), "add" | "remove") => (m.as_str(), true, s, Some(d)),
        [m, s, d] if matches!(m.as_str(), "add" | "remove") => (m.as_str(), false, s, Some(d)),
        _ => return Err(Fail::Die(USAGE.into())),
    };
    let (t, a, _lk) = admin_of(sel)?;
    let devs = a.dict("devices").unwrap_or_default();
    let admins = a.dict("admins").unwrap_or_default();
    let Some(device) = device else {
        for (dev, d) in devs.items() {
            let Some(d) = d.as_dict() else { continue };
            let role = match d.get("n").and_then(|n| n.as_int()) {
                Some(0) => "owner",
                _ if admins.has(&dev) => "admin",
                _ => continue,
            };
            out!("{dev}\t{}\t{role}", label_of(&d, &dev));
        }
        return Ok(());
    };
    owner_only(&t, if mode == "add" { "授予管理员身份" } else { "收回管理员身份" })?;
    let dev = match find_device(&a, device) {
        Ok(d) => d,
        Err(why) => return fail("no_device", &why),
    };
    let d = devs.dict(&dev).unwrap_or_default();
    let label = label_of(&d, &dev);
    if dev == t.dev {
        return fail("self", "这是建这个 Team 的设备：它本来就是管理员（拿着根密钥）");
    }
    match mode {
        "add" if admins.has(&dev) => return fail("already", &format!("「{label}」已经是管理员了")),
        "add" if d.str("state").as_deref() != Some("joined") || d.str("pub").and_then(|p| root_line(&p)).is_none() => {
            return fail(
                "state",
                &format!("「{label}」还没加入，或者它的公钥还没核对过（旧版本的 cca 加入的）：等它加入、更新到 3.6.15 以后再试（cca team devices 看）"),
            );
        }
        "remove" if !admins.has(&dev) => return fail("not_admin_dev", &format!("「{label}」不是管理员")),
        _ => {}
    }
    if plan {
        out!("{}\t{dev}\t{label}", t.id);
        return Ok(());
    }
    if !t.attached() {
        return fail("not_attached", &admin_cfg(&t)?.err().unwrap_or_default());
    }
    // 先和服务器对一遍（公钥、加入了没有照它核对过的）
    if let Err(why) = reconcile(&t, &a)? {
        return fail("server", &why);
    }
    let admins = a.dict("admins").unwrap_or_default();
    if mode == "add" {
        admins.set(dev.as_str(), dict! { "at" => now() });
        if a.str("akey").is_none() {
            a.set("akey", token_hex(32));
            a.set("arev", 0);
        }
    } else {
        admins.pop(&dev);
        // 换一把管理员的密钥：收回了的那台手里的那一把解不开以后存上去的设置（它已经看过的收不回来：见 docs/dev/team.md）
        a.set("akey", token_hex(32));
        a.pop("asha");
    }
    a.set("admins", admins);
    let epoch = a.get("admins_epoch").and_then(|e| e.as_int()).unwrap_or(0) + 1;
    a.set("admins_epoch", epoch);
    // 先放名单（服务器从此认它 / 不再认它），再存共用的设置（新的密钥封的），最后发布：拿到管理员密钥的设备取得到设置
    match admins_post(&t, &a)? {
        Ok(()) => {}
        Err((c, why)) => return fail(c, &why),
    }
    admin_save(&t.id, &a)?;
    let stored = settle(&t, &a, None)?;
    out!("{stored}");
    Ok(())
}
