//! `xml.etree.ElementTree.fromstring(字节)`，和它上面的 `find` / `findall` / `findtext` / `get`（只有直接子元素、完整的标签名这
//! 一种写法：原来用到的就是这些）。底下原来是 expat 2.7（带命名空间处理，标签写成 `{地址}名字`）：接受什么、拒绝什么照它的来，
//! 拿真的 CPython 3.14 核对过（tests/vectors/clients.json 的 xml 一节，移植时生成的固定数据）。
//!
//! 客户端更新读 ChatGPT 的版本列表（Sparkle 的 appcast）用它。文档写得不对一律是 `ParseError`（原来的 `ET.ParseError`）。
//!
//! 和 Python 不一样的地方：
//! - 带 `<!DOCTYPE …>` 的一律不收。原来 expat 认它（ElementTree 不管里面写了什么，但内部声明的实体会展开）；客户端更新在解析前
//!   就把带 `<!DOCTYPE`、`<!ENTITY` 字样的版本列表挡掉了，只有 UTF-16 写的（那两个词在字节里看不出来）才到得了这里
//! - 编码只认 expat 自己会的那几种：UTF-8、UTF-16（带不带 BOM）、ISO-8859-1、US-ASCII。声明里写别的名字（`utf8`、`latin-1`、
//!   `windows-1252`、`gbk`……）时，原来 pyexpat 去找 Python 的编码表（单字节的能读，多字节的是 ValueError，没有这个名字是
//!   LookupError），这里都当作读不了
//! - UTF-16 里落单的代理项当作写错了（expat 不查后半个是不是代理项，照样往下读）
//! - 元素的 `tail`（结束标签后面的文字）不记：原来没用到

#[path = "xml_tables.rs"]
mod tables;

/// 文档写得不对（`ET.ParseError`）；带着大致的原因，只给调试看
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError(pub String);

fn bad<T>(why: &str) -> Result<T, ParseError> {
    Err(ParseError(why.to_string()))
}

struct Node {
    tag: String,
    attrib: Vec<(String, String)>,
    /// 开始标签到第一个子元素（没有子元素就是到结束标签）之间的文字；没有是空串（原来是 None）
    text: String,
    children: Vec<usize>,
}

/// 解析出来的整棵树。元素放在一张表里、互相用下标指：套得很深的文档（两百万字节能套几十万层）建和丢都不用递归
pub struct Tree {
    nodes: Vec<Node>,
}

/// 树里的一个元素（`xml.etree.ElementTree.Element`）
#[derive(Clone, Copy)]
pub struct El<'a> {
    tree: &'a Tree,
    at: usize,
}

impl Tree {
    /// 根元素
    pub fn root(&self) -> El<'_> {
        El { tree: self, at: 0 }
    }
}

impl<'a> El<'a> {
    fn node(&self) -> &'a Node {
        &self.tree.nodes[self.at]
    }

    /// `e.tag`：带命名空间的是 `{地址}名字`
    pub fn tag(&self) -> &'a str {
        &self.node().tag
    }

    /// `e.text or ''`
    pub fn text(&self) -> &'a str {
        &self.node().text
    }

    /// `e.get(name)`：没有这个属性是 None
    pub fn get(&self, name: &str) -> Option<&'a str> {
        self.node().attrib.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// `list(e)`：直接的子元素
    pub fn children(&self) -> Vec<El<'a>> {
        self.node().children.iter().map(|&at| El { tree: self.tree, at }).collect()
    }

    /// `e.findall(tag)`：标签正好是 tag 的直接子元素
    pub fn findall(&self, tag: &str) -> Vec<El<'a>> {
        self.children().into_iter().filter(|c| c.tag() == tag).collect()
    }

    /// `e.find(tag)`
    pub fn find(&self, tag: &str) -> Option<El<'a>> {
        self.children().into_iter().find(|c| c.tag() == tag)
    }

    /// `e.findtext(tag)`：第一个这样的子元素的文字（没有文字是空串）；没有这样的子元素是 None
    pub fn findtext(&self, tag: &str) -> Option<&'a str> {
        self.find(tag).map(|c| c.text())
    }
}

/// 文档用的编码
#[derive(Clone, Copy, PartialEq, Debug)]
enum Enc {
    Utf8,
    Latin1,
    Ascii,
    Utf16 { big: bool },
}

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

fn in_table(table: &[(u32, u32)], c: char) -> bool {
    let x = c as u32;
    table
        .binary_search_by(|&(a, b)| {
            if x < a {
                std::cmp::Ordering::Greater
            } else if x > b {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// `<?xml … ?>` 里的三样（expat 的 doParseXmlDecl）：decl 是 `<?xml` 和 `?>` 之间的那一段。写得不对是 None
fn parse_xml_decl(decl: &[char]) -> Option<(String, Option<String>)> {
    let mut i = 0;
    // 一个 名字="值"；到头了是 Ok(None)
    let pseudo = |i: &mut usize| -> Result<Option<(String, String)>, ()> {
        if *i == decl.len() {
            return Ok(None);
        }
        if !is_ws(decl[*i]) {
            return Err(());
        }
        while *i < decl.len() && is_ws(decl[*i]) {
            *i += 1;
        }
        if *i == decl.len() {
            return Ok(None);
        }
        let start = *i;
        let name_end;
        loop {
            let c = *decl.get(*i).ok_or(())?;
            if !c.is_ascii() {
                return Err(());
            }
            if c == '=' {
                name_end = *i;
                break;
            }
            if is_ws(c) {
                name_end = *i;
                while *i < decl.len() && is_ws(decl[*i]) {
                    *i += 1;
                }
                if decl.get(*i) != Some(&'=') {
                    return Err(());
                }
                break;
            }
            *i += 1;
        }
        if name_end == start {
            return Err(());
        }
        *i += 1;
        while *i < decl.len() && is_ws(decl[*i]) {
            *i += 1;
        }
        let open = *decl.get(*i).ok_or(())?;
        if open != '"' && open != '\'' {
            return Err(());
        }
        *i += 1;
        let value_start = *i;
        loop {
            let c = *decl.get(*i).ok_or(())?;
            if c == open {
                break;
            }
            if !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) {
                return Err(());
            }
            *i += 1;
        }
        let value: String = decl[value_start..*i].iter().collect();
        *i += 1;
        Ok(Some((decl[start..name_end].iter().collect(), value)))
    };
    let (name, version) = pseudo(&mut i).ok()??;
    if name != "version" {
        return None;
    }
    let Some((mut name, mut value)) = pseudo(&mut i).ok()? else {
        return Some((version, None));
    };
    let mut encoding = None;
    if name == "encoding" {
        if !value.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
            return None;
        }
        encoding = Some(value);
        match pseudo(&mut i).ok()? {
            None => return Some((version, encoding)),
            Some(next) => (name, value) = next,
        }
    }
    if name != "standalone" || (value != "yes" && value != "no") {
        return None;
    }
    while i < decl.len() && is_ws(decl[i]) {
        i += 1;
    }
    (i == decl.len()).then_some((version, encoding))
}

/// expat 自己认识的编码名（不分大小写）
fn known_encoding(name: &str) -> Option<Enc> {
    match name.to_ascii_uppercase().as_str() {
        "UTF-8" => Some(Enc::Utf8),
        "ISO-8859-1" => Some(Enc::Latin1),
        "US-ASCII" => Some(Enc::Ascii),
        "UTF-16" | "UTF-16BE" => Some(Enc::Utf16 { big: true }),
        "UTF-16LE" => Some(Enc::Utf16 { big: false }),
        _ => None,
    }
}

/// 字节 → 字符，连同用的是哪种编码。开头认法照 expat：BOM、头两个字节里有 0（没有 BOM 的 UTF-16）、不然是单字节的一类，
/// 再看 `<?xml … encoding="…"?>` 怎么说
fn decode(data: &[u8]) -> Result<(Vec<char>, Enc), ParseError> {
    let utf16 = |body: &[u8], big: bool| -> Result<(Vec<char>, Enc), ParseError> {
        if !body.len().is_multiple_of(2) {
            return bad("UTF-16 的字节数不是双数");
        }
        let units = body.chunks(2).map(|c| if big { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) });
        match char::decode_utf16(units).collect::<Result<Vec<char>, _>>() {
            Ok(chars) => Ok((chars, Enc::Utf16 { big })),
            Err(_) => bad("UTF-16 里有落单的代理项"),
        }
    };
    if data.starts_with(&[0xfe, 0xff]) {
        return utf16(&data[2..], true);
    }
    if data.starts_with(&[0xff, 0xfe]) {
        return utf16(&data[2..], false);
    }
    let body = data.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(data);
    if body.len() == data.len() && data.len() >= 2 {
        if data[0] == 0 {
            return utf16(data, true);
        }
        if data[1] == 0 {
            return utf16(data, false);
        }
    }
    // 单字节的一类：声明里说的编码
    let mut enc = Enc::Utf8;
    if body.starts_with(b"<?xml") && body.get(5).is_some_and(|b| is_ws(*b as char)) {
        if let Some(end) = body.windows(2).position(|w| w == b"?>") {
            let decl: Vec<char> = body[5..end].iter().map(|b| *b as char).collect();
            if let Some((_, Some(name))) = parse_xml_decl(&decl) {
                enc = match known_encoding(&name) {
                    Some(Enc::Utf16 { .. }) => return bad("声明的编码和实际的对不上"),
                    Some(enc) => enc,
                    None => return bad("不认识的编码"),
                };
            }
        }
    }
    let chars = match enc {
        Enc::Utf8 => match std::str::from_utf8(body) {
            Ok(s) => s.chars().collect(),
            Err(_) => return bad("不是 UTF-8"),
        },
        Enc::Latin1 => body.iter().map(|b| *b as char).collect(),
        _ => {
            if !body.is_ascii() {
                return bad("US-ASCII 里有 0x80 以上的字节");
            }
            body.iter().map(|b| *b as char).collect()
        }
    };
    Ok((chars, enc))
}

/// 还开着的元素
struct Open {
    node: usize,
    /// 开始标签里写的名字（结束标签要和它一字不差）
    raw: String,
    /// 这个元素开始之前有多少条命名空间绑定（它结束时退回去）
    bindings: usize,
}

struct Parser<'a> {
    s: &'a [char],
    i: usize,
    enc: Enc,
    nodes: Vec<Node>,
    open: Vec<Open>,
    /// 命名空间绑定（前缀 → 地址；默认的那个前缀是空串），里层的在后面
    bindings: Vec<(String, String)>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn at(&self, lit: &str) -> bool {
        lit.chars().enumerate().all(|(k, c)| self.s.get(self.i + k) == Some(&c))
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(is_ws) {
            self.i += 1;
        }
    }

    /// 能当名字开头的字符（带命名空间处理时冒号不算）。U+0080 以上的：UTF-8 的文档照 Unicode 的那张表，ISO-8859-1、UTF-16 的文档里
    /// U+00FF 以下的照 expat 的 Latin-1 表（多认 ª µ º）
    fn name_start(&self, c: char) -> bool {
        if c.is_ascii() {
            return c.is_ascii_alphabetic() || c == '_';
        }
        if self.enc != Enc::Utf8 && (c as u32) < 0x100 {
            return in_table(tables::LATIN1_START, c);
        }
        in_table(tables::NAME_START, c)
    }

    fn name_char(&self, c: char) -> bool {
        if c.is_ascii() {
            return c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.');
        }
        if self.enc != Enc::Utf8 && (c as u32) < 0x100 {
            return in_table(tables::LATIN1_START, c) || in_table(tables::LATIN1_MORE, c);
        }
        in_table(tables::NAME_START, c) || in_table(tables::NAME_MORE, c)
    }

    /// 元素、属性的名字：最多一个冒号，冒号后面要接着能开头的字符
    fn qname(&mut self) -> Result<String, ParseError> {
        let start = self.i;
        if !self.peek().is_some_and(|c| self.name_start(c)) {
            return bad("名字开头不对");
        }
        self.i += 1;
        let mut had_colon = false;
        while let Some(c) = self.peek() {
            if self.name_char(c) {
                self.i += 1;
            } else if c == ':' {
                if had_colon {
                    return bad("名字里有两个冒号");
                }
                had_colon = true;
                self.i += 1;
                if !self.peek().is_some_and(|c| self.name_start(c)) {
                    return bad("冒号后面不是名字");
                }
                self.i += 1;
            } else {
                break;
            }
        }
        Ok(self.s[start..self.i].iter().collect())
    }

    /// 没有冒号的名字（处理指令的目标、实体的名字）
    fn ncname(&mut self) -> Result<String, ParseError> {
        let start = self.i;
        if !self.peek().is_some_and(|c| self.name_start(c)) {
            return bad("名字开头不对");
        }
        self.i += 1;
        while self.peek().is_some_and(|c| self.name_char(c)) {
            self.i += 1;
        }
        Ok(self.s[start..self.i].iter().collect())
    }

    /// `&` 后面的引用（位置在 `&` 之后）：五个预定义的实体、字符引用；别的实体没有定义
    fn reference(&mut self) -> Result<char, ParseError> {
        if self.peek() == Some('#') {
            self.i += 1;
            let hex = self.peek() == Some('x');
            if hex {
                self.i += 1;
            }
            let mut n: u32 = 0;
            let mut digits = 0;
            let mut too_big = false;
            while let Some(d) = self.peek().and_then(|c| c.to_digit(if hex { 16 } else { 10 })) {
                n = n.saturating_mul(if hex { 16 } else { 10 }).saturating_add(d);
                too_big |= n >= 0x110000;
                digits += 1;
                self.i += 1;
            }
            if digits == 0 || self.peek() != Some(';') {
                return bad("字符引用写得不对");
            }
            self.i += 1;
            // expat 的 checkCharRefNumber：代理项、U+FFFE / U+FFFF、制表符换行回车以外的控制字符不行
            let ok = !too_big
                && !(0xd800..=0xdfff).contains(&n)
                && n != 0xfffe
                && n != 0xffff
                && !(n < 0x20 && !matches!(n, 9 | 10 | 13));
            return match char::from_u32(n).filter(|_| ok) {
                Some(c) => Ok(c),
                None => bad("字符引用指的不是合法的字符"),
            };
        }
        let name = self.ncname()?;
        if self.peek() != Some(';') {
            return bad("实体引用没有分号");
        }
        self.i += 1;
        match name.as_str() {
            "lt" => Ok('<'),
            "gt" => Ok('>'),
            "amp" => Ok('&'),
            "quot" => Ok('"'),
            "apos" => Ok('\''),
            _ => bad("没有定义的实体"),
        }
    }

    /// `<!--` 之后到 `-->`
    fn comment(&mut self) -> Result<(), ParseError> {
        loop {
            match self.peek() {
                None => return bad("注释没有结束"),
                Some('-') if self.s.get(self.i + 1) == Some(&'-') => {
                    if self.s.get(self.i + 2) == Some(&'>') {
                        self.i += 3;
                        return Ok(());
                    }
                    return bad("注释里有 --");
                }
                Some(_) => self.i += 1,
            }
        }
    }

    /// `<?` 之后的处理指令。返回它是不是 XML 声明（`<?xml`，内容是 `<?xml` 和 `?>` 之间的那一段）
    fn pi(&mut self) -> Result<Option<(usize, usize)>, ParseError> {
        let target = self.ncname()?;
        let is_decl = target == "xml";
        if !is_decl && target.eq_ignore_ascii_case("xml") {
            return bad("处理指令的目标不能是 XML");
        }
        let content = self.i;
        match self.peek() {
            Some('?') if self.s.get(self.i + 1) == Some(&'>') => {
                self.i += 2;
                Ok(is_decl.then_some((content, content)))
            }
            Some(c) if is_ws(c) => loop {
                match self.peek() {
                    None => return bad("处理指令没有结束"),
                    Some('?') if self.s.get(self.i + 1) == Some(&'>') => {
                        let end = self.i;
                        self.i += 2;
                        return Ok(is_decl.then_some((content, end)));
                    }
                    Some(_) => self.i += 1,
                }
            },
            _ => bad("处理指令的目标后面不对"),
        }
    }

    /// 属性的值（位置在开头的引号之后）：换行、制表符换成空格，引用展开
    fn attribute_value(&mut self, quote: char) -> Result<String, ParseError> {
        let mut out = String::new();
        loop {
            let Some(c) = self.peek() else { return bad("属性的值没有结束") };
            self.i += 1;
            match c {
                c if c == quote => return Ok(out),
                '<' => return bad("属性的值里有 <"),
                '&' => out.push(self.reference()?),
                '\r' => {
                    if self.peek() == Some('\n') {
                        self.i += 1;
                    }
                    out.push(' ');
                }
                '\n' | '\t' => out.push(' '),
                c => out.push(c),
            }
        }
    }

    /// 前缀现在绑到哪个地址
    fn lookup(&self, prefix: &str) -> Option<&str> {
        if prefix == "xml" {
            return Some(XML_NS);
        }
        self.bindings.iter().rev().find(|(p, _)| p == prefix).map(|(_, uri)| uri.as_str())
    }

    /// 一条 `xmlns` / `xmlns:前缀` 的绑定（expat 的 addBinding）
    fn bind(&mut self, prefix: &str, uri: String) -> Result<(), ParseError> {
        if uri.is_empty() && !prefix.is_empty() {
            return bad("不能把前缀的绑定取消");
        }
        if prefix == "xmlns" {
            return bad("xmlns 这个前缀不能绑");
        }
        if uri.contains('}') {
            return bad("命名空间的地址里有 }");
        }
        if (prefix == "xml") != (uri == XML_NS) {
            return bad("xml 这个前缀和它的地址只能互相绑");
        }
        if prefix != "xml" && uri == XMLNS_NS {
            return bad("不能绑到 xmlns 的地址上");
        }
        self.bindings.push((prefix.to_string(), uri));
        Ok(())
    }

    /// 名字照命名空间展开成 `{地址}名字`。属性没有前缀就没有命名空间；元素没有前缀用默认的
    fn expand(&self, raw: &str, is_attribute: bool) -> Result<String, ParseError> {
        let (prefix, local) = match raw.split_once(':') {
            Some((prefix, local)) => (prefix, local),
            None if is_attribute => return Ok(raw.to_string()),
            None => ("", raw),
        };
        match self.lookup(prefix) {
            Some(uri) if !uri.is_empty() => Ok(format!("{{{uri}}}{local}")),
            _ if prefix.is_empty() => Ok(local.to_string()),
            _ => bad("前缀没有绑定"),
        }
    }

    /// `<` 之后是名字：开始标签（或者 `<…/>`）
    fn start_tag(&mut self) -> Result<(), ParseError> {
        let raw = self.qname()?;
        let mut attrs: Vec<(String, String)> = Vec::new();
        let empty;
        loop {
            let had_ws = self.peek().is_some_and(is_ws);
            self.skip_ws();
            match self.peek() {
                None => return bad("开始标签没有结束"),
                Some('>') => {
                    self.i += 1;
                    empty = false;
                    break;
                }
                Some('/') => {
                    if self.s.get(self.i + 1) != Some(&'>') {
                        return bad("/ 后面不是 >");
                    }
                    self.i += 2;
                    empty = true;
                    break;
                }
                Some(_) if !had_ws => return bad("属性前面没有空白"),
                Some(_) => {}
            }
            let name = self.qname()?;
            self.skip_ws();
            if self.peek() != Some('=') {
                return bad("属性名后面不是 =");
            }
            self.i += 1;
            self.skip_ws();
            let quote = match self.peek() {
                Some(q @ ('"' | '\'')) => q,
                _ => return bad("属性的值没有引号"),
            };
            self.i += 1;
            let value = self.attribute_value(quote)?;
            if attrs.iter().any(|(n, _)| *n == name) {
                return bad("属性重复了");
            }
            attrs.push((name, value));
        }
        // 先收这个元素上的命名空间绑定，再展开名字
        let bindings_before = self.bindings.len();
        let mut plain = Vec::with_capacity(attrs.len());
        for (name, value) in attrs {
            if name == "xmlns" {
                self.bind("", value)?;
            } else if let Some(prefix) = name.strip_prefix("xmlns:") {
                self.bind(prefix, value)?;
            } else {
                plain.push((name, value));
            }
        }
        let tag = self.expand(&raw, false)?;
        let mut attrib: Vec<(String, String)> = Vec::with_capacity(plain.len());
        for (name, value) in plain {
            let name = self.expand(&name, true)?;
            if attrib.iter().any(|(n, _)| *n == name) {
                return bad("属性重复了");
            }
            attrib.push((name, value));
        }
        let node = self.nodes.len();
        self.nodes.push(Node { tag, attrib, text: String::new(), children: Vec::new() });
        if let Some(parent) = self.open.last() {
            let parent = parent.node;
            self.nodes[parent].children.push(node);
        }
        if empty {
            self.bindings.truncate(bindings_before);
        } else {
            self.open.push(Open { node, raw, bindings: bindings_before });
        }
        Ok(())
    }

    /// `</` 之后：结束标签，名字要和开着的那个一字不差
    fn end_tag(&mut self) -> Result<(), ParseError> {
        let start = self.i;
        if !self.peek().is_some_and(|c| self.name_start(c)) {
            return bad("结束标签的名字不对");
        }
        while self.peek().is_some_and(|c| self.name_char(c) || c == ':') {
            self.i += 1;
        }
        let raw: String = self.s[start..self.i].iter().collect();
        self.skip_ws();
        if self.peek() != Some('>') {
            return bad("结束标签没有结束");
        }
        self.i += 1;
        match self.open.pop() {
            Some(open) if open.raw == raw => {
                self.bindings.truncate(open.bindings);
                Ok(())
            }
            _ => bad("结束标签和开始标签对不上"),
        }
    }

    /// 文字进当前元素的 `text`：有了子元素以后的是子元素的 tail，不记
    fn data(&mut self, text: &str) {
        if let Some(open) = self.open.last() {
            let node = &mut self.nodes[open.node];
            if node.children.is_empty() {
                node.text.push_str(text);
            }
        }
    }

    /// 根元素里面的内容，到根元素结束
    fn content(&mut self) -> Result<(), ParseError> {
        let mut run = String::new();
        while !self.open.is_empty() {
            let Some(c) = self.peek() else { return bad("文档没有结束") };
            if c != '<' && c != '&' {
                self.i += 1;
                match c {
                    '\r' => {
                        if self.peek() == Some('\n') {
                            self.i += 1;
                        }
                        run.push('\n');
                    }
                    ']' if self.at("]>") => return bad("内容里有 ]]>"),
                    c => run.push(c),
                }
                continue;
            }
            self.i += 1;
            if c == '&' {
                run.push(self.reference()?);
                continue;
            }
            // 标签、注释、处理指令：先把攒着的文字交出去（注释、处理指令不打断文字，但顺序无所谓：都进同一个 text）
            if self.at("![CDATA[") {
                self.i += 8;
                loop {
                    match self.peek() {
                        None => return bad("CDATA 没有结束"),
                        Some(']') if self.at("]]>") => {
                            self.i += 3;
                            break;
                        }
                        Some('\r') => {
                            self.i += 1;
                            if self.peek() == Some('\n') {
                                self.i += 1;
                            }
                            run.push('\n');
                        }
                        Some(c) => {
                            self.i += 1;
                            run.push(c);
                        }
                    }
                }
                continue;
            }
            if self.at("!--") {
                self.i += 3;
                self.comment()?;
                continue;
            }
            if self.peek() == Some('?') {
                self.i += 1;
                if self.pi()?.is_some() {
                    return bad("XML 声明不在开头");
                }
                continue;
            }
            self.data(&run);
            run.clear();
            if self.peek() == Some('/') {
                self.i += 1;
                self.end_tag()?;
            } else {
                self.start_tag()?;
            }
        }
        Ok(())
    }

    /// 根元素前后：只能有空白、注释、处理指令。before：根元素之前（遇到 `<名字` 停下，返回 true）
    fn misc(&mut self, before: bool) -> Result<bool, ParseError> {
        loop {
            self.skip_ws();
            let Some(c) = self.peek() else { return Ok(false) };
            if c != '<' {
                return bad("根元素外面有别的东西");
            }
            if self.at("<!--") {
                self.i += 4;
                self.comment()?;
            } else if self.at("<?") {
                self.i += 2;
                if self.pi()?.is_some() {
                    return bad("XML 声明不在开头");
                }
            } else if self.at("<!") {
                return bad("不收带 DOCTYPE 的文档");
            } else if before {
                self.i += 1;
                return Ok(true);
            } else {
                return bad("根元素后面还有元素");
            }
        }
    }

    fn document(&mut self) -> Result<(), ParseError> {
        // 控制字符（制表符、换行、回车以外的）、U+FFFE、U+FFFF 在哪都不行
        if self.s.iter().any(|&c| (c < ' ' && !is_ws(c)) || matches!(c, '\u{fffe}' | '\u{ffff}')) {
            return bad("有不能出现在 XML 里的字符");
        }
        if self.at("<?xml") && self.s.get(5).is_some_and(|&c| is_ws(c) || c == '?') {
            self.i = 2;
            let Some((from, to)) = self.pi()? else { return bad("XML 声明写得不对") };
            let Some((_, encoding)) = parse_xml_decl(&self.s[from..to]) else { return bad("XML 声明写得不对") };
            if let Some(name) = encoding {
                // UTF-16 的文档：声明里要么是 UTF-16，要么是和实际一样的那一种；单字节的在 decode 里看过了
                let fits = match (self.enc, known_encoding(&name)) {
                    (Enc::Utf16 { big }, Some(Enc::Utf16 { big: declared })) => {
                        name.eq_ignore_ascii_case("UTF-16") || big == declared
                    }
                    (Enc::Utf16 { .. }, _) => false,
                    (_, Some(Enc::Utf16 { .. })) | (_, None) => false,
                    _ => true,
                };
                if !fits {
                    return bad("声明的编码和实际的对不上");
                }
            }
        }
        if !self.misc(true)? {
            return bad("没有根元素");
        }
        self.start_tag()?;
        self.content()?;
        self.misc(false)?;
        Ok(())
    }
}

/// `ET.fromstring(data)`
pub fn fromstring(data: &[u8]) -> Result<Tree, ParseError> {
    let (chars, enc) = decode(data)?;
    let mut p = Parser { s: &chars, i: 0, enc, nodes: Vec::new(), open: Vec::new(), bindings: Vec::new() };
    p.document()?;
    Ok(Tree { nodes: p.nodes })
}

/// 对照数据里的一段字节：短的是 base64，长的是压缩以后的（`{"z": base64}`）
#[cfg(test)]
pub fn test_blob(v: &crate::py::obj::V) -> Vec<u8> {
    use crate::py::obj::V;
    match v {
        V::Str(s) => crate::py::base64::b64decode(s, true).expect("base64"),
        V::Dict(d) => {
            let packed = crate::py::base64::b64decode(&d.at("z").py_str(), true).expect("base64");
            crate::py::zlib::decompress_limited(&packed, 64 << 20).expect("zlib").0
        }
        other => panic!("对照数据里这一项不是字节：{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list;
    use crate::py::json;
    use crate::py::obj::{List, V};

    /// `[tag, [[属性, 值]…], text, [子元素…]]`
    fn dump(e: El) -> V {
        let attrib = List::from_vec(e.node().attrib.iter().map(|(k, v)| V::List(list![k.as_str(), v.as_str()])).collect());
        let children = List::from_vec(e.children().into_iter().map(dump).collect());
        V::List(list![e.tag(), attrib, e.text(), children])
    }

    /// 这份文档是不是落在「有意和 Python 不一样」的那两类里：带 DOCTYPE、声明了 expat 自己不会的编码
    fn excused(data: &[u8]) -> bool {
        let wide = data.starts_with(&[0xfe, 0xff])
            || data.starts_with(&[0xff, 0xfe])
            || (data.len() >= 2 && (data[0] == 0 || data[1] == 0));
        let text: String = if wide {
            data.iter().filter(|b| **b != 0).map(|b| *b as char).collect()
        } else {
            data.iter().map(|b| *b as char).collect()
        };
        let lower = text.to_ascii_lowercase();
        if lower.contains("<!doctype") {
            return true;
        }
        match lower.find("encoding") {
            Some(at) => {
                let rest = lower[at + 8..].trim_start().trim_start_matches('=').trim_start();
                let name: String = rest.chars().skip(1).take_while(|c| !matches!(c, '"' | '\'')).collect();
                !matches!(name.as_str(), "utf-8" | "iso-8859-1" | "us-ascii" | "utf-16" | "utf-16be" | "utf-16le")
            }
            None => false,
        }
    }

    #[test]
    fn like_elementtree_on_expat() {
        // 右边是 CPython 3.14（expat 2.7）给的：解析出来的树，或者出错
        let doc = json::loads(include_str!("../../tests/vectors/clients.json")).unwrap();
        let rows = doc.as_dict().unwrap().list("xml").unwrap().items();
        assert!(rows.len() > 900);
        let (mut wrong, mut skipped) = (Vec::new(), 0);
        for row in &rows {
            let row = row.as_list().unwrap();
            let (data, want) = (test_blob(&row.at(0)), row.at(1).as_dict().unwrap());
            let got = fromstring(&data);
            let same = match (&got, want.get("ok")) {
                (Ok(tree), Some(want)) => dump(tree.root()).py_eq(&want),
                (Err(_), None) => true,
                _ => false,
            };
            if same {
                continue;
            }
            // 有意不一样的：Python 读得了（或者出的是 LookupError / ValueError），这里一律读不了
            if got.is_err() && excused(&data) {
                skipped += 1;
                continue;
            }
            wrong.push(format!(
                "{:?}\n   python: {:?}\n   rust:   {:?}",
                String::from_utf8_lossy(&data),
                want,
                got.as_ref().map(|t| dump(t.root())).map_err(|e| e.0.clone())
            ));
        }
        assert!(wrong.is_empty(), "{} 份文档和 Python 不一样：\n{}", wrong.len(), wrong[..wrong.len().min(25)].join("\n"));
        assert!(skipped < rows.len() / 10, "有意不一样的太多了：{skipped}");
    }

    #[test]
    fn reads_a_feed() {
        let t = fromstring(
            br#"<?xml version="1.0" encoding="utf-8"?>
<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" version="2.0"><channel><title>x</title>
<item><sparkle:version>200</sparkle:version><enclosure url="https://a.example.com/x.zip" length="5" sparkle:edSignature="QQ=="/>t</item>
<item/></channel></rss>"#,
        )
        .unwrap();
        let root = t.root();
        assert_eq!((root.tag(), root.get("version"), root.get("nope")), ("rss", Some("2.0"), None));
        let items: Vec<El> = root.findall("channel").into_iter().flat_map(|c| c.findall("item")).collect();
        assert_eq!(items.len(), 2);
        let ns = "{http://www.andymatuschak.org/xml-namespaces/sparkle}";
        assert_eq!(items[0].findtext(&format!("{ns}version")), Some("200"));
        assert_eq!(items[0].findtext(&format!("{ns}shortVersionString")), None);
        assert_eq!(items[0].text(), "");
        let e = items[0].find("enclosure").unwrap();
        assert_eq!(
            (e.get("url"), e.get(&format!("{ns}edSignature")), e.get("sparkle:edSignature")),
            (Some("https://a.example.com/x.zip"), Some("QQ=="), None)
        );
        assert!(items[1].children().is_empty() && items[1].find("enclosure").is_none());
    }

    #[test]
    fn deep_nesting_needs_no_recursion() {
        let n = 200_000;
        let doc = format!("{}{}", "<a>".repeat(n), "</a>".repeat(n));
        let t = fromstring(doc.as_bytes()).unwrap();
        assert_eq!(t.nodes.len(), n);
        assert!(fromstring(format!("{}{}", "<a>".repeat(n), "</a>".repeat(n - 1)).as_bytes()).is_err());
    }
}
