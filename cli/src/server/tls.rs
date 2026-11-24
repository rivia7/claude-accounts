//! 监听用的 TLS：读一对证书和私钥（`load_pair`），网页端那张会换的证书（`CertCtx`）；连证书机构用的客户端配置（`client_config`）。
//!
//! 和 Python 的 `tls_context`（Py:1670-1677）对应的地方：最低 TLS 1.2，不要客户端证书，不带 ALPN；证书和私钥不是一对时读不了
//! （`load_cert_chain` 也是）。私钥认 SEC1（`BEGIN EC PRIVATE KEY`：安装脚本、测试里 `openssl ecparam -genkey -noout` 出来的）、
//! PKCS#8（`BEGIN PRIVATE KEY`：证书助手生成的）、PKCS#1（RSA）。
//!
//! 是不是一对不用 rustls 自己的那个检查（`CertifiedKey::keys_match`、`with_single_cert` 里也是它）：它要先用 webpki 解析证书，
//! 而 webpki 不认 v1 的证书——macOS 自带的 LibreSSL `req -new -x509` 自签出来的就是 v1（实测 LibreSSL 3.3.6），这样的证书 Python
//! 的服务端一直在用（设备钉的是公钥，不看证书别的地方）。所以这里自己从证书里取公钥（`x509::cert_spki`）和私钥的比。

use super::x509;
use crate::py::fs::open_error_text;
use crate::py::tls::TlsError;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::sign::{CertifiedKey, SingleCertAndKey};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

/// 一对证书、私钥读不了的原因
enum Why {
    /// 没有这个文件
    Missing(String),
    /// 别的：没权限、不是证书、私钥用不了、不是一对……
    Other(String),
}

impl Why {
    fn text(self) -> String {
        match self {
            Why::Missing(path) => format!("没有 {path}"),
            Why::Other(why) => why,
        }
    }
}

fn read(path: &Path) -> Result<Vec<u8>, Why> {
    let name = path.to_string_lossy();
    std::fs::read(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Why::Missing(name.to_string()),
        _ => Why::Other(open_error_text(&e, &name)),
    })
}

fn load(cert: &Path, key: &Path) -> Result<Arc<ServerConfig>, Why> {
    let (cert_pem, key_pem) = (read(cert)?, read(key)?);
    let (cert_name, key_name) = (cert.to_string_lossy(), key.to_string_lossy());
    let chain = CertificateDer::pem_slice_iter(&cert_pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| Why::Other(format!("{cert_name} 里的证书读不了（{e}）")))?;
    let Some(first) = chain.first() else {
        return Err(Why::Other(format!("{cert_name} 里没有证书")));
    };
    let key_der = PrivateKeyDer::from_pem_slice(&key_pem).map_err(|e| Why::Other(format!("{key_name} 里的私钥读不了（{e}）")))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let signing =
        provider.key_provider.load_private_key(key_der).map_err(|e| Why::Other(format!("{key_name} 里的私钥用不了（{e}）")))?;
    let Some(spki) = x509::cert_spki(first.as_ref()) else {
        return Err(Why::Other(format!("{cert_name} 里的证书看不懂")));
    };
    // 私钥给不出公钥的（ring 的三种私钥都给得出）没法比：和 rustls 一样放过
    if signing.public_key().is_some_and(|public| public.as_ref() != spki.as_slice()) {
        return Err(Why::Other(format!("证书和私钥不是一对（{cert_name}、{key_name}）")));
    }
    let config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|e| Why::Other(e.to_string()))?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(SingleCertAndKey::from(CertifiedKey::new(chain, signing))));
    Ok(Arc::new(config))
}

/// 读一对证书（PEM，可以连着中间证书）和私钥，做成监听用的配置；读不了、不是一对是 Err（给人看的一句话）。
/// 启动时读不了：调用方说一句、退出码 1
pub fn load_pair(cert: &Path, key: &Path) -> Result<Arc<ServerConfig>, String> {
    load(cert, key).map_err(Why::text)
}

/// 连证书机构用的客户端配置：照 roots 校验对方的证书和名字（名字可以是 IP 地址），TLS 1.3 和 1.2
pub fn client_config(roots: RootCertStore) -> Result<Arc<ClientConfig>, String> {
    crate::py::tls::client_config(roots).map_err(|e| match e {
        TlsError::Cert(why) | TlsError::Other(why) => why,
    })
}

/// 两个文件现在的样子：各自的修改时间（秒、纳秒）和大小
type Stamp = [(i64, i64, u64); 2];

struct CertState {
    /// 上次看文件是什么时候（单调时钟）
    checked: f64,
    /// 上次说过的原因：同一个只说一次
    warned: Option<String>,
    /// 现在用的那一对读进来时文件的样子
    seen: Option<Stamp>,
    config: Option<Arc<ServerConfig>>,
}

/// 浏览器信得过的那张证书（--web-cert）：证书机构签的会到期、会被续上（90 天一换，给 IP 地址签的 6 天一换），所以文件变了就重新读
/// （最多一分钟看一次），不用重启；新的读不了时接着用旧的、在日志里说一声。启动时证书还没有也照样起来（证书助手和这个服务一起装上时，
/// 头一张证书要等它去要）：那之前这个端口上的连接一握手就断，隔一秒看一次文件来了没有（Py:1680-1717）
pub struct CertCtx {
    cert: PathBuf,
    key: PathBuf,
    state: Mutex<CertState>,
}

impl CertCtx {
    /// 有配置时隔这么多秒看一次文件变了没有
    const EVERY: f64 = 60.0;
    /// 还没有配置时
    const EVERY_WAITING: f64 = 1.0;

    /// now 是单调时钟（`py::time::monotonic()`）。建的时候就读一次：读得了说一句“有了”，读不了说一句为什么
    pub fn new(cert: &Path, key: &Path, now: f64) -> CertCtx {
        let ctx = CertCtx::unread(cert, key);
        ctx.get(now);
        ctx
    }

    /// 还一次都没看过文件的
    fn unread(cert: &Path, key: &Path) -> CertCtx {
        CertCtx {
            cert: cert.to_path_buf(),
            key: key.to_path_buf(),
            state: Mutex::new(CertState { checked: f64::NEG_INFINITY, warned: None, seen: None, config: None }),
        }
    }

    fn stamp(&self) -> Result<Stamp, Why> {
        use std::os::unix::fs::MetadataExt;
        let of = |path: &Path| match std::fs::metadata(path) {
            Ok(m) => Ok((m.mtime(), m.mtime_nsec(), m.size())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(Why::Missing(path.to_string_lossy().to_string())),
            Err(e) => Err(Why::Other(open_error_text(&e, &path.to_string_lossy()))),
        };
        Ok([of(&self.cert)?, of(&self.key)?])
    }

    /// 现在用的那一份；一张都还没有是 None（这时不握手，直接断开）。证书有了、换了、读不了时在日志（stderr）里说一句
    pub fn get(&self, now: f64) -> Option<Arc<ServerConfig>> {
        self.get_saying(now, &mut |line| crate::err!("{line}"))
    }

    /// say：要说的那一句交给谁（测试里收起来比）
    fn get_saying(&self, now: f64, say: &mut dyn FnMut(String)) -> Option<Arc<ServerConfig>> {
        let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let every = if s.config.is_some() { Self::EVERY } else { Self::EVERY_WAITING };
        if now - s.checked >= every {
            s.checked = now;
            let loaded = self.stamp().and_then(|stamp| match s.seen == Some(stamp) {
                true => Ok(None),
                false => load(&self.cert, &self.key).map(|config| Some((config, stamp))),
            });
            match loaded {
                Ok(None) => {}
                Ok(Some((config, stamp))) => {
                    let had = s.config.is_some();
                    (s.config, s.seen, s.warned) = (Some(config), Some(stamp), None);
                    say(format!("cca-presence: 网页端的证书{}", if had { "换了，已经重新读" } else { "有了，这个端口开始用它" }));
                }
                Err(why) => {
                    let missing = matches!(why, Why::Missing(_));
                    let why = why.text();
                    // 续证书的程序正写到一半：下一次再看；同一个错只说一次
                    if s.warned.as_deref() != Some(why.as_str()) {
                        if s.config.is_some() {
                            say(format!("cca-presence: 网页端的新证书读不了（{why}），接着用原来的"));
                        } else {
                            let what = if missing { "还没有" } else { "读不了" };
                            say(format!("cca-presence: 网页端的证书{what}（{why}），等它好了这个端口再开始用（这之前手机连不上）"));
                        }
                        s.warned = Some(why);
                    }
                }
            }
        }
        s.config.clone()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::super::x509::testkit::{der, new_key, pem, point_of, self_signed};
    use super::super::x509::San;
    use super::*;
    use crate::py::fs::TempDir;
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{ServerName, UnixTime};
    use rustls::{
        ClientConnection, DigitallySignedStruct, ProtocolVersion, ServerConnection, SignatureScheme, SupportedProtocolVersion,
    };

    /// PKCS#8 的私钥（ring 现造的）改写成 SEC1（RFC 5915）、带着曲线的参数和公钥：`openssl ecparam -name prime256v1 -genkey -noout`
    /// 写出来的就是这个样子（121 个字节）
    fn sec1_of(pkcs8: &[u8]) -> Vec<u8> {
        // ring 生成的 PKCS#8 是固定的样子：第 36 个字节起是 32 个字节的私钥
        assert_eq!((pkcs8.len(), &pkcs8[33..36]), (138, &[0x01, 0x04, 0x20][..]));
        let curve = der(0xa0, &[&der(0x06, &[&[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07]])]);
        let public = der(0xa1, &[&der(0x03, &[&[0], &point_of(pkcs8)])]);
        let out = der(0x30, &[&der(0x02, &[&[1]]), &der(0x04, &[&pkcs8[36..68]]), &curve, &public]);
        assert_eq!(out.len(), 121);
        out
    }

    struct Files {
        dir: TempDir,
        /// 写了几次：每写一次，文件的修改时间往后拨一秒——修改时间只精确到秒的文件系统上，紧接着的两次写也分得出来
        writes: std::cell::Cell<u64>,
    }

    impl Files {
        fn new() -> Files {
            Files { dir: TempDir::new().unwrap(), writes: std::cell::Cell::new(0) }
        }

        fn put(&self, name: &str, text: &str) -> PathBuf {
            let path = Path::new(&self.dir.path).join(name);
            std::fs::write(&path, text).unwrap();
            self.writes.set(self.writes.get() + 1);
            let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_767_225_600 + self.writes.get());
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(at).unwrap();
            path
        }
    }

    /// 只认这一张证书（设备钉住公钥的做法）：握手的签名自己拿证书里的公钥验。rustls 自带的验法要用 webpki 解析证书，v1 的过不了
    #[derive(Debug)]
    struct Pinned {
        cert: Vec<u8>,
    }

    impl Pinned {
        fn check(&self, message: &[u8], dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
            let spki = x509::cert_spki(&self.cert).unwrap();
            ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_ASN1, &spki[26..])
                .verify(message, dss.signature())
                .map(|_| HandshakeSignatureValid::assertion())
                .map_err(|_| rustls::Error::General("握手的签名不对".into()))
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
            match end_entity.as_ref() == self.cert.as_slice() {
                true => Ok(ServerCertVerified::assertion()),
                false => Err(rustls::Error::General("不是钉住的那张证书".into())),
            }
        }

        fn verify_tls12_signature(
            &self,
            message: &[u8],
            _cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            self.check(message, dss)
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            _cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            self.check(message, dss)
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            vec![SignatureScheme::ECDSA_NISTP256_SHA256]
        }
    }

    const BOTH: &[&SupportedProtocolVersion] = &[&rustls::version::TLS13, &rustls::version::TLS12];

    fn pinned_client(cert: &[u8], versions: &[&'static SupportedProtocolVersion]) -> Arc<ClientConfig> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(versions)
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Pinned { cert: cert.to_vec() }))
            .with_no_client_auth();
        Arc::new(config)
    }

    /// 信任这一张（自签的）证书、照常校验名字的客户端
    fn trusting_client(cert: &[u8]) -> Arc<ClientConfig> {
        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from(cert.to_vec())).unwrap();
        client_config(roots).unwrap()
    }

    /// 在内存里把握手做完：成了给出用的协议版本和服务器给的证书
    fn handshake(
        server: &Arc<ServerConfig>,
        client: &Arc<ClientConfig>,
        name: &str,
    ) -> Result<(ProtocolVersion, Vec<Vec<u8>>), rustls::Error> {
        let mut c = ClientConnection::new(client.clone(), ServerName::try_from(name.to_string()).unwrap())?;
        let mut s = ServerConnection::new(server.clone())?;
        for _ in 0..20 {
            let mut to_server = Vec::new();
            while c.wants_write() {
                c.write_tls(&mut to_server).unwrap();
            }
            let mut rd = &to_server[..];
            while !rd.is_empty() {
                s.read_tls(&mut rd).unwrap();
                s.process_new_packets()?;
            }
            let mut to_client = Vec::new();
            while s.wants_write() {
                s.write_tls(&mut to_client).unwrap();
            }
            let mut rd = &to_client[..];
            while !rd.is_empty() {
                c.read_tls(&mut rd).unwrap();
                c.process_new_packets()?;
            }
            if !c.is_handshaking() && !s.is_handshaking() {
                let certs = c.peer_certificates().unwrap().iter().map(|d| d.as_ref().to_vec()).collect();
                return Ok((c.protocol_version().unwrap(), certs));
            }
        }
        panic!("握手 20 个来回还没完");
    }

    /// 安装脚本、测试里那种：SEC1 的私钥 + v1 的自签证书（LibreSSL 的 req -new -x509）。读得了，TLS 1.3、1.2 都握得了手，
    /// 给出去的就是文件里的那张证书
    #[test]
    fn sec1_key_with_a_v1_certificate() {
        let (f, key) = (Files::new(), new_key());
        let cert = self_signed(&key, None);
        let cert_path = f.put("cert.pem", &pem("CERTIFICATE", &cert));
        let key_path = f.put("key.pem", &pem("EC PRIVATE KEY", &sec1_of(&key)));
        let config = load_pair(&cert_path, &key_path).unwrap();
        assert!(config.alpn_protocols.is_empty(), "不带 ALPN");
        for (versions, want) in [
            (BOTH, ProtocolVersion::TLSv1_3),
            (&[&rustls::version::TLS12][..], ProtocolVersion::TLSv1_2),
            (&[&rustls::version::TLS13][..], ProtocolVersion::TLSv1_3),
        ] {
            let (version, certs) = handshake(&config, &pinned_client(&cert, versions), "team.example.com").unwrap();
            assert_eq!((version, certs), (want, vec![cert.clone()]));
        }
        // 钉的是另一张证书的客户端握不了手
        let other = self_signed(&new_key(), None);
        assert!(handshake(&config, &pinned_client(&other, BOTH), "team.example.com").is_err());
        // rustls 自己的那个检查认不了 v1 的证书（所以 load 里不用它）：哪天它认了，这里会提醒可以换回去
        let provider = rustls::crypto::ring::default_provider();
        let signing = provider
            .key_provider
            .load_private_key(PrivateKeyDer::from_pem_slice(pem("EC PRIVATE KEY", &sec1_of(&key)).as_bytes()).unwrap());
        assert!(CertifiedKey::new(vec![CertificateDer::from(cert)], signing.unwrap()).keys_match().is_err());
    }

    /// 证书机构签的那种：PKCS#8 的私钥 + 带 subjectAltName 的 v3 证书。客户端照常校验：名字（域名、IPv4、IPv6）对得上才握得了手
    #[test]
    fn pkcs8_key_and_names_checked_by_webpki() {
        let (f, key) = (Files::new(), new_key());
        let names = [San::Dns("team.example.com".into()), San::Ip("192.0.2.10".into()), San::Ip("2001:db8::1".into())];
        let cert = self_signed(&key, Some(&names));
        assert_eq!(x509::cert_info(&cert).unwrap().san, names.iter().cloned().collect());
        let cert_path = f.put("fullchain.pem", &pem("CERTIFICATE", &cert));
        let config = load_pair(&cert_path, &f.put("privkey.pem", &pem("PRIVATE KEY", &key))).unwrap();
        let client = trusting_client(&cert);
        for name in ["team.example.com", "192.0.2.10", "2001:db8::1"] {
            let (version, certs) = handshake(&config, &client, name).unwrap();
            assert_eq!((version, certs.len()), (ProtocolVersion::TLSv1_3, 1), "{name}");
        }
        for name in ["other.example.com", "192.0.2.11", "2001:db8::2"] {
            match handshake(&config, &client, name) {
                Err(rustls::Error::InvalidCertificate(_)) => {}
                other => panic!("{name}：该是证书对不上名字，得到的是 {other:?}"),
            }
        }
        // 不信任这张证书的客户端
        let stranger = trusting_client(&self_signed(&new_key(), Some(&names)));
        assert!(matches!(handshake(&config, &stranger, "team.example.com"), Err(rustls::Error::InvalidCertificate(_))));
        // 同一把私钥写成 SEC1 也读得了；文件里证书后面跟着中间证书时，头一张是服务器自己的，握手时整条给出去
        let extra = self_signed(&new_key(), Some(&[San::Dns("ca.example.com".into())]));
        let chain_path = f.put("chain.pem", &format!("{}{}", pem("CERTIFICATE", &cert), pem("CERTIFICATE", &extra)));
        let config = load_pair(&chain_path, &f.put("sec1.pem", &pem("EC PRIVATE KEY", &sec1_of(&key)))).unwrap();
        let (_, certs) = handshake(&config, &pinned_client(&cert, BOTH), "team.example.com").unwrap();
        assert_eq!(certs, vec![cert.clone(), extra]);
    }

    /// 证书和私钥不是一对：读不了（Python 的 load_cert_chain 也不收），v1、v3 的证书都查得出来
    #[test]
    fn a_mismatched_pair_is_refused() {
        let (f, key, other) = (Files::new(), new_key(), new_key());
        let other_sec1 = f.put("other.key", &pem("EC PRIVATE KEY", &sec1_of(&other)));
        let other_pkcs8 = f.put("other8.key", &pem("PRIVATE KEY", &other));
        for names in [None, Some(&[San::Dns("example.com".into())][..])] {
            let cert_path = f.put("cert.pem", &pem("CERTIFICATE", &self_signed(&key, names)));
            for key_path in [&other_sec1, &other_pkcs8] {
                let why = load_pair(&cert_path, key_path).err().unwrap();
                assert!(why.starts_with("证书和私钥不是一对（") && why.contains("cert.pem"), "{why}");
            }
            assert!(load_pair(&cert_path, &f.put("own.key", &pem("PRIVATE KEY", &key))).is_ok());
        }
        // 文件里有两张证书时比的是头一张
        let two = format!("{}{}", pem("CERTIFICATE", &self_signed(&other, None)), pem("CERTIFICATE", &self_signed(&key, None)));
        assert!(load_pair(&f.put("two.pem", &two), &other_sec1).is_ok());
        assert!(load_pair(&f.put("two.pem", &two), &f.put("own.key", &pem("PRIVATE KEY", &key))).is_err());
    }

    /// 文件没有、不是 PEM、放错了：给出一句说得清的原因，不 panic
    #[test]
    fn unusable_files_say_why() {
        let (f, key) = (Files::new(), new_key());
        let cert = f.put("cert.pem", &pem("CERTIFICATE", &self_signed(&key, None)));
        let good_key = f.put("key.pem", &pem("PRIVATE KEY", &key));
        let gone = Path::new(&f.dir.path).join("gone.pem");
        let why = |c: &Path, k: &Path| load_pair(c, k).err().unwrap();
        assert_eq!(why(&gone, &good_key), format!("没有 {}", gone.display()));
        assert_eq!(why(&cert, &gone), format!("没有 {}", gone.display()));
        assert!(why(Path::new(&f.dir.path), &good_key).contains("Is a directory"), "证书的位置上是个目录");
        assert!(why(&f.put("empty.pem", ""), &good_key).ends_with("empty.pem 里没有证书"));
        assert!(why(&good_key, &good_key).ends_with("key.pem 里没有证书"), "证书的位置上放的是私钥");
        assert!(why(&cert, &cert).contains("cert.pem 里的私钥读不了（"), "私钥的位置上放的是证书");
        assert!(why(&cert, &f.put("junk.key", "不是 PEM\n")).contains("junk.key 里的私钥读不了（"));
        let half = pem("CERTIFICATE", &self_signed(&key, None)).replace("-----END CERTIFICATE-----\n", "");
        assert!(why(&f.put("half.pem", &half), &good_key).contains("half.pem 里的证书读不了（"), "证书写到一半");
        assert!(why(&f.put("bad.pem", &pem("CERTIFICATE", &[0x30, 0x03, 0x02, 0x01, 0x00])), &good_key)
            .ends_with("bad.pem 里的证书看不懂"));
        let bad_key = f.put("bad.key", &pem("PRIVATE KEY", &[0x30, 0x03, 0x02, 0x01, 0x00]));
        assert!(why(&cert, &bad_key).contains("bad.key 里的私钥用不了（"));
    }

    /// 网页端的那张证书：没有时等着（一秒看一次），来了就用；换了重新读（一分钟看一次）；新的读不了接着用旧的。
    /// 日志里说的话和 Python 的一样（读不了的原因除外：那是 rustls 的说法），同一个原因只说一次
    #[test]
    fn cert_ctx_follows_the_files() {
        let (f, key) = (Files::new(), new_key());
        let (cert_path, key_path) = (Path::new(&f.dir.path).join("fullchain.pem"), Path::new(&f.dir.path).join("privkey.pem"));
        let (cert_name, key_name) = (cert_path.display().to_string(), key_path.display().to_string());
        let first = self_signed(&key, None);
        let ctx = CertCtx::unread(&cert_path, &key_path);
        let said = std::cell::RefCell::new(Vec::<String>::new());
        let get = |now: f64| ctx.get_saying(now, &mut |line| said.borrow_mut().push(line));
        // 给出去的是哪一张：照钉着这张证书的客户端握不握得了手看
        let serves =
            |now: f64, cert: &[u8]| get(now).map(|config| handshake(&config, &pinned_client(cert, BOTH), "x.example.com").is_ok());
        let take = || said.borrow_mut().drain(..).collect::<Vec<String>>();

        assert_eq!(serves(100.0, &first), None, "证书还没有");
        assert_eq!(
            take(),
            [format!("cca-presence: 网页端的证书还没有（没有 {cert_name}），等它好了这个端口再开始用（这之前手机连不上）")]
        );
        assert_eq!((serves(100.5, &first), serves(101.0, &first), take()), (None, None, vec![]), "同一个原因只说一次");
        // 证书先到、私钥还没到：换了一个原因，再说一次
        f.put("fullchain.pem", &pem("CERTIFICATE", &first));
        assert_eq!(serves(102.0, &first), None);
        assert_eq!(
            take(),
            [format!("cca-presence: 网页端的证书还没有（没有 {key_name}），等它好了这个端口再开始用（这之前手机连不上）")]
        );
        // 私钥的位置上是别的东西：读不了
        f.put("privkey.pem", "不是 PEM\n");
        assert_eq!(serves(103.0, &first), None);
        let lines = take();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with(&format!("cca-presence: 网页端的证书读不了（{key_name} 里的私钥读不了（")), "{lines:?}");
        assert!(lines[0].ends_with("），等它好了这个端口再开始用（这之前手机连不上）"), "{lines:?}");
        // 都好了：不到一秒不看，到了就用上
        f.put("privkey.pem", &pem("PRIVATE KEY", &key));
        assert_eq!((serves(103.9, &first), take()), (None, vec![]), "不到一秒不看");
        assert_eq!(serves(104.0, &first), Some(true));
        assert_eq!(take(), ["cca-presence: 网页端的证书有了，这个端口开始用它"]);

        // 换一对新的（另一把私钥）：一分钟里还是原来的，到点才重新读
        let key2 = new_key();
        let second = self_signed(&key2, None);
        f.put("privkey.pem", &pem("EC PRIVATE KEY", &sec1_of(&key2)));
        f.put("fullchain.pem", &pem("CERTIFICATE", &second));
        assert_eq!((serves(163.9, &first), take()), (Some(true), vec![]), "不到一分钟不看");
        assert_eq!((serves(164.0, &first), serves(164.0, &second)), (Some(false), Some(true)), "换成新的那张了");
        assert_eq!(take(), ["cca-presence: 网页端的证书换了，已经重新读"]);
        // 文件没变：到点看了也不重新读、不说话
        assert_eq!((serves(224.0, &second), serves(284.0, &second), take()), (Some(true), Some(true), vec![]));

        // 续证书的程序写到一半（私钥换了、证书还没换：不是一对）：接着用原来的，说一次
        f.put("privkey.pem", &pem("PRIVATE KEY", &key));
        assert_eq!(serves(344.0, &second), Some(true), "新的读不了：还是第二张");
        assert_eq!(
            take(),
            [format!("cca-presence: 网页端的新证书读不了（证书和私钥不是一对（{cert_name}、{key_name}）），接着用原来的")]
        );
        assert_eq!((serves(404.0, &second), take()), (Some(true), vec![]), "同一个原因只说一次");
        // 证书也换好了：下一次看的时候用上
        f.put("fullchain.pem", &pem("CERTIFICATE", &first));
        assert_eq!((serves(463.9, &second), serves(464.0, &first)), (Some(true), Some(true)));
        assert_eq!(take(), ["cca-presence: 网页端的证书换了，已经重新读"]);

        // 文件没了：接着用手里的
        std::fs::remove_file(&cert_path).unwrap();
        assert_eq!(serves(524.0, &first), Some(true));
        assert_eq!(take(), [format!("cca-presence: 网页端的新证书读不了（没有 {cert_name}），接着用原来的")]);
        // 时钟倒着走（不该有）也不出错
        assert!(get(-5.0).is_some() && take().is_empty());

        // 建的时候文件就在：马上有（这一句经 err! 说到 stderr 上）
        f.put("fullchain.pem", &pem("CERTIFICATE", &first));
        let ready = CertCtx::new(&cert_path, &key_path, 0.0);
        assert!(ready.get(0.0).is_some() && ready.get(1e9).is_some());
    }
}
