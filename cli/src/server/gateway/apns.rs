//! 苹果的推送服务（APNs）那一头：令牌认证用的 JWT、请求的头和内容、回的怎么算。照苹果的文档（developer.apple.com 的 User
//! Notifications 下面）：Establishing a token-based connection to APNs、Sending notification requests to APNs、Handling notification
//! responses from APNs。
//!
//! - 令牌：头 `{"alg":"ES256","kid":密钥编号}`、内容 `{"iss":Team ID,"iat":签的时间}`，各自 base64url，签名是 P-256 的 r‖s（64 个
//!   字节），拿开发者账号里建的那个 .p8 签。苹果要一小时以内签的、又不许 20 分钟以内就换（TooManyProviderTokenUpdates）：45 分钟换一个；
//!   回说 ExpiredProviderToken 的话，用了一分钟以上的作废、换一个再发一次
//! - 请求：`POST /3/device/<令牌>`，apns-push-type alert、apns-priority 10、apns-expiration（`EXPIRES`：手机这么久都没连上就不要了；
//!   推送服务对一个 App 本来也只留最后一条）、apns-collapse-id 是宿主给的编号（同一条经两个配对到了同一台手机时只显示一次）。
//!   内容是 `{"aps":{"alert":{…},"mutable-content":1,"sound":"default"},"n":密文}`：alert 是手机解不开时显示的那两句，App 的通知扩展
//!   解开 n、换成真的标题和正文
//! - 回的（`judge`）：200 是送到了；410、BadDeviceToken、DeviceTokenNotForTopic、Unregistered、ExpiredToken 是这个令牌没用了（Team 的
//!   服务器拿掉那个登记）；429 是这台手机推得太多了；ExpiredProviderToken 换令牌再试；5xx 换一条连接再试；别的不再试

use crate::py::base64::urlsafe_b64encode;
use crate::py::json::{self, Fmt};
use crate::py::obj::V;
use crate::server::wire::Blob;
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::PrivateKeyDer;
use std::sync::{Mutex, PoisonError};

/// 正式、开发两种环境的推送服务
pub const PRODUCTION: &str = "https://api.push.apple.com";
pub const SANDBOX: &str = "https://api.sandbox.push.apple.com";
/// iOS 上 cca App 的 bundle id：推送的 topic
pub const TOPIC: &str = "com.cca.remote";
/// 手机这么久没连上就不要了（秒）
pub const EXPIRES: i64 = 12 * 3600;
/// 令牌用多久换一个；回说过期了时，用了多久的才作废（秒）
const TOKEN_FOR: f64 = 45.0 * 60.0;
const TOKEN_MIN: f64 = 60.0;
/// 手机解不开时显示的那两句（App 的通知扩展解开了就换掉）
const FALLBACK_TITLE: &str = "cca";
const FALLBACK_BODY: &str = "远程会话里有新的通知";

fn b64url(data: &[u8]) -> String {
    urlsafe_b64encode(data).trim_end_matches('=').to_string()
}

/// 签令牌的：.p8 里的私钥、它的编号（Key ID）、开发者账号的 Team ID
pub struct Signer {
    key: EcdsaKeyPair,
    key_id: String,
    team_id: String,
    rng: SystemRandom,
}

impl Signer {
    /// pem 是 .p8 文件的内容（PKCS#8 的 P-256 私钥）。读不了、用不了是 Err（给人看的一句话，不带私钥的内容）
    pub fn new(pem: &[u8], key_id: &str, team_id: &str) -> Result<Signer, String> {
        let der = match PrivateKeyDer::from_pem_slice(pem) {
            Ok(PrivateKeyDer::Pkcs8(der)) => der,
            Ok(_) => return Err("不是 PKCS#8 的私钥（.p8 的开头是 -----BEGIN PRIVATE KEY-----）".into()),
            Err(_) => return Err("读不出私钥（要的是开发者账号里建的那个 .p8 文件）".into()),
        };
        let rng = SystemRandom::new();
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, der.secret_pkcs8_der(), &rng)
            .map_err(|e| format!("私钥用不了（{e}）：要的是 P-256 的"))?;
        Ok(Signer { key, key_id: key_id.to_string(), team_id: team_id.to_string(), rng })
    }

    /// iat（签的时间，秒）签的令牌
    pub fn token(&self, iat: i64) -> Result<String, String> {
        let head = json::dumps(&V::Dict(crate::dict! { "alg" => "ES256", "kid" => self.key_id.as_str() }), Fmt::new().compact());
        let claims = json::dumps(&V::Dict(crate::dict! { "iss" => self.team_id.as_str(), "iat" => iat }), Fmt::new().compact());
        let input = format!("{}.{}", b64url(head.as_bytes()), b64url(claims.as_bytes()));
        let sig = self.key.sign(&self.rng, input.as_bytes()).map_err(|_| "签不了令牌".to_string())?;
        Ok(format!("{input}.{}", b64url(sig.as_ref())))
    }
}

/// 现在用的令牌（两种环境的那两条线程共用一个）
pub struct Tokens {
    signer: Signer,
    cur: Mutex<Option<(String, f64)>>,
}

impl Tokens {
    pub fn new(signer: Signer) -> Tokens {
        Tokens { signer, cur: Mutex::new(None) }
    }

    /// 现在该用的：没有、用了 `TOKEN_FOR` 以上（或者时钟往回走了）的换一个。now 是 time.time()
    pub fn get(&self, now: f64) -> Result<String, String> {
        let mut cur = self.cur.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((token, _)) = cur.as_ref().filter(|(_, at)| (0.0..TOKEN_FOR).contains(&(now - at))) {
            return Ok(token.clone());
        }
        let token = self.signer.token(now as i64)?;
        *cur = Some((token.clone(), now));
        Ok(token)
    }

    /// 推送服务说令牌过期了：用了 `TOKEN_MIN` 以上的作废（下一次 get 换新的；刚换的不作废，免得换得太勤）
    pub fn stale(&self, now: f64) {
        let mut cur = self.cur.lock().unwrap_or_else(PoisonError::into_inner);
        if cur.as_ref().is_some_and(|(_, at)| now - at >= TOKEN_MIN) {
            *cur = None;
        }
    }
}

/// 一条通知的请求头（照先后，伪头在前）。authority：推送服务的主机名（不是 443 端口时带端口）
pub fn headers(authority: &str, device: &str, jwt: &str, topic: &str, id: &str, expires: i64) -> Vec<(String, String)> {
    [
        (":method", "POST".to_string()),
        (":scheme", "https".to_string()),
        (":authority", authority.to_string()),
        (":path", format!("/3/device/{device}")),
        ("authorization", format!("bearer {jwt}")),
        ("apns-push-type", "alert".to_string()),
        ("apns-topic", topic.to_string()),
        ("apns-priority", "10".to_string()),
        ("apns-expiration", expires.to_string()),
        ("apns-collapse-id", id.to_string()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// 一条通知的内容：解不开时显示的那两句、要通知扩展来改（mutable-content）、密文
pub fn payload(note: &Blob) -> Vec<u8> {
    let alert = crate::dict! { "title" => FALLBACK_TITLE, "body" => FALLBACK_BODY };
    let aps = crate::dict! { "alert" => alert, "mutable-content" => 1, "sound" => "default" };
    json::dumps(&V::Dict(crate::dict! { "aps" => aps, "n" => note.to_v() }), Fmt::new().utf8().compact()).into_bytes()
}

/// 推送服务回的怎么算
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// 送到了
    Sent,
    /// 这个令牌没用了（App 删了、不是这个 App 的、不是这种环境的）：状态码和原因
    Gone(String),
    /// 这台手机推得太多了
    Busy,
    /// 我们的令牌（JWT）过期了：换一个再试
    Stale,
    /// 推送服务那头出了错、连不上、没回：换一条连接再试（试过了的话不再试）
    Down(String),
    /// 别的（请求不对、我们的密钥不对……）：不再试
    Refused(String),
}

/// 状态码（认不出来是 None）和回的正文（出错时是 `{"reason": …}`）
pub fn judge(status: Option<u16>, body: &[u8]) -> Outcome {
    let reason = std::str::from_utf8(body)
        .ok()
        .and_then(|text| json::loads(text).ok())
        .and_then(|v| v.as_dict())
        .and_then(|d| d.str("reason"))
        .map(|r| r.to_string())
        .unwrap_or_default();
    let said = || {
        let code = status.map_or_else(|| "看不懂的状态".to_string(), |s| s.to_string());
        if reason.is_empty() {
            code
        } else {
            format!("{code} {reason}")
        }
    };
    match (status, reason.as_str()) {
        (Some(200), _) => Outcome::Sent,
        (_, "Unregistered" | "ExpiredToken" | "BadDeviceToken" | "DeviceTokenNotForTopic") | (Some(410), _) => {
            Outcome::Gone(said())
        }
        (Some(429), _) => Outcome::Busy,
        (Some(403), "ExpiredProviderToken") => Outcome::Stale,
        (Some(s), _) if s >= 500 => Outcome::Down(said()),
        _ => Outcome::Refused(said()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::py::base64::urlsafe_b64decode;
    use crate::server::wire::parse_blob;
    use crate::server::x509::testkit;
    use crate::server::PUSH_BLOB;
    use ring::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_FIXED};

    fn pem(der: &[u8]) -> Vec<u8> {
        testkit::pem("PRIVATE KEY", der).into_bytes()
    }

    /// 苹果给的 .p8 的样子：ECPrivateKey 里多一段 [0] 参数（曲线的 OID）；ring 生成的没有它
    fn apple_style(ring_der: &[u8]) -> Vec<u8> {
        let head: [u8; 36] = [
            0x30, 0x81, 0x87, 0x02, 0x01, 0x00, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
            0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x04, 0x6d, 0x30, 0x6b, 0x02, 0x01, 0x01, 0x04, 0x20,
        ];
        assert_eq!(ring_der[..36], head);
        let (key, public) = ring_der[36..].split_at(32);
        let mut out = vec![0x30, 0x81, 0x93];
        out.extend_from_slice(&head[3..27]);
        out.extend_from_slice(&[0x04, 0x79, 0x30, 0x77, 0x02, 0x01, 0x01, 0x04, 0x20]);
        out.extend_from_slice(key);
        out.extend_from_slice(&[0xa0, 0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07]);
        out.extend_from_slice(public);
        out
    }

    fn part(token: &str, i: usize) -> Vec<u8> {
        let p = token.split('.').nth(i).unwrap();
        urlsafe_b64decode(&format!("{p}{}", "=".repeat((4 - p.len() % 4) % 4))).unwrap()
    }

    /// 令牌是 ES256 的 JWT：头、内容照苹果的文档，签名拿 .p8 的公钥验得过。ring 生成的、苹果那种带参数的 .p8 都认
    #[test]
    fn tokens_are_es256_jwts() {
        let der = testkit::new_key();
        let apple = apple_style(&der);
        assert!(crate::py::base64::b64encode(&apple).starts_with("MIGTAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBHkwdwIBAQQg"));
        let public = testkit::point_of(&der);
        for key in [&der, &apple] {
            let s = Signer::new(&pem(key), "KEYID12345", "TEAMID1234").unwrap();
            let token = s.token(1_794_000_000).unwrap();
            assert_eq!(part(&token, 0), br#"{"alg":"ES256","kid":"KEYID12345"}"#);
            assert_eq!(part(&token, 1), br#"{"iss":"TEAMID1234","iat":1794000000}"#);
            let sig = part(&token, 2);
            assert_eq!(sig.len(), 64);
            let input = token.rsplit_once('.').unwrap().0;
            UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &public).verify(input.as_bytes(), &sig).unwrap();
        }
        // 不是那种私钥的：说清楚，不带内容
        let sec1 = testkit::pem("EC PRIVATE KEY", &[0x30, 0x03, 0x02, 0x01, 0x01]).into_bytes();
        assert!(Signer::new(&sec1, "KEYID12345", "TEAMID1234").err().unwrap().starts_with("不是 PKCS#8"));
        assert!(Signer::new(b"hello", "KEYID12345", "TEAMID1234").err().unwrap().starts_with("读不出私钥"));
        let ed = ring::signature::Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
        assert!(Signer::new(&pem(ed.as_ref()), "KEYID12345", "TEAMID1234").err().unwrap().starts_with("私钥用不了"));
    }

    /// 一个令牌用 45 分钟；推送服务说过期了：用了一分钟以上的才作废；时钟往回走了也换
    #[test]
    fn tokens_are_reused_for_45_minutes() {
        let t = Tokens::new(Signer::new(&pem(&testkit::new_key()), "KEYID12345", "TEAMID1234").unwrap());
        let first = t.get(10_000.0).unwrap();
        assert_eq!(t.get(10_000.0 + 44.0 * 60.0).unwrap(), first);
        let second = t.get(10_000.0 + 46.0 * 60.0).unwrap();
        assert_ne!(second, first);
        t.stale(10_000.0 + 46.5 * 60.0);
        assert_eq!(t.get(10_000.0 + 46.6 * 60.0).unwrap(), second, "刚换的不作废");
        t.stale(10_000.0 + 48.0 * 60.0);
        let third = t.get(10_000.0 + 48.0 * 60.0).unwrap();
        assert_ne!(third, second);
        assert_ne!(t.get(9_000.0).unwrap(), third, "时钟往回走了");
    }

    /// 请求的头、内容；最长的通知放得下（推送服务一条最多 4096 个字节）
    #[test]
    fn headers_and_payload() {
        let head = headers("api.push.apple.com", "ab12", "x.y.z", TOPIC, "0123456789abcdef", 1_794_043_200);
        let want = [
            (":method", "POST"),
            (":scheme", "https"),
            (":authority", "api.push.apple.com"),
            (":path", "/3/device/ab12"),
            ("authorization", "bearer x.y.z"),
            ("apns-push-type", "alert"),
            ("apns-topic", "com.cca.remote"),
            ("apns-priority", "10"),
            ("apns-expiration", "1794043200"),
            ("apns-collapse-id", "0123456789abcdef"),
        ];
        assert_eq!(head, want.map(|(k, v)| (k.to_string(), v.to_string())));
        let blob = |c: usize| {
            let text = format!(r#"{{"n":"{}==","c":"{}","t":"{}="}}"#, "A".repeat(22), "A".repeat(c), "A".repeat(43));
            parse_blob(&json::loads(&text).unwrap(), PUSH_BLOB).unwrap()
        };
        assert_eq!(
            String::from_utf8(payload(&blob(4))).unwrap(),
            format!(
                r#"{{"aps":{{"alert":{{"title":"cca","body":"远程会话里有新的通知"}},"mutable-content":1,"sound":"default"}},"n":{{"n":"{}==","c":"AAAA","t":"{}="}}}}"#,
                "A".repeat(22),
                "A".repeat(43)
            )
        );
        assert!(payload(&blob(PUSH_BLOB)).len() < 4096);
    }

    /// 回的怎么算
    #[test]
    fn replies() {
        let r = |status: Option<u16>, reason: &str| {
            let body =
                if reason.is_empty() { String::new() } else { format!(r#"{{"reason":"{reason}","timestamp":1794000000000}}"#) };
            judge(status, body.as_bytes())
        };
        assert_eq!(r(Some(200), ""), Outcome::Sent);
        assert_eq!(r(Some(410), "Unregistered"), Outcome::Gone("410 Unregistered".into()));
        assert_eq!(r(Some(410), "ExpiredToken"), Outcome::Gone("410 ExpiredToken".into()));
        assert_eq!(r(Some(410), ""), Outcome::Gone("410".into()));
        assert_eq!(r(Some(400), "BadDeviceToken"), Outcome::Gone("400 BadDeviceToken".into()));
        assert_eq!(r(Some(400), "DeviceTokenNotForTopic"), Outcome::Gone("400 DeviceTokenNotForTopic".into()));
        assert_eq!(r(Some(429), "TooManyRequests"), Outcome::Busy);
        assert_eq!(r(Some(403), "ExpiredProviderToken"), Outcome::Stale);
        assert_eq!(r(Some(403), "InvalidProviderToken"), Outcome::Refused("403 InvalidProviderToken".into()));
        assert_eq!(r(Some(413), "PayloadTooLarge"), Outcome::Refused("413 PayloadTooLarge".into()));
        assert_eq!(r(Some(500), "InternalServerError"), Outcome::Down("500 InternalServerError".into()));
        assert_eq!(r(Some(503), "ServiceUnavailable"), Outcome::Down("503 ServiceUnavailable".into()));
        assert_eq!(r(None, ""), Outcome::Refused("看不懂的状态".into()));
        assert_eq!(judge(Some(400), b"not json"), Outcome::Refused("400".into()));
    }
}
