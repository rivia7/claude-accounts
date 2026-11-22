//! 配对过的机器：`~/.cca/remote/peers.json`（600，没有密钥）记着每个配对的编号、对方的指纹和名字、谁显示的配对码、
//! 对方能不能接入这台机器的会话；配对出来的密钥存在钥匙串里（服务名 `cca-remote-<配对编号>`；没有钥匙串的系统是
//! `~/.cca/secrets/` 下的文件），存的是 64 个十六进制字符。

use super::local;
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
    /// 对方的设备指纹（它的 deploy key 的）
    pub fp: String,
    /// 对方给自己起的名字（它的机器名）
    pub name: String,
    pub incoming: Incoming,
    /// 配对码是这台机器显示的（握手、派生密钥时两台机器的先后照它）
    pub shown: bool,
    pub at: f64,
    /// 对方是手机上的网页端（`cca remote pair --web` 配的、不在服务器的设备清单里）：只是给人看的，它能做什么照旧看 incoming
    pub web: bool,
}

fn file() -> String {
    path::join(&local::dir(), "peers.json")
}

/// 配对表文件现在的样子（修改时间、大小、inode）：变了就是有人改过。没有这个文件是 None
pub type Stamp = Option<(i64, i64, u64, u64)>;

pub fn stamp() -> Stamp {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(file()).ok()?;
    Some((m.mtime(), m.mtime_nsec(), m.size(), m.ino()))
}

/// 32 个小写十六进制字符（配对、会话的编号，令牌）
pub fn hex32(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

pub fn load() -> Vec<Peer> {
    let Some(peers) = fs::load_quiet(&file()).and_then(|j| j.as_dict()).and_then(|j| j.dict("peers")) else { return Vec::new() };
    let mut out = Vec::new();
    for (pid, v) in peers.items() {
        let Some(d) = v.as_dict() else { continue };
        let peer = (|| {
            Some(Peer {
                pid: pid.to_string(),
                fp: d.str("fp")?.to_string(),
                name: d.str("name")?.to_string(),
                incoming: Incoming::parse(&d.str("incoming")?)?,
                shown: d.at("shown").as_bool()?,
                at: d.at("at").as_num()?,
                web: d.at("web").as_bool() == Some(true),
            })
        })();
        if let Some(p) = peer.filter(|p| hex32(&p.pid)) {
            out.push(p);
        }
    }
    out.sort_by(|a, b| a.at.total_cmp(&b.at));
    out
}

pub fn save(peers: &[Peer]) -> R<()> {
    let d = local::ensure_dir()?;
    let all = Dict::new();
    for p in peers {
        let row =
            dict! { "fp" => p.fp.as_str(), "name" => p.name.as_str(), "incoming" => p.incoming.word(), "shown" => p.shown, "at" => p.at };
        if p.web {
            row.set("web", true); // 只有网页端的才写这一项：别的机器的那几行和以前逐字节一样
        }
        all.set(p.pid.as_str(), row);
    }
    fs::save_json(&path::join(&d, "peers.json"), &V::Dict(dict! { "v" => 1, "peers" => all }), Some(0o600), true)
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
