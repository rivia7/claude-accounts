//! 状态栏：cca 启动的 Claude Code 交互会话里状态栏的命令（`cca _claude-statusline`），和 agy 的（agy.rs 的 `cca _agy-statusline`）
//! 共用的几样。
//!
//! cca 不主动查 Claude、agy 的用量：不用打印模式（`claude -p`、`agy -p` cca 自己不用，`py launch-check` 也不让经 cca 用）。
//! Claude Code、agy 每次状态变了都起一次状态栏的命令，JSON 从 stdin 进，里面有这个账号现在的额度（Claude Code 的 rate_limits、
//! agy 的 quota）：cca 照它把用量记进账号目录的 usage.json（usage.rs 的 `usage_note`），`cca usage`、菜单、App 都读这一份。
//! 所以只有经 cca 启动、用过的账号有用量，最近没用的显示上次记下的。用户自己的状态栏命令接在 cca 的后面照样运行（`--then`），
//! 它的输出就是状态栏。用户没配自己的：Claude Code 2.1.332 只要配了状态栏命令就留出那一行，命令什么都不输出也留（2026-11-27
//! 离线用真的程序量过），所以 cca 在那一行写账号和记下的用量（`claude_status_text`）；agy 的什么都不输出（没登录时的界面上不多
//! 一行，登录以后的没量过）。

use super::settings::CLAUDE_VALUES;
use super::usage::{iso, py_round, span, usage_note};
use super::{arity, Cmd};
use crate::core::base::SELF_PATH;
use crate::core::model::{acct_id, cdir, is_api, parse_key, tables_load};
use crate::core::tool::{claude, t, Tool};
use crate::dict;
use crate::py::json;
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::proc::Cmd as Proc;
use crate::py::text::{self, sh_quote};
use crate::py::time;
use crate::py::{die, exit, R};
use crate::{out_raw, re};
use std::io::Read;

/// 这个模块的子命令
pub const CMDS: &[Cmd] = &[Cmd::all("claude-statusline", cmd_claude_statusline)];

/// Claude Code 2.1 的子命令（2.1.332 的程序里登记在最上一层的）：选项后面第一个不是选项的参数是它们之一时不开会话；别的是提示词
const CLAUDE_SUBCOMMANDS: &[&str] = &[
    "agents",
    "auth",
    "auto-mode",
    "doctor",
    "gateway",
    "import",
    "install",
    "mcp",
    "plugin",
    "plugins",
    "remote-control",
    "sandbox",
    "setup-token",
    "ultrareview",
    "update",
    "upgrade",
];

/// 这次启动是不是 Claude Code 的交互会话（只有它画状态栏）：打印模式（-p、--print，launch-check 已经不让用了）、--version、--help、
/// 子命令不是；自己带了 --settings 的也不加（Claude Code 只认后写的那个 --settings，cca 的这一份反正会被顶掉）。短选项可以几个
/// 写在一起（-cp）；-- 后面都是提示词
pub fn claude_takes_statusline(args: &[String]) -> bool {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--" {
            return true;
        }
        if let Some(long) = a.strip_prefix("--") {
            let (name, inline) = long.split_once('=').map_or((long, false), |(n, _)| (n, true));
            if ["version", "help", "print", "settings"].contains(&name) {
                return false;
            }
            if !inline && CLAUDE_VALUES.iter().any(|v| v.strip_prefix("--") == Some(name)) {
                it.next();
            }
            continue;
        }
        if let Some(short) = a.strip_prefix('-').filter(|s| !s.is_empty()) {
            if short.contains(['v', 'h', 'p']) {
                return false;
            }
            if CLAUDE_VALUES.contains(&a.as_str()) {
                it.next();
            }
            continue;
        }
        return !CLAUDE_SUBCOMMANDS.contains(&a.as_str());
    }
    true
}

/// 交给 Claude Code 的 statusLine（`claude_launch_args` 放进 --settings，只这一次、不保存）：官方账号交互的会话才有。
/// 用户自己配了状态栏命令时（这个目录的 .claude/settings.local.json、.claude/settings.json，再是账号的 settings.json——也就是
/// 共享的那一份：先找到的算，和 Claude Code 的先后一样；托管的设置比 --settings 先，那时 cca 的这一条不起作用）照用户的那一份
/// （padding、refreshInterval 这些都留着），命令换成 cca 的、`--then` 接着运行用户的。读不了、不是 JSON 对象的文件不知道用户配了
/// 什么：不加（别把用户的状态栏盖掉）；type 不是 command 的不动它
pub fn claude_statusline(key: &str, args: &[String]) -> Option<Dict> {
    if !claude_takes_statusline(args) {
        return None;
    }
    let cwd = std::env::current_dir().ok()?.to_string_lossy().into_owned();
    let files = [
        path::join_all(&[&cwd, ".claude", "settings.local.json"]),
        path::join_all(&[&cwd, ".claude", "settings.json"]),
        path::join(&cdir(key), "settings.json"),
    ];
    let mut user = None;
    for f in &files {
        let raw = match std::fs::read(f) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        };
        let d = json::loads_file_bytes(&raw).ok()?.as_dict()?;
        if let Some(line) = d.dict("statusLine") {
            user = Some(line);
            break;
        }
    }
    let base = format!("{} _claude-statusline {}", sh_quote(&SELF_PATH), sh_quote(&acct_id(key, t())));
    let own = || dict! { "type" => "command", "command" => base.as_str() };
    let Some(u) = user else { return Some(own()) };
    if u.get("type").is_some_and(|x| !matches!(x, V::None) && x.as_str() != Some("command")) {
        return None;
    }
    let cmd = u.str("command").map(|c| text::strip(&c).to_string()).unwrap_or_default();
    if cmd.is_empty() || cmd.contains("_claude-statusline") {
        return Some(own());
    }
    let line = u.copy();
    line.set("type", "command");
    line.set("command", format!("{base} --then {}", sh_quote(&cmd)));
    Some(line)
}

/// 状态栏的命令收到的：账号目录（id 要像账号目录名、目录要在：只往 cca 自己的账号目录里写）和状态栏的 JSON（最多 256 KB）
pub fn statusline_input(tool: &Tool, id: &str, raw: &[u8]) -> Option<(String, Dict)> {
    if raw.len() > 256 << 10 || re!(r"[a-z0-9][a-z0-9-]{0,63}").fullmatch(id).is_none() {
        return None;
    }
    let dir = path::join_all(&[&tool.sroot, "accounts", id]);
    let d = json::loads_file_bytes(raw).ok()?.as_dict()?;
    path::isdir(&dir).then_some((dir, d))
}

/// 状态栏的命令读进来的 JSON（stdin 是终端时不读：手动运行时不等输入）。第二个值是 stdin 是不是终端
pub fn statusline_stdin() -> (Vec<u8>, bool) {
    // SAFETY: 只是问 fd 0 是不是终端
    let tty = unsafe { libc::isatty(0) } == 1;
    let mut raw = Vec::new();
    if !tty {
        let _ = std::io::stdin().lock().read_to_end(&mut raw);
    }
    (raw, tty)
}

/// `--then` 带着的用户自己的状态栏命令：用同一份 JSON 接着运行它（Claude Code 用 bash 起状态栏的命令，agy 用 sh），
/// 它的输出和退出码就是这一次的（照它画状态栏）。没有就什么都不做
pub fn run_then(then: Option<&String>, shell: &str, raw: Vec<u8>, tty: bool) -> R<()> {
    let Some(cmd) = then else { return Ok(()) };
    let p = Proc::new([shell, "-c", cmd.as_str()]);
    match if tty { p } else { p.input(raw) }.run() {
        Ok(d) if d.rc == 0 => Ok(()),
        Ok(d) => exit(if d.rc < 0 { 128 - d.rc } else { d.rc }),
        Err(_) => exit(127),
    }
}

/// 状态栏 JSON 里的 rate_limits（five_hour、seven_day：used_percentage 是 0–100 的百分比（一位小数），resets_at 是 Unix 秒；
/// 订阅的账号收到第一个回应以后才有）→ usage.json 里的用量窗口（和 /api/oauth/usage 的一样：utilization 是百分比，resets_at 是
/// ISO 时间）。网关的 spend_limit 不要（那是网关定的花费上限，不是订阅的额度）。一个窗口也没有是 None
pub fn claude_rate_limits(d: &Dict) -> Option<Dict> {
    let rl = d.dict("rate_limits")?;
    let out = Dict::new();
    for k in ["five_hour", "seven_day"] {
        let Some(w) = rl.dict(k) else { continue };
        let Some(pct) = used_pct(&w) else { continue };
        let at = w.get("resets_at").and_then(|x| x.as_num()).filter(|x| x.is_finite() && *x > 0.0).and_then(|x| iso(x).ok());
        out.set(k, dict! { "utilization" => pct, "resets_at" => at.map_or(V::None, V::from) });
    }
    (!out.is_empty()).then_some(out)
}

/// 状态栏 JSON 里一个窗口的 used_percentage：整数或者有限的小数，别的（null、布尔值、字符串）不认
fn used_pct(w: &Dict) -> Option<V> {
    w.get("used_percentage").filter(|p| p.is_int() || matches!(p, V::Float(f) if f.is_finite()))
}

/// 用户没配自己的状态栏时 cca 写在状态栏那一行的（Claude Code 配了状态栏命令就留出这一行，空着不如写上）：哪个账号（name）和
/// 这个会话里记下的用量，`alice · 5 小时 13%（2 小时 13 分后重置）· 本周 41%`。百分比照 cca usage 的表格取整；本周的不写重置
/// 时间，免得太长（放不下时 Claude Code 截掉后面的）。这个会话还没有回应（JSON 里没有 rate_limits）、窗口认不出时只有账号
pub fn claude_status_text(name: &str, d: &Dict, now: f64) -> R<String> {
    let rl = d.dict("rate_limits").unwrap_or_default();
    let mut line = name.to_string();
    for (k, label) in [("five_hour", "5 小时"), ("seven_day", "本周")] {
        let Some(w) = rl.dict(k) else { continue };
        let Some(pct) = used_pct(&w) else { continue };
        // 全角的括号自带空白：「）」后面直接接「·」
        line.push_str(if line.ends_with('）') { "· " } else { " · " });
        line.push_str(&format!("{label} {}%", py_round(&pct)?.text));
        let at = w.get("resets_at").and_then(|x| x.as_num()).filter(|at| at.is_finite() && *at > now);
        if let (Some(at), "five_hour") = (at, k) {
            line.push_str(&format!("（{}后重置）", span(at - now)?));
        }
    }
    Ok(line)
}

/// 账号目录名 → 状态栏上写的账号：有别名写别名，没有写邮箱。账号表里没有这个官方账号（删掉了）、读不了账号表时是 None
fn status_name(id: &str) -> Option<String> {
    tables_load().ok()?;
    let table = t().tab()?;
    let key = table.keys().into_iter().find(|k| acct_id(k, t()) == id && !is_api(&table.at(k)))?;
    let (alias, email) = parse_key(&key);
    Some(if alias.is_empty() { email } else { alias })
}

/// `cca _claude-statusline 账号目录名 [--then 命令]`：cca 启动的 Claude Code 交互会话里状态栏的命令（`claude_statusline` 经
/// --settings 交给它）。Claude Code 每次状态变了都用 bash 起它，JSON 从 stdin 进：里面的 rate_limits 记进账号的 usage.json
/// （`claude_rate_limits` → usage.rs 的 `usage_note`；这个会话的请求计数是 cost.total_api_duration_ms：有了新的回应才变）。
/// --then 是用户自己的状态栏命令（`run_then`：它的输出就是状态栏）；没有时输出那一行的字（`claude_status_text`）。出什么错都不说
fn cmd_claude_statusline(args: &[String]) -> R<()> {
    arity("cmd_claude_statusline", args, 1, 3)?;
    let (id, then) = match args {
        [id] => (id, None),
        [id, flag, cmd] if flag == "--then" => (id, Some(cmd)),
        _ => return die("用法：cca _claude-statusline 账号目录名 [--then 命令]"),
    };
    let (raw, tty) = statusline_stdin();
    let input = statusline_input(claude(), id, &raw);
    if let Some((dir, d)) = &input {
        if let Some(rl) = claude_rate_limits(d) {
            let session = d.str("session_id").map(|s| s.to_string()).unwrap_or_default();
            let mark = d.dict("cost").and_then(|c| c.get("total_api_duration_ms")).map_or_else(String::new, |v| v.py_str());
            usage_note(id, dir, &rl, Some((&session, &mark)), None, true);
        }
    }
    if then.is_some() {
        return run_then(then, "bash", raw, tty);
    }
    if let Some((_, d)) = &input {
        // 不带换行（Claude Code 照样去掉两头的空白；假 claude 照原样记下输出）
        if let Some(line) = status_name(id).and_then(|name| claude_status_text(&name, d, time::time()).ok()) {
            out_raw!("{line}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn interactive_or_not() {
        for yes in [&[][..], &["--model", "opus"], &["看看这个"], &["-c"], &["--resume", "x"], &["--", "-p"], &["--model", "mcp"]]
        {
            assert!(claude_takes_statusline(&s(yes)), "{yes:?}");
        }
        for no in [
            &["-p", "hi"][..],
            &["--print", "hi"],
            &["-cp", "hi"],
            &["--version"],
            &["-v"],
            &["-h"],
            &["--help"],
            &["mcp", "list"],
            &["--model", "opus", "doctor"],
            &["update"],
            &["--settings", "{}"],
            &["--settings={}"],
        ] {
            assert!(!claude_takes_statusline(&s(no)), "{no:?}");
        }
    }

    #[test]
    fn rate_limits_to_windows() {
        let d = json::loads(
            r#"{"rate_limits": {"five_hour": {"used_percentage": 13.4, "resets_at": 1795526400},
                                "seven_day": {"used_percentage": 41, "resets_at": 1795920000},
                                "spend_limit": {"used_percentage": 5, "resets_at": 1795920000}}}"#,
        )
        .unwrap()
        .as_dict()
        .unwrap();
        let rl = claude_rate_limits(&d).unwrap();
        assert_eq!(rl.keys().len(), 2);
        assert_eq!(rl.dict("five_hour").unwrap().at("utilization").as_num(), Some(13.4));
        assert_eq!(rl.dict("five_hour").unwrap().at("resets_at").as_str(), Some("2026-11-24T13:20:00Z"));
        assert_eq!(rl.dict("seven_day").unwrap().at("utilization").as_num(), Some(41.0));
        // 没有百分比的窗口不要；一个窗口也没有（还没收到回应、按量计费的账号）是 None
        let none = json::loads(r#"{"rate_limits": {"five_hour": {"resets_at": 1}}}"#).unwrap().as_dict().unwrap();
        assert!(claude_rate_limits(&none).is_none());
        assert!(claude_rate_limits(&Dict::new()).is_none());
    }

    #[test]
    fn status_line_text() {
        let now = 1_795_500_000.0;
        let text = |name: &str, raw: &str| claude_status_text(name, &json::loads(raw).unwrap().as_dict().unwrap(), now).unwrap();
        // 这个会话还没有回应：只有账号
        assert_eq!(text("alice@example.com", r#"{"session_id": "s"}"#), "alice@example.com");
        // 5 小时的写还有多久重置（全角括号后面直接接「·」），本周的不写；百分比照表格取整（四舍六入五成双）
        let both = r#"{"rate_limits": {"five_hour": {"used_percentage": 13.4, "resets_at": 1795508010},
                                       "seven_day": {"used_percentage": 41, "resets_at": 1795777200}}}"#;
        assert_eq!(text("alice", both), "alice · 5 小时 13%（2 小时 13 分后重置）· 本周 41%");
        // 没有 5 小时限制的套餐：只有本周
        assert_eq!(text("工作", r#"{"rate_limits": {"seven_day": {"used_percentage": 41.6}}}"#), "工作 · 本周 42%");
        // 重置时间过了、没有：不写；认不出的窗口不写
        let past = r#"{"rate_limits": {"five_hour": {"used_percentage": 99, "resets_at": 1795499999}, "seven_day": "x"}}"#;
        assert_eq!(text("alice", past), "alice · 5 小时 99%");
        let bad = r#"{"rate_limits": {"five_hour": {"used_percentage": null}, "seven_day": {"used_percentage": true}}}"#;
        assert_eq!(text("alice", bad), "alice");
        // 不到一小时
        let soon = r#"{"rate_limits": {"five_hour": {"used_percentage": 98.5, "resets_at": 1795501500}}}"#;
        assert_eq!(text("alice", soon), "alice · 5 小时 98%（25 分钟后重置）");
    }
}
