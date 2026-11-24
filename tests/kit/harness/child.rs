//! 一个模块的子进程（`suite --child 模块`）：用例一个接一个跑，结果一条条经管道（`--report-fd`）报给上一级
//!
//! 每个用例在自己的线程里跑，主线程看着它：到了时限、收到信号、上一级没了，就收场——登记的收尾都跑掉、起过的子进程都
//! 停掉——然后退出。报给上一级的记录一行一条，制表符隔开；end 那一行后面紧跟 n 个字节的说明（通过、跳过的是一句话，
//! 失败、超时的是用例的输出）：
//!
//! ```text
//! begin <用例> <时限，秒>
//! end <用例> <ok|skip|fail|timeout> <用了多少毫秒> <n>
//! stop                                      （到了时限、被打断了：后面的用例不跑了）
//! done                                      （用例都跑过、收尾也做完了）
//! ```

use super::{audit, catch_signals, secs, signalled, Ctx, Module, Opts, Skip, Test};
use crate::fsx;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// 用例到了时限，模块的进程用这个退出码退出
const TIMEOUT_CODE: i32 = 124;
/// 主线程多久看一次（时限、信号、上一级还在不在）
const TICK: Duration = Duration::from_millis(50);
/// 收场时，登记的收尾最多跑这么久
const DEFER_LIMIT: Duration = Duration::from_secs(20);
/// 一个用例的输出最多留这么多字节（再多只留开头和末尾）
const OUTPUT_MAX: usize = 256 * 1024;
/// 用例那条线程的栈（主线程默认 8 MB，另起的线程默认只有 2 MB）
const STACK: usize = 16 << 20;

type Deferred = Box<dyn FnOnce() + Send>;

/// 登记着的收尾：做什么、是不是等模块完了才做
static DEFERS: Mutex<Vec<(Deferred, bool)>> = Mutex::new(Vec::new());

/// 这个用例里各处 panic 时说的话（不抓输出的时候拿它当失败的说明）
static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) fn push_defer(f: Deferred, module: bool) {
    locked(&DEFERS).push((f, module));
}

/// 登记着的收尾照登记的反序跑掉：module 是 false 时只跑用例的，true 时模块的也跑。返回有几件 panic 了
fn run_defers(module: bool) -> usize {
    let mut bad = 0;
    loop {
        // 一次取一件、放开锁再跑：收尾里还可以再登记
        let next = {
            let mut list = locked(&DEFERS);
            let at = list.iter().rposition(|(_, of_module)| module || !*of_module);
            at.map(|i| list.remove(i).0)
        };
        let Some(f) = next else { return bad };
        if catch_unwind(AssertUnwindSafe(f)).is_err() {
            bad += 1;
        }
    }
}

/// panic 时除了照常打印，再记一份
fn hook_panics() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let what = payload.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| payload.downcast_ref::<String>().cloned());
        let at = info.location().map(|l| format!("{}:{}：", l.file(), l.line())).unwrap_or_default();
        locked(&PANICS).push(format!("{at}{}", what.unwrap_or_else(|| "panic 了（没说为什么）".to_string())));
        default(info);
    }));
}

/// 直接写标准错误（不经 Rust 的那把锁：卡着的线程可能正拿着它）
fn say(text: &str) {
    // SAFETY: 写的就是 text 的那些字节
    unsafe { libc::write(2, text.as_ptr().cast(), text.len()) };
}

fn dup_high(fd: RawFd) -> std::io::Result<OwnedFd> {
    // SAFETY: fcntl 成了给的是一个新的描述符，马上交给 OwnedFd 管
    unsafe {
        let copy = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 10);
        if copy < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(OwnedFd::from_raw_fd(copy))
        }
    }
}

/// 让 fd 这个号指着 to 指的那个文件
fn point(fd: RawFd, to: &impl AsRawFd) {
    // SAFETY: dup2 只换这个号指着的东西
    unsafe { libc::dup2(to.as_raw_fd(), fd) };
}

/// 这个进程本来的标准输出、标准错误，另存的一份
struct Saved {
    out: OwnedFd,
    err: OwnedFd,
}

impl Saved {
    fn new() -> std::io::Result<Saved> {
        Ok(Saved { out: dup_high(1)?, err: dup_high(2)? })
    }

    /// 1、2 号指回本来的地方
    fn restore(&self) {
        let _ = std::io::stdout().flush();
        point(1, &self.out);
        point(2, &self.err);
    }
}

/// 一个用例的输出：1、2 号都指着一个没有名字的文件（用例起的子进程接着同一个），另开一个描述符来读
struct Capture {
    read: File,
}

impl Capture {
    fn start(n: usize) -> std::io::Result<Capture> {
        let path = fsx::tmp_base().join(format!(".suite-output.{}.{n}", std::process::id()));
        let write = std::fs::OpenOptions::new().append(true).create_new(true).open(&path)?;
        let read = File::open(&path);
        let _ = std::fs::remove_file(&path);
        let read = read?;
        let _ = std::io::stdout().flush();
        point(1, &write);
        point(2, &write);
        Ok(Capture { read })
    }

    /// 到现在为止的输出（太长的只留开头和末尾）
    fn text(&mut self) -> Vec<u8> {
        let len = self.read.metadata().map(|m| m.len() as usize).unwrap_or(0);
        let mut piece = |from: usize, n: usize| {
            let mut buf = Vec::new();
            if self.read.seek(SeekFrom::Start(from as u64)).is_ok() {
                let _ = (&mut self.read).take(n as u64).read_to_end(&mut buf);
            }
            buf
        };
        if len <= OUTPUT_MAX {
            return piece(0, len);
        }
        let head = OUTPUT_MAX / 4;
        let tail = OUTPUT_MAX - head;
        let mut text = piece(0, head);
        text.extend_from_slice(format!("\n……（输出太长，中间的 {} 个字节省掉了）……\n", len - OUTPUT_MAX).as_bytes());
        text.extend(piece(len - tail, tail));
        text
    }
}

/// 往上一级报。没有上一级（手工起的 --child）：结果说在标准错误上
struct Report {
    pipe: Option<File>,
    /// 写不进去了：上一级不在了
    broken: bool,
}

impl Report {
    fn open(fd: Option<RawFd>) -> Report {
        // SAFETY: fcntl 只是给这个号标上“exec 时关掉”（用例起的子进程不该带着它），不成就是这个号没开着；
        // 开着的话它是上一级专门给的，交给 File 管
        let pipe = fd
            .filter(|fd| *fd > 2 && unsafe { libc::fcntl(*fd, libc::F_SETFD, libc::FD_CLOEXEC) } == 0)
            .map(|fd| unsafe { File::from_raw_fd(fd) });
        Report { pipe, broken: false }
    }

    fn send(&mut self, head: String, blob: &[u8]) {
        if let Some(pipe) = &mut self.pipe {
            let mut record = head.into_bytes();
            record.push(b'\n');
            record.extend_from_slice(blob);
            if pipe.write_all(&record).is_err() {
                self.broken = true;
            }
        }
    }

    fn begin(&mut self, test: &str, timeout_s: u32) {
        self.send(format!("begin\t{test}\t{timeout_s}"), b"");
    }

    fn end(&mut self, test: &str, status: &str, took: Duration, blob: &[u8]) {
        if self.pipe.is_none() {
            say(&format!("{status} {test} {} {}\n", secs(took), String::from_utf8_lossy(blob)));
        }
        self.send(format!("end\t{test}\t{status}\t{}\t{}", took.as_millis(), blob.len()), blob);
    }

    fn stop(&mut self) {
        self.send("stop".to_string(), b"");
    }

    fn done(&mut self) {
        self.send("done".to_string(), b"");
    }
}

/// 一个用例跑下来
enum Outcome {
    /// 通过；带一句要记下来的话（跳过了哪些子用例），没有就是空的
    Ok(String),
    Skip(String),
    Fail(String),
}

/// 模块为什么中途停下
enum Stop {
    Timeout,
    Signal(i32),
    Orphaned,
}

/// 在用例自己的线程里：跑它，跑它登记的收尾，删它的临时目录
fn run_one(module: &'static str, test: Test) -> Outcome {
    let mut ctx = Ctx::new(module, test.name);
    let result = catch_unwind(AssertUnwindSafe(|| (test.f)(&mut ctx)));
    let bad_defers = run_defers(false);
    let mut notes = Vec::new();
    let mut skip = None;
    if let Err(payload) = result {
        match payload.downcast::<Skip>() {
            Ok(why) => skip = Some(why.0),
            Err(_) => notes.push("用例 panic 了（说了什么在上面）".to_string()),
        }
    }
    if !ctx.failed.is_empty() {
        notes.push(format!("{} 个子用例失败：{}", ctx.failed.len(), ctx.failed.join("；")));
    }
    if bad_defers > 0 {
        notes.push(format!("{bad_defers} 件收尾的事 panic 了"));
    }
    let skipped = std::mem::take(&mut ctx.skipped);
    drop(ctx); // 临时目录在收尾之后删
    match (notes.is_empty(), skip) {
        (false, _) => Outcome::Fail(notes.join("\n")),
        (true, Some(why)) => Outcome::Skip(why),
        (true, None) if skipped.is_empty() => Outcome::Ok(String::new()),
        (true, None) => Outcome::Ok(format!("跳过了 {} 个子用例：{}", skipped.len(), skipped.join("；"))),
    }
}

/// 上一级没了（被杀掉了）：再跑下去没有人收结果
fn orphaned(parent: libc::pid_t, report: &Report) -> bool {
    // SAFETY: getppid 只是问一下
    report.broken || (report.pipe.is_some() && unsafe { libc::getppid() } != parent)
}

/// 等用例跑完；到了时限、收到信号、上一级没了就不等了
fn watch(
    rx: &mpsc::Receiver<Outcome>,
    started: Instant,
    timeout_s: u32,
    parent: libc::pid_t,
    report: &Report,
) -> Result<Outcome, Stop> {
    loop {
        match rx.recv_timeout(TICK) {
            Ok(outcome) => return Ok(outcome),
            // 线程没交结果就没了（不该有：用例的 panic 都接住了）
            Err(RecvTimeoutError::Disconnected) => return Ok(Outcome::Fail("用例的线程没交结果就结束了".to_string())),
            Err(RecvTimeoutError::Timeout) => {}
        }
        if signalled() != 0 {
            return Err(Stop::Signal(signalled()));
        }
        if orphaned(parent, report) {
            return Err(Stop::Orphaned);
        }
        if started.elapsed() >= Duration::from_secs(timeout_s.into()) {
            return Err(Stop::Timeout);
        }
    }
}

/// 收场：登记着的收尾都跑掉（先用例的、再模块的；最多等 DEFER_LIMIT），起过的子进程都停掉、从此不再起新的。
/// 返回收尾里有几件没成
fn teardown() -> usize {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new().name("收尾".to_string()).spawn(move || {
        let bad = run_defers(false) + run_defers(true);
        let _ = tx.send(bad);
    });
    let bad = match (spawned, rx.recv_timeout(DEFER_LIMIT)) {
        (Ok(_), Ok(bad)) => bad,
        (Ok(_), Err(_)) => {
            say(&format!("suite：收尾过了 {} 秒还没完，不等了\n", DEFER_LIMIT.as_secs()));
            1
        }
        // 起不了线程：就在这条线上跑
        (Err(_), _) => run_defers(false) + run_defers(true),
    };
    crate::proc::close_all();
    bad
}

/// 在 text 后面另起一行接上 line
fn push_line(text: &mut Vec<u8>, line: &str) {
    if !text.is_empty() && !text.ends_with(b"\n") {
        text.push(b'\n');
    }
    text.extend_from_slice(line.as_bytes());
    text.push(b'\n');
}

/// 跑一个模块；返回退出码（都通过是 0，有失败的是 1）。到了时限、收到信号是在里面直接退出的（124、128 + 信号）
pub(super) fn run(opts: &Opts, modules: &[Module], name: &str) -> i32 {
    let Some(module) = modules.iter().find(|m| m.name == name).copied() else {
        eprintln!("suite：没有叫 {name} 的模块");
        return 2;
    };
    let tests: Vec<Test> = opts.pick(&module).into_iter().copied().collect();
    let mut report = Report::open(opts.report_fd);
    // SAFETY: getppid 只是问一下
    let parent = unsafe { libc::getppid() };
    catch_signals();
    hook_panics();
    let saved = if opts.nocapture { None } else { Saved::new().ok() };
    let mut failed = false;

    if let Some(problem) = audit(&module) {
        report.begin("(audit)", 0);
        report.end("(audit)", "fail", Duration::ZERO, problem.as_bytes());
        failed = true;
    }
    for (n, test) in tests.iter().copied().enumerate() {
        // 两个用例之间收到信号、上一级没了：收场，后面的不跑
        let stop = if signalled() != 0 { Some(128 + signalled()) } else { orphaned(parent, &report).then_some(1) };
        if let Some(code) = stop {
            report.stop();
            teardown();
            // SAFETY: 马上退出（后台可能还有用例留下的线程：不等它们）
            unsafe { libc::_exit(code) };
        }
        report.begin(test.name, test.timeout_s);
        if let Some(why) = test.needs.iter().find_map(|need| need.unmet()) {
            report.end(test.name, "skip", Duration::ZERO, why.as_bytes());
            continue;
        }
        locked(&PANICS).clear();
        let mut capture = saved.as_ref().and_then(|_| Capture::start(n).ok());
        let started = Instant::now();
        let (tx, rx) = mpsc::channel();
        let module_name = module.name;
        let worker = std::thread::Builder::new().name(test.name.to_string()).stack_size(STACK).spawn(move || {
            let _ = tx.send(run_one(module_name, test));
        });
        let verdict = match &worker {
            Ok(_) => watch(&rx, started, test.timeout_s, parent, &report),
            Err(e) => Ok(Outcome::Fail(format!("起不了用例的线程：{e}"))),
        };
        let took = started.elapsed();
        let mut output = match &mut capture {
            Some(capture) => capture.text(),
            None => locked(&PANICS).join("\n").into_bytes(),
        };
        match verdict {
            Ok(outcome) => {
                if let Ok(handle) = worker {
                    let _ = handle.join();
                }
                if let Some(saved) = &saved {
                    saved.restore();
                }
                match outcome {
                    Outcome::Ok(note) => report.end(test.name, "ok", took, note.as_bytes()),
                    Outcome::Skip(why) => report.end(test.name, "skip", took, why.as_bytes()),
                    Outcome::Fail(note) => {
                        failed = true;
                        push_line(&mut output, &note);
                        report.end(test.name, "fail", took, &output);
                    }
                }
            }
            Err(stop) => {
                let (status, note, code) = match stop {
                    Stop::Timeout => (
                        "timeout",
                        format!("用例过了时限（{} 秒）还没完：整个模块停下，后面的用例不跑了", test.timeout_s),
                        TIMEOUT_CODE,
                    ),
                    Stop::Signal(sig) => ("fail", format!("收到信号 {sig}：整个模块停下"), 128 + sig),
                    Stop::Orphaned => ("fail", "上一级不在了：整个模块停下".to_string(), 1),
                };
                push_line(&mut output, &note);
                // 先把 1、2 号指回去（用例的输出上面已经读出来了）：手工起的没有上一级，结果是说在标准错误上的
                if let Some(saved) = &saved {
                    saved.restore();
                }
                report.end(test.name, status, took, &output);
                report.stop();
                teardown();
                // SAFETY: 马上退出：用例那条线程还卡着，不等它、不跑它的析构
                unsafe { libc::_exit(code) };
            }
        }
    }

    locked(&PANICS).clear();
    let bad = teardown();
    if bad > 0 {
        failed = true;
        let mut text = locked(&PANICS).join("\n").into_bytes();
        push_line(&mut text, &format!("模块的收尾里有 {bad} 件没成"));
        report.begin("(teardown)", 0);
        report.end("(teardown)", "fail", Duration::ZERO, &text);
    }
    report.done();
    i32::from(failed)
}
