//! 手机上的「账号」页和额度的通知（docs/dev/remote.md「手机上的账号」）。这台机器上各个账号的状态（登录、套餐、用量、每周用量、
//! 在不在用）按配对封好（`crypto::status_seal`，只有那个手机解得开），放到这个配对所在的那台服务器上（`status_put`）；手机查会话
//! 列表时顺带取走（`relay_list` 的 `have` / `status`）。额度用完了马上推一条通知（`push_note`）；恢复的那一条交给服务器（`status_put`
//! 的 `later`），到了重置的时候由它推：这台机器睡着了、手机锁着屏也收得到。
//!
//! - 只给手机（`peer.web`：iOS 上的 cca App、网页端），不给别的电脑；不让接入这台机器的（`Incoming::None`）不给，改成不让以后放一份
//!   空的上去盖掉原来的
//! - 和会话一样分 Team：一个配对只看得到走它那台服务器的账号（`relay::session_hub`）
//! - 不给邮箱：名字是别名（没起别名的是邮箱 @ 前面那一截，`far::short_name`）；认账号用的是按配对算的编号（`crypto::usage_tag`）和
//!   会话说明里的同一个账号摘要（`far::aid_of`：手机照它把会话放到账号下面）
//! - 额度用完的规则和 App 的一样（`exhausted`）
//!
//! `remote-status`（bash 的 `_remote-status`，在后台跑，CCA_RO=1）：查完用量（usage-render）、配好一个手机、改了它能不能接入以后由
//! `status_kick` 起；同时只有一个在跑，跑着的时候又有人叫的话它跑完再来一轮。这台机器记着放过什么（`~/.cca/remote/status.json`：
//! 每个配对放的那一份的摘要和时间、推过的“用完了”），没变的半小时才再放一次。

use super::crypto;
use super::far;
use super::local;
use super::peers::{self, Incoming, Peer};
use super::relay::{self, Fault, Server};
use crate::cmd::arity;
use crate::cmd::gui::gui_snapshot;
use crate::cmd::hub::HubKind;
use crate::cmd::presence::{flock, spawn_background};
use crate::dict;
use crate::py::fs;
use crate::py::hash::sha256_hex;
use crate::py::json;
use crate::py::obj::{Dict, List, V};
use crate::py::path;
use crate::py::time::{strftime_local, time};
use crate::py::R;
use crate::server::{LATER_MAX, LATER_SPAN, STATUS_BLOB};

/// 没变的话多久再放一次：服务器只在内存里存着（重启就没了），手机也照放的时间说这是多久以前的
const EVERY: f64 = 1800.0;
/// 服务器不认 status_put（3.7.5 以前的、没开中转的）：多久以后再试
const OLD_AGAIN: f64 = 6.0 * 3600.0;
/// 两次查到的重置时间差不到这么多算同一轮（Codex 的重置时间是照“还有几秒”算的，每次差一两秒；下一轮至少晚 5 小时）
const SAME_ROUND: f64 = 3600.0;
/// 推过的“用完了”过了重置时间多久才忘掉：这期间同一轮的不再推（查到的用量时新时旧，用完没用完会来回变）
const FORGET: f64 = 86400.0;
/// 每个请求最多等几秒
const TIMEOUT: i64 = 10;
/// 判断额度用完的两个窗口，5 小时在前
const SLOTS: [&str; 2] = ["five_hour", "seven_day"];

/// 快照（`_gui state` 的那一份）里的一个账号
struct Acct {
    tool: String,
    /// 工具的叫法（Claude Code、Codex、Antigravity）
    label: String,
    /// 认账号（`acct_id`：按邮箱，换别名不变）：只用来算按配对的编号，不发出去
    ident: String,
    /// 账号表的键：算会话说明里的账号摘要，不发出去
    key: String,
    name: String,
    /// 这个账号的会话走哪台服务器（一个配对只看得到走它那台的）
    hub: Option<HubKind>,
    /// 给手机的那一项（按配对的 id、aid 另加，见 doc_for）
    row: Dict,
    /// 挡住它的额度窗口（exhausted）
    used: Vec<(&'static str, Option<f64>)>,
}

impl Acct {
    /// 按配对算的编号：同一个账号在这个配对里一直是它（换别名不变），别的配对、服务器看不出是哪个账号
    fn tag(&self, p: &[u8]) -> String {
        tag(p, &["acct", &self.tool, &self.ident])
    }
}

fn tag(p: &[u8], parts: &[&str]) -> String {
    crypto::usage_tag(p, &parts.join("\0"))
}

fn number(d: &Dict, k: &str) -> Option<f64> {
    d.get(k).filter(|v| !v.is_bool()).and_then(|v| v.as_num()).filter(|x| x.is_finite())
}

fn num(d: &Dict, k: &str) -> V {
    number(d, k).map_or(V::None, V::Float)
}

fn flag(d: &Dict, k: &str) -> bool {
    d.get(k).and_then(|v| v.as_bool()) == Some(true)
}

/// 过了重置时间（`reset`，或者重置时间不晚于现在）
fn is_reset(w: &Dict, now: f64) -> bool {
    flag(w, "reset") || number(w, "resets_epoch").is_some_and(|at| at <= now)
}

/// Antigravity 的几组额度里这个窗口用得最少的那个（停用的、没有百分比的不算；过了重置时间的算 0；一样多时先重置的那个，不知道什么时候
/// 重置的算最晚）：App 的 `UsageEntry.quotaLeast`
fn least(groups: &List, slot: &str, now: f64) -> Option<Dict> {
    let mut best: Option<(f64, f64, Dict)> = None;
    for g in groups.items() {
        let Some(buckets) = g.as_dict().and_then(|g| g.list("buckets")) else { continue };
        for b in buckets.items() {
            let Some(b) = b.as_dict() else { continue };
            if b.str("slot").as_deref() != Some(slot) || flag(&b, "disabled") {
                continue;
            }
            let Some(p) = number(&b, "utilization") else { continue };
            let used = if is_reset(&b, now) { 0.0 } else { p };
            let at = number(&b, "resets_epoch").unwrap_or(f64::INFINITY);
            if best.as_ref().is_none_or(|(u, t, _)| used < *u || (used == *u && at < *t)) {
                best = Some((used, at, b));
            }
        }
    }
    best.map(|(_, _, b)| b)
}

/// 挡住这个账号的额度窗口：`(窗口, 重置时间)`，5 小时在前。和 App 的 `Diagnosis.quotaPressure` 里用完了的那些是同一个规则
/// （macos/Sources/CcaKit/Model/Diagnosis.swift；iOS 的 App 也照它）：登录着、有用量（`source` 不是 none）、不是登录失效（`error_code`
/// 是 auth）；5 小时、本周两个窗口（Antigravity 是各组里这个窗口用得最少的那个：一组用完了别的组的模型照样能用），四舍五入以后到了
/// 100%、还没到重置时间（数据旧不旧不看：没到重置时间不会降下来）。模型级的窗口不算。a 是快照里的一个账号
pub fn exhausted(a: &Dict, now: f64) -> Vec<(&'static str, Option<f64>)> {
    if a.str("login").as_deref() != Some("logged_in") {
        return Vec::new();
    }
    let Some(u) = a.dict("usage") else { return Vec::new() };
    if u.str("source").as_deref().is_none_or(|s| s == "none") || u.str("error_code").as_deref() == Some("auth") {
        return Vec::new();
    }
    let groups = u.list("quota_groups").filter(|g| !g.is_empty());
    let mut out = Vec::new();
    for slot in SLOTS {
        let w = match &groups {
            Some(g) => least(g, slot, now),
            None => u.dict(slot),
        };
        let Some(w) = w else { continue };
        let Some(p) = number(&w, "utilization") else { continue };
        if !is_reset(&w, now) && p.round() >= 100.0 {
            out.push((slot, number(&w, "resets_epoch")));
        }
    }
    out
}

/// 一个用量窗口：用了百分之几、什么时候重置、过没过重置时间；没有是 null
fn window(w: Option<Dict>) -> V {
    let Some(w) = w else { return V::None };
    V::Dict(
        dict! { "utilization" => num(&w, "utilization"), "resets_epoch" => num(&w, "resets_epoch"), "reset" => flag(&w, "reset") },
    )
}

/// 用量条目（PROTOCOL 3.1）里手机要的：各个窗口用了多少、什么时候重置，什么时候查到的、出没出错（只给原因码，不给说明：说明里有
/// 端口号这些）。不给邮箱
fn usage_of(u: &Dict) -> Dict {
    let code = u.get("error_code").filter(|v| v.is_str()).unwrap_or(V::None);
    let d = dict! {
        "source" => u.str("source").map_or(V::None, V::from), "fetched_epoch" => num(u, "fetched_epoch"), "stale" => flag(u, "stale"),
        "error_code" => code, "five_hour" => window(u.dict("five_hour")), "seven_day" => window(u.dict("seven_day")),
    };
    let scoped = List::new();
    for m in u.list("model_scoped").map(|l| l.items()).unwrap_or_default() {
        let Some(m) = m.as_dict() else { continue };
        let row = window(Some(m.clone())).as_dict().expect("刚建的");
        row.set("name", far::clean(&m.str("name").map(|s| s.to_string()).unwrap_or_default(), 32));
        for k in ["allowance_percent_of_weekly", "used_percent_of_weekly"] {
            if let Some(x) = number(&m, k) {
                row.set(k, x);
            }
        }
        scoped.push(row);
    }
    d.set("model_scoped", scoped);
    let first = u.dict("use_first").map_or(V::None, |f| {
        V::Dict(dict! { "left_percent" => num(&f, "left_percent"), "from_epoch" => num(&f, "from_epoch"), "until_epoch" => num(&f, "until_epoch") })
    });
    d.set("use_first", first);
    // Codex 还能重置几次额度
    if let Some(r) = u.dict("reset_credits") {
        d.set(
            "reset_credits",
            dict! { "availableCount" => num(&r, "availableCount"), "next_expires_epoch" => num(&r, "next_expires_epoch") },
        );
    }
    // Antigravity 的几组额度
    if let Some(groups) = u.list("quota_groups") {
        let out = List::new();
        for g in groups.items() {
            let Some(g) = g.as_dict() else { continue };
            let buckets = List::new();
            for b in g.list("buckets").map(|l| l.items()).unwrap_or_default() {
                let Some(b) = b.as_dict() else { continue };
                let row = window(Some(b.clone())).as_dict().expect("刚建的");
                let slot = b.str("slot").filter(|s| SLOTS.contains(&&**s));
                row.set("slot", slot.map_or(V::None, V::from));
                row.set("disabled", flag(&b, "disabled"));
                row.set("remaining_amount", b.get("remaining_amount").filter(|v| v.is_int()).unwrap_or(V::None));
                buckets.push(row);
            }
            out.push(
                dict! { "name" => far::clean(&g.str("name").map(|s| s.to_string()).unwrap_or_default(), 64), "buckets" => buckets },
            );
        }
        d.set("quota_groups", out);
    }
    d
}

/// 每周用量（PROTOCOL 2.2 的 `usage_weeks`）里手机的图要的
fn weeks_of(w: &Dict) -> Dict {
    let weeks = List::new();
    for x in w.list("weeks").map(|l| l.items()).unwrap_or_default() {
        let Some(x) = x.as_dict() else { continue };
        weeks.push(dict! {
            "start_epoch" => num(&x, "start_epoch"), "end_epoch" => num(&x, "end_epoch"), "used_percent" => num(&x, "used_percent"),
            "resets" => num(&x, "resets"), "current" => flag(&x, "current"), "partial" => flag(&x, "partial"),
        });
    }
    let summary = w.dict("summary").map_or(V::None, |s| {
        V::Dict(dict! {
            "weeks" => num(&s, "weeks"), "average_percent" => num(&s, "average_percent"), "maxed" => num(&s, "maxed"),
            "under_half" => num(&s, "under_half"),
        })
    });
    dict! { "since_epoch" => num(w, "since_epoch"), "weeks" => weeks, "summary" => summary }
}

/// 头像的颜色（启动器的颜色，`#RRGGBB`）
fn color_ok(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// 给手机的一个账号（不带邮箱、键；按配对的 id、aid 另加）。a 是快照里的一个账号
fn row_of(a: &Dict, tool: &str, name: &str) -> Dict {
    let running = |k: &str| a.dict(k).is_some_and(|x| flag(&x, "running"));
    let s = |k: &str| a.str(k).map(|s| s.to_string()).unwrap_or_default();
    let d = dict! {
        "tool" => tool, "name" => name, "kind" => far::clean(&s("kind"), 16), "login" => far::clean(&s("login"), 32),
        "plan" => far::clean(&s("plan"), 32), "tier" => far::clean(&s("plan_tier"), 32), "busy" => running("app") || running("cli"),
        "usage" => a.dict("usage").map_or(V::None, |u| V::Dict(usage_of(&u))),
        "weeks" => a.dict("usage_weeks").map_or(V::None, |w| V::Dict(weeks_of(&w))),
    };
    // 头像（启动器的那一个）：颜色和上面的字
    if let Some(b) = a.dict("badge") {
        if let Some(c) = b.str("color").filter(|c| color_ok(c)) {
            d.set("color", &*c);
        }
        d.set("badge", far::clean(&b.str("label").map(|s| s.to_string()).unwrap_or_default(), 8));
    }
    d
}

/// 快照里的账号（三个工具的，照账号表的先后）
fn accounts(snap: &Dict, now: f64) -> Vec<Acct> {
    let mut out = Vec::new();
    for tool in snap.list("tools").map(|l| l.items()).unwrap_or_default() {
        let Some(tool) = tool.as_dict() else { continue };
        let Some(id) = tool.str("id") else { continue };
        let label = tool.str("label").map(|s| s.to_string()).unwrap_or_default();
        for a in tool.list("accounts").map(|l| l.items()).unwrap_or_default() {
            let Some(a) = a.as_dict() else { continue };
            let (Some(key), Some(ident)) = (a.str("key"), a.str("id")) else { continue };
            let name = far::clean(far::short_name(&a.str("name").map(|s| s.to_string()).unwrap_or_default()), 64);
            out.push(Acct {
                tool: id.to_string(),
                label: label.clone(),
                ident: ident.to_string(),
                key: key.to_string(),
                hub: relay::session_hub(&id, &key),
                row: row_of(&a, &id, &name),
                used: exhausted(&a, now),
                name,
            });
        }
    }
    out
}

/// 给一个配对的那一份（PROTOCOL 见 docs/dev/remote.md「手机上的账号」）：走这台服务器的账号（`accts`）。`id`：按配对算的账号编号；
/// `aid`：会话说明里的同一个账号摘要（有这个 Team 的盐、账号有邮箱时才有）。weeks：带不带每周用量；只放前 n 个账号（太长时）
fn doc_for(p: &[u8], accts: &[&Acct], salt: Option<&str>, machine: &str, weeks: bool, n: usize) -> Dict {
    let list = List::new();
    for a in accts.iter().take(n) {
        let row = a.row.copy();
        row.set("id", a.tag(p));
        if let Some(aid) = salt.and_then(|s| far::aid_of(s, &a.tool, &a.key)) {
            row.set("aid", aid);
        }
        if !weeks {
            row.set("weeks", V::None);
        }
        list.push(row);
    }
    let d = dict! { "v" => 1, "machine" => machine, "accounts" => list };
    if !weeks {
        // 太长，没带每周用量：手机照它说一声，不说成还没有记录
        d.set("weeks", false);
    }
    if n < accts.len() {
        d.set("more", accts.len() - n);
    }
    d
}

/// 封好的那一份和它的摘要（不算放的时间：没变就不用再放）。太长（服务器不收）时先不带每周用量，再一个个少放账号；一个都放不下是 None
fn seal_doc(p: &[u8], pid: &str, accts: &[&Acct], salt: Option<&str>, machine: &str, now: f64) -> R<Option<(Dict, String)>> {
    let mut tries = vec![(true, accts.len())];
    tries.extend((0..=accts.len()).rev().map(|n| (false, n)));
    for (weeks, n) in tries {
        let doc = doc_for(p, accts, salt, machine, weeks, n);
        let sum = sha256_hex(json::dumps(&V::Dict(doc.copy()), json::Fmt::new().compact()));
        doc.set("at", now.floor() as i64);
        let blob = crypto::status_seal(p, pid, &doc)?;
        if blob.str("c").is_some_and(|c| c.len() <= STATUS_BLOB) {
            return Ok(Some((blob, sum)));
        }
    }
    Ok(None)
}

fn window_text(slot: &str) -> &'static str {
    if slot == "five_hour" {
        "5 小时"
    } else {
        "本周"
    }
}

/// 几点（今天的只写时间）
fn when(t: f64, now: f64) -> String {
    if strftime_local("%Y%m%d", t) == strftime_local("%Y%m%d", now) {
        strftime_local("%H:%M", t)
    } else {
        strftime_local("%m-%d %H:%M", t)
    }
}

/// 一条额度的通知（封给手机的，docs/dev/push.md「额度的通知」）：`kind` 是 usage，`event` 是 used_up（用完了，马上推）或者 back
/// （恢复了，服务器到点推）。手机照 window、resets_epoch 自己写字（它那里的时区，和 App 里一样的说法）；title、subtitle、body 是给认不出
/// 这几项的看的
fn usage_note(
    id: &str,
    a: &Acct,
    atag: &str,
    aid: Option<String>,
    machine: &str,
    slot: &str,
    resets: Option<f64>,
    back: bool,
    now: f64,
) -> Dict {
    let (title, body) = if back {
        ("额度恢复了", format!("{}的额度恢复了，可以接着用了", window_text(slot)))
    } else {
        let at = resets.map(|r| format!("，{} 恢复", when(r, now))).unwrap_or_default();
        ("额度用完了", format!("{}的额度用完了{at}", window_text(slot)))
    };
    let d = dict! {
        "v" => 1, "id" => id, "kind" => "usage", "event" => if back { "back" } else { "used_up" },
        "at" => if back { resets.unwrap_or(now) } else { now }.floor() as i64,
        "account" => atag, "tool" => a.tool.as_str(), "name" => a.name.as_str(), "machine" => machine, "window" => slot,
        "resets_epoch" => resets.map_or(V::None, V::Float),
        "title" => title, "subtitle" => format!("{} · {}", a.name, a.label), "body" => body,
    };
    if let Some(aid) = aid {
        d.set("aid", aid);
    }
    d
}

/// 新用完的一个额度窗口（这一轮要推的）
struct Fresh {
    acct: usize,
    slot: &'static str,
    resets: Option<f64>,
    /// 记在 noted 里的键（账号编号/窗口）和这一轮的时间（重置时间，不知道是 0）
    key: String,
    anchor: f64,
    id: String,
}

/// 到点再推的一条（额度恢复的那一刻）
struct Back {
    at: f64,
    acct: usize,
    slot: &'static str,
    /// 服务器上这一项的位置（同一个账号的同一个窗口一直是它）、这条通知的编号
    place: String,
    id: String,
}

/// 这一轮给一个配对要做的：推过的里面还要记着的（`keep`）、新用完的（`fresh`，推成了再记上）、到点再推的（`back`，先到点的在前，
/// 最多 LATER_MAX 条）。noted 是上次记下的 `{账号编号/窗口: 这一轮的时间}`
fn plan(p: &[u8], accts: &[&Acct], noted: &Dict, now: f64) -> (Dict, Vec<Fresh>, Vec<Back>) {
    let keep = Dict::new();
    // 推过的：过了重置时间一天才忘掉（这期间同一轮的不再推）；不知道什么时候重置的只在还用完着时记着（下面）
    for (k, v) in noted.items() {
        if v.as_num().is_some_and(|at| at > 0.0 && now - at < FORGET) {
            keep.set(k, v);
        }
    }
    let (mut fresh, mut back) = (Vec::new(), Vec::new());
    for (i, a) in accts.iter().enumerate() {
        let atag = a.tag(p);
        for (slot, resets) in &a.used {
            let key = format!("{atag}/{slot}");
            let r = resets.unwrap_or(0.0);
            let prev = noted.get(&key).and_then(|v| v.as_num());
            let anchor = match prev {
                Some(x) if (x - r).abs() < SAME_ROUND => x,
                _ => r,
            };
            let round = (anchor as i64).to_string();
            if prev == Some(anchor) {
                keep.set(key.as_str(), anchor);
            } else {
                fresh.push(Fresh { acct: i, slot, resets: *resets, id: tag(p, &["used", &atag, slot, &round]), key, anchor });
            }
            // 服务器只收 LATER_SPAN 以内的（本周的最多 7 天以后）
            if let Some(at) = resets.filter(|at| *at > now && *at < now + LATER_SPAN as f64 - 60.0) {
                back.push(Back {
                    at,
                    acct: i,
                    slot,
                    place: tag(p, &["slot", &atag, slot]),
                    id: tag(p, &["back", &atag, slot, &round]),
                });
            }
        }
    }
    back.sort_by(|a, b| a.at.total_cmp(&b.at));
    back.truncate(LATER_MAX);
    (keep, fresh, back)
}

/// 服务器不认这一种请求（3.7.5 以前的，或者没开中转）
fn old_server(f: &Fault) -> bool {
    f.code == 404 && f.error == "not_found"
}

/// 这一轮给一个配对的：放状态（变了，或者上次放的超过 EVERY 了）、推新用完的额度。st 是这台机器记着的这个配对的那一项（改在它上面）
#[allow(clippy::too_many_arguments)]
fn publish_peer(
    srv: &Server,
    peer: &Peer,
    p: &[u8],
    accts: &[&Acct],
    salt: Option<&str>,
    machine: &str,
    now: f64,
    st: &Dict,
) -> R<()> {
    if number(st, "old").is_some_and(|t| (0.0..OLD_AGAIN).contains(&(now - t))) {
        return Ok(()); // 这台服务器还不认：过一阵再试
    }
    let pid = peer.pid.as_str();
    let aid = |a: &Acct| salt.and_then(|s| far::aid_of(s, &a.tool, &a.key));
    let (keep, fresh, back) = plan(p, accts, &st.dict("noted").unwrap_or_default(), now);
    let Some((blob, sum)) = seal_doc(p, pid, accts, salt, machine, now)? else { return Ok(()) };
    let later = List::new();
    let mut ids = Vec::new();
    for b in &back {
        let a = accts[b.acct];
        let note = crypto::note_seal(p, pid, &usage_note(&b.id, a, &a.tag(p), aid(a), machine, b.slot, Some(b.at), true, now))?;
        later.push(dict! { "slot" => b.place.as_str(), "at" => b.at, "id" => b.id.as_str(), "note" => note });
        ids.push(format!("{}:{}", b.id, b.at as i64));
    }
    let sum = sha256_hex(format!("{sum}|{}", ids.join(",")));
    let put = number(st, "put");
    if st.str("sum").as_deref() != Some(sum.as_str()) || put.is_none_or(|t| !(0.0..EVERY).contains(&(now - t))) {
        match srv.post("status_put", &dict! { "pid" => pid, "blob" => blob, "later" => later }, TIMEOUT)? {
            Ok(_) => {
                st.set("sum", sum);
                st.set("put", now);
                st.pop("old");
            }
            Err(f) if old_server(&f) => {
                st.set("old", now);
                return Ok(());
            }
            Err(_) => return Ok(()), // 下一轮再放（推过的照旧记着）
        }
    }
    for f in fresh {
        let a = accts[f.acct];
        let note = crypto::note_seal(p, pid, &usage_note(&f.id, a, &a.tag(p), aid(a), machine, f.slot, f.resets, false, now))?;
        match srv.post("push_note", &dict! { "pid" => pid, "id" => f.id.as_str(), "note" => note }, TIMEOUT)? {
            // 手机没要额度的通知、服务器没开推送（sent 是 false）也记上：之后打开了，也不补推以前用完的
            Ok(_) => keep.set(f.key.as_str(), f.anchor),
            Err(e) if old_server(&e) => {
                st.set("old", now);
                break;
            }
            Err(_) => {} // 下一轮再推
        }
    }
    st.set("noted", keep);
    Ok(())
}

/// 不让接入这台机器了：放一份空的（`off`）盖掉原来的、到点再推的也都拿掉。true：盖掉了（或者服务器上本来就没有）
fn retract(srv: &Server, peer: &Peer, p: &[u8], machine: &str, now: f64) -> R<bool> {
    let doc = dict! { "v" => 1, "machine" => machine, "accounts" => List::new(), "off" => true, "at" => now.floor() as i64 };
    let blob = crypto::status_seal(p, &peer.pid, &doc)?;
    Ok(match srv.post("status_put", &dict! { "pid" => peer.pid.as_str(), "blob" => blob, "later" => List::new() }, TIMEOUT)? {
        Ok(_) => true,
        Err(f) => f.code == 404, // 服务器不认、上面没有这个配对：也就没有要盖掉的
    })
}

fn state_file() -> String {
    path::join(&local::dir(), "status.json")
}

/// 这台机器记着的：`{"v": 1, "pids": {配对编号: {"sum", "put", "old", "noted"}}}`
fn load_state() -> Dict {
    let d = fs::load_quiet(&state_file()).and_then(|v| v.as_dict()).filter(|d| d.get("v").and_then(|v| v.as_int()) == Some(1));
    let d = d.unwrap_or_else(|| dict! { "v" => 1 });
    if d.dict("pids").is_none() {
        d.set("pids", Dict::new());
    }
    d
}

/// 放一轮：每个手机的配对各一份
fn publish(now: f64) -> R<()> {
    let state = load_state();
    let pids = state.dict("pids").expect("load_state 保证了");
    let peers = peers::load();
    // 配对表里没有了的：忘掉（服务器上的，取消配对时 pair_drop 一起删了）
    for k in pids.keys() {
        if !peers.iter().any(|p| p.web && *p.pid == *k) {
            pids.pop(&k);
        }
    }
    let phones: Vec<&Peer> = peers.iter().filter(|p| p.web && (p.incoming != Incoming::None || pids.has(&p.pid))).collect();
    if !phones.is_empty() {
        let machine = peers::machine_name();
        let accts =
            if phones.iter().any(|p| p.incoming != Incoming::None) { accounts(&gui_snapshot(false)?, now) } else { Vec::new() };
        let mut hubs: Vec<HubKind> = Vec::new();
        for p in &phones {
            if !hubs.contains(&p.hub) {
                hubs.push(p.hub);
            }
        }
        for hub in hubs {
            let Ok(srv) = relay::server_of(hub)? else { continue };
            let salt = far::team_salt(hub);
            let mine: Vec<&Acct> = accts.iter().filter(|a| a.hub == Some(hub)).collect();
            for peer in phones.iter().filter(|p| p.hub == hub) {
                let Some(secret) = peers::secret(&peer.pid) else { continue };
                if peer.incoming == Incoming::None {
                    if retract(&srv, peer, &secret, &machine, now)? {
                        pids.pop(&peer.pid);
                    }
                    continue;
                }
                let st = pids.dict(&peer.pid).unwrap_or_default();
                publish_peer(&srv, peer, &secret, &mine, salt.as_deref(), &machine, now, &st)?;
                pids.set(peer.pid.as_str(), st);
            }
        }
    }
    if pids.is_empty() && !path::exists(&state_file()) {
        return Ok(());
    }
    local::ensure_dir()?;
    fs::save_json(&state_file(), &V::Dict(state), Some(0o600), true)
}

/// `remote-status`（bash 的 `_remote-status`，status_kick 在后台起的）：放一轮账号的状态、推新用完的额度。同时只有一个在跑：跑着的
/// 时候又有人叫（`status.again`），它跑完再来一轮。出什么错都安静地退出（下次再来）
pub fn cmd_remote_status(args: &[String]) -> R<()> {
    arity("cmd_remote_status", args, 0, 0)?;
    let Ok(d) = local::ensure_dir() else { return Ok(()) };
    let again = path::join(&d, "status.again");
    let touch = || std::fs::OpenOptions::new().append(true).create(true).open(&again).map(drop);
    loop {
        if touch().is_err() {
            return Ok(());
        }
        let Ok(lock) = std::fs::OpenOptions::new().append(true).create(true).open(path::join(&d, "status.lock")) else {
            return Ok(());
        };
        if !flock(&lock, libc::LOCK_EX | libc::LOCK_NB) {
            return Ok(()); // 在跑的那个看到 status.again 会再来一轮
        }
        while path::exists(&again) {
            let _ = std::fs::remove_file(&again);
            if let Err(e) = publish(time()) {
                e.swallowed();
            }
        }
        drop(lock);
        // 放开锁的这一刻又有人叫、它没拿到锁就走了：接着来
        if !path::exists(&again) {
            return Ok(());
        }
    }
}

/// 有手机配对时在后台放一轮账号的状态（查完用量、配好一个手机、改了它能不能接入以后）。静默
pub fn status_kick() {
    let peers = peers::load();
    if !peers.iter().any(|p| p.web) {
        return;
    }
    let stated = || load_state().dict("pids").is_some_and(|d| !d.is_empty());
    if peers.iter().any(|p| p.web && p.incoming != Incoming::None) || stated() {
        spawn_background(&["_remote-status"]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: [u8; 32] = [7; 32];
    const NOW: f64 = 1_794_000_000.0;

    fn w(util: f64, resets: Option<f64>) -> V {
        V::Dict(
            dict! { "utilization" => util, "resets_epoch" => resets.map_or(V::None, V::Float), "reset" => false, "resets_at" => "x" },
        )
    }

    fn acct(five: V, week: V) -> Dict {
        dict! {
            "key" => "工作(alice@example.com)", "id" => "alice@example.com", "name" => "工作", "kind" => "oauth", "login" => "logged_in",
            "plan" => "Max", "plan_tier" => "20x", "app" => dict! { "running" => false }, "cli" => dict! { "running" => true },
            "badge" => dict! { "label" => "工", "color" => "#30A46C" },
            "usage" => dict! {
                "account" => "alice@example.com", "email" => "alice@example.com", "key" => "工作(alice@example.com)",
                "source" => "cache", "fetched_epoch" => NOW - 60.0, "stale" => false, "error" => "出口端口 7901 不通", "error_code" => V::None,
                "five_hour" => five, "seven_day" => week, "model_scoped" => V::List(List::new()), "use_first" => V::None,
            },
            "usage_weeks" => V::None,
        }
    }

    fn agy(buckets: Vec<V>) -> Dict {
        let a = acct(V::None, V::None);
        let u = a.dict("usage").unwrap();
        u.pop("five_hour");
        u.pop("seven_day");
        u.set(
            "quota_groups",
            V::List(List::from_vec(vec![V::Dict(
                dict! { "name" => "Gemini models", "buckets" => V::List(List::from_vec(buckets)) },
            )])),
        );
        a
    }

    fn bucket(slot: &str, util: f64, resets: f64) -> V {
        V::Dict(
            dict! { "slot" => slot, "disabled" => false, "utilization" => util, "resets_epoch" => resets, "reset" => false, "remaining_amount" => V::None },
        )
    }

    #[test]
    fn what_counts_as_used_up() {
        let r = Some(NOW + 3600.0);
        assert_eq!(exhausted(&acct(w(99.5, r), w(40.0, r)), NOW), vec![("five_hour", r)], "四舍五入到 100% 算用完");
        assert!(exhausted(&acct(w(99.4, r), w(40.0, r)), NOW).is_empty(), "99%：快用完了，没用完");
        assert_eq!(exhausted(&acct(w(100.0, r), w(100.0, None)), NOW), vec![("five_hour", r), ("seven_day", None)], "5 小时在前");
        assert!(exhausted(&acct(w(100.0, Some(NOW - 1.0)), V::None), NOW).is_empty(), "过了重置时间：已经恢复了");
        let a = acct(w(100.0, r), V::None);
        a.dict("usage").unwrap().dict("five_hour").unwrap().set("reset", true);
        assert!(exhausted(&a, NOW).is_empty(), "reset：已经重置过了");
        let a = acct(w(100.0, r), V::None);
        a.dict("usage").unwrap().set("stale", true);
        assert_eq!(exhausted(&a, NOW).len(), 1, "数据旧不旧不看：没到重置时间不会降下来");
        for (k, v) in [("login", V::from("logged_out")), ("login", V::from("email_mismatch")), ("login", V::from("api"))] {
            let a = acct(w(100.0, r), V::None);
            a.set(k, v);
            assert!(exhausted(&a, NOW).is_empty(), "没登录的不说额度");
        }
        let a = acct(w(100.0, r), V::None);
        a.dict("usage").unwrap().set("error_code", "auth");
        assert!(exhausted(&a, NOW).is_empty(), "登录失效：只说登录失效");
        let a = acct(w(100.0, r), V::None);
        a.dict("usage").unwrap().set("source", "none");
        assert!(exhausted(&a, NOW).is_empty());
        let a = acct(V::None, V::None);
        a.dict("usage")
            .unwrap()
            .set("model_scoped", V::List(List::from_vec(vec![V::Dict(dict! { "name" => "Sonnet", "utilization" => 100.0 })])));
        assert!(exhausted(&a, NOW).is_empty(), "模型级的窗口不算");
        let a = acct(w(100.0, r), V::None);
        a.set("usage", V::None);
        assert!(exhausted(&a, NOW).is_empty());
    }

    #[test]
    fn antigravity_counts_the_least_used_group() {
        let (r1, r2) = (NOW + 600.0, NOW + 1200.0);
        let a = agy(vec![bucket("five_hour", 100.0, r1), bucket("five_hour", 40.0, r1)]);
        assert!(exhausted(&a, NOW).is_empty(), "一组用完了，别的组的模型照样能用");
        let a = agy(vec![bucket("five_hour", 100.0, r2), bucket("five_hour", 100.0, r1), bucket("seven_day", 100.0, r2)]);
        assert_eq!(exhausted(&a, NOW), vec![("five_hour", Some(r1)), ("seven_day", Some(r2))], "都用完了：先重置的那个");
        let a = agy(vec![bucket("five_hour", 100.0, r1), bucket("five_hour", 10.0, NOW - 5.0)]);
        assert!(exhausted(&a, NOW).is_empty(), "过了重置时间的算 0");
        let off = bucket("five_hour", 10.0, r1);
        off.as_dict().unwrap().set("disabled", true);
        let a = agy(vec![bucket("five_hour", 100.0, r1), off]);
        assert_eq!(exhausted(&a, NOW).len(), 1, "停用的不算");
        let a = agy(vec![
            bucket("five_hour", 100.0, r1),
            V::Dict(dict! { "slot" => V::None, "disabled" => false, "utilization" => 5.0 }),
        ]);
        assert_eq!(exhausted(&a, NOW).len(), 1, "认不出窗口的不算");
    }

    fn accts_of(list: &[Dict]) -> Vec<Acct> {
        let tool = dict! { "id" => "claude", "label" => "Claude Code", "accounts" => V::List(List::from_vec(list.iter().map(|d| V::Dict(d.clone())).collect())) };
        accounts(&dict! { "tools" => V::List(List::from_vec(vec![V::Dict(tool)])) }, NOW)
    }

    #[test]
    fn what_the_phone_gets() {
        let a = acct(w(100.0, Some(NOW + 60.0)), w(41.0, Some(NOW + 86400.0)));
        a.set("usage_weeks", dict! {
            "since_epoch" => 1.0,
            "weeks" => V::List(List::from_vec(vec![V::Dict(dict! { "start_epoch" => 1.0, "end_epoch" => 2.0, "used_percent" => 72, "resets" => 0, "last_epoch" => 3.0, "current" => true, "partial" => false })])),
            "summary" => dict! { "weeks" => 11, "average_percent" => 84, "maxed" => 3, "under_half" => 0 },
        });
        let accts = accts_of(&[a]);
        let refs: Vec<&Acct> = accts.iter().collect();
        let salt = "00".repeat(16);
        let doc = doc_for(&P, &refs, Some(&salt), "box", true, refs.len());
        let text = json::dumps(&V::Dict(doc.clone()), json::Fmt::new().utf8().compact());
        assert!(!text.contains("example.com") && !text.contains("7901"), "不给邮箱、出错的说明：{text}");
        let row = doc.list("accounts").unwrap().get(0).unwrap().as_dict().unwrap();
        assert_eq!(row.str("name").unwrap().to_string(), "工作");
        assert_eq!(row.str("color").unwrap().to_string(), "#30A46C");
        assert_eq!(row.get("busy").unwrap().as_bool(), Some(true));
        assert_eq!(row.str("id").unwrap().len(), 16);
        assert_eq!(
            row.str("aid").unwrap().to_string(),
            far::aid_of(&salt, "claude", "工作(alice@example.com)").unwrap(),
            "和会话说明里的一样"
        );
        let other = doc_for(&[8; 32], &refs, None, "box", true, 1);
        let orow = other.list("accounts").unwrap().get(0).unwrap().as_dict().unwrap();
        assert_ne!(orow.str("id"), row.str("id"), "按配对算：别的配对看不出是同一个账号");
        assert!(orow.get("aid").is_none(), "没有盐：没有账号摘要");
        let u = row.dict("usage").unwrap();
        assert_eq!(u.dict("five_hour").unwrap().get("utilization").unwrap().as_num(), Some(100.0));
        assert!(u.dict("five_hour").unwrap().get("resets_at").is_none());
        let wk = row.dict("weeks").unwrap();
        assert_eq!(wk.list("weeks").unwrap().len(), 1);
        assert!(wk.list("weeks").unwrap().get(0).unwrap().as_dict().unwrap().get("last_epoch").is_none());
        // 太长时先不带每周用量，再少放账号
        let (blob, _) = seal_doc(&P, &"ab".repeat(16), &refs, None, "box", NOW).unwrap().unwrap();
        let open = crypto::status_open(&P, &"ab".repeat(16), &V::Dict(blob)).unwrap();
        assert!(open.list("accounts").unwrap().get(0).unwrap().as_dict().unwrap().dict("weeks").is_some());
        assert_eq!(open.get("at").unwrap().as_int(), Some(NOW as i64));
        let big = acct(V::None, V::None);
        big.set("name", "很".repeat(64));
        let many: Vec<Dict> = (0..400).map(|_| big.copy()).collect();
        let accts = accts_of(&many);
        let refs: Vec<&Acct> = accts.iter().collect();
        let (blob, _) = seal_doc(&P, &"ab".repeat(16), &refs, None, "box", NOW).unwrap().unwrap();
        assert!(blob.str("c").unwrap().len() <= STATUS_BLOB);
        let open = crypto::status_open(&P, &"ab".repeat(16), &V::Dict(blob)).unwrap();
        let n = open.list("accounts").unwrap().len();
        assert!(n > 0 && n < 400 && open.get("more").unwrap().as_int() == Some(400 - n as i64), "{n}");
    }

    #[test]
    fn rounds_and_later() {
        let r = NOW + 3600.0;
        let accts = accts_of(&[acct(w(100.0, Some(r)), w(100.0, Some(NOW + 9.0 * 86400.0)))]);
        let refs: Vec<&Acct> = accts.iter().collect();
        let (keep, fresh, back) = plan(&P, &refs, &Dict::new(), NOW);
        assert!(keep.is_empty());
        assert_eq!(fresh.iter().map(|f| f.slot).collect::<Vec<_>>(), vec!["five_hour", "seven_day"], "第一次看到：两个都推");
        assert_eq!(back.len(), 1, "8 天以后才重置的不交给服务器（它不收）");
        assert_eq!((back[0].at, back[0].slot), (r, "five_hour"));
        assert!(back[0].id.len() == 16 && back[0].place.len() == 16 && fresh[0].id.len() == 16);
        // 推成了记下；下一次查到的重置时间差一两秒：同一轮，不再推，编号也一样
        let noted = Dict::new();
        for f in &fresh {
            noted.set(f.key.as_str(), f.anchor);
        }
        let accts2 = accts_of(&[acct(w(100.0, Some(r + 2.0)), w(100.0, Some(NOW + 9.0 * 86400.0 + 1.0)))]);
        let refs2: Vec<&Acct> = accts2.iter().collect();
        let (keep2, fresh2, back2) = plan(&P, &refs2, &noted, NOW + 300.0);
        assert!(fresh2.is_empty(), "同一轮");
        assert_eq!(keep2.len(), 2);
        assert_eq!(back2[0].id, back[0].id, "到点再推的那一条也是同一条（服务器照它去重）");
        // 下一轮（晚了 5 小时）：再推
        let accts3 = accts_of(&[acct(w(100.0, Some(r + 5.0 * 3600.0)), V::None)]);
        let refs3: Vec<&Acct> = accts3.iter().collect();
        let (_, fresh3, back3) = plan(&P, &refs3, &noted, NOW + 4.0 * 3600.0);
        assert_eq!(fresh3.len(), 1);
        assert_ne!(fresh3[0].id, fresh[0].id);
        assert_eq!(back3[0].place, back[0].place, "服务器上还是那个位置：换成新的这一条");
        // 不再用完了：推过的记到过了重置时间一天
        let accts4 = accts_of(&[acct(w(10.0, Some(r)), V::None)]);
        let refs4: Vec<&Acct> = accts4.iter().collect();
        assert_eq!(plan(&P, &refs4, &noted, NOW + 600.0).0.len(), 2);
        assert_eq!(plan(&P, &refs4, &noted, r + FORGET + 1.0).0.len(), 1, "5 小时的那一项忘掉了（本周的还没到）");
        // 不知道什么时候重置的：还用完着时记着，不用完了就忘掉
        let accts5 = accts_of(&[acct(w(100.0, None), V::None)]);
        let refs5: Vec<&Acct> = accts5.iter().collect();
        let (_, f5, b5) = plan(&P, &refs5, &Dict::new(), NOW);
        assert!(f5.len() == 1 && b5.is_empty() && f5[0].anchor == 0.0);
        let n5 = dict! { f5[0].key.as_str() => 0.0 };
        assert!(plan(&P, &refs5, &n5, NOW).1.is_empty());
        assert!(plan(&P, &refs4, &n5, NOW).0.is_empty());
    }

    #[test]
    fn notes() {
        let accts = accts_of(&[acct(w(100.0, Some(NOW + 3600.0)), V::None)]);
        let a = &accts[0];
        let n = usage_note(
            "0123456789abcdef",
            a,
            "fedcba9876543210",
            Some("aa".repeat(16)),
            "box",
            "five_hour",
            Some(NOW + 3600.0),
            false,
            NOW,
        );
        assert_eq!((n.str("kind").unwrap().to_string(), n.str("event").unwrap().to_string()), ("usage".into(), "used_up".into()));
        assert!(n.str("body").unwrap().starts_with("5 小时的额度用完了，") && n.str("body").unwrap().ends_with(" 恢复"));
        assert_eq!(n.str("subtitle").unwrap().to_string(), "工作 · Claude Code");
        let b = usage_note("0123456789abcdef", a, "fedcba9876543210", None, "box", "seven_day", Some(NOW + 3600.0), true, NOW);
        assert_eq!(b.str("body").unwrap().to_string(), "本周的额度恢复了，可以接着用了");
        assert_eq!(b.get("at").unwrap().as_int(), Some(NOW as i64 + 3600), "到点推的：时间是恢复的那一刻");
        assert!(b.get("aid").is_none());
        let sealed = crypto::note_seal(&P, &"ab".repeat(16), &n).unwrap();
        assert!(sealed.str("c").unwrap().len() <= crate::server::PUSH_BLOB, "服务器收得下");
        assert!(!json::dumps(&V::Dict(n), json::Fmt::new().utf8().compact()).contains("example.com"));
    }
}
