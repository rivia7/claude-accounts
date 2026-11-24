//! Team 的配置整份导出、导入、修改（`cca team export|import|edit`，3.6.14 起；docs/dev/team.md「导出、导入、修改配置」）：管理员把
//! 这个 Team 的配置——账号走哪个节点、别名、锁定别名、安装客户端用的节点、节点的定义——写成一个 JSON 文件，或者照这样一个文件整份换掉
//! （先说清楚改了什么，bash 那边问过同意再换、发布）。文件的样子：
//! ```text
//! {"kind": "cca-team-config", "version": 1, "team": 导出时 Team 的名字（只给人看：导入时不管，名字照 cca team name 改）,
//!  "accounts": {工具: {邮箱: {"node": 节点, "alias": 别名}}}（值也可以只写节点；没写别名的：原来就在的照原来的，新加的编成 team-N）,
//!  "alias_lock": true | false, "install_node": 节点（没指定是 ""）,
//!  "nodes": {"group": 策略组, "proxies": [节点定义…]} | null}
//! ```
//! 节点的定义带着密钥（密码、uuid）：文件只给自己读写（600），和根密钥一样当密码保管。设备、每台设备能用哪些账号不在里面（它们是这个
//! Team 自己的：导到别的 Team 里没有意义），导入时照旧。写错了的、不认得的项都不导（说出是哪几处），免得改动悄悄丢了

use super::{admin_of, admin_save, alias_plan, default_aliases, fail, settle};
use crate::core::model::cfg_load;
use crate::core::policy::{alias_ok, fill_problems, in_nodes};
use crate::core::team::{team_path, Team};
use crate::core::tool::tools;
use crate::py::fs::{read_text, save_json};
use crate::py::hash::sha256_hex;
use crate::py::obj::{Dict, List, V};
use crate::py::{json, path, R};
use crate::{dict, out};
use std::rc::Rc;

/// 导出的文件开头的 kind（version 是 1）
const KIND: &str = "cca-team-config";
/// cca team edit 改的那一份：Team 的目录里（只给自己读写，cca 启动的 Claude Code 读不到，cca restore 的备份不带它），改完就删
const EDIT_FILE: &str = "edit.json";
/// 列改动时一种最多列几个（多了说「等 N 个」）
const LIST_MAX: usize = 6;
/// 文件不对时最多说几处
const PROBLEMS_MAX: usize = 8;
const ALIAS_RULE: &str = "别名不能是空的，不能有空格、引号、括号、反斜杠或 @，最多 12 个汉字或 24 个字母";
const USAGE: &str =
    "用法: py team-config export Team 文件 [--force] | export Team --edit | check Team 文件 [--base 摘要] | testconfig 文件 | apply Team 文件 [--sha 校验和]";

/// 文件里的配置（查过、规整过）
struct Parsed {
    /// {工具: {邮箱: 节点}}：邮箱小写，照文件里的先后
    accounts: Dict,
    /// {工具: {邮箱: 别名}}：文件里写了别名的
    aliases: Dict,
    alias_lock: bool,
    install_node: String,
    /// {"group", "proxies"} | null
    nodes: V,
}

/// py team-config export Team 文件 [--force] | export Team --edit | check Team 文件 [--base 摘要] | testconfig 文件 | apply Team 文件 [--sha 校验和]。
/// 原因码：no_team、not_admin、usage、file（写不了、读不了、内容不对：原因里一一说出来；apply 时文件和确认时的不一样了）、server、publish（apply
/// 发布没成：配置已经换了，照 cca team publish 再发）
pub fn cmd_team_config(args: &[String]) -> R<()> {
    match args.first().map(|s| s.as_str()) {
        Some("export") => export(&args[1..]),
        Some("check") => check(&args[1..]),
        Some("testconfig") => testconfig(&args[1..]),
        Some("apply") => apply(&args[1..]),
        _ => fail("usage", USAGE),
    }
}

/// export：把这个 Team 的配置写进文件（600；--edit 是 Team 的目录里给 cca team edit 改的那一份，原来有就换掉）。文件已经有了不覆盖，
/// 除非 --force（App 的保存面板问过了）。stdout 一行一项：文件的路径、Team 的编号、名字、几个账号、几个节点、配置的摘要（check 的 --base）
fn export(args: &[String]) -> R<()> {
    let (sel, dest, force) = match args {
        [s, flag] if flag == "--edit" => (s, None, true),
        [s, f] => (s, Some(f), false),
        [s, f, flag] if flag == "--force" => (s, Some(f), true),
        _ => return fail("usage", "用法: cca team export Team 文件 [--force]"),
    };
    let (t, a, _lk) = admin_of(sel)?;
    let file = match dest {
        None => team_path(&t.id, EDIT_FILE),
        Some(f) => {
            let file = path::abspath(&path::expanduser(f));
            let dir = path::dirname(&file);
            if path::isdir(&file) {
                return fail("file", &format!("「{file}」是一个文件夹：写一个文件名"));
            }
            if path::lexists(&file) && !force {
                return fail("file", &format!("「{file}」已经有了：换一个文件名，或者加 --force 覆盖它"));
            }
            if !path::isdir(&dir) {
                return fail("file", &format!("没有「{dir}」这个文件夹"));
            }
            file
        }
    };
    let doc = export_doc(&t, &a);
    // 不跟着链接写：文件里有节点的密钥，别让放在那里的链接把它引到别处
    if let Err(e) = save_json(&file, &V::Dict(doc.clone()), Some(0o600), false) {
        return fail("file", &format!("写不了「{file}」（{e}）"));
    }
    let accounts: usize = doc.dict("accounts").map_or(0, |m| m.values().iter().filter_map(|x| x.as_dict()).map(|x| x.len()).sum());
    out!("{file}\n{}\n{}\n{accounts}\n{}\n{}", t.id, t.name, proxy_defs(&doc.at("nodes")).len(), config_digest(&a));
    Ok(())
}

/// check：照文件换掉的话会改什么（不改）。stdout 一行一项：Team 的编号、名字、改了几项（0 是一样）、去掉几个账号、文件的 SHA-256（apply 的
/// --sha）、新加和改了的节点定义有几个（不是 0 时 bash 先拿内核校验 testconfig 给的配置），再往下是给人看的说明，一行一种改动；「! 」开头的是
/// 提醒（不算改动）。--base：export 时的摘要，这中间配置在别处改过的话提醒一句（cca team edit 用）
fn check(args: &[String]) -> R<()> {
    let (sel, file, base) = match args {
        [s, f] => (s, f, None),
        [s, f, flag, d] if flag == "--base" => (s, f, Some(d.as_str())),
        _ => return fail("usage", "用法: cca team import Team 文件"),
    };
    let (t, a, _lk) = admin_of(sel)?;
    let (p, sha) = read_parsed(file)?;
    let b = candidate(&a, &p);
    let here = cfg_load().ok().and_then(|c| c.list("nodes"));
    let mut ch = changes(&a, &b, &here);
    if base.is_some_and(|d| d != config_digest(&a)) {
        ch.lines.push(format!("! 打开以后「{}」的配置在别处改过（App 里、别的命令）：照这份换掉的话，那些改动就没了", t.name));
    }
    out!("{}\n{}\n{}\n{}\n{sha}\n{}", t.id, t.name, ch.count, ch.removed, ch.defs);
    for l in &ch.lines {
        out!("{l}");
    }
    Ok(())
}

/// testconfig：拿文件里全部的节点定义拼一份内核的最小配置（和 node-testconfig 一个样子：不监听端口、全都 REJECT），bash 拿内核 -t 校验
/// （和 cca team node add 一样：内核认不出的节点发出去，各台设备的路由器、内置代理整份配置都载不进去）。一起校验：dialer-proxy 连着的也在里面。
/// stdout 有节点的密钥：bash 写进只有自己能读的临时目录
fn testconfig(args: &[String]) -> R<()> {
    let [file] = args else { return fail("usage", USAGE) };
    let (p, _) = read_parsed(file)?;
    let px = p.nodes.as_dict().map_or(V::None, |n| n.at("proxies"));
    let names: Vec<V> = proxy_defs(&p.nodes).into_iter().map(|(n, _)| V::from(n.as_str())).collect();
    if names.is_empty() {
        return fail("usage", "这份配置里没有节点的定义");
    }
    let cfg = dict! {
        "allow-lan" => false, "bind-address" => "127.0.0.1", "mode" => "rule", "log-level" => "silent",
        "proxies" => px,
        "proxy-groups" => List::from_vec(vec![V::Dict(dict! { "name" => "CCA-TEST", "type" => "select", "proxies" => List::from_vec(names) })]),
        "rules" => List::from_vec(vec![V::from("MATCH,REJECT")]),
    };
    out!("{}", json::dumps(&V::Dict(cfg), json::LINE));
    Ok(())
}

/// apply：照文件整份换掉这个 Team 的配置，再发布（接上了服务器才发）。--sha：check 时文件的 SHA-256（问过同意的是那一份：中间改过了不换）。
/// stdout：存上了几台的配置
fn apply(args: &[String]) -> R<()> {
    let (sel, file, want) = match args {
        [s, f] => (s, f, None),
        [s, f, flag, h] if flag == "--sha" => (s, f, Some(h.as_str())),
        _ => return fail("usage", "用法: cca team import Team 文件"),
    };
    let (t, a, _lk) = admin_of(sel)?;
    let (p, sha) = read_parsed(file)?;
    if want.is_some_and(|h| h != sha) {
        return fail("file", &format!("「{}」在确认以后又改过了：没有换，重新导入一次", path::abspath(&path::expanduser(file))));
    }
    let b = candidate(&a, &p);
    a.set("config", b.at("config"));
    a.set("alias_n", b.at("alias_n"));
    admin_save(&t.id, &a)?;
    let stored = if t.attached() { settle(&t, &a, None)? } else { 0 };
    out!("{stored}");
    Ok(())
}

/// 配置的摘要（只用来看中间有没有在别处改过）
fn config_digest(a: &Dict) -> String {
    sha256_hex(json::dumps(&a.at("config"), json::Fmt::new().compact().sort_keys()))[..16].to_string()
}

/// 导出的内容：各工具都列出来（没有账号的是 {}，照着加就行），每个账号带着别名（没起过的是默认编的 team-N）
fn export_doc(t: &Team, a: &Dict) -> Dict {
    let config = a.dict("config").unwrap_or_default();
    let names = alias_plan(a).by_tool;
    let all = config.dict("accounts").unwrap_or_default();
    let accounts = Dict::new();
    for tool in tools().iter() {
        let (m, n, rows) = (all.dict(tool.name).unwrap_or_default(), names.dict(tool.name).unwrap_or_default(), Dict::new());
        for (e, node) in m.items() {
            let row = dict! { "node" => node };
            if let Some(x) = n.get(&e) {
                row.set("alias", x);
            }
            rows.set(&*e, row);
        }
        accounts.set(tool.name, rows);
    }
    dict! {
        "kind" => KIND, "version" => 1, "team" => t.name.as_str(), "accounts" => accounts,
        "alias_lock" => matches!(config.get("alias_lock"), Some(V::Bool(true))),
        "install_node" => config.str("install_node").map_or(String::new(), |n| n.to_string()),
        "nodes" => config.get("nodes").filter(|n| n.as_dict().is_some()).unwrap_or(V::None),
    }
}

/// 读文件、查一遍；不对时 fail("file")，原因里一处处说（最多 PROBLEMS_MAX 处）。连同文件的 SHA-256
fn read_parsed(file: &str) -> R<(Parsed, String)> {
    let f = path::abspath(&path::expanduser(file));
    let Ok(text) = read_text(&f) else { return fail("file", &format!("读不了「{f}」")) };
    let doc = match json::loads(&text) {
        Ok(v) => v,
        Err(e) => return fail("file", &format!("「{f}」不是合法的 JSON（{e}）")),
    };
    match parse(&doc) {
        Ok(p) => Ok((p, sha256_hex(text.as_bytes()))),
        Err(errs) => {
            let more = errs.len().saturating_sub(PROBLEMS_MAX);
            let mut why = errs.into_iter().take(PROBLEMS_MAX).collect::<Vec<_>>().join("；");
            if more > 0 {
                why += &format!("；还有 {more} 处");
            }
            fail("file", &format!("「{f}」里有不对的地方：{why}"))
        }
    }
}

fn email_ok(e: &str) -> bool {
    e.contains('@') && !e.starts_with('@') && !e.ends_with('@') && !e.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// 查文件的内容，规整（邮箱小写、去掉首尾空格）；不对的地方一处一句
fn parse(doc: &V) -> Result<Parsed, Vec<String>> {
    let Some(d) = doc.as_dict() else { return Err(vec!["不是 JSON 对象".into()]) };
    if d.str("kind").as_deref() != Some(KIND) {
        return Err(vec![format!("不是 cca team export 导出的 Team 配置（开头要有 \"kind\": \"{KIND}\"）")]);
    }
    match d.get("version") {
        Some(V::Int(1)) => {}
        Some(V::Int(n)) if n > 1 => {
            return Err(vec![format!("这份配置是新版本的 cca 导出的（version {n}）：先更新 cca（cca update）")])
        }
        _ => return Err(vec!["version 应该是 1".into()]),
    }
    let mut errs: Vec<String> = Vec::new();
    for k in d.keys() {
        if !["kind", "version", "team", "accounts", "alias_lock", "install_node", "nodes"].contains(&&*k) {
            errs.push(format!("不认得「{k}」（能写的是 accounts、alias_lock、install_node、nodes）"));
        }
    }
    let (accounts, aliases) = (Dict::new(), Dict::new());
    match d.get("accounts") {
        Some(V::Dict(m)) => {
            for (tool, v) in m.items() {
                let Some(t) = tools().iter().find(|t| t.name == &*tool) else {
                    let known: Vec<&str> = tools().iter().map(|t| t.name).collect();
                    errs.push(format!("accounts 里的「{tool}」不是工具（能写的是 {}）", known.join("、")));
                    continue;
                };
                let Some(v) = v.as_dict() else {
                    errs.push(format!("accounts.{tool} 应该是 {{邮箱: 节点或者 {{\"node\": …, \"alias\": …}}}}"));
                    continue;
                };
                let (nodes, names) = (Dict::new(), Dict::new());
                for (raw, x) in v.items() {
                    let email = raw.trim().to_lowercase();
                    let place = format!("{} 的 {raw}", t.label);
                    if !email_ok(&email) {
                        errs.push(format!("{place}：不是邮箱"));
                        continue;
                    }
                    if nodes.has(&email) {
                        errs.push(format!("{place}：写了两次"));
                        continue;
                    }
                    let (node, alias) = match &x {
                        V::Str(s) => (s.trim().to_string(), None),
                        V::Dict(o) => {
                            for k in o.keys().iter().filter(|k| !["node", "alias"].contains(&&***k)) {
                                errs.push(format!("{place}：不认得「{k}」（能写的是 node、alias）"));
                            }
                            (o.str("node").map_or(String::new(), |n| n.trim().to_string()), o.get("alias").filter(|a| !a.is_none()))
                        }
                        _ => {
                            errs.push(format!("{place}：应该写节点的名字，或者 {{\"node\": 节点, \"alias\": 别名}}"));
                            continue;
                        }
                    };
                    if node.is_empty() || node.chars().any(char::is_control) {
                        errs.push(format!("{place}：没写走哪个节点（node）"));
                        continue;
                    }
                    if node == "-" {
                        errs.push(format!("{place}：节点不能写 -（不要这个账号就把这一行删掉）"));
                        continue;
                    }
                    if let Some(a) = alias {
                        if !alias_ok(&a) {
                            errs.push(format!("{place}：别名 {} 不行（{ALIAS_RULE}）", a.py_repr()));
                            continue;
                        }
                        let a = a.py_str();
                        if let Some((other, _)) = names.items().into_iter().find(|(_, x)| x.as_str() == Some(a.as_str())) {
                            errs.push(format!("{} 里 {other} 和 {email} 都叫「{a}」", t.label));
                            continue;
                        }
                        names.set(email.as_str(), a.as_str());
                    }
                    nodes.set(email.as_str(), node.as_str());
                }
                accounts.set(t.name, nodes);
                if !names.is_empty() {
                    aliases.set(t.name, names);
                }
            }
        }
        None => errs.push("没有 accounts（一个账号都不要时写 \"accounts\": {}）".into()),
        Some(_) => errs.push("accounts 应该是 {工具: {邮箱: …}}".into()),
    }
    let alias_lock = match d.get("alias_lock") {
        None | Some(V::None) => false,
        Some(V::Bool(b)) => b,
        Some(_) => {
            errs.push("alias_lock 应该是 true 或者 false".into());
            false
        }
    };
    let nodes = match d.get("nodes") {
        None | Some(V::None) => V::None,
        Some(V::Dict(n)) => {
            for k in n.keys().iter().filter(|k| !["group", "proxies"].contains(&&***k)) {
                errs.push(format!("nodes 里不认得「{k}」（能写的是 group、proxies）"));
            }
            match n.get("proxies") {
                Some(V::List(l)) if l.is_empty() => V::None,
                px => {
                    let probe =
                        V::Dict(dict! { "version" => 1, "group" => n.at("group"), "proxies" => px.clone().unwrap_or(V::None) });
                    let probs = fill_problems(&probe);
                    errs.extend(probs.iter().map(|p| format!("nodes：{p}")));
                    if probs.is_empty() {
                        V::Dict(dict! { "group" => n.at("group"), "proxies" => px.unwrap_or(V::None) })
                    } else {
                        V::None
                    }
                }
            }
        }
        Some(_) => {
            errs.push("nodes 应该是 {\"group\": 策略组, \"proxies\": [节点定义…]}，没有节点写 null".into());
            V::None
        }
    };
    let install_node = match d.get("install_node") {
        None | Some(V::None) => String::new(),
        Some(V::Str(s)) => s.trim().to_string(),
        Some(_) => {
            errs.push("install_node 应该是节点的名字（不指定写 \"\"）".into());
            String::new()
        }
    };
    if !install_node.is_empty() && !proxy_defs(&nodes).iter().any(|(n, _)| *n == install_node) {
        errs.push(format!("install_node「{install_node}」不是 nodes 里定义的节点（安装客户端用的只能是 Team 的节点）"));
    }
    if errs.is_empty() {
        Ok(Parsed { accounts, aliases, alias_lock, install_node, nodes })
    } else {
        Err(errs)
    }
}

/// 照文件换掉配置以后的 admin.json（深拷贝，a 不动）。账号照原来的先后：还在的在前，新加的照文件里的先后接在后面（各台设备的配置不会
/// 只因为先后变了就重新发布）。别名：文件里写了的照它；没写的、原来就在的留着原来的（和写了的撞了就不留），剩下的 default_aliases 编成 team-N
fn candidate(a: &Dict, p: &Parsed) -> Dict {
    let b = V::Dict(a.clone()).deepcopy().as_dict().unwrap_or_default();
    let config = b.dict("config").unwrap_or_default();
    let (old_acc, old_names) = (config.dict("accounts").unwrap_or_default(), alias_plan(a).by_tool);
    let (accounts, by_tool) = (Dict::new(), Dict::new());
    for tool in tools().iter() {
        let (want, given) = (p.accounts.dict(tool.name).unwrap_or_default(), p.aliases.dict(tool.name).unwrap_or_default());
        let before: Vec<Rc<str>> = old_acc.dict(tool.name).map(|m| m.keys()).unwrap_or_default();
        let order: Vec<Rc<str>> =
            before.iter().filter(|e| want.has(e)).cloned().chain(want.keys().into_iter().filter(|e| !before.contains(e))).collect();
        let (m, names, old) = (Dict::new(), Dict::new(), old_names.dict(tool.name).unwrap_or_default());
        let taken: Vec<String> = given.values().iter().map(|x| x.py_str()).collect();
        for e in &order {
            m.set(&**e, want.at(e));
            match (given.get(e), old.get(e)) {
                (Some(x), _) => names.set(&**e, x),
                (None, Some(x)) if !taken.contains(&x.py_str()) => names.set(&**e, x),
                _ => {}
            }
        }
        if !m.is_empty() {
            accounts.set(tool.name, m);
        }
        if !names.is_empty() {
            by_tool.set(tool.name, names);
        }
    }
    config.set("accounts", accounts);
    config.set("tool_aliases", by_tool);
    if p.alias_lock {
        config.set("alias_lock", true);
    } else {
        config.pop("alias_lock");
    }
    if !p.install_node.is_empty() || config.has("install_node") {
        config.set("install_node", p.install_node.as_str());
    }
    config.set("nodes", p.nodes.clone());
    b.set("config", config);
    default_aliases(&b);
    b
}

/// 照文件换掉会改的：给人看的说明（一种一行，「! 」开头的是提醒）、改了几项、去掉几个账号、新加和改了的节点定义有几个
struct Changes {
    lines: Vec<String>,
    count: usize,
    removed: usize,
    defs: usize,
}

/// 节点的定义：[(名字, 整个定义排好键的 JSON)]
fn proxy_defs(nodes: &V) -> Vec<(String, String)> {
    let px = nodes.as_dict().and_then(|n| n.list("proxies")).map(|l| l.items()).unwrap_or_default();
    px.iter()
        .filter_map(|p| p.as_dict())
        .filter_map(|p| {
            p.str("name").map(|n| (n.to_string(), json::dumps(&V::Dict(p.clone()), json::Fmt::new().compact().sort_keys())))
        })
        .collect()
}

/// 前 LIST_MAX 个用「、」连起来，多了说一共几个
fn listed(v: &[String]) -> String {
    let head = v.iter().take(LIST_MAX).cloned().collect::<Vec<_>>().join("、");
    if v.len() > LIST_MAX {
        format!("{head} 等 {} 个", v.len())
    } else {
        head
    }
}

/// a（原来的）换成 b（candidate 算的）会改什么。here：这台设备的可选节点（config.json 的 nodes；没有是 None，不提醒节点的事：说不准）
fn changes(a: &Dict, b: &Dict, here: &Option<List>) -> Changes {
    let (oc, nc) = (a.dict("config").unwrap_or_default(), b.dict("config").unwrap_or_default());
    let (oa, na) = (oc.dict("accounts").unwrap_or_default(), nc.dict("accounts").unwrap_or_default());
    let (old_names, new_names) = (alias_plan(a).by_tool, nc.dict("tool_aliases").unwrap_or_default());
    let (np, op) = (proxy_defs(&nc.at("nodes")), proxy_defs(&oc.at("nodes")));
    let (mut added, mut removed, mut moved, mut renamed) = (vec![], vec![], vec![], vec![]);
    // 新加的、换了节点的账号走的节点
    let mut wanted: Vec<String> = Vec::new();
    for tool in tools().iter() {
        let (om, nm) = (oa.dict(tool.name).unwrap_or_default(), na.dict(tool.name).unwrap_or_default());
        let (on, nn) = (old_names.dict(tool.name).unwrap_or_default(), new_names.dict(tool.name).unwrap_or_default());
        for (e, n) in nm.items() {
            let n = n.py_str();
            let Some(o) = om.str(&e) else {
                added.push(format!("{} {e}（{n}）", tool.label));
                wanted.push(n);
                continue;
            };
            if *o != *n {
                moved.push(format!("{} {e}：{o} → {n}", tool.label));
                wanted.push(n);
            }
            let (x, y) =
                (on.str(&e).map(|s| s.to_string()).unwrap_or_default(), nn.str(&e).map(|s| s.to_string()).unwrap_or_default());
            if x != y {
                renamed.push(format!("{} {e}：{x} → {y}", tool.label));
            }
        }
        removed.extend(om.keys().iter().filter(|e| !nm.has(e)).map(|e| format!("{} {e}", tool.label)));
    }
    let mut lines = Vec::new();
    if !added.is_empty() {
        lines.push(format!("加 {} 个账号：{}", added.len(), listed(&added)));
    }
    if !removed.is_empty() {
        lines.push(format!("去掉 {} 个账号：{}（各台设备上收起来，登录留着）", removed.len(), listed(&removed)));
    }
    if !moved.is_empty() {
        lines.push(format!("换节点：{}", listed(&moved)));
    }
    if !renamed.is_empty() {
        lines.push(format!("改别名：{}", listed(&renamed)));
    }
    let n_add: Vec<String> = np.iter().filter(|(n, _)| !op.iter().any(|(o, _)| o == n)).map(|(n, _)| n.clone()).collect();
    let n_mod: Vec<String> =
        np.iter().filter(|(n, d)| op.iter().any(|(o, od)| o == n && od != d)).map(|(n, _)| n.clone()).collect();
    let n_rm: Vec<String> = op.iter().filter(|(o, _)| !np.iter().any(|(n, _)| n == o)).map(|(o, _)| o.clone()).collect();
    let parts: Vec<String> = [("加", &n_add), ("改", &n_mod), ("去掉", &n_rm)]
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(w, v)| format!("{w} {}", listed(v)))
        .collect();
    if !parts.is_empty() {
        lines.push(format!("节点的定义：{}", parts.join("；")));
    }
    let group = |c: &Dict| c.dict("nodes").map(|n| n.at("group").py_str()).unwrap_or_default();
    let regroup = !op.is_empty() && !np.is_empty() && group(&oc) != group(&nc);
    if regroup {
        lines.push(format!("节点的策略组：{} → {}", group(&oc), group(&nc)));
    }
    let locked = |c: &Dict| matches!(c.get("alias_lock"), Some(V::Bool(true)));
    let relock = locked(&oc) != locked(&nc);
    if relock {
        let say = if locked(&nc) {
            "锁定别名（各台设备上照 Team 给的别名用，自己改不了）"
        } else {
            "解开别名（各台设备上又能自己改）"
        };
        lines.push(say.into());
    }
    let install = |c: &Dict| c.str("install_node").map_or(String::new(), |n| n.to_string());
    let reinstall = install(&oc) != install(&nc);
    if reinstall {
        let say = |n: String| if n.is_empty() { "不指定".to_string() } else { n };
        lines.push(format!("安装客户端用的节点：{} → {}", say(install(&oc)), say(install(&nc))));
    }
    let count =
        [added.len(), removed.len(), moved.len(), renamed.len(), n_add.len(), n_mod.len(), n_rm.len()].iter().sum::<usize>()
            + usize::from(regroup)
            + usize::from(relock)
            + usize::from(reinstall);
    if here.is_some() {
        let mut said: Vec<&str> = Vec::new();
        for n in &wanted {
            if !np.iter().any(|(x, _)| x == n) && !in_nodes(here, n) && !said.contains(&n.as_str()) {
                said.push(n);
                lines.push(format!("! 节点「{n}」：这份配置里没有它的定义，这台设备上也没有：各台设备上要有同名的节点才连得上"));
            }
        }
        for n in n_add.iter().filter(|n| in_nodes(here, n)) {
            lines.push(format!("! 节点「{n}」和这台设备上已有的节点同名：有同名节点的设备照它自己的那个走，不用这份配置里的定义"));
        }
    }
    Changes { lines, count, removed: removed.len(), defs: n_add.len() + n_mod.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> V {
        json::loads(s).expect("测试数据是 JSON")
    }

    fn parsed(s: &str) -> Parsed {
        parse(&d(s)).expect("合格")
    }

    fn errs(s: &str) -> Vec<String> {
        parse(&d(s)).err().unwrap_or_default()
    }

    fn js(v: &V) -> String {
        json::dumps(v, json::Fmt::new().utf8())
    }

    #[test]
    fn parse_normalizes_and_rejects() {
        let p = parsed(
            r#"{"kind": "cca-team-config", "version": 1, "team": "x",
            "accounts": {"claude": {" Alice@Example.com ": {"node": " JP-A ", "alias": "小王"}, "bob@example.com": "JP-B"}, "codex": {}},
            "alias_lock": true, "install_node": "", "nodes": null}"#,
        );
        assert_eq!(
            js(&V::Dict(p.accounts.clone())),
            r#"{"claude": {"alice@example.com": "JP-A", "bob@example.com": "JP-B"}, "codex": {}}"#
        );
        assert_eq!(js(&V::Dict(p.aliases.clone())), r#"{"claude": {"alice@example.com": "小王"}}"#);
        assert!(p.alias_lock && p.install_node.is_empty() && p.nodes.is_none());
        assert_eq!(
            errs(r#"{"kind": "team_devices", "version": 1}"#),
            ["不是 cca team export 导出的 Team 配置（开头要有 \"kind\": \"cca-team-config\"）"]
        );
        assert_eq!(
            errs(r#"{"kind": "cca-team-config", "version": 2, "accounts": {}}"#),
            ["这份配置是新版本的 cca 导出的（version 2）：先更新 cca（cca update）"]
        );
        assert_eq!(errs(r#"{"kind": "cca-team-config", "version": 1}"#), ["没有 accounts（一个账号都不要时写 \"accounts\": {}）"]);
        let e = errs(
            r#"{"kind": "cca-team-config", "version": 1, "nodez": null, "alias_lock": "yes",
            "accounts": {"gemini": {}, "claude": {"nope": "JP-A", "a@x.com": {"node": "JP-A", "alais": "y"}, "b@x.com": {"alias": "b"},
                         "c@x.com": {"node": "JP-A", "alias": "a b"}, "d@x.com": {"node": "JP-A", "alias": "同名"}, "e@x.com": {"node": "JP-A", "alias": "同名"},
                         "A@x.com": "JP-B", "f@x.com": "-"}}}"#,
        );
        assert_eq!(
            e,
            [
                "不认得「nodez」（能写的是 accounts、alias_lock、install_node、nodes）",
                "accounts 里的「gemini」不是工具（能写的是 claude、codex、agy）",
                "Claude 的 nope：不是邮箱",
                "Claude 的 a@x.com：不认得「alais」（能写的是 node、alias）",
                "Claude 的 b@x.com：没写走哪个节点（node）",
                "Claude 的 c@x.com：别名 'a b' 不行（别名不能是空的，不能有空格、引号、括号、反斜杠或 @，最多 12 个汉字或 24 个字母）",
                "Claude 里 d@x.com 和 e@x.com 都叫「同名」",
                "Claude 的 A@x.com：写了两次",
                "Claude 的 f@x.com：节点不能写 -（不要这个账号就把这一行删掉）",
                "alias_lock 应该是 true 或者 false",
            ]
        );
        let e = errs(
            r#"{"kind": "cca-team-config", "version": 1, "accounts": {}, "install_node": "JP-C",
            "nodes": {"group": "AI", "proxies": [{"name": "JP-A", "type": "socks5", "server": "192.0.2.10", "port": 1080}], "extra": 1}}"#,
        );
        assert_eq!(
            e,
            [
                "nodes 里不认得「extra」（能写的是 group、proxies）",
                "install_node「JP-C」不是 nodes 里定义的节点（安装客户端用的只能是 Team 的节点）"
            ]
        );
        let e = errs(
            r#"{"kind": "cca-team-config", "version": 1, "accounts": {}, "nodes": {"group": "AI", "proxies": [{"name": "JP-A"}]}}"#,
        );
        assert!(!e.is_empty() && e.iter().all(|e| e.starts_with("nodes：")), "{e:?}");
        let p = parsed(r#"{"kind": "cca-team-config", "version": 1, "accounts": {}, "nodes": {"group": "AI", "proxies": []}}"#);
        assert!(p.nodes.is_none(), "一个节点都没有就是 null");
    }

    #[test]
    fn candidate_keeps_order_and_aliases() {
        let a = d(r#"{"alias_n": {"claude": 2}, "config": {"accounts": {"claude": {"a@x.com": "JP-A", "b@x.com": "JP-B"}},
            "tool_aliases": {"claude": {"a@x.com": "team-1", "b@x.com": "小王"}}, "aliases": {}, "nodes": null}}"#)
        .as_dict()
        .expect("对象");
        // 先后变了（照原来的先后，新加的接在后面）、b 只写了节点（别名照原来的）、新加的 c 编成 team-3（用过的号不再用）
        let p = parsed(
            r#"{"kind": "cca-team-config", "version": 1,
            "accounts": {"claude": {"c@x.com": "JP-A", "b@x.com": "JP-B", "a@x.com": {"node": "JP-B", "alias": "team-1"}}}}"#,
        );
        let b = candidate(&a, &p);
        let c = b.dict("config").expect("有");
        assert_eq!(js(&c.at("accounts")), r#"{"claude": {"a@x.com": "JP-B", "b@x.com": "JP-B", "c@x.com": "JP-A"}}"#);
        assert_eq!(js(&c.at("tool_aliases")), r#"{"claude": {"a@x.com": "team-1", "b@x.com": "小王", "c@x.com": "team-3"}}"#);
        assert_eq!(js(&b.at("alias_n")), r#"{"claude": 3}"#);
        assert_eq!(
            a.dict("config").and_then(|c| c.dict("accounts")).and_then(|m| m.dict("claude")).map(|m| m.len()),
            Some(2),
            "a 不动"
        );
        let ch = changes(&a, &b, &None);
        assert_eq!(
            ch.lines,
            ["加 1 个账号：Claude c@x.com（JP-A）", "换节点：Claude a@x.com：JP-A → JP-B"],
            "不知道这台设备的节点：不提醒"
        );
        assert_eq!((ch.count, ch.removed, ch.defs), (2, 0, 0));
        // d 写的别名是 b 原来的：b 换成默认编的；d 走的节点哪儿都没有：提醒
        let p = parsed(
            r#"{"kind": "cca-team-config", "version": 1,
            "accounts": {"claude": {"a@x.com": "JP-A", "b@x.com": "JP-B", "d@x.com": {"node": "JP-Z", "alias": "小王"}}}, "alias_lock": true}"#,
        );
        let b = candidate(&a, &p);
        let ch = changes(&a, &b, &Some(List::from_vec(vec![V::from("JP-A"), V::from("JP-B")])));
        assert_eq!(
            ch.lines,
            [
                "加 1 个账号：Claude d@x.com（JP-Z）",
                "改别名：Claude b@x.com：小王 → team-3",
                "锁定别名（各台设备上照 Team 给的别名用，自己改不了）",
                "! 节点「JP-Z」：这份配置里没有它的定义，这台设备上也没有：各台设备上要有同名的节点才连得上",
            ]
        );
        assert_eq!((ch.count, ch.removed), (3, 0));
        // 去掉一个账号、加两个节点的定义（JP-A 和这台设备上的同名）、指定安装用的节点
        let p = parsed(
            r#"{"kind": "cca-team-config", "version": 1, "accounts": {"claude": {"a@x.com": "JP-A"}}, "install_node": "JP-C",
            "nodes": {"group": "AI", "proxies": [{"name": "JP-A", "type": "socks5", "server": "192.0.2.10", "port": 1080},
                                                 {"name": "JP-C", "type": "socks5", "server": "192.0.2.10", "port": 1081}]}}"#,
        );
        let b = candidate(&a, &p);
        let ch = changes(&a, &b, &Some(List::from_vec(vec![V::from("JP-A")])));
        assert_eq!(
            ch.lines,
            [
                "去掉 1 个账号：Claude b@x.com（各台设备上收起来，登录留着）",
                "节点的定义：加 JP-A、JP-C",
                "安装客户端用的节点：不指定 → JP-C",
                "! 节点「JP-A」和这台设备上已有的节点同名：有同名节点的设备照它自己的那个走，不用这份配置里的定义",
            ]
        );
        assert_eq!((ch.count, ch.removed, ch.defs), (4, 1, 2));
        assert_eq!(js(&b.at("config").as_dict().expect("有").at("tool_aliases")), r#"{"claude": {"a@x.com": "team-1"}}"#);
        // 和自己比：什么都没改
        assert_eq!(changes(&b, &candidate(&b, &p), &None).count, 0);
        assert_eq!(listed(&(1..=8).map(|i| i.to_string()).collect::<Vec<_>>()), "1、2、3、4、5、6 等 8 个");
    }
}
