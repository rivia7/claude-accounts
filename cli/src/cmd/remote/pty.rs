//! 伪终端和终端模式：开一对 pty、在上面起客户端、把用户的终端切到原始模式（退出时还原）。
//! 只用 libc 里 macOS 和 musl 都有的：posix_openpt / grantpt / unlockpt / ptsname（musl 没有 forkpty）。

use std::ffi::CStr;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}

impl Size {
    /// 两边都画得下的大小
    pub fn min(self, other: Size) -> Size {
        Size { cols: self.cols.min(other.cols), rows: self.rows.min(other.rows) }
    }

    pub fn valid(self) -> bool {
        self.cols >= 2 && self.rows >= 2
    }
}

fn last_err<T>() -> io::Result<T> {
    Err(io::Error::last_os_error())
}

/// fd 是终端、这个进程是会话的头一个进程、却还没有控制终端：把它认下来。界面 App 起的接入端就是这样（App 用
/// posix_spawn 开一个新会话，把它开的伪终端接成标准输入输出；macOS 上打开终端并不会让它成为控制终端，实测过）：
/// 认下来以后窗口大小变了才收得到 SIGWINCH。从 shell 里运行时不是会话的头一个进程，什么都不做
pub fn adopt_tty(fd: RawFd) {
    // SAFETY: 都只是查询；TIOCSCTTY 只在这个进程是会话头、还没有控制终端时才成（别的情况出错，不管）
    unsafe {
        if libc::getsid(0) != libc::getpid() || libc::isatty(fd) != 1 {
            return;
        }
        let probe = libc::open(c"/dev/tty".as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
        if probe >= 0 {
            libc::close(probe);
            return;
        }
        libc::ioctl(fd, libc::TIOCSCTTY as _, 0);
    }
}

pub fn isatty(fd: RawFd) -> bool {
    // SAFETY: 只是问这个描述符是不是终端
    unsafe { libc::isatty(fd) == 1 }
}

/// 终端窗口的大小；问不到、是 0 的时候 None
pub fn get_size(fd: RawFd) -> Option<Size> {
    // SAFETY: ws 是有效的，TIOCGWINSZ 往里写
    let ws = unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(fd, libc::TIOCGWINSZ as _, &mut ws) != 0 {
            return None;
        }
        ws
    };
    let s = Size { cols: ws.ws_col, rows: ws.ws_row };
    s.valid().then_some(s)
}

/// 改 pty 的大小（内核会给上面的前台进程组发 SIGWINCH）
pub fn set_size(fd: RawFd, s: Size) -> bool {
    let ws = libc::winsize { ws_row: s.rows, ws_col: s.cols, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: ws 是有效的
    unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, &ws) == 0 }
}

pub fn get_termios(fd: RawFd) -> io::Result<libc::termios> {
    // SAFETY: t 清零后由 tcgetattr 填
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut t) != 0 {
            return last_err();
        }
        Ok(t)
    }
}

fn set_termios(fd: RawFd, t: &libc::termios) -> bool {
    // SAFETY: t 是有效的。TCSANOW：不丢还没读走的输入
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, t) == 0 }
}

/// 现在被切到原始模式的那个终端和它原来的设置。辅助程序是 `panic = "abort"`，panic 时 main 的钩子说一句就退出，
/// `Drop` 不跑：钩子靠这里记着的把终端还原（`restore_on_panic`），不然用户的终端就停在原始模式里了
static ACTIVE: std::sync::Mutex<Option<(RawFd, libc::termios)>> = std::sync::Mutex::new(None);

/// main 的 panic 钩子里调：有终端正在原始模式里就还原（拿不到锁——正是改它的时候 panic 的——就算了）
pub fn restore_on_panic() {
    if let Ok(active) = ACTIVE.try_lock() {
        if let Some((fd, saved)) = active.as_ref() {
            set_termios(*fd, saved);
        }
    }
}

/// 把终端切到原始模式（按键一个个原样交上来，^C、^Z 也只是字节），离开作用域时还原
pub struct Raw {
    fd: RawFd,
    saved: libc::termios,
    on: bool,
}

impl Raw {
    pub fn enter(fd: RawFd) -> io::Result<Raw> {
        let saved = get_termios(fd)?;
        let mut raw = Raw { fd, saved, on: false };
        raw.resume();
        Ok(raw)
    }

    /// 暂时还原（挂起之前、终端不在了）
    pub fn pause(&mut self) {
        if self.on {
            set_termios(self.fd, &self.saved);
            self.on = false;
            *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }

    pub fn resume(&mut self) {
        let mut t = self.saved;
        // SAFETY: t 是有效的 termios
        unsafe { libc::cfmakeraw(&mut t) };
        t.c_cc[libc::VMIN] = 1;
        t.c_cc[libc::VTIME] = 0;
        self.on = set_termios(self.fd, &t);
        if self.on {
            *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = Some((self.fd, self.saved));
        }
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        self.pause();
    }
}

fn set_cloexec(fd: RawFd) {
    // SAFETY: 只改这个描述符的标志
    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
}

pub fn set_nonblock(fd: RawFd) {
    // SAFETY: 只改这个描述符的标志
    unsafe { libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK) };
}

/// pty 从设备的路径
fn slave_path(master: RawFd) -> io::Result<std::ffi::CString> {
    #[cfg(target_os = "linux")]
    {
        let mut buf = [0 as libc::c_char; 128];
        // SAFETY: buf 够长，ptsname_r 写进去的是以 0 结尾的字符串
        unsafe {
            if libc::ptsname_r(master, buf.as_mut_ptr(), buf.len()) != 0 {
                return last_err();
            }
            Ok(CStr::from_ptr(buf.as_ptr()).to_owned())
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        // ptsname 用的是一块静态的缓冲区：一次只让一个线程用（辅助程序本来只有一条线程，这是给同时跑的测试的）
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: master 是 posix_openpt 给的；返回的指针马上拷走
        unsafe {
            let p = libc::ptsname(master);
            if p.is_null() {
                return last_err();
            }
            Ok(CStr::from_ptr(p).to_owned())
        }
    }
}

/// 开一对 pty：主的那头不带给子进程、不阻塞；从的那头照 termios、大小设好
pub fn open(termios: Option<&libc::termios>, size: Size) -> io::Result<(OwnedFd, OwnedFd)> {
    // SAFETY: posix_openpt 给的是新的描述符，马上交给 OwnedFd 管；后面几步失败时它会被关掉
    let master = unsafe {
        let fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        if fd < 0 {
            return last_err();
        }
        OwnedFd::from_raw_fd(fd)
    };
    // SAFETY: master 开着
    unsafe {
        if libc::grantpt(master.as_raw_fd()) != 0 || libc::unlockpt(master.as_raw_fd()) != 0 {
            return last_err();
        }
    }
    let path = slave_path(master.as_raw_fd())?;
    // SAFETY: path 是以 0 结尾的字符串；O_NOCTTY：不让它变成这个进程的控制终端
    let slave = unsafe {
        let fd = libc::open(path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
        if fd < 0 {
            return last_err();
        }
        OwnedFd::from_raw_fd(fd)
    };
    if let Some(t) = termios {
        set_termios(slave.as_raw_fd(), t);
    }
    set_size(slave.as_raw_fd(), size);
    set_cloexec(master.as_raw_fd());
    set_cloexec(slave.as_raw_fd());
    set_nonblock(master.as_raw_fd());
    Ok((master, slave))
}

/// 在 pty 上起客户端：自己一个会话，pty 是它的控制终端，标准输入、输出、错误都是它；环境、当前目录原样继承，
/// 信号的处理方式和直接运行时一样（都是默认的）
pub fn spawn(argv: &[String], slave: OwnedFd) -> io::Result<Child> {
    let mut c = Command::new(&argv[0]);
    c.args(&argv[1..]);
    c.stdin(Stdio::from(slave.try_clone()?));
    c.stdout(Stdio::from(slave.try_clone()?));
    c.stderr(Stdio::from(slave));
    // SAFETY: fork 之后、exec 之前只调 setsid、ioctl、signal，都是可以在这里调的；这时 0 号描述符已经是 pty 从的那头
    unsafe {
        c.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            for sig in [libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM, libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU] {
                libc::signal(sig, libc::SIG_DFL);
            }
            Ok(())
        });
    }
    c.spawn()
}

/// 用户的终端另开一份（不阻塞）：标准输入那一份是和 shell 共用的，在它上面设不阻塞会连 shell 的也改掉
pub fn reopen_tty(fd: RawFd) -> io::Result<OwnedFd> {
    let mut buf = [0 as libc::c_char; 256];
    // SAFETY: buf 够长，ttyname_r 写进去的是以 0 结尾的字符串；open 给的新描述符马上交给 OwnedFd
    unsafe {
        let rc = libc::ttyname_r(fd, buf.as_mut_ptr(), buf.len());
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc));
        }
        let tty = libc::open(buf.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC);
        if tty < 0 {
            return last_err();
        }
        Ok(OwnedFd::from_raw_fd(tty))
    }
}

/// 不阻塞地读：Ok(0) 是对面关了，None 是现在没有
pub fn read_some(fd: RawFd, buf: &mut [u8]) -> Option<io::Result<usize>> {
    loop {
        // SAFETY: buf 是有效的、够长
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n >= 0 {
            return Some(Ok(n as usize));
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::EAGAIN) => return None,
            _ => return Some(Err(e)),
        }
    }
}

/// 不阻塞地写：写进去多少（现在写不了是 0）；对面没了是 Err
pub fn write_some(fd: RawFd, data: &[u8]) -> io::Result<usize> {
    loop {
        // SAFETY: data 是有效的
        let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
        if n >= 0 {
            return Ok(n as usize);
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::EAGAIN) => return Ok(0),
            _ => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::fd::AsRawFd;

    /// 读到出现 want 为止（最多等 5 秒）
    fn read_until(master: RawFd, want: &str) -> String {
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        let end = crate::py::time::monotonic() + 5.0;
        while crate::py::time::monotonic() < end {
            match read_some(master, &mut buf) {
                Some(Ok(n)) if n > 0 => got.extend_from_slice(&buf[..n]),
                Some(_) => break,
                None => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
            if String::from_utf8_lossy(&got).contains(want) {
                break;
            }
        }
        String::from_utf8_lossy(&got).into_owned()
    }

    #[test]
    fn client_gets_a_controlling_terminal_of_the_given_size() {
        let (master, slave) = open(None, Size { cols: 97, rows: 31 }).unwrap();
        // 有控制终端（/dev/tty 打得开）、大小对、标准输入输出都是终端
        let script =
            "stty size </dev/tty && [ -t 0 ] && [ -t 1 ] && [ -t 2 ] && echo tty-ok; read line; echo \"got:$line\"; exit 7";
        let mut child = spawn(&["/bin/sh".to_string(), "-c".to_string(), script.to_string()], slave).unwrap();
        let out = read_until(master.as_raw_fd(), "tty-ok");
        assert!(out.contains("31 97"), "{out:?}");
        assert!(out.contains("tty-ok"), "{out:?}");
        // 改大小：客户端再问就是新的
        assert!(set_size(master.as_raw_fd(), Size { cols: 80, rows: 24 }));
        assert_eq!(get_size(master.as_raw_fd()), Some(Size { cols: 80, rows: 24 }));
        assert_eq!(write_some(master.as_raw_fd(), "你好\n".as_bytes()).unwrap(), 7);
        let out = read_until(master.as_raw_fd(), "got:你好");
        assert!(out.contains("got:你好"), "{out:?}");
        assert_eq!(child.wait().unwrap().code(), Some(7));
    }

    #[test]
    fn raw_mode_is_restored() {
        let (_master, slave) = open(None, Size { cols: 80, rows: 24 }).unwrap();
        let fd = slave.as_raw_fd();
        let before = get_termios(fd).unwrap();
        assert!(before.c_lflag & libc::ICANON != 0, "新开的 pty 是行模式");
        {
            let mut raw = Raw::enter(fd).unwrap();
            let t = get_termios(fd).unwrap();
            assert!(t.c_lflag & (libc::ICANON | libc::ECHO | libc::ISIG) == 0 && t.c_oflag & libc::OPOST == 0);
            raw.pause();
            assert!(get_termios(fd).unwrap().c_lflag & libc::ICANON != 0);
            raw.resume();
            assert!(get_termios(fd).unwrap().c_lflag & libc::ICANON == 0);
        }
        let after = get_termios(fd).unwrap();
        // PENDIN 是内核自己记的（从原始模式切回行模式时，还没读走的输入要重新过一遍），不算设置
        let lflag = |t: &libc::termios| t.c_lflag & !libc::PENDIN;
        assert_eq!((lflag(&after), after.c_oflag, after.c_iflag), (lflag(&before), before.c_oflag, before.c_iflag));
    }

    #[test]
    fn reopened_tty_does_not_change_the_shared_one() {
        let (master, slave) = open(None, Size { cols: 80, rows: 24 }).unwrap();
        let own = reopen_tty(slave.as_raw_fd()).unwrap();
        // SAFETY: 只是读标志
        let (shared, mine) =
            unsafe { (libc::fcntl(slave.as_raw_fd(), libc::F_GETFL), libc::fcntl(own.as_raw_fd(), libc::F_GETFL)) };
        assert!(shared & libc::O_NONBLOCK == 0 && mine & libc::O_NONBLOCK != 0);
        // 另开的那一份读写的是同一个终端
        assert_eq!(write_some(own.as_raw_fd(), b"x").unwrap(), 1);
        let mut f = std::fs::File::from(master);
        let mut b = [0u8; 1];
        // 主的那头是不阻塞的：等它到
        let end = crate::py::time::monotonic() + 5.0;
        loop {
            match f.read(&mut b) {
                Ok(1) => break,
                _ if crate::py::time::monotonic() > end => panic!("没读到"),
                _ => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
        assert_eq!(&b, b"x");
    }
}
