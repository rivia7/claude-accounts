//! TOML：`tomllib.loads` 的移植（CPython 3.14 的 tomllib；原来给旧版 Python 内嵌的 tomli 2.3.0 是同一套代码）。
//!
//! 共享的 config.toml、`-c` 给的设置解析失败必须拦住启动，认什么、不认什么要和原来完全一样，所以不用现成的 TOML 库
//! （它们认 TOML 1.1 的写法，重复定义、不合法的日期报不报错也各有出入），而是把那个解析器一个函数一个函数照搬过来：
//! 函数名、先后、出错的说法和位置都一样。对照数据在 tests/vectors/toml.json（当初用 CPython 生成的，已经固定）。
//!
//! 值是 `py::obj::V`：表是 `Dict`（键的先后就是文件里的先后）、数组是 `List`，字符串、布尔、整数、小数各是各的。
//! - 放不进 i64 的整数是 `V::Big`（十进制数字）。十进制超过 4300 位的 0x / 0o / 0b 整数 Python 自己也转不成十进制
//!   文字（`str()`、`json.dumps` 都是 ValueError），原样留成 `hex()` 的写法（`0x…`）。
//! - 日期和时间在 Python 里是 `datetime` 的对象，`V` 里没有对应的类型：`loads` 给的是 `V::None`。TOML 自己没有空值，
//!   所以结果里的 None 就是“这里是个日期或时间”——它不是字符串、数字、布尔、表、数组里的任何一种，`isinstance` 那类
//!   判断的结果和 Python 一样。要它的值时用 `loads_with` 自己接（`Temporal`）。
//!
//! 嵌套的深度：CPython 的调用栈超过 `sys.getrecursionlimit()`（1000 层）是 RecursionError，原来靠它挡住套得太深的
//! 行内数组、行内表（一层数组两层调用，一层行内表三层）。这里每个照搬过来的函数进门也记一层（`frame()`），数到同一个
//! 上限就是 `TomlError::Recursion`：和 Python（3.12 以上；更早的版本比较运算也占一层）在同一层上出错，Rust 自己的栈
//! 也不会被用完（套到上限时调试版用不到 1MB 的栈）。Python 那边到上限时已经在第几层取决于谁调用的 `loads`，所以有
//! `loads_at`。带点的键、表头不走递归、不受这个上限管：写了上万段的，解析出来的值也套了上万层，用完要经 `dispose`
//! 一层层拆（`V` 自己释放时是递归的）。

// 解析器是从 tomli 2.3.0（https://github.com/hukkin/tomli/tree/2.3.0/src/tomli 的 _parser.py、_re.py）译过来的，
// 原来的版权和许可说明：
//
// MIT License
//
// Copyright (c) 2021 Taneli Hukkinen
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use super::fail::Fail;
use super::obj::{repr_str, Dict, List, V};
use std::borrow::Cow;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// `sys.getrecursionlimit()` 的默认值
pub const RECURSION_LIMIT: usize = 1000;
/// `int()` 认的十进制数字最多几位（`sys.get_int_max_str_digits()` 的默认值）
const INT_MAX_STR_DIGITS: usize = 4300;

/// 解析失败
#[derive(Debug, Clone, PartialEq)]
pub enum TomlError {
    /// `tomllib.TOMLDecodeError`（ValueError 的子类），带着 `str(e)`
    Decode(String),
    /// 别的 ValueError：十进制整数超过 4300 位
    Value(String),
    /// RecursionError：行内数组、行内表套得太深
    Recursion,
}

impl std::fmt::Display for TomlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TomlError::Decode(m) | TomlError::Value(m) => f.write_str(m),
            TomlError::Recursion => f.write_str("maximum recursion depth exceeded"),
        }
    }
}

impl From<TomlError> for Fail {
    /// TOMLDecodeError 是 ValueError；RecursionError 不是（`except ValueError` 接不住它）
    fn from(e: TomlError) -> Fail {
        match e {
            TomlError::Recursion => Fail::Other("RecursionError", vec![e.to_string()]),
            TomlError::Decode(m) | TomlError::Value(m) => Fail::Value(m),
        }
    }
}

/// 日期、时间的值：只有 date 的是 `datetime.date`，只有 time 的是 `datetime.time`，两样都有的是 `datetime.datetime`
/// （offset 是时区偏移的分钟数，None 是没写时区的本地时间）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Temporal {
    /// 年、月、日
    pub date: Option<(u32, u32, u32)>,
    /// 时、分、秒、微秒
    pub time: Option<(u32, u32, u32, u32)>,
    pub offset: Option<i32>,
}

impl Temporal {
    /// `x.isoformat()`
    #[cfg(test)]
    pub fn isoformat(&self) -> String {
        let mut out = String::new();
        if let Some((y, m, d)) = self.date {
            out.push_str(&format!("{y:04}-{m:02}-{d:02}"));
        }
        if let Some((h, m, s, us)) = self.time {
            if self.date.is_some() {
                out.push('T');
            }
            out.push_str(&format!("{h:02}:{m:02}:{s:02}"));
            if us != 0 {
                out.push_str(&format!(".{us:06}"));
            }
        }
        if let Some(off) = self.offset {
            out.push_str(&format!("{}{:02}:{:02}", if off < 0 { '-' } else { '+' }, off.abs() / 60, off.abs() % 60));
        }
        out
    }
}

/// `tomllib.loads(s)`，像从脚本最外层调用的那样（`loads` 自己在调用栈的第 2 层）
#[cfg(test)]
pub fn loads(s: &str) -> Result<Dict, TomlError> {
    loads_at(s, 2)
}

/// `tomllib.loads(s)`，`loads` 自己在 Python 调用栈的第 frame 层（最外层的模块是第 1 层）：套得太深在哪一层出错跟着它走
pub fn loads_at(s: &str, frame: usize) -> Result<Dict, TomlError> {
    loads_with(s, frame, &|_| V::None)
}

/// 同上，日期和时间的值交给 temporal 换成 `V`。它不能给表或数组：解析时要靠类型分清哪些是文件里写的表和数组
/// （tomllib 对 `parse_float` 也是这个要求）
pub fn loads_with(s: &str, frame: usize, temporal: &dyn Fn(&Temporal) -> V) -> Result<Dict, TomlError> {
    // The spec allows converting "\r\n" to "\n", even in string literals. Let's do so to simplify parsing.
    let src: Cow<str> = if s.contains("\r\n") { Cow::Owned(s.replace("\r\n", "\n")) } else { Cow::Borrowed(s) };
    Parser { src: &src, b: src.as_bytes(), depth: Cell::new(frame), temporal }.loads()
}

type Key = Vec<Rc<str>>;
type Res<T> = Result<T, TomlError>;

// Flags 的两种标记
/// Marks an immutable namespace (inline array or inline table).
const FROZEN: u8 = 1;
/// Marks a nest that has been explicitly created and can no longer be opened using the "[table]" syntax.
const EXPLICIT_NEST: u8 = 2;

#[derive(Default)]
struct FlagNode {
    flags: u8,
    recursive_flags: u8,
    /// 下一层：键 → 在 `Flags::nodes` 里的位置
    nested: HashMap<Rc<str>, usize>,
}

/// Flags that map to parsed keys/namespaces. 原来是一层套一层的字典，这里的节点都放在一个数组里（`nodes[0]` 是根，
/// 只用它的 `nested`）：键写得再深，用完释放时也不用一层层递归
struct Flags {
    nodes: Vec<FlagNode>,
    pending: HashSet<(Key, u8)>,
}

impl Flags {
    fn new() -> Flags {
        Flags { nodes: vec![FlagNode::default()], pending: HashSet::new() }
    }

    fn child(&self, at: usize, k: &str) -> Option<usize> {
        self.nodes[at].nested.get(k).copied()
    }

    fn add_pending(&mut self, key: Key, flag: u8) {
        self.pending.insert((key, flag));
    }

    fn finalize_pending(&mut self) {
        for (key, flag) in std::mem::take(&mut self.pending) {
            self.set(&key, flag, false);
        }
    }

    fn unset_all(&mut self, key: &[Rc<str>]) {
        let Some((stem, parent)) = key.split_last() else { return };
        let mut at = 0;
        for k in parent {
            match self.child(at, k) {
                Some(i) => at = i,
                None => return,
            }
        }
        self.nodes[at].nested.remove(&**stem);
    }

    fn set(&mut self, key: &[Rc<str>], flag: u8, recursive: bool) {
        let mut at = 0;
        for k in key {
            at = match self.child(at, k) {
                Some(i) => i,
                None => {
                    let i = self.nodes.len();
                    self.nodes.push(FlagNode::default());
                    self.nodes[at].nested.insert(k.clone(), i);
                    i
                }
            };
        }
        let node = &mut self.nodes[at];
        if recursive {
            node.recursive_flags |= flag;
        } else {
            node.flags |= flag;
        }
    }

    fn is_(&self, key: &[Rc<str>], flag: u8) -> bool {
        let Some((stem, parent)) = key.split_last() else {
            return false; // document root has no flags
        };
        let mut at = 0;
        for k in parent {
            let Some(i) = self.child(at, k) else { return false };
            if self.nodes[i].recursive_flags & flag != 0 {
                return true;
            }
            at = i;
        }
        self.child(at, stem).is_some_and(|i| (self.nodes[i].flags | self.nodes[i].recursive_flags) & flag != 0)
    }
}

/// `NestedDict.get_or_create_nest`。Err 是原来的 KeyError（这个键后面不是表）
fn get_or_create_nest(root: &Dict, key: &[Rc<str>], access_lists: bool) -> Result<Dict, ()> {
    let mut cont = root.clone();
    for k in key {
        let mut next = match cont.get(k) {
            Some(v) => v,
            None => {
                let d = Dict::new();
                cont.set(k.clone(), &d);
                V::Dict(d)
            }
        };
        if access_lists {
            if let V::List(l) = &next {
                next = l.at(-1);
            }
        }
        match next {
            V::Dict(d) => cont = d,
            _ => return Err(()), // There is no nest behind this key
        }
    }
    Ok(cont)
}

/// `NestedDict.append_nest_to_list`
fn append_nest_to_list(root: &Dict, key: &[Rc<str>]) -> Result<(), ()> {
    let (last_key, parent) = key.split_last().expect("键至少有一段");
    let cont = get_or_create_nest(root, parent, true)?;
    match cont.get(last_key) {
        Some(V::List(l)) => l.push(Dict::new()),
        Some(_) => return Err(()), // An object other than list found behind this key
        None => cont.set(last_key.clone(), List::from_vec(vec![V::Dict(Dict::new())])),
    }
    Ok(())
}

struct Output {
    data: Dict,
    flags: Flags,
}

impl Drop for Output {
    /// 解析到一半出错时，已经建好的那部分也要一层层拆（成功时 data 已经交出去，这里是空的）
    fn drop(&mut self) {
        dispose(std::mem::take(&mut self.data));
    }
}

/// 把解析出来的表一层层拆开再释放。带点的键、表头写了上万段时值也跟着套了上万层，让 Rust 自己递归着释放会把栈用完
/// （Python 释放这么深的字典不出事）：整份配置用完以后调它
pub fn dispose(table: Dict) {
    let mut pending = vec![V::Dict(table)];
    while let Some(v) = pending.pop() {
        // 先把里面的各项接过来、从它身上摘掉，它自己就只剩一个空壳
        match &v {
            V::Dict(d) => {
                pending.extend(d.values());
                d.clear();
            }
            V::List(l) => {
                pending.extend(l.items());
                l.clear();
            }
            _ => {}
        }
    }
}

/// TOML_WS
fn is_ws(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

/// TOML_WS_AND_NEWLINE
fn is_ws_or_newline(c: u8) -> bool {
    is_ws(c) || c == b'\n'
}

/// BARE_KEY_CHARS
fn is_bare_key_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_'
}

/// ILLEGAL_BASIC_STR_CHARS（也是 ILLEGAL_LITERAL_STR_CHARS、ILLEGAL_COMMENT_CHARS）：制表符以外的控制字符
fn illegal_one_line(c: u8) -> bool {
    (c < 0x20 && c != b'\t') || c == 0x7f
}

/// ILLEGAL_MULTILINE_BASIC_STR_CHARS（也是 ILLEGAL_MULTILINE_LITERAL_STR_CHARS）：换行也可以有
fn illegal_multiline(c: u8) -> bool {
    illegal_one_line(c) && c != b'\n'
}

/// `repr(一个 ASCII 字符)`
fn char_repr(c: u8) -> String {
    repr_str(&(c as char).to_string())
}

/// `str(键)`：键是字符串的元组，`('a', 'b')`，只有一段时是 `('a',)`
fn key_repr(key: &[Rc<str>]) -> String {
    let parts: Vec<String> = key.iter().map(|k| repr_str(k)).collect();
    if parts.len() == 1 {
        format!("({},)", parts[0])
    } else {
        format!("({})", parts.join(", "))
    }
}

/// 调用栈上的一层：离开作用域时退回去
struct Frame<'p>(&'p Cell<usize>);

impl Drop for Frame<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

struct Parser<'a> {
    src: &'a str,
    /// src 的字节。位置都是字节位置：有语法含义的字符全是 ASCII，多字节字符只会整个出现在字符串、注释里
    b: &'a [u8],
    /// 现在相当于 Python 调用栈的第几层
    depth: Cell<usize>,
    temporal: &'a dyn Fn(&Temporal) -> V,
}

impl Parser<'_> {
    /// 进一个照搬过来的 Python 函数：调用栈深一层，超过上限就是 RecursionError
    fn frame(&self) -> Res<Frame<'_>> {
        let d = self.depth.get() + 1;
        if d > RECURSION_LIMIT {
            return Err(TomlError::Recursion);
        }
        self.depth.set(d);
        Ok(Frame(&self.depth))
    }

    /// 调一个不再往下调用的 Python 函数（`match_to_number`、`is_unicode_scalar_value` 这些没有单独搬成函数的）
    fn leaf(&self) -> Res<()> {
        self.frame().map(drop)
    }

    /// `TOMLDecodeError(msg, src, pos)`：说法后面带着出错的行和列（按字符数）。造这个异常也是一次调用（`__init__`），
    /// 已经在最深一层时出来的是 RecursionError
    fn err(&self, msg: impl AsRef<str>, pos: usize) -> TomlError {
        if self.depth.get() + 1 > RECURSION_LIMIT {
            return TomlError::Recursion;
        }
        let coord = if pos >= self.b.len() {
            "end of document".to_string()
        } else {
            let head = &self.src[..pos];
            let lineno = head.matches('\n').count() + 1;
            let colno = match head.rfind('\n') {
                Some(nl) => head[nl + 1..].chars().count() + 1,
                None => head.chars().count() + 1,
            };
            format!("line {lineno}, column {colno}")
        };
        TomlError::Decode(format!("{} (at {})", msg.as_ref(), coord))
    }

    /// Parse TOML from a string.
    fn loads(&self) -> Res<Dict> {
        let mut pos = 0;
        let mut out = Output { data: Dict::new(), flags: Flags::new() };
        let mut header: Key = Vec::new();
        // Parse one statement at a time (typically means one line in TOML source)
        loop {
            // 1. Skip line leading whitespace
            pos = self.skip_chars(pos, is_ws)?;
            // 2. Parse rules. Expect one of the following:
            //    - end of file
            //    - end of line
            //    - comment
            //    - key/value pair
            //    - append dict to list (and move to its namespace)
            //    - create dict (and move to its namespace)
            // Skip trailing whitespace when applicable.
            let Some(&c) = self.b.get(pos) else { break };
            if c == b'\n' {
                pos += 1;
                continue;
            }
            if is_bare_key_char(c) || c == b'"' || c == b'\'' {
                pos = self.key_value_rule(pos, &mut out, &header)?;
                pos = self.skip_chars(pos, is_ws)?;
            } else if c == b'[' {
                out.flags.finalize_pending();
                (pos, header) = if self.b.get(pos + 1) == Some(&b'[') {
                    self.create_list_rule(pos, &mut out)?
                } else {
                    self.create_dict_rule(pos, &mut out)?
                };
                pos = self.skip_chars(pos, is_ws)?;
            } else if c != b'#' {
                return Err(self.err("Invalid statement", pos));
            }
            // 3. Skip comment
            pos = self.skip_comment(pos)?;
            // 4. Expect end of line or end of file
            let Some(&c) = self.b.get(pos) else { break };
            if c != b'\n' {
                return Err(self.err("Expected newline or end of document after a statement", pos));
            }
            pos += 1;
        }
        Ok(std::mem::take(&mut out.data))
    }

    fn skip_chars(&self, mut pos: usize, chars: fn(u8) -> bool) -> Res<usize> {
        let _f = self.frame()?;
        while self.b.get(pos).is_some_and(|c| chars(*c)) {
            pos += 1;
        }
        Ok(pos)
    }

    fn skip_until(&self, pos: usize, expect: &str, error_on: fn(u8) -> bool, error_on_eof: bool) -> Res<usize> {
        let _f = self.frame()?;
        let new_pos = match self.src[pos..].find(expect) {
            Some(i) => pos + i,
            None => {
                if error_on_eof {
                    return Err(self.err(format!("Expected {}", repr_str(expect)), self.b.len()));
                }
                self.b.len()
            }
        };
        if let Some(i) = self.b[pos..new_pos].iter().position(|c| error_on(*c)) {
            return Err(self.err(format!("Found invalid character {}", char_repr(self.b[pos + i])), pos + i));
        }
        Ok(new_pos)
    }

    fn skip_comment(&self, pos: usize) -> Res<usize> {
        let _f = self.frame()?;
        if self.b.get(pos) == Some(&b'#') {
            return self.skip_until(pos + 1, "\n", illegal_one_line, false);
        }
        Ok(pos)
    }

    fn skip_comments_and_array_ws(&self, mut pos: usize) -> Res<usize> {
        let _f = self.frame()?;
        loop {
            let pos_before_skip = pos;
            pos = self.skip_chars(pos, is_ws_or_newline)?;
            pos = self.skip_comment(pos)?;
            if pos == pos_before_skip {
                return Ok(pos);
            }
        }
    }

    fn create_dict_rule(&self, pos: usize, out: &mut Output) -> Res<(usize, Key)> {
        let _f = self.frame()?;
        let pos = self.skip_chars(pos + 1, is_ws)?; // Skip "["
        let (pos, key) = self.parse_key(pos)?;
        if out.flags.is_(&key, EXPLICIT_NEST) || out.flags.is_(&key, FROZEN) {
            return Err(self.err(format!("Cannot declare {} twice", key_repr(&key)), pos));
        }
        out.flags.set(&key, EXPLICIT_NEST, false);
        if get_or_create_nest(&out.data, &key, true).is_err() {
            return Err(self.err("Cannot overwrite a value", pos));
        }
        if self.b.get(pos) != Some(&b']') {
            return Err(self.err("Expected ']' at the end of a table declaration", pos));
        }
        Ok((pos + 1, key))
    }

    fn create_list_rule(&self, pos: usize, out: &mut Output) -> Res<(usize, Key)> {
        let _f = self.frame()?;
        let pos = self.skip_chars(pos + 2, is_ws)?; // Skip "[["
        let (pos, key) = self.parse_key(pos)?;
        if out.flags.is_(&key, FROZEN) {
            return Err(self.err(format!("Cannot mutate immutable namespace {}", key_repr(&key)), pos));
        }
        // Free the namespace now that it points to another empty list item...
        out.flags.unset_all(&key);
        // ...but this key precisely is still prohibited from table declaration
        out.flags.set(&key, EXPLICIT_NEST, false);
        if append_nest_to_list(&out.data, &key).is_err() {
            return Err(self.err("Cannot overwrite a value", pos));
        }
        if !self.b[pos..].starts_with(b"]]") {
            return Err(self.err("Expected ']]' at the end of an array declaration", pos));
        }
        Ok((pos + 2, key))
    }

    fn key_value_rule(&self, pos: usize, out: &mut Output, header: &[Rc<str>]) -> Res<usize> {
        let _f = self.frame()?;
        let (pos, key, value) = self.parse_key_value_pair(pos)?;
        let (key_stem, key_parent) = key.split_last().expect("键至少有一段");
        let abs_key_parent: Key = [header, key_parent].concat();
        for i in 1..key.len() {
            let cont_key: Key = [header, &key[..i]].concat();
            // Check that dotted key syntax does not redefine an existing table
            if out.flags.is_(&cont_key, EXPLICIT_NEST) {
                return Err(self.err(format!("Cannot redefine namespace {}", key_repr(&cont_key)), pos));
            }
            // Containers in the relative path can't be opened with the table syntax or
            // dotted key/value syntax in following table sections.
            out.flags.add_pending(cont_key, EXPLICIT_NEST);
        }
        if out.flags.is_(&abs_key_parent, FROZEN) {
            return Err(self.err(format!("Cannot mutate immutable namespace {}", key_repr(&abs_key_parent)), pos));
        }
        let Ok(nest) = get_or_create_nest(&out.data, &abs_key_parent, true) else {
            return Err(self.err("Cannot overwrite a value", pos));
        };
        if nest.has(key_stem) {
            return Err(self.err("Cannot overwrite a value", pos));
        }
        // Mark inline table and array namespaces recursively immutable
        if matches!(value, V::Dict(_) | V::List(_)) {
            out.flags.set(&[header, &key[..]].concat(), FROZEN, true);
        }
        nest.set(key_stem.clone(), value);
        Ok(pos)
    }

    fn parse_key_value_pair(&self, pos: usize) -> Res<(usize, Key, V)> {
        let _f = self.frame()?;
        let (pos, key) = self.parse_key(pos)?;
        if self.b.get(pos) != Some(&b'=') {
            return Err(self.err("Expected '=' after a key in a key/value pair", pos));
        }
        let pos = self.skip_chars(pos + 1, is_ws)?;
        let (pos, value) = self.parse_value(pos)?;
        Ok((pos, key, value))
    }

    fn parse_key(&self, pos: usize) -> Res<(usize, Key)> {
        let _f = self.frame()?;
        let (pos, key_part) = self.parse_key_part(pos)?;
        let mut key: Key = vec![Rc::from(key_part)];
        let mut pos = self.skip_chars(pos, is_ws)?;
        loop {
            if self.b.get(pos) != Some(&b'.') {
                return Ok((pos, key));
            }
            pos = self.skip_chars(pos + 1, is_ws)?;
            let (p, key_part) = self.parse_key_part(pos)?;
            key.push(Rc::from(key_part));
            pos = self.skip_chars(p, is_ws)?;
        }
    }

    fn parse_key_part(&self, pos: usize) -> Res<(usize, String)> {
        let _f = self.frame()?;
        match self.b.get(pos) {
            Some(c) if is_bare_key_char(*c) => {
                let end = self.skip_chars(pos, is_bare_key_char)?;
                Ok((end, self.src[pos..end].to_string()))
            }
            Some(b'\'') => self.parse_literal_str(pos),
            Some(b'"') => self.parse_one_line_basic_str(pos),
            _ => Err(self.err("Invalid initial character for a key part", pos)),
        }
    }

    fn parse_one_line_basic_str(&self, pos: usize) -> Res<(usize, String)> {
        let _f = self.frame()?;
        self.parse_basic_str(pos + 1, false)
    }

    fn parse_array(&self, pos: usize) -> Res<(usize, Vec<V>)> {
        let _f = self.frame()?;
        let mut array = Vec::new();
        let mut pos = self.skip_comments_and_array_ws(pos + 1)?;
        if self.b.get(pos) == Some(&b']') {
            return Ok((pos + 1, array));
        }
        loop {
            let (p, val) = self.parse_value(pos)?;
            array.push(val);
            pos = self.skip_comments_and_array_ws(p)?;
            match self.b.get(pos) {
                Some(b']') => return Ok((pos + 1, array)),
                Some(b',') => {}
                _ => return Err(self.err("Unclosed array", pos)),
            }
            pos = self.skip_comments_and_array_ws(pos + 1)?;
            if self.b.get(pos) == Some(&b']') {
                return Ok((pos + 1, array));
            }
        }
    }

    fn parse_inline_table(&self, pos: usize) -> Res<(usize, Dict)> {
        let _f = self.frame()?;
        let nested_dict = Dict::new();
        let mut flags = Flags::new();
        let mut pos = self.skip_chars(pos + 1, is_ws)?;
        if self.b.get(pos) == Some(&b'}') {
            return Ok((pos + 1, nested_dict));
        }
        loop {
            let (p, key, value) = self.parse_key_value_pair(pos)?;
            let nests = matches!(value, V::Dict(_) | V::List(_));
            self.inline_table_set(&nested_dict, &flags, &key, value, p)?;
            pos = self.skip_chars(p, is_ws)?;
            match self.b.get(pos) {
                Some(b'}') => return Ok((pos + 1, nested_dict)),
                Some(b',') => {}
                _ => return Err(self.err("Unclosed inline table", pos)),
            }
            if nests {
                flags.set(&key, FROZEN, true);
            }
            pos = self.skip_chars(pos + 1, is_ws)?;
        }
    }

    /// `parse_inline_table` 里把一对键和值放进表的那几句。单拿出来是为了让套在递归路上的那个函数占的栈小一些
    #[inline(never)]
    fn inline_table_set(&self, nested_dict: &Dict, flags: &Flags, key: &[Rc<str>], value: V, pos: usize) -> Res<()> {
        let (key_stem, key_parent) = key.split_last().expect("键至少有一段");
        if flags.is_(key, FROZEN) {
            return Err(self.err(format!("Cannot mutate immutable namespace {}", key_repr(key)), pos));
        }
        let Ok(nest) = get_or_create_nest(nested_dict, key_parent, false) else {
            return Err(self.err("Cannot overwrite a value", pos));
        };
        if nest.has(key_stem) {
            return Err(self.err(format!("Duplicate inline table key {}", repr_str(key_stem)), pos));
        }
        nest.set(key_stem.clone(), value);
        Ok(())
    }

    fn parse_basic_str_escape(&self, pos: usize, multiline: bool) -> Res<(usize, String)> {
        let _f = self.frame()?;
        // escape_id = src[pos:pos + 2]：反斜杠和它后面那个字符（文件到头了就只有反斜杠），pos 跟着越过它们
        let next = self.src[pos + 1..].chars().next();
        let pos = pos + 1 + next.map_or(0, char::len_utf8);
        if multiline && matches!(next, Some(' ' | '\t' | '\n')) {
            // Skip whitespace until next non-whitespace character or end of
            // the doc. Error if non-whitespace is found before newline.
            let mut pos = pos;
            if next != Some('\n') {
                pos = self.skip_chars(pos, is_ws)?;
                match self.b.get(pos) {
                    None => return Ok((pos, String::new())),
                    Some(b'\n') => pos += 1,
                    Some(_) => return Err(self.err("Unescaped '\\' in a string", pos)),
                }
            }
            pos = self.skip_chars(pos, is_ws_or_newline)?;
            return Ok((pos, String::new()));
        }
        let replacement = match next {
            Some('u') => return self.parse_hex_char(pos, 4),
            Some('U') => return self.parse_hex_char(pos, 8),
            // BASIC_STR_ESCAPE_REPLACEMENTS
            Some('b') => '\u{8}',
            Some('t') => '\t',
            Some('n') => '\n',
            Some('f') => '\u{c}',
            Some('r') => '\r',
            Some('"') => '"',
            Some('\\') => '\\',
            _ => return Err(self.err("Unescaped '\\' in a string", pos)),
        };
        Ok((pos, replacement.to_string()))
    }

    fn parse_basic_str_escape_multiline(&self, pos: usize) -> Res<(usize, String)> {
        let _f = self.frame()?;
        self.parse_basic_str_escape(pos, true)
    }

    fn parse_hex_char(&self, pos: usize, hex_len: usize) -> Res<(usize, String)> {
        let _f = self.frame()?;
        // hex_str = src[pos:pos + hex_len]，按字符数：都是十六进制数字的话也就是这么多个字节
        let hex_ok = self.src[pos..].chars().take(hex_len).filter(char::is_ascii_hexdigit).count() == hex_len;
        if !hex_ok {
            return Err(self.err("Invalid hex value", pos));
        }
        let hex_int = u32::from_str_radix(&self.src[pos..pos + hex_len], 16).expect("刚核对过是十六进制数字");
        let pos = pos + hex_len;
        self.leaf()?; // is_unicode_scalar_value
        match char::from_u32(hex_int) {
            Some(c) => Ok((pos, c.to_string())),
            None => Err(self.err("Escaped character is not a Unicode scalar value", pos)),
        }
    }

    fn parse_literal_str(&self, pos: usize) -> Res<(usize, String)> {
        let _f = self.frame()?;
        let start_pos = pos + 1; // Skip starting apostrophe
        let pos = self.skip_until(start_pos, "'", illegal_one_line, true)?;
        Ok((pos + 1, self.src[start_pos..pos].to_string())) // Skip ending apostrophe
    }

    fn parse_multiline_str(&self, pos: usize, literal: bool) -> Res<(usize, String)> {
        let _f = self.frame()?;
        let mut pos = pos + 3;
        if self.b.get(pos) == Some(&b'\n') {
            pos += 1;
        }
        let (delim, mut result);
        if literal {
            delim = '\'';
            let end_pos = self.skip_until(pos, "'''", illegal_multiline, true)?;
            result = self.src[pos..end_pos].to_string();
            pos = end_pos + 3;
        } else {
            delim = '"';
            (pos, result) = self.parse_basic_str(pos, true)?;
        }
        // Add at maximum two extra apostrophes/quotes if the end sequence
        // is 4 or 5 chars long instead of just 3.
        for _ in 0..2 {
            if self.b.get(pos) != Some(&(delim as u8)) {
                break;
            }
            pos += 1;
            result.push(delim);
        }
        Ok((pos, result))
    }

    fn parse_basic_str(&self, mut pos: usize, multiline: bool) -> Res<(usize, String)> {
        let _f = self.frame()?;
        let error_on = if multiline { illegal_multiline } else { illegal_one_line };
        let mut result = String::new();
        let mut start_pos = pos;
        loop {
            let Some(&c) = self.b.get(pos) else {
                return Err(self.err("Unterminated string", pos));
            };
            if c == b'"' {
                if !multiline {
                    result.push_str(&self.src[start_pos..pos]);
                    return Ok((pos + 1, result));
                }
                if self.b[pos..].starts_with(b"\"\"\"") {
                    result.push_str(&self.src[start_pos..pos]);
                    return Ok((pos + 3, result));
                }
                pos += 1;
                continue;
            }
            if c == b'\\' {
                result.push_str(&self.src[start_pos..pos]);
                let (p, parsed_escape) =
                    if multiline { self.parse_basic_str_escape_multiline(pos)? } else { self.parse_basic_str_escape(pos, false)? };
                result.push_str(&parsed_escape);
                pos = p;
                start_pos = pos;
                continue;
            }
            if error_on(c) {
                return Err(self.err(format!("Illegal character {}", char_repr(c)), pos));
            }
            pos += 1;
        }
    }

    fn parse_value(&self, pos: usize) -> Res<(usize, V)> {
        let _f = self.frame()?;
        match self.b.get(pos) {
            // Arrays
            Some(b'[') => self.parse_array(pos).map(|(pos, array)| (pos, V::List(List::from_vec(array)))),
            // Inline tables
            Some(b'{') => self.parse_inline_table(pos).map(|(pos, table)| (pos, V::Dict(table))),
            _ => self.parse_scalar(pos),
        }
    }

    /// `parse_value` 里数组、行内表以外的那些（不会再往下套）。原来是同一个函数，照原来的先后一样一样试；拆出来是为了
    /// 让套在递归路上的 `parse_value` 占的栈小一些，在 Python 那边不另算一层调用
    #[inline(never)]
    fn parse_scalar(&self, pos: usize) -> Res<(usize, V)> {
        let rest = &self.b[pos..];
        // IMPORTANT: order conditions based on speed of checking and likelihood
        match rest.first() {
            // Basic strings
            Some(b'"') => {
                let (pos, s) = if rest.starts_with(b"\"\"\"") {
                    self.parse_multiline_str(pos, false)?
                } else {
                    self.parse_one_line_basic_str(pos)?
                };
                return Ok((pos, V::from(s)));
            }
            // Literal strings
            Some(b'\'') => {
                let (pos, s) =
                    if rest.starts_with(b"'''") { self.parse_multiline_str(pos, true)? } else { self.parse_literal_str(pos)? };
                return Ok((pos, V::from(s)));
            }
            // Booleans
            Some(b't') if rest.starts_with(b"true") => return Ok((pos + 4, V::Bool(true))),
            Some(b'f') if rest.starts_with(b"false") => return Ok((pos + 5, V::Bool(false))),
            _ => {}
        }
        // Dates and times
        if let Some((end, datetime)) = match_datetime(self.b, pos) {
            self.leaf()?; // match_to_datetime
            if !valid_date(&datetime) {
                return Err(self.err("Invalid date or datetime", pos));
            }
            return Ok((end, self.temporal_value(&datetime)));
        }
        if let Some((end, time)) = match_time(self.b, pos) {
            self.leaf()?; // match_to_localtime
            return Ok((end, self.temporal_value(&Temporal { date: None, time: Some(time), offset: None })));
        }
        // Integers and "normal" floats.
        // The regex will greedily match any type starting with a decimal
        // char, so needs to be located after handling of dates and times.
        if let Some((end, floatpart)) = match_number(self.b, pos) {
            self.leaf()?; // match_to_number
            let text = &self.src[pos..end];
            return Ok((end, if floatpart { V::Float(py_float(text)) } else { py_int(text)? }));
        }
        // Special floats
        if rest.starts_with(b"inf") {
            return Ok((pos + 3, V::Float(f64::INFINITY)));
        }
        if rest.starts_with(b"nan") {
            return Ok((pos + 3, V::Float(f64::NAN)));
        }
        if rest.starts_with(b"+inf") {
            return Ok((pos + 4, V::Float(f64::INFINITY)));
        }
        if rest.starts_with(b"-inf") {
            return Ok((pos + 4, V::Float(f64::NEG_INFINITY)));
        }
        if rest.starts_with(b"+nan") || rest.starts_with(b"-nan") {
            return Ok((pos + 4, V::Float(f64::NAN)));
        }
        Err(self.err("Invalid value", pos))
    }

    fn temporal_value(&self, t: &Temporal) -> V {
        let v = (self.temporal)(t);
        assert!(!matches!(v, V::Dict(_) | V::List(_)), "日期、时间不能换成表或数组");
        v
    }
}

/// 位置 i 起的两位数字
fn two_digits(b: &[u8], i: usize) -> Option<u32> {
    match (b.get(i), b.get(i + 1)) {
        (Some(x @ b'0'..=b'9'), Some(y @ b'0'..=b'9')) => Some(((x - b'0') * 10 + (y - b'0')) as u32),
        _ => None,
    }
}

/// `RE_LOCALTIME.match(src, pos)`：`([01][0-9]|2[0-3]):([0-5][0-9]):([0-5][0-9])(?:\.([0-9]{1,6})[0-9]*)?`。
/// 返回结束的位置和（时、分、秒、微秒）
fn match_time(b: &[u8], pos: usize) -> Option<(usize, (u32, u32, u32, u32))> {
    let hour = two_digits(b, pos).filter(|h| *h <= 23)?;
    if b.get(pos + 2) != Some(&b':') {
        return None;
    }
    let minute = two_digits(b, pos + 3).filter(|m| *m <= 59)?;
    if b.get(pos + 5) != Some(&b':') {
        return None;
    }
    let sec = two_digits(b, pos + 6).filter(|s| *s <= 59)?;
    let mut end = pos + 8;
    let mut micros = 0;
    if b.get(end) == Some(&b'.') && b.get(end + 1).is_some_and(u8::is_ascii_digit) {
        let start = end + 1;
        end = start;
        while b.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        // 前 6 位是微秒，不够的在后面补 0，多出来的不要：int(micros_str.ljust(6, "0"))
        micros = b[start..end].iter().map(|c| (c - b'0') as u32).chain(std::iter::repeat(0)).take(6).fold(0, |acc, d| acc * 10 + d);
    }
    Some((end, (hour, minute, sec, micros)))
}

/// `RE_DATETIME.match(src, pos)`：日期，后面可以跟 `T`（或 `t`、空格）和时间，再后面可以跟 `Z` 或 `±HH:MM`。
/// 只照写法认，日期是不是真有这一天由 `valid_date` 看
fn match_datetime(b: &[u8], pos: usize) -> Option<(usize, Temporal)> {
    let year = two_digits(b, pos)? * 100 + two_digits(b, pos + 2)?;
    if b.get(pos + 4) != Some(&b'-') {
        return None;
    }
    let month = two_digits(b, pos + 5).filter(|m| (1..=12).contains(m))?;
    if b.get(pos + 7) != Some(&b'-') {
        return None;
    }
    let day = two_digits(b, pos + 8).filter(|d| (1..=31).contains(d))?;
    let mut end = pos + 10;
    let (mut time, mut offset) = (None, None);
    if matches!(b.get(end), Some(b'T' | b't' | b' ')) {
        if let Some((e, t)) = match_time(b, end + 1) {
            (end, time) = (e, Some(t));
            match b.get(end) {
                Some(b'Z' | b'z') => {
                    offset = Some(0);
                    end += 1;
                }
                Some(sign @ (b'+' | b'-')) => {
                    let hour = two_digits(b, end + 1).filter(|h| *h <= 23);
                    let minute = two_digits(b, end + 4).filter(|m| *m <= 59);
                    if let (Some(h), Some(b':'), Some(m)) = (hour, b.get(end + 3), minute) {
                        let minutes = (h * 60 + m) as i32;
                        offset = Some(if *sign == b'+' { minutes } else { -minutes });
                        end += 6;
                    }
                }
                _ => {}
            }
        }
    }
    Some((end, Temporal { date: Some((year, month, day)), time, offset }))
}

/// `datetime.date(年, 月, 日)` 收不收：年从 1 起，日不超过那个月的天数（时间的范围写法上已经限定了）
fn valid_date(t: &Temporal) -> bool {
    let Some((year, month, day)) = t.date else { return true };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        2 => 28 + leap as u32,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    year >= 1 && day <= days
}

fn is_bin_digit(c: &u8) -> bool {
    matches!(c, b'0' | b'1')
}

fn is_oct_digit(c: &u8) -> bool {
    matches!(c, b'0'..=b'7')
}

/// `(?:_?数字)*`：一串数字，中间可以夹单个下划线。返回结束的位置
fn digit_run(b: &[u8], mut i: usize, digit: fn(&u8) -> bool) -> usize {
    loop {
        if b.get(i).is_some_and(digit) {
            i += 1;
        } else if b.get(i) == Some(&b'_') && b.get(i + 1).is_some_and(digit) {
            i += 2;
        } else {
            return i;
        }
    }
}

/// `RE_NUMBER.match(src, pos)`：0x / 0b / 0o 开头的整数，或者十进制的整数、小数（数字之间可以有单个下划线）。
/// 返回结束的位置和有没有小数部分或指数（`match.group("floatpart")`）
fn match_number(b: &[u8], pos: usize) -> Option<(usize, bool)> {
    if b.get(pos) == Some(&b'0') {
        let digit: Option<fn(&u8) -> bool> = match b.get(pos + 1) {
            Some(b'x') => Some(u8::is_ascii_hexdigit),
            Some(b'b') => Some(is_bin_digit),
            Some(b'o') => Some(is_oct_digit),
            _ => None,
        };
        if let Some(digit) = digit {
            if b.get(pos + 2).is_some_and(digit) {
                return Some((digit_run(b, pos + 3, digit), false));
            }
        }
    }
    // [+-]?(?:0|[1-9](?:_?[0-9])*)
    let mut i = pos;
    if matches!(b.get(i), Some(b'+' | b'-')) {
        i += 1;
    }
    match b.get(i) {
        Some(b'0') => i += 1,
        Some(b'1'..=b'9') => i = digit_run(b, i + 1, u8::is_ascii_digit),
        _ => return None,
    }
    let int_end = i;
    // (?:\.[0-9](?:_?[0-9])*)?
    if b.get(i) == Some(&b'.') && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
        i = digit_run(b, i + 2, u8::is_ascii_digit);
    }
    // (?:[eE][+-]?[0-9](?:_?[0-9])*)?
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        if b.get(j).is_some_and(u8::is_ascii_digit) {
            i = digit_run(b, j + 1, u8::is_ascii_digit);
        }
    }
    Some((i, i > int_end))
}

/// `float(文字)`：RE_NUMBER 认出来的小数（下划线去掉；太大是 inf，太小是 0.0）
fn py_float(text: &str) -> f64 {
    text.replace('_', "").parse().expect("照 RE_NUMBER 认出来的小数")
}

/// `int(文字, 0)`：RE_NUMBER 认出来的整数
fn py_int(text: &str) -> Res<V> {
    let digits = text.replace('_', "");
    for (prefix, radix) in [("0x", 16), ("0o", 8), ("0b", 2)] {
        if let Some(d) = digits.strip_prefix(prefix) {
            return Ok(match i64::from_str_radix(d, radix) {
                Ok(n) => V::Int(n),
                Err(_) => V::Big(Rc::from(big_from_radix(d, radix))),
            });
        }
    }
    let (negative, d) = match digits.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, digits.strip_prefix('+').unwrap_or(&digits)),
    };
    if d.len() > INT_MAX_STR_DIGITS {
        return Err(TomlError::Value(format!(
            "Exceeds the limit ({INT_MAX_STR_DIGITS} digits) for integer string conversion: value has {} digits; \
             use sys.set_int_max_str_digits() to increase the limit",
            d.len()
        )));
    }
    let canonical = if negative { format!("-{d}") } else { d.to_string() };
    Ok(match canonical.parse::<i64>() {
        Ok(n) => V::Int(n),
        Err(_) => V::Big(Rc::from(canonical)),
    })
}

/// 放不进 i64 的 0x / 0o / 0b 整数的写法：十进制不超过 4300 位的是十进制数字，再大的是 `hex()` 的写法
fn big_from_radix(digits: &str, radix: u32) -> String {
    let bits_per_digit = radix.trailing_zeros() as usize;
    let digits = digits.trim_start_matches('0');
    // 10 ** 4300 不到 2 ** 14285：位数比这多的十进制一定超过 4300 位，不用算
    if digits.len() * bits_per_digit <= 14285 + bits_per_digit {
        // 每 9 位十进制一段（低位在前）：乘上进制、加上这一位
        let mut limbs: Vec<u32> = vec![0];
        for c in digits.chars() {
            let mut carry = c.to_digit(radix).expect("照 RE_NUMBER 认出来的数字") as u64;
            for limb in limbs.iter_mut() {
                let x = *limb as u64 * radix as u64 + carry;
                *limb = (x % 1_000_000_000) as u32;
                carry = x / 1_000_000_000;
            }
            if carry > 0 {
                limbs.push(carry as u32);
            }
        }
        let mut out = limbs.last().expect("至少一段").to_string();
        for limb in limbs.iter().rev().skip(1) {
            out.push_str(&format!("{limb:09}"));
        }
        if out.len() <= INT_MAX_STR_DIGITS {
            return out;
        }
    }
    // 换成十六进制：先排成二进制的位，再从低位起每 4 位一组
    let mut bits: Vec<u8> = Vec::with_capacity(digits.len() * bits_per_digit);
    for c in digits.chars() {
        let d = c.to_digit(radix).expect("照 RE_NUMBER 认出来的数字");
        bits.extend((0..bits_per_digit).rev().map(|i| ((d >> i) & 1) as u8));
    }
    let first = bits.iter().position(|b| *b == 1).unwrap_or(bits.len());
    let bits = &bits[first..];
    let mut out = String::from("0x");
    let lead = bits.len() % 4;
    let groups = (if lead > 0 { vec![&bits[..lead]] } else { Vec::new() }).into_iter().chain(bits[lead..].chunks(4));
    for g in groups {
        let d = g.iter().fold(0u32, |acc, b| acc * 2 + *b as u32);
        out.push(char::from_digit(d, 16).expect("四位以内"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::py::json;

    /// 对照数据里日期、时间和太大的整数的记法（生成对照数据时定的）
    fn marked(t: &Temporal) -> V {
        let kind = match (t.date.is_some(), t.time.is_some()) {
            (true, true) => "datetime",
            (true, false) => "date",
            _ => "time",
        };
        V::str(format!("@{} {}", kind, t.isoformat()))
    }

    fn plain(v: &V) -> V {
        match v {
            V::Dict(d) => V::Dict(Dict::from_pairs(d.items().into_iter().map(|(k, x)| (k, plain(&x))))),
            V::List(l) => V::List(List::from_vec(l.items().iter().map(plain).collect())),
            V::Big(s) if s.starts_with("0x") => V::str(format!("@int {s}")),
            x => x.clone(),
        }
    }

    // 这些测试就在测试线程默认的 2MB 栈上跑，里面有套到上限的文件（一千层调用）：解析器哪天占的栈多到放不下，这里会先崩

    #[test]
    fn matches_cpython_vectors() {
        let doc = json::loads(include_str!("../../tests/vectors/toml.json")).unwrap();
        let cases = doc.as_dict().unwrap().dict("cases").unwrap();
        let frame = cases.at("frame").as_int().unwrap() as usize;
        let docs = cases.list("docs").unwrap().items();
        assert!(docs.len() > 500);
        let (mut ok, mut bad) = (0, 0);
        for c in docs {
            let c = c.as_dict().unwrap();
            let input = c.str("in").unwrap();
            let got = loads_with(&input, frame, &marked);
            match (got, c.str("ok"), c.str("err")) {
                (Ok(d), Some(want), _) => {
                    assert_eq!(json::dumps(&plain(&V::Dict(d)), json::LINE), &*want, "{input:?}");
                    ok += 1;
                }
                (Err(e), None, Some(class)) => {
                    let (got_class, msg) = match &e {
                        TomlError::Decode(m) => ("TOMLDecodeError", Some(m)),
                        TomlError::Value(m) => ("ValueError", Some(m)),
                        TomlError::Recursion => ("RecursionError", None),
                    };
                    assert_eq!(got_class, &*class, "{input:?}：{e}");
                    if let Some(m) = msg {
                        assert_eq!(m.as_str(), &*c.str("msg").unwrap(), "{input:?}");
                    }
                    bad += 1;
                }
                (got, want, err) => panic!("{input:?}：Rust 给的是 {got:?}，Python 给的是 {want:?} {err:?}"),
            }
        }
        assert!(ok > 200 && bad > 200, "{ok} {bad}");
    }

    #[test]
    fn nesting_limit_is_where_cpython_hits_it() {
        let doc = json::loads(include_str!("../../tests/vectors/toml.json")).unwrap();
        let probes = doc.as_dict().unwrap().dict("cases").unwrap().list("frames").unwrap().items();
        assert!(probes.len() > 30);
        for p in probes {
            let p = p.as_dict().unwrap();
            let (input, want, max) = (p.str("in").unwrap(), p.str("want").unwrap(), p.at("max").as_int().unwrap() as usize);
            let class = |r: Result<Dict, TomlError>| match r {
                Ok(_) => "ok",
                Err(TomlError::Recursion) => "RecursionError",
                Err(_) => "ValueError",
            };
            // loads 在第 max 层时还是这个结果，再深一层就是 RecursionError
            assert_eq!(class(loads_at(&input, max)), &*want, "{input:?} 在第 {max} 层");
            assert_eq!(class(loads_at(&input, max + 1)), "RecursionError", "{input:?} 在第 {} 层", max + 1);
        }
    }

    #[test]
    fn dates_are_none_unless_asked() {
        let d = loads("d = 1979-05-27\nt = 07:32:00.5\ndt = 1979-05-27T07:32:00-07:30\ns = '1979-05-27'").unwrap();
        assert!(d.at("d").is_none() && d.at("t").is_none() && d.at("dt").is_none() && d.at("s").is_str());
        let seen = std::cell::RefCell::new(Vec::new());
        loads_with("a = [1979-05-27, 07:32:00.5, 1979-05-27 07:32:00Z, 1979-05-27t07:32:00.000001-00:30]", 2, &|t| {
            seen.borrow_mut().push(*t);
            V::None
        })
        .unwrap();
        let seen = seen.into_inner();
        assert_eq!(seen[0], Temporal { date: Some((1979, 5, 27)), time: None, offset: None });
        assert_eq!(seen[1], Temporal { date: None, time: Some((7, 32, 0, 500000)), offset: None });
        assert_eq!(seen[2].offset, Some(0));
        assert_eq!(
            seen.iter().map(Temporal::isoformat).collect::<Vec<_>>(),
            ["1979-05-27", "07:32:00.500000", "1979-05-27T07:32:00+00:00", "1979-05-27T07:32:00.000001-00:30"]
        );
    }

    #[test]
    fn errors_map_to_python_exception_classes() {
        let decode = Fail::from(loads("a = ").unwrap_err());
        assert!(decode.is_value() && decode.to_string() == "Invalid value (at end of document)");
        assert_eq!(loads("\u{feff}a = 1").unwrap_err(), TomlError::Decode("Invalid statement (at line 1, column 1)".into()));
        let recursion = Fail::from(loads(&format!("a = {}", "[".repeat(600))).unwrap_err());
        assert!(recursion.is("RecursionError") && !recursion.is_value());
    }

    #[test]
    fn deep_tables_are_freed_without_recursion() {
        // 三十万层的表：直接放手的话 Rust 一层层递归着释放，栈放不下
        let root = Dict::new();
        let mut cur = root.clone();
        for _ in 0..300_000 {
            let next = Dict::new();
            cur.set("a", List::from_vec(vec![V::Dict(next.clone())]));
            cur = next;
        }
        drop(cur);
        dispose(root);
        // 带点的键、表头写得很长时值就是这样套出来的（解析本身不递归）；解析到一半出错时建了一半的也经 dispose 拆
        let key = ["a"; 400].join(".");
        assert_eq!(loads(&format!("{key} = 1\n[b.{key}]\nc = 2")).unwrap().len(), 2);
        assert!(loads(&format!("[{key}]\n{key} = 1\n{key} = 2")).is_err());
    }

    #[test]
    fn big_integers() {
        let d = loads("a = 0xFFFF_FFFF_FFFF_FFFF\nb = 0o1_777777777777777777777\nc = -9223372036854775809\nd = 0x7FFFFFFFFFFFFFFF")
            .unwrap();
        assert_eq!(
            V::Dict(d).py_repr(),
            "{'a': 18446744073709551615, 'b': 18446744073709551615, 'c': -9223372036854775809, 'd': 9223372036854775807}"
        );
        // 十进制超过 4300 位：留成 hex() 的写法，不去算十进制
        let huge = loads(&format!("a = 0b{}\nb = 0o17{}", "1".repeat(20000), "0".repeat(5000))).unwrap();
        assert_eq!(huge.at("a").py_repr(), format!("0x{}", "f".repeat(5000)));
        assert_eq!(huge.at("b").py_repr(), format!("0xf{}", "0".repeat(3750)));
    }
}
