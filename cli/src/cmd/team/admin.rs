//! Team 的管理员这一头（拿着根密钥的设备）：建 Team、接上服务器、给设备发邀请、定每台设备能用哪些账号、维护账号 → 节点和节点的定义、
//! 把每台设备的那一份配置封好签好存到服务器上。
//!
//! admin.json（600，只有管理员的设备有）：
//! ```text
//! {"v": 1, "master": 主密钥（64 个十六进制字符）, "salt": 这个 Team 的盐, "next": 下一台设备的序数, "gen": 服务器上这一份记录的编号,
//!  "config": {"accounts": {工具: {邮箱: 节点}}, "aliases": {邮箱: 别名}, "install_node": 节点, "nodes": {"group", "proxies"} | null},
//!  "devices": {设备编号: {"n": 序数, "label": 管理员起的名字, "state": local | invited | joined | expired | revoked,
//!              "pub": 它的公钥, "name": 它在服务器上的名字, "grants": "all" | {工具: [邮箱…]}, "seq": 发布到第几版,
//!              "sha": 那一版内容的摘要, "exp": 邀请什么时候过期}}}
//! ```
//! 每台设备那串随机数（邀请里的 k）不存：是 HMAC(主密钥, "cca-team-k-v1\0" 序数) 的前 16 个字节，随时重算。所以备份根密钥和主密钥
//! 就够了（管理员的本机状态丢了，各台设备的密钥还算得出来）。

use super::doc::{
    doc_digest, doc_problems, envelope_seal, invite_encode, invite_parts, pub_proof, root_line, team_id, team_name_ok, token_hash,
    Invite, SEQ_MAX,
};
use super::{cfg_of, fail, fault, find_team, index_add, now, pub_line, save_team, sync_locked, team_lock};
use crate::cmd::hub::{hex32, host_name, host_of, keygen, name_ok, NAME_BAD};
use crate::cmd::hub::{route, HubKind};
use crate::cmd::presence::{key_at, presence_key, presence_post, ssh_fp, url_ok, PRESENCE_PIN_RE};
use crate::cmd::{arity, keyshare::is_num};
use crate::core::model::{node_of, parse_key};
use crate::core::policy::{fill_problems, policy_problems, policy_sources};
use crate::core::team::{load_team, team_dir, team_path, Role, Team, TEAMS_DIR};
use crate::core::tool::tools;
use crate::py::fs::{self, load_quiet, save_json};
use crate::py::hash::{from_hex, hmac_sha256, to_hex};
use crate::py::obj::{str_list, Dict, List, V};
use crate::py::rand::token_hex;
use crate::py::{json, path, Fail, R};
use crate::{dict, out};
use std::collections::BTreeSet;

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
    match j.filter(|j| t.role == Role::Admin && ok(j)) {
        Some(j) => Ok(j),
        None => Err(format!("这台设备不是「{}」的管理员（只有建这个 Team 的设备能管理它）", t.name)),
    }
}

fn admin_save(tid: &str, a: &Dict) -> R<()> {
    save_json(&admin_path(tid), &V::Dict(a.copy()), Some(0o600), true)
}

/// 第 n 台设备那串随机数（邀请里的 k）：HMAC(主密钥, "cca-team-k-v1\0" n) 的前 16 个字节
fn device_secret(master: &str, n: i64) -> String {
    let key = from_hex(master).unwrap_or_default();
    to_hex(&hmac_sha256(&key, format!("cca-team-k-v1\0{n}").as_bytes())[..16])
}

/// 拿根密钥签名的配置（presence_post 用）；没接上服务器、根密钥不见了是给人看的原因
fn root_cfg(t: &Team) -> R<Result<Dict, String>> {
    if !t.attached() {
        return Ok(Err(format!("「{}」还没有接上同步服务器：先 cca team attach {} https://服务器:端口", t.name, &t.id[..8])));
    }
    Ok(match key_at(&t.path("root_ed25519"))? {
        Some((key, fp)) => Ok(cfg_of(&t.url, &t.pins, key, fp)),
        None => Err(format!("「{}」的根密钥不见了（~/.cca/teams/{}/root_ed25519）", t.name, t.id)),
    })
}

/// 管理员的请求没成的原因
fn admin_fault(t: &Team, code: i64, j: &Dict) -> (&'static str, String) {
    match code {
        403 => {
            ("denied", format!("{} 配置的不是这个 Team 的根公钥（服务器要用 --team-root 这个 Team 的根公钥运行）", host_of(&t.url)))
        }
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
fn build_doc(t: &Team, a: &Dict, dev: &Dict) -> Dict {
    let config = a.dict("config").unwrap_or_default();
    // 拿别的密钥用了这个邀请的（reconcile 记的 mismatch）：一个账号都不给
    let grants = if dev.get("mismatch").is_some_and(|m| m.truthy()) { V::Dict(Dict::new()) } else { dev.at("grants") };
    let accounts = granted(&config, &grants);
    let mut emails: BTreeSet<String> = BTreeSet::new();
    let mut used: BTreeSet<String> = BTreeSet::new();
    for m in accounts.values().iter().filter_map(|m| m.as_dict()) {
        for (e, n) in m.items() {
            emails.insert(e.to_string());
            used.insert(n.py_str());
        }
    }
    let aliases = Dict::new();
    for (e, alias) in config.dict("aliases").unwrap_or_default().items() {
        if emails.contains(&*e) {
            aliases.set(&*e, alias);
        }
    }
    let install = config.str("install_node").map(|n| n.to_string()).unwrap_or_default();
    if !install.is_empty() {
        used.insert(install.clone());
    }
    dict! {
        "v" => 1, "at" => now(), "name" => t.name.as_str(), "salt" => a.at("salt"),
        "server" => dict! { "u" => t.url.as_str(), "p" => str_list(&t.pins) },
        "accounts" => accounts, "aliases" => aliases, "install_node" => install, "nodes" => needed_nodes(config.dict("nodes"), &used),
    }
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
/// 备份里恢复的）就接着它的往上加再存一次——它说的数要像话（不比记着的小、没到上限），不然不信；服务器不认得的编号（邀请过期很久了、
/// 被移除了）记成 expired。改了 admin.json
fn publish(t: &Team, a: &Dict, only: Option<&str>) -> R<Result<Published, String>> {
    let cfg = match root_cfg(t)? {
        Ok(c) => c,
        Err(why) => return Ok(Err(why)),
    };
    let devs = a.dict("devices").unwrap_or_default();
    let (master, root_key) = (a.at("master").py_str(), t.path("root_ed25519"));
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
            let doc = build_doc(t, a, &d);
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
            match envelope_seal(&root_key, &parts.keys, &t.id, &dev, seq, &doc)? {
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
                d.set("state", "expired");
                out.problems.push(format!("{label}：服务器不认得这台设备（邀请过期很久了，或者被移除了）"));
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
    let cfg = match root_cfg(old)? {
        Ok(c) => c,
        Err(why) => return Ok(why),
    };
    for dev in pending.strs() {
        let (code, j) = presence_post(&cfg, "team_revoke", &dict! { "dev" => &*dev }, 15)?;
        if code != 200 && code != 404 {
            return Ok(admin_fault(old, code, &j).1);
        }
    }
    let (master, root_key) = (a.at("master").py_str(), t.path("root_ed25519"));
    let mut envs: Vec<(String, Dict)> = Vec::new();
    for (dev, d) in a.dict("devices").unwrap_or_default().items() {
        let Some(d) = d.as_dict().filter(|d| d.str("state").as_deref() == Some("joined")) else { continue };
        let (n, seq) = (d.get("n").and_then(|n| n.as_int()).unwrap_or(0), d.get("seq").and_then(|s| s.as_int()).unwrap_or(0));
        let Some(parts) = invite_parts(&device_secret(&master, n)).filter(|_| (1..SEQ_MAX).contains(&seq)) else { continue };
        if let Ok(env) = envelope_seal(&root_key, &parts.keys, &t.id, &dev, seq, &build_doc(t, a, &d))? {
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
/// - 加入了的设备 invited → joined；邀请不在了、设备也没加入的记成 expired；编号没变、服务器上却没有了的是它自己退出了（left：
///   不登记回去；它又出现了就恢复成 joined）；服务器上存着的那一版比发布过的旧就重新发布
///
/// 返回 {"gen", "by_dev": {设备编号: 服务器的那一行}, "pending": {设备编号: 邀请的那一行}, "problems": {设备编号: 要告诉管理员的}}；
/// 没成是给人看的原因。改了 admin.json
fn reconcile(t: &Team, a: &Dict) -> R<Result<Dict, String>> {
    let cfg = match root_cfg(t)? {
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
    for (dev, d) in devs.items() {
        let Some(d) = d.as_dict() else { continue };
        let state = d.str("state").map(|s| s.to_string()).unwrap_or_default();
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
                // 这个邀请是给某一把公钥的（照映射迁过来的机器：它的 deploy key），服务器报的却是别的：邀请被别的机器用了。它已经
                // 取走的那一份收不回来，以后不给它账号（build_doc 看 mismatch）。服务器没报公钥不算；后来报的对上了就不再这样记
                // （服务器一时说错，不该把一台好好的设备的账号永久拿掉）
                None if d.str("expect").is_some_and(|e| reported.as_deref().is_some_and(|p| p != &*e)) => {
                    d.set("mismatch", true);
                    problems.set(
                        &*dev,
                        format!(
                            "拿这个邀请加入的不是指定的那把密钥（邀请被别的机器用了）：不给它账号；移除它（cca team revoke {} {}），\
                             再运行一次 ./policy.sh team 给那台机器重发",
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
        } else if state == "joined" {
            d.set("state", "left"); // 服务器上没有它了：它自己退出了（记录没了的那种，上面已经登记回去）
            d.pop("sha");
        }
    }
    admin_save(&t.id, a)?;
    rows.set("problems", problems);
    Ok(Ok(rows))
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
    if let Err(why) = admin_load(&t) {
        return fail("not_admin", &why);
    }
    let lock = team_lock(&t.id)?;
    // 等锁的时候别的命令可能改过：拿到以后现读
    let t = load_team(&t.id).ok_or_else(|| Fail::Die("这个 Team 在本机的记录不见了".into()))?;
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

/// py team-new 名字 [--device 这台设备的名字]：在这台设备上建一个 Team（还没有服务器）：生成根密钥、主密钥和盐，这台设备是管理员、
/// 也是第一台设备。stdout：Team 的编号、根公钥（部署服务器时的 --team-root）。原因码：name
pub fn cmd_team_new(args: &[String]) -> R<()> {
    let (name, device) = match args {
        [n] => (n.trim().to_string(), host_name()),
        [n, flag, d] if flag == "--device" => (n.trim().to_string(), d.trim().to_string()),
        _ => return Err(Fail::Die("用法: py team-new 名字 [--device 这台设备的名字]".into())),
    };
    if !team_name_ok(&name) {
        return fail("name", "Team 的名字要 1～64 个字符，不能有控制字符");
    }
    if !name_ok(&device) {
        return fail("name", NAME_BAD);
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
            "v" => 1, "master" => master, "salt" => token_hex(16), "next" => 1,
            "config" => dict! { "accounts" => Dict::new(), "aliases" => Dict::new(), "install_node" => "", "nodes" => V::None },
            "devices" => dict! { dev.as_str() => me },
        };
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
    out!("{tid}\n{root}");
    Ok(())
}

/// py team-attach Team https://服务器:端口 [证书公钥的指纹…]：接上（换一台）同步服务器。服务器要已经用这个 Team 的根公钥运行着
/// （--team-root）。把这台设备登记上去，别的加入过的设备也登记回去，再发布各台的配置；换了一台的话，新的配置也存到原来那台上
/// （还连着它的设备取到以后自己换过来）。stdout：服务器的地址:端口、这台设备在服务器上的名字、现在几台设备、存上了几台的配置、
/// 换服务器时原来那台没存上的原因（存上了、不是换服务器是空行）。原因码：no_team、not_admin、url、pin、denied（服务器配置的不是这个 Team 的根公钥）、
/// old、unreachable、cert、clock、busy、server、publish
pub fn cmd_team_attach(args: &[String]) -> R<()> {
    let [sel, url, pins @ ..] = args else {
        return Err(Fail::Die("用法: py team-attach Team https://服务器:端口 [证书公钥的指纹…]".into()));
    };
    let (t, a, _lk) = admin_of(sel)?;
    let url = url.trim_end_matches('/').to_string();
    if !url_ok(&url) {
        return fail("url", "服务器的地址要写成 https://地址:端口");
    }
    if pins.len() > 4 || !pins.iter().all(|p| PRESENCE_PIN_RE.fullmatch(p).is_some()) {
        return fail("pin", "证书公钥的指纹要写成 sha256//…（最多 4 个）");
    }
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
        admin_save(&t.id, &a)?;
    }
    let raw = t.raw.copy();
    raw.set("url", url.as_str());
    raw.set("pins", str_list(pins));
    let t = Team { url, pins: pins.to_vec(), raw: raw.copy(), ..t };
    let cfg = match root_cfg(&t)? {
        Ok(c) => c,
        Err(why) => return fail("key", &why),
    };
    // 先登记这台设备：服务器认不认这把根密钥、是不是 Team 的服务器，这一下就知道。认了才把新的地址写进 team.json
    let me = devs.dict(&t.dev).unwrap_or_default();
    let my_pub = pub_line(&t.path("id_ed25519"))?;
    let payload = dict! { t.dev.as_str() => dict! { "pub" => my_pub.as_str(), "name" => t.device.as_str() } };
    let (code, j) = presence_post(&cfg, "team_enroll", &dict! { "devices" => payload }, 20)?;
    if code != 200 {
        if !load_team(&t.id).is_some_and(|cur| cur.attached()) {
            a.pop("moved_from"); // 第一次接就没接上：没有原来那台
            admin_save(&t.id, &a)?;
        }
        let (c, why) = admin_fault(&t, code, &j);
        return fail(c, &why);
    }
    let device = j.dict("names").and_then(|n| n.str(&t.dev)).filter(|n| name_ok(n)).map_or(t.device.clone(), |n| n.to_string());
    raw.set("device", device.as_str());
    save_team(&t.id, &raw)?;
    me.set("state", "joined");
    me.set("name", device.as_str());
    me.set("pub", my_pub);
    devs.set(t.dev.as_str(), me);
    admin_save(&t.id, &a)?;
    let t = load_team(&t.id).ok_or_else(|| Fail::Die("刚写好的 team.json 读不回来".into()))?;
    let stored = settle(&t, &a, None)?;
    sync_locked(&t)?;
    let members = devs.values().iter().filter(|d| d.as_dict().is_some_and(|d| d.str("state").as_deref() == Some("joined"))).count();
    let note = finish_move(&t, &a)?;
    out!("{}\n{device}\n{members}\n{stored}\n{note}", host_of(&t.url));
    Ok(())
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

/// py team-invite Team 给这台设备起的名字 [--accounts all|工具:邮箱,…] [--ttl 秒]：给一台设备发邀请。stdout：邀请链接（只这一次，
/// 它是凭证）、设备编号、配置没存上时的原因（存上了是空行）。原因码：no_team、not_admin、usage、name、full（没用掉的邀请太多）、
/// 以及请求没成的那些
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
    let cfg = match root_cfg(&t)? {
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
    Ok(Ok((invite_link(t, &secret, exp), parts.dev)))
}

/// 邀请链接（那串随机数是照主密钥和序数算的，所以发过的链接能原样再算出来）
fn invite_link(t: &Team, secret: &str, exp: i64) -> String {
    invite_encode(&Invite {
        url: t.url.clone(),
        pins: t.pins.clone(),
        root: t.root.clone(),
        secret: secret.to_string(),
        name: t.name.clone(),
        exp: Some(exp),
    })
}

/// py team-handoff Team：把用着维护者映射的各台机器迁到这个 Team（./policy.sh team 调的）。stdin 一行一台：「公钥（ssh-ed25519 AAAA…）
/// \t别名」——映射仓库上各台机器的 deploy key。每台机器一个邀请，指定要拿这把公钥加入（expect）、账号全给：拿这把公钥加入了的不用
/// 再发；发过、还没用、离过期还有三天以上的照原来的那个；自己退出了的、管理员移除了的不再发（要它回来：手工 cca team invite … --key，
/// 退出过的那台还要自己 cca team join）；别的（没发过、过期了、被别的密钥用掉了）新发一个，30 天过期，同一把公钥以前没用上的那几项
/// 去掉。最后和服务器对一遍、发布。stdout 一行 JSON：{"version": 1, "kind": "team_handoff", "team": 编号, "name": 名字,
/// "section": {"v": 1, "invites": {deploy key 的指纹: 邀请链接}}（写进 policy.json 的 team 一节）, "joined": [别名…], "kept": [别名…],
/// "new": [别名…], "left": [别名…], "revoked": [别名…], "problems": [没发成的原因…]}。原因码：no_team、not_admin、not_attached、
/// usage（stdin 里没有一把合格的公钥）、server、publish
pub fn cmd_team_handoff(args: &[String]) -> R<()> {
    arity("team-handoff", args, 1, 1)?;
    let mut machines: Vec<(String, String, String)> = Vec::new(); // (公钥, 指纹, 别名)
    for line in crate::py::io::read_stdin_text().lines() {
        let (key, alias) = line.split_once('\t').unwrap_or((line, ""));
        let Some(key) = root_line(key) else { continue };
        let Some(fp) = key.split(' ').nth(1).and_then(ssh_fp) else { continue };
        if machines.iter().any(|m| m.0 == key) {
            continue;
        }
        let alias =
            Some(alias.trim().to_string()).filter(|n| name_ok(n)).unwrap_or_else(|| format!("device-{}", machines.len() + 1));
        machines.push((key, fp, alias));
    }
    if machines.is_empty() {
        return fail("usage", "没有要迁的机器：stdin 一行一台「公钥\\t别名」");
    }
    let (t, a, _lk) = admin_of(&args[0])?;
    let cfg = match root_cfg(&t)? {
        Ok(c) => c,
        Err(why) => return fail("not_attached", &why),
    };
    if let Err(why) = reconcile(&t, &a)? {
        return fail("server", &why);
    }
    let master = a.at("master").py_str();
    let devs = a.dict("devices").unwrap_or_default();
    // 管理员自己这台（它的 deploy key 多半也在仓库上）本来就在 Team 里：不用给它发
    let own = presence_key()?.map(|(_, fp)| fp);
    let invites = Dict::new();
    let (mut joined, mut kept, mut fresh, mut problems): (Vec<V>, Vec<V>, Vec<V>, Vec<V>) = Default::default();
    let (mut left, mut revoked): (Vec<V>, Vec<V>) = Default::default();
    let mut full = false;
    for (key, fp, alias) in &machines {
        let mine: Vec<(String, Dict)> = devs
            .items()
            .iter()
            .filter_map(|(dev, d)| Some((dev.to_string(), d.as_dict()?)))
            .filter(|(_, d)| d.str("expect").as_deref() == Some(key.as_str()))
            .collect();
        let state = |d: &Dict| d.str("state").map(|s| s.to_string()).unwrap_or_default();
        if own.as_deref() == Some(fp.as_str())
            || mine.iter().any(|(_, d)| state(d) == "joined" && !d.get("mismatch").is_some_and(|m| m.truthy()))
        {
            joined.push(V::from(alias.as_str()));
            continue;
        }
        let live = mine
            .iter()
            .find(|(_, d)| state(d) == "invited" && d.get("exp").and_then(|e| e.as_int()).is_some_and(|e| e - now() > 3 * 86400));
        if let Some((_, d)) = live {
            let secret = device_secret(&master, d.get("n").and_then(|n| n.as_int()).unwrap_or(0));
            let exp = d.get("exp").and_then(|e| e.as_int()).unwrap_or(0);
            invites.set(fp.as_str(), invite_link(&t, &secret, exp));
            kept.push(V::from(alias.as_str()));
            continue;
        }
        // 这台机器自己退出了、被管理员移除了（拿别的密钥用了邀请、后来被移除的那一项不算：那不是它）：不再自己把它请回来
        let wrong = |d: &Dict| d.get("mismatch").is_some_and(|m| m.truthy());
        if let Some(gone) = mine.iter().map(|(_, d)| d).find(|d| matches!(state(d).as_str(), "left" | "revoked") && !wrong(d)) {
            let list = if state(gone) == "left" { &mut left } else { &mut revoked };
            list.push(V::from(alias.as_str()));
            continue;
        }
        // 以前给这把公钥的、没用上的那几项不要了（服务器上还没用掉的邀请一起撤回，免得占着名额）
        for (dev, d) in &mine {
            if state(d) == "invited" {
                presence_post(&cfg, "team_revoke", &dict! { "dev" => dev.as_str() }, 15)?;
            }
            if state(d) != "joined" {
                devs.pop(dev);
            }
        }
        if full {
            continue;
        }
        match issue(&t, &a, &cfg, alias, V::from("all"), 30 * 86400, Some(key))? {
            Ok((link, _)) => {
                invites.set(fp.as_str(), link);
                fresh.push(V::from(alias.as_str()));
            }
            Err((c, why)) => {
                full = c == "full"; // 名额满了：后面的也发不了，等前面的机器加入了再运行一次
                problems.push(V::from(format!("{alias}：{why}")));
            }
        }
    }
    admin_save(&t.id, &a)?;
    match publish(&t, &a, None)? {
        Ok(p) => problems.extend(p.problems.into_iter().map(V::from)),
        Err(why) => return fail("publish", &why),
    }
    let doc = dict! {
        "version" => 1, "kind" => "team_handoff", "team" => t.id.as_str(), "name" => t.name.as_str(),
        "section" => dict! { "v" => 1, "invites" => invites },
        "joined" => joined, "kept" => kept, "new" => fresh, "left" => left, "revoked" => revoked, "problems" => problems,
    };
    out!("{}", json::dumps(&V::Dict(doc), json::Fmt::new().utf8()));
    Ok(())
}

/// 授权给人看的样子：全部 / 几个账号
fn grants_text(g: &V) -> String {
    match g.as_dict() {
        None => "全部账号".into(),
        Some(d) => format!("{} 个账号", d.values().iter().filter_map(|l| l.as_list()).map(|l| l.len()).sum::<usize>()),
    }
}

/// py team-devices Team [--json]：这个 Team 的设备（先和服务器对一遍，见 reconcile）。--json：{"version": 1, "kind": "team_devices",
/// "team": 编号, "devices": [{dev, label, name, state, me, grants, seq, have: 它手里是第几版（没说过是 null）, seen_age,
/// expires_epoch: 邀请什么时候过期, problem: 对一遍时发现的、要告诉管理员的（没有是 null）}]}；不带时一台一行：
/// 设备编号\t名字\t状态\t授权\t发布到第几版\t它手里第几版\t要告诉管理员的（没有的写 -）
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
        };
        list.push(V::Dict(row));
    }
    if as_json {
        let doc = dict! { "version" => 1, "kind" => "team_devices", "team" => t.id.as_str(), "devices" => list };
        out!("{}", json::dumps(&V::Dict(doc), json::Fmt::new().utf8().indent(2)));
        return Ok(());
    }
    for r in list.iter().filter_map(|r| r.as_dict()) {
        // 没有的写 -：bash 按制表符读，空的栏会被并掉
        let s = |k: &str| r.get(k).filter(|v| !v.is_none()).map_or("-".to_string(), |v| v.py_str());
        let grants = grants_text(&r.at("grants"));
        out!("{}\t{}\t{}\t{grants}\t{}\t{}\t{}", s("dev"), s("label"), s("state"), s("seq"), s("have"), s("problem"));
    }
    Ok(())
}

/// py team-revoke plan | apply Team 设备：移除一台设备（或者撤回还没用掉的邀请）。plan（只读）：stdout 是「Team 的编号\t设备编号\t给它
/// 起的名字\t状态」（bash 据此问同意）。apply：服务器就此不认它的密钥，给它的配置删掉（它已经拿到的节点凭证收不回来）；stdout：给它
/// 起的名字。原因码：no_team、not_admin、no_device、self（管理员移除不了自己这台）、not_attached、以及请求没成的那些
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
    if mode == "plan" {
        out!("{}\t{dev}\t{}\t{}", t.id, d.at("label").py_str(), d.at("state").py_str());
        return Ok(());
    }
    let cfg = match root_cfg(&t)? {
        Ok(c) => c,
        Err(why) => return fail("not_attached", &why),
    };
    let (code, j) = presence_post(&cfg, "team_revoke", &dict! { "dev" => dev.as_str() }, 15)?;
    if code != 200 && !(code == 404 && j.str("error").as_deref() == Some("no_device")) {
        let (c, why) = admin_fault(&t, code, &j);
        return fail(c, &why);
    }
    d.set("state", "revoked");
    d.pop("sha");
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

/// py team-import Team policy.json [nodes.json]：把现有的映射（账号 → 节点、别名、安装客户端用的节点）和参考节点导进这个 Team 的
/// 配置（整个换掉原来的），再发布。文件就是维护者映射仓库里的那两份（格式照旧查）。stdout：几个账号、几个节点、存上了几台的配置。
/// 原因码：no_team、not_admin、file（读不了、格式不对）、publish
///
/// py team-import Team --mine：把这台设备上**自己的**官方账号（不是维护者共享的、也不是哪个 Team 给的；第三方 API 账号不算）连同现在
/// 走的节点、别名加进这个 Team 的配置（原来有的留着，同一个账号照这台设备上的换），再发布——把自己的几台设备并成一个 Team 时用
/// （代替自己的同步服务器）。stdout 同上（节点数是 0：节点各台设备自己有）
pub fn cmd_team_import(args: &[String]) -> R<()> {
    arity("team-import", args, 2, 3)?;
    let (t, a, _lk) = admin_of(&args[0])?;
    if args[1] == "--mine" {
        if args.len() != 2 {
            return fail("usage", "用法: cca team import Team --mine");
        }
        let config = a.dict("config").unwrap_or_default();
        let accounts = config.dict("accounts").unwrap_or_default();
        let aliases = config.dict("aliases").unwrap_or_default();
        let mut count = 0;
        for tool in tools().iter() {
            let Some(table) = load_quiet(&tool.table).and_then(|j| j.as_dict()) else { continue };
            let src = policy_sources(tool);
            let m = accounts.dict(tool.name).unwrap_or_default();
            for (k, v) in table.items() {
                let node = node_of(&v);
                if route(&src, &k, &v) != Some(HubKind::Own) || node.is_empty() {
                    continue;
                }
                let (alias, email) = parse_key(&k);
                let email = email.to_lowercase();
                m.set(email.as_str(), &*node);
                if !alias.is_empty() && !aliases.has(&email) {
                    aliases.set(email.as_str(), alias.as_str());
                }
                count += 1;
            }
            accounts.set(tool.name, m);
        }
        if count == 0 {
            return fail("file", "这台设备上没有自己的官方账号可加（维护者共享的、别的 Team 给的、第三方 API 账号都不算）");
        }
        config.set("accounts", accounts);
        config.set("aliases", aliases);
        a.set("config", config);
        admin_save(&t.id, &a)?;
        let stored = if t.attached() { settle(&t, &a, None)? } else { 0 };
        out!("{count}\n0\n{stored}");
        return Ok(());
    }
    let read = |p: &str| load_quiet(p).filter(|j| j.is_dict());
    let Some(policy) = read(&args[1]) else { return fail("file", &format!("{} 读不了，或者不是 JSON 对象", args[1])) };
    if let Some(why) = policy_problems(&policy, false).into_iter().next() {
        return fail("file", &format!("{} 的格式不对：{why}", args[1]));
    }
    let policy = policy.as_dict().expect("刚查过是对象");
    let accounts = Dict::new();
    let mut count = 0;
    for tool in tools().iter() {
        let m = Dict::new();
        for (e, n) in policy.dict(tool.name).unwrap_or_default().items() {
            if n.as_str().is_some_and(|n| !n.is_empty()) {
                m.set(e.to_lowercase(), n);
                count += 1;
            }
        }
        accounts.set(tool.name, m);
    }
    let aliases = Dict::new();
    for (e, alias) in policy.dict("aliases").unwrap_or_default().items() {
        aliases.set(e.to_lowercase(), alias);
    }
    let mut nodes = V::None;
    let mut node_count = 0;
    if let Some(p) = args.get(2) {
        let Some(j) = read(p) else { return fail("file", &format!("{p} 读不了，或者不是 JSON 对象")) };
        if let Some(why) = fill_problems(&j).into_iter().next() {
            return fail("file", &format!("{p} 的格式不对：{why}"));
        }
        let j = j.as_dict().expect("刚查过是对象");
        node_count = j.list("proxies").map_or(0, |l| l.len());
        nodes = V::Dict(dict! { "group" => j.at("group"), "proxies" => j.at("proxies") });
    }
    let config = dict! {
        "accounts" => accounts, "aliases" => aliases, "install_node" => policy.str("install_node").map_or(String::new(), |n| n.to_string()),
        "nodes" => nodes,
    };
    a.set("config", config);
    admin_save(&t.id, &a)?;
    let stored = if t.attached() { settle(&t, &a, None)? } else { 0 };
    out!("{count}\n{node_count}\n{stored}");
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
    admin_save(&t.id, &a)?;
    let stored = if t.attached() { settle(&t, &a, None)? } else { 0 };
    out!("{stored}");
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
