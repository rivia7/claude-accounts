//! 宿主那头的转接（`remote-link <会话编号>`）：会话宿主起的一个小进程，把配对过的机器经服务器接到这个会话上。
//! 联网的事都在这里，宿主自己不碰网络——这个进程卡住、崩了，会话和这台机器上的接入都不受影响（宿主过一会儿再起一个）。
//!
//! - 和服务器之间一条控制连接（`relay_host` 拿票据进数据端口）：把会话的说明封给各个允许接入的配对（ANNOUNCE），
//!   隔一阵说一声（PING），断了按退避重连；
//! - 服务器说有接入方来了（JOIN，带一个令牌）：分出一个子进程，拿令牌另连一条上去和接入方对接，握手（核对它有
//!   配对的密钥），然后像这台机器上的接入方一样连宿主的 socket，两头对转。一个接入方一个进程：握手是阻塞着做的，
//!   一个慢的、捣乱的拖不住别人；
//! - 两个方向的帧都一个个看过再转（`bridge`）。对方能做什么照配对表（`peers.json`）里写的：只让看的，HELLO 上加
//!   “只看”和“不算进窗口大小”，按键、改窗口大小的都不转；不让接入的不告诉它有这个会话。宿主发给对方的只转输出和
//!   “会话结束了”：它的状态里有账号的邮箱、完整的路径、进程号，不给。配对表变了（`cca remote allow`、取消配对）
//!   几秒钟内照新的来：不该再接着的子进程结束掉；
//! - 标准输入是宿主留着的一根管道：宿主不在了（会话结束、宿主被强行结束）读到头，告诉服务器会话结束了就退出。
//!   这个进程自己被强行结束时，子进程看到父进程换了也跟着走（不然它们没人管着，配对表变了也照转）。
//!
//! 退出码 3：这台机器用不了中转（不在哪个 Team 里、用不了会话走的那台服务器、服务器没开中转），宿主不用再起它。

use super::far;
use super::local::{self, Asked, Conn, Decoder, Meta};
use super::peers::{self, Incoming, Peer};
use super::relay::{self, ctl_frame, ctl_next, Server, Tunnel, DEAD_AFTER, PING_EVERY};
use super::sig;
use super::{crypto, pty};
use crate::cmd::arity;
use crate::cmd::hub::HubKind;
use crate::py::json;
use crate::py::obj::{Dict, V};
use crate::py::time::{monotonic, sleep};
use crate::py::{exit, R};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::os::fd::AsRawFd;
use std::time::Duration;

pub const EXIT_OFF: i32 = 3;
/// 一个会话同时最多转接几处（服务器那边也是这个数）
const MAX_KIDS: usize = 4;
/// 10 秒里最多为接入方分出这么多个子进程（服务器一个劲地说“有人来了”也不跟着它转）
const FORKS_PER_10S: usize = 8;
/// 一头还没写出去的超过这么多就先不读另一头
const HIGH: usize = 1024 * 1024;
/// 隔多久看一眼配对表变了没有
const PEERS_EVERY: f64 = 3.0;

/// 一个允许接入的配对
struct Allowed {
    peer: Peer,
    secret: Vec<u8>,
}

/// 和服务器之间的控制连接
struct Ctl {
    stream: TcpStream,
    /// 进来时那张票据的密钥：之后每个接入方的那一条连接凭它和令牌进
    key: Vec<u8>,
    port: u16,
    inbuf: Vec<u8>,
    out: VecDeque<u8>,
    heard: f64,
    pinged: f64,
}

/// 转接着一个接入方的子进程
struct Kid {
    child: libc::pid_t,
    pid: String,
    access: Incoming,
}

/// 这个会话在哪：宿主的 socket
struct Session {
    meta: Meta,
    dir: String,
    sock: String,
}

/// 这个会话走 hub 这台服务器：只有记在它上面的配对看得到这个会话
fn allowed_now(hub: HubKind) -> Vec<Allowed> {
    peers::load()
        .into_iter()
        .filter(|p| p.hub == hub && p.incoming != Incoming::None)
        .filter_map(|peer| peers::secret(&peer.pid).map(|secret| Allowed { peer, secret }))
        .take(16)
        .collect()
}

/// 配对表里和转接有关的那些（变了才要动）
fn shape(a: &[Allowed]) -> Vec<(String, Incoming)> {
    a.iter().map(|x| (x.peer.pid.clone(), x.peer.incoming)).collect()
}

fn announce(meta: &Meta, allowed: &[Allowed]) -> Vec<u8> {
    let blobs = Dict::new();
    for a in allowed {
        if let Ok(b) = crypto::dir_seal(&a.secret, &a.peer.pid, &meta.sid, &far::dir_doc(meta, a.peer.incoming)) {
            blobs.set(a.peer.pid.as_str(), b);
        }
    }
    ctl_frame(relay::F_ANNOUNCE, json::dumps(&V::Dict(blobs), json::Fmt::new().utf8().compact()).as_bytes())
}

/// 连上服务器的控制连接。Err(Some(秒))：这么久以后再试；Err(None)：用不了，退出
fn connect(srv: &Server, meta: &Meta, allowed: &[Allowed], fails: u32) -> R<Result<Ctl, Option<f64>>> {
    let pids: Vec<V> = allowed.iter().map(|a| V::from(a.peer.pid.as_str())).collect();
    let backoff = (5.0 * f64::from(1u32 << fails.min(5))).min(120.0);
    let t = match srv.ticket("relay_host", &crate::dict! { "sid" => meta.sid.as_str(), "pids" => pids })? {
        Ok(t) => t,
        Err(f) if f.code == 404 && f.error == "not_found" => return Ok(Err(None)),
        // 服务器不认这台机器、不认这些配对（对方都取消了）：等久一点（配对表变了会马上再试）
        Err(f) if f.code == 403 || f.code == 404 => return Ok(Err(Some(600.0))),
        Err(_) => return Ok(Err(Some(backoff))),
    };
    let Ok(stream) = srv.dial(t.port, "host", &t.tid, &t.key, 6.0) else { return Ok(Err(Some(backoff))) };
    if stream.set_nonblocking(true).is_err() {
        return Ok(Err(Some(backoff)));
    }
    let now = monotonic();
    let mut ctl = Ctl { stream, key: t.key, port: t.port, inbuf: Vec::new(), out: VecDeque::new(), heard: now, pinged: now };
    ctl.out.extend(announce(meta, allowed));
    Ok(Ok(ctl))
}

/// 读控制连接上现在有的，交出完整的帧；连接没了是 Err
fn frames(ctl: &mut Ctl) -> io::Result<Vec<(u8, Vec<u8>)>> {
    let mut buf = [0u8; 4096];
    loop {
        match ctl.stream.read(&mut buf) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => ctl.inbuf.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
        if ctl.inbuf.len() > 256 * 1024 {
            return Err(io::Error::other("控制连接上来的太多了"));
        }
    }
    let mut out = Vec::new();
    while let Some(f) = ctl_next(&mut ctl.inbuf) {
        out.push(f);
    }
    ctl.heard = monotonic();
    Ok(out)
}

/// 告诉服务器会话结束了（写不出去就算了：它过一分钟自己也会清掉）
fn bye(mut ctl: Ctl) {
    ctl.out.extend(ctl_frame(relay::F_BYE, b""));
    let _ = ctl.stream.set_nonblocking(false);
    let _ = ctl.stream.set_write_timeout(Some(Duration::from_secs(2)));
    let (a, b) = ctl.out.as_slices();
    let _ = ctl.stream.write_all(a).and_then(|()| ctl.stream.write_all(b));
}

/// 子进程里：转接一个接入方，到一头断了为止。parent 是分出它的那个进程：它不在了就走
fn bridge(srv: &Server, ctl: &Ctl, token: &str, a: &Allowed, at: &Session, parent: libc::pid_t) -> io::Result<()> {
    let mut stream = srv.dial(ctl.port, "join", token, &ctl.key, 10.0)?;
    let allowed = |pid: &str| (pid == a.peer.pid).then(|| (&a.peer, a.secret.clone()));
    let (chan, _) = relay::handshake_host(&mut stream, &at.meta.sid, &srv.fp, allowed)?;
    let mut conn = Conn::new(local::connect(&at.dir, &at.sock)?)?;
    let mut tun = Tunnel::new(stream, chan)?;
    let control = a.peer.incoming == Incoming::Control;
    let (mut from_far, mut from_host) = (Decoder::default(), Decoder::default());
    let mut greeted = false;
    const BAD: libc::c_short = libc::POLLERR | libc::POLLHUP | libc::POLLNVAL;
    // 接入方那头没了：告诉宿主断开了。宿主那头没了（会话结束、这一处被它断开）：把它最后说的写给接入方再走
    fn far_gone(conn: &mut Conn) -> io::Result<()> {
        conn.send(&local::frame(local::DETACH, b""));
        conn.drain(1.0);
        Ok(())
    }
    fn host_gone(tun: &mut Tunnel) -> io::Result<()> {
        tun.drain(2.0);
        Ok(())
    }
    loop {
        let ev = |read: bool, write: bool| if read { libc::POLLIN } else { 0 } | if write { libc::POLLOUT } else { 0 };
        let mut fds = [
            libc::pollfd { fd: tun.fd(), events: ev(conn.pending() < HIGH, tun.pending() > 0), revents: 0 },
            libc::pollfd { fd: conn.fd(), events: ev(tun.pending() < HIGH, conn.pending() > 0), revents: 0 },
        ];
        // SAFETY: fds 是有效的数组
        unsafe { libc::poll(fds.as_mut_ptr(), 2, 1000) };
        // SAFETY: getppid 没有参数、不会失败
        if unsafe { libc::getppid() } != parent {
            return far_gone(&mut conn);
        }
        // 接入方发来的：只让看的，按键、改窗口大小的不转（它的窗口多小都不该影响会话）；问宿主状态的谁都不转
        if fds[0].revents & (libc::POLLIN | BAD) != 0 {
            match tun.recv() {
                Ok(Some(data)) => from_far.push(&data),
                Ok(None) | Err(_) => return far_gone(&mut conn),
            }
            loop {
                match from_far.next() {
                    Ok(Some((local::HELLO, mut p))) if !greeted && p.len() >= 2 => {
                        greeted = true;
                        if !control {
                            p[1] |= local::FLAG_READ_ONLY | local::FLAG_PASSIVE;
                        }
                        conn.send(&local::frame(local::HELLO, &p));
                    }
                    Ok(Some((kind @ (local::INPUT | local::RESIZE), p))) if greeted && control => {
                        conn.send(&local::frame(kind, &p))
                    }
                    Ok(Some((local::DETACH, p))) if greeted => conn.send(&local::frame(local::DETACH, &p)),
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(()) => return far_gone(&mut conn),
                }
            }
        }
        // 宿主发来的：只转输出和“会话结束了”
        if fds[1].revents & (libc::POLLIN | BAD) != 0 {
            match conn.recv() {
                Ok(Some(data)) => from_host.push(&data),
                Ok(None) | Err(_) => return host_gone(&mut tun),
            }
            loop {
                match from_host.next() {
                    Ok(Some((kind @ (local::OUTPUT | local::EXIT), p))) => tun.send(&local::frame(kind, &p)),
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(()) => return host_gone(&mut tun),
                }
            }
        }
        if !tun.tick() || tun.flush().is_err() {
            return far_gone(&mut conn);
        }
        if conn.flush().is_err() {
            return host_gone(&mut tun);
        }
    }
}

/// 收走退出了的子进程
fn reap(kids: &mut Vec<Kid>) {
    kids.retain(|k| {
        let mut status = 0;
        // SAFETY: status 是有效的；child 是自己分出来的子进程
        unsafe { libc::waitpid(k.child, &mut status, libc::WNOHANG) == 0 }
    });
}

fn stop<'a>(kids: impl IntoIterator<Item = &'a Kid>) {
    for k in kids {
        // SAFETY: 给自己分出来的子进程发信号；它已经没了时出错，不管
        unsafe { libc::kill(k.child, libc::SIGTERM) };
    }
}

/// 宿主刚起来、正忙时可能一下没答：多问几次
fn find_session(sid: &str) -> Option<Session> {
    let (dir, sock) = (local::dir(), format!("{}.sock", local::short(sid)));
    for _ in 0..10 {
        match local::ask_status(&dir, &sock) {
            Asked::Alive(st) if st.meta.sid == sid => return Some(Session { meta: st.meta, dir, sock }),
            Asked::Alive(_) | Asked::Gone => return None,
            Asked::Busy => sleep(0.1),
        }
    }
    None
}

/// `remote-link 会话编号`
pub fn cmd_remote_link(args: &[String]) -> R<()> {
    arity("cmd_remote_link", args, 1, 1)?;
    let Some(at) = find_session(&args[0]) else { return exit(1) };
    // 会话走哪台服务器照它的账号：是哪个 Team 给的账号就走那个 Team 的服务器，别的走这台机器的家（加入的第一个 Team 的；
    // 不在哪个 Team 里时没有，这个会话不经中转）
    let Some(hub) = relay::session_hub(&at.meta.tool, &at.meta.key) else { return exit(EXIT_OFF) };
    let Ok(srv) = relay::server_of(hub)? else { return exit(EXIT_OFF) };
    let wake = sig::install()?;

    let mut allowed: Vec<Allowed> = Vec::new();
    let mut stamp: Option<peers::Stamp> = None; // 还没看过是 None
    let mut checked = f64::NEG_INFINITY;
    let mut ctl: Option<Ctl> = None;
    let mut kids: Vec<Kid> = Vec::new();
    let mut forks: VecDeque<f64> = VecDeque::new();
    let (mut next_try, mut fails) = (0.0f64, 0u32);
    let mut buf = [0u8; 64];
    'run: loop {
        let now = monotonic();
        reap(&mut kids);
        // 配对表变了：照新的来
        if now - checked >= PEERS_EVERY {
            checked = now;
            let s = peers::stamp();
            if stamp != Some(s) {
                stamp = Some(s);
                let fresh = allowed_now(hub);
                if shape(&fresh) != shape(&allowed) {
                    // 不该再接着的（不让了、改成只看了、取消配对了）结束掉；它们留在 kids 里等着收走
                    let keep = shape(&fresh);
                    stop(kids.iter().filter(|k| !keep.contains(&(k.pid.clone(), k.access))));
                    allowed = fresh;
                    // 换一条控制连接（新的票据上是新的那几个配对）：旧的直接断开；一个都不让了就告诉服务器会话没了
                    if let Some(old) = ctl.take() {
                        if allowed.is_empty() {
                            bye(old);
                        }
                    }
                    (next_try, fails) = (now, 0);
                }
            }
        }
        if ctl.is_none() && !allowed.is_empty() && now >= next_try {
            match connect(&srv, &at.meta, &allowed, fails)? {
                Ok(c) => {
                    ctl = Some(c);
                    fails = 0;
                }
                Err(Some(wait)) => {
                    fails += 1;
                    next_try = now + wait;
                }
                Err(None) => {
                    stop(&kids);
                    return exit(EXIT_OFF);
                }
            }
        }
        let ctl_fd = ctl.as_ref().map_or(-1, |c| c.stream.as_raw_fd());
        let ctl_out = ctl.as_ref().is_some_and(|c| !c.out.is_empty());
        let mut fds = [
            libc::pollfd { fd: wake, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: ctl_fd, events: libc::POLLIN | if ctl_out { libc::POLLOUT } else { 0 }, revents: 0 },
        ];
        // SAFETY: fds 是有效的数组；被信号打断就是要它马上回来
        unsafe { libc::poll(fds.as_mut_ptr(), 3, 1000) };
        if fds[0].revents != 0 || sig::pending() {
            let got = sig::take(wake);
            if got.has(libc::SIGTERM) || got.has(libc::SIGINT) || got.has(libc::SIGHUP) || got.has(libc::SIGQUIT) {
                break 'run;
            }
        }
        // 宿主不在了：管道读到头（它从不往里写）
        if fds[1].revents != 0 && !matches!(pty::read_some(0, &mut buf), Some(Ok(n)) if n > 0) {
            break 'run;
        }
        let mut lost = false;
        if let Some(c) = ctl.as_mut() {
            let now = monotonic();
            if fds[2].revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                match frames(c) {
                    Ok(list) => {
                        for (kind, payload) in list {
                            if kind == relay::F_JOIN {
                                join(&srv, c, &payload, &allowed, &at, &mut kids, &mut forks);
                            }
                        }
                    }
                    Err(_) => lost = true,
                }
            }
            if now - c.pinged >= PING_EVERY {
                c.pinged = now;
                c.out.extend(ctl_frame(relay::F_PING, b""));
            }
            lost = lost || now - c.heard > DEAD_AFTER || local::write_queue(&mut c.stream, &mut c.out).is_err();
        }
        if lost {
            ctl = None;
            fails += 1;
            next_try = monotonic() + (2.0 * f64::from(1u32 << fails.min(5))).min(60.0);
        }
    }
    // 收场：告诉服务器会话结束了；子进程把宿主最后说的（“会话结束了”）转完自己会走，等它们一会儿
    if let Some(c) = ctl.take() {
        bye(c);
    }
    let end = monotonic() + 3.0;
    while !kids.is_empty() && monotonic() < end {
        sleep(0.05);
        reap(&mut kids);
    }
    stop(&kids);
    Ok(())
}

/// 服务器说有接入方来了：分出一个子进程去转接它。不是允许接入的配对、已经转接着够多了、它说得太勤：不理
/// （接入方那头等一会儿，服务器告诉它宿主没来）
fn join(
    srv: &Server,
    ctl: &Ctl,
    payload: &[u8],
    allowed: &[Allowed],
    at: &Session,
    kids: &mut Vec<Kid>,
    forks: &mut VecDeque<f64>,
) {
    let Some(j) = std::str::from_utf8(payload).ok().and_then(|s| json::loads(s).ok()).and_then(|j| j.as_dict()) else { return };
    let (Some(token), Some(pid)) = (j.str("token").filter(|t| peers::hex32(t)), j.str("pid")) else { return };
    let Some(a) = allowed.iter().find(|a| *a.peer.pid == *pid) else { return };
    let now = monotonic();
    while forks.front().is_some_and(|t| now - t > 10.0) {
        forks.pop_front();
    }
    if kids.len() >= MAX_KIDS || forks.len() >= FORKS_PER_10S {
        return;
    }
    forks.push_back(now);
    // SAFETY: getpid 没有参数、不会失败
    let me = unsafe { libc::getpid() };
    // SAFETY: 这个进程只有一条线程；子进程里只用自己这一份内存、新开的连接，做完直接 _exit（不跑父进程的收尾）
    match unsafe { libc::fork() } {
        0 => {
            sig::reset();
            // SAFETY: 关掉从父进程带来的控制连接（父进程那一份不受影响；不关的话父进程断开它时服务器看不到）。
            // 之后这个子进程不再读写 ctl.stream（只用里面记着的端口和票据密钥），最后是 _exit，不会再关一次
            unsafe { libc::close(ctl.stream.as_raw_fd()) };
            let ok = bridge(srv, ctl, &token, a, at, me).is_ok();
            // SAFETY: 子进程到此为止
            unsafe { libc::_exit(if ok { 0 } else { 1 }) }
        }
        child if child > 0 => kids.push(Kid { child, pid: a.peer.pid.clone(), access: a.peer.incoming }),
        _ => {}
    }
}
