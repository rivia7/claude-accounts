//! cca 自己带的 Claude Code 钩子（`remote-hook`）：cca 启动 Claude Code 时经 `--settings` 带上 Stop、PermissionRequest、
//! Notification 三个钩子（`hooks`，accounts.rs 的 `claude_hooks` 把它和记下会话的 SessionStart 并在一起），回调 `cca _remote-hook`。
//! 这一轮做完了、停下来等你回答时：
//! - 在会话宿主里（远程会话开着：`cca remote on`、这一次 `CCA_REMOTE=1`）推一条通知（`NOTIFY_REQ`）给接着这个会话的各处——
//!   别的机器上的终端、cca App，手机锁着屏时经推送（docs/dev/push.md）
//! - 这台 Mac 上弹一个（popup.rs：config.json 的 `popup` 不是 false）。只给交互的会话（`CLAUDE_CODE_ENTRYPOINT` 是 cli：打印模式
//!   是 sdk-cli，继承来的 cli 在打印模式里 Claude Code 也改成 sdk-cli），用户自己的钩子已经会弹的那一种不再弹（`user_pops`），不然一件事弹两次
//!
//! 两样都用不上的时候（Linux 上远程会话没开、popup 关着）cca 不带这几个钩子。
//!
//! 照维护者原来自己写的那两个钩子（docs/dev/remote.md「cca 自己带的钩子」）：
//! - Stop：还有后台任务在跑（`background_tasks`，Claude Code 自己的 dream、auto_mode_scan 不算）、到点会醒来（`session_crons`）时
//!   不说（做完了会叫醒会话，那一轮结束时再来一次 Stop）；正文是最后一条回复的第一行
//! - PermissionRequest（提问、确认一个操作、批准计划）：这时不说，把要说的那一句记下来；6 秒后还没人答，Claude Code 自己发
//!   Notification（`permission_prompt`，只有一句 "Claude needs your permission"），那时用记下的那一句推。马上答了的不打扰
//! - 别的在等你的 Notification（MCP 服务器要你填的、后台的会话要你回答的……）照 Claude Code 的原话
//!
//! 总是成功、什么都不打印：Stop 的钩子退出码 2 会拦着 Claude Code 不让它停，stdout 在有的钩子里会进 Claude 的上下文
//!
//! Codex、Antigravity 的「做完了 / 在等你」也在这里（docs/dev/remote.md「Codex、Antigravity 的通知」）：Codex 一轮做完了起
//! `cca _remote-hook --codex …`（它的 notify，`codex_hook`），在等你的那几种它往终端里写 OSC 9、宿主认出来（`codex_osc_note`）；
//! Antigravity 的从状态栏的命令里来（agy.rs 的 `agy_turn`）。这几种在会话宿主里时推给接着它的各处，这台 Mac 上再弹一个
//! （popup.rs，`deliver`）
use super::local::{self, Note};
use super::manage::{host_of_me, notify_host};
use super::popup;
use crate::core::base::SELF_PATH;
use crate::dict;
use crate::py::fs;
use crate::py::json;
use crate::py::obj::{Dict, V};
use crate::py::proc::getenv;
use crate::py::text::sh_quote;
use crate::py::{path, R};
use std::io::Read;

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
/// 用户自己的钩子弹不弹通知：看它的命令、它跑的脚本里有没有这几个字（小写比较）。osascript、notif（notify、notifier、
/// notification、terminal-notifier、notify-send）、alerter
const NOTIFIER_WORDS: [&str; 3] = ["osascript", "notif", "alerter"];
/// 用户的钩子跑的脚本最多看这么多（字节）
const SCRIPT_MAX: u64 = 256 << 10;
/// 这台 Mac 上弹的那一条最多等它多久（秒）：钩子退出以后留下的进程没人管，等它弹完再走（平时一两百毫秒）
const POP_WAIT: f64 = 3.0;

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
    let one = |s: &str| s.replace(['\n', '\r'], " ");
    // 马上答了的（没等到 Notification）留下的：一天以前的清掉
    fs::note_file(&path::join(&local::dir(), "ask"), file, &format!("{}\n{}\n", one(title), one(body)), true);
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
    Note::parse(&Note::new(title, subtitle(&project_of(d)), body).to_json())
}

/// 项目的文件夹：Claude Code 给钩子的 CLAUDE_PROJECT_DIR，没有时是输入里的 cwd
fn project_of(d: &Dict) -> String {
    getenv("CLAUDE_PROJECT_DIR").filter(|p| !p.is_empty()).or_else(|| d.str("cwd").map(|c| c.to_string())).unwrap_or_default()
}

/// Claude Code 读的那几份设置：用户的（CLAUDE_CONFIG_DIR 下的 settings.json，cca 启动的会话里它链到共享的
/// ~/.claude/settings.json）、项目的 .claude/settings.json 和 settings.local.json。--settings 里的是 cca 自己的这几个，不算；
/// 系统级的（管理员放的 managed-settings.json）、插件带的不看
fn settings_files(project: &str) -> Vec<String> {
    let user = getenv("CLAUDE_CONFIG_DIR")
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| path::join(&getenv("HOME").unwrap_or_default(), ".claude"));
    let mut files = vec![path::join(&user, "settings.json")];
    if !project.is_empty() {
        files.push(path::join_all(&[project, ".claude", "settings.json"]));
        files.push(path::join_all(&[project, ".claude", "settings.local.json"]));
    }
    files
}

/// Notification 的 matcher 认不认这一种通知（照 Claude Code：空的、* 都认；只有字母、数字、_、| 的按 | 分开逐个比；别的当正则，
/// 哪里匹配上都算，写错了的不认）
fn matcher_hits(matcher: &str, kind: &str) -> bool {
    if matcher.is_empty() || matcher == "*" {
        return true;
    }
    if matcher.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '|')) {
        return matcher.split('|').any(|m| m == kind);
    }
    regex::Regex::new(matcher).is_ok_and(|r| r.is_match(kind))
}

/// 命令里的一段路径：开头的 ~、Claude Code 给钩子的几个变量换成真的值
fn expand(word: &str) -> String {
    let mut s = match word.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", getenv("HOME").unwrap_or_default()),
        None => word.to_string(),
    };
    for name in ["HOME", "CLAUDE_PROJECT_DIR", "CLAUDE_CONFIG_DIR"] {
        let v = getenv(name).unwrap_or_default();
        s = s.replace(&format!("${{{name}}}"), &v).replace(&format!("${name}"), &v);
    }
    s
}

/// 脚本文件的内容（最多 SCRIPT_MAX 字节）。不是普通文件的（目录；管道：打开它会一直等着）、读不了的、里面有 NUL 的（程序文件，
/// 像 /bin/bash：里面就有 notify 这个词）是 None
fn script_text(p: &str) -> Option<String> {
    if !std::fs::metadata(p).ok()?.is_file() {
        return None;
    }
    let mut buf = Vec::new();
    std::fs::File::open(p).ok()?.take(SCRIPT_MAX).read_to_end(&mut buf).ok()?;
    (!buf.contains(&0)).then(|| String::from_utf8_lossy(&buf).into_owned())
}

/// 照 sh 的样子把命令拆成一段段（引号里的空白不拆、引号去掉；; & | ( ) < > ` 和 = 也当作分隔）：只为找出里面提到的路径
/// （"$CLAUDE_PROJECT_DIR"/.claude/hooks/x.sh 这种写法），不求和 sh 一模一样
fn words(command: &str) -> Vec<String> {
    let (mut out, mut cur, mut quote) = (Vec::new(), String::new(), None::<char>);
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') | (None, '\\') => cur.extend(chars.next()),
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => quote = Some(c),
            (None, c) if c.is_whitespace() || ";&|()<>`=".contains(c) => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 这条钩子的命令会不会弹通知：命令里、或者它提到的脚本（像路径的那几段）里有 NOTIFIER_WORDS 的字
fn notifier(command: &str) -> bool {
    let says = |s: &str| {
        let s = s.to_lowercase();
        NOTIFIER_WORDS.iter().any(|w| s.contains(w))
    };
    says(command) || words(command).iter().filter(|w| w.contains('/')).any(|w| script_text(&expand(w)).is_some_and(|t| says(&t)))
}

/// 这几份设置里有没有用户自己的、会为这一种弹通知的钩子（`notifier`）。done 是做完了：看 Stop 和认 idle_prompt 的
/// Notification；不然是在等你：看 PermissionRequest 和认 MATCHER 里那几种的 Notification。只看 command 那种钩子
fn pops_in(files: &[String], done: bool) -> bool {
    let kinds: Vec<&str> = if done { vec!["idle_prompt"] } else { MATCHER.split('|').collect() };
    let pops = |hooks: &Dict, event: &str, by_kind: bool| {
        hooks.list(event).is_some_and(|groups| {
            groups.items().iter().filter_map(|g| g.as_dict()).any(|g| {
                let matcher = g.str("matcher").map(|m| m.to_string()).unwrap_or_default();
                (!by_kind || kinds.iter().any(|k| matcher_hits(&matcher, k)))
                    && g.list("hooks").is_some_and(|hs| {
                        hs.items()
                            .iter()
                            .filter_map(|h| h.as_dict())
                            .any(|h| h.str("type").as_deref() == Some("command") && h.str("command").is_some_and(|c| notifier(&c)))
                    })
            })
        })
    };
    files
        .iter()
        .filter_map(|f| fs::load_quiet(f).and_then(|v| v.as_dict()).and_then(|s| s.dict("hooks")))
        .any(|hooks| pops(&hooks, if done { "Stop" } else { "PermissionRequest" }, false) || pops(&hooks, "Notification", true))
}

/// 用户自己的钩子会不会为这一种弹通知（`pops_in`）：会的话 cca 不再弹，不然一件事弹两次
fn user_pops(done: bool, project: &str) -> bool {
    pops_in(&settings_files(project), done)
}

/// 副标题：账号的别名（CCA_ACCOUNT 是 bash 给会话设的「别名(邮箱)」，括号前那一段）· 项目的文件夹
pub fn subtitle(dir: &str) -> String {
    let account = getenv("CCA_ACCOUNT").map(|a| a.split('(').next().unwrap_or_default().to_string()).unwrap_or_default();
    if dir.is_empty() {
        account
    } else {
        format!("{account} · {}", base(dir))
    }
}

/// Codex、Antigravity 会话里的一条通知：在会话宿主里时推给接着它的各处（`NOTIFY_REQ`，和 Claude Code 的钩子一样），这台 Mac 上
/// 再弹一个（popup.rs，开着时）。先照宿主的规矩清理、限长；什么字都没有的不推。不说话
pub fn deliver(n: &Note) {
    let Some(n) = Note::parse(&n.to_json()) else { return };
    if let Some(sid) = host_of_me() {
        let _ = notify_host(&sid, &n);
    }
    if popup::on() {
        let _ = popup::show(&n);
    }
}

/// 和 `deliver` 一样，只是会话宿主是给的（agy.rs 的 `agy-turn-wait`：等着推的那个进程不在宿主下面，起它的状态栏命令替它找好）：
/// 给了宿主、宿主已经不在了（会话结束了）时都不推，这台 Mac 上也不弹
pub fn deliver_to(n: &Note, host: Option<&str>) {
    let Some(n) = Note::parse(&n.to_json()) else { return };
    if let Some(sid) = host {
        if notify_host(sid, &n) == Err(65) {
            return;
        }
    }
    if popup::on() {
        let _ = popup::show(&n);
    }
}

/// Codex 一轮做完了（notify 的 JSON：`{"type": "agent-turn-complete", "cwd", "last-assistant-message", …}`）：正文照 Claude Code 的
/// 做完了（最后一条回复的第一行）。别的种类不推
fn codex_done(d: &Dict) -> Option<Note> {
    if d.str("type").as_deref() != Some("agent-turn-complete") {
        return None;
    }
    let body = clip(&line(&d.get("last-assistant-message").unwrap_or(V::None)));
    let body = if body.is_empty() { "这一轮做完了".to_string() } else { body };
    Some(Note::new("Codex 做完了", subtitle(&d.str("cwd").unwrap_or_default()), body))
}

/// `remote-hook --codex [--then 用户的 notify] JSON`：Codex 的 notify（codex.rs 的 `codex_launch_args` 经 -c 加的），JSON 是 Codex
/// 加在最后的那一个参数。--then 是用户自己在 config.toml 里写的 notify（JSON 的字符串数组）：照 Codex 的做法接着起它（同一份 JSON
/// 加在最后），不等它
fn codex_hook(args: &[String]) {
    let (user, payload) = match args {
        [flag, user, payload] if flag == "--then" => (Some(user), payload),
        [payload] => (None, payload),
        _ => return,
    };
    let argv: Option<Vec<String>> = user
        .and_then(|u| json::loads(u).ok())
        .and_then(|v| v.as_list())
        .and_then(|l| l.items().iter().map(|a| a.as_str().map(str::to_string)).collect());
    if let Some([prog, rest @ ..]) = argv.as_deref() {
        let _ = std::process::Command::new(prog)
            .args(rest)
            .arg(payload)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    if let Some(n) = json::loads(payload).ok().and_then(|v| v.as_dict()).and_then(|d| codex_done(&d)) {
        deliver(&n);
    }
}

/// 宿主在 Codex 会话的输出里认出的一条通知（vt.rs 的 `osc_note_text`：OSC 9 的字、OSC 777 notify 的正文）：Codex 的
/// tui.notifications，codex_launch_args 只开了在等你的那几种。认得的英文（0.172.0 的写法）换成中文、标题照 Claude Code 的那几种，
/// 认不出的「Codex 在等你」+ 原话。一轮做完了的那一句（Agent turn complete：用户自己的 -c 又打开了它）不推：notify 推过了。
/// cwd 是会话的文件夹（副标题）
pub fn codex_osc_note(text: &str, cwd: &str) -> Option<Note> {
    let t = text.trim();
    if t.is_empty() || t == "Agent turn complete" {
        return None;
    }
    let after = |p: &str| t.strip_prefix(p).map(str::trim);
    let (title, body) = if let Some(cmd) = after("Approval requested: ") {
        ("Codex 要你确认", format!("运行：{}", clip(cmd)))
    } else if let Some(what) = after("Codex wants to edit ") {
        match what.strip_suffix(" files").and_then(|n| n.parse::<u32>().ok()) {
            Some(n) => ("Codex 要你确认", format!("改 {n} 个文件")),
            None => ("Codex 要你确认", format!("改文件：{}", base(what))),
        }
    } else if let Some(server) = after("Approval requested by ") {
        ("Codex 要你确认", format!("用 {}", clip(server)))
    } else if let Some(plan) = after("Plan mode prompt: ") {
        ("Codex 等你批准计划", clip(plan))
    } else if let Some(q) = after("Question: ") {
        ("Codex 在问你", clip(q))
    } else {
        ("Codex 在等你", clip(&line(&V::str(t))))
    };
    Some(Note::new(title, subtitle(cwd), body))
}

/// Antigravity 的几件事（agy.rs 的状态栏命令从状态的变化里看出来的）
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AgyEvent {
    /// 干完活回到 idle
    Done,
    /// 干着活到了 error
    Failed,
    /// 要你确认一个操作（tool_confirmation_pending）
    Ask,
}

/// Antigravity 的一条通知：标题照 Claude Code 的那几种；正文是会话的标题（状态栏 JSON 的 conversation_title：它不给最后一条
/// 回复），没有时是一句默认的话。cwd 是 agy 启动的文件夹（副标题）
pub fn agy_note(what: AgyEvent, topic: &str, cwd: &str) -> Note {
    let topic = clip(&line(&V::str(topic)));
    let or = |dflt: &str| if topic.is_empty() { dflt.to_string() } else { topic.clone() };
    let (title, body) = match what {
        AgyEvent::Done => ("Antigravity 做完了", or("这一轮做完了")),
        AgyEvent::Failed => ("Antigravity 出错了", or("这一轮没做完")),
        AgyEvent::Ask => ("Antigravity 在等你", "要你确认一个操作".to_string()),
    };
    Note::new(title, subtitle(cwd), body)
}

/// `remote-hook`：stdin 是 Claude Code 给钩子的输入（JSON）；`remote-hook --codex …` 是 Codex 的 notify（`codex_hook`）
pub fn cmd_remote_hook(args: &[String]) -> R<()> {
    if args.first().is_some_and(|a| a == "--codex") {
        codex_hook(&args[1..]);
        return Ok(());
    }
    // SAFETY: 只是问 fd 0 是不是终端（手动运行时不等输入）
    if unsafe { libc::isatty(0) } == 1 {
        return Ok(());
    }
    // 这台 Mac 上弹不弹：开着（popup::on），又是交互的会话（打印模式是 sdk-cli；Claude App 里的不经 cca）
    let pop = popup::on() && getenv("CLAUDE_CODE_ENTRYPOINT").as_deref() == Some("cli");
    let host = host_of_me();
    if host.is_none() && !pop {
        return Ok(());
    }
    let mut raw = Vec::new();
    if std::io::stdin().lock().take(INPUT_MAX).read_to_end(&mut raw).is_err() {
        return Ok(());
    }
    let Some(d) = std::str::from_utf8(&raw).ok().and_then(|s| json::loads(s).ok()).and_then(|v| v.as_dict()) else { return Ok(()) };
    let pop = pop && !user_pops(d.str("hook_event_name").as_deref() == Some("Stop"), &project_of(&d));
    if host.is_none() && !pop {
        return Ok(());
    }
    if let Some(note) = note_for(&d) {
        if let Some(sid) = &host {
            let _ = notify_host(sid, &note);
        }
        if pop {
            popup::show_wait(&note, POP_WAIT);
        }
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
    fn agy_notes() {
        let n = agy_note(AgyEvent::Done, "**Explain** the code\nmore", "/a/work");
        assert_eq!((n.title.as_str(), n.body.as_str()), ("Antigravity 做完了", "Explain the code"));
        assert!(n.subtitle.ends_with(" · work"), "{:?}", n.subtitle);
        assert_eq!(agy_note(AgyEvent::Done, "", "").body, "这一轮做完了");
        assert_eq!(
            (agy_note(AgyEvent::Failed, " ", "").title.as_str(), agy_note(AgyEvent::Failed, " ", "").body.as_str()),
            ("Antigravity 出错了", "这一轮没做完")
        );
        let ask = agy_note(AgyEvent::Ask, "标题不用", "");
        assert_eq!((ask.title.as_str(), ask.body.as_str()), ("Antigravity 在等你", "要你确认一个操作"));
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
    fn codex_turns() {
        let done = |j: &str| codex_done(&doc(j)).map(|n| (n.title, n.body));
        assert_eq!(
            done(r#"{"type": "agent-turn-complete", "cwd": "/a/work", "last-assistant-message": "**改好了**\n细节"}"#),
            Some(("Codex 做完了".into(), "改好了".into()))
        );
        assert_eq!(done(r#"{"type": "agent-turn-complete", "last-assistant-message": null}"#).unwrap().1, "这一轮做完了");
        assert_eq!(done(r#"{"type": "approval-requested"}"#), None, "别的种类不推");
        assert!(codex_done(&doc(r#"{"type": "agent-turn-complete", "cwd": "/a/work"}"#)).unwrap().subtitle.ends_with(" · work"));
    }

    #[test]
    fn codex_waits() {
        let note = |t: &str| codex_osc_note(t, "/a/work").map(|n| (n.title, n.body));
        let pair = |a: &str, b: &str| Some((a.to_string(), b.to_string()));
        assert_eq!(note("Approval requested: rm -rf build"), pair("Codex 要你确认", "运行：rm -rf build"));
        assert_eq!(note("Codex wants to edit src/main.rs"), pair("Codex 要你确认", "改文件：main.rs"));
        assert_eq!(note("Codex wants to edit 3 files"), pair("Codex 要你确认", "改 3 个文件"));
        assert_eq!(note("Approval requested by github"), pair("Codex 要你确认", "用 github"));
        assert_eq!(note("Plan mode prompt: Implement this plan?"), pair("Codex 等你批准计划", "Implement this plan?"));
        assert_eq!(note("Question: 用哪个库？"), pair("Codex 在问你", "用哪个库？"));
        assert_eq!(note("**Something** new"), pair("Codex 在等你", "Something new"), "认不出的照原话");
        assert_eq!(note("   "), None);
        assert_eq!(note("Agent turn complete"), None, "一轮做完了的 notify 推过了");
        assert_eq!(note(&format!("Question: {}", "字".repeat(200))).unwrap().1.chars().count(), CLIP + 1);
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

    #[test]
    fn matchers() {
        assert!(matcher_hits("", "idle_prompt") && matcher_hits("*", "permission_prompt"), "空的、* 都认");
        assert!(matcher_hits("permission_prompt|idle_prompt", "idle_prompt"));
        assert!(!matcher_hits("permission_prompt", "idle_prompt"));
        assert!(!matcher_hits("idle", "idle_prompt"), "只有字母、数字、_、| 的逐个比，不是找得到就算");
        assert!(matcher_hits("idle.*", "idle_prompt") && matcher_hits("^perm", "permission_prompt"), "别的当正则");
        assert!(!matcher_hits("(", "idle_prompt"), "写错了的正则不认");
    }

    #[test]
    fn users_own_popups() {
        let dir = fs::TempDir::new().unwrap();
        let at = |n: &str| path::join(&dir.path, n);
        std::fs::write(at("say.sh"), "#!/bin/sh\nterminal-notifier -message \"$1\"\n").unwrap();
        std::fs::write(at("log.sh"), "#!/bin/sh\necho done >> \"$HOME/x.log\"\n").unwrap();
        std::fs::write(at("prog"), b"\xcf\xfa\xed\xfe\0\0notify\0").unwrap();
        assert!(notifier("osascript -e 'display notification \"好了\"'"));
        assert!(notifier("/bin/bash \"$HOME/.claude/hooks/cca-notify-done.sh\""), "名字里就有");
        assert!(notifier(&format!("/bin/sh '{}' done", at("say.sh"))), "它跑的脚本里有");
        assert!(notifier(&format!("FOO=1 {}&&true", at("say.sh"))));
        assert!(!notifier(&format!("/bin/sh {}", at("log.sh"))));
        assert!(!notifier(&format!("{} --quiet", at("prog"))), "程序文件不看（/bin/bash 里就有 notify 这个词）");
        assert!(!notifier(&format!("cat {}", dir.path)), "目录不看");
        assert!(!notifier("echo hi"));
        // "$CLAUDE_PROJECT_DIR"/.claude/hooks/x.sh 这种写法：引号去掉、不拆开；引号里的空白不拆
        let (parent, base) = dir.path.rsplit_once('/').unwrap();
        assert!(notifier(&format!("\"{parent}\"/{base}/say.sh")));
        std::fs::create_dir(at("a b")).unwrap();
        std::fs::write(at("a b/say.sh"), "osascript -e x\n").unwrap();
        assert!(notifier(&format!("/bin/sh '{}'", at("a b/say.sh"))));
        assert_eq!(words(r#"FOO=1 "/a b"/c\ d.sh|x;'y z'&&w"#), ["FOO", "1", "/a b/c d.sh", "x", "y z", "w"]);
        let mine = format!("/bin/sh {}", at("say.sh"));
        let group = |matcher: Option<&str>, kind: &str, command: &str| {
            let g = dict! { "hooks" => crate::list![dict! { "type" => kind, "command" => command }] };
            if let Some(m) = matcher {
                g.set("matcher", m);
            }
            V::Dict(g)
        };
        let n = std::cell::Cell::new(0);
        let settings = |event: &str, g: V| {
            n.set(n.get() + 1);
            let f = at(&format!("settings-{}.json", n.get()));
            let hooks = dict! {};
            hooks.set(event, crate::list![g]);
            std::fs::write(&f, json::dumps(&V::Dict(dict! { "hooks" => hooks }), json::LINE)).unwrap();
            f
        };
        // （事件，matcher，做完了的那一种算不算，在等你的那一种算不算）
        for (event, matcher, done, ask) in [
            ("Stop", None, true, false),
            ("PermissionRequest", None, false, true),
            ("Notification", None, true, true),
            ("Notification", Some("idle_prompt"), true, false),
            ("Notification", Some("permission_prompt|elicitation_dialog"), false, true),
            ("Notification", Some("auth_success"), false, false),
            ("SessionStart", None, false, false),
        ] {
            let f = [settings(event, group(matcher, "command", &mine))];
            assert_eq!((pops_in(&f, true), pops_in(&f, false)), (done, ask), "{event} {matcher:?}");
        }
        let quiet = [settings("Stop", group(None, "command", &format!("/bin/sh {}", at("log.sh"))))];
        assert!(!pops_in(&quiet, true), "不弹通知的钩子不算");
        let prompt = [settings("Stop", group(None, "prompt", "notify me"))];
        assert!(!pops_in(&prompt, true), "只看 command 那种");
        std::fs::write(at("bad.json"), "{not json").unwrap();
        assert!(!pops_in(&[at("bad.json"), at("missing.json")], true), "读不了的、不是 JSON 的不算");
        let mixed = [at("missing.json"), settings("Stop", group(None, "command", "osascript -e x"))];
        assert!(pops_in(&mixed, true), "几份里有一份有就算");
    }
}
