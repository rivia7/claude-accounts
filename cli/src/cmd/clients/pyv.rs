//! 这一块里 Python 的动态行为：state.json、transaction.json 是从文件读进来的任意 JSON，原来的代码直接 `stage['metadata']['version']`、
//! `metadata.get('sha256')` 地用，类型不对时出的是哪一类异常决定了后面走哪条路（`except (ClientUpdateError, OSError, ValueError,
//! TypeError, KeyError)` 接得住 TypeError / KeyError，接不住 AttributeError）。这里照 Python 的规矩给出同一类异常。

use crate::py::obj::{repr_str, Dict, V};
use crate::py::text;
use crate::py::uni;
use crate::py::{Fail, R};
use std::cmp::Ordering;

/// `ClientUpdateError(status, message)`：`str(e)` 是 message
pub fn cu_error<T>(status: &str, message: impl Into<String>) -> R<T> {
    Err(cu_fail(status, message))
}

pub fn cu_fail(status: &str, message: impl Into<String>) -> Fail {
    Fail::Other("ClientUpdateError", vec![message.into(), status.to_string()])
}

/// `(e.status, e.message)`；不是 ClientUpdateError 是 None
pub fn cu_parts(e: &Fail) -> Option<(&str, &str)> {
    match e {
        Fail::Other("ClientUpdateError", args) if args.len() == 2 => Some((&args[1], &args[0])),
        _ => None,
    }
}

/// `e.status`（不是 ClientUpdateError 时是空串）
pub fn cu_status(e: &Fail) -> &str {
    cu_parts(e).map_or("", |p| p.0)
}

pub fn is_cu(e: &Fail) -> bool {
    e.is("ClientUpdateError")
}

/// `except (ClientUpdateError, OSError, ValueError, TypeError, KeyError)`
pub fn caught(e: &Fail) -> bool {
    is_cu(e) || e.is_os_or_value() || e.is("TypeError") || e.is("KeyError")
}

/// `except Exception`：`die()` / `sys.exit()`（SystemExit）和 Ctrl-C（KeyboardInterrupt）不算
pub fn is_exception(e: &Fail) -> bool {
    // PowerLoss：单元测试里模拟断电用的（原来的测试里也是一个 BaseException）
    !e.is_exit() && !e.is("KeyboardInterrupt") && !(cfg!(test) && e.is("PowerLoss"))
}

pub fn type_error<T>(msg: impl Into<String>) -> R<T> {
    Err(Fail::Other("TypeError", vec![msg.into()]))
}

pub fn key_error<T>(key: &str) -> R<T> {
    Err(Fail::Other("KeyError", vec![repr_str(key)]))
}

pub fn attr_error<T>(msg: impl Into<String>) -> R<T> {
    Err(Fail::Other("AttributeError", vec![msg.into()]))
}

/// `type(x).__name__`
pub fn type_name(v: &V) -> &'static str {
    match v {
        V::None => "NoneType",
        V::Bool(_) => "bool",
        V::Int(_) | V::Big(_) => "int",
        V::Float(_) => "float",
        V::Str(_) => "str",
        V::List(_) => "list",
        V::Dict(_) => "dict",
    }
}

/// `v[key]`（key 是字符串）：字典里没有是 KeyError，别的类型是 TypeError
pub fn sub(v: &V, key: &str) -> R<V> {
    match v {
        V::Dict(d) => d.get(key).map_or_else(|| key_error(key), Ok),
        V::Str(_) => type_error("string indices must be integers, not 'str'"),
        V::List(_) => type_error("list indices must be integers or slices, not str"),
        other => type_error(format!("'{}' object is not subscriptable", type_name(other))),
    }
}

/// `d[key]`
pub fn at(d: &Dict, key: &str) -> R<V> {
    d.get(key).map_or_else(|| key_error(key), Ok)
}

/// `v[a][b]`
pub fn sub2(v: &V, a: &str, b: &str) -> R<V> {
    sub(&sub(v, a)?, b)
}

/// `v.get(key)`：不是字典是 AttributeError（那几个 except 都接不住）
pub fn get(v: &V, key: &str) -> R<Option<V>> {
    match v {
        V::Dict(d) => Ok(d.get(key)),
        other => attr_error(format!("'{}' object has no attribute 'get'", type_name(other))),
    }
}

/// 路径一类的参数（`os.path.join`、`os.lstat`、`subprocess` 的参数……）必须是字符串：别的是 TypeError
pub fn path_str(v: &V) -> R<String> {
    match v {
        V::Str(s) => Ok(s.to_string()),
        other => type_error(format!("expected str, bytes or os.PathLike object, not {}", type_name(other))),
    }
}

/// Python 的整数（多大都行）：符号和去掉了前导 0 的十进制数字。版本号、构建号比大小用
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PyInt {
    neg: bool,
    digits: String,
}

impl PyInt {
    pub fn zero() -> PyInt {
        PyInt { neg: false, digits: "0".into() }
    }

    /// `int(s)`：前后的空白、正负号、数字之间单个的下划线、各种文字的十进制数字都认；认不出、超过 4300 位（Python 3.11 起）是 ValueError
    pub fn parse(s: &str) -> R<PyInt> {
        let bad = || Fail::Value(format!("invalid literal for int() with base 10: {}", repr_str(s)));
        let body = text::strip(s);
        let (neg, body) = match body.strip_prefix('-') {
            Some(b) => (true, b),
            None => (false, body.strip_prefix('+').unwrap_or(body)),
        };
        let mut digits = String::new();
        let mut prev_digit = false;
        for c in body.chars() {
            if c == '_' {
                if !prev_digit {
                    return Err(bad());
                }
                prev_digit = false;
                continue;
            }
            let d = uni::decimal_value(c).ok_or_else(bad)?;
            digits.push(char::from_digit(d, 10).expect("十进制数字"));
            prev_digit = true;
        }
        if digits.is_empty() || !prev_digit {
            return Err(bad());
        }
        if digits.len() > 4300 {
            return Err(Fail::Value(format!("Exceeds the limit (4300 digits) for integer string conversion: value has {} digits; use sys.set_int_max_str_digits() to increase the limit", digits.len())));
        }
        let trimmed = digits.trim_start_matches('0');
        let digits = if trimmed.is_empty() { "0".to_string() } else { trimmed.to_string() };
        Ok(PyInt { neg: neg && digits != "0", digits })
    }

    /// 放得进 i64 时的值
    pub fn to_i64(&self) -> Option<i64> {
        self.to_decimal().parse().ok()
    }

    pub fn from_i64(n: i64) -> PyInt {
        PyInt { neg: n < 0, digits: n.unsigned_abs().to_string() }
    }

    pub fn is_negative(&self) -> bool {
        self.neg
    }

    /// `str(n)`
    pub fn to_decimal(&self) -> String {
        format!("{}{}", if self.neg { "-" } else { "" }, self.digits)
    }

    /// `self + n`：按位加减
    pub fn add_i64(&self, n: i64) -> PyInt {
        let other = PyInt::from_i64(n);
        let smaller = self.digits.len().cmp(&other.digits.len()).then_with(|| self.digits.cmp(&other.digits)) == Ordering::Less;
        let digits_of = |s: &str| -> Vec<u8> { s.bytes().rev().map(|b| b - b'0').collect() };
        let (a, b) = (digits_of(&self.digits), digits_of(&other.digits));
        let (neg, mut out) = if self.neg == other.neg {
            let mut out = Vec::with_capacity(a.len().max(b.len()) + 1);
            let mut carry = 0;
            for i in 0..a.len().max(b.len()) {
                let sum = a.get(i).copied().unwrap_or(0) + b.get(i).copied().unwrap_or(0) + carry;
                out.push(sum % 10);
                carry = sum / 10;
            }
            if carry > 0 {
                out.push(carry);
            }
            (self.neg, out)
        } else {
            // 符号不一样：大的减小的，符号跟大的
            let (big, small, neg) = if smaller { (&b, &a, other.neg) } else { (&a, &b, self.neg) };
            let mut out = Vec::with_capacity(big.len());
            let mut borrow = 0i8;
            for (i, digit) in big.iter().enumerate() {
                let mut d = *digit as i8 - small.get(i).copied().unwrap_or(0) as i8 - borrow;
                borrow = (d < 0) as i8;
                if d < 0 {
                    d += 10;
                }
                out.push(d as u8);
            }
            (neg, out)
        };
        while out.len() > 1 && out.last() == Some(&0) {
            out.pop();
        }
        let digits: String = out.iter().rev().map(|d| (b'0' + d) as char).collect();
        PyInt { neg: neg && digits != "0", digits }
    }
}

impl Ord for PyInt {
    fn cmp(&self, other: &PyInt) -> Ordering {
        let magnitude = |a: &PyInt, b: &PyInt| a.digits.len().cmp(&b.digits.len()).then_with(|| a.digits.cmp(&b.digits));
        match (self.neg, other.neg) {
            (false, false) => magnitude(self, other),
            (true, true) => magnitude(other, self),
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
        }
    }
}

impl PartialOrd for PyInt {
    fn partial_cmp(&self, other: &PyInt) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// `\d+(?:\.\d+){min,max}`（fullmatch）：`\d` 是 Python 认的十进制数字（各种文字的）。不经正则库：它的 Unicode 表和 Python 的不是同一版
pub fn dotted(s: &str, min_dots: usize, max_dots: usize) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    (min_dots + 1..=max_dots + 1).contains(&parts.len()) && parts.iter().all(|p| !p.is_empty() && p.chars().all(uni::is_decimal))
}

/// `\d{1,12}`（fullmatch）
pub fn digits_1_12(s: &str) -> bool {
    let n = s.chars().count();
    (1..=12).contains(&n) && s.chars().all(uni::is_decimal)
}

/// `[0-9a-f]{64}`（fullmatch）
pub fn hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// 整数和小数比大小，照 Python 的精确比（不是先把整数变成小数：纳秒的时间戳变成小数会丢掉末几位）
fn cmp_int_float(i: i128, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    if f.is_infinite() {
        return Some(if f > 0.0 { Ordering::Less } else { Ordering::Greater });
    }
    // 变成小数是单调的：不相等时先后就是对的；相等时再精确比（这时 f 是整数，i128 放得下）
    match (i as f64).partial_cmp(&f)? {
        Ordering::Equal if f.abs() < 1.7e38 => Some(i.cmp(&(f as i128))),
        other => Some(other),
    }
}

/// Python 的 `==`，和 `V::py_eq` 一样，只是整数和小数精确地比：兼容层的 `py_eq` 把整数先变成小数再比，1795078097521852400 和
/// 1.7950780975218524e+18 在它那里相等、在 Python 里不等。这一块拿快照里纳秒的时间戳、inode 和 state.json 里记的比，要精确
pub fn eq(a: &V, b: &V) -> bool {
    match (a, b) {
        (V::List(x), V::List(y)) => {
            let (x, y) = (x.items(), y.items());
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| eq(p, q))
        }
        (V::Dict(x), V::Dict(y)) => x.len() == y.len() && x.items().iter().all(|(k, v)| y.get(k).is_some_and(|w| eq(v, &w))),
        _ => match (Num::of(a, "=="), Num::of(b, "==")) {
            (Ok(x), Ok(y)) => x.cmp(&y) == Some(Ordering::Equal),
            (Err(_), Err(_)) => a.py_eq(b),
            _ => false,
        },
    }
}

/// `a.eq_py(&b)`：就是 `eq(&a, &b)`，写在值后面好读
pub trait PyEq {
    fn eq_py(&self, other: &V) -> bool;
}

impl PyEq for V {
    fn eq_py(&self, other: &V) -> bool {
        eq(self, other)
    }
}

/// `a == b`，b 可能没有（`d.get(k)` 是 None）
pub fn eq_opt(a: &V, b: Option<V>) -> bool {
    match b {
        Some(b) => eq(a, &b),
        None => a.is_none(),
    }
}

/// Python 的数（`time.time() - x`、`min(…) + 间隔` 用）：整数（多大都行）和小数分开记，结果写成 JSON 时一个是 `21600`、一个是 `21600.0`
#[derive(Clone, Debug, PartialEq)]
pub enum Num {
    Int(i64),
    /// 放不进 i64 的整数
    Big(PyInt),
    Float(f64),
}

impl Num {
    /// JSON 里的数（布尔值也是整数）。别的类型做算术、比大小是 TypeError
    pub fn of(v: &V, what: &str) -> R<Num> {
        match v {
            V::Bool(b) => Ok(Num::Int(*b as i64)),
            V::Int(i) => Ok(Num::Int(*i)),
            V::Float(f) => Ok(Num::Float(*f)),
            V::Big(s) => Ok(Num::Big(PyInt::parse(s)?)),
            other => type_error(format!("unsupported operand type(s) for {}: '{}'", what, type_name(other))),
        }
    }

    /// `float(x)`：整数太大是 OverflowError
    pub fn f(&self) -> R<f64> {
        match self {
            Num::Int(i) => Ok(*i as f64),
            Num::Float(f) => Ok(*f),
            Num::Big(n) => match n.to_decimal().parse::<f64>() {
                Ok(f) if f.is_finite() => Ok(f),
                _ => Err(Fail::Other("OverflowError", vec!["int too large to convert to float".into()])),
            },
        }
    }

    fn as_big(&self) -> Option<PyInt> {
        match self {
            Num::Int(i) => Some(PyInt::from_i64(*i)),
            Num::Big(n) => Some(n.clone()),
            Num::Float(_) => None,
        }
    }

    /// `a + n`（n 是个不大的整数）
    pub fn add_int(&self, n: i64) -> Num {
        match self {
            Num::Float(f) => Num::Float(f + n as f64),
            Num::Int(i) => match i.checked_add(n) {
                Some(sum) => Num::Int(sum),
                None => Num::Big(PyInt::from_i64(*i).add_i64(n)),
            },
            Num::Big(big) => {
                let sum = big.add_i64(n);
                sum.to_i64().map_or(Num::Big(sum), Num::Int)
            }
        }
    }

    /// 比大小（有 NaN 时是 None：什么比较都不成立）
    pub fn cmp(&self, other: &Num) -> Option<Ordering> {
        match (self, other) {
            (Num::Float(a), Num::Float(b)) => a.partial_cmp(b),
            (Num::Float(f), n) => n.cmp(&Num::Float(*f)).map(Ordering::reverse),
            (Num::Int(i), Num::Float(f)) => cmp_int_float(*i as i128, *f),
            (Num::Big(n), Num::Float(f)) => {
                // 超出 i64 的整数：先按大小的量级比，量级一样再把小数写成精确的整数来比
                let approx: f64 = n.to_decimal().parse().unwrap_or(if n.is_negative() { f64::NEG_INFINITY } else { f64::INFINITY });
                match approx.partial_cmp(f)? {
                    Ordering::Equal => Some(n.cmp(&PyInt::parse(&format!("{f:.0}")).ok()?)),
                    other => Some(other),
                }
            }
            (a, b) => Some(a.as_big()?.cmp(&b.as_big()?)),
        }
    }

    /// `a < b`
    pub fn lt(&self, other: &Num) -> bool {
        self.cmp(other) == Some(Ordering::Less)
    }

    pub fn to_v(&self) -> V {
        match self {
            Num::Int(i) => V::Int(*i),
            Num::Float(f) => V::Float(*f),
            Num::Big(n) => V::Big(n.to_decimal().into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict;

    #[test]
    fn subscripts_raise_like_python() {
        let d = V::Dict(dict! { "a" => dict! { "b" => 1 } });
        assert!(sub2(&d, "a", "b").unwrap().py_eq(&V::Int(1)));
        assert!(sub(&d, "x").unwrap_err().is("KeyError"));
        for v in [V::None, V::Int(1), V::from("s"), V::List(Default::default()), V::Bool(true), V::Float(1.5)] {
            assert!(sub(&v, "a").unwrap_err().is("TypeError"), "{v:?}");
            assert!(get(&v, "a").unwrap_err().is("AttributeError"), "{v:?}");
        }
        assert!(get(&d, "x").unwrap().is_none());
        assert!(caught(&sub(&d, "x").unwrap_err()) && !caught(&get(&V::None, "a").unwrap_err()));
        let e = cu_fail("blocked", "说明");
        assert_eq!((cu_parts(&e), e.to_string().as_str()), (Some(("blocked", "说明")), "说明"));
        assert!(
            caught(&e)
                && is_exception(&e)
                && !is_exception(&Fail::Die("x".into()))
                && !is_exception(&Fail::Other("KeyboardInterrupt", vec![]))
        );
    }

    #[test]
    fn ints_of_any_size() {
        let n = |s: &str| PyInt::parse(s).unwrap();
        assert!(n("10") > n("9") && n("-10") < n("-9") && n("0") == n("-0") && n("007") == n("7") && n(" 1_000 ") == n("1000"));
        assert!(n("99999999999999999999999999") > n("9223372036854775807") && n("-1") < n("0"));
        assert_eq!(n("٣٤").to_i64(), Some(34));
        assert_eq!(n("99999999999999999999").to_i64(), None);
        for bad in ["", " ", "1__0", "_1", "1_", "a", "1.0", "+", "-", "1 2", "²"] {
            assert!(PyInt::parse(bad).unwrap_err().is_value(), "{bad:?}");
        }
        assert!(PyInt::parse(&"9".repeat(4300)).is_ok() && PyInt::parse(&"9".repeat(4301)).unwrap_err().is_value());
    }

    #[test]
    fn version_shapes() {
        assert!(dotted("1.2", 1, 5) && dotted("1.2.3.4.5.6", 1, 5) && !dotted("1", 1, 5) && !dotted("1.2.3.4.5.6.7", 1, 5));
        assert!(dotted("1", 0, 5) && !dotted("", 0, 5) && !dotted("1.", 0, 5) && !dotted(".1", 0, 5) && !dotted("1..2", 0, 5));
        assert!(dotted("١.٢", 1, 5) && !dotted("1.2\n", 1, 5) && !dotted("1.a", 1, 5) && !dotted("²", 0, 5));
        assert!(
            digits_1_12("1")
                && digits_1_12("123456789012")
                && !digits_1_12("1234567890123")
                && !digits_1_12("")
                && !digits_1_12("1a")
        );
        assert!(
            hex64(&"a".repeat(64))
                && !hex64(&"A".repeat(64))
                && !hex64(&"a".repeat(63))
                && !hex64(&format!("{}\n", "a".repeat(64)))
        );
    }

    #[test]
    fn numbers_keep_their_type() {
        // 右边都是 CPython 3.14 算的
        let n = |v: V| Num::of(&v, "+").unwrap();
        let big = |s: &str| V::Big(s.into());
        assert_eq!(n(V::Bool(true)).add_int(21600).to_v().py_repr(), "21601");
        assert_eq!(n(V::Float(1.5)).add_int(21600).to_v().py_repr(), "21601.5");
        assert_eq!(n(V::Int(0)).add_int(21600).to_v().py_repr(), "21600");
        assert_eq!(n(big("1000000000000000000000000000000")).add_int(21600).to_v().py_repr(), "1000000000000000000000000021600");
        assert_eq!(n(V::Int(i64::MAX)).add_int(21600).to_v().py_repr(), "9223372036854797407");
        assert_eq!(n(big("-9223372036854775809")).add_int(21600).to_v().py_repr(), "-9223372036854754209");
        assert_eq!(n(big("9223372036854775808")).add_int(-1), Num::Int(i64::MAX));
        assert_eq!(n(big("-10000000000000000000000")).add_int(1).to_v().py_repr(), "-9999999999999999999999");
        assert_eq!(n(big("10000000000000000000000")).add_int(-1).to_v().py_repr(), "9999999999999999999999");
        assert!(
            Num::Int(1).lt(&Num::Float(1.5))
                && !Num::Float(2.0).lt(&Num::Int(2))
                && Num::Float(-1e300).lt(&n(big("-99999999999999999999")))
        );
        assert!(
            n(big("99999999999999999999")).lt(&Num::Float(1e30))
                && !n(big("99999999999999999999")).lt(&Num::Int(5))
                && Num::Int(5).lt(&n(big("99999999999999999999")))
        );
        assert!(!Num::Float(f64::NAN).lt(&Num::Int(1)) && !Num::Int(1).lt(&Num::Float(f64::NAN)));
        assert!(Num::of(&V::from("1"), "-").unwrap_err().is("TypeError") && Num::of(&V::None, "-").unwrap_err().is("TypeError"));
        assert!(
            n(big(&"9".repeat(400))).f().unwrap_err().is("OverflowError") && n(big("100000000000000000000")).f().unwrap() == 1e20
        );
    }

    #[test]
    fn equality_is_exact_between_ints_and_floats() {
        // 1795078097521852400 == 1.7950780975218524e+18 在 Python 里是 False（那个小数其实是 …852416）
        assert!(!eq(&V::Int(1795078097521852400), &V::Float(1.7950780975218524e18)));
        assert!(eq(&V::Int(1795078097521852416), &V::Float(1.7950780975218524e18)));
        assert!(
            eq(&V::Int(1454), &V::Float(1454.0))
                && eq(&V::Bool(true), &V::Float(1.0))
                && eq(&V::Bool(true), &V::Int(1))
                && !eq(&V::Int(1), &V::Float(1.5))
        );
        assert!(
            eq(&V::Big("1000000000000000019884624838656".into()), &V::Float(1e30))
                && !eq(&V::Big("1000000000000000000000000000000".into()), &V::Float(1e30))
        );
        assert!(
            !eq(&V::Float(f64::NAN), &V::Float(f64::NAN))
                && !eq(&V::Int(1), &V::from("1"))
                && eq(&V::None, &V::None)
                && !eq(&V::None, &V::Int(0))
        );
        let (a, b) = (dict! { "x" => crate::list![1, 2.0], "y" => "s" }, dict! { "y" => "s", "x" => crate::list![1.0, 2] });
        assert!(
            eq(&V::Dict(a.clone()), &V::Dict(b)) && !eq(&V::Dict(a), &V::Dict(dict! { "x" => crate::list![1, 2.5], "y" => "s" }))
        );
        assert!(eq_opt(&V::None, None) && !eq_opt(&V::Int(0), None) && eq_opt(&V::Int(0), Some(V::Bool(false))));
    }
}
