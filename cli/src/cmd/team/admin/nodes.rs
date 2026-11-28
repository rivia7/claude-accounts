//! Team 的节点（`cca team node`）：管理员给这个 Team 的配置加、复制、去掉节点的定义，列出来，起临时内核实测出口，指定各台设备
//! 安装客户端用哪个（install_node）。节点的定义存在
//! admin.json 的 config.nodes（`{"group": 策略组, "proxies": [节点定义…]}`，和以前维护者映射仓库的 nodes.json 一个样子）；给各台
//! 设备的配置只带它的账号用得着的那几个（needed_nodes），所以加了没人用的节点，哪台设备的配置都不变。
//! 节点的密钥不出这台设备：列出来、实测出口都不显示 uuid、密码这些。见 docs/dev/team.md「Team 的节点」

use super::{admin_of, admin_save, fail, settle};
use crate::cmd::arity;
use crate::cmd::corenet::{config_write_new, test_proxy_config};
use crate::core::policy::fill_problems;
use crate::core::team::Team;
use crate::core::tool::tools;
use crate::dict;
use crate::out;
use crate::py::io::read_stdin_text;
use crate::py::obj::{str_list, Dict, List, V};
use crate::py::{exit, json, Fail, R};

/// 还没有节点的 Team 加第一个节点时用的策略组（各台设备把节点写进路由器的配置时，名字加进这个组）：和以前维护者映射默认的一样
const DEFAULT_GROUP: &str = "OpenAi";

/// 看服务器自己的出口用的网站（经它看到的是节点服务器自己的 IP；Antigravity 测出口也用它）
const DIRECT_HOST: &str = "www.cloudflare.com";

/// 这个 Team 的节点定义（config.nodes.proxies；没有节点是空的）
fn proxies(a: &Dict) -> Vec<Dict> {
    a.dict("config")
        .and_then(|c| c.dict("nodes"))
        .and_then(|n| n.list("proxies"))
        .map(|l| l.items().iter().filter_map(|p| p.as_dict()).collect())
        .unwrap_or_default()
}

/// 节点定义里的一项（没有是 null：`at` 是 Python 的 `d[k]`，没有就是 KeyError）
fn field(p: &Dict, k: &str) -> V {
    p.get(k).unwrap_or(V::None)
}

fn named<'a>(px: &'a [Dict], name: &str) -> Option<&'a Dict> {
    px.iter().find(|p| p.str("name").as_deref() == Some(name))
}

/// 换掉这个 Team 的节点定义（空了就是没有节点：null）。先照成员设备查配置的那一套（fill_problems）查一遍：不合格的不存，免得发出去
/// 各台设备整份不用
pub(super) fn set_proxies(a: &Dict, px: Vec<Dict>) -> Result<(), String> {
    let config = a.dict("config").unwrap_or_default();
    let group = config.dict("nodes").and_then(|n| n.str("group")).map_or(DEFAULT_GROUP.to_string(), |g| g.to_string());
    if px.is_empty() {
        config.set("nodes", V::None);
    } else {
        let list = List::from_vec(px.into_iter().map(V::Dict).collect());
        let check = V::Dict(dict! { "version" => 1, "group" => group.as_str(), "proxies" => list.clone() });
        if let Some(why) = fill_problems(&check).into_iter().next() {
            return Err(why);
        }
        config.set("nodes", dict! { "group" => group.as_str(), "proxies" => list });
    }
    a.set("config", config);
    Ok(())
}

/// 这个 Team 里走这个节点的账号：工具:邮箱
fn users(a: &Dict, name: &str) -> Vec<String> {
    let accounts = a.dict("config").and_then(|c| c.dict("accounts")).unwrap_or_default();
    let mut out = Vec::new();
    for tool in tools().iter() {
        for (e, n) in accounts.dict(tool.name).unwrap_or_default().items() {
            if n.as_str() == Some(name) {
                out.push(format!("{}:{e}", tool.name));
            }
        }
    }
    out
}

fn install_node(a: &Dict) -> String {
    a.dict("config").and_then(|c| c.str("install_node")).map_or(String::new(), |n| n.to_string())
}

/// 发布（接上了服务器才发），返回存上了几台
fn publish_now(t: &Team, a: &Dict) -> R<usize> {
    if t.attached() {
        settle(t, a, None)
    } else {
        Ok(0)
    }
}

/// py team-node-ls Team [--json]：这个 Team 的节点定义（不带密钥）。一行一个：名字\t类型\t服务器\t端口\t经哪个节点连出去（dialer-proxy，
/// 没有是 -）\t走它的账号（工具:邮箱，用「,」连起来，没有是 -）\t安装客户端用它（1 | 0）。--json：{"version": 1, "kind": "team_nodes",
/// "team": 编号, "name": Team 的名字, "group": 策略组（没有节点是 null）, "install_node", "accounts": {工具: {邮箱: 节点}}（这个 Team
/// 的全部账号，走的节点不在这里的也算）, "nodes": [{name, type, server, port, dialer, accounts: [工具:邮箱…], install}]}。
/// 原因码：no_team、not_admin
pub fn cmd_team_node_ls(args: &[String]) -> R<()> {
    let (sel, as_json) = match args {
        [s] => (s, false),
        [s, f] if f == "--json" => (s, true),
        _ => return Err(Fail::Die("用法: py team-node-ls Team [--json]".into())),
    };
    let (t, a, _lk) = admin_of(sel)?;
    let install = install_node(&a);
    let rows: Vec<Dict> = proxies(&a)
        .iter()
        .map(|p| {
            let name = p.str("name").map_or(String::new(), |n| n.to_string());
            dict! {
                "name" => name.as_str(), "type" => field(p, "type"), "server" => field(p, "server"), "port" => field(p, "port"),
                "dialer" => field(p, "dialer-proxy"), "accounts" => str_list(users(&a, &name)), "install" => !install.is_empty() && name == install,
            }
        })
        .collect();
    if as_json {
        let config = a.dict("config").unwrap_or_default();
        let doc = dict! {
            "version" => 1, "kind" => "team_nodes", "team" => t.id.as_str(), "name" => t.name.as_str(),
            "group" => config.dict("nodes").map_or(V::None, |n| field(&n, "group")), "install_node" => install.as_str(),
            "accounts" => config.dict("accounts").unwrap_or_default(), "nodes" => List::from_vec(rows.into_iter().map(V::Dict).collect()),
        };
        out!("{}", json::dumps(&V::Dict(doc), json::Fmt::new().utf8().indent(2)));
        return Ok(());
    }
    for r in rows {
        // 没有的写 -：bash 按制表符读，空的栏会被并掉
        let s = |k: &str| r.get(k).filter(|v| !v.is_none() && !v.py_str().is_empty()).map_or("-".to_string(), |v| v.py_str());
        let using = r.list("accounts").map(|l| l.strs().join(",")).filter(|u| !u.is_empty()).unwrap_or_else(|| "-".into());
        let install = if r.truthy("install") { 1 } else { 0 };
        out!("{}\t{}\t{}\t{}\t{}\t{using}\t{install}", s("name"), s("type"), s("server"), s("port"), s("dialer"));
    }
    Ok(())
}

/// py team-node-add Team：stdin 是一个节点的 JSON（bash 先 node-check 解析、起好不冲突的名字，再拿内核 -t 校验过），加进这个 Team 的
/// 节点，再发布（还没有账号走它，各台设备的配置不变）。stdout：节点的名字、存上了几台的配置。原因码：no_team、not_admin、name（这个
/// Team 里已经有这个名字）、node（不是节点定义、格式不对）、server、publish
pub fn cmd_team_node_add(args: &[String]) -> R<()> {
    arity("team-node-add", args, 1, 1)?;
    let p = match json::loads(&read_stdin_text()) {
        Ok(V::Dict(p)) => p,
        _ => return fail("node", "节点应该是一个 JSON 对象"),
    };
    let Some(name) = p.str("name").map(|n| n.to_string()) else { return fail("node", "节点没有名字") };
    let (t, a, _lk) = admin_of(&args[0])?;
    let mut px = proxies(&a);
    if named(&px, &name).is_some() {
        return fail(
            "name",
            &format!("这个 Team 里已经有节点「{name}」了：换一个名字，或者先去掉原来的（cca team node {} rm {name}）", t.name),
        );
    }
    px.push(p);
    if let Err(why) = set_proxies(&a, px) {
        return fail("node", &why);
    }
    admin_save(&t.id, &a)?;
    let stored = publish_now(&t, &a)?;
    out!("{name}\n{stored}");
    Ok(())
}

/// py team-node-copy Team 模板 新名字 [--port 端口]：照这个 Team 里的一个节点复制一个（定义一样，名字换成新的，给了端口换端口），再发布
/// （还没有账号走它，各台设备的配置不变）。一台服务器上按端口分开的几个入站（各有各的出口）各起一个节点时用：节点的密钥不出这台设备。
/// stdout：新节点的名字、存上了几台的配置。原因码：no_team、not_admin、usage、no_node（没有这个模板）、name（新名字已经有了）、node（新名字
/// 不合规）、server、publish
pub fn cmd_team_node_copy(args: &[String]) -> R<()> {
    const USAGE: &str = "用法: cca team node Team copy 模板 新名字 [--port 端口]";
    let (sel, from, name, port) = match args {
        [s, f, n] => (s, f, n.trim().to_string(), None),
        [s, f, n, flag, p] if flag == "--port" => match p.trim().parse::<i64>() {
            Ok(x) if (1..=65535).contains(&x) => (s, f, n.trim().to_string(), Some(x)),
            _ => return fail("usage", "--port 后面是 1–65535 的端口"),
        },
        _ => return fail("usage", USAGE),
    };
    let (t, a, _lk) = admin_of(sel)?;
    let mut px = proxies(&a);
    let Some(tpl) = named(&px, from) else { return fail("no_node", &format!("这个 Team 里没有节点「{from}」")) };
    if named(&px, &name).is_some() {
        return fail("name", &format!("这个 Team 里已经有节点「{name}」了"));
    }
    // 整份复制：里面的 reality-opts 这些也是新的一份，不和模板共用
    let Some(new) = V::Dict(tpl.clone()).deepcopy().as_dict() else { return Err(Fail::Die("复制节点的定义失败".into())) };
    new.set("name", name.as_str());
    if let Some(p) = port {
        new.set("port", p);
    }
    px.push(new);
    if let Err(why) = set_proxies(&a, px) {
        return fail("node", &why);
    }
    admin_save(&t.id, &a)?;
    let stored = publish_now(&t, &a)?;
    out!("{name}\n{stored}");
    Ok(())
}

/// py team-node-rm Team 名字：从这个 Team 的节点里去掉一个，再发布。还有账号走它、别的节点经它连出去（dialer-proxy）、安装客户端用它
/// 时不去掉。stdout：存上了几台的配置。原因码：no_team、not_admin、no_node、node_used（原因里说是谁在用）、server、publish
pub fn cmd_team_node_rm(args: &[String]) -> R<()> {
    arity("team-node-rm", args, 2, 2)?;
    let name = args[1].trim();
    let (t, a, _lk) = admin_of(&args[0])?;
    let px = proxies(&a);
    if named(&px, name).is_none() {
        return fail("no_node", &format!("这个 Team 里没有节点「{name}」"));
    }
    let using = users(&a, name);
    if !using.is_empty() {
        return fail(
            "node_used",
            &format!("还有账号走「{name}」（{}）：先给它们换个节点（cca team map {} 工具 邮箱 节点）", using.join("、"), t.name),
        );
    }
    if let Some(d) = px.iter().find(|p| p.str("dialer-proxy").as_deref() == Some(name)) {
        return fail("node_used", &format!("节点「{}」经「{name}」连出去（dialer-proxy）：先去掉它", field(d, "name").py_str()));
    }
    if install_node(&a) == name {
        return fail("node_used", &format!("各台设备经内置代理安装客户端用的是「{name}」（install_node）"));
    }
    let rest: Vec<Dict> = px.into_iter().filter(|p| p.str("name").as_deref() != Some(name)).collect();
    if let Err(why) = set_proxies(&a, rest) {
        return fail("node", &why);
    }
    admin_save(&t.id, &a)?;
    let stored = publish_now(&t, &a)?;
    out!("{stored}");
    Ok(())
}

/// py team-node-install Team 节点|-：指定各台设备直连拿不到安装脚本时经内置代理的哪个节点装客户端（config.install_node；- 是不再指定），
/// 再发布。只能是这个 Team 的节点；每台设备的配置都带着它（连同它的定义：needed_nodes），所以换了一般每台都发一版新的。
/// stdout：原来的（没有是空行）、存上了几台的配置。原因码：no_team、not_admin、usage、no_node、server、publish
pub fn cmd_team_node_install(args: &[String]) -> R<()> {
    arity("team-node-install", args, 2, 2)?;
    let name = args[1].trim();
    if name.is_empty() {
        return fail("usage", "用法: cca team node Team install 节点|-（- 是不再指定）");
    }
    let (t, a, _lk) = admin_of(&args[0])?;
    if name != "-" && named(&proxies(&a), name).is_none() {
        return fail("no_node", &format!("这个 Team 里没有节点「{name}」（cca team node {} 列出有哪些）", t.name));
    }
    let was = install_node(&a);
    let config = a.dict("config").unwrap_or_default();
    config.set("install_node", if name == "-" { "" } else { name });
    a.set("config", config);
    admin_save(&t.id, &a)?;
    let stored = publish_now(&t, &a)?;
    out!("{was}\n{stored}");
    Ok(())
}

/// py team-node-testconfig Team 文件 [节点…] [--tool 工具]：实测出口（cca team node Team test）用的临时内核的配置，写进文件（600，新建；
/// corenet 的 test_proxy_config：127.0.0.1 上每个节点一个入站，不开控制接口、通用的代理端口、TUN、DNS）。没写节点是这个 Team 的全部节点。
/// stdout 第一行是要测的网站（空格隔开：账号要用的服务，最后是 www.cloudflare.com，看服务器自己的出口），后面一个节点一行：
/// 名字\t本机端口\t服务器\t端口\t工具（走它的账号是哪几种工具的，逗号隔开，--tool 给了就是那一个；没有账号走它时是空的）。要用的服务：--tool 给了就是那个工具的；没给时看这个 Team 里哪些工具的账号走这几个节点，都没有时是
/// Claude 的。原因码：no_team、not_admin、usage、no_node
pub fn cmd_team_node_testconfig(args: &[String]) -> R<()> {
    const USAGE: &str = "用法: cca team node Team test [节点…] [--tool claude|codex|agy] [--json]";
    let (mut rest, mut only): (Vec<&str>, Option<&str>) = (Vec::new(), None);
    let mut it = args.iter();
    while let Some(x) = it.next() {
        if x == "--tool" {
            match it.next() {
                Some(v) if tools().iter().any(|tool| tool.name == v.as_str()) => only = Some(v.as_str()),
                _ => return fail("usage", USAGE),
            }
        } else {
            rest.push(x.as_str());
        }
    }
    let [sel, file, names @ ..] = rest.as_slice() else { return fail("usage", USAGE) };
    let (_t, a, _lk) = admin_of(sel)?;
    let px = proxies(&a);
    let mut want: Vec<String> = Vec::new();
    for n in names.iter().map(|n| n.trim()) {
        if !n.is_empty() && !want.iter().any(|w| w == n) {
            want.push(n.to_string());
        }
    }
    if want.is_empty() {
        want = px.iter().filter_map(|p| p.str("name").map(|n| n.to_string())).collect();
        if want.is_empty() {
            return fail("no_node", "这个 Team 还没有节点的定义");
        }
    }
    if let Some(n) = want.iter().find(|n| named(&px, n).is_none()) {
        return fail("no_node", &format!("这个 Team 里没有节点「{n}」"));
    }
    let mut hosts: Vec<&str> = Vec::new();
    for tool in tools().iter() {
        let used = match only {
            Some(o) => o == tool.name,
            None => want.iter().any(|n| users(&a, n).iter().any(|u| u.split(':').next() == Some(tool.name))),
        };
        if used && !hosts.contains(&tool.trace_host) {
            hosts.push(tool.trace_host);
        }
    }
    if hosts.is_empty() {
        hosts.push(tools()[0].trace_host);
    }
    hosts.retain(|h| *h != DIRECT_HOST);
    hosts.push(DIRECT_HOST);
    // 系统给的空闲端口（临时端口段，碰不到端口池的 79xx）：全都拿到以后再一起放开，免得两个入站拿到同一个；放开后到内核绑定之间
    // 被别人占了，内核起不来，bash 照实说
    let mut held = Vec::new();
    let mut ports: Vec<(String, u16)> = Vec::new();
    for n in &want {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        ports.push((n.clone(), l.local_addr()?.port()));
        held.push(l);
    }
    drop(held);
    config_write_new(file, &test_proxy_config(&px, &ports))?;
    out!("{}", hosts.join(" "));
    for (n, port) in &ports {
        let p = named(&px, n).expect("刚核对过有");
        let mine: Vec<&str> = match only {
            Some(o) => vec![o],
            None => {
                let u = users(&a, n);
                tools()
                    .iter()
                    .filter(|tool| u.iter().any(|x| x.split(':').next() == Some(tool.name)))
                    .map(|tool| tool.name)
                    .collect()
            }
        };
        out!("{n}\t{port}\t{}\t{}\t{}", field(p, "server").py_str(), field(p, "port").py_str(), mine.join(","));
    }
    Ok(())
}

/// 经一个节点取一个网站的 /cdn-cgi/trace 的结果
#[derive(Clone, Debug, PartialEq)]
enum Trace {
    /// 取到了：出口 IP、地区代码、Cloudflare 的机房
    Exit { ip: String, loc: String, colo: String },
    /// 没取到：curl 说的
    Error(String),
}

impl Trace {
    fn ip(&self) -> Option<&str> {
        match self {
            Trace::Exit { ip, .. } => Some(ip),
            Trace::Error(_) => None,
        }
    }
}

/// 一个节点测出来的样子
#[derive(Debug, PartialEq)]
struct Verdict {
    name: String,
    /// ok | down | flag
    state: &'static str,
    /// 给人看的一句：各个网站看到的出口，或者不通的原因
    line: String,
    /// 要提醒的（flag 的原因）
    notes: Vec<String>,
}

/// h 看到的出口地区 loc 是用 h 测出口的那几种工具（trace_host 是 h 的；www.cloudflare.com 是 agy 的）不能用的地区时：地区名和
/// 这几种工具的叫法（照各自的名单 `restricted`）
fn restricted_at(h: &str, loc: &str) -> Option<(&'static str, String)> {
    let mut region = None;
    let mut who: Vec<&str> = Vec::new();
    for t in tools().iter().filter(|t| t.trace_host == h) {
        if let Some((_, name)) = t.restricted.iter().find(|(cc, _)| cc.eq_ignore_ascii_case(loc)) {
            region.get_or_insert(*name);
            who.push(t.label);
        }
    }
    region.map(|r| (r, who.join("、")))
}

/// 照实测的结果判断每个节点：哪个网站都没取到是不通（tcp：直连它的服务器端口连不连得上，分清是没开入站还是入站的参数不对）；
/// 取到了的再看要用的服务：经节点出得去、这个服务却不通，出口在受限地区，和别的节点看到同一个 IP（走这几个节点的账号在那边是同一个
/// 出口），别的节点在这个服务那边有自己的出口（看到的 IP 和服务器自己的不一样）、它没有。hosts 最后一个是 www.cloudflare.com（服务器
/// 自己的出口）；uses 是每个节点上是哪几种工具的账号（testconfig 给的）：有别的服务时服务器自己的出口只拿来比 IP，走这个节点的账号里
/// 有 agy 的（它的出口就是服务器自己的）才再照 agy 的名单看地区。返回每个节点的判断和退出码：0 都好；1 有节点不通；2 全都不通（多半是
/// 这台机器的网络）；3 都通了、有要提醒的
fn judge(
    hosts: &[String],
    order: &[String],
    traces: &[(String, String, Trace)],
    tcp: &[(String, bool)],
    uses: &[(String, Vec<String>)],
) -> (Vec<Verdict>, i32) {
    let get = |n: &str, h: &str| traces.iter().find(|(x, y, _)| x == n && y == h).map(|(_, _, r)| r.clone());
    let direct = hosts.last().map_or(DIRECT_HOST, |h| h.as_str());
    let services: Vec<&str> = hosts.iter().map(|h| h.as_str()).filter(|h| *h != direct).collect();
    let up = |n: &str| hosts.iter().any(|h| get(n, h).is_some_and(|r| r.ip().is_some()));
    let ip_of = |n: &str, h: &str| get(n, h).and_then(|r| r.ip().map(|s| s.to_string()));
    // 别的节点在这个服务那边有没有自己的出口
    let dedicated = |me: &str, h: &str| {
        order.iter().any(|o| o != me && up(o) && ip_of(o, h).is_some_and(|x| ip_of(o, direct).is_some_and(|d| d != x)))
    };
    let mut out = Vec::new();
    for n in order {
        if !up(n) {
            let why = hosts.iter().find_map(|h| match get(n, h) {
                Some(Trace::Error(e)) => Some(e),
                _ => None,
            });
            let line = match tcp.iter().find(|(x, _)| x == n).map(|(_, open)| *open) {
                Some(false) => "不通：直连它的服务器端口也连不上（服务器上还没开这个入站，或者被防火墙挡住）".to_string(),
                Some(true) => {
                    "不通：它的服务器端口连得上，经它却出不去（入站的参数和这个节点的定义对不上，或者入站还没配好）".to_string()
                }
                None => format!("不通（{}）", why.unwrap_or_else(|| "没取到出口".into())),
            };
            out.push(Verdict { name: n.clone(), state: "down", line, notes: Vec::new() });
            continue;
        }
        let mut parts = Vec::new();
        let mut notes = Vec::new();
        for h in hosts {
            let label =
                if h == direct && !services.is_empty() { "服务器自己的出口".to_string() } else { format!("{h} 看到") };
            match get(n, h) {
                Some(Trace::Exit { ip, loc, .. }) => parts.push(format!("{label} {ip}（{loc}）")),
                Some(Trace::Error(e)) => parts.push(format!("{h} 不通（{e}）")),
                None => parts.push(format!("{h} 没测")),
            }
        }
        // 走这个节点、出口就是服务器自己的出口的那几种工具的账号（agy）：有别的服务时也照它们的名单看服务器自己的出口
        let mine = uses.iter().find(|(x, _)| x == n).map_or(&[][..], |(_, ts)| ts.as_slice());
        let on_direct: Vec<&str> =
            tools().iter().filter(|t| t.trace_host == direct && mine.iter().any(|x| x == t.name)).map(|t| t.label).collect();
        let mut checked: Vec<&str> = if services.is_empty() { vec![direct] } else { services.clone() };
        if !services.is_empty() && !on_direct.is_empty() {
            checked.push(direct);
        }
        for h in checked {
            let what = if h == direct && !services.is_empty() {
                "服务器自己的出口".to_string()
            } else {
                format!("{h} 看到的出口")
            };
            match get(n, h) {
                Some(Trace::Exit { ip, loc, .. }) => {
                    if let Some((region, who)) = restricted_at(h, &loc) {
                        notes.push(format!("{what}在{region}（{loc}）：{who} 在那边用不了"));
                    }
                    if h != direct {
                        let same: Vec<&str> = order
                            .iter()
                            .filter(|o| *o != n && ip_of(o, h).as_deref() == Some(ip.as_str()))
                            .map(|o| o.as_str())
                            .collect();
                        if !same.is_empty() {
                            notes.push(format!(
                                "{h} 看到的出口 {ip} 和 {} 一样：走这几个节点的账号在那边是同一个 IP",
                                same.join("、")
                            ));
                        }
                        if dedicated(n, h) && ip_of(n, direct).as_deref() == Some(ip.as_str()) {
                            notes.push(format!(
                                "{h} 看到的是服务器自己的 IP {ip}，别的节点在那边有自己的出口：服务器上还没给这个入站配 {h} 的出口"
                            ));
                        }
                    }
                }
                _ if h == direct => {
                    notes.push(format!("经它出得去，{what}（{h}）却不通：走它的 {} 账号用的就是这个出口", on_direct.join("、")))
                }
                _ => notes.push(format!("经它出得去，{h} 却不通：服务器给它的 {h} 出口有问题")),
            }
        }
        let state = if notes.is_empty() { "ok" } else { "flag" };
        out.push(Verdict { name: n.clone(), state, line: parts.join("；"), notes });
    }
    let down = out.iter().filter(|v| v.state == "down").count();
    let code = if down == out.len() {
        2
    } else if down > 0 {
        1
    } else if out.iter().any(|v| v.state == "flag") {
        3
    } else {
        0
    };
    (out, code)
}

/// py team-node-verdict [--json]：照实测的结果（stdin，bash 经临时内核取的）说每个节点怎么样（judge）。stdin 第一行是测过的网站（同
/// testconfig 的第一行），后面每行一项：名字\t网站\tip\t地区\t机房（取到了）、名字\t网站\t-\t出错的话（没取到）、名字\ttcp\topen|closed
/// （哪个网站都没取到时 bash 直连它的服务器端口）、名字\ttools\t工具,…（testconfig 给的最后一列：走它的账号是哪几种工具的，没有时不给）。stdout 一个节点一行：ok|down|flag\t名字\t说明，它的每条提醒接着一行 !\t名字\t提醒。
/// --json：{"version": 1, "kind": "team_node_test", "hosts": [网站…], "nodes": [{name, state, exits: {网站: {ip, loc, colo} | {error}},
/// tcp: open | closed | null, notes: [提醒…]}]}。退出码同 judge
pub fn cmd_team_node_verdict(args: &[String]) -> R<()> {
    let as_json = match args {
        [] => false,
        [f] if f == "--json" => true,
        _ => return Err(Fail::Die("用法: py team-node-verdict [--json]".into())),
    };
    let text = read_stdin_text();
    let mut lines = text.lines();
    let hosts: Vec<String> = lines.next().unwrap_or("").split_whitespace().map(String::from).collect();
    let (mut order, mut traces, mut tcp): (Vec<String>, Vec<(String, String, Trace)>, Vec<(String, bool)>) =
        (Vec::new(), Vec::new(), Vec::new());
    let mut uses: Vec<(String, Vec<String>)> = Vec::new();
    for l in lines {
        let f: Vec<&str> = l.split('\t').collect();
        if f.len() < 3 || f[0].is_empty() {
            continue;
        }
        let n = f[0].to_string();
        if !order.contains(&n) {
            order.push(n.clone());
        }
        let at = |i: usize| f.get(i).map_or(String::new(), |s| s.to_string());
        if f[1] == "tcp" {
            tcp.push((n, f[2] == "open"));
        } else if f[1] == "tools" {
            uses.push((n, f[2].split(',').filter(|x| !x.is_empty()).map(String::from).collect()));
        } else if f[2] == "-" {
            traces.push((n, at(1), Trace::Error(f.get(3).map_or("没取到", |e| e).to_string())));
        } else {
            traces.push((n, at(1), Trace::Exit { ip: at(2), loc: at(3), colo: at(4) }));
        }
    }
    let (verdicts, code) = judge(&hosts, &order, &traces, &tcp, &uses);
    if as_json {
        let nodes: Vec<V> = verdicts
            .iter()
            .map(|v| {
                let exits = Dict::new();
                for (_, h, r) in traces.iter().filter(|(n, _, _)| *n == v.name) {
                    let e = match r {
                        Trace::Exit { ip, loc, colo } => {
                            dict! { "ip" => ip.as_str(), "loc" => loc.as_str(), "colo" => colo.as_str() }
                        }
                        Trace::Error(e) => dict! { "error" => e.as_str() },
                    };
                    exits.set(h.as_str(), e);
                }
                let probe = tcp
                    .iter()
                    .find(|(n, _)| *n == v.name)
                    .map_or(V::None, |(_, open)| V::from(if *open { "open" } else { "closed" }));
                V::Dict(dict! {
                    "name" => v.name.as_str(), "state" => v.state, "exits" => exits, "tcp" => probe, "notes" => str_list(&v.notes),
                })
            })
            .collect();
        let doc =
            dict! { "version" => 1, "kind" => "team_node_test", "hosts" => str_list(&hosts), "nodes" => List::from_vec(nodes) };
        out!("{}", json::dumps(&V::Dict(doc), json::Fmt::new().utf8().indent(2)));
    } else {
        for v in &verdicts {
            out!("{}\t{}\t{}", v.state, v.name, v.line);
            for note in &v.notes {
                out!("!\t{}\t{note}", v.name);
            }
        }
    }
    exit(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exit_at(ip: &str, loc: &str) -> Trace {
        Trace::Exit { ip: ip.into(), loc: loc.into(), colo: "SJC".into() }
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn tr(n: &str, h: &str, t: Trace) -> (String, String, Trace) {
        (n.to_string(), h.to_string(), t)
    }

    /// 一台服务器上的几个入站：各有各的 claude.ai 出口、服务器自己的出口是同一个
    #[test]
    fn judge_dedicated_exits() {
        let hosts = s(&["claude.ai", "www.cloudflare.com"]);
        let order = s(&["A", "B", "C"]);
        let traces = vec![
            tr("A", "claude.ai", exit_at("203.0.113.1", "US")),
            tr("A", "www.cloudflare.com", exit_at("198.51.100.1", "US")),
            tr("B", "claude.ai", exit_at("203.0.113.2", "US")),
            tr("B", "www.cloudflare.com", exit_at("198.51.100.1", "US")),
        ];
        // C 不通，直连它的端口连不上
        let (v, code) = judge(&hosts, &order, &traces, &[("C".into(), false)], &[]);
        assert_eq!(code, 1);
        assert_eq!((v[0].state, v[1].state, v[2].state), ("ok", "ok", "down"));
        assert_eq!(v[0].line, "claude.ai 看到 203.0.113.1（US）；服务器自己的出口 198.51.100.1（US）");
        assert!(v[2].line.contains("连不上"));
        // 端口连得上、经它出不去
        let (v, _) = judge(&hosts, &order, &traces, &[("C".into(), true)], &[]);
        assert!(v[2].line.contains("参数"));
        // B 的 claude.ai 出口和 A 的一样；C 看到的是服务器自己的 IP（A 有自己的出口），出口还在受限地区
        let traces = vec![
            tr("A", "claude.ai", exit_at("203.0.113.1", "US")),
            tr("A", "www.cloudflare.com", exit_at("198.51.100.1", "US")),
            tr("B", "claude.ai", exit_at("203.0.113.1", "US")),
            tr("B", "www.cloudflare.com", exit_at("198.51.100.1", "US")),
            tr("C", "claude.ai", exit_at("198.51.100.1", "cn")),
            tr("C", "www.cloudflare.com", exit_at("198.51.100.1", "cn")),
        ];
        let (v, code) = judge(&hosts, &order, &traces, &[], &[]);
        assert_eq!(code, 3);
        assert!(v[0].notes.iter().any(|n| n.contains("和 B 一样")));
        assert!(v[1].notes.iter().any(|n| n.contains("和 A 一样")));
        assert!(v[2].notes.iter().any(|n| n.contains("服务器自己的 IP 198.51.100.1")));
        assert!(v[2].notes.iter().any(|n| n.contains("中国大陆（cn）")));
        // 都不通：2；没有直连的结果时说 curl 的话
        let traces = vec![tr("A", "claude.ai", Trace::Error("超时".into()))];
        let (v, code) = judge(&hosts, &s(&["A"]), &traces, &[], &[]);
        assert_eq!(code, 2);
        assert_eq!(v[0].line, "不通（超时）");
        // 都好
        let traces =
            vec![tr("A", "claude.ai", exit_at("203.0.113.1", "US")), tr("A", "www.cloudflare.com", exit_at("198.51.100.1", "US"))];
        assert_eq!(judge(&hosts, &s(&["A"]), &traces, &[], &[]).1, 0);
        // 经节点出得去、要用的服务却不通
        let traces =
            vec![tr("A", "claude.ai", Trace::Error("被拒".into())), tr("A", "www.cloudflare.com", exit_at("198.51.100.1", "US"))];
        let (v, code) = judge(&hosts, &s(&["A"]), &traces, &[], &[]);
        assert_eq!((code, v[0].state), (3, "flag"));
        assert!(v[0].notes[0].contains("claude.ai 却不通"));
    }

    /// 只测服务器自己的出口时（Antigravity 的账号）：只看地区，不比 IP
    #[test]
    fn judge_direct_only() {
        let hosts = s(&["www.cloudflare.com"]);
        let traces = vec![
            tr("A", "www.cloudflare.com", exit_at("198.51.100.1", "US")),
            tr("B", "www.cloudflare.com", exit_at("198.51.100.1", "US")),
        ];
        let (v, code) = judge(&hosts, &s(&["A", "B"]), &traces, &[], &[]);
        assert_eq!(code, 0);
        assert_eq!(v[0].line, "www.cloudflare.com 看到 198.51.100.1（US）");
    }

    /// 地区照各工具自己的名单：claude.ai 看到的照 Claude 的，chatgpt.com 照 OpenAI 的，服务器自己的出口照 Antigravity 的（有别的服务时
    /// 只在走这个节点的账号里有 agy 的时候看）
    #[test]
    fn judge_regions_per_tool() {
        let hosts = s(&["claude.ai", "chatgpt.com", "www.cloudflare.com"]);
        let at = |cc: &str| {
            vec![
                tr("A", "claude.ai", exit_at("203.0.113.1", cc)),
                tr("A", "chatgpt.com", exit_at("203.0.113.2", cc)),
                tr("A", "www.cloudflare.com", exit_at("198.51.100.1", cc)),
            ]
        };
        let one = s(&["A"]);
        // 缅甸：Claude 不能用，OpenAI 能用
        let (v, code) = judge(&hosts, &one, &at("MM"), &[], &[]);
        assert_eq!((code, v[0].state), (3, "flag"));
        assert_eq!(v[0].notes, s(&["claude.ai 看到的出口在缅甸（MM）：Claude 在那边用不了"]));
        // 科索沃：两家都不能用，各说各的
        let (v, _) = judge(&hosts, &one, &at("XK"), &[], &[]);
        assert_eq!(
            v[0].notes,
            s(&[
                "claude.ai 看到的出口在科索沃（XK）：Claude 在那边用不了",
                "chatgpt.com 看到的出口在科索沃（XK）：Codex 在那边用不了"
            ])
        );
        // 摩尔多瓦：Claude、OpenAI 都能用，Antigravity 不能；服务器自己的出口只在走它的账号里有 agy 的时候看
        assert_eq!(judge(&hosts, &one, &at("MD"), &[], &[]).1, 0);
        assert_eq!(judge(&hosts, &one, &at("MD"), &[], &[("A".into(), s(&["claude", "codex"]))]).1, 0);
        let (v, code) = judge(&hosts, &one, &at("MD"), &[], &[("A".into(), s(&["claude", "agy"]))]);
        assert_eq!(code, 3);
        assert_eq!(v[0].notes, s(&["服务器自己的出口在摩尔多瓦（MD）：Antigravity 在那边用不了"]));
        // 服务器自己的出口不通、走它的有 agy 的账号
        let traces =
            vec![tr("A", "claude.ai", exit_at("203.0.113.1", "US")), tr("A", "www.cloudflare.com", Trace::Error("超时".into()))];
        let (v, _) = judge(&s(&["claude.ai", "www.cloudflare.com"]), &one, &traces, &[], &[("A".into(), s(&["agy"]))]);
        assert!(v[0].notes[0].contains("服务器自己的出口（www.cloudflare.com）却不通：走它的 Antigravity 账号"));
        // 只测服务器自己的出口（只有 agy 的账号）：照 Antigravity 的名单，委内瑞拉能用
        let direct = s(&["www.cloudflare.com"]);
        let (v, code) = judge(&direct, &one, &[tr("A", "www.cloudflare.com", exit_at("198.51.100.1", "MD"))], &[], &[]);
        assert_eq!(code, 3);
        assert_eq!(v[0].notes, s(&["www.cloudflare.com 看到的出口在摩尔多瓦（MD）：Antigravity 在那边用不了"]));
        assert_eq!(judge(&direct, &one, &[tr("A", "www.cloudflare.com", exit_at("198.51.100.1", "VE"))], &[], &[]).1, 0);
    }
}
