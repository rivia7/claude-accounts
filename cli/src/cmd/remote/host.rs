//! 会话宿主（`remote-host`）：客户端（claude / codex / agy）不直接接在用户的终端上，而是跑在这个进程开的 pty 里。
//!
//! cca 里它顶替的是客户端的位置（`run_account` 的前台子进程）：环境、参数、当前目录原样交给客户端，等客户端退出、把它的
//! 退出码当成自己的退出码，所以 cca 后面的收尾什么都不用改。多出来的是：
//! - 启动它的那个终端没了（关了窗口、SSH 断了）会话照样运行；
//! - 别的接入方经 `~/.cca/remote/` 里的 socket 接进来看和操作（`cca remote attach`），后接入的先收到一段回放；
//! - 几个接入方的窗口大小不一样时，按都画得下的大小算；画得了大会话的（iOS 上的 App：缩小了画）只在最后打字的是它时算进来。
//!
//! 终端的内容只在内存里（回放留最近的 256 KB），不写文件、不进日志；这个进程自己什么都不往终端上说。
//!
//! 这个进程不碰网络。配对过别的机器时（有配对表），它让一个转接进程（`remote-link`，link.rs）跑着：那个进程连服务器、
//! 把远处的接入方像这台机器上的接入方一样接到 socket 上，会话里推的通知也经它推给配对的手机；它退出了过一会儿再起，会话本身
//! 不受影响。

use super::link;
use super::local::{self, Decoder, Meta, Note, NoteBudget, Status};
use super::peers;
use super::pty::{self, Raw, Size};
use super::sig;
use super::vt::{osc_note_text, replay_bytes, typing, Class, Modes, Part, Ring, Route, Scanner, Seg};
use super::{hook, popup};
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
/// 会话里的钩子推通知：一下子最多几条，之后每几秒一条（Claude Code 一轮做完、等你回答时各推一条，用不了更多）
pub const NOTE_BURST: f64 = 5.0;
pub const NOTE_GAP: f64 = 3.0;
/// 一模一样的一条（标题、副标题、正文都一样）这么久（秒）以内又来了不再推：cca 自己带的钩子（hook.rs）和用户自己照着同样的写法
/// 写的钩子（调 `cca remote notify`）会为同一件事各推一条
const NOTE_SAME: f64 = 10.0;
/// 这台 Mac 上弹通知的 osascript 这么久（秒）还没完就结束掉它
const POPUP_MAX: f64 = 10.0;

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
    /// 画得了和自己的窗口不一样大的会话（`FLAG_ZOOM`）：最后打字的是它时才算进窗口大小；会话的大小告诉它（`SIZE`）
    zoom: bool,
    dead: bool,
    route: Route,
}

/// 转接进程（`remote-link`）
#[derive(Default)]
struct LinkProc {
    /// 它的标准输入是一根管道（写的那头不阻塞）：宿主往里写推出去的通知（`NOTIFY` 帧，它封给配对的手机、经服务器推过去）；
    /// 这个进程不在了它读到头，自己退出
    child: Option<Child>,
    /// 还没写进那根管道的（写不动时攒着，到 `LINK_HIGH` 为止）
    out: VecDeque<u8>,
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
    let child = c.spawn().ok()?;
    if let Some(stdin) = child.stdin.as_ref() {
        pty::set_nonblock(stdin.as_raw_fd()); // 只是写的这一头：它读的那一头照旧
    }
    Some(child)
}

/// 给转接进程的通知还没写出去的到了这么多，新的不再给它（它卡住了：退出以后宿主再起一个）
const LINK_HIGH: usize = 64 * 1024;

/// 一段输出不给谁（`Host::osc_note`）：viewers：接入方（它们收到推出去的那一条通知）；tty：启动它的终端（这台 Mac 上弹过了）
#[derive(Clone, Copy, Default)]
struct Hide {
    viewers: bool,
    tty: bool,
}

/// 无头的会话（`--headless`：远程启动的，launch.rs）：没有启动它的终端，等着手机接进来
struct Headless {
    /// 客户端退出时还没人接入过：留这么久（秒）等人接上来看它最后说了什么（CCA_HEADLESS_LINGER）
    linger: f64,
    /// 一直没人接入：这么久（秒）以后结束会话（CCA_HEADLESS_IDLE）
    idle: f64,
}

/// 环境变量里的秒数（测试里调短）；没有、不是正数用 default
fn env_secs(name: &str, default: f64) -> f64 {
    std::env::var(name).ok().and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(default)
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
    /// 最后打字的（鼠标上报、焦点进出不算：`vt::typing`）。能缩放的接入方只在是它时算进窗口大小
    lead: Who,
    /// 为了让客户端重画把行数减了一：到这个时候改回来
    restore_at: Option<f64>,
    /// 客户端退出了：(退出码, 什么时候)
    ended: Option<(i32, f64)>,
    /// 有人要结束这个会话，已经发过 SIGHUP：到这个时候还没退出就强行结束
    kill_at: Option<f64>,
    master_eof: bool,
    last_output: f64,
    touched: f64,
    link: LinkProc,
    /// 会话里的钩子推通知（`NOTIFY_REQ`）的限流
    notes: NoteBudget,
    /// 上一条推出去的通知和什么时候（NOTE_SAME）
    last_note: Option<(Note, f64)>,
    /// 这台 Mac 上弹通知的 osascript（popup.rs）和什么时候起的：主循环里收尸，POPUP_MAX 秒还没完的结束掉
    popups: Vec<(Child, f64)>,
    /// 无头的会话（启动器起的）才有
    headless: Option<Headless>,
    /// 有人接入过（说过 HELLO）
    ever_attached: bool,
    born: f64,
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
    /// 开 pty、开 socket、起客户端。Err：宿主起不来（客户端还没起），调用方照原样直接运行客户端。
    /// headless：无头的（启动器起的）：没有终端，会话的编号、窗口大小是启动器给的
    fn start(tool: &str, key: &str, name: &str, argv: &[String], headless: Option<(String, Size)>) -> io::Result<Host> {
        let (sid, size, termios, tty) = match &headless {
            Some((sid, size)) => (sid.clone(), *size, None, None),
            None => {
                if !pty::isatty(0) || !pty::isatty(1) {
                    return Err(io::Error::other("标准输入、输出不是终端"));
                }
                let size = pty::get_size(0).unwrap_or(Size { cols: 80, rows: 24 });
                (token_hex(16), size, Some(pty::get_termios(0)?), Some(pty::reopen_tty(0)?))
            }
        };
        let (master, slave) = pty::open(termios.as_ref(), size)?;
        let dir = local::ensure_dir()?;
        let meta = Meta {
            sid,
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
        let raw = tty.as_ref().and_then(|t| Raw::enter(t.as_raw_fd()).ok());
        let _ = meta.save(&dir);
        let now = monotonic();
        Ok(Host {
            meta,
            dir,
            master,
            child,
            listener,
            sig: wake,
            local: Local { tty, raw, out: VecDeque::new(), size, resync: false, route: Route::default() },
            viewers: Vec::new(),
            next_id: 1,
            input: VecDeque::new(),
            scan: Scanner::default(),
            modes: Modes::default(),
            ring: Ring::new(RING_CAP),
            ring_route: Route::default(),
            size,
            primary: Who::Local,
            lead: Who::Local,
            restore_at: None,
            ended: None,
            kill_at: None,
            master_eof: false,
            last_output: now,
            touched: now,
            link: LinkProc::default(),
            notes: NoteBudget::new(NOTE_BURST, NOTE_GAP),
            last_note: None,
            popups: Vec::new(),
            headless: headless
                .map(|_| Headless { linger: env_secs("CCA_HEADLESS_LINGER", 600.0), idle: env_secs("CCA_HEADLESS_IDLE", 900.0) }),
            ever_attached: false,
            born: now,
        })
    }

    /// 无头的会话，客户端退出时还没人接入过：留着（最多 linger 秒）等人接上来看它最后说了什么
    fn lingering(&self, now: f64, at: f64) -> bool {
        self.headless.as_ref().is_some_and(|h| !self.ever_attached && now - at < h.linger)
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
        // 每一段（接入方不给，启动它的终端不给）：Codex 会话里的通知序列换成推出去的通知（`osc_note`）
        let hide: Vec<Hide> = pieces().map(|(seg, bytes)| self.osc_note(seg, bytes)).collect();
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
            for ((seg, bytes), h) in pieces().zip(&hide) {
                self.local.route.take((primary == Some(Who::Local) || seg.class.for_all()) && !h.tty, seg, bytes, &mut out);
            }
            push(&mut self.local.out, &out);
        }
        for v in self.viewers.iter_mut().filter(|v| v.size.is_some() && !v.dead) {
            out.clear();
            for ((seg, bytes), h) in pieces().zip(&hide) {
                v.route.take((primary == Some(Who::Viewer(v.id)) || seg.class.for_all()) && !h.viewers, seg, bytes, &mut out);
            }
            if !out.is_empty() {
                push(&mut v.out, &local::frame(local::OUTPUT, &out));
            }
        }
        self.last_output = monotonic();
    }

    /// Codex 会话的一段输出要是一条通知序列（它的 tui.notifications 写的 OSC 9：codex_launch_args 只开了在等你的那几种）：
    /// 换成一条推出去的通知（`emit_note`），这台 Mac 上弹一个（popup.rs，开着时）。返回这一段不给谁（`Hide`：接入方不给，它们收到
    /// 推出去的那一条，不然要弹两次；启动它的终端不给）。启动它的终端：这台 Mac 上弹着时不给——弹了的、不推的（一轮做完了的那一句：notify 弹过了）、
    /// 刚推过一模一样的、一下子太多的都不给，不然它认得这种序列的话自己再弹一次；不弹（popup 关着、osascript 起不来）时照给。
    /// 别的工具的会话、别的序列原样
    fn osc_note(&mut self, seg: Seg, bytes: &[u8]) -> Hide {
        if self.meta.tool != "codex" || seg.class != Class::Effect || seg.part != Part::Whole {
            return Hide::default();
        }
        let Some(text) = osc_note_text(bytes) else { return Hide::default() };
        let pop = popup::on();
        let Some(n) = hook::codex_osc_note(&text, &self.meta.cwd).and_then(|n| Note::parse(&n.to_json())) else {
            return Hide { viewers: true, tty: pop };
        };
        if !self.emit_note(n.clone()) || !pop {
            return Hide { viewers: true, tty: pop };
        }
        match popup::show(&n) {
            Some(child) => {
                self.popups.push((child, monotonic()));
                Hide { viewers: true, tty: true }
            }
            None => Hide { viewers: true, tty: false },
        }
    }

    /// 推一条通知：交给已经接上的各处（说过 HELLO 的），别的机器上的那边弹出来；转接进程也给一份（封给配对的手机，经服务器
    /// 推过去：锁着屏、App 没开着也收得到）。宿主给每一条一个编号：手机经推送、经接着的会话各收到一份时认得出是同一条。
    /// 刚推过一模一样的（NOTE_SAME）、一下子太多的（`notes`）不推。推了是 true
    fn emit_note(&mut self, mut n: Note) -> bool {
        let now = monotonic();
        let again = self.last_note.as_ref().is_some_and(|(last, at)| last.same_text(&n) && now - at < NOTE_SAME);
        if again || !self.notes.take() {
            return false;
        }
        n.id = token_hex(Note::ID_LEN / 2);
        let f = local::frame(local::NOTIFY, &n.to_json());
        self.last_note = Some((n, now));
        for v in self.viewers.iter_mut().filter(|v| v.size.is_some() && !v.dead) {
            push(&mut v.out, &f);
        }
        let l = &mut self.link;
        if l.child.is_some() && l.out.len() + f.len() <= LINK_HIGH {
            push(&mut l.out, &f);
        }
        true
    }

    /// 弹通知的 osascript：完了的收尸，太久还没完的结束掉
    fn popup_tick(&mut self, now: f64) {
        self.popups.retain_mut(|(child, at)| match child.try_wait() {
            Ok(None) if now - *at < POPUP_MAX => true,
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                false
            }
            _ => false,
        });
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

    /// 按现在的接入方重新定 pty 的大小（`fit`），变了告诉能缩放的那几处。true：变了（客户端会自己重画）
    fn apply_size(&mut self) -> bool {
        let changed = self.fit();
        if changed {
            self.tell_size();
        }
        changed
    }

    /// 定 pty 的大小。普通的终端（启动它的那个、`cca remote attach`、Mac App 里的终端）画不了比自己大的会话：都算进来，列、行
    /// 各取最小。能缩放的（`FLAG_ZOOM`：iOS 上的 App）只在最后打字的是它时算——谁最后打字就照谁的排，手机接上、看着、滚着看，
    /// 电脑上的都不跟着变窄。一个算得上的都没有（只剩手机，都还没打过字）就不动。true：变了
    fn fit(&mut self) -> bool {
        let lead = self.lead;
        let mut want: Option<Size> = self.local.tty.is_some().then_some(self.local.size);
        for v in self.viewers.iter().filter(|v| !v.dead && !v.passive && (!v.zoom || lead == Who::Viewer(v.id))) {
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

    /// 告诉能缩放的接入方会话现在的大小
    fn tell_size(&mut self) {
        let f = local::frame(local::SIZE, &local::size_bytes(self.size));
        for v in self.viewers.iter_mut().filter(|v| v.zoom && v.size.is_some() && !v.dead) {
            push(&mut v.out, &f);
        }
    }

    /// 有人打了字：会话照它排（它是能缩放的接入方时才有变化）
    fn typed(&mut self, who: Who) {
        if self.lead != who {
            self.lead = who;
            self.apply_size();
        }
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

    /// 给客户端那个会话发一个信号：pty 上的前台进程组，加上带头的那个进程（客户端自己；它不在前台进程组里时——
    /// 它起了个前台的子进程——单独给它）
    fn signal_session(&self, sig: libc::c_int) {
        // SAFETY: 只是问进程组、发信号；对方已经没了时出错，不管
        unsafe {
            let pg = libc::tcgetpgrp(self.master.as_raw_fd());
            if pg > 1 {
                libc::killpg(pg, sig);
            }
            if pg <= 1 || libc::getpgid(self.child) != pg {
                libc::kill(self.child, sig);
            }
        }
    }

    /// 这台机器上有人要结束这个会话（`cca remote end`）：照终端被关掉那样给里面的程序发 SIGHUP（停着的让它接着跑，
    /// 不然收不到），`END_GRACE` 秒以后还没退出就强行结束（`run` 里看 `kill_at`）。之后和客户端自己退出一样收场
    fn end(&mut self) {
        if self.ended.is_some() || self.kill_at.is_some() {
            return;
        }
        self.signal_session(libc::SIGHUP);
        self.signal_session(libc::SIGCONT);
        self.kill_at = Some(monotonic() + local::END_GRACE);
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
                        zoom: false,
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
                    self.ever_attached = true;
                    self.viewers[i].read_only = payload[1] & local::FLAG_READ_ONLY != 0;
                    self.viewers[i].passive = payload[1] & local::FLAG_PASSIVE != 0;
                    self.viewers[i].zoom = payload[1] & local::FLAG_ZOOM != 0;
                    let changed = self.fit();
                    let welcome = local::frame(local::WELCOME, self.status().to_json().as_bytes());
                    let replay = local::frame(local::OUTPUT, &self.replay());
                    push(&mut self.viewers[i].out, &welcome);
                    // 能缩放的先知道会话多大，再收回放（照这个大小画的）；大小变了的话别的能缩放的也要知道
                    if changed {
                        self.tell_size();
                    } else if self.viewers[i].zoom {
                        let f = local::frame(local::SIZE, &local::size_bytes(self.size));
                        push(&mut self.viewers[i].out, &f);
                    }
                    push(&mut self.viewers[i].out, &replay);
                    if !changed {
                        self.repaint();
                    }
                }
                local::INPUT if self.viewers[i].size.is_some() && !self.viewers[i].read_only => {
                    if self.input.len() + payload.len() <= INPUT_HIGH {
                        push(&mut self.input, &payload);
                    }
                    let who = Who::Viewer(self.viewers[i].id);
                    self.primary = who;
                    if typing(&payload) {
                        self.typed(who);
                    }
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
                // 这条 socket 上都是这台机器上的同一个用户（别的机器经转接进程来，它不转这一种）
                local::END => self.end(),
                // 会话里的钩子推一条通知（`cca remote notify`、cca 自己带的钩子、Codex 的 notify、agy 的状态栏命令）：推给接着
                // 这个会话的各处（`emit_note`）。启动它的终端不给、宿主也不弹：这台机器上的钩子自己会弹（hook.rs 的 `deliver`）
                local::NOTIFY_REQ => {
                    if let Some(n) = Note::parse(&payload) {
                        self.emit_note(n);
                    }
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
            l.out.clear(); // 是给那根管道的（可能写了半帧）：下一个转接进程从头来
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

    /// 给转接进程的通知写进它的标准输入：写不动的留到下一轮（最多等 1 秒，主循环至少这么久醒一次）；它不在了的不要了
    fn link_flush(&mut self) {
        let l = &mut self.link;
        if l.out.is_empty() {
            return;
        }
        match l.child.as_ref().and_then(|c| c.stdin.as_ref()) {
            Some(stdin) if flush(stdin.as_raw_fd(), &mut l.out).is_ok() => {}
            _ => l.out.clear(),
        }
    }

    /// 主循环：到客户端退出、它的输出读完为止。返回客户端的退出码
    fn run(&mut self) -> i32 {
        let mut buf = vec![0u8; READ_BUF];
        loop {
            self.reap();
            let now = monotonic();
            self.popup_tick(now);
            if self.ended.is_none() && self.kill_at.is_some_and(|t| now >= t) {
                self.kill_at = Some(now + local::END_GRACE); // 万一这一下没结束掉（换了前台进程组），过一阵再来一次
                self.signal_session(libc::SIGKILL);
            }
            self.link_tick(now);
            self.link_flush();
            if self.restore_at.is_some_and(|t| now >= t) {
                self.restore_at = None;
                pty::set_size(self.master.as_raw_fd(), self.size);
            }
            if let Some((code, at)) = self.ended {
                // 客户端退出以后把它最后的输出读完：pty 关了，或者 0.2 秒没有新的（它起的后台进程可能还开着 pty），最多等 2 秒。
                // 无头的会话还没人接入过：再留一阵（lingering），有人接上来、收到回放以后再收场
                let drained = self.master_eof || now - self.last_output.max(at) > 0.2 || now - at > 2.0;
                if drained && !self.lingering(now, at) {
                    return code;
                }
            } else if self.headless.as_ref().is_some_and(|h| !self.ever_attached && now - self.born > h.idle) {
                // 无头的会话一直没人接入：照 cca remote end 那样结束。是自己结束的：客户端退出以后不再留着等人来看
                self.headless = None;
                self.end();
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
                (Some((_, at)), _) if self.lingering(now, at) => 1000,
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
                            if typing(&buf[..n]) {
                                self.typed(Who::Local);
                            }
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
        self.link_flush(); // 最后那一条通知（多半是「做完了」）也交给它；管道里已经有的关了以后它照样读得到
        drop(self.link.child.take()); // 管道关了：转接进程推完、告诉服务器会话结束了，把最后的输出转完自己退出（不等它）
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

/// `列x行`（启动器给的窗口大小）
fn parse_size(s: &str) -> Option<Size> {
    let (c, r) = s.split_once('x')?;
    let size = Size { cols: c.parse().ok()?, rows: r.parse().ok()? };
    size.valid().then_some(size)
}

/// `remote-host [--headless --sid 会话编号 --size 列x行] 工具 账号 名字 -- 客户端 参数…`：在宿主里运行客户端，退出码是客户端的。
/// 宿主起不来（不在终端里、开不了 pty、建不了会话目录）时照原样直接运行客户端：开关开着也不能让会话启动不了。
/// `--headless`：启动器起的（launch.rs），没有终端；起不来就退出（启动器告诉手机没起来）
pub fn cmd_remote_host(args: &[String]) -> R<()> {
    const USAGE: &str = "用法：remote-host [--headless --sid 会话编号 --size 列x行] 工具 账号 名字 -- 客户端 参数…";
    let (headless, args) = match args {
        [h, s, sid, z, size, rest @ ..] if h == "--headless" && s == "--sid" && z == "--size" => {
            let Some(size) = parse_size(size).filter(|_| peers::hex32(sid)) else { return die(USAGE) };
            (Some((sid.clone(), size)), rest)
        }
        [h, ..] if h == "--headless" => return die(USAGE),
        _ => (None, args),
    };
    let sep = args.iter().position(|a| a == "--");
    let (Some(3), true) = (sep, args.len() >= 5) else {
        return die(USAGE);
    };
    let argv = &args[4..];
    let no_tty = headless.is_some();
    match Host::start(&args[0], &args[1], &args[2], argv, headless) {
        Ok(mut host) => {
            let code = host.run();
            host.finish(code);
            exit(code)
        }
        Err(_) if no_tty => exit(1),
        Err(_) => {
            let e = std::process::Command::new(&argv[0]).args(&argv[1..]).exec();
            crate::err!("cca: 运行 {} 失败：{}", argv[0], e);
            exit(if e.kind() == io::ErrorKind::NotFound { 127 } else { 126 })
        }
    }
}
