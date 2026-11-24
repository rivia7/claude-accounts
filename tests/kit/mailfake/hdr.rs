//! 邮件头。SEARCH 的 FROM、SUBJECT 比的是解开以后的文字（原来的 hdr_text：email.message_from_bytes 取出头的值，再
//! decode_header + make_header + str() 解开 RFC 2047 的 =?字符集?编码?…?=），FETCH 的 HEADER.FIELDS 只挑整行（header_fields）。
//! 都照 Python 3.14 的 email 包（compat32）一步步写：值里折行的 CRLF 留着、不是 ASCII 的字节变成 U+FFFD、解不开的用原来的值。
//! 字符集只认 utf-8、us-ascii、iso-8859-1（连同 Python 认的别名）和 unknown-8bit；别的（gb2312、iso-2022-jp……）当作解不开、
//! 用原来的值——Python 能解开它们，测试里的邮件用不到。

use super::py;
use std::collections::HashSet;

/// 按 \r\n、\r、\n 切成行，带着行尾（email 的 BufferedSubFile 就是这样切的）
fn lines(raw: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < raw.len() {
        match raw[i] {
            b'\n' => {
                out.push(&raw[start..=i]);
                start = i + 1;
            }
            b'\r' => {
                let end = if raw.get(i + 1) == Some(&b'\n') { i + 2 } else { i + 1 };
                out.push(&raw[start..end]);
                start = end;
                i = end;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    if start < raw.len() {
        out.push(&raw[start..]);
    }
    out
}

/// headerRE：^(From |[\041-\071\073-\176]*:|[\t ])——还是头的一行（不然是正文或者那一行空行）
fn header_line(line: &[u8]) -> bool {
    if line.starts_with(b"From ") || matches!(line.first(), Some(b'\t' | b' ')) {
        return true;
    }
    let n = line.iter().take_while(|b| matches!(b, 0x21..=0x39 | 0x3b..=0x7e)).count();
    line.get(n) == Some(&b':')
}

/// compat32 的 header_source_parse：名字是第一个冒号前面的；值是后面的连同续行，去掉开头的空白和换行、结尾的换行
fn source_parse(src: &[&[u8]]) -> (Vec<u8>, Vec<u8>) {
    let first = src[0];
    let i = first.iter().position(|b| *b == b':').unwrap_or(first.len());
    let mut value: Vec<u8> = first.get(i + 1..).unwrap_or(&[]).to_vec();
    for l in &src[1..] {
        value.extend_from_slice(l);
    }
    let lead = value.iter().take_while(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n')).count();
    let mut v = value.split_off(lead);
    while matches!(v.last(), Some(b'\r' | b'\n')) {
        v.pop();
    }
    (first[..i].to_vec(), v)
}

/// message_from_bytes(raw) 的各个头，(名字, 值)，照原来的先后（feedparser 的 _parsegen 和 _parse_headers）
pub fn headers(raw: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    let hl: Vec<&[u8]> = lines(raw).into_iter().take_while(|l| header_line(l)).collect();
    let mut out = Vec::new();
    let mut last: Vec<&[u8]> = Vec::new();
    for (lineno, line) in hl.iter().enumerate() {
        if matches!(line[0], b' ' | b'\t') {
            // 续行；第一个头之前的续行不算
            if !last.is_empty() {
                last.push(line);
            }
            continue;
        }
        if !last.is_empty() {
            out.push(source_parse(&last));
            last.clear();
        }
        if line.starts_with(b"From ") {
            // 第一行是 unix-from；最后一行像 unix-from 的算正文；中间的不算
            if lineno + 1 == hl.len() && lineno != 0 {
                return out;
            }
            continue;
        }
        if line.first() == Some(&b':') {
            continue; // 没有名字
        }
        last.push(line);
    }
    if !last.is_empty() {
        out.push(source_parse(&last));
    }
    out
}

/// 原来的 hdr_text(raw, name)：这个头（不分大小写）所有的值解开以后用空格连起来
pub fn hdr_text(raw: &[u8], name: &str) -> String {
    let mut vals: Vec<String> = Vec::new();
    for (k, v) in headers(raw) {
        if !k.eq_ignore_ascii_case(name.as_bytes()) {
            continue;
        }
        // str(v)：有 ASCII 以外的字节时是 unknown-8bit 的 Header，每个这样的字节变成 U+FFFD
        let s: String = v.iter().map(|&b| if b < 0x80 { b as char } else { '\u{fffd}' }).collect();
        vals.push(decoded(&s).unwrap_or(s));
    }
    vals.join(" ")
}

// --- decode_header + make_header + str(Header) --------------------------------------------------------------

/// 解码以后的一段：还是文字（没编码的、q 解出来的），或者已经是字节（b 解出来的）
enum Word {
    Text(String),
    Bytes(Vec<u8>),
}

/// 一个字符集（email.charset.Charset 用到的那几样）
#[derive(Clone, Debug)]
struct Charset {
    /// 规整过的名字：比较、拼接都按它
    input: String,
    input_codec: Option<String>,
    output_codec: Option<String>,
}

const ALIASES: &[(&str, &str)] = &[
    ("latin_1", "iso-8859-1"),
    ("latin-1", "iso-8859-1"),
    ("latin_2", "iso-8859-2"),
    ("latin-2", "iso-8859-2"),
    ("latin_3", "iso-8859-3"),
    ("latin-3", "iso-8859-3"),
    ("latin_4", "iso-8859-4"),
    ("latin-4", "iso-8859-4"),
    ("latin_5", "iso-8859-9"),
    ("latin-5", "iso-8859-9"),
    ("latin_6", "iso-8859-10"),
    ("latin-6", "iso-8859-10"),
    ("latin_7", "iso-8859-13"),
    ("latin-7", "iso-8859-13"),
    ("latin_8", "iso-8859-14"),
    ("latin-8", "iso-8859-14"),
    ("latin_9", "iso-8859-15"),
    ("latin-9", "iso-8859-15"),
    ("latin_10", "iso-8859-16"),
    ("latin-10", "iso-8859-16"),
    ("cp949", "ks_c_5601-1987"),
    ("euc_jp", "euc-jp"),
    ("euc_kr", "euc-kr"),
    ("ascii", "us-ascii"),
];

fn alias(name: &str) -> String {
    ALIASES.iter().find(|(k, _)| *k == name).map_or_else(|| name.to_string(), |(_, v)| v.to_string())
}

/// CODEC_MAP.get(name, name)
fn codec_of(name: &str) -> Option<String> {
    match name {
        "gb2312" => Some("eucgb2312_cn".to_string()),
        "big5" => Some("big5_tw".to_string()),
        "us-ascii" => None,
        other => Some(other.to_string()),
    }
}

impl Charset {
    /// Charset(name)：名字不是 ASCII 是 CharsetError（None）
    fn new(name: &str) -> Option<Charset> {
        if !name.is_ascii() {
            return None;
        }
        let input = alias(&name.to_ascii_lowercase());
        let conv = match input.as_str() {
            "euc-jp" | "shift_jis" => "iso-2022-jp".to_string(),
            _ => input.clone(),
        };
        let output = alias(&conv);
        Some(Charset { input_codec: codec_of(&input), output_codec: codec_of(&output), input })
    }
    fn utf8() -> Charset {
        Charset::new("utf-8").unwrap_or_else(|| unreachable!())
    }
    /// 在 Header.__str__ 里算不算 None / us-ascii
    fn ascii_like(cs: Option<&Charset>) -> bool {
        cs.is_none_or(|c| c.input == "us-ascii" || c.input == "none")
    }
}

/// Python 的编解码器名字：小写、标点并成一个下划线（encodings.normalize_encoding），再查别名。只认这三种
#[derive(Clone, Copy, PartialEq, Eq)]
enum Codec {
    Utf8,
    Ascii,
    Latin1,
}

fn codec(name: &str) -> Option<Codec> {
    let mut norm = String::new();
    let mut punct = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '.' {
            if punct && !norm.is_empty() {
                norm.push('_');
            }
            norm.push(c.to_ascii_lowercase());
            punct = false;
        } else {
            punct = true;
        }
    }
    let known = |n: &str| match n {
        "utf_8" | "cp65001" | "u8" | "utf" | "utf8" | "utf8_ucs2" | "utf8_ucs4" => Some(Codec::Utf8),
        "ascii" | "646" | "ansi_x3.4_1968" | "ansi_x3.4_1986" | "ansi_x3_4_1968" | "cp367" | "csascii" | "ibm367" | "iso646_us"
        | "iso_646.irv_1991" | "iso_ir_6" | "us" | "us_ascii" => Some(Codec::Ascii),
        "latin_1" | "8859" | "cp819" | "csisolatin1" | "ibm819" | "iso8859" | "iso8859_1" | "iso_8859_1" | "iso_8859_1_1987"
        | "iso_ir_100" | "l1" | "latin" | "latin1" => Some(Codec::Latin1),
        _ => None,
    };
    known(&norm).or_else(|| known(&norm.replace('.', "_")))
}

/// bytes.decode(name)（严格）：认不出的编解码器、解不开都是 None
fn decode_bytes(name: &str, b: &[u8]) -> Option<String> {
    match codec(name)? {
        Codec::Utf8 => String::from_utf8(b.to_vec()).ok(),
        Codec::Ascii => b.is_ascii().then(|| b.iter().map(|&x| x as char).collect()),
        Codec::Latin1 => Some(b.iter().map(|&x| x as char).collect()),
    }
}

/// s.encode(name) 成不成：Some(能)、Some(不能)、None 是认不出的编解码器
fn encodable(name: &str, s: &str) -> Option<bool> {
    Some(match codec(name)? {
        Codec::Utf8 => true,
        Codec::Ascii => s.is_ascii(),
        Codec::Latin1 => s.chars().all(|c| (c as u32) < 0x100),
    })
}

/// str.encode('raw-unicode-escape')
fn raw_unicode_escape(s: &str) -> Vec<u8> {
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

/// quoprimime.header_decode：_ 是空格，=XX 是那个字节对应的字符
fn q_decode(s: &str) -> String {
    let cs: Vec<char> = s.replace('_', " ").chars().collect();
    let mut out = String::with_capacity(cs.len());
    let mut i = 0;
    while i < cs.len() {
        if cs[i] == '=' && i + 2 < cs.len() {
            let (a, b) = (cs[i + 1], cs[i + 2]);
            if a.is_ascii_hexdigit() && b.is_ascii_hexdigit() {
                let v = (a.to_digit(16).unwrap_or(0) * 16 + b.to_digit(16).unwrap_or(0)) as u8;
                out.push(v as char);
                i += 3;
                continue;
            }
        }
        out.push(cs[i]);
        i += 1;
    }
    out
}

/// binascii.a2b_base64（不严格的那种）：不认识的字符跳过；= 够了就停；剩下零碎的字符是 binascii.Error（None）
fn a2b_base64(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() / 4 * 3);
    let mut quad = 0;
    let mut left = 0u8;
    let mut pads = 0;
    for &c in data {
        if c == b'=' {
            if quad >= 2 {
                pads += 1;
                if quad + pads >= 4 {
                    return Some(out);
                }
            }
            continue;
        }
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        pads = 0;
        match quad {
            0 => {
                quad = 1;
                left = v;
            }
            1 => {
                quad = 2;
                out.push((left << 2) | (v >> 4));
                left = v & 0x0f;
            }
            2 => {
                quad = 3;
                out.push((left << 4) | (v >> 2));
                left = v & 0x03;
            }
            _ => {
                quad = 0;
                out.push((left << 6) | v);
                left = 0;
            }
        }
    }
    (quad == 0).then_some(out)
}

/// str(make_header(decode_header(s)))；None 是途中抛了异常（hdr_text 那时用原来的值）
fn decoded(s: &str) -> Option<String> {
    let ecre = re!(r"=\?([^?]*?)\?([qQbB])\?(.*?)\?=");
    if !ecre.is_match(s) {
        return Some(s.to_string());
    }
    // (文字, 编码, 字符集)：没编码的那段后两项是 None
    let mut words: Vec<(String, Option<String>, Option<String>)> = Vec::new();
    for line in py::splitlines(s) {
        // ecre.split(line)：[没编码的, 字符集, 编码, 编码的内容, 没编码的, …]
        let mut parts: std::collections::VecDeque<String> = std::collections::VecDeque::new();
        let mut at = 0;
        for c in ecre.captures_iter(line) {
            let m = c.get(0).map_or(0..0, |m| m.range());
            parts.push_back(line[at..m.start].to_string());
            for g in 1..=3 {
                parts.push_back(c.get(g).map_or("", |x| x.as_str()).to_string());
            }
            at = m.end;
        }
        parts.push_back(line[at..].to_string());
        let mut first = true;
        while let Some(mut unencoded) = parts.pop_front() {
            if first {
                unencoded = py::lstrip(&unencoded).to_string();
                first = false;
            }
            if !unencoded.is_empty() {
                words.push((unencoded, None, None));
            }
            if !parts.is_empty() {
                let charset = parts.pop_front().unwrap_or_default().to_lowercase();
                let encoding = parts.pop_front().unwrap_or_default().to_lowercase();
                let encoded = parts.pop_front().unwrap_or_default();
                words.push((encoded, Some(encoding), Some(charset)));
            }
        }
    }
    // 两个编码的词中间只有空白的那一段去掉
    let mut drop = Vec::new();
    for n in 2..words.len() {
        if words[n].1.is_some() && words[n - 2].1.is_some() && py::all_space(&words[n - 1].0) {
            drop.push(n - 1);
        }
    }
    for d in drop.into_iter().rev() {
        words.remove(d);
    }
    let mut dec: Vec<(Word, Option<String>)> = Vec::new();
    for (text, encoding, charset) in words {
        match encoding.as_deref() {
            None => dec.push((Word::Text(text), charset)),
            Some("q") => dec.push((Word::Text(q_decode(&text)), charset)),
            Some("b") => {
                let mut t = text;
                let pad = t.chars().count() % 4;
                if pad != 0 {
                    t.push_str(&"==="[..4 - pad]);
                }
                let b = if t.is_empty() { Vec::new() } else { a2b_base64(&raw_unicode_escape(&t))? };
                dec.push((Word::Bytes(b), charset));
            }
            Some(_) => return None,
        }
    }
    // 先后相同字符集的并起来（没编码的中间加空格）
    let mut collapsed: Vec<(Vec<u8>, Option<String>)> = Vec::new();
    for (w, cs) in dec {
        let b = match w {
            Word::Text(t) => raw_unicode_escape(&t),
            Word::Bytes(b) => b,
        };
        match collapsed.last_mut() {
            Some((lw, lcs)) if *lcs == cs => {
                if cs.is_none() {
                    lw.push(b' ');
                }
                lw.extend_from_slice(&b);
            }
            _ => collapsed.push((b, cs)),
        }
    }
    if collapsed.is_empty() {
        return None; // make_header 里 None.decode：AttributeError
    }
    // make_header：每段按字符集解成文字
    let usascii = Charset::new("us-ascii")?;
    let mut chunks: Vec<(String, Charset)> = Vec::new();
    for (b, cs) in collapsed {
        let mut charset = match cs {
            None => usascii.clone(),
            Some(name) => Charset::new(&name)?,
        };
        let input = charset.input_codec.clone().unwrap_or_else(|| "us-ascii".to_string());
        let text = if input == "unknown-8bit" {
            // surrogateescape：str() 的时候这些字节都变成 U+FFFD，这里直接换
            b.iter().map(|&x| if x < 0x80 { x as char } else { '\u{fffd}' }).collect()
        } else {
            decode_bytes(&input, &b)?
        };
        let output = charset.output_codec.clone().unwrap_or_else(|| "us-ascii".to_string());
        if output != "unknown-8bit" && !encodable(&output, &text)? {
            if output != "us-ascii" {
                return None;
            }
            charset = Charset::utf8();
        }
        chunks.push((text, charset));
    }
    // Header._normalize：先后相同字符集的用空格并起来
    let mut norm: Vec<(String, Charset)> = Vec::new();
    for (t, cs) in chunks {
        match norm.last_mut() {
            Some((lt, lcs)) if lcs.input == cs.input => {
                lt.push(' ');
                lt.push_str(&t);
            }
            _ => norm.push((t, cs)),
        }
    }
    // Header.__str__：编码的和没编码的交界处补一个空格（已经有空白、括号、反斜杠的不补）
    let nonctext = |c: char| py::isspace(c) || matches!(c, '(' | ')' | '\\');
    let mut out = String::new();
    let mut lastcs: Option<Charset> = None;
    let mut lastspace = false;
    for (i, (t, cs)) in norm.into_iter().enumerate() {
        let mut nextcs = Some(cs);
        if i > 0 {
            let hasspace = t.chars().next().is_some_and(nonctext);
            if !Charset::ascii_like(lastcs.as_ref()) {
                if Charset::ascii_like(nextcs.as_ref()) && !hasspace {
                    out.push(' ');
                    nextcs = None;
                }
            } else if !Charset::ascii_like(nextcs.as_ref()) && !lastspace {
                out.push(' ');
            }
        }
        lastspace = t.chars().last().is_some_and(nonctext);
        lastcs = nextcs;
        out.push_str(&t);
    }
    Some(out)
}

/// 原来的 header_fields(raw, names)：头里名字在 names 里（大写）的那几行连同续行，后面接一个空行
pub fn header_fields(raw: &[u8], names: &HashSet<String>) -> Vec<u8> {
    let head = match raw.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(i) => &raw[..i],
        None => raw,
    };
    let mut out: Vec<&[u8]> = Vec::new();
    let mut keep = false;
    let mut rest = head;
    loop {
        let (line, next) = match rest.windows(2).position(|w| w == b"\r\n") {
            Some(i) => (&rest[..i], Some(&rest[i + 2..])),
            None => (rest, None),
        };
        if matches!(line.first(), Some(b' ' | b'\t')) {
            if keep {
                out.push(line);
            }
        } else {
            let name = line.split(|b| *b == b':').next().unwrap_or(&[]);
            // bytes.strip()：去掉的空白比 trim_ascii 多一个 \x0b
            let ws = |b: &u8| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
            let start = name.iter().position(|b| !ws(b)).unwrap_or(name.len());
            let end = name.iter().rposition(|b| !ws(b)).map_or(start, |e| e + 1);
            let name = name[start..end].to_ascii_uppercase();
            keep = names.contains(&name.iter().map(|&b| b as char).collect::<String>());
            if keep {
                out.push(line);
            }
        }
        match next {
            Some(n) => rest = n,
            None => break,
        }
    }
    let mut v = out.join(&b"\r\n"[..]);
    v.extend_from_slice(b"\r\n\r\n");
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folded_and_eight_bit_values() {
        let raw = b"From x\r\nSubject: Your secure\r\n link\r\nFrom: a\xc3\xa9 <a@example.com>\r\nsubject: two\r\n\r\nbody: no\r\n";
        assert_eq!(hdr_text(raw, "Subject"), "Your secure\r\n link two");
        assert_eq!(hdr_text(raw, "From"), "a\u{fffd}\u{fffd} <a@example.com>");
        assert_eq!(hdr_text(raw, "Body"), "");
    }

    #[test]
    fn encoded_words() {
        let raw = b"Subject: =?utf-8?b?5L2g5aW9?= world =?ISO-8859-1?Q?caf=E9?= =?utf-8?q?_x?=\r\n\r\n";
        assert_eq!(hdr_text(raw, "subject"), "\u{4f60}\u{597d} world caf\u{e9} x");
        // 解不开的（base64 剩一个字符、认不出的字符集）用原来的值
        let raw = b"Subject: =?utf-8?b?QUJDDE?= =?x-unknown?q?a?=\r\n\r\n";
        assert_eq!(hdr_text(raw, "Subject"), "=?utf-8?b?QUJDDE?= =?x-unknown?q?a?=");
    }

    #[test]
    fn fields() {
        let raw = b"From: a\r\n b\r\nTo: c\r\nSubject : d\r\n\r\nFrom: body\r\n";
        let names: HashSet<String> = ["FROM", "SUBJECT"].iter().map(|s| s.to_string()).collect();
        assert_eq!(header_fields(raw, &names), b"From: a\r\n b\r\nSubject : d\r\n\r\n".to_vec());
    }
}
