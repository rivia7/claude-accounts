//! 伪终端：开一对、设 / 问窗口大小、在上面起程序、把主的那头读到的攒起来、等输出里出现某个样子（expect）
//!
//! 测试里“在终端里运行”的程序都经它：[`Pty::open`] 开一对，[`Pty::spawn`] 把程序起在上面（自己一个会话、这个终端是它的
//! 控制终端），然后 [`Pty::send`] 当键盘、[`Pty::expect`] 看屏幕，[`Pty::wait`] 等它退出。
//!
//! macOS 上量出来的两件事（Linux 上没有，照着写两边都对）：
//! - 有控制终端的会话，头一个进程退出时系统要等它写在终端上的输出被主的那头读走，读走之前它退不完（一直是“正在退出”）。
//!   所以等这种程序退出要一边等一边读：用 [`Pty::wait`]，别直接 `Child::wait`（那样两边互相等，永远等不到）
//! - 没把它认作控制终端的程序（自己拿 [`Pty::slave`] 接上去、不换会话的那种）：最后一个从的那头关掉的那一刻，还没读走的
//!   输出就丢了。这种用法要留着这边的那一份（先别调 [`Pty::close_slave`]），读完了再关

use crate::proc::{Child, Cmd, Exit, Session};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 一对伪终端和主的那头到现在为止读到的输出
#[derive(Debug)]
pub struct Pty {
    master: Option<File>,
    slave: Option<OwnedFd>,
    path: PathBuf,
    /// 到现在为止读到的全部输出（[`Pty::pump`]、[`Pty::expect`] 往里攒）
    pub out: Vec<u8>,
}

fn cvt(rc: libc::c_int) -> io::Result<libc::c_int> {
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(rc)
    }
}

/// ptsname 用的是一块静态的缓冲区：一次只让一个线程用
static PTSNAME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn set_winsize(fd: RawFd, rows: u16, cols: u16) -> io::Result<()> {
    let size = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: size 是有效的
    cvt(unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, &size) }).map(drop)
}

impl Pty {
    /// 开一对，窗口是 rows 行 cols 列。两头都不会带给不相干的子进程
    pub fn open(rows: u16, cols: u16) -> io::Result<Pty> {
        // SAFETY: posix_openpt 给的是新的描述符，马上交给 OwnedFd 管（后面哪一步不成它就被关掉）；grantpt、unlockpt、
        // fcntl 只动这个描述符；ptsname 返回的指针拿着锁马上拷走
        let (master, path) = unsafe {
            let master = OwnedFd::from_raw_fd(cvt(libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY))?);
            cvt(libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC))?;
            cvt(libc::grantpt(master.as_raw_fd()))?;
            cvt(libc::unlockpt(master.as_raw_fd()))?;
            let _one_at_a_time = PTSNAME.lock().unwrap_or_else(|e| e.into_inner());
            let name = libc::ptsname(master.as_raw_fd());
            if name.is_null() {
                return Err(io::Error::last_os_error());
            }
            (master, PathBuf::from(std::ffi::OsStr::from_bytes(std::ffi::CStr::from_ptr(name).to_bytes())))
        };
        let path_c =
            std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::other("伪终端的名字里有 0 字节"))?;
        // SAFETY: path_c 是以 0 结尾的路径；O_NOCTTY：不让它变成这个进程的控制终端
        let slave =
            unsafe { OwnedFd::from_raw_fd(cvt(libc::open(path_c.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC))?) };
        set_winsize(slave.as_raw_fd(), rows, cols)?;
        Ok(Pty { master: Some(File::from(master)), slave: Some(slave), path, out: Vec::new() })
    }

    /// 从的那头的路径（/dev/ttys012、/dev/pts/3 这样的）
    pub fn slave_path(&self) -> &Path {
        &self.path
    }

    /// 从的那头另开一份（给 [`Cmd::stdin`] 这些用）；这边留着的那一份已经关了就照路径重新打开
    pub fn slave(&self) -> io::Result<OwnedFd> {
        match &self.slave {
            Some(fd) => fd.try_clone(),
            None => Ok(std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NOCTTY).open(&self.path)?.into()),
        }
    }

    /// 把这边留着的从的那头关掉：以后上面的程序都退出了，主的那头就读到头。上面的程序没把它认作控制终端时，
    /// 先把要看的输出读到了再关（见模块开头的说明）
    pub fn close_slave(&mut self) {
        self.slave = None;
    }

    /// 在这个终端上起一个程序：从的那头接成它的标准输入、输出、错误，它自己一个会话、这个终端是它的控制终端
    /// （和在终端里运行一样：`pty.fork()`）。起完把这边留着的从的那头关掉：它那个会话结束了，主的那头就读到头。
    /// 等它退出用 [`Pty::wait`]（见模块开头的说明）
    pub fn spawn(&mut self, cmd: &mut Cmd) -> io::Result<Child> {
        let child = cmd.stdin(self.slave()?).stdout(self.slave()?).stderr(self.slave()?).session(Session::NewCtty).spawn();
        self.close_slave();
        child
    }

    /// 等终端上的这个程序退出，一边等一边把输出攒进 `out`；退出以后把剩下的输出也读完。过了 limit 还没退出是
    /// `TimedOut` 的错（不杀它）
    pub fn wait(&mut self, child: &mut Child, limit: Duration) -> io::Result<Exit> {
        let end = Instant::now() + limit;
        let exit = loop {
            if let Some(exit) = child.try_wait()? {
                break exit;
            }
            if Instant::now() > end {
                let tail = String::from_utf8_lossy(&self.out[self.out.len().saturating_sub(600)..]).into_owned();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("进程 {} 没有退出，输出的末尾：{tail:?}", child.pid()),
                ));
            }
            if !self.pump(Duration::from_millis(20)) {
                std::thread::sleep(Duration::from_millis(2)); // 终端已经读到头了，只剩等它退出
            }
        };
        // 它退出前写的、还没读走的
        loop {
            let seen = self.out.len();
            if !self.pump(Duration::from_millis(20)) || self.out.len() == seen {
                return Ok(exit);
            }
        }
    }

    fn master(&self) -> io::Result<&File> {
        self.master.as_ref().ok_or_else(|| io::Error::other("这个伪终端主的那头已经关了"))
    }

    /// 改窗口的大小（上面在前台的程序会收到 SIGWINCH）
    pub fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
        set_winsize(self.master()?.as_raw_fd(), rows, cols)
    }

    /// 现在的窗口大小：（行，列）
    pub fn size(&self) -> io::Result<(u16, u16)> {
        // SAFETY: size 是有效的，TIOCGWINSZ 往里写
        unsafe {
            let mut size: libc::winsize = std::mem::zeroed();
            cvt(libc::ioctl(self.master()?.as_raw_fd(), libc::TIOCGWINSZ as _, &mut size))?;
            Ok((size.ws_row, size.ws_col))
        }
    }

    /// 往终端里“打字”
    pub fn send(&mut self, data: impl AsRef<[u8]>) -> io::Result<()> {
        self.master()?.write_all(data.as_ref())
    }

    /// 关掉主的那头：终端没了（上面的会话收到 SIGHUP）
    pub fn hangup(&mut self) {
        self.master = None;
    }

    /// 等最多 wait 那么久，把现在读得到的输出攒进 `out`。false：对面关了（上面没有程序了），或者主的那头已经关了
    pub fn pump(&mut self, wait: Duration) -> bool {
        let Some(master) = &mut self.master else { return false };
        let mut fds = [libc::pollfd { fd: master.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        // SAFETY: fds 是有效的 pollfd 数组
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 1, wait.as_millis().min(60_000) as libc::c_int) };
        if ready <= 0 {
            return true; // 这会儿没有输出（或者被信号打断了）
        }
        let mut buf = [0u8; 65536];
        match master.read(&mut buf) {
            // 读到 0 个字节（macOS）、EIO（Linux）都是“从的那头没人开着了”
            Ok(0) => false,
            Ok(n) => {
                self.out.extend_from_slice(&buf[..n]);
                true
            }
            Err(e) => e.kind() == io::ErrorKind::Interrupted,
        }
    }

    /// 等输出里（从第 since 个字节起）出现 pattern：等到了给出匹配末尾的位置；过了 limit、或者对面关了还没有，是 None。
    /// pattern 是按字节匹配的正则（和 Python 里拿 bytes 写的正则一样：`.` 是换行以外的任何一个字节，`\x1b` 这样写控制字符，
    /// 中文照 UTF-8 的字节匹配）
    pub fn wait_output(&mut self, pattern: &str, limit: Duration, since: usize) -> Option<usize> {
        let rx = regex::bytes::RegexBuilder::new(pattern)
            .unicode(false)
            .build()
            .unwrap_or_else(|e| panic!("正则 {pattern:?} 写得不对：{e}"));
        let end = Instant::now() + limit;
        let find = |out: &[u8]| (since <= out.len()).then(|| rx.find_at(out, since)).flatten().map(|m| m.end());
        loop {
            if let Some(at) = find(&self.out) {
                return Some(at);
            }
            if Instant::now() > end || !self.pump(Duration::from_millis(50)) {
                return find(&self.out);
            }
        }
    }

    /// 等输出里出现 pattern，给出匹配末尾的位置（下一次从这里接着等：[`Pty::expect_since`]）；没等到就 panic，带上输出的末尾
    pub fn expect(&mut self, pattern: &str, limit: Duration) -> usize {
        self.expect_since(pattern, limit, 0)
    }

    /// 同 [`Pty::expect`]，只看第 since 个字节以后的输出
    pub fn expect_since(&mut self, pattern: &str, limit: Duration, since: usize) -> usize {
        match self.wait_output(pattern, limit, since) {
            Some(at) => at,
            None => panic!(
                "没等到 {pattern:?}，输出的末尾：{:?}",
                String::from_utf8_lossy(&self.out[self.out.len().saturating_sub(600)..])
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAIT: Duration = Duration::from_secs(20);

    /// 不换会话（换会话要经 cca-test，在 tests/kit_selftest.rs 里测）：只把从的那头接成 0、1、2，这边的那一份留着
    fn on(pty: &Pty, script: &str) -> Child {
        let mut cmd = Cmd::new("/bin/sh");
        cmd.args(["-c", script]).env("PATH", "/usr/bin:/bin");
        cmd.stdin(pty.slave().unwrap()).stdout(pty.slave().unwrap()).stderr(pty.slave().unwrap()).spawn().unwrap()
    }

    #[test]
    fn type_and_read_back() {
        let mut pty = Pty::open(31, 97).unwrap();
        assert_eq!(pty.size().unwrap(), (31, 97));
        assert!(pty.slave_path().starts_with("/dev"), "{:?}", pty.slave_path());
        let mut child =
            on(&pty, "[ -t 0 ] && [ -t 1 ] && [ -t 2 ] && echo tty-ok; stty size; read line; echo \"got:$line\"; exit 7");
        let at = pty.expect("tty-ok\r\n31 97\r\n", WAIT);
        pty.resize(24, 80).unwrap();
        assert_eq!(pty.size().unwrap(), (24, 80));
        pty.send("你好\n").unwrap();
        // 终端把打的字照原样显示一遍（回显），然后才是程序的输出；中文照字节匹配
        let end = pty.expect_since("got:你好\r\n", WAIT, at);
        assert_eq!(end, pty.out.len());
        assert_eq!(pty.wait_output("got:", Duration::ZERO, end), None, "since 以后没有");
        assert_eq!(pty.wait(&mut child, WAIT).unwrap().py(), 7);
        // 这边还留着从的那头：没到头，只是没有新的输出
        assert!(pty.pump(Duration::from_millis(10)));
        // 关掉以后上面一个程序都没有了：读到头
        pty.close_slave();
        assert!(pty.wait_output("永远不会有", WAIT, 0).is_none());
        assert!(!pty.pump(Duration::from_millis(10)));
    }

    #[test]
    fn output_written_before_exit_is_kept_while_the_slave_is_held() {
        let mut pty = Pty::open(24, 80).unwrap();
        let mut child = on(&pty, "echo first; echo last");
        assert_eq!(child.wait().unwrap().py(), 0);
        // 程序已经退出了才来读：这边留着从的那头，它写过的都还在
        pty.expect("first\r\nlast\r\n", WAIT);
        // 写得比终端的缓冲多的程序：不读它就卡在写上，wait 是一边等一边读的
        let mut child = on(&pty, "i=0; while [ $i -lt 3000 ]; do echo \"line $i of many, padded out to be a bit longer\"; i=$((i+1)); done; echo done-writing");
        assert_eq!(pty.wait(&mut child, WAIT).unwrap().py(), 0);
        let text = String::from_utf8_lossy(&pty.out);
        assert!(
            text.contains("line 0 of many") && text.contains("line 2999 of many") && text.ends_with("done-writing\r\n"),
            "{}",
            text.len()
        );
    }

    #[test]
    fn expect_is_bytewise_and_says_what_it_saw() {
        let mut pty = Pty::open(24, 80).unwrap();
        let mut child = on(&pty, "printf 'a\\033[6nb\\377c\\n'; read x");
        // 控制字符、不是 UTF-8 的字节都能写进正则；. 是任何一个字节
        pty.expect(r"a\x1b\[6nb\xffc", WAIT);
        pty.expect(r"b.c\r\n", WAIT);
        let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pty.expect("没有的", Duration::from_millis(100))))
            .unwrap_err();
        let text = err.downcast_ref::<String>().unwrap();
        assert!(text.starts_with("没等到 \"没有的\"，输出的末尾：\"a\\u{1b}[6nb"), "{text}");
        // wait 到时限：说清楚是谁、看到了什么，不杀它
        let err = pty.wait(&mut child, Duration::from_millis(100)).unwrap_err();
        assert!(err.kind() == io::ErrorKind::TimedOut && err.to_string().contains("没有退出，输出的末尾"), "{err}");
        assert_eq!(child.try_wait().unwrap(), None);
        // 关掉主的那头：等输入的程序读到头
        pty.hangup();
        assert!(!pty.pump(Duration::ZERO));
        assert!(pty.send("x").is_err() && pty.size().is_err());
        pty.close_slave();
        child.wait().unwrap();
    }
}
