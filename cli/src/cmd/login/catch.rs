//! login-catch（Mac 上，cca login-page）：等浏览器带着 code、state 跳回本机的那个小 HTTP 服务。
//!
//! 套接字直接用 C 库的 socket / setsockopt / bind / listen / accept：127.0.0.1 和 ::1 各听一个，都带 SO_REUSEADDR，IPv6 的
//! 带 IPV6_V6ONLY，等待队列 8，和原来 Python 的 socket 模块做的一样。

use super::sys::{interrupted, run_browser, trap, NetErr, PySock, Stop};
use super::{ascii_encode_error, ascii_only, login_target, q_get, q_some};
use crate::cmd::{arg, arity};
use crate::core::tool::t;
use crate::py::num::floordiv;
use crate::py::obj::repr_str;
use crate::py::proc::{Cmd as Proc, Out};
use crate::py::text;
use crate::py::time::time;
use crate::py::url::{parse_qsl, urlsplit};
use crate::py::{die, exit, Fail, R};
use crate::out;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

// 回给浏览器的那一页（原来的 LOGIN_PAGE）：标题出现两次，正文接在最后
const PAGE_HEAD: &str = "<!doctype html><html lang=zh-CN><meta charset=utf-8><title>cca：";
const PAGE_STYLE: &str = "</title>\n\
<style>body{font:15px/1.6 -apple-system,BlinkMacSystemFont,sans-serif;max-width:40em;margin:4em auto;padding:0 1.2em;color:#222}\n\
pre{white-space:pre-wrap;word-break:break-all;background:#f2f2f2;padding:.7em .9em;border-radius:6px;font-size:13px}\n\
@media (prefers-color-scheme:dark){body{background:#1e1e1e;color:#ddd}pre{background:#2c2c2c}}</style>\n\
<h2>";
const PAGE_BODY: &str = "</h2>";

/// `html.escape(s)`（引号也转）
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#x27;")
}

/// 回一页给浏览器；发不出去（对方走了、10 秒没发完）不管
fn login_catch_reply(conn: &PySock, status: &str, title: &str, body: &str) -> Result<(), Stop> {
    let title = html_escape(title);
    let page = format!("{PAGE_HEAD}{title}{PAGE_STYLE}{title}{PAGE_BODY}{body}");
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        page.len()
    );
    match conn.sendall(format!("{head}{page}").as_bytes()) {
        Err(NetErr::Interrupted) => Err(Stop::Interrupt),
        _ => Ok(()),
    }
}

/// 一个请求读出来的结果
#[derive(Debug, PartialEq)]
enum Caught {
    /// 不是这次登录的回调（已经回了 404 / 400，或者根本不像个请求）
    Nothing,
    /// 是回调，登录页说没成：原因
    Refused(String),
    /// 是回调：code
    Code(String),
}

/// 读一个请求：是这次登录的回调（路径、state 都对）时返回 code 或者登录页说的原因，别的请求（favicon、对不上的）回 404 / 400
fn login_catch_one(conn: &PySock, path: &str, state: &str) -> Result<Caught, Stop> {
    let mut data: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    while !data.windows(4).any(|w| w == b"\r\n\r\n") && data.len() < 16384 {
        match conn.recv(&mut buf) {
            Ok(0) => break,
            Ok(n) => data.extend_from_slice(&buf[..n]),
            Err(NetErr::Interrupted) => return Err(Stop::Interrupt),
            Err(_) => return Ok(Caught::Nothing), // 10 秒没等到、连接断了
        }
    }
    // 请求行（iso-8859-1：一个字节一个字符），按单个空格切
    let first = data.windows(2).position(|w| w == b"\r\n").map_or(&data[..], |i| &data[..i]);
    let line: String = first.iter().map(|b| *b as char).collect();
    let parts: Vec<&str> = line.split(' ').collect();
    if parts.len() < 2 || parts[0] != "GET" {
        return Ok(Caught::Nothing);
    }
    let u = urlsplit(parts[1]).ok();
    let q = u.as_ref().map(|u| parse_qsl(&u.query, true)).unwrap_or_default();
    if !u.is_some_and(|u| u.path == path) {
        login_catch_reply(conn, "404 Not Found", "找不到", "")?;
        return Ok(Caught::Nothing);
    }
    if q_get(&q, "state") != Some(state) {
        login_catch_reply(conn, "400 Bad Request", "不是这一次的登录", "<p>这一页不是这一次登录跳回来的。回到终端看看 cca login-page 有没有在运行。</p>")?;
        return Ok(Caught::Nothing);
    }
    match q_some(&q, "code") {
        Some(code) if q_some(&q, "error").is_none() => Ok(Caught::Code(code.to_string())),
        _ => {
            let why = q_some(&q, "error_description").or(q_some(&q, "error")).unwrap_or("no code");
            Ok(Caught::Refused(ascii_only(why, Some('?'), 200)))
        }
    }
}

/// 在本机的一个地址（127.0.0.1 或 ::1）上占住端口、开始听
fn listen_on(v6: bool, port: u16) -> io::Result<OwnedFd> {
    // 和 Python 的套接字一样不留给子进程：Linux 上建的时候就带上，别的系统紧跟着设
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let kind = libc::SOCK_STREAM | libc::SOCK_CLOEXEC;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let kind = libc::SOCK_STREAM;
    let check = |rc: libc::c_int| if rc < 0 { Err(io::Error::last_os_error()) } else { Ok(rc) };
    let one: libc::c_int = 1;
    let one_ptr = (&raw const one).cast::<libc::c_void>();
    let one_len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: 建的描述符马上交给 OwnedFd 管；setsockopt 的值、bind 的地址都是这里的局部变量，长度如实给出；
    // sockaddr_in / sockaddr_in6 是纯数据，全 0 是合法的值
    unsafe {
        let s = OwnedFd::from_raw_fd(check(libc::socket(if v6 { libc::AF_INET6 } else { libc::AF_INET }, kind, 0))?);
        let fd = s.as_raw_fd();
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        check(libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC))?;
        check(libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, one_ptr, one_len))?;
        if v6 {
            check(libc::setsockopt(fd, libc::IPPROTO_IPV6, libc::IPV6_V6ONLY, one_ptr, one_len))?;
            let mut a: libc::sockaddr_in6 = std::mem::zeroed();
            a.sin6_family = libc::AF_INET6 as libc::sa_family_t;
            a.sin6_port = port.to_be();
            a.sin6_addr.s6_addr = std::net::Ipv6Addr::LOCALHOST.octets();
            check(libc::bind(fd, (&raw const a).cast(), std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t))?;
        } else {
            let mut a: libc::sockaddr_in = std::mem::zeroed();
            a.sin_family = libc::AF_INET as libc::sa_family_t;
            a.sin_port = port.to_be();
            a.sin_addr.s_addr = u32::from(std::net::Ipv4Addr::LOCALHOST).to_be();
            check(libc::bind(fd, (&raw const a).cast(), std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t))?;
        }
        check(libc::listen(fd, 8))?;
        Ok(s)
    }
}

/// `s.accept()[0]`：接下来的连接也不留给子进程
fn accept(listener: &OwnedFd) -> Result<io::Result<OwnedFd>, Stop> {
    loop {
        // SAFETY: 不要对方的地址（两个空指针）；拿到的描述符马上交给 OwnedFd 管
        let fd = unsafe { libc::accept(listener.as_raw_fd(), std::ptr::null_mut(), std::ptr::null_mut()) };
        if fd >= 0 {
            // SAFETY: fd 是刚接下来的、没有别人用的套接字
            let conn = unsafe { OwnedFd::from_raw_fd(fd) };
            // SAFETY: 只是给自己的描述符设一个标志
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
            return Ok(Ok(conn));
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Ok(Err(e));
        }
        if interrupted() {
            return Err(Stop::Interrupt);
        }
    }
}

/// `select.select(socks, [], [], secs)[0]`：有连接在等的那几个监听（先后照 socks）
fn select(socks: &[OwnedFd], secs: f64) -> Result<Vec<&OwnedFd>, Stop> {
    let mut fds: Vec<libc::pollfd> = socks.iter().map(|s| libc::pollfd { fd: s.as_raw_fd(), events: libc::POLLIN, revents: 0 }).collect();
    let ms = (secs * 1000.0).ceil().min(i32::MAX as f64) as i32;
    // SAFETY: fds 是有效的数组，个数如实给出
    let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) };
    if n < 0 {
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e.into());
        }
        if interrupted() {
            return Err(Stop::Interrupt);
        }
        return Ok(Vec::new()); // 别的信号打断的：回去照剩下的时间接着等
    }
    Ok(socks.iter().zip(&fds).filter(|(_, p)| p.revents != 0).map(|(s, _)| s).collect())
}

/// `'%d' % (int(secs) // 60)`：等了几分钟。secs 是 float(超时的参数)，什么都可能
fn whole_minutes(secs: f64) -> R<String> {
    if secs.is_nan() {
        return Err(Fail::Value("cannot convert float NaN to integer".into()));
    }
    if secs.is_infinite() {
        return Err(Fail::Other("OverflowError", vec!["OverflowError: cannot convert float infinity to integer".into()]));
    }
    let whole = secs.trunc();
    if whole.abs() < 9.0e18 {
        return Ok(floordiv(whole as i64, 60).to_string());
    }
    // i64 放不下的：Python 的整数没有上限，照十进制的写法算。小数这么大时本身就是整数：尾数 × 2 的若干次方
    let bits = whole.abs().to_bits();
    let (mantissa, shift) = ((bits & ((1u64 << 52) - 1)) | (1u64 << 52), (bits >> 52) as i32 - 1075);
    let mut digits: Vec<u8> = mantissa.to_string().bytes().map(|b| b - b'0').collect(); // 高位在前
    for _ in 0..shift {
        let mut carry = 0;
        for d in digits.iter_mut().rev() {
            let x = *d * 2 + carry;
            (*d, carry) = (x % 10, x / 10);
        }
        if carry > 0 {
            digits.insert(0, carry);
        }
    }
    // 除以 60（向下取整：负数有余数时再往下一个）
    let mut rem: u32 = 0;
    let mut quotient: Vec<u8> = Vec::with_capacity(digits.len());
    for d in &digits {
        let x = rem * 10 + *d as u32;
        quotient.push((x / 60) as u8);
        rem = x % 60;
    }
    if whole < 0.0 && rem != 0 {
        let mut carried = false;
        for d in quotient.iter_mut().rev() {
            if *d == 9 {
                *d = 0;
            } else {
                *d += 1;
                carried = true;
                break;
            }
        }
        if !carried {
            quotient.insert(0, 1);
        }
    }
    let text: String = quotient.iter().skip_while(|d| **d == 0).map(|d| (b'0' + d) as char).collect();
    Ok(format!("{}{}", if whole < 0.0 { "-" } else { "" }, text))
}

/// 等浏览器跳回来的那个循环：拿到回调（或者出错、到点）才返回，返回的都是怎么退出
fn wait_for_callback(socks: &[OwnedFd], path: &str, state: &str, secs: f64) -> Result<(), Stop> {
    let end = time() + secs;
    loop {
        // 信号刚好落在两次等待之间时，没有哪个系统调用被打断：每一轮先看一眼（原来是到下一条字节码就抛出来）
        if interrupted() {
            return Err(Stop::Interrupt);
        }
        let left = end - time();
        if left <= 0.0 {
            return Err(Fail::Die(format!("等了 {} 分钟还没登录完：要登录的话在服务器上重新开始", whole_minutes(secs)?)).into());
        }
        if left.is_nan() {
            // select 的时限 min(nan, 5) 还是 nan：没人接的 ValueError
            return Err(Fail::Value("Invalid value NaN (not a number)".into()).into());
        }
        for s in select(socks, left.min(5.0))? {
            let Ok(conn) = accept(s)? else { continue };
            // conn.settimeout(10)
            let Ok(conn) = PySock::new(conn, 10.0) else { continue };
            match login_catch_one(&conn, path, state)? {
                Caught::Nothing => continue,
                Caught::Refused(why) => {
                    let body = format!("<p>登录页说：<code>{}</code></p><p>回到服务器的终端直接回车取消，再重新登录一遍。</p>", html_escape(&why));
                    login_catch_reply(&conn, "200 OK", "没有登录成功", &body)?;
                    return Err(Fail::Die(format!("登录页说没有登录成功：{why}")).into());
                }
                Caught::Code(code) => {
                    let text = format!("{code}#{state}");
                    // text.encode('ascii')：验证码里有不是 ASCII 的字符是没人接的 UnicodeEncodeError（浏览器那边连回应都没有）
                    if let Some(e) = ascii_encode_error(&text) {
                        return Err(e.into());
                    }
                    // 放进剪贴板；没有 pbcopy、10 秒没做完、退出码不是 0 都算没复制成
                    let copied = matches!(Proc::new(["pbcopy"]).input(text.as_bytes()).stdout(Out::Null).stderr(Out::Null).timeout(10.0).run(), Ok(done) if done.rc == 0);
                    if interrupted() {
                        return Err(Stop::Interrupt);
                    }
                    let body = format!(
                        "<p>{}回到服务器的终端，粘贴、回车，服务器上就登录好了。之后可以关掉这一页。</p><pre>{}</pre>",
                        if copied { "验证码已经复制到剪贴板（就是下面这一行）：" } else { "复制下面这一行验证码，" },
                        html_escape(&text)
                    );
                    login_catch_reply(&conn, "200 OK", "这边登录好了，还差一步", &body)?;
                    out!("{text}");
                    return Err(Fail::Exit(if copied { 0 } else { 3 }).into());
                }
            }
        }
    }
}

/// cca login-page（Mac 上）：先占住登录网址里回调的端口（127.0.0.1，能占的话还有 ::1），再回调 cca _browser 在账号专属的
/// Chrome 里打开登录页（走账号的出口；回调地址是本机，Chrome 不经代理），等浏览器带着 code、state 跳回来：放进剪贴板，给浏览器
/// 一页说明，把 code#state 打到 stdout。退出码：0 拿到了、复制好了；3 拿到了、没复制成；4 端口被别的程序占着（照样打开了登录页，
/// stdout 是端口，bash 说登录后从地址栏复制整个网址贴回服务器）；1 没拿到，原因打到 stderr
pub fn cmd_login_catch(args: &[String]) -> R<()> {
    arity("cmd_login_catch", args, 1, 2)?;
    let (url, timeout) = (args[0].as_str(), arg(args, 1, "900"));
    let Some((_, port, path, state)) = login_target(url) else {
        return die(format!("这不是 {} 的登录网址", t().label));
    };
    let mut socks: Vec<OwnedFd> = Vec::new();
    for v6 in [false, true] {
        match listen_on(v6, port) {
            Ok(s) => socks.push(s),
            Err(_) if !v6 => break, // 127.0.0.1 上占不住：::1 也不试了
            Err(_) => {}
        }
    }
    match run_browser(url) {
        Ok(0) => {}
        Ok(_) => return die("没能在账号的浏览器里打开登录页"),
        Err(Stop::Fail(e)) => return Err(e),
        Err(Stop::Interrupt) => return exit(130), // 这时还没装信号处理函数，走不到
    }
    if socks.is_empty() {
        // bash 照这个端口说怎么办
        out!("{port}");
        return exit(4);
    }
    let Some(secs) = text::to_float(timeout) else {
        return Err(Fail::Value(format!("could not convert string to float: {}", repr_str(timeout))));
    };
    // Ctrl-C：说一声已取消（原来的 except KeyboardInterrupt）
    trap(libc::SIGINT, true);
    match wait_for_callback(&socks, &path, &state, secs) {
        Ok(()) => Ok(()),
        Err(Stop::Fail(e)) => Err(e),
        Err(Stop::Interrupt) => die("已取消"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::py::time::monotonic;
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpStream};

    #[test]
    fn escaping_and_minutes() {
        // 右边是 Python 的 html.escape()
        assert_eq!(html_escape("a&b<c>d\"e'f"), "a&amp;b&lt;c&gt;d&quot;e&#x27;f");
        assert_eq!(html_escape("&amp;"), "&amp;amp;");
        assert_eq!(html_escape("找不到 code#state"), "找不到 code#state");
        // 右边是 Python 的 '%d' % (int(float(s)) // 60)
        for (s, want) in [
            ("900", "15"), ("30", "0"), ("59.9", "0"), ("60", "1"), ("119", "1"), ("120.5", "2"), ("0", "0"), ("-0", "0"), ("-1", "-1"), ("-59", "-1"), ("-60", "-1"),
            ("-61", "-2"), ("-90", "-2"), ("1e3", "16"), (" 12 ", "0"), ("1_000", "16"), ("0.5", "0"), ("-0.5", "0"), ("1e19", "166666666666666666"),
            ("-1e19", "-166666666666666667"), ("-3.7e25", "-616666666666666647372869"), ("-9223372036854775808", "-153722867280912931"),
            ("-9.3e18", "-155000000000000000"), ("9007199254740993", "150119987579016"), ("-123456789012345678901234567890", "-2057613150205761297961993285"),
            ("-1e300", "-16666666666666667541746004253407004145074476351802652581930901925196707633148469929772856251340797734061740730548064636282375387256007176260746536413111783047473120015442930062297170563246468167656149220582846665751351983982794014667910879046335708242987646480334280713968594491203273114424323342336"),
        ] {
            assert_eq!(whole_minutes(text::to_float(s).unwrap()).unwrap(), want, "{s}");
        }
        assert!(matches!(whole_minutes(f64::NEG_INFINITY), Err(Fail::Other("OverflowError", _))));
        assert!(matches!(whole_minutes(f64::NAN), Err(Fail::Value(_))));
        assert_eq!((text::to_float("abc"), text::to_float("")), (None, None));
    }

    /// 在一个空着的端口上听，返回端口和监听（127.0.0.1 的，这台机器有 IPv6 的话还有 ::1 的）
    fn listeners() -> (u16, Vec<OwnedFd>) {
        let has_v6 = std::net::TcpListener::bind("[::1]:0").is_ok();
        for port in 41000..41400 {
            match (listen_on(false, port), listen_on(true, port)) {
                (Ok(a), Ok(b)) => return (port, vec![a, b]),
                (Ok(a), Err(_)) if !has_v6 => return (port, vec![a]),
                _ => {}
            }
        }
        panic!("找不到空着的端口");
    }

    fn ask(addr: &str, request: &[u8]) -> String {
        let mut c = TcpStream::connect(addr).unwrap();
        c.write_all(request).unwrap();
        c.shutdown(Shutdown::Write).unwrap();
        let mut reply = String::new();
        c.read_to_string(&mut reply).unwrap();
        reply
    }

    #[test]
    fn listens_on_both_loopbacks_and_answers() {
        let (port, socks) = listeners();
        // 同一个端口再占一次占不上
        assert_eq!(listen_on(false, port).unwrap_err().raw_os_error(), Some(libc::EADDRINUSE));
        let serve = |n: usize| -> Vec<Caught> {
            let mut got = Vec::new();
            while got.len() < n {
                for s in select(&socks, 5.0).unwrap() {
                    let conn = PySock::new(accept(s).unwrap().unwrap(), 10.0).unwrap();
                    got.push(login_catch_one(&conn, "/cb", "st-1").unwrap());
                }
            }
            got
        };
        std::thread::scope(|scope| {
            let has_v6 = socks.len() == 2;
            let client = scope.spawn(move || {
                let v4 = format!("127.0.0.1:{port}");
                let v6 = if has_v6 { format!("[::1]:{port}") } else { v4.clone() };
                vec![
                    ask(&v4, b"GET /favicon.ico HTTP/1.1\r\n\r\n"),
                    ask(&v6, b"GET /cb?code=x&state=wrong HTTP/1.1\r\n\r\n"),
                    ask(&v4, b"POST /cb?code=x&state=st-1 HTTP/1.1\r\n\r\n"),
                    ask(&v4, b"GE"),
                    ask(&v6, b"GET /cb?code=good%2Fcode&state=st-1&state=st-1 HTTP/1.1\r\nHost: x\r\n\r\n"),
                    ask(&v4, b"GET /cb?error=access_denied&error_description=%E4%B8%8D%E8%A1%8C+no&state=st-1"),
                    ask(&v4, b"GET /cb?state=st-1 HTTP/1.1\r\n\r\n"),
                ]
            });
            let got = serve(7);
            assert_eq!(
                got,
                [
                    Caught::Nothing,
                    Caught::Nothing,
                    Caught::Nothing,
                    Caught::Nothing,
                    Caught::Code("good/code".into()),
                    Caught::Refused("?? no".into()),
                    Caught::Refused("no code".into()),
                ]
            );
            let replies = client.join().unwrap();
            assert!(replies[0].starts_with("HTTP/1.1 404 Not Found\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: "), "{}", replies[0]);
            assert!(replies[0].ends_with("<h2>找不到</h2>") && replies[0].contains("\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n<!doctype html>"));
            assert!(replies[1].starts_with("HTTP/1.1 400 Bad Request\r\n") && replies[1].ends_with("<p>这一页不是这一次登录跳回来的。回到终端看看 cca login-page 有没有在运行。</p>"));
            // 不是 GET、半截的请求：什么都不回，直接关
            assert_eq!((replies[2].as_str(), replies[3].as_str()), ("", ""));
            // 是回调的：login_catch_one 自己不回（等复制完由外面回）
            assert_eq!((replies[4].as_str(), replies[5].as_str(), replies[6].as_str()), ("", "", ""));
            // Content-Length 是字节数
            let (head, page) = replies[0].split_once("\r\n\r\n").unwrap();
            assert!(head.contains(&format!("Content-Length: {}\r\n", page.len())));
        });
        // 没有连接来：等到点，空手回来
        let t0 = monotonic();
        assert!(select(&socks, 0.2).unwrap().is_empty());
        assert!((0.15..20.0).contains(&(monotonic() - t0)));
    }
}
