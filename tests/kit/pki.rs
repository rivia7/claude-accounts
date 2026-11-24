//! 测试用的密钥和证书（原来 tests/presence.py 开头的那几个函数，别的几份测试也用）：ssh-keygen 生成的 ed25519 密钥和它签的名，
//! openssl 现造的服务器证书（自签的、证书机构签的），照管理员那边的办法（openssl）算证书公钥的指纹。
//!
//! 都是照 PATH 找到的真命令跑（子进程的环境里只有 PATH；openssl 另带它自己的 OPENSSL_CONF），出了错直接 panic、说清楚是哪一条：
//! 测试里密钥、证书造不出来就是用例失败。没有 ssh-keygen、openssl 的机器上，用例自己先 `needs(&[Need::Bin("openssl")])`。
//!
//! 和原来的命令只差一处：签证书的那几条多写了 `-sha256`。macOS 自带的 LibreSSL（/usr/bin/openssl）签证书默认用 SHA-1，
//! rustls（辅助程序里的服务器、kit::tls）不认这种签名；OpenSSL 3 本来就默认 SHA-256，结果不变。

use crate::crypto;
use crate::proc::{Cmd, Stdio};
use crate::spec;
use std::ffi::OsStr;
use std::path::Path;

/// [`keygen`] 给密钥写的备注（原来 keygen 的 comment 的默认值）
pub const COMMENT: &str = "test";

/// 生成一把 ed25519 密钥（`ssh-keygen -t ed25519 -N ''`）：私钥是 path，公钥是 path.pub，备注是 [`COMMENT`]。
/// 给出公钥那一行的前两段（`ssh-ed25519 AAAA…`：登记、加入时交的就是这个样子）
pub fn keygen(path: impl AsRef<Path>) -> String {
    keygen_as(path, COMMENT)
}

/// 同 [`keygen`]，备注另写（`cca-team-root`、`cca-team@设备名` 这些）
pub fn keygen_as(path: impl AsRef<Path>, comment: &str) -> String {
    let path = path.as_ref();
    let mut cmd = Cmd::new("ssh-keygen");
    cmd.args(["-q", "-t", "ed25519", "-N", "", "-C", comment, "-f"]).arg(path);
    cmd.pass_env(&["PATH"]).stdout(Stdio::Null).stderr(Stdio::Null);
    let out = cmd.output().unwrap_or_else(|e| panic!("ssh-keygen 运行不了：{e}"));
    assert!(out.status.success(), "ssh-keygen 没生成 {}：{}", path.display(), out.status);
    let mut pubfile = path.as_os_str().to_os_string();
    pubfile.push(".pub");
    spec::pub_line(&crate::fsx::read_text(&pubfile))
}

/// 拿这把私钥（文件）在命名空间 ns 里给 data 签名（`ssh-keygen -Y sign`）：给的是 armored 的签名。签不了就 panic
pub fn sign(key: impl AsRef<Path>, ns: &str, data: &[u8]) -> Vec<u8> {
    spec::ssh_sign(key.as_ref(), ns, data, true).unwrap_or_else(|e| panic!("{e}"))
}

/// 跑一条 openssl：标准输出收回来，它在 stderr 上说的不要；退出码不是 0 就 panic
fn openssl<S: AsRef<OsStr>>(args: &[S], input: &[u8]) -> Vec<u8> {
    let shown = || args.iter().map(|a| a.as_ref().to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ");
    let mut cmd = Cmd::new("openssl");
    cmd.args(args).pass_env(&["PATH", "OPENSSL_CONF"]).input(input).stderr(Stdio::Null);
    let out = cmd.output().unwrap_or_else(|e| panic!("openssl 运行不了：{e}"));
    assert!(out.status.success(), "openssl {} 没成：{}", shown(), out.status);
    out.stdout
}

/// 路径当 openssl 的参数
fn arg(path: &Path) -> &OsStr {
    path.as_os_str()
}

/// 自签的服务器证书：d/key.pem（P-256）、d/cert.pem（CN=cca-presence，2 天）。设备不照证书机构验它，靠钉住公钥（[`openssl_pin`]）
pub fn make_cert(d: &Path) {
    let (key, cert) = (d.join("key.pem"), d.join("cert.pem"));
    let s = OsStr::new;
    openssl(&[s("ecparam"), s("-name"), s("prime256v1"), s("-genkey"), s("-noout"), s("-out"), arg(&key)], b"");
    openssl(
        &[
            s("req"),
            s("-new"),
            s("-x509"),
            s("-sha256"),
            s("-key"),
            arg(&key),
            s("-subj"),
            s("/CN=cca-presence"),
            s("-days"),
            s("2"),
            s("-out"),
            arg(&cert),
        ],
        b"",
    );
}

/// 证书机构签的证书（SAN 是 127.0.0.1）：照证书机构验的服务器（托管平台上的 Team 的服务器、网页端）用；设备的 curl 经
/// CURL_CA_BUNDLE=ca.pem 认这个机构。d 下写出 ca.key、ca.pem（CN=cca-test-ca）、key.pem、csr.pem、ext.cnf、cert.pem（CN=127.0.0.1）
pub fn make_ca_cert(d: &Path) {
    let p = |name: &str| d.join(name);
    let (ca_key, ca, key, csr, ext, cert) = (p("ca.key"), p("ca.pem"), p("key.pem"), p("csr.pem"), p("ext.cnf"), p("cert.pem"));
    let s = OsStr::new;
    openssl(&[s("ecparam"), s("-name"), s("prime256v1"), s("-genkey"), s("-noout"), s("-out"), arg(&ca_key)], b"");
    openssl(
        &[
            s("req"),
            s("-new"),
            s("-x509"),
            s("-sha256"),
            s("-key"),
            arg(&ca_key),
            s("-subj"),
            s("/CN=cca-test-ca"),
            s("-days"),
            s("2"),
            s("-out"),
            arg(&ca),
        ],
        b"",
    );
    openssl(&[s("ecparam"), s("-name"), s("prime256v1"), s("-genkey"), s("-noout"), s("-out"), arg(&key)], b"");
    openssl(&[s("req"), s("-new"), s("-key"), arg(&key), s("-subj"), s("/CN=127.0.0.1"), s("-out"), arg(&csr)], b"");
    crate::fsx::write(&ext, "subjectAltName=IP:127.0.0.1\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n");
    openssl(
        &[
            s("x509"),
            s("-req"),
            s("-sha256"),
            s("-in"),
            arg(&csr),
            s("-CA"),
            arg(&ca),
            s("-CAkey"),
            arg(&ca_key),
            s("-CAcreateserial"),
            s("-days"),
            s("2"),
            s("-extfile"),
            arg(&ext),
            s("-out"),
            arg(&cert),
        ],
        b"",
    );
}

/// 管理员那边（安装脚本、文档）算证书公钥的指纹的办法：`openssl x509 -pubkey` 取出公钥、`openssl pkey -outform der` 转成 DER，
/// 给出 `sha256//` 加它的 SHA-256 的 base64。和服务器的 --print-pin、设备钉住的 pins 必须一样
pub fn openssl_pin(cert: impl AsRef<Path>) -> String {
    let s = OsStr::new;
    let public = openssl(&[s("x509"), s("-in"), arg(cert.as_ref()), s("-pubkey"), s("-noout")], b"");
    let der = openssl(&["pkey", "-pubin", "-outform", "der"], &public);
    format!("sha256//{}", crypto::b64(&crypto::sha256(&der)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsx::{self, TempDir};
    use crate::proc::which;
    use crate::tls;

    #[test]
    fn keys_and_signatures() {
        if which("ssh-keygen").is_none() {
            return;
        }
        let d = TempDir::new("cca-pki-test-");
        let public = keygen(d.join("k"));
        let other = keygen_as(d.join("o"), "cca-team@某台设备");
        // 公钥那一行的前两段；备注在文件里、不在给出来的那一行里
        let parts: Vec<&str> = public.split(' ').collect();
        assert_eq!((parts.len(), parts[0]), (2, "ssh-ed25519"), "{public}");
        assert!(fsx::read_text(d.join("k.pub")).trim_end().ends_with(" test"));
        assert!(fsx::read_text(d.join("o.pub")).trim_end().ends_with(" cca-team@某台设备"));
        assert_eq!(fsx::mode(d.join("k")), 0o600);
        let sig = sign(d.join("k"), "cca-presence", b"hello");
        assert!(sig.starts_with(b"-----BEGIN SSH SIGNATURE-----"), "{}", String::from_utf8_lossy(&sig));
        assert!(spec::ssh_verify(&public, "cca-presence", b"hello", &sig));
        assert!(!spec::ssh_verify(&public, "cca-presence", b"hello!", &sig), "消息改过");
        assert!(!spec::ssh_verify(&public, "cca-devices", b"hello", &sig), "命名空间不对");
        assert!(!spec::ssh_verify(&other, "cca-presence", b"hello", &sig), "不是这把公钥签的");
    }

    #[test]
    fn certificates_and_pins() {
        if which("openssl").is_none() {
            return;
        }
        let d = TempDir::new("cca-pki-test-");
        make_cert(&d);
        let pin = openssl_pin(d.join("cert.pem"));
        // openssl 算的和照证书的结构自己数出来的（kit::tls）是同一个
        let der = tls::pem_certs(&fsx::read(d.join("cert.pem"))).expect("证书读得出来");
        assert_eq!(Some(pin.clone()), tls::pin_of(&der[0]));
        assert_eq!((pin.len(), &pin[..8], &pin[51..]), (52, "sha256//", "="), "{pin}");
        // 证书机构签的那一张：照这个机构验得过（rustls：签名是 SHA-256 的、名字是 IP 127.0.0.1），不照它验不过
        let ca = TempDir::new("cca-pki-test-");
        make_ca_cert(&ca);
        for name in ["ca.key", "ca.pem", "key.pem", "csr.pem", "ext.cnf", "cert.pem"] {
            assert!(ca.join(name).exists(), "{name}");
        }
        let acceptor = crate::tlsd::Acceptor::from_pem_files(&ca.join("cert.pem"), &ca.join("key.pem")).expect("证书和私钥用得了");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑端口");
        let port = listener.local_addr().expect("端口").port();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                if let Ok((sock, _)) = listener.accept() {
                    let _ = acceptor.accept(sock);
                }
            }
        });
        let limit = std::time::Duration::from_secs(10);
        let trusted = tls::Client::with_ca_file(ca.join("ca.pem")).expect("CA 读得出来");
        let stream = trusted.connect("127.0.0.1", port, limit).expect("照测试的证书机构验得过");
        assert_eq!(stream.peer_pin(), Some(openssl_pin(ca.join("cert.pem"))));
        drop(stream);
        let stranger = tls::Client::with_ca_file(d.join("cert.pem")).expect("读得出来");
        assert!(stranger.connect("127.0.0.1", port, limit).is_err(), "别的机构：验不过");
        server.join().expect("服务端的线程");
    }
}
