//! 上一级（不带 --child 的 suite）：选出要跑的模块，一个模块起一个子进程（同时最多 jobs 个），收它们报的结果，
//! 写结果文件和记录；模块完了把它留下的进程、临时目录收掉

use super::{catch_signals, secs, signalled, Module, Opts, Test};
use crate::fsx::{self, TempDir};
use crate::proc::{self, Child, Cmd, Exit, Session, Stdio};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 用例到了时限、模块的进程自己没停下（它收场最多用二十几秒）：再等这么久就杀掉它
const GRACE: Duration = Duration::from_secs(45);
/// 模块的进程不在跑用例（刚起来、在收尾）时，这么久没有动静就算卡住了
const IDLE_LIMIT: Duration = Duration::from_secs(120);
/// 收到信号以后，给各个模块这么久收场，过了就杀
const STOP_LIMIT: Duration = Duration::from_secs(40);
/// 模块的进程里 HOME 指着这里：/dev/null 是个文件，它下面什么都建不出来（root 也不行）——
/// 哪个用例忘了给被测的程序指一个临时的 HOME，马上就出错，碰不到真的主目录
const NO_HOME: &str = "/dev/null/cca-suite-no-home";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Status {
    Ok,
    Skip,
    Fail,
    Timeout,
    NotRun,
}

impl Status {
    /// 记录里、屏幕上的写法
    fn word(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Skip => "skip",
            Status::Fail => "FAIL",
            Status::Timeout => "TIMEOUT",
            Status::NotRun => "NOTRUN",
        }
    }

    /// 模块的进程报上来的写法
    fn parse(word: &str) -> Option<Status> {
        match word {
            "ok" => Some(Status::Ok),
            "skip" => Some(Status::Skip),
            "fail" => Some(Status::Fail),
            "timeout" => Some(Status::Timeout),
            _ => None,
        }
    }
}

/// 模块的进程报上来的一条
#[derive(Debug, PartialEq)]
enum Record {
    Begin {
        test: String,
        timeout_s: u32,
    },
    End {
        test: String,
        status: Status,
        took: Duration,
        blob: Vec<u8>,
    },
    /// 到了时限、被打断了：它在收场，后面的用例不跑了
    Stop,
    Done,
}

/// 从 buf 的开头取出一条完整的记录：还不够一条是 None（等读到更多）；认不出的那一行是 Err
fn take_record(buf: &mut Vec<u8>) -> Option<Result<Record, String>> {
    let nl = buf.iter().position(|b| *b == b'\n')?;
    let head = String::from_utf8_lossy(&buf[..nl]).into_owned();
    let fields: Vec<&str> = head.split('\t').collect();
    let (record, used) = match fields.as_slice() {
        ["begin", test, limit] => (Record::Begin { test: test.to_string(), timeout_s: limit.parse().unwrap_or(0) }, nl + 1),
        ["stop"] => (Record::Stop, nl + 1),
        ["done"] => (Record::Done, nl + 1),
        ["end", test, status, ms, n] => match (Status::parse(status), ms.parse::<u64>(), n.parse::<usize>()) {
            (Some(status), Ok(ms), Ok(n)) => {
                let end = nl + 1 + n;
                if buf.len() < end {
                    return None;
                }
                (
                    Record::End {
                        test: test.to_string(),
                        status,
                        took: Duration::from_millis(ms),
                        blob: buf[nl + 1..end].to_vec(),
                    },
                    end,
                )
            }
            _ => {
                buf.drain(..=nl);
                return Some(Err(head));
            }
        },
        _ => {
            buf.drain(..=nl);
            return Some(Err(head));
        }
    };
    buf.drain(..used);
    Some(Ok(record))
}

/// 记录里的一段：状态那一行（通过、跳过的把那句话接在后面），失败、超时的后面缩进 4 格跟着说明
fn entry(status: Status, test: &str, took: Duration, blob: &[u8]) -> String {
    let text = String::from_utf8_lossy(blob);
    let mut out = format!("{} {test} {}", status.word(), secs(took));
    if matches!(status, Status::Ok | Status::Skip) {
        let note: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        if !note.is_empty() {
            out.push(' ');
            out.push_str(&note.join(" "));
        }
        out.push('\n');
    } else {
        out.push('\n');
        for line in text.lines() {
            out.push_str("    ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// 一个模块的结果
struct Tally {
    name: &'static str,
    /// 选中的用例，照表里的先后
    tests: Vec<&'static str>,
    /// 报过结果的
    seen: Vec<String>,
    passed: usize,
    failed: usize,
    skipped: usize,
    notrun: usize,
    took: Duration,
    pass: bool,
}

impl Tally {
    fn line(&self) -> String {
        let verdict = if self.pass { "pass" } else { "fail" };
        format!("{}\t{verdict}\t{}\t{}\t{}\t{}\n", self.name, self.passed, self.failed, self.skipped, secs(self.took))
    }
}

/// 一个正在跑的模块
struct Run {
    job: usize,
    child: Child,
    /// 它报结果的管道，读到头就放掉
    pipe: Option<File>,
    buf: Vec<u8>,
    started: Instant,
    /// 正在跑的用例：名字、从什么时候起、时限（秒）
    current: Option<(String, Instant, u32)>,
    /// 最近一次有动静
    last: Instant,
    /// 它说了 stop：到了时限、被打断了，后面的用例不跑了
    stopped: bool,
    /// 它说了 done：用例都跑过、收尾也做完了
    done: bool,
    /// 上一级把它杀掉了：正在跑的用例记成什么、为什么
    killed: Option<(Status, &'static str)>,
    log: Option<File>,
    /// 它的临时目录
    dir: PathBuf,
}

/// 往哪写：屏幕、记录的目录、结果文件
struct Sink {
    quiet: bool,
    logs: Option<PathBuf>,
    timing: Option<File>,
    results: Option<File>,
}

impl Sink {
    /// 屏幕上（标准输出）；写不进去（对面关了）不算错
    fn say(&self, text: &str) {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(text.as_bytes());
        if !text.ends_with('\n') {
            let _ = out.write_all(b"\n");
        }
        let _ = out.flush();
    }

    fn open_log(&self, module: &str) -> Option<File> {
        File::create(self.logs.as_ref()?.join(format!("{module}.log"))).ok()
    }
}

fn append(file: &mut Option<File>, text: &str) {
    if let Some(file) = file {
        let _ = file.write_all(text.as_bytes());
    }
}

/// 一个用例有结果了：记数、写记录、说在屏幕上
fn note_test(
    sink: &mut Sink,
    tally: &mut Tally,
    log: &mut Option<File>,
    limit: Option<u32>,
    test: &str,
    status: Status,
    took: Duration,
    blob: &[u8],
) {
    match status {
        Status::Ok => tally.passed += 1,
        Status::Skip => tally.skipped += 1,
        Status::Fail | Status::Timeout => tally.failed += 1,
        Status::NotRun => tally.notrun += 1,
    }
    tally.seen.push(test.to_string());
    append(log, &entry(status, test, took, blob));
    append(&mut sink.timing, &format!("{}\t{test}\t{}\t{}\n", tally.name, status.word(), secs(took)));
    let full = format!("{}::{test}", tally.name);
    if !(sink.quiet && status == Status::Ok) {
        sink.say(&entry(status, &full, took, blob));
    }
    // 用掉了时限的一多半：先提个醒，机器一忙它就该超时了
    if let Some(limit) =
        limit.filter(|l| *l > 0 && matches!(status, Status::Ok | Status::Fail) && took.as_secs_f64() * 2.0 > f64::from(*l))
    {
        append(log, &format!("SLOW {test} {} 时限 {limit} 秒\n", secs(took)));
        sink.say(&format!("SLOW {full} {} 时限 {limit} 秒", secs(took)));
    }
}

/// 模块的进程报上来的记录，有几条处理几条
fn drain_records(sink: &mut Sink, tally: &mut Tally, run: &mut Run) {
    while let Some(record) = take_record(&mut run.buf) {
        match record {
            Ok(Record::Begin { test, timeout_s }) => run.current = Some((test, Instant::now(), timeout_s)),
            Ok(Record::End { test, status, took, blob }) => {
                let limit = run.current.take().filter(|(name, _, _)| *name == test).map(|(_, _, limit)| limit);
                note_test(sink, tally, &mut run.log, limit, &test, status, took, &blob);
            }
            Ok(Record::Stop) => run.stopped = true,
            Ok(Record::Done) => run.done = true,
            Err(line) => sink.say(&format!("suite：模块 {} 报上来一行认不出的：{line:?}", tally.name)),
        }
    }
}

/// 管道里现在读得到的都读进 buf（管道是不等的）
fn pull(run: &mut Run) {
    let Some(pipe) = &mut run.pipe else { return };
    let mut chunk = [0u8; 65536];
    let eof = loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break true,
            Ok(n) => {
                run.buf.extend_from_slice(&chunk[..n]);
                run.last = Instant::now();
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break false, // 这会儿没有了
        }
    };
    if eof {
        run.pipe = None;
    }
}

/// 命令行里带着 marker（一个模块的临时目录）的进程：这个模块的用例起的、没人管的后台进程。给出（进程号，命令行）
#[cfg(target_os = "linux")]
fn strays(marker: &str) -> Vec<(i32, String)> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else { return found };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<i32>().ok()) else { continue };
        let Ok(raw) = std::fs::read(entry.path().join("cmdline")) else { continue };
        let command = String::from_utf8_lossy(&raw).replace('\0', " ");
        if pid != std::process::id() as i32 && command.contains(marker) {
            found.push((pid, command.trim().to_string()));
        }
    }
    found
}

#[cfg(not(target_os = "linux"))]
fn strays(marker: &str) -> Vec<(i32, String)> {
    let Ok(out) = Cmd::new("/bin/ps").args(["-axww", "-o", "pid=,command="]).timeout(Duration::from_secs(20)).output() else {
        return Vec::new();
    };
    let me = std::process::id() as i32;
    let text = out.out();
    let rows = text
        .lines()
        .filter_map(|line| line.trim_start().split_once(' '))
        .filter_map(|(pid, command)| Some((pid.parse::<i32>().ok()?, command.trim().to_string())));
    rows.filter(|(pid, command)| *pid != me && command.contains(marker)).collect()
}

/// 这个进程号还在不在
fn alive(pid: i32) -> bool {
    // SAFETY: 信号 0 只是问一下，什么都不发
    unsafe { libc::kill(pid, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}

/// 模块留下的后台进程停掉：TERM，等 2 秒，还在的 KILL。给出找到的那些
fn sweep(dir: &Path) -> Vec<(i32, String)> {
    let marker = format!("{}/", dir.display());
    let found = strays(&marker);
    for signal in [libc::SIGTERM, libc::SIGKILL] {
        for (pid, _) in &found {
            if alive(*pid) {
                // SAFETY: 发给刚才照命令行找到的那个进程
                unsafe { libc::kill(*pid, signal) };
            }
        }
        let end = Instant::now() + Duration::from_secs(2);
        while Instant::now() < end && found.iter().any(|(pid, _)| alive(*pid)) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    found
}

/// 一个模块完了：没报结果的用例补上，留下的进程和临时目录收掉，写它那一行结果
fn finish(sink: &mut Sink, tally: &mut Tally, mut run: Run, exit: Exit) {
    pull(&mut run);
    drain_records(sink, tally, &mut run);
    // 为什么没跑完，说过了没有
    let mut explained = run.stopped;
    if let Some((test, since, _)) = run.current.take() {
        // 跑到一半，模块的进程没了
        let (status, why) = match run.killed {
            Some((status, why)) => (status, why.to_string()),
            None => (Status::Fail, format!("模块的进程中途没了（{exit}），这个用例没有报结果")),
        };
        note_test(sink, tally, &mut run.log, None, &test, status, since.elapsed(), why.as_bytes());
        explained = true;
    }
    // 好好结束的：说了 done，退出码 0，或者有用例失败时的 1
    let clean = run.done && (exit.code == Some(0) || (exit.code == Some(1) && tally.failed > 0));
    if !clean && !explained {
        let why = match run.killed {
            Some((_, why)) => why.to_string(),
            None => format!("模块的进程没有好好结束（{exit}）"),
        };
        note_test(sink, tally, &mut run.log, None, "(module)", Status::Fail, Duration::ZERO, why.as_bytes());
    }
    for test in tally.tests.clone() {
        if !tally.seen.iter().any(|seen| seen == test) {
            note_test(sink, tally, &mut run.log, None, test, Status::NotRun, Duration::ZERO, b"");
        }
    }
    for (pid, command) in sweep(&run.dir) {
        let command: String = command.chars().take(300).collect();
        append(&mut run.log, &format!("LEFTOVER {pid} {command}\n"));
        sink.say(&format!("LEFTOVER {} {pid} {command}", tally.name));
    }
    if !fsx::keep() {
        fsx::remove_tree(&run.dir);
    }
    tally.took = run.started.elapsed();
    tally.pass = tally.failed == 0 && tally.notrun == 0;
    close(sink, tally);
}

/// 一个模块没跑成（起不了它的进程，或者轮到它之前就被打断了）：它的用例都记成没跑
fn abandon(sink: &mut Sink, tally: &mut Tally, why: Option<String>) {
    let mut log = sink.open_log(tally.name);
    if let Some(why) = why {
        note_test(sink, tally, &mut log, None, "(module)", Status::Fail, Duration::ZERO, why.as_bytes());
    }
    for test in tally.tests.clone() {
        note_test(sink, tally, &mut log, None, test, Status::NotRun, Duration::ZERO, b"");
    }
    tally.pass = false;
    close(sink, tally);
}

/// 结果文件里添上这个模块的一行，屏幕上说一句
fn close(sink: &mut Sink, tally: &Tally) {
    append(&mut sink.results, &tally.line());
    let verdict = if tally.pass { "pass" } else { "fail" };
    let notrun = if tally.notrun > 0 { format!("，没跑 {}", tally.notrun) } else { String::new() };
    sink.say(&format!(
        "模块 {}：{verdict}，通过 {}，失败 {}，跳过 {}{notrun}，{} 秒",
        tally.name,
        tally.passed,
        tally.failed,
        tally.skipped,
        secs(tally.took)
    ));
}

/// 起一个模块的进程
fn start(sink: &Sink, opts: &Opts, exe: &Path, run_dir: &Path, job: usize, name: &str) -> std::io::Result<Run> {
    let dir = run_dir.join(name);
    std::fs::create_dir_all(&dir)?;
    let (read, write) = std::io::pipe()?;
    proc::set_nonblocking(read.as_raw_fd());
    let mut cmd = Cmd::new(exe);
    cmd.args(["--child", name, "--module", name, "--report-fd", "3"]).args(&opts.filters);
    for skip in &opts.skips {
        cmd.args(["--skip", skip]);
    }
    for (on, flag) in [(opts.all, "--all"), (opts.nocapture, "--nocapture"), (opts.fixtures, "--kit-fixtures")] {
        if on {
            cmd.arg(flag);
        }
    }
    cmd.env_inherit().env("CCA_SUITE_TMP", &dir).env("HOME", NO_HOME);
    cmd.env("CCA_TEST_BIN", proc::tool()?).env("CCA_HELPER", super::helper()).env("CCA_REPO", super::repo());
    cmd.fd(3, write.into()).stdout(Stdio::Inherit).stderr(Stdio::Inherit);
    let child = cmd.spawn()?;
    let now = Instant::now();
    let pipe = Some(File::from(std::os::fd::OwnedFd::from(read)));
    Ok(Run {
        job,
        child,
        pipe,
        buf: Vec::new(),
        started: now,
        current: None,
        last: now,
        stopped: false,
        done: false,
        killed: None,
        log: sink.open_log(name),
        dir,
    })
}

/// --setsid：自己换一个会话。自己是进程组的头时换不了（在交互的 shell 里直接运行的就是）：另起一份、让它换好会话再跑，
/// 等它、把退出码传回去。返回 Some(退出码)：另起的那一份跑完了
fn detach(args: &[String]) -> Option<i32> {
    // SAFETY: setsid 只是一个系统调用
    if unsafe { libc::setsid() } >= 0 {
        return None;
    }
    let again = || -> std::io::Result<Child> {
        let mut cmd = Cmd::new(std::env::current_exe()?);
        cmd.args(args.iter().filter(|a| *a != "--setsid")).env_inherit().session(Session::New);
        cmd.stdin(Stdio::Inherit).stdout(Stdio::Inherit).stderr(Stdio::Inherit).spawn()
    };
    let mut child = match again() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("suite：--setsid 换不了会话，另起一份也不成：{e}");
            return Some(2);
        }
    };
    catch_signals();
    let mut told = false;
    loop {
        match child.wait_timeout(Duration::from_millis(100)) {
            Ok(Some(exit)) => return Some(exit.shell()),
            Ok(None) => {}
            Err(_) => return Some(1),
        }
        if signalled() != 0 && !told {
            told = true;
            let _ = child.signal(libc::SIGTERM);
        }
    }
}

/// 跑选中的模块；返回退出码
pub(super) fn run(opts: &Opts, modules: &[Module], args: &[String]) -> i32 {
    for (i, module) in modules.iter().enumerate() {
        if modules[..i].iter().any(|m| m.name == module.name) {
            eprintln!("suite：模块表里有两个叫 {} 的", module.name);
            return 2;
        }
    }
    if let Some(name) = opts.modules.iter().find(|name| !modules.iter().any(|m| m.name == *name)) {
        eprintln!("suite：没有叫 {name} 的模块（--list 看有哪些）");
        return 2;
    }
    let picked: Vec<(&Module, Vec<&Test>)> =
        modules.iter().map(|m| (m, opts.pick(m))).filter(|(_, tests)| !tests.is_empty()).collect();
    if opts.list {
        let mut out = std::io::stdout().lock();
        for (module, tests) in &picked {
            for test in tests {
                let _ = writeln!(out, "{}::{}", module.name, test.name);
            }
        }
        return 0;
    }
    if opts.setsid {
        if let Some(code) = detach(args) {
            return code;
        }
    }
    catch_signals();

    let started = Instant::now();
    let jobs = opts.jobs.or_else(|| std::env::var("CCA_SUITE_JOBS").ok()?.parse().ok()).unwrap_or(4).max(1);
    let grace = opts.grace.map_or(GRACE, Duration::from_secs);
    let mut sink = Sink { quiet: opts.quiet, logs: opts.logs.clone(), timing: None, results: None };
    if let Some(dir) = &sink.logs {
        if let Err(e) =
            std::fs::create_dir_all(dir).and_then(|()| File::create(dir.join("timing.tsv")).map(|f| sink.timing = Some(f)))
        {
            eprintln!("suite：记录写不到 {}：{e}", dir.display());
            return 2;
        }
    }
    if let Some(path) = &opts.results {
        match File::create(path) {
            Ok(file) => sink.results = Some(file),
            Err(e) => {
                eprintln!("suite：结果写不到 {}：{e}", path.display());
                return 2;
            }
        }
    }
    let mut tallies: Vec<Tally> = picked
        .iter()
        .map(|(module, tests)| Tally {
            name: module.name,
            tests: tests.iter().map(|t| t.name).collect(),
            seen: Vec::new(),
            passed: 0,
            failed: 0,
            skipped: 0,
            notrun: 0,
            took: Duration::ZERO,
            pass: false,
        })
        .collect();
    if tallies.is_empty() {
        sink.say("suite：没有选中的用例");
        return 0;
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("suite：不知道自己这个程序在哪：{e}");
            return 2;
        }
    };
    let run_dir = TempDir::new("cca-suite.");

    let mut running: Vec<Run> = Vec::new();
    let mut next = 0;
    // 收到信号以后：到什么时候还没收完场就杀
    let mut stop_at: Option<Instant> = None;
    loop {
        while stop_at.is_none() && running.len() < jobs && next < tallies.len() {
            match start(&sink, opts, &exe, &run_dir, next, tallies[next].name) {
                Ok(run) => running.push(run),
                Err(e) => abandon(&mut sink, &mut tallies[next], Some(format!("起不了模块的进程：{e}"))),
            }
            next += 1;
        }
        if running.is_empty() {
            break;
        }
        let mut fds: Vec<libc::pollfd> = running
            .iter()
            .filter_map(|run| run.pipe.as_ref())
            .map(|pipe| libc::pollfd { fd: pipe.as_raw_fd(), events: libc::POLLIN, revents: 0 })
            .collect();
        // 管道到头了的模块马上就要退出：看得勤一点，别让每个模块白等 100 毫秒
        let wait_ms = if running.iter().any(|run| run.pipe.is_none()) { 2 } else { 100 };
        // SAFETY: fds 是有效的 pollfd 数组（一个都没有时就是等一会儿）
        unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, wait_ms) };

        if signalled() != 0 && stop_at.is_none() {
            stop_at = Some(Instant::now() + STOP_LIMIT);
            for run in &running {
                let _ = run.child.signal(libc::SIGTERM);
            }
        }
        let overdue = stop_at.is_some_and(|at| Instant::now() > at);
        let mut i = 0;
        while i < running.len() {
            let run = &mut running[i];
            pull(run);
            drain_records(&mut sink, &mut tallies[run.job], run);
            if run.killed.is_none() {
                let stuck = match &run.current {
                    Some((_, since, limit)) => (since.elapsed() > Duration::from_secs((*limit).into()) + grace)
                        .then_some((Status::Timeout, "用例到了时限、模块的进程自己没停下：杀掉了")),
                    None => {
                        (run.last.elapsed() > IDLE_LIMIT).then_some((Status::Fail, "模块的进程不在跑用例、也一直没有动静：杀掉了"))
                    }
                };
                let stuck = stuck.or(overdue.then_some((Status::Fail, "收到信号以后模块的进程一直没收完场：杀掉了")));
                if let Some(why) = stuck {
                    run.killed = Some(why);
                    let _ = run.child.kill();
                }
            }
            match run.child.try_wait() {
                Ok(Some(exit)) => {
                    let run = running.remove(i);
                    finish(&mut sink, &mut tallies[run.job], run, exit);
                }
                _ => i += 1,
            }
        }
    }
    // 被打断时还没轮到的模块
    for tally in tallies.iter_mut().skip(next) {
        abandon(&mut sink, tally, None);
    }

    // 结果文件照模块表的先后重写一遍（跑的时候是谁先完谁先写的）
    if let (Some(path), Some(_)) = (&opts.results, sink.results.take()) {
        let text: String = tallies.iter().map(Tally::line).collect();
        let fresh = PathBuf::from(format!("{}.tmp", path.display()));
        if std::fs::write(&fresh, text).and_then(|()| std::fs::rename(&fresh, path)).is_err() {
            let _ = std::fs::remove_file(&fresh);
        }
    }
    let failed: Vec<&str> = tallies.iter().filter(|t| !t.pass).map(|t| t.name).collect();
    let sum = |f: fn(&Tally) -> usize| tallies.iter().map(f).sum::<usize>();
    let verdict =
        if failed.is_empty() { "都过了".to_string() } else { format!("{} 个没过（{}）", failed.len(), failed.join("、")) };
    sink.say(&format!(
        "suite：{} 个模块，{verdict}；用例通过 {}，失败 {}，跳过 {}；{} 秒",
        tallies.len(),
        sum(|t| t.passed),
        sum(|t| t.failed),
        sum(|t| t.skipped),
        secs(started.elapsed())
    ));
    if fsx::keep() {
        sink.say(&format!("临时目录留着：{}", run_dir.display()));
    }
    match signalled() {
        0 => i32::from(!failed.is_empty()),
        sig => 128 + sig,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_are_taken_one_at_a_time() {
        let mut buf = b"begin\tone\t300\nend\tone\tfail\t1234\t7\nab\ncd\n\nbegin\ttwo\t5\nend\ttwo\tok\t0\t0\ngarbage line\nstop\ndone\nend\tthree\tskip\t7\t6\nabc".to_vec();
        let mut got = Vec::new();
        while let Some(record) = take_record(&mut buf) {
            got.push(record);
        }
        assert_eq!(
            got,
            [
                Ok(Record::Begin { test: "one".to_string(), timeout_s: 300 }),
                Ok(Record::End {
                    test: "one".to_string(),
                    status: Status::Fail,
                    took: Duration::from_millis(1234),
                    blob: b"ab\ncd\n\n".to_vec()
                }),
                Ok(Record::Begin { test: "two".to_string(), timeout_s: 5 }),
                Ok(Record::End { test: "two".to_string(), status: Status::Ok, took: Duration::ZERO, blob: Vec::new() }),
                Err("garbage line".to_string()),
                Ok(Record::Stop),
                Ok(Record::Done),
            ]
        );
        // 最后一条的说明还没到齐：留着，等读到更多
        assert_eq!(buf, b"end\tthree\tskip\t7\t6\nabc");
        buf.extend_from_slice(b"def");
        assert_eq!(
            take_record(&mut buf),
            Some(Ok(Record::End {
                test: "three".to_string(),
                status: Status::Skip,
                took: Duration::from_millis(7),
                blob: b"abcdef".to_vec()
            }))
        );
        assert_eq!((take_record(&mut buf), buf.len()), (None, 0));
        // 一行还没完
        let mut buf = b"begin\tfour".to_vec();
        assert_eq!(take_record(&mut buf), None);
        // 写得不对的 end
        let mut buf = b"end\tx\tweird\t1\t0\nend\tx\tok\tsoon\t0\n".to_vec();
        assert_eq!(take_record(&mut buf), Some(Err("end\tx\tweird\t1\t0".to_string())));
        assert_eq!(take_record(&mut buf), Some(Err("end\tx\tok\tsoon\t0".to_string())));
    }

    #[test]
    fn log_entries() {
        let took = Duration::from_millis(1234);
        assert_eq!(entry(Status::Ok, "a", took, b""), "ok a 1.23\n");
        assert_eq!(entry(Status::Ok, "a", took, "跳过了 1 个子用例\n".as_bytes()), "ok a 1.23 跳过了 1 个子用例\n");
        assert_eq!(entry(Status::Skip, "m::a", Duration::ZERO, "没有 node".as_bytes()), "skip m::a 0.00 没有 node\n");
        assert_eq!(entry(Status::Fail, "a", took, b"one\n\n  two\n"), "FAIL a 1.23\n    one\n    \n      two\n");
        assert_eq!(entry(Status::Timeout, "a", took, b"x"), "TIMEOUT a 1.23\n    x\n");
        assert_eq!(entry(Status::NotRun, "a", Duration::ZERO, b""), "NOTRUN a 0.00\n");
    }
}
