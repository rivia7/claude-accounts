//! handler 的单元测试，和 front、cli 的测试也用的几样：造签名的 `Signer`、放在临时目录里的一份服务（`Bench`）、拼请求的几个函数。
//!
//! 请求行、请求头、Content-Length、签名那个头的各种写法和真的 Python 服务器逐字节对的那一份在 front.rs 的测试里
//! （tests/vectors/server_http.json）；这里是验过签名以后的那几步，和各步的先后。

use super::*;
use crate::py::fs::TempDir;
use crate::py::hash;
use crate::py::json::Fmt;
use crate::server::limits::{Guards, Limits};
use crate::server::store::tests::said;
use crate::server::store::{Store, StoreCfg};
use crate::server::web::Web;
use ring::signature::{Ed25519KeyPair, KeyPair};
use std::cell::Cell;
use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::Arc;

/// 测试里服务器的时间
pub(in crate::server) const NOW: f64 = 1_800_000_000.25;
/// 测试里连过来的地址
pub(in crate::server) const PEER: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7));

fn put(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u32).to_be_bytes());
    out.extend_from_slice(s);
}

/// 一把测试用的 Ed25519 密钥（种子是 32 个 seed）：公钥那一行、指纹，给请求签名
pub(in crate::server) struct Signer {
    pair: Ed25519KeyPair,
    blob: Vec<u8>,
    /// `ssh-ed25519 AAAA…`
    pub line: String,
    /// `SHA256:…`
    pub fp: String,
}

impl Signer {
    pub fn new(seed: u8) -> Signer {
        let pair = Ed25519KeyPair::from_seed_unchecked(&[seed; 32]).unwrap();
        let mut blob = Vec::new();
        put(&mut blob, b"ssh-ed25519");
        put(&mut blob, pair.public_key().as_ref());
        Signer { line: format!("ssh-ed25519 {}", base64::b64encode(&blob)), fp: sshsig::key_fp(&blob), blob, pair }
    }

    /// `ssh-keygen -Y sign -n 命名空间` 写出来的那个样子（SSHSIG，sha512）
    pub fn armored(&self, data: &[u8], ns: &str) -> String {
        let digest = ring::digest::digest(&ring::digest::SHA512, data);
        let mut signed = b"SSHSIG".to_vec();
        for part in [ns.as_bytes(), b"", b"sha512", digest.as_ref()] {
            put(&mut signed, part);
        }
        let mut sig = Vec::new();
        put(&mut sig, b"ssh-ed25519");
        put(&mut sig, self.pair.sign(&signed).as_ref());
        let mut out = b"SSHSIG".to_vec();
        out.extend_from_slice(&1u32.to_be_bytes());
        for part in [&self.blob[..], ns.as_bytes(), b"", b"sha512", &sig] {
            put(&mut out, part);
        }
        let b64 = base64::b64encode(&out);
        let lines: Vec<&str> = b64.as_bytes().chunks(70).map(|c| std::str::from_utf8(c).unwrap()).collect();
        format!("-----BEGIN SSH SIGNATURE-----\n{}\n-----END SSH SIGNATURE-----\n", lines.join("\n"))
    }

    /// X-Cca-Sig 的值：签名再包一层 base64
    pub fn sign(&self, body: &[u8]) -> String {
        base64::b64encode(self.armored(body, NS_REPORT).as_bytes())
    }
}

/// 临时目录里的一份服务：根密钥是 `root`，没有设备，不开网页端
pub(in crate::server) struct Bench {
    pub dir: TempDir,
    pub ctx: Arc<Ctx>,
    pub root: Signer,
    count: Cell<u64>,
}

impl Bench {
    pub fn new() -> Bench {
        Bench::with(false)
    }

    /// relay：带不带中转记着的那一份（`Ctx.remote`；数据端口不起）
    pub fn with(relay: bool) -> Bench {
        said();
        let dir = TempDir::new().unwrap();
        let root = Signer::new(200);
        let cfg = StoreCfg {
            path: format!("{}/state.json", dir.path).into(),
            ttl: 150.0,
            idle: 600.0,
            retain: 30.0 * 86400.0,
            keys_path: None,
            cost_path: None,
            cost_keep: 100.0 * 86400.0,
        };
        let store = Store::open(&cfg, &root.line, Limits::DEFAULT, NOW).unwrap();
        let remote = relay.then(|| {
            let path = format!("{}/remote.json", dir.path);
            Arc::new(remote::Shared::new(remote::Remote::open(Path::new(&path), 18444, None, Limits::DEFAULT)))
        });
        let ctx = Ctx { store: Mutex::new(store), remote, relay: None, web: None, guards: Mutex::new(Guards::new()), sig_ok: true };
        Bench { dir, ctx: Arc::new(ctx), root, count: Cell::new(0) }
    }

    fn tick(&self) -> u64 {
        self.count.set(self.count.get() + 1);
        self.count.get()
    }

    /// 一个没用过的 nonce
    pub fn nonce(&self) -> String {
        format!("{:032x}", self.tick())
    }

    /// 一个请求的正文：v、op、key、ts（就是 NOW）、nonce，后面接 more（`,"units":{}` 这样的）
    pub fn body(&self, op: &str, who: &Signer, more: &str) -> String {
        format!(r#"{{"v":1,"op":"{op}","key":"{}","ts":{NOW},"nonce":"{}"{more}}}"#, who.fp, self.nonce())
    }

    /// 处理一个请求：服务器的时间是 now，单调时钟是 mono
    pub fn run_at(&self, req: &Request, now: f64, mono: f64) -> (u16, String) {
        shown(match admit_at(&self.ctx, PEER, false, req, mono) {
            Ok(n) => answer_at(&self.ctx, PEER, false, req, n, now, mono),
            Err(out) => out,
        })
    }

    /// 单调时钟：每问一次往后走 2 秒（各个桶都攒得回来：连着发多少个都不会被限流）
    pub fn mono(&self) -> f64 {
        2.0 * self.tick() as f64
    }

    /// 处理一个请求，服务器的时间是 NOW
    pub fn run(&self, req: &Request) -> (u16, String) {
        self.run_at(req, NOW, self.mono())
    }

    /// who 签名发一个 op
    pub fn send(&self, op: &str, who: &Signer, more: &str) -> (u16, String) {
        let body = self.body(op, who, more);
        self.run(&signed(op, who, &body))
    }

    /// 管理员登记一台设备
    pub fn enroll(&self, who: &Signer, dev: &str, name: &str) {
        let more = format!(r#","devices":{{"{dev}":{{"pub":"{}","name":"{name}"}}}}"#, who.line);
        let got = self.send("team_enroll", &self.root, &more);
        assert_eq!(got.0, 200, "{got:?}");
    }

    /// 管理员看到的那一台设备的 seen_age（没来过是 null）
    fn seen_age(&self, who: &Signer) -> V {
        let (code, text) = self.send("team_members", &self.root, "");
        assert_eq!(code, 200, "{text}");
        let rows = json::loads(&text).unwrap().as_dict().unwrap().list("members").unwrap().items();
        let row = rows.iter().map(|r| r.as_dict().unwrap()).find(|r| r.str("fp").as_deref() == Some(&who.fp)).unwrap();
        row.get("seen_age").unwrap()
    }
}

/// 结果写成好比的样子：(状态码, JSON 文字)；不回就断开是 (0, "")
pub(in crate::server) fn shown(out: Outcome) -> (u16, String) {
    match out {
        Outcome::Json(code, v) => (code, json::dumps(&v, Fmt::new().utf8().compact())),
        Outcome::Raw { code, headers, body } => (code, format!("{headers:?} {}", String::from_utf8_lossy(&body))),
        Outcome::Upgrade { accept, ip } => (101, format!("{accept} {ip}")),
        Outcome::Close => (0, String::new()),
    }
}

pub(in crate::server) fn request(method: Method, target: &str, headers: &[(&str, &str)], body: &[u8]) -> Request {
    let headers = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    Request { method, target: target.to_string(), headers, body: body.to_vec() }
}

/// POST /v1/<op>：Content-Length 是正文的长度，签名是 who 的
pub(in crate::server) fn signed(op: &str, who: &Signer, body: &str) -> Request {
    let (n, sig) = (body.len().to_string(), who.sign(body.as_bytes()));
    request(Method::Post, &format!("/v1/{op}"), &[("Content-Length", &n), ("X-Cca-Sig", &sig)], body.as_bytes())
}

fn bad() -> (u16, String) {
    (400, r#"{"error":"bad"}"#.into())
}

fn denied() -> (u16, String) {
    (403, r#"{"error":"denied"}"#.into())
}

fn rate() -> (u16, String) {
    (429, r#"{"error":"rate"}"#.into())
}

fn dev(n: u8) -> String {
    format!("{n:02x}").repeat(8)
}

/// Content-Length：Python 的 int() 认什么、不认什么（每一条都在真的 Python 上试过）
#[test]
fn content_length_is_read_like_python_int() {
    use Length::{Bytes, Invalid, Negative};
    let nel_nbsp = "\u{85}12\u{a0}";
    for (text, want) in [
        ("0", Bytes(0)),
        ("12", Bytes(12)),
        ("00", Bytes(0)),
        ("+0012", Bytes(12)),
        ("-0", Bytes(0)),
        ("0_0", Bytes(0)),
        ("1_2_3", Bytes(123)),
        (" 12 ", Bytes(12)),
        ("\t12\n", Bytes(12)),
        ("\x0b12\x0c\r", Bytes(12)),
        (nel_nbsp, Bytes(12)),
        ("\u{a0}+1_2\u{85}", Bytes(12)),
        ("65536", Bytes(65536)),
        ("99999999999999999999999999999999", Bytes(usize::MAX)),
        ("-1", Negative),
        (" -12 ", Negative),
        ("-99999999999999999999999999999999", Negative),
        ("", Invalid),
        (" ", Invalid),
        ("+", Invalid),
        ("-", Invalid),
        ("1 2", Invalid),
        ("+-1", Invalid),
        ("--1", Invalid),
        ("+ 1", Invalid),
        ("- 1", Invalid),
        ("_1", Invalid),
        ("1_", Invalid),
        ("1__2", Invalid),
        ("1e3", Invalid),
        ("1.0", Invalid),
        ("0x2", Invalid),
        ("0b1", Invalid),
        ("abc", Invalid),
        ("\x1c12", Invalid),
        ("12\x1f", Invalid),
        ("1\u{0}", Invalid),
        ("\u{b2}", Invalid),
        ("1\u{a0}2", Invalid),
    ] {
        assert_eq!(content_length(text), want, "{text:?}");
    }
    // 数字多于 4300 个不认（开头的 0、下划线隔开的也算数字）
    let digits = |n: usize| "1".repeat(n);
    assert_eq!(content_length(&digits(4300)), Bytes(usize::MAX));
    assert_eq!(content_length(&digits(4301)), Invalid);
    assert_eq!(content_length(&"0".repeat(4300)), Bytes(0));
    assert_eq!(content_length(&"0".repeat(4301)), Invalid);
    assert_eq!(content_length(&format!("{}1", "1_".repeat(4299))), Bytes(usize::MAX));
    assert_eq!(content_length(&format!("{}1", "1_".repeat(4300))), Invalid);
    assert_eq!(content_length(&format!(" -{} ", digits(4300))), Negative);
}

/// 按哪个 IP 限流：平时是连过来的地址；--behind-proxy 时是 X-Forwarded-For 最后一项，没有、不像 IP 时还是连过来的地址
#[test]
fn client_ip_trusts_only_the_last_forwarded_item() {
    let get = |headers: &[(&str, &str)]| request(Method::Get, "/v1/health", headers, b"");
    let ip = |proxied: bool, headers: &[(&str, &str)]| client_ip(PEER, proxied, &get(headers));
    assert_eq!(ip(false, &[]), "192.0.2.7");
    assert_eq!(ip(false, &[("X-Forwarded-For", "198.51.100.1")]), "192.0.2.7", "不在代理后面：这个头谁都能写，不看");
    assert_eq!(ip(true, &[]), "192.0.2.7");
    assert_eq!(ip(true, &[("X-Forwarded-For", "198.51.100.1")]), "198.51.100.1");
    assert_eq!(ip(true, &[("x-forwarded-for", "203.0.113.9, 198.51.100.1 ")]), "198.51.100.1", "前面的是客户端自己写的");
    assert_eq!(
        ip(true, &[("X-Forwarded-For", "198.51.100.1"), ("X-Forwarded-For", "203.0.113.9,\t198.51.100.2")]),
        "198.51.100.2",
        "有几个这个头：接起来看最后一项"
    );
    assert_eq!(ip(true, &[("X-Forwarded-For", "2001:DB8::0:1")]), "2001:db8::1", "写成 Python 的 ipaddress 给的样子");
    for junk in ["", "unknown", "198.51.100.1,", "198.51.100.300", "198.51.100.1:443", "[2001:db8::1]"] {
        assert_eq!(ip(true, &[("X-Forwarded-For", junk)]), "192.0.2.7", "{junk:?}");
    }
    // 听 IPv6 时连过来的 IPv4 地址
    let mapped: IpAddr = "::ffff:192.0.2.7".parse().unwrap();
    assert_eq!(client_ip(mapped, false, &get(&[])), "192.0.2.7");
    assert_eq!(client_ip("2001:db8::7".parse().unwrap(), false, &get(&[])), "2001:db8::7");
}

/// GET：/v1/health（后面可以带 ?…），别的在没开网页端时都是 404
#[test]
fn get_health_and_not_found() {
    let b = Bench::new();
    let get = |target: &str| b.run(&request(Method::Get, target, &[], b""));
    let health = get("/v1/health");
    let team = sshsig::team_id(&sshsig::key_blob(&b.root.line).unwrap());
    assert_eq!(
        health,
        (
            200,
            format!(
                r#"{{"v":1,"ok":true,"sig_ok":true,"machines":0,"seq":0,"mode":"team","ops":["report","view","keys","ledger","team_enroll","team_invite","team_put","team_members","team_revoke","team_join","team_get","team_leave"],"team":"{team}"}}"#
            )
        )
    );
    assert_eq!(get("/v1/health?x=1&y"), health);
    for target in [
        "/",
        "/index.html",
        "/v1/health/",
        "/v1/healthz",
        "/v1/relay/ws?t=00000000000000000000000000000000",
        "/v1/report",
        "",
        "?",
        "health",
    ] {
        assert_eq!(get(target), (404, r#"{"error":"not_found"}"#.into()), "{target:?}");
    }
    // 开着中转：ops 里多出中转的那几个，带着数据端口
    let relayed = Bench::with(true);
    let text = relayed.run(&request(Method::Get, "/v1/health", &[], b"")).1;
    assert!(
        text.contains(r#""team_leave","pair_open","pair_put""#) && text.ends_with(r#","relay":{"port":18444,"guests":true}}"#),
        "{text}"
    );
    // GET 不扣限流的配额（health 是给监控、安装脚本一直问的）
    for _ in 0..300 {
        assert_eq!(b.run_at(&request(Method::Get, "/v1/health", &[], b""), NOW, 0.0).0, 200);
    }
}

/// POST 验签名之前的那几步，照 Python 的先后：411、413、400（正文、签名那个头）、404、400（JSON、v、op、key）
#[test]
fn post_checks_before_the_signature() {
    let b = Bench::new();
    let a = Signer::new(1);
    let body = b.body("report", &a, r#","units":{}"#);
    let sig = a.sign(body.as_bytes());
    let n = body.len().to_string();
    let post = |target: &str, headers: &[(&str, &str)], body: &[u8]| b.run(&request(Method::Post, target, headers, body));
    let unknown = (403, format!(r#"{{"error":"unknown_device","gen":"{}"}}"#, lock(&b.ctx.store).gen()));

    // 全对的请求走到验签名那一步：这把密钥不在名单里
    assert_eq!(post("/v1/report", &[("Content-Length", &n), ("X-Cca-Sig", &sig)], body.as_bytes()), unknown);
    // Content-Length
    assert_eq!(post("/v1/report", &[("X-Cca-Sig", &sig)], body.as_bytes()), (411, r#"{"error":"bad"}"#.into()));
    assert_eq!(post("/v1/report", &[("Content-Length", "abc"), ("X-Cca-Sig", &sig)], body.as_bytes()).0, 411);
    assert_eq!(
        post("/v1/report", &[("Content-Length", "-1"), ("X-Cca-Sig", &sig)], body.as_bytes()),
        (413, r#"{"error":"bad"}"#.into())
    );
    let big = vec![b' '; MAX_BODY + 1];
    for op in ["report", "view", "team_enroll", "team_join", "nope"] {
        assert_eq!(post(&format!("/v1/{op}"), &[("Content-Length", "65537"), ("X-Cca-Sig", &sig)], &big).0, 413, "{op}");
    }
    for op in ["keys", "ledger", "team_put"] {
        // 交换密文的、存配置的：上限是 1 MiB，65537 个字节过得了这一步（正文不是 JSON：400）
        assert_eq!(post(&format!("/v1/{op}"), &[("Content-Length", "65537"), ("X-Cca-Sig", &sig)], &big), bad(), "{op}");
        assert_eq!(post(&format!("/v1/{op}"), &[("Content-Length", "1048577"), ("X-Cca-Sig", &sig)], &big).0, 413, "{op}");
    }
    // 正文没读全；比 Content-Length 说的长时只看前面那么多
    assert_eq!(
        post("/v1/report", &[("Content-Length", &(body.len() + 1).to_string()), ("X-Cca-Sig", &sig)], body.as_bytes()),
        bad()
    );
    let longer = format!("{body}   ");
    assert_eq!(post("/v1/report", &[("Content-Length", &n), ("X-Cca-Sig", &sig)], longer.as_bytes()), unknown);
    // 签名那个头
    assert_eq!(post("/v1/report", &[("Content-Length", &n)], body.as_bytes()), bad());
    for sig in ["", "!!!!", "QUJ", &format!("{sig} "), &base64::b64encode(&vec![b'x'; MAX_SIG + 1])] {
        assert_eq!(post("/v1/report", &[("Content-Length", &n), ("X-Cca-Sig", sig)], body.as_bytes()), bad(), "{sig:?}");
    }
    // 这些 400 排在“不认识的 op”前面
    assert_eq!(post("/v1/nope", &[("Content-Length", &n)], body.as_bytes()), bad());
    // 不认识的 op；中转的那几个没开中转时也不认
    for target in
        ["/v1/nope", "/v1/machines", "/v1/", "/v1", "/report", "/v1/report?x=1", "/v1/pair_open", "/v1/relay_list", "/v2/report"]
    {
        let got = post(target, &[("Content-Length", &n), ("X-Cca-Sig", &sig)], body.as_bytes());
        assert_eq!(got, (404, r#"{"error":"not_found"}"#.into()), "{target}");
    }
    // 正文
    let with =
        |body: &str| post("/v1/report", &[("Content-Length", &body.len().to_string()), ("X-Cca-Sig", &sig)], body.as_bytes());
    let fp = &a.fp;
    for body in [
        String::new(),
        "{".into(),
        "[]".into(),
        "null".into(),
        format!(r#"{{"v":2,"op":"report","key":"{fp}"}}"#),
        format!(r#"{{"v":"1","op":"report","key":"{fp}"}}"#),
        format!(r#"{{"op":"report","key":"{fp}"}}"#),
        format!(r#"{{"v":1,"op":"view","key":"{fp}"}}"#),
        format!(r#"{{"v":1,"op":7,"key":"{fp}"}}"#),
        format!(r#"{{"v":1,"key":"{fp}"}}"#),
        r#"{"v":1,"op":"report"}"#.into(),
        r#"{"v":1,"op":"report","key":7}"#.into(),
        r#"{"v":1,"op":"report","key":"SHA256:short"}"#.into(),
        format!(r#"{{"v":1,"op":"report","key":"{}"}}"#, &fp[7..]),
        format!("\u{feff}{{\"v\":1,\"op\":\"report\",\"key\":\"{fp}\"}}"),
    ] {
        assert_eq!(with(&body), bad(), "{body}");
    }
    let not_utf8 = [b"{\"v\":1,\"op\":\"report\",\"key\":\"".as_slice(), fp.as_bytes(), b"\",\"x\":\"\xff\"}"].concat();
    assert_eq!(post("/v1/report", &[("Content-Length", &not_utf8.len().to_string()), ("X-Cca-Sig", &sig)], &not_utf8), bad());
    // v 照 Python 的相等：1.0、true 也是 1。key 结尾带一个换行也合那个正则（Python 的 $），只是哪台设备都不是它
    for body in [
        format!(r#"{{"v":1.0,"op":"report","key":"{fp}"}}"#),
        format!(r#"{{"v":true,"op":"report","key":"{fp}"}}"#),
        format!(r#"{{"v":1,"op":"report","key":"{fp}\n"}}"#),
        format!(r#"{{"v":7,"v":1,"op":"view","op":"report","key":"{fp}"}}"#),
    ] {
        assert_eq!(with(&body), unknown, "{body}");
    }
    assert!(said().is_empty());
}

/// 签名分三种验：设备的、管理员的、加入的；不在名单里的回 unknown_device 带着 gen
#[test]
fn signatures_by_kind() {
    let b = Bench::new();
    let (a, other) = (Signer::new(1), Signer::new(2));
    let gen = lock(&b.ctx.store).gen().to_string();
    let unknown = (403, format!(r#"{{"error":"unknown_device","gen":"{gen}"}}"#));
    assert_eq!(b.send("report", &a, r#","units":{}"#), unknown);
    assert_eq!(b.send("view", &a, r#","accounts":[]"#), unknown);
    assert_eq!(b.send("team_get", &a, ""), unknown);
    assert_eq!(b.send("team_leave", &a, ""), unknown);

    // 管理员的 op：key 要是根公钥的指纹，签名要是根密钥的
    let more = format!(r#","devices":{{"{}":{{"pub":"{}","name":"书房"}}}}"#, dev(1), a.line);
    assert_eq!(b.send("team_enroll", &a, &more), denied(), "不是根密钥");
    let body = b.body("team_enroll", &b.root, &more);
    let forged = request(
        Method::Post,
        "/v1/team_enroll",
        &[("Content-Length", &body.len().to_string()), ("X-Cca-Sig", &a.sign(body.as_bytes()))],
        body.as_bytes(),
    );
    assert_eq!(b.run(&forged), denied(), "key 写的是根公钥的指纹，签名是别人的");
    assert_eq!(lock(&b.ctx.store).machines(), 0);
    let enrolled = format!(r#"{{"v":1,"ok":true,"names":{{"{}":"书房"}},"members":1,"gen":"{gen}"}}"#, dev(1));
    assert_eq!(b.send("team_enroll", &b.root, &more), (200, enrolled));

    // 设备的 op：拿名单里这台设备的公钥验
    assert_eq!(b.send("report", &a, r#","units":{}"#), (200, r#"{"v":1,"ok":true,"ttl":150.0,"idle":600.0}"#.into()));
    let body = b.body("report", &a, r#","units":{}"#);
    let sig_of = |text: &str| -> String { text.to_string() };
    let with_sig = |sig: String| {
        request(
            Method::Post,
            "/v1/report",
            &[("Content-Length", &body.len().to_string()), ("X-Cca-Sig", &sig_of(&sig))],
            body.as_bytes(),
        )
    };
    assert_eq!(b.run(&with_sig(other.sign(body.as_bytes()))), denied(), "别的密钥签的");
    assert_eq!(b.run(&with_sig(a.sign(b"another body"))), denied(), "签的是别的内容");
    assert_eq!(b.run(&with_sig(base64::b64encode(a.armored(body.as_bytes(), "cca-team").as_bytes()))), denied(), "别的命名空间");
    assert_eq!(b.run(&with_sig(base64::b64encode(b"not a signature"))), denied());
    assert_eq!(b.run(&with_sig(a.sign(body.as_bytes()))).0, 200, "同一个正文，签名对了就过（上面几次都没走到记 nonce 那一步）");
    // 设备发管理员的 op、管理员的密钥发设备的 op（根密钥不在名单里）
    assert_eq!(b.send("team_members", &a, ""), denied());
    assert_eq!(b.send("team_revoke", &a, &format!(r#","dev":"{}""#, dev(1))), denied());
    assert_eq!(b.send("report", &b.root, r#","units":{}"#), unknown);
    assert!(said().is_empty());
}

/// 验过签名以后：按指纹限流、ts、nonce，照这个先后；不在名单里的扣不到配额
#[test]
fn clock_nonce_and_rate() {
    let b = Bench::new();
    let a = Signer::new(1);
    b.enroll(&a, &dev(1), "a");
    let clock = |now: f64| (400, format!(r#"{{"error":"clock","now":{now:?}}}"#));
    let report = |ts: &str, nonce: &str| {
        let body = format!(r#"{{"v":1,"op":"report","key":"{}","ts":{ts},"nonce":{nonce},"units":{{}}}}"#, a.fp);
        signed("report", &a, &body)
    };
    let fresh = || format!("\"{}\"", b.nonce());
    // ts：是数（布尔值不算）、和服务器的时间差 300 秒以内
    for ts in ["1800000300.25", "1799999700.25", "1800000000", "1.8e9", "NaN"] {
        assert_eq!(b.run(&report(ts, &fresh())).0, 200, "{ts}");
    }
    for ts in [
        "1800000300.5",
        "1799999700",
        "0",
        "-1800000000.25",
        "true",
        "null",
        "\"1800000000\"",
        "[1800000000]",
        "Infinity",
        "-Infinity",
    ] {
        assert_eq!(b.run(&report(ts, &fresh())), clock(NOW), "{ts}");
    }
    assert_eq!(b.run(&report("123456789012345678901234567890", &fresh())), clock(NOW), "放不进 i64 的整数");
    let no_ts = format!(r#"{{"v":1,"op":"report","key":"{}","nonce":{},"units":{{}}}}"#, a.fp, fresh());
    assert_eq!(b.run(&signed("report", &a, &no_ts)), clock(NOW));
    assert_eq!(
        b.run_at(&report("1800000000.25", &fresh()), 1_800_000_301.0, b.mono()),
        clock(1_800_000_301.0),
        "now 是服务器的时间"
    );
    // nonce：32 个小写的十六进制字符，用过的不认
    for nonce in [
        "null",
        "7",
        "\"\"",
        "\"abc\"",
        "\"0123456789ABCDEF0123456789ABCDEF\"",
        "\"0123456789abcdef0123456789abcdef0\"",
        "\"0123456789abcdef0123456789abcdeg\"",
    ] {
        assert_eq!(b.run(&report("1800000000.25", nonce)), bad(), "{nonce}");
    }
    let once = fresh();
    assert_eq!(b.run(&report("1800000000.25", &once)).0, 200);
    assert_eq!(b.run(&report("1800000000.25", &once)), (409, r#"{"error":"replay"}"#.into()));
    assert_eq!(b.run(&report("1800000001", &once)), (409, r#"{"error":"replay"}"#.into()), "正文不一样、nonce 一样也不行");
    // 结尾带一个换行的 nonce 合那个正则（Python 的 $），算另一个
    let newline = format!("{}\\n\"", once.trim_end_matches('"'));
    assert_eq!(b.run(&report("1800000000.25", &newline)).0, 200);
    assert_eq!(b.run(&report("1800000000.25", &newline)).0, 409);

    // 按指纹限流：一下子最多 30 个；ts 不对的也扣（限流排在 ts 前面），签名不对的、不在名单里的不扣
    let c = Signer::new(3);
    b.enroll(&c, &dev(3), "c");
    let stranger = Signer::new(4);
    let ip_of = |i: usize| IpAddr::V4(Ipv4Addr::new(198, 51, 100, (i % 250) as u8));
    let run_from = |req: &Request, i: usize| {
        shown(match admit_at(&b.ctx, ip_of(i), false, req, 1000.0) {
            Ok(n) => answer_at(&b.ctx, ip_of(i), false, req, n, NOW, 1000.0),
            Err(out) => out,
        })
    };
    let report_by = |who: &Signer, ts: &str| {
        let body = format!(r#"{{"v":1,"op":"report","key":"{}","ts":{ts},"nonce":{},"units":{{}}}}"#, who.fp, fresh());
        signed("report", who, &body)
    };
    for i in 0..40 {
        assert_eq!(run_from(&report_by(&stranger, "1800000000.25"), i).0, 403, "不在名单里");
        let body = b.body("report", &c, r#","units":{}"#);
        let forged = request(
            Method::Post,
            "/v1/report",
            &[("Content-Length", &body.len().to_string()), ("X-Cca-Sig", &stranger.sign(body.as_bytes()))],
            body.as_bytes(),
        );
        assert_eq!(run_from(&forged, i), denied(), "别人拿这台设备的指纹乱发");
    }
    for i in 0..30 {
        let ts = if i % 2 == 0 { "1800000000.25" } else { "0" };
        assert_eq!(run_from(&report_by(&c, ts), i).0, if i % 2 == 0 { 200 } else { 400 }, "第 {i} 个");
    }
    assert_eq!(run_from(&report_by(&c, "1800000000.25"), 0), rate());
    assert_eq!(run_from(&report_by(&c, "0"), 1), rate(), "限流排在 ts 前面");
    assert_eq!(run_from(&report_by(&a, "1800000000.25"), 2).0, 200, "各台设备各算各的");
    // 管理员的 op 另算：一下子 120 个
    for i in 0..120 {
        let body = b.body("team_members", &b.root, "");
        assert_eq!(run_from(&signed("team_members", &b.root, &body), i).0, 200, "第 {i} 个");
    }
    let body = b.body("team_members", &b.root, "");
    assert_eq!(run_from(&signed("team_members", &b.root, &body), 0), rate());
    said();
}

/// 按 IP 限流在最前面（读正文之前）：一下子最多 120 个，每秒涨 2 个；GET /v1/health 不算
#[test]
fn rate_by_ip_comes_first() {
    let b = Bench::new();
    let junk = request(Method::Post, "/v1/nope", &[], b"");
    for i in 0..120 {
        assert_eq!(b.run_at(&junk, NOW, 50.0).0, 411, "第 {i} 个");
    }
    assert_eq!(b.run_at(&junk, NOW, 50.0), rate());
    assert_eq!(b.run_at(&request(Method::Get, "/v1/health", &[], b""), NOW, 50.0).0, 200);
    assert_eq!(b.run_at(&junk, NOW, 50.4), rate());
    assert_eq!(b.run_at(&junk, NOW, 50.5).0, 411, "半秒涨回来一个");
    assert_eq!(b.run_at(&junk, NOW, 50.5), rate());
    // 别的 IP 不受影响；--behind-proxy 时照 X-Forwarded-For 的最后一项分
    let other: IpAddr = "192.0.2.8".parse().unwrap();
    assert_eq!(shown(admit_at(&b.ctx, other, false, &junk, 50.5).map(|_| Outcome::Close).unwrap_or_else(|out| out)).0, 411);
    let via = |xff: &str| {
        let req = request(Method::Post, "/v1/nope", &[("X-Forwarded-For", xff)], b"");
        shown(admit_at(&b.ctx, PEER, true, &req, 50.5).map(|_| Outcome::Close).unwrap_or_else(|out| out)).0
    };
    assert_eq!(via("203.0.113.1, 192.0.2.7"), 429, "最后一项是已经用完配额的那个地址");
    assert_eq!(via("192.0.2.7, 203.0.113.1"), 411);
    assert_eq!(via("not an ip"), 429, "不像 IP：照连过来的地址");
}

/// 设备发过验过签的请求才记“来过”；管理员的请求、没加入成的不记
#[test]
fn seen_is_recorded_for_device_requests() {
    let b = Bench::new();
    let a = Signer::new(1);
    b.enroll(&a, &dev(1), "a");
    assert!(matches!(b.seen_age(&a), V::None), "登记了、还没来过");
    assert_eq!(b.send("report", &a, r#","units":7"#), bad(), "请求里的各项不合格：签名是对的，照样算来过");
    assert!(matches!(b.seen_age(&a), V::Int(0)), "{:?}", b.seen_age(&a).as_num());
    // ts 不对、nonce 用过的走不到那一步
    let c = Signer::new(3);
    b.enroll(&c, &dev(3), "c");
    let stale = format!(r#"{{"v":1,"op":"report","key":"{}","ts":5,"nonce":"{}","units":{{}}}}"#, c.fp, b.nonce());
    assert_eq!(b.run(&signed("report", &c, &stale)).0, 400);
    assert!(matches!(b.seen_age(&c), V::None));
    said();
}

/// 拿邀请加入：拿要加入的这把密钥签名；没有这个邀请和签名不对回一样的 403，签名对了才说邀请是过期了还是用过了
#[test]
fn team_join() {
    let b = Bench::new();
    let (a, c) = (Signer::new(1), Signer::new(3));
    let token = |n: u8| format!("{n:02x}").repeat(16);
    let invite = |n: u8, d: u8, ttl: i64| {
        let more = format!(r#","token":"{}","dev":"{}","ttl":{ttl}"#, hash::sha256_hex(token(n)), dev(d));
        b.send("team_invite", &b.root, &more)
    };
    let join_body = |who: &Signer, key: &Signer, n: u8, name: &str| {
        b.body("team_join", who, &format!(r#","pub":"{}","name":"{name}","token":"{}""#, key.line, token(n)))
    };
    assert_eq!(invite(1, 1, 3600).0, 200);
    // 没有这个邀请、公钥不是自己的、签名不对：一样的 403
    assert_eq!(b.run(&signed("team_join", &a, &join_body(&a, &a, 9, "a"))), denied(), "没有这个邀请");
    assert_eq!(b.run(&signed("team_join", &a, &join_body(&a, &c, 1, "a"))), denied(), "pub 不是 key 说的那把");
    assert_eq!(b.run(&signed("team_join", &c, &join_body(&a, &a, 1, "a"))), denied(), "签名不是要加入的这把密钥的");
    let no_pub = b.body("team_join", &a, &format!(r#","name":"a","token":"{}""#, token(1)));
    assert_eq!(b.run(&signed("team_join", &a, &no_pub)), denied());
    let root_body = join_body(&b.root, &b.root, 1, "root");
    assert_eq!(b.run(&signed("team_join", &b.root, &root_body)), denied(), "根密钥不能当设备加入");
    // 名字不合格：400（这时 nonce 已经记下了）
    let unnamed = join_body(&a, &a, 1, "");
    assert_eq!(b.run(&signed("team_join", &a, &unnamed)), bad());
    assert_eq!(b.run(&signed("team_join", &a, &unnamed)).0, 409);
    // 加入
    let (code, text) = b.run(&signed("team_join", &a, &join_body(&a, &a, 1, "书房")));
    assert_eq!(code, 200, "{text}");
    assert_eq!(said(), ["cca-presence: 加入了一台设备，现在 1 台"]);
    assert!(matches!(b.seen_age(&a), V::Int(0)), "加入的那一次在加入时记");
    assert_eq!(b.send("report", &a, r#","units":{}"#).0, 200);
    // 用过的、过期的邀请：签名对了才说是哪一种
    assert_eq!(b.run(&signed("team_join", &c, &join_body(&c, &c, 1, "c"))), (403, r#"{"error":"used"}"#.into()));
    assert_eq!(b.run(&signed("team_join", &a, &join_body(&c, &c, 1, "c"))), denied());
    assert_eq!(invite(2, 3, 60).0, 200);
    let late = join_body(&c, &c, 2, "c");
    let late_req = signed("team_join", &c, &late);
    assert_eq!(b.run_at(&late_req, NOW + 61.0, 5000.0), (403, r#"{"error":"expired"}"#.into()));
    assert_eq!(b.run_at(&late_req, NOW + 59.0, 5002.0).0, 200, "没走到记 nonce 那一步：同一个请求还能用");
    said();
}

/// 中转的 op：没开中转时不认（404）；开了就归 remote 管，验签名、认访客还是这里做
#[test]
fn relay_ops_go_to_the_relay() {
    let off = Bench::new();
    let a = Signer::new(1);
    off.enroll(&a, &dev(1), "a");
    for op in RELAY_OPS {
        assert_eq!(off.send(op, &a, ""), (404, r#"{"error":"not_found"}"#.into()), "{op}");
    }
    let on = Bench::with(true);
    on.enroll(&a, &dev(1), "a");
    said();
    let stranger = Signer::new(4);
    let gen = lock(&on.ctx.store).gen().to_string();
    for op in RELAY_OPS {
        // 不在名单里、也不是访客（没有配对、没有配对码）
        assert_eq!(on.send(op, &stranger, ""), (403, format!(r#"{{"error":"unknown_device","gen":"{gen}"}}"#)), "{op}");
        // 名单里的设备，签名不对
        let body = on.body(op, &a, "");
        let forged = request(
            Method::Post,
            &format!("/v1/{op}"),
            &[("Content-Length", &body.len().to_string()), ("X-Cca-Sig", &stranger.sign(body.as_bytes()))],
            body.as_bytes(),
        );
        assert_eq!(on.run(&forged), denied(), "{op}");
    }
    assert!(said().is_empty());
    // 签名对了：交给 remote::handle_op（各个 op 自己的事在 remote 的测试里）
    let rid = format!(r#","rid":"{}""#, "5".repeat(32));
    assert_eq!(on.send("pair_open", &a, &rid), (200, r#"{"v":1,"ok":true,"ttl":600}"#.into()));
    assert_eq!(on.send("pair_open", &a, &rid), (409, r#"{"error":"taken"}"#.into()));
    assert_eq!(on.send("pair_get", &a, ""), bad(), "op 自己的检查也是它做");
    assert_eq!(on.send("pair_close", &a, &format!(r#","rid":"{}""#, "6".repeat(32))), (404, r#"{"error":"no_pairing"}"#.into()));
    // 没开网页端：开不了给网页端的配对（设备照这个说服务器没开网页端）
    assert_eq!(
        on.send("pair_open", &a, &format!(r#","rid":"{}","web":true"#, "7".repeat(32))),
        (404, r#"{"error":"no_web"}"#.into())
    );
    // 设备的请求记“刚来过”，访客的不记：这里没有访客，名单里的设备发中转的请求也算来过
    assert!(matches!(on.seen_age(&a), V::Int(0)), "{:?}", on.seen_age(&a));
    assert!(said().is_empty());
}

/// 名单变了（登记、移除、退出）要告诉中转：它那边不在名单里的设备的配对、会话跟着清掉
#[test]
fn team_changes_reach_the_relay() {
    let b = Bench::with(true);
    let a = Signer::new(1);
    let more = format!(r#","devices":{{"{}":{{"pub":"{}","name":"a"}}}}"#, dev(1), a.line);
    let (code, text) = b.send("team_enroll", &b.root, &more);
    assert_eq!(lock(&b.ctx.store).machines(), 1);
    let lines = said();
    assert!(code == 200 && lines.is_empty(), "{code} {text} {lines:?}");
    // 两台设备配了对；其中一台被移除：这个配对跟着没了（Py:2027-2028 的 keep_only）
    let other = Signer::new(2);
    let more = format!(r#","devices":{{"{}":{{"pub":"{}","name":"b"}}}}"#, dev(2), other.line);
    assert_eq!(b.send("team_enroll", &b.root, &more).0, 200);
    let sh = b.ctx.remote.as_ref().unwrap();
    lock(&sh.m).test_edge(&"e".repeat(32), &a.fp, &other.fp, NOW);
    assert_eq!(lock(&sh.m).edges(), 1);
    assert_eq!(b.send("team_members", &b.root, "").0, 200);
    assert_eq!(lock(&sh.m).edges(), 1, "没改名单的 op 不动配对");
    assert_eq!(b.send("team_revoke", &b.root, &format!(r#","dev":"{}""#, dev(2))).0, 200);
    assert_eq!(lock(&sh.m).edges(), 0, "被移除的设备的配对清掉了");
    said();
    // 没改名单的 op 不去问中转
    assert_eq!(b.send("team_members", &b.root, "").0, 200);
    assert_eq!(b.send("team_get", &a, "").0, 200);
    assert_eq!(b.send("team_revoke", &b.root, &format!(r#","dev":"{}""#, dev(9))), (404, r#"{"error":"no_device"}"#.into()));
    assert!(said().is_empty());
}

/// 存不了：Python 里是没人接的异常——日志里说一句，不回就断开
#[test]
fn a_failed_save_closes_without_a_reply() {
    let b = Bench::new();
    let a = Signer::new(1);
    std::fs::remove_dir_all(&b.dir.path).unwrap();
    let more = format!(r#","devices":{{"{}":{{"pub":"{}","name":"a"}}}}"#, dev(1), a.line);
    assert_eq!(b.send("team_enroll", &b.root, &more), (0, String::new()));
    let lines = said();
    let tmp = format!("{}/state.json.{}.tmp", b.dir.path, std::process::id());
    assert_eq!(lines, [format!("cca-presence: 处理请求出错：[Errno 2] No such file or directory: '{tmp}'")]);
}

/// 套得很深的 JSON：在处理连接的线程（默认大小的栈）里解析不会把栈用完。一千层以内照常，再深当作写错了的 JSON
#[test]
fn deep_json_fits_a_connection_thread() {
    let done = std::thread::Builder::new()
        .spawn(|| {
            let b = Bench::new();
            let a = Signer::new(1);
            b.enroll(&a, &dev(1), "a");
            let nested = |depth: usize| format!(r#","units":{{}},"x":{}{}"#, "[".repeat(depth), "]".repeat(depth));
            let ok = b.send("report", &a, &nested(999));
            let too_deep = b.send("report", &a, &nested(1000));
            let objects = b.send("report", &a, &format!(r#","units":{{}},"x":{}1{}"#, r#"{"a":"#.repeat(999), "}".repeat(999)));
            said();
            (ok.0, too_deep, objects.0)
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(done, (200, bad(), 200));
}

/// 网页端的文件和 WebSocket 的握手（Py:1832-1870）经 GET 走到 web.rs、ws.rs（它们自己和 Python 逐字节对的那一份在各自的测试里）
#[test]
fn pages_and_websocket_checks() {
    let web = Web::open("https://team.example.com/", None, None).unwrap();
    let mut b = Bench::with(true);
    let origin = web.origin.clone();
    let index_etag = web.files.get("index.html").unwrap().etag.clone();
    let index_len = web.files.get("index.html").unwrap().raw.len();
    let ctx = Arc::get_mut(&mut b.ctx).unwrap();
    ctx.web = Some(web);
    let get = |target: &str, headers: &[(&str, &str)]| {
        answer_at(&b.ctx, PEER, false, &request(Method::Get, target, headers, b""), 0, NOW, 0.0)
    };
    // 页面：/ 就是 index.html；带着 ETag、no-cache；ETag 一样回 304
    let Outcome::Raw { code, headers, body } = get("/", &[]) else { panic!("/ 该是页面") };
    assert_eq!((code, body.len()), (200, index_len));
    let value = |name: &str| headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    assert_eq!(value("Content-Type").as_deref(), Some("text/html; charset=utf-8"));
    assert_eq!(value("Content-Length"), Some(index_len.to_string()));
    assert_eq!(
        (value("ETag"), value("Cache-Control").as_deref(), value("Vary").as_deref()),
        (Some(index_etag.clone()), Some("no-cache"), Some("Accept-Encoding"))
    );
    assert_eq!(
        headers.iter().map(|(k, _)| k.as_str()).take(5).collect::<Vec<_>>(),
        ["Content-Type", "Content-Length", "ETag", "Cache-Control", "Vary"]
    );
    let Outcome::Raw { code, headers, body } = get("/index.html?x", &[("If-None-Match", &index_etag)]) else { panic!() };
    assert_eq!((code, body.len(), headers.first().map(|(k, _)| k.as_str())), (304, 0, Some("ETag")));
    assert_eq!(shown(get("/README.md", &[])).0, 404, "说明不是页面的一部分");
    assert_eq!(shown(get("/nope.js", &[])).0, 404);
    // /v1/health 带着页面的地址
    assert!(shown(get("/v1/health", &[])).1.ends_with(r#","web":"https://team.example.com/"}"#));
    // WebSocket：各项检查照先后
    let ws = |headers: &[(&str, &str)], query: &str| shown(get(&format!("/v1/relay/ws{query}"), headers));
    let key = "dGhlIHNhbXBsZSBub25jZQ==";
    let ticket = format!("?t={}", "0".repeat(32));
    let good = [
        ("Upgrade", "WebSocket"),
        ("Connection", "keep-alive, Upgrade"),
        ("Sec-WebSocket-Version", "13"),
        ("Sec-WebSocket-Key", key),
    ];
    for i in 0..good.len() {
        let mut headers = good.to_vec();
        headers.remove(i);
        assert_eq!(ws(&headers, &ticket), bad(), "少了 {}", good[i].0);
    }
    let mut old = good.to_vec();
    old[2].1 = "8";
    assert_eq!(ws(&old, &ticket), bad());
    let mut foreign = good.to_vec();
    foreign.push(("Origin", "https://evil.example.com"));
    assert_eq!(ws(&foreign, &ticket), denied(), "别的页面发起的");
    let mut own = good.to_vec();
    own.push(("Origin", &origin));
    for query in ["", "?t=abc", "?x=1", ticket.as_str()] {
        assert_eq!(ws(&own, query), (403, r#"{"error":"ticket"}"#.into()), "{query}：没有这张票据");
        assert_eq!(ws(&good, query).0, 403, "不带 Origin 的（不是浏览器）也要票据");
    }
}
