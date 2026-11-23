//! 开关和列表：`remote-set on|off`（config.json 的 `remote`）、`remote-list`（这台机器上在宿主里运行的会话，
//! 和配对过的机器上可以接入的会话）。

use super::local::{self, Status};
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

/// 列表里的一行：编号的前 8 位、工具、账号、文件夹、几点开始的、现在谁接着
pub fn row(s: &Status) -> String {
    let cwd = home_tilde(&s.meta.cwd);
    let who = match (s.local, s.viewers) {
        _ if s.busy => "没有回应（可能被挂起了、正忙）".to_string(),
        (true, 0) => "启动它的终端开着".to_string(),
        (true, n) => format!("启动它的终端开着，另有 {n} 处接入"),
        (false, 0) => "没有终端接着".to_string(),
        (false, n) => format!("{n} 处接入"),
    };
    format!(
        "{}  {}  {}  {}  {} 开始  {}",
        &s.meta.sid[..8],
        s.meta.tool,
        s.meta.name,
        cwd,
        strftime_local("%m-%d %H:%M", s.meta.since),
        who
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
            sessions.push(dict! {
                "id" => s.meta.sid.as_str(), "short" => &s.meta.sid[..8], "where" => "local", "peer" => V::None, "machine" => V::None,
                "tool" => s.meta.tool.as_str(), "name" => s.meta.name.as_str(), "cwd" => home_tilde(&s.meta.cwd),
                "since_epoch" => s.meta.since, "viewers" => s.viewers, "local_terminal" => s.local,
                "state" => if s.busy { "busy" } else { "ok" }, "access" => "control"
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
            sessions.push(dict! {
                "id" => f.sid.as_str(), "short" => &f.sid[..8], "where" => "peer", "peer" => f.pid.as_str(),
                "machine" => f.peer.as_str(), "tool" => f.tool.as_str(), "name" => f.name.as_str(), "cwd" => f.cwd.as_str(),
                "since_epoch" => if f.since > 0.0 { V::from(f.since) } else { V::None }, "viewers" => f.viewers,
                "local_terminal" => V::None,
                "state" => if f.online { "ok" } else { "offline" }, "access" => if f.view_only { "view" } else { "control" }
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
        crate::out!("{}", far::row(f));
    }
    for (_, why, asked) in &seen.errors {
        crate::err!("别的机器上的会话{}：{why}", if asked.is_some() { "没查到" } else { "没查" });
    }
    Ok(())
}
