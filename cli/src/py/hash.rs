//! `hashlib`、`hmac`：sha1、sha256 的十六进制摘要，HMAC-SHA256。

use sha1::Sha1;
use sha2::{Digest, Sha256};

fn hex(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 15) as usize] as char);
    }
    s
}

/// `bytes.hex()`
pub fn to_hex(bytes: &[u8]) -> String {
    hex(bytes)
}

/// `bytes.fromhex(s)`：不是成对的十六进制数字是 ValueError（这里是 None）；中间的空白不认
pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() % 2 != 0 {
        return None;
    }
    b.chunks(2).map(|p| Some(((p[0] as char).to_digit(16)? * 16 + (p[1] as char).to_digit(16)?) as u8)).collect()
}

/// `hashlib.sha1(data).hexdigest()`
pub fn sha1_hex(data: impl AsRef<[u8]>) -> String {
    hex(&Sha1::digest(data.as_ref()))
}

/// `hashlib.sha256(data).digest()`
pub fn sha256(data: impl AsRef<[u8]>) -> [u8; 32] {
    Sha256::digest(data.as_ref()).into()
}

/// `hashlib.sha256(data).hexdigest()`
pub fn sha256_hex(data: impl AsRef<[u8]>) -> String {
    hex(&sha256(data))
}

/// `hashlib.sha256()`，一块一块喂（大文件）
#[derive(Default)]
pub struct Sha256Stream(Sha256);

impl Sha256Stream {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, data: impl AsRef<[u8]>) {
        self.0.update(data.as_ref());
    }

    pub fn hexdigest(self) -> String {
        hex(&self.0.finalize())
    }
}

/// `hmac.new(key, msg, 'sha256').digest()`
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&sha256(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// `hmac.compare_digest(a, b)`：比的时间不随内容变
pub fn compare_digest(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests() {
        assert_eq!(sha1_hex("abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(sha256_hex("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let mut s = Sha256Stream::new();
        s.update("a");
        s.update("bc");
        assert_eq!(s.hexdigest(), sha256_hex("abc"));
    }

    #[test]
    fn hmac_rfc4231() {
        // RFC 4231 的第 1、2、6 组
        assert_eq!(to_hex(&hmac_sha256(&[0x0b; 20], b"Hi There")), "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
        assert_eq!(
            to_hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            to_hex(&hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn hex_and_compare() {
        assert_eq!(from_hex("00ff10"), Some(vec![0, 255, 16]));
        assert_eq!(from_hex("0g"), None);
        assert_eq!(from_hex("abc"), None);
        assert!(compare_digest(b"abc", b"abc") && !compare_digest(b"abc", b"abd") && !compare_digest(b"ab", b"abc"));
    }
}
