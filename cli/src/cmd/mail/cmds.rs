//! 邮箱助手的各个子命令（`cmd_mail_setup` 到 `cmd_mail_link_check`、`cmd_gui_mail_list`）。提示的文字、退出码、先做什么后做什么都照原文。

use super::header::{mail_addrs, mail_text};
use super::hunt::{mail_deliver, mail_hunt, mail_take, mail_verdict, sleep_interruptible, Deliver, Found, Hunt, Pend};
use super::imap::{mail_crit, Cand, MailBox, MailSource, Uids};
use super::judge::{mail_ar, mail_digest, mail_from_alias, mail_fwd_ok, mail_judge_any, mail_links, mail_shape, urlsplit_host};
use super::message::{mail_parse, Message};
use super::route::{mail_route, mail_route_note};
use super::{
    catch_sigint, doc_forwarders, mail_all_addrs, mail_cfg, mail_clean, mail_error, mail_fatal, mail_forwarders, mail_last,
    mail_one, mail_pw_problem, mail_ready, mail_rules, mail_why, py_int, team_fwd_of, MailRule, MAIL_ADDR_RE, MAIL_DIR,
    MAIL_DOMAIN_RE, MAIL_HOST, MAIL_PROXY_HINT, MAIL_RECENT, MAIL_SVC, MAIL_WATCH_MAX,
};
use crate::cmd::secrets::{keyfile_store, keystore_name, ks_del, ks_read, ks_write};
use crate::cmd::{arg, arity};
use crate::core::base::{secret_re_match, CFG};
use crate::core::ctx::cfg_save;
use crate::core::model::{acct_id, cfg_load, name_of, parse_key, table_load};
use crate::core::policy::policy_sources;
use crate::core::team::grants;
use crate::core::tool::t;
use crate::py::fs;
use crate::py::io::{read_stdin, read_stdin_line};
use crate::py::json;
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::{die, exit, text, time, uni};
use crate::py::{Fail, R};
use crate::{dict, err, out};
use std::collections::HashMap;

/// `max(lo, min(int(s), hi))`（lo 不是负的）：多大的数都行，认不出是 None
fn clamp_int(s: &str, lo: i64, hi: i64) -> Option<i64> {
    let (neg, digits) = py_int(s)?;
    if neg {
        return Some(lo);
    }
    let n = if digits.len() > 18 { i64::MAX } else { digits.parse::<i64>().unwrap_or(i64::MAX) };
    Some(n.min(hi).max(lo))
}

/// 不是负的整数：十进制的写法，和它的值（放不进 64 位的没有值：IMAP 里 UID、UIDVALIDITY 是 32 位的，这种只可能是手写的）
struct Num {
    text: String,
    n: Option<u64>,
}

/// `--msg` 的「文件夹:uid:uidvalidity」
struct Handle {
    folder: String,
    uid: Num,
    valid: Num,
}

/// --msg 的「文件夹:uid:uidvalidity」→ (folder, uid, uidvalidity)：文件夹名不含冒号（Gmail 的 INBOX、[Gmail]/All Mail 都不含），
/// 从右边切两刀。写得不对是 None（原来的 ValueError）
fn mail_handle(s: &str) -> Option<Handle> {
    let mut parts = s.rsplitn(3, ':');
    let (valid, uid, folder) = (parts.next()?, parts.next()?, parts.next()?);
    if folder.is_empty() || !text::isdigit(uid) || !text::isdigit(valid) {
        return None;
    }
    // isdigit() 认的字符比 int() 多（上标的 ² 这些）：那种在 int() 里是 ValueError
    let num = |s: &str| py_int(s).map(|(_, text)| Num { n: text.parse().ok(), text });
    Some(Handle { folder: folder.to_string(), uid: num(uid)?, valid: num(valid)? })
}

/// `{parse_key(k)[1].lower(): k for k in table_load() if parse_key(k)[1]}`：账号的邮箱（小写）→ 账号表的键
fn accounts_by_email() -> R<HashMap<String, String>> {
    let mut names = HashMap::new();
    for k in table_load(t())?.keys() {
        let email = parse_key(&k).1;
        if !email.is_empty() {
            names.insert(email.to_lowercase(), k.to_string());
        }
    }
    Ok(names)
}

/// `frm[0].rpartition('@')[2]`
fn domain_of(addr: &str) -> &str {
    addr.rsplit_once('@').map_or(addr, |x| x.1)
}

/// 没人接的异常的类名（`type(e).__name__`）
fn class_name(e: &Fail) -> String {
    match e {
        Fail::Os(io) => match io.raw_os_error() {
            Some(libc::ENOENT) => "FileNotFoundError",
            Some(libc::EACCES | libc::EPERM) => "PermissionError",
            Some(libc::EEXIST) => "FileExistsError",
            Some(libc::EISDIR) => "IsADirectoryError",
            Some(libc::ENOTDIR) => "NotADirectoryError",
            Some(libc::EINTR) => "InterruptedError",
            Some(libc::ESRCH) => "ProcessLookupError",
            Some(libc::ECHILD) => "ChildProcessError",
            Some(libc::EPIPE | libc::ESHUTDOWN) => "BrokenPipeError",
            Some(libc::ECONNABORTED) => "ConnectionAbortedError",
            Some(libc::ECONNREFUSED) => "ConnectionRefusedError",
            Some(libc::ECONNRESET) => "ConnectionResetError",
            Some(libc::ETIMEDOUT) => "TimeoutError",
            Some(libc::EAGAIN | libc::EALREADY | libc::EINPROGRESS) => "BlockingIOError",
            _ => "OSError",
        }
        .to_string(),
        Fail::Value(_) => "ValueError".to_string(),
        Fail::Other(name, _) => name.to_string(),
        Fail::Die(_) | Fail::Exit(_) => "SystemExit".to_string(),
    }
}

/// `(code, text)`：这个错是 MailError 的话
fn mail_err(e: &Fail) -> Option<(String, String)> {
    mail_error(e).map(|(c, t)| (c.to_string(), t.to_string()))
}

/// cca mail setup [--account 键]：stdin 是应用专用密码。登录一次、列出邮件夹，通过了才存进钥匙串、写 config.json。
/// acct 为空时配默认（所有账号共用 config.json 的 mail），否则配这个账号的覆盖（mail.accounts[acct_id]）。
/// 退出码：0 好了；2 地址不对；3 密码不对（2、3 bash 重问）；4 连不上（网络、代理、证书）；5 存不进钥匙串
pub fn cmd_mail_setup(args: &[String]) -> R<()> {
    arity("mail-setup", args, 1, 3)?;
    let (proxy, acct) = (arg(args, 1, ""), arg(args, 2, ""));
    let stop = |rc: i32, text: &str| -> R<()> {
        err!("! {text}");
        exit(rc)
    };
    let addr = text::strip(&args[0]).to_lowercase();
    if !MAIL_ADDR_RE.matches(&addr) {
        return stop(2, "这不像邮箱地址（只认字母、数字和 . _ % + - 组成的地址）");
    }
    let pw: String = read_stdin().chars().filter(|c| !uni::is_space(*c)).collect();
    if !secret_re_match(&pw) {
        return stop(3, "应用专用密码是 16 个字母（空格不用输）");
    }
    let c = cfg_load()?;
    let spec = if proxy.is_empty() {
        let saved = mail_cfg(&c, Some(acct).filter(|a| !a.is_empty())).and_then(|m| m.str("proxy"));
        saved.filter(|p| !p.is_empty()).map_or_else(|| "system".to_string(), |p| p.to_string())
    } else {
        proxy.to_string()
    };
    let connect = || -> R<Vec<(String, String)>> {
        let route = mail_route(Some(&spec), &c, None)?;
        err!("正在连接 {}（{}）…", MAIL_HOST, mail_route_note(&route));
        let mut bx = MailBox::new(&addr, &pw, &route);
        bx.open()?;
        let places = bx.folders();
        bx.close(true);
        places
    };
    let places = match connect() {
        Ok(places) => places,
        Err(e) => match mail_err(&e) {
            Some((code, text)) if e.is("MailFatal") => return stop(if code == "auth" { 3 } else { 4 }, &text),
            Some((code, text)) if matches!(code.as_str(), "net" | "proxy") => {
                return stop(4, &format!("{text}。{MAIL_PROXY_HINT}"))
            }
            Some((_, text)) => return stop(4, &text),
            None => return Err(e),
        },
    };
    // 存不进去时原来的设置和密码都还在，照样能用
    if !ks_write(MAIL_SVC, &addr, &pw)? {
        let hint = if keyfile_store() { "看看 ~/.cca/secrets 的权限和磁盘空间" } else { "钥匙串锁着？" };
        return stop(5, &format!("应用专用密码没能存进{}（{}）", keystore_name(), hint));
    }
    let c = cfg_load()?;
    let before = mail_all_addrs(&c);
    let m = c.dict("mail").unwrap_or_default();
    let new = dict! { "user" => addr.as_str() };
    if acct.is_empty() {
        if matches!(m.get("auto"), Some(V::Bool(false))) {
            new.set("auto", false); // 用户关掉过的自动打开不改回来
        }
        if spec != "system" {
            new.set("proxy", spec.as_str());
        }
        if let Some(accts) = m.dict("accounts").filter(|a| !a.is_empty()) {
            new.set("accounts", accts); // 账号覆盖留着
        }
        c.set("mail", new);
    } else {
        let aid = acct_id(acct, t());
        let accts = m.dict("accounts").unwrap_or_default();
        let old = accts.dict(&aid).unwrap_or_default();
        if matches!(old.get("auto"), Some(V::Bool(false))) {
            new.set("auto", false); // 用户关掉过的自动打开不改回来
        }
        if spec != "system" {
            new.set("proxy", spec.as_str());
        }
        accts.set(aid, new);
        m.set("accounts", accts);
        c.set("mail", m);
    }
    cfg_save(&c)?;
    // 换了地址：旧地址没人再用了就删钥匙串里的应用专用密码（原来是两个集合相减，先后不定；这里按地址排好）
    let after = mail_all_addrs(&c);
    let mut gone: Vec<&String> = before.iter().filter(|a| !after.contains(a) && **a != addr).collect();
    gone.sort();
    for a in gone {
        ks_del(MAIL_SVC, Some(a))?;
    }
    let listed = places.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join("、");
    if acct.is_empty() {
        err!(
            "✓ 已登录 {}，看这些邮件夹：{}\n✓ 已保存：应用专用密码在{}里，设置在 {} 的 mail 一项。之后在 cca 里登录 Claude 账号时，\
             它的登录邮件一到就自动在它专属的浏览器里打开（cca login、第一次启动账号、网页版里的登录）；别的时候用 cca mail open 账号",
            addr,
            listed,
            keystore_name(),
            *CFG
        );
    } else {
        err!(
            "✓ 已登录 {}，看这些邮件夹：{}\n✓ 已保存：{} 单独用这个 Gmail 收登录邮件，应用专用密码在{}里，设置在 {} 的 mail.accounts 一项",
            addr,
            listed,
            name_of(acct),
            keystore_name(),
            *CFG
        );
    }
    Ok(())
}

/// `time.strftime('%m-%d %H:%M', time.localtime(at))`，at 是 last.json 里的整数（布尔值在 Python 里也是整数）。
/// 大得出奇的原来是 time.localtime 里没人接的 OverflowError / OSError
fn last_time(at: &V) -> R<String> {
    let secs = match at {
        V::Bool(b) => i64::from(*b),
        V::Int(i) => *i,
        _ => return Err(Fail::Other("OverflowError", vec!["timestamp out of range for platform time_t".into()])),
    };
    time::localtime(0.0); // 先让 C 库读好时区（py::time 里只做一次）
                          // SAFETY: tm 是纯数据，全 0 是合法的值；两个指针都指向有效的内存
    let tm = unsafe {
        let mut out: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&(secs as _), &mut out).is_null() {
            // 年份放不进 C 的 int：time.localtime 是 OSError
            let e = std::io::Error::last_os_error();
            return Err(Fail::Os(if e.raw_os_error().unwrap_or(0) == 0 {
                std::io::Error::from_raw_os_error(libc::EINVAL)
            } else {
                e
            }));
        }
        out
    };
    let tm = time::Tm {
        year: tm.tm_year + 1900,
        mon: tm.tm_mon + 1,
        mday: tm.tm_mday,
        hour: tm.tm_hour,
        min: tm.tm_min,
        sec: tm.tm_sec,
        wday: (tm.tm_wday + 6) % 7,
        yday: tm.tm_yday + 1,
        isdst: tm.tm_isdst,
    };
    Ok(time::strftime("%m-%d %H:%M", &tm))
}

pub fn cmd_mail_status(args: &[String]) -> R<()> {
    arity("mail-status", args, 0, 0)?;
    let c = cfg_load()?;
    let m = mail_cfg(&c, None);
    let accts = c.dict("mail").and_then(|mm| mm.dict("accounts")).unwrap_or_default();
    if m.is_none() && accts.is_empty() {
        out!("还没配置邮箱助手：运行 cca mail setup（Claude 账号的登录邮件转发到一个 Gmail，登录时 cca 在账号专属的浏览器里打开邮件里的登录链接）");
        return Ok(());
    }
    let stored = |user: &str| -> String {
        let (rc, pw) = ks_read(MAIL_SVC, user);
        format!("{}（{}）", user, if pw.is_empty() { mail_pw_problem(rc) } else { format!("应用专用密码在{}里", keystore_name()) })
    };
    let mut rows: Vec<(String, String)> = Vec::new();
    if let Some(m) = &m {
        let (user, proxy) = (m.at("user").py_str(), m.at("proxy").py_str());
        let mailbox = stored(&user);
        let route = match mail_route(Some(&proxy), &c, None) {
            Ok(r) => format!("{}{}", mail_route_note(&r), if proxy == "system" { "（照系统代理）" } else { "" }),
            Err(e) if e.is("MailFatal") => mail_err(&e).map(|x| x.1).unwrap_or_default(),
            Err(e) => return Err(e),
        };
        rows.push(("邮箱".into(), mailbox));
        rows.push(("连接方式".into(), route));
        rows.push((
            "登录时".into(),
            if m.at("auto").truthy() {
                "自动打开登录邮件里的链接（cca login、第一次启动账号、网页版里的登录、cca login-page）"
            } else {
                "不自动打开（config.json 的 mail.auto 是 false）：用 cca mail open 账号"
            }
            .into(),
        ));
    }
    // 信任的转发中转（不安全）是谁定的：Team 给的 Claude 账号照 Team 的名单（一个 Team 一行），这台设备给它们登记的不算数；别的照这台设备登记的
    let mut fwd_rows: Vec<(String, String)> = Vec::new();
    for g in grants().iter() {
        let list = doc_forwarders(&g.doc);
        if !list.is_empty() && policy_sources(t()).values().iter().any(|v| v.as_str() == Some(g.id.as_str())) {
            fwd_rows.push((
                "信任中转".into(),
                format!("「{}」给的账号：{}（不安全；照 Team 的配置，这台设备上改不了）", g.name, list.join("、")),
            ));
        }
    }
    if !accts.is_empty() {
        let mut names: HashMap<String, (String, String)> = HashMap::new();
        for k in table_load(t())?.keys() {
            names.insert(acct_id(&k, t()), (name_of(&k), parse_key(&k).1));
        }
        let mut ids = accts.keys();
        ids.sort();
        for aid in ids {
            let (name, email) = names.get(&*aid).cloned().unwrap_or_default();
            let name = if name.is_empty() { aid.to_string() } else { name };
            if let Some(one) = mail_one(&accts.at(&aid), true, "system") {
                rows.push((format!("账号 {name}"), stored(&one.at("user").py_str())));
            }
            let Some(d) = accts.dict(&aid).and_then(|a| a.str("forwarder")).map(|d| d.to_lowercase()) else { continue };
            let said = match team_fwd_of(&email) {
                Some(f) => format!("{name}：这台设备登记的 {d} 不算数（照「{}」的配置）", f.team),
                None => format!("{name}：{d}（不安全；这台设备登记的）"),
            };
            fwd_rows.push(("信任中转".into(), said));
        }
    }
    rows.extend(fwd_rows);
    if let Some(V::Dict(last)) = fs::load_quiet(&path::join(&MAIL_DIR, "last.json")) {
        if let (Some(at), Some(text)) = (last.get("at").filter(|at| at.isinstance_int()), last.str("text")) {
            let name = last.get("name").filter(|n| n.truthy()).map(|n| n.py_str()).unwrap_or_default();
            rows.push(("上次".into(), format!("{} {}：{}", last_time(&at)?, name, text)));
        }
    }
    out!("邮箱助手");
    for (k, v) in rows {
        out!("  {}{}{}", k, " ".repeat(10usize.saturating_sub(uni::width(&k))), v);
    }
    Ok(())
}

pub fn cmd_mail_ready(args: &[String]) -> R<()> {
    arity("mail-ready", args, 0, 1)?;
    mail_ready(None, Some(arg(args, 0, "")).filter(|k| !k.is_empty())).map(|_| ())
}

/// 配置过的 Gmail 地址（cca mail setup 重新配置时当默认值）；没配置是空行
pub fn cmd_mail_get_user(args: &[String]) -> R<()> {
    arity("mail-get-user", args, 0, 0)?;
    out!("{}", mail_cfg(&cfg_load()?, None).map(|m| m.at("user").py_str()).unwrap_or_default());
    Ok(())
}

/// 移除邮箱助手设置（没配置过也照样能跑）：
/// 无参数 = 全部（默认 + 所有账号覆盖 + 钥匙串里所有地址 + ~/.cca/mail；cca mail forget、cca restore）；
/// default = 只移默认那一项（账号覆盖保留）；account 键 = 只移这个账号的覆盖。
/// 地址没人再用了才删钥匙串里的应用专用密码
pub fn cmd_mail_forget(args: &[String]) -> R<()> {
    arity("mail-forget", args, 0, 2)?;
    let (mode, key) = (arg(args, 0, ""), arg(args, 1, ""));
    let c = cfg_load()?;
    if matches!(mode, "default" | "account") {
        let before = mail_all_addrs(&c);
        if let Some(m) = c.dict("mail") {
            if mode == "account" {
                if let Some(accts) = m.dict("accounts").filter(|a| !a.is_empty()) {
                    accts.pop(&acct_id(key, t()));
                }
            } else {
                for kk in ["user", "auto", "proxy"] {
                    m.pop(kk);
                }
            }
            if !m.dict("accounts").is_some_and(|a| !a.is_empty()) {
                m.pop("accounts");
            }
            if m.is_empty() {
                c.pop("mail");
            }
            cfg_save(&c)?;
        }
        // 原来是两个集合相减，先后不定；这里按地址排好再删
        let after = mail_all_addrs(&c);
        let mut gone: Vec<&String> = before.iter().filter(|a| !after.contains(a)).collect();
        gone.sort();
        for a in gone {
            ks_del(MAIL_SVC, Some(a))?;
        }
        return Ok(());
    }
    ks_del(MAIL_SVC, None)?; // 全部：服务名下所有地址
    if c.has("mail") {
        c.pop("mail");
        cfg_save(&c)?;
    }
    let _ = fs::rmtree(&MAIL_DIR);
    Ok(())
}

/// cca mail trust-forwarder 账号 域名|off：给这个账号登记（或关闭）「信任转发中转」（不安全、默认不登记）。
/// 存 config.json 的 mail.accounts[acct_id].forwarder。Team 给的 Claude 账号信任哪些中转照 Team 的（mail_forwarders），这里登记不了：
/// stdout 一行「Team 的编号\tTeam 的名字\t名单（逗号连起来，可以是空的）」、退出码 3（off 照样能清掉这台设备上以前登记的）。
/// 账号 --check：只看是不是这样（是：同上；不是：什么都不打、退出码 0）。退出码：0 好了；2 域名不对；3 Team 给的账号
pub fn cmd_mail_trust_forwarder(args: &[String]) -> R<()> {
    arity("mail-trust-forwarder", args, 2, 2)?;
    let (key, domain) = (&args[0], &args[1]);
    let aid = acct_id(key, t());
    let off = matches!(domain.as_str(), "off" | "");
    if !off {
        if let Some(f) = team_fwd_of(&parse_key(key).1) {
            out!("{}\t{}\t{}", f.tid, f.team, f.domains.join(","));
            return exit(3);
        }
        if domain == "--check" {
            return Ok(());
        }
    }
    let d = text::strip(domain).to_lowercase();
    if !off && !MAIL_DOMAIN_RE.matches(&d) {
        err!("! 中转域名写成 relay.example.com 这样（不带 @、http://、端口和空格）");
        return exit(2);
    }
    let c = cfg_load()?;
    let m = c.dict("mail").unwrap_or_default();
    let accts = m.dict("accounts").unwrap_or_default();
    let cur = accts.dict(&aid).unwrap_or_default();
    if off {
        cur.pop("forwarder");
    } else {
        cur.set("forwarder", d);
    }
    if cur.is_empty() {
        // 这个账号的覆盖空了：清掉
        accts.pop(&aid);
        if accts.is_empty() {
            m.pop("accounts");
        }
        if m.is_empty() {
            c.pop("mail");
        } else {
            c.set("mail", m);
        }
    } else {
        accts.set(aid, cur);
        m.set("accounts", accts);
        c.set("mail", m);
    }
    cfg_save(&c)
}

/// 一封邮件是哪个账号的：收件人是账号表里的邮箱（严格），或者 From 的显示名是一个信任转发中转的账号的邮箱、From 的域名是它信任的
/// 中转之一（信任中转路由）。返回 (To, From, 账号的键, 这个账号信任的中转：mail_forwarders)
#[allow(clippy::type_complexity)]
fn whose(msg: &Message, names: &HashMap<String, String>, c: &Dict) -> R<(Vec<String>, Vec<String>, Option<String>, Vec<String>)> {
    let to = mail_addrs(msg, "To")?;
    let frm = mail_addrs(msg, "From")?;
    let mut acctkey = to.iter().find_map(|a| names.get(a)).cloned();
    let mut rfwds = acctkey.as_deref().map(|k| mail_forwarders(c, k)).unwrap_or_default();
    if acctkey.is_none() {
        let cand = names.get(&mail_from_alias(msg)?).cloned();
        let cfwds = cand.as_deref().map(|k| mail_forwarders(c, k)).unwrap_or_default();
        if via_forwarder(&cfwds, &frm) {
            (acctkey, rfwds) = (cand, cfwds);
        }
    }
    Ok((to, frm, acctkey, rfwds))
}

/// From 的域名落在 rfwds 里的一个中转下面
fn via_forwarder(rfwds: &[String], frm: &[String]) -> bool {
    frm.first().is_some_and(|first| rfwds.iter().any(|f| mail_fwd_ok(domain_of(first), f)))
}

/// 核对过了的那条规则（没有规则却通过了：原来是往下用 None 的 AttributeError，实际走不到）
fn passed(rule: Option<&'static MailRule>) -> R<&'static MailRule> {
    rule.ok_or_else(|| Fail::Other("AttributeError", vec!["'NoneType' object has no attribute 'subject_ok'".into()]))
}

/// 一封候选的邮件：(Gmail 收到的时间, UID, 给人看的邮件夹名, IMAP 里的邮件夹名, UIDVALIDITY, 头)
type Row = (i64, u64, String, String, Option<u64>, Vec<u8>);

/// 各个邮件夹里最近这段时间 Anthropic 发来的（或者标题像登录邮件的），新的在前
fn recent(bx: &mut MailBox, places: &[(String, String)], rules: &[&'static MailRule], since: f64) -> R<Vec<Row>> {
    let mut rows: Vec<Row> = Vec::new();
    for (label, name) in places {
        let (valid, _) = bx.examine(name)?;
        let hits = bx.search(&mail_crit(rules, since))?;
        for (u, (got, raw)) in bx.headers(&Uids::List(hits))? {
            if got as f64 >= since {
                rows.push((got, u, label.clone(), name.clone(), valid, raw));
            }
        }
    }
    // rows.sort(reverse=True)：时间、UID、邮件夹名
    rows.sort_by(|a, b| (b.0, b.1, &b.2).cmp(&(a.0, a.1, &a.2)));
    Ok(rows)
}

/// cca mail test [小时]：用存着的设置连一次 Gmail，逐封说最近这么多小时里 Anthropic 发来的（或者标题像登录邮件的）邮件：
/// 收到的时间、标题、发件人、收件人、Gmail 的验证结果、核对的结果和链接的样子（不打出链接本身）。只读：不标已读
pub fn cmd_mail_test(args: &[String]) -> R<()> {
    arity("mail-test", args, 0, 1)?;
    let (m, pw, route) = mail_ready(None, None)?;
    let Some(h) = clamp_int(arg(args, 0, "24"), 1, 336) else {
        return die("用法: cca mail test [小时]");
    };
    let rules = mail_rules();
    let c = cfg_load()?;
    let names = accounts_by_email()?;
    let since = time::time() - (h * 3600) as f64;
    err!("正在连接 {}（{}）…", MAIL_HOST, mail_route_note(&route));
    let user = m.at("user").py_str();
    let mut bx = MailBox::new(&user, &pw, &route);
    let run = (|| -> R<()> {
        bx.open()?;
        let places = bx.folders()?;
        out!("✓ 已登录 {}，看这些邮件夹：{}", user, places.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join("、"));
        let rows = recent(&mut bx, &places, &rules, since)?;
        let tail = if rows.len() > 20 {
            "，下面是最近的 20 封："
        } else if rows.is_empty() {
            ""
        } else {
            "："
        };
        out!("最近 {} 小时里 Anthropic 发来的（或者标题像登录邮件的）有 {} 封{}", h, rows.len(), tail);
        // 标题、地址、链接的主机都是发件人写的
        let show = |s: String| out!("{}", mail_clean(&s));
        for (got, u, label, name, _, raw) in rows.iter().take(20) {
            let msg = mail_parse(raw)?;
            let (to, frm, acctkey, rfwds) = whose(&msg, &names, &c)?;
            let via = via_forwarder(&rfwds, &frm);
            let acct = match &acctkey {
                Some(k) => parse_key(k).1.to_lowercase(),
                None => to.first().cloned().unwrap_or_default(),
            };
            let got = *got as f64;
            let (rule, why) = mail_judge_any(&rules, &msg, got, &acct, since, &rfwds)?;
            let ar = mail_ar(&msg);
            let subject = mail_text(msg.get("Subject"));
            let subject = text::head(&subject, 80);
            show(format!(
                "  {}  {}  {}",
                time::strftime_local("%m-%d %H:%M", got),
                label,
                if subject.is_empty() { "（没有标题）" } else { subject }
            ));
            let join = |v: &[String]| Some(v.join("、")).filter(|j| !j.is_empty()).unwrap_or_else(|| "？".to_string());
            let who = match &acctkey {
                Some(k) => format!("（账号 {}{}）", name_of(k), if via { "，经可信中转" } else { "" }),
                None => "（不是账号表里的邮箱）".to_string(),
            };
            show(format!("      发件人 {} → 收件人 {}{}", join(&frm), join(&to), who));
            let checked = match &ar {
                None => "不是 Gmail 写的验证结果".to_string(),
                Some(ar) if ar.is_empty() => "没有 DKIM 签名".to_string(),
                Some(ar) => ar
                    .iter()
                    .map(|(r, d, _)| format!("dkim={} {}", r, if d.is_empty() { "？" } else { d }))
                    .collect::<Vec<_>>()
                    .join("、"),
            };
            show(format!("      Gmail 的验证：{checked}"));
            if let Some(why) = why {
                out!("      ✗ 没通过：{}", mail_why(why).unwrap_or(why));
                continue;
            }
            let rule = passed(rule)?;
            bx.examine(name)?;
            let (why, link, full) = mail_verdict(rule, &bx.message(*u)?, got, &acct, since, &rfwds)?;
            if let Some(link) = link.filter(|l| !l.is_empty()) {
                out!("      ✓ 通过核对，链接 {}", mail_shape(&link));
            } else if let Some(why) = why {
                out!("      ✗ 没通过：{}", mail_why(why).unwrap_or(why));
            } else {
                let mut hosts: Vec<String> = mail_links(&full)?
                    .iter()
                    .map(|l| urlsplit_host(l))
                    .map(|h| if h.is_empty() { "？".to_string() } else { h })
                    .collect();
                hosts.sort();
                hosts.dedup();
                let found = if hosts.is_empty() {
                    String::new()
                } else {
                    format!("（里面的链接在这些主机上：{}）", hosts.join("、"))
                };
                show(format!("      ✗ 没通过：邮件里没有{}{}", rule.what, found));
            }
        }
        Ok(())
    })();
    bx.close(true);
    match run {
        Err(e) => match mail_err(&e) {
            Some((_, text)) => die(text),
            None => Err(e),
        },
        ok => ok,
    }
}

/// 打开 App 里点中的这一封登录邮件：取它的头定出规则和收到的时间，再核对、取链接、打开。
/// 返回 (code, text)，code：opened | used | gone | error | fail | refused | 没通过核对的原因。用户明确点开这一封，
/// 所以不按「这次登录开始以后」卡（old 检查只防自动打开旧邮件），核对靠 DKIM + 收件人 + 标题（开了信任中转的账号另走那条）
fn mail_open_one(src: &mut MailSource<MailBox>, h: &Hunt, handle: &Handle, deliver: Deliver) -> R<(String, String)> {
    let _frame = super::py_frame();
    let gone = |text: &str| Ok(("gone".to_string(), text.to_string()));
    let run = (|| -> R<(String, String)> {
        src.bx.open()?;
        let (valid, _) = src.bx.examine(&handle.folder)?;
        if handle.valid.text != "0" && valid.is_some() && valid != handle.valid.n {
            return gone("邮件夹变动了");
        }
        let hdrs = src.bx.headers(&Uids::Text(handle.uid.text.clone()))?;
        let Some((uid, (got, hdr))) = handle.uid.n.and_then(|u| hdrs.get(&u).map(|x| (u, x))) else {
            return gone("邮件不在了");
        };
        let got = *got as f64;
        let msg = mail_parse(hdr)?;
        let (rule, why) = mail_judge_any(h.rules, &msg, got, h.acct, got, h.forwarders)?;
        if let Some(why) = why {
            return Ok((why.to_string(), mail_why(why).unwrap_or("").to_string()));
        }
        let cand = Cand { folder: handle.folder.clone(), label: String::new(), uid, got, hdr: Vec::new(), valid: None };
        let p = Pend::new(cand, passed(rule)?, mail_digest(&msg, &handle.folder, uid, got));
        let st = mail_take(h, src, &p, got, deliver)?;
        let text = mail_why(&st).unwrap_or("").to_string();
        Ok((if st == "ok" { "opened".to_string() } else { st }, text))
    })();
    match run {
        Err(e) => match mail_err(&e) {
            Some((_, text)) => Ok(("error".to_string(), text)),
            None => Err(e),
        },
        ok => ok,
    }
}

/// `try: … except KeyboardInterrupt: 已取消、退出码 130 … finally: src.close()`：先说、再关、再退出
fn finish<T>(res: R<T>, src: &mut MailSource<MailBox>) -> R<T> {
    if matches!(&res, Err(e) if e.is("KeyboardInterrupt")) {
        err!("已取消");
        src.close();
        return exit(130);
    }
    src.close();
    res
}

/// cca mail open：这个账号最新的一封登录邮件（最近半小时），交给 cca _mail-open 在账号专属的浏览器里打开；没有就最多等 secs 秒。
/// --msg 文件夹:uid:uidvalidity 时打开指定的那一封（App 里点列表的一封，不等待）。退出码：0 打开了；1 没有（原因打到 stderr）；130 取消
pub fn cmd_mail_open(args: &[String]) -> R<()> {
    arity("mail-open", args, 1, 4)?;
    let key = args[0].as_str();
    let (secs, again, msg) = (arg(args, 1, "90"), arg(args, 2, "") == "again", arg(args, 3, ""));
    let (m, pw, route) = mail_ready(None, Some(key))?;
    let acct = parse_key(key).1.to_lowercase();
    if acct.is_empty() {
        return die(format!("账号表里 {} 没写邮箱：认不出哪封登录邮件是它的", name_of(key)));
    }
    let rules = mail_rules();
    let fwds = mail_forwarders(&cfg_load()?, key);
    let mut src = MailSource::new(MailBox::new(&m.at("user").py_str(), &pw, &route), &rules);
    let say = |s: &str| err!("{s}");
    let (now, sleep) = (|| time::time(), |s: f64| sleep_interruptible(s));
    let (poll, base) = Hunt::knobs();
    let h = Hunt {
        dir: &MAIL_DIR,
        rules: &rules,
        acct: &acct,
        forwarders: &fwds,
        again,
        once: true,
        now: &now,
        sleep: &sleep,
        pid: std::process::id(),
        poll,
        base,
    };
    let mut deliver = mail_deliver(key);
    let opened = || mail_last(&name_of(key), "打开了登录邮件里的链接（cca mail open）");
    if !msg.is_empty() {
        let Some(handle) = mail_handle(msg) else {
            src.close();
            return die("--msg 要写成 文件夹:uid:uidvalidity");
        };
        catch_sigint();
        let res = mail_open_one(&mut src, &h, &handle, &mut deliver);
        let (code, text) = finish(res, &mut src)?;
        return match code.as_str() {
            "opened" => opened(),
            "used" => die("这封登录邮件已经打开过了：加 --again 可以再打开一次"),
            "gone" => die("这封邮件已经不在了（被移动或删除了？）：刷新列表再试"),
            "error" => die(format!("邮箱助手：{text}")),
            "fail" => die("收到登录邮件了，但没能打开浏览器"),
            other => die(format!("这封没通过核对：{}", mail_why(other).unwrap_or(other))),
        };
    }
    let Some(secs) = clamp_int(secs, 0, 900) else {
        src.close();
        return die("--wait 后面要写秒数");
    };
    let t0 = time::time();
    let waiting = || say(&format!("• 等 {acct} 的登录邮件（最多 {secs} 秒，Ctrl-C 取消）…"));
    let idle: Option<&dyn Fn()> = if secs == 0 { None } else { Some(&waiting) };
    catch_sigint();
    let res = mail_hunt(&h, &mut src, t0 - MAIL_RECENT, t0 + secs as f64, &mut deliver, &|| true, &say, None, idle);
    let res: Found = finish(res, &mut src)?;
    if res.code == "opened" {
        return opened();
    }
    if !matches!(res.code.as_str(), "none" | "fail") {
        return die(format!("邮箱助手：{}", res.text));
    }
    let mut msg = if res.code == "fail" {
        "收到登录邮件了，但没能打开浏览器（原因见上面）".to_string()
    } else if res.used > 0 {
        "最近的登录邮件已经打开过了：等新的一封，或者加 --again 再打开一次".to_string()
    } else {
        format!(
            "最近 30 分钟里没有 {} 的登录邮件：在登录页上用邮箱登录（Anthropic 会发来登录邮件），收到后再运行 cca mail open {}（--wait 秒数 可以多等一会儿）",
            acct,
            name_of(key)
        )
    };
    let rej: Vec<&(String, u32)> = res.rejected.iter().filter(|(k, _)| !matches!(k.as_str(), "old" | "subject")).collect();
    if !rej.is_empty() {
        let count: u32 = rej.iter().map(|(_, n)| n).sum();
        let whys = rej.iter().map(|(k, _)| mail_why(k).unwrap_or(k)).collect::<Vec<_>>().join("；");
        msg.push_str(&format!("；另有 {count} 封没通过核对（{whys}），运行 cca mail test 看详情"));
    }
    die(msg)
}

/// 不是负的整数写进 JSON（比 i64 大的照样是数）
fn uint(n: u64) -> V {
    match i64::try_from(n) {
        Ok(i) => V::Int(i),
        Err(_) => V::Big(n.to_string().into()),
    }
}

/// cca _gui mail-list [账号的键] [小时]：图形界面按需拉取的登录邮件列表（账号页展开/刷新时跑，像 _gui live 一样实时连，CCA_RO=1）。
/// acct 给定时连这个账号解析到的 Gmail、只留发给它的；为空时连默认 Gmail、跨账号列。只读；绝不含登录链接本身（只给样子）、不改邮箱。
/// 一行 JSON：{version,kind,generated_at,user,account,hours,folders,rows,truncated,error}。每封：got_epoch、folder、subject、from、to、
/// account、dkim、verdict{ok,why,why_text}、link_shape、openable、msg{folder,uid,uidvalidity}（能打开时才有句柄）
pub fn cmd_gui_mail_list(args: &[String]) -> R<()> {
    arity("gui-mail-list", args, 0, 2)?;
    let acct = arg(args, 0, "");
    let key = Some(acct).filter(|a| !a.is_empty());
    let h = clamp_int(arg(args, 1, "24"), 1, 336).unwrap_or(24);
    let base = dict! {
        "version" => 1, "kind" => "mail-list", "generated_at" => time::time().trunc() as i64, "hours" => h,
        "account" => key.map(name_of), "key" => key, "user" => V::None,
        "folders" => V::List(Default::default()), "rows" => V::List(Default::default()), "truncated" => false, "error" => V::None,
    };
    let done = |base: &Dict| -> R<()> {
        out!("{}", json::dumps(&V::Dict(base.clone()), json::LINE));
        Ok(())
    };
    let c = cfg_load()?;
    let Some(m) = mail_cfg(&c, key) else {
        base.set("error", dict! { "code" => "not_configured", "message" => "还没配置邮箱助手" });
        return done(&base);
    };
    let user = m.at("user").py_str();
    base.set("user", user.as_str());
    let (rc, pw) = ks_read(MAIL_SVC, &user);
    if pw.is_empty() {
        base.set("error", dict! { "code" => "no_password", "message" => mail_pw_problem(rc) });
        return done(&base);
    }
    let route = match mail_route(Some(&m.at("proxy").py_str()), &c, None) {
        Ok(r) => r,
        Err(e) if e.is("MailFatal") => {
            base.set("error", dict! { "code" => "route", "message" => mail_err(&e).map(|x| x.1).unwrap_or_default() });
            return done(&base);
        }
        Err(e) => return Err(e),
    };
    let rules = mail_rules();
    let names = accounts_by_email()?;
    let want = key.map(|a| parse_key(a).1.to_lowercase()).filter(|w| !w.is_empty());
    let since = time::time() - (h * 3600) as f64;
    let mut bx = MailBox::new(&user, &pw, &route);
    let run = (|| -> R<()> {
        bx.open()?;
        let places = bx.folders()?;
        base.set("folders", V::from(places.iter().map(|(n, _)| V::from(n.as_str())).collect::<Vec<_>>()));
        let found = recent(&mut bx, &places, &rules, since)?;
        base.set("truncated", found.len() > 50);
        let mut rows: Vec<V> = Vec::new();
        for (got, u, label, name, valid, raw) in found.iter().take(50) {
            let msg = mail_parse(raw)?;
            let (to, frm, acctkey, rfwds) = whose(&msg, &names, &c)?;
            let tgt = acctkey.as_deref().map(|k| parse_key(k).1.to_lowercase());
            // 账号作用域：只留发给这个账号的
            if want.as_ref().is_some_and(|w| tgt.as_ref() != Some(w)) {
                continue;
            }
            let judge = tgt.filter(|t| !t.is_empty()).or_else(|| to.first().cloned()).unwrap_or_default();
            let (rule, why) = mail_judge_any(&rules, &msg, *got as f64, &judge, since, &rfwds)?;
            let ar = mail_ar(&msg);
            let via = via_forwarder(&rfwds, &frm).then_some("forwarder");
            let strs = |v: &[String]| V::from(v.iter().map(|s| V::from(s.as_str())).collect::<Vec<_>>());
            let row = dict! {
                "got_epoch" => *got, "folder" => label.as_str(), "subject" => mail_text(msg.get("Subject")),
                "from" => strs(&frm), "to" => strs(&to),
                "account" => acctkey.as_deref().map(|k| dict! { "key" => k, "name" => name_of(k) }),
                "dkim" => ar.map(|ar| V::from(ar.iter().map(|(r, d, _)| V::from(dict! { "result" => r.as_str(), "domain" => d.as_str() })).collect::<Vec<_>>())),
                "verdict" => dict! { "ok" => why.is_none(), "why" => why, "why_text" => why.map(|w| mail_why(w).unwrap_or(w)) },
                "link_shape" => V::None, "openable" => false, "via" => V::None, "msg" => V::None,
            };
            if why.is_none() {
                bx.examine(name)?;
                let (why2, link, _) = mail_verdict(passed(rule)?, &bx.message(*u)?, *got as f64, &judge, since, &rfwds)?;
                match link.filter(|l| !l.is_empty()) {
                    Some(link) => {
                        row.set("link_shape", mail_shape(&link));
                        row.set("openable", acctkey.is_some());
                        row.set("via", via); // 经可信中转认的（不安全）：App 标注；严格 Anthropic 的是 null
                        row.set(
                            "msg",
                            dict! { "folder" => name.as_str(), "uid" => uint(*u), "uidvalidity" => valid.map_or(V::Int(0), uint) },
                        );
                    }
                    None => {
                        let w = why2.unwrap_or("nolink");
                        row.set("verdict", dict! { "ok" => false, "why" => w, "why_text" => mail_why(w).unwrap_or(w) });
                    }
                }
            }
            rows.push(V::Dict(row));
        }
        base.set("rows", V::from(rows));
        Ok(())
    })();
    bx.close(true);
    if let Err(e) = run {
        match mail_err(&e) {
            Some((_, text)) => base.set("error", dict! { "code" => "connect", "message" => text }),
            None => return Err(e),
        }
    }
    done(&base)
}

/// 登录结束：马上走（正在打开的 cca _mail-open 是另一个进程，照样做完）
extern "C" fn leave_now(_: libc::c_int) {
    // SAFETY: _exit 可以在信号处理函数里调
    unsafe { libc::_exit(0) }
}

/// 登录时在后台盯邮箱（bash 的 mail_watch_start 起它，登录结束时 TERM；cca 不在了、盯够 15 分钟也停）：这个账号的登录邮件一到
/// 就交给 cca _mail-open。没配置、关掉了自动、账号表里没写邮箱时什么都不做；出什么错都只说一句，退出码总是 0，绝不影响登录
pub fn cmd_mail_watch(args: &[String]) -> R<()> {
    arity("mail-watch", args, 1, 1)?;
    // SAFETY: 处理函数只调 _exit；Ctrl-C 是给登录的：cca 退出以后这里看父进程不在了自己走
    let parent = unsafe {
        libc::signal(libc::SIGTERM, leave_now as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::getppid()
    };
    // SAFETY: getppid 只是问一下
    let alive = || unsafe { libc::getppid() } == parent;
    match mail_watch(&args[0], &alive, MAIL_WATCH_MAX) {
        Ok(()) | Err(Fail::Exit(_)) => {}
        // die() 是先说了再退出的
        Err(Fail::Die(said)) => err!("{said}"),
        Err(e) => match mail_err(&e) {
            Some((_, text)) => err!("! 邮箱助手：{text}，这次不看邮箱（登录照常进行）"),
            // 绝不让登录跟着出错：只说一句
            None => err!("! 邮箱助手出错了（{}），这次不看邮箱（登录照常进行）", class_name(&e)),
        },
    }
    exit(0)
}

fn mail_watch(key: &str, alive: &dyn Fn() -> bool, limit: f64) -> R<()> {
    let _frame = super::py_frame();
    let c = cfg_load()?;
    let m = mail_cfg(&c, Some(key));
    let acct = parse_key(key).1.to_lowercase();
    let rules = mail_rules();
    let Some(m) = m.filter(|m| m.at("auto").truthy() && !rules.is_empty() && !acct.is_empty()) else {
        return Ok(());
    };
    let user = m.at("user").py_str();
    let (rc, pw) = ks_read(MAIL_SVC, &user);
    if pw.is_empty() {
        return Err(mail_fatal("keychain", mail_pw_problem(rc)));
    }
    let route = mail_route(Some(&m.at("proxy").py_str()), &c, None)?;
    let (name, start) = (name_of(key), time::time());
    let mut src = MailSource::new(MailBox::new(&user, &pw, &route), &rules);
    let say = |s: &str| err!("{s}");
    let hello = || {
        say(&format!("• 邮箱助手：已连上 {}（{}），{} 的登录邮件一到就在它专属的浏览器里打开", user, mail_route_note(&route), name))
    };
    let fwds = mail_forwarders(&c, key);
    let (now, sleep) = (|| time::time(), |s: f64| sleep_interruptible(s));
    let (poll, base) = Hunt::knobs();
    let h = Hunt {
        dir: &MAIL_DIR,
        rules: &rules,
        acct: &acct,
        forwarders: &fwds,
        again: false,
        once: false,
        now: &now,
        sleep: &sleep,
        pid: std::process::id(),
        poll,
        base,
    };
    let mut deliver = mail_deliver(key);
    let res = mail_hunt(&h, &mut src, start, start + limit, &mut deliver, alive, &say, Some(&hello), None);
    src.close();
    watch_report(&res?, &name, alive, limit, &say, &|text| mail_last(&name, text))
}

/// 盯完以后：记下上次的结果（last：`mail_last(name, 说明)`），没等到、连不上的说一句
fn watch_report(
    res: &Found,
    name: &str,
    alive: &dyn Fn() -> bool,
    limit: f64,
    say: &dyn Fn(&str),
    last: &dyn Fn(&str) -> R<()>,
) -> R<()> {
    let minutes = ((limit / 60.0).floor() as i64).max(1);
    if res.code == "opened" {
        last("打开了登录邮件里的链接")?;
    } else if !matches!(res.code.as_str(), "none" | "fail") {
        last(&res.text)?;
        say(&format!("! 邮箱助手：{}，不再看邮箱（登录照常进行）", res.text));
    } else if alive() {
        last(&format!("{minutes} 分钟里没等到登录邮件"))?;
        say(&format!("• 邮箱助手：{minutes} 分钟里没等到 {name} 的登录邮件，不再看了"));
    }
    Ok(())
}

/// cca _mail-open 的核对：stdin 的链接是不是这个工具的某一条规则认的登录链接（不对时往 stderr 说，不打出链接）
pub fn cmd_mail_link_check(args: &[String]) -> R<()> {
    arity("mail-link-check", args, 0, 0)?;
    let line = read_stdin_line();
    let url = text::strip(&line);
    if !mail_rules().iter().any(|r| r.link_ok(url)) {
        return die("邮箱助手给的链接不是登录链接，没有打开");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn python_int() {
        let v = |s: &str| py_int(s).map(|(neg, d)| format!("{}{}", if neg { "-" } else { "" }, d));
        assert_eq!(v("24").as_deref(), Some("24"));
        assert_eq!(v(" +007\n").as_deref(), Some("7"));
        assert_eq!(v("-0").as_deref(), Some("0"));
        assert_eq!(v("1_000").as_deref(), Some("1000"));
        assert_eq!(v("\u{663}\u{661}").as_deref(), Some("31"), "别的文字的十进制数字 int() 也认");
        assert_eq!(v(&"9".repeat(4300)).map(|d| d.len()), Some(4300));
        for bad in ["", " ", "-", "+", "1.5", "1e3", "0x10", "_1", "1_", "1__0", "- 1", "--1", "\u{b2}", "1 2", "一"] {
            assert_eq!(v(bad), None, "{bad:?}");
        }
        assert_eq!(v(&"9".repeat(4301)), None, "超过 int() 的位数上限是 ValueError");
        assert_eq!(v(&format!("{}1", "0".repeat(4300))), None, "前导 0 也算位数");
        assert_eq!(clamp_int("24", 1, 336), Some(24));
        assert_eq!(clamp_int("0", 1, 336), Some(1));
        assert_eq!(clamp_int("-5", 1, 336), Some(1));
        assert_eq!(clamp_int("-5", 0, 900), Some(0));
        assert_eq!(clamp_int("99999999999999999999999999", 1, 336), Some(336));
        assert_eq!(clamp_int("x", 1, 336), None);
    }

    #[test]
    fn handles() {
        let h = mail_handle("[Gmail]/All Mail:42:11").unwrap();
        assert_eq!((h.folder.as_str(), h.uid.n, h.valid.n, h.uid.text.as_str()), ("[Gmail]/All Mail", Some(42), Some(11), "42"));
        let h = mail_handle("a:b:007:0").unwrap();
        assert_eq!((h.folder.as_str(), h.uid.text.as_str(), h.valid.text.as_str()), ("a:b", "7", "0"));
        let h = mail_handle("INBOX:99999999999999999999999:1").unwrap();
        assert_eq!((h.uid.n, h.uid.text.as_str()), (None, "99999999999999999999999"), "放不进 64 位的照样是个数");
        for bad in [
            "",
            "INBOX",
            "INBOX:1",
            ":1:2",
            "INBOX:x:2",
            "INBOX:1:",
            "INBOX::2",
            "INBOX:-1:2",
            "INBOX:1:+2",
            "INBOX:1 :2",
            "INBOX:\u{b2}:2",
        ] {
            assert!(mail_handle(bad).is_none(), "{bad:?}");
        }
    }

    /// --msg 的句柄、小时数和 --wait 的秒数，和 Python 的 mail_handle、int() 一样
    #[test]
    fn handles_and_numbers_like_python() {
        use crate::cmd::mail::testing::vectors;
        let cases = vectors("handle");
        assert!(cases.len() > 25);
        for c in cases {
            let s = c.at("in").py_str();
            let got = mail_handle(&s).map(|h| vec![h.folder, h.uid.text, h.valid.text]);
            let want = c.list("out").map(|l| l.strs().iter().map(|x| x.to_string()).collect::<Vec<_>>());
            assert_eq!(got, want, "{s:?}");
        }
        let cases = vectors("int");
        assert!(cases.len() > 35);
        for c in cases {
            let s = c.at("in").py_str();
            assert_eq!(clamp_int(&s, 1, 336), c.at("hours").as_int(), "{s:?}");
            assert_eq!(clamp_int(&s, 0, 900), c.at("secs").as_int(), "{s:?}");
        }
    }

    #[test]
    fn exception_names() {
        let os = |n: i32| class_name(&Fail::Os(std::io::Error::from_raw_os_error(n)));
        assert_eq!(os(libc::ENOENT), "FileNotFoundError");
        assert_eq!(os(libc::EACCES), "PermissionError");
        assert_eq!(os(libc::EEXIST), "FileExistsError");
        assert_eq!(os(libc::ENOSPC), "OSError");
        assert_eq!(class_name(&Fail::Value("x".into())), "ValueError");
        assert_eq!(class_name(&Fail::Other("RecursionError", vec![])), "RecursionError");
    }

    #[test]
    fn last_time_values() {
        assert_eq!(last_time(&V::Int(1794708000)).unwrap().len(), 11);
        assert_eq!(last_time(&V::Bool(true)).unwrap(), last_time(&V::Int(1)).unwrap());
        assert!(last_time(&V::Big("99999999999999999999999999".into())).unwrap_err().is("OverflowError"));
        assert!(last_time(&V::Int(i64::MAX)).unwrap_err().is_os());
    }

    /// 盯完以后记下什么、说什么（原文在 mail_watch 的最后几行；盯够 15 分钟的那条路经子命令测等不起，这里照原文的字核对）
    #[test]
    fn watch_reports() {
        let run = |code: &str, text: &str, alive: bool, limit: f64| -> (Vec<String>, Vec<String>) {
            let (said, kept) = (RefCell::new(Vec::new()), RefCell::new(Vec::new()));
            let found = Found { code: code.into(), text: text.into(), ..Found::default() };
            let (say, last) = (
                |s: &str| said.borrow_mut().push(s.to_string()),
                |s: &str| -> R<()> {
                    kept.borrow_mut().push(s.to_string());
                    Ok(())
                },
            );
            watch_report(&found, "主号", &|| alive, limit, &say, &last).unwrap();
            (said.into_inner(), kept.into_inner())
        };
        let none: Vec<String> = Vec::new();
        assert_eq!(run("opened", "", true, 900.0), (none.clone(), vec!["打开了登录邮件里的链接".to_string()]));
        assert_eq!(
            run("net", "连不上 代理 127.0.0.1:9（Connection refused）", true, 900.0),
            (
                vec!["! 邮箱助手：连不上 代理 127.0.0.1:9（Connection refused），不再看邮箱（登录照常进行）".to_string()],
                vec!["连不上 代理 127.0.0.1:9（Connection refused）".to_string()]
            )
        );
        // 登录还在、盯够了时间：none（没等到）和 fail（浏览器没打开，已经说过了）都是这一句
        for code in ["none", "fail"] {
            assert_eq!(
                run(code, "", true, 900.0),
                (
                    vec!["• 邮箱助手：15 分钟里没等到 主号 的登录邮件，不再看了".to_string()],
                    vec!["15 分钟里没等到登录邮件".to_string()]
                )
            );
            // 登录结束了（cca 不在了）：什么都不记、不说
            assert_eq!(run(code, "", false, 900.0), (none.clone(), none.clone()));
        }
        assert_eq!(run("none", "", true, 30.0).1, ["1 分钟里没等到登录邮件"], "max(1, int(limit // 60))");
        assert_eq!(run("auth", "x", false, 900.0).1, ["x"], "连不上的不看登录还在不在");
    }
}
