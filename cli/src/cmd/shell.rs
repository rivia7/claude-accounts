//! bash 编排里原来零散内嵌的几小段 Python（`"$PYTHON" -I -c '…'`）：切换以后 bash 改成调这里的子命令。
//! 它们不在原来 PY_SRC 的 `CMDS` 里，名字都以 `sh-` 开头；做的事、输出和退出码照原来那一小段。
//!
//! | 子命令 | 原来在 bash 的哪 |
//! |---|---|
//! | `sh-auth-email` | `claude_email`：`claude auth status --json` 的输出里取登录的邮箱 |
//! | `sh-oauth-drop 文件` | `claude_wipe`：去掉 `.claude.json` 里的 oauthAccount |
//! | `sh-flock 描述符` | `chrome_launch_lock` / `app_launch_lock`：给继承来的描述符（9 号）上锁，拿不到返回 1 |
//! | `sh-re-escape 文字` | `app_pid`：`re.escape`，拼 pgrep 的模式用 |
//! | `sh-ps-tree 进程号` | `app_process_tree`：stdin 是 `ps -axo pid=,ppid=`，输出这个进程和它的子孙 |
//! | `sh-lsof-under 目录` | `app_profile_in_use`：stdin 是 `lsof -Fn`，有没有打开着这个目录里的文件 |
//! | `sh-clients-text` | `cmd_clients`：客户端更新状态的 JSON → 给人看的几行 |
//! | `sh-dock-wait 进程号 目录 命令…` | `cmd_dock_wait`（`DOCK_WAIT`）：等那个进程退出，再换成后面的命令 |

use super::{arity, Cmd};
use crate::py::fs;
use crate::py::io;
use crate::py::json;
use crate::py::obj::V;
use crate::py::path;
use crate::py::text;
use crate::py::{exit, Fail, R};
use crate::{out, out_raw};
use std::os::unix::io::AsRawFd;

/// 这个模块的子命令（都是新加的，不在原来的 `CMDS` 里）
pub const CMDS: &[Cmd] = &[
    Cmd::all("sh-auth-email", cmd_auth_email),
    Cmd::all("sh-oauth-drop", cmd_oauth_drop),
    Cmd::all("sh-flock", cmd_flock),
    Cmd::all("sh-re-escape", cmd_re_escape),
    Cmd::all("sh-ps-tree", cmd_ps_tree),
    Cmd::all("sh-lsof-under", cmd_lsof_under),
    Cmd::all("sh-clients-text", cmd_clients_text),
    Cmd::all("sh-dock-wait", cmd_dock_wait),
];

/// `d.get(k)` 当文字用（`"…" + d.get(k, "")`）：不是字符串在原来是 TypeError
fn text_of(v: Option<V>, what: &str) -> R<String> {
    match v {
        Some(V::Str(s)) => Ok(s.to_string()),
        other => Err(Fail::Other(
            "TypeError",
            vec![format!("can only concatenate str (not \"{}\") to str：{what}", other.map_or("NoneType".into(), |v| v.py_repr()))],
        )),
    }
}

/// `claude auth status --json` 的输出（stdin）：登录着就打印邮箱，否则打印空行。读不懂当作没登录
pub fn cmd_auth_email(args: &[String]) -> R<()> {
    arity("cmd_auth_email", args, 0, 0)?;
    let d = json::loads_file_bytes(&io::read_stdin_bytes()).unwrap_or_else(|_| V::Dict(Default::default()));
    let Some(d) = d.as_dict() else {
        return Err(Fail::Other("AttributeError", vec![format!("'{}' object has no attribute 'get'", d.py_repr())]));
    };
    let email = if d.truthy("loggedIn") {
        d.get("email").filter(|e| e.truthy()).map_or(String::new(), |e| e.py_str())
    } else {
        String::new()
    };
    out!("{email}");
    Ok(())
}

/// 去掉这个 JSON 文件里的 oauthAccount（Claude Code 自己记着的登录身份），照 `json.dump(j, f, indent=2)` 写回去
/// （原地写、不换行结尾，和原来一样；读不了、不是 JSON 是出错退出，bash 那边不看）
pub fn cmd_oauth_drop(args: &[String]) -> R<()> {
    arity("cmd_oauth_drop", args, 1, 1)?;
    let p = args[0].as_str();
    let raw = std::fs::read(p)?;
    let j = json::loads_file_bytes(&raw)?;
    match &j {
        V::Dict(d) => {
            d.pop("oauthAccount");
        }
        V::List(_) => return Err(Fail::Other("TypeError", vec!["pop expected at most 1 argument, got 2".into()])),
        other => return Err(Fail::Other("AttributeError", vec![format!("'{}' object has no attribute 'pop'", other.py_repr())])),
    }
    std::fs::write(p, json::dumps(&j, json::Fmt::new().indent(2)))?;
    Ok(())
}

/// 给继承来的描述符上独占锁（不等）：拿到了返回 0，别人拿着（或者描述符不对）返回 1。锁跟着那个打开的文件走，
/// 这个进程退出以后 bash 手里的描述符还拿着它
pub fn cmd_flock(args: &[String]) -> R<()> {
    arity("cmd_flock", args, 1, 1)?;
    let fd = text::to_int(&args[0])
        .ok_or_else(|| Fail::Value(format!("invalid literal for int() with base 10: {}", crate::py::obj::repr_str(&args[0]))))?;
    // SAFETY: 只是对一个描述符号做 flock，号不对会返回错误
    if unsafe { libc::flock(fd as i32, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return exit(1);
    }
    Ok(())
}

/// `re.escape(s)`（Python 3.7 起）：这些字符前面加反斜杠，别的原样
pub fn re_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if "()[]{}?*+-|^$\\.&~# \t\n\r\x0b\x0c".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

pub fn cmd_re_escape(args: &[String]) -> R<()> {
    arity("cmd_re_escape", args, 1, 1)?;
    out!("{}", re_escape(&args[0]));
    Ok(())
}

/// stdin 是 `ps -axo pid=,ppid=`：输出这个进程和它现在的子孙的进程号（从小到大，逗号隔开）
pub fn cmd_ps_tree(args: &[String]) -> R<()> {
    arity("cmd_ps_tree", args, 1, 1)?;
    let int = |s: &str| {
        text::to_int(s)
            .ok_or_else(|| Fail::Value(format!("invalid literal for int() with base 10: {}", crate::py::obj::repr_str(s))))
    };
    let root = int(&args[0])?;
    let mut rows: Vec<(i64, i64)> = Vec::new();
    let input = io::read_stdin_text();
    for line in input.split('\n') {
        let f = text::split_ws(line);
        if f.len() == 2 {
            rows.push((int(f[0])?, int(f[1])?));
        }
    }
    let mut pids = vec![root];
    loop {
        let found: Vec<i64> =
            rows.iter().filter(|(pid, parent)| pids.contains(parent) && !pids.contains(pid)).map(|(pid, _)| *pid).collect();
        if found.is_empty() {
            break;
        }
        for pid in found {
            if !pids.contains(&pid) {
                pids.push(pid);
            }
        }
    }
    pids.sort_unstable();
    out!("{}", pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(","));
    Ok(())
}

/// stdin 是 `lsof -Fn` 的输出：有哪一行是 `n目录/…`（打开着这个目录里的文件）返回 0，没有返回 1
pub fn cmd_lsof_under(args: &[String]) -> R<()> {
    arity("cmd_lsof_under", args, 1, 1)?;
    let prefix = format!("n{}/", args[0]);
    if io::read_stdin_text().split('\n').any(|line| line.starts_with(&prefix)) {
        Ok(())
    } else {
        exit(1)
    }
}

/// 客户端更新状态的 JSON（stdin，`client-updates status` 的输出）→ 给人看的几行
pub fn cmd_clients_text(args: &[String]) -> R<()> {
    arity("cmd_clients_text", args, 0, 0)?;
    let d = json::loads_file_bytes(&io::read_stdin_bytes())?;
    let Some(d) = d.as_dict() else {
        return Err(Fail::Other("AttributeError", vec![format!("'{}' object has no attribute 'get'", d.py_repr())]));
    };
    let auto = d.get("mode").is_some_and(|m| m.as_str() == Some("auto"));
    out!(
        "客户端自动检查：{}；允许自动更新：{}",
        if auto { "开启" } else { "关闭" },
        if d.truthy("auto_install") {
            "开启（查到新版本就装，客户端开着时等它退出）"
        } else {
            "关闭（安装前先问你）"
        }
    );
    let clients = match d.get("clients") {
        None => Vec::new(),
        Some(V::List(l)) => l.items(),
        Some(other) => return Err(Fail::Other("TypeError", vec![format!("clients 不是列表：{}", other.py_repr())])),
    };
    for c in clients {
        let Some(c) = c.as_dict() else {
            return Err(Fail::Other("AttributeError", vec![format!("'{}' object has no attribute 'get'", c.py_repr())]));
        };
        if c.get("status").is_some_and(|s| s.as_str() == Some("missing")) {
            continue;
        }
        let label = text_of(c.get("label").or_else(|| Some(c.get("id").unwrap_or_else(|| V::from("")))), "label")?;
        let message = text_of(c.get("message").or_else(|| Some(c.get("status").unwrap_or_else(|| V::from("")))), "message")?;
        let install = if c.truthy("can_install") && c.truthy("install_token") {
            format!("\n安装：cca clients install {}", text_of(c.get("install_token"), "install_token")?)
        } else {
            String::new()
        };
        out!("{label}：{message}{install}");
    }
    Ok(())
}

/// 等一个进程退出（已经没了就不等）：macOS 用 kqueue 盯着它；别的系统隔一会儿看一眼
fn wait_for_exit(pid: i64) {
    #[cfg(target_os = "macos")]
    // SAFETY: kq 是刚建的；kevent 的两个结构体都是有效的内存；出错（进程已经没了）就不等
    unsafe {
        let kq = libc::kqueue();
        if kq < 0 {
            return;
        }
        let mut change: libc::kevent = std::mem::zeroed();
        change.ident = pid as usize;
        change.filter = libc::EVFILT_PROC;
        change.flags = libc::EV_ADD | libc::EV_ONESHOT;
        change.fflags = libc::NOTE_EXIT;
        if libc::kevent(kq, &change, 1, std::ptr::null_mut(), 0, std::ptr::null()) == 0 {
            let mut event: libc::kevent = std::mem::zeroed();
            while libc::kevent(kq, std::ptr::null(), 0, &mut event, 1, std::ptr::null()) < 0
                && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
            {}
        }
        libc::close(kq);
    }
    #[cfg(not(target_os = "macos"))]
    // SAFETY: 发 0 号信号只是问进程在不在
    while unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
        crate::py::time::sleep(0.5);
    }
}

/// cca _dock-wait：等进程 pid 退出，再换成后面的命令（`cca _dock-tidy App`）。目录 d 里放一个 `进程号.wait` 当锁：同一个
/// 进程已经有人在等就走；等完把它删掉。原来是 bash 里的 DOCK_WAIT
pub fn cmd_dock_wait(args: &[String]) -> R<()> {
    if args.len() < 3 {
        return Err(Fail::Other("IndexError", vec!["list index out of range".into()]));
    }
    let pid = text::to_int(&args[0])
        .ok_or_else(|| Fail::Value(format!("invalid literal for int() with base 10: {}", crate::py::obj::repr_str(&args[0]))))?;
    let (d, argv) = (args[1].as_str(), &args[2..]);
    let mut lock: Option<(std::fs::File, String)> = None;
    if !d.is_empty() {
        let name = path::join(d, &format!("{pid}.wait"));
        if let Ok(f) = fs::makedirs_mode(d, 0o700).and_then(|()| std::fs::OpenOptions::new().append(true).create(true).open(&name))
        {
            // SAFETY: f 开着，fd 有效
            if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock {
                    return Ok(()); // 这个进程已经有人在等
                }
            } else {
                lock = Some((f, name));
            }
        }
    }
    wait_for_exit(pid);
    if let Some((_, name)) = &lock {
        let _ = std::fs::remove_file(name);
    }
    io::flush_out();
    // os.execv：换成后面的命令；锁文件的描述符带过去（和原来一样，命令跑完才放）
    if let Some((f, _)) = &lock {
        // SAFETY: f 开着；去掉 close-on-exec，让锁跟着换上去的命令
        unsafe { libc::fcntl(f.as_raw_fd(), libc::F_SETFD, 0) };
    }
    let argv: Vec<&str> = argv.iter().map(|a| a.as_str()).collect();
    let _ = crate::py::proc::execv(argv[0], &argv);
    out_raw!("");
    Ok(()) // 换不上去（命令不在了）就算了
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真的等到进程退出才往下走；已经没了的不等
    #[test]
    fn waits_until_the_process_exits() {
        let mut child = std::process::Command::new("/bin/sleep").arg("0.4").spawn().unwrap();
        let t0 = std::time::Instant::now();
        wait_for_exit(child.id() as i64);
        let waited = t0.elapsed().as_secs_f64();
        assert!((0.3..5.0).contains(&waited), "等了 {waited} 秒");
        child.wait().unwrap();
        let t0 = std::time::Instant::now();
        wait_for_exit(child.id() as i64);
        assert!(t0.elapsed().as_secs_f64() < 0.3);
    }

    #[test]
    fn re_escape_like_python() {
        // 期望值是 CPython 3.14 的 re.escape
        assert_eq!(re_escape("/Users/me/.cca/accounts/a-b_c (1)/desktop"), r"/Users/me/\.cca/accounts/a\-b_c\ \(1\)/desktop");
        assert_eq!(
            re_escape("a.b*c+d?e|f^g$h[i]j{k}l\\m&n~o#p\tq\nr"),
            "a\\.b\\*c\\+d\\?e\\|f\\^g\\$h\\[i\\]j\\{k\\}l\\\\m\\&n\\~o\\#p\\\tq\\\nr"
        );
        assert_eq!(re_escape("中文 \"'<>,;:=!@%_`/"), "中文\\ \"'<>,;:=!@%_`/");
        assert_eq!(re_escape(""), "");
    }
}
