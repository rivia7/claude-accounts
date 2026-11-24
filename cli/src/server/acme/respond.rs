//! http-01 的应答：证书机构从公网来取 `/.well-known/acme-challenge/<令牌>`，回“令牌.账户公钥的摘要”。放的地方有两种（Acme:182-235）：
//! - `Responder`：自己听一个端口（平常是 80）。别的一律 404，不记日志
//! - `Webroot`：80 端口上已经有别的网站服务时，把文件写进它的目录；文件只在验证的那一会儿在
//!
//! Python 的 Responder 是标准库的 http.server。这里没有那一层：`answer` 是照它（CPython 3.14）对各种请求的回法写的，一个字节不差
//! ——对照数据 cli/tests/vectors/server_acme_responder.json 是真的跑它录下来的。证书机构只发最普通的 GET，但这个端口对着公网，
//! 什么都会收到；回得和以前一样，省得一条条想哪种不一样了要不要紧。
//!
//! 和 Python 不一样的（都是为了公网上有人捣乱时不被拖住）：
//! - 同时最多 64 个连接、同一个 IP 最多 8 个，多了的直接断开（Python 来一个起一个线程，没有上限）
//! - 读一个请求一共最多等 15 秒（Python 是每读一次最多等 15 秒：隔十几秒送一个字节就能一直占着一个线程）
//! - 连接上出了错不往 stderr 说（Python 的 socketserver 会打一段 traceback）
//! - host 写的是 IPv6 的地址时听 IPv6（Python 的 HTTPServer 只开 IPv4 的，那样写起不来）

use super::files::makedirs;
use super::lines::Lines;
use super::{token_re, Trouble};
use crate::py::fail::os_error_text;
use crate::py::fs::{chmod, open_error_text};
use crate::py::obj::repr_str;
use crate::py::{path, text, time};
use crate::server::limits::Slots;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// 请求的第一行、每一行头最多这么长（http.server 的 65536）
const MAX_LINE: usize = 65536;
/// 头最多这么多行（http.client 的 _MAXHEADERS；结尾那个空行也算一行）
const MAX_HEADERS: usize = 100;
/// 读一个请求一共最多等这么久，写回应每一步最多等这么久（Acme:222 的 `timeout = 15`）
const TIMEOUT: Duration = Duration::from_secs(15);
/// 证书机构来取的路径的开头（Acme:228）
const PREFIX: &str = "/.well-known/acme-challenge/";
/// 400 那一页上的说明（http.server 的 responses[400][1]）
const BAD_REQUEST: &str = "Bad request syntax or unsupported method";

/// 放 http-01 的内容的地方：`Responder` 或者 `Webroot`（Python 里是两个有 put、drop 的类）
pub trait Challenges {
    /// 放上：证书机构来取这个令牌时回 body
    fn put(&self, token: &str, body: &[u8]) -> Result<(), Trouble>;
    /// 拿掉（验证完了，或者没成）。Python 里叫 `drop`
    fn remove(&self, token: &str);
}

/// 出错的那一页（http.server 的 DEFAULT_ERROR_MESSAGE）；message、explain 里的 & < > 转义
fn error_page(code: u16, message: &str, explain: &str) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        r#"<!DOCTYPE HTML>
<html lang="en">
    <head>
        <meta charset="utf-8">
        <style type="text/css">
            :root {{
                color-scheme: light dark;
            }}
        </style>
        <title>Error response</title>
    </head>
    <body>
        <h1>Error response</h1>
        <p>Error code: {code}</p>
        <p>Message: {}.</p>
        <p>Error code explanation: {code} - {}.</p>
    </body>
</html>
"#,
        esc(message),
        esc(explain)
    )
}

/// 状态行和开头的两行头（send_response）。状态行里的说明按 latin-1 写出去（一个字一个字节）：里面的字都是从请求里按
/// latin-1 读进来的，或者是 ASCII，写得回去
fn head(code: u16, message: &str, now: f64) -> Vec<u8> {
    let mut out = b"HTTP/1.0 ".to_vec();
    out.extend(format!("{code} {message}\r\n").chars().map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?')));
    out.extend_from_slice(b"Server: cca-acme \r\n");
    out.extend_from_slice(format!("Date: {}\r\n", time::strftime_utc("%a, %d %b %Y %H:%M:%S GMT", now)).as_bytes());
    out
}

/// 出错的回应（send_error）。headers：带不带状态行和头（对面说的是 HTTP/0.9、或者第一行就没看懂时不带）；
/// page：带不带那一页（HEAD 不带）
fn failed(now: f64, headers: bool, page: bool, code: u16, message: &str, explain: &str) -> Vec<u8> {
    let body = error_page(code, message, explain);
    let mut out = Vec::new();
    if headers {
        out = head(code, message, now);
        out.extend_from_slice(b"Connection: close\r\nContent-Type: text/html;charset=utf-8\r\n");
        out.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
    }
    if page {
        out.extend_from_slice(body.as_bytes());
    }
    out
}

/// 第一行最后那个词是不是 `HTTP/<数字>.<数字>`（各 1 到 10 位）：是就给出前一个数和斜杠后面的原文
fn version_of(version: &str) -> Option<(u64, &str)> {
    let base = version.strip_prefix("HTTP/")?;
    let mut parts = base.split('.');
    let (major, minor) = (parts.next()?, parts.next()?);
    let number = |s: &str| (1..=10).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit());
    if parts.next().is_some() || !number(major) || !number(minor) {
        return None;
    }
    Some((major.parse().ok()?, base))
}

/// 读一个请求，给出整个回应的字节（空的：什么都不回，关掉就是）。find 是令牌 → 内容，clock 是现在几点（回应里的 Date）。
/// 一步步对着 http.server 的 handle_one_request、parse_request、send_error 和 Acme:227-235 的 do_GET
pub fn answer<R: Read>(
    conn: &mut Lines<R>,
    find: &dyn Fn(&str) -> Option<Vec<u8>>,
    clock: &dyn Fn() -> f64,
) -> io::Result<Vec<u8>> {
    let raw = conn.line(MAX_LINE.saturating_add(1))?;
    if raw.len() > MAX_LINE {
        return Ok(failed(clock(), true, true, 414, "URI Too Long", "URI is too long"));
    }
    // 按 latin-1 当成字（一个字节一个字），去掉结尾的换行，照空白切开
    let line: String = raw.iter().map(|b| char::from(*b)).collect();
    let line = line.trim_end_matches(['\r', '\n']);
    let words = text::split_ws(line);
    let (Some(command), Some(last)) = (words.first().copied(), words.last().copied()) else {
        return Ok(Vec::new()); // 连上来什么都没说、或者只有空白：不回
    };
    // 三个词以上：最后一个是版本。写的是 HTTP/0.9（或者没写：只有两个词）时回应不带状态行和头
    let mut headers = false;
    if words.len() >= 3 {
        let Some((major, base)) = version_of(last) else {
            return Ok(failed(clock(), false, true, 400, &format!("Bad request version ({})", repr_str(last)), BAD_REQUEST));
        };
        if major >= 2 {
            return Ok(failed(clock(), false, true, 505, &format!("Invalid HTTP version ({base})"), "Cannot fulfill request"));
        }
        headers = last != "HTTP/0.9";
    }
    if !(2..=3).contains(&words.len()) {
        return Ok(failed(clock(), headers, true, 400, &format!("Bad request syntax ({})", repr_str(line)), BAD_REQUEST));
    }
    if words.len() == 2 && command != "GET" {
        let message = format!("Bad HTTP/0.9 request type ({})", repr_str(command));
        return Ok(failed(clock(), headers, true, 400, &message, BAD_REQUEST));
    }
    // 从这里起方法定了：出错的那一页 HEAD 不带
    let page = command != "HEAD";
    // 头：一行行读到空行（或者读到头）。内容不看：Python 看的是 Connection 和 Expect，但它回的是 HTTP/1.0，两样都不起作用
    let mut count = 0usize;
    loop {
        let header = conn.line(MAX_LINE.saturating_add(1))?;
        if header.len() > MAX_LINE {
            let explain = "got more than 65536 bytes when reading header line";
            return Ok(failed(clock(), headers, page, 431, "Line too long", explain));
        }
        count = count.saturating_add(1);
        if count > MAX_HEADERS {
            return Ok(failed(clock(), headers, page, 431, "Too many headers", "got more than 100 headers"));
        }
        if matches!(header.as_slice(), b"\r\n" | b"\n" | b"") {
            break;
        }
    }
    if command != "GET" {
        let message = format!("Unsupported method ({})", repr_str(command));
        return Ok(failed(clock(), headers, page, 501, &message, "Server does not support this operation"));
    }
    // do_GET。路径开头多写的斜杠并成一个（http.server 自己做的）；问号后面的不去掉：带着就不是登记的那个令牌
    let path = words.get(1).copied().unwrap_or("");
    let path = if path.starts_with("//") { format!("/{}", path.trim_start_matches('/')) } else { path.to_string() };
    let token = path.strip_prefix(PREFIX).unwrap_or("");
    let body = if token_re().matches(token) { find(token).unwrap_or_default() } else { Vec::new() };
    let mut out = Vec::new();
    if headers {
        // 内容是空的也算没有（`200 if body else 404`）
        out = if body.is_empty() { head(404, "Not Found", clock()) } else { head(200, "OK", clock()) };
        out.extend_from_slice(format!("Content-Type: text/plain\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes());
    }
    out.extend_from_slice(&body);
    Ok(out)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

type Tokens = Arc<Mutex<HashMap<String, Vec<u8>>>>;

/// 自己听一个端口的那一种（Acme:182-195）
pub struct Responder {
    tokens: Tokens,
    listener: TcpListener,
    port: u16,
    /// 读一个请求最多等多久（测试里换成短的）
    timeout: Duration,
}

impl Responder {
    /// 监听 host:port（端口给 0 是让系统挑一个）；还不收连接，等 `start`
    pub fn bind(host: &str, port: u16) -> Result<Responder, Trouble> {
        let os = |e: io::Error| Trouble::Os(os_error_text(&e));
        let v6 = host.contains(':');
        let addrs: Vec<SocketAddr> = (host, port).to_socket_addrs().map_err(os)?.filter(|a| a.is_ipv6() == v6).collect();
        let listener = TcpListener::bind(addrs.as_slice()).map_err(os)?;
        let port = listener.local_addr().map_err(os)?.port();
        Ok(Responder { tokens: Tokens::default(), listener, port, timeout: TIMEOUT })
    }

    /// 实际听的端口
    pub fn port(&self) -> u16 {
        self.port
    }

    /// 起收连接的线程（Python 里那个叫 http-01 的线程）；它一直收到进程退出
    pub fn start(&self) -> Result<(), Trouble> {
        let os = |e: io::Error| Trouble::Os(os_error_text(&e));
        let listener = self.listener.try_clone().map_err(os)?;
        let (tokens, timeout) = (self.tokens.clone(), self.timeout);
        let spawned =
            std::thread::Builder::new().name("http-01".to_string()).spawn(move || accept_loop(&listener, &tokens, timeout));
        spawned.map(|_| ()).map_err(os)
    }
}

impl Challenges for Responder {
    fn put(&self, token: &str, body: &[u8]) -> Result<(), Trouble> {
        lock(&self.tokens).insert(token.to_string(), body.to_vec());
        Ok(())
    }

    fn remove(&self, token: &str) {
        lock(&self.tokens).remove(token);
    }
}

/// 一个连接占着的名额：连接处理完（或者线程没起来）时还回去
struct Seat {
    slots: Arc<Mutex<Slots>>,
    ip: String,
}

impl Drop for Seat {
    fn drop(&mut self) {
        lock(&self.slots).free(&self.ip);
    }
}

fn accept_loop(listener: &TcpListener, tokens: &Tokens, timeout: Duration) {
    let slots = Arc::new(Mutex::new(Slots::for_listener()));
    loop {
        let Ok((conn, peer)) = listener.accept() else {
            // 文件描述符用完了之类：歇一下再收，别空转
            std::thread::sleep(Duration::from_millis(100));
            continue;
        };
        let ip = peer.ip().to_string();
        if !lock(&slots).take(&ip, false) {
            continue; // 连接太多：直接断开，不排队
        }
        let seat = Seat { slots: slots.clone(), ip };
        let tokens = tokens.clone();
        // 线程起不来（系统不给了）：这个连接不管了，名额随着 seat 还回去
        let _ = std::thread::Builder::new().name("http-01-conn".to_string()).spawn(move || {
            let _seat = seat;
            let _ = one(&conn, &tokens, timeout);
        });
    }
}

/// 读连接，但一共只等到 deadline：每次读之前把这一次最多等多久改成剩下的时间
struct Timed<'a> {
    conn: &'a TcpStream,
    deadline: Instant,
}

impl Read for Timed<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        self.conn.set_read_timeout(Some(left))?;
        let mut conn = self.conn;
        conn.read(buf)
    }
}

/// 一个连接：读一个请求、回、关。出什么错都只是这个连接不要了
fn one(conn: &TcpStream, tokens: &Tokens, timeout: Duration) -> io::Result<()> {
    conn.set_write_timeout(Some(timeout))?;
    let deadline = Instant::now().checked_add(timeout).ok_or(io::ErrorKind::InvalidInput)?;
    let mut lines = Lines::new(Timed { conn, deadline });
    let reply = answer(&mut lines, &|token| lock(tokens).get(token).cloned(), &time::time)?;
    let mut out = conn;
    out.write_all(&reply)?;
    conn.shutdown(Shutdown::Write)
}

/// 把文件放进别的网站服务的目录的那一种（Acme:198-216）
pub struct Webroot {
    dir: String,
}

impl Webroot {
    /// root 是网站的根目录：文件放在它下面的 .well-known/acme-challenge 里（没有就建）
    pub fn new(root: &str) -> Result<Webroot, Trouble> {
        let dir = path::join_all(&[root, ".well-known", "acme-challenge"]);
        makedirs(&dir, 0o755)?;
        Ok(Webroot { dir })
    }
}

impl Challenges for Webroot {
    /// 先写到旁边再换过去。没成时旁边那个临时文件不删：Python 那一份也不删
    fn put(&self, token: &str, body: &[u8]) -> Result<(), Trouble> {
        let tmp = path::join(&self.dir, &format!(".{}.{}.tmp", text::head(token, 16), std::process::id()));
        let dst = path::join(&self.dir, token);
        let done = (|| {
            let mut f = fs::File::create(&tmp).map_err(|e| open_error_text(&e, &tmp))?;
            f.write_all(body).map_err(|e| os_error_text(&e))?;
            drop(f);
            chmod(&tmp, 0o644).map_err(|e| open_error_text(&e, &tmp))?; // 网站服务多半是另一个用户在跑：要它读得了
            fs::rename(&tmp, &dst).map_err(|e| format!("{}: {} -> {}", os_error_text(&e), repr_str(&tmp), repr_str(&dst)))
        })();
        done.map_err(Trouble::Os)
    }

    fn remove(&self, token: &str) {
        let _ = fs::remove_file(path::join(&self.dir, token));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::py::fs::{listdir_sorted, mode_of, TempDir};
    use crate::py::json;
    use crate::server::acme::lines::tests::{as_text, from_parts, Drip};

    /// 对照数据：Python 的 Responder（CPython 3.14 的 http.server）对一个个原样的请求回了什么。这里要回得一个字节不差——
    /// 正常的、别的方法、HTTP/0.9 的写法、各种写坏的版本、太长的行、太多的头、latin-1 的字、引号和尖括号……一下子给、
    /// 几个字节几个字节地给都一样
    #[test]
    fn answers_like_http_server() {
        let doc = json::loads(include_str!("../../../tests/vectors/server_acme_responder.json")).unwrap().as_dict().unwrap();
        let now = doc.get("now").unwrap().as_num().unwrap();
        let (token, body) = (doc.str("token").unwrap().to_string(), doc.str("body").unwrap().to_string());
        let find = |t: &str| match t {
            "empty" => Some(Vec::new()),
            t if t == token => Some(body.clone().into_bytes()),
            _ => None,
        };
        let cases = doc.list("cases").unwrap().items();
        assert!(cases.len() >= 100, "{}", cases.len());
        let mut kinds = std::collections::BTreeSet::new();
        for c in cases {
            let c = c.as_dict().unwrap();
            let name = c.str("name").unwrap().to_string();
            let request = from_parts(&c.list("request").unwrap());
            let want = c.str("response").unwrap().to_string();
            for step in [usize::MAX, 7, 1] {
                if step == 1 && request.len() > 4000 {
                    continue;
                }
                let mut conn = Lines::new(Drip { data: &request, step, errors: vec![] });
                let got = as_text(&answer(&mut conn, &find, &|| now).unwrap());
                assert!(
                    got == want,
                    "{name}（一次给 {step} 个字节）：\n得到 {:?}\n要的 {:?}",
                    text::head(&got, 600),
                    text::head(&want, 600)
                );
            }
            // 这一条是哪一种回法：带状态行的记状态码；不带的（HTTP/0.9 的写法、第一行没看懂）只有内容或者只有出错的那一页
            let page = want.split("<p>Error code: ").nth(1).and_then(|rest| rest.split('<').next());
            kinds.insert(match (want.strip_prefix("HTTP/1.0 "), page) {
                (Some(rest), _) => text::head(rest, 3).to_string(),
                (None, Some(code)) => format!("只有那一页 {code}"),
                (None, None) if want.is_empty() => "不回".to_string(),
                (None, None) => "只有内容".to_string(),
            });
        }
        let kinds: Vec<&str> = kinds.iter().map(|k| k.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "200",
                "400",
                "404",
                "414",
                "431",
                "501",
                "不回",
                "只有内容",
                "只有那一页 400",
                "只有那一页 431",
                "只有那一页 501",
                "只有那一页 505"
            ]
        );
    }

    /// 其中值得写明的几条
    #[test]
    fn answers_spelled_out() {
        let find = |t: &str| (t == "tok").then(|| b"tok.thumb".to_vec());
        let ask = |request: &str| as_text(&answer(&mut Lines::new(request.as_bytes()), &find, &|| 0.0).unwrap());
        let date = "Server: cca-acme \r\nDate: Thu, 01 Jan 1970 00:00:00 GMT\r\n";
        assert_eq!(
            ask("GET /.well-known/acme-challenge/tok HTTP/1.1\r\nHost: example.com\r\n\r\n"),
            format!("HTTP/1.0 200 OK\r\n{date}Content-Type: text/plain\r\nContent-Length: 9\r\n\r\ntok.thumb")
        );
        // 别的路径、没登记的令牌、令牌后面带着别的：404，内容是空的
        let none = format!("HTTP/1.0 404 Not Found\r\n{date}Content-Type: text/plain\r\nContent-Length: 0\r\n\r\n");
        for p in ["/", "/.well-known/acme-challenge/", "/.well-known/acme-challenge/nope", "/x/.well-known/acme-challenge/tok"] {
            assert_eq!(ask(&format!("GET {p} HTTP/1.1\r\n\r\n")), none, "{p}");
        }
        for p in ["/.well-known/acme-challenge/tok?x=1", "/.well-known/acme-challenge/tok/", "/.well-known/acme-challenge/tok%20"] {
            assert_eq!(ask(&format!("GET {p} HTTP/1.1\r\n\r\n")), none, "{p}");
        }
        // 开头多写的斜杠并成一个
        assert!(ask("GET //.well-known/acme-challenge/tok HTTP/1.0\r\n\r\n").ends_with("\r\n\r\ntok.thumb"));
        // HTTP/0.9 的写法（只有两个词）：只回内容
        assert_eq!(ask("GET /.well-known/acme-challenge/tok\r\n\r\n"), "tok.thumb");
        assert_eq!(ask("GET /nope\r\n\r\n"), "");
        // 什么都没说、只有空白：不回
        assert_eq!((ask(""), ask("\r\n"), ask("   \r\n")), (String::new(), String::new(), String::new()));
        // 别的方法：501；HEAD 不带那一页
        let post = ask("POST /.well-known/acme-challenge/tok HTTP/1.1\r\nContent-Length: 0\r\n\r\n");
        let top = format!("HTTP/1.0 501 Unsupported method ('POST')\r\n{date}Connection: close\r\n");
        assert!(
            post.starts_with(&format!(
                "{top}Content-Type: text/html;charset=utf-8\r\nContent-Length: 482\r\n\r\n<!DOCTYPE HTML>\n"
            )),
            "{post}"
        );
        assert!(
            post.ends_with("<p>Error code explanation: 501 - Server does not support this operation.</p>\n    </body>\n</html>\n")
        );
        let head_only = ask("HEAD / HTTP/1.1\r\n\r\n");
        assert!(head_only.starts_with("HTTP/1.0 501 Unsupported method ('HEAD')\r\n"), "{head_only}");
        assert!(head_only.ends_with("Content-Length: 482\r\n\r\n"), "{head_only}");
        // 出错页里的字要转义
        assert!(ask("<b>&x / HTTP/1.1\r\n\r\n").contains("<p>Message: Unsupported method ('&lt;b&gt;&amp;x').</p>"));
        // 版本
        assert_eq!(version_of("HTTP/1.1"), Some((1, "1.1")));
        assert_eq!(version_of("HTTP/0000000002.0"), Some((2, "0000000002.0")));
        for bad in ["HTTP/1", "HTTP/1.1.1", "HTTP/1.", "HTTP/.1", "http/1.1", "HTTP/1.x", "HTTP/12345678901.0", "HTTP/+1.1", "1.1"]
        {
            assert_eq!(version_of(bad), None, "{bad}");
        }
    }

    /// 连上去、发、读到对面关掉；连接上出了错的算什么都没收到
    fn exchange(port: u16, request: &[u8]) -> String {
        let mut conn = TcpStream::connect(("127.0.0.1", port)).unwrap();
        conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let _ = conn.write_all(request);
        let mut got = Vec::new();
        let _ = conn.read_to_end(&mut got);
        as_text(&got)
    }

    #[test]
    fn responder_serves_tokens() {
        let r = Responder::bind("127.0.0.1", 0).unwrap();
        let port = r.port();
        assert_ne!(port, 0);
        r.start().unwrap();
        r.put("tok-1", b"tok-1.thumb").unwrap();
        let get =
            |token: &str| exchange(port, format!("GET /.well-known/acme-challenge/{token} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes());
        let got = get("tok-1");
        assert!(got.starts_with("HTTP/1.0 200 OK\r\nServer: cca-acme \r\nDate: "), "{got}");
        assert!(got.ends_with(" GMT\r\nContent-Type: text/plain\r\nContent-Length: 11\r\n\r\ntok-1.thumb"), "{got}");
        assert!(get("other").starts_with("HTTP/1.0 404 Not Found\r\n"));
        r.put("tok-1", b"changed").unwrap();
        assert!(get("tok-1").ends_with("\r\n\r\nchanged"));
        r.remove("tok-1");
        assert!(get("tok-1").starts_with("HTTP/1.0 404 Not Found\r\n"));
        r.remove("tok-1"); // 再拿一次不算错
                           // 几处同时来、一个接一个
        r.put("many", b"m").unwrap();
        let busy: Vec<_> = (0..3)
            .map(|_| {
                std::thread::spawn(move || {
                    let ask = b"GET /.well-known/acme-challenge/many HTTP/1.0\r\n\r\n";
                    (0..20).all(|_| exchange(port, ask).ends_with("\r\n\r\nm"))
                })
            })
            .collect();
        assert!(busy.into_iter().all(|t| t.join().unwrap()));
        // 端口被占着：说得出原因（Python 的 OSError 那句）
        let Err(Trouble::Os(why)) = Responder::bind("127.0.0.1", port) else { panic!("占着的端口不该听得了") };
        assert!(why.starts_with("[Errno ") && why.contains("in use"), "{why}");
        // 名字解析：localhost 听的是 IPv4 的那个
        let named = Responder::bind("localhost", 0).unwrap();
        assert!(named.listener.local_addr().unwrap().is_ipv4());
    }

    /// 同一个 IP 同时最多 8 个连接：第 9 个直接断开；有人走了才进得来
    #[test]
    fn responder_caps_connections_per_ip() {
        let r = Responder::bind("127.0.0.1", 0).unwrap();
        let port = r.port();
        r.start().unwrap();
        r.put("t", b"x").unwrap();
        let ask = b"GET /.well-known/acme-challenge/t HTTP/1.0\r\n\r\n";
        let idle: Vec<TcpStream> = (0..8).map(|_| TcpStream::connect(("127.0.0.1", port)).unwrap()).collect();
        assert_eq!(exchange(port, ask), "", "第 9 个连接不该有回应");
        drop(idle);
        let served = (0..300).any(|_| {
            if exchange(port, ask).ends_with("\r\n\r\nx") {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
            false
        });
        assert!(served, "占着的走了以后要进得来");
    }

    /// 读一个请求一共只等这么久：送了一半不送了的，到点就断开，不回
    #[test]
    fn responder_gives_up_on_slow_requests() {
        let mut r = Responder::bind("127.0.0.1", 0).unwrap();
        r.timeout = Duration::from_millis(150);
        r.start().unwrap();
        let mut conn = TcpStream::connect(("127.0.0.1", r.port())).unwrap();
        conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        conn.write_all(b"GET /.well-known/acme-challenge/t HTTP/1.1\r\nHost:").unwrap();
        let began = Instant::now();
        let mut got = Vec::new();
        let _ = conn.read_to_end(&mut got);
        let took = began.elapsed();
        assert!(got.is_empty() && took >= Duration::from_millis(100) && took < Duration::from_secs(4), "{got:?} {took:?}");
    }

    #[test]
    fn webroot_puts_files_where_the_web_server_reads_them() {
        let d = TempDir::new().unwrap();
        let root = path::join(&d.path, "www/site");
        let w = Webroot::new(&root).unwrap();
        let dir = path::join(&root, ".well-known/acme-challenge");
        assert!(path::isdir(&dir));
        let token = "tok_EN-123456789abcdefgh";
        let f = path::join(&dir, token);
        w.put(token, b"tok.thumb").unwrap();
        assert_eq!((fs::read(&f).unwrap(), mode_of(&f).unwrap()), (b"tok.thumb".to_vec(), 0o644));
        assert_eq!(listdir_sorted(&dir).unwrap(), [token], "旁边不留临时文件");
        w.put(token, b"again").unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"again");
        w.remove(token);
        assert!(!path::exists(&f));
        w.remove(token); // 不在也不算错
                         // 目录已经有了：照用
        chmod(&dir, 0o700).unwrap();
        Webroot::new(&root).unwrap();
        assert_eq!(mode_of(&dir).unwrap(), 0o700, "已经有的目录权限不动");
        // 目录没了：放不进去，说的话照 Python 的 OSError（临时文件的名字是令牌的前 16 个字加进程号）
        fs::remove_dir(&dir).unwrap();
        let Err(Trouble::Os(why)) = w.put("0123456789abcdefXYZ", b"x") else { panic!("不该放得进去") };
        assert_eq!(why, format!("[Errno 2] No such file or directory: '{dir}/.0123456789abcdef.{}.tmp'", std::process::id()));
        // 网站的根目录其实是个文件：目录建不出来
        let file = path::join(&d.path, "file");
        fs::write(&file, b"").unwrap();
        let Err(Trouble::Os(why)) = Webroot::new(&file) else { panic!("不该建得出来") };
        assert_eq!(why, format!("[Errno 20] Not a directory: '{file}/.well-known'"));
    }
}
