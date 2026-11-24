//! `cca-core team-acme` 的参数（Acme:566-584）。
//!
//! Python 那一份用的是 argparse；装它的脚本（server/hub-install.sh）、文档里的命令行都照那个写法，所以这里认的写法、不对时说的话
//! 都照它（CPython 3.14）——对照数据 cli/tests/vectors/server_acme_args.json 是真的拿它解析一条条命令行录下来的：
//! - `--out 目录` 和 `--out=目录` 都行；同一个参数给了几次用最后一次的（--id 是每次都算）
//! - 该带值的后面跟的是另一个开关、`--`，或者什么都没有：`argument --out: expected one argument`。负数的样子（`-1`、`-.5`）、
//!   带空格的、单独一个 `-` 不算开关，可以当值
//! - 从左到右，碰到第一个不对的就停；`-h`、`--help` 轮到它时打帮助、退出码 0（在它前面的错先说）
//! - 都看完了才说少了哪个必须给的，再说多出来哪些不认识的
//!
//! 和 Python 不一样的：不认缩写（`--acc` 当 `--account`）——当成不认识的参数。
//! 用法那一行、帮助的排版也不是 argparse 的原样（它照终端的宽度折行）。

use crate::py::obj::repr_str;
use crate::py::{text, uni};

/// 参数不对时先打的那一行
pub const USAGE: &str = "usage: cca-core team-acme [-h] [--id ID] [--teams TEAMS] [--store STORE] --out OUT --account ACCOUNT \
[--listen LISTEN] [--webroot WEBROOT] [--directory DIRECTORY] [--profile PROFILE] [--once] [--ca CA] [--any-ip] \
[--port-file PORT_FILE] [--renew-at RENEW_AT]";

/// `-h` 打的帮助：各项的说明照 Python 那一份
pub const HELP: &str = "\
给 cca 的网页端申请、续上浏览器信得过的证书（ACME，http-01）

options:
  -h, --help            show this help message and exit
  --id ID               给哪个地址签：公网的 IP 地址，或者域名（可以写几次：一张证书上几个地址）
  --teams TEAMS         几个 Team 共用这一个证书助手：这个目录里一个 Team 一个文件（文件名是服务名，内容是它的网页端的地址），
                        每个地址一张证书，放进 --out/<服务名>/；和 --id 只能用一种
  --store STORE         --teams 时每个地址的证书、私钥存在这里（一个地址一个子目录）
  --out OUT             证书放在这个目录里：fullchain.pem、privkey.pem（--teams 时这里每个 Team 一个子目录）
  --account ACCOUNT     账户的私钥放在这个目录里（account.key；留着它，续证书时用的是同一个账户）
  --listen LISTEN       应答证书机构验证的端口（公网的 80 端口要指到它；默认 0.0.0.0:18080）
  --webroot WEBROOT     不自己听端口：验证的文件写到 这个目录/.well-known/acme-challenge/ 里，由 80 端口上已有的网站服务给出去
  --directory DIRECTORY
                        证书机构的 ACME 目录（默认是 Let's Encrypt）
  --profile PROFILE     要哪一种证书（默认 shortlived：约 6 天，给 IP 地址签的只有这一种）；空的是证书机构默认的
  --once                该要、该续就做一次，然后退出
  --ca CA               测试用：照这个证书机构验 ACME 服务器的证书
  --any-ip              测试用：不是公网的地址也去要
  --port-file PORT_FILE
                        测试用：实际监听的端口写到这里
  --renew-at RENEW_AT   测试用：证书过了有效期的这么多（0～1）就续，默认 0.5";

/// 解析出来的各项（argparse 的 Namespace）。没给的：`id` 是空的，别的可以不给的是 None
#[derive(Clone, Debug, PartialEq)]
pub struct Args {
    pub id: Vec<String>,
    pub teams: Option<String>,
    pub store: Option<String>,
    pub out: String,
    pub account: String,
    pub listen: String,
    pub webroot: Option<String>,
    pub directory: String,
    pub profile: String,
    pub once: bool,
    pub ca: Option<String>,
    pub any_ip: bool,
    pub port_file: Option<String>,
    pub renew_at: f64,
}

impl Args {
    /// 证书过了有效期的这么多就续：--renew-at 夹在 0 到 0.9 之间（Acme:586 的 `min(max(a.renew_at, 0.0), 0.9)`）。
    /// 写法照 Python 的 min、max：给的是 nan 时出来的还是 nan（那样每一轮都去续）
    pub fn fraction(&self) -> f64 {
        let low = if 0.0 > self.renew_at { 0.0 } else { self.renew_at };
        if 0.9 < low {
            0.9
        } else {
            low
        }
    }
}

/// 一条命令行解析出来是什么
#[derive(Clone, Debug, PartialEq)]
pub enum Parsed {
    /// 照这些运行
    Run(Box<Args>),
    /// 打帮助（stdout），退出码 0
    Help,
    /// 参数不对：这句话接在 `cca-core team-acme: error: ` 后面，退出码 2
    Error(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// 后面带一个值
    Value,
    /// 开关，不带值
    Flag,
    Help,
}

/// 认识的参数：名字、哪一种、出错时怎么称呼它
const OPTIONS: [(&str, Kind, &str); 16] = [
    ("-h", Kind::Help, "-h/--help"),
    ("--help", Kind::Help, "-h/--help"),
    ("--id", Kind::Value, "--id"),
    ("--teams", Kind::Value, "--teams"),
    ("--store", Kind::Value, "--store"),
    ("--out", Kind::Value, "--out"),
    ("--account", Kind::Value, "--account"),
    ("--listen", Kind::Value, "--listen"),
    ("--webroot", Kind::Value, "--webroot"),
    ("--directory", Kind::Value, "--directory"),
    ("--profile", Kind::Value, "--profile"),
    ("--once", Kind::Flag, "--once"),
    ("--ca", Kind::Value, "--ca"),
    ("--any-ip", Kind::Flag, "--any-ip"),
    ("--port-file", Kind::Value, "--port-file"),
    ("--renew-at", Kind::Value, "--renew-at"),
];

fn known(name: &str) -> Option<(Kind, &'static str)> {
    OPTIONS.iter().find(|(option, _, _)| *option == name).map(|(_, kind, label)| (*kind, *label))
}

/// 命令行里的一项是什么（argparse 的 _parse_optional）
enum Token<'a> {
    /// 不是开关：可以当前一个参数的值；没人要就是多出来的
    Plain,
    /// `--`：不能当值；它后面的都不当开关看
    Dashes,
    /// 像开关、但不认识
    Unknown,
    /// 认识的。explicit 是 `--名字=值` 里的值
    Option { kind: Kind, label: &'static str, explicit: Option<&'a str> },
    /// `-h` 后面直接跟着别的字（`-hx`）：rest 是后面那些
    ShortHelp { rest: &'a str },
}

fn token(arg: &str) -> Token<'_> {
    if !arg.starts_with('-') {
        return Token::Plain;
    }
    if let Some((kind, label)) = known(arg) {
        return Token::Option { kind, label, explicit: None };
    }
    if arg.len() == 1 {
        return Token::Plain;
    }
    if let Some((name, value)) = arg.split_once('=') {
        if let Some((kind, label)) = known(name) {
            return Token::Option { kind, label, explicit: Some(value) };
        }
    }
    // 一个横线的：前两个字是 -h 就当它是 -h 后面连着写了别的
    if let Some(rest) = arg.strip_prefix("-h") {
        return Token::ShortHelp { rest };
    }
    // 负数的样子（-1、-.5、-1e5：横线、可有可无的点、一个数字开头）不算开关；带空格的也不算
    let mut after = arg.chars().skip(1);
    let first = after.next();
    let digit = if first == Some('.') { after.next() } else { first };
    if digit.is_some_and(uni::is_decimal) || arg.contains(' ') {
        return Token::Plain;
    }
    Token::Unknown
}

fn ignored(label: &str, explicit: &str) -> Parsed {
    Parsed::Error(format!("argument {label}: ignored explicit argument {}", repr_str(explicit)))
}

/// `-h` 后面连着写了别的字（argparse 把它当成几个单字母的开关连着写）：后面是 `=值`、横线开头的不行；是别的字就当那是一个
/// 不认识的开关，前面的 -h 照样算——打帮助
fn short_help(rest: &str) -> Parsed {
    let (mut sep, mut explicit) = (false, rest);
    loop {
        if explicit.is_empty() || sep || explicit.starts_with('-') {
            return ignored("-h/--help", explicit);
        }
        let Some(next) = explicit.strip_prefix('h') else { return Parsed::Help };
        if next.is_empty() {
            return Parsed::Help;
        }
        (sep, explicit) = match next.strip_prefix('=') {
            Some(value) => (true, value),
            None => (false, next),
        };
    }
}

/// 解析命令行（不带程序名和子命令）
pub fn parse(argv: &[String]) -> Parsed {
    let mut tokens = Vec::with_capacity(argv.len());
    let mut dashed = false;
    for arg in argv {
        tokens.push(if dashed {
            Token::Plain
        } else if arg == "--" {
            dashed = true;
            Token::Dashes
        } else {
            token(arg)
        });
    }
    let mut a = Args {
        id: Vec::new(),
        teams: None,
        store: None,
        out: String::new(),
        account: String::new(),
        listen: "0.0.0.0:18080".to_string(),
        webroot: None,
        directory: super::LETSENCRYPT.to_string(),
        profile: "shortlived".to_string(),
        once: false,
        ca: None,
        any_ip: false,
        port_file: None,
        renew_at: 0.5,
    };
    let (mut out, mut account) = (false, false);
    let mut extras: Vec<&str> = Vec::new();
    let mut i = 0usize;
    while let (Some(arg), Some(tok)) = (argv.get(i), tokens.get(i)) {
        i = i.saturating_add(1);
        let (kind, label, explicit) = match tok {
            Token::Plain | Token::Dashes | Token::Unknown => {
                extras.push(arg);
                continue;
            }
            Token::ShortHelp { rest } => return short_help(rest),
            Token::Option { kind, label, explicit } => (*kind, *label, *explicit),
        };
        if kind != Kind::Value {
            if let Some(explicit) = explicit {
                return ignored(label, explicit);
            }
            match label {
                "--once" => a.once = true,
                "--any-ip" => a.any_ip = true,
                _ => return Parsed::Help,
            }
            continue;
        }
        let value = match (explicit, tokens.get(i), argv.get(i)) {
            (Some(explicit), _, _) => explicit.to_string(),
            (None, Some(Token::Plain), Some(next)) => {
                i = i.saturating_add(1);
                next.clone()
            }
            _ => return Parsed::Error(format!("argument {label}: expected one argument")),
        };
        match label {
            "--id" => a.id.push(value),
            "--teams" => a.teams = Some(value),
            "--store" => a.store = Some(value),
            "--out" => (a.out, out) = (value, true),
            "--account" => (a.account, account) = (value, true),
            "--listen" => a.listen = value,
            "--webroot" => a.webroot = Some(value),
            "--directory" => a.directory = value,
            "--profile" => a.profile = value,
            "--ca" => a.ca = Some(value),
            "--port-file" => a.port_file = Some(value),
            _ => match text::to_float(&value) {
                Some(n) => a.renew_at = n,
                None => return Parsed::Error(format!("argument --renew-at: invalid float value: {}", repr_str(&value))),
            },
        }
    }
    let missing: Vec<&str> =
        [("--out", out), ("--account", account)].iter().filter(|(_, given)| !given).map(|(name, _)| *name).collect();
    if !missing.is_empty() {
        return Parsed::Error(format!("the following arguments are required: {}", missing.join(", ")));
    }
    if !extras.is_empty() {
        return Parsed::Error(format!("unrecognized arguments: {}", extras.join(" ")));
    }
    // Acme:583。空的字符串算没给（Python 的 bool('')）
    let given = |value: &Option<String>| value.as_deref().is_some_and(|v| !v.is_empty());
    if !a.id.is_empty() == given(&a.teams) || given(&a.teams) != given(&a.store) {
        return Parsed::Error("要给 --id（一张证书），或者 --teams 连同 --store（几个 Team 共用）".to_string());
    }
    Parsed::Run(Box::new(a))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::py::json;
    use crate::py::num::float_repr;
    use crate::py::obj::{Dict, V};

    fn argv(c: &Dict) -> Vec<String> {
        c.list("argv").unwrap().strs().iter().map(|s| s.to_string()).collect()
    }

    fn opt(v: Option<V>) -> Option<String> {
        v.and_then(|v| v.as_str().map(str::to_string))
    }

    /// 对照数据：Python 的 argparse 对这一条条命令行怎么说，这里就怎么说——收下的各项一样，没收的那句话一样，打帮助的一样
    #[test]
    fn parses_like_argparse() {
        let doc = json::loads(include_str!("../../../tests/vectors/server_acme_args.json")).unwrap().as_dict().unwrap();
        let cases = doc.list("cases").unwrap().items();
        let (mut runs, mut errors, mut helps) = (0, 0, 0);
        for c in &cases {
            let c = c.as_dict().unwrap();
            if c.truthy("abbrev") {
                continue;
            }
            let argv = argv(&c);
            let got = parse(&argv);
            if let Some(ns) = c.dict("ns") {
                let Parsed::Run(a) = got else { panic!("{argv:?}：Python 收了，这里是 {got:?}") };
                let id: Vec<String> = ns.list("id").map(|l| l.strs().iter().map(|s| s.to_string()).collect()).unwrap_or_default();
                assert_eq!(a.id, id, "{argv:?}");
                assert_eq!(
                    (a.teams.clone(), a.store.clone(), a.webroot.clone(), a.ca.clone(), a.port_file.clone()),
                    (
                        opt(ns.get("teams")),
                        opt(ns.get("store")),
                        opt(ns.get("webroot")),
                        opt(ns.get("ca")),
                        opt(ns.get("port_file"))
                    ),
                    "{argv:?}"
                );
                assert_eq!(
                    (
                        Some(a.out.clone()),
                        Some(a.account.clone()),
                        Some(a.listen.clone()),
                        Some(a.directory.clone()),
                        Some(a.profile.clone())
                    ),
                    (
                        opt(ns.get("out")),
                        opt(ns.get("account")),
                        opt(ns.get("listen")),
                        opt(ns.get("directory")),
                        opt(ns.get("profile"))
                    ),
                    "{argv:?}"
                );
                assert_eq!(
                    (Some(a.once), Some(a.any_ip)),
                    (ns.get("once").unwrap().as_bool(), ns.get("any_ip").unwrap().as_bool()),
                    "{argv:?}"
                );
                assert_eq!(float_repr(a.renew_at), &*ns.str("renew_at").unwrap(), "{argv:?}");
                assert_eq!(float_repr(a.fraction()), &*c.str("fraction").unwrap(), "{argv:?}");
                assert_eq!(Some(a.any_ip), c.get("any_ip").unwrap().as_bool(), "{argv:?}");
                runs += 1;
            } else if c.truthy("help") {
                assert_eq!((got, c.get("code").unwrap().as_int()), (Parsed::Help, Some(0)), "{argv:?}");
                helps += 1;
            } else {
                assert_eq!(c.get("code").unwrap().as_int(), Some(2), "{argv:?}");
                assert_eq!(got, Parsed::Error(c.str("error").unwrap().to_string()), "{argv:?}");
                errors += 1;
            }
        }
        assert!(runs >= 45 && errors >= 40 && helps >= 9, "{runs} {errors} {helps}");
    }

    /// 缩写：Python 认（`--acc` 当 `--account`），这里不认——当成不认识的参数
    #[test]
    fn abbreviations_are_not_accepted() {
        let doc = json::loads(include_str!("../../../tests/vectors/server_acme_args.json")).unwrap().as_dict().unwrap();
        let cases: Vec<Dict> =
            doc.list("cases").unwrap().items().iter().map(|c| c.as_dict().unwrap()).filter(|c| c.truthy("abbrev")).collect();
        assert!(cases.len() >= 6);
        let mut python_took = 0;
        for c in cases {
            let argv = argv(&c);
            python_took += usize::from(c.dict("ns").is_some());
            match parse(&argv) {
                Parsed::Error(why) => assert!(
                    why.starts_with("unrecognized arguments: --") || why.starts_with("the following arguments are required: "),
                    "{argv:?}: {why}"
                ),
                other => panic!("{argv:?}: {other:?}"),
            }
        }
        assert!(python_took >= 4, "对照数据里要有 Python 收下的缩写");
        let run = |args: &[&str]| parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(
            run(&["--acc", "/a", "--out", "/o", "--id", "x"]),
            Parsed::Error("the following arguments are required: --account".to_string())
        );
        assert_eq!(
            run(&["--out", "/o", "--account", "/a", "--id", "x", "--onc"]),
            Parsed::Error("unrecognized arguments: --onc".to_string())
        );
    }

    /// 帮助里每个参数都有；用法那一行也是
    #[test]
    fn help_lists_every_option() {
        for (name, _, _) in OPTIONS {
            assert!(HELP.contains(&format!("  {name}")) || HELP.contains(&format!(", {name}")), "{name}");
            assert!(USAGE.contains(if name == "--help" { "[-h]" } else { name }), "{name}");
        }
        assert!(USAGE.starts_with("usage: cca-core team-acme [-h] ") && !USAGE.contains('\n'));
    }
}
