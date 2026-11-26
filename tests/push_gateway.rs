//! 推送网关（`cca-core team-push`，docs/dev/push.md）：把辅助程序当黑盒，对着假的推送服务（kit::apnsfake：照规范写的 HTTP/2
//! 服务端，照苹果的文档核对每个请求）跑——交给推送服务的令牌（JWT）、头、内容，推送服务回的各种结果怎么交回 Team 的服务器、
//! 一条连接接着用和换新的、流量控制、不收的请求和限流、命令行和起不来的时候。Team 的服务器经它推到手机的那一整条在
//! tests/web_relay.rs 的 app_guest_gets_pushes。不联网。

use kit::apnsfake::{self, Gateway, Knobs, Seen};
use kit::harness::{self, Ctx, Module};
use kit::json::Value;
use kit::proc::Cmd;
use kit::{case, crypto, fleet, fsx, obj, pki, spec};
use std::net::TcpListener;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const MODULE: Module = Module {
    name: "push_gateway",
    src: include_str!("push_gateway.rs"),
    default: true,
    tests: &[
        case!(pin_health_and_paths),
        case!(what_the_push_service_gets),
        case!(replies_and_retries),
        case!(connections_are_reused_and_replaced),
        case!(flow_control),
        case!(bad_requests_and_limits),
        case!(arguments_and_startup),
    ],
};

// ---- 几份用例共用的小东西 ----

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

fn ok() -> Value {
    obj! { "v" => 1, "ok" => true }
}

fn error(code: &str) -> Value {
    obj! { "error" => code }
}

/// 照真的封好的一条通知（kit::spec 封的；网关不看里面，原样交给推送服务）
fn note() -> Value {
    let doc = obj! {
        "v" => 1, "id" => crypto::token_hex(8), "sid" => crypto::token_hex(16), "at" => 1_794_000_000,
        "title" => "Claude Code 做完了", "subtitle" => "工作 · work", "body" => "改好了 两处",
    };
    spec::note_seal(&crypto::random(32), &crypto::token_hex(16), &doc)
}

/// Team 的服务器收的最长的那种（密文 3072 个 base64 字符：服务端的 PUSH_BLOB）
fn longest_note() -> Value {
    obj! { "n" => crypto::b64(&[7; 16]), "c" => "A".repeat(3072), "t" => crypto::b64(&[9; 32]) }
}

/// 交一条 tail 结尾的令牌的通知，要它成：返回令牌
fn push_ok(gw: &Gateway, tail: &str, env: &str) -> String {
    let token = Gateway::token(tail);
    assert_eq!(gw.post(&Gateway::body(&token, env, &crypto::token_hex(8), &note())), (200, ok()), "{tail}");
    token
}

/// 推送服务收到的、这个令牌的那几个
fn of(seen: &[Seen], token: &str) -> Vec<Seen> {
    seen.iter().filter(|s| s.token() == token).cloned().collect()
}

/// 两个假推送服务都没挑出 HTTP/2 上的毛病
#[track_caller]
fn clean(gw: &Gateway) {
    assert_eq!((gw.production.complaints(), gw.sandbox.complaints()), (Vec::<String>::new(), Vec::<String>::new()));
}

/// 跑一次 `cca-core team-push 参数…`（会自己退出的那种）：(退出码, stdout, stderr)
fn run(args: &[&str]) -> (i32, String, String) {
    let helper = harness::helper();
    let mut cmd = Cmd::new(&helper);
    cmd.arg("team-push").args(args).env("LC_ALL", "en_US.UTF-8").timeout(Duration::from_secs(60));
    let out = cmd.output().unwrap_or_else(|e| panic!("{} 运行不了：{e}", helper.display()));
    (out.status.py(), out.out(), out.err())
}

fn text(p: &Path) -> String {
    p.to_str().unwrap_or_else(|| panic!("路径不是 UTF-8：{}", p.display())).to_string()
}

// ---- 用例 ----

/// --print-pin 和 openssl 算的一样（Team 的服务器的 --push-pin 填它）；health、不认的路径和方法；开头那一句说了什么
fn pin_health_and_paths(t: &mut Ctx) {
    let d = t.tmp("cca-push-");
    let gw = Gateway::start(&d, Knobs::default());
    let cert = text(&d.join("gw").join("cert.pem"));
    assert_eq!(run(&["--cert", &cert, "--print-pin"]), (0, format!("{}\n", gw.pin), String::new()));
    assert_eq!(gw.get("/v1/health"), (200, ok()));
    assert_eq!(gw.get("/v1/push"), (405, error("method")));
    assert_eq!(gw.get("/v1/nope"), (404, error("not_found")));
    let said = gw.said();
    assert!(said.contains(&format!("证书的公钥 {}（Team 的服务器的 --push-pin 填它）", gw.pin)), "{said}");
    assert!(said.contains("App com.cca.remote，密钥 KEYID12345（Team ID TEAMID1234）"), "{said}");
    assert!(!said.contains("PRIVATE KEY") && !said.contains("MIGTAgEA"), "不说密钥的内容：{said}");
    assert!(gw.production.seen().is_empty() && gw.sandbox.seen().is_empty());
}

/// 交给推送服务的：头（照先后）、ES256 的令牌（假推送服务拿 .p8 的公钥验过）、内容（解不开时显示的两句、要通知扩展来改、
/// 原样的密文）；开发环境的交给另一个；令牌接着用；最长的通知也收得下
fn what_the_push_service_gets(t: &mut Ctx) {
    let d = t.tmp("cca-push-");
    let gw = Gateway::start(&d, Knobs::default());
    let (token, id, blob) = (Gateway::token("aa"), crypto::token_hex(8), note());
    let before = now();
    assert_eq!(gw.post(&Gateway::body(&token, "production", &id, &blob)), (200, ok()));
    let seen = gw.production.seen();
    assert_eq!((seen.len(), gw.sandbox.seen().len()), (1, 0));
    let s = &seen[0];
    let names: Vec<&str> = s.headers.iter().map(|(n, _)| n.as_str()).collect();
    let want = [
        ":method",
        ":scheme",
        ":authority",
        ":path",
        "authorization",
        "apns-push-type",
        "apns-topic",
        "apns-priority",
        "apns-expiration",
        "apns-collapse-id",
    ];
    assert_eq!(names, want);
    let authority = format!("127.0.0.1:{}", gw.production.port);
    let path = format!("/3/device/{token}");
    assert_eq!(
        [":method", ":scheme", ":authority", ":path", "apns-push-type", "apns-topic", "apns-priority", "apns-collapse-id"]
            .map(|n| s.header(n)),
        ["POST", "https", authority.as_str(), path.as_str(), "alert", "com.cca.remote", "10", id.as_str()]
    );
    let expires: f64 = s.header("apns-expiration").parse().unwrap_or_else(|_| panic!("{}", s.header("apns-expiration")));
    assert!((before + 43_200.0 - 2.0..now() + 43_200.0 + 2.0).contains(&expires), "12 小时后过期：{expires}");
    let (head, claims) = apnsfake::jwt_parts(s.header("authorization"));
    assert_eq!(head, obj! { "alg" => "ES256", "kid" => "KEYID12345" });
    assert_eq!(claims["iss"], Value::from("TEAMID1234"));
    assert!((before - 2.0..now() + 2.0).contains(&claims["iat"].f64()), "{claims}");
    assert_eq!((s.status, s.reason.as_str()), (200, ""), "假推送服务验过了令牌");
    let aps = obj! {
        "alert" => obj! { "title" => "cca", "body" => "远程会话里有新的通知" }, "mutable-content" => 1, "sound" => "default",
    };
    assert_eq!(s.json(), obj! { "aps" => aps, "n" => blob.clone() });
    for word in ["改好了", "Claude Code", "工作"] {
        assert!(!String::from_utf8_lossy(&s.body).contains(word), "推送服务看得到通知里的字：{word}");
    }
    // 开发环境的交给 --apns-sandbox；同一个令牌（JWT）接着用（45 分钟）
    push_ok(&gw, "aa", "sandbox");
    let s2 = gw.sandbox.seen().remove(0);
    assert_eq!(s2.header(":authority"), format!("127.0.0.1:{}", gw.sandbox.port));
    assert_eq!(s2.header("authorization"), s.header("authorization"));
    // 最长的通知：推送服务一条最多 4096 个字节，收得下
    let token = Gateway::token("aa");
    assert_eq!(gw.post(&Gateway::body(&token, "production", &crypto::token_hex(8), &longest_note())), (200, ok()));
    let long = of(&gw.production.seen(), &token).remove(0);
    assert!(long.body.len() <= apnsfake::PAYLOAD_MAX, "{} 个字节", long.body.len());
    assert_eq!(long.json()["n"], longest_note());
    clean(&gw);
}

/// 推送服务回的各种结果交回 Team 的服务器的样子；过期的令牌、没处理的流、503 再试一次；日志里说了原因、不带令牌
fn replies_and_retries(t: &mut Ctx) {
    let d = t.tmp("cca-push-");
    let gw = Gateway::start(&d, Knobs::default());
    // (令牌的最后两个字符, 网关回的状态码和 error, 推送服务收到的这个令牌的几次和回的)
    let cases: [(&str, u16, &str, &[(u16, &str)]); 6] = [
        ("bb", 410, "gone", &[(410, "Unregistered")]),
        ("cc", 410, "gone", &[(400, "BadDeviceToken")]),
        ("ee", 429, "busy", &[(429, "TooManyRequests")]),
        ("dd", 200, "", &[(403, "ExpiredProviderToken"), (200, "")]),
        ("66", 200, "", &[(0, "REFUSED_STREAM"), (200, "")]),
        ("ff", 502, "upstream", &[(503, "ServiceUnavailable"), (503, "ServiceUnavailable")]),
    ];
    let mut tokens = Vec::new();
    for (tail, status, code, upstream) in cases {
        let token = Gateway::token(tail);
        let want = if status == 200 { ok() } else { error(code) };
        assert_eq!(gw.post(&Gateway::body(&token, "production", &crypto::token_hex(8), &note())), (status, want), "{tail}");
        let got: Vec<(u16, String)> = of(&gw.production.seen(), &token).iter().map(|s| (s.status, s.reason.clone())).collect();
        let want: Vec<(u16, String)> = upstream.iter().map(|(s, r)| (*s, r.to_string())).collect();
        assert_eq!(got, want, "{tail}");
        tokens.push(token);
    }
    let seen = gw.production.seen();
    // 过期了重发的还是那一个令牌：才签了不到一分钟，换了也一样（苹果还限着多久换一次）；同一条连接
    let dd = of(&seen, &tokens[3]);
    assert_eq!((dd[0].header("authorization"), dd[0].conn), (dd[1].header("authorization"), dd[1].conn));
    // 没处理（REFUSED_STREAM）、503：换一条连接再发
    for token in [&tokens[4], &tokens[5]] {
        let two = of(&seen, token);
        assert_ne!(two[0].conn, two[1].conn, "{token}");
    }
    let said = gw.said();
    assert!(said.contains("cca-push: 推送服务（production）没收下：503 ServiceUnavailable"), "{said}");
    for token in &tokens {
        assert!(!said.contains(&token[..16]), "日志里有令牌：{said}");
    }
    clean(&gw);
}

/// 一条连接接着用（流的编号 1、3、5）；对面说了 GOAWAY 以后不再用它；连接断了换一条再发
fn connections_are_reused_and_replaced(t: &mut Ctx) {
    let d = t.tmp("cca-push-");
    let gw = Gateway::start(&d, Knobs::default());
    let first: Vec<String> = (0..3).map(|_| push_ok(&gw, "aa", "production")).collect();
    let seen = gw.production.seen();
    let used: Vec<(usize, u32)> = first.iter().map(|t| of(&seen, t).remove(0)).map(|s| (s.conn, s.stream)).collect();
    assert_eq!(used, [(1, 1), (1, 3), (1, 5)]);
    let conn_of = |token: &str| -> Vec<usize> { of(&gw.production.seen(), token).iter().map(|s| s.conn).collect() };
    let goaway = push_ok(&gw, "77", "production");
    assert_eq!(conn_of(&goaway), [1]);
    let next = push_ok(&gw, "aa", "production");
    assert_eq!(conn_of(&next), [2], "说了 GOAWAY 的连接不再用");
    let cut = push_ok(&gw, "88", "production");
    assert_eq!(conn_of(&cut), [2]);
    std::thread::sleep(Duration::from_millis(300));
    let after = push_ok(&gw, "aa", "production");
    assert_eq!(conn_of(&after), [3], "断了的连接：换一条再发");
    assert_eq!(gw.production.connections(), 3);
    clean(&gw);
}

/// 推送服务给的窗口很小（100 个字节）：网关照窗口一段段地发、等它的 WINDOW_UPDATE，内容一个字节不差。头一个请求在网关
/// 收到推送服务的 SETTINGS 以前就发出去了（照默认的 65535），第二个起才照 100
fn flow_control(t: &mut Ctx) {
    let d = t.tmp("cca-push-");
    let gw = Gateway::start(&d, Knobs { window: 100 });
    for _ in 0..2 {
        let body = Gateway::body(&Gateway::token("aa"), "production", &crypto::token_hex(8), &longest_note());
        assert_eq!(gw.post(&body), (200, ok()));
    }
    let seen = gw.production.seen();
    assert_eq!((seen.len(), seen[0].conn, seen[1].conn), (2, 1, 1));
    assert!(seen[1].data_frames >= seen[1].body.len() / 100, "分了 {} 段（{} 个字节）", seen[1].data_frames, seen[1].body.len());
    assert_eq!(seen[1].json()["n"], longest_note());
    clean(&gw);
}

/// 不收的请求（400、411、413）不交给推送服务；同一个令牌攒得下 30 条，多了 429，别的手机不受影响
fn bad_requests_and_limits(t: &mut Ctx) {
    let d = t.tmp("cca-push-");
    let gw = Gateway::start(&d, Knobs::default());
    let (token, id, blob) = (Gateway::token("aa"), crypto::token_hex(8), note());
    let too_long = obj! { "n" => crypto::b64(&[7; 16]), "c" => "A".repeat(3076), "t" => crypto::b64(&[9; 32]) };
    let bad: Vec<Vec<u8>> = vec![
        Gateway::body(&token, "production", &id, &blob).iter().map(|b| if *b == b'1' { b'2' } else { *b }).collect(),
        Gateway::body(&token.to_uppercase(), "production", &id, &blob),
        Gateway::body(&token[..62], "production", &id, &blob),
        Gateway::body(&token, "test", &id, &blob),
        Gateway::body(&token, "production", "0123", &blob),
        Gateway::body(&token, "production", &id, &Value::from("x")),
        Gateway::body(&token, "production", &id, &too_long),
        b"{".to_vec(),
        b"\xff\xfe".to_vec(),
    ];
    for body in &bad {
        assert_eq!(gw.post(body), (400, error("bad")), "{}", String::from_utf8_lossy(body));
    }
    let host = format!("Host: 127.0.0.1:{}\r\n", gw.port);
    assert_eq!(gw.raw(format!("POST /v1/push HTTP/1.1\r\n{host}Connection: close\r\n\r\n").as_bytes()), (411, error("length")));
    let big = format!("POST /v1/push HTTP/1.1\r\n{host}Content-Length: 9000\r\nConnection: close\r\n\r\n");
    assert_eq!(gw.raw(big.as_bytes()), (413, error("too_big")), "不等它发完就回");
    assert!(gw.production.seen().is_empty(), "不收的都没交给推送服务");
    let burst: Vec<u16> = (0..31).map(|_| gw.post(&Gateway::body(&token, "production", &crypto::token_hex(8), &blob)).0).collect();
    assert_eq!((&burst[..30], burst[30]), (&[200u16; 30][..], 429));
    push_ok(&gw, "aa", "production");
    assert_eq!(gw.production.seen().len(), 31, "超了的那一条没交给推送服务");
    clean(&gw);
}

/// 命令行写错了是 2、说怎么用；.p8 读不了、不是那种私钥、端口被占了是 1、说清楚（不说密钥的内容）；.p8 别人也读得了说一句、照样起来
fn arguments_and_startup(t: &mut Ctx) {
    let d = t.tmp("cca-push-");
    pki::make_cert(&d);
    let p8 = d.join("AuthKey_KEYID12345.p8");
    apnsfake::auth_key(&p8);
    let (cert, key, p8s) = (text(&d.join("cert.pem")), text(&d.join("key.pem")), text(&p8));
    let base = |extra: &[&str]| -> Vec<String> {
        let mut v: Vec<String> =
            ["--cert", &cert, "--key", &key, "--apns-key", &p8s, "--key-id", "KEYID12345", "--team-id", "TEAMID1234"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        v.extend(["--listen", "127.0.0.1:0"].map(String::from));
        v.extend(extra.iter().map(|s| s.to_string()));
        v
    };
    let run_v = |v: Vec<String>| run(&v.iter().map(String::as_str).collect::<Vec<_>>());
    let (code, out, err) = run(&[]);
    assert_eq!((code, out.as_str()), (2, ""));
    assert!(err.contains("usage: cca-core team-push") && err.contains("cca-core team-push: error: 要给 --cert"), "{err}");
    let (code, out, _) = run(&["-h"]);
    assert_eq!(code, 0);
    assert!(out.contains("推送网关") && out.contains("--apns-key APNS_KEY"), "{out}");
    let (code, _, err) = run_v(base(&["--key-id", "keyid12345"]));
    assert_eq!(code, 2);
    assert!(err.contains("--key-id、--team-id 是 10 个大写字母、数字"), "{err}");
    let (code, _, err) = run_v(base(&["--apns-key", &text(&d.join("missing.p8"))]));
    assert_eq!(code, 1);
    assert!(err.contains("cca-push: 起不来：推送服务的密钥") && err.contains("读不了"), "{err}");
    // 不是 PKCS#8 的（openssl ecparam 造的那种 EC PRIVATE KEY）
    let sec1 = d.join("sec1.pem");
    fsx::write_mode(&sec1, "-----BEGIN EC PRIVATE KEY-----\nMHcCAQEEIA==\n-----END EC PRIVATE KEY-----\n", 0o600);
    let (code, _, err) = run_v(base(&["--apns-key", &text(&sec1)]));
    assert_eq!(code, 1);
    assert!(err.contains("推送服务的密钥") && !err.contains("MHcCAQEEIA"), "{err}");
    // 端口被占了
    let busy = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("{e}"));
    let port = busy.local_addr().unwrap_or_else(|e| panic!("{e}")).port();
    let (code, _, err) = run_v(base(&["--listen", &format!("127.0.0.1:{port}")]));
    assert_eq!(code, 1);
    assert!(err.contains("听不了"), "{err}");
    drop(busy);
    // .p8 别的用户也读得了：说一句，照样起来
    fsx::chmod(&p8, 0o644);
    let port_file = d.join("port");
    let v = base(&["--port-file", &text(&port_file)]);
    let mut proc = fleet::ServerProc::spawn_with(kit::server::push_argv(), &v, &[]);
    proc.wait_file(&port_file, "推送网关起来", Duration::from_secs(30));
    let said = proc.stop();
    assert!(said.contains(&format!("cca-push: {p8s} 别的用户也读得了：chmod 600 {p8s}")), "{said}");
}
