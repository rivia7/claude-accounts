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
    if args.first().map(String::as_str) == Some("-I") {
        let python = py::proc::getenv("CCA_REAL_PYTHON").filter(|s| !s.is_empty()).unwrap_or_else(|| "python3".into());
        let at = as_python(&args);
        // 这三个只是给混合版自己看的：不留给后面起的进程（回归会查子进程的环境里多没多东西）
        for k in ["CCA_REAL_PYTHON", "CCA_CORE_LOG", "CCA_CORE_SKIP"] {
            std::env::remove_var(k);
        }
        match at {
            Some(at) => args.drain(..at).for_each(drop),
            None => {
                let e = std::process::Command::new(&python).args(&args).exec();
                return py::die(format!("cca-core: 起不了 {python}：{e}"));
            }
        }
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

/// 开发时的混合版。cca 认 `CCA_PYTHON`：把它指到 cca-core，cca-core 就被当成 python3 调用——
/// `-I -X utf8 -c <PY_BOOT> <缓存目录> <打包的源码> <子命令> <参数…>`（Linux 上打包的源码是「+段数」再跟那么多段）。
/// 这个子命令（对当前工具）移植了，就返回它在参数里的位置，由 cca-core 自己做；没移植、`CCA_CORE_SKIP` 里点了名、
/// 或者根本不是这种调用（cca 里另外几处内联的 python），返回 None，原样交给真的 Python（`CCA_REAL_PYTHON`）。
/// 这样不改 cca 就能拿整套回归验证移植了的那部分。切换到只有 Rust 的那天删掉
fn as_python(args: &[String]) -> Option<usize> {
    if args.len() < 8 || args[1] != "-X" || args[2] != "utf8" || args[3] != "-c" {
        return None;
    }
    let at = match args[6].strip_prefix('+').and_then(|n| n.parse::<usize>().ok()) {
        Some(n) => 7 + n,
        None => 7,
    };
    let sub = args.get(at)?;
    let tool = py::proc::getenv("CCA_TOOL").filter(|s| !s.is_empty()).unwrap_or_else(|| "claude".into());
    let ported = cmd::find(sub).is_some_and(|c| c.tools.contains(&tool.as_str()));
    let skipped = py::proc::env_or_empty("CCA_CORE_SKIP").split_whitespace().any(|s| s == sub);
    let native = ported && !skipped;
    if let Some(log) = py::proc::getenv("CCA_CORE_LOG").filter(|s| !s.is_empty()) {
        // 记下这一次是谁做的（R = cca-core，P = Python），看混合着跑时移植了的占多少
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(log) {
            let _ = writeln!(f, "{}\t{}\t{}", if native { "R" } else { "P" }, tool, sub);
        }
    }
    native.then_some(at)
}
