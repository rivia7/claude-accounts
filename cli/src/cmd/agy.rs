//! Antigravity（agy）的额度（`cca agy usage`）：agy 1.1.11 起，打印模式里的 /usage 不经模型、直接回额度：
//! `agy --gemini_dir=账号目录 -p /usage --output-format json`，回的 JSON 里 `command.data` 是 RetrieveUserQuotaSummary
//! 的结果（1.5.16 的 printmode.quotaJSON：`{description, groups: [{name, description, buckets: [{id, name, description, window,
//! disabled, remaining_fraction, remaining_amount, reset_time}]}]}`，reset_time 是 UTC 的 RFC 3339）。登录 token 由 agy 自己读取
//! 和刷新，cca 不碰。这里有查询（agy 账号的 usage-query 走到这里）、`--json` 里的 quota_groups（macos/PROTOCOL.md 3.1）、表格和菜单里
//! 那一格。查到的原样存进 usage.json（usage_save），显示的时候再算用了多少、过没过重置时间。
//!
//! 套餐也在这里：agy 1.5 的登录 token 里没有档位了，只有交互会话的状态栏 JSON 里有（plan_tier）。cca 启动交互的会话时经
//! `--settings` 带上自己的状态栏命令（`agy_launch_args`），它（agy-statusline）把套餐记进账号目录（`agy_plan_seen` 读），
//! 用户自己的状态栏命令接在后面照样运行。Team 给的账号的套餐还跟着折合金额的同步传给 Team 里别的设备（`agy_plan_own`、
//! `agy_plan_adopt`，cost.rs 的 cost_round）：没在那台设备上开过会话也看得到。状态栏每次状态变了 agy 都起一次这个命令：
//! 它顺带看出这一轮做完了、在等你确认，推一条通知（`agy_turn`，和 Claude Code、Codex 的一样：在会话宿主里时推给接着它的各处，
//! 这台 Mac 上弹一个）。

use super::login::sys::{interrupted, trap};
use super::remote::{agy_note, host_of_me, notify_deliver, notify_deliver_to, AgyEvent};
use super::settings::AGY_VALUES;
use super::usage::{ago, cols_of, dicts, float_arg, parse_ts, py_round, reset_short, usage_why, Row, WhyTable, AGY_STALE};
use super::{arity, Cmd};
use crate::core::base::{EMAIL_RE, SELF_PATH};
use crate::core::model::{acct_id, adir, cdir};
use crate::core::tool::{agy, AGY_FILE_ENV};
use crate::py::fs::{load_quiet, save_json};
use crate::py::json;
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::proc::{self, Cmd as Proc, In, Out, ProcError};
use crate::py::rand::token_hex;
use crate::py::shutil::TemporaryDirectory;
use crate::py::text::{self, fit, sh_quote};
use crate::py::time::{monotonic, time};
use crate::py::uni;
use crate::py::{die, exit, R};
use crate::{dict, out, re};
use std::io::Read;
use std::os::fd::{AsRawFd, RawFd};

/// 这个模块的子命令
pub const CMDS: &[Cmd] = &[Cmd::all("agy-statusline", cmd_agy_statusline), Cmd::all("agy-turn-wait", cmd_agy_turn_wait)];

/// 没登录、登录失效（刷新不了）时 agy 在 stderr 上打出 Google 的登录网址（OAuth 授权的那一个），然后等 60 秒（没有终端也等，
/// 1.5.16 实测）：看到它就停下，说登录失效。只认授权网址的开头：额度的说明里带个 Google 账号的链接不算
const AGY_LOGIN_SAID: &str = "https://accounts.google.com/o/oauth2/";
/// agy 退出以后 stdout、stderr 还有人开着（它起的进程拿着）时最多再读这么久
const AGY_DRAIN: f64 = 1.0;

/// 在账号的环境里（bash 的 child_env 设好了出口和 CCA_ACCOUNT）无界面地问 agy 的额度。stdout 打印一行 JSON：
/// {ok: true, usage: {rate_limits_available: true, rate_limits: 额度}}（额度是 agy 给的 command.data 原样），
/// 或 {ok: false, why: auth|unsupported|timeout|error, msg: 说明}
pub fn agy_usage_query(bin_: &str, timeout: &str) -> R<()> {
    let Some(key) = proc::getenv("CCA_ACCOUNT").filter(|k| !k.is_empty()) else {
        return die("cca(py): 没有账号");
    };
    let secs = float_arg(timeout)?;
    let said = agy_ask(bin_, &key, secs)?;
    out!("{}", json::dumps(&V::Dict(said), json::LINE));
    Ok(())
}

fn fail(why: &str, msg: &str) -> Dict {
    dict! { "ok" => false, "why" => why, "msg" => text::head(text::strip(msg), 200) }
}

/// 停掉 agy 和它起的进程（新会话里的整个进程组）：先 TERM，一秒内没退出再 KILL
fn stop_group(child: &mut std::process::Child) {
    let pid = child.id() as libc::pid_t;
    // SAFETY: 发给自己起的、自成一组（setsid）的进程组
    unsafe { libc::kill(-pid, libc::SIGTERM) };
    let end = monotonic() + 1.0;
    while monotonic() < end {
        if matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
            break;
        }
        crate::py::time::sleep(0.02);
    }
    // SAFETY: 同上；组里已经没有进程时只是返回 ESRCH
    unsafe { libc::kill(-pid, libc::SIGKILL) };
    let _ = child.wait();
}

/// 读一次能读的（poll 说有东西时才来）：读到头是 false
fn read_some(fd: RawFd, into: &mut Vec<u8>) -> bool {
    let mut buf = [0u8; 65536];
    loop {
        // SAFETY: buf 是有效的可写内存，长度如实给出
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n > 0 {
            into.extend_from_slice(&buf[..n as usize]);
            return true;
        }
        if n == 0 {
            return false;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted || interrupted() {
            return false;
        }
    }
}

/// 起 agy 问额度：agy 在新的会话里（setsid）跑，没有控制终端——要授权码时不会去读用户在终端里敲的字；
/// stdin 是空的；工作目录是一个空的临时目录（用完删掉）：agy 把工作目录当成项目，别让它去看账号目录（里面有浏览器的资料）。
/// 看到登录网址、到点、cca 被叫停（Ctrl-C、cca usage 停后台查询的 SIGTERM）时整组停掉
fn agy_ask(bin_: &str, key: &str, secs: f64) -> R<Dict> {
    let mut env = proc::environ();
    for (k, v) in AGY_FILE_ENV {
        proc::env_set(&mut env, k, v); // 不管 bash 那边设没设：查用量的 agy 绝不能读钥匙串里那一份（别的账号的登录）
    }
    // 查一下额度不该顺带换掉装好的 agy（它平时自己在后台更新；cca 启动 agy 时照旧）
    proc::env_set(&mut env, "AGY_CLI_DISABLE_AUTO_UPDATE", "true");
    let gemini = format!("--gemini_dir={}", cdir(key));
    let work = match TemporaryDirectory::new("cca-agy-usage.", &crate::py::fs::gettempdir()) {
        Ok(work) => work,
        Err(e) => return Ok(fail("error", &e.to_string())),
    };
    let cmd = Proc::new([bin_, gemini.as_str(), "-p", "/usage", "--output-format", "json"])
        .stdin(In::Null)
        .stdout(Out::Capture)
        .stderr(Out::Capture)
        .env(env)
        .cwd(work.name.as_str())
        .new_session();
    trap(libc::SIGTERM, false);
    trap(libc::SIGINT, true);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(ProcError::Os(e)) => return Ok(fail("error", &e.to_string())),
        Err(ProcError::Timeout { .. } | ProcError::Interrupted) => unreachable!("spawn 不限时，也不会被信号打断"),
    };
    let out_pipe = child.stdout.take().expect("stdout 接的是管道");
    let err_pipe = child.stderr.take().expect("stderr 接的是管道");
    let (out_fd, err_fd) = (out_pipe.as_raw_fd(), err_pipe.as_raw_fd());
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let (mut out_open, mut err_open) = (true, true);
    let end = monotonic() + secs;
    let mut exited_at: Option<f64> = None;
    loop {
        if interrupted() {
            stop_group(&mut child);
            return exit(130);
        }
        let mut fds: Vec<libc::pollfd> = [(out_open, out_fd), (err_open, err_fd)]
            .iter()
            .filter(|(open, _)| *open)
            .map(|(_, fd)| libc::pollfd { fd: *fd, events: libc::POLLIN, revents: 0 })
            .collect();
        if fds.is_empty() {
            crate::py::time::sleep(0.05);
        } else {
            // SAFETY: fds 是有效的 pollfd 数组，长度如实给出
            unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 50) };
        }
        for p in &fds {
            if p.revents == 0 {
                continue;
            }
            if p.fd == out_fd {
                out_open = read_some(out_fd, &mut out);
            } else {
                err_open = read_some(err_fd, &mut err);
            }
        }
        if asks_login(&out, &err) {
            stop_group(&mut child);
            return Ok(fail("auth", ""));
        }
        let now = monotonic();
        if exited_at.is_none() && matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
            exited_at = Some(now);
        }
        match exited_at {
            Some(at) if (!out_open && !err_open) || now - at > AGY_DRAIN => break,
            Some(_) => {}
            None if now > end => {
                stop_group(&mut child);
                return Ok(fail("timeout", ""));
            }
            None => {}
        }
    }
    let rc = child.wait().ok().and_then(|s| s.code());
    let said_err = String::from_utf8_lossy(&err).into_owned();
    Ok(agy_answer(&String::from_utf8_lossy(&out), &said_err, rc))
}

/// agy 打出了登录网址、在等人去登录：stderr 上（1.5.16 实测在这里），stdout 上的也算
fn asks_login(out: &[u8], err: &[u8]) -> bool {
    [err, out].iter().any(|b| String::from_utf8_lossy(b).contains(AGY_LOGIN_SAID))
}

/// agy 的 stdout（--output-format json 的那一份）→ 查询的结果。rc 是退出码（被信号结束是 None），err 是它在 stderr 上说的
pub(super) fn agy_answer(said: &str, err: &str, rc: Option<i32>) -> Dict {
    let last_err = text::splitlines(text::strip(err)).last().map(|s| s.to_string()).unwrap_or_default();
    let Some(doc) = json_doc(said) else {
        let msg = if last_err.is_empty() {
            format!("没有回应就退出了（退出码 {}）", rc.map_or_else(|| "None".to_string(), |rc| rc.to_string()))
        } else {
            last_err
        };
        return fail("error", &msg);
    };
    let status = doc.str("status").map(|s| s.to_ascii_uppercase()).unwrap_or_default();
    if status != "SUCCESS" {
        let e = doc.str("error").map(|e| e.to_string()).filter(|e| !text::strip(e).is_empty()).unwrap_or(last_err);
        // 打印模式不认 /usage 的版本：说这个命令在这里用不了
        let old = crate::re!(r"(?i)not (available|supported) in print mode|unknown (slash )?command").found(&e);
        return fail(if old { "unsupported" } else { "error" }, if e.is_empty() { "agy 说查询失败，没给原因" } else { &e });
    }
    // 没有 command：这个版本把 /usage 当成了一句话交给模型（1.1.11 以前），不是额度
    let Some(cmd) = doc.dict("command") else { return fail("unsupported", "") };
    let Some(data) = cmd.dict("data").filter(|d| d.list("groups").is_some()) else {
        return fail("error", "agy 回的额度认不出（格式变了）");
    };
    if !has_buckets(&data) {
        return fail("error", "agy 没给这个账号的额度");
    }
    dict! { "ok" => true, "usage" => dict! { "rate_limits_available" => true, "rate_limits" => data } }
}

/// stdout 里的 JSON 文档：整个是一份（现在是一行），或者最后一份从行首的 { 开始的
fn json_doc(said: &str) -> Option<Dict> {
    if let Ok(V::Dict(d)) = json::loads(text::strip(said)) {
        return Some(d);
    }
    let lines = text::splitlines(said);
    (0..lines.len()).rev().filter(|i| lines[*i].starts_with('{')).find_map(|i| {
        match json::loads(text::strip(&lines[i..].join("\n"))) {
            Ok(V::Dict(d)) => Some(d),
            _ => None,
        }
    })
}

/// 额度里至少有一个桶
pub fn has_buckets(rl: &Dict) -> bool {
    dicts(rl, "groups").iter().any(|g| !dicts(g, "buckets").is_empty())
}

/// 一个有限的数
fn finite(v: &Option<V>) -> Option<f64> {
    match v {
        Some(V::Int(_) | V::Big(_) | V::Float(_)) => v.as_ref().and_then(|x| x.as_num()).filter(|f| f.is_finite()),
        _ => None,
    }
}

/// 字符串原样，别的（没有、不是字符串、空的）是 null
fn text_of(d: &Dict, k: &str) -> V {
    match d.str(k) {
        Some(s) if !text::strip(&s).is_empty() => V::from(s),
        _ => V::None,
    }
}

/// 一个桶在 --json 里的样子：agy 给的几项原样（id、name、description、window、disabled、remaining_fraction、remaining_amount），
/// 另加 slot（认得的窗口对应 Claude、Codex 的哪一个：five_hour、seven_day，认不出是 null；App 照它摆，不用认 agy 的叫法）
/// 和别的工具的用量窗口一样的四项：utilization（用了百分之几：100 × (1 − remaining_fraction)，两位小数）、resets_at
/// （agy 给的 reset_time）、resets_epoch（Unix 秒，认不出是 null）、reset（重置时间已经过了：缓存里的数不算数了）
fn bucket_view(b: &Dict, now: f64) -> Dict {
    let frac = finite(&b.get("remaining_fraction"));
    let used = frac.map(|f| ((1.0 - f.clamp(0.0, 1.0)) * 10000.0).round() / 100.0);
    let at = b.get("reset_time").and_then(|x| parse_ts(&x).ok().flatten()).filter(|at| at.is_finite());
    dict! {
        "id" => text_of(b, "id"),
        "name" => text_of(b, "name"),
        "description" => text_of(b, "description"),
        "window" => text_of(b, "window"),
        "slot" => window_of(b).map(|i| V::from(WINDOWS[i].3)).unwrap_or(V::None),
        "disabled" => b.get("disabled").and_then(|d| d.as_bool()).unwrap_or(false),
        "utilization" => used,
        "remaining_fraction" => frac,
        "remaining_amount" => b.get("remaining_amount").filter(|x| x.is_int()),
        "resets_at" => text_of(b, "reset_time"),
        "resets_epoch" => at,
        "reset" => at.is_some_and(|at| at <= now),
    }
}

/// --json 的 quota_groups：一组一项（name、description、buckets）；额度里没有 groups 是 None
pub fn quota_groups(rl: &Dict, now: f64) -> Option<V> {
    rl.list("groups")?;
    let groups: Vec<V> = dicts(rl, "groups")
        .iter()
        .map(|g| {
            let buckets: Vec<V> = dicts(g, "buckets").iter().map(|b| V::Dict(bucket_view(b, now))).collect();
            V::Dict(dict! { "name" => text_of(g, "name"), "description" => text_of(g, "description"), "buckets" => buckets })
        })
        .collect();
    Some(V::from(groups))
}

/// 周结算提醒（usage.rs 的 use_first）看的「本周」：各组的本周额度里用得最少的那个（过了重置时间的算 0；一样多时先重置的那个）
/// ——它也用完了这个窗口才算用完（App 的 `UsageEntry.quotaLeast` 是同一条规则），还剩不少、快重置了才值得先用这个账号。
/// 写成和 Claude、Codex 的 seven_day 一样的 {utilization, resets_at}；没有认得出的本周额度是 None
pub fn agy_week(rl: &Dict, now: f64) -> Option<Dict> {
    let mut least: Option<(f64, f64, Dict)> = None;
    for g in dicts(rl, "groups") {
        for b in dicts(&g, "buckets") {
            if b.truthy("disabled") || window_of(&b).map(|i| WINDOWS[i].3) != Some("seven_day") {
                continue;
            }
            let v = bucket_view(&b, now);
            let Some(used) = v.get("utilization").and_then(|u| u.as_num()) else { continue };
            let at = v.get("resets_epoch").and_then(|x| x.as_num());
            let used = if v.truthy("reset") { 0.0 } else { used };
            let at_key = at.unwrap_or(f64::INFINITY);
            if least.as_ref().is_none_or(|(u, a, _)| used < *u || (used == *u && at_key < *a)) {
                least = Some((used, at_key, dict! { "utilization" => used, "resets_at" => v.at("resets_at") }));
            }
        }
    }
    least.map(|(_, _, d)| d)
}

/// 每周用量按组记（usage.rs 的 `groups_add`）：各组的本周额度（没停用、认得出是 weekly、有剩余比例的第一个桶），一组一项：
/// 认这一组的 id（那个桶的 id，没有时是组名；最多 64 个字）、agy 给的组名、照 Claude、Codex 写成的 `{"seven_day": {utilization, resets_at}}`。
/// 没有本周额度的组不在里面；两组认出的 id 一样时只要前一组
pub fn agy_weeklies(rl: &Dict) -> Vec<(String, String, Dict)> {
    let cut = |s: Option<std::rc::Rc<str>>| s.map(|s| text::strip(&s).chars().take(64).collect::<String>()).unwrap_or_default();
    let mut out: Vec<(String, String, Dict)> = Vec::new();
    for g in dicts(rl, "groups") {
        let name = cut(g.str("name"));
        let Some(b) = dicts(&g, "buckets").into_iter().find(|b| {
            !b.truthy("disabled")
                && window_of(b).map(|i| WINDOWS[i].3) == Some("seven_day")
                && finite(&b.get("remaining_fraction")).is_some()
        }) else {
            continue;
        };
        let id = Some(cut(b.str("id"))).filter(|s| !s.is_empty()).unwrap_or_else(|| name.clone());
        if id.is_empty() || out.iter().any(|(x, _, _)| *x == id) {
            continue;
        }
        let v = bucket_view(&b, 0.0);
        let week = dict! { "utilization" => v.at("utilization"), "resets_at" => v.at("resets_at") };
        out.push((id, name, dict! { "seven_day" => week }));
    }
    out
}

/// 表格、菜单里一个桶的名字：组名和桶名（一样、一个包含另一个、只有一个时只写一个）
fn label(group: &V, bucket: &V) -> String {
    let (g, b) = (text::strip(group.as_str().unwrap_or("")), text::strip(bucket.as_str().unwrap_or("")));
    if g.is_empty() || b.contains(g) {
        b.to_string()
    } else if b.is_empty() || g.contains(b) {
        g.to_string()
    } else {
        format!("{g} · {b}")
    }
}

/// agy 给的额度窗口（1.5.16 在真实账号上回的是 5h、weekly）→ 和 Claude、Codex 的用量一样的叫法：表格里的（5 小时、本周）、
/// 菜单那一格里的（5h、周）。认得的窗口不显示桶名：真实的桶名是 Five Hour Limit Remaining 这种，写在「已用」旁边会被看反
/// 最后一项是 --json 里的 slot（和别的工具的窗口同名）
const WINDOWS: [(&str, &str, &str, &str); 2] = [("5h", "5 小时", "5h", "five_hour"), ("weekly", "本周", "周", "seven_day")];

/// 桶的 window 是 WINDOWS 里的哪一种（认不出是 None）
fn window_of(b: &Dict) -> Option<usize> {
    let w = b.str("window")?;
    let w = text::strip(&w).to_ascii_lowercase();
    WINDOWS.iter().position(|(k, _, _, _)| *k == w)
}

/// 表格里一个桶的名字：组名 · 窗口（认得的），认不出窗口时组名 · 桶名
fn bucket_label(g: &Dict, b: &Dict) -> String {
    match window_of(b) {
        Some(i) => label(&g.at("name"), &V::from(WINDOWS[i].1)),
        None => label(&g.at("name"), &b.at("name")),
    }
}

/// 表格里的一项额度：名字、用了多少（右对齐的百分比，或者 已重置 / 不可用 / 剩几次 / —）、重置时间、颜色
struct Item {
    label: String,
    used: String,
    resets: String,
    code: &'static str,
}

fn items(rl: &Dict, now: f64) -> R<Vec<Item>> {
    let mut out = Vec::new();
    for g in quota_groups(rl, now).and_then(|v| v.as_list()).map(|l| l.items()).unwrap_or_default() {
        let Some(g) = g.as_dict() else { continue };
        for b in dicts(&g, "buckets") {
            let mut it = Item { label: bucket_label(&g, &b), used: "—".into(), resets: String::new(), code: "" };
            let at = b.get("resets_epoch").and_then(|x| x.as_num());
            if b.truthy("disabled") {
                it.used = "不可用".into();
                it.code = "2";
            } else if b.truthy("reset") {
                it.used = "已重置".into();
            } else {
                if let Some(u) = b.get("utilization").filter(|u| !u.is_none()) {
                    let p = py_round(&u)?;
                    it.used = format!("{:>3}%", p.text);
                    it.code = if p.val >= 90.0 {
                        "31"
                    } else if p.val >= 70.0 {
                        "33"
                    } else {
                        ""
                    };
                } else if let Some(n) = b.get("remaining_amount").filter(|n| !n.is_none()) {
                    it.used = format!("剩 {}", n.py_str());
                }
                if let Some(at) = at {
                    it.resets = reset_short(at, now, at - now >= 86400.0)?;
                }
            }
            out.push(it);
        }
    }
    Ok(out)
}

/// cca agy usage 的表格：一个账号几行（一个额度一行，名字、用了多少、什么时候重置），账号和套餐只写在第一行（都没有套餐时不要那一列）；
/// 用缓存的账号，原因和多久以前的数写在第一行后面（放不下时另起一行）；查不到的从额度那一列起写原因。
/// 整行不超过终端的宽度：依次收起有别名的账号的邮箱、套餐那一列、重置时间，再截短额度的名字和账号
pub(super) fn agy_table_lines(rows: &[Row], now: f64, cols: &str, color: bool) -> R<Vec<String>> {
    let wide = (cols_of(cols)?.max(40) - 2) as usize;
    let paint = |s: &str, code: &str| -> String {
        if color && !code.is_empty() && !text::strip(s).is_empty() {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    };
    struct Acct<'a> {
        r: &'a Row,
        items: Vec<Item>,
        notes: Vec<String>,
    }
    let mut data: Vec<Acct> = Vec::new();
    for r in rows {
        let mut notes = Vec::new();
        if r.source == "none" || r.why.truthy() {
            notes.push(usage_why(r, WhyTable::Usage)?);
        }
        if r.source == "cache" {
            notes.push(format!("缓存（{}前）", ago(now - r.at.unwrap_or(now))?));
        }
        let items = if r.source == "none" { Vec::new() } else { items(&r.rl, now)? };
        data.push(Acct { r, items, notes });
    }
    let aw = rows.iter().map(|r| uni::width(&r.alias)).max().unwrap_or(0).min(16);
    let ew = rows.iter().map(|r| uni::width(&r.email)).max().unwrap_or(0).min(32);
    let acct = |r: &Row, email: bool| -> String {
        if email {
            let mut parts = Vec::new();
            if aw > 0 {
                parts.push(fit(&r.alias, aw, true));
            }
            if ew > 0 {
                parts.push(fit(&r.email, ew, true));
            }
            return parts.join("  ");
        }
        fit(if r.alias.is_empty() { &r.email } else { &r.alias }, 32, false)
    };
    let all: Vec<&Item> = data.iter().flat_map(|d| d.items.iter()).collect();
    let lw_full = all.iter().map(|i| uni::width(&i.label)).chain([uni::width("额度")]).max().unwrap_or(0).min(32);
    let uw = all.iter().map(|i| uni::width(&i.used)).chain([uni::width("已用")]).max().unwrap_or(0);
    let rw_full = all.iter().map(|i| uni::width(&i.resets)).max().unwrap_or(0);
    let has_plan = rows.iter().any(|r| !text::strip(&r.plan).is_empty());
    let (mut email, mut plan, mut resets, mut lw) = (true, has_plan, rw_full > 0, lw_full);
    let width_of = |email: bool, plan: bool, resets: bool, lw: usize| -> usize {
        let a = rows.iter().map(|r| uni::width(&acct(r, email))).chain([uni::width("账号")]).max().unwrap_or(0);
        let p =
            if plan { rows.iter().map(|r| uni::width(&r.plan)).chain([uni::width("套餐")]).max().unwrap_or(0) + 2 } else { 0 };
        2 + a + p + 2 + lw + 2 + uw + if resets { 2 + rw_full.max(uni::width("重置")) } else { 0 }
    };
    if width_of(email, plan, resets, lw) > wide && rows.iter().any(|r| !r.alias.is_empty() && !r.email.is_empty()) {
        email = false;
    }
    if width_of(email, plan, resets, lw) > wide {
        plan = false;
    }
    if width_of(email, plan, resets, lw) > wide {
        resets = false;
    }
    while width_of(email, plan, resets, lw) > wide && lw > 8 {
        lw -= 1;
    }
    let a = rows.iter().map(|r| uni::width(&acct(r, email))).chain([uni::width("账号")]).max().unwrap_or(0);
    let a = if width_of(email, plan, resets, lw) > wide {
        a.saturating_sub(width_of(email, plan, resets, lw) - wide).max(10)
    } else {
        a
    };
    let p = if plan { rows.iter().map(|r| uni::width(&r.plan)).chain([uni::width("套餐")]).max().unwrap_or(0) } else { 0 };
    let rw = if resets { rw_full.max(uni::width("重置")) } else { 0 };
    let lead = |first: bool, r: &Row| -> String {
        let mut s = format!("  {}", if first { fit(&acct(r, email), a, true) } else { " ".repeat(a) });
        if plan {
            s.push_str("  ");
            s.push_str(&if first { fit(&r.plan, p, true) } else { " ".repeat(p) });
        }
        s
    };
    let mut lines = Vec::new();
    let mut head = format!("  {}", fit("账号", a, true));
    if plan {
        head.push_str(&format!("  {}", fit("套餐", p, true)));
    }
    head.push_str(&format!("  {}  {}", fit("额度", lw, true), fit("已用", uw, true)));
    if resets {
        head.push_str(&format!("  {}", fit("重置", rw, true)));
    }
    lines.push(paint(&fit(text::rstrip(&head), wide, false), "2"));
    for d in &data {
        let dim = if d.r.source == "live" { "" } else { "2" };
        if d.items.is_empty() {
            let first = lead(true, d.r);
            let start = uni::width(&first) + 2;
            let said = fit(&d.notes.join("；"), wide.saturating_sub(start).max(10), false);
            lines.push(format!("{}  {}", paint(&first, dim), paint(&said, "2")));
            continue;
        }
        let first = lines.len();
        let mut first_width = 0;
        for (i, it) in d.items.iter().enumerate() {
            let cells = [
                (lead(i == 0, d.r), dim),
                (fit(&it.label, lw, true), dim),
                (fit(&it.used, uw, true), if it.code.is_empty() { dim } else { it.code }),
                (if resets { fit(&it.resets, rw, true) } else { String::new() }, dim),
            ];
            let cells = &cells[..if resets { 4 } else { 3 }];
            if i == 0 {
                first_width = uni::width(text::rstrip(&cells.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>().join("  ")));
            }
            let line = cells.iter().map(|(s, code)| paint(s, code)).collect::<Vec<_>>().join("  ");
            lines.push(text::rstrip(&line).to_string());
        }
        if !d.notes.is_empty() {
            // 备注：第一行后面放得下就放那里，不然在下面另起一行（和额度的名字对齐）
            let said = d.notes.join("；");
            if first_width + 2 + uni::width(&said) <= wide {
                lines[first] = format!("{}  {}", lines[first], paint(&said, "2"));
            } else {
                let start = uni::width(&lead(false, d.r)) + 2;
                let note = format!("{}{}", " ".repeat(start), fit(&said, wide.saturating_sub(start).max(10), false));
                lines.insert(first + 1, paint(&note, "2"));
            }
        }
    }
    Ok(lines)
}

/// 菜单里的一格（只读缓存）：和 Claude、Codex 的一样写 5h 13% · 周 41%——各组里同一个窗口用得最多的那个（这个窗口的桶都过了
/// 重置时间时写已重置）；窗口都认不出时写用得最多的那个额度（Gemini Pro 13%）。数据旧了注明多久前；没有百分比时不写
pub fn agy_cell(rl: &Dict, at: f64, now: f64) -> R<String> {
    let mut most: [Option<(f64, String)>; 2] = [None, None];
    let mut reset = [false; 2];
    let mut best: Option<(f64, String, String)> = None;
    for g in quota_groups(rl, now).and_then(|v| v.as_list()).map(|l| l.items()).unwrap_or_default() {
        let Some(g) = g.as_dict() else { continue };
        for b in dicts(&g, "buckets") {
            if b.truthy("disabled") {
                continue;
            }
            let w = window_of(&b);
            if b.truthy("reset") {
                if let Some(i) = w {
                    reset[i] = true;
                }
                continue;
            }
            let Some(u) = b.get("utilization").filter(|u| !u.is_none()) else { continue };
            let p = py_round(&u)?;
            if let Some(i) = w {
                if most[i].as_ref().is_none_or(|(v, _)| p.val > *v) {
                    most[i] = Some((p.val, p.text.clone()));
                }
            }
            if best.as_ref().is_none_or(|(v, _, _)| p.val > *v) {
                best = Some((p.val, fit(&label(&g.at("name"), &b.at("name")), 16, false), p.text));
            }
        }
    }
    let mut cells: Vec<String> = Vec::new();
    for (i, (_, _, short, _)) in WINDOWS.iter().enumerate() {
        match &most[i] {
            Some((_, pct)) => cells.push(format!("{short} {pct}%")),
            None if reset[i] => cells.push(format!("{short} 已重置")),
            None => {}
        }
    }
    if cells.is_empty() {
        let Some((_, name, pct)) = best else { return Ok(String::new()) };
        cells.push(format!("{name} {pct}%"));
    }
    let old = if now - at >= AGY_STALE { format!("（{}前）", ago(now - at)?) } else { String::new() };
    Ok(format!("{}{}", cells.join(" · "), old))
}

// --- 套餐：cca 启动的交互会话里状态栏的命令 -----------------------------------------------------------------------

/// 状态栏记下的套餐（账号目录里：`plan_note` 写、`agy_plan_seen` 读）：`{email, plan_tier, at}`。Team 里别的设备记下、经折合金额的
/// 同步拿来的（`agy_plan_adopt`）多一项 from（那台设备的名字）：只有这台设备自己记下的才传给别的设备（`agy_plan_own`）
const AGY_PLAN_FILE: &str = "agy-plan.json";
/// 套餐和记着的一样时隔这么久才重写一次：at 是最近一次看到它的时间（差不到这么久），几台设备记的不一样时按它认谁新
const AGY_PLAN_REFRESH: f64 = 3600.0;
/// 有了它就不是交互的会话（不画状态栏）的选项：打印模式、版本、帮助
const AGY_ONE_SHOT: &[&str] = &["p", "print", "prompt", "version", "help", "h"];
/// agy 1.5.18 的子命令（agy --help）：选项后面第一个不是选项的参数是它们之一时，不开会话
const AGY_SUBCOMMANDS: &[&str] = &[
    "acp",
    "agent",
    "agents",
    "changelog",
    "help",
    "install",
    "mcp",
    "mic-serve",
    "models",
    "plugin",
    "plugins",
    "remote-control",
    "update",
];

/// 状态栏上次记下的套餐的叫法（`plan_note`；或者 Team 里别的设备记下的，`agy_plan_adopt`）：记下时的邮箱和现在登录的一样
/// （大小写不论）才算；换了账号登录、没记过是空的
pub fn agy_plan_seen(key: &str, email: &str) -> String {
    let Some(d) = load_quiet(&path::join(&adir(key), AGY_PLAN_FILE)).and_then(|v| v.as_dict()) else { return String::new() };
    match (d.str("email"), d.str("plan_tier")) {
        (Some(e), Some(p)) if !email.is_empty() && e.to_lowercase() == email.to_lowercase() => p.to_string(),
        _ => String::new(),
    }
}

/// 这台设备自己的状态栏记下的这个账号的套餐（不是从别的设备那里拿来的；记下时的邮箱是这个账号的，大小写不论）：(叫法, 记下的
/// 时间)。dir 是账号目录。折合金额的同步把它放进这台设备传的那一份（cost.rs 的 cost_round：明文的 pl），Team 里别的设备拿去显示
pub fn agy_plan_own(dir: &str, email: &str) -> Option<(String, f64)> {
    let d = load_quiet(&path::join(dir, AGY_PLAN_FILE))?.as_dict()?;
    let (plan, at) = (d.str("plan_tier")?, d.get("at")?.as_num().filter(|a| a.is_finite() && *a > 0.0)?);
    let mine = !d.has("from") && d.str("email").is_some_and(|e| e.to_lowercase() == email.to_lowercase());
    (mine && !text::strip(&plan).is_empty() && plan.chars().count() <= 64).then(|| (plan.to_string(), at))
}

/// Team 里别的设备记下的这个账号的套餐（折合金额的同步解开的、最新的那一份：叫法、那台设备记下它的时间、那台设备的名字）：
/// 这台设备没记过、记的不是这个邮箱的，或者记的套餐不一样、又比它旧时记下来（标着 from，邮箱是这个账号的），App 照它显示（agy 1.5
/// 的登录信息里没有套餐，只有开过交互会话的设备记得下）。套餐一样时不动（这台设备自己记的照旧传）；账号目录不在（这台设备上没用过
/// 这个账号）不建。返回写了没有
pub fn agy_plan_adopt(dir: &str, email: &str, plan: &str, at: f64, from: &str) -> bool {
    if !path::isdir(dir) {
        return false;
    }
    let file = path::join(dir, AGY_PLAN_FILE);
    if let Some(old) = load_quiet(&file).and_then(|v| v.as_dict()) {
        let ours = old.str("email").is_some_and(|e| e.to_lowercase() == email.to_lowercase());
        let old_at = old.get("at").and_then(|a| a.as_num()).filter(|a| a.is_finite()).unwrap_or(0.0);
        if ours && (old.str("plan_tier").as_deref() == Some(plan) || old_at >= at) {
            return false;
        }
    }
    let rec = dict! { "email" => email.to_lowercase(), "plan_tier" => plan, "at" => at, "from" => from };
    save_json(&file, &V::Dict(rec), Some(0o600), false).is_ok()
}

/// 这次启动是不是 agy 的交互会话（只有它画状态栏）。agy 照 Go 的 flag 认参数：`-x` 和 `--x` 一样，选项认到第一个不是选项的
/// 参数（或者 `--`）为止，那个参数是子命令就不开会话。打印模式、--version、--help 不是；自己带了 --settings 的也不加
/// （agy 只认一个 --settings，用户的那一份要原样生效）
fn agy_takes_statusline(args: &[String]) -> bool {
    agy_session(args, false)
}

/// 是不是 agy 的交互会话（远程启动的模板只能起这种，remote/template.rs）：和上面一样认，只是自己带了 --settings 的也算
pub(crate) fn agy_interactive(args: &[String]) -> bool {
    agy_session(args, true)
}

/// 远程启动的开场白交给 agy 的选项（remote/template.rs 的 `line_args`）：agy 不认参数里的一句话（1.5.18：`agy -- 一句话` 报
/// unexpected argument、退出码 2；话只能经 -p / --print、-i / --prompt-interactive 或者标准输入给），交互的会话带开场白是
/// `--prompt-interactive=开场白`（写成一项：开场白以 - 开头也不会被当成选项）
pub(crate) const AGY_LINE_FLAG: &str = "--prompt-interactive";

/// 远程启动能不能给这次会话加一句开场白（[`AGY_LINE_FLAG`]）：参数里已经有 -i / --prompt-interactive 的不给（已经有头一句话了，和
/// Claude Code、Codex 参数里已经有一句话的一样），有不是选项的参数或者 `--` 的也不给（agy 不认它们，本来就起不来）；打印模式这些不是
/// 交互的会话，也不给
pub(crate) fn agy_line_ok(args: &[String]) -> bool {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let Some(flag) = a.strip_prefix("--").or_else(|| a.strip_prefix('-')).filter(|f| !f.is_empty()) else {
            return false;
        };
        let (name, inline) = flag.split_once('=').map_or((flag, false), |(n, _)| (n, true));
        if matches!(name, "i" | "prompt-interactive") || AGY_ONE_SHOT.contains(&name) {
            return false;
        }
        if !inline && AGY_VALUES.iter().any(|v| v.trim_start_matches('-') == name) {
            it.next();
        }
    }
    true
}

/// agy 的选项名（不带 -、--、=值），认到第一个不是选项的参数（或者 `--`）为止；带值的（settings.rs 的 AGY_VALUES）跳过它的值
fn agy_flags(args: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let Some(flag) = a.strip_prefix("--").or_else(|| a.strip_prefix('-')).filter(|f| !f.is_empty()) else { break };
        let (name, inline) = flag.split_once('=').map_or((flag, false), |(n, _)| (n, true));
        out.push(name);
        if !inline && AGY_VALUES.iter().any(|v| v.trim_start_matches('-') == name) {
            it.next();
        }
    }
    out
}

/// 这一次是不是 agy 干活的会话（凭证防护只加给它们）：交互的会话（`agy_interactive`，自己带了 --settings 的也算），或者打印模式
/// （-p / --print / --prompt：问一句、答完就退出）。--version、--help、子命令不是
fn agy_agent(args: &[String]) -> bool {
    let flags = agy_flags(args);
    if flags.iter().any(|f| matches!(*f, "version" | "help" | "h")) {
        return false;
    }
    agy_interactive(args) || flags.iter().any(|f| matches!(*f, "p" | "print" | "prompt"))
}

fn agy_session(args: &[String], own_settings: bool) -> bool {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--" {
            return it.next().is_none_or(|x| !AGY_SUBCOMMANDS.contains(&x.as_str()));
        }
        let Some(flag) = a.strip_prefix("--").or_else(|| a.strip_prefix('-')).filter(|f| !f.is_empty()) else {
            return !AGY_SUBCOMMANDS.contains(&a.as_str());
        };
        let (name, inline) = flag.split_once('=').map_or((flag, false), |(n, _)| (n, true));
        if (name == "settings" && !own_settings) || AGY_ONE_SHOT.contains(&name) {
            return false;
        }
        if !inline && AGY_VALUES.iter().any(|v| v.trim_start_matches('-') == name) {
            it.next();
        }
    }
    true
}

/// 交给 agy 的 statusLine：用户自己没配（或者停用了、命令是空的）时只有 cca 的，叠在 agy 自己的状态栏上（stack_with_default；
/// cca 的什么都不输出）；用户配了命令时照用户的那一份（padding 这些都留着），命令换成 cca 的、后面经 `--then` 接着运行用户的
/// （同一份 JSON，它的输出就是状态栏）。type 不是 command 的（不知道是什么）不动它：返回 None。user 不改
fn statusline_for(user: Option<&V>, base: &str) -> Option<Dict> {
    let own =
        || dict! { "type" => "command", "command" => base, "enabled" => true, "padding" => 0i64, "stack_with_default" => true };
    let Some(u) = user.and_then(V::as_dict) else { return Some(own()) };
    if u.get("type").is_some_and(|x| !matches!(x, V::None) && x.as_str() != Some("command")) {
        return None;
    }
    let cmd = u.str("command").map(|c| text::strip(&c).to_string()).unwrap_or_default();
    if cmd.is_empty() || cmd.contains("_agy-statusline") || matches!(u.get("enabled"), Some(V::Bool(false))) {
        return Some(own());
    }
    let line = u.copy();
    line.set("type", "command");
    line.set("command", format!("{base} --then {}", sh_quote(&cmd)));
    line.set("enabled", true);
    Some(line)
}

/// 启动 agy 时只这一次加在用户参数前面的（`py launch-args`，见 accounts.rs 的 `cmd_launch_args`）和启动时说的一句（凭证挡不住的
/// 原因），都在一个 --settings 里（agy 只认一个；它整个顶掉 settings.json 里同名的顶层键，别的照 settings.json）——
/// - 干活的会话（交互的、打印模式：`agy_agent`）带上 permissions：用户自己的那一份，deny 加上 ~/.cca 里的登录凭证（guard.rs 的
///   `agy_permissions`）
/// - 交互的会话再带上 cca 的状态栏命令（statusLine），用来记下套餐：agy 1.5 的登录文件里没有档位了，只有状态栏的 JSON 里有（plan_tier）
///
/// 自己带了 --settings 的什么都不加（用户的那一份原样生效），说一句；共享的 settings.json 读不了、不是 JSON 对象时也不加（不知道
/// 用户配了什么，别把它的状态栏、权限盖掉），说一句
pub fn agy_launch_args(key: &str, args: &[String]) -> (Vec<String>, Option<String>) {
    if !agy_agent(args) {
        return (Vec::new(), None);
    }
    let note = |why: &str| Some(format!("~/.cca 里的登录凭证这次没给 Antigravity 挡住：{why}"));
    if agy_flags(args).contains(&"settings") {
        return (Vec::new(), note("这次带了自己的 --settings（agy 只认一个）"));
    }
    let user = match std::fs::read(path::join_all(&[&agy().master, "antigravity-cli", "settings.json"])) {
        Ok(raw) => match json::loads_file_bytes(&raw).ok().and_then(|v| v.as_dict()) {
            Some(d) => d,
            None => return (Vec::new(), note("共享的 settings.json 不是 JSON 对象")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Dict::new(),
        Err(_) => return (Vec::new(), note("共享的 settings.json 读不了")),
    };
    let out = Dict::new();
    if agy_takes_statusline(args) {
        // 会话标记：这一次启动（一个 agy 进程）的状态记在它名下（`agy_turn`：和上一次比才看得出做完了）
        let base = format!("{} _agy-statusline {} {}", sh_quote(&SELF_PATH), sh_quote(&acct_id(key, agy())), token_hex(8));
        if let Some(line) = statusline_for(user.get("statusLine").as_ref(), &base) {
            out.set("statusLine", line);
        }
    }
    let said = match super::guard::agy_permissions(user.get("permissions")) {
        Ok(p) => {
            out.set("permissions", p);
            None
        }
        Err(why) => note(&why),
    };
    if out.is_empty() {
        return (Vec::new(), said);
    }
    (vec!["--settings".to_string(), json::dumps(&V::Dict(out), json::LINE)], said)
}

/// `cca _agy-statusline 账号目录名 会话标记 [--then 命令]`：cca 启动的 agy 交互会话里状态栏的命令（`agy_launch_args` 经 --settings
/// 交给 agy）。状态变了 agy 就用 sh -c 起它，JSON 从 stdin 进（1.5.18：最多 300 毫秒一次，5 秒不完就杀掉，连着 30 次失败就不再起）：
/// 里面的 plan_tier（套餐的叫法）连同 email 记进账号目录（`plan_note`），和这个会话上一次的状态比、该推的通知推出去（`agy_turn`），
/// 自己什么都不输出。--then 是用户自己的状态栏命令：用同一份 JSON 接着运行它，它的输出和退出码就是这一次的（agy 照它画状态栏）。
/// 也认没有会话标记的（换版本以前启动、还开着的会话）：只记套餐
fn cmd_agy_statusline(args: &[String]) -> R<()> {
    arity("cmd_agy_statusline", args, 1, 4)?;
    let (id, run, then) = match args {
        [id] => (id, None, None),
        [id, flag, cmd] if flag == "--then" => (id, None, Some(cmd)),
        [id, run] => (id, Some(run), None),
        [id, run, flag, cmd] if flag == "--then" => (id, Some(run), Some(cmd)),
        _ => return die("用法：cca _agy-statusline 账号目录名 会话标记 [--then 命令]"),
    };
    // SAFETY: 只是问 fd 0 是不是终端（手动运行时不等输入）
    let tty = unsafe { libc::isatty(0) } == 1;
    let mut raw = Vec::new();
    if !tty {
        let _ = std::io::stdin().lock().read_to_end(&mut raw);
        if let Some((dir, d)) = statusline_input(id, &raw) {
            plan_note(&dir, &d);
            if let Some(run) = run {
                if let Some(t) = agy_turn(&dir, run, &d) {
                    agy_queue(&dir, run, &t);
                }
            }
        }
    }
    let Some(cmd) = then else { return Ok(()) };
    let p = Proc::new(["sh", "-c", cmd.as_str()]);
    match if tty { p } else { p.input(raw) }.run() {
        Ok(d) if d.rc == 0 => Ok(()),
        Ok(d) => exit(if d.rc < 0 { 128 - d.rc } else { d.rc }),
        Err(_) => exit(127),
    }
}

/// 状态栏的命令收到的：账号目录（`agy_dir`）和状态栏的 JSON（最多 256 KB）
fn statusline_input(id: &str, raw: &[u8]) -> Option<(String, Dict)> {
    if raw.len() > 256 << 10 {
        return None;
    }
    let dir = agy_dir(id)?;
    let d = json::loads_file_bytes(raw).ok()?.as_dict()?;
    Some((dir, d))
}

/// 账号目录名对应的账号目录：名字要像账号目录名、目录要在（只碰 cca 自己的账号目录）
fn agy_dir(id: &str) -> Option<String> {
    re!(r"[a-z0-9][a-z0-9-]{0,63}").fullmatch(id)?;
    let dir = path::join_all(&[&agy().sroot, "accounts", id]);
    path::isdir(&dir).then_some(dir)
}

/// 记下状态栏 JSON 里的套餐：plan_tier 是不空的字符串（最多 64 个字），email 像个邮箱（读的时候拿它和登录的邮箱比）。和记着的
/// 一样、是这台设备自己记的、记下不到 AGY_PLAN_REFRESH 就不写（agy 最多 300 毫秒起一次）；从别的设备那里拿来的（from）换成这台
/// 设备自己看到的。写不了也不说（状态栏的命令不出声）
fn plan_note(dir: &str, d: &Dict) {
    let plan = d.str("plan_tier").map(|p| text::strip(&p).to_string()).unwrap_or_default();
    let email = d.str("email").map(|e| e.to_string()).unwrap_or_default();
    if plan.is_empty() || plan.chars().count() > 64 || !EMAIL_RE.matches(&email) {
        return;
    }
    let (file, now) = (path::join(dir, AGY_PLAN_FILE), time());
    if let Some(old) = load_quiet(&file).and_then(|v| v.as_dict()) {
        let age = now - old.get("at").and_then(|a| a.as_num()).unwrap_or(f64::NAN);
        if old.str("plan_tier").as_deref() == Some(plan.as_str())
            && old.str("email").as_deref() == Some(email.as_str())
            && !old.has("from")
            && (0.0..AGY_PLAN_REFRESH).contains(&age)
        {
            return;
        }
    }
    let _ = save_json(&file, &V::Dict(dict! { "email" => email, "plan_tier" => plan, "at" => now }), Some(0o600), false);
}

/// 每个会话（会话标记）上一次的状态记在账号目录的这个目录下（`agy_turn`）
const AGY_TURNS: &str = "agy-turns";
/// agent_state 是这几种时在干活（1.5.18 的文档：idle、working、tool_use、reviewing、authenticating、initializing、error）
const AGY_BUSY: &[&str] = &["working", "tool_use", "reviewing"];
/// 停下（回到 idle、到了 error）以后等这么久（秒）再推「做完了 / 出错了」，这中间又干活了就不推：agy 一轮里会停下好几次（两步之间、
/// 等后台的命令和子代理），状态栏那时也说 idle。2026-11-27 一天推出去的 65 条「做完了」有 49 条推在一轮的中途，其中 38 条不到 30 秒
/// 就接着干了（停得更久的多半在等后台的事：`agy_bg`）
const AGY_DONE_WAIT: f64 = 30.0;
/// tool_confirmation_pending 变成 true 以后等这么久（秒）再推「在等你」：这中间确认过了（人就在终端前面）就不推
const AGY_ASK_WAIT: f64 = 6.0;
/// 会话的标题最多记这么多个字（推的时候还要截短）
const AGY_TOPIC_MAX: usize = 400;

/// 一个会话上一次的状态：agent_state、在不在等你确认（tool_confirmation_pending）、上次排上通知以后干过活没有；排着等推的那一条
/// （`due`，没有是 None）和它的编号（`mark`：每排上、撤掉一次加一，等着推的进程醒来时编号没变才推）；会话的标题、agy 启动的文件夹
/// （推的时候用最新记下的）
#[derive(Clone, Debug, Default, PartialEq)]
struct Turn {
    state: String,
    ask: bool,
    worked: bool,
    due: Option<AgyEvent>,
    mark: u64,
    topic: String,
    cwd: String,
}

impl Turn {
    /// 文件里是一行 JSON；3.7.11 及以前写的是一行「状态 0|1 0|1」（换版本以前启动、还开着的会话留下的）：那样的没有排着的
    fn parse(s: &str) -> Option<Turn> {
        if let Some(d) = json::loads(s).ok().and_then(|v| v.as_dict()) {
            let text = |k: &str| d.str(k).map(|s| s.to_string()).unwrap_or_default();
            let flag = |k: &str| d.get(k).and_then(|v| v.as_bool()) == Some(true);
            let state = text("state");
            return (!state.is_empty()).then(|| Turn {
                state,
                ask: flag("ask"),
                worked: flag("worked"),
                due: d.str("due").and_then(|w| due_of(&w)),
                mark: d.get("mark").and_then(|v| v.as_int()).and_then(|n| u64::try_from(n).ok()).unwrap_or(0),
                topic: text("topic"),
                cwd: text("cwd"),
            });
        }
        let mut w = s.split_whitespace();
        let (state, ask, worked) = (w.next()?, w.next()?, w.next()?);
        Some(Turn { state: state.to_string(), ask: ask == "1", worked: worked == "1", ..Turn::default() })
    }

    fn line(&self) -> String {
        let d = dict! {
            "state" => self.state.as_str(), "ask" => self.ask, "worked" => self.worked,
            "due" => self.due.map_or(V::None, |e| V::from(due_word(e))), "mark" => i64::try_from(self.mark).unwrap_or(i64::MAX),
            "topic" => self.topic.as_str(), "cwd" => self.cwd.as_str()
        };
        json::dumps(&V::Dict(d), json::LINE) + "\n"
    }

    /// 换掉排着的那一条（排上、撤掉）：编号加一，之前起的等着推的进程醒来时就不推了
    fn queue(&mut self, due: Option<AgyEvent>) {
        self.due = due;
        self.mark += 1;
    }
}

fn due_word(e: AgyEvent) -> &'static str {
    match e {
        AgyEvent::Done => "done",
        AgyEvent::Failed => "failed",
        AgyEvent::Ask => "ask",
    }
}

fn due_of(w: &str) -> Option<AgyEvent> {
    match w {
        "done" => Some(AgyEvent::Done),
        "failed" => Some(AgyEvent::Failed),
        "ask" => Some(AgyEvent::Ask),
        _ => None,
    }
}

/// 后台还有事：在跑的后台任务（task_count）、子代理（subagents 里 status 是 running 的）、排着的消息（pending_input_count）。
/// agy 等它们的时候停下（状态栏说 idle），它们完了接着干：这时停下不算做完
fn agy_bg(d: &Dict) -> bool {
    let some = |k: &str| d.get(k).and_then(|v| v.as_num()).is_some_and(|n| n > 0.0);
    let running = d
        .get("subagents")
        .and_then(|v| v.as_list())
        .is_some_and(|l| l.items().iter().any(|s| s.as_dict().is_some_and(|s| s.str("status").as_deref() == Some("running"))));
    some("task_count") || some("pending_input_count") || running
}

/// 和上一次比，该排上的通知排上、排着的该撤的撤掉（排上的等一会儿再推：`agy_queue`）。干过活以后停下（idle、error），后台没事
/// （`agy_bg`）、也不在等你确认：排上做完了 / 出错了；tool_confirmation_pending 变成 true：排上在等你。排着的做完了 / 出错了，
/// 又干活了、后台有事了、要你确认了就撤掉（干的活留着：再停下时再排）；排着的在等你，确认框没了就撤掉。这个会话头一次（还没有
/// 上一次）只记下
fn turn_step(prev: Option<&Turn>, d: &Dict) -> Turn {
    let state: String =
        d.str("agent_state").map(|s| s.chars().filter(|c| c.is_ascii_graphic()).take(32).collect()).unwrap_or_default();
    let ask = d.get("tool_confirmation_pending").is_some_and(|v| v.as_bool() == Some(true));
    let busy = AGY_BUSY.contains(&state.as_str());
    let bg = agy_bg(d);
    let topic = d.str("conversation_title").map(|t| t.chars().take(AGY_TOPIC_MAX).collect()).unwrap_or_default();
    let cwd = d.str("cwd").map(|c| c.to_string()).unwrap_or_default();
    let Some(p) = prev else { return Turn { state, ask, worked: busy, topic, cwd, ..Turn::default() } };
    let mut t = Turn { state, ask, worked: p.worked || busy, due: p.due, mark: p.mark, topic, cwd };
    match t.due {
        Some(AgyEvent::Done | AgyEvent::Failed) if busy || bg || ask => {
            t.queue(None);
            t.worked = true;
        }
        Some(AgyEvent::Ask) if !ask => t.queue(None),
        _ => {}
    }
    if ask && !p.ask {
        t.queue(Some(AgyEvent::Ask));
    } else if t.worked && !busy && !bg && !ask && (t.state == "idle" || t.state == "error") {
        t.queue(Some(if t.state == "idle" { AgyEvent::Done } else { AgyEvent::Failed }));
        t.worked = false;
    }
    t
}

/// 这个会话（会话标记 run：一次启动一个）这一次的状态：和上一次比（`turn_step`），记进账号目录的 agy-turns/<会话标记>（目录 700、
/// 文件 600，原子写；新的会话头一次记的时候顺手删掉一天以前的）。这一次新排上了通知时给出记下的这一份（`agy_queue` 接着办）。
/// JSON 里没有 agent_state 的（旧版的 agy）看不出来，什么都不记、不推（不然记下的那一份读不回来，每一次都当头一次）。写不了也不说
fn agy_turn(dir: &str, run: &str, d: &Dict) -> Option<Turn> {
    if !hex_of(run, 16) {
        return None;
    }
    let turns = path::join(dir, AGY_TURNS);
    let file = path::join(&turns, run);
    let prev = std::fs::read_to_string(&file).ok().and_then(|s| Turn::parse(&s));
    let now = turn_step(prev.as_ref(), d);
    if now.state.is_empty() {
        return None;
    }
    if prev.as_ref() != Some(&now) {
        // 新会话头一次记时删掉一天以前的（早就结束了的会话留下的）
        crate::py::fs::note_file(&turns, &file, &now.line(), prev.is_none());
    }
    let queued = prev.is_some_and(|p| p.mark != now.mark) && now.due.is_some();
    queued.then_some(now)
}

/// 排上了一条通知：等一会儿再推（起 `agy-turn-wait`）；不用等（`wait_for` 是 0）、起不来时当场推（remote 的 `notify_deliver`）
fn agy_queue(dir: &str, run: &str, t: &Turn) {
    let Some(due) = t.due else { return };
    if wait_for(due) <= 0.0 || !spawn_waiter(dir, run, t.mark, due) {
        notify_deliver(&agy_note(due, &t.topic, &t.cwd));
    }
}

/// 排上的通知等多久再推（秒）。`CCA_AGY_NOTE_WAIT` 是测试用的：两种都等这么久，0 是不等、当场推
fn wait_for(due: AgyEvent) -> f64 {
    let given = proc::getenv("CCA_AGY_NOTE_WAIT").and_then(|s| s.trim().parse::<f64>().ok()).filter(|w| w.is_finite() && *w >= 0.0);
    given.map_or(if due == AgyEvent::Ask { AGY_ASK_WAIT } else { AGY_DONE_WAIT }, |w| w.min(600.0))
}

/// 起等着推的那个进程（`agy-turn-wait`）：自己一个会话（agy 杀状态栏的命令时轮不到它）、不接 agy 的管道（状态栏的命令照样马上
/// 返回）、不等它。它醒来时已经不在会话宿主下面了：宿主在这里先找好交给它。起不来是 false
fn spawn_waiter(dir: &str, run: &str, mark: u64, due: AgyEvent) -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let mut argv = vec![exe.to_string_lossy().into_owned(), "agy-turn-wait".to_string(), path::basename(dir).to_string()];
    argv.extend([run.to_string(), mark.to_string(), due_word(due).to_string()]);
    argv.extend(host_of_me());
    Proc::new(argv).stdin(In::Null).stdout(Out::Null).stderr(Out::Null).new_session().spawn().is_ok()
}

/// `agy-turn-wait 账号目录名 会话标记 编号 done|failed|ask [会话宿主的编号]`：状态栏的命令排上一条通知以后起的（`spawn_waiter`）。
/// 等一会儿（`wait_for`）再看这个会话的状态：排着的还是这一条（`still_due`）才推——给了会话宿主时推给接着它的各处，宿主已经不在了
/// （会话结束了）就都不推（remote 的 `notify_deliver_to`）；这台 Mac 上弹一个。什么都不说
fn cmd_agy_turn_wait(args: &[String]) -> R<()> {
    arity("cmd_agy_turn_wait", args, 4, 5)?;
    let host = args.get(4).filter(|s| hex_of(s, 32)).map(String::as_str);
    let (Some(dir), Ok(mark), Some(due)) = (agy_dir(&args[0]), args[2].parse::<u64>(), due_of(&args[3])) else { return Ok(()) };
    if !hex_of(&args[1], 16) {
        return Ok(());
    }
    crate::py::time::sleep(wait_for(due));
    if let Some(t) = still_due(&dir, &args[1], mark, due) {
        notify_deliver_to(&agy_note(due, &t.topic, &t.cwd), host);
    }
    Ok(())
}

/// 醒来时这个会话排着的还是不是这一条（编号、种类都对得上：这中间没又干活、没确认过）：是的话给出记下的这一份（标题、文件夹是最新的）
fn still_due(dir: &str, run: &str, mark: u64, due: AgyEvent) -> Option<Turn> {
    let t = std::fs::read_to_string(path::join_all(&[dir, AGY_TURNS, run])).ok().and_then(|s| Turn::parse(&s))?;
    (t.mark == mark && t.due == Some(due)).then_some(t)
}

/// n 个小写十六进制字符（会话标记 16 个，会话宿主的编号 32 个）
fn hex_of(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quota() -> Dict {
        json::loads(
            r#"{"description": "Quota refreshes periodically", "groups": [
                {"name": "Gemini Pro", "buckets": [
                    {"id": "pro-5h", "name": "5-hour limit", "window": "5h", "remaining_fraction": 0.87, "reset_time": "2026-11-24T15:00:00Z"},
                    {"id": "pro-week", "name": "Weekly limit", "window": "weekly", "remaining_fraction": 0.05, "reset_time": "2026-11-28T00:00:00Z"}]},
                {"name": "Claude", "description": "Claude and GPT-OSS models", "buckets": [
                    {"name": "Claude", "remaining_fraction": 1, "reset_time": "2026-11-24T08:00:00Z"},
                    {"name": "Requests", "remaining_amount": 12},
                    {"name": "Off", "disabled": true}]}]}"#,
        )
        .unwrap()
        .as_dict()
        .unwrap()
    }

    #[test]
    fn login_prompt() {
        let said = "Authentication required. Please visit the URL to log in:\n  https://accounts.google.com/o/oauth2/auth?access_type=offline&client_id=x\n";
        assert!(asks_login(b"", said.as_bytes()));
        assert!(asks_login(said.as_bytes(), b""));
        // 额度的说明里带个 Google 账号的链接、刷新 token 的地址：不是在等人登录
        let doc = r#"{"status":"SUCCESS","command":{"data":{"description":"see https://accounts.google.com/ for details"}}}"#;
        assert!(!asks_login(doc.as_bytes(), b""));
        assert!(!asks_login(b"", b"refreshing via https://oauth2.googleapis.com/token\n"));
    }

    #[test]
    fn answers() {
        let ok = format!(
            r#"{{"conversation_id":"c-1","status":"SUCCESS","response":"","error":"","command":{{"name":"usage","data":{}}}}}"#,
            json::dumps(&V::Dict(quota()), json::LINE)
        );
        let r = agy_answer(&ok, "", Some(0));
        assert!(r.truthy("ok"));
        assert!(r.dict("usage").unwrap().truthy("rate_limits_available"));
        assert_eq!(dicts(&r.dict("usage").unwrap().dict("rate_limits").unwrap(), "groups").len(), 2);
        // 前面多几行别的、整份分成几行也认
        let pretty = format!("warning: x\n{}\n", json::dumps(&json::loads(&ok).unwrap(), json::Fmt::new().indent(2)));
        assert!(agy_answer(&pretty, "", Some(0)).truthy("ok"));
        let why = |said: &str, err: &str, rc: Option<i32>| {
            let r = agy_answer(said, err, rc);
            (r.str("why").unwrap_or_default().to_string(), r.str("msg").unwrap_or_default().to_string())
        };
        // 1.5.16 实测：没登录、等到超时
        let auth = r#"{"conversation_id":"","status":"ERROR","response":"","error":"authentication failed or timed out","duration_seconds":0,"num_turns":0,"finished_reason":"error"}"#;
        assert_eq!(
            why(auth, "Error: authentication timed out.\n", Some(1)),
            ("error".into(), "authentication failed or timed out".into())
        );
        assert_eq!(why(r#"{"status":"SUCCESS","response":"I can't check that."}"#, "", Some(0)).0, "unsupported");
        assert_eq!(why(r#"{"status":"ERROR","error":"/usage is not available in print mode"}"#, "", Some(1)).0, "unsupported");
        assert_eq!(
            why(r#"{"status":"SUCCESS","command":{"name":"usage","data":{"x":1}}}"#, "", Some(0)).1,
            "agy 回的额度认不出（格式变了）"
        );
        assert_eq!(
            why(r#"{"status":"SUCCESS","command":{"name":"usage","data":{"groups":[{"name":"x","buckets":[]}]}}}"#, "", Some(0)).1,
            "agy 没给这个账号的额度"
        );
        assert_eq!(why("", "boom\nError: no network\n", Some(1)), ("error".into(), "Error: no network".into()));
        assert_eq!(why("", "", Some(2)), ("error".into(), "没有回应就退出了（退出码 2）".into()));
        assert_eq!(why("", "", None), ("error".into(), "没有回应就退出了（退出码 None）".into()));
        assert_eq!(why(r#"{"status":"ERROR","error":""}"#, "", Some(1)).1, "agy 说查询失败，没给原因");
    }

    #[test]
    fn groups_view() {
        let now = 1795508400.0; // 2026-11-24T08:20:00Z
        let g = quota_groups(&quota(), now).unwrap().as_list().unwrap().items();
        let pro = g[0].as_dict().unwrap();
        assert_eq!(pro.at("name").as_str(), Some("Gemini Pro"));
        assert!(pro.at("description").is_none());
        let b: Vec<Dict> = dicts(&pro, "buckets");
        assert_eq!(b[0].at("utilization").as_num(), Some(13.0));
        assert_eq!(b[0].at("remaining_fraction").as_num(), Some(0.87));
        assert_eq!(b[0].at("resets_at").as_str(), Some("2026-11-24T15:00:00Z"));
        assert_eq!(b[0].at("resets_epoch").as_num(), Some(1795532400.0));
        assert_eq!(b[0].at("reset").as_bool(), Some(false));
        assert_eq!(b[0].at("window").as_str(), Some("5h"));
        // slot：认得的窗口对应 Claude、Codex 的哪一个（App 照它摆），认不出是 null
        assert_eq!((b[0].at("slot").as_str(), b[1].at("slot").as_str()), (Some("five_hour"), Some("seven_day")));
        assert_eq!(b[1].at("utilization").as_num(), Some(95.0));
        let cl: Vec<Dict> = dicts(&g[1].as_dict().unwrap(), "buckets");
        assert_eq!(cl[0].at("utilization").as_num(), Some(0.0));
        assert_eq!(cl[0].at("reset").as_bool(), Some(true)); // 重置时间已经过了
        assert!(cl[1].at("utilization").is_none() && cl[1].at("remaining_amount").as_int() == Some(12));
        assert_eq!(cl[2].at("disabled").as_bool(), Some(true));
        assert!(quota_groups(&Dict::new(), now).is_none());
        assert!(has_buckets(&quota()) && !has_buckets(&Dict::new()));
    }

    #[test]
    fn labels() {
        let l = |g: &str, b: &str| label(&V::from(g), &V::from(b));
        assert_eq!(l("Gemini Pro", "5-hour limit"), "Gemini Pro · 5-hour limit");
        assert_eq!(l("Claude", "Claude"), "Claude");
        assert_eq!(l("Gemini", "Gemini 3 Flash"), "Gemini 3 Flash");
        assert_eq!(l("", "Requests"), "Requests");
        assert_eq!(l("Claude models", ""), "Claude models");
        assert_eq!(label(&V::None, &V::None), "");
        // 认得的窗口写成和 Claude 一样的叫法、不用桶名（1.5.16 真实的桶名是 Five Hour Limit Remaining 这种）
        let names = |rl: &Dict| items(rl, 1795508400.0).unwrap().into_iter().map(|it| it.label).collect::<Vec<_>>();
        assert_eq!(names(&quota()), ["Gemini Pro · 5 小时", "Gemini Pro · 本周", "Claude", "Claude · Requests", "Claude · Off"]);
        let real = json::loads(
            r#"{"groups": [{"name": "Gemini Models", "buckets": [
                {"id": "gemini-weekly", "name": "Weekly Limit Remaining", "window": "weekly", "remaining_fraction": 0.87},
                {"id": "gemini-5h", "name": "Five Hour Limit Remaining", "window": "5h", "remaining_fraction": 0.95}]}]}"#,
        )
        .unwrap()
        .as_dict()
        .unwrap();
        assert_eq!(names(&real), ["Gemini Models · 本周", "Gemini Models · 5 小时"]);
    }

    #[test]
    fn plan_column() {
        let now = 1795508400.0;
        let row = |plan: &str| Row {
            key: "a(alice@example.com)".into(),
            name: "a".into(),
            alias: "a".into(),
            email: "alice@example.com".into(),
            plan: plan.into(),
            source: "live",
            at: Some(now),
            rl: quota(),
            why: V::None,
            msg: V::None,
            outdated_login_plan: String::new(),
        };
        // 都没有套餐（token 里读不到档位）：不要那一列
        let lines = agy_table_lines(&[row("")], now, "100", false).unwrap();
        assert!(!lines[0].contains("套餐") && lines[1].contains("Gemini Pro · 5 小时"), "{lines:?}");
        let lines = agy_table_lines(&[row("Google AI Pro")], now, "100", false).unwrap();
        assert!(lines[0].contains("套餐") && lines[1].contains("Google AI Pro"), "{lines:?}");
    }

    #[test]
    fn week_for_use_first() {
        let now = 1795508400.0; // 2026-11-24T08:20:00Z
        let rl = |groups: &str| json::loads(&format!(r#"{{"groups": {groups}}}"#)).unwrap().as_dict().unwrap();
        // 只有一组：就是它的本周
        let w = agy_week(&quota(), now).unwrap();
        assert_eq!((w.at("utilization").as_num(), w.at("resets_at").as_str()), (Some(95.0), Some("2026-11-28T00:00:00Z")));
        // 几组：用得最少的那个（它也用完了才算用完）；5 小时、认不出窗口、不可用的不算
        let two = rl(r#"[
            {"name": "Gemini", "buckets": [{"window": "weekly", "remaining_fraction": 0.2, "reset_time": "2026-11-25T00:00:00Z"},
                                          {"window": "5h", "remaining_fraction": 1}]},
            {"name": "Claude", "buckets": [{"window": "weekly", "remaining_fraction": 0.6, "reset_time": "2026-11-25T02:00:00Z"},
                                          {"window": "weekly", "disabled": true, "remaining_fraction": 1},
                                          {"name": "x", "remaining_fraction": 1}]}]"#);
        let w = agy_week(&two, now).unwrap();
        assert_eq!((w.at("utilization").as_num(), w.at("resets_at").as_str()), (Some(40.0), Some("2026-11-25T02:00:00Z")));
        // 一样多时先重置的那个；过了重置时间的算 0（use_first 看到重置时间过了就不提示）
        let tie = rl(
            r#"[{"name": "A", "buckets": [{"window": "weekly", "remaining_fraction": 0.5, "reset_time": "2026-11-26T00:00:00Z"}]},
                        {"name": "B", "buckets": [{"window": "weekly", "remaining_fraction": 0.5, "reset_time": "2026-11-25T00:00:00Z"}]}]"#,
        );
        assert_eq!(agy_week(&tie, now).unwrap().at("resets_at").as_str(), Some("2026-11-25T00:00:00Z"));
        let old = rl(
            r#"[{"name": "A", "buckets": [{"window": "weekly", "remaining_fraction": 0.1, "reset_time": "2026-11-24T00:00:00Z"}]},
                        {"name": "B", "buckets": [{"window": "weekly", "remaining_fraction": 0.5, "reset_time": "2026-11-25T00:00:00Z"}]}]"#,
        );
        assert_eq!(agy_week(&old, now).unwrap().at("utilization").as_num(), Some(0.0));
        // 没有本周额度：None
        assert!(agy_week(&rl(r#"[{"name": "A", "buckets": [{"window": "5h", "remaining_fraction": 0.5}]}]"#), now).is_none());
        assert!(agy_week(&Dict::new(), now).is_none());
    }

    #[test]
    fn cell() {
        let now = 1795508400.0;
        // 和 Claude、Codex 那一格一样：各组里同一个窗口用得最多的那个
        assert_eq!(agy_cell(&quota(), now, now).unwrap(), "5h 13% · 周 95%");
        assert_eq!(agy_cell(&quota(), now - 3600.0, now).unwrap(), "5h 13% · 周 95%（1 小时前）");
        // 5 小时那个过了重置时间（2026-11-24T15:00Z）、每周的还没过；都过了（2026-11-28T00:00Z 以后）
        assert_eq!(agy_cell(&quota(), now + 7.0 * 3600.0, now + 7.0 * 3600.0).unwrap(), "5h 已重置 · 周 95%");
        assert_eq!(agy_cell(&quota(), 1795824001.0, 1795824001.0).unwrap(), "5h 已重置 · 周 已重置");
        // 窗口都认不出：写用得最多的那个额度
        let plain = json::loads(r#"{"groups": [{"name": "Claude", "buckets": [{"name": "Claude", "remaining_fraction": 0.4}]}]}"#)
            .unwrap()
            .as_dict()
            .unwrap();
        assert_eq!(agy_cell(&plain, now, now).unwrap(), "Claude 60%");
        assert_eq!(agy_cell(&Dict::new(), now, now).unwrap(), "");
    }

    fn args_of(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn statusline_only_for_interactive_sessions() {
        // 交互的会话：没有参数、开场白、带值的选项（值不算子命令）、-- 后面不是子命令
        let yes: &[&[&str]] = &[
            &[],
            &["--model", "gemini-x"],
            &["fix the bug"],
            &["-i", "update"],
            &["--resume", "help"],
            &["-r=x", "-c"],
            &["--", "hi"],
            &["-"],
        ];
        for a in yes {
            assert!(agy_takes_statusline(&args_of(a)), "{a:?}");
        }
        // 打印模式、版本、帮助、子命令（-- 后面的也算）、自己带了 --settings 的
        let no: &[&[&str]] = &[
            &["-p", "hi"],
            &["-print=hi"],
            &["--prompt", "x"],
            &["--version"],
            &["-h"],
            &["help"],
            &["mcp", "list"],
            &["--model", "x", "update"],
            &["--", "update"],
            &["--settings", "{}"],
            &["-settings={}"],
            &["--model=x", "--print", "hi"],
        ];
        for a in no {
            assert!(!agy_takes_statusline(&args_of(a)), "{a:?}");
        }
    }

    #[test]
    fn turns() {
        let step = |prev: Option<&Turn>, j: &str| turn_step(prev, &json::loads(j).unwrap().as_dict().unwrap());
        let t = step(None, r#"{"agent_state": "working", "conversation_title": "修 bug", "cwd": "/a/w"}"#);
        let first = Turn { state: "working".into(), worked: true, topic: "修 bug".into(), cwd: "/a/w".into(), ..Turn::default() };
        assert_eq!(t, first, "头一次只记下");
        let t = step(Some(&t), r#"{"agent_state": "tool_use", "tool_confirmation_pending": true}"#);
        assert_eq!((t.due, t.mark, t.topic.as_str()), (Some(AgyEvent::Ask), 1, ""), "要你确认：排上在等你；标题照这一次的");
        let t = step(Some(&t), r#"{"agent_state": "tool_use", "tool_confirmation_pending": true}"#);
        assert_eq!((t.due, t.mark), (Some(AgyEvent::Ask), 1), "还在等：不再排");
        let t = step(Some(&t), r#"{"agent_state": "tool_use"}"#);
        assert_eq!((t.due, t.mark), (None, 2), "确认过了：撤掉");
        let t = step(Some(&t), r#"{"agent_state": "idle"}"#);
        assert_eq!((t.due, t.mark, t.worked), (Some(AgyEvent::Done), 3, false), "干完活停下：排上做完了");
        let t = step(Some(&t), r#"{"agent_state": "idle", "context_window": {}}"#);
        assert_eq!((t.due, t.mark), (Some(AgyEvent::Done), 3), "别的变了、状态没变：还是排着的那一条");
        let t = step(Some(&t), r#"{"agent_state": "working"}"#);
        assert_eq!((t.due, t.mark, t.worked), (None, 4, true), "又干活了：撤掉");
        // 停下时后台还有事：不排，干的活留着；都完了再停下才排
        let sub = |s: &str| {
            format!(
                r#""subagents": [{{"name": "a", "role": "r", "status": "completed"}}, {{"name": "b", "role": "r", "status": "{s}"}}]"#
            )
        };
        for bg in [r#""task_count": 1"#.to_string(), r#""pending_input_count": 2"#.to_string(), sub("running")] {
            let held = step(Some(&t), &format!(r#"{{"agent_state": "idle", {bg}}}"#));
            assert_eq!((held.due, held.mark, held.worked), (None, 4, true), "{bg}");
        }
        let done = step(Some(&t), &format!(r#"{{"agent_state": "idle", "task_count": 0, {}}}"#, sub("completed")));
        assert_eq!((done.due, done.mark), (Some(AgyEvent::Done), 5), "子代理都做完了");
        // 排着的做完了：后台又有事了、要你确认了也撤掉，干的活留着
        let held = step(Some(&done), r#"{"agent_state": "idle", "task_count": 1}"#);
        assert_eq!((held.due, held.mark, held.worked), (None, 6, true));
        assert_eq!(step(Some(&held), r#"{"agent_state": "idle"}"#).due, Some(AgyEvent::Done));
        let asked = step(Some(&done), r#"{"agent_state": "idle", "tool_confirmation_pending": true}"#);
        assert_eq!((asked.due, asked.mark, asked.worked), (Some(AgyEvent::Ask), 7, true), "撤掉做完了、排上在等你");
        let t = step(Some(&t), r#"{"agent_state": "error"}"#);
        assert_eq!((t.due, t.mark), (Some(AgyEvent::Failed), 5));
        let t2 = step(Some(&t), r#"{"agent_state": "idle"}"#);
        assert_eq!((t2.due, t2.mark), (Some(AgyEvent::Failed), 5), "出错停下已经排过了");
        let t = step(None, r#"{"agent_state": "idle"}"#);
        assert_eq!((t.due, t.worked), (None, false), "刚启动");
        assert_eq!(step(Some(&t), r#"{"agent_state": "authenticating"}"#).due, None);
        assert_eq!(step(None, r#"{"agent_state": "a b\nc"}"#).state, "abc", "写进文件的只有看得见的 ASCII");
        let long = "长".repeat(AGY_TOPIC_MAX + 5);
        let t = step(None, &format!(r#"{{"agent_state": "idle", "conversation_title": "{long}"}}"#));
        assert_eq!(t.topic.chars().count(), AGY_TOPIC_MAX);
        // 文件里是一行 JSON；3.7.11 及以前写的「状态 0|1 0|1」也认（没有排着的）
        let t = Turn {
            state: "tool_use".into(),
            ask: true,
            worked: true,
            due: Some(AgyEvent::Ask),
            mark: 7,
            topic: "a \"b\"\n".into(),
            cwd: "/x y".into(),
        };
        assert_eq!(Turn::parse(&t.line()), Some(t.clone()));
        assert!(t.line().ends_with("}\n") && t.line().matches('\n').count() == 1, "一行");
        assert_eq!(Turn::parse("idle 0 1\n"), Some(Turn { state: "idle".into(), worked: true, ..Turn::default() }));
        for bad in ["garbage", "{}", r#"{"state": ""}"#, "[1, 2]"] {
            assert_eq!(Turn::parse(bad), None, "{bad}");
        }
        assert_eq!(Turn::parse(r#"{"state": "idle", "due": "later", "mark": -3}"#).map(|t| (t.due, t.mark)), Some((None, 0)));
        assert_eq!(wait_for(AgyEvent::Ask), AGY_ASK_WAIT);
        assert_eq!(wait_for(AgyEvent::Failed), AGY_DONE_WAIT);
    }

    #[test]
    fn queued_and_still_due() {
        let tmp = TemporaryDirectory::new("cca-agy-due.", &crate::py::fs::gettempdir()).unwrap();
        let dir = tmp.name.clone();
        let run = "0123456789abcdef";
        let d = |j: &str| json::loads(j).unwrap().as_dict().unwrap();
        // 换版本以前起的、还开着的会话：文件里是旧的一行，接着认
        crate::py::fs::note_file(&path::join(&dir, AGY_TURNS), &path::join_all(&[&dir, AGY_TURNS, run]), "working 0 1\n", false);
        let q =
            agy_turn(&dir, run, &d(r#"{"agent_state": "idle", "conversation_title": "修 bug", "cwd": "/a/w"}"#)).expect("排上了");
        assert_eq!((q.due, q.mark), (Some(AgyEvent::Done), 1));
        assert_eq!(still_due(&dir, run, 1, AgyEvent::Done).map(|t| t.topic), Some("修 bug".into()), "醒来时还是它：推");
        assert_eq!(still_due(&dir, run, 1, AgyEvent::Ask), None, "种类不对");
        assert_eq!(still_due(&dir, "fedcba9876543210", 1, AgyEvent::Done), None, "别的会话");
        assert!(agy_turn(&dir, run, &d(r#"{"agent_state": "idle", "conversation_title": "修好了"}"#)).is_none(), "没新排上");
        assert_eq!(still_due(&dir, run, 1, AgyEvent::Done).map(|t| t.topic), Some("修好了".into()), "推的时候用最新的标题");
        assert!(agy_turn(&dir, run, &d(r#"{"agent_state": "working"}"#)).is_none(), "撤掉不用推");
        assert_eq!(still_due(&dir, run, 1, AgyEvent::Done), None, "这中间又干活了：不推");
        let again = agy_turn(&dir, run, &d(r#"{"agent_state": "idle"}"#)).expect("再停下：再排上");
        assert_eq!((again.mark, still_due(&dir, run, 3, AgyEvent::Done).is_some()), (3, true));
        assert!(hex_of(run, 16) && !hex_of("0123456789ABCDEF", 16) && !hex_of(run, 32));
    }

    #[test]
    fn turns_need_a_state() {
        let tmp = TemporaryDirectory::new("cca-agy-turns.", &crate::py::fs::gettempdir()).unwrap();
        let dir = tmp.name.clone();
        let turns = path::join(&dir, AGY_TURNS);
        let d = |j: &str| json::loads(j).unwrap().as_dict().unwrap();
        agy_turn(&dir, "0123456789abcdef", &d(r#"{"plan_tier": "x", "tool_confirmation_pending": false}"#));
        assert!(!path::exists(&turns), "没有 agent_state：什么都不记");
        agy_turn(&dir, "0123456789abcdef", &d(r#"{"agent_state": " "}"#));
        assert!(!path::exists(&turns), "agent_state 里没有看得见的字也一样");
        agy_turn(&dir, "not-a-nonce", &d(r#"{"agent_state": "working"}"#));
        assert!(!path::exists(&turns), "会话标记不对：不记");
        agy_turn(&dir, "0123456789abcdef", &d(r#"{"agent_state": "working"}"#));
        let file = path::join(&turns, "0123456789abcdef");
        let got = Turn::parse(&std::fs::read_to_string(&file).unwrap());
        assert_eq!(got, Some(Turn { state: "working".into(), worked: true, ..Turn::default() }));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&turns).unwrap().permissions().mode() & 0o777, 0o700);
    }

    #[test]
    fn plan_noted_refreshed_and_adopted() {
        let tmp = TemporaryDirectory::new("cca-agy-plan.", &crate::py::fs::gettempdir()).unwrap();
        let dir = tmp.name.clone();
        let file = path::join(&dir, AGY_PLAN_FILE);
        let rec = || load_quiet(&file).and_then(|v| v.as_dict()).expect("记下了");
        let at_of = |d: &Dict| d.get("at").and_then(|a| a.as_num()).expect("有 at");
        let put = |j: String| std::fs::write(&file, j).unwrap();
        let line = |j: &str| json::loads(j).unwrap().as_dict().unwrap();
        let ultra = line(r#"{"plan_tier": " Google AI Ultra ", "email": "Alice@example.com"}"#);
        plan_note(&dir, &ultra);
        let first = rec();
        assert_eq!((first.str("plan_tier").as_deref(), first.has("from")), (Some("Google AI Ultra"), false));
        assert_eq!(agy_plan_own(&dir, "alice@example.com"), Some(("Google AI Ultra".to_string(), at_of(&first))), "邮箱大小写不论");
        assert_eq!(agy_plan_own(&dir, "bob@example.com"), None, "别的账号的不传");
        // 一样的、刚记下的不重写；记下一小时以上的重写：at 是最近一次看到它的时间
        let now = time() as i64;
        put(format!(r#"{{"email": "Alice@example.com", "plan_tier": "Google AI Ultra", "at": {}}}"#, now - 60));
        plan_note(&dir, &ultra);
        assert_eq!(at_of(&rec()), (now - 60) as f64, "刚记下的");
        put(format!(r#"{{"email": "Alice@example.com", "plan_tier": "Google AI Ultra", "at": {}}}"#, now - 7200));
        plan_note(&dir, &ultra);
        let own_at = at_of(&rec());
        assert!(own_at >= now as f64, "记下两小时了：重写");
        // 别的设备的：比这台设备记的旧的、套餐一样的不要；新的、不一样的记下来，标着那台设备，不再当这台设备自己的传
        assert!(!agy_plan_adopt(&dir, "alice@example.com", "Google AI Pro", own_at - 1.0, "bob-mini"), "旧的");
        assert!(!agy_plan_adopt(&dir, "alice@example.com", "Google AI Ultra", own_at + 100.0, "bob-mini"), "套餐一样");
        assert!(agy_plan_adopt(&dir, "Alice@example.com", "Google AI Pro", own_at + 100.0, "bob-mini"));
        let got = rec();
        assert_eq!(
            (got.str("plan_tier").as_deref(), got.str("from").as_deref(), got.str("email").as_deref(), at_of(&got)),
            (Some("Google AI Pro"), Some("bob-mini"), Some("alice@example.com"), own_at + 100.0)
        );
        assert_eq!(agy_plan_own(&dir, "alice@example.com"), None, "拿来的不再传给别的设备");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        // 这台设备自己又看到了：换成自己的（套餐一样也换）
        plan_note(&dir, &line(r#"{"plan_tier": "Google AI Pro", "email": "alice@example.com"}"#));
        assert_eq!(
            (rec().has("from"), agy_plan_own(&dir, "alice@example.com").map(|p| p.0)),
            (false, Some("Google AI Pro".into()))
        );
        // 记的是别的邮箱的（换过账号登录）：不管新旧都换成这个账号的
        put(r#"{"email": "bob@example.com", "plan_tier": "Google AI Ultra", "at": 9999999999}"#.to_string());
        assert!(agy_plan_adopt(&dir, "alice@example.com", "Google AI Pro", 1.0, "bob-mini"));
        assert_eq!(rec().str("email").as_deref(), Some("alice@example.com"));
        // 账号目录不在（这台设备上没用过这个账号）：不建
        let gone = path::join(&dir, "gone");
        assert!(!agy_plan_adopt(&gone, "alice@example.com", "Google AI Pro", 1.0, "bob-mini"));
        assert!(!path::exists(&gone));
    }

    #[test]
    fn statusline_chains_the_users_own() {
        let base = "/x/cca _agy-statusline alice-1";
        let own = r#"{"type": "command", "command": "/x/cca _agy-statusline alice-1", "enabled": true, "padding": 0, "stack_with_default": true}"#;
        let line = |user: Option<&str>| {
            let v = user.map(|u| json::loads(u).unwrap());
            statusline_for(v.as_ref(), base).map(|d| json::dumps(&V::Dict(d), json::LINE))
        };
        // 用户没配、停用了、命令是空的、是 cca 自己的：只有 cca 的，叠在 agy 自己的状态栏上
        for u in [
            None,
            Some("true"),
            Some("{}"),
            Some(r#"{"command": "~/sl.sh", "enabled": false}"#),
            Some(r#"{"command": "  "}"#),
            Some(r#"{"command": "'/y/cca' _agy-statusline bob-2"}"#),
        ] {
            assert_eq!(line(u).as_deref(), Some(own), "{u:?}");
        }
        // 用户的命令接在后面（照 shlex.quote 引起来）：padding 这些都留着，type、enabled 补上
        assert_eq!(
            line(Some(r#"{"command": "~/sl.sh 'a b'", "padding": 2, "stack_with_default": false}"#)).as_deref(),
            Some(
                r#"{"command": "/x/cca _agy-statusline alice-1 --then '~/sl.sh '\"'\"'a b'\"'\"''", "padding": 2, "stack_with_default": false, "type": "command", "enabled": true}"#
            )
        );
        assert_eq!(
            line(Some(r#"{"type": null, "command": "x"}"#)).as_deref(),
            Some(r#"{"type": "command", "command": "/x/cca _agy-statusline alice-1 --then x", "enabled": true}"#)
        );
        // 别的 type（不知道是什么）不动
        assert_eq!(line(Some(r#"{"type": "builtin", "command": "x"}"#)), None);
        // 原来的那一份不改
        let u = json::loads(r#"{"command": "x"}"#).unwrap();
        assert!(statusline_for(Some(&u), base).is_some());
        assert_eq!(json::dumps(&u, json::LINE), r#"{"command": "x"}"#);
    }
}
