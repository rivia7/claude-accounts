//! kit 自己的测试：起进程（kit::proc）、cca-test exec、伪终端（kit::pty）、临时目录和等待（kit::fsx）、运行器（kit::harness）。
//!
//! 运行器从里面测不到的那些（用例 panic、超时、漏登记、结果文件和记录的样子、同时跑几个模块、被打断）：起一份 suite
//! 跑 kit 里留的那几个故意出事的模块（`--kit-fixtures`，见 tests/kit/harness/fixtures.rs），从外面看。

use kit::case;
use kit::fsx::{self, wait_for};
use kit::harness::{self, Ctx, Module};
use kit::proc::{self, Child, Cmd, Session, Stdio};
use kit::pty::Pty;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const MODULE: Module = Module {
    name: "kit_selftest",
    src: include_str!("kit_selftest.rs"),
    default: true,
    tests: &[
        case!(proc_env_starts_empty),
        case!(proc_exit_codes_and_signals),
        case!(proc_output_never_deadlocks),
        case!(proc_timeout_kills),
        case!(proc_only_named_fds_reach_the_child),
        case!(proc_finds_programs_on_the_childs_path),
        case!(proc_children_die_with_their_handle),
        case!(proc_children_get_clean_signals),
        case!(proc_cwd_and_arg0),
        case!(proc_stdio_by_hand),
        case!(proc_spawning_leaks_nothing),
        case!(exec_starts_a_new_session),
        case!(exec_says_why_it_could_not_run),
        case!(exec_opens_the_terminal_after_setsid),
        case!(pty_program_gets_a_controlling_terminal),
        case!(pty_keys_and_resize_reach_the_program),
        case!(fsx_wait_for_says_what_it_waited_for),
        case!(fsx_temp_dirs_live_under_the_module_dir),
        case!(harness_knows_where_things_are),
        case!(ctx_defers_are_registered),
        case!(ctx_defers_ran_in_reverse),
        case!(ctx_subs_run_in_place),
        case!(runner_reports_each_test_and_module),
        case!(runner_stops_a_module_at_the_time_limit),
        case!(runner_flags_slow_tests),
        case!(runner_names_unregistered_tests),
        case!(runner_runs_modules_side_by_side),
        case!(runner_survives_a_module_that_dies),
        case!(runner_kills_a_module_that_cannot_stop_itself),
        case!(runner_selects_modules_and_tests),
        case!(runner_gives_modules_a_safe_environment),
        case!(runner_setsid_gets_its_own_session),
        case!(runner_cleans_up_when_interrupted),
        case!(runner_modules_clean_up_when_the_suite_is_killed),
        case!(runner_sweeps_leftover_processes),
        case!(runner_keeps_temp_dirs_when_asked),
        case!(runner_output_modes),
    ],
};

/// 等的时限：机器再忙也该等得到
const WAIT: Duration = Duration::from_secs(30);

fn sh(script: &str) -> Cmd {
    let mut cmd = Cmd::new("/bin/sh");
    cmd.args(["-c", script]).env("PATH", "/usr/bin:/bin");
    cmd
}

fn sleeper() -> Cmd {
    let mut cmd = Cmd::new("/bin/sleep");
    cmd.arg("300");
    cmd
}

/// 这个进程号还在不在（自己起的、还没收走的也算在）
fn alive(pid: i32) -> bool {
    // SAFETY: 信号 0 只是问一下，什么都不发
    unsafe { libc::kill(pid, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}

// —— kit::proc ——

fn proc_env_starts_empty(_t: &mut Ctx) {
    let home = std::env::var("HOME").expect("这个进程自己的环境里是有东西的");
    // 什么都不给：子进程的环境是空的
    assert_eq!(Cmd::new("/usr/bin/env").output().unwrap().check().out(), "");
    // 给什么就是什么，照给的先后；后给的盖掉先给的，env_remove 去掉
    let mut cmd = Cmd::new("/usr/bin/env");
    cmd.env("B", "2").env("A", "中文 和 空格").env("B", "3").env("GONE", "x").env_remove("GONE");
    assert_eq!(cmd.output().unwrap().check().out(), "B=3\nA=中文 和 空格\n");
    // 点了名的才从这个进程带过去（这个进程里没有的不带）
    let mut cmd = Cmd::new("/usr/bin/env");
    cmd.pass_env(&["HOME", "CCA_KIT_NO_SUCH_VAR"]).envs([("X", "1"), ("Y", "2")]);
    assert_eq!(cmd.output().unwrap().check().out(), format!("HOME={home}\nX=1\nY=2\n"));
    // 整个带过去，再去掉、改掉几个
    let out = Cmd::new("/usr/bin/env").env_inherit().env_remove("HOME").env("X", "1").output().unwrap().check();
    let text = out.out();
    let vars: Vec<&str> = text.lines().collect();
    assert!(vars.contains(&"X=1") && !vars.iter().any(|v| v.starts_with("HOME=")), "{vars:?}");
    // 运行器给每个模块的进程都设了这一个
    if let Ok(tmp) = std::env::var("CCA_SUITE_TMP") {
        assert!(vars.contains(&format!("CCA_SUITE_TMP={tmp}").as_str()), "{vars:?}");
    }
}

fn proc_exit_codes_and_signals(_t: &mut Ctx) {
    let exit = sh("exit 3").output().unwrap().status;
    assert_eq!((exit.code, exit.signal, exit.py(), exit.shell(), exit.success()), (Some(3), None, 3, 3, false));
    assert_eq!(exit.to_string(), "退出码 3");
    assert!(sh("true").output().unwrap().status.success());
    let exit = sh("kill -TERM $$").output().unwrap().status;
    assert_eq!((exit.code, exit.signal, exit.py(), exit.shell(), exit.success()), (None, Some(libc::SIGTERM), -15, 143, false));
    assert_eq!(exit.to_string(), "被信号 15 杀掉");
    // 还在跑的：问是 None，等一会儿也是 None（不杀它）；kill 之后收到的是 KILL
    let mut child = sleeper().spawn().unwrap();
    assert_eq!(child.try_wait().unwrap(), None);
    assert_eq!(child.wait_timeout(Duration::from_millis(50)).unwrap(), None);
    assert_eq!(child.rusage(), None);
    assert!(alive(child.pid()));
    child.kill().unwrap();
    let exit = child.wait().unwrap();
    assert_eq!(exit.signal, Some(libc::SIGKILL));
    // 收走以后再问还是同一个结果；再发信号不算错
    assert_eq!((child.try_wait().unwrap(), child.wait().unwrap()), (Some(exit), exit));
    assert!(child.rusage().is_some());
    child.kill().unwrap();
    child.signal(libc::SIGTERM).unwrap();
    // 进程号就是那个程序自己看到的
    let mut child = sh("echo $$").stdout(Stdio::Piped).spawn().unwrap();
    let mut text = String::new();
    child.stdout.take().unwrap().read_to_string(&mut text).unwrap();
    assert_eq!(text.trim(), child.pid().to_string());
    assert!(child.wait().unwrap().success());
    // 用掉的 CPU 时间是它自己的
    let mut child = sh("i=0; while [ $i -lt 20000 ]; do i=$((i+1)); done").spawn().unwrap();
    child.wait().unwrap();
    let cpu = child.rusage().unwrap().cpu();
    assert!(cpu > Duration::from_millis(1) && cpu < Duration::from_secs(60), "{cpu:?}");
}

fn proc_output_never_deadlocks(_t: &mut Ctx) {
    // 三路一起：喂给标准输入 3 MB，它先往标准输出、标准错误各写 3.5 MB（都比管道的缓冲大得多），再把标准输入抄一遍
    let input: Vec<u8> = (0..3_000_000u32).map(|i| b'a' + (i % 26) as u8).collect();
    let script = "head -c 3500000 /dev/zero | tr '\\0' o; head -c 3500000 /dev/zero | tr '\\0' e >&2; cat";
    let out = sh(script).input(input.clone()).timeout(Duration::from_secs(120)).output().unwrap().check();
    assert_eq!((out.stdout.len(), out.stderr.len()), (3_500_000 + input.len(), 3_500_000));
    assert!(out.stdout[..3_500_000].iter().all(|b| *b == b'o') && out.stdout[3_500_000..] == input[..]);
    assert!(out.stderr.iter().all(|b| *b == b'e'));
    // 不读标准输入的程序：喂不进去的就算了
    assert_eq!(sh("echo done").input(vec![b'x'; 1_000_000]).output().unwrap().check().out(), "done\n");
    // 没给 input：标准输入是 /dev/null，读它的程序马上读到头
    assert_eq!(sh("cat; echo end").output().unwrap().check().out(), "end\n");
    // 退出码不是 0：check 带着两路输出 panic
    let failed = std::panic::catch_unwind(|| sh("echo 出; echo 错 >&2; exit 4").output().unwrap().check()).unwrap_err();
    assert_eq!(failed.downcast_ref::<String>().unwrap(), "命令没成（退出码 4）\n标准输出：出\n\n标准错误：错\n");
}

fn proc_timeout_kills(t: &mut Ctx) {
    let pidfile = t.tmp("kit-timeout-").join("pid");
    // output：到时限杀掉、收走；错误里带着到那时为止的输出
    let started = Instant::now();
    let err = sh("echo $$ > \"$0\"; echo 说了一半; echo 错了一半 >&2; exec sleep 300")
        .arg(&pidfile)
        .timeout(Duration::from_millis(1500))
        .output()
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::TimedOut);
    let text = err.to_string();
    assert!(text.contains("杀掉了") && text.contains("说了一半") && text.contains("错了一半"), "{text}");
    assert!(started.elapsed() < Duration::from_secs(20), "{:?}", started.elapsed());
    let pid: i32 = fsx::read_text(&pidfile).trim().parse().unwrap();
    assert!(!alive(pid), "到时限的进程收走了");
    // spawn 以后 wait：一样
    let mut child = sleeper().timeout(Duration::from_millis(200)).spawn().unwrap();
    assert_eq!(child.wait().unwrap_err().kind(), ErrorKind::TimedOut);
    assert!(!alive(child.pid()));
    assert_eq!(child.try_wait().unwrap().unwrap().signal, Some(libc::SIGKILL));
    // 时限之内完的不受影响
    assert_eq!(sh("echo ok").timeout(WAIT).output().unwrap().check().out(), "ok\n");
}

fn proc_only_named_fds_reach_the_child(t: &mut Ctx) {
    let dir = t.tmp("kit-fds-");
    let tool = harness::tool();
    // 这个进程里开着、没标“exec 时关掉”的描述符：一个都不该带过去
    let file = std::fs::File::create(dir.join("f")).unwrap();
    // SAFETY: dup 给的是新的描述符，马上交给 OwnedFd 管
    let plain: Vec<OwnedFd> = (0..3).map(|_| unsafe { OwnedFd::from_raw_fd(libc::dup(file.as_raw_fd())) }).collect();
    // SAFETY: 只是问这个描述符的标志
    assert!(plain.iter().all(|fd| unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) } == 0), "这几个没标 exec 时关掉");
    let fds_of = |cmd: &mut Cmd| cmd.output().unwrap().check().out().trim().to_string();
    assert_eq!(fds_of(Cmd::new(&tool).arg("fds")), "0 1 2");
    // 经 cca-test exec 换会话的也一样
    assert_eq!(fds_of(Cmd::new(&tool).arg("fds").session(Session::New)), "0 1 2");
    // 点了名的带过去，号码照给的
    let (r1, w1) = std::io::pipe().unwrap();
    let (r2, w2) = std::io::pipe().unwrap();
    let given = w1.as_raw_fd();
    let mut cmd = Cmd::new(&tool);
    cmd.arg("fds").fd(7, w1.into()).fd(4, r2.into());
    assert_eq!(fds_of(&mut cmd), "0 1 2 4 7");
    // 交出去的那一份，这边已经关了；同一个 Cmd 再起一次：给过的描述符用掉了，说出来、不悄悄少带
    // SAFETY: 只是问这个号还开着没有
    assert!(unsafe { libc::fcntl(given, libc::F_GETFD) } < 0);
    assert_eq!(cmd.output().unwrap_err().kind(), ErrorKind::InvalidInput);
    for session in [Session::Same, Session::New] {
        // 带过去的能用：子进程往 5 号写，这边读得到，而且读得到头（写的那一头这边没留着）
        let (mut read, write) = std::io::pipe().unwrap();
        sh("echo 经 5 号写的 >&5").fd(5, write.into()).session(session).output().unwrap().check();
        let mut text = String::new();
        read.read_to_string(&mut text).unwrap();
        assert_eq!(text, "经 5 号写的\n");
    }
    // 0、1、2 号不归 fd() 管；一个号不能给两次
    let null = || OwnedFd::from(std::fs::File::open("/dev/null").unwrap());
    assert_eq!(sh("true").fd(1, null()).output().unwrap_err().kind(), ErrorKind::InvalidInput);
    assert_eq!(sh("true").fd(5, null()).fd(5, null()).output().unwrap_err().kind(), ErrorKind::InvalidInput);
    drop((plain, r1, w2));
}

fn proc_finds_programs_on_the_childs_path(t: &mut Ctx) {
    let dir = t.tmp("kit-path-");
    let bin = dir.join("bin");
    fsx::write_script(bin.join("kit-fake-tool"), "#!/bin/sh\necho 假的 \"$@\"\n");
    assert!(proc::which("kit-fake-tool").is_none(), "这个进程的 PATH 上没有它");
    assert!(proc::which("sh").is_some());
    // 只写了名字的：照子进程的 PATH 找
    let out = Cmd::new("kit-fake-tool").arg("甲").env("PATH", format!("{}:/usr/bin:/bin", bin.display())).output().unwrap().check();
    assert_eq!(out.out(), "假的 甲\n");
    // PATH 上靠前的顶替靠后的：假命令就是这样顶替真命令的
    fsx::write_script(bin.join("ls"), "#!/bin/sh\necho 假的 ls\n");
    assert_eq!(Cmd::new("ls").env("PATH", format!("{}:/usr/bin:/bin", bin.display())).output().unwrap().check().out(), "假的 ls\n");
    assert_ne!(
        Cmd::new("ls").env("PATH", format!("/usr/bin:/bin:{}", bin.display())).cwd(&dir).output().unwrap().check().out(),
        "假的 ls\n"
    );
    // 子进程没有 PATH：只找 /usr/bin 和 /bin
    assert!(Cmd::new("env").output().unwrap().status.success());
    let err = Cmd::new("kit-fake-tool").output().unwrap_err();
    assert!(err.kind() == ErrorKind::NotFound && err.to_string().contains("kit-fake-tool"), "{err}");
    // 带 / 的是路径：相对路径从子进程的当前目录算
    assert_eq!(Cmd::new("./bin/kit-fake-tool").cwd(&dir).output().unwrap().check().out(), "假的\n");
    // 不在的、不能运行的：起之前就说出来
    fsx::write(dir.join("plain"), "不是程序");
    assert_eq!(Cmd::new(dir.join("plain")).output().unwrap_err().kind(), ErrorKind::PermissionDenied);
    assert_eq!(Cmd::new(dir.join("missing")).output().unwrap_err().kind(), ErrorKind::NotFound);
}

fn proc_children_die_with_their_handle(t: &mut Ctx) {
    let dir = t.tmp("kit-drop-");
    // Child 离开作用域：TERM，收走
    let child = sleeper().spawn().unwrap();
    let pid = child.pid();
    assert!(alive(pid));
    drop(child);
    assert!(!alive(pid));
    // 新会话起的：连它那个进程组里的后台进程一起停
    let pidfile = dir.join("bg.pid");
    let mut child = sh("sleep 300 & echo $! > \"$0\"; wait").arg(&pidfile).session(Session::New).spawn().unwrap();
    let bg: i32 = wait_for("后台进程的进程号写下来", WAIT, || {
        fsx::try_read_text(&pidfile).filter(|s| s.ends_with('\n')).and_then(|s| s.trim().parse().ok())
    });
    assert!(alive(bg));
    assert_eq!(child.stop().unwrap().signal, Some(libc::SIGTERM));
    wait_for("进程组里的后台进程也退出", WAIT, || !alive(bg));
    // 不理 TERM 的：2 秒后 KILL
    let mut child = sh("trap '' TERM; echo ready; while :; do sleep 1; done").stdout(Stdio::Piped).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    assert_eq!(line, "ready\n");
    let started = Instant::now();
    assert_eq!(child.stop().unwrap().signal, Some(libc::SIGKILL));
    assert!(
        started.elapsed() >= Duration::from_millis(1900) && started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    // kill_all：登记着的都停掉，说出停了几个
    let mut a = sleeper().spawn().unwrap();
    let mut b = sleeper().session(Session::New).spawn().unwrap();
    assert_eq!(proc::kill_all(), 2);
    assert_eq!(
        (a.try_wait().unwrap().unwrap().signal, b.try_wait().unwrap().unwrap().signal),
        (Some(libc::SIGTERM), Some(libc::SIGTERM))
    );
    assert_eq!(proc::kill_all(), 0);
}

fn proc_children_get_clean_signals(_t: &mut Ctx) {
    // 这条线程屏蔽着 USR1、这个进程不理 USR2（Rust 的程序本来就不理 PIPE）：都不该留给子进程
    // SAFETY: 两个信号集清零后是有效的；改的是这条线程的屏蔽和 USR2 的处理方式，下面改回去
    let (old_mask, old_usr2) = unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        let mut old: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGUSR1);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut old);
        (old, libc::signal(libc::SIGUSR2, libc::SIG_IGN))
    };
    let mut seen = Vec::new();
    for session in [Session::Same, Session::New] {
        for (name, signal) in [("USR1", libc::SIGUSR1), ("USR2", libc::SIGUSR2), ("PIPE", libc::SIGPIPE)] {
            let out = sh(&format!("kill -{name} $$; echo survived")).session(session.clone()).output().unwrap();
            seen.push((name, session.clone(), signal, out.status.signal, out.out()));
        }
    }
    // SAFETY: 改回去
    unsafe {
        libc::pthread_sigmask(libc::SIG_SETMASK, &old_mask, std::ptr::null_mut());
        libc::signal(libc::SIGUSR2, old_usr2);
    }
    for (name, session, signal, died_of, out) in seen {
        assert_eq!(died_of, Some(signal), "{name}（{session:?}）在子进程里不是默认的处理方式：{out:?}");
    }
}

fn proc_cwd_and_arg0(t: &mut Ctx) {
    let dir = t.tmp("kit-cwd-");
    for session in [Session::Same, Session::New] {
        assert_eq!(sh("pwd -P").cwd(&dir).session(session.clone()).output().unwrap().check().out().trim(), dir.to_str().unwrap());
        // argv[0] 默认就是 new 里写的那个；另外起名的话，进程列表里看到的就是那个名字。
        // 这里起的名字得是 sh 认识的：busybox 照 argv[0] 认自己是哪个命令
        assert_eq!(sh("echo \"$0\"").session(session.clone()).output().unwrap().check().out(), "/bin/sh\n");
        let out = Cmd::new("/bin/sh").arg0("sh").args(["-c", "echo \"$0\""]).session(session).output().unwrap().check();
        assert_eq!(out.out(), "sh\n");
    }
    let err = sh("true").cwd(dir.join("没有的目录")).output().unwrap_err();
    assert!(err.kind() == ErrorKind::NotFound && err.to_string().contains("没有的目录"), "{err}");
}

fn proc_stdio_by_hand(t: &mut Ctx) {
    let dir = t.tmp("kit-stdio-");
    // 管道：一句一句地喂、一句一句地读
    let mut child = sh("while read line; do echo \"got:$line\"; done").stdin(Stdio::Piped).stdout(Stdio::Piped).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    for word in ["一", "二"] {
        writeln!(stdin, "{word}").unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        assert_eq!(line, format!("got:{word}\n"));
    }
    drop(stdin);
    assert!(child.wait().unwrap().success());
    // 文件：标准输出写进文件，标准错误并到同一个文件里
    let log = std::fs::File::create(dir.join("log")).unwrap();
    let both = log.try_clone().unwrap();
    assert!(sh("echo 出; echo 错 >&2").stdout(log).stderr(both).spawn().unwrap().wait().unwrap().success());
    assert_eq!(fsx::read_text(dir.join("log")), "出\n错\n");
    // 文件当标准输入
    fsx::write(dir.join("in"), "文件里的\n");
    assert_eq!(sh("cat").stdin(std::fs::File::open(dir.join("in")).unwrap()).output().unwrap().check().out(), "文件里的\n");
}

/// 这个进程现在开着的文件描述符
fn open_fds() -> Vec<i32> {
    // SAFETY: 只是挨个问这个号开着没有
    (0..1024).filter(|fd| unsafe { libc::fcntl(*fd, libc::F_GETFD) } >= 0).collect()
}

/// 各种起法各来一遍：收输出的、管道自己拿着的、换会话的、带描述符的、到时限的、起不来的、伪终端上的
fn spawn_every_way(dir: &Path) {
    sh("echo out; echo err >&2; cat").input("喂进去的").output().unwrap().check();
    let mut child = sh("cat").stdin(Stdio::Piped).stdout(Stdio::Piped).stderr(Stdio::Piped).spawn().unwrap();
    assert!(child.wait().unwrap().success());
    drop(child);
    sh("true").session(Session::New).stdout(Stdio::Piped).spawn().unwrap().wait().unwrap();
    let (read, write) = std::io::pipe().unwrap();
    sh("true").fd(5, write.into()).output().unwrap().check();
    drop(read);
    assert_eq!(sleeper().timeout(Duration::from_millis(1)).output().unwrap_err().kind(), ErrorKind::TimedOut);
    // 起不来的：找不到程序、目录不在（都是起之前查出来的），系统不认这个文件（管道都建好了才知道）
    let (_read, write) = std::io::pipe().unwrap();
    assert_eq!(
        Cmd::new(dir.join("missing")).stdin(Stdio::Piped).fd(5, write.into()).output().unwrap_err().kind(),
        ErrorKind::NotFound
    );
    assert_eq!(sh("true").cwd(dir.join("missing")).session(Session::New).output().unwrap_err().kind(), ErrorKind::NotFound);
    let (_read, write) = std::io::pipe().unwrap();
    Cmd::new(dir.join("not-a-program")).stdin(Stdio::Piped).fd(5, write.into()).output().unwrap_err();
    let mut pty = Pty::open(24, 80).unwrap();
    let mut child = pty.spawn(&mut sh("echo hi")).unwrap();
    assert!(pty.wait(&mut child, WAIT).unwrap().success());
}

fn proc_spawning_leaks_nothing(t: &mut Ctx) {
    let dir = t.tmp("kit-leak-");
    // 能执行、却不是程序（没有 #! 那一行）
    fsx::write_script(dir.join("not-a-program"), "不是程序\n");
    // 先来一遍（头一次用到才开的东西不算），再记下开着哪些描述符
    spawn_every_way(&dir);
    let before = open_fds();
    for _ in 0..12 {
        spawn_every_way(&dir);
    }
    // 数得出多开的一个
    let extra = std::fs::File::open("/dev/null").unwrap();
    assert_eq!(open_fds().len(), before.len() + 1);
    drop(extra);
    // 一个模块要起几百上千次子进程，macOS 上一个进程默认只能开 256 个描述符：漏一个都不行
    assert_eq!(open_fds(), before, "起过子进程以后，这个进程里不该多出描述符");
    // 也没有没收走的子进程（退出了却没人收的会一直占着进程表）
    // SAFETY: 不等，只是问有没有可以收的子进程——这时候一个子进程都不该有
    let rc = unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) };
    assert!(
        rc == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD),
        "还有没收走的子进程（waitpid 给的是 {rc}）"
    );
}

// —— cca-test exec：换会话、接终端 ——

fn exec_starts_a_new_session(t: &mut Ctx) {
    // SAFETY: getsid、getpgid 只是问一下
    let ids = |pid: i32| unsafe { (libc::getsid(pid), libc::getpgid(pid)) };
    let ours = ids(0);
    let mut same = sleeper().spawn().unwrap();
    assert_eq!(ids(same.pid()), ours, "不换会话的：和这个进程同一个会话、同一个进程组");
    // 换会话的：进程号没变（就是 spawn 给的那个），它是新会话、新进程组的头
    let mut own = sleeper().session(Session::New).spawn().unwrap();
    let pid = own.pid();
    wait_for("换好会话", WAIT, || ids(pid) == (pid, pid));
    assert_ne!(pid, ours.0);
    // 新会话没有控制终端
    let out = sh("( : < /dev/tty ) 2>/dev/null && echo has-ctty || echo no-ctty").session(Session::New).output().unwrap().check();
    assert_eq!(out.out(), "no-ctty\n");
    // signal_group：给它带头的进程组发信号——只有新会话起的才有自己的进程组
    assert_eq!(same.signal_group(libc::SIGTERM).unwrap_err().kind(), ErrorKind::InvalidInput);
    let pidfile = t.tmp("kit-session-").join("bg.pid");
    let mut group = sh("sleep 300 & echo $! > \"$0\"; wait").arg(&pidfile).session(Session::New).spawn().unwrap();
    let bg: i32 = wait_for("后台进程的进程号写下来", WAIT, || {
        fsx::try_read_text(&pidfile).filter(|s| s.ends_with('\n')).and_then(|s| s.trim().parse().ok())
    });
    assert_eq!(ids(bg), (group.pid(), group.pid()), "后台进程在它那个会话、那个进程组里");
    group.signal_group(libc::SIGKILL).unwrap();
    assert_eq!(group.wait().unwrap().signal, Some(libc::SIGKILL));
    wait_for("进程组里的都退出", WAIT, || !alive(bg));
    same.kill().unwrap();
    own.kill().unwrap();
    assert_eq!((same.wait().unwrap().signal, own.wait().unwrap().signal), (Some(libc::SIGKILL), Some(libc::SIGKILL)));
}

fn exec_says_why_it_could_not_run(t: &mut Ctx) {
    let tool = harness::tool();
    let run = |args: &[&str]| {
        let out = Cmd::new(&tool).arg("exec").args(args).output().unwrap();
        (out.status.py(), out.out(), out.err())
    };
    // 哪一步不成：说一行，退出码 127，那个程序没有运行
    let cases: [(&[&str], &str); 4] = [
        (&["--", "/nonexistent/cca-kit-prog", "x"], "cca-test exec：运行不了 /nonexistent/cca-kit-prog："),
        (&["--cwd", "/nonexistent/cca-kit-dir", "--", "/bin/echo", "ran"], "cca-test exec：进不了目录 /nonexistent/cca-kit-dir："),
        (
            &["--tty-after-setsid", "/nonexistent/cca-kit-tty", "--", "/bin/echo", "ran"],
            "cca-test exec：打不开终端 /nonexistent/cca-kit-tty：",
        ),
        // 标准输入不是终端
        (&["--setsid", "--ctty", "--", "/bin/echo", "ran"], "cca-test exec：标准输入认不成控制终端（TIOCSCTTY）："),
    ];
    for (args, why) in cases {
        let (code, out, err) = run(args);
        assert_eq!((code, out.as_str()), (127, ""), "{args:?}：{err}");
        assert!(err.starts_with(why) && err.ends_with('\n') && err.lines().count() == 1, "{args:?}：{err:?}");
    }
    // 已经是会话的头了：换不了第二次
    let (code, out, err) = run(&["--setsid", "--", tool.to_str().unwrap(), "exec", "--setsid", "--", "/bin/echo", "ran"]);
    assert_eq!((code, out.as_str()), (127, ""), "{err}");
    assert!(err.starts_with("cca-test exec：换不了会话（setsid）："), "{err:?}");
    // 用法不对：退出码 2
    for args in [&["/bin/echo", "ran"][..], &["--"], &["--cwd"], &["--bogus", "--", "/bin/echo"]] {
        let (code, out, err) = run(args);
        assert_eq!((code, out.as_str()), (2, ""), "{args:?}：{err}");
    }
    // 好好的：换目录、另起名字（sh 认识的名字：busybox 照 argv[0] 认自己是哪个命令）；退出码是那个程序自己的
    let dir = t.tmp("kit-exec-");
    let (code, out, err) =
        run(&["--cwd", dir.to_str().unwrap(), "--arg0", "sh", "--", "/bin/sh", "-c", "echo \"$0\"; pwd -P; exit 7"]);
    assert_eq!((code, out, err), (7, format!("sh\n{}\n", dir.display()), String::new()));
}

fn exec_opens_the_terminal_after_setsid(_t: &mut Ctx) {
    // 界面 App 起终端的办法：新会话里把终端打开、接成 0、1、2。Linux 上它就成了控制终端，macOS 上不会。
    // macOS 上不能拿 bash（/bin/sh）试：它当了会话的头、标准输入又是终端时，自己会去把终端认成控制终端
    let shell = if cfg!(target_os = "macos") { "/bin/dash" } else { "/bin/sh" };
    let mut pty = Pty::open(24, 80).unwrap();
    let mut cmd = Cmd::new(shell);
    cmd.args(["-c", "tty; ( : < /dev/tty ) 2>/dev/null && echo has-ctty || echo no-ctty"]).env("PATH", "/usr/bin:/bin");
    let mut child = cmd.session(Session::NewOpenTty(pty.slave_path().to_path_buf())).spawn().unwrap();
    // 这边的从的那头留着：它没认控制终端时，最后一个从的那头一关，没读走的输出就丢了
    assert!(pty.wait(&mut child, WAIT).unwrap().success());
    let text = String::from_utf8_lossy(&pty.out).into_owned();
    let expected = if cfg!(target_os = "linux") { "has-ctty" } else { "no-ctty" };
    assert_eq!(text.lines().collect::<Vec<_>>(), [pty.slave_path().to_str().unwrap(), expected]);
}

// —— kit::pty ——

fn pty_program_gets_a_controlling_terminal(_t: &mut Ctx) {
    let mut pty = Pty::open(24, 80).unwrap();
    let script =
        "tty; ( : < /dev/tty ) 2>/dev/null && echo has-ctty || echo no-ctty; ps -o tt= -p $$ 2>/dev/null | tr -d ' '; exit 5";
    let mut child = pty.spawn(&mut sh(script)).unwrap();
    assert_eq!(pty.wait(&mut child, WAIT).unwrap().py(), 5);
    let text = String::from_utf8_lossy(&pty.out).into_owned();
    let lines: Vec<&str> = text.lines().collect();
    let path = pty.slave_path().to_str().unwrap();
    assert_eq!(lines[..2], [path, "has-ctty"], "{text:?}");
    // ps 里的终端一栏：macOS 上 /dev/ttys012 写成 s012，Linux 上 /dev/pts/3 写成 pts/3。
    // Linux 上不一定有 ps，有的（busybox 的）也不认这个写法：它什么都没说就不看这一栏
    let short = path.strip_prefix(if cfg!(target_os = "macos") { "/dev/tty" } else { "/dev/" }).unwrap();
    let shown = lines.get(2).copied().unwrap_or_default();
    if cfg!(target_os = "macos") || !shown.is_empty() {
        assert_eq!(shown, short, "{text:?}");
    }
    // 它那个会话结束了、这边也没留着从的那头：读到头
    assert!(!pty.pump(Duration::from_millis(200)));
}

fn pty_keys_and_resize_reach_the_program(_t: &mut Ctx) {
    let mut pty = Pty::open(24, 80).unwrap();
    let script = "trap 'stty size' WINCH; trap 'echo got-int; exit 9' INT; stty size; echo ready; while :; do sleep 0.1; done";
    let mut child = pty.spawn(&mut sh(script)).unwrap();
    let at = pty.expect("24 80\r\nready\r\n", WAIT);
    // 改窗口大小：前台的程序收到 SIGWINCH，问到的是新的大小
    pty.resize(40, 132).unwrap();
    let at = pty.expect_since("40 132\r\n", WAIT, at);
    // 打 Ctrl-C：终端给前台的进程组发 SIGINT（它是这个终端的会话才有这回事）
    pty.send([3u8]).unwrap();
    pty.expect_since("got-int\r\n", WAIT, at);
    assert_eq!(pty.wait(&mut child, WAIT).unwrap().py(), 9);
}

// —— kit::fsx ——

fn fsx_wait_for_says_what_it_waited_for(_t: &mut Ctx) {
    let started = Instant::now();
    let failed = std::panic::catch_unwind(|| wait_for("后台进程写下结果", Duration::from_millis(150), || false)).unwrap_err();
    assert_eq!(failed.downcast_ref::<String>().unwrap(), "没等到：后台进程写下结果");
    assert!(started.elapsed() >= Duration::from_millis(150) && started.elapsed() < WAIT, "{:?}", started.elapsed());
    // 成立了就回来，带着值
    let mut n = 0;
    let got = wait_for("数到 3", WAIT, || {
        n += 1;
        (n >= 3).then_some(n * 10)
    });
    assert_eq!(got, 30);
    wait_for("本来就成立的", Duration::ZERO, || true);
}

fn fsx_temp_dirs_live_under_the_module_dir(t: &mut Ctx) {
    // 运行器给每个模块指了一个临时目录：用例建的都在它下面，模块完了整个删掉
    let base = fsx::tmp_base();
    let given = std::env::var_os("CCA_SUITE_TMP").expect("运行器给的 CCA_SUITE_TMP");
    assert_eq!(base, std::fs::canonicalize(given).unwrap());
    assert!(base.ends_with("kit_selftest"), "{base:?}");
    let dir = t.tmp("kit-a-");
    let name = dir.file_name().unwrap().to_str().unwrap();
    assert!(dir.parent() == Some(&*base) && name.starts_with("kit-a-") && name.len() == "kit-a-".len() + 6, "{dir:?}");
    assert_eq!(fsx::mode(&dir), 0o700);
    assert_ne!(t.tmp("kit-a-"), dir, "每次一个新的");
    // 自己管的临时目录：离开作用域就删（连里面不让写的目录一起）
    let own = fsx::TempDir::new("kit-b-");
    let path = own.path().to_path_buf();
    fsx::write(path.join("深/一层/文件"), "x");
    fsx::chmod(path.join("深/一层"), 0o500);
    drop(own);
    assert_eq!(path.exists(), fsx::keep());
}

// —— kit::harness，从里面看得到的 ——

fn harness_knows_where_things_are(_t: &mut Ctx) {
    let repo = harness::repo();
    assert!(
        repo.join("tests/kit/harness.rs").is_file() && repo.join("cli/Cargo.toml").is_file(),
        "仓库不在 {repo:?}（编好的程序拿到别处跑要设 CCA_REPO）"
    );
    let tool = harness::tool();
    assert!(tool.is_file() && proc::tool().unwrap() == tool, "{tool:?}");
    assert!(harness::helper().ends_with("cca-core") || std::env::var_os("CCA_HELPER").is_some(), "{:?}", harness::helper());
    // 模块的进程里 HOME 指着一个建不出来的地方：忘了给被测的程序指临时 HOME 的用例马上出错，碰不到真的主目录
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    assert!(!home.exists() && std::fs::create_dir_all(&home).is_err(), "{home:?}");
}

/// 上一个用例登记的收尾跑的先后（下一个用例来看）
static ORDER: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// 上一个用例的临时目录
static LAST_TMP: Mutex<Option<PathBuf>> = Mutex::new(None);

fn ctx_defers_are_registered(t: &mut Ctx) {
    assert_eq!((t.module(), t.name()), ("kit_selftest", "ctx_defers_are_registered"));
    let dir = t.tmp("kit-ctx-");
    fsx::write(dir.join("f"), "x");
    *LAST_TMP.lock().unwrap() = Some(dir.clone());
    ORDER.lock().unwrap().clear();
    for n in 1..=3 {
        t.defer(move || ORDER.lock().unwrap().push(format!("defer {n}")));
    }
    // 收尾跑的时候临时目录还在（收尾都跑完才删）
    t.defer(move || ORDER.lock().unwrap().push(format!("tmp there: {}", dir.join("f").exists())));
    ORDER.lock().unwrap().push("body".to_string());
}

fn ctx_defers_ran_in_reverse(t: &mut Ctx) {
    let order = ORDER.lock().unwrap().clone();
    if order.is_empty() {
        t.skip("要和上一个用例（ctx_defers_are_registered）一起跑：它登记，这里看结果");
    }
    assert_eq!(order, ["body", "tmp there: true", "defer 3", "defer 2", "defer 1"]);
    let dir = LAST_TMP.lock().unwrap().clone().unwrap();
    assert_eq!(dir.exists(), fsx::keep(), "用例的临时目录在它的收尾之后删掉");
}

fn ctx_subs_run_in_place(t: &mut Ctx) {
    let mut seen = Vec::new();
    for n in 0..3 {
        t.sub(format!("第 {n} 个"), |t| seen.push((n, t.name())));
    }
    assert_eq!(seen, [(0, "ctx_subs_run_in_place"), (1, "ctx_subs_run_in_place"), (2, "ctx_subs_run_in_place")]);
    // 可以套着
    let mut inner = false;
    t.sub("外", |t| t.sub("里", |_t| inner = true));
    assert!(inner);
}

// —— 运行器，从外面看：起一份 suite 跑 kit 里留的那几个模块 ——

/// 起过一份 suite 以后，从外面看得到的
struct Ran {
    /// 退出码（被信号杀掉的是 128 + 信号）
    code: i32,
    /// 它的标准输出（秒数都换成了 T）、标准错误
    out: String,
    err: String,
    /// 结果文件（秒数都换成了 T）
    results: String,
    /// 用例一行行记下的事
    noted: String,
    /// --results、--logs、FX_OUT、CCA_SUITE_TMP 都指在这下面
    dir: PathBuf,
    took: Duration,
}

impl Ran {
    /// 一个模块的记录（秒数都换成了 T）
    fn log(&self, module: &str) -> String {
        shape(&fsx::read_text(self.dir.join("logs").join(format!("{module}.log"))))
    }

    /// 用例记下的以 key 开头的那一行，key 后面的部分
    fn noted_value(&self, key: &str) -> &str {
        self.noted.lines().find_map(|line| line.strip_prefix(key)).unwrap_or_else(|| panic!("用例没记下 {key}：{}", self.noted))
    }
}

/// 每次都不一样的秒数换成 T：记录里状态那一行的、结果文件和 timing.tsv 里最后一栏的、屏幕上“… 秒”的
fn shape(text: &str) -> String {
    let rx = regex::Regex::new(r"(?m)^((?:ok|FAIL|skip|TIMEOUT|SLOW|NOTRUN) \S+ |(?:\S+\t){3,5})\d+\.\d\d|\d+\.\d\d( 秒)").unwrap();
    rx.replace_all(text, |caps: &regex::Captures| {
        format!("{}T{}", caps.get(1).map_or("", |m| m.as_str()), caps.get(2).map_or("", |m| m.as_str()))
    })
    .into_owned()
}

/// 跑 kit 里留的模块的那条命令：结果、记录、用例记的事、临时目录都放在 dir 下面
fn fixtures_cmd(dir: &Path, args: &[&str]) -> Cmd {
    let mut cmd = Cmd::new(std::env::current_exe().expect("自己这个程序在哪"));
    cmd.arg("--kit-fixtures").args(args).arg("--results").arg(dir.join("results")).arg("--logs").arg(dir.join("logs"));
    cmd.pass_env(&["PATH"])
        .env("CCA_SUITE_TMP", dir.join("tmp"))
        .env("FX_OUT", dir.join("noted"))
        .env("CCA_TEST_BIN", harness::tool());
    cmd
}

/// dir 下面还留着的东西
fn left_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default()
}

/// 起一份 suite 跑 kit 里留的模块，等它跑完
fn fixtures(t: &mut Ctx, args: &[&str], env: &[(&str, &str)]) -> Ran {
    let dir = t.tmp("kit-run-");
    let started = Instant::now();
    let out = fixtures_cmd(&dir, args).envs(env.iter().copied()).timeout(Duration::from_secs(120)).output().expect("起 suite");
    let read = |name: &str| fsx::try_read_text(dir.join(name)).unwrap_or_default();
    let ran = Ran {
        code: out.status.shell(),
        out: shape(&out.out()),
        err: out.err(),
        results: shape(&read("results")),
        noted: read("noted"),
        dir: dir.clone(),
        took: started.elapsed(),
    };
    if !env.iter().any(|(key, _)| *key == "KEEP") {
        assert_eq!(left_in(&dir.join("tmp")), Vec::<String>::new(), "跑完了临时目录里不该留东西\n{}\n{}", ran.out, ran.err);
    }
    ran
}

fn runner_reports_each_test_and_module(t: &mut Ctx) {
    let ran = fixtures(t, &["--module", "fx_fail", "--module", "fx_pass", "--jobs", "1"], &[]);
    assert_eq!((ran.code, ran.err.as_str()), (1, ""), "有模块没过：退出码 1\n{}", ran.out);
    // 结果文件：每个模块一行，照模块表的先后（不是命令行上的先后）
    assert_eq!(ran.results, "fx_pass\tpass\t4\t0\t2\tT\nfx_fail\tfail\t2\t3\t0\tT\n");
    // 记录：每个用例一行；跳过的带着原因
    let log = "ok plain T\n\
        skip skipped T 这台机器上没有这个条件\n\
        skip needs_missing T 没有 cca-kit-no-such-command\n\
        ok tmp_and_defer T\n\
        ok subs_and_names T 跳过了 1 个子用例：跳过的：子用例也能跳过\n\
        ok after_all T\n";
    assert_eq!(ran.log("fx_pass"), log);
    // 失败的后面缩进 4 格跟着它的输出：自己说的、子进程说的、panic 说的；一个失败不耽误后面的
    let log = ran.log("fx_fail");
    assert!(
        log.starts_with(
            "ok first_ok T\nFAIL panics T\n    标准输出里的一行\n    标准错误里的一行\n    子进程说的\n    子进程的错\n"
        ),
        "{log}"
    );
    for part in [
        "kit/harness/fixtures.rs:",
        "\n    故意失败：42\n",
        "\n    用例 panic 了（说了什么在上面）\nFAIL sub_fails T\n",
        "\n    assertion `left == right` failed: 甲不对\n",
        "\n    子用例（甲）失败\n",
        "\n    丙里面不对\n    子用例（丙 / 里面的）失败\n    2 个子用例失败：甲；丙 / 里面的\nFAIL defer_panics T\n",
        "\n    收尾不成\n    1 件收尾的事 panic 了\nok last_ok T\n",
    ] {
        assert!(log.contains(part), "记录里该有 {part:?}：\n{log}");
    }
    assert!(log.ends_with("ok last_ok T\n"), "{log}");
    let status_lines = |text: &str| text.lines().filter(|l| !l.starts_with("    ")).map(str::to_string).collect::<Vec<_>>();
    assert_eq!(status_lines(&log), ["ok first_ok T", "FAIL panics T", "FAIL sub_fails T", "FAIL defer_panics T", "ok last_ok T"]);
    // timing.tsv：每个用例一行
    let timing =
        "fx_pass\tplain\tok\tT\nfx_pass\tskipped\tskip\tT\nfx_pass\tneeds_missing\tskip\tT\nfx_pass\ttmp_and_defer\tok\tT\n\
        fx_pass\tsubs_and_names\tok\tT\nfx_pass\tafter_all\tok\tT\nfx_fail\tfirst_ok\tok\tT\nfx_fail\tpanics\tFAIL\tT\n\
        fx_fail\tsub_fails\tFAIL\tT\nfx_fail\tdefer_panics\tFAIL\tT\nfx_fail\tlast_ok\tok\tT\n";
    assert_eq!(shape(&fsx::read_text(ran.dir.join("logs/timing.tsv"))), timing);
    // 屏幕上：用例一行行报，每个模块一句，最后一句总的
    let said = [
        "ok fx_pass::plain T",
        "skip fx_pass::skipped T 这台机器上没有这个条件",
        "skip fx_pass::needs_missing T 没有 cca-kit-no-such-command",
        "ok fx_pass::tmp_and_defer T",
        "ok fx_pass::subs_and_names T 跳过了 1 个子用例：跳过的：子用例也能跳过",
        "ok fx_pass::after_all T",
        "模块 fx_pass：pass，通过 4，失败 0，跳过 2，T 秒",
        "ok fx_fail::first_ok T",
        "FAIL fx_fail::panics T",
        "FAIL fx_fail::sub_fails T",
        "FAIL fx_fail::defer_panics T",
        "ok fx_fail::last_ok T",
        "模块 fx_fail：fail，通过 2，失败 3，跳过 0，T 秒",
        "suite：2 个模块，1 个没过（fx_fail）；用例通过 6，失败 3，跳过 2；T 秒",
    ];
    assert_eq!(status_lines(&ran.out), said);
    assert!(ran.out.contains("FAIL fx_fail::panics T\n    标准输出里的一行\n"), "{}", ran.out);
    // 用例里做的事的先后：收尾照登记的反序、在用例之后（panic 了也跑），模块的收尾等用例都跑完；跳过的、条件不够的没往下跑
    let tmps: Vec<&str> = ran.noted.lines().filter_map(|l| l.strip_prefix("tmp /")).collect();
    let noted: Vec<&str> = ran.noted.lines().filter(|l| !l.starts_with("tmp /")).collect();
    let did = [
        "skipped: before",
        "body end",
        "tmp still there in defer: true",
        "defer 3",
        "defer 2",
        "defer 1",
        "ctx fx_pass subs_and_names",
        "subs [0, 1, 2]",
        "after_all",
        "module defer 2",
        "module defer 1",
        "defer after panic 2",
        "defer after panic 1",
        "sub_fails: reached the end",
        "defer_panics: the earlier one still runs",
        "last_ok",
    ];
    assert_eq!(noted, did);
    // 用例的临时目录建在这次运行、这个模块的目录下面，已经删掉了
    assert_eq!(tmps.len(), 2);
    for (tmp, module) in tmps.iter().zip(["fx_pass", "fx_fail"]) {
        let tmp = PathBuf::from(format!("/{tmp}"));
        let run_dir = tmp.parent().unwrap().parent().unwrap();
        assert!(tmp.parent().unwrap().ends_with(module) && run_dir.parent() == Some(&*ran.dir.join("tmp")), "{tmp:?}");
        assert!(run_dir.file_name().unwrap().to_str().unwrap().starts_with("cca-suite.") && !tmp.exists(), "{tmp:?}");
    }
}

fn runner_stops_a_module_at_the_time_limit(t: &mut Ctx) {
    let ran = fixtures(t, &["--module", "fx_timeout", "--module", "fx_audit", "--jobs", "2"], &[]);
    assert_eq!((ran.code, ran.err.as_str()), (1, ""), "{}", ran.out);
    // 到时限的算失败，后面的用例没跑（不算通过、不算失败，模块算没过）；结果照模块表的先后，哪怕后面的模块先跑完
    assert_eq!(ran.results, "fx_timeout\tfail\t1\t1\t0\tT\nfx_audit\tfail\t1\t1\t0\tT\n");
    assert!(
        ran.out.find("模块 fx_audit：").unwrap()
            < ran.out.find("模块 fx_timeout：fail，通过 1，失败 1，跳过 0，没跑 1，T 秒").unwrap(),
        "{}",
        ran.out
    );
    let log = "ok quick T\n\
        TIMEOUT hangs T\n\
        \x20   卡住之前说的话\n\
        \x20   用例过了时限（1 秒）还没完：整个模块停下，后面的用例不跑了\n\
        NOTRUN never_runs T\n";
    assert_eq!(ran.log("fx_timeout"), log);
    // 登记的收尾都跑了：先用例的（反序），再模块的；没轮到的用例真的没跑
    let noted: Vec<&str> = ran
        .noted
        .lines()
        .filter(|l| !l.starts_with("tmp /") && !l.starts_with("pids ") && *l != "hangs: got past the wait")
        .collect();
    assert_eq!(noted, ["quick", "later defer on timeout", "defer on timeout", "module defer on timeout"]);
    // 它起过的子进程都停掉了：同一个会话的、自己一个会话的、那个会话里的后台进程
    let pids: Vec<i32> = ran.noted_value("pids ").split(' ').map(|p| p.parse().unwrap()).collect();
    assert_eq!(pids.len(), 3);
    wait_for("超时的用例起的子进程都退出", WAIT, || !pids.iter().any(|pid| alive(*pid)));
    // 时限 1 秒，加上收场：用不了多久
    assert!(ran.took >= Duration::from_secs(1) && ran.took < Duration::from_secs(25), "{:?}", ran.took);
    // 模块的进程自己：到了时限用 124 退出（手工起的没有上一级，结果说在标准错误上）
    let dir = t.tmp("kit-child-");
    let mut cmd = Cmd::new(std::env::current_exe().unwrap());
    cmd.args(["--kit-fixtures", "--child", "fx_timeout", "--module", "fx_timeout"]).pass_env(&["PATH"]);
    let out =
        cmd.env("CCA_SUITE_TMP", &dir).env("CCA_TEST_BIN", harness::tool()).timeout(Duration::from_secs(60)).output().unwrap();
    let said = out.err();
    assert_eq!(out.status.code, Some(124), "{said}");
    assert!(said.starts_with("ok quick ") && said.contains("\ntimeout hangs ") && !said.contains("never_runs"), "{said}");
    assert!(said.contains(" 卡住之前说的话\n用例过了时限（1 秒）还没完：整个模块停下，后面的用例不跑了\n"), "{said}");
}

fn runner_flags_slow_tests(t: &mut Ctx) {
    // 用掉了时限的一多半：照样算通过，多记一行 SLOW
    let ran = fixtures(t, &["--module", "fx_slow"], &[]);
    assert_eq!((ran.code, ran.results.as_str()), (0, "fx_slow\tpass\t1\t0\t0\tT\n"), "{}\n{}", ran.out, ran.err);
    assert_eq!(ran.log("fx_slow"), "ok takes_most_of_its_budget T\nSLOW takes_most_of_its_budget T 时限 4 秒\n");
    assert!(ran.out.contains("\nSLOW fx_slow::takes_most_of_its_budget T 时限 4 秒\n"), "{}", ran.out);
}

fn runner_names_unregistered_tests(t: &mut Ctx) {
    // 模块的原文里有一个像用例的函数没登记进表里：模块算没过，说出是哪个；登记了的照跑
    let ran = fixtures(t, &["--module", "fx_audit"], &[]);
    assert_eq!((ran.code, ran.results.as_str()), (1, "fx_audit\tfail\t1\t1\t0\tT\n"), "{}\n{}", ran.out, ran.err);
    assert_eq!(ran.log("fx_audit"), "FAIL (audit) T\n    这几个用例没登记在 fx_audit 的 tests 里：forgotten\nok listed T\n");
    // 这一份自己的表是全的（漏了的话这个模块已经报失败了）：表里的名字没有重的
    let mut names: Vec<&str> = MODULE.tests.iter().map(|test| test.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), MODULE.tests.len());
}

fn runner_runs_modules_side_by_side(t: &mut Ctx) {
    let both = ["--module", "fx_par_a", "--module", "fx_par_b"];
    // 同时跑两个：两个模块的用例互相看到对方正在跑。没说几个（默认 4 个）也是同时
    for jobs in [&["--jobs", "2"][..], &[]] {
        let ran = fixtures(t, &[&both[..], jobs].concat(), &[("FX_MEET_MS", "60000")]);
        assert_eq!(
            (ran.code, ran.results.as_str()),
            (0, "fx_par_a\tpass\t1\t0\t0\tT\nfx_par_b\tpass\t1\t0\t0\tT\n"),
            "{}\n{}",
            ran.out,
            ran.err
        );
        assert!(ran.noted.contains("saw fx_par_a\n") && ran.noted.contains("saw fx_par_b\n"), "{jobs:?}：{}", ran.noted);
    }
    // 一次一个：照模块表的先后，一个跑完才起下一个（libtest 的 --test-threads、环境里的 CCA_SUITE_JOBS 也认）
    let one_by_one = "enter fx_par_a\nleave fx_par_a\nenter fx_par_b\nleave fx_par_b\n";
    for (jobs, env) in [(&["--jobs", "1"][..], &[][..]), (&["--test-threads=1"], &[]), (&[], &[("CCA_SUITE_JOBS", "1")])] {
        let ran = fixtures(t, &[&both[..], jobs].concat(), &[&[("FX_MEET_MS", "150")], env].concat());
        assert_eq!((ran.code, ran.noted.as_str()), (0, one_by_one), "{jobs:?} {env:?}\n{}\n{}", ran.out, ran.err);
    }
}

fn runner_survives_a_module_that_dies(t: &mut Ctx) {
    // 模块的进程中途没了（被杀掉了；用例里调了 exit）：正在跑的用例算失败，后面的算没跑，别的模块不受影响
    let ran = fixtures(t, &["--module", "fx_crash", "--module", "fx_exit0", "--module", "fx_pass"], &[]);
    assert_eq!((ran.code, ran.err.as_str()), (1, ""), "{}", ran.out);
    assert_eq!(ran.results, "fx_pass\tpass\t4\t0\t2\tT\nfx_crash\tfail\t1\t1\t0\tT\nfx_exit0\tfail\t0\t1\t0\tT\n");
    assert_eq!(
        ran.log("fx_crash"),
        "ok before T\nFAIL dies T\n    模块的进程中途没了（被信号 9 杀掉），这个用例没有报结果\nNOTRUN after T\n"
    );
    assert_eq!(ran.log("fx_exit0"), "FAIL exits_early T\n    模块的进程中途没了（退出码 0），这个用例没有报结果\nNOTRUN after T\n");
}

fn runner_kills_a_module_that_cannot_stop_itself(t: &mut Ctx) {
    // 整个进程停住了（SIGSTOP）：它自己看时限的那条线程也不动。上一级过了时限再等一阵（这里调成 1 秒）就杀掉它
    let ran = fixtures(t, &["--module", "fx_stop", "--kit-grace", "1"], &[]);
    assert_eq!((ran.code, ran.results.as_str()), (1, "fx_stop\tfail\t0\t1\t0\tT\n"), "{}\n{}", ran.out, ran.err);
    assert_eq!(ran.log("fx_stop"), "TIMEOUT stops_itself T\n    用例到了时限、模块的进程自己没停下：杀掉了\nNOTRUN after T\n");
    assert!(ran.took >= Duration::from_secs(2) && ran.took < Duration::from_secs(25), "{:?}", ran.took);
}

fn runner_selects_modules_and_tests(t: &mut Ctx) {
    let mut list = |args: &[&str]| {
        let ran = fixtures(t, &[&["--list"], args].concat(), &[]);
        assert_eq!((ran.code, ran.err.as_str(), ran.results.as_str()), (0, "", ""), "--list 只列出来，不跑");
        ran.out.lines().map(str::to_string).collect::<Vec<_>>()
    };
    // 平时不跑的模块不在里面；--all 才有
    let usual = list(&[]);
    assert!(
        usual.starts_with(&["fx_pass::plain".to_string(), "fx_pass::skipped".to_string()])
            && usual.contains(&"fx_leak::leaves_processes_behind".to_string()),
        "{usual:?}"
    );
    assert!(!usual.iter().any(|name| name.starts_with("fx_off::") || name.starts_with("fx_hang::")), "{usual:?}");
    let all = list(&["--all"]);
    assert!(all.len() == usual.len() + 3 && all.contains(&"fx_off::only_when_asked".to_string()), "{all:?}");
    // 模块名：整个模块（平时不跑的，点了名也跑）；别的照 模块::用例 里的一截；--skip 去掉；--module 只在这几个模块里选
    assert_eq!(list(&["fx_exit0"]), ["fx_exit0::exits_early", "fx_exit0::after"]);
    assert_eq!(list(&["fx_off"]), ["fx_off::only_when_asked"]);
    assert_eq!(list(&["--module", "fx_off"]), ["fx_off::only_when_asked"]);
    assert_eq!(list(&["only_when"]), Vec::<String>::new());
    assert_eq!(list(&["fx_pass::sk", "fail::last"]), ["fx_pass::skipped", "fx_fail::last_ok"]);
    assert_eq!(
        list(&["fx_pass", "--skip", "::s", "--skip=defer"]),
        ["fx_pass::plain", "fx_pass::needs_missing", "fx_pass::after_all"]
    );
    assert_eq!(list(&["--module", "fx_crash", "--module=fx_stop", "after"]), ["fx_crash::after", "fx_stop::after"]);
    // 跑起来也是这几个
    let ran = fixtures(t, &["fx_pass::plain", "fx_fail::first_ok", "fx_off"], &[]);
    assert_eq!((ran.code, ran.noted.as_str()), (0, "off: ran\n"), "{}\n{}", ran.out, ran.err);
    assert_eq!(ran.results, "fx_pass\tpass\t1\t0\t0\tT\nfx_fail\tpass\t1\t0\t0\tT\nfx_off\tpass\t1\t0\t0\tT\n");
    assert_eq!(ran.log("fx_fail"), "ok first_ok T\n");
    // 什么都没选中：不算错（cargo test 把同一串参数交给每个测试程序）
    let ran = fixtures(t, &["没有这样的用例"], &[]);
    assert_eq!((ran.code, ran.out.as_str(), ran.results.as_str()), (0, "suite：没有选中的用例\n", ""), "{}", ran.err);
    // 写错了的：退出码 2，说清楚
    for (args, why) in [
        (&["--module", "fx_nope"][..], "suite：没有叫 fx_nope 的模块"),
        (&["--bogus"], "suite：不认识 --bogus"),
        (&["--jobs", "0"], "suite：--jobs 要一个不小于 1 的数"),
    ] {
        let ran = fixtures(t, args, &[]);
        assert!(
            ran.code == 2 && ran.err.starts_with(why) && ran.out.is_empty(),
            "{args:?}：{} {:?} {:?}",
            ran.code,
            ran.out,
            ran.err
        );
    }
}

fn runner_gives_modules_a_safe_environment(t: &mut Ctx) {
    let ran =
        fixtures(t, &["--module", "fx_env"], &[("HOME", "/Users/someone"), ("CCA_HELPER", "/x/cca-core"), ("CCA_REPO", "/x/repo")]);
    assert_eq!(ran.code, 0, "{}\n{}", ran.out, ran.err);
    // HOME 不是起它的人的；临时目录是这次运行、这个模块的
    assert_eq!(ran.noted_value("HOME="), "/dev/null/cca-suite-no-home");
    let module_tmp = PathBuf::from(ran.noted_value("CCA_SUITE_TMP="));
    assert!(module_tmp.starts_with(ran.dir.join("tmp")) && module_tmp.ends_with("fx_env"), "{module_tmp:?}");
    assert!(Path::new(ran.noted_value("tmp=")).starts_with(&module_tmp));
    // 辅助程序、仓库、cca-test：环境里给的
    assert_eq!((ran.noted_value("helper="), ran.noted_value("CCA_HELPER=")), ("/x/cca-core", "/x/cca-core"));
    assert_eq!((ran.noted_value("repo="), ran.noted_value("CCA_REPO=")), ("/x/repo", "/x/repo"));
    assert_eq!(
        (Path::new(ran.noted_value("tool=")), Path::new(ran.noted_value("CCA_TEST_BIN="))),
        (&*harness::tool(), &*harness::tool())
    );
    // 标准输入是 /dev/null
    assert_eq!(ran.noted_value("stdin-bytes="), "0");
    // 没给的：仓库是编这个程序的那一份，辅助程序是它里面编出来的
    let ran = fixtures(t, &["--module", "fx_env"], &[]);
    let repo = ran.noted_value("repo=");
    assert_eq!(Path::new(repo), Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap());
    assert_eq!(ran.noted_value("helper="), format!("{repo}/cli/target/release/cca-core"));
}

fn runner_setsid_gets_its_own_session(t: &mut Ctx) {
    // SAFETY: getsid 只是问一下
    let ours = unsafe { libc::getsid(0) };
    // 模块的进程报的：它的上一级（suite）的进程号、它在哪个会话里
    let ids = |noted: &str| {
        let value = |key: &str| noted.lines().find_map(|line| line.strip_prefix(key)).unwrap().parse::<i32>().unwrap();
        (value("ppid="), value("sid="))
    };
    // 不带 --setsid：和起它的在同一个会话里
    let ran = fixtures(t, &["--module", "fx_env"], &[]);
    assert_eq!(ids(&ran.noted).1, ours);
    // 带上：suite 自己一个会话——会话号就是它的进程号
    let ran = fixtures(t, &["--module", "fx_env", "--setsid"], &[]);
    let (suite, session) = ids(&ran.noted);
    assert!(ran.code == 0 && session == suite && session != ours, "{} {suite} {session} {ours}\n{}", ran.code, ran.err);
    // suite 自己已经是进程组的头时换不了（在交互的 shell 里直接运行的就是）：它另起一份换好会话的、等它、把退出码传回来
    let dir = t.tmp("kit-setsid-");
    let mut cmd = fixtures_cmd(&dir, &["--module", "fx_env", "--module", "fx_fail", "--setsid"]);
    let mut first = cmd.session(Session::New).stdout(Stdio::Null).stderr(Stdio::Null).spawn().unwrap();
    let exit = first.wait_timeout(WAIT).unwrap().expect("suite 该跑完了");
    assert_eq!(exit.code, Some(1), "退出码是另起的那一份的（有模块没过）");
    let (suite, session) = ids(&fsx::read_text(dir.join("noted")));
    assert!(session == suite && suite != first.pid(), "{suite} {session} {}", first.pid());
    assert_eq!(fsx::read_text(dir.join("results")).lines().count(), 2);
}

/// 起一份 suite 跑那个一直卡着的模块，等它的用例跑起来。给出 suite、模块的进程号、用例起的 sleep 的进程号
fn hanging_suite(dir: &Path) -> (Child, i32, i32) {
    let said = std::fs::File::create(dir.join("said")).unwrap();
    let suite =
        fixtures_cmd(dir, &["--module", "fx_hang"]).stdout(said.try_clone().unwrap()).stderr(said).spawn().expect("起 suite");
    let (module, sleeper) = wait_for("卡着的用例跑起来", WAIT, || {
        let noted = fsx::try_read_text(dir.join("noted")).filter(|noted| noted.contains("hang: ready\n"))?;
        let (module, sleeper) = noted.lines().find_map(|line| line.strip_prefix("hang: pids "))?.split_once(' ')?;
        Some((module.parse::<i32>().ok()?, sleeper.parse::<i32>().ok()?))
    });
    (suite, module, sleeper)
}

fn runner_cleans_up_when_interrupted(t: &mut Ctx) {
    let dir = t.tmp("kit-int-");
    let (mut suite, module, sleeper) = hanging_suite(&dir);
    assert!(alive(module) && alive(sleeper));
    suite.signal(libc::SIGTERM).unwrap();
    let exit = suite.wait_timeout(WAIT).unwrap().expect("收到 TERM 的 suite 该退出");
    assert_eq!(exit.code, Some(128 + libc::SIGTERM), "{}", fsx::read_text(dir.join("said")));
    // 模块收了场：登记的收尾跑了，起过的子进程停掉了
    assert!(fsx::read_text(dir.join("noted")).ends_with("hang: ready\nhang: defer\n"));
    wait_for("模块的进程和它起的子进程都退出", WAIT, || !alive(module) && !alive(sleeper));
    // 结果照样写下来：被打断的用例算失败，没轮到的算没跑；临时目录收掉了
    assert_eq!(shape(&fsx::read_text(dir.join("results"))), "fx_hang\tfail\t0\t1\t0\tT\n");
    assert_eq!(
        shape(&fsx::read_text(dir.join("logs/fx_hang.log"))),
        "FAIL waits_to_be_interrupted T\n    收到信号 15：整个模块停下\nNOTRUN after T\n"
    );
    assert_eq!(left_in(&dir.join("tmp")), Vec::<String>::new());
}

fn runner_modules_clean_up_when_the_suite_is_killed(t: &mut Ctx) {
    let dir = t.tmp("kit-kill-");
    let (mut suite, module, sleeper) = hanging_suite(&dir);
    suite.kill().unwrap();
    assert_eq!(suite.wait().unwrap().signal, Some(libc::SIGKILL));
    // suite 什么都来不及做；模块的进程发现上一级没了，自己收场
    wait_for("模块的进程自己收场", WAIT, || {
        fsx::read_text(dir.join("noted")).ends_with("hang: defer\n") && !alive(module) && !alive(sleeper)
    });
}

fn runner_sweeps_leftover_processes(t: &mut Ctx) {
    // 用例通过了，却留下了进程：登记着没人收的在模块收尾时停掉；没登记的后台进程，上一级照它命令行里的临时目录找到、
    // 停掉、记一行 LEFTOVER（模块照样算通过）
    let ran = fixtures(t, &["--module", "fx_leak"], &[]);
    assert_eq!((ran.code, ran.results.as_str()), (0, "fx_leak\tpass\t1\t0\t0\tT\n"), "{}\n{}", ran.out, ran.err);
    let forgotten: i32 = ran.noted_value("forgotten ").parse().unwrap();
    let stray: i32 = ran.noted_value("stray ").parse().unwrap();
    wait_for("留下的进程都退出", WAIT, || !alive(forgotten) && !alive(stray));
    let log = ran.log("fx_leak");
    let line = log.lines().nth(1).unwrap_or_default();
    assert!(log.starts_with("ok leaves_processes_behind T\n") && log.lines().count() == 2, "{log}");
    assert!(line.starts_with(&format!("LEFTOVER {stray} ")) && line.ends_with("/fake-daemon"), "{log}");
    assert!(ran.out.contains(&format!("\nLEFTOVER fx_leak {stray} ")), "{}", ran.out);
}

fn runner_keeps_temp_dirs_when_asked(t: &mut Ctx) {
    let ran = fixtures(t, &["--module", "fx_pass"], &[("KEEP", "1")]);
    assert_eq!(ran.code, 0, "{}\n{}", ran.out, ran.err);
    // 用例的临时目录、这次运行的目录都留着，最后说一声在哪
    let tmp = PathBuf::from(format!("/{}", ran.noted_value("tmp /")));
    assert!(tmp.join("深/一层/file").is_file(), "{tmp:?}");
    let run_dir = tmp.parent().unwrap().parent().unwrap();
    assert_eq!(left_in(&ran.dir.join("tmp")), [run_dir.file_name().unwrap().to_str().unwrap()]);
    assert!(ran.out.ends_with(&format!("临时目录留着：{}\n", run_dir.display())), "{}", ran.out);
}

fn runner_output_modes(t: &mut Ctx) {
    // -q：通过的用例不一行行报，失败的照报
    let ran = fixtures(t, &["--module", "fx_fail", "-q"], &[]);
    assert!(
        !ran.out.contains("ok fx_fail::")
            && ran.out.contains("FAIL fx_fail::panics T\n    标准输出里的一行\n")
            && ran.out.contains("模块 fx_fail：fail"),
        "{}",
        ran.out
    );
    assert_eq!(ran.results, "fx_fail\tfail\t2\t3\t0\tT\n");
    // 平时：通过的用例说的话不出现
    let ran = fixtures(t, &["--module", "fx_pass"], &[]);
    assert!(!ran.out.contains("通过的用例说的话") && ran.err.is_empty(), "{}\n{}", ran.out, ran.err);
    // --nocapture：用例说的话直接出来；记录里失败的说明是 panic 说的那句
    let ran = fixtures(t, &["--module", "fx_pass", "--module", "fx_fail", "--nocapture"], &[]);
    assert!(
        ran.out.contains("通过的用例说的话\n") && ran.out.contains("标准输出里的一行\n") && ran.out.contains("子进程说的\n"),
        "{}",
        ran.out
    );
    assert!(
        ran.err.contains("标准错误里的一行\n") && ran.err.contains("子进程的错\n") && ran.err.contains("故意失败：42\n"),
        "{}",
        ran.err
    );
    assert_eq!(ran.results, "fx_pass\tpass\t4\t0\t2\tT\nfx_fail\tfail\t2\t3\t0\tT\n");
    let log = ran.log("fx_fail");
    assert!(log.contains("fixtures.rs:") && log.contains("：故意失败：42\n    用例 panic 了（说了什么在上面）\n"), "{log}");
    assert!(!log.contains("标准输出里的一行"), "{log}");
}
