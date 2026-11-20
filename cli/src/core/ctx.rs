//! 对账：校验账号表、照维护者的映射改、分配端口，以及每条命令开头的 `ctx()`。
//! 对应 PY_SRC 的 `reconcile` 到 `slot_nodes`。

use super::base::*;
use super::model::*;
use super::policy::*;
use super::tool::{acct_json_cached, acct_json_store, t, tools, Tool};
use crate::dict;
use crate::py::fs::{load_json, load_quiet, save_json};
use crate::py::num::ceil_to;
use crate::py::obj::{Dict, List, V};
use crate::py::path;
use crate::py::R;
use crate::re;
use std::collections::HashSet;
use std::rc::Rc;

/// 端口这类值当整数看（`isinstance(p, int)`：布尔值在 Python 里也是 int）
fn int_value(v: &V) -> Option<i64> {
    match v {
        V::Int(i) => Some(*i),
        V::Bool(b) => Some(*b as i64),
        _ => None,
    }
}

/// 校验一个工具的账号表；给走节点的账号分配固定端口（直连的第三方 API 账号不占端口），回收已删除账号的端口。
/// reserved 是别的工具占着的端口（大家共用一个端口池），不分给这张表。返回错误列表。
/// 只改了别名的账号按邮箱（第三方 API 账号按 id）认出来：端口和"上次使用"跟着走。
/// 维护者的映射里有的官方账号，节点和映射不一致时改回映射的节点（写回账号表，说一声）；映射里有、这台机器上还没有的
/// 加进来（policy_merge，这台机器自己的账号不动），还没有别名的按映射补上别名（policy_alias_fill）：每条命令都经过
/// 这里，添加、cca edit、启动、同步前都管得到
pub fn reconcile(c: &Dict, table: &Dict, tool: &Tool, reserved: &HashSet<i64>) -> R<Vec<String>> {
    let st = tool.state(c, true);
    let added = policy_merge(c, table, tool);
    for k in &added {
        let node = table.at(k);
        notice(
            "policy_added",
            &format!("• 按维护者的映射添加了 {} 账号 {}，出口 {}", tool.label, label_of(k), node.py_str()),
            dict! { "tool" => tool.name, "key" => k, "node" => node },
        );
    }
    // 关掉共享时收起来的账号又在账号表里了（重新打开共享时按映射加回来了，或者用户自己加回来了）：不再算收起来的
    if let Some(p) = st.dict("parked") {
        let ids: HashSet<String> = table.keys().iter().map(|k| acct_id(k, tool)).collect();
        for aid in p.keys() {
            if ids.contains(&*aid) {
                p.pop(&aid);
            }
        }
        if p.is_empty() {
            st.pop("parked");
        }
    }
    let renamed = policy_alias_fill(table, tool);
    for (k, new) in &renamed {
        notice(
            "policy_alias",
            &format!("• 按维护者的映射给 {} 账号 {} 起了别名 {}", tool.label, k, parse_key(new).0),
            dict! { "tool" => tool.name, "key" => new, "old_key" => k },
        );
    }
    let pm = policy_map(tool, Some(c));
    let pinned = |k: &str| pm.get(&parse_key(k).1.to_lowercase());
    let fixed: Vec<Rc<str>> = table
        .items()
        .into_iter()
        .filter(|(k, v)| !is_api(v) && pinned(k).is_some_and(|want| !want.py_eq(v)))
        .map(|(k, _)| k)
        .collect();
    for k in &fixed {
        let want = pinned(k).expect("上面刚查过");
        let old = table.at(k);
        notice(
            "policy_pinned",
            &format!("! {} 的出口按维护者的映射固定为 {}（账号表里写的是 {}，已改回）", label_of(k), want.py_str(), old.py_str()),
            dict! { "tool" => tool.name, "key" => k, "node" => &want, "old_node" => old },
        );
        table.set(k.clone(), want);
    }
    if (!fixed.is_empty() || !added.is_empty() || !renamed.is_empty()) && !*RO {
        save_json(&tool.table, &V::Dict(table.clone()), Some(0o600), true)?;
    }
    let errs = table_problems(c, table, tool, &pm);
    // 第三方 API 账号的键 → 账号目录名，记在 config.json：改过不止一次别名也认得出旧的端口和"上次使用"
    let known = st.dict("api_ids").unwrap_or_default();
    let known_id = |k: &str| known.get(k).filter(|v| v.truthy()).map(|v| v.py_str()).unwrap_or_else(|| acct_id(k, tool));
    let old = st.dict("ports").unwrap_or_default();
    let by_id: Vec<(String, V)> = old.items().iter().map(|(k, p)| (known_id(k), p.clone())).collect();
    // by_id 是字典：同一个 id 后写的盖掉先写的
    let by_id_get = |id: &str| by_id.iter().rev().find(|(i, _)| i == id).map(|(_, p)| p.clone());
    let wanted: Vec<Rc<str>> = table.items().into_iter().filter(|(_, v)| !node_of(v).is_empty()).map(|(k, _)| k).collect();
    let ports = Dict::new();
    let mut used = reserved.clone();
    for k in &wanted {
        let p = old.get(k).filter(|v| v.truthy()).or_else(|| by_id_get(&acct_id(k, tool)));
        if let Some(p) = p {
            if let Some(n) = int_value(&p) {
                if !used.contains(&n) {
                    ports.set(k.clone(), p);
                    used.insert(n);
                }
            }
        }
    }
    for k in &wanted {
        if !ports.has(k) {
            let mut p = FIRST_PORT;
            while used.contains(&p) {
                p += 1;
            }
            ports.set(k.clone(), p);
            used.insert(p);
        }
    }
    st.set("ports", ports);
    let last = st.get("last").unwrap_or(V::None);
    // st.get("last") not in t：列表、字典不能当键查（原来是没人接的 TypeError）
    if last.is_list() || last.is_dict() {
        panic!("TypeError: unhashable type（config.json 里的 last 是 {}）", last.py_repr());
    }
    if !last.as_str().is_some_and(|s| table.has(s)) {
        let lid = match last.as_str() {
            Some(s) if !s.is_empty() => known_id(s),
            // 不是文字又不是空的（数字、true）：原来 acct_id 拿它当文字用，是没人接的 TypeError
            None if last.truthy() => {
                panic!("TypeError: expected string or bytes-like object（config.json 里的 last 是 {}）", last.py_repr())
            }
            _ => String::new(),
        };
        let found = table.keys().into_iter().find(|k| acct_id(k, tool) == lid);
        st.set("last", found.map_or(V::from(""), V::from));
    }
    let now = Dict::new();
    for (k, v) in table.items() {
        if is_api(&v) {
            now.set(k.clone(), acct_id(&k, tool));
        }
    }
    if !now.is_empty() || !known.is_empty() {
        st.set("api_ids", now);
    }
    Ok(errs)
}

/// 账号表的问题（只看，不改）：reconcile、表单都用它
pub fn table_problems(c: &Dict, table: &Dict, tool: &Tool, pm: &Dict) -> Vec<String> {
    let mut errs = Vec::new();
    let (emails, aliases, ids) = (Dict::new(), Dict::new(), Dict::new());
    let nodes = c.at("nodes");
    let node_list = nodes.as_list();
    if !nodes.truthy() && table.values().iter().any(|v| !node_of(v).is_empty() || !is_api(v)) {
        errs.push("config.json 里还没有可选节点（nodes）：运行 cca router setup 从路由器的节点里选".to_string());
    }
    for (k, v) in table.items() {
        if !KEY_RE.matches(&k) {
            errs.push(format!("「{k}」：不能含空格、引号或反斜杠，最长 100 个字符"));
        }
        let (a, e) = parse_key(&k);
        if e.is_empty() && re!("[()（）]").found(&k) {
            errs.push(format!("「{k}」：括号里应该是登录邮箱，例如 主号(name@example.com)"));
        }
        if !e.is_empty() {
            let el = e.to_lowercase();
            if let Some(first) = emails.get(&el) {
                errs.push(format!("邮箱 {} 同时出现在「{}」和「{}」里", e, first.py_str(), k));
            }
            emails.setdefault(&el, &k);
        }
        if !a.is_empty() {
            if let Some(first) = aliases.get(&a) {
                errs.push(format!("别名「{}」同时用在「{}」和「{}」上", a, first.py_str(), k));
            }
            aliases.setdefault(&a, &k);
        }
        match &v {
            V::Dict(d) => errs.extend(api_problems(c, &k, d, tool).into_iter().map(|m| format!("「{k}」：{m}"))),
            V::Str(s) if !s.is_empty() => {
                if nodes.truthy() && !in_nodes(&node_list, s) {
                    let all = join_names(&nodes);
                    errs.push(if !e.is_empty() && pm.has(&e.to_lowercase()) {
                        format!(
                            "{k}：维护者的映射要求它走「{s}」，可这个节点不在可选列表（{all}）。cca edit 改不了它：\
                             运行 cca router setup 重新选可选节点（路由器上也没有这个节点的话，请联系维护者）"
                        )
                    } else {
                        format!("{k}：节点「{s}」不在可选列表（{all}）")
                    });
                }
            }
            _ => errs.push(format!("{k}：节点必须是非空字符串")),
        }
        let i = acct_id(&k, tool);
        if let Some(first) = ids.str(&i) {
            // 官方账号的邮箱重复上面已经报过
            if is_api(&v) || is_api(&table.at(&first)) {
                errs.push(format!("「{first}」和「{k}」会用同一个账号目录（id 或邮箱重复了）"));
            }
        }
        ids.setdefault(&i, &k);
    }
    errs
}

/// 维护者的映射要用、还不在可选节点里的节点，是维护者参考配置里的（同步时 cca 把它写进路由器的配置）：自动加进
/// 可选节点、记下摘要，说一声，映射里的账号随后就能加进来。还没有可选节点（没配置路由器）、这台机器删掉过的账号不算。
/// server：服务器上导入维护者提供的节点（cca install 里维护者确认了这台机器之后，见 cmd_nodes_import）：还没有可选节点也加，
/// 映射指定的安装节点也加，不说（bash 汇总成一句）。返回加进来的节点
pub fn policy_nodes_fill(c: &Dict, server: bool) -> Vec<String> {
    let Some(f) = policy_fill() else {
        return Vec::new();
    };
    if !(c.truthy("nodes") || server) {
        return Vec::new();
    }
    let proxies: Vec<Dict> = f.list("proxies").unwrap_or_default().items().iter().filter_map(|p| p.as_dict()).collect();
    let fp: Vec<(String, String)> = proxies.iter().map(|p| (p.at("name").py_str(), fill_fp(p))).collect();
    // fp 是字典：同名的后一个盖掉前一个
    let fp_of = |n: &str| fp.iter().rev().find(|(x, _)| x == n).map(|(_, g)| g.clone());
    let have: Vec<String> = c.list("nodes").unwrap_or_default().strs().iter().map(|s| s.to_string()).collect();
    let mut want: Vec<(String, Vec<String>)> = Vec::new();
    let inst = if server { policy_install_node() } else { String::new() };
    if fp_of(&inst).is_some() && !have.contains(&inst) {
        want.push((inst, Vec::new()));
    }
    for tool in tools() {
        if !policy_in_use(tool) {
            continue;
        }
        let skip: HashSet<String> = match tool.state(c, false).get("policy_skip") {
            Some(V::List(l)) => l.strs().iter().map(|s| s.to_string()).collect(),
            _ => HashSet::new(),
        };
        for (e, n) in policy_map(tool, Some(c)).items() {
            let n = n.py_str();
            if fp_of(&n).is_some() && !have.contains(&n) && !skip.contains(&*e) {
                match want.iter_mut().find(|(x, _)| *x == n) {
                    Some((_, who)) => who.push(e.to_string()),
                    None => want.push((n, vec![e.to_string()])),
                }
            }
        }
    }
    for (n, who) in &want {
        let nodes = c.list("nodes").map(|l| l.items()).unwrap_or_default();
        c.set("nodes", List::from_vec(nodes.into_iter().chain([V::from(n)]).collect()));
        let g = fp_of(n).expect("want 里的都在 fp 里");
        if !g.is_empty() {
            let ids = node_ids(c).copy();
            ids.set(n.as_str(), g);
            c.set("node_ids", ids);
        }
        if !server {
            notice(
                "nodes_filled",
                &format!(
                    "• 维护者的映射要 {} 走节点 {}：它在维护者的参考配置里（同步时写进路由器的配置），已加进可选节点",
                    who.join("、"),
                    n
                ),
                dict! { "node" => n },
            );
        }
    }
    want.into_iter().map(|(n, _)| n).collect()
}

/// 各工具的账号表一起对账（大家共用一个端口池）：各自已有的端口不动，新端口避开别的工具占着的。
/// 账号表读不了的工具不对账，它记着的端口照样留着。映射要的参考节点先加进可选节点（policy_nodes_fill）
pub fn reconcile_all(c: &Dict) -> R<()> {
    for tool in tools() {
        tool.clear_errs();
    }
    policy_nodes_fill(c, false);
    for tool in tools() {
        let Some(table) = tool.tab() else { continue };
        let mut reserved = HashSet::new();
        for other in tools().iter().filter(|o| o.kind != tool.kind) {
            if let Some(ports) = other.state(c, false).dict("ports") {
                reserved.extend(ports.values().iter().filter_map(int_value));
            }
        }
        let errs = reconcile(c, &table, tool, &reserved)?;
        tool.set_errs(errs);
    }
    Ok(())
}

/// 每条要看账号表的命令的开头：读 config.json 和各张账号表、对账，有变化（而且不是只读）就把 config.json 存回去。
/// 返回 (配置, 当前工具的账号表, 这张表的问题)
pub fn ctx() -> R<(Dict, Dict, Vec<String>)> {
    let c = cfg_load()?;
    tables_load()?;
    reconcile_all(&c)?;
    cfg_clean(&c);
    if !*RO && !load_json(&CFG)?.is_some_and(|on_disk| V::Dict(c.clone()).py_eq(&on_disk)) {
        cfg_save(&c)?;
    }
    Ok((c, t().tab().expect("tables_load 读了当前工具的表"), t().errs()))
}

/// 没有 Codex 账号时不在 config.json 里留下空的 codex 一节：只有 Claude 账号时 config.json 和以前一样
pub fn cfg_clean(c: &Dict) {
    for tool in tools() {
        let Some(key) = tool.state_key else { continue };
        if let Some(s) = c.dict(key) {
            let keep = ["ports", "last", "api_ids", "api_keys", "ref_keys", "key_hints", "policy_skip", "parked"]
                .iter()
                .any(|k| s.truthy(k));
            if !keep {
                c.pop(key);
            }
        }
    }
    // 空的不留在 config.json 里
    if c.has("device_id") && !c.truthy("device_id") {
        c.pop("device_id");
    }
}

pub fn cfg_save(c: &Dict) -> R<()> {
    cfg_clean(c);
    save_json(&CFG, &V::Dict(c.clone()), Some(0o600), true)
}

/// 这台机器上删掉的映射里的账号记进 policy_skip：以后不再自动加回来（cca add 加回来时清掉）
pub fn policy_skip_add(tool: &Tool, emails: &[String]) -> R<()> {
    let c = cfg_load()?;
    let st = tool.state(&c, true);
    let mut all: Vec<String> = match st.get("policy_skip") {
        Some(V::List(l)) => l.strs().iter().map(|s| s.to_string()).collect(),
        _ => Vec::new(),
    };
    all.extend(emails.iter().cloned());
    all.sort();
    all.dedup();
    st.set("policy_skip", crate::py::obj::str_list(all));
    cfg_save(&c)
}

/// 账号目录里的 .claude.json（一次进程里只读一次）。只读（CCA_RO）时读不了不退出：当作没登录，记进 RO_BAD 给图形界面报
pub fn acct_json(key: &str) -> R<Dict> {
    if let Some(j) = acct_json_cached(key) {
        return Ok(j);
    }
    let p = path::join(&cdir(key), ".claude.json");
    let j = if *RO {
        let j = load_quiet(&p);
        if j.is_none() && path::exists(&p) {
            RO_BAD.with(|b| b.borrow_mut().push((t().name.to_string(), p.clone())));
        }
        j
    } else {
        load_json(&p)?
    };
    let j = j.and_then(|v| v.as_dict()).unwrap_or_default();
    acct_json_store(key, &j);
    Ok(j)
}

/// 账号目录的 .claude.json 里的 oauthAccount（登录的是谁）；没有是空字典
pub fn oauth_of(key: &str) -> R<Dict> {
    Ok(acct_json(key)?.dict("oauthAccount").unwrap_or_default())
}

impl Tool {
    /// `T.email(key)`：登录的邮箱（只读账号目录里的状态，不联网）
    pub fn email(&self, key: &str) -> R<String> {
        use super::tool::Kind;
        use crate::cmd::codex::{agy_claims, codex_claims};
        let or_empty = |v: Option<V>| v.filter(|v| v.truthy()).map_or(String::new(), |v| v.py_str());
        match self.kind {
            Kind::Claude => Ok(or_empty(oauth_of(key)?.get("emailAddress"))),
            Kind::Codex => Ok(or_empty(codex_claims(key, None).get("email"))),
            Kind::Agy => Ok(or_empty(agy_claims(key).get("email"))),
        }
    }
}

/// 路由器上的端口池按 8 个槽位一档；槽位够用时增删账号、换节点都不用重载 OpenClash / ShellCrash
pub const POOL_STEP: i64 = 8;

/// 端口池要开多少个槽位（8 的整数倍；一个走节点的账号都没有是 0）
pub fn pool_size(c: &Dict) -> i64 {
    let top = pool(c).iter().filter_map(|(_, _, _, p)| int_value(p)).map(|p| p - FIRST_PORT + 1).max();
    match top {
        Some(m) => POOL_STEP.max(ceil_to(m, POOL_STEP)),
        None => 0,
    }
}

/// 每个槽位应选中的节点：分给账号的用账号的节点，空槽位用 REJECT
pub fn slot_nodes(c: &Dict) -> Vec<String> {
    let taken: Vec<(i64, Rc<str>)> =
        pool(c).iter().filter_map(|(_, _, v, p)| Some((int_value(p)? - FIRST_PORT + 1, node_of(v)))).collect();
    // 字典：同一个槽位后写的盖掉先写的
    (1..=pool_size(c))
        .map(|s| taken.iter().rev().find(|(n, _)| *n == s).map_or("REJECT".to_string(), |(_, node)| node.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tool::{claude, codex};
    use crate::list;

    fn cfg(nodes: List) -> Dict {
        cfg_fill(dict! { "nodes" => nodes })
    }

    #[test]
    fn ports_are_kept_and_filled() {
        let c = cfg(list!["US-A", "US-B"]);
        c.set("ports", dict! { "gone@example.com" => 7901, "b@example.com" => 7903 });
        c.set("last", "gone@example.com");
        let table = dict! { "a@example.com" => "US-A", "b@example.com" => "US-B", "直连" => dict! { "base_url" => "https://x.example.com" },
        "走节点" => dict! { "base_url" => "https://y.example.com", "node" => "US-A" } };
        let errs = reconcile(&c, &table, claude(), &HashSet::from([7902])).unwrap();
        assert!(errs.is_empty(), "{errs:?}");
        // 原有端口的先进字典，新分配的排在后面；7902 被别的工具占着
        assert_eq!(V::Dict(c.dict("ports").unwrap()).py_repr(), "{'b@example.com': 7903, 'a@example.com': 7901, '走节点': 7904}");
        assert_eq!(c.str("last").as_deref(), Some(""));
        assert_eq!(
            V::Dict(c.dict("api_ids").unwrap()).py_repr(),
            format!("{{'直连': '{}', '走节点': '{}'}}", ident("直连"), ident("走节点"))
        );
    }

    #[test]
    fn renamed_alias_keeps_port_and_last() {
        let c = cfg(list!["US-A"]);
        c.set("ports", dict! { "旧名(a@example.com)" => 7905 });
        c.set("last", "旧名(a@example.com)");
        let table = dict! { "新名(a@example.com)" => "US-A" };
        reconcile(&c, &table, claude(), &HashSet::new()).unwrap();
        assert_eq!(V::Dict(c.dict("ports").unwrap()).py_repr(), "{'新名(a@example.com)': 7905}");
        assert_eq!(c.str("last").as_deref(), Some("新名(a@example.com)"));
    }

    #[test]
    fn table_checks() {
        let c = cfg(list!["US-A"]);
        let table = dict! { "a b" => "US-A", "x(y)" => "US-A", "甲(a@example.com)" => "US-A", "甲(A@example.com)" => "ZZ", "空" => "", "数" => 1 };
        assert_eq!(
            table_problems(&c, &table, claude(), &Dict::new()),
            [
                "「a b」：不能含空格、引号或反斜杠，最长 100 个字符",
                "「x(y)」：括号里应该是登录邮箱，例如 主号(name@example.com)",
                "邮箱 A@example.com 同时出现在「甲(a@example.com)」和「甲(A@example.com)」里",
                "别名「甲」同时用在「甲(a@example.com)」和「甲(A@example.com)」上",
                "甲(A@example.com)：节点「ZZ」不在可选列表（US-A）",
                "空：节点必须是非空字符串",
                "数：节点必须是非空字符串",
            ]
        );
        assert_eq!(
            table_problems(&cfg(list![]), &dict! { "a@example.com" => "US-A" }, claude(), &Dict::new()),
            ["config.json 里还没有可选节点（nodes）：运行 cca router setup 从路由器的节点里选"]
        );
        let pinned = dict! { "a@example.com" => "ZZ" };
        assert!(table_problems(&c, &dict! { "a@example.com" => "ZZ" }, claude(), &pinned)[0]
            .starts_with("a@example.com：维护者的映射要求它走「ZZ」"));
    }

    #[test]
    fn clean_and_pool() {
        let c = cfg(list!["US-A", "HK-A"]);
        c.set("codex", dict! { "ports" => dict! {}, "last" => "", "order_base" => list!["x"] });
        c.set("agy", dict! { "ports" => dict! { "g@example.com" => 7910 } });
        c.set("device_id", "");
        cfg_clean(&c);
        assert!(!c.has("codex") && c.has("agy") && !c.has("device_id"));
        assert_eq!(pool_size(&c), 0);
        c.set("ports", dict! { "a@example.com" => 7901, "b@example.com" => 7909 });
        claude().set_tab(Some(dict! { "a@example.com" => "US-A", "b@example.com" => "HK-A" }));
        codex().set_tab(None);
        assert_eq!(pool_size(&c), 16);
        let s = slot_nodes(&c);
        assert_eq!((s.len(), s[0].as_str(), s[1].as_str(), s[8].as_str()), (16, "US-A", "REJECT", "HK-A"));
        claude().set_tab(None);
    }
}
