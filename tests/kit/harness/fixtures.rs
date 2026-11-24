//! 运行器测自己用的几个模块（`suite --kit-fixtures` 时模块表换成这一张）：故意失败的、超时的、漏登记的……
//! tests/kit_selftest.rs 起一份 suite 跑它们，从外面看结果文件、记录、退出码、有没有进程留下。
//! 用例把做过的事一行行记进环境变量 FX_OUT 指的文件（没设就不记）。

use super::{Ctx, Module, Need, Test};
use crate::fsx;
use crate::proc::{Cmd, Session};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// 表里的一项：名字、平时跑不跑、拿去查漏登记的原文、用例
macro_rules! fx {
    ($name:literal, $default:literal, $src:literal, [$($test:expr),* $(,)?]) => {
        Module { name: $name, src: $src, default: $default, tests: &[$($test),*] }
    };
}

pub(super) const MODULES: &[Module] = &[
    fx!(
        "fx_pass",
        true,
        "",
        [
            Test::new("plain", plain),
            Test::new("skipped", skipped),
            Test::new("needs_missing", needs_missing).needs(&[Need::Bin("sh"), Need::Bin("cca-kit-no-such-command")]),
            Test::new("tmp_and_defer", tmp_and_defer),
            Test::new("subs_and_names", subs_and_names),
            Test::new("after_all", after_all),
        ]
    ),
    fx!(
        "fx_fail",
        true,
        "",
        [
            Test::new("first_ok", nothing),
            Test::new("panics", panics),
            Test::new("sub_fails", sub_fails),
            Test::new("defer_panics", defer_panics),
            Test::new("last_ok", last_ok),
        ]
    ),
    fx!(
        "fx_timeout",
        true,
        "",
        [Test::new("quick", quick), Test::new("hangs", hangs).timeout(1), Test::new("never_runs", never_runs)]
    ),
    fx!(
        "fx_audit",
        true,
        "use kit::harness::Ctx;\n\nfn listed(t: &mut Ctx) {\n}\n\nfn forgotten(t: &mut Ctx) {\n}\n",
        [Test::new("listed", nothing)]
    ),
    fx!("fx_par_a", true, "", [Test::new("meet", meet)]),
    fx!("fx_par_b", true, "", [Test::new("meet", meet)]),
    fx!("fx_crash", true, "", [Test::new("before", nothing), Test::new("dies", dies), Test::new("after", nothing)]),
    fx!("fx_exit0", true, "", [Test::new("exits_early", exits_early), Test::new("after", nothing)]),
    fx!("fx_stop", true, "", [Test::new("stops_itself", stops_itself).timeout(1), Test::new("after", nothing)]),
    fx!("fx_env", true, "", [Test::new("shows", shows)]),
    fx!("fx_slow", true, "", [Test::new("takes_most_of_its_budget", takes_most_of_its_budget).timeout(4)]),
    fx!("fx_leak", true, "", [Test::new("leaves_processes_behind", leaves_processes_behind)]),
    fx!("fx_hang", false, "", [Test::new("waits_to_be_interrupted", waits_to_be_interrupted), Test::new("after", nothing)]),
    fx!("fx_off", false, "", [Test::new("only_when_asked", only_when_asked)]),
];

/// 记一行
fn note(line: impl AsRef<str>) {
    let Some(path) = std::env::var_os("FX_OUT") else { return };
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).expect("FX_OUT 打不开");
    file.write_all(format!("{}\n", line.as_ref()).as_bytes()).expect("FX_OUT 写不进去");
}

/// 到现在为止记下的里有没有这一行
fn noted(line: &str) -> bool {
    let all = std::env::var_os("FX_OUT").and_then(fsx::try_read_text).unwrap_or_default();
    all.lines().any(|l| l == line)
}

fn sleeper() -> Cmd {
    let mut cmd = Cmd::new("/bin/sleep");
    cmd.arg("300");
    cmd
}

fn nothing(_t: &mut Ctx) {}

// —— fx_pass：通过的、跳过的 ——

fn plain(_t: &mut Ctx) {
    println!("通过的用例说的话");
}

fn skipped(t: &mut Ctx) {
    note("skipped: before");
    t.skip("这台机器上没有这个条件")
}

fn needs_missing(_t: &mut Ctx) {
    note("needs_missing: ran");
}

fn tmp_and_defer(t: &mut Ctx) {
    let dir = t.tmp("fx-pass-");
    fsx::write(dir.join("深/一层/file"), "x");
    note(format!("tmp {}", dir.display()));
    for n in 1..=3 {
        t.defer(move || note(format!("defer {n}")));
    }
    let seen = dir.clone();
    t.defer(move || note(format!("tmp still there in defer: {}", seen.join("深/一层/file").exists())));
    t.defer_module(|| note("module defer 1"));
    t.defer_module(|| note("module defer 2"));
    note("body end");
}

fn subs_and_names(t: &mut Ctx) {
    note(format!("ctx {} {}", t.module(), t.name()));
    let mut seen = Vec::new();
    for n in 0..3 {
        t.sub(format!("n={n}"), |_t| seen.push(n));
    }
    t.sub("跳过的", |t| t.skip("子用例也能跳过"));
    note(format!("subs {seen:?}"));
}

fn after_all(_t: &mut Ctx) {
    note("after_all");
}

// —— fx_fail：各种失败；一个失败不耽误后面的 ——

fn panics(t: &mut Ctx) {
    println!("标准输出里的一行");
    eprintln!("标准错误里的一行");
    // 子进程接着同一个标准输出、标准错误：它说的也在失败的说明里
    let mut child = Cmd::new("/bin/sh").args(["-c", "echo 子进程说的; echo 子进程的错 >&2"]).spawn().expect("起 sh");
    child.wait().expect("等 sh");
    let dir = t.tmp("fx-fail-");
    note(format!("tmp {}", dir.display()));
    t.defer(|| note("defer after panic 1"));
    t.defer(|| note("defer after panic 2"));
    panic!("故意失败：{}", 6 * 7);
}

fn sub_fails(t: &mut Ctx) {
    t.sub("甲", |_t| assert_eq!(1 + 1, 3, "甲不对"));
    t.sub("乙", |_t| {});
    t.sub("丙", |t| t.sub("里面的", |_t| panic!("丙里面不对")));
    note("sub_fails: reached the end");
}

fn defer_panics(t: &mut Ctx) {
    t.defer(|| note("defer_panics: the earlier one still runs"));
    t.defer(|| panic!("收尾不成"));
}

fn last_ok(_t: &mut Ctx) {
    note("last_ok");
}

// —— fx_timeout：到时限 ——

fn quick(_t: &mut Ctx) {
    note("quick");
}

fn hangs(t: &mut Ctx) {
    println!("卡住之前说的话");
    let dir = t.tmp("fx-timeout-");
    t.defer(|| note("defer on timeout"));
    t.defer_module(|| note("module defer on timeout"));
    t.defer(|| note("later defer on timeout"));
    let mut same = sleeper().spawn().expect("起 sleep");
    // 自己一个会话的，还带着一个后台进程
    let pidfile = dir.join("bg.pid");
    let mut own = Cmd::new("/bin/sh")
        .args(["-c", "sleep 300 & echo $! > \"$0\"; wait"])
        .arg(&pidfile)
        .session(Session::New)
        .spawn()
        .expect("起 sh");
    let bg = fsx::wait_for("后台进程的进程号", Duration::from_secs(20), || {
        fsx::try_read_text(&pidfile).filter(|s| s.ends_with('\n'))
    });
    note(format!("pids {} {} {}", same.pid(), own.pid(), bg.trim()));
    note(format!("tmp {}", dir.display()));
    let _ = same.wait(); // 等不到：卡在这
    let _ = own.wait();
    note("hangs: got past the wait");
}

fn never_runs(_t: &mut Ctx) {
    note("never_runs: ran");
}

// —— fx_par_a、fx_par_b：是不是同时在跑 ——

/// 进来、等对面也在里面（最多 FX_MEET_MS 毫秒）、看到了就记一笔，再等对面也看到自己、出去
fn meet(t: &mut Ctx) {
    let me = t.module();
    let other = if me == "fx_par_a" { "fx_par_b" } else { "fx_par_a" };
    let limit = Duration::from_millis(std::env::var("FX_MEET_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(200));
    let end = Instant::now() + limit;
    let until = |cond: &dyn Fn() -> bool| loop {
        if cond() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    note(format!("enter {me}"));
    if until(&|| noted(&format!("enter {other}")) && !noted(&format!("leave {other}"))) {
        note(format!("saw {me}"));
        until(&|| noted(&format!("saw {other}")));
    }
    note(format!("leave {me}"));
}

// —— fx_crash、fx_exit0、fx_stop：模块的进程自己出事 ——

fn dies(_t: &mut Ctx) {
    println!("死之前说的话");
    // SAFETY: 给自己发 KILL
    unsafe { libc::kill(libc::getpid(), libc::SIGKILL) };
    std::thread::sleep(Duration::from_secs(300));
}

fn exits_early(_t: &mut Ctx) {
    std::process::exit(0);
}

fn stops_itself(_t: &mut Ctx) {
    note("stop: stopping");
    // SAFETY: 给自己发 STOP：整个进程停住，看时限的那条线程也停了
    unsafe { libc::kill(libc::getpid(), libc::SIGSTOP) };
    std::thread::sleep(Duration::from_secs(300));
}

// —— fx_env：模块的进程看到的环境 ——

fn shows(t: &mut Ctx) {
    let env = |key: &str| std::env::var(key).unwrap_or_else(|_| "(unset)".to_string());
    for key in ["HOME", "CCA_SUITE_TMP", "CCA_TEST_BIN", "CCA_HELPER", "CCA_REPO"] {
        note(format!("{key}={}", env(key)));
    }
    note(format!("tool={}", super::tool().display()));
    note(format!("helper={}", super::helper().display()));
    note(format!("repo={}", super::repo().display()));
    note(format!("tmp={}", t.tmp("fx-env-").display()));
    // SAFETY: 这几个只是问一下
    let (pid, ppid, sid, pgid) = unsafe { (libc::getpid(), libc::getppid(), libc::getsid(0), libc::getpgrp()) };
    note(format!("pid={pid}"));
    note(format!("ppid={ppid}"));
    note(format!("sid={sid}"));
    note(format!("pgid={pgid}"));
    // 标准输入是 /dev/null：一读就到头
    let mut buf = [0u8; 8];
    note(format!("stdin-bytes={}", std::io::stdin().read(&mut buf).map_or(-1, |n| n as i64)));
}

// —— fx_slow：用掉时限的一多半 ——

fn takes_most_of_its_budget(_t: &mut Ctx) {
    std::thread::sleep(Duration::from_millis(2200));
}

// —— fx_leak：用例通过了，却留下了进程 ——

fn leaves_processes_behind(t: &mut Ctx) {
    let dir = t.tmp("fx-leak-");
    // 登记着、却没人收的：模块收尾时停掉
    let child = sleeper().session(Session::New).spawn().expect("起 sleep");
    note(format!("forgotten {}", child.pid()));
    std::mem::forget(child);
    // 没登记的后台进程（起它的 sh 已经退出了），命令行里带着这个模块的临时目录：上一级照路径找到、停掉。
    // 它自己把进程号写下来：等到了，它的命令行就已经是 sh <临时目录>/fake-daemon 了
    let daemon = dir.join("fake-daemon");
    fsx::write_script(&daemon, "#!/bin/sh\necho $$ > \"$0.pid\"\nwhile :; do sleep 1; done\n");
    let mut sh = Cmd::new("/bin/sh")
        .args(["-c", "\"$0\" > /dev/null 2>&1 &"])
        .arg(&daemon)
        .env("PATH", "/usr/bin:/bin")
        .spawn()
        .expect("起 sh");
    assert!(sh.wait().expect("等 sh").success());
    let pid = fsx::wait_for("后台进程写下它的进程号", Duration::from_secs(20), || {
        fsx::try_read_text(dir.join("fake-daemon.pid")).filter(|s| s.ends_with('\n'))
    });
    note(format!("stray {}", pid.trim()));
}

// —— fx_hang：一直卡着，等着被打断 ——

fn waits_to_be_interrupted(t: &mut Ctx) {
    t.defer(|| note("hang: defer"));
    let mut child = sleeper().session(Session::New).spawn().expect("起 sleep");
    // SAFETY: getpid 只是问一下
    note(format!("hang: pids {} {}", unsafe { libc::getpid() }, child.pid()));
    note("hang: ready");
    let _ = child.wait();
    std::thread::sleep(Duration::from_secs(300));
}

// —— fx_off：平时不跑的模块 ——

fn only_when_asked(_t: &mut Ctx) {
    note("off: ran");
}
