//! 测试的替身里要算的几样：tests/isolation_*.rs 在 PATH 上放的替身（假的 open、pgrep、Claude Code……）调它们。原来替身里写的是
//! `"$PYTHON" -I -c '…'`，现在是 `"$CCA_TEST" 角色 参数…`：cca-test 在哪，经替身的环境变量 `CCA_TEST` 给（原来给的是 `PYTHON`）。
//! 打出来的和原来那一行 Python 打的逐字节一样，测试照原样解析：JSON 是 `json.dumps` 的默认写法（`", "` 和 `": "`、
//! 非 ASCII 转义成 `\uXXXX`），后面跟一个换行（`print`）。
//!
//! ```text
//! env-json                      整个环境（照环境里的先后），一个 JSON 对象一行；退出码 0
//!                               （原来是 print(json.dumps(dict(os.environ)))）
//! argv-json 参数…               参数，一个 JSON 数组一行；退出码 0（print(json.dumps(sys.argv[1:]))）
//! env-args-json 名字… -- 参数…  {"env": {名字: 值（环境里没有的是 null）…}, "args": [参数…]} 一行；退出码 0
//!                               （print(json.dumps({"env": {k: os.environ.get(k) for k in 名字}, "args": sys.argv[1:]}))）
//! re-search 正则 名字 文字      环境变量「名字」的值里搜得到这个正则：打出「文字」一行，退出码 0；搜不到：什么都不打，退出码 1
//!                               （re.search；正则是 regex 的写法——cca 拼给 pgrep 的那种，和 Python 的 re 一样）
//! fd-closed 号                  这个号的文件描述符没开着（fstat 不成）：退出码 0；开着：退出码 1
//! hold 文件 就绪文件            以写的方式打开「文件」（没有就建、有就清空），再建出「就绪文件」（touch），然后一直拿着「文件」不关，
//!                               直到标准输入读到头（测试那头关上管道：这就是叫它停）或者被信号停掉；退出码 0。
//!                               标准输入得是管道（是 /dev/null 的话马上读到头，拿不住）
//! ```
//!
//! 环境里的值不是 UTF-8 的：不能解码的字节换成 U+FFFD（Python 写成 \udcXX；测试里没有这样的值）。env-json 打的就是收到的
//! 环境：原来的 Python 自己还会往里添两个（C locale 下 PEP 538 加的 LC_CTYPE=C.UTF-8、macOS 的 __CF_USER_TEXT_ENCODING），
//! 测试只看点了名的几个变量，不受影响。
//! 用法不对（少了参数、正则写错了、环境里没有要搜的变量、号不是数）：stderr 上说一句，退出码 2，在 $CCA_TEST_TRAP 里记一行——
//! 原来的 Python 这几种是没人接的异常、退出码 1，可 1 是「搜不到」：要是正则这边认不了，测试会把它当成没有这个进程，悄悄通过。

use kit::json::{self, Obj, Style, Value};
use std::io::{Read, Write};

pub const CMDS: &[(&str, crate::Run)] = &[
    ("env-json", env_json),
    ("argv-json", argv_json),
    ("env-args-json", env_args_json),
    ("re-search", re_search),
    ("fd-closed", fd_closed),
    ("hold", hold),
];

/// 用法不对：说在 stderr，在 $CCA_TEST_TRAP 里记一行（参数里的换行写成 \n，保证一条一行），退出码 2
fn usage(role: &str, args: &[String], why: &str) -> i32 {
    eprintln!("cca-test {role}：{why}");
    if let Some(trap) = std::env::var_os("CCA_TEST_TRAP").filter(|t| !t.is_empty()) {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(trap) {
            let line = format!("cca-test {role} {}：{why}", args.join(" ")).replace('\n', "\\n");
            let _ = writeln!(f, "{line}");
        }
    }
    2
}

/// print(json.dumps(v))：写一行。对方不收了（EPIPE）就算了
fn print_json(v: &Value) -> i32 {
    let mut line = json::dumps(v, &Style::default());
    line.push('\n');
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(line.as_bytes()).and_then(|()| out.flush());
    0
}

/// 一个环境变量的值；没有就是 None
fn env_value(name: &str) -> Option<String> {
    std::env::var_os(name).map(|v| v.to_string_lossy().into_owned())
}

fn env_json(args: &[String]) -> i32 {
    if !args.is_empty() {
        return usage("env-json", args, "不要参数");
    }
    let mut env = Obj::new();
    for (key, value) in std::env::vars_os() {
        let key = key.to_string_lossy().into_owned();
        // 同一个名字出现两次的话留头一个（和 Python 的 os.environ 一样）
        if !env.contains_key(&key) {
            env.insert(key, value.to_string_lossy().into_owned());
        }
    }
    print_json(&Value::Obj(env))
}

fn argv_json(args: &[String]) -> i32 {
    print_json(&Value::from(args.to_vec()))
}

fn env_args_json(args: &[String]) -> i32 {
    let Some(at) = args.iter().position(|a| a == "--") else {
        return usage("env-args-json", args, "用法：cca-test env-args-json 名字… -- 参数…");
    };
    let mut env = Obj::new();
    for name in &args[..at] {
        env.insert(name.as_str(), Value::from(env_value(name)));
    }
    let mut doc = Obj::new();
    doc.insert("env", env);
    doc.insert("args", args[at + 1..].to_vec());
    print_json(&Value::Obj(doc))
}

fn re_search(args: &[String]) -> i32 {
    let [pattern, name, say] = args else {
        return usage("re-search", args, "用法：cca-test re-search 正则 环境变量名 搜到时打的文字");
    };
    let rx = match regex::Regex::new(pattern) {
        Ok(rx) => rx,
        Err(e) => return usage("re-search", args, &format!("正则认不了：{e}")),
    };
    let Some(value) = env_value(name) else {
        return usage("re-search", args, &format!("环境里没有 {name}"));
    };
    if !rx.is_match(&value) {
        return 1;
    }
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{say}").and_then(|()| out.flush());
    0
}

fn fd_closed(args: &[String]) -> i32 {
    let [n] = args else { return usage("fd-closed", args, "用法：cca-test fd-closed 号") };
    let Ok(fd) = n.parse::<libc::c_int>() else { return usage("fd-closed", args, &format!("{n} 不是描述符的号")) };
    // SAFETY: fstat 只往 st 里写；st 全是数字，清零就是有效的值
    let open = unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        libc::fstat(fd, &mut st) == 0
    };
    i32::from(open)
}

fn hold(args: &[String]) -> i32 {
    let [path, ready] = args else { return usage("hold", args, "用法：cca-test hold 文件 就绪文件") };
    let held = match std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("cca-test hold：打不开 {path}：{e}");
            return 1;
        }
    };
    if let Err(e) = std::fs::OpenOptions::new().create(true).append(true).open(ready) {
        eprintln!("cca-test hold：建不了 {ready}：{e}");
        return 1;
    }
    // 拿着不放，直到标准输入读到头（读出来的不管）
    let mut stdin = std::io::stdin().lock();
    let mut buf = [0u8; 4096];
    loop {
        match stdin.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    drop(held);
    0
}
