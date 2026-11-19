//! 多设备共用一个路由器的合并（`router_merge`、`cmd_router_merge`、`cmd_slot_name`、`cmd_router_evict`）、节点改名跟着走
//! （`node_match` 到 `cmd_nodes_rename`）、fill-check / fill-summary / nodes-fill、policy-check / policy-request / policy-lock。
//!
//! 移植了：router-merge、slot-name、router-evict。其余还没移植。

use super::table::all_errors;
use super::{arity, Cmd};
use crate::core::base::*;
use crate::core::ctx::{cfg_save, ctx, POOL_STEP};
use crate::core::model::*;
use crate::core::policy::policy_fill;
use crate::core::sync::router_accounts;
use crate::core::tool::{tools, Tool};
use crate::py::io;
use crate::py::json;
use crate::py::num::ceil_to;
use crate::py::obj::{Dict, List, V};
use crate::py::rand::token_hex;
use crate::py::text;
use crate::py::{die, Fail, R};
use crate::{dict, out};

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[
    Cmd::all("router-merge", cmd_router_merge),
    Cmd::all("slot-name", cmd_slot_name),
    Cmd::all("router-evict", cmd_router_evict),
];

/// 这台设备的随机标识（多设备共用一个路由器时写进路由器的 owners，合并时据此认出哪些槽位是别的设备的）：懒生成，非隐私、
/// 不含主机名。调用方（cmd_router_merge，非只读）负责 cfg_save 落盘
pub fn device_id(c: &Dict) -> V {
    match c.get("device_id").filter(|d| d.truthy()) {
        Some(d) => d,
        None => {
            let d = V::from(token_hex(8));
            c.set("device_id", &d);
            d
        }
    }
}

/// `int(x)`：槽位这类值当整数用（不是数字在 Python 里是没人接的 TypeError / ValueError）
fn py_int(v: &V) -> i64 {
    match v {
        V::Int(i) => *i,
        V::Bool(b) => *b as i64,
        V::Float(f) => f.trunc() as i64,
        V::Str(s) => text::to_int(s).unwrap_or_else(|| panic!("ValueError: invalid literal for int() with base 10: {}", v.py_repr())),
        _ => panic!("TypeError: int() argument must be a string or a real number, not {}", v.py_repr()),
    }
}

/// `isinstance(x, int)` 的值
fn int_of(v: &V) -> Option<i64> {
    match v {
        V::Int(i) => Some(*i),
        V::Bool(b) => Some(*b as i64),
        _ => None,
    }
}

/// 回读来的账号里，owners 标着是别的设备的那些：有名字、槽位有主、主不是我
fn others_of(racc: &[V], rown: &Dict, did: &V) -> Vec<Dict> {
    racc.iter()
        .filter_map(|a| a.as_dict())
        .filter(|a| {
            let owner = rown.get(&a.get("slot").unwrap_or(V::None).py_str());
            a.truthy("name") && owner.is_some_and(|o| o.truthy() && !o.py_eq(did))
        })
        .collect()
}

/// `max(POOL_STEP, -(-top // POOL_STEP) * POOL_STEP)`
fn slots_for(top: i64) -> i64 {
    POOL_STEP.max(ceil_to(top, POOL_STEP))
}

/// 多设备共用一个路由器：把本机账号合并进路由器现有的并集。remote 是回读来的 {'accounts': [...], 'owners': {槽位: 设备标识}}。
/// owners 认出哪些槽位是别的设备的（原样保留）、哪些是我自己的（不在账号表里的就是我删掉的，去掉）。owners 读不到（旧版 cca、
/// 全新路由器、升级前的备份还原）时当作全是我的 → 直接覆盖（和以前一样）。会写回 c：本机账号的端口改成并集里的槽位。
/// 返回 (accounts 载荷, owners)；单设备时 accounts 载荷和 router_accounts(c) 逐字节一样
pub fn router_merge(c: &Dict, remote: &Dict) -> (Dict, Dict) {
    let did = device_id(c);
    let racc = remote.list("accounts").map(|l| l.items()).unwrap_or_default();
    let rown = remote.dict("owners").unwrap_or_default();
    let owner_of = |s: &V| rown.get(&s.py_str());
    let mine: Vec<(String, std::rc::Rc<str>, std::rc::Rc<str>, &'static Tool)> =
        pool(c).into_iter().map(|(tool, k, v, _)| (format!("{}{}", tool.pool_prefix, acct_id(&k, tool)), node_of(&v), k, tool)).collect();
    // 别的设备的槽位：owners 里标着、且不是我这台（owners 读不到时谁都不是"别的设备"，走单设备的覆盖）
    let others = others_of(&racc, &rown, &did);
    if others.is_empty() {
        // 单设备 / 只有我这台：accounts.yaml 和以前逐字节一样（我删掉的账号不在 router_accounts 里，自动去掉）
        let payload = router_accounts(c);
        let owners = Dict::new();
        for a in payload.list("accounts").unwrap_or_default().items() {
            owners.set(a.as_dict().expect("router_accounts 给的是字典").at("slot").py_str(), &did);
        }
        return (payload, owners);
    }
    // 多设备：按身份（账号名）合并成并集。共用的账号（两台都有）天然共用一个槽位、同一个出口
    let mut remote_slot: Vec<(V, i64)> = Vec::new(); // 字典：同名的后一个盖掉前一个
    for a in racc.iter().filter_map(|a| a.as_dict()) {
        if let Some(s) = a.get("slot").as_ref().and_then(int_of) {
            let name = a.at("name");
            match remote_slot.iter_mut().find(|(n, _)| n.py_eq(&name)) {
                Some(e) => e.1 = s,
                None => remote_slot.push((name, s)),
            }
        }
    }
    let mut used: Vec<i64> = Vec::new();
    let mut union: Vec<Dict> = Vec::new();
    let owners = Dict::new();
    for a in &others {
        // 别的设备的槽位：原样保留，owner 不动
        let s = py_int(&a.at("slot"));
        if s > 0 && !used.contains(&s) {
            used.push(s);
            union.push(dict! { "slot" => s, "name" => a.at("name"), "node" => a.get("node").unwrap_or(V::None) });
            owners.set(s.to_string(), owner_of(&V::Int(s)).unwrap_or(V::None));
        }
    }
    let placed: Vec<V> = union.iter().map(|a| a.at("name")).collect();
    let mut mine_slot: Vec<(&'static str, std::rc::Rc<str>, i64)> = Vec::new();
    for (name, node, k, tool) in &mine {
        let name_v = V::from(name);
        if placed.iter().any(|p| p.py_eq(&name_v)) {
            // 共用账号：已经在并集里（别的设备推的），沿用它的槽位、不重复
            let s = union.iter().find(|a| a.at("name").py_eq(&name_v)).map(|a| py_int(&a.at("slot"))).expect("刚在 placed 里找到过");
            mine_slot.push((tool.name, k.clone(), s));
            continue;
        }
        // 我上次推过这个账号：沿用它的槽位
        let mut s = remote_slot.iter().find(|(n, _)| n.py_eq(&name_v)).map(|(_, s)| *s).filter(|s| *s > 0 && !used.contains(s)).unwrap_or(0);
        if s == 0 {
            s = 1;
            while used.contains(&s) {
                s += 1;
            }
        }
        used.push(s);
        union.push(dict! { "slot" => s, "name" => name, "node" => node });
        owners.set(s.to_string(), &did);
        mine_slot.push((tool.name, k.clone(), s));
    }
    let slots = used.iter().max().map_or(0, |m| slots_for(*m));
    let mut nodes: Vec<V> = Vec::new(); // list(dict.fromkeys(可选节点 + 别的设备用着的节点))
    let onodes = others.iter().filter_map(|a| a.get("node")).filter(|n| n.truthy() && n.as_str() != Some("REJECT"));
    for n in c.at("nodes").as_list().map(|l| l.items()).unwrap_or_default().into_iter().chain(onodes) {
        if !nodes.iter().any(|x| x.py_eq(&n)) {
            nodes.push(n);
        }
    }
    union.sort_by_key(|a| py_int(&a.at("slot")));
    let payload = dict! { "base" => FIRST_PORT - 1, "slots" => slots, "nodes" => nodes, "accounts" => List::from_vec(union.into_iter().map(V::Dict).collect()) };
    if let Some(f) = policy_fill() {
        payload.set("fill", f);
    }
    for tool in tools() {
        // 本机账号的端口改成并集里的槽位（启动、内置代理用的端口 = 并集槽位）
        if let Some(ports) = tool.state(c, false).dict("ports") {
            for key in ports.keys() {
                if let Some((_, _, s)) = mine_slot.iter().rev().find(|(t, k, _)| *t == tool.name && *k == key) {
                    ports.set(key, FIRST_PORT - 1 + s);
                }
            }
        }
    }
    (payload, owners)
}

/// stdin 里「accounts.yaml 的内容\n---CCA-OWN---\nowners 的内容」的两半（各自去掉前后的空白；没有分隔符时第二半是空的）
fn read_two_parts() -> (String, String) {
    let input = io::read_stdin_text();
    let (acc, _, own) = text::partition(&input, "\n---CCA-OWN---\n");
    (text::strip(acc).to_string(), text::strip(own).to_string())
}

/// 要推上去的四段：accounts.yaml / ports / select / owners
fn print_sections(payload: &Dict, base: i64, slots: i64, owners: &Dict) {
    let accounts = payload.list("accounts").unwrap_or_default().items();
    // 槽位 → 节点（字典：同一个槽位后写的盖掉先写的）
    let node_at = |s: i64| accounts.iter().rev().filter_map(|a| a.as_dict()).find(|a| a.at("slot").py_eq(&V::Int(s))).map(|a| a.at("node").py_str());
    out!("{}", json::dumps(&V::Dict(payload.clone()), json::LINE));
    out!("---CCA-PORTS---");
    for s in 1..=slots {
        out!("{}", base + s);
    }
    out!("---CCA-SELECT---");
    for s in 1..=slots {
        out!("CCA-{}\t{}", s, node_at(s).unwrap_or_else(|| "REJECT".to_string()));
    }
    out!("---CCA-OWN---");
    out!("{}", json::dumps(&V::Dict(owners.clone()), json::LINE));
}

/// 把路由器回读来的 accounts.yaml + owners（stdin，中间用 ---CCA-OWN--- 隔开；空的就是全新/单设备）和本机账号合并成并集，
/// 写回 config.json（本机端口 → 并集槽位、device_id），输出要推的 accounts.yaml / ports / select / owners 四段
pub fn cmd_router_merge(args: &[String]) -> R<()> {
    arity("cmd_router_merge", args, 0, 0)?;
    let c = ctx()?.0;
    let errs = all_errors();
    if !errs.is_empty() {
        return die(format!("账号表有错误，未同步：\n  {}", errs.join("\n  ")));
    }
    let (acc_txt, own_txt) = read_two_parts();
    let remote = Dict::new();
    if let Some(V::Dict(a)) = (!acc_txt.is_empty()).then(|| json::loads(&acc_txt).ok()).flatten() {
        remote.set("accounts", a.get("accounts").unwrap_or(V::None));
    }
    if let Some(V::Dict(o)) = (!own_txt.is_empty()).then(|| json::loads(&own_txt).ok()).flatten() {
        remote.set("owners", o);
    }
    let (payload, owners) = router_merge(&c, &remote);
    if !*RO {
        cfg_save(&c)?;
    }
    let (base, slots) = (py_int(&payload.at("base")), py_int(&payload.at("slots")));
    print_sections(&payload, base, slots, &owners);
    Ok(())
}

/// 回读的 accounts.yaml（stdin）里某个槽位上的账号名；route_router 用它校验路由器上这个端口现在是不是我这个账号
pub fn cmd_slot_name(args: &[String]) -> R<()> {
    arity("cmd_slot_name", args, 1, 1)?;
    let input = io::read_stdin_text();
    let d = json::loads(if input.is_empty() { "{}" } else { &input }).unwrap_or_else(|_| V::Dict(Dict::new()));
    let s = text::to_int(&args[0]).ok_or_else(|| Fail::Value(format!("invalid literal for int() with base 10: {}", crate::py::obj::repr_str(&args[0]))))?;
    let accounts = d.as_dict().and_then(|d| d.list("accounts")).map(|l| l.items()).unwrap_or_default();
    for a in accounts.iter().filter_map(|a| a.as_dict()) {
        if a.get("slot").is_some_and(|x| x.py_eq(&V::Int(s))) {
            out!("{}", a.get("name").filter(|n| n.truthy()).map_or(String::new(), |n| n.py_str()));
            return Ok(());
        }
    }
    out!();
    Ok(())
}

/// 卸载时：路由器回读的 accounts.yaml + owners（stdin，用 ---CCA-OWN--- 隔开）里去掉我这台的槽位，保留别的设备的。
/// 还有别的设备 → 输出剩下的 accounts.yaml / ports / select / owners 四段（调用方推上去，撤掉我的那部分）；
/// 没有别的设备（或 owners 读不到）→ 只打印 EMPTY，调用方据此整体拆除路由器
pub fn cmd_router_evict(args: &[String]) -> R<()> {
    arity("cmd_router_evict", args, 0, 0)?;
    let c = ctx()?.0;
    let did = device_id(&c);
    let (acc_txt, own_txt) = read_two_parts();
    let parse = |t: &str| if t.is_empty() { None } else { json::loads(t).ok() };
    let racc = parse(&acc_txt).and_then(|a| a.as_dict()).and_then(|a| a.list("accounts")).map(|l| l.items()).unwrap_or_default();
    let rown = parse(&own_txt).and_then(|o| o.as_dict()).unwrap_or_default();
    let mut others = others_of(&racc, &rown, &did);
    if others.is_empty() {
        if !*RO {
            cfg_save(&c)?; // 落盘 device_id（懒生成的）
        }
        out!("EMPTY");
        return Ok(());
    }
    let top = others.iter().map(|x| py_int(&x.at("slot"))).max().expect("others 不是空的");
    let slots = slots_for(top);
    let mut nodes: Vec<V> = Vec::new();
    for n in others.iter().filter_map(|x| x.get("node")).filter(|n| n.truthy() && n.as_str() != Some("REJECT")) {
        if !nodes.iter().any(|x| x.py_eq(&n)) {
            nodes.push(n);
        }
    }
    let owners = Dict::new();
    for x in &others {
        let slot = x.at("slot").py_str();
        owners.set(slot.as_str(), rown.get(&slot).unwrap_or(V::None));
    }
    others.sort_by_key(|x| py_int(&x.at("slot")));
    let accounts: Vec<V> = others.iter().map(|x| V::Dict(dict! { "slot" => x.at("slot"), "name" => x.at("name"), "node" => x.get("node").unwrap_or(V::None) })).collect();
    if !*RO {
        cfg_save(&c)?;
    }
    let payload = dict! { "base" => FIRST_PORT - 1, "slots" => slots, "nodes" => nodes, "accounts" => List::from_vec(accounts) };
    print_sections(&payload, FIRST_PORT - 1, slots, &owners);
    Ok(())
}
