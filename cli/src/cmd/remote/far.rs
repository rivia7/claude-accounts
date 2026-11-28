//! 别的机器上的会话：宿主那头把每个会话的说明封给各个配对（`dir_doc`，服务器存着转交、看不到里面），这一头问服务器
//! 要来、用配对的密钥打开（`fetch`）。说明是对方写的：显示之前去掉控制字符。
//!
//! 那台机器允许这台机器远程启动时（launch.rs），它的启动器也在里面：在服务器上登记成一个“会话”，说明用另一把密钥封
//! （`crypto::launch_open`，server/README.md「启动器」），打开了是一份模板的清单（`FarLauncher`）。

use super::crypto;
use super::local::Meta;
use super::peers::{self, Incoming, Peer};
use super::relay::{self, Fault, Server};
use crate::cmd::hub::HubKind;
use crate::cmd::presence::presence_aid;
use crate::cmd::settings::home_tilde;
use crate::core::model::parse_key;
use crate::core::tool::tools;
use crate::dict;
use crate::py::fs::load_quiet;
use crate::py::obj::{Dict, List};
use crate::py::time::strftime_local;
use crate::py::R;
use std::collections::{HashMap, HashSet};

/// 别的机器上的一个会话
#[derive(Clone, Debug)]
pub struct Far {
    pub sid: String,
    /// 是哪个配对的那台机器上的
    pub pid: String,
    /// 那台机器的名字（配对时记下的）
    pub peer: String,
    pub tool: String,
    /// 账号的名字：这台机器上也有这个账号（`aid` 对得上）时是这台机器上的别名，否则是那台机器说的
    pub name: String,
    /// 那台机器说的名字（它那里的别名）
    pub host_name: String,
    /// 那台机器给的账号摘要（`presence_aid`，用这个配对的 Team 的盐算的；旧版的 cca 不给：空的）
    pub aid: String,
    pub cwd: String,
    pub since: f64,
    pub viewers: i64,
    /// 那台机器现在连着服务器
    pub online: bool,
    /// 连着服务器、宿主却有一阵没说话了（服务器给的 `busy`：被挂起了、机器睡着了），这会儿接不上；旧的服务器不给，当它有回应
    pub busy: bool,
    /// 那边只让看
    pub view_only: bool,
}

/// 别的机器上的启动器（远程启动：那台机器允许这台机器用它的模板新建会话，start.rs）
#[derive(Clone, Debug)]
pub struct FarLauncher {
    /// 它在服务器上登记成的那个会话的编号：进启动器要的是它的票据
    pub sid: String,
    /// 是哪个配对的那台机器上的
    pub pid: String,
    /// 那台机器的名字（配对时记下的）
    pub peer: String,
    /// 启动器这会儿连着服务器
    pub online: bool,
    /// 连着服务器、却有一阵没说话了（同 `Far::busy`）：这会儿起不了会话
    pub busy: bool,
    /// 说明里放不下的模板还有几个
    pub more: i64,
    pub templates: Vec<FarTemplate>,
    /// 起会话时能挑的账号（3.7.10 起：模板不带账号）；那台机器上的 cca 是旧版、说明里没有这一项时是 None（模板用它自己的账号）
    pub accounts: Option<Vec<FarAccount>>,
}

/// 启动器的说明里起会话时能挑的一个账号（清理过：和 iOS 的 Launch.swift 的 `launchOpen` 一样）
#[derive(Clone, Debug, PartialEq)]
pub struct FarAccount {
    /// 编号（16 个小写的十六进制字符）：按这个配对算的，起会话时说用它（LAUNCH 的 `a`）
    pub id: String,
    /// claude、codex、agy
    pub tool: String,
    /// 名字：这台机器上也有这个账号（`aid` 对得上）时是这台机器上的别名，否则是那台机器说的（短名字）
    pub name: String,
    /// 那台机器说的名字
    pub host_name: String,
    /// 账号摘要（同会话的说明；那台机器不在 Team 里、账号没有邮箱时是空的）
    pub aid: String,
}

/// 启动器的说明里的一个模板（清理过：和 iOS 的 Launch.swift 的 `launchOpen` 一样）
#[derive(Clone, Debug, PartialEq)]
pub struct FarTemplate {
    /// 编号（8 个小写的十六进制字符）
    pub id: String,
    /// 摘要（16 个小写的十六进制字符）：起的时候带回去，那台机器上改过了就对不上（`changed`）
    pub digest: String,
    pub name: String,
    /// claude、codex、agy
    pub tool: String,
    /// 账号的名字（3.7.9 及以前的模板带着账号；新的说明里这是给旧版看的：能挑的只有一个时是它，几个时是「启动时选账号」）：这台机器上
    /// 也有这个账号（`aid` 对得上）时是这台机器上的别名，否则是那台机器说的（短名字）
    pub account: String,
    /// 那台机器说的账号的名字
    pub host_account: String,
    /// 账号摘要（同会话的说明；那台机器不在 Team 里时是空的）
    pub aid: String,
    /// 文件夹（`~/…` 的样子）
    pub dir: String,
    /// 参数（给人看的一行，太长的截断了）
    pub args: String,
    /// 能不能带开场白
    pub line: bool,
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
pub(super) fn short_name(name: &str) -> &str {
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

/// 一个 Team 的盐（设备统计里认账号用的那个，Team 里的设备都有）；这台设备没在这个 Team 里、还没同步到配置时没有
pub fn team_salt(hub: HubKind) -> Option<String> {
    let id = hub.team();
    crate::cmd::team::hub_cfgs().into_iter().find(|(t, _)| *t == id).map(|(_, c)| c.at("salt").py_str()).filter(|s| !s.is_empty())
}

/// 账号摘要（和设备统计的一样：同一个 Team 里的设备各自算得出来，不知道邮箱的算不出邮箱）；没有邮箱的账号没有
pub(super) fn aid_of(salt: &str, tool: &str, key: &str) -> Option<String> {
    let email = parse_key(key).1;
    if email.is_empty() {
        return None;
    }
    presence_aid(salt, tool, &email).ok()
}

/// 这台机器上的账号：(工具, 账号摘要) → 名字（别名；没起别名的是邮箱 @ 前面那一截，和 dir_doc 给的一样短）
fn local_names(salt: &str) -> HashMap<(String, String), String> {
    let mut out = HashMap::new();
    for t in tools() {
        let Some(table) = load_quiet(&t.table).and_then(|v| v.as_dict()) else { continue };
        for (key, _) in table.items() {
            let key = key.to_string();
            if let Some(aid) = aid_of(salt, t.name, &key) {
                out.insert((t.name.to_string(), aid), short_name(&crate::core::model::name_of(&key)).to_string());
            }
        }
    }
    out
}

/// 这台机器上的会话给一个配对看的说明。账号只给短名字（别名；没起别名的给邮箱 @ 前面那一截）和账号摘要（`salt`：这个配对的 Team 的盐；
/// 接入方用它认出自己那里的同一个账号、显示自己起的别名），不给邮箱；各项和显示时一样限制长度（封好的密文太长的话服务器不收，整条说明都发不出去）
pub fn dir_doc(meta: &Meta, access: Incoming, salt: Option<&str>) -> Dict {
    let d = dict! {
        "v" => 1, "machine" => peers::machine_name(), "tool" => clean(&meta.tool, 16), "name" => clean(short_name(&meta.name), 64),
        "cwd" => clean(&home_tilde(&meta.cwd), 200), "since" => meta.since, "access" => access.word()
    };
    if let Some(aid) = salt.and_then(|s| aid_of(s, &meta.tool, &meta.key)) {
        d.set("aid", aid);
    }
    // 那一头结束得了它（END）：无头的会话（远程启动的）、能操作的配对；别的会话只在这台机器上结束得了（转接进程不替别的机器转）
    if meta.headless && access == Incoming::Control {
        d.set("end", true);
    }
    d
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
    } else if f.busy {
        note.push_str("，那台机器没有回应");
    }
    let when = if f.since > 0.0 { format!("{} 开始  ", strftime_local("%m-%d %H:%M", f.since)) } else { String::new() };
    format!("{}  {}  {}  {}  {}{}", &f.sid[..8], f.tool, f.name, f.cwd, when, note)
}

pub struct Listing {
    pub sessions: Vec<Far>,
    /// 服务器上还在的配对（不在里面的：对方取消了配对，或者太久没用被服务器清掉了）
    pub known: HashSet<String>,
    /// 允许这台机器远程启动的那几台的启动器（一个配对最多一个：连着服务器的优先）
    pub launchers: Vec<FarLauncher>,
}

/// 打开了的启动器的说明（`launcher_of`）
struct Opened {
    templates: Vec<FarTemplate>,
    /// 放不下的模板的个数
    more: i64,
    accounts: Option<Vec<FarAccount>>,
}

/// 启动器的说明（打开了的明文）里的模板：编号、摘要、工具不是那个样子的不要，编号和前面留下的重复的不要（留头一个）；字照会话的说明那样
/// 清理、限长度。能挑的账号（`accounts`）同样：编号、工具不是那个样子的、编号重复的不要；没有这一项（旧版的 cca）是 None。
/// 不是 v1 的是 None
fn launcher_of(doc: &Dict) -> Option<Opened> {
    if doc.get("v").and_then(|v| v.as_int()) != Some(1) {
        return None;
    }
    let accounts = doc.get("accounts").map(|v| {
        let mut out: Vec<FarAccount> = Vec::new();
        for a in v.as_list().map(|l| l.items()).unwrap_or_default() {
            let Some(a) = a.as_dict() else { continue };
            let id = a.str("i").filter(|s| peers::lower_hex(s, 16));
            let tool = a.str("k").filter(|k| matches!(&**k, "claude" | "codex" | "agy"));
            let (Some(id), Some(tool)) = (id, tool) else { continue };
            if out.iter().any(|x| *x.id == *id) {
                continue;
            }
            let name = a.str("n").map(|s| clean(&s, 64)).unwrap_or_default();
            out.push(FarAccount {
                id: id.to_string(),
                tool: tool.to_string(),
                host_name: name.clone(),
                name,
                aid: a.str("aid").filter(|x| peers::hex32(x)).map(|x| x.to_string()).unwrap_or_default(),
            });
        }
        out
    });
    let mut rows: Vec<FarTemplate> = Vec::new();
    for t in doc.list("t").map(|l| l.items()).unwrap_or_default() {
        let Some(t) = t.as_dict() else { continue };
        let id = t.str("i").filter(|s| peers::lower_hex(s, 8));
        let digest = t.str("h").filter(|s| peers::lower_hex(s, 16));
        let tool = t.str("k").filter(|k| matches!(&**k, "claude" | "codex" | "agy"));
        let (Some(id), Some(digest), Some(tool)) = (id, digest, tool) else { continue };
        if rows.iter().any(|r| *r.id == *id) {
            continue;
        }
        let text = |k: &str, max: usize| t.str(k).map(|s| clean(&s, max)).unwrap_or_default();
        let account = text("a", 64);
        rows.push(FarTemplate {
            id: id.to_string(),
            digest: digest.to_string(),
            name: text("n", 40),
            tool: tool.to_string(),
            host_account: account.clone(),
            account,
            aid: t.str("aid").filter(|a| peers::hex32(a)).map(|a| a.to_string()).unwrap_or_default(),
            dir: text("d", 200),
            args: text("x", 300),
            line: t.get("l").and_then(|v| v.as_bool()) == Some(true),
        });
    }
    Some(Opened { templates: rows, more: doc.get("more").and_then(|v| v.as_int()).unwrap_or(0).clamp(0, 99), accounts })
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
    let mut launchers: Vec<FarLauncher> = Vec::new();
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
            let Some(blob) = r.get("blob") else { continue };
            // 服务器回的、对方写的都可能缺了哪一项：用 `get`（`at` 取不存在的键会崩）
            let online = r.get("online").and_then(|v| v.as_bool()).unwrap_or(false);
            let busy = online && r.get("busy").and_then(|v| v.as_bool()).unwrap_or(false);
            let Some(doc) = crypto::dir_open(&secret, &peer.pid, &sid, &blob) else {
                // 会话的说明的密钥解不开的：可能是那台机器的启动器（另一把密钥封的），也解不开的不算
                let opened = crypto::launch_open(&secret, &peer.pid, &sid, &blob).and_then(|d| launcher_of(&d));
                if let Some(o) = opened {
                    let l = FarLauncher {
                        sid: sid.to_string(),
                        pid: peer.pid.clone(),
                        peer: peer.name.clone(),
                        online,
                        busy,
                        more: o.more,
                        templates: o.templates,
                        accounts: o.accounts,
                    };
                    match launchers.iter_mut().find(|x| x.pid == l.pid) {
                        // 有回应的优先（一个配对只给一个）
                        Some(x) if l.online && !l.busy && (!x.online || x.busy) => *x = l,
                        Some(_) => {}
                        None => launchers.push(l),
                    }
                }
                continue;
            };
            let text = |k: &str, max: usize| doc.str(k).map(|s| clean(&s, max)).unwrap_or_default();
            let name = text("name", 64);
            sessions.push(Far {
                sid: sid.to_string(),
                pid: peer.pid.clone(),
                peer: peer.name.clone(),
                tool: text("tool", 16),
                host_name: name.clone(),
                name,
                aid: doc.str("aid").filter(|a| peers::hex32(a)).map(|a| a.to_string()).unwrap_or_default(),
                cwd: text("cwd", 200),
                since: sane_time(doc.get("since").and_then(|v| v.as_num()).unwrap_or(0.0)),
                viewers: r.get("viewers").and_then(|v| v.as_int()).unwrap_or(0).clamp(0, 99),
                online,
                busy,
                view_only: doc.str("access").as_deref() == Some("view"),
            });
        }
    }
    sessions.sort_by(|a, b| a.since.total_cmp(&b.since));
    Ok(Ok(Listing { sessions, known, launchers }))
}

/// 各台服务器上问来的合在一起
pub struct Seen {
    pub sessions: Vec<Far>,
    /// 允许这台机器远程启动的那几台的启动器
    pub launchers: Vec<FarLauncher>,
    /// 问到了的那几台服务器上还在的配对
    pub known: HashSet<String>,
    /// 它的服务器这次问到了的配对（不在 known 里的：对方取消了；不在这里面的：这次不知道）
    pub asked: HashSet<String>,
    /// 没问到的服务器：是哪台、原因、服务器回的（没问出去的是 None）
    pub errors: Vec<(HubKind, String, Option<Fault>)>,
}

/// 配对记在哪台服务器上就问哪台：每台只问它上面的那几个配对
pub fn fetch_all(peers: &[Peer], timeout: i64) -> R<Seen> {
    let mut seen =
        Seen { sessions: Vec::new(), launchers: Vec::new(), known: HashSet::new(), asked: HashSet::new(), errors: Vec::new() };
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
            Ok(mut l) => {
                rename_here(hub, &mut l.sessions, &mut l.launchers);
                seen.sessions.extend(l.sessions);
                seen.launchers.extend(l.launchers);
                seen.known.extend(l.known);
                seen.asked.extend(group.iter().map(|p| p.pid.clone()));
            }
            Err(f) => seen.errors.push((hub, f.text.clone(), Some(f))),
        }
    }
    seen.sessions.sort_by(|a, b| a.since.total_cmp(&b.since));
    Ok(seen)
}

/// 账号的名字以这台机器为准：这台机器上也有这个账号（hub 那个 Team 的盐算出来的摘要对得上）就用这里的别名。会话、启动器的模板、
/// 启动器能挑的账号都是
pub fn rename_here(hub: HubKind, sessions: &mut [Far], launchers: &mut [FarLauncher]) {
    let tagged = sessions.iter().any(|f| !f.aid.is_empty())
        || launchers.iter().flat_map(|l| &l.templates).any(|t| !t.aid.is_empty())
        || launchers.iter().flat_map(|l| l.accounts.iter().flatten()).any(|a| !a.aid.is_empty());
    if !tagged {
        return;
    }
    let Some(salt) = team_salt(hub) else { return };
    let names = local_names(&salt);
    rename(sessions, &names);
    for l in launchers.iter_mut() {
        for t in l.templates.iter_mut() {
            if let Some(n) = names.get(&(t.tool.clone(), t.aid.clone())) {
                t.account = n.clone();
            }
        }
        for a in l.accounts.iter_mut().flatten() {
            if let Some(n) = names.get(&(a.tool.clone(), a.aid.clone())) {
                a.name = n.clone();
            }
        }
    }
}

/// 认得出账号的会话换成这台机器上的名字
fn rename(sessions: &mut [Far], names: &HashMap<(String, String), String>) {
    for f in sessions {
        if let Some(n) = names.get(&(f.tool.clone(), f.aid.clone())) {
            f.name = n.clone();
        }
    }
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
            host_name: "工作".into(),
            aid: String::new(),
            cwd: "~/项目".into(),
            since: 0.0,
            viewers: 2,
            online: false,
            busy: false,
            view_only: true,
        };
        let r = row(&f);
        assert_eq!(
            r, "01234567  claude  工作  ~/项目  在「服务器」上，只能看，2 处接入，那台机器暂时连不上",
            "没有启动时间的不写时间"
        );
        let r = row(&Far { online: true, busy: true, viewers: 0, view_only: false, ..f.clone() });
        assert!(r.ends_with("在「服务器」上，那台机器没有回应"), "连着服务器、宿主没有回应：{r}");
        let r = row(&Far { since: 1794000000.0, ..f });
        assert!(r.starts_with("01234567  claude  工作  ~/项目  ") && r.contains(" 开始  在「服务器」上"), "{r}");
    }

    #[test]
    fn launcher_docs_from_the_other_side() {
        let doc = |s: &str| crate::py::json::loads(s).expect("JSON").as_dict().expect("对象");
        let long = "字".repeat(50);
        let good = format!(
            r#"{{"v":1,"machine":"box","more":3,"t":[
            {{"i":"0a1b2c3d","h":"0011223344556677","n":"修 bug\u001b[2J","k":"claude","a":"alice","aid":"0123456789abcdef0123456789abcdef","d":"~/work","x":"--model opus","l":true}},
            {{"i":"0a1b2c3d","h":"8899aabbccddeeff","n":"同一个编号","k":"claude","a":"x","d":"~","x":"","l":true}},
            {{"i":"0A1B2C3D","h":"0011223344556677","n":"大写的编号","k":"claude"}},
            {{"i":"1a1b1c1d","h":"00112233","n":"摘要太短","k":"claude"}},
            {{"i":"2a2b2c2d","h":"0011223344556677","n":"不认识的工具","k":"vim"}},
            {{"i":"3a3b3c3d","h":"0011223344556677","n":"{long}","k":"agy","aid":"not-hex","l":"yes"}},
            "不是对象"]}}"#
        );
        let opened = launcher_of(&doc(&good)).expect("认得");
        let (rows, more) = (opened.templates, opened.more);
        assert_eq!(more, 3);
        assert_eq!(opened.accounts, None, "旧版的 cca 写的：没有能挑的账号（模板用它自己的）");
        assert_eq!(
            rows.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["0a1b2c3d", "3a3b3c3d"],
            "编号重复的、样子不对的、不认识的工具不要"
        );
        let (a, b) = (&rows[0], &rows[1]);
        assert_eq!(
            (a.digest.as_str(), a.name.as_str(), a.tool.as_str()),
            ("0011223344556677", "修 bug[2J", "claude"),
            "那台机器写的字去掉控制字符"
        );
        assert_eq!((a.account.as_str(), a.host_account.as_str(), a.aid.len()), ("alice", "alice", 32));
        assert_eq!((a.dir.as_str(), a.args.as_str(), a.line), ("~/work", "--model opus", true));
        assert_eq!(b.name.chars().count(), 40, "名字最多 40 个字");
        assert_eq!(
            (b.account.as_str(), b.aid.as_str(), b.dir.as_str(), b.args.as_str(), b.line),
            ("", "", "", "", false),
            "没写的空着，l 不是 true 就不能带"
        );
        // 版本不对的不认；more 夹在 0..=99；没有模板的也认（列表里照样有这个启动器：那台机器上还没有模板）
        assert!(launcher_of(&doc(r#"{"v":2,"t":[]}"#)).is_none());
        assert!(launcher_of(&doc(r#"{"t":[]}"#)).is_none());
        assert_eq!(launcher_of(&doc(r#"{"v":1,"more":500}"#)).map(|o| (o.templates.len(), o.more)), Some((0, 99)));
        assert_eq!(launcher_of(&doc(r#"{"v":1,"more":-5,"t":"不是列表"}"#)).map(|o| (o.templates.len(), o.more)), Some((0, 0)));
        // 3.7.10 起：起会话时能挑的账号
        let picks = format!(
            r#"{{"v":1,"t":[],"accounts":[
            {{"i":"00112233aabbccdd","k":"claude","n":"工作\u202e","aid":"0123456789abcdef0123456789abcdef"}},
            {{"i":"00112233aabbccdd","k":"codex","n":"编号重复"}},
            {{"i":"00112233AABBCCDD","k":"claude","n":"大写的编号"}},
            {{"i":"0011","k":"claude","n":"编号太短"}},
            {{"i":"99112233aabbccdd","k":"vim","n":"不认识的工具"}},
            {{"i":"44112233aabbccdd","k":"agy","n":"{long}","aid":"not-hex"}},
            7]}}"#
        );
        let accts = launcher_of(&doc(&picks)).and_then(|o| o.accounts).expect("有能挑的账号");
        assert_eq!(accts.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["00112233aabbccdd", "44112233aabbccdd"]);
        assert_eq!(
            (accts[0].tool.as_str(), accts[0].name.as_str(), accts[0].host_name.as_str(), accts[0].aid.len()),
            ("claude", "工作", "工作", 32),
            "那台机器写的字去掉看不见的字符"
        );
        assert_eq!((accts[1].name.chars().count(), accts[1].aid.as_str()), (50, ""));
        assert_eq!(launcher_of(&doc(r#"{"v":1,"accounts":"不是列表"}"#)).and_then(|o| o.accounts), Some(Vec::new()));
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
            headless: false,
        };
        // 那一头结束不结束得了它：只有无头的会话（远程启动的）、能操作的配对
        assert!(dir_doc(&meta, Incoming::Control, None).get("end").is_none(), "终端里起的会话只在这台机器上结束得了");
        let launched = Meta { headless: true, ..meta.clone() };
        assert_eq!(dir_doc(&launched, Incoming::Control, None).at("end").as_bool(), Some(true));
        assert!(dir_doc(&launched, Incoming::View, None).get("end").is_none(), "只让看的结束不了");
        let d = dir_doc(&meta, Incoming::View, None);
        assert_eq!(d.str("cwd").as_deref(), Some("~/proj"));
        assert!(d.get("aid").is_none(), "没有 Team 的盐：不给摘要");
        assert_eq!(d.str("access").as_deref(), Some("view"));
        assert!(!crate::py::json::dumps(&crate::py::obj::V::Dict(d), crate::py::json::LINE).contains("example.com"));
        // 没起别名的账号，名字就是邮箱：只给 @ 前面那一截
        let d = dir_doc(
            &Meta { name: "alice@example.com".into(), key: "alice@example.com".into(), ..meta.clone() },
            Incoming::Control,
            None,
        );
        assert_eq!(d.str("name").as_deref(), Some("alice"));
        assert!(!crate::py::json::dumps(&crate::py::obj::V::Dict(d), crate::py::json::LINE).contains("example.com"));
        // 有 Team 的盐：给账号摘要（和设备统计的一样），还是不给邮箱
        let salt = "ab".repeat(16);
        let d = dir_doc(&meta, Incoming::Control, Some(&salt));
        let aid = presence_aid(&salt, "codex", "alice@example.com").unwrap();
        assert_eq!(d.str("aid").as_deref(), Some(aid.as_str()));
        assert!(!crate::py::json::dumps(&crate::py::obj::V::Dict(d), crate::py::json::LINE).contains("example.com"));
        assert_eq!((short_name("工作"), short_name("@x"), short_name("")), ("工作", "@x", ""));
    }

    #[test]
    fn the_name_on_this_machine_wins() {
        let salt = "cd".repeat(16);
        let aid = aid_of(&salt, "claude", "主力(alice@example.com)").unwrap();
        assert_eq!(aid_of(&salt, "claude", "ALICE@example.com"), Some(aid.clone()), "邮箱不分大小写");
        assert_eq!(aid_of(&salt, "claude", "只有别名"), None);
        let far = |aid: &str, tool: &str| Far {
            sid: "0123456789abcdef0123456789abcdef".into(),
            pid: "ab".repeat(16),
            peer: "Mac mini".into(),
            tool: tool.into(),
            name: "那边的名字".into(),
            host_name: "那边的名字".into(),
            aid: aid.into(),
            cwd: "~".into(),
            since: 0.0,
            viewers: 0,
            online: true,
            busy: false,
            view_only: false,
        };
        let names = HashMap::from([(("claude".to_string(), aid.clone()), "这边的名字".to_string())]);
        let mut s = vec![far(&aid, "claude"), far(&aid, "codex"), far("", "claude"), far(&"0".repeat(32), "claude")];
        rename(&mut s, &names);
        let got: Vec<(&str, &str)> = s.iter().map(|f| (f.name.as_str(), f.host_name.as_str())).collect();
        assert_eq!(
            got,
            [
                ("这边的名字", "那边的名字"),
                ("那边的名字", "那边的名字"),
                ("那边的名字", "那边的名字"),
                ("那边的名字", "那边的名字")
            ],
            "只有同一个工具、摘要对得上的才换"
        );
    }
}
