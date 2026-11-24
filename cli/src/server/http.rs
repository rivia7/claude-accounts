//! HTTP/1.0：读请求、写回复、带期限的连接。Python 那边是标准库的 `BaseHTTPRequestHandler`（Py:1799-1815），这里照它的行为自己写
//! （对照数据 tests/vectors/server_http.json 是真的 Python 服务器对一串原样的请求回的字节，front.rs 的测试逐个比）：
//! - 请求行最长 65536 个字节（超了回 414），一行请求头最长 65536 个字节、连结尾的空行最多 100 行（超了回 431）
//! - 请求行按空白切成几段：三段、第三段是 `HTTP/数字.数字` 才算带版本的请求（2.0 以上回 505）；两段的只认 GET，当 HTTP/0.9：
//!   回的时候不带状态行和头，只有正文（`send_response_only` 就是这样；`GET /x HTTP/0.9` 也是）。请求行没读明白时
//!   版本还没定下来，那几种出错的回复同样只有正文
//! - GET、POST 以外的方法回 501；HEAD 的出错回复不带正文
//! - 请求头照标准库 email 包的读法：`名字:` 开头的才是头，空格、制表符开头的接在上一个后面；遇到不像头的一行就到此为止，
//!   后面的都不算；值去掉开头的空格和制表符、结尾的回车换行（结尾的空格留着）
//! - 回复的头一行是 `HTTP/1.0 状态码 说明`，带 `Server: cca-presence/1 `（结尾那个空格 Python 就有）和 `Date`；
//!   一条连接只处理一个请求
//! - 连接从接进来起最多 15 秒（`CONN_DEADLINE`），握手时一次读写最多等 5 秒（`HANDSHAKE_TIMEOUT`）：`DeadlineStream`
//!   包着 `TcpStream` 记着到期的时间，每次读写之前把超时设成两个里短的那个（Python 是另起一个定时器到点把连接断掉；这里不用
//!   别的线程）；`Conn` 是明文的或者 TLS 的那一条
//!
//! do_POST 是先按 IP 限流（429）、看 Content-Length（411、413），然后才读正文（Py:1903-1913）：所以这里读头（`read_head`）和
//! 读正文（`Reader::read`）是分开的两步，中间那几步在 `handler::admit`。

use crate::py::json::{self, Fmt};
use crate::py::obj::{repr_str, V};
use crate::py::{num, text, time};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use std::io::{self, BufRead, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::os::fd::{AsRawFd, RawFd};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 请求行、一行请求头最长多少个字节（http.client 的 _MAXLINE）
const MAX_LINE: usize = 65536;
/// 请求头连结尾的空行最多多少行（http.client 的 _MAXHEADERS）
const MAX_HEADERS: usize = 100;
/// `Server` 那个头的值：server_version + ' ' + sys_version（Py:1801-1802：后一个是空的，所以结尾有个空格）
const SERVER: &str = "cca-presence/1 ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// 读进来的一个请求
#[derive(Clone, Debug)]
pub struct Request {
    pub method: Method,
    /// 请求行里的地址，原样（带着 `?` 后面的）
    pub target: String,
    /// 各个请求头，照来的先后；名字是原样的（查的时候不分大小写），值去掉了开头的空格、制表符和结尾的回车换行
    pub headers: Vec<(String, String)>,
    /// 正文：照 Content-Length 读到的（对方提前断开时比它短）
    pub body: Vec<u8>,
}

impl Request {
    /// `self.headers.get(name)`：名字不分大小写，有几个同名的是头一个
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// `self.headers.get_all(name)`：同名的全部，照先后（X-Forwarded-For 可以有几个）
    pub fn header_all(&self, name: &str) -> Vec<&str> {
        self.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str()).collect()
    }
}

// ---- 带期限的连接 ----

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "timed out")
}

/// 是不是等过了头（Python 的 TimeoutError：不回、不说话，直接断开）
pub fn is_timeout(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
}

/// 记着期限的套接字：每次读写最多等 per_op，整条连接到 deadline 为止（一个字节一个字节慢慢发也占不住，Py:2099-2108）
pub struct DeadlineStream {
    sock: TcpStream,
    deadline: Option<Instant>,
    per_op: Duration,
}

impl DeadlineStream {
    pub fn new(sock: TcpStream, deadline: Option<Instant>, per_op: Duration) -> DeadlineStream {
        DeadlineStream { sock, deadline, per_op }
    }

    /// 一次读写最多等多久（握手完了从 5 秒换成 15 秒；成了 WebSocket 以后是它自己的）
    pub fn set_per_op(&mut self, per_op: Duration) {
        self.per_op = per_op;
    }

    /// 不再有整条连接的期限（成了 WebSocket：要开很久，Py:2053-2057）
    pub fn clear_deadline(&mut self) {
        self.deadline = None;
    }

    pub fn sock(&self) -> &TcpStream {
        &self.sock
    }

    /// 这一次读写最多等多久；已经到期是 Err
    fn wait(&self) -> io::Result<Duration> {
        let mut wait = self.per_op;
        if let Some(deadline) = self.deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(timed_out());
            }
            wait = wait.min(left);
        }
        Ok(wait.max(Duration::from_millis(1))) // 0 是“不限”的意思，套接字不收
    }
}

impl Read for DeadlineStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            // 设不上不算错（macOS 上对方已经把连接重置了的套接字设不了，是 EINVAL）：照旧去读，读到的才是真的原因
            let _ = self.sock.set_read_timeout(Some(self.wait()?));
            match self.sock.read(buf) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                // 超时在各个系统上是 EAGAIN 或者 ETIMEDOUT：都报成 TimedOut（rustls 把 WouldBlock 当成“套接字是不阻塞的，回头再来”）
                Err(e) if is_timeout(&e) => return Err(timed_out()),
                done => return done,
            }
        }
    }
}

impl Write for DeadlineStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        loop {
            let _ = self.sock.set_write_timeout(Some(self.wait()?));
            match self.sock.write(buf) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if is_timeout(&e) => return Err(timed_out()),
                done => return done,
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 一条连接：明文的（--behind-proxy、本机试网页端的那个监听）或者 TLS 的
pub enum Conn {
    Plain(DeadlineStream),
    Tls(Box<StreamOwned<ServerConnection, DeadlineStream>>),
}

impl Conn {
    /// 握手（一次读写等多久照 stream 现在的设置）。握不成是 Err：调用方不回、不说话，直接断开（Py:2114-2120）
    pub fn tls(mut stream: DeadlineStream, config: Arc<ServerConfig>) -> io::Result<Conn> {
        let mut tls = ServerConnection::new(config).map_err(io::Error::other)?;
        while tls.is_handshaking() {
            // 一个字节都没读没写却还在握手：不会再有进展
            if tls.complete_io(&mut stream)? == (0, 0) && tls.is_handshaking() {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
        }
        Ok(Conn::Tls(Box::new(StreamOwned::new(tls, stream))))
    }

    pub fn stream(&self) -> &DeadlineStream {
        match self {
            Conn::Plain(s) => s,
            Conn::Tls(t) => &t.sock,
        }
    }

    pub fn stream_mut(&mut self) -> &mut DeadlineStream {
        match self {
            Conn::Plain(s) => s,
            Conn::Tls(t) => &mut t.sock,
        }
    }

    /// TLS 里已经解出来、还没读走的字节有没有。这些字节不会再让套接字“可读”：等套接字之前要先看它（网页端的 WebSocket 那一头用）
    pub fn pending(&mut self) -> bool {
        match self {
            Conn::Plain(_) => false,
            Conn::Tls(t) => matches!(t.conn.reader().fill_buf(), Ok(chunk) if !chunk.is_empty()),
        }
    }

    /// 说完了：TLS 的先说一声 close_notify，再关掉写的那一半（Python 的 shutdown_request 是 SHUT_WR 然后关）。出错不管：对方可能已经走了
    pub fn finish(mut self) {
        if let Conn::Tls(t) = &mut self {
            t.conn.send_close_notify();
            let _ = t.flush();
        }
        let _ = self.stream().sock.shutdown(Shutdown::Write);
    }
}

impl AsRawFd for Conn {
    fn as_raw_fd(&self) -> RawFd {
        self.stream().sock.as_raw_fd()
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Conn::Plain(s) => s.read(buf),
            // 对方没说 close_notify 就断开：当成读到头（Python 的 ssl 默认也是，suppress_ragged_eofs）
            Conn::Tls(t) => match t.read(buf) {
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(0),
                done => done,
            },
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Conn::Plain(s) => s.write(buf),
            Conn::Tls(t) => t.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Conn::Plain(s) => s.flush(),
            Conn::Tls(t) => t.flush(),
        }
    }
}

// ---- 读请求 ----

/// 带缓冲地读（Python 的 rfile）：一行一行读请求行和头，再照长度读正文；读头时多读进来的字节留在缓冲里（`into_parts` 给出来：
/// 升级成 WebSocket 时，握手后面紧跟着的那几个字节）
pub struct Reader<R: Read> {
    io: R,
    buf: Vec<u8>,
    pos: usize,
}

impl<R: Read> Reader<R> {
    /// 一次从连接里读这么多
    const CHUNK: usize = 8192;

    pub fn new(io: R) -> Reader<R> {
        Reader { io, buf: Vec::new(), pos: 0 }
    }

    pub fn io(&mut self) -> &mut R {
        &mut self.io
    }

    /// 连接和缓冲里还没用掉的字节
    pub fn into_parts(self) -> (R, Vec<u8>) {
        let rest = self.buf.get(self.pos..).unwrap_or_default().to_vec();
        (self.io, rest)
    }

    /// 缓冲里还没用掉的
    fn rest(&self) -> &[u8] {
        self.buf.get(self.pos..).unwrap_or_default()
    }

    /// 缓冲用完了：再读一块。读到头是 false
    fn fill(&mut self) -> io::Result<bool> {
        self.buf.resize(Self::CHUNK, 0);
        self.pos = 0;
        let n = loop {
            match self.io.read(&mut self.buf) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                done => break done,
            }
        };
        self.buf.truncate(*n.as_ref().unwrap_or(&0));
        Ok(n? > 0)
    }

    /// `rfile.readline(limit)`：读到换行（带着它）、满 limit 个字节或者读到头为止
    pub fn line(&mut self, limit: usize) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        loop {
            let rest = self.rest();
            let room = limit.saturating_sub(out.len());
            let part = rest.get(..room.min(rest.len())).unwrap_or_default();
            let (take, done) = match part.iter().position(|b| *b == b'\n') {
                Some(i) => (i.saturating_add(1), true),
                None => (part.len(), false),
            };
            out.extend_from_slice(part.get(..take).unwrap_or_default());
            self.pos = self.pos.saturating_add(take);
            if done || out.len() >= limit || !self.fill()? {
                return Ok(out);
            }
        }
    }

    /// `rfile.read(n)`：读满 n 个字节，对方先断开了就是读到的那些。n 是查过上限的（最多 1 MiB）
    pub fn read(&mut self, n: usize) -> io::Result<Vec<u8>> {
        let mut out = Vec::with_capacity(n.min(Self::CHUNK));
        loop {
            let rest = self.rest();
            let take = n.saturating_sub(out.len()).min(rest.len());
            out.extend_from_slice(rest.get(..take).unwrap_or_default());
            self.pos = self.pos.saturating_add(take);
            if out.len() >= n || !self.fill()? {
                return Ok(out);
            }
        }
    }
}

/// 读到的请求头（正文还没读）
pub enum Head {
    /// 一个请求。v09：HTTP/0.9 的，回的时候不带状态行和头
    Req { req: Request, v09: bool },
    /// 请求不对：整份回复都在这里了（照 `send_error`），写出去就断开
    Reply(Vec<u8>),
    /// 不回，直接断开（一个字节都没来、请求行是空的）
    Close,
}

/// 每个字节是一个字符（Python 的 `str(raw, 'iso-8859-1')`）
fn latin1(raw: &[u8]) -> String {
    raw.iter().map(|b| char::from(*b)).collect()
}

/// 写回去（`.encode('latin-1')`）。这里的字符都是从 `latin1` 来的，放不进一个字节的不会有
fn to_latin1(s: &str) -> Vec<u8> {
    s.chars().map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?')).collect()
}

/// 读请求行和请求头：`handle_one_request` 的前半和 `parse_request`。now 是出错的回复里 Date 用的时间
pub fn read_head<R: Read>(r: &mut Reader<R>, now: f64) -> io::Result<Head> {
    let raw = r.line(MAX_LINE.saturating_add(1))?;
    if raw.len() > MAX_LINE {
        return Ok(Head::Reply(error_reply(414, None, None, "", "", now)));
    }
    if raw.is_empty() {
        return Ok(Head::Close);
    }
    let line = latin1(&raw);
    let line = text::rstrip_chars(&line, "\r\n");
    let words = text::split_ws(line);
    let (Some(first), Some(last)) = (words.first().copied(), words.last().copied()) else {
        return Ok(Head::Close);
    };
    // 版本还没认出来时是 HTTP/0.9（default_request_version）：这时候出错的回复只有正文
    let mut version = "HTTP/0.9";
    if words.len() >= 3 {
        let base = last.strip_prefix("HTTP/");
        let parts: Vec<&str> = base.unwrap_or_default().split('.').collect();
        let number = |p: &&str| if text::isdigit(p) && text::len(p) <= 10 { text::to_int(p) } else { None };
        let major = match (base, parts.as_slice()) {
            (Some(_), [major, minor]) => number(major).zip(number(minor)).map(|(major, _)| major),
            _ => None,
        };
        let Some(major) = major else {
            let message = format!("Bad request version ({})", repr_str(last));
            return Ok(Head::Reply(error_reply(400, Some(&message), None, version, "", now)));
        };
        if major >= 2 {
            let message = format!("Invalid HTTP version ({})", base.unwrap_or_default());
            return Ok(Head::Reply(error_reply(505, Some(&message), None, version, "", now)));
        }
        version = last;
    }
    let (command, path) = match words.as_slice() {
        [command, path] | [command, path, _] => (*command, *path),
        _ => {
            let message = format!("Bad request syntax ({})", repr_str(line));
            return Ok(Head::Reply(error_reply(400, Some(&message), None, version, "", now)));
        }
    };
    debug_assert_eq!(command, first);
    if words.len() == 2 && command != "GET" {
        let message = format!("Bad HTTP/0.9 request type ({})", repr_str(command));
        return Ok(Head::Reply(error_reply(400, Some(&message), None, version, "", now)));
    }
    // 开头连着几个 / 的只留一个（标准库防着 //host 被当成另一个站的地址）
    let target = match path.starts_with("//") {
        true => format!("/{}", path.trim_start_matches('/')),
        false => path.to_string(),
    };
    // 请求头：一行一行读到空行或者读到头（http.client 的 _read_headers）
    let mut block = Vec::new();
    let mut lines = 0usize;
    loop {
        let raw = r.line(MAX_LINE.saturating_add(1))?;
        if raw.len() > MAX_LINE {
            let explain = format!("got more than {MAX_LINE} bytes when reading header line");
            return Ok(Head::Reply(error_reply(431, Some("Line too long"), Some(&explain), version, command, now)));
        }
        lines = lines.saturating_add(1);
        if lines > MAX_HEADERS {
            let explain = format!("got more than {MAX_HEADERS} headers");
            return Ok(Head::Reply(error_reply(431, Some("Too many headers"), Some(&explain), version, command, now)));
        }
        let end = matches!(raw.as_slice(), b"\r\n" | b"\n" | b"");
        block.extend_from_slice(&raw);
        if end {
            break;
        }
    }
    let method = match command {
        "GET" => Method::Get,
        "POST" => Method::Post,
        _ => {
            let message = format!("Unsupported method ({})", repr_str(command));
            return Ok(Head::Reply(error_reply(501, Some(&message), None, version, command, now)));
        }
    };
    let req = Request { method, target, headers: parse_headers(&block), body: Vec::new() };
    Ok(Head::Req { req, v09: version == "HTTP/0.9" })
}

/// 按 \r\n、\r、\n 断行，行尾留着（email 包读头时就是这么断的：别的像换行的字符不算）
fn split_lines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let end = match c {
            '\n' => i.saturating_add(1),
            '\r' => match chars.peek() {
                Some((j, '\n')) => {
                    let end = j.saturating_add(1);
                    chars.next();
                    end
                }
                _ => i.saturating_add(1),
            },
            _ => continue,
        };
        out.push(text.get(start..end).unwrap_or_default());
        start = end;
    }
    if start < text.len() {
        out.push(text.get(start..).unwrap_or_default());
    }
    out
}

/// 请求头那一块（连着结尾的空行）→ 各个头。照 email 包的 FeedParser（compat32）：
/// - 像头的行（`From ` 开头、`名字:` 开头——名字是不带空格和冒号的可见 ASCII、可以是空的——、空格或制表符开头）一行行收，
///   遇到头一个不像的（空行也是）就到此为止
/// - 空格、制表符开头的接在上一个头后面（前面没有头的不要）；`From ` 开头的、名字是空的不要
/// - 值：冒号后面去掉开头的空格和制表符，接上续行（原样，带着行尾），最后去掉结尾的回车换行
fn parse_headers(block: &[u8]) -> Vec<(String, String)> {
    let text = latin1(block);
    let mut out: Vec<(String, String)> = Vec::new();
    // 正在收的那个头：名字、到现在为止的值
    let mut last: Option<(String, String)> = None;
    let mut flush = |last: &mut Option<(String, String)>| {
        if let Some((name, value)) = last.take() {
            out.push((name, text::rstrip_chars(&value, "\r\n").to_string()));
        }
    };
    for line in split_lines(&text) {
        if line.starts_with([' ', '\t']) {
            if let Some((_, value)) = last.as_mut() {
                value.push_str(line);
            }
            continue;
        }
        let named = line.split_once(':').filter(|(name, _)| name.chars().all(|c| ('\u{21}'..='\u{7e}').contains(&c)));
        if !line.starts_with("From ") && named.is_none() {
            break;
        }
        flush(&mut last);
        match named {
            Some((name, value)) if !line.starts_with("From ") && !name.is_empty() => {
                last = Some((name.to_string(), value.trim_start_matches([' ', '\t']).to_string()));
            }
            _ => {}
        }
    }
    flush(&mut last);
    out
}

// ---- 写回复 ----

/// 状态码的说明（http.HTTPStatus 的 phrase）；不认识的是空的
fn reason(code: u16) -> &'static str {
    match code {
        100 => "Continue",
        101 => "Switching Protocols",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Content Too Large",
        414 => "URI Too Long",
        415 => "Unsupported Media Type",
        416 => "Range Not Satisfiable",
        417 => "Expectation Failed",
        421 => "Misdirected Request",
        422 => "Unprocessable Content",
        423 => "Locked",
        424 => "Failed Dependency",
        425 => "Too Early",
        426 => "Upgrade Required",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        451 => "Unavailable For Legal Reasons",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        507 => "Insufficient Storage",
        508 => "Loop Detected",
        _ => "",
    }
}

/// 出错的回复里那句长一点的解释（HTTPStatus 的 description）：只有 `read_head` 会回的那几个
fn explanation(code: u16) -> &'static str {
    match code {
        400 => "Bad request syntax or unsupported method",
        414 => "URI is too long",
        431 => "The server is unwilling to process the request because its header fields are too large",
        501 => "Server does not support this operation",
        505 => "Cannot fulfill request",
        _ => "???",
    }
}

/// `email.utils.formatdate(t, usegmt=True)`：`Tue, 24 Nov 2026 10:28:00 GMT`
fn http_date(t: f64) -> String {
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    // datetime.fromtimestamp 把秒的小数四舍六入到微秒：差不到半微秒就到下一秒的算下一秒
    let whole = t.floor();
    let secs = if num::round((t - whole) * 1e6) >= 1_000_000 { whole + 1.0 } else { whole };
    let tm = time::gmtime(secs);
    let pick = |names: &[&'static str], i: i32| usize::try_from(i).ok().and_then(|i| names.get(i).copied()).unwrap_or("???");
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        pick(&DAYS, tm.wday),
        tm.mday,
        pick(&MONTHS, tm.mon - 1),
        tm.year,
        tm.hour,
        tm.min,
        tm.sec
    )
}

/// 回复开头的那几行：状态行、Server、Date（`send_response`）。message 是状态行里的说明
fn start(code: u16, message: &str, now: f64) -> Vec<u8> {
    to_latin1(&format!("HTTP/1.0 {code} {message}\r\nServer: {SERVER}\r\nDate: {}\r\n", http_date(now)))
}

fn header(out: &mut Vec<u8>, name: &str, value: &str) {
    out.extend_from_slice(&to_latin1(&format!("{name}: {value}\r\n")));
}

/// JSON 的回复（Py:1808-1815）：紧凑的 UTF-8，带 `Cache-Control: no-store`
pub fn json_reply(code: u16, obj: &V, v09: bool, now: f64) -> Vec<u8> {
    let body = json::dumps(obj, Fmt::new().utf8().compact());
    let headers = [
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Content-Length".to_string(), body.len().to_string()),
        ("Cache-Control".to_string(), "no-store".to_string()),
    ];
    raw_reply(code, &headers, body.as_bytes(), v09, now)
}

/// 别的回复（网页端的文件、304）：状态行、Server、Date、给的那几个头（照先后）、正文
pub fn raw_reply(code: u16, headers: &[(String, String)], body: &[u8], v09: bool, now: f64) -> Vec<u8> {
    if v09 {
        return body.to_vec();
    }
    let mut out = start(code, reason(code), now);
    for (name, value) in headers {
        header(&mut out, name, value);
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(body);
    out
}

/// 升级成 WebSocket 的那一句（Py:1873-1874：是原样写出去的，不带 Server、Date）
pub fn switching(accept: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    )
    .into_bytes()
}

/// `send_error(code, message, explain)`：一小段 HTML。version 是这时候认出来的版本（HTTP/0.9：只有正文；请求行太长时是空的），
/// command 是方法（HEAD 的不带正文）
fn error_reply(code: u16, message: Option<&str>, explain: Option<&str>, version: &str, command: &str, now: f64) -> Vec<u8> {
    let message = message.unwrap_or_else(|| reason(code));
    let explain = explain.unwrap_or_else(|| explanation(code));
    // html.escape(s, quote=False)
    let escape = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let body = format!(
        concat!(
            "<!DOCTYPE HTML>\n",
            "<html lang=\"en\">\n",
            "    <head>\n",
            "        <meta charset=\"utf-8\">\n",
            "        <style type=\"text/css\">\n",
            "            :root {{\n",
            "                color-scheme: light dark;\n",
            "            }}\n",
            "        </style>\n",
            "        <title>Error response</title>\n",
            "    </head>\n",
            "    <body>\n",
            "        <h1>Error response</h1>\n",
            "        <p>Error code: {code}</p>\n",
            "        <p>Message: {message}.</p>\n",
            "        <p>Error code explanation: {code} - {explain}.</p>\n",
            "    </body>\n",
            "</html>\n"
        ),
        code = code,
        message = escape(message),
        explain = escape(explain)
    );
    let mut out = Vec::new();
    if version != "HTTP/0.9" {
        out = start(code, message, now);
        header(&mut out, "Connection", "close");
        header(&mut out, "Content-Type", "text/html;charset=utf-8");
        header(&mut out, "Content-Length", &body.len().to_string());
        out.extend_from_slice(b"\r\n");
    }
    if command != "HEAD" {
        out.extend_from_slice(body.as_bytes());
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn head(raw: &[u8]) -> Head {
        read_head(&mut Reader::new(raw), 0.0).unwrap()
    }

    fn headers(raw: &str) -> Vec<(String, String)> {
        match head(format!("GET / HTTP/1.0\r\n{raw}").as_bytes()) {
            Head::Req { req, .. } => req.headers,
            _ => panic!("不是一个请求：{raw:?}"),
        }
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn headers_are_case_insensitive_and_first_wins() {
        let req = Request {
            method: Method::Post,
            target: "/v1/report".into(),
            headers: pairs(&[
                ("Content-Length", "2"),
                ("X-Forwarded-For", "198.51.100.7"),
                ("x-cca-sig", "AAAA"),
                ("x-forwarded-for", "192.0.2.1, 192.0.2.2"),
                ("X-Cca-Sig", "BBBB"),
            ]),
            body: b"{}".to_vec(),
        };
        assert_eq!((req.header("content-length"), req.header("X-Cca-Sig"), req.header("Origin")), (Some("2"), Some("AAAA"), None));
        assert_eq!(req.header_all("X-FORWARDED-FOR"), ["198.51.100.7", "192.0.2.1, 192.0.2.2"]);
        assert!(req.header_all("Upgrade").is_empty() && req.method == Method::Post && req.method != Method::Get);
    }

    /// 请求行：带版本的、HTTP/0.9 的、地址开头连着的 /
    #[test]
    fn request_lines() {
        let req = |raw: &str| match head(raw.as_bytes()) {
            Head::Req { req, v09 } => (req.method, req.target, v09),
            _ => panic!("不是一个请求：{raw:?}"),
        };
        assert_eq!(req("GET /v1/health HTTP/1.0\r\n\r\n"), (Method::Get, "/v1/health".into(), false));
        assert_eq!(req("POST /v1/report?x HTTP/1.1\nA: b\n\n"), (Method::Post, "/v1/report?x".into(), false));
        assert_eq!(req("GET /v1/health\r\n\r\n"), (Method::Get, "/v1/health".into(), true));
        assert_eq!(req("GET /v1/health HTTP/0.9\r\n\r\n"), (Method::Get, "/v1/health".into(), true));
        assert_eq!(req("GET ///a//b HTTP/1.9\r\n"), (Method::Get, "/a//b".into(), false));
        assert_eq!(req(" \tGET\t/x \x1f HTTP/01.00\r\r\n"), (Method::Get, "/x".into(), false), "按 Python 认的空白切");
        assert_eq!(req("GET /caf\u{e9} HTTP/1.0"), (Method::Get, "/caf\u{c3}\u{a9}".into(), false), "每个字节一个字符");
        assert!(
            matches!(head(b""), Head::Close) && matches!(head(b"\r\n"), Head::Close) && matches!(head(b" \t \r\n"), Head::Close)
        );
    }

    /// 请求头：照 email 包的读法（每一条都和真的 Python 服务器对过，见 tests/vectors/server_http.json 里同名的那几项）
    #[test]
    fn header_block() {
        assert_eq!(headers("A: b\r\nC:d\r\nE:\t f \r\n\r\n"), pairs(&[("A", "b"), ("C", "d"), ("E", "f ")]), "结尾的空格留着");
        assert_eq!(headers("A: b\nC: d\n\n"), pairs(&[("A", "b"), ("C", "d")]));
        assert_eq!(headers("A: b\r\n c\r\n\td\r\nE: f\r\n\r\n"), pairs(&[("A", "b\r\n c\r\n\td"), ("E", "f")]), "续行原样接上");
        assert_eq!(headers("A:\r\n 7\r\n\r\n"), pairs(&[("A", "\r\n 7")]));
        assert_eq!(headers(" x\r\nA: b\r\n\r\n"), pairs(&[("A", "b")]), "前面没有头的续行不要");
        assert_eq!(headers(": x\r\n y\r\nA: b\r\n\r\n"), pairs(&[("A", "b")]), "名字是空的不要，它的续行也不要");
        assert_eq!(headers("From x\r\nA: b\r\nFrom y\r\n z\r\nC: d\r\nFrom w\r\n\r\n"), pairs(&[("A", "b"), ("C", "d")]));
        assert_eq!(headers("From: x\r\n\r\n"), pairs(&[("From", "x")]));
        assert_eq!(headers("A: b\r\njunk\r\nC: d\r\n\r\n"), pairs(&[("A", "b")]), "不像头的一行：到此为止");
        assert_eq!(headers("A: b\r\nX Y: z\r\nC: d\r\n\r\n"), pairs(&[("A", "b")]));
        assert_eq!(headers("A: b\r\nX-\u{e9}: z\r\nC: d\r\n\r\n"), pairs(&[("A", "b")]));
        assert_eq!(headers("A : b\r\nC: d\r\n\r\n"), pairs(&[]));
        assert_eq!(headers("A: b\r\r\nC: d\r\n\r\n"), pairs(&[("A", "b")]), "单独的回车也断行：后面那个空行就是头的结尾");
        assert_eq!(headers("A: b\x0bC: d\r\nE: f\u{85}G: h\r\n\r\n"), pairs(&[("A", "b\x0bC: d"), ("E", "f\u{c2}\u{85}G: h")]));
        assert_eq!(headers("A: b"), pairs(&[("A", "b")]), "读到头也算结束");
        assert_eq!(headers("A: b\r\nA: c\r\na: d\r\n\r\n"), pairs(&[("A", "b"), ("A", "c"), ("a", "d")]));
        assert_eq!(split_lines("a\r\nb\rc\n\r\r\nd"), ["a\r\n", "b\r", "c\n", "\r", "\r\n", "d"]);
        assert!(split_lines("").is_empty());
    }

    /// 一行一行读、照长度读：缓冲里剩下的留给后面
    #[test]
    fn reader_lines_and_bodies() {
        let mut r = Reader::new(&b"ab\ncdef\n\nxyz"[..]);
        assert_eq!(
            (r.line(10).unwrap(), r.line(2).unwrap(), r.line(10).unwrap()),
            (b"ab\n".to_vec(), b"cd".to_vec(), b"ef\n".to_vec())
        );
        assert_eq!((r.line(10).unwrap(), r.read(2).unwrap()), (b"\n".to_vec(), b"xy".to_vec()));
        assert_eq!(r.into_parts().1, b"z");
        let mut r = Reader::new(&b"abc"[..]);
        assert_eq!(
            (r.read(0).unwrap(), r.read(10).unwrap(), r.read(10).unwrap(), r.line(10).unwrap()),
            (vec![], b"abc".to_vec(), vec![], vec![])
        );
        // 比一块大的：分几次读进来
        let big: Vec<u8> = (0..20000u32).map(|i| (i % 251) as u8).filter(|b| *b != b'\n').collect();
        let mut text = big.clone();
        text.extend_from_slice(b"\nrest");
        let mut r = Reader::new(&text[..]);
        assert_eq!(r.line(1 << 20).unwrap().len(), big.len() + 1);
        assert_eq!(r.read(100).unwrap(), b"rest");
        let mut r = Reader::new(&text[..]);
        assert_eq!((r.read(big.len()).unwrap(), r.line(5).unwrap()), (big, b"\n".to_vec()));
    }

    /// Date 的写法；秒的小数差不到半微秒就到下一秒的，算下一秒（Python 的 datetime 这么取整）
    #[test]
    fn dates() {
        assert_eq!(http_date(0.0), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(http_date(1_795_516_080.25), "Tue, 24 Nov 2026 10:28:00 GMT");
        assert_eq!(http_date(951_782_399.999_999_2), "Mon, 28 Feb 2000 23:59:59 GMT");
        assert_eq!(http_date(951_782_399.999_999_8), "Tue, 29 Feb 2000 00:00:00 GMT");
    }

    /// 回复的样子（整份的字节和 Python 的对照在 front.rs 的测试里）
    #[test]
    fn replies() {
        let text = |raw: Vec<u8>| String::from_utf8(raw).unwrap();
        let v = json::loads(r#"{"error":"bad","名字":"书房"}"#).unwrap();
        assert_eq!(
            text(json_reply(400, &v, false, 0.0)),
            "HTTP/1.0 400 Bad Request\r\nServer: cca-presence/1 \r\nDate: Thu, 01 Jan 1970 00:00:00 GMT\r\nContent-Type: application/json\r\n\
             Content-Length: 33\r\nCache-Control: no-store\r\n\r\n{\"error\":\"bad\",\"名字\":\"书房\"}"
        );
        assert_eq!(text(json_reply(200, &v, true, 0.0)), "{\"error\":\"bad\",\"名字\":\"书房\"}", "HTTP/0.9：只有正文");
        assert_eq!(
            text(raw_reply(304, &pairs(&[("ETag", "\"x\"")]), b"", false, 0.0)),
            "HTTP/1.0 304 Not Modified\r\nServer: cca-presence/1 \r\nDate: Thu, 01 Jan 1970 00:00:00 GMT\r\nETag: \"x\"\r\n\r\n"
        );
        assert!(text(raw_reply(599, &[], b"", false, 0.0)).starts_with("HTTP/1.0 599 \r\n"), "不认识的状态码：说明是空的");
        assert_eq!(
            text(switching("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")),
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n"
        );
        for code in [200u16, 304, 400, 403, 404, 409, 411, 413, 429, 507] {
            assert!(!reason(code).is_empty(), "{code}");
        }
    }

    /// 期限：一次读写最多等 per_op；整条连接到点以后读写马上出错；对方断开是读到头
    #[test]
    fn deadlines() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let (sock, _) = l.accept().unwrap();
        let begin = Instant::now();
        let mut s = DeadlineStream::new(sock, Some(begin + Duration::from_millis(250)), Duration::from_millis(40));
        let mut buf = [0u8; 8];
        let e = s.read(&mut buf).unwrap_err();
        assert!(is_timeout(&e) && e.kind() == io::ErrorKind::TimedOut, "{e}");
        let waited = begin.elapsed();
        assert!(waited >= Duration::from_millis(35) && waited < Duration::from_millis(200), "{waited:?}");
        (&client).write_all(b"hi").unwrap();
        assert_eq!(s.read(&mut buf).unwrap(), 2);
        s.set_per_op(Duration::from_secs(5));
        let e = s.read(&mut buf).unwrap_err(); // per_op 是 5 秒，可整条连接只剩 200 毫秒
        assert!(is_timeout(&e) && begin.elapsed() >= Duration::from_millis(240) && begin.elapsed() < Duration::from_secs(2));
        assert!(is_timeout(&s.read(&mut buf).unwrap_err()) && is_timeout(&s.write(b"x").unwrap_err()), "到点以后不再读写");
        s.clear_deadline();
        s.set_per_op(Duration::from_millis(40));
        assert_eq!(s.write(b"ok").unwrap(), 2);
        let mut got = [0u8; 2];
        (&client).read_exact(&mut got).unwrap(); // 读走了再关：留着没读的字节关掉，对方收到的是连接被重置
        drop(client);
        assert_eq!(s.read(&mut buf).unwrap(), 0, "对方断开：读到头");
    }
}
