//! TLS 服务端（rustls）：从 PEM 的证书和私钥建接入器，给各个假服务用（假 Gmail 的 IMAP、以后别的假服务器）。
//! 握手在调用的线程上做完（阻塞）；连接断开时不发 close_notify（和 Python 的 ssl 一样：SSLSocket.close() 不发），对方读到的是连接直接关了。

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use std::io;
use std::net::TcpStream;
use std::path::Path;
use std::sync::Arc;

/// 握手完的连接：读写的是明文
pub type Stream = StreamOwned<ServerConnection, TcpStream>;

/// TLS 的接入器：一份证书链和私钥，接进来的连接各自握手
#[derive(Clone)]
pub struct Acceptor {
    cfg: Arc<ServerConfig>,
}

fn bad(what: &str, e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{what}：{e}"))
}

/// PEM 里的私钥（PKCS#8、SEC1、PKCS#1）。SEC1 的 P-256 私钥少了开头的 0 时补上：LibreSSL（macOS 自带的 openssl，实测 3.3.6）的
/// `ecparam -genkey` 照数值写私钥那一段，开头正好是 0 的（实测 1500 把里 8 把）就短一个字节，ring 不收（RFC 5915 要正好 32 个
/// 字节）。测试里拿 openssl 现造的私钥都经这里读；服务端自己读的时候也补（cli/src/server/x509.rs 的 `sec1_padded`）
pub fn private_key(key_pem: &[u8]) -> io::Result<PrivateKeyDer<'static>> {
    let key = PrivateKeyDer::from_pem_slice(key_pem).map_err(|e| bad("私钥读不出来", e))?;
    let padded = match &key {
        PrivateKeyDer::Sec1(sec1) => p256_padded(sec1.secret_sec1_der()),
        _ => None,
    };
    Ok(padded.map(|der| PrivateKeyDer::Sec1(der.into())).unwrap_or(key))
}

/// `30 LL 02 01 01 04 KK <KK 个字节的私钥> <后面的>`：KK 不到 32、最后是 P-256 的公钥（a1 44 03 42 00 04 X Y）的，私钥补成
/// 32 个字节；别的样子是 None
fn p256_padded(der: &[u8]) -> Option<Vec<u8>> {
    let (head, rest) = der.split_at_checked(7)?;
    let &[0x30, total, 0x02, 0x01, 0x01, 0x04, len] = head else { return None };
    let len = usize::from(len);
    if len >= 32 || usize::from(total) != der.len().checked_sub(2)? {
        return None;
    }
    let (scalar, tail) = rest.split_at_checked(len)?;
    let public = tail.len().checked_sub(70)?;
    if tail.get(public..public.checked_add(6)?)? != [0xa1, 0x44, 0x03, 0x42, 0x00, 0x04] {
        return None;
    }
    let body = u8::try_from(tail.len().checked_add(37)?).ok().filter(|n| *n < 0x80)?;
    let mut out = vec![0x30, body, 0x02, 0x01, 0x01, 0x04, 0x20];
    out.resize(out.len().checked_add(32usize.checked_sub(len)?)?, 0);
    out.extend_from_slice(scalar);
    out.extend_from_slice(tail);
    Some(out)
}

impl Acceptor {
    /// 从两个 PEM 文件建：证书文件里可以有好几张（第一张是自己的，后面是中间证书），私钥认 PKCS#8、SEC1（EC PRIVATE KEY）、PKCS#1（RSA）
    pub fn from_pem_files(cert: &Path, key: &Path) -> io::Result<Acceptor> {
        let certs = std::fs::read(cert)?;
        let key = std::fs::read(key)?;
        Acceptor::from_pem(&certs, &key)
    }

    /// 同上，内容已经读进来了
    pub fn from_pem(cert_pem: &[u8], key_pem: &[u8]) -> io::Result<Acceptor> {
        let certs: Vec<CertificateDer<'static>> =
            CertificateDer::pem_slice_iter(cert_pem).collect::<Result<_, _>>().map_err(|e| bad("证书读不出来", e))?;
        if certs.is_empty() {
            return Err(bad("证书读不出来", "文件里没有 CERTIFICATE"));
        }
        let key = private_key(key_pem)?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let cfg = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| bad("TLS 的设置", e))?
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| bad("证书和私钥用不了", e))?;
        Ok(Acceptor { cfg: Arc::new(cfg) })
    }

    /// rustls 的设置（要自己管连接的时候用）
    pub fn config(&self) -> Arc<ServerConfig> {
        self.cfg.clone()
    }

    /// 在这个连接上做服务端的握手（阻塞到做完）；握手失败、对方中途断开是 Err
    pub fn accept(&self, mut sock: TcpStream) -> io::Result<Stream> {
        let mut conn = ServerConnection::new(self.cfg.clone()).map_err(|e| bad("TLS", e))?;
        while conn.is_handshaking() {
            conn.complete_io(&mut sock)?;
        }
        // 握手后服务端要发的（TLS 1.3 的会话票据）也先发出去
        while conn.wants_write() {
            conn.write_tls(&mut sock)?;
        }
        Ok(StreamOwned::new(conn, sock))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{EcdsaKeyPair, ECDSA_P256_SHA256_ASN1_SIGNING};

    fn pem(der: &[u8]) -> Vec<u8> {
        format!("-----BEGIN EC PRIVATE KEY-----\n{}\n-----END EC PRIVATE KEY-----\n", crate::crypto::b64(der)).into_bytes()
    }

    /// LibreSSL 少写了开头那个 0 的私钥：ring 不收，补过的收，补出来的和够长的那一份一个字节不差
    #[test]
    fn short_libressl_keys_are_padded() {
        // ring 现造到私钥开头是 0 的那一把（PKCS#8：第 36 个字节起是私钥，最后 65 个字节是公钥那个点）
        let rng = ring::rand::SystemRandom::new();
        let new = || EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap().as_ref().to_vec();
        let pkcs8 = std::iter::repeat_with(new).find(|k| k[36] == 0).unwrap();
        let (scalar, point) = (&pkcs8[36..68], &pkcs8[pkcs8.len() - 65..]);
        // openssl ecparam -name prime256v1 -genkey -noout 写出来的样子：私钥、曲线的参数、公钥
        let tail =
            [&[0xa0, 0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0xa1, 0x44, 0x03, 0x42, 0x00][..], point]
                .concat();
        let sec1 = |scalar: &[u8]| {
            [&[0x30, (5 + scalar.len() + tail.len()) as u8, 0x02, 0x01, 0x01, 0x04, scalar.len() as u8][..], scalar, &tail].concat()
        };
        let (full, short) = (sec1(scalar), sec1(&scalar[1..]));
        assert_eq!(
            (full.len(), &short[..7]),
            (121, &[0x30, 0x76, 0x02, 0x01, 0x01, 0x04, 0x1f][..]),
            "实测 LibreSSL 写的就是这个开头"
        );
        assert_eq!(p256_padded(&short), Some(full.clone()));
        for same in [&full[..], &short[..60], b"", b"0v", &pkcs8] {
            assert_eq!(p256_padded(same), None);
        }
        let provider = rustls::crypto::ring::default_provider();
        let raw = PrivateKeyDer::from_pem_slice(&pem(&short)).unwrap();
        assert!(provider.key_provider.load_private_key(raw).is_err(), "ring 收了短的：补的那一步可以去掉了");
        let fixed = private_key(&pem(&short)).unwrap();
        assert_eq!(fixed.secret_der(), &full[..]);
        assert!(provider.key_provider.load_private_key(fixed).is_ok());
        assert_eq!(private_key(&pem(&full)).unwrap().secret_der(), &full[..], "够长的原样");
    }
}
