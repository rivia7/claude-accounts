//! web-demo [--port 端口] [--] [会话里跑的命令…]：在本机把远程会话的网页端跑起来看（原来的 `python3 tests/web_demo.py`）。
//! 不联网、不碰真实的 ~/.cca：一个真的服务器（网页端开在 http://localhost）、一台临时的“电脑”（真的 cca 辅助程序，上面一个在
//! 宿主里运行的会话），再替它开一次给网页端的配对。照打印出来的地址在这台 Mac 的浏览器里打开、输入配对码，就能接进那个会话。
//! 手机上试不了这个（手机要 HTTPS 和信得过的证书：见 docs/dev/remote.md「网页端」）。
//!
//! 会话里跑的命令默认是测试用的假客户端（cca-test fake-client：敲 `say 你好` 它回 `<你好>`，`exit 3` 结束）；换别的写在后面，
//! 比如 `-- vim`、`-- bash -l`。Ctrl-C 结束（被 TERM 叫停、终端关了也一样）：服务器、会话都停掉，临时目录删掉。
//!
//! 要先编好辅助程序（`cd cli && cargo build --release`；`CCA_HELPER` 换一份），再
//! `cargo run --profile kit -p cca-tests --bin cca-test -- web-demo`。夹具是 kit::relay 的那一套（和 tests/web_relay.rs 用的一样）。

use kit::fleet::Device;
use kit::fsx::{self, TempDir};
use kit::pty::Pty;
use kit::relay::{self, Base, Popen};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const CMDS: &[(&str, crate::Run)] = &[("web-demo", run)];

const USAGE: &str = "用法：cca-test web-demo [--port 端口] [--] [会话里跑的命令…]";

/// 每隔多久看一眼（配对成了没有、会话还在不在、有没有被叫停）
const TICK: Duration = Duration::from_millis(200);

/// 被叫停了（Ctrl-C、TERM、终端关了）：信号处理函数只记这一笔，主循环看到了就收场
static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_stop(_sig: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

fn stopped() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// 说一行、马上送出去（接着管道看的人也马上看得到）；标准输出没人收了就算了
fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// 用法不对：说在 stderr（后面接着用法），在 $CCA_TEST_TRAP 里记一行，退出码 2
fn usage(args: &[String], why: &str) -> i32 {
    crate::trap::usage_with("web-demo", args, why, &format!("\n{USAGE}"))
}

/// 命令行上写的
struct Args {
    /// 网页端听哪个端口；0：随便挑一个空着的
    port: u16,
    /// 会话里跑的命令；空的：测试用的假客户端
    cmd: Vec<String>,
}

/// 读命令行（照原来 argparse 的那一份：--port 端口 / --port=端口，-- 后面的都算命令）。Ok(None)：要的是用法（-h、--help）
fn parse(args: &[String]) -> Result<Option<Args>, String> {
    let mut parsed = Args { port: 0, cmd: Vec::new() };
    let port = |text: &str| text.parse::<u16>().map_err(|_| format!("--port 后面要跟端口（0 到 65535 的数），不是 {text:?}"));
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--" => {
                parsed.cmd.extend(it.by_ref().cloned());
            }
            "-h" | "--help" => return Ok(None),
            "--port" => match it.next() {
                Some(text) => parsed.port = port(text)?,
                None => return Err("--port 后面要跟端口".to_string()),
            },
            s if s.starts_with("--port=") => parsed.port = port(&s["--port=".len()..])?,
            s if s.starts_with('-') && s.len() > 1 => return Err(format!("不认识 {s}（会话里跑的命令带选项的话，前面加 --）")),
            _ => parsed.cmd.push(a.clone()),
        }
    }
    Ok(Some(parsed))
}

/// 夹具里起服务器、起会话要经 cca-test（换会话、假客户端）：就是自己
fn here() {
    if let Ok(exe) = std::env::current_exe().and_then(std::fs::canonicalize) {
        kit::proc::set_tool(exe);
    }
}

fn run(args: &[String]) -> i32 {
    let parsed = match parse(args) {
        Ok(Some(parsed)) => parsed,
        Ok(None) => {
            say(&format!(
                "{USAGE}\n本机试远程会话的网页端。--port：网页端听哪个端口（默认随便挑一个）；命令默认是测试用的假客户端"
            ));
            return 0;
        }
        Err(why) => return usage(args, &why),
    };
    here();
    // SAFETY: sigaction 清零后填上处理函数就是有效的；处理函数只往一个原子变量里记一笔。不带 SA_RESTART：
    // 等终端输出的那一步（poll）要被打断，好马上看到被叫停了
    unsafe {
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_stop as extern "C" fn(libc::c_int) as libc::sighandler_t;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(sig, &action, std::ptr::null_mut());
        }
    }
    // 夹具里出了岔子是 panic（服务器没起来、端口被占着……）：原因已经说在 stderr 上了，起过的东西在展开的路上都停掉了
    match std::panic::catch_unwind(|| demo(&parsed)) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

fn demo(args: &Args) {
    // 先挑一个空着的端口：页面的地址（--web-url）里要写它
    let port = if args.port == 0 { fsx::free_port() } else { args.port };
    let url = format!("http://localhost:{port}/");
    // 声明的先后就是收场的反过来：先停会话，再停设备、服务器、转发器，最后删临时目录
    let d = TempDir::new("cca-relay-");
    let fx = Base::in_dir(&d, true, |_| {
        vec!["--web-url".to_string(), url.clone(), "--web-listen".to_string(), format!("127.0.0.1:{port}")]
    });
    let dev = fx.device("box");
    fx.machines(&[&dev]);
    let client = relay::client(&dev).to_string_lossy().into_owned();
    let mut argv = vec!["remote-host", "claude", Device::KEY, "alice", "--"];
    if args.cmd.is_empty() {
        argv.push(&client);
    } else {
        argv.extend(args.cmd.iter().map(String::as_str));
    }
    // 会话的终端：没人看它（看的人在浏览器里），只是把它的输出读走、扔掉，不然宿主写满了会卡住
    let mut pty = Pty::open(24, 80).unwrap_or_else(|e| panic!("开不了伪终端：{e}"));
    let mut host = pty.spawn(&mut relay::helper_cmd(&dev, &argv)).unwrap_or_else(|e| panic!("{e}"));
    let drain = |pty: &mut Pty| {
        if !pty.pump(TICK) {
            std::thread::sleep(TICK); // 会话已经结束了：没有输出可等
        }
        pty.out.clear();
    };
    say(&format!("网页端：{url}"));
    while !stopped() {
        let mut p = Popen::spawn(&mut relay::helper_cmd(&dev, &["remote-pair-show", "--web"]));
        let line = p.out_line();
        let opened = line.starts_with("code ");
        if opened {
            say(&format!("配对码：{}", line.split_whitespace().nth(1).unwrap_or_default()));
            p.out_line(); // 手机上打开的地址那一行：上面已经说过了
        }
        let code = loop {
            if stopped() {
                break None;
            }
            match p.poll() {
                Some(code) => break Some(code),
                None => drain(&mut pty),
            }
        };
        let Some(code) = code else { break };
        if !opened {
            say(&format!("没开成配对：{}", p.err_rest()));
            break;
        }
        if code == 0 {
            say(&format!("已配对：{}", p.out_rest().trim()));
            break;
        }
        say(&format!("这个配对码没用上（{}），换一个", p.err_rest().trim()));
    }
    // 会话结束（宿主退出、终端上没有程序了）为止
    while !stopped() && pty.pump(TICK) {
        pty.out.clear();
    }
    if !stopped() {
        say("会话结束了（服务器还开着：页面上看得到“没有可以接入的会话”；Ctrl-C 结束）");
        while !stopped() {
            std::thread::sleep(TICK);
        }
    }
    // 收场：关掉会话的终端（宿主收到 SIGHUP），它还不退出就停掉；剩下的（设备、服务器、转发器、临时目录）离开这里时收
    pty.hangup();
    let _ = host.stop();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(words: &[&str]) -> Result<Option<(u16, Vec<String>)>, String> {
        let args: Vec<String> = words.iter().map(|w| w.to_string()).collect();
        parse(&args).map(|parsed| parsed.map(|a| (a.port, a.cmd)))
    }

    fn cmd(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    /// 命令行照原来 argparse 的那一份读：端口两种写法、写在命令前后都行，-- 后面的原样算命令
    #[test]
    fn command_line() {
        assert_eq!(read(&[]), Ok(Some((0, cmd(&[])))));
        assert_eq!(read(&["--port", "8080", "--", "bash", "-l"]), Ok(Some((8080, cmd(&["bash", "-l"])))));
        assert_eq!(read(&["--port=9", "vim"]), Ok(Some((9, cmd(&["vim"])))));
        assert_eq!(read(&["vim", "--port", "7"]), Ok(Some((7, cmd(&["vim"])))));
        assert_eq!(read(&["--", "--port", "1"]), Ok(Some((0, cmd(&["--port", "1"])))));
        assert_eq!(read(&["-"]), Ok(Some((0, cmd(&["-"])))));
        assert_eq!(read(&["--help"]), Ok(None));
        assert_eq!(read(&["vim", "-h"]), Ok(None));
        for bad in [&["--port"][..], &["--port", "x"], &["--port=70000"], &["--port", "-1"], &["-x"], &["bash", "-l"]] {
            assert!(read(bad).is_err(), "{bad:?}");
        }
    }
}
