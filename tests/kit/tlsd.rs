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
        let key = PrivateKeyDer::from_pem_slice(key_pem).map_err(|e| bad("私钥读不出来", e))?;
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
