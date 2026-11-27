//! 手机上的「总览」「用量统计」「折合金额」「共享与同步」几页和额度的通知（docs/dev/remote.md「手机上的账号」）。这台机器上各个账号的状态
//! （登录、套餐、用量、每周用量、折合金额、在不在用）和这个配对所在的 Team 的状态按配对封好（`crypto::status_seal`，只有那个手机解得开），放到这个配对所在的那台服务器上（`status_put`）；手机查会话
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
    /// 给手机的那一项（按配对的 id、aid、折合金额另加，见 doc_for）
    row: Dict,
    /// 折合金额（cost_of）：全的、去掉按模型和按设备的（太长时用它）；还没有记录是 None
    cost: Option<(Dict, Dict)>,
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

/// 金额（cca 给的美元）写成美分的整数（手机的那一份里金额都这么写：短）；没有是 null
fn cents(d: &Dict, k: &str) -> V {
    number(d, k).map_or(V::None, |x| V::Int((x * 100.0).round() as i64))
}

/// 一项的金额：没有的算 0
fn cents0(d: &Dict, k: &str) -> V {
    match cents(d, k) {
        V::None => V::Int(0),
        v => v,
    }
}

/// 0～1 的比例（缓存命中率）留三位小数；没有是 null
fn ratio(d: &Dict, k: &str) -> V {
    number(d, k).map_or(V::None, |x| V::Float((x * 1000.0).round() / 1000.0))
}

/// 次数、周数（不小于 0 的整数）；没有是 null
fn count(d: &Dict, k: &str) -> V {
    number(d, k).map_or(V::None, |x| V::Int(x.max(0.0).round() as i64))
}

/// 时间（秒，取整）；没有是 null
fn epoch(d: &Dict, k: &str) -> V {
    number(d, k).map_or(V::None, |x| V::Int(x.floor() as i64))
}

fn text(d: &Dict, k: &str, max: usize) -> String {
    far::clean(&d.str(k).map(|s| s.to_string()).unwrap_or_default(), max)
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

/// 按模型的最多给几个（cca 排好的先后：手机上其余的照合计减出来，算「其他」）、按设备的最多给几台
const COST_MODELS: usize = 3;
const COST_DEVICES: usize = 6;

/// 账号的折合金额（PROTOCOL 2.2 的 `cost`）里手机的「折合金额」页要的，写得紧凑（docs/dev/remote.md「手机上的账号」）：金额都是美分的整数；
/// 按周的各项（最多 13 项，最后一项是这一周）、按天的 30 天只给金额，起止照首尾相接推出来（`week_end`：最后一项结束的时间，`week_hours`：
/// 各项有几个小时，换了切法的那一项不是 7 天；按天的一项一天，`day_end`：今天结束的时间）；`since`：开始有记录的时间（手机照它认哪几项
/// 开始时还没有记录）；`sum`（合计：列出的各周、30 天）、`average`（平均）、`average_n`（平均了几周、几天）、`cache_hit`（缓存命中率）都是
/// 按周、按天两个（合计照 cca 算好的给：各项四舍五入到分以后再加会差一两分）；同步没成时 `sync` 是 cca 给的状态，还没算进来的设备有几台是 `missing`。
/// brief：不带按模型的（`models`：前 COST_MODELS 个，`[模型名（没按模型记下的是 null）, 近 12 周, 近 30 天, 估算用的同系列模型（cca 的价格表里
/// 还没有它时才有）]`）和按设备的
/// （`devices`：用过它的设备有两台以上、或者有很久没同步上来的时才有，`[名字, 这台设备是 1, 近 12 周, 近 30 天, 很久没同步上来是 1, 上次同步的时间]`）：
/// 太长时先去掉它们。没有按周、按天的（还没有记录）是 None
fn cost_of(c: &Dict, brief: bool) -> Option<Dict> {
    let items =
        |k: &str| -> Vec<Dict> { c.list(k).map(|l| l.items()).unwrap_or_default().iter().filter_map(|x| x.as_dict()).collect() };
    let (weeks, days) = (items("weeks"), items("days"));
    let week_end = weeks.last().and_then(|w| number(w, "end_epoch"))?;
    let day_end = days.last().and_then(|w| number(w, "end_epoch"))?;
    let usd = |l: &[Dict]| -> Vec<V> { l.iter().map(|x| cents0(x, "usd")).collect() };
    let hours: Vec<V> = weeks
        .iter()
        .map(|w| {
            let span = number(w, "end_epoch").unwrap_or(0.0) - number(w, "start_epoch").unwrap_or(0.0);
            V::Int((span / 3600.0).round().max(0.0) as i64)
        })
        .collect();
    let s = c.dict("summary").unwrap_or_default();
    let d = dict! {
        "since" => epoch(c, "since_epoch"), "weeks" => usd(&weeks), "week_end" => week_end.floor() as i64, "week_hours" => hours,
        "days" => usd(&days), "day_end" => day_end.floor() as i64,
        "sum" => vec![cents0(&s, "weeks_usd"), cents0(&s, "days_usd")],
        "average" => vec![cents(&s, "average_usd"), cents(&s, "average_day_usd")],
        "average_n" => vec![count(&s, "average_weeks"), count(&s, "average_days")],
        "cache_hit" => vec![ratio(&s, "weeks_cache_hit"), ratio(&s, "days_cache_hit")],
    };
    if let Some(sync) = c.dict("sync") {
        let state = text(&sync, "state", 24);
        if !state.is_empty() && state != "ok" {
            d.set("sync", state);
        }
        let missing = sync.list("missing").map_or(0, |l| l.len());
        if missing > 0 {
            d.set("missing", missing);
        }
    }
    if brief {
        return Some(d);
    }
    let models: Vec<V> = items("models")
        .iter()
        .take(COST_MODELS)
        .map(|m| {
            let name = m.get("model").filter(|v| v.is_str()).map_or(V::None, |_| V::from(text(m, "model", 48)));
            let at = |k: &str| m.dict(k).map_or(V::Int(0), |x| cents0(&x, "usd"));
            let mut row = vec![name, at("weeks"), at("days")];
            let base = text(m, "priced_as", 48);
            if !base.is_empty() {
                row.push(V::from(base));
            }
            V::from(row)
        })
        .collect();
    d.set("models", models);
    let used: Vec<Dict> = items("devices").into_iter().filter(|x| flag(x, "used")).collect();
    if used.len() >= 2 || used.iter().any(|x| flag(x, "stale") && !flag(x, "me")) {
        let rows: Vec<V> = used
            .iter()
            .take(COST_DEVICES)
            .map(|x| {
                let ps = x.dict("summary").unwrap_or_default();
                V::from(vec![
                    V::from(text(x, "name", 48)),
                    V::Int(flag(x, "me") as i64),
                    cents0(&ps, "weeks_usd"),
                    cents0(&ps, "days_usd"),
                    V::Int(flag(x, "stale") as i64),
                    epoch(x, "synced_epoch"),
                ])
            })
            .collect();
        d.set("devices", rows);
    }
    Some(d)
}

/// 这台电脑的折合金额里手机要的那几样（PROTOCOL 2.2 的 `cost_total`）：价格表的日期、上次统计的时间（还在做第一次统计时 `scanning`）、
/// 这台设备近 30 天认不出是哪个账号的（美分，`unattributed`：只算这台设备自己的，几台电脑的加起来不会重复）。折合金额关着、没有官方账号
/// （`cost_total` 是 null）时是 None
fn cost_meta(snap: &Dict) -> Option<Dict> {
    let t = snap.dict("cost_total")?;
    let scan = t.dict("scan").unwrap_or_default();
    let d = dict! { "prices_date" => text(&t, "prices_date", 16), "updated" => epoch(&scan, "updated_epoch") };
    if scan.str("state").as_deref() == Some("scanning") {
        d.set("scanning", true);
    }
    if let Some(V::Int(n)) = t.dict("unattributed").map(|u| cents(&u, "days_usd")) {
        if n > 0 {
            d.set("unattributed", n);
        }
    }
    Some(d)
}

/// 这个配对所在的 Team（PROTOCOL 4.14 的 `config.teams` 里编号对得上的那一项）里手机的「共享与同步」页要的：名字、这台设备的角色
/// （`role`：admin、member；`coadmin`：别的管理员授予的）、同步的状态（`state`；没同步成时 cca 给的原因 `error`；`seq`：配置同步到第几版，
/// 0 是还没同步过）、这台设备在里面叫什么（`device`）、分给它几个账号、服务器的地址（`host`）、什么时候加入的（`joined`），服务器要更新时
/// 为什么（`update`）。只给这一个 Team 的：和账号一样分 Team（别的 Team 经别的服务器）
fn team_of(snap: &Dict, hub: HubKind) -> Option<Dict> {
    let id = hub.team();
    let teams = snap.dict("config")?.list("teams")?;
    let t = teams.items().iter().filter_map(|t| t.as_dict()).find(|t| t.str("id").as_deref() == Some(id.as_str()))?;
    let d = dict! {
        "name" => text(&t, "name", 64), "role" => text(&t, "role", 16), "state" => text(&t, "state", 16), "seq" => count(&t, "seq"),
        "accounts" => count(&t, "accounts"), "device" => text(&t, "device", 64), "host" => text(&t, "host", 128),
    };
    if flag(&t, "coadmin") {
        d.set("coadmin", true);
    }
    let error = text(&t, "error", 160);
    if !error.is_empty() {
        d.set("error", error);
    }
    if let V::Int(j) = epoch(&t, "joined_epoch") {
        d.set("joined", j);
    }
    if let Some(u) = t.dict("server_update") {
        d.set("update", text(&u, "why", 160));
    }
    Some(d)
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
    // 在用什么（总览的卡片左上角）：桌面 App 开着、命令行里几个会话、最早那个从什么时候开始的；没在用的不写
    if running("app") {
        d.set("app", true);
    }
    if let Some(cli) = a.dict("cli").filter(|c| flag(c, "running")) {
        d.set("cli", number(&cli, "sessions").map_or(1, |n| n.max(1.0) as i64));
        if let V::Int(t) = epoch(&cli, "since_epoch") {
            d.set("since", t);
        }
    }
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
                cost: a.dict("cost").and_then(|c| Some((cost_of(&c, false)?, cost_of(&c, true)?))),
                used: exhausted(&a, now),
                name,
            });
        }
    }
    out
}

/// 放多少（太长时一级一级少放，seal_doc）
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fit {
    /// 带每周用量
    weeks: bool,
    cost: CostFit,
    /// 只放前几个账号
    n: usize,
}

/// 折合金额带多少：全带、不带按模型和按设备的、不带
#[derive(Clone, Copy, Debug, PartialEq)]
enum CostFit {
    Full,
    Brief,
    Off,
}

/// 给一个配对的那一份里不是哪个账号的：这个 Team 的盐（算 aid）、机器名、这台电脑的折合金额那几样（cost_meta）、这个配对所在的 Team（team_of）
struct Ctx<'a> {
    salt: Option<&'a str>,
    machine: &'a str,
    cost: Option<&'a Dict>,
    team: Option<&'a Dict>,
}

/// 给一个配对的那一份（PROTOCOL 见 docs/dev/remote.md「手机上的账号」）：走这台服务器的账号（`accts`）。`id`：按配对算的账号编号；
/// `aid`：会话说明里的同一个账号摘要（有这个 Team 的盐、账号有邮箱时才有）；`cost`：账号的折合金额（cost_of）。外面的 `cost` 是这台电脑的
/// 折合金额那几样（没带账号的折合金额时是 false），`team` 是这个配对所在的 Team。fit：带多少（太长时）
fn doc_for(p: &[u8], accts: &[&Acct], cx: &Ctx, fit: Fit) -> Dict {
    let list = List::new();
    for a in accts.iter().take(fit.n) {
        let row = a.row.copy();
        row.set("id", a.tag(p));
        if let Some(aid) = cx.salt.and_then(|s| far::aid_of(s, &a.tool, &a.key)) {
            row.set("aid", aid);
        }
        if !fit.weeks {
            row.set("weeks", V::None);
        }
        match (&a.cost, fit.cost) {
            (Some((full, _)), CostFit::Full) => row.set("cost", full),
            (Some((_, brief)), CostFit::Brief) => row.set("cost", brief),
            _ => {}
        }
        list.push(row);
    }
    let d = dict! { "v" => 1, "machine" => cx.machine, "accounts" => list };
    if !fit.weeks {
        // 太长，没带每周用量：手机照它说一声，不说成还没有记录
        d.set("weeks", false);
    }
    if let Some(c) = cx.cost {
        if fit.cost == CostFit::Off {
            // 太长，没带折合金额：同上
            d.set("cost", false);
        } else {
            let c = c.copy();
            if fit.cost == CostFit::Brief {
                c.set("brief", true);
            }
            d.set("cost", c);
        }
    }
    if let Some(t) = cx.team {
        d.set("team", t);
    }
    if fit.n < accts.len() {
        d.set("more", accts.len() - fit.n);
    }
    d
}

/// 封好的那一份和它的摘要（不算放的时间：没变就不用再放）。太长（服务器不收）时一级一级少放：先不带折合金额里按模型、按设备的，
/// 再不带每周用量，再不带折合金额，最后一个个少放账号；一个都放不下是 None
fn seal_doc(p: &[u8], pid: &str, accts: &[&Acct], cx: &Ctx, now: f64) -> R<Option<(Dict, String)>> {
    let all = accts.len();
    let fit = |weeks, cost, n| Fit { weeks, cost, n };
    let mut tries = vec![fit(true, CostFit::Full, all), fit(true, CostFit::Brief, all), fit(false, CostFit::Brief, all)];
    tries.extend((0..=all).rev().map(|n| fit(false, CostFit::Off, n)));
    for f in tries {
        let doc = doc_for(p, accts, cx, f);
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
fn publish_peer(srv: &Server, peer: &Peer, p: &[u8], accts: &[&Acct], cx: &Ctx, now: f64, st: &Dict) -> R<()> {
    if number(st, "old").is_some_and(|t| (0.0..OLD_AGAIN).contains(&(now - t))) {
        return Ok(()); // 这台服务器还不认：过一阵再试
    }
    let pid = peer.pid.as_str();
    let (salt, machine) = (cx.salt, cx.machine);
    let aid = |a: &Acct| salt.and_then(|s| far::aid_of(s, &a.tool, &a.key));
    let (keep, fresh, back) = plan(p, accts, &st.dict("noted").unwrap_or_default(), now);
    let Some((blob, sum)) = seal_doc(p, pid, accts, cx, now)? else { return Ok(()) };
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
        let snap = if phones.iter().any(|p| p.incoming != Incoming::None) { Some(gui_snapshot(false)?) } else { None };
        let accts = snap.as_ref().map_or_else(Vec::new, |s| accounts(s, now));
        let cost = snap.as_ref().and_then(cost_meta);
        let mut hubs: Vec<HubKind> = Vec::new();
        for p in &phones {
            if !hubs.contains(&p.hub) {
                hubs.push(p.hub);
            }
        }
        for hub in hubs {
            let Ok(srv) = relay::server_of(hub)? else { continue };
            let salt = far::team_salt(hub);
            let team = snap.as_ref().and_then(|s| team_of(s, hub));
            let cx = Ctx { salt: salt.as_deref(), machine: &machine, cost: cost.as_ref(), team: team.as_ref() };
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
                publish_peer(&srv, peer, &secret, &mine, &cx, now, &st)?;
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

    fn cx(salt: Option<&str>) -> Ctx<'_> {
        Ctx { salt, machine: "box", cost: None, team: None }
    }

    fn full(n: usize) -> Fit {
        Fit { weeks: true, cost: CostFit::Full, n }
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
        let doc = doc_for(&P, &refs, &cx(Some(&salt)), full(refs.len()));
        let text = json::dumps(&V::Dict(doc.clone()), json::Fmt::new().utf8().compact());
        assert!(!text.contains("example.com") && !text.contains("7901"), "不给邮箱、出错的说明：{text}");
        let row = doc.list("accounts").unwrap().get(0).unwrap().as_dict().unwrap();
        assert_eq!(row.str("name").unwrap().to_string(), "工作");
        assert_eq!(row.str("color").unwrap().to_string(), "#30A46C");
        assert_eq!(row.get("busy").unwrap().as_bool(), Some(true));
        assert_eq!(
            (row.get("cli").unwrap().as_int(), row.get("app").is_none()),
            (Some(1), true),
            "命令行里在用；桌面 App 没开着的不写"
        );
        assert_eq!(row.str("id").unwrap().len(), 16);
        assert_eq!(
            row.str("aid").unwrap().to_string(),
            far::aid_of(&salt, "claude", "工作(alice@example.com)").unwrap(),
            "和会话说明里的一样"
        );
        let other = doc_for(&[8; 32], &refs, &cx(None), full(1));
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
        let (blob, _) = seal_doc(&P, &"ab".repeat(16), &refs, &cx(None), NOW).unwrap().unwrap();
        let open = crypto::status_open(&P, &"ab".repeat(16), &V::Dict(blob)).unwrap();
        assert!(open.list("accounts").unwrap().get(0).unwrap().as_dict().unwrap().dict("weeks").is_some());
        assert!(open.get("cost").is_none() && open.get("team").is_none(), "折合金额关着、不在 Team 里：都不写");
        assert_eq!(open.get("at").unwrap().as_int(), Some(NOW as i64));
        let big = acct(V::None, V::None);
        big.set("name", "很".repeat(64));
        let many: Vec<Dict> = (0..400).map(|_| big.copy()).collect();
        let accts = accts_of(&many);
        let refs: Vec<&Acct> = accts.iter().collect();
        let (blob, _) = seal_doc(&P, &"ab".repeat(16), &refs, &cx(None), NOW).unwrap().unwrap();
        assert!(blob.str("c").unwrap().len() <= STATUS_BLOB);
        let open = crypto::status_open(&P, &"ab".repeat(16), &V::Dict(blob)).unwrap();
        let n = open.list("accounts").unwrap().len();
        assert!(n > 0 && n < 400 && open.get("more").unwrap().as_int() == Some(400 - n as i64), "{n}");
    }

    /// 账号的折合金额（PROTOCOL 2.2 的样子）：weeks 项按周的（最后一项是这一周，倒数第三项换过切法、只有 3 天）、30 天按天的、
    /// 4 个模型（第二个是按同系列估算的）、devices 台用过它的设备（第二台很久没同步上来）和一台只为诊断列出来的
    fn cost_fx(weeks: usize, devices: usize) -> Dict {
        let week = 7.0 * 86400.0;
        let lens: Vec<f64> = (0..weeks).map(|i| if i + 3 == weeks { 3.0 * 86400.0 } else { week }).collect();
        let mut t = NOW + 2.0 * 86400.0 - lens.iter().sum::<f64>();
        let ws: Vec<V> = lens
            .iter()
            .enumerate()
            .map(|(i, len)| {
                let d = dict! {
                    "start_epoch" => t, "end_epoch" => t + len, "current" => i + 1 == weeks, "partial" => i == 0,
                    "usd" => i as f64 * 1.254 + 0.004, "requests" => 3, "unpriced_requests" => 0, "basis" => "quota",
                };
                t += len;
                V::Dict(d)
            })
            .collect();
        let day_end = NOW + 3600.0;
        let ds: Vec<V> = (0..30)
            .map(|i| {
                V::Dict(dict! {
                    "start_epoch" => day_end - (30 - i) as f64 * 86400.0, "end_epoch" => day_end - (29 - i) as f64 * 86400.0,
                    "date" => format!("2026-11-{:02}", i % 28 + 1), "current" => i == 29, "partial" => false,
                    "usd" => if i % 2 == 0 { 0.5 } else { 0.0 },
                })
            })
            .collect();
        let dev = |name: &str, me: bool, used: bool, stale: bool| {
            V::Dict(dict! {
                "id" => name, "name" => name, "me" => me, "used" => used, "stale" => stale, "synced_epoch" => NOW - 7200.0,
                "weeks_usd" => V::List(List::new()), "days_usd" => V::List(List::new()),
                "summary" => dict! { "weeks_usd" => 12.345, "days_usd" => 1.5 },
            })
        };
        let mut devs = vec![dev("studio", true, true, false)];
        for i in 1..devices {
            devs.push(dev(&format!("box-{i}"), false, true, i == 1));
        }
        devs.push(dev("old", false, false, false));
        let model = |m: V, usd: f64, est: bool| {
            let d = dict! { "model" => m, "weeks" => dict! { "usd" => usd, "requests" => 1 }, "days" => dict! { "usd" => usd / 2.0, "requests" => 1 } };
            if est {
                d.set("priced_as", "claude-sonnet-5");
            }
            V::Dict(d)
        };
        dict! {
            "currency" => "USD", "prices_date" => "2026-11-01", "since_epoch" => NOW - 80.0 * 86400.0 + 0.7, "weeks" => ws, "days" => ds,
            "summary" => dict! {
                "weeks_usd" => 99.0, "current_usd" => 9.0, "average_usd" => 7.456, "average_weeks" => 11, "days_usd" => 7.5, "today_usd" => 0.5,
                "average_day_usd" => 0.25, "average_days" => 29, "requests" => 40, "unpriced_requests" => 2, "weeks_cache_hit" => 0.97349,
                "days_cache_hit" => V::None,
            },
            "devices" => devs,
            "models" => vec![model(V::from("claude-opus-5-5"), 50.0, false), model(V::from("claude-sonnet-5-5"), 20.0, true), model(V::None, 5.0, false), model(V::from("x"), 1.0, false)],
            "sync" => dict! { "state" => "error", "message" => "服务器 192.0.2.10 不通", "missing" => vec![V::Dict(dict! { "id" => "m", "name" => "mini" })] },
            "unpriced_models" => V::List(List::new()),
        }
    }

    fn ints(d: &Dict, k: &str) -> Vec<i64> {
        d.list(k).unwrap().items().iter().map(|v| v.as_int().unwrap()).collect()
    }

    #[test]
    fn cost_for_the_phone() {
        let c = cost_of(&cost_fx(13, 3), false).unwrap();
        let weeks = ints(&c, "weeks");
        assert_eq!((weeks.len(), weeks[0], weeks[1], weeks[12]), (13, 0, 126, 1505), "美分：0.004 → 0、1.258 → 126");
        let hours = ints(&c, "week_hours");
        assert_eq!(hours[10], 72, "换了切法的那一项 3 天");
        assert!(hours.iter().enumerate().all(|(i, h)| i == 10 || *h == 168), "{hours:?}");
        assert_eq!(c.get("week_end").unwrap().as_int(), Some((NOW + 2.0 * 86400.0) as i64));
        let days = ints(&c, "days");
        assert_eq!((days.len(), days[0], days[1]), (30, 50, 0));
        assert_eq!(c.get("day_end").unwrap().as_int(), Some((NOW + 3600.0) as i64));
        assert_eq!(c.get("since").unwrap().as_int(), Some((NOW - 80.0 * 86400.0) as i64));
        assert_eq!(ints(&c, "average"), vec![746, 25]);
        assert_eq!(ints(&c, "average_n"), vec![11, 29]);
        assert_eq!(ints(&c, "sum"), vec![9900, 750], "合计照 cca 给的");
        let hit = c.list("cache_hit").unwrap();
        assert_eq!(hit.get(0).unwrap().as_num(), Some(0.973));
        assert!(matches!(hit.get(1), Some(V::None)), "cca 没给的照样是 null");
        assert_eq!(c.str("sync").as_deref(), Some("error"));
        assert_eq!(c.get("missing").unwrap().as_int(), Some(1));
        let models = c.list("models").unwrap();
        assert_eq!(models.len(), COST_MODELS, "只给前几个");
        let m0 = models.get(0).unwrap().as_list().unwrap();
        assert_eq!(
            (m0.get(0).unwrap().as_str().map(String::from), m0.get(1).unwrap().as_int(), m0.get(2).unwrap().as_int()),
            (Some("claude-opus-5-5".into()), Some(5000), Some(2500))
        );
        let m1 = models.get(1).unwrap().as_list().unwrap();
        assert_eq!(
            (m1.len(), m1.get(3).unwrap().as_str().map(String::from)),
            (4, Some("claude-sonnet-5".into())),
            "按同系列估算的"
        );
        assert!(matches!(models.get(2).unwrap().as_list().unwrap().get(0), Some(V::None)), "没按模型记下的");
        let devices = c.list("devices").unwrap();
        assert_eq!(devices.len(), 3, "只为诊断列出来的不给");
        let d0 = devices.get(0).unwrap().as_list().unwrap();
        let row: Vec<String> = d0.items().iter().map(|v| v.py_repr()).collect();
        assert_eq!(row, vec!["'studio'", "1", "1235", "150", "0", &format!("{}", (NOW - 7200.0) as i64)]);
        assert_eq!(devices.get(1).unwrap().as_list().unwrap().get(4).unwrap().as_int(), Some(1), "很久没同步上来");
        let text = json::dumps(&V::Dict(c.clone()), json::Fmt::new().utf8().compact());
        assert!(!text.contains("192.0.2.10") && !text.contains("tokens"), "不给同步出错的说明、token 数：{text}");
        // brief：去掉按模型、按设备的
        let b = cost_of(&cost_fx(13, 3), true).unwrap();
        assert!(b.get("models").is_none() && b.get("devices").is_none() && b.list("weeks").is_some_and(|w| w.len() == 13));
        // 只有一台设备、都同步着：不给按设备的
        let one = cost_fx(13, 1);
        assert!(cost_of(&one, false).unwrap().get("devices").is_none());
        // 还没有记录
        assert!(cost_of(&dict! { "weeks" => V::List(List::new()), "days" => V::List(List::new()) }, false).is_none());
        // 同步着的不写 sync
        let ok = cost_fx(2, 1);
        ok.set("sync", dict! { "state" => "ok", "missing" => V::List(List::new()) });
        let c = cost_of(&ok, false).unwrap();
        assert!(c.get("sync").is_none() && c.get("missing").is_none());
        assert_eq!(ints(&c, "week_hours"), vec![168, 168]);
    }

    #[test]
    fn team_and_host_cost_for_the_phone() {
        let tid = "ab".repeat(16);
        let hub = HubKind::Team(crate::cmd::hub::TeamTag::new(&tid).unwrap());
        let team = |id: &str, state: &str| {
            V::Dict(dict! {
                "id" => id, "name" => "示例 Team", "role" => "admin", "coadmin" => true, "url" => "https://hub.example.com:18445",
                "host" => "hub.example.com:18445", "device" => "studio", "state" => state, "error" => V::None, "seq" => 7, "accounts" => 5,
                "joined_epoch" => NOW - 86400.0, "server_level" => 1,
                "server_update" => dict! { "level" => 1, "need" => 2, "why" => "手机上查看账号" },
            })
        };
        let snap = dict! {
            "config" => dict! { "teams" => vec![team(&"cd".repeat(16), "error"), team(&tid, "ok")] },
            "cost_total" => dict! {
                "prices_date" => "2026-11-01", "scan" => dict! { "state" => "done", "updated_epoch" => NOW - 60.0 },
                "unattributed" => dict! { "days_usd" => 3.456, "requests" => 9 },
            },
        };
        let t = team_of(&snap, hub).unwrap();
        let text = json::dumps(&V::Dict(t.clone()), json::Fmt::new().utf8().compact());
        assert_eq!(
            text,
            format!(
                r#"{{"name":"示例 Team","role":"admin","state":"ok","seq":7,"accounts":5,"device":"studio","host":"hub.example.com:18445","coadmin":true,"joined":{},"update":"手机上查看账号"}}"#,
                (NOW - 86400.0) as i64
            ),
            "只给这个配对所在的那一个 Team；不给地址里的协议、编号"
        );
        let other = HubKind::Team(crate::cmd::hub::TeamTag::new(&"ef".repeat(16)).unwrap());
        assert!(team_of(&snap, other).is_none());
        let m = cost_meta(&snap).unwrap();
        assert_eq!(
            json::dumps(&V::Dict(m), json::Fmt::new().utf8().compact()),
            format!(r#"{{"prices_date":"2026-11-01","updated":{},"unattributed":346}}"#, (NOW - 60.0) as i64)
        );
        assert!(cost_meta(&dict! { "cost_total" => V::None }).is_none(), "折合金额关着");
        // 在文档里：外面的 cost、team，账号的 cost
        let a = acct(w(10.0, Some(NOW + 60.0)), V::None);
        a.set("cost", cost_fx(13, 3));
        a.set("app", dict! { "running" => true, "pid" => 42 });
        a.set("cli", dict! { "running" => true, "sessions" => 2, "since_epoch" => NOW - 600.5 });
        let accts = accts_of(&[a]);
        let refs: Vec<&Acct> = accts.iter().collect();
        let meta = cost_meta(&snap).unwrap();
        let c = Ctx { salt: None, machine: "box", cost: Some(&meta), team: Some(&t) };
        let doc = doc_for(&P, &refs, &c, full(1));
        let row = doc.list("accounts").unwrap().get(0).unwrap().as_dict().unwrap();
        assert_eq!(row.dict("cost").unwrap().list("models").unwrap().len(), COST_MODELS);
        assert_eq!(
            (row.get("app").unwrap().as_bool(), row.get("cli").unwrap().as_int(), row.get("since").unwrap().as_int()),
            (Some(true), Some(2), Some((NOW - 601.0) as i64))
        );
        assert!(row.get("pid").is_none());
        assert_eq!(doc.dict("team").unwrap().str("name").as_deref(), Some("示例 Team"));
        assert!(doc.dict("cost").unwrap().get("brief").is_none());
        let brief = doc_for(&P, &refs, &c, Fit { weeks: true, cost: CostFit::Brief, n: 1 });
        assert_eq!(brief.dict("cost").unwrap().get("brief").unwrap().as_bool(), Some(true));
        assert!(brief.list("accounts").unwrap().get(0).unwrap().as_dict().unwrap().dict("cost").unwrap().get("models").is_none());
        let off = doc_for(&P, &refs, &c, Fit { weeks: false, cost: CostFit::Off, n: 1 });
        assert_eq!(off.get("cost").unwrap().as_bool(), Some(false));
        assert!(off.list("accounts").unwrap().get(0).unwrap().as_dict().unwrap().get("cost").is_none());
        assert!(off.dict("team").is_some(), "Team 的那一项一直带着");
    }

    #[test]
    fn too_long_drops_in_order() {
        // 账号多到放不下：先去掉折合金额里按模型、按设备的，再去掉每周用量，再去掉折合金额，最后少放账号
        let a = acct(w(10.0, Some(NOW + 60.0)), w(20.0, Some(NOW + 86400.0)));
        a.set("cost", cost_fx(13, 6));
        let weeks: Vec<V> = (0..13)
            .map(|i| {
                V::Dict(dict! {
                    "start_epoch" => NOW - (13 - i) as f64 * 604800.0, "end_epoch" => NOW - (12 - i) as f64 * 604800.0, "used_percent" => 50,
                    "resets" => 0, "current" => i == 12, "partial" => false,
                })
            })
            .collect();
        a.set("usage_weeks", dict! { "since_epoch" => 1.0, "weeks" => weeks, "summary" => dict! { "weeks" => 12, "average_percent" => 50, "maxed" => 0, "under_half" => 0 } });
        let meta = dict! { "prices_date" => "2026-11-01", "updated" => NOW as i64 };
        let c = Ctx { salt: None, machine: "box", cost: Some(&meta), team: None };
        let pid = "ab".repeat(16);
        let fits = |n: usize| -> (Option<bool>, bool, bool, usize) {
            let many: Vec<Dict> = (0..n).map(|_| a.copy()).collect();
            let accts = accts_of(&many);
            let refs: Vec<&Acct> = accts.iter().collect();
            let (blob, _) = seal_doc(&P, &pid, &refs, &c, NOW).unwrap().unwrap();
            let open = crypto::status_open(&P, &pid, &V::Dict(blob)).unwrap();
            let cost = open.get("cost").unwrap();
            let brief = cost.as_dict().map(|d| d.truthy("brief"));
            let first = open.list("accounts").unwrap().get(0).unwrap().as_dict().unwrap();
            (brief, first.dict("weeks").is_some(), first.dict("cost").is_some(), open.list("accounts").unwrap().len())
        };
        assert_eq!(fits(2), (Some(false), true, true, 2), "放得下：全带");
        let mut seen = Vec::new();
        for n in [20, 30, 40, 60, 80, 120] {
            let (brief, weeks, cost, shown) = fits(n);
            seen.push((brief, weeks, cost, shown == n));
        }
        // 每一档都照先后：brief 之后才去掉每周用量，去掉每周用量之后才去掉折合金额，都去掉了才少放账号
        for (brief, weeks, cost, all) in &seen {
            match brief {
                Some(false) => assert!(*weeks && *cost && *all),
                Some(true) => assert!(*cost && *all),
                None => assert!(!*weeks && !*cost),
            }
            if !*all {
                assert!(brief.is_none() && !*weeks && !*cost);
            }
        }
        assert!(seen.iter().any(|s| s.0 == Some(true)), "{seen:?}");
        assert!(seen.iter().any(|s| s.0 == Some(true) && !s.1), "{seen:?}");
        assert!(seen.iter().any(|s| s.0.is_none() && s.3), "{seen:?}");
        assert!(seen.iter().any(|s| !s.3), "{seen:?}");
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
