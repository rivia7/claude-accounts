//! 连证书机构用的那一点 HTTP 客户端。Python 那一份用的是 urllib（Acme:247-255）；这里只做它用到的：一个请求一个连接
//! （HTTP/1.1、`Connection: close`），https（http 是测试、内网的证书机构用的），GET、HEAD、POST。
//!
//! 发出去的请求和 urllib 发的一样（头的先后也是）。读回应照 http.client 的规矩——对照数据
//! cli/tests/vectors/server_acme_http.json 是真的拿它读一个个回应录下来的：
//! - 状态行、每行头最长 65536 个字节，头最多 100 行（结尾的空行也算）；`100 Continue` 跳过
//! - 内容：HEAD 没有；`Transfer-Encoding: chunked` 一块块读（Let's Encrypt 的服务器用它）；204、304 没有；有 Content-Length 的读
//!   那么多；都没有的读到对面关掉。哪一种都最多要 1 MiB（Acme:251 的 `r.read(1 << 20)`），多的不读
//! - 头照 Python（email 的解析器）的办法拆：名字不分大小写、取头一个，续行接在上一个后面，碰到不像头的一行就到此为止
//! - 连接、握手、每一次读写最多等 30 秒
//!
//! 和 Python 不一样的：
//! - 不跟着 3xx 转；不看 http_proxy 这些环境变量和系统的代理设置；只认 http、https（urllib 还认 file、ftp）
//! - 没成时给的是一个英文的名字，放进“连不上证书机构（…）”的括号里。Python 那里是异常的类名（多半是笼统的 URLError），这里是
//!   具体一点的原因：`ConnectionRefused`、`TimedOut`、`InvalidCertificate(UnknownIssuer)`、`BadStatusLine`……
//! - 回应写坏了（状态行不对、头太多、chunked 没写完）在 Python 里是没人接的异常，整个进程退出；这里也算连不上，过一阵再试
//! - chunked 最后那一段后面的头（trailer）最多读 100 行（Python 没有上限）

use super::lines::Lines;
use super::AGENT;
use crate::py::text;
use crate::py::tls::{self, TlsError};
use rustls::ClientConfig;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

/// 连接、握手、每一次读写最多等多久（Acme:250 的 `timeout=30`）
const TIMEOUT: Duration = Duration::from_secs(30);
/// 回应的内容最多要这么多（Acme:251）
const BODY_MAX: usize = 1 << 20;
/// 一行最长（http.client 的 _MAXLINE）
const MAX_LINE: usize = 65536;
/// 头最多几行（http.client 的 _MAXHEADERS）
const MAX_HEADERS: usize = 100;

/// 一个回应
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    /// 照先后的（名字, 值）
    headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    /// `headers.get(name)`：名字不分大小写，有几个时是头一个
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// 读回应没成：连接上的错，或者回应本身不对（名字照 http.client 的异常）
#[derive(Debug)]
enum Bad {
    Io(io::Error),
    Kind(&'static str),
}

impl From<io::Error> for Bad {
    fn from(e: io::Error) -> Bad {
        Bad::Io(e)
    }
}

/// 连接上的错叫什么。等到超时的在不同系统上是两种（EAGAIN、ETIMEDOUT）：都叫 TimedOut
fn kind_of(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => "TimedOut".to_string(),
        kind => format!("{kind:?}"),
    }
}

/// 握手没成叫什么：证书的问题带上是哪一种（rustls 的说法），握手时套接字出的错还原成那一种错的名字
fn tls_kind(e: &TlsError) -> String {
    match e {
        TlsError::Cert(why) => {
            let name: String = why.chars().take_while(char::is_ascii_alphanumeric).collect();
            format!("InvalidCertificate({})", name.strip_suffix("Context").unwrap_or(&name))
        }
        TlsError::Other(why) => {
            let errno = why.strip_prefix("[Errno ").and_then(|rest| rest.split(']').next()).and_then(|n| n.parse::<i32>().ok());
            errno.map_or_else(|| "TlsError".to_string(), |n| kind_of(&io::Error::from_raw_os_error(n)))
        }
    }
}

/// 一个网址拆出来的：连哪、Host 头写什么、请求行里写什么
#[derive(Debug, PartialEq)]
struct Target {
    tls: bool,
    /// 连的名字（也是核对证书的名字）：IPv6 的地址不带方括号
    host: String,
    port: u16,
    /// 网址里主机那一段原样（Host 头）
    netloc: String,
    /// 路径连同问号后面的
    selector: String,
}

/// 照 urllib（Request 的 _parse）和 http.client（_get_hostport）拆网址；这里用不了的是 None
fn target(url: &str) -> Option<Target> {
    let url = text::strip(url);
    // # 后面的不发（urllib 按最后一个 # 切）
    let url = url.rfind('#').map_or(Some(url), |i| url.get(..i))?;
    // 请求行、Host 头里不能有空白、控制字符、ASCII 以外的字
    if !url.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return None;
    }
    let (scheme, rest) = url.split_once(':')?;
    let tls = match scheme.to_ascii_lowercase().as_str() {
        "https" => true,
        "http" => false,
        _ => return None,
    };
    let rest = rest.strip_prefix("//")?;
    let (netloc, path) = rest.split_at_checked(rest.find(['/', '?']).unwrap_or(rest.len()))?;
    if netloc.contains('@') {
        return None; // 带用户名、密码的写法：urllib 也连不上（把它当成了主机名的一部分）
    }
    let selector = match path {
        "" => "/".to_string(),
        p if p.starts_with('/') => p.to_string(),
        p => format!("/{p}"),
    };
    // 最后一个冒号在最后一个 ] 的后面：它后面是端口（空的是默认的那个）
    let default = if tls { 443 } else { 80 };
    let (host, port) = match (netloc.rfind(':'), netloc.rfind(']')) {
        (Some(i), j) if j.is_none_or(|j| i > j) => {
            let (host, port) = (netloc.get(..i)?, netloc.get(i.saturating_add(1)..)?);
            (host, if port.is_empty() { default } else { u16::try_from(text::to_int(port)?).ok()? })
        }
        _ => (netloc, default),
    };
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    if host.is_empty() {
        return None;
    }
    Some(Target { tls, host: host.to_string(), port, netloc: netloc.to_string(), selector })
}

/// 状态行：版本和状态码
fn status_line<R: Read>(conn: &mut Lines<R>) -> Result<(String, u16), Bad> {
    let raw = conn.line(MAX_LINE.saturating_add(1))?;
    if raw.len() > MAX_LINE {
        return Err(Bad::Kind("LineTooLong"));
    }
    if raw.is_empty() {
        return Err(Bad::Kind("RemoteDisconnected")); // 连上了、一个字没回就关了
    }
    let line: String = raw.iter().map(|b| char::from(*b)).collect();
    let words = text::split_ws_n(&line, 2);
    let (Some(version), Some(status)) = (words.first(), words.get(1)) else {
        return Err(Bad::Kind("BadStatusLine"));
    };
    if !version.starts_with("HTTP/") {
        return Err(Bad::Kind("BadStatusLine"));
    }
    // int(status)：Python 的写法都认（+200、2_0_0）
    let status = text::to_int(status).filter(|n| (100..=999).contains(n)).and_then(|n| u16::try_from(n).ok());
    Ok((version.to_string(), status.ok_or(Bad::Kind("BadStatusLine"))?))
}

/// 头的那几行原样的字节：读到空行（或者读到头）为止
fn read_headers<R: Read>(conn: &mut Lines<R>) -> Result<Vec<u8>, Bad> {
    let (mut all, mut count) = (Vec::new(), 0usize);
    loop {
        let line = conn.line(MAX_LINE.saturating_add(1))?;
        if line.len() > MAX_LINE {
            return Err(Bad::Kind("LineTooLong"));
        }
        count = count.saturating_add(1);
        if count > MAX_HEADERS {
            return Err(Bad::Kind("TooManyHeaders"));
        }
        let end = matches!(line.as_slice(), b"\r\n" | b"\n" | b"");
        all.extend_from_slice(&line);
        if end {
            return Ok(all);
        }
    }
}

/// 这一行是不是“名字:”开头：名字是空白、冒号、控制字符以外的 ASCII（可以是空的）
fn name_colon(line: &str) -> bool {
    line.trim_start_matches(|c: char| matches!(u32::from(c), 0x21..=0x39 | 0x3b..=0x7e)).starts_with(':')
}

/// 把头拆成（名字, 值）。照 email.feedparser：按 latin-1 当成字；\r\n、\r、\n 都算换行；到第一个不像头的行为止
/// （空行也是）；空白开头的是续行，原样接在上一个的值后面；“From ”开头的、冒号开头的不要
fn parse_headers(raw: &[u8]) -> Vec<(String, String)> {
    let text: String = raw.iter().map(|b| char::from(*b)).collect();
    let mut lines: Vec<&str> = Vec::new();
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let cut = match rest.find(['\r', '\n']) {
            Some(i) if rest.get(i..).is_some_and(|r| r.starts_with("\r\n")) => i.saturating_add(2),
            Some(i) => i.saturating_add(1),
            None => rest.len(),
        };
        let Some((line, tail)) = rest.split_at_checked(cut) else { break };
        lines.push(line);
        rest = tail;
    }
    let continued = |line: &str| line.starts_with([' ', '\t']);
    let wanted = lines.iter().take_while(|l| l.starts_with("From ") || continued(l) || name_colon(l)).count();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut last: Option<(String, String)> = None;
    for (i, line) in lines.iter().take(wanted).enumerate() {
        if continued(line) {
            if let Some((_, value)) = last.as_mut() {
                value.push_str(line);
            }
            continue;
        }
        out.extend(last.take());
        if line.starts_with("From ") {
            if i > 0 && i.saturating_add(1) == wanted {
                break; // 最后一行：当成内容的开头，头到此为止
            }
            continue;
        }
        match line.split_once(':') {
            Some((name, value)) if !name.is_empty() => {
                last = Some((name.to_string(), value.trim_start_matches([' ', '\t']).to_string()));
            }
            _ => {}
        }
    }
    out.extend(last);
    for (_, value) in &mut out {
        value.truncate(value.trim_end_matches(['\r', '\n']).len());
    }
    out
}

/// chunked 的一块有多大：十六进制，分号后面的不看。看不懂是 None
fn chunk_size(line: &[u8]) -> Option<usize> {
    let digits = line.split(|b| *b == b';').next()?.trim_ascii();
    let digits = digits.strip_prefix(b"+").unwrap_or(digits);
    let digits = digits.strip_prefix(b"0x").or_else(|| digits.strip_prefix(b"0X")).unwrap_or(digits);
    if digits.is_empty() {
        return None;
    }
    // 大得离谱的当成“很大”：反正最多要 1 MiB
    digits.iter().try_fold(0usize, |n, b| Some(n.saturating_mul(16).saturating_add(char::from(*b).to_digit(16)? as usize)))
}

/// chunked 的内容，最多要 amt 个字节（http.client 的 _read_chunked）
fn read_chunked<R: Read>(conn: &mut Lines<R>, mut amt: usize) -> Result<Vec<u8>, Bad> {
    let incomplete = Bad::Kind("IncompleteRead");
    let mut out = Vec::new();
    let mut first = true;
    loop {
        // 上一块后面的换行：两个字节，不看是什么
        if !first && conn.take(2)?.len() < 2 {
            return Err(incomplete);
        }
        first = false;
        let line = conn.line(MAX_LINE.saturating_add(1))?;
        if line.len() > MAX_LINE {
            return Err(Bad::Kind("LineTooLong"));
        }
        let Some(size) = chunk_size(&line) else { return Err(incomplete) };
        if size == 0 {
            break;
        }
        let want = size.min(amt);
        let got = conn.take(want)?;
        if got.len() < want {
            return Err(incomplete);
        }
        out.extend_from_slice(&got);
        if amt <= size {
            return Ok(out); // 够了：后面的不读
        }
        amt = amt.saturating_sub(size);
    }
    // 最后那一块后面的头：读掉，到空行（或者读到头）为止
    for _ in 0..MAX_HEADERS {
        let line = conn.line(MAX_LINE.saturating_add(1))?;
        if line.len() > MAX_LINE {
            return Err(Bad::Kind("LineTooLong"));
        }
        if matches!(line.as_slice(), b"\r\n" | b"\n" | b"") {
            return Ok(out);
        }
    }
    Err(Bad::Kind("TooManyHeaders"))
}

/// 读一个回应（http.client 的 begin 和 read）。head：发的是 HEAD，回应没有内容
fn read_response<R: Read>(conn: &mut Lines<R>, head: bool) -> Result<Response, Bad> {
    let (mut version, mut status) = status_line(conn)?;
    // 100 Continue：连同它的头跳过，后面才是真的回应。最多跳这么多个（Python 没有上限）
    let mut skipped = 0usize;
    while status == 100 {
        skipped = skipped.saturating_add(1);
        if skipped > MAX_HEADERS {
            return Err(Bad::Kind("BadStatusLine"));
        }
        read_headers(conn)?;
        (version, status) = status_line(conn)?;
    }
    if !(version == "HTTP/1.0" || version == "HTTP/0.9" || version.starts_with("HTTP/1.")) {
        return Err(Bad::Kind("UnknownProtocol"));
    }
    let mut response = Response { status, headers: parse_headers(&read_headers(conn)?), body: Vec::new() };
    let chunked = response.header("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked"));
    response.body = if head {
        Vec::new()
    } else if chunked {
        read_chunked(conn, BODY_MAX)?
    } else if matches!(status, 204 | 304 | 100..=199) {
        Vec::new()
    } else {
        // Content-Length 看不懂、是负的：当成没有，读到对面关掉。没读够对面就关了的不算错（read(amt) 就是这样）
        let length = response.header("content-length").and_then(text::to_int).and_then(|n| usize::try_from(n).ok());
        conn.take(length.map_or(BODY_MAX, |n| n.min(BODY_MAX)))?
    };
    Ok(response)
}

/// 对面没好好关 TLS 就断了（没发 close_notify）：当成读到头。Python 的 ssl 默认也是这样
struct Ragged<S>(S);

impl<S: Read> Read for Ragged<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.0.read(buf) {
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(0),
            other => other,
        }
    }
}

/// 在连好的连接上发请求、读回应
fn talk<S: Read + Write>(mut stream: S, request: &[u8], head: bool) -> Result<Response, Bad> {
    stream.write_all(request)?;
    stream.flush()?;
    read_response(&mut Lines::new(Ragged(stream)), head)
}

/// 连上去：名字解析出来的地址一个个试，都不行时说最后一个的原因
fn connect(host: &str, port: u16, timeout: Duration) -> Result<TcpStream, String> {
    let addrs = (host, port).to_socket_addrs().map_err(|_| "UnknownHost".to_string())?;
    let mut why = "UnknownHost".to_string();
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(sock) => return Ok(sock),
            Err(e) => why = kind_of(&e),
        }
    }
    Err(why)
}

fn request_within(
    method: &str,
    url: &str,
    body: Option<&[u8]>,
    config: &Arc<ClientConfig>,
    timeout: Duration,
) -> Result<Response, String> {
    let Some(to) = target(url) else { return Err("InvalidUrl".to_string()) };
    // urllib 发的那几行头，先后也照它
    let mut request = format!("{method} {} HTTP/1.1\r\nAccept-Encoding: identity\r\n", to.selector);
    if let Some(body) = body {
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str(&format!("Host: {}\r\nUser-Agent: {AGENT}\r\n", to.netloc));
    request.push_str("Content-Type: application/jose+json\r\nConnection: close\r\n\r\n");
    let mut request = request.into_bytes();
    request.extend_from_slice(body.unwrap_or(&[]));

    let sock = connect(&to.host, to.port, timeout)?;
    let io = |e: io::Error| kind_of(&e);
    sock.set_read_timeout(Some(timeout)).map_err(io)?;
    sock.set_write_timeout(Some(timeout)).map_err(io)?;
    let _ = sock.set_nodelay(true);
    let head = method == "HEAD";
    let done = if to.tls {
        talk(tls::connect_with(sock, &to.host, config.clone()).map_err(|e| tls_kind(&e))?, &request, head)
    } else {
        talk(sock, &request, head)
    };
    done.map_err(|bad| match bad {
        Bad::Io(e) => kind_of(&e),
        Bad::Kind(kind) => kind.to_string(),
    })
}

/// 发一个请求、读回应。method 是 GET、HEAD、POST；body 是 POST 的内容；config 是 https 时照哪些根证书验对方。
/// 没成是一个名字（见这个文件开头）
pub fn request(method: &str, url: &str, body: Option<&[u8]>, config: &Arc<ClientConfig>) -> Result<Response, String> {
    request_within(method, url, body, config, TIMEOUT)
}

/// 测试里的假服务：一个连接读一个请求，回给定的字节，关掉
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
pub(super) mod testkit {
    use super::*;
    use crate::server::acme::lines::tests::as_text;
    use std::net::TcpListener;
    use std::sync::Mutex;

    /// 服务那头看到的一个请求
    #[derive(Clone, Debug)]
    pub struct Seen {
        /// 请求行和头，原样（连同结尾的空行）
        pub head: String,
        pub body: Vec<u8>,
    }

    impl Seen {
        /// 请求行（不带换行）
        pub fn line(&self) -> &str {
            self.head.split("\r\n").next().unwrap()
        }

        pub fn header(&self, name: &str) -> Option<&str> {
            let lines = self.head.split("\r\n").skip(1);
            lines.filter_map(|l| l.split_once(": ")).find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v)
        }
    }

    /// 从连接上读一个请求
    pub fn read_request<S: Read>(conn: S) -> Seen {
        let mut lines = Lines::new(conn);
        let mut head = String::new();
        loop {
            let line = lines.line(1 << 20).unwrap();
            head.push_str(&as_text(&line));
            if line == b"\r\n" || line.is_empty() {
                break;
            }
        }
        let mut seen = Seen { head, body: Vec::new() };
        let length: usize = seen.header("Content-Length").map_or(0, |n| n.parse().unwrap());
        seen.body = lines.take(length).unwrap();
        seen
    }

    pub struct Server {
        pub port: u16,
        /// 到现在收到的请求，照先后
        pub seen: Arc<Mutex<Vec<Seen>>>,
    }

    impl Server {
        /// 起一个：每来一个请求调一次 reply(第几个, 请求)，把它给的字节原样回过去
        pub fn start(reply: impl Fn(usize, &Seen) -> Vec<u8> + Send + 'static) -> Server {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let seen = Arc::new(Mutex::new(Vec::new()));
            let log = seen.clone();
            std::thread::spawn(move || {
                for (n, conn) in listener.incoming().enumerate() {
                    let Ok(mut conn) = conn else { break };
                    let request = read_request(conn.try_clone().unwrap());
                    let out = reply(n, &request);
                    log.lock().unwrap().push(request);
                    let _ = conn.write_all(&out);
                }
            });
            Server { port, seen }
        }

        pub fn url(&self, path: &str) -> String {
            format!("http://127.0.0.1:{}{path}", self.port)
        }

        pub fn requests(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }
    }

    /// 不验谁的客户端配置：只连 http 的测试用它占个位置
    pub fn no_roots() -> Arc<ClientConfig> {
        tls::client_config(rustls::RootCertStore::empty()).unwrap()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::testkit::{no_roots, read_request, Server};
    use super::*;
    use crate::py::fs::TempDir;
    use crate::py::hash::{sha256, to_hex};
    use crate::py::json;
    use crate::py::obj::V;
    use crate::server::acme::lines::tests::{as_text, from_parts, Drip};
    use crate::server::x509::{testkit as certs, San};
    use std::net::TcpListener;

    /// 对照数据：http.client 读这些回应读出了什么。这里读出来的要一样——状态码、每个头、内容；它读不了的这里也读不了，
    /// 原因对得上。一下子给、几个字节几个字节地给都一样
    #[test]
    fn reads_responses_like_http_client() {
        let doc = json::loads(include_str!("../../../tests/vectors/server_acme_http.json")).unwrap().as_dict().unwrap();
        let cases = doc.list("cases").unwrap().items();
        assert!(cases.len() >= 120, "{}", cases.len());
        let mut errors = std::collections::BTreeSet::new();
        for c in cases {
            let c = c.as_dict().unwrap();
            let (name, method) = (c.str("name").unwrap().to_string(), c.str("method").unwrap().to_string());
            let raw = from_parts(&c.list("response").unwrap());
            let want = c.dict("python").unwrap();
            for step in [usize::MAX, 5, 1] {
                if step == 1 && raw.len() > 3000 {
                    continue;
                }
                let got = read_response(&mut Lines::new(Drip { data: &raw, step, errors: vec![] }), method == "HEAD");
                let at = format!("{name}（一次给 {step} 个字节）");
                if let Some(error) = want.str("error") {
                    // Python 的异常 → 这里的名字。头太多在 Python 里是光秃秃的 HTTPException
                    let kind = match (&*error, &*want.str("text").unwrap()) {
                        ("HTTPException", "got more than 100 headers") => "TooManyHeaders".to_string(),
                        (other, _) => other.to_string(),
                    };
                    match got {
                        Err(Bad::Kind(got)) => assert_eq!(got, kind, "{at}"),
                        other => panic!("{at}：要的是 {kind}，得到 {other:?}"),
                    }
                    errors.insert(kind);
                    continue;
                }
                let got = got.unwrap_or_else(|e| panic!("{at}：{e:?}"));
                assert_eq!(i64::from(got.status), want.get("status").unwrap().as_int().unwrap(), "{at}");
                let pairs: Vec<V> = got.headers.iter().map(|(k, v)| V::List(crate::py::obj::str_list([k, v]))).collect();
                match want.list("headers") {
                    Some(headers) => {
                        let want = json::dumps(&V::List(headers), json::Fmt::new());
                        assert_eq!(json::dumps(&V::List(crate::py::obj::List::from_vec(pairs)), json::Fmt::new()), want, "{at}");
                    }
                    None => assert_eq!(pairs.len() as i64, want.get("headers_count").unwrap().as_int().unwrap(), "{at}"),
                }
                for (key, value) in want.dict("get").unwrap().items() {
                    assert_eq!(got.header(&key), value.as_str(), "{at}：{key}");
                }
                assert_eq!(got.body.len() as i64, want.get("body_len").unwrap().as_int().unwrap(), "{at}");
                assert_eq!(to_hex(&sha256(&got.body)), &*want.str("body_sha256").unwrap(), "{at}");
                assert!(as_text(&got.body).starts_with(&*want.str("body_head").unwrap()), "{at}");
            }
        }
        let errors: Vec<&str> = errors.iter().map(|e| e.as_str()).collect();
        assert_eq!(
            errors,
            ["BadStatusLine", "IncompleteRead", "LineTooLong", "RemoteDisconnected", "TooManyHeaders", "UnknownProtocol"]
        );
    }

    /// 网址怎么拆
    #[test]
    fn urls() {
        let t = |url: &str| target(url).map(|t| (t.tls, t.host, t.port, t.netloc, t.selector));
        let ok = |tls: bool, host: &str, port: u16, netloc: &str, selector: &str| {
            Some((tls, host.to_string(), port, netloc.to_string(), selector.to_string()))
        };
        assert_eq!(
            t("https://acme-v02.api.letsencrypt.org/directory"),
            ok(true, "acme-v02.api.letsencrypt.org", 443, "acme-v02.api.letsencrypt.org", "/directory")
        );
        assert_eq!(t("https://127.0.0.1:14000/dir"), ok(true, "127.0.0.1", 14000, "127.0.0.1:14000", "/dir"));
        assert_eq!(t("http://ca.example.com"), ok(false, "ca.example.com", 80, "ca.example.com", "/"));
        assert_eq!(t("HTTP://ca.example.com:/x?y=1&z=2"), ok(false, "ca.example.com", 80, "ca.example.com:", "/x?y=1&z=2"));
        assert_eq!(t("http://ca.example.com?q"), ok(false, "ca.example.com", 80, "ca.example.com", "/?q"));
        assert_eq!(t("  https://ca.example.com/a#frag \n"), ok(true, "ca.example.com", 443, "ca.example.com", "/a"));
        assert_eq!(t("https://[2001:db8::1]/a"), ok(true, "2001:db8::1", 443, "[2001:db8::1]", "/a"));
        assert_eq!(t("https://[2001:db8::1]:8443/a"), ok(true, "2001:db8::1", 8443, "[2001:db8::1]:8443", "/a"));
        assert_eq!(t("http://ca.example.com:+8080/"), ok(false, "ca.example.com", 8080, "ca.example.com:+8080", "/"));
        for bad in [
            "",
            "ca.example.com/dir",
            "ftp://ca.example.com/",
            "file:///etc/hosts",
            "https:/ca.example.com/",
            "https:///dir",
            "https://:443/",
            "https://ca.example.com:port/",
            "https://ca.example.com:70000/",
            "https://ca.example.com:-1/",
            "https://user:pw@ca.example.com/",
            "https://ca.example.com/a b",
            "https://ca.example.com/\u{e9}",
            "https://ca.exam\tple.com/",
            "a",
            "123",
        ] {
            assert_eq!(t(bad), None, "{bad:?}");
        }
    }

    fn reply(status: &str, headers: &str, body: &str) -> Vec<u8> {
        format!("HTTP/1.1 {status}\r\n{headers}\r\n{body}").into_bytes()
    }

    /// 对着真的连接：发出去的请求和 urllib 发的一样；内容的三种给法、HEAD、1 MiB 的上限
    #[test]
    fn talks_to_a_server() {
        let big = "abcdefgh".repeat(140_000);
        let chunky = big.clone();
        let server = Server::start(move |_, seen| match seen.line().split([' ', '?']).nth(1).unwrap() {
            "/length" => {
                reply("200 OK", "Content-Type: application/json\r\nReplay-Nonce: n1\r\nContent-Length: 9\r\n", "{\"a\": 1}\n")
            }
            "/chunked" => reply(
                "201 Created",
                "Transfer-Encoding: chunked\r\nLocation: http://example.com/acct/7\r\n",
                "4\r\nWiki\r\n5;x=y\r\npedia\r\n0\r\nT: 1\r\n\r\n",
            ),
            "/eof" => reply("400 Bad Request", "Content-Type: application/problem+json\r\n", "{\"detail\": \"no\"}"),
            "/head" => reply("200 OK", "Replay-Nonce: n2\r\nContent-Length: 500\r\n", ""),
            "/big-length" => reply("200 OK", &format!("Content-Length: {}\r\n", chunky.len()), &chunky),
            "/big-eof" => reply("200 OK", "", &chunky),
            "/big-chunked" => {
                reply("200 OK", "Transfer-Encoding: chunked\r\n", &format!("{:x}\r\n{chunky}\r\n0\r\n\r\n", chunky.len()))
            }
            "/nothing" => Vec::new(),
            "/garbage" => b"SSH-2.0-OpenSSH_9.9\r\n".to_vec(),
            other => panic!("{other}"),
        });
        let config = no_roots();
        let get = |method: &str, path: &str, body: Option<&[u8]>| request(method, &server.url(path), body, &config);

        let r = get("GET", "/length", None).unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"{\"a\": 1}\n"[..]));
        assert_eq!(
            (r.header("replay-nonce"), r.header("Content-Type"), r.header("Location")),
            (Some("n1"), Some("application/json"), None)
        );
        let r = get("POST", "/chunked?x=1", Some(b"{\"payload\": \"\"}")).unwrap();
        assert_eq!(
            (r.status, r.body.as_slice(), r.header("Location")),
            (201, &b"Wikipedia"[..], Some("http://example.com/acct/7"))
        );
        let r = get("POST", "/eof", Some(b"")).unwrap();
        assert_eq!((r.status, r.body.as_slice()), (400, &b"{\"detail\": \"no\"}"[..]));
        let r = get("HEAD", "/head", None).unwrap();
        assert_eq!((r.status, r.body.len(), r.header("Replay-Nonce")), (200, 0, Some("n2")));
        // 最多要 1 MiB
        for path in ["/big-length", "/big-eof", "/big-chunked"] {
            let r = get("GET", path, None).unwrap();
            assert_eq!((r.status, r.body.len()), (200, BODY_MAX), "{path}");
            assert!(r.body == big.as_bytes()[..BODY_MAX], "{path}");
        }
        assert_eq!(get("GET", "/nothing", None).unwrap_err(), "RemoteDisconnected");
        assert_eq!(get("GET", "/garbage", None).unwrap_err(), "BadStatusLine");

        // 服务那头看到的请求：一个字节不差
        let seen = server.requests();
        let port = server.port;
        assert_eq!(
            seen[0].head,
            format!("GET /length HTTP/1.1\r\nAccept-Encoding: identity\r\nHost: 127.0.0.1:{port}\r\nUser-Agent: cca-acme/1\r\nContent-Type: application/jose+json\r\nConnection: close\r\n\r\n")
        );
        assert_eq!(
            seen[1].head,
            format!("POST /chunked?x=1 HTTP/1.1\r\nAccept-Encoding: identity\r\nContent-Length: 15\r\nHost: 127.0.0.1:{port}\r\nUser-Agent: cca-acme/1\r\nContent-Type: application/jose+json\r\nConnection: close\r\n\r\n")
        );
        assert_eq!(seen[1].body, b"{\"payload\": \"\"}");
        assert_eq!((seen[2].header("Content-Length"), seen[2].body.len()), (Some("0"), 0), "空的内容也写长度");
        assert!(seen[3].head.starts_with("HEAD /head HTTP/1.1\r\nAccept-Encoding: identity\r\nHost: "));
    }

    /// 连不上的几种，各自的名字
    #[test]
    fn failures_have_names() {
        let config = no_roots();
        assert_eq!(request("GET", "ftp://example.com/", None, &config).unwrap_err(), "InvalidUrl");
        assert_eq!(request("GET", "", None, &config).unwrap_err(), "InvalidUrl");
        // 没人听的端口
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(request("GET", &format!("http://127.0.0.1:{port}/"), None, &config).unwrap_err(), "ConnectionRefused");
        // 连上了不回：等到点
        let server = Server::start(|_, _| {
            std::thread::sleep(Duration::from_millis(600));
            Vec::new()
        });
        let began = std::time::Instant::now();
        let got = request_within("GET", &server.url("/"), None, &config, Duration::from_millis(150));
        assert_eq!(got.unwrap_err(), "TimedOut");
        assert!(began.elapsed() < Duration::from_millis(550), "{:?}", began.elapsed());
        // 对面说的不是 TLS（一连上就回明文）
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let _ = conn.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n");
            let _ = conn.read(&mut [0u8; 4096]);
        });
        let got = request_within("GET", &format!("https://127.0.0.1:{port}/"), None, &config, Duration::from_secs(5));
        assert_eq!(got.unwrap_err(), "TlsError");
        // 名字
        assert_eq!(kind_of(&io::Error::from_raw_os_error(libc::ECONNRESET)), "ConnectionReset");
        assert_eq!(kind_of(&io::Error::from_raw_os_error(libc::EAGAIN)), "TimedOut");
        assert_eq!(kind_of(&io::Error::from_raw_os_error(libc::ETIMEDOUT)), "TimedOut");
        assert_eq!(tls_kind(&TlsError::Cert("UnknownIssuer".into())), "InvalidCertificate(UnknownIssuer)");
        assert_eq!(
            tls_kind(&TlsError::Cert("NotValidForNameContext { expected: x }".into())),
            "InvalidCertificate(NotValidForName)"
        );
        assert_eq!(tls_kind(&TlsError::Other(format!("[Errno {}] Connection reset by peer", libc::ECONNRESET))), "ConnectionReset");
        assert_eq!(tls_kind(&TlsError::Other("received corrupt message of type InvalidContentType".into())), "TlsError");
    }

    /// https：证书是信得过的机构签的、名字对得上才连得上；不然说得出是证书的哪种问题
    #[test]
    fn https_checks_the_certificate() {
        let d = TempDir::new().unwrap();
        let key = certs::new_key();
        let cert = certs::self_signed(&key, Some(&[San::Ip("127.0.0.1".into())]));
        let (cert_file, key_file) = (format!("{}/cert.pem", d.path), format!("{}/key.pem", d.path));
        std::fs::write(&cert_file, certs::pem("CERTIFICATE", &cert)).unwrap();
        std::fs::write(&key_file, certs::pem("PRIVATE KEY", &key)).unwrap();
        let server_config = crate::server::tls::load_pair(cert_file.as_ref(), key_file.as_ref()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { break };
                let tls = rustls::ServerConnection::new(server_config.clone()).unwrap();
                let mut stream = rustls::StreamOwned::new(tls, conn);
                // 握手没成（对面不信这张证书）时读不到请求：这个连接就算了
                let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read_request(&mut stream)));
                if let Ok(seen) = done {
                    let body = format!("you asked {}", seen.line());
                    let _ = stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes());
                    // 不发 close_notify 就关：客户端要当成正常读完
                }
            }
        });
        let mut roots = rustls::RootCertStore::empty();
        roots.add(rustls::pki_types::CertificateDer::from(cert.clone())).unwrap();
        let trusting = tls::client_config(roots).unwrap();
        let r = request("GET", &format!("https://127.0.0.1:{port}/dir"), None, &trusting).unwrap();
        assert_eq!((r.status, as_text(&r.body)), (200, "you asked GET /dir HTTP/1.1".to_string()));
        // 不信这个机构
        let got = request("GET", &format!("https://127.0.0.1:{port}/dir"), None, &no_roots()).unwrap_err();
        assert_eq!(got, "InvalidCertificate(UnknownIssuer)");
        // 证书不是给这个名字的
        let got = request("GET", &format!("https://localhost:{port}/dir"), None, &trusting).unwrap_err();
        assert_eq!(got, "InvalidCertificate(NotValidForName)");
    }
}
