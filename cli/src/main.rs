//! cca-core：cca 的辅助程序，原来内嵌在 cca 里的 Python（PY_SRC）的 Rust 版。
//! bash 照旧 `py 子命令 参数…` 地调它：子命令的名字、参数、标准输入、输出和退出码都和原来一样。

#![allow(dead_code)] // 移植是一个模块一个模块来的：先进来的公共函数，用它的模块可能还没到

mod cmd;
mod core;
mod py;

use py::{Fail, R};

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

fn run(args: Vec<String>) -> R<()> {
    if args.first().map(String::as_str) == Some("_ported") {
        // 看进度用：移植好的子命令「名字<TAB>工具…」，还没移植的「名字<TAB>-」
        for name in cmd::NAMES {
            match cmd::find(name) {
                Some(c) => out!("{}\t{}", name, c.tools.join(" ")),
                None => out!("{}\t-", name),
            }
        }
        return Ok(());
    }
    core::tool::init_current()?;
    let Some(sub) = args.first().filter(|sub| cmd::known(sub)) else {
        let listed = py::obj::V::List(py::obj::str_list(&args)).py_repr();
        return py::die(format!("cca(py): 未知子命令 {listed}"));
    };
    let Some(c) = cmd::find(sub) else {
        return py::die(format!("cca-core: 子命令 {sub} 还没移植"));
    };
    core::model::load_ids();
    (c.run)(&args[1..])
}
