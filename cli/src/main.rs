//! cca-core：cca 的辅助程序，原来内嵌在 cca 里的 Python（PY_SRC）的 Rust 版。
//! bash 照旧 `py 子命令 参数…` 地调它：子命令的名字、参数、标准输入、输出和退出码都和原来一样。

#![allow(dead_code)] // 移植是一个模块一个模块来的：先进来的公共函数，用它的模块可能还没到

mod cmd;
mod core;
mod py;

use py::{Fail, R};
use std::os::unix::process::CommandExt;

fn main() {
    // 没人接的 panic 相当于 Python 里没人接的异常：说一句，退出码 1
    std::panic::set_hook(Box::new(|info| {
        py::io::finish();
        eprintln!("cca-core: 内部错误：{info}");
        std::process::exit(1);
    }));
    let args: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    let code = match run(args) {
        Ok(()) => 0,
        Err(Fail::Die(msg)) => {
            err!("{msg}");
            1
        }
        Err(Fail::Exit(n)) => n,
        Err(e) => {
            // Python 里是没人接的异常（打印调用栈，退出码 1）
            err!("cca-core: 没处理的错误：{e}");
            1
        }
    };
    let flushed = py::io::finish();
    std::process::exit(if code == 0 && !flushed { 120 } else { code });
}

fn run(mut args: Vec<String>) -> R<()> {
    match args.first().map(String::as_str) {
        Some("_ported") => {
            // 已经移植的子命令（开发时混合版脚本和进度都看它）：「名字<TAB>工具…」
            for c in cmd::TABLE {
                if c.run.is_some() {
                    out!("{}\t{}", c.name, c.tools.join(" "));
                }
            }
            return Ok(());
        }
        Some("_mixed") => {
            // 开发时的混合版：`_mixed <python 和它的参数…> -- <子命令> <参数…>`，这个子命令（对当前工具）移植了就自己做，
            // 没移植、或者 CCA_CORE_SKIP 里点了名，就原样交给 Python。切换到只有 Rust 的那天删掉
            let split = args.iter().position(|a| a == "--").unwrap_or(args.len());
            let rest: Vec<String> = args.split_off((split + 1).min(args.len()));
            let python: Vec<String> = args[1..split].to_vec();
            let tool = py::proc::getenv("CCA_TOOL").filter(|s| !s.is_empty()).unwrap_or_else(|| "claude".into());
            let skip = py::proc::env_or_empty("CCA_CORE_SKIP");
            let native = rest.first().is_some_and(|sub| {
                cmd::find(sub).is_some_and(|c| c.run.is_some() && c.tools.contains(&tool.as_str())) && !skip.split_whitespace().any(|s| s == sub)
            });
            if !native {
                if python.is_empty() {
                    return py::die(format!("cca-core: 子命令 {} 还没移植，又没有给 Python", rest.first().map_or("", |s| s.as_str())));
                }
                let e = std::process::Command::new(&python[0]).args(&python[1..]).args(&rest).exec();
                return py::die(format!("cca-core: 起不了 {}：{}", python[0], e));
            }
            args = rest;
        }
        _ => {}
    }
    core::tool::init_current()?;
    let Some(c) = args.first().and_then(|sub| cmd::find(sub)) else {
        let listed = py::obj::V::List(py::obj::str_list(&args)).py_repr();
        return py::die(format!("cca(py): 未知子命令 {listed}"));
    };
    let Some(f) = c.run else {
        return py::die(format!("cca-core: 子命令 {} 还没移植", c.name));
    };
    core::model::load_ids();
    f(&args[1..])
}
