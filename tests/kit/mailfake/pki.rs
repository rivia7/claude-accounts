//! 测试用的证书（原来的 make_pki）：一个 CA 和它签的 imap.gmail.com 的证书，EC P-256，用 openssl 命令生成（PATH 上找到的那一个，
//! LibreSSL、OpenSSL 3 都行）。辅助程序经 CCA_MAIL_CAFILE 信任这个 CA，照常校验证书。
//! 比原来的命令多写了 -sha256：macOS 自带的 LibreSSL（/usr/bin/openssl）签证书默认用 SHA-1，辅助程序（rustls）不认这种签名
//!（原来的 Python 服务端在 LibreSSL 下也起不来：CA_MD_TOO_WEAK）；OpenSSL 3 本来就默认 SHA-256，结果不变。

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const CNF: &str = "[req]\ndistinguished_name = dn\nprompt = no\n[dn]\nCN = cca test mail CA\n\
[v3_ca]\nbasicConstraints = critical, CA:TRUE\nkeyUsage = critical, keyCertSign, cRLSign\n\
subjectKeyIdentifier = hash\n\
[leaf]\nbasicConstraints = critical, CA:FALSE\nkeyUsage = critical, digitalSignature\n\
extendedKeyUsage = serverAuth\nsubjectAltName = DNS:imap.gmail.com\nsubjectKeyIdentifier = hash\n\
authorityKeyIdentifier = keyid:always\n";

/// 跑一条 openssl：输出都不要，退出码不是 0 算错。环境只带找 openssl 要的 PATH 和它自己的 OPENSSL_CONF
fn openssl(args: &[&OsStr]) -> io::Result<()> {
    let mut cmd = Command::new("openssl");
    cmd.args(args).env_clear().stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    for k in ["PATH", "OPENSSL_CONF"] {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    let st = cmd.status().map_err(|e| io::Error::new(e.kind(), format!("openssl 运行不了：{e}")))?;
    if !st.success() {
        let shown: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        return Err(io::Error::other(format!("openssl {} 失败（{st}）", shown.join(" "))));
    }
    Ok(())
}

/// d/ca.pem、d/cert.pem、d/key.pem（imap.gmail.com，CA 签的；Python 3.13 起默认的严格校验也过）。返回 CA 文件的路径
pub fn make_pki(d: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(d)?;
    let cnf = d.join("pki.cnf");
    fs::write(&cnf, CNF)?;
    let p = |n: &str| d.join(n);
    let (ca_key, ca, key, csr, cert) = (p("ca.key"), p("ca.pem"), p("key.pem"), p("leaf.csr"), p("cert.pem"));
    let s = |x: &'static str| OsStr::new(x);
    openssl(&[s("ecparam"), s("-name"), s("prime256v1"), s("-genkey"), s("-noout"), s("-out"), ca_key.as_os_str()])?;
    openssl(&[
        s("req"),
        s("-new"),
        s("-x509"),
        s("-key"),
        ca_key.as_os_str(),
        s("-config"),
        cnf.as_os_str(),
        s("-extensions"),
        s("v3_ca"),
        s("-sha256"),
        s("-days"),
        s("3"),
        s("-out"),
        ca.as_os_str(),
    ])?;
    openssl(&[s("ecparam"), s("-name"), s("prime256v1"), s("-genkey"), s("-noout"), s("-out"), key.as_os_str()])?;
    openssl(&[s("req"), s("-new"), s("-key"), key.as_os_str(), s("-subj"), s("/CN=imap.gmail.com"), s("-out"), csr.as_os_str()])?;
    openssl(&[
        s("x509"),
        s("-req"),
        s("-in"),
        csr.as_os_str(),
        s("-CA"),
        ca.as_os_str(),
        s("-CAkey"),
        ca_key.as_os_str(),
        s("-CAcreateserial"),
        s("-sha256"),
        s("-days"),
        s("2"),
        s("-extfile"),
        cnf.as_os_str(),
        s("-extensions"),
        s("leaf"),
        s("-out"),
        cert.as_os_str(),
    ])?;
    Ok(ca)
}
