//! 词法：把程序切成记号（数字、字符串、名字、运算符）。不支持的写法（三引号、字节串、f-string、复数）在这里就说清楚是什么。

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Name(String),
    Int(i128),
    Float(f64),
    Str(String),
    Op(&'static str),
    /// 括号外面的换行：和分号一样把语句分开
    Newline,
    End,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    /// 在原文里是第几个字符（从 0 起）
    pub pos: usize,
}

/// 读不懂或者不支持的写法：在第几个字符、是什么（退出码 2）
#[derive(Debug, Clone, PartialEq)]
pub struct Bad {
    pub pos: usize,
    pub msg: String,
}

pub fn bad<T>(pos: usize, msg: impl Into<String>) -> Result<T, Bad> {
    Err(Bad { pos, msg: msg.into() })
}

// 长的在前：先试三个字符的，再两个、一个
const OPS: &[&str] = &[
    "**=", "//=", ">>=", "<<=", "...", "**", "//", "==", "!=", "<=", ">=", "<<", ">>", "+=", "-=", "*=", "/=", "%=", "&=", "|=",
    "^=", "@=", "->", ":=", "(", ")", "[", "]", "{", "}", ",", ":", ".", ";", "=", "<", ">", "+", "-", "*", "/", "%", "&", "|",
    "^", "~", "@",
];

fn is_name_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn is_name_char(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// free_first：第一行前面的空格、制表符不算缩进（eval() 会先把它们去掉；exec() 不会）
pub fn lex(src: &str, free_first: bool) -> Result<Vec<Token>, Bad> {
    let s: Vec<char> = src.chars().collect();
    let mut out: Vec<Token> = Vec::new();
    let mut i = 0;
    let mut depth = 0usize;
    // 这一行开头有几个空白：Some 是还在行首（只见过空白），出了第一个记号就是 None。
    // 这里没有带语句块的写法，所以一行前面有缩进就是 Python 的 unexpected indent
    let mut indent: Option<usize> = if free_first { None } else { Some(0) };
    while i < s.len() {
        let c = s[i];
        let start = i;
        if !matches!(c, ' ' | '\t' | '\u{c}' | '\n' | '\r' | '#') {
            if indent.is_some_and(|n| n > 0) {
                return bad(i, "这一行前面多了缩进（Python 报 unexpected indent）");
            }
            indent = None;
        }
        match c {
            ' ' | '\t' => {
                i += 1;
                if let Some(n) = indent.as_mut() {
                    *n += 1;
                }
            }
            '\u{c}' => {
                // 换页符：Python 把这一行的缩进从头算
                i += 1;
                if let Some(n) = indent.as_mut() {
                    *n = 0;
                }
            }
            '\n' | '\r' => {
                i += 1;
                if c == '\r' && s.get(i) == Some(&'\n') {
                    i += 1;
                }
                // 括号里的换行不算；连着的几个换行、开头的换行只留一个
                if depth == 0 {
                    indent = Some(0);
                    if !matches!(out.last().map(|t| &t.tok), None | Some(Tok::Newline)) {
                        out.push(Token { tok: Tok::Newline, pos: start });
                    }
                }
            }
            '#' => {
                while i < s.len() && s[i] != '\n' && s[i] != '\r' {
                    i += 1;
                }
            }
            '\\' => match s.get(i + 1) {
                Some('\n') => i += 2,
                Some('\r') => {
                    i += 2;
                    if s.get(i) == Some(&'\n') {
                        i += 1;
                    }
                }
                _ => return bad(i, "反斜杠只能放在行尾（续行）"),
            },
            '"' | '\'' => {
                let text = string(&s, &mut i, false)?;
                out.push(Token { tok: Tok::Str(text), pos: start });
            }
            '0'..='9' => {
                let tok = number(&s, &mut i)?;
                out.push(Token { tok, pos: start });
            }
            '.' if matches!(s.get(i + 1), Some('0'..='9')) => {
                let tok = number(&s, &mut i)?;
                out.push(Token { tok, pos: start });
            }
            c if is_name_start(c) => {
                let mut j = i;
                while j < s.len() && is_name_char(s[j]) {
                    j += 1;
                }
                let word: String = s[i..j].iter().collect();
                i = j;
                if matches!(s.get(j), Some('"' | '\'')) {
                    // 字符串的前缀
                    match word.to_ascii_lowercase().as_str() {
                        "r" => {
                            let text = string(&s, &mut i, true)?;
                            out.push(Token { tok: Tok::Str(text), pos: start });
                            continue;
                        }
                        "u" => {
                            let text = string(&s, &mut i, false)?;
                            out.push(Token { tok: Tok::Str(text), pos: start });
                            continue;
                        }
                        "b" | "br" | "rb" => return bad(start, "不支持字节串（b\"…\"）"),
                        "f" | "fr" | "rf" => return bad(start, "不支持 f-string（用 % 或者 + 拼）"),
                        "t" | "tr" | "rt" => return bad(start, "不支持 t-string"),
                        _ => {}
                    }
                }
                out.push(Token { tok: Tok::Name(word), pos: start });
            }
            _ => {
                let Some(op) = OPS.iter().find(|op| op.chars().enumerate().all(|(k, oc)| s.get(i + k) == Some(&oc))) else {
                    return bad(i, format!("不认识的字符 {c:?}"));
                };
                match *op {
                    "(" | "[" | "{" => depth += 1,
                    ")" | "]" | "}" => depth = depth.saturating_sub(1),
                    _ => {}
                }
                i += op.len();
                out.push(Token { tok: Tok::Op(op), pos: start });
            }
        }
    }
    out.push(Token { tok: Tok::End, pos: s.len() });
    Ok(out)
}

/// 一串数字（下划线只能夹在两个数字中间）；返回去掉下划线的那几位
fn digits(s: &[char], i: &mut usize, radix: u32) -> Result<String, Bad> {
    let start = *i;
    let mut out = String::new();
    while let Some(&c) = s.get(*i) {
        if c.is_digit(radix) {
            out.push(c);
        } else if c == '_' {
            let prev_ok = *i > start && s[*i - 1] != '_';
            let next_ok = s.get(*i + 1).is_some_and(|n| n.is_digit(radix));
            if !(prev_ok && next_ok) {
                return bad(*i, "数字里的下划线要夹在两个数字中间");
            }
        } else {
            break;
        }
        *i += 1;
    }
    Ok(out)
}

fn number(s: &[char], i: &mut usize) -> Result<Tok, Bad> {
    let start = *i;
    let too_big = |pos| bad(pos, "整数太大（这里的整数最多 128 位）");
    // 0x、0o、0b
    if s[*i] == '0' {
        let radix = match s.get(*i + 1) {
            Some('x' | 'X') => 16,
            Some('o' | 'O') => 8,
            Some('b' | 'B') => 2,
            _ => 0,
        };
        if radix != 0 {
            *i += 2;
            if s.get(*i) == Some(&'_') && s.get(*i + 1).is_some_and(|c| c.is_digit(radix)) {
                *i += 1; // 0x_ff 也是合法的
            }
            let text = digits(s, i, radix)?;
            if text.is_empty() || s.get(*i).is_some_and(|&c| is_name_char(c)) {
                return bad(start, "数字写得不对");
            }
            return match i128::from_str_radix(&text, radix) {
                Ok(n) => Ok(Tok::Int(n)),
                Err(_) => too_big(start),
            };
        }
    }
    let mut text = digits(s, i, 10)?;
    let mut float = false;
    if s.get(*i) == Some(&'.') {
        float = true;
        *i += 1;
        text.push('.');
        text.push_str(&digits(s, i, 10)?);
    }
    if matches!(s.get(*i), Some('e' | 'E')) {
        let mut j = *i + 1;
        if matches!(s.get(j), Some('+' | '-')) {
            j += 1;
        }
        if !s.get(j).is_some_and(|c| c.is_ascii_digit()) {
            return bad(start, "数字写得不对");
        }
        float = true;
        text.push('e');
        if s[*i + 1] == '-' {
            text.push('-');
        }
        *i = j;
        text.push_str(&digits(s, i, 10)?);
    }
    match s.get(*i) {
        Some('j' | 'J') => return bad(start, "不支持复数"),
        Some(&c) if is_name_char(c) => return bad(start, "数字写得不对（后面紧跟着字母）"),
        _ => {}
    }
    if float {
        return match text.parse::<f64>() {
            Ok(f) => Ok(Tok::Float(f)),
            Err(_) => bad(start, "数字写得不对"),
        };
    }
    if text.len() > 1 && text.starts_with('0') && text.bytes().any(|b| b != b'0') {
        return bad(start, "十进制整数不能以 0 开头");
    }
    match text.parse::<i128>() {
        Ok(n) => Ok(Tok::Int(n)),
        Err(_) => too_big(start),
    }
}

fn hex(s: &[char], i: &mut usize, n: usize, what: &str) -> Result<char, Bad> {
    let text: String = s.get(*i..*i + n).unwrap_or(&[]).iter().collect();
    if text.len() != n || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return bad(*i, format!("{what} 后面要 {n} 位十六进制"));
    }
    let code = u32::from_str_radix(&text, 16).unwrap_or(u32::MAX);
    *i += n;
    match char::from_u32(code) {
        Some(c) => Ok(c),
        None if (0xd800..0xe000).contains(&code) => bad(*i - n, "单独的代理项（U+D800 到 U+DFFF）这里不支持"),
        None => bad(*i - n, "不是合法的字符编号"),
    }
}

/// 一个字符串字面量：*i 指着开头的引号，读完指到结尾的引号后面
fn string(s: &[char], i: &mut usize, raw: bool) -> Result<String, Bad> {
    let start = *i;
    let q = s[*i];
    if s.get(*i + 1) == Some(&q) && s.get(*i + 2) == Some(&q) {
        return bad(start, "不支持三引号字符串");
    }
    *i += 1;
    let mut out = String::new();
    loop {
        let Some(&c) = s.get(*i) else { return bad(start, "字符串没有结束") };
        *i += 1;
        if c == q {
            return Ok(out);
        }
        if c == '\n' || c == '\r' {
            return bad(start, "字符串没有结束");
        }
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(&e) = s.get(*i) else { return bad(start, "字符串没有结束") };
        *i += 1;
        if raw {
            // 原样留着反斜杠；反斜杠后面的引号不结束字符串
            out.push('\\');
            out.push(e);
            continue;
        }
        match e {
            '\n' => {}
            '\r' => {
                if s.get(*i) == Some(&'\n') {
                    *i += 1;
                }
            }
            '\\' => out.push('\\'),
            '\'' => out.push('\''),
            '"' => out.push('"'),
            'a' => out.push('\u{7}'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'v' => out.push('\u{b}'),
            'x' => out.push(hex(s, i, 2, "\\x")?),
            'u' => out.push(hex(s, i, 4, "\\u")?),
            'U' => out.push(hex(s, i, 8, "\\U")?),
            'N' => return bad(*i - 2, "不支持 \\N{名字} 这种转义"),
            '0'..='7' => {
                // 八进制：最多三位
                let mut code = e.to_digit(8).unwrap_or(0);
                for _ in 0..2 {
                    match s.get(*i).and_then(|c| c.to_digit(8)) {
                        Some(d) => {
                            code = code * 8 + d;
                            *i += 1;
                        }
                        None => break,
                    }
                }
                out.push(char::from_u32(code).unwrap_or('\0'));
            }
            // 不认识的转义 Python 原样留着（连反斜杠）
            other => {
                out.push('\\');
                out.push(other);
            }
        }
    }
}
