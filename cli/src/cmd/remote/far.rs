//! 别的机器上的会话：宿主那头把每个会话的说明封给各个配对（`dir_doc`，服务器存着转交、看不到里面），这一头问服务器
//! 要来、用配对的密钥打开（`fetch`）。说明是对方写的：显示之前去掉控制字符。

use super::crypto;
use super::local::Meta;
use super::peers::{self, Incoming, Peer};
use super::relay::{self, Fault, Server};
use crate::cmd::hub::HubKind;
use crate::cmd::settings::home_tilde;
use crate::dict;
use crate::py::obj::{Dict, List};
use crate::py::time::strftime_local;
use crate::py::R;
use std::collections::HashSet;

/// 别的机器上的一个会话
#[derive(Clone, Debug)]
pub struct Far {
    pub sid: String,
    /// 是哪个配对的那台机器上的
    pub pid: String,
    /// 那台机器的名字（配对时记下的）
    pub peer: String,
    pub tool: String,
    pub name: String,
    pub cwd: String,
    pub since: f64,
    pub viewers: i64,
    /// 那台机器现在连着服务器
    pub online: bool,
    /// 那边只让看
    pub view_only: bool,
}

/// 去掉控制字符和看不见的排版字符（改变文字方向的、零宽的、换行换段的、软连字符、标签字符：能让一行字看起来是另一个意思）、
/// 限制长度。对方写的、服务器转交的文字都先过这一道再显示
pub fn clean(s: &str, max: usize) -> String {
    let hidden = |c: char| {
        matches!(c, '\u{ad}' | '\u{61c}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}'
            | '\u{e0000}'..='\u{e007f}')
    };
    s.chars().filter(|c| !c.is_control() && !hidden(*c)).take(max).collect()
}

/// 账号的短名字：别名；没起别名的账号名字就是邮箱，只给 @ 前面那一截
fn short_name(name: &str) -> &str {
    match name.split_once('@') {
        Some((head, _)) if !head.is_empty() => head,
        _ => name,
    }
}

/// 对方说的“什么时候启动的”：不是个像样的时间（不是数、无穷大、负的、远得离谱）就当没有（0）。无穷大、NaN 写进给界面 App
/// 的 JSON 里是 `Infinity`、`NaN`，App 整份都解不开
fn sane_time(t: f64) -> f64 {
    if t.is_finite() && t > 0.0 && t < 1e11 {
        t
    } else {
        0.0
    }
}

/// 这台机器上的会话给一个配对看的说明。账号只给短名字（别名；没起别名的给邮箱 @ 前面那一截），不给邮箱；各项和显示时一样限制长度
/// （封好的密文太长的话服务器不收，整条说明都发不出去）
pub fn dir_doc(meta: &Meta, access: Incoming) -> Dict {
    dict! {
        "v" => 1, "machine" => peers::machine_name(), "tool" => clean(&meta.tool, 16), "name" => clean(short_name(&meta.name), 64),
        "cwd" => clean(&home_tilde(&meta.cwd), 200), "since" => meta.since, "access" => access.word()
    }
}

/// 列表里的一行（和这台机器上的会话一个样子，最后说在哪台机器上）
pub fn row(f: &Far) -> String {
    let mut note = format!("在「{}」上", f.peer);
    if f.view_only {
        note.push_str("，只能看");
    }
    if f.viewers > 0 {
        note.push_str(&format!("，{} 处接入", f.viewers));
    }
    if !f.online {
        note.push_str("，那台机器暂时连不上");
    }
    let when = if f.since > 0.0 { format!("{} 开始  ", strftime_local("%m-%d %H:%M", f.since)) } else { String::new() };
    format!("{}  {}  {}  {}  {}{}", &f.sid[..8], f.tool, f.name, f.cwd, when, note)
}

pub struct Listing {
    pub sessions: Vec<Far>,
    /// 服务器上还在的配对（不在里面的：对方取消了配对，或者太久没用被服务器清掉了）
    pub known: HashSet<String>,
}

/// 问服务器：配对过的各台机器上现在有哪些可以接入的会话。打不开的说明（不是那台机器用配对的密钥封的）不算
pub fn fetch(srv: &Server, peers: &[Peer], timeout: i64) -> R<Result<Listing, Fault>> {
    let pids = List::new();
    for p in peers.iter().take(16) {
        pids.push(p.pid.as_str());
    }
    let j = match srv.post("relay_list", &dict! { "pids" => pids }, timeout)? {
        Ok(j) => j,
        Err(f) => return Ok(Err(f)),
    };
    let known: HashSet<String> =
        j.dict("peers").map(|d| d.items().into_iter().map(|(k, _)| k.to_string()).collect()).unwrap_or_default();
    let mut sessions = Vec::new();
    for (pid, rows) in j.dict("sessions").map(|d| d.items()).unwrap_or_default() {
        let Some(peer) = peers.iter().find(|p| *p.pid == *pid) else { continue };
        let rows = rows.as_list().map(|l| l.items()).unwrap_or_default();
        if rows.is_empty() {
            continue;
        }
        let Some(secret) = peers::secret(&peer.pid) else { continue };
        for r in rows {
            let Some(r) = r.as_dict() else { continue };
            let Some(sid) = r.str("sid").filter(|s| peers::hex32(s)) else { continue };
            let Some(doc) = r.get("blob").and_then(|b| crypto::dir_open(&secret, &peer.pid, &sid, &b)) else { continue };
            let text = |k: &str, max: usize| doc.str(k).map(|s| clean(&s, max)).unwrap_or_default();
            sessions.push(Far {
                sid: sid.to_string(),
                pid: peer.pid.clone(),
                peer: peer.name.clone(),
                tool: text("tool", 16),
                name: text("name", 64),
                cwd: text("cwd", 200),
                since: sane_time(doc.at("since").as_num().unwrap_or(0.0)),
                viewers: r.at("viewers").as_int().unwrap_or(0).clamp(0, 99),
                online: r.at("online").as_bool().unwrap_or(false),
                view_only: doc.str("access").as_deref() == Some("view"),
            });
        }
    }
    sessions.sort_by(|a, b| a.since.total_cmp(&b.since));
    Ok(Ok(Listing { sessions, known }))
}

/// 各台服务器上问来的合在一起
pub struct Seen {
    pub sessions: Vec<Far>,
    /// 问到了的那几台服务器上还在的配对
    pub known: HashSet<String>,
    /// 它的服务器这次问到了的配对（不在 known 里的：对方取消了；不在这里面的：这次不知道）
    pub asked: HashSet<String>,
    /// 没问到的服务器：是哪台、原因、服务器回的（没问出去的是 None）
    pub errors: Vec<(HubKind, String, Option<Fault>)>,
}

/// 配对记在哪台服务器上就问哪台：每台只问它上面的那几个配对
pub fn fetch_all(peers: &[Peer], timeout: i64) -> R<Seen> {
    let mut seen = Seen { sessions: Vec::new(), known: HashSet::new(), asked: HashSet::new(), errors: Vec::new() };
    let mut hubs: Vec<HubKind> = Vec::new();
    for p in peers {
        if !hubs.contains(&p.hub) {
            hubs.push(p.hub);
        }
    }
    for hub in hubs {
        let group: Vec<Peer> = peers.iter().filter(|p| p.hub == hub).cloned().collect();
        let srv = match relay::server_of(hub)? {
            Ok(s) => s,
            Err(why) => {
                seen.errors.push((hub, why, None));
                continue;
            }
        };
        match fetch(&srv, &group, timeout)? {
            Ok(l) => {
                seen.sessions.extend(l.sessions);
                seen.known.extend(l.known);
                seen.asked.extend(group.iter().map(|p| p.pid.clone()));
            }
            Err(f) => seen.errors.push((hub, f.text.clone(), Some(f))),
        }
    }
    seen.sessions.sort_by(|a, b| a.since.total_cmp(&b.since));
    Ok(seen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_from_the_other_side_is_cleaned() {
        assert_eq!(clean("a\x1b[31mb\r\nc\u{9b}d", 10), "a[31mbcd");
        assert_eq!(clean("ab\u{202e}dc\u{200b}\u{2066}e\u{feff}", 10), "abdce", "改文字方向的、零宽的也去掉");
        assert_eq!(clean("a\u{2028}b\u{2029}c\u{ad}d\u{e0041}e\u{e007f}f", 10), "abcdef", "换行换段的、软连字符、标签字符也去掉");
        assert_eq!(clean("很长的名字很长的名字", 4), "很长的名");
        let f = Far {
            sid: "0123456789abcdef0123456789abcdef".into(),
            pid: "ab".repeat(16),
            peer: "服务器".into(),
            tool: "claude".into(),
            name: "工作".into(),
            cwd: "~/项目".into(),
            since: 0.0,
            viewers: 2,
            online: false,
            view_only: true,
        };
        let r = row(&f);
        assert_eq!(
            r, "01234567  claude  工作  ~/项目  在「服务器」上，只能看，2 处接入，那台机器暂时连不上",
            "没有启动时间的不写时间"
        );
        let r = row(&Far { since: 1794000000.0, ..f });
        assert!(r.starts_with("01234567  claude  工作  ~/项目  ") && r.contains(" 开始  在「服务器」上"), "{r}");
    }

    #[test]
    fn times_from_the_other_side_are_checked() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 0.0, 1e300] {
            assert_eq!(sane_time(bad), 0.0, "{bad}");
        }
        assert_eq!(sane_time(1794000000.5), 1794000000.5);
    }

    #[test]
    fn directory_entry_hides_the_email() {
        let meta = Meta {
            sid: "0123456789abcdef0123456789abcdef".into(),
            pid: 1,
            tool: "codex".into(),
            key: "工作(alice@example.com)".into(),
            name: "工作".into(),
            cwd: format!("{}/proj", crate::core::base::HOME.as_str()),
            since: 1794000000.0,
        };
        let d = dir_doc(&meta, Incoming::View);
        assert_eq!(d.str("cwd").as_deref(), Some("~/proj"));
        assert_eq!(d.str("access").as_deref(), Some("view"));
        assert!(!crate::py::json::dumps(&crate::py::obj::V::Dict(d), crate::py::json::LINE).contains("example.com"));
        // 没起别名的账号，名字就是邮箱：只给 @ 前面那一截
        let d = dir_doc(&Meta { name: "alice@example.com".into(), key: "alice@example.com".into(), ..meta }, Incoming::Control);
        assert_eq!(d.str("name").as_deref(), Some("alice"));
        assert!(!crate::py::json::dumps(&crate::py::obj::V::Dict(d), crate::py::json::LINE).contains("example.com"));
        assert_eq!((short_name("工作"), short_name("@x"), short_name("")), ("工作", "@x", ""));
    }
}
