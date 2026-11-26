//! 远程启动用的模板（docs/dev/launch.md）：主人在这台机器上写好「哪个工具、哪个账号、在哪个文件夹、带哪些参数」；允许远程启动的手机
//! （配对表的 `launch`）只能挑一个模板起会话，再加一句开场白（launch.rs）：账号、文件夹、参数都改不了。
//!
//! - 存在 `~/.cca/remote/templates.json`（600，改之前拿 `Lock`）：`{"v":1,"templates":[{"id","name","tool","account","dir","args","at"}]}`。
//!   账号存邮箱（没有邮箱的存账号表的键），每次用的时候重新找：改别名不影响，账号删了这个模板就用不了（`no_account`）
//! - 摘要（`digest`）：工具、账号、文件夹、参数算出来的，放在给手机的说明里；手机起会话时带回来，和现在的对不上（模板改过了）就不起：
//!   手机上看到的参数就是要运行的
//! - `remote-template list [--json]`、`add`、`rm`：bash 的 `cca remote template`（PROTOCOL 9.4）

use super::far::{aid_of, clean, short_name, team_salt};
use super::{launch, local, peers, relay};
use crate::cmd::agy::agy_interactive;
use crate::cmd::codex::codex_interactive;
use crate::cmd::hub::HubKind;
use crate::cmd::settings::{home_tilde, launch_settings_problems_in, CLAUDE_VALUES};
use crate::cmd::table::account_named;
use crate::core::base::Lock;
use crate::core::model::parse_key;
use crate::core::tool::{tools, Tool, Using};
use crate::dict;
use crate::py::fs;
use crate::py::hash::sha256_hex;
use crate::py::json;
use crate::py::obj::{Dict, List, V};
use crate::py::path;
use crate::py::rand::token_hex;
use crate::py::text::{sh_quote, sh_split};
use crate::py::time::time;
use crate::py::{die, R};

/// 最多存这么多个（给手机的说明里也最多放这么多）
pub const MAX: usize = 16;
const NAME_MAX: usize = 40;
/// 参数最多几个、合起来最多多少字节
const ARGS_MAX: usize = 64;
const ARGS_BYTES: usize = 4096;
/// 给手机的说明（明文）最多这么大：服务器收的密文最长 4096 个字符（base64），正好是 3072 个字节
pub const DOC_MAX: usize = 3072;
const USAGE: &str =
    "用法：remote-template list [--json] | add 名字 工具 账号 文件夹 [--replace 编号] (-- 参数… | --args-line 一行参数) | rm 名字或编号";

#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    /// 8 个小写十六进制字符
    pub id: String,
    pub name: String,
    pub tool: String,
    /// 邮箱，没有邮箱的账号是账号表的键
    pub account: String,
    /// 绝对路径
    pub dir: String,
    pub args: Vec<String>,
    pub at: f64,
}

fn file() -> String {
    path::join(&local::dir(), "templates.json")
}

/// 模板文件现在的样子（启动器隔几秒看一眼：变了就重发给手机的说明）
pub fn stamp() -> peers::Stamp {
    peers::stamp_of(&file())
}

fn row_of(d: &Dict) -> Option<Template> {
    let args: Option<Vec<String>> = d.get("args")?.as_list()?.items().iter().map(|v| v.as_str().map(str::to_string)).collect();
    let t = Template {
        id: d.str("id")?.to_string(),
        name: d.str("name")?.to_string(),
        tool: d.str("tool")?.to_string(),
        account: d.str("account")?.to_string(),
        dir: d.str("dir")?.to_string(),
        args: args?,
        at: d.get("at").and_then(|v| v.as_num()).unwrap_or(0.0),
    };
    let ok = peers::lower_hex(&t.id, 8)
        && !t.name.is_empty()
        && tool_named(&t.tool).is_some()
        && !t.account.is_empty()
        && t.dir.starts_with('/');
    ok.then_some(t)
}

/// 存着的模板（样子不对的那一项不算；同一个编号的只认头一个）
pub fn load() -> Vec<Template> {
    let Some(list) = fs::load_quiet(&file()).and_then(|j| j.as_dict()).and_then(|j| j.get("templates")).and_then(|v| v.as_list())
    else {
        return Vec::new();
    };
    let mut out: Vec<Template> = Vec::new();
    for v in list.items() {
        if let Some(t) = v.as_dict().and_then(|d| row_of(&d)) {
            if out.len() < MAX && !out.iter().any(|o| o.id == t.id) {
                out.push(t);
            }
        }
    }
    out
}

fn save(all: &[Template]) -> R<()> {
    local::ensure_dir()?;
    let list = List::new();
    for t in all {
        list.push(dict! {
            "id" => t.id.as_str(), "name" => t.name.as_str(), "tool" => t.tool.as_str(), "account" => t.account.as_str(),
            "dir" => t.dir.as_str(), "args" => t.args_v(), "at" => t.at
        });
    }
    fs::save_json(&file(), &V::Dict(dict! { "v" => 1, "templates" => list }), Some(0o600), true)?;
    Ok(())
}

fn tool_named(name: &str) -> Option<&'static Tool> {
    tools().iter().find(|t| t.name == name)
}

impl Template {
    pub fn tool(&self) -> &'static Tool {
        tool_named(&self.tool).expect("读的时候核对过")
    }

    /// 参数（JSON 的数组）
    fn args_v(&self) -> V {
        V::from(self.args.iter().map(|a| V::from(a.as_str())).collect::<Vec<V>>())
    }

    /// 摘要：`SHA256(紧凑的 JSON [工具, 账号, 文件夹, [参数…]])` 的前 16 个十六进制字符（server/README.md「启动器」）
    pub fn digest(&self) -> String {
        let doc = crate::list![self.tool.as_str(), self.account.as_str(), self.dir.as_str(), self.args_v()];
        sha256_hex(json::dumps(&V::List(doc), json::Fmt::new().utf8().compact()))[..16].to_string()
    }

    /// 账号表里现在的这个账号：(键, 给人看的名字：别名，没有别名的是邮箱)。没有了是 None
    pub fn account_now(&self) -> Option<(String, String)> {
        let table = fs::load_quiet(&self.tool().table)?.as_dict()?;
        let want = self.account.to_lowercase();
        let key = table.keys().into_iter().find(|k| {
            let (_, e) = parse_key(k);
            **k == *self.account || (!e.is_empty() && e.to_lowercase() == want)
        })?;
        let (a, e) = parse_key(&key);
        Some((key.to_string(), if a.is_empty() { e } else { a }))
    }

    /// 能不能带开场白（接在参数后面：`-- 开场白`，工具当它是头一句话）：Claude Code、Codex 认；agy 还没核对过，先不给；参数里
    /// 已经有 `--` 的也不给（后面再接一个就不是一句话了）
    pub fn line_ok(&self) -> bool {
        matches!(self.tool.as_str(), "claude" | "codex") && !self.args.iter().any(|a| a == "--")
    }

    /// 现在用得了用不了：`no_account`（账号表里没有了）、`no_dir`（文件夹没了）、`bad_args`（参数有问题，或者不是交互的会话）、
    /// `no_relay`（这个账号的会话不经中转：这台机器不在哪个 Team 里）；用得了是 None
    pub fn problem(&self) -> R<Option<&'static str>> {
        let Some((key, _)) = self.account_now() else { return Ok(Some("no_account")) };
        if !path::isdir(&self.dir) {
            return Ok(Some("no_dir"));
        }
        if !args_problems(self.tool(), &self.args, &self.dir)?.is_empty() {
            return Ok(Some("bad_args"));
        }
        if relay::session_hub(&self.tool, &key).is_none() {
            return Ok(Some("no_relay"));
        }
        Ok(None)
    }

    /// 参数拼成一行：照 shell 的写法，要引的才用单引号引起来、空格隔开（`sh_split` 拆回去是原样；App 拿它给人改，PROTOCOL 9.4 的 `args_line`）
    pub fn args_line(&self) -> String {
        self.args.iter().map(|a| sh_quote(a)).collect::<Vec<_>>().join(" ")
    }

    /// 参数给人看的样子（`args_line`，去掉控制字符；最多 max 个字符，多了截断加 …）
    pub fn args_text(&self, max: usize) -> String {
        let full = clean(&self.args_line(), usize::MAX);
        if full.chars().count() <= max {
            return full;
        }
        let mut s: String = full.chars().take(max.saturating_sub(1)).collect();
        s.push('…');
        s
    }
}

/// Claude Code 的子命令（`claude mcp …` 这些不是会话）
const CLAUDE_SUBCOMMANDS: &[&str] = &["config", "mcp", "plugin", "migrate-installer", "setup-token", "doctor", "update", "install"];

/// 这次是不是 Claude Code 交互的会话：没有 -p / --print、--version、--help，头一个不是选项的参数不是子命令（`--` 后面的都是开场白）
fn claude_interactive(args: &[String]) -> bool {
    let mut i = 0;
    while let Some(a) = args.get(i) {
        match a.as_str() {
            "--" => return true,
            "-p" | "--print" | "-v" | "--version" | "-h" | "--help" => return false,
            f if CLAUDE_VALUES.contains(&f) => i += 2,
            f if f.starts_with('-') && f.len() > 1 => i += 1,
            p => return !CLAUDE_SUBCOMMANDS.contains(&p),
        }
    }
    true
}

/// 模板的参数有什么问题：不是交互的会话（打印模式、`codex exec` 这些跑完就退出，远程起它没有意义）是 `not_interactive`；别的照启动前的
/// 检查（settings.rs 的 launch_settings_problems，在模板的文件夹里看）——只算参数带来的：共享的设置本来就有的问题（不带参数也有的）
/// 不算在模板头上，起会话时 cca 照常在终端里说
pub fn args_problems(tool: &'static Tool, args: &[String], dir: &str) -> R<Vec<String>> {
    let interactive = match tool.name {
        "codex" => codex_interactive(args),
        "agy" => agy_interactive(args),
        _ => claude_interactive(args),
    };
    if !interactive {
        return Ok(vec!["not_interactive".into()]);
    }
    let _using = Using::tool(tool);
    let with = launch_settings_problems_in(args, dir)?;
    if with.is_empty() {
        return Ok(with);
    }
    let without = launch_settings_problems_in(&[], dir)?;
    Ok(with.into_iter().filter(|p| !without.contains(p)).collect())
}

/// 给手机的说明（server/README.md「启动器」）：账号走 hub 这台服务器、现在用得了的模板（账号、文件夹还在），最多 MAX 个；明文放不下
/// DOC_MAX 的不放、记在 `more` 里。参数有没有问题这里不看（共享的设置变了它也跟着变）：起的时候再查
pub fn launch_doc(hub: HubKind) -> Dict {
    let salt = team_salt(hub);
    let machine = peers::machine_name();
    let doc_of = |rows: &[V], more: usize| dict! { "v" => 1, "machine" => machine.as_str(), "t" => rows.to_vec(), "more" => more };
    let (mut rows, mut more): (Vec<V>, usize) = (Vec::new(), 0);
    for t in load() {
        let Some((key, name)) = t.account_now() else { continue };
        if relay::session_hub(&t.tool, &key) != Some(hub) || !path::isdir(&t.dir) {
            continue;
        }
        let row = dict! {
            "i" => t.id.as_str(), "h" => t.digest(), "n" => clean(&t.name, NAME_MAX), "k" => t.tool.as_str(),
            "a" => clean(short_name(&name), 64), "d" => clean(&home_tilde(&t.dir), 200), "x" => t.args_text(300), "l" => t.line_ok()
        };
        if let Some(aid) = salt.as_deref().and_then(|s| aid_of(s, &t.tool, &key)) {
            row.set("aid", aid);
        }
        rows.push(V::Dict(row));
        // more 最多两位数：按两位数算，放进去以后也不会超
        if json::dumps(&V::Dict(doc_of(&rows, 99)), json::Fmt::new().utf8().compact()).len() > DOC_MAX {
            rows.pop();
            more += 1;
        }
    }
    doc_of(&rows, more)
}

fn problem_text(code: &str) -> &'static str {
    match code {
        "no_account" => "账号表里没有这个账号了",
        "no_dir" => "文件夹不在了",
        "bad_args" => "参数用不了",
        "no_relay" => "这个账号的会话不经中转（先加入一个 Team）",
        _ => "用不了",
    }
}

/// `remote-template list [--json] | add … | rm …`
pub fn cmd_remote_template(args: &[String]) -> R<()> {
    match args.first().map(String::as_str) {
        Some("list") => list(&args[1..]),
        Some("add") => add(&args[1..]),
        Some("rm") => rm(&args[1..]),
        _ => die(USAGE),
    }
}

/// 列出来：一行一个（`编号  名字  工具  账号  文件夹  参数`，用不了的后面说原因）；--json 是给界面 App 的（PROTOCOL 9.4）
fn list(args: &[String]) -> R<()> {
    let as_json = match args {
        [] => false,
        [a] if a == "--json" => true,
        _ => return die(USAGE),
    };
    let all = load();
    if as_json {
        let rows = List::new();
        for t in &all {
            let problem = t.problem()?;
            let now = t.account_now();
            let (key, name) = (now.as_ref().map(|(k, _)| k.as_str()), now.as_ref().map(|(_, n)| n.as_str()));
            rows.push(dict! {
                "id" => t.id.as_str(), "name" => t.name.as_str(), "tool" => t.tool.as_str(), "account" => t.account.as_str(),
                "account_key" => key.map_or(V::None, V::from), "account_name" => name.map_or(V::None, V::from), "dir" => t.dir.as_str(),
                "args" => t.args_v(), "args_line" => t.args_line(), "line" => t.line_ok(), "digest" => t.digest(),
                "problem" => problem.map_or(V::None, V::from)
            });
        }
        let doc = dict! { "version" => 1, "kind" => "remote_templates", "templates" => rows, "launcher" => launch::summary() };
        crate::out!("{}", json::dumps(&V::Dict(doc), json::LINE));
        return Ok(());
    }
    for t in &all {
        let who = t.account_now().map_or_else(|| t.account.clone(), |(_, n)| n);
        let note = t.problem()?.map(|p| format!("  （{}）", problem_text(p))).unwrap_or_default();
        let args = if t.args.is_empty() { String::new() } else { format!("  {}", t.args_text(200)) };
        crate::out!("{}  {}  {}  {}  {}{}{}", t.id, clean(&t.name, NAME_MAX), t.tool().label, who, home_tilde(&t.dir), args, note);
    }
    Ok(())
}

/// `add 名字 工具 账号 文件夹 [--replace 编号] (-- 参数… | --args-line 一行参数)`：账号照 `cca @账号` 那样认（别名、邮箱、编号、唯一的
/// 前缀），存成邮箱；文件夹相对当前目录、可以带 ~，要已经在。--replace：改那一个（编号、位置不变）。参数是一项一项的（`--` 后面），
/// 或者照 shell 的写法写成的一行（`--args-line`：App 给的，这里拆，不展开 ~、$ 这些）。stdout：`编号<TAB>名字`
fn add(args: &[String]) -> R<()> {
    // 照位置认（名字、文件夹、参数那一行本身是 `--` 也不会认错）
    let [name, tool, account, dir, rest @ ..] = args else { return die(USAGE) };
    let (replace, rest) = match rest {
        [flag, id, rest @ ..] if flag == "--replace" => (Some(id.as_str()), rest),
        _ => (None, rest),
    };
    let argv = match rest {
        [sep, argv @ ..] if sep == "--" => argv.to_vec(),
        [flag, line] if flag == "--args-line" => split_line(line)?,
        _ => return die(USAGE),
    };
    let argv = argv.as_slice();
    let name = clean(name.trim(), usize::MAX);
    if name.is_empty() || name.chars().count() > NAME_MAX {
        return die(format!("模板的名字要有，最长 {NAME_MAX} 个字"));
    }
    let Some(tool) = tool_named(tool) else { return die("工具只能是 claude、codex、agy") };
    let table = fs::load_quiet(&tool.table).and_then(|v| v.as_dict()).unwrap_or_default();
    let key = {
        let _using = Using::tool(tool);
        account_named(&table, account)?
    };
    let (_, email) = parse_key(&key);
    let account = if email.is_empty() { key.clone() } else { email };
    let dir = path::abspath(&path::join(&std::env::current_dir()?.to_string_lossy(), &path::expanduser(dir)));
    if !path::isdir(&dir) {
        return die(format!("没有这个文件夹：{dir}"));
    }
    if argv.len() > ARGS_MAX || argv.iter().map(String::len).sum::<usize>() > ARGS_BYTES {
        return die(format!("参数太多了（最多 {ARGS_MAX} 个、合起来 {ARGS_BYTES} 个字节）"));
    }
    let bad = args_problems(tool, argv, &dir)?;
    if !bad.is_empty() {
        let why = if bad.iter().any(|b| b == "not_interactive") {
            "模板只能起交互的会话（打印模式、子命令跑完就退出）".to_string()
        } else {
            format!("这些参数启动前的检查过不了：{}（cca settings 看说明）", bad.join("、"))
        };
        return die(why);
    }
    let _lock = Lock::acquire()?;
    let mut all = load();
    let at = match replace {
        Some(id) => match all.iter().position(|t| t.id == id) {
            Some(i) => Some(i),
            None => return die(format!("没有编号是 {id} 的模板")),
        },
        None => None,
    };
    if all.iter().enumerate().any(|(i, t)| Some(i) != at && t.name == name) {
        return die(format!("已经有叫「{name}」的模板了"));
    }
    if at.is_none() && all.len() >= MAX {
        return die(format!("模板最多 {MAX} 个：先删掉用不着的（cca remote template rm 名字）"));
    }
    let id = match at {
        Some(i) => all[i].id.clone(),
        None => loop {
            let id = token_hex(4);
            if !all.iter().any(|t| t.id == id) {
                break id;
            }
        },
    };
    let t = Template { id, name, tool: tool.name.to_string(), account, dir, args: argv.to_vec(), at: time() };
    let said = format!("{}\t{}", t.id, t.name);
    match at {
        Some(i) => all[i] = t,
        None => all.push(t),
    }
    save(&all)?;
    crate::out!("{said}");
    Ok(())
}

/// `--args-line` 的那一行拆成参数（`sh_split`：POSIX shell 的规则，不展开 ~、$、通配符）；拆不开说哪里不对
fn split_line(line: &str) -> R<Vec<String>> {
    sh_split(line).or_else(|e| {
        die(if e.contains("escaped") {
            "参数最后多了一个 \\：拆不开"
        } else {
            "参数里的引号没有配对：拆不开"
        })
    })
}

/// `rm 名字或编号`：stdout 是删掉的那个的名字
fn rm(args: &[String]) -> R<()> {
    let [who] = args else { return die(USAGE) };
    let _lock = Lock::acquire()?;
    let mut all = load();
    let hits: Vec<usize> = (0..all.len()).filter(|i| all[*i].name == *who || all[*i].id == *who).collect();
    let i = match hits.as_slice() {
        [i] => *i,
        [] => return die(format!("没有叫「{who}」的模板（cca remote template 看有哪些）")),
        _ => return die(format!("有好几个模板叫「{who}」：用编号（cca remote template 里第一列）")),
    };
    let t = all.remove(i);
    crate::out!("{}", t.name);
    save(&all)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tpl(tool: &str, args: &[&str]) -> Template {
        Template {
            id: "0123abcd".into(),
            name: "修 bug".into(),
            tool: tool.into(),
            account: "a@example.com".into(),
            dir: "/srv/项目".into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            at: 1.0,
        }
    }

    #[test]
    fn args_line_round_trips() {
        let samples: [&[&str]; 3] = [
            &[],
            &["--model", "opus", "--append-system-prompt", "说中文，改完跑一遍测试"],
            &["a b", "it's", "\"q\"", "$HOME", "~", "", "back\\slash", "tab\tx", "new\nline", "*", "--", "-", "a=b,c:d@e%f+g"],
        ];
        for args in samples {
            let t = tpl("claude", args);
            assert_eq!(split_line(&t.args_line()).ok(), Some(t.args.clone()), "{}", t.args_line());
        }
        // 要引的才引（单引号）；App 拿它给人改，终端、手机上看到的也是这样（args_text）
        let t = tpl("claude", &["--model", "opus", "a b", "it's", ""]);
        assert_eq!(t.args_line(), r#"--model opus 'a b' 'it'"'"'s' ''"#);
        assert_eq!(t.args_text(300), t.args_line());
    }

    #[test]
    fn split_lines() {
        assert_eq!(
            split_line("  --model   opus\t--verbose\n-c ").ok(),
            Some(vec!["--model".into(), "opus".into(), "--verbose".into(), "-c".into()])
        );
        assert_eq!(split_line(" \t\n ").ok(), Some(Vec::<String>::new()));
        assert_eq!(
            split_line(r#"--name='a b'c "" '' "x \"y\"" ~ $HOME *.md"#).ok(),
            Some(["--name=a bc", "", "", "x \"y\"", "~", "$HOME", "*.md"].map(String::from).to_vec())
        );
        for bad in ["--x 'abc", "--x \"abc", "abc\\"] {
            assert!(split_line(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn digest_covers_what_runs() {
        let t = tpl("claude", &["--model", "opus"]);
        // 照 server/README.md「启动器」：SHA256('["claude","a@example.com","/srv/项目",["--model","opus"]]')
        assert_eq!(t.digest(), sha256_hex(r#"["claude","a@example.com","/srv/项目",["--model","opus"]]"#)[..16]);
        let mut u = t.clone();
        u.name = "别的名字".into();
        u.at = 9.0;
        assert_eq!(u.digest(), t.digest(), "名字、时间不算");
        let changes: [fn(&mut Template); 4] = [
            |t| t.args.push("-c".into()),
            |t| t.dir.push('x'),
            |t| t.account = "b@example.com".into(),
            |t| t.tool = "codex".into(),
        ];
        for change in changes {
            let mut u = t.clone();
            change(&mut u);
            assert_ne!(u.digest(), t.digest());
        }
    }

    #[test]
    fn opening_lines() {
        assert!(tpl("claude", &[]).line_ok());
        assert!(tpl("codex", &["--model", "x"]).line_ok());
        assert!(!tpl("agy", &[]).line_ok(), "agy 还没核对过");
        assert!(!tpl("claude", &["--", "x"]).line_ok(), "已经带着 --");
    }

    #[test]
    fn only_interactive_sessions() {
        let s = |a: &[&str]| claude_interactive(&a.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        assert!(s(&[]) && s(&["--model", "mcp"]) && s(&["--", "-p"]) && s(&["--resume"]));
        for no in [&["-p", "hi"][..], &["--print"], &["mcp", "list"], &["--version"], &["-h"]] {
            assert!(!s(no), "{no:?}");
        }
    }

    #[test]
    fn rows_and_text() {
        let base = || {
            dict! {
                "id" => "0123abcd", "name" => "x", "tool" => "codex", "account" => "a@example.com", "dir" => "/tmp",
                "args" => crate::list!["a b", "c"], "at" => 2.0
            }
        };
        let t = row_of(&base()).unwrap();
        assert_eq!(t.args_text(100), "'a b' c");
        assert_eq!(t.args_text(4), "'a …");
        for (k, bad) in
            [("id", V::from("0123ABCD")), ("tool", "vim".into()), ("dir", "rel".into()), ("args", crate::list![1].into())]
        {
            let e = base();
            e.set(k, bad);
            assert!(row_of(&e).is_none(), "{k}");
        }
    }
}
