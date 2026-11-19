//! Python 字符串的那些方法。和 Rust 自带的同名方法不一样的地方（认哪些空白、按字符还是按字节数、
//! `splitlines` 在哪断行）都照 Python 的来；Rust 自带的就够的（`split(sep)`、`starts_with`、`replace`）直接用自带的。

use super::uni;

/// `s.strip()`
pub fn strip(s: &str) -> &str {
    s.trim_matches(uni::is_space)
}

/// `s.lstrip()`
pub fn lstrip(s: &str) -> &str {
    s.trim_start_matches(uni::is_space)
}

/// `s.rstrip()`
pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(uni::is_space)
}

/// `s.strip(chars)`
pub fn strip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_matches(|c| chars.contains(c))
}

/// `s.rstrip(chars)`
pub fn rstrip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_end_matches(|c| chars.contains(c))
}

/// `s.lstrip(chars)`
pub fn lstrip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_start_matches(|c| chars.contains(c))
}

/// `s.split()`：按连着的空白切，开头结尾的空白不算
pub fn split_ws(s: &str) -> Vec<&str> {
    s.split(uni::is_space).filter(|x| !x.is_empty()).collect()
}

/// `s.split(None, n)`：最多切 n 刀，剩下的原样（开头的空白去掉）
pub fn split_ws_n(s: &str, n: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = lstrip(s);
    while !rest.is_empty() {
        if out.len() == n {
            out.push(rest);
            return out;
        }
        match rest.find(uni::is_space) {
            Some(i) => {
                out.push(&rest[..i]);
                rest = lstrip(&rest[i..]);
            }
            None => {
                out.push(rest);
                break;
            }
        }
    }
    out
}

/// `s.partition(sep)`
pub fn partition<'a>(s: &'a str, sep: &str) -> (&'a str, &'a str, &'a str) {
    match s.find(sep) {
        Some(i) => (&s[..i], &s[i..i + sep.len()], &s[i + sep.len()..]),
        None => (s, "", ""),
    }
}

/// `s.rpartition(sep)`
pub fn rpartition<'a>(s: &'a str, sep: &str) -> (&'a str, &'a str, &'a str) {
    match s.rfind(sep) {
        Some(i) => (&s[..i], &s[i..i + sep.len()], &s[i + sep.len()..]),
        None => ("", "", s),
    }
}

/// `s.splitlines()`：`\n`、`\r`、`\r\n` 之外，`\v`、`\f`、`\x1c`–`\x1e`、`\x85`、U+2028、U+2029 也断行；末尾不多出一个空行
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let brk = matches!(c, '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}');
        if !brk {
            continue;
        }
        out.push(&s[start..i]);
        let mut next = i + c.len_utf8();
        if c == '\r' {
            if let Some(&(j, '\n')) = it.peek() {
                it.next();
                next = j + 1;
            }
        }
        start = next;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// `len(s)`：字符数
pub fn len(s: &str) -> usize {
    s.chars().count()
}

/// `s[:n]`
pub fn head(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// `s[n:]`
pub fn skip(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[i..],
        None => "",
    }
}

/// `s[-n:]`
pub fn tail(s: &str, n: usize) -> &str {
    if n == 0 {
        return s; // s[-0:] 就是整个 s
    }
    match s.char_indices().rev().nth(n - 1) {
        Some((i, _)) => &s[i..],
        None => s,
    }
}

/// `s[a:b]`（a、b 是不小于 0 的字符位置）
pub fn slice(s: &str, a: usize, b: usize) -> &str {
    if b <= a {
        return "";
    }
    head(skip(s, a), b - a)
}

/// `s.isdigit()`
pub fn isdigit(s: &str) -> bool {
    !s.is_empty() && s.chars().all(uni::is_digit)
}

/// `s.isspace()`
pub fn isspace(s: &str) -> bool {
    !s.is_empty() && s.chars().all(uni::is_space)
}

/// `s.isprintable()`（空串算能打印）
pub fn isprintable(s: &str) -> bool {
    s.chars().all(uni::is_printable)
}

/// `s.isalnum()`
pub fn isalnum(s: &str) -> bool {
    !s.is_empty() && s.chars().all(uni::is_alnum)
}

/// `int(s)`：前后的空白、正负号、数字之间的下划线、各种文字的十进制数字都认；别的是 ValueError（这里是 None）
pub fn to_int(s: &str) -> Option<i64> {
    let s = strip(s);
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let mut n: i64 = 0;
    let mut prev_digit = false;
    let mut any = false;
    for c in body.chars() {
        if c == '_' {
            if !prev_digit {
                return None;
            }
            prev_digit = false;
            continue;
        }
        let d = uni::decimal_value(c)? as i64;
        n = n.checked_mul(10)?.checked_add(d)?;
        prev_digit = true;
        any = true;
    }
    if !any || !prev_digit {
        return None;
    }
    Some(if neg { -n } else { n })
}

/// `float(s)`：前后的空白、`nan`、`inf`、`infinity`（不分大小写）、数字之间的下划线都认
pub fn to_float(s: &str) -> Option<f64> {
    let s = strip(s);
    if s.is_empty() {
        return None;
    }
    // 别的文字的十进制数字（全角的 １２３ 之类）Python 也认：先换成 ASCII 的
    let ascii: String;
    let s = if s.is_ascii() {
        s
    } else {
        ascii = s.chars().map(|c| if c.is_ascii() { Some(c) } else { uni::decimal_value(c).and_then(|d| char::from_digit(d, 10)) }).collect::<Option<String>>()?;
        &ascii
    };
    let b = s.as_bytes();
    let mut t = String::with_capacity(s.len());
    for (i, &c) in b.iter().enumerate() {
        if c == b'_' {
            let ok = i > 0 && b[i - 1].is_ascii_digit() && b.get(i + 1).is_some_and(|x| x.is_ascii_digit());
            if !ok {
                return None;
            }
        } else {
            t.push(c as char);
        }
    }
    // Rust 认的写法比 Python 多不出什么：十六进制、单独的点都不认，和 Python 一样
    t.parse().ok()
}

/// `shlex.quote(s)`：只有字母数字和 `_@%+=:,./-` 的原样，别的用单引号包起来
pub fn sh_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    if s.bytes().all(|c| c.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&c)) {
        return s.into();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// `shlex.split(s)`（POSIX 规则）：引号没合上、反斜杠后面没字符是 ValueError，这里给出它的说法
pub fn sh_split(s: &str) -> Result<Vec<String>, &'static str> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut has = false; // 这一项开始了没有（'' 也算一项）
    let mut it = s.chars();
    while let Some(c) = it.next() {
        match c {
            ' ' | '\t' | '\r' | '\n' => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            '\'' => {
                has = true;
                loop {
                    match it.next() {
                        Some('\'') => break,
                        Some(x) => cur.push(x),
                        None => return Err("No closing quotation"),
                    }
                }
            }
            '"' => {
                has = true;
                loop {
                    match it.next() {
                        Some('"') => break,
                        Some('\\') => match it.next() {
                            Some(x @ ('"' | '\\')) => cur.push(x),
                            Some(x) => {
                                cur.push('\\');
                                cur.push(x);
                            }
                            None => return Err("No closing quotation"),
                        },
                        Some(x) => cur.push(x),
                        None => return Err("No closing quotation"),
                    }
                }
            }
            '\\' => {
                has = true;
                match it.next() {
                    Some(x) => cur.push(x),
                    None => return Err("No escaped character"),
                }
            }
            x => {
                has = true;
                cur.push(x);
            }
        }
    }
    if has {
        out.push(cur);
    }
    Ok(out)
}

/// 截到 w 格宽，放不下时末尾换成 `…`；pad 时右边用空格补齐到 w 格（原来的 `fit`）
pub fn fit(s: &str, w: usize, pad: bool) -> String {
    let total = uni::width(s);
    let mut out = String::new();
    if total > w {
        let mut used = 0;
        for c in s.chars() {
            let cw = if uni::is_wide(c) { 2 } else { 1 };
            if used + cw > w.saturating_sub(1) {
                break;
            }
            out.push(c);
            used += cw;
        }
        out.push('…');
    } else {
        out.push_str(s);
    }
    if pad {
        let have = uni::width(&out);
        for _ in have..w {
            out.push(' ');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_and_split() {
        assert_eq!(strip(" \u{3000}a b\t\n\u{1c}"), "a b");
        assert_eq!(split_ws("  a \u{a0} b\tc\n"), ["a", "b", "c"]);
        assert_eq!(split_ws_n(" a  b  c d ", 2), ["a", "b", "c d "]);
        assert_eq!(split_ws_n("a", 1), ["a"]);
        assert!(split_ws("   ").is_empty());
        assert_eq!(partition("a=b=c", "="), ("a", "=", "b=c"));
        assert_eq!(partition("abc", "="), ("abc", "", ""));
        assert_eq!(rpartition("a=b=c", "="), ("a=b", "=", "c"));
        assert_eq!(rpartition("abc", "="), ("", "", "abc"));
    }

    #[test]
    fn splitlines_like_python() {
        assert_eq!(splitlines("a\nb\r\nc\rd"), ["a", "b", "c", "d"]);
        assert_eq!(splitlines("a\n"), ["a"]);
        assert_eq!(splitlines("a\n\nb"), ["a", "", "b"]);
        assert_eq!(splitlines("a\u{b}b\u{c}c\u{1c}d\u{85}e\u{2028}f"), ["a", "b", "c", "d", "e", "f"]);
        assert!(splitlines("").is_empty());
        assert_eq!(splitlines("\n"), [""]);
    }

    #[test]
    fn slices_by_char() {
        assert_eq!(head("中文abc", 3), "中文a");
        assert_eq!(skip("中文abc", 2), "abc");
        assert_eq!(tail("中文abc", 4), "文abc");
        assert_eq!(tail("ab", 5), "ab");
        assert_eq!(slice("中文abc", 1, 3), "文a");
        assert_eq!(len("中文a"), 3);
    }

    #[test]
    fn numbers() {
        assert_eq!(to_int(" 42 "), Some(42));
        assert_eq!(to_int("-1_000"), Some(-1000));
        assert_eq!(to_int("+7"), Some(7));
        assert_eq!(to_int("٣٤"), Some(34));
        for bad in ["", "_1", "1_", "1__0", "1.0", "0x10", "²", "- 1"] {
            assert_eq!(to_int(bad), None, "{bad}");
        }
        assert_eq!(to_float(" 1_0.5 "), Some(10.5));
        assert_eq!(to_float("1e3"), Some(1000.0));
        assert!(to_float("NaN").unwrap().is_nan());
        assert_eq!(to_float("-Infinity"), Some(f64::NEG_INFINITY));
        assert_eq!([to_float(".5"), to_float("5."), to_float("１.５")], [Some(0.5), Some(5.0), Some(1.5)]);
        for bad in ["", "1_", "abc", "1,5", "1e", "0x1p3"] {
            assert_eq!(to_float(bad), None, "{bad}");
        }
    }

    #[test]
    fn shell_quoting() {
        assert_eq!(sh_quote(""), "''");
        assert_eq!(sh_quote("abc-1.2/@x"), "abc-1.2/@x");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), "'it'\"'\"'s'");
        assert_eq!(sh_quote("中"), "'中'");
        assert_eq!(sh_split("a 'b c' \"d \\\" e\" f\\ g ''").unwrap(), ["a", "b c", "d \" e", "f g", ""]);
        assert_eq!(sh_split("A=1 B='x y'").unwrap(), ["A=1", "B=x y"]);
        assert_eq!(sh_split("a 'b"), Err("No closing quotation"));
        assert_eq!(sh_split("a \\"), Err("No escaped character"));
    }

    #[test]
    fn fit_to_width() {
        assert_eq!(fit("abcdef", 4, false), "abc…");
        assert_eq!(fit("中文字", 5, false), "中文…");
        assert_eq!(fit("中文字", 4, false), "中…");
        assert_eq!(fit("ab", 4, true), "ab  ");
        assert_eq!(fit("中文", 4, true), "中文");
    }
}
