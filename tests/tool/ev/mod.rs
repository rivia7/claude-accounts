//! 断言的求值器：回归里的断言是 Python 表达式的写法（d["tools"][0]["accounts"] == […] 这样），原来交给 python3 -c 去算；
//! 这里认其中用到的那一部分 Python，算出和 python3 一样的结果（对照的是 Python 3.14：不同版本说法不一样的地方跟的是它）。
//! 认不出、不敢担保的写法一律退出码 2，不猜。
//! 子命令：jok、evq、check、val、edit、run、syntax（参数和退出码见 cli.rs 开头）。
//!
//! 各个文件：lex（切记号）→ parse（语法树，ast）→ interp（求值）；value（值）、ops（运算）、iter（迭代和推导式）、
//! builtins（内建函数）、methods（方法和属性）、pyfmt（repr / str / % 格式化）、jsonio（文件和 JSON）、names（Python 自己有哪些名字）。

mod ast;
mod builtins;
mod cli;
mod interp;
mod iter;
mod jsonio;
mod lex;
mod methods;
mod names;
mod ops;
mod parse;
mod pyfmt;
mod value;

#[cfg(test)]
mod tests;

use cli::Sub;

fn jok(args: &[String]) -> i32 {
    cli::run(Sub::Jok, args.to_vec(), cli::Io::real())
}
fn evq(args: &[String]) -> i32 {
    cli::run(Sub::Evq, args.to_vec(), cli::Io::real())
}
fn check(args: &[String]) -> i32 {
    cli::run(Sub::Check, args.to_vec(), cli::Io::real())
}
fn val(args: &[String]) -> i32 {
    cli::run(Sub::Val, args.to_vec(), cli::Io::real())
}
fn edit(args: &[String]) -> i32 {
    cli::run(Sub::Edit, args.to_vec(), cli::Io::real())
}
fn run(args: &[String]) -> i32 {
    cli::run(Sub::Run, args.to_vec(), cli::Io::real())
}
fn syntax(args: &[String]) -> i32 {
    cli::run(Sub::Syntax, args.to_vec(), cli::Io::real())
}

pub const CMDS: &[(&str, crate::Run)] =
    &[("jok", jok), ("evq", evq), ("check", check), ("val", val), ("edit", edit), ("run", run), ("syntax", syntax)];
