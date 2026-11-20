//! 接入端（`remote-attach`，`cca remote attach`）：一个普通的终端程序，把这个终端接到一个在宿主里运行的会话上。
//! 按键原样送过去、那边的输出原样写到这个终端；窗口大小变了告诉宿主。
//!
//! 会话可以在这台机器上（连宿主的 socket），也可以在配对过的另一台机器上（经服务器中转，两头之间端到端加密：
//! `relay.rs`）。这台机器上没有对得上的会话时才去问服务器；之后说的话是同一套帧，只是走的路不一样（`Wire`）。
//!
//! 断开（会话在那边接着运行）：先按 Ctrl-\，再按 d 或者 .；要给会话本身发一个 Ctrl-\ 就连按两次。这个终端被关掉、
//! 这个进程收到 SIGTERM / SIGHUP 也是断开。退出前把会话在这个终端上打开的模式（备用屏幕、括号粘贴、鼠标上报……）关掉。
//!
//! 退出码：会话结束了是客户端的退出码；自己断开的是 0；64 用法不对或者前缀对上了好几个，65 没有这个会话，
//! 66 经服务器接入没成（连不上服务器、那台机器不在线、对不上配对的密钥），68 宿主没了或者连接断了（没说会话结束
//! 就断了），69 不在终端里。

use super::far::{self, Far};
use super::keys::Keys;
use super::local::{self, Conn, Decoder, Status};
use super::peers::{self, Peer};
use super::pty::{self, Raw};
use super::relay::{self, Server, Tunnel};
use super::sig;
use super::vt::{Class, Modes, Part, Scanner};
use crate::core::base::RO;
use crate::dict;
use crate::py::time::monotonic;
use crate::py::{die, exit, R};
use std::collections::VecDeque;
use std::io;
use std::os::fd::{AsRawFd, RawFd};

/// 这个终端还没写出去的超过这么多就先不从宿主那边读（宿主那边攒多了会断开这个接入）
const OUT_HIGH: usize = 1024 * 1024;

pub const EXIT_USAGE: i32 = 64;
pub const EXIT_NOT_FOUND: i32 = 65;
pub const EXIT_RELAY: i32 = 66;
pub const EXIT_HOST_GONE: i32 = 68;
pub const EXIT_NO_TTY: i32 = 69;

/// 要接入的会话
enum Target {
    Near(Status),
    Far(Far),
}

/// 到宿主的那条路：这台机器上的 socket，或者经服务器的加密通道。说的是同一套帧
enum Wire {
    Near(Conn),
    Far(Box<Tunnel>),
}

impl Wire {
    fn fd(&self) -> RawFd {
        match self {
            Wire::Near(c) => c.fd(),
            Wire::Far(t) => t.fd(),
        }
    }

    fn pending(&self) -> usize {
        match self {
            Wire::Near(c) => c.pending(),
            Wire::Far(t) => t.pending(),
        }
    }

    fn send(&mut self, data: &[u8]) {
        match self {
            Wire::Near(c) => c.send(data),
            Wire::Far(t) => t.send(data),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Wire::Near(c) => c.flush(),
            Wire::Far(t) => t.flush(),
        }
    }

    fn recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        match self {
            Wire::Near(c) => c.recv(),
            Wire::Far(t) => t.recv(),
        }
    }

    /// 隔一阵说一声（中转的连接闲着会被掐）；false：对面太久没动静了
    fn tick(&mut self) -> bool {
        match self {
            Wire::Near(_) => true,
            Wire::Far(t) => t.tick(),
        }
    }

    fn drain(&mut self, secs: f64) {
        match self {
            Wire::Near(c) => c.drain(secs),
            Wire::Far(t) => t.drain(secs),
        }
    }
}

/// 别的机器上的会话（配对过才有）。没问到的原因写到 stderr、当作一个都没有
fn far_sessions(all: &[Peer]) -> R<(Option<Server>, Vec<Far>)> {
    if all.is_empty() {
        return Ok((None, Vec::new()));
    }
    let srv = match relay::server()? {
        Ok(s) => s,
        Err(why) => {
            crate::err!("别的机器上的会话没查：{why}");
            return Ok((None, Vec::new()));
        }
    };
    match far::fetch(&srv, all, 8)? {
        Ok(l) => Ok((Some(srv), l.sessions)),
        Err(f) => {
            crate::err!("别的机器上的会话没查到：{}", f.text);
            Ok((Some(srv), Vec::new()))
        }
    }
}

/// 给一个前缀，找出要接入的会话：这台机器上有对得上的就是它（不联网）；没有才看配对过的机器上的。
/// Err(退出码)：说明已经写到 stderr
fn pick(prefix: Option<&str>, all: &[Peer]) -> R<Result<(Target, Option<Server>), i32>> {
    let prefix = prefix.map(|p| p.to_ascii_lowercase());
    let hit = |sid: &str| prefix.as_deref().is_none_or(|p| sid.starts_with(p));
    let near: Vec<Status> = local::sessions(!*RO).into_iter().filter(|s| hit(&s.meta.sid)).collect();
    if let [one] = near.as_slice() {
        return Ok(Ok((Target::Near(one.clone()), None)));
    }
    let (srv, far) = if near.is_empty() { far_sessions(all)? } else { (None, Vec::new()) };
    let far: Vec<Far> = far.into_iter().filter(|f| hit(&f.sid)).collect();
    if let ([], [one]) = (near.as_slice(), far.as_slice()) {
        return Ok(Ok((Target::Far(one.clone()), srv)));
    }
    Ok(Err(match (near.len() + far.len(), prefix) {
        (0, None) if all.is_empty() => {
            crate::err!("这台机器上现在没有可以接入的会话（先 cca remote on，之后启动的会话才能接入）");
            EXIT_NOT_FOUND
        }
        (0, None) => {
            crate::err!("这台机器上、配对过的机器上现在都没有可以接入的会话（那边要先 cca remote on，之后启动的会话才能接入）");
            EXIT_NOT_FOUND
        }
        (0, Some(p)) => {
            crate::err!("没有编号以 {p} 开头的会话（cca remote list 看现在有哪些）");
            EXIT_NOT_FOUND
        }
        (n, _) => {
            crate::err!("有 {n} 个会话，说一下接入哪个：cca remote attach 编号");
            for s in &near {
                crate::err!("  {}", super::manage::row(s));
            }
            for f in &far {
                crate::err!("  {}", far::row(f));
            }
            EXIT_USAGE
        }
    }))
}

/// 经服务器接到另一台机器上的会话。Err(退出码)：说明已经写到 stderr
fn dial_far(f: &Far, all: &[Peer], srv: &Server) -> R<Result<Tunnel, i32>> {
    let (Some(peer), Some(secret)) = (all.iter().find(|p| p.pid == f.pid), peers::secret(&f.pid)) else {
        crate::err!("和「{}」配对的密钥读不到了：两边重新配对（cca remote pair / cca remote join）", f.peer);
        return Ok(Err(EXIT_RELAY));
    };
    crate::err!("正在接入「{}」上的会话 {}…", f.peer, &f.sid[..8]);
    let t = match srv.ticket("relay_attach", &dict! { "sid" => f.sid.as_str(), "pid" => f.pid.as_str() })? {
        Ok(t) => t,
        Err(e) => {
            crate::err!("{}", e.text);
            return Ok(Err(if e.error == "no_session" { EXIT_NOT_FOUND } else { EXIT_RELAY }));
        }
    };
    // 服务器要等那台机器另连一条上来才回话
    let made = srv
        .dial(t.port, "view", &t.tid, &t.key, 16.0)
        .and_then(|mut s| Ok((relay::handshake_viewer(&mut s, peer, &secret, &f.sid, &srv.fp)?, s)))
        .and_then(|(chan, s)| Tunnel::new(s, chan));
    Ok(match made {
        Ok(t) => Ok(t),
        Err(e) => {
            let why = if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) {
                "等了一阵那台机器没有回应（它不在线，或者网络不通）".to_string()
            } else {
                e.to_string()
            };
            crate::err!("没接上：{why}");
            Err(EXIT_RELAY)
        }
    })
}

enum End {
    /// 会话结束了（客户端的退出码）
    Exited(i32),
    /// 自己断开的；true：终端还在，可以说一句
    Detached(bool),
    HostGone,
}

pub fn cmd_remote_attach(args: &[String]) -> R<()> {
    let mut read_only = false;
    let mut prefix: Option<&str> = None;
    for a in args {
        match a.as_str() {
            "-r" | "--read-only" => read_only = true,
            p if prefix.is_none() && !p.is_empty() && p.bytes().all(|b| b.is_ascii_hexdigit()) => prefix = Some(p),
            _ => {
                crate::err!("用法：cca remote attach [-r] [会话编号的前几位]");
                return exit(EXIT_USAGE);
            }
        }
    }
    let all = peers::load();
    let (target, srv) = match pick(prefix, &all)? {
        Ok(t) => t,
        Err(code) => return exit(code),
    };
    if !pty::isatty(0) || !pty::isatty(1) {
        crate::err!("cca remote attach 要在终端里运行");
        return exit(EXIT_NO_TTY);
    }
    let (sid, is_far) = match &target {
        Target::Near(s) => (s.meta.sid.clone(), false),
        Target::Far(f) => (f.sid.clone(), true),
    };
    let mut wire = match &target {
        Target::Near(s) => {
            let conn = local::connect(&local::dir(), &format!("{}.sock", local::short(&s.meta.sid))).and_then(Conn::new);
            let Ok(conn) = conn else {
                crate::err!("连不上这个会话（它可能刚结束）");
                return exit(EXIT_NOT_FOUND);
            };
            Wire::Near(conn)
        }
        Target::Far(f) => {
            let srv = srv.expect("别的机器上的会话是问服务器问来的");
            if f.view_only && !read_only {
                crate::err!("「{}」只让这台机器看这个会话，不能操作", f.peer);
                read_only = true;
            }
            match dial_far(f, &all, &srv)? {
                Ok(t) => Wire::Far(Box::new(t)),
                Err(code) => return exit(code),
            }
        }
    };
    pty::adopt_tty(0);
    let Ok(tty) = pty::reopen_tty(0) else { return die("打不开这个终端") };
    let wake = sig::install()?;
    let Some(size) = pty::get_size(tty.as_raw_fd()) else { return die("问不到这个终端的大小") };
    wire.send(&local::hello(size, read_only));
    let mut raw = Raw::enter(tty.as_raw_fd())?;

    let tty_fd = tty.as_raw_fd();
    let mut dec = Decoder::default();
    let mut keys = Keys::default();
    let (mut scan, mut modes) = (Scanner::default(), Modes::default());
    let mut to_tty: VecDeque<u8> = VecDeque::new();
    let mut buf = vec![0u8; 4096];
    let end = 'run: loop {
        let mut fds = [
            libc::pollfd { fd: wake, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: tty_fd, events: libc::POLLIN | if to_tty.is_empty() { 0 } else { libc::POLLOUT }, revents: 0 },
            libc::pollfd {
                fd: wire.fd(),
                events: if to_tty.len() < OUT_HIGH { libc::POLLIN } else { 0 }
                    | if wire.pending() == 0 { 0 } else { libc::POLLOUT },
                revents: 0,
            },
        ];
        // SAFETY: fds 是有效的数组；被信号打断（EINTR）就是要它马上回来
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 3, 1000) };
        if !wire.tick() {
            break End::HostGone;
        }
        if n <= 0 && !sig::pending() && wire.pending() == 0 {
            continue;
        }
        const BAD: libc::c_short = libc::POLLERR | libc::POLLHUP | libc::POLLNVAL;
        if fds[0].revents != 0 || sig::pending() {
            let got = sig::take(wake);
            if got.has(libc::SIGHUP) {
                break End::Detached(false);
            }
            if got.has(libc::SIGTERM) || got.has(libc::SIGINT) || got.has(libc::SIGQUIT) {
                break End::Detached(true);
            }
            if got.has(libc::SIGCONT) {
                raw.resume();
            }
            if got.has(libc::SIGWINCH) || got.has(libc::SIGCONT) {
                if let Some(s) = pty::get_size(tty_fd) {
                    wire.send(&local::resize(s));
                }
            }
        }
        // 按键
        if fds[1].revents & (libc::POLLIN | BAD) != 0 {
            match pty::read_some(tty_fd, &mut buf) {
                Some(Ok(0)) | Some(Err(_)) => break End::Detached(false), // 终端没了
                Some(Ok(n)) => {
                    let mut send = Vec::with_capacity(n);
                    if keys.feed(&buf[..n], &mut send) {
                        break End::Detached(true);
                    }
                    if !send.is_empty() && !read_only {
                        wire.send(&local::frame(local::INPUT, &send));
                    }
                }
                None => {}
            }
        }
        // 宿主那边来的
        if fds[2].revents & (libc::POLLIN | BAD) != 0 {
            match wire.recv() {
                Ok(Some(data)) => dec.push(&data),
                Ok(None) | Err(_) => break 'run End::HostGone,
            }
            loop {
                match dec.next() {
                    Ok(Some((local::OUTPUT, payload))) => {
                        scan.feed(&payload, &mut |seg, bytes| {
                            if seg.class == Class::Seq && seg.part == Part::Whole {
                                modes.observe(bytes);
                            }
                        });
                        to_tty.extend(payload);
                    }
                    Ok(Some((local::EXIT, payload))) => {
                        let code = payload.get(..4).map_or(1, |b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]));
                        break 'run End::Exited(code);
                    }
                    Ok(Some(_)) => {} // WELCOME、以后加的帧：不用管
                    Ok(None) => break,
                    Err(()) => break 'run End::HostGone,
                }
            }
        }
        if wire.flush().is_err() {
            break End::HostGone;
        }
        while !to_tty.is_empty() {
            match pty::write_some(tty_fd, to_tty.as_slices().0) {
                Ok(0) => break,
                Ok(n) => {
                    to_tty.drain(..n);
                }
                Err(_) => break 'run End::Detached(false),
            }
        }
    };

    // 收场：告诉宿主断开了、把这个终端还原
    if matches!(end, End::Detached(_)) {
        wire.send(&local::frame(local::DETACH, b""));
        wire.drain(1.0);
    }
    let tty_alive = !matches!(end, End::Detached(false));
    if tty_alive {
        to_tty.extend(modes.undo());
        to_tty.extend(b"\r\n".iter().copied());
        let deadline = monotonic() + 1.0;
        while !to_tty.is_empty() && monotonic() < deadline {
            let mut p = libc::pollfd { fd: tty_fd, events: libc::POLLOUT, revents: 0 };
            // SAFETY: p 是有效的一项
            unsafe { libc::poll(&mut p, 1, 100) };
            match pty::write_some(tty_fd, to_tty.as_slices().0) {
                Ok(n) => {
                    to_tty.drain(..n);
                }
                Err(_) => break,
            }
        }
    }
    drop(raw);
    let short = &sid[..8];
    match end {
        End::Exited(code) => {
            if code == 0 {
                crate::err!("会话已经结束");
            } else {
                crate::err!("会话已经结束（退出码 {code}）");
            }
            exit(code)
        }
        End::Detached(say) => {
            if say {
                crate::err!("已断开，会话还在运行（再接入：cca remote attach {short}）");
            }
            Ok(())
        }
        End::HostGone if is_far => {
            crate::err!("连接断了：网络断了、那台机器上的会话不在了，或者这边太慢被断开了（cca remote list 看它还在不在，再接入：cca remote attach {short}）");
            exit(EXIT_HOST_GONE)
        }
        End::HostGone => {
            crate::err!("连接断了：这个会话的宿主不在了，或者这边太慢被它断开了（cca remote list 看它还在不在）");
            exit(EXIT_HOST_GONE)
        }
    }
}
