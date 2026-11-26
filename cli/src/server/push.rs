//! 推送（docs/dev/push.md）：手机锁着屏、App 没开着时也收得到会话里的通知。会话宿主经控制连接交来 PUSH（`{"pid", "id", "note"}`：
//! 一条通知封给一个配对的手机，这里解不开），这台服务器照那台手机登记的推送令牌（`push_set`，记在 remote.json 的访客那一项里）
//! 转给推送网关（`--push`：维护者的服务器上的 `cca-core team-push`，推送服务的密钥只在它那里）。这台服务器知道的只是“这台手机
//! 有一条通知”，网关知道的是“这个令牌有一条通知”，都看不到里面写的什么。
//!
//! 一条线程、一个有界的队列：中转的线程只往队列里放（`Pusher::send`：满了就丢，不等，那条线程不能被网络拖住）；这条线程一条条
//! POST 给网关，一条一个连接（HTTP/1.0 那样回完就关；通知不多，握手的那点开销不算什么），钉住网关证书的公钥（`--push-pin`，
//! curl 的 --pinnedpubkey 那种写法），不看证书机构。每台手机（令牌）限流：攒得下 `PHONE_BURST` 条、`PHONE_GAP` 秒回一条。
//! 网关回 410（推送服务说这个令牌没用了：App 删了、换了手机）就把这个登记拿掉；别的出错在日志里说一句（一分钟最多一句，不带令牌）。
//!
//! 额度的通知（3.7.5 起）：宿主经 push_note 交来的（额度用完了）也放进这个队列；到点再推的（status_put 的 later：额度恢复的那一刻）
//! 记在 Remote 里，这条线程最多隔 `LATER_POLL` 秒看一眼，到点的拿出来推（`Remote::due`）。

use super::remote::Shared;
use super::wire::Blob;
use super::{x509, LATER_POLL};
use crate::py::json::{self, Fmt};
use crate::py::obj::V;
use crate::py::time::{self, monotonic};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::Duration;

/// 队列多长（满了新来的丢掉）
const QUEUE: usize = 256;
/// 每台手机攒得下几条、多少秒回一条（宿主那边每个会话还有自己的限流：NOTE_BURST、NOTE_GAP）
const PHONE_BURST: f64 = 20.0;
const PHONE_GAP: f64 = 3.0;
/// 连网关、等它回最多多久（秒）
const CONNECT_WAIT: u64 = 5;
const REPLY_WAIT: u64 = 30;
/// 网关回的最多读这么多
const REPLY_MAX: usize = 16 * 1024;
/// 日志里出错的那一句最多多久说一次（秒）
const SAY_EVERY: f64 = 60.0;

/// 推送网关在哪、钉住的公钥
#[derive(Clone, Debug, PartialEq)]
pub struct Gateway {
    pub host: String,
    pub port: u16,
    /// 证书里公钥（SubjectPublicKeyInfo 的 DER）的 SHA-256
    pub pin: [u8; 32],
}

impl Gateway {
    /// `--push https://主机[:端口]`（没写端口是 443；主机可以是 IP、名字，IPv6 写在方括号里）和 `--push-pin sha256//base64`。
    /// 不对的是 Err（说哪里不对）
    pub fn parse(url: &str, pin: &str) -> Result<Gateway, String> {
        let (host, port) = https_addr(url).ok_or_else(|| format!("--push 要写成 https://主机[:端口]：{url}"))?;
        let pin = pin
            .strip_prefix("sha256//")
            .and_then(|b| crate::py::base64::b64decode(b, true).ok())
            .and_then(|raw| <[u8; 32]>::try_from(raw).ok())
            .ok_or_else(|| format!("--push-pin 要写成 sha256// 加证书公钥的 SHA-256 的 base64（44 个字符）：{pin}"))?;
        Ok(Gateway { host, port, pin })
    }

    /// 日志里说的：主机:端口
    pub fn shown(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// `https://主机[:端口][/]` 里的主机和端口（没写端口是 443；主机可以是 IP、名字，IPv6 写在方括号里）。推送网关的地址（--push）、
/// 网关自己连的推送服务（gateway.rs 的 --apns）都这么写；不对是 None
pub fn https_addr(url: &str) -> Option<(String, u16)> {
    let rest = url.strip_prefix("https://")?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let (host, port) = match rest.strip_prefix('[') {
        Some(v6) => {
            let (host, after) = v6.split_once(']')?;
            match after {
                "" => (host, None),
                _ => (host, Some(after.strip_prefix(':')?)),
            }
        }
        None => match rest.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (rest, None),
        },
    };
    let port = match port {
        None => 443,
        Some(p) => p.parse::<u16>().ok().filter(|p| *p != 0)?,
    };
    (!host.is_empty() && !host.contains(['/', '@', '?', '#', ' ', '[', ']'])).then(|| (host.to_string(), port))
}

/// 交给网关的一条：哪个令牌（推送服务的哪一种环境）、宿主给的编号、封好的通知
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub token: String,
    pub env: String,
    pub id: String,
    pub note: Blob,
}

/// 中转的线程拿着的那一头
#[derive(Clone, Debug)]
pub struct Pusher {
    tx: SyncSender<Job>,
}

impl Pusher {
    /// 放进队列：满了（网关慢、一下子来得太多）就丢掉这一条，不等。那条线程没了（不会）也一样
    pub fn send(&self, job: Job) {
        match self.tx.try_send(job) {
            Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// 测试里：不起线程，交给它的都从 rx 里拿
    #[cfg(test)]
    pub fn for_test() -> (Pusher, Receiver<Job>) {
        let (tx, rx) = sync_channel(QUEUE);
        (Pusher { tx }, rx)
    }
}

/// 起那条线程
pub fn start(gw: Gateway, sh: Arc<Shared>) -> io::Result<Pusher> {
    let config = pinned(gw.pin).map_err(io::Error::other)?;
    let (tx, rx) = sync_channel(QUEUE);
    std::thread::Builder::new().name("push".into()).spawn(move || run(&gw, &config, &sh, &rx))?;
    Ok(Pusher { tx })
}

fn run(gw: &Gateway, config: &Arc<ClientConfig>, sh: &Shared, rx: &Receiver<Job>) {
    let mut budget: HashMap<String, (f64, f64)> = HashMap::new();
    let mut said = f64::NEG_INFINITY;
    loop {
        // 等队列里来一条，最多等到下一条到点再推的通知（最多 LATER_POLL 秒：中间新放上来的早一点的也赶得上）
        let next = sh.lock().next_due();
        let wait = next.map_or(LATER_POLL, |at| (at - time::time()).clamp(0.0, LATER_POLL));
        let wait = Duration::try_from_secs_f64(wait).unwrap_or(Duration::from_secs(1));
        let mut jobs = match rx.recv_timeout(wait) {
            Ok(job) => vec![job],
            Err(RecvTimeoutError::Timeout) => Vec::new(),
            Err(RecvTimeoutError::Disconnected) => return,
        };
        jobs.extend(sh.lock().due(time::time()));
        for job in jobs {
            deliver(gw, config, sh, job, &mut budget, &mut said);
        }
    }
}

/// 交一条给网关（每台手机限流）：网关说令牌没用了就拿掉登记，别的出错在日志里说一句（一分钟最多一句）
fn deliver(
    gw: &Gateway,
    config: &Arc<ClientConfig>,
    sh: &Shared,
    job: Job,
    budget: &mut HashMap<String, (f64, f64)>,
    said: &mut f64,
) {
    let now = monotonic();
    if !take(budget, &job.token, now, PHONE_BURST, PHONE_GAP) {
        return;
    }
    let body = json::dumps(
        &V::Dict(crate::dict! { "v" => 1, "token" => &job.token, "env" => &job.env, "id" => &job.id, "note" => job.note.to_v() }),
        Fmt::new().compact(),
    );
    let why = match post(gw, config, body.as_bytes()) {
        Ok(200) => None,
        Ok(410) => {
            sh.lock().push_gone(&job.token);
            None
        }
        Ok(code) => Some(format!("回 {code}")),
        Err(e) => Some(format!("连不上、没回（{e}）")),
    };
    if let Some(why) = why.filter(|_| now - *said >= SAY_EVERY) {
        *said = now;
        crate::err!("cca-presence: 推送网关 {} {why}，这一条没推出去", gw.shown());
    }
}

/// 限流的桶：key（令牌、IP）还有没有额度（有的话用掉一条）。攒得下 burst 条、gap 秒回一条；记着的太多时，额度攒满了的那些不用记了。
/// 推送网关（gateway.rs）也用它
pub(super) fn take(budget: &mut HashMap<String, (f64, f64)>, key: &str, now: f64, burst: f64, gap: f64) -> bool {
    if budget.len() > 4096 {
        budget.retain(|_, (left, at)| *left + (now - *at) / gap < burst);
    }
    let (left, at) = budget.entry(key.to_string()).or_insert((burst, now));
    *left = (*left + (now - *at) / gap).min(burst);
    *at = now;
    if *left < 1.0 {
        return false;
    }
    *left -= 1.0;
    true
}

/// 连上网关、POST 一条、读完它回的：状态码
fn post(gw: &Gateway, config: &Arc<ClientConfig>, body: &[u8]) -> io::Result<u16> {
    let addr = (gw.host.as_str(), gw.port).to_socket_addrs()?.next().ok_or_else(|| io::Error::other("查不到地址"))?;
    let sock = TcpStream::connect_timeout(&addr, Duration::from_secs(CONNECT_WAIT))?;
    sock.set_read_timeout(Some(Duration::from_secs(REPLY_WAIT)))?;
    sock.set_write_timeout(Some(Duration::from_secs(REPLY_WAIT)))?;
    let name = ServerName::try_from(gw.host.clone()).map_err(|_| io::Error::other("主机名不对"))?;
    let conn = ClientConnection::new(Arc::clone(config), name).map_err(io::Error::other)?;
    let mut tls = StreamOwned::new(conn, sock);
    let mut req = format!(
        "POST /v1/push HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        gw.shown(),
        body.len()
    )
    .into_bytes();
    req.extend_from_slice(body);
    tls.write_all(&req)?;
    tls.flush()?;
    let mut reply = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match tls.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => reply.extend_from_slice(buf.get(..n).unwrap_or_default()),
            // 网关回完就关，没说 close_notify 也算读完了
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        }
        if reply.len() > REPLY_MAX {
            break;
        }
    }
    status(&reply).ok_or_else(|| io::Error::other("回的不是 HTTP"))
}

/// `HTTP/1.x 状态码 …` 那一行里的状态码
fn status(reply: &[u8]) -> Option<u16> {
    let line = reply.split(|b| *b == b'\n').next()?;
    let text = std::str::from_utf8(line).ok()?;
    let mut parts = text.split_whitespace();
    parts.next().filter(|v| v.starts_with("HTTP/1."))?;
    parts.next()?.parse().ok()
}

/// 只认钉住的那把公钥的客户端配置（P-256：网关的证书是安装脚本用 openssl 现造的）
fn pinned(pin: [u8; 32]) -> Result<Arc<ClientConfig>, rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned { pin }))
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// 证书只看公钥：它的 SHA-256 要是钉住的那个；握手的签名拿证书里的公钥验（证书机构、名字、有效期都不看）
#[derive(Debug)]
struct Pinned {
    pin: [u8; 32],
}

impl Pinned {
    fn check(
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let bad = || rustls::Error::General("握手的签名不对".into());
        let spki = x509::cert_spki(cert.as_ref()).ok_or_else(bad)?;
        // P-256 的公钥：91 个字节，最后 65 个是那个点
        let point = spki.get(26..).filter(|p| spki.len() == 91 && p.first() == Some(&4)).ok_or_else(bad)?;
        ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_ASN1, point)
            .verify(message, dss.signature())
            .map(|()| HandshakeSignatureValid::assertion())
            .map_err(|_| bad())
    }
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match x509::cert_spki(end_entity.as_ref()).map(crate::py::hash::sha256) {
            Some(h) if h == self.pin => Ok(ServerCertVerified::assertion()),
            _ => Err(rustls::Error::General("不是钉住的那把公钥".into())),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Pinned::check(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Pinned::check(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ECDSA_NISTP256_SHA256]
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn gateway_addresses() {
        let pin = format!("sha256//{}", crate::py::base64::b64encode(&[7u8; 32]));
        let gw = Gateway::parse("https://push.example.com:8443/", &pin).unwrap();
        assert_eq!((gw.host.as_str(), gw.port, gw.pin), ("push.example.com", 8443, [7u8; 32]));
        assert_eq!(Gateway::parse("https://192.0.2.10", &pin).unwrap().shown(), "192.0.2.10:443");
        assert_eq!(Gateway::parse("https://[2001:db8::1]:9", &pin).unwrap().shown(), "[2001:db8::1]:9");
        for bad in [
            "http://192.0.2.10",
            "https://",
            "https://192.0.2.10:0",
            "https://a/b",
            "https://u@h",
            "https://h:x",
            "https://[::1]x",
            "https://[::1",
        ] {
            assert!(Gateway::parse(bad, &pin).is_err(), "{bad}");
        }
        for bad in ["", "sha256//", "sha1//AAAA", &pin[..pin.len() - 4]] {
            assert!(Gateway::parse("https://h", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn each_phone_has_its_own_budget() {
        let mut b = HashMap::new();
        let mut t = |key: &str, now: f64| take(&mut b, key, now, PHONE_BURST, PHONE_GAP);
        let sent = (0..30).filter(|_| t("a", 100.0)).count();
        assert_eq!(sent, PHONE_BURST as usize);
        assert!(t("b", 100.0), "别的手机不受影响");
        assert!(!t("a", 100.0 + PHONE_GAP * 0.5));
        assert!(t("a", 100.0 + PHONE_GAP * 1.01));
    }

    #[test]
    fn status_lines() {
        assert_eq!(status(b"HTTP/1.0 200 OK\r\nServer: x\r\n\r\n{}"), Some(200));
        assert_eq!(status(b"HTTP/1.1 410 Gone\r\n"), Some(410));
        for bad in [&b""[..], b"SSH-2.0-x\r\n", b"HTTP/2 200\r\n", b"HTTP/1.0 abc\r\n"] {
            assert_eq!(status(bad), None);
        }
    }
}
