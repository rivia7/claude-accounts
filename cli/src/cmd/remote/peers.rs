//! 配对过的机器：`~/.cca/remote/peers.json`（600，没有密钥）记着每个配对的编号、对方的指纹和名字、谁显示的配对码、
//! 对方能不能接入这台机器的会话；配对出来的密钥存在钥匙串里（服务名 `cca-remote-<配对编号>`；没有钥匙串的系统是
//! `~/.cca/secrets/` 下的文件），存的是 64 个十六进制字符。

use super::local;
use crate::cmd::hub::{HubKind, TeamTag};
use crate::cmd::secrets::{ks_del, ks_read, ks_write};
use crate::dict;
use crate::py::fs;
use crate::py::hash::{from_hex, to_hex};
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::R;

/// 对方能对这台机器上的会话做什么
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Incoming {
    /// 看和操作
    Control,
    /// 只看
    View,
    /// 不让接入（这台机器的会话也不告诉它）
    None,
}

impl Incoming {
    pub fn parse(s: &str) -> Option<Incoming> {
        match s {
            "control" => Some(Incoming::Control),
            "view" => Some(Incoming::View),
            "none" => Some(Incoming::None),
            _ => None,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Incoming::Control => "control",
            Incoming::View => "view",
            Incoming::None => "none",
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            Incoming::Control => "可以看和操作这台机器的会话",
            Incoming::View => "只能看这台机器的会话",
            Incoming::None => "不能接入这台机器",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Peer {
    /// 配对的编号（32 个十六进制字符）
    pub pid: String,
    /// 对方的设备指纹（它在服务器上的设备密钥的）
    pub fp: String,
    /// 对方给自己起的名字（它的机器名）
    pub name: String,
    pub incoming: Incoming,
    /// 配对码是这台机器显示的（握手、派生密钥时两台机器的先后照它）
    pub shown: bool,
    pub at: f64,
    /// 对方是手机上的网页端（`cca remote pair --web` 配的、不在服务器的设备清单里）：只是给人看的，它能做什么照旧看 incoming
    pub web: bool,
    /// 这个配对是在哪台服务器上做的（记在它上面）：经它只看得到走这台服务器的会话——某个 Team 的同步服务器
    pub hub: HubKind,
    /// 对方能用这台机器上的模板新建会话（远程启动，launch.rs；`cca remote allow 名字 launch on`）。只有手机、看和操作都让的
    /// 才算（读的时候不是这样的当它没开）；配对表里只在开着时写这一项
    pub launch: bool,
}

/// 配对表里 hub 这一项（Team 的编号）是哪台服务器。没有这一项的是以前在维护者那一台上配的：3.6.0 起那台服务器没了，
/// 这一项不算（old_pids）
fn hub_of(word: Option<&str>) -> Option<HubKind> {
    word.and_then(TeamTag::new).map(HubKind::Team)
}

/// 配对表里以前在维护者那一台上配的（没有 hub 这一项，load 不认了）的编号：写回配对表、cca restore 时把它们的密钥也删掉，
/// 不然钥匙串里一直留着
fn old_pids() -> Vec<String> {
    let Some(peers) = fs::load_quiet(&file()).and_then(|j| j.as_dict()).and_then(|j| j.dict("peers")) else { return Vec::new() };
    let old = |v: &V| v.as_dict().is_some_and(|d| d.get("hub").is_none_or(|h| h.is_none()));
    peers.items().into_iter().filter(|(pid, v)| hex32(pid) && old(v)).map(|(pid, _)| pid.to_string()).collect()
}

/// 配对表里还有以前在维护者那一台上配的（见 old_pids）
pub fn has_old() -> bool {
    !old_pids().is_empty()
}

fn file() -> String {
    path::join(&local::dir(), "peers.json")
}

/// 配对表文件现在的样子（修改时间、大小、inode）：变了就是有人改过。没有这个文件是 None
pub type Stamp = Option<(i64, i64, u64, u64)>;

pub fn stamp() -> Stamp {
    stamp_of(&file())
}

/// 一个文件现在的样子（同上：模板、cca 自己也这样看变了没有）
pub fn stamp_of(f: &str) -> Stamp {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(f).ok()?;
    Some((m.mtime(), m.mtime_nsec(), m.size(), m.ino()))
}

/// 32 个小写十六进制字符（配对、会话的编号，令牌）
pub fn hex32(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// 配对表里的一项；缺了该有的、样子不对是 None（这一项不算）
fn row_of(pid: &str, d: &Dict) -> Option<Peer> {
    Some(Peer {
        pid: pid.to_string(),
        fp: d.str("fp")?.to_string(),
        name: d.str("name")?.to_string(),
        incoming: Incoming::parse(&d.str("incoming")?)?,
        shown: d.get("shown")?.as_bool()?,
        at: d.get("at")?.as_num()?,
        // 两台机器之间的配对、旧版写的配对表没有这一项（`at` 取不存在的键会崩：用 `get`）
        web: d.get("web").and_then(|v| v.as_bool()) == Some(true),
        // 没有这一项（维护者那一台上的旧配对）、写了却看不懂的（以后的版本加的别的种类）：这一项不算
        hub: hub_of(d.get("hub").filter(|v| !v.is_none()).map(|v| v.py_str()).as_deref())?,
        launch: false,
    })
    .map(|mut p| {
        p.launch = d.get("launch").and_then(|v| v.as_bool()) == Some(true) && p.web && p.incoming == Incoming::Control;
        p
    })
}

pub fn load() -> Vec<Peer> {
    let Some(peers) = fs::load_quiet(&file()).and_then(|j| j.as_dict()).and_then(|j| j.dict("peers")) else { return Vec::new() };
    let mut out = Vec::new();
    for (pid, v) in peers.items() {
        let Some(d) = v.as_dict() else { continue };
        if let Some(p) = row_of(&pid, &d).filter(|p| hex32(&p.pid)) {
            out.push(p);
        }
    }
    out.sort_by(|a, b| a.at.total_cmp(&b.at));
    out
}

pub fn save(peers: &[Peer]) -> R<()> {
    let d = local::ensure_dir()?;
    // 维护者那一台上的旧配对（load 不认了）写回去就没有了：它们的密钥也删掉
    let old = old_pids();
    let all = Dict::new();
    for p in peers {
        let row = dict! { "fp" => p.fp.as_str(), "name" => p.name.as_str(), "incoming" => p.incoming.word(), "shown" => p.shown, "at" => p.at };
        if p.web {
            row.set("web", true); // 只有网页端的才写这一项：别的机器的那几行和以前逐字节一样
        }
        if p.launch {
            row.set("launch", true); // 同样只在开着时写（旧版的 cca 写回配对表时丢掉它：等于关上）
        }
        row.set("hub", p.hub.team());
        all.set(p.pid.as_str(), row);
    }
    fs::save_json(&path::join(&d, "peers.json"), &V::Dict(dict! { "v" => 1, "peers" => all }), Some(0o600), true)?;
    for pid in old {
        if let Err(e) = del_secret(&pid) {
            e.swallowed(); // 删不掉的只是留在钥匙串里，配对表里已经没有它了
        }
    }
    Ok(())
}

fn svc(pid: &str) -> String {
    format!("cca-remote-{pid}")
}

/// 这个配对的密钥；没有、读不了是 None
pub fn secret(pid: &str) -> Option<Vec<u8>> {
    let (rc, s) = ks_read(&svc(pid), "pair");
    if rc != 0 {
        return None;
    }
    from_hex(&s).filter(|p| p.len() == 32)
}

/// 存这个配对的密钥；false：没存进去（读回来对不上）
pub fn put_secret(pid: &str, p: &[u8]) -> R<bool> {
    ks_write(&svc(pid), "pair", &to_hex(p))
}

pub fn del_secret(pid: &str) -> R<()> {
    ks_del(&svc(pid), Some("pair"))
}

/// 按名字或者配对编号的前几位找：正好一个才算（名字一样的两台要用编号分）
pub fn find<'a>(peers: &'a [Peer], who: &str) -> Result<&'a Peer, String> {
    let hits: Vec<&Peer> =
        peers.iter().filter(|p| p.name == who || (who.len() >= 4 && p.pid.starts_with(&who.to_lowercase()))).collect();
    match hits.as_slice() {
        [one] => Ok(one),
        [] => Err(format!("没有叫「{who}」的配对（cca remote peers 看有哪些）")),
        _ => Err(format!("有 {} 台都叫「{who}」：用配对编号的前几位（cca remote peers 里第一列）", hits.len())),
    }
}

/// 这台机器的短名字（给配对的另一台看）
pub fn machine_name() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: buf 够长，gethostname 写进去的是以 0 结尾的字符串（太长被截断时不保证结尾，所以最后一个字节留着是 0）
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len() - 1) } == 0;
    let end = buf.iter().position(|b| *b == 0).unwrap_or(0);
    let full = String::from_utf8_lossy(&buf[..end]).into_owned();
    let short = full.split('.').next().unwrap_or("").trim().to_string();
    if ok && !short.is_empty() {
        short.chars().take(48).collect()
    } else {
        "cca".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows() {
        let pid = "ab".repeat(16);
        let tid = "0123456789abcdef0123456789abcdef";
        let full = dict! { "fp" => "SHA256:x", "name" => "box", "incoming" => "view", "shown" => true, "at" => 5.0, "hub" => tid };
        let p = row_of(&pid, &full).unwrap();
        assert_eq!(
            (p.incoming, p.shown, p.at, p.web),
            (Incoming::View, true, 5.0, false),
            "没有 web 这一项：不是网页端（两台机器之间的配对都这样）"
        );
        assert_eq!(p.hub.team(), tid);
        full.pop("hub");
        assert!(row_of(&pid, &full).is_none(), "没有 hub 这一项：维护者那一台上的旧配对，3.6.0 起不算");
        full.set("hub", V::None);
        assert!(row_of(&pid, &full).is_none(), "hub 是 null 也一样");
        full.set("hub", "something-new");
        assert!(row_of(&pid, &full).is_none(), "看不懂的 hub：这一项不算");
        full.set("hub", tid);
        full.set("web", true);
        assert!(row_of(&pid, &full).unwrap().web);
        full.set("web", "yes");
        assert!(!row_of(&pid, &full).unwrap().web, "不是 true 的都不算");
        // 远程启动：只有手机、看和操作都让的才算开着
        full.set("launch", true);
        assert!(!row_of(&pid, &full).unwrap().launch, "不是手机");
        full.set("web", true);
        assert!(!row_of(&pid, &full).unwrap().launch, "只让看");
        full.set("incoming", "control");
        assert!(row_of(&pid, &full).unwrap().launch);
        full.set("launch", 1);
        assert!(!row_of(&pid, &full).unwrap().launch, "不是 true 的都不算");
        full.pop("launch");
        assert!(!row_of(&pid, &full).unwrap().launch);
        full.set("incoming", "view");
        full.set("web", "yes");
        // 缺了该有的、样子不对：这一项不算，不崩
        for (k, bad) in [("fp", V::None), ("incoming", "all".into()), ("shown", "x".into()), ("at", "x".into())] {
            let d = dict! { "fp" => "SHA256:x", "name" => "box", "incoming" => "view", "shown" => true, "at" => 5.0, "hub" => tid };
            assert!(row_of(&pid, &d).is_some());
            d.set(k, bad);
            assert!(row_of(&pid, &d).is_none(), "{k}");
        }
        for k in ["fp", "name", "incoming", "shown", "at"] {
            let d = Dict::new();
            for (kk, v) in full.items() {
                if &*kk != k {
                    d.set(kk, v);
                }
            }
            assert!(row_of(&pid, &d).is_none(), "少了 {k}");
        }
    }

    #[test]
    fn words_and_lookup() {
        for i in [Incoming::Control, Incoming::View, Incoming::None] {
            assert_eq!(Incoming::parse(i.word()), Some(i));
        }
        assert_eq!(Incoming::parse("all"), None);
        let peer = |pid: &str, name: &str| Peer {
            pid: pid.repeat(32 / pid.len()),
            fp: "SHA256:x".into(),
            name: name.into(),
            incoming: Incoming::Control,
            shown: true,
            at: 1.0,
            web: false,
            hub: HubKind::Team(TeamTag::new(&"01".repeat(16)).expect("是个 Team 的编号")),
            launch: false,
        };
        let peers = vec![peer("ab", "服务器"), peer("cd", "服务器"), peer("ef", "笔记本")];
        assert_eq!(find(&peers, "笔记本").unwrap().pid, "ef".repeat(16));
        assert_eq!(find(&peers, "CDCD").unwrap().pid, "cd".repeat(16));
        assert!(find(&peers, "服务器").unwrap_err().contains("有 2 台"));
        assert!(find(&peers, "没有").unwrap_err().contains("没有叫"));
        assert!(find(&peers, "ab").is_err(), "编号至少给 4 位");
        assert!(!machine_name().is_empty() && !machine_name().contains('.'));
        assert!(hex32(&"0a".repeat(16)) && !hex32(&"0A".repeat(16)) && !hex32("../x"));
    }
}
