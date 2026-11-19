//! `signal`：处理函数里 `raise` 的那种用法（上报进程的 `Stop`、等登录时的 `KeyboardInterrupt`……）。
//!
//! Python 的处理函数在主线程的两条字节码之间跑，所以它 raise 的异常能从任何地方冒出来：正在 `time.sleep`、等子进程、
//! 等 kqueue 的，马上被打断（`subprocess.run` 还会先把子进程杀掉）。Rust 的处理函数里只能做极少的事，这里只记一笔
//! “来过了”，由会阻塞的地方在被信号打断（EINTR）以后来看：
//! - `raise_on(信号, 异常类)` 装上处理函数（不带 SA_RESTART：阻塞着的系统调用马上返回）；
//! - `check()`：来过就清掉、返回 `Err(Fail::Other(异常类, …))`，调用方 `?` 一路传上去，和原来的异常走的是同一条路；
//! - `sleep(秒)`：会被打断的 `time.sleep`；`py::proc` 等子进程时看 `pending()`，来过就杀掉子进程、返回 `ProcError::Interrupted`
//!   （那一笔留着，调用方接着 `check()?`）。
//!
//! 和原来不一样的地方：纯计算的中途不会被打断，要等到下一个会阻塞的地方或者下一次 `check()`；长时间不阻塞的循环里自己
//! 隔一段 `check()` 一次。

use super::fail::{Fail, R};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// 装了处理函数的信号来过、还没人接
static RAISED: AtomicBool = AtomicBool::new(false);
/// 来了以后 `check()` 抛的异常类（最后一次 `raise_on` 给的）
static CLASS: Mutex<&'static str> = Mutex::new("KeyboardInterrupt");

extern "C" fn mark(_sig: libc::c_int) {
    // 处理函数里只做这一件事（原子变量的写是可以在这里做的）
    RAISED.store(true, Ordering::SeqCst);
}

/// `signal.signal(sig, 处理函数)`，处理函数是 `raise 异常类()`
pub fn raise_on(sig: libc::c_int, class: &'static str) {
    *CLASS.lock().unwrap_or_else(|e| e.into_inner()) = class;
    // SAFETY: sa 清零后填好再交给 sigaction；mark 只写一个原子变量
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = mark as extern "C" fn(libc::c_int) as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        sa.sa_flags = 0; // 不要 SA_RESTART：和 CPython 一样（siginterrupt(sig, 1)）
        libc::sigaction(sig, &sa, std::ptr::null_mut());
    }
}

/// `signal.signal(sig, signal.SIG_IGN)`
pub fn ignore(sig: libc::c_int) {
    // SAFETY: 只是把这个信号设成忽略
    unsafe { libc::signal(sig, libc::SIG_IGN) };
}

/// 来过、还没人接（不清掉）
pub fn pending() -> bool {
    RAISED.load(Ordering::SeqCst)
}

/// 来过就清掉、当成那个异常返回；没来过是 Ok
pub fn check() -> R<()> {
    if RAISED.swap(false, Ordering::SeqCst) {
        return Err(Fail::Other(*CLASS.lock().unwrap_or_else(|e| e.into_inner()), Vec::new()));
    }
    Ok(())
}

/// `time.sleep(secs)`：装了 `raise_on` 的信号来了马上醒、返回那个异常；别的信号打断了接着睡完
pub fn sleep(secs: f64) -> R<()> {
    check()?;
    let deadline = super::time::monotonic() + secs;
    loop {
        let left = deadline - super::time::monotonic();
        if left <= 0.0 || left.is_nan() {
            return Ok(());
        }
        let ts = super::time::timespec(left);
        // SAFETY: ts 是有效的；不要剩下的时间（自己按 deadline 算）
        unsafe { libc::nanosleep(&ts, std::ptr::null_mut()) };
        check()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 这几步看的是同一个全局的标志：放在一个测试里按顺序做。cargo test 是多线程的，信号要发给这个测试自己的线程
    // （pthread_kill），正在阻塞的系统调用才会被打断；真的辅助程序只有一个线程，kill 发来的就落在它上面
    #[test]
    fn raise_check_sleep_and_subprocess() {
        raise_on(libc::SIGUSR2, "Stop");
        assert!(check().is_ok() && !pending());
        // SAFETY: 给自己的线程发一个装了处理函数的信号
        let me = unsafe { libc::pthread_self() };
        unsafe { libc::pthread_kill(me, libc::SIGUSR2) };
        assert!(pending());
        let e = check().unwrap_err();
        assert!(e.is("Stop") && !e.is_os_or_value() && !e.is_exit());
        assert!(check().is_ok(), "接过一次就清掉了");

        // 睡着的时候来：马上醒
        let killer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            // SAFETY: 同上
            unsafe { libc::pthread_kill(me, libc::SIGUSR2) };
        });
        let t0 = crate::py::time::monotonic();
        let r = sleep(5.0);
        assert!(r.unwrap_err().is("Stop"));
        let _ = killer.join();
        assert!(crate::py::time::monotonic() - t0 < 3.0);

        // 等子进程的时候来：杀掉子进程，那一笔留给调用方
        let killer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            // SAFETY: 同上
            unsafe { libc::pthread_kill(me, libc::SIGUSR2) };
        });
        let t0 = crate::py::time::monotonic();
        let e = crate::py::proc::Cmd::new(["/bin/sleep", "5"]).capture().timeout(8.0).run().unwrap_err();
        let _ = killer.join();
        assert!(matches!(e, crate::py::proc::ProcError::Interrupted), "{e:?}");
        assert!(crate::py::time::monotonic() - t0 < 4.0);
        assert!(check().unwrap_err().is("Stop"));
        assert!(sleep(0.01).is_ok());
    }
}
