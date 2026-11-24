//! 接连接、给每个连接起一个线程、在线程里读一个请求回一个回复：Python 的 `Server`（Py:2032-2129）和 `Handler` 外面那一圈
//! （`handle_one_request`：读到什么算一个请求在 http.rs，请求怎么处理在 handler.rs）。
//!
//! - 一个监听一份名额（`Slots`）：同时处理 64 个连接、同一个 IP 8 个（--behind-proxy 时连过来的都是代理，只看总数）。
//!   满了的连接直接断开，不排队（Py:2059-2086）
//! - 一个连接一个线程。期限在线程里自己守（`http::DeadlineStream`）：整条连接从接进来起最多 15 秒，握手时一次读写最多等
//!   5 秒，之后 15 秒；一个字节一个字节慢慢发也占不住。Python 是另起一个定时器到点把连接断掉（Py:2098-2129）
//! - TLS 握手在处理这个连接的线程里做；握不成不回、不说话，直接断开。网页端的证书还没有（`CertCtx::get` 是 None）：不握手，
//!   直接断开
//! - 一条连接只处理一个请求（HTTP/1.0），回完就关
//! - 读写时等过了头：不回、不说话（Python 的 TimeoutError）。别的读写的错（对方把连接重置了……）在日志里说一句
//!   `cca-presence: 处理请求出错：…`（Py:2095-2096）
//! - 升级成 WebSocket 的（`Outcome::Upgrade`）：回 101（`ws::switching`），处理请求的名额现在就还、不再受 15 秒的期限管
//!   （Py:2053-2057），连接交给 WebSocket 的泵（`ws::pump`：这个线程接着搬，中转那一头经 `relay::Handle::adopt` 交给中转的
//!   线程）；不管怎么结束，网页端的那个名额都要还（`Web::free`）
//!
//! 和 Python 不一样的：回复是一次写出去的（Python 是头和正文各写一次）；连接到了期限就不再处理（Python 把连接断掉以后，读到一半
//! 的请求还会被当成一个请求处理一下，回不出去，日志里多一句 Broken pipe）；TLS 的连接关之前说一声 close_notify（Python 不说）。

use super::handler::{self, lock, Outcome};
use super::http::{self, Conn, DeadlineStream, Head, Method, Reader};
use super::limits::Slots;
use super::store::say;
use super::tls::CertCtx;
use super::web::Web;
use super::{ws, Ctx, CONN_DEADLINE, HANDSHAKE_TIMEOUT};
use crate::py::fail::os_error_text;
use crate::py::time;
use rustls::ServerConfig;
use std::io::{self, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// 这个监听说的是什么
pub enum Tls {
    /// 只说 HTTP（--behind-proxy：HTTPS 由前面的代理做；本机试网页端的那个监听）
    Plain,
    /// 一张不换的证书（--cert、--key）
    Fixed(Arc<ServerConfig>),
    /// 网页端的那张证书：会换，还没有时这个端口上的连接直接断开
    Reload(Arc<CertCtx>),
}

/// 各个期限（测试里换短的）
#[derive(Clone, Copy, Debug)]
struct Timing {
    /// 整条连接从接进来起最多多久（CONN_DEADLINE）
    deadline: Duration,
    /// 握手时一次读写最多等多久（HANDSHAKE_TIMEOUT）
    handshake: Duration,
    /// 握手以后一次读写最多等多久（Handler.timeout，Py:1803）
    request: Duration,
    /// 成了 WebSocket 以后：读一次、写一帧最多等多久（WS_STALL），两头都没动静多久就断开（WS_IDLE）
    ws: ws::Times,
}

fn secs(n: i64) -> Duration {
    Duration::from_secs(u64::try_from(n).unwrap_or_default())
}

impl Timing {
    fn python() -> Timing {
        Timing { deadline: secs(CONN_DEADLINE), handshake: secs(HANDSHAKE_TIMEOUT), request: secs(15), ws: ws::Times::REAL }
    }
}

/// 一个监听（Python 的一个 Server）：两个监听（原来的端口、网页端另开的端口）各有一份名额，别的都在共用的 `Ctx` 里
pub struct Front {
    tls: Tls,
    /// 在代理后面：限流照代理报的地址，同一个 IP 的连接数不限
    proxied: bool,
    ctx: Arc<Ctx>,
    slots: Mutex<Slots>,
    timing: Timing,
}

/// 占着的一个处理请求的名额：线程结束（或者连接成了 WebSocket）时还
struct Slot<'a> {
    slots: &'a Mutex<Slots>,
    ip: &'a str,
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        lock(self.slots).free(self.ip);
    }
}

/// 占着的一个 WebSocket 的名额（`Web::take` 在 handler 里占的）：这条连接不管怎么结束都要还（Py:1886-1887）
struct WsSlot<'a> {
    web: Option<&'a Web>,
    ip: &'a str,
}

impl Drop for WsSlot<'_> {
    fn drop(&mut self) {
        if let Some(web) = self.web {
            web.free(self.ip);
        }
    }
}

/// 一个请求处理完了，这条连接接下来
enum Next {
    /// 回过了（或者不用回）：关掉
    Done,
    /// 要升级成 WebSocket：accept 是 Sec-WebSocket-Accept 的值，ip 是占网页端名额用的那个地址
    Upgrade { accept: String, ip: String },
}

/// 在 l 上一直接连接（不返回）。tls：这个监听说的是什么；proxied：是不是在代理后面；ctx：两个监听共用的那一份
pub fn serve(l: TcpListener, tls: Tls, proxied: bool, ctx: Arc<Ctx>) -> ! {
    let front = Front::new(tls, proxied, ctx);
    loop {
        match l.accept() {
            Ok((sock, peer)) => front.accept(sock, peer),
            // 对方在排队的时候就走了、被信号打断：接着接
            Err(e) if matches!(e.kind(), io::ErrorKind::ConnectionAborted | io::ErrorKind::Interrupted) => {}
            // 文件描述符用完了这一类：等一下再接，别空转
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn send(conn: &mut Conn, bytes: &[u8]) -> io::Result<()> {
    conn.write_all(bytes)?;
    conn.flush() // TLS：write 只是放进缓冲，flush 才真的写出去、才知道写不写得了
}

impl Front {
    pub fn new(tls: Tls, proxied: bool, ctx: Arc<Ctx>) -> Arc<Front> {
        Arc::new(Front { tls, proxied, ctx, slots: Mutex::new(Slots::for_listener()), timing: Timing::python() })
    }

    /// 接进来一个连接：有名额就起一个线程处理它，没有就直接断开，不排队（Py:2078-2086）
    pub fn accept(self: &Arc<Front>, sock: TcpStream, peer: SocketAddr) {
        let accepted = Instant::now();
        let peer = peer.ip().to_canonical(); // 听 IPv6 时连过来的 IPv4 地址（::ffff:a.b.c.d）照 IPv4 的算
        let ip = peer.to_string();
        if !lock(&self.slots).take(&ip, self.proxied) {
            let _ = sock.shutdown(Shutdown::Write);
            return;
        }
        let (me, key) = (Arc::clone(self), ip.clone());
        let spawned = thread::Builder::new().name("conn".into()).spawn(move || {
            let slot = Slot { slots: &me.slots, ip: &key };
            me.connection(sock, peer, slot, accepted);
        });
        if let Err(e) = spawned {
            // 线程起不来（sock 跟着没起成的线程一起关掉了）：名额还回去，说一句
            lock(&self.slots).free(&ip);
            say(format!("cca-presence: 处理请求出错：{}", os_error_text(&e)));
        }
    }

    /// 处理一条连接（在它自己的线程里）：握手、读一个请求、回、关（Py:2098-2129）。accepted 是接进来的时间：期限从它算起
    fn connection(&self, sock: TcpStream, peer: IpAddr, slot: Slot<'_>, accepted: Instant) {
        let _ = sock.set_nonblocking(false); // 有的系统上接进来的连接跟着监听的那个算不算阻塞的
        let stream = DeadlineStream::new(sock, Some(accepted + self.timing.deadline), self.timing.handshake);
        let config = match &self.tls {
            Tls::Plain => None,
            Tls::Fixed(config) => Some(Arc::clone(config)),
            Tls::Reload(certs) => match certs.get(time::monotonic()) {
                Some(config) => Some(config),
                None => return, // 网页端的证书还没有：不握手，直接断开
            },
        };
        let mut conn = match config {
            None => Conn::Plain(stream),
            Some(config) => match Conn::tls(stream, config) {
                Ok(conn) => conn,
                Err(_) => return, // 握不成（不是 TLS、对方不认这张证书、太慢）：不回、不说话
            },
        };
        conn.stream_mut().set_per_op(self.timing.request);
        let mut reader = Reader::new(conn);
        match self.exchange(&mut reader, peer) {
            Ok(Next::Done) => {}
            Ok(Next::Upgrade { accept, ip }) => return self.upgraded(reader, slot, &accept, &ip),
            Err(e) if http::is_timeout(&e) => {}
            Err(e) => say(format!("cca-presence: 处理请求出错：{}", os_error_text(&e))),
        }
        reader.into_parts().0.finish();
    }

    /// 读一个请求、处理、回（`handle_one_request`）。读写出错是 Err
    fn exchange(&self, r: &mut Reader<Conn>, peer: IpAddr) -> io::Result<Next> {
        let (mut req, v09) = match http::read_head(r, time::time())? {
            Head::Req { req, v09 } => (req, v09),
            Head::Reply(reply) => return send(r.io(), &reply).map(|()| Next::Done),
            Head::Close => return Ok(Next::Done),
        };
        // 先过读正文之前的那几步（限流、Content-Length），过了才去读那么多个字节（Py:1903-1913）
        let out = match handler::admit(&self.ctx, peer, self.proxied, &req) {
            Ok(n) => {
                if req.method == Method::Post {
                    req.body = r.read(n)?;
                }
                handler::answer(&self.ctx, peer, self.proxied, &req, n)
            }
            Err(out) => out,
        };
        let reply = match out {
            Outcome::Json(code, obj) => http::json_reply(code, &obj, v09, time::time()),
            Outcome::Raw { code, headers, body } => http::raw_reply(code, &headers, &body, v09, time::time()),
            Outcome::Upgrade { accept, ip } => return Ok(Next::Upgrade { accept, ip }),
            Outcome::Close => return Ok(Next::Done),
        };
        send(r.io(), &reply).map(|()| Next::Done)
    }

    /// 这条连接成了 WebSocket（Py:1871-1888）：回 101；它要开很久，处理请求的名额现在就还、不再受 15 秒的期限管
    /// （`long_lived`，Py:2053-2057；WebSocket 有自己的上限：`Web::take`）；然后交给 WebSocket 的泵
    fn upgraded(&self, mut reader: Reader<Conn>, slot: Slot<'_>, accept: &str, ip: &str) {
        let _ws = WsSlot { web: self.ctx.web.as_ref(), ip };
        if send(reader.io(), &ws::switching(accept)).is_err() {
            return;
        }
        drop(slot);
        // 读请求头时可能多读了几个字节进缓冲（浏览器等到回了 101 才发帧，一般没有）：取出来一起交过去
        let (mut conn, early) = reader.into_parts();
        conn.stream_mut().clear_deadline();
        // 浏览器那一头和中转之间来回搬（Py:1883）：字节流的一头交给中转的线程，之后和数据端口上接进来的连接一样对待。
        // 没有中转（开了网页端就一定有，启动时查过）时那一头直接关掉：泵马上知道中转那头没了，发个关闭帧就完
        let relay = self.ctx.relay.as_deref();
        let adopt = |sock| {
            if let Some(relay) = relay {
                relay.adopt(sock, ip.to_string());
            }
        };
        ws::pump(&mut conn, &early, adopt, self.timing.ws);
        conn.finish();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests;
