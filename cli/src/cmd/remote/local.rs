//! 这台机器上的会话目录（`~/.cca/remote/`，700）：每个在宿主里运行的会话一个 socket（`<编号前 12 位>.sock`，600）和一份
//! 说明（`.json`，600：哪个账号、哪个文件夹、什么时候开始；不含密钥，终端内容不落盘）。接入方和宿主在 socket 上说的话是
//! 下面的帧：`类型(1) 长度(4，大端) 内容`。

use super::pty::Size;
use crate::core::base::CCA_HOME;
use crate::dict;
use crate::py::fs;
use crate::py::json;
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::time::monotonic;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::Duration;

/// 帧的类型。接入方 → 宿主
pub const HELLO: u8 = 0x01;
pub const INPUT: u8 = 0x02;
pub const RESIZE: u8 = 0x03;
pub const DETACH: u8 = 0x04;
pub const STATUS_REQ: u8 = 0x06;
/// 结束这个会话（`cca remote end`）：这台机器上的 socket 上说得了；别的机器上的接入方只有能操作、会话又是无头的（远程启动的，
/// `Meta::headless`）时转接那头才替它转（link.rs 的 `bridge`）。只看的说了不算
pub const END: u8 = 0x07;
/// 推一条通知给接着的各处（`cca remote notify`：会话里的钩子说的）。也只有这台机器上的 socket 上说得了，转接那头不转这一种
pub const NOTIFY_REQ: u8 = 0x08;
/// 用一个模板起一个会话（JSON，launch.rs 的 `Ask`）：只在手机和启动器之间的通道上说（转接会话的那头不转这一种）
pub const LAUNCH: u8 = 0x09;
/// 宿主 → 接入方
pub const WELCOME: u8 = 0x11;
pub const OUTPUT: u8 = 0x12;
pub const EXIT: u8 = 0x13;
pub const STATUS: u8 = 0x15;
/// 一条通知（`Note` 的 JSON，宿主清理过、限过长度）：转接那头转给别的机器上的接入方，那边弹出来
pub const NOTIFY: u8 = 0x16;
/// 会话现在的大小（`size_bytes`）：只给说过 `FLAG_ZOOM` 的，接入时说一次（在回放前面），变了再说。转接那头照转
pub const SIZE: u8 = 0x17;
/// 启动器 → 手机：起了没有（JSON：新会话的编号，或者原因的代码和说明），说完就断开
pub const LAUNCHED: u8 = 0x18;

/// 这一版的帧格式
pub const PROTO: u8 = 1;
/// HELLO 的标志：只看、不操作
pub const FLAG_READ_ONLY: u8 = 1;
/// HELLO 的标志：不算进窗口大小。别的机器上只让看的接入方带着它（转接那头加的）：它的窗口多小都不该让会话跟着变，
/// 比会话小的话它那边画不全，是它的事
pub const FLAG_PASSIVE: u8 = 2;
/// HELLO 的标志：画得了和自己的窗口不一样大的会话（缩小了画、拖着看：iOS 上的 cca App）。它的窗口只在它是最后打字的那一处时
/// 算进会话的大小（宿主的 `lead`）：一接上就算它打了字（只看的除外），电脑上又打了字就照电脑排，手机上只是看着、滚着看不抢回来。
/// 会话的大小宿主告诉它（`SIZE`）。旧的宿主不认这一位，照旧算进去；3.7.4～3.7.6 开的会话接上时不照它排，等它打了字
pub const FLAG_ZOOM: u8 = 4;
/// 结束会话（`END`）：里面的程序收到 SIGHUP 以后这么久（秒）还没退出就强行结束
pub const END_GRACE: f64 = 5.0;
/// 一帧最多这么大（多了就是对面乱了，断开）
pub const MAX_FRAME: usize = 1 << 20;

pub fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + payload.len());
    out.push(kind);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// 一条通知：标题、副标题、正文，和宿主给它的编号。字都是别人写的（会话里的程序、别的机器）：`parse` 去掉控制字符和看不见的
/// 排版字符、限长度
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub title: String,
    pub subtitle: String,
    pub body: String,
    /// 宿主推出去时给的编号（`ID_LEN` 个小写的十六进制字符）：手机经推送、经接着的会话各收到一份同一条时认得出来。
    /// 会话里推来的（`NOTIFY_REQ`）没有，有也换掉；旧版的宿主推来的没有（空的）
    pub id: String,
}

impl Note {
    pub const TITLE_MAX: usize = 64;
    pub const BODY_MAX: usize = 240;
    pub const ID_LEN: usize = 16;

    pub fn new(title: impl Into<String>, subtitle: impl Into<String>, body: impl Into<String>) -> Note {
        Note { title: title.into(), subtitle: subtitle.into(), body: body.into(), id: String::new() }
    }

    pub fn to_json(&self) -> Vec<u8> {
        let d = dict! { "title" => self.title.as_str(), "subtitle" => self.subtitle.as_str(), "body" => self.body.as_str() };
        if !self.id.is_empty() {
            d.set("id", self.id.as_str());
        }
        json::dumps(&V::Dict(d), json::LINE).into_bytes()
    }

    /// 帧里的 JSON → 通知；不是 JSON、什么字都没有的不算。编号不是那个样子的不要（当没有）
    pub fn parse(p: &[u8]) -> Option<Note> {
        let d = json::loads(std::str::from_utf8(p).ok()?).ok()?.as_dict()?;
        let get = |k: &str, max: usize| super::far::clean(&d.str(k).unwrap_or_default(), max).trim().to_string();
        let n = Note {
            title: get("title", Note::TITLE_MAX),
            subtitle: get("subtitle", Note::TITLE_MAX),
            body: get("body", Note::BODY_MAX),
            id: d.str("id").filter(|i| Note::id_ok(i)).map(|i| i.to_string()).unwrap_or_default(),
        };
        (!n.title.is_empty() || !n.body.is_empty()).then_some(n)
    }

    /// 编号的样子：`ID_LEN` 个小写的十六进制字符
    pub fn id_ok(id: &str) -> bool {
        id.len() == Note::ID_LEN && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    /// 说的一样（编号不比）
    pub fn same_text(&self, o: &Note) -> bool {
        (&self.title, &self.subtitle, &self.body) == (&o.title, &o.subtitle, &o.body)
    }
}

/// 通知的限流：攒得下 `cap` 条、每 `gap` 秒回一条。会话里的程序、别的机器一个劲地发，也只弹这么多
pub struct NoteBudget {
    left: f64,
    at: f64,
    cap: f64,
    gap: f64,
}

impl NoteBudget {
    pub fn new(cap: f64, gap: f64) -> NoteBudget {
        NoteBudget { left: cap, at: monotonic(), cap, gap }
    }

    pub fn take(&mut self) -> bool {
        let now = monotonic();
        self.left = (self.left + (now - self.at) / self.gap).min(self.cap);
        self.at = now;
        if self.left >= 1.0 {
            self.left -= 1.0;
            true
        } else {
            false
        }
    }
}

pub fn hello(size: Size, read_only: bool) -> Vec<u8> {
    let mut p = vec![PROTO, if read_only { FLAG_READ_ONLY } else { 0 }];
    p.extend_from_slice(&size_bytes(size));
    frame(HELLO, &p)
}

pub fn resize(size: Size) -> Vec<u8> {
    frame(RESIZE, &size_bytes(size))
}

/// 帧里的大小：列数、行数，各两个字节（大端）。HELLO（从第 2 个字节起）、RESIZE、SIZE 里都是这样
pub fn size_bytes(size: Size) -> [u8; 4] {
    let (c, r) = (size.cols.to_be_bytes(), size.rows.to_be_bytes());
    [c[0], c[1], r[0], r[1]]
}

/// HELLO、RESIZE 里的大小（从第 at 个字节起的 4 个字节）
pub fn size_at(payload: &[u8], at: usize) -> Option<Size> {
    let b = payload.get(at..at + 4)?;
    let s = Size { cols: u16::from_be_bytes([b[0], b[1]]), rows: u16::from_be_bytes([b[2], b[3]]) };
    s.valid().then_some(s)
}

/// 把收到的字节拼成帧
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
    /// buf 里这之前的已经交出去了。不每交一帧就挪一次：一大块里全是几个字节的小帧时，那样是平方级的
    at: usize,
}

impl Decoder {
    pub fn push(&mut self, data: &[u8]) {
        if self.at > 0 {
            self.buf.drain(..self.at);
            self.at = 0;
        }
        self.buf.extend_from_slice(data);
    }

    /// 下一帧；还不够是 Ok(None)；长度不对是 Err（断开）
    pub fn next(&mut self) -> Result<Option<(u8, Vec<u8>)>, ()> {
        let rest = &self.buf[self.at..];
        if rest.len() < 5 {
            return Ok(None);
        }
        let len = u32::from_be_bytes([rest[1], rest[2], rest[3], rest[4]]) as usize;
        if len > MAX_FRAME {
            return Err(());
        }
        if rest.len() < 5 + len {
            return Ok(None);
        }
        let frame = (rest[0], rest[5..5 + len].to_vec());
        self.at += 5 + len;
        Ok(Some(frame))
    }
}

/// 把队列里待写的尽量写到一条不阻塞的连接上（写不动了就先停）；连接没了是 Err
pub fn write_queue(w: &mut impl Write, q: &mut VecDeque<u8>) -> io::Result<()> {
    while !q.is_empty() {
        match w.write(q.as_slices().0) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => {
                q.drain(..n);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

pub fn dir() -> String {
    path::join(&CCA_HOME, "remote")
}

/// 建好会话目录：700、是自己的、不是链接（别人的、是链接的不用：里面的 socket 能往会话里敲键盘）
pub fn ensure_dir() -> io::Result<String> {
    let d = dir();
    fs::makedirs_mode(&d, 0o700)?;
    let m = std::fs::symlink_metadata(&d)?;
    // SAFETY: geteuid 没有参数、不会失败
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } {
        return Err(io::Error::other(format!("{d} 不是自己的目录")));
    }
    if m.mode() & 0o777 != 0o700 {
        fs::chmod(&d, 0o700)?;
    }
    Ok(d)
}

/// 文件名里用的那一截编号
pub fn short(sid: &str) -> &str {
    &sid[..sid.len().min(12)]
}

/// socket 地址最长 104 个字节（macOS；Linux 是 108）：`~/.cca` 很深时放不下，先进到目录里再用相对路径
const SUN_MAX: usize = 100;

/// 进到 dir 里做一件事再回来（当前目录是整个进程的：只在还没有别的线程、没起子进程的时候用）。
/// 先试一下回不回得来（当前目录读得了却进不去时 fchdir 会失败）：回不来就不去——客户端要在原来的目录里启动
fn in_dir<T>(dir: &str, f: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let here = File::open(".")?;
    // SAFETY: here 开着，是一个目录
    let back = || if unsafe { libc::fchdir(here.as_raw_fd()) } == 0 { Ok(()) } else { Err(io::Error::last_os_error()) };
    back()?;
    std::env::set_current_dir(dir)?;
    let r = f();
    back()?;
    r
}

/// 开会话的 socket（600，不阻塞）。权限是建好以后改的，不动 umask（它是整个进程的：别的线程这时建的文件、目录也会
/// 跟着变）；建好到改好之间别的用户也碰不到它，会话目录本身只有自己进得去
pub fn listen(dir: &str, name: &str) -> io::Result<UnixListener> {
    let full = path::join(dir, name);
    let _ = std::fs::remove_file(&full);
    let l = if full.len() < SUN_MAX { UnixListener::bind(&full) } else { in_dir(dir, || UnixListener::bind(name)) }?;
    if let Err(e) = fs::chmod(&full, 0o600).and_then(|()| l.set_nonblocking(true)) {
        let _ = std::fs::remove_file(&full);
        return Err(e);
    }
    Ok(l)
}

pub fn connect(dir: &str, name: &str) -> io::Result<UnixStream> {
    let full = path::join(dir, name);
    if full.len() < SUN_MAX {
        UnixStream::connect(&full)
    } else {
        in_dir(dir, || UnixStream::connect(name))
    }
}

/// 接在宿主 socket 上的一条不阻塞的连接：`send` 排队、`flush` 写出去、`recv` 读现在有的（和中转那头的
/// `relay::Tunnel` 一个用法：接入端、宿主那头的转接两边都用）
pub struct Conn {
    stream: UnixStream,
    out: VecDeque<u8>,
}

impl Conn {
    pub fn new(stream: UnixStream) -> io::Result<Conn> {
        stream.set_nonblocking(true)?;
        Ok(Conn { stream, out: VecDeque::new() })
    }

    pub fn fd(&self) -> RawFd {
        self.stream.as_raw_fd()
    }

    /// 还没写出去的有多少
    pub fn pending(&self) -> usize {
        self.out.len()
    }

    pub fn send(&mut self, data: &[u8]) {
        self.out.extend(data.iter().copied());
    }

    pub fn flush(&mut self) -> io::Result<()> {
        write_queue(&mut self.stream, &mut self.out)
    }

    /// 读现在有的（可能是空的）；Ok(None) 是对面关了、之前读到的都交出去了。读到一半出了错：先把已经读到的交出去
    /// （对面说完“会话结束了”马上断开时，那一句不能丢），错误下一次再报
    pub fn recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut got = Vec::new();
        let mut buf = [0u8; 32 * 1024];
        loop {
            match self.stream.read(&mut buf) {
                Ok(0) if got.is_empty() => return Ok(None),
                Ok(0) => break,
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) if !got.is_empty() => break,
                Err(e) => return Err(e),
            }
            if got.len() >= 1024 * 1024 {
                break; // 先交出去这些
            }
        }
        Ok(Some(got))
    }

    /// 收场：把还没写出去的写完（最多等 secs 秒）
    pub fn drain(&mut self, secs: f64) {
        let end = monotonic() + secs;
        while !self.out.is_empty() && monotonic() < end {
            let mut p = libc::pollfd { fd: self.fd(), events: libc::POLLOUT, revents: 0 };
            // SAFETY: p 是有效的一项
            unsafe { libc::poll(&mut p, 1, 100) };
            if self.flush().is_err() {
                break;
            }
        }
    }
}

/// 连上来的是不是同一个用户（目录和 socket 的权限已经只给自己，这里再核对一遍）
pub fn peer_is_me(s: &UnixStream) -> bool {
    // SAFETY: geteuid 没有参数、不会失败
    let me = unsafe { libc::geteuid() };
    #[cfg(target_os = "linux")]
    {
        // SAFETY: cred、len 是有效的，SO_PEERCRED 往里写
        unsafe {
            let mut cred: libc::ucred = std::mem::zeroed();
            let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
            let rc = libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&raw mut cred).cast(), &mut len);
            rc == 0 && cred.uid == me
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let (mut uid, mut gid): (libc::uid_t, libc::gid_t) = (0, 0);
        // SAFETY: uid、gid 是有效的，getpeereid 往里写
        unsafe { libc::getpeereid(s.as_raw_fd(), &mut uid, &mut gid) == 0 && uid == me }
    }
}

/// 一个在宿主里运行的会话
#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    /// 32 个十六进制字符
    pub sid: String,
    /// 宿主的进程号
    pub pid: i64,
    pub tool: String,
    /// 账号表的键
    pub key: String,
    /// 账号的短名字（别名，没有就是邮箱）
    pub name: String,
    pub cwd: String,
    pub since: f64,
    /// 无头的（远程启动的：启动器起的，这台机器上没有它的终端）：配对的手机、电脑能操作的话也结束得了它（`END`）。
    /// 说明里只有是的才写这一项（3.7.11 及以前的宿主写的没有：当它不是）
    pub headless: bool,
}

impl Meta {
    pub fn to_dict(&self) -> Dict {
        let d = dict! {
            "v" => 1, "sid" => self.sid.as_str(), "pid" => self.pid, "tool" => self.tool.as_str(), "key" => self.key.as_str(),
            "name" => self.name.as_str(), "cwd" => self.cwd.as_str(), "since" => self.since
        };
        if self.headless {
            d.set("headless", true);
        }
        d
    }

    pub fn from_dict(d: &Dict) -> Option<Meta> {
        let s = |k: &str| d.str(k).map(|x| x.to_string());
        let sid = s("sid")?;
        if d.at("v").as_int() != Some(1) || sid.len() != 32 || !sid.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        Some(Meta {
            sid,
            pid: d.at("pid").as_int()?,
            tool: s("tool")?,
            key: s("key")?,
            name: s("name")?,
            cwd: s("cwd")?,
            since: d.at("since").as_num()?,
            headless: d.get("headless").and_then(|v| v.as_bool()).unwrap_or(false),
        })
    }

    pub fn file(&self, dir: &str, ext: &str) -> String {
        path::join(dir, &format!("{}.{}", short(&self.sid), ext))
    }

    pub fn save(&self, dir: &str) -> crate::py::R<()> {
        fs::save_json(&self.file(dir, "json"), &V::Dict(self.to_dict()), Some(0o600), true)
    }

    /// 宿主退出时：socket 和说明都删掉
    pub fn remove(&self, dir: &str) {
        let _ = std::fs::remove_file(self.file(dir, "sock"));
        let _ = std::fs::remove_file(self.file(dir, "json"));
    }
}

/// 宿主现在的状态（STATUS 帧的内容）
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub meta: Meta,
    pub size: Size,
    /// 经 socket 接入的有几个
    pub viewers: i64,
    /// 启动它的那个终端还在不在
    pub local: bool,
    /// 没问到（宿主没回应）：上面三项不知道，只有说明文件里的那些
    pub busy: bool,
}

impl Status {
    pub fn to_json(&self) -> String {
        let d = self.meta.to_dict();
        d.set("cols", self.size.cols as i64);
        d.set("rows", self.size.rows as i64);
        d.set("viewers", self.viewers);
        d.set("local", self.local);
        json::dumps(&V::Dict(d), json::LINE)
    }

    pub fn from_json(s: &str) -> Option<Status> {
        let d = json::loads(s).ok()?.as_dict()?;
        Some(Status {
            meta: Meta::from_dict(&d)?,
            size: Size { cols: d.at("cols").as_int()? as u16, rows: d.at("rows").as_int()? as u16 },
            viewers: d.at("viewers").as_int()?,
            local: d.at("local").as_bool()?,
            busy: false,
        })
    }
}

/// 问一个宿主的结果
#[derive(Debug, PartialEq)]
pub enum Asked {
    Alive(Status),
    /// 连上了却没答、答的看不懂，或者连的时候出了别的错：它多半还在（被挂起了、机器正忙），不能当它没了
    Busy,
    /// 确实没人在听这个 socket 了（宿主被强行结束留下的文件）
    Gone,
}

/// 问一个宿主它现在的状态（半秒没回就算没答）
pub fn ask_status(dir: &str, sock: &str) -> Asked {
    let mut s = match connect(dir, sock) {
        Ok(s) => s,
        Err(e) if matches!(e.raw_os_error(), Some(libc::ECONNREFUSED | libc::ENOENT)) => return Asked::Gone,
        Err(_) => return Asked::Busy,
    };
    let mut talk = || -> Option<Status> {
        s.set_read_timeout(Some(Duration::from_millis(500))).ok()?;
        s.set_write_timeout(Some(Duration::from_millis(500))).ok()?;
        s.write_all(&frame(STATUS_REQ, b"")).ok()?;
        let mut dec = Decoder::default();
        let mut buf = [0u8; 4096];
        loop {
            match dec.next().ok()? {
                Some((STATUS, payload)) => return Status::from_json(std::str::from_utf8(&payload).ok()?),
                Some(_) => continue,
                None => {}
            }
            let n = s.read(&mut buf).ok()?;
            if n == 0 {
                return None;
            }
            dec.push(&buf[..n]);
        }
    };
    talk().map_or(Asked::Busy, Asked::Alive)
}

/// 各个宿主最后一次有动静的时间（宿主的进程号 → 说明文件的修改时间：宿主收到输入、客户端有输出时碰它，最多 5 秒一次）。
/// 设备统计算闲置用（`presence_scan`）：只读文件，不去问宿主
pub fn activity() -> std::collections::HashMap<i64, f64> {
    activity_in(&dir())
}

/// `activity`，会话目录由调用方给
pub fn activity_in(d: &str) -> std::collections::HashMap<i64, f64> {
    let mut out = std::collections::HashMap::new();
    let Ok(names) = fs::listdir_sorted(d) else { return out };
    for n in names {
        let Some(stem) = n.strip_suffix(".json") else { continue };
        let file = path::join(d, &n);
        let meta = fs::load_quiet(&file).and_then(|j| j.as_dict()).and_then(|j| Meta::from_dict(&j));
        let Some(meta) = meta.filter(|m| short(&m.sid) == stem) else { continue }; // peers.json 这类不是会话的说明
        if let Ok(st) = std::fs::metadata(&file) {
            out.insert(meta.pid, crate::py::shutil::st_mtime(&st));
        }
    }
    out
}

/// 这台机器上现在在宿主里运行的会话（按开始的先后）。clean：宿主已经不在了的，把它留下的 socket 和说明删掉
pub fn sessions(clean: bool) -> Vec<Status> {
    sessions_in(&dir(), clean)
}

/// `sessions`，会话目录由调用方给。只有确实没人在听的才算不在了：没答的（宿主被挂起、机器正忙）照说明文件列出来、
/// 标成没回应，它的文件不动——删了的话这个会话就再也接不上了
pub fn sessions_in(d: &str, clean: bool) -> Vec<Status> {
    let Ok(names) = fs::listdir_sorted(d) else { return Vec::new() };
    let mut out = Vec::new();
    for n in names {
        let Some(stem) = n.strip_suffix(".sock") else { continue };
        let json = path::join(d, &format!("{stem}.json"));
        match ask_status(d, &n) {
            Asked::Alive(st) if short(&st.meta.sid) == stem => out.push(st),
            Asked::Alive(_) => {} // 答的不是这个会话：不认，也不动它
            Asked::Busy => {
                let meta = fs::load_quiet(&json).and_then(|j| j.as_dict()).and_then(|j| Meta::from_dict(&j));
                if let Some(meta) = meta.filter(|m| short(&m.sid) == stem) {
                    out.push(Status { meta, size: Size { cols: 0, rows: 0 }, viewers: 0, local: false, busy: true });
                }
            }
            Asked::Gone if clean => {
                let _ = std::fs::remove_file(path::join(d, &n));
                let _ = std::fs::remove_file(&json);
            }
            Asked::Gone => {}
        }
    }
    out.sort_by(|a, b| a.meta.since.total_cmp(&b.meta.since));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> Meta {
        Meta {
            sid: "0123456789abcdef0123456789abcdef".into(),
            pid: 4242,
            tool: "claude".into(),
            key: "工作(alice@example.com)".into(),
            name: "工作".into(),
            cwd: "/tmp/项目".into(),
            since: 1794000000.5,
            headless: false,
        }
    }

    #[test]
    fn frames() {
        let mut d = Decoder::default();
        let mut bytes = hello(Size { cols: 120, rows: 40 }, true);
        bytes.extend(frame(INPUT, "你好".as_bytes()));
        bytes.extend(resize(Size { cols: 80, rows: 24 }));
        // 一个字节一个字节地到
        let mut got = Vec::new();
        for b in &bytes {
            d.push(&[*b]);
            while let Some(f) = d.next().unwrap() {
                got.push(f);
            }
        }
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].0, HELLO);
        assert_eq!((got[0].1[0], got[0].1[1] & FLAG_READ_ONLY), (PROTO, 1));
        assert_eq!(size_at(&got[0].1, 2), Some(Size { cols: 120, rows: 40 }));
        assert_eq!(got[1], (INPUT, "你好".as_bytes().to_vec()));
        assert_eq!(size_at(&got[2].1, 0), Some(Size { cols: 80, rows: 24 }));
        // 大小是 0、内容不够长：不认
        assert_eq!(size_at(&[0, 0, 0, 24], 0), None);
        assert_eq!(size_at(&[0, 80], 0), None);
        // 长度离谱：断开
        let mut d = Decoder::default();
        d.push(&[OUTPUT, 0x7f, 0xff, 0xff, 0xff]);
        assert!(d.next().is_err());
        // 一大块里全是小帧（一兆、二十万帧），中间还断在半帧上：一帧不少，也不慢（逐帧挪缓冲区的话这里要好几秒）
        let small = frame(RESIZE, &[]);
        let block: Vec<u8> = small.iter().copied().cycle().take(1024 * 1024).collect();
        let (mut d, mut n) = (Decoder::default(), 0);
        let started = std::time::Instant::now();
        for part in [&block[..700_003], &block[700_003..]] {
            d.push(part);
            while let Some((kind, payload)) = d.next().unwrap() {
                assert!(kind == RESIZE && payload.is_empty());
                n += 1;
            }
        }
        assert_eq!(n, 1024 * 1024 / small.len());
        assert!(started.elapsed().as_secs_f64() < 2.0);
    }

    #[test]
    fn meta_and_status_round_trip() {
        let m = meta();
        assert_eq!(Meta::from_dict(&m.to_dict()), Some(m.clone()));
        let st = Status { meta: m.clone(), size: Size { cols: 100, rows: 30 }, viewers: 2, local: false, busy: false };
        assert_eq!(Status::from_json(&st.to_json()), Some(st));
        // 无头的（远程启动的）记一笔，STATUS 里也有（转接进程照它放不放行 END）；不是的不写这一项（和以前的说明一样）
        assert!(m.to_dict().get("headless").is_none());
        let h = Meta { headless: true, ..m.clone() };
        assert_eq!(h.to_dict().at("headless").as_bool(), Some(true));
        assert_eq!(Meta::from_dict(&h.to_dict()), Some(h.clone()));
        let st = Status { meta: h.clone(), size: Size { cols: 100, rows: 30 }, viewers: 0, local: false, busy: false };
        assert_eq!(Status::from_json(&st.to_json()).map(|s| s.meta.headless), Some(true));
        // 编号不是 32 个十六进制字符、版本不对：不认
        let d = m.to_dict();
        d.set("sid", "../../etc");
        assert_eq!(Meta::from_dict(&d), None);
        let d = m.to_dict();
        d.set("v", 2);
        assert_eq!(Meta::from_dict(&d), None);
        assert_eq!(short(&m.sid), "0123456789ab");
    }

    #[test]
    fn socket_is_private_and_answers_status() {
        let tmp = crate::py::fs::gettempdir();
        // 名字要短：整个路径超过 socket 地址的长度就会走“先进到目录里”那条路，而当前目录是整个进程的，别的测试正在跑
        let d = path::join(&tmp, &format!("ccar-{}", crate::py::rand::token_hex(4)));
        std::fs::create_dir(&d).unwrap();
        let m = meta();
        let name = format!("{}.sock", short(&m.sid));
        let l = listen(&d, &name).unwrap();
        let mode = std::fs::symlink_metadata(path::join(&d, &name)).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let st = Status { meta: m, size: Size { cols: 90, rows: 20 }, viewers: 1, local: true, busy: false };
        let reply = frame(STATUS, st.to_json().as_bytes());
        let server = std::thread::spawn(move || {
            l.set_nonblocking(false).unwrap();
            let (mut s, _) = l.accept().unwrap();
            assert!(peer_is_me(&s));
            let mut dec = Decoder::default();
            let mut buf = [0u8; 64];
            loop {
                let n = s.read(&mut buf).unwrap();
                dec.push(&buf[..n]);
                if let Some((STATUS_REQ, _)) = dec.next().unwrap() {
                    break;
                }
            }
            s.write_all(&reply).unwrap();
        });
        assert_eq!(ask_status(&d, &name), Asked::Alive(st));
        server.join().unwrap();
        // 没人听的 socket（宿主的线程收场了、文件还在）：确实不在了。别的测试这时正好在起子进程的话，那个子进程 exec 之前
        // 还带着这个监听的 socket（测试进程是多线程的；辅助程序自己不是），要过一瞬间才真的没人听：等它过去
        let mut asked = ask_status(&d, &name);
        for _ in 0..40 {
            if asked == Asked::Gone {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
            asked = ask_status(&d, &name);
        }
        assert_eq!(asked, Asked::Gone);
        assert_eq!(ask_status(&d, "nothing.sock"), Asked::Gone);
        // 设备统计用的：宿主的进程号 → 说明文件的修改时间；不是会话说明的文件（配对表）不算
        let m = meta();
        assert!(m.save(&d).is_ok());
        std::fs::write(path::join(&d, "peers.json"), "{\"v\": 1, \"peers\": {}}").unwrap();
        let act = activity_in(&d);
        assert_eq!(act.keys().copied().collect::<Vec<_>>(), vec![m.pid]);
        assert!((act[&m.pid] - crate::py::time::time()).abs() < 60.0, "{act:?}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// 宿主没回应（被挂起了、机器正忙）不等于不在了：照说明文件列出来，它的文件一个都不删；确实没人听的才清掉
    #[test]
    fn a_silent_host_is_listed_and_its_files_are_kept() {
        let tmp = crate::py::fs::gettempdir();
        // 名字要短：整个路径超过 socket 地址的长度就会走“先进到目录里”那条路，而当前目录是整个进程的，别的测试正在跑
        let d = path::join(&tmp, &format!("ccar-{}", crate::py::rand::token_hex(4)));
        std::fs::create_dir(&d).unwrap();
        let (silent, dead) = (meta(), Meta { sid: "ffffffffffffffffffffffffffffffff".into(), since: 1794000001.0, ..meta() });
        // 开着却从不回答的
        let quiet = listen(&d, &format!("{}.sock", short(&silent.sid))).unwrap();
        silent.save(&d).unwrap();
        // 被强行结束留下的：socket 文件在，没人听
        drop(listen(&d, &format!("{}.sock", short(&dead.sid))).unwrap());
        dead.save(&d).unwrap();
        let files = |d: &str| fs::listdir_sorted(d).unwrap();
        assert_eq!(files(&d).len(), 4);

        // 不清理：只列出没回应的那个，四个文件都在
        let got = sessions_in(&d, false);
        assert_eq!(got.len(), 1);
        assert!(got[0].busy && got[0].meta == silent);
        assert_eq!(files(&d).len(), 4);
        // 清理：没人听的那两个文件删掉，没回应的那个不动
        let got = sessions_in(&d, true);
        assert_eq!(got.len(), 1);
        assert_eq!(files(&d), vec![format!("{}.json", short(&silent.sid)), format!("{}.sock", short(&silent.sid))]);
        drop(quiet);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
