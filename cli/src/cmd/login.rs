//! 登录的辅助（`login_hint_url` 到 `cmd_login_catch`）：login-url、agy-probe、agy-login、login-check、node-fp、login-page-acct、
//! login-forward、login-catch。这一块的子命令都移植了。
//!
//! - 这一份：登录网址的认法（`login_hint_url`、`login_target`、`agy_login_target`、`urlsplit_host`）、节点的摘要
//!   （`node_fp_here`），login-url、login-check、node-fp、login-page-acct
//! - `login/agy.rs`：agy 的探测和登录（`agy_probe`、`agy_remote_note`、`read_answer`、`agy_code_problem`），agy-probe、agy-login
//! - `login/forward.rs`：login-forward，和它用到的那一小部分 `http.client`
//! - `login/catch.rs`：login-catch（`login_catch_reply`、`login_catch_one`）
//! - `login/sys.rs`：原来靠 Python 运行时的几样——被信号打断（KeyboardInterrupt）、带时限的套接字、等子进程
//!
//! 登录网址、授权码、state 都是凭证一类的东西：原来不打印、不写进文件的，这里照样；交给别的进程的方式（`cca _browser 网址`
//! 的参数、agy 的 stdin、pbcopy 的 stdin）也照原样。
//!
//! 服务器上登录（bash 的 remote_login / cmd_login_page）：claude / codex 在服务器上跑登录、等本机的回调；登录页由 Mac 上的
//! cca login-page 用账号专属的 Chrome、经同一个节点打开，Mac 接住跳回 localhost 的那一下，验证码（code#state）贴回服务器，
//! 服务器核对 state 后转给本机的回调。换 token 的始终是服务器上的 claude / codex（PKCE：验证码离了它手里的 verifier 没用）

mod agy;
mod catch;
mod forward;
mod sys;

pub use agy::{cmd_agy_login, cmd_agy_probe};
pub use catch::cmd_login_catch;
pub use forward::cmd_login_forward;

use super::table::account_named;
use super::{arg, arity, Cmd};
use crate::core::base::{sh, EMAIL_RE};
use crate::core::ctx::ctx;
use crate::core::model::{cfg_load, is_api, label_of, node_of, parse_key};
use crate::core::policy::{alias_problem, fill_fp, node_ids, policy_fill, policy_map};
use crate::core::tool::{agy, t, tools, Kind};
use crate::py::obj::{Dict, V};
use crate::py::text;
use crate::py::url::{parse_qsl, quote, urlsplit};
use crate::py::{die, Fail, R};
use crate::{out, re};

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[
    Cmd::all("login-url", cmd_login_url),
    Cmd::all("agy-probe", cmd_agy_probe),
    Cmd::all("agy-login", cmd_agy_login),
    Cmd::all("login-check", cmd_login_check),
    Cmd::all("node-fp", cmd_node_fp),
    Cmd::all("login-page-acct", cmd_login_page_acct),
    Cmd::all("login-forward", cmd_login_forward),
    Cmd::all("login-catch", cmd_login_catch),
];

/// agy 的登录页登录后跳到的地方（页面上显示授权码，不回本机）
pub const AGY_REDIRECT: &str = "https://antigravity.google/oauth-callback";

/// 没人接的 TypeError：原实现走到这里是调用栈 + 退出码 1
fn type_error<T>(msg: String) -> R<T> {
    Err(Fail::Other("TypeError", vec![format!("TypeError: {msg}")]))
}

/// `type(x).__name__`
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

/// `re.match(r'^[\x21-\x7e]{1,8192}$', s)`：一行可见的 ASCII。和原来的正则一样，结尾多一个换行也算
fn visible_ascii(s: &str) -> bool {
    let body = s.strip_suffix('\n').unwrap_or(s);
    (1..=8192).contains(&body.len()) && body.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

/// `re.match(r'^https://[\x21-\x7e]{1,8192}$', url)`
fn https_url(url: &str) -> bool {
    url.strip_prefix("https://").is_some_and(visible_ascii)
}

/// `LOGIN_CODE_RE.match(s)`（`^[\x21-\x22\x24-\x7e]{1,4096}$`）：可见的 ASCII，不含 #（code#state 的分隔）。结尾多一个换行也算
fn login_code_re(s: &str) -> bool {
    let body = s.strip_suffix('\n').unwrap_or(s);
    (1..=4096).contains(&body.len()) && body.bytes().all(|b| matches!(b, 0x21..=0x22 | 0x24..=0x7e))
}

/// `dict(parse_qsl(query, keep_blank_values=True)).get(k)`：同名的参数以后面的为准
fn q_get<'a>(q: &'a [(String, String)], k: &str) -> Option<&'a str> {
    q.iter().rev().find(|(name, _)| name == k).map(|(_, v)| v.as_str())
}

/// `q.get(k)` 当真假用：没有、是空的都不算
fn q_some<'a>(q: &'a [(String, String)], k: &str) -> Option<&'a str> {
    q_get(q, k).filter(|v| !v.is_empty())
}

/// `re.sub(r'[^\x20-\x7e]', 换成什么, s)[:n]`：可见的 ASCII（和空格）以外的字符换掉（with 是 None 就去掉），截到 n 个字符
fn ascii_only(s: &str, with: Option<char>, n: usize) -> String {
    s.chars().filter_map(|c| if (' '..='~').contains(&c) { Some(c) } else { with }).take(n).collect()
}

/// `s.encode('ascii')` 不成时 UnicodeEncodeError 说的话（原来没人接：调用栈 + 退出码 1）；都是 ASCII 就是 None
fn ascii_encode_error(s: &str) -> Option<Fail> {
    let (at, ch) = s.chars().enumerate().find(|(_, c)| !c.is_ascii())?;
    let run = s.chars().skip(at).take_while(|c| !c.is_ascii()).count();
    let what = if run > 1 {
        format!("characters in position {at}-{}", at + run - 1)
    } else {
        let n = ch as u32;
        let shown = if n < 0x100 {
            format!("\\x{n:02x}")
        } else if n < 0x10000 {
            format!("\\u{n:04x}")
        } else {
            format!("\\U{n:08x}")
        };
        format!("character '{shown}' in position {at}")
    };
    Some(Fail::Value(format!("'ascii' codec can't encode {what}: ordinal not in range(128)")))
}

/// 打开的登录地址（OAuth authorize）没带 login_hint 时补上账号邮箱，登录页就会预填它。claude auth login --email
/// 自己会带，会话里执行 /login 时不带；codex、agy 生成的登录网址都不带（OpenAI 的登录页不认的话只是忽略它；Google 的
/// 登录页按它选好账号）。网址切不开（方括号不成对之类）是没人接的 ValueError
pub fn login_hint_url(key: &str, url: &str) -> R<String> {
    let e = parse_key(key).1;
    let u = urlsplit(url).map_err(Fail::Value)?;
    if !e.is_empty()
        && u.scheme == "https"
        && t().login_paths.iter().any(|p| u.path.ends_with(p))
        && u.hostname().is_some_and(|h| t().login_hosts.contains(&h.as_str()))
        && q_get(&parse_qsl(&u.query, true), "login_hint").is_none()
    {
        // urlunsplit(u._replace(query=…))：走到这里网址一定有主机（上面核对过 hostname），拼法只剩这一种。重新拼出来的
        // 和给的不一定逐字一样（协议变小写，开头的空白和哪里的制表符、换行已经去掉，空的 # 片段不留）
        let mut out = format!("{}://{}", u.scheme, u.netloc);
        if !u.path.is_empty() && !u.path.starts_with('/') {
            out.push('/');
        }
        out.push_str(&u.path);
        out.push('?');
        if !u.query.is_empty() {
            out.push_str(&u.query);
            out.push('&');
        }
        // urlencode({'login_hint': e})：邮箱里没有空格（EMAIL_RE 不收），quote_plus 就是 safe 为空的 quote
        out.push_str("login_hint=");
        out.push_str(&quote(&e, ""));
        if !u.fragment.is_empty() {
            out.push('#');
            out.push_str(&u.fragment);
        }
        return Ok(out);
    }
    Ok(url.to_string())
}

pub fn cmd_login_url(args: &[String]) -> R<()> {
    arity("cmd_login_url", args, 2, 2)?;
    out!("{}", login_hint_url(&args[0], &args[1])?);
    Ok(())
}

/// agy 的登录网址（Google 的 OAuth、带 PKCE，回到 antigravity.google 的页面显示授权码）：返回 state；不是就 None
pub fn agy_login_target(url: &str) -> Option<String> {
    if !https_url(url) {
        return None;
    }
    let u = urlsplit(url).ok()?;
    let q = parse_qsl(&u.query, true);
    let state = q_get(&q, "state").unwrap_or("");
    if !u.hostname().is_some_and(|h| agy().login_hosts.contains(&h.as_str()))
        || !agy().login_paths.contains(&u.path.as_str())
        || q_get(&q, "response_type") != Some("code")
        || q_some(&q, "code_challenge").is_none()
        || q_get(&q, "redirect_uri") != Some(AGY_REDIRECT)
        || !login_code_re(state)
    {
        return None;
    }
    Some(state.to_string())
}

/// 登录网址 → (回调的主机, 端口, 路径, state)；不是这个工具的登录页（T.login_hosts 里的 https 地址、/oauth/authorize、
/// 带 PKCE），或者回调不在本机的 http 端口上时返回 None
pub fn login_target(url: &str) -> Option<(String, u16, String, String)> {
    if !https_url(url) {
        return None;
    }
    let u = urlsplit(url).ok()?;
    let q = parse_qsl(&u.query, true);
    let r = urlsplit(q_get(&q, "redirect_uri").unwrap_or("")).ok()?;
    let port = r.port().ok()?;
    let host = r.hostname().unwrap_or_default();
    let state = q_get(&q, "state").unwrap_or("");
    if !u.hostname().is_some_and(|h| t().login_hosts.contains(&h.as_str()))
        || !u.path.ends_with("/oauth/authorize")
        || q_get(&q, "response_type") != Some("code")
        || q_some(&q, "code_challenge").is_none()
        || r.scheme != "http"
        || !matches!(host.as_str(), "localhost" | "127.0.0.1")
        || !login_code_re(state)
    {
        return None;
    }
    let port = port.filter(|p| *p != 0)?;
    Some((host, port, if r.path.is_empty() { "/".to_string() } else { r.path }, state.to_string()))
}

/// cca login-page 拿到的网址是不是这个工具的登录页（别的网址不在账号的浏览器里打开）
pub fn cmd_login_check(args: &[String]) -> R<()> {
    arity("cmd_login_check", args, 1, 1)?;
    let url = args[0].as_str();
    let ok = if t().kind == Kind::Agy {
        agy_login_target(url).is_some_and(|state| !state.is_empty())
    } else {
        login_target(url).is_some()
    };
    if ok {
        return Ok(());
    }
    let host = urlsplit_host(url);
    let other = tools().iter().find(|x| !x.is_current() && x.login_hosts.contains(&host.as_str()));
    die(format!(
        "这不是 {} 的登录网址：要用服务器上的 cca 给出的那一整行{}",
        t().label,
        other.map_or(String::new(), |o| format!("（{} 账号用 {} login-page）", o.label, o.cmd))
    ))
}

/// `urlsplit(url).hostname or ''`，网址切不开也是空的
pub fn urlsplit_host(url: &str) -> String {
    urlsplit(url).ok().and_then(|u| u.hostname()).unwrap_or_default()
}

/// 这台机器上节点 n（账号表里的叫法）的摘要：路由器同步时记下的（node_ids），没有就从内置代理拷来的节点定义、维护者的
/// 参考节点里按名字找。认不出是空的。node_ids 里记的原样返回（被手改成别的类型时也是：原来就这样）
pub fn node_fp_here(c: &Dict, n: &str) -> V {
    if let Some(f) = node_ids(c).get(n).filter(|f| f.truthy()) {
        return f;
    }
    let fill = policy_fill().and_then(|f| f.list("proxies")).map(|l| l.items()).unwrap_or_default();
    let found = crate::cmd::corenet::core_defs()
        .into_iter()
        .chain(fill.iter().filter_map(|p| p.as_dict()))
        .find(|p| p.get("name").is_some_and(|name| name.as_str() == Some(n)));
    V::from(found.map_or(String::new(), |p| fill_fp(&p)))
}

/// 服务器上登录时交给 Mac 的节点摘要（login-page 的 --node-id）：Mac 按它认是不是同一个节点，两边的叫法可以不同
pub fn cmd_node_fp(args: &[String]) -> R<()> {
    arity("cmd_node_fp", args, 1, 1)?;
    out!("{}", node_fp_here(&cfg_load()?, &args[0]).py_str());
    Ok(())
}

/// cca login-page（Mac 上）：服务器上的账号在这台 Mac 上是哪个、走的是不是同一个节点。节点按摘要（类型 + 服务器 + 端口 +
/// TLS 域名）认，叫法可以不同；服务器没给摘要（旧版本），或者这边认不出这个节点的地址时按名字。输出给 bash eval：LP_KEY 是
/// 这台 Mac 上的账号；还没有这个账号时 LP_KEY 是空的，LP_NODE / LP_ALIAS 是加上它用的节点和别名（别名用不了就不要）。
/// 对不上就说清楚、退出 1
pub fn cmd_login_page_acct(args: &[String]) -> R<()> {
    arity("cmd_login_page_acct", args, 1, 4)?;
    let (who, node, fid, alias) = (args[0].as_str(), arg(args, 1, ""), arg(args, 2, ""), arg(args, 3, ""));
    let (c, table, _) = ctx()?;
    let email = text::strip(who).to_lowercase();
    if !fid.is_empty() && !re!(r"^[0-9a-f]{16}$").matches(fid) {
        return die("--node-id 不对：要用服务器上的 cca 给出的那一整行");
    }
    let key = if EMAIL_RE.matches(&email) {
        // 服务器给的是邮箱：只认这个邮箱（不按前缀猜）
        table.keys().into_iter().find(|k| parse_key(k).1.to_lowercase() == email).map_or(String::new(), |k| k.to_string())
    } else {
        account_named(&table, who)?
    };
    // 这台 Mac 上的节点 n 和服务器上的是不是同一个
    let same = |n: &str| {
        let f = node_fp_here(&c, n);
        if !fid.is_empty() && f.truthy() {
            f.as_str() == Some(fid)
        } else {
            n == node
        }
    };
    if !key.is_empty() {
        let v = table.at(&key);
        if is_api(&v) {
            return die(format!("{} 是第三方 API 账号，不用登录", label_of(&key)));
        }
        let here = node_of(&v);
        if !node.is_empty() && !same(&here) {
            return die(format!(
                "{} 在服务器上走 {}，在这台 Mac 上走 {}：不是同一个节点，已停止（登录页和服务器换 token 要经同一个出口）。先把两边改成同一个节点",
                label_of(&key),
                node,
                here
            ));
        }
        sh(&[("LP_KEY", &key), ("LP_NODE", &here), ("LP_ALIAS", "")]);
        return Ok(());
    }
    if node.is_empty() {
        return die(format!("这台 Mac 上没有 {} 这个账号：先用 {} add 加上它，再运行这一行", who, t().cmd));
    }
    let want = policy_map(t(), None).str(&email);
    // c['nodes'] 平时是节点名的列表；被手改成别的东西时照 Python 的遍历和 in 来
    let nodes = c.at("nodes");
    let mut hit: Vec<String> = Vec::new();
    match &want {
        Some(w) if same(w) => hit.push(w.to_string()),
        Some(_) => {}
        None => {
            for n in py_iter(&nodes)? {
                match &n {
                    V::Str(s) if same(s) => hit.push(s.to_string()),
                    // node_ids(c).get(n)：列表、字典当不了键
                    V::List(_) | V::Dict(_) => {
                        let kind = type_name(&n);
                        return type_error(format!("cannot use '{kind}' as a dict key (unhashable type: '{kind}')"));
                    }
                    // 别的（数字、null）：node_ids 里没有这样的键，也不等于节点名
                    _ => {}
                }
            }
        }
    }
    let n = if hit.iter().any(|h| h == node) { node.to_string() } else { hit.first().cloned().unwrap_or_default() };
    if let Some(want) = want.as_deref().filter(|_| n.is_empty()) {
        return die(format!(
            "维护者的映射要 {email} 走 {want}，和服务器上的 {node} 不是同一个节点：两边的映射可能不是同一版，先在两边各运行一次 cca policy pull"
        ));
    }
    if n.is_empty() || !py_contains(&nodes, &n)? {
        return die(format!(
            "这台 Mac 的可选节点里没有服务器上的 {}（按地址也认不出）：先运行 cca router setup 把它加进可选节点，或者在服务器上给 {} 换一个两边都有的节点",
            want.as_deref().unwrap_or(node),
            email
        ));
    }
    let a = text::strip(alias);
    sh(&[("LP_KEY", ""), ("LP_NODE", &n), ("LP_ALIAS", if a.is_empty() || !alias_problem(&table, a).is_empty() { "" } else { a })]);
    Ok(())
}

/// `for n in x`：列表的各项、字符串的各个字符、字典的键；别的遍历不了（TypeError）
fn py_iter(x: &V) -> R<Vec<V>> {
    match x {
        V::List(l) => Ok(l.items()),
        V::Str(s) => Ok(s.chars().map(|ch| V::str(ch.to_string())).collect()),
        V::Dict(d) => Ok(d.keys().into_iter().map(V::from).collect()),
        v => type_error(format!("'{}' object is not iterable", type_name(v))),
    }
}

/// `n in x`（n 是字符串）：列表看有没有这一项，字符串看是不是它的一段，字典看有没有这个键
fn py_contains(x: &V, n: &str) -> R<bool> {
    match x {
        V::List(l) => Ok(l.has_str(n)),
        V::Str(s) => Ok(s.contains(n)),
        V::Dict(d) => Ok(d.has(n)),
        v => type_error(format!("argument of type '{}' is not a container or iterable", type_name(v))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dict, list};

    #[test]
    fn validators_like_the_python_regexes() {
        // 右边是 Python 的 re.match(…) 给的真假
        assert!(visible_ascii("a") && visible_ascii("!~") && visible_ascii("abc\n") && visible_ascii(&"x".repeat(8192)));
        assert!(
            !visible_ascii("")
                && !visible_ascii("\n")
                && !visible_ascii("a b")
                && !visible_ascii("abc\n\n")
                && !visible_ascii("中")
        );
        assert!(!visible_ascii(&"x".repeat(8193)) && !visible_ascii("a\tb") && !visible_ascii("a\u{7f}"));
        assert!(https_url("https://x") && https_url("https://x\n") && https_url(&format!("https://{}", "x".repeat(8192))));
        assert!(!https_url("https://") && !https_url("http://x") && !https_url("HTTPS://x") && !https_url(" https://x"));
        assert!(!https_url("https://a b") && !https_url(&format!("https://{}", "x".repeat(8193))) && !https_url("https://x\n\n"));
        assert!(login_code_re("abc") && login_code_re("a\"b$c~!") && login_code_re("abc\n") && login_code_re(&"x".repeat(4096)));
        assert!(
            !login_code_re("") && !login_code_re("a#b") && !login_code_re("a b") && !login_code_re("#") && !login_code_re("\n")
        );
        assert!(!login_code_re(&"x".repeat(4097)) && !login_code_re("abc\n\n") && !login_code_re("é"));
        assert_eq!(ascii_only("a\u{1}b中c d~\u{7f}", None, 80), "abc d~");
        assert_eq!(ascii_only("a\u{1}b中c d~\u{7f}", Some('?'), 200), "a?b?c d~?");
        assert_eq!(ascii_only(&"é".repeat(300), Some('?'), 200), "?".repeat(200));
        assert_eq!(ascii_only(&"ab".repeat(100), None, 80).len(), 80);
    }

    #[test]
    fn ascii_errors_say_what_python_says() {
        // 右边是 Python 的 str(UnicodeEncodeError)（s.encode('ascii')）
        let said = |s: &str| match ascii_encode_error(s) {
            Some(Fail::Value(m)) => m,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            said("GET /中 HTTP/1.1"),
            "'ascii' codec can't encode character '\\u4e2d' in position 5: ordinal not in range(128)"
        );
        assert_eq!(said("café#st"), "'ascii' codec can't encode character '\\xe9' in position 3: ordinal not in range(128)");
        assert_eq!(
            said("a\u{1f600}b"),
            "'ascii' codec can't encode character '\\U0001f600' in position 1: ordinal not in range(128)"
        );
        assert_eq!(said("码码#x"), "'ascii' codec can't encode characters in position 0-1: ordinal not in range(128)");
        assert_eq!(said("xéèy中"), "'ascii' codec can't encode characters in position 1-2: ordinal not in range(128)");
        assert!(ascii_encode_error("ok").is_none() && ascii_encode_error("").is_none());
    }

    #[test]
    fn query_lookup_is_last_wins() {
        let q = parse_qsl("a=1&b=&a=2&c", true);
        assert_eq!((q_get(&q, "a"), q_get(&q, "b"), q_get(&q, "c"), q_get(&q, "d")), (Some("2"), Some(""), Some(""), None));
        assert_eq!((q_some(&q, "a"), q_some(&q, "b"), q_some(&q, "d")), (Some("2"), None, None));
    }

    #[test]
    fn hint_is_added_only_to_this_tools_login_page() {
        // 右边是 Python 的 login_hint_url()（T 是 Claude）
        let hint = |key: &str, url: &str| login_hint_url(key, url).unwrap();
        let k = "主号(alice@example.com)";
        for (url, want) in [
            ("https://claude.ai/oauth/authorize?a=1", "https://claude.ai/oauth/authorize?a=1&login_hint=alice%40example.com"),
            ("HTTPS://Claude.AI/oauth/authorize", "https://Claude.AI/oauth/authorize?login_hint=alice%40example.com"),
            ("https://claude.ai/oauth/authorize?", "https://claude.ai/oauth/authorize?login_hint=alice%40example.com"),
            ("https://claude.ai/oauth/authorize#frag", "https://claude.ai/oauth/authorize?login_hint=alice%40example.com#frag"),
            ("https://claude.ai/oauth/authorize?a=1#", "https://claude.ai/oauth/authorize?a=1&login_hint=alice%40example.com"),
            (" https://claude.ai/oauth/\tauthorize?a=1\n", "https://claude.ai/oauth/authorize?a=1&login_hint=alice%40example.com"),
            (
                "https://u:p@claude.ai:443/x/oauth/authorize?a=1&&b",
                "https://u:p@claude.ai:443/x/oauth/authorize?a=1&&b&login_hint=alice%40example.com",
            ),
            (
                "https://claude.ai/oauth/authorize?LOGIN_HINT=x",
                "https://claude.ai/oauth/authorize?LOGIN_HINT=x&login_hint=alice%40example.com",
            ),
            // 不动的：已经带了、不是 https、别的域名、别的路径
            ("https://claude.ai/oauth/authorize?login_hint", "https://claude.ai/oauth/authorize?login_hint"),
            (
                "https://claude.ai/oauth/authorize?a=1&login_hint=x%40y.z",
                "https://claude.ai/oauth/authorize?a=1&login_hint=x%40y.z",
            ),
            ("http://claude.ai/oauth/authorize?a=1", "http://claude.ai/oauth/authorize?a=1"),
            ("https://claude.ai./oauth/authorize?a=1", "https://claude.ai./oauth/authorize?a=1"),
            ("https://auth.openai.com/oauth/authorize?a=1", "https://auth.openai.com/oauth/authorize?a=1"),
            ("https://claude.ai/oauth/authorize/x?a=1", "https://claude.ai/oauth/authorize/x?a=1"),
            (" HTTPS://evil.example.com/oauth/authorize", " HTTPS://evil.example.com/oauth/authorize"),
        ] {
            assert_eq!(hint(k, url), want, "{url}");
        }
        assert_eq!(hint("只有别名", "https://claude.ai/oauth/authorize?a=1"), "https://claude.ai/oauth/authorize?a=1");
        assert_eq!(
            hint("o'neil+x@example.com", "https://claude.com/cai/oauth/authorize"),
            "https://claude.com/cai/oauth/authorize?login_hint=o%27neil%2Bx%40example.com"
        );
        assert!(matches!(login_hint_url(k, "https://[::1/oauth/authorize"), Err(Fail::Value(m)) if m == "Invalid IPv6 URL"));
        // Codex、agy 的登录页经子命令测（这里不换当前工具：它是全进程的，别的测试同时在跑）
    }

    const CB: &str = "response_type=code&code_challenge=CC&state=st-1&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback";

    #[test]
    fn login_targets() {
        let target = |q: &str| login_target(&format!("https://claude.ai/oauth/authorize?{q}"));
        assert_eq!(target(CB), Some(("localhost".into(), 1455, "/auth/callback".into(), "st-1".into())));
        let with = |uri: &str| target(&CB.replace("http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback", uri));
        assert_eq!(with("http://127.0.0.1:8/"), Some(("127.0.0.1".into(), 8, "/".into(), "st-1".into())));
        assert_eq!(with("http://LocalHost:65535"), Some(("localhost".into(), 65535, "/".into(), "st-1".into())));
        assert_eq!(with("http://localhost:1455/a%2520b?x=1%23f"), Some(("localhost".into(), 1455, "/a%20b".into(), "st-1".into())));
        for bad in [
            "http://localhost/cb",
            "http://localhost:0/cb",
            "http://localhost:65536/cb",
            "http://localhost:x/cb",
            "https://localhost:1/cb",
            "http://example.com:1/cb",
            "http://[::1]:1/cb",
            "http://[::1/cb",
            "localhost:1455",
            "",
        ] {
            assert_eq!(with(bad), None, "{bad}");
        }
        assert_eq!(target(&CB.replace("state=st-1", "state=a%23b")), None);
        assert_eq!(target(&CB.replace("state=st-1", "state=")), None);
        assert_eq!(target(&CB.replace("&state=st-1", "")), None);
        assert_eq!(target(&CB.replace("state=st-1", "state=a%0A")).map(|t| t.3), Some("a\n".into())); // 正则的 $ 也认结尾的换行
        assert_eq!(target(&format!("{CB}&state=two")).map(|t| t.3), Some("two".into()));
        assert_eq!(target(&CB.replace("code_challenge=CC", "code_challenge=")), None);
        assert_eq!(target(&CB.replace("response_type=code", "response_type=token")), None);
        for url in [
            format!("http://claude.ai/oauth/authorize?{CB}"),
            format!("https://claude.ai/oauth/authorize/?{CB}"),
            format!("https://evil.example.com/oauth/authorize?{CB}"),
            format!("https://auth.openai.com/oauth/authorize?{CB}"),
            format!("https://claude.ai/oauth/authorize?{CB} "),
            format!("https://claude.ai/oauth/authorize?{CB}\n\n"),
        ] {
            assert_eq!(login_target(&url), None, "{url}");
        }
        assert!(login_target(&format!("https://platform.claude.com/x/oauth/authorize?{CB}\n")).is_some());
    }

    #[test]
    fn agy_login_targets() {
        let q = "response_type=code&code_challenge=CC&state=st-agy&redirect_uri=https%3A%2F%2Fantigravity.google%2Foauth-callback";
        // 不看当前工具（这里是 Claude）：认的总是 agy 的登录页
        assert_eq!(agy_login_target(&format!("https://accounts.google.com/o/oauth2/auth?{q}")), Some("st-agy".into()));
        assert_eq!(agy_login_target(&format!("https://accounts.google.com/o/oauth2/v2/auth?{q}")), Some("st-agy".into()));
        assert_eq!(agy_login_target(&format!("https://Accounts.Google.com/o/oauth2/auth?{q}&x")), Some("st-agy".into()));
        for bad in [
            format!("https://accounts.google.com/o/oauth2/auth/?{q}"),
            format!("https://accounts.google.com/x/o/oauth2/auth?{q}"),
            format!("http://accounts.google.com/o/oauth2/auth?{q}"),
            format!("https://google.com/o/oauth2/auth?{q}"),
            format!("https://accounts.google.com/o/oauth2/auth?{}", q.replace("antigravity.google", "antigravity.example.com")),
            format!("https://accounts.google.com/o/oauth2/auth?{}", q.replace("state=st-agy", "state=a%20b")),
            format!("https://accounts.google.com/o/oauth2/auth?{}", q.replace("code_challenge=CC", "x=1")),
            format!("https://accounts.google.com/o/oauth2/auth?{}", q.replace("response_type=code", "response_type=")),
            "https://accounts.google.com/o/oauth2/auth".to_string(),
            String::new(),
        ] {
            assert_eq!(agy_login_target(&bad), None, "{bad}");
        }
        assert_eq!(urlsplit_host("https://User@Accounts.Google.com:443/x"), "accounts.google.com");
        assert_eq!(
            (urlsplit_host("no-scheme/path"), urlsplit_host("https://[::1/x"), urlsplit_host("")),
            (String::new(), String::new(), String::new())
        );
    }

    #[test]
    fn python_iteration_and_membership() {
        let strs = |x: &V| py_iter(x).unwrap().iter().map(|v| v.py_str()).collect::<Vec<_>>();
        assert_eq!(strs(&V::from(list!["US-A", 1])), ["US-A", "1"]);
        assert_eq!(strs(&V::from("节点A")), ["节", "点", "A"]);
        assert_eq!(strs(&V::Dict(dict! { "a" => 1, "b" => 2 })), ["a", "b"]);
        assert!(
            matches!(py_iter(&V::None), Err(Fail::Other("TypeError", m)) if m[0] == "TypeError: 'NoneType' object is not iterable")
        );
        assert!(py_iter(&V::Int(3)).is_err() && py_iter(&V::Bool(true)).is_err());
        assert!(py_contains(&V::from(list!["US-A"]), "US-A").unwrap() && !py_contains(&V::from(list!["US-A"]), "US").unwrap());
        assert!(py_contains(&V::from("US-A,US-B"), "US").unwrap() && py_contains(&V::Dict(dict! { "US-A" => 1 }), "US-A").unwrap());
        assert!(py_contains(&V::None, "x").is_err() && py_contains(&V::Float(1.5), "x").is_err());
    }
}
