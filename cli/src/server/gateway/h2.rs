//! 连推送服务（APNs）用的那一点 HTTP/2（RFC 9113）客户端。推送网关一种环境一条连接，请求一个接一个地发（通知不多，用不着同时
//! 开好几个流），所以这里只做这些：
//!
//! - 开场：TLS 要说好 h2（ALPN），然后是 preface 和我们的 SETTINGS——不要服务器推送；HPACK 的动态表是 0：对面回的头只能写原文，
//!   不能拿表里的编号指代（我们只认 `:status`，用不着记表）
//! - 请求：HEADERS（一帧放不下时接 CONTINUATION）、DATA。自己说的头用 HPACK 最简单的写法：`:method`、`:scheme` 用静态表的编号，
//!   别的一律「不进表的原文」、不压缩（RFC 7541 6.2.2）
//! - 等回的时候顺手处理对面别的帧：SETTINGS 回 ACK、照它调窗口和一帧最长多少，PING 回 ACK，WINDOW_UPDATE 加窗口，收到的 DATA
//!   马上还回窗口（WINDOW_UPDATE），GOAWAY 记下（不再开新的流；我们这个流对面没处理的话是 `Fail::Retry`：换一条连接重发不会
//!   重复），RST_STREAM 的 REFUSED_STREAM 也是 `Retry`；PUSH_PROMISE（说了不要）、帧比说好的长是协议错误；不认识的帧不理
//! - 回的头只认出 `:status`（`hpack::status`）：它一定是第一个头（伪头在前，回复只有这一个伪头），值可能是哈夫曼压缩过的数字
//!   （只认得数字的那几个码：`hpack::digits`）
//!
//! 下面那一层是个 trait（`Wire`）：平常是 rustls 的客户端连接，单元测试里是一对 Unix 套接字（明文的 h2，对面由测试一帧帧地演）。

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, StreamOwned};
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 连接开头客户端先说的那一串（RFC 9113 3.4）
pub const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

// 帧的类型（RFC 9113 6）
pub const DATA: u8 = 0x0;
pub const HEADERS: u8 = 0x1;
pub const RST_STREAM: u8 = 0x3;
pub const SETTINGS: u8 = 0x4;
pub const PUSH_PROMISE: u8 = 0x5;
pub const PING: u8 = 0x6;
pub const GOAWAY: u8 = 0x7;
pub const WINDOW_UPDATE: u8 = 0x8;
pub const CONTINUATION: u8 = 0x9;

// 标志
pub const END_STREAM: u8 = 0x1;
pub const ACK: u8 = 0x1;
pub const END_HEADERS: u8 = 0x4;
pub const PADDED: u8 = 0x8;
pub const PRIORITY: u8 = 0x20;

// SETTINGS 的几项
pub const HEADER_TABLE_SIZE: u16 = 0x1;
pub const ENABLE_PUSH: u16 = 0x2;
pub const INITIAL_WINDOW_SIZE: u16 = 0x4;
pub const MAX_FRAME_SIZE: u16 = 0x5;

/// RST_STREAM、GOAWAY 的错误码：对面没处理这个流
pub const REFUSED_STREAM: u32 = 0x7;

/// 窗口、一帧最长的默认值（RFC 9113 6.5.2）。我们没改自己收的一帧最长多少：对面发来的不能比它长
const DEFAULT_WINDOW: i64 = 65_535;
const DEFAULT_FRAME: usize = 16_384;
/// 流的编号、窗口最大到这里
const LAST: u32 = 0x7fff_ffff;
/// 回的正文最多留这么多（推送服务回的是一小段 JSON）
const BODY_MAX: usize = 64 * 1024;

/// 一帧
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub kind: u8,
    pub flags: u8,
    pub stream: u32,
    pub payload: Vec<u8>,
}

/// 一帧的字节：长度（3 个字节）、类型、标志、流的编号（4 个字节，最高位是保留的 0）、内容。内容不超过一帧最长（调用方切好）
pub fn frame(kind: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let len = u32::try_from(payload.len()).unwrap_or(u32::MAX).to_be_bytes();
    let mut out = Vec::with_capacity(payload.len().saturating_add(9));
    out.extend_from_slice(len.get(1..).unwrap_or_default());
    out.push(kind);
    out.push(flags);
    out.extend_from_slice(&(stream & LAST).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// SETTINGS 里的一项
pub fn setting(key: u16, value: u32) -> Vec<u8> {
    let mut out = key.to_be_bytes().to_vec();
    out.extend_from_slice(&value.to_be_bytes());
    out
}

/// 内容开头 4 个字节的那个数（最高位不要）
fn word(payload: &[u8]) -> Option<u32> {
    let &[a, b, c, d] = payload.get(..4)? else { return None };
    Some(u32::from_be_bytes([a, b, c, d]) & LAST)
}

/// 去掉填充（带着 PADDED 的 DATA、HEADERS）
fn unpad(f: &Frame) -> Result<&[u8], Fail> {
    if f.flags & PADDED == 0 {
        return Ok(&f.payload);
    }
    let bad = Fail::Protocol("填充的长度不对");
    let Some((&pad, rest)) = f.payload.split_first() else { return Err(bad) };
    rest.len().checked_sub(usize::from(pad)).and_then(|n| rest.get(..n)).ok_or(bad)
}

/// 回的：状态码（认不出来是 None）和正文
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub status: Option<u16>,
    pub body: Vec<u8>,
}

/// 没拿到回的
#[derive(Debug)]
pub enum Fail {
    /// 读写出错、断了、等过了头：这条连接不能再用。请求可能已经到了对面
    Io(io::Error),
    /// 对面说它没处理这个流（GOAWAY 的编号比它小、RST_STREAM 的 REFUSED_STREAM），或者这条连接不能再开新的流：换一条重发不会重复
    Retry,
    /// 对面不照协议来：这条连接不能再用
    Protocol(&'static str),
    /// 对面把这个流重置了（别的错误码）
    Reset(u32),
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fail::Io(e) => write!(f, "{e}"),
            Fail::Retry => f.write_str("对面没处理这个请求"),
            Fail::Protocol(why) => f.write_str(why),
            Fail::Reset(code) => write!(f, "对面重置了这个请求（错误码 {code}）"),
        }
    }
}

/// 下面那一层：读写，和每次读写最多等多久
pub trait Wire: Read + Write {
    fn set_wait(&mut self, wait: Duration) -> io::Result<()>;
}

impl Wire for StreamOwned<ClientConnection, TcpStream> {
    fn set_wait(&mut self, wait: Duration) -> io::Result<()> {
        self.sock.set_read_timeout(Some(wait))?;
        self.sock.set_write_timeout(Some(wait))
    }
}

#[cfg(test)]
impl Wire for std::os::unix::net::UnixStream {
    fn set_wait(&mut self, wait: Duration) -> io::Result<()> {
        self.set_read_timeout(Some(wait))?;
        self.set_write_timeout(Some(wait))
    }
}

/// 到 deadline 还有多久；已经过了是 Err（等过了头）
fn left(deadline: Instant) -> Result<Duration, Fail> {
    let d = deadline.saturating_duration_since(Instant::now());
    if d.is_zero() {
        return Err(Fail::Io(io::Error::new(io::ErrorKind::TimedOut, "等过了头")));
    }
    Ok(d.max(Duration::from_millis(1)))
}

/// 推送服务的 TLS 连接
pub type Tls = StreamOwned<ClientConnection, TcpStream>;

/// 连上 host:port、TLS 握手（照 config 验证书，要说好 h2）、说开场白。wait：连上、握手各最多等多久
pub fn connect(host: &str, port: u16, config: &Arc<ClientConfig>, wait: Duration) -> io::Result<Conn<Tls>> {
    let mut last = io::Error::other("查不到地址");
    let mut sock = None;
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, wait) {
            Ok(s) => {
                sock = Some(s);
                break;
            }
            Err(e) => last = e,
        }
    }
    let sock = sock.ok_or(last)?;
    sock.set_nodelay(true)?;
    let name = ServerName::try_from(host.to_string()).map_err(|_| io::Error::other(format!("{host} 不是主机名")))?;
    let tls = ClientConnection::new(Arc::clone(config), name).map_err(io::Error::other)?;
    let mut io = StreamOwned::new(tls, sock);
    io.set_wait(wait)?;
    while io.conn.is_handshaking() {
        io.conn.complete_io(&mut io.sock)?;
    }
    if io.conn.alpn_protocol() != Some(b"h2") {
        return Err(io::Error::other("对面不说 HTTP/2（ALPN 没说好 h2）"));
    }
    Conn::start(io, wait)
}

/// 一条连接
pub struct Conn<W: Wire> {
    io: W,
    /// 下一个流的编号（客户端开的是单数）
    next: u32,
    /// 连接一级的发送窗口（对面给的）
    window: i64,
    /// 对面 SETTINGS 里说的：新开的流的发送窗口、一帧最长
    stream_init: i64,
    max_frame: usize,
    /// 对面说过 GOAWAY：它最后会处理的那个流
    goaway: Option<u32>,
    /// 读进来、还没成帧的
    inbuf: Vec<u8>,
    /// 上一次用完的时候（太久没用的换一条：对面多半早就关了）
    pub idle_since: Instant,
}

/// 正在等的那个流
#[derive(Default)]
struct Pending {
    id: u32,
    /// 这个流的发送窗口
    window: i64,
    status: Option<u16>,
    /// 头还没收齐（HEADERS 没带 END_HEADERS，后面是 CONTINUATION）：收到的那几段
    block: Option<Vec<u8>>,
    /// 头收齐以后这个流就完了（HEADERS 带着 END_STREAM）
    end_after_block: bool,
    /// 收过一次最终的头了（再来的是 trailers：不管）
    headed: bool,
    body: Vec<u8>,
    ended: bool,
}

impl<W: Wire> Conn<W> {
    /// 在已经连上的 io 上说开场白：preface 和 SETTINGS（不要服务器推送、动态表是 0）。不等对面的 SETTINGS（发请求时顺手处理）
    pub fn start(mut io: W, wait: Duration) -> io::Result<Conn<W>> {
        let mut hello = PREFACE.to_vec();
        hello.extend(frame(SETTINGS, 0, 0, &[setting(ENABLE_PUSH, 0), setting(HEADER_TABLE_SIZE, 0)].concat()));
        io.set_wait(wait.max(Duration::from_millis(1)))?;
        io.write_all(&hello)?;
        io.flush()?;
        Ok(Conn {
            io,
            next: 1,
            window: DEFAULT_WINDOW,
            stream_init: DEFAULT_WINDOW,
            max_frame: DEFAULT_FRAME,
            goaway: None,
            inbuf: Vec::new(),
            idle_since: Instant::now(),
        })
    }

    /// 还能不能开新的流：对面没说 GOAWAY、编号没用完
    pub fn open(&self) -> bool {
        self.goaway.is_none() && self.next <= LAST
    }

    fn send(&mut self, bytes: &[u8], deadline: Instant) -> Result<(), Fail> {
        self.io.set_wait(left(deadline)?).map_err(Fail::Io)?;
        self.io.write_all(bytes).and_then(|()| self.io.flush()).map_err(Fail::Io)
    }

    /// 缓冲里有一整帧就交出来
    fn take_frame(&mut self) -> Result<Option<Frame>, Fail> {
        let Some(&[a, b, c, kind, flags, s0, s1, s2, s3]) = self.inbuf.get(..9) else { return Ok(None) };
        let len = usize::from(a) << 16 | usize::from(b) << 8 | usize::from(c);
        if len > DEFAULT_FRAME {
            return Err(Fail::Protocol("对面发来的帧比说好的长"));
        }
        let Some(payload) = self.inbuf.get(9..9 + len).map(<[u8]>::to_vec) else { return Ok(None) };
        self.inbuf.drain(..9 + len);
        Ok(Some(Frame { kind, flags, stream: u32::from_be_bytes([s0, s1, s2, s3]) & LAST, payload }))
    }

    fn read_frame(&mut self, deadline: Instant) -> Result<Frame, Fail> {
        let mut buf = [0u8; 16 * 1024];
        loop {
            if let Some(f) = self.take_frame()? {
                return Ok(f);
            }
            self.io.set_wait(left(deadline)?).map_err(Fail::Io)?;
            match self.io.read(&mut buf) {
                Ok(0) => return Err(Fail::Io(io::ErrorKind::UnexpectedEof.into())),
                Ok(n) => self.inbuf.extend_from_slice(buf.get(..n).unwrap_or_default()),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(Fail::Io(e)),
            }
        }
    }

    /// 发一个请求（head 照先后，伪头在前），等它回完。wait：从现在起最多等多久
    pub fn request(&mut self, head: &[(&str, &str)], body: &[u8], wait: Duration) -> Result<Reply, Fail> {
        let deadline = Instant::now() + wait;
        if !self.open() {
            return Err(Fail::Retry);
        }
        let id = self.next;
        self.next = self.next.saturating_add(2);
        let mut p = Pending { id, window: self.stream_init, ..Pending::default() };
        // 头：一帧放不下时接 CONTINUATION；没有正文的话 HEADERS 带着 END_STREAM
        let block = hpack::encode(head);
        let parts: Vec<&[u8]> = block.chunks(self.max_frame).collect();
        let mut out = Vec::new();
        for (i, part) in parts.iter().enumerate() {
            let mut flags = if i + 1 == parts.len() { END_HEADERS } else { 0 };
            if i == 0 && body.is_empty() {
                flags |= END_STREAM;
            }
            out.extend(frame(if i == 0 { HEADERS } else { CONTINUATION }, flags, id, part));
        }
        if out.is_empty() {
            return Err(Fail::Protocol("请求没有头"));
        }
        self.send(&out, deadline)?;
        // 正文：两级窗口都有余地才发，没有就先读对面的帧（等它的 WINDOW_UPDATE）
        let mut rest = body;
        while !rest.is_empty() && !p.ended {
            let room = self.window.min(p.window).min(self.max_frame as i64);
            if room <= 0 {
                let f = self.read_frame(deadline)?;
                self.on_frame(f, &mut p, deadline)?;
                continue;
            }
            let (now, later) = rest.split_at(rest.len().min(room as usize));
            self.send(&frame(DATA, if later.is_empty() { END_STREAM } else { 0 }, id, now), deadline)?;
            self.window -= now.len() as i64;
            p.window -= now.len() as i64;
            rest = later;
        }
        while !p.ended {
            let f = self.read_frame(deadline)?;
            self.on_frame(f, &mut p, deadline)?;
        }
        self.idle_since = Instant::now();
        Ok(Reply { status: p.status, body: p.body })
    }

    /// 等回的时候来的一帧
    fn on_frame(&mut self, f: Frame, p: &mut Pending, deadline: Instant) -> Result<(), Fail> {
        // 头没收齐时只能来这个流的 CONTINUATION（RFC 9113 6.10）
        if p.block.is_some() && !(f.kind == CONTINUATION && f.stream == p.id) {
            return Err(Fail::Protocol("头没收齐就来了别的帧"));
        }
        match f.kind {
            SETTINGS if f.stream == 0 => {
                if f.flags & ACK != 0 {
                    return Ok(());
                }
                let (entries, rest) = f.payload.as_chunks::<6>();
                if !rest.is_empty() {
                    return Err(Fail::Protocol("SETTINGS 的长度不对"));
                }
                for &[k0, k1, v0, v1, v2, v3] in entries {
                    let (key, value) = (u16::from_be_bytes([k0, k1]), u32::from_be_bytes([v0, v1, v2, v3]));
                    match key {
                        INITIAL_WINDOW_SIZE if value > LAST => return Err(Fail::Protocol("窗口太大")),
                        INITIAL_WINDOW_SIZE => {
                            // 改了的是所有流的：开着的这个跟着差那么多（RFC 9113 6.9.2）
                            p.window += i64::from(value) - self.stream_init;
                            self.stream_init = i64::from(value);
                        }
                        MAX_FRAME_SIZE if !(16_384..=16_777_215).contains(&value) => {
                            return Err(Fail::Protocol("一帧最长不对"));
                        }
                        MAX_FRAME_SIZE => self.max_frame = value as usize,
                        _ => {}
                    }
                }
                self.send(&frame(SETTINGS, ACK, 0, &[]), deadline)
            }
            PING if f.stream == 0 => match (f.flags & ACK != 0, f.payload.len()) {
                (true, _) => Ok(()),
                (false, 8) => self.send(&frame(PING, ACK, 0, &f.payload), deadline),
                (false, _) => Err(Fail::Protocol("PING 的长度不对")),
            },
            GOAWAY if f.stream == 0 => {
                let last = word(&f.payload).ok_or(Fail::Protocol("GOAWAY 太短"))?;
                self.goaway = Some(last);
                if last < p.id {
                    return Err(Fail::Retry);
                }
                Ok(())
            }
            WINDOW_UPDATE => {
                let inc = i64::from(word(&f.payload).ok_or(Fail::Protocol("WINDOW_UPDATE 太短"))?);
                if f.stream == 0 {
                    self.window += inc;
                } else if f.stream == p.id {
                    p.window += inc;
                }
                Ok(())
            }
            RST_STREAM if f.stream == p.id => match word(&f.payload) {
                Some(REFUSED_STREAM) => Err(Fail::Retry),
                Some(code) => Err(Fail::Reset(code)),
                None => Err(Fail::Protocol("RST_STREAM 太短")),
            },
            PUSH_PROMISE => Err(Fail::Protocol("说了不要服务器推送")),
            HEADERS | CONTINUATION if f.stream == p.id => {
                let fragment = if f.kind == HEADERS {
                    let rest = unpad(&f)?;
                    p.end_after_block = f.flags & END_STREAM != 0;
                    if f.flags & PRIORITY != 0 {
                        rest.get(5..).ok_or(Fail::Protocol("HEADERS 太短"))?
                    } else {
                        rest
                    }
                } else if p.block.is_some() {
                    &f.payload
                } else {
                    return Err(Fail::Protocol("前面没有 HEADERS 的 CONTINUATION"));
                };
                let mut block = p.block.take().unwrap_or_default();
                block.extend_from_slice(fragment);
                if f.flags & END_HEADERS == 0 {
                    p.block = Some(block);
                    return Ok(());
                }
                if !p.headed {
                    p.status = hpack::status(&block);
                    // 1xx 是中间的回复：后面还有真的那一个
                    p.headed = !p.status.is_some_and(|s| (100..200).contains(&s));
                }
                if p.end_after_block {
                    p.ended = true;
                }
                Ok(())
            }
            DATA => {
                // 收到的都马上还回窗口（推送服务回的很少，不用攒）：连接一级的；这个流还没完的话流一级的也还
                let used = u32::try_from(f.payload.len()).unwrap_or(0);
                if used > 0 {
                    self.send(&frame(WINDOW_UPDATE, 0, 0, &used.to_be_bytes()), deadline)?;
                }
                if f.stream == p.id {
                    let data = unpad(&f)?;
                    let room = BODY_MAX.saturating_sub(p.body.len()).min(data.len());
                    p.body.extend_from_slice(data.get(..room).unwrap_or_default());
                    if f.flags & END_STREAM != 0 {
                        p.ended = true;
                    } else if used > 0 {
                        self.send(&frame(WINDOW_UPDATE, 0, p.id, &used.to_be_bytes()), deadline)?;
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// HPACK（RFC 7541）里用到的那一点
pub mod hpack {
    /// 静态表里我们说的头用得到的名字（RFC 7541 附录 A）
    const NAMES: [(&str, usize); 5] =
        [(":authority", 1), (":path", 4), ("authorization", 23), ("content-length", 28), ("content-type", 31)];

    /// 一串头：`:method` POST / GET、`:scheme` https 用静态表的编号，别的是「不进表的原文」（名字在静态表里的用它的编号），不压缩
    pub fn encode(head: &[(&str, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        for &(name, value) in head {
            match (name, value) {
                (":method", "GET") => out.push(0x82),
                (":method", "POST") => out.push(0x83),
                (":scheme", "https") => out.push(0x87),
                _ => {
                    match NAMES.iter().find(|(n, _)| *n == name) {
                        Some(&(_, index)) => int(&mut out, 0x00, 4, index),
                        None => {
                            out.push(0x00);
                            string(&mut out, name.as_bytes());
                        }
                    }
                    string(&mut out, value.as_bytes());
                }
            }
        }
        out
    }

    /// 带前缀的整数（RFC 7541 5.1）：first 是第一个字节前缀以外的那几位
    pub fn int(out: &mut Vec<u8>, first: u8, prefix: u32, value: usize) {
        let max = (1usize << prefix) - 1;
        if value < max {
            out.push(first | value as u8);
            return;
        }
        out.push(first | max as u8);
        let mut rest = value - max;
        while rest >= 0x80 {
            out.push((rest & 0x7f) as u8 | 0x80);
            rest >>= 7;
        }
        out.push(rest as u8);
    }

    /// 不压缩的字符串（RFC 7541 5.2）
    fn string(out: &mut Vec<u8>, s: &[u8]) {
        int(out, 0x00, 7, s.len());
        out.extend_from_slice(s);
    }

    /// 读一个带前缀的整数：(值, 下一个字节在哪)
    pub fn read_int(b: &[u8], at: usize, prefix: u32) -> Option<(usize, usize)> {
        let max = (1usize << prefix) - 1;
        let first = usize::from(*b.get(at)?) & max;
        if first < max {
            return Some((first, at + 1));
        }
        let (mut value, mut shift, mut i) = (max, 0u32, at + 1);
        loop {
            let byte = *b.get(i)?;
            i += 1;
            value = value.checked_add(usize::from(byte & 0x7f) << shift)?;
            if byte & 0x80 == 0 {
                return Some((value, i));
            }
            shift += 7;
            if shift > 28 {
                return None;
            }
        }
    }

    /// 读一个字符串：(内容, 下一个字节在哪)。哈夫曼压缩过的只认得数字（`digits`）
    fn read_string(b: &[u8], at: usize) -> Option<(Vec<u8>, usize)> {
        let huffman = b.get(at)? & 0x80 != 0;
        let (len, at) = read_int(b, at, 7)?;
        let end = at.checked_add(len)?;
        let raw = b.get(at..end)?;
        let value = if huffman { digits(raw)?.into_bytes() } else { raw.to_vec() };
        Some((value, end))
    }

    /// 回的头里的 `:status`：开头的动态表大小的更新跳过，第一个头要是它——静态表的 8～14 号（200、204、206、304、400、404、500），
    /// 或者原文（进不进表的三种写法都行；名字是 8～14 号里的、或者原文 `:status`），值是三位数。别的样子是 None
    pub fn status(block: &[u8]) -> Option<u16> {
        let mut at = 0;
        while block.get(at)? & 0xe0 == 0x20 {
            at = read_int(block, at, 5)?.1;
        }
        let first = *block.get(at)?;
        if first & 0x80 != 0 {
            return match read_int(block, at, 7)?.0 {
                8 => Some(200),
                9 => Some(204),
                10 => Some(206),
                11 => Some(304),
                12 => Some(400),
                13 => Some(404),
                14 => Some(500),
                _ => None,
            };
        }
        // 原文：进表的是 01 开头、名字的编号 6 位；不进表的 0000、永不进表的 0001 开头，编号 4 位
        let prefix = if first & 0x40 != 0 { 6 } else { 4 };
        let (name, at) = read_int(block, at, prefix)?;
        let at = match name {
            8..=14 => at,
            0 => {
                let (name, at) = read_string(block, at)?;
                (name == b":status").then_some(at)?
            }
            _ => return None,
        };
        let value = String::from_utf8(read_string(block, at)?.0).ok()?;
        (value.len() == 3 && value.bytes().all(|c| c.is_ascii_digit())).then(|| value.parse().ok())?
    }

    /// 哈夫曼压缩过的一串数字（RFC 7541 附录 B：'0'～'2' 是 5 位的 00000～00010，'3'～'9' 是 6 位的 011001～011111）。
    /// 结尾的填充要是全 1、不到 8 位；里面有数字以外的字是 None
    pub fn digits(code: &[u8]) -> Option<String> {
        let total = code.len() * 8;
        let bit = |i: usize| u32::from(code.get(i / 8).map_or(0, |b| (b >> (7 - i % 8)) & 1));
        let take = |at: usize, n: usize| (at..at + n).fold(0u32, |acc, i| acc << 1 | bit(i));
        let mut out = String::new();
        let mut at = 0;
        while at < total {
            let left = total - at;
            if left < 8 && take(at, left) == (1 << left) - 1 {
                break;
            }
            if left >= 5 && take(at, 5) <= 2 {
                out.push(char::from(b'0' + take(at, 5) as u8));
                at += 5;
            } else if left >= 6 && (0x19..=0x1f).contains(&take(at, 6)) {
                out.push(char::from(b'3' + (take(at, 6) - 0x19) as u8));
                at += 6;
            } else {
                return None;
            }
        }
        Some(out)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use std::thread;

    const WAIT: Duration = Duration::from_secs(5);

    /// 对面（服务器那头）：读我们说的帧、照用例写它的
    struct Peer {
        s: UnixStream,
    }

    impl Peer {
        fn read_exact(&mut self, n: usize) -> Vec<u8> {
            let mut buf = vec![0u8; n];
            self.s.read_exact(&mut buf).unwrap();
            buf
        }

        fn frame(&mut self) -> Frame {
            let h = self.read_exact(9);
            let len = usize::from(h[0]) << 16 | usize::from(h[1]) << 8 | usize::from(h[2]);
            Frame { kind: h[3], flags: h[4], stream: u32::from_be_bytes([h[5], h[6], h[7], h[8]]), payload: self.read_exact(len) }
        }

        fn send(&mut self, bytes: &[u8]) {
            self.s.write_all(bytes).unwrap();
        }

        /// 我们说完一个请求：HEADERS（解出来的头）和 DATA 的内容
        fn request(&mut self, id: u32) -> (Vec<(String, String)>, Vec<u8>) {
            let h = self.frame();
            assert_eq!((h.kind, h.flags & END_HEADERS, h.stream), (HEADERS, END_HEADERS, id));
            let mut body = Vec::new();
            if h.flags & END_STREAM == 0 {
                loop {
                    let d = self.frame();
                    assert_eq!((d.kind, d.stream), (DATA, id));
                    body.extend_from_slice(&d.payload);
                    if d.flags & END_STREAM != 0 {
                        break;
                    }
                }
            }
            (decode(&h.payload), body)
        }
    }

    /// 我们写的头照 RFC 7541 解回来：只该有静态表的那三个编号、不进表的原文（名字是静态表的编号或者原文），不压缩
    fn decode(block: &[u8]) -> Vec<(String, String)> {
        let names = [(1, ":authority"), (4, ":path"), (23, "authorization"), (28, "content-length"), (31, "content-type")];
        let text = |b: &[u8], at: usize| {
            assert_eq!(b[at] & 0x80, 0, "不该压缩");
            let (len, at) = hpack::read_int(b, at, 7).unwrap();
            (String::from_utf8(b[at..at + len].to_vec()).unwrap(), at + len)
        };
        let (mut out, mut at) = (Vec::new(), 0);
        while at < block.len() {
            let b = block[at];
            if b & 0x80 != 0 {
                let pair = match b & 0x7f {
                    2 => (":method", "GET"),
                    3 => (":method", "POST"),
                    7 => (":scheme", "https"),
                    other => panic!("不该用静态表的 {other} 号"),
                };
                out.push((pair.0.to_string(), pair.1.to_string()));
                at += 1;
                continue;
            }
            assert_eq!(b & 0xf0, 0, "只该是不进表的原文");
            let (index, next) = hpack::read_int(block, at, 4).unwrap();
            let (name, next) = match index {
                0 => text(block, next),
                i => (names.iter().find(|(n, _)| *n == i).unwrap().1.to_string(), next),
            };
            let (value, next) = text(block, next);
            out.push((name, value));
            at = next;
        }
        out
    }

    /// 一串数字照 RFC 7541 附录 B 压缩（另写的一份，对着解的那一份）
    fn huff(digits: &str) -> Vec<u8> {
        let mut bits: Vec<u8> = Vec::new();
        for c in digits.bytes() {
            let (code, len) = if c <= b'2' { (u32::from(c - b'0'), 5) } else { (0x19 + u32::from(c - b'3'), 6) };
            bits.extend((0..len).rev().map(|i| ((code >> i) & 1) as u8));
        }
        while !bits.len().is_multiple_of(8) {
            bits.push(1);
        }
        bits.chunks(8).map(|b| b.iter().fold(0u8, |acc, bit| acc << 1 | bit)).collect()
    }

    fn pair() -> (Conn<UnixStream>, Peer) {
        let (a, b) = UnixStream::pair().unwrap();
        b.set_read_timeout(Some(WAIT)).unwrap();
        let mut peer = Peer { s: b };
        let conn = Conn::start(a, WAIT).unwrap();
        assert_eq!(peer.read_exact(PREFACE.len()), PREFACE);
        let s = peer.frame();
        assert_eq!((s.kind, s.flags, s.stream), (SETTINGS, 0, 0));
        assert_eq!(s.payload, [setting(ENABLE_PUSH, 0), setting(HEADER_TABLE_SIZE, 0)].concat(), "不要服务器推送、动态表是 0");
        (conn, peer)
    }

    const HEAD: [(&str, &str); 6] = [
        (":method", "POST"),
        (":scheme", "https"),
        (":authority", "api.example.com"),
        (":path", "/3/device/ab"),
        ("authorization", "bearer x.y.z"),
        ("apns-topic", "com.cca.remote"),
    ];

    fn ask(
        mut c: Conn<UnixStream>,
        body: &'static [u8],
        wait: Duration,
    ) -> thread::JoinHandle<(Conn<UnixStream>, Result<Reply, Fail>)> {
        thread::spawn(move || {
            let r = c.request(&HEAD, body, wait);
            (c, r)
        })
    }

    fn owned(head: &[(&str, &str)]) -> Vec<(String, String)> {
        head.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// 一个请求：头和正文照写的到了对面；对面的 SETTINGS、PING 回了 ACK；回的 200（前面有动态表大小的更新）、没有正文
    #[test]
    fn a_request_and_its_reply() {
        let (c, mut peer) = pair();
        let h = ask(c, b"{\"aps\":{}}", WAIT);
        let (head, body) = peer.request(1);
        assert_eq!((head, body), (owned(&HEAD), b"{\"aps\":{}}".to_vec()));
        peer.send(&frame(SETTINGS, 0, 0, &[setting(0x3, 1000), setting(MAX_FRAME_SIZE, 32_768)].concat()));
        peer.send(&frame(PING, 0, 0, b"12345678"));
        assert_eq!(peer.frame(), Frame { kind: SETTINGS, flags: ACK, stream: 0, payload: vec![] });
        assert_eq!(peer.frame(), Frame { kind: PING, flags: ACK, stream: 0, payload: b"12345678".to_vec() });
        let mut block = vec![0x20, 0x88];
        hpack::int(&mut block, 0x00, 4, 0);
        block.extend_from_slice(b"\x07apns-id\x24eabeae54-14a8-11e5-b60b-1697f925ec7b");
        peer.send(&frame(HEADERS, END_HEADERS | END_STREAM, 1, &block));
        let (c, r) = h.join().unwrap();
        assert_eq!(r.unwrap(), Reply { status: Some(200), body: vec![] });
        assert!(c.open());
        assert_eq!(c.max_frame, 32_768);
        // 下一个是 3 号流
        let h = ask(c, b"x", WAIT);
        assert_eq!(peer.request(3).1, b"x");
        peer.send(&frame(HEADERS, END_HEADERS | END_STREAM, 3, &[0x88]));
        assert_eq!(h.join().unwrap().1.unwrap().status, Some(200));
    }

    /// 出错的回复：状态码是压缩过的原文（填充、优先级、CONTINUATION 都有），原因在 DATA 里（也带填充）；收到的 DATA 还回窗口
    #[test]
    fn an_error_reply_in_pieces() {
        let (c, mut peer) = pair();
        let h = ask(c, b"{}", WAIT);
        peer.request(1);
        let mut status = vec![0x48, 0x80 | 2];
        status.extend(huff("410"));
        assert_eq!(&status[2..], [0x68, 0x20]);
        // HEADERS：填充 3 个字节、带优先级（5 个字节），里面只有前半段；CONTINUATION 接着后半段
        let mut first = vec![3];
        first.extend_from_slice(&[0, 0, 0, 0, 16]);
        first.extend_from_slice(&status[..2]);
        first.extend_from_slice(&[0, 0, 0]);
        peer.send(&frame(HEADERS, PADDED | PRIORITY, 1, &first));
        peer.send(&frame(CONTINUATION, END_HEADERS, 1, &status[2..]));
        let reason = br#"{"reason":"Unregistered","timestamp":1794000000000}"#;
        let mut data = vec![2];
        data.extend_from_slice(reason);
        data.extend_from_slice(&[0, 0]);
        peer.send(&frame(DATA, PADDED | END_STREAM, 1, &data));
        assert_eq!(
            peer.frame(),
            Frame { kind: WINDOW_UPDATE, flags: 0, stream: 0, payload: (data.len() as u32).to_be_bytes().to_vec() }
        );
        let (_, r) = h.join().unwrap();
        assert_eq!(r.unwrap(), Reply { status: Some(410), body: reason.to_vec() });
    }

    /// 流量控制：对面把流的初始窗口改成 10 以后，下一个请求先发 10 个字节，等它的 WINDOW_UPDATE 再发剩下的
    #[test]
    fn data_waits_for_the_window() {
        let (c, mut peer) = pair();
        let h = ask(c, b"first", WAIT);
        peer.request(1);
        peer.send(&frame(SETTINGS, 0, 0, &setting(INITIAL_WINDOW_SIZE, 10)));
        assert_eq!(peer.frame().flags, ACK);
        peer.send(&frame(HEADERS, END_HEADERS | END_STREAM, 1, &[0x88]));
        let (c, r) = h.join().unwrap();
        assert!(r.is_ok());
        let h = ask(c, b"0123456789abcdefghijklmno", WAIT);
        let head = peer.frame();
        assert_eq!((head.kind, head.stream, head.flags & END_STREAM), (HEADERS, 3, 0));
        assert_eq!(peer.frame(), Frame { kind: DATA, flags: 0, stream: 3, payload: b"0123456789".to_vec() });
        peer.send(&frame(WINDOW_UPDATE, 0, 3, &20u32.to_be_bytes()));
        assert_eq!(peer.frame(), Frame { kind: DATA, flags: END_STREAM, stream: 3, payload: b"abcdefghijklmno".to_vec() });
        peer.send(&frame(HEADERS, END_HEADERS | END_STREAM, 3, &[0x88]));
        assert_eq!(h.join().unwrap().1.unwrap().status, Some(200));
    }

    /// GOAWAY：编号比我们的流小——对面没处理，Retry；不小——这一个照样回完。之后这条连接不再开新的流
    #[test]
    fn goaway() {
        let (c, mut peer) = pair();
        let h = ask(c, b"x", WAIT);
        peer.request(1);
        peer.send(&frame(GOAWAY, 0, 0, &[0, 0, 0, 0, 0, 0, 0, 0]));
        let (c, r) = h.join().unwrap();
        assert!(matches!(r, Err(Fail::Retry)) && !c.open());
        let (mut c, _) = (c, ());
        assert!(matches!(c.request(&HEAD, b"x", WAIT), Err(Fail::Retry)), "不再开新的流");
        let (c, mut peer) = pair();
        let h = ask(c, b"x", WAIT);
        peer.request(1);
        peer.send(&frame(GOAWAY, 0, 0, &[0, 0, 0, 1, 0, 0, 0, 0]));
        peer.send(&frame(HEADERS, END_HEADERS | END_STREAM, 1, &[0x88]));
        let (c, r) = h.join().unwrap();
        assert_eq!(r.unwrap().status, Some(200));
        assert!(!c.open());
    }

    /// RST_STREAM：REFUSED_STREAM 是 Retry，别的是 Reset
    #[test]
    fn resets() {
        for (code, refused) in [(REFUSED_STREAM, true), (2, false)] {
            let (c, mut peer) = pair();
            let h = ask(c, b"x", WAIT);
            peer.request(1);
            peer.send(&frame(RST_STREAM, 0, 1, &code.to_be_bytes()));
            match (h.join().unwrap().1, refused) {
                (Err(Fail::Retry), true) => {}
                (Err(Fail::Reset(2)), false) => {}
                (other, _) => panic!("{code}: {other:?}"),
            }
        }
    }

    /// 不照协议来的、断了的、一直不回的
    #[test]
    fn broken_peers() {
        let mut long = vec![0u8; 9];
        long[..3].copy_from_slice(&[0x00, 0x40, 0x01]);
        let cases: [(&str, Vec<u8>); 4] = [
            ("服务器推送", frame(PUSH_PROMISE, END_HEADERS, 1, &[0, 0, 0, 2])),
            ("帧太长", long),
            ("头没收齐来了别的", [frame(HEADERS, 0, 1, &[0x88]), frame(PING, 0, 0, b"12345678")].concat()),
            ("SETTINGS 长度不对", frame(SETTINGS, 0, 0, &[0, 4, 0])),
        ];
        for (what, bytes) in cases {
            let (c, mut peer) = pair();
            let h = ask(c, b"x", WAIT);
            peer.request(1);
            peer.send(&bytes);
            assert!(matches!(h.join().unwrap().1, Err(Fail::Protocol(_))), "{what}");
        }
        // 对面关了
        let (c, mut peer) = pair();
        let h = ask(c, b"x", WAIT);
        peer.request(1);
        drop(peer);
        assert!(matches!(h.join().unwrap().1, Err(Fail::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof));
        // 一直不回：等到 wait 就不等了
        let (c, mut peer) = pair();
        let t0 = Instant::now();
        let h = ask(c, b"x", Duration::from_millis(300));
        peer.request(1);
        assert!(matches!(h.join().unwrap().1, Err(Fail::Io(_))));
        assert!(t0.elapsed() < Duration::from_secs(3));
    }

    /// 回的 :status 各种写法：RFC 7541 附录 C 的例子（C.5.1 不压缩、C.6.1 和 C.6.2 压缩过的 302、307）和别的几种
    #[test]
    fn status_in_every_spelling() {
        let cases: [(&[u8], Option<u16>); 14] = [
            (&[0x88], Some(200)),
            (&[0x8d], Some(404)),
            (&[0x20, 0x88], Some(200)),
            (&[0x3f, 0xe1, 0x1f, 0x88], Some(200)),
            (&[0x48, 0x03, b'3', b'0', b'2'], Some(302)),
            (&[0x48, 0x82, 0x64, 0x02], Some(302)),
            (&[0x48, 0x83, 0x64, 0x0e, 0xff], Some(307)),
            (&[0x08, 0x03, b'5', b'0', b'3'], Some(503)),
            (&[0x18, 0x03, b'4', b'2', b'9'], Some(429)),
            (&[0x40, 0x07, b':', b's', b't', b'a', b't', b'u', b's', 0x03, b'2', b'0', b'4'], Some(204)),
            (&[0xbe], None),
            (&[0x84], None),
            (&[0x48, 0x82, 0x1c, 0x6f], None),
            (&[0x48, 0x04, b'2', b'0', b'0', b'0'], None),
        ];
        for (block, want) in cases {
            assert_eq!(hpack::status(block), want, "{block:02x?}");
        }
        for code in 100..600 {
            let text = code.to_string();
            assert_eq!(hpack::digits(&huff(&text)).as_deref(), Some(text.as_str()));
            let mut block = vec![0x48, 0x80 | huff(&text).len() as u8];
            block.extend(huff(&text));
            assert_eq!(hpack::status(&block), Some(code as u16));
        }
        assert_eq!(hpack::digits(&[0xff]), None, "填充不能有 8 位");
        assert_eq!(hpack::digits(&[0x0f]).as_deref(), Some("1"), "00001 加 3 位的填充");
        assert_eq!(hpack::digits(&[0x08]), None, "数字后面的填充要是全 1");
    }

    /// 带前缀的整数：RFC 7541 C.1 的三个例子
    #[test]
    fn prefixed_integers() {
        let mut out = Vec::new();
        hpack::int(&mut out, 0x00, 5, 10);
        hpack::int(&mut out, 0x00, 5, 1337);
        hpack::int(&mut out, 0x00, 8, 42);
        assert_eq!(out, [0x0a, 0x1f, 0x9a, 0x0a, 0x2a]);
        assert_eq!(hpack::read_int(&out, 1, 5), Some((1337, 4)));
        for v in [0, 14, 15, 16, 127, 128, 300, 16_383, 1 << 20] {
            let mut b = Vec::new();
            hpack::int(&mut b, 0x40, 6, v);
            assert_eq!(hpack::read_int(&b, 0, 6), Some((v, b.len())));
        }
        let mut long = Vec::new();
        hpack::encode(&[("authorization", &"x".repeat(300))]).iter().take(4).for_each(|b| long.push(*b));
        assert_eq!(long, [0x0f, 0x08, 0x7f, 0xad]);
    }
}
