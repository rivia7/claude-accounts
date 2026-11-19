//! 邮件头的值：RFC 2047 的编码词（`=?utf-8?q?…?=`）解成文字，地址（显示名、注释、组）切出来。
//! 原来是 `email.header.decode_header` + `make_header`、`email.utils.getaddresses` / `parseaddr`；这里照 CPython 3.14 的
//! 行为手写（地址用的是 3.13 起默认的严格解析：写法不对的整条不认），对照数据在 tests/vectors/mail.json。
//!
//! 进来的文字都是 `message::hstr` 给的：ASCII 加 U+FFFD（原来的非 ASCII 字节）。函数本身照任意文字写。

use super::codec::{self, raw_unicode_escape, Lookup};
use super::mail_clean;
use crate::py::base64::a2b_base64;
use crate::py::{text, uni};
use crate::py::{Fail, R};
use crate::re;

// --- 编码词 --------------------------------------------------------------------------------------------------------

/// `email.charset.ALIASES`
const CHARSET_ALIASES: &[(&str, &str)] = &[
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

const UNKNOWN8BIT: &str = "unknown-8bit";

/// 解出来的一段：文字，或者 unknown-8bit 的原始字节（显示时非 ASCII 的换成 U+FFFD）
enum Piece {
    Text(String),
    Raw(Vec<u8>),
}

/// `email.quoprimime.header_decode`：下划线是空格，`=XX` 是那个码位的字符
fn q_decode(s: &str) -> String {
    let cs: Vec<char> = s.chars().map(|c| if c == '_' { ' ' } else { c }).collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < cs.len() {
        if cs[i] == '=' && i + 2 < cs.len() && cs[i + 1].is_ascii_hexdigit() && cs[i + 2].is_ascii_hexdigit() {
            let n = cs[i + 1].to_digit(16).unwrap_or(0) * 16 + cs[i + 2].to_digit(16).unwrap_or(0);
            out.push(char::from_u32(n).unwrap_or('\u{fffd}'));
            i += 3;
        } else {
            out.push(cs[i]);
            i += 1;
        }
    }
    out
}

/// `decode_header(header)` + `make_header(…)` + `str(…)`。None：原来这一路上会出异常（base64 解不了、字符集不认识、
/// 字节不合这个字符集……），调用方改用原样的文字
fn decode_header_text(header: &str) -> Option<String> {
    let ecre = re!(r"=\?([^?]*?)\?([qQbB])\?(.*?)\?=");
    if !ecre.found(header) {
        return Some(header.to_string());
    }
    // (文字, 编码 q / b 或者没有, 字符集)
    let mut words: Vec<(String, Option<char>, Option<String>)> = Vec::new();
    for line in text::splitlines(header) {
        let parts = ecre.split(line);
        let mut i = 0;
        while i < parts.len() {
            let mut unencoded = parts[i].unwrap_or("");
            if i == 0 {
                unencoded = text::lstrip(unencoded);
            }
            if !unencoded.is_empty() {
                words.push((unencoded.to_string(), None, None));
            }
            if i + 3 < parts.len() {
                let charset = parts[i + 1].unwrap_or("").to_lowercase();
                let encoding = parts[i + 2].unwrap_or("").to_ascii_lowercase().chars().next();
                words.push((parts[i + 3].unwrap_or("").to_string(), encoding, Some(charset)));
            }
            i += 4;
        }
    }
    // 两个编码词之间只有空白的那一段不要
    let mut droplist = Vec::new();
    for n in 2..words.len() {
        if words[n].1.is_some() && words[n - 2].1.is_some() && text::isspace(&words[n - 1].0) {
            droplist.push(n - 1);
        }
    }
    for d in droplist.into_iter().rev() {
        words.remove(d);
    }
    // 各自解成字节
    let mut decoded: Vec<(Vec<u8>, Option<String>)> = Vec::new();
    for (word, encoding, charset) in words {
        let bytes = match encoding {
            None => raw_unicode_escape(&word),
            Some('q') => raw_unicode_escape(&q_decode(&word)),
            Some(_) => {
                // 少了填充的补上
                let mut padded = word.clone();
                let paderr = text::len(&word) % 4;
                if paderr != 0 {
                    padded.push_str(&"===".chars().take(4 - paderr).collect::<String>());
                }
                if padded.is_empty() {
                    Vec::new()
                } else {
                    a2b_base64(&raw_unicode_escape(&padded), false).ok()?
                }
            }
        };
        decoded.push((bytes, charset));
    }
    // 连着的、字符集一样的合起来（没编码的中间加空格）
    let mut collapsed: Vec<(Vec<u8>, Option<String>)> = Vec::new();
    for (word, charset) in decoded {
        match collapsed.last_mut() {
            Some((last, last_charset)) if *last_charset == charset => {
                if charset.is_none() {
                    last.push(b' ');
                }
                last.extend_from_slice(&word);
            }
            _ => collapsed.push((word, charset)),
        }
    }
    if collapsed.is_empty() {
        return None;
    }
    // make_header：每一段照它的字符集解成文字
    let mut chunks: Vec<(Piece, String)> = Vec::new();
    for (bytes, charset) in collapsed {
        let name = match charset {
            None => "us-ascii".to_string(),
            Some(cs) => {
                if !cs.is_ascii() {
                    return None; // CharsetError
                }
                let cs = cs.to_ascii_lowercase();
                CHARSET_ALIASES.iter().find(|(a, _)| *a == cs).map_or(cs, |(_, to)| to.to_string())
            }
        };
        // email.charset.CODEC_MAP：us-ascii 不转换（照 us-ascii 解），gb2312、big5 换成 Python 的编码名；
        // 名字是空的也照 us-ascii 解（`charset.input_codec or 'us-ascii'`）
        let input_codec = match name.as_str() {
            "us-ascii" | "" => "us-ascii",
            "gb2312" => "eucgb2312_cn",
            "big5" => "big5_tw",
            other => other,
        };
        let piece = if input_codec == UNKNOWN8BIT {
            Piece::Raw(bytes)
        } else {
            match codec::lookup(input_codec) {
                Lookup::Known(c) => Piece::Text(c.decode_strict(&bytes)?),
                Lookup::Unknown | Lookup::Null => return None,
            }
        };
        chunks.push((piece, name));
    }
    // Header._normalize：字符集一样的相邻两段用空格接起来
    let mut merged: Vec<(Piece, String)> = Vec::new();
    for (piece, name) in chunks {
        match merged.last_mut() {
            Some((last, last_name)) if *last_name == name => match (last, piece) {
                (Piece::Text(a), Piece::Text(b)) => {
                    a.push(' ');
                    a.push_str(&b);
                }
                (Piece::Raw(a), Piece::Raw(b)) => {
                    a.push(b' ');
                    a.extend_from_slice(&b);
                }
                _ => return None,
            },
            _ => merged.push((piece, name)),
        }
    }
    // Header.__str__：编码词和普通文字之间补上空格（两边已经有空白、括号、反斜杠的不补）
    let nonctext = |c: char| uni::is_space(c) || matches!(c, '(' | ')' | '\\');
    let mut out = String::new();
    let (mut first, mut last_ascii, mut lastspace) = (true, true, false);
    for (piece, name) in merged {
        let string = match piece {
            Piece::Text(s) => s,
            Piece::Raw(b) => codec::ascii_replace(&b),
        };
        let next_ascii = name == "us-ascii";
        if !first {
            let hasspace = string.chars().next().is_some_and(nonctext);
            if !last_ascii {
                if next_ascii && !hasspace {
                    out.push(' ');
                }
            } else if !next_ascii && !lastspace {
                out.push(' ');
            }
        }
        lastspace = string.chars().last().is_some_and(nonctext);
        last_ascii = next_ascii;
        out.push_str(&string);
        first = false;
    }
    Some(out)
}

/// `mail_text(v)`，v 已经是文字（`str(v or '')`）：编码词解开，解不开就用原样的；一行、不带控制字符
pub fn mail_text_str(v: &str) -> String {
    let s = decode_header_text(v).unwrap_or_else(|| v.to_string());
    text::split_ws(&mail_clean(&s)).join(" ")
}

/// `mail_text(msg.get(名字))`：邮件头的值（可能是 =?utf-8?…?= 编码的）→ 文字
pub fn mail_text(value: Option<&[u8]>) -> String {
    mail_text_str(&value.map(super::message::hstr).unwrap_or_default())
}

// --- 地址 ----------------------------------------------------------------------------------------------------------

/// 地址里的注释、组套得太深时 Python 是 RecursionError（调用栈一千层）：这里数的是原来那几个函数各占的一层，
/// 上限取 cca 里实际走到的那条路上量出来的数（`cca mail test` 里取收件人的那一处）
pub const ADDR_FRAMES: usize = 994;

const SPECIALS: &str = "()<>@,:;.\"[]";

fn is_lws(c: char) -> bool {
    c == ' ' || c == '\t'
}

fn is_crlf(c: char) -> bool {
    c == '\r' || c == '\n'
}

fn is_atomend(c: char) -> bool {
    SPECIALS.contains(c) || is_lws(c) || is_crlf(c)
}

fn is_phraseend(c: char) -> bool {
    c != '.' && is_atomend(c)
}

/// `email._parseaddr.AddrlistClass`
struct AddrParser {
    field: Vec<char>,
    pos: usize,
    commentlist: Vec<String>,
    /// 现在在原来的第几层调用里
    frames: usize,
}

impl AddrParser {
    fn at(&self) -> Option<char> {
        self.field.get(self.pos).copied()
    }

    fn enter(&mut self) -> R<()> {
        self.frames += 1;
        if self.frames > ADDR_FRAMES {
            return Err(Fail::Other("RecursionError", vec!["maximum recursion depth exceeded".into()]));
        }
        Ok(())
    }

    /// 跳过空白、收下注释；返回跳过的空格和制表符
    fn gotonext(&mut self) -> R<String> {
        self.enter()?;
        let mut ws = String::new();
        while let Some(c) = self.at() {
            if is_lws(c) || is_crlf(c) {
                if !is_crlf(c) {
                    ws.push(c);
                }
                self.pos += 1;
            } else if c == '(' {
                let comment = self.getcomment()?;
                self.commentlist.push(comment);
            } else {
                break;
            }
        }
        self.frames -= 1;
        Ok(ws)
    }

    fn getaddrlist(&mut self) -> R<Vec<(String, String)>> {
        self.enter()?;
        let mut result = Vec::new();
        while self.pos < self.field.len() {
            let ad = self.getaddress()?;
            if ad.is_empty() {
                result.push((String::new(), String::new()));
            } else {
                result.extend(ad);
            }
        }
        self.frames -= 1;
        Ok(result)
    }

    fn getaddress(&mut self) -> R<Vec<(String, String)>> {
        self.enter()?;
        self.commentlist = Vec::new();
        self.gotonext()?;
        let oldpos = self.pos;
        let plist = self.getphraselist()?;
        self.gotonext()?;
        let mut returnlist = Vec::new();
        match self.at() {
            None => {
                // 没有域名的地址
                if let Some(first) = plist.first() {
                    returnlist.push((self.commentlist.join(" "), first.clone()));
                }
            }
            Some('.' | '@') => {
                // 只有地址：从头再按地址读一遍（注释会再收一遍，原来就这样）
                self.pos = oldpos;
                let addrspec = self.getaddrspec()?;
                returnlist.push((self.commentlist.join(" "), addrspec));
            }
            Some(':') => {
                // 组
                let fieldlen = self.field.len();
                self.pos += 1;
                while self.pos < self.field.len() {
                    self.gotonext()?;
                    if self.pos < fieldlen && self.at() == Some(';') {
                        self.pos += 1;
                        break;
                    }
                    let inner = self.getaddress()?;
                    returnlist.extend(inner);
                }
            }
            Some('<') => {
                // 显示名加尖括号里的地址
                let routeaddr = self.getrouteaddr()?;
                if self.commentlist.is_empty() {
                    returnlist.push((plist.join(" "), routeaddr));
                } else {
                    returnlist.push((format!("{} ({})", plist.join(" "), self.commentlist.join(" ")), routeaddr));
                }
            }
            Some(c) => {
                if let Some(first) = plist.first() {
                    returnlist.push((self.commentlist.join(" "), first.clone()));
                } else if SPECIALS.contains(c) {
                    self.pos += 1;
                }
            }
        }
        self.gotonext()?;
        if self.at() == Some(',') {
            self.pos += 1;
        }
        self.frames -= 1;
        Ok(returnlist)
    }

    /// 尖括号里的：前面的路由（@域名:）跳过，只要地址
    fn getrouteaddr(&mut self) -> R<String> {
        self.enter()?;
        let mut adlist = String::new();
        if self.at() == Some('<') {
            let mut expectroute = false;
            self.pos += 1;
            self.gotonext()?;
            while let Some(c) = self.at() {
                if expectroute {
                    self.getdomain()?;
                    expectroute = false;
                } else if c == '>' {
                    self.pos += 1;
                    break;
                } else if c == '@' {
                    self.pos += 1;
                    expectroute = true;
                } else if c == ':' {
                    self.pos += 1;
                } else {
                    adlist = self.getaddrspec()?;
                    self.pos += 1;
                    break;
                }
                self.gotonext()?;
            }
        }
        self.frames -= 1;
        Ok(adlist)
    }

    fn getaddrspec(&mut self) -> R<String> {
        self.enter()?;
        let mut aslist: Vec<String> = Vec::new();
        self.gotonext()?;
        let blank_last = |aslist: &Vec<String>| aslist.last().is_some_and(|s| text::strip(s).is_empty());
        while let Some(c) = self.at() {
            let mut preserve_ws = true;
            if c == '.' {
                if blank_last(&aslist) {
                    aslist.pop();
                }
                aslist.push(".".to_string());
                self.pos += 1;
                preserve_ws = false;
            } else if c == '"' {
                let q = self.getquote()?;
                aslist.push(format!("\"{}\"", q.replace('\\', "\\\\").replace('"', "\\\"")));
            } else if is_atomend(c) {
                if blank_last(&aslist) {
                    aslist.pop();
                }
                break;
            } else {
                let atom = self.getatom(false)?;
                aslist.push(atom);
            }
            let ws = self.gotonext()?;
            if preserve_ws && !ws.is_empty() {
                aslist.push(ws);
            }
        }
        let out = if self.at() != Some('@') {
            aslist.concat()
        } else {
            aslist.push("@".to_string());
            self.pos += 1;
            self.gotonext()?;
            let domain = self.getdomain()?;
            // 域名不对：整个地址不要（不留下本地部分冒充地址）
            if domain.is_empty() {
                String::new()
            } else {
                aslist.concat() + &domain
            }
        };
        self.frames -= 1;
        Ok(out)
    }

    fn getdomain(&mut self) -> R<String> {
        self.enter()?;
        let mut sdlist = String::new();
        while let Some(c) = self.at() {
            if is_lws(c) {
                self.pos += 1;
            } else if c == '(' {
                let comment = self.getcomment()?;
                self.commentlist.push(comment);
            } else if c == '[' {
                let literal = self.getdomainliteral()?;
                sdlist.push_str(&literal);
            } else if c == '.' {
                self.pos += 1;
                sdlist.push('.');
            } else if c == '@' {
                // a@evil.example@good.example 这种有两个 @ 的不认
                self.frames -= 1;
                return Ok(String::new());
            } else if is_atomend(c) {
                break;
            } else {
                let atom = self.getatom(false)?;
                sdlist.push_str(&atom);
            }
        }
        self.frames -= 1;
        Ok(sdlist)
    }

    /// 两个定界符之间的一段（引号、注释、域名的方括号）：反斜杠后面的字符原样要
    fn getdelimited(&mut self, beginchar: char, endchars: &str, allowcomments: bool) -> R<String> {
        self.enter()?;
        let mut out = String::new();
        if self.at() == Some(beginchar) {
            let mut quote = false;
            self.pos += 1;
            while let Some(c) = self.at() {
                if quote {
                    out.push(c);
                    quote = false;
                } else if endchars.contains(c) {
                    self.pos += 1;
                    break;
                } else if allowcomments && c == '(' {
                    let inner = self.getcomment()?;
                    out.push_str(&inner);
                    continue; // getcomment 已经往前走了
                } else if c == '\\' {
                    quote = true;
                } else {
                    out.push(c);
                }
                self.pos += 1;
            }
        }
        self.frames -= 1;
        Ok(out)
    }

    fn getquote(&mut self) -> R<String> {
        self.enter()?;
        let out = self.getdelimited('"', "\"\r", false)?;
        self.frames -= 1;
        Ok(out)
    }

    fn getcomment(&mut self) -> R<String> {
        self.enter()?;
        let out = self.getdelimited('(', ")\r", true)?;
        self.frames -= 1;
        Ok(out)
    }

    fn getdomainliteral(&mut self) -> R<String> {
        self.enter()?;
        let out = format!("[{}]", self.getdelimited('[', "]\r", false)?);
        self.frames -= 1;
        Ok(out)
    }

    /// 一个原子：到分隔的字符为止。phrase：显示名里的（点不算分隔）
    fn getatom(&mut self, phrase: bool) -> R<String> {
        self.enter()?;
        let mut out = String::new();
        while let Some(c) = self.at() {
            if if phrase { is_phraseend(c) } else { is_atomend(c) } {
                break;
            }
            out.push(c);
            self.pos += 1;
        }
        self.frames -= 1;
        Ok(out)
    }

    fn getphraselist(&mut self) -> R<Vec<String>> {
        self.enter()?;
        let mut plist = Vec::new();
        while let Some(c) = self.at() {
            if is_lws(c) || is_crlf(c) {
                self.pos += 1;
            } else if c == '"' {
                let q = self.getquote()?;
                plist.push(q);
            } else if c == '(' {
                let comment = self.getcomment()?;
                self.commentlist.push(comment);
            } else if is_phraseend(c) {
                break;
            } else {
                let atom = self.getatom(true)?;
                plist.push(atom);
            }
        }
        self.frames -= 1;
        Ok(plist)
    }
}

/// `_AddressList(field).addresslist`
fn address_list(field: &str) -> R<Vec<(String, String)>> {
    if field.is_empty() {
        return Ok(Vec::new());
    }
    let parse = || AddrParser { field: field.chars().collect(), pos: 0, commentlist: Vec::new(), frames: 0 }.getaddrlist();
    // 套一层要一个左括号（注释）或者冒号（组）：多得出奇的才可能套得很深，放到栈够大的线程里解析
    if field.chars().filter(|c| matches!(c, '(' | ':')).count() > 40 {
        super::with_deep_stack(parse)
    } else {
        parse()
    }
}

/// `_iter_escaped_chars(addr)`：(位置, 字符)，反斜杠转义的那一对算一个（不会等于引号、括号）
fn escaped_chars(addr: &[char]) -> Vec<(usize, Option<char>)> {
    let mut out = Vec::with_capacity(addr.len());
    let mut escape = false;
    for (pos, &ch) in addr.iter().enumerate() {
        if escape {
            out.push((pos, None));
            escape = false;
        } else if ch == '\\' {
            escape = true;
        } else {
            out.push((pos, Some(ch)));
        }
    }
    if escape {
        out.push((addr.len().saturating_sub(1), None));
    }
    out
}

/// `_strip_quoted_realnames(addr)`：引号里的显示名去掉
fn strip_quoted_realnames(addr: &str) -> String {
    if !addr.contains('"') {
        return addr.to_string();
    }
    let cs: Vec<char> = addr.chars().collect();
    let mut start = 0;
    let mut open_pos: Option<usize> = None;
    let mut result = String::new();
    for (pos, ch) in escaped_chars(&cs) {
        if ch == Some('"') {
            match open_pos {
                None => open_pos = Some(pos),
                Some(open) => {
                    if start != open {
                        result.extend(&cs[start..open]);
                    }
                    start = pos + 1;
                    open_pos = None;
                }
            }
        }
    }
    if start < cs.len() {
        result.extend(&cs[start..]);
    }
    result
}

/// `_check_parenthesis(addr)`：引号外面的括号成不成对
fn check_parenthesis(addr: &str) -> bool {
    let stripped: Vec<char> = strip_quoted_realnames(addr).chars().collect();
    let mut opens: i64 = 0;
    for (_, ch) in escaped_chars(&stripped) {
        if ch == Some('(') {
            opens += 1;
        } else if ch == Some(')') {
            opens -= 1;
            if opens < 0 {
                return false;
            }
        }
    }
    opens == 0
}

/// 括号不成对的那一条换成这个（解析出来是一个空地址）
const REJECTED: &str = "('', '')";

fn pre_parse(v: &str) -> String {
    if check_parenthesis(v) {
        v.to_string()
    } else {
        REJECTED.to_string()
    }
}

/// 解析完地址里还有 `[` 的（域名的方括号没读对）不认
fn post_parse(list: Vec<(String, String)>) -> Vec<(String, String)> {
    list.into_iter().map(|v| if v.1.contains('[') { (String::new(), String::new()) } else { v }).collect()
}

/// `email.utils.getaddresses(fieldvalues)`（严格）：[(显示名, 地址)]；解析出来的个数和逗号数对不上时整个不认，
/// 只给一个空的（`alice@example.com <bob@example.com>` 这种写法想冒充两个地址）
pub fn getaddresses(fieldvalues: &[String]) -> R<Vec<(String, String)>> {
    let fieldvalues: Vec<String> = fieldvalues.iter().map(|v| pre_parse(v)).collect();
    let result = post_parse(address_list(&fieldvalues.join(", "))?);
    let n: usize = fieldvalues.iter().map(|v| 1 + strip_quoted_realnames(v).matches(',').count()).sum();
    if result.len() != n {
        return Ok(vec![(String::new(), String::new())]);
    }
    Ok(result)
}

/// `email.utils.parseaddr(addr)`（严格）：(显示名, 地址)；不是正好一个地址时是两个空串
pub fn parseaddr(addr: &str) -> R<(String, String)> {
    let mut addrs = post_parse(address_list(&pre_parse(addr))?);
    if addrs.len() != 1 {
        return Ok((String::new(), String::new()));
    }
    Ok(addrs.remove(0))
}

/// `mail_addrs(msg, name)`：这个头里的各个地址，小写
pub fn mail_addrs(msg: &super::message::Message, name: &str) -> R<Vec<String>> {
    let values: Vec<String> = msg.get_all(name).into_iter().map(super::message::hstr).collect();
    Ok(getaddresses(&values)?.into_iter().filter(|(_, a)| a.contains('@')).map(|(_, a)| a.to_lowercase()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::mail::testing::vectors;
    use crate::py::obj::V;

    #[test]
    fn header_text_like_python() {
        let cases = vectors("text");
        assert!(cases.len() > 300);
        let mut divergent = 0;
        for c in cases {
            let input = c.at("in").py_str();
            // Python 认识、这里不认识的字符集：这里照「解不开用原样的」，结果记在 out_unknown 里
            if let Some(alt) = c.get("out_unknown") {
                divergent += 1;
                assert_eq!(mail_text_str(&input), alt.py_str(), "{input:?}（字符集这里不认识）");
            } else {
                assert_eq!(mail_text_str(&input), c.at("out").py_str(), "{input:?}");
            }
        }
        assert!(divergent > 3);
    }

    #[test]
    fn addresses_like_python() {
        let cases = vectors("addresses");
        assert!(cases.len() > 300);
        for c in cases {
            let values: Vec<String> = c.at("values").as_list().unwrap().strs().iter().map(|s| s.to_string()).collect();
            let got = getaddresses(&values);
            match c.get("error") {
                Some(want) => assert!(matches!(&got, Err(Fail::Other(class, _)) if *class == want.py_str()), "{values:?} {got:?}"),
                None => {
                    let pairs = |list: Vec<(String, String)>| V::from(list.into_iter().map(|(n, a)| V::from(crate::list![n, a])).collect::<Vec<_>>());
                    let got = pairs(got.unwrap());
                    assert!(got.py_eq(&c.at("pairs")), "{values:?}\n  这里：{got:?}\n  Python：{:?}", c.at("pairs"));
                    // 严格的检查之前，解析器自己切出来的
                    let loose = pairs(address_list(&values.join(", ")).unwrap());
                    assert!(loose.py_eq(&c.at("loose")), "{values:?}（不严格）\n  这里：{loose:?}\n  Python：{:?}", c.at("loose"));
                }
            }
            if values.len() == 1 {
                let (name, addr) = parseaddr(&values[0]).unwrap_or_default();
                let want = c.at("parseaddr").as_list().unwrap();
                assert_eq!((name, addr), (want.at(0).py_str(), want.at(1).py_str()), "parseaddr {:?}", values[0]);
            }
        }
    }

    /// 注释、组能套几层：和 Python 在 cca mail test 那条路上量出来的一样（tools/gen_mail_vectors.py 的 measure_limits）
    #[test]
    fn nesting_limit_like_python() {
        let nest = |kind: &str, n: usize| -> String {
            let (pre, open, close, post) = match kind {
                "comment" => ("", "(", ")", " a@example.com"),
                "domain comment" => ("a@", "(", ")", "b.example.com"),
                "phrase comment" => ("x ", "(", ")", " <a@example.com>"),
                "route comment" => ("<", "(", ")", "a@example.com>"),
                "group comment" => ("g: ", "(", ")", "a@example.com;"),
                "group" => ("", "a:", "", ""),
                other => panic!("{other}"),
            };
            format!("{pre}{}{}{post}", open.repeat(n), close.repeat(n))
        };
        let cases: Vec<_> = vectors("limits").into_iter().filter(|c| c.at("what").py_str() == "address").collect();
        assert!(cases.len() >= 6);
        for c in cases {
            let (kind, max) = (c.at("kind").py_str(), c.at("max").as_int().unwrap() as usize);
            assert!(getaddresses(&[nest(&kind, max)]).is_ok(), "{kind} 套 {max} 层");
            assert!(matches!(getaddresses(&[nest(&kind, max + 1)]), Err(Fail::Other("RecursionError", _))), "{kind} 套 {} 层", max + 1);
        }
    }

    #[test]
    fn deep_comments_are_an_error_not_a_crash() {
        let deep = format!("{}{} a@example.com", "(".repeat(5000), ")".repeat(5000));
        assert!(matches!(getaddresses(&[deep]), Err(Fail::Other("RecursionError", _))));
        let groups = "a:".repeat(5000);
        assert!(matches!(getaddresses(&[groups]), Err(Fail::Other("RecursionError", _))));
        let fine = format!("{}{} a@example.com", "(".repeat(100), ")".repeat(100));
        assert_eq!(getaddresses(&[fine]).unwrap()[0].1, "a@example.com");
    }
}
