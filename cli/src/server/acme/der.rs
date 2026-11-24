//! 拼 DER 用的那一点东西：标签、长度、内容。证书请求（csr.rs）、把 SEC1 的私钥包成 PKCS#8（key.rs）用它。
//! 读 DER 的在 `x509.rs`（只读证书）；这里只写。

pub const INTEGER: u8 = 0x02;
pub const BIT_STRING: u8 = 0x03;
pub const OCTET_STRING: u8 = 0x04;
pub const OID: u8 = 0x06;
pub const SEQUENCE: u8 = 0x30;
pub const SET: u8 = 0x31;

/// 一项 DER：标签、长度（最短的那种写法：DER 只许这一种）、接起来的内容
pub fn der(tag: u8, parts: &[&[u8]]) -> Vec<u8> {
    let len: usize = parts.iter().map(|p| p.len()).sum();
    let mut out = Vec::with_capacity(len.saturating_add(10));
    out.push(tag);
    if len < 0x80 {
        out.push(len as u8);
    } else {
        // 长的：先写“后面几个字节是长度”，再写长度（高位在前，不带前面的 0）
        let digits: Vec<u8> = len.to_be_bytes().iter().copied().skip_while(|b| *b == 0).collect();
        out.push(0x80 | digits.len() as u8);
        out.extend_from_slice(&digits);
    }
    for p in parts {
        out.extend_from_slice(p);
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::server::x509::testkit;

    /// 长度的几种写法：127 以内一个字节，再长的前面加一个“几个字节”
    #[test]
    fn length_forms() {
        assert_eq!(der(SEQUENCE, &[]), [0x30, 0x00]);
        assert_eq!(der(INTEGER, &[&[0]]), [0x02, 0x01, 0x00]);
        assert_eq!(der(OCTET_STRING, &[&[7u8; 127]])[..2], [0x04, 0x7f]);
        assert_eq!(der(OCTET_STRING, &[&[7u8; 128]])[..3], [0x04, 0x81, 0x80]);
        assert_eq!(der(OCTET_STRING, &[&[7u8; 255]])[..3], [0x04, 0x81, 0xff]);
        assert_eq!(der(OCTET_STRING, &[&[7u8; 256]])[..4], [0x04, 0x82, 0x01, 0x00]);
        assert_eq!(der(OCTET_STRING, &[&[7u8; 65535]])[..4], [0x04, 0x82, 0xff, 0xff]);
        let big = der(OCTET_STRING, &[&vec![7u8; 65536]]);
        assert_eq!((&big[..5], big.len()), (&[0x04, 0x83, 0x01, 0x00, 0x00][..], 65536 + 5));
        // 几段内容接在一起
        assert_eq!(der(SET, &[&[1, 2], &[], &[3]]), [0x31, 0x03, 1, 2, 3]);
        assert_eq!(der(BIT_STRING, &[&[0], &[0xaa, 0xbb]]), [0x03, 0x03, 0x00, 0xaa, 0xbb]);
    }

    /// 和测试里造证书的那一个（x509.rs 的 testkit）拼出来的一样
    #[test]
    fn same_as_the_test_builder() {
        for n in [0usize, 1, 127, 128, 200, 255, 256, 1000, 65535] {
            let body = vec![0x5a; n];
            assert_eq!(der(OID, &[&body]), testkit::der(OID, &[&body]), "{n}");
        }
    }
}
