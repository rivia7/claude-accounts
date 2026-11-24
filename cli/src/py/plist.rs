//! `plistlib`：属性列表的 XML 和二进制两种格式，读和写。照 CPython 3.14 的 plistlib 移植，输出逐字节一样
//! （tests/vectors/plist.json：当初用 CPython 生成的，已经固定）。
//!
//! 出错时的异常类和 Python 对得上（调用方靠 `except (OSError, ValueError)` 这些分情况）：
//! - InvalidFileException、ValueError、binascii.Error → `Fail::Value`（说明也一样）
//! - XML 写得不对（expat 的 ExpatError，不是 ValueError）→ `Fail::Other("ExpatError", …)`，说明只是大意
//! - TypeError / OverflowError / AttributeError / IndexError → `Fail::Other(类名, …)`
//!
//! 和 Python 不一样的地方（都是正常的属性列表里没有的）：
//! - 值是一棵各管各的树，不是互相引用的对象：二进制文件里几处引用同一个数组 / 字典时读成各自一份，再写出去也就各写一份
//!   （Python 按对象的 id 只写一份）；自己套自己的数组 / 字典读不了（Python 读得出一个环）
//! - 二进制文件里字典的键不是字符串的读不了（Python 只要能当键就行）；超过 16 字节的整数读不了
//! - XML 只认 UTF-8、带 BOM 的 UTF-16、ISO-8859-1 / US-ASCII 这几种编码

use super::base64::{a2b_base64, b64encode};
use super::fail::{Fail, R};
use super::num::float_repr;
use super::text;

/// 属性列表里的一个值
#[derive(Clone, Debug, PartialEq)]
pub enum P {
    /// Python 的 None：只有二进制格式里有（XML 写不了）
    Null,
    Bool(bool),
    /// 写得出去的范围是 -2^63 到 2^64-1
    Int(i128),
    Real(f64),
    Str(String),
    Data(Vec<u8>),
    /// 2001-01-01 00:00:00 起的微秒数（Python 的 datetime 精确到微秒，没有时区）
    Date(i64),
    Array(Vec<P>),
    /// 键的先后照读到 / 放进去的顺序；写的时候按键排序（plistlib 默认的 sort_keys=True）
    Dict(Vec<(String, P)>),
    /// NSKeyedArchiver 的 UID
    Uid(u64),
}

impl P {
    pub fn dict<K: Into<String>>(items: impl IntoIterator<Item = (K, P)>) -> P {
        let mut d = P::Dict(Vec::new());
        for (k, v) in items {
            d.set(k, v);
        }
        d
    }

    /// `d.get(k)`：不是字典、没有这个键都是 None
    pub fn get(&self, k: &str) -> Option<&P> {
        match self {
            P::Dict(items) => items.iter().find(|(key, _)| key == k).map(|(_, v)| v),
            _ => None,
        }
    }

    /// `d[k] = v`：已有的键留在原来的位置。不是字典时什么都不做
    pub fn set(&mut self, k: impl Into<String>, v: P) {
        if let P::Dict(items) = self {
            let k = k.into();
            match items.iter_mut().find(|(key, _)| *key == k) {
                Some(slot) => slot.1 = v,
                None => items.push((k, v)),
            }
        }
    }

    pub fn is_dict(&self) -> bool {
        matches!(self, P::Dict(_))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            P::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Python 里这个值算不算真（`x or 默认值`）
    pub fn truthy(&self) -> bool {
        match self {
            P::Null => false,
            P::Bool(b) => *b,
            P::Int(i) => *i != 0,
            P::Real(f) => *f != 0.0,
            P::Str(s) => !s.is_empty(),
            P::Data(d) => !d.is_empty(),
            P::Array(a) => !a.is_empty(),
            P::Dict(d) => !d.is_empty(),
            P::Date(_) | P::Uid(_) => true,
        }
    }
}

impl From<&str> for P {
    fn from(s: &str) -> P {
        P::Str(s.to_string())
    }
}
impl From<String> for P {
    fn from(s: String) -> P {
        P::Str(s)
    }
}
impl From<bool> for P {
    fn from(b: bool) -> P {
        P::Bool(b)
    }
}
impl From<i64> for P {
    fn from(i: i64) -> P {
        P::Int(i as i128)
    }
}
impl From<Vec<P>> for P {
    fn from(a: Vec<P>) -> P {
        P::Array(a)
    }
}

fn invalid<T>() -> R<T> {
    Err(Fail::Value("Invalid file".into()))
}

fn value_error<T>(msg: impl Into<String>) -> R<T> {
    Err(Fail::Value(msg.into()))
}

fn other<T>(class: &'static str, msg: impl Into<String>) -> R<T> {
    Err(Fail::Other(class, vec![msg.into()]))
}

// ---------- 日期 ----------

const MICROS_PER_DAY: i64 = 86_400_000_000;
/// 0001-01-01 到 2001-01-01 的天数
const DAYS_TO_2001: i64 = 730_485;
/// 0001-01-01 到 9999-12-31 的天数
const DAYS_TO_9999_END: i64 = 3_652_058;

fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// 0001-01-01 起的天数（公历往前推）
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y0 = y - 1;
    let mut days = y0 * 365 + y0 / 4 - y0 / 100 + y0 / 400;
    for month in 1..m {
        days += days_in_month(y, month);
    }
    days + d - 1
}

/// 0001-01-01 起的天数 → (年, 月, 日)
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let mut y = days / 366 + 1; // 不会超过真正的年份，再一年一年补上
    while days_from_civil(y + 1, 1, 1) <= days {
        y += 1;
    }
    let mut rest = days - days_from_civil(y, 1, 1);
    let mut m = 1;
    while rest >= days_in_month(y, m) {
        rest -= days_in_month(y, m);
        m += 1;
    }
    (y, m, rest + 1)
}

/// `datetime.datetime(年, 月, 日, 时, 分, 秒)` → 2001 年起的微秒数；哪一项不对报的话和 Python 一样
fn date_from_fields(f: &[i64]) -> R<i64> {
    let (y, m, d) = (f[0], f[1], f[2]);
    let (hh, mm, ss) = (f.get(3).copied().unwrap_or(0), f.get(4).copied().unwrap_or(0), f.get(5).copied().unwrap_or(0));
    if !(1..=9999).contains(&y) {
        return value_error(format!("year must be in 1..9999, not {y}"));
    }
    if !(1..=12).contains(&m) {
        return value_error(format!("month must be in 1..12, not {m}"));
    }
    let dim = days_in_month(y, m);
    if !(1..=dim).contains(&d) {
        return value_error(format!("day {d} must be in range 1..{dim} for month {m} in year {y}"));
    }
    if !(0..=23).contains(&hh) {
        return value_error(format!("hour must be in 0..23, not {hh}"));
    }
    if !(0..=59).contains(&mm) {
        return value_error(format!("minute must be in 0..59, not {mm}"));
    }
    if !(0..=59).contains(&ss) {
        return value_error(format!("second must be in 0..59, not {ss}"));
    }
    Ok((days_from_civil(y, m, d) - DAYS_TO_2001) * MICROS_PER_DAY + (hh * 3600 + mm * 60 + ss) * 1_000_000)
}

/// XML 里的日期：`YYYY-MM-DDTHH:MM:SSZ`，后面的几段可以省（和 plistlib 的 `_dateParser` 一样：从头匹配，Z 后面的不管）
fn date_from_string(s: &str) -> R<i64> {
    let b = s.as_bytes();
    let digits = |at: usize, n: usize| -> Option<i64> {
        let part = b.get(at..at + n)?;
        part.iter().all(u8::is_ascii_digit).then(|| part.iter().fold(0i64, |acc, c| acc * 10 + (c - b'0') as i64))
    };
    let no_match = || other("AttributeError", "'NoneType' object has no attribute 'groupdict'");
    let Some(year) = digits(0, 4) else { return no_match() };
    let mut fields = vec![year];
    let mut at = 4;
    for sep in *b"--T::" {
        if b.get(at) != Some(&sep) {
            break;
        }
        let Some(v) = digits(at + 1, 2) else { break };
        fields.push(v);
        at += 3;
    }
    // 可选的几段是一层套一层的：后面跟的不是 Z 时，正则会退回去少认几段再试
    loop {
        if b.get(at) == Some(&b'Z') {
            break;
        }
        if fields.len() == 1 {
            return no_match();
        }
        fields.pop();
        at -= 3;
    }
    if fields.len() < 3 {
        return other("TypeError", "function missing required argument");
    }
    date_from_fields(&fields)
}

fn date_to_string(micros: i64) -> String {
    let days = micros.div_euclid(MICROS_PER_DAY);
    let secs = micros.rem_euclid(MICROS_PER_DAY) / 1_000_000;
    let (y, m, d) = civil_from_days(days + DAYS_TO_2001);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, secs / 3600, secs / 60 % 60, secs % 60)
}

/// 二进制格式里的日期（2001 年起的秒数，小数）→ 微秒数，照 `datetime(2001, 1, 1) + timedelta(seconds=f)` 的算法取整
fn date_from_seconds(f: f64) -> R<i64> {
    if !f.is_finite() {
        return invalid();
    }
    let whole = f.trunc();
    if whole.abs() > 1e15 {
        return invalid();
    }
    let mut micros = whole as i128 * 1_000_000;
    let frac = f - whole;
    if frac != 0.0 {
        let scaled = 1_000_000.0 * frac;
        let scaled_whole = scaled.trunc();
        micros += scaled_whole as i128;
        let leftover = scaled - scaled_whole;
        if leftover != 0.0 {
            let mut rounded = leftover.round();
            if (rounded - leftover).abs() == 0.5 {
                let odd = (micros & 1) as f64;
                rounded = 2.0 * ((leftover + odd) * 0.5).round() - odd;
            }
            micros += rounded as i128;
        }
    }
    let lowest = -(DAYS_TO_2001 as i128) * MICROS_PER_DAY as i128;
    let highest = (DAYS_TO_9999_END - DAYS_TO_2001 + 1) as i128 * MICROS_PER_DAY as i128 - 1;
    if micros < lowest || micros > highest {
        return invalid();
    }
    Ok(micros as i64)
}

/// `timedelta.total_seconds()`：微秒数除以一百万，结果是最接近的那个浮点数（整数相除，只舍入一次）
fn seconds_from_micros(micros: i64) -> f64 {
    if micros == 0 {
        return 0.0;
    }
    let (neg, n) = (micros < 0, micros.unsigned_abs() as u128);
    let d = 1_000_000u128;
    let bits = |x: u128| 128 - x.leading_zeros() as i32;
    let shift = 76 - bits(n); // 商至少 55 位
    let (q, sticky) = ((n << shift) / d, (n << shift) % d != 0);
    let extra = bits(q) - 53;
    let (mut mantissa, rest, half) = (q >> extra, q & ((1u128 << extra) - 1), 1u128 << (extra - 1));
    if rest > half || (rest == half && (sticky || mantissa & 1 == 1)) {
        mantissa += 1;
    }
    let v = mantissa as f64 * 2f64.powi(extra - shift);
    if neg {
        -v
    } else {
        v
    }
}

// ---------- 写 XML ----------

const XML_HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n";

fn escape(s: &str) -> R<String> {
    if s.chars().any(|c| (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r')) {
        return value_error("strings can't contain control characters; use bytes instead");
    }
    Ok(s.replace("\r\n", "\n").replace('\r', "\n").replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;"))
}

struct XmlWriter {
    out: String,
    level: usize,
}

impl XmlWriter {
    fn line(&mut self, s: &str) {
        for _ in 0..self.level {
            self.out.push('\t');
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn simple(&mut self, element: &str, value: &str) -> R<()> {
        let line = format!("<{element}>{}</{element}>", escape(value)?);
        self.line(&line);
        Ok(())
    }

    fn value(&mut self, v: &P) -> R<()> {
        match v {
            P::Null => return other("TypeError", "unsupported type: <class 'NoneType'>"),
            P::Str(s) => self.simple("string", s)?,
            P::Bool(true) => self.line("<true/>"),
            P::Bool(false) => self.line("<false/>"),
            P::Int(i) => {
                if !(-(1i128 << 63)..(1i128 << 64)).contains(i) {
                    return other("OverflowError", i.to_string());
                }
                self.simple("integer", &i.to_string())?
            }
            P::Real(f) => self.simple("real", &float_repr(*f))?,
            P::Date(d) => self.simple("date", &date_to_string(*d))?,
            P::Uid(_) => return other("TypeError", "unsupported type: <class 'plistlib.UID'>"),
            P::Data(data) => {
                self.line("<data>");
                // 一行最多 76 列（缩进的一个 TAB 算 8 列），至少放 16 个字符
                let width = 16.max(76usize.saturating_sub(8 * self.level));
                for chunk in data.chunks(width / 4 * 3) {
                    self.line(&b64encode(chunk));
                }
                self.line("</data>");
            }
            P::Array(items) => {
                if items.is_empty() {
                    self.line("<array/>");
                } else {
                    self.line("<array>");
                    self.level += 1;
                    for item in items {
                        self.value(item)?;
                    }
                    self.level -= 1;
                    self.line("</array>");
                }
            }
            P::Dict(items) => {
                if items.is_empty() {
                    self.line("<dict/>");
                } else {
                    self.line("<dict>");
                    self.level += 1;
                    for (k, item) in sorted(items) {
                        self.simple("key", k)?;
                        self.value(item)?;
                    }
                    self.level -= 1;
                    self.line("</dict>");
                }
            }
        }
        Ok(())
    }
}

/// `sorted(d.items())`：按键的码位排
fn sorted(items: &[(String, P)]) -> Vec<&(String, P)> {
    let mut refs: Vec<&(String, P)> = items.iter().collect();
    refs.sort_by(|a, b| a.0.cmp(&b.0));
    refs
}

/// `plistlib.dumps(value)`：XML 格式
pub fn dumps_xml(v: &P) -> R<Vec<u8>> {
    let mut w = XmlWriter { out: String::from(XML_HEADER), level: 0 };
    w.line("<plist version=\"1.0\">");
    w.value(v)?;
    w.line("</plist>");
    Ok(w.out.into_bytes())
}

// ---------- 写二进制 ----------

/// 可以合并的标量：同一种、同一个值的只写一份（`(type(value), value)`）
#[derive(PartialEq)]
enum Scalar<'a> {
    Null,
    Bool(bool),
    Int(i128),
    Real(f64),
    Str(&'a str),
    Data(&'a [u8]),
    Date(i64),
}

enum Obj<'a> {
    Scalar(Scalar<'a>),
    Uid(u64),
    Array(Vec<usize>),
    Dict(Vec<usize>, Vec<usize>),
}

fn scalar(v: &P) -> Option<Scalar<'_>> {
    Some(match v {
        P::Null => Scalar::Null,
        P::Bool(b) => Scalar::Bool(*b),
        P::Int(i) => Scalar::Int(*i),
        P::Real(f) => Scalar::Real(*f),
        P::Str(s) => Scalar::Str(s),
        P::Data(d) => Scalar::Data(d),
        P::Date(d) => Scalar::Date(*d),
        P::Array(_) | P::Dict(_) | P::Uid(_) => return None,
    })
}

/// 把整棵树摊平成一张对象表，返回这个值的编号。先登记自己，再登记里面的（字典：先所有的键，再所有的值）
fn flatten<'a>(v: &'a P, objs: &mut Vec<Obj<'a>>) -> usize {
    if let Some(s) = scalar(v) {
        // 浮点数：0.0 和 -0.0 算一个，NaN 和谁都不算一个（和 Python 字典的键一样）
        if let Some(i) = objs.iter().position(|o| matches!(o, Obj::Scalar(x) if *x == s)) {
            return i;
        }
        objs.push(Obj::Scalar(s));
        return objs.len() - 1;
    }
    let me = objs.len();
    match v {
        P::Uid(u) => objs.push(Obj::Uid(*u)),
        P::Array(items) => {
            objs.push(Obj::Array(Vec::new()));
            let refs: Vec<usize> = items.iter().map(|item| flatten(item, objs)).collect();
            objs[me] = Obj::Array(refs);
        }
        P::Dict(items) => {
            objs.push(Obj::Dict(Vec::new(), Vec::new()));
            let items = sorted(items);
            let mut keys = Vec::with_capacity(items.len());
            for (k, _) in &items {
                let key = Scalar::Str(k);
                keys.push(match objs.iter().position(|o| matches!(o, Obj::Scalar(x) if *x == key)) {
                    Some(i) => i,
                    None => {
                        objs.push(Obj::Scalar(key));
                        objs.len() - 1
                    }
                });
            }
            let values: Vec<usize> = items.iter().map(|(_, item)| flatten(item, objs)).collect();
            objs[me] = Obj::Dict(keys, values);
        }
        _ => unreachable!("标量在上面处理了"),
    }
    me
}

fn count_to_size(n: usize) -> usize {
    match n {
        n if n < 1 << 8 => 1,
        n if n < 1 << 16 => 2,
        n if (n as u64) < 1 << 32 => 4,
        _ => 8,
    }
}

fn put_uint(out: &mut Vec<u8>, v: u64, size: usize) {
    out.extend_from_slice(&v.to_be_bytes()[8 - size..]);
}

fn put_size(out: &mut Vec<u8>, token: u8, size: usize) {
    if size < 15 {
        out.push(token | size as u8);
    } else {
        out.push(token | 0x0f);
        let (marker, width) = match size {
            s if s < 1 << 8 => (0x10, 1),
            s if s < 1 << 16 => (0x11, 2),
            s if (s as u64) < 1 << 32 => (0x12, 4),
            _ => (0x13, 8),
        };
        out.push(marker);
        put_uint(out, size as u64, width);
    }
}

/// `plistlib.dumps(value, fmt=plistlib.FMT_BINARY)`
pub fn dumps_binary(v: &P) -> R<Vec<u8>> {
    let mut objs = Vec::new();
    let top = flatten(v, &mut objs);
    let ref_size = count_to_size(objs.len());
    let mut out = b"bplist00".to_vec();
    let mut offsets = Vec::with_capacity(objs.len());
    let put_refs = |out: &mut Vec<u8>, refs: &[usize]| refs.iter().for_each(|r| put_uint(out, *r as u64, ref_size));
    for obj in &objs {
        offsets.push(out.len());
        match obj {
            Obj::Scalar(Scalar::Null) => out.push(0x00),
            Obj::Scalar(Scalar::Bool(false)) => out.push(0x08),
            Obj::Scalar(Scalar::Bool(true)) => out.push(0x09),
            Obj::Scalar(Scalar::Int(i)) => {
                let i = *i;
                if !(-(1i128 << 63)..1i128 << 64).contains(&i) {
                    return other("OverflowError", i.to_string());
                }
                if i < 0 {
                    out.push(0x13);
                    out.extend_from_slice(&(i as i64).to_be_bytes());
                } else if i < 1 << 8 {
                    out.push(0x10);
                    put_uint(&mut out, i as u64, 1);
                } else if i < 1 << 16 {
                    out.push(0x11);
                    put_uint(&mut out, i as u64, 2);
                } else if i < 1 << 32 {
                    out.push(0x12);
                    put_uint(&mut out, i as u64, 4);
                } else if i < 1 << 63 {
                    out.push(0x13);
                    put_uint(&mut out, i as u64, 8);
                } else {
                    out.push(0x14);
                    out.extend_from_slice(&i.to_be_bytes());
                }
            }
            Obj::Scalar(Scalar::Real(f)) => {
                out.push(0x23);
                out.extend_from_slice(&f.to_be_bytes());
            }
            Obj::Scalar(Scalar::Date(d)) => {
                out.push(0x33);
                out.extend_from_slice(&seconds_from_micros(*d).to_be_bytes());
            }
            Obj::Scalar(Scalar::Data(d)) => {
                put_size(&mut out, 0x40, d.len());
                out.extend_from_slice(d);
            }
            Obj::Scalar(Scalar::Str(s)) => {
                if s.is_ascii() {
                    put_size(&mut out, 0x50, s.len());
                    out.extend_from_slice(s.as_bytes());
                } else {
                    let units: Vec<u16> = s.encode_utf16().collect();
                    put_size(&mut out, 0x60, units.len());
                    units.iter().for_each(|u| out.extend_from_slice(&u.to_be_bytes()));
                }
            }
            Obj::Uid(u) => {
                let width = match *u {
                    u if u < 1 << 8 => 1,
                    u if u < 1 << 16 => 2,
                    u if u < 1 << 32 => 4,
                    _ => 8,
                };
                out.push(0x80 | (width as u8 - 1));
                put_uint(&mut out, *u, width);
            }
            Obj::Array(refs) => {
                put_size(&mut out, 0xa0, refs.len());
                put_refs(&mut out, refs);
            }
            Obj::Dict(keys, values) => {
                put_size(&mut out, 0xd0, keys.len());
                put_refs(&mut out, keys);
                put_refs(&mut out, values);
            }
        }
    }
    let table_at = out.len();
    let offset_size = count_to_size(table_at);
    offsets.iter().for_each(|o| put_uint(&mut out, *o as u64, offset_size));
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0, offset_size as u8, ref_size as u8]);
    put_uint(&mut out, objs.len() as u64, 8);
    put_uint(&mut out, top as u64, 8);
    put_uint(&mut out, table_at as u64, 8);
    Ok(out)
}

// ---------- 读二进制 ----------

struct BinReader<'a> {
    data: &'a [u8],
    ref_size: usize,
    offsets: Vec<u64>,
    /// 每个对象：还没读 / 正在读（读到自己就是环）/ 读好了
    objects: Vec<Slot>,
}

#[derive(Clone)]
enum Slot {
    Unread,
    Reading,
    Done(P),
}

impl<'a> BinReader<'a> {
    /// `fp.read(n)`：到头了就少给
    fn read(&self, at: &mut usize, n: usize) -> &'a [u8] {
        let start = (*at).min(self.data.len());
        let end = start.saturating_add(n).min(self.data.len());
        *at = end;
        &self.data[start..end]
    }

    /// `struct.unpack` 定长的无符号整数：不够长是错
    fn uint(&self, at: &mut usize, size: usize) -> R<u64> {
        let b = self.read(at, size);
        if b.len() != size {
            return invalid();
        }
        Ok(b.iter().fold(0u64, |acc, x| (acc << 8) | *x as u64))
    }

    fn ints(&self, at: &mut usize, n: u64, size: usize) -> R<Vec<u64>> {
        let total = (n as u128) * (size as u128);
        if size == 0 || size > 8 || total > (self.data.len() as u128) {
            return invalid();
        }
        (0..n).map(|_| self.uint(at, size)).collect()
    }

    fn size(&self, at: &mut usize, low: u8) -> R<usize> {
        if low != 0x0f {
            return Ok(low as usize);
        }
        let Some(marker) = self.read(at, 1).first() else { return invalid() };
        let n = self.uint(at, 1 << (marker & 0x03))?;
        usize::try_from(n).or_else(|_| invalid())
    }

    fn object(&mut self, r: u64, depth: usize) -> R<P> {
        let Some(slot) = usize::try_from(r).ok().filter(|i| *i < self.objects.len()) else { return invalid() };
        match &self.objects[slot] {
            Slot::Done(p) => return Ok(p.clone()),
            Slot::Reading => return invalid(),
            Slot::Unread => {}
        }
        if depth > 512 {
            return invalid();
        }
        let Ok(mut at) = usize::try_from(self.offsets[slot]) else { return invalid() };
        let Some(&token) = self.read(&mut at, 1).first() else { return invalid() };
        let (high, low) = (token & 0xf0, token & 0x0f);
        let result = match (token, high) {
            (0x00, _) => P::Null,
            (0x08, _) => P::Bool(false),
            (0x09, _) => P::Bool(true),
            (0x0f, _) => P::Data(Vec::new()),
            (_, 0x10) => {
                if low > 4 {
                    return invalid();
                }
                // int.from_bytes：到头了就拿到几个字节算几个；8 字节起是有符号的
                let b = self.read(&mut at, 1 << low);
                let unsigned = b.iter().fold(0u128, |acc, x| (acc << 8) | *x as u128);
                let negative = low >= 3 && b.first().is_some_and(|x| x & 0x80 != 0);
                P::Int(if negative && b.len() < 16 { unsigned as i128 - (1i128 << (8 * b.len())) } else { unsigned as i128 })
            }
            (0x22, _) => {
                let b = self.read(&mut at, 4);
                let Ok(raw) = <[u8; 4]>::try_from(b) else { return invalid() };
                P::Real(f32::from_be_bytes(raw) as f64)
            }
            (0x23, _) | (0x33, _) => {
                let b = self.read(&mut at, 8);
                let Ok(raw) = <[u8; 8]>::try_from(b) else { return invalid() };
                let f = f64::from_be_bytes(raw);
                if token == 0x23 {
                    P::Real(f)
                } else {
                    P::Date(date_from_seconds(f)?)
                }
            }
            (_, 0x40) => {
                let n = self.size(&mut at, low)?;
                let b = self.read(&mut at, n);
                if b.len() != n {
                    return invalid();
                }
                P::Data(b.to_vec())
            }
            (_, 0x50) => {
                let n = self.size(&mut at, low)?;
                let b = self.read(&mut at, n);
                if b.len() != n || !b.is_ascii() {
                    return invalid();
                }
                P::Str(String::from_utf8_lossy(b).into_owned())
            }
            (_, 0x60) => {
                let Some(n) = self.size(&mut at, low)?.checked_mul(2) else { return invalid() };
                let b = self.read(&mut at, n);
                if b.len() != n {
                    return invalid();
                }
                let units: Vec<u16> = b.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                match String::from_utf16(&units) {
                    Ok(s) => P::Str(s),
                    Err(_) => return invalid(),
                }
            }
            (_, 0x80) => {
                // int.from_bytes(read(1 + low))：最多 16 个字节，超过 2^64 的 UID 在 Python 里是 ValueError
                let b = self.read(&mut at, 1 + low as usize);
                let v = b.iter().fold(0u128, |acc, x| (acc << 8) | *x as u128);
                match u64::try_from(v) {
                    Ok(u) => P::Uid(u),
                    Err(_) => return invalid(),
                }
            }
            (_, 0xa0) => {
                let n = self.size(&mut at, low)?;
                let refs = self.ints(&mut at, n as u64, self.ref_size)?;
                self.objects[slot] = Slot::Reading;
                let mut items = Vec::with_capacity(refs.len());
                for r in refs {
                    items.push(self.object(r, depth + 1)?);
                }
                P::Array(items)
            }
            (_, 0xd0) => {
                let n = self.size(&mut at, low)?;
                let keys = self.ints(&mut at, n as u64, self.ref_size)?;
                let values = self.ints(&mut at, n as u64, self.ref_size)?;
                self.objects[slot] = Slot::Reading;
                let mut d = P::Dict(Vec::with_capacity(keys.len()));
                for (k, v) in keys.into_iter().zip(values) {
                    let P::Str(key) = self.object(k, depth + 1)? else { return invalid() };
                    let value = self.object(v, depth + 1)?;
                    d.set(key, value);
                }
                d
            }
            _ => return invalid(),
        };
        self.objects[slot] = Slot::Done(result.clone());
        Ok(result)
    }
}

fn loads_binary(data: &[u8]) -> R<P> {
    if data.len() < 32 {
        return invalid();
    }
    let trailer = &data[data.len() - 32..];
    let (offset_size, ref_size) = (trailer[6] as usize, trailer[7] as usize);
    let word = |i: usize| u64::from_be_bytes(trailer[i..i + 8].try_into().expect("8 个字节"));
    let (num_objects, top, table_at) = (word(8), word(16), word(24));
    let mut reader = BinReader { data, ref_size, offsets: Vec::new(), objects: Vec::new() };
    let Ok(mut at) = usize::try_from(table_at) else { return invalid() };
    reader.offsets = reader.ints(&mut at, num_objects, offset_size)?;
    reader.objects = vec![Slot::Unread; reader.offsets.len()];
    reader.object(top, 0)
}

// ---------- 读 XML ----------

/// 一个还没收尾的数组 / 字典，和它收尾以后放到哪（`add_object` 在开始标签那里就核对、认领了位置）
struct Frame {
    node: P,
    place: Place,
}

enum Place {
    Root,
    Append,
    Key(String),
}

struct XmlReader<'a> {
    s: &'a str,
    at: usize,
    line: usize,
    /// 当前事件（标签）开头在第几行：报错里的 `CurrentLineNumber`
    event_line: usize,
    open: Vec<&'a str>,
    stack: Vec<Frame>,
    current_key: Option<String>,
    root: P,
    data: String,
    /// 文档带着没读的外部 DTD（DOCTYPE 里有 PUBLIC / SYSTEM，又没声明 standalone="yes"）：没定义的实体不算错，跳过
    lenient_entities: bool,
}

fn expat<T>(msg: &str, line: usize) -> R<T> {
    other("ExpatError", format!("{msg}: line {line}"))
}

fn is_name_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == ':' || (c as u32) >= 0x80
}

fn is_name_char(c: char) -> bool {
    is_name_start(c) || c.is_ascii_digit() || c == '-' || c == '.'
}

/// XML 里能出现的字符（控制字符只有 TAB、换行、回车）
fn is_xml_char(c: char) -> bool {
    matches!(c as u32, 0x09 | 0x0a | 0x0d | 0x20..=0xd7ff | 0xe000..=0xfffd | 0x10000..=0x10ffff)
}

impl<'a> XmlReader<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.at..]
    }

    fn advance(&mut self, n: usize) {
        self.line += self.s[self.at..self.at + n].bytes().filter(|b| *b == b'\n').count();
        self.at += n;
    }

    fn bad<T>(&self, msg: &str) -> R<T> {
        expat(msg, self.line)
    }

    /// 跳到 `end` 后面；找不到是没写完
    fn skip_past(&mut self, end: &str) -> R<&'a str> {
        match self.rest().find(end) {
            Some(i) => {
                let inner = &self.rest()[..i];
                self.advance(i + end.len());
                Ok(inner)
            }
            None => self.bad("unclosed token"),
        }
    }

    fn skip_space(&mut self) {
        let n = self.rest().len() - self.rest().trim_start_matches([' ', '\t', '\n']).len();
        self.advance(n);
    }

    fn name(&mut self) -> R<&'a str> {
        let rest = self.rest();
        let end = rest
            .char_indices()
            .find(|(i, c)| if *i == 0 { !is_name_start(*c) } else { !is_name_char(*c) })
            .map_or(rest.len(), |(i, _)| i);
        if end == 0 {
            return self.bad("not well-formed (invalid token)");
        }
        self.advance(end);
        Ok(&rest[..end])
    }

    /// `&amp;`、`&#10;` 这些换成字符；不认识的名字是错（属性列表里不许自己定义实体）
    fn unescape(&self, raw: &str, out: &mut String) -> R<()> {
        let mut rest = raw;
        while let Some(i) = rest.find('&') {
            out.push_str(&rest[..i]);
            rest = &rest[i + 1..];
            let Some(end) = rest.find(';') else { return self.bad("not well-formed (invalid token)") };
            let name = &rest[..end];
            rest = &rest[end + 1..];
            let number = match name.strip_prefix('#') {
                Some(n) => match n.strip_prefix('x') {
                    Some(hex) if !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()) => {
                        Some(u32::from_str_radix(hex, 16).ok())
                    }
                    None if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => Some(n.parse::<u32>().ok()),
                    _ => return self.bad("not well-formed (invalid token)"),
                },
                None => None,
            };
            match (name, number) {
                (_, Some(n)) => match n.and_then(char::from_u32).filter(|c| is_xml_char(*c)) {
                    Some(c) => out.push(c),
                    None => return self.bad("reference to invalid character number"),
                },
                ("amp", _) => out.push('&'),
                ("lt", _) => out.push('<'),
                ("gt", _) => out.push('>'),
                ("quot", _) => out.push('"'),
                ("apos", _) => out.push('\''),
                _ if self.lenient_entities => {}
                _ => return self.bad("undefined entity"),
            }
        }
        out.push_str(rest);
        Ok(())
    }

    fn chars(&mut self, raw: &'a str, literal: bool) -> R<()> {
        if let Some(c) = raw.chars().find(|c| !is_xml_char(*c)) {
            let _ = c;
            return self.bad("not well-formed (invalid token)");
        }
        if self.open.is_empty() {
            // 根元素外面只能有空白
            return if raw.trim_matches([' ', '\t', '\n']).is_empty() && !literal { Ok(()) } else { self.bad("syntax error") };
        }
        if literal {
            self.data.push_str(raw);
            Ok(())
        } else {
            if raw.contains("]]>") {
                return self.bad("not well-formed (invalid token)");
            }
            let mut text = std::mem::take(&mut self.data);
            let done = self.unescape(raw, &mut text);
            self.data = text;
            done
        }
    }

    fn get_data(&mut self) -> String {
        std::mem::take(&mut self.data)
    }

    /// `add_object` 的前一半：这个值该放到哪（认领当前的键），放不了是错
    fn claim(&mut self) -> R<Place> {
        if let Some(key) = self.current_key.take() {
            return match self.stack.last() {
                Some(Frame { node: P::Dict(_), .. }) => Ok(Place::Key(key)),
                Some(_) => value_error(format!("unexpected element at line {}", self.event_line)),
                None => other("IndexError", "list index out of range"),
            };
        }
        match self.stack.last() {
            None => Ok(Place::Root),
            Some(Frame { node: P::Array(_), .. }) => Ok(Place::Append),
            Some(_) => value_error(format!("unexpected element at line {}", self.event_line)),
        }
    }

    /// `add_object` 的后一半
    fn put(&mut self, place: Place, value: P) {
        match (place, self.stack.last_mut()) {
            (Place::Key(k), Some(frame)) => frame.node.set(k, value),
            (Place::Append, Some(Frame { node: P::Array(items), .. })) => items.push(value),
            _ => self.root = value,
        }
    }

    fn add(&mut self, value: P) -> R<()> {
        let place = self.claim()?;
        self.put(place, value);
        Ok(())
    }

    fn begin(&mut self, element: &str) -> R<()> {
        self.data.clear();
        let node = match element {
            "dict" => P::Dict(Vec::new()),
            "array" => P::Array(Vec::new()),
            _ => return Ok(()),
        };
        let place = self.claim()?;
        self.stack.push(Frame { node, place });
        Ok(())
    }

    fn end(&mut self, element: &str) -> R<()> {
        let line = self.event_line;
        match element {
            "dict" | "array" => {
                if element == "dict" {
                    if let Some(key) = self.current_key.as_ref().filter(|k| !k.is_empty()) {
                        return value_error(format!("missing value for key '{key}' at line {line}"));
                    }
                }
                let Some(frame) = self.stack.pop() else { return other("IndexError", "pop from empty list") };
                self.put(frame.place, frame.node);
            }
            "key" => {
                let pending = self.current_key.as_ref().is_some_and(|k| !k.is_empty());
                match self.stack.last() {
                    None => return other("IndexError", "list index out of range"),
                    Some(Frame { node: P::Dict(_), .. }) if !pending => {}
                    Some(_) => return value_error(format!("unexpected key at line {line}")),
                }
                self.current_key = Some(self.get_data());
            }
            "true" => self.add(P::Bool(true))?,
            "false" => self.add(P::Bool(false))?,
            "integer" => {
                let raw = self.get_data();
                let parsed = match raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
                    Some(_) => int_base16(&raw),
                    None => int_base10(&raw),
                };
                match parsed {
                    Some(i) => self.add(P::Int(i))?,
                    None => {
                        let base = if raw.starts_with("0x") || raw.starts_with("0X") { 16 } else { 10 };
                        return value_error(format!("invalid literal for int() with base {base}: {}", super::obj::repr_str(&raw)));
                    }
                }
            }
            "real" => {
                let raw = self.get_data();
                match text::to_float(&raw) {
                    Some(f) => self.add(P::Real(f))?,
                    None => return value_error(format!("could not convert string to float: {}", super::obj::repr_str(&raw))),
                }
            }
            "string" => {
                let s = self.get_data();
                self.add(P::Str(s))?
            }
            "data" => {
                let raw = self.get_data();
                self.add(P::Data(a2b_base64(raw.as_bytes(), false)?))?
            }
            "date" => {
                let raw = self.get_data();
                self.add(P::Date(date_from_string(&raw)?))?
            }
            _ => {}
        }
        Ok(())
    }

    /// 属性：`名字="值"`，只看写得对不对（属性列表不用属性）
    fn attributes(&mut self) -> R<bool> {
        let mut seen: Vec<&str> = Vec::new();
        loop {
            let before = self.at;
            self.skip_space();
            let rest = self.rest();
            if rest.starts_with("/>") {
                self.advance(2);
                return Ok(true);
            }
            if rest.starts_with('>') {
                self.advance(1);
                return Ok(false);
            }
            if rest.is_empty() {
                return self.bad("unclosed token");
            }
            if self.at == before {
                return self.bad("not well-formed (invalid token)");
            }
            let name = self.name()?;
            if seen.contains(&name) {
                return self.bad("duplicate attribute");
            }
            seen.push(name);
            self.skip_space();
            if !self.rest().starts_with('=') {
                return self.bad("not well-formed (invalid token)");
            }
            self.advance(1);
            self.skip_space();
            let Some(quote) = self.rest().chars().next().filter(|c| matches!(c, '"' | '\'')) else {
                return self.bad("not well-formed (invalid token)");
            };
            self.advance(1);
            let value = self.skip_past(if quote == '"' { "\"" } else { "'" })?;
            if value.contains('<') || value.chars().any(|c| !is_xml_char(c)) {
                return self.bad("not well-formed (invalid token)");
            }
            self.unescape(value, &mut String::new())?;
        }
    }

    /// `<!DOCTYPE …>`：里面自己定义实体的不收（plistlib 也不收），别的跳过
    fn doctype(&mut self, standalone: bool) -> R<()> {
        self.skip_space();
        self.name()?;
        self.skip_space();
        if !standalone && (self.rest().starts_with("PUBLIC") || self.rest().starts_with("SYSTEM")) {
            self.lenient_entities = true;
        }
        let mut depth = 0usize;
        loop {
            let rest = self.rest();
            let Some(c) = rest.chars().next() else { return self.bad("unclosed token") };
            if rest.starts_with("<!ENTITY") {
                return value_error("XML entity declarations are not supported in plist files");
            }
            if rest.starts_with("<!--") {
                self.advance(4);
                self.skip_past("-->")?;
                continue;
            }
            match c {
                '"' | '\'' => {
                    self.advance(1);
                    self.skip_past(if c == '"' { "\"" } else { "'" })?;
                    continue;
                }
                '[' | '<' => depth += 1,
                ']' => depth = depth.saturating_sub(1),
                '>' if depth == 0 => {
                    self.advance(1);
                    return Ok(());
                }
                '>' => depth -= 1,
                _ => {}
            }
            self.advance(c.len_utf8());
        }
    }

    fn parse(mut self) -> R<P> {
        let mut seen_root = false;
        let mut seen_doctype = false;
        let mut standalone = false;
        if self.rest().starts_with("<?xml") && self.rest()[5..].starts_with([' ', '\t', '\n']) {
            self.advance(5);
            let declaration = self.skip_past("?>")?;
            let value = declaration.split("standalone").nth(1).map(|v| v.trim_start().trim_start_matches('=').trim_start());
            standalone = value.is_some_and(|v| v.starts_with("\"yes\"") || v.starts_with("'yes'"));
        }
        loop {
            let rest = self.rest();
            if rest.is_empty() {
                break;
            }
            if !rest.starts_with('<') {
                let end = rest.find('<').unwrap_or(rest.len());
                self.chars(&rest[..end], false)?;
                self.advance(end);
                continue;
            }
            self.event_line = self.line;
            if rest.starts_with("<!--") {
                self.advance(4);
                if self.skip_past("-->")?.contains("--") {
                    return self.bad("not well-formed (invalid token)");
                }
            } else if rest.starts_with("<![CDATA[") {
                if self.open.is_empty() {
                    return self.bad("syntax error");
                }
                self.advance(9);
                let inner = self.skip_past("]]>")?;
                self.chars(inner, true)?;
            } else if rest.starts_with("<?") {
                self.advance(2);
                let target = self.name()?;
                if target.eq_ignore_ascii_case("xml") {
                    return self.bad("XML or text declaration not at start of entity");
                }
                self.skip_past("?>")?;
            } else if rest.starts_with("<!DOCTYPE") {
                if seen_root || seen_doctype {
                    return self.bad("syntax error");
                }
                seen_doctype = true;
                self.advance(9);
                self.doctype(standalone)?;
            } else if rest.starts_with("</") {
                self.advance(2);
                let name = self.name()?;
                self.skip_space();
                if !self.rest().starts_with('>') {
                    return self.bad("not well-formed (invalid token)");
                }
                self.advance(1);
                if self.open.pop() != Some(name) {
                    return self.bad("mismatched tag");
                }
                self.end(name)?;
            } else {
                if seen_root && self.open.is_empty() {
                    return self.bad("junk after document element");
                }
                seen_root = true;
                self.advance(1);
                let name = self.name()?;
                let empty = self.attributes()?;
                self.begin(name)?;
                if empty {
                    self.end(name)?;
                } else {
                    self.open.push(name);
                }
            }
        }
        if !seen_root || !self.open.is_empty() {
            return self.bad("no element found");
        }
        Ok(self.root)
    }
}

/// `int(s)`（十进制）：前后的空白、正负号、数字之间的下划线
fn int_base10(s: &str) -> Option<i128> {
    int_digits(text::strip(s), 10)
}

/// `int(s, 16)`：前后的空白、正负号、`0x` 前缀、数字之间的下划线
fn int_base16(s: &str) -> Option<i128> {
    let s = text::strip(s);
    let (sign, body) = match s.strip_prefix('-') {
        Some(b) => ("-", b),
        None => ("", s.strip_prefix('+').unwrap_or(s)),
    };
    let body = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X"))?;
    // 前缀后面可以紧跟一个下划线（0x_1f）
    int_digits(&format!("{sign}{}", body.strip_prefix('_').unwrap_or(body)), 16)
}

fn int_digits(s: &str, radix: u32) -> Option<i128> {
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let mut v: i128 = 0;
    for c in body.chars().filter(|c| *c != '_') {
        let d = if radix == 10 { super::uni::decimal_value(c)? } else { c.to_digit(16)? };
        v = v.checked_mul(radix as i128)?.checked_add(d as i128)?;
    }
    Some(if neg { -v } else { v })
}

/// 文件的字节 → 文字：UTF-8（BOM 去掉）、带 BOM 的 UTF-16、声明了 ISO-8859-1 / US-ASCII 的
fn decode_xml(data: &[u8]) -> R<String> {
    let utf16 = |be: bool, body: &[u8]| {
        let units: Vec<u16> = body
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| if be { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) })
            .collect();
        String::from_utf16(&units).or_else(|_| expat("not well-formed (invalid token)", 1))
    };
    let text = match data {
        [0xef, 0xbb, 0xbf, rest @ ..] => {
            String::from_utf8(rest.to_vec()).or_else(|_| expat("not well-formed (invalid token)", 1))?
        }
        [0xfe, 0xff, rest @ ..] => utf16(true, rest)?,
        [0xff, 0xfe, rest @ ..] => utf16(false, rest)?,
        _ => {
            let head = String::from_utf8_lossy(&data[..data.len().min(200)]).to_ascii_lowercase();
            let declared = head.split("?>").next().filter(|_| head.starts_with("<?xml")).and_then(|decl| {
                let after = &decl[decl.find("encoding")? + 8..];
                let after = after.trim_start().strip_prefix('=')?.trim_start();
                let quote = after.chars().next().filter(|c| matches!(c, '"' | '\''))?;
                Some(after[1..].split(quote).next()?.to_string())
            });
            match declared.as_deref() {
                None | Some("utf-8") | Some("utf8") => {
                    String::from_utf8(data.to_vec()).or_else(|_| expat("not well-formed (invalid token)", 1))?
                }
                Some("iso-8859-1") | Some("us-ascii") => data.iter().map(|b| *b as char).collect(),
                Some(_) => return expat("unknown encoding", 1),
            }
        }
    };
    Ok(if text.contains('\r') { text.replace("\r\n", "\n").replace('\r', "\n") } else { text })
}

fn is_xml(header: &[u8]) -> bool {
    let prefixes: [&[u8]; 2] = [b"<?xml", b"<plist"];
    if prefixes.iter().any(|p| header.starts_with(p)) {
        return true;
    }
    let wide = |p: &[u8], be: bool| -> Vec<u8> { p.iter().flat_map(|c| if be { [0, *c] } else { [*c, 0] }).collect() };
    prefixes.iter().any(|p| {
        header.strip_prefix(&[0xef, 0xbb, 0xbf]).is_some_and(|h| h.starts_with(p))
            || header.strip_prefix(&[0xfe, 0xff]).is_some_and(|h| h.starts_with(&wide(p, true)))
            || header.strip_prefix(&[0xff, 0xfe]).is_some_and(|h| h.starts_with(&wide(p, false)))
    })
}

/// `plistlib.loads(data)`（`plistlib.load(f)` 也是它）：看开头认格式，认不出是 InvalidFileException
pub fn loads(data: &[u8]) -> R<P> {
    let header = &data[..data.len().min(32)];
    if is_xml(header) {
        let text = decode_xml(data)?;
        let reader = XmlReader {
            s: &text,
            at: 0,
            line: 1,
            event_line: 1,
            open: Vec::new(),
            stack: Vec::new(),
            current_key: None,
            root: P::Null,
            data: String::new(),
            lenient_entities: false,
        };
        reader.parse()
    } else if header.starts_with(b"bplist00") {
        loads_binary(data)
    } else {
        invalid()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::py::hash::{from_hex, to_hex};
    use crate::py::json;
    use crate::py::obj::V;

    /// 向量里带类型标记的值 → P
    fn build(v: &V) -> P {
        let d = v.as_dict().unwrap();
        let val = d.get("v").unwrap_or(V::None);
        match d.at("t").py_str().as_str() {
            "null" => P::Null,
            "bool" => P::Bool(val.truthy()),
            "int" => P::Int(val.py_str().parse().unwrap()),
            "real" => P::Real(f64::from_bits(u64::from_str_radix(&val.py_str(), 16).unwrap())),
            "str" => P::Str(val.py_str()),
            "data" => P::Data(from_hex(&val.py_str()).unwrap()),
            "dataseq" => P::Data((0..val.py_str().parse::<usize>().unwrap()).map(|i| (i * 7 % 256) as u8).collect()),
            "date" => P::Date(val.py_str().parse().unwrap()),
            "uid" => P::Uid(val.py_str().parse().unwrap()),
            "array" => P::Array(val.as_list().unwrap().items().iter().map(build).collect()),
            "dict" => P::Dict(
                val.as_list()
                    .unwrap()
                    .items()
                    .iter()
                    .map(|kv| {
                        let kv = kv.as_list().unwrap();
                        (kv.at(0).py_str(), build(&kv.at(1)))
                    })
                    .collect(),
            ),
            other => panic!("不认识的标记 {other}"),
        }
    }

    /// 两棵树一样（浮点数比每一位，NaN 也算一样）
    fn same(a: &P, b: &P) -> bool {
        match (a, b) {
            (P::Real(x), P::Real(y)) => x.to_bits() == y.to_bits(),
            (P::Array(x), P::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
            (P::Dict(x), P::Dict(y)) => x.len() == y.len() && x.iter().zip(y).all(|((k, p), (l, q))| k == l && same(p, q)),
            _ => a == b,
        }
    }

    fn class_of(e: &Fail) -> &'static str {
        match e {
            Fail::Value(_) => "ValueError",
            Fail::Other(c, _) => c,
            _ => "?",
        }
    }

    fn check_error(e: &Fail, want: &crate::py::obj::Dict, what: &str) {
        let class = want.at("err").py_str();
        // InvalidFileException、binascii.Error 都是 ValueError 的子类
        let class = if matches!(class.as_str(), "InvalidFileException" | "Error") { "ValueError".to_string() } else { class };
        assert_eq!(class_of(e), class, "{what}：{e:?}");
        if class == "ValueError" {
            assert_eq!(e.to_string(), want.at("msg").py_str(), "{what}");
        }
    }

    #[test]
    fn matches_cpython_vectors() {
        let doc = json::loads(include_str!("../../tests/vectors/plist.json")).unwrap();
        let cases = doc.as_dict().unwrap().dict("cases").unwrap();
        let dumps = cases.list("dumps").unwrap().items();
        assert!(dumps.len() > 40);
        for c in dumps {
            let c = c.as_dict().unwrap();
            let name = c.at("name").py_str();
            let value = build(&c.at("value"));
            for (kind, got) in [("xml", dumps_xml(&value)), ("binary", dumps_binary(&value))] {
                let want = c.dict(kind).unwrap();
                match (got, want.get("ok").or_else(|| want.get("sha256"))) {
                    (Ok(bytes), Some(ok)) => {
                        // 大的输出向量里只记了摘要
                        let digest = if want.has("sha256") { crate::py::hash::sha256_hex(&bytes) } else { to_hex(&bytes) };
                        assert_eq!(digest, ok.py_str(), "dumps {kind} {name}");
                        // 写出去的再读回来，和 Python 读回来的一样（XML 里日期只到秒；最大的日期写成秒数进了位，读不回来）
                        let back = want.dict("back").unwrap();
                        match (loads(&bytes), back.get("ok")) {
                            (Ok(p), Some(ok)) => assert!(same(&p, &build(&ok)), "读回 {kind} {name}：{p:?}"),
                            (Err(e), None) => check_error(&e, &back, &format!("读回 {kind} {name}")),
                            (got, _) => panic!("读回 {kind} {name}：{got:?}，应该是 {back:?}"),
                        }
                    }
                    (Err(e), None) => check_error(&e, &want, &format!("dumps {kind} {name}")),
                    (got, _) => panic!("dumps {kind} {name}：{:?}，应该是 {:?}", got.map(|b| to_hex(&b)), want),
                }
            }
        }
        let loads_cases = cases.list("loads").unwrap().items();
        assert!(loads_cases.len() > 80);
        for c in loads_cases {
            let c = c.as_dict().unwrap();
            let name = c.at("name").py_str();
            let got = loads(&from_hex(&c.at("hex").py_str()).unwrap());
            let want = c.dict("out").unwrap();
            match (got, want.get("ok")) {
                (Ok(p), Some(ok)) => assert!(same(&p, &build(&ok)), "loads {name}：{p:?}"),
                (Err(e), None) => check_error(&e, &want, &format!("loads {name}")),
                (got, _) => panic!("loads {name}：{got:?}，应该是 {want:?}"),
            }
        }
    }

    #[test]
    fn self_referencing_containers_are_rejected() {
        // 一个数组，唯一的元素是它自己（Python 读得出一个环；这里的值是一棵树，装不下）
        let mut data = b"bplist00".to_vec();
        data.extend_from_slice(&[0xa1, 0x00]);
        data.push(8); // 对象 0 在第 8 个字节
        data.extend_from_slice(&[0, 0, 0, 0, 0, 0, 1, 1]);
        for word in [1u64, 0, 10] {
            data.extend_from_slice(&word.to_be_bytes());
        }
        assert!(matches!(loads(&data), Err(Fail::Value(m)) if m == "Invalid file"));
    }

    #[test]
    fn dates_round_trip_through_the_calendar() {
        for (y, m, d) in [(1, 1, 1), (1999, 12, 31), (2000, 2, 29), (2001, 1, 1), (2024, 2, 29), (2100, 3, 1), (9999, 12, 31)] {
            assert_eq!(civil_from_days(days_from_civil(y, m, d)), (y, m, d));
        }
        assert_eq!(days_from_civil(2001, 1, 1), DAYS_TO_2001);
        assert_eq!(days_from_civil(9999, 12, 31), DAYS_TO_9999_END);
        assert_eq!(date_to_string(0), "2001-01-01T00:00:00Z");
        assert_eq!(date_to_string(-1), "2000-12-31T23:59:59Z");
    }
}
