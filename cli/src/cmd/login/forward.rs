//! login-forward（服务器上）：把贴回来的验证码转给本机正在等的登录回调。
//!
//! 原来用 `http.client` 发这一个 GET。这里照它实际的做法手写：请求的字节一样（请求行、`Host`、`Accept-Encoding: identity`，
//! 一次发出去），回应照它的规矩读（状态行、跳过 100、最多 100 个头、`Content-Length` / 分块、正文最多读 65536 字节），
//! 出错时说的话就是原来的 `str(e)`。不经代理，也不跟随跳转。

use super::sys::{NetErr, PySock};
use super::{ascii_encode_error, ascii_only, login_code_re, login_target, q_get};
use crate::cmd::arity;
use crate::err;
use crate::py::fail::os_error_text;
use crate::py::fs;
use crate::py::io;
use crate::py::obj::repr_str;
use crate::py::text;
use crate::py::url::{parse_qsl, quote, urlsplit};
use crate::py::{die, exit, Fail, R};
use std::ffi::{CStr, CString};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, TcpStream};
use std::os::fd::OwnedFd;
use std::time::Duration;

/// `http.client._MAXLINE`、`_MAXHEADERS`
const MAXLINE: usize = 65536;
const MAXHEADERS: usize = 100;

/// `sys.stdin.read()`（`-X utf8` 下是 surrogateescape）：不是 UTF-8 的字节一个字节一个记号。原来的记号是孤立的代理项，
/// 这里用 U+FFFD：往下只看它是不是可见的 ASCII、数它有几个，没有区别
fn stdin_text(raw: &[u8]) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    loop {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                out.push_str(s);
                return out;
            }
            Err(e) => {
                let (good, bad) = rest.split_at(e.valid_up_to());
                out.push_str(&String::from_utf8_lossy(good));
                let n = e.error_len().unwrap_or(bad.len());
                out.extend(std::iter::repeat_n('\u{fffd}', n));
                rest = &bad[n..];
            }
        }
    }
}

/// 服务器上：把贴回来的验证码转给本机正在等的登录回调。stdin 是用户贴的：Mac 上 cca login-page 给的 code#state，或者浏览器
/// 跳回 localhost 时地址栏里的整个网址。state 和这次的登录网址对不上就不转（Claude Code 收到对不上的 state 整个登录就失败了）。
/// 直接连本机的回调：不经代理，也不跟随跳转（Claude Code 回的是跳到成功页的 302，跟过去就直连了外网）。Codex 换好 token
/// 才回答，所以多等一会儿。退出码：0 转过去了；2 贴的不对（没转，可以重新贴）；1 转不过去，原因打到 stderr
pub fn cmd_login_forward(args: &[String]) -> R<()> {
    arity("cmd_login_forward", args, 1, 1)?;
    let tgt = match fs::read_text(&args[0]) {
        Ok(url) => login_target(text::strip(&url)),
        Err(e) if e.is_os() => None,
        Err(e) => return Err(e), // 不是 UTF-8：原来是没人接的 UnicodeDecodeError
    };
    let Some((host, port, path, state)) = tgt else {
        return die("这次的登录网址不对，没法转交验证码");
    };
    let input = stdin_text(&io::read_stdin_bytes());
    let text = text::strip(&input);
    let (mut code, mut got, mut err, mut why) = (None, None, None, None);
    if text.starts_with("http://") || text.starts_with("https://") {
        let q = urlsplit(text).map(|u| parse_qsl(&u.query, true)).unwrap_or_default();
        let get = |k: &str| q_get(&q, k).map(str::to_string);
        (code, got, err, why) = (get("code"), get("state"), get("error"), get("error_description"));
    } else if text.contains('#') {
        let (c, _, g) = text::partition(text, "#");
        (code, got) = (Some(c.to_string()), Some(g.to_string()));
    }
    if let Some(err) = err.filter(|e| !e.is_empty()) {
        if got.as_deref() == Some(state.as_str()) {
            let said = why.filter(|w| !w.is_empty()).unwrap_or(err);
            return die(format!("登录页说没有登录成功：{}", ascii_only(&said, Some('?'), 200)));
        }
    }
    let (code, got) = match (code, got) {
        (Some(code), Some(got)) if login_code_re(&code) && login_code_re(&got) => (code, got),
        _ => {
            err!("认不出验证码：粘贴 Mac 上 cca login-page 给的那一行（形如 xxxx#yyyy），或者浏览器跳回 localhost 时地址栏里的整个网址");
            return exit(2);
        }
    };
    if got != state {
        err!("这个验证码不是这一次登录的（state 对不上）：在 Mac 上运行上面这一次给出的命令，重新登录一遍");
        return exit(2);
    }
    let target = format!("{}?code={}&state={}", path, quote(&code, ""), quote(&got, ""));
    match http_get(&host, port, &target, 120.0) {
        Ok(st) if st >= 400 => die(format!("登录没收下这个验证码（HTTP {st}）")),
        Ok(_) => Ok(()),
        Err(HttpErr::Said(e)) => die(format!("连不上这台机器上等着的登录（{host}:{port}），它可能已经退出了：{e}")),
        Err(HttpErr::Fail(e)) => Err(e),
    }
}

/// 请求没成的两种：原来接住的（OSError、http.client.HTTPException，带着 `str(e)`），和没人接的别的异常
#[derive(Debug)]
enum HttpErr {
    Said(String),
    Fail(Fail),
}

impl From<NetErr> for HttpErr {
    fn from(e: NetErr) -> HttpErr {
        HttpErr::Said(e.text())
    }
}

/// `'got more than %d bytes when reading %s'`（http.client.LineTooLong）
fn line_too_long<T>(what: &str) -> Result<T, HttpErr> {
    Err(HttpErr::Said(format!("got more than {MAXLINE} bytes when reading {what}")))
}

/// `c.request('GET', target)` 发出去的那些字节：请求行、`Host`（端口不是 80 才带上）、`Accept-Encoding: identity`。
/// 网址里不能有控制字符和空格（InvalidURL），请求行要是 ASCII（不是的话是没人接的 UnicodeEncodeError）
fn request_bytes(host: &str, port: u16, target: &str) -> Result<String, HttpErr> {
    if let Some(bad) = target.chars().find(|c| *c <= ' ' || *c == '\u{7f}') {
        return Err(HttpErr::Said(format!(
            "URL can't contain control characters. {} (found at least {})",
            repr_str(target),
            repr_str(&bad.to_string())
        )));
    }
    let request = format!("GET {target} HTTP/1.1");
    if let Some(e) = ascii_encode_error(&request) {
        return Err(HttpErr::Fail(e));
    }
    let host_header = if port == 80 { host.to_string() } else { format!("{host}:{port}") };
    Ok(format!("{request}\r\nHost: {host_header}\r\nAccept-Encoding: identity\r\n\r\n"))
}

/// `c = HTTPConnection(host, port, timeout=timeout); c.request('GET', target); r = c.getresponse(); r.read(65536)`，
/// 返回 `r.status`
fn http_get(host: &str, port: u16, target: &str, timeout: f64) -> Result<i64, HttpErr> {
    // 请求有问题的在连接之前就不干了
    let message = request_bytes(host, port, target)?;
    let sock = create_connection(host, port, timeout).map_err(HttpErr::Said)?;
    sock.sendall(message.as_bytes())?;
    let mut r = Response { fp: Reader { sock, buf: Vec::new(), at: 0 }, status: 0, chunked: false, chunk_left: None, length: None };
    r.begin()?;
    r.read(65536)?;
    Ok(r.status)
}

/// `socket.getaddrinfo(host, port, 0, socket.SOCK_STREAM)` 给的地址，先后照 C 库给的。出错时是 `str(socket.gaierror)`
fn getaddrinfo(host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    let (Ok(node), Ok(service)) = (CString::new(host), CString::new(port.to_string())) else {
        return Err("embedded null character".into());
    };
    let mut out = Vec::new();
    // SAFETY: addrinfo 是纯数据，全 0 是合法的值；getaddrinfo 给的链表只在 freeaddrinfo 之前读，地址按它说的地址族和长度读
    unsafe {
        let mut hints: libc::addrinfo = std::mem::zeroed();
        hints.ai_socktype = libc::SOCK_STREAM;
        let mut res: *mut libc::addrinfo = std::ptr::null_mut();
        let rc = libc::getaddrinfo(node.as_ptr(), service.as_ptr(), &hints, &mut res);
        if rc == libc::EAI_SYSTEM {
            return Err(os_error_text(&std::io::Error::last_os_error()));
        }
        if rc != 0 {
            return Err(format!("[Errno {}] {}", rc, CStr::from_ptr(libc::gai_strerror(rc)).to_string_lossy()));
        }
        let mut p = res;
        while !p.is_null() {
            let ai = &*p;
            let len = ai.ai_addrlen as usize;
            if ai.ai_family == libc::AF_INET && !ai.ai_addr.is_null() && len >= std::mem::size_of::<libc::sockaddr_in>() {
                let a = &*ai.ai_addr.cast::<libc::sockaddr_in>();
                out.push(SocketAddr::V4(SocketAddrV4::new(
                    Ipv4Addr::from(u32::from_be(a.sin_addr.s_addr)),
                    u16::from_be(a.sin_port),
                )));
            } else if ai.ai_family == libc::AF_INET6 && !ai.ai_addr.is_null() && len >= std::mem::size_of::<libc::sockaddr_in6>() {
                let a = &*ai.ai_addr.cast::<libc::sockaddr_in6>();
                out.push(SocketAddr::V6(SocketAddrV6::new(
                    Ipv6Addr::from(a.sin6_addr.s6_addr),
                    u16::from_be(a.sin6_port),
                    a.sin6_flowinfo,
                    a.sin6_scope_id,
                )));
            }
            p = ai.ai_next;
        }
        libc::freeaddrinfo(res);
    }
    Ok(out)
}

/// `socket.create_connection((host, port), timeout)` 加上 `HTTPConnection.connect` 里的 TCP_NODELAY：各个地址挨个试，
/// 每个最多等 timeout 秒，连上一个就用它；都连不上时说的是最后一个的错。连上的套接字带着同样的时限
fn create_connection(host: &str, port: u16, timeout: f64) -> Result<PySock, String> {
    let mut last = "getaddrinfo returns an empty list".to_string();
    for addr in getaddrinfo(host, port)? {
        match TcpStream::connect_timeout(&addr, Duration::from_secs_f64(timeout)) {
            Ok(stream) => {
                if let Err(e) = stream.set_nodelay(true) {
                    if e.raw_os_error() != Some(libc::ENOPROTOOPT) {
                        return Err(os_error_text(&e));
                    }
                }
                return PySock::new(OwnedFd::from(stream), timeout).map_err(|e| os_error_text(&e));
            }
            // 等到点的（不是系统说的超时）：Python 的说法是 timed out
            Err(e) if e.raw_os_error().is_none() && e.kind() == std::io::ErrorKind::TimedOut => last = "timed out".into(),
            Err(e) => last = os_error_text(&e),
        }
    }
    Err(last)
}

/// `sock.makefile('rb')`：带缓冲地读
struct Reader {
    sock: PySock,
    buf: Vec<u8>,
    /// buf 里读到哪了
    at: usize,
}

impl Reader {
    /// 再收一段进缓冲；对方关了是 false
    fn fill(&mut self) -> Result<bool, NetErr> {
        if self.at > 0 {
            self.buf.drain(..self.at);
            self.at = 0;
        }
        let mut chunk = [0u8; 8192];
        let n = self.sock.recv(&mut chunk)?;
        self.buf.extend_from_slice(&chunk[..n]);
        Ok(n > 0)
    }

    fn take(&mut self, n: usize) -> Vec<u8> {
        let out = self.buf[self.at..self.at + n].to_vec();
        self.at += n;
        out
    }

    /// `fp.readline(limit)`：到换行为止（带着换行），最多 limit 个字节；读到头就是剩下的
    fn readline(&mut self, limit: usize) -> Result<Vec<u8>, NetErr> {
        let mut seen = 0; // 缓冲里已经找过换行的那一截
        loop {
            let have = self.buf.len() - self.at;
            if let Some(i) = self.buf[self.at + seen..self.at + have.min(limit)].iter().position(|b| *b == b'\n') {
                return Ok(self.take(seen + i + 1));
            }
            if have >= limit {
                return Ok(self.take(limit));
            }
            seen = have;
            if !self.fill()? {
                return Ok(self.take(seen));
            }
        }
    }

    /// `fp.read(n)`：读满 n 个字节，读到头就是剩下的
    fn read(&mut self, n: usize) -> Result<Vec<u8>, NetErr> {
        while self.buf.len() - self.at < n {
            if !self.fill()? {
                return Ok(self.take(self.buf.len() - self.at));
            }
        }
        Ok(self.take(n))
    }

    /// `fp.read(-1)`：读到头
    fn read_all(&mut self) -> Result<Vec<u8>, NetErr> {
        while self.fill()? {}
        Ok(self.take(self.buf.len() - self.at))
    }
}

/// iso-8859-1：一个字节一个字符
fn latin1(raw: &[u8]) -> String {
    raw.iter().map(|b| *b as char).collect()
}

/// `http.client.HTTPResponse` 里 `begin()` 和 `read(amt)` 用到的那部分。不管连接接着用不用（will_close）：这里只发这一个请求
struct Response {
    fp: Reader,
    status: i64,
    chunked: bool,
    /// 分块时这一块还剩多少；0 是刚读完一块（结尾的 CRLF 还没读），None 是还没读到下一块的大小
    chunk_left: Option<i64>,
    /// 正文还剩多少字节；None 是不知道
    length: Option<i64>,
}

/// 读分块的正文时出的错：IncompleteRead（外面会换成只说读到了多少的那一句），或者别的
enum ChunkErr {
    Incomplete,
    Other(HttpErr),
}

impl From<NetErr> for ChunkErr {
    fn from(e: NetErr) -> ChunkErr {
        ChunkErr::Other(e.into())
    }
}

impl Response {
    /// `_read_status()`：(版本, 状态码)
    fn read_status(&mut self) -> Result<(String, i64), HttpErr> {
        let raw = self.fp.readline(MAXLINE + 1)?;
        if raw.len() > MAXLINE {
            return line_too_long("status line");
        }
        if raw.is_empty() {
            // 多半是对方没回答就关了连接
            return Err(HttpErr::Said("Remote end closed connection without response".into()));
        }
        let line = latin1(&raw);
        // 版本 状态码 说明；说明可以没有。不够两段时版本当作空的，下面一样认成坏的状态行（BadStatusLine，说的就是这一行）
        let parts = text::split_ws_n(&line, 2);
        let bad = || Err(HttpErr::Said(line.clone()));
        if parts.len() < 2 || !parts[0].starts_with("HTTP/") {
            return bad();
        }
        // 状态码是三位数
        match text::to_int(parts[1]) {
            Some(status) if (100..=999).contains(&status) => Ok((parts[0].to_string(), status)),
            _ => bad(),
        }
    }

    /// `_read_headers(fp)`：到空行（或者读到头）为止的各行
    fn read_headers(&mut self) -> Result<Vec<Vec<u8>>, HttpErr> {
        let mut headers: Vec<Vec<u8>> = Vec::new();
        loop {
            let line = self.fp.readline(MAXLINE + 1)?;
            if line.len() > MAXLINE {
                return line_too_long("header line");
            }
            let last = matches!(line.as_slice(), b"\r\n" | b"\n" | b"");
            headers.push(line);
            if headers.len() > MAXHEADERS {
                return Err(HttpErr::Said(format!("got more than {MAXHEADERS} headers")));
            }
            if last {
                return Ok(headers);
            }
        }
    }

    /// `begin()`：读到正文之前
    fn begin(&mut self) -> Result<(), HttpErr> {
        // 一直读到不是 100 的回应
        let (version, status) = loop {
            let (version, status) = self.read_status()?;
            if status != 100 {
                break (version, status);
            }
            self.read_headers()?; // 跳过 100 那一份的头
        };
        self.status = status;
        // HTTP/0.9、HTTP/1.x 都认；别的是 UnknownProtocol，说的就是这个版本
        if !version.starts_with("HTTP/1.") && version != "HTTP/0.9" {
            return Err(HttpErr::Said(version));
        }
        let headers = parse_headers(&self.read_headers()?.concat());
        let get = |name: &str| headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str());
        // 是不是分块传的
        self.chunked = get("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked"));
        self.chunk_left = None;
        // 有没有 Content-Length（分块时不看它）；不是数、负数都当没有。大得放不下的和没有一样：反正最多读 65536
        self.length = match get("content-length") {
            Some(v) if !v.is_empty() && !self.chunked => text::to_int(v).filter(|n| *n >= 0),
            _ => None,
        };
        // 这几种回应没有正文
        if status == 204 || status == 304 || (100..200).contains(&status) {
            self.length = Some(0);
        }
        Ok(())
    }

    /// `read(amt)`：正文最多读 amt 个字节（读到的不要，只是照样读走）
    fn read(&mut self, amt: i64) -> Result<(), HttpErr> {
        if self.chunked {
            return self.read_chunked(amt);
        }
        let amt = self.length.map_or(amt, |left| amt.min(left));
        self.fp.read(amt as usize)?;
        Ok(())
    }

    /// `_safe_read(amt)`：该有 amt 个字节，没有就是 IncompleteRead。返回读到了多少
    fn safe_read(&mut self, amt: i64) -> Result<usize, ChunkErr> {
        if amt < -1 {
            // fp.read(负数)：没人接的 ValueError
            return Err(ChunkErr::Other(HttpErr::Fail(Fail::Value("read length must be non-negative or -1".into()))));
        }
        if amt == -1 {
            return Ok(self.fp.read_all()?.len()); // fp.read(-1) 是读到头，多少都不嫌少
        }
        let data = self.fp.read(amt as usize)?;
        if (data.len() as i64) < amt {
            return Err(ChunkErr::Incomplete);
        }
        Ok(data.len())
    }

    /// `_get_chunk_left()`：这一块还剩多少，读完了一块就读下一块的大小；None 是最后一块也读完了
    fn get_chunk_left(&mut self) -> Result<Option<i64>, ChunkErr> {
        let mut chunk_left = self.chunk_left;
        if chunk_left.is_none_or(|n| n == 0) {
            if chunk_left.is_some() {
                self.safe_read(2)?; // 这一块结尾的 CRLF
            }
            let line = self.fp.readline(MAXLINE + 1)?;
            if line.len() > MAXLINE {
                return Err(ChunkErr::Other(HttpErr::Said(format!("got more than {MAXLINE} bytes when reading chunk size"))));
            }
            // 分号后面是这一块的附加说明，不要；大小不是十六进制数时没法接着读了
            let size = line.split(|b| *b == b';').next().unwrap_or(&[]);
            let n = int16(size).ok_or(ChunkErr::Incomplete)?;
            chunk_left = if n == 0 {
                // 最后一块：后面跟着的（本不该有的）尾部各行读走、不要，到空行或者读到头为止
                loop {
                    let line = self.fp.readline(MAXLINE + 1)?;
                    if line.len() > MAXLINE {
                        return Err(ChunkErr::Other(HttpErr::Said(format!(
                            "got more than {MAXLINE} bytes when reading trailer line"
                        ))));
                    }
                    if matches!(line.as_slice(), b"\r\n" | b"\n" | b"") {
                        break;
                    }
                }
                None
            } else {
                Some(n)
            };
            self.chunk_left = chunk_left;
        }
        Ok(chunk_left)
    }

    /// `_read_chunked(amt)`
    fn read_chunked(&mut self, mut amt: i64) -> Result<(), HttpErr> {
        let mut got = 0usize; // 读完整的各段一共多少字节
        let mut step = |r: &mut Response, got: &mut usize| -> Result<bool, ChunkErr> {
            let Some(chunk_left) = r.get_chunk_left()? else { return Ok(false) };
            if amt <= chunk_left {
                *got += r.safe_read(amt)?;
                r.chunk_left = Some(chunk_left - amt);
                return Ok(false);
            }
            *got += r.safe_read(chunk_left)?;
            amt -= chunk_left;
            r.chunk_left = Some(0);
            Ok(true)
        };
        loop {
            match step(self, &mut got) {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                // 原来在这里换成 IncompleteRead(已经读到的)，它的 str() 只说读到了多少
                Err(ChunkErr::Incomplete) => return Err(HttpErr::Said(format!("IncompleteRead({got} bytes read)"))),
                Err(ChunkErr::Other(e)) => return Err(e),
            }
        }
    }
}

/// `int(字节串, 16)`：两头的空白（只认 ASCII 的）、正负号、`0x` 前缀、数字之间的单个下划线都认；别的是 ValueError（None）。
/// 大得放不下的收成 i64 的两头：用它的地方只拿它和 65536 以内的数比
fn int16(raw: &[u8]) -> Option<i64> {
    let is_space = |b: &u8| matches!(*b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
    let start = raw.iter().position(|b| !is_space(b))?;
    let end = raw.iter().rposition(|b| !is_space(b))? + 1;
    let mut s = &raw[start..end];
    let neg = s.first() == Some(&b'-');
    if matches!(s.first(), Some(b'+' | b'-')) {
        s = &s[1..];
    }
    if s.len() >= 2 && s[0] == b'0' && matches!(s[1], b'x' | b'X') {
        s = &s[2..];
        // 前缀后面可以紧跟一个下划线
        if s.first() == Some(&b'_') {
            s = &s[1..];
        }
    }
    let mut n: i64 = 0;
    let mut prev_digit = false;
    for &b in s {
        if b == b'_' {
            if !prev_digit {
                return None;
            }
            prev_digit = false;
            continue;
        }
        let d = (b as char).to_digit(16)? as i64;
        n = n.saturating_mul(16).saturating_add(d);
        prev_digit = true;
    }
    if !prev_digit {
        return None;
    }
    Some(if neg { n.saturating_neg() } else { n })
}

/// `email.parser.Parser().parsestr(头的那一段)` 认出来的头 [(名字, 值)]（http.client 就是这么读回应的头的）。和按 CRLF 一行
/// 一个头不全一样：单独的 CR 也断行；遇到第一行不像头的（不是「名字:」、不是续行、不是 `From `）就到此为止，后面的都不算；
/// 续行接在上一个头的值后面；值去掉开头的空白和结尾的 CR、LF
fn parse_headers(block: &[u8]) -> Vec<(String, String)> {
    // 断行：\n、\r\n、单独的 \r（行尾留在行里）
    let mut lines: Vec<&[u8]> = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < block.len() {
        let end = match block[i] {
            b'\n' => i + 1,
            b'\r' if block.get(i + 1) == Some(&b'\n') => i + 2,
            b'\r' => i + 1,
            _ => {
                i += 1;
                continue;
            }
        };
        lines.push(&block[start..end]);
        start = end;
        i = end;
    }
    if start < block.len() {
        lines.push(&block[start..]);
    }
    // 像头的各行（headerRE：`From `、「可见的 ASCII 里除了冒号的若干个字符 + 冒号」、空格或制表符开头）
    let name_len = |l: &[u8]| l.iter().take_while(|b| matches!(**b, 0x21..=0x39 | 0x3b..=0x7e)).count();
    let like_header =
        |l: &[u8]| l.starts_with(b"From ") || l.get(name_len(l)) == Some(&b':') || matches!(l.first(), Some(b' ' | b'\t'));
    let lines: Vec<&[u8]> = lines.into_iter().take_while(|l| like_header(l)).collect();
    /// 攒着的那个头（冒号在哪、它的各行）记成 (名字, 值)
    fn flush(last: &mut Option<(usize, Vec<&[u8]>)>, out: &mut Vec<(String, String)>) {
        if let Some((colon, parts)) = last.take() {
            let value = latin1(&[&parts[0][colon + 1..], &parts[1..].concat()].concat());
            let value = value.trim_start_matches([' ', '\t', '\r', '\n']).trim_end_matches(['\r', '\n']);
            out.push((latin1(&parts[0][..colon]), value.to_string()));
        }
    }
    let mut out: Vec<(String, String)> = Vec::new();
    let mut last: Option<(usize, Vec<&[u8]>)> = None;
    for (lineno, line) in lines.iter().copied().enumerate() {
        // 续行；前面还没有头的续行不算数
        if matches!(line[0], b' ' | b'\t') {
            if let Some((_, parts)) = &mut last {
                parts.push(line);
            }
            continue;
        }
        flush(&mut last, &mut out);
        // 信封上的 From 行：在最前面的、夹在中间的都跳过，在最后的到此为止
        if line.starts_with(b"From ") {
            if lineno == 0 || lineno != lines.len() - 1 {
                continue;
            }
            return out;
        }
        // 冒号打头：没有名字，跳过
        match line.iter().position(|b| *b == b':') {
            Some(0) | None => continue,
            Some(colon) => last = Some((colon, vec![line])),
        }
    }
    flush(&mut last, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn stdin_bytes_decode_one_mark_per_bad_byte() {
        // Python 的 surrogateescape：每个不是 UTF-8 的字节一个代理项（这里是一个 U+FFFD）
        let marks = |raw: &[u8]| stdin_text(raw).chars().map(|c| if c == '\u{fffd}' { '?' } else { c }).collect::<String>();
        assert_eq!(marks(b"abc#def"), "abc#def");
        assert_eq!(marks("中文#x".as_bytes()), "中文#x");
        assert_eq!(marks(b"ab\xe4\xb8"), "ab??");
        assert_eq!(marks(b"\xf0\x9f\x98x"), "???x");
        assert_eq!(marks(b"\xc0\xaf"), "??");
        assert_eq!(marks(b"\xed\xa0\x80"), "???");
        assert_eq!(marks(b"\xf4\x90\x80\x80"), "????");
        assert_eq!(marks(b"a\xffb\xfe"), "a?b?");
        assert_eq!(marks(b"\xe4\xb8\xad\xe6"), "中?");
        assert_eq!(marks(b"\xf8\x88\x80\x80\x80"), "?????");
        assert_eq!(marks(b""), "");
    }

    #[test]
    fn hex_sizes_like_python_int() {
        // 右边是 Python 的 int(b, 16)（ValueError 是 None）
        for (raw, want) in [
            (&b"5\r\n"[..], Some(5)),
            (b"1a\r\n", Some(26)),
            (b" 1A \r\n", Some(26)),
            (b"", None),
            (b"\r\n", None),
            (b"0x1f", Some(31)),
            (b"0X1F", Some(31)),
            (b"0x", None),
            (b"1_0", Some(16)),
            (b"_10", None),
            (b"1__0", None),
            (b"10_", None),
            (b"+a", Some(10)),
            (b"-5", Some(-5)),
            (b"- 5", None),
            (b"--5", None),
            (b"0", Some(0)),
            (b"00", Some(0)),
            (b"-0", Some(0)),
            (b"g", None),
            (b"5 5", None),
            (b"\t5\x0b\x0c", Some(5)),
            (b"5\x00", None),
            (b"\xa05", None),
            (b"0x_1f", Some(31)),
            (b"0_x1f", None),
            (b"5\n", Some(5)),
            (b"-1", Some(-1)),
            (b"7fffffffffffffff", Some(i64::MAX)),
            (b"0b1", Some(177)),
            (b"0o7", None),
            (b"x5", None),
            (b"0x-5", None),
            (b"+0x5", Some(5)),
            (b"-0x5", Some(-5)),
            (b"0x 5", None),
            (b"1\x1c", None),
            (b"\x1c1", None),
            (b"\x855", None),
        ] {
            assert_eq!(int16(raw), want, "{raw:?}");
        }
        // 放不下的：Python 是大整数，这里收到两头
        assert_eq!(int16(b"8000000000000000"), Some(i64::MAX));
        assert_eq!(int16(b"ffffffffffffffffffff"), Some(i64::MAX));
        assert!(int16(b"-ffffffffffffffffffff").unwrap() < -1);
    }

    #[test]
    fn headers_like_the_email_parser() {
        // 右边是 Python 的 http.client.parse_headers(io.BytesIO(…)).items()
        let parse = |raw: &[u8]| parse_headers(raw);
        let pairs = |want: &[(&str, &str)]| want.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
        assert_eq!(parse(b"Content-Length: 5\r\nX-Y:  a b \r\n\r\n"), pairs(&[("Content-Length", "5"), ("X-Y", "a b ")]));
        assert_eq!(parse(b"A: 1\nB:2\n\n"), pairs(&[("A", "1"), ("B", "2")]));
        assert_eq!(parse(b"A: 1\rB: 2\r\n\r\n"), pairs(&[("A", "1"), ("B", "2")]));
        assert_eq!(parse(b"A: 1\r\n more\r\n\tand more\r\nB: 2\r\n\r\n"), pairs(&[("A", "1\r\n more\r\n\tand more"), ("B", "2")]));
        assert_eq!(parse(b" lead\r\nA: 1\r\n\r\n"), pairs(&[("A", "1")]));
        assert_eq!(parse(b"A: 1\r\nBad Header\r\nB: 2\r\n\r\n"), pairs(&[("A", "1")]));
        assert_eq!(parse(b"A: 1\r\n: no name\r\nB: 2\r\n\r\n"), pairs(&[("A", "1"), ("B", "2")]));
        assert_eq!(parse(b"From x\r\nA: 1\r\nFrom y\r\nB: 2\r\nFrom z\r\n"), pairs(&[("A", "1"), ("B", "2")]));
        assert_eq!(parse(b"A:\r\nB: \xe4\xb8\xad\r\n\r\n"), pairs(&[("A", ""), ("B", "\u{e4}\u{b8}\u{ad}")]));
        assert_eq!(parse(b"A: 1"), pairs(&[("A", "1")]));
        assert_eq!(parse(b"\r\nA: 1\r\n"), pairs(&[]));
        assert_eq!(parse(b""), pairs(&[]));
        assert_eq!(parse(b"A: x:y: z\r\n\r\n"), pairs(&[("A", "x:y: z")]));
        assert_eq!(parse(b"A: 1\x0bB: 2\r\n\r\n"), pairs(&[("A", "1\u{b}B: 2")]));
        assert_eq!(parse(b"From only\r\n"), pairs(&[]));
        assert_eq!(parse(b"A: 1\r\nFrom last\r\n"), pairs(&[("A", "1")]));
        assert_eq!(parse(b"A: 1\r\n\r\nB: 2\r\n"), pairs(&[("A", "1")]));
        assert_eq!(parse(b"A : 1\r\nB: 2\r\n\r\n"), pairs(&[]));
        assert_eq!(parse(b"A: 1\r\n \r\nB: 2\r\n\r\n"), pairs(&[("A", "1\r\n "), ("B", "2")]));
        assert_eq!(parse(b"A: 1\r"), pairs(&[("A", "1")]));
        assert_eq!(parse(b"A: 1\r\rB: 2\r\n"), pairs(&[("A", "1")]));
        assert_eq!(parse(b"Content-Length:\t 7 \t\r\n\r\n"), pairs(&[("Content-Length", "7 \t")]));
        assert_eq!(parse(b"a\x80b: 1\r\nB: 2\r\n\r\n"), pairs(&[]));
    }

    /// 本机起一个只接一次的监听：收到请求头以后回答 reply；hold 时连接先不关，等测试那边放手（返回的发送端被丢掉）
    fn serve_until(reply: &'static [u8], hold: bool) -> (u16, std::thread::JoinHandle<Vec<u8>>, std::sync::mpsc::Sender<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (release, held) = std::sync::mpsc::channel::<()>();
        let t = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                match conn.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => got.extend_from_slice(&buf[..n]),
                }
            }
            let _ = conn.write_all(reply);
            if hold {
                let _ = held.recv_timeout(std::time::Duration::from_secs(30));
            }
            got
        });
        (port, t, release)
    }

    fn serve(reply: &'static [u8]) -> (u16, std::thread::JoinHandle<Vec<u8>>) {
        let (port, t, _) = serve_until(reply, false);
        (port, t)
    }

    #[test]
    fn requests_are_what_http_client_sends() {
        // 右边是 Python 的 http.client 交给 sendall 的字节（套接字换成只记内容的假的，没有真的连出去）
        let req = |host: &str, port: u16, target: &str| request_bytes(host, port, target).unwrap();
        assert_eq!(
            req("localhost", 80, "/cb?code=x&state=y"),
            "GET /cb?code=x&state=y HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: identity\r\n\r\n"
        );
        assert_eq!(req("127.0.0.1", 80, "/"), "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept-Encoding: identity\r\n\r\n");
        assert_eq!(
            req("localhost", 1455, "/auth/callback?code=a%2Fb&state=s"),
            "GET /auth/callback?code=a%2Fb&state=s HTTP/1.1\r\nHost: localhost:1455\r\nAccept-Encoding: identity\r\n\r\n"
        );
        assert_eq!(
            req("127.0.0.1", 8, "/?code=x&state=y"),
            "GET /?code=x&state=y HTTP/1.1\r\nHost: 127.0.0.1:8\r\nAccept-Encoding: identity\r\n\r\n"
        );
        assert_eq!(req("localhost", 443, "/x"), "GET /x HTTP/1.1\r\nHost: localhost:443\r\nAccept-Encoding: identity\r\n\r\n");
        assert_eq!(
            req("localhost", 65535, "/it's~ok!*()?code=x"),
            "GET /it's~ok!*()?code=x HTTP/1.1\r\nHost: localhost:65535\r\nAccept-Encoding: identity\r\n\r\n"
        );
    }

    #[test]
    fn request_bytes_and_plain_answers() {
        // 发出去的字节是 http.client 实际发的那些（对着 Python 抓的）
        let (port, t) = serve(b"HTTP/1.1 302 Found\r\nLocation: https://x.example.com/ok\r\nContent-Length: 0\r\n\r\n");
        assert_eq!(http_get("127.0.0.1", port, "/callback?code=c%20o%2Bd%2Fe&state=st%231", 5.0).unwrap(), 302);
        let want = format!(
            "GET /callback?code=c%20o%2Bd%2Fe&state=st%231 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept-Encoding: identity\r\n\r\n"
        );
        assert_eq!(String::from_utf8(t.join().unwrap()).unwrap(), want);
        let (port, t) = serve(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n");
        assert_eq!(http_get("localhost", port, "/", 5.0).unwrap(), 200);
        assert!(String::from_utf8(t.join().unwrap()).unwrap().contains(&format!("\r\nHost: localhost:{port}\r\n")));
    }

    #[test]
    fn errors_say_what_python_says() {
        let said = |r: Result<i64, HttpErr>| match r {
            Err(HttpErr::Said(s)) => s,
            other => panic!("{other:?}"),
        };
        // 右边都是 Python 里 str(e) 的原话
        let (port, t) = serve(b"");
        assert_eq!(said(http_get("127.0.0.1", port, "/cb", 5.0)), "Remote end closed connection without response");
        t.join().unwrap();
        let (port, t) = serve(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n");
        assert_eq!(said(http_get("127.0.0.1", port, "/cb", 5.0)), "IncompleteRead(5 bytes read)");
        t.join().unwrap();
        // 不回答：等到点。原来的时限是 120 秒，经子命令测等不起，在这里用短的走一遍
        let (port, t, release) = serve_until(b"", true);
        let t0 = crate::py::time::monotonic();
        assert_eq!(said(http_get("127.0.0.1", port, "/cb", 0.4)), "timed out");
        assert!((0.3..20.0).contains(&(crate::py::time::monotonic() - t0)));
        drop(release);
        t.join().unwrap();
        // 头来了、正文不来也不关：也是等到点
        let (port, t, release) = serve_until(b"HTTP/1.1 200 OK\r\n\r\npartial", true);
        assert_eq!(said(http_get("127.0.0.1", port, "/cb", 0.4)), "timed out");
        drop(release);
        t.join().unwrap();
        // 没人听的端口
        let dead = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(said(http_get("127.0.0.1", dead, "/cb", 5.0)), format!("[Errno {}] Connection refused", libc::ECONNREFUSED));
        // 网址里有空格、控制字符：连接之前就不干了
        assert_eq!(
            said(http_get("127.0.0.1", dead, "/a b?code=x&state=y", 5.0)),
            "URL can't contain control characters. '/a b?code=x&state=y' (found at least ' ')"
        );
        assert_eq!(
            said(http_get("127.0.0.1", dead, "/a\u{7f}b?code=x", 5.0)),
            "URL can't contain control characters. '/a\\x7fb?code=x' (found at least '\\x7f')"
        );
        assert_eq!(
            said(http_get("127.0.0.1", dead, "/q'\"x y", 5.0)),
            "URL can't contain control characters. '/q\\'\"x y' (found at least ' ')"
        );
        match http_get("127.0.0.1", dead, "/中?code=x", 5.0) {
            Err(HttpErr::Fail(Fail::Value(m))) => {
                assert_eq!(m, "'ascii' codec can't encode character '\\u4e2d' in position 5: ordinal not in range(128)")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn localhost_resolves() {
        let addrs = getaddrinfo("localhost", 1455).unwrap();
        assert!(!addrs.is_empty() && addrs.iter().all(|a| a.port() == 1455 && a.ip().is_loopback()), "{addrs:?}");
        assert_eq!(getaddrinfo("127.0.0.1", 80).unwrap(), ["127.0.0.1:80".parse().unwrap()]);
    }
}
