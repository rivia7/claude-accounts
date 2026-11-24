//! Antigravity（agy）的额度（`cca agy usage`）：agy 1.1.11 起，打印模式里的 /usage 不经模型、直接回额度：
//! `agy --gemini_dir=账号目录 -p /usage --output-format json`，回的 JSON 里 `command.data` 是 RetrieveUserQuotaSummary
//! 的结果（1.5.16 的 printmode.quotaJSON：`{description, groups: [{name, description, buckets: [{id, name, description, window,
//! disabled, remaining_fraction, remaining_amount, reset_time}]}]}`，reset_time 是 UTC 的 RFC 3339）。登录 token 由 agy 自己读取
//! 和刷新，cca 不碰。这里有查询（agy 账号的 usage-query 走到这里）、`--json` 里的 quota_groups（macos/PROTOCOL.md 3.1）、表格和菜单里
//! 那一格。查到的原样存进 usage.json（usage_save），显示的时候再算用了多少、过没过重置时间。

use super::login::sys::{interrupted, trap};
use super::usage::{ago, cols_of, dicts, float_arg, parse_ts, py_round, reset_short, usage_why, Row, WhyTable, USAGE_STALE};
use crate::core::model::cdir;
use crate::core::tool::AGY_FILE_ENV;
use crate::py::json;
use crate::py::obj::{Dict, V};
use crate::py::proc::{self, Cmd as Proc, In, Out, ProcError};
use crate::py::shutil::TemporaryDirectory;
use crate::py::text::{self, fit};
use crate::py::time::monotonic;
use crate::py::uni;
use crate::py::{die, exit, R};
use crate::{dict, out};
use std::os::fd::{AsRawFd, RawFd};

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
/// 另加和别的工具的用量窗口一样的四项：utilization（用了百分之几：100 × (1 − remaining_fraction)，两位小数）、resets_at
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
const WINDOWS: [(&str, &str, &str); 2] = [("5h", "5 小时", "5h"), ("weekly", "本周", "周")];

/// 桶的 window 是 WINDOWS 里的哪一种（认不出是 None）
fn window_of(b: &Dict) -> Option<usize> {
    let w = b.str("window")?;
    let w = text::strip(&w).to_ascii_lowercase();
    WINDOWS.iter().position(|(k, _, _)| *k == w)
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
    for (i, (_, _, short)) in WINDOWS.iter().enumerate() {
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
    let old = if now - at >= USAGE_STALE { format!("（{}前）", ago(now - at)?) } else { String::new() };
    Ok(format!("{}{}", cells.join(" · "), old))
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
}
