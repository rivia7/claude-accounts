//! 开关和列表：`remote-set on|off`（config.json 的 `remote`）、`remote-list`（这台机器上在宿主里运行的会话，
//! 和配对过的机器上可以接入的会话）。

use super::local::{self, Note, Status};
use super::{far, peers, relay};
use crate::cmd::arity;
use crate::cmd::settings::home_tilde;
use crate::core::base::{Lock, RO};
use crate::core::ctx::cfg_save;
use crate::core::model::cfg_load;
use crate::dict;
use crate::py::json;
use crate::py::obj::{Dict, List, V};
use crate::py::time::{strftime_local, time};
use crate::py::{die, exit, R};
use std::io::{self, Read, Write};
use std::time::Duration;

/// config.json 的 `remote` 是 true：之后启动的会话放进宿主里运行（`py env` 的 CFG_REMOTE 告诉 bash）
pub fn remote_on(c: &Dict) -> bool {
    c.get("remote").is_some_and(|v| v.as_bool() == Some(true))
}

/// 这次启动的会话会放进宿主里运行（bash `remote_prepare` 的头一个条件）：`CCA_REMOTE` 不空时照它（1 是，别的不是），
/// 空着照 config.json 的开关。cca 自己带的钩子（hook.rs）照它加不加；在不在终端里这里看不到，不在的话钩子什么都不做
pub fn hooks_on() -> bool {
    match crate::py::proc::getenv("CCA_REMOTE").filter(|v| !v.is_empty()) {
        Some(v) => v == "1",
        None => crate::py::fs::load_quiet(&crate::core::base::CFG).and_then(|c| c.as_dict()).is_some_and(|c| remote_on(&c)),
    }
}

/// `remote-set on|off`：off 时把这一项删掉（不留一个 false）。读了再写整份 config.json：拿着锁
pub fn cmd_remote_set(args: &[String]) -> R<()> {
    arity("cmd_remote_set", args, 1, 1)?;
    let _lock = Lock::acquire()?;
    let c = cfg_load()?;
    match args[0].as_str() {
        "on" => c.set("remote", true),
        "off" => {
            c.pop("remote");
        }
        _ => return die("用法：remote-set on|off"),
    }
    cfg_save(&c)
}

/// 列表里的一行：编号的前 8 位、工具、账号、文件夹、几点开始的、现在谁接着；会话走哪个 Team 的服务器中转时最后带上它的名字
pub fn row(s: &Status) -> String {
    let team = relay::session_hub(&s.meta.tool, &s.meta.key).map(|k| format!("  「{}」", relay::team_name(k))).unwrap_or_default();
    let cwd = home_tilde(&s.meta.cwd);
    let who = match (s.local, s.viewers) {
        _ if s.busy => "没有回应（可能被挂起了、正忙）".to_string(),
        (true, 0) => "启动它的终端开着".to_string(),
        (true, n) => format!("启动它的终端开着，另有 {n} 处接入"),
        (false, 0) => "没有终端接着".to_string(),
        (false, n) => format!("{n} 处接入"),
    };
    format!(
        "{}  {}  {}  {}  {} 开始  {}{}",
        &s.meta.sid[..8],
        s.meta.tool,
        s.meta.name,
        cwd,
        strftime_local("%m-%d %H:%M", s.meta.since),
        who,
        team
    )
}

/// `remote-end [--show] [会话编号的前几位]`：结束这台机器上的一个会话，等于关掉它的终端——里面的程序收到 SIGHUP，
/// `END_GRACE` 秒还不退出就被强行结束（宿主做的；这里只是告诉它，等它收场）。只管这台机器上的：别的机器上的会话
/// 要到那台机器上结束（接进去让里面的程序自己退出也行）。只有一个会话时不用写编号。
/// `--show`：只找出是哪一个，打印“编号<TAB>列表里的那一行”（bash 拿去问用户要不要结束）。
/// 退出码照 `remote-attach` 的：64 用法不对或者前缀对上了好几个，65 没有这个会话；1 说了以后它没结束
pub fn cmd_remote_end(args: &[String]) -> R<()> {
    let (mut show, mut prefix) = (false, None);
    for a in args {
        match a.as_str() {
            "--show" if !show => show = true,
            p if prefix.is_none() && !p.is_empty() && p.bytes().all(|b| b.is_ascii_hexdigit()) => {
                prefix = Some(p.to_ascii_lowercase())
            }
            _ => {
                crate::err!("用法：cca remote end [会话编号的前几位]");
                return exit(64);
            }
        }
    }
    let all = local::sessions(!*RO);
    let hit: Vec<&Status> = all.iter().filter(|s| prefix.as_deref().is_none_or(|p| s.meta.sid.starts_with(p))).collect();
    let one = match (hit.as_slice(), &prefix) {
        ([one], _) => *one,
        ([], None) => {
            crate::err!("这台机器上现在没有在会话宿主里运行的会话");
            return exit(65);
        }
        ([], Some(p)) => {
            crate::err!(
                "这台机器上没有编号以 {p} 开头的会话（cca remote list --local 看有哪些；别的机器上的会话要到那台机器上结束）"
            );
            return exit(65);
        }
        (many, _) => {
            crate::err!("有 {} 个会话，说一下结束哪个：cca remote end 编号", many.len());
            for s in many {
                crate::err!("  {}", row(s));
            }
            return exit(64);
        }
    };
    if show {
        crate::out!("{}\t{}", one.meta.sid, row(one));
        return Ok(());
    }
    let (dir, sock) = (local::dir(), format!("{}.sock", local::short(&one.meta.sid)));
    let gone = || local::ask_status(&dir, &sock) == local::Asked::Gone;
    let Ok(mut s) = local::connect(&dir, &sock) else {
        return if gone() { Ok(()) } else { die("连不上这个会话的宿主") }; // 刚好自己结束了也算结束了
    };
    // 说完等宿主收场：它退出时这条连接就关了。里面的程序不理 SIGHUP 的话宿主过 END_GRACE 秒强行结束它，再留几秒收场
    let wait = Duration::from_secs_f64(local::END_GRACE + 5.0);
    let told = s.set_write_timeout(Some(Duration::from_secs(2))).and_then(|()| s.write_all(&local::frame(local::END, b"")));
    if told.is_ok() && s.set_read_timeout(Some(Duration::from_millis(200))).is_ok() {
        let (start, mut buf) = (std::time::Instant::now(), [0u8; 4096]);
        while start.elapsed() < wait {
            match s.read(&mut buf) {
                Ok(0) => break, // 连接关了：宿主收场了，也可能是它没收这条连接（接着的已经够多了）
                Ok(_) => {}
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => {}
                Err(_) => break, // 连接被重置
            }
        }
    }
    // 连接关了不等于会话结束了：看宿主的 socket 还有没有人听（它收场时先删 socket，再关各条连接）
    let checked = std::time::Instant::now();
    loop {
        if gone() {
            return Ok(());
        }
        if checked.elapsed() >= Duration::from_secs(2) {
            break;
        }
        crate::py::time::sleep(0.1);
    }
    die("这个会话还没结束：宿主没有回应（被挂起了、机器正忙、接着的太多），或者里面的程序结束不掉（cca remote list --local 看它还在不在）")
}

/// 一个进程的父进程（macOS：proc_pidinfo；Linux：/proc/进程号/stat 的第 4 栏）；问不到是 None
fn parent_of(pid: i64) -> Option<i64> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: info 是够大的一块；proc_pidinfo 只往里写 size 个字节
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        let got = unsafe {
            libc::proc_pidinfo(pid as i32, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size)
        };
        (got == size).then_some(i64::from(info.pbi_ppid))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // 第 2 栏是括号里的命令名（里面可能有空格、括号）：从最后一个右括号往后数
        stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
    }
}

/// `remote-notify [--title 标题] [--subtitle 副标题] [--] 正文…`：从会话里（Claude Code 的钩子）推一条通知给接着这个会话的
/// 别的机器——那边的终端、cca App 弹出来。这个进程往上找到它所在的会话宿主（会话说明里的进程号），在它的 socket 上说一声；
/// 不在哪个会话宿主里（远程会话没开、不是经 cca 启动的）退出码 65，什么都不说（钩子每次都调它，不该在终端上留话）。
/// 推不推得到看那边：只有接着的别的机器收得到，没人接着就没有；这台机器上的不给（钩子自己会弹）
pub fn cmd_remote_notify(args: &[String]) -> R<()> {
    let (mut title, mut subtitle, mut body) = (String::new(), String::new(), Vec::new());
    let mut it = args.iter();
    let mut rest = false;
    while let Some(a) = it.next() {
        match a.as_str() {
            "--title" if !rest => title = it.next().cloned().unwrap_or_default(),
            "--subtitle" if !rest => subtitle = it.next().cloned().unwrap_or_default(),
            "--" if !rest => rest = true,
            _ => body.push(a.clone()),
        }
    }
    let note = Note::new(title, subtitle, body.join(" "));
    let Some(note) = Note::parse(&note.to_json()) else {
        crate::err!("用法：cca remote notify [--title 标题] [--subtitle 副标题] 正文");
        return exit(64);
    };
    let Some(sid) = host_of_me() else { return exit(65) };
    match notify_host(&sid, &note) {
        Ok(()) => Ok(()),
        Err(code) => exit(code),
    }
}

/// 这个进程在哪个会话宿主里：往上一层层找父进程，是哪个会话的宿主（会话说明里的进程号）就是哪个会话的编号；
/// 不在哪个会话宿主里（远程会话没开、不是经 cca 启动的）是 None
pub fn host_of_me() -> Option<String> {
    let hosts: std::collections::HashMap<i64, String> =
        local::sessions(false).into_iter().map(|s| (s.meta.pid, s.meta.sid)).collect();
    let mut pid = i64::from(std::process::id());
    for _ in 0..64 {
        match parent_of(pid) {
            Some(p) if p > 1 => pid = p,
            _ => return None,
        }
        if let Some(s) = hosts.get(&pid) {
            return Some(s.clone());
        }
    }
    None
}

/// 在会话宿主的 socket 上说一声：推一条通知（`NOTIFY_REQ`）。连不上（宿主刚走）是 Err(65)，写不出去是 Err(1)
pub fn notify_host(sid: &str, note: &Note) -> Result<(), i32> {
    let Ok(mut s) = local::connect(&local::dir(), &format!("{}.sock", local::short(sid))) else { return Err(65) };
    let _ = s.set_write_timeout(Some(Duration::from_secs(1)));
    if s.write_all(&local::frame(local::NOTIFY_REQ, &note.to_json())).is_err() {
        return Err(1);
    }
    // 等宿主读完再关（它读到这一帧、再读到头）：不然连接一关，没读的就丢了
    let _ = s.shutdown(std::net::Shutdown::Write);
    let _ = s.set_read_timeout(Some(Duration::from_secs(1)));
    let _ = s.read(&mut [0u8; 64]);
    Ok(())
}

/// 没问到服务器的原因归成这几种（给界面 App：PROTOCOL 9.1 的 `errors[].code`）
fn relay_code(f: &relay::Fault) -> &'static str {
    match (f.code, f.error.as_str()) {
        (404, "not_found") => "relay_old",
        (0, _) => "relay_unreachable",
        (403, _) => "relay_denied",
        _ => "relay_error",
    }
}

/// `remote-list --json` 的那一份（PROTOCOL 9.1）：开关、中转用不用得了、这台机器上和配对过的机器上的会话、配对过的机器。
/// 没问到服务器不算失败：原因在 `errors` 里，这台机器上的照给
fn list_doc(near: bool, distant: bool) -> R<Dict> {
    let sessions = List::new();
    if near {
        for s in local::sessions(!*RO) {
            // team：这个会话走哪个 Team 的服务器中转（账号是哪个 Team 给的；别的走这台机器的家），没有中转时是 null
            let team = relay::session_hub(&s.meta.tool, &s.meta.key).map_or(V::None, |k| V::from(k.team()));
            sessions.push(dict! {
                "id" => s.meta.sid.as_str(), "short" => &s.meta.sid[..8], "where" => "local", "peer" => V::None, "machine" => V::None,
                "tool" => s.meta.tool.as_str(), "name" => s.meta.name.as_str(), "cwd" => home_tilde(&s.meta.cwd),
                "since_epoch" => s.meta.since, "viewers" => s.viewers, "local_terminal" => s.local,
                "state" => if s.busy { "busy" } else { "ok" }, "access" => "control", "team" => team
            });
        }
    }
    let all = peers::load();
    let errors = List::new();
    let mut seen = None;
    if distant && !all.is_empty() {
        // 配对记在哪台服务器上就问哪台；哪一台没问到，只是它上面的那几个配对这次不知道
        let got = far::fetch_all(&all, 8)?;
        for f in &got.sessions {
            // 别的机器上的会话是经哪个配对看到的，就是哪个 Team 的服务器上的
            let team = all.iter().find(|p| p.pid == f.pid).map_or(V::None, |p| V::from(p.hub.team()));
            sessions.push(dict! {
                "id" => f.sid.as_str(), "short" => &f.sid[..8], "where" => "peer", "peer" => f.pid.as_str(),
                "machine" => f.peer.as_str(), "tool" => f.tool.as_str(), "name" => f.name.as_str(), "host_name" => f.host_name.as_str(),
                "cwd" => f.cwd.as_str(),
                "since_epoch" => if f.since > 0.0 { V::from(f.since) } else { V::None }, "viewers" => f.viewers,
                "local_terminal" => V::None,
                "state" => if f.online { "ok" } else { "offline" }, "access" => if f.view_only { "view" } else { "control" },
                "team" => team
            });
        }
        for (_, why, asked) in &got.errors {
            errors.push(dict! { "code" => asked.as_ref().map_or("relay_off", relay_code), "message" => why.as_str() });
        }
        seen = Some(got);
    }
    let paired = List::new();
    for p in &all {
        let state = match &seen {
            Some(s) if !s.asked.contains(&p.pid) => "unknown",
            Some(s) if s.known.contains(&p.pid) => "ok",
            Some(_) => "gone",
            None => "unknown",
        };
        // team：这个配对记在哪个 Team 的服务器上（经它看得到的是这个 Team 的账号的会话）。3.6.0 起总是 Team 的编号
        // （维护者那一台上的旧配对读配对表时就不算了）
        paired.push(dict! {
            "id" => p.pid.as_str(), "short" => &p.pid[..8], "name" => p.name.as_str(), "incoming" => p.incoming.word(),
            "shown" => p.shown, "paired_epoch" => p.at, "state" => state, "web" => p.web,
            "team" => p.hub.team()
        });
    }
    // 用得了哪几台中转服务器（配对时要选在哪一台上）：各个 Team 的。一台都用不了时 message 是原因（不在哪个 Team 里，或者
    // 加入的第一个 Team 的服务器为什么用不了）
    let hubs = List::new();
    for s in relay::servers()? {
        hubs.push(dict! { "team" => s.kind.team(), "name" => relay::label(s.kind) });
    }
    let relay = if !hubs.is_empty() {
        dict! { "available" => true, "message" => V::None, "hubs" => hubs }
    } else {
        dict! { "available" => false, "message" => relay::no_home_why(), "hubs" => hubs }
    };
    Ok(dict! {
        "version" => 1, "kind" => "remote", "generated_epoch" => time(), "enabled" => remote_on(&cfg_load()?), "relay" => relay,
        "sessions" => sessions, "peers" => paired, "errors" => errors
    })
}

/// `remote-list [--local|--far] [--json]`：一行一个会话；一个都没有时什么都不打印。先是这台机器上的（顺手清掉宿主已经不在了的
/// 会话留下的文件，只读时不清），再是配对过的机器上的（要问服务器；没配对过就不问，没问到的原因写到 stderr）。
/// `--local` 只要这台机器上的（不联网），`--far` 只要别的机器上的。`--json`：给界面 App 的一份文档（`list_doc`）
pub fn cmd_remote_list(args: &[String]) -> R<()> {
    let (mut near, mut distant, mut as_json) = (true, true, false);
    for a in args {
        match a.as_str() {
            "--local" if near && distant => distant = false,
            "--far" if near && distant => near = false,
            "--json" if !as_json => as_json = true,
            _ => return die("用法：remote-list [--local|--far] [--json]"),
        }
    }
    if as_json {
        crate::out!("{}", json::dumps(&V::Dict(list_doc(near, distant)?), json::LINE));
        return Ok(());
    }
    if near {
        for s in local::sessions(!*RO) {
            crate::out!("{}", row(&s));
        }
    }
    let all = if distant { peers::load() } else { Vec::new() };
    if all.is_empty() {
        return Ok(());
    }
    let seen = far::fetch_all(&all, 8)?;
    for f in &seen.sessions {
        let team = all.iter().find(|p| p.pid == f.pid).map(|p| format!("  「{}」", relay::team_name(p.hub))).unwrap_or_default();
        crate::out!("{}{team}", far::row(f));
    }
    for (_, why, asked) in &seen.errors {
        crate::err!("别的机器上的会话{}：{why}", if asked.is_some() { "没查到" } else { "没查" });
    }
    Ok(())
}
