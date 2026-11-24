//! 哈希、HMAC、PBKDF2、随机数、base64 / hex / Crockford base32：都用 ring 和这里自己写的，不用辅助程序的代码
//!
//! 名字照 Python 里测试原来用的那几个：`hashlib.sha256` → [`sha256`]，`hmac.new(k, m, sha256)` → [`hmac_sha256`]，
//! `hashlib.pbkdf2_hmac` → [`pbkdf2_hmac_sha256`]，`secrets.token_bytes / token_hex` → [`random`] / [`token_hex`]，
//! `hmac.compare_digest` → [`ct_eq`]，`base64.b64encode / urlsafe_b64encode` → [`b64`] / [`b64url`]。
//! 解码都是严格的：字母表以外的字符、= 的个数不对、末尾多出来的位不是 0，一律不认（测试拿到的是被测的程序写出来的，
//! 写得不规矩就该看得出来）。

use ring::rand::SecureRandom;

/// SHA-256
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(ring::digest::digest(&ring::digest::SHA256, data).as_ref());
    out
}

/// SHA-256，写成小写的十六进制（`hashlib.sha256(x).hexdigest()`）
pub fn sha256_hex(data: &[u8]) -> String {
    hex(&sha256(data))
}

/// SHA-1（账号目录名里那 6 位摘要、WebSocket 握手用的是它；别的地方不要用）
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut out = [0u8; 20];
    out.copy_from_slice(ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, data).as_ref());
    out
}

/// SHA-1，写成小写的十六进制
pub fn sha1_hex(data: &[u8]) -> String {
    hex(&sha1(data))
}

/// HMAC-SHA256（`hmac.new(key, data, hashlib.sha256).digest()`）
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let tag = ring::hmac::sign(&ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key), data);
    let mut out = [0u8; 32];
    out.copy_from_slice(tag.as_ref());
    out
}

/// PBKDF2-HMAC-SHA256（`hashlib.pbkdf2_hmac('sha256', password, salt, iterations, len)`）；iterations 是 0 就 panic
pub fn pbkdf2_hmac_sha256(password: &[u8], salt: &[u8], iterations: u32, len: usize) -> Vec<u8> {
    let rounds = std::num::NonZeroU32::new(iterations).expect("PBKDF2 的轮数不能是 0");
    let mut out = vec![0u8; len];
    ring::pbkdf2::derive(ring::pbkdf2::PBKDF2_HMAC_SHA256, rounds, salt, password, &mut out);
    out
}

/// n 个随机字节（系统的随机数；`secrets.token_bytes(n)`）
pub fn random(n: usize) -> Vec<u8> {
    let mut out = vec![0u8; n];
    ring::rand::SystemRandom::new().fill(&mut out).expect("系统的随机数取不到");
    out
}

/// n 个随机字节写成 2n 个十六进制字符（`secrets.token_hex(n)`）
pub fn token_hex(n: usize) -> String {
    hex(&random(n))
}

/// n 个随机字节写成 URL 安全的 base64、不带 =（`secrets.token_urlsafe(n)`）
pub fn token_urlsafe(n: usize) -> String {
    b64url_nopad(&random(n))
}

/// 两段字节是不是一样；用时只看长度，不看从第几个字节开始不一样（`hmac.compare_digest`）
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    // black_box：不让优化器把上面的循环改成“见到不一样就提前出来”
    std::hint::black_box(diff) == 0
}

const STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn b64_encode(data: &[u8], table: &[u8; 64], pad: bool) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        // 1 个字节写 2 个字符，2 个写 3 个，3 个写 4 个；不够 4 个的用 = 补齐（要补的话）
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(table[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else if pad {
                out.push('=');
            }
        }
    }
    out
}

fn b64_decode_with(text: &str, table: &[u8; 64], pad: bool) -> Result<Vec<u8>, String> {
    let bytes = text.as_bytes();
    let body = if pad {
        if !bytes.len().is_multiple_of(4) {
            return Err(format!("base64 的长度（{}）不是 4 的倍数", bytes.len()));
        }
        let pads = bytes.iter().rev().take_while(|b| **b == b'=').count();
        if pads > 2 {
            return Err("base64 末尾的 = 多于两个".to_string());
        }
        &bytes[..bytes.len() - pads]
    } else {
        bytes
    };
    if body.len() % 4 == 1 {
        return Err(format!("base64 的长度（{}）不对：多出来一个字符", body.len()));
    }
    let mut out = Vec::with_capacity(body.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for (i, b) in body.iter().enumerate() {
        let Some(v) = table.iter().position(|t| t == b) else {
            return Err(format!("base64 里第 {} 个字符（{:?}）不在字母表里", i + 1, *b as char));
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    if acc != 0 {
        return Err("base64 末尾多出来的位不是 0".to_string());
    }
    Ok(out)
}

/// base64，标准字母表，带 =（`base64.b64encode`）
pub fn b64(data: &[u8]) -> String {
    b64_encode(data, STD, true)
}

/// base64，标准字母表，不带 =（ssh 的指纹 `SHA256:…` 是这个样子）
pub fn b64_nopad(data: &[u8]) -> String {
    b64_encode(data, STD, false)
}

/// base64，URL 安全的字母表（- 和 _），带 =（`base64.urlsafe_b64encode`）
pub fn b64url(data: &[u8]) -> String {
    b64_encode(data, URL, true)
}

/// base64，URL 安全的字母表，不带 =（链接、JWS 里的写法）
pub fn b64url_nopad(data: &[u8]) -> String {
    b64_encode(data, URL, false)
}

/// 解 [`b64`] 写的：= 要带齐
pub fn b64_decode(text: &str) -> Result<Vec<u8>, String> {
    b64_decode_with(text, STD, true)
}

/// 解 [`b64_nopad`] 写的：不能带 =
pub fn b64_nopad_decode(text: &str) -> Result<Vec<u8>, String> {
    b64_decode_with(text, STD, false)
}

/// 解 [`b64url`] 写的：= 要带齐
pub fn b64url_decode(text: &str) -> Result<Vec<u8>, String> {
    b64_decode_with(text, URL, true)
}

/// 解 [`b64url_nopad`] 写的：不能带 =
pub fn b64url_nopad_decode(text: &str) -> Result<Vec<u8>, String> {
    b64_decode_with(text, URL, false)
}

/// 小写的十六进制（`bytes.hex()`）
pub fn hex(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 2);
    for b in data {
        out.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((b & 15) as u32, 16).unwrap());
    }
    out
}

/// 十六进制读回字节（大小写都认；`bytes.fromhex`，但不认中间的空白）
pub fn unhex(text: &str) -> Result<Vec<u8>, String> {
    let digits = text.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return Err(format!("十六进制的长度（{}）是单数", digits.len()));
    }
    let digit = |i: usize| {
        (digits[i] as char).to_digit(16).ok_or_else(|| format!("第 {} 个字符（{:?}）不是十六进制", i + 1, digits[i] as char))
    };
    (0..digits.len() / 2).map(|i| Ok((digit(2 * i)? << 4 | digit(2 * i + 1)?) as u8)).collect()
}

/// Crockford base32 的字母表（没有 I L O U）
const B32C: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Crockford base32：把这几个字节当一个大端的整数写出来，5 位一个字符，前面不够的位补 0（10 个字节正好 16 个字符：
/// 远程会话的配对码就是这样写的，见 server/README.md）
pub fn b32c(data: &[u8]) -> String {
    let chars = (data.len() * 8).div_ceil(5);
    let mut out = String::with_capacity(chars);
    for i in (0..chars).rev() {
        // 从低位数第 5i 位起的 5 位：可能跨两个字节
        let mut v = 0u8;
        for bit in (5 * i..5 * i + 5).rev() {
            let byte = bit / 8;
            let set = byte < data.len() && data[data.len() - 1 - byte] >> (bit % 8) & 1 == 1;
            v = v << 1 | set as u8;
        }
        out.push(B32C[v as usize] as char);
    }
    out
}

/// 读回 [`b32c`] 写的 len 个字节。人输入的写法都认：大小写、中间的横线和空格随意，I L 当 1、O 当 0；
/// 字符数不对、有别的字符、数值放不进 len 个字节是 None
pub fn b32c_decode(text: &str, len: usize) -> Option<Vec<u8>> {
    let mut digits = Vec::new();
    for ch in text.chars().filter(|c| *c != '-' && *c != ' ') {
        let ch = match ch.to_ascii_uppercase() {
            'I' | 'L' => '1',
            'O' => '0',
            c => c,
        };
        digits.push(B32C.iter().position(|b| *b as char == ch)? as u8);
    }
    if digits.len() != (len * 8).div_ceil(5) {
        return None;
    }
    let mut out = vec![0u8; len];
    for (i, v) in digits.iter().rev().enumerate() {
        for k in 0..5 {
            if v >> k & 1 == 1 {
                let bit = 5 * i + k;
                // 最前面那个字符多出来的位必须是 0
                *out.get_mut(len.checked_sub(1 + bit / 8)?)? |= 1 << (bit % 8);
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    /// RFC 4231 的 1、2、3、4、6、7 号（5 号是截短的，这里没有截短的写法）
    #[test]
    fn hmac_rfc4231() {
        let cases: &[(Vec<u8>, Vec<u8>, &str)] = &[
            (vec![0x0b; 20], b"Hi There".to_vec(), "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"),
            (b"Jefe".to_vec(), b"what do ya want for nothing?".to_vec(), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"),
            (vec![0xaa; 20], vec![0xdd; 50], "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"),
            ((1..=25).collect(), vec![0xcd; 50], "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b"),
            (
                vec![0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First".to_vec(),
                "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54",
            ),
            (
                vec![0xaa; 131],
                b"This is a test using a larger than block-size key and a larger than block-size data. The key needs to be hashed before being used by the HMAC algorithm.".to_vec(),
                "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2",
            ),
        ];
        for (key, data, want) in cases {
            assert_eq!(hex(&hmac_sha256(key, data)), *want);
        }
    }

    /// RFC 7914 第 11 节的两组，和照 RFC 6070 的输入算的 SHA-256 那几组（各家实现都拿它们对）
    #[test]
    fn pbkdf2_vectors() {
        let cases: &[(&[u8], &[u8], u32, usize, &str)] = &[
            (
                b"passwd",
                b"salt",
                1,
                64,
                "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783",
            ),
            (
                b"Password",
                b"NaCl",
                80000,
                64,
                "4ddcd8f60b98be21830cee5ef22701f9641a4418d04c0414aeff08876b34ab56a1d425a1225833549adb841b51c9b3176a272bdebba1d078478f62b397f33c8d",
            ),
            (b"password", b"salt", 1, 32, "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"),
            (b"password", b"salt", 2, 32, "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43"),
            (b"password", b"salt", 4096, 32, "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a"),
            (
                b"passwordPASSWORDpassword",
                b"saltSALTsaltSALTsaltSALTsaltSALTsalt",
                4096,
                40,
                "348c89dbcbd32b2f32d814b8116e84cf2b17347ebc1800181c4e2a1fb8dd53e1c635518c7dac47e9",
            ),
            (b"pass\0word", b"sa\0lt", 4096, 16, "89b69d0516f829893c696226650a8687"),
        ];
        for (password, salt, rounds, len, want) in cases {
            assert_eq!(hex(&pbkdf2_hmac_sha256(password, salt, *rounds, *len)), *want);
        }
    }

    /// RFC 4648 第 10 节
    #[test]
    fn base64_rfc4648() {
        let cases = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (plain, coded) in cases {
            assert_eq!(b64(plain.as_bytes()), coded);
            assert_eq!(b64_decode(coded).unwrap(), plain.as_bytes());
            assert_eq!(b64url(plain.as_bytes()), coded);
            assert_eq!(b64url_decode(coded).unwrap(), plain.as_bytes());
            let bare = coded.trim_end_matches('=');
            assert_eq!(b64_nopad(plain.as_bytes()), bare);
            assert_eq!(b64_nopad_decode(bare).unwrap(), plain.as_bytes());
            assert_eq!(b64url_nopad(plain.as_bytes()), bare);
            assert_eq!(b64url_nopad_decode(bare).unwrap(), plain.as_bytes());
        }
    }

    #[test]
    fn base64_alphabets_and_strictness() {
        assert_eq!(b64(&[0xfb, 0xff, 0xfe]), "+//+");
        assert_eq!(b64url(&[0xfb, 0xff, 0xfe]), "-__-");
        assert_eq!(b64url(&[0xfb, 0xff]), "-_8=");
        assert_eq!(b64url_nopad(&[0xfb, 0xff]), "-_8");
        assert_eq!(b64url_decode("-_8=").unwrap(), [0xfb, 0xff]);
        assert_eq!(b64url_nopad_decode("-_8").unwrap(), [0xfb, 0xff]);
        // 两种字母表不混着认
        assert!(b64_decode("-_8=").is_err());
        assert!(b64url_decode("+/8=").is_err());
        // = 要么带齐（带 = 的那两个），要么一个不带（不带的那两个）
        for bad in ["Zg", "Zg=", "Zm8", "Zg===", "=Zg=", "Z=g=", "Zm9v====", "Zm9vZ"] {
            assert!(b64_decode(bad).is_err(), "{bad}");
        }
        for bad in ["Zg==", "Zm8=", "Z"] {
            assert!(b64_nopad_decode(bad).is_err(), "{bad}");
            assert!(b64url_nopad_decode(bad).is_err(), "{bad}");
        }
        // 空白、换行、别的字符不跳过
        for bad in ["Zm9v\n", " Zm9v", "Zm 9v", "Zm9v!AAA", "Zm9\u{e9}"] {
            assert!(b64_decode(bad).is_err(), "{bad:?}");
        }
        // 末尾多出来的位不是 0（"Zh==" 和 "Zg==" 解出来都是 f，只有后一个是规矩的写法）
        assert!(b64_decode("Zh==").is_err());
        assert!(b64_decode("Zm9=").is_err());
        assert!(b64_nopad_decode("Zh").is_err());
        // 各种长度来回一遍
        let data: Vec<u8> = (0..=255).collect();
        for n in 0..data.len() {
            assert_eq!(b64_decode(&b64(&data[..n])).unwrap(), &data[..n]);
            assert_eq!(b64url_nopad_decode(&b64url_nopad(&data[..n])).unwrap(), &data[..n]);
        }
    }

    #[test]
    fn hex_round_trip() {
        assert_eq!(hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
        assert_eq!(unhex("000fA5Ff").unwrap(), [0x00, 0x0f, 0xa5, 0xff]);
        assert_eq!(unhex("").unwrap(), Vec::<u8>::new());
        for bad in ["0", "0g", "00 11", "+1"] {
            assert!(unhex(bad).is_err(), "{bad}");
        }
    }

    /// 对照值是固定的数据：原来那份独立实现的 code_text（去掉横线）算出来的
    #[test]
    fn crockford() {
        let cases = [
            ("00010203040506070809", "000G40R40M30E209"),
            ("ffffffffffffffffffff", "ZZZZZZZZZZZZZZZZ"),
            ("00000000000000000000", "0000000000000000"),
            ("0123456789abcdef0123", "04HMASW9NF6YY093"),
            ("01", "01"),
            ("ffff", "1ZZZ"),
            ("68656c6c6f", "D1JPRV3F"),
            ("", ""),
        ];
        for (bytes, text) in cases {
            let bytes = unhex(bytes).unwrap();
            assert_eq!(b32c(&bytes), text);
            assert_eq!(b32c_decode(text, bytes.len()).unwrap(), bytes);
        }
        // 人输入的写法：小写、横线、空格、I L O
        assert_eq!(b32c_decode("o4hm-asw9 nf6y-yo93", 10).unwrap(), unhex("0123456789abcdef0123").unwrap());
        assert_eq!(b32c_decode("iLl1", 2), b32c_decode("1111", 2));
        // 字符数不对、字母表以外的字符（U）、放不下
        assert_eq!(b32c_decode("04HMASW9NF6YY09", 10), None);
        assert_eq!(b32c_decode("04HMASW9NF6YY0933", 10), None);
        assert_eq!(b32c_decode("U4HMASW9NF6YY093", 10), None);
        assert_eq!(b32c_decode("ZZZZ", 2), None);
        assert_eq!(b32c_decode("1ZZZ", 2).unwrap(), [0xff, 0xff]);
    }

    #[test]
    fn random_and_compare() {
        let (a, b) = (random(32), random(32));
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert_eq!(random(0).len(), 0);
        let t = token_hex(16);
        assert!(t.len() == 32 && t.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{t}");
        let u = token_urlsafe(16);
        assert!(u.len() == 22 && b64url_nopad_decode(&u).unwrap().len() == 16, "{u}");
        assert!(ct_eq(b"", b""));
        assert!(ct_eq(b"same bytes", b"same bytes"));
        assert!(!ct_eq(b"same bytes", b"same bytez"));
        assert!(!ct_eq(b"short", b"shorter"));
    }
}
