//! 邮件的解析：原来是 `email.message_from_bytes`（compat32 的 FeedParser）和 `email.message.Message` 的那几个方法。
//! 这里照 CPython 3.14 的行为手写、只移植邮箱助手用到的部分：头（折行、同名的头）、multipart / message 的结构、
//! Content-Type 的参数（含 RFC 2231 的续行和编码）、Content-Transfer-Encoding、`walk()`。
//!
//! Python 把邮件的字节按 ASCII 读成文字，非 ASCII 的字节留成落单的代理项（surrogateescape）；这里头的名字、值和正文
//! 一直是字节。`hstr` 是 `str(msg.get(…))`：非 ASCII 的字节各成一个 U+FFFD（compat32 把带这种字节的头包成
//! unknown-8bit 的 Header，`str()` 就是这个结果）。
//!
//! 原来靠 Python 的调用栈上限（一千层）挡住套得太深的邮件，那时是没人接的 RecursionError；这里照量出来的层数（`MAX_DEPTH`）。

use super::codec::{self, int_too_long, raw_unicode_escape, Lookup, INT_MAX_STR_DIGITS};
use crate::py::text;
use crate::py::{Fail, R};
use std::collections::VecDeque;

/// multipart / message 最多套这么多层：再深 Python 那边是 RecursionError。这个数是 `cca mail test` 取全文那条路上的
/// （cmd_mail_test → mail_verdict → mail_parse，对照数据的 limits 一节）；原来每多隔一层函数就少一层（CPython 3.14.0 上实测：
/// 子命令里直接调 mail_parse 984 层，隔着四层 980 层），照 `py_depth` 算
pub const MAX_DEPTH: usize = 983;

#[derive(Debug, Clone, PartialEq)]
pub enum Payload {
    /// 不是容器：正文的字节（还没按 Content-Transfer-Encoding 解）
    Text(Vec<u8>),
    /// multipart 的各部分，或者 message/* 里包着的那封邮件
    Parts(Vec<Message>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    /// 头，照出现的先后：(名字, 值)，值里折行的换行留着
    pub headers: Vec<(Vec<u8>, Vec<u8>)>,
    pub payload: Payload,
    /// 没有 Content-Type 时算什么类型（multipart/digest 里的部分是 message/rfc822）
    default_type: &'static str,
}

/// Content-Type 的一个参数的值：普通的，或者 RFC 2231 编码过的 (字符集, 语言, 值)
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    Plain(String),
    Extended(Option<String>, Option<String>, String),
}

/// `str(msg.get(名字))`：头的值当文字，非 ASCII 的字节各换成一个 U+FFFD
pub fn hstr(value: &[u8]) -> String {
    codec::ascii_replace(value)
}

fn recursion_error<T>() -> R<T> {
    Err(Fail::Other("RecursionError", vec!["maximum recursion depth exceeded".into()]))
}

/// `utils.unquote(s)`：去掉两头的双引号（里面的转义还原）或尖括号
fn unquote(s: &str) -> String {
    if text::len(s) > 1 {
        if s.starts_with('"') && s.ends_with('"') {
            return s[1..s.len() - 1].replace("\\\\", "\\").replace("\\\"", "\"");
        }
        if s.starts_with('<') && s.ends_with('>') {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

/// `_parseaddr.quote(s)`：反斜杠和双引号前面加反斜杠
fn quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// `urllib.parse.unquote(s, encoding='latin-1')`：`%XX` 换成那个字节对应的字符，不成对的原样留着
fn percent_latin1(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < cs.len() {
        if cs[i] == '%' && i + 2 < cs.len() && cs[i + 1].is_ascii_hexdigit() && cs[i + 2].is_ascii_hexdigit() {
            let byte = cs[i + 1].to_digit(16).unwrap_or(0) * 16 + cs[i + 2].to_digit(16).unwrap_or(0);
            out.push(char::from_u32(byte).unwrap_or('\u{fffd}'));
            i += 3;
        } else {
            out.push(cs[i]);
            i += 1;
        }
    }
    out
}

/// `_splitparam(value)[0]`：第一个分号之前的那一段，去掉两头的空白
fn splitparam_head(value: &str) -> &str {
    text::strip(value.split(';').next().unwrap_or(""))
}

/// `_parseparam(value)`：按分号切开（引号里的分号不算），`名字=值` 的名字转小写、两边去空白
fn parseparam(value: &str) -> Vec<String> {
    let mut s: &str = &format!(";{value}");
    let mut plist = Vec::new();
    while s.starts_with(';') {
        s = &s[1..];
        let count = |hay: &str, needle: &str| hay.matches(needle).count();
        let mut end = s.find(';');
        while let Some(e) = end {
            if e > 0 && (count(&s[..e], "\"") - count(&s[..e], "\\\"")) % 2 == 1 {
                end = s[e + 1..].find(';').map(|i| i + e + 1);
            } else {
                break;
            }
        }
        let end = end.unwrap_or(s.len());
        let f = &s[..end];
        let f = match f.find('=') {
            Some(i) => format!("{}={}", text::strip(&f[..i]).to_lowercase(), text::strip(&f[i + 1..])),
            None => f.to_string(),
        };
        plist.push(text::strip(&f).to_string());
        s = &s[end..];
    }
    plist
}

/// 续行的编号（`int(num)`）：位数可以很多，只比大小、看是不是 0。去掉开头的 0 以后按（位数，数字）比
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Num(usize, String);

impl Num {
    fn parse(digits: &str) -> R<Num> {
        if digits.len() > INT_MAX_STR_DIGITS {
            return Err(int_too_long(digits.len()));
        }
        let d = digits.trim_start_matches('0');
        Ok(Num(d.len(), d.to_string()))
    }

    fn zero() -> Num {
        Num(0, String::new())
    }
}

/// `rfc2231_continuation.match(name)`：`名字*`、`名字*数字`、`名字*数字*`（名字是 ASCII 的字母、数字、下划线）→ (名字, 数字)
fn continuation(name: &str) -> Option<(&str, Option<&str>)> {
    let star = name.find('*')?;
    let (base, rest) = (&name[..star], &name[star + 1..]);
    if base.is_empty() || !base.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    if rest.is_empty() {
        return Some((base, None));
    }
    let digits = rest.strip_suffix('*').unwrap_or(rest);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((base, Some(digits)))
}

/// `utils.decode_params(params)`：RFC 2231 的续行接起来、编码过的段解开
fn decode_params(params: Vec<(String, String)>) -> R<Vec<(String, Param)>> {
    let mut it = params.into_iter();
    let mut new_params: Vec<(String, Param)> = Vec::new();
    if let Some((name, value)) = it.next() {
        new_params.push((name, Param::Plain(value)));
    }
    // 名字 → [(编号, 值, 是不是编码过的)]，名字照第一次出现的先后
    let mut rfc2231: Vec<(String, Vec<(Option<Num>, String, bool)>)> = Vec::new();
    for (name, value) in it {
        let encoded = name.ends_with('*');
        let value = unquote(&value);
        match continuation(&name) {
            Some((base, num)) => {
                let num = num.map(Num::parse).transpose()?;
                match rfc2231.iter_mut().find(|(n, _)| n == base) {
                    Some((_, list)) => list.push((num, value, encoded)),
                    None => rfc2231.push((base.to_string(), vec![(num, value, encoded)])),
                }
            }
            None => new_params.push((name, Param::Plain(format!("\"{}\"", quote(&value))))),
        }
    }
    for (name, continuations) in rfc2231 {
        // 没写编号的当 0；已经有 0 号的话没写编号的不要
        let has_zero = continuations.iter().any(|x| x.0.as_ref() == Some(&Num::zero()));
        let mut list: Vec<(Num, String, bool)> = if has_zero {
            continuations.into_iter().filter_map(|(n, s, e)| n.map(|n| (n, s, e))).collect()
        } else {
            continuations.into_iter().map(|(n, s, e)| (n.unwrap_or_else(Num::zero), s, e)).collect()
        };
        list.sort_by(|a, b| a.0.cmp(&b.0));
        let mut value = String::new();
        let mut extended = false;
        for (_, s, encoded) in list {
            if encoded {
                value.push_str(&percent_latin1(&s));
                extended = true;
            } else {
                value.push_str(&s);
            }
        }
        let value = quote(&value);
        if extended {
            // decode_rfc2231：字符集'语言'值
            let parts: Vec<&str> = value.splitn(3, '\'').collect();
            let param = if parts.len() <= 2 {
                Param::Extended(None, None, format!("\"{value}\""))
            } else {
                Param::Extended(Some(parts[0].to_string()), Some(parts[1].to_string()), format!("\"{}\"", parts[2]))
            };
            new_params.push((name, param));
        } else {
            new_params.push((name, Param::Plain(format!("\"{value}\""))));
        }
    }
    Ok(new_params)
}

/// 行首是不是换行（`NLCRE.match(line)`）
fn starts_with_newline(line: &[u8]) -> bool {
    matches!(line.first(), Some(b'\r' | b'\n'))
}

/// 去掉结尾的一个换行（`NLCRE_eol`：`\r\n`、`\r`、`\n`）
fn strip_eol(s: &mut Vec<u8>) {
    if s.ends_with(b"\r\n") {
        s.truncate(s.len() - 2);
    } else if s.ends_with(b"\r") || s.ends_with(b"\n") {
        s.truncate(s.len() - 1);
    }
}

/// `headerRE`：`From `、「可见的 ASCII（不含冒号）若干个 + 冒号」、空格或制表符开头
fn is_header_line(line: &[u8]) -> bool {
    if line.starts_with(b"From ") || matches!(line.first(), Some(b'\t' | b' ')) {
        return true;
    }
    for &b in line {
        if b == b':' {
            return true;
        }
        if !(0x21..=0x7e).contains(&b) {
            return false;
        }
    }
    false
}

/// 按 `\r\n`、`\r`、`\n` 切成行，换行留在行尾（`StringIO(newline='').readlines()`）
fn split_keep_ends(raw: &[u8]) -> VecDeque<Vec<u8>> {
    let mut out = VecDeque::new();
    let (mut start, mut i) = (0, 0);
    while i < raw.len() {
        match raw[i] {
            b'\n' => {
                i += 1;
                out.push_back(raw[start..i].to_vec());
                start = i;
            }
            b'\r' => {
                i += if raw.get(i + 1) == Some(&b'\n') { 2 } else { 1 };
                out.push_back(raw[start..i].to_vec());
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < raw.len() {
        out.push_back(raw[start..].to_vec());
    }
    out
}

/// 一行算不算读到头了（`push_eof_matcher` 的那两种）
#[derive(Clone)]
enum Eof {
    /// 空行（message/delivery-status 的各段之间）
    Blank,
    /// multipart 的分隔行：`--边界`，后面可以有 `--`、空格、换行。边界里有非 ASCII 的字符时哪一行都对不上（None）
    Boundary(Option<Vec<u8>>),
}

/// `boundarymatch(line)`：是分隔行时给出 (是不是结束的那一条, 后面有没有换行)
fn boundary_match(separator: &Option<Vec<u8>>, line: &[u8]) -> Option<(bool, bool)> {
    let rest = line.strip_prefix(separator.as_ref()?.as_slice())?;
    // boundaryendRE：(--)?[ \t]*(\r\n|\r|\n)?$
    let tail_ok = |rest: &[u8]| -> Option<bool> {
        let ws = rest.iter().take_while(|b| matches!(b, b' ' | b'\t')).count();
        match &rest[ws..] {
            [] => Some(false),
            b"\r\n" | b"\r" | b"\n" => Some(true),
            _ => None,
        }
    };
    if let Some(after) = rest.strip_prefix(b"--") {
        if let Some(linesep) = tail_ok(after) {
            return Some((true, linesep));
        }
    }
    tail_ok(rest).map(|linesep| (false, linesep))
}

impl Eof {
    fn hit(&self, line: &[u8]) -> bool {
        match self {
            Eof::Blank => starts_with_newline(line),
            Eof::Boundary(sep) => boundary_match(sep, line).is_some(),
        }
    }
}

/// 解析时的一封邮件（放在 `Parser::nodes` 里，互相用下标指）
struct Node {
    headers: Vec<(Vec<u8>, Vec<u8>)>,
    /// None：还没定（`_payload is None`）
    text: Option<Vec<u8>>,
    parts: Option<Vec<usize>>,
    default_type: &'static str,
}

struct Parser {
    /// 还没读的行。空的一项是被放回来的「读到头了」（原来的 `unreadline('')`）：读到它算一次读到头
    lines: VecDeque<Vec<u8>>,
    eofstack: Vec<Eof>,
    nodes: Vec<Node>,
    msgstack: Vec<usize>,
    cur: Option<usize>,
    last: usize,
    /// 最多套几层
    max_depth: usize,
}

impl Parser {
    /// `self._input.readline()`：None 是读到头了（没有行了，或者下一行是外面哪一层的分隔行）
    fn readline(&mut self) -> Option<Vec<u8>> {
        let line = self.lines.pop_front()?;
        if line.is_empty() {
            return None;
        }
        if self.eofstack.iter().rev().any(|m| m.hit(&line)) {
            self.lines.push_front(line);
            return None;
        }
        Some(line)
    }

    fn unreadline(&mut self, line: Vec<u8>) {
        self.lines.push_front(line);
    }

    fn view(&self, i: usize) -> View<'_> {
        View { headers: &self.nodes[i].headers, default_type: self.nodes[i].default_type }
    }

    fn new_message(&mut self) {
        let mut node = Node { headers: Vec::new(), text: None, parts: None, default_type: "text/plain" };
        // `if self._cur and …`：一个头都没有的邮件算假（Message 有 __len__）
        if let Some(cur) = self.cur {
            if !self.nodes[cur].headers.is_empty() && self.view(cur).content_type() == "multipart/digest" {
                node.default_type = "message/rfc822";
            }
        }
        let i = self.nodes.len();
        self.nodes.push(node);
        if let Some(&top) = self.msgstack.last() {
            self.nodes[top].parts.get_or_insert_with(Vec::new).push(i);
        }
        self.msgstack.push(i);
        self.cur = Some(i);
        self.last = i;
    }

    fn pop_message(&mut self) {
        self.msgstack.pop();
        self.cur = self.msgstack.last().copied();
    }

    /// `_parse_headers(lines)`
    fn parse_headers(&mut self, lines: Vec<Vec<u8>>) {
        let cur = self.cur.expect("解析头的时候总有一封当前的邮件");
        let count = lines.len();
        let mut lastvalue: Vec<Vec<u8>> = Vec::new(); // 空的就是 lastheader == ''
        for (lineno, line) in lines.into_iter().enumerate() {
            if matches!(line[0], b' ' | b'\t') {
                if !lastvalue.is_empty() {
                    lastvalue.push(line);
                }
                continue; // 前面没有头的续行不要
            }
            if !lastvalue.is_empty() {
                let header = source_parse(std::mem::take(&mut lastvalue));
                self.nodes[cur].headers.push(header);
            }
            if line.starts_with(b"From ") {
                if lineno == 0 {
                    continue; // 信封上的那一行
                }
                if lineno == count - 1 {
                    self.unreadline(line); // 最后一行：其实是正文
                    return;
                }
                continue;
            }
            if line.first() == Some(&b':') {
                continue; // 没有名字
            }
            lastvalue = vec![line];
        }
        if !lastvalue.is_empty() {
            let header = source_parse(lastvalue);
            self.nodes[cur].headers.push(header);
        }
    }

    /// 读到头为止的各行接起来
    fn rest(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some(line) = self.readline() {
            out.extend_from_slice(&line);
        }
        out
    }

    /// `_parsegen()`：解析一封邮件（新建一封、接到当前那封下面）
    fn parsegen(&mut self, depth: usize) -> R<()> {
        if depth > self.max_depth {
            return recursion_error();
        }
        self.new_message();
        let cur = self.cur.expect("刚建的");
        let mut headers = Vec::new();
        while let Some(line) = self.readline() {
            if !is_header_line(&line) {
                // 头和正文之间的空行不要；别的（没有空行就开始了正文）放回去
                if !starts_with_newline(&line) {
                    self.unreadline(line);
                }
                break;
            }
            headers.push(line);
        }
        self.parse_headers(headers);
        let ctype = self.view(cur).content_type();
        if ctype == "message/delivery-status" {
            // 一段一段的头，段和段之间是空行
            loop {
                self.eofstack.push(Eof::Blank);
                let sub = self.parsegen(depth + 1);
                self.pop_message();
                self.eofstack.pop();
                sub?;
                self.readline();
                match self.readline() {
                    None => break,
                    Some(line) => self.unreadline(line),
                }
            }
            return Ok(());
        }
        let maintype = ctype.split('/').next().unwrap_or("");
        if maintype == "message" {
            let sub = self.parsegen(depth + 1);
            self.pop_message();
            return sub;
        }
        if maintype == "multipart" {
            let Some(boundary) = self.view(cur).boundary()? else {
                let all = self.rest();
                self.nodes[cur].text = Some(all);
                return Ok(());
            };
            // 行是「ASCII + 落单的代理项」，边界里别的非 ASCII 字符哪一行都对不上
            let separator = boundary.is_ascii().then(|| format!("--{boundary}").into_bytes());
            let mut capturing_preamble = true;
            let mut preamble: Vec<u8> = Vec::new();
            let mut close_boundary_seen = false;
            while let Some(line) = self.readline() {
                match boundary_match(&separator, &line) {
                    Some((true, _)) => {
                        close_boundary_seen = true;
                        break;
                    }
                    Some((false, _)) => {
                        if capturing_preamble {
                            capturing_preamble = false;
                            self.unreadline(line);
                            continue;
                        }
                        // 连着的几条分隔行算一条
                        loop {
                            match self.readline() {
                                Some(next) if boundary_match(&separator, &next).is_some() => {}
                                Some(next) => {
                                    self.unreadline(next);
                                    break;
                                }
                                None => {
                                    self.unreadline(Vec::new()); // 原来就是把空串放了回去
                                    break;
                                }
                            }
                        }
                        self.eofstack.push(Eof::Boundary(separator.clone()));
                        let sub = self.parsegen(depth + 1);
                        if sub.is_ok() && self.view(self.last).content_maintype() != "multipart" {
                            // 分隔行前面的那个换行不算这一部分的正文
                            if let Some(payload) = self.nodes[self.last].text.as_mut() {
                                strip_eol(payload);
                            }
                        }
                        self.eofstack.pop();
                        self.pop_message();
                        self.last = cur;
                        sub?;
                    }
                    None => {
                        if !capturing_preamble {
                            return Err(Fail::Other("AssertionError", vec![String::new()]));
                        }
                        preamble.extend_from_slice(&line);
                    }
                }
            }
            if capturing_preamble {
                // 一条分隔行都没找到：前面读到的当正文，剩下的不要
                self.nodes[cur].text = Some(preamble);
                self.rest();
                return Ok(());
            }
            if close_boundary_seen {
                self.rest(); // 结束的分隔行后面的（epilogue）
            }
            return Ok(());
        }
        let all = self.rest();
        self.nodes[cur].text = Some(all);
        Ok(())
    }

    fn build(&mut self, i: usize) -> Message {
        let parts = self.nodes[i].parts.take();
        let payload = match parts {
            Some(parts) => Payload::Parts(parts.into_iter().map(|p| self.build(p)).collect()),
            None => Payload::Text(self.nodes[i].text.take().unwrap_or_default()),
        };
        Message { headers: std::mem::take(&mut self.nodes[i].headers), payload, default_type: self.nodes[i].default_type }
    }
}

/// compat32 的 `header_source_parse`：名字是冒号之前的，值是后面的各行接起来、去掉开头的空白和结尾的换行
fn source_parse(lines: Vec<Vec<u8>>) -> (Vec<u8>, Vec<u8>) {
    let first = &lines[0];
    let colon = first.iter().position(|b| *b == b':').unwrap_or(first.len());
    let name = first[..colon].to_vec();
    let mut value: Vec<u8> = first.get(colon + 1..).unwrap_or(&[]).to_vec();
    for line in &lines[1..] {
        value.extend_from_slice(line);
    }
    let start = value.iter().position(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n')).unwrap_or(value.len());
    let end = value.iter().rposition(|b| !matches!(b, b'\r' | b'\n')).map_or(start, |e| e + 1).max(start);
    (name, value[start..end].to_vec())
}

/// 只看头就能答的那些（解析到一半时也要用）
struct View<'a> {
    headers: &'a [(Vec<u8>, Vec<u8>)],
    default_type: &'static str,
}

impl<'a> View<'a> {
    fn get(&self, name: &str) -> Option<&'a [u8]> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name.as_bytes())).map(|(_, v)| v.as_slice())
    }

    fn content_type(&self) -> String {
        let Some(value) = self.get("content-type") else {
            return self.default_type.to_string();
        };
        let ctype = splitparam_head(&hstr(value)).to_lowercase();
        // RFC 2045：写得不对的当 text/plain
        if ctype.matches('/').count() != 1 {
            return "text/plain".to_string();
        }
        ctype
    }

    fn content_maintype(&self) -> String {
        self.content_type().split('/').next().unwrap_or("").to_string()
    }

    /// `msg.get_param(param, header=…)`（值已经去掉引号）
    fn param(&self, param: &str, header: &str) -> R<Option<Param>> {
        let Some(value) = self.get(header) else {
            return Ok(None);
        };
        let mut params = Vec::new();
        for p in parseparam(&hstr(value)) {
            match p.split_once('=') {
                Some((name, val)) => params.push((text::strip(name).to_string(), text::strip(val).to_string())),
                None => params.push((text::strip(&p).to_string(), String::new())),
            }
        }
        for (k, v) in decode_params(params)? {
            if k.to_lowercase() == param.to_lowercase() {
                return Ok(Some(match v {
                    Param::Plain(s) => Param::Plain(unquote(&s)),
                    Param::Extended(cs, lang, s) => Param::Extended(cs, lang, unquote(&s)),
                }));
            }
        }
        Ok(None)
    }

    /// `msg.get_boundary()`
    fn boundary(&self) -> R<Option<String>> {
        let value = match self.param("boundary", "content-type")? {
            None => return Ok(None),
            Some(Param::Plain(s)) => unquote(&s),
            // collapse_rfc2231_value：照写着的字符集解（没写按 us-ascii），Python 不认识这个字符集时原样用
            Some(Param::Extended(charset, _, text)) => match codec::lookup(charset.as_deref().unwrap_or("us-ascii")) {
                Lookup::Known(c) => c.decode_replace(&raw_unicode_escape(&text)),
                Lookup::Unknown => unquote(&text),
                Lookup::Null => return Err(Fail::Value("embedded null character".into())),
            },
        };
        Ok(Some(text::rstrip(&value).to_string()))
    }
}

impl Message {
    fn view(&self) -> View<'_> {
        View { headers: &self.headers, default_type: self.default_type }
    }

    /// `msg.get(名字)`：第一个叫这个名字的头（名字不分大小写）的值
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.view().get(name)
    }

    /// `msg.get_all(名字) or []`
    pub fn get_all(&self, name: &str) -> Vec<&[u8]> {
        self.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case(name.as_bytes())).map(|(_, v)| v.as_slice()).collect()
    }

    /// `str(msg.get(名字) or '')`
    pub fn get_str(&self, name: &str) -> String {
        self.get(name).map(hstr).unwrap_or_default()
    }

    pub fn is_multipart(&self) -> bool {
        matches!(self.payload, Payload::Parts(_))
    }

    pub fn get_content_type(&self) -> String {
        self.view().content_type()
    }

    /// `msg.get_content_disposition()`
    pub fn get_content_disposition(&self) -> Option<String> {
        self.get("content-disposition").map(|v| splitparam_head(&hstr(v)).to_lowercase())
    }

    /// `msg.get_content_charset()`：Content-Type 的 charset 参数（小写）；没有、不是 ASCII 的是 None
    pub fn get_content_charset(&self) -> R<Option<String>> {
        let charset = match self.view().param("charset", "content-type")? {
            None => return Ok(None),
            Some(Param::Plain(s)) => s,
            // RFC 2231 编码过的：照它说的字符集解，解不了、不认识就用原样的
            Some(Param::Extended(pcharset, _, text)) => {
                let pcharset = pcharset.filter(|p| !p.is_empty()).unwrap_or_else(|| "us-ascii".to_string());
                match codec::lookup(&pcharset) {
                    Lookup::Known(c) => c.decode_strict(&raw_unicode_escape(&text)).unwrap_or(text),
                    Lookup::Unknown => text,
                    Lookup::Null => return Err(Fail::Value("embedded null character".into())),
                }
            }
        };
        if !charset.is_ascii() {
            return Ok(None);
        }
        Ok(Some(charset.to_lowercase()))
    }

    /// `msg.get_payload(decode=True)`：正文按 Content-Transfer-Encoding 解开；容器是 None
    pub fn get_payload_decoded(&self) -> Option<Vec<u8>> {
        let Payload::Text(payload) = &self.payload else {
            return None;
        };
        let cte = text::strip(&self.get_str("content-transfer-encoding")).to_lowercase();
        Some(codec::decode_transfer(&cte, payload))
    }

    /// `msg.walk()`：自己和下面各层的各部分，先上后下
    pub fn walk(&self) -> Vec<&Message> {
        let mut out = Vec::new();
        let mut stack = vec![self];
        while let Some(m) = stack.pop() {
            out.push(m);
            if let Payload::Parts(parts) = &m.payload {
                stack.extend(parts.iter().rev());
            }
        }
        out
    }
}

/// `mail_parse(raw)`：`email.message_from_bytes(raw or b'')`。Python 那边会出没人接的异常的几种情况照样出错：
/// 套得太深（RecursionError）、参数续行的编号太长、字符集的名字里有 NUL（ValueError）
pub fn mail_parse(raw: &[u8]) -> R<Message> {
    // MAX_DEPTH 是隔着一层（mail_verdict）调的时候的数
    let max_depth = (MAX_DEPTH + 1).saturating_sub(super::py_depth());
    let parse = || {
        let mut p = Parser { lines: split_keep_ends(raw), eofstack: Vec::new(), nodes: Vec::new(), msgstack: Vec::new(), cur: None, last: 0, max_depth };
        p.parsegen(0)?;
        Ok(p.build(0))
    };
    // 每套一层要有一个 Content-Type 头：多得出奇的才可能套得很深，放到栈够大的线程里解析
    let types = raw.windows(12).filter(|w| w.eq_ignore_ascii_case(b"content-type")).count();
    if types > 40 {
        super::with_deep_stack(parse)
    } else {
        parse()
    }
}

/// 照 UTF-8 解的字符集（`get_content_charset() or 'utf-8'` 再 `decode(…, 'replace')`，不认识的字符集也照 UTF-8）
pub fn decode_text(part: &Message, raw: &[u8]) -> R<String> {
    codec::decode_body(part.get_content_charset()?.as_deref(), raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::mail::testing::{bytes_of, vectors};
    use crate::py::obj::V;

    fn class_of(e: &Fail) -> &'static str {
        match e {
            Fail::Value(_) => "ValueError",
            Fail::Other(name, _) => name,
            _ => "?",
        }
    }

    /// 每一部分：是不是容器、类型、Content-Disposition、字符集、解开的正文
    fn describe(m: &Message) -> Result<Vec<V>, &'static str> {
        let mut out = Vec::new();
        for part in m.walk() {
            let charset = match part.get_content_charset() {
                Ok(c) => V::from(c),
                Err(e) => V::from(format!("!{}", class_of(&e))),
            };
            out.push(V::Dict(crate::dict! {
                "multipart" => part.is_multipart(),
                "type" => part.get_content_type(),
                "disposition" => part.get_content_disposition(),
                "charset" => charset,
                "payload" => part.get_payload_decoded().map(|p| codec::latin1(&p)),
            }));
        }
        Ok(out)
    }

    #[test]
    fn structure_like_python() {
        let cases = vectors("message");
        assert!(cases.len() > 150);
        for c in cases {
            let raw = bytes_of(&c.at("raw"));
            let what = codec::bytes_repr(&raw);
            match (mail_parse(&raw), c.get("error")) {
                (Err(e), Some(want)) => assert_eq!(class_of(&e), want.py_str(), "{what}"),
                (Ok(_), Some(want)) => panic!("Python 出了 {want:?}，这里没出错：{what}"),
                (Err(e), None) => panic!("Python 没出错，这里出了 {e:?}：{what}"),
                (Ok(m), None) => {
                    let got = V::from(describe(&m).unwrap());
                    assert!(got.py_eq(&c.at("parts")), "{what}\n  这里：{got:?}\n  Python：{:?}", c.at("parts"));
                    let headers = V::from(m.headers.iter().map(|(k, v)| V::from(crate::list![codec::latin1(k), hstr(v)])).collect::<Vec<_>>());
                    assert!(headers.py_eq(&c.at("headers")), "{what}\n  这里：{headers:?}\n  Python：{:?}", c.at("headers"));
                }
            }
        }
    }

    #[test]
    fn lines_and_boundaries() {
        let lines: Vec<Vec<u8>> = split_keep_ends(b"a\r\nb\rc\n\nd").into_iter().collect();
        assert_eq!(lines, [b"a\r\n".to_vec(), b"b\r".to_vec(), b"c\n".to_vec(), b"\n".to_vec(), b"d".to_vec()]);
        let sep = Some(b"--b".to_vec());
        assert_eq!(boundary_match(&sep, b"--b\r\n"), Some((false, true)));
        assert_eq!(boundary_match(&sep, b"--b--"), Some((true, false)));
        assert_eq!(boundary_match(&sep, b"--b-- \t\n"), Some((true, true)));
        assert_eq!(boundary_match(&sep, b"--b \r"), Some((false, true)));
        assert_eq!(boundary_match(&sep, b"--bx\r\n"), None);
        assert_eq!(boundary_match(&sep, b"--b--x\r\n"), None);
        assert_eq!(boundary_match(&sep, b"--b - -\r\n"), None);
        assert_eq!(boundary_match(&None, b"--b\r\n"), None);
        assert!(is_header_line(b"From x") && is_header_line(b":x") && is_header_line(b"A-b:") && is_header_line(b" x"));
        assert!(!is_header_line(b"a b: c") && !is_header_line(b"\r\n") && !is_header_line(b"abc") && !is_header_line(b"\xe4:"));
    }

    /// 能套几层：和 Python 在 cca mail test 那条路上量出来的一样（tools/gen_mail_vectors.py 的 measure_limits）
    #[test]
    fn nesting_limit_like_python() {
        let nest = |kind: &str, n: usize| -> String {
            let mut raw = String::new();
            for i in 0..n {
                match kind {
                    "rfc822" => raw.push_str("Content-Type: message/rfc822\r\n\r\n"),
                    _ => raw.push_str(&format!("Content-Type: multipart/mixed; boundary=b{i}\r\n\r\n--b{i}\r\n")),
                }
            }
            raw + if kind == "rfc822" { "Subject: x\r\n\r\nhi\r\n" } else { "\r\nhi\r\n" }
        };
        let cases: Vec<_> = vectors("limits").into_iter().filter(|c| c.at("what").py_str() == "message").collect();
        assert!(cases.len() >= 2);
        // 对照数据是隔着一层函数量的（相当于 cmd_mail_test → mail_verdict → mail_parse）
        let ok = |kind: &str, n: usize| mail_parse(nest(kind, n).as_bytes());
        for c in cases {
            let (kind, max) = (c.at("kind").py_str(), c.at("max").as_int().unwrap() as usize);
            let _one = crate::cmd::mail::py_frame();
            assert_eq!(ok(&kind, max).unwrap().walk().len(), max + 1, "{kind} 套 {max} 层");
            assert!(matches!(ok(&kind, max + 1), Err(Fail::Other("RecursionError", _))), "{kind} 套 {} 层", max + 1);
        }
        // 各条路上实测的（对照数据的 limit_paths 一节）：每多隔一层函数少一层
        let paths: Vec<_> = vectors("limit_paths").into_iter().filter(|c| c.at("what").py_str() == "message").collect();
        assert!(paths.len() >= 8);
        for c in paths {
            let (kind, max, depth) = (c.at("kind").py_str(), c.at("max").as_int().unwrap() as usize, c.at("depth").as_int().unwrap());
            let _held: Vec<_> = (0..depth).map(|_| crate::cmd::mail::py_frame()).collect();
            assert!(ok(&kind, max).is_ok(), "{}：{kind} 套 {max} 层", c.at("path").py_str());
            assert!(matches!(ok(&kind, max + 1), Err(Fail::Other("RecursionError", _))), "{}：{kind} 套 {} 层", c.at("path").py_str(), max + 1);
        }
        assert_eq!(crate::cmd::mail::py_depth(), 0);
    }

    #[test]
    fn too_deep_is_an_error_not_a_crash() {
        let mut raw = String::new();
        for i in 0..MAX_DEPTH + 5 {
            raw.push_str(&format!("Content-Type: multipart/mixed; boundary=b{i}\r\n\r\n--b{i}\r\n"));
        }
        raw.push_str("\r\nhi\r\n");
        assert!(matches!(mail_parse(raw.as_bytes()), Err(Fail::Other("RecursionError", _))));
        let mut ok = String::new();
        for i in 0..MAX_DEPTH - 5 {
            ok.push_str(&format!("Content-Type: message/rfc822\r\nX: {i}\r\n\r\n"));
        }
        assert_eq!(mail_parse(ok.as_bytes()).unwrap().walk().len(), MAX_DEPTH - 4);
    }
}
