//! 内置代理（`slot_orders` 到 `write_text`，加上 `cmd_select_lines`、`cmd_slot_rows`）：
//!
//! - 这一份：本机端口池和槽位（local-env / local-set / local-node、select-lines、slot-rows）、自定义节点的几个子命令
//!   （node-check / node-testconfig / node-save / node-ls / node-rm）、内核的配置和节点定义（core-config / core-nodes-save /
//!   core-stale / core-missing / core-summary）、辅助安装的临时代理（install-env / install-node）
//! - `corenet/parse.rs`：分享链接和 Clash 节点块的解析（`_y_scalar` 到 `node_parse`）
//! - `corenet/bind.rs`：别的程序开着 TUN 时出站绑定物理网卡（`sys_cmd_out` 到 `core_bind_apply`）
//! - `corenet/udp.rs`：本机 UDP 走节点（udp-env / udp-node / udp-build / udp-probe，`udp_view`、`udp_leak_hint`）
//! - `corenet/keep.rs`：守着内核的常驻进程（core-keep）
//!
//! 没移植的只有原来就走不到的 `flow_get` / `fill_members`（死代码），见 FINDINGS.md；`file-replace` 没有调用方，照原样留着。
//!
//! 节点定义里有节点的密钥（密码、uuid）：只存在文件里（`core/nodes.json`、`core/custom-nodes.json`、`core/config.yaml`，都是 600），
//! 不进任何进程的命令行和环境变量。
//!
//! config.json、缓存里手改成别的类型的值：照 Python 对那个类型的行为做（遍历字符串是一个个字符、拿列表当键是没人接的
//! TypeError……），没人接的异常这里是 panic，由 main 的钩子说一句、退出码 1。

mod bind;
mod keep;
mod parse;
mod udp;

#[allow(unused_imports)] // 别的模块（界面的 state / live、在家还是在外的判断）移植时从这里拿
pub use bind::{core_bind, core_bind_apply, virtual_iface, Bind, CORE_ROUTE_PROBE, VIRTUAL_IFACE_RE};
pub use keep::cmd_core_keep;
#[allow(unused_imports)] // udp_view：_gui state；udp_leak_hint：leak-verdict；udp_running、udp_paths：bash 那一半对应的几处
pub use udp::{cmd_udp_build, cmd_udp_env, cmd_udp_node, cmd_udp_probe, udp_leak_hint, udp_paths, udp_running, udp_state, udp_view, UDP_FLAG, UDP_HELPER, UDP_LABEL};

use super::clients::cu_claude_env;
use super::router::{node_match, node_users_all};
use super::secrets::{py_set, py_sorted};
use super::{arg, arity, Cmd};
use crate::core::base::*;
use crate::core::ctx::{cfg_save, ctx, pool_size, slot_nodes};
use crate::core::model::*;
use crate::core::policy::{fill_fp, fill_plan, node_alias, node_ids, policy_fill, policy_install_node, FILL_BUILTIN};
use crate::core::tool::{t, Kind};
use crate::py::fs::{self, load_quiet, save_json};
use crate::py::hash::sha1_hex;
use crate::py::io;
use crate::py::json;
use crate::py::obj::{str_list, Dict, List, V};
use crate::py::path;
use crate::py::time::time;
use crate::py::{die, exit, R};
use crate::{dict, list, out};
use parse::{json_key, json_loads, node_clean_name, node_parse};
use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::rc::Rc;
use std::sync::LazyLock;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[
    Cmd::all("select-lines", cmd_select_lines),
    Cmd::all("local-env", cmd_local_env),
    Cmd::all("local-set", cmd_local_set),
    Cmd::all("local-node", cmd_local_node),
    Cmd::all("slot-rows", cmd_slot_rows),
    Cmd::all("node-check", cmd_node_check),
    Cmd::all("node-testconfig", cmd_node_testconfig),
    Cmd::all("node-save", cmd_node_save),
    Cmd::all("node-ls", cmd_node_ls),
    Cmd::all("node-rm", cmd_node_rm),
    Cmd::all("core-config", cmd_core_config),
    Cmd::all("core-nodes-save", cmd_core_nodes_save),
    Cmd::all("core-stale", cmd_core_stale),
    Cmd::all("core-missing", cmd_core_missing),
    Cmd::all("core-summary", cmd_core_summary),
    Cmd::all("install-env", cmd_install_env),
    Cmd::all("install-node", cmd_install_node),
    Cmd::all("udp-env", cmd_udp_env),
    Cmd::all("udp-node", cmd_udp_node),
    Cmd::all("udp-build", cmd_udp_build),
    Cmd::all("udp-probe", cmd_udp_probe),
    Cmd::all("core-keep", cmd_core_keep),
    Cmd::all("file-replace", cmd_file_replace),
];

// --- 内置代理：在外面、这台 Mac 上又没有 Clash 客户端时，界面 App 托管的 mihomo 内核（bash 的"内置代理"一节） ----------------
pub static CORE_DIR: LazyLock<String> = LazyLock::new(|| path::join(&CCA_HOME, "core"));
/// 在家时从路由器拷的节点定义（带密钥，600）
pub static CORE_NODES: LazyLock<String> = LazyLock::new(|| path::join(&CORE_DIR, "nodes.json"));
/// 内核的配置（用 JSON 写，YAML 也认；带密钥，600）
pub static CORE_CONFIG: LazyLock<String> = LazyLock::new(|| path::join(&CORE_DIR, "config.yaml"));
pub const CORE_STALE_AFTER: f64 = 12.0 * 3600.0;
/// 用户自己填/粘贴的节点（分享链接或 Clash 节点块，只给内置代理，绝不推路由器；带密钥，600）
pub static CORE_CUSTOM: LazyLock<String> = LazyLock::new(|| path::join(&CORE_DIR, "custom-nodes.json"));
/// Linux 上内核的输出（macOS 上在界面 App 的缓存目录里）
pub static CORE_LOG: LazyLock<String> = LazyLock::new(|| path::join(&CORE_DIR, "core.log"));
/// 路由器端 defs 输出的最后一行：没读到它就是没拷完整
const DEFS_END: &str = "\n# cca-defs-end\n";
/// 临时内核里唯一的组（和内置代理的 CCA-n 一样：入站直通它）
const INSTALL_GROUP: &str = "CCA-INSTALL";

/// `type(x).__name__`（照 Python 说没人接的 TypeError / AttributeError 时用）
fn type_name(v: &V) -> &'static str {
    match v {
        V::None => "NoneType",
        V::Bool(_) => "bool",
        V::Int(_) | V::Big(_) => "int",
        V::Float(_) => "float",
        V::Str(_) => "str",
        V::List(_) => "list",
        V::Dict(_) => "dict",
    }
}

/// 当字典的键、进集合、在集合里找：列表和字典在 Python 里是没人接的 TypeError
fn hashable(v: &V) -> &V {
    if v.is_list() || v.is_dict() {
        panic!("TypeError: unhashable type: '{}'", type_name(v));
    }
    v
}

/// `c.get(k) or {}`，接着就要 `.get()`：config.json 的一节（local、router）。手改成不是对象的真值时，Python 里是没人接的 AttributeError
fn sect(c: &Dict, k: &str) -> Dict {
    match c.get(k) {
        Some(V::Dict(d)) => d,
        Some(v) if v.truthy() => panic!("AttributeError: '{}' object has no attribute 'get'", type_name(&v)),
        _ => Dict::new(),
    }
}

/// `(c.get('local') or {}).get('client') == 'builtin'`
fn client_builtin(c: &Dict) -> bool {
    matches!(sect(c, "local").get("client"), Some(V::Str(s)) if &*s == "builtin")
}

/// `(c.get('router') or {}).get('host')` 算不算真
fn router_host(c: &Dict) -> bool {
    sect(c, "router").truthy("host")
}

/// `for x in v`：Python 里能这样遍历的 JSON 值——列表的项、字符串的一个个字符、字典的键；别的（数字、true、null）是没人接的 TypeError
fn py_iter(v: &V) -> Vec<V> {
    match v {
        V::List(l) => l.items(),
        V::Str(s) => s.chars().map(|ch| V::str(ch.to_string())).collect(),
        V::Dict(d) => d.keys().into_iter().map(V::Str).collect(),
        v => panic!("TypeError: '{}' object is not iterable", type_name(v)),
    }
}

/// `for n in c['nodes']`：可选节点。正常是字符串的列表
fn nodes_of(c: &Dict) -> Vec<V> {
    py_iter(&c.at("nodes"))
}

/// `for n in (c.get('nodes') or [])`
fn nodes_or_empty(c: &Dict) -> Vec<V> {
    match c.get("nodes").filter(|n| n.truthy()) {
        Some(n) => py_iter(&n),
        None => Vec::new(),
    }
}

/// `x in (v or [])`：列表看有没有这一项，字符串看是不是它的一段，字典看有没有这个键
fn py_contains(v: Option<V>, x: &V) -> bool {
    match v.filter(|v| v.truthy()) {
        None => false,
        Some(V::List(l)) => l.contains(x),
        Some(V::Str(s)) => match x {
            V::Str(part) => s.contains(&**part),
            other => panic!("TypeError: 'in <string>' requires string as left operand, not {}", type_name(other)),
        },
        Some(V::Dict(d)) => matches!(hashable(x), V::Str(k) if d.has(k)),
        Some(v) => panic!("TypeError: argument of type '{}' is not a container or iterable", type_name(&v)),
    }
}

/// `m.get(n, n)`：m 的键都是字符串（节点名 → 内置代理里的叫法），别的类型的 n 查不到，原样
fn mapped(m: &Dict, n: &V) -> V {
    match hashable(n) {
        V::Str(s) => m.get(s).unwrap_or_else(|| n.clone()),
        _ => n.clone(),
    }
}

/// `'、'.join(列表)`：有不是字符串的项是没人接的 TypeError
fn join_strs(items: &[V]) -> String {
    let strs: Vec<&str> = items
        .iter()
        .enumerate()
        .map(|(i, v)| v.as_str().unwrap_or_else(|| panic!("TypeError: sequence item {}: expected str instance, {} found", i, type_name(v))))
        .collect();
    strs.join("、")
}

/// 节点定义的列表写成 JSON 的列表
fn defs_list(defs: &[Dict]) -> List {
    List::from_vec(defs.iter().map(V::from).collect())
}

/// `isinstance(p, dict) and isinstance(p.get('name'), str)`：像样的节点定义
fn named(p: &V) -> Option<Dict> {
    p.as_dict().filter(|d| d.str("name").is_some())
}

/// `p['name']`，p 是 `named` 筛过的节点定义
fn def_name(p: &Dict) -> Rc<str> {
    p.str("name").unwrap_or_else(|| panic!("TypeError: 节点定义的 name 不是文字（{}）", p.get("name").unwrap_or(V::None).py_repr()))
}

/// `[(p['name'], fill_fp(p)) for p in px]`
fn fps_of(px: &[Dict]) -> Vec<(String, String)> {
    px.iter().map(|p| (def_name(p).to_string(), fill_fp(p))).collect()
}

// --- 出门在外：本机 Clash 客户端的端口池 -----------------------------------------

/// 端口池每个槽位：(组名, 端口, 成员顺序)。和路由器上一样，分到账号的槽位把它的节点排第一（客户端重新加载后默认就选中它），
/// 空槽位 REJECT 排第一。local：给内置代理用，节点名换成它那边的叫法（local_names）
pub fn slot_orders(c: &Dict, local: bool) -> Vec<(String, i64, Vec<V>)> {
    let m = if local { local_names(c) } else { Dict::new() };
    let mut nodes: Vec<V> = Vec::new(); // list(dict.fromkeys(m.get(n, n) for n in c['nodes']))
    for n in nodes_of(c) {
        let n = mapped(&m, &n);
        if !nodes.iter().any(|have| have.py_eq(&n)) {
            nodes.push(n);
        }
    }
    let mut out = Vec::new();
    for (i, node) in slot_nodes(c).into_iter().enumerate() {
        let s = i as i64 + 1;
        let node = m.get(&node).unwrap_or_else(|| V::from(node));
        let reject = node.as_str() == Some("REJECT");
        let mut order = vec![node.clone()];
        order.extend(nodes.iter().filter(|n| !n.py_eq(&node)).cloned());
        if !reject {
            order.push(V::from("REJECT"));
        }
        out.push((format!("CCA-{s}"), FIRST_PORT - 1 + s, order));
    }
    out
}

/// 和全局扩展脚本里的算法一样：只留客户端里有的节点；分到的节点不在时 REJECT 排第一
fn local_members(order: &[V], have: &[Rc<str>]) -> Vec<V> {
    let is_reject = |n: &V| n.as_str() == Some("REJECT");
    let m: Vec<V> = order.iter().filter(|n| is_reject(n) || matches!(hashable(n), V::Str(s) if have.contains(s))).cloned().collect();
    if m[0].py_eq(&order[0]) {
        return m;
    }
    let mut out = vec![V::from("REJECT")];
    out.extend(m.into_iter().filter(|n| !is_reject(n)));
    out
}

/// 端口池的结构（可选节点在客户端里的叫法 + 槽位数）：变了才需要让客户端重新加载；只换节点走控制接口。
/// 内置代理里给节点改了名，换算出来的叫法变了，也要重新加载（端口池的组成员要换成新名字）
pub fn local_want(c: &Dict) -> R<String> {
    let m = local_names(c);
    let names = py_sorted(nodes_of(c).iter().map(|n| mapped(&m, n)).collect())?;
    let want = list![List::from_vec(names), pool_size(c)];
    let f = policy_fill();
    if client_builtin(c) {
        // 内置代理：节点定义变了（路由器上改了节点、参考节点更新）也要重新加载
        let defs = core_node_defs(c, Some(&m));
        want.push(sha1_hex(json::dumps(&V::List(defs_list(&defs)), json::Fmt::new().utf8().sort_keys())));
        if let Some(b) = core_bind(c, &defs) {
            // 出站绑定了物理网卡（别的程序开着 TUN）：网卡、DNS 变了也要重新加载（没绑定时摘要和以前一样）
            want.push(list![b.iface, str_list(&b.dns)]);
        }
    } else if let Some(f) = f {
        // 参考节点进了内置代理：变了也要重新加载（没有参考节点时摘要和以前一样）
        let id = f.at("id");
        if sect(c, "local").get("fill").is_some_and(|have| have.py_eq(&id)) {
            want.push(id);
        }
    }
    Ok(sha1_hex(json::dumps(&V::List(want), json::Fmt::new())))
}

/// 账号表里的节点名 → 内置代理的节点定义里的叫法（core_names，按服务器地址认：路由器上改了名、还没同步时也认得出）
pub fn local_names(c: &Dict) -> Dict {
    if client_builtin(c) {
        core_names(c)
    } else {
        Dict::new()
    }
}

/// 可选节点在节点列表 have=[(名字, 摘要)] 里的叫法（按服务器地址认，见 node_match）；认不出的不在结果里
fn names_in(c: &Dict, have: &[(String, String)]) -> Dict {
    let out = Dict::new();
    let ids = node_ids(c);
    for n in nodes_of(c) {
        // 不是字符串的可选节点：记下的摘要里查不到它，节点列表里也没有它的名字，认不出
        let V::Str(n) = hashable(&n) else { continue };
        let found = node_match(n, &ids.get(n).unwrap_or_else(|| "".into()), have);
        if !found.is_empty() {
            out.set(n.clone(), found);
        }
    }
    out
}

/// 账号的节点在内置代理的节点定义里的叫法（换算不了就是原名）
pub fn cmd_local_node(args: &[String]) -> R<()> {
    arity("cmd_local_node", args, 1, 1)?;
    let m = local_names(&cfg_load()?);
    out!("{}", m.get(&args[0]).map_or_else(|| args[0].clone(), |found| found.py_str()));
    Ok(())
}

pub fn local_label(_client: &str) -> &'static str {
    "内置代理"
}

/// 内置代理（_gui state / live）：配置过没有
pub fn local_info(c: &Dict) -> Dict {
    let client = if client_builtin(c) { "builtin" } else { "" };
    dict! { "configured" => !client.is_empty(), "client" => client, "name" => local_label(client) }
}

pub fn cmd_local_env(args: &[String]) -> R<()> {
    arity("cmd_local_env", args, 0, 0)?;
    let (c, _, _) = ctx()?;
    let loc = match c.at("local") {
        V::Dict(loc) => loc,
        other => panic!("AttributeError: '{}' object has no attribute 'get'", type_name(&other)),
    };
    // 旧配置里的 clash-verge / manual：当作没配置（下次 cca local setup 配成内置代理）
    let builtin = matches!(loc.get("client"), Some(V::Str(s)) if &*s == "builtin");
    let client = if builtin { "builtin" } else { "" };
    let n = pool_size(&c);
    let ctl = if builtin { "unix:@core" } else { "" }; // @core：bash 的 core_sock
    let want = local_want(&c)?;
    let b = if builtin { core_bind(&c, &[]) } else { None }; // local_want 刚按节点定义查过（一个进程只查一次）
    // 本机 UDP 走节点（在外面启动时 bash 的 udp_launch_check 看）
    let (unode, ustate) = if cfg!(target_os = "macos") { udp_state(&c) } else { (String::new(), "off") };
    let (iface, via, dns) = match b {
        Some(b) => (b.iface, b.via, b.dns.join(" ")),
        None => (String::new(), String::new(), String::new()),
    };
    sh_vals(&[
        ("LOCAL_CLIENT", client.into()),
        ("LOCAL_NAME", local_label(client).into()),
        ("LOCAL_CTL", ctl.into()),
        ("LOCAL_SECRET", "".into()),
        ("LOCAL_APPLIED", loc.get("applied").unwrap_or_else(|| "".into())),
        ("LOCAL_WANT", want.into()),
        ("LOCAL_SLOTS", n.into()),
        ("LOCAL_FIRST", FIRST_PORT.into()),
        ("LOCAL_LAST", (FIRST_PORT + n - 1).into()),
        ("LOCAL_BIND", iface.into()),
        ("LOCAL_BIND_VIA", via.into()),
        ("LOCAL_BIND_DNS", dns.into()),
        ("LOCAL_UDP", unode.into()),
        ("LOCAL_UDP_STATE", ustate.into()),
    ]);
    Ok(())
}

/// local-set 键 值 …；换客户端（或 client 设成空）时清掉旧的设置（关掉过内置代理的记号 core_off、本机 UDP 走节点 udp 也一起清掉）。
/// udp 设成空是关掉本机 UDP 走节点：去掉这一项
pub fn cmd_local_set(args: &[String]) -> R<()> {
    let c = cfg_load()?;
    let loc = c.at("local");
    // zip(kv[::2], kv[1::2])：两个两个地取，多出来的一个不管
    for [k, v] in args.as_chunks::<2>().0 {
        let (k, v) = (k.as_str(), v.as_str());
        let V::Dict(loc) = &loc else {
            panic!("AttributeError: config.json 的 local 不是对象（{}）", type_name(&loc));
        };
        if k == "client" && loc.get("client").is_none_or(|have| have.as_str() != Some(v)) {
            loc.clear();
        }
        if k == "udp" && v.is_empty() {
            loc.pop(k);
        } else if !v.is_empty() || k != "client" {
            loc.set(k, v);
        }
    }
    cfg_save(&c)
}

/// 每个槽位应选中的节点；local 时用内置代理节点定义里的叫法
pub fn cmd_select_lines(args: &[String]) -> R<()> {
    arity("cmd_select_lines", args, 0, 1)?;
    let (c, _, _) = ctx()?;
    let m = if arg(args, 0, "") == "local" { local_names(&c) } else { Dict::new() };
    for (i, n) in slot_nodes(&c).iter().enumerate() {
        out!("CCA-{}\t{}", i + 1, m.get(n).map_or_else(|| n.clone(), |found| found.py_str()));
    }
    Ok(())
}

/// 端口池每个槽位：组名、端口、应选中的节点、用它的账号
pub fn cmd_slot_rows(args: &[String]) -> R<()> {
    arity("cmd_slot_rows", args, 0, 0)?;
    let (c, _, _) = ctx()?;
    // who = {p: shown(tool, k) …}：端口 → 账号（同一个端口后写的盖掉先写的）
    let who: Vec<(V, String)> = pool(&c).into_iter().map(|(tool, k, _, p)| (hashable(&p).clone(), shown(tool, &k))).collect();
    for (g, p, order) in slot_orders(&c, true) {
        let user = who.iter().rev().find(|(port, _)| port.py_eq(&V::Int(p))).map_or("", |(_, name)| name.as_str());
        out!("{}\t{}\t{}\t{}", g, p, order[0].py_str(), user);
    }
    Ok(())
}

// --- 内置代理的节点定义 ---------------------------------------------------------

/// 在外面会走内置代理的机器：本机客户端就是它；或者没配置本机客户端、没关掉过内置代理（cca local off）
pub fn core_eligible(c: &Dict) -> bool {
    let loc = sect(c, "local");
    match loc.get("client").filter(|client| client.truthy()) {
        Some(client) => client.as_str() == Some("builtin"),
        None => !loc.truthy("core_off"),
    }
}

/// core/nodes.json：{'proxies': [节点定义…], 'saved_at': 时间, 'nodes': 拷的时候的可选节点}；没有、读不了时是空的
fn core_cache() -> Dict {
    match load_quiet(&CORE_NODES) {
        Some(V::Dict(j)) if j.list("proxies").is_some() => j,
        _ => Dict::new(),
    }
}

/// `core_cache().get('proxies', [])`
fn cached_proxies(j: &Dict) -> Vec<V> {
    j.list("proxies").map(|l| l.items()).unwrap_or_default()
}

/// core/custom-nodes.json 里用户自定义的节点定义；没有、读不了时是空的
pub fn core_custom() -> Vec<Dict> {
    match load_quiet(&CORE_CUSTOM).and_then(|j| j.as_dict()).and_then(|j| j.list("proxies")) {
        Some(px) => px.items().iter().filter_map(named).collect(),
        None => Vec::new(),
    }
}

/// 这个名字没被可选节点、路由器/参考节点、内置出站占用
fn node_name_free(c: &Dict, name: &str) -> R<bool> {
    let mut taken = py_set(c.get("nodes"))?;
    // {p['name'] for p in core_cache().get('proxies', [])}：缓存里有不像节点定义的项时是没人接的 TypeError / KeyError
    for p in cached_proxies(&core_cache()) {
        let n = match &p {
            V::Dict(p) => p.at("name"),
            other => panic!("TypeError: 节点缓存里有一项不是对象（{}）", type_name(other)),
        };
        taken.push(hashable(&n).clone());
    }
    if let Some(f) = policy_fill() {
        taken.extend(f.list("proxies").unwrap_or_default().items().iter().filter_map(|p| p.as_dict()).map(|p| p.at("name")));
    }
    Ok(!name.is_empty() && !taken.iter().any(|have| have.as_str() == Some(name)) && !FILL_BUILTIN.contains(&name))
}

/// stdin：用户粘贴的分享链接或 Clash 节点块。解析成一个节点、起个不冲突的名字，打印节点 JSON。
/// 解析不了时 stderr 说原因、退出码 1。还不写盘：bash 先拿内核 -t 校验过再 node-save
pub fn cmd_node_check(args: &[String]) -> R<()> {
    arity("cmd_node_check", args, 0, 0)?;
    let mut p = match node_parse(&io::read_stdin_text()) {
        Ok(p) => p,
        Err(e) if e.is("NodeParseError") => return die(format!("这段配置认不出来：{e}")),
        Err(e) => return Err(e),
    };
    let c = cfg_load()?;
    let cleaned = node_clean_name(p.get("name"));
    let base = if cleaned.is_empty() { p.get("type").and_then(|kind| kind.as_str()).expect("node_parse 核对过 type 是文字").to_string() } else { cleaned };
    let (mut name, mut i) = (base.clone(), 2);
    while !node_name_free(&c, &name)? {
        name = format!("{base}-{i}");
        i += 1;
    }
    p.set("name", name);
    out!("{}", p.dumps());
    Ok(())
}

/// `json.loads(sys.stdin.read())` 和它的 `p['name']`：不是 JSON、不是对象、没有 name 都是没人接的异常
fn node_from_stdin() -> R<(V, V)> {
    let p = json_loads(&io::read_stdin_text())?;
    let name = match &p {
        V::Dict(d) => d.at("name"),
        other => panic!("TypeError: 节点应该是 JSON 对象（{}）", type_name(other)),
    };
    Ok((p, name))
}

/// stdin：一个节点的 JSON。打印只含它的最小 mihomo 配置（JSON），给 bash 拿内核 -t 校验
pub fn cmd_node_testconfig(args: &[String]) -> R<()> {
    arity("cmd_node_testconfig", args, 0, 0)?;
    let (p, name) = node_from_stdin()?;
    let cfg = dict! {
        "allow-lan" => false, "bind-address" => "127.0.0.1", "mode" => "rule", "log-level" => "silent",
        "proxies" => list![p],
        "proxy-groups" => list![dict! { "name" => "CCA-TEST", "type" => "select", "proxies" => list![name] }],
        "rules" => list!["MATCH,REJECT"],
    };
    out!("{}", json::dumps(&V::Dict(cfg), json::LINE));
    Ok(())
}

/// stdin：一个（已校验过的）节点 JSON。存进 custom-nodes.json，名字加进可选节点、记下摘要。打印节点名
pub fn cmd_node_save(args: &[String]) -> R<()> {
    arity("cmd_node_save", args, 0, 0)?;
    let (p, name) = node_from_stdin()?;
    {
        let _lock = Lock::acquire()?;
        let mut cur: Vec<V> = core_custom().into_iter().filter(|q| !q.at("name").py_eq(&name)).map(V::Dict).collect();
        cur.push(p.clone());
        save_json(&CORE_CUSTOM, &V::Dict(dict! { "version" => 1, "proxies" => cur }), Some(0o600), false)?;
        let c = cfg_load()?;
        if !py_contains(c.get("nodes"), &name) {
            let mut nodes = nodes_or_empty(&c);
            nodes.push(name.clone());
            c.set("nodes", nodes);
        }
        let ids = node_ids(&c).copy();
        ids.set(json_key(&name), fill_fp(&p.as_dict().expect("node_from_stdin 核对过是对象")));
        c.set("node_ids", ids);
        cfg_save(&c)?;
    }
    out!("{}", name.py_str());
    Ok(())
}

/// 自定义节点：名字<TAB>类型<TAB>服务器:端口<TAB>用它的账号（跨工具）
pub fn cmd_node_ls(args: &[String]) -> R<()> {
    arity("cmd_node_ls", args, 0, 0)?;
    let users = node_users_all();
    for p in core_custom() {
        let name = def_name(&p);
        let col = |k: &str| p.get(k).filter(|v| v.truthy()).map_or(String::new(), |v| v.py_str());
        let who = users.iter().find(|(node, _)| **node == *name).map_or(String::new(), |(_, names)| names.join("、"));
        out!("{}\t{}\t{}:{}\t{}", name, col("type"), col("server"), col("port"), who);
    }
    Ok(())
}

/// 删一个自定义节点：从 custom-nodes.json 和可选节点里去掉（还有账号在用时退出码 2、stdout 是用它的账号）
pub fn cmd_node_rm(args: &[String]) -> R<()> {
    arity("cmd_node_rm", args, 1, 1)?;
    let name = args[0].as_str();
    {
        let _lock = Lock::acquire()?;
        if !core_custom().iter().any(|p| &*def_name(p) == name) {
            return die(format!("没有这个自定义节点：{name}"));
        }
        let users = node_users_all().into_iter().find(|(node, _)| node == name).map(|(_, names)| names).unwrap_or_default();
        if !users.is_empty() {
            out!("{}", users.join("、"));
            return exit(2);
        }
        let rest: Vec<V> = core_custom().into_iter().filter(|p| &*def_name(p) != name).map(V::Dict).collect();
        save_json(&CORE_CUSTOM, &V::Dict(dict! { "version" => 1, "proxies" => rest }), Some(0o600), false)?;
        let c = cfg_load()?;
        let nodes: Vec<V> = nodes_or_empty(&c).into_iter().filter(|n| n.as_str() != Some(name)).collect();
        c.set("nodes", nodes);
        let ids = node_ids(&c).copy();
        ids.pop(name);
        c.set("node_ids", ids);
        cfg_save(&c)?;
    }
    out!("{name}");
    Ok(())
}

/// `time.time() - (j.get('saved_at') or 0)`：节点定义是多久以前拷的。saved_at 不是数字在 Python 里是没人接的 TypeError，
/// 是大到放不进小数的整数（三百多位）时是没人接的 OverflowError
fn cache_age(j: &Dict) -> f64 {
    let saved = match j.get("saved_at").filter(|at| at.truthy()) {
        Some(at) => {
            let n = at.as_pynum().unwrap_or_else(|| panic!("TypeError: unsupported operand type(s) for -: 'float' and '{}'", type_name(&at)));
            if n.is_infinite() && !matches!(at, V::Float(_)) {
                panic!("OverflowError: int too large to convert to float");
            }
            n
        }
        None => 0.0,
    };
    time() - saved
}

/// 小数的 `x // n`（x 不小于 0、n 大于 0）：照 CPython 的 `float_floor_div`——先取余数（`fmod`，不丢精度），减掉以后再除，
/// 凑到最近的整数。x 很大（一亿年以上的秒数）时和 `floor(x / n)` 差 1：那边先除，四舍五入可能进到下一个整数
fn py_floordiv(x: f64, n: f64) -> f64 {
    let div = (x - x % n) / n;
    if div == 0.0 {
        return 0.0;
    }
    let whole = div.floor();
    if div - whole > 0.5 {
        whole + 1.0
    } else {
        whole
    }
}

/// 在家时该不该从路由器重新拷节点定义：会走内置代理、配置了路由器，而缓存不在、超过 12 小时、可选节点变了
pub fn core_stale(c: &Dict) -> bool {
    if !core_eligible(c) || !router_host(c) {
        return false;
    }
    let j = core_cache();
    j.is_empty() || cache_age(&j) > CORE_STALE_AFTER || !j.get("nodes").unwrap_or(V::None).py_eq(&c.at("nodes"))
}

/// 内置代理能用的节点定义（还没按可选节点挑）：路由器上拷来的，加上维护者参考节点里要补的（和往路由器里补的
/// 同一套规则 fill_plan：同名的、服务器一样的以路由器的为准）
pub fn core_defs() -> Vec<Dict> {
    let mut px: Vec<Dict> = cached_proxies(&core_cache()).iter().filter_map(named).collect();
    px.extend(core_custom());
    let have = fps_of(&px);
    let names: Vec<String> = have.iter().map(|(n, _)| n.clone()).collect();
    px.extend(fill_plan(&have, &names, policy_fill().as_ref()).0);
    px
}

/// 可选节点在内置代理的节点定义里叫什么（按服务器地址认：路由器上改了名、还没同步时也认得出）
pub fn core_names(c: &Dict) -> Dict {
    names_in(c, &fps_of(&core_defs()))
}

/// `core_pick` 顺着 dialer-proxy 最多走几层（Python 3.14 下 core-missing 这条路实测的层数：996 层还行，997 层 RecursionError）
const PICK_DEPTH: usize = 996;

/// 节点定义 px 里，names 要的节点和它们（一层层）经 dialer-proxy 用到的节点。dialer-proxy 指向的不是节点（策略组、找不到、
/// 绕成圈）的不要：内置代理里没有那些组，带上它整份配置都加载不了。顺序照 px，同名的留第一个
fn core_pick(px: &[Dict], names: &[V]) -> Vec<Dict> {
    let mut order: Vec<(Rc<str>, &Dict)> = Vec::new();
    let mut by: HashMap<Rc<str>, usize> = HashMap::new(); // by.setdefault(p['name'], p)
    for p in px {
        if let Some(n) = p.str("name") {
            if !by.contains_key(&n) {
                by.insert(n.clone(), order.len());
                order.push((n, p));
            }
        }
    }
    let mut keep: HashMap<Rc<str>, bool> = HashMap::new();

    fn need(n: &str, path: &mut Vec<Rc<str>>, order: &[(Rc<str>, &Dict)], by: &HashMap<Rc<str>, usize>, keep: &mut HashMap<Rc<str>, bool>) -> bool {
        // 一层层递归：Python 到递归上限是没人接的 RecursionError。具体第几层随子命令（调用栈有多深）和 Python 的版本差一两层，
        // 这里固定成 PICK_DEPTH 层（见 FINDINGS.md）
        if path.len() >= PICK_DEPTH {
            panic!("RecursionError: maximum recursion depth exceeded");
        }
        if let Some(known) = keep.get(n) {
            return *known;
        }
        let Some((name, p)) = by.get(n).map(|i| &order[*i]) else {
            return false;
        };
        if path.iter().any(|seen| &**seen == n) {
            return false;
        }
        let ok = match p.get("dialer-proxy") {
            None | Some(V::None) => true,
            Some(V::Str(dp)) => {
                path.push(name.clone());
                let ok = need(&dp, path, order, by, keep);
                path.pop();
                ok
            }
            Some(_) => false,
        };
        keep.insert(name.clone(), ok);
        ok
    }

    for n in names {
        if let V::Str(n) = hashable(n) {
            need(n, &mut Vec::new(), &order, &by, &mut keep);
        }
    }
    order.into_iter().filter(|(n, _)| keep.get(n).copied().unwrap_or(false)).map(|(_, p)| p.clone()).collect()
}

/// `[m.get(n, n) for n in c['nodes']]`
fn wanted_names(c: &Dict, m: &Dict) -> Vec<V> {
    nodes_of(c).iter().map(|n| mapped(m, n)).collect()
}

/// 可选节点里内置代理没有定义的（名字和服务器地址都对不上）
pub fn core_missing(c: &Dict) -> Vec<V> {
    let m = core_names(c);
    let have: Vec<Rc<str>> = core_pick(&core_defs(), &wanted_names(c, &m)).iter().map(def_name).collect();
    nodes_of(c).into_iter().filter(|n| !matches!(mapped(&m, n), V::Str(found) if have.contains(&found))).collect()
}

/// 内置代理里放哪些节点定义：可选节点（按内置代理里的叫法）和它们经 dialer-proxy 用到的
fn core_node_defs(c: &Dict, m: Option<&Dict>) -> Vec<Dict> {
    let own;
    let m = match m {
        Some(m) => m,
        None => {
            own = local_names(c);
            &own
        }
    };
    core_pick(&core_defs(), &wanted_names(c, m))
}

/// 内置代理的配置（mihomo）：只有本机 127.0.0.1 上的入站 cca-n，各自直通策略组 CCA-n（成员：分到的节点
/// 排第一，没有定义的拿掉，分到的没有就 REJECT 排第一）；不开通用的代理端口、系统代理、TUN、DNS，规则只有一条 MATCH,REJECT
/// （入站都指定了组，用不到规则）；控制接口只开 Unix socket；不记选中的节点（重新加载后选组里第一个，就是账号的节点）。
/// 别的程序开着 TUN 时出站绑定到物理网卡（core_bind；系统的 DNS 到不了时另配物理网卡的 DNS），没开时和以前逐字节一样
fn core_config(c: &Dict, sock: &str) -> Dict {
    let defs = core_node_defs(c, None);
    let have: Vec<Rc<str>> = defs.iter().map(def_name).collect();
    let (groups, listeners) = (List::new(), List::new());
    for (g, port, order) in slot_orders(c, true) {
        groups.push(dict! { "name" => &g, "type" => "select", "proxies" => local_members(&order, &have) });
        listeners.push(dict! { "name" => g.to_lowercase(), "type" => "mixed", "listen" => "127.0.0.1", "port" => port, "users" => List::new(), "proxy" => &g });
    }
    let cfg = dict! {
        "allow-lan" => false, "bind-address" => "127.0.0.1", "mode" => "rule", "log-level" => "warning", "find-process-mode" => "off",
        "geo-auto-update" => false, "profile" => dict! { "store-selected" => false, "store-fake-ip" => false },
        "external-controller-unix" => sock, "proxies" => defs_list(&defs), "proxy-groups" => groups, "listeners" => listeners,
        "rules" => list!["MATCH,REJECT"],
    };
    core_bind_apply(cfg, c, &defs)
}

fn core_dir() -> std::io::Result<()> {
    fs::makedirs_mode(&CORE_DIR, 0o700)?;
    fs::chmod(&CORE_DIR, 0o700)
}

/// 配置（JSON 写的，mihomo 照 YAML 读）写成文字：`json.dumps(conf, ensure_ascii=False, indent=1) + '\n'`
fn config_text(conf: &Dict) -> String {
    json::dumps(&V::Dict(conf.clone()), json::Fmt::new().utf8().indent(1)) + "\n"
}

/// 写内置代理的配置 core/config.yaml（JSON 写的，mihomo 照 YAML 读）：内容变了才写，界面 App 看着这个文件，一变就重新读 state
pub fn cmd_core_config(args: &[String]) -> R<()> {
    arity("cmd_core_config", args, 1, 1)?;
    let (c, _, _) = ctx()?;
    let text = config_text(&core_config(&c, &args[0]));
    match fs::read_text(&CORE_CONFIG) {
        Ok(old) if old == text => return Ok(()),
        Ok(_) => {}
        Err(e) if e.is_os() => {}
        Err(e) => return Err(e), // 不是 UTF-8：原来是没人接的 UnicodeDecodeError
    }
    core_dir()?;
    let tmp = format!("{}.{}.tmp", *CORE_CONFIG, std::process::id());
    std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?.write_all(text.as_bytes())?;
    std::fs::rename(&tmp, &*CORE_CONFIG)?;
    Ok(())
}

/// stdin 是路由器端 defs 给的节点定义（JSON 的 {"proxies": [...]}，后面跟着结束标记）→ core/nodes.json：只留可选节点（按名字和
/// 服务器地址认）和它们经 dialer-proxy 用到的，再记下拷的时间和当时的可选节点（变了就重新拷）。没拷完整、读不懂的不写
pub fn cmd_core_nodes_save(args: &[String]) -> R<()> {
    arity("cmd_core_nodes_save", args, 0, 0)?;
    let c = cfg_load()?;
    let s = io::read_stdin_text();
    let Some(body) = s.strip_suffix(DEFS_END) else {
        return die("节点定义没拷完整");
    };
    let px = match json_loads(body) {
        Ok(V::Dict(j)) => j.list("proxies"),
        _ => None,
    };
    let Some(px) = px else {
        return die("读不懂路由器给的节点定义");
    };
    let px: Vec<Dict> = px.items().iter().filter_map(named).collect();
    let m = names_in(&c, &fps_of(&px));
    core_dir()?;
    let picked = core_pick(&px, &wanted_names(&c, &m));
    let data = dict! { "version" => 1, "saved_at" => time(), "nodes" => c.at("nodes"), "proxies" => defs_list(&picked) };
    save_json(&CORE_NODES, &V::Dict(data), Some(0o600), false)
}

/// 在家启动后要在后台做的（cca _core-nodes 的参数）：nodes 从路由器拷节点定义，bin 下载内核文件（已经用内置代理、文件还不在）。
/// 只读 config.json、节点缓存：每次在家启动都会跑，不能慢
pub fn cmd_core_stale(args: &[String]) -> R<()> {
    arity("cmd_core_stale", args, 0, 1)?;
    let c = cfg_load()?;
    let mut todo = Vec::new();
    if core_stale(&c) {
        todo.push("nodes");
    }
    if client_builtin(&c) && arg(args, 0, "").is_empty() && router_host(&c) {
        todo.push("bin");
    }
    out!("{}", todo.join(" "));
    Ok(())
}

pub fn cmd_core_missing(args: &[String]) -> R<()> {
    arity("cmd_core_missing", args, 0, 0)?;
    out!("{}", join_strs(&core_missing(&cfg_load()?)));
    Ok(())
}

/// cca local status 里内置代理的一行：节点定义从哪来、多久以前拷的、缺哪些
pub fn cmd_core_summary(args: &[String]) -> R<()> {
    arity("cmd_core_summary", args, 0, 0)?;
    let c = cfg_load()?;
    let j = core_cache();
    let src = if j.is_empty() {
        "还没从路由器拷过（在家启动账号时会拷）".to_string()
    } else {
        // age = max(0, …)：NaN、负数都是 0。时间差是无穷大（saved_at 写成 -Infinity）时 Python 里是没人接的 ValueError
        let age = cache_age(&j);
        let age = if age > 0.0 { age } else { 0.0 };
        if age.is_infinite() {
            panic!("ValueError: cannot convert float NaN to integer");
        }
        // '%d' % (age // n)：小数的整除，再照整数写（很大的数也写全）
        let whole = |n: f64| format!("{:.0}", py_floordiv(age, n));
        let when = if age < 3600.0 {
            format!("{} 分钟前", whole(60.0))
        } else if age < 86400.0 * 2.0 {
            format!("{} 小时前", whole(3600.0))
        } else {
            format!("{} 天前", whole(86400.0))
        };
        format!("从路由器拷的 {} 个（{}）", cached_proxies(&j).len(), when)
    };
    // names = {p['name'] for p in core_cache().get('proxies', []) if isinstance(p, dict)}：对象里没有 name 是没人接的 KeyError
    let names: Vec<V> = cached_proxies(&core_cache()).iter().filter_map(|p| p.as_dict()).map(|p| hashable(&p.at("name")).clone()).collect();
    let m = core_names(&c);
    // 可选节点真正用到的（参考节点只算补进来的）
    let used = core_pick(&core_defs(), &wanted_names(&c, &m));
    let extra = used.iter().map(def_name).filter(|n| !names.iter().any(|have| have.as_str() == Some(n))).count();
    let miss = core_missing(&c);
    let filled = if extra > 0 { format!("，补上维护者参考节点 {extra} 个") } else { String::new() };
    let lacking = if miss.is_empty() { String::new() } else { format!("；没有定义的节点：{}（用它们的账号在外面会被拒绝连接）", join_strs(&miss)) };
    out!("节点定义：{src}{filled}{lacking}");
    Ok(())
}

// --- 辅助安装（bash 的“辅助安装”一节）：直连装不上 claude / codex / agy 时，经内置代理的一个节点装 ------------------------------

/// 这台机器拿得到定义、能拿来装的节点（内置代理的节点定义：路由器上拷来的，加上维护者参考节点里补的），同名的只算一次
pub fn install_names() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in core_defs() {
        let n = def_name(&p);
        if !out.iter().any(|have| **have == *n) {
            out.push(n.to_string());
        }
    }
    out
}

/// 映射要的安装节点在这里的节点定义里叫什么：同名的；路由器上改过名、cca 跟着改过的（node_alias）；名字都对不上时按服务器地址认
/// （参考节点里同名那个的摘要，或者可选节点记下的摘要）。认不出是空的
fn install_resolve(c: &Dict, want: &str) -> String {
    let defs = core_defs();
    for n in [want.to_string(), node_alias(Some(c), want)] {
        if defs.iter().any(|p| *def_name(p) == *n) {
            return n;
        }
    }
    let mut fps: Vec<String> = match policy_fill() {
        Some(f) => f.list("proxies").unwrap_or_default().items().iter().filter_map(|p| p.as_dict()).filter(|p| p.str("name").as_deref() == Some(want)).map(|p| fill_fp(&p)).collect(),
        None => Vec::new(),
    };
    // fps.add(node_ids(c)[want])：记下的摘要不是文字的（手改的）永远对不上
    if let Some(V::Str(id)) = node_ids(c).get(want).filter(|id| id.truthy()).map(|id| hashable(&id).clone()) {
        fps.push(id.to_string());
    }
    fps.retain(|f| !f.is_empty());
    defs.iter().find(|p| fps.contains(&fill_fp(p))).map_or(String::new(), |p| def_name(p).to_string())
}

/// 临时内核的配置（JSON 写的，mihomo 照 YAML 读）：只有 127.0.0.1 上一个入站 cca-install，直通只有这个节点的组（和它经 dialer-proxy
/// 用到的节点）；规则只有 MATCH,REJECT，不开控制接口、通用的代理端口、系统代理、TUN、DNS（开着 TUN 时和内置代理一样绑定物理网卡，
/// 见 core_bind）。节点没有定义时是 None
fn install_proxy_config(name: &str, port: u16) -> Option<Dict> {
    let defs = core_pick(&core_defs(), &[V::from(name)]);
    if !defs.iter().any(|p| &*def_name(p) == name) {
        return None;
    }
    let c = match load_quiet(&CFG) {
        Some(V::Dict(c)) => c,
        _ => Dict::new(),
    };
    let cfg = dict! {
        "allow-lan" => false, "bind-address" => "127.0.0.1", "mode" => "rule", "log-level" => "warning", "find-process-mode" => "off",
        "geo-auto-update" => false, "profile" => dict! { "store-selected" => false, "store-fake-ip" => false },
        "proxies" => defs_list(&defs), "proxy-groups" => list![dict! { "name" => INSTALL_GROUP, "type" => "select", "proxies" => list![name] }],
        "listeners" => list![dict! { "name" => "cca-install", "type" => "mixed", "listen" => "127.0.0.1", "port" => port, "users" => List::new(), "proxy" => INSTALL_GROUP }],
        "rules" => list!["MATCH,REJECT"],
    };
    Some(core_bind_apply(cfg, &c, &defs))
}

/// `T.install_env(folder)`：官方安装脚本在空环境里运行时另加的变量。claude install 用一次性的配置目录（不往账号和 ~/.claude 的
/// 配置里写，不带设备标识，和客户端更新里的 claude update 一样）；codex 不问问题（stdin 是 /dev/null）；agy 没有
fn install_env(folder: &str) -> R<Vec<(String, String)>> {
    match t().kind {
        Kind::Claude => cu_claude_env(folder),
        Kind::Codex => Ok(vec![("CODEX_NON_INTERACTIVE".into(), "1".into())]),
        Kind::Agy => Ok(Vec::new()),
    }
}

/// 辅助安装：官方安装脚本用什么 shell 运行（bash 的 INSTALL_SHELL），在空环境上另加哪些变量（数组 INSTALL_ENV）。folder 是这次的临时目录
pub fn cmd_install_env(args: &[String]) -> R<()> {
    arity("cmd_install_env", args, 1, 1)?;
    sh(&[("INSTALL_SHELL", t().install_shell)]);
    let mut env = install_env(&args[0])?;
    env.sort();
    sh_array("INSTALL_ENV", &env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>());
    Ok(())
}

/// 辅助安装的节点。get：映射指定的安装节点在这里叫什么（退出码 3：映射没指定；4：指定了、这台机器没有它的定义，stdout 是映射里写的
/// 名字）；choices：这台机器拿得到定义的节点，一行一个（现选用）；proxy 节点 文件：把临时内核的配置写进文件（600，新建），打印端口
pub fn cmd_install_node(args: &[String]) -> R<()> {
    arity("cmd_install_node", args, 0, 3)?;
    let (action, name, file) = (arg(args, 0, "get"), arg(args, 1, ""), arg(args, 2, ""));
    let c = cfg_load()?;
    if action == "get" {
        let want = policy_install_node();
        if want.is_empty() {
            return exit(3);
        }
        let got = install_resolve(&c, &want);
        out!("{}", if got.is_empty() { &want } else { &got });
        return exit(if got.is_empty() { 4 } else { 0 });
    }
    if action == "choices" {
        for n in install_names() {
            out!("{n}");
        }
        return Ok(());
    }
    if action != "proxy" {
        return die("用法: install-node get|choices|proxy 节点 文件");
    }
    // 系统给的空闲端口（临时端口段，碰不到端口池的 79xx）；关掉后到内核绑定之间被别人占了，内核起不来，bash 照实说
    let port = std::net::TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port();
    let Some(conf) = install_proxy_config(name, port) else {
        return die(format!("内置代理里没有节点 {name} 的定义"));
    };
    std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(file)?.write_all(config_text(&conf).as_bytes())?;
    out!("{port}");
    Ok(())
}

/// 把 src 的内容原子地换进 dst（保留 dst 的权限），然后删掉 src。（现在的 bash 不调它了；留着是因为原来的 CMDS 里有）
pub fn cmd_file_replace(args: &[String]) -> R<()> {
    arity("cmd_file_replace", args, 2, 2)?;
    // open(src, encoding="utf-8", newline="")：换行原样，不是 UTF-8 是没人接的 UnicodeDecodeError
    let raw = std::fs::read(&args[0])?;
    let content = match std::str::from_utf8(&raw) {
        Ok(content) => content,
        Err(e) => return Err(crate::py::Fail::Value(crate::py::json::utf8_error_text(&raw, &e))),
    };
    write_text(&args[1], content)?;
    std::fs::remove_file(&args[0])?;
    Ok(())
}

/// 原子替换文本文件，保留原来的权限（客户端随时可能来读）。现在只有 file-replace 用它
pub fn write_text(p: &str, content: &str) -> R<()> {
    let mode = match fs::mode_of(p) {
        Ok(mode) => mode,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0o644,
        Err(e) => return Err(e.into()),
    };
    let tmp = format!("{}.cca-{}.tmp", p, std::process::id());
    std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o666).open(&tmp)?.write_all(content.as_bytes())?;
    fs::chmod(&tmp, mode)?;
    std::fs::rename(&tmp, p)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, dialer: Option<V>) -> Dict {
        let p = dict! { "name" => name, "type" => "ss", "server" => format!("{name}.example.com"), "port" => 8388 };
        if let Some(dp) = dialer {
            p.set("dialer-proxy", dp);
        }
        p
    }

    fn picked(px: &[Dict], names: &[&str]) -> Vec<String> {
        core_pick(px, &names.iter().map(|n| V::from(*n)).collect::<Vec<_>>()).iter().map(|p| def_name(p).to_string()).collect()
    }

    #[test]
    fn local_info_like_python() {
        // 右边是原来的 local_info 给的（_gui state / live 的 config.local）：只有 builtin 算配置过，名字总是「内置代理」
        let info = |c: Dict| json::dumps(&V::Dict(local_info(&c)), json::LINE);
        let (on, off) = (r#"{"configured": true, "client": "builtin", "name": "内置代理"}"#, r#"{"configured": false, "client": "", "name": "内置代理"}"#);
        assert_eq!(info(dict! { "local" => dict! { "client" => "builtin", "udp" => "US-A" } }), on);
        for local in [V::Dict(dict! { "client" => "clash-verge" }), V::Dict(dict! { "client" => "" }), V::Dict(Dict::new()), V::None, V::Dict(dict! { "client" => 5 }),
                      V::List(List::new()), V::Dict(dict! { "client" => "Builtin" })] {
            assert_eq!(info(dict! { "local" => local }), off);
        }
        assert_eq!(info(Dict::new()), off);
        assert_eq!([local_label("builtin"), local_label(""), local_label("x")], ["内置代理"; 3]);
    }

    #[test]
    fn floor_division_like_python() {
        // (x, n, 原来的 x // n)：右边是真的 Python 算的。前九行是 floor(x / n) 会多 1（有一行多 2）的
        for (x, n, want) in [
            (7.18591708146297e+16, 60.0, 1197652846910494.0),
            (2.3500226223506874e+17, 60.0, 3916704370584478.0),
            (3.690181914010943e+17, 60.0, 6150303190018237.0),
            (3.0356666330571936e+19, 3600.0, 8432407314047759.0),
            (3.0474563443844493e+19, 3600.0, 8465156512179025.0),
            (1.6230156883271571e+19, 3600.0, 4508376912019880.0),
            (4.049206297974861e+19, 86400.0, 468658136339682.0),
            (7.48512119185738e+20, 86400.0, 8663334712797892.0),
            (3.1723819521640314e+20, 86400.0, 3671738370560220.0),
            (0.0, 60.0, 0.0),
            (59.99999999999999, 60.0, 0.0),
            (60.0, 60.0, 1.0),
            (3599.9999999999995, 60.0, 59.0),
            (3599.9999999999995, 3600.0, 0.0),
            (172799.99999999997, 3600.0, 47.0),
            (172800.0, 86400.0, 2.0),
            (1795000000.123, 86400.0, 20775.0),
            (1795000000.123, 3600.0, 498611.0),
            (1e+300, 86400.0, 1.1574074074074075e+295),
            (9007199254740992.0, 60.0, 150119987579016.0),
            (1.7976931348623157e+308, 86400.0, 2.0806633505350878e+303),
            (5e-324, 60.0, 0.0),
            (86399.99999999999, 86400.0, 0.0),
            (1e+16, 60.0, 166666666666666.0),
            (1.2345678901234568e+17, 3600.0, 34293552503429.0),
        ] {
            assert_eq!(py_floordiv(x, n), want, "{x} // {n}");
        }
        // '%d' % (1e22 // 86400)、'%d' % (2.5e15 // 60)
        assert_eq!(format!("{:.0}", py_floordiv(1e22, 86400.0)), "115740740740740736");
        assert_eq!(format!("{:.0}", py_floordiv(2.5e15, 60.0)), "41666666666666");
        assert_eq!(format!("{:.0}", py_floordiv(0.0, 60.0)), "0");
    }

    #[test]
    fn write_text_keeps_mode() {
        let d = std::env::temp_dir().join(format!("cca-corenet-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("f.txt");
        let p = p.to_str().unwrap();
        write_text(p, "一\n").unwrap();
        assert_eq!((std::fs::read_to_string(p).unwrap().as_str(), fs::mode_of(p).unwrap()), ("一\n", 0o644));
        fs::chmod(p, 0o600).unwrap();
        write_text(p, "二").unwrap();
        assert_eq!((std::fs::read_to_string(p).unwrap().as_str(), fs::mode_of(p).unwrap()), ("二", 0o600));
        assert_eq!(fs::listdir_sorted(d.to_str().unwrap()).unwrap(), ["f.txt"]); // 临时文件没留下
        assert!(write_text(&format!("{p}/x"), "").is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn pick_follows_dialer_proxy() {
        let px = [
            node("US-A", None),
            node("US-B", Some("HK-A".into())),
            node("HK-A", None),
            node("US-C", Some("GROUP".into())),
            node("US-D", Some(V::Int(5))),
            node("L1", Some("L2".into())),
            node("L2", Some("L1".into())),
            node("N1", Some(V::None)),
            node("C1", Some("C2".into())),
            node("C2", Some("C3".into())),
            node("C3", None),
            node("US-A", Some("nowhere".into())),
        ];
        assert_eq!(picked(&px, &["US-A"]), ["US-A"]);
        assert_eq!(picked(&px, &["US-B"]), ["US-B", "HK-A"]);
        assert_eq!(picked(&px, &["HK-A", "US-B", "US-A"]), ["US-A", "US-B", "HK-A"]);
        assert!(picked(&px, &["US-C", "US-D", "L1", "L2", "nope", ""]).is_empty());
        assert_eq!(picked(&px, &["N1", "C1"]), ["N1", "C1", "C2", "C3"]);
        assert_eq!(picked(&px, &["C2"]), ["C2", "C3"]);
        assert!(core_pick(&px, &[V::Int(1), V::None, V::Bool(true)]).is_empty());
        assert!(picked(&[], &["US-A"]).is_empty());
    }

    /// n 个节点一个接一个经 dialer-proxy 连成一串（最后一个直接连）
    fn chain(n: usize) -> Vec<Dict> {
        (0..n).map(|i| node(&format!("n{i}"), (i + 1 < n).then(|| V::from(format!("n{}", i + 1))))).collect()
    }

    #[test]
    fn pick_goes_as_deep_as_python() {
        // 原来的 core-missing（Python 3.14）：996 层还行，997 层是 RecursionError
        assert_eq!(picked(&chain(996), &["n0"]).len(), 996);
        // 从中间开始的、已经算过的那一段不再往下走，不嫌深
        assert_eq!(picked(&chain(1500), &["n600", "n0"]).len(), 1500);
    }

    #[test]
    #[should_panic(expected = "RecursionError")]
    fn pick_stops_where_python_does() {
        picked(&chain(997), &["n0"]);
    }

    #[test]
    fn members_keep_assigned_node_first() {
        let have: Vec<Rc<str>> = ["US-A", "US-B"].iter().map(|s| Rc::from(*s)).collect();
        let m = |order: &[&str]| local_members(&order.iter().map(|n| V::from(*n)).collect::<Vec<_>>(), &have).iter().map(|v| v.py_str()).collect::<Vec<_>>();
        assert_eq!(m(&["US-A", "US-B", "HK-A", "REJECT"]), ["US-A", "US-B", "REJECT"]);
        assert_eq!(m(&["HK-A", "US-A", "US-B", "REJECT"]), ["REJECT", "US-A", "US-B"]);
        assert_eq!(m(&["REJECT", "US-A", "HK-A"]), ["REJECT", "US-A"]);
        assert_eq!(m(&["HK-A", "REJECT"]), ["REJECT"]);
    }
}
