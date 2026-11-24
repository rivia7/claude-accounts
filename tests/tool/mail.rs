//! mail serve 目录、mail eml 文件 …：假的 Gmail 和登录邮件（kit::mailfake 的命令行），原来是 python3 tests/mail_fake.py serve|eml。
//!
//! 选项、写出的文件（目录/ports.json、目录/imap.log、邮件）、退出码都和原来一样：用法不对是 2（照 cca-test 的规矩，还在
//! $CCA_TEST_TRAP 里记一行），-h 是 0，出错（--ar 写错、写不了文件、openssl 失败……）是 1；serve 一直跑到被 kill，收到 TERM 时退出码 0。
//! 选项照 Python 3.14 的 argparse 认：--选项 值、--选项=值、不会混淆的前缀（--sub 就是 --subject）、同一个选项写几次以最后一次为准、
//! -- 后面都算位置参数、像负数的（-5、-.5）可以当值；选项和位置参数可以穿插。
//!
//!   mail serve 目录      起假 Gmail 和代理，端口写进 目录/ports.json（{"imap": …, "proxy": …}），日志 目录/imap.log
//!   mail eml 文件 --to 地址 [--link 网址] [--subject …] [--sign from:to:subject] [--ar pass|fail|none|missing]
//!                [--from 地址] [--dkim-d 域名] [--reply-to 地址] [--at 时间戳] [--msgid …]
//!
//! CCA_TEST_MAIL_PIN=秒（只给对照测试用）：eml 随机的部分换成钉住的（kit::mailfake::Pinned），Date 和没给 --at 时的修改时间都用这个时间，
//! 和原来的 Python 逐字节比。

use kit::mailfake::{self, Ar, Eml, Pinned, Random, Source};
use std::io::Write;
use std::path::Path;
use std::sync::OnceLock;

pub const CMDS: &[(&str, crate::Run)] = &[("mail", run)];

const HELP: &str =
    "用法：cca-test mail serve 目录\n      cca-test mail eml 文件 --to 地址 [--link 网址] [--subject 标题] [--sign h=] \
[--ar pass|fail|none|missing] [--from 地址] [--dkim-d 域名] [--reply-to 地址] [--at 时间戳] [--msgid Message-ID]\n";

/// eml 认的选项（都要一个值），先后和原来 add_argument 的一样
const EML_OPTS: &[&str] = &["--to", "--link", "--subject", "--sign", "--ar", "--from", "--dkim-d", "--reply-to", "--at", "--msgid"];

/// 用法不对：说在 stderr，在 $CCA_TEST_TRAP 里记一行（参数里的换行写成 \n，保证一条一行），退出码 2
fn usage(args: &[String], why: &str) -> i32 {
    eprintln!("cca-test mail：{why}（用法见 cca-test mail -h）");
    if let Some(trap) = std::env::var_os("CCA_TEST_TRAP").filter(|t| !t.is_empty()) {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(trap) {
            let line = format!("cca-test mail {}：{why}", args.join(" ")).replace('\n', "\\n");
            let _ = writeln!(f, "{line}");
        }
    }
    2
}

/// 解析的结果
enum Parse {
    /// -h：说用法，退出码 0
    Help,
    /// 用法不对（原来 argparse 的 error）
    Bad(String),
    /// 位置参数（只有一个）和各个选项的值（按出现的先后；同一个选项以最后一次为准）
    Ok(Option<String>, Vec<(&'static str, String)>),
}

/// 一个参数算什么（argparse 的 _parse_optional）
enum Cls {
    /// 位置参数（argparse 的 'A'）
    Arg,
    /// 选项：配上的（名字, = 后面的值, 有没有 =），配上不止一个是写得不明确
    Opt(Vec<(&'static str, Option<String>, bool)>),
    /// 像选项又没有这个选项：最后算「认不出的参数」
    Unknown,
}

fn classify(a: &str, names: &[&'static str]) -> Cls {
    if a.is_empty() || !a.starts_with('-') {
        return Cls::Arg;
    }
    if let Some(n) = names.iter().find(|n| **n == a) {
        return Cls::Opt(vec![(n, None, false)]);
    }
    if a.chars().count() == 1 {
        return Cls::Arg;
    }
    if let Some((o, v)) = a.split_once('=') {
        if let Some(n) = names.iter().find(|n| **n == o) {
            return Cls::Opt(vec![(n, Some(v.to_string()), true)]);
        }
    }
    let (prefix, explicit) = match a.split_once('=') {
        Some((p, v)) => (p, Some(v.to_string())),
        None => (a, None),
    };
    let sep = explicit.is_some();
    let mut found = Vec::new();
    if a.starts_with("--") {
        found.extend(names.iter().filter(|n| n.starts_with(prefix)).map(|n| (*n, explicit.clone(), sep)));
    } else {
        // 单个 - 的：-h 后面直接跟着的算它的值（-hx），别的按前缀
        let short: String = a.chars().take(2).collect();
        let rest: String = a.chars().skip(2).collect();
        for n in names {
            if *n == short {
                found.push((*n, Some(rest.clone()), false));
            } else if n.starts_with(prefix) {
                found.push((*n, explicit.clone(), sep));
            }
        }
    }
    if !found.is_empty() {
        return Cls::Opt(found);
    }
    if looks_negative(a) || a.contains(' ') {
        return Cls::Arg;
    }
    Cls::Unknown
}

/// 像负数（argparse 的 -\.?\d：-5、-.5、-1e3、-5x 都算）：当位置参数或者值，不当选项
fn looks_negative(a: &str) -> bool {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"^-\.?\d").expect("正则写错了")).is_match(a)
}

/// -h 后面直接跟着字符（-hx、-hh=…）：argparse 把后面的字符当成别的单字母选项接着认。true 是照样说用法，false 是出错
fn short_help_chain(mut ex: String, mut sep: bool) -> bool {
    loop {
        if ex.is_empty() || sep || ex.starts_with('-') {
            return false;
        }
        let mut cs = ex.chars();
        let c = cs.next();
        let rest: String = cs.collect();
        if c != Some('h') || rest.is_empty() {
            return true;
        }
        match rest.strip_prefix('=') {
            Some(r) => (ex, sep) = (r.to_string(), true),
            None => (ex, sep) = (rest, false),
        }
    }
}

/// 照 argparse（Python 3.14）解析一个子命令的参数：选项 opts 都要一个值，位置参数一个（名字 pos），required 是必须给的选项
fn argparse(args: &[String], opts: &[&'static str], pos: &str, required: &[&str], floats: &[&str]) -> Parse {
    let mut names: Vec<&'static str> = vec!["-h", "--help"];
    names.extend_from_slice(opts);
    // 每个参数的样子：A（位置参数）、O（选项）、-（第一个 --，它后面的都是 A）
    let mut pat = Vec::with_capacity(args.len());
    let mut cls = Vec::with_capacity(args.len());
    let mut dd = false;
    for a in args {
        if dd {
            pat.push('A');
            cls.push(Cls::Arg);
        } else if a == "--" {
            dd = true;
            pat.push('-');
            cls.push(Cls::Arg);
        } else {
            let c = classify(a, &names);
            pat.push(if matches!(c, Cls::Arg) { 'A' } else { 'O' });
            cls.push(c);
        }
    }
    let mut path: Option<String> = None;
    let mut pos_left = true;
    let mut vals: Vec<(&'static str, String)> = Vec::new();
    let mut extras: Vec<String> = Vec::new();
    // 位置参数 -*A-*：吃掉一个 A 连同前后的 --（第一个 -- 去掉）
    let mut consume_pos = |start: usize, path: &mut Option<String>| -> usize {
        if !pos_left {
            return start;
        }
        let mut i = start;
        while pat.get(i) == Some(&'-') {
            i += 1;
        }
        if pat.get(i) != Some(&'A') {
            return start;
        }
        i += 1;
        while pat.get(i) == Some(&'-') {
            i += 1;
        }
        let mut taken: Vec<String> = args[start..i].to_vec();
        if pat[start..i].contains(&'-') {
            if let Some(k) = taken.iter().position(|s| s == "--") {
                taken.remove(k);
            }
        }
        *path = taken.into_iter().next();
        pos_left = false;
        i
    };
    let max_opt = pat.iter().rposition(|c| *c == 'O');
    let mut i = 0;
    while max_opt.is_some_and(|m| i <= m) {
        let m = max_opt.unwrap_or(0);
        let next = (i..=m).find(|k| pat[*k] == 'O').unwrap_or(m + 1);
        if i != next {
            let end = consume_pos(i, &mut path);
            if end > i {
                i = end;
                continue;
            }
        }
        if pat.get(i) != Some(&'O') {
            extras.extend_from_slice(&args[i..next]);
            i = next;
        }
        // 这个选项连同它的值
        let Cls::Opt(found) = &cls[i] else {
            extras.push(args[i].clone());
            i += 1;
            continue;
        };
        if found.len() > 1 {
            let all: Vec<&str> = found.iter().map(|f| f.0).collect();
            return Parse::Bad(format!("选项 {} 写得不明确：可以是 {}", args[i], all.join("、")));
        }
        let (name, explicit, sep) = found[0].clone();
        if name == "-h" || name == "--help" {
            return match explicit {
                None => Parse::Help,
                Some(ex) if name == "-h" && !ex.is_empty() && short_help_chain(ex.clone(), sep) => Parse::Help,
                Some(ex) => Parse::Bad(format!("{name} 不带值（{}）", ex)),
            };
        }
        let (value, stop) = match explicit {
            Some(v) => (v, i + 1),
            None if pat.get(i + 1) == Some(&'A') => (args[i + 1].clone(), i + 2),
            None => return Parse::Bad(format!("{name} 后面要跟一个值")),
        };
        if floats.contains(&name) && mailfake_float(&value).is_none() {
            return Parse::Bad(format!("{name} 的值不是数：{value}"));
        }
        vals.push((name, value));
        i = stop;
    }
    let stop = consume_pos(i, &mut path);
    extras.extend_from_slice(&args[stop..]);
    let mut missing: Vec<&str> = Vec::new();
    if path.is_none() {
        missing.push(pos);
    }
    missing.extend(required.iter().filter(|r| !vals.iter().any(|(n, _)| n == *r)));
    if !missing.is_empty() {
        return Parse::Bad(format!("缺少 {}", missing.join("、")));
    }
    if !extras.is_empty() {
        return Parse::Bad(format!("认不出的参数：{}", extras.join(" ")));
    }
    Parse::Ok(path, vals)
}

/// float(值)：和 --at 的 type=float 一样
fn mailfake_float(s: &str) -> Option<f64> {
    kit::mailfake::py_float(s)
}

fn run(args: &[String]) -> i32 {
    // 子命令前面只认 -h（和它的前缀）；别的像选项的记下来，最后算认不出的参数
    let mut extras: Vec<String> = Vec::new();
    let mut k = 0;
    while k < args.len() {
        let a = &args[k];
        if a == "--" {
            k += 1;
            break;
        }
        match classify(a, &["-h", "--help"]) {
            Cls::Arg => break,
            Cls::Opt(found) => {
                let (name, explicit, sep) = found[0].clone();
                match explicit {
                    None => {
                        print!("{HELP}");
                        return 0;
                    }
                    Some(ex) if name == "-h" && !ex.is_empty() && short_help_chain(ex.clone(), sep) => {
                        print!("{HELP}");
                        return 0;
                    }
                    Some(ex) => return usage(args, &format!("{name} 不带值（{ex}）")),
                }
            }
            Cls::Unknown => extras.push(a.clone()),
        }
        k += 1;
    }
    let Some(cmd) = args.get(k) else {
        if !extras.is_empty() {
            return usage(args, &format!("认不出的参数：{}", extras.join(" ")));
        }
        // 原来没有子命令时也是说用法、退出码 2
        print!("{HELP}");
        return usage(args, "要一个子命令：serve 或 eml");
    };
    let rest = &args[k + 1..];
    let parsed = match cmd.as_str() {
        "serve" => argparse(rest, &[], "目录", &[], &[]),
        "eml" => argparse(rest, EML_OPTS, "文件", &["--to"], &["--at"]),
        other => return usage(args, &format!("没有 {other} 这个子命令（serve、eml）")),
    };
    let (path, vals) = match parsed {
        Parse::Help => {
            print!("{HELP}");
            return 0;
        }
        Parse::Bad(why) => return usage(args, &why),
        Parse::Ok(_, _) if !extras.is_empty() => return usage(args, &format!("认不出的参数：{}", extras.join(" "))),
        Parse::Ok(p, v) => (p.unwrap_or_default(), v),
    };
    if cmd == "serve" {
        return serve(&path);
    }
    eml(&path, &vals)
}

/// SIGTERM：马上退出、退出码 0（原来的 signal.signal(SIGTERM, lambda *_: os._exit(0))）
extern "C" fn on_term(_: libc::c_int) {
    // SAFETY: _exit 是 async-signal-safe 的，不碰任何锁和堆
    unsafe { libc::_exit(0) }
}

fn serve(dir: &str) -> i32 {
    let started = mailfake::serve_dir(Path::new(dir));
    let _kept = match started {
        Ok(both) => both,
        Err(e) => {
            eprintln!("cca-test mail serve：{e}");
            return 1;
        }
    };
    // SAFETY: 处理函数只调 _exit；装在端口写好以后，和原来一样
    unsafe {
        libc::signal(libc::SIGTERM, on_term as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
    loop {
        std::thread::park();
    }
}

fn eml(path: &str, vals: &[(&'static str, String)]) -> i32 {
    let get = |k: &str| vals.iter().rev().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
    let mut e = Eml::new(&get("--to").unwrap_or_default());
    e.link = get("--link");
    if let Some(v) = get("--subject") {
        e.subject = v;
    }
    if let Some(v) = get("--sign") {
        e.sign = v;
    }
    if let Some(v) = get("--from") {
        e.frm = v;
    }
    if let Some(v) = get("--dkim-d") {
        e.dkim_d = v;
    }
    e.msgid = get("--msgid");
    e.reply_to = get("--reply-to");
    match get("--ar").as_deref().unwrap_or("pass").parse::<Ar>() {
        Ok(ar) => e.ar = ar,
        Err(err) => {
            eprintln!("cca-test mail eml：--ar 只认 pass、fail、none、missing（{err}）");
            return 1;
        }
    }
    let at = get("--at").and_then(|v| mailfake_float(&v));
    let pin = std::env::var("CCA_TEST_MAIL_PIN").ok().and_then(|v| mailfake_float(&v));
    let mut src: Box<dyn Source> = match pin {
        Some(t) => Box::new(Pinned::new(t)),
        None => Box::new(Random),
    };
    let raw = e.build_with(src.as_mut());
    match mailfake::put_eml(Path::new(path), &raw, at.or(pin)) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("cca-test mail eml：{path}：{err}");
            1
        }
    }
}
