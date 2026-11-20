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
use crate::py::{die, R};

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
    let server = relay::server()?;
    let mut known: Option<std::collections::HashSet<String>> = None;
    if distant && !all.is_empty() {
        match &server {
            Ok(srv) => match far::fetch(srv, &all, 8)? {
                Ok(l) => {
                    for f in &l.sessions {
                        sessions.push(dict! {
                            "id" => f.sid.as_str(), "short" => &f.sid[..8], "where" => "peer", "peer" => f.pid.as_str(),
                            "machine" => f.peer.as_str(), "tool" => f.tool.as_str(), "name" => f.name.as_str(), "cwd" => f.cwd.as_str(),
                            "since_epoch" => if f.since > 0.0 { V::from(f.since) } else { V::None }, "viewers" => f.viewers,
                            "local_terminal" => V::None,
                            "state" => if f.online { "ok" } else { "offline" }, "access" => if f.view_only { "view" } else { "control" }
                        });
                    }
                    known = Some(l.known);
                }
                Err(f) => errors.push(dict! { "code" => relay_code(&f), "message" => f.text }),
            },
            Err(why) => errors.push(dict! { "code" => "relay_off", "message" => why.as_str() }),
        }
    }
    let paired = List::new();
    for p in &all {
        let state = match &known {
            Some(k) if k.contains(&p.pid) => "ok",
            Some(_) => "gone",
            None => "unknown",
        };
        paired.push(dict! {
            "id" => p.pid.as_str(), "short" => &p.pid[..8], "name" => p.name.as_str(), "incoming" => p.incoming.word(),
            "shown" => p.shown, "paired_epoch" => p.at, "state" => state
        });
    }
    let relay = match &server {
        Ok(_) => dict! { "available" => true, "message" => V::None },
        Err(why) => dict! { "available" => false, "message" => why.as_str() },
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
    match relay::server()? {
        Ok(srv) => match far::fetch(&srv, &all, 8)? {
            Ok(l) => {
                for f in &l.sessions {
                    crate::out!("{}", far::row(f));
                }
            }
            Err(f) => crate::err!("别的机器上的会话没查到：{}", f.text),
        },
        Err(why) => crate::err!("别的机器上的会话没查：{why}"),
    }
    Ok(())
}
