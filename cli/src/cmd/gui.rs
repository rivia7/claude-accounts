//! 给界面 App 的接口（`using` 到 `cmd_check_render`、`gui_api_key` 到 `cmd_update_check`）：health-save、health-note、check-render、
//! onboard-done、gui-state、gui-live、tool-env、update-check。格式见 macos/PROTOCOL.md：只能加字段；删改要升 bash 的 GUI_API，
//! 同时改 Swift 模型（macos/Sources/CcaKit/Protocol）和回归测试（10g）。
//!
//! 都移植了。gui-state 把各模块读好的东西拼成一份快照：账号（登录、套餐、缓存的用量、每周用量、折合金额、出口检测、
//! 邮箱助手、记下的 API key）、端口池、内置代理、设备统计。

use super::corenet::{local_info, local_names, local_want, slot_orders, udp_view};
use super::cost::CostView;
use super::keyshare::keyshare_error;
use super::launchers::{launcher_badges, launcher_names};
use super::leak::coffee_parse;
use super::presence::{app_proc_re, presence_load};
use super::session::{cli_marks, cli_view, gui_procs, proc_starts};
use super::usage::{parse_ts, usage_cached, usage_entry, usage_row, usage_weeks_view};
use super::{arg, arity, Cmd};
use crate::cmd::secrets::{kc_hints, kc_list, kc_slot, py_in};
use crate::core::base::{sh, CCA_HOME, CFG, FIRST_PORT, HINT_RE, NOTICES, RO_BAD};
use crate::core::ctx::{cfg_save, ctx, pool_size, reconcile_all};
use crate::core::model::{acct_id, adir, api_host, cfg_fill, cfg_load, is_api, key_hint, label_of, name_of, node_of, parse_key, pool, secret_ok};
use crate::core::policy::policy_map;
use crate::core::sync::sync_digest;
use crate::core::tool::{t, tools, Kind, Using};
use crate::py::fs::{load_quiet, save_json};
use crate::py::io;
use crate::py::json;
use crate::py::obj::{repr_str, Dict, List, V};
use crate::py::path;
use crate::py::proc::{getenv, Cmd as Proc, In, Out};
use crate::py::re::Re;
use crate::py::text;
use crate::py::time::time;
use crate::py::uni;
use crate::py::{exit, Fail, R};
use crate::{dict, out, re};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::LazyLock;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[
    Cmd::all("gui-state", cmd_gui_state),
    Cmd::all("gui-live", cmd_gui_live),
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

/// App 下次该拉映射的时间：上次拉（$POLICY_DIR/checked）之后过 POLICY_POLL_INTERVAL 秒；没拉过是 None
pub fn policy_next_check() -> Option<f64> {
    Some(crate::py::shutil::getmtime(&path::join_all(&[&CCA_HOME, "policy", "checked"])).ok()? + crate::core::policy::POLICY_POLL_INTERVAL as f64)
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

// --- cca _gui state / live -------------------------------------------------------------------------------------------------

/// `c.get(k) or {}`，接着就要 `.get()`：config.json 的一节（router、local）。手改成不是对象的真值时原来是没人接的 AttributeError
fn section(c: &Dict, k: &str) -> Dict {
    match c.get(k) {
        Some(V::Dict(d)) => d,
        Some(v) if v.truthy() => panic!("AttributeError: config.json 的 {k} 是 {}，不是对象", v.py_repr()),
        _ => Dict::new(),
    }
}

/// `x or None`
fn or_none(x: Option<V>) -> V {
    x.filter(|v| v.truthy()).unwrap_or(V::None)
}

/// bash 才知道的（找到的命令、App、Chrome）经 CCA_GUI_* 环境变量传进来，加上这一次读好的设备统计、折合金额
struct GuiEnv<'a> {
    bin: Option<String>,
    app: Option<String>,
    chrome: bool,
    presence: bool,
    key_error: Option<String>,
    cost: &'a mut CostView,
}

/// `mail_view(c, k)`（邮箱助手：账号页的「邮箱」一节）。在 mail 那一批里移植，合进来以后换成它
fn mail_view(c: &Dict, key: &str) -> V {
    let m = mail_cfg(c, key);
    let field = |k: &str, default: V| m.as_ref().map_or(default, |m| m.get(k).unwrap_or(V::None));
    V::Dict(dict! {
        "configured" => m.is_some(),
        "user" => field("user", V::None),
        "source" => field("source", V::None),
        "auto" => field("auto", V::Bool(true)),
        "forwarder" => field("forwarder", V::None),
    })
}

/// `mail_cfg(c, key)`：某账号解析后的邮箱助手设置（覆盖 config.json 的 mail.accounts[acct_id] → 默认 mail）：
/// {user, auto, proxy, source: 'account' | 'default', forwarder} 或 None（没有可连的 Gmail）。
/// mail 那一批合进来以前先放在这里（只有 key 给定的这一种用法），合进来以后换成它的
fn mail_cfg(c: &Dict, key: &str) -> Option<Dict> {
    let m = c.dict("mail")?;
    let default = super::mail::mail_one(&V::Dict(m.clone()), true, "system");
    let ov = m.dict("accounts").and_then(|a| a.get(&acct_id(key, t()))).and_then(|o| o.as_dict());
    let one = ov.as_ref().and_then(|o| {
        let (auto, proxy) = match &default {
            Some(d) => (d.at("auto").truthy(), d.at("proxy").py_str()),
            None => (true, "system".to_string()),
        };
        super::mail::mail_one(&V::Dict(o.clone()), auto, &proxy)
    });
    let one = match (one, default) {
        (Some(one), _) => {
            one.set("source", "account");
            one
        }
        (None, Some(default)) => {
            let one = Dict::new(); // dict(default, source='default')
            for (k, v) in default.items() {
                one.set(k, v);
            }
            one.set("source", "default");
            one
        }
        (None, None) => return None,
    };
    let fwd = ov.and_then(|o| o.str("forwarder").map(|f| f.to_lowercase())).filter(|f| super::mail::MAIL_DOMAIN_RE.matches(f));
    one.set("forwarder", fwd.map_or(V::None, V::from));
    Some(one)
}

/// _gui state 里的一个账号（当前工具是它的工具）；cli 是 cli_view 的结果
fn gui_account(c: &Dict, k: &str, v: &V, procs: &[(i64, String)], badges: &[(Rc<str>, (String, &'static str))], pm: &Dict, env: &mut GuiEnv, now: f64, cli: Dict) -> R<Dict> {
    let tool = t();
    let (a, e) = parse_key(k);
    let api = is_api(v);
    // T.state(c)['ports'].get(k)
    let port = match tool.state(c, false).at("ports") {
        V::Dict(ports) => ports.get(k).unwrap_or(V::None),
        other => panic!("AttributeError: ports 是 {}，不是对象", other.py_repr()),
    };
    let slot = if port.truthy() {
        Some(port.as_int().unwrap_or_else(|| panic!("TypeError: 端口是 {}，不是整数", port.py_repr())) - FIRST_PORT + 1)
    } else {
        None
    };
    let login = if api { "api" } else { login_code(k)? };
    let (mut usage, mut plan, mut cached, mut tier) = (V::None, None, None, "");
    if !api && login == "logged_in" {
        cached = usage_cached(k)?;
        let sub = or_none(cached.as_ref().and_then(|x| x.rl.get("plan")));
        let p = tool.plan(k, &sub)?;
        tier = tool.plan_tier(k, &sub)?;
        if let Some(x) = &cached {
            let mut row = usage_row(k, p.as_str())?;
            row.source = "cache";
            row.at = Some(x.at);
            row.rl = x.rl.clone();
            row.outdated_login_plan = tool.outdated_login_plan(k)?;
            usage = V::Dict(usage_entry(&row, now)?);
        }
        plan = Some(p);
    }
    let pat = if api { None } else { app_proc_re(tool, &path::join(&adir(k), "desktop")) };
    let pid = pat.and_then(|pat| procs.iter().find(|(_, cmd)| pat.found(cmd)).map(|(p, _)| *p));
    let why = Dict::new();
    let has_bin = env.bin.is_some();
    if api {
        why.set("app", "api_account");
        why.set("web", "api_account");
        why.set("usage", "api_account");
    } else {
        if !tool.has_app {
            why.set("app", "unsupported"); // agy 没有桌面 App
        } else if env.app.is_none() {
            why.set("app", "app_missing");
        } else if tool.kind == Kind::Codex && !has_bin {
            why.set("app", "tool_missing"); // ChatGPT App 打开前要用 codex 命令检查设置
        }
        if !env.chrome {
            why.set("web", "no_chrome"); // 没有 Chrome 时 cca web 只能用默认浏览器打开，不经账号的出口
        }
        if !tool.has_usage {
            why.set("usage", "unsupported"); // agy 没有查用量的接口
        } else if !has_bin {
            why.set("usage", "tool_missing");
        } else if login != "logged_in" {
            why.set("usage", "not_logged_in");
        }
    }
    if !has_bin {
        why.set("cli", "tool_missing"); // 第三方 API 账号也是经 claude / codex 命令用的
    }
    let badge = badges.iter().find(|(key, _)| &**key == k).map(|(_, (label, color))| dict! { "label" => label.as_str(), "color" => *color });
    let weeks = if api || !tool.has_usage { None } else { usage_weeks_view(k, now, cached.as_ref())? };
    let can_cost = env.cost.on && tool.has_cost && !api; // 折合金额（PROTOCOL 2.2 的 cost）
    let lower = e.to_lowercase();
    let mapped = !e.is_empty() && pm.has(&lower);
    let plan_shown = plan.as_deref().filter(|p| !p.is_empty() && *p != "—");
    let can = Dict::new();
    for x in ["app", "web", "usage", "check", "cli"] {
        can.set(x, !why.has(x));
    }
    can.set("billing", !why.has("web") && !tool.billing_url.is_empty()); // 控制台的账单页和网页版走同一条路（cmd_web）；agy 没有
    can.set("limit_resets", tool.kind == Kind::Claude && !why.has("web")); // claude.ai 的用量页，只有 Claude 账号有
    can.set("policy_lock", !api && !e.is_empty() && c.truthy("policy") && !pm.has(&lower));
    can.set("devices", env.presence && !api && mapped); // 账号页的「设备」（PROTOCOL 3.4）
    can.set("api_key", true); // 记下 / 显示 / 复制 API key（cca [codex] key，PROTOCOL 4.10）
    let with_mail = tool.kind == Kind::Claude && !api && !e.is_empty();
    can.set("mail", with_mail); // 邮箱助手：账号页的「邮箱」一节（PROTOCOL 4.11），只有 Claude 官方账号有
    can.set("cost", can_cost);
    let share = env.presence && !api && mapped && matches!(tool.kind, Kind::Claude | Kind::Codex);
    let share_err = env.key_error.as_deref().map_or(V::None, V::from);
    let cost = if can_cost { env.cost.account(tool, k, weeks.as_ref(), mapped, login == "logged_in") } else { V::None };
    let node = node_of(v);
    Ok(dict! {
        "key" => k, "id" => acct_id(k, tool), "alias" => a, "email" => e.as_str(), "name" => name_of(k), "label" => label_of(k),
        "kind" => if api { "api" } else { "oauth" }, "login" => login, "login_email" => if api { String::new() } else { tool.email(k)? },
        "plan" => plan_shown.unwrap_or(""), "plan_tier" => if plan_shown.is_some() { tier } else { "" },
        "node" => if node.is_empty() { V::None } else { V::from(&*node) }, "port" => &port, "slot" => slot,
        "group" => slot.map(|s| format!("CCA-{s}")),
        "pinned" => !api && mapped, "pinned_node" => if !api && !e.is_empty() { pm.get(&lower).unwrap_or(V::None) } else { V::None },
        "is_last" => tool.state(c, false).get("last").is_some_and(|l| l.as_str() == Some(k)),
        "api" => if api {
            let d = v.as_dict().expect("is_api 说它是字典");
            V::Dict(dict! { "host" => api_host(&d), "model" => d.get("model").filter(|m| m.truthy()).unwrap_or_else(|| V::from("")) })
        } else {
            V::None
        },
        "badge" => badge,
        "app" => dict! { "running" => pid.is_some(), "pid" => pid }, "cli" => cli,
        "can" => can, "can_why" => why,
        "mail" => if with_mail { mail_view(c, k) } else { V::None },
        "api_key" => gui_api_key(c, k, v, share, &share_err)?,
        "usage" => usage, "usage_weeks" => weeks,
        "cost" => cost,
        "health" => health_view(k, v, now),
    })
}

/// cca _gui state：图形界面的只读快照，三个工具一起。CCA_RO=1 下运行：对账只在内存里做，不写文件、不往 stderr 说
/// （本来会做的改动在 notices 里）。读不了的文件报在 errors 里，不退出。stdin 是路由器端代码（算同步摘要）；
/// bash 才知道的（找到的命令、App、Chrome）经 CCA_GUI_* 环境变量传进来
pub fn cmd_gui_state(args: &[String]) -> R<()> {
    arity("cmd_gui_state", args, 0, 0)?;
    let now = time();
    let mut errors: Vec<V> = Vec::new();
    let raw = match load_quiet(&CFG) {
        Some(V::Dict(raw)) => raw,
        _ => {
            errors.push(V::Dict(dict! { "code" => "config_unreadable", "tool" => V::None, "message" => format!("{} 读不了或格式不对", *CFG) }));
            Dict::new()
        }
    };
    let c = cfg_fill(raw);
    for tool in tools() {
        if !path::exists(&tool.table) {
            if tool.kind == Kind::Claude {
                tool.set_tab(Some(Dict::new()));
            }
            continue;
        }
        let Some(V::Dict(table)) = load_quiet(&tool.table) else {
            errors.push(V::Dict(dict! { "code" => "table_unreadable", "tool" => tool.name, "message" => format!("{} 读不了或格式不对", tool.table) }));
            continue;
        };
        tool.set_ids(&table);
        tool.set_tab(Some(table));
    }
    reconcile_all(&c)?;
    let code = io::read_stdin_text();
    let (r, loc) = (section(&c, "router"), section(&c, "local"));
    // local_want 只在配了内置代理时才算（没配时原来也不调它）
    let want = if loc.truthy("client") { local_want(&c)? } else { String::new() };
    let lnames = local_names(&c);
    // who = {端口: (工具名, 键)}：端口一样的后来的算数
    let port_key = |p: &V| p.py_repr();
    let mut who: HashMap<String, (&'static str, Rc<str>)> = HashMap::new();
    for (tool, k, _, p) in pool(&c) {
        who.insert(port_key(&p), (tool.name, k));
    }
    let local_name = |n: &V| match n.as_str() {
        Some(s) => lnames.get(s).unwrap_or_else(|| n.clone()),
        None => n.clone(),
    };
    let mut slots: Vec<V> = Vec::new();
    for (s, (g, p, order)) in slot_orders(&c, false).into_iter().enumerate() {
        let tk = who.get(&port_key(&V::Int(p)));
        let first = order.first().cloned().unwrap_or_else(|| panic!("IndexError: 槽位 {g} 没有节点"));
        slots.push(V::Dict(dict! {
            "slot" => s + 1, "group" => g, "port" => p, "tool" => tk.map(|x| x.0), "key" => tk.map(|x| V::from(&x.1)),
            "node" => &first, "local_node" => local_name(&first),
        }));
    }
    let users = Dict::new();
    for (tool, k, v, _) in pool(&c) {
        let node = node_of(&v);
        let list = users.setdefault(&node, List::new()).as_list().expect("刚放进去的");
        list.push(dict! { "tool" => tool.name, "key" => k });
    }
    let pol = c.get("policy").unwrap_or(V::None);
    let nodes = c.at("nodes");
    let node_rows: Vec<V> = match &nodes {
        V::List(l) => l
            .items()
            .iter()
            .map(|n| {
                let used = n.as_str().and_then(|s| users.get(s)).unwrap_or_else(|| V::List(List::new()));
                V::Dict(dict! { "name" => n, "local_name" => local_name(n), "users" => used })
            })
            .collect(),
        other => panic!("TypeError: nodes 是 {}，不是列表", other.py_repr()),
    };
    let host = r.get("host").filter(|h| h.truthy());
    let local_sync = if !loc.truthy("client") {
        "n/a"
    } else if loc.get("applied").is_some_and(|a| a.as_str() == Some(want.as_str())) {
        "ok"
    } else {
        "pending"
    };
    let local = local_info(&c);
    let out = dict! {
        "version" => 1, "kind" => "state", "generated_at" => now, "cca_version" => getenv("CCA_VERSION").unwrap_or_default(),
        "errors" => V::None, "notices" => V::None,
        "config" => dict! {
            "route_pref" => c.get("route").filter(|x| x.truthy()).unwrap_or_else(|| V::from("auto")),
            "router" => dict! {
                "configured" => host.is_some(), "host" => host.clone(), "ssh_port" => r.get("port").filter(|x| x.truthy()).unwrap_or(V::Int(22)),
                "proxy_host" => c.get("proxy_host").filter(|x| x.truthy()).or_else(|| host.clone()),
            },
            "local" => &local,
            "nodes" => &nodes, "min_trust" => c.at("min_trust"), "leak_check" => !matches!(c.get("leak_check"), Some(V::Bool(false))),
            "update" => c.at("update"),
            "policy" => dict! {
                "configured" => pol.truthy(), "repo" => if pol.is_str() { pol.clone() } else { V::None },
                "next_check_epoch" => if pol.truthy() { policy_next_check() } else { None },
            },
            "onboarded" => onboarded(&c),
        },
        "routing" => dict! {
            "first_port" => FIRST_PORT, "pool_size" => pool_size(&c),
            "sync" => if host.is_none() { "n/a" } else if c.get("synced").is_some_and(|s| s.as_str() == Some(sync_digest(&c, &code).as_str())) { "ok" } else { "pending" },
            "local_sync" => local_sync,
            "slots" => slots,
            "nodes" => node_rows,
        },
        "tools" => List::new(),
    };
    let procs = gui_procs();
    local.set("udp", udp_view(&c, Some(&procs))); // 本机 UDP 走节点（只在 macOS 上，别的系统是 null）
    let pcfg = presence_load()?.1;
    let presence = pcfg.is_some();
    // 记下备查的 API key 最近一次没同步成的原因（api_key 的 sync_error）
    let key_error = if presence { keyshare_error() } else { None };
    // 折合金额：读一次统计好的（cost/ 里的），各账号按时段切
    let mut cv = CostView::new(&c, now, pcfg.as_ref());
    let entries = out.list("tools").expect("刚建的");
    let mut enabled_ids: Vec<&'static str> = Vec::new();
    for tool in tools() {
        let name = tool.name.to_uppercase();
        let var = |suffix: &str| getenv(&format!("CCA_GUI_{name}_{suffix}")).filter(|s| !s.is_empty());
        let mut env = GuiEnv { bin: var("BIN"), app: var("APP"), chrome: getenv("CCA_GUI_CHROME").as_deref() == Some("1"), presence, key_error: key_error.clone(), cost: &mut cv };
        let enabled = tool.kind == Kind::Claude || path::exists(&tool.table);
        let entry = dict! {
            "id" => tool.name, "label" => tool.label, "app_label" => tool.app_label,
            "cli_prefix" => text::split_ws(tool.cmd).into_iter().skip(1).map(V::from).collect::<Vec<V>>(),
            "features" => dict! { "app" => tool.has_app, "api" => tool.has_api, "usage" => tool.has_usage, "cost" => tool.has_cost },
            "enabled" => enabled, "bin" => env.bin.clone(), "app" => dict! { "installed" => env.app.is_some(), "path" => env.app.clone() },
            "chrome" => env.chrome, "settings_problems" => List::new(), "errors" => List::new(), "last" => V::None, "accounts" => List::new(),
        };
        if let Some(table) = tool.tab().filter(|_| enabled) {
            let _using = Using::tool(tool);
            entry.set("settings_problems", tool.settings_problems().into_iter().map(V::from).collect::<Vec<V>>());
            let official = Dict::new();
            for (k, v) in table.items() {
                if !is_api(&v) {
                    official.set(k, v);
                }
            }
            let badges = launcher_badges(&c, &official, &launcher_names(&official));
            let pm = policy_map(tool, Some(&c));
            entry.set("errors", tool.errs().into_iter().map(|m| V::Dict(dict! { "code" => "table_invalid", "message" => m })).collect::<Vec<V>>());
            entry.set("last", or_none(tool.state(&c, false).get("last")));
            let marks: Vec<(Rc<str>, Vec<Dict>)> = table.keys().into_iter().map(|k| (k.clone(), cli_marks(&k))).collect();
            // 没有登记时不跑 ps
            let pids: Vec<i64> = marks.iter().flat_map(|(_, ms)| ms.iter().map(|m| m.at("pid").as_int().unwrap_or(0))).collect();
            let starts = proc_starts(&pids);
            let accounts = List::new();
            for ((k, v), (_, ms)) in table.items().into_iter().zip(marks.iter()) {
                accounts.push(gui_account(&c, &k, &v, &procs, &badges, &pm, &mut env, now, cli_view(ms, &starts))?);
            }
            entry.set("accounts", accounts);
        }
        if enabled {
            enabled_ids.push(tool.name);
        }
        entries.push(entry);
    }
    // dict.fromkeys(RO_BAD)：重复的只留先来的
    let mut seen: Vec<(String, String)> = Vec::new();
    for bad in RO_BAD.with(|b| b.borrow().clone()) {
        if !seen.contains(&bad) {
            errors.push(V::Dict(dict! { "code" => "account_unreadable", "tool" => bad.0.as_str(), "message" => format!("{} 读不了或格式不对（当作没登录）", bad.1) }));
            seen.push(bad);
        }
    }
    out.set("errors", errors);
    out.set("notices", NOTICES.with(|n| n.borrow().iter().cloned().map(V::Dict).collect::<Vec<V>>()));
    out.set("cost_total", cv.total(&enabled_ids));
    out!("{}", json::dumps(&V::Dict(out), json::FILE));
    Ok(())
}

/// 这台 Mac 连不连得上这些端口：{端口: bool}（并行，每个最多 1 秒；用 PATH 里的 nc，和 probe 一样）
fn gui_reach(host: &str, ports: &[i64]) -> HashMap<i64, bool> {
    let handles: Vec<(i64, std::thread::JoinHandle<bool>)> = ports
        .iter()
        .map(|p| {
            let (host, port) = (host.to_string(), p.to_string());
            let handle = std::thread::spawn(move || {
                let wait = if cfg!(target_os = "macos") { "-G" } else { "-w" };
                let cmd = Proc::new(["nc", "-z", wait, "1", host.as_str(), port.as_str()]).stdin(In::Null).stdout(Out::Null).stderr(Out::Null).timeout(5.0);
                cmd.run().is_ok_and(|done| done.rc == 0)
            });
            (*p, handle)
        })
        .collect();
    handles.into_iter().map(|(p, h)| (p, h.join().unwrap_or(false))).collect()
}

/// 路由器 status() 的槽位行
static GUI_SLOT_RE: LazyLock<Re> = LazyLock::new(|| Re::new(r"^槽位 (CCA-\d+) :(\d+) (监听中|未监听) → (.*?)(?:（应为 .*）)?$"));

/// cca _gui live：端口池的实况（只读）。stdin 是 bash 收集的原始数据：在路由器上是这一版 router_sh 的 status 输出
/// （rc 是 ssh 的退出码），在本机是控制接口的 V<TAB>状态码、逐组 G<TAB>组<TAB>状态码<TAB>选中的节点。
/// 应选的节点按这台 Mac 的账号表算（不看路由器上的应为），选中的不一样就是 drift，同步一次就好
pub fn cmd_gui_live(args: &[String]) -> R<()> {
    arity("cmd_gui_live", args, 1, 5)?;
    let (route, host, rc, at_home) = (args[0].as_str(), arg(args, 1, ""), arg(args, 2, "0"), arg(args, 3, ""));
    let c = ctx()?.0;
    let input = io::read_stdin_text();
    let r = section(&c, "router");
    // 连不连得上路由器（不管经哪块网卡）；at_home 是算不算在家（只经 VPN、ZeroTier 这类虚拟网卡连得到的不算）
    let reach_arg = match arg(args, 4, "") {
        "" => at_home,
        given => given,
    };
    let rhost = r.get("host").filter(|h| h.truthy());
    let router = dict! {
        "configured" => rhost.is_some(), "host" => rhost.clone(), "reachable" => reach_arg == "yes", "backend" => V::None,
        "running" => V::None, "hook" => V::None, "cron" => V::None, "error" => V::None,
    };
    let local = local_info(&c).copy();
    local.set("running", V::None);
    local.set("error", V::None);
    let lines: Vec<&str> = text::splitlines(&input).into_iter().filter(|l| !text::strip(l).is_empty()).collect();
    // 各组：选中的节点（没有是 None）、内核里有没有、在不在监听
    let (mut sel, mut inkernel, mut listening): (HashMap<String, Option<String>>, HashMap<String, bool>, HashMap<String, bool>) = Default::default();
    if route == "router" {
        if rhost.is_none() {
            router.set("error", dict! { "code" => "not_configured", "message" => "还没配置路由器（cca router setup）" });
        } else if reach_arg != "yes" {
            router.set("error", dict! { "code" => "unreachable", "message" => format!("连不上路由器 {}", r.get("host").unwrap_or(V::None).py_str()) });
        } else if rc != "0" {
            router.set("error", dict! { "code" => if rc == "255" { "unreachable" } else { "status_failed" }, "message" => lines.last().copied().unwrap_or("") });
        } else {
            for l in &lines {
                if let Some(m) = re!(r"^(OpenClash|ShellCrash)：(运行中|未运行)").match_(l) {
                    router.set("backend", m.g(1));
                    router.set("running", m.g(2) == "运行中");
                } else if l.starts_with("钩子：") {
                    router.set("hook", *l == "钩子：已安装");
                } else if l.starts_with("自愈 cron：") {
                    router.set("cron", *l == "自愈 cron：已安装");
                } else if let Some(m) = GUI_SLOT_RE.match_(l) {
                    let (g, now) = (m.g(1).to_string(), m.g(4));
                    listening.insert(g.clone(), m.g(3) == "监听中");
                    let known = now != "（内核里没有）";
                    inkernel.insert(g.clone(), known);
                    sel.insert(g, if known && !now.is_empty() && now != "?" { Some(now.to_string()) } else { None });
                }
            }
            if matches!(router.get("running"), Some(V::Bool(false))) {
                router.set("error", dict! { "code" => "not_running", "message" => format!("{} 没在运行", router.at("backend").py_str()) });
            }
        }
    } else if route == "local" {
        if !local.truthy("configured") {
            local.set("error", dict! { "code" => "not_configured", "message" => "还没配置内置代理（cca local setup）" });
        }
        for l in &lines {
            let f: Vec<&str> = l.split('\t').collect();
            if f[0] == "V" && f.len() > 1 {
                local.set("running", f[1] == "200");
            } else if f[0] == "G" && f.len() > 2 {
                inkernel.insert(f[1].to_string(), f[2] == "200");
                let now = f.get(3).copied().unwrap_or("");
                sel.insert(f[1].to_string(), if f[2] == "200" && !now.is_empty() { Some(now.to_string()) } else { None });
            }
        }
        if local.truthy("configured") && matches!(local.get("running"), Some(V::Bool(false))) {
            local.set("error", dict! { "code" => "not_running", "message" => format!("{} 没在运行", local.at("name").py_str()) });
        }
    }
    let mut who: HashMap<i64, (&'static str, Rc<str>)> = HashMap::new();
    for (tool, k, _, p) in pool(&c) {
        if let Some(p) = p.as_int() {
            who.insert(p, (tool.name, k));
        }
    }
    let plan = slot_orders(&c, route == "local");
    let probe = !host.is_empty() && route != "none";
    let reach = if probe { gui_reach(host, &plan.iter().map(|(_, p, _)| *p).filter(|p| who.contains_key(p)).collect::<Vec<_>>()) } else { HashMap::new() };
    let mut slots: Vec<V> = Vec::new();
    let summary = dict! { "ok" => 0, "drift" => 0, "closed" => 0, "unknown" => 0, "empty" => 0 };
    for (i, (g, p, order)) in plan.iter().enumerate() {
        let tk = who.get(p);
        let s = sel.get(g).cloned().flatten();
        let expected = order.first().cloned().unwrap_or_else(|| panic!("IndexError: 槽位 {g} 没有节点"));
        let reachable = reach.get(p).copied();
        let st = if tk.is_none() {
            "empty"
        } else if !probe {
            "unknown"
        } else if reachable != Some(true) {
            "closed"
        } else {
            match &s {
                None => "unknown",
                Some(s) if expected.as_str() != Some(s.as_str()) => "drift",
                Some(_) => "ok",
            }
        };
        summary.set(st, summary.at(st).as_int().unwrap_or(0) + 1);
        let listens = if route == "router" { listening.get(g).copied() } else { tk.and(reachable) };
        slots.push(V::Dict(dict! {
            "slot" => i + 1, "group" => g.as_str(), "port" => *p, "tool" => tk.map(|x| x.0), "key" => tk.map(|x| V::from(&x.1)),
            "expected" => &expected, "selected" => s, "in_kernel" => inkernel.get(g).copied(),
            "listening" => listens, "reachable" => tk.and(reachable), "state" => st,
        }));
    }
    let doc = dict! {
        "version" => 1, "kind" => "live", "generated_at" => time(), "route" => route,
        "route_pref" => c.get("route").filter(|x| x.truthy()).unwrap_or_else(|| V::from("auto")),
        "at_home" => at_home == "yes", "proxy_host" => if host.is_empty() { V::None } else { V::from(host) },
        "router" => router, "local" => local, "slots" => slots, "summary" => summary,
    };
    out!("{}", json::dumps(&V::Dict(doc), json::FILE));
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
