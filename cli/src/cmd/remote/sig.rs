//! 宿主和接入端等信号的办法：处理函数只记一笔、往一根管道里写一个字节，主循环的 poll 看着这根管道（和 corenet/keep.rs 一样）。

use std::io;
use std::os::fd::RawFd;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

/// 来过哪些信号（按位记）
static PENDING: AtomicU32 = AtomicU32::new(0);
/// 叫醒主循环用的管道（写的那一头）
static WAKE: AtomicI32 = AtomicI32::new(-1);

const WATCHED: [libc::c_int; 7] =
    [libc::SIGWINCH, libc::SIGCHLD, libc::SIGHUP, libc::SIGTERM, libc::SIGINT, libc::SIGQUIT, libc::SIGCONT];

extern "C" fn on_signal(sig: libc::c_int) {
    if let Some(i) = WATCHED.iter().position(|s| *s == sig) {
        PENDING.fetch_or(1 << i, Ordering::SeqCst);
    }
    let fd = WAKE.load(Ordering::SeqCst);
    if fd >= 0 {
        // SAFETY: write 可以在信号处理函数里调；fd 是自己开的管道，写不进去（满了）也无妨，已经记下了
        unsafe { libc::write(fd, b"x".as_ptr().cast(), 1) };
    }
}

/// 装上处理函数（窗口大小变了、子进程有动静、终端没了、要它停、被继续），返回管道读的那一头（不阻塞、不带给子进程）。
/// 处理函数在 exec 以后自动恢复成默认的，所以可以在起客户端之前装
pub fn install() -> io::Result<RawFd> {
    let mut pipe = [-1; 2];
    // SAFETY: pipe 是两项的数组；on_signal 只动原子变量、调 write；不带 SA_RESTART，等着的 poll 马上回来
    unsafe {
        if libc::pipe(pipe.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        for fd in pipe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
        }
        WAKE.store(pipe[1], Ordering::SeqCst);
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        for sig in WATCHED {
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
    Ok(pipe[0])
}

/// fork 出来、不 exec 的子进程里：处理函数换回默认的（收到 SIGTERM 就结束），父进程的那根管道不再写
pub fn reset() {
    WAKE.store(-1, Ordering::SeqCst);
    PENDING.store(0, Ordering::SeqCst);
    for sig in WATCHED {
        // SAFETY: 只是把这几个信号的处理方式改回默认
        unsafe { libc::signal(sig, libc::SIG_DFL) };
    }
}

/// 这一次来过的信号
pub struct Got(u32);

impl Got {
    pub fn has(&self, sig: libc::c_int) -> bool {
        WATCHED.iter().position(|s| *s == sig).is_some_and(|i| self.0 & (1 << i) != 0)
    }
}

/// 取走来过的信号，管道里叫醒用的字节也读掉
pub fn take(wake: RawFd) -> Got {
    let mut buf = [0u8; 64];
    // SAFETY: 从自己的管道（不阻塞）读进一段有效的内存，读到没有为止
    while unsafe { libc::read(wake, buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
    Got(PENDING.swap(0, Ordering::SeqCst))
}

/// 有还没取走的（管道满了没叫醒时靠它）
pub fn pending() -> bool {
    PENDING.load(Ordering::SeqCst) != 0
}
