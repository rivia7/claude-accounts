//! 给界面 App 的接口（`using` 到 `cmd_check_render`、`gui_api_key` 到 `cmd_update_check`）：health-save、health-note、check-render、
//! onboard-done、gui-state、gui-live、tool-env、update-check。格式见 macos/PROTOCOL.md：只能加字段；删改要升 bash 的 GUI_API，
//! 同时改 Swift 模型（macos/Sources/CcaKit/Protocol）和回归测试（10g）。
//!
//! 还没移植的：gui-state、gui-live。

use super::leak::coffee_parse;
use super::usage::parse_ts;
use super::{arg, arity, Cmd};
use crate::core::base::sh;
use crate::core::ctx::cfg_save;
use crate::cmd::secrets::{kc_hints, kc_list, kc_slot, py_in};
use crate::core::base::{CCA_HOME, HINT_RE};
use crate::core::model::{acct_id, adir, cfg_load, is_api, key_hint, name_of, node_of, parse_key, secret_ok};
use crate::core::tool::t;
use crate::py::fs::{load_quiet, save_json};
use crate::py::io;
use crate::py::json;
use crate::py::obj::{repr_str, Dict, List, V};
use crate::py::path;
use crate::py::text;
use crate::py::time::time;
use crate::py::uni;
use crate::py::{exit, Fail, R};
use crate::{dict, out, re};

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[
    Cmd::all("health-save", cmd_health_save),
    Cmd::all("health-note", cmd_health_note),
    Cmd::all("check-render", cmd_check_render),
    Cmd::all("onboard-done", cmd_onboard_done),
    Cmd::all("tool-env", cmd_tool_env),
    Cmd::all("update-check", cmd_update_check),
];

/// 出口检测结果超过 6 小时算旧（和 DNS 泄露检测的缓存一样久）
pub const HEALTH_STALE: i64 = 6 * 3600;

/// coffee_parse 的 rc → health.json 的 result
pub fn health_result(rc: i64) -> &'static str {
    match rc {
        0 => "ok",
        1 => "low_purity",
        2 => "restricted",
        3 => "no_exit",
        4 => "no_score",
        _ => panic!("KeyError: {rc}"),
    }
}

pub fn health_file(key: &str) -> String {
    path::join(&adir(key), "health.json")
}

/// `int(s) if s.isdigit() else None`
fn num_or_none(s: &str) -> R<V> {
    if !text::isdigit(s) {
        return Ok(V::None);
    }
    match text::to_int(s) {
        Some(n) => Ok(V::Int(n)),
        None => Err(Fail::Value(format!("invalid literal for int() with base 10: {}", repr_str(s)))),
    }
}

/// 记下这个账号最近一次的出口检测（原子替换，600），并打印出来（cca check --json 收集）。字段见 macos/PROTOCOL.md 3.3。
/// fields 里已有的键（message、leak、ip……）留在原来的位置，新的（result）接在后面
pub fn health_write(key: &str, source: &str, kind: &str, route: &str, node: &str, port: &str, fields: &[(&str, V)]) -> R<()> {
    let or_none = |s: &str| if s.is_empty() { V::None } else { V::from(s) };
    let h = dict! {
        "ip" => V::None, "country" => V::None, "org" => V::None, "trust" => V::None, "trust_label" => V::None, "tags" => List::new(),
        "restricted_region" => V::None, "message" => "", "note" => "", "leak" => V::None,
        "version" => 1, "checked_at" => time(), "source" => source, "kind" => kind, "route" => or_none(route), "node" => or_none(node),
        "port" => num_or_none(port)?,
    };
    for (k, v) in fields {
        h.set(*k, v);
    }
    let h = V::Dict(h);
    match save_json(&health_file(key), &h, Some(0o600), true) {
        Err(e) if !e.is_os() => return Err(e),
        _ => {}
    }
    out!("{}", json::dumps(&h, json::LINE));
    Ok(())
}

/// _gui state 里的 health：health.json 加上 age_seconds、stale、applies（测的时候用的节点就是账号现在的节点）
pub fn health_view(key: &str, v: &V, now: f64) -> Option<Dict> {
    let h = load_quiet(&health_file(key))?.as_dict()?;
    // isinstance(at, bool) or not isinstance(at, (int, float))：布尔值不算数
    let at = h.get("checked_at").filter(crate::cmd::keyshare::is_num)?.as_num()?;
    let node = h.get("node").filter(|n| n.truthy());
    let current = node_of(v);
    let applies = match (&node, current.is_empty()) {
        (None, true) => true,
        (Some(n), false) => n.as_str() == Some(&*current),
        _ => false,
    };
    h.set("age_seconds", (now - at).max(0.0).trunc() as i64);
    h.set("stale", now - at >= HEALTH_STALE as f64);
    h.set("applies", applies);
    Some(h)
}

/// 登录状态（只读账号目录，不联网）：logged_in / logged_out / email_mismatch（登录的不是这个账号）
pub fn login_code(key: &str) -> R<&'static str> {
    let e = t().email(key)?;
    if e.is_empty() {
        return Ok("logged_out");
    }
    let want = parse_key(key).1;
    Ok(if !want.is_empty() && e.to_lowercase() != want.to_lowercase() { "email_mismatch" } else { "logged_in" })
}

pub fn short_state(key: &str) -> R<&'static str> {
    Ok(match login_code(key)? {
        "logged_out" => "未登录",
        "email_mismatch" => "邮箱不符",
        _ => "已登录",
    })
}

/// _gui state 的 api_key（PROTOCOL 2.2）：存没存 key、掩码、在哪。照 config.json 里记的说（存的时候算好的掩码），不读钥匙串；
/// 第三方 API 账号换了服务商（域名和存的时候不一样）算没存——启动时也取不到它。官方账号记下备查的还有：shared（会经设备统计
/// 的服务器加密同步给登录了这个账号的其他设备，见 keyshare 一节）、saved_epoch（什么时候记下的）、from_device（从哪台设备
/// 同步来的）、sync_error（最近一次没同步成的原因，share_err）
pub fn gui_api_key(c: &Dict, k: &str, v: &V, share: bool, share_err: &V) -> R<Dict> {
    let out = dict! {
        "saved" => false, "hint" => "", "source" => "", "shared" => share, "saved_epoch" => V::None, "from_device" => V::None,
        "sync_error" => if share { share_err.clone() } else { V::None },
    };
    if let Some(key) = v.as_dict().filter(|_| is_api(v)).and_then(|d| d.get("key")).filter(secret_ok) {
        out.set("saved", true);
        out.set("hint", key_hint(&key.py_str()));
        out.set("source", "table");
        return Ok(out);
    }
    let (aid, st) = (acct_id(k, t()), t().state(c, false));
    let h = kc_hints(&st).dict(&aid).unwrap_or_default();
    let (host, is_ref) = kc_slot(v);
    // h.get("host", host) == host：没记域名的（旧版本存的）算对得上
    let host_ok = h.get("host").is_none_or(|x| x.as_str() == Some(host.as_str()));
    let saved = py_in(&aid, st.get(kc_list(is_ref)))? && host_ok;
    let hint = if saved && h.get("host").is_some_and(|x| x.as_str() == Some(host.as_str())) { h.str("hint").map_or(String::new(), |s| s.to_string()) } else { String::new() };
    out.set("saved", saved);
    out.set("hint", if HINT_RE.matches(&hint) { hint } else { String::new() });
    out.set("source", if saved { "keychain" } else { "" });
    if saved && is_ref {
        let at = h.get("at").filter(crate::cmd::keyshare::is_num).filter(|a| a.as_num().is_some_and(|a| a > 0.0));
        out.set("saved_epoch", at.unwrap_or(V::None));
        let from = h.get("from").filter(|f| f.as_str().is_some_and(|f| !f.is_empty() && text::isprintable(f)));
        out.set("from_device", from.unwrap_or(V::None));
    }
    Ok(out)
}

/// App 多久拉一次映射（App 不重算，看 state 的 next_check_epoch）；命令行还是启动时一天一次
pub const POLICY_POLL_INTERVAL: f64 = 3600.0;

/// App 下次该拉映射的时间：上次拉（$POLICY_DIR/checked）之后过 POLICY_POLL_INTERVAL 秒；没拉过是 None
pub fn policy_next_check() -> Option<f64> {
    Some(crate::py::shutil::getmtime(&path::join_all(&[&CCA_HOME, "policy", "checked"])).ok()? + POLICY_POLL_INTERVAL)
}

/// 配置向导还要不要出现：显式标记过（cmd_onboard 做完写的），或者这台机器已经有账号 / 配过路由器 / 配过本机客户端
/// （老用户升级上来：自然算配置过，不打扰）→ 不再出现。全新装、什么都没配时才是 False。只读，不写文件
pub fn onboarded(c: &Dict) -> bool {
    if c.truthy("onboarded") {
        return true;
    }
    let set = |section: &str, field: &str| c.get(section).filter(|s| s.truthy()).and_then(|s| s.as_dict()).is_some_and(|s| s.truthy(field));
    if set("router", "host") || set("local", "client") {
        return true;
    }
    crate::core::tool::tools().iter().any(|tool| tool.tab().is_some_and(|tab| !tab.is_empty()))
}

/// coffee_check 之后（启动、cca check）：这次的出口检测记进 health.json。泄露的状态用 leak-verdict 的 S 行，不解析中文
pub fn cmd_health_save(args: &[String]) -> R<()> {
    arity("cmd_health_save", args, 8, 13)?;
    let r = coffee_parse(&args[5], &args[6], &args[7])?;
    let (line, dns_msg, webrtc_msg, dns, webrtc) = (arg(args, 8, ""), arg(args, 9, ""), arg(args, 10, ""), arg(args, 11, ""), arg(args, 12, ""));
    let or_none = |s: &str| if s.is_empty() { V::None } else { V::from(s) };
    // 有横幅那一句才有 S 行（leak-verdict）
    let leak = if line.is_empty() {
        V::None
    } else {
        V::Dict(dict! { "dns" => or_none(dns), "webrtc" => or_none(webrtc), "line" => line, "dns_message" => dns_msg, "webrtc_message" => webrtc_msg })
    };
    let rc = r.at("rc").as_int().unwrap_or_else(|| panic!("KeyError: {}", r.at("rc").py_repr()));
    let mut fields = vec![("result", V::from(health_result(rc))), ("message", r.at("line")), ("leak", leak)];
    for k in ["ip", "country", "org", "trust", "trust_label", "tags", "restricted_region", "note"] {
        fields.push((k, r.at(k)));
    }
    health_write(&args[0], &args[1], "exit", &args[2], &args[3], &args[4], &fields)
}

/// 没有出口检测数据的结果也记进 health.json：第三方 API 账号的接口连不连得上（api_ok / api_unreachable）、端口不通
pub fn cmd_health_note(args: &[String]) -> R<()> {
    arity("cmd_health_note", args, 7, 8)?;
    let fields = [("result", V::from(&args[3])), ("message", V::from(arg(args, 7, "")))];
    health_write(&args[0], &args[1], &args[2], &args[4], &args[5], &args[6], &fields)
}

/// cca check --json：stdin 是 bash 收集的每个账号一行「键<TAB>health 的 JSON」
pub fn cmd_check_render(args: &[String]) -> R<()> {
    arity("cmd_check_render", args, 0, 1)?;
    let route = arg(args, 0, "");
    let rows = List::new();
    let mut ips: Vec<V> = Vec::new();
    let input = io::read_stdin_text();
    for line in input.split_inclusive('\n') {
        let (k, _, j) = text::partition(line.trim_end_matches('\n'), "\t");
        if k.is_empty() {
            continue;
        }
        let h = if j.is_empty() { None } else { json::loads(j).ok().and_then(|h| h.as_dict()) };
        let field = |name: &str| h.as_ref().and_then(|h| h.get(name)).unwrap_or(V::None);
        if let Some(ip) = h.as_ref().filter(|h| !h.is_empty()).and_then(|h| h.get("ip")).filter(|ip| ip.truthy()) {
            ips.push(ip);
        }
        let health = h.as_ref().map_or(V::None, V::from);
        rows.push(dict! { "key" => k, "name" => name_of(k), "node" => field("node"), "port" => field("port"), "health" => health });
    }
    // sorted({ip for ip in ips if ips.count(ip) > 1})
    let mut shared: Vec<V> = Vec::new();
    for ip in &ips {
        if ips.iter().filter(|x| x.py_eq(ip)).count() > 1 && !shared.iter().any(|x| x.py_eq(ip)) {
            shared.push(ip.clone());
        }
    }
    shared.sort_by_key(|ip| ip.py_str());
    let doc = dict! {
        "version" => 1, "kind" => "check", "tool" => t().name, "generated_at" => time(),
        "route" => if route.is_empty() { V::None } else { V::from(route) }, "accounts" => rows, "shared_exit_ips" => shared,
    };
    out!("{}", json::dumps(&V::Dict(doc), json::FILE));
    Ok(())
}

/// cca(py) onboard-done：记下这台机器已经走过配置向导（config.json 的 onboarded），以后 App 不再弹向导。
/// cmd_onboard 做完、以及 App 的“以后再说”（--skip）调用
pub fn cmd_onboard_done(args: &[String]) -> R<()> {
    arity("cmd_onboard_done", args, 0, 0)?;
    let c = cfg_load()?;
    c.set("onboarded", true);
    cfg_save(&c)
}

/// bash 切换到当前工具时要的变量
pub fn cmd_tool_env(args: &[String]) -> R<()> {
    arity("cmd_tool_env", args, 0, 0)?;
    let t = t();
    sh(&[
        ("TABLE", &t.table),
        ("CMD", t.cmd),
        ("TOOL_LABEL", t.label),
        ("TOOL_SETTINGS", t.settings_disp),
        ("TRACE_HOST", t.trace_host),
        ("APP_LABEL", t.app_label),
        ("BILLING_URL", t.billing_url),
        ("WEB_URL", t.web_url),
        ("REGION_NAME", t.region_name),
        ("INSTALL_URL", t.install_url),
    ]);
    Ok(())
}

/// 版本号里的一段数字：去掉前面的 0 以后先比位数再比数字（多长都行，和 Python 的整数一样）
type Part = (usize, String);

/// 版本号的先后，和 bash 的 ver_gt 一样：同号的测试版比正式版旧，测试版之间比 beta 后面的数；认不出是 None。
/// 最后一项：(是不是正式版, beta 后面的数)，正式版排在所有测试版后面（Python 里是 float('inf')）
pub fn ver_key(v: &str) -> Option<(Part, Part, Part, (bool, Part))> {
    let m = re!(r"^(\d+)\.(\d+)\.(\d+)(?:-beta\.(\d+))?$").match_(v)?;
    let part = |s: &str| -> Part {
        let digits: String = s.chars().map(|c| char::from_digit(uni::decimal_value(c).unwrap_or(0), 10).unwrap_or('0')).collect();
        let trimmed = digits.trim_start_matches('0');
        (trimmed.len(), trimmed.to_string())
    };
    let beta = match m.group(4) {
        Some(b) => (false, part(b)),
        None => (true, (0, String::new())),
    };
    Some((part(m.g(1)), part(m.g(2)), part(m.g(3)), beta))
}

/// cca update --check [--json]：stdin 是 GitHub 上最近的发布（api.github.com 的 releases 列表，拿不到时是空的），
/// fallback 是 releases/latest 指向的正式版（列表拿不到时 bash 才去取，只有版本号）。按现在的版本挑：测试版上看更新的
/// 测试版和正式版，正式版上只看正式版（自动更新、不带版本号的 cca update 不变，只看正式版）。
/// mode=json：stdout 输出 kind=update 的文档（macos/PROTOCOL.md 5.1）；否则给 bash UP_LATEST / UP_AVAILABLE / UP_BETA /
/// UP_NOTES（更新说明的前几行）。一个版本都没拿到时退出码 1
pub fn cmd_update_check(args: &[String]) -> R<()> {
    arity("cmd_update_check", args, 1, 4)?;
    let (current, mode, fallback, repo) = (args[0].as_str(), arg(args, 1, ""), arg(args, 2, ""), arg(args, 3, ""));
    let beta = current.contains("-beta.");
    let input = io::read_stdin_text();
    let rels = json::loads(if input.is_empty() { "null" } else { &input }).ok().and_then(|r| r.as_list()).map(|l| l.items()).unwrap_or_default();
    // (版本, 更新说明, 网址, 发布时间)
    let mut found: Vec<(String, V, String, V)> = Vec::new();
    for x in &rels {
        let x = x.as_dict().filter(|x| !x.truthy("draft"));
        let tag = x.as_ref().and_then(|x| x.get("tag_name")).filter(|tag| tag.truthy()).map_or(String::new(), |tag| tag.py_str());
        let v = if tag.starts_with(['v', 'V']) { &tag[1..] } else { &tag[..] };
        if ver_key(v).is_none() || (v.contains("-beta.") && !beta) {
            continue;
        }
        let x = x.expect("认得出版本号的一定是字典");
        let str_or_none = |k: &str| x.get(k).filter(|s| s.as_str().is_some());
        let url = str_or_none("html_url").map_or_else(|| format!("{repo}/releases/tag/{tag}"), |u| u.py_str());
        let published = parse_ts(&x.get("published_at").unwrap_or(V::None))?.map_or(V::None, V::Float);
        found.push((v.to_string(), str_or_none("body").unwrap_or(V::None), url, published));
    }
    let mut source = "releases";
    if found.is_empty() && ver_key(fallback).is_some() {
        found.push((fallback.to_string(), V::None, format!("{repo}/releases/tag/v{fallback}"), V::None));
        source = "latest";
    }
    // max(found, key=…)：一样大时取先出现的
    let Some(best) = found.iter().reduce(|best, f| if ver_key(&f.0) > ver_key(&best.0) { f } else { best }) else {
        return exit(1);
    };
    let available = ver_key(current).is_none_or(|now| ver_key(&best.0).is_some_and(|new| new > now));
    let notes = Some(&best.1).filter(|n| n.truthy());
    if mode == "json" {
        let doc = dict! {
            "version" => 1, "kind" => "update", "generated_epoch" => time(), "cca_version" => current,
            "channel" => if beta { "beta" } else { "stable" }, "source" => source, "latest" => best.0.as_str(), "available" => available,
            "notes" => notes.cloned().unwrap_or(V::None), "url" => best.2.as_str(), "published_epoch" => &best.3,
        };
        out!("{}", json::dumps(&V::Dict(doc), json::FILE));
        return Ok(());
    }
    let notes = notes.map_or(String::new(), |n| n.py_str());
    let lines: Vec<&str> = text::splitlines(&notes).into_iter().map(text::strip).filter(|s| !s.is_empty()).take(3).collect();
    sh(&[
        ("UP_LATEST", &best.0),
        ("UP_AVAILABLE", if available { "1" } else { "" }),
        ("UP_BETA", if best.0.contains("-beta.") { "1" } else { "" }),
        ("UP_NOTES", &lines.join("\n")),
    ]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ver_key_orders_like_bash_ver_gt() {
        let k = |s: &str| ver_key(s).unwrap();
        assert!(k("3.4.1") > k("3.4.1-beta.9") && k("3.4.1-beta.10") > k("3.4.1-beta.9") && k("3.10.0") > k("3.9.9"));
        assert!(k("3.4.1-beta.1") > k("3.4.0") && k("10.0.0") > k("9.99.99") && k("03.4.1") == k("3.04.1"));
        assert!(k("99999999999999999999999.0.0") > k("9999999999999999999999.0.0"));
        assert!(ver_key("3.4").is_none() && ver_key("v3.4.1").is_none() && ver_key("3.4.1-rc.1").is_none() && ver_key("").is_none());
        assert!(ver_key("3.4.1\n").is_some()); // `$` 在结尾的换行前也算（Python 就是这样）
    }
}
