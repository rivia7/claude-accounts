//! 和证书机构说话（RFC 8555）：每个请求都是拿账户的私钥签的 JWS。Python 的 `Acme`（Acme:238-366）。
//!
//! - 连接、读回应在 fetch.rs；这里的 `http` 是 Acme:247-262：哪个回应里有 Replay-Nonce 都记下来，回的内容只有 Content-Type 里带
//!   json 时才当 JSON 解析（解析不了就还是原样的字节）。连不上、握不了手、回应写坏了：`Problem("连不上证书机构（哪一类）")`
//! - `post`（nonce 过期了照新给的再发，最多三次）、`why`（证书机构说的原因：只取类型和 detail，限制长度、去掉控制字符）、
//!   `account`、`wait`（最多等 `POLL` 秒；Retry-After 夹在 1 到 10 秒之间，没给是 2 秒）、`issue`（下单、应答 http-01、
//!   交证书请求、取证书；验证用的令牌最后一定收掉）一句一句照 Python 的来，说的话一个字不差
//! - 连证书机构时信哪些根证书在 roots.rs（证书机构的名字可以是 IP 地址：测试里的假证书机构就是）
//!
//! 和 Python 不一样的：证书机构回的 JSON 样子不对（该是网址的地方不是字符串、该是数组的地方不是数组、Retry-After 是 nan）时，
//! Python 那一份是没人接的异常、整个进程退出；这里当成这一次没要到（多半是“连不上证书机构（InvalidUrl）”），过一阵再试。
//!
//! 一个 `Acme` 只在一个线程里用（里面的 JSON 是 `py::obj::V`，不能跨线程）。

use super::csr;
use super::fetch::{self, Response};
use super::key::{b64u, Key};
use super::respond::Challenges;
use super::roots;
use super::{token_re, Problem, Trouble, POLL};
use crate::dict;
use crate::py::json::{self, Fmt};
use crate::py::obj::{Dict, V};
use crate::py::{text, time};
use crate::server::tls::client_config;
use crate::server::x509::San;
use rustls::ClientConfig;
use std::sync::Arc;

/// 等证书机构时看的钟、怎么等：平常是真的，测试里换成假的（不真的睡）
pub struct Pace {
    /// `time.monotonic()`
    pub monotonic: Box<dyn Fn() -> f64>,
    /// `time.sleep(秒)`
    pub sleep: Box<dyn Fn(f64)>,
}

impl Pace {
    pub fn real() -> Pace {
        Pace { monotonic: Box::new(time::monotonic), sleep: Box::new(time::sleep) }
    }
}

/// 证书机构回的一个回应（Acme:247-262 的 `status, headers, parsed`）
struct Reply {
    status: u16,
    /// 头；内容没当成 JSON 时原样的字节也在这里
    head: Response,
    /// 内容当成 JSON 解析出来的；Content-Type 里没有 json、解析不了时是 None（Python 里那时是 bytes）
    json: Option<V>,
}

impl Reply {
    /// `j if isinstance(j, dict)`
    fn dict(&self) -> Option<Dict> {
        self.json.as_ref().and_then(V::as_dict)
    }

    /// Location 头（空的当没有）
    fn location(&self) -> Option<&str> {
        self.head.header("Location").filter(|l| !l.is_empty())
    }

    fn why(&self) -> String {
        why(self.status, self.json.as_ref())
    }
}

/// 证书机构说的原因：只取它的类型和 detail（给人看的那一句），限制长度、去掉控制字符（Acme:285-293）
pub fn why(status: u16, j: Option<&V>) -> String {
    if let Some(j) = j.and_then(V::as_dict) {
        // str(j.get('type', '')).rsplit(':', 1)[-1]：最后一个冒号后面的
        let kind = j.get("type").map_or_else(String::new, |v| v.py_str());
        let kind = kind.rsplit(':').next().unwrap_or("");
        let detail = j.get("detail").map_or_else(String::new, |v| v.py_str());
        let said: String =
            format!("{kind} {detail}").chars().map(|c| if matches!(c, '\0'..='\x1f' | '\x7f') { ' ' } else { c }).collect();
        let said = text::strip(text::head(&said, 300));
        if !said.is_empty() {
            return format!("{said}（{status}）");
        }
    }
    format!("状态码 {status}")
}

/// 和一个证书机构的一段对话：账户、下一个请求用的 nonce 都记在里面。没成以后整个丢掉、下次从头来
pub struct Acme<'k> {
    /// 账户的私钥
    key: &'k Key,
    /// 账户的网址（开了账户以后请求里写它，之前写公钥）
    kid: Option<String>,
    /// 下一个请求用的 nonce（上一个回应给的）
    nonce: Option<String>,
    config: Arc<ClientConfig>,
    /// 证书机构的目录：各种请求发到哪
    dir: Dict,
    pace: Pace,
}

impl<'k> Acme<'k> {
    /// 连上证书机构、取它的目录（Acme:240-245）。ca 是 --ca 给的文件：读不了是 `Trouble::Os`（Python 里是 OSError），
    /// 目录取不到、不对是 `Trouble::Problem`
    pub fn new(directory: &str, key: &'k Key, ca: Option<&str>) -> Result<Acme<'k>, Trouble> {
        let config = client_config(roots::trust(ca)?).map_err(Trouble::Os)?;
        Ok(Acme::with(directory, key, config, Pace::real())?)
    }

    /// 同上，TLS 的设置和钟是给定的
    pub fn with(directory: &str, key: &'k Key, config: Arc<ClientConfig>, pace: Pace) -> Result<Acme<'k>, Problem> {
        let mut acme = Acme { key, kid: None, nonce: None, config, dir: Dict::new(), pace };
        let reply = acme.http("GET", directory, None)?;
        match reply.dict() {
            Some(dir) if reply.status == 200 && ["newNonce", "newAccount", "newOrder"].iter().all(|k| dir.str(k).is_some()) => {
                acme.dir = dir;
                Ok(acme)
            }
            _ => Err(Problem(format!("证书机构的目录读不了（{}）", reply.status))),
        }
    }

    /// 目录里的一个网址（`new` 里查过这三个都是字符串）
    fn dir_url(&self, name: &str) -> String {
        self.dir.str(name).map_or_else(String::new, |u| u.to_string())
    }

    /// 发一个请求（Acme:247-262）。data 是 POST 的内容
    fn http(&mut self, method: &str, url: &str, data: Option<&[u8]>) -> Result<Reply, Problem> {
        let head = fetch::request(method, url, data, &self.config).map_err(|kind| Problem(format!("连不上证书机构（{kind}）")))?;
        if let Some(nonce) = head.header("Replay-Nonce").filter(|n| !n.is_empty()) {
            self.nonce = Some(nonce.to_string());
        }
        let json = if head.header("Content-Type").is_some_and(|kind| kind.contains("json")) {
            std::str::from_utf8(&head.body).ok().and_then(|body| json::loads(body).ok())
        } else {
            None
        };
        Ok(Reply { status: head.status, head, json })
    }

    /// 发一个签了名的请求（Acme:264-282）。payload 是 None：只是取（POST-as-GET）。nonce 过期了（badNonce）照新给的再发
    fn post(&mut self, url: &str, payload: Option<&V>) -> Result<Reply, Problem> {
        for attempt in 0..3 {
            let nonce = match self.nonce.take() {
                Some(nonce) => nonce,
                None => {
                    let new_nonce = self.dir_url("newNonce");
                    self.http("HEAD", &new_nonce, None)?;
                    self.nonce.take().ok_or_else(|| Problem("证书机构没给 nonce".to_string()))?
                }
            };
            let protected = dict! { "alg" => "ES256", "nonce" => nonce, "url" => url };
            match &self.kid {
                Some(kid) => protected.set("kid", kid.as_str()),
                None => protected.set("jwk", self.key.jwk()),
            }
            let p = b64u(json::dumps(&V::Dict(protected), Fmt::new()).as_bytes());
            let d = payload.map_or_else(String::new, |payload| b64u(json::dumps(payload, Fmt::new()).as_bytes()));
            let signature = b64u(&self.key.sign(format!("{p}.{d}").as_bytes())?);
            let body = json::dumps(&V::Dict(dict! { "protected" => p, "payload" => d, "signature" => signature }), Fmt::new());
            let reply = self.http("POST", url, Some(body.as_bytes()))?;
            let bad_nonce = reply.status == 400
                && reply.dict().is_some_and(|j| j.get("type").is_some_and(|kind| kind.py_str().ends_with(":badNonce")));
            if bad_nonce && attempt < 2 {
                continue;
            }
            return Ok(reply);
        }
        Err(Problem("证书机构一直说 nonce 不对".to_string()))
    }

    /// 开账户（已经有了的话证书机构给的还是原来那个）（Acme:295-299）
    fn account(&mut self) -> Result<(), Problem> {
        let url = self.dir_url("newAccount");
        let reply = self.post(&url, Some(&V::Dict(dict! { "termsOfServiceAgreed" => true })))?;
        match reply.location() {
            Some(kid) if matches!(reply.status, 200 | 201) => {
                self.kid = Some(kid.to_string());
                Ok(())
            }
            _ => Err(Problem(format!("证书机构没给开账户：{}", reply.why()))),
        }
    }

    /// 一遍遍取 url，到它的状态成了 valid 为止；状态成了 invalid 就是没成（Acme:301-320）
    fn wait(&mut self, url: &str, what: &str) -> Result<Dict, Problem> {
        let end = (self.pace.monotonic)() + POLL as f64;
        loop {
            let reply = self.post(url, None)?;
            let j = match reply.dict() {
                Some(j) if reply.status == 200 => j,
                _ => return Err(Problem(format!("{what}：{}", reply.why()))),
            };
            let status = j.str("status");
            if status.as_deref() == Some("invalid") {
                // 各个验证办法里头一个说了原因的；都没说就看整个的
                let challenges = j.list("challenges").map(|l| l.items()).unwrap_or_default();
                let said = challenges.iter().filter_map(V::as_dict).filter_map(|c| c.get("error")).find(V::truthy);
                let said = said.or_else(|| j.get("error")).filter(V::truthy);
                let said = said.map_or_else(|| "证书机构没说原因".to_string(), |error| why(reply.status, Some(&error)));
                return Err(Problem(format!("{what}没过：{said}")));
            }
            if status.as_deref() == Some("valid") {
                return Ok(j);
            }
            if (self.pace.monotonic)() > end {
                return Err(Problem(format!("{what}等了 {POLL} 秒还没好")));
            }
            // Retry-After 写的是秒数才认（写成日期的、看不懂的照 2 秒）
            let pause = reply.head.header("Retry-After").filter(|v| !v.is_empty()).and_then(text::to_float).filter(|p| !p.is_nan());
            (self.pace.sleep)(pause.unwrap_or(2.0).clamp(1.0, 10.0));
        }
    }

    /// 要一张证书：返回 PEM（证书连同中间证书）（Acme:322-366）。ids 是证书上的地址，profile 是要哪一种（空的：证书机构默认的），
    /// cert_key 是证书的私钥，responder 是放 http-01 的内容的地方
    pub fn issue(&mut self, ids: &[San], profile: &str, cert_key: &Key, responder: &dyn Challenges) -> Result<Vec<u8>, Trouble> {
        if self.kid.is_none() {
            self.account()?;
        }
        let identifiers: Vec<V> = ids.iter().map(|id| V::Dict(dict! { "type" => id.kind(), "value" => id.value() })).collect();
        let order = dict! { "identifiers" => identifiers };
        let offered = self.dir.dict("meta").and_then(|meta| meta.dict("profiles")).is_some_and(|profiles| profiles.has(profile));
        if !profile.is_empty() && offered {
            order.set("profile", profile); // 证书机构说它有这种证书才写（没列出来的写了会被拒）
        } else if ids.iter().any(|id| matches!(id, San::Ip(_))) {
            return Err(Problem(format!("这个证书机构没有给 IP 地址签的那种证书（{profile}）")).into());
        }
        let url = self.dir_url("newOrder");
        let reply = self.post(&url, Some(&V::Dict(order)))?;
        let (order, order_url) = match (reply.dict(), reply.location()) {
            (Some(order), Some(at)) if reply.status == 201 => (order, at.to_string()),
            _ => return Err(Problem(format!("证书机构没接这张单子：{}", reply.why())).into()),
        };
        // 放上去的令牌不管成没成最后都拿掉（Python 的 finally）
        let mut added = Vec::new();
        let done = self.fulfil(&order, &order_url, ids, cert_key, responder, &mut added);
        for token in &added {
            responder.remove(token);
        }
        done
    }

    /// 下了单以后的那几步：一个个地址验证，交证书请求，等签发，取证书。放上去的令牌记进 added
    fn fulfil(
        &mut self,
        order: &Dict,
        order_url: &str,
        ids: &[San],
        cert_key: &Key,
        responder: &dyn Challenges,
        added: &mut Vec<String>,
    ) -> Result<Vec<u8>, Trouble> {
        for url in order.list("authorizations").map(|l| l.items()).unwrap_or_default() {
            let url = url.as_str().unwrap_or("").to_string();
            let reply = self.post(&url, None)?;
            let authz = match reply.dict() {
                Some(authz) if reply.status == 200 => authz,
                _ => return Err(Problem(format!("验证的要求取不到：{}", reply.why())).into()),
            };
            if authz.str("status").as_deref() == Some("valid") {
                continue;
            }
            let challenges = authz.list("challenges").map(|l| l.items()).unwrap_or_default();
            let http01 = challenges.iter().filter_map(V::as_dict).find(|c| c.str("type").as_deref() == Some("http-01"));
            let picked = http01.and_then(|c| {
                let token = c.get("token").map_or_else(String::new, |t| t.py_str());
                let at = c.str("url")?;
                token_re().matches(&token).then(|| (token, at.to_string()))
            });
            let Some((token, challenge_url)) = picked else {
                return Err(Problem("证书机构没给 http-01 这种验证办法".to_string()).into());
            };
            responder.put(&token, format!("{token}.{}", self.key.thumbprint()).as_bytes())?;
            added.push(token);
            let reply = self.post(&challenge_url, Some(&V::Dict(Dict::new())))?;
            if reply.status != 200 {
                return Err(Problem(format!("证书机构没开始验证：{}", reply.why())).into());
            }
            self.wait(&url, "验证这个地址是你的（证书机构从公网连 80 端口）")?;
        }
        let request = csr::csr(cert_key, ids)?;
        let finalize = order.str("finalize").map_or_else(String::new, |u| u.to_string());
        let reply = self.post(&finalize, Some(&V::Dict(dict! { "csr" => b64u(&request) })))?;
        if reply.status != 200 {
            return Err(Problem(format!("证书机构没收证书请求：{}", reply.why())).into());
        }
        let done = self.wait(order_url, "签发证书")?;
        let certificate = done.str("certificate").map_or_else(String::new, |u| u.to_string());
        let reply = self.post(&certificate, None)?;
        let is_pem = reply.json.is_none() && reply.head.body.windows(17).any(|w| w == b"BEGIN CERTIFICATE");
        if reply.status != 200 || !is_pem {
            return Err(Problem(format!("证书取不下来：{}", reply.why())).into());
        }
        Ok(reply.head.body)
    }
}

/// 测试里的假证书机构（http，不带 TLS）：一个账户、一张单子、一个地址。请求照规范核对——签名验得过、nonce 只能用一次、url 要对、
/// 开账户用 jwk 之后用 kid——不对就回错误；各个开关让它在某一步出岔子。mod.rs 的测试也用它
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
pub(super) mod testkit {
    use super::super::fetch::testkit::{Seen, Server};
    use super::*;
    use crate::py::base64;
    use crate::py::hash::sha256;
    use ring::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_FIXED};
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, HashSet};
    use std::rc::Rc;
    use std::sync::Mutex;

    /// 假证书机构给的令牌
    pub const TOKEN: &str = "tok_en-123";

    pub fn unb64u(s: &str) -> Vec<u8> {
        base64::urlsafe_b64decode(&format!("{s}{}", "=".repeat((4 - s.len() % 4) % 4))).unwrap()
    }

    /// 放 http-01 的内容的假地方：记着现在放着什么、放过拿过什么
    #[derive(Clone, Default)]
    pub struct Shelf {
        pub held: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        pub history: Arc<Mutex<Vec<String>>>,
        /// 放不上去（磁盘满了之类）
        pub broken: bool,
    }

    impl Shelf {
        pub fn history(&self) -> Vec<String> {
            self.history.lock().unwrap().clone()
        }
    }

    impl Challenges for Shelf {
        fn put(&self, token: &str, body: &[u8]) -> Result<(), Trouble> {
            if self.broken {
                return Err(Trouble::Os("[Errno 28] No space left on device".to_string()));
            }
            self.history.lock().unwrap().push(format!("put {token}"));
            self.held.lock().unwrap().insert(token.to_string(), body.to_vec());
            Ok(())
        }

        fn remove(&self, token: &str) {
            self.history.lock().unwrap().push(format!("remove {token}"));
            self.held.lock().unwrap().remove(token);
        }
    }

    /// 让假证书机构怎么回
    #[derive(Clone, Default)]
    pub struct Knobs {
        /// 目录里列的证书种类
        pub profiles: Vec<&'static str>,
        /// 第几个 POST（从 1 数起）说 nonce 不对
        pub bad_nonce: Vec<usize>,
        /// 验证、签发各要多取几次才好
        pub pending: usize,
        /// 还没好的那几次回应里的 Retry-After，一次用一个
        pub retry_after: Vec<&'static str>,
        /// 验证不过：里面是证书机构说的原因（None：不说原因）
        pub invalid: Option<Option<&'static str>>,
        /// 不给 http-01 这种验证办法
        pub no_http01: bool,
        /// 这个路径上回错误：(路径, 状态码, 类型, detail)
        pub refuse: Option<(&'static str, u16, &'static str, &'static str)>,
        /// 签出来的证书（PEM）
        pub cert: Vec<u8>,
        /// 目录那个请求原样回这些字节
        pub directory: Option<Vec<u8>>,
    }

    struct Inner {
        knobs: Knobs,
        shelf: Shelf,
        nonces: HashSet<String>,
        made: usize,
        posts: usize,
        /// 账户的公钥那个点、它的摘要
        account: Option<(Vec<u8>, String)>,
        challenged: bool,
        authz_left: usize,
        finalized: bool,
        order_left: usize,
        log: Vec<String>,
    }

    pub struct Ca {
        pub server: Server,
        inner: Arc<Mutex<Inner>>,
    }

    impl Ca {
        pub fn start(knobs: Knobs, shelf: &Shelf) -> Ca {
            let inner = Inner {
                knobs,
                shelf: shelf.clone(),
                nonces: HashSet::new(),
                made: 0,
                posts: 0,
                account: None,
                challenged: false,
                authz_left: 0,
                finalized: false,
                order_left: 0,
                log: Vec::new(),
            };
            let inner = Arc::new(Mutex::new(inner));
            let held = inner.clone();
            Ca { server: Server::start(move |_, seen| handle(&mut held.lock().unwrap(), seen)), inner }
        }

        pub fn directory(&self) -> String {
            self.server.url("/dir")
        }

        /// 做过的事，一行一件
        pub fn log(&self) -> Vec<String> {
            self.inner.lock().unwrap().log.clone()
        }

        /// 收到的请求：方法和路径
        pub fn lines(&self) -> Vec<String> {
            self.server.requests().iter().map(|seen| seen.line().trim_end_matches(" HTTP/1.1").to_string()).collect()
        }

        pub fn set(&self, change: impl FnOnce(&mut Knobs)) {
            change(&mut self.inner.lock().unwrap().knobs);
        }
    }

    fn reply(inner: &mut Inner, status: u16, kind: &str, extra: &[(&str, String)], body: &[u8]) -> Vec<u8> {
        inner.made += 1;
        let nonce = format!("nonce-{}", inner.made);
        inner.nonces.insert(nonce.clone());
        let mut out =
            format!("HTTP/1.1 {status} X\r\nReplay-Nonce: {nonce}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n", body.len());
        for (name, value) in extra {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        out.push_str("\r\n");
        [out.as_bytes(), body].concat()
    }

    fn doc(inner: &mut Inner, status: u16, extra: &[(&str, String)], body: Dict) -> Vec<u8> {
        reply(inner, status, "application/json", extra, json::dumps(&V::Dict(body), Fmt::new()).as_bytes())
    }

    fn problem(inner: &mut Inner, status: u16, kind: &str, detail: &str) -> Vec<u8> {
        inner.log.push(format!("refused {kind}"));
        let body = dict! { "type" => format!("urn:ietf:params:acme:error:{kind}"), "detail" => detail };
        reply(inner, status, "application/problem+json", &[], json::dumps(&V::Dict(body), Fmt::new()).as_bytes())
    }

    /// 核对一个 JWS：给出里面的内容（None 是 POST-as-GET）和它是不是带着公钥（jwk）来的；不对是要回的错误
    fn verify(inner: &mut Inner, url: &str, body: &[u8]) -> Result<(Option<V>, bool), Vec<u8>> {
        let j = json::loads(std::str::from_utf8(body).unwrap()).unwrap().as_dict().unwrap();
        let (p, d, s) = (j.str("protected").unwrap(), j.str("payload").unwrap(), j.str("signature").unwrap());
        let protected = json::loads(std::str::from_utf8(&unb64u(&p)).unwrap()).unwrap().as_dict().unwrap();
        if protected.str("alg").as_deref() != Some("ES256") || protected.str("url").as_deref() != Some(url) {
            return Err(problem(inner, 400, "malformed", "alg 或者 url 不对"));
        }
        inner.posts += 1;
        let fresh = protected.str("nonce").is_some_and(|nonce| inner.nonces.remove(&*nonce));
        if !fresh || inner.knobs.bad_nonce.contains(&inner.posts) {
            return Err(problem(inner, 400, "badNonce", "nonce 不对"));
        }
        let (point, by_jwk) = match (protected.dict("jwk"), protected.str("kid"), &inner.account) {
            (Some(jwk), None, _) => {
                assert_eq!((jwk.str("kty").as_deref(), jwk.str("crv").as_deref()), (Some("EC"), Some("P-256")));
                ([&[4u8][..], &unb64u(&jwk.str("x").unwrap()), &unb64u(&jwk.str("y").unwrap())].concat(), true)
            }
            (None, Some(kid), Some((point, _))) if kid.ends_with("/acct/1") => (point.clone(), false),
            _ => return Err(problem(inner, 400, "malformed", "jwk 和 kid 要有且只有一个")),
        };
        let signed = format!("{p}.{d}");
        if UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &point).verify(signed.as_bytes(), &unb64u(&s)).is_err() {
            return Err(problem(inner, 403, "unauthorized", "签名不对"));
        }
        if by_jwk {
            let jwk = protected.dict("jwk").unwrap();
            let canonical =
                format!(r#"{{"crv":"P-256","kty":"EC","x":"{}","y":"{}"}}"#, jwk.str("x").unwrap(), jwk.str("y").unwrap());
            inner.account = Some((point, b64u(&sha256(canonical.as_bytes()))));
        }
        Ok((if d.is_empty() { None } else { Some(json::loads(std::str::from_utf8(&unb64u(&d)).unwrap()).unwrap()) }, by_jwk))
    }

    fn handle(inner: &mut Inner, seen: &Seen) -> Vec<u8> {
        let base = format!("http://{}", seen.header("Host").unwrap());
        let mut words = seen.line().split(' ');
        let (method, path) = (words.next().unwrap().to_string(), words.next().unwrap().to_string());
        assert_eq!(seen.header("User-Agent"), Some("cca-acme/1"));
        assert_eq!(seen.header("Content-Type"), Some("application/jose+json"));
        match (method.as_str(), path.as_str()) {
            ("GET", "/dir") => {
                if let Some(raw) = inner.knobs.directory.clone() {
                    return raw;
                }
                let profiles = Dict::new();
                for profile in &inner.knobs.profiles {
                    profiles.set(*profile, format!("{base}/docs#{profile}"));
                }
                let body = dict! {
                    "newNonce" => format!("{base}/nonce"), "newAccount" => format!("{base}/acct"), "newOrder" => format!("{base}/order"),
                    "meta" => dict! { "profiles" => profiles },
                };
                doc(inner, 200, &[], body)
            }
            ("HEAD", "/nonce") => reply(inner, 200, "application/json", &[], b""),
            ("POST", _) => {
                let (payload, by_jwk) = match verify(inner, &format!("{base}{path}"), &seen.body) {
                    Ok(verified) => verified,
                    Err(refused) => return refused,
                };
                if let Some((_, status, kind, detail)) = inner.knobs.refuse.filter(|(at, _, _, _)| *at == path) {
                    return problem(inner, status, kind, detail);
                }
                route(inner, &base, &path, payload, by_jwk)
            }
            _ => reply(inner, 404, "text/plain", &[], b"not found"),
        }
    }

    fn route(inner: &mut Inner, base: &str, path: &str, payload: Option<V>, by_jwk: bool) -> Vec<u8> {
        let said = payload.as_ref().map(|p| json::dumps(p, Fmt::new()));
        let challenge = || dict! { "type" => "http-01", "url" => format!("{base}/chall/1"), "token" => TOKEN };
        match path {
            "/acct" => {
                if !by_jwk || said.as_deref() != Some(r#"{"termsOfServiceAgreed": true}"#) {
                    return problem(inner, 400, "malformed", "开账户要用 jwk，并且同意条款");
                }
                inner.log.push("account".to_string());
                doc(inner, 201, &[("Location", format!("{base}/acct/1"))], dict! { "status" => "valid" })
            }
            _ if by_jwk => problem(inner, 400, "malformed", "这个请求要用 kid"),
            "/order" => {
                inner.log.push(format!("order {}", said.unwrap_or_default()));
                let body = dict! {
                    "status" => "pending", "authorizations" => crate::py::obj::str_list([format!("{base}/authz/1")]),
                    "finalize" => format!("{base}/fin/1"),
                };
                doc(inner, 201, &[("Location", format!("{base}/o/1"))], body)
            }
            "/authz/1" if payload.is_none() => {
                let mut extra = Vec::new();
                let status = if !inner.challenged {
                    "pending"
                } else if inner.knobs.invalid.is_some() {
                    "invalid"
                } else if inner.authz_left > 0 {
                    inner.authz_left -= 1;
                    if !inner.knobs.retry_after.is_empty() {
                        extra.push(("Retry-After", inner.knobs.retry_after.remove(0).to_string()));
                    }
                    "pending"
                } else {
                    "valid"
                };
                let offered = challenge();
                if let Some(Some(detail)) = inner.knobs.invalid.filter(|_| inner.challenged) {
                    offered.set("error", dict! { "type" => "urn:ietf:params:acme:error:unauthorized", "detail" => detail });
                }
                let other = dict! { "type" => "dns-01", "url" => format!("{base}/chall/2"), "token" => "other" };
                let challenges = if inner.knobs.no_http01 { vec![V::Dict(other)] } else { vec![V::Dict(other), V::Dict(offered)] };
                doc(inner, 200, &extra, dict! { "status" => status, "challenges" => challenges })
            }
            "/chall/1" if said.as_deref() == Some("{}") => {
                // http-01：应答那头现在放着的要是“令牌.账户公钥的摘要”
                let want = format!("{TOKEN}.{}", inner.account.as_ref().unwrap().1);
                let ok = inner.shelf.held.lock().unwrap().get(TOKEN).map(Vec::as_slice) == Some(want.as_bytes());
                inner.log.push(format!("validated {ok}"));
                if !ok && inner.knobs.invalid.is_none() {
                    inner.knobs.invalid = Some(Some("取到的内容不对"));
                }
                inner.challenged = true;
                inner.authz_left = inner.knobs.pending;
                doc(inner, 200, &[], challenge())
            }
            "/fin/1" => {
                let csr = unb64u(&payload.and_then(|p| p.as_dict()).and_then(|p| p.str("csr")).unwrap());
                inner.log.push(format!("finalize {}", csr.len()));
                assert_eq!(csr[0], 0x30);
                inner.finalized = true;
                inner.order_left = inner.knobs.pending;
                doc(inner, 200, &[], dict! { "status" => "processing" })
            }
            "/o/1" if payload.is_none() && inner.finalized => {
                if inner.order_left > 0 {
                    inner.order_left -= 1;
                    let mut extra = Vec::new();
                    if !inner.knobs.retry_after.is_empty() {
                        extra.push(("Retry-After", inner.knobs.retry_after.remove(0).to_string()));
                    }
                    return doc(inner, 200, &extra, dict! { "status" => "processing" });
                }
                doc(inner, 200, &[], dict! { "status" => "valid", "certificate" => format!("{base}/cert/1") })
            }
            "/cert/1" if payload.is_none() => {
                inner.log.push("issued".to_string());
                let cert = inner.knobs.cert.clone();
                reply(inner, 200, "application/pem-certificate-chain", &[], &cert)
            }
            _ => problem(inner, 404, "malformed", "没有这个地址"),
        }
    }

    /// 假的钟：睡多久就过多久，睡过的记下来
    pub fn fake_pace() -> (Pace, Rc<RefCell<Vec<f64>>>) {
        let (now, naps) = (Rc::new(Cell::new(1000.0)), Rc::new(RefCell::new(Vec::new())));
        let (read, moved, noted) = (now.clone(), now, naps.clone());
        let sleep = move |secs: f64| {
            noted.borrow_mut().push(secs);
            moved.set(moved.get() + secs);
        };
        (Pace { monotonic: Box::new(move || read.get()), sleep: Box::new(sleep) }, naps)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::super::fetch::testkit::{no_roots, Server};
    use super::super::key::pem;
    use super::testkit::{fake_pace, unb64u, Ca, Knobs, Shelf, TOKEN};
    use super::*;
    use crate::py::fs::TempDir;
    use crate::server::x509::testkit as certs;

    /// 账户的私钥、证书的私钥，和假证书机构签给 ids 的那张证书
    struct Kit {
        _dir: TempDir,
        account: Key,
        cert_key: Key,
        cert: Vec<u8>,
    }

    fn kit(ids: &[San]) -> Kit {
        let dir = TempDir::new().unwrap();
        let account = Key::open(&format!("{}/account.key", dir.path), true).unwrap();
        let pkcs8 = certs::new_key();
        let at = format!("{}/privkey.pem", dir.path);
        std::fs::write(&at, pem("PRIVATE KEY", &pkcs8)).unwrap();
        let cert = certs::pem("CERTIFICATE", &certs::self_signed(&pkcs8, Some(ids))).into_bytes();
        Kit { _dir: dir, account, cert_key: Key::open(&at, false).unwrap(), cert }
    }

    fn ip() -> Vec<San> {
        vec![San::Ip("192.0.2.10".to_string())]
    }

    fn knobs(kit: &Kit) -> Knobs {
        Knobs { profiles: vec!["classic", "shortlived"], cert: kit.cert.clone(), ..Knobs::default() }
    }

    fn problem<T: std::fmt::Debug>(r: Result<T, Trouble>) -> String {
        match r {
            Err(Trouble::Problem(why)) => why,
            other => panic!("要的是 Problem，得到 {other:?}"),
        }
    }

    const VALIDATING: &str = "验证这个地址是你的（证书机构从公网连 80 端口）";

    /// 要一张证书的全过程：先取目录、nonce，开账户，下单，取验证的要求，放上应答、让证书机构来验，交证书请求，取证书。
    /// 假证书机构核对每个请求的签名、nonce、url；令牌验完拿掉。同一段对话里再要一张：不再开账户，验证过的地址不再验
    #[test]
    fn issues_a_certificate() {
        let kit = kit(&ip());
        let shelf = Shelf::default();
        let ca = Ca::start(knobs(&kit), &shelf);
        let (pace, naps) = fake_pace();
        let mut acme = Acme::with(&ca.directory(), &kit.account, no_roots(), pace).unwrap();
        assert_eq!(acme.issue(&ip(), "shortlived", &kit.cert_key, &shelf).unwrap(), kit.cert);
        assert_eq!(
            ca.lines(),
            [
                "GET /dir",
                "POST /acct",
                "POST /order",
                "POST /authz/1",
                "POST /chall/1",
                "POST /authz/1",
                "POST /fin/1",
                "POST /o/1",
                "POST /cert/1"
            ],
            "取目录的那个回应里就带着 nonce：不用另取"
        );
        let log = ca.log();
        assert_eq!(log[0], "account");
        assert_eq!(log[1], r#"order {"identifiers": [{"type": "ip", "value": "192.0.2.10"}], "profile": "shortlived"}"#);
        assert_eq!((log[2].as_str(), log[4].as_str(), log.len()), ("validated true", "issued", 5), "{log:?}");
        assert!(log[3].starts_with("finalize "));
        assert_eq!(shelf.history(), [format!("put {TOKEN}"), format!("remove {TOKEN}")]);
        assert!(naps.borrow().is_empty(), "一取就好了的不用等");
        // 开账户的那个请求带的是公钥（JWK，各项的先后照 Python），之后的带账户的网址
        let requests = ca.server.requests();
        let protected = |n: usize| {
            let body = json::loads(std::str::from_utf8(&requests[n].body).unwrap()).unwrap().as_dict().unwrap();
            String::from_utf8(unb64u(&body.str("protected").unwrap())).unwrap()
        };
        let jwk = json::dumps(&V::Dict(kit.account.jwk()), Fmt::new());
        assert_eq!(
            protected(1),
            format!(r#"{{"alg": "ES256", "nonce": "nonce-1", "url": "{}", "jwk": {jwk}}}"#, ca.server.url("/acct"))
        );
        let kid = ca.server.url("/acct/1");
        assert_eq!(
            protected(2),
            format!(r#"{{"alg": "ES256", "nonce": "nonce-2", "url": "{}", "kid": "{kid}"}}"#, ca.server.url("/order"))
        );
        // 再要一张
        assert_eq!(acme.issue(&ip(), "shortlived", &kit.cert_key, &shelf).unwrap(), kit.cert);
        assert_eq!(ca.lines()[9..], ["POST /order", "POST /authz/1", "POST /fin/1", "POST /o/1", "POST /cert/1"]);
        assert_eq!(shelf.history().len(), 2);
    }

    /// 证书机构说 nonce 不对：照它新给的再发，最多发三次
    #[test]
    fn bad_nonce_is_retried() {
        let kit = kit(&ip());
        let shelf = Shelf::default();
        let ca = Ca::start(Knobs { bad_nonce: vec![1, 2, 5], ..knobs(&kit) }, &shelf);
        let mut acme = Acme::with(&ca.directory(), &kit.account, no_roots(), fake_pace().0).unwrap();
        assert_eq!(acme.issue(&ip(), "shortlived", &kit.cert_key, &shelf).unwrap(), kit.cert);
        let lines = ca.lines();
        assert_eq!(
            lines[..7],
            ["GET /dir", "POST /acct", "POST /acct", "POST /acct", "POST /order", "POST /authz/1", "POST /authz/1"]
        );
        assert!(!lines.contains(&"HEAD /nonce".to_string()), "说不对的那个回应里带着新的 nonce：不用另取");
        assert_eq!(ca.log().iter().filter(|l| *l == "refused badNonce").count(), 3);
        // 连着三次都不对：不再发了
        let ca = Ca::start(Knobs { bad_nonce: vec![1, 2, 3, 4], ..knobs(&kit) }, &shelf);
        let mut acme = Acme::with(&ca.directory(), &kit.account, no_roots(), fake_pace().0).unwrap();
        assert_eq!(
            problem(acme.issue(&ip(), "shortlived", &kit.cert_key, &shelf)),
            "证书机构没给开账户：badNonce nonce 不对（400）"
        );
        assert_eq!(ca.lines().iter().filter(|l| *l == "POST /acct").count(), 3);
    }

    /// 还没好就等一等再取：等多久照 Retry-After（1 到 10 秒之间；没给、看不懂的是 2 秒）
    #[test]
    fn waits_as_long_as_the_authority_says() {
        let kit = kit(&ip());
        let shelf = Shelf::default();
        let ca = Ca::start(Knobs { pending: 3, retry_after: vec!["0", "100", "abc", "", " 3.5 ", "nan"], ..knobs(&kit) }, &shelf);
        let (pace, naps) = fake_pace();
        let mut acme = Acme::with(&ca.directory(), &kit.account, no_roots(), pace).unwrap();
        assert_eq!(acme.issue(&ip(), "shortlived", &kit.cert_key, &shelf).unwrap(), kit.cert);
        assert_eq!(*naps.borrow(), [1.0, 10.0, 2.0, 2.0, 3.5, 2.0]);
        assert_eq!(ca.lines().iter().filter(|l| *l == "POST /authz/1").count(), 5);
    }

    /// 一直不好：等够 90 秒就算没成；令牌照样拿掉
    #[test]
    fn gives_up_after_the_poll_time() {
        let kit = kit(&ip());
        let shelf = Shelf::default();
        let ca = Ca::start(Knobs { pending: 1000, retry_after: vec!["10"; 20], ..knobs(&kit) }, &shelf);
        let (pace, naps) = fake_pace();
        let mut acme = Acme::with(&ca.directory(), &kit.account, no_roots(), pace).unwrap();
        assert_eq!(problem(acme.issue(&ip(), "shortlived", &kit.cert_key, &shelf)), format!("{VALIDATING}等了 90 秒还没好"));
        assert_eq!(*naps.borrow(), [10.0; 10], "过了 90 秒以后的那一次取完就不等了");
        assert_eq!(shelf.history(), [format!("put {TOKEN}"), format!("remove {TOKEN}")]);
    }

    /// 验证没过：说证书机构给的原因；没给原因也说得清
    #[test]
    fn validation_failures_say_why() {
        let kit = kit(&ip());
        let shelf = Shelf::default();
        let ca = Ca::start(Knobs { invalid: Some(Some("连不上 80 端口\r\n（ConnectionRefusedError）")), ..knobs(&kit) }, &shelf);
        let mut acme = Acme::with(&ca.directory(), &kit.account, no_roots(), fake_pace().0).unwrap();
        assert_eq!(
            problem(acme.issue(&ip(), "shortlived", &kit.cert_key, &shelf)),
            format!("{VALIDATING}没过：unauthorized 连不上 80 端口  （ConnectionRefusedError）（200）")
        );
        assert!(shelf.held.lock().unwrap().is_empty());
        assert!(!ca.lines().contains(&"POST /fin/1".to_string()), "没验过就不交证书请求");
        let ca = Ca::start(Knobs { invalid: Some(None), ..knobs(&kit) }, &shelf);
        let mut acme = Acme::with(&ca.directory(), &kit.account, no_roots(), fake_pace().0).unwrap();
        assert_eq!(problem(acme.issue(&ip(), "shortlived", &kit.cert_key, &shelf)), format!("{VALIDATING}没过：证书机构没说原因"));
        // 应答放不上去（写不了文件）：不是“没要到、过一阵再试”的那一类
        let ca = Ca::start(knobs(&kit), &shelf);
        let mut acme = Acme::with(&ca.directory(), &kit.account, no_roots(), fake_pace().0).unwrap();
        let broken = Shelf { broken: true, ..Shelf::default() };
        assert_eq!(
            acme.issue(&ip(), "shortlived", &kit.cert_key, &broken).unwrap_err(),
            Trouble::Os("[Errno 28] No space left on device".to_string())
        );
        assert!(!ca.lines().contains(&"POST /chall/1".to_string()));
    }

    /// 要哪一种证书：证书机构列了才写；给 IP 地址签的没有那一种就不下单
    #[test]
    fn profile_only_when_offered() {
        let names = vec![San::Dns("relay.example.com".to_string())];
        let kit = kit(&names);
        let shelf = Shelf::default();
        let ca = Ca::start(Knobs { profiles: vec!["classic"], ..knobs(&kit) }, &shelf);
        let mut acme = Acme::with(&ca.directory(), &kit.account, no_roots(), fake_pace().0).unwrap();
        assert_eq!(
            problem(acme.issue(&ip(), "shortlived", &kit.cert_key, &shelf)),
            "这个证书机构没有给 IP 地址签的那种证书（shortlived）"
        );
        assert_eq!(problem(acme.issue(&ip(), "", &kit.cert_key, &shelf)), "这个证书机构没有给 IP 地址签的那种证书（）");
        assert_eq!(ca.lines(), ["GET /dir", "POST /acct"], "没有那一种就不下单");
        acme.issue(&names, "shortlived", &kit.cert_key, &shelf).unwrap();
        assert_eq!(ca.log()[1], r#"order {"identifiers": [{"type": "dns", "value": "relay.example.com"}]}"#);
        // 两个地址、要的是它有的那一种
        let both = [ip(), names.clone()].concat();
        let ca = Ca::start(Knobs { profiles: vec!["classic"], ..knobs(&kit) }, &shelf);
        let mut acme = Acme::with(&ca.directory(), &kit.account, no_roots(), fake_pace().0).unwrap();
        acme.issue(&both, "classic", &kit.cert_key, &shelf).unwrap();
        assert_eq!(
            ca.log()[1],
            r#"order {"identifiers": [{"type": "ip", "value": "192.0.2.10"}, {"type": "dns", "value": "relay.example.com"}], "profile": "classic"}"#
        );
    }

    /// 证书机构在各步上不答应：说是哪一步、它给的原因
    #[test]
    fn refusals_name_the_step() {
        let kit = kit(&ip());
        let shelf = Shelf::default();
        let said = |refuse: (&'static str, u16, &'static str, &'static str), change: &dyn Fn(&mut Knobs)| {
            let mut k = Knobs { refuse: Some(refuse), ..knobs(&kit) };
            change(&mut k);
            let ca = Ca::start(k, &shelf);
            let mut acme = Acme::with(&ca.directory(), &kit.account, no_roots(), fake_pace().0).unwrap();
            let why = problem(acme.issue(&ip(), "shortlived", &kit.cert_key, &shelf));
            assert!(shelf.held.lock().unwrap().is_empty(), "{why}：令牌没拿掉");
            why
        };
        let same = |_: &mut Knobs| {};
        assert_eq!(said(("/acct", 403, "unauthorized", "账户停用了"), &same), "证书机构没给开账户：unauthorized 账户停用了（403）");
        assert_eq!(
            said(("/order", 429, "rateLimited", "too many certificates"), &same),
            "证书机构没接这张单子：rateLimited too many certificates（429）"
        );
        assert_eq!(said(("/authz/1", 500, "serverInternal", ""), &same), "验证的要求取不到：serverInternal（500）");
        assert_eq!(said(("/chall/1", 400, "malformed", "x"), &same), "证书机构没开始验证：malformed x（400）");
        assert_eq!(
            said(("/fin/1", 403, "orderNotReady", "地址还没验证完"), &same),
            "证书机构没收证书请求：orderNotReady 地址还没验证完（403）"
        );
        assert_eq!(said(("/o/1", 503, "serverInternal", "busy"), &same), "签发证书：serverInternal busy（503）");
        assert_eq!(said(("/cert/1", 404, "malformed", "没有"), &same), "证书取不下来：malformed 没有（404）");
        // 不是出错、但给的东西不对
        assert_eq!(said(("/none", 200, "", ""), &|k| k.no_http01 = true), "证书机构没给 http-01 这种验证办法");
        assert_eq!(said(("/none", 200, "", ""), &|k| k.cert = b"not a certificate".to_vec()), "证书取不下来：状态码 200");
    }

    /// 目录取不到、不对；连不上
    #[test]
    fn directory_problems() {
        let kit = kit(&ip());
        let open = |raw: &str| {
            let ca = Ca::start(Knobs { directory: Some(raw.as_bytes().to_vec()), ..Knobs::default() }, &Shelf::default());
            Acme::with(&ca.directory(), &kit.account, no_roots(), fake_pace().0).err().map(|p| p.0)
        };
        let json = |body: &str| {
            format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len())
        };
        assert_eq!(open("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").as_deref(), Some("证书机构的目录读不了（404）"));
        assert_eq!(open(&json(r#"{"newNonce": "a", "newAccount": "b"}"#)).as_deref(), Some("证书机构的目录读不了（200）"));
        assert_eq!(
            open(&json(r#"{"newNonce": "a", "newAccount": "b", "newOrder": 5}"#)).as_deref(),
            Some("证书机构的目录读不了（200）")
        );
        assert_eq!(open(&json(r#"["newNonce", "newAccount", "newOrder"]"#)).as_deref(), Some("证书机构的目录读不了（200）"));
        assert_eq!(open(&json("{not json")).as_deref(), Some("证书机构的目录读不了（200）"));
        let text = r#"{"newNonce": "a", "newAccount": "b", "newOrder": "c"}"#;
        let plain = format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{text}", text.len());
        assert_eq!(open(&plain).as_deref(), Some("证书机构的目录读不了（200）"), "Content-Type 里没有 json 就不当 JSON 看");
        assert_eq!(open(&json(text)), None);
        assert_eq!(open("garbage\r\n\r\n").as_deref(), Some("连不上证书机构（BadStatusLine）"));
        // 那个端口上没人听
        assert_eq!(
            Acme::with("http://127.0.0.1:1/dir", &kit.account, no_roots(), fake_pace().0).err().map(|p| p.0).as_deref(),
            Some("连不上证书机构（ConnectionRefused）")
        );
        assert_eq!(
            Acme::with("ftp://ca.example.com/dir", &kit.account, no_roots(), fake_pace().0).err().map(|p| p.0).as_deref(),
            Some("连不上证书机构（InvalidUrl）")
        );
        // --ca 指的文件读不了：不是 Problem（入口说“起不来”）
        match Acme::new("http://127.0.0.1:1/dir", &kit.account, Some("/nonexistent/ca.pem")) {
            Err(Trouble::Os(why)) => assert_eq!(why, "[Errno 2] No such file or directory"),
            other => panic!("{:?}", other.err()),
        }
    }

    /// 手里没有 nonce 就先去取一个（HEAD newNonce）；证书机构没给：不发
    #[test]
    fn no_nonce() {
        let kit = kit(&ip());
        let dir = r#"{"newNonce": "http://HOST/nonce", "newAccount": "http://HOST/acct", "newOrder": "http://HOST/order"}"#;
        let server = Server::start(move |_, seen| {
            if seen.line().starts_with("GET ") {
                let body = dir.replace("HOST", seen.header("Host").unwrap());
                return format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .into_bytes();
            }
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec()
        });
        let mut acme = Acme::with(&server.url("/dir"), &kit.account, no_roots(), fake_pace().0).unwrap();
        assert_eq!(problem(acme.issue(&ip(), "shortlived", &kit.cert_key, &Shelf::default())), "证书机构没给 nonce");
        assert_eq!(server.requests().len(), 2);
    }

    /// 证书机构说的原因：类型只要最后一段，连上 detail；控制字符换成空格、最长 300 个字；不是那个样子的只说状态码
    #[test]
    fn why_like_python() {
        let of = |status: u16, text: &str| why(status, Some(&json::loads(text).unwrap()));
        assert_eq!(
            of(403, r#"{"type": "urn:ietf:params:acme:error:unauthorized", "detail": "No.\nReally."}"#),
            "unauthorized No. Really.（403）"
        );
        assert_eq!(of(400, r#"{"type": "badNonce"}"#), "badNonce（400）");
        assert_eq!(of(400, r#"{"detail": "只有这一句"}"#), "只有这一句（400）");
        assert_eq!(of(400, r#"{"type": null, "detail": 5}"#), "None 5（400）");
        assert_eq!(of(400, r#"{"type": "a:b:", "detail": ["x"]}"#), "['x']（400）");
        assert_eq!(of(500, "{}"), "状态码 500");
        assert_eq!(of(500, &format!(r#"{{"type": "", "detail": " \t{}"}}"#, '\x7f')), "状态码 500");
        assert_eq!(of(500, r#"["type"]"#), "状态码 500");
        assert_eq!(of(500, r#""text""#), "状态码 500");
        assert_eq!(why(502, None), "状态码 502");
        let long = of(400, &format!(r#"{{"type": "x:kind", "detail": "{}"}}"#, "长".repeat(400)));
        assert_eq!(long, format!("kind {}（400）", "长".repeat(295)));
    }
}
