//! 多设备共用一个路由器的合并（`router_merge`、`cmd_router_merge`、`cmd_slot_name`、`cmd_router_evict`）、节点改名跟着走
//! （`node_match` 到 `cmd_nodes_rename`）、fill-check / fill-summary / nodes-fill、policy-check / policy-request / policy-lock。
//!
//! 这一块的子命令都移植了。

use super::table::all_errors;
use super::{arg, arity, Cmd};
use crate::core::base::*;
use crate::core::ctx::{cfg_save, ctx, POOL_STEP};
use crate::core::model::*;
use crate::core::policy::{
    fill_fp, fill_name_ok, fill_plan, fill_problems, node_fp, node_ids, policy_fill, policy_map, policy_problems,
};
use crate::core::sync::router_accounts;
use crate::core::tool::{t, tools, Tool};
use crate::py::fs::{self, load_quiet, save_json};
use crate::py::io;
use crate::py::json;
use crate::py::num::ceil_to;
use crate::py::obj::{Dict, List, V};
use crate::py::path;
use crate::py::rand::token_hex;
use crate::py::text;
use crate::py::{die, exit, Fail, R};
use crate::{dict, err, out};

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[
    Cmd::all("router-merge", cmd_router_merge),
    Cmd::all("slot-name", cmd_slot_name),
    Cmd::all("router-evict", cmd_router_evict),
    Cmd::all("custom-clashes", cmd_custom_clashes),
    Cmd::all("policy-check", cmd_policy_check),
    Cmd::all("fill-check", cmd_fill_check),
    Cmd::all("fill-summary", cmd_fill_summary),
    Cmd::all("nodes-fill", cmd_nodes_fill),
    Cmd::all("nodes-follow", cmd_nodes_follow),
    Cmd::all("nodes-rename", cmd_nodes_rename),
    Cmd::all("policy-request", cmd_policy_request),
    Cmd::all("policy-lock", cmd_policy_lock),
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
        V::Str(s) => {
            text::to_int(s).unwrap_or_else(|| panic!("ValueError: invalid literal for int() with base 10: {}", v.py_repr()))
        }
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

/// 多设备共用一个路由器：把本机账号合并进路由器现有的并集。remote 是回读来的 {'accounts': [...], 'owners': {槽位: 设备标识},
/// 'custom': 账号表里的自定义节点}。
/// owners 认出哪些槽位是别的设备的（原样保留）、哪些是我自己的（不在账号表里的就是我删掉的，去掉）。owners 读不到（旧版 cca、
/// 全新路由器、升级前的备份还原）时当作全是我的 → 直接覆盖（和以前一样）。会写回 c：本机账号的端口改成并集里的槽位。
/// 返回 (accounts 载荷, owners)；单设备时 accounts 载荷和 router_accounts(c) 逐字节一样
pub fn router_merge(c: &Dict, remote: &Dict) -> (Dict, Dict) {
    let did = device_id(c);
    let racc = remote.list("accounts").map(|l| l.items()).unwrap_or_default();
    let rown = remote.dict("owners").unwrap_or_default();
    let owner_of = |s: &V| rown.get(&s.py_str());
    let mine: Vec<(String, std::rc::Rc<str>, std::rc::Rc<str>, &'static Tool)> = pool(c)
        .into_iter()
        .map(|(tool, k, v, _)| (format!("{}{}", tool.pool_prefix, acct_id(&k, tool)), node_of(&v), k, tool))
        .collect();
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
    let custom = super::corenet::custom_fill(c);
    let cks = super::corenet::custom_cks(custom.as_ref()); // 槽位的 ck 照这台的定义算：路由器上的对不上就断开
    let mut used: Vec<i64> = Vec::new();
    let mut union: Vec<Dict> = Vec::new();
    let owners = Dict::new();
    for a in &others {
        // 别的设备的槽位：原样保留（用它那台的自定义节点时带着的 ck 也是），owner 不动
        let s = py_int(&a.at("slot"));
        if s > 0 && !used.contains(&s) {
            used.push(s);
            union.push(keep_ck(dict! { "slot" => s, "name" => a.at("name"), "node" => a.get("node").unwrap_or(V::None) }, a));
            owners.set(s.to_string(), owner_of(&V::Int(s)).unwrap_or(V::None));
        }
    }
    let placed: Vec<V> = union.iter().map(|a| a.at("name")).collect();
    let mut mine_slot: Vec<(&'static str, std::rc::Rc<str>, i64)> = Vec::new();
    for (name, node, k, tool) in &mine {
        let name_v = V::from(name);
        if placed.iter().any(|p| p.py_eq(&name_v)) {
            // 共用账号：已经在并集里（别的设备推的），沿用它的槽位、不重复
            let s =
                union.iter().find(|a| a.at("name").py_eq(&name_v)).map(|a| py_int(&a.at("slot"))).expect("刚在 placed 里找到过");
            mine_slot.push((tool.name, k.clone(), s));
            continue;
        }
        // 我上次推过这个账号：沿用它的槽位
        let mut s = remote_slot
            .iter()
            .find(|(n, _)| n.py_eq(&name_v))
            .map(|(_, s)| *s)
            .filter(|s| *s > 0 && !used.contains(s))
            .unwrap_or(0);
        if s == 0 {
            s = 1;
            while used.contains(&s) {
                s += 1;
            }
        }
        used.push(s);
        let e = dict! { "slot" => s, "name" => name, "node" => node };
        super::corenet::slot_ck(&e, &cks);
        union.push(e);
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
    if let Some(f) = merged_custom(custom, others_custom(remote.get("custom"), &others)) {
        payload.set("custom", f);
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
    let node_at = |s: i64| {
        accounts.iter().rev().filter_map(|a| a.as_dict()).find(|a| a.at("slot").py_eq(&V::Int(s))).map(|a| a.at("node").py_str())
    };
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
        remote.set("custom", a.get("custom").unwrap_or(V::None)); // 别的设备在用的自定义节点照样带着（others_custom）
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
    let s = text::to_int(&args[0])
        .ok_or_else(|| Fail::Value(format!("invalid literal for int() with base 10: {}", crate::py::obj::repr_str(&args[0]))))?;
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

/// 别的设备在用的自定义节点：回读的 accounts.yaml 的 custom 里，名字是别的设备的槽位在用的、槽位的 ck 就是它（或者槽位没带 ck：
/// 旧版 cca 同步时丢了，它用的就是路由器上的那个）。同步时照样带上：订阅更新把配置文件整个换掉以后，OpenClash 启动时的钩子
/// 照账号表把别的设备的也写回去（不然要等那台再同步一次，期间它的账号断开）
fn others_custom(rcustom: Option<V>, others: &[Dict]) -> Vec<Dict> {
    let px = rcustom.as_ref().and_then(|f| f.as_dict()).and_then(|f| f.list("proxies")).map(|l| l.items()).unwrap_or_default();
    let mut out: Vec<Dict> = Vec::new();
    for p in px.iter().filter_map(|p| p.as_dict()) {
        let Some(name) = p.get("name").filter(|n| n.as_str().is_some_and(|s| !s.is_empty())) else { continue };
        if out.iter().any(|q| q.at("name").py_eq(&name)) {
            continue;
        }
        let ck = super::corenet::custom_ck(&p);
        let used = others.iter().any(|o| {
            o.get("node").is_some_and(|n| n.py_eq(&name))
                && match o.get("ck") {
                    Some(V::Str(s)) => ck.as_deref() == Some(&*s),
                    _ => true,
                }
        });
        if used {
            out.push(p);
        }
    }
    out
}

/// 多设备时账号表的 custom：这台的自定义节点（mine，custom_fill 的结果）加上别的设备在用的（theirs）。同名的：一样（ck 相同）
/// 的只留一个；不一样的留别的设备的（它的账号正用着路由器上的那个），这台的不写、提示换个名字——这台用它的槽位带着这台的 ck，
/// 路由器上对不上就断开，不换出口
fn merged_custom(mine: Option<Dict>, theirs: Vec<Dict>) -> Option<Dict> {
    use super::corenet::{custom_ck, custom_payload};
    let mine = mine.and_then(|f| f.list("proxies")).map(|l| l.items()).unwrap_or_default();
    let mut px: Vec<Dict> = Vec::new();
    for p in mine.iter().filter_map(|p| p.as_dict()) {
        let name = p.at("name");
        match theirs.iter().find(|q| q.at("name").py_eq(&name)) {
            Some(q) if custom_ck(q) != custom_ck(&p) => {
                if !*RO {
                    err!(
                        "! 自定义节点「{}」和别的设备写进路由器、还在用的同名节点不是同一个，没有写：这台用它的账号经路由器会断开（不换出口）。换个名字重新加（cca node add），再让账号改走它",
                        name.py_str()
                    );
                }
            }
            _ => px.push(p),
        }
    }
    for q in theirs {
        if !px.iter().any(|p| p.at("name").py_eq(&q.at("name"))) {
            px.push(q);
        }
    }
    custom_payload(px)
}

/// 同步前（stdin 是回读的「accounts.yaml\n---CCA-OWN---\nowners」）：这台的自定义节点里，和别的设备在路由器上正用着的同名节点
/// 不是同一个的（ck 不一样），一行一个名字。bash 先给它们换个名字（node-rename）再合并：不然 merged_custom 只能不写这台的，
/// 这台用它的槽位断开，`select_all` 切不过去，经路由器启动时别的账号也会停下
pub fn cmd_custom_clashes(args: &[String]) -> R<()> {
    arity("cmd_custom_clashes", args, 0, 0)?;
    let c = cfg_load()?;
    let (acc_txt, own_txt) = read_two_parts();
    let parse = |t: &str| if t.is_empty() { None } else { json::loads(t).ok() };
    let acc = parse(&acc_txt).and_then(|a| a.as_dict());
    let racc = acc.as_ref().and_then(|a| a.list("accounts")).map(|l| l.items()).unwrap_or_default();
    let rown = parse(&own_txt).and_then(|o| o.as_dict()).unwrap_or_default();
    let theirs = others_custom(acc.as_ref().and_then(|a| a.get("custom")), &others_of(&racc, &rown, &device_id(&c)));
    let mine = super::corenet::custom_fill(&c).and_then(|f| f.list("proxies")).map(|l| l.items()).unwrap_or_default();
    for p in mine.iter().filter_map(|p| p.as_dict()) {
        let name = p.at("name");
        if theirs.iter().any(|q| q.at("name").py_eq(&name) && super::corenet::custom_ck(q) != super::corenet::custom_ck(&p)) {
            out!("{}", name.py_str());
        }
    }
    Ok(())
}

/// 别的设备的槽位 e（从回读的那一项 a 照抄的）：a 带着自定义节点的 ck 时照样带上（路由器按它核对，见 `custom_ck`）
fn keep_ck(e: Dict, a: &Dict) -> Dict {
    if let Some(ck) = a.get("ck").filter(|v| v.as_str().is_some()) {
        e.set("ck", ck);
    }
    e
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
    let accounts: Vec<V> = others
        .iter()
        .map(|x| {
            V::Dict(keep_ck(
                dict! { "slot" => x.at("slot"), "name" => x.at("name"), "node" => x.get("node").unwrap_or(V::None) },
                x,
            ))
        })
        .collect();
    if !*RO {
        cfg_save(&c)?;
    }
    let payload = dict! { "base" => FIRST_PORT - 1, "slots" => slots, "nodes" => nodes, "accounts" => List::from_vec(accounts) };
    let rcustom = parse(&acc_txt).and_then(|a| a.as_dict()).and_then(|a| a.get("custom"));
    if let Some(f) = super::corenet::custom_payload(others_custom(rcustom, &others)) {
        payload.set("custom", f); // 留下的设备在用的自定义节点照样带着（钩子照它写回配置文件）
    }
    print_sections(&payload, FIRST_PORT - 1, slots, &owners);
    Ok(())
}

/// `json.load(open(path))`：读不了、不是 JSON 时给出 `str(e)`
fn load_or_why(p: &str) -> Result<V, String> {
    let raw = std::fs::read(p).map_err(|e| fs::open_error_text(&e, p))?;
    json::loads_file_bytes(&raw).map_err(|e| e.0)
}

/// policy-check / fill-check 共用：读文件、打印问题（stdout）、有问题退出码 1
fn check_file(p: &str, problems: impl Fn(&V) -> Vec<String>) -> R<()> {
    let j = match load_or_why(p) {
        Ok(j) => j,
        Err(why) => {
            out!("读不了，或者不是正确的 JSON：{why}");
            return exit(1);
        }
    };
    let errs = problems(&j);
    for e in &errs {
        out!("{e}");
    }
    exit(if errs.is_empty() { 0 } else { 1 })
}

/// 检查映射文件（拉下来验签之后、维护者发布之前）：有问题就打印出来（stdout）并返回 1
pub fn cmd_policy_check(args: &[String]) -> R<()> {
    arity("cmd_policy_check", args, 1, 2)?;
    let strict = arg(args, 1, "") == "strict";
    check_file(&args[0], |j| policy_problems(j, strict))
}

/// 检查参考节点文件（拉下来验签之后、维护者发布之前）：有问题就打印出来（stdout）并返回 1
pub fn cmd_fill_check(args: &[String]) -> R<()> {
    arity("cmd_fill_check", args, 1, 1)?;
    check_file(&args[0], fill_problems)
}

/// 参考节点的摘要，例如：策略组 OpenAi 的 11 个节点；没有参考节点时什么都不打印
pub fn cmd_fill_summary(args: &[String]) -> R<()> {
    arity("cmd_fill_summary", args, 0, 0)?;
    if let Some(f) = policy_fill() {
        out!("策略组 {} 的 {} 个节点", f.at("group").py_str(), f.list("proxies").map_or(0, |l| l.len()));
    }
    Ok(())
}

/// stdin 是路由器上的节点列表（cca-router.sh nodes 的输出），原样输出，后面接上参考节点、可选节点里的自定义节点里路由器上
/// 还没有的：同步时路由器会补上它们，可选节点里先就选得到，改名跟随也不会把它们当成丢了。没读到节点时只原样输出
pub fn cmd_nodes_fill(args: &[String]) -> R<()> {
    arity("cmd_nodes_fill", args, 0, 0)?;
    let input = io::read_stdin_text();
    crate::out_raw!("{input}");
    let have = parse_node_list(&input);
    if have.is_empty() {
        return Ok(());
    }
    if !input.ends_with('\n') {
        crate::out_raw!("\n");
    }
    let names: Vec<String> = input
        .split('\n')
        .map(|line| line.split('\t').collect::<Vec<_>>())
        .filter(|f| f.len() > 1 && matches!(f[0], "P" | "G" | "C"))
        .map(|f| f[1].to_string())
        .collect();
    let col = |v: Option<V>| match v {
        None | Some(V::None) => String::new(),
        Some(v) => v.py_str().replace(['\t', '\n'], " "),
    };
    let mut more = fill_plan(&have, &names, policy_fill().as_ref()).0;
    let custom = super::corenet::custom_fill(&cfg_load()?).and_then(|f| f.list("proxies")).map(|l| l.items()).unwrap_or_default();
    for p in custom.iter().filter_map(|p| p.as_dict()) {
        let n = p.at("name").py_str();
        if !names.contains(&n) && !more.iter().any(|q| q.at("name").py_str() == n) {
            more.push(p);
        }
    }
    for p in more {
        let sni = p.get("servername").filter(|v| v.truthy()).or_else(|| p.get("sni"));
        let cols = [col(p.get("name")), col(p.get("type")), col(p.get("server")), col(p.get("port")), col(sni)];
        out!("P\t{}", cols.join("\t"));
    }
    Ok(())
}

/// 路由器上 cca-router.sh nodes 的输出 → [(名字, 摘要)]；只给名字的（读不到地址）摘要是空的
pub fn parse_node_list(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.split('\n') {
        let f: Vec<&str> = line.split('\t').collect();
        if f[0] == "P" && f.len() > 1 && !f[1].is_empty() {
            let at = |i: usize| f.get(i).map(|s| V::from(*s));
            out.push((f[1].to_string(), node_fp(at(2), at(3), at(4), at(5))));
        }
    }
    out
}

/// 可选节点 n（记下的摘要 f）在节点列表 have=[(名字, 摘要)] 里现在叫什么：地址对上一个节点就是它（名字改没改都算，
/// 两个节点互换名字也认得出），对上好几个时同名的优先；地址对不上（换了服务器，或者以前没记摘要）再按名字。找不到返回空
pub fn node_match(n: &str, f: &V, have: &[(String, String)]) -> String {
    let cand: Vec<&str> =
        have.iter().filter(|(_, g)| f.truthy() && f.as_str() == Some(g.as_str())).map(|(m, _)| m.as_str()).collect();
    if cand.contains(&n) {
        return n.to_string();
    }
    if cand.len() == 1 {
        return cand[0].to_string();
    }
    if have.iter().any(|(m, _)| m == n) {
        n.to_string()
    } else {
        String::new()
    }
}

/// 各工具现在的账号表（文件不在、读不了、不是对象的跳过）
fn tables_on_disk() -> Vec<(&'static Tool, Dict)> {
    tools()
        .iter()
        .filter_map(|tool| {
            let table = if path::exists(&tool.table) { load_quiet(&tool.table) } else { None };
            Some((tool, table?.as_dict()?))
        })
        .collect()
}

/// 把改名 [(旧名, 新名)] 用到可选节点和各工具的账号表上（第三方 API 账号的 node 也改）。alias：按地址认出的改名，
/// 记下来（维护者的映射里还写着旧名字时靠它换算，见 node_alias）；用户自己选的代替节点不记，映射照样要求旧节点。
/// 返回 [(旧名, [受影响的账号])]
pub fn nodes_rename(c: &Dict, ren: &[(String, String)], alias: bool) -> R<Vec<(String, Vec<String>)>> {
    let to = |n: &str| ren.iter().find(|(o, _)| o == n).map(|(_, new)| new.as_str());
    let mut hit: Vec<(String, Vec<String>)> = ren.iter().map(|(o, _)| (o.clone(), Vec::new())).collect();
    for (tool, table) in tables_on_disk() {
        let mut changed = false;
        for (k, v) in table.items() {
            let n = node_of(&v);
            let Some(new) = to(&n) else { continue };
            match &v {
                V::Dict(d) => d.set("node", new),
                _ => table.set(k.clone(), new),
            }
            hit.iter_mut().find(|(o, _)| **o == *n).expect("ren 里的都在 hit 里").1.push(shown(tool, &k));
            changed = true;
        }
        if changed {
            save_json(&tool.table, &V::Dict(table), Some(0o600), true)?;
        }
    }
    let mut nodes: Vec<V> = Vec::new(); // list(dict.fromkeys(…))：换名后去重，保持先后
    for n in c.at("nodes").as_list().map(|l| l.items()).unwrap_or_default() {
        let n = match n.as_str().and_then(to) {
            Some(new) => V::from(new),
            None => n,
        };
        if !nodes.iter().any(|x| x.py_eq(&n)) {
            nodes.push(n);
        }
    }
    c.set("nodes", nodes);
    if alias {
        let r = c.dict("node_renames").map(|d| d.copy()).unwrap_or_default();
        for (o, n) in ren {
            r.pop(n); // 新名字以前是别的节点的旧名字：那条换算作废
            r.set(o.as_str(), n);
        }
        let items = r.items();
        let keep = items.len().saturating_sub(50);
        c.set("node_renames", Dict::from_pairs(items.into_iter().skip(keep)));
    }
    Ok(hit)
}

/// 记下各可选节点现在的摘要；暂时找不到的保留以前记的（它回来了、或者改了名都还认得出）
pub fn nodes_record(c: &Dict, have: &[(String, String)]) {
    // fp 是字典：同名的后一个盖掉前一个
    let fp = |n: &str| have.iter().rev().find(|(m, f)| m == n && !f.is_empty()).map(|(_, f)| V::from(f));
    let old = node_ids(c);
    let ids = Dict::new();
    // 自定义节点记它自己的摘要（路由器上同名的可能是别的节点：写不进去，账号的端口宁可断开）
    let own = super::corenet::core_custom();
    let own_fp = |n: &str| own.iter().find(|p| p.str("name").is_some_and(|m| &*m == n)).map(|p| V::from(fill_fp(p)));
    for n in c.at("nodes").as_list().map(|l| l.strs()).unwrap_or_default() {
        if let Some(f) = own_fp(&n).or_else(|| fp(&n)).or_else(|| old.get(&n).filter(|v| v.truthy())) {
            ids.set(n, f);
        }
    }
    if ids.is_empty() {
        c.pop("node_ids");
    } else {
        c.set("node_ids", ids);
    }
}

/// 各工具的账号表里，每个节点被哪些账号用着（跨工具的显示名）
pub fn node_users_all() -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for (tool, table) in tables_on_disk() {
        for (k, v) in table.items() {
            let n = node_of(&v);
            if n.is_empty() {
                continue;
            }
            match out.iter_mut().find(|(x, _)| **x == *n) {
                Some((_, names)) => names.push(shown(tool, &k)),
                None => out.push((n.to_string(), vec![shown(tool, &k)])),
            }
        }
    }
    out
}

/// stdin 是路由器上的节点列表。可选节点改了名的（按服务器地址认），可选节点和各工具的账号表跟着改；记下各可选节点的摘要。
/// stdout：每个改名一行 R<TAB>旧名<TAB>新名；找不到、又有账号在用的一行 L<TAB>旧名<TAB>用它的账号（顿号连起来）。
/// 没读到节点（路由器上的配置读不了）时什么都不改，返回 1。不经过 ctx：账号表这时可能正因为节点名对不上而校验不过
pub fn cmd_nodes_follow(args: &[String]) -> R<()> {
    arity("cmd_nodes_follow", args, 0, 0)?;
    let have = parse_node_list(&io::read_stdin_text());
    if have.is_empty() {
        return exit(1);
    }
    let c = cfg_load()?;
    let sorted = json::Fmt::new().sort_keys();
    let before = json::dumps(&V::Dict(c.clone()), sorted);
    // 可选节点里不是字符串的项（手改坏的配置）在 Python 里只会进 lost、后面又查不到用它的账号：等于跳过
    // 自定义节点归 cca 自己管（同步时写进路由器）：不跟着路由器上的改名走，路由器上暂时没有也不算丢了
    let own = super::corenet::custom_names();
    let nodes: Vec<String> = c
        .at("nodes")
        .as_list()
        .map(|l| l.strs().iter().map(|n| n.to_string()).filter(|n| !own.contains(n)).collect())
        .unwrap_or_default();
    let mut ren: Vec<(String, String)> = Vec::new(); // 字典：同一个旧名字只有一条
    let mut lost: Vec<String> = Vec::new();
    for n in &nodes {
        let m = node_match(n, &node_ids(&c).get(n).unwrap_or_else(|| "".into()), &have);
        if m.is_empty() {
            lost.push(n.clone());
        } else if m != *n && !ren.iter().any(|(o, _)| o == n) {
            ren.push((n.clone(), m));
        }
    }
    // 不自动合并：认到的新名字同时是好几个旧名字的、或者是另一个还在用的可选节点（地址一样的两个节点没了一个）。
    // 这不是改名，自动改过去会让账号悄悄换到另一个节点上，交给用户选
    let unsafe_ones: Vec<String> = ren
        .iter()
        .filter(|(_, m)| ren.iter().filter(|(_, x)| x == m).count() > 1 || (nodes.contains(m) && !ren.iter().any(|(o, _)| o == m)))
        .map(|(o, _)| o.clone())
        .collect();
    for o in unsafe_ones {
        ren.retain(|(x, _)| *x != o);
        lost.push(o);
    }
    let hit = if ren.is_empty() { Vec::new() } else { nodes_rename(&c, &ren, true)? };
    nodes_record(&c, &have);
    let r = c.dict("node_renames").unwrap_or_default();
    for o in r.keys() {
        // 旧名字在路由器上又出现了（给了新节点）：那条换算作废
        if have.iter().any(|(n, _)| **n == *o) {
            r.pop(&o);
        }
    }
    if r.is_empty() {
        c.pop("node_renames");
    }
    if json::dumps(&V::Dict(c.clone()), sorted) != before {
        cfg_save(&c)?;
    }
    for (o, n) in &ren {
        let who = &hit.iter().find(|(x, _)| x == o).expect("ren 里的都在 hit 里").1;
        let also = if who.is_empty() { String::new() } else { format!("和 {} 的出口", who.join("、")) };
        err!("! 节点「{o}」在路由器上改名为「{n}」（服务器地址没变）：可选节点{also}已跟着改");
        out!("R\t{o}\t{n}");
    }
    let users = node_users_all();
    for o in &lost {
        if let Some((_, names)) = users.iter().find(|(n, _)| n == o) {
            out!("L\t{}\t{}", o, names.join("、"));
        }
    }
    Ok(())
}

/// 可选节点 old 在路由器上找不到了，用户选了 new 代替（stdin 是路由器上的节点列表）：可选节点和账号表跟着改
pub fn cmd_nodes_rename(args: &[String]) -> R<()> {
    arity("cmd_nodes_rename", args, 2, 2)?;
    let have = parse_node_list(&io::read_stdin_text());
    if !have.iter().any(|(n, _)| *n == args[1]) {
        return die(format!("路由器上没有节点「{}」", args[1]));
    }
    let c = cfg_load()?;
    nodes_rename(&c, &[(args[0].clone(), args[1].clone())], false)?;
    nodes_record(&c, &have);
    cfg_save(&c)
}

/// cca [codex] policy request 账号（key 是 bash 经 resolve 找好的）：写一行维护者直接运行的命令（在 cca 的仓库里），把这个账号
/// 现在的节点锁进映射。不能请求时 stdout 是原因、退出码 2
pub fn cmd_policy_request(args: &[String]) -> R<()> {
    arity("cmd_policy_request", args, 1, 1)?;
    let (c, table, _) = ctx()?;
    let key = args[0].as_str();
    let e = parse_key(key).1;
    let pinned = if e.is_empty() { None } else { policy_map(t(), Some(&c)).get(&e.to_lowercase()).filter(|p| p.truthy()) };
    let v = table.at(key);
    let why = if is_api(&v) || e.is_empty() {
        Some(format!("{} 是第三方 API 账号：映射只管官方账号", label_of(key)))
    } else if !c.truthy("policy") {
        Some("这台机器没打开维护者的共享：先运行 cca policy on".to_string())
    } else {
        pinned.map(|p| format!("{} 已经按维护者的映射固定在 {} 上了", label_of(key), p.py_str()))
    };
    if let Some(why) = why {
        out!("{why}");
        return exit(2);
    }
    out!("./policy.sh lock {} {} {}", t().name, text::sh_quote(&e.to_lowercase()), text::sh_quote(&node_of(&v)));
    Ok(())
}

/// 维护者把账号锁到节点（policy.sh lock）：plan 说明会改什么，apply 写进 policy.json。退出码：0 有要改的（apply 时已改好，
/// stdout 是提交说明）；3 已经是这样了；1 参数或映射有问题（原因在 stdout）。命令多半是别人发来的：控制字符、不合规的值不收，
/// 也不原样显示，免得搅乱维护者的终端
pub fn cmd_policy_lock(args: &[String]) -> R<()> {
    arity("cmd_policy_lock", args, 0, 5)?;
    let (action, file, tool, email, node) =
        (arg(args, 0, ""), arg(args, 1, ""), arg(args, 2, ""), arg(args, 3, ""), arg(args, 4, ""));
    if !matches!(action, "plan" | "apply") || file.is_empty() {
        return die("用法: cca _policy-lock plan|apply policy.json 工具 邮箱 节点");
    }
    let names: Vec<(&str, &str)> = tools().iter().map(|x| (x.name, x.label)).collect();
    let mut errs = Vec::new();
    if !names.iter().any(|(n, _)| *n == tool) {
        let all: Vec<&str> = names.iter().map(|(n, _)| *n).collect();
        errs.push(format!("工具应该是 {} 或 {}", all[..all.len() - 1].join("、"), all[all.len() - 1]));
    }
    if email.chars().any(|ch| (ch as u32) < 0x20 || ch == '\u{7f}') || !EMAIL_RE.matches(email) {
        errs.push("邮箱不对".to_string());
    }
    if !fill_name_ok(&V::from(node)) {
        errs.push("节点名不对（不能是空的，首尾不能有空格，不能有控制字符，不能以 CCA- 开头）".to_string());
    }
    let j = load_quiet(file);
    if errs.is_empty() && !j.as_ref().is_some_and(|j| j.is_dict() && policy_problems(j, false).is_empty()) {
        errs.push(format!("{file} 读不出来，或者格式不对"));
    }
    if !errs.is_empty() {
        out!("{}", errs.join("\n"));
        return exit(1);
    }
    let j = j.and_then(|j| j.as_dict()).expect("上面查过是字典");
    let s = match j.setdefault(tool, Dict::new()) {
        V::Dict(d) => d,
        other => panic!("AttributeError: 映射里 {tool} 不是对象（{}）", other.py_repr()),
    };
    let email = email.to_lowercase();
    let key = s.keys().into_iter().find(|k| k.to_lowercase() == email).map_or(email.clone(), |k| k.to_string());
    let old = s.get(&key);
    let label = names.iter().find(|(n, _)| *n == tool).expect("上面查过").1;
    let head = format!("  {label}  {key}");
    if old.as_ref().is_some_and(|o| o.as_str() == Some(node)) {
        out!("{head} → {node}（已经是这样）");
        return exit(3);
    }
    s.set(key.as_str(), node);
    let mut lines = vec![match &old {
        None | Some(V::None) => format!("{head} → {node}（新加）"),
        Some(o) => format!("{}：{} → {}（换节点）", head, o.py_str(), node),
    }];
    let fill = load_quiet(&path::join(&path::dirname(file), "nodes.json"));
    let known: Vec<V> = fill
        .and_then(|f| f.as_dict())
        .and_then(|f| f.list("proxies"))
        .map(|l| l.items().iter().filter_map(|p| p.as_dict()).map(|p| p.get("name").unwrap_or(V::None)).collect())
        .unwrap_or_default();
    if !known.is_empty() && !known.iter().any(|n| n.as_str() == Some(node)) {
        lines.push(format!("  ! 节点 {node} 不在参考节点（nodes.json）里：路由器上没有这个节点的机器，这个账号会用不了"));
    }
    let errs = policy_problems(&V::Dict(j.clone()), true);
    if !errs.is_empty() {
        out!("写进去之后映射的检查不通过：\n{}", errs.join("\n"));
        return exit(1);
    }
    if action == "plan" {
        out!("{}", lines.join("\n"));
    } else {
        save_json(file, &V::Dict(j), None, true)?;
        out!("按请求锁定账号的节点：{key} → {node}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::corenet::custom_ck;

    fn px(name: &str, user: &str) -> Dict {
        dict! { "name" => name, "type" => "socks5", "server" => "gw.example.com", "port" => 1080, "username" => user, "password" => "x" }
    }

    fn names(f: &Option<Dict>) -> Vec<String> {
        f.as_ref()
            .and_then(|f| f.list("proxies"))
            .map(|l| l.items())
            .unwrap_or_default()
            .iter()
            .map(|p| p.as_dict().unwrap().at("name").py_str())
            .collect()
    }

    /// 别的设备在用的自定义节点照样带着：槽位的 ck 是它、或者没带 ck（旧版 cca 丢了）；ck 不是它的、没人用的不带
    #[test]
    fn others_custom_keeps_what_other_devices_use() {
        let (home, spare) = (px("Home IP", "b"), px("Spare", "b"));
        let rcustom = V::Dict(dict! { "id" => "x", "proxies" => vec![V::Dict(home.clone()), V::Dict(spare)] });
        let slot = |ck: Option<String>| {
            let o = dict! { "slot" => 3, "name" => "dev2", "node" => "Home IP" };
            if let Some(ck) = ck {
                o.set("ck", ck);
            }
            o
        };
        let got = |o: Dict| others_custom(Some(rcustom.clone()), &[o]).iter().map(|p| p.at("name").py_str()).collect::<Vec<_>>();
        assert_eq!(got(slot(custom_ck(&home))), ["Home IP"]);
        assert_eq!(got(slot(None)), ["Home IP"]);
        assert!(got(slot(Some("0000000000000000".into()))).is_empty());
        assert!(others_custom(None, &[slot(None)]).is_empty());
    }

    /// 同名的：一样的只留一个；不一样的留别的设备在用的，这台的不写
    #[test]
    fn merged_custom_prefers_what_other_devices_use() {
        let mine = |ps: Vec<Dict>| crate::cmd::corenet::custom_payload(ps);
        let theirs = vec![px("Home IP", "b")];
        let got = merged_custom(mine(vec![px("Home IP", "a"), px("Office", "a")]), theirs.clone());
        assert_eq!(names(&got), ["Office", "Home IP"]);
        let home = got.as_ref().unwrap().list("proxies").unwrap().items()[1].as_dict().unwrap();
        assert_eq!(home.at("username").py_str(), "b");
        assert_eq!(names(&merged_custom(mine(vec![px("Home IP", "b")]), theirs.clone())), ["Home IP"]);
        assert_eq!(names(&merged_custom(None, theirs)), ["Home IP"]);
        assert!(merged_custom(None, Vec::new()).is_none());
    }
}
