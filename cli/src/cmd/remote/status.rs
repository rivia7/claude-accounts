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
use crate::cmd::gui::{gui_snapshot, update_check_file, ver_key};
use crate::cmd::hub::HubKind;
use crate::cmd::presence::{flock, spawn_background};
use crate::dict;
use crate::py::fs;
use crate::py::hash::sha256_hex;
use crate::py::json;
use crate::py::obj::{Dict, List, V};
use crate::py::path;
use crate::py::proc::{getenv, Cmd as Proc, In, Out};
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
        account_tag(p, &self.tool, &self.ident)
    }
}

/// 一个账号按配对算的编号（ident：`acct_id`，账号目录名）。远程启动的说明里能挑的账号也用它（template.rs 的 `Pick::tag`）：
/// 手机照它认出是「账号」页里的哪一个
pub(super) fn account_tag(p: &[u8], tool: &str, ident: &str) -> String {
    tag(p, &["acct", tool, ident])
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
    // Claude Code 登录时记下的套餐过时了（升降级以后要重新登录才会更新）：记下的那个（Mac App 的 Diagnosis.outdatedLoginPlan）
    let old = text(u, "outdated_login_plan", 32);
    if !old.is_empty() {
        d.set("outdated_login_plan", old);
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

/// Antigravity 的每周用量最多给几组（agy 现在给两组）
const GROUP_WEEKS: usize = 4;

/// Antigravity 各组的每周用量（PROTOCOL 2.2 的 `usage_weeks_groups`）：一组一项，认这一组的 id、agy 给的组名（手机照 Mac App 的译法显示），
/// 加上 weeks_of 的几项；不是对象、没有 id 的不要
fn groups_of(l: &List) -> List {
    let out = List::new();
    for g in l.items() {
        let Some(g) = g.as_dict() else { continue };
        let id = far::clean(&g.str("id").map(|s| s.to_string()).unwrap_or_default(), 64);
        if id.is_empty() || out.len() >= GROUP_WEEKS {
            continue;
        }
        let d = dict! { "id" => id, "name" => far::clean(&g.str("name").map(|s| s.to_string()).unwrap_or_default(), 64) };
        d.update(&weeks_of(&g));
        out.push(d);
    }
    out
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

/// 账号是哪个 Team 给的（PROTOCOL 2.2 账号的 `team`，Team 的编号）：那个 Team 的名字（没记名字时是编号的前 8 位，和配对的链接里一样）；
/// 这台设备已经不在那个 Team 里了是 None（手机按 Team 筛时照「不在 Team 里的」）
fn team_name(snap: &Dict, id: &str) -> Option<String> {
    let teams = snap.dict("config")?.list("teams")?;
    let t = teams.items().iter().filter_map(|t| t.as_dict()).find(|t| t.str("id").as_deref() == Some(id))?;
    let name = text(&t, "name", 64);
    Some(if name.is_empty() { far::clean(&id.chars().take(8).collect::<String>(), 8) } else { name })
}

/// 这台电脑的几样（手机上那台电脑的一栏）：cca 的版本（`cca`）、有新版本时是哪个（`update`，update_of）、「远程会话」的开关（`remote`：
/// 之后启动的会话放不放进会话宿主，manage.rs 的 `remote_on`）、Mac App 页顶的那几条提示（`problems`：账号表校验不过、共享设置里有会盖掉
/// 账号代理 / 接口 / 凭证的项、文件读不了，加上 extra：后台查用量要同意、没查成，内置代理停了）。提示只给原因码、哪个工具和共享设置里的项
/// （来源和配置键，不带值），不给说明：说明里有路径、账号表的键、端口
fn host_of(snap: &Dict, remote: bool, extra: &[(String, String)], update: Option<&str>) -> Dict {
    let d = Dict::new();
    let v = text(snap, "cca_version", 32);
    if !v.is_empty() {
        d.set("cca", v);
    }
    d.set("remote", remote);
    if let Some(u) = update {
        d.set("update", u);
    }
    let problems = List::new();
    let add = |code: String, tool: Option<String>, items: Option<List>| {
        let p = dict! { "code" => code };
        if let Some(t) = tool.filter(|t| !t.is_empty()) {
            p.set("tool", t);
        }
        if let Some(items) = items {
            p.set("items", items);
        }
        let same = |x: &V| x.as_dict().is_some_and(|x| x.str("code") == p.str("code") && x.str("tool") == p.str("tool"));
        if problems.len() < PROBLEMS_MAX && !problems.items().iter().any(same) {
            problems.push(p);
        }
    };
    // 照 Mac App 页顶的先后：账号表校验不过、共享设置有问题（各个工具的），文件读不了，后台查用量的（extra 里先要同意的、再没查成的），
    // 内置代理停了
    let tools: Vec<Dict> = snap.list("tools").map(|l| l.items()).unwrap_or_default().iter().filter_map(|t| t.as_dict()).collect();
    for tool in &tools {
        let id = text(tool, "id", 16);
        for e in tool.list("errors").map(|l| l.items()).unwrap_or_default() {
            let code = e.as_dict().map(|e| text(&e, "code", 32)).unwrap_or_default();
            if !code.is_empty() {
                add(code, Some(id.clone()), None);
            }
        }
    }
    for tool in &tools {
        let id = text(tool, "id", 16);
        let items: Vec<V> = tool
            .list("settings_problems")
            .map(|l| l.items())
            .unwrap_or_default()
            .iter()
            .filter_map(|s| s.as_str().map(|s| far::clean(s, 64)))
            .filter(|s| !s.is_empty())
            .take(PROBLEM_ITEMS)
            .map(V::from)
            .collect();
        if !items.is_empty() {
            add("settings_problems".to_string(), Some(id), Some(List::from_vec(items)));
        }
    }
    for e in snap.list("errors").map(|l| l.items()).unwrap_or_default() {
        let Some(e) = e.as_dict() else { continue };
        let code = text(&e, "code", 32);
        if !code.is_empty() {
            add(code, Some(text(&e, "tool", 16)), None);
        }
    }
    for (code, tool) in extra {
        add(code.clone(), Some(tool.clone()), None);
    }
    if !problems.is_empty() {
        d.set("problems", problems);
    }
    d
}

/// 页顶的提示最多给几条、共享设置里的项最多列几个
const PROBLEMS_MAX: usize = 8;
const PROBLEM_ITEMS: usize = 6;

// --- 电脑上另外查的：端口池的实况、有没有新版本 ------------------------------------------------------------------------------

/// 端口池的实况（`cca _gui live`：在家经 SSH 问路由器、在外面问内置代理，再挨个连账号的端口）多久查一次：Mac App 的总览看得见时
/// 两分钟一次，这里在后台，放慢些
const LIVE_EVERY: f64 = 600.0;
/// 上次查到了问题（端口连不上、节点选错了、内置代理停了）：两分钟就再查（和 Mac App 的总览看得见时一样快），好了马上不说
const LIVE_RECHECK: f64 = 120.0;
/// 记着的实况过了这么久不再给（这期间没人叫这里放一轮，不知道现在怎样）
const LIVE_STALE: f64 = 1800.0;
/// `_gui live` 最多等几秒（问路由器要 SSH 过去）
const LIVE_WAIT: f64 = 45.0;
/// 新版本要是这么久以内查到的才说（App 一天查一次；记着的太旧就不说了）
const UPDATE_FRESH: f64 = 7.0 * 86400.0;

/// 端口池的实况里给手机的（PROTOCOL 2.3 的 `slots`、`local`）：哪几个账号的端口连不上（closed）、路由器给它选错了节点（drift），和
/// Mac App 的 Diagnosis 一个口径；走内置代理、它却没在运行（core_down：Mac App 页顶的「内置代理停了」）。账号按 `live_tag` 认：
/// 记在 remote/status.json 里的不带邮箱、账号表的键
#[derive(Debug, Default, PartialEq)]
struct Live {
    ports: Vec<(String, String)>,
    core_down: bool,
}

/// 实况里认账号的：工具和账号表的键的 SHA-256 的前 16 个字符
fn live_tag(tool: &str, key: &str) -> String {
    sha256_hex(format!("{tool}\0{key}"))[..16].to_string()
}

/// `_gui live` 的那一份里给手机的
fn live_of(doc: &Dict) -> Live {
    let mut ports = Vec::new();
    for s in doc.list("slots").map(|l| l.items()).unwrap_or_default() {
        let Some(s) = s.as_dict() else { continue };
        let (Some(tool), Some(key), Some(st)) = (s.str("tool"), s.str("key"), s.str("state")) else { continue };
        if matches!(&*st, "closed" | "drift") {
            ports.push((live_tag(&tool, &key), st.to_string()));
        }
    }
    let local = doc.dict("local").unwrap_or_default();
    let stopped = local.get("running").and_then(|v| v.as_bool()) == Some(false);
    Live { ports, core_down: doc.str("route").as_deref() == Some("local") && flag(&local, "configured") && stopped }
}

/// 记着的实况（remote/status.json 的 `live`：`{"at", "ports": {live_tag: closed|drift}, "core_down"}`，没查成的只有 `at`）
fn live_saved(l: &Dict) -> Option<Live> {
    let ports = l.dict("ports")?;
    let ports = ports.items().into_iter().filter_map(|(k, v)| v.as_str().map(|s| (k.to_string(), s.to_string()))).collect();
    Some(Live { ports, core_down: flag(l, "core_down") })
}

/// 放这一轮用的实况：记着的还不到 LIVE_STALE 才用（查是 `live_refresh` 的事，放完这一轮才查）
fn live_now(state: &Dict, now: f64) -> Live {
    state
        .dict("live")
        .filter(|l| number(l, "at").is_some_and(|at| (0.0..LIVE_STALE).contains(&(now - at))))
        .and_then(|l| live_saved(&l))
        .unwrap_or_default()
}

/// 这台电脑上 Mac App 看得到的那几样（端口池的实况、后台查用量的结果）现在算不算数：macOS 上是界面 App 开着（bash 的 `gui_running`：
/// 它不开着时内置代理没人托管、没人在后台查用量，Mac 上也什么都不显示；启动账号时 cca 会把它叫起来）；Linux 上没有界面 App，总是算
fn watched() -> bool {
    if !cfg!(target_os = "macos") {
        return true;
    }
    let app = getenv("CCA_GUI_APP")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| path::join_all(&[&crate::core::base::HOME, "Applications", "cca.app"]));
    let pat = format!("{app}/Contents/MacOS/cca( |$)");
    let cmd = Proc::new(["pgrep", "-f", pat.as_str()]).stdin(In::Null).stdout(Out::Null).stderr(Out::Null).timeout(5.0);
    cmd.run().is_ok_and(|d| d.rc == 0)
}

/// 问一次端口池的实况（`cca _gui live`，只读）；没有 CCA_SELF（不是 cca 起的）、查不成、超时是 None
fn run_live() -> Option<Dict> {
    let me = getenv("CCA_SELF").filter(|s| !s.is_empty())?;
    let cmd = Proc::new([me.as_str(), "_gui", "live"]).stdin(In::Null).stdout(Out::Capture).stderr(Out::Null).timeout(LIVE_WAIT);
    let done = cmd.run().ok().filter(|d| d.rc == 0)?;
    json::loads(&done.out()).ok()?.as_dict().filter(|d| d.str("kind").as_deref() == Some("live"))
}

/// 端口池的实况到了该查的时候（LIVE_EVERY）查一次，记进 remote/status.json；和记着的不一样时是 true（再放一轮）。只在有手机看得到
/// 这台电脑的账号、`watched` 的时候查；`CCA_STATUS_LIVE=off` 不查（回归：它在后台随时会经 SSH 问路由器）
fn live_refresh(now: f64) -> bool {
    if getenv("CCA_STATUS_LIVE").as_deref() == Some("off") || !peers::load().iter().any(|p| p.web && p.incoming != Incoming::None) {
        return false;
    }
    let state = load_state();
    let old = state.dict("live");
    let had = old.as_ref().and_then(live_saved).unwrap_or_default();
    let every = if had == Live::default() { LIVE_EVERY } else { LIVE_RECHECK };
    if old.as_ref().and_then(|l| number(l, "at")).is_some_and(|at| (0.0..every).contains(&(now - at))) || !watched() {
        return false;
    }
    let got = run_live().map(|d| live_of(&d));
    let rec = dict! { "at" => now };
    if let Some(l) = &got {
        let ports = Dict::new();
        for (tag, st) in &l.ports {
            ports.set(tag.as_str(), st.as_str());
        }
        rec.set("ports", ports);
        rec.set("core_down", l.core_down);
    }
    let changed = had != got.unwrap_or_default();
    state.set("live", rec);
    let saved = local::ensure_dir().is_ok() && fs::save_json(&state_file(), &V::Dict(state), Some(0o600), true).is_ok();
    saved && changed
}

/// 电脑上的 cca 有没有新版本（Mac App 页顶的「cca 有新版本 X」「cca 有新的测试版 X」）：照最近一次 `cca update --check` 记下的那一份
/// （gui.rs 的 `update_check_file`：App 一天查一次，这里不联网）。开发模式（cca 旁边有 .git：用 git 更新）不说，和 App 一样
fn update_of(now: f64) -> Option<String> {
    let me = getenv("CCA_SELF").filter(|s| !s.is_empty())?;
    if path::exists(&path::join(&path::dirname(&me), ".git")) {
        return None;
    }
    let doc = fs::load_quiet(&update_check_file()).and_then(|v| v.as_dict())?;
    update_in(&doc, &getenv("CCA_VERSION")?, now)
}

// --- App 后台刷新用量的结果（remote-usage-poll）-----------------------------------------------------------------------------------

/// 记着的：`{"v": 1, "tools": {工具: {"consent": App 的进程号, "failed": App 的进程号}}}`（remote/usage-poll.json）。值是记下它的那个
/// App 的进程号：Mac App 记在内存里，退出了就没了，这里照它（进程不在了就不算）
fn poll_file() -> String {
    path::join(&local::dir(), "usage-poll.json")
}

/// 一次 `cca usage` 结束以后要怎么改记着的
#[derive(Debug, PartialEq)]
enum PollEnd {
    /// 成功：两样都清掉
    Clear,
    /// App 里手动刷新的失败：弹框说过了，清掉「没查成」
    ClearFailed,
    /// 后台刷新要同意（consent）、没查成（failed）：记下
    Set(&'static str),
    /// 不动：取消的（被信号停掉、`cancelled`）、要到终端里做的（`terminal`）、手动刷新时问同意的那一轮（App 接着弹框问）
    Keep,
}

/// 和 Mac App 的 `AppModel.finishUsage`（PROTOCOL 4.3 的重跑流程认的那几样）一个口径。events 是这次的事件文件
fn poll_end(rc: i64, background: bool, events: &str) -> PollEnd {
    if rc == 0 {
        return PollEnd::Clear;
    }
    if rc >= 128 {
        return PollEnd::Keep;
    }
    let evs: Vec<Dict> = events.lines().filter_map(|l| json::loads(l).ok().and_then(|v| v.as_dict())).collect();
    let has = |k: &str| evs.iter().any(|e| e.str("ev").as_deref() == Some(k));
    if has("consent") {
        return if background { PollEnd::Set("consent") } else { PollEnd::Keep };
    }
    let last_fail = evs.iter().rev().find(|e| e.str("ev").as_deref() == Some("fail")).and_then(|e| e.str("code"));
    if has("terminal") || last_fail.as_deref() == Some("cancelled") {
        return PollEnd::Keep;
    }
    if background {
        PollEnd::Set("failed")
    } else {
        PollEnd::ClearFailed
    }
}

/// `remote-usage-poll 退出码 [App 的进程号] [事件文件]`（bash cmd_usage 的 EXIT trap：App 起的 `cca usage` 才调，事件文件是 CCA_EVENTS，
/// 后台刷新时 App 带着自己的进程号 CCA_USAGE_POLL）：记下 App 刷新用量的结果，给手机上那台电脑的一栏（host_of 的 extra：「刷新 X 的
/// 用量需要你的同意」「后台刷新 X 的用量失败」，poll_problems）。工具是 CCA_TOOL。变了就放一轮。出什么错都安静地退出
pub fn cmd_remote_usage_poll(args: &[String]) -> R<()> {
    arity("cmd_remote_usage_poll", args, 1, 3)?;
    if *crate::core::base::RO {
        return Ok(());
    }
    let rc = args[0].trim().parse::<i64>().unwrap_or(1);
    let pid = crate::cmd::arg(args, 1, "").trim().parse::<i64>().ok().filter(|p| *p > 1);
    let events =
        Some(crate::cmd::arg(args, 2, "")).filter(|p| !p.is_empty()).and_then(|p| fs::read_text(p).ok()).unwrap_or_default();
    let end = poll_end(rc, pid.is_some(), &events);
    if end == PollEnd::Keep {
        return Ok(());
    }
    let Ok(d) = local::ensure_dir() else { return Ok(()) };
    let Ok(lock) = std::fs::OpenOptions::new().append(true).create(true).open(path::join(&d, "usage-poll.lock")) else {
        return Ok(());
    };
    if !flock(&lock, libc::LOCK_EX) {
        return Ok(());
    }
    let file = poll_file();
    let doc = fs::load_quiet(&file).and_then(|v| v.as_dict()).filter(|d| d.get("v").and_then(|v| v.as_int()) == Some(1));
    let doc = doc.unwrap_or_else(|| dict! { "v" => 1 });
    let tools = doc.dict("tools").unwrap_or_default();
    let tool = crate::core::tool::t().name.to_string();
    let rec = tools.dict(&tool).unwrap_or_default();
    let before = json::dumps(&V::Dict(rec.clone()), json::LINE);
    match end {
        PollEnd::Clear => {
            rec.pop("consent");
            rec.pop("failed");
        }
        PollEnd::ClearFailed => {
            rec.pop("failed");
        }
        PollEnd::Set(k) => rec.set(k, pid.unwrap_or(0)),
        PollEnd::Keep => {}
    }
    if json::dumps(&V::Dict(rec.clone()), json::LINE) == before {
        return Ok(());
    }
    if rec.is_empty() {
        tools.pop(&tool);
    } else {
        tools.set(tool.as_str(), rec);
    }
    doc.set("tools", tools);
    if fs::save_json(&file, &V::Dict(doc), Some(0o600), true).is_ok() {
        status_kick();
    }
    Ok(())
}

/// 进程还在（App 没退出）
fn alive(pid: i64) -> bool {
    pid > 1 && i32::try_from(pid).is_ok_and(|p| unsafe { libc::kill(p, 0) } == 0)
}

/// 记着的结果里还算数的（记下它的 App 还开着）：照 Mac App 页顶的先后，每个工具一条，要同意的在前（`(原因码, 工具)`）
fn poll_problems() -> Vec<(String, String)> {
    let Some(tools) = fs::load_quiet(&poll_file()).and_then(|v| v.as_dict()).and_then(|d| d.dict("tools")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for tool in crate::core::tool::TOOLS.iter().map(|t| t.name) {
        let Some(rec) = tools.dict(tool) else { continue };
        let on = |k: &str| rec.get(k).and_then(|v| v.as_int()).is_some_and(alive);
        if on("consent") {
            out.push(("usage_consent".to_string(), tool.to_string()));
        } else if on("failed") {
            out.push(("usage_failed".to_string(), tool.to_string()));
        }
    }
    out
}

/// 记下的那一份里的最新版本：比现在的新、查了不到 UPDATE_FRESH 才算
fn update_in(doc: &Dict, current: &str, now: f64) -> Option<String> {
    let at = number(doc, "generated_epoch")?;
    let latest = text(doc, "latest", 32);
    let newer = matches!((ver_key(&latest), ver_key(current)), (Some(new), Some(cur)) if new > cur);
    ((0.0..UPDATE_FRESH).contains(&(now - at)) && newer).then_some(latest)
}

/// 头像的颜色（启动器的颜色，`#RRGGBB`）
fn color_ok(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// 出口检测的结果（PROTOCOL 3.3 的 `health`）里手机要的：`result`、不可用的地区（`region`）、纯净度（`trust`、`trust_label`）、
/// DNS / WebRTC 的泄露检测（`dns`、`webrtc`）、过时了（`stale`：超过 6 小时）、不是现在的节点测的（`applies` 是 false）、什么时候测的（`at`）、
/// 是启动时测的（`source` 是 launch）。从没测过是空的 `{}`（手机照它说「尚未检测出口」）。不给出口 IP、运营商、节点、说明（`message` 里有 IP）
fn net_of(h: Option<Dict>) -> Dict {
    let d = Dict::new();
    let Some(h) = h else { return d };
    d.set("result", text(&h, "result", 24));
    let region = text(&h, "restricted_region", 32);
    if !region.is_empty() {
        d.set("region", region);
    }
    if let V::Int(t) = count(&h, "trust") {
        d.set("trust", t.min(100));
        let label = text(&h, "trust_label", 16);
        if !label.is_empty() {
            d.set("trust_label", label);
        }
    }
    if let Some(l) = h.dict("leak") {
        for k in ["dns", "webrtc"] {
            let s = text(&l, k, 16);
            if !s.is_empty() {
                d.set(k, s);
            }
        }
    }
    if flag(&h, "stale") {
        d.set("stale", true);
    }
    if h.get("applies").and_then(|v| v.as_bool()) == Some(false) {
        d.set("applies", false);
    }
    if let V::Int(t) = epoch(&h, "checked_at") {
        d.set("at", t);
    }
    if h.str("source").as_deref() == Some("launch") {
        d.set("source", "launch");
    }
    d
}

/// 为什么用不了（PROTOCOL 2.2 的 `can_why`）里手机要的：桌面 App（`app`：`app_missing` 是没装）、在终端里打开（`cli`：`tool_missing`
/// 是没装 claude / codex / agy）；都能用时是 None
fn why_of(a: &Dict) -> Option<Dict> {
    let w = a.dict("can_why")?;
    let d = Dict::new();
    for k in ["app", "cli"] {
        let s = text(&w, k, 24);
        if !s.is_empty() {
            d.set(k, s);
        }
    }
    (!d.is_empty()).then_some(d)
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
    // Antigravity 的每周用量分组（usage_weeks_groups）：有才写
    if let Some(l) = a.list("usage_weeks_groups") {
        d.set("weeks_groups", groups_of(&l));
    }
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
    // 出口检测的结果（总览的「有问题」、账号页那句话：和 Mac App 的 Diagnosis 一个口径）、为什么用不了、第三方 API 的模型
    d.set("net", net_of(a.dict("health")));
    if let Some(why) = why_of(a) {
        d.set("why", why);
    }
    let model = a.dict("api").map(|x| text(&x, "model", 64)).unwrap_or_default();
    if !model.is_empty() {
        d.set("model", model);
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
            let row = row_of(&a, &id, &name);
            // 是哪个 Team 给的（手机按 Team 筛）
            if let Some(t) = a.str("team").and_then(|t| team_name(snap, &t)) {
                row.set("team", t);
            }
            out.push(Acct {
                tool: id.to_string(),
                label: label.clone(),
                ident: ident.to_string(),
                key: key.to_string(),
                hub: relay::session_hub(&id, &key),
                row,
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

/// 给一个配对的那一份里不是哪个账号的：这个 Team 的盐（算 aid）、机器名、这台电脑的折合金额那几样（cost_meta）、这个配对所在的 Team（team_of）、
/// 这台电脑的版本和开关、提示（host_of）、这台手机在这台电脑上能做什么（`control` 看和操作、`view` 只看：peers.json 的 `incoming`）
struct Ctx<'a> {
    salt: Option<&'a str>,
    machine: &'a str,
    cost: Option<&'a Dict>,
    team: Option<&'a Dict>,
    host: Option<&'a Dict>,
    allow: &'a str,
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
            row.pop("weeks_groups");
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
    if let Some(h) = cx.host {
        d.update(h);
    }
    if !cx.allow.is_empty() {
        d.set("allow", cx.allow);
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
        let remote =
            fs::load_quiet(&crate::core::base::CFG).and_then(|c| c.as_dict()).is_some_and(|c| super::manage::remote_on(&c));
        // Mac App 看得到的那几样：App 开着才给（watched）。端口的问题照记着的实况（查在放完以后，live_refresh）
        let watched = snap.is_some() && watched();
        let live = if watched { live_now(&state, now) } else { Live::default() };
        for a in &accts {
            let tag = live_tag(&a.tool, &a.key);
            if let Some((_, st)) = live.ports.iter().find(|(t, _)| *t == tag) {
                a.row.set("port", st.as_str());
            }
        }
        let mut extra = if watched { poll_problems() } else { Vec::new() };
        if live.core_down {
            extra.push(("core_down".to_string(), String::new()));
        }
        let update = update_of(now);
        let host = snap.as_ref().map(|s| host_of(s, remote, &extra, update.as_deref()));
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
            let mine: Vec<&Acct> = accts.iter().filter(|a| a.hub == Some(hub)).collect();
            for peer in phones.iter().filter(|p| p.hub == hub) {
                let Some(secret) = peers::secret(&peer.pid) else { continue };
                if peer.incoming == Incoming::None {
                    if retract(&srv, peer, &secret, &machine, now)? {
                        pids.pop(&peer.pid);
                    }
                    continue;
                }
                let cx = Ctx {
                    salt: salt.as_deref(),
                    machine: &machine,
                    cost: cost.as_ref(),
                    team: team.as_ref(),
                    host: host.as_ref(),
                    allow: peer.incoming.word(),
                };
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
            // 端口池的实况该查了：先放上面这一轮（问路由器要几秒），查完和记着的不一样再放一轮
            if live_refresh(time()) {
                let _ = touch();
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
        Ctx { salt, machine: "box", cost: None, team: None, host: None, allow: "" }
    }

    fn full(n: usize) -> Fit {
        Fit { weeks: true, cost: CostFit::Full, n }
    }

    fn accts_of(list: &[Dict]) -> Vec<Acct> {
        let tool = dict! { "id" => "claude", "label" => "Claude Code", "accounts" => V::List(List::from_vec(list.iter().map(|d| V::Dict(d.clone())).collect())) };
        accounts(&dict! { "tools" => V::List(List::from_vec(vec![V::Dict(tool)])) }, NOW)
    }

    #[test]
    fn group_weeks_for_the_phone() {
        // Antigravity 的每周用量分组（usage_weeks_groups → weeks_groups）：组的 id、名字和 weeks_of 的几项（不带 last_epoch）；
        // 不是对象、没有 id 的不要；没有的账号不写；太长、不带每周用量时一起不带
        let week = V::Dict(dict! {
            "start_epoch" => 1.0, "end_epoch" => 2.0, "used_percent" => 40, "resets" => 0, "last_epoch" => 2.0, "current" => true, "partial" => false,
        });
        let group = |id: &str, name: &str| {
            V::Dict(dict! {
                "id" => id, "name" => name, "since_epoch" => 1.0, "weeks" => V::List(List::from_vec(vec![week.clone()])),
                "summary" => dict! { "weeks" => 0, "average_percent" => V::None, "maxed" => 0, "under_half" => 0 },
            })
        };
        let a = acct(w(10.0, Some(NOW + 60.0)), w(20.0, Some(NOW + 86400.0)));
        a.set(
            "usage_weeks_groups",
            V::List(List::from_vec(vec![
                group("gemini-weekly", "Gemini Models"),
                V::from("bad"),
                group("", "no id"),
                group("3p-weekly", "Claude and GPT models\u{202e}"),
            ])),
        );
        let accts = accts_of(&[a, acct(w(10.0, None), w(20.0, None))]);
        let refs: Vec<&Acct> = accts.iter().collect();
        let doc = doc_for(&P, &refs, &cx(None), full(2));
        let rows = doc.list("accounts").unwrap();
        let row = rows.get(0).unwrap().as_dict().unwrap();
        let g = row.list("weeks_groups").unwrap();
        let ids: Vec<String> = g.items().iter().map(|x| x.as_dict().unwrap().str("id").unwrap().to_string()).collect();
        assert_eq!(ids, ["gemini-weekly", "3p-weekly"]);
        let g0 = g.get(0).unwrap().as_dict().unwrap();
        assert_eq!(g0.str("name").as_deref(), Some("Gemini Models"));
        assert!(g0.get("since_epoch").is_some() && g0.dict("summary").is_some());
        let wk = g0.list("weeks").unwrap().get(0).unwrap().as_dict().unwrap();
        assert!(wk.get("last_epoch").is_none() && wk.get("used_percent").is_some());
        assert!(!g.get(1).unwrap().as_dict().unwrap().str("name").unwrap().contains('\u{202e}'), "名字照别的字段一样清理");
        assert!(rows.get(1).unwrap().as_dict().unwrap().get("weeks_groups").is_none(), "没有的账号不写");
        let lean = doc_for(&P, &refs, &cx(None), Fit { weeks: false, cost: CostFit::Off, n: 2 });
        let lrow = lean.list("accounts").unwrap().get(0).unwrap().as_dict().unwrap();
        assert!(lrow.get("weeks_groups").is_none() && lrow.get("weeks").is_some_and(|w| w.is_none()));
        assert!(accts[0].row.list("weeks_groups").is_some(), "少放的是给这一个配对的那一份，不改账号本身");
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
        let c = Ctx { salt: None, machine: "box", cost: Some(&meta), team: Some(&t), host: None, allow: "" };
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
        let c = Ctx { salt: None, machine: "box", cost: Some(&meta), team: None, host: None, allow: "" };
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

    #[test]
    fn health_why_and_team_for_the_phone() {
        // 出口检测（health → net：不给出口 IP、运营商、说明；从没测过是 {}）、can_why（只给 app、cli）、第三方 API 的模型（不给接口的地址）、
        // 套餐过时、是哪个 Team 给的（不在 config.teams 里的 Team 不写）
        let l = |xs: Vec<V>| V::List(List::from_vec(xs));
        let json_of = |d: &Dict| json::dumps(&V::Dict(d.clone()), json::Fmt::new().utf8().compact());
        let tid = "ab".repeat(16);
        let a = acct(w(10.0, Some(NOW + 60.0)), V::None);
        a.set(
            "health",
            dict! {
                "version" => 1, "checked_at" => NOW - 100.5, "age_seconds" => 100, "stale" => false, "applies" => true, "source" => "check",
                "result" => "restricted", "ip" => "203.0.113.1", "country" => "XX", "org" => "Example Networks", "trust" => 40,
                "restricted_region" => "某地\u{202e}", "message" => "203.0.113.1 · XX · Example Networks",
                "leak" => dict! { "dns" => "leak", "webrtc" => "note", "line" => "DNS 203.0.113.9", "dns_message" => "203.0.113.9" },
            },
        );
        a.set("can_why", dict! { "app" => "app_missing", "web" => "no_chrome" });
        a.set("team", tid.as_str());
        a.dict("usage").unwrap().set("outdated_login_plan", "Pro");
        let api = acct(V::None, V::None);
        api.set("kind", "api");
        api.set("login", "api");
        api.set("usage", V::None);
        api.set("api", dict! { "host" => "api.example.com", "model" => "model-x" });
        api.set(
            "health",
            dict! {
                "result" => "api_ok", "stale" => true, "applies" => false, "leak" => V::None, "checked_at" => V::None, "source" => "launch",
                "trust" => V::None, "trust_label" => "",
            },
        );
        api.set("team", "ef".repeat(16));
        let fresh = acct(V::None, V::None);
        fresh.set("can_why", Dict::new());
        let tool =
            dict! { "id" => "claude", "label" => "Claude Code", "accounts" => l(vec![V::Dict(a), V::Dict(api), V::Dict(fresh)]) };
        let team = dict! { "id" => tid.as_str(), "name" => "示例 Team", "role" => "member" };
        let snap = dict! { "tools" => l(vec![V::Dict(tool)]), "config" => dict! { "teams" => l(vec![V::Dict(team)]) } };
        let accts = accounts(&snap, NOW);
        let refs: Vec<&Acct> = accts.iter().collect();
        let doc = doc_for(&P, &refs, &cx(None), full(3));
        let text = json_of(&doc);
        for no in ["203.0.113", "Example Networks", "example.com", "no_chrome", "\u{202e}"] {
            assert!(!text.contains(no), "{no}：{text}");
        }
        let rows = doc.list("accounts").unwrap();
        let row = |i: usize| rows.get(i).unwrap().as_dict().unwrap();
        assert_eq!(
            json_of(&row(0).dict("net").unwrap()),
            format!(
                r#"{{"result":"restricted","region":"某地","trust":40,"dns":"leak","webrtc":"note","at":{}}}"#,
                (NOW - 101.0) as i64
            )
        );
        assert_eq!(json_of(&row(0).dict("why").unwrap()), r#"{"app":"app_missing"}"#);
        assert_eq!(row(0).str("team").as_deref(), Some("示例 Team"));
        assert_eq!(row(0).dict("usage").unwrap().str("outdated_login_plan").as_deref(), Some("Pro"));
        assert!(row(0).get("model").is_none());
        assert_eq!(json_of(&row(1).dict("net").unwrap()), r#"{"result":"api_ok","stale":true,"applies":false,"source":"launch"}"#);
        assert_eq!(row(1).str("model").as_deref(), Some("model-x"));
        assert!(row(1).get("team").is_none(), "这台设备已经不在那个 Team 里了");
        assert_eq!(json_of(&row(2).dict("net").unwrap()), "{}", "从没测过");
        assert!(row(2).get("why").is_none() && row(2).dict("usage").unwrap().get("outdated_login_plan").is_none());
        // 这台电脑的几样：版本、开关、页顶的提示（只给原因码、工具、共享设置里的项；同样的只给一次，不给说明）
        let item = |code: &str, msg: &str| V::Dict(dict! { "code" => code, "message" => msg });
        let snap2 = dict! {
            "cca_version" => "3.8.0-beta.2",
            "errors" => l(vec![
                V::Dict(dict! { "code" => "account_unreadable", "tool" => "codex", "message" => "/Users/x/.cca/accounts/alice@example.com" }),
                V::Dict(dict! { "code" => "account_unreadable", "tool" => "codex", "message" => "又一个" }),
                V::Dict(dict! { "code" => "config_unreadable", "tool" => V::None, "message" => "/Users/x/.cca/config.json" }),
            ]),
            "tools" => l(vec![
                V::Dict(dict! {
                    "id" => "claude", "errors" => l(vec![item("table_invalid", "alice@example.com 的节点 US-Z 不存在"), item("table_invalid", "…")]),
                    "settings_problems" => l(vec![V::from("settings.json:ANTHROPIC_BASE_URL"), V::from(""), V::from("settings.local.json:env.HTTPS_PROXY")]),
                }),
                V::Dict(dict! { "id" => "codex", "errors" => l(vec![]), "settings_problems" => l(vec![]) }),
            ]),
        };
        // 照 Mac App 页顶的先后：账号表、共享设置（各个工具的），文件读不了，后台查用量的，内置代理停了；有新版本时带着版本号
        let extra = [("usage_consent".to_string(), "codex".to_string()), ("core_down".to_string(), String::new())];
        let h = host_of(&snap2, true, &extra, Some("3.8.1"));
        assert_eq!(
            json_of(&h),
            concat!(
                r#"{"cca":"3.8.0-beta.2","remote":true,"update":"3.8.1","problems":[{"code":"table_invalid","tool":"claude"},"#,
                r#"{"code":"settings_problems","tool":"claude","items":["settings.json:ANTHROPIC_BASE_URL","settings.local.json:env.HTTPS_PROXY"]},"#,
                r#"{"code":"account_unreadable","tool":"codex"},{"code":"config_unreadable"},{"code":"usage_consent","tool":"codex"},"#,
                r#"{"code":"core_down"}]}"#
            )
        );
        assert_eq!(json_of(&host_of(&Dict::new(), false, &[], None)), r#"{"remote":false}"#, "没有版本、提示时只有开关");
        let c = Ctx { salt: None, machine: "box", cost: None, team: None, host: Some(&h), allow: "view" };
        let doc = doc_for(&P, &refs, &c, full(3));
        assert_eq!((doc.str("cca").as_deref(), doc.get("remote").unwrap().as_bool()), (Some("3.8.0-beta.2"), Some(true)));
        assert_eq!((doc.str("allow").as_deref(), doc.list("problems").unwrap().len()), (Some("view"), 6));
        assert_eq!(doc.str("update").as_deref(), Some("3.8.1"));
        let lean = doc_for(&P, &refs, &c, Fit { weeks: false, cost: CostFit::Off, n: 0 });
        assert!(lean.list("problems").is_some() && lean.str("allow").is_some() && lean.str("cca").is_some(), "少放账号时照样带着");
        assert!(doc_for(&P, &refs, &cx(None), full(3)).get("allow").is_none(), "不知道（测试里）时不写");
    }

    #[test]
    fn live_ports_and_core_for_the_phone() {
        // _gui live 的 slots：只要 closed、drift，按工具和账号表的键的摘要认（status.json 里不带邮箱）；内置代理：走它、配置了、没在运行才算停了
        let l = |xs: Vec<V>| V::List(List::from_vec(xs));
        let slot = |tool: &str, key: &str, st: &str| V::Dict(dict! { "tool" => tool, "key" => key, "state" => st, "port" => 7001 });
        let doc = dict! {
            "kind" => "live", "route" => "local",
            "slots" => l(vec![
                slot("claude", "工作(alice@example.com)", "closed"), slot("codex", "bob@example.com", "drift"),
                slot("claude", "carol@example.com", "ok"), V::from("坏的"),
            ]),
            "local" => dict! { "configured" => true, "running" => false },
        };
        let live = live_of(&doc);
        let want = vec![
            (live_tag("claude", "工作(alice@example.com)"), "closed".to_string()),
            (live_tag("codex", "bob@example.com"), "drift".to_string()),
        ];
        assert_eq!((&live.ports, live.core_down), (&want, true));
        assert_ne!(live_tag("claude", "bob@example.com"), live_tag("codex", "bob@example.com"), "两个工具里同一个邮箱是两个账号");
        assert_eq!(live_tag("claude", "bob@example.com").len(), 16);
        for (route, local) in [
            ("router", dict! { "configured" => true, "running" => false }),
            ("local", dict! { "configured" => false, "running" => false }),
            ("local", dict! { "configured" => true, "running" => true }),
            ("local", dict! { "configured" => true, "running" => V::None }),
        ] {
            let d = dict! { "route" => route, "local" => local.clone() };
            assert!(!live_of(&d).core_down, "{route} {}", json::dumps(&V::Dict(local), json::LINE));
        }
        // 记进 status.json 的读回来一样；太旧的、没查成的（只有 at）不用
        let ports = Dict::new();
        for (t, s) in &want {
            ports.set(t.as_str(), s.as_str());
        }
        let rec = dict! { "at" => NOW - 60.0, "ports" => ports, "core_down" => true };
        assert_eq!(live_saved(&rec), Some(Live { ports: want.clone(), core_down: true }));
        let st = dict! { "v" => 1, "live" => rec };
        assert_eq!(live_now(&st, NOW), Live { ports: want, core_down: true });
        assert_eq!(live_now(&st, NOW - 60.0 + LIVE_STALE), Live::default(), "太旧了");
        assert_eq!(live_now(&dict! { "live" => dict! { "at" => NOW } }, NOW), Live::default(), "没查成的只有 at");
    }

    #[test]
    fn update_from_the_last_check() {
        let doc =
            |latest: &str, at: f64| dict! { "kind" => "update", "latest" => latest, "generated_epoch" => at, "available" => true };
        assert_eq!(update_in(&doc("3.8.1", NOW), "3.8.0", NOW).as_deref(), Some("3.8.1"));
        assert_eq!(
            update_in(&doc("3.8.0-beta.3", NOW), "3.8.0-beta.2", NOW).as_deref(),
            Some("3.8.0-beta.3"),
            "测试版上的更新的测试版"
        );
        assert_eq!(update_in(&doc("3.8.0", NOW), "3.8.0-beta.2", NOW).as_deref(), Some("3.8.0"), "同号的正式版比测试版新");
        assert_eq!(update_in(&doc("3.8.0", NOW), "3.8.0", NOW), None, "已经换成它了（App 还没再查）");
        assert_eq!(update_in(&doc("3.7.13", NOW), "3.8.0-beta.2", NOW), None, "比现在的旧");
        assert_eq!(update_in(&doc("3.8.1", NOW - UPDATE_FRESH), "3.8.0", NOW), None, "查得太久以前了");
        assert_eq!(update_in(&doc("坏的", NOW), "3.8.0", NOW), None);
        assert_eq!(update_in(&dict! { "latest" => "3.8.1" }, "3.8.0", NOW), None, "没有时间");
    }

    #[test]
    fn usage_poll_like_the_mac_app() {
        // 和 AppModel.finishUsage 一个口径：成功清掉；后台的要同意、没查成记下；手动的失败清掉「没查成」；取消的、要到终端里做的不动
        let consent = r#"{"v":1,"ev":"consent","item":"router_reload","scope":"openclash","grant":"router_reload:openclash"}"#;
        let fail = |code: &str| format!(r#"{{"v":1,"ev":"fail","code":"{code}","message":"x"}}"#);
        assert_eq!(poll_end(0, true, consent), PollEnd::Clear);
        assert_eq!(poll_end(0, false, ""), PollEnd::Clear, "App 里手动刷新成了也清掉");
        assert_eq!(poll_end(1, true, consent), PollEnd::Set("consent"));
        assert_eq!(poll_end(1, false, consent), PollEnd::Keep, "手动的：App 接着弹框问");
        assert_eq!(poll_end(1, true, &fail("port_closed")), PollEnd::Set("failed"));
        assert_eq!(poll_end(1, false, &fail("port_closed")), PollEnd::ClearFailed, "手动的失败弹框说过了");
        assert_eq!(poll_end(1, true, &format!("{}\n{}", fail("port_closed"), fail("cancelled"))), PollEnd::Keep, "取消的");
        assert_eq!(poll_end(1, true, r#"{"v":1,"ev":"terminal","code":"login","argv":[]}"#), PollEnd::Keep, "要到终端里做的");
        assert_eq!(poll_end(130, true, ""), PollEnd::Keep, "被信号停掉的");
        assert_eq!(poll_end(1, true, "不是 JSON\n"), PollEnd::Set("failed"));
        assert!(alive(i64::from(std::process::id())) && !alive(0) && !alive(1));
    }
}
