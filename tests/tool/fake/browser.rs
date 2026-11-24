//! browser state code：扮演 Mac 上账号的 Chrome（cca login-page 的用例）。等 open.log 里出现这个 state 的登录网址
//! （cca 在账号的浏览器里打开了它），再像登录完的浏览器一样跳回网址里 redirect_uri 的回调：先来个 favicon、一个 state 不对的
//! （都不该算数），再带着 code 跳回来。打印每次的 HTTP 状态和最后那一页。原来的 fake_browser.py（不走代理，像 urllib）。

use super::claude::arg;
use super::*;
use std::net::ToSocketAddrs;

pub fn run(a: &[String]) -> R<i32> {
    let t = cca_t()?;
    let (state, code) = (arg(a, 0)?, arg(a, 1)?);
    let re = regex::Regex::new(&format!(r"\[(https://[^\]]*state={}[^\]]*)\]", regex::escape(state)))
        .map_err(|e| Fail(e.to_string()))?;
    let p = join(&[&t, "open.log"]);
    let mut found = None;
    for _ in 0..400 {
        if exists(&p) {
            if let Some(c) = re.captures(&read_text(&p)?) {
                found = Some(c[1].to_string());
                break;
            }
        }
        sleep(0.05);
    }
    let Some(url) = found else {
        print("never opened");
        return Ok(1);
    };
    let q = parse_qsl(&urlsplit(&url)?.query);
    let redirect = qs_get(&q, "redirect_uri").ok_or_else(|| Fail("KeyError: 'redirect_uri'".into()))?;
    let r = urlsplit(redirect)?;
    let port = r.port()?.ok_or_else(|| Fail("TypeError: %d format: a real number is required, not NoneType".into()))?;
    let base = format!("http://localhost:{port}");
    print(&format!("favicon {}", get_url(&format!("{base}/favicon.ico"))?.0));
    print(&format!("wrong {}", get_url(&format!("{base}{}?code=x&state=wrong", r.path))?.0));
    let (st, page) = get_url(&format!("{base}{}?code={code}&state={state}", r.path))?;
    print(&format!("callback {st}"));
    print(&page);
    Ok(0)
}

/// op.open(u, timeout=10) 的结果：2xx 是 (状态, 页面)；跳转照着跳（最多 10 次，只跳 http 的）；别的状态是 (状态, "")；
/// 连不上、超时、对方没回答就断开是 (0, 出错的说法)
fn get_url(url: &str) -> R<(u16, String)> {
    let mut url = url.to_string();
    for _ in 0..=10 {
        let u = urlsplit(&url)?;
        if u.scheme != "http" {
            return Ok((0, format!("<urlopen error unknown url type: {}>", u.scheme)));
        }
        let port = u.port()?.unwrap_or(80);
        let mut sel = if u.path.is_empty() { "/".to_string() } else { u.path.clone() };
        if !u.query.is_empty() {
            sel = format!("{sel}?{}", u.query);
        }
        let host = u.hostname();
        let resp = match fetch(&host, port, &u.netloc, &sel) {
            Ok(r) => r,
            Err(Net::Connect(e)) => return Ok((0, format!("<urlopen error {}>", errno_text(&e)))),
            Err(Net::Io(e)) => return Ok((0, errno_text(&e))),
            Err(Net::NoAnswer) => return Ok((0, "Remote end closed connection without response".into())),
            Err(Net::Bad(line)) => return fail(format!("http.client.BadStatusLine: {}", repr_str(&line))),
        };
        match resp.status {
            200..=299 => return Ok((resp.status, String::from_utf8_lossy(&resp.body).into_owned())),
            301 | 302 | 303 | 307 | 308 => match resp.location {
                Some(loc) => url = urljoin(&url, &loc),
                None => return Ok((resp.status, String::new())),
            },
            s => return Ok((s, String::new())),
        }
    }
    Ok((0, "HTTP Error: The HTTP server returned a redirect error that would lead to an infinite loop.".into()))
}

/// OSError 的 str()：[Errno 61] Connection refused
fn errno_text(e: &io::Error) -> String {
    let msg = e.to_string();
    let msg = msg.split(" (os error").next().unwrap_or(&msg).to_string();
    match e.raw_os_error() {
        Some(n) => format!("[Errno {n}] {msg}"),
        None if e.kind() == io::ErrorKind::TimedOut || e.kind() == io::ErrorKind::WouldBlock => "timed out".into(),
        None => msg,
    }
}

enum Net {
    Connect(io::Error),
    Io(io::Error),
    NoAnswer,
    Bad(String),
}

struct Resp {
    status: u16,
    location: Option<String>,
    body: Vec<u8>,
}

/// 一次 GET（HTTP/1.1，Connection: close，像 urllib 那样带 Host 和 Accept-Encoding: identity）
fn fetch(host: &str, port: u16, netloc: &str, sel: &str) -> Result<Resp, Net> {
    let timeout = Duration::from_secs(10);
    let addrs: Vec<_> = (host, port).to_socket_addrs().map_err(Net::Connect)?.collect();
    let mut last = io::Error::new(io::ErrorKind::NotFound, "getaddrinfo returns an empty list");
    let mut conn = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(s) => {
                conn = Some(s);
                break;
            }
            Err(e) => last = e,
        }
    }
    let mut s = conn.ok_or(Net::Connect(last))?;
    s.set_read_timeout(Some(timeout)).map_err(Net::Io)?;
    s.set_write_timeout(Some(timeout)).map_err(Net::Io)?;
    let host_hdr = netloc.rsplit_once('@').map_or(netloc, |(_, h)| h);
    let req = format!(
        "GET {sel} HTTP/1.1\r\nHost: {host_hdr}\r\nAccept-Encoding: identity\r\nUser-Agent: cca-test\r\nConnection: close\r\n\r\n"
    );
    s.write_all(req.as_bytes()).map_err(Net::Io)?;
    // 头：读到空行
    let mut data = Vec::new();
    let mut buf = [0u8; 8192];
    let head_end = loop {
        if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        let n = s.read(&mut buf).map_err(Net::Io)?;
        if n == 0 {
            if data.is_empty() {
                return Err(Net::NoAnswer);
            }
            return Err(Net::Bad(String::from_utf8_lossy(&data).into_owned()));
        }
        data.extend_from_slice(&buf[..n]);
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default().to_string();
    let mut parts = status_line.splitn(3, ' ');
    let (ver, code) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
    let status = match (ver.starts_with("HTTP/"), code.parse::<u16>()) {
        (true, Ok(c)) if (100..1000).contains(&c) => c,
        _ => return Err(Net::Bad(status_line)),
    };
    let (mut length, mut chunked, mut location) = (None, false, None);
    for l in lines {
        let Some((k, v)) = l.split_once(':') else { continue };
        let v = v.trim();
        match k.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = v.parse::<usize>().ok(),
            "transfer-encoding" => chunked = v.eq_ignore_ascii_case("chunked"),
            "location" => location = Some(v.to_string()),
            _ => {}
        }
    }
    let mut body = data[head_end..].to_vec();
    if chunked {
        let mut raw = body;
        let mut out = Vec::new();
        let mut pos = 0;
        loop {
            // 一块的长度那一行
            let line_end = loop {
                if let Some(i) = raw[pos..].windows(2).position(|w| w == b"\r\n") {
                    break pos + i;
                }
                if !read_more(&mut s, &mut raw).map_err(Net::Io)? {
                    return Ok(Resp { status, location, body: out });
                }
            };
            let size_txt = String::from_utf8_lossy(&raw[pos..line_end]).into_owned();
            let size = usize::from_str_radix(size_txt.split(';').next().unwrap_or_default().trim(), 16).unwrap_or(0);
            pos = line_end + 2;
            if size == 0 {
                break;
            }
            while raw.len() < pos + size + 2 {
                if !read_more(&mut s, &mut raw).map_err(Net::Io)? {
                    break;
                }
            }
            let end = (pos + size).min(raw.len());
            out.extend_from_slice(&raw[pos..end]);
            pos = (pos + size + 2).min(raw.len());
        }
        return Ok(Resp { status, location, body: out });
    }
    match length {
        Some(n) => {
            while body.len() < n {
                if !read_more(&mut s, &mut body).map_err(Net::Io)? {
                    break;
                }
            }
            body.truncate(n);
        }
        None => while read_more(&mut s, &mut body).map_err(Net::Io)? {},
    }
    Ok(Resp { status, location, body })
}

fn read_more(s: &mut TcpStream, into: &mut Vec<u8>) -> io::Result<bool> {
    let mut b = [0u8; 8192];
    let n = s.read(&mut b)?;
    into.extend_from_slice(&b[..n]);
    Ok(n > 0)
}

/// urljoin(base, 跳转到的)：只管跳转里会出现的几种写法
fn urljoin(base: &str, loc: &str) -> String {
    if loc.contains("://") {
        return loc.to_string();
    }
    let Ok(b) = urlsplit(base) else { return loc.to_string() };
    if let Some(rest) = loc.strip_prefix("//") {
        return format!("{}://{rest}", b.scheme);
    }
    if loc.starts_with('/') {
        return format!("{}://{}{loc}", b.scheme, b.netloc);
    }
    let dir = b.path.rfind('/').map_or("/", |i| &b.path[..=i]);
    format!("{}://{}{dir}{loc}", b.scheme, b.netloc)
}
