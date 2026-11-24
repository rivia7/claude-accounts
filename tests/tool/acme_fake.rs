//! acme-fake 目录 应答端口 [--bad-nonce N] [--no-shortlived]：假的证书机构（kit::acmefake 的命令行），原来是
//! `python3 tests/acme_fake.py 目录 应答端口 …`，参数、写出的文件和它一样。
//!
//! 目录里要有 ca.pem、ca.key（签证书的）、cert.pem、key.pem（它自己的 HTTPS 证书）；没有 ca.pem 时先照 kit::acmefake::make_pki
//! 做一套（原来是测试自己做的）。实际监听的端口写到 目录/port，做过的事一行一条 JSON 追加到 目录/log；一直跑到被 kill（收到 TERM 时
//! 退出码 0）。应答端口：去哪个端口取 http-01 的文件（真的证书机构是 80）。--bad-nonce N：每 N 个请求说一次 nonce 不对（客户端应该
//! 自己再发）；--no-shortlived：目录里不列 shortlived 这一种。用法不对退出码 2（照 cca-test 的规矩，还在 $CCA_TEST_TRAP 里记一行），
//! 起不来（证书读不了、openssl 失败……）是 1。

use kit::acmefake::{self, FakeCa, Knobs};
use std::io::Write;
use std::path::Path;

pub const CMDS: &[(&str, crate::Run)] = &[("acme-fake", run)];

const USAGE: &str = "用法：cca-test acme-fake 目录 应答端口 [--bad-nonce N] [--no-shortlived]";

/// 用法不对：说在 stderr，在 $CCA_TEST_TRAP 里记一行（参数里的换行写成 \n，保证一条一行），退出码 2
fn usage(args: &[String], why: &str) -> i32 {
    eprintln!("cca-test acme-fake：{why}\n{USAGE}");
    if let Some(trap) = std::env::var_os("CCA_TEST_TRAP").filter(|t| !t.is_empty()) {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(trap) {
            let line = format!("cca-test acme-fake {}：{why}", args.join(" ")).replace('\n', "\\n");
            let _ = writeln!(f, "{line}");
        }
    }
    2
}

extern "C" fn on_term(_: libc::c_int) {
    // SAFETY: _exit 是 async-signal-safe 的，不碰任何锁和堆
    unsafe { libc::_exit(0) }
}

fn run(args: &[String]) -> i32 {
    let mut pos: Vec<&str> = Vec::new();
    let mut knobs = Knobs::new(0);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return 0;
            }
            "--no-shortlived" => knobs.shortlived = false,
            "--bad-nonce" => {
                i += 1;
                match args.get(i).and_then(|v| v.parse::<u64>().ok()) {
                    Some(n) => knobs.bad_nonce = n,
                    None => return usage(args, "--bad-nonce 后面要跟一个数"),
                }
            }
            a if a.starts_with('-') && a.len() > 1 => return usage(args, &format!("不认识 {a}")),
            a => pos.push(a),
        }
        i += 1;
    }
    let [dir, port] = pos[..] else { return usage(args, "要两个参数：目录、应答端口") };
    let Ok(port) = port.parse::<u16>() else { return usage(args, &format!("应答端口要是一个数，不是 {port}")) };
    knobs.http_port = port;
    let dir = Path::new(dir);
    if !dir.join("ca.pem").exists() {
        if let Err(e) = acmefake::make_pki(dir) {
            eprintln!("cca-test acme-fake：测试的证书做不出来：{e}");
            return 1;
        }
    }
    let _ca = match FakeCa::start(dir, knobs) {
        Ok(ca) => ca,
        Err(e) => {
            eprintln!("cca-test acme-fake：起不来：{e}");
            return 1;
        }
    };
    // SAFETY: 处理函数只调 _exit；装在端口写好以后，和 mail serve 一样
    unsafe {
        libc::signal(libc::SIGTERM, on_term as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
    loop {
        std::thread::park();
    }
}
