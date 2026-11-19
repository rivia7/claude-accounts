//! 账号表和 config.json 上的那些小命令：init、keys、resolve、validate、get / set、ports、digest、router-env……
//! 以及映射的几个查询。对应 PY_SRC「命令」一节开头和 `cmd_get` 那一段。

use super::{arg, arity, Cmd};
use crate::core::base::*;
use crate::core::ctx::*;
use crate::core::model::*;
use crate::core::policy::*;
use crate::core::sync::sync_digest;
use crate::core::tool::{agy, claude, t, tools};
use crate::py::fs::{self, load_quiet, save_json};
use crate::py::io;
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::text;
use crate::py::{die, exit, Fail, R};
use crate::{dict, err, out};
use std::collections::HashSet;

pub fn cmd_init(args: &[String]) -> R<()> {
    arity("cmd_init", args, 0, 0)?;
    fs::makedirs(&CCA_HOME)?;
    if !path::exists(&t().table) {
        save_json(&t().table, &V::Dict(Dict::new()), Some(0o600), true)?;
    }
    ctx()?;
    Ok(())
}

/// `need(t, key)`：账号表里没有这个键就报错退出
pub fn need(table: &Dict, key: &str) -> R<()> {
    if table.has(key) {
        Ok(())
    } else {
        die(format!("没有账号: {key}"))
    }
}

pub fn cmd_router_env(args: &[String]) -> R<()> {
    arity("cmd_router_env", args, 0, 0)?;
    let c = cfg_load()?;
    let r = c.dict("router").unwrap_or_default();
    let or = |v: Option<V>, d: V| v.filter(|x| x.truthy()).unwrap_or(d);
    let host = r.get("host").unwrap_or_else(|| "".into());
    sh_vals(&[
        ("ROUTER_HOST", host.clone()),
        ("ROUTER_PORT", or(r.get("port"), 22.into())),
        ("ROUTER_USER", or(r.get("user"), "root".into())),
        ("ROUTER_KEY", r.get("key").unwrap_or_else(|| "".into())),
        ("PROXY_HOST", or(c.get("proxy_host"), host)),
        ("ROUTE_PREF", or(c.get("route"), "auto".into())),
        ("POOL_FIRST", FIRST_PORT.into()),
        // 配置过本机的客户端（内置代理，见 load_local）
        ("LOCAL_SET", c.dict("local").map_or("".into(), |loc| or(loc.get("client"), "".into()))),
    ]);
    Ok(())
}

pub fn cmd_router_set(args: &[String]) -> R<()> {
    arity("cmd_router_set", args, 4, 4)?;
    let c = cfg_load()?;
    let port = text::to_int(&args[1]).ok_or_else(|| Fail::Value(format!("invalid literal for int() with base 10: {}", crate::py::obj::repr_str(&args[1]))))?;
    // 换路由器：新路由器上没有 owners 文件，第一次推会直接覆盖
    c.set("router", dict! { "host" => &args[0], "port" => port, "user" => &args[2], "key" => &args[3] });
    c.set("proxy_host", &args[0]);
    cfg_save(&c)
}

/// 账号表里的键；kind=official 时只列官方账号（查订阅用量用）
pub fn cmd_keys(args: &[String]) -> R<()> {
    arity("cmd_keys", args, 0, 1)?;
    let kind = arg(args, 0, "");
    for (k, v) in ctx()?.1.items() {
        if kind != "official" || !is_api(&v) {
            out!("{k}");
        }
    }
    Ok(())
}

/// 按完整的键、编号、别名或邮箱找账号，其次按它们的唯一前缀；找不到就退出
pub fn account_named(table: &Dict, q: &str) -> R<String> {
    if q.is_empty() {
        return die("需要指定账号（别名、邮箱、编号或唯一前缀）");
    }
    let keys = table.keys();
    if table.has(q) {
        return Ok(q.to_string());
    }
    if text::isdigit(q) {
        // 原来是 int(q)：² 这类“是数字但不是十进制数字”的在 Python 里会出错退出
        let n = text::to_int(q).ok_or_else(|| Fail::Value(format!("invalid literal for int() with base 10: {}", crate::py::obj::repr_str(q))))?;
        if 1 <= n && n as usize <= keys.len() {
            return Ok(keys[n as usize - 1].to_string());
        }
    }
    let ql = q.to_lowercase();
    let names: Vec<Vec<String>> = keys
        .iter()
        .map(|k| {
            let (a, e) = parse_key(k);
            [k.to_string(), a, e].iter().filter(|x| !x.is_empty()).map(|x| x.to_lowercase()).collect()
        })
        .collect();
    let rules: [fn(&str, &str) -> bool; 2] = [|x, q| x == q, |x, q| x.starts_with(q)];
    for hit in rules {
        let hits: Vec<&str> = keys.iter().zip(&names).filter(|(_, ns)| ns.iter().any(|x| hit(x, &ql))).map(|(k, _)| &**k).collect();
        if hits.len() == 1 {
            return Ok(hits[0].to_string());
        }
        if !hits.is_empty() {
            return die(format!("「{}」匹配到多个账号: {}", q, hits.iter().map(|k| label_of(k)).collect::<Vec<_>>().join("、")));
        }
    }
    die(format!("没有匹配的账号: {q}"))
}

pub fn cmd_resolve(args: &[String]) -> R<()> {
    arity("cmd_resolve", args, 0, 1)?;
    let q = arg(args, 0, "");
    if q.is_empty() {
        return die("需要指定账号（别名、邮箱、编号或唯一前缀）");
    }
    let key = account_named(&ctx()?.1, q)?;
    out!("{key}");
    Ok(())
}

/// 各工具账号表的问题（同步路由器、内置代理之前看：端口池里有各工具的账号）。Claude 的照旧，别的注明是哪张表
pub fn all_errors() -> Vec<String> {
    let mut errs = Vec::new();
    for tool in tools() {
        if tool.tab().is_none() {
            errs.push(format!("{} 不是正确的 JSON（用 {} edit 修正）", tool.table, tool.cmd));
        }
        for e in tool.errs() {
            errs.push(if tool.kind == claude().kind { e } else { format!("{} 账号表：{}", tool.label, e) });
        }
    }
    errs
}

/// 当前工具的账号表有没有问题；scope=all 时各工具的都看
pub fn cmd_validate(args: &[String]) -> R<()> {
    arity("cmd_validate", args, 0, 1)?;
    let mut errs = ctx()?.2;
    if arg(args, 0, "") == "all" {
        errs = all_errors();
    }
    for e in &errs {
        err!("! {e}");
    }
    exit(if errs.is_empty() { 0 } else { 1 })
}

pub fn cmd_email(args: &[String]) -> R<()> {
    arity("cmd_email", args, 1, 1)?;
    out!("{}", t().email(&args[0])?);
    Ok(())
}

pub fn cmd_name(args: &[String]) -> R<()> {
    arity("cmd_name", args, 1, 1)?;
    out!("{}", if args[0].is_empty() { String::new() } else { name_of(&args[0]) });
    Ok(())
}

pub fn cmd_get(args: &[String]) -> R<()> {
    arity("cmd_get", args, 1, 1)?;
    let c = ctx()?.0;
    // 上次使用的账号各工具分开记
    let v = if args[0] == "last" { t().state(&c, false).get("last") } else { c.get(&args[0]) };
    match v {
        None | Some(V::None) => out!(),
        Some(v) => out!("{}", v.py_str()),
    }
    Ok(())
}

pub fn cmd_set(args: &[String]) -> R<()> {
    arity("cmd_set", args, 2, 2)?;
    let c = cfg_load()?;
    c.set(args[0].as_str(), &args[1]);
    cfg_save(&c)
}

/// 服务器上（没配置过路由器、直连不通，cca install 里维护者确认了这台机器之后）导入维护者提供的节点：映射指定的安装节点、
/// 映射里这台机器的账号要用的节点（都要在维护者参考节点里，定义有签名），加进可选节点、记下摘要。打印加进来的节点
/// （一行一个）；可选节点里一个能用的都没有时退出码 3（stdout 是原因）
pub fn cmd_nodes_import(args: &[String]) -> R<()> {
    arity("cmd_nodes_import", args, 0, 0)?;
    let c = cfg_load()?;
    let added = policy_nodes_fill(&c, true);
    if !added.is_empty() {
        cfg_save(&c)?;
    }
    for n in &added {
        out!("{n}");
    }
    if !c.truthy("nodes") {
        out!("维护者的映射里没有这台机器能用的节点（映射指定的安装节点、映射里账号的节点都不在维护者参考节点里）");
        return exit(3);
    }
    Ok(())
}

/// 出口的地区代码在不能用的地区（RESTRICTED）里时打印地区名、退出码 0；不在时退出码 1
pub fn cmd_restricted(args: &[String]) -> R<()> {
    arity("cmd_restricted", args, 0, 1)?;
    let loc = text::strip(arg(args, 0, "")).to_uppercase();
    match RESTRICTED.iter().find(|(cc, _)| *cc == loc) {
        Some((_, name)) => {
            out!("{name}");
            Ok(())
        }
        None => exit(1),
    }
}

/// named：当前工具的账号名与端口（给 Mac 端检查；scope=all 时各工具的）；否则：端口池里所有端口（给路由器自检）
pub fn cmd_ports(args: &[String]) -> R<()> {
    arity("cmd_ports", args, 0, 2)?;
    let (named, scope) = (arg(args, 0, ""), arg(args, 1, ""));
    let (c, table, _) = ctx()?;
    let port = |p: &V| p.as_int().unwrap_or_else(|| panic!("TypeError: %d format: a real number is required, not {}", p.py_repr()));
    if !named.is_empty() && scope == "all" {
        for (tool, k, _, p) in pool(&c) {
            out!("{}\t{}", shown(tool, &k), port(&p));
        }
    } else if !named.is_empty() {
        for (k, _, p) in routed(&c, &table, t()) {
            out!("{}\t{}", name_of(&k), port(&p));
        }
    } else {
        for s in 1..=pool_size(&c) {
            out!("{}", FIRST_PORT - 1 + s);
        }
    }
    Ok(())
}

/// 端口池里有各工具的账号：哪张账号表有错误（或读不了）都不同步，免得把别的工具的账号同步错。
/// （现在的 bash 不调它了：同步用 sync-state；留着是因为原来的 CMDS 里有）
pub fn cmd_accounts_json(args: &[String]) -> R<()> {
    arity("cmd_accounts_json", args, 0, 0)?;
    let c = ctx()?.0;
    let errs = all_errors();
    if !errs.is_empty() {
        return die(format!("账号表有错误，未同步：\n  {}", errs.join("\n  ")));
    }
    out!("{}", crate::py::json::dumps(&V::Dict(crate::core::sync::router_accounts(&c)), crate::py::json::LINE));
    Ok(())
}

/// 账号表 + 路由器端代码（从 stdin 读）的摘要：cca 更新改了路由器脚本，下次启动也会重新部署
pub fn cmd_digest(args: &[String]) -> R<()> {
    arity("cmd_digest", args, 0, 0)?;
    let c = ctx()?.0;
    out!("{}", sync_digest(&c, &io::read_stdin_text()));
    Ok(())
}

/// 配置的映射仓库。只读 config.json，不经过 ctx()：后台拉取时不碰账号表，免得和前台的命令抢着写
pub fn cmd_policy_repo(args: &[String]) -> R<()> {
    arity("cmd_policy_repo", args, 0, 0)?;
    let repo = load_quiet(&CFG).and_then(|c| c.as_dict()).and_then(|c| c.get("policy")).filter(|p| p.truthy());
    out!("{}", repo.map_or(String::new(), |p| p.py_str()));
    Ok(())
}

/// 终端里启动账号时要不要引导配置映射：还没配置映射仓库、也没选过「不用映射」（退出码 0 = 要问）。不经过 ctx
pub fn cmd_policy_ask(args: &[String]) -> R<()> {
    arity("cmd_policy_ask", args, 0, 0)?;
    let c = load_quiet(&CFG).and_then(|c| c.as_dict()).unwrap_or_default();
    exit(if !c.truthy("policy") && !c.truthy("policy_declined") { 0 } else { 1 })
}

/// 引导里选了「不用映射」：以后启动不再问（cca policy setup 会清掉这个标记）。配置向导里先记下了仓库再选的不用：仓库也去掉
/// （deploy key、拉到过的映射由 bash 删掉），App 不再定时拉
pub fn cmd_policy_decline(args: &[String]) -> R<()> {
    arity("cmd_policy_decline", args, 0, 0)?;
    let c = cfg_load()?;
    c.set("policy_declined", true);
    c.pop("policy");
    cfg_save(&c)
}

/// 配置映射仓库（清掉「不用映射」的标记）
pub fn cmd_policy_set(args: &[String]) -> R<()> {
    arity("cmd_policy_set", args, 1, 1)?;
    let c = cfg_load()?;
    c.set("policy", &args[0]);
    c.pop("policy_declined");
    cfg_save(&c)
}

/// 维护者的映射给这个邮箱（当前工具）定的节点；没有就打印空行
pub fn cmd_policy_node(args: &[String]) -> R<()> {
    arity("cmd_policy_node", args, 1, 1)?;
    let node = policy_map(t(), None).get(&text::strip(&args[0]).to_lowercase());
    out!("{}", node.map_or(String::new(), |n| n.py_str()));
    Ok(())
}

/// 映射里各工具有几个账号，例如：Claude 3 个账号、Codex 1 个账号（Antigravity 只在映射里有它的账号时才说）
pub fn cmd_policy_summary(args: &[String]) -> R<()> {
    arity("cmd_policy_summary", args, 0, 0)?;
    let parts: Vec<String> = tools()
        .iter()
        .map(|x| (x, policy_map(x, None).len()))
        .filter(|(x, n)| *n > 0 || x.kind != agy().kind)
        .map(|(x, n)| format!("{} {} 个账号", x.label, n))
        .collect();
    out!("{}", parts.join("、"));
    Ok(())
}

/// 这台机器上受映射约束的账号（各工具一起），节点不在可选列表时注明这个账号用不了；映射里有、这台机器上
/// 没加的也列出来，注明原因（能加的 ctx 已经加进来了）
pub fn cmd_policy_rows(args: &[String]) -> R<()> {
    arity("cmd_policy_rows", args, 0, 0)?;
    let c = ctx()?.0;
    let (mut rows, mut todo) = (Vec::new(), Vec::new());
    let names = policy_names();
    let nodes = c.at("nodes");
    for tool in tools() {
        let Some(table) = tool.tab() else { continue };
        let (pm, raw) = (policy_map(tool, Some(&c)), policy_raw(tool));
        for (k, v) in table.items() {
            let e = parse_key(&k).1.to_lowercase();
            let Some(n) = (if is_api(&v) { None } else { pm.get(&e) }).filter(|n| n.truthy()) else { continue };
            let n = n.py_str();
            let bad = nodes.truthy() && !nodes.as_list().is_some_and(|l| l.has_str(&n));
            let written = raw.get(&e);
            let note = if written.as_ref().is_some_and(|w| w.as_str() == Some(n.as_str())) {
                String::new()
            } else {
                format!("（映射里写的是 {}，路由器上已经改名）", written.map_or("None".to_string(), |w| w.py_str()))
            };
            rows.push(format!("    {}  {} → {}{}{}", tool.label, label_of(&k), n, note, if bad { "（不在可选节点里，这个账号用不了）" } else { "" }));
        }
        let miss = policy_missing(&c, &table, tool);
        if !miss.is_empty() && !policy_in_use(tool) {
            todo.push(format!("    {}  {} 个账号：第一次用 {} 时加进来", tool.label, miss.len(), tool.cmd));
        } else {
            for (e, n, why) in miss.iter().filter(|m| !m.2.is_empty()) {
                let who = match names.get(e) {
                    Some(a) => label_of(&format!("{}({})", a.py_str(), e)),
                    None => e.clone(),
                };
                todo.push(format!("    {}  {} → {}：{}", tool.label, who, n, why));
            }
        }
    }
    if !rows.is_empty() {
        out!("  这台机器上按映射固定出口的账号：\n{}", rows.join("\n"));
    }
    if !todo.is_empty() {
        out!("  映射里还没加到这台机器上的账号：\n{}", todo.join("\n"));
    }
    Ok(())
}

/// 编辑账号表之前：映射里有、账号表里也有的官方账号的邮箱（一行一个）。只读现在的表，不经过 ctx
pub fn cmd_policy_have(args: &[String]) -> R<()> {
    arity("cmd_policy_have", args, 0, 0)?;
    if let Some(V::Dict(table)) = load_quiet(&t().table) {
        let raw = policy_raw(t());
        let mut both: Vec<String> = table_emails(&table).into_iter().filter(|e| raw.has(e)).collect();
        both.sort();
        for e in both {
            out!("{e}");
        }
    }
    Ok(())
}

/// 编辑账号表之后、ctx 按映射把账号加回来之前：编辑前在、现在不在了的映射里的账号记进 policy_skip，以后不再自动加回来
pub fn cmd_policy_forget(args: &[String]) -> R<()> {
    arity("cmd_policy_forget", args, 0, 1)?;
    let Some(V::Dict(table)) = load_quiet(&t().table) else {
        return Ok(());
    };
    let now = table_emails(&table);
    let had: HashSet<&str> = text::split_ws(arg(args, 0, "")).into_iter().collect();
    let mut gone: Vec<String> = had.into_iter().filter(|e| !now.contains(*e)).map(String::from).collect();
    gone.sort();
    if !gone.is_empty() {
        policy_skip_add(t(), &gone)?;
        for e in &gone {
            err!("• {} 在维护者的映射里：删掉了就不再自动加回来（要加回来：{} add）", e, t().cmd);
        }
    }
    Ok(())
}

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[
    Cmd::all("init", cmd_init),
    Cmd::all("router-env", cmd_router_env),
    Cmd::all("router-set", cmd_router_set),
    Cmd::all("keys", cmd_keys),
    Cmd::all("resolve", cmd_resolve),
    Cmd::all("validate", cmd_validate),
    Cmd::all("email", cmd_email),
    Cmd::all("name", cmd_name),
    Cmd::all("get", cmd_get),
    Cmd::all("set", cmd_set),
    Cmd::all("nodes-import", cmd_nodes_import),
    Cmd::all("restricted", cmd_restricted),
    Cmd::all("ports", cmd_ports),
    Cmd::all("accounts-json", cmd_accounts_json),
    Cmd::all("digest", cmd_digest),
    Cmd::all("policy-node", cmd_policy_node),
    Cmd::all("policy-summary", cmd_policy_summary),
    Cmd::all("policy-rows", cmd_policy_rows),
    Cmd::all("policy-repo", cmd_policy_repo),
    Cmd::all("policy-ask", cmd_policy_ask),
    Cmd::all("policy-decline", cmd_policy_decline),
    Cmd::all("policy-set", cmd_policy_set),
    Cmd::all("policy-have", cmd_policy_have),
    Cmd::all("policy-forget", cmd_policy_forget),
];
