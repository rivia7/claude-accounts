//! 连证书机构时信哪些根证书。
//! - 给了 --ca：只信那个文件里的（测试用。Python 的 `ssl.create_default_context(cafile=…)` 给了文件也就不再看系统的）
//! - 没给：系统的那一份——环境变量 SSL_CERT_FILE 指的文件，没设就是几个常见位置里头一个有的——再加上编在程序里的
//!   Let's Encrypt 的两张根证书（ISRG Root X1、X2）。默认的证书机构就是它：跑在没装根证书的最小镜像里也连得上
//!
//! Python 那一份靠的是 OpenSSL 默认的位置（它还看 SSL_CERT_DIR 那种一张证书一个文件的目录；这里不看）。
//!
//! 那两张根证书是从 Mozilla 的根证书表里原样取出来的（isrg-root-x1.pem、isrg-root-x2.pem），测试里核对它们的 SHA-256 指纹：
//! 和 <https://letsencrypt.org/certificates/> 上公布的一样才对。X1 到 2035 年、X2 到 2040 年；Let's Encrypt 换根证书时这里要跟着加。

use super::Trouble;
use crate::py::fail::os_error_text;
use crate::py::path;
use crate::py::tls::pem_certificates;
use rustls::pki_types::CertificateDer;
use rustls::RootCertStore;

/// 编在程序里的根证书
const EMBEDDED: [&str; 2] = [include_str!("isrg-root-x1.pem"), include_str!("isrg-root-x2.pem")];

/// 系统的根证书常见的位置：Debian、Ubuntu、Alpine；RHEL、Fedora；openSUSE；RHEL 新的位置；macOS、BSD
const SYSTEM: [&str; 5] = [
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/ca-bundle.pem",
    "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem",
    "/etc/ssl/cert.pem",
];

/// 把 PEM 里的各张证书加进去，返回加了几张（认不出的那一张跳过，别的照用）
fn add(roots: &mut RootCertStore, pem: &str) -> usize {
    pem_certificates(pem).into_iter().filter(|der| roots.add(CertificateDer::from(der.clone())).is_ok()).count()
}

/// env_file 是环境变量 SSL_CERT_FILE 的值，system 是没设它时挨个看的那几个位置
fn trust_from(ca: Option<&str>, env_file: Option<&str>, system: &[&str]) -> Result<RootCertStore, Trouble> {
    let mut roots = RootCertStore::empty();
    if let Some(ca) = ca.filter(|ca| !ca.is_empty()) {
        // 读不了：照 Python 的说法，只有原因、不带文件名
        let pem = std::fs::read(ca).map_err(|e| Trouble::Os(os_error_text(&e)))?;
        if add(&mut roots, &String::from_utf8_lossy(&pem)) == 0 {
            return Err(Trouble::Os(format!("{ca} 里没有证书")));
        }
        return Ok(roots);
    }
    let file = match env_file.filter(|file| !file.is_empty()) {
        Some(file) => Some(file),
        None => system.iter().copied().find(|file| path::isfile(file)),
    };
    // 系统的那一份读不了不算错：还有编进来的那两张
    if let Some(pem) = file.and_then(|file| std::fs::read(file).ok()) {
        add(&mut roots, &String::from_utf8_lossy(&pem));
    }
    for pem in EMBEDDED {
        add(&mut roots, pem);
    }
    Ok(roots)
}

/// 连证书机构时信的根证书。ca 是 --ca 给的文件（没给、给的是空的：照系统的）
pub fn trust(ca: Option<&str>) -> Result<RootCertStore, Trouble> {
    let env_file = std::env::var_os("SSL_CERT_FILE");
    trust_from(ca, env_file.as_deref().and_then(|file| file.to_str()), &SYSTEM)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::py::fs::TempDir;
    use crate::py::hash::{sha256, to_hex};
    use crate::py::time::timegm;
    use crate::server::x509::{cert_info, testkit as certs};

    /// 编进来的是 ISRG Root X1、X2：SHA-256 指纹和 Let's Encrypt 公布的一样，到期时间也对
    #[test]
    fn embedded_roots_are_the_isrg_ones() {
        let want = [
            ("96bcec06264976f37460779acf28c5a7cfe8a3c0aae11a8ffcee05c0bddf08c6", timegm(2035, 6, 4, 11, 4, 38)),
            ("69729b8e15a86efc177a57afb7171dfc64add28c2fca8cf1507e34453ccb1470", timegm(2040, 9, 17, 16, 0, 0)),
        ];
        for (pem, (fingerprint, not_after)) in EMBEDDED.iter().zip(want) {
            let ders = pem_certificates(pem);
            assert_eq!(ders.len(), 1);
            assert_eq!(to_hex(&sha256(&ders[0])), fingerprint);
            assert_eq!(cert_info(&ders[0]).unwrap().not_after, not_after);
        }
        let mut roots = RootCertStore::empty();
        assert_eq!((add(&mut roots, EMBEDDED[0]), add(&mut roots, EMBEDDED[1]), roots.len()), (1, 1, 2));
    }

    fn pem_of(n: usize) -> String {
        (0..n).map(|_| certs::pem("CERTIFICATE", &certs::self_signed(&certs::new_key(), Some(&[])))).collect()
    }

    #[test]
    fn which_roots_are_trusted() {
        let d = TempDir::new().unwrap();
        let file = |name: &str, text: &str| {
            let p = format!("{}/{name}", d.path);
            std::fs::write(&p, text).unwrap();
            p
        };
        let (one, three, junk) =
            (file("one.pem", &pem_of(1)), file("three.pem", &pem_of(3)), file("junk.pem", "not a certificate\n"));
        let missing = format!("{}/missing.pem", d.path);
        let len = |ca: Option<&str>, env: Option<&str>, system: &[&str]| trust_from(ca, env, system).map(|roots| roots.len());
        // --ca：只有那个文件里的
        assert_eq!(len(Some(&one), Some(&three), &[&three]), Ok(1));
        assert_eq!(len(Some(&three), None, &[]), Ok(3));
        assert_eq!(len(Some(&missing), None, &[&three]), Err(Trouble::Os("[Errno 2] No such file or directory".into())));
        assert_eq!(len(Some(&d.path), None, &[]), Err(Trouble::Os("[Errno 21] Is a directory".into())));
        assert_eq!(len(Some(&junk), None, &[]), Err(Trouble::Os(format!("{junk} 里没有证书"))));
        // 没给（给的是空的也算没给）：系统的加上编进来的两张
        assert_eq!(len(None, None, &[]), Ok(2));
        assert_eq!(len(Some(""), None, &[&missing, &three, &one]), Ok(5), "头一个有的");
        assert_eq!(len(None, Some(&one), &[&three]), Ok(3), "SSL_CERT_FILE 说了算");
        assert_eq!(len(None, Some(&missing), &[&three]), Ok(2), "它指的文件不在：不再去找别的");
        assert_eq!(len(None, Some(""), &[&three]), Ok(5));
        assert_eq!(len(None, None, &[&junk]), Ok(2));
        // 这台机器上真的那一份
        assert!(trust(None).unwrap().len() >= 2);
    }
}
