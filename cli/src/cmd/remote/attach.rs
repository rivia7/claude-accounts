//! 接入端（`remote-attach`，`cca remote attach`）：一个普通的终端程序，把这个终端接到一个在宿主里运行的会话上。
//! 按键原样送过去、那边的输出原样写到这个终端；窗口大小变了告诉宿主。
//!
//! 会话可以在这台机器上（连宿主的 socket），也可以在配对过的另一台机器上（经服务器中转，两头之间端到端加密：
//! `relay.rs`）。这台机器上没有对得上的会话时才去问服务器；之后说的话是同一套帧，只是走的路不一样（`Wire`）。
//!
//! 断开（会话在那边接着运行）：先按 Ctrl-\，再按 d 或者 .；要给会话本身发一个 Ctrl-\ 就连按两次。这个终端被关掉、
//! 这个进程收到 SIGTERM / SIGHUP 也是断开。退出前把会话在这个终端上打开的模式（备用屏幕、括号粘贴、鼠标上报……）关掉。
//!
//! 别的机器上的会话，连接断了（网络断了、这台机器睡了一觉、服务器重启）自己重新接上（`reconnect`）：在终端上说一句，
//! 隔一阵试一次，一共试 `RETRY_FOR` 那么久；接上以后那边照新接入的补上模式和回放。等的时候断开的键照样管用。
//!
//! 退出码：会话结束了是客户端的退出码；自己断开的是 0；64 用法不对或者前缀对上了好几个，65 没有这个会话，
//! 66 经服务器接入没成（连不上服务器、那台机器不在线、对不上配对的密钥），68 宿主没了或者连接断了（没说会话结束
//! 就断了；别的机器上的是试了一阵没重新接上），69 不在终端里。

use super::far::{self, Far};
use super::host::{NOTE_BURST, NOTE_GAP};
use super::keys::Keys;
use super::local::{self, Conn, Decoder, Note, NoteBudget, Status};
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
/// 别的机器上的会话，连接断了以后一共试这么久（秒）。`CCA_REMOTE_RETRY` 能改，0 是不试
const RETRY_FOR: f64 = 300.0;
/// 两次之间最多隔这么久（从 1 秒起一次比一次久）。服务器给每台设备限了流（每 2 秒 1 次），一次试最多两个请求
const RETRY_GAP: f64 = 15.0;
/// 服务器答了、却一直说没有这个会话，这么久以后算它不在了。多半是真的结束了（这台机器睡了一觉，那边早做完了），不让人
/// 干等；留这一阵是因为服务器重启、宿主那头的转接断了一下以后，会话要等那头连上来才又有（头几次是隔 4、10、20 秒试）。
/// 那台机器断网久了（服务器一分钟后就不再留着它的会话）等不到：过后再 `cca remote attach`
const MISSING_FOR: f64 = 45.0;
/// 接上不到 10 秒又断，连着这么多次就不再试
const FLAPS: u32 = 5;

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

    /// 断了的那一条不用了（要另连一条）
    fn close(&mut self) {
        if let Wire::Far(t) = self {
            t.close();
        }
    }
}

/// 别的机器上的会话（配对过才有；配对记在哪台服务器上就问哪台）。没问到的原因写到 stderr、当作那一台上一个都没有
fn far_sessions(all: &[Peer]) -> R<Vec<Far>> {
    if all.is_empty() {
        return Ok(Vec::new());
    }
    let seen = far::fetch_all(all, 8)?;
    for (_, why, asked) in &seen.errors {
        crate::err!("别的机器上的会话{}：{why}", if asked.is_some() { "没查到" } else { "没查" });
    }
    Ok(seen.sessions)
}

/// 这个会话走的那台服务器（它的配对记在哪台上）
fn server_for(f: &Far, all: &[Peer]) -> R<Option<Server>> {
    let Some(peer) = all.iter().find(|p| p.pid == f.pid) else { return Ok(None) };
    Ok(relay::server_of(peer.hub)?.ok())
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
    let far = if near.is_empty() { far_sessions(all)? } else { Vec::new() };
    let far: Vec<Far> = far.into_iter().filter(|f| hit(&f.sid)).collect();
    if let ([], [one]) = (near.as_slice(), far.as_slice()) {
        return Ok(Ok((Target::Far(one.clone()), server_for(one, all)?)));
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

/// 没接上别的机器上的会话：退出码和原因
pub(super) struct Miss {
    pub(super) code: i32,
    pub(super) why: String,
    /// 服务器给了票据，是后面连数据端口、握手没成
    pub(super) dialed: bool,
}

/// 经服务器接到另一台机器上的会话：换票据、连数据端口、握手。什么都不往终端上说（重连时终端在原始模式里）
fn dial_far(f: &Far, peer: &Peer, secret: &[u8], srv: &Server) -> R<Result<Tunnel, Miss>> {
    dial(&f.sid, peer, secret, srv)
}

/// 同 [`dial_far`]：sid 是那台机器上的哪个“会话”（远程启动时是它的启动器，start.rs），peer 是它所在的那个配对
pub(super) fn dial(sid: &str, peer: &Peer, secret: &[u8], srv: &Server) -> R<Result<Tunnel, Miss>> {
    let t = match srv.ticket("relay_attach", &dict! { "sid" => sid, "pid" => peer.pid.as_str() })? {
        Ok(t) => t,
        Err(e) => {
            let code = if e.error == "no_session" { EXIT_NOT_FOUND } else { EXIT_RELAY };
            return Ok(Err(Miss { code, why: e.text, dialed: false }));
        }
    };
    // 服务器要等那台机器另连一条上来才回话
    let made = srv
        .dial(t.port, "view", &t.tid, &t.key, 16.0)
        .and_then(|mut s| Ok((relay::handshake_viewer(&mut s, peer, secret, sid, &srv.fp)?, s)))
        .and_then(|(chan, s)| Tunnel::new(s, chan));
    Ok(made.map_err(|e| {
        let why = if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) {
            "等了一阵那台机器没有回应（它不在线，或者网络不通）".to_string()
        } else {
            e.to_string()
        };
        Miss { code: EXIT_RELAY, why, dialed: true }
    }))
}

enum End {
    /// 会话结束了（客户端的退出码）
    Exited(i32),
    /// 自己断开的；true：终端还在，可以说一句
    Detached(bool),
    HostGone,
    /// 别的机器上的会话，连接断了以后没重新接上（原因）
    Lost(String),
}

/// 这个终端这一头
struct Side {
    tty: RawFd,
    /// 信号来了会有东西可读的那根管道
    wake: RawFd,
    raw: Raw,
    keys: Keys,
    dec: Decoder,
    /// 认出会话在这个终端上打开了哪些模式：退出、重连前关掉
    scan: Scanner,
    modes: Modes,
    /// 还没写到终端上的
    to_tty: VecDeque<u8>,
    read_only: bool,
    /// 别的机器上的会话推来的通知（`NOTIFY`）：等终端上不在一条序列的半中间时写成通知的序列
    notes: VecDeque<Note>,
    note_budget: NoteBudget,
}

/// 一次最多攒几条还没写出去的通知
const NOTES_HELD: usize = 4;

/// 会话里的程序写剪贴板（OSC 52，Claude Code 的「复制」、选中即复制用它）：别的机器上的会话，这边自己也放进这台机器的剪贴板
/// （pbcopy），不靠终端认不认 OSC 52（系统自带的“终端”不认）。宿主只把它给主的那个终端（最后按过键的）；这边再要求刚按过键、点过鼠标
/// （`CLIP_AFTER_INPUT` 秒内）——不然那台机器上的输出能在背后把这台机器的剪贴板换掉（换成一条等着被粘贴的命令）。读剪贴板（`?`）不给
#[derive(Default)]
struct Clip {
    /// 正在攒的那一条（还没结束）
    buf: Option<Vec<u8>>,
    /// 上一次往会话里送按键的时候
    typed: f64,
}

/// 按键、点鼠标以后多久里会话写的剪贴板算数
const CLIP_AFTER_INPUT: f64 = 5.0;
/// 写进剪贴板的最多多少（解出来的字节）
const CLIP_MAX: usize = 1 << 20;

impl Clip {
    /// 会话的输出里的一段（`Scanner` 分好的）；一条写剪贴板的序列完了、算数时给出要放进剪贴板的字
    fn seg(&mut self, class: Class, part: Part, bytes: &[u8], now: f64) -> Option<String> {
        if class != Class::Clip {
            // 自己没带结束符的那一条，是被这一段开头的 ESC 结束的
            let b = self.buf.take()?;
            return self.done(&b, now);
        }
        if part != Part::More {
            self.buf = Some(Vec::new());
        }
        let b = self.buf.as_mut()?;
        if b.len() + bytes.len() > CLIP_MAX * 4 / 3 + 64 {
            self.buf = None; // 太长：不要了
            return None;
        }
        b.extend_from_slice(bytes);
        if b.ends_with(b"\x07") || b.ends_with(b"\x1b\\") || b.ends_with(b"\x9c") {
            let b = self.buf.take()?;
            return self.done(&b, now);
        }
        None
    }

    fn done(&self, seq: &[u8], now: f64) -> Option<String> {
        if now - self.typed > CLIP_AFTER_INPUT {
            return None;
        }
        clip_text(seq)
    }
}

/// 放进这台机器的剪贴板（macOS 的 pbcopy；别的系统靠终端自己认 OSC 52）。另起一个线程：不让会话的输出等它
fn copy_here(text: String) {
    if cfg!(target_os = "macos") {
        std::thread::spawn(move || {
            use crate::py::proc::{Cmd, Out};
            let _ = Cmd::new(["pbcopy"]).input(text.into_bytes()).stdout(Out::Null).stderr(Out::Null).timeout(10.0).run();
        });
    }
}

/// 一条 OSC 52 里要写进剪贴板的字：`ESC ] 52 ; 哪个剪贴板 ; base64 结束符`。问剪贴板里有什么（`?`）、解不开、不是 UTF-8、空的、太长的都不算
fn clip_text(seq: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(seq).ok()?;
    let s = s.strip_prefix("\x1b]")?;
    let s = s.strip_suffix('\x07').or_else(|| s.strip_suffix("\x1b\\")).unwrap_or(s);
    let mut it = s.splitn(3, ';');
    let num = it.next()?;
    if num.trim_start_matches('0') != "52" {
        return None;
    }
    let _which = it.next()?;
    let data = it.next()?;
    if data == "?" || data.is_empty() {
        return None;
    }
    let raw = crate::py::base64::b64decode(data, true).ok()?;
    if raw.is_empty() || raw.len() > CLIP_MAX {
        return None;
    }
    String::from_utf8(raw).ok()
}

/// 通知写成终端认的序列：iTerm2 认 OSC 9（只有一段字），别的（Ghostty、WezTerm、kitty 的新版、cca App 里的终端……）
/// 认 OSC 777 的 notify（标题、正文两段）。字已经过了 `Note::parse`（没有控制字符）；分号是 OSC 777 的分隔，标题里的换成全角的
pub fn note_seq(n: &Note, term_program: &str) -> Vec<u8> {
    let title = if n.subtitle.is_empty() {
        n.title.clone()
    } else if n.title.is_empty() {
        n.subtitle.clone()
    } else {
        format!("{} · {}", n.title, n.subtitle)
    };
    if term_program == "iTerm.app" {
        let text = match (title.is_empty(), n.body.is_empty()) {
            (false, false) => format!("{title}：{}", n.body),
            (false, true) => title,
            _ => n.body.clone(),
        };
        // OSC 9 后面是数字开头时有的终端当成进度条（9;4;…）：前面垫一个全角的空格
        let text = if text.starts_with(|c: char| c.is_ascii_digit()) { format!("\u{3000}{text}") } else { text };
        return format!("\x1b]9;{text}\x1b\\").into_bytes();
    }
    let title = title.replace(';', "；");
    format!("\x1b]777;notify;{title};{}\x1b\\", n.body).into_bytes()
}

/// q 的最后是不是一个字的半截（UTF-8 的多字节字还没完）：这时候插一条序列进去会把这个字拆坏
fn mid_char(q: &VecDeque<u8>) -> bool {
    let tail: Vec<u8> = q.iter().rev().take(4).copied().collect(); // 倒着的最后几个字节
    for (i, b) in tail.iter().enumerate() {
        match b {
            0x80..=0xbf => continue, // 字的后半截：再往前看
            0xc0..=0xdf => return i < 1,
            0xe0..=0xef => return i < 2,
            0xf0..=0xf7 => return i < 3,
            _ => return false,
        }
    }
    false
}

/// 连接断了以后一共试多久（秒）；0 是不试
fn retry_for() -> f64 {
    let set = std::env::var("CCA_REMOTE_RETRY").ok().and_then(|v| v.trim().parse::<f64>().ok());
    set.filter(|v| v.is_finite() && *v >= 0.0).map_or(RETRY_FOR, |v| v.min(86400.0))
}

/// 把攒着的写到终端上，最多等 secs 秒；false：终端没了
fn write_out(tty: RawFd, q: &mut VecDeque<u8>, secs: f64) -> bool {
    let deadline = monotonic() + secs;
    while !q.is_empty() && monotonic() < deadline {
        let mut p = libc::pollfd { fd: tty, events: libc::POLLOUT, revents: 0 };
        // SAFETY: p 是有效的一项
        unsafe { libc::poll(&mut p, 1, 100) };
        match pty::write_some(tty, q.as_slices().0) {
            Ok(n) => {
                q.drain(..n);
            }
            Err(_) => return false,
        }
    }
    true
}

/// 终端在原始模式里时说一行（回车要自己写）；false：终端没了。排在还没写出去的后面（前面那一句、关模式的那几个字节
/// 还没写完时不能插到它们中间）；一秒钟写不完的留着，下一次、接上以后接着写
fn say_raw(side: &mut Side, text: &str) -> bool {
    side.to_tty.extend(format!("{text}\r\n").bytes());
    write_out(side.tty, &mut side.to_tty, 1.0)
}

/// 重连的那一阵等 secs 秒（0：只看一眼）。这期间按了断开的键、终端没了、进程被叫停：Some(怎么完的)；别的按键不要
/// （会话那头收不到，攒着等接上了再送会变成一串迟到的按键）
fn idle(side: &mut Side, secs: f64) -> Option<End> {
    let until = monotonic() + secs;
    let mut buf = [0u8; 1024];
    loop {
        let left = until - monotonic();
        let mut fds = [
            libc::pollfd { fd: side.wake, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: side.tty, events: libc::POLLIN, revents: 0 },
        ];
        // SAFETY: fds 是有效的数组；被信号打断就是要它马上回来
        unsafe { libc::poll(fds.as_mut_ptr(), 2, (left.max(0.0) * 1000.0).ceil().min(1000.0) as i32) };
        if fds[0].revents != 0 || sig::pending() {
            let got = sig::take(side.wake);
            if got.has(libc::SIGHUP) {
                return Some(End::Detached(false));
            }
            if got.has(libc::SIGTERM) || got.has(libc::SIGINT) || got.has(libc::SIGQUIT) {
                return Some(End::Detached(true));
            }
            if got.has(libc::SIGCONT) {
                side.raw.resume();
            }
        }
        if fds[1].revents != 0 {
            // 读到没有为止：问服务器、连的那一阵（阻塞着，可以有半分钟）粘贴进来的一大段也都读掉，不留到接上以后
            loop {
                match pty::read_some(side.tty, &mut buf) {
                    Some(Ok(0)) | Some(Err(_)) => return Some(End::Detached(false)), // 终端没了
                    Some(Ok(n)) if side.keys.feed(&buf[..n], &mut Vec::new()) => return Some(End::Detached(true)),
                    Some(Ok(_)) => {}
                    None => break,
                }
            }
        }
        if left <= 0.0 {
            return None;
        }
    }
}

/// 别的机器上的会话，连接断了：隔一阵试一次，接上了给新的通道（第二项：那边现在是不是只让看）。
/// Err(怎么完的)：用户不等了，或者没接上（`End::Lost`）。
///
/// 每次先问一遍服务器这个会话还在不在：不在了、那台机器没连着服务器时不去换票据；那边改成只让看了也是从这里知道的
/// （转接那头会把按键扔掉，这边得告诉用户）。问、连、握手都是阻塞着做的（合起来最多半分钟），做完马上看一眼
/// 有没有按断开的键、有没有被叫停（SIGTERM、终端关了）：这一步做到一半时收到的，最迟等它做完才看到
fn reconnect(f: &Far, srv: &Server, side: &mut Side, window: f64, flaps: u32) -> Result<(Tunnel, bool), End> {
    let start = monotonic();
    let mut missing: Option<f64> = None;
    let mut said = String::new();
    let mut gap = f64::from(1u32 << flaps.min(4)).min(RETRY_GAP);
    loop {
        if let Some(end) = idle(side, gap) {
            return Err(end);
        }
        gap = (gap * 2.0).min(RETRY_GAP);
        let all = peers::load();
        let (Some(peer), Some(secret)) = (all.iter().find(|p| p.pid == f.pid), peers::secret(&f.pid)) else {
            return Err(End::Lost(format!("这台机器上已经没有和「{}」的配对了", f.peer)));
        };
        let now = monotonic();
        // 这里出的错（连请求都没发成的那一种）不往外抛：终端还在原始模式里、会话打开的模式还没关。当成这一次没问到，下次再试
        let unasked = |_| Err(relay::Fault { code: 0, error: String::new(), text: "这一次没问成服务器".into() });
        let why = match far::fetch(srv, std::slice::from_ref(peer), 8).unwrap_or_else(unasked) {
            Err(fault) => fault.text,
            Ok(l) if !l.known.contains(&f.pid) => {
                return Err(End::Lost(format!("「{}」取消了和这台机器的配对（或者太久没用被服务器清掉了）", f.peer)));
            }
            Ok(l) => match l.sessions.into_iter().find(|x| x.sid == f.sid) {
                None => {
                    if now - *missing.get_or_insert(now) >= MISSING_FOR.min(window) {
                        let why = "那台机器上已经没有这个会话了（它结束了、那边不让这台机器接入了，或者那台机器不在线）";
                        return Err(End::Lost(why.into()));
                    }
                    "那台机器上现在没有这个会话（它可能结束了，也可能那边还没连上服务器）".to_string()
                }
                Some(x) if !x.online => {
                    missing = None;
                    "那台机器暂时没连着服务器".to_string()
                }
                Some(x) if x.busy => {
                    missing = None;
                    "那台机器上的这个会话没有回应（那台机器可能睡着了）".to_string()
                }
                Some(x) => {
                    missing = None;
                    let undialed = |_| Err(Miss { code: EXIT_RELAY, why: "这一次没问成服务器".into(), dialed: false });
                    match dial_far(&x, peer, &secret, srv).unwrap_or_else(undialed) {
                        Ok(t) => {
                            // 问、连的这一阵敲的键也不要（不然接上以后才送到）；按的是断开的键就不接了
                            return idle(side, 0.0).map_or(Ok((t, x.view_only)), Err);
                        }
                        Err(m) => m.why,
                    }
                }
            },
        };
        if let Some(end) = idle(side, 0.0) {
            return Err(end);
        }
        if monotonic() - start >= window {
            return Err(End::Lost(why));
        }
        if why != said {
            if !say_raw(side, &format!("还没接上：{why}。接着试…")) {
                return Err(End::Detached(false));
            }
            said = why;
        }
    }
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
    run(target, srv, read_only, &all)
}

/// 远程启动起好的会话接进来（start.rs：它已经查过是在终端里、等到了服务器上有这个会话）
pub(super) fn attach_launched(f: Far, srv: Server) -> R<()> {
    let all = peers::load();
    run(Target::Far(f), Some(srv), false, &all)
}

/// 接进去、一直到断开或者会话结束：这台机器上的连宿主的 socket，别的机器上的经服务器（srv）。退出码见开头
fn run(target: Target, srv: Option<Server>, mut read_only: bool, all: &[Peer]) -> R<()> {
    let asked_ro = read_only; // 用户要的只看（下面那边只让看时也会改成只看：那一种重连以后照那边现在的来）
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
            let srv = srv.as_ref().expect("别的机器上的会话是问服务器问来的");
            if f.view_only && !read_only {
                crate::err!("「{}」只让这台机器看这个会话，不能操作", f.peer);
                read_only = true;
            }
            let (Some(peer), Some(secret)) = (all.iter().find(|p| p.pid == f.pid), peers::secret(&f.pid)) else {
                crate::err!("和「{}」配对的密钥读不到了：两边重新配对（cca remote pair / cca remote join）", f.peer);
                return exit(EXIT_RELAY);
            };
            crate::err!("正在接入「{}」上的会话 {}…", f.peer, &f.sid[..8]);
            match dial_far(f, peer, &secret, srv)? {
                Ok(t) => Wire::Far(Box::new(t)),
                Err(m) => {
                    crate::err!("{}{}", if m.dialed { "没接上：" } else { "" }, m.why);
                    return exit(m.code);
                }
            }
        }
    };
    pty::adopt_tty(0);
    let Ok(tty) = pty::reopen_tty(0) else { return die("打不开这个终端") };
    let wake = sig::install()?;
    let Some(size) = pty::get_size(tty.as_raw_fd()) else { return die("问不到这个终端的大小") };
    wire.send(&local::hello(size, read_only));
    let raw = Raw::enter(tty.as_raw_fd())?;

    let tty_fd = tty.as_raw_fd();
    let mut side = Side {
        tty: tty_fd,
        wake,
        raw,
        keys: Keys::default(),
        dec: Decoder::default(),
        scan: Scanner::default(),
        modes: Modes::default(),
        to_tty: VecDeque::new(),
        read_only,
        notes: VecDeque::new(),
        note_budget: NoteBudget::new(NOTE_BURST, NOTE_GAP),
    };
    let mut flaps = 0u32;
    let end = loop {
        let up = monotonic();
        let end = attached(&mut side, &mut wire);
        // 别的机器上的会话，连接断了：试着重新接上
        let (End::HostGone, Target::Far(f), Some(srv)) = (&end, &target, srv.as_ref()) else { break end };
        let window = retry_for();
        if window <= 0.0 {
            break end;
        }
        flaps = if monotonic() - up < 10.0 { flaps + 1 } else { 0 };
        if flaps >= FLAPS {
            break End::Lost("连着几次都是一接上就断".into());
        }
        wire.close();
        // 会话那边还没写到终端上的不要了（接上以后有回放）。断的时候可能正写到一条序列的半中间：先让终端回到平常的状态
        // （CAN），把会话打开的模式关掉（鼠标上报还开着的话，等的这一阵动一下鼠标就是一串乱码），再说话。终端一时收不下
        // 的留在 to_tty 里，之后说的话排在它后面（`say_raw`），接上以后也是先把它写完
        side.to_tty.clear();
        side.to_tty.push_back(0x18);
        side.to_tty.extend(side.modes.undo());
        if !say_raw(&mut side, "\r\n连接断了，正在重新接上…（不等了：先按 Ctrl-\\，再按 d）") {
            break End::Detached(false);
        }
        // 那边会照新接入的来（补模式、回放、重画）：这边认序列、记模式的也从头来
        (side.dec, side.scan, side.modes) = (Decoder::default(), Scanner::default(), Modes::default());
        match reconnect(f, srv, &mut side, window, flaps) {
            Ok((t, view_only)) => {
                let note = format!("「{}」现在只让这台机器看这个会话，不能操作", f.peer);
                if view_only && !side.read_only && !say_raw(&mut side, &note) {
                    break End::Detached(false);
                }
                side.read_only = asked_ro || view_only;
                wire = Wire::Far(Box::new(t));
                wire.send(&local::hello(pty::get_size(tty_fd).unwrap_or(size), side.read_only));
            }
            Err(end) => break end,
        }
    };

    // 收场：告诉宿主断开了、把这个终端还原
    if matches!(end, End::Detached(_)) {
        wire.send(&local::frame(local::DETACH, b""));
        wire.drain(1.0);
    }
    let tty_alive = !matches!(end, End::Detached(false));
    if tty_alive {
        side.to_tty.extend(side.modes.undo());
        side.to_tty.extend(b"\r\n".iter().copied());
        write_out(tty_fd, &mut side.to_tty, 1.0);
    }
    drop(side); // 终端的设置还原了再说话
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
        End::Lost(why) => {
            crate::err!("没重新接上：{why}（cca remote list 看它还在不在，再接入：cca remote attach {short}）");
            exit(EXIT_HOST_GONE)
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

/// 接着的那一段：按键送过去、那边的输出写到终端上，到会话结束、自己断开、连接断了为止
fn attached(side: &mut Side, wire: &mut Wire) -> End {
    let Side { tty: tty_fd, wake, raw, keys, dec, scan, modes, to_tty, read_only, notes, note_budget } = side;
    let far = matches!(wire, Wire::Far(_));
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let (tty_fd, wake, read_only) = (*tty_fd, *wake, *read_only);
    let mut buf = vec![0u8; 4096];
    let mut clip = Clip::default();
    'run: loop {
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
                        clip.typed = monotonic();
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
                            if far {
                                if let Some(t) = clip.seg(seg.class, seg.part, bytes, monotonic()) {
                                    copy_here(t);
                                }
                            }
                        });
                        to_tty.extend(payload);
                    }
                    Ok(Some((local::EXIT, payload))) => {
                        let code = payload.get(..4).map_or(1, |b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]));
                        break 'run End::Exited(code);
                    }
                    // 通知：只认别的机器上的会话推来的（这台机器上的，会话里的钩子自己已经弹过了）。那边写的字再过一遍、再限一次流
                    Ok(Some((local::NOTIFY, payload))) if far => {
                        if let Some(n) = Note::parse(&payload) {
                            if notes.len() < NOTES_HELD && note_budget.take() {
                                notes.push_back(n);
                            }
                        }
                    }
                    Ok(Some(_)) => {} // WELCOME、以后加的帧：不用管
                    Ok(None) => break,
                    Err(()) => break 'run End::HostGone,
                }
            }
        }
        // 攒着的通知：会话的输出不在一条序列、一个字的半中间时才插进去（不然会把会话的那一条打断）
        while !notes.is_empty() && scan.idle() && !mid_char(to_tty) {
            if let Some(n) = notes.pop_front() {
                to_tty.extend(note_seq(&n, &term_program));
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
    }
}

#[cfg(test)]
mod note_tests {
    use super::*;

    fn note(title: &str, subtitle: &str, body: &str) -> Note {
        Note::new(title, subtitle, body)
    }

    #[test]
    fn notes_become_terminal_notifications() {
        let n = note("做完了", "alice · site", "第一行");
        assert_eq!(note_seq(&n, ""), "\x1b]777;notify;做完了 · alice · site;第一行\x1b\\".as_bytes());
        assert_eq!(note_seq(&n, "iTerm.app"), "\x1b]9;做完了 · alice · site：第一行\x1b\\".as_bytes());
        // 标题里的分号是 OSC 777 的分隔：换成全角的；OSC 9 数字开头的会被当成进度条：垫一个全角空格
        assert_eq!(note_seq(&note("a;b", "", "c;d"), ""), "\x1b]777;notify;a；b;c;d\x1b\\".as_bytes());
        assert_eq!(note_seq(&note("", "", "4;1"), "iTerm.app"), "\x1b]9;　4;1\x1b\\".as_bytes());
    }

    #[test]
    fn what_a_note_may_say() {
        // 别人写的字：控制字符（ESC、BEL、换行）、改文字方向的去掉，限长度；什么字都没有的不算
        let raw = br#"{"title": "a\u001b]52;c;x\u0007b", "subtitle": "\u202eevil", "body": "line1\nline2"}"#;
        let n = Note::parse(raw).expect("通知");
        assert_eq!((n.title.as_str(), n.subtitle.as_str(), n.body.as_str()), ("a]52;c;xb", "evil", "line1line2"));
        let long = format!(r#"{{"body": "{}"}}"#, "字".repeat(500));
        assert_eq!(Note::parse(long.as_bytes()).expect("通知").body.chars().count(), Note::BODY_MAX);
        assert_eq!(Note::parse(br#"{"title": " ", "body": ""}"#), None);
        assert_eq!(Note::parse(b"not json"), None);
        assert_eq!(Note::parse(&n.to_json()), Some(n));
        // 宿主给的编号：照样留着；不是 16 个小写的十六进制字符的当没有
        let id = |raw: &[u8]| Note::parse(raw).expect("通知").id;
        assert_eq!(id(br#"{"title": "a", "id": "0123456789abcdef"}"#), "0123456789abcdef");
        for bad in ["0123456789ABCDEF", "0123456789abcde", "0123456789abcdefa", "0123456789abcdeg"] {
            assert_eq!(id(format!(r#"{{"title": "a", "id": "{bad}"}}"#).as_bytes()), "", "{bad}");
        }
        let mut with = note("a", "b", "c");
        with.id = "0123456789abcdef".into();
        assert_eq!(Note::parse(&with.to_json()), Some(with.clone()));
        assert!(with.same_text(&note("a", "b", "c")) && !with.same_text(&note("a", "b", "d")));
    }

    #[test]
    fn notes_wait_for_a_whole_character() {
        let q = |b: &[u8]| b.iter().copied().collect::<VecDeque<u8>>();
        assert!(!mid_char(&q(b"")) && !mid_char(&q(b"abc")) && !mid_char(&q("中".as_bytes())));
        assert!(mid_char(&q(&"中".as_bytes()[..2])) && mid_char(&q(&"中".as_bytes()[..1])));
        assert!(mid_char(&q(&"😀".as_bytes()[..3])) && !mid_char(&q("a😀".as_bytes())));
    }

    #[test]
    fn clipboard_writes_from_the_session() {
        use super::{clip_text, Clip, CLIP_AFTER_INPUT};
        use crate::cmd::remote::vt::{Class, Part};
        assert_eq!(clip_text(b"\x1b]52;c;aGk=\x07").as_deref(), Some("hi"));
        assert_eq!(clip_text("\x1b]52;c;5L2g5aW9\x1b\\".as_bytes()).as_deref(), Some("你好"));
        assert_eq!(clip_text(b"\x1b]052;;aGk=\x07").as_deref(), Some("hi"));
        assert_eq!(clip_text(b"\x1b]52;c;aGk="), Some("hi".into()), "没带结束符（被下一个 ESC 结束）的也认");
        for bad in [&b"\x1b]52;c;?\x07"[..], b"\x1b]52;c;\x07", b"\x1b]52;c;!!!\x07", b"\x1b]53;c;aGk=\x07", b"\x1b]52;c;/w==\x07"]
        {
            assert_eq!(clip_text(bad), None, "{:?}", String::from_utf8_lossy(bad));
        }
        // 刚按过键：给；很久没按：不给（那台机器在背后写的）
        let mut c = Clip { typed: 100.0, ..Default::default() };
        assert_eq!(c.seg(Class::Clip, Part::Whole, b"\x1b]52;c;aGk=\x07", 101.0).as_deref(), Some("hi"));
        assert_eq!(c.seg(Class::Clip, Part::Whole, b"\x1b]52;c;aGk=\x07", 100.0 + CLIP_AFTER_INPUT + 1.0), None);
        // 分成几截来的、没带结束符被下一段结束的
        assert_eq!(c.seg(Class::Clip, Part::Head, b"\x1b]52;c;5L2", 101.0), None);
        assert_eq!(c.seg(Class::Clip, Part::More, b"g5aW9", 101.0), None);
        assert_eq!(c.seg(Class::Text, Part::Whole, b"x", 101.0).as_deref(), Some("你好"));
        assert_eq!(c.seg(Class::Text, Part::Whole, b"x", 101.0), None, "只给一次");
    }

    #[test]
    fn the_budget() {
        let mut b = NoteBudget::new(2.0, 3600.0);
        assert!(b.take() && b.take() && !b.take());
    }
}
