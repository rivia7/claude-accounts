//! 照 Python 的语义写的几样小东西。测试、假命令、求值器大多是从原来的 Python 一行行搬过来的：空白、拆行、真假、repr 要和
//! Python（对照的是 3.14）一个字不差的地方都用这里的，别处不再各写一份。
//!
//! - 空白：[`isspace`]（`str.isspace` 认的字符）、[`strip`]、[`split`]（不带参数的 `str.split()`）、[`strip_bytes`]（`bytes.strip()`）
//! - 拆行：[`splitlines`]、[`splitlines_keep`]（`str.splitlines(True)`）
//! - 真假：[`truthy`]（JSON 读出来的值拿去 `if x`）
//! - 写出来的样子：[`py_str`]（`str(x)`、`'%s' % x`）、[`py_repr`]、[`repr_str`] / [`push_repr_str`]、[`isprintable`]
//!
//! 求值器（tests/tool/ev）有自己的值（元组、集合、视图……），它们的真假和 repr 在它那里；字符串的 repr、空白、拆行用的是这里的。

use crate::json::{float_repr, Value};
use std::sync::OnceLock;

/// `str.isspace()` 认的字符：Unicode 的空白，加上 Python 另算的四个分隔控制符 U+001C～U+001F
/// （就是 Rust 的 `char::is_whitespace` 多这四个）
pub fn isspace(c: char) -> bool {
    matches!(
        c,
        '\t'..='\r'
            | '\x1c'..='\x1f'
            | ' '
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

/// `str.strip()`：去掉两头的空白
pub fn strip(s: &str) -> &str {
    s.trim_matches(isspace)
}

/// `str.split()`（不带参数）：按连着的空白切开，空的不要
pub fn split(s: &str) -> impl Iterator<Item = &str> {
    s.split(isspace).filter(|part| !part.is_empty())
}

/// `bytes.strip()`：去掉两头的空格、\t、\n、\r、\x0b、\x0c（比 `<[u8]>::trim_ascii` 多一个 \x0b）
pub fn strip_bytes(b: &[u8]) -> &[u8] {
    let ws = |c: &u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
    let start = b.iter().position(|c| !ws(c)).unwrap_or(b.len());
    let end = b.iter().rposition(|c| !ws(c)).map_or(start, |i| i + 1);
    &b[start..end]
}

/// `str.splitlines()`：\n、\r、\r\n、\v、\f、\x1c、\x1d、\x1e、U+0085、U+2028、U+2029 都算换行（比读文件时认的多）；
/// 给出的各行不带换行符，最后一行后面的换行不另算一行
pub fn splitlines(s: &str) -> Vec<&str> {
    lines(s, false)
}

/// `str.splitlines(True)`：同上，各行带着自己的换行符
pub fn splitlines_keep(s: &str) -> Vec<&str> {
    lines(s, true)
}

fn lines(s: &str, keep: bool) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if !matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}') {
            continue;
        }
        let mut end = i + c.len_utf8();
        if c == '\r' {
            if let Some((j, '\n')) = chars.peek().copied() {
                chars.next();
                end = j + 1;
            }
        }
        out.push(&s[start..if keep { end } else { i }]);
        start = end;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// `bool(x)`：JSON 读出来的值在 Python 里算不算真（`if x`、`x or y`、`assertTrue(x)`）——null、false、0、0.0、
/// 空的字符串 / 数组 / 对象是假，别的都是真
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Int(n) => *n != 0,
        Value::Float(f) => *f != 0.0,
        Value::Str(s) => !s.is_empty(),
        Value::List(l) => !l.is_empty(),
        Value::Obj(o) => !o.is_empty(),
    }
}

/// `str(x)`（`'%s' % x`、print）：字符串原样，别的和 repr 一样
pub fn py_str(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        other => py_repr(other),
    }
}

/// `repr(x)`，x 是 json.load 读出来的值：None、True、1.0、inf、['a', 1]、{'k': "it's"}
pub fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => float_repr(*f),
        Value::Str(s) => repr_str(s),
        Value::List(l) => format!("[{}]", l.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Obj(o) => {
            format!("{{{}}}", o.iter().map(|(k, v)| format!("{}: {}", repr_str(k), py_repr(v))).collect::<Vec<_>>().join(", "))
        }
    }
}

/// `repr(字符串)`：单引号括着，里面有单引号、没有双引号时改用双引号；反斜杠、括它的那种引号、\t \n \r 写成转义，
/// 不可打印的字符（[`isprintable`] 是假的）写成 \xhh、\uhhhh、\Uhhhhhhhh
pub fn repr_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    push_repr_str(&mut out, s);
    out
}

/// [`repr_str`]，接在 out 后面
pub fn push_repr_str(out: &mut String, s: &str) {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if isprintable(c) => out.push(c),
            c => {
                let n = c as u32;
                out.push_str(&if n < 0x100 {
                    format!("\\x{n:02x}")
                } else if n < 0x10000 {
                    format!("\\u{n:04x}")
                } else {
                    format!("\\U{n:08x}")
                });
            }
        }
    }
    out.push(quote);
}

/// `str.isprintable()` 对一个字符：Unicode 的 Other（控制符、格式符、私用区、没分配的）和 Separator 两大类不可打印，
/// 只有空格（U+0020）除外。没分配的照 regex 这个库带的那一版 Unicode 算
pub fn isprintable(c: char) -> bool {
    let n = c as u32;
    if n < 0x7f {
        return n >= 0x20;
    }
    // 常见的几段直接认，不动用 Unicode 的表（那张表用到才编，编一次要几毫秒）
    let known = matches!(n,
        0xa1..=0xac | 0xae..=0x24f | 0x391..=0x3a1 | 0x3a3..=0x3c9 | 0x410..=0x44f | 0x2010..=0x2027 | 0x2030..=0x205e
        | 0x2190..=0x22ff | 0x2500..=0x257f | 0x25a0..=0x26ff | 0x3001..=0x303f | 0x3041..=0x3096 | 0x30a0..=0x30ff
        | 0x4e00..=0x9fff | 0xac00..=0xd7a3 | 0xff01..=0xff5e);
    if known {
        return true;
    }
    static OTHER: OnceLock<regex::Regex> = OnceLock::new();
    let other = OTHER.get_or_init(|| regex::Regex::new(r"^[\p{C}\p{Z}]$").expect("写死的正则"));
    !other.is_match(c.encode_utf8(&mut [0; 4]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    #[test]
    fn whitespace_like_python() {
        assert!(isspace('\x1c') && isspace('\x1f') && isspace('\u{3000}') && isspace('\u{a0}') && !isspace('\u{200b}'));
        // 就是 Rust 认的空白多 U+001C～U+001F 四个：原来各处有照这个写的，也有一个个列出来的，两种写法是一回事
        for c in (0..=0x10ffffu32).filter_map(char::from_u32) {
            assert_eq!(isspace(c), c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c), "U+{:04X}", c as u32);
        }
        assert_eq!(strip(" \t\x1cx y\u{3000}\n"), "x y");
        assert_eq!(strip("\u{1f} \n"), "");
        assert_eq!(split("  a \t b\x1c c ").collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(split(" \n").count(), 0);
        assert_eq!(strip_bytes(b" \t\x0b\x0cabc d\r\n"), b"abc d");
        assert_eq!(strip_bytes(b" \t\r\n"), b"");
        assert_eq!(strip_bytes(b"x"), b"x");
        // bytes.strip() 不认 \x1c、不认非 ASCII 的空白
        assert_eq!(strip_bytes(b"\x1cx\xa0"), b"\x1cx\xa0");
    }

    #[test]
    fn splitlines_like_python() {
        assert_eq!(splitlines(""), Vec::<&str>::new());
        assert_eq!(splitlines("\n"), [""]);
        assert_eq!(splitlines("\n\n"), ["", ""]);
        assert_eq!(splitlines("\n\nx"), ["", "", "x"]);
        assert_eq!(splitlines("a\nb\n"), ["a", "b"]);
        assert_eq!(splitlines("a\r\nb\rc"), ["a", "b", "c"]);
        assert_eq!(splitlines("a\r\nb\rc\nd\x0be\u{2028}f\n"), ["a", "b", "c", "d", "e", "f"]);
        // U+001F 是空白，不是换行
        assert_eq!(
            splitlines("a\u{0b}b\u{0c}c\u{1c}d\u{1d}e\u{1e}f\u{85}g\u{2028}h\u{2029}i\u{1f}j"),
            ["a", "b", "c", "d", "e", "f", "g", "h", "i\u{1f}j"]
        );
        // Python：'a\nb\r\nc\rd\x0be'.splitlines(True)
        assert_eq!(splitlines_keep("a\nb\r\nc\rd\x0be"), ["a\n", "b\r\n", "c\r", "d\x0b", "e"]);
        assert_eq!(splitlines_keep("\r\n\r"), ["\r\n", "\r"]);
        assert_eq!(splitlines_keep(""), Vec::<&str>::new());
    }

    #[test]
    fn truthy_like_python() {
        for (text, want) in [
            ("null", false),
            ("false", false),
            ("true", true),
            ("0", false),
            ("0.0", false),
            ("-0.0", false),
            ("2", true),
            ("0.5", true),
            ("\"\"", false),
            ("\"0\"", true),
            ("[]", false),
            ("[0]", true),
            ("{}", false),
            ("{\"a\": null}", true),
        ] {
            assert_eq!(truthy(&json::parse(text).unwrap()), want, "{text}");
        }
    }

    #[test]
    fn repr_like_python() {
        assert_eq!(repr_str(""), "''");
        assert_eq!(repr_str("a'b"), "\"a'b\"");
        assert_eq!(repr_str("a\"b"), "'a\"b'");
        assert_eq!(repr_str("a'\"b"), r#"'a\'"b'"#);
        assert_eq!(repr_str("a\\b\tc\nd\re"), r"'a\\b\tc\nd\re'");
        assert_eq!(repr_str("a\nb\x01\x7f"), r"'a\nb\x01\x7f'");
        assert_eq!(repr_str("名字 ¡ é"), "'名字 ¡ é'");
        // Python：repr('\x1b\x85\xa0' + chr(0x2028) + 'x' + chr(0x1F600))
        assert_eq!(repr_str("\x1b\u{85}\u{a0}\u{2028}x\u{1F600}"), concat!(r"'\x1b\x85\xa0\", "u2028x\u{1F600}'"));
        // 软连字符、零宽空格、全角空格、BOM、私用区、增补平面里的格式符：repr('\xad​　﻿\U000e0001')
        assert_eq!(
            repr_str("\u{ad}\u{200b}\u{3000}\u{feff}\u{e000}\u{e0001}"),
            concat!(r"'\xad\", r"u200b\", r"u3000\", r"ufeff\", r"ue000\", "U000e0001'")
        );
        let mut out = String::from("KeyError: ");
        push_repr_str(&mut out, "k");
        assert_eq!(out, "KeyError: 'k'");
        assert!(isprintable(' ') && isprintable('~') && isprintable('中') && isprintable('\u{1F600}'));
        assert!(!isprintable('\x7f') && !isprintable('\u{a0}') && !isprintable('\u{2029}') && !isprintable('\u{10ffff}'));
    }

    #[test]
    fn values_print_like_python() {
        let v = |text: &str| json::parse(text).unwrap();
        for (text, want) in [
            (r#""机房组""#, "机房组"),
            ("true", "True"),
            ("false", "False"),
            ("null", "None"),
            ("7890", "7890"),
            ("1.0", "1.0"),
            ("2.5", "2.5"),
            ("1e16", "1e+16"),
            ("Infinity", "inf"),
            ("-Infinity", "-inf"),
            ("NaN", "nan"),
            ("{}", "{}"),
            ("[]", "[]"),
            (r#"["a", 1, false, {"k": "it's"}]"#, r#"['a', 1, False, {'k': "it's"}]"#),
            (r#"{"a": [1, 2.0, null, true, "x"]}"#, "{'a': [1, 2.0, None, True, 'x']}"),
        ] {
            assert_eq!(py_str(&v(text)), want, "{text}");
        }
        let j = v(r#"{"error": "bad_token", "n": [1, 2.5, null, true, false], "名字": "it's", "q": "a\"b'c\n"}"#);
        assert_eq!(py_repr(&j), r#"{'error': 'bad_token', 'n': [1, 2.5, None, True, False], '名字': "it's", 'q': 'a"b\'c\n'}"#);
        assert_eq!(py_repr(&Value::from("原样")), "'原样'");
        let odd = Value::List(vec![Value::Str("a\nb\u{a0}\u{1}\u{3000}".to_string())]);
        assert_eq!(py_str(&odd), format!("['a{0}nb{0}xa0{0}x01{0}u3000']", '\\'));
    }
}
