//! 连 Gmail 走哪条路、怎么连上去（`mail_proxy_parse` 到 `mail_tls`）：照 macOS 的系统代理（和系统设置里网络的代理一样），
//! 不读环境变量——cca 起的进程里 https_proxy 是账号的出口，Gmail 和 Claude 账号不能从同一个出口出去。
//! 隧道（SOCKS5 不带认证、HTTP CONNECT）和 TLS：证书照常校验，绝不跳过。
//!
//! 套接字出错的原因照 Python 里 `e.strerror or str(e)` 的样子给（系统的说法：Connection refused、timed out……）。

use super::{mail_fatal, mail_oserr, mail_retry, MAIL_HOST, MAIL_PORT, MAIL_PROXY_HINT};
use crate::cmd::dock::home_swapped;
use crate::core::tool::tools;
use crate::py::fail::errno_text;
use crate::py::obj::{Dict, V};
use crate::py::proc::{getenv, Cmd, In, Out};
use crate::py::text;
use crate::py::tls::{self, TlsError, TlsStream};
use crate::py::url::urlsplit;
use crate::py::{Fail, R};
use crate::re;
use std::ffi::{CStr, CString};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, TcpStream};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Via {
    Direct,
    Socks5,
    Http,
}

/// 连 Gmail 的路：原来的三元组 ('direct', '', 0) / ('socks5', 主机, 端口) / ('http', 主机, 端口)
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Route {
    pub via: Via,
    pub host: String,
    pub port: u16,
}

impl Route {
    pub fn direct() -> Route {
        Route { via: Via::Direct, host: String::new(), port: 0 }
    }
}

/// `scutil --proxy` 的顶层键值和例外列表
pub type SysProxy = (Vec<(String, String)>, Vec<String>);

/// scutil --proxy 的输出 → (顶层的键值, 例外列表)：只认两格缩进的顶层「键 : 值」（__SCOPED__ 里按网卡分的不管）
pub fn mail_proxy_parse(output: &str) -> SysProxy {
    let (mut kv, mut exc, mut inexc): (Vec<(String, String)>, Vec<String>, bool) = (Vec::new(), Vec::new(), false);
    for line in text::splitlines(output) {
        if inexc {
            if let Some(m) = re!(r"^    \d+ : (.+)$").match_(line) {
                exc.push(text::strip(m.g(1)).to_string());
                continue;
            }
            inexc = false;
        }
        if let Some(m) = re!(r"^  (\w+) : (.*)$").match_(line) {
            if m.g(1) == "ExceptionsList" {
                inexc = true;
            } else {
                let (k, v) = (m.g(1).to_string(), text::strip(m.g(2)).to_string());
                match kv.iter_mut().find(|(x, _)| *x == k) {
                    Some(slot) => slot.1 = v,
                    None => kv.push((k, v)),
                }
            }
        }
    }
    (kv, exc)
}

/// 系统代理的设置：起 `scutil --proxy`（起不来、5 秒没跑完当作没有代理）
pub fn mail_sysproxy() -> SysProxy {
    match Cmd::new(["scutil", "--proxy"]).stdin(In::Null).stdout(Out::Capture).stderr(Out::Null).timeout(5.0).run() {
        Ok(done) => mail_proxy_parse(&String::from_utf8_lossy(&done.stdout)),
        Err(_) => (Vec::new(), Vec::new()),
    }
}

/// `fnmatch` 的方括号里那一段（已经照 `fnmatch.translate` 整理成正则的字符组的写法）：单个字符和区间
struct CharSet {
    negate: bool,
    ranges: Vec<(char, char)>,
}

/// 通配符模式的一项
enum Glob {
    Star,
    Any,
    Lit(char),
    Set(CharSet),
    /// 空的字符组 `[]…`：哪个字符都对不上
    Never,
}

/// 正则的字符组（方括号里面的内容）照 Python 的 re 读：反斜杠后面的字符原样算，`a-z` 是区间，结尾的 `-` 是它自己
fn parse_charset(stuff: &str) -> Result<CharSet, Fail> {
    let cs: Vec<char> = stuff.chars().collect();
    let (negate, mut i) = if cs.first() == Some(&'^') { (true, 1) } else { (false, 0) };
    let mut ranges = Vec::new();
    let item = |i: &mut usize| -> Option<char> {
        let c = *cs.get(*i)?;
        *i += 1;
        if c == '\\' {
            let e = *cs.get(*i)?;
            *i += 1;
            return Some(e);
        }
        Some(c)
    };
    while i < cs.len() {
        let Some(lo) = item(&mut i) else { break };
        if cs.get(i) == Some(&'-') {
            if i + 1 >= cs.len() {
                // 结尾的连字符：就是它自己
                ranges.push((lo, lo));
                ranges.push(('-', '-'));
                break;
            }
            i += 1;
            let Some(hi) = item(&mut i) else { break };
            if hi < lo {
                return Err(Fail::Other("PatternError", vec![format!("bad character range {lo}-{hi}")]));
            }
            ranges.push((lo, hi));
        } else {
            ranges.push((lo, lo));
        }
    }
    Ok(CharSet { negate, ranges })
}

/// `fnmatch.translate(pat)`（CPython 3.14）：`*`、`?`、`[…]`、`[!…]`，别的原样。方括号没合上的 `[` 是它自己
fn glob_compile(pat: &str) -> Result<Vec<Glob>, Fail> {
    let p: Vec<char> = pat.chars().collect();
    let n = p.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let c = p[i];
        i += 1;
        match c {
            '*' => {
                out.push(Glob::Star);
                while i < n && p[i] == '*' {
                    i += 1; // 连着的星号算一个
                }
            }
            '?' => out.push(Glob::Any),
            '[' => {
                let mut j = i;
                if j < n && p[j] == '!' {
                    j += 1;
                }
                if j < n && p[j] == ']' {
                    j += 1;
                }
                while j < n && p[j] != ']' {
                    j += 1;
                }
                if j >= n {
                    out.push(Glob::Lit('['));
                    continue;
                }
                let escape = |s: &[char]| -> String { s.iter().collect::<String>().replace('\\', "\\\\") };
                let stuff: String = if !p[i..j].contains(&'-') {
                    escape(&p[i..j])
                } else {
                    // 连字符把这一段切成几块：块和块之间的连字符是区间，块里面的是它自己
                    let mut chunks: Vec<Vec<char>> = Vec::new();
                    let mut k = if p[i] == '!' { i + 2 } else { i + 1 };
                    while let Some(at) = (k..j).find(|x| p[*x] == '-') {
                        chunks.push(p[i..at].to_vec());
                        i = at + 1;
                        k = at + 3;
                    }
                    if i < j {
                        chunks.push(p[i..j].to_vec());
                    } else if let Some(last) = chunks.last_mut() {
                        last.push('-');
                    }
                    // 倒着的区间（z-a）去掉：正则里不许有
                    let mut idx = chunks.len();
                    while idx > 1 {
                        idx -= 1;
                        if chunks[idx - 1].last() > chunks[idx].first() {
                            let tail: Vec<char> = chunks[idx][1..].to_vec();
                            chunks[idx - 1].pop();
                            chunks[idx - 1].extend(tail);
                            chunks.remove(idx);
                        }
                    }
                    chunks.iter().map(|s| escape(s).replace('-', "\\-")).collect::<Vec<_>>().join("-")
                };
                i = j + 1;
                if stuff.is_empty() {
                    out.push(Glob::Never);
                } else if stuff == "!" {
                    out.push(Glob::Any);
                } else {
                    // & ~ | 在正则的字符组里要转义（照 Python 读的话都只是字符本身）
                    let mut stuff: String =
                        stuff.chars().flat_map(|c| if matches!(c, '&' | '~' | '|') { vec!['\\', c] } else { vec![c] }).collect();
                    if let Some(rest) = stuff.strip_prefix('!') {
                        stuff = format!("^{rest}");
                    } else if stuff.starts_with(['^', '[']) {
                        stuff = format!("\\{stuff}");
                    }
                    out.push(Glob::Set(parse_charset(&stuff)?));
                }
            }
            other => out.push(Glob::Lit(other)),
        }
    }
    Ok(out)
}

fn glob_match(items: &[Glob], name: &[char]) -> bool {
    // 经典的两个指针：星号先当空的，对不上了回到最近的星号让它多吃一个字符
    let (mut i, mut k) = (0, 0);
    let mut back: Option<(usize, usize)> = None;
    while k < name.len() {
        let hit = match items.get(i) {
            Some(Glob::Star) => {
                back = Some((i, k));
                i += 1;
                continue;
            }
            Some(Glob::Any) => true,
            Some(Glob::Lit(c)) => *c == name[k],
            Some(Glob::Set(set)) => set.ranges.iter().any(|(lo, hi)| *lo <= name[k] && name[k] <= *hi) != set.negate,
            Some(Glob::Never) | None => false,
        };
        if hit {
            i += 1;
            k += 1;
        } else if let Some((star, at)) = back {
            back = Some((star, at + 1));
            i = star + 1;
            k = at + 1;
        } else {
            return false;
        }
    }
    items[i.min(items.len())..].iter().all(|g| matches!(g, Glob::Star))
}

/// `fnmatch.fnmatchcase(name, pat)`
pub fn fnmatchcase(name: &str, pat: &str) -> Result<bool, Fail> {
    let name: Vec<char> = name.chars().collect();
    Ok(glob_match(&glob_compile(pat)?, &name))
}

/// 系统代理的例外里有没有这个主机（*.gmail.com、.gmail.com、imap.gmail.com 这些写法）
pub fn mail_bypass(host: &str, pats: &[String]) -> R<bool> {
    for p in pats {
        let p = text::strip(p).to_lowercase();
        if !p.is_empty() && (fnmatchcase(host, &p)? || (p.starts_with('.') && host.ends_with(&p))) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn mail_route_note(r: &Route) -> String {
    match r.via {
        Via::Direct => "直连".to_string(),
        Via::Socks5 => format!("经代理 {}:{}，SOCKS5", r.host, r.port),
        Via::Http => format!("经代理 {}:{}，HTTP CONNECT", r.host, r.port),
    }
}

/// 原来对手改坏了的 config.json 直接调方法，类型不对是没人接的 AttributeError
fn attribute_error(what: &str, v: &V, attr: &str) -> Fail {
    let kind = match v {
        V::None => "NoneType",
        V::Bool(_) => "bool",
        V::Int(_) | V::Big(_) => "int",
        V::Float(_) => "float",
        V::Str(_) => "str",
        V::List(_) => "list",
        V::Dict(_) => "dict",
    };
    Fail::Other("AttributeError", vec![format!("'{kind}' object has no attribute '{attr}'（config.json 的 {what}）")])
}

/// 这条路是不是账号的出口：这一次的 CCA_PROXY，或者端口池里的端口（本机、路由器上的）。
/// cca_proxy 是环境变量 CCA_PROXY 的值，调用的人取了给进来（单元测试不去动进程的环境变量）
pub fn mail_route_exit(r: &Route, c: &Dict, cca_proxy: &str) -> R<bool> {
    let (host, port) = (r.host.to_lowercase(), r.port);
    if let Ok(u) = urlsplit(cca_proxy) {
        if let Some(h) = u.hostname().filter(|h| !h.is_empty()) {
            // 端口写得不对（u.port 出 ValueError）：当作不是
            if let Ok(p) = u.port() {
                if h.to_lowercase() == host && p == Some(port) {
                    return Ok(true);
                }
            }
        }
    }
    let mut in_pool = false;
    for tool in tools() {
        match tool.state(c, false).get("ports").filter(|p| p.truthy()) {
            None => {}
            Some(V::Dict(ports)) => {
                in_pool |= ports.values().iter().any(|p| p.isinstance_int() && p.py_eq(&V::Int(port as i64)));
            }
            Some(other) => return Err(attribute_error("ports", &other, "values")),
        }
    }
    let proxy_host = crate::core::base::str_or_empty(&c.get("proxy_host")).to_lowercase();
    let router_host = match c.get("router").filter(|x| x.truthy()) {
        None => String::new(),
        Some(V::Dict(router)) => crate::core::base::str_or_empty(&router.get("host")).to_lowercase(),
        Some(other) => return Err(attribute_error("router", &other, "get")),
    };
    let near = ["127.0.0.1", "localhost", "::1", proxy_host.as_str(), router_host.as_str()];
    Ok(in_pool && near.contains(&host.as_str()))
}

/// 连 Gmail 走哪条路。spec 是 config.json 的 mail.proxy：system（默认：照系统代理，SOCKS 优先，再是 HTTPS、HTTP 代理的
/// CONNECT；没开代理、只有 PAC、例外里有 Gmail 时直连）、direct、socks5://主机:端口、http://主机:端口。走不通就报错，
/// 不换别的路。指到账号出口的不用：Gmail 和 Claude 账号不能从同一个出口出去
pub fn mail_route(spec: Option<&str>, c: &Dict, sysp: Option<SysProxy>) -> R<Route> {
    route_with(spec, c, sysp, &getenv("CCA_PROXY").unwrap_or_default())
}

/// `mail_route`，这一次的 CCA_PROXY 由调用的人给（见 `mail_route_exit`）
fn route_with(spec: Option<&str>, c: &Dict, sysp: Option<SysProxy>, cca_proxy: &str) -> R<Route> {
    let r = match spec {
        None | Some("") | Some("system") => {
            let (kv, exc) = sysp.unwrap_or_else(mail_sysproxy);
            let get = |k: String| kv.iter().find(|(x, _)| *x == k).map(|(_, v)| v.as_str());
            let mut r = Route::direct();
            if !mail_bypass(MAIL_HOST, &exc)? {
                for (via, k) in [(Via::Socks5, "SOCKS"), (Via::Http, "HTTPS"), (Via::Http, "HTTP")] {
                    let (h, p) = (get(format!("{k}Proxy")).unwrap_or(""), get(format!("{k}Port")).unwrap_or(""));
                    if get(format!("{k}Enable")) == Some("1") && !h.is_empty() && text::isdigit(p) {
                        // isdigit 认的字符比 int() 多（上标的 ² 这类）：那时原来是没人接的 ValueError
                        let n = port_number(p)?;
                        if (1..65536).contains(&n) {
                            r = Route { via, host: h.to_string(), port: n as u16 };
                            break;
                        }
                    }
                }
            }
            r
        }
        Some("direct") => Route::direct(),
        Some(spec) => {
            let parsed = re!(r"^(socks5h?|http)://(\[[0-9A-Fa-f:.]+\]|[A-Za-z0-9.-]+):(\d{1,5})/?$").match_(spec).and_then(|m| {
                let port = text::to_int(m.g(3)).filter(|n| (1..65536).contains(n))?;
                let via = if m.g(1).starts_with("socks") { Via::Socks5 } else { Via::Http };
                Some(Route { via, host: text::strip_chars(m.g(2), "[]").to_string(), port: port as u16 })
            });
            match parsed {
                Some(r) => r,
                None => {
                    return Err(mail_fatal(
                        "proxy",
                        format!("连 Gmail 的方式 {spec} 认不出：要写 system、direct、socks5://主机:端口 或 http://主机:端口"),
                    ))
                }
            }
        }
    };
    if r.via != Via::Direct && mail_route_exit(&r, c, cca_proxy)? {
        return Err(mail_fatal("proxy", format!("代理 {}:{} 是账号的出口，Gmail 不能经它连。{}", r.host, r.port, MAIL_PROXY_HINT)));
    }
    Ok(r)
}

/// `int(p)`，p 是 `isdigit()` 的字符串：位数太多、不是十进制数字（上标数字）是 ValueError；大得放不下的反正不是合法的端口
fn port_number(p: &str) -> R<i64> {
    let digits = text::len(p);
    if digits > super::codec::INT_MAX_STR_DIGITS {
        return Err(super::codec::int_too_long(digits));
    }
    if !p.chars().all(crate::py::uni::is_decimal) {
        return Err(Fail::Value(format!("invalid literal for int() with base 10: {}", crate::py::obj::repr_str(p))));
    }
    Ok(text::to_int(p).unwrap_or(i64::MAX))
}

// --- 隧道、TLS ---------------------------------------------------------------------------------------------------

/// 套接字出的错在 `mail_oserr` 里的样子：(`e.strerror or str(e)`, 类名)
pub fn io_text(e: &io::Error) -> (String, &'static str) {
    if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) && e.raw_os_error() != Some(libc::ETIMEDOUT) {
        return ("timed out".to_string(), "TimeoutError");
    }
    match e.raw_os_error() {
        Some(n) => (
            errno_text(n),
            match n {
                libc::ECONNREFUSED => "ConnectionRefusedError",
                libc::ECONNRESET => "ConnectionResetError",
                libc::ECONNABORTED => "ConnectionAbortedError",
                libc::EPIPE => "BrokenPipeError",
                libc::ETIMEDOUT => "TimeoutError",
                _ => "OSError",
            },
        ),
        None => (e.to_string(), "OSError"),
    }
}

/// Python 的 getaddrinfo 先把主机名按 idna 编码，编不了是没人接的 UnicodeEncodeError：ASCII 的名字里有空的一段
/// （最后一个点后面的不算），或者哪一段有 64 个字符以上
fn idna_problem(host: &str) -> Option<&'static str> {
    if !host.is_ascii() {
        return None;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels[..labels.len() - 1].iter().any(|l| l.is_empty()) {
        return Some("label empty");
    }
    labels.iter().any(|l| l.len() >= 64).then_some("label too long")
}

/// `socket.getaddrinfo(host, port, 0, SOCK_STREAM)` 的各个地址；解析不了是 gaierror（给出系统的说法）
fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>, (String, &'static str)> {
    let bad = || ("nodename nor servname provided, or not known".to_string(), "gaierror");
    let (chost, cport) = (CString::new(host).map_err(|_| bad())?, CString::new(port.to_string()).map_err(|_| bad())?);
    let mut out = Vec::new();
    // SAFETY: addrinfo 是纯数据，全 0 是合法的值；getaddrinfo 给的链表只在 freeaddrinfo 之前读，
    // 每一项的 ai_addr 照它自己的地址族读成 sockaddr_in / sockaddr_in6（长度够才读）
    unsafe {
        let mut hints: libc::addrinfo = std::mem::zeroed();
        hints.ai_socktype = libc::SOCK_STREAM;
        let mut res: *mut libc::addrinfo = std::ptr::null_mut();
        let rc = libc::getaddrinfo(chost.as_ptr(), cport.as_ptr(), &hints, &mut res);
        if rc != 0 {
            return Err((CStr::from_ptr(libc::gai_strerror(rc)).to_string_lossy().into_owned(), "gaierror"));
        }
        let mut p = res;
        while !p.is_null() {
            let ai = &*p;
            if !ai.ai_addr.is_null() {
                if ai.ai_family == libc::AF_INET && ai.ai_addrlen as usize >= std::mem::size_of::<libc::sockaddr_in>() {
                    let sa = &*ai.ai_addr.cast::<libc::sockaddr_in>();
                    out.push(SocketAddr::V4(SocketAddrV4::new(
                        Ipv4Addr::from(u32::from_be(sa.sin_addr.s_addr)),
                        u16::from_be(sa.sin_port),
                    )));
                } else if ai.ai_family == libc::AF_INET6 && ai.ai_addrlen as usize >= std::mem::size_of::<libc::sockaddr_in6>() {
                    let sa = &*ai.ai_addr.cast::<libc::sockaddr_in6>();
                    let ip = Ipv6Addr::from(sa.sin6_addr.s6_addr);
                    out.push(SocketAddr::V6(SocketAddrV6::new(ip, u16::from_be(sa.sin6_port), sa.sin6_flowinfo, sa.sin6_scope_id)));
                }
            }
            p = ai.ai_next;
        }
        libc::freeaddrinfo(res);
    }
    Ok(out)
}

/// `socket.create_connection((host, port), timeout)`：各个地址挨个试（每个最多等 timeout 秒），都不成给出最后一个的原因
fn create_connection(host: &str, port: u16, timeout: f64) -> R<Result<TcpStream, (String, &'static str)>> {
    if let Some(why) = idna_problem(host) {
        return Err(Fail::Other("UnicodeEncodeError", vec![format!("'idna' codec can't encode characters in {host:?}: {why}")]));
    }
    let addrs = match resolve(host, port) {
        Ok(a) => a,
        Err(e) => return Ok(Err(e)),
    };
    let mut last = ("getaddrinfo returns an empty list".to_string(), "OSError");
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, Duration::from_secs_f64(timeout)) {
            Ok(s) => return Ok(Ok(s)),
            Err(e) => last = io_text(&e),
        }
    }
    Ok(Err(last))
}

/// 隧道没搭成的两种原因
enum Tunnel {
    /// 套接字出错（超时、被重置……）：mail_dial 说「没有接通 Gmail」
    Os(io::Error),
    /// 代理的回答不对：已经是说好的 MailRetry
    Mail(Fail),
}

impl From<io::Error> for Tunnel {
    fn from(e: io::Error) -> Tunnel {
        Tunnel::Os(e)
    }
}

fn proxy_retry(text: impl Into<String>) -> Tunnel {
    Tunnel::Mail(mail_retry("proxy", text))
}

/// 读满 n 个字节；对方关了是「代理断开了连接」
fn mail_recv(s: &mut TcpStream, n: usize) -> Result<Vec<u8>, Tunnel> {
    let mut buf = vec![0u8; n];
    let mut got = 0;
    while got < n {
        match s.read(&mut buf[got..]) {
            Ok(0) => return Err(proxy_retry("代理断开了连接")),
            Ok(k) => got += k,
            // 被信号打断：按了 Ctrl-C 就不等了，别的接着读
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                if super::interrupted() {
                    return Err(Tunnel::Mail(super::keyboard_interrupt()));
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(buf)
}

/// SOCKS5（不带认证），目标写域名：交给代理解析
fn mail_socks5(s: &mut TcpStream) -> Result<(), Tunnel> {
    let host = MAIL_HOST.as_bytes();
    s.write_all(b"\x05\x01\x00")?;
    if mail_recv(s, 2)? != b"\x05\x00" {
        return Err(proxy_retry("SOCKS5 代理要认证，或者不认这个请求（cca 只用不带认证的 SOCKS5）"));
    }
    let mut req = b"\x05\x01\x00\x03".to_vec();
    req.push(host.len() as u8);
    req.extend_from_slice(host);
    req.extend_from_slice(&MAIL_PORT.to_be_bytes());
    s.write_all(&req)?;
    let head = mail_recv(s, 4)?;
    if head[1] != 0 {
        return Err(proxy_retry(format!("SOCKS5 代理没能连上 {}:{}（错误码 {}）", MAIL_HOST, MAIL_PORT, head[1])));
    }
    let n = match head[3] {
        1 => 4,
        4 => 16,
        3 => mail_recv(s, 1)?[0] as usize,
        _ => return Err(proxy_retry("SOCKS5 代理的回应认不出")),
    };
    mail_recv(s, n + 2)?;
    Ok(())
}

/// HTTP 代理的 CONNECT：一个字节一个字节地读回应（后面就是 TLS，不能多读）
fn mail_connect(s: &mut TcpStream) -> Result<(), Tunnel> {
    let target = format!("{MAIL_HOST}:{MAIL_PORT}");
    s.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())?;
    let mut buf: Vec<u8> = Vec::new();
    while !buf.ends_with(b"\r\n\r\n") {
        if buf.len() > 8192 {
            return Err(proxy_retry("HTTP 代理的回应太长"));
        }
        buf.extend(mail_recv(s, 1)?);
    }
    let end = buf.windows(2).position(|w| w == b"\r\n").unwrap_or(buf.len());
    let line: Vec<char> = buf[..end].iter().map(|b| *b as char).collect();
    // ^HTTP/1\.[01] 200\b：200 后面不能紧跟着字（Python 的 \w：字母数字和下划线）
    let head: String = line.iter().take(12).collect();
    let ok = (head == "HTTP/1.0 200" || head == "HTTP/1.1 200")
        && !line.get(12).is_some_and(|c| *c == '_' || crate::py::uni::is_alnum(*c));
    if !ok {
        let shown: String = line.iter().map(|c| if (' '..='~').contains(c) { *c } else { '?' }).take(80).collect();
        return Err(proxy_retry(format!("HTTP 代理不肯连 {target}（{shown}）")));
    }
    Ok(())
}

/// 连上 imap.gmail.com:993（还没有 TLS）：直连，或者经代理
pub fn mail_dial(r: &Route, timeout: f64) -> R<TcpStream> {
    let (place, host, port) = match r.via {
        Via::Direct => (format!("{MAIL_HOST}:{MAIL_PORT}"), MAIL_HOST, MAIL_PORT),
        _ => (format!("代理 {}:{}", r.host, r.port), r.host.as_str(), r.port),
    };
    let mut s = match create_connection(host, port, timeout)? {
        Ok(s) => s,
        Err((why, class)) => {
            super::check_interrupt()?;
            return Err(mail_retry("net", format!("连不上 {}（{}）", place, mail_oserr(&why, class))));
        }
    };
    let t = Some(Duration::from_secs_f64(timeout));
    let tunnel = s.set_read_timeout(t).and_then(|()| s.set_write_timeout(t)).map_err(Tunnel::Os).and_then(|()| match r.via {
        Via::Socks5 => mail_socks5(&mut s),
        Via::Http => mail_connect(&mut s),
        Via::Direct => Ok(()),
    });
    match tunnel {
        Ok(()) => Ok(s),
        Err(Tunnel::Os(e)) => {
            super::check_interrupt()?;
            let (why, class) = io_text(&e);
            Err(mail_retry("proxy", format!("{} 没有接通 Gmail（{}）", place, mail_oserr(&why, class))))
        }
        Err(Tunnel::Mail(e)) => Err(e),
    }
}

/// CCA_MAIL_CAFILE（只给测试的另一份 CA）这一次认不认：只在 HOME 换过（测试的沙箱）、套接字连的是本机（测试的代理）时才算。
/// 用户真正的主目录下、连的不是本机时一律不认：环境变量换不掉对 Gmail 的证书校验
fn test_ca(ca: Option<&str>, swapped: impl FnOnce() -> bool, peer: impl FnOnce() -> Option<IpAddr>) -> Option<&str> {
    ca.filter(|ca| !ca.is_empty())
        .filter(|_| swapped() && peer().is_some_and(|ip| matches!(ip.to_string().as_str(), "127.0.0.1" | "::1")))
}

/// 在连好的套接字上做 TLS，照常校验 imap.gmail.com 的证书，绝不跳过校验。
/// CCA_MAIL_CAFILE（只给测试）：另外信任这个 CA，只在 HOME 换过（测试的沙箱）、套接字连的是本机（测试的代理）时才算
pub fn mail_tls(s: TcpStream, timeout: f64) -> R<TlsStream> {
    let ca = getenv("CCA_MAIL_CAFILE");
    let extra = test_ca(ca.as_deref(), home_swapped, || s.peer_addr().ok().map(|peer| peer.ip()));
    match tls::connect(s, MAIL_HOST, extra) {
        Ok(t) => {
            let _ = t.set_timeout(timeout);
            Ok(t)
        }
        Err(e) => {
            super::check_interrupt()?;
            Err(match e {
                TlsError::Cert(why) => mail_fatal(
                    "tls",
                    format!(
                        // 原来这里说的是“或者这个 Python 缺根证书”：现在没有 Python 了，根证书照系统的那一份（py::tls）
                        "Gmail 的证书校验没通过（{}）：网络里有东西在拦截 TLS，或者系统的根证书（/etc/ssl/cert.pem）不全。cca 不会跳过校验",
                        mail_oserr(&why, "SSLCertVerificationError")
                    ),
                ),
                TlsError::Other(why) => mail_retry("net", format!("和 Gmail 的 TLS 握手没成（{}）", mail_oserr(&why, "SSLError"))),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::mail::mail_error;
    use crate::cmd::mail::testing::vectors;
    use crate::dict;
    use crate::py::obj::V;
    use std::net::TcpListener;

    /// 原来 tests/mail_view.py 的 Route
    const SCUTIL: &str = concat!(
        "<dictionary> {\n  ExceptionsList : <array> {\n    0 : *.local\n    1 : 169.254/16\n  }\n  FTPPassive : 1\n",
        "  HTTPEnable : 1\n  HTTPPort : 7890\n  HTTPProxy : 127.0.0.1\n  HTTPSEnable : 1\n  HTTPSPort : 7891\n",
        "  HTTPSProxy : 127.0.0.1\n  SOCKSEnable : 1\n  SOCKSPort : 7892\n  SOCKSProxy : 127.0.0.1\n",
        "  __SCOPED__ : <dictionary> {\n    en0 : <dictionary> {\n      SOCKSPort : 1\n    }\n  }\n}\n"
    );

    /// 不带这一次的 CCA_PROXY（跑测试的这个进程的环境里有没有、是什么都不管）
    fn route(spec: Option<&str>, sysp: Option<SysProxy>, c: Option<Dict>) -> R<Route> {
        route_with(spec, &c.unwrap_or_default(), Some(sysp.unwrap_or_else(|| mail_proxy_parse(SCUTIL))), "")
    }

    fn via(v: Via, host: &str, port: u16) -> Route {
        Route { via: v, host: host.to_string(), port }
    }

    fn set(kv: &mut [(String, String)], k: &str, v: &str) {
        kv.iter_mut().find(|(x, _)| x == k).unwrap().1 = v.to_string();
    }

    #[test]
    fn scutil_parse_top_level_only() {
        let (kv, exc) = mail_proxy_parse(SCUTIL);
        let get = |k: &str| kv.iter().find(|(x, _)| x == k).map(|(_, v)| v.as_str());
        assert_eq!((get("SOCKSPort"), get("HTTPSProxy")), (Some("7892"), Some("127.0.0.1")));
        assert_eq!(exc, ["*.local", "169.254/16"]);
    }

    #[test]
    fn scutil_parse_like_python() {
        let cases = vectors("proxy_parse");
        assert!(cases.len() > 10);
        for c in cases {
            let (kv, exc) = mail_proxy_parse(&c.at("in").py_str());
            let got_kv = Dict::from_pairs(kv);
            assert!(V::Dict(got_kv.clone()).py_eq(&c.at("kv")), "{:?}：{got_kv:?} / {:?}", c.at("in"), c.at("kv"));
            assert!(V::from(crate::py::obj::str_list(&exc)).py_eq(&c.at("exc")), "{:?}：{exc:?}", c.at("in"));
        }
    }

    #[test]
    fn system_proxy_socks_first_then_https_then_http() {
        assert_eq!(route(Some("system"), None, None).unwrap(), via(Via::Socks5, "127.0.0.1", 7892));
        let (mut kv, _) = mail_proxy_parse(SCUTIL);
        set(&mut kv, "SOCKSEnable", "0");
        assert_eq!(route(Some(""), Some((kv.clone(), vec![])), None).unwrap(), via(Via::Http, "127.0.0.1", 7891));
        set(&mut kv, "HTTPSEnable", "0");
        assert_eq!(route(None, Some((kv.clone(), vec![])), None).unwrap(), via(Via::Http, "127.0.0.1", 7890));
        set(&mut kv, "HTTPEnable", "0");
        assert_eq!(route(Some("system"), Some((kv, vec![])), None).unwrap(), Route::direct());
        let pac = vec![("ProxyAutoConfigEnable".to_string(), "1".to_string())];
        assert_eq!(route(Some("system"), Some((pac, vec![])), None).unwrap(), Route::direct(), "只有 PAC：直连");
    }

    #[test]
    fn system_proxy_exceptions() {
        let (kv, _) = mail_proxy_parse(SCUTIL);
        for pat in ["*.gmail.com", ".gmail.com", "imap.gmail.com", "IMAP.GMAIL.COM"] {
            assert_eq!(route(Some("system"), Some((kv.clone(), vec![pat.to_string()])), None).unwrap(), Route::direct(), "{pat}");
        }
        assert_eq!(
            route(Some("system"), Some((kv, vec!["*.google.com".to_string()])), None).unwrap(),
            via(Via::Socks5, "127.0.0.1", 7892)
        );
    }

    #[test]
    fn explicit_specs() {
        assert_eq!(route(Some("direct"), None, None).unwrap(), Route::direct());
        assert_eq!(route(Some("socks5://127.0.0.1:7897"), None, None).unwrap(), via(Via::Socks5, "127.0.0.1", 7897));
        assert_eq!(
            route(Some("socks5h://proxy.example.com:1080/"), None, None).unwrap(),
            via(Via::Socks5, "proxy.example.com", 1080)
        );
        assert_eq!(route(Some("http://[::1]:8080"), None, None).unwrap(), via(Via::Http, "::1", 8080));
        for bad in [
            "socks4://127.0.0.1:1",
            "http://127.0.0.1",
            "http://127.0.0.1:0",
            "http://127.0.0.1:70000",
            "http://a b:1",
            "https://127.0.0.1:1",
            "system2",
        ] {
            let e = route(Some(bad), None, None).unwrap_err();
            assert!(e.is("MailFatal"), "{bad}");
            assert_eq!(
                mail_error(&e),
                Some((
                    "proxy",
                    format!("连 Gmail 的方式 {bad} 认不出：要写 system、direct、socks5://主机:端口 或 http://主机:端口").as_str()
                ))
            );
        }
    }

    #[test]
    fn never_through_an_account_exit() {
        let c = || {
            dict! {
                "ports" => dict! { "alice@example.com" => 7901 }, "proxy_host" => "192.0.2.10",
                "codex" => dict! { "ports" => dict! { "dave@example.com" => 7905 } },
            }
        };
        for spec in ["socks5://127.0.0.1:7901", "http://192.0.2.10:7901", "http://localhost:7905"] {
            let e = route(Some(spec), None, Some(c())).unwrap_err();
            assert!(e.is("MailFatal") && mail_error(&e).unwrap().1.contains("是账号的出口，Gmail 不能经它连。"), "{spec}");
        }
        assert_eq!(route(Some("socks5://127.0.0.1:7897"), None, Some(c())).unwrap(), via(Via::Socks5, "127.0.0.1", 7897));
        // 这一次的 CCA_PROXY（账号的出口）就是它：同一个主机和端口，写成 socks5 还是 http 都不行
        let sysp = || Some(mail_proxy_parse(SCUTIL));
        let e = route_with(Some("socks5://127.0.0.1:7897"), &c(), sysp(), "http://127.0.0.1:7897").unwrap_err();
        assert!(e.is("MailFatal"));
        assert_eq!(
            mail_error(&e),
            Some(("proxy", format!("代理 127.0.0.1:7897 是账号的出口，Gmail 不能经它连。{MAIL_PROXY_HINT}").as_str()))
        );
        // 别的端口、别的主机、写坏了的 CCA_PROXY 不算
        for other in ["http://127.0.0.1:7898", "http://192.0.2.10:7897", "", "http://127.0.0.1:x7897", "nonsense"] {
            assert_eq!(
                route_with(Some("socks5://127.0.0.1:7897"), &c(), sysp(), other).unwrap(),
                via(Via::Socks5, "127.0.0.1", 7897),
                "{other}"
            );
        }
        assert!(
            mail_route_exit(&via(Via::Http, "LOCALHOST", 7897), &Dict::new(), "socks5://localhost:7897").unwrap(),
            "主机名不分大小写"
        );
        // 手改坏了的 config.json：原来是没人接的 AttributeError
        let bad = dict! { "ports" => crate::list![7901] };
        assert!(route(Some("socks5://127.0.0.1:7901"), None, Some(bad)).unwrap_err().is("AttributeError"));
        let bad = dict! { "router" => "192.0.2.10" };
        assert!(route(Some("socks5://127.0.0.1:7901"), None, Some(bad)).unwrap_err().is("AttributeError"));
    }

    /// tests/mail_view.py 的 Wire.test_tls_is_always_verified 的后一半（「HOME 是用户真正的主目录：只给测试的 CA 不认」）：
    /// 那一条在外面测不了——辅助程序要在真的主目录下跑才走得到。这里钉住认不认的规则；不认的时候握手确实过不了，
    /// tests/mail_view.py 的同名用例用空的 CCA_MAIL_CAFILE 测
    #[test]
    fn test_ca_only_in_a_swapped_home_and_to_this_machine() {
        fn ip(s: &'static str) -> impl FnOnce() -> Option<IpAddr> {
            move || Some(s.parse().unwrap())
        }
        let ca = Some("/tmp/test-ca.pem");
        assert_eq!(test_ca(ca, || true, ip("127.0.0.1")), ca);
        assert_eq!(test_ca(ca, || true, ip("::1")), ca);
        assert_eq!(test_ca(ca, || false, ip("127.0.0.1")), None, "HOME 是用户真正的主目录：不认");
        assert_eq!(test_ca(ca, || false, ip("::1")), None);
        for far in ["192.0.2.10", "127.0.0.2", "::ffff:127.0.0.1", "2001:db8::1"] {
            assert_eq!(test_ca(ca, || true, ip(far)), None, "连的不是本机（{far}）：不认");
        }
        assert_eq!(test_ca(ca, || true, || None), None, "问不出对方是谁：不认");
        assert_eq!(test_ca(Some(""), || true, ip("127.0.0.1")), None);
        assert_eq!(test_ca(None, || true, ip("127.0.0.1")), None);
        // 没给这个变量时不去问 HOME、不去问对方
        assert_eq!(test_ca(None, || panic!("不该问"), || panic!("不该问")), None);
        assert_eq!(test_ca(Some(""), || panic!("不该问"), || panic!("不该问")), None);
        assert_eq!(test_ca(ca, || false, || panic!("不该问")), None);
    }

    #[test]
    fn fnmatch_like_python() {
        let cases = vectors("fnmatch");
        assert!(cases.len() > 400);
        for c in cases {
            let (name, pat) = (c.at("name").py_str(), c.at("pat").py_str());
            assert_eq!(fnmatchcase(&name, &pat).unwrap(), c.at("ok").truthy(), "fnmatchcase({name:?}, {pat:?})");
        }
    }

    #[test]
    fn route_notes() {
        assert_eq!(mail_route_note(&Route::direct()), "直连");
        assert_eq!(mail_route_note(&via(Via::Socks5, "127.0.0.1", 7897)), "经代理 127.0.0.1:7897，SOCKS5");
        assert_eq!(mail_route_note(&via(Via::Http, "proxy.example.com", 8080)), "经代理 proxy.example.com:8080，HTTP CONNECT");
    }

    /// 本机起一个假代理：照 script 回答，返回它收到的字节
    fn fake_proxy(reply: Vec<Vec<u8>>, hang: bool) -> (u16, std::thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            conn.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
            let mut seen = Vec::new();
            let mut buf = [0u8; 1024];
            for r in reply {
                if let Ok(n) = conn.read(&mut buf) {
                    seen.extend_from_slice(&buf[..n]);
                }
                let _ = conn.write_all(&r);
            }
            if hang {
                std::thread::sleep(Duration::from_millis(800));
            }
            while let Ok(n) = conn.read(&mut buf) {
                if n == 0 {
                    break;
                }
                seen.extend_from_slice(&buf[..n]);
            }
            seen
        });
        (port, h)
    }

    fn dial_err(v: Via, reply: Vec<Vec<u8>>, hang: bool, timeout: f64) -> (String, String) {
        let (port, h) = fake_proxy(reply, hang);
        let e = mail_dial(&via(v, "127.0.0.1", port), timeout).unwrap_err();
        let _ = h.join();
        let (code, text) = mail_error(&e).unwrap_or_else(|| panic!("{e:?}"));
        assert!(e.is("MailRetry"));
        (code.to_string(), text.replace(&port.to_string(), "PORT"))
    }

    #[test]
    fn socks5_handshake_bytes_and_errors() {
        let (port, h) = fake_proxy(vec![b"\x05\x00".to_vec(), b"\x05\x00\x00\x01\x7f\x00\x00\x01\x1f\x90".to_vec()], false);
        let s = mail_dial(&via(Via::Socks5, "127.0.0.1", port), 2.0).unwrap();
        drop(s);
        assert_eq!(h.join().unwrap(), b"\x05\x01\x00\x05\x01\x00\x03\x0eimap.gmail.com\x03\xe1");
        // 回答里的地址是域名、IPv6 的也认
        for bound in [b"\x05\x00\x00\x03\x03abc\x00\x50".to_vec(), [b"\x05\x00\x00\x04".to_vec(), vec![0; 18]].concat()] {
            let (port, h) = fake_proxy(vec![b"\x05\x00".to_vec(), bound], false);
            assert!(mail_dial(&via(Via::Socks5, "127.0.0.1", port), 2.0).is_ok());
            let _ = h.join();
        }
        assert_eq!(
            dial_err(Via::Socks5, vec![b"\x05\xff".to_vec()], false, 2.0),
            ("proxy".into(), "SOCKS5 代理要认证，或者不认这个请求（cca 只用不带认证的 SOCKS5）".into())
        );
        assert_eq!(
            dial_err(Via::Socks5, vec![b"\x05\x00".to_vec(), b"\x05\x05\x00\x01\x00\x00\x00\x00\x00\x00".to_vec()], false, 2.0),
            ("proxy".into(), "SOCKS5 代理没能连上 imap.gmail.com:993（错误码 5）".into())
        );
        assert_eq!(
            dial_err(Via::Socks5, vec![b"\x05\x00".to_vec(), b"\x05\x00\x00\x09".to_vec()], false, 2.0),
            ("proxy".into(), "SOCKS5 代理的回应认不出".into())
        );
        assert_eq!(dial_err(Via::Socks5, vec![b"\x05".to_vec()], false, 2.0), ("proxy".into(), "代理断开了连接".into()));
        assert_eq!(
            dial_err(Via::Socks5, vec![], true, 0.2),
            ("proxy".into(), "代理 127.0.0.1:PORT 没有接通 Gmail（timed out）".into())
        );
    }

    #[test]
    fn http_connect_bytes_and_errors() {
        let (port, h) = fake_proxy(vec![b"HTTP/1.1 200 Connection established\r\n\r\n".to_vec()], false);
        let s = mail_dial(&via(Via::Http, "127.0.0.1", port), 2.0).unwrap();
        drop(s);
        assert_eq!(h.join().unwrap(), b"CONNECT imap.gmail.com:993 HTTP/1.1\r\nHost: imap.gmail.com:993\r\n\r\n");
        for ok in ["HTTP/1.0 200\r\n\r\n", "HTTP/1.1 200-ok\r\nX: y\r\n\r\n"] {
            let (port, h) = fake_proxy(vec![ok.as_bytes().to_vec()], false);
            assert!(mail_dial(&via(Via::Http, "127.0.0.1", port), 2.0).is_ok(), "{ok:?}");
            let _ = h.join();
        }
        assert_eq!(
            dial_err(Via::Http, vec![b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n".to_vec()], false, 2.0),
            ("proxy".into(), "HTTP 代理不肯连 imap.gmail.com:993（HTTP/1.1 403 Forbidden）".into())
        );
        for bad in ["HTTP/1.1 2000 x", "HTTP/1.1 200_x", "HTTP/2 200", "http/1.1 200 ok", " HTTP/1.1 200"] {
            let (code, text) = dial_err(Via::Http, vec![format!("{bad}\r\n\r\n").into_bytes()], false, 2.0);
            assert_eq!((code.as_str(), text), ("proxy", format!("HTTP 代理不肯连 imap.gmail.com:993（{bad}）")), "{bad:?}");
        }
        let long = format!("HTTP/1.1 500 {}\x01\u{e9}\r\n\r\n", "x".repeat(100));
        let (_, text) = dial_err(Via::Http, vec![long.into_bytes()], false, 2.0);
        assert_eq!(text, format!("HTTP 代理不肯连 imap.gmail.com:993（HTTP/1.1 500 {}）", "x".repeat(67)));
        assert_eq!(dial_err(Via::Http, vec![vec![b'x'; 9000]], false, 2.0), ("proxy".into(), "HTTP 代理的回应太长".into()));
        assert_eq!(dial_err(Via::Http, vec![b"HTTP/1.1 200".to_vec()], false, 2.0), ("proxy".into(), "代理断开了连接".into()));
    }

    #[test]
    fn cannot_reach_the_proxy() {
        // 没人听的端口
        let free = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let e = mail_dial(&via(Via::Socks5, "127.0.0.1", free), 2.0).unwrap_err();
        assert_eq!(mail_error(&e), Some(("net", format!("连不上 代理 127.0.0.1:{free}（Connection refused）").as_str())));
        // 主机名编不了码：原来是没人接的 UnicodeEncodeError
        assert!(mail_dial(&via(Via::Http, "a..b", 80), 1.0).unwrap_err().is("UnicodeEncodeError"));
        assert!(mail_dial(&via(Via::Http, &format!("{}.example", "x".repeat(64)), 80), 1.0).unwrap_err().is("UnicodeEncodeError"));
    }

    #[test]
    fn io_errors_like_python() {
        assert_eq!(
            io_text(&io::Error::from_raw_os_error(libc::ECONNREFUSED)),
            ("Connection refused".into(), "ConnectionRefusedError")
        );
        assert_eq!(
            io_text(&io::Error::from_raw_os_error(libc::ECONNRESET)),
            ("Connection reset by peer".into(), "ConnectionResetError")
        );
        assert_eq!(io_text(&io::Error::from_raw_os_error(libc::EPIPE)), ("Broken pipe".into(), "BrokenPipeError"));
        assert_eq!(io_text(&io::Error::from(io::ErrorKind::WouldBlock)).0, "timed out");
        assert_eq!(io_text(&io::Error::from(io::ErrorKind::TimedOut)).0, "timed out");
        assert_eq!(io_text(&io::Error::from_raw_os_error(libc::ETIMEDOUT)).0, "Operation timed out");
    }
}
