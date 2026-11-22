//! HTML 正文里的链接：原来是 `html.parser.HTMLParser` 的一个子类，只接 `handle_starttag`，收 `<a href=…>`。
//! 这里照 CPython 3.14.0 的 html/parser.py 手写它认标签的那一套（哪里算一个开始标签、属性怎么切、注释和声明怎么跳过、
//! script / style / textarea / title 里的不当标签看、属性值里的字符引用怎么解），别的回调原来就是空的，不移植。
//! 对照数据在 tests/vectors/mail.json 的 html 一节（移植时生成的固定数据）。
//!
//! 原来先 `feed(text)` 再 `close()`：凑不成一个完整标签的那一截留到 close 时当到头处理，结果和一遍看到头一样，
//! 这里就是一遍看到头。原来唯一会出的异常是字符引用里的十进制数字超过 4300 位（`int()` 的上限，没人接、被外面的
//! `except Exception` 吃掉）：从那里起不再往下看，前面已经收到的链接照样算。
//!
//! 原来的正则带前瞻、后顾（`(?<=['"\t\n\r\f /])`、`/(?!>)`、`(?!['"])`），这里是手写的判断，回退的地方在
//! `value_part` 里写明了。

use super::codec::INT_MAX_STR_DIGITS;
use super::html5::{HTML5, INVALID_CHARREFS, INVALID_CODEPOINTS};
use crate::py::text;
use std::cell::Cell;

/// 标签里算空白的字符：`[\t\n\r\f ]`
fn is_ws(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\x0c' | ' ')
}

/// 整篇文字，按字符
struct Doc<'a> {
    raw: &'a [char],
    /// 从这个位置往后再没有单引号 / 双引号了（找过一次没找到的记下来：一堆没合上的引号不用每次都找到头）
    no_quote_after: [Cell<usize>; 2],
}

impl Doc<'_> {
    fn len(&self) -> usize {
        self.raw.len()
    }

    fn at(&self, i: usize) -> Option<char> {
        self.raw.get(i).copied()
    }

    fn is(&self, i: usize, c: char) -> bool {
        self.raw.get(i) == Some(&c)
    }

    /// 从 from 起找字符 c
    fn find(&self, c: char, from: usize) -> Option<usize> {
        self.raw.get(from..)?.iter().position(|x| *x == c).map(|i| i + from)
    }

    fn find_quote(&self, quote: char, from: usize) -> Option<usize> {
        let memo = &self.no_quote_after[usize::from(quote == '"')];
        if from >= memo.get() {
            return None;
        }
        let found = self.find(quote, from);
        if found.is_none() {
            memo.set(from);
        }
        found
    }

    fn starts_with(&self, at: usize, s: &str) -> bool {
        s.chars().enumerate().all(|(k, c)| self.is(at + k, c))
    }

    /// `rawdata[a:b]`
    fn slice(&self, a: usize, b: usize) -> String {
        let b = b.min(self.len());
        if a >= b {
            return String::new();
        }
        self.raw[a..b].iter().collect()
    }

    /// 属性的「= 值」那一段（attrfind_tolerant、locatetagend 里是同一段写法），从属性名的后面 q 起：
    /// (值从哪, 值到哪, 这一段到哪结束)；没有这一段是 None。值是引号括起来的（连引号）或者不带引号的一串。
    /// 引号没合上时正则会回退：等号后面有空白就退一格、当成空值；没有空白可退，整段不算
    fn value_part(&self, q: usize) -> Option<(usize, usize, usize)> {
        let n = self.len();
        let mut r = q;
        while r < n && is_ws(self.raw[r]) {
            r += 1;
        }
        if !self.is(r, '=') {
            return None;
        }
        let after_eq = r + 1;
        let mut v = after_eq;
        while v < n && is_ws(self.raw[v]) {
            v += 1;
        }
        match self.at(v) {
            Some(quote @ ('\'' | '"')) => match self.find_quote(quote, v + 1) {
                Some(close) => Some((v, close + 1, close + 1)),
                None if v > after_eq => Some((v - 1, v - 1, v - 1)),
                None => None,
            },
            _ => {
                let mut e = v;
                while e < n && self.raw[e] != '>' && !is_ws(self.raw[e]) {
                    e += 1;
                }
                Some((v, e, e))
            }
        }
    }

    /// 属性名能不能从 p 开始：前一个字符是引号、空白或斜杠，这一个不是空白、斜杠、`>`
    fn attr_starts(&self, p: usize) -> bool {
        let behind = p.checked_sub(1).and_then(|i| self.at(i)).is_some_and(|c| matches!(c, '\'' | '"' | '/') || is_ws(c));
        behind && self.at(p).is_some_and(|c| !is_ws(c) && !matches!(c, '/' | '>'))
    }

    /// 属性名到哪结束（第一个字符之后，到空白、斜杠、等号、`>` 为止）
    fn attr_name_end(&self, p: usize) -> usize {
        let mut q = p + 1;
        while q < self.len() && !is_ws(self.raw[q]) && !matches!(self.raw[q], '/' | '=' | '>') {
            q += 1;
        }
        q
    }

    /// 标签名到哪结束（start 是它的第一个字母）：到空白、斜杠、`>` 为止
    fn tag_name_end(&self, start: usize) -> usize {
        let mut p = start + 1;
        while p < self.len() && !is_ws(self.raw[p]) && !matches!(self.raw[p], '/' | '>') {
            p += 1;
        }
        p
    }

    /// `locatetagend.match(rawdata, start).end()`：标签名、各个属性，最后可以有一个 `>`
    fn locate_tag_end(&self, start: usize) -> usize {
        let skip = |mut p: usize| {
            while p < self.len() && (is_ws(self.raw[p]) || self.raw[p] == '/') {
                p += 1;
            }
            p
        };
        let mut p = skip(self.tag_name_end(start));
        while self.attr_starts(p) {
            let q = self.attr_name_end(p);
            p = skip(self.value_part(q).map_or(q, |(_, _, end)| end));
        }
        if self.is(p, '>') {
            p += 1;
        }
        p
    }

    /// 空白，或者后面不是 `>` 的斜杠，连着跳过去（tagfind_tolerant、attrfind_tolerant 的结尾）
    fn skip_ws_and_slashes(&self, mut p: usize) -> usize {
        while p < self.len() && (is_ws(self.raw[p]) || (self.raw[p] == '/' && !self.is(p + 1, '>'))) {
            p += 1;
        }
        p
    }
}

/// 数字的字符引用换成字符（`html._replace_charref` 的那一半）；位数超过上限是 None（原来是 ValueError）
fn numeric_charref(digits: &str, hex: bool) -> Option<String> {
    let num: u32 = if hex {
        // 十六进制没有位数上限；放不下的反正比 0x10FFFF 大
        let d = digits.trim_start_matches('0');
        if d.len() > 8 {
            u32::MAX
        } else {
            u32::from_str_radix(d, 16).unwrap_or(0)
        }
    } else {
        if digits.len() > INT_MAX_STR_DIGITS {
            return None;
        }
        let d = digits.trim_start_matches('0');
        if d.len() > 9 {
            u32::MAX
        } else {
            d.parse().unwrap_or(0)
        }
    };
    if let Some((_, to)) = INVALID_CHARREFS.iter().find(|(n, _)| *n == num) {
        return Some(char::from_u32(*to).map(String::from).unwrap_or_default());
    }
    if (0xD800..=0xDFFF).contains(&num) || num > 0x10FFFF {
        return Some("\u{fffd}".to_string());
    }
    if INVALID_CODEPOINTS.binary_search(&num).is_ok() {
        return Some(String::new());
    }
    Some(char::from_u32(num).map(String::from).unwrap_or_default())
}

/// `html.entities.html5[name]`（name 带不带分号都是表里原样的键）
fn html5(name: &str) -> Option<&'static str> {
    HTML5.binary_search_by(|(k, _)| (*k).cmp(name)).ok().map(|i| HTML5[i].1)
}

/// `_unescape_attrvalue(s)`：属性值里的字符引用。数字的都解；名字的只在整个对得上、后面不是等号时才解
/// （`&lang=zh` 里的 `&lang` 不动）。None：数字太长，原来在这里出异常
fn unescape_attrvalue(s: &str) -> Option<String> {
    if !s.contains('&') {
        return Some(s.to_string());
    }
    let cs: Vec<char> = s.chars().collect();
    let n = cs.len();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < n {
        if cs[i] != '&' {
            out.push(cs[i]);
            i += 1;
            continue;
        }
        // &(#[0-9]+|#[xX][0-9a-fA-F]+|[a-zA-Z][a-zA-Z0-9]*)[;=]?
        let mut j = i + 1;
        let mut numeric: Option<(String, bool)> = None;
        if cs.get(j) == Some(&'#') {
            let mut k = j + 1;
            while k < n && cs[k].is_ascii_digit() {
                k += 1;
            }
            if k > j + 1 {
                numeric = Some((cs[j + 1..k].iter().collect(), false));
                j = k;
            } else if matches!(cs.get(j + 1), Some('x' | 'X')) {
                let mut k = j + 2;
                while k < n && cs[k].is_ascii_hexdigit() {
                    k += 1;
                }
                if k > j + 2 {
                    numeric = Some((cs[j + 2..k].iter().collect(), true));
                    j = k;
                }
            }
            if numeric.is_none() {
                out.push('&');
                i += 1;
                continue;
            }
        } else if cs.get(j).is_some_and(|c| c.is_ascii_alphabetic()) {
            while j < n && cs[j].is_ascii_alphanumeric() {
                j += 1;
            }
        } else {
            out.push('&');
            i += 1;
            continue;
        }
        let tail = cs.get(j).copied().filter(|c| matches!(c, ';' | '='));
        let end = j + usize::from(tail.is_some());
        match numeric {
            Some((digits, hex)) => {
                out.push_str(&numeric_charref(&digits, hex)?);
                if tail == Some('=') {
                    out.push('='); // 数字后面的分号吃掉，等号留着
                }
            }
            None => {
                let name: String = cs[i + 1..end].iter().collect();
                match html5(&name).filter(|_| tail != Some('=')) {
                    Some(value) => out.push_str(value),
                    None => out.extend(&cs[i..end]),
                }
            }
        }
        i = end;
    }
    Some(out)
}

/// script、style（里面的原样）还是 textarea、title（里面的字符引用照解）
#[derive(Clone, Copy)]
struct Cdata {
    elem: &'static str,
    escapable: bool,
}

struct Scanner<'a> {
    doc: Doc<'a>,
    cdata: Option<Cdata>,
    links: Vec<String>,
}

/// 解析到一半停下的原因
enum Stop {
    /// 这个结构没写完（没有 `>`、注释没结束……）：后面的都不看了
    Incomplete,
    /// 原来在这里出了异常
    Raised,
}

impl Scanner<'_> {
    /// [a, b) 这段文字交给 `html.unescape` 时会不会出异常：`&#` 后面跟着超过上限那么多位的十进制数字
    fn unescape_raises(&self, a: usize, b: usize) -> bool {
        let raw = self.doc.raw;
        let mut i = a;
        while i + 1 < b {
            if raw[i] == '&' && raw[i + 1] == '#' {
                let mut k = i + 2;
                while k < b && raw[k].is_ascii_digit() {
                    k += 1;
                }
                if k - (i + 2) > INT_MAX_STR_DIGITS {
                    return true;
                }
                i = k;
            } else {
                i += 1;
            }
        }
        false
    }

    /// `parse_starttag(i)`：返回这个标签到哪结束
    fn parse_starttag(&mut self, i: usize) -> Result<usize, Stop> {
        let doc = &self.doc;
        let endpos = doc.locate_tag_end(i + 1);
        if !doc.is(endpos - 1, '>') {
            return Err(Stop::Incomplete);
        }
        let name_end = doc.tag_name_end(i + 1);
        let mut k = doc.skip_ws_and_slashes(name_end);
        let tag = doc.slice(i + 1, name_end).to_lowercase();
        let mut hrefs: Vec<String> = Vec::new();
        while k < endpos && doc.attr_starts(k) {
            let q = doc.attr_name_end(k);
            let value = doc.value_part(q);
            if let Some((mut a, mut b, _)) = value {
                // 两头是同一种引号的去掉引号
                if b - a >= 2 && matches!(doc.raw[a], '\'' | '"') && doc.raw[b - 1] == doc.raw[a] {
                    a += 1;
                    b -= 1;
                }
                if a < b {
                    // 每个属性的值都解（不只是 href）：原来哪个属性里出了异常，这个标签就没了
                    let unescaped = unescape_attrvalue(&doc.slice(a, b)).ok_or(Stop::Raised)?;
                    if tag == "a" && !unescaped.is_empty() && doc.slice(k, q).to_lowercase() == "href" {
                        hrefs.push(unescaped);
                    }
                }
            }
            k = doc.skip_ws_and_slashes(value.map_or(q, |(_, _, end)| end));
        }
        let end = doc.slice(k, endpos);
        let end = text::strip(&end);
        if end != ">" && end != "/>" {
            return Ok(endpos); // 标签没写对：整个当文字
        }
        self.links.extend(hrefs);
        if end != "/>" {
            // 这几种元素里面的东西不当标签看，直到它自己的结束标签
            self.cdata = match tag.as_str() {
                "script" => Some(Cdata { elem: "script", escapable: false }),
                "style" => Some(Cdata { elem: "style", escapable: false }),
                "textarea" => Some(Cdata { elem: "textarea", escapable: true }),
                "title" => Some(Cdata { elem: "title", escapable: true }),
                _ => None,
            };
        }
        Ok(endpos)
    }

    /// `parse_endtag(i)`
    fn parse_endtag(&mut self, i: usize) -> Result<usize, Stop> {
        let doc = &self.doc;
        let Some(gt) = doc.find('>', i + 2) else {
            return Err(Stop::Incomplete);
        };
        if !doc.at(i + 2).is_some_and(|c| c.is_ascii_alphabetic()) {
            // </> 不算什么；别的当注释，到下一个 > 为止
            return Ok(if doc.is(i + 2, '>') { i + 3 } else { gt + 1 });
        }
        let j = doc.locate_tag_end(i + 2);
        if !doc.is(j - 1, '>') {
            return Err(Stop::Incomplete);
        }
        self.cdata = None;
        Ok(j)
    }

    /// 注释：到 `-->` 或 `--!>`；`<!-->`、`<!--->` 也算结束了
    fn parse_comment(&self, i: usize) -> Result<usize, Stop> {
        let doc = &self.doc;
        for p in i + 4..doc.len() {
            if doc.is(p, '-') && doc.is(p + 1, '-') {
                if doc.is(p + 2, '>') {
                    return Ok(p + 3);
                }
                if doc.is(p + 2, '!') && doc.is(p + 3, '>') {
                    return Ok(p + 4);
                }
            }
        }
        if doc.is(i + 4, '>') {
            return Ok(i + 5);
        }
        if doc.is(i + 4, '-') && doc.is(i + 5, '>') {
            return Ok(i + 6);
        }
        Err(Stop::Incomplete)
    }

    /// `<!…>`：CDATA 段到 `]]>`，别的（DOCTYPE、乱写的）到下一个 `>`
    fn parse_declaration(&self, i: usize) -> Result<usize, Stop> {
        let doc = &self.doc;
        if doc.starts_with(i, "<![CDATA[") {
            for p in i + 9..doc.len() {
                if doc.is(p, ']') && doc.is(p + 1, ']') && doc.is(p + 2, '>') {
                    return Ok(p + 3);
                }
            }
            return Err(Stop::Incomplete);
        }
        let from = if doc.slice(i, i + 9).to_lowercase() == "<!doctype" { i + 9 } else { i + 2 };
        doc.find('>', from).map(|p| p + 1).ok_or(Stop::Incomplete)
    }

    /// script 这类元素里：找它的结束标签 `</名字` 后面跟着空白、斜杠或 `>`（名字不分大小写，只认 ASCII）
    fn find_cdata_end(&self, elem: &str, from: usize) -> Option<usize> {
        let raw = self.doc.raw;
        let len = elem.len();
        let mut p = from;
        while p + 2 + len < raw.len() {
            if raw[p] == '<'
                && raw[p + 1] == '/'
                && raw[p + 2..p + 2 + len].iter().zip(elem.chars()).all(|(a, b)| a.to_ascii_lowercase() == b)
                && (is_ws(raw[p + 2 + len]) || matches!(raw[p + 2 + len], '/' | '>'))
            {
                return Some(p);
            }
            p += 1;
        }
        None
    }

    /// `goahead(end=True)`
    fn run(&mut self) {
        let n = self.doc.len();
        let mut i = 0;
        while i < n {
            let j = match self.cdata {
                None => self.doc.find('<', i).unwrap_or(n),
                Some(c) => match self.find_cdata_end(c.elem, i) {
                    Some(j) => j,
                    None => return, // 没有结束标签：剩下的都是它里面的文字，后面再没有标签了
                },
            };
            // 标签之间的文字原来要过一遍 html.unescape（script、style 里的不过）
            if i < j && self.cdata.is_none_or(|c| c.escapable) && self.unescape_raises(i, j) {
                return;
            }
            i = j;
            if i == n {
                return;
            }
            let step = if self.doc.at(i + 1).is_some_and(|c| c.is_ascii_alphabetic()) {
                self.parse_starttag(i)
            } else if self.doc.starts_with(i, "</") {
                self.parse_endtag(i)
            } else if self.doc.starts_with(i, "<!--") {
                self.parse_comment(i)
            } else if self.doc.starts_with(i, "<?") {
                self.doc.find('>', i + 2).map(|p| p + 1).ok_or(Stop::Incomplete)
            } else if self.doc.starts_with(i, "<!") {
                self.parse_declaration(i)
            } else {
                Ok(i + 1) // 单独的一个 <：当文字
            };
            match step {
                Ok(k) => i = k,
                Err(Stop::Incomplete | Stop::Raised) => return,
            }
        }
    }
}

/// HTML 里各个 `<a>` 的 href（解过字符引用、非空的），按出现的先后
pub fn hrefs(html: &str) -> Vec<String> {
    let raw: Vec<char> = html.chars().collect();
    let doc = Doc { raw: &raw, no_quote_after: [Cell::new(usize::MAX), Cell::new(usize::MAX)] };
    let mut s = Scanner { doc, cdata: None, links: Vec::new() };
    s.run();
    s.links
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::mail::testing::vectors;

    #[test]
    fn links_like_python() {
        let cases = vectors("html");
        assert!(cases.len() > 400);
        for c in cases {
            let input = c.at("in").py_str();
            let want: Vec<String> = c.at("hrefs").as_list().unwrap().strs().iter().map(|s| s.to_string()).collect();
            assert_eq!(hrefs(&input), want, "{input:?}");
        }
    }

    #[test]
    fn attribute_values_like_python() {
        let cases = vectors("html_attr");
        assert!(cases.len() > 150);
        for c in cases {
            let input = c.at("in").py_str();
            assert_eq!(unescape_attrvalue(&input), c.get("out").map(|o| o.py_str()), "{input:?}");
        }
    }

    #[test]
    fn tables_are_sorted() {
        assert_eq!(HTML5.len(), 2231);
        assert!(HTML5.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(INVALID_CODEPOINTS.windows(2).all(|w| w[0] < w[1]));
        assert_eq!((html5("amp;"), html5("amp"), html5("numsp;"), html5("numsp")), (Some("&"), Some("&"), Some("\u{2007}"), None));
    }

    #[test]
    fn a_pile_of_open_quotes_is_not_quadratic() {
        let html = "<a x=\"".repeat(60_000);
        let t0 = crate::py::time::monotonic();
        assert!(hrefs(&html).is_empty());
        assert!(crate::py::time::monotonic() - t0 < 5.0);
    }
}
