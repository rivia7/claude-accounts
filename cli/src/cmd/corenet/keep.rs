//! 守着内核的常驻进程（`core_wanted` 到 `cmd_core_keep`）：core-keep。
//!
//! cca _core-keep（Linux，以及 macOS 26 以下）：没有界面 App 时，照 App 的 CoreHost 托管内置代理的内核。
//!
//! 信号：原来 SIGTERM / SIGINT 的处理函数直接抛异常（`Stop`），在哪一步都能打断。这里处理函数只记一笔、往一根管道里写一个字节，
//! 各个等待的地方（等子进程、退避时的睡眠）看着这根管道：信号一到马上醒，在那里当作抛了 `Stop`。接不接得住照原来的：
//! 主循环里的（`try` 里面）接住，先停内核再退出，退出码 0；已经在收尾（`finally` 里停内核的那几秒）时再来一个，原来是没人接的
//! 异常——不等内核停下就退出，退出码 1，这里照样（见 FINDINGS.md）。
//! 只有一处照不了：原来信号落在起子进程的那一瞬间（`Popen` 里面）时，内核已经起来、却还没记下，就没人停它了；这里只在等待的地方
//! 看信号，起来的内核一定记下了。

use super::{core_dir, type_name, CORE_CONFIG, CORE_DIR, CORE_LOG};
use crate::cmd::arity;
use crate::core::base::{CFG, SELF_PATH};
use crate::py::fs::load_quiet;
use crate::py::obj::V;
use crate::py::path;
use crate::py::proc::environ;
use crate::py::time::monotonic;
use crate::py::{Fail, R};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

/// 内核（`cca _core run`）不带的：App 的提问会话、事件这些，和别的后台进程一样
const KEEP_ENV_DROP: [&str; 7] = ["CCA_EVENTS", "CCA_GRANT", "CCA_RO", "CCA_PROMPT", "CCA_TTY", "CCA_YES", "CCA_TOOL"];
/// 内核的输出超过这么大，下次启动前清空
const LOG_MAX: u64 = 1048576;

/// 收到过几次 SIGTERM / SIGINT
static SIGNALS: AtomicUsize = AtomicUsize::new(0);
/// 叫醒正在等的主循环用的管道（写的那一头）
static WAKE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_term(_sig: libc::c_int) {
    SIGNALS.fetch_add(1, Ordering::SeqCst);
    let fd = WAKE.load(Ordering::SeqCst);
    if fd >= 0 {
        // SAFETY: write 可以在信号处理函数里调；fd 是自己开的管道，写不进去（满了）也无妨，已经记下了
        unsafe { libc::write(fd, b"x".as_ptr().cast(), 1) };
    }
}

/// 等待的地方看信号用：管道读的那一头，和已经处理过几次信号
struct Signals {
    wake: i32,
    seen: usize,
}

impl Signals {
    /// 有新来的信号吗（原来处理函数在这时抛 `Stop`）。有就算处理过了，管道里叫醒用的字节也读掉
    fn stop(&mut self) -> bool {
        let n = SIGNALS.load(Ordering::SeqCst);
        if n == self.seen {
            return false;
        }
        self.seen = n;
        let mut buf = [0u8; 64];
        // SAFETY: 从自己的管道（不阻塞）读进一段有效的内存，读到没有为止
        while unsafe { libc::read(self.wake, buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
        true
    }

    /// 睡 secs 秒，信号一到马上醒
    fn nap(&self, secs: f64) {
        if secs > 0.0 {
            let mut p = libc::pollfd { fd: self.wake, events: libc::POLLIN, revents: 0 };
            // SAFETY: p 是有效的一项；被信号打断（EINTR）就是要它马上回来
            unsafe { libc::poll(&mut p, 1, (secs * 1000.0).ceil().min(i32::MAX as f64) as i32) };
        }
    }
}

/// cca _core-keep 要不要内核在跑：本机客户端是内置代理、配置文件在（cca local off、换了客户端时 core_retire 先删掉它）。
/// Err：config.json 的 local 一节被改成了不是对象的东西——原来是没人接的 AttributeError，但托管着内核时要先把它停掉
/// （`finally`）才退出，所以这里不 panic，往上传
fn core_wanted() -> R<bool> {
    let Some(V::Dict(c)) = load_quiet(&CFG) else {
        return Ok(false);
    };
    let client = match c.get("local") {
        Some(V::Dict(local)) => local.get("client"),
        Some(v) if v.truthy() => return Err(Fail::Other("AttributeError", vec![format!("AttributeError: '{}' object has no attribute 'get'", type_name(&v))])),
        _ => None,
    };
    Ok(matches!(client, Some(V::Str(s)) if &*s == "builtin") && path::isfile(&CORE_CONFIG))
}

/// 配置文件的修改时间（纳秒）；没有、读不了是 None
fn core_config_mtime() -> Option<i128> {
    let m = std::fs::metadata(&*CORE_CONFIG).ok()?;
    Some(m.mtime() as i128 * 1_000_000_000 + m.mtime_nsec() as i128)
}

/// 这个进程现在开着的 3 号以上的文件描述符（和 `py::proc` 里的一样：/dev/fd 里列的；读不了就当 3 到 255 都开着）
fn open_fds_from_3() -> Vec<i32> {
    match std::fs::read_dir("/dev/fd") {
        Ok(dir) => dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<i32>().ok()).filter(|fd| *fd >= 3).collect(),
        Err(_) => (3..256).collect(),
    }
}

/// `subprocess.Popen([SELF_PATH, '_core', 'run'], stdin=DEVNULL, stdout=fd, stderr=fd, close_fds=True, start_new_session=True, env=env)`。
/// `py::proc::Cmd` 还不能把子进程的输出接到一个文件上，先照它的做法在这里起：命令照名字找（照新环境的 PATH）、不带 3 号以上的
/// 文件描述符（keep.lock 那把锁漏给内核的话，守着它的进程没了锁还在）、自己一个会话
fn spawn_core(log: &std::fs::File, env: &[(String, String)]) -> io::Result<Child> {
    let mut c = Command::new(&*SELF_PATH);
    c.args(["_core", "run"]);
    c.env_clear();
    c.envs(env.iter().map(|(k, v)| (k, v)));
    c.stdin(Stdio::null());
    c.stdout(Stdio::from(log.try_clone()?));
    c.stderr(Stdio::from(log.try_clone()?));
    let inherited = open_fds_from_3();
    // SAFETY: fork 之后、exec 之前只调 fcntl 和 setsid，都是可以在这里调的系统调用；inherited 是之前就分配好的
    unsafe {
        c.pre_exec(move || {
            for fd in &inherited {
                libc::fcntl(*fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    c.spawn()
}

/// 等了一阵子的结果
#[derive(PartialEq)]
enum Waited {
    /// 子进程退出了
    Exited,
    /// 到时间了它还在（`subprocess.TimeoutExpired`）
    Timeout,
    /// 等的时候来了信号（`Stop`）
    Stop,
}

/// `child.wait(timeout=secs)`：和 Python 一样隔一会儿看一眼（0.5 毫秒起，每次加倍，最多 50 毫秒）
fn wait(child: &mut Child, sig: &mut Signals, secs: f64) -> Waited {
    let end = monotonic() + secs;
    let mut delay = 0.0005;
    loop {
        // 问不到（ECHILD）也当它已经没了，和 Python 一样
        if !matches!(child.try_wait(), Ok(None)) {
            return Waited::Exited;
        }
        if sig.stop() {
            return Waited::Stop;
        }
        let left = end - monotonic();
        if left <= 0.0 {
            return Waited::Timeout;
        }
        delay = f64::min(delay * 2.0, 0.05).min(left);
        sig.nap(delay);
    }
}

/// 停掉 cca _core run 起的内核（它是自己进程组的头）：SIGTERM，3 秒还没退出就 SIGKILL。false：停到一半来了信号（`Stop`），没停完
fn core_halt(child: &mut Child, sig: &mut Signals) -> bool {
    for signal in [libc::SIGTERM, libc::SIGKILL] {
        // SAFETY: 只是给子进程那一组发信号；它已经没了时出错，不管
        unsafe { libc::killpg(child.id() as libc::pid_t, signal) };
        match wait(child, sig, 3.0) {
            Waited::Exited => return true,
            Waited::Stop => return false,
            Waited::Timeout => {}
        }
    }
    true
}

/// 主循环怎么出来的
enum End {
    /// 内核不要了（循环正常结束，或者 `return`）
    Done,
    /// 来了信号（`except Stop: pass`）
    Stop,
    /// 别的没人接的异常（起不了内核、开不了日志、config.json 的 local 不是对象）
    Fail(Fail),
}

/// 原来 `try` 里的那一段。出来时 child 里还有的就是可能还没停的（由调用方停，原来的 `finally`）
fn keep(child: &mut Option<Child>, sig: &mut Signals, env: &[(String, String)]) -> End {
    // core_wanted()，出错就带着错出去
    macro_rules! wanted {
        () => {
            match core_wanted() {
                Ok(yes) => yes,
                Err(e) => return End::Fail(e),
            }
        };
    }
    let mut backoff = 1.0;
    while wanted!() {
        if sig.stop() {
            return End::Stop;
        }
        if std::fs::metadata(&*CORE_LOG).is_ok_and(|m| m.len() > LOG_MAX) {
            let _ = std::fs::remove_file(&*CORE_LOG);
        }
        let log = match std::fs::OpenOptions::new().append(true).create(true).mode(0o600).open(&*CORE_LOG) {
            Ok(log) => log,
            Err(e) => return End::Fail(e.into()),
        };
        let started = monotonic();
        let running = match spawn_core(&log, env) {
            Ok(running) => child.insert(running),
            Err(e) => return End::Fail(e.into()),
        };
        drop(log);
        loop {
            match wait(running, sig, 2.0) {
                Waited::Exited => break,
                Waited::Stop => return End::Stop,
                Waited::Timeout => {
                    if !wanted!() {
                        return if core_halt(running, sig) { End::Done } else { End::Stop };
                    }
                }
            }
        }
        *child = None;
        if monotonic() - started >= 60.0 {
            backoff = 1.0;
        }
        let (mark, until) = (core_config_mtime(), monotonic() + backoff);
        while monotonic() < until && wanted!() && core_config_mtime() == mark {
            if sig.stop() {
                return End::Stop;
            }
            sig.nap(0.5);
        }
        backoff = f64::min(backoff * 2.0, 60.0);
    }
    End::Done
}

/// cca _core-keep（Linux）：没有界面 App 时，照 App 的 CoreHost 托管内置代理的内核。运行 cca _core run（生成配置后 exec 内核）；
/// 退出了就隔一会儿重启：1、2、4…秒，最多 60 秒，跑够 60 秒才退出的从 1 秒重新算，等的时候配置文件变了（cca 碰了一下）马上重启。
/// 不再要了（core_wanted）就停掉内核、自己退出，被 SIGTERM 时也先停内核。同时只有一个（core/keep.lock）。内核的输出追加进
/// core/core.log，每次启动前超过 1 MB 就清空。自己一个会话：终端的 Ctrl-C、退出登录都碰不到它
pub fn cmd_core_keep(args: &[String]) -> R<()> {
    arity("cmd_core_keep", args, 0, 0)?;
    // 已经是会话的头（bash 经 POSIX::setsid 起的）时不成，不管
    // SAFETY: setsid 没有参数，失败只是返回 -1
    unsafe { libc::setsid() };
    if !core_wanted()? {
        return Ok(());
    }
    core_dir()?;
    let lock = std::fs::OpenOptions::new().append(true).create(true).open(path::join(&CORE_DIR, "keep.lock"))?;
    // SAFETY: lock 开着，fd 有效
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Ok(()); // 已经有一个在托管：它会看到配置文件变了
    }
    let mut pipe = [-1; 2];
    // SAFETY: pipe 是两项的数组；两头都不留给子进程、不阻塞（写的那头在信号处理函数里写，读的那头要读到没有为止）
    unsafe {
        if libc::pipe(pipe.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error().into());
        }
        for fd in pipe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
        }
    }
    WAKE.store(pipe[1], Ordering::SeqCst);
    // SAFETY: on_term 只动原子变量、调 write；SIGHUP 改成不理（子进程也跟着不理，和原来一样）
    unsafe {
        let handler = on_term as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
    let env: Vec<(String, String)> = environ().into_iter().filter(|(k, _)| !KEEP_ENV_DROP.contains(&k.as_str())).collect();
    let mut sig = Signals { wake: pipe[0], seen: 0 };
    let mut child = None;
    let end = keep(&mut child, &mut sig, &env);
    // finally：内核还在就停掉。这时再来信号，原来是没人接的 Stop：不等内核停下就退出
    if let Some(running) = child.as_mut() {
        if matches!(running.try_wait(), Ok(None)) && !core_halt(running, &mut sig) {
            return Err(Fail::Other("Stop", vec!["Stop：停内核的时候又收到了信号".to_string()]));
        }
    }
    drop(lock);
    match end {
        End::Fail(e) => Err(e),
        End::Done | End::Stop => Ok(()),
    }
}
