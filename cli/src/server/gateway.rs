//! 推送网关（`cca-core team-push`，docs/dev/push.md）：跑在维护者的服务器上，推送服务（苹果的 APNs）的密钥（.p8）只在它这里。
//! 各个 Team 的服务器把登记了推送的手机的通知 POST 过来（`/v1/push`，见 push.rs），这里签好令牌交给推送服务。通知的内容是密文
//! （只有那台手机解得开）：这里知道的只是哪个令牌（哪台手机上的 App）、什么时候、多长。
//!
//! - 监听：TLS（--cert、--key：安装脚本用 openssl 现造的 P-256 自签证书；Team 的服务器钉住它的公钥，`--print-pin` 打出来给它们填），
//!   一个连接一个线程（最多 `CONNS` 个），一个连接一个请求，回完就关（读请求、写回复用的是同步服务器的那一套：http.rs）
//! - 请求：`{"v":1,"token","env","id","note"}`（push.rs 发的那样），不对是 400；每个 IP、每个令牌各一个限流的桶，超了 429。
//!   GET /v1/health 回 `{"v":1,"ok":true}`（安装脚本、监控看它起来了没有）
//! - 推送服务：正式、开发两种环境各一条线程、各一条 HTTP/2 连接（h2.rs、apns.rs），排不上队 503。回的：送到了 200、令牌没用了 410
//!   （Team 的服务器拿掉那个登记）、推送服务说这台手机太多了 429、别的 502（日志里说原因，一分钟最多一句，不带令牌）
//! - 不认谁能用：知道网关的地址、手机的令牌才推得过来，推过来的手机也解不开（显示的是 apns.rs 里那两句）；限流挡着刷屏的

pub mod apns;
pub mod h2;

use self::apns::{Outcome, Signer, Tokens};
use super::acme::roots;
use super::http::{self, Conn, DeadlineStream, Head, Method, Reader, Request};
use super::push::{https_addr, take};
use super::wire::{self, Blob};
use super::{tls, x509, PUSH_BLOB};
use crate::py::fail::os_error_text;
use crate::py::json;
use crate::py::obj::V;
use crate::py::time::{self, monotonic};
use crate::py::R;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, ServerConfig};
use std::collections::HashMap;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// 同时最多处理几个连接（多的接进来就关）
const CONNS: usize = 64;
/// 一个连接从接进来起最多多久、握手时一次读写最多等多久、握手以后一次读写最多等多久
const CONN_DEADLINE: Duration = Duration::from_secs(40);
const HANDSHAKE: Duration = Duration::from_secs(5);
const PER_OP: Duration = Duration::from_secs(15);
/// 请求的正文最长多少
const BODY_MAX: usize = 8 * 1024;
/// 每种环境的队列多长
const QUEUE: usize = 256;
/// 限流：每个 IP（Team 的服务器）、每个令牌（手机）攒得下几条、多少秒回一条
const IP_BURST: f64 = 300.0;
const IP_GAP: f64 = 0.1;
const TOKEN_BURST: f64 = 30.0;
const TOKEN_GAP: f64 = 2.0;
/// 连推送服务、等它回最多多久；一条连接空了这么久就换一条（对面多半早关了）
const CONNECT_WAIT: Duration = Duration::from_secs(5);
const REPLY_WAIT: Duration = Duration::from_secs(8);
const IDLE_MAX: Duration = Duration::from_secs(600);
/// 等那条线程交回结果最多多久（它最多试两次：连上、等回各两遍）
const ANSWER_WAIT: Duration = Duration::from_secs(28);
/// 日志里出错的那一句最多多久说一次（秒）
const SAY_EVERY: f64 = 60.0;
/// 默认听的地址
const LISTEN: &str = "0.0.0.0:18460";

const USAGE: &str = "usage: cca-core team-push [-h] --cert CERT [--key KEY] [--print-pin] [--apns-key APNS_KEY] [--key-id KEY_ID] \
[--team-id TEAM_ID] [--listen LISTEN] [--topic TOPIC] [--apns URL] [--apns-sandbox URL] [--ca CA] [--port-file PORT_FILE]";

const HELP: &str = "\
推送网关：各个 Team 的服务器把手机的通知（密文）交过来，这里签好令牌交给苹果的推送服务（APNs）。推送服务的密钥只放在这台机器上

options:
  -h, --help            show this help message and exit
  --cert CERT           这个网关的证书（PEM，P-256：Team 的服务器钉住它的公钥）
  --key KEY             证书的私钥
  --print-pin           打出 --cert 的公钥（sha256// 加 base64：Team 的服务器的 --push-pin 填它），然后退出
  --apns-key APNS_KEY   推送服务的密钥：苹果开发者账号里建的 .p8 文件（只读它，别处不放）
  --key-id KEY_ID       那把密钥的 Key ID（10 个字符）
  --team-id TEAM_ID     开发者账号的 Team ID（10 个字符）
  --listen LISTEN       听哪里（默认 0.0.0.0:18460）
  --topic TOPIC         App 的 bundle id（默认 com.cca.remote）
  --apns URL            测试用：正式环境的推送服务（默认 https://api.push.apple.com）
  --apns-sandbox URL    测试用：开发环境的推送服务（默认 https://api.sandbox.push.apple.com）
  --ca CA               照这个文件里的证书机构验推送服务的证书（默认照系统的；Docker 的空容器里没有，挂一份进去指给它）
  --port-file PORT_FILE
                        测试用：实际监听的端口写到这里";

/// 命令行
#[derive(Clone, Debug, Default, PartialEq)]
struct Args {
    cert: String,
    key: String,
    print_pin: bool,
    apns_key: String,
    key_id: String,
    team_id: String,
    listen: String,
    topic: String,
    apns: String,
    sandbox: String,
    ca: Option<String>,
    port_file: Option<String>,
}

#[derive(Debug, PartialEq)]
enum Stop {
    Help,
    Usage(String),
}

/// `--名字 值`、`--名字=值` 都行；同一个给了几次用最后一次的
fn parse(argv: &[String]) -> Result<Args, Stop> {
    let mut a = Args {
        listen: LISTEN.into(),
        topic: apns::TOPIC.into(),
        apns: apns::PRODUCTION.into(),
        sandbox: apns::SANDBOX.into(),
        ..Args::default()
    };
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        match name {
            "-h" | "--help" => return Err(Stop::Help),
            "--print-pin" => a.print_pin = true,
            "--cert" | "--key" | "--apns-key" | "--key-id" | "--team-id" | "--listen" | "--topic" | "--apns" | "--apns-sandbox"
            | "--ca" | "--port-file" => {
                let value = match inline {
                    Some(v) => v,
                    None => it.next().cloned().ok_or_else(|| Stop::Usage(format!("argument {name}: expected one argument")))?,
                };
                match name {
                    "--cert" => a.cert = value,
                    "--key" => a.key = value,
                    "--apns-key" => a.apns_key = value,
                    "--key-id" => a.key_id = value,
                    "--team-id" => a.team_id = value,
                    "--listen" => a.listen = value,
                    "--topic" => a.topic = value,
                    "--apns" => a.apns = value,
                    "--apns-sandbox" => a.sandbox = value,
                    "--ca" => a.ca = Some(value).filter(|v| !v.is_empty()),
                    _ => a.port_file = Some(value).filter(|v| !v.is_empty()),
                }
            }
            _ => return Err(Stop::Usage(format!("unrecognized arguments: {arg}"))),
        }
    }
    let need = |v: &str, name: &str| if v.is_empty() { Err(Stop::Usage(format!("要给 {name}"))) } else { Ok(()) };
    need(&a.cert, "--cert")?;
    if a.print_pin {
        return Ok(a);
    }
    for (v, name) in [(&a.key, "--key"), (&a.apns_key, "--apns-key"), (&a.key_id, "--key-id"), (&a.team_id, "--team-id")] {
        need(v, name)?;
    }
    let id_ok = |s: &str| s.len() == 10 && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
    if !id_ok(&a.key_id) || !id_ok(&a.team_id) {
        return Err(Stop::Usage("--key-id、--team-id 是 10 个大写字母、数字（开发者账号里看得到）".into()));
    }
    if a.topic.is_empty() || !a.topic.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') {
        return Err(Stop::Usage(format!("--topic 是 App 的 bundle id：{}", a.topic)));
    }
    for (url, name) in [(&a.apns, "--apns"), (&a.sandbox, "--apns-sandbox")] {
        if https_addr(url).is_none() {
            return Err(Stop::Usage(format!("{name} 要写成 https://主机[:端口]：{url}")));
        }
    }
    if listen_addr(&a.listen).is_none() {
        return Err(Stop::Usage(format!("--listen 要写成 地址:端口：{}", a.listen)));
    }
    Ok(a)
}

/// `地址:端口`（IPv6 写在方括号里）
fn listen_addr(given: &str) -> Option<(String, u16)> {
    let (host, port) = given.rsplit_once(':')?;
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    Some((if host.is_empty() { "0.0.0.0" } else { host }.to_string(), port.parse().ok()?))
}

/// 证书文件里头一张证书的公钥：`sha256//` 加 base64（curl 的 --pinnedpubkey 那种写法）
fn pin_of(cert: &str) -> Result<String, String> {
    let pem = std::fs::read(cert).map_err(|e| format!("{cert} 读不了（{}）", os_error_text(&e)))?;
    let first = CertificateDer::pem_slice_iter(&pem).next().and_then(Result::ok).ok_or_else(|| format!("{cert} 里没有证书"))?;
    let spki = x509::cert_spki(first.as_ref()).ok_or_else(|| format!("{cert} 里的证书看不懂"))?;
    Ok(format!("sha256//{}", crate::py::base64::b64encode(&crate::py::hash::sha256(&spki))))
}

/// 推送服务在哪（一种环境）
#[derive(Clone, Debug, PartialEq)]
struct Upstream {
    host: String,
    port: u16,
}

impl Upstream {
    fn of(url: &str) -> Option<Upstream> {
        https_addr(url).map(|(host, port)| Upstream { host, port })
    }

    /// :authority：443 端口的不带端口
    fn authority(&self) -> String {
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        if self.port == 443 {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }
}

/// Team 的服务器交来的一条
#[derive(Clone, Debug, PartialEq)]
struct Push {
    token: String,
    sandbox: bool,
    id: String,
    note: Blob,
}

impl Push {
    /// `{"v":1,"token","env","id","note"}`：样子不对是 None
    fn parse(body: &[u8]) -> Option<Push> {
        let d = json::loads(std::str::from_utf8(body).ok()?).ok()?.as_dict()?;
        if d.get("v").and_then(|v| v.as_int()) != Some(1) {
            return None;
        }
        let token = d.str("token").filter(|t| wire::push_token_re().matches(t))?;
        let sandbox = match &*d.str("env")? {
            "production" => false,
            "sandbox" => true,
            _ => return None,
        };
        let id = d.str("id").filter(|i| wire::note_id_re().matches(i))?;
        let note = wire::parse_blob(&d.get("note")?, PUSH_BLOB)?;
        Some(Push { token: token.to_string(), sandbox, id: id.to_string(), note })
    }
}

/// 交给一种环境的那条线程的：一条通知、结果交回哪里
type Work = (Push, SyncSender<Outcome>);

/// 这个网关：监听用的证书、两条线程的队列、限流、日志
struct Gate {
    tls: Arc<ServerConfig>,
    production: SyncSender<Work>,
    sandbox: SyncSender<Work>,
    /// 每个 IP、每个令牌的桶
    buckets: Mutex<(HashMap<String, (f64, f64)>, HashMap<String, (f64, f64)>)>,
    active: AtomicUsize,
    /// 上一次在日志里说出错是什么时候（单调时钟）
    said: Mutex<f64>,
}

impl Gate {
    fn say(&self, line: String) {
        let now = monotonic();
        let mut said = self.said.lock().unwrap_or_else(PoisonError::into_inner);
        if now - *said >= SAY_EVERY {
            *said = now;
            crate::err!("cca-push: {line}");
        }
    }

    /// 处理一条连接（在它自己的线程里）：握手、读一个请求、回、关
    fn connection(&self, sock: TcpStream) {
        let _ = sock.set_nonblocking(false);
        let peer = sock.peer_addr().map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |a| a.ip());
        let stream = DeadlineStream::new(sock, Some(Instant::now() + CONN_DEADLINE), HANDSHAKE);
        let Ok(mut conn) = Conn::tls(stream, Arc::clone(&self.tls)) else { return };
        conn.stream_mut().set_per_op(PER_OP);
        let mut r = Reader::new(conn);
        let reply = match http::read_head(&mut r, time::time()) {
            Ok(Head::Req { req, v09 }) => self.answer(&mut r, &req, v09, peer),
            Ok(Head::Reply(reply)) => reply,
            Ok(Head::Close) | Err(_) => Vec::new(),
        };
        let mut conn = r.into_parts().0;
        if !reply.is_empty() {
            let _ = conn.write_all(&reply).and_then(|()| conn.flush());
        }
        conn.finish();
    }

    fn answer(&self, r: &mut Reader<Conn>, req: &Request, v09: bool, peer: IpAddr) -> Vec<u8> {
        let now = time::time();
        let reply = |code: u16, d: crate::py::obj::Dict| http::json_reply(code, &V::Dict(d), v09, now);
        let fail = |code: u16, why: &str| reply(code, crate::dict! { "error" => why });
        let path = req.target.split('?').next().unwrap_or_default();
        match (req.method, path) {
            (Method::Get, "/v1/health") => return reply(200, crate::dict! { "v" => 1, "ok" => true }),
            (Method::Post, "/v1/push") => {}
            (_, "/v1/push") => return fail(405, "method"),
            _ => return fail(404, "not_found"),
        }
        let mono = monotonic();
        if !take(&mut self.buckets.lock().unwrap_or_else(PoisonError::into_inner).0, &peer.to_string(), mono, IP_BURST, IP_GAP) {
            return fail(429, "busy");
        }
        let n = match req.header("Content-Length").map(|v| v.trim().parse::<usize>()) {
            Some(Ok(n)) if n <= BODY_MAX => n,
            Some(Ok(_)) => return fail(413, "too_big"),
            _ => return fail(411, "length"),
        };
        let push = match r.read(n) {
            Ok(body) if body.len() == n => Push::parse(&body),
            _ => None,
        };
        let Some(push) = push else { return fail(400, "bad") };
        if !take(&mut self.buckets.lock().unwrap_or_else(PoisonError::into_inner).1, &push.token, mono, TOKEN_BURST, TOKEN_GAP) {
            return fail(429, "busy");
        }
        let (tx, rx) = sync_channel(1);
        let env = if push.sandbox { "sandbox" } else { "production" };
        let queue = if push.sandbox { &self.sandbox } else { &self.production };
        if queue.try_send((push, tx)).is_err() {
            self.say(format!("推送服务（{env}）那条线程排不上队了"));
            return fail(503, "full");
        }
        match rx.recv_timeout(ANSWER_WAIT) {
            Ok(Outcome::Sent) => reply(200, crate::dict! { "v" => 1, "ok" => true }),
            Ok(Outcome::Gone(_)) => fail(410, "gone"),
            Ok(Outcome::Busy) => fail(429, "busy"),
            Ok(Outcome::Stale) => {
                self.say(format!("推送服务（{env}）换了令牌还说过期了：看看这台机器的时钟"));
                fail(502, "upstream")
            }
            Ok(Outcome::Down(why) | Outcome::Refused(why)) => {
                self.say(format!("推送服务（{env}）没收下：{why}"));
                fail(502, "upstream")
            }
            Err(_) => {
                self.say(format!("推送服务（{env}）{} 秒都没回", ANSWER_WAIT.as_secs()));
                fail(504, "timeout")
            }
        }
    }
}

/// 交给推送服务的一种环境：起一条线程，一条条地发（一条连接，不行了换一条）
fn worker(
    env: &str,
    up: Upstream,
    config: Arc<ClientConfig>,
    tokens: Arc<Tokens>,
    topic: String,
) -> std::io::Result<SyncSender<Work>> {
    let (tx, rx) = sync_channel::<Work>(QUEUE);
    thread::Builder::new().name(format!("apns-{env}")).spawn(move || {
        let mut conn = None;
        while let Ok((push, done)) = rx.recv() {
            let outcome = deliver(&mut conn, &up, &config, &tokens, &topic, &push);
            let _ = done.try_send(outcome);
        }
    })?;
    Ok(tx)
}

/// 发一条：最多试两次——令牌过期了换一个、对面没处理（GOAWAY、REFUSED_STREAM）或者出了错换一条连接（用了一阵的连接断了：
/// 多半是对面早关了，再试一次；新连的也不行就算了）
fn deliver(
    conn: &mut Option<h2::Conn<h2::Tls>>,
    up: &Upstream,
    config: &Arc<ClientConfig>,
    tokens: &Tokens,
    topic: &str,
    push: &Push,
) -> Outcome {
    let body = apns::payload(&push.note);
    let mut last = Outcome::Down("没试".into());
    for attempt in 0..2 {
        let now = time::time();
        let jwt = match tokens.get(now) {
            Ok(jwt) => jwt,
            Err(why) => return Outcome::Refused(why),
        };
        if conn.as_ref().is_some_and(|c| !c.open() || c.idle_since.elapsed() > IDLE_MAX) {
            *conn = None;
        }
        let reused = conn.is_some();
        let c = match conn {
            Some(c) => c,
            None => match h2::connect(&up.host, up.port, config, CONNECT_WAIT) {
                Ok(c) => conn.insert(c),
                Err(e) => return Outcome::Down(format!("连不上 {}（{e}）", up.authority())),
            },
        };
        let head = apns::headers(&up.authority(), &push.token, &jwt, topic, &push.id, now as i64 + apns::EXPIRES);
        let head: Vec<(&str, &str)> = head.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        match c.request(&head, &body, REPLY_WAIT) {
            Ok(reply) => match apns::judge(reply.status, &reply.body) {
                Outcome::Stale if attempt == 0 => {
                    tokens.stale(now);
                    last = Outcome::Stale;
                }
                Outcome::Down(why) => {
                    *conn = None;
                    last = Outcome::Down(why);
                }
                outcome => return outcome,
            },
            Err(h2::Fail::Retry) => {
                *conn = None;
                last = Outcome::Down("对面没处理（GOAWAY）".into());
            }
            Err(e) => {
                *conn = None;
                last = Outcome::Down(format!("{e}"));
                if !reused {
                    return last;
                }
            }
        }
    }
    last
}

fn fatal(why: String) -> R<()> {
    crate::err!("cca-push: 起不来：{why}");
    crate::py::exit(1)
}

/// `cca-core team-push …`
pub fn cmd_team_push(argv: &[String]) -> R<()> {
    let a = match parse(argv) {
        Ok(a) => a,
        Err(Stop::Help) => {
            crate::out!("{USAGE}\n\n{HELP}");
            return Ok(());
        }
        Err(Stop::Usage(why)) => {
            crate::err!("{USAGE}");
            crate::err!("cca-core team-push: error: {why}");
            return crate::py::exit(2);
        }
    };
    let pin = match pin_of(&a.cert) {
        Ok(pin) => pin,
        Err(why) => return fatal(why),
    };
    if a.print_pin {
        crate::out!("{pin}");
        return Ok(());
    }
    let server_tls = match tls::load_pair(a.cert.as_ref(), a.key.as_ref()) {
        Ok(config) => config,
        Err(why) => return fatal(why),
    };
    // 推送服务的密钥：只读这一次，留在内存里
    let pem = match std::fs::read(&a.apns_key) {
        Ok(pem) => pem,
        Err(e) => return fatal(format!("推送服务的密钥 {} 读不了（{}）", a.apns_key, os_error_text(&e))),
    };
    if std::fs::metadata(&a.apns_key).is_ok_and(|m| std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o077 != 0) {
        crate::err!("cca-push: {} 别的用户也读得了：chmod 600 {}", a.apns_key, a.apns_key);
    }
    let tokens = match Signer::new(&pem, &a.key_id, &a.team_id) {
        Ok(signer) => Arc::new(Tokens::new(signer)),
        Err(why) => return fatal(format!("推送服务的密钥 {}：{why}", a.apns_key)),
    };
    if let Err(why) = tokens.get(time::time()) {
        return fatal(why);
    }
    let roots = match roots::trust(a.ca.as_deref()) {
        Ok(roots) => roots,
        Err(e) => return fatal(format!("证书机构的根证书读不了（{e}）")),
    };
    let client = match tls::client_config(roots) {
        Ok(config) => {
            let mut config = (*config).clone();
            config.alpn_protocols = vec![b"h2".to_vec()];
            Arc::new(config)
        }
        Err(why) => return fatal(why),
    };
    let (Some(prod_up), Some(sandbox_up)) = (Upstream::of(&a.apns), Upstream::of(&a.sandbox)) else {
        return fatal("推送服务的地址不对".into());
    };
    let shown = format!("{}、{}", prod_up.authority(), sandbox_up.authority());
    let start = |env: &str, up: Upstream| worker(env, up, Arc::clone(&client), Arc::clone(&tokens), a.topic.clone());
    let (production, sandbox) = match (start("production", prod_up), start("sandbox", sandbox_up)) {
        (Ok(p), Ok(s)) => (p, s),
        (Err(e), _) | (_, Err(e)) => return fatal(format!("线程起不来（{}）", os_error_text(&e))),
    };
    let Some((host, port)) = listen_addr(&a.listen) else { return fatal(format!("--listen 不对：{}", a.listen)) };
    let l = match TcpListener::bind((host.as_str(), port)) {
        Ok(l) => l,
        Err(e) => return fatal(format!("{} 听不了（{}）", a.listen, os_error_text(&e))),
    };
    let bound = match l.local_addr() {
        Ok(addr) => addr,
        Err(e) => return fatal(os_error_text(&e)),
    };
    if let Some(file) = &a.port_file {
        let tmp = format!("{file}.tmp");
        if let Err(e) = std::fs::write(&tmp, format!("{}\n", bound.port())).and_then(|()| std::fs::rename(&tmp, file)) {
            return fatal(format!("端口文件 {file} 写不了（{}）", os_error_text(&e)));
        }
    }
    crate::err!(
        "cca-push: 推送网关，监听 {bound}，证书的公钥 {pin}（Team 的服务器的 --push-pin 填它）；推送服务 {shown}，App {}，密钥 {}（Team ID {}）",
        a.topic,
        a.key_id,
        a.team_id
    );
    let gate = Arc::new(Gate {
        tls: server_tls,
        production,
        sandbox,
        buckets: Mutex::new((HashMap::new(), HashMap::new())),
        active: AtomicUsize::new(0),
        said: Mutex::new(f64::NEG_INFINITY),
    });
    for sock in l.incoming() {
        let Ok(sock) = sock else {
            thread::sleep(Duration::from_millis(50));
            continue;
        };
        if gate.active.fetch_add(1, Ordering::SeqCst) >= CONNS {
            gate.active.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        let g = Arc::clone(&gate);
        let spawned = thread::Builder::new().name("conn".into()).spawn(move || {
            g.connection(sock);
            g.active.fetch_sub(1, Ordering::SeqCst);
        });
        if spawned.is_err() {
            gate.active.fetch_sub(1, Ordering::SeqCst);
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::py::fs::TempDir;
    use crate::server::x509::{testkit, San};

    fn args(list: &[&str]) -> Result<Args, Stop> {
        parse(&list.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    const BASE: [&str; 10] =
        ["--cert", "c.pem", "--key", "k.pem", "--apns-key", "AuthKey.p8", "--key-id", "KEYID12345", "--team-id", "TEAMID1234"];

    #[test]
    fn arguments() {
        let a = args(&BASE).unwrap();
        assert_eq!(
            (a.listen.as_str(), a.topic.as_str(), a.apns.as_str()),
            ("0.0.0.0:18460", "com.cca.remote", "https://api.push.apple.com")
        );
        assert_eq!(
            (a.sandbox.as_str(), a.ca.clone(), a.port_file.clone(), a.print_pin),
            ("https://api.sandbox.push.apple.com", None, None, false)
        );
        let more = [
            &BASE[..],
            &["--listen=127.0.0.1:0", "--apns", "https://127.0.0.1:9", "--apns-sandbox=https://[::1]:9", "--ca", "ca.pem"],
        ]
        .concat();
        let a = args(&more).unwrap();
        assert_eq!(
            (a.listen.as_str(), a.apns.as_str(), a.sandbox.as_str(), a.ca.as_deref()),
            ("127.0.0.1:0", "https://127.0.0.1:9", "https://[::1]:9", Some("ca.pem"))
        );
        assert!(args(&["--cert", "c.pem", "--print-pin"]).unwrap().print_pin, "只打公钥：别的不用给");
        assert_eq!(args(&["-h"]), Err(Stop::Help));
        let why = |list: &[&str]| match args(list) {
            Err(Stop::Usage(why)) => why,
            other => panic!("{list:?}: {other:?}"),
        };
        assert_eq!(why(&["--print-pin"]), "要给 --cert");
        assert_eq!(why(&BASE[..8]), "要给 --team-id");
        assert_eq!(why(&[&BASE[..], &["--key"]].concat()), "argument --key: expected one argument");
        assert_eq!(why(&[&BASE[..], &["--nope"]].concat()), "unrecognized arguments: --nope");
        assert!(why(&[&BASE[..], &["--key-id", "short"]].concat()).starts_with("--key-id、--team-id"));
        assert!(why(&[&BASE[..], &["--team-id", "teamid1234"]].concat()).starts_with("--key-id、--team-id"));
        assert!(why(&[&BASE[..], &["--apns", "http://x"]].concat()).starts_with("--apns 要写成"));
        assert!(why(&[&BASE[..], &["--listen", "nope"]].concat()).starts_with("--listen 要写成"));
        assert!(why(&[&BASE[..], &["--topic", "a b"]].concat()).starts_with("--topic"));
        assert_eq!(listen_addr(":18460"), Some(("0.0.0.0".into(), 18460)));
        assert_eq!(listen_addr("[::1]:0"), Some(("::1".into(), 0)));
    }

    /// Team 的服务器交来的那一条（push.rs 发的样子）
    #[test]
    fn pushes() {
        let token = "ab".repeat(32);
        let blob = r#"{"n":"AAAAAAAAAAAAAAAAAAAAAA==","c":"AAAA","t":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}"#;
        let body = |v: &str, token: &str, env: &str, id: &str, note: &str| {
            format!(r#"{{"v":{v},"token":"{token}","env":"{env}","id":"{id}","note":{note}}}"#).into_bytes()
        };
        let p = Push::parse(&body("1", &token, "sandbox", "0123456789abcdef", blob)).unwrap();
        assert_eq!((p.token.as_str(), p.sandbox, p.id.as_str()), (token.as_str(), true, "0123456789abcdef"));
        assert!(!Push::parse(&body("1", &token, "production", "0123456789abcdef", blob)).unwrap().sandbox);
        let long = format!(r#"{{"n":"{}==","c":"{}","t":"{}="}}"#, "A".repeat(22), "A".repeat(PUSH_BLOB + 4), "A".repeat(43));
        for bad in [
            body("2", &token, "sandbox", "0123456789abcdef", blob),
            body("1", "AB", "sandbox", "0123456789abcdef", blob),
            body("1", &token, "test", "0123456789abcdef", blob),
            body("1", &token, "sandbox", "0123", blob),
            body("1", &token, "sandbox", "0123456789abcdef", r#""x""#),
            body("1", &token, "sandbox", "0123456789abcdef", &long),
            b"{".to_vec(),
            b"\xff".to_vec(),
        ] {
            assert_eq!(Push::parse(&bad), None, "{}", String::from_utf8_lossy(&bad));
        }
    }

    #[test]
    fn upstreams() {
        let up = |url: &str| Upstream::of(url).unwrap().authority();
        assert_eq!(up(apns::PRODUCTION), "api.push.apple.com");
        assert_eq!(up("https://127.0.0.1:8443/"), "127.0.0.1:8443");
        assert_eq!(up("https://[::1]:8443"), "[::1]:8443");
        assert_eq!(Upstream::of("https://api.sandbox.push.apple.com").unwrap().port, 443);
    }

    /// --print-pin 打的是证书公钥的 SHA-256（和 Team 的服务器钉住的那样算）
    #[test]
    fn pins() {
        let d = TempDir::new().unwrap();
        let key = testkit::new_key();
        let cert = testkit::self_signed(&key, Some(&[San::Ip("127.0.0.1".into())]));
        let path = format!("{}/cert.pem", d.path);
        std::fs::write(&path, testkit::pem("CERTIFICATE", &cert)).unwrap();
        let spki = x509::cert_spki(&cert).unwrap();
        assert_eq!(pin_of(&path).unwrap(), format!("sha256//{}", crate::py::base64::b64encode(&crate::py::hash::sha256(&spki))));
        let gw = super::super::push::Gateway::parse("https://127.0.0.1", &pin_of(&path).unwrap()).unwrap();
        assert_eq!(gw.pin, crate::py::hash::sha256(&spki));
        assert!(pin_of(&format!("{}/missing.pem", d.path)).unwrap_err().contains("读不了"));
    }
}
