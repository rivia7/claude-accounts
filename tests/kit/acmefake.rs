//! 假的证书机构（原来的 tests/acme_fake.py）：ACME（RFC 8555）+ 给 IP 地址签（RFC 8738）+ 证书种类（profile 的草案），
//! 给证书助手（server/cca-acme.py、`cca-core team-acme`）的测试用。不是随便回几句的桩：客户端的每个请求都照规范核对——
//! JWS 的签名（拿 openssl 验）、nonce 只能用一次、url 要对、开账户用 jwk 之后用 kid、单子里的地址和证书种类、
//! http-01 是真的去取那个文件比对内容、证书请求里的地址要和单子上的一样而且没有 Common Name（拿 openssl 读）——不对就照规范回错误。
//! 签出来的证书由临时的测试机构签（证书里的地址照单子上的）。
//!
//! - [`make_pki`]：测试的证书机构（ca.pem、ca.key）和假证书机构自己的 HTTPS 证书（cert.pem、key.pem，给 127.0.0.1 的）
//! - [`FakeCa::start`]：在 127.0.0.1 的一个空闲端口上起（HTTPS：kit::httpd + kit::tlsd，一个连接一个线程），端口写到 目录/port；
//!   做过的事一行一条 JSON 追加到 目录/log（[`read_log`] 读）——`{"account": 公钥的摘要, "new": …}`、
//!   `{"order": 编号, "ids": […], "profile": …}`、`{"validated": 地址, "ok": …}`、`{"issued": 编号, "ids": […], "profile": …}`、
//!   `{"refused": 错误的种类, "path": …}`
//! - [`Knobs`]：去哪个端口取 http-01 的文件（真的证书机构是 80）、每 N 个请求说一次 nonce 不对（客户端应该自己再发）、
//!   目录里列不列 shortlived 这一种
//!
//! 命令行（tests/tool/acme_fake.rs）：`cca-test acme-fake 目录 应答端口 [--bad-nonce N] [--no-shortlived]`，
//! 一直跑到被 kill。
//!
//! 和原来不一样的几处：
//! - 签证书的几条 openssl 命令多写了 `-sha256`：macOS 自带的 LibreSSL（/usr/bin/openssl）默认用 SHA-1 签，rustls 不认这种签名
//!   （见 kit::mailfake 的 pki.rs）；OpenSSL 3 本来就默认 SHA-256，结果不变
//! - base64url 照 kit::crypto 严格地解：Python 的放过了末尾的一个换行、末尾多出来的不是 0 的位
//! - 请求写得太离谱的几处，原来的 Python 抛了没接住的异常、连接直接断开（protected 不是对象、Content-Length 不是数、
//!   kid / nonce 不是字符串……），这里照规范回错误（malformed、accountDoesNotExist、badNonce）；openssl 运行不了回 500 serverInternal

use crate::crypto;
use crate::fsx::{self, TempDir};
use crate::httpd::{self, Request, Response};
use crate::json::{self, Obj, Style, Value};
use crate::proc::Cmd;
use crate::tlsd::Acceptor;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// 编一次、记下来的正则
macro_rules! re {
    ($p:expr) => {{
        static R: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        R.get_or_init(|| regex::Regex::new($p).expect("正则写错了"))
    }};
}

/// openssl 的参数：字符串、路径混着写
macro_rules! args {
    ($($a:expr),* $(,)?) => { [$(AsRef::<OsStr>::as_ref(&$a)),*] };
}

/// P-256 公钥的 SubjectPublicKeyInfo 的开头（DER），后面接 04 X Y
const P256_SPKI: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03,
    0x01, 0x07, 0x03, 0x42, 0x00,
];

/// 回 JSON 时的 Content-Type
const JSON: &str = "application/json";

/// 假证书机构的几个开关（原来的命令行参数）
#[derive(Clone, Copy, Debug)]
pub struct Knobs {
    /// 去哪个端口取 http-01 的文件（真的证书机构是 80）
    pub http_port: u16,
    /// 每这么多个请求说一次 nonce 不对（`--bad-nonce N`；0 是不说）：客户端应该自己再发
    pub bad_nonce: u64,
    /// 目录里列不列 shortlived 这一种（`--no-shortlived` 是 false）
    pub shortlived: bool,
}

impl Knobs {
    /// 照这个端口取 http-01 的文件，别的都是默认的（nonce 不出错、列着 shortlived）
    pub fn new(http_port: u16) -> Knobs {
        Knobs { http_port, bad_nonce: 0, shortlived: true }
    }
}

/// openssl 跑完了：退出码是不是 0、标准输出、标准错误
struct Ran {
    ok: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// 跑一条 openssl（PATH 上找到的那一个）：环境只带找它要的 PATH 和它自己的 OPENSSL_CONF，最多 30 秒（和原来一样）
fn openssl(args: &[&OsStr], input: Option<&[u8]>) -> io::Result<Ran> {
    let mut cmd = Cmd::new("openssl");
    cmd.args(args).pass_env(&["PATH", "OPENSSL_CONF"]).timeout(Duration::from_secs(30));
    if let Some(data) = input {
        cmd.input(data.to_vec());
    }
    let out = cmd.output()?;
    Ok(Ran { ok: out.status.success(), stdout: out.stdout, stderr: out.stderr })
}

/// 同上，退出码不是 0 算错（带上它在 stderr 上说的）
fn openssl_ok(args: &[&OsStr]) -> io::Result<()> {
    let ran = openssl(args, None)?;
    if ran.ok {
        return Ok(());
    }
    let shown: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    Err(io::Error::other(format!("openssl {} 失败：{}", shown.join(" "), String::from_utf8_lossy(&ran.stderr).trim())))
}

/// 测试的证书机构和假证书机构自己的 HTTPS 证书，都放在 d 里（没有就建）：
/// ca.pem、ca.key（签证书的；带着“能签证书”的用途，新的 Python 默认连这个也查）、cert.pem、key.pem（给 127.0.0.1 的，
/// 测试的机构签的），还有中间的 csr.pem、ext.cnf、ca.srl。命令和原来一样，签名的两条多写了 -sha256（见开头）
pub fn make_pki(d: &Path) -> io::Result<()> {
    fs::create_dir_all(d)?;
    let p = |n: &str| d.join(n);
    let (ca_key, ca, key, csr, ext, cert) = (p("ca.key"), p("ca.pem"), p("key.pem"), p("csr.pem"), p("ext.cnf"), p("cert.pem"));
    openssl_ok(&args![
        "req",
        "-x509",
        "-newkey",
        "ec",
        "-pkeyopt",
        "ec_paramgen_curve:prime256v1",
        "-nodes",
        "-days",
        "2",
        "-subj",
        "/CN=cca test ca",
        "-addext",
        "basicConstraints=critical,CA:TRUE",
        "-addext",
        "keyUsage=critical,keyCertSign,cRLSign",
        "-sha256",
        "-keyout",
        ca_key,
        "-out",
        ca
    ])?;
    openssl_ok(&args![
        "req",
        "-newkey",
        "ec",
        "-pkeyopt",
        "ec_paramgen_curve:prime256v1",
        "-nodes",
        "-subj",
        "/CN=localhost",
        "-keyout",
        key,
        "-out",
        csr
    ])?;
    fs::write(
        &ext,
        "subjectAltName=IP:127.0.0.1\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\nkeyUsage=critical,digitalSignature\n",
    )?;
    openssl_ok(&args![
        "x509",
        "-req",
        "-in",
        csr,
        "-CA",
        ca,
        "-CAkey",
        ca_key,
        "-CAcreateserial",
        "-days",
        "2",
        "-sha256",
        "-extfile",
        ext,
        "-out",
        cert
    ])
}

/// 目录/log 里记下的（一行一条 JSON，原来测试里的 log()）；还没有这个文件是空的。读不了、有一行不是 JSON 就 panic（测试里用）
pub fn read_log(dir: &Path) -> Vec<Value> {
    let path = dir.join("log");
    if !path.exists() {
        return Vec::new();
    }
    let text = fsx::read_text(&path);
    text.lines()
        .map(|line| json::parse(line).unwrap_or_else(|e| panic!("{} 里有一行不是 JSON（{e}）：{line}", path.display())))
        .collect()
}

/// 跑着的假证书机构（127.0.0.1 上的一个空闲端口，HTTPS）。丢掉它（或者 [`FakeCa::stop`]）就停
pub struct FakeCa {
    /// HTTPS 的端口
    pub port: u16,
    /// 它的目录（证书、port、log 在里面）
    pub dir: PathBuf,
    srv: httpd::Server,
}

impl FakeCa {
    /// 起一个：目录里要有 ca.pem、ca.key（签证书的）、cert.pem、key.pem（它自己的 HTTPS 证书）——[`make_pki`] 做的那几个。
    /// 实际监听的端口写到 目录/port（先写到 port.tmp 再换过去），做过的事追加到 目录/log
    pub fn start(dir: &Path, knobs: Knobs) -> io::Result<FakeCa> {
        let tls = Acceptor::from_pem_files(&dir.join("cert.pem"), &dir.join("key.pem"))?;
        let setup = httpd::Server::bind(("127.0.0.1", 0))?.tls(tls);
        let port = setup.port();
        let ca = Arc::new(Ca {
            dir: dir.to_path_buf(),
            knobs,
            base: format!("https://127.0.0.1:{port}"),
            st: Mutex::new(State::default()),
            log_lock: Mutex::new(()),
        });
        let srv = setup.start(move |req| ca.handle(req))?;
        let tmp = dir.join("port.tmp");
        fs::write(&tmp, format!("{port}\n"))?;
        fs::rename(&tmp, dir.join("port"))?;
        Ok(FakeCa { port, dir: dir.to_path_buf(), srv })
    }

    /// ACME 的目录的地址（证书助手的 --directory）
    pub fn directory(&self) -> String {
        format!("https://127.0.0.1:{}/dir", self.port)
    }

    /// 到现在为止记下的（[`read_log`]）
    pub fn log(&self) -> Vec<Value> {
        read_log(&self.dir)
    }

    /// 停下：不再接新的连接，接着的断开
    pub fn stop(&mut self) {
        self.srv.stop();
    }
}

/// 一个账户：开账户时的公钥和它的摘要（RFC 7638）
struct Account {
    jwk: Value,
    thumb: String,
}

/// 一张单子
struct Order {
    /// 单子上的地址，照客户端发来的原样
    ids: Vec<Value>,
    /// 它的几个验证（authz 的编号）
    authz: Vec<String>,
    profile: Option<String>,
    status: &'static str,
    /// 下单的账户（kid）
    kid: Option<String>,
}

/// 一个地址的验证（http-01）
struct Authz {
    identifier: Value,
    status: &'static str,
    token: String,
    kid: Option<String>,
    /// 没过的原因
    error: Option<String>,
}

#[derive(Default)]
struct State {
    nonces: HashSet<String>,
    /// 账户的地址 → 账户
    accounts: HashMap<String, Account>,
    orders: HashMap<String, Order>,
    authz: HashMap<String, Authz>,
    /// 单子的编号 → 签出来的证书（连着测试的机构的那一张）
    certs: HashMap<String, Vec<u8>>,
    /// 核对过 alg、url 的请求一共有几个（--bad-nonce 照它数）
    requests: u64,
}

/// 假证书机构本身：几个线程共用
struct Ca {
    dir: PathBuf,
    knobs: Knobs,
    /// `https://127.0.0.1:端口`：JWS 里的 url、回的各个地址都以它开头
    base: String,
    st: Mutex<State>,
    log_lock: Mutex<()>,
}

/// 照规范回的错误（原来的 Refuse）：状态码、错误的种类（`urn:ietf:params:acme:error:` 后面那一截）、给人看的一句
struct Refuse {
    status: u16,
    kind: &'static str,
    detail: String,
}

fn refuse(status: u16, kind: &'static str, detail: &str) -> Refuse {
    Refuse { status, kind, detail: detail.to_string() }
}

/// 这边自己出了毛病（openssl 运行不了、临时文件写不了）：500 serverInternal
fn internal(e: io::Error) -> Refuse {
    Refuse { status: 500, kind: "serverInternal", detail: format!("假的证书机构自己出了错：{e}") }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// base64url、不带 =（JWS 里的写法），严格地解（见开头）
fn unb64u(s: &str) -> Option<Vec<u8>> {
    crypto::b64url_nopad_decode(s).ok()
}

/// JWS 的签名（r、s 各 32 个字节接在一起）换成 openssl 认的 DER（SEQUENCE { INTEGER r, INTEGER s }），和原来一样逐字节拼
fn der_sig(sig: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    for half in [&sig[..32], &sig[32..]] {
        let start = half.iter().position(|b| *b != 0).unwrap_or(half.len());
        let int: &[u8] = if start == half.len() { &[0] } else { &half[start..] };
        let high = int[0] >> 7;
        body.push(0x02);
        body.push(int.len() as u8 + high);
        if high == 1 {
            body.push(0);
        }
        body.extend_from_slice(int);
    }
    let mut der = vec![0x30, body.len() as u8];
    der.extend(body);
    der
}

/// RFC 7638：crv、kty、x、y 这几项按键名排好、不带空白的 JSON 的 SHA-256（原来的 `json.dumps(sort_keys=True, separators=(',', ':'))`）
fn thumbprint(jwk: &Value) -> String {
    let mut key = Obj::new();
    for k in ["crv", "kty", "x", "y"] {
        key.insert(k, jwk[k].clone());
    }
    let text = json::dumps(&Value::Obj(key), &Style { compact: true, sort_keys: true, ..Style::default() });
    crypto::b64url_nopad(&crypto::sha256(text.as_bytes()))
}

/// 去掉两头的空白（Python 的 bytes.strip()：空格、\t、\n、\r、\x0b、\x0c）
fn py_strip(b: &[u8]) -> &[u8] {
    let ws = |c: &u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
    let start = b.iter().position(|c| !ws(c)).unwrap_or(b.len());
    let end = b.iter().rposition(|c| !ws(c)).map_or(start, |i| i + 1);
    &b[start..end]
}

/// 核对过的一个 JWS
struct Signed {
    /// 用 kid 的：那个账户的公钥摘要；用 jwk 的是 None
    acct: Option<String>,
    jwk: Value,
    kid: Option<String>,
    /// 内容；None 是 POST-as-GET（payload 是空的）
    data: Option<Value>,
}

impl Ca {
    /// 目录/log 里记一行（非 ASCII 原样写：`json.dumps(ensure_ascii=False)`）
    fn log(&self, what: Value) {
        let _g = lock(&self.log_lock);
        let line = json::dumps(&what, &Style { unicode: true, ..Style::default() }) + "\n";
        if let Ok(mut f) = OpenOptions::new().append(true).create(true).open(self.dir.join("log")) {
            let _ = f.write_all(line.as_bytes());
        }
    }

    /// 发一个新的 nonce（记下来，只能用一次）
    fn nonce(&self) -> String {
        let n = crypto::token_urlsafe(16);
        lock(&self.st).nonces.insert(n.clone());
        n
    }

    /// 回一个：每个回复都带一个新的 Replay-Nonce；内容是 raw，或者 obj 的 JSON（`json.dumps` 默认的写法），或者没有
    fn reply(&self, status: u16, obj: Option<&Value>, headers: &[(&str, String)], raw: Option<Vec<u8>>, ctype: &str) -> Response {
        let body = match (raw, obj) {
            (Some(raw), _) => raw,
            (None, Some(v)) => json::dumps(v, &Style::default()).into_bytes(),
            (None, None) => Vec::new(),
        };
        let mut resp = Response::new(status)
            .header("Replay-Nonce", self.nonce())
            .header("Content-Type", ctype)
            .header("Content-Length", body.len().to_string());
        for (k, v) in headers {
            resp = resp.header(*k, v.clone());
        }
        resp.body(body)
    }

    fn handle(&self, req: &Request) -> Response {
        match req.method.as_str() {
            "HEAD" => self.reply(if req.path == "/nonce" { 200 } else { 404 }, None, &[], None, JSON),
            "GET" => self.get(req),
            "POST" => self.post(req),
            other => Response::text(501, &format!("Unsupported method ('{other}')")).close(),
        }
    }

    fn get(&self, req: &Request) -> Response {
        if req.path == "/dir" {
            let base = &self.base;
            let mut profiles = Obj::new();
            profiles.insert("classic", format!("{base}/docs#classic"));
            if self.knobs.shortlived {
                profiles.insert("shortlived", format!("{base}/docs#shortlived"));
            }
            let doc = crate::obj! {
                "newNonce" => format!("{base}/nonce"),
                "newAccount" => format!("{base}/acct"),
                "newOrder" => format!("{base}/order"),
                "meta" => crate::obj! { "profiles" => profiles },
            };
            return self.reply(200, Some(&doc), &[], None, JSON);
        }
        self.reply(if req.path == "/nonce" { 204 } else { 404 }, None, &[], None, JSON)
    }

    fn post(&self, req: &Request) -> Response {
        match self.post_checked(req) {
            Ok(resp) => resp,
            Err(e) => {
                self.log(crate::obj! { "refused" => e.kind, "path" => req.path.as_str() });
                let doc = crate::obj! { "type" => format!("urn:ietf:params:acme:error:{}", e.kind), "detail" => e.detail };
                self.reply(e.status, Some(&doc), &[], None, "application/problem+json")
            }
        }
    }

    fn post_checked(&self, req: &Request) -> Result<Response, Refuse> {
        if req.header("Content-Type") != Some("application/jose+json") {
            return Err(refuse(415, "malformed", "Content-Type 要是 application/jose+json"));
        }
        let signed = self.verify(&format!("{}{}", self.base, req.path), &req.body)?;
        self.route(&req.path, signed)
    }

    /// 核对一个 JWS（先后和原来一样：看得懂 → alg 和 url → nonce（这一步起算请求数）→ jwk / kid → 公钥的样子 → 签名）
    fn verify(&self, url: &str, body: &[u8]) -> Result<Signed, Refuse> {
        let malformed = || refuse(400, "malformed", "JWS 看不懂");
        let j = json::parse_bytes(body).map_err(|_| malformed())?;
        let text = |k: &str| j.get(k).and_then(Value::as_str).ok_or_else(malformed);
        let (protected_b64, payload) = (text("protected")?, text("payload")?);
        let protected = unb64u(protected_b64).and_then(|b| json::parse_bytes(&b).ok()).ok_or_else(malformed)?;
        let sig = unb64u(text("signature")?).ok_or_else(malformed)?;
        let data = match payload {
            "" => None,
            p => Some(unb64u(p).and_then(|b| json::parse_bytes(&b).ok()).ok_or_else(malformed)?),
        };
        let Some(p) = protected.as_obj() else { return Err(malformed()) };
        if p.get("alg") != Some(&Value::from("ES256")) || p.get("url") != Some(&Value::from(url)) {
            return Err(refuse(400, "malformed", "alg 或者 url 不对"));
        }
        let (fresh, flaky) = {
            let mut st = lock(&self.st);
            st.requests += 1;
            let fresh = p.get("nonce").and_then(Value::as_str).is_some_and(|n| st.nonces.remove(n));
            (fresh, self.knobs.bad_nonce > 0 && st.requests.is_multiple_of(self.knobs.bad_nonce))
        };
        if !fresh || flaky {
            return Err(refuse(400, "badNonce", "nonce 不对"));
        }
        if p.contains_key("jwk") == p.contains_key("kid") {
            return Err(refuse(400, "malformed", "jwk 和 kid 要有且只有一个"));
        }
        let (jwk, acct, kid) = match p.get("jwk") {
            Some(jwk) => (jwk.clone(), None, None),
            None => {
                let kid = p.get("kid").and_then(Value::as_str).map(str::to_string);
                let st = lock(&self.st);
                let found = kid.as_ref().and_then(|k| st.accounts.get(k));
                let Some(acct) = found else { return Err(refuse(400, "accountDoesNotExist", "没有这个账户")) };
                (acct.jwk.clone(), Some(acct.thumb.clone()), kid)
            }
        };
        let bad_key = || refuse(400, "badPublicKey", "公钥或者签名的样子不对");
        let key = jwk.as_obj().ok_or_else(bad_key)?;
        if key.get("kty") != Some(&Value::from("EC")) || key.get("crv") != Some(&Value::from("P-256")) {
            return Err(bad_key());
        }
        let coord = |k: &str| key.get(k).and_then(Value::as_str).and_then(unb64u).ok_or_else(bad_key);
        let (x, y) = (coord("x")?, coord("y")?);
        if (x.len(), y.len(), sig.len()) != (32, 32, 64) {
            return Err(bad_key());
        }
        let tmp = TempDir::new("acmefake-");
        let (pubf, sigf) = (tmp.join("pub.der"), tmp.join("sig"));
        fs::write(&pubf, [&P256_SPKI[..], &[4], &x, &y].concat()).map_err(internal)?;
        fs::write(&sigf, der_sig(&sig)).map_err(internal)?;
        let signed_text = format!("{protected_b64}.{payload}");
        let ran = openssl(
            &args!["dgst", "-sha256", "-verify", pubf, "-keyform", "DER", "-signature", sigf],
            Some(signed_text.as_bytes()),
        )
        .map_err(internal)?;
        if !ran.ok {
            return Err(refuse(403, "unauthorized", "签名不对"));
        }
        Ok(Signed { acct, jwk, kid, data })
    }

    fn route(&self, path: &str, s: Signed) -> Result<Response, Refuse> {
        if path == "/acct" {
            let agreed = s
                .data
                .as_ref()
                .and_then(Value::as_obj)
                .is_some_and(|d| matches!(d.get("termsOfServiceAgreed"), Some(Value::Bool(true))));
            if s.acct.is_some() || !agreed {
                return Err(refuse(400, "malformed", "开账户要用 jwk，并且同意条款"));
            }
            let thumb = thumbprint(&s.jwk);
            let url = format!("{}/acct/{}", self.base, &thumb[..16]);
            let new = {
                let mut st = lock(&self.st);
                let new = !st.accounts.contains_key(&url);
                st.accounts.insert(url.clone(), Account { jwk: s.jwk.clone(), thumb: thumb.clone() });
                new
            };
            self.log(crate::obj! { "account" => thumb, "new" => new });
            return Ok(self.reply(
                if new { 201 } else { 200 },
                Some(&crate::obj! { "status" => "valid" }),
                &[("Location", url)],
                None,
                JSON,
            ));
        }
        let Some(thumb) = s.acct.clone() else { return Err(refuse(400, "malformed", "这个请求要用 kid")) };
        let (kid, data) = (s.kid, s.data);
        if path == "/order" {
            return self.new_order(data.as_ref(), kid);
        }
        let trimmed = path.trim_matches('/');
        let (kind, n) = trimmed.split_once('/').unwrap_or((trimmed, ""));
        let mut st = lock(&self.st);
        let owns_order = st.orders.get(n).is_some_and(|o| o.kid == kid);
        if kind == "o" && owns_order && data.is_none() {
            let doc = self.order_doc(&mut st, n);
            drop(st);
            return Ok(self.reply(200, Some(&doc), &[], None, JSON));
        }
        if (kind == "authz" || kind == "chall") && st.authz.get(n).is_some_and(|a| a.kid == kid) {
            if kind == "chall" {
                if data != Some(Value::Obj(Obj::new())) {
                    return Err(refuse(400, "malformed", "开始验证要发 {}"));
                }
                if st.authz[n].status == "pending" {
                    drop(st);
                    self.validate(n, &thumb);
                    st = lock(&self.st);
                }
            } else if data.is_some() {
                return Err(refuse(400, "malformed", "取验证的要求是 POST-as-GET"));
            }
            let a = &st.authz[n];
            let mut ch = Obj::new();
            ch.insert("type", "http-01");
            ch.insert("url", format!("{}/chall/{n}", self.base));
            ch.insert("token", a.token.as_str());
            ch.insert("status", a.status);
            if let Some(why) = &a.error {
                ch.insert("error", crate::obj! { "type" => "urn:ietf:params:acme:error:unauthorized", "detail" => why.as_str() });
            }
            let doc = if kind == "chall" {
                Value::Obj(ch)
            } else {
                crate::obj! { "status" => a.status, "identifier" => &a.identifier, "challenges" => vec![Value::Obj(ch)] }
            };
            drop(st);
            return Ok(self.reply(200, Some(&doc), &[], None, JSON));
        }
        if kind == "fin" && owns_order {
            drop(st);
            return self.finalize(n, data.as_ref());
        }
        if kind == "cert" && st.certs.contains_key(n) && owns_order && data.is_none() {
            let pem = st.certs[n].clone();
            drop(st);
            return Ok(self.reply(200, None, &[], Some(pem), "application/pem-certificate-chain"));
        }
        Err(refuse(404, "malformed", "没有这个地址"))
    }

    fn new_order(&self, data: Option<&Value>, kid: Option<String>) -> Result<Response, Refuse> {
        let fields = data.and_then(Value::as_obj);
        let good = |i: &Value| {
            i.as_obj().is_some_and(|o| {
                matches!(o.get("type").and_then(Value::as_str), Some("ip" | "dns"))
                    && o.get("value").and_then(Value::as_str).is_some()
            })
        };
        let ids =
            fields.and_then(|d| d.get("identifiers")).and_then(Value::as_list).filter(|l| !l.is_empty() && l.iter().all(good));
        let Some(ids) = ids.map(<[Value]>::to_vec) else { return Err(refuse(400, "malformed", "单子上的地址不对")) };
        // 没写和写了 null 一样（原来的 data.get('profile')）
        let profile = match fields.and_then(|d| d.get("profile")).filter(|v| !v.is_null()) {
            None => None,
            Some(v) => match v.as_str() {
                Some("classic") => Some("classic".to_string()),
                Some("shortlived") if self.knobs.shortlived => Some("shortlived".to_string()),
                _ => return Err(refuse(400, "invalidProfile", "没有这一种证书")),
            },
        };
        if ids.iter().any(|i| i["type"].str() == "ip") && profile.as_deref() != Some("shortlived") {
            return Err(refuse(400, "rejectedIdentifier", "给 IP 地址签的只有 shortlived 这一种"));
        }
        let (n, doc) = {
            let mut st = lock(&self.st);
            let n = (st.orders.len() + 1).to_string();
            let mut authz = Vec::new();
            for i in &ids {
                let a = format!("{n}-{}", authz.len());
                let token = crypto::token_urlsafe(24);
                st.authz
                    .insert(a.clone(), Authz { identifier: i.clone(), status: "pending", token, kid: kid.clone(), error: None });
                authz.push(a);
            }
            st.orders.insert(n.clone(), Order { ids: ids.clone(), authz, profile: profile.clone(), status: "pending", kid });
            let doc = self.order_doc(&mut st, &n);
            (n, doc)
        };
        self.log(crate::obj! { "order" => n.as_str(), "ids" => ids, "profile" => profile });
        Ok(self.reply(201, Some(&doc), &[("Location", format!("{}/o/{n}", self.base))], None, JSON))
    }

    /// 单子现在的样子：验证都过了是 ready，有一个没过是 invalid
    fn order_doc(&self, st: &mut State, n: &str) -> Value {
        let statuses: Vec<&'static str> = st.orders[n].authz.iter().map(|a| st.authz[a].status).collect();
        let Some(o) = st.orders.get_mut(n) else { return Value::Null };
        if o.status == "pending" && statuses.iter().all(|s| *s == "valid") {
            o.status = "ready";
        }
        if statuses.contains(&"invalid") {
            o.status = "invalid";
        }
        let base = &self.base;
        let mut doc = Obj::new();
        doc.insert("status", o.status);
        doc.insert("identifiers", o.ids.clone());
        doc.insert("authorizations", o.authz.iter().map(|a| Value::from(format!("{base}/authz/{a}"))).collect::<Vec<_>>());
        doc.insert("finalize", format!("{base}/fin/{n}"));
        if let Some(p) = &o.profile {
            doc.insert("profile", p.as_str());
        }
        if o.status == "valid" {
            doc.insert("certificate", format!("{base}/cert/{n}"));
        }
        Value::Obj(doc)
    }

    /// http-01：从“公网”（这里是本机的应答端口）取那个文件，Host 是要验证的地址，内容要是 令牌.账户公钥的摘要
    fn validate(&self, n: &str, thumb: &str) {
        let (token, value) = {
            let st = lock(&self.st);
            let a = &st.authz[n];
            (a.token.clone(), a.identifier["value"].str().to_string())
        };
        let want = format!("{token}.{thumb}");
        let (ok, why) = match fetch(self.knobs.http_port, &value, &format!("/.well-known/acme-challenge/{token}")) {
            Ok((status, got)) => {
                let ok = status == 200 && py_strip(&got) == want.as_bytes();
                (ok, (!ok).then(|| format!("取到的内容不对（状态码 {status}）")))
            }
            Err(kind) => (false, Some(format!("连不上 80 端口（{kind}）"))),
        };
        if let Some(a) = lock(&self.st).authz.get_mut(n) {
            a.status = if ok { "valid" } else { "invalid" };
            a.error = why;
        }
        self.log(crate::obj! { "validated" => value, "ok" => ok });
    }

    fn finalize(&self, n: &str, data: Option<&Value>) -> Result<Response, Refuse> {
        let (ids, profile) = {
            let mut st = lock(&self.st);
            self.order_doc(&mut st, n);
            let o = &st.orders[n];
            if o.status != "ready" {
                return Err(refuse(403, "orderNotReady", "地址还没验证完"));
            }
            (o.ids.clone(), o.profile.clone())
        };
        let csr = data
            .and_then(|d| d.get("csr"))
            .and_then(Value::as_str)
            .and_then(unb64u)
            .ok_or_else(|| refuse(400, "malformed", "没有证书请求"))?;
        let text = openssl(&args!["req", "-inform", "DER", "-noout", "-text", "-verify"], Some(&csr)).map_err(internal)?;
        if !text.ok {
            return Err(refuse(400, "badCSR", "证书请求看不懂，或者它自己的签名不对"));
        }
        let out = String::from_utf8_lossy(&text.stdout);
        if re!(r"(?m)^\s*Subject:(.*)$").captures(&out).is_some_and(|c| c[1].contains("CN")) {
            return Err(refuse(400, "badCSR", "证书请求里不该有 Common Name"));
        }
        let mut have: Vec<(String, String)> = re!(r"(IP Address|DNS):([^,\s]+)")
            .captures_iter(&out)
            .map(|c| {
                if &c[1] == "IP Address" {
                    ("ip".to_string(), c[2].to_string())
                } else {
                    ("dns".to_string(), c[2].to_lowercase())
                }
            })
            .collect();
        have.sort();
        let mut want: Vec<(String, String)> =
            ids.iter().map(|i| (i["type"].str().to_string(), i["value"].str().to_string())).collect();
        want.sort();
        if have != want {
            return Err(refuse(400, "badCSR", "证书请求里的地址和单子上的不一样"));
        }
        let tmp = TempDir::new("acmefake-");
        let (req, ext, crt) = (tmp.join("csr.der"), tmp.join("ext.cnf"), tmp.join("cert.pem"));
        let san: Vec<String> = have.iter().map(|(t, v)| format!("{}{v}", if t == "ip" { "IP:" } else { "DNS:" })).collect();
        fs::write(&req, &csr).map_err(internal)?;
        let ext_text = format!(
            "subjectAltName=critical,{}\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n",
            san.join(",")
        );
        fs::write(&ext, ext_text).map_err(internal)?;
        let (ca_pem, ca_key) = (self.dir.join("ca.pem"), self.dir.join("ca.key"));
        let serial = format!("0x{}", crypto::token_hex(8));
        let days = if profile.as_deref() == Some("shortlived") { "7" } else { "90" };
        let signed = openssl(
            &args![
                "x509",
                "-req",
                "-inform",
                "DER",
                "-in",
                req,
                "-CA",
                ca_pem,
                "-CAkey",
                ca_key,
                "-set_serial",
                serial,
                "-days",
                days,
                "-sha256",
                "-extfile",
                ext,
                "-out",
                crt
            ],
            None,
        )
        .map_err(internal)?;
        if !signed.ok {
            return Err(refuse(500, "serverInternal", "签不出来"));
        }
        let mut pem = fs::read(&crt).map_err(internal)?;
        pem.extend(fs::read(&ca_pem).map_err(internal)?);
        {
            let mut st = lock(&self.st);
            st.certs.insert(n.to_string(), pem);
            if let Some(o) = st.orders.get_mut(n) {
                o.status = "valid";
            }
        }
        self.log(crate::obj! { "issued" => n, "ids" => ids, "profile" => profile });
        let doc = self.order_doc(&mut lock(&self.st), n);
        Ok(self.reply(200, Some(&doc), &[], None, JSON))
    }
}

/// http-01 的那一下（原来的 http.client，时限 5 秒）：连 127.0.0.1 的应答端口，GET 路径，Host 是要验证的地址。
/// 给出状态码和内容的前 4096 个字节（`r.read(4096)`）。连不上、超时、对方什么都没回就断了：Err(Python 里那个异常的名字)；
/// 回的看不懂也是 Err（BadStatusLine：原来的 Python 在这里抛了没接住的异常）
fn fetch(port: u16, host: &str, path: &str) -> Result<(u16, Vec<u8>), &'static str> {
    const MAX: usize = 4096;
    let limit = Duration::from_secs(5);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let s = TcpStream::connect_timeout(&addr, limit).map_err(|e| py_error(&e))?;
    let _ = s.set_read_timeout(Some(limit));
    let _ = s.set_write_timeout(Some(limit));
    let mut rd = Reader { s, buf: Vec::new() };
    // http.client 的写法：Host 写在请求的头里时，它自己的那个 Host 不加；Accept-Encoding: identity 在前
    rd.s.write_all(format!("GET {path} HTTP/1.1\r\nAccept-Encoding: identity\r\nHost: {host}\r\n\r\n").as_bytes())
        .map_err(|e| py_error(&e))?;
    let line = rd.line().map_err(|e| py_error(&e))?;
    if line.is_empty() {
        return Err("RemoteDisconnected");
    }
    let line = String::from_utf8_lossy(&line);
    let mut words = line.split_whitespace();
    let status = match (words.next(), words.next()) {
        (Some(v), Some(code)) if v.starts_with("HTTP/") && code.len() == 3 => code.parse::<u16>().map_err(|_| "BadStatusLine")?,
        _ => return Err("BadStatusLine"),
    };
    let (mut length, mut chunked) = (None, false);
    loop {
        let h = rd.line().map_err(|e| py_error(&e))?;
        let h = String::from_utf8_lossy(&h);
        let h = h.trim_end_matches(['\r', '\n']);
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                length = v.trim().parse::<usize>().ok();
            } else if k.trim().eq_ignore_ascii_case("transfer-encoding") && v.to_ascii_lowercase().contains("chunked") {
                chunked = true;
            }
        }
    }
    let body = if status == 204 || status == 304 || status < 200 {
        Vec::new()
    } else if chunked {
        rd.chunked(MAX).map_err(|e| py_error(&e))?
    } else {
        rd.upto(length.map_or(MAX, |n| n.min(MAX))).map_err(|e| py_error(&e))?
    };
    Ok((status, body))
}

/// io 的错误在 Python 里叫什么（http.client 抛的那几个 OSError）
fn py_error(e: &io::Error) -> &'static str {
    match e.kind() {
        io::ErrorKind::ConnectionRefused => "ConnectionRefusedError",
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => "TimeoutError",
        io::ErrorKind::ConnectionReset => "ConnectionResetError",
        io::ErrorKind::ConnectionAborted => "ConnectionAbortedError",
        io::ErrorKind::BrokenPipe => "BrokenPipeError",
        _ => "OSError",
    }
}

/// 带缓冲地读回复
struct Reader {
    s: TcpStream,
    buf: Vec<u8>,
}

impl Reader {
    /// 再读一些；对方关了是 false
    fn fill(&mut self) -> io::Result<bool> {
        let mut chunk = [0u8; 4096];
        loop {
            match self.s.read(&mut chunk) {
                Ok(0) => return Ok(false),
                Ok(n) => {
                    self.buf.extend_from_slice(&chunk[..n]);
                    return Ok(true);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }

    /// 一行（带着 \n）；对方关了时是剩下的（可能是空的）
    fn line(&mut self) -> io::Result<Vec<u8>> {
        loop {
            if let Some(i) = self.buf.iter().position(|b| *b == b'\n') {
                return Ok(self.buf.drain(..=i).collect());
            }
            if self.buf.len() > 65536 || !self.fill()? {
                return Ok(std::mem::take(&mut self.buf));
            }
        }
    }

    /// 最多 n 个字节（对方先关了就是读到的那些）
    fn upto(&mut self, n: usize) -> io::Result<Vec<u8>> {
        while self.buf.len() < n && self.fill()? {}
        let k = n.min(self.buf.len());
        Ok(self.buf.drain(..k).collect())
    }

    /// chunked 的内容，最多 max 个字节
    fn chunked(&mut self, max: usize) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        while out.len() < max {
            let line = self.line()?;
            let line = String::from_utf8_lossy(&line);
            let size = usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16).unwrap_or(0);
            if size == 0 {
                break;
            }
            let part = self.upto(size)?;
            let short = part.len() < size;
            out.extend(part);
            if short {
                break;
            }
            self.line()?;
        }
        out.truncate(max);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// 只回一次的应答端口：读完请求的头，原样写回 reply，然后断开（chunked、什么都不回这几种 kit::httpd 不会写）
    fn once(reply: &'static [u8]) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").expect("听一个端口");
        let port = l.local_addr().expect("端口").port();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().expect("接一个连接");
            let mut got = Vec::new();
            let mut b = [0u8; 1024];
            while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut b) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => got.extend_from_slice(&b[..n]),
                }
            }
            let _ = s.write_all(reply);
        });
        port
    }

    /// 期望值是固定的数据（原来那份 Python 拼 DER 的那几行算的）：前面的 0 去掉、全是 0 时留一个、最高位是 1 的前面补一个 0
    #[test]
    fn signatures_become_der_like_python() {
        let mut sig = vec![0u8; 32];
        sig.push(0x80);
        sig.extend([1u8; 31]);
        assert_eq!(hex(&der_sig(&sig)), "30260201000221008001010101010101010101010101010101010101010101010101010101010101");
        let mut sig = vec![0u8, 0, 0x7f];
        sig.extend([2u8; 29]);
        sig.extend([0u8, 0xff]);
        sig.extend([3u8; 30]);
        assert_eq!(
            hex(&der_sig(&sig)),
            "3042021e7f0202020202020202020202020202020202020202020202020202020202022000ff030303030303030303030303030303030303030303030303030303030303"
        );
    }

    /// 公钥的摘要（RFC 7638）只看 crv、kty、x、y；期望值是 Python 的 json.dumps(sort_keys=True, separators=(',', ':')) 算的。
    /// 两头去空白照 Python 的 bytes.strip()
    #[test]
    fn thumbprint_and_strip_like_python() {
        let jwk = crate::obj! {
            "kty" => "EC",
            "crv" => "P-256",
            "x" => "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
            "y" => "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0",
            "extra" => 1,
        };
        assert_eq!(thumbprint(&jwk), "oKIywvGUpTVTyxMQ3bwIIeQUudfr_CkLMjCE19ECD-U");
        assert_eq!(py_strip(b" \t\x0b\x0cabc d\r\n"), b"abc d");
        assert_eq!(py_strip(b" \t\r\n"), b"");
        assert_eq!(py_strip(b"x"), b"x");
    }

    /// http-01 的那一下：Host 是要验证的地址、内容最多要 4096 个字节、chunked 也读得懂；连不上、对方什么都没回就断了、
    /// 回的看不懂，给出 Python 里那个异常的名字
    #[test]
    fn fetch_like_http_client() {
        let srv = httpd::Server::bind(("127.0.0.1", 0))
            .expect("听一个端口")
            .start(|req| match req.path.as_str() {
                "/big" => Response::new(200).body(vec![b'x'; 5000]),
                "/gone" => Response::new(404),
                _ => Response::text(200, &format!("{} {}", req.header("host").unwrap_or("-"), req.path)),
            })
            .expect("起应答端口");
        let path = "/.well-known/acme-challenge/t";
        assert_eq!(fetch(srv.port(), "relay.example.com", path), Ok((200, format!("relay.example.com {path}").into_bytes())));
        assert_eq!(fetch(srv.port(), "127.0.0.1", "/big").map(|(s, b)| (s, b.len())), Ok((200, 4096)));
        assert_eq!(fetch(srv.port(), "127.0.0.1", "/gone"), Ok((404, Vec::new())));
        let chunked = once(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n");
        assert_eq!(fetch(chunked, "127.0.0.1", "/"), Ok((200, b"hello world".to_vec())));
        assert_eq!(fetch(once(b""), "127.0.0.1", "/"), Err("RemoteDisconnected"));
        assert_eq!(fetch(once(b"SSH-2.0-OpenSSH\r\n\r\n"), "127.0.0.1", "/"), Err("BadStatusLine"));
        assert_eq!(fetch(fsx::free_port(), "127.0.0.1", "/"), Err("ConnectionRefusedError"));
    }

    /// 还没有记录时是空的；一行一条
    #[test]
    fn the_log() {
        let d = TempDir::new("acmefake-log-");
        assert_eq!(read_log(d.path()), Vec::<Value>::new());
        fsx::write(d.path().join("log"), "{\"order\": \"1\", \"profile\": null}\n{\"validated\": \"127.0.0.1\", \"ok\": true}\n");
        let log = read_log(d.path());
        assert_eq!(log.len(), 2);
        assert_eq!((&log[0]["profile"], &log[1]["ok"]), (&Value::Null, &Value::from(true)));
    }
}
