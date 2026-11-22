//! 邮件里的字节和文字怎么互相变：字符集、正文的传输编码（quoted-printable、base64、uuencode）、`repr(bytes)`、
//! `raw-unicode-escape`。原来这些是 Python 的 `codecs`、`binascii`、`quopri`、`email._encoded_words` 做的，这里照
//! CPython 3.14 的行为手写，对照数据在 tests/vectors/mail.json（移植时生成的固定数据）。
//!
//! 字符集只认四个：UTF-8、US-ASCII、ISO-8859-1（Latin-1）、Windows-1252，名字的写法（大小写、连字符、别名）照 Python
//! 的规则认。别的字符集（GBK、Big5、ISO-2022-JP、UTF-16……）Python 认、这里不认：一律走原实现里「Python 也不认识这个
//! 字符集」的那条路（`Lookup::Unknown`，调用方照 LookupError 处理）。这是有意和原实现不一样的地方，见 FINDINGS.md。

use crate::py::base64::a2b_base64;
use crate::py::{Fail, R};

/// 认得的四个字符集
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Codec {
    Utf8,
    Ascii,
    Latin1,
    Cp1252,
}

/// `codecs.lookup(名字)` 的三种结果
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lookup {
    Known(Codec),
    /// LookupError（Python 不认识的名字；Python 认识、这里不认识的也算在这里）
    Unknown,
    /// 名字里有 NUL：Python 在查之前就报 ValueError（embedded null character），没有人接
    Null,
}

/// `encodings.aliases.aliases` 里指到这四个字符集的名字
const ALIASES: &[(&str, Codec)] = &[
    ("1252", Codec::Cp1252),
    ("646", Codec::Ascii),
    ("8859", Codec::Latin1),
    ("ansi_x3.4_1968", Codec::Ascii),
    ("ansi_x3.4_1986", Codec::Ascii),
    ("ansi_x3_4_1968", Codec::Ascii),
    ("cp367", Codec::Ascii),
    ("cp65001", Codec::Utf8),
    ("cp819", Codec::Latin1),
    ("csascii", Codec::Ascii),
    ("csisolatin1", Codec::Latin1),
    ("ibm367", Codec::Ascii),
    ("ibm819", Codec::Latin1),
    ("iso646_us", Codec::Ascii),
    ("iso8859", Codec::Latin1),
    ("iso8859_1", Codec::Latin1),
    ("iso_646.irv_1991", Codec::Ascii),
    ("iso_8859_1", Codec::Latin1),
    ("iso_8859_1_1987", Codec::Latin1),
    ("iso_ir_100", Codec::Latin1),
    ("iso_ir_6", Codec::Ascii),
    ("l1", Codec::Latin1),
    ("latin", Codec::Latin1),
    ("latin1", Codec::Latin1),
    ("u8", Codec::Utf8),
    ("us", Codec::Ascii),
    ("us_ascii", Codec::Ascii),
    ("utf", Codec::Utf8),
    ("utf8", Codec::Utf8),
    ("utf8_ucs2", Codec::Utf8),
    ("utf8_ucs4", Codec::Utf8),
    ("windows_1252", Codec::Cp1252),
];

/// `encodings` 包里这四个字符集的模块名（规范化以后的名字直接就是模块名时用）
const MODULES: &[(&str, Codec)] = &[
    ("ascii", Codec::Ascii),
    ("cp1252", Codec::Cp1252),
    ("iso8859_1", Codec::Latin1),
    ("latin_1", Codec::Latin1),
    ("utf_8", Codec::Utf8),
];

/// Windows-1252 的 0x80–0x9f（0 是没定义的那五个）；0xa0 以上和 Latin-1 一样
const CP1252_HIGH: [u16; 32] = [
    0x20ac, 0x0000, 0x201a, 0x0192, 0x201e, 0x2026, 0x2020, 0x2021, 0x02c6, 0x2030, 0x0160, 0x2039, 0x0152, 0x0000, 0x017d, 0x0000,
    0x0000, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014, 0x02dc, 0x2122, 0x0161, 0x203a, 0x0153, 0x0000, 0x017e, 0x0178,
];

/// 字符集的名字规范化（CPython 3.14 的 `_Py_normalize_encoding`，按 UTF-8 的字节看）：ASCII 的字母数字和点留着、转成小写，
/// 别的（包括所有非 ASCII 的字符）当分隔，连着的分隔合成一个下划线，开头结尾的不要
fn normalize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut punct = false;
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b == b'.' {
            if punct && !out.is_empty() {
                out.push('_');
            }
            punct = false;
            out.push(b.to_ascii_lowercase() as char);
        } else {
            punct = true;
        }
    }
    out
}

/// `codecs.lookup(name)`：先按别名找（带点的名字再把点换成下划线找一次），再看是不是模块名（带点的不算）
pub fn lookup(name: &str) -> Lookup {
    if name.contains('\0') {
        return Lookup::Null;
    }
    let norm = normalize(name);
    let alias = |n: &str| ALIASES.iter().find(|(a, _)| *a == n).map(|(_, c)| *c);
    if let Some(c) = alias(&norm).or_else(|| alias(&norm.replace('.', "_"))) {
        return Lookup::Known(c);
    }
    if norm.contains('.') {
        return Lookup::Unknown;
    }
    match MODULES.iter().find(|(m, _)| *m == norm) {
        Some((_, c)) => Lookup::Known(*c),
        None => Lookup::Unknown,
    }
}

impl Codec {
    /// `data.decode(字符集, 'replace')`：解不了的字节换成 U+FFFD
    pub fn decode_replace(self, data: &[u8]) -> String {
        match self {
            Codec::Utf8 => String::from_utf8_lossy(data).into_owned(),
            Codec::Ascii => data.iter().map(|&b| if b < 0x80 { b as char } else { '\u{fffd}' }).collect(),
            Codec::Latin1 => data.iter().map(|&b| b as char).collect(),
            Codec::Cp1252 => data.iter().map(|&b| cp1252(b).unwrap_or('\u{fffd}')).collect(),
        }
    }

    /// `data.decode(字符集)`：有解不了的字节是 UnicodeDecodeError（这里是 None）
    pub fn decode_strict(self, data: &[u8]) -> Option<String> {
        match self {
            Codec::Utf8 => std::str::from_utf8(data).ok().map(str::to_string),
            Codec::Ascii => data.is_ascii().then(|| data.iter().map(|&b| b as char).collect()),
            Codec::Latin1 => Some(data.iter().map(|&b| b as char).collect()),
            Codec::Cp1252 => data.iter().map(|&b| cp1252(b)).collect(),
        }
    }
}

fn cp1252(b: u8) -> Option<char> {
    match b {
        0x80..=0x9f => match CP1252_HIGH[(b - 0x80) as usize] {
            0 => None,
            c => char::from_u32(c as u32),
        },
        _ => Some(b as char),
    }
}

/// `s.encode('raw-unicode-escape')`：256 以下的字符就是那个字节，别的写成 `\uXXXX` / `\UXXXXXXXX`
pub fn raw_unicode_escape(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        let n = c as u32;
        if n < 0x100 {
            out.push(n as u8);
        } else if n < 0x10000 {
            out.extend_from_slice(format!("\\u{n:04x}").as_bytes());
        } else {
            out.extend_from_slice(format!("\\U{n:08x}").as_bytes());
        }
    }
    out
}

/// `data.decode('ascii', 'replace')`
pub fn ascii_replace(data: &[u8]) -> String {
    Codec::Ascii.decode_replace(data)
}

/// `data.decode('latin-1')`
pub fn latin1(data: &[u8]) -> String {
    Codec::Latin1.decode_replace(data)
}

/// `repr(bytes)` / `str(bytes)`：`b'…'`，里面有单引号、没有双引号时用双引号；不可见的字节写成转义
pub fn bytes_repr(data: &[u8]) -> String {
    let quote = if data.contains(&b'\'') && !data.contains(&b'"') { b'"' } else { b'\'' };
    let mut out = String::with_capacity(data.len() + 3);
    out.push('b');
    out.push(quote as char);
    for &b in data {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            _ if b == quote => {
                out.push('\\');
                out.push(b as char);
            }
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push(quote as char);
    out
}

/// `int(文字)` 的位数上限（`sys.int_info.default_max_str_digits`）：十进制的数字超过这么多位是 ValueError
pub const INT_MAX_STR_DIGITS: usize = 4300;

/// 十进制数字太多时 `int()` 的那句 ValueError
pub fn int_too_long(digits: usize) -> Fail {
    Fail::Value(format!(
        "Exceeds the limit ({INT_MAX_STR_DIGITS} digits) for integer string conversion: value has {digits} digits; use sys.set_int_max_str_digits() to increase the limit"
    ))
}

/// `binascii.a2b_qp(data, header=False)`（`quopri.decodestring`）
pub fn a2b_qp(data: &[u8]) -> Vec<u8> {
    let n = data.len();
    let mut out = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        if data[i] != b'=' {
            out.push(data[i]);
            i += 1;
            continue;
        }
        i += 1;
        if i >= n {
            break;
        }
        if data[i] == b'\n' || data[i] == b'\r' {
            // 软换行：等号后面直到换行都不要
            if data[i] != b'\n' {
                while i < n && data[i] != b'\n' {
                    i += 1;
                }
            }
            if i < n {
                i += 1;
            }
        } else if data[i] == b'=' {
            // 写坏了的 quoted-printable：两个等号算一个
            out.push(b'=');
            i += 1;
        } else if i + 1 < n && data[i].is_ascii_hexdigit() && data[i + 1].is_ascii_hexdigit() {
            let hex = |b: u8| (b as char).to_digit(16).unwrap_or(0) as u8;
            out.push((hex(data[i]) << 4) | hex(data[i + 1]));
            i += 2;
        } else {
            out.push(b'=');
        }
    }
    out
}

/// `bytes.splitlines()` 再接起来：`\r\n`、`\r`、`\n` 都去掉
fn join_lines(data: &[u8]) -> Vec<u8> {
    data.iter().copied().filter(|b| !matches!(b, b'\r' | b'\n')).collect()
}

/// `email._encoded_words.decode_b`：先补上填充严格地解，不行再宽松地解，还不行多补两个等号，再不行原样返回
pub fn decode_b(encoded: &[u8]) -> Vec<u8> {
    let pad_err = encoded.len() % 4;
    let mut padded = encoded.to_vec();
    if pad_err != 0 {
        padded.extend(std::iter::repeat_n(b'=', 4 - pad_err));
    }
    if let Ok(v) = a2b_base64(&padded, true) {
        return v;
    }
    if let Ok(v) = a2b_base64(encoded, false) {
        return v;
    }
    let mut more = encoded.to_vec();
    more.extend_from_slice(b"==");
    match a2b_base64(&more, false) {
        Ok(v) => v,
        Err(_) => encoded.to_vec(),
    }
}

/// base64 的正文（`get_payload(decode=True)` 里 Content-Transfer-Encoding 是 base64 的那一支）
pub fn decode_base64_body(payload: &[u8]) -> Vec<u8> {
    decode_b(&join_lines(payload))
}

/// `binascii.a2b_uu(line)`：出错是 binascii.Error（ValueError 的子类），这里给出它的说法
fn a2b_uu(line: &[u8]) -> Result<Vec<u8>, &'static str> {
    // 第一个字节是这一行有多少个字节；空的一行 C 里读到的是结尾的 0（所以是 32 个 0 字节）
    let first = line.first().copied().unwrap_or(0);
    let mut bin_len = (first.wrapping_sub(b' ') & 0o77) as usize;
    let rest = line.get(1..).unwrap_or(&[]);
    let mut out = Vec::with_capacity(bin_len);
    let (mut leftchar, mut leftbits) = (0u32, 0u32);
    let mut i = 0;
    while bin_len > 0 {
        let mut ch = rest.get(i).copied().unwrap_or(0);
        if ch == b'\n' || ch == b'\r' || i >= rest.len() {
            ch = 0; // 行尾的空格被吃掉了：当 0
        } else {
            if !(b' '..=b' ' + 64).contains(&ch) {
                return Err("Illegal char");
            }
            ch = (ch - b' ') & 0o77;
        }
        leftchar = (leftchar << 6) | ch as u32;
        leftbits += 6;
        if leftbits >= 8 {
            leftbits -= 8;
            out.push(((leftchar >> leftbits) & 0xff) as u8);
            leftchar &= (1 << leftbits) - 1;
            bin_len -= 1;
        }
        i += 1;
    }
    // 这一行剩下的只能是空白（有的编码器拿反引号当填充）
    for &ch in rest.get(i..).unwrap_or(&[]) {
        if !matches!(ch, b' ' | b'`' | b'\n' | b'\r') {
            return Err("Trailing garbage");
        }
    }
    Ok(out)
}

/// `int(字节, base=8)` 认不认（只看认不认，不要值）：前后的空白、正负号、`0o` 前缀、数字之间的下划线
fn octal_ok(s: &[u8]) -> bool {
    let is_space = |b: &u8| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
    let start = s.iter().position(|b| !is_space(b)).unwrap_or(s.len());
    let end = s.iter().rposition(|b| !is_space(b)).map_or(start, |e| e + 1);
    let mut s = &s[start..end];
    if let Some(r) = s.strip_prefix(b"+").or_else(|| s.strip_prefix(b"-")) {
        s = r;
    }
    let mut prev_digit = false;
    if let Some(r) = s.strip_prefix(b"0o").or_else(|| s.strip_prefix(b"0O")) {
        s = r;
        if let Some(r) = s.strip_prefix(b"_") {
            s = r; // 前缀后面可以有一个下划线
        }
    }
    let mut any = false;
    for &b in s {
        if b == b'_' {
            if !prev_digit {
                return false;
            }
            prev_digit = false;
        } else if (b'0'..=b'7').contains(&b) {
            prev_digit = true;
            any = true;
        } else {
            return false;
        }
    }
    any && prev_digit
}

/// `email.message._decode_uu`：解不了是 ValueError（这里是 None，调用方原样用没解的正文）
fn decode_uu(encoded: &[u8]) -> Option<Vec<u8>> {
    let mut lines = split_lines(encoded).into_iter();
    let mut found = false;
    for line in lines.by_ref() {
        if let Some(rest) = line.strip_prefix(b"begin ") {
            let mode = rest.split(|b| *b == b' ').next().unwrap_or(&[]);
            if octal_ok(mode) {
                found = true;
                break;
            }
        }
    }
    if !found {
        return None; // `begin` line not found
    }
    let mut out = Vec::new();
    for line in lines {
        if line.is_empty() {
            return None; // Truncated input
        }
        let is_trim = |b: &u8| matches!(b, b' ' | b'\t' | b'\r' | b'\n' | 0x0c);
        let start = line.iter().position(|b| !is_trim(b)).unwrap_or(line.len());
        let end = line.iter().rposition(|b| !is_trim(b)).map_or(start, |e| e + 1);
        if &line[start..end] == b"end" {
            break;
        }
        let decoded = match a2b_uu(line) {
            Ok(d) => d,
            Err(_) => {
                // 有的编码器一行里多写了东西：照开头说的长度截断再解一次
                let nbytes = (((line[0].wrapping_sub(32) & 63) as usize) * 4 + 5) / 3;
                a2b_uu(&line[..nbytes.min(line.len())]).ok()?
            }
        };
        out.extend_from_slice(&decoded);
    }
    Some(out)
}

/// `bytes.splitlines()`：只在 `\n`、`\r`、`\r\n` 处断
fn split_lines(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < data.len() {
        match data[i] {
            b'\n' => {
                out.push(&data[start..i]);
                i += 1;
                start = i;
            }
            b'\r' => {
                out.push(&data[start..i]);
                i += if data.get(i + 1) == Some(&b'\n') { 2 } else { 1 };
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < data.len() {
        out.push(&data[start..]);
    }
    out
}

/// `msg.get_payload(decode=True)` 里按 Content-Transfer-Encoding（已经去空白、转小写）解正文
pub fn decode_transfer(cte: &str, payload: &[u8]) -> Vec<u8> {
    match cte {
        "quoted-printable" => a2b_qp(payload),
        "base64" => decode_base64_body(payload),
        "x-uuencode" | "uuencode" | "uue" | "x-uue" => decode_uu(payload).unwrap_or_else(|| payload.to_vec()),
        _ => payload.to_vec(),
    }
}

/// `data.decode(charset or 'utf-8', 'replace')`，Python 不认识这个字符集（LookupError）时照 UTF-8 解。
/// 名字里有 NUL 是没人接的 ValueError
pub fn decode_body(charset: Option<&str>, data: &[u8]) -> R<String> {
    match lookup(charset.filter(|c| !c.is_empty()).unwrap_or("utf-8")) {
        Lookup::Known(c) => Ok(c.decode_replace(data)),
        Lookup::Unknown => Ok(Codec::Utf8.decode_replace(data)),
        Lookup::Null => Err(Fail::Value("embedded null character".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::mail::testing::{bytes_of, vectors};

    #[test]
    fn charset_names_like_python() {
        let cases = vectors("codec_lookup");
        assert!(cases.len() > 100);
        for c in cases {
            let name = c.at("name").py_str();
            let got = match lookup(&name) {
                Lookup::Known(Codec::Utf8) => "utf-8",
                Lookup::Known(Codec::Ascii) => "ascii",
                Lookup::Known(Codec::Latin1) => "iso8859-1",
                Lookup::Known(Codec::Cp1252) => "cp1252",
                Lookup::Unknown => "LookupError",
                Lookup::Null => "ValueError",
            };
            // Python 认识、这里不认识的字符集（"other"）算不认识：有意不一样的那一处
            let want = c.at("py").py_str();
            assert_eq!(got, if want == "other" { "LookupError" } else { want.as_str() }, "{name:?}");
        }
    }

    #[test]
    fn decoding_like_python() {
        let cases = vectors("codec_decode");
        assert!(cases.len() > 100);
        for c in cases {
            let data = bytes_of(&c.at("data"));
            for (key, codec) in
                [("utf-8", Codec::Utf8), ("ascii", Codec::Ascii), ("latin-1", Codec::Latin1), ("cp1252", Codec::Cp1252)]
            {
                let want = c.at(key).as_dict().unwrap();
                assert_eq!(codec.decode_replace(&data), want.at("replace").py_str(), "{key} {data:?}");
                assert_eq!(
                    codec.decode_strict(&data),
                    want.get("strict").and_then(|s| s.rc_str()).map(|s| s.to_string()),
                    "{key} strict {data:?}"
                );
            }
            assert_eq!(bytes_repr(&data), c.at("repr").py_str());
        }
    }

    #[test]
    fn raw_unicode_escape_like_python() {
        for c in vectors("raw_unicode_escape") {
            assert_eq!(raw_unicode_escape(&c.at("s").py_str()), bytes_of(&c.at("out")));
        }
    }

    #[test]
    fn transfer_encodings_like_python() {
        let cases = vectors("transfer");
        assert!(cases.len() > 200);
        for c in cases {
            let data = bytes_of(&c.at("data"));
            for cte in ["quoted-printable", "base64", "x-uuencode", "8bit"] {
                assert_eq!(decode_transfer(cte, &data), bytes_of(&c.at(cte)), "{cte} {:?}", bytes_repr(&data));
            }
        }
    }

    #[test]
    fn digits_limit() {
        assert!(int_too_long(4301)
            .to_string()
            .starts_with("Exceeds the limit (4300 digits) for integer string conversion: value has 4301 digits"));
        assert!(
            octal_ok(b"644")
                && octal_ok(b" 0o7_7 ")
                && octal_ok(b"+0")
                && !octal_ok(b"8")
                && !octal_ok(b"")
                && !octal_ok(b"7_")
                && !octal_ok(b"0o")
        );
    }
}
