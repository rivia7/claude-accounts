//! 宿主那头的转接（`remote-link <会话编号>`）：会话宿主起的一个小进程，把配对过的机器经服务器接到这个会话上。
//! 联网的事都在这里，宿主自己不碰网络——这个进程卡住、崩了，会话和这台机器上的接入都不受影响（宿主过一会儿再起一个）。
//!
//! - 和服务器之间一条控制连接（`relay_host` 拿票据进数据端口）：把会话的说明封给各个允许接入的配对（ANNOUNCE），
//!   隔一阵说一声（PING），断了按退避重连；
//! - 服务器说有接入方来了（JOIN，带一个令牌）：分出一个子进程，拿令牌另连一条上去和接入方对接，握手（核对它有
//!   配对的密钥），然后像这台机器上的接入方一样连宿主的 socket，两头对转。一个接入方一个进程：握手是阻塞着做的，
//!   一个慢的、捣乱的拖不住别人；
//! - 两个方向的帧都一个个看过再转（`bridge`）。对方能做什么照配对表（`peers.json`）里写的：只让看的，HELLO 上加
//!   “只看”和“不算进窗口大小”，按键、改窗口大小的都不转；不让接入的不告诉它有这个会话。宿主发给对方的只转输出、
//!   “会话结束了”、通知和会话的大小（`SIZE`：只有说过能缩放的才收得到）：它的状态里有账号的邮箱、完整的路径、进程号，不给。配对表变了（`cca remote allow`、取消配对）
//!   几秒钟内照新的来：不该再接着的子进程结束掉；
//! - 会话里推的通知（宿主写进标准输入的 `NOTIFY` 帧）：给每个允许接入的手机（配对表里 `web` 的）封一份、一个配对一帧 PUSH，
//!   服务器交给推送服务（手机锁着屏、App 没开着也收得到；只有手机解得开）。和服务器断着时攒几条，连上了再推；
//! - 标准输入是宿主留着的一根管道：宿主不在了（会话结束、宿主被强行结束）读到头，告诉服务器会话结束了就退出。
//!   这个进程自己被强行结束时，子进程看到父进程换了也跟着走（不然它们没人管着，配对表变了也照转）。
//!
//! 退出码 3：这台机器用不了中转（不在哪个 Team 里、用不了会话走的那台服务器、服务器没开中转），宿主不用再起它。
//!
//! 远程启动的转接进程（launch.rs 的 `cmd_launcher_link`）也是这样连服务器、接手机的：公共的部分是这里的 `Link`。

use super::far;
use super::local::{self, Asked, Conn, Decoder, Meta, Note};
use super::peers::{self, Incoming, Peer};
use super::relay::{self, ctl_frame, ctl_next, Server, Tunnel, DEAD_AFTER, PING_EVERY};
use super::sig;
use super::{crypto, pty};
use crate::cmd::arity;
use crate::cmd::hub::HubKind;
use crate::py::json;
use crate::py::obj::{Dict, V};
use crate::py::time::{monotonic, sleep, time};
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
/// 和服务器断着时宿主交来的通知最多攒几条、攒多久（连上了再推；再早的推过去也没用了）
const NOTES_KEPT: usize = 8;
const NOTE_KEEP: f64 = 60.0;

/// 一个允许接入的配对
pub(super) struct Allowed {
    pub(super) peer: Peer,
    pub(super) secret: Vec<u8>,
}

/// 和服务器之间的控制连接
pub(super) struct Ctl {
    pub(super) stream: TcpStream,
    /// 进来时那张票据的密钥：之后每个接入方的那一条连接凭它和令牌进
    pub(super) key: Vec<u8>,
    pub(super) port: u16,
    inbuf: Vec<u8>,
    pub(super) out: VecDeque<u8>,
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
    // 这几个配对都在同一台服务器（同一个 Team）上：盐只要找一次
    let salt = allowed.first().and_then(|a| far::team_salt(a.peer.hub));
    for a in allowed {
        if let Ok(b) = crypto::dir_seal(&a.secret, &a.peer.pid, &meta.sid, &far::dir_doc(meta, a.peer.incoming, salt.as_deref())) {
            blobs.set(a.peer.pid.as_str(), b);
        }
    }
    ctl_frame(relay::F_ANNOUNCE, json::dumps(&V::Dict(blobs), json::Fmt::new().utf8().compact()).as_bytes())
}

/// 连上服务器的控制连接：登记成 sid 这个会话，连上以后先说 first（ANNOUNCE：会话的说明是这里封的，启动器的是 launch.rs 封的）。
/// Err(Some(秒))：这么久以后再试；Err(None)：用不了，退出
fn connect_as(srv: &Server, sid: &str, allowed: &[Allowed], first: Vec<u8>, fails: u32) -> R<Result<Ctl, Option<f64>>> {
    let pids: Vec<V> = allowed.iter().map(|a| V::from(a.peer.pid.as_str())).collect();
    let backoff = (5.0 * f64::from(1u32 << fails.min(5))).min(120.0);
    let t = match srv.ticket("relay_host", &crate::dict! { "sid" => sid, "pids" => pids })? {
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
    ctl.out.extend(first);
    Ok(Ok(ctl))
}

/// 控制连接的这一轮：ready（poll 说有动静）时读进来、交出完整的帧；该说一声了说一声；把要说的写出去。断了（读写出错、太久没动静）是 Err
fn ctl_step(c: &mut Ctl, ready: bool) -> Result<Vec<(u8, Vec<u8>)>, ()> {
    let got = if ready { frames(c).map_err(|_| ())? } else { Vec::new() };
    let now = monotonic();
    if now - c.pinged >= PING_EVERY {
        c.pinged = now;
        c.out.extend(ctl_frame(relay::F_PING, b""));
    }
    if now - c.heard > DEAD_AFTER || local::write_queue(&mut c.stream, &mut c.out).is_err() {
        return Err(());
    }
    Ok(got)
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

/// 一条通知封给每个允许接入的手机：一个配对一帧 PUSH（`{"pid", "id", "note"}`，note 只有那个手机解得开；id 是宿主给的编号，
/// 服务器交给推送服务时当它的 collapse id）。手机没登记推送、服务器没开推送的，服务器自己不理
fn push_note(ctl: &mut Ctl, meta: &Meta, allowed: &[Allowed], n: &Note) {
    let doc = crate::dict! {
        "v" => 1, "id" => n.id.as_str(), "sid" => meta.sid.as_str(), "at" => time() as i64,
        "title" => n.title.as_str(), "subtitle" => n.subtitle.as_str(), "body" => n.body.as_str(),
    };
    for a in allowed.iter().filter(|a| a.peer.web) {
        if let Ok(blob) = crypto::note_seal(&a.secret, &a.peer.pid, &doc) {
            let j = crate::dict! { "pid" => a.peer.pid.as_str(), "id" => n.id.as_str(), "note" => blob };
            ctl.out.extend(ctl_frame(relay::F_PUSH, json::dumps(&V::Dict(j), json::Fmt::new().compact()).as_bytes()));
        }
    }
}

/// 告诉服务器会话结束了（写不出去就算了：它过一分钟自己也会清掉）
fn bye(mut ctl: Ctl) {
    ctl.out.extend(ctl_frame(relay::F_BYE, b""));
    let _ = ctl.stream.set_nonblocking(false);
    let _ = ctl.stream.set_write_timeout(Some(Duration::from_secs(2)));
    let (a, b) = ctl.out.as_slices();
    let _ = ctl.stream.write_all(a).and_then(|()| ctl.stream.write_all(b));
}

/// 子进程里：转接一个接入方，到一头断了为止。分出它的那个进程（j.parent）不在了就走
fn bridge(j: &Join, at: &Session) -> io::Result<()> {
    let mut tun = j.tunnel()?;
    let mut conn = Conn::new(local::connect(&at.dir, &at.sock)?)?;
    let control = j.a.peer.incoming == Incoming::Control;
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
        if unsafe { libc::getppid() } != j.parent {
            return far_gone(&mut conn);
        }
        // 接入方发来的：只让看的，按键、改窗口大小的不转（它的窗口多小都不该影响会话）；问宿主状态的谁都不转；起会话的（LAUNCH：
        // 只在启动器的通道上有用，launch.rs）也不转
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
        // 宿主发来的：只转输出、“会话结束了”、通知（会话里的钩子推的，宿主清理过）和会话的大小（给能缩放的接入方）
        if fds[1].revents & (libc::POLLIN | BAD) != 0 {
            match conn.recv() {
                Ok(Some(data)) => from_host.push(&data),
                Ok(None) | Err(_) => return host_gone(&mut tun),
            }
            loop {
                match from_host.next() {
                    Ok(Some((kind @ (local::OUTPUT | local::EXIT | local::NOTIFY | local::SIZE), p))) => {
                        tun.send(&local::frame(kind, &p))
                    }
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

/// `remote-link 会话编号`（`remote-link --launcher Team 编号 会话编号`：启动器的转接进程，launch.rs）
pub fn cmd_remote_link(args: &[String]) -> R<()> {
    if args.first().is_some_and(|a| a == "--launcher") {
        return super::launch::cmd_launcher_link(&args[1..]);
    }
    arity("cmd_remote_link", args, 1, 1)?;
    let Some(at) = find_session(&args[0]) else { return exit(1) };
    // 会话走哪台服务器照它的账号：是哪个 Team 给的账号就走那个 Team 的服务器，别的走这台机器的家（加入的第一个 Team 的；
    // 不在哪个 Team 里时没有，这个会话不经中转）
    let Some(hub) = relay::session_hub(&at.meta.tool, &at.meta.key) else { return exit(EXIT_OFF) };
    let Ok(srv) = relay::server_of(hub)? else { return exit(EXIT_OFF) };
    let mut link = Link::new(srv, &at.meta.sid)?;
    let (mut from_host, mut notes) = (Decoder::default(), VecDeque::<(f64, Note)>::new());
    let mut buf = [0u8; 4096];
    loop {
        let now = monotonic();
        link.reap();
        link.refresh(now, || allowed_now(hub));
        // 连上以后先说 ANNOUNCE，攒着的通知下面接着推
        if !link.connect_if_due(now, |allowed| announce(&at.meta, allowed))? {
            return exit(EXIT_OFF);
        }
        let Some(woke) = link.wait() else { break };
        // 宿主交来的通知；读到头（出错、帧不对）是宿主不在了：这一轮的推完再走
        let mut host_gone = false;
        if woke.stdin {
            match pty::read_some(0, &mut buf) {
                Some(Ok(n)) if n > 0 => from_host.push(&buf[..n]),
                None => {}
                _ => host_gone = true,
            }
            loop {
                match from_host.next() {
                    Ok(Some((local::NOTIFY, p))) => {
                        if let Some(n) = Note::parse(&p).filter(|n| Note::id_ok(&n.id)) {
                            notes.push_back((now, n));
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(()) => {
                        host_gone = true;
                        break;
                    }
                }
            }
            while notes.len() > NOTES_KEPT {
                notes.pop_front();
            }
        }
        match link.ctl.as_mut() {
            Some(c) => {
                for (_, n) in notes.drain(..) {
                    push_note(c, &at.meta, &link.allowed, &n);
                }
            }
            None => notes.retain(|(t, _)| now - t < NOTE_KEEP),
        }
        if host_gone {
            break;
        }
        for p in &woke.joins {
            link.join(p, |_| Some(()), |j, ()| if bridge(j, &at).is_ok() { 0 } else { 1 });
        }
    }
    // 收场：告诉服务器会话结束了；子进程把宿主最后说的（“会话结束了”）转完自己会走
    link.finish();
    Ok(())
}

/// 转接进程退出了、没起来时过多久再起（会话宿主、远程启动的后台进程都这样管它）：一次比一次久（5、10、20、40 秒，最多一分钟），
/// 上一次跑了一分钟以上的从头算
#[derive(Default)]
pub(super) struct Respawn {
    started: f64,
    /// 什么时候再起（宿主也拿它记什么时候再看一眼）
    pub(super) next: f64,
    fails: u32,
}

impl Respawn {
    pub(super) fn started(&mut self, now: f64) {
        self.started = now;
    }

    /// 它退出了（或者没起来）
    pub(super) fn failed(&mut self, now: f64) {
        self.fails = if now - self.started > 60.0 { 0 } else { self.fails + 1 };
        self.next = now + (5.0 * f64::from(1u32 << self.fails.min(4))).min(60.0);
    }
}

/// 转接进程的公共部分（会话的 `cmd_remote_link`、远程启动的 launch.rs 的 `cmd_launcher_link`）：在一台服务器上登记成一个会话，
/// 照配对表允许的配对开一条控制连接（断了按退避再连），配对表变了几秒钟内照新的来；接入方来了（JOIN）分出子进程去接
pub(super) struct Link {
    srv: Server,
    sid: String,
    wake: libc::c_int,
    /// 允许接入的配对（各带配对的密钥）
    pub(super) allowed: Vec<Allowed>,
    /// 配对表上一次的样子（还没看过是 None）、什么时候看的
    stamp: Option<peers::Stamp>,
    checked: f64,
    pub(super) ctl: Option<Ctl>,
    kids: Vec<Kid>,
    /// 最近 10 秒里分出子进程的时候
    forks: VecDeque<f64>,
    /// 控制连接什么时候再连、接连失败了几次
    next_try: f64,
    fails: u32,
}

/// 这一轮等到的（`Link::wait`）
pub(super) struct Woke {
    /// 标准输入上有动静（读它的是调用方）
    pub(super) stdin: bool,
    /// 服务器说有接入方来了（JOIN 的内容；交给 `Link::join`）
    pub(super) joins: Vec<Vec<u8>>,
}

/// 子进程要接的那一个接入方（`Link::join` 交给子进程的）
pub(super) struct Join<'a> {
    srv: &'a Server,
    ctl: &'a Ctl,
    pub(super) sid: &'a str,
    token: &'a str,
    pub(super) a: &'a Allowed,
    /// 分出它的进程：不在了，子进程也该走
    pub(super) parent: libc::pid_t,
}

impl Join<'_> {
    /// 拿令牌另连一条上去，和接入方握手（核对它有配对的密钥）
    pub(super) fn tunnel(&self) -> io::Result<Tunnel> {
        let mut stream = self.srv.dial(self.ctl.port, "join", self.token, &self.ctl.key, 10.0)?;
        let allowed = |pid: &str| (pid == self.a.peer.pid).then(|| (&self.a.peer, self.a.secret.clone()));
        let (chan, _) = relay::handshake_host(&mut stream, self.sid, &self.srv.fp, allowed)?;
        Tunnel::new(stream, chan)
    }
}

impl Link {
    /// 登记成 sid 这个会话（还没连：`connect_if_due`）；收结束的信号
    pub(super) fn new(srv: Server, sid: &str) -> R<Link> {
        let wake = sig::install()?;
        Ok(Link {
            srv,
            sid: sid.to_string(),
            wake,
            allowed: Vec::new(),
            stamp: None,
            checked: f64::NEG_INFINITY,
            ctl: None,
            kids: Vec::new(),
            forks: VecDeque::new(),
            next_try: 0.0,
            fails: 0,
        })
    }

    /// 收走退出了的子进程，交出正常退出的那些的退出码
    pub(super) fn reap(&mut self) -> Vec<i32> {
        let mut codes = Vec::new();
        self.kids.retain(|k| {
            let mut status = 0;
            // SAFETY: status 是有效的；child 是自己分出来的子进程
            let r = unsafe { libc::waitpid(k.child, &mut status, libc::WNOHANG) };
            if r == k.child && libc::WIFEXITED(status) {
                codes.push(libc::WEXITSTATUS(status));
            }
            r == 0
        });
        codes
    }

    /// 隔 PEERS_EVERY 秒看一眼配对表。变了就照新的（allowed_now 算的）来：不该再接着的子进程（不让了、改成只看了、取消配对了）
    /// 结束掉，它们留着等收走；换一条控制连接（新的票据上是新的那几个配对），旧的直接断开，一个都不让了就告诉服务器会话没了。
    /// 这一轮看了是 true（调用方隔一阵要看的别的也跟着看）
    pub(super) fn refresh(&mut self, now: f64, allowed_now: impl FnOnce() -> Vec<Allowed>) -> bool {
        if now - self.checked < PEERS_EVERY {
            return false;
        }
        self.checked = now;
        let s = peers::stamp();
        if self.stamp != Some(s) {
            self.stamp = Some(s);
            let fresh = allowed_now();
            if shape(&fresh) != shape(&self.allowed) {
                let keep = shape(&fresh);
                stop(self.kids.iter().filter(|k| !keep.contains(&(k.pid.clone(), k.access))));
                self.allowed = fresh;
                if let Some(old) = self.ctl.take() {
                    if self.allowed.is_empty() {
                        bye(old);
                    }
                }
                (self.next_try, self.fails) = (now, 0);
            }
        }
        true
    }

    /// 没连着、有允许的配对、到时候了：连上服务器，先说 first 封的（ANNOUNCE）。false：这台机器用不了中转（子进程都结束掉了；
    /// 调用方退出，退出码 EXIT_OFF）
    pub(super) fn connect_if_due(&mut self, now: f64, first: impl FnOnce(&[Allowed]) -> Vec<u8>) -> R<bool> {
        if self.ctl.is_some() || self.allowed.is_empty() || now < self.next_try {
            return Ok(true);
        }
        match connect_as(&self.srv, &self.sid, &self.allowed, first(&self.allowed), self.fails)? {
            Ok(c) => {
                self.ctl = Some(c);
                self.fails = 0;
            }
            Err(Some(wait)) => {
                self.fails += 1;
                self.next_try = now + wait;
            }
            Err(None) => {
                stop(&self.kids);
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// 等一会儿（最多 1 秒）：信号、标准输入、控制连接。收到要结束的信号是 None。控制连接这一轮照常读写（`ctl_step`），
    /// 断了按退避再连
    pub(super) fn wait(&mut self) -> Option<Woke> {
        let ctl_fd = self.ctl.as_ref().map_or(-1, |c| c.stream.as_raw_fd());
        let ctl_out = self.ctl.as_ref().is_some_and(|c| !c.out.is_empty());
        let mut fds = [
            libc::pollfd { fd: self.wake, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: ctl_fd, events: libc::POLLIN | if ctl_out { libc::POLLOUT } else { 0 }, revents: 0 },
        ];
        // SAFETY: fds 是有效的数组；被信号打断就是要它马上回来
        unsafe { libc::poll(fds.as_mut_ptr(), 3, 1000) };
        if fds[0].revents != 0 || sig::pending() {
            let got = sig::take(self.wake);
            if [libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGQUIT].into_iter().any(|s| got.has(s)) {
                return None;
            }
        }
        let mut woke = Woke { stdin: fds[1].revents != 0, joins: Vec::new() };
        if let Some(c) = self.ctl.as_mut() {
            let ready = fds[2].revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0;
            match ctl_step(c, ready) {
                Ok(list) => woke.joins = list.into_iter().filter(|(kind, _)| *kind == relay::F_JOIN).map(|(_, p)| p).collect(),
                Err(()) => {
                    self.ctl = None;
                    self.fails += 1;
                    self.next_try = monotonic() + (2.0 * f64::from(1u32 << self.fails.min(5))).min(60.0);
                }
            }
        }
        Some(woke)
    }

    /// 服务器说有接入方来了（JOIN：令牌、配对编号）：是允许的配对、转接着的没到 MAX_KIDS 个、10 秒里分出的没到 FORKS_PER_10S 个、
    /// admit 也说行（交出给子进程的）时，分出一个子进程做 run（退出码是它返回的）。不然不理：接入方那头等一会儿，服务器告诉它
    /// 这边没来
    pub(super) fn join<T>(&mut self, payload: &[u8], admit: impl FnOnce(&Allowed) -> Option<T>, run: impl FnOnce(&Join, T) -> i32) {
        let Some(ctl) = self.ctl.as_ref() else { return };
        let Some(j) = std::str::from_utf8(payload).ok().and_then(|s| json::loads(s).ok()).and_then(|j| j.as_dict()) else { return };
        let (Some(token), Some(pid)) = (j.str("token").filter(|t| peers::hex32(t)), j.str("pid")) else { return };
        let Some(a) = self.allowed.iter().find(|a| *a.peer.pid == *pid) else { return };
        let now = monotonic();
        while self.forks.front().is_some_and(|t| now - t > 10.0) {
            self.forks.pop_front();
        }
        if self.kids.len() >= MAX_KIDS || self.forks.len() >= FORKS_PER_10S {
            return;
        }
        let Some(given) = admit(a) else { return };
        self.forks.push_back(now);
        let (srv, sid) = (&self.srv, self.sid.as_str());
        let kid = |parent| run(&Join { srv, ctl, sid, token: &token, a, parent }, given);
        if let Some(child) = fork_kid(ctl, kid) {
            self.kids.push(Kid { child, pid: a.peer.pid.clone(), access: a.peer.incoming });
        }
    }

    /// 收场：告诉服务器会话结束了；子进程把手上的做完自己会走，等它们一会儿（3 秒），还没走的结束掉
    pub(super) fn finish(mut self) {
        if let Some(c) = self.ctl.take() {
            bye(c);
        }
        let end = monotonic() + 3.0;
        while !self.kids.is_empty() && monotonic() < end {
            sleep(0.05);
            self.reap();
        }
        stop(&self.kids);
    }
}

/// 为一个接入方分出一个子进程做 run（交给它的是分出它的那个进程的编号：那个进程不在了，子进程也该走），子进程做完直接 _exit，
/// 退出码是 run 返回的。分不出来是 None
fn fork_kid(ctl: &Ctl, run: impl FnOnce(libc::pid_t) -> i32) -> Option<libc::pid_t> {
    // SAFETY: getpid 没有参数、不会失败
    let me = unsafe { libc::getpid() };
    // SAFETY: 这个进程只有一条线程；子进程里只用自己这一份内存、新开的连接，做完直接 _exit（不跑父进程的收尾）
    match unsafe { libc::fork() } {
        0 => {
            sig::reset();
            // SAFETY: 关掉从父进程带来的控制连接（父进程那一份不受影响；不关的话父进程断开它时服务器看不到）。
            // 之后这个子进程不再读写 ctl.stream（只用里面记着的端口和票据密钥），最后是 _exit，不会再关一次
            unsafe { libc::close(ctl.stream.as_raw_fd()) };
            let code = run(me);
            // SAFETY: 子进程到此为止
            unsafe { libc::_exit(code) }
        }
        child if child > 0 => Some(child),
        _ => None,
    }
}
