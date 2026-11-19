//! Python 的 `re`，底下是 `regex` 库。
//!
//! 正则照 Python 里的原样写（`re!(r"…")`），这里翻译成 `regex` 认的写法，并补上两边不一样的地方：
//!
//! - `$`（不带 `re.M`）在 Python 里也匹配结尾那个换行**之前**：`re.match(r'^abc$', 'abc\n')` 是匹配的。
//!   原来的校验（账号表的键、邮箱、key……）都带着这个行为，照样保留。
//! - `\s` 认的空白、`\w` 认的字符照 Python 的范围（`regex` 的 `\s` 不认 `\x1c`–`\x1f`，`\w` 多认组合字符）。
//! - 字符组里没转义的 `[`、`&`、`~` 在 Python 里是普通字符，在 `regex` 里有特殊含义。
//!
//! 没有的：前瞻、后顾、反向引用（`regex` 不支持，遇到直接 panic）。原来用到的几处改成手写的判断。
//! 替换时和 Python 还有一处不同：能匹配空串的正则，紧挨着上一个匹配的那个空匹配 `regex` 不算，原来的代码没有靠它的。

use regex::{Captures, Regex};
use std::sync::OnceLock;

pub const I: u32 = 1;
pub const M: u32 = 2;
pub const S: u32 = 4;

/// Python 的 `str.isspace()` / `\s` 认的字符，写成字符组里的内容
const PY_SPACE: &str = r"\t\n\x0b\x0c\r\x1c-\x1f \x85\xa0  -     　";
/// Python 的 `\w`：`isalnum()` 的字符和下划线
const PY_WORD: &str = r"\p{L}\p{N}_";

pub struct Re {
    /// 翻译好的正则（带着开头的标志）；三种匹配方式各自第一次用到时才编译，多数正则只用得到一种
    prefix: String,
    body: String,
    /// 哪里都能匹配（`re.search`）
    any: OnceLock<Regex>,
    /// 只从开头匹配（`re.match`）
    head: OnceLock<Regex>,
    /// 开头到结尾（`re.fullmatch`）
    full: OnceLock<Regex>,
    /// 正则里有不带 `re.M` 的 `$`：文字以换行结尾时，去掉这个换行再试一次
    dollar: bool,
}

/// 一次匹配（`re.Match`）
pub struct Match<'t> {
    caps: Captures<'t>,
}

impl<'t> Match<'t> {
    /// `m.group(i)`：这一组没参与匹配时是 None
    pub fn group(&self, i: usize) -> Option<&'t str> {
        self.caps.get(i).map(|m| m.as_str())
    }

    /// `m.group(i)`，没参与匹配的当空串
    pub fn g(&self, i: usize) -> &'t str {
        self.group(i).unwrap_or("")
    }

    /// `m.group('名字')`
    pub fn name(&self, n: &str) -> Option<&'t str> {
        self.caps.name(n).map(|m| m.as_str())
    }

    /// `m.start(i)`：字节位置（切同一段文字时和 Python 的字符位置等价，别拿它当字符数）
    pub fn start(&self, i: usize) -> usize {
        self.caps.get(i).map_or(0, |m| m.start())
    }

    pub fn end(&self, i: usize) -> usize {
        self.caps.get(i).map_or(0, |m| m.end())
    }

    /// 组数（不算第 0 组）
    pub fn groups(&self) -> usize {
        self.caps.len() - 1
    }
}

impl Re {
    /// `re.compile(pattern)`。正则写错了、用了不支持的写法是程序的错，直接 panic
    pub fn new(pattern: &str) -> Re {
        Re::with_flags(pattern, 0)
    }

    /// `re.compile(pattern, flags)`，flags 是 `I`、`M`、`S` 的组合
    pub fn with_flags(pattern: &str, flags: u32) -> Re {
        let multiline = flags & M != 0 || pattern.contains("(?m");
        let (body, dollar) = translate(pattern, multiline);
        let mut prefix = String::new();
        for (bit, ch) in [(I, 'i'), (M, 'm'), (S, 's')] {
            if flags & bit != 0 {
                prefix.push(ch);
            }
        }
        let prefix = if prefix.is_empty() { String::new() } else { format!("(?{prefix})") };
        let re = Re { prefix, body, any: OnceLock::new(), head: OnceLock::new(), full: OnceLock::new(), dollar };
        #[cfg(debug_assertions)]
        re.any(); // 调试版里当场编译一次：写错的正则不用等到用到它的那条路才发现
        re
    }

    fn build(&self, p: String) -> Regex {
        regex::RegexBuilder::new(&p).size_limit(64 << 20).build().unwrap_or_else(|e| panic!("正则 {:?} 翻译成 {p:?} 后编译不了：{e}", self.body))
    }

    fn any(&self) -> &Regex {
        self.any.get_or_init(|| self.build(format!("{}{}", self.prefix, self.body)))
    }

    fn head(&self) -> &Regex {
        self.head.get_or_init(|| self.build(format!(r"{}\A(?:{})", self.prefix, self.body)))
    }

    fn full(&self) -> &Regex {
        self.full.get_or_init(|| self.build(format!(r"{}\A(?:{})\z", self.prefix, self.body)))
    }

    fn run<'t>(&self, rx: &Regex, text: &'t str) -> Option<Match<'t>> {
        let whole = rx.captures(text);
        if !self.dollar || !text.ends_with('\n') {
            return whole.map(|caps| Match { caps });
        }
        // `$` 也算结尾那个换行之前：去掉换行再匹配一次，哪个起点靠前用哪个，一样靠前用完整的那次
        let cut = rx.captures(&text[..text.len() - 1]);
        let pick_cut = match (&whole, &cut) {
            (Some(w), Some(c)) => c.get(0).unwrap().start() < w.get(0).unwrap().start(),
            (None, Some(_)) => true,
            _ => false,
        };
        if pick_cut { cut } else { whole }.map(|caps| Match { caps })
    }

    /// `rx.search(text)`
    pub fn search<'t>(&self, text: &'t str) -> Option<Match<'t>> {
        self.run(self.any(), text)
    }

    /// `rx.match(text)`：只从开头匹配
    pub fn match_<'t>(&self, text: &'t str) -> Option<Match<'t>> {
        self.run(self.head(), text)
    }

    /// `rx.fullmatch(text)`
    pub fn fullmatch<'t>(&self, text: &'t str) -> Option<Match<'t>> {
        self.full().captures(text).map(|caps| Match { caps })
    }

    /// `bool(rx.search(text))`
    pub fn found(&self, text: &str) -> bool {
        self.search(text).is_some()
    }

    /// `bool(rx.match(text))`
    pub fn matches(&self, text: &str) -> bool {
        self.match_(text).is_some()
    }

    /// `rx.finditer(text)`
    pub fn finditer<'t>(&self, text: &'t str) -> Vec<Match<'t>> {
        let mut out: Vec<Match<'t>> = self.any().captures_iter(text).map(|caps| Match { caps }).collect();
        if self.dollar && text.ends_with('\n') {
            // 去掉结尾的换行才匹配得到的那一个（`$` 在换行之前）
            let from = out.last().map_or(0, |m| m.end(0));
            let cut = &text[..text.len() - 1];
            if from <= cut.len() {
                if let Some(caps) = self.any().captures_at(cut, from) {
                    let m0 = caps.get(0).unwrap();
                    if m0.end() == cut.len() && !(m0.is_empty() && from == cut.len() && !out.is_empty()) {
                        out.push(Match { caps });
                    }
                }
            }
        }
        out
    }

    /// `rx.findall(text)` 在没有分组或只有一组时的结果：有一组就是那一组，没有分组是整个匹配
    pub fn findall<'t>(&self, text: &'t str) -> Vec<&'t str> {
        self.finditer(text).iter().map(|m| if m.groups() >= 1 { m.g(1) } else { m.g(0) }).collect()
    }

    /// `rx.sub(函数, text)`
    pub fn sub_with(&self, text: &str, mut f: impl FnMut(&Match) -> String) -> String {
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for m in self.finditer(text) {
            out.push_str(&text[last..m.start(0)]);
            out.push_str(&f(&m));
            last = m.end(0);
        }
        out.push_str(&text[last..]);
        out
    }

    /// `rx.sub(repl, text)`：repl 是 Python 的替换写法（`\1`、`\g<名字>`、`\\`、`\n`）
    pub fn sub(&self, repl: &str, text: &str) -> String {
        if !repl.contains('\\') {
            return self.sub_with(text, |_| repl.to_string());
        }
        self.sub_with(text, |m| expand(repl, m))
    }

    /// `rx.sub(repl, text, count=1)`
    pub fn sub_once(&self, repl: &str, text: &str) -> String {
        match self.search(text) {
            Some(m) => format!("{}{}{}", &text[..m.start(0)], expand(repl, &m), &text[m.end(0)..]),
            None => text.to_string(),
        }
    }

    /// `rx.split(text)`：正则里有分组时，分组匹配到的也在结果里（没参与的那组是 None）
    pub fn split<'t>(&self, text: &'t str) -> Vec<Option<&'t str>> {
        let mut out = Vec::new();
        let mut last = 0;
        for m in self.finditer(text) {
            if m.end(0) == m.start(0) && (m.start(0) == 0 || m.start(0) == text.len()) {
                continue; // 两头的空匹配不切
            }
            out.push(Some(&text[last..m.start(0)]));
            for i in 1..=m.groups() {
                out.push(m.group(i));
            }
            last = m.end(0);
        }
        out.push(Some(&text[last..]));
        out
    }
}

/// Python 的替换写法展开成文字
fn expand(repl: &str, m: &Match) -> String {
    let mut out = String::with_capacity(repl.len());
    let mut it = repl.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some(d) if d.is_ascii_digit() => {
                let mut n = d.to_digit(10).unwrap() as usize;
                if let Some(d2) = it.peek().and_then(|x| x.to_digit(10)) {
                    n = n * 10 + d2 as usize;
                    it.next();
                }
                out.push_str(m.g(n));
            }
            Some('g') => {
                let mut name = String::new();
                if it.next() != Some('<') {
                    panic!("替换写法 {repl:?} 里的 \\g 后面要跟 <名字>");
                }
                for x in it.by_ref() {
                    if x == '>' {
                        break;
                    }
                    name.push(x);
                }
                match name.parse::<usize>() {
                    Ok(n) => out.push_str(m.g(n)),
                    Err(_) => out.push_str(m.name(&name).unwrap_or("")),
                }
            }
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(x) => panic!("替换写法 {repl:?} 里有不认识的 \\{x}"),
            None => panic!("替换写法 {repl:?} 以反斜杠结尾"),
        }
    }
    out
}

/// `re.escape(s)`
pub fn escape(s: &str) -> String {
    regex::escape(s)
}

/// Python 的正则翻译成 `regex` 的写法；第二个返回值：里面有不带 `re.M` 的 `$`（已经换成 `\z`）
fn translate(p: &str, multiline: bool) -> (String, bool) {
    let mut out = String::with_capacity(p.len() + 16);
    let mut dollar = false;
    let mut in_class = false;
    let mut class_start = 0; // 字符组开头在 out 里的位置（`[` 或 `[^` 之后）
    let cs: Vec<char> = p.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        i += 1;
        if c == '\\' {
            let Some(&e) = cs.get(i) else { panic!("正则 {p:?} 以反斜杠结尾") };
            i += 1;
            match e {
                's' if in_class => out.push_str(&format!("[{PY_SPACE}]")),
                's' => out.push_str(&format!("[{PY_SPACE}]")),
                'S' => out.push_str(&format!("[^{PY_SPACE}]")),
                'w' => out.push_str(&format!("[{PY_WORD}]")),
                'W' => out.push_str(&format!("[^{PY_WORD}]")),
                'Z' if !in_class => out.push_str(r"\z"),
                '0' => out.push_str(r"\x00"),
                '1'..='9' if !in_class => panic!("正则 {p:?} 用了反向引用，这里不支持"),
                _ => {
                    out.push('\\');
                    out.push(e);
                }
            }
            continue;
        }
        if in_class {
            match c {
                ']' if out.len() > class_start => {
                    in_class = false;
                    out.push(']');
                }
                '[' | '&' | '~' => {
                    out.push('\\');
                    out.push(c);
                }
                _ => out.push(c),
            }
            continue;
        }
        match c {
            '[' => {
                in_class = true;
                out.push('[');
                if cs.get(i) == Some(&'^') {
                    out.push('^');
                    i += 1;
                }
                class_start = out.len();
                if cs.get(i) == Some(&']') {
                    // 开头的 ] 是普通字符
                    out.push_str(r"\]");
                    i += 1;
                }
            }
            '$' if !multiline => {
                dollar = true;
                out.push_str(r"\z");
            }
            '(' if cs.get(i) == Some(&'?') => {
                match cs.get(i + 1) {
                    Some('=') | Some('!') => panic!("正则 {p:?} 用了前瞻，这里不支持"),
                    Some('<') if matches!(cs.get(i + 2), Some('=') | Some('!')) => panic!("正则 {p:?} 用了后顾，这里不支持"),
                    Some('P') if cs.get(i + 2) == Some(&'=') => panic!("正则 {p:?} 用了反向引用，这里不支持"),
                    _ => {}
                }
                out.push('(');
            }
            '{' if cs.get(i) == Some(&',') => out.push_str("{0"),
            _ => out.push(c),
        }
    }
    (out, dollar)
}

/// 写在代码里的正则：第一次用到时编译，之后一直用同一份。`re!(r"^\d+$")`、`re!(r"abc", re::I)`
#[macro_export]
macro_rules! re {
    ($pat:expr) => {{
        static R: std::sync::LazyLock<$crate::py::re::Re> = std::sync::LazyLock::new(|| $crate::py::re::Re::new($pat));
        &*R
    }};
    ($pat:expr, $flags:expr) => {{
        static R: std::sync::LazyLock<$crate::py::re::Re> =
            std::sync::LazyLock::new(|| $crate::py::re::Re::with_flags($pat, $flags));
        &*R
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dollar_matches_before_final_newline() {
        let key = Re::new(r#"^[^\s"\\]{1,100}$"#);
        assert!(key.matches("abc"));
        assert!(key.matches("abc\n")); // Python 就是这样
        assert!(!key.matches("abc\n\n"));
        assert!(!key.matches("a b"));
        assert!(key.fullmatch("abc\n").is_none());
        let m = Re::new(r"^(\w+)$").match_("abc\n").unwrap();
        assert_eq!(m.g(1), "abc");
        assert_eq!(Re::new(r"x$").sub("y", "ax\n"), "ay\n");
        assert_eq!(Re::new(r"\s+$").sub("", "abc  \n"), "abc");
        assert_eq!(Re::with_flags(r"^b$", M).findall("a\nb\nc"), ["b"]);
    }

    #[test]
    fn python_classes() {
        assert!(Re::new(r"^\s$").matches("\u{1c}"));
        assert!(Re::new(r"^\S$").matches("\u{200b}"));
        assert!(Re::new(r"^[^\s]$").matches("x") && !Re::new(r"^[^\s]$").matches("\u{3000}"));
        assert!(Re::new(r"^\w+$").matches("中文_a1½"));
        assert!(!Re::new(r"^\w+$").matches("a\u{301}")); // 组合字符 Python 不算
        assert!(Re::new(r"^[\w.-]+$").matches("a.b-c_d"));
        assert!(Re::new(r"^[^\W_]+$").matches("ab1") && !Re::new(r"^[^\W_]+$").matches("a_b"));
        assert!(Re::new(r"^[[&~]+$").matches("[&~"));
        assert!(Re::new(r"^[]a]+$").matches("]a"));
        assert!(Re::new(r"^[\x21-\x7e]{0,8}…[\x21-\x7e]{4}$").matches("sk-ant-…abcd"));
        assert!(Re::new(r"^a{,2}$").matches("aa") && !Re::new(r"^a{,2}$").matches("aaa"));
    }

    #[test]
    fn flags_groups_and_sub() {
        let r = Re::with_flags(r"^(https?|all)_proxy$", I);
        assert!(r.matches("HTTPS_PROXY"));
        let m = Re::new(r"(?P<host>[a-z.]+):(\d+)").search("x example.com:443 y").unwrap();
        assert_eq!((m.name("host"), m.g(2)), (Some("example.com"), "443"));
        assert_eq!(Re::new(r"(a)|(b)").search("b").unwrap().group(1), None);
        assert_eq!(Re::new(r"[^a-z0-9]+").sub("-", "Hello, World!"), "-ello-orld-");
        assert_eq!(Re::new(r"(\w+)@(\w+)").sub(r"\2 at \1 \\ \g<1>", "me@host"), "host at me \\ me");
        assert_eq!(Re::new(r"\d").sub_once("#", "a1b2"), "a#b2");
        assert_eq!(Re::new(r"\d+").findall("a1b22c333"), ["1", "22", "333"]);
        assert_eq!(Re::new(r"(\w)=").findall("a=1 b=2"), ["a", "b"]);
        assert_eq!(Re::new(r",\s*").split("a, b,c"), [Some("a"), Some("b"), Some("c")]);
        assert_eq!(Re::new(r"(,)").split("a,b"), [Some("a"), Some(","), Some("b")]);
        assert!(re!(r"^\d+$").matches("123"));
    }

    #[test]
    #[should_panic(expected = "前瞻")]
    fn lookahead_is_rejected() {
        Re::new(r"a(?!b)");
    }
}
