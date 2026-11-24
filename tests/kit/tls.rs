//! TLS 的客户端（rustls）：测试里连 Team 的服务器、网页端这些用。握手在调用的线程上做完（阻塞）；断开时不发 close_notify、
//! 对方没发就断开当成读到头（两样都和 Python 的 ssl 一样）。服务器的证书怎么验：
//! - [`Client::insecure`]：不验（测试服务器的证书是现造的自签证书；Python 那边是 `CERT_NONE`、不核对主机名）。握手里服务器拿
//!   证书的私钥签的名照样核对（OpenSSL 也是这样）；证书怪到 rustls 读不出来（比如不带扩展的 v1 证书）时连这个也不查
//! - [`Client::with_ca_file`] / [`Client::with_ca_pem`]：照给的 CA 验证书链、有效期、名字（主机名或 IP，要写在证书的
//!   subjectAltName 里）。rustls 不认 SHA-1 签的证书（macOS 自带的 LibreSSL 的 `openssl x509 -req` 默认用它：造证书时写上
//!   `-sha256`），也不认拿 CA 证书（basicConstraints 是 CA:TRUE，`openssl req -x509` 默认出的就是）当服务器证书的——那种自签的
//!   证书用 `insecure()` 加 [`Client::pin`]
//! - 两种都可以再 [`Client::pin`]：服务器证书里公钥的摘要要在名单里（`sha256//…`：SubjectPublicKeyInfo 的 SHA-256 的 base64，
//!   和 curl 的 `--pinnedpubkey`、邀请链接里的 p 一个写法；[`pin_of`] 从证书算它）
//!
//! ```ignore
//! let mut s = tls::Client::insecure().pin(&[&pin])?.connect("127.0.0.1", port, Duration::from_secs(10))?;
//! s.write_all(b"hello\n")?;
//! ```

use crate::crypto::{b64, b64_decode, sha256};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, WebPkiSupportedAlgorithms};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, RootCertStore, SignatureScheme, StreamOwned};
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn bad(what: &str, e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{what}：{e}"))
}

/// 读、写等过了时限：系统给的是 WouldBlock（“再试一次”），这里统一说成 TimedOut
pub fn timed(e: io::Error) -> io::Error {
    if e.kind() == io::ErrorKind::WouldBlock {
        io::Error::new(io::ErrorKind::TimedOut, "等过了时限，对方没有动静")
    } else {
        e
    }
}

/// 连 host:port，最多等 timeout；连上以后每次读、每次写也各自最多等这么久
pub fn tcp_connect(host: &str, port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let mut last = None;
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(sock) => {
                sock.set_read_timeout(Some(timeout))?;
                sock.set_write_timeout(Some(timeout))?;
                sock.set_nodelay(true)?;
                return Ok(sock);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{host} 解析不出地址"))))
}

/// DER 开头的一项：(标签, 内容, 这一项连头带内容, 后面剩下的)；不成样子是 None
fn der_item(data: &[u8]) -> Option<(u8, &[u8], &[u8], &[u8])> {
    let tag = *data.first()?;
    let first = *data.get(1)? as usize;
    let (len, head) = if first < 0x80 {
        (first, 2)
    } else {
        // 长的写法：后面 n 个字节是长度
        let n = first & 0x7f;
        if n == 0 || n > 4 {
            return None;
        }
        (data.get(2..2 + n)?.iter().fold(0usize, |acc, b| acc << 8 | *b as usize), 2 + n)
    };
    let end = head.checked_add(len)?;
    Some((tag, data.get(head..end)?, &data[..end], &data[end..]))
}

/// 证书（DER）里的 SubjectPublicKeyInfo（连外面那层 SEQUENCE）；不是证书的样子是 None。
/// 自己照 X.509 的结构数过去，不靠 rustls：它读不了的证书（v1 的）也要算得出公钥的摘要
pub fn spki_of(cert: &[u8]) -> Option<&[u8]> {
    const SEQUENCE: u8 = 0x30;
    let (SEQUENCE, cert, _, _) = der_item(cert)? else { return None };
    let (SEQUENCE, mut tbs, _, _) = der_item(cert)? else { return None };
    // 版本（[0]，v1 的证书没有这一项）
    if tbs.first() == Some(&0xa0) {
        tbs = der_item(tbs)?.3;
    }
    // 序列号、签名算法、签发者、有效期、主体：跳过去，下一项就是公钥
    for _ in 0..5 {
        tbs = der_item(tbs)?.3;
    }
    let (SEQUENCE, _, spki, _) = der_item(tbs)? else { return None };
    Some(spki)
}

/// 证书（DER）里公钥的摘要：`sha256//` 加 SubjectPublicKeyInfo 的 SHA-256 的 base64
pub fn pin_of(cert: &[u8]) -> Option<String> {
    spki_of(cert).map(|spki| format!("sha256//{}", b64(&sha256(spki))))
}

/// PEM 里的各张证书（DER），照文件里的先后；一张都没有是 Err
pub fn pem_certs(pem: &[u8]) -> io::Result<Vec<Vec<u8>>> {
    let certs: Vec<CertificateDer<'static>> =
        CertificateDer::pem_slice_iter(pem).collect::<Result<_, _>>().map_err(|e| bad("证书读不出来", e))?;
    if certs.is_empty() {
        return Err(bad("证书读不出来", "里面没有 CERTIFICATE"));
    }
    Ok(certs.iter().map(|c| c.as_ref().to_vec()).collect())
}

/// 服务器的证书怎么验（见这个文件开头）
#[derive(Debug)]
struct Verifier {
    /// 握手里的签名认哪些算法
    algs: WebPkiSupportedAlgorithms,
    /// 照 CA 验的那一个；None 是不验
    chain: Option<Arc<dyn ServerCertVerifier>>,
    /// 公钥的摘要要在这里面；空的是不查
    pins: Vec<[u8; 32]>,
}

impl Verifier {
    /// 不验证书、证书又怪到 rustls 读不出来：握手里的签名没法核对，放过去
    fn unreadable(&self, cert: &CertificateDer<'_>) -> bool {
        self.chain.is_none() && rustls::server::ParsedCertificate::try_from(cert).is_err()
    }
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if let Some(chain) = &self.chain {
            chain.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)?;
        }
        if !self.pins.is_empty() && !spki_of(end_entity).is_some_and(|spki| self.pins.contains(&sha256(spki))) {
            return Err(rustls::Error::General("服务器证书的公钥不在钉住的名单里".to_string()));
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        if self.unreadable(cert) {
            return Ok(HandshakeSignatureValid::assertion());
        }
        verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        if self.unreadable(cert) {
            return Ok(HandshakeSignatureValid::assertion());
        }
        verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

/// TLS 的客户端：定好怎么验证书，之后连几次都行（抄一份很便宜）
#[derive(Clone)]
pub struct Client {
    chain: Option<Arc<dyn ServerCertVerifier>>,
    pins: Vec<[u8; 32]>,
    cfg: Arc<ClientConfig>,
}

impl Client {
    fn build(chain: Option<Arc<dyn ServerCertVerifier>>, pins: Vec<[u8; 32]>) -> Client {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = Verifier { algs: provider.signature_verification_algorithms, chain: chain.clone(), pins: pins.clone() };
        let cfg = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("ring 的那一套认 TLS 1.2 和 1.3")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        Client { chain, pins, cfg: Arc::new(cfg) }
    }

    /// 不验服务器的证书（`CERT_NONE`、不核对主机名）：测试服务器的证书是现造的
    pub fn insecure() -> Client {
        Client::build(None, Vec::new())
    }

    /// 照这个 CA 文件（PEM，可以有好几张）验证书链、有效期和名字
    pub fn with_ca_file(path: impl AsRef<Path>) -> io::Result<Client> {
        let path = path.as_ref();
        let pem = std::fs::read(path).map_err(|e| io::Error::new(e.kind(), format!("读不了 CA 文件 {}：{e}", path.display())))?;
        Client::with_ca_pem(&pem)
    }

    /// 同 [`Client::with_ca_file`]，内容已经读进来了
    pub fn with_ca_pem(pem: &[u8]) -> io::Result<Client> {
        let mut roots = RootCertStore::empty();
        for cert in pem_certs(pem)? {
            roots.add(CertificateDer::from(cert)).map_err(|e| bad("CA 的证书用不了", e))?;
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let chain = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider)
            .build()
            .map_err(|e| bad("CA 的证书用不了", e))?;
        Ok(Client::build(Some(chain), Vec::new()))
    }

    /// 另外要求服务器证书里公钥的摘要是这几个里的一个（`sha256//…`）；写得不成样子是 Err
    pub fn pin(self, pins: &[&str]) -> io::Result<Client> {
        let mut all = self.pins;
        for pin in pins {
            let digest =
                pin.strip_prefix("sha256//").and_then(|body| b64_decode(body).ok()).and_then(|d| <[u8; 32]>::try_from(d).ok());
            let Some(digest) = digest else {
                return Err(bad("钉住的公钥摘要不成样子（要 sha256// 加 32 个字节的 base64）", pin));
            };
            all.push(digest);
        }
        Ok(Client::build(self.chain, all))
    }

    /// 连上 host:port、握完手。连、握手、之后的每次读写各自最多等 timeout；证书该是谁的照 host（主机名或 IP）
    pub fn connect(&self, host: &str, port: u16, timeout: Duration) -> io::Result<Stream> {
        self.wrap(tcp_connect(host, port, timeout)?, host)
    }

    /// 在已经连上的连接上握手；name 是证书该是谁的（主机名或 IP；是 IP 时不发 SNI）。握不成是 Err：对方断开了、
    /// 证书没验过（那时说明里是 rustls 的那句话）
    pub fn wrap(&self, mut sock: TcpStream, name: &str) -> io::Result<Stream> {
        let name = ServerName::try_from(name.to_string()).map_err(|e| bad("服务器的名字不成样子", e))?;
        let mut conn = ClientConnection::new(self.cfg.clone(), name).map_err(|e| bad("TLS", e))?;
        while conn.is_handshaking() {
            conn.complete_io(&mut sock).map_err(timed)?;
        }
        // 握手最后自己要发的那一段也先发出去：对方那边握手这就算完了
        while conn.wants_write() {
            conn.write_tls(&mut sock).map_err(timed)?;
        }
        Ok(Stream { inner: StreamOwned::new(conn, sock) })
    }
}

/// 握完手的连接：读写的是明文
pub struct Stream {
    inner: StreamOwned<ClientConnection, TcpStream>,
}

impl Stream {
    /// 服务器给的证书链（DER；第一张是它自己的）
    pub fn peer_certs(&self) -> Vec<Vec<u8>> {
        self.inner.conn.peer_certificates().unwrap_or_default().iter().map(|c| c.as_ref().to_vec()).collect()
    }

    /// 服务器证书里公钥的摘要（`sha256//…`）
    pub fn peer_pin(&self) -> Option<String> {
        self.inner.conn.peer_certificates()?.first().and_then(|c| pin_of(c.as_ref()))
    }

    /// 握出来的是哪个版本：`TLSv1.3` / `TLSv1.2`
    pub fn version(&self) -> &'static str {
        match self.inner.conn.protocol_version() {
            Some(rustls::ProtocolVersion::TLSv1_3) => "TLSv1.3",
            Some(rustls::ProtocolVersion::TLSv1_2) => "TLSv1.2",
            _ => "?",
        }
    }

    /// 底下的那条连接（改时限、看地址）
    pub fn sock(&self) -> &TcpStream {
        &self.inner.sock
    }

    /// 之后每次读、每次写最多等多久（None 是一直等）
    pub fn set_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.sock.set_read_timeout(timeout)?;
        self.inner.sock.set_write_timeout(timeout)
    }

    /// 规矩地收场：发 close_notify（不调它、直接丢掉就是连接直接断开）
    pub fn close_notify(&mut self) -> io::Result<()> {
        self.inner.conn.send_close_notify();
        self.inner.flush().map_err(timed)
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.inner.read(buf) {
            // 对方没发 close_notify 就断开了：当成读到头（Python 的 ssl 默认也这样）
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(0),
            other => other.map_err(timed),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf).map_err(timed)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush().map_err(timed)
    }
}

/// 单元测试共用的（这个文件、http、spec 的测试都要一台现造证书的 TLS 服务器）
#[cfg(test)]
pub(crate) mod testkit {
    use crate::proc::Cmd;
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::CertificateDer;
    use rustls::server::{ClientHello, ResolvesServerCert};
    use rustls::sign::CertifiedKey;
    use rustls::{ServerConfig, ServerConnection, StreamOwned};
    use std::net::{TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    /// 服务端握完手的连接
    pub type Served = StreamOwned<ServerConnection, TcpStream>;

    /// 这台机器上有的 openssl：PATH 上的那个，和系统自带的（macOS 自带的是 LibreSSL：`req -x509` 出的自签证书是 v1 的、
    /// `x509 -req` 默认拿 SHA-1 签；两种都要过）
    pub fn openssls() -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = Vec::new();
        for candidate in [crate::proc::which("openssl"), Some(PathBuf::from("/usr/bin/openssl"))].into_iter().flatten() {
            let real = std::fs::canonicalize(&candidate).unwrap_or(candidate);
            if real.is_file() && !found.contains(&real) {
                found.push(real);
            }
        }
        assert!(!found.is_empty(), "这台机器上没有 openssl");
        found
    }

    /// 在 dir 里跑一条 openssl（环境只带 PATH 和它自己的 OPENSSL_CONF）；给的是标准输出
    pub fn openssl(bin: &Path, dir: &Path, args: &[&str]) -> Vec<u8> {
        let mut cmd = Cmd::new(bin);
        cmd.args(args).cwd(dir).pass_env(&["PATH", "OPENSSL_CONF"]);
        cmd.output().unwrap_or_else(|e| panic!("{e}")).check().stdout
    }

    /// 自签的证书（回归里 tm_cert 的那两条命令）：dir/key.pem、dir/cert.pem
    pub fn self_signed(bin: &Path, dir: &Path) {
        openssl(bin, dir, &["ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", "key.pem"]);
        openssl(bin, dir, &["req", "-new", "-x509", "-key", "key.pem", "-subj", "/CN=cca-team", "-days", "2", "-out", "cert.pem"]);
    }

    /// CA 签的证书，名字是 127.0.0.1（签的时候带 -sha256）：dir/ca.pem、dir/cert.pem、dir/key.pem
    pub fn ca_signed(bin: &Path, dir: &Path) {
        openssl(bin, dir, &["ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", "ca.key"]);
        openssl(bin, dir, &["req", "-new", "-x509", "-key", "ca.key", "-subj", "/CN=cca-test-ca", "-days", "2", "-out", "ca.pem"]);
        openssl(bin, dir, &["ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", "key.pem"]);
        openssl(bin, dir, &["req", "-new", "-key", "key.pem", "-subj", "/CN=127.0.0.1", "-out", "csr.pem"]);
        crate::fsx::write(
            dir.join("ext.cnf"),
            "subjectAltName=IP:127.0.0.1\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n",
        );
        let sign =
            ["x509", "-req", "-in", "csr.pem", "-CA", "ca.pem", "-CAkey", "ca.key", "-CAcreateserial", "-sha256", "-days", "2"];
        openssl(bin, dir, &[&sign[..], &["-extfile", "ext.cnf", "-out", "cert.pem"]].concat());
    }

    /// 不管对方问的是哪个名字，都给这一张证书
    #[derive(Debug)]
    struct Fixed(Arc<CertifiedKey>);

    impl ResolvesServerCert for Fixed {
        fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
            Some(self.0.clone())
        }
    }

    /// 在 127.0.0.1 上起一台 TLS 服务器（一个线程，一条条地接）：dir 里的 cert.pem、key.pem，每条握完手的连接交给 handle；
    /// 握不成的不管。返回端口。不用 [`crate::tlsd`]：它要读证书核对私钥，v1 的证书 rustls 读不了
    pub fn serve(dir: &Path, handle: impl Fn(Served) + Send + 'static) -> u16 {
        let certs: Vec<CertificateDer<'static>> =
            CertificateDer::pem_slice_iter(&crate::fsx::read(dir.join("cert.pem"))).collect::<Result<_, _>>().expect("证书");
        let key = crate::tlsd::private_key(&crate::fsx::read(dir.join("key.pem"))).expect("私钥");
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let key = provider.key_provider.load_private_key(key).expect("私钥用得了");
        let cfg = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("TLS 的版本")
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(Fixed(Arc::new(CertifiedKey::new(certs, key)))));
        let cfg = Arc::new(cfg);
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("绑端口");
        let port = listener.local_addr().expect("端口").port();
        std::thread::spawn(move || {
            for mut sock in listener.incoming().flatten() {
                let Ok(mut conn) = ServerConnection::new(cfg.clone()) else { continue };
                let shaken = (|| {
                    while conn.is_handshaking() {
                        conn.complete_io(&mut sock)?;
                    }
                    while conn.wants_write() {
                        conn.write_tls(&mut sock)?;
                    }
                    std::io::Result::Ok(())
                })();
                if shaken.is_ok() {
                    handle(StreamOwned::new(conn, sock));
                }
            }
        });
        port
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::*;
    use super::*;
    use crate::fsx::{self, TempDir};

    const WAIT: Duration = Duration::from_secs(10);

    /// 读一行就原样写回去的服务器
    fn echo(mut stream: Served) {
        let mut buf = [0u8; 256];
        if let Ok(n) = stream.read(&mut buf) {
            let _ = stream.write_all(&buf[..n]);
        }
    }

    fn roundtrip(mut stream: Stream) -> Vec<u8> {
        stream.write_all(b"ping\n").unwrap();
        let mut back = [0u8; 5];
        stream.read_exact(&mut back).unwrap();
        back.to_vec()
    }

    /// openssl 算的公钥摘要（文档里教管理员的那条命令的头一步：取出公钥，它的 DER 就是 SubjectPublicKeyInfo）
    fn openssl_pin(bin: &Path, dir: &Path) -> String {
        let pem = String::from_utf8(openssl(bin, dir, &["x509", "-in", "cert.pem", "-pubkey", "-noout"])).unwrap();
        let body: String = pem.lines().filter(|line| !line.starts_with("-----")).collect();
        format!("sha256//{}", b64(&sha256(&b64_decode(&body).unwrap())))
    }

    #[test]
    fn pins_match_openssl_and_self_signed_servers_work_without_verification() {
        for bin in openssls() {
            let dir = TempDir::new("tls-unit-");
            self_signed(&bin, &dir);
            let der = pem_certs(&fsx::read(dir.join("cert.pem"))).unwrap().remove(0);
            let pin = pin_of(&der).expect("证书里找得到公钥");
            assert_eq!(pin, openssl_pin(&bin, &dir), "{}", bin.display());

            let port = serve(&dir, echo);
            let stream = Client::insecure().connect("127.0.0.1", port, WAIT).unwrap_or_else(|e| panic!("{}：{e}", bin.display()));
            assert_eq!(stream.peer_pin().as_deref(), Some(pin.as_str()));
            assert_eq!(stream.peer_certs(), vec![der]);
            assert_eq!(stream.version(), "TLSv1.3");
            assert_eq!(roundtrip(stream), b"ping\n");

            // 钉住公钥：对的那一把在名单里就行，不在就握不成
            let other = format!("sha256//{}", b64(&[7u8; 32]));
            let pinned = Client::insecure().pin(&[&other, &pin]).unwrap();
            assert_eq!(roundtrip(pinned.connect("127.0.0.1", port, WAIT).unwrap()), b"ping\n");
            let err = Client::insecure().pin(&[&other]).unwrap().connect("127.0.0.1", port, WAIT).err().expect("公钥不对");
            assert!(err.to_string().contains("不在钉住的名单里"), "{err}");

            // 照 CA 验：自签的这一张不是哪个 CA 签的
            let ca_dir = TempDir::new("tls-unit-");
            ca_signed(&bin, &ca_dir);
            let strict = Client::with_ca_file(ca_dir.join("ca.pem")).unwrap();
            assert!(strict.connect("127.0.0.1", port, WAIT).is_err());
        }
    }

    #[test]
    fn ca_mode_checks_the_chain_and_the_name() {
        for bin in openssls() {
            let (dir, other) = (TempDir::new("tls-unit-"), TempDir::new("tls-unit-"));
            ca_signed(&bin, &dir);
            ca_signed(&bin, &other);
            let port = serve(&dir, echo);
            let client = Client::with_ca_file(dir.join("ca.pem")).unwrap();
            let stream = client.connect("127.0.0.1", port, WAIT).unwrap_or_else(|e| panic!("{}：{e}", bin.display()));
            let pin = stream.peer_pin().expect("公钥的摘要");
            assert_eq!(roundtrip(stream), b"ping\n");

            // 名字不对（证书是给 127.0.0.1 的）
            let sock = tcp_connect("127.0.0.1", port, WAIT).unwrap();
            let err = client.wrap(sock, "localhost").err().expect("名字不对");
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{err}");
            // 别的 CA
            assert!(Client::with_ca_file(other.join("ca.pem")).unwrap().connect("127.0.0.1", port, WAIT).is_err());
            // CA 之外再钉公钥：两样都要对
            assert_eq!(roundtrip(client.clone().pin(&[&pin]).unwrap().connect("127.0.0.1", port, WAIT).unwrap()), b"ping\n");
            let wrong = format!("sha256//{}", b64(&[7u8; 32]));
            assert!(client.clone().pin(&[&wrong]).unwrap().connect("127.0.0.1", port, WAIT).is_err());
            // 不验的也连得上
            assert_eq!(roundtrip(Client::insecure().connect("127.0.0.1", port, WAIT).unwrap()), b"ping\n");
        }
    }

    #[test]
    fn a_dropped_connection_reads_as_the_end() {
        let dir = TempDir::new("tls-unit-");
        ca_signed(&openssls()[0], &dir);
        // 服务器写完就丢掉连接（不发 close_notify）
        let port = serve(&dir, |mut stream| {
            let _ = stream.write_all(b"bye");
        });
        let mut stream = Client::insecure().connect("127.0.0.1", port, WAIT).unwrap();
        let mut all = Vec::new();
        stream.read_to_end(&mut all).unwrap();
        assert_eq!(all, b"bye");
        // 握完手又关了的连接上规矩地收场不出错
        let mut again = Client::insecure().connect("127.0.0.1", port, WAIT).unwrap();
        again.set_timeout(Some(Duration::from_secs(5))).unwrap();
        let _ = again.close_notify();
    }

    #[test]
    fn a_silent_or_missing_server_is_an_error_not_a_hang() {
        // 接了连接、一个字节都不回的服务器
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let started = std::time::Instant::now();
        let err = Client::insecure().connect("127.0.0.1", port, Duration::from_millis(300)).err().expect("握不成");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
        // 没人听的端口
        drop(listener);
        assert!(crate::fsx::settle_port(port), "关掉的端口还连得上");
        let err = Client::insecure().connect("127.0.0.1", port, WAIT).err().expect("连不上");
        assert_eq!(err.kind(), io::ErrorKind::ConnectionRefused, "{err}");
        // 说的不是 TLS 的服务器
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut sock in listener.incoming().flatten() {
                let _ = sock.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n");
            }
        });
        assert!(Client::insecure().connect("127.0.0.1", port, WAIT).is_err());
    }

    #[test]
    fn malformed_input_is_rejected() {
        assert_eq!(spki_of(b""), None);
        assert_eq!(spki_of(b"not a certificate"), None);
        assert_eq!(spki_of(&[0x30, 0x82, 0xff, 0xff, 0x30]), None, "长度比实际的长");
        assert_eq!(pin_of(&[0x30, 0x03, 0x30, 0x01, 0x00]), None);
        assert!(pem_certs(b"no pem here").is_err());
        assert!(Client::with_ca_pem(b"").is_err());
        for bad in [
            "",
            "sha256//",
            "sha256//AAAA",
            "sha1//AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        ] {
            assert!(Client::insecure().pin(&[bad]).is_err(), "{bad:?}");
        }
        assert!(Client::insecure().wrap(std::net::TcpStream::connect(free_listener()).unwrap(), "not a name").is_err());
    }

    /// 一个接了连接什么都不做的端口（留着不关）
    fn free_listener() -> std::net::SocketAddr {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        std::mem::forget(listener);
        addr
    }
}
