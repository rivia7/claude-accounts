//! JSON：读和写都和 CPython 的 `json` 模块逐字节一致。
//!
//! 一致是硬要求：账号表、config.json 的写法进了路由器的同步摘要和回归的固定对照；`.claude.json`、`settings.json`
//! 这些别人的文件由 cca 整份重写，数字的写法、键的顺序都不能变。所以不用现成的 JSON 库（小数的写法、
//! `ensure_ascii`、分隔符、出错的说法都对不上）。
//!
//! 和 Python 不一样的只有一处：字符串里落单的代理项（`"\ud83d"`，不成对的半个表情）Rust 的字符串放不下，读成 U+FFFD。
//! Python 读得进来，但带着它用 `ensure_ascii=False` 写文件时会直接出错退出。

use super::num::float_repr;
use super::obj::{Dict, List, V};
use std::fmt::Write as _;
use std::rc::Rc;

/// 解析出错：`str(e)` 和 CPython 3.14 的 JSONDecodeError 一样（「说明: line 行 column 列 (char 位置)」）
#[derive(Debug, Clone, PartialEq)]
pub struct JsonError(pub String);

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// `json.loads(文字)`
pub fn loads(s: &str) -> Result<V, JsonError> {
    let mut p = Parser { s, b: s.as_bytes(), i: 0, depth: 0 };
    if s.starts_with('\u{feff}') {
        return Err(p.err("Unexpected UTF-8 BOM (decode using utf-8-sig)", 0));
    }
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i != p.b.len() {
        return Err(p.err("Extra data", p.i));
    }
    Ok(v)
}

/// 文件的内容（字节）照 `open(path, encoding='utf-8')` 读成文字再解析：不是 UTF-8 的报 UnicodeDecodeError 那句话，
/// `\r\n`、单独的 `\r` 先换成 `\n`（文本模式的通用换行）
pub fn loads_file_bytes(raw: &[u8]) -> Result<V, JsonError> {
    let text = match std::str::from_utf8(raw) {
        Ok(t) => t,
        Err(e) => return Err(JsonError(utf8_error_text(raw, &e))),
    };
    if text.contains('\r') {
        loads(&text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        loads(text)
    }
}

/// `str(UnicodeDecodeError)`：不是 UTF-8 的那几个字节在哪、为什么不对
pub fn utf8_error_text(raw: &[u8], e: &std::str::Utf8Error) -> String {
    let pos = e.valid_up_to();
    let (n, why) = match e.error_len() {
        None => (raw.len() - pos, "unexpected end of data"),
        // 能当多字节字符开头的字节（0xC2–0xF4）后面跟的不对；别的字节本身就不能开头
        Some(n) if (0xc2..=0xf4).contains(&raw[pos]) => (n, "invalid continuation byte"),
        Some(n) => (n, "invalid start byte"),
    };
    if n == 1 {
        format!("'utf-8' codec can't decode byte 0x{:02x} in position {}: {}", raw[pos], pos, why)
    } else {
        format!("'utf-8' codec can't decode bytes in position {}-{}: {}", pos, pos + n - 1, why)
    }
}

/// 对象、数组最多套这么多层。Python 的解析器套到一千层上下是 RecursionError（不是 ValueError，多数地方没人接）；这里
/// 当作写错了的 JSON（读配置的地方当作读不了，读会话记录的地方跳过这一行），也免得很深的输入把栈用完。正常的文件到不了几十层
const MAX_DEPTH: usize = 512;

struct Parser<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
    depth: usize,
}

impl Parser<'_> {
    /// 出错的位置按字符数（不是字节数）说，和 Python 一样
    fn err(&self, msg: &str, at: usize) -> JsonError {
        let at = at.min(self.b.len());
        let head = &self.s[..at];
        let pos = head.chars().count();
        let line = head.matches('\n').count() + 1;
        let col = match head.rfind('\n') {
            Some(nl) => head[nl + 1..].chars().count() + 1,
            None => pos + 1,
        };
        JsonError(format!("{msg}: line {line} column {col} (char {pos})"))
    }

    fn ws(&mut self) {
        while let Some(c) = self.b.get(self.i) {
            if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
                self.i += 1;
            } else {
                break;
            }
        }
    }

    fn eat(&mut self, word: &str) -> bool {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> Result<V, JsonError> {
        let start = self.i;
        let nothing = |p: &Parser| Err(p.err("Expecting value", start));
        match self.b.get(self.i) {
            None => nothing(self),
            Some(b'"') => {
                self.i += 1;
                Ok(V::Str(Rc::from(self.string(start)?)))
            }
            Some(b'{') | Some(b'[') => {
                if self.depth >= MAX_DEPTH {
                    return Err(self.err("Too deeply nested", start));
                }
                self.depth += 1;
                self.i += 1;
                let v = if self.b[start] == b'{' { self.object() } else { self.array() };
                self.depth -= 1;
                v
            }
            Some(b'n') if self.eat("null") => Ok(V::None),
            Some(b't') if self.eat("true") => Ok(V::Bool(true)),
            Some(b'f') if self.eat("false") => Ok(V::Bool(false)),
            Some(b'N') if self.eat("NaN") => Ok(V::Float(f64::NAN)),
            Some(b'I') if self.eat("Infinity") => Ok(V::Float(f64::INFINITY)),
            Some(b'-') if self.eat("-Infinity") => Ok(V::Float(f64::NEG_INFINITY)),
            Some(_) => match self.number() {
                Some(v) => Ok(v),
                None => nothing(self),
            },
        }
    }

    /// `-?(0|[1-9]\d*)(\.\d+)?([eE][-+]?\d+)?`：小数点、e 后面没有数字时数字就到它前面为止
    fn number(&mut self) -> Option<V> {
        let (b, start) = (self.b, self.i);
        let mut i = start;
        if b.get(i) == Some(&b'-') {
            i += 1;
        }
        match b.get(i) {
            Some(b'0') => i += 1,
            Some(b'1'..=b'9') => {
                while matches!(b.get(i), Some(b'0'..=b'9')) {
                    i += 1;
                }
            }
            _ => return None,
        }
        let mut float = false;
        if b.get(i) == Some(&b'.') && matches!(b.get(i + 1), Some(b'0'..=b'9')) {
            float = true;
            i += 2;
            while matches!(b.get(i), Some(b'0'..=b'9')) {
                i += 1;
            }
        }
        if matches!(b.get(i), Some(b'e' | b'E')) {
            let mut j = i + 1;
            if matches!(b.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            if matches!(b.get(j), Some(b'0'..=b'9')) {
                float = true;
                while matches!(b.get(j), Some(b'0'..=b'9')) {
                    j += 1;
                }
                i = j;
            }
        }
        let text = &self.s[start..i];
        self.i = i;
        Some(if float {
            V::Float(text.parse().expect("照语法认出来的小数"))
        } else {
            match text.parse::<i64>() {
                Ok(n) => V::Int(n),
                Err(_) => V::Big(Rc::from(text)),
            }
        })
    }

    /// 进来时已经越过开头的引号；begin 是那个引号的位置
    fn string(&mut self, begin: usize) -> Result<String, JsonError> {
        let b = self.b;
        let mut out = String::new();
        let mut run = self.i; // 还没抄进 out 的那一段从哪开始
        loop {
            let Some(&c) = b.get(self.i) else {
                return Err(self.err("Unterminated string starting at", begin));
            };
            match c {
                b'"' => {
                    out.push_str(&self.s[run..self.i]);
                    self.i += 1;
                    return Ok(out);
                }
                b'\\' => {
                    out.push_str(&self.s[run..self.i]);
                    let Some(&e) = b.get(self.i + 1) else {
                        return Err(self.err("Unterminated string starting at", begin));
                    };
                    let simple = match e {
                        b'"' => Some('"'),
                        b'\\' => Some('\\'),
                        b'/' => Some('/'),
                        b'b' => Some('\u{8}'),
                        b'f' => Some('\u{c}'),
                        b'n' => Some('\n'),
                        b'r' => Some('\r'),
                        b't' => Some('\t'),
                        _ => None,
                    };
                    if let Some(ch) = simple {
                        out.push(ch);
                        self.i += 2;
                    } else if e == b'u' {
                        let u = self.hex4(self.i + 2, self.i + 1)?;
                        self.i += 6;
                        let mut cp = u;
                        if (0xd800..0xdc00).contains(&u) && b[self.i..].starts_with(b"\\u") {
                            // 后面紧跟着低位代理项才合成一个字符；不是的话后一个照常另算
                            if let Ok(lo) = self.hex4(self.i + 2, self.i + 1) {
                                if (0xdc00..0xe000).contains(&lo) {
                                    cp = 0x10000 + ((u - 0xd800) << 10) + (lo - 0xdc00);
                                    self.i += 6;
                                }
                            }
                        }
                        out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                    } else {
                        return Err(self.err("Invalid \\escape", self.i));
                    }
                    run = self.i;
                }
                0..=0x1f => return Err(self.err("Invalid control character at", self.i)),
                _ => self.i += 1,
            }
        }
    }

    /// `\u` 后面的四位十六进制（at 是第一位的位置，u_at 是那个 u 的位置）。CPython 还要求四位后面至少再有一个字符
    fn hex4(&self, at: usize, u_at: usize) -> Result<u32, JsonError> {
        let bad = || self.err("Invalid \\uXXXX escape", u_at);
        if at + 4 >= self.b.len() {
            return Err(bad());
        }
        let mut n = 0u32;
        for &d in &self.b[at..at + 4] {
            n = n * 16 + (d as char).to_digit(16).ok_or_else(bad)?;
        }
        Ok(n)
    }

    fn object(&mut self) -> Result<V, JsonError> {
        let d = Dict::new();
        self.ws();
        if self.b.get(self.i) == Some(&b'}') {
            self.i += 1;
            return Ok(V::Dict(d));
        }
        loop {
            if self.b.get(self.i) != Some(&b'"') {
                return Err(self.err("Expecting property name enclosed in double quotes", self.i));
            }
            let begin = self.i;
            self.i += 1;
            let key = self.string(begin)?;
            self.ws();
            if self.b.get(self.i) != Some(&b':') {
                return Err(self.err("Expecting ':' delimiter", self.i));
            }
            self.i += 1;
            self.ws();
            let v = self.value()?;
            d.set(key, v); // 重复的键：值是后写的，位置是先写的那个
            self.ws();
            match self.b.get(self.i) {
                Some(b'}') => {
                    self.i += 1;
                    return Ok(V::Dict(d));
                }
                Some(b',') => {
                    let comma = self.i;
                    self.i += 1;
                    self.ws();
                    if self.b.get(self.i) == Some(&b'}') {
                        return Err(self.err("Illegal trailing comma before end of object", comma));
                    }
                }
                _ => return Err(self.err("Expecting ',' delimiter", self.i)),
            }
        }
    }

    fn array(&mut self) -> Result<V, JsonError> {
        let mut items = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b']') {
            self.i += 1;
            return Ok(V::List(List::from_vec(items)));
        }
        loop {
            items.push(self.value()?);
            self.ws();
            match self.b.get(self.i) {
                Some(b']') => {
                    self.i += 1;
                    return Ok(V::List(List::from_vec(items)));
                }
                Some(b',') => {
                    let comma = self.i;
                    self.i += 1;
                    self.ws();
                    if self.b.get(self.i) == Some(&b']') {
                        return Err(self.err("Illegal trailing comma before end of array", comma));
                    }
                }
                _ => return Err(self.err("Expecting ',' delimiter", self.i)),
            }
        }
    }
}

/// `json.dumps` 的参数。默认值和 Python 一样：`ensure_ascii=True`，一行，分隔符 `", "` 和 `": "`
#[derive(Clone, Copy)]
pub struct Fmt {
    indent: Option<usize>,
    ensure_ascii: bool,
    seps: Option<(&'static str, &'static str)>,
    sort_keys: bool,
}

impl Default for Fmt {
    fn default() -> Self {
        Fmt::new()
    }
}

impl Fmt {
    /// `json.dumps(x)`
    pub const fn new() -> Fmt {
        Fmt { indent: None, ensure_ascii: true, seps: None, sort_keys: false }
    }

    /// `ensure_ascii=False`：中文原样写，不写成 `\uXXXX`
    pub const fn utf8(mut self) -> Fmt {
        self.ensure_ascii = false;
        self
    }

    /// `indent=n`（这时项之间的分隔符是 `,` 加换行）
    pub const fn indent(mut self, n: usize) -> Fmt {
        self.indent = Some(n);
        self
    }

    /// `separators=(',', ':')`
    pub const fn compact(mut self) -> Fmt {
        self.seps = Some((",", ":"));
        self
    }

    /// `sort_keys=True`（按码点排，和 UTF-8 的字节序一样）
    pub const fn sort_keys(mut self) -> Fmt {
        self.sort_keys = true;
        self
    }
}

/// 存文件用的写法（save_json）：`indent=2, ensure_ascii=False`
pub const FILE: Fmt = Fmt::new().utf8().indent(2);
/// 一行、中文原样：`ensure_ascii=False`
pub const LINE: Fmt = Fmt::new().utf8();

/// `json.dumps(v, …)`
pub fn dumps(v: &V, f: Fmt) -> String {
    let (item, key) = f.seps.unwrap_or(if f.indent.is_some() { (",", ": ") } else { (", ", ": ") });
    let mut out = String::new();
    Writer { f, item, key, out: &mut out }.value(v, 0);
    out
}

struct Writer<'a> {
    f: Fmt,
    item: &'static str,
    key: &'static str,
    out: &'a mut String,
}

impl Writer<'_> {
    fn newline(&mut self, level: usize) {
        if let Some(n) = self.f.indent {
            self.out.push('\n');
            for _ in 0..n * level {
                self.out.push(' ');
            }
        }
    }

    fn value(&mut self, v: &V, level: usize) {
        match v {
            V::None => self.out.push_str("null"),
            V::Bool(true) => self.out.push_str("true"),
            V::Bool(false) => self.out.push_str("false"),
            V::Int(i) => {
                let _ = write!(self.out, "{i}");
            }
            V::Big(s) => self.out.push_str(s),
            V::Float(x) => {
                if x.is_nan() {
                    self.out.push_str("NaN");
                } else if x.is_infinite() {
                    self.out.push_str(if *x > 0.0 { "Infinity" } else { "-Infinity" });
                } else {
                    self.out.push_str(&float_repr(*x));
                }
            }
            V::Str(s) => write_str(self.out, s, self.f.ensure_ascii),
            V::List(l) => {
                let items = l.items();
                if items.is_empty() {
                    self.out.push_str("[]");
                    return;
                }
                self.out.push('[');
                for (n, x) in items.iter().enumerate() {
                    if n > 0 {
                        self.out.push_str(self.item);
                    }
                    self.newline(level + 1);
                    self.value(x, level + 1);
                }
                self.newline(level);
                self.out.push(']');
            }
            V::Dict(d) => {
                let mut items = d.items();
                if items.is_empty() {
                    self.out.push_str("{}");
                    return;
                }
                if self.f.sort_keys {
                    items.sort_by(|a, b| a.0.cmp(&b.0));
                }
                self.out.push('{');
                for (n, (k, x)) in items.iter().enumerate() {
                    if n > 0 {
                        self.out.push_str(self.item);
                    }
                    self.newline(level + 1);
                    write_str(self.out, k, self.f.ensure_ascii);
                    self.out.push_str(self.key);
                    self.value(x, level + 1);
                }
                self.newline(level);
                self.out.push('}');
            }
        }
    }
}

/// 一个 JSON 字符串。`"`、`\`、控制字符总要转义；`ensure_ascii` 时空格到 `~` 以外的都写成小写的 `\uXXXX`
/// （0x7f 也算，BMP 以外拆成一对），不带 `ensure_ascii` 时 0x7f 原样写
pub fn write_str(out: &mut String, s: &str, ensure_ascii: bool) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c if ensure_ascii && (c as u32) > 0x7e => {
                let mut pair = [0u16; 2];
                for u in c.encode_utf16(&mut pair) {
                    let _ = write!(out, "\\u{:04x}", u);
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// 一个 JSON 字符串（`json.dumps(文字, ensure_ascii=…)`）
pub fn dump_str(s: &str, ensure_ascii: bool) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    write_str(&mut out, s, ensure_ascii);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dict, list};

    fn rt(text: &str, f: Fmt) -> String {
        dumps(&loads(text).unwrap(), f)
    }

    #[test]
    fn formats_match_python() {
        // 每一行右边都是 CPython 的输出
        let src = r#"{"b": [1, 2.0, "中", null, true], "a": {"k": "v\n\"\\\u0001"}, "e": [], "o": {}}"#;
        assert_eq!(rt(src, Fmt::new()), r#"{"b": [1, 2.0, "\u4e2d", null, true], "a": {"k": "v\n\"\\\u0001"}, "e": [], "o": {}}"#);
        assert_eq!(rt(src, LINE), r#"{"b": [1, 2.0, "中", null, true], "a": {"k": "v\n\"\\\u0001"}, "e": [], "o": {}}"#);
        assert_eq!(rt(src, Fmt::new().utf8().compact()), r#"{"b":[1,2.0,"中",null,true],"a":{"k":"v\n\"\\\u0001"},"e":[],"o":{}}"#);
        assert_eq!(
            rt(src, FILE),
            "{\n  \"b\": [\n    1,\n    2.0,\n    \"中\",\n    null,\n    true\n  ],\n  \"a\": {\n    \"k\": \"v\\n\\\"\\\\\\u0001\"\n  },\n  \"e\": [],\n  \"o\": {}\n}"
        );
        assert_eq!(
            rt(src, Fmt::new().utf8().sort_keys()),
            r#"{"a": {"k": "v\n\"\\\u0001"}, "b": [1, 2.0, "中", null, true], "e": [], "o": {}}"#
        );
        assert_eq!(rt(r#"{"x": [{"y": 1}]}"#, Fmt::new().utf8().indent(1)), "{\n \"x\": [\n  {\n   \"y\": 1\n  }\n ]\n}");
    }

    #[test]
    fn numbers_roundtrip_like_python() {
        assert_eq!(rt("[1e3, 1.50, -0, 0.1, 1E-7, 12345678901234567890, 1e999, -1e999, 10000000000000000.0]", Fmt::new()),
                   "[1000.0, 1.5, 0, 0.1, 1e-07, 12345678901234567890, Infinity, -Infinity, 1e+16]");
        assert_eq!(rt("[NaN, Infinity, -Infinity, -0.0, 1731999999123]", Fmt::new()), "[NaN, Infinity, -Infinity, -0.0, 1731999999123]");
    }

    #[test]
    fn strings() {
        assert_eq!(rt(r#""\ud83d\ude00 \u00e9 \/ \u007f""#, Fmt::new()), r#""\ud83d\ude00 \u00e9 / \u007f""#);
        assert_eq!(rt(r#""\ud83d\ude00 \u007f""#, LINE), "\"\u{1f600} \u{7f}\"");
        assert_eq!(dump_str("a\u{8}\u{c}\u{1f}", false), r#""a\b\f\u001f""#);
    }

    #[test]
    fn duplicate_keys_keep_first_position_last_value() {
        assert_eq!(rt(r#"{"a": 1, "b": 2, "a": 3}"#, Fmt::new()), r#"{"a": 3, "b": 2}"#);
    }

    #[test]
    fn nesting_has_a_limit() {
        let nested = |n: usize| format!("{}1{}", "[".repeat(n), "]".repeat(n));
        assert!(loads(&nested(MAX_DEPTH)).is_ok());
        assert!(loads(&nested(MAX_DEPTH + 1)).unwrap_err().0.starts_with("Too deeply nested"));
        // 很深的输入不会把栈用完；并排的不算深
        assert!(loads(&nested(200_000)).is_err());
        assert!(loads(&format!("[{}1]", "[{\"a\":[]}],".repeat(5000))).is_ok());
    }

    #[test]
    fn errors_match_python() {
        // 右边是 CPython 3.14 的 str(JSONDecodeError)（本机实测）
        for (text, want) in [
            ("", "Expecting value: line 1 column 1 (char 0)"),
            ("  ", "Expecting value: line 1 column 3 (char 2)"),
            ("{", "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"),
            ("{\"a\"}", "Expecting ':' delimiter: line 1 column 5 (char 4)"),
            ("{\"a\": 1 \"b\"}", "Expecting ',' delimiter: line 1 column 9 (char 8)"),
            ("{\"a\": 1,}", "Illegal trailing comma before end of object: line 1 column 8 (char 7)"),
            ("[1,]", "Illegal trailing comma before end of array: line 1 column 3 (char 2)"),
            ("[1,,2]", "Expecting value: line 1 column 4 (char 3)"),
            ("{\"a\":}", "Expecting value: line 1 column 6 (char 5)"),
            ("{,}", "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"),
            ("[", "Expecting value: line 1 column 2 (char 1)"),
            ("[1", "Expecting ',' delimiter: line 1 column 3 (char 2)"),
            ("{\"a\":1", "Expecting ',' delimiter: line 1 column 7 (char 6)"),
            ("\"abc\\", "Unterminated string starting at: line 1 column 1 (char 0)"),
            ("\"\\u123", "Invalid \\uXXXX escape: line 1 column 3 (char 2)"),
            ("\"\\ud83d\\u12\"", "Invalid \\uXXXX escape: line 1 column 9 (char 8)"),
            ("[1 2]", "Expecting ',' delimiter: line 1 column 4 (char 3)"),
            ("\"abc", "Unterminated string starting at: line 1 column 1 (char 0)"),
            ("\"a\tb\"", "Invalid control character at: line 1 column 3 (char 2)"),
            ("\"a\\xb\"", "Invalid \\escape: line 1 column 3 (char 2)"),
            ("\"\\u12\"", "Invalid \\uXXXX escape: line 1 column 3 (char 2)"),
            ("{} x", "Extra data: line 1 column 4 (char 3)"),
            ("{\n  \"中文\": tru\n}", "Expecting value: line 2 column 9 (char 10)"),
            ("nul", "Expecting value: line 1 column 1 (char 0)"),
            ("-", "Expecting value: line 1 column 1 (char 0)"),
            ("\u{feff}{}", "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)"),
        ] {
            assert_eq!(loads(text).unwrap_err().0, want, "{text:?}");
        }
    }

    #[test]
    fn number_stops_before_dangling_dot_or_exponent() {
        assert_eq!(loads("1.").unwrap_err().0, "Extra data: line 1 column 2 (char 1)");
        assert_eq!(loads("1e").unwrap_err().0, "Extra data: line 1 column 2 (char 1)");
        assert_eq!(loads("01").unwrap_err().0, "Extra data: line 1 column 2 (char 1)");
    }

    #[test]
    fn file_bytes() {
        assert_eq!(loads_file_bytes(b"\xff").unwrap_err().0, "'utf-8' codec can't decode byte 0xff in position 0: invalid start byte");
        assert_eq!(loads_file_bytes(b"\"\xe4\xb8\"").unwrap_err().0, "'utf-8' codec can't decode bytes in position 1-2: invalid continuation byte");
        assert_eq!(loads_file_bytes(b"\"\xe4\xb8").unwrap_err().0, "'utf-8' codec can't decode bytes in position 1-2: unexpected end of data");
        assert_eq!(loads_file_bytes(b"\xe4").unwrap_err().0, "'utf-8' codec can't decode byte 0xe4 in position 0: unexpected end of data");
        assert_eq!(loads_file_bytes(b"\xc0\x80").unwrap_err().0, "'utf-8' codec can't decode byte 0xc0 in position 0: invalid start byte");
        assert_eq!(loads_file_bytes(b"\xed\xa0\x80").unwrap_err().0, "'utf-8' codec can't decode byte 0xed in position 0: invalid continuation byte");
        assert_eq!(dumps(&loads_file_bytes(b"{\r\n\"a\":\r1}").unwrap(), Fmt::new()), r#"{"a": 1}"#);
        assert_eq!(loads_file_bytes(b"[\r\n x").unwrap_err().0, "Expecting value: line 2 column 2 (char 3)");
    }

    #[test]
    fn built_values() {
        let v = V::Dict(dict! { "n" => 1, "f" => 0.5, "l" => list!["x", V::None], "t" => true });
        assert_eq!(dumps(&v, LINE), r#"{"n": 1, "f": 0.5, "l": ["x", null], "t": true}"#);
    }
}
