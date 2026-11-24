//! 网页端的 WebSocket（RFC 6455 里用得着的那一点）。Python 的 `Handler.websocket`、`ws_frame`、`ws_frames`、`ws_pump`
//! （Py:1720-1796、1853-1888）。
//!
//! 浏览器开不了普通的 TCP 连接，所以它把本该发到数据端口的字节装在二进制帧里发过来：这里拆出来，当成一条字节流交给中转
//! （`UnixStream` 的一头经 `relay::Handle::adopt` 交过去，和数据端口上接进来的连接一样对待），中转回的装成帧发回去。
//! 里面照旧是开场白和两头之间的密文，这里一个字节都不看。
//!
//! - `websocket`：GET /v1/relay/ws 的那一步。先看请求头像不像 WebSocket，再看来源（Origin）、按 IP 限流、票据、名额，
//!   先后和 Python 一步不差；成了回 `Outcome::Upgrade`（已经占了 `Web` 的一个名额）
//! - `next_frame`：拆帧。用了扩展位的、没加掩码的（浏览器发的一定加）、超过 `WS_FRAME` 的、控制帧超过 125 个字节的都不收；
//!   不看 FIN（分片的后续帧和二进制帧一样，内容接起来就是那条字节流），也不管长度是不是最短的写法
//! - `pump`：一条 WebSocket 一个线程（front.rs 处理这条连接的那个）。两头的套接字改成不阻塞的，等都用 `libc::poll`：同时等两头
//!   （Python 是 select），读一次、写一帧各有一个期限（Python 设了超时的套接字就是这样做的，sendall 的超时算的是整个写完的）。
//!   二进制帧和后续帧的内容交给中转，ping 回 pong，pong 不理；关闭帧、文本帧、不认识的操作码、不收的帧就停。中转那头关了、
//!   `WS_IDLE` 里两头都没动静、读写出错、一次读或者一次写（整个写完）`WS_STALL` 还没完也停。停了给浏览器发一个关闭帧
//!   （发不出去不管），再关掉中转那一头

use super::handler::{error, Outcome};
use super::http::Request;
use super::web::Web;
use super::wire::{ws_key_re, ws_ticket_re};
use super::{WS_FRAME, WS_GUID, WS_IDLE, WS_STALL};
use crate::py::base64::b64encode;
use crate::py::hash::sha1;
use rustls::{ServerConnection, StreamOwned};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

/// 一次从哪一头读多少（Python 的 recv(65536)）
const CHUNK: usize = 65536;

/// 服务器发的一帧（不加掩码）：op 是操作码（2 二进制、8 关闭、10 pong）（Py:1720-1724）
pub fn frame(op: u8, data: &[u8]) -> Vec<u8> {
    let n = data.len();
    let mut out = Vec::with_capacity(n.saturating_add(10));
    out.push(0x80 | op);
    match (u8::try_from(n), u16::try_from(n)) {
        (Ok(small), _) if small < 126 => out.push(small),
        (_, Ok(medium)) => {
            out.push(126);
            out.extend_from_slice(&medium.to_be_bytes());
        }
        _ => {
            out.push(127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(data);
    out
}

/// Sec-WebSocket-Accept（RFC 6455 4.2.2）：key 接上那串固定的 GUID 取 SHA-1，再写成 base64（Py:1872）
pub fn accept(key: &str) -> String {
    b64encode(&sha1(format!("{key}{WS_GUID}")))
}

/// 答应升级时回的那几行（Py:1873-1874）。和别的回复不一样：头一行是 HTTP/1.1，没有 Server、Date
pub fn switching(accept: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    )
    .into_bytes()
}

/// GET /v1/relay/ws?t=<票据的编号>：网页端进数据端口的路（Py:1853-1870）。只认自己那一页发起的（Origin，没带的也认：
/// 不是浏览器）、手里有一张没用过的接入票据的（没有凭证的人开不了、占不了名额）；进得了中转靠的还是开场白里证明有票据的密钥。
///
/// query 是请求行里的地址 `?` 后面那段；ip 是限流用的那个地址（`client_ip`）；bucket 按 IP 限流扣一下
/// （`ip_bucket.take(ip)`）；claim 用掉一张票据（`remote.ws_claim(编号, 现在)`）。先后照 Python：请求头不对 400 bad →
/// 来源不对 403 denied → 限流 429 rate → 票据 403 ticket → 名额 429 busy。成了是 `Outcome::Upgrade`：名额已经占了，
/// 这条连接完了（不管怎么完的）要 `Web::free(ip)`
pub fn websocket(
    req: &Request,
    query: &str,
    web: &Web,
    ip: &str,
    bucket: impl FnOnce() -> bool,
    claim: impl FnOnce(&str) -> bool,
) -> Outcome {
    let key = req.header("Sec-WebSocket-Key").unwrap_or("");
    let upgrade = req.header("Upgrade").unwrap_or("").eq_ignore_ascii_case("websocket");
    // 照 Python 只看里面有没有 upgrade 这几个字母（`keep-alive, Upgrade` 算，`Upgraded` 也算）。比的都是 ASCII，只转 ASCII 的
    // 大小写：请求头照 Python 按 latin-1 解的话，别的字符转了也成不了这几个字母，两边一样
    let connection = req.header("Connection").unwrap_or("").to_ascii_lowercase().contains("upgrade");
    if !upgrade || !connection || req.header("Sec-WebSocket-Version") != Some("13") || !ws_key_re().matches(key) {
        return error(400, "bad");
    }
    if req.header("Origin").is_some_and(|origin| origin != web.origin) {
        return error(403, "denied");
    }
    if !bucket() {
        return error(429, "rate");
    }
    if !ws_ticket_re().match_(query).and_then(|m| m.group(1)).is_some_and(claim) {
        return error(403, "ticket");
    }
    if !web.take(ip) {
        return error(429, "busy");
    }
    Outcome::Upgrade { accept: accept(key), ip: ip.to_string() }
}

/// 不收的帧、不认的操作码（Python 的 ValueError：'frame'、'long'、'op'）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bad {
    /// 用了扩展位（RSV1-3），或者没加掩码
    Frame,
    /// 超过 `WS_FRAME`，或者控制帧超过 125 个字节：一看到长度就不收，不等内容到齐
    Long,
    /// 文本帧、不认识的操作码
    Op,
}

/// buf 开头收齐了的一帧（Py:1727-1751）：(操作码, 去掉掩码的内容, 这一帧占了几个字节)；还不够一帧是 None
pub fn next_frame(buf: &[u8]) -> Result<Option<(u8, Vec<u8>, usize)>, Bad> {
    let (Some(&b0), Some(&b1)) = (buf.first(), buf.get(1)) else { return Ok(None) };
    if b0 & 0x70 != 0 || b1 & 0x80 == 0 {
        return Err(Bad::Frame);
    }
    let (n, at) = match b1 & 0x7f {
        126 => match buf.get(2..4).and_then(|b| <[u8; 2]>::try_from(b).ok()) {
            Some(len) => (u64::from(u16::from_be_bytes(len)), 4),
            None => return Ok(None),
        },
        127 => match buf.get(2..10).and_then(|b| <[u8; 8]>::try_from(b).ok()) {
            Some(len) => (u64::from_be_bytes(len), 10),
            None => return Ok(None),
        },
        small => (u64::from(small), 2),
    };
    if n > WS_FRAME as u64 || (b0 & 0x08 != 0 && n > 125) {
        return Err(Bad::Long);
    }
    // 上面查过了：n 不超过 1 MiB，加起来不会溢出
    let n = usize::try_from(n).map_err(|_| Bad::Long)?;
    let end = at + 4 + n;
    let (Some(mask), Some(data)) = (buf.get(at..at + 4), buf.get(at + 4..end)) else { return Ok(None) };
    let data = data.iter().zip(mask.iter().cycle()).map(|(d, m)| d ^ m).collect();
    Ok(Some((b0 & 0x0f, data, end)))
}

/// pump 要的那条连接（浏览器 ↔ 这里）：明文的、TLS 的都行。pump 一开始把底下的套接字改成不阻塞的，以后读写碰到
/// WouldBlock 就用 poll 等（等多久 pump 自己算），所以 read、write、flush 在不阻塞的套接字上要原样交回 WouldBlock
pub trait WsConn: Read + Write {
    /// 底下的套接字（poll 等它）
    fn fd(&self) -> RawFd;
    /// TLS 已经解出来、还没取走的字节数：它们不在套接字里，poll 看不到（Py:1775）。明文的没有
    fn pending(&mut self) -> io::Result<usize> {
        Ok(0)
    }
    /// TLS 还有没写出去的（握手以后的会话票据、回给对方的密钥更新这些）：读之前要先等写得出去
    /// （Python 的 SSL_ERROR_WANT_WRITE）。明文的没有
    fn wants_write(&self) -> bool {
        false
    }
    /// 底下的套接字改成不阻塞的
    fn nonblocking(&mut self) -> io::Result<()>;
}

impl WsConn for TcpStream {
    fn fd(&self) -> RawFd {
        self.as_raw_fd()
    }

    fn nonblocking(&mut self) -> io::Result<()> {
        self.set_nonblocking(true)
    }
}

impl WsConn for UnixStream {
    fn fd(&self) -> RawFd {
        self.as_raw_fd()
    }

    fn nonblocking(&mut self) -> io::Result<()> {
        self.set_nonblocking(true)
    }
}

/// TLS 的那条：rustls 包着底下的（front.rs 握完手的那个）
impl<S: WsConn> WsConn for StreamOwned<ServerConnection, S> {
    fn fd(&self) -> RawFd {
        self.sock.fd()
    }

    fn pending(&mut self) -> io::Result<usize> {
        let state = self.conn.process_new_packets().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(state.plaintext_bytes_to_read())
    }

    fn wants_write(&self) -> bool {
        self.conn.wants_write()
    }

    fn nonblocking(&mut self) -> io::Result<()> {
        self.sock.nonblocking()
    }
}

/// pump 等的两种时间（测试换成短的）
#[derive(Clone, Copy, Debug)]
pub struct Times {
    /// 读一次、写一帧（整个写完）最多等多久（Python 的 `settimeout(WS_STALL)`）
    pub stall: Duration,
    /// 两头都没动静多久就停（Python 的 select 的超时 `WS_IDLE`）
    pub idle: Duration,
}

impl Times {
    pub const REAL: Times =
        Times { stall: Duration::from_secs(WS_STALL.unsigned_abs()), idle: Duration::from_secs(WS_IDLE.unsigned_abs()) };
}

/// pump 为什么停了（Python 不分，都是 return；测试看它）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// 浏览器发了关闭帧
    Close,
    /// 浏览器那头读到头了
    Gone,
    /// 中转那头关了（票据不对、会话没了、对方断开、闲得太久）
    Relay,
    /// `WS_IDLE` 里两头都没动静
    Idle,
    /// 不收的帧、不认的操作码
    Bad(Bad),
    /// 读写出错、超时
    Io(io::ErrorKind),
    /// socketpair 都开不了（文件描述符用完了）：Python 这时连关闭帧都不发，socketpair 在 try 外面（Py:1758）
    NoPair,
}

/// 一条网页端的 WebSocket，从答应升级以后到完（Python 的 `ws_pump`，Py:1754-1796）。early：读请求时多读进来的字节
/// （浏览器等到回了 101 才发帧，一般没有）；adopt：把字节流的一头交给中转（`relay.adopt(a, ip)`）。
/// 停了以后给浏览器发一个关闭帧（发不出去不管）、关掉中转那一头；这条连接本身由调用的人关（它这时是不阻塞的）
pub fn pump<C: WsConn>(conn: &mut C, early: &[u8], adopt: impl FnOnce(UnixStream), times: Times) -> End {
    let Ok((a, mut b)) = UnixStream::pair() else { return End::NoPair };
    adopt(a);
    let end = shuttle(conn, &mut b, early, times).unwrap_or_else(|e| End::Io(e.kind()));
    let _ = send_all(conn, &frame(8, b""), times.stall);
    drop(b);
    end
}

/// pump 里面那个循环（Py:1761-1788）
fn shuttle<C: WsConn>(conn: &mut C, b: &mut UnixStream, early: &[u8], times: Times) -> io::Result<End> {
    conn.nonblocking()?;
    b.nonblocking()?;
    let mut buf = early.to_vec();
    let mut chunk = vec![0u8; CHUNK];
    loop {
        // 收齐了的帧一个个处理
        let mut used = 0;
        loop {
            let (op, data, n) = match next_frame(buf.get(used..).unwrap_or(&[])) {
                Ok(Some(f)) => f,
                Ok(None) => break,
                Err(bad) => return Ok(End::Bad(bad)),
            };
            used += n;
            match op {
                // 二进制的（和它的后续帧）：内容接起来就是那条字节流
                0 | 2 if data.is_empty() => {}
                0 | 2 => send_all(b, &data, times.stall)?,
                9 => send_all(conn, &frame(10, &data), times.stall)?,
                8 => return Ok(End::Close),
                10 => {}
                _ => return Ok(End::Bad(Bad::Op)), // 文本的、不认识的
            }
        }
        buf.drain(..used.min(buf.len()));
        // TLS 自己要发的（会话票据这些）先发出去再等：OpenSSL 是握手、读的时候就发了
        if conn.wants_write() {
            send_all(conn, b"", times.stall)?;
        }
        let pending = conn.pending()? > 0;
        let wait = if pending { Duration::ZERO } else { times.idle };
        let (from_conn, from_relay) = wait2(conn.fd(), b.as_raw_fd(), Instant::now() + wait)?;
        if !from_conn && !from_relay && !pending {
            return Ok(End::Idle);
        }
        if from_relay {
            let n = read_some(b, &mut chunk, times.stall)?;
            let Some(data) = chunk.get(..n).filter(|d| !d.is_empty()) else { return Ok(End::Relay) };
            send_all(conn, &frame(2, data), times.stall)?;
        }
        if pending || from_conn {
            let n = read_some(conn, &mut chunk, times.stall)?;
            let Some(data) = chunk.get(..n).filter(|d| !d.is_empty()) else { return Ok(End::Gone) };
            buf.extend_from_slice(data);
        }
    }
}

/// 读一次（Python 设了超时的 recv）：没有可读的就等，最多等 stall；TLS 一条记录没收齐也接着等，一共也最多 stall
fn read_some<C: WsConn + ?Sized>(c: &mut C, chunk: &mut [u8], stall: Duration) -> io::Result<usize> {
    let deadline = Instant::now() + stall;
    loop {
        match c.read(chunk) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                let events = if c.wants_write() { libc::POLLOUT } else { libc::POLLIN };
                wait(c.fd(), events, deadline)?;
            }
            other => return other,
        }
    }
}

/// Python 的 `sock.sendall(data)`（套接字设了超时）：整个写完最多等 stall——Python 3.5 起 sendall 的超时算的是总的，
/// 不是每写出去一点重新算（TLS 的 SSL_write 也是一次写完、一个期限）。对方一点点地读也拖不过 stall。
/// TLS 写进去的还在 rustls 里，flush 才真的出去；data 是空的就只是把 rustls 里攒着的发出去
fn send_all<C: WsConn + ?Sized>(c: &mut C, data: &[u8], stall: Duration) -> io::Result<()> {
    let deadline = Instant::now() + stall;
    let mut rest = data;
    while !rest.is_empty() {
        match c.write(rest) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => rest = rest.get(n..).unwrap_or(&[]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => wait(c.fd(), libc::POLLOUT, deadline)?,
            Err(e) => return Err(e),
        }
    }
    loop {
        match c.flush() {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => wait(c.fd(), libc::POLLOUT, deadline)?,
            Err(e) => return Err(e),
        }
    }
}

/// 等 fd 能读（POLLIN）或者能写（POLLOUT）了，到 deadline 还没有是超时（Python 的 "timed out"）。
/// 出错、对方关了也算（poll 一定报这几样），接下来的读写会知道是什么错
fn wait(fd: RawFd, events: libc::c_short, deadline: Instant) -> io::Result<()> {
    let mut fds = [libc::pollfd { fd, events, revents: 0 }];
    match poll_until(&mut fds, deadline)? {
        true => Ok(()),
        false => Err(io::ErrorKind::TimedOut.into()),
    }
}

/// 等两头哪个能读了（Python 的 `select.select([conn, b], [], [], timeout)`，Py:1776），最多等到 deadline：
/// (浏览器那头, 中转那头)。读到头、出错也算能读（select 也是这样，读的时候才知道）
fn wait2(conn: RawFd, relay: RawFd, deadline: Instant) -> io::Result<(bool, bool)> {
    let mut fds =
        [libc::pollfd { fd: conn, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: relay, events: libc::POLLIN, revents: 0 }];
    poll_until(&mut fds, deadline)?;
    let [c, r] = fds;
    Ok((c.revents != 0, r.revents != 0))
}

/// poll 这几个 fd，最多等到 deadline（已经过了就只看一眼）：有一个好了是 true，到时候了是 false。
/// 被信号打断了接着等剩下的时间
fn poll_until(fds: &mut [libc::pollfd], deadline: Instant) -> io::Result<bool> {
    loop {
        // 不足一毫秒的往上取：别为了几微秒空转一圈
        let ms = deadline.saturating_duration_since(Instant::now()).as_micros().div_ceil(1000);
        let ms = libc::c_int::try_from(ms).unwrap_or(libc::c_int::MAX);
        // SAFETY: fds 是初始化好的 pollfd，个数照实给；poll 只在这次调用里写它们的 revents
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) };
        match n {
            0 => return Ok(false),
            n if n > 0 => return Ok(true),
            _ => {
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::Interrupted {
                    return Err(e);
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::py::base64::b64decode;
    use crate::py::fs::TempDir;
    use crate::py::hash::{from_hex, sha256_hex};
    use crate::py::json::{self, Fmt};
    use crate::py::obj::{Dict, List, V};
    use crate::server::http::Method;
    use std::cell::Cell;
    use std::net::Shutdown;
    use std::path::Path;
    use std::sync::mpsc;
    use std::thread;

    /// 测试里的客户端加的掩码（和生成对照数据的脚本一样）
    const MASK: [u8; 4] = [0x37, 0xfa, 0x21, 0x3d];
    const WAIT: Duration = Duration::from_secs(5);

    /// 对照数据（cli/tests/vectors/server_ws.json）：真的 server/cca-presence.py 给的结果。生成它的是一份不进仓库的脚本：
    /// 进程里调 ws_frames；握手和 ws_pump 是在 socketpair 上跑真的 Handler（WS_IDLE、WS_STALL 换成 0.6、0.4 秒）
    fn vectors() -> Dict {
        json::loads(include_str!("../../tests/vectors/server_ws.json")).unwrap().as_dict().unwrap()
    }

    fn b64(s: &str) -> Vec<u8> {
        b64decode(s, false).unwrap()
    }

    fn int(v: V) -> usize {
        v.as_int().unwrap() as usize
    }

    /// 定下来的一串字节：对照数据里的 {"pattern": [个数, 种子]}，和脚本里的算法一样
    fn pattern(n: usize, seed: usize) -> Vec<u8> {
        (0..n).map(|i| ((i * 7 + seed * 13 + (i >> 8)) & 0xff) as u8).collect()
    }

    /// 对照数据里的一段字节：{"b64"}、{"text"}、{"fill": [字节, 个数]}、{"repeat": [b64, 次数]}、{"pattern": [个数, 种子]}
    fn payload(p: &Dict) -> Vec<u8> {
        if let Some(s) = p.str("b64") {
            return b64(&s);
        }
        if let Some(s) = p.str("text") {
            return s.as_bytes().to_vec();
        }
        if let Some(l) = p.list("fill") {
            return vec![int(l.at(0)) as u8; int(l.at(1))];
        }
        if let Some(l) = p.list("pattern") {
            return pattern(int(l.at(0)), int(l.at(1)));
        }
        let l = p.list("repeat").unwrap();
        b64(l.at(0).as_str().unwrap()).repeat(int(l.at(1)))
    }

    /// 客户端发的一帧（对照数据里的写法）：{"op", "data", "fin"=true, "mask"=十六进制|null, "rsv"=0, "len"=126|127}，
    /// 或者 {"raw": 字节}；"cut": [a, b] 只留第 a 到 b 个字节
    fn frame_bytes(f: &Dict) -> Vec<u8> {
        let out = match f.dict("raw") {
            Some(raw) => payload(&raw),
            None => {
                let data = f.dict("data").map(|d| payload(&d)).unwrap_or_default();
                let n = data.len();
                let fin = f.get("fin").is_none_or(|v| v.truthy());
                let b0 = if fin { 0x80 } else { 0 } | f.get("rsv").map_or(0, int) as u8 | int(f.at("op")) as u8;
                let mask = match f.get("mask") {
                    None => Some(MASK.to_vec()),
                    Some(V::None) => None,
                    Some(hex) => Some(from_hex(hex.as_str().unwrap()).unwrap()),
                };
                let m = if mask.is_some() { 0x80 } else { 0 };
                let form = f.get("len").map(int);
                let mut out = vec![b0];
                if form == Some(126) || (form.is_none() && (126..65536).contains(&n)) {
                    out.push(m | 126);
                    out.extend((n as u16).to_be_bytes());
                } else if form == Some(127) || (form.is_none() && n >= 65536) {
                    out.push(m | 127);
                    out.extend((n as u64).to_be_bytes());
                } else {
                    out.push(m | n as u8);
                }
                match mask {
                    None => out.extend(data),
                    Some(k) => {
                        out.extend(&k);
                        out.extend(data.iter().zip(k.iter().cycle()).map(|(d, k)| d ^ k));
                    }
                }
                out
            }
        };
        match f.list("cut") {
            Some(cut) => out[int(cut.at(0)).min(out.len())..int(cut.at(1)).min(out.len())].to_vec(),
            None => out,
        }
    }

    fn frames_bytes(fs: &List) -> Vec<u8> {
        fs.items().iter().flat_map(|f| frame_bytes(&f.as_dict().unwrap())).collect()
    }

    /// 客户端发一帧（加了掩码的）
    fn masked(op: u8, data: &[u8]) -> Vec<u8> {
        let mut out = frame(op, data);
        out[1] |= 0x80;
        let at = out.len() - data.len();
        let body: Vec<u8> = data.iter().zip(MASK.iter().cycle()).map(|(d, k)| d ^ k).collect();
        out.truncate(at);
        out.extend(MASK);
        out.extend(body);
        out
    }

    /// 和对照数据里记内容的写法一样：256 个字节以内是 {"b64"}，长的是 {"len", "sha256"}
    fn out_v(d: &[u8]) -> V {
        let out = Dict::new();
        if d.len() <= 256 {
            out.set("b64", V::str(b64encode(d)));
        } else {
            out.set("len", V::Int(d.len() as i64));
            out.set("sha256", V::str(sha256_hex(d)));
        }
        V::Dict(out)
    }

    fn same(a: &V, b: &V) -> bool {
        json::dumps(a, Fmt::new().sort_keys()) == json::dumps(b, Fmt::new().sort_keys())
    }

    fn show(v: &V) -> String {
        json::dumps(v, Fmt::new().sort_keys()).chars().take(400).collect()
    }

    /// 浏览器收到的：一串帧拆成事件，挨着的二进制帧接成一个 data（和生成对照数据的脚本一样）
    fn server_events(data: &[u8]) -> V {
        let mut ev: Vec<(String, Vec<u8>)> = Vec::new();
        let mut i = 0;
        let out = List::new();
        while i < data.len() {
            if data.len() - i < 2 {
                ev.push(("partial".into(), vec![(data.len() - i) as u8]));
                break;
            }
            let (b0, b1) = (data[i], data[i + 1]);
            let (mut n, mut at) = ((b1 & 0x7f) as usize, i + 2);
            if n == 126 {
                n = u16::from_be_bytes(data[at..at + 2].try_into().unwrap()) as usize;
                at += 2;
            } else if n == 127 {
                n = u64::from_be_bytes(data[at..at + 8].try_into().unwrap()) as usize;
                at += 8;
            }
            let body = &data[at..(at + n).min(data.len())];
            if b1 & 0x80 != 0 || body.len() < n || b0 & 0x70 != 0 || b0 & 0x80 == 0 {
                ev.push(("odd".into(), data[i..].to_vec()));
                break;
            }
            match (b0 & 0x0f, ev.last_mut()) {
                (2, Some((kind, d))) if kind == "data" => d.extend(body),
                (2, _) => ev.push(("data".into(), body.to_vec())),
                (op, _) => {
                    let kind = match op {
                        8 => "close".to_string(),
                        9 => "ping".to_string(),
                        10 => "pong".to_string(),
                        op => format!("op{op}"),
                    };
                    ev.push((kind, body.to_vec()));
                }
            }
            i = at + n;
        }
        for (kind, d) in ev {
            let item = List::new();
            item.push(V::str(&kind));
            match kind.as_str() {
                "partial" => item.push(V::Int(i64::from(d[0]))),
                "odd" => item.push(V::str(b64encode(&d))),
                _ => item.push(out_v(&d)),
            }
            out.push(V::List(item));
        }
        V::List(out)
    }

    /// 服务器发的（不加掩码的）一帧收齐了没有：(操作码, 内容, 占了几个字节)
    fn whole_frame(data: &[u8]) -> Option<(u8, Vec<u8>, usize)> {
        let (b0, b1) = (*data.first()?, *data.get(1)?);
        assert_eq!(b1 & 0x80, 0, "服务器发的帧不加掩码");
        let (n, at) = match b1 & 0x7f {
            126 => (u16::from_be_bytes(data.get(2..4)?.try_into().unwrap()) as usize, 4),
            127 => (u64::from_be_bytes(data.get(2..10)?.try_into().unwrap()) as usize, 10),
            n => (n as usize, 2),
        };
        Some((b0 & 0x0f, data.get(at..at + n)?.to_vec(), at + n))
    }

    /// 只有 index.html 的页面：握手用不着别的文件
    fn small_web(origin: &str) -> (TempDir, Web) {
        let dir = TempDir::new().unwrap();
        std::fs::write(Path::new(&dir.path).join("index.html"), "<title>cca</title>\n").unwrap();
        let web = Web::open(&format!("{origin}/"), Some(Path::new(&dir.path)), None).unwrap();
        (dir, web)
    }

    fn request(target: &str, headers: &[(&str, &str)]) -> Request {
        let headers = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Request { method: Method::Get, target: target.into(), headers, body: Vec::new() }
    }

    fn json_reply(o: &Outcome) -> Option<(u16, String)> {
        match o {
            Outcome::Json(code, v) => Some((*code, json::dumps(v, Fmt::new().utf8().compact()))),
            _ => None,
        }
    }

    /// RFC 6455 1.3 的例子，和对照数据里另外几把
    #[test]
    fn accept_keys() {
        assert_eq!(accept("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
        let doc = vectors();
        assert_eq!(doc.str("guid").as_deref(), Some(WS_GUID));
        for case in doc.list("accept").unwrap().items() {
            let case = case.as_list().unwrap();
            assert_eq!(accept(case.at(0).as_str().unwrap()), case.at(1).as_str().unwrap());
        }
        assert_eq!(
            switching("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n"
        );
    }

    /// 长度的三种写法，和 Python 的 ws_frame 给的一样
    #[test]
    fn frame_lengths() {
        assert_eq!(frame(2, b""), [0x82, 0x00]);
        assert_eq!(frame(8, b""), [0x88, 0x00]);
        assert_eq!(frame(10, b"hi"), [0x8a, 0x02, b'h', b'i']);
        let f = frame(2, &[7u8; 125]);
        assert_eq!((&f[..2], f.len()), (&[0x82, 125][..], 127));
        let f = frame(2, &[7u8; 126]);
        assert_eq!((&f[..4], f.len()), (&[0x82, 126, 0, 126][..], 130));
        let f = frame(2, &[7u8; 65535]);
        assert_eq!((&f[..4], f.len()), (&[0x82, 126, 0xff, 0xff][..], 65539));
        let f = frame(2, &[7u8; 65536]);
        assert_eq!((&f[..10], f.len()), (&[0x82, 127, 0, 0, 0, 0, 0, 1, 0, 0][..], 65546));
        assert!(f[10..].iter().all(|b| *b == 7));
    }

    /// 拆帧：各种长度（到 WS_FRAME 为止）、不完整的、不收的
    #[test]
    fn frames_of_every_length_class() {
        for n in [0, 1, 4, 5, 125, 126, 127, 65535, 65536, 70000, WS_FRAME] {
            let data = pattern(n, n % 7);
            let f = masked(2, &data);
            assert_eq!(next_frame(&f), Ok(Some((2, data.clone(), f.len()))), "{n}");
            // 少一个字节都还不是一帧（只看几个位置：头、掩码、内容的开头和结尾）
            for cut in [0, 1, 2, 3, 5, 9, 10, 13, 14, f.len() / 2, f.len() - 1] {
                if cut < f.len() {
                    assert_eq!(next_frame(&f[..cut]), Ok(None), "{n} 截到 {cut}");
                }
            }
            // 后面跟着别的：只取这一帧
            let mut two = f.clone();
            two.extend(masked(9, b"next"));
            assert_eq!(next_frame(&two).unwrap().unwrap().2, f.len());
        }
        // 控制帧最多 125 个字节：看到长度就不收
        assert_eq!(next_frame(&masked(9, &[1; 125])).unwrap().unwrap().1, vec![1; 125]);
        for op in [8, 9, 10, 11, 15] {
            assert_eq!(next_frame(&masked(op, &[1; 126])[..4]), Err(Bad::Long), "{op}");
        }
        // 超过 WS_FRAME：只有头也不收；正好 WS_FRAME 的头收了、等内容
        let mut head = vec![0x82, 0xff];
        head.extend((WS_FRAME as u64 + 1).to_be_bytes());
        assert_eq!(next_frame(&head), Err(Bad::Long));
        head[2..].copy_from_slice(&(WS_FRAME as u64).to_be_bytes());
        assert_eq!(next_frame(&head), Ok(None));
        head[2..].copy_from_slice(&u64::MAX.to_be_bytes());
        assert_eq!(next_frame(&head), Err(Bad::Long));
        // 没加掩码、用了扩展位：两个字节就够判断
        assert_eq!(next_frame(&frame(2, b"x")), Err(Bad::Frame));
        for rsv in [0x40, 0x20, 0x10] {
            let mut f = masked(2, b"x");
            f[0] |= rsv;
            assert_eq!(next_frame(&f[..2]), Err(Bad::Frame), "{rsv:#x}");
        }
        // 不看 FIN、不管长度是不是最短的写法
        let mut f = masked(9, b"p");
        f[0] &= 0x7f;
        assert_eq!(next_frame(&f), Ok(Some((9, b"p".to_vec(), f.len()))));
        let mut long_form = vec![0x82, 0x80 | 127];
        long_form.extend(5u64.to_be_bytes());
        long_form.extend(MASK);
        long_form.extend(b"short".iter().zip(MASK.iter().cycle()).map(|(d, k)| d ^ k));
        assert_eq!(next_frame(&long_form), Ok(Some((2, b"short".to_vec(), 19))));
    }

    #[test]
    fn vectors_frames() {
        for case in vectors().list("frames").unwrap().items() {
            let case = case.as_dict().unwrap();
            let about = case.str("about").unwrap();
            let mut buf = frames_bytes(&case.list("in").unwrap());
            let got = List::new();
            let end = Dict::new();
            loop {
                match next_frame(&buf) {
                    Ok(Some((op, data, n))) => {
                        let item = List::new();
                        item.push(V::Int(i64::from(op)));
                        item.push(out_v(&data));
                        got.push(V::List(item));
                        buf.drain(..n);
                    }
                    Ok(None) => {
                        end.set("more", V::Int(buf.len() as i64));
                        break;
                    }
                    Err(bad) => {
                        end.set("error", V::str(if bad == Bad::Long { "long" } else { "frame" }));
                        end.set("left", V::Int(buf.len() as i64));
                        break;
                    }
                }
            }
            assert!(same(&V::List(got.clone()), &case.at("frames")), "{about}：{}", show(&V::List(got)));
            assert!(same(&V::Dict(end.clone()), &case.at("end")), "{about}：{}", show(&V::Dict(end)));
        }
    }

    /// 握手：真的 Python 服务器在各种请求头、限流、票据、名额下回了什么，这里一样（请求头的值照 Python 读到的样子给：
    /// 去掉开头的空白，结尾的留着）
    #[test]
    fn vectors_handshake() {
        let doc = vectors();
        let hs = doc.dict("handshake").unwrap();
        let (origin, ticket, ip) = (hs.str("origin").unwrap(), hs.str("ticket").unwrap(), hs.str("ip").unwrap());
        for case in hs.list("cases").unwrap().items() {
            let case = case.as_dict().unwrap();
            let about = case.str("about").unwrap();
            let (_dir, web) = small_web(&origin);
            let taken = int(case.at("taken"));
            for _ in 0..taken {
                assert!(web.take(&ip));
            }
            let headers: Vec<(String, String)> = case
                .list("headers")
                .unwrap()
                .items()
                .iter()
                .map(|h| {
                    let h = h.as_list().unwrap();
                    (h.at(0).as_str().unwrap().to_string(), h.at(1).as_str().unwrap().trim_start_matches([' ', '\t']).to_string())
                })
                .collect();
            let query = case.str("query").unwrap();
            let r = Request { method: Method::Get, target: format!("/v1/relay/ws{query}"), headers, body: Vec::new() };
            let (bucket, claim) = (case.at("bucket").truthy(), case.at("claim").truthy());
            let out = websocket(&r, query.strip_prefix('?').unwrap_or(&query), &web, &ip, || bucket, |t| claim && t == &*ticket);
            let status = case.str("status_line").unwrap();
            let want_conns = int(case.at("slots_after").as_list().unwrap().at(0));
            match out {
                Outcome::Upgrade { accept, ip: got_ip } => {
                    assert!(status.starts_with("HTTP/1.1 101 "), "{about}：{status}");
                    let mut want = format!("{status}\r\n");
                    for h in case.list("response_headers").unwrap().items() {
                        let h = h.as_list().unwrap();
                        want.push_str(&format!("{}: {}\r\n", h.at(0).as_str().unwrap(), h.at(1).as_str().unwrap()));
                    }
                    want.push_str("\r\n");
                    assert_eq!(String::from_utf8(switching(&accept)).unwrap(), want, "{about}");
                    assert_eq!((got_ip.as_str(), web.conns()), (case.str("ip").as_deref().unwrap(), taken + 1), "{about}");
                    web.free(&got_ip);
                }
                out => {
                    let (code, body) = json_reply(&out).unwrap();
                    assert_eq!(code.to_string(), status.split(' ').nth(1).unwrap(), "{about}");
                    assert_eq!(body, case.str("body").unwrap().to_string(), "{about}");
                }
            }
            assert_eq!(web.conns(), want_conns, "{about}：名额");
        }
    }

    /// 先后：请求头不对时不看来源、不扣限流、不用票据；票据的样子不对时不去用它
    #[test]
    fn handshake_checks_in_order() {
        let (_dir, web) = small_web("https://relay.example.com");
        let good = [
            ("Upgrade", "websocket"),
            ("Connection", "Upgrade"),
            ("Sec-WebSocket-Version", "13"),
            ("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ];
        let calls = Cell::new(0);
        let count = || {
            calls.set(calls.get() + 1);
            true
        };
        let bad_key = request("/v1/relay/ws?t=x", &[good[0], good[1], good[2], ("Origin", "https://evil.example.com")]);
        let out = websocket(&bad_key, "t=x", &web, "192.0.2.1", count, |_| panic!("不该用票据"));
        assert_eq!((json_reply(&out).unwrap().0, calls.get()), (400, 0));
        let mut h = good.to_vec();
        h.push(("Origin", "https://evil.example.com"));
        let out = websocket(&request("/v1/relay/ws", &h), "", &web, "192.0.2.1", count, |_| panic!("不该用票据"));
        assert_eq!((json_reply(&out).unwrap().0, calls.get()), (403, 0));
        let ticket = "0123456789abcdef0123456789abcdef";
        for query in ["", "t=", "t=0123", "x=1&t=0123456789abcdef0123456789abcdef", "t=0123456789ABCDEF0123456789ABCDEF"] {
            let out = websocket(&request("/v1/relay/ws", &good), query, &web, "192.0.2.1", count, |_| panic!("不该用票据"));
            assert_eq!(json_reply(&out).unwrap(), (403, r#"{"error":"ticket"}"#.to_string()), "{query}");
        }
        let seen = std::cell::RefCell::new(String::new());
        let out = websocket(
            &request("/v1/relay/ws", &good),
            &format!("t={ticket}"),
            &web,
            "192.0.2.1",
            || true,
            |t| {
                *seen.borrow_mut() = t.to_string();
                true
            },
        );
        assert!(matches!(out, Outcome::Upgrade { ref ip, .. } if ip == "192.0.2.1"));
        assert_eq!((seen.borrow().as_str(), web.conns()), (ticket, 1));
    }

    /// 读到头或者出错才停的读者：读到的都交回来
    fn reader(mut s: UnixStream) -> mpsc::Receiver<Vec<u8>> {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut all = Vec::new();
            let mut chunk = [0u8; 65536];
            loop {
                match s.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => all.extend_from_slice(&chunk[..n]),
                }
            }
            let _ = tx.send(all);
        });
        rx
    }

    /// 写出去，出错不管；写不完（对面不读）最多等 5 秒（和生成对照数据的脚本一样）
    fn send_quietly(s: &UnixStream, data: &[u8]) {
        let (mut s, data) = (s.try_clone().unwrap(), data.to_vec());
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = s.write_all(&data);
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(WAIT);
    }

    enum Step {
        Client(Vec<u8>),
        Relay(Vec<u8>),
        ClientShut,
        RelayShut,
        RelayClose,
        Wait(Duration),
    }

    /// 一次 pump：浏览器那头、中转那头各是 socketpair 的一头，照 steps 一步步来，每步之后等 settle。
    /// 给出 pump 为什么停的、浏览器收到的全部字节、中转收到的全部字节
    fn run(early: &[u8], steps: &[Step], relay_reads: bool, times: Times, settle: Duration) -> (End, Vec<u8>, Vec<u8>) {
        let (client, mut server) = UnixStream::pair().unwrap();
        let (relay_tx, relay_rx) = mpsc::channel();
        let (end_tx, end_rx) = mpsc::channel();
        let early = early.to_vec();
        thread::spawn(move || {
            let end = pump(&mut server, &early, |a| relay_tx.send(a).unwrap(), times);
            // 和 Python 的 socketserver 一样：处理完了先关写的那一半，再关掉
            let _ = server.shutdown(Shutdown::Write);
            drop(server);
            let _ = end_tx.send(end);
        });
        let relay: UnixStream = relay_rx.recv_timeout(WAIT).unwrap();
        let from_server = reader(client.try_clone().unwrap());
        let mut from_relay = relay_reads.then(|| reader(relay.try_clone().unwrap()));
        for step in steps {
            match step {
                Step::Client(data) => send_quietly(&client, data),
                Step::Relay(data) => send_quietly(&relay, data),
                Step::ClientShut => drop(client.shutdown(Shutdown::Write)),
                Step::RelayShut => drop(relay.shutdown(Shutdown::Write)),
                Step::RelayClose => drop(relay.shutdown(Shutdown::Both)),
                Step::Wait(d) => thread::sleep(*d),
            }
            thread::sleep(settle);
        }
        let got = from_server.recv_timeout(WAIT).expect("浏览器那头该读到头");
        let end = end_rx.recv_timeout(WAIT).expect("pump 该停了");
        let relayed =
            from_relay.get_or_insert_with(|| reader(relay.try_clone().unwrap())).recv_timeout(WAIT).expect("中转那头该读到头");
        (end, got, relayed)
    }

    struct Scenario {
        about: String,
        early: Vec<u8>,
        steps: Vec<Step>,
        relay_reads: bool,
        loose: bool,
    }

    /// 对照数据里的 pump：真的 Python 服务器（握手以后）浏览器收到了什么、中转收到了什么，这里一样。几十个一起跑（各自的
    /// socketpair、线程），一共一两秒
    #[test]
    fn vectors_pump() {
        let doc = vectors();
        let p = doc.dict("pump").unwrap();
        let ms = |k: &str| Duration::from_millis(int(p.at(k)) as u64);
        let times = Times { stall: ms("stall_ms"), idle: ms("idle_ms") };
        let settle = ms("settle_ms");
        let cases: Vec<Dict> = p.list("cases").unwrap().items().iter().map(|c| c.as_dict().unwrap()).collect();
        let mut running = Vec::new();
        for case in &cases {
            let steps = case
                .list("steps")
                .unwrap()
                .items()
                .iter()
                .map(|s| {
                    let s = s.as_list().unwrap();
                    match s.at(0).as_str().unwrap() {
                        "client" => Step::Client(frames_bytes(&s.at(1).as_list().unwrap())),
                        "relay" => Step::Relay(payload(&s.at(1).as_dict().unwrap())),
                        "client_shut" => Step::ClientShut,
                        "relay_shut" => Step::RelayShut,
                        "relay_close" => Step::RelayClose,
                        "wait" => Step::Wait(Duration::from_millis(int(s.at(1)) as u64)),
                        other => panic!("认不得的一步 {other}"),
                    }
                })
                .collect();
            let s = Scenario {
                about: case.str("about").unwrap().to_string(),
                early: frames_bytes(&case.list("extra").unwrap()),
                steps,
                relay_reads: case.at("relay_reads").truthy(),
                loose: case.at("loose").truthy(),
            };
            running.push(thread::spawn(move || {
                let got = run(&s.early, &s.steps, s.relay_reads, times, settle);
                (s.about, s.loose, got)
            }));
        }
        for (case, t) in cases.iter().zip(running) {
            let (about, loose, (end, got, relayed)) = t.join().unwrap();
            assert_eq!(case.str("about").as_deref(), Some(about.as_str()));
            let events = server_events(&got);
            let events = match (loose, &events) {
                (true, V::List(l)) => {
                    let last = List::new();
                    last.push(l.at(-1));
                    V::List(last)
                }
                _ => events,
            };
            assert!(same(&events, &case.at("client")), "{about}（{end:?}）：浏览器收到的 {}", show(&events));
            if !loose {
                assert!(same(&out_v(&relayed), &case.at("relay")), "{about}（{end:?}）：中转收到的 {}", show(&out_v(&relayed)));
            }
            assert_eq!((case.str("client_end").as_deref(), case.str("relay_end").as_deref()), (Some("eof"), Some("eof")));
        }
    }

    const QUICK: Times = Times { stall: Duration::from_millis(200), idle: Duration::from_millis(300) };
    const SETTLE: Duration = Duration::from_millis(30);

    /// 两头的数据、每一头先关、超时：停的原因对，浏览器最后都收到一个关闭帧
    #[test]
    fn pump_ends() {
        let close = masked(8, &[3, 0xe8]);
        let close_frame = frame(8, b"");
        let cases: Vec<(&str, Vec<u8>, Vec<Step>, End, Vec<u8>, Vec<u8>)> = vec![
            (
                "浏览器关",
                vec![],
                vec![Step::Client(masked(2, b"up")), Step::Relay(b"down".to_vec()), Step::Client(close.clone())],
                End::Close,
                b"up".to_vec(),
                [frame(2, b"down"), close_frame.clone()].concat(),
            ),
            (
                "中转关",
                vec![],
                vec![Step::Client(masked(2, b"x")), Step::RelayClose],
                End::Relay,
                b"x".to_vec(),
                close_frame.clone(),
            ),
            (
                "中转先说完再关写的一半",
                vec![],
                vec![Step::Relay(b"bye".to_vec()), Step::RelayShut],
                End::Relay,
                vec![],
                [frame(2, b"bye"), close_frame.clone()].concat(),
            ),
            (
                "浏览器读到头",
                vec![],
                vec![Step::Client(masked(2, b"x")), Step::ClientShut],
                End::Gone,
                b"x".to_vec(),
                close_frame.clone(),
            ),
            ("闲着", vec![], vec![], End::Idle, vec![], close_frame.clone()),
            ("没加掩码", vec![], vec![Step::Client(frame(2, b"x"))], End::Bad(Bad::Frame), vec![], close_frame.clone()),
            ("文本帧", vec![], vec![Step::Client(masked(1, b"hi"))], End::Bad(Bad::Op), vec![], close_frame.clone()),
            ("太长的 ping", vec![], vec![Step::Client(masked(9, &[0; 126]))], End::Bad(Bad::Long), vec![], close_frame.clone()),
            (
                "ping 回 pong",
                masked(9, b"p"),
                vec![Step::Client(close.clone())],
                End::Close,
                vec![],
                [frame(10, b"p"), close_frame.clone()].concat(),
            ),
        ];
        for (about, early, steps, end, relayed, got) in cases {
            let (e, g, r) = run(&early, &steps, true, QUICK, SETTLE);
            assert_eq!((e, g, r), (end, got, relayed), "{about}");
        }
    }

    /// 中转那头不读：往它那里写一次最多等 stall，停了照样给浏览器发关闭帧
    #[test]
    fn pump_stalls_when_the_relay_does_not_read() {
        let big: Vec<u8> = (0..48).flat_map(|i| masked(2, &pattern(65536, i))).collect();
        let started = Instant::now();
        let (end, got, _) = run(&[], &[Step::Client(big)], false, QUICK, SETTLE);
        assert!(matches!(end, End::Io(_)), "{end:?}");
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
        assert!(got.ends_with(&frame(8, b"")));
    }

    /// 浏览器不读、中转一直发：发给浏览器的那一帧写不完，最多等 stall 就停
    #[test]
    fn pump_stalls_when_the_browser_does_not_read() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        // 先设好：macOS 上对方关了以后 setsockopt 是 EINVAL
        client.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        let (relay_tx, relay_rx) = mpsc::channel();
        let (end_tx, end_rx) = mpsc::channel();
        thread::spawn(move || {
            let end = pump(&mut server, &[], |a| relay_tx.send(a).unwrap(), QUICK);
            let _ = end_tx.send(end);
        });
        let relay: UnixStream = relay_rx.recv_timeout(WAIT).unwrap();
        let started = Instant::now();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let mut feed = relay.try_clone().unwrap();
        thread::spawn(move || {
            let chunk = vec![7u8; 65536];
            while stop_rx.try_recv().is_err() && feed.write_all(&chunk).is_ok() {}
        });
        let end = end_rx.recv_timeout(WAIT).expect("pump 该停了");
        let _ = stop_tx.send(());
        assert!(matches!(end, End::Io(_)), "{end:?}");
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
        let mut some = [0u8; 16];
        assert!(client.read(&mut some).unwrap() > 0, "之前的照样发出去了");
    }

    /// sendall 的超时是总的：对面一点点地读，每次写都有进展，也只等 stall（不然慢慢读的一方能一直拖着）
    #[test]
    fn send_all_is_bounded_in_total() {
        let (mut a, b) = UnixStream::pair().unwrap();
        a.set_nonblocking(true).unwrap();
        let filler = [0u8; 4096];
        // 填满（pump 里两头本来就是不阻塞的）
        while a.write(&filler).is_ok() {}
        let mut slow = b.try_clone().unwrap();
        thread::spawn(move || {
            let mut small = [0u8; 512];
            while slow.read(&mut small).is_ok_and(|n| n > 0) {
                thread::sleep(Duration::from_millis(20));
            }
        });
        let started = Instant::now();
        let r = send_all(&mut a, &vec![1u8; 256 * 1024], Duration::from_millis(300));
        let took = started.elapsed();
        assert!(r.is_err(), "该超时");
        assert!(took >= Duration::from_millis(250) && took < Duration::from_secs(2), "{took:?}");
        drop(b);
    }

    /// TLS 已经解出来、还没取走的字节 poll 看不到：pump 要先取它们，不能干等到 idle
    #[test]
    fn pump_takes_pending_bytes_first() {
        struct Buffered {
            sock: UnixStream,
            pre: Vec<u8>,
        }
        impl Read for Buffered {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.pre.is_empty() {
                    return self.sock.read(buf);
                }
                let n = buf.len().min(self.pre.len());
                buf[..n].copy_from_slice(&self.pre[..n]);
                self.pre.drain(..n);
                Ok(n)
            }
        }
        impl Write for Buffered {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.sock.write(buf)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        impl WsConn for Buffered {
            fn fd(&self) -> RawFd {
                self.sock.as_raw_fd()
            }
            fn pending(&mut self) -> io::Result<usize> {
                Ok(self.pre.len())
            }
            fn nonblocking(&mut self) -> io::Result<()> {
                self.sock.set_nonblocking(true)
            }
        }
        let (client, server) = UnixStream::pair().unwrap();
        let mut conn = Buffered { sock: server, pre: [masked(2, b"held"), masked(8, b"")].concat() };
        let (tx, rx) = mpsc::channel();
        let slow = Times { stall: Duration::from_secs(1), idle: Duration::from_secs(3) };
        let started = Instant::now();
        let end = pump(&mut conn, &[], |a| tx.send(a).unwrap(), slow);
        assert_eq!(end, End::Close);
        assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
        let mut relayed = Vec::new();
        rx.recv().unwrap().read_to_end(&mut relayed).unwrap();
        assert_eq!(relayed, b"held");
        drop(client);
    }

    /// 真的 TLS（rustls，和 front.rs 用的一样）：握完手以后照样两头搬，最后浏览器收到关闭帧
    #[test]
    fn pump_over_tls() {
        use crate::server::tls::{client_config, load_pair};
        use crate::server::x509::testkit::{new_key, pem, self_signed};
        use crate::server::x509::San;
        use rustls::pki_types::{CertificateDer, ServerName};
        use rustls::{ClientConnection, RootCertStore};
        let dir = TempDir::new().unwrap();
        let key = new_key();
        let cert = self_signed(&key, Some(&[San::Dns("relay.example.com".into())]));
        let (cert_path, key_path) = (Path::new(&dir.path).join("cert.pem"), Path::new(&dir.path).join("key.pem"));
        std::fs::write(&cert_path, pem("CERTIFICATE", &cert)).unwrap();
        std::fs::write(&key_path, pem("PRIVATE KEY", &key)).unwrap();
        let server_config = load_pair(&cert_path, &key_path).unwrap();
        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from(cert.clone())).unwrap();
        let client_config = client_config(roots).unwrap();

        let (client_sock, server_sock) = UnixStream::pair().unwrap();
        let (relay_tx, relay_rx) = mpsc::channel();
        let pumping = thread::spawn(move || {
            let mut tls = StreamOwned::new(ServerConnection::new(server_config).unwrap(), server_sock);
            tls.sock.set_read_timeout(Some(WAIT)).unwrap();
            tls.sock.set_write_timeout(Some(WAIT)).unwrap();
            while tls.conn.is_handshaking() {
                tls.conn.complete_io(&mut tls.sock).unwrap();
            }
            pump(&mut tls, &[], |a| relay_tx.send(a).unwrap(), QUICK)
        });
        let name = ServerName::try_from("relay.example.com".to_string()).unwrap();
        let mut client = StreamOwned::new(ClientConnection::new(client_config, name).unwrap(), client_sock);
        client.sock.set_read_timeout(Some(WAIT)).unwrap();
        client.sock.set_write_timeout(Some(WAIT)).unwrap();
        // 中转那头另起一个线程：pump 往中转写的时候不读浏览器那头（Python 也是），两头都在这个线程里做会互相等着
        let relay_side = thread::spawn(move || {
            let mut relay: UnixStream = relay_rx.recv_timeout(WAIT).unwrap();
            relay.set_read_timeout(Some(WAIT)).unwrap();
            let mut got = vec![0u8; 43000];
            relay.read_exact(&mut got).unwrap();
            assert_eq!(got, [pattern(40000, 1), pattern(3000, 2)].concat(), "二进制帧和后续帧的内容接起来");
            relay.write_all(&pattern(70000, 3)).unwrap();
            relay
        });
        // 几帧一次写出去（TLS 会切成几条记录）：最大的那帧比一条记录（16 KiB）大
        let up: Vec<u8> = [masked(2, &pattern(40000, 1)), masked(9, b"ping"), masked(0, &pattern(3000, 2))].concat();
        client.write_all(&up).unwrap();
        client.flush().unwrap();
        // pong，然后是中转发的（可能分成几帧）：收齐了的帧拆出来，二进制的接起来
        let (mut down, mut chunk) = (Vec::new(), [0u8; 65536]);
        let (mut ops, mut data) = (Vec::new(), Vec::new());
        while data.len() < 70000 {
            let n = client.read(&mut chunk).unwrap();
            assert!(n > 0, "没收到就断了");
            down.extend_from_slice(&chunk[..n]);
            while let Some((op, body, used)) = whole_frame(&down) {
                ops.push(op);
                if op == 2 {
                    data.extend(body);
                } else {
                    assert_eq!((op, body.as_slice()), (10, &b"ping"[..]), "pong 带回原样的内容");
                }
                down.drain(..used);
            }
        }
        assert_eq!((ops[0], data), (10, pattern(70000, 3)), "先是 pong，然后是中转发的");
        let relay = relay_side.join().unwrap();
        client.write_all(&masked(8, b"")).unwrap();
        client.flush().unwrap();
        assert_eq!(pumping.join().unwrap(), End::Close);
        let mut rest = Vec::new();
        loop {
            match client.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => rest.extend_from_slice(&chunk[..n]),
            }
        }
        assert_eq!(rest, frame(8, b""), "最后是关闭帧");
        drop(relay);
    }
}
