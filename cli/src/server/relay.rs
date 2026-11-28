//! 数据端口：两头（会话宿主、接入的设备或者网页端）各连进来，服务器把密文从一头搬到另一头，里面一个字节都不看。
//! Python 的 `RelayConn`、`Relay`（Py:1233-1569）。
//!
//! 一个线程：`libc::poll` 看着监听的套接字、所有连接和一个叫醒用的套接字（超时 1 秒，每秒清一次过期的）；套接字都不阻塞。
//! 网页端的 WebSocket 那一头（`ws::pump`）是 `UnixStream` 的一头：经 `Handle::adopt` 交进来，和数据端口上接进来的连接一样对待。
//! 线上来的字节（开场白那一行、控制帧）都经 `wire::Cur` 读；开场白的样子是 `wire::relay_line()`。
//!
//! 连上来先说一行开场白（可见的 ASCII），凭票据进来（Py:1243-1247）：
//! - host：会话的控制连接（一个会话一条）。之后是帧：类型(1) 长度(2) 内容——宿主说 ANNOUNCE（各个配对的说明密文）、PING、BYE、
//!   PUSH（一条通知封给一个配对的手机：交给推送网关，push.rs），服务器说 JOIN（有接入方来了：带一个令牌）、PONG
//! - view：接入方。服务器给宿主的控制连接发 JOIN，等宿主拿着令牌另连一条（join）上来，再回 OK
//! - join：宿主为这个接入方连上来的那一条。对上以后两条连接原样对接：服务器只转字节，里面是两头之间端到端加密的内容
//!
//! 和 Python 对着看：
//! - Python 的连接是对象，互相拿着引用（`c.peer`、`x['conn'] is c`）；这里每条连接一个编号（从 1 起，用过的不再用），互相记编号。
//! - Python 用 selectors（Linux 上是 epoll），不想看的连接从里面拿掉；这里每一轮照各条连接现在想看的重新排一遍 poll 的表，
//!   不想看的不放进去（poll 对放进去的连接总会报挂断、出错：放进去就等于还看着它）。
//! - Remote 的锁：Python 里是可重入的，哪里用哪里拿；这里一轮（一条连接的一次动静、一次清理）里用到时才拿、拿了留到这一轮完
//!   （`Lazy`），不会自己等自己，也不会拿着锁等网络。只搬字节的那条路不碰这把锁。
//! - 时限和上限照 Python 开头的常量（`Knobs::DEFAULT`）；测试里换小的。

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod replay;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
pub(in crate::server) mod tests;

use super::omap::OMap;
use super::remote::{self, Remote, Role, Session, Shared};
use super::wire::{self, Blob, Cur};
use super::{
    CTL_FRAME, DIR_BLOB, F_ANNOUNCE, F_BYE, F_JOIN, F_PING, F_PONG, F_PUSH, JOIN_WAIT, PREAMBLE_WAIT, RELAY_CONN, RELAY_CONN_IP,
    RELAY_GRACE, RELAY_IDLE, RELAY_QUEUE,
};
use crate::py::json::{self, Fmt};
use crate::py::obj::V;
use crate::py::{base64, hash, time, R};
use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// 一次最多读这么多（Py:1419）
const CHUNK: usize = 65536;

/// 数据端口的几个上限和时限：平常就是 Python 的那几个常量；测试里换小的（不用真的等 5 秒、90 秒）
#[derive(Clone, Copy, Debug)]
struct Knobs {
    /// 同时几个连接（RELAY_CONN）
    conn_max: usize,
    /// 同一个 IP 同时几个连接（RELAY_CONN_IP）
    conn_ip: usize,
    /// 一头还没转出去的超过这么多，就先不从另一头读（RELAY_QUEUE）
    queue: usize,
    /// 这么久一个字节都没有的连接断开（RELAY_IDLE）
    idle: f64,
    /// 会话的控制连接断了以后，会话留这么久等它重连（RELAY_GRACE）
    grace: f64,
    /// 连上以后这么久还没说完开场白就断开（PREAMBLE_WAIT）
    preamble_wait: f64,
    /// 接入方等宿主那一头连上来最多这么久（JOIN_WAIT）
    join_wait: f64,
    /// 对接的一头断了，另一头还没写完的最多再等这么久（Py:1476）
    drain_wait: f64,
    /// 没有动静时多久醒一次，毫秒（Py:1536）
    poll_ms: i32,
    /// 多久清一次过期的（Py:1564）
    sweep_every: f64,
}

impl Knobs {
    const DEFAULT: Knobs = Knobs {
        conn_max: RELAY_CONN,
        conn_ip: RELAY_CONN_IP,
        queue: RELAY_QUEUE,
        idle: RELAY_IDLE as f64,
        grace: RELAY_GRACE as f64,
        preamble_wait: PREAMBLE_WAIT as f64,
        join_wait: JOIN_WAIT as f64,
        drain_wait: 5.0,
        poll_ms: 1000,
        sweep_every: 1.0,
    };
}

/// 一条连接走到哪一步了（Python 的 `c.state`）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// 还没说完开场白
    Pre,
    /// 接入方：等宿主那一头连上来（这时不看它：回 OK 之前它不该说话）
    Wait,
    /// 和另一条对接着，原样转
    Pipe,
    /// 会话的控制连接
    Ctl,
    /// 对接的另一头断了：还没转给它的先写完再关
    Drain,
    /// 被顶掉的、说了 BYE 的、会话没了的控制连接：关它的时候不动会话
    Gone,
}

/// 数据端口上接进来的是 TCP；网页端交过来的是 socketpair 的一头
enum Sock {
    Tcp(TcpStream),
    Unix(UnixStream),
}

impl Sock {
    fn fd(&self) -> RawFd {
        match self {
            Sock::Tcp(s) => s.as_raw_fd(),
            Sock::Unix(s) => s.as_raw_fd(),
        }
    }

    /// 读一次、写一次（套接字不阻塞：没有、写不动是 WouldBlock）
    fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Sock::Tcp(s) => (&mut &*s).read(buf),
            Sock::Unix(s) => (&mut &*s).read(buf),
        }
    }

    fn send(&self, data: &[u8]) -> io::Result<usize> {
        match self {
            Sock::Tcp(s) => (&mut &*s).write(data),
            Sock::Unix(s) => (&mut &*s).write(data),
        }
    }

    /// 不阻塞；TCP 的不攒小包（Py:1510-1514，后一项不成不要紧）
    fn prepare(&self) -> io::Result<()> {
        match self {
            Sock::Tcp(s) => {
                s.set_nonblocking(true)?;
                let _ = s.set_nodelay(true);
            }
            Sock::Unix(s) => s.set_nonblocking(true)?,
        }
        Ok(())
    }
}

/// 一条连接（Python 的 `RelayConn`）
struct Conn {
    sock: Sock,
    ip: String,
    state: State,
    /// 读进来还没处理的（开场白、控制帧）
    inb: Vec<u8>,
    /// 要写给它、还没写出去的
    out: VecDeque<u8>,
    /// 对接的那一条
    peer: Option<u64>,
    /// 什么时候连上的（单调时钟）；改成 Drain 时重新记
    born: f64,
    /// 最后一次有字节是什么时候
    seen: f64,
    /// 是哪个会话的
    sid: Option<String>,
    /// 接入方的连接、和它对接的那一条：是哪个配对的（配对没了就断开）
    pid: Option<String>,
    /// 控制连接：它那张票据的密钥（宿主为接入方另连的那一条要证明有它）
    key: Option<Vec<u8>>,
    /// 等着的接入方：早先备好的那句 OK
    ok: Option<Vec<u8>>,
    /// 对面写不动：先不读这一头
    paused: bool,
}

impl Conn {
    /// 现在想看它的什么（Py:1261-1262）：停着的、等着的不读；有没写完的才看能不能写。0 是不看它
    fn wants(&self) -> libc::c_short {
        let read = if self.paused || self.state == State::Wait { 0 } else { libc::POLLIN };
        read | if self.out.is_empty() { 0 } else { libc::POLLOUT }
    }
}

/// 用到 Remote 时才拿它的锁，拿了留到这一轮完
struct Lazy<'a> {
    sh: &'a Shared,
    held: Option<MutexGuard<'a, Remote>>,
}

impl<'a> Lazy<'a> {
    fn new(sh: &'a Shared) -> Lazy<'a> {
        Lazy { sh, held: None }
    }

    fn get(&mut self) -> &mut Remote {
        let sh = self.sh;
        self.held.get_or_insert_with(|| sh.lock())
    }
}

/// 测试里给中转的线程递的话
#[cfg(test)]
#[derive(Default)]
struct Probe {
    /// 停下（线程退出、所有连接关掉）
    stop: bool,
    /// 换一套时限
    knobs: Option<Knobs>,
    /// 现在几条连接（中转的线程每一轮记一次）
    conns: usize,
}

/// 中转的线程在外面的把手
pub struct Handle {
    /// 等着交给中转的线程的连接（网页端的 WebSocket 那一头）和它的 IP
    adopted: Mutex<Vec<(UnixStream, String)>>,
    /// 叫醒中转的线程用的那一对的一头
    wake_w: UnixStream,
    #[cfg(test)]
    probe: Mutex<Probe>,
}

impl Handle {
    /// 把一条字节流交给中转（Py:1497-1504）：放进队列、往叫醒用的套接字里写一个字节。之后和数据端口上接进来的一样——先说开场白，
    /// 同样的限额（中转那边连接太多时直接关掉它）。ip 是浏览器的地址
    pub fn adopt(&self, sock: UnixStream, ip: String) {
        self.adopted.lock().unwrap_or_else(PoisonError::into_inner).push((sock, ip));
        self.wake();
    }

    fn wake(&self) {
        let mut w = &self.wake_w;
        let _ = w.write(&[0]); // 叫醒用的那一对满了：已经有没读走的，照样会醒
    }
}

/// 起中转的线程（Py:1242-1259、1530-1569）。l 是已经绑好、在听的数据端口。线程起不来是 Err
pub fn start(l: TcpListener, sh: Arc<Shared>) -> R<Arc<Handle>> {
    start_with(l, sh, Knobs::DEFAULT)
}

fn start_with(l: TcpListener, sh: Arc<Shared>, knobs: Knobs) -> R<Arc<Handle>> {
    l.set_nonblocking(true)?;
    let (wake_r, wake_w) = UnixStream::pair()?;
    wake_r.set_nonblocking(true)?;
    wake_w.set_nonblocking(true)?;
    let handle = Arc::new(Handle {
        adopted: Mutex::new(Vec::new()),
        wake_w,
        #[cfg(test)]
        probe: Mutex::new(Probe::default()),
    });
    let relay = Relay {
        lsock: l,
        handle: handle.clone(),
        wake_r,
        conns: HashMap::new(),
        last_id: 0,
        per_ip: HashMap::new(),
        waiting: HashMap::new(),
        knobs,
        scratch: vec![0; CHUNK],
    };
    std::thread::Builder::new().name("relay".to_string()).spawn(move || relay.run(&sh))?;
    Ok(handle)
}

/// `b64u(hmac256(key, 各段接起来))`：认证码的写法（Py:944-949）
fn mac(key: &[u8], parts: &[&[u8]]) -> String {
    base64::urlsafe_b64encode(&hash::hmac_sha256(key, &parts.concat())).trim_end_matches('=').to_string()
}

/// 开场白对了以后回的那一行：服务器也证明自己认得这把密钥
fn ok_line(key: &[u8], ident: &str, cn: &str) -> Vec<u8> {
    format!("CCA-RELAY/1 OK {}\n", mac(key, &[b"s", ident.as_bytes(), cn.as_bytes()])).into_bytes()
}

/// 控制连接上的一帧：类型(1) 长度(2，高位在前) 内容（Py:1317-1318）
fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![kind];
    out.extend_from_slice(&u16::try_from(payload.len()).unwrap_or(u16::MAX).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// 这条线程管着所有连接：哪一步出了没料到的错，都只断开那一条、记一行（只记是哪种错，不记内容），接着转别的（Py:1520-1523）
fn oops(what: &str) {
    crate::err!("cca-presence: 中转{what}出错（OSError），接着运行");
}

fn pollfd(fd: RawFd, events: libc::c_short) -> libc::pollfd {
    libc::pollfd { fd, events, revents: 0 }
}

/// 中转的线程自己的东西（Python 的 `Relay`）
struct Relay {
    lsock: TcpListener,
    handle: Arc<Handle>,
    wake_r: UnixStream,
    /// 连接的编号 → 连接
    conns: HashMap<u64, Conn>,
    /// 上一条连接的编号
    last_id: u64,
    /// 每个 IP 现在几条连接
    per_ip: HashMap<String, usize>,
    /// 令牌 → (等着的接入方, 宿主控制连接的票据密钥)
    waiting: HashMap<String, (u64, Vec<u8>)>,
    knobs: Knobs,
    /// 读连接用的那一块内存
    scratch: Vec<u8>,
}

impl Relay {
    /// Py:1530-1569
    fn run(mut self, sh: &Shared) {
        let mut swept = 0.0;
        let (mut fds, mut ids): (Vec<libc::pollfd>, Vec<u64>) = (Vec::new(), Vec::new());
        loop {
            #[cfg(test)]
            if self.probed() {
                return;
            }
            fds.clear();
            ids.clear();
            fds.push(pollfd(self.lsock.as_raw_fd(), libc::POLLIN));
            fds.push(pollfd(self.wake_r.as_raw_fd(), libc::POLLIN));
            for (id, c) in &self.conns {
                let events = c.wants();
                if events != 0 {
                    fds.push(pollfd(c.sock.fd(), events));
                    ids.push(*id);
                }
            }
            // SAFETY: fds 是有效的数组，长度就是传进去的那个；poll 只往各项的 revents 里写
            let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, self.knobs.poll_ms) };
            if n < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                oops("等连接的动静时");
                time::sleep(0.2);
                continue;
            }
            if n > 0 {
                for (i, fd) in fds.iter().enumerate() {
                    if fd.revents == 0 {
                        continue;
                    }
                    let now = time::monotonic();
                    match (i, ids.get(i.wrapping_sub(2))) {
                        (0, _) => self.accept(now),
                        (1, _) => self.woken(now),
                        (_, Some(id)) => self.ready(sh, *id, fd.events, fd.revents, now),
                        _ => {}
                    }
                }
            }
            let now = time::monotonic();
            if now - swept >= self.knobs.sweep_every {
                swept = now;
                self.sweep(&mut Lazy::new(sh), now);
            }
        }
    }

    /// 测试递了话来：换时限；要停下是 true
    #[cfg(test)]
    fn probed(&mut self) -> bool {
        let mut probe = self.handle.probe.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(knobs) = probe.knobs.take() {
            self.knobs = knobs;
        }
        probe.conns = self.conns.len();
        probe.stop
    }

    /// 一条连接有动静。和 Python 的 selectors（epoll 那一种）一样算：出错、挂断时读和写都算有动静，再照当初想看的筛一遍；先写后读
    /// （Py:1557-1562）
    fn ready(&mut self, sh: &Shared, id: u64, wanted: libc::c_short, got: libc::c_short, now: f64) {
        let mut lz = Lazy::new(sh);
        if wanted & libc::POLLOUT != 0 && got & !libc::POLLIN != 0 && self.conns.contains_key(&id) {
            self.writable(&mut lz, id, now);
        }
        if wanted & libc::POLLIN != 0
            && got & !libc::POLLOUT != 0
            && self.conns.contains_key(&id)
            && self.readable(&mut lz, id, now).is_err()
        {
            oops("的一条连接");
            self.close(&mut lz, id, None, now);
        }
    }

    /// Py:1490-1495：一次接一条（还有的话下一轮接着接）
    fn accept(&mut self, now: f64) {
        if let Ok((sock, addr)) = self.lsock.accept() {
            self.admit(Sock::Tcp(sock), addr.ip().to_string(), now);
        }
    }

    /// 别的线程交了连接过来（Py:1548-1556）
    fn woken(&mut self, now: f64) {
        let mut buf = [0u8; 4096];
        let mut r = &self.wake_r;
        let _ = r.read(&mut buf);
        let handed = std::mem::take(&mut *self.handle.adopted.lock().unwrap_or_else(PoisonError::into_inner));
        for (sock, ip) in handed {
            self.admit(Sock::Unix(sock), ip, now);
        }
    }

    /// 收下一条连接（Py:1506-1518）；连接太多、这个 IP 的太多：直接关掉
    fn admit(&mut self, sock: Sock, ip: String, now: f64) {
        if self.conns.len() >= self.knobs.conn_max || self.per_ip.get(&ip).copied().unwrap_or(0) >= self.knobs.conn_ip {
            return;
        }
        if sock.prepare().is_err() {
            return oops("的一条连接");
        }
        self.last_id += 1;
        *self.per_ip.entry(ip.clone()).or_insert(0) += 1;
        let conn = Conn {
            sock,
            ip,
            state: State::Pre,
            inb: Vec::new(),
            out: VecDeque::new(),
            peer: None,
            born: now,
            seen: now,
            sid: None,
            pid: None,
            key: None,
            ok: None,
            paused: false,
        };
        self.conns.insert(self.last_id, conn);
    }

    /// 排一段要写给它的（Py:1273-1277）。控制连接上只发不收（一个 PING 换一个 PONG）：不替它攒着
    fn send(&mut self, lz: &mut Lazy, id: u64, data: &[u8], now: f64) {
        let Some(c) = self.conns.get_mut(&id) else { return };
        c.out.extend(data);
        if c.state == State::Ctl && c.out.len() > self.knobs.queue {
            self.close(lz, id, None, now);
        }
    }

    /// 关一条连接（Py:1279-1315）。why：还没进来的（没说完开场白、等着宿主的）先回一行为什么
    fn close(&mut self, lz: &mut Lazy, id: u64, why: Option<&str>, now: f64) {
        let Some(c) = self.conns.remove(&id) else { return };
        if let (Some(why), State::Pre | State::Wait) = (why, c.state) {
            let _ = c.sock.send(format!("CCA-RELAY/1 ERR {why}\n").as_bytes());
        }
        match self.per_ip.get_mut(&c.ip) {
            Some(n) if *n > 1 => *n -= 1,
            _ => {
                self.per_ip.remove(&c.ip);
            }
        }
        if let (Some(sid), State::Ctl | State::Pipe) = (&c.sid, c.state) {
            if let Some(x) = lz.get().sessions.get_mut(sid) {
                if c.state == State::Pipe {
                    // 对接着的一对断了一头：这一处接入算结束（另一头下面改成 Drain，不会再算一次）
                    x.viewers = x.viewers.saturating_sub(1);
                } else if x.conn == Some(id) {
                    (x.conn, x.lost) = (None, Some(now));
                }
            }
        }
        self.waiting.retain(|_, w| w.0 != id);
        let Some(peer) = c.peer else { return };
        drop(c); // 先关掉这一条，再管另一头
                 // 另一头：还没转给它的先写完再关（会话结束时宿主最后说的那一句“结束了”不能丢），最多再等 5 秒
        let flushing = match self.conns.get_mut(&peer) {
            Some(p) => {
                (p.peer, p.state, p.paused, p.born) = (None, State::Drain, true, now);
                !p.out.is_empty()
            }
            None => false,
        };
        if !flushing {
            self.close(lz, peer, None, now);
        }
    }

    /// 开场白那一行（Py:1320-1375）。Err：取不到随机数（Python 那边是没料到的错：记一行、断开这一条）
    fn preamble(&mut self, lz: &mut Lazy, id: u64, line: &[u8], now: f64) -> R<()> {
        let Some(m) = std::str::from_utf8(line).ok().and_then(|text| wire::relay_line().match_(text)) else {
            self.close(lz, id, Some("bad"), now);
            return Ok(());
        };
        let (role, ident, cn, given) = (m.g(1), m.g(2), m.g(3), m.g(4));
        let proven = |key: &[u8], parts: &[&[u8]]| hash::compare_digest(given.as_bytes(), mac(key, parts).as_bytes());
        if role == "join" {
            // 宿主为一个接入方另连的那一条：令牌是 JOIN 帧里给的，要证明有控制连接那张票据的密钥
            // 证明不了的不算用掉令牌（光知道令牌的人顶不掉等着的接入方）
            let known = self.waiting.get(ident).is_some_and(|w| proven(&w.1, &[b"j", ident.as_bytes(), cn.as_bytes()]));
            let waiter = known.then(|| self.waiting.remove(ident)).flatten();
            // 等着的接入方一定还在：它关的时候 waiting 里它的那几项就拿掉了
            let seat = waiter.and_then(|(viewer, key)| self.conns.get_mut(&viewer).map(|v| (viewer, key, v)));
            let Some((viewer, key, v)) = seat else {
                self.close(lz, id, Some("ticket"), now);
                return Ok(());
            };
            (v.peer, v.state) = (Some(id), State::Pipe);
            let (sid, pid, ready) = (v.sid.clone(), v.pid.clone(), v.ok.take().unwrap_or_default());
            if let Some(x) = sid.as_deref().and_then(|sid| lz.get().sessions.get_mut(sid)) {
                x.viewers = x.viewers.saturating_add(1);
            }
            if let Some(c) = self.conns.get_mut(&id) {
                (c.peer, c.state, c.sid, c.pid) = (Some(viewer), State::Pipe, sid, pid);
            }
            self.send(lz, id, &ok_line(&key, ident, cn), now);
            self.send(lz, viewer, &ready, now); // 接入方那句 OK 是早先备好的
            return Ok(());
        }
        let (role, lead) = if role == "host" { (Role::Host, &b"chost"[..]) } else { (Role::View, &b"cview"[..]) };
        let wall = time::time();
        let Some(k) = lz.get().take_ticket(ident, role, wall, |key| proven(key, &[lead, ident.as_bytes(), cn.as_bytes()])) else {
            self.close(lz, id, Some("ticket"), now);
            return Ok(());
        };
        let ok = ok_line(&k.key, ident, cn);
        if let Some(c) = self.conns.get_mut(&id) {
            c.sid = Some(k.sid.clone());
        }
        if role == Role::Host {
            // 会话的控制连接：新开一个会话，或者接回原来的（顶掉旧的那一条）
            let r = lz.get();
            let cap = r.lim.sessions_dev;
            let mine = r.sessions.values().filter(|x| x.fp == k.fp).count();
            let old = match r.sessions.get_mut(&k.sid) {
                Some(x) if x.fp != k.fp => Err("taken"),
                Some(x) => {
                    let old = x.conn.replace(id);
                    x.pids.clone_from(&k.pids);
                    (x.lost, x.heard) = (None, wall);
                    x.blobs.retain(|p, _| k.pids.iter().any(|q| q == p));
                    Ok(old)
                }
                // 换票据时数过一次；攒着票据一起连上来的在这里拦住
                None if mine >= cap => Err("sessions"),
                None => {
                    let (fp, pids) = (k.fp.clone(), k.pids.clone());
                    r.sessions.insert(
                        k.sid.clone(),
                        Session { fp, pids, blobs: OMap::new(), conn: Some(id), lost: None, heard: wall, viewers: 0, since: wall },
                    );
                    Ok(None)
                }
            };
            let old = match old {
                Ok(old) => old,
                Err(why) => {
                    self.close(lz, id, Some(why), now);
                    return Ok(());
                }
            };
            if let Some(c) = self.conns.get_mut(&id) {
                (c.state, c.key) = (State::Ctl, Some(k.key));
            }
            self.send(lz, id, &ok, now);
            if let Some(old) = old.filter(|old| *old != id) {
                // 被新的控制连接顶掉了：关它的时候不动会话
                if let Some(o) = self.conns.get_mut(&old) {
                    o.state = State::Gone;
                }
                self.close(lz, old, None, now);
            }
            return Ok(());
        }
        // 接入方：会话得开着、控制连接连着、给这个配对看
        let pid = k.pid.unwrap_or_default();
        let host = lz.get().sessions.get(&k.sid).filter(|x| x.pids.contains(&pid)).and_then(|x| x.conn);
        let host_key = host.and_then(|h| self.conns.get(&h)).and_then(|h| h.key.clone());
        let (Some(host), Some(host_key)) = (host, host_key) else {
            self.close(lz, id, Some("nohost"), now);
            return Ok(());
        };
        let token = hash::to_hex(&remote::random(16)?);
        self.waiting.insert(token.clone(), (id, host_key));
        let join = json::dumps(&V::Dict(crate::dict! { "token" => &token, "pid" => &pid }), Fmt::new().compact());
        if let Some(c) = self.conns.get_mut(&id) {
            (c.state, c.ok, c.pid) = (State::Wait, Some(ok), Some(pid));
        }
        self.send(lz, host, &frame(F_JOIN, join.as_bytes()), now);
        Ok(())
    }

    /// 宿主的控制连接上的帧：读到的这一块里有几帧就处理几帧，处理过的最后一起从缓冲区里拿掉（Py:1377-1391）。一帧还没到齐时
    /// 缓冲区原样放回去，不抄一遍（一个字节一个字节地送一大帧的，不能让这条线程每次都抄几十 KB）
    fn control(&mut self, lz: &mut Lazy, id: u64, now: f64) {
        let Some(c) = self.conns.get_mut(&id) else { return };
        let mut buf = std::mem::take(&mut c.inb);
        let mut cur = Cur::new(&buf);
        while self.conns.contains_key(&id) {
            let mut next = Cur::new(cur.rest());
            let (Some(kind), Some(n)) = (next.u8(), next.u16()) else { break };
            if usize::from(n) > CTL_FRAME {
                self.close(lz, id, None, now);
                break;
            }
            let Some(payload) = next.take(usize::from(n)) else { break };
            cur = next;
            self.control_frame(lz, id, kind, payload, now);
        }
        let used = buf.len().saturating_sub(cur.rest().len());
        if let Some(c) = self.conns.get_mut(&id) {
            buf.drain(..used);
            c.inb = buf;
        }
    }

    /// 控制连接上的一帧。不对的就把这条连接关掉（control 看到它不在了就停）（Py:1393-1415）
    fn control_frame(&mut self, lz: &mut Lazy, id: u64, kind: u8, payload: &[u8], now: f64) {
        let Some(sid) = self.conns.get(&id).and_then(|c| c.sid.clone()) else { return };
        // 宿主还在说话（PING、ANNOUNCE……）：记下是什么时候（relay_list 照它说这个会话有没有回应）
        if let Some(x) = lz.get().sessions.get_mut(&sid).filter(|x| x.conn == Some(id)) {
            x.heard = time::time();
        }
        match kind {
            F_PING => self.send(lz, id, &frame(F_PONG, payload.get(..16).unwrap_or(payload)), now),
            F_BYE => {
                lz.get().sessions.remove(&sid);
                if let Some(c) = self.conns.get_mut(&id) {
                    c.state = State::Gone;
                }
                self.close(lz, id, None, now);
            }
            F_ANNOUNCE => {
                // 各个配对一份说明的密文：整份换掉。不是这个会话给看的配对的不要；有一份不合格就断开
                let blobs = std::str::from_utf8(payload).ok().and_then(|text| json::loads(text).ok()).and_then(|v| v.as_dict());
                let x = lz.get().sessions.get_mut(&sid).filter(|x| x.conn == Some(id));
                let good = blobs.zip(x).and_then(|(blobs, x)| {
                    let mut good: OMap<Blob> = OMap::new();
                    for (pid, b) in blobs.items() {
                        if x.pids.iter().any(|p| **p == *pid) {
                            good.insert(&*pid, wire::parse_blob(&b, DIR_BLOB)?);
                        }
                    }
                    x.blobs = good;
                    Some(())
                });
                if good.is_none() {
                    self.close(lz, id, None, now);
                }
            }
            // 一条通知封给一个配对的手机（push.rs）：那台手机登记过推送才交给推送网关。样子不对的、对不上的不理，不断开（只是一条
            // 通知；以后的版本可能多带东西）
            F_PUSH => {
                let r = lz.get();
                if let Some(p) = r.pusher.as_ref() {
                    if let Some(job) = r.push_job(&sid, id, payload) {
                        p.send(job);
                    }
                }
            }
            _ => {} // 不认识的帧不理
        }
    }

    /// 能读了（Py:1417-1451）
    fn readable(&mut self, lz: &mut Lazy, id: u64, now: f64) -> R<()> {
        let mut buf = std::mem::take(&mut self.scratch);
        let got = self.conns.get(&id).map(|c| c.sock.recv(&mut buf));
        let done = match got {
            Some(Ok(n)) if n > 0 => self.received(lz, id, buf.get(..n).unwrap_or_default(), now),
            Some(Err(e)) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => Ok(()),
            Some(_) => {
                self.close(lz, id, None, now); // 对面关了，或者出错了
                Ok(())
            }
            None => Ok(()),
        };
        self.scratch = buf;
        done
    }

    /// 读到了一块
    fn received(&mut self, lz: &mut Lazy, id: u64, data: &[u8], now: f64) -> R<()> {
        let Some(c) = self.conns.get_mut(&id) else { return Ok(()) };
        c.seen = now;
        if c.state == State::Pipe {
            // 原样转给对接的那一条。对面写不动：先不读这一头（让发的那一头自己慢下来）
            let queue = self.knobs.queue;
            let over = c.peer.and_then(|p| self.conns.get_mut(&p)).map(|p| {
                p.seen = now;
                p.out.extend(data);
                p.out.len() > queue
            });
            match (over, self.conns.get_mut(&id)) {
                (Some(true), Some(c)) => c.paused = true,
                (None, _) => self.close(lz, id, None, now),
                _ => {}
            }
            return Ok(());
        }
        c.inb.extend_from_slice(data);
        if c.state == State::Pre {
            let Some(end) = c.inb.iter().position(|b| *b == b'\n') else {
                if c.inb.len() > 256 {
                    self.close(lz, id, Some("bad"), now);
                }
                return Ok(());
            };
            let rest = c.inb.split_off(end + 1);
            let mut line = std::mem::replace(&mut c.inb, rest);
            line.pop();
            self.preamble(lz, id, &line, now)?;
        }
        match self.conns.get(&id).map(|c| (c.state, c.inb.is_empty())) {
            Some((State::Ctl, _)) => self.control(lz, id, now),
            Some((State::Wait | State::Pipe, false)) => self.close(lz, id, None, now), // 回 OK 之前不该有别的话
            _ => {}
        }
        Ok(())
    }

    /// 能写了（Py:1453-1468）
    fn writable(&mut self, lz: &mut Lazy, id: u64, now: f64) {
        let Some(c) = self.conns.get_mut(&id) else { return };
        // 没写出去的在一个环形的缓冲区里：是前后两段。前一段全写出去了才接着写后一段（它写不动、出错都留到下一轮）
        let (head, tail) = c.out.as_slices();
        let n = match c.sock.send(head) {
            Ok(n) if n == head.len() && !tail.is_empty() => n.saturating_add(c.sock.send(tail).unwrap_or(0)),
            Ok(n) => n,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => return,
            Err(_) => return self.close(lz, id, None, now),
        };
        c.out.drain(..n.min(c.out.len()));
        if c.state == State::Drain && c.out.is_empty() {
            return self.close(lz, id, None, now);
        }
        // 写出去不少了：对面那一头接着读
        let (left, peer) = (c.out.len(), c.peer);
        if let Some(p) = peer.and_then(|p| self.conns.get_mut(&p)) {
            if p.paused && left < self.knobs.queue / 2 {
                p.paused = false;
            }
        }
    }

    /// 清过期的（Py:1470-1488）
    fn sweep(&mut self, lz: &mut Lazy, now: f64) {
        let k = self.knobs;
        let ids: Vec<u64> = self.conns.keys().copied().collect();
        for id in ids {
            let Some(c) = self.conns.get(&id) else { continue };
            let why = match c.state {
                State::Pre if now - c.born > k.preamble_wait => Some("slow"),
                State::Wait if now - c.born > k.preamble_wait + k.join_wait => Some("nohost"),
                State::Drain if now - c.born > k.drain_wait => None,
                State::Ctl | State::Pipe if now - c.seen > k.idle => None,
                _ => continue,
            };
            self.close(lz, id, why, now);
        }
        // 控制连接断了太久的会话不留
        let r = lz.get();
        r.sessions.retain(|_, x| x.conn.is_some() || now - x.lost.filter(|lost| *lost != 0.0).unwrap_or(now) <= k.grace);
        // 配对取消了、设备被移除了：会话不在了的控制连接断开，那个配对正接着的（和等着接的）也断开
        let conn_of = |c: &Conn| c.sid.as_deref().and_then(|sid| r.sessions.get(sid)).and_then(|x| x.conn);
        let gone: Vec<u64> =
            self.conns.iter().filter(|(id, c)| c.state == State::Ctl && conn_of(c) != Some(**id)).map(|(id, _)| *id).collect();
        for id in gone {
            if let Some(c) = self.conns.get_mut(&id) {
                c.state = State::Gone;
            }
            self.close(lz, id, None, now);
        }
        let r = lz.get();
        let cut: Vec<u64> = self
            .conns
            .iter()
            .filter(|(_, c)| matches!(c.state, State::Pipe | State::Wait) && !c.pid.as_deref().is_some_and(|pid| r.has_edge(pid)))
            .map(|(id, _)| *id)
            .collect();
        for id in cut {
            self.close(lz, id, None, now);
        }
    }
}
