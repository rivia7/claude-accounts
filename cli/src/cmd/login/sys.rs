//! 这一块原来靠 Python 运行时的三样东西：被信号打断、带时限的套接字、等子进程。
//!
//! - KeyboardInterrupt：Python 的信号处理函数不带 SA_RESTART，卡着的系统调用被信号打断（EINTR）后，运行时在主线程里把它变成
//!   异常，从卡着的地方一路抛出来（`except KeyboardInterrupt` 接住）。这里的处理函数只记一下，卡着的地方被打断后看
//!   `interrupted()`，是就返回 `Stop::Interrupt`，一路 `?` 上去；别的 EINTR 照 Python 的做法接着等（PEP 475）。
//! - `sock.settimeout(秒)`：Python 把套接字设成不阻塞，每次收发前用 poll 等到能收发或者到点（`TimeoutError: timed out`）。
//!   `recv` 的时限管这一次调用，`sendall` 的管整个调用。
//! - `subprocess.Popen` 的 `wait()` / `wait(秒)` / `terminate()` / `kill()`：自己 waitpid（标准库的 `Child::wait` 被信号打断后
//!   自己接着等，看不到信号）。

use crate::core::base::SELF_PATH;
use crate::py::fail::os_error_text;
use crate::py::proc::{Cmd as Proc, In, ProcError};
use crate::py::time::{monotonic, sleep};
use crate::py::Fail;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_sig: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// 来过 `trap` 装过的信号（原来这时已经在抛 KeyboardInterrupt 了）
pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// 这个信号来了不再照默认的结束进程，只记一下。only_default：现在是默认处理才装——Python 启动时对 SIGINT 就是这样，
/// 被忽略着的（非交互 shell 的后台任务）照旧忽略；`signal.signal(SIGTERM, …)` 那种是不管原来怎样都装
pub fn trap(sig: libc::c_int, only_default: bool) {
    // SAFETY: sigaction 的结构是纯数据，全 0 是合法的值；处理函数只写一个原子变量（信号处理函数里可以做的事）
    unsafe {
        if only_default {
            let mut old: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(sig, std::ptr::null(), &mut old) != 0 || old.sa_sigaction != libc::SIG_DFL {
                return;
            }
        }
        let mut act: libc::sigaction = std::mem::zeroed();
        act.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::sigemptyset(&mut act.sa_mask);
        act.sa_flags = 0; // 不带 SA_RESTART：卡着的 read / waitpid / poll 要被打断
        libc::sigaction(sig, &act, std::ptr::null_mut());
    }
}

/// 为什么不往下做了
#[derive(Debug)]
pub enum Stop {
    /// 原来的 KeyboardInterrupt：来了信号，或者代码里自己 `raise KeyboardInterrupt`
    Interrupt,
    /// 别的：`die()`、`sys.exit()`、没人接的异常，照常往上传
    Fail(Fail),
}

impl From<Fail> for Stop {
    fn from(e: Fail) -> Stop {
        Stop::Fail(e)
    }
}

impl From<io::Error> for Stop {
    fn from(e: io::Error) -> Stop {
        Stop::Fail(Fail::Os(e))
    }
}

/// `os.read(fd, len(buf))`：读到头是 0。被要停下的信号打断是 `Stop::Interrupt`，别的信号打断了接着读
pub fn os_read(fd: RawFd, buf: &mut [u8]) -> Result<usize, Stop> {
    loop {
        // SAFETY: buf 是有效的可写内存，长度如实给出
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n >= 0 {
            return Ok(n as usize);
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e.into());
        }
        if interrupted() {
            return Err(Stop::Interrupt);
        }
    }
}

/// 套接字上等不下去的原因
#[derive(Debug)]
pub enum NetErr {
    /// `TimeoutError('timed out')`
    Timeout,
    /// 等的时候来了要停下的信号
    Interrupted,
    /// OSError
    Os(io::Error),
}

impl NetErr {
    /// `str(e)`
    pub fn text(&self) -> String {
        match self {
            NetErr::Timeout => "timed out".into(),
            NetErr::Interrupted => "interrupted".into(),
            NetErr::Os(e) => os_error_text(e),
        }
    }
}

/// 设了时限的 Python 套接字（`sock.settimeout(秒)` 之后的 `recv`、`sendall`）；离开作用域时关掉
pub struct PySock {
    fd: OwnedFd,
    timeout: f64,
}

impl PySock {
    /// `sock.settimeout(timeout)`：套接字从此不阻塞，等由 poll 来
    pub fn new(fd: OwnedFd, timeout: f64) -> io::Result<PySock> {
        // SAFETY: fd 是自己拿着的、开着的套接字
        unsafe {
            let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
            if flags < 0 || libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(PySock { fd, timeout })
    }

    /// 等到能收（能发），最晚到 deadline
    fn wait(&self, writing: bool, deadline: f64) -> Result<(), NetErr> {
        loop {
            let left = deadline - monotonic();
            if left < 0.0 {
                return Err(NetErr::Timeout);
            }
            let mut p = libc::pollfd { fd: self.fd.as_raw_fd(), events: if writing { libc::POLLOUT } else { libc::POLLIN }, revents: 0 };
            let ms = (left * 1000.0).ceil().min(i32::MAX as f64) as i32;
            // SAFETY: p 是有效的可写内存，个数如实给出
            let n = unsafe { libc::poll(&mut p, 1, ms) };
            if n > 0 {
                return Ok(());
            }
            if n == 0 {
                return Err(NetErr::Timeout);
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(NetErr::Os(e));
            }
            if interrupted() {
                return Err(NetErr::Interrupted);
            }
        }
    }

    /// `sock.recv(len(buf))`：对方关了是 0
    pub fn recv(&self, buf: &mut [u8]) -> Result<usize, NetErr> {
        let deadline = monotonic() + self.timeout;
        loop {
            self.wait(false, deadline)?;
            loop {
                // SAFETY: buf 是有效的可写内存，长度如实给出
                let n = unsafe { libc::recv(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
                if n >= 0 {
                    return Ok(n as usize);
                }
                let e = io::Error::last_os_error();
                match e.kind() {
                    io::ErrorKind::Interrupted if interrupted() => return Err(NetErr::Interrupted),
                    io::ErrorKind::Interrupted => {}
                    io::ErrorKind::WouldBlock => break, // poll 说能收、其实没有：回去接着等
                    _ => return Err(NetErr::Os(e)),
                }
            }
        }
    }

    /// `sock.sendall(data)`。对方已经走了是 OSError（EPIPE）：SIGPIPE 在这个进程里一直是忽略的（原来的 Python 也是）
    pub fn sendall(&self, mut data: &[u8]) -> Result<(), NetErr> {
        let deadline = monotonic() + self.timeout;
        while !data.is_empty() {
            self.wait(true, deadline)?;
            // SAFETY: data 是有效的内存，长度如实给出
            let n = unsafe { libc::send(self.fd.as_raw_fd(), data.as_ptr().cast(), data.len(), 0) };
            if n >= 0 {
                data = &data[n as usize..];
                continue;
            }
            let e = io::Error::last_os_error();
            match e.kind() {
                io::ErrorKind::Interrupted if interrupted() => return Err(NetErr::Interrupted),
                io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock => {}
                _ => return Err(NetErr::Os(e)),
            }
        }
        Ok(())
    }
}

/// 起起来的子进程里和等它、停它有关的那一小部分（`subprocess.Popen` 的 `returncode`、`wait`、`send_signal`）
pub struct Kid {
    pid: libc::pid_t,
    /// 退出码（被信号结束的是负的信号号）；还没等到是 None
    rc: Option<i32>,
}

impl Kid {
    pub fn of(child: &std::process::Child) -> Kid {
        Kid { pid: child.id() as libc::pid_t, rc: None }
    }

    /// `os.waitpid(pid, flags)`，等到了就记下退出码。stoppable：卡着的时候来了要停下的信号就不等了
    fn reap(&mut self, flags: libc::c_int, stoppable: bool) -> Result<(), Stop> {
        while self.rc.is_none() {
            let mut status = 0;
            // SAFETY: status 是有效的可写内存；pid 是自己起的子进程
            let r = unsafe { libc::waitpid(self.pid, &mut status, flags) };
            if r == self.pid {
                self.rc = Some(if libc::WIFSIGNALED(status) { -libc::WTERMSIG(status) } else { libc::WEXITSTATUS(status) });
            } else if r == 0 {
                break; // WNOHANG：还在跑
            } else if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                // 等不到它了（SIGCHLD 被忽略着的时候系统不留退出码）：Python 当它已经退出、退出码 0
                self.rc = Some(0);
            } else if stoppable && interrupted() {
                return Err(Stop::Interrupt);
            }
        }
        Ok(())
    }

    /// `p.wait()`
    pub fn wait(&mut self) -> Result<i32, Stop> {
        if interrupted() {
            return Err(Stop::Interrupt);
        }
        self.reap(0, true)?;
        Ok(self.rc.expect("reap(0) 等到了才返回"))
    }

    /// `p.wait(secs)`：到点还没退出是 None（原来的 TimeoutExpired）。和 Python 一样隔一会儿看一次：一开始 0.5 毫秒，每次加倍，
    /// 最多 50 毫秒
    pub fn wait_for(&mut self, secs: f64) -> Option<i32> {
        let end = monotonic() + secs;
        let mut delay = 0.0005;
        loop {
            let _ = self.reap(libc::WNOHANG, false);
            if self.rc.is_some() {
                return self.rc;
            }
            let left = end - monotonic();
            if left <= 0.0 {
                return None;
            }
            delay = f64::min(delay * 2.0, left).min(0.05);
            sleep(delay);
        }
    }

    /// `p.send_signal(sig)`（`terminate()` 是 SIGTERM，`kill()` 是 SIGKILL）：先看一眼，已经退出的不发
    pub fn send(&mut self, sig: libc::c_int) {
        let _ = self.reap(libc::WNOHANG, false);
        if self.rc.is_none() {
            // SAFETY: pid 是自己起的、还没等到退出的子进程
            unsafe { libc::kill(self.pid, sig) };
        }
    }
}

/// `subprocess.run([SELF_PATH, '_browser', url], stdin=subprocess.DEVNULL).returncode`：回调 cca 在账号专属的浏览器里打开网址
/// （网址是参数：原来就是这样交的）。起不来是没人接的 OSError
pub fn run_browser(url: &str) -> Result<i32, Stop> {
    let child = match Proc::new([SELF_PATH.as_str(), "_browser", url]).stdin(In::Null).spawn() {
        Ok(child) => child,
        Err(ProcError::Os(e)) => return Err(e.into()),
        Err(ProcError::Timeout { .. } | ProcError::Interrupted) => unreachable!("spawn 不限时，也不会被信号打断"),
    };
    let mut kid = Kid::of(&child);
    match kid.wait() {
        Err(Stop::Interrupt) => {
            // subprocess.run 等到一半来了 KeyboardInterrupt：再等它 0.25 秒（Ctrl-C 多半它也收到了），还在就杀掉，然后接着往上抛
            if kid.wait_for(0.25).is_none() {
                kid.send(libc::SIGKILL);
            }
            Err(Stop::Interrupt)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::FromRawFd;

    fn pair() -> (PySock, OwnedFd) {
        let mut fds = [0; 2];
        // SAFETY: fds 放得下两个描述符；拿到的两个都马上交给 OwnedFd 管
        unsafe {
            assert_eq!(libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()), 0);
            (PySock::new(OwnedFd::from_raw_fd(fds[0]), 0.3).unwrap(), OwnedFd::from_raw_fd(fds[1]))
        }
    }

    #[test]
    fn socket_waits_like_settimeout() {
        let (a, b) = pair();
        // 没人发：等到点，是 Python 的 TimeoutError('timed out')
        let t0 = monotonic();
        let mut buf = [0u8; 16];
        assert!(matches!(a.recv(&mut buf), Err(NetErr::Timeout)));
        assert!((0.25..20.0).contains(&(monotonic() - t0)));
        assert_eq!(NetErr::Timeout.text(), "timed out");
        // 发过来的收得到；对方关了是 0
        let peer = PySock::new(b, 0.3).unwrap();
        peer.sendall(b"hello").unwrap();
        assert_eq!(a.recv(&mut buf).unwrap(), 5);
        assert_eq!(&buf[..5], b"hello");
        drop(peer);
        assert_eq!(a.recv(&mut buf).unwrap(), 0);
        // 对方已经走了：管道断了是 OSError，不是信号把进程结束
        let e = a.sendall(&[0u8; 8]).unwrap_err();
        assert!(matches!(&e, NetErr::Os(os) if os.raw_os_error() == Some(libc::EPIPE)), "{e:?}");
        assert_eq!(e.text(), format!("[Errno {}] Broken pipe", libc::EPIPE));
    }

    #[test]
    fn sendall_gives_up_at_the_deadline() {
        let (a, _b) = pair();
        // 对方不收：缓冲区满了以后等到点
        let t0 = monotonic();
        assert!(matches!(a.sendall(&vec![0u8; 8 << 20]), Err(NetErr::Timeout)));
        assert!((0.25..20.0).contains(&(monotonic() - t0)));
    }

    #[test]
    fn child_exit_codes_and_signals() {
        let run = |script: &str| Proc::new(["/bin/sh", "-c", script]).stdin(In::Null).spawn().unwrap();
        let mut kid = Kid::of(&run("exit 7"));
        assert_eq!(kid.wait().unwrap(), 7);
        assert_eq!(kid.wait().unwrap(), 7); // 等过了再等：还是那个退出码
        kid.send(libc::SIGKILL); // 已经退出的不发信号（进程号可能已经是别人的了）
        let mut kid = Kid::of(&run("kill -9 $$"));
        assert_eq!(kid.wait().unwrap(), -9);
        // 到点还没退出是 None；terminate 以后等得到
        let mut kid = Kid::of(&run("exec sleep 30"));
        let t0 = monotonic();
        assert_eq!(kid.wait_for(0.2), None);
        assert!((0.15..20.0).contains(&(monotonic() - t0)));
        kid.send(libc::SIGTERM);
        assert_eq!(kid.wait_for(5.0), Some(-libc::SIGTERM));
    }

    #[test]
    fn os_read_reads_what_is_there() {
        let mut fds = [0; 2];
        // SAFETY: fds 放得下两个描述符；两个都交给 OwnedFd 管
        let (r, w) = unsafe {
            assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
            (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))
        };
        // SAFETY: 往自己的管道写三个字节
        assert_eq!(unsafe { libc::write(w.as_raw_fd(), b"abc".as_ptr().cast(), 3) }, 3);
        let mut buf = [0u8; 64];
        assert_eq!(os_read(r.as_raw_fd(), &mut buf).unwrap(), 3);
        drop(w);
        assert_eq!(os_read(r.as_raw_fd(), &mut buf).unwrap(), 0);
        assert!(matches!(os_read(-1, &mut buf), Err(Stop::Fail(Fail::Os(_)))));
    }
}
