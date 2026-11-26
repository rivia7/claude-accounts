//! `cca-core team-server`：Team 的同步服务器的参数和启动。Python 的 `main`（Py:2135-2263）。
//!
//! 参数、环境变量、出错时说的话都照 Python 的 argparse：
//! - 参数：--listen、--cert、--key、--behind-proxy、--mode（只有 team）、--team-root、--state、--ttl、--idle、--retain-days、--keys、
//!   --cost、--cost-days、--port-file、--relay-listen、--relay-port、--relay-port-file、--remote、--web-url、--web-listen、
//!   --web-cert、--web-key、--web-root、--web-name-file、--web-port-file，后加的 --push、--push-pin（推送）；`--名字 值` 和
//!   `--名字=值` 两种写法都行
//! - 环境变量：CCA_HUB_HOSTED=1（托管平台上的预设：等于 --behind-proxy --listen 0.0.0.0:8080 --state /data/state.json）、
//!   CCA_TEAM_ROOT、CCA_WEB_URL、CCA_WEB_LISTEN、CCA_WEB_CERT、CCA_WEB_KEY、CCA_PUSH_URL、CCA_PUSH_PIN（命令行里写了的照命令行的）
//! - 参数不对：`usage: cca-core team-server [options]` 和 `cca-core team-server: error: 原因`，退出码 2（原因的写法是 argparse 的）
//! - 启动时出错（证书读不了、状态文件存不了、端口占着、端口文件写不了）：`cca-presence: 起不来：原因` 一句话，退出码 1
//!   （Python 是没人接的异常：一段调用栈，退出码也是 1）
//! - 起来以后在 stderr 说一句（监听在哪、设备几台、签名自检过没过……，Py:2253-2258），然后主线程接原来那个端口的连接；
//!   网页端另开的端口、中转各有自己的线程
//! - 信号：TERM 照系统默认的（马上结束：状态每个请求处理完就存了，没有要收尾的）；INT（Ctrl-C）不说话、退出码 0
//!   （Python 接住 KeyboardInterrupt 就结束，Py:2260-2263）
//!
//! 先后照 Python（哪一步不成时前面的已经做了、后面的没做，测试和安装脚本看得到）：查参数（这时什么都还没写）→ 读证书 → 验签自检 →
//! 读状态文件（根公钥换了的从头开始，这一步会写文件）→ 绑数据端口、读配对的记录、起中转 → 读网页端的文件 → 写数据端口的端口文件 →
//! 绑原来那个端口、写它的端口文件 → 绑网页端另开的端口、起它的线程、写它的端口文件 → 说那一句 → 接连接。
//!
//! 和 Python 不一样的：
//! - 参数名不能只写开头几个字母（argparse 认 `--stat` 是 `--state`）
//! - 监听的地址写得不对（没有端口、端口不是 0～65535 的数）是参数不对、退出码 2、什么都还没写；Python 要到绑的时候才出错
//!   （退出码 1，那时状态文件已经写过了）。--relay-port 不在 0～65535 里也是（Python 原样回给设备）
//! - 原来那个端口也可以听 IPv6 的地址（Python 的只有数据端口可以）
//! - --web-root 没给时用编在程序里的那一份页面（Python 用程序旁边的 web/）

use super::front::{self, Tls};
use super::limits::{Guards, Limits};
use super::push::{self, Gateway};
use super::remote::{Remote, Shared};
use super::store::{Store, StoreCfg};
use super::tls::CertCtx;
use super::web::{self, Web};
use super::wire::{self, Num};
use super::{relay, sshsig, tls, Ctx};
use crate::py::fail::os_error_text;
use crate::py::obj::repr_str;
use crate::py::{path, text, time, uni, Fail, R};
use std::io;
use std::net::{TcpListener, ToSocketAddrs};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

/// 出错时说的那两行里的名字（Python 的是 cca-presence.py）
const PROG: &str = "cca-core team-server";
/// 托管平台上（CCA_HUB_HOSTED=1）状态文件的位置：/data 要挂一块存储（Py:2132）
const HOSTED_STATE: &str = "/data/state.json";
/// 监听的套接字上排队的连接最多几个（Py:2035 的 request_queue_size、Py:2221）
const BACKLOG: libc::c_int = 64;

// ---------------------------------------------------------------- 参数（照 argparse）

/// 一个参数要什么样的值
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// 一段文字
    Text,
    /// `type=float`
    Float,
    /// `type=int`
    Int,
    /// 不带值（`action='store_true'`）
    Flag,
    /// --mode：只能是 team
    Mode,
    /// -h、--help
    Help,
}

/// 认得的参数：名字、要什么值、说明（Py:2141-2176，先后也照它）
const OPTS: [(&str, Kind, &str); 29] = [
    ("--help", Kind::Help, "说明这些参数，然后退出"),
    (
        "--listen",
        Kind::Text,
        "监听的地址:端口（端口 0：随便挑一个，写进 --port-file；默认 0.0.0.0:18443，CCA_HUB_HOSTED=1 时是 0.0.0.0:8080）",
    ),
    ("--cert", Kind::Text, "TLS 证书（自签的 P-256；macOS 的 curl 只给 RSA 和 P-256/P-384 做公钥钉住）"),
    ("--key", Kind::Text, "TLS 私钥"),
    (
        "--behind-proxy",
        Kind::Flag,
        "在托管平台、反向代理后面：只说 HTTP（HTTPS 由平台做，不用 --cert、--key），按 X-Forwarded-For 的最后一项限流",
    ),
    ("--mode", Kind::Mode, "只有 team（Team 的同步服务器）：照旧认得这个参数，不写也是它（以前的 maintainer、own 去掉了）"),
    (
        "--team-root",
        Kind::Text,
        "Team 的根公钥（ssh-ed25519 AAAA…，是公开的；也可以用环境变量 CCA_TEAM_ROOT；只写后面那一段 AAAA… 也行：安装脚本、\
         托管平台的环境变量里没有空格好写）",
    ),
    ("--state", Kind::Text, "状态文件（CCA_HUB_HOSTED=1 时默认是 /data/state.json）"),
    ("--ttl", Kind::Float, "开着会话却这么多秒没心跳就算离线"),
    ("--idle", Kind::Float, "这么多秒没动静算闲置"),
    ("--retain-days", Kind::Float, "上次使用的记录留几天"),
    ("--keys", Kind::Text, "API key 的密文（默认在状态文件旁边的 keys.json）"),
    ("--cost", Kind::Text, "折合金额的密文（默认在状态文件旁边的 cost-slots.json）"),
    ("--cost-days", Kind::Float, "折合金额的密文这么多天没再传就删掉"),
    ("--port-file", Kind::Text, "实际监听的端口写到这里（测试用）"),
    ("--relay-listen", Kind::Text, "远程会话的中转：数据端口监听的地址:端口（不给就不开中转）"),
    ("--relay-port", Kind::Int, "客户端连数据端口用的端口（容器里外端口不一样时给；默认就是监听的那个）"),
    ("--relay-port-file", Kind::Text, "数据端口实际监听的端口写到这里（测试用）"),
    (
        "--no-guests",
        Kind::Flag,
        "不认访客（手机：iOS 上的 cca App、网页端）：设备开不了给手机的配对，已经配了对的手机接入不了（记录留着）。也可以用环境变量 \
         CCA_GUESTS=0",
    ),
    (
        "--push",
        Kind::Text,
        "推送网关（https://主机[:端口]：维护者的服务器上的 cca-core team-push，推送服务的密钥只在它那里）：手机锁着屏、App 没开着时\
         会话里的通知经它推到手机上（只有手机解得开）。要和 --relay-listen、--push-pin 一起给；也可以用环境变量 CCA_PUSH_URL",
    ),
    (
        "--push-pin",
        Kind::Text,
        "推送网关证书的公钥（sha256// 加 base64，curl 的 --pinnedpubkey 那种写法：只认这把公钥）；也可以用环境变量 CCA_PUSH_PIN",
    ),
    ("--remote", Kind::Text, "配对的记录（默认在状态文件旁边的 remote.json）"),
    (
        "--web-url",
        Kind::Text,
        "网页端（手机浏览器接入远程会话的那一页）：手机上打开的地址，https://域名[:端口]/。给了才开网页端（要和 --relay-listen \
         一起；也可以用环境变量 CCA_WEB_URL）。浏览器只认信得过的证书：用 --web-listen 另开一个端口，或者放在做 HTTPS 的代理后面\
         （--behind-proxy）",
    ),
    (
        "--web-listen",
        Kind::Text,
        "网页端另开的监听 地址:端口，用 --web-cert、--web-key 的证书（原来那个端口上自签的证书照旧给 cca 的设备用）。这三项也可以用\
         环境变量 CCA_WEB_LISTEN、CCA_WEB_CERT、CCA_WEB_KEY",
    ),
    ("--web-cert", Kind::Text, "浏览器信得过的证书（连同中间证书；证书机构签的）。文件换了会自己重新读"),
    ("--web-key", Kind::Text, "--web-cert 的私钥"),
    ("--web-root", Kind::Text, "网页端的文件在哪（默认用编在这个程序里的那一份）"),
    (
        "--web-name-file",
        Kind::Text,
        "Team 的名字在这个文件的第一行（安装脚本写的 app/name）：网页的标题、手机主屏幕上的名字带上它。读不了、不对时照旧叫 cca",
    ),
    ("--web-port-file", Kind::Text, "网页端实际监听的端口写到这里（测试用）"),
];

/// 命令行里一个参数的值
#[derive(Clone, Debug, PartialEq)]
enum Val {
    Text(String),
    Float(f64),
    Int(i64),
    Flag,
}

/// 命令行里给了的（还没套默认值、没查合不合在一起）。同一个参数给了几次的，后面的算数
#[derive(Debug, Default, PartialEq)]
struct Given(Vec<(&'static str, Val)>);

impl Given {
    fn get(&self, name: &str) -> Option<&Val> {
        self.0.iter().rev().find(|(n, _)| *n == name).map(|(_, v)| v)
    }

    fn text(&self, name: &str) -> Option<&str> {
        match self.get(name) {
            Some(Val::Text(s)) => Some(s),
            _ => None,
        }
    }

    fn float(&self, name: &str) -> Option<f64> {
        match self.get(name) {
            Some(Val::Float(f)) => Some(*f),
            _ => None,
        }
    }

    fn int(&self, name: &str) -> Option<i64> {
        match self.get(name) {
            Some(Val::Int(n)) => Some(*n),
            _ => None,
        }
    }

    fn flag(&self, name: &str) -> bool {
        self.get(name).is_some()
    }
}

/// 参数读不下去了
#[derive(Debug, PartialEq)]
enum Stop {
    /// -h、--help：说明打到 stdout，退出码 0
    Help,
    /// 参数不对：这是 `error: ` 后面的那一句
    Usage(String),
}

/// 命令行里的一段是什么（argparse 的 `_parse_optional`）
#[derive(Clone, Copy, Debug, PartialEq)]
enum Tok<'a> {
    /// 不是参数名：只能是前面那个参数的值
    Arg,
    /// `--`：它后面的都不是参数名
    Dashes,
    /// 认得的参数（在 OPTS 里的位置），`--名字=值` 时带着值
    Known(usize, Option<&'a str>),
    /// 样子是参数名、但不认得
    Unknown,
}

/// 名字 → 在 OPTS 里的位置
fn option(name: &str) -> Option<usize> {
    let name = if name == "-h" { "--help" } else { name };
    OPTS.iter().position(|(n, _, _)| *n == name)
}

fn classify(arg: &str) -> Tok<'_> {
    if !arg.starts_with('-') {
        return Tok::Arg; // 空的也是
    }
    if let Some(at) = option(arg) {
        return Tok::Known(at, None);
    }
    if arg == "-" {
        return Tok::Arg;
    }
    let (name, sep, value) = text::partition(arg, "=");
    if let Some(at) = option(name).filter(|_| !sep.is_empty()) {
        return Tok::Known(at, Some(value));
    }
    // 像负数的（`-\.?\d`）、带空格的：是值，不是参数名
    let digits = arg.strip_prefix('-').unwrap_or_default();
    let digits = digits.strip_prefix('.').unwrap_or(digits);
    if digits.chars().next().is_some_and(|c| uni::decimal_value(c).is_some()) || arg.contains(' ') {
        return Tok::Arg;
    }
    Tok::Unknown
}

/// 出错的那一句里参数叫什么
fn label(at: usize) -> &'static str {
    match OPTS.get(at) {
        Some((_, Kind::Help, _)) => "-h/--help",
        Some((name, _, _)) => name,
        None => "",
    }
}

/// 照 argparse 读命令行（`_parse_known_args`）：从左到右，头一个不对的地方就停；不认得的攒到最后一起说
fn parse(args: &[String]) -> Result<Given, Stop> {
    let mut toks: Vec<Tok> = Vec::with_capacity(args.len());
    let mut literal = false;
    for arg in args {
        toks.push(match (literal, arg.as_str()) {
            (true, _) => Tok::Arg,
            (false, "--") => {
                literal = true;
                Tok::Dashes
            }
            (false, arg) => classify(arg),
        });
    }
    let (mut given, mut extras) = (Given::default(), Vec::<&str>::new());
    let mut each = args.iter().zip(&toks).peekable();
    while let Some((arg, tok)) = each.next() {
        let Tok::Known(at, explicit) = *tok else {
            extras.push(arg);
            continue;
        };
        let Some((name, kind, _)) = OPTS.get(at).copied() else {
            continue;
        };
        let bad = |why: String| Err(Stop::Usage(format!("argument {}: {why}", label(at))));
        let value = match (matches!(kind, Kind::Flag | Kind::Help), explicit) {
            (true, Some(extra)) => return bad(format!("ignored explicit argument {}", repr_str(extra))),
            (true, None) => "",
            (false, Some(value)) => value,
            // 值是紧跟着的那一段，它得不是参数名
            (false, None) => match each.next_if(|(_, next)| **next == Tok::Arg) {
                Some((value, _)) => value.as_str(),
                None => return bad("expected one argument".into()),
            },
        };
        let val = match kind {
            Kind::Help => return Err(Stop::Help),
            Kind::Flag => Val::Flag,
            Kind::Text => Val::Text(value.to_string()),
            Kind::Float => match text::to_float(value) {
                Some(f) => Val::Float(f),
                None => return bad(format!("invalid float value: {}", repr_str(value))),
            },
            Kind::Int => match text::to_int(value) {
                Some(n) => Val::Int(n),
                None => return bad(format!("invalid int value: {}", repr_str(value))),
            },
            Kind::Mode if value == "team" => Val::Text(value.to_string()),
            Kind::Mode => return bad(format!("invalid choice: {} (choose from team)", repr_str(value))),
        };
        given.0.push((name, val));
    }
    if !extras.is_empty() {
        return Err(Stop::Usage(format!("unrecognized arguments: {}", extras.join(" "))));
    }
    Ok(given)
}

/// -h 打出来的说明
fn help() -> String {
    let mut out = format!("usage: {PROG} [options]\n\ncca 的 Team 的同步服务器\n\noptions:\n");
    for (name, kind, about) in OPTS {
        let head = match kind {
            Kind::Help => "-h, --help".to_string(),
            Kind::Flag => name.to_string(),
            Kind::Mode => format!("{name} {{team}}"),
            Kind::Text | Kind::Float | Kind::Int => {
                format!("{name} {}", name.trim_start_matches('-').replace('-', "_").to_uppercase())
            }
        };
        out.push_str(&format!("  {head}\n        {about}\n"));
    }
    out
}

// ---------------------------------------------------------------- 查参数合不合在一起（Py:2178-2210）

/// 一个监听的地址：主机（方括号去掉了，没写的是 0.0.0.0）和端口
type Addr = (String, u16);

/// 中转（给了 --relay-listen）
#[derive(Clone, Debug, PartialEq)]
struct RelayPlan {
    listen: Addr,
    /// --relay-port：客户端连数据端口用的端口；没给、是 0 就是监听的那个
    port: Option<u16>,
    port_file: Option<String>,
    /// 配对的记录
    remote: String,
    /// 认不认访客（--no-guests、CCA_GUESTS=0 关掉）
    guests: bool,
    /// 推送网关（--push、--push-pin）
    push: Option<Gateway>,
}

/// 网页端另开的监听（给了 --web-listen）
#[derive(Clone, Debug, PartialEq)]
struct WebListen {
    listen: Addr,
    /// 浏览器信得过的证书和私钥；本机试（http://localhost）时没有，只说 HTTP
    pair: Option<(PathBuf, PathBuf)>,
    port_file: Option<String>,
}

/// 网页端（给了 --web-url）
#[derive(Clone, Debug, PartialEq)]
struct WebPlan {
    /// 写成浏览器自己说的那个样子、结尾带 `/` 的地址
    url: String,
    root: Option<PathBuf>,
    name_file: Option<PathBuf>,
    listen: Option<WebListen>,
}

/// 查过了的参数：照它启动
#[derive(Clone, Debug)]
struct Plan {
    listen: Addr,
    proxied: bool,
    /// --cert、--key（--behind-proxy 时没有）
    pair: Option<(PathBuf, PathBuf)>,
    /// 根公钥那一行（`ssh-ed25519 AAAA…`，备注去掉了）
    root: String,
    store: StoreCfg,
    /// 回给设备的 ttl、idle：命令行里给了的是小数，没给的是 Python 的默认值（整数 150、600）
    ttl: Num,
    idle: Num,
    port_file: Option<String>,
    relay: Option<RelayPlan>,
    web: Option<WebPlan>,
}

/// 查参数时要问外面的几样（测试里换成给定的）
struct Probe<'a> {
    /// 环境变量
    env: &'a dyn Fn(&str) -> Option<String>,
    /// 有没有 /data 这个目录
    has_data: bool,
}

/// `地址:端口`：照 Python 的拆法（`rpartition(':')`、主机去掉方括号、没写主机是 0.0.0.0、端口是 `int()` 认的数）
fn addr(given: &str) -> Option<Addr> {
    let (host, _, port) = text::rpartition(given, ":");
    let host = text::strip_chars(host, "[]");
    let port = u16::try_from(text::to_int(port)?).ok()?;
    Some((if host.is_empty() { "0.0.0.0" } else { host }.to_string(), port))
}

/// 参数各自都读得了以后：套上默认值和环境变量，查它们合不合在一起。不合是 `error: ` 后面的那一句（先后照 Python：
/// 两处都不对时说的是前面的那一处）
fn plan(g: &Given, probe: &Probe) -> Result<Plan, String> {
    let env = probe.env;
    // 托管平台上（CCA_HUB_HOSTED=1，cca team hosted 生成的设置）：平台的表单里只填得了环境变量、一条命令，所以不带参数也能跑
    let hosted = env("CCA_HUB_HOSTED").as_deref() == Some("1");
    // 命令行里写了的照命令行的（写成空的也算写了），不然是环境变量；空的和没给一样（Python 里都是照真假用的）
    let pick = |name: &str, fallback: Option<String>| g.text(name).map(str::to_string).or(fallback).filter(|s| !s.is_empty());
    let listen = pick("--listen", None).unwrap_or_else(|| if hosted { "0.0.0.0:8080" } else { "0.0.0.0:18443" }.to_string());
    let proxied = g.flag("--behind-proxy") || hosted;
    let Some(state) = pick("--state", hosted.then(|| HOSTED_STATE.to_string())) else {
        return Err("要给 --state（状态文件）".into());
    };
    if hosted && state == HOSTED_STATE && !probe.has_data {
        return Err(
            "没有 /data：在平台上给这个服务挂一块存储，挂载路径 /data（设备、记录、密文存在那里，不然重启一次就全没了）".into()
        );
    }
    let pair = match (proxied, pick("--cert", None), pick("--key", None)) {
        (true, None, None) => None,
        (true, _, _) => return Err("--behind-proxy 时 HTTPS 由平台做：不用 --cert、--key".into()),
        (false, Some(cert), Some(key)) => Some((PathBuf::from(cert), PathBuf::from(key))),
        (false, _, _) => return Err("要给 --cert 和 --key（在托管平台、反向代理后面运行时用 --behind-proxy）".into()),
    };
    // 网页端的那几条（Py:2187-2203）：查法和说的话在 web.rs（`web_args`，和 Python 的命令行逐条对过）
    let relay_listen = pick("--relay-listen", None);
    let web_listen = pick("--web-listen", env("CCA_WEB_LISTEN"));
    let (web_cert, web_key) = (pick("--web-cert", env("CCA_WEB_CERT")), pick("--web-key", env("CCA_WEB_KEY")));
    let (web_url, web_local) = web::web_args(
        pick("--web-url", env("CCA_WEB_URL")).as_deref(),
        relay_listen.as_deref(),
        web_listen.as_deref(),
        web_cert.as_deref(),
        web_key.as_deref(),
        proxied,
    )?;
    // 另开的端口用的证书：本机试（http://localhost）时没有，只说 HTTP
    let web_pair = match (web_local, web_cert, web_key) {
        (false, Some(cert), Some(key)) => Some((PathBuf::from(cert), PathBuf::from(key))),
        _ => None,
    };
    // 根公钥：整行（后面的备注不要）或者只写后面那一段
    let given = pick("--team-root", env("CCA_TEAM_ROOT")).unwrap_or_default();
    let mut fields = text::split_ws(&given);
    fields.truncate(2);
    if fields.len() == 1 && fields.first() != Some(&"ssh-ed25519") {
        fields.insert(0, "ssh-ed25519");
    }
    let line = fields.join(" ");
    let Some(key) = wire::key_re().match_(&line) else {
        return Err("Team 的同步服务器要给根公钥（--team-root 或者环境变量 CCA_TEAM_ROOT：ssh-ed25519 AAAA…）".into());
    };
    let root = format!("ssh-ed25519 {}", key.g(1));

    // 这以下 Python 要到用的时候才出错（那时状态文件已经写过了）；这里先查，不对就是参数不对
    let listening = |name: &str, given: &str| {
        addr(given).ok_or_else(|| format!("{name} 要写成 地址:端口（端口是 0～65535 的数）：{}", repr_str(given)))
    };
    let listen = listening("--listen", &listen)?;
    // 推送：两项一起给。没开中转时命令行里给了是参数不对，环境变量里的不管（推送是远程会话的）
    let push = match (pick("--push", env("CCA_PUSH_URL")), pick("--push-pin", env("CCA_PUSH_PIN"))) {
        _ if relay_listen.is_none() && (g.text("--push").is_some() || g.text("--push-pin").is_some()) => {
            return Err("--push 要和 --relay-listen 一起给（推送是远程会话的）".into());
        }
        (None, None) => None,
        (Some(url), Some(pin)) => Some(Gateway::parse(&url, &pin)?),
        (Some(_), None) => return Err("--push 要和 --push-pin（推送网关证书的公钥）一起给".into()),
        (None, Some(_)) => return Err("--push-pin 要和 --push 一起给".into()),
    };
    let relay = match relay_listen {
        None => None,
        Some(given) => {
            let port = match g.int("--relay-port") {
                None => None,
                Some(n) => match u16::try_from(n) {
                    Ok(n) => Some(n),
                    Err(_) => return Err(format!("--relay-port 要是 0～65535 的数：{n}")),
                },
            };
            let dir = match path::dirname(&state) {
                dir if dir.is_empty() => ".".to_string(),
                dir => dir,
            };
            Some(RelayPlan {
                listen: listening("--relay-listen", &given)?,
                port,
                port_file: pick("--relay-port-file", None),
                remote: pick("--remote", None).unwrap_or_else(|| path::join(&dir, "remote.json")),
                guests: !(g.flag("--no-guests") || env("CCA_GUESTS").as_deref() == Some("0")),
                push,
            })
        }
    };
    let web = match web_url {
        None => None,
        Some(url) => Some(WebPlan {
            url,
            root: pick("--web-root", None).map(PathBuf::from),
            name_file: pick("--web-name-file", None).map(PathBuf::from),
            listen: match web_listen {
                None => None,
                Some(given) => Some(WebListen {
                    listen: listening("--web-listen", &given)?,
                    pair: web_pair,
                    port_file: pick("--web-port-file", None),
                }),
            },
        }),
    };
    // type=float 的参数：给了的是小数；没给的是默认值，Python 里它们是整数（回给设备的 ttl、idle 写出来不带小数点）
    let or_int = |name: &str, default: i64| g.float(name).map_or(Num::Int(default), Num::Float);
    let (ttl, idle) = (or_int("--ttl", 150), or_int("--idle", 600));
    let store = StoreCfg {
        path: PathBuf::from(&state),
        ttl: ttl.f(),
        idle: idle.f(),
        retain: g.float("--retain-days").unwrap_or(30.0) * 86400.0,
        keys_path: pick("--keys", None).map(PathBuf::from),
        cost_path: pick("--cost", None).map(PathBuf::from),
        cost_keep: g.float("--cost-days").unwrap_or(100.0) * 86400.0,
    };
    Ok(Plan { listen, proxied, pair, root, store, ttl, idle, port_file: pick("--port-file", None), relay, web })
}

// ---------------------------------------------------------------- 启动（Py:2211-2263）

/// 参数不对：说两行，退出码 2（argparse 的 `ap.error`）
fn usage<T>(why: &str) -> R<T> {
    crate::err!("usage: {PROG} [options]");
    crate::err!("{PROG}: error: {why}");
    crate::py::exit(2)
}

/// 启动时出的错：一句话，退出码 1
fn fatal(why: impl std::fmt::Display) -> Fail {
    Fail::Die(format!("cca-presence: 起不来：{why}"))
}

/// 绑一个监听的端口（Py:2032-2035、2218-2221）：主机写的是名字就先查出地址；带冒号的当 IPv6，别的只用 IPv4 的地址
/// （Python 的套接字是照这个定的哪一族）；排队的连接最多 64 个
fn bind(at: &Addr) -> io::Result<TcpListener> {
    let (host, port) = (at.0.as_str(), at.1);
    let v6 = host.contains(':');
    let mut last = io::Error::new(io::ErrorKind::AddrNotAvailable, "没有这一族的地址");
    for addr in (host, port).to_socket_addrs()? {
        if addr.is_ipv6() != v6 {
            continue;
        }
        match TcpListener::bind(addr) {
            Ok(l) => {
                // 标准库绑的时候排队的上限是它自己定的（128）
                // SAFETY: l 是刚绑好、正在听的套接字；再 listen 一次只改排队的上限
                unsafe { libc::listen(l.as_raw_fd(), BACKLOG) };
                return Ok(l);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn bind_or_die(what: &str, at: &Addr) -> R<(TcpListener, std::net::SocketAddr)> {
    let trouble = |e: io::Error| fatal(format!("{what} {}:{} 听不了（{}）", at.0, at.1, os_error_text(&e)));
    let l = bind(at).map_err(trouble)?;
    let bound = l.local_addr().map_err(trouble)?;
    Ok((l, bound))
}

/// 实际监听的端口写到一个文件里（测试用：端口给的是 0 时等着这个文件）：先写到 `路径.tmp` 再换过去（Py:2234-2242）
fn port_file(path: &str, port: u16) -> R<()> {
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, format!("{port}\n"))
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| fatal(format!("端口文件 {path} 写不了（{}）", os_error_text(&e))))
}

extern "C" fn quit(_sig: libc::c_int) {
    // SAFETY: _exit 可以在信号的处理函数里调；没有要收尾的（状态每个请求处理完就存了，日志是一行一写的）
    unsafe { libc::_exit(0) }
}

/// 起来以后说的那一句（Py:2253-2258）
#[allow(clippy::too_many_arguments)]
fn banner(
    ip: &str,
    port: u16,
    proxied: bool,
    machines: usize,
    sig_ok: bool,
    relay: Option<(u16, usize)>,
    web: Option<(&str, usize, Option<u16>)>,
) -> String {
    let mut line = format!(
        "cca-presence: Team 的同步服务器，监听 {ip}:{port}{}，设备 {machines} 台，签名自检{}",
        if proxied { "（HTTP，HTTPS 由前面的代理做）" } else { "" },
        if sig_ok { "通过" } else { "没通过" },
    );
    if let Some((port, edges)) = relay {
        line.push_str(&format!("，中转的数据端口 {port}（{edges} 个配对）"));
    }
    if let Some((url, files, extra)) = web {
        let extra = extra.map(|port| format!("，另开的端口 {port}")).unwrap_or_default();
        line.push_str(&format!("，网页端 {url}（{files} 个文件{extra}）"));
    }
    line
}

/// 照查过的参数启动，然后一直接连接（不返回）；启动不了是 Err
fn run(p: &Plan) -> R<()> {
    // Ctrl-C：不说话，退出码 0（Py:2260-2263）
    // SAFETY: quit 里只调 _exit
    unsafe { libc::signal(libc::SIGINT, quit as extern "C" fn(libc::c_int) as libc::sighandler_t) };
    let main_tls = match &p.pair {
        None => Tls::Plain,
        Some((cert, key)) => Tls::Fixed(tls::load_pair(cert, key).map_err(fatal)?),
    };
    let sig_ok = sshsig::self_test();
    let mut store =
        Store::open(&p.store, &p.root, Limits::DEFAULT, time::time()).map_err(|e| fatal(format!("状态文件存不了（{e}）")))?;
    store.reply_times(p.ttl, p.idle);
    let machines = store.machines();

    let (mut remote, mut handle, mut web, mut relay_said, mut push_said) = (None, None, None, None, None);
    if let Some(r) = &p.relay {
        let (l, bound) = bind_or_die("数据端口", &r.listen)?;
        l.set_nonblocking(true).map_err(fatal)?;
        // 网页端的地址：旧版 cca 开“给网页端的配对”要它
        let web_url = p.web.as_ref().map(|w| w.url.clone());
        let advertised = r.port.filter(|port| *port != 0).unwrap_or(bound.port());
        let mut rem = Remote::open(Path::new(&r.remote), advertised, web_url, Limits::DEFAULT);
        rem.guests_on = r.guests;
        // 已经不在名单里的设备的配对不要了（根公钥换了、上次停着的时候名单改过）
        rem.keep_only(&store.fps(), time::time()).map_err(fatal)?;
        relay_said = Some((bound.port(), rem.edges()));
        let shared = Arc::new(Shared::new(rem));
        if let Some(gw) = &r.push {
            let pusher = push::start(gw.clone(), Arc::clone(&shared)).map_err(fatal)?;
            shared.lock().set_pusher(pusher);
            push_said = Some(gw.shown());
        }
        handle = Some(relay::start(l, Arc::clone(&shared)).map_err(fatal)?);
        remote = Some(shared);
        if let Some(w) = &p.web {
            match Web::open(&w.url, w.root.as_deref(), web::web_name(w.name_file.as_deref()).as_deref()) {
                Ok(opened) => web = Some(opened),
                // 这时状态文件已经写过了、中转也起来了：Python 也是到这一步才知道（Py:2232-2233）
                Err(e) => return usage(&format!("网页端的文件读不了：{e}")),
            }
        }
        if let Some(file) = &r.port_file {
            port_file(file, bound.port())?;
        }
    }
    let ctx = Arc::new(Ctx { store: Mutex::new(store), remote, relay: handle, web, guards: Mutex::new(Guards::new()), sig_ok });

    let (l, bound) = bind_or_die("端口", &p.listen)?;
    if let Some(file) = &p.port_file {
        port_file(file, bound.port())?;
    }
    // 网页端另开的端口：同一个服务（同样的接口、同一份限流），只是证书是浏览器信得过的那一张。证书有问题也照样起来
    // （只是这个端口握不了手），日志里说
    let mut web_port = None;
    if let Some(wl) = p.web.as_ref().and_then(|w| w.listen.as_ref()) {
        let (web_l, web_bound) = bind_or_die("网页端另开的端口", &wl.listen)?;
        let web_tls = match &wl.pair {
            None => Tls::Plain,
            Some((cert, key)) => Tls::Reload(Arc::new(CertCtx::new(cert, key, time::monotonic()))),
        };
        let shared = Arc::clone(&ctx);
        thread::Builder::new()
            .name("web".into())
            .spawn(move || -> () { front::serve(web_l, web_tls, false, shared) })
            .map_err(|e| fatal(format!("线程起不来（{}）", os_error_text(&e))))?;
        if let Some(file) = &wl.port_file {
            port_file(file, web_bound.port())?;
        }
        web_port = Some(web_bound.port());
    }
    let web_said = ctx.web.as_ref().map(|w| (w.url.as_str(), w.files.len(), web_port));
    crate::err!("{}", banner(&bound.ip().to_string(), bound.port(), p.proxied, machines, sig_ok, relay_said, web_said));
    if let Some(gw) = push_said {
        crate::err!("cca-presence: 推送：手机登记了的，会话里的通知转给推送网关 {gw}");
    }
    front::serve(l, main_tls, p.proxied, ctx)
}

/// `cca-core team-server …`
pub fn cmd_team_server(args: &[String]) -> R<()> {
    let given = match parse(args) {
        Ok(given) => given,
        Err(Stop::Help) => {
            crate::out_raw!("{}", help());
            return Ok(());
        }
        Err(Stop::Usage(why)) => return usage(&why),
    };
    let probe =
        Probe { env: &|name| std::env::var_os(name).map(|v| v.to_string_lossy().into_owned()), has_data: path::isdir("/data") };
    match plan(&given, &probe) {
        Ok(plan) => run(&plan),
        Err(why) => usage(&why),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::py::fs::TempDir;

    /// 一把编出来的根公钥（32 个 0xc8）
    const ROOT: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjI";

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// 参数读不下去时 `error: ` 后面的那一句
    fn refused(list: &[&str]) -> String {
        match parse(&args(list)) {
            Err(Stop::Usage(why)) => why,
            other => panic!("{list:?}: {other:?}"),
        }
    }

    fn given(list: &[&str]) -> Given {
        parse(&args(list)).unwrap_or_else(|stop| panic!("{list:?}: {stop:?}"))
    }

    /// 查参数。环境变量、有没有 /data 是给的
    fn planned(list: &[&str], env: &[(&str, &str)], has_data: bool) -> Result<Plan, String> {
        let env = |name: &str| env.iter().find(|(n, _)| *n == name).map(|(_, v)| v.to_string());
        plan(&given(list), &Probe { env: &env, has_data })
    }

    /// 两段参数接起来
    fn cat<'a>(base: &[&'a str], more: &[&'a str]) -> Vec<&'a str> {
        base.iter().chain(more).copied().collect()
    }

    fn why(list: &[&str], env: &[(&str, &str)]) -> String {
        planned(list, env, false).expect_err(&format!("{list:?}"))
    }

    /// 参数读不下去时说的话，和真的 argparse（Python 3.14，拿 server/cca-presence.py 里造的那个 ap 试的）一样
    #[test]
    fn parse_errors_like_argparse() {
        assert_eq!(refused(&["--mode", "maintainer"]), "argument --mode: invalid choice: 'maintainer' (choose from team)");
        assert_eq!(refused(&["--mode=own"]), "argument --mode: invalid choice: 'own' (choose from team)");
        assert_eq!(refused(&["--mode="]), "argument --mode: invalid choice: '' (choose from team)");
        assert_eq!(refused(&["--mode"]), "argument --mode: expected one argument");
        assert_eq!(refused(&["--release-key", "/x/y", "--state", "s"]), "unrecognized arguments: --release-key /x/y");
        assert_eq!(refused(&["--foo", "a", "b", "--bar=1", "--state", "s"]), "unrecognized arguments: --foo a b --bar=1");
        assert_eq!(refused(&["extra", "--state", "s"]), "unrecognized arguments: extra");
        assert_eq!(refused(&["--state", "s", "x", "--ttl", "5", "y"]), "unrecognized arguments: x y");
        assert_eq!(refused(&[""]), "unrecognized arguments: ");
        assert_eq!(refused(&["-"]), "unrecognized arguments: -");
        assert_eq!(refused(&["-x"]), "unrecognized arguments: -x");
        assert_eq!(refused(&["-5"]), "unrecognized arguments: -5");
        assert_eq!(refused(&["--ttl", "abc"]), "argument --ttl: invalid float value: 'abc'");
        assert_eq!(refused(&["--relay-port", "1.5"]), "argument --relay-port: invalid int value: '1.5'");
        assert_eq!(refused(&["--state", "s", "--ttl"]), "argument --ttl: expected one argument");
        assert_eq!(refused(&["--ttl", "--idle", "5"]), "argument --ttl: expected one argument");
        assert_eq!(refused(&["--ttl", "--5"]), "argument --ttl: expected one argument");
        assert_eq!(refused(&["--state", "-x"]), "argument --state: expected one argument");
        assert_eq!(refused(&["--state", "--"]), "argument --state: expected one argument");
        assert_eq!(refused(&["--behind-proxy=1"]), "argument --behind-proxy: ignored explicit argument '1'");
        assert_eq!(refused(&["--behind-proxy="]), "argument --behind-proxy: ignored explicit argument ''");
        assert_eq!(refused(&["--help=1"]), "argument -h/--help: ignored explicit argument '1'");
        assert_eq!(refused(&["-h=1"]), "argument -h/--help: ignored explicit argument '1'");
        assert_eq!(refused(&["-h="]), "argument -h/--help: ignored explicit argument ''");
        // `--` 后面的都不是参数名；这个程序没有不带名字的参数，所以都是不认得的
        assert_eq!(refused(&["--state", "s", "--", "--ttl", "5"]), "unrecognized arguments: -- --ttl 5");
        assert_eq!(refused(&["--", "--state", "s"]), "unrecognized arguments: -- --state s");
        // 从左到右，头一个不对的地方就停；不认得的攒到最后才说
        assert_eq!(refused(&["--nope", "--ttl", "x"]), "argument --ttl: invalid float value: 'x'");
        assert_eq!(refused(&["--ttl", "x", "-h"]), "argument --ttl: invalid float value: 'x'");
        assert_eq!(parse(&args(&["-h", "--ttl", "x"])), Err(Stop::Help));
        assert_eq!(parse(&args(&["--help"])), Err(Stop::Help));
        // 和 Python 不一样的那一处：只写开头几个字母的不认
        assert_eq!(refused(&["--stat", "s"]), "unrecognized arguments: --stat s");
    }

    /// 读得下去的：两种写法、数字的各种写法、像负数的和带空格的是值、同一个参数后面的算数
    #[test]
    fn parse_values_like_argparse() {
        assert_eq!(given(&[]), Given::default());
        let float = |list: &[&str]| given(list).float("--ttl").unwrap();
        assert_eq!(float(&["--ttl", "-5"]), -5.0);
        assert_eq!(float(&["--ttl", "-.5"]), -0.5);
        assert_eq!(float(&["--ttl=5"]), 5.0);
        assert_eq!(float(&["--ttl", " 7 "]), 7.0);
        assert_eq!(float(&["--ttl", "1_0"]), 10.0);
        assert_eq!(float(&["--ttl", "１２"]), 12.0);
        assert_eq!(float(&["--ttl", "inf"]), f64::INFINITY);
        assert!(float(&["--ttl", "nan"]).is_nan());
        assert_eq!(float(&["--ttl", "1", "--ttl", "2"]), 2.0);
        assert_eq!(given(&["--relay-port", "-1"]).int("--relay-port"), Some(-1));
        assert_eq!(given(&["--relay-port", " 18_446 "]).int("--relay-port"), Some(18446));
        let text = |list: &[&str], name: &str| given(list).text(name).unwrap().to_string();
        assert_eq!(text(&["--state", "a b"], "--state"), "a b");
        assert_eq!(text(&["--state", "-a b"], "--state"), "-a b");
        assert_eq!(text(&["--state="], "--state"), "");
        assert_eq!(text(&["--state", ""], "--state"), "");
        assert_eq!(text(&["--state", "-"], "--state"), "-");
        assert_eq!(text(&["--cert=a=b"], "--cert"), "a=b");
        assert_eq!(text(&["--web-url", "-.5x"], "--web-url"), "-.5x");
        assert_eq!(text(&["--web-url", "-1x"], "--web-url"), "-1x");
        assert_eq!(text(&["--listen", "-x y"], "--listen"), "-x y");
        assert_eq!(text(&["--mode", "team", "--mode=team"], "--mode"), "team");
        let g = given(&["--behind-proxy", "--behind-proxy", "--team-root", "ssh-ed25519 AAAA x"]);
        assert!(g.flag("--behind-proxy") && !given(&["--state", "s"]).flag("--behind-proxy"));
        assert_eq!(g.text("--team-root"), Some("ssh-ed25519 AAAA x"));
        assert_eq!((g.text("--state"), g.float("--ttl"), g.int("--relay-port")), (None, None, None));
    }

    /// 说明里每个参数都有；表里没有重名的
    #[test]
    fn help_lists_every_option() {
        let text = help();
        assert!(text.starts_with("usage: cca-core team-server [options]\n\ncca 的 Team 的同步服务器\n\noptions:\n  -h, --help\n"));
        for (name, _, about) in OPTS {
            assert!(text.contains(name) && text.contains(about), "{name}");
            assert_eq!(OPTS.iter().filter(|(n, _, _)| *n == name).count(), 1, "{name}");
        }
        assert!(text.contains("  --listen LISTEN\n") && text.contains("  --retain-days RETAIN_DAYS\n"));
        assert!(text.contains("  --mode {team}\n") && text.contains("  --behind-proxy\n"));
    }

    /// 参数合不合在一起：说的话、两处都不对时先说哪一处，都和 Python 的 main 一样（拿真的服务器试出来的那些）
    #[test]
    fn plan_errors_in_python_order() {
        let base = ["--behind-proxy", "--state", "s", "--team-root", ROOT];
        let tls = ["--state", "s", "--team-root", ROOT, "--cert", "c", "--key", "k"];
        let state = "要给 --state（状态文件）";
        assert_eq!(why(&[], &[]), state);
        assert_eq!(why(&["--state", "", "--team-root", ROOT, "--behind-proxy"], &[]), state);
        assert_eq!(why(&["--team-root", ROOT, "--behind-proxy"], &[]), state);
        assert_eq!(why(&cat(&base, &["--cert", "c"]), &[]), "--behind-proxy 时 HTTPS 由平台做：不用 --cert、--key");
        assert_eq!(why(&cat(&base, &["--key", "k"]), &[]), "--behind-proxy 时 HTTPS 由平台做：不用 --cert、--key");
        let pair = "要给 --cert 和 --key（在托管平台、反向代理后面运行时用 --behind-proxy）";
        assert_eq!(why(&["--state", "s", "--team-root", ROOT], &[]), pair);
        assert_eq!(why(&["--state", "s", "--team-root", ROOT, "--cert", "c"], &[]), pair);
        assert_eq!(why(&["--state", "s", "--team-root", ROOT, "--cert", "c", "--key", ""], &[]), pair);
        let url = "--web-url 要写成 https://域名[:端口]/（手机上打开的就是它；本机试的时候可以是 http://localhost:端口/）";
        assert_eq!(why(&cat(&base, &["--web-url", "ftp://x"]), &[]), url);
        assert_eq!(why(&cat(&base, &[]), &[("CCA_WEB_URL", "ftp://x")]), url);
        assert_eq!(
            why(&cat(&base, &["--web-url", "https://example.com/"]), &[]),
            "网页端是经中转接入会话的：--web-url 要和 --relay-listen 一起给"
        );
        let web = ["--web-url", "https://example.com/", "--relay-listen", "127.0.0.1:0"];
        assert_eq!(
            why(&cat(&base, &[web.as_slice(), &["--web-listen", "127.0.0.1:0"]].concat()), &[]),
            "--behind-proxy 时 HTTPS 由平台做：网页端就在同一个端口上，不用 --web-listen"
        );
        assert_eq!(why(&cat(&tls, &["--web-listen", "127.0.0.1:0"]), &[]), "--web-listen 要和 --web-url 一起给");
        assert_eq!(why(&cat(&tls, &[web.as_slice(), &["--web-cert", "x"]].concat()), &[]), "--web-listen 要和 --web-url 一起给");
        assert_eq!(why(&cat(&tls, &web), &[("CCA_WEB_KEY", "y")]), "--web-listen 要和 --web-url 一起给");
        assert_eq!(
            why(&cat(&tls, &[web.as_slice(), &["--web-listen", "127.0.0.1:0"]].concat()), &[]),
            "--web-listen 要带浏览器信得过的证书：--web-cert、--web-key"
        );
        assert_eq!(
            why(&cat(&tls, &[web.as_slice(), &["--web-listen", "127.0.0.1:0", "--web-cert", "x"]].concat()), &[]),
            "--web-listen 要带浏览器信得过的证书：--web-cert、--web-key"
        );
        let local = ["--web-url", "http://localhost:8080/", "--relay-listen", "127.0.0.1:0"];
        let local_only = "--web-url 是 http://localhost 时（本机试）：--web-listen 只能听本机的地址，不用证书";
        let listen = ["--web-listen", "127.0.0.1:0", "--web-cert", "x", "--web-key", "y"];
        assert_eq!(why(&cat(&tls, &[local.as_slice(), &listen].concat()), &[]), local_only);
        assert_eq!(why(&cat(&tls, &[local.as_slice(), &["--web-listen", "0.0.0.0:0"]].concat()), &[]), local_only);
        assert_eq!(why(&cat(&tls, &[local.as_slice(), &["--web-listen", ":0"]].concat()), &[]), local_only);
        let root = "Team 的同步服务器要给根公钥（--team-root 或者环境变量 CCA_TEAM_ROOT：ssh-ed25519 AAAA…）";
        assert_eq!(why(&["--behind-proxy", "--state", "s", "--team-root", "nope"], &[]), root);
        assert_eq!(why(&["--behind-proxy", "--state", "s", "--team-root", "ssh-rsa AAAA"], &[]), root);
        assert_eq!(why(&["--behind-proxy", "--state", "s"], &[("CCA_TEAM_ROOT", "x")]), root);
        assert_eq!(why(&["--behind-proxy", "--state", "s"], &[]), root);
        // 命令行里写了的（写成空的也算）盖过环境变量
        assert_eq!(why(&["--behind-proxy", "--state", "s", "--team-root", ""], &[("CCA_TEAM_ROOT", ROOT)]), root);
        // 托管平台上：没有 /data 不启动；--behind-proxy 是默认的，所以带证书的也不行
        let bare = ROOT.split(' ').nth(1).unwrap();
        let hosted = [("CCA_HUB_HOSTED", "1"), ("CCA_TEAM_ROOT", bare)];
        assert_eq!(
            why(&[], &hosted),
            "没有 /data：在平台上给这个服务挂一块存储，挂载路径 /data（设备、记录、密文存在那里，不然重启一次就全没了）"
        );
        assert_eq!(
            why(&["--cert", "c", "--key", "k", "--state", "s"], &hosted),
            "--behind-proxy 时 HTTPS 由平台做：不用 --cert、--key"
        );
        // 先后：前面的那一处先说
        assert_eq!(why(&["--cert", "c", "--web-url", "ftp://x", "--team-root", "nope"], &[]), state);
        assert_eq!(why(&["--state", "s", "--cert", "c", "--web-url", "ftp://x", "--team-root", "nope"], &[]), pair);
        assert_eq!(why(&["--state", "s", "--behind-proxy", "--web-url", "ftp://x", "--team-root", "nope"], &[]), url);
        assert_eq!(
            why(&["--state", "s", "--behind-proxy", "--web-key", "k", "--team-root", "nope"], &[]),
            "--behind-proxy 时 HTTPS 由平台做：网页端就在同一个端口上，不用 --web-listen"
        );
        // 监听的地址这里先查（Python 要到绑的时候才出错）：根公钥不对的先说
        assert_eq!(why(&cat(&base, &["--listen", "nope"]), &[]), "--listen 要写成 地址:端口（端口是 0～65535 的数）：'nope'");
        assert_eq!(why(&["--behind-proxy", "--state", "s", "--listen", "nope"], &[]), root);
        assert!(why(&cat(&base, &["--listen", "127.0.0.1:http"]), &[]).starts_with("--listen 要写成"));
        assert!(why(&cat(&base, &["--listen", "127.0.0.1:65536"]), &[]).starts_with("--listen 要写成"));
        assert!(why(&cat(&base, &["--relay-listen", "127.0.0.1"]), &[]).starts_with("--relay-listen 要写成"));
        assert_eq!(
            why(&cat(&base, &["--relay-listen", ":0", "--relay-port", "65536"]), &[]),
            "--relay-port 要是 0～65535 的数：65536"
        );
        assert_eq!(why(&cat(&base, &["--relay-listen", ":0", "--relay-port", "-1"]), &[]), "--relay-port 要是 0～65535 的数：-1");
    }

    /// 默认值、环境变量、托管平台上的预设、根公钥的两种写法
    #[test]
    fn plan_defaults_env_and_presets() {
        let p = planned(&["--state", "/srv/t/state.json", "--cert", "c.pem", "--key", "k.pem", "--team-root", ROOT], &[], false)
            .unwrap();
        assert_eq!((p.listen.clone(), p.proxied, p.root.as_str()), (("0.0.0.0".to_string(), 18443), false, ROOT));
        assert_eq!(p.pair, Some((PathBuf::from("c.pem"), PathBuf::from("k.pem"))));
        assert_eq!((p.port_file.clone(), p.relay.clone(), p.web.clone()), (None, None, None));
        // 没给 --ttl、--idle：Python 的默认值是整数，回给设备的写出来是 150、600
        assert_eq!((p.ttl, p.idle), (Num::Int(150), Num::Int(600)));
        let s = &p.store;
        assert_eq!(
            (s.path.to_str(), s.ttl, s.idle, s.retain, s.cost_keep),
            (Some("/srv/t/state.json"), 150.0, 600.0, 2_592_000.0, 8_640_000.0)
        );
        assert_eq!((s.keys_path.clone(), s.cost_path.clone()), (None, None));

        // 给了的：小数；别的参数照写的
        let p = planned(
            &[
                "--behind-proxy",
                "--state=state.json",
                "--listen",
                "[::1]:8443",
                "--ttl",
                "60",
                "--idle=30",
                "--retain-days",
                "0.5",
                "--cost-days",
                "2",
                "--keys",
                "k.json",
                "--cost",
                "c.json",
                "--port-file",
                "port",
                "--relay-listen",
                ":0",
                "--relay-port",
                "18446",
                "--relay-port-file",
                "rport",
                "--mode",
                "team",
            ],
            &[("CCA_TEAM_ROOT", &format!("  {ROOT}   书房的 备注 "))],
            false,
        )
        .unwrap();
        assert_eq!((p.listen.clone(), p.proxied, p.pair.clone(), p.root.as_str()), (("::1".to_string(), 8443), true, None, ROOT));
        assert_eq!((p.ttl, p.idle), (Num::Float(60.0), Num::Float(30.0)));
        assert_eq!((p.store.retain, p.store.cost_keep), (43_200.0, 172_800.0));
        assert_eq!((p.store.keys_path.clone(), p.store.cost_path.clone()), (Some("k.json".into()), Some("c.json".into())));
        assert_eq!(p.port_file.as_deref(), Some("port"));
        // 配对的记录默认在状态文件旁边（状态文件没带目录：当前目录）
        let relay = RelayPlan {
            listen: ("0.0.0.0".into(), 0),
            port: Some(18446),
            port_file: Some("rport".into()),
            remote: "./remote.json".into(),
            guests: true,
            push: None,
        };
        assert_eq!(p.relay, Some(relay));
        let p = planned(
            &["--behind-proxy", "--state", "/d/state.json", "--relay-listen", "127.0.0.1:1"],
            &[("CCA_TEAM_ROOT", ROOT)],
            false,
        );
        assert_eq!(p.unwrap().relay.unwrap().remote, "/d/remote.json");
        let p = planned(
            &["--behind-proxy", "--state", "s", "--relay-listen", ":1", "--remote", "/x/r.json", "--team-root", ROOT],
            &[],
            false,
        );
        assert_eq!(
            p.unwrap().relay.unwrap(),
            RelayPlan {
                listen: ("0.0.0.0".into(), 1),
                port: None,
                port_file: None,
                remote: "/x/r.json".into(),
                guests: true,
                push: None
            }
        );
        // 不认访客：--no-guests 或者 CCA_GUESTS=0（别的值照认）
        let guests = |args: &[&str], env: &[(&str, &str)]| {
            let base = ["--behind-proxy", "--state", "s", "--relay-listen", ":1", "--team-root", ROOT];
            planned(&[&base[..], args].concat(), env, false).unwrap().relay.unwrap().guests
        };
        assert!(!guests(&["--no-guests"], &[]));
        assert!(!guests(&[], &[("CCA_GUESTS", "0")]));
        assert!(guests(&[], &[("CCA_GUESTS", "1")]));
        // 推送：--push、--push-pin 一起给（也可以是环境变量 CCA_PUSH_URL、CCA_PUSH_PIN）；没开中转时命令行里给了是参数不对，
        // 环境变量里的不管
        let pin = format!("sha256//{}", crate::py::base64::b64encode(&[7u8; 32]));
        let relayed = ["--behind-proxy", "--state", "s", "--relay-listen", ":1", "--team-root", ROOT];
        let pushed = |args: &[&str], env: &[(&str, &str)]| planned(&cat(&relayed, args), env, false).unwrap().relay.unwrap().push;
        let gw = pushed(&["--push", "https://192.0.2.10:18460", "--push-pin", &pin], &[]).unwrap();
        assert_eq!((gw.shown(), gw.pin), ("192.0.2.10:18460".to_string(), [7u8; 32]));
        assert_eq!(pushed(&[], &[("CCA_PUSH_URL", "https://192.0.2.10"), ("CCA_PUSH_PIN", &pin)]).unwrap().port, 443);
        assert_eq!(pushed(&[], &[]), None);
        assert_eq!(
            why(&cat(&relayed, &["--push", "https://192.0.2.10"]), &[]),
            "--push 要和 --push-pin（推送网关证书的公钥）一起给"
        );
        assert_eq!(why(&cat(&relayed, &["--push-pin", &pin]), &[]), "--push-pin 要和 --push 一起给");
        assert!(why(&cat(&relayed, &["--push", "http://192.0.2.10", "--push-pin", &pin]), &[]).starts_with("--push 要写成"));
        assert!(why(&cat(&relayed, &["--push", "https://192.0.2.10", "--push-pin", "x"]), &[]).starts_with("--push-pin 要写成"));
        let plain = ["--behind-proxy", "--state", "s", "--team-root", ROOT];
        assert_eq!(
            why(&cat(&plain, &["--push", "https://192.0.2.10", "--push-pin", &pin]), &[]),
            "--push 要和 --relay-listen 一起给（推送是远程会话的）"
        );
        assert!(planned(&plain, &[("CCA_PUSH_URL", "https://192.0.2.10"), ("CCA_PUSH_PIN", &pin)], false).is_ok());

        // 根公钥只写后面那一段（安装脚本、托管平台的环境变量里这样写：没有空格）
        let bare = ROOT.split(' ').nth(1).unwrap();
        let hosted = [("CCA_HUB_HOSTED", "1"), ("CCA_TEAM_ROOT", bare)];
        let p = planned(&[], &hosted, true).unwrap();
        assert_eq!(
            (p.listen.clone(), p.proxied, p.pair.clone(), p.root.as_str()),
            (("0.0.0.0".to_string(), 8080), true, None, ROOT)
        );
        assert_eq!(p.store.path.to_str(), Some("/data/state.json"));
        // 托管平台上状态文件写在别处：不用 /data；命令行里的监听地址盖过预设
        let p = planned(&["--state", "/tmp/s.json", "--listen", "127.0.0.1:0"], &hosted, false).unwrap();
        assert_eq!((p.listen.clone(), p.proxied, p.store.path.to_str()), (("127.0.0.1".to_string(), 0), true, Some("/tmp/s.json")));
        // CCA_HUB_HOSTED 只认 1
        assert_eq!(why(&[], &[("CCA_HUB_HOSTED", "true"), ("CCA_TEAM_ROOT", bare)]), "要给 --state（状态文件）");
        // --team-root 盖过环境变量；别的种类的密钥、多出来的字段不算
        let other = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEB";
        let p = planned(&["--behind-proxy", "--state", "s", "--team-root", other], &[("CCA_TEAM_ROOT", ROOT)], false).unwrap();
        assert_eq!(p.root, other);
    }

    /// 网页端的几项：命令行和环境变量、另开的端口、本机试
    #[test]
    fn plan_web() {
        let tls = ["--state", "s", "--team-root", ROOT, "--cert", "c", "--key", "k", "--relay-listen", "127.0.0.1:0"];
        // 只给了地址：网页端就在原来那个端口上
        let p = planned(&cat(&tls, &["--web-url", "https://Relay.Example.com:18445"]), &[], false).unwrap();
        let web = WebPlan { url: "https://relay.example.com:18445/".into(), root: None, name_file: None, listen: None };
        assert_eq!(p.web, Some(web));
        // 另开的端口：环境变量里给的三项
        let env = [
            ("CCA_WEB_URL", "https://relay.example.com/"),
            ("CCA_WEB_LISTEN", "[::]:443"),
            ("CCA_WEB_CERT", "full.pem"),
            ("CCA_WEB_KEY", "priv.pem"),
        ];
        let p = planned(&cat(&tls, &["--web-root", "/w", "--web-name-file", "/n", "--web-port-file", "wp"]), &env, false).unwrap();
        let listen = WebListen {
            listen: ("::".into(), 443),
            pair: Some(("full.pem".into(), "priv.pem".into())),
            port_file: Some("wp".into()),
        };
        let web = WebPlan {
            url: "https://relay.example.com/".into(),
            root: Some("/w".into()),
            name_file: Some("/n".into()),
            listen: Some(listen),
        };
        assert_eq!(p.web, Some(web));
        // 命令行里的盖过环境变量
        let p = planned(
            &cat(&tls, &["--web-url", "https://b.example.com/", "--web-listen", ":1", "--web-cert", "c2", "--web-key", "k2"]),
            &env,
            false,
        );
        let web = p.unwrap().web.unwrap();
        assert_eq!((web.url.as_str(), web.listen.unwrap().pair), ("https://b.example.com/", Some(("c2".into(), "k2".into()))));
        // 本机试：http://localhost，另开的端口只说 HTTP、只能听本机的地址
        for at in ["127.0.0.1:8080", "localhost:8080", "[::1]:8080"] {
            let p = planned(&cat(&tls, &["--web-url", "http://localhost:8080/", "--web-listen", at]), &[], false).unwrap();
            let listen = p.web.unwrap().listen.unwrap();
            assert_eq!((listen.pair, listen.listen.1), (None, 8080), "{at}");
        }
        assert!(why(
            &cat(&tls, &["--web-url", "https://relay.example.com/", "--web-listen", "nope", "--web-cert", "c", "--web-key", "k"]),
            &[]
        )
        .starts_with("--web-listen 要写成"));
    }

    #[test]
    fn listen_addresses() {
        assert_eq!(addr("127.0.0.1:18443"), Some(("127.0.0.1".into(), 18443)));
        assert_eq!(addr(":0"), Some(("0.0.0.0".into(), 0)));
        assert_eq!(addr("[]:1"), Some(("0.0.0.0".into(), 1)));
        assert_eq!(addr("[::1]:8443"), Some(("::1".into(), 8443)));
        assert_eq!(addr("::1:8443"), Some(("::1".into(), 8443)));
        assert_eq!(addr("team.example.com: 8_0 "), Some(("team.example.com".into(), 80)));
        assert_eq!(addr("0.0.0.0:65535"), Some(("0.0.0.0".into(), 65535)));
        for bad in ["", "nope", "127.0.0.1", "127.0.0.1:", "127.0.0.1:http", "127.0.0.1:-1", "127.0.0.1:65536", "127.0.0.1:1.5"] {
            assert_eq!(addr(bad), None, "{bad}");
        }
    }

    /// 绑端口、写端口文件
    #[test]
    fn binds_and_writes_the_port_file() {
        let l = bind(&("127.0.0.1".into(), 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(port != 0 && l.local_addr().unwrap().is_ipv4());
        std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        // 占着的端口：说清楚是哪个
        let Err(Fail::Die(said)) = bind_or_die("端口", &("127.0.0.1".into(), port)) else {
            panic!("占着的端口绑上了");
        };
        assert!(said.starts_with(&format!("cca-presence: 起不来：端口 127.0.0.1:{port} 听不了（[Errno ")), "{said}");
        // 名字查出来的地址只用 IPv4 的（Python 的套接字是 AF_INET）
        assert!(bind(&("localhost".into(), 0)).unwrap().local_addr().unwrap().is_ipv4());
        assert!(bind(&("192.0.2.1".into(), 0)).is_err(), "不是这台机器上的地址");

        let dir = TempDir::new().unwrap();
        let file = format!("{}/port", dir.path);
        port_file(&file, port).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), format!("{port}\n"));
        assert!(!std::path::Path::new(&format!("{file}.tmp")).exists());
        port_file(&file, 1).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "1\n");
        let Err(Fail::Die(said)) = port_file(&format!("{}/nope/port", dir.path), 1) else {
            panic!("没有的目录里写成了");
        };
        assert!(
            said.starts_with("cca-presence: 起不来：端口文件 ") && said.ends_with("写不了（[Errno 2] No such file or directory）"),
            "{said}"
        );
    }

    /// 起来以后说的那一句：和 Python 的一个字不差（Py:2253-2258）
    #[test]
    fn banner_like_python() {
        assert_eq!(
            banner("127.0.0.1", 18443, false, 3, true, None, None),
            "cca-presence: Team 的同步服务器，监听 127.0.0.1:18443，设备 3 台，签名自检通过"
        );
        assert_eq!(
            banner("0.0.0.0", 8080, true, 0, false, None, None),
            "cca-presence: Team 的同步服务器，监听 0.0.0.0:8080（HTTP，HTTPS 由前面的代理做），设备 0 台，签名自检没通过"
        );
        assert_eq!(
            banner("0.0.0.0", 18445, false, 12, true, Some((18446, 2)), None),
            "cca-presence: Team 的同步服务器，监听 0.0.0.0:18445，设备 12 台，签名自检通过，中转的数据端口 18446（2 个配对）"
        );
        assert_eq!(
            banner("0.0.0.0", 18445, false, 1, true, Some((18446, 0)), Some(("https://relay.example.com/", 12, None))),
            "cca-presence: Team 的同步服务器，监听 0.0.0.0:18445，设备 1 台，签名自检通过，中转的数据端口 18446（0 个配对），\
             网页端 https://relay.example.com/（12 个文件）"
        );
        assert_eq!(
            banner("0.0.0.0", 18445, false, 1, true, Some((18446, 0)), Some(("https://relay.example.com:18447/", 12, Some(18447)))),
            "cca-presence: Team 的同步服务器，监听 0.0.0.0:18445，设备 1 台，签名自检通过，中转的数据端口 18446（0 个配对），\
             网页端 https://relay.example.com:18447/（12 个文件，另开的端口 18447）"
        );
    }
}
