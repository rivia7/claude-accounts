//! 状态栏：cca 启动的 Claude Code 交互会话里状态栏的命令（`cca _claude-statusline`），和 agy 的（agy.rs 的 `cca _agy-statusline`）
//! 共用的几样。
//!
//! cca 不主动查 Claude、agy 的用量：不用打印模式（`claude -p`、`agy -p` cca 自己不用，`py launch-check` 也不让经 cca 用）。
//! Claude Code、agy 每次状态变了都起一次状态栏的命令，JSON 从 stdin 进，里面有这个账号现在的额度（Claude Code 的 rate_limits、
//! agy 的 quota）：cca 照它把用量记进账号目录的 usage.json（usage.rs 的 `usage_note`），`cca usage`、菜单、App 都读这一份。
//! 所以只有经 cca 启动、用过的账号有用量，最近没用的显示上次记下的。用户自己的状态栏命令接在 cca 的后面照样运行（`--then`），
//! 它的输出就是状态栏；cca 自己什么都不输出（没有输出时 Claude Code 不画状态栏那一行）。

use super::settings::CLAUDE_VALUES;
use super::usage::{iso, usage_note};
use super::{arity, Cmd};
use crate::core::base::SELF_PATH;
use crate::core::model::{acct_id, cdir};
use crate::core::tool::{claude, t, Tool};
use crate::dict;
use crate::py::json;
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::proc::Cmd as Proc;
use crate::py::text::{self, sh_quote};
use crate::py::{die, exit, R};
use crate::re;
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
        let Some(pct) = w.get("used_percentage").filter(|p| p.is_int() || matches!(p, V::Float(f) if f.is_finite())) else {
            continue;
        };
        let at = w.get("resets_at").and_then(|x| x.as_num()).filter(|x| x.is_finite() && *x > 0.0).and_then(|x| iso(x).ok());
        out.set(k, dict! { "utilization" => pct, "resets_at" => at.map_or(V::None, V::from) });
    }
    (!out.is_empty()).then_some(out)
}

/// `cca _claude-statusline 账号目录名 [--then 命令]`：cca 启动的 Claude Code 交互会话里状态栏的命令（`claude_statusline` 经
/// --settings 交给它）。Claude Code 每次状态变了都用 bash 起它，JSON 从 stdin 进：里面的 rate_limits 记进账号的 usage.json
/// （`claude_rate_limits` → usage.rs 的 `usage_note`；这个会话的请求计数是 cost.total_api_duration_ms：有了新的回应才变），
/// 自己什么都不输出。--then 是用户自己的状态栏命令（`run_then`）
fn cmd_claude_statusline(args: &[String]) -> R<()> {
    arity("cmd_claude_statusline", args, 1, 3)?;
    let (id, then) = match args {
        [id] => (id, None),
        [id, flag, cmd] if flag == "--then" => (id, Some(cmd)),
        _ => return die("用法：cca _claude-statusline 账号目录名 [--then 命令]"),
    };
    let (raw, tty) = statusline_stdin();
    if let Some((dir, d)) = statusline_input(claude(), id, &raw) {
        if let Some(rl) = claude_rate_limits(&d) {
            let session = d.str("session_id").map(|s| s.to_string()).unwrap_or_default();
            let mark = d.dict("cost").and_then(|c| c.get("total_api_duration_ms")).map_or_else(String::new, |v| v.py_str());
            usage_note(id, &dir, &rl, Some((&session, &mark)), None, true);
        }
    }
    run_then(then, "bash", raw, tty)
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
}
