//! P-256 的私钥（账户的、证书的）：一个 PEM 文件，只有自己读得了。Python 的 `Key`（Acme:131-179）。
//!
//! Python 那一份靠 openssl 命令生成（`genpkey`）、取公钥（`pkey -pubout`）、签名（`dgst -sign`）；这里用 ring，不起外部命令。
//! 续证书时账户的私钥、证书的私钥都不换，所以以前 openssl 写的文件必须照样读得了。认两种写法：
//! - PKCS#8（`BEGIN PRIVATE KEY`）：Python 那一份 `openssl genpkey` 出来的、这里生成的都是
//! - SEC1（`BEGIN EC PRIVATE KEY`）：`openssl ecparam -genkey` 出来的（安装脚本自签的那张证书的私钥），前面带一段
//!   `EC PARAMETERS` 的也行
//!
//! 和 Python 不一样的地方（都是 ring 的规矩）：私钥里要带着公钥（openssl、这里写的都带）；曲线要写成名字（`prime256v1`），把曲线的
//! 参数整个写出来的那种不认（LibreSSL 的 `genpkey` 默认这么写；它的 `ecparam -genkey` 不是）。读不了时说的话也不一样：Python 说的是
//! openssl 的哪一步没成（“openssl pkey 没成”），这里没有 openssl，说的是文件不对。

use super::der::{der, INTEGER, OCTET_STRING, OID, SEQUENCE};
use super::files::{put_file, read_capped, strerror, READ_MAX};
use super::{Problem, Trouble};
use crate::dict;
use crate::py::obj::Dict;
use crate::py::{base64, hash, path};
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING, ECDSA_P256_SHA256_FIXED_SIGNING};

/// P-256 的公钥在 SubjectPublicKeyInfo 里那 65 个字节（04 X Y）前面的部分：算法是 id-ecPublicKey、曲线是 prime256v1
pub const P256_SPKI_HEAD: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03,
    0x01, 0x07, 0x03, 0x42, 0x00,
];
/// id-ecPublicKey：1.2.840.10045.2.1
const EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
/// prime256v1（P-256）：1.2.840.10045.3.1.7
const PRIME256V1: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];

/// `base64.urlsafe_b64encode(data).rstrip(b'=')`（Acme:69-70）：JWS 里的各段都是这么写的
pub fn b64u(data: &[u8]) -> String {
    base64::urlsafe_b64encode(data).trim_end_matches('=').to_string()
}

/// DER → PEM：64 个字符一行（openssl 写出来的样子）
pub fn pem(label: &str, der: &[u8]) -> String {
    let mut out = format!("-----BEGIN {label}-----\n");
    for line in base64::b64encode(der).as_bytes().chunks(64) {
        out.push_str(&String::from_utf8_lossy(line));
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

/// PEM 文件里头一把私钥，写成 PKCS#8 的 DER。两种标签里哪个在前用哪个（文件里一般只有一把）；
/// 加了密的（`ENCRYPTED PRIVATE KEY`）、RSA 的（`RSA PRIVATE KEY`）标签不一样，找不到
fn pkcs8_of(text: &str) -> Option<Vec<u8>> {
    let mut first: Option<(usize, bool, &str)> = None;
    for (label, sec1) in [("PRIVATE KEY", false), ("EC PRIVATE KEY", true)] {
        let (begin, end) = (format!("-----BEGIN {label}-----"), format!("-----END {label}-----"));
        let Some(at) = text.find(&begin) else { continue };
        let Some(body) = text.get(at.saturating_add(begin.len())..) else { continue };
        let Some(stop) = body.find(&end) else { continue };
        if first.is_none_or(|(before, _, _)| at < before) {
            first = Some((at, sec1, body.get(..stop)?));
        }
    }
    let (_, sec1, body) = first?;
    let der_bytes = base64::a2b_base64(body.as_bytes(), false).ok()?;
    // LibreSSL 写的 SEC1 偶尔少了私钥开头的 0（x509::sec1_padded）：补上再包
    Some(if sec1 { wrap_sec1(&crate::server::x509::sec1_padded(&der_bytes).unwrap_or(der_bytes)) } else { der_bytes })
}

/// SEC1 的私钥（RFC 5915 的 ECPrivateKey）包成 PKCS#8（ring 只读这一种）：前面加上“这是 P-256 的 EC 私钥”。
/// 里面要不是 P-256 的（写着别的曲线、私钥长度不对），ring 读的时候不收
fn wrap_sec1(sec1: &[u8]) -> Vec<u8> {
    der(
        SEQUENCE,
        &[
            &der(INTEGER, &[&[0]]),
            &der(SEQUENCE, &[&der(OID, &[EC_PUBLIC_KEY]), &der(OID, &[PRIME256V1])]),
            &der(OCTET_STRING, &[sec1]),
        ],
    )
}

/// 一把 P-256 的私钥
pub struct Key {
    /// 文件在哪（只有测试看：拿它拼旁边的路径）
    #[cfg(test)]
    pub path: String,
    /// 公钥的那个点：04 X Y，65 个字节
    point: Vec<u8>,
    /// 签 JWS 用：签名是 r、s 各 32 个字节接在一起（ES256）
    jws: EcdsaKeyPair,
    /// 签证书请求用：签名是 DER 的 SEQUENCE { r, s }
    asn1: EcdsaKeyPair,
    rng: SystemRandom,
}

impl Key {
    /// 读这个文件里的私钥；没有就生成（make 是 false 时不生成：没有就是 Problem）（Acme:133-152）。
    /// 生成的写成 PKCS#8 的 PEM，只有自己读得了（600）
    pub fn open(path: &str, make: bool) -> Result<Key, Trouble> {
        if !make && !path::isfile(path) {
            return Err(Problem(format!("没有 {path}")).into());
        }
        let rng = SystemRandom::new();
        if !path::exists(path) {
            let made = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
                .map_err(|_| Problem("私钥生成不了（系统给不出随机数）".to_string()))?;
            put_file(path, pem("PRIVATE KEY", made.as_ref()).as_bytes(), 0o600)?;
        }
        let raw = read_capped(path, READ_MAX).map_err(|e| Problem(format!("读不了 {path}（{}）", strerror(&e))))?;
        let wrong = || Trouble::from(Problem(format!("{path} 不是 P-256 的私钥")));
        let pkcs8 = pkcs8_of(&String::from_utf8_lossy(&raw)).ok_or_else(wrong)?;
        let jws = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &pkcs8, &rng).map_err(|_| wrong())?;
        let asn1 = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &pkcs8, &rng).map_err(|_| wrong())?;
        let point = jws.public_key().as_ref().to_vec();
        if point.len() != 65 || point.first() != Some(&4) {
            return Err(wrong());
        }
        Ok(Key {
            #[cfg(test)]
            path: path.to_string(),
            point,
            jws,
            asn1,
            rng,
        })
    }

    fn x(&self) -> &[u8] {
        self.point.get(1..33).unwrap_or(&[])
    }

    fn y(&self) -> &[u8] {
        self.point.get(33..65).unwrap_or(&[])
    }

    /// 公钥：SubjectPublicKeyInfo 的 DER（`openssl pkey -pubout -outform DER` 给的就是它）。证书是不是这把私钥的，比的是它
    pub fn spki(&self) -> Vec<u8> {
        [&P256_SPKI_HEAD[..], &self.point].concat()
    }

    /// 公钥写成 JWK（Acme:154-155）；各项的先后也照 Python 的
    pub fn jwk(&self) -> Dict {
        dict! { "crv" => "P-256", "kty" => "EC", "x" => b64u(self.x()), "y" => b64u(self.y()) }
    }

    /// RFC 7638 的摘要：这几项按键名排好、不带空白的 JSON 的 SHA-256（Acme:157-159）。http-01 回的内容里有它
    pub fn thumbprint(&self) -> String {
        let canonical = format!(r#"{{"crv":"P-256","kty":"EC","x":"{}","y":"{}"}}"#, b64u(self.x()), b64u(self.y()));
        b64u(&hash::sha256(canonical.as_bytes()))
    }

    /// ES256：r、s 各 32 个字节接在一起（Acme:161-179）
    pub fn sign(&self, data: &[u8]) -> Result<Vec<u8>, Problem> {
        self.jws.sign(&self.rng, data).map(|s| s.as_ref().to_vec()).map_err(|_| Problem("签不了名（系统给不出随机数）".to_string()))
    }

    /// 同一种签名写成 DER（证书请求里的那一段要这种）
    pub fn sign_der(&self, data: &[u8]) -> Result<Vec<u8>, Problem> {
        self.asn1
            .sign(&self.rng, data)
            .map(|s| s.as_ref().to_vec())
            .map_err(|_| Problem("签不了名（系统给不出随机数）".to_string()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
pub(super) mod tests {
    use super::*;
    use crate::py::fs::{listdir_sorted, mode_of, TempDir};
    use crate::py::hash::to_hex;
    use crate::py::json;
    use crate::py::obj::V;
    use crate::server::x509::testkit;
    use ring::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_ASN1, ECDSA_P256_SHA256_FIXED};

    /// 对照数据（cli/tests/vectors/server_acme_openssl.json）：openssl 造的密钥、证书请求、证书，和 Python 那一份对它们的说法
    pub fn fixture() -> Dict {
        json::loads(include_str!("../../../tests/vectors/server_acme_openssl.json")).unwrap().as_dict().unwrap()
    }

    /// 对照数据里的一个文件（原样的字节）写进 dir，返回路径
    pub fn drop_file(dir: &str, name: &str, file_b64: &str) -> String {
        let p = path::join(dir, name);
        std::fs::write(&p, base64::a2b_base64(file_b64.as_bytes(), true).unwrap()).unwrap();
        p
    }

    /// 对照数据里叫这个名字的那把私钥，读进来
    pub fn fixture_key(dir: &str, name: &str) -> Key {
        let doc = fixture();
        let k =
            doc.list("keys").unwrap().items().into_iter().map(|k| k.as_dict().unwrap()).find(|k| &*k.str("name").unwrap() == name);
        let k = k.unwrap_or_else(|| panic!("对照数据里没有 {name}"));
        Key::open(&drop_file(dir, &format!("{name}.pem"), &k.str("file_b64").unwrap()), false)
            .unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    fn problem(r: Result<Key, Trouble>) -> String {
        match r {
            Err(Trouble::Problem(why)) => why,
            Err(other) => panic!("要的是 Problem，得到 {other:?}"),
            Ok(_) => panic!("不该读得了"),
        }
    }

    /// openssl 写的私钥——PKCS#8 的、SEC1 的、前面带 EC PARAMETERS 的，OpenSSL 3 和 LibreSSL 的——都读得了；公钥、JWK、
    /// 摘要和 Python 那一份（它是让 openssl 算的）一样；签出来的验得过
    #[test]
    fn loads_keys_written_by_openssl() {
        let d = TempDir::new().unwrap();
        let doc = fixture();
        let keys = doc.list("keys").unwrap().items();
        let names: Vec<String> = keys.iter().map(|k| k.as_dict().unwrap().str("name").unwrap().to_string()).collect();
        for want in
            ["pkcs8-openssl3", "pkcs8-from-sec1", "sec1-openssl3", "sec1-libressl", "sec1-params-openssl3", "sec1-params-libressl"]
        {
            assert!(names.iter().any(|n| n == want), "对照数据里要有 {want}");
        }
        for k in keys {
            let k = k.as_dict().unwrap();
            let name = k.str("name").unwrap();
            let p = drop_file(&d.path, &format!("{name}.pem"), &k.str("file_b64").unwrap());
            let key = Key::open(&p, false).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(key.path, p);
            assert_eq!(to_hex(&key.spki()), &*k.str("spki").unwrap(), "{name}");
            assert_eq!(
                json::dumps(&V::Dict(key.jwk()), json::Fmt::new()),
                json::dumps(&k.get("jwk").unwrap(), json::Fmt::new()),
                "{name}"
            );
            assert_eq!(key.thumbprint(), &*k.str("thumbprint").unwrap(), "{name}");
            // 两种签名：每次不一样（带随机数），都验得过；验的是文件里那把私钥的公钥
            let msg = b"protected.payload";
            let point = &key.spki()[P256_SPKI_HEAD.len()..];
            let (a, b) = (key.sign(msg).unwrap(), key.sign(msg).unwrap());
            assert_eq!((a.len(), b.len(), a == b), (64, 64, false), "{name}");
            UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, point).verify(msg, &a).unwrap();
            assert!(UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, point).verify(b"other", &a).is_err());
            let der_sig = key.sign_der(msg).unwrap();
            assert_eq!(der_sig[0], 0x30, "{name}");
            UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, point).verify(msg, &der_sig).unwrap();
            // make 是 true 时已有的文件原样留着
            let before = std::fs::read(&p).unwrap();
            assert_eq!(to_hex(&Key::open(&p, true).unwrap().spki()), &*k.str("spki").unwrap());
            assert_eq!(std::fs::read(&p).unwrap(), before);
        }
        // 同一把私钥的两种写法：公钥一样
        let spki = |name: &str| to_hex(&fixture_key(&d.path, name).spki());
        assert_eq!(spki("pkcs8-openssl3"), spki("sec1-from-pkcs8"));
        assert_eq!(spki("sec1-openssl3"), spki("pkcs8-from-sec1"));
        assert_ne!(spki("pkcs8-openssl3"), spki("sec1-openssl3"));
    }

    /// 不是 P-256 的私钥的文件：别的曲线、RSA、Ed25519、公钥、加了密的、不是密钥的、空的——都说“不是 P-256 的私钥”。
    /// Python 那一份对其中几种说的是“openssl pkey 没成”；曲线的参数整个写出来的那种它收、这里不收（见这个文件开头）
    #[test]
    fn rejects_what_is_not_a_p256_key() {
        let d = TempDir::new().unwrap();
        let doc = fixture();
        let (mut accepted_by_python, mut seen) = (Vec::new(), Vec::new());
        for k in doc.list("bad_keys").unwrap().items() {
            let k = k.as_dict().unwrap();
            let name = k.str("name").unwrap().to_string();
            let p = drop_file(&d.path, &format!("{name}.pem"), &k.str("file_b64").unwrap());
            assert_eq!(problem(Key::open(&p, false)), format!("{p} 不是 P-256 的私钥"), "{name}");
            assert_eq!(problem(Key::open(&p, true)), format!("{p} 不是 P-256 的私钥"), "{name}：已有的文件不会被换掉");
            match &*k.str("python").unwrap() {
                "accepted" => accepted_by_python.push(name.clone()),
                "openssl pkey 没成" | "<path> 不是 P-256 的私钥" => {}
                other => panic!("{name}: Python 说的是 {other}"),
            }
            seen.push(name);
        }
        seen.sort();
        assert_eq!(
            seen,
            [
                "bad-ed25519",
                "bad-empty",
                "bad-encrypted",
                "bad-explicit",
                "bad-explicit-libressl",
                "bad-p384",
                "bad-p384-sec1",
                "bad-public",
                "bad-rsa",
                "bad-text"
            ]
        );
        assert_eq!(accepted_by_python, ["bad-explicit-libressl", "bad-explicit"], "和 Python 不一样的只有这一种");
    }

    /// 没有就生成：PKCS#8 的 PEM、64 个字符一行、只有自己读得了、旁边不留临时文件；再读是同一把。make 是 false 时不生成
    #[test]
    fn makes_a_key_when_there_is_none() {
        let d = TempDir::new().unwrap();
        let p = path::join(&d.path, "account.key");
        assert_eq!(problem(Key::open(&p, false)), format!("没有 {p}"));
        assert!(!path::exists(&p), "make 是 false 时不生成");
        let key = Key::open(&p, true).unwrap();
        assert_eq!((mode_of(&p).unwrap(), listdir_sorted(&d.path).unwrap()), (0o600, vec!["account.key".to_string()]));
        let text = std::fs::read_to_string(&p).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!((lines[0], lines[lines.len() - 1]), ("-----BEGIN PRIVATE KEY-----", "-----END PRIVATE KEY-----"));
        assert!(
            text.ends_with("-----END PRIVATE KEY-----\n")
                && lines[1..lines.len() - 1].iter().all(|l| l.len() <= 64 && !l.is_empty())
        );
        assert_eq!(lines[1].len(), 64);
        let again = Key::open(&p, true).unwrap();
        assert_eq!(key.spki(), again.spki());
        assert_eq!(Key::open(&p, false).unwrap().thumbprint(), key.thumbprint());
        assert_eq!(key.spki().len(), 91);
        assert_ne!(Key::open(&path::join(&d.path, "other.key"), true).unwrap().spki(), key.spki(), "每次生成的不一样");
        // 那里是个目录：make 是 false 时算没有；是 true 时读不了（不会去动它）
        let dir = path::join(&d.path, "dir");
        std::fs::create_dir(&dir).unwrap();
        assert_eq!(problem(Key::open(&dir, false)), format!("没有 {dir}"));
        assert_eq!(problem(Key::open(&dir, true)), format!("读不了 {dir}（Is a directory）"));
        // 目录不在：写不进去，是系统那一类的错（--id 那种方式下一路到入口：起不来）
        let nowhere = path::join(&d.path, "nope/privkey.pem");
        match Key::open(&nowhere, true) {
            Err(Trouble::Os(said)) => {
                assert_eq!(said, format!("[Errno 2] No such file or directory: '{nowhere}.{}.tmp'", std::process::id()))
            }
            other => panic!("{:?}", other.err()),
        }
    }

    /// 摘要照 RFC 7638 算：那几项按键名排好、不带空白的 JSON。和 `json.dumps(jwk, sort_keys=True, separators=(',', ':'))` 是同一串
    #[test]
    fn thumbprint_is_over_the_canonical_jwk() {
        let d = TempDir::new().unwrap();
        let key = fixture_key(&d.path, "pkcs8-openssl3");
        let canonical = json::dumps(&V::Dict(key.jwk()), json::Fmt::new().compact().sort_keys());
        assert!(canonical.starts_with(r#"{"crv":"P-256","kty":"EC","x":""#), "{canonical}");
        assert_eq!(key.thumbprint(), b64u(&hash::sha256(canonical.as_bytes())));
        assert_eq!(key.thumbprint().len(), 43);
        assert_eq!(key.jwk().keys().iter().map(|k| k.to_string()).collect::<Vec<_>>(), ["crv", "kty", "x", "y"]);
        // b64u：换成 URL 里能用的字符，不带结尾的 =
        assert_eq!(
            (b64u(b""), b64u(&[0xfb, 0xff]), b64u(b"a"), b64u(b"{}")),
            (String::new(), "-_8".into(), "YQ".into(), "e30".into())
        );
    }

    /// 公钥前面那一段和测试里造证书的那一份（x509.rs 的 testkit）是同一个；SEC1 包成的 PKCS#8 ring 读得了
    #[test]
    fn spki_head_and_sec1_wrapping() {
        assert_eq!(P256_SPKI_HEAD, testkit::P256_SPKI_HEAD);
        // 从 PKCS#8 里把 SEC1 的那一段取出来（最后那个 OCTET STRING 的内容），自己再包一遍：和原来的一个字节不差
        let pkcs8 = testkit::new_key();
        let at = pkcs8.windows(2).position(|w| w == [0x04, 0x6d]).unwrap();
        assert_eq!(wrap_sec1(&pkcs8[at + 2..]), pkcs8, "ring 生成的 PKCS#8 就是这个样子");
        let text = pem("EC PRIVATE KEY", &pkcs8[at + 2..]);
        assert_eq!(pkcs8_of(&text).unwrap(), pkcs8);
        // 私钥开头是 0、LibreSSL 少写了那一个字节的：补上了，包出来的和原来的一样
        let zero = std::iter::repeat_with(testkit::new_key).find(|k| k[36] == 0).unwrap();
        let mut short = zero[at + 2..].to_vec();
        short.remove(7);
        short[1] -= 1;
        short[6] -= 1;
        assert_eq!(pkcs8_of(&pem("EC PRIVATE KEY", &short)).unwrap(), zero);
        // 两种都有时用前面的那个；一种都没有是 None
        let both = format!("{}{}", pem("PRIVATE KEY", b"first"), pem("EC PRIVATE KEY", b"second"));
        assert_eq!(pkcs8_of(&both).unwrap(), b"first");
        let both = format!("junk\n{}{}", pem("EC PRIVATE KEY", b"second"), pem("PRIVATE KEY", b"first"));
        assert_eq!(pkcs8_of(&both).unwrap(), wrap_sec1(b"second"));
        assert_eq!(pkcs8_of(&pem("RSA PRIVATE KEY", b"x")), None);
        assert_eq!(pkcs8_of(&pem("ENCRYPTED PRIVATE KEY", b"x")), None);
        assert_eq!(pkcs8_of("-----BEGIN PRIVATE KEY-----\nAAAA\n"), None, "没有结尾那一行");
        assert_eq!(pkcs8_of(""), None);
    }
}
