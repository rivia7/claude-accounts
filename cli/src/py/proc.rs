//! `subprocess`：起外部命令、取输出、限时。
//!
//! 命令照名字在 PATH 里找（给了新环境就照新环境的 PATH）：回归把 curl、ssh、security、ps……换成 PATH 上的假命令，
//! 移植出来的代码必须照旧“按名字起命令”，不能改成进程内的实现，也不要写绝对路径（原来写了绝对路径的照写）。

use std::io;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, Stdio};

/// 这个进程现在开着的文件描述符里不小于 from 的那些（/dev/fd 里列的；读不了就当 from 到 255 都开着）
fn open_fds_from(from: i32) -> Vec<i32> {
    match std::fs::read_dir("/dev/fd") {
        Ok(dir) => dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<i32>().ok()).filter(|fd| *fd >= from).collect(),
        Err(_) => (from..256).collect(),
    }
}

/// 起了以后不等的子进程（`subprocess._active`）。Python 的 Popen 对象没人管了以后，下一次起子进程时 `_cleanup()` 把已经退出的
/// 收走，不留僵尸；Rust 的 `Child` 丢掉就没人收了——常驻的上报进程每隔几分钟在后台起一次统计，一天下来就是上百个僵尸，
/// 占着这个用户的进程数。交给 `forget` 的子进程由 `reap` 收：每次 `spawn` 之前收一遍（上报进程每分钟都要起 ps、curl）
static ACTIVE: std::sync::Mutex<Vec<Child>> = std::sync::Mutex::new(Vec::new());

/// 这个子进程不等了：记下来，退出以后由 `reap` 收走
pub fn forget(child: Child) {
    reap();
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).push(child);
}

/// 把记着的、已经退出的子进程收走（`subprocess._cleanup()`）
pub fn reap() {
    let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    // 还在跑的留着；退出了的（try_wait 收走了它）和问不了的不留
    active.retain_mut(|child| matches!(child.try_wait(), Ok(None)));
}

/// 子进程的标准输入
#[derive(Clone, Default)]
pub enum In {
    /// 继承（`subprocess` 的默认）
    #[default]
    Inherit,
    /// `stdin=subprocess.DEVNULL`
    Null,
    /// `input=…`
    Bytes(Vec<u8>),
}

/// 子进程的标准输出 / 标准错误
#[derive(Clone, Copy, Default, PartialEq)]
pub enum Out {
    #[default]
    Inherit,
    /// `subprocess.DEVNULL`
    Null,
    /// `subprocess.PIPE`：取回来
    Capture,
    /// `stderr=subprocess.STDOUT`（只给 stderr 用）
    ToStdout,
}

/// 一条命令怎么起（`subprocess.run` 的那些参数）
#[derive(Clone, Default)]
pub struct Cmd {
    pub argv: Vec<String>,
    /// `env=…`：整个换掉；None 是继承
    pub env: Option<Vec<(String, String)>>,
    pub cwd: Option<String>,
    pub stdin: In,
    pub stdout: Out,
    pub stderr: Out,
    /// 秒；到时间杀掉（SIGKILL）
    pub timeout: Option<f64>,
    /// `start_new_session=True`
    pub new_session: bool,
}

/// 跑完的结果（`subprocess.CompletedProcess`）
#[derive(Debug)]
pub struct Done {
    /// 退出码；被信号杀掉的是负的信号号
    pub rc: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Done {
    /// 输出当文字（不是 UTF-8 的字节换成 U+FFFD）
    pub fn out(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn err(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

#[derive(Debug)]
pub enum ProcError {
    /// 起不来（找不到命令是 FileNotFoundError，也是 OSError）
    Os(io::Error),
    /// `subprocess.TimeoutExpired`：超时前拿到的输出也带着（和原来的异常一样；现在的调用方都不看，测试看 stdout）
    Timeout {
        #[cfg_attr(not(test), allow(dead_code))]
        stdout: Vec<u8>,
        #[allow(dead_code)]
        stderr: Vec<u8>,
    },
    /// 等它的时候，处理函数会 raise 的信号来了（`py::signal::raise_on` 装的；没装过就不会有）：子进程已经杀掉。
    /// 原来是那个异常从 `subprocess.run` 里冒出来；这里那一笔还留着，调用方接着 `py::signal::check()?` 把它传上去
    Interrupted,
}

impl ProcError {
    #[cfg(test)]
    pub fn is_timeout(&self) -> bool {
        matches!(self, ProcError::Timeout { .. })
    }

    #[cfg(test)]
    pub fn is_not_found(&self) -> bool {
        matches!(self, ProcError::Os(e) if e.kind() == io::ErrorKind::NotFound)
    }
}

impl Cmd {
    pub fn new<S: AsRef<str>>(argv: impl IntoIterator<Item = S>) -> Cmd {
        Cmd { argv: argv.into_iter().map(|s| s.as_ref().to_string()).collect(), ..Cmd::default() }
    }

    /// `capture_output=True`
    pub fn capture(mut self) -> Cmd {
        self.stdout = Out::Capture;
        self.stderr = Out::Capture;
        self
    }

    pub fn stdout(mut self, o: Out) -> Cmd {
        self.stdout = o;
        self
    }

    pub fn stderr(mut self, o: Out) -> Cmd {
        self.stderr = o;
        self
    }

    pub fn stdin(mut self, i: In) -> Cmd {
        self.stdin = i;
        self
    }

    /// `input=文字`
    pub fn input(mut self, data: impl Into<Vec<u8>>) -> Cmd {
        self.stdin = In::Bytes(data.into());
        self
    }

    pub fn timeout(mut self, secs: f64) -> Cmd {
        self.timeout = Some(secs);
        self
    }

    pub fn env(mut self, env: Vec<(String, String)>) -> Cmd {
        self.env = Some(env);
        self
    }

    pub fn cwd(mut self, dir: impl Into<String>) -> Cmd {
        self.cwd = Some(dir.into());
        self
    }

    pub fn new_session(mut self) -> Cmd {
        self.new_session = true;
        self
    }

    fn command(&self) -> Command {
        let mut c = Command::new(&self.argv[0]);
        c.args(&self.argv[1..]);
        if let Some(env) = &self.env {
            c.env_clear();
            c.envs(env.iter().map(|(k, v)| (k, v)));
        }
        if let Some(d) = &self.cwd {
            c.current_dir(d);
        }
        c.stdin(match self.stdin {
            In::Inherit => Stdio::inherit(),
            In::Null => Stdio::null(),
            In::Bytes(_) => Stdio::piped(),
        });
        let io_of = |o: Out| match o {
            Out::Inherit | Out::ToStdout => Stdio::inherit(),
            Out::Null => Stdio::null(),
            Out::Capture => Stdio::piped(),
        };
        c.stdout(io_of(self.stdout));
        c.stderr(io_of(self.stderr));
        let (merge, session) = (self.stderr == Out::ToStdout, self.new_session);
        // Python 的 subprocess 默认 close_fds=True：子进程只带着 0、1、2。bash 那边开着的 3（提问的终端 / 答案）、9（启动锁）
        // 不能漏给子进程，后台的上报进程带着 9 号会一直占着锁。先在这里记下开着哪些，fork 以后只做 fcntl（exec 成功才关：
        // 标准库报告 exec 失败用的管道也在里面，不能直接关）
        let inherited = open_fds_from(3);
        // SAFETY: fork 之后、exec 之前只调 fcntl、dup2 和 setsid，都是可以在这里调的系统调用；inherited 是之前就分配好的
        unsafe {
            c.pre_exec(move || {
                for fd in &inherited {
                    libc::fcntl(*fd, libc::F_SETFD, libc::FD_CLOEXEC);
                }
                if merge && libc::dup2(1, 2) < 0 {
                    return Err(io::Error::last_os_error());
                }
                if session && libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        c
    }

    /// `subprocess.Popen(…)`：起来就返回，不等它
    pub fn spawn(&self) -> Result<Child, ProcError> {
        reap();
        // Python 的 subprocess 起子进程前不替你 flush；这里也不 flush，先后顺序才一样
        self.command().spawn().map_err(ProcError::Os)
    }

    /// `subprocess.run(…)`：等它跑完（或超时杀掉）
    pub fn run(&self) -> Result<Done, ProcError> {
        let mut child = self.spawn()?;
        let deadline = self.timeout.map(|t| super::time::monotonic() + t);
        let input = match &self.stdin {
            In::Bytes(b) => b.clone(),
            _ => Vec::new(),
        };
        let (stdout, stderr, mut end) = pump(&mut child, &input, deadline);
        if end == End::Done {
            end = wait_until(&mut child, deadline);
        }
        if end != End::Done {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if end == End::Signal { ProcError::Interrupted } else { ProcError::Timeout { stdout, stderr } });
        }
        let status = child.wait().map_err(ProcError::Os)?;
        let rc = status.code().unwrap_or_else(|| -status.signal().unwrap_or(0));
        Ok(Done { rc, stdout, stderr })
    }
}

/// 等子进程是怎么结束的
#[derive(Clone, Copy, PartialEq)]
enum End {
    /// 读到头了 / 退出了
    Done,
    /// 到期限了
    Deadline,
    /// 处理函数会 raise 的信号来了（`py::signal`）
    Signal,
}

/// 把 input 喂给子进程的 stdin，同时把它的 stdout、stderr 读到头（三个管道一起看着，谁也不会堵住谁）。
/// 第三个返回值：读到头了，还是到期限了、被信号打断了
fn pump(child: &mut Child, input: &[u8], deadline: Option<f64>) -> (Vec<u8>, Vec<u8>, End) {
    use std::os::fd::AsRawFd;
    let mut stdin = child.stdin.take();
    let (mut out, mut err) = (child.stdout.take(), child.stderr.take());
    let (mut got_out, mut got_err) = (Vec::new(), Vec::new());
    let mut sent = 0;
    if stdin.is_some() && input.is_empty() {
        stdin = None; // 没有要喂的：马上关上
    }
    for fd in [stdin.as_ref().map(|s| s.as_raw_fd()), out.as_ref().map(|s| s.as_raw_fd()), err.as_ref().map(|s| s.as_raw_fd())]
        .into_iter()
        .flatten()
    {
        // SAFETY: fd 是刚拿到的、自己的管道
        unsafe { libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK) };
    }
    let mut buf = [0u8; 65536];
    while stdin.is_some() || out.is_some() || err.is_some() {
        if super::signal::pending() {
            return (got_out, got_err, End::Signal);
        }
        let mut fds = Vec::with_capacity(3);
        if let Some(s) = &stdin {
            fds.push(libc::pollfd { fd: s.as_raw_fd(), events: libc::POLLOUT, revents: 0 });
        }
        if let Some(s) = &out {
            fds.push(libc::pollfd { fd: s.as_raw_fd(), events: libc::POLLIN, revents: 0 });
        }
        if let Some(s) = &err {
            fds.push(libc::pollfd { fd: s.as_raw_fd(), events: libc::POLLIN, revents: 0 });
        }
        let wait_ms = match deadline {
            None => -1,
            Some(d) => {
                let left = d - super::time::monotonic();
                if left <= 0.0 {
                    return (got_out, got_err, End::Deadline);
                }
                (left * 1000.0).ceil().min(i32::MAX as f64) as i32
            }
        };
        // SAFETY: fds 是有效的数组，长度如实给出
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, wait_ms) };
        if n < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        let ready = |fd: i32| fds.iter().any(|p| p.fd == fd && p.revents != 0);
        if let Some(s) = &stdin {
            if ready(s.as_raw_fd()) {
                // SAFETY: 往自己的管道写一段有效的内存
                let w = unsafe { libc::write(s.as_raw_fd(), input[sent..].as_ptr().cast(), input.len() - sent) };
                if w > 0 {
                    sent += w as usize;
                }
                let gone =
                    w < 0 && !matches!(io::Error::last_os_error().kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted);
                if sent == input.len() || gone {
                    stdin = None; // 喂完了，或者它不读了（管道断了不算错）
                }
            }
        }
        // 读一段接到 dst 后面；返回 true 是读到头了（或者读不了了），这个管道可以关了
        let mut drain = |fd: i32, dst: &mut Vec<u8>| -> bool {
            // SAFETY: 从自己的管道读进一段有效的可写内存
            let r = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            if r > 0 {
                dst.extend_from_slice(&buf[..r as usize]);
                return false;
            }
            r == 0 || !matches!(io::Error::last_os_error().kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted)
        };
        if let Some(fd) = out.as_ref().map(|s| s.as_raw_fd()) {
            if ready(fd) && drain(fd, &mut got_out) {
                out = None;
            }
        }
        if let Some(fd) = err.as_ref().map(|s| s.as_raw_fd()) {
            if ready(fd) && drain(fd, &mut got_err) {
                err = None;
            }
        }
    }
    (got_out, got_err, End::Done)
}

/// 等子进程退出，最晚等到 deadline（到点了还没退出是 `End::Deadline`）。不限时的交给调用方的 wait()：那里信号打断不了，
/// 现在要能被打断的地方（上报进程）起的命令都限了时
fn wait_until(child: &mut Child, deadline: Option<f64>) -> End {
    let Some(deadline) = deadline else {
        return End::Done;
    };
    // 和 Python 一样：一开始 0.5 毫秒看一次，每次加倍，最多 50 毫秒
    let mut delay = 0.0005;
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return End::Done,
            Ok(None) => {}
        }
        if super::signal::pending() {
            return End::Signal;
        }
        let left = deadline - super::time::monotonic();
        if left <= 0.0 {
            return End::Deadline;
        }
        delay = f64::min(delay * 2.0, 0.05).min(left);
        super::time::sleep(delay);
    }
}

/// `os.execv(path, argv)`：这个进程换成另一个程序（不在 PATH 里找，环境照旧）。回来了就是没换成，返回原因。
/// 和 Python 一样不替你 flush：缓冲里还有要写出去的，先 `py::io::flush_out()`
pub fn execv(path: &str, argv: &[&str]) -> io::Error {
    let c = |s: &str| std::ffi::CString::new(s).ok();
    let (Some(path), Some(args)) = (c(path), argv.iter().map(|a| c(a)).collect::<Option<Vec<_>>>()) else {
        return io::Error::new(io::ErrorKind::InvalidInput, "embedded null byte");
    };
    let mut ptrs: Vec<*const libc::c_char> = args.iter().map(|a| a.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    // SAFETY: path 和 ptrs 里各项都是以 0 结尾的字符串，ptrs 以空指针结尾，调用期间都活着
    unsafe { libc::execv(path.as_ptr(), ptrs.as_ptr()) };
    io::Error::last_os_error()
}

/// `os.environ` 的一份拷贝，可以增删后交给 `Cmd::env`
pub fn environ() -> Vec<(String, String)> {
    std::env::vars_os().map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned())).collect()
}

/// 环境里设一个变量（已有的换掉）
pub fn env_set(env: &mut Vec<(String, String)>, k: &str, v: &str) {
    match env.iter_mut().find(|(x, _)| x == k) {
        Some(e) => e.1 = v.to_string(),
        None => env.push((k.to_string(), v.to_string())),
    }
}

/// 环境里去掉一个变量
#[cfg(test)]
pub fn env_drop(env: &mut Vec<(String, String)>, k: &str) {
    env.retain(|(x, _)| x != k);
}

/// `os.environ.get(k)`
pub fn getenv(k: &str) -> Option<String> {
    std::env::var_os(k).map(|v| v.to_string_lossy().into_owned())
}

/// `os.environ.get(k) or ''`
pub fn env_or_empty(k: &str) -> String {
    getenv(k).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 不等的子进程退出以后不留僵尸：下一次起子进程（或者直接 `reap`）时收走
    #[test]
    fn forgotten_children_are_reaped() {
        let child = Cmd::new(["/bin/sh", "-c", "exit 0"]).stdin(In::Null).spawn().unwrap();
        let pid = child.id() as libc::pid_t;
        forget(child);
        // 等它退出（这时它是僵尸，进程号还占着；别的测试起子进程时也可能先把它收了，所以不在这里查“还占着”）
        std::thread::sleep(std::time::Duration::from_millis(300));
        Cmd::new(["/bin/sh", "-c", "exit 0"]).stdin(In::Null).run().unwrap(); // 起下一个子进程时顺手收走
                                                                              // SAFETY: 只是问这个进程号在不在
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "收走以后这个进程号应该不在了");
        // 还在跑的不动
        let child = Cmd::new(["/bin/sleep", "2"]).stdin(In::Null).spawn().unwrap();
        let pid = child.id() as libc::pid_t;
        forget(child);
        reap();
        // SAFETY: 同上
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
        // SAFETY: 测试自己起的进程，停掉它
        unsafe { libc::kill(pid, libc::SIGKILL) };
        std::thread::sleep(std::time::Duration::from_millis(200));
        reap();
        // SAFETY: 同上
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    }

    #[test]
    fn captures_output_and_exit_code() {
        let d = Cmd::new(["/bin/sh", "-c", "printf out; printf err >&2; exit 3"]).capture().run().unwrap();
        assert_eq!((d.rc, d.out().as_str(), d.err().as_str()), (3, "out", "err"));
    }

    #[test]
    fn feeds_stdin_without_deadlock() {
        let big = "x".repeat(300_000);
        let d = Cmd::new(["/bin/cat"]).input(big.clone()).capture().run().unwrap();
        assert_eq!(d.stdout.len(), big.len());
        // 不读 stdin 就退出的命令：管道断了不算错
        let d = Cmd::new(["/usr/bin/true"]).input(big).capture().run().unwrap();
        assert_eq!(d.rc, 0);
    }

    #[test]
    fn timeout_kills() {
        let t0 = crate::py::time::monotonic();
        let e = Cmd::new(["/bin/sh", "-c", "printf partial; sleep 5"]).capture().timeout(0.3).run().unwrap_err();
        assert!(e.is_timeout());
        assert!(crate::py::time::monotonic() - t0 < 2.0);
        if let ProcError::Timeout { stdout, .. } = e {
            assert_eq!(stdout, b"partial");
        }
    }

    #[test]
    fn missing_command_and_signal() {
        assert!(Cmd::new(["/nonexistent/cca-no-such-command"]).run().unwrap_err().is_not_found());
        let d = Cmd::new(["/bin/sh", "-c", "kill -9 $$"]).capture().run().unwrap();
        assert_eq!(d.rc, -9);
    }

    #[test]
    fn env_and_merge() {
        let d = Cmd::new(["/bin/sh", "-c", "echo $A; echo e >&2"])
            .env(vec![("A".into(), "1".into())])
            .stdout(Out::Capture)
            .stderr(Out::ToStdout)
            .run()
            .unwrap();
        assert_eq!(d.out(), "1\ne\n");
        let mut env = vec![("A".to_string(), "1".to_string())];
        env_set(&mut env, "A", "2");
        env_set(&mut env, "B", "3");
        env_drop(&mut env, "A");
        assert_eq!(env, [("B".to_string(), "3".to_string())]);
    }
}
