//! HTTP/1.1 的客户端：一个请求一条连接，测试里向本机的测试服务器发请求用（http 和 https；https 怎么验证书见 [`crate::tls`]）。
//!
//! 发出去的样子照 Python 的 http.client（原来的测试都用它）：请求行，Host、`Accept-Encoding: identity`、Content-Length，接着是
//! 自己加的头（照加的先后），空行，body；头和 body 一次写出去。自己加了 Host、Accept-Encoding、Content-Length（或者
//! Transfer-Encoding）的话，那一项就用自己的。回应认 Content-Length、chunked、读到连接关掉三种，100 Continue 跳过去。
//! 连、每次读、每次写各自最多等 [`Request::timeout`]（默认 30 秒），过了是 `TimedOut` 的错；连不上、对方断开这些错的种类
//! （`ConnectionRefused`、`ConnectionReset`、`BrokenPipe`、`UnexpectedEof`）原样留着，说明前面加上是哪个请求。
//!
//! ```ignore
//! let r = Request::new("POST", &format!("https://127.0.0.1:{port}/v1/team_get"))
//!     .header("Content-Type", "application/json")
//!     .body(raw)
//!     .tls(tls::Client::insecure())
//!     .timeout(Duration::from_secs(10))
//!     .send()?;
//! assert_eq!((r.status, r.header("content-type")), (200, Some("application/json")));
//! let j = r.json().expect("回的是 JSON");
//! ```
//!
//! 要在同一条连接上接着说别的（WebSocket 的升级）：[`Request::open`] 连上，[`Request::write_to`] 发请求，
//! [`Response::read_head`] 读回应的头，之后连接自己用（读要经同一个 `BufReader`：它可能已经多读了几个字节）。

use crate::json::{self, Value};
use crate::tls::{self, tcp_connect, timed};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// 一条连上的连接：明文的，或者握完手的 TLS
pub enum Conn {
    Plain(TcpStream),
    Tls(Box<tls::Stream>),
}

impl Conn {
    /// 底下的那条 TCP 连接（改时限、看地址）
    pub fn sock(&self) -> &TcpStream {
        match self {
            Conn::Plain(sock) => sock,
            Conn::Tls(stream) => stream.sock(),
        }
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Conn::Plain(sock) => sock.read(buf).map_err(timed),
            Conn::Tls(stream) => stream.read(buf),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Conn::Plain(sock) => sock.write(buf).map_err(timed),
            Conn::Tls(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Conn::Plain(sock) => sock.flush().map_err(timed),
            Conn::Tls(stream) => stream.flush(),
        }
    }
}

fn invalid(why: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why)
}

/// 网址拆成 (是不是 https, 主机, 端口, 路径和查询)
fn split_url(url: &str) -> Result<(bool, String, u16, String), String> {
    let (https, rest) = match (url.strip_prefix("https://"), url.strip_prefix("http://")) {
        (Some(rest), _) => (true, rest),
        (_, Some(rest)) => (false, rest),
        _ => return Err("要以 http:// 或 https:// 开头".to_string()),
    };
    let rest = rest.split('#').next().unwrap_or(rest);
    let (authority, target) = match rest.find(['/', '?']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let target = if target.starts_with('/') { target.to_string() } else { format!("/{target}") };
    // 主机：[IPv6]:端口、主机:端口、主机
    let (host, port) = match authority.strip_prefix('[') {
        Some(v6) => {
            let (host, after) = v6.split_once(']').ok_or("IPv6 的地址少了 ]")?;
            (host, after.strip_prefix(':'))
        }
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    if host.is_empty() || host.contains(['@', ' ']) {
        return Err(format!("主机（{host:?}）不成样子"));
    }
    let port = match port {
        Some(port) => port.parse::<u16>().map_err(|_| format!("端口（{port:?}）不是数"))?,
        None if https => 443,
        None => 80,
    };
    Ok((https, host.to_string(), port, target))
}

/// 一个请求：拼好了可以发几次（每次一条新连接）
#[derive(Clone)]
pub struct Request {
    method: String,
    https: bool,
    host: String,
    port: u16,
    target: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    tls: Option<tls::Client>,
    timeout: Duration,
}

impl Request {
    /// method（GET、POST……）和网址（`http://主机[:端口]/路径?查询`、`https://…`）；网址不成样子就 panic
    pub fn new(method: &str, url: &str) -> Request {
        let (https, host, port, target) = split_url(url).unwrap_or_else(|why| panic!("网址 {url:?} 不成样子：{why}"));
        Request {
            method: method.to_string(),
            https,
            host,
            port,
            target,
            headers: Vec::new(),
            body: None,
            tls: None,
            timeout: Duration::from_secs(30),
        }
    }

    pub fn get(url: &str) -> Request {
        Request::new("GET", url)
    }

    pub fn post(url: &str) -> Request {
        Request::new("POST", url)
    }

    /// 加一个头（照加的先后发出去；同一个名字加几次就发几行）
    pub fn header(mut self, name: &str, value: &str) -> Request {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// body（原样的字节）；Content-Length 自动写上
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Request {
        self.body = Some(body.into());
        self
    }

    /// body 是这份 JSON（紧凑、非 ASCII 原样写），带上 `Content-Type: application/json`
    pub fn json(self, value: &Value) -> Request {
        self.header("Content-Type", "application/json").body(value.compact())
    }

    /// https 时怎么验服务器的证书；不设就是不验（[`tls::Client::insecure`]）：测试服务器的证书是现造的。http 的请求不看它
    pub fn tls(mut self, client: tls::Client) -> Request {
        self.tls = Some(client);
        self
    }

    /// 连、每次读、每次写各自最多等多久（默认 30 秒）
    pub fn timeout(mut self, timeout: Duration) -> Request {
        self.timeout = timeout;
        self
    }

    /// 说明里怎么称呼这个请求
    fn what(&self) -> String {
        format!("{} {}://{}{}", self.method, if self.https { "https" } else { "http" }, self.authority(true), self.target)
    }

    /// Host 那一项的写法：端口是这种网址默认的就不写（always 时总是写）
    fn authority(&self, always: bool) -> String {
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        if !always && self.port == if self.https { 443 } else { 80 } {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }

    fn has_header(&self, name: &str) -> bool {
        self.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name))
    }

    /// 发出去的字节：请求行、头、空行、body
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut head = format!("{} {} HTTP/1.1\r\n", self.method, self.target);
        let mut put = |name: &str, value: &str| head.push_str(&format!("{name}: {value}\r\n"));
        if !self.has_header("Host") {
            put("Host", &self.authority(false));
        }
        if !self.has_header("Accept-Encoding") {
            put("Accept-Encoding", "identity");
        }
        if !self.has_header("Content-Length") && !self.has_header("Transfer-Encoding") {
            // 没有 body 的 POST、PUT、PATCH 也写 0（http.client 也是）
            let expects_body = ["POST", "PUT", "PATCH"].iter().any(|m| self.method.eq_ignore_ascii_case(m));
            match &self.body {
                Some(body) => put("Content-Length", &body.len().to_string()),
                None if expects_body => put("Content-Length", "0"),
                None => {}
            }
        }
        for (name, value) in &self.headers {
            put(name, value);
        }
        head.push_str("\r\n");
        let mut out = head.into_bytes();
        out.extend_from_slice(self.body.as_deref().unwrap_or_default());
        out
    }

    /// 给出错的说明加上是哪个请求（错的种类不变）
    fn named(&self, e: io::Error) -> io::Error {
        io::Error::new(e.kind(), format!("{}：{e}", self.what()))
    }

    /// 连上（https 的握完手），还没发请求
    pub fn open(&self) -> io::Result<Conn> {
        // 头里有换行的话，发出去就成了别的头：不发
        for (name, value) in &self.headers {
            if name.is_empty() || name.contains([':', '\r', '\n', ' ']) || value.contains(['\r', '\n']) {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{}：头 {name:?} 写得不成样子", self.what())));
            }
        }
        let sock = tcp_connect(&self.host, self.port, self.timeout).map_err(|e| self.named(e))?;
        if !self.https {
            return Ok(Conn::Plain(sock));
        }
        let client = self.tls.clone().unwrap_or_else(tls::Client::insecure);
        Ok(Conn::Tls(Box::new(client.wrap(sock, &self.host).map_err(|e| self.named(e))?)))
    }

    /// 把请求写到连接上
    pub fn write_to(&self, conn: &mut impl Write) -> io::Result<()> {
        conn.write_all(&self.to_bytes()).and_then(|()| conn.flush()).map_err(|e| self.named(e))
    }

    /// 发出去、把回应读完；连接用完就关
    pub fn send(&self) -> io::Result<Response> {
        let mut conn = BufReader::new(self.open()?);
        self.write_to(conn.get_mut())?;
        Response::read_from(&mut conn, &self.method).map_err(|e| self.named(e))
    }
}

/// 回应
#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    /// 状态码
    pub status: u16,
    /// 状态码后面的那句话
    pub reason: String,
    /// 头：名字照服务器写的，先后照来的
    pub headers: Vec<(String, String)>,
    /// body 的原样字节
    pub body: Vec<u8>,
}

/// 一行最长多少、头最多几行：超过了就是对面坏了，别一直读下去
const LINE_MAX: usize = 65536;
const HEADERS_MAX: usize = 200;

/// 读一行（到 \n 为止，去掉行尾的 \r\n）；一个字节都没读到就到头了是 None
fn read_line(r: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    r.take(LINE_MAX as u64 + 1).read_until(b'\n', &mut line)?;
    if line.is_empty() {
        return Ok(None);
    }
    if line.last() != Some(&b'\n') {
        let why = if line.len() > LINE_MAX { "一行太长" } else { "一行没写完连接就断了" };
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("回应读不下去：{why}")));
    }
    line.pop();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Ok(Some(line))
}

/// 头里的字节当 Latin-1 读（http.client 也是）
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|b| *b as char).collect()
}

impl Response {
    /// 读状态行和头（body 还没读，留在连接上）。服务器什么都没回就断开是 `UnexpectedEof` 的错
    pub fn read_head(r: &mut impl BufRead) -> io::Result<Response> {
        let Some(line) = read_line(r)? else {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "服务器什么都没回就断开了"));
        };
        let line = latin1(&line);
        let mut parts = line.splitn(3, ' ');
        let (version, status, reason) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default(), parts.next());
        let code = status.parse::<u16>().ok().filter(|code| status.len() == 3 && (100..1000).contains(code));
        let (true, Some(status)) = (version.starts_with("HTTP/"), code) else {
            return Err(invalid(format!("回应的第一行不成样子：{line:?}")));
        };
        let mut headers: Vec<(String, String)> = Vec::new();
        loop {
            let Some(line) = read_line(r)? else {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "回应的头没写完连接就断了"));
            };
            if line.is_empty() {
                break;
            }
            let line = latin1(&line);
            if line.starts_with([' ', '\t']) {
                // 接着上一行写的
                if let Some((_, value)) = headers.last_mut() {
                    value.push(' ');
                    value.push_str(line.trim());
                }
                continue;
            }
            let Some((name, value)) = line.split_once(':') else {
                return Err(invalid(format!("回应的头里有一行不成样子：{line:?}")));
            };
            if headers.len() >= HEADERS_MAX {
                return Err(invalid(format!("回应的头超过 {HEADERS_MAX} 行")));
            }
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
        Ok(Response { status, reason: reason.unwrap_or_default().trim().to_string(), headers, body: Vec::new() })
    }

    /// 读整个回应（100 Continue 跳过去）；method 是发的请求的：HEAD 的回应没有 body
    pub fn read_from(r: &mut impl BufRead, method: &str) -> io::Result<Response> {
        let mut response = loop {
            let head = Response::read_head(r)?;
            if head.status != 100 {
                break head;
            }
        };
        response.read_body(r, method)?;
        Ok(response)
    }

    /// 接着 [`Response::read_head`] 把 body 读进来
    pub fn read_body(&mut self, r: &mut impl BufRead, method: &str) -> io::Result<()> {
        let none = method.eq_ignore_ascii_case("HEAD") || self.status / 100 == 1 || self.status == 204 || self.status == 304;
        if none {
            return Ok(());
        }
        let mut body = Vec::new();
        if self.header("Transfer-Encoding").is_some_and(|v| v.trim().eq_ignore_ascii_case("chunked")) {
            loop {
                // 一块：十六进制的长度（后面可以有 ;扩展）一行，内容，再一个空行；长度是 0 的那一块后面是结尾的头，读到空行为止
                let line =
                    read_line(r)?.ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "分块的 body 没写完连接就断了"))?;
                let text = latin1(&line);
                let size = text.split(';').next().unwrap_or_default().trim();
                let size = usize::from_str_radix(size, 16).map_err(|_| invalid(format!("分块的长度不成样子：{text:?}")))?;
                if size == 0 {
                    while read_line(r)?.is_some_and(|line| !line.is_empty()) {}
                    break;
                }
                let at = body.len();
                body.resize(at + size, 0);
                r.read_exact(&mut body[at..])?;
                read_line(r)?;
            }
        } else if let Some(length) = self.header("Content-Length") {
            let length: usize = length.trim().parse().map_err(|_| invalid(format!("Content-Length 不是数：{length:?}")))?;
            r.take(length as u64).read_to_end(&mut body)?;
            if body.len() < length {
                let why = format!("body 说好 {length} 个字节，来了 {} 个连接就断了", body.len());
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, why));
            }
        } else {
            r.read_to_end(&mut body)?;
        }
        self.body = body;
        Ok(())
    }

    /// 头里的一项（名字不分大小写；有几行的话是第一行）
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// body 当文字（不是 UTF-8 的字节换成 U+FFFD）
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// body 读成 JSON
    pub fn json(&self) -> Result<Value, json::Error> {
        json::parse_bytes(&self.body)
    }
}

/// 单元测试共用的
#[cfg(test)]
pub(crate) mod testkit {
    use std::io::Read;

    /// 从连接上读一个请求：头，和 Content-Length 那么多的 body；对方中途断开就给读到的那些
    pub fn read_request(stream: &mut impl Read) -> Vec<u8> {
        let mut got = Vec::new();
        let mut byte = [0u8; 1];
        while !got.ends_with(b"\r\n\r\n") {
            if stream.read(&mut byte).unwrap_or(0) == 0 {
                return got;
            }
            got.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&got).to_lowercase();
        let length =
            head.lines().find_map(|line| line.strip_prefix("content-length:")).and_then(|v| v.trim().parse::<usize>().ok());
        let mut body = vec![0u8; length.unwrap_or(0)];
        if stream.read_exact(&mut body).is_ok() {
            got.extend(body);
        }
        got
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::read_request;
    use super::*;
    use crate::fsx::TempDir;
    use crate::tls::testkit::{ca_signed, openssls, serve};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// 只会照本宣科的服务器（明文）：每条连接读完请求、记下来，回 replies 里的下一段，关掉
    fn canned(replies: Vec<&'static [u8]>) -> (u16, mpsc::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (seen, requests) = mpsc::channel();
        std::thread::spawn(move || {
            for (mut sock, reply) in listener.incoming().flatten().zip(replies) {
                let _ = seen.send(read_request(&mut sock));
                let _ = sock.write_all(reply);
            }
        });
        (port, requests)
    }

    fn get(port: u16) -> io::Result<Response> {
        Request::get(&format!("http://127.0.0.1:{port}/x")).timeout(Duration::from_secs(10)).send()
    }

    /// 发出去的字节和 Python 的 http.client 一样（下面这几段是拿它实际发的抄下来的，端口换过）
    #[test]
    fn requests_look_like_http_client() {
        let post = Request::new("POST", "https://127.0.0.1:50762/v1/keys")
            .header("Content-Type", "application/json")
            .header("X-Cca-Sig", "c2ln")
            .body(b"{}".to_vec());
        assert_eq!(
            String::from_utf8(post.to_bytes()).unwrap(),
            "POST /v1/keys HTTP/1.1\r\nHost: 127.0.0.1:50762\r\nAccept-Encoding: identity\r\nContent-Length: 2\r\n\
             Content-Type: application/json\r\nX-Cca-Sig: c2ln\r\n\r\n{}"
        );
        let get = Request::get("http://127.0.0.1:50764/x?y=1").header("Upgrade", "websocket");
        assert_eq!(
            String::from_utf8(get.to_bytes()).unwrap(),
            "GET /x?y=1 HTTP/1.1\r\nHost: 127.0.0.1:50764\r\nAccept-Encoding: identity\r\nUpgrade: websocket\r\n\r\n"
        );
        // 没有 body 的 POST 也写长度
        assert_eq!(
            String::from_utf8(Request::post("http://127.0.0.1:50766/empty").to_bytes()).unwrap(),
            "POST /empty HTTP/1.1\r\nHost: 127.0.0.1:50766\r\nAccept-Encoding: identity\r\nContent-Length: 0\r\n\r\n"
        );
        // 自己写了 Host、Accept-Encoding：用自己的，照自己写的先后
        let own = Request::get("http://127.0.0.1:50768/h").header("Host", "relay.example.com").header("accept-encoding", "gzip");
        assert_eq!(
            String::from_utf8(own.to_bytes()).unwrap(),
            "GET /h HTTP/1.1\r\nHost: relay.example.com\r\naccept-encoding: gzip\r\n\r\n"
        );
        // 默认的端口不写进 Host；IPv6 的地址带方括号；JSON 的 body 不转义非 ASCII
        let default_port = Request::post("https://example.com").json(&crate::obj! { "名字" => "书房" });
        assert_eq!(
            String::from_utf8(default_port.to_bytes()).unwrap(),
            "POST / HTTP/1.1\r\nHost: example.com\r\nAccept-Encoding: identity\r\nContent-Length: 19\r\n\
             Content-Type: application/json\r\n\r\n{\"名字\":\"书房\"}"
        );
        assert!(String::from_utf8(Request::get("http://[::1]:8080/").to_bytes()).unwrap().contains("\r\nHost: [::1]:8080\r\n"));
    }

    #[test]
    fn urls() {
        let ok = |url: &str| split_url(url).unwrap();
        assert_eq!(ok("http://127.0.0.1:8080/v1/x?a=1"), (false, "127.0.0.1".to_string(), 8080, "/v1/x?a=1".to_string()));
        assert_eq!(ok("https://example.com"), (true, "example.com".to_string(), 443, "/".to_string()));
        assert_eq!(ok("http://example.com?q=1#frag"), (false, "example.com".to_string(), 80, "/?q=1".to_string()));
        assert_eq!(ok("https://[::1]:9/p"), (true, "::1".to_string(), 9, "/p".to_string()));
        assert_eq!(ok("https://[2001:db8::1]/"), (true, "2001:db8::1".to_string(), 443, "/".to_string()));
        for bad in
            ["ftp://example.com/", "example.com", "http://", "http://:80/", "http://example.com:port/", "http://u@example.com/"]
        {
            assert!(split_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn responses_of_every_shape() {
        let (port, requests) = canned(vec![
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Two: a\r\n  b\r\nContent-Length: 11\r\n\r\n{\"ok\":true}trailing junk",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6;ext=1\r\n world\r\n0\r\nX-Trailer: 1\r\n\r\n",
            b"HTTP/1.0 404 Not Found\r\n\r\nall of it",
            b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\nok",
            b"HTTP/1.1 204 No Content\r\n\r\n",
            b"HTTP/1.1 200\r\ncontent-length: 0\r\n\r\n",
        ]);
        let first = get(port).unwrap();
        assert_eq!((first.status, first.reason.as_str(), first.text().as_str()), (200, "OK", "{\"ok\":true}"));
        assert_eq!(first.header("content-type"), Some("application/json"));
        assert_eq!(first.header("X-TWO"), Some("a b"), "折行的头接回一行");
        assert_eq!(first.header("missing"), None);
        assert_eq!(first.json().unwrap(), crate::obj! { "ok" => true });
        assert_eq!(first.headers.len(), 3);
        // 服务器收到的就是 to_bytes() 的那些
        let sent = requests.recv().unwrap();
        assert_eq!(
            String::from_utf8(sent).unwrap(),
            format!("GET /x HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept-Encoding: identity\r\n\r\n")
        );

        assert_eq!(get(port).unwrap().text(), "hello world", "分块的");
        let third = get(port).unwrap();
        assert_eq!((third.status, third.reason.as_str(), third.text().as_str()), (404, "Not Found", "all of it"), "读到连接关掉");
        let fourth = get(port).unwrap();
        assert_eq!((fourth.status, fourth.text().as_str()), (201, "ok"), "100 Continue 跳过去");
        let fifth = get(port).unwrap();
        assert_eq!((fifth.status, fifth.body.len()), (204, 0));
        let sixth = get(port).unwrap();
        assert_eq!((sixth.status, sixth.reason.as_str(), sixth.body.len()), (200, "", 0), "状态码后面没有话");
    }

    #[test]
    fn bodies_are_sent_and_head_has_none() {
        let (port, requests) =
            canned(vec![b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok", b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n"]);
        let body = vec![0xe4u8, 0xb8, 0xad, 0, 255, b'\n'];
        let request = Request::post(&format!("http://127.0.0.1:{port}/up")).header("X-Extra", "1").body(body.clone());
        assert_eq!(request.send().unwrap().text(), "ok");
        let sent = requests.recv().unwrap();
        assert_eq!(sent, request.to_bytes());
        assert!(sent.ends_with(&body), "body 的字节原样到了");
        // HEAD：回应里写着长度，但后面没有 body
        let head = Request::new("HEAD", &format!("http://127.0.0.1:{port}/up")).send().unwrap();
        assert_eq!((head.status, head.header("Content-Length"), head.body.len()), (200, Some("10"), 0));
    }

    #[test]
    fn broken_responses_are_errors_that_name_the_request() {
        let (port, _requests) = canned(vec![
            b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc",
            b"",
            b"SSH-2.0-OpenSSH_9.9\r\n",
            b"HTTP/1.1 200 OK\r\nContent-Length: ten\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhel",
            b"HTTP/1.1 200 OK\r\nno colon here\r\n\r\n",
        ]);
        let kinds: Vec<io::ErrorKind> = (0..6).map(|_| get(port).expect_err("回应是坏的").kind()).collect();
        use io::ErrorKind::{InvalidData, UnexpectedEof};
        assert_eq!(kinds, [UnexpectedEof, UnexpectedEof, InvalidData, InvalidData, UnexpectedEof, InvalidData]);
        // 没人听的端口、不回话的服务器、头里夹着换行
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let quiet = listener.local_addr().unwrap().port();
        let started = std::time::Instant::now();
        let err = Request::get(&format!("http://127.0.0.1:{quiet}/slow")).timeout(Duration::from_millis(300)).send().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err}");
        assert!(err.to_string().starts_with(&format!("GET http://127.0.0.1:{quiet}/slow：")), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(listener);
        assert!(crate::fsx::settle_port(quiet), "关掉的端口还连得上");
        assert_eq!(get(quiet).unwrap_err().kind(), io::ErrorKind::ConnectionRefused);
        let split = Request::get(&format!("http://127.0.0.1:{port}/")).header("X-A", "1\r\nX-B: 2").send().unwrap_err();
        assert_eq!(split.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn https_with_and_without_verification() {
        let (dir, other) = (TempDir::new("http-unit-"), TempDir::new("http-unit-"));
        ca_signed(&openssls()[0], &dir);
        ca_signed(&openssls()[0], &other);
        let port = serve(&dir, |mut stream| {
            let request = read_request(&mut stream);
            let body = format!("got {} bytes", request.len());
            let _ = stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes());
        });
        let request = Request::post(&format!("https://127.0.0.1:{port}/v1/x")).body("0123456789").timeout(Duration::from_secs(10));
        let want = format!("got {} bytes", request.to_bytes().len());
        assert_eq!(request.send().unwrap().text(), want, "不设就是不验证书");
        let ca = tls::Client::with_ca_file(dir.join("ca.pem")).unwrap();
        assert_eq!(request.clone().tls(ca).send().unwrap().text(), want);
        let err = request.clone().tls(tls::Client::with_ca_file(other.join("ca.pem")).unwrap()).send().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{err}");
        assert!(err.to_string().starts_with(&format!("POST https://127.0.0.1:{port}/v1/x：")), "{err}");

        // 同一条连接上接着用：自己发请求、只读回应的头
        let mut conn = BufReader::new(request.open().unwrap());
        request.write_to(conn.get_mut()).unwrap();
        let mut head = Response::read_head(&mut conn).unwrap();
        assert_eq!((head.status, head.body.len()), (200, 0));
        head.read_body(&mut conn, "POST").unwrap();
        assert_eq!(head.text(), want);
        assert!(conn.get_ref().sock().peer_addr().unwrap().ip().is_loopback());
    }
}
