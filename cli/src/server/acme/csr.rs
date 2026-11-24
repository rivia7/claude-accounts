//! 证书请求（PKCS#10，RFC 2986）。Python 那一份是 `openssl req -new -key … -subj / -config …` 做的（Acme:350-355），
//! 这里自己拼 DER，拼出来要签名的那一段和 openssl 的一个字节不差（测试里对着它做的那几份比）：
//!
//! ```text
//! CertificationRequest ::= SEQUENCE {
//!   certificationRequestInfo SEQUENCE {
//!     version       INTEGER 0,
//!     subject       SEQUENCE {}                -- 空的：证书里不要 Common Name（给 IP 签的不许有），地址只写在下面
//!     subjectPKInfo SubjectPublicKeyInfo,
//!     attributes    [0] { SEQUENCE { OID extensionRequest, SET { SEQUENCE {     -- 要证书带上的扩展
//!                         SEQUENCE { OID subjectAltName, OCTET STRING { SEQUENCE { 各个地址 } } } } } } } },
//!   signatureAlgorithm SEQUENCE { OID ecdsa-with-SHA256 },
//!   signature          BIT STRING }
//! ```
//!
//! subjectAltName 没标 critical（openssl 做出来的就没标）；地址的先后照给的顺序。

use super::der::{der, BIT_STRING, INTEGER, OCTET_STRING, OID, SEQUENCE, SET};
use super::key::Key;
use super::Problem;
use crate::py::ipaddr::{ip_address, Ip};
use crate::server::x509::San;

/// extensionRequest：1.2.840.113549.1.9.14
const EXTENSION_REQUEST: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x0e];
/// subjectAltName：2.5.29.17
const SUBJECT_ALT_NAME: &[u8] = &[0x55, 0x1d, 0x11];
/// ecdsa-with-SHA256：1.2.840.10045.4.3.2
const ECDSA_WITH_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
/// certificationRequestInfo 里的 [0]（attributes）
const ATTRIBUTES: u8 = 0xa0;
/// GeneralName 里的 [2] dNSName、[7] iPAddress
const DNS_NAME: u8 = 0x82;
const IP_ADDRESS: u8 = 0x87;

/// subjectAltName 里的一项：域名原样的字节，IP 地址是 4 个或 16 个字节
fn general_name(id: &San) -> Result<Vec<u8>, Problem> {
    match id {
        San::Dns(name) => Ok(der(DNS_NAME, &[name.as_bytes()])),
        San::Ip(text) => match ip_address(text) {
            Ok(parsed) if parsed.scope.is_none() => Ok(match parsed.ip {
                Ip::V4(ip) => der(IP_ADDRESS, &[&ip.octets()]),
                Ip::V6(ip) => der(IP_ADDRESS, &[&ip.octets()]),
            }),
            // 带区域标识的（fe80::1%eth0：只有开着 --any-ip 才到得了这里）证书里写不了。Python 那边是 openssl req 没成
            _ => Err(Problem(format!("证书请求做不出来（{text} 写不成证书里的地址）"))),
        },
    }
}

/// 要签名的那一段（certificationRequestInfo）：公钥是 spki 的、要的地址是 ids
pub fn request_info(spki: &[u8], ids: &[San]) -> Result<Vec<u8>, Problem> {
    let mut names = Vec::new();
    for id in ids {
        names.extend(general_name(id)?);
    }
    let san = der(SEQUENCE, &[&der(OID, &[SUBJECT_ALT_NAME]), &der(OCTET_STRING, &[&der(SEQUENCE, &[&names])])]);
    let attribute = der(SEQUENCE, &[&der(OID, &[EXTENSION_REQUEST]), &der(SET, &[&der(SEQUENCE, &[&san])])]);
    Ok(der(SEQUENCE, &[&der(INTEGER, &[&[0]]), &der(SEQUENCE, &[]), spki, &der(ATTRIBUTES, &[&attribute])]))
}

/// 证书请求（DER）：给 ids 这几个地址、拿 key 这把私钥签的
pub fn csr(key: &Key, ids: &[San]) -> Result<Vec<u8>, Problem> {
    let info = request_info(&key.spki(), ids)?;
    let signature = key.sign_der(&info)?;
    Ok(der(SEQUENCE, &[&info, &der(SEQUENCE, &[&der(OID, &[ECDSA_WITH_SHA256])]), &der(BIT_STRING, &[&[0], &signature])]))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::py::fs::TempDir;
    use crate::py::hash::{from_hex, to_hex};
    use crate::server::acme::key::tests::{fixture, fixture_key};
    use crate::server::acme::key::P256_SPKI_HEAD;
    use ring::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_ASN1};

    fn ids_of(c: &crate::py::obj::Dict) -> Vec<San> {
        let pairs = c.list("ids").unwrap().items();
        pairs
            .iter()
            .map(|p| {
                let p = p.as_list().unwrap();
                let (kind, value) = (p.at(0).as_str().unwrap().to_string(), p.at(1).as_str().unwrap().to_string());
                if kind == "ip" {
                    San::Ip(value)
                } else {
                    San::Dns(value)
                }
            })
            .collect()
    }

    /// 对照数据：同一把私钥、同样的地址，`openssl req` 照 Python 那一份的办法做出来的证书请求。要签名的那一段一个字节不差
    /// （IPv4、IPv6、域名、混着的、长到长度要多写一个字节的），整个请求的签名验得过
    #[test]
    fn same_request_info_as_openssl() {
        let d = TempDir::new().unwrap();
        let doc = fixture();
        let cases = doc.list("csr").unwrap().items();
        let mut names = Vec::new();
        for c in cases {
            let c = c.as_dict().unwrap();
            let name = c.str("name").unwrap().to_string();
            let key = fixture_key(&d.path, &c.str("key").unwrap());
            let ids = ids_of(&c);
            let want = from_hex(&c.str("info").unwrap()).unwrap();
            assert_eq!(to_hex(&request_info(&key.spki(), &ids).unwrap()), to_hex(&want), "{name}");
            // 整个请求：SEQUENCE { 那一段, 算法, 签名 }；长度和 openssl 的差不出签名长短那一两个字节
            let whole = csr(&key, &ids).unwrap();
            let at = whole.windows(want.len()).position(|w| w == want).unwrap();
            assert!(whole[0] == 0x30 && at <= 4, "{name}");
            let rest = &whole[at + want.len()..];
            assert_eq!(rest[..12], [0x30, 0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02], "{name}");
            assert_eq!((rest[12], rest[13] as usize, rest[14]), (0x03, rest.len() - 14, 0x00), "{name}");
            let point = &key.spki()[P256_SPKI_HEAD.len()..];
            UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, point).verify(&want, &rest[15..]).unwrap();
            let len = c.get("len").unwrap().as_int().unwrap();
            assert!((whole.len() as i64 - len).abs() <= 2, "{name}: {} / {len}", whole.len());
            names.push(name);
        }
        names.sort();
        assert_eq!(names, ["dns", "ipv4", "ipv6", "long", "many", "mixed", "two-ips"]);
    }

    /// 里面各项的样子：空的 subject、没标 critical 的 subjectAltName、地址照给的先后
    #[test]
    fn shape_of_the_request() {
        let spki = [&P256_SPKI_HEAD[..], &[4u8; 65]].concat();
        let info = request_info(&spki, &[San::Ip("192.0.2.10".into())]).unwrap();
        let want =
            format!("3081840201003000{}a022302006092a864886f70d01090e31133011300f0603551d11040830068704c000020a", to_hex(&spki));
        assert_eq!(to_hex(&info), want);
        let san = |ids: &[San]| {
            let info = to_hex(&request_info(&spki, ids).unwrap());
            info[info.find("0603551d1104").unwrap() + 12..].to_string()
        };
        assert_eq!(san(&[San::Dns("a.example.com".into())]), "11300f820d612e6578616d706c652e636f6d");
        assert_eq!(san(&[San::Ip("2001:db8::1".into())]), "143012871020010db8000000000000000000000001");
        assert_eq!(san(&[San::Ip("::ffff:192.0.2.1".into())]), "143012871000000000000000000000ffffc0000201");
        let (a, b) = (San::Ip("192.0.2.1".into()), San::Dns("b.example.com".into()));
        assert_ne!(san(&[a.clone(), b.clone()]), san(&[b, a]), "先后照给的，不排序");
        assert_eq!(san(&[]), "023000", "一个地址都没有（用不到）：空的列表");
        // 写不进证书的地址
        let why = request_info(&spki, &[San::Ip("fe80::1%eth0".into())]).unwrap_err();
        assert_eq!(why.0, "证书请求做不出来（fe80::1%eth0 写不成证书里的地址）");
        assert!(request_info(&spki, &[San::Ip("not an ip".into())]).is_err());
    }
}
