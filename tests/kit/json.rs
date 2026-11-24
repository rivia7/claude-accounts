//! JSON：对象保持键的顺序；读和写都照 Python 的 json 模块来（写出来的逐字节一样）。
//! 回归的断言、假命令、各份测试都用它：状态文件和夹具原来是 Python 写的，新旧两边要能互相读、原样写回去。
//! 对照数据在 tests/vectors/json_py.json（Python 的 json 算出来的，改了这里的写法拿它核对）。

use std::fmt;

/// 一个 JSON 值。整数和小数分开（Python 也分：1 和 1.0 写出来不一样），整数用 i128（夹具里有超过 64 位的）
#[derive(Clone, Debug)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i128),
    Float(f64),
    Str(String),
    List(Vec<Value>),
    Obj(Obj),
}

/// 对象：按放进去的先后排；给已有的键赋值不挪位置（和 Python 的 dict 一样）
#[derive(Clone, Debug, Default)]
pub struct Obj(Vec<(String, Value)>);

impl Obj {
    pub fn new() -> Obj {
        Obj(Vec::new())
    }
    pub fn from_pairs(pairs: Vec<(String, Value)>) -> Obj {
        let mut o = Obj::new();
        for (k, v) in pairs {
            o.insert(k, v);
        }
        o
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.0.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }
    /// 放进去；键已经在的话换掉值、位置不动，返回原来的值
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Value>) -> Option<Value> {
        let (key, value) = (key.into(), value.into());
        match self.get_mut(&key) {
            Some(slot) => Some(std::mem::replace(slot, value)),
            None => {
                self.0.push((key, value));
                None
            }
        }
    }
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let i = self.0.iter().position(|(k, _)| k == key)?;
        Some(self.0.remove(i).1)
    }
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(k, _)| k.as_str())
    }
    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.0.iter().map(|(_, v)| v)
    }
    pub fn into_pairs(self) -> Vec<(String, Value)> {
        self.0
    }
}

static NULL: Value = Value::Null;

impl Value {
    /// 对象里的一项；不是对象、没有这个键都是 None
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(o) => o.get(key),
            _ => None,
        }
    }
    /// 数组里的一项
    pub fn at(&self, i: usize) -> Option<&Value> {
        match self {
            Value::List(l) => l.get(i),
            _ => None,
        }
    }
    /// 一路往下取（键或者下标）；中间哪一步没有就是 Null
    pub fn path(&self, path: &[&str]) -> &Value {
        let mut v = self;
        for p in path {
            v = match v {
                Value::Obj(o) => o.get(p).unwrap_or(&NULL),
                Value::List(l) => p.parse::<usize>().ok().and_then(|i| l.get(i)).unwrap_or(&NULL),
                _ => &NULL,
            };
        }
        v
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(n) => i64::try_from(*n).ok(),
            _ => None,
        }
    }
    /// 数字（整数也算）
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(n) => Some(*n as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(l) => Some(l),
            _ => None,
        }
    }
    pub fn as_obj(&self) -> Option<&Obj> {
        match self {
            Value::Obj(o) => Some(o),
            _ => None,
        }
    }
    pub fn as_obj_mut(&mut self) -> Option<&mut Obj> {
        match self {
            Value::Obj(o) => Some(o),
            _ => None,
        }
    }
    /// 取字符串，不是字符串就 panic（测试里用：说清楚是什么）
    pub fn str(&self) -> &str {
        self.as_str().unwrap_or_else(|| panic!("不是字符串：{self}"))
    }
    pub fn i64(&self) -> i64 {
        self.as_i64().unwrap_or_else(|| panic!("不是整数：{self}"))
    }
    pub fn f64(&self) -> f64 {
        self.as_f64().unwrap_or_else(|| panic!("不是数字：{self}"))
    }
    pub fn bool(&self) -> bool {
        self.as_bool().unwrap_or_else(|| panic!("不是 true / false：{self}"))
    }
    pub fn list(&self) -> &[Value] {
        self.as_list().unwrap_or_else(|| panic!("不是数组：{self}"))
    }
    pub fn obj(&self) -> &Obj {
        self.as_obj().unwrap_or_else(|| panic!("不是对象：{self}"))
    }
    /// 紧凑、不转义非 ASCII 的一行（状态文件、线上的请求都是这个样子）
    pub fn compact(&self) -> String {
        dumps(self, &Style::compact())
    }
    /// 缩进 2 格、不转义非 ASCII（cca 写 config.json、账号表的样子）
    pub fn pretty(&self) -> String {
        dumps(self, &Style::pretty())
    }
}

/// 像 Python 的 j[key] / j[i]：没有就 panic，说清楚缺什么（测试里一个键没了应该当场失败，不是悄悄变成 null）
impl std::ops::Index<&str> for Value {
    type Output = Value;
    fn index(&self, key: &str) -> &Value {
        self.get(key).unwrap_or_else(|| panic!("没有键 {key:?}：{}", brief(self)))
    }
}
impl std::ops::Index<usize> for Value {
    type Output = Value;
    fn index(&self, i: usize) -> &Value {
        self.at(i).unwrap_or_else(|| panic!("没有第 {i} 项：{}", brief(self)))
    }
}

fn brief(v: &Value) -> String {
    let s = v.compact();
    if s.chars().count() > 300 {
        format!("{}…", s.chars().take(300).collect::<String>())
    } else {
        s
    }
}

/// 相等：整数和小数按数值比（1 == 1.0），对象不看键的顺序；true 和 1 不相等（Python 里相等：测试里类型不对应该看得出来）
impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Int(a), Value::Float(b)) | (Value::Float(b), Value::Int(a)) => int_eq_float(*a, *b),
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Obj(a), Value::Obj(b)) => a == b,
            _ => false,
        }
    }
}
impl PartialEq for Obj {
    fn eq(&self, other: &Obj) -> bool {
        self.len() == other.len() && self.iter().all(|(k, v)| other.get(k) == Some(v))
    }
}

/// 整数和小数是不是同一个数（不经 f64 转换丢精度：2**53+1 != float(2**53)）
pub fn int_eq_float(a: i128, b: f64) -> bool {
    b.is_finite() && b.fract() == 0.0 && b.abs() < 1.7e38 && (b as i128) == a
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.compact())
    }
}

macro_rules! from_int {
    ($($t:ty),*) => { $(impl From<$t> for Value { fn from(n: $t) -> Value { Value::Int(n as i128) } })* };
}
from_int!(i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, usize);
impl From<bool> for Value {
    fn from(b: bool) -> Value {
        Value::Bool(b)
    }
}
impl From<f64> for Value {
    fn from(f: f64) -> Value {
        Value::Float(f)
    }
}
impl From<&str> for Value {
    fn from(s: &str) -> Value {
        Value::Str(s.to_string())
    }
}
impl From<String> for Value {
    fn from(s: String) -> Value {
        Value::Str(s)
    }
}
impl From<&String> for Value {
    fn from(s: &String) -> Value {
        Value::Str(s.clone())
    }
}
impl From<&Value> for Value {
    fn from(v: &Value) -> Value {
        v.clone()
    }
}
impl From<Obj> for Value {
    fn from(o: Obj) -> Value {
        Value::Obj(o)
    }
}
impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(l: Vec<T>) -> Value {
        Value::List(l.into_iter().map(Into::into).collect())
    }
}
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(o: Option<T>) -> Value {
        o.map_or(Value::Null, Into::into)
    }
}

/// 拼一个对象：obj! { "键" => 值, … }（值是能转成 Value 的任何东西，None 是 null）
#[macro_export]
macro_rules! obj {
    ($($k:expr => $v:expr),* $(,)?) => {
        $crate::json::Value::Obj($crate::json::Obj::from_pairs(vec![$(($k.to_string(), $crate::json::Value::from($v))),*]))
    };
}
/// 拼一个数组：list![值, …]
#[macro_export]
macro_rules! list {
    ($($v:expr),* $(,)?) => {
        $crate::json::Value::List(vec![$($crate::json::Value::from($v)),*])
    };
}

// ---- 读 ----

/// 读不出来：第几个字符、为什么
#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    pub pos: usize,
    pub msg: String,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}（第 {} 个字符）", self.msg, self.pos)
    }
}
impl std::error::Error for Error {}

/// json.loads：NaN、Infinity、-Infinity 认；重复的键留第一次的位置、最后一次的值；值后面还有东西算错
pub fn parse(text: &str) -> Result<Value, Error> {
    let mut p = Parser { s: text.as_bytes(), i: 0, depth: 0 };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i < p.s.len() {
        return Err(p.err("值后面还有内容"));
    }
    Ok(v)
}

/// 同上，输入是字节（先按 UTF-8 解；开头的 BOM 和 Python 一样不认）
pub fn parse_bytes(bytes: &[u8]) -> Result<Value, Error> {
    let text = std::str::from_utf8(bytes).map_err(|e| Error { pos: e.valid_up_to(), msg: "不是 UTF-8".to_string() })?;
    parse(text)
}

const MAX_DEPTH: usize = 900; // Python 嵌套到约 1000 层报 RecursionError；这里留一点余量，不让栈溢出

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
}

impl Parser<'_> {
    fn err(&self, msg: &str) -> Error {
        // 位置按字符数（和 Python 报的 char 一样），不是字节
        let pos = String::from_utf8_lossy(&self.s[..self.i.min(self.s.len())]).chars().count();
        Error { pos, msg: msg.to_string() }
    }
    fn ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.s.get(self.i) {
            self.i += 1;
        }
    }
    fn eat(&mut self, word: &str) -> bool {
        if self.s[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            true
        } else {
            false
        }
    }
    fn value(&mut self) -> Result<Value, Error> {
        match self.s.get(self.i) {
            None => Err(self.err("缺一个值")),
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b'{') => self.nested(Self::object),
            Some(b'[') => self.nested(Self::array),
            Some(b'n') if self.eat("null") => Ok(Value::Null),
            Some(b't') if self.eat("true") => Ok(Value::Bool(true)),
            Some(b'f') if self.eat("false") => Ok(Value::Bool(false)),
            Some(b'N') if self.eat("NaN") => Ok(Value::Float(f64::NAN)),
            Some(b'I') if self.eat("Infinity") => Ok(Value::Float(f64::INFINITY)),
            Some(b'-') if self.eat("-Infinity") => Ok(Value::Float(f64::NEG_INFINITY)),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.err("缺一个值")),
        }
    }
    fn nested(&mut self, f: fn(&mut Self) -> Result<Value, Error>) -> Result<Value, Error> {
        if self.depth >= MAX_DEPTH {
            return Err(self.err("嵌套太深"));
        }
        self.depth += 1;
        let v = f(self);
        self.depth -= 1;
        v
    }
    fn object(&mut self) -> Result<Value, Error> {
        self.i += 1;
        let mut o = Obj::new();
        self.ws();
        if self.s.get(self.i) == Some(&b'}') {
            self.i += 1;
            return Ok(Value::Obj(o));
        }
        loop {
            self.ws();
            if self.s.get(self.i) != Some(&b'"') {
                return Err(self.err("键要用双引号括起来"));
            }
            let k = self.string()?;
            self.ws();
            if self.s.get(self.i) != Some(&b':') {
                return Err(self.err("缺 ':'"));
            }
            self.i += 1;
            self.ws();
            let v = self.value()?;
            o.insert(k, v);
            self.ws();
            match self.s.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Value::Obj(o));
                }
                _ => return Err(self.err("缺 ',' 或 '}'")),
            }
        }
    }
    fn array(&mut self) -> Result<Value, Error> {
        self.i += 1;
        let mut l = Vec::new();
        self.ws();
        if self.s.get(self.i) == Some(&b']') {
            self.i += 1;
            return Ok(Value::List(l));
        }
        loop {
            self.ws();
            l.push(self.value()?);
            self.ws();
            match self.s.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Value::List(l));
                }
                _ => return Err(self.err("缺 ',' 或 ']'")),
            }
        }
    }
    fn number(&mut self) -> Result<Value, Error> {
        let start = self.i;
        if self.s.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        match self.s.get(self.i) {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => self.digits(),
            _ => {
                self.i = start;
                return Err(self.err("缺一个值"));
            }
        }
        let mut float = false;
        if self.s.get(self.i) == Some(&b'.') && matches!(self.s.get(self.i + 1), Some(b'0'..=b'9')) {
            float = true;
            self.i += 1;
            self.digits();
        }
        if let Some(b'e' | b'E') = self.s.get(self.i) {
            let mut j = self.i + 1;
            if let Some(b'+' | b'-') = self.s.get(j) {
                j += 1;
            }
            if matches!(self.s.get(j), Some(b'0'..=b'9')) {
                float = true;
                self.i = j;
                self.digits();
            }
        }
        let text = std::str::from_utf8(&self.s[start..self.i]).unwrap_or("");
        if float {
            text.parse::<f64>().map(Value::Float).map_err(|_| self.err("数字读不出来"))
        } else {
            text.parse::<i128>().map(Value::Int).map_err(|_| self.err("整数太大"))
        }
    }
    fn digits(&mut self) {
        while let Some(b'0'..=b'9') = self.s.get(self.i) {
            self.i += 1;
        }
    }
    fn hex4(&mut self) -> Result<u32, Error> {
        let h = self
            .s
            .get(self.i..self.i + 4)
            .and_then(|h| std::str::from_utf8(h).ok())
            .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()));
        match h.and_then(|h| u32::from_str_radix(h, 16).ok()) {
            Some(n) => {
                self.i += 4;
                Ok(n)
            }
            None => Err(self.err("反斜杠 u 后面要 4 位十六进制")),
        }
    }
    fn string(&mut self) -> Result<String, Error> {
        self.i += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let Some(&b) = self.s.get(self.i) else { return Err(self.err("字符串没有结束")) };
            self.i += 1;
            match b {
                b'"' => break,
                b'\\' => {
                    let Some(&e) = self.s.get(self.i) else { return Err(self.err("字符串没有结束")) };
                    self.i += 1;
                    let c = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let mut n = self.hex4()?;
                            if (0xd800..0xdc00).contains(&n) && self.s[self.i..].starts_with(b"\\u") {
                                let save = self.i;
                                self.i += 2;
                                let lo = self.hex4()?;
                                if (0xdc00..0xe000).contains(&lo) {
                                    n = 0x10000 + ((n - 0xd800) << 10) + (lo - 0xdc00);
                                } else {
                                    self.i = save;
                                }
                            }
                            // 单独的代理项 Python 留着（str 里放得下），这里放不下：算错，夹具里没有这种
                            char::from_u32(n).ok_or_else(|| self.err("单独的代理项（U+D800 到 U+DFFF）这里不支持"))?
                        }
                        _ => {
                            self.i -= 1;
                            return Err(self.err("不认识的转义"));
                        }
                    };
                    out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
                }
                0..=0x1f => {
                    self.i -= 1;
                    return Err(self.err("字符串里有控制字符"));
                }
                _ => out.push(b),
            }
        }
        // 输入本来就是合法的 UTF-8，只在引号、反斜杠（都是 ASCII）处切开过，拼回去还是合法的
        String::from_utf8(out).map_err(|_| self.err("不是 UTF-8"))
    }
}

// ---- 写 ----

/// 怎么写：和 json.dump 的参数一一对应。Style::default() 就是 json.dumps 不带参数（一行、", " 和 ": "、非 ASCII 转义）
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    /// indent=N：一项一行，缩进 N 格
    pub indent: Option<usize>,
    /// separators=(",", ":")：一行里不留空格（和 indent 一起用时冒号后面也没有空格）
    pub compact: bool,
    /// ensure_ascii=False：非 ASCII 原样写
    pub unicode: bool,
    /// sort_keys=True
    pub sort_keys: bool,
}

impl Style {
    /// separators=(",", ":"), ensure_ascii=False：状态文件、线上的请求
    pub fn compact() -> Style {
        Style { compact: true, unicode: true, ..Style::default() }
    }
    /// indent=2, ensure_ascii=False：config.json、账号表
    pub fn pretty() -> Style {
        Style { indent: Some(2), unicode: true, ..Style::default() }
    }
    pub fn indent(mut self, n: usize) -> Style {
        self.indent = Some(n);
        self
    }
    pub fn unicode(mut self) -> Style {
        self.unicode = true;
        self
    }
    pub fn sorted(mut self) -> Style {
        self.sort_keys = true;
        self
    }
}

/// json.dumps
pub fn dumps(v: &Value, st: &Style) -> String {
    let mut out = String::new();
    write_value(&mut out, v, st, 0);
    out
}

fn newline(out: &mut String, st: &Style, level: usize) {
    if let Some(n) = st.indent {
        out.push('\n');
        for _ in 0..n * level {
            out.push(' ');
        }
    }
}

fn write_value(out: &mut String, v: &Value, st: &Style, level: usize) {
    let item_sep = if st.indent.is_some() || st.compact { "," } else { ", " };
    let key_sep = if st.compact { ":" } else { ": " };
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(n) => out.push_str(&n.to_string()),
        Value::Float(f) => out.push_str(&if f.is_nan() {
            "NaN".to_string()
        } else if f.is_infinite() {
            if *f > 0.0 { "Infinity" } else { "-Infinity" }.to_string()
        } else {
            float_repr(*f)
        }),
        Value::Str(s) => write_str(out, s, st.unicode),
        Value::List(l) if l.is_empty() => out.push_str("[]"),
        Value::List(l) => {
            out.push('[');
            for (i, x) in l.iter().enumerate() {
                if i > 0 {
                    out.push_str(item_sep);
                }
                newline(out, st, level + 1);
                write_value(out, x, st, level + 1);
            }
            newline(out, st, level);
            out.push(']');
        }
        Value::Obj(o) if o.is_empty() => out.push_str("{}"),
        Value::Obj(o) => {
            let mut items: Vec<(&str, &Value)> = o.iter().collect();
            if st.sort_keys {
                items.sort_by(|a, b| a.0.cmp(b.0)); // UTF-8 的字节序就是码点的顺序，和 Python 排 str 一样
            }
            out.push('{');
            for (i, (k, x)) in items.into_iter().enumerate() {
                if i > 0 {
                    out.push_str(item_sep);
                }
                newline(out, st, level + 1);
                write_str(out, k, st.unicode);
                out.push_str(key_sep);
                write_value(out, x, st, level + 1);
            }
            newline(out, st, level);
            out.push('}');
        }
    }
}

fn write_str(out: &mut String, s: &str, unicode: bool) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if unicode || (' '..='~').contains(&c) => out.push(c),
            c => {
                // ensure_ascii：0x7f 和非 ASCII 都写成反斜杠 u 加 4 位十六进制，基本平面以外的拆成一对代理项
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            }
        }
    }
    out.push('"');
}

/// Python 的 repr(float)：最短的、读回来还是同一个数的那几位；小于 1e-4 或者不小于 1e16 用指数写法；整数值带 ".0"
pub fn float_repr(f: f64) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf" } else { "-inf" }.to_string();
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0.0" } else { "0.0" }.to_string();
    }
    // {:e} 给出最短的那几位："d.ddde指数"
    let sci = format!("{:e}", f.abs());
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: String = mant.chars().filter(char::is_ascii_digit).collect();
    let sign = if f < 0.0 { "-" } else { "" };
    if !(-4..16).contains(&exp) {
        let frac = if digits.len() > 1 { format!(".{}", &digits[1..]) } else { String::new() };
        return format!("{sign}{}{frac}e{}{:02}", &digits[..1], if exp < 0 { '-' } else { '+' }, exp.abs());
    }
    if exp < 0 {
        return format!("{sign}0.{}{digits}", "0".repeat((-exp - 1) as usize));
    }
    let int_len = exp as usize + 1;
    if digits.len() <= int_len {
        format!("{sign}{digits}{}.0", "0".repeat(int_len - digits.len()))
    } else {
        format!("{sign}{}.{}", &digits[..int_len], &digits[int_len..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_print_like_python() {
        for (f, want) in [
            (1.0, "1.0"),
            (0.1, "0.1"),
            (-2.5, "-2.5"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e+16"),
            (1.5e16, "1.5e+16"),
            (123456789012345680.0, "1.2345678901234568e+17"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (1.5e-7, "1.5e-07"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            (1795499811.1234567, "1795499811.1234567"),
            (0.079, "0.079"),
            (-0.0, "-0.0"),
            (100.0, "100.0"),
        ] {
            assert_eq!(float_repr(f), want);
        }
    }

    #[test]
    fn objects_keep_order_and_position() {
        let v = parse(r#"{"b": 1, "a": 2, "b": 3}"#).unwrap();
        assert_eq!(v.compact(), r#"{"b":3,"a":2}"#);
        let mut o = v.obj().clone();
        o.insert("a", 9);
        o.insert("c", Value::Null);
        assert_eq!(o.keys().collect::<Vec<_>>(), ["b", "a", "c"]);
        assert_eq!(o.remove("b"), Some(Value::Int(3)));
        assert_eq!(Value::Obj(o).compact(), r#"{"a":9,"c":null}"#);
    }

    #[test]
    fn equality_is_by_value() {
        assert_eq!(parse("[1, 2.0]").unwrap(), parse("[1.0, 2]").unwrap());
        assert_eq!(parse(r#"{"a": 1, "b": 2}"#).unwrap(), parse(r#"{"b": 2, "a": 1}"#).unwrap());
        assert_ne!(Value::Bool(true), Value::Int(1));
        assert_ne!(Value::Int(9007199254740993), Value::Float(9007199254740992.0));
        assert_eq!(obj! { "a" => 1, "b" => list![true, "x", None::<i64>] }.compact(), r#"{"a":1,"b":[true,"x",null]}"#);
    }

    #[test]
    fn bad_input_is_refused() {
        for bad in [
            "",
            " ",
            "{",
            "[1,]",
            "{\"a\":1,}",
            "01",
            "1 2",
            "'a'",
            "{a:1}",
            "\"a\nb\"",
            "\"\\x\"",
            "nul",
            "[1 2]",
            "\"\\ud800\"",
            "-",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        assert!(parse(&format!("{}1{}", "[".repeat(2000), "]".repeat(2000))).is_err());
        assert_eq!(parse("1.").unwrap_err().pos, 1);
    }

    /// 对照数据：每一项是 [原文, 默认写法, indent=2, 紧凑+不转义, sort_keys+indent=1+不转义]，都是 Python 的 json 模块算的
    #[test]
    fn dumps_match_python() {
        let cases = parse(include_str!("../vectors/json_py.json")).unwrap();
        assert!(cases.list().len() > 60);
        for c in cases.list() {
            let v = parse(c[0].str()).unwrap_or_else(|e| panic!("{}: {e}", c[0].str()));
            assert_eq!(dumps(&v, &Style::default()), c[1].str(), "{}", c[0].str());
            assert_eq!(dumps(&v, &Style::default().indent(2)), c[2].str(), "{}", c[0].str());
            assert_eq!(dumps(&v, &Style::compact()), c[3].str(), "{}", c[0].str());
            assert_eq!(dumps(&v, &Style::default().indent(1).unicode().sorted()), c[4].str(), "{}", c[0].str());
            // 默认写法读回来再写一遍还是它（不直接比两个值：NaN 和自己不相等）
            assert_eq!(dumps(&parse(c[1].str()).unwrap(), &Style::default()), c[1].str(), "{}", c[0].str());
        }
    }
}
