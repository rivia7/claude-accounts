//! HTTP/1.1 的服务端，给测试里的假服务用（假的证书机构 kit::acmefake、给测试当“别的网站”的 [`files`]……）：
//! 一个连接一个线程，处理请求的是一个闭包；明文的，或者 TLS 的（kit::tlsd）。照 Python 的 http.server（BaseHTTPRequestHandler）的规矩：
//!
//! - 请求行、头（名字不分大小写，同名的有几个时取第一个：`self.headers.get`）、请求体（Content-Length；chunked 的也解开）
//! - 默认回 HTTP/1.1：一个连接上可以接着发下一个请求（keep-alive）；对方说了 `Connection: close`，或者对方是 HTTP/1.0
//!   又没说 `Connection: keep-alive`，回完这一个就关。[`Setup::http10`]：回的是 HTTP/1.0，每个请求回完就关
//!   （`python3 -m http.server` 那样）
//! - `Expect: 100-continue`：先回 100 再读请求体
//! - 请求写得不对就不交给闭包：请求行看不懂 400、太长 414、头太长太多 431、请求体太大 413，回完关上连接；对方什么都没发就断了，
//!   直接关上
//! - 回的：闭包给的状态码、头（照先后，前面加一个 Date）、内容；闭包没写 Content-Length 的补上（HEAD 也写它该有的长度，只是不发内容）；
//!   要关的连接（HTTP/1.1 时）带上 `Connection: close`。闭包 panic 了回 500、关上连接
//! - [`Server::stop`]（或者丢掉 [`Server`]）：不再接新的连接、监听的端口关掉，已经接上的断开，等正在处理的请求做完
//!   （最多 5 秒）。TLS 的连接断开时不发 close_notify（和 Python 的 ssl 一样，见 kit::tlsd）
//!
//! ```ignore
//! let srv = httpd::Server::bind(("127.0.0.1", 0))?.tls(acceptor).start(|req| {
//!     if req.method == "GET" && req.path == "/hello" { Response::new(200).body("你好") } else { Response::new(404) }
//! })?;
//! let url = format!("https://127.0.0.1:{}/hello", srv.port());
//! ```

use crate::tlsd::Acceptor;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 一行（请求行、一个头）最长多少字节（Python 的 _MAXLINE）
const MAX_LINE: usize = 65536;
/// 最多多少个头（Python 的 _MAXHEADERS）
const MAX_HEADERS: usize = 100;
/// 请求体最大多少：假的服务用不着更大的
const MAX_BODY: usize = 64 << 20;
/// stop 时等正在处理的请求做完，最多等多久
const DRAIN: Duration = Duration::from_secs(5);
/// 接新连接的线程多久看一眼该不该停（毫秒）
const LOOK_MS: i32 = 100;

/// 收到的一个请求
#[derive(Clone, Debug)]
pub struct Request {
    /// GET、POST……（原样，区分大小写）
    pub method: String,
    /// 请求行里的目标，原样（`/dir`、`/a?b=1`）：Python 的 self.path
    pub path: String,
    /// HTTP/1.1、HTTP/1.0
    pub version: String,
    /// 头，照收到的先后（名字照对方写的大小写）
    pub headers: Vec<(String, String)>,
    /// 请求体（Content-Length 或者 chunked 的，解开了的）
    pub body: Vec<u8>,
    /// 对方的地址
    pub peer: SocketAddr,
    /// 这一头的地址（监听的那个地址和端口）
    pub local: SocketAddr,
    /// 是不是 TLS 的连接
    pub tls: bool,
}

impl Request {
    /// 一个头的值：名字不分大小写，同名的有几个时取第一个；没有是 None
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// 自己的地址（`https://127.0.0.1:端口`），拼绝对的网址用
    pub fn base(&self) -> String {
        format!("{}://{}", if self.tls { "https" } else { "http" }, self.local)
    }
}

/// 回给对方的：状态码、头、内容
#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    /// 照先后写出去；没写 Content-Length 的话补上
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// 回完就关上连接
    pub close: bool,
}

impl Response {
    /// 这个状态码、没有头、没有内容
    pub fn new(status: u16) -> Response {
        Response { status, headers: Vec::new(), body: Vec::new(), close: false }
    }

    /// 一段文字（text/plain; charset=utf-8）
    pub fn text(status: u16, text: &str) -> Response {
        Response::new(status).header("Content-Type", "text/plain; charset=utf-8").body(text)
    }

    /// 加一个头（照加的先后写出去）
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Response {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// 内容
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Response {
        self.body = body.into();
        self
    }

    /// 回完就关上连接
    pub fn close(mut self) -> Response {
        self.close = true;
        self
    }

    fn has(&self, name: &str) -> bool {
        self.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name))
    }
}

/// 状态码的说明（状态行里的那几个字）
pub fn reason(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Content Too Large",
        414 => "URI Too Long",
        415 => "Unsupported Media Type",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        505 => "HTTP Version Not Supported",
        _ => "",
    }
}

/// HTTP 的时间写法（`Sun, 06 Nov 1994 08:49:37 GMT`）：Date 头用
pub fn http_date(t: SystemTime) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let secs = match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    };
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // 从 1970-01-01 起的天数换成年月日（Howard Hinnant 的 civil_from_days）
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
        DAYS[(days + 4).rem_euclid(7) as usize], // 1970-01-01 是星期四
        day,
        MONTHS[(month - 1) as usize],
        year,
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// 处理请求的闭包
type Handler = dyn Fn(&Request) -> Response + Send + Sync;

/// 绑好了端口、还没开始接连接的服务（[`Server::bind`] 给的）：在这里定 TLS、HTTP/1.0、空闲多久断开，再 [`Setup::start`]
pub struct Setup {
    listener: TcpListener,
    tls: Option<Acceptor>,
    http10: bool,
    idle: Option<Duration>,
}

impl Setup {
    /// 绑上的端口（绑 0 号端口时是系统给的那个）
    pub fn port(&self) -> u16 {
        self.listener.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    /// 走 TLS（证书、私钥在接入器里）
    pub fn tls(mut self, acceptor: Acceptor) -> Setup {
        self.tls = Some(acceptor);
        self
    }

    /// 回 HTTP/1.0、每个请求回完就关（Python 的 protocol_version 是默认的 HTTP/1.0 时那样）
    pub fn http10(mut self) -> Setup {
        self.http10 = true;
        self
    }

    /// 一个连接上这么久读不到东西就断开（默认一直等，和 Python 的 http.server 一样）
    pub fn idle_timeout(mut self, limit: Duration) -> Setup {
        self.idle = Some(limit);
        self
    }

    /// 开始接连接：一个连接一个线程，每个请求交给 handler
    pub fn start(self, handler: impl Fn(&Request) -> Response + Send + Sync + 'static) -> io::Result<Server> {
        let addr = self.listener.local_addr()?;
        // 接连接的线程用 poll 等，隔一会儿看一眼该不该停；不阻塞，免得两个线程抢同一个连接时卡住
        self.listener.set_nonblocking(true)?;
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            conns: Mutex::new(HashMap::new()),
            active: Mutex::new(0),
            drained: Condvar::new(),
            next: AtomicU64::new(0),
            handler: Box::new(handler),
            tls: self.tls,
            http10: self.http10,
            idle: self.idle,
        });
        let sh = shared.clone();
        let listener = self.listener;
        let acceptor = std::thread::Builder::new().name("httpd-accept".to_string()).spawn(move || accept_loop(listener, sh))?;
        Ok(Server { addr, shared, acceptor: Some(acceptor) })
    }
}

/// 几个线程共用的
struct Shared {
    stop: AtomicBool,
    /// 接上了、还没断开的连接（stop 时把它们断开）
    conns: Mutex<HashMap<u64, TcpStream>>,
    /// 还在处理的连接有几个
    active: Mutex<usize>,
    drained: Condvar,
    next: AtomicU64,
    handler: Box<Handler>,
    tls: Option<Acceptor>,
    http10: bool,
    idle: Option<Duration>,
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 跑着的服务。丢掉它就停（同 [`Server::stop`]）
pub struct Server {
    addr: SocketAddr,
    shared: Arc<Shared>,
    acceptor: Option<JoinHandle<()>>,
}

impl Server {
    /// 绑一个地址（`("127.0.0.1", 0)`：系统给一个空闲端口）
    pub fn bind(addr: impl ToSocketAddrs) -> io::Result<Setup> {
        Ok(Setup { listener: TcpListener::bind(addr)?, tls: None, http10: false, idle: None })
    }

    /// 监听的端口
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// 监听的地址
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// 停下：不再接新的连接、监听的端口关掉，已经接上的断开，等正在处理的请求做完（最多 5 秒）。停过了再调什么都不做
    pub fn stop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        let Some(acceptor) = self.acceptor.take() else { return };
        let _ = acceptor.join();
        // 接连接的线程已经退出了：不会再有新登记的连接
        for sock in locked(&self.shared.conns).values() {
            let _ = sock.shutdown(Shutdown::Both);
        }
        let end = Instant::now() + DRAIN;
        let mut active = locked(&self.shared.active);
        while *active > 0 {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            active = self.shared.drained.wait_timeout(active, left).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 有新连接来了没有（最多等 ms 毫秒）
fn readable(fd: i32, ms: i32) -> bool {
    let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    // SAFETY: 一个 pollfd，调用期间一直有效
    unsafe { libc::poll(&mut p, 1, ms) > 0 }
}

fn accept_loop(listener: TcpListener, sh: Arc<Shared>) {
    let fd = listener.as_raw_fd();
    while !sh.stop.load(Ordering::SeqCst) {
        if !readable(fd, LOOK_MS) {
            continue;
        }
        let Ok((sock, peer)) = listener.accept() else { continue };
        // macOS 上接进来的连接跟着监听的那个是不阻塞的：改回阻塞
        if sock.set_nonblocking(false).is_err() {
            continue;
        }
        let Ok(clone) = sock.try_clone() else { continue };
        let id = sh.next.fetch_add(1, Ordering::SeqCst);
        locked(&sh.conns).insert(id, clone);
        *locked(&sh.active) += 1;
        let worker = sh.clone();
        let spawned = std::thread::Builder::new().name("httpd-conn".to_string()).spawn(move || {
            let _done = Done { sh: worker.clone(), id };
            connection(&worker, sock, peer);
        });
        if spawned.is_err() {
            // 线程没起来：上面登记的撤掉（连接随着闭包一起关了）
            drop(Done { sh: sh.clone(), id });
        }
    }
}

/// 一个连接处理完了：撤掉登记，叫醒等着 stop 的
struct Done {
    sh: Arc<Shared>,
    id: u64,
}

impl Drop for Done {
    fn drop(&mut self) {
        locked(&self.sh.conns).remove(&self.id);
        let mut active = locked(&self.sh.active);
        *active = active.saturating_sub(1);
        self.sh.drained.notify_all();
    }
}

fn connection(sh: &Shared, sock: TcpStream, peer: SocketAddr) {
    let Ok(local) = sock.local_addr() else { return };
    let _ = sock.set_nodelay(true);
    if let Some(limit) = sh.idle {
        let _ = sock.set_read_timeout(Some(limit));
    }
    match &sh.tls {
        Some(acceptor) => {
            if let Ok(stream) = acceptor.accept(sock) {
                serve(sh, Conn::new(stream), peer, local, true);
            }
        }
        None => serve(sh, Conn::new(sock), peer, local, false),
    }
}

/// 带缓冲的连接：按行读、按长度读、写
struct Conn<S> {
    s: S,
    buf: Vec<u8>,
}

/// 读一行的结果
enum Line {
    Got(Vec<u8>),
    /// 对方关了（或者出错、超时）：前面没读完的一截不要了
    Closed,
    /// 超过了长度
    TooLong,
}

impl<S: Read + Write> Conn<S> {
    fn new(s: S) -> Conn<S> {
        Conn { s, buf: Vec::new() }
    }

    /// 再读一些进缓冲；对方关了、出错是 false（没有 close_notify 就断开也算关了：Python 的 suppress_ragged_eofs）
    fn fill(&mut self) -> bool {
        let mut chunk = [0u8; 16384];
        loop {
            match self.s.read(&mut chunk) {
                Ok(0) => return false,
                Ok(n) => {
                    self.buf.extend_from_slice(&chunk[..n]);
                    return true;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return false,
            }
        }
    }

    /// 读一行（到 \n 为止，带着它）
    fn line(&mut self, max: usize) -> Line {
        loop {
            if let Some(i) = self.buf.iter().position(|b| *b == b'\n') {
                if i >= max {
                    return Line::TooLong;
                }
                return Line::Got(self.buf.drain(..=i).collect());
            }
            if self.buf.len() >= max {
                return Line::TooLong;
            }
            if !self.fill() {
                return Line::Closed;
            }
        }
    }

    /// 正好读 n 个字节；不够就断了是 None
    fn exact(&mut self, n: usize) -> Option<Vec<u8>> {
        while self.buf.len() < n {
            if !self.fill() {
                return None;
            }
        }
        Some(self.buf.drain(..n).collect())
    }

    fn send(&mut self, data: &[u8]) -> io::Result<()> {
        self.s.write_all(data)?;
        self.s.flush()
    }
}

/// 读请求出的岔子
enum Bad {
    /// 对方关了：直接关
    Closed,
    /// 回这个状态码和这句话，然后关
    Reply(u16, &'static str),
}

/// 一个连接上一个接一个地处理请求
fn serve<S: Read + Write>(sh: &Shared, mut c: Conn<S>, peer: SocketAddr, local: SocketAddr, tls: bool) {
    loop {
        if sh.stop.load(Ordering::SeqCst) {
            return;
        }
        let (req, keep) = match read_request(&mut c, sh.http10, peer, local, tls) {
            Ok(got) => got,
            Err(Bad::Closed) => return,
            Err(Bad::Reply(status, why)) => {
                let resp = Response::text(status, why).close();
                let _ = c.send(&render(&resp, false, sh.http10, true));
                return;
            }
        };
        let resp = match catch_unwind(AssertUnwindSafe(|| (sh.handler)(&req))) {
            Ok(resp) => resp,
            Err(_) => Response::text(500, "处理请求的时候出错了").close(),
        };
        let keep = keep && !resp.close && !sh.http10;
        if c.send(&render(&resp, req.method == "HEAD", sh.http10, !keep)).is_err() || !keep {
            return;
        }
    }
}

/// 写出一个回复：状态行、Date、闭包给的头、补上的 Content-Length（和 Connection: close）、内容（HEAD 不发）
fn render(resp: &Response, head: bool, http10: bool, closing: bool) -> Vec<u8> {
    let mut out = format!("HTTP/{} {} {}\r\n", if http10 { "1.0" } else { "1.1" }, resp.status, reason(resp.status));
    if !resp.has("Date") {
        out += &format!("Date: {}\r\n", http_date(SystemTime::now()));
    }
    for (k, v) in &resp.headers {
        out += &format!("{k}: {v}\r\n");
    }
    let bodiless = resp.status < 200 || resp.status == 204 || resp.status == 304;
    if !resp.has("Content-Length") && !bodiless {
        out += &format!("Content-Length: {}\r\n", resp.body.len());
    }
    if closing && !http10 && !resp.has("Connection") {
        out += "Connection: close\r\n";
    }
    out += "\r\n";
    let mut bytes = out.into_bytes();
    if !head && !bodiless {
        bytes.extend_from_slice(&resp.body);
    }
    bytes
}

/// Connection 头里有没有这一项（逗号隔开，不分大小写）
fn conn_has(value: Option<&str>, token: &str) -> bool {
    value.is_some_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)))
}

/// 去掉行尾的 \r\n（或者 \n），当 latin-1 读（和 Python 一样，一个字节一个字）
fn latin1(line: &[u8]) -> String {
    let mut end = line.len();
    while end > 0 && matches!(line[end - 1], b'\r' | b'\n') {
        end -= 1;
    }
    line[..end].iter().map(|b| *b as char).collect()
}

/// 读一个请求：(请求, 回完以后还接不接着用这个连接)
fn read_request<S: Read + Write>(
    c: &mut Conn<S>,
    http10: bool,
    peer: SocketAddr,
    local: SocketAddr,
    tls: bool,
) -> Result<(Request, bool), Bad> {
    let line = match c.line(MAX_LINE) {
        Line::Got(l) => l,
        Line::Closed => return Err(Bad::Closed),
        Line::TooLong => return Err(Bad::Reply(414, "请求行太长")),
    };
    let text = latin1(&line);
    let words: Vec<&str> = text.split_ascii_whitespace().collect();
    if words.is_empty() {
        return Err(Bad::Closed);
    }
    let [method, path, version] = words[..] else { return Err(Bad::Reply(400, "请求行看不懂")) };
    let Some((major, minor)) = version.strip_prefix("HTTP/").and_then(|v| v.split_once('.')) else {
        return Err(Bad::Reply(400, "HTTP 的版本看不懂"));
    };
    let digits = |s: &str| !s.is_empty() && s.len() <= 10 && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(major) || !digits(minor) {
        return Err(Bad::Reply(400, "HTTP 的版本看不懂"));
    }
    let ver: (u64, u64) = (major.parse().unwrap_or(0), minor.parse().unwrap_or(0));
    if ver >= (2, 0) {
        return Err(Bad::Reply(505, "只认 HTTP/1.x"));
    }
    let mut headers: Vec<(String, String)> = Vec::new();
    loop {
        let line = match c.line(MAX_LINE) {
            Line::Got(l) => l,
            Line::Closed => return Err(Bad::Closed),
            Line::TooLong => return Err(Bad::Reply(431, "头太长")),
        };
        let h = latin1(&line);
        if h.is_empty() {
            break;
        }
        if h.starts_with([' ', '\t']) {
            // 折到下一行的（obs-fold）：接在上一个头后面
            match headers.last_mut() {
                Some((_, v)) => {
                    v.push(' ');
                    v.push_str(h.trim());
                }
                None => return Err(Bad::Reply(400, "头看不懂")),
            }
            continue;
        }
        let Some((k, v)) = h.split_once(':') else { return Err(Bad::Reply(400, "头看不懂")) };
        if headers.len() >= MAX_HEADERS {
            return Err(Bad::Reply(431, "头太多"));
        }
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    let get = |name: &str| headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str());
    let connection = get("Connection");
    let keep = !http10 && !conn_has(connection, "close") && (ver >= (1, 1) || conn_has(connection, "keep-alive"));
    if ver >= (1, 1) && !http10 && get("Expect").is_some_and(|v| v.eq_ignore_ascii_case("100-continue")) {
        let _ = c.send(b"HTTP/1.1 100 Continue\r\n\r\n");
    }
    let chunked =
        get("Transfer-Encoding").is_some_and(|v| v.rsplit(',').next().is_some_and(|t| t.trim().eq_ignore_ascii_case("chunked")));
    let body = if chunked {
        read_chunked(c)?
    } else {
        let len = match get("Content-Length") {
            None => 0,
            Some(v) => v.trim().parse::<usize>().map_err(|_| Bad::Reply(400, "Content-Length 不是数"))?,
        };
        if len > MAX_BODY {
            return Err(Bad::Reply(413, "请求体太大"));
        }
        c.exact(len).ok_or(Bad::Closed)?
    };
    let req = Request {
        method: method.to_string(),
        path: path.to_string(),
        version: version.to_string(),
        headers,
        body,
        peer,
        local,
        tls,
    };
    Ok((req, keep))
}

/// 读 chunked 的请求体：一段段的「十六进制的长度（; 后面的不管）、内容、\r\n」，长度 0 的那段之后是可有可无的尾部的头
fn read_chunked<S: Read + Write>(c: &mut Conn<S>) -> Result<Vec<u8>, Bad> {
    let mut body = Vec::new();
    loop {
        let size = match c.line(MAX_LINE) {
            Line::Got(l) => latin1(&l),
            Line::Closed => return Err(Bad::Closed),
            Line::TooLong => return Err(Bad::Reply(400, "chunked 的长度那一行太长")),
        };
        let hex = size.split(';').next().unwrap_or("").trim();
        let n = usize::from_str_radix(hex, 16).map_err(|_| Bad::Reply(400, "chunked 的长度看不懂"))?;
        if body.len() + n > MAX_BODY {
            return Err(Bad::Reply(413, "请求体太大"));
        }
        if n == 0 {
            loop {
                match c.line(MAX_LINE) {
                    Line::Got(l) if latin1(&l).is_empty() => return Ok(body),
                    Line::Got(_) => {}
                    Line::Closed => return Err(Bad::Closed),
                    Line::TooLong => return Err(Bad::Reply(431, "头太长")),
                }
            }
        }
        body.extend(c.exact(n).ok_or(Bad::Closed)?);
        match c.line(MAX_LINE) {
            Line::Got(l) if latin1(&l).is_empty() => {}
            Line::Got(_) | Line::TooLong => return Err(Bad::Reply(400, "chunked 的一段后面不是空行")),
            Line::Closed => return Err(Bad::Closed),
        }
    }
}

/// 照一个目录给文件（`python3 -m http.server --directory 目录` 那样，给测试当“别的网站”）：只认 GET、HEAD（别的 501）。
/// 路径里的 `?`、`#` 后面的不管，%xx 解开，`..` 出不了这个目录；文件回 200（Content-Type 照扩展名认几种常见的，
/// 别的是 application/octet-stream），以 / 结尾的文件 404；目录：不以 / 结尾的 301 到带 / 的，带 / 的给里面的 index.html，没有就 404
/// （不列目录：和 Python 不一样的一处）；没有的 404
pub fn files(root: impl Into<PathBuf>) -> impl Fn(&Request) -> Response + Send + Sync + 'static {
    let root = root.into();
    move |req: &Request| {
        if req.method != "GET" && req.method != "HEAD" {
            return Response::text(501, &format!("Unsupported method ('{}')", req.method)).close();
        }
        let raw = req.path.split(['?', '#']).next().unwrap_or("");
        let decoded = percent_decode(raw.as_bytes());
        let mut path = root.clone();
        let mut parts: Vec<&[u8]> = Vec::new();
        for seg in decoded.split(|b| *b == b'/') {
            match seg {
                b"" | b"." => {}
                b".." => {
                    parts.pop();
                }
                s => parts.push(s),
            }
        }
        for p in parts {
            path.push(OsStr::from_bytes(p));
        }
        let not_found = || Response::text(404, "File not found").close();
        if path.is_dir() {
            if !raw.ends_with('/') {
                let tail = req.path.strip_prefix(raw).unwrap_or("");
                return Response::new(301).header("Location", format!("{raw}/{tail}"));
            }
            path.push("index.html");
            if !path.is_file() {
                return not_found();
            }
        } else if raw.ends_with('/') {
            return not_found();
        }
        match std::fs::read(&path) {
            Ok(body) => Response::new(200).header("Content-Type", content_type(&path)).body(body),
            Err(_) => not_found(),
        }
    }
}

/// 照扩展名认的 Content-Type（几种常见的）
fn content_type(path: &std::path::Path) -> &'static str {
    let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("html" | "htm") => "text/html",
        Some("txt") => "text/plain",
        Some("json") => "application/json",
        Some("js" | "mjs") => "text/javascript",
        Some("css") => "text/css",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("pem") => "application/x-pem-file",
        _ => "application/octet-stream",
    }
}

/// %xx 解开（写错的照原样留着）
fn percent_decode(s: &[u8]) -> Vec<u8> {
    let hex = |b: u8| (b as char).to_digit(16);
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'%' && i + 2 < s.len() {
            if let (Some(h), Some(l)) = (hex(s[i + 1]), hex(s[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;

    /// 读一个回复：状态行、头、内容（照 Content-Length）
    fn response(rd: &mut io::BufReader<TcpStream>) -> (String, Vec<(String, String)>, Vec<u8>) {
        let mut status = String::new();
        rd.read_line(&mut status).unwrap();
        let mut headers = Vec::new();
        loop {
            let mut line = String::new();
            rd.read_line(&mut line).unwrap();
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            let (k, v) = line.split_once(':').unwrap();
            headers.push((k.to_string(), v.trim().to_string()));
        }
        let len = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).map_or(0, |(_, v)| v.parse().unwrap());
        let mut body = vec![0u8; len];
        rd.read_exact(&mut body).unwrap();
        (status.trim_end().to_string(), headers, body)
    }

    fn get<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
        headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// 把请求原样回过去的服务
    fn echo() -> Server {
        Server::bind(("127.0.0.1", 0))
            .unwrap()
            .start(|req| {
                let text = format!(
                    "{} {} {} {}",
                    req.method,
                    req.path,
                    req.header("x-Thing").unwrap_or("-"),
                    String::from_utf8_lossy(&req.body)
                );
                Response::text(200, &text).header("X-Peer-Local", req.local.to_string())
            })
            .unwrap()
    }

    fn connect(srv: &Server) -> (TcpStream, io::BufReader<TcpStream>) {
        let s = TcpStream::connect(("127.0.0.1", srv.port())).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let rd = io::BufReader::new(s.try_clone().unwrap());
        (s, rd)
    }

    /// 对方关了：再读读到头
    fn closed(rd: &mut io::BufReader<TcpStream>) -> bool {
        let mut rest = Vec::new();
        rd.read_to_end(&mut rest).is_ok() && rest.is_empty()
    }

    #[test]
    fn http_dates() {
        let at = |s: u64| http_date(UNIX_EPOCH + Duration::from_secs(s));
        assert_eq!(at(0), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(at(784111777), "Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(at(951782400), "Tue, 29 Feb 2000 00:00:00 GMT");
        assert_eq!(at(4102444799), "Thu, 31 Dec 2099 23:59:59 GMT");
    }

    #[test]
    fn keep_alive_until_the_client_says_close() {
        let srv = echo();
        let (mut s, mut rd) = connect(&srv);
        // 同一个连接上两个请求；头的名字不分大小写，同名的取第一个
        s.write_all(b"POST /a?b=1 HTTP/1.1\r\nHost: x\r\nx-thing: one\r\nX-Thing: two\r\nContent-Length: 5\r\n\r\nhello").unwrap();
        let (status, headers, body) = response(&mut rd);
        assert_eq!(status, "HTTP/1.1 200 OK");
        assert_eq!(body, b"POST /a?b=1 one hello");
        assert_eq!(get(&headers, "Content-Length"), Some("21"));
        assert!(get(&headers, "Date").is_some_and(|d| d.ends_with(" GMT")));
        assert_eq!(get(&headers, "X-Peer-Local"), Some(format!("127.0.0.1:{}", srv.port()).as_str()));
        assert_eq!(get(&headers, "Connection"), None);
        s.write_all(b"GET /second HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
        let (status, headers, body) = response(&mut rd);
        assert_eq!((status.as_str(), body.as_slice()), ("HTTP/1.1 200 OK", &b"GET /second - "[..]));
        assert_eq!(get(&headers, "Connection"), Some("close"));
        assert!(closed(&mut rd));
    }

    #[test]
    fn http10_clients_and_http10_servers_close() {
        let srv = echo();
        // HTTP/1.0 的客户端：没说 keep-alive 就关，说了就接着用
        let (mut s, mut rd) = connect(&srv);
        s.write_all(b"GET /old HTTP/1.0\r\nConnection: keep-alive\r\n\r\n").unwrap();
        assert_eq!(response(&mut rd).2, b"GET /old - ");
        s.write_all(b"GET /old2 HTTP/1.0\r\n\r\n").unwrap();
        assert_eq!(response(&mut rd).2, b"GET /old2 - ");
        assert!(closed(&mut rd));
        // HTTP/1.0 的服务：状态行是 1.0，回完一个就关
        let srv = Server::bind(("127.0.0.1", 0)).unwrap().http10().start(|_| Response::text(200, "x")).unwrap();
        let (mut s, mut rd) = connect(&srv);
        s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let (status, headers, _) = response(&mut rd);
        assert_eq!((status.as_str(), get(&headers, "Connection")), ("HTTP/1.0 200 OK", None));
        assert!(closed(&mut rd));
    }

    #[test]
    fn head_chunked_and_expect() {
        let srv = echo();
        let (mut s, mut rd) = connect(&srv);
        // HEAD：Content-Length 照写，内容不发（下一个回复紧接着来）
        s.write_all(b"HEAD /h HTTP/1.1\r\nHost: x\r\n\r\nGET /next HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut status = String::new();
        rd.read_line(&mut status).unwrap();
        assert_eq!(status, "HTTP/1.1 200 OK\r\n");
        let mut lines = Vec::new();
        loop {
            let mut line = String::new();
            rd.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            lines.push(line);
        }
        assert!(lines.contains(&"Content-Length: 10\r\n".to_string()), "{lines:?}");
        assert_eq!(response(&mut rd).2, b"GET /next - ");
        // chunked 的请求体解开；Expect: 100-continue 先回 100
        s.write_all(b"POST /c HTTP/1.1\r\nHost: x\r\nExpect: 100-continue\r\nTransfer-Encoding: chunked\r\n\r\n").unwrap();
        let mut cont = String::new();
        rd.read_line(&mut cont).unwrap();
        assert_eq!(cont, "HTTP/1.1 100 Continue\r\n");
        rd.read_line(&mut cont).unwrap();
        s.write_all(b"3;ext=1\r\nabc\r\n2\r\nde\r\n0\r\nTrailer: x\r\n\r\n").unwrap();
        assert_eq!(response(&mut rd).2, b"POST /c - abcde");
    }

    #[test]
    fn bad_requests_are_answered_and_closed() {
        let srv = echo();
        for (raw, want) in [
            (&b"NONSENSE\r\n\r\n"[..], "HTTP/1.1 400 Bad Request"),
            (b"GET / HTTP/2.0\r\n\r\n", "HTTP/1.1 505 HTTP Version Not Supported"),
            (b"GET / FTP/1.1\r\n\r\n", "HTTP/1.1 400 Bad Request"),
            (b"POST / HTTP/1.1\r\nContent-Length: lots\r\n\r\n", "HTTP/1.1 400 Bad Request"),
            (b"GET / HTTP/1.1\r\nno colon here\r\n\r\n", "HTTP/1.1 400 Bad Request"),
        ] {
            let (mut s, mut rd) = connect(&srv);
            s.write_all(raw).unwrap();
            let (status, headers, _) = response(&mut rd);
            assert_eq!(status, want, "{}", String::from_utf8_lossy(raw));
            assert_eq!(get(&headers, "Connection"), Some("close"));
            assert!(closed(&mut rd));
        }
        // 请求行太长：414（正好发到上限、不带换行：服务读完了才回，关的时候没有没读的，不会变成 RST）
        let (mut s, mut rd) = connect(&srv);
        s.write_all(format!("GET /{}", "a".repeat(MAX_LINE - 5)).as_bytes()).unwrap();
        assert_eq!(response(&mut rd).0, "HTTP/1.1 414 URI Too Long");
        // 什么都没发就关了：服务照常
        drop(TcpStream::connect(("127.0.0.1", srv.port())).unwrap());
        let (mut s, mut rd) = connect(&srv);
        s.write_all(b"GET /still HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(response(&mut rd).2, b"GET /still - ");
    }

    #[test]
    fn a_panicking_handler_gets_a_500() {
        let srv = Server::bind(("127.0.0.1", 0))
            .unwrap()
            .start(|req| if req.path == "/boom" { panic!("故意的") } else { Response::new(204) })
            .unwrap();
        let (mut s, mut rd) = connect(&srv);
        s.write_all(b"GET /fine HTTP/1.1\r\n\r\n").unwrap();
        let (status, headers, body) = response(&mut rd);
        assert_eq!((status.as_str(), get(&headers, "Content-Length"), body.len()), ("HTTP/1.1 204 No Content", None, 0));
        s.write_all(b"GET /boom HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(response(&mut rd).0, "HTTP/1.1 500 Internal Server Error");
        assert!(closed(&mut rd));
    }

    #[test]
    fn stop_closes_the_port_and_the_connections() {
        let mut srv = echo();
        let port = srv.port();
        let (mut s, mut rd) = connect(&srv);
        s.write_all(b"GET /1 HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(response(&mut rd).2, b"GET /1 - ");
        let started = Instant::now();
        srv.stop();
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
        assert!(closed(&mut rd), "接着的连接断开了");
        assert_eq!(TcpStream::connect(("127.0.0.1", port)).unwrap_err().kind(), io::ErrorKind::ConnectionRefused);
        srv.stop();
    }

    #[test]
    fn files_like_python_http_server() {
        let root = crate::fsx::TempDir::new("httpd-unit-");
        crate::fsx::write(root.join("index.html"), "首页");
        crate::fsx::write(root.join("sub/a b.txt"), "带空格");
        crate::fsx::write(root.join("sub/token"), "tok.thumb");
        let outside = crate::fsx::TempDir::new("httpd-unit-");
        crate::fsx::write(outside.join("secret"), "不该给");
        let srv = Server::bind(("127.0.0.1", 0)).unwrap().http10().start(files(root.path())).unwrap();
        let fetch = |raw: String| {
            let (mut s, mut rd) = connect(&srv);
            s.write_all(raw.as_bytes()).unwrap();
            response(&mut rd)
        };
        let get_path = |path: &str| fetch(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n"));
        let (status, headers, body) = get_path("/");
        assert_eq!(
            (status.as_str(), get(&headers, "Content-Type"), body),
            ("HTTP/1.0 200 OK", Some("text/html"), "首页".as_bytes().to_vec())
        );
        assert_eq!(get_path("/sub/a%20b.txt?x=1").2, "带空格".as_bytes());
        let (_, headers, body) = get_path("/sub/token");
        assert_eq!((get(&headers, "Content-Type"), body.as_slice()), (Some("application/octet-stream"), &b"tok.thumb"[..]));
        let (status, headers, _) = get_path("/sub?q");
        assert_eq!((status.as_str(), get(&headers, "Location")), ("HTTP/1.0 301 Moved Permanently", Some("/sub/?q")));
        let name = outside.file_name().unwrap().to_str().unwrap().to_string();
        let (up, up2) = (format!("/../{name}/secret"), format!("/sub/../../{name}/secret"));
        for missing in ["/nope", "/sub/", "/sub/token/", up.as_str(), up2.as_str()] {
            assert_eq!(get_path(missing).0, "HTTP/1.0 404 Not Found", "{missing}");
        }
        // HEAD：长度照写，没有内容，回完就关
        let (mut s, mut rd) = connect(&srv);
        s.write_all(b"HEAD /sub/token HTTP/1.1\r\n\r\n").unwrap();
        let mut all = String::new();
        rd.read_to_string(&mut all).unwrap();
        assert!(
            all.starts_with("HTTP/1.0 200 OK\r\n") && all.contains("\r\nContent-Length: 9\r\n") && all.ends_with("\r\n\r\n"),
            "{all}"
        );
        assert_eq!(fetch("POST /sub/token HTTP/1.1\r\nContent-Length: 0\r\n\r\n".to_string()).0, "HTTP/1.0 501 Not Implemented");
    }
}
