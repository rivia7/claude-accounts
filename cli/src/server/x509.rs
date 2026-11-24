//! X.509 证书里用得着的那几样：有效期、subjectAltName 里的地址、公钥（SubjectPublicKeyInfo）。
//!
//! Python 那一份是起 openssl 来看的（cca-acme.py 的 cert_state，Acme:369-391）；Rust 版不起外部命令，自己读 DER。
//! 只取这几样，不验证书（签名、证书链、用途）：那是握手时对方的事。这里要知道的只是“这张证书是不是给这几个地址的、是不是这把私钥的、
//! 什么时候到期”。v1 的证书（没有扩展：macOS 自带的 LibreSSL 自签出来的就是）也读得了。
//!
//! 证书文件是别的程序写的（证书机构给的、安装脚本生成的）：读不懂一律是 None，不 panic。长度只认 DER 的写法（最短的那一种）。

use super::wire::Cur;
use crate::py::ipaddr::{Ip, Parsed};
use crate::py::time;
use std::collections::BTreeSet;
use std::net::{Ipv4Addr, Ipv6Addr};

const BOOLEAN: u8 = 0x01;
const INTEGER: u8 = 0x02;
const BIT_STRING: u8 = 0x03;
const OCTET_STRING: u8 = 0x04;
const OID: u8 = 0x06;
const UTC_TIME: u8 = 0x17;
const GENERALIZED_TIME: u8 = 0x18;
const SEQUENCE: u8 = 0x30;
/// TBSCertificate 里的 [0]（版本）、[3]（扩展）
const VERSION: u8 = 0xa0;
const EXTENSIONS: u8 = 0xa3;
/// ECPrivateKey 里带着的公钥：[1]
const EC_PUBLIC: u8 = 0xa1;
/// GeneralName 里的 [2] dNSName、[7] iPAddress
const DNS_NAME: u8 = 0x82;
const IP_ADDRESS: u8 = 0x87;
/// subjectAltName 的 OID：2.5.29.17
const SAN_OID: &[u8] = &[0x55, 0x1d, 0x11];

/// 证书是给谁的：subjectAltName 里的一项。证书助手要签的地址（ACME 的 identifier）也用它，两边直接比
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum San {
    /// IP 地址，写成 Python 的 `str(ipaddress.ip_address(…))` 那个样子
    Ip(String),
    /// 域名，小写
    Dns(String),
}

impl San {
    /// ACME 的 identifier 里的 type
    pub fn kind(&self) -> &'static str {
        match self {
            San::Ip(_) => "ip",
            San::Dns(_) => "dns",
        }
    }

    /// ACME 的 identifier 里的 value
    pub fn value(&self) -> &str {
        match self {
            San::Ip(v) | San::Dns(v) => v,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CertInfo {
    /// 有效期从什么时候起（时间戳）
    pub not_before: i64,
    /// 到什么时候
    pub not_after: i64,
    /// subjectAltName 里的 IP 地址和域名（别的种类不算）；没有这个扩展是空的
    pub san: BTreeSet<San>,
    /// 公钥：SubjectPublicKeyInfo 的 DER，连头带尾（`openssl pkey -pubin -outform DER` 给的就是它）
    pub spki: Vec<u8>,
}

/// DER 的一项
struct Item<'a> {
    tag: u8,
    /// 内容
    body: &'a [u8],
    /// 连标签、长度在内的整项
    whole: &'a [u8],
}

/// 往后读一项。不认的：多字节的标签、不定长的（BER）、长度没用最短写法的、长度超过 4 个字节的
fn item<'a>(cur: &mut Cur<'a>) -> Option<Item<'a>> {
    let start = cur.rest();
    let tag = cur.u8()?;
    if tag & 0x1f == 0x1f {
        return None;
    }
    let first = cur.u8()?;
    let len = if first < 0x80 {
        usize::from(first)
    } else {
        let n = usize::from(first & 0x7f);
        if n == 0 || n > 4 {
            return None;
        }
        let mut len = 0usize;
        for b in cur.take(n)? {
            len = len.checked_mul(256)?.checked_add(usize::from(*b))?;
        }
        let least = if n == 1 { 0x80 } else { 1usize << (8 * (n - 1)) };
        if len < least {
            return None;
        }
        len
    };
    let body = cur.take(len)?;
    let used = start.len().checked_sub(cur.rest().len())?;
    Some(Item { tag, body, whole: start.get(..used)? })
}

/// 往后读一项，标签得是这个
fn expect<'a>(cur: &mut Cur<'a>, tag: u8) -> Option<Item<'a>> {
    item(cur).filter(|i| i.tag == tag)
}

/// 一串十进制数字
fn digits(b: &[u8]) -> Option<i64> {
    if b.is_empty() || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    b.iter().try_fold(0i64, |n, d| n.checked_mul(10)?.checked_add(i64::from(d.checked_sub(b'0')?)))
}

/// 证书里的一个时间 → 时间戳。只认 RFC 5280 的两种写法：UTCTime `YYMMDDHHMMSSZ`（50 起算 19 几年）、
/// GeneralizedTime `YYYYMMDDHHMMSSZ`；不带秒的、带时区的、日期不存在的都不认
fn time_of(t: &Item<'_>) -> Option<i64> {
    let mut c = Cur::new(t.body);
    let year = match t.tag {
        UTC_TIME => match digits(c.take(2)?)? {
            yy if yy >= 50 => 1900 + yy,
            yy => 2000 + yy,
        },
        GENERALIZED_TIME => digits(c.take(4)?)?,
        _ => return None,
    };
    let mut two = || digits(c.take(2)?);
    let (mon, day, hour, min, sec) = (two()?, two()?, two()?, two()?, two()?);
    if c.rest() != b"Z" {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match mon {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if !(1..=days).contains(&day) || hour > 23 || min > 59 || sec > 59 {
        return None;
    }
    Some(time::timegm(year, mon, day, hour, min, sec))
}

/// TBSCertificate 里从头走到公钥：(有效期那一项, 公钥那一项, 公钥后面剩下的)
fn walk(der: &[u8]) -> Option<(Item<'_>, Item<'_>, Cur<'_>)> {
    let mut top = Cur::new(der);
    let cert = expect(&mut top, SEQUENCE)?;
    if !top.is_empty() {
        return None; // 证书后面还有东西
    }
    let mut c = Cur::new(cert.body);
    let tbs = expect(&mut c, SEQUENCE)?;
    let mut t = Cur::new(tbs.body);
    let mut serial = item(&mut t)?;
    if serial.tag == VERSION {
        serial = item(&mut t)?; // v1 的证书没有版本这一项
    }
    if serial.tag != INTEGER {
        return None;
    }
    expect(&mut t, SEQUENCE)?; // 签名算法
    expect(&mut t, SEQUENCE)?; // 签发者
    let validity = expect(&mut t, SEQUENCE)?;
    expect(&mut t, SEQUENCE)?; // 证书是给谁的（名字）
    let spki = expect(&mut t, SEQUENCE)?;
    Some((validity, spki, t))
}

/// IP 地址的字节 → Python 的 `str(ip)`；不是 4 个、16 个字节的不认（openssl 把它打成 `<invalid>`，Python 那边跟着不认）
fn ip_text(b: &[u8]) -> Option<String> {
    let ip = match b.len() {
        4 => Ip::V4(Ipv4Addr::from(<[u8; 4]>::try_from(b).ok()?)),
        16 => Ip::V6(Ipv6Addr::from(<[u8; 16]>::try_from(b).ok()?)),
        _ => return None,
    };
    Some(Parsed { ip, scope: None }.text())
}

/// 域名：看得见的 ASCII（不带逗号：Python 是从 openssl 打出来的那一行里按逗号、空白切的，带这些的本来也不是域名），转成小写
fn dns_text(b: &[u8]) -> Option<String> {
    let name = std::str::from_utf8(b).ok()?;
    (!name.is_empty() && name.bytes().all(|c| c.is_ascii_graphic() && c != b',')).then(|| name.to_ascii_lowercase())
}

/// [3] 里的各个扩展：subjectAltName 里的 IP 地址和域名加进 san
fn read_extensions(body: &[u8], san: &mut BTreeSet<San>) -> Option<()> {
    let mut outer = Cur::new(body);
    let list = expect(&mut outer, SEQUENCE)?;
    if !outer.is_empty() {
        return None;
    }
    let mut exts = Cur::new(list.body);
    while !exts.is_empty() {
        let ext = expect(&mut exts, SEQUENCE)?;
        let mut e = Cur::new(ext.body);
        let oid = expect(&mut e, OID)?;
        let mut value = item(&mut e)?;
        if value.tag == BOOLEAN {
            value = item(&mut e)?; // critical：可以没有
        }
        if value.tag != OCTET_STRING || !e.is_empty() {
            return None;
        }
        if oid.body != SAN_OID {
            continue;
        }
        let mut inner = Cur::new(value.body);
        let names = expect(&mut inner, SEQUENCE)?;
        if !inner.is_empty() {
            return None;
        }
        let mut n = Cur::new(names.body);
        while !n.is_empty() {
            let name = item(&mut n)?;
            match name.tag {
                DNS_NAME => san.insert(San::Dns(dns_text(name.body)?)),
                IP_ADDRESS => san.insert(San::Ip(ip_text(name.body)?)),
                _ => false, // 邮箱、URI 这些不是要比的地址（Python 也只认 DNS 和 IP Address 两种）
            };
        }
    }
    Some(())
}

/// 一张证书（DER）的有效期、地址、公钥；看不懂是 None
pub fn cert_info(der: &[u8]) -> Option<CertInfo> {
    let (validity, spki, mut rest) = walk(der)?;
    let mut v = Cur::new(validity.body);
    let not_before = time_of(&item(&mut v)?)?;
    let not_after = time_of(&item(&mut v)?)?;
    if !v.is_empty() {
        return None;
    }
    let mut san = BTreeSet::new();
    while !rest.is_empty() {
        let extra = item(&mut rest)?;
        if extra.tag == EXTENSIONS {
            read_extensions(extra.body, &mut san)?; // 别的是 issuerUniqueID、subjectUniqueID：不看
        }
    }
    Some(CertInfo { not_before, not_after, san, spki: spki.whole.to_vec() })
}

/// 只要公钥（SubjectPublicKeyInfo 的 DER）：证书是不是这把私钥的，比的就是它。有效期、扩展写得怪不怪不管
/// （Python 的 `load_cert_chain` 也只看这一对合不合）
pub fn cert_spki(der: &[u8]) -> Option<Vec<u8>> {
    walk(der).map(|(_, spki, _)| spki.whole.to_vec())
}

/// SEC1 的私钥（RFC 5915 的 ECPrivateKey）里私钥那一段比曲线的长度短时，前面补上 0，给出补好的整份；不用补、看不懂的是 None。
/// RFC 5915 要这一段正好是曲线的长度，ring 照这个查；LibreSSL（实测 3.3.6，macOS 自带的就是它）的 `openssl ecparam -genkey`
/// 却照数值写：开头正好是 0 的私钥就少一个字节（实测 1500 把里 8 把），Python 的 ssl 读得了这样的。
/// 曲线多长从后面带着的公钥看（04 X Y：去掉开头那个字节的一半）；没带公钥的不管
pub fn sec1_padded(sec1: &[u8]) -> Option<Vec<u8>> {
    let mut outer = Cur::new(sec1);
    let seq = expect(&mut outer, SEQUENCE)?;
    let mut cur = Cur::new(seq.body);
    let version = expect(&mut cur, INTEGER)?;
    let secret = expect(&mut cur, OCTET_STRING)?;
    let tail = cur.rest();
    let mut size = None;
    while !cur.is_empty() {
        let part = item(&mut cur)?;
        if part.tag == EC_PUBLIC {
            // BIT STRING：头一个字节是“最后有几位不用”，后面是那个点
            let point = expect(&mut Cur::new(part.body), BIT_STRING)?.body.get(1..)?;
            size = point.first().filter(|form| **form == 4).map(|_| point.len() / 2);
        }
    }
    // 最长的曲线（P-521）是 66 个字节
    let missing = size.filter(|n| *n <= 66)?.checked_sub(secret.body.len()).filter(|n| *n > 0)?;
    let padded = [vec![0u8; missing].as_slice(), secret.body].concat();
    let der = super::acme::der::der;
    Some(der(SEQUENCE, &[version.whole, &der(OCTET_STRING, &[&padded]), tail]))
}

/// PEM 文件里的头一张证书（带中间证书的文件里它就是服务器自己的那张）
pub fn cert_info_pem(pem: &[u8]) -> Option<CertInfo> {
    cert_info(crate::py::tls::pem_certificates(&String::from_utf8_lossy(pem)).first()?)
}

/// 测试里造证书用的（这里的测试、tls.rs 的测试都用）
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
pub(super) mod testkit {
    use super::San;
    use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING};

    /// P-256 的公钥在 SubjectPublicKeyInfo 里那 65 个字节前面的部分
    pub const P256_SPKI_HEAD: [u8; 26] = [
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d,
        0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
    ];

    /// 一项 DER：标签、长度、接起来的内容
    pub fn der(tag: u8, parts: &[&[u8]]) -> Vec<u8> {
        let body = parts.concat();
        let mut out = vec![tag];
        match body.len() {
            n if n < 0x80 => out.push(n as u8),
            n if n < 0x100 => out.extend([0x81, n as u8]),
            n => out.extend([0x82, (n >> 8) as u8, n as u8]),
        }
        out.extend(body);
        out
    }

    /// 时间：13 个字符的是 UTCTime，别的是 GeneralizedTime
    pub fn time(t: &str) -> Vec<u8> {
        der(if t.len() == 13 { 0x17 } else { 0x18 }, &[t.as_bytes()])
    }

    /// subjectAltName 里的一项
    pub fn general_name(s: &San) -> Vec<u8> {
        match s {
            San::Dns(name) => der(0x82, &[name.as_bytes()]),
            San::Ip(ip) => match ip.parse::<std::net::IpAddr>().unwrap() {
                std::net::IpAddr::V4(v4) => der(0x87, &[&v4.octets()]),
                std::net::IpAddr::V6(v6) => der(0x87, &[&v6.octets()]),
            },
        }
    }

    /// 一张证书的样子（不签名：签名那一段是假的），各项可以换：读证书的测试用
    pub struct Shape {
        /// 带不带 [0] 版本（v3）
        pub v3: bool,
        pub not_before: Vec<u8>,
        pub not_after: Vec<u8>,
        pub spki: Vec<u8>,
        /// [3] 里的各个扩展（每个是一整项）；空的就不带 [3]
        pub extensions: Vec<Vec<u8>>,
    }

    impl Shape {
        pub fn new() -> Shape {
            Shape {
                v3: true,
                not_before: time("260101000000Z"),
                not_after: time("20510607080910Z"),
                spki: [&P256_SPKI_HEAD[..], &[4u8; 65]].concat(),
                extensions: Vec::new(),
            }
        }

        /// TBSCertificate
        pub fn tbs(&self) -> Vec<u8> {
            let alg = der(0x30, &[&der(0x06, &[&[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02]])]); // ecdsa-with-SHA256
            let name = der(0x30, &[&der(0x31, &[&der(0x30, &[&der(0x06, &[&[0x55, 0x04, 0x03]]), &der(0x0c, &[b"cca-test"])])])]);
            let mut parts: Vec<Vec<u8>> = Vec::new();
            if self.v3 {
                parts.push(der(0xa0, &[&der(0x02, &[&[2]])]));
            }
            parts.push(der(0x02, &[&[0x01]])); // 序列号
            parts.push(alg);
            parts.push(name.clone());
            parts.push(der(0x30, &[&self.not_before, &self.not_after]));
            parts.push(name);
            parts.push(self.spki.clone());
            if !self.extensions.is_empty() {
                let all: Vec<&[u8]> = self.extensions.iter().map(Vec::as_slice).collect();
                parts.push(der(0xa3, &[&der(0x30, &all)]));
            }
            let all: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
            der(0x30, &all)
        }

        /// 整张证书，签名是 sig（DER 的 ECDSA 签名）
        pub fn cert(&self, sig: &[u8]) -> Vec<u8> {
            let alg = der(0x30, &[&der(0x06, &[&[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02]])]);
            der(0x30, &[&self.tbs(), &alg, &der(0x03, &[&[0], sig])])
        }
    }

    /// subjectAltName 这个扩展
    pub fn san_extension(names: &[San]) -> Vec<u8> {
        let items: Vec<Vec<u8>> = names.iter().map(general_name).collect();
        let all: Vec<&[u8]> = items.iter().map(Vec::as_slice).collect();
        der(0x30, &[&der(0x06, &[&[0x55, 0x1d, 0x11]]), &der(0x04, &[&der(0x30, &all)])])
    }

    /// basicConstraints（critical，不是 CA）
    pub fn not_a_ca() -> Vec<u8> {
        der(0x30, &[&der(0x06, &[&[0x55, 0x1d, 0x13]]), &der(0x01, &[&[0xff]]), &der(0x04, &[&der(0x30, &[])])])
    }

    /// 现造一把 P-256 的私钥（PKCS#8）
    pub fn new_key() -> Vec<u8> {
        EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &ring::rand::SystemRandom::new()).unwrap().as_ref().to_vec()
    }

    /// 这把私钥的公钥那个点（65 个字节）
    pub fn point_of(pkcs8: &[u8]) -> Vec<u8> {
        let rng = ring::rand::SystemRandom::new();
        EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8, &rng).unwrap().public_key().as_ref().to_vec()
    }

    /// 拿这把私钥自签一张证书。names 是 None：v1 的、没有扩展（安装脚本在 macOS 上用 LibreSSL 自签出来的那种）；
    /// 给了：v3 的，不是 CA，subjectAltName 是这几个地址（证书机构签的那种样子）
    pub fn self_signed(pkcs8: &[u8], names: Option<&[San]>) -> Vec<u8> {
        let rng = ring::rand::SystemRandom::new();
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8, &rng).unwrap();
        let mut shape = Shape::new();
        shape.spki = [&P256_SPKI_HEAD[..], key.public_key().as_ref()].concat();
        shape.not_before = time("250101000000Z");
        shape.not_after = time("20991231235959Z");
        match names {
            None => shape.v3 = false,
            Some(names) => shape.extensions = vec![not_a_ca(), san_extension(names)],
        }
        let sig = key.sign(&rng, &shape.tbs()).unwrap();
        shape.cert(sig.as_ref())
    }

    /// DER → PEM（64 个字符一行）
    pub fn pem(label: &str, der: &[u8]) -> String {
        let b64 = crate::py::base64::b64encode(der);
        let lines: Vec<&str> = b64.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap()).collect();
        format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::testkit::{der, san_extension, time, Shape};
    use super::*;
    use crate::py::hash::{from_hex, to_hex};

    // 三张真的证书（一次性的密钥，只有证书在这里）。期望的值是 openssl 给的：
    // openssl x509 -noout -startdate -enddate -ext subjectAltName、openssl x509 -pubkey -noout | openssl pkey -pubin -outform DER

    /// OpenSSL 3.6 `req -new -x509` 自签的（Team 的服务器在 Linux 上的那一种）：v3，没有 subjectAltName
    const OPENSSL_SELF_SIGNED: &str = "-----BEGIN CERTIFICATE-----
MIIBejCCASGgAwIBAgIUDwszD6BRYnV6mBiXaIjLPePwe5wwCgYIKoZIzj0EAwIw
EzERMA8GA1UEAwwIY2NhLXRlYW0wHhcNMjYwMTAxMDAwMDAwWhcNMzYwMTAxMDAw
MDAwWjATMREwDwYDVQQDDAhjY2EtdGVhbTBZMBMGByqGSM49AgEGCCqGSM49AwEH
A0IABEhdWB/ClAev+OIlZibJVAJ8PktoPVcCaHZCkfZg1C99bo3axIrM2GIAXsRF
5y3vxZ6uLiTtr37uCjpIdKk7SXqjUzBRMB0GA1UdDgQWBBSX7AHHi3smzbILG76P
WQsZGIis7jAfBgNVHSMEGDAWgBSX7AHHi3smzbILG76PWQsZGIis7jAPBgNVHRMB
Af8EBTADAQH/MAoGCCqGSM49BAMCA0cAMEQCIBGfy0JUcqZt9yaLsQ8laLStOf4t
iF25kQrfU3WsF7EJAiBouWUqE+gqBgCNWGgi3WkbYluGI7dNqWODzfZXt7ttKg==
-----END CERTIFICATE-----
";
    /// 带 subjectAltName 的：两个 IP（v4、v6）、两个域名（一个带大写）；到期时间是 GeneralizedTime（2051 年）
    const WITH_SAN: &str = "-----BEGIN CERTIFICATE-----
MIIBxDCCAWugAwIBAgIUTURk1CdMNpGRECaDfpYGb/M2eXAwCgYIKoZIzj0EAwIw
FTETMBEGA1UEAwwKMTkyLjAuMi4xMDAgFw0yNjAxMDIwMzA0MDVaGA8yMDUxMDYw
NzA4MDkxMFowFTETMBEGA1UEAwwKMTkyLjAuMi4xMDBZMBMGByqGSM49AgEGCCqG
SM49AwEHA0IABEXYMShaRxvd1Ja2kFOJdWSB75DNVbZIZB27UOQ9GfJuj4Row+Hf
yUksioRRyAG/l0nVKNq8wcqWYVcdqO0jG1OjgZYwgZMwHQYDVR0OBBYEFHBsnMjn
zNcInnfLS9CbIlkKL9YiMB8GA1UdIwQYMBaAFHBsnMjnzNcInnfLS9CbIlkKL9Yi
MA8GA1UdEwEB/wQFMAMBAf8wQAYDVR0RBDkwN4cEwAACCocQIAENuAAAAAAAAAAA
AAAAAYIQVGVhbS5FeGFtcGxlLkNPTYILZXhhbXBsZS5jb20wCgYIKoZIzj0EAwID
RwAwRAIgW+KUKk4OaQ0k2XBXHPdmDbcmcczUETLDFa8eH5Em9pUCID5hNkd9i/hM
R2REXrTvSdIea1Zd/wWfrnH7Hg9geWak
-----END CERTIFICATE-----
";
    /// macOS 自带的 LibreSSL 3.3.6 `req -new -x509` 自签的：v1（没有版本那一项、没有扩展），序列号前面带一个 0
    const LIBRESSL_V1: &str = "-----BEGIN CERTIFICATE-----
MIIBETCBugIJALXdOIrlOrNlMAoGCCqGSM49BAMCMBIxEDAOBgNVBAMMB2NjYS1o
dWIwHhcNMjYxMTI0MDc1NTA4WhcNMzYxMTIxMDc1NTA4WjASMRAwDgYDVQQDDAdj
Y2EtaHViMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAE8qauKdhYN7sMs2lPFiu8
aG3+2YTOuqS/8J9Xjpj/9ftnOL+IkGF3mwzZlgW6oeJOFvGuGoFrtPvSv5G+1uR+
YTAKBggqhkjOPQQDAgNGADBDAiBm9BYzIACyoNc2Tajb5kqBuyjqycE37N0pkL7Y
/sSVTQIfOGh7+iiNqxDJmeDn0tIYNOFw1g+XYtK1e5RCeS9K/w==
-----END CERTIFICATE-----
";

    fn names(items: &[San]) -> BTreeSet<San> {
        items.iter().cloned().collect()
    }

    fn ip(s: &str) -> San {
        San::Ip(s.into())
    }

    fn dns(s: &str) -> San {
        San::Dns(s.into())
    }

    #[test]
    fn reads_real_certificates_like_openssl() {
        let c = cert_info_pem(OPENSSL_SELF_SIGNED.as_bytes()).unwrap();
        assert_eq!((c.not_before, c.not_after), (1767225600, 2082758400), "2026-01-01、2036-01-01");
        assert!(c.san.is_empty());
        assert_eq!(
            to_hex(&c.spki),
            "3059301306072a8648ce3d020106082a8648ce3d03010703420004485d581fc29407aff8e2256626c954027c3e4b683d570268764291f660d42f7d6e\
             8ddac48accd862005ec445e72defc59eae2e24edaf7eee0a3a4874a93b497a"
        );

        let c = cert_info_pem(WITH_SAN.as_bytes()).unwrap();
        assert_eq!((c.not_before, c.not_after), (1767323045, 2569738150), "2026-01-02 03:04:05、2051-06-07 08:09:10");
        // openssl 打出来的是 IP Address:2001:DB8:0:0:0:0:0:1、DNS:Team.Example.COM：这里和 Python 一样写成规范的样子
        assert_eq!(c.san, names(&[ip("192.0.2.10"), ip("2001:db8::1"), dns("team.example.com"), dns("example.com")]));
        assert_eq!(
            to_hex(&c.spki),
            "3059301306072a8648ce3d020106082a8648ce3d0301070342000445d831285a471bddd496b6905389756481ef90cd55b648641dbb50e43d19f26e\
             8f8468c3e1dfc9492c8a8451c801bf9749d528dabcc1ca9661571da8ed231b53"
        );

        let c = cert_info_pem(LIBRESSL_V1.as_bytes()).unwrap();
        assert_eq!((c.not_before, c.not_after), (1795506908, 2110866908), "2026-11-24 07:55:08、2036-11-21 07:55:08");
        assert!(c.san.is_empty());
        assert_eq!(
            to_hex(&c.spki),
            "3059301306072a8648ce3d020106082a8648ce3d03010703420004f2a6ae29d85837bb0cb3694f162bbc686dfed984cebaa4bff09f578e98fff5fb\
             6738bf889061779b0cd99605baa1e24e16f1ae1a816bb4fbd2bf91bed6e47e61"
        );
        // 只要公钥的那个读法给的是同一段
        for pem in [OPENSSL_SELF_SIGNED, WITH_SAN, LIBRESSL_V1] {
            let der = crate::py::tls::pem_certificates(pem).remove(0);
            assert_eq!(cert_spki(&der), cert_info(&der).map(|c| c.spki));
        }
    }

    /// 文件里有几张证书时看头一张；没有证书、不是 PEM 是 None
    #[test]
    fn pem_uses_the_first_certificate() {
        let both = format!("说明文字\n{WITH_SAN}{OPENSSL_SELF_SIGNED}");
        assert_eq!(cert_info_pem(both.as_bytes()), cert_info_pem(WITH_SAN.as_bytes()));
        assert!(
            cert_info_pem(b"").is_none()
                && cert_info_pem(b"-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n").is_none()
        );
        assert!(cert_info_pem(b"-----BEGIN PRIVATE KEY-----\nAQID\n-----END PRIVATE KEY-----\n").is_none());
        assert!(cert_info_pem(&[0xff, 0xfe, 0x00]).is_none());
    }

    /// 截短的、改坏的证书：一律 None，不 panic（每个长度、每个字节都试一遍）
    #[test]
    fn damaged_certificates_are_none_not_a_panic() {
        for pem in [OPENSSL_SELF_SIGNED, WITH_SAN, LIBRESSL_V1] {
            let der = crate::py::tls::pem_certificates(pem).remove(0);
            for n in 0..der.len() {
                assert!(cert_info(&der[..n]).is_none() && cert_spki(&der[..n]).is_none(), "截到 {n} 个字节");
            }
            let mut longer = der.clone();
            longer.push(0);
            assert!(cert_info(&longer).is_none(), "后面多一个字节");
            for i in 0..der.len() {
                for flip in [0x01u8, 0x80, 0xff] {
                    let mut bad = der.clone();
                    bad[i] ^= flip;
                    let _ = cert_info(&bad); // 结果是什么不管：不 panic 就行
                    let _ = cert_spki(&bad);
                }
            }
        }
        for junk in [
            &b""[..],
            &[0x30],
            &[0x30, 0x80],
            &[0x30, 0x84, 0xff, 0xff, 0xff, 0xff],
            &[0x30, 0x85, 1, 0, 0, 0, 0],
            &[0x1f, 0x81, 0x00],
        ] {
            assert!(cert_info(junk).is_none() && cert_spki(junk).is_none());
        }
    }

    /// 长度的写法：只认 DER 的（最短的）
    #[test]
    fn lengths_must_be_minimal_der() {
        let read = |bytes: &[u8]| item(&mut Cur::new(bytes)).map(|i| (i.tag, i.body.len(), i.whole.len()));
        assert_eq!(read(&[0x04, 0x02, 9, 9, 7]), Some((0x04, 2, 4)), "后面多的留给下一项");
        assert_eq!(read(&[0x04, 0x00]), Some((0x04, 0, 2)));
        let long = [&[0x04, 0x81, 0x80][..], &[0u8; 0x80]].concat();
        assert_eq!(read(&long), Some((0x04, 0x80, 0x83)));
        let longer = [&[0x04, 0x82, 0x01, 0x00][..], &[0u8; 0x100]].concat();
        assert_eq!(read(&longer), Some((0x04, 0x100, 0x104)));
        assert_eq!(read(&[0x04, 0x81, 0x7f]), None, "不到 128 却用了长的写法");
        assert_eq!(read(&[&[0x04, 0x82, 0x00, 0x80][..], &[0u8; 0x80]].concat()), None, "前面多一个 0");
        assert_eq!(read(&[0x04, 0x80, 0, 0]), None, "不定长的");
        assert_eq!(read(&[0x04, 0x03, 1, 2]), None, "内容不够");
        assert_eq!(read(&[0x04]), None);
        assert_eq!(read(&[0x1f, 0x01, 0x00]), None, "多字节的标签");
        assert_eq!(read(&[0x04, 0x85, 0, 0, 0, 0, 1, 7]), None, "长度写了 5 个字节");
    }

    /// 私钥那一段短了的 SEC1 补得回来（几种曲线、短几个字节都行）；够长的、没带公钥的、看不懂的不动
    #[test]
    fn short_sec1_scalars_are_padded() {
        let d = testkit::der;
        let build = |scalar: &[u8], tail: &[u8]| d(SEQUENCE, &[&d(INTEGER, &[&[1]]), &d(OCTET_STRING, &[scalar]), tail]);
        let public = |size: usize| d(EC_PUBLIC, &[&d(BIT_STRING, &[&[0, 4], &vec![7u8; 2 * size]])]);
        let curve = d(0xa0, &[&d(OID, &[&[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07]])]);
        for size in [32usize, 48, 66] {
            let scalar: Vec<u8> = (0..size).map(|i| if i < 2 { 0 } else { i as u8 }).collect();
            let tail = [curve.clone(), public(size)].concat();
            let full = build(&scalar, &tail);
            assert_eq!(sec1_padded(&full), None, "{size}：够长的不动");
            assert_eq!(sec1_padded(&build(&scalar[1..], &tail)), Some(full.clone()), "{size}：少一个字节");
            assert_eq!(sec1_padded(&build(&scalar[2..], &tail)), Some(full.clone()), "{size}：少两个字节");
            assert_eq!(sec1_padded(&build(&scalar[1..], &public(size))), Some(build(&scalar, &public(size))), "不带曲线的参数");
            assert_eq!(sec1_padded(&build(&scalar[1..], &curve)), None, "{size}：没带公钥，不知道该多长");
            assert_eq!(sec1_padded(&build(&[&[9u8][..], &scalar].concat(), &tail)), None, "{size}：比曲线还长的不管");
        }
        // P-256 的那一份就是 openssl ecparam -genkey -noout 写出来的大小
        assert_eq!(build(&[1u8; 32], &[curve.clone(), public(32)].concat()).len(), 121);
        let compressed = d(EC_PUBLIC, &[&d(BIT_STRING, &[&[0, 2], &[7u8; 32]])]);
        assert_eq!(sec1_padded(&build(&[1u8; 31], &compressed)), None, "压缩写法的公钥：不猜");
        let huge = build(&[1u8; 31], &public(100));
        assert_eq!(sec1_padded(&huge), None, "没有这么长的曲线");
        let short = build(&[1u8; 31], &[curve, public(32)].concat());
        for junk in [&b""[..], b"\x30\x00", &short[..50], b"not der at all", &[&short[..], b"x"].concat()[1..]] {
            assert_eq!(sec1_padded(junk), None, "{junk:?}");
        }
    }

    /// 时间的两种写法、世纪怎么定；不合规矩的不认
    #[test]
    fn times() {
        let at = |t: &str| {
            let mut shape = Shape::new();
            shape.not_before = time(t);
            cert_info(&shape.cert(&[0x30, 0x00])).map(|c| c.not_before)
        };
        assert_eq!(at("500101000000Z"), Some(-631152000), "UTCTime 的 50 是 1950");
        assert_eq!(at("491231235959Z"), Some(2524607999), "49 是 2049");
        assert_eq!(at("700101000000Z"), Some(0));
        assert_eq!(at("20000229120000Z"), Some(951825600), "2000 年是闰年");
        assert_eq!(at("99991231235959Z"), Some(253402300799), "“没有到期时间”的那个写法");
        assert_eq!(at("240229000000Z"), Some(1709164800));
        for bad in [
            "230229000000Z",     // 2023 年没有 2 月 29 日
            "21000229000000Z",   // 2100 年不是闰年
            "260100000000Z",     // 0 日
            "261301000000Z",     // 13 月
            "260431000000Z",     // 4 月 31 日
            "260101240000Z",     // 24 点
            "260101006000Z",     // 60 分
            "260101000060Z",     // 60 秒
            "2601010000Z",       // 不带秒
            "260101000000",      // 不带 Z
            "260101000000+0800", // 带时区
            "20260101000000.5Z", // 带小数
            "26010100000aZ",
            "",
            "Z",
        ] {
            assert_eq!(at(bad), None, "{bad}");
        }
        // 标签不是时间的
        let mut shape = Shape::new();
        shape.not_after = der(0x0c, &[b"20510607080910Z"]);
        assert!(cert_info(&shape.cert(&[0x30, 0x00])).is_none());
        assert!(cert_spki(&shape.cert(&[0x30, 0x00])).is_some(), "只要公钥时不看有效期写得对不对");
    }

    /// subjectAltName：IP 写成 Python 的样子，域名转小写，别的种类不算；写坏了的整张不认
    #[test]
    fn subject_alt_names() {
        let with = |exts: Vec<Vec<u8>>| {
            let mut shape = Shape::new();
            shape.extensions = exts;
            cert_info(&shape.cert(&[0x30, 0x00])).map(|c| c.san)
        };
        let general = |tag: u8, body: &[u8]| der(tag, &[body]);
        let san_of = |items: &[Vec<u8>]| {
            let all: Vec<&[u8]> = items.iter().map(Vec::as_slice).collect();
            der(0x30, &[&der(0x06, &[&[0x55, 0x1d, 0x11]]), &der(0x04, &[&der(0x30, &all)])])
        };
        assert_eq!(with(vec![]), Some(names(&[])), "v3 但没有扩展");
        assert_eq!(
            with(vec![san_extension(&[
                ip("198.51.100.7"),
                ip("2001:db8:0:0:0:0:0:1"),
                ip("::ffff:192.0.2.1"),
                dns("A.Example.COM")
            ])]),
            Some(names(&[ip("198.51.100.7"), ip("2001:db8::1"), ip("::ffff:192.0.2.1"), dns("a.example.com")]))
        );
        // critical 的也认；别的扩展、别的种类的名字（邮箱 [1]、URI [6]）跳过；重复的只算一个
        let critical = der(
            0x30,
            &[
                &der(0x06, &[&[0x55, 0x1d, 0x11]]),
                &der(0x01, &[&[0xff]]),
                &der(
                    0x04,
                    &[&der(
                        0x30,
                        &[
                            &general(0x81, b"a@example.com"),
                            &general(0x82, b"example.com"),
                            &general(0x86, b"https://example.com/"),
                        ],
                    )],
                ),
            ],
        );
        let other = der(0x30, &[&der(0x06, &[&[0x55, 0x1d, 0x0e]]), &der(0x04, &[&[0x04, 0x01, 0x07]])]);
        assert_eq!(
            with(vec![other.clone(), critical, san_of(&[general(0x82, b"EXAMPLE.com")])]),
            Some(names(&[dns("example.com")]))
        );
        // IP 地址不是 4 个、16 个字节（openssl 打成 <invalid>）；域名是空的、带逗号、带空白、不是 ASCII
        for bad in [
            general(0x87, &[192, 0, 2]),
            general(0x87, &[192, 0, 2, 0, 255, 255, 255, 0]),
            general(0x87, &[]),
            general(0x82, b""),
            general(0x82, b"a.example.com,b.example.com"),
            general(0x82, b"a example.com"),
            general(0x82, "例子.example.com".as_bytes()),
        ] {
            assert_eq!(with(vec![san_of(std::slice::from_ref(&bad))]), None, "{bad:02x?}");
        }
        // 扩展本身写坏了：值不是 OCTET STRING、后面多东西、里面不是 SEQUENCE
        let oid = der(0x06, &[&[0x55, 0x1d, 0x11]]);
        assert_eq!(with(vec![der(0x30, &[&oid, &der(0x0c, &[b"x"])])]), None);
        assert_eq!(with(vec![der(0x30, &[&oid, &der(0x04, &[&der(0x30, &[])]), &[0x05, 0x00]])]), None);
        assert_eq!(with(vec![der(0x30, &[&oid, &der(0x04, &[&der(0x31, &[])])])]), None);
        assert_eq!(with(vec![der(0x30, &[&oid, &der(0x04, &[&der(0x30, &[]), &[0x05, 0x00]])])]), None);
        assert_eq!(with(vec![der(0x30, &[&oid, &der(0x04, &[&der(0x30, &[])])])]), Some(names(&[])), "空的 subjectAltName");
    }

    /// v1（没有版本、没有扩展）和 v3 的都读得了；公钥是原样的那一段
    #[test]
    fn v1_and_v3_shapes() {
        let mut shape = Shape::new();
        shape.spki =
            from_hex("3059301306072a8648ce3d020106082a8648ce3d030107034200").unwrap().into_iter().chain([7u8; 65]).collect();
        let v3 = cert_info(&shape.cert(&[0x30, 0x00])).unwrap();
        shape.v3 = false;
        let v1 = cert_info(&shape.cert(&[0x30, 0x00])).unwrap();
        assert_eq!(v1, v3);
        assert_eq!((v1.spki.len(), &v1.spki[..2], &v1.spki[26..]), (91, &[0x30, 0x59][..], &[7u8; 65][..]));
        assert_eq!((San::Ip("192.0.2.1".into()).kind(), San::Dns("example.com".into()).kind()), ("ip", "dns"));
        assert_eq!((San::Ip("192.0.2.1".into()).value(), San::Dns("example.com".into()).value()), ("192.0.2.1", "example.com"));
        // 排序：IP 在域名前面（集合里的先后只用来比，不写出去）
        assert!(ip("203.0.113.9") < dns("a.example.com"));
    }
}
