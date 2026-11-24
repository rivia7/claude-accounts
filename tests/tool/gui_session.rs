//! gui-session 事件文件 答案表 [--eof-at 题号] [--stale] [--wrong-field 题号] [--keep] -- 命令…：扮演界面 App 的提问会话
//! （kit::gui_session；回归 10h 的 sess，原来的 `python3 tests/gui_session.py`）。命令带着这个小工具的环境，在新会话里运行，
//! fd 3 是答案管道；退出码就是命令的退出码，收到信号时是 128 + 信号。用法不对退出码 2（在 $CCA_TEST_TRAP 里记一行），
//! 答案表、事件文件读不懂是 1。

use kit::gui_session::{self, Opts};
use kit::json;
use kit::proc::Cmd;
use std::path::Path;

pub const CMDS: &[(&str, crate::Run)] = &[("gui-session", gui_session)];

const USAGE: &str = "用法：cca-test gui-session 事件文件 答案表 [--eof-at 题号] [--stale] [--wrong-field 题号] [--keep] -- 命令…";

/// 用法不对：说在 stderr（后面接着用法），在 $CCA_TEST_TRAP 里记一行，退出码 2
fn usage(args: &[String], why: &str) -> i32 {
    crate::trap::usage_with("gui-session", args, why, &format!("\n{USAGE}"))
}

fn gui_session(args: &[String]) -> i32 {
    let [events, table_path, rest @ ..] = args else { return usage(args, "要写事件文件和答案表") };
    let mut opts = Opts::default();
    let mut it = rest.iter();
    let cmd: Vec<&String> = loop {
        match it.next().map(String::as_str) {
            Some("--") => break it.collect(),
            Some(opt @ ("--eof-at" | "--wrong-field")) => {
                let Some(q) = it.next() else { return usage(args, &format!("{opt} 后面要跟题号")) };
                if opt == "--eof-at" {
                    opts.eof_at = Some(q.clone());
                } else {
                    opts.wrong_field = Some(q.clone());
                }
            }
            Some("--stale") => opts.stale = true,
            Some("--keep") => opts.keep = true,
            Some(other) => return usage(args, &format!("不认识的参数：{other}")),
            None => return usage(args, "少了 -- 和要运行的命令"),
        }
    };
    let Some((prog, cmd_args)) = cmd.split_first() else { return usage(args, "-- 后面要写运行哪个命令") };
    let table = match std::fs::read(table_path) {
        Ok(raw) => match json::parse_bytes(&raw) {
            Ok(table) => table,
            Err(e) => {
                eprintln!("cca-test gui-session：答案表 {table_path} 不是 JSON：{e}");
                return 1;
            }
        },
        Err(e) => {
            eprintln!("cca-test gui-session：读不了答案表 {table_path}：{e}");
            return 1;
        }
    };
    // 换会话要经 cca-test exec：就是自己
    if let Ok(exe) = std::env::current_exe().and_then(std::fs::canonicalize) {
        kit::proc::set_tool(exe);
    }
    let mut cmd = Cmd::new(prog);
    cmd.args(cmd_args).env_inherit();
    match gui_session::run(&mut cmd, Path::new(events), &table, &opts) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("cca-test gui-session：{e}");
            1
        }
    }
}
