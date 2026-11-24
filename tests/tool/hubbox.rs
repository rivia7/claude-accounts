//! hub-box 目录 [--docker] [--no-systemd] [--ip 地址] [--any-ip]：建一台假的 Linux 服务器（kit::hubbox；原来的
//! `python3 tests/hub_box.py 目录`）。不带选项时和原来一样：没有 Docker、有 systemd、地址 203.0.113.7。建好了什么都不说，退出码 0。
//!
//! box-systemctl、box-docker、acme-stub：假服务器里的 systemctl、docker、证书助手（假服务器的 bin/ 里那两行 sh 调的、它起的，
//! 不是给人用的；见 kit::hubbox 开头）。

use kit::hubbox::{self, Box, Opts};
use std::path::Path;

pub const CMDS: &[(&str, crate::Run)] =
    &[("hub-box", hub_box), ("box-systemctl", box_systemctl), ("box-docker", box_docker), ("acme-stub", acme_stub)];

const USAGE: &str = "用法：cca-test hub-box 目录 [--docker] [--no-systemd] [--ip 地址] [--any-ip]";

/// 用法不对：说在 stderr（后面接着用法），在 $CCA_TEST_TRAP 里记一行，退出码 2
fn usage(args: &[String], why: &str) -> i32 {
    crate::trap::usage_with("hub-box", args, why, &format!("\n{USAGE}"))
}

/// 假服务器里的命令要知道 cca-test 在哪（起服务要换会话，经 cca-test exec；证书助手也是它）：就是自己
fn here() {
    if let Ok(exe) = std::env::current_exe().and_then(std::fs::canonicalize) {
        kit::proc::set_tool(exe);
    }
}

fn hub_box(args: &[String]) -> i32 {
    let mut opts = Opts::default();
    let mut dir = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--docker" => opts.docker = true,
            "--no-systemd" => opts.systemd = false,
            "--any-ip" => opts.any_ip = true,
            "--ip" => match it.next() {
                Some(ip) => opts.ip = ip.clone(),
                None => return usage(args, "--ip 后面要跟地址"),
            },
            s if s.starts_with('-') => return usage(args, &format!("不认识 {s}")),
            _ if dir.is_none() => dir = Some(a),
            _ => return usage(args, "只能写一个目录"),
        }
    }
    let Some(dir) = dir else { return usage(args, "要写在哪个目录建") };
    here();
    match Box::create(Path::new(dir), &opts) {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("cca-test hub-box：建不了假服务器 {dir}：{e}");
            1
        }
    }
}

fn box_systemctl(args: &[String]) -> i32 {
    here();
    hubbox::systemctl(args)
}

fn box_docker(args: &[String]) -> i32 {
    here();
    hubbox::docker(args)
}

fn acme_stub(args: &[String]) -> i32 {
    here();
    hubbox::acme_stub(args)
}
