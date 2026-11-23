//! 账号表和 config.json 上的那些小命令：init、keys、resolve、validate、get / set、ports、digest、router-env……
//! 以及映射（Team 的配置）的几个查询、升级到 3.6.0 时去掉维护者的映射留在 config.json 里的那几项（policy-retire）。
//! 对应 PY_SRC「命令」一节开头和 `cmd_get` 那一段。

use super::{arg, arity, Cmd};
use crate::core::base::*;
use crate::core::ctx::*;
use crate::core::model::*;
use crate::core::policy::*;
use crate::core::sync::sync_digest;
use crate::core::tool::{claude, t, tools};
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
    let port = text::to_int(&args[1])
        .ok_or_else(|| Fail::Value(format!("invalid literal for int() with base 10: {}", crate::py::obj::repr_str(&args[1]))))?;
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
        let n = text::to_int(q)
            .ok_or_else(|| Fail::Value(format!("invalid literal for int() with base 10: {}", crate::py::obj::repr_str(q))))?;
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

/// 没配置过路由器的设备（服务器上直连不通、加入 Team 以后：team_settle、cca install）导入 Team 的配置带的节点：配置指定的安装节点、
/// 配置里这台设备的账号要用的节点（都要在配置带的节点定义里，配置有签名），加进可选节点、记下摘要。打印加进来的节点
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
        out!("Team 的配置里没有这台机器能用的节点（指定的安装节点、给这台设备的账号的节点都不在配置带的节点里）");
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

/// 升级到 3.6.0 以后（bash 看到还有 ~/.cca/policy/ 时）调一次：config.json 里去掉维护者的映射的那几项（policy、policy_was、
/// policy_declined；有才写回去，别的项原样），拿着 sync.lock。共享原来开着、这台设备又不在哪个 Team 里（teams/index.json 里没有
/// 读得出来的 Team）时 stdout 是一行 alone（bash 提醒一次：向 Team 的管理员要邀请链接）；别的时候什么都不打印。只动 config.json
/// （~/.cca/policy/、维护者那一台留下的同步文件由 bash 删）。没改成（config.json 读不了、不是对象、写不进去、只读）时 stdout 是原因、
/// 退出码 1：bash 留着 ~/.cca/policy/，下一条命令再来
pub fn cmd_policy_retire(args: &[String]) -> R<()> {
    arity("cmd_policy_retire", args, 0, 0)?;
    let failed = |why: String| -> R<()> {
        out!("{why}");
        exit(1)
    };
    if *RO {
        return failed("只读（CCA_RO=1）时不改 config.json".to_string());
    }
    if !path::exists(&CFG) {
        return Ok(()); // 没有 config.json：没有要去掉的
    }
    let _lock = match Lock::acquire() {
        Ok(l) => l,
        Err(e) if e.is_exit() => return Err(e),
        Err(e) => return failed(format!("拿不到 sync.lock：{e}")),
    };
    let raw = match std::fs::read(&*CFG) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return failed(format!("config.json 读不了：{}", fs::open_error_text(&e, &CFG))),
    };
    let c = match crate::py::json::loads_file_bytes(&raw) {
        Ok(V::Dict(c)) => c,
        Ok(v) if !v.truthy() => return Ok(()), // 空的（cfg_load 也当它是空的）：没有要去掉的
        Ok(_) => return failed("config.json 不是 JSON 对象".to_string()),
        Err(e) => return failed(format!("config.json 不是正确的 JSON：{}", e.0)),
    };
    let in_team = crate::core::team::index_order().iter().any(|tid| crate::core::team::load_team(tid).is_some());
    let (changed, alone) = retire_keys(&c, in_team);
    if changed {
        if let Err(e) = save_json(&CFG, &V::Dict(c), Some(0o600), true) {
            if e.is_exit() {
                return Err(e);
            }
            return failed(format!("config.json 写不进去：{e}"));
        }
    }
    if alone {
        out!("alone");
    }
    Ok(())
}

/// policy-retire 改 config.json 的那一步（c 原地改）：去掉 policy、policy_was、policy_declined。返回 (去掉了什么没有, 要不要说 alone：
/// 共享原来开着、这台设备又不在哪个 Team 里)
fn retire_keys(c: &Dict, in_team: bool) -> (bool, bool) {
    let alone = c.truthy("policy") && !in_team;
    let mut changed = false;
    for k in ["policy", "policy_was", "policy_declined"] {
        changed |= c.pop(k).is_some();
    }
    (changed, alone)
}

/// cca restore 退出登录之前：收起来的账号（Team 不再给的；3.6.0 以前关掉共享时收起来的）放回账号表（只在这时用：还原接着就删掉
/// 整个 ~/.cca），好让它们也退出登录、删掉登录凭证
pub fn cmd_unpark(args: &[String]) -> R<()> {
    arity("cmd_unpark", args, 0, 0)?;
    let _lock = Lock::acquire()?;
    let c = cfg_load()?;
    for tool in tools() {
        let p = parked(&c, tool);
        if p.is_empty() {
            continue;
        }
        let table = match load_quiet(&tool.table) {
            Some(V::Dict(t)) => t,
            None if !path::exists(&tool.table) => Dict::new(),
            _ => continue, // 读不了、格式不对的表不碰（还原时这个工具的账号照样按表退出登录）
        };
        let ids: HashSet<String> = table.keys().iter().map(|k| acct_id(k, tool)).collect();
        for (aid, v) in p.items() {
            let Some(d) = v.as_dict() else { continue };
            let Some(k) = d.str("key").filter(|k| !k.is_empty()) else { continue };
            if !ids.contains(&*aid) && !table.has(&k) {
                table.set(k, d.get("node").unwrap_or_else(|| V::from("")));
            }
        }
        save_json(&tool.table, &V::Dict(table), Some(0o600), true)?;
    }
    Ok(())
}

/// Team 的配置给这个邮箱（当前工具）定的节点；没有就打印空行
pub fn cmd_policy_node(args: &[String]) -> R<()> {
    arity("cmd_policy_node", args, 1, 1)?;
    let node = policy_map(t(), None).get(&text::strip(&args[0]).to_lowercase());
    out!("{}", node.map_or(String::new(), |n| n.py_str()));
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

/// 编辑账号表之后、ctx 按映射（Team 的配置）把账号加回来之前：编辑前在、现在不在了的映射里的账号记进 policy_skip，以后不再自动加回来
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
            err!("• {} 在{}里：删掉了就不再自动加回来（要加回来：{} add）", e, by_whom(t(), e), t().cmd);
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
    Cmd::all("policy-retire", cmd_policy_retire),
    Cmd::all("unpark", cmd_unpark),
    Cmd::all("policy-have", cmd_policy_have),
    Cmd::all("policy-forget", cmd_policy_forget),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list;

    #[test]
    fn retire_drops_the_mapping_keys() {
        let c = dict! { "version" => 2, "policy" => "git@example.com:x/policy.git", "nodes" => list!["US-A"], "policy_was" => "y" };
        assert_eq!(retire_keys(&c, false), (true, true), "共享开着、不在 Team 里：说 alone");
        assert_eq!(V::Dict(c.clone()).py_repr(), "{'version': 2, 'nodes': ['US-A']}", "别的项原样、先后不变");
        assert_eq!(retire_keys(&c, false), (false, false), "已经去掉了：不用写回去");
        let c = dict! { "policy" => "git@example.com:x/policy.git" };
        assert_eq!(retire_keys(&c, true), (true, false), "在 Team 里：不说");
        let c = dict! { "policy_was" => "git@example.com:x/policy.git", "policy_declined" => true };
        assert_eq!(retire_keys(&c, false), (true, false), "共享关着：不说");
        assert!(c.is_empty());
        assert_eq!(retire_keys(&dict! { "policy" => "" }, false), (true, false), "空的算关着");
    }
}
