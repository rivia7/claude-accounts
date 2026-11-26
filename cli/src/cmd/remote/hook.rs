//! cca 自己带的 Claude Code 钩子（`remote-hook`）：远程会话开着时（`cca remote on`、这一次 `CCA_REMOTE=1`），cca 启动 Claude Code
//! 时经 `--settings` 带上 Stop、PermissionRequest、Notification 三个钩子（`hooks`，accounts.rs 的 `claude_hooks` 把它和记下会话的
//! SessionStart 并在一起），回调 `cca _remote-hook`：这一轮做完了、停下来等你回答时推一条通知（`NOTIFY_REQ`）给接着这个会话的
//! 各处——别的机器上的终端、cca App，手机锁着屏时经推送（docs/dev/push.md）。
//!
//! 照维护者原来自己写的那两个钩子（docs/dev/remote.md「cca 自己带的钩子」）：
//! - Stop：还有后台任务在跑（`background_tasks`，Claude Code 自己的 dream、auto_mode_scan 不算）、到点会醒来（`session_crons`）时
//!   不说（做完了会叫醒会话，那一轮结束时再来一次 Stop）；正文是最后一条回复的第一行
//! - PermissionRequest（提问、确认一个操作、批准计划）：这时不说，把要说的那一句记下来；6 秒后还没人答，Claude Code 自己发
//!   Notification（`permission_prompt`，只有一句 "Claude needs your permission"），那时用记下的那一句推。马上答了的不打扰
//! - 别的在等你的 Notification（MCP 服务器要你填的、后台的会话要你回答的……）照 Claude Code 的原话
//!
//! 不在会话宿主里（这次没放进宿主：图形界面里启动的、不是在终端里）什么都不做。总是成功、什么都不打印：Stop 的钩子退出码 2
//! 会拦着 Claude Code 不让它停，stdout 在有的钩子里会进 Claude 的上下文

use super::local::{self, Note};
use super::manage::{host_of_me, notify_host};
use crate::core::base::SELF_PATH;
use crate::dict;
use crate::py::fs;
use crate::py::json;
use crate::py::obj::{Dict, V};
use crate::py::proc::getenv;
use crate::py::text::sh_quote;
use crate::py::{path, R};
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

/// Notification 里认的几种（idle_prompt 不认：做完了在等下一句，Stop 那一条已经说了）
pub const MATCHER: &str = "permission_prompt|elicitation_dialog|elicitation_url_dialog|agent_needs_input|worker_permission_prompt";
/// 钩子最多跑多久（秒，Claude Code 的 timeout）
const TIMEOUT: i64 = 10;
/// PermissionRequest 记下的那一句多久以内算刚记下的（Notification 在它之后 6 秒来）
const ASK_FRESH: f64 = 120.0;
/// 一行话最长几个字（多的截掉，加一个“…”）
const CLIP: usize = 110;
/// 从 stdin 最多读这么多（Stop 的输入里有最后一条回复）
const INPUT_MAX: u64 = 8 << 20;

/// 交给 Claude Code 的 `hooks` 里的这三项：回调 `cca _remote-hook`
pub fn hooks() -> Dict {
    let hook = || {
        crate::list![
            dict! { "type" => "command", "command" => format!("{} _remote-hook", sh_quote(&SELF_PATH)), "timeout" => TIMEOUT }
        ]
    };
    dict! {
        "Stop" => crate::list![dict! { "hooks" => hook() }],
        "PermissionRequest" => crate::list![dict! { "hooks" => hook() }],
        "Notification" => crate::list![dict! { "matcher" => MATCHER, "hooks" => hook() }],
    }
}

/// 一行话：第一行不空的，去掉 Markdown 的记号（*、`、#、>、|）和开头的空白、横线，结尾的空白
fn line(v: &V) -> String {
    let s = match v {
        V::Str(s) => s.to_string(),
        V::None => String::new(),
        other => json::dumps(other, json::Fmt::new().utf8().compact()),
    };
    for raw in s.split('\n') {
        let t: String = raw.chars().filter(|c| !matches!(c, '*' | '`' | '#' | '>' | '|')).collect();
        let t = t.trim_start_matches(|c: char| c.is_whitespace() || c == '-').trim_end();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    String::new()
}

/// 太长的截掉
fn clip(s: &str) -> String {
    if s.chars().count() > CLIP {
        s.chars().take(CLIP).collect::<String>() + "…"
    } else {
        s.to_string()
    }
}

/// 路径的最后一截（`${x##*/}`）
fn base(s: &str) -> String {
    s.rsplit('/').next().unwrap_or_default().to_string()
}

/// 有值的头一个（null、false 不算，和 jq 的 `//` 一样）；都没有是 dflt
fn pick(d: &Dict, keys: &[&str], dflt: &str) -> V {
    keys.iter().filter_map(|k| d.get(k)).find(|v| !matches!(v, V::None | V::Bool(false))).unwrap_or_else(|| V::str(dflt))
}

/// PermissionRequest：标题和正文（问的是什么、要确认的是哪个操作）
fn ask_text(d: &Dict) -> (String, String) {
    let input = d.dict("tool_input").unwrap_or_default();
    let confirm = "Claude Code 要你确认".to_string();
    let tool = d.get("tool_name").filter(|v| !matches!(v, V::None | V::Bool(false)));
    let name = tool.as_ref().map(|t| t.as_str().map_or_else(|| line(t), str::to_string));
    match name.as_deref() {
        Some("AskUserQuestion") => {
            let qs = input.list("questions").unwrap_or_default();
            let first = qs
                .get(0)
                .and_then(|q| q.as_dict())
                .map_or_else(|| V::str("有问题要你回答"), |q| pick(&q, &["question"], "有问题要你回答"));
            let more = if qs.len() > 1 { format!("（一共 {} 个问题）", qs.len()) } else { String::new() };
            ("Claude Code 在问你".into(), clip(&line(&first)) + &more)
        }
        Some("ExitPlanMode") => ("Claude Code 等你批准计划".into(), clip(&line(&pick(&input, &["plan"], "计划写好了")))),
        Some("Bash") => (confirm, format!("运行：{}", clip(&line(&pick(&input, &["description", "command"], "命令"))))),
        Some("Write") => (confirm, format!("写文件：{}", base(&pick(&input, &["file_path"], "").py_str()))),
        Some("Edit" | "MultiEdit" | "NotebookEdit") => {
            (confirm, format!("改文件：{}", base(&pick(&input, &["file_path", "notebook_path"], "").py_str())))
        }
        Some("WebFetch") => (confirm, format!("访问：{}", clip(&line(&pick(&input, &["url"], ""))))),
        Some("WebSearch") => (confirm, format!("搜索：{}", clip(&line(&pick(&input, &["query"], ""))))),
        Some(t) if t.starts_with("mcp__") => {
            let parts: Vec<&str> = t.split("__").collect();
            let server = parts.get(1).copied().unwrap_or("null");
            (confirm, format!("用 {} 的 {}", server, parts.get(2..).map_or(String::new(), |p| p.join("__"))))
        }
        Some(t) => (confirm, format!("用 {t}")),
        None => (confirm, "用 一个工具".into()),
    }
}

/// Stop：还有后台任务在跑、到点会醒来时是 None；不然是正文（最后一条回复的第一行）
fn done_text(d: &Dict) -> Option<String> {
    let tasks = d.list("background_tasks").map_or(0, |l| {
        l.items()
            .iter()
            .filter(|t| !matches!(t.as_dict().and_then(|t| t.str("type")).as_deref(), Some("dream" | "auto_mode_scan")))
            .count()
    });
    let crons = d.list("session_crons").map_or(0, |l| l.len());
    if tasks + crons > 0 {
        return None;
    }
    let body = clip(&line(&d.get("last_assistant_message").unwrap_or(V::None)));
    Some(if body.is_empty() { "这一轮做完了".into() } else { body })
}

/// PermissionRequest 记下的那一句放在哪：会话目录下的 ask/（700），一个 Claude Code 会话一个文件（600）
fn ask_file(session: &str) -> Option<String> {
    let id: String = session.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')).take(128).collect();
    (!id.is_empty()).then(|| path::join_all(&[&local::dir(), "ask", &id]))
}

fn ask_save(file: &str, title: &str, body: &str) {
    let dir = path::join(&local::dir(), "ask");
    if std::fs::DirBuilder::new().mode(0o700).recursive(true).create(&dir).is_err() {
        return;
    }
    // 马上答了的（没等到 Notification）留下的：一天以前的清掉
    if let Ok(names) = fs::listdir_sorted(&dir) {
        let now = std::time::SystemTime::now();
        for n in names {
            let p = path::join(&dir, &n);
            let old = std::fs::metadata(&p).and_then(|m| m.modified()).ok().and_then(|m| now.duration_since(m).ok());
            if old.is_some_and(|age| age.as_secs() > 86400) {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    let tmp = format!("{file}.tmp");
    let one = |s: &str| s.replace(['\n', '\r'], " ");
    let text = format!("{}\n{}\n", one(title), one(body));
    let wrote = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).and_then(|mut f| {
        use std::io::Write;
        f.write_all(text.as_bytes())
    });
    if wrote.is_err() || std::fs::rename(&tmp, file).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// 刚记下的那一句（ASK_FRESH 秒以内的；马上答了的那次留下的旧话不拿来配别的通知），读了就删
fn ask_take(file: &str, permission: bool) -> Option<(String, String)> {
    let fresh = std::fs::metadata(file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|m| std::time::SystemTime::now().duration_since(m).ok())
        .is_some_and(|age| age.as_secs_f64() <= ASK_FRESH);
    let text = if permission && fresh { std::fs::read_to_string(file).ok() } else { None };
    let _ = std::fs::remove_file(file);
    let mut lines = text?.lines().map(str::to_string).collect::<Vec<_>>().into_iter();
    let title = lines.next().filter(|t| !t.is_empty())?;
    Some((title, lines.next().unwrap_or_default()))
}

/// 这一次要推的通知；不用推是 None。PermissionRequest 只记下要说的话
fn note_for(d: &Dict) -> Option<Note> {
    let session = d.str("session_id").map(|s| s.to_string()).unwrap_or_default();
    let (title, body) = match d.str("hook_event_name").as_deref() {
        Some("Stop") => ("Claude Code 做完了".to_string(), done_text(d)?),
        Some("PermissionRequest") => {
            let (title, body) = ask_text(d);
            if let Some(f) = ask_file(&session) {
                ask_save(&f, &title, &body);
            }
            return None;
        }
        Some("Notification") => {
            let permission = d.str("notification_type").as_deref() == Some("permission_prompt");
            match ask_file(&session).and_then(|f| ask_take(&f, permission)) {
                Some(tb) => tb,
                None => {
                    let said = clip(&line(&d.get("message").unwrap_or(V::None)));
                    let body = if said.is_empty() || said.starts_with("Claude needs your permission") {
                        "要你确认一个操作".into()
                    } else {
                        said
                    };
                    ("Claude Code 在等你".to_string(), body)
                }
            }
        }
        _ => return None,
    };
    // 副标题：账号的别名（CCA_ACCOUNT 是「别名(邮箱)」）· 项目的文件夹
    let account = getenv("CCA_ACCOUNT").map(|a| a.split('(').next().unwrap_or_default().to_string()).unwrap_or_default();
    let dir =
        getenv("CLAUDE_PROJECT_DIR").filter(|d| !d.is_empty()).or_else(|| d.str("cwd").map(|c| c.to_string())).unwrap_or_default();
    let subtitle = if dir.is_empty() { account } else { format!("{account} · {}", base(&dir)) };
    Note::parse(&Note::new(title, subtitle, body).to_json())
}

/// `remote-hook`：stdin 是 Claude Code 给钩子的输入（JSON）
pub fn cmd_remote_hook(_args: &[String]) -> R<()> {
    // SAFETY: 只是问 fd 0 是不是终端（手动运行时不等输入）
    if unsafe { libc::isatty(0) } == 1 {
        return Ok(());
    }
    let Some(sid) = host_of_me() else { return Ok(()) };
    let mut raw = Vec::new();
    if std::io::stdin().lock().take(INPUT_MAX).read_to_end(&mut raw).is_err() {
        return Ok(());
    }
    let Some(d) = std::str::from_utf8(&raw).ok().and_then(|s| json::loads(s).ok()).and_then(|v| v.as_dict()) else { return Ok(()) };
    if let Some(note) = note_for(&d) {
        let _ = notify_host(&sid, &note);
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Dict {
        json::loads(text).unwrap().as_dict().unwrap()
    }

    #[test]
    fn one_line() {
        assert_eq!(line(&V::str("\n\n  - **做完了**：改了 `a.rs`  \n第二行")), "做完了：改了 a.rs");
        assert_eq!(line(&V::str("# 标题\n")), "标题");
        assert_eq!(line(&V::str("   \n---\n")), "");
        assert_eq!(line(&V::None), "");
        assert_eq!(line(&V::Int(3)), "3");
        let long = "字".repeat(200);
        assert_eq!(clip(&long).chars().count(), CLIP + 1);
        assert!(clip(&long).ends_with('…'));
        assert_eq!(clip("短的"), "短的");
    }

    #[test]
    fn done_and_busy() {
        assert_eq!(done_text(&doc(r#"{"last_assistant_message": "改好了\n细节"}"#)).as_deref(), Some("改好了"));
        assert_eq!(done_text(&doc(r#"{"last_assistant_message": null}"#)).as_deref(), Some("这一轮做完了"));
        assert_eq!(done_text(&doc(r#"{"background_tasks": [{"type": "shell"}]}"#)), None, "后台的 Shell 还在跑");
        assert_eq!(
            done_text(&doc(
                r#"{"background_tasks": [{"type": "dream"}, {"type": "auto_mode_scan"}], "last_assistant_message": "好了"}"#
            ))
            .as_deref(),
            Some("好了")
        );
        assert_eq!(done_text(&doc(r#"{"session_crons": [{"id": "x"}]}"#)), None, "到点会醒来");
    }

    #[test]
    fn asks() {
        let ask = |j: &str| ask_text(&doc(j));
        assert_eq!(
            ask(
                r#"{"tool_name": "AskUserQuestion", "tool_input": {"questions": [{"question": "用哪个库？"}, {"question": "b"}]}}"#
            ),
            ("Claude Code 在问你".into(), "用哪个库？（一共 2 个问题）".into())
        );
        assert_eq!(ask(r#"{"tool_name": "AskUserQuestion", "tool_input": {}}"#).1, "有问题要你回答");
        assert_eq!(
            ask(r###"{"tool_name": "ExitPlanMode", "tool_input": {"plan": "## 计划\n1. 改"}}"###),
            ("Claude Code 等你批准计划".into(), "计划".into())
        );
        assert_eq!(
            ask(r#"{"tool_name": "Bash", "tool_input": {"command": "rm -rf build", "description": null}}"#).1,
            "运行：rm -rf build"
        );
        assert_eq!(
            ask(r#"{"tool_name": "Bash", "tool_input": {"command": "x", "description": "清掉编译的"}}"#).1,
            "运行：清掉编译的"
        );
        assert_eq!(ask(r#"{"tool_name": "Write", "tool_input": {"file_path": "/a/b/c.rs"}}"#).1, "写文件：c.rs");
        assert_eq!(ask(r#"{"tool_name": "NotebookEdit", "tool_input": {"notebook_path": "/a/n.ipynb"}}"#).1, "改文件：n.ipynb");
        assert_eq!(
            ask(r#"{"tool_name": "WebFetch", "tool_input": {"url": "https://example.com/x"}}"#).1,
            "访问：https://example.com/x"
        );
        assert_eq!(ask(r#"{"tool_name": "WebSearch", "tool_input": {"query": "rust hpack"}}"#).1, "搜索：rust hpack");
        assert_eq!(ask(r#"{"tool_name": "mcp__github__create_issue__v2"}"#).1, "用 github 的 create_issue__v2");
        assert_eq!(ask(r#"{"tool_name": "Task"}"#), ("Claude Code 要你确认".into(), "用 Task".into()));
        assert_eq!(ask(r#"{}"#).1, "用 一个工具");
    }
}
