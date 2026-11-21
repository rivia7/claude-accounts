//! 维护者的映射（policy.json）和参考节点（nodes.json）：读出来、和账号表合并、查格式。
//! 对应 PY_SRC 的 `policy_raw` 到 `fill_plan`，加上 `node_fp` / `node_ids` / `policy_install_node`。

use super::base::*;
use super::model::*;
use super::tool::{tools, Tool};
use crate::py::fs::load_quiet;
use crate::py::hash::sha256_hex;
use crate::py::json;
use crate::py::obj::{Dict, List, V};
use crate::py::path;
use crate::py::text;
use crate::py::uni;
use crate::re;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::LazyLock;

/// 维护者的账号 → 节点映射（bash 的 policy_fetch 验签后才放进来）
pub static POLICY_FILE: LazyLock<String> = LazyLock::new(|| path::join_all(&[&CCA_HOME, "policy", "policy.json"]));
/// App 多久拉一次映射（App 不重算，看 state 的 next_check_epoch）；命令行还是启动时一天一次
pub const POLICY_POLL_INTERVAL: i64 = 3600;
/// 维护者的参考节点（bash 的 policy_fetch 验签后才放进来）
pub static FILL_FILE: LazyLock<String> = LazyLock::new(|| path::join_all(&[&CCA_HOME, "policy", "nodes.json"]));
pub const FILL_BUILTIN: &[&str] = &["DIRECT", "REJECT", "REJECT-DROP", "PASS", "COMPATIBLE", "GLOBAL"];

thread_local! {
    /// policy_on 的结果，按 config.json 的 (设备号, inode, 大小, 修改时间) 记着：一条命令里要问几十次（每张账号表对账都问），
    /// 不每次都解析 config.json；cfg_save 原子替换（换了 inode）、内容变了都会重新读
    static POLICY_ON: std::cell::Cell<Option<((u64, u64, u64, i64, i64), bool)>> = const { std::cell::Cell::new(None) };
}

/// 共享（维护者的映射）开着吗：config.json 里配置了映射仓库（`policy`）。`cca policy off` 去掉这一项（仓库记进 `policy_was`），
/// 这时磁盘上留着的 policy.json、nodes.json 一律不生效：关掉时正好在后台拉的那一次（`policy_hook` 另起的进程）可能在之后才写回来。
/// 没有 config.json 是关着；有、但读不出对象（手改坏了）时照旧看磁盘上的映射：关掉共享是原子地重写 config.json，不会把它写坏，
/// 坏了也不该让设备统计的上报进程停下来
pub fn policy_on() -> bool {
    use std::os::unix::fs::MetadataExt;
    let st = match std::fs::metadata(&*CFG) {
        Ok(st) => st,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    let sig = (st.dev(), st.ino(), st.size(), st.mtime(), st.mtime_nsec());
    if let Some((s, on)) = POLICY_ON.get() {
        if s == sig {
            return on;
        }
    }
    let on = match load_quiet(&CFG) {
        Some(V::Dict(c)) => c.truthy("policy"),
        _ => true,
    };
    POLICY_ON.set(Some((sig, on)));
    on
}

thread_local! {
    /// policy.json 解析好的样子，按文件的 (设备号, inode, 大小, 修改时间) 记着：对账时每张账号表要问好几次
    static POLICY_DOC: std::cell::RefCell<Option<((u64, u64, u64, i64, i64), Option<Dict>)>> = const { std::cell::RefCell::new(None) };
}

/// 生效的映射（policy.json）：共享开着、读得出 JSON 对象；否则 None
pub fn policy_doc() -> Option<Dict> {
    use std::os::unix::fs::MetadataExt;
    if !policy_on() {
        return None;
    }
    let st = std::fs::metadata(&*POLICY_FILE).ok()?;
    let sig = (st.dev(), st.ino(), st.size(), st.mtime(), st.mtime_nsec());
    if let Some(doc) = POLICY_DOC.with(|c| c.borrow().as_ref().filter(|(s, _)| *s == sig).map(|(_, d)| d.clone())) {
        return doc;
    }
    let doc = load_quiet(&POLICY_FILE).and_then(|j| j.as_dict());
    POLICY_DOC.with(|c| *c.borrow_mut() = Some((sig, doc.clone())));
    doc
}

/// 维护者的映射（旧的通道：policy.json）里这个工具的官方账号：{邮箱（小写）: 映射里写的节点}。没配置、共享关着、读不了、格式不对都当作没有
pub fn legacy_raw(tool: &Tool) -> Dict {
    let out = Dict::new();
    if let Some(s) = policy_doc().and_then(|j| j.dict(tool.name)) {
        for (e, n) in s.items() {
            if let V::Str(n) = n {
                if !n.is_empty() {
                    out.set(e.to_lowercase(), n);
                }
            }
        }
    }
    out
}

/// 这个工具的官方账号各由谁定节点：[(邮箱（小写）, 节点, 谁：Team 的编号，维护者的映射是空的)]。各个 Team 的配置在前（照加入的先后），
/// 维护者的映射在后；同一个账号先出现的算数。只有维护者的映射时，先后和它里面写的一样
fn mapping(tool: &Tool) -> Vec<(Rc<str>, V, Rc<str>)> {
    let mut out: Vec<(Rc<str>, V, Rc<str>)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for g in super::team::grants().iter() {
        let accounts = g.doc.dict("accounts").and_then(|a| a.dict(tool.name)).unwrap_or_default();
        for (e, n) in accounts.items() {
            let e = e.to_lowercase();
            if n.as_str().is_some_and(|n| !n.is_empty()) && seen.insert(e.clone()) {
                out.push((Rc::from(e), n, Rc::from(g.id.as_str())));
            }
        }
    }
    for (e, n) in legacy_raw(tool).items() {
        if seen.insert(e.to_string()) {
            out.push((e, n, Rc::from("")));
        }
    }
    out
}

/// 映射里这个工具的官方账号：{邮箱（小写）: 节点}——各个 Team 给这台设备的配置加上维护者的映射（见 mapping）
pub fn policy_raw(tool: &Tool) -> Dict {
    let out = Dict::new();
    for (e, n, _) in mapping(tool) {
        out.set(e, n);
    }
    out
}

/// 映射里这个工具的官方账号是谁定的：{邮箱（小写）: Team 的编号}，维护者的映射定的是空的
pub fn policy_sources(tool: &Tool) -> Dict {
    let out = Dict::new();
    for (e, _, who) in mapping(tool) {
        out.set(e, &*who);
    }
    out
}

/// 这个账号的节点是谁定的，写进给人看的话里：「维护者的映射」，或者「「Team 的名字」的配置」
pub fn by_whom(tool: &Tool, email: &str) -> String {
    match policy_sources(tool).str(&email.to_lowercase()).filter(|t| !t.is_empty()) {
        Some(tid) => format!("「{}」的配置", super::team::grant_name(&tid).unwrap_or_else(|| "Team".to_string())),
        None => "维护者的映射".to_string(),
    }
}

/// policy_raw，节点换成现在的名字：映射里写的是路由器上改名前的旧名字（cca 跟着改过名，维护者还没更新映射）时
/// 按记下的改名换算（node_alias），免得账号表被改回旧名字、整张表因为节点不在可选列表而用不了。
/// c 是 None 时现读 config.json（读不了就不换算）
pub fn policy_map(tool: &Tool, c: Option<&Dict>) -> Dict {
    let loaded;
    let c = match c {
        Some(c) => Some(c),
        None => {
            loaded = load_quiet(&CFG).and_then(|v| v.as_dict());
            loaded.as_ref()
        }
    };
    let out = Dict::new();
    for (e, n) in policy_raw(tool).items() {
        let n = n.rc_str().expect("policy_raw 只留字符串");
        out.set(e, node_alias(c, &n));
    }
    out
}

/// 节点名 n 不在可选节点里、又是 cca 跟着改过名的旧名字时，换成现在的名字（可能改过不止一次）
pub fn node_alias(c: Option<&Dict>, n: &str) -> String {
    let Some(c) = c else {
        return n.to_string();
    };
    let nodes = c.list("nodes").unwrap_or_default();
    let ren = c.dict("node_renames").unwrap_or_default();
    let mut n = n.to_string();
    let mut seen = HashSet::new();
    while !nodes.has_str(&n) && !seen.contains(&n) {
        let Some(next) = ren.str(&n) else { break };
        seen.insert(n.clone());
        n = next.to_string();
    }
    n
}

/// 别名合不合规：不能是空的，不能有空格、引号、括号、反斜杠或 @，最多 24 格宽
pub fn alias_ok(a: &V) -> bool {
    a.as_str().is_some_and(|s| !s.is_empty() && !ALIAS_BAD.found(s) && uni::width(s) <= 24)
}

/// 映射里给账号起的别名（可选的 aliases 一节，{"邮箱": "别名"}，各工具通用）：{邮箱（小写）: 别名}。不合规的不用。各个 Team 的配置里的在前
/// （先出现的算数），维护者的映射里的在后。
/// 旧版本的 cca 不认识这一节、照样忽略它：加了别名，旧版本也照常拉得到映射
pub fn policy_names() -> Dict {
    let out = Dict::new();
    let teams = super::team::grants();
    let docs = teams.iter().filter_map(|g| g.doc.dict("aliases")).chain(policy_doc().and_then(|j| j.dict("aliases")));
    for s in docs {
        for (e, a) in s.items() {
            let e = e.to_lowercase();
            if alias_ok(&a) && !out.has(&e) {
                out.set(e, a);
            }
        }
    }
    out
}

/// 这台机器用着这个工具吗：当前的工具，或者已经有它的账号表。映射里的 Codex 账号要到第一次用 cca codex 时
/// 才加进来，免得不用 Codex 的机器白占路由器上的端口
pub fn policy_in_use(tool: &Tool) -> bool {
    tool.is_current() || path::exists(&tool.table)
}

/// `set(tool.state(c).get('policy_skip') or [])`
fn policy_skip(c: &Dict, tool: &Tool) -> HashSet<String> {
    match tool.state(c, false).get("policy_skip") {
        Some(V::List(l)) => l.strs().iter().map(|s| s.to_string()).collect(),
        _ => HashSet::new(),
    }
}

/// 映射里有、这台机器的账号表里没有的官方账号 [(邮箱, 节点, 不加的原因)]；原因是空的，下一次 ctx 就把它加进来
pub fn policy_missing(c: &Dict, table: &Dict, tool: &Tool) -> Vec<(String, String, String)> {
    let have = table_emails_raw(table);
    let skip = policy_skip(c, tool);
    let nodes = c.get("nodes").unwrap_or(V::None);
    let mut out = Vec::new();
    for (e, n) in policy_map(tool, Some(c)).items() {
        let n = n.py_str();
        if have.contains(&*e) {
            continue;
        }
        let why = if !policy_in_use(tool) {
            format!("第一次用 {} 时加进来", tool.cmd)
        } else if skip.contains(&*e) {
            format!("这台机器上删掉过（要加回来：{} add）", tool.cmd)
        } else if !nodes.truthy() {
            let router_host = c.get("router").filter(|r| r.truthy()).and_then(|r| r.as_dict()).is_some_and(|r| r.truthy("host"));
            if router_host {
                "这台机器还没有可选节点（运行 cca router setup）".to_string()
            } else {
                "这台机器还没有可选节点（运行 cca router setup；连不上路由器的服务器上运行 cca install，导入维护者提供的节点）"
                    .to_string()
            }
        } else if !nodes.as_list().is_some_and(|l| l.has_str(&n)) {
            format!("节点 {n} 不在这台机器的可选节点里（运行 cca router setup 选上它）")
        } else if !KEY_RE.matches(&e) {
            "邮箱太长，账号表里放不下".to_string()
        } else {
            String::new()
        };
        out.push((e.to_string(), n, why));
    }
    out
}

/// `{parse_key(k)[1].lower() for k, v in t.items() if not is_api(v)}`（可能含空串）
fn table_emails_raw(table: &Dict) -> HashSet<String> {
    table.items().iter().filter(|(_, v)| !is_api(v)).map(|(k, _)| parse_key(k).1.to_lowercase()).collect()
}

/// 账号表里官方账号的邮箱（小写）
pub fn table_emails(table: &Dict) -> HashSet<String> {
    let mut s = table_emails_raw(table);
    s.remove("");
    s
}

/// 这个别名能不能用在账号表 t 里；能用返回空字符串
pub fn alias_problem(table: &Dict, alias: &str) -> String {
    if ALIAS_BAD.found(alias) {
        return "别名里不能有空格、引号、括号、反斜杠或 @".into();
    }
    if uni::width(alias) > 24 {
        return "别名太长了（最多 12 个汉字或 24 个字母）".into();
    }
    for k in table.keys() {
        if parse_key(&k).0 == alias {
            return format!("别名「{}」已经用在 {} 上了", alias, label_of(&k));
        }
    }
    String::new()
}

/// 关掉共享时收起来的账号（工具状态里的 `parked`：{账号目录名: {"key": 原来账号表里的键, "node": 节点}}）。它们已经不在
/// 账号表里，登录目录、钥匙串里的东西原样留着（secrets-forget、邮箱设置的清理都跳过它们），重新打开共享时按映射加回来
/// （policy_merge 照原来的键加：账号目录由键里邮箱的写法推出来，不用重新登录），加回来了就从这里去掉（reconcile）
pub fn parked(c: &Dict, tool: &Tool) -> Dict {
    tool.state(c, false).dict("parked").unwrap_or_default()
}

/// 收起来的账号里这个邮箱（小写）原来的键
fn parked_key(c: &Dict, tool: &Tool, e: &str) -> Option<String> {
    parked(c, tool)
        .values()
        .iter()
        .filter_map(|v| v.as_dict().and_then(|d| d.str("key")).map(|k| k.to_string()))
        .find(|k| parse_key(k).1.to_lowercase() == e)
}

/// 维护者的映射和这台机器的账号表合并：映射里有、这台机器上还没有的官方账号加进账号表，返回加进来的键。
/// 别名用映射里 aliases 写的（和这台机器上别的账号重名时只写邮箱）；关掉共享时收起来的照原来的键加回来。
/// 这台机器自己的账号（映射里没有的）、这台机器上改过的别名都不动；不加的见 policy_missing 的原因（cca policy 里列出来）
pub fn policy_merge(c: &Dict, table: &Dict, tool: &Tool) -> Vec<String> {
    let names = policy_names();
    let mut added = Vec::new();
    for (e, n, why) in policy_missing(c, table, tool) {
        if !why.is_empty() {
            continue;
        }
        let k = if let Some(old) = parked_key(c, tool, &e) {
            // 收起来的：原来的键（邮箱的大小写也照原来的：账号目录由它推出来）；别名撞上了这台机器上别的账号就只写邮箱
            let (a, em) = parse_key(&old);
            if a.is_empty() || alias_problem(table, &a).is_empty() {
                old
            } else {
                em
            }
        } else {
            let a = names.str(&e).unwrap_or_else(|| Rc::from(""));
            let k = if a.is_empty() { e.clone() } else { format!("{a}({e})") };
            if !a.is_empty() && (!alias_problem(table, &a).is_empty() || !KEY_RE.matches(&k)) {
                e.clone()
            } else {
                k
            }
        };
        table.set(k.as_str(), n);
        added.push(k);
    }
    added
}

/// 映射里的账号在这台机器上还没有别名（键只是邮箱）、映射里给它起了别名：补上，返回 [(旧键, 新键)]。比如先按没写
/// 别名的映射加进来的账号，维护者后来在 aliases 里补了别名。这台机器上起过别名的不改，别名和这台机器上别的账号重名的
/// 不补。只改键：账号目录、端口和上次使用都按邮箱认，跟着走（reconcile 里随后的端口分配按 acct_id 找回原来的端口）
pub fn policy_alias_fill(table: &Dict, tool: &Tool) -> Vec<(String, String)> {
    let (pm, names) = (policy_raw(tool), policy_names());
    let mut ren: Vec<(String, String)> = Vec::new();
    let mut used = HashSet::new();
    for (k, v) in table.items() {
        let (a, e) = parse_key(&k);
        let al = if e.is_empty() { Rc::from("") } else { names.str(&e.to_lowercase()).unwrap_or_else(|| Rc::from("")) };
        if is_api(&v) || !a.is_empty() || al.is_empty() || !pm.has(&e.to_lowercase()) || used.contains(&*al) {
            continue;
        }
        let new = format!("{al}({e})");
        if !alias_problem(table, &al).is_empty() || !KEY_RE.matches(&new) {
            continue;
        }
        ren.push((k.to_string(), new));
        used.insert(al.to_string());
    }
    if !ren.is_empty() {
        // 原地改（table 就是全局记着的那张表），顺序不变
        let items: Vec<(String, V)> = table
            .items()
            .into_iter()
            .map(|(k, v)| (ren.iter().find(|(old, _)| **old == *k).map_or_else(|| k.to_string(), |(_, new)| new.clone()), v))
            .collect();
        table.clear();
        for (k, v) in items {
            table.set(k, v);
        }
    }
    ren
}

/// 映射文件的问题：{"version": 1, "claude": {"邮箱": "节点"}, "codex": {…}, "aliases": {"邮箱": "别名"}, "install_node": "节点"}。
/// strict（维护者发布前查）时不认识的字段、aliases 里的问题也算：客户端照样忽略它们（不合规的别名不用），免得以后
/// 加了新东西，旧版本的 cca 就整个不认、拉不到新的节点
pub fn policy_problems(j: &V, strict: bool) -> Vec<String> {
    let Some(j) = j.as_dict() else {
        return vec![
            "应该是 JSON 对象，例如 {\"version\": 1, \"claude\": {\"name@example.com\": \"节点\"}, \"codex\": {}}".to_string()
        ];
    };
    let names: Vec<&str> = tools().iter().map(|x| x.name).collect();
    let mut errs = Vec::new();
    if !j.get("version").is_some_and(|v| v.py_eq(&V::Int(1))) {
        errs.push("version 应该是 1".to_string());
    }
    for n in &names {
        let s = match j.get(n) {
            None => Dict::new(),
            Some(V::Dict(d)) => d,
            Some(_) => {
                errs.push(format!("{n} 应该是 {{\"邮箱\": \"节点\"}} 形式的对象"));
                continue;
            }
        };
        let mut seen = HashSet::new();
        for (e, node) in s.items() {
            if !EMAIL_RE.matches(&e) {
                errs.push(format!("{n}：「{e}」不像邮箱"));
            }
            if !seen.insert(e.to_lowercase()) {
                errs.push(format!("{n}：{e} 写了不止一次（邮箱不分大小写）"));
            }
            if !node.as_str().is_some_and(|x| !text::strip(x).is_empty()) {
                errs.push(format!("{n}：{e} 的节点应该是非空字符串"));
            }
        }
    }
    if strict {
        errs.extend(alias_problems(&j, &names));
        if let Some(p) = j.get("presence") {
            // 设备统计的服务器（客户端只在完全合格时启用，不合格就当没有）
            errs.extend(crate::cmd::presence::presence_problems(&p));
        }
        if let Some(n) = j.get("install_node") {
            if !(fill_name_ok(&n) && !FILL_BUILTIN.contains(&n.as_str().unwrap_or(""))) {
                errs.push("install_node 应该是节点的名字（辅助安装直连不通时经内置代理走的节点）".to_string());
            }
        }
        if let Some(t) = j.get("team") {
            errs.extend(team_section_problems(&t));
        }
        for k in j.keys() {
            if !matches!(&*k, "version" | "aliases" | "presence" | "install_node" | "team") && !names.contains(&&*k) {
                errs.push(format!(
                    "不认识的字段 {}（只能有 version、{}、aliases、presence、install_node、team）",
                    k,
                    names.join("、")
                ));
            }
        }
    }
    errs
}

/// 映射里 team 一节的问题（维护者发布前查；./policy.sh team 写的：给各台机器的 Team 邀请，见 docs/dev/team.md「迁移」）：
/// {"v": 1, "invites": {deploy key 的指纹（SHA256:…）: 邀请链接}}。客户端只认完全合格的那一项（cmd/team.rs 的 policy_invite）
pub fn team_section_problems(t: &V) -> Vec<String> {
    let Some(t) = t.as_dict() else { return vec!["team 应该是 {\"v\": 1, \"invites\": {…}} 形式的对象".to_string()] };
    let mut errs = Vec::new();
    if !t.get("v").is_some_and(|v| v.py_eq(&V::Int(1))) {
        errs.push("team：v 应该是 1".to_string());
    }
    match t.get("invites") {
        Some(V::Dict(m)) => {
            for (fp, link) in m.items() {
                if !(fp.starts_with("SHA256:") && fp.len() == 50) {
                    errs.push(format!("team：「{fp}」不像 deploy key 的指纹（SHA256:…）"));
                }
                if !link.as_str().is_some_and(|l| crate::cmd::team::doc::invite_decode(l).is_ok()) {
                    errs.push(format!("team：{fp} 的邀请链接不对"));
                }
            }
        }
        _ => errs.push("team：invites 应该是 {\"指纹\": \"邀请链接\"} 形式的对象".to_string()),
    }
    for k in t.keys() {
        if !matches!(&*k, "v" | "invites") {
            errs.push(format!("team：不认识的字段 {k}"));
        }
    }
    errs
}

/// 映射里 aliases 一节的问题（维护者发布前查）：邮箱要在映射里，别名要合规，同一个工具里的两个账号不能同名
pub fn alias_problems(j: &Dict, names: &[&str]) -> Vec<String> {
    let al = match j.get("aliases") {
        None => Dict::new(),
        Some(V::Dict(d)) => d,
        Some(_) => return vec!["aliases 应该是 {\"邮箱\": \"别名\"} 形式的对象".to_string()],
    };
    let mut errs = Vec::new();
    let low = Dict::new();
    let mapped: HashSet<String> = names.iter().filter_map(|n| j.dict(n)).flat_map(|s| s.keys()).map(|e| e.to_lowercase()).collect();
    for (e, a) in al.items() {
        let el = e.to_lowercase();
        if !mapped.contains(&el) {
            errs.push(format!("aliases：{} 不在映射里（{} 里都没有这个邮箱）", e, names.join("、")));
        }
        if low.has(&el) {
            errs.push(format!("aliases：{e} 写了不止一次（邮箱不分大小写）"));
        }
        low.setdefault(&el, &a);
        if !alias_ok(&a) {
            errs.push(format!(
                "aliases：{} 的别名「{}」不行：不能是空的，不能有空格、引号、括号、反斜杠或 @，最多 12 个汉字或 24 个字母",
                e,
                a.py_str()
            ));
        }
    }
    for n in names {
        let mut seen: Vec<(String, Rc<str>)> = Vec::new();
        for e in j.dict(n).unwrap_or_default().keys() {
            let Some(a) = low.get(&e.to_lowercase()).filter(alias_ok) else { continue };
            let a = a.py_str();
            match seen.iter().find(|(x, _)| *x == a) {
                Some((_, first)) => errs.push(format!("aliases：{n} 里的 {first} 和 {e} 起了同一个别名「{a}」")),
                None => seen.push((a, e)),
            }
        }
    }
    errs
}

// --- 维护者参考配置里的节点（映射仓库里的 nodes.json，./policy.sh 从维护者的 custom.yaml 抽出一个策略组和它引用的节点）：
// 直接写进路由器（OpenClash 选用的配置、ShellCrash 的订阅配置），缺的补进 proxies、名字补进同名策略组。
// 路由器端 apply.rb 的 --fill-source 写配置文件；内置代理的参考节点直接进内核的节点定义、不写文件。
// 这里的 fill_plan 要和路由器端的同一套规则保持一致

/// 策略组、节点的名字：不能是空的，首尾不能有空格，不能有控制字符（路由器上按 TAB 分列），不能以 CCA- 开头（cca 自己的组）
pub fn fill_name_ok(n: &V) -> bool {
    n.as_str().is_some_and(|s| {
        let len = text::len(s);
        0 < len
            && len <= 128
            && s == text::strip(s)
            && !s.chars().any(|c| (c as u32) < 0x20 || c == '\u{7f}')
            && !s.starts_with("CCA-")
    })
}

/// 参考节点文件的问题：{"version": 1, "group": "策略组", "proxies": [节点定义, …]}（节点定义和 Clash 配置里的一样）。
/// 客户端和维护者发布前查的是同一套：文件是签过名的，有问题就整个不用
pub fn fill_problems(j: &V) -> Vec<String> {
    let Some(j) = j.as_dict() else {
        return vec!["应该是 JSON 对象，例如 {\"version\": 1, \"group\": \"OpenAi\", \"proxies\": [{\"name\": \"节点\", \"type\": \"vless\", …}]}".to_string()];
    };
    let mut errs = Vec::new();
    if !j.get("version").is_some_and(|v| v.py_eq(&V::Int(1))) {
        errs.push("version 应该是 1".to_string());
    }
    if !fill_name_ok(&j.get("group").unwrap_or(V::None)) {
        errs.push("group 应该是策略组的名字（不能是空的，不能有控制字符，不能以 CCA- 开头）".to_string());
    }
    let px = match j.get("proxies") {
        Some(V::List(l)) if !l.is_empty() => l,
        _ => {
            errs.push("proxies 应该是节点定义的列表（至少一个）".to_string());
            return errs;
        }
    };
    let mut seen: Vec<V> = Vec::new();
    for (i, p) in px.items().iter().enumerate() {
        let i = i + 1;
        let n = p.as_dict().and_then(|d| d.get("name")).unwrap_or(V::None);
        let named = match n.as_str() {
            Some(s) => format!("「{s}」"),
            None => String::new(),
        };
        let place = format!("第 {i} 个节点{named}");
        let Some(p) = p.as_dict() else {
            errs.push(format!("{place}应该是对象"));
            continue;
        };
        if !fill_name_ok(&n) || FILL_BUILTIN.contains(&n.as_str().unwrap_or("")) {
            errs.push(format!("{place}：name 不能是空的，首尾不能有空格，不能有控制字符，不能以 CCA- 开头，也不能是 DIRECT、REJECT 这些内置的名字"));
        } else if seen.iter().any(|x| x.py_eq(&n)) {
            errs.push(format!("{place}：名字重复了"));
        }
        seen.push(n);
        if !p.str("type").is_some_and(|x| re!(r"^[A-Za-z0-9_-]{1,32}$").matches(&x)) {
            errs.push(format!("{place}：type 不对"));
        }
        if !p.str("server").is_some_and(|x| re!(r"^[^\s\x00-\x1f\x7f]{1,255}$").matches(&x)) {
            errs.push(format!("{place}：server 不对"));
        }
        let mut port = p.get("port").unwrap_or(V::None);
        if let Some(s) = port.as_str() {
            if re!(r"^\d{1,5}$").matches(s) {
                port = V::Int(text::to_int(s).expect("刚核对过是数字"));
            }
        }
        if !port.as_int().is_some_and(|n| 0 < n && n < 65536) {
            errs.push(format!("{place}：port 应该是 1–65535"));
        }
    }
    errs
}

/// 维护者的参考节点（旧的通道：nodes.json）：(策略组, 节点定义)；没有、共享关着、读不了、格式不对都是 None
fn legacy_fill() -> Option<(V, V)> {
    if !policy_on() {
        return None;
    }
    let j = load_quiet(&FILL_FILE).unwrap_or(V::None);
    if !fill_problems(&j).is_empty() {
        return None;
    }
    let j = j.as_dict()?;
    Some((j.at("group"), j.at("proxies")))
}

/// 参考节点：{'id': 摘要, 'group': 策略组, 'proxies': [节点定义…]}——各个 Team 的配置里带的节点在前（照加入的先后），维护者的参考节点
/// 在后；同名的先出现的算数，策略组照第一份有节点的。都没有是 None。只有维护者的参考节点时和它原样一样（id 也一样）。
/// id 随内容变：路由器按它认这一版有没有在自己的内核上校验过
pub fn policy_fill() -> Option<Dict> {
    let teams = super::team::grants();
    let mut parts: Vec<(V, V)> =
        teams.iter().filter_map(|g| g.doc.dict("nodes")).map(|n| (n.at("group"), n.at("proxies"))).collect();
    let alone = parts.is_empty();
    parts.extend(legacy_fill());
    let group = parts.first()?.0.clone();
    let proxies = if alone {
        parts[0].1.clone()
    } else {
        let mut seen: HashSet<String> = HashSet::new();
        let mut all: Vec<V> = Vec::new();
        for p in parts.iter().filter_map(|(_, p)| p.as_list()).flat_map(|l| l.items()) {
            if p.as_dict().and_then(|d| d.str("name")).is_some_and(|n| seen.insert(n.to_string())) {
                all.push(p);
            }
        }
        V::from(all)
    };
    let s = json::dumps(&V::from(vec![group.clone(), proxies.clone()]), json::Fmt::new().utf8().sort_keys());
    let out = Dict::new();
    out.set("id", &sha256_hex(s)[..16]);
    out.set("group", group);
    out.set("proxies", proxies);
    Some(out)
}

/// 节点的摘要：类型 + 服务器 + 端口 + TLS 域名（小写，去掉末尾的点），sha256 的前 16 位；没有服务器是空串。
/// config.json 的 node_ids 存的就是它，路由器上改了名也认得出是同一个节点
pub fn node_fp(kind: Option<V>, server: Option<V>, port: Option<V>, sni: Option<V>) -> String {
    let clean = |v: &Option<V>| text::strip(&str_or_empty(v)).to_lowercase();
    let server = clean(&server).trim_end_matches('.').to_string();
    if server.is_empty() {
        return String::new();
    }
    let s = format!("{}|{}|{}|{}", clean(&kind), server, text::strip(&str_or_empty(&port)), clean(&sni).trim_end_matches('.'));
    sha256_hex(s)[..16].to_string()
}

/// `c['node_ids']`（不是字典就当空的）
pub fn node_ids(c: &Dict) -> Dict {
    c.dict("node_ids").unwrap_or_default()
}

/// 参考节点里一个节点定义的摘要
pub fn fill_fp(p: &Dict) -> String {
    let sni = p.get("servername").filter(|v| v.truthy()).or_else(|| p.get("sni"));
    node_fp(p.get("type"), p.get("server"), p.get("port"), sni)
}

/// 映射里指定的安装节点（install_node）：各个 Team 的配置里第一个写了的，都没写时是维护者的映射里的；没有、不合规都是空的。
/// 旧版本的 cca 不认识这一项，照样忽略它
pub fn policy_install_node() -> String {
    let teams = super::team::grants();
    let all = teams.iter().filter_map(|g| g.doc.get("install_node")).chain(policy_doc().and_then(|j| j.get("install_node")));
    for n in all {
        if let Some(s) = n.as_str().filter(|s| !s.is_empty() && fill_name_ok(&n) && !FILL_BUILTIN.contains(s)) {
            return s.to_string();
        }
    }
    String::new()
}

/// 参考节点怎么补进一份配置：have 是配置里的节点 [(名字, 摘要)]，names 是节点和策略组的全部名字，f 是参考节点
/// （`policy_fill()` 的结果）。返回 (要加的节点定义, 要补进策略组的名字, 策略组名)。同名的节点是用户自己的（只补进组），
/// 名字被策略组占了的不补，用户已经有同一个节点（摘要一样）只是叫法不同的用他的名字，要经别的节点连接
/// （dialer-proxy）而那个节点没有的不补
pub fn fill_plan(have: &[(String, String)], names: &[String], f: Option<&Dict>) -> (Vec<Dict>, Vec<String>, String) {
    let Some(f) = f else {
        return (Vec::new(), Vec::new(), String::new());
    };
    let proxies: Vec<Dict> = f.list("proxies").unwrap_or_default().items().iter().filter_map(|p| p.as_dict()).collect();
    let name_of = |p: &Dict| p.at("name").py_str();
    let px: HashSet<&str> = have.iter().map(|(n, _)| n.as_str()).collect();
    // 摘要 → 用户配置里第一个这个摘要的节点名
    let by = |g: &str| have.iter().find(|(_, x)| !x.is_empty() && x == g).map(|(n, _)| n.clone());
    let taken: HashSet<&str> =
        names.iter().map(|s| s.as_str()).chain(px.iter().copied()).chain(FILL_BUILTIN.iter().copied()).collect();
    let ok: HashSet<String> =
        proxies.iter().filter(|p| !taken.contains(name_of(p).as_str()) && by(&fill_fp(p)).is_none()).map(name_of).collect();
    let (mut add, mut members) = (Vec::new(), Vec::new());
    for p in &proxies {
        let (n, g) = (name_of(p), fill_fp(p));
        let dp = p.get("dialer-proxy").filter(|v| v.truthy());
        if px.contains(n.as_str()) {
            members.push(n);
        } else if taken.contains(n.as_str()) {
            continue;
        } else if let Some(theirs) = by(&g).filter(|_| !g.is_empty()) {
            members.push(theirs);
        } else if dp.as_ref().is_some_and(|d| !d.as_str().is_some_and(|s| taken.contains(s) || ok.contains(s))) {
            continue;
        } else {
            add.push(p.clone());
            members.push(n);
        }
    }
    let mut uniq: Vec<String> = Vec::new(); // list(dict.fromkeys(members))：去重，保持先后
    for m in members {
        if !uniq.contains(&m) {
            uniq.push(m);
        }
    }
    (add, uniq, f.at("group").py_str())
}

/// 可选节点里有没有这个名字
pub fn in_nodes(nodes: &Option<List>, n: &str) -> bool {
    nodes.as_ref().is_some_and(|l| l.has_str(n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dict, list};

    #[test]
    fn node_fingerprint() {
        // 右边是 Python 的 node_fp()
        assert_eq!(
            node_fp(Some("VLESS".into()), Some(" Example.COM. ".into()), Some(443.into()), Some("SNI.example.com.".into())),
            "8f6083da5882fd49"
        );
        assert_eq!(node_fp(Some("ss".into()), Some("192.0.2.10".into()), Some("8388".into()), None), "60a7807f1456989d");
        assert_eq!(node_fp(Some("ss".into()), None, Some(1.into()), None), "");
    }

    #[test]
    fn alias_rules() {
        assert!(alias_ok(&"工作".into()) && !alias_ok(&"".into()) && !alias_ok(&"a b".into()) && !alias_ok(&V::Int(1)));
        assert!(alias_ok(&"一二三四五六七八九十一二".into()) && !alias_ok(&"一二三四五六七八九十一二三".into()));
        let table = dict! { "主号(a@example.com)" => "US-A" };
        assert_eq!(alias_problem(&table, "主号"), "别名「主号」已经用在 主号（a@example.com） 上了");
        assert_eq!(alias_problem(&table, "a@b"), "别名里不能有空格、引号、括号、反斜杠或 @");
        assert_eq!(alias_problem(&table, "别的"), "");
        assert_eq!(
            node_alias(Some(&dict! { "nodes" => list!["新"], "node_renames" => dict! { "旧" => "中", "中" => "新" } }), "旧"),
            "新"
        );
        assert_eq!(node_alias(Some(&dict! { "nodes" => list!["旧"], "node_renames" => dict! { "旧" => "新" } }), "旧"), "旧");
        assert_eq!(node_alias(Some(&dict! { "node_renames" => dict! { "a" => "b", "b" => "a" } }), "a"), "a");
        assert_eq!(node_alias(None, "x"), "x");
    }

    #[test]
    fn problems() {
        assert_eq!(policy_problems(&V::Int(1), false).len(), 1);
        let j = V::Dict(
            dict! { "version" => 1, "claude" => dict! { "A@example.com" => "US-A", "a@example.com" => "", "bad" => 1 }, "codex" => list![] },
        );
        assert_eq!(
            policy_problems(&j, false),
            [
                "claude：a@example.com 写了不止一次（邮箱不分大小写）",
                "claude：a@example.com 的节点应该是非空字符串",
                "claude：「bad」不像邮箱",
                "claude：bad 的节点应该是非空字符串",
                "codex 应该是 {\"邮箱\": \"节点\"} 形式的对象",
            ]
        );
        let f = V::Dict(dict! { "version" => 2, "group" => "CCA-x", "proxies" => list![
            dict! { "name" => "DIRECT", "type" => "ss", "server" => "s.example.com", "port" => "443" },
            dict! { "name" => "n1", "type" => "b a d", "server" => "", "port" => 0 },
            dict! { "name" => "n1", "type" => "ss", "server" => "s", "port" => true },
            1,
        ] });
        assert_eq!(
            fill_problems(&f),
            [
                "version 应该是 1",
                "group 应该是策略组的名字（不能是空的，不能有控制字符，不能以 CCA- 开头）",
                "第 1 个节点「DIRECT」：name 不能是空的，首尾不能有空格，不能有控制字符，不能以 CCA- 开头，也不能是 DIRECT、REJECT 这些内置的名字",
                "第 2 个节点「n1」：type 不对",
                "第 2 个节点「n1」：server 不对",
                "第 2 个节点「n1」：port 应该是 1–65535",
                "第 3 个节点「n1」：名字重复了",
                "第 3 个节点「n1」：port 应该是 1–65535",
                "第 4 个节点应该是对象",
            ]
        );
    }

    #[test]
    fn fill_planning() {
        let f = dict! { "group" => "G", "proxies" => list![
            dict! { "name" => "mine", "type" => "ss", "server" => "a.example.com", "port" => 1 },
            dict! { "name" => "G2", "type" => "ss", "server" => "b.example.com", "port" => 2 },
            dict! { "name" => "same", "type" => "ss", "server" => "c.example.com", "port" => 3 },
            dict! { "name" => "via-missing", "type" => "ss", "server" => "d.example.com", "port" => 4, "dialer-proxy" => "nowhere" },
            dict! { "name" => "via-new", "type" => "ss", "server" => "e.example.com", "port" => 5, "dialer-proxy" => "new" },
            dict! { "name" => "new", "type" => "ss", "server" => "f.example.com", "port" => 6 },
        ] };
        let fp_c = node_fp(Some("ss".into()), Some("c.example.com".into()), Some(3.into()), None);
        let have = vec![("mine".to_string(), String::new()), ("theirs".to_string(), fp_c)];
        let names = vec!["mine".to_string(), "theirs".to_string(), "G2".to_string()];
        let (add, members, group) = fill_plan(&have, &names, Some(&f));
        assert_eq!(add.iter().map(|p| p.at("name").py_str()).collect::<Vec<_>>(), ["via-new", "new"]);
        assert_eq!(members, ["mine", "theirs", "via-new", "new"]);
        assert_eq!(group, "G");
        assert_eq!(fill_plan(&have, &names, None).2, "");
    }
}
