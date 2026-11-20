//! 会话宿主（`remote-host`）：客户端（claude / codex / agy）不直接接在用户的终端上，而是跑在这个进程开的 pty 里。
//!
//! cca 里它顶替的是客户端的位置（`run_account` 的前台子进程）：环境、参数、当前目录原样交给客户端，等客户端退出、把它的
//! 退出码当成自己的退出码，所以 cca 后面的收尾什么都不用改。多出来的是：
//! - 启动它的那个终端没了（关了窗口、SSH 断了）会话照样运行；
//! - 别的接入方经 `~/.cca/remote/` 里的 socket 接进来看和操作（`cca remote attach`），后接入的先收到一段回放；
//! - 几个接入方的窗口大小不一样时，按都画得下的大小算。
//!
//! 终端的内容只在内存里（回放留最近的 256 KB），不写文件、不进日志；这个进程自己什么都不往终端上说。
//!
//! 这个进程不碰网络。配对过别的机器时（有配对表），它让一个转接进程（`remote-link`，link.rs）跑着：那个进程连服务器、
//! 把远处的接入方像这台机器上的接入方一样接到 socket 上；它退出了过一会儿再起，会话本身不受影响。

use super::link;
use super::local::{self, Decoder, Meta, Status};
use super::peers;
use super::pty::{self, Raw, Size};
use super::sig;
use super::vt::{replay_bytes, Class, Modes, Part, Ring, Route, Scanner, Seg};
use crate::py::fs;
use crate::py::rand::token_hex;
use crate::py::time::{monotonic, time};
use crate::py::{die, exit, R};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// 回放留多少
const RING_CAP: usize = 256 * 1024;
/// 一次从 pty 读多少
const READ_BUF: usize = 32 * 1024;
/// 启动它的终端还没写出去的超过这么多：没有别的接入方时先不读客户端的输出（和直接运行时一样：终端慢，客户端等着）
const LOCAL_HIGH: usize = 256 * 1024;
/// 有别的接入方时不能让它拖住大家：攒到这么多就把它待写的丢掉，等它写得动了从头回放。要比一次回放（RING_CAP 再加
/// 一点）大得多，不然刚回放进去的又被丢掉，永远追不上
const LOCAL_DROP: usize = 2 * 1024 * 1024;
/// 一个接入方还没写出去的超过这么多：断开它（它再接入时重新回放），不让一个慢的拖住别人
const VIEWER_HIGH: usize = 2 * 1024 * 1024;
/// 还没写进 pty 的输入最多攒这么多
const INPUT_HIGH: usize = 1024 * 1024;
const MAX_VIEWERS: usize = 8;

/// 最近一次输入是谁的（终端的回答也算输入：查询只发给它，免得每个接入方的终端都答一遍）
#[derive(Clone, Copy, PartialEq)]
enum Who {
    Local,
    Viewer(u64),
}

/// 启动它的那个终端
struct Local {
    /// 另开的一份（不阻塞）；终端没了以后是 None
    tty: Option<OwnedFd>,
    raw: Option<Raw>,
    out: VecDeque<u8>,
    size: Size,
    /// 待写的丢过：等能写了从头回放
    resync: bool,
    route: Route,
}

/// 经 socket 接进来的
struct Viewer {
    id: u64,
    stream: UnixStream,
    dec: Decoder,
    out: VecDeque<u8>,
    /// 说过 HELLO 以后才有（之前只能问状态）
    size: Option<Size>,
    read_only: bool,
    /// 不算进窗口大小（`FLAG_PASSIVE`）
    passive: bool,
    dead: bool,
    route: Route,
}

/// 转接进程（`remote-link`）
#[derive(Default)]
struct LinkProc {
    /// 它的标准输入是一根管道，写的那头在这里留着不写：这个进程不在了它读到头，自己退出
    child: Option<Child>,
    started: f64,
    /// 什么时候再看一眼
    next: f64,
    fails: u32,
    /// 这台机器用不了中转（它自己说的）：不再试
    off: bool,
}

fn spawn_link(sid: &str) -> Option<Child> {
    let mut c = Command::new(std::env::current_exe().ok()?);
    c.args(["remote-link", sid]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: fork 之后、exec 之前只调 setsid：自己一个会话，启动的终端关了（SIGHUP）、^Z 把这一组停下都轮不到它
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    c.spawn().ok()
}

struct Host {
    meta: Meta,
    dir: String,
    master: OwnedFd,
    child: libc::pid_t,
    listener: UnixListener,
    sig: RawFd,
    local: Local,
    viewers: Vec<Viewer>,
    next_id: u64,
    /// 还没写进 pty 的输入
    input: VecDeque<u8>,
    scan: Scanner,
    modes: Modes,
    ring: Ring,
    ring_route: Route,
    /// pty 现在的大小
    size: Size,
    primary: Who,
    /// 为了让客户端重画把行数减了一：到这个时候改回来
    restore_at: Option<f64>,
    /// 客户端退出了：(退出码, 什么时候)
    ended: Option<(i32, f64)>,
    master_eof: bool,
    last_output: f64,
    touched: f64,
    link: LinkProc,
}

fn push(q: &mut VecDeque<u8>, data: &[u8]) {
    q.extend(data.iter().copied());
}

/// `Scanner::feed` 交出来的各段（接在一起存着，记着每段到哪为止）一段段取出来
fn pieces<'a>(full: &'a [u8], segs: &'a [(Seg, usize)]) -> impl Iterator<Item = (Seg, &'a [u8])> {
    let mut from = 0;
    segs.iter().map(move |(seg, to)| {
        let bytes = &full[from..*to];
        from = *to;
        (*seg, bytes)
    })
}

/// 把待写的尽量写出去；对面没了是 Err
fn flush(fd: RawFd, q: &mut VecDeque<u8>) -> io::Result<()> {
    while !q.is_empty() {
        let n = pty::write_some(fd, q.as_slices().0)?;
        if n == 0 {
            break;
        }
        q.drain(..n);
    }
    Ok(())
}

impl Host {
    /// 开 pty、开 socket、起客户端。Err：宿主起不来（客户端还没起），调用方照原样直接运行客户端
    fn start(tool: &str, key: &str, name: &str, argv: &[String]) -> io::Result<Host> {
        if !pty::isatty(0) || !pty::isatty(1) {
            return Err(io::Error::other("标准输入、输出不是终端"));
        }
        let size = pty::get_size(0).unwrap_or(Size { cols: 80, rows: 24 });
        let termios = pty::get_termios(0)?;
        let tty = pty::reopen_tty(0)?;
        let (master, slave) = pty::open(Some(&termios), size)?;
        let dir = local::ensure_dir()?;
        let meta = Meta {
            sid: token_hex(16),
            // SAFETY: getpid 没有参数、不会失败
            pid: unsafe { libc::getpid() } as i64,
            tool: tool.to_string(),
            key: key.to_string(),
            name: name.to_string(),
            cwd: std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            since: time(),
        };
        let listener = local::listen(&dir, &format!("{}.sock", local::short(&meta.sid)))?;
        // 先装好信号再起客户端：它一起来就退出的话，SIGCHLD 也不会漏
        let started = sig::install().and_then(|wake| Ok((wake, pty::spawn(argv, slave)?.id() as libc::pid_t)));
        let (wake, child) = match started {
            Ok(x) => x,
            Err(e) => {
                meta.remove(&dir);
                return Err(e);
            }
        };
        let raw = Raw::enter(tty.as_raw_fd()).ok();
        let _ = meta.save(&dir);
        let now = monotonic();
        Ok(Host {
            meta,
            dir,
            master,
            child,
            listener,
            sig: wake,
            local: Local { tty: Some(tty), raw, out: VecDeque::new(), size, resync: false, route: Route::default() },
            viewers: Vec::new(),
            next_id: 1,
            input: VecDeque::new(),
            scan: Scanner::default(),
            modes: Modes::default(),
            ring: Ring::new(RING_CAP),
            ring_route: Route::default(),
            size,
            primary: Who::Local,
            restore_at: None,
            ended: None,
            master_eof: false,
            last_output: now,
            touched: now,
            link: LinkProc::default(),
        })
    }

    fn status(&self) -> Status {
        Status {
            meta: self.meta.clone(),
            size: self.size,
            viewers: self.viewers.iter().filter(|v| v.size.is_some() && !v.dead).count() as i64,
            local: self.local.tty.is_some(),
            busy: false,
        }
    }

    /// 主的那个接入方（查询、写剪贴板、认不出来的序列只发给它，收到的和直接运行时逐字节一样）：最近有输入的那个；
    /// 它不在了就是启动的终端，再不然是最先接入的能操作的。只有只看的接着时没有主的（没人能答）
    fn primary(&self) -> Option<Who> {
        let alive = |w: Who| match w {
            Who::Local => self.local.tty.is_some(),
            Who::Viewer(id) => self.viewers.iter().any(|v| v.id == id && v.size.is_some() && !v.dead && !v.read_only),
        };
        if alive(self.primary) {
            return Some(self.primary);
        }
        if alive(Who::Local) {
            return Some(Who::Local);
        }
        self.viewers.iter().find(|v| v.size.is_some() && !v.dead && !v.read_only).map(|v| Who::Viewer(v.id))
    }

    /// 客户端的一块输出：认出序列，记进回放，分给各个接入方（谁收哪些段见 vt.rs 的 `Class`、`Route`）
    fn on_output(&mut self, data: &[u8]) {
        let mut full = Vec::with_capacity(data.len());
        let mut segs: Vec<(Seg, usize)> = Vec::new(); // (这一段，它在 full 里到哪为止)
        self.scan.feed(data, &mut |seg, bytes| {
            full.extend_from_slice(bytes);
            segs.push((seg, full.len()));
        });
        let pieces = || pieces(&full, &segs);
        let mut out = Vec::new();
        for (seg, bytes) in pieces() {
            if seg.class == Class::Seq && seg.part == Part::Whole {
                self.modes.observe(bytes);
            }
            out.clear();
            self.ring_route.take(seg.class.replayed() && seg.part == Part::Whole, seg, &replay_bytes(seg, bytes), &mut out);
            self.ring.push(&out);
        }
        let primary = self.primary();
        if self.local.tty.is_some() && !self.local.resync {
            out.clear();
            for (seg, bytes) in pieces() {
                self.local.route.take(primary == Some(Who::Local) || seg.class.for_all(), seg, bytes, &mut out);
            }
            push(&mut self.local.out, &out);
        }
        for v in self.viewers.iter_mut().filter(|v| v.size.is_some() && !v.dead) {
            out.clear();
            for (seg, bytes) in pieces() {
                v.route.take(primary == Some(Who::Viewer(v.id)) || seg.class.for_all(), seg, bytes, &mut out);
            }
            if !out.is_empty() {
                push(&mut v.out, &local::frame(local::OUTPUT, &out));
            }
        }
        self.last_output = monotonic();
    }

    /// 接入时（或者待写的丢过以后）补上的那一段：终端的模式、最近的输出，最后是键盘协议的标志
    fn replay(&self) -> Vec<u8> {
        let mut out = self.modes.restore();
        out.extend(self.ring.replay());
        out.extend(self.modes.restore_keys());
        out
    }

    /// 让客户端重画一遍：pty 的行数减一、过一会儿改回来（客户端收到两次窗口大小变了）
    fn repaint(&mut self) {
        if self.size.rows > 2 && self.ended.is_none() {
            pty::set_size(self.master.as_raw_fd(), Size { cols: self.size.cols, rows: self.size.rows - 1 });
            self.restore_at = Some(monotonic() + 0.08);
        }
    }

    /// 按现在的接入方重新定 pty 的大小（都画得下的）。true：变了（客户端会自己重画）
    fn apply_size(&mut self) -> bool {
        let mut want: Option<Size> = self.local.tty.is_some().then_some(self.local.size);
        for v in self.viewers.iter().filter(|v| !v.dead && !v.passive) {
            if let Some(s) = v.size {
                want = Some(want.map_or(s, |w| w.min(s)));
            }
        }
        let Some(want) = want.filter(|w| w.valid()) else { return false };
        if want == self.size && self.restore_at.is_none() {
            return false;
        }
        let changed = want != self.size;
        self.size = want;
        self.restore_at = None;
        pty::set_size(self.master.as_raw_fd(), want);
        changed
    }

    /// 启动它的终端没了：会话照样运行
    fn local_gone(&mut self) {
        if self.local.tty.is_none() {
            return;
        }
        self.local.raw = None; // 还原终端的设置（终端已经没了时改不了，无妨）
        self.local.tty = None;
        self.local.out.clear();
        self.apply_size();
    }

    fn signals(&mut self) {
        let got = sig::take(self.sig);
        let has = |s: libc::c_int| got.has(s);
        if has(libc::SIGHUP) {
            self.local_gone();
        }
        if has(libc::SIGWINCH) {
            if let Some(s) = self.local.tty.as_ref().and_then(|t| pty::get_size(t.as_raw_fd())) {
                self.local.size = s;
                self.apply_size();
            }
        }
        if has(libc::SIGCONT) {
            // 被停过又继续了：终端的设置可能被 shell 改回去了
            if let Some(raw) = self.local.raw.as_mut() {
                raw.resume();
            }
            self.repaint();
        }
        for s in [libc::SIGTERM, libc::SIGINT, libc::SIGQUIT] {
            if has(s) {
                self.forward(s);
            }
        }
    }

    /// 别人发给宿主的信号转给客户端（pty 上的前台进程组；问不到就只给客户端自己）
    fn forward(&self, sig: libc::c_int) {
        // SAFETY: 只是问 pty 的前台进程组、发信号；对方已经没了时出错，不管
        unsafe {
            let pg = libc::tcgetpgrp(self.master.as_raw_fd());
            if pg > 1 {
                libc::killpg(pg, sig);
            } else {
                libc::kill(self.child, sig);
            }
        }
    }

    /// 客户端退出了没有、是不是把自己停了
    fn reap(&mut self) {
        if self.ended.is_some() {
            return;
        }
        let mut status = 0;
        // SAFETY: status 是有效的；child 是自己起的子进程
        let r = unsafe { libc::waitpid(self.child, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if r == self.child {
            if libc::WIFSTOPPED(status) {
                self.suspended();
            } else {
                let code = if libc::WIFSIGNALED(status) { 128 + libc::WTERMSIG(status) } else { libc::WEXITSTATUS(status) };
                self.ended = Some((code, monotonic()));
            }
        } else if r < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD) {
            self.ended = Some((1, monotonic())); // 问不到了：当它已经退出
        }
    }

    /// 客户端把自己停了（^Z）。启动它的终端还在：还原终端、连同 cca 一起停下，shell 把这个任务算作挂起；
    /// `fg` 回来以后让客户端接着跑。终端不在了没人能 `fg`：马上让它接着跑
    fn suspended(&mut self) {
        if self.local.tty.is_some() {
            if let Some(raw) = self.local.raw.as_mut() {
                raw.pause();
            }
            // SAFETY: 给自己这一组（cca 和这个进程）发 SIGTSTP；继续以后从这里接着走
            unsafe { libc::kill(0, libc::SIGTSTP) };
            if let Some(raw) = self.local.raw.as_mut() {
                raw.resume();
            }
        }
        // SAFETY: child 是自己起的子进程
        unsafe { libc::kill(self.child, libc::SIGCONT) };
        self.repaint();
    }

    fn accept(&mut self) {
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if !local::peer_is_me(&stream) || self.viewers.len() >= MAX_VIEWERS || stream.set_nonblocking(true).is_err() {
                        continue; // 丢掉这个连接
                    }
                    let id = self.next_id;
                    self.next_id += 1;
                    self.viewers.push(Viewer {
                        id,
                        stream,
                        dec: Decoder::default(),
                        out: VecDeque::new(),
                        size: None,
                        read_only: false,
                        passive: false,
                        dead: false,
                        route: Route::default(),
                    });
                }
                Err(_) => return,
            }
        }
    }

    /// 一个接入方发来的东西
    fn viewer_input(&mut self, i: usize) {
        let mut buf = [0u8; 8192];
        loop {
            match self.viewers[i].stream.read(&mut buf) {
                Ok(0) => {
                    self.viewers[i].dead = true;
                    break;
                }
                Ok(n) => self.viewers[i].dec.push(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.viewers[i].dead = true;
                    break;
                }
            }
        }
        loop {
            let (kind, payload) = match self.viewers[i].dec.next() {
                Ok(Some(f)) => f,
                Ok(None) => break,
                Err(()) => {
                    self.viewers[i].dead = true;
                    break;
                }
            };
            match kind {
                // 只认头一个 HELLO：之后再说一遍也改不了“只看”（转接那头替别的机器把着这一条，这里再守一道）
                local::HELLO if self.viewers[i].size.is_none() => {
                    let size = local::size_at(&payload, 2);
                    if payload.first() != Some(&local::PROTO) || size.is_none() {
                        self.viewers[i].dead = true;
                        break;
                    }
                    self.viewers[i].size = size;
                    self.viewers[i].read_only = payload[1] & local::FLAG_READ_ONLY != 0;
                    self.viewers[i].passive = payload[1] & local::FLAG_PASSIVE != 0;
                    let changed = self.apply_size();
                    let welcome = local::frame(local::WELCOME, self.status().to_json().as_bytes());
                    let replay = local::frame(local::OUTPUT, &self.replay());
                    let v = &mut self.viewers[i];
                    push(&mut v.out, &welcome);
                    push(&mut v.out, &replay);
                    if !changed {
                        self.repaint();
                    }
                }
                local::INPUT if self.viewers[i].size.is_some() && !self.viewers[i].read_only => {
                    if self.input.len() + payload.len() <= INPUT_HIGH {
                        push(&mut self.input, &payload);
                    }
                    self.primary = Who::Viewer(self.viewers[i].id);
                }
                local::RESIZE if self.viewers[i].size.is_some() => {
                    if let Some(s) = local::size_at(&payload, 0) {
                        self.viewers[i].size = Some(s);
                        self.apply_size();
                    }
                }
                local::DETACH => {
                    self.viewers[i].dead = true;
                    break;
                }
                local::STATUS_REQ => {
                    let st = local::frame(local::STATUS, self.status().to_json().as_bytes());
                    push(&mut self.viewers[i].out, &st);
                }
                _ => {} // 不认识的帧（以后加的）：不理
            }
        }
    }

    /// 配对过别的机器（有配对表）就让转接进程跑着；它退出了过一会儿再起（一次比一次久，最多一分钟）
    fn link_tick(&mut self, now: f64) {
        let l = &mut self.link;
        if l.off || now < l.next {
            return;
        }
        l.next = now + 2.0;
        if let Some(child) = l.child.as_mut() {
            match child.try_wait() {
                Ok(None) => return,
                Ok(Some(st)) if st.code() == Some(link::EXIT_OFF) => l.off = true,
                _ => {
                    l.fails = if now - l.started > 60.0 { 0 } else { l.fails + 1 };
                    l.next = now + (5.0 * f64::from(1u32 << l.fails.min(4))).min(60.0);
                }
            }
            l.child = None;
            return;
        }
        if peers::stamp().is_some() {
            l.child = spawn_link(&self.meta.sid);
            l.started = now;
            if l.child.is_none() {
                // 没起来（辅助程序的文件刚被新版本换掉之类）：和它自己退出一样，过一会儿再试
                l.fails += 1;
                l.next = now + (5.0 * f64::from(1u32 << l.fails.min(4))).min(60.0);
            }
        }
    }

    /// 主循环：到客户端退出、它的输出读完为止。返回客户端的退出码
    fn run(&mut self) -> i32 {
        let mut buf = vec![0u8; READ_BUF];
        loop {
            self.reap();
            let now = monotonic();
            self.link_tick(now);
            if self.restore_at.is_some_and(|t| now >= t) {
                self.restore_at = None;
                pty::set_size(self.master.as_raw_fd(), self.size);
            }
            if let Some((code, at)) = self.ended {
                // 客户端退出以后把它最后的输出读完：pty 关了，或者 0.2 秒没有新的（它起的后台进程可能还开着 pty），最多等 2 秒
                if self.master_eof || now - self.last_output.max(at) > 0.2 || now - at > 2.0 {
                    return code;
                }
            }
            let attached = self.viewers.iter().any(|v| v.size.is_some() && !v.dead);
            // 启动的终端太慢：有别的接入方时不等它（待写的丢掉，等它能写了重新回放）；没有时让客户端等着
            if self.local.out.len() > LOCAL_DROP && attached {
                self.local.out.clear();
                self.local.resync = true;
            }
            if self.local.resync && self.local.out.is_empty() && self.local.tty.is_some() {
                self.local.resync = false;
                self.local.route = Route::default(); // 从头来：正在半路的长序列它没收到开头，后面的也不收
                push(&mut self.local.out, b"\x1bc");
                let replay = self.replay();
                push(&mut self.local.out, &replay);
                self.repaint();
            }
            // 有别的接入方时照读（它们不该等这个终端；它攒得太多了上面会丢掉重来）。客户端退出以后也不再讲究：把 pty 里
            // 剩下的都读出来（就那么多了），收场时再慢慢写给终端，一个字节都不丢
            let read_master = !self.master_eof
                && (attached || self.ended.is_some() || self.local.out.len() <= LOCAL_HIGH || self.local.tty.is_none());

            let mut fds: Vec<libc::pollfd> = Vec::with_capacity(4 + self.viewers.len());
            let pfd = |fd: RawFd, read: bool, write: bool| libc::pollfd {
                fd,
                events: if read { libc::POLLIN } else { 0 } | if write { libc::POLLOUT } else { 0 },
                revents: 0,
            };
            fds.push(pfd(self.sig, true, false));
            // pty 已经没人开着了（客户端关了它却还没退出），或者这一轮既不读也不写它：这一项的 fd 给 -1，poll 不看它
            // （挂断是不问也报的：不这样的话每次都马上回来，空转）
            let idle = !read_master && self.input.is_empty();
            let master = if self.master_eof || idle { -1 } else { self.master.as_raw_fd() };
            fds.push(pfd(master, read_master, !self.input.is_empty()));
            fds.push(pfd(self.listener.as_raw_fd(), true, false));
            // 终端没了以后这一项的 fd 也是 -1
            fds.push(pfd(self.local.tty.as_ref().map_or(-1, |t| t.as_raw_fd()), true, !self.local.out.is_empty()));
            for v in &self.viewers {
                fds.push(pfd(v.stream.as_raw_fd(), true, !v.out.is_empty()));
            }
            let timeout = match (self.ended, self.restore_at) {
                (Some(_), _) => 50,
                (None, Some(t)) => ((t - now) * 1000.0).ceil().clamp(1.0, 1000.0) as i32,
                (None, None) => 1000,
            };
            // SAFETY: fds 是有效的数组；被信号打断（EINTR）就是要它马上回来
            let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
            if n <= 0 {
                if sig::pending() {
                    self.signals(); // 被信号打断的、管道满了没叫醒的
                }
                continue;
            }
            let hit = |i: usize, mask: libc::c_short| fds[i].revents & mask != 0;
            const BAD: libc::c_short = libc::POLLERR | libc::POLLHUP | libc::POLLNVAL;
            if hit(0, libc::POLLIN) {
                self.signals();
            }
            // 客户端的输出
            if hit(1, libc::POLLIN | BAD) && read_master {
                match pty::read_some(self.master.as_raw_fd(), &mut buf) {
                    Some(Ok(0)) | Some(Err(_)) => self.master_eof = true, // 没人开着 pty 了
                    Some(Ok(n)) => {
                        self.on_output(&buf[..n]);
                        self.touch();
                    }
                    None => {}
                }
            }
            if hit(1, libc::POLLOUT) && flush(self.master.as_raw_fd(), &mut self.input).is_err() {
                self.input.clear();
            }
            if hit(2, libc::POLLIN) {
                self.accept();
            }
            // 启动它的终端
            if let Some(tty) = self.local.tty.as_ref().map(|t| t.as_raw_fd()) {
                if hit(3, libc::POLLIN | BAD) {
                    match pty::read_some(tty, &mut buf[..4096]) {
                        Some(Ok(0)) | Some(Err(_)) => self.local_gone(),
                        Some(Ok(n)) => {
                            if self.input.len() + n <= INPUT_HIGH {
                                push(&mut self.input, &buf[..n]);
                            }
                            self.primary = Who::Local;
                            self.touch();
                        }
                        None => {}
                    }
                }
                if self.local.tty.is_some() && hit(3, libc::POLLOUT) && flush(tty, &mut self.local.out).is_err() {
                    self.local_gone();
                }
            }
            // 各个接入方（这一轮 poll 之后才接进来的下一轮再看）
            for i in 0..fds.len() - 4 {
                if hit(4 + i, libc::POLLIN | BAD) {
                    self.viewer_input(i);
                }
            }
            for (i, v) in self.viewers.iter_mut().enumerate() {
                let writable = fds.get(4 + i).is_some_and(|f| f.revents & libc::POLLOUT != 0);
                if !v.dead && (writable || !v.out.is_empty()) && flush(v.stream.as_raw_fd(), &mut v.out).is_err() {
                    v.dead = true;
                }
                if v.out.len() > VIEWER_HIGH {
                    v.dead = true;
                }
            }
            if self.viewers.iter().any(|v| v.dead) {
                self.viewers.retain(|v| !v.dead);
                self.apply_size();
            }
            // 输入马上试着写进去（多半一次就写完，不用再等一轮）
            if !self.input.is_empty() && !self.master_eof && flush(self.master.as_raw_fd(), &mut self.input).is_err() {
                self.input.clear();
            }
        }
    }

    /// 有动静时碰一下说明文件的修改时间（设备统计算闲置用），最多 5 秒一次
    fn touch(&mut self) {
        let now = monotonic();
        if now - self.touched >= 5.0 {
            self.touched = now;
            let _ = fs::utime_now(&self.meta.file(&self.dir, "json"));
        }
    }

    /// 收场：告诉各个接入方会话结束了、把还没写出去的写完（各等最多 1 秒）、还原终端、删掉 socket 和说明
    fn finish(mut self, code: i32) {
        self.meta.remove(&self.dir); // 先删：收场的这一会儿不再有人接进来
        drop(self.link.child.take()); // 管道关了：转接进程告诉服务器会话结束了，把最后的输出转完自己退出（不等它）
                                      // 启动它的终端：只要还写得动就写完（直接运行时客户端也是等终端收完才退出的）；2 秒一个字节都写不进去才算了
        if let Some(tty) = self.local.tty.as_ref().map(|t| t.as_raw_fd()) {
            let mut moved = monotonic();
            while !self.local.out.is_empty() && monotonic() - moved < 2.0 {
                let mut p = libc::pollfd { fd: tty, events: libc::POLLOUT, revents: 0 };
                // SAFETY: p 是有效的一项
                unsafe { libc::poll(&mut p, 1, 100) };
                let before = self.local.out.len();
                if flush(tty, &mut self.local.out).is_err() {
                    break;
                }
                if self.local.out.len() < before {
                    moved = monotonic();
                }
            }
        }
        self.local.raw = None; // 终端先还给用户，再去告诉别的接入方（它们慢的话不耽误这边）
        let bye = local::frame(local::EXIT, &code.to_be_bytes());
        let deadline = monotonic() + 2.0; // 所有接入方合起来最多等这么久
        for v in self.viewers.iter_mut().filter(|v| !v.dead) {
            if v.size.is_some() {
                push(&mut v.out, &bye);
            }
            let left = (deadline - monotonic()).max(0.05);
            let _ = v.stream.set_nonblocking(false);
            let _ = v.stream.set_write_timeout(Some(Duration::from_secs_f64(left)));
            let (a, b) = v.out.as_slices();
            let _ = v.stream.write_all(a).and_then(|()| v.stream.write_all(b));
        }
    }
}

/// `remote-host 工具 账号 名字 -- 客户端 参数…`：在宿主里运行客户端，退出码是客户端的。
/// 宿主起不来（不在终端里、开不了 pty、建不了会话目录）时照原样直接运行客户端：开关开着也不能让会话启动不了
pub fn cmd_remote_host(args: &[String]) -> R<()> {
    let sep = args.iter().position(|a| a == "--");
    let (Some(3), true) = (sep, args.len() >= 5) else {
        return die("用法：remote-host 工具 账号 名字 -- 客户端 参数…");
    };
    let argv = &args[4..];
    match Host::start(&args[0], &args[1], &args[2], argv) {
        Ok(mut host) => {
            let code = host.run();
            host.finish(code);
            exit(code)
        }
        Err(_) => {
            let e = std::process::Command::new(&argv[0]).args(&argv[1..]).exec();
            crate::err!("cca: 运行 {} 失败：{}", argv[0], e);
            exit(if e.kind() == io::ErrorKind::NotFound { 127 } else { 126 })
        }
    }
}
