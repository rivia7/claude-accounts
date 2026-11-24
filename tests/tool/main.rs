//! cca-test：回归用的小工具。用法：cca-test 子命令 参数…
//! 各个子命令在各自的模块里（每个模块一张 CMDS 表）；出错（用法不对、认不出的写法）退出码 2，说在 stderr。
mod acme_fake;
mod double;
mod ev;
mod exec;
mod fake;
mod fake_client;
mod fx;
mod gui_session;
mod hubbox;
mod lint;
mod mail;
mod spec;
mod util;

/// 一个子命令：收下子命令后面的参数，返回退出码
pub type Run = fn(&[String]) -> i32;

/// 各个模块的子命令表
const TABLES: &[&[(&str, Run)]] = &[
    acme_fake::CMDS,
    double::CMDS,
    ev::CMDS,
    exec::CMDS,
    fake::CMDS,
    fake_client::CMDS,
    fx::CMDS,
    gui_session::CMDS,
    hubbox::CMDS,
    lint::CMDS,
    mail::CMDS,
    spec::CMDS,
    util::CMDS,
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(name) = args.first() else {
        eprintln!("用法：cca-test 子命令 参数…");
        std::process::exit(2);
    };
    match TABLES.iter().flat_map(|t| t.iter()).find(|(n, _)| n == name) {
        Some((_, run)) => std::process::exit(run(&args[1..])),
        None => {
            eprintln!("cca-test：没有 {name} 这个子命令");
            std::process::exit(2);
        }
    }
}
