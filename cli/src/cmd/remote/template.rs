//! 远程启动用的模板（docs/dev/launch.md）：主人在这台机器上写好「哪个工具、在哪个文件夹、带哪些参数」；允许远程启动的手机
//! （配对表的 `launch`）挑一个模板、挑一个账号起会话，再加一句开场白（launch.rs）：文件夹、参数都改不了，账号只能挑这个工具的账号表里
//! 会话走那台服务器的。
//!
//! - 存在 `~/.cca/remote/templates.json`（600，改之前拿 `Lock`）：`{"v":1,"templates":[{"id","name","tool","dir","args","at"}]}`。
//!   3.7.9 及以前的模板还带着账号（`account`）：读的时候不理，存的时候不再写
//! - 摘要（`digest`）：工具、文件夹、参数算出来的，放在给手机的说明里；手机起会话时带回来，和现在的对不上（模板改过了）就不起：
//!   手机上看到的参数就是要运行的
//! - 起会话时挑的账号（`Pick`）：给手机的说明里列出模板用到的工具的账号（会话走这台服务器的），各带一个按配对算的编号（和手机上
//!   「账号」页里同一个账号的编号一样：status.rs 的 `account_tag`），手机起会话时说用哪个（LAUNCH 的 `a`）
//! - `remote-template list [--json]`、`add`、`rm`：bash 的 `cca remote template`（PROTOCOL 9.4）

use super::far::{aid_of, clean, short_name, team_salt};
use super::status::account_tag;
use super::{launch, local, peers, relay};
use crate::cmd::agy::agy_interactive;
use crate::cmd::codex::codex_interactive;
use crate::cmd::hub::HubKind;
use crate::cmd::settings::{home_tilde, launch_settings_problems_in, CLAUDE_VALUES, CODEX_VALUES};
use crate::core::base::Lock;
use crate::core::model::{acct_id, name_of};
use crate::core::team::{index_path, team_path, teams};
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
use std::collections::HashMap;

/// 最多存这么多个（给手机的说明里也最多放这么多）
pub const MAX: usize = 16;
/// 给手机的说明里一个工具最多列几个能挑的账号（照账号表的先后）
const PICKS_MAX: usize = 16;
/// 给旧版的 App、cca 看的账号的名字（它们不认 `accounts`，照模板的 `a` 显示）：能挑的账号不止一个时
const ANY_ACCOUNT: &str = "启动时选账号";
const NAME_MAX: usize = 40;
/// 参数最多几个、合起来最多多少字节
const ARGS_MAX: usize = 64;
const ARGS_BYTES: usize = 4096;
/// 给手机的说明（明文）最多这么大：服务器收的密文最长 4096 个字符（base64），正好是 3072 个字节
pub const DOC_MAX: usize = 3072;
const USAGE: &str =
    "用法：remote-template list [--json] | add 名字 工具 文件夹 [--replace 编号] (-- 参数… | --args-line 一行参数) | rm 名字或编号";

#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    /// 8 个小写十六进制字符
    pub id: String,
    pub name: String,
    pub tool: String,
    /// 绝对路径
    pub dir: String,
    pub args: Vec<String>,
    pub at: f64,
}

fn file() -> String {
    path::join(&local::dir(), "templates.json")
}

/// 说明里用到的文件现在的样子：模板文件，和能挑的账号用到的——各个工具的账号表、各个 Team 的配置（账号归谁、走哪台服务器）。
/// 启动器隔几秒看一眼：变了就重算一遍说明（和原来的不一样才重发给手机）
pub fn stamp() -> Vec<peers::Stamp> {
    let mut out = vec![peers::stamp_of(&file()), peers::stamp_of(&index_path())];
    out.extend(tools().iter().map(|t| peers::stamp_of(&t.table)));
    for t in teams() {
        out.push(peers::stamp_of(&team_path(&t.id, "doc.json")));
        out.push(peers::stamp_of(&team_path(&t.id, "sync.json")));
    }
    out
}

fn row_of(d: &Dict) -> Option<Template> {
    let args: Option<Vec<String>> = d.get("args")?.as_list()?.items().iter().map(|v| v.as_str().map(str::to_string)).collect();
    let t = Template {
        id: d.str("id")?.to_string(),
        name: d.str("name")?.to_string(),
        tool: d.str("tool")?.to_string(),
        dir: d.str("dir")?.to_string(),
        args: args?,
        at: d.get("at").and_then(|v| v.as_num()).unwrap_or(0.0),
    };
    let ok = peers::lower_hex(&t.id, 8) && !t.name.is_empty() && tool_named(&t.tool).is_some() && t.dir.starts_with('/');
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
            "id" => t.id.as_str(), "name" => t.name.as_str(), "tool" => t.tool.as_str(), "dir" => t.dir.as_str(),
            "args" => t.args_v(), "at" => t.at
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

    /// 摘要：`SHA256(紧凑的 JSON [工具, 文件夹, [参数…]])` 的前 16 个十六进制字符（server/README.md「启动器」）。账号不在里面：
    /// 起会话时另外挑（LAUNCH 的 `a`）
    pub fn digest(&self) -> String {
        let doc = crate::list![self.tool.as_str(), self.dir.as_str(), self.args_v()];
        sha256_hex(json::dumps(&V::List(doc), json::Fmt::new().utf8().compact()))[..16].to_string()
    }

    /// 能不能带开场白（接在参数后面：`-- 开场白`，工具当它是头一句话）：Claude Code、Codex 认；agy 还没核对过，先不给。参数里
    /// 已经有一句话的（[`has_words`]）也不给：再接一句的话，Claude Code 只拿头一句、手机给的那句不理（2.1.332：多出来的参数
    /// 它不拦也不用），Codex 起不来（0.172.0：unexpected argument）
    pub fn line_ok(&self) -> bool {
        match self.tool.as_str() {
            "claude" => !has_words(&self.args, CLAUDE_VALUES),
            "codex" => !has_words(&self.args, CODEX_VALUES),
            _ => false,
        }
    }

    /// 现在用得了用不了（picks：这个工具的账号，[`picks`]）：`no_account`（这个工具一个账号都没有）、`no_dir`（文件夹没了）、
    /// `bad_args`（参数有问题，或者不是交互的会话）、`no_relay`（这个工具的账号的会话都不经中转：这台机器不在哪个 Team 里）；
    /// 用得了是 None
    pub fn problem(&self, picks: &[Pick]) -> R<Option<&'static str>> {
        if picks.is_empty() {
            return Ok(Some("no_account"));
        }
        if !path::isdir(&self.dir) {
            return Ok(Some("no_dir"));
        }
        if !args_problems(self.tool(), &self.args, &self.dir)?.is_empty() {
            return Ok(Some("bad_args"));
        }
        if picks.iter().all(|p| p.hub.is_none()) {
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

/// 起会话时能挑的一个账号（这个工具的账号表里的）
#[derive(Clone)]
pub struct Pick {
    pub tool: &'static Tool,
    /// 账号表的键
    pub key: String,
    /// 给人看的名字：别名，没起别名的是邮箱
    pub name: String,
    /// 它的会话走哪台服务器中转（relay.rs 的 `session_hub`；没有中转是 None，远程起不了）
    pub hub: Option<HubKind>,
}

impl Pick {
    /// 按配对算的编号（16 个十六进制字符；p 是这个配对的密钥）：同一个账号在这个配对里一直是它（换别名不变），和手机上「账号」页里的
    /// 那个账号的编号一样（status.rs）；别的配对、服务器看不出是哪个账号
    pub fn tag(&self, p: &[u8]) -> String {
        account_tag(p, self.tool.name, &acct_id(&self.key, self.tool))
    }
}

/// 这个工具的账号表里的账号，照账号表的先后，各自的会话走哪台服务器现查
pub fn picks(tool: &'static Tool) -> Vec<Pick> {
    let Some(table) = fs::load_quiet(&tool.table).and_then(|v| v.as_dict()) else { return Vec::new() };
    let keys: Vec<String> = table.keys().into_iter().map(|k| k.to_string()).collect();
    keys.into_iter().map(|key| Pick { tool, name: name_of(&key), hub: relay::session_hub(tool.name, &key), key }).collect()
}

/// Claude Code 的子命令（`claude mcp …` 这些不是会话）
const CLAUDE_SUBCOMMANDS: &[&str] = &["config", "mcp", "plugin", "migrate-installer", "setup-token", "doctor", "update", "install"];

/// 参数里有没有一句话：有不是选项、也不是选项的值的参数（values 是这个工具带一个值的选项：跳过它的值），或者 `--`（后面的都是话）
fn has_words(args: &[String], values: &[&str]) -> bool {
    let mut i = 0;
    while let Some(a) = args.get(i) {
        match a.as_str() {
            f if values.contains(&f) => i += 2,
            f if f.starts_with('-') && f.len() > 1 && f != "--" => i += 1,
            _ => return true,
        }
    }
    false
}

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

/// 给手机的说明（server/README.md「启动器」）：现在用得了的模板（文件夹还在、这个工具有会话走 hub 这台服务器的账号），最多 MAX 个；
/// 明文放不下 DOC_MAX 的不放、记在 `more` 里。起会话时能挑的账号（`accounts`）：模板用到的工具的、会话走 hub 的，一个工具最多
/// PICKS_MAX 个，各带按配对算的编号——封给每台手机之前各算各的（`for_peer`）。参数有没有问题这里不看（共享的设置变了它也跟着变）：
/// 起的时候再查
pub struct LaunchDoc {
    machine: String,
    rows: Vec<V>,
    more: usize,
    /// 能挑的账号，各带账号摘要（这个 Team 的盐算的；没有邮箱、还没有盐的没有）
    picks: Vec<(Pick, Option<String>)>,
}

impl LaunchDoc {
    pub fn build(hub: HubKind) -> LaunchDoc {
        let salt = team_salt(hub);
        let aid = |p: &Pick| salt.as_deref().and_then(|s| aid_of(s, p.tool.name, &p.key));
        let mut doc = LaunchDoc { machine: peers::machine_name(), rows: Vec::new(), more: 0, picks: Vec::new() };
        // 各个工具会话走 hub 的账号（一个工具只查一次）
        let mut here: HashMap<&'static str, Vec<Pick>> = HashMap::new();
        for t in load() {
            if !path::isdir(&t.dir) {
                continue;
            }
            let tool = t.tool();
            let mine = here
                .entry(tool.name)
                .or_insert_with(|| picks(tool).into_iter().filter(|p| p.hub == Some(hub)).take(PICKS_MAX).collect())
                .clone();
            if mine.is_empty() {
                continue;
            }
            let before = doc.picks.len();
            if !doc.picks.iter().any(|(p, _)| p.tool.name == tool.name) {
                doc.picks.extend(mine.iter().map(|p| (p.clone(), aid(p))));
            }
            let row = dict! {
                "i" => t.id.as_str(), "h" => t.digest(), "n" => clean(&t.name, NAME_MAX), "k" => t.tool.as_str(),
                "d" => clean(&home_tilde(&t.dir), 200), "x" => t.args_text(300), "l" => t.line_ok()
            };
            // 给旧版的 App、cca 看的账号（它们不认 accounts，起会话时也不说用哪个：能挑的只有一个时启动器就用它）
            match mine.as_slice() {
                [only] => {
                    row.set("a", clean(short_name(&only.name), 64));
                    if let Some(a) = aid(only) {
                        row.set("aid", a);
                    }
                }
                _ => row.set("a", ANY_ACCOUNT),
            }
            doc.rows.push(V::Dict(row));
            // more 最多两位数：按两位数算，放进去以后也不会超
            let more = std::mem::replace(&mut doc.more, 99);
            let fits = doc.text().len() <= DOC_MAX;
            doc.more = more;
            if !fits {
                doc.rows.pop();
                doc.picks.truncate(before);
                doc.more += 1;
            }
        }
        doc
    }

    /// 封给一台手机的那一份（p：和它配对的密钥）：`{"v":1,"machine","t":[模板…],"more","accounts":[{"i","k","n","aid"}…]}`
    pub fn for_peer(&self, p: &[u8]) -> Dict {
        let accounts: Vec<V> = self
            .picks
            .iter()
            .map(|(a, aid)| {
                let d = dict! { "i" => a.tag(p), "k" => a.tool.name, "n" => clean(short_name(&a.name), 64) };
                if let Some(aid) = aid {
                    d.set("aid", aid.as_str());
                }
                V::Dict(d)
            })
            .collect();
        dict! { "v" => 1, "machine" => self.machine.as_str(), "t" => self.rows.clone(), "more" => self.more, "accounts" => accounts }
    }

    /// 和配对无关的样子（变了才重发、量多长）：账号的编号用一把全是 0 的密钥算，账号变了它也跟着变，长度和真的一样
    pub fn text(&self) -> String {
        json::dumps(&V::Dict(self.for_peer(&[0u8; 32])), json::Fmt::new().utf8().compact())
    }
}

fn problem_text(code: &str) -> &'static str {
    match code {
        "no_account" => "这个工具还没有账号",
        "no_dir" => "文件夹不在了",
        "bad_args" => "参数用不了",
        "no_relay" => "这个工具的账号的会话都不经中转（先加入一个 Team）",
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

/// 列出来：一行一个（`编号  名字  工具  文件夹  参数`，用不了的后面说原因）；--json 是给界面 App 的（PROTOCOL 9.4）
fn list(args: &[String]) -> R<()> {
    let as_json = match args {
        [] => false,
        [a] if a == "--json" => true,
        _ => return die(USAGE),
    };
    let all = load();
    // 各个工具的账号（一个工具只查一次）
    let mut seen: HashMap<&'static str, Vec<Pick>> = HashMap::new();
    let mut problem = |t: &Template| t.problem(seen.entry(t.tool().name).or_insert_with(|| picks(t.tool())));
    if as_json {
        let rows = List::new();
        for t in &all {
            let problem = problem(t)?;
            // account、account_key、account_name：3.7.9 及以前模板带着账号，旧版的 App 读它们；现在总是空的
            rows.push(dict! {
                "id" => t.id.as_str(), "name" => t.name.as_str(), "tool" => t.tool.as_str(), "account" => "",
                "account_key" => V::None, "account_name" => V::None, "dir" => t.dir.as_str(),
                "args" => t.args_v(), "args_line" => t.args_line(), "line" => t.line_ok(), "digest" => t.digest(),
                "problem" => problem.map_or(V::None, V::from)
            });
        }
        let doc = dict! { "version" => 1, "kind" => "remote_templates", "templates" => rows, "launcher" => launch::summary() };
        crate::out!("{}", json::dumps(&V::Dict(doc), json::LINE));
        return Ok(());
    }
    for t in &all {
        let note = problem(t)?.map(|p| format!("  （{}）", problem_text(p))).unwrap_or_default();
        let args = if t.args.is_empty() { String::new() } else { format!("  {}", t.args_text(200)) };
        crate::out!("{}  {}  {}  {}{}{}", t.id, clean(&t.name, NAME_MAX), t.tool().label, home_tilde(&t.dir), args, note);
    }
    Ok(())
}

/// `add 名字 工具 文件夹 [--replace 编号] (-- 参数… | --args-line 一行参数)`：文件夹相对当前目录、可以带 ~，要已经在。--replace：改那一个
/// （编号、位置不变）。参数是一项一项的（`--` 后面），或者照 shell 的写法写成的一行（`--args-line`：App 给的，这里拆，不展开 ~、$ 这些）。
/// 账号不存在模板里：起会话时挑。stdout：`编号<TAB>名字`
fn add(args: &[String]) -> R<()> {
    // 照位置认（名字、文件夹、参数那一行本身是 `--` 也不会认错）
    let [name, tool, dir, rest @ ..] = args else { return die(USAGE) };
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
    let t = Template { id, name, tool: tool.name.to_string(), dir, args: argv.to_vec(), at: time() };
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
        // 照 server/README.md「启动器」：SHA256('["claude","/srv/项目",["--model","opus"]]')（账号不在里面：起的时候挑）
        assert_eq!(t.digest(), sha256_hex(r#"["claude","/srv/项目",["--model","opus"]]"#)[..16]);
        let mut u = t.clone();
        u.name = "别的名字".into();
        u.at = 9.0;
        assert_eq!(u.digest(), t.digest(), "名字、时间不算");
        let changes: [fn(&mut Template); 3] = [|t| t.args.push("-c".into()), |t| t.dir.push('x'), |t| t.tool = "codex".into()];
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
        assert!(!tpl("claude", &["--model", "opus", "说中文"]).line_ok(), "参数里已经有一句话");
        assert!(tpl("claude", &["--append-system-prompt", "说中文", "--model=opus", "-c"]).line_ok(), "选项的值不是话");
        assert!(!tpl("codex", &["-c", "a=1", "修一下"]).line_ok());
        assert!(tpl("codex", &["-c", "a=1", "--search"]).line_ok());
        assert!(!tpl("codex", &["resume"]).line_ok(), "子命令：再接一句就成了它的参数");
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
                "id" => "0123abcd", "name" => "x", "tool" => "codex", "dir" => "/tmp", "args" => crate::list!["a b", "c"], "at" => 2.0
            }
        };
        let t = row_of(&base()).unwrap();
        assert_eq!(t.args_text(100), "'a b' c");
        assert_eq!(t.args_text(4), "'a …");
        // 3.7.9 及以前存的带着账号：照样认，账号不理（存的时候也不再写）
        let old = base();
        old.set("account", "a@example.com");
        assert_eq!(row_of(&old), Some(t.clone()));
        for (k, bad) in
            [("id", V::from("0123ABCD")), ("tool", "vim".into()), ("dir", "rel".into()), ("args", crate::list![1].into())]
        {
            let e = base();
            e.set(k, bad);
            assert!(row_of(&e).is_none(), "{k}");
        }
    }

    #[test]
    fn accounts_in_the_launcher_doc() {
        let claude = tool_named("claude").unwrap();
        let pick = |key: &str, name: &str| Pick { tool: claude, key: key.into(), name: name.into(), hub: None };
        let doc = LaunchDoc {
            machine: "box".into(),
            rows: Vec::new(),
            more: 0,
            picks: vec![
                (pick("工作(alice@example.com)", "工作"), Some("ab".repeat(16))),
                (pick("bob@example.com", "bob@example.com"), None),
            ],
        };
        let (p1, p2) = ([1u8; 32], [2u8; 32]);
        let mine = doc.for_peer(&p1);
        let accts: Vec<Dict> = mine.list("accounts").unwrap().items().iter().map(|v| v.as_dict().unwrap()).collect();
        // 编号和手机上「账号」页里的同一个账号一样（status.rs：按配对、工具、账号目录名算），别的配对算出来的不一样
        let want = account_tag(&p1, "claude", &acct_id("工作(alice@example.com)", claude));
        assert_eq!(accts[0].str("i").as_deref(), Some(want.as_str()));
        assert!(peers::lower_hex(&want, 16));
        assert_ne!(doc.for_peer(&p2).list("accounts").unwrap().items()[0].as_dict().unwrap().str("i"), accts[0].str("i"));
        assert_eq!(pick("主力(alice@example.com)", "主力").tag(&p1), want, "按邮箱认：改别名不算");
        // 名字只给短的（没起别名的给邮箱 @ 前面那一截），摘要有才给
        assert_eq!((accts[0].str("n").as_deref(), accts[0].str("k").as_deref()), (Some("工作"), Some("claude")));
        assert_eq!(accts[0].str("aid").map(|s| s.len()), Some(32));
        assert_eq!((accts[1].str("n").as_deref(), accts[1].get("aid").is_none()), (Some("bob"), true));
        let text = doc.text();
        assert!(!text.contains("example.com") && text.contains(r#""more":0"#), "{text}");
    }
}
