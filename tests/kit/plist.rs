//! 属性列表的读和写：假的 launchctl 读 cca 写的 launchd 配置，程序坞的假配置（cca 写的是二进制的，测试自己写 XML 的）。
//! 照 Python 的 plistlib：读的时候 XML 和二进制（bplist00）都认（plistlib.load），写的是 XML，
//! 逐字节和 plistlib.dump 一样（字典的键排好序、制表符缩进、文件头那两行）。原来这些假命令是用 plistlib 写的，
//! cca 和测试会接着读它们写出来的文件。

use std::fmt;

/// 属性列表里的一个值
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Str(String),
    Int(i128),
    Real(f64),
    Bool(bool),
    Data(Vec<u8>),
    Date(Date),
    Array(Vec<Value>),
    /// 字典：按文件里的先后（写的时候照 plistlib 排序）；同一个键只留一项
    Dict(Vec<(String, Value)>),
    /// 二进制属性列表里的 UID（NSKeyedArchiver 用的）：读得出，写不成 XML（plistlib 也一样）
    Uid(u64),
}

/// 日期：UTC 的年月日时分秒。plistlib 写的时候不带小数秒，读二进制的日期时这里也只留到秒（往下取整）
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Date {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// 读不出来、写不出去：为什么
#[derive(Clone, Debug, PartialEq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}

fn err<T>(msg: impl Into<String>) -> Result<T, Error> {
    Err(Error(msg.into()))
}

impl Value {
    /// 字典里的一项；不是字典、没有这个键都是 None
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Dict(d) => d.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
    /// 给字典放一项：键已经在的换掉值、位置不动（和 Python 的 dict 一样）；不是字典什么都不做
    pub fn set(&mut self, key: &str, value: Value) {
        if let Value::Dict(d) = self {
            dict_set(d, key.to_string(), value);
        }
    }
}

fn dict_set(d: &mut Vec<(String, Value)>, key: String, value: Value) {
    match d.iter_mut().find(|(k, _)| *k == key) {
        Some(slot) => slot.1 = value,
        None => d.push((key, value)),
    }
}

// ---- 读 ----

/// plistlib.load：看开头认是 XML 还是二进制
pub fn load(data: &[u8]) -> Result<Value, Error> {
    let head = &data[..data.len().min(32)];
    let body = head.strip_prefix(b"\xef\xbb\xbf".as_slice()).unwrap_or(head);
    if body.starts_with(b"<?xml") || body.starts_with(b"<plist") {
        loads_xml(data)
    } else if data.starts_with(b"bplist00") {
        loads_binary(data)
    } else {
        err("Invalid file")
    }
}

/// 读一个文件（读不了是 io 的错，读出来不是属性列表是 Error）
pub fn load_file(path: &std::path::Path) -> std::io::Result<Result<Value, Error>> {
    Ok(load(&std::fs::read(path)?))
}

/// XML 里的一个记号
enum Tok {
    Start(String),
    End(String),
    Text(String),
}

/// XML 的属性列表：和 plistlib 的 _PlistParser 一样只看认得的元素（plist 这一层、不认得的元素都跳过），每个元素开头清掉攒下的文字
pub fn loads_xml(data: &[u8]) -> Result<Value, Error> {
    let data = data.strip_prefix(b"\xef\xbb\xbf".as_slice()).unwrap_or(data);
    let text = std::str::from_utf8(data).map_err(|_| Error("not well-formed (invalid token): 不是 UTF-8".into()))?;
    let toks = tokenize(text)?;

    // 正在拼的字典、数组，和它在上一层里的键（上一层是字典时）
    enum Frame {
        Dict(Vec<(String, Value)>, Option<String>),
        Array(Vec<Value>, Option<String>),
    }
    let mut stack: Vec<Frame> = Vec::new();
    let mut key: Option<String> = None;
    let mut root: Option<Value> = None;
    let mut data = String::new();

    // add_object：有键时放进上一层的字典，没有上一层时是最外面的值，否则接在上一层的数组后面
    fn place(
        stack: &mut [Frame],
        key: &mut Option<String>,
        root: &mut Option<Value>,
        value: Value,
        own_key: Option<String>,
    ) -> Result<(), Error> {
        if let Some(k) = own_key {
            match stack.last_mut() {
                Some(Frame::Dict(d, _)) => dict_set(d, k, value),
                _ => return err("unexpected element"),
            }
            return Ok(());
        }
        match stack.last_mut() {
            None => *root = Some(value),
            Some(Frame::Array(a, _)) => a.push(value),
            Some(Frame::Dict(..)) => {
                let _ = key;
                return err("unexpected element");
            }
        }
        Ok(())
    }
    // 开始一个字典、数组时先占住位置（plistlib 在开头就 add_object）：记下用的是哪个键
    fn claim(stack: &[Frame], key: &mut Option<String>) -> Result<Option<String>, Error> {
        match (key.take(), stack.last()) {
            (Some(k), Some(Frame::Dict(..))) => Ok(Some(k)),
            (Some(_), _) => err("unexpected element"),
            (None, Some(Frame::Dict(..))) => err("unexpected element"),
            (None, _) => Ok(None),
        }
    }

    for tok in toks {
        match tok {
            Tok::Text(t) => data.push_str(&t),
            Tok::Start(name) => {
                data.clear();
                match name.as_str() {
                    "dict" => {
                        let k = claim(&stack, &mut key)?;
                        stack.push(Frame::Dict(Vec::new(), k));
                    }
                    "array" => {
                        let k = claim(&stack, &mut key)?;
                        stack.push(Frame::Array(Vec::new(), k));
                    }
                    _ => {}
                }
            }
            Tok::End(name) => {
                let raw = std::mem::take(&mut data);
                let value = match name.as_str() {
                    "dict" => {
                        if key.as_ref().is_some_and(|k| !k.is_empty()) {
                            return err(format!("missing value for key '{}'", key.unwrap_or_default()));
                        }
                        let Some(Frame::Dict(d, k)) = stack.pop() else { return err("unexpected end of dict") };
                        place(&mut stack, &mut key, &mut root, Value::Dict(d), k)?;
                        continue;
                    }
                    "array" => {
                        let Some(Frame::Array(a, k)) = stack.pop() else { return err("unexpected end of array") };
                        place(&mut stack, &mut key, &mut root, Value::Array(a), k)?;
                        continue;
                    }
                    "key" => {
                        if key.as_ref().is_some_and(|k| !k.is_empty()) || !matches!(stack.last(), Some(Frame::Dict(..))) {
                            return err("unexpected key");
                        }
                        key = Some(raw);
                        continue;
                    }
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    "integer" => Value::Int(parse_int(&raw)?),
                    "real" => Value::Real(parse_real(&raw)?),
                    "string" => Value::Str(raw),
                    "data" => Value::Data(base64_decode(&raw)),
                    "date" => Value::Date(parse_date(&raw)?),
                    _ => continue,
                };
                let k = claim(&stack, &mut key)?;
                place(&mut stack, &mut key, &mut root, value, k)?;
            }
        }
    }
    root.ok_or_else(|| Error("no element found".into()))
}

/// 拆成开始、结束、文字三种记号：跳过声明、注释、DOCTYPE，实体换成字符，CDATA 原样，换行统一成 \n（和 expat 一样）
fn tokenize(s: &str) -> Result<Vec<Tok>, Error> {
    let mut out = Vec::new();
    let mut i = 0;
    let b = s.as_bytes();
    while i < b.len() {
        if b[i] != b'<' {
            let end = s[i..].find('<').map_or(b.len(), |j| i + j);
            out.push(Tok::Text(unescape(&s[i..end])?));
            i = end;
            continue;
        }
        let rest = &s[i..];
        if let Some(r) = rest.strip_prefix("<?") {
            i += 2 + r.find("?>").ok_or_else(|| Error("unclosed token".into()))? + 2;
        } else if let Some(r) = rest.strip_prefix("<!--") {
            i += 4 + r.find("-->").ok_or_else(|| Error("unclosed token".into()))? + 3;
        } else if let Some(r) = rest.strip_prefix("<![CDATA[") {
            let n = r.find("]]>").ok_or_else(|| Error("unclosed CDATA section".into()))?;
            out.push(Tok::Text(newlines(&r[..n])));
            i += 9 + n + 3;
        } else if rest.starts_with("<!") {
            // DOCTYPE：跳到配对的 >（里面可以有 [ … ] 的内部子集）；plistlib 不收实体声明
            let mut depth = 0i32;
            let mut j = i + 2;
            loop {
                match b.get(j) {
                    None => return err("unclosed token"),
                    Some(b'[') => depth += 1,
                    Some(b']') => depth -= 1,
                    Some(b'>') if depth <= 0 => break,
                    _ => {}
                }
                j += 1;
            }
            if s[i..j].contains("<!ENTITY") {
                return err("XML entity declarations are not supported in plist files");
            }
            i = j + 1;
        } else {
            // 元素：<名字 属性…> 、</名字>、<名字/>（属性里引号中的 > 不算结尾）
            let mut j = i + 1;
            let mut quote: Option<u8> = None;
            while j < b.len() {
                match (quote, b[j]) {
                    (Some(q), c) if c == q => quote = None,
                    (Some(_), _) => {}
                    (None, b'"' | b'\'') => quote = Some(b[j]),
                    (None, b'>') => break,
                    _ => {}
                }
                j += 1;
            }
            if j >= b.len() {
                return err("unclosed token");
            }
            let inner = &s[i + 1..j];
            if let Some(name) = inner.strip_prefix('/') {
                out.push(Tok::End(name.trim().to_string()));
            } else {
                let empty = inner.ends_with('/');
                let inner = inner.strip_suffix('/').unwrap_or(inner);
                let name: String = inner.chars().take_while(|c| !c.is_whitespace()).collect();
                if name.is_empty() {
                    return err("not well-formed (invalid token)");
                }
                out.push(Tok::Start(name.clone()));
                if empty {
                    out.push(Tok::End(name));
                }
            }
            i = j + 1;
        }
    }
    Ok(out)
}

fn newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// 文字里的实体：五个预定义的和数字的
fn unescape(s: &str) -> Result<String, Error> {
    let s = newlines(s);
    if !s.contains('&') {
        return Ok(s);
    }
    let mut out = String::new();
    let mut rest = s.as_str();
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        let after = &rest[p + 1..];
        let semi = after.find(';').ok_or_else(|| Error("not well-formed (invalid token)".into()))?;
        let name = &after[..semi];
        let c = match name {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let n = if let Some(h) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
                    u32::from_str_radix(h, 16).ok()
                } else if let Some(d) = name.strip_prefix('#') {
                    d.parse::<u32>().ok()
                } else {
                    return err(format!("undefined entity &{name};"));
                };
                n.and_then(char::from_u32).ok_or_else(|| Error("reference to invalid character number".into()))?
            }
        };
        out.push(c);
        rest = &after[semi + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// plistlib 的 end_integer：0x 开头按十六进制，别的照 int()（前后可以有空白）
fn parse_int(raw: &str) -> Result<i128, Error> {
    let bad = || Error(format!("invalid literal for int() with base 10: {raw:?}"));
    if let Some(h) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        return i128::from_str_radix(h.trim(), 16).map_err(|_| bad());
    }
    let t = raw.trim();
    let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return Err(bad());
    }
    t.parse::<i128>().map_err(|_| bad())
}

/// float()：前后可以有空白，认 inf、nan
fn parse_real(raw: &str) -> Result<f64, Error> {
    let t = raw.trim();
    let lower = t.to_ascii_lowercase();
    let body = lower.strip_prefix(['+', '-']).unwrap_or(&lower);
    let neg = lower.starts_with('-');
    match body {
        "inf" | "infinity" => return Ok(if neg { f64::NEG_INFINITY } else { f64::INFINITY }),
        "nan" => return Ok(f64::NAN),
        _ => {}
    }
    t.parse::<f64>().map_err(|_| Error(format!("could not convert string to float: {raw:?}")))
}

/// plistlib 的 _date_from_string：YYYY[-MM[-DD[THH[:MM[:SS]]]]]Z
fn parse_date(raw: &str) -> Result<Date, Error> {
    let bad = || Error(format!("bad date: {raw:?}"));
    let b = raw.as_bytes();
    let num = |from: usize, n: usize| -> Option<u32> {
        let part = b.get(from..from + n)?;
        part.iter().all(u8::is_ascii_digit).then(|| part.iter().fold(0u32, |a, c| a * 10 + u32::from(c - b'0')))
    };
    let year = num(0, 4).ok_or_else(bad)?;
    let mut d = Date { year: i64::from(year), month: 1, day: 1, hour: 0, minute: 0, second: 0 };
    let mut i = 4;
    for (sep, slot) in [(b'-', 0), (b'-', 1), (b'T', 2), (b':', 3), (b':', 4)] {
        if b.get(i) != Some(&sep) {
            break;
        }
        let Some(v) = num(i + 1, 2) else { break };
        match slot {
            0 => d.month = v,
            1 => d.day = v,
            2 => d.hour = v,
            3 => d.minute = v,
            _ => d.second = v,
        }
        i += 3;
    }
    if b.get(i) != Some(&b'Z') {
        return Err(bad());
    }
    Ok(d)
}

/// binascii.a2b_base64（不严格的那种：字母表以外的字符跳过，遇到补齐的 = 就停）
fn base64_decode(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut acc, mut n) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' if n >= 2 => break,
            _ => continue,
        };
        acc = (acc << 6) | u32::from(v);
        n += 1;
        if n == 4 {
            out.extend_from_slice(&[(acc >> 16) as u8, (acc >> 8) as u8, acc as u8]);
            acc = 0;
            n = 0;
        }
    }
    match n {
        2 => out.push((acc >> 4) as u8),
        3 => out.extend_from_slice(&[(acc >> 10) as u8, (acc >> 2) as u8]),
        _ => {}
    }
    out
}

/// 二进制属性列表（bplist00），照 plistlib 的 _BinaryPlistParser
pub fn loads_binary(data: &[u8]) -> Result<Value, Error> {
    let bad = || Error("Invalid file".into());
    if data.len() < 8 + 32 || !data.starts_with(b"bplist00") {
        return Err(bad());
    }
    let tr = &data[data.len() - 32..];
    let offset_size = usize::from(tr[6]);
    let ref_size = usize::from(tr[7]);
    let be = |b: &[u8]| b.iter().fold(0u128, |a, x| (a << 8) | u128::from(*x));
    let num_objects = usize::try_from(be(&tr[8..16])).map_err(|_| bad())?;
    let top = usize::try_from(be(&tr[16..24])).map_err(|_| bad())?;
    let table = usize::try_from(be(&tr[24..32])).map_err(|_| bad())?;
    if offset_size == 0 || ref_size == 0 || !matches!(ref_size, 1 | 2 | 4 | 8) {
        return Err(bad());
    }
    let mut offsets = Vec::with_capacity(num_objects.min(1 << 20));
    for k in 0..num_objects {
        let at = table.checked_add(k * offset_size).ok_or_else(bad)?;
        let b = data.get(at..at + offset_size).ok_or_else(bad)?;
        offsets.push(usize::try_from(be(b)).map_err(|_| bad())?);
    }
    let p = Bin { data, offsets, ref_size };
    p.object(top, 0)
}

struct Bin<'a> {
    data: &'a [u8],
    offsets: Vec<usize>,
    ref_size: usize,
}

impl Bin<'_> {
    fn bytes(&self, at: usize, n: usize) -> Result<&[u8], Error> {
        self.data
            .get(at..at.checked_add(n).ok_or_else(|| Error("Invalid file".into()))?)
            .ok_or_else(|| Error("Invalid file".into()))
    }
    /// 长度：低 4 位，是 0xF 时后面跟着一个整数
    fn size(&self, low: u8, at: &mut usize) -> Result<usize, Error> {
        if low != 0xF {
            return Ok(usize::from(low));
        }
        let m = self.bytes(*at, 1)?[0] & 0x3;
        let n = 1usize << m;
        let b = self.bytes(*at + 1, n)?;
        *at += 1 + n;
        usize::try_from(b.iter().fold(0u128, |a, x| (a << 8) | u128::from(*x))).map_err(|_| Error("Invalid file".into()))
    }
    fn refs(&self, at: &mut usize, n: usize) -> Result<Vec<usize>, Error> {
        let b = self.bytes(*at, n.checked_mul(self.ref_size).ok_or_else(|| Error("Invalid file".into()))?)?;
        *at += n * self.ref_size;
        Ok(b.chunks(self.ref_size).map(|c| c.iter().fold(0usize, |a, x| (a << 8) | usize::from(*x))).collect())
    }
    fn object(&self, r: usize, depth: usize) -> Result<Value, Error> {
        if depth > 512 {
            return err("Invalid file");
        }
        let mut at = *self.offsets.get(r).ok_or_else(|| Error("Invalid file".into()))?;
        let token = self.bytes(at, 1)?[0];
        at += 1;
        let (high, low) = (token & 0xF0, token & 0x0F);
        Ok(match token {
            0x08 => Value::Bool(false),
            0x09 => Value::Bool(true),
            0x00 | 0x0F => return err("unsupported plist object (null / fill)"),
            0x22 => {
                let b = self.bytes(at, 4)?;
                Value::Real(f64::from(f32::from_be_bytes([b[0], b[1], b[2], b[3]])))
            }
            0x23 => {
                let b = self.bytes(at, 8)?;
                Value::Real(f64::from_be_bytes(b.try_into().map_err(|_| Error("Invalid file".into()))?))
            }
            0x33 => {
                let b = self.bytes(at, 8)?;
                let f = f64::from_be_bytes(b.try_into().map_err(|_| Error("Invalid file".into()))?);
                Value::Date(date_from_2001(f)?)
            }
            _ => match high {
                0x10 => {
                    let n = 1usize << low;
                    let b = self.bytes(at, n)?;
                    if n > 16 {
                        return err("Invalid file");
                    }
                    let u = b.iter().fold(0u128, |a, x| (a << 8) | u128::from(*x));
                    // 8 字节以上的按有符号读（16 字节的高 8 字节一般是 0）
                    Value::Int(if low >= 3 && n < 16 && b[0] & 0x80 != 0 { u as i128 - (1i128 << (8 * n)) } else { u as i128 })
                }
                0x40 => {
                    let n = self.size(low, &mut at)?;
                    Value::Data(self.bytes(at, n)?.to_vec())
                }
                0x50 => {
                    let n = self.size(low, &mut at)?;
                    let b = self.bytes(at, n)?;
                    if !b.is_ascii() {
                        return err("'ascii' codec can't decode");
                    }
                    Value::Str(String::from_utf8_lossy(b).into_owned())
                }
                0x60 => {
                    let n = self.size(low, &mut at)?;
                    let b = self.bytes(at, n * 2)?;
                    let units: Vec<u16> = b.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                    Value::Str(String::from_utf16(&units).map_err(|_| Error("'utf-16-be' codec can't decode".into()))?)
                }
                0x80 => {
                    let b = self.bytes(at, 1 + usize::from(low))?;
                    Value::Uid(b.iter().fold(0u64, |a, x| (a << 8) | u64::from(*x)))
                }
                0xA0 => {
                    let n = self.size(low, &mut at)?;
                    let refs = self.refs(&mut at, n)?;
                    Value::Array(refs.into_iter().map(|x| self.object(x, depth + 1)).collect::<Result<_, _>>()?)
                }
                0xD0 => {
                    let n = self.size(low, &mut at)?;
                    let keys = self.refs(&mut at, n)?;
                    let vals = self.refs(&mut at, n)?;
                    let mut d = Vec::new();
                    for (k, v) in keys.into_iter().zip(vals) {
                        let Value::Str(k) = self.object(k, depth + 1)? else { return err("dict keys must be strings") };
                        dict_set(&mut d, k, self.object(v, depth + 1)?);
                    }
                    Value::Dict(d)
                }
                _ => return err("Invalid file"),
            },
        })
    }
}

/// 2001-01-01 以来的秒数 → 年月日时分秒（往下取整到秒）
fn date_from_2001(f: f64) -> Result<Date, Error> {
    if !f.is_finite() || f.abs() > 1e15 {
        return err("date value out of range");
    }
    let secs = f.floor() as i64 + 978_307_200; // 1970 到 2001 的秒数
    Ok(date_from_unix(secs))
}

/// 1970 以来的秒数 → UTC 的年月日时分秒（Howard Hinnant 的 civil_from_days）
pub fn date_from_unix(secs: i64) -> Date {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    Date { year, month, day, hour: (rem / 3600) as u32, minute: (rem / 60 % 60) as u32, second: (rem % 60) as u32 }
}

// ---- 写 ----

const HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n";

/// plistlib.dumps(value)（XML、sort_keys=True）
pub fn dumps_xml(v: &Value) -> Result<Vec<u8>, Error> {
    let mut w = Writer { out: String::from(HEADER), level: 0 };
    w.line("<plist version=\"1.0\">");
    w.value(v)?;
    w.line("</plist>");
    Ok(w.out.into_bytes())
}

struct Writer {
    out: String,
    level: usize,
}

impl Writer {
    fn line(&mut self, s: &str) {
        if !s.is_empty() {
            for _ in 0..self.level {
                self.out.push('\t');
            }
            self.out.push_str(s);
        }
        self.out.push('\n');
    }
    fn simple(&mut self, el: &str, value: Option<&str>) -> Result<(), Error> {
        match value {
            Some(v) => {
                let v = escape(v)?;
                self.line(&format!("<{el}>{v}</{el}>"));
            }
            None => self.line(&format!("<{el}/>")),
        }
        Ok(())
    }
    fn begin(&mut self, el: &str) {
        self.line(&format!("<{el}>"));
        self.level += 1;
    }
    fn end(&mut self, el: &str) {
        self.level -= 1;
        self.line(&format!("</{el}>"));
    }
    fn value(&mut self, v: &Value) -> Result<(), Error> {
        match v {
            Value::Str(s) => self.simple("string", Some(s))?,
            Value::Bool(true) => self.simple("true", None)?,
            Value::Bool(false) => self.simple("false", None)?,
            Value::Int(n) => {
                if !(-(1i128 << 63)..(1i128 << 64)).contains(n) {
                    return err(format!("OverflowError: {n}"));
                }
                self.simple("integer", Some(&n.to_string()))?;
            }
            Value::Real(f) => self.simple("real", Some(&crate::json::float_repr(*f)))?,
            Value::Dict(d) if d.is_empty() => self.simple("dict", None)?,
            Value::Dict(d) => {
                self.begin("dict");
                let mut items: Vec<&(String, Value)> = d.iter().collect();
                items.sort_by(|a, b| a.0.cmp(&b.0));
                for (k, x) in items {
                    self.simple("key", Some(k))?;
                    self.value(x)?;
                }
                self.end("dict");
            }
            Value::Data(b) => {
                self.begin("data");
                self.level -= 1;
                let width = 76usize.saturating_sub(8 * self.level).max(16);
                for chunk in b.chunks(width / 4 * 3) {
                    self.line(&base64_encode(chunk));
                }
                self.level += 1;
                self.end("data");
            }
            Value::Date(d) => {
                let s = format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", d.year, d.month, d.day, d.hour, d.minute, d.second);
                self.simple("date", Some(&s))?;
            }
            Value::Array(a) if a.is_empty() => self.simple("array", None)?,
            Value::Array(a) => {
                self.begin("array");
                for x in a {
                    self.value(x)?;
                }
                self.end("array");
            }
            Value::Uid(_) => return err("unsupported type: <class 'plistlib.UID'>"),
        }
        Ok(())
    }
}

/// plistlib 的 _escape：控制字符（除了制表符、换行、回车）不收；回车换成换行；& < > 转义
fn escape(s: &str) -> Result<String, Error> {
    if s.chars().any(|c| (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r')) {
        return err("strings can't contain control characters; use bytes instead");
    }
    Ok(s.replace("\r\n", "\n").replace('\r', "\n").replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;"))
}

fn base64_encode(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        s.push(A[(n >> 18) as usize & 63] as char);
        s.push(A[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(pairs: &[(&str, Value)]) -> Value {
        Value::Dict(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
    }

    /// plistlib.dumps 写出来的（Python 3.14 算的）
    #[test]
    fn writes_like_plistlib() {
        let v = d(&[
            (
                "recent-apps",
                Value::Array(vec![d(&[
                    ("tile-type", Value::Str("file-tile".into())),
                    ("tile-data", d(&[("file-label", Value::Str("A&<b>".into())), ("n", Value::Int(15))])),
                ])]),
            ),
            ("persistent-apps", Value::Array(vec![])),
            ("show-recents", Value::Bool(false)),
            ("e", d(&[])),
            ("r", Value::Real(0.5)),
            ("x", Value::Data(b"hello world, this is a longer piece of data to wrap at the line limit!".to_vec())),
            ("t", Value::Date(Date { year: 2026, month: 11, day: 3, hour: 4, minute: 5, second: 6 })),
        ]);
        let want = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\t<key>e</key>\n\t<dict/>\n\t<key>persistent-apps</key>\n\t<array/>\n\t<key>r</key>\n\t<real>0.5</real>\n\t<key>recent-apps</key>\n\t<array>\n\t\t<dict>\n\t\t\t<key>tile-data</key>\n\t\t\t<dict>\n\t\t\t\t<key>file-label</key>\n\t\t\t\t<string>A&amp;&lt;b&gt;</string>\n\t\t\t\t<key>n</key>\n\t\t\t\t<integer>15</integer>\n\t\t\t</dict>\n\t\t\t<key>tile-type</key>\n\t\t\t<string>file-tile</string>\n\t\t</dict>\n\t</array>\n\t<key>show-recents</key>\n\t<false/>\n\t<key>t</key>\n\t<date>2026-11-03T04:05:06Z</date>\n\t<key>x</key>\n\t<data>\n\taGVsbG8gd29ybGQsIHRoaXMgaXMgYSBsb25nZXIgcGllY2Ugb2YgZGF0YSB0byB3cmFw\n\tIGF0IHRoZSBsaW5lIGxpbWl0IQ==\n\t</data>\n</dict>\n</plist>\n";
        assert_eq!(String::from_utf8(dumps_xml(&v).unwrap()).unwrap(), want);
        // 读回来是同一个（键的先后按写出来的，比的时候不看先后）
        let back = load(want.as_bytes()).unwrap();
        assert_eq!(back.get("show-recents"), Some(&Value::Bool(false)));
        assert_eq!(back.get("x"), v.get("x"));
        assert_eq!(back.get("t"), v.get("t"));
        assert_eq!(
            back.get("recent-apps").unwrap().as_array().unwrap()[0].get("tile-data").unwrap().get("file-label").unwrap().as_str(),
            Some("A&<b>")
        );
        assert!(dumps_xml(&Value::Str("a\u{1}".into())).is_err());
    }

    /// 测试里手写的一行 XML（没有 DOCTYPE）、plutil 写的、带注释和实体的都读得出
    #[test]
    fn reads_hand_written_xml() {
        let one = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>local.cca.test.a</string><key>CFBundleExecutable</key><string>A</string></dict></plist>\n";
        let v = load(one.as_bytes()).unwrap();
        assert_eq!(v.get("CFBundleIdentifier").and_then(Value::as_str), Some("local.cca.test.a"));
        let other = "<?xml version=\"1.0\"?><!-- x --><plist version=\"1.0\"><dict><key>a</key><string>x &#x41;&#66;&quot;</string><key>b</key><integer>0x10</integer><key>c</key><string/><key>a</key><true/></dict></plist>";
        let v = load(other.as_bytes()).unwrap();
        assert_eq!(v, d(&[("a", Value::Bool(true)), ("b", Value::Int(16)), ("c", Value::Str(String::new()))]));
        assert!(load(b"<plist><dict><string>x</string></dict></plist>").is_err());
        assert!(load(b"nothing").is_err());
    }

    /// plistlib.dumps(…, fmt=FMT_BINARY) 写的（Python 3.14 算的）：整数（含负数、8 字节的）、真假、UTF-16 的字符串、小数、数据、嵌套的字典
    #[test]
    fn reads_binary() {
        let hex = "62706c6973743030d801020304050607080910111213141518516151625163516451655166516e5173a60a0b0c0d0e0f100113fffffffffffffffe11012c120001117013000001000000000013ffffff0000000000096200fc20ac233ff800000000000042787908d11617516b517655706c61696e08191b1d1f212325272930323b3e434c55565b6467686b6d6f0000000000000101000000000000001900000000000000000000000000000075";
        let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
        let v = load(&bytes).unwrap();
        let ints = [1, -2, 300, 70000, 1 << 40, -(1 << 40)].map(Value::Int).to_vec();
        assert_eq!(
            v,
            d(&[
                ("a", Value::Array(ints)),
                ("b", Value::Bool(true)),
                ("c", Value::Str("\u{fc}\u{20ac}".into())),
                ("d", Value::Real(1.5)),
                ("e", Value::Data(b"xy".to_vec())),
                ("f", Value::Bool(false)),
                ("n", d(&[("k", Value::Str("v".into()))])),
                ("s", Value::Str("plain".into())),
            ])
        );
    }

    #[test]
    fn dates_from_unix() {
        assert_eq!(date_from_unix(0), Date { year: 1970, month: 1, day: 1, hour: 0, minute: 0, second: 0 });
        assert_eq!(date_from_unix(978_307_200), Date { year: 2001, month: 1, day: 1, hour: 0, minute: 0, second: 0 });
        assert_eq!(date_from_unix(1_795_499_811), Date { year: 2026, month: 11, day: 24, hour: 5, minute: 56, second: 51 });
        assert_eq!(date_from_unix(-1), Date { year: 1969, month: 12, day: 31, hour: 23, minute: 59, second: 59 });
    }

    /// 用 macOS 自带的 plutil 现做一份二进制的，读出来要和 XML 的那份一样
    #[test]
    fn round_trip_through_plutil() {
        if !std::path::Path::new("/usr/bin/plutil").exists() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("kit-plist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let x = dir.join("a.plist");
        let v = d(&[
            (
                "arr",
                Value::Array(vec![
                    Value::Int(1),
                    Value::Int(-2),
                    Value::Int(300),
                    Value::Int(70000),
                    Value::Int(1 << 40),
                    Value::Int(-(1 << 40)),
                ]),
            ),
            ("b", Value::Bool(true)),
            ("f", Value::Bool(false)),
            ("u", Value::Str("ü€ 中文".into())),
            ("r", Value::Real(1.5)),
            ("data", Value::Data(vec![0, 1, 2, 255])),
            ("date", Value::Date(Date { year: 2026, month: 11, day: 3, hour: 4, minute: 5, second: 6 })),
            ("nest", d(&[("k", Value::Str("v".into())), ("e", Value::Array(vec![]))])),
        ]);
        std::fs::write(&x, dumps_xml(&v).unwrap()).unwrap();
        let st = std::process::Command::new("/usr/bin/plutil").args(["-convert", "binary1"]).arg(&x).status().unwrap();
        assert!(st.success());
        let raw = std::fs::read(&x).unwrap();
        assert!(raw.starts_with(b"bplist00"));
        let back = load(&raw).unwrap();
        let sorted = |v: &Value| String::from_utf8(dumps_xml(v).unwrap()).unwrap();
        assert_eq!(sorted(&back), sorted(&v));
        std::fs::remove_dir_all(&dir).ok();
    }
}
