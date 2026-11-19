//! `ssl`：在连好的套接字上做 TLS 客户端握手，照常校验证书和域名。只有邮箱助手用（连 Gmail 的 IMAP），只有 macOS 的构建带
//! （rustls + ring；别的系统上 `connect` 直接报错，邮箱助手本来也只在 macOS 上有）。
//!
//! 和原来 Python 的 `ssl.create_default_context()` 对应的地方：
//! - 最低 TLS 1.2；绝不跳过校验
//! - 信任的根证书是系统的 `/etc/ssl/cert.pem`（原来没装根证书的 Python 也是退到它）；`extra_ca` 是测试用的另一份 CA
//! - 证书校验没通过（`ssl.SSLCertVerificationError`）和别的握手失败（`OSError` / `ValueError`）分开报：调用方一个算
//!   “不用再试”，一个算“过会儿再试”
//!
//! 和 Python 不一样的地方：出错原因的文字是 rustls 的（原来是 OpenSSL 的 `verify_message`），对不上原文。

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// 握手没成的两类原因，里面是给人看的说明
#[derive(Debug)]
pub enum TlsError {
    /// 证书校验没通过（过期、不是可信的机构签的、域名对不上……）
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))] // 只有 macOS 的构建带 TLS
    Cert(String),
    /// 别的：连接断了、超时、对方不说 TLS、协议版本太旧……
    Other(String),
}

/// PEM 文件里的各张证书（DER）：`-----BEGIN CERTIFICATE-----` 到 `-----END CERTIFICATE-----` 之间的 base64
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // 只有 macOS 的构建带 TLS
pub fn pem_certificates(pem: &str) -> Vec<Vec<u8>> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let mut out = Vec::new();
    let mut rest = pem;
    while let Some(start) = rest.find(BEGIN) {
        let body = &rest[start + BEGIN.len()..];
        let Some(end) = body.find(END) else { break };
        if let Ok(der) = super::base64::a2b_base64(&body.as_bytes()[..end], false) {
            if !der.is_empty() {
                out.push(der);
            }
        }
        rest = &body[end + END.len()..];
    }
    out
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use rustls::pki_types::{CertificateDer, ServerName};
    use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
    use std::sync::Arc;

    /// 做好 TLS 的连接：读写的是明文
    pub struct TlsStream(StreamOwned<ClientConnection, TcpStream>);

    fn trusted(extra_ca: Option<&str>) -> RootCertStore {
        let mut roots = RootCertStore::empty();
        for file in ["/etc/ssl/cert.pem"].into_iter().chain(extra_ca) {
            if let Ok(pem) = std::fs::read(file) {
                for der in pem_certificates(&String::from_utf8_lossy(&pem)) {
                    let _ = roots.add(CertificateDer::from(der)); // 认不出的那一张跳过，别的照用
                }
            }
        }
        roots
    }

    /// 握手失败的原因归到哪一类
    fn classify(e: &io::Error) -> TlsError {
        match e.get_ref().and_then(|inner| inner.downcast_ref::<rustls::Error>()) {
            Some(rustls::Error::InvalidCertificate(why)) => TlsError::Cert(format!("{why:?}")),
            Some(other) => TlsError::Other(other.to_string()),
            None => TlsError::Other(crate::py::fail::os_error_text(e)),
        }
    }

    pub fn connect(sock: TcpStream, server_name: &str, extra_ca: Option<&str>) -> Result<TlsStream, TlsError> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
            .map_err(|e| TlsError::Other(e.to_string()))?
            .with_root_certificates(trusted(extra_ca))
            .with_no_client_auth();
        let name = ServerName::try_from(server_name.to_string()).map_err(|e| TlsError::Other(e.to_string()))?;
        let conn = ClientConnection::new(Arc::new(config), name).map_err(|e| TlsError::Other(e.to_string()))?;
        let mut stream = StreamOwned::new(conn, sock);
        // 握手现在就做完：证书的问题在这里报出来，不拖到第一次读写
        while stream.conn.is_handshaking() {
            stream.conn.complete_io(&mut stream.sock).map_err(|e| classify(&e))?;
        }
        Ok(TlsStream(stream))
    }

    impl TlsStream {
        /// `sock.settimeout(秒)`：读、写各自最多等这么久
        pub fn set_timeout(&self, secs: f64) -> io::Result<()> {
            let t = Some(Duration::from_secs_f64(secs));
            self.0.sock.set_read_timeout(t)?;
            self.0.sock.set_write_timeout(t)
        }

        /// 好好地关：发 close_notify，再关套接字
        pub fn close(mut self) {
            self.0.conn.send_close_notify();
            let _ = self.0.conn.complete_io(&mut self.0.sock);
            let _ = self.0.sock.shutdown(std::net::Shutdown::Both);
        }
    }

    impl Read for TlsStream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
    }

    impl Write for TlsStream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;

    /// 这个系统的构建不带 TLS：建不出来
    pub struct TlsStream(TcpStream);

    pub fn connect(_sock: TcpStream, _server_name: &str, _extra_ca: Option<&str>) -> Result<TlsStream, TlsError> {
        Err(TlsError::Other("这个系统上的 cca 不带 TLS（邮箱助手只在 macOS 上有）".into()))
    }

    impl TlsStream {
        pub fn set_timeout(&self, secs: f64) -> io::Result<()> {
            let t = Some(Duration::from_secs_f64(secs));
            self.0.set_read_timeout(t)?;
            self.0.set_write_timeout(t)
        }

        pub fn close(self) {}
    }

    impl Read for TlsStream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
    }

    impl Write for TlsStream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }
}

#[allow(unused_imports)] // 邮箱助手移植好之前还没人用
pub use imp::{connect, TlsStream};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pem_blocks_are_found_and_decoded() {
        let pem = "junk\n-----BEGIN CERTIFICATE-----\nAQID\nBAU=\n-----END CERTIFICATE-----\ntext between\n-----BEGIN CERTIFICATE-----\n/w==\n-----END CERTIFICATE-----\n-----BEGIN CERTIFICATE-----\nnot closed";
        assert_eq!(pem_certificates(pem), vec![vec![1, 2, 3, 4, 5], vec![0xff]]);
        assert!(
            pem_certificates("").is_empty()
                && pem_certificates("-----BEGIN PRIVATE KEY-----\nAQID\n-----END PRIVATE KEY-----").is_empty()
        );
    }

    /// 系统的根证书读得出来（macOS 上这个文件一直都有；别的系统不查）
    #[cfg(target_os = "macos")]
    #[test]
    fn system_roots_are_readable() {
        let pem = std::fs::read("/etc/ssl/cert.pem").unwrap();
        assert!(pem_certificates(&String::from_utf8_lossy(&pem)).len() > 50);
    }

    /// 对方不说 TLS：握手失败，归在“别的”那一类（不是证书的问题）
    #[cfg(target_os = "macos")]
    #[test]
    fn plain_text_peer_is_a_handshake_failure() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let _ = conn.write_all(b"* OK plain text, not TLS\r\n");
            let mut sink = [0u8; 1024];
            let _ = conn.read(&mut sink);
        });
        let sock = TcpStream::connect(addr).unwrap();
        sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        match connect(sock, "imap.example.com", None) {
            Err(TlsError::Other(why)) => assert!(!why.is_empty()),
            Err(TlsError::Cert(why)) => panic!("不该算证书的问题：{why}"),
            Ok(_) => panic!("不该握手成功"),
        }
        server.join().unwrap();
    }
}
