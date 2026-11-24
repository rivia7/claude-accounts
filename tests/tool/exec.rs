//! exec：换会话、接上终端、换目录以后 exec 一个程序（测试起子进程时经它：测试程序是多线程的，不能自己 fork 了再做这些）
//!
//! 用法：cca-test exec [--setsid] [--ctty] [--tty-after-setsid 路径] [--cwd 目录] [--arg0 名字] -- 程序 参数…
//!
//! 照写的先后做：setsid；打开那个终端、接成 0/1/2；把 0 号认作控制终端；换目录；exec。进程号一直是同一个：
//! 起它的那一头等到的退出码、用掉的 CPU 时间都是那个程序自己的。哪一步不成就说一行、退出码 127，那个程序没有运行。
//!
//! --tty-after-setsid 照的是界面 App 起终端的办法（posix_spawn 开新会话、用它的 file action 把伪终端从的那头打开接成
//! 0/1/2）起出来的样子：Linux 上那个终端就成了控制终端，macOS 上没有（那里的 file action 是系统替它打开的，不算它自己
//! 打开）。这里是程序自己打开的——macOS 上自己打开终端会把它认成控制终端（量过），所以 macOS 上带着 O_NOCTTY 打开，
//! 出来的样子才和 App 起的一样。--ctty 是另一回事：伪终端从的那头已经是 0 号时，像 login_tty 那样把它认作控制终端。
//!
//! fds：列出这个进程开着的文件描述符（一行，空格隔开）。测试拿它看子进程到底带着哪几个。

use std::ffi::CString;

pub const CMDS: &[(&str, crate::Run)] = &[("exec", exec), ("fds", fds)];

const USAGE: &str = "用法：cca-test exec [--setsid] [--ctty] [--tty-after-setsid 路径] [--cwd 目录] [--arg0 名字] -- 程序 参数…";

/// 说一行为什么没起来；退出码 127
fn fail(what: &str) -> i32 {
    eprintln!("cca-test exec：{what}：{}", std::io::Error::last_os_error());
    127
}

fn cstring(s: &str) -> Option<CString> {
    CString::new(s).ok()
}

fn exec(args: &[String]) -> i32 {
    let (mut setsid, mut ctty) = (false, false);
    let (mut tty, mut cwd, mut arg0) = (None, None, None);
    let mut it = args.iter();
    loop {
        match it.next().map(String::as_str) {
            Some("--") => break,
            Some("--setsid") => setsid = true,
            Some("--ctty") => ctty = true,
            Some(opt @ ("--tty-after-setsid" | "--cwd" | "--arg0")) => {
                let Some(value) = it.next() else {
                    eprintln!("cca-test exec：{opt} 后面要跟一个值\n{USAGE}");
                    return 2;
                };
                match opt {
                    "--tty-after-setsid" => tty = Some(value),
                    "--cwd" => cwd = Some(value),
                    _ => arg0 = Some(value),
                }
            }
            other => {
                eprintln!(
                    "cca-test exec：{}\n{USAGE}",
                    other.map_or("少了 -- 和要运行的程序".to_string(), |o| format!("不认识 {o}"))
                );
                return 2;
            }
        }
    }
    let rest: Vec<&String> = it.collect();
    let Some(prog) = rest.first() else {
        eprintln!("cca-test exec：-- 后面要写运行哪个程序\n{USAGE}");
        return 2;
    };
    // 字符串里带 0 字节的交不给系统
    let c = |s: &str| cstring(s).ok_or_else(|| eprintln!("cca-test exec：参数里有 0 字节"));
    let Ok(prog_c) = c(prog) else { return 2 };
    let mut argv_c = Vec::new();
    for (i, a) in rest.iter().enumerate() {
        let Ok(a) = c(if i == 0 { arg0.unwrap_or(a) } else { a }) else { return 2 };
        argv_c.push(a);
    }
    let mut argv: Vec<*const libc::c_char> = argv_c.iter().map(|a| a.as_ptr()).collect();
    argv.push(std::ptr::null());
    let (Ok(tty_c), Ok(cwd_c)) = (tty.map(|t| c(t)).transpose(), cwd.map(|d| c(d)).transpose()) else { return 2 };

    // SAFETY: 这个进程只有一条线程；下面都是普通的系统调用，交进去的字符串以 0 结尾、在 exec 之前一直有效
    unsafe {
        if setsid && libc::setsid() < 0 {
            return fail("换不了会话（setsid）");
        }
        if let Some(path) = &tty_c {
            // Linux 上不带 O_NOCTTY（它就这样成了控制终端），macOS 上带（见上面的说明）
            let noctty = if cfg!(target_os = "macos") { libc::O_NOCTTY } else { 0 };
            let fd = libc::open(path.as_ptr(), libc::O_RDWR | noctty);
            if fd < 0 {
                return fail(&format!("打不开终端 {}", tty.unwrap()));
            }
            for target in 0..3 {
                if fd != target && libc::dup2(fd, target) < 0 {
                    return fail("终端接不到标准输入输出上（dup2）");
                }
            }
            if fd > 2 {
                libc::close(fd);
            }
        }
        if ctty && libc::ioctl(0, libc::TIOCSCTTY as _, 0) != 0 {
            return fail("标准输入认不成控制终端（TIOCSCTTY）");
        }
        if let Some(dir) = &cwd_c {
            if libc::chdir(dir.as_ptr()) != 0 {
                return fail(&format!("进不了目录 {}", cwd.unwrap()));
            }
        }
        // Rust 的程序一启动就把 SIGPIPE 设成不理：这个设置会留给 exec 出来的程序，改回默认的
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        libc::execvp(prog_c.as_ptr(), argv.as_ptr());
        fail(&format!("运行不了 {prog}"))
    }
}

/// 这个进程开着的文件描述符：挨个问，不为了列它们再打开什么（ls /dev/fd 会把它自己打开的目录也列进去）
fn fds(_args: &[String]) -> i32 {
    // SAFETY: getrlimit 往 lim 里写；fcntl(F_GETFD) 只是问这个号开着没有
    let open: Vec<String> = unsafe {
        let mut lim: libc::rlimit = std::mem::zeroed();
        let max = if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) == 0 { lim.rlim_cur.min(65536) as i32 } else { 1024 };
        (0..max).filter(|fd| libc::fcntl(*fd, libc::F_GETFD) >= 0).map(|fd| fd.to_string()).collect()
    };
    println!("{}", open.join(" "));
    0
}
