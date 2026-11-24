//! Python 的 repr()、str() 和 % 格式化：写出来要和 Python 一个字不差。

use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::OnceLock;

use kit::json::float_repr;

use super::ops::{as_int, float_to_int, num, MAX_DATA_DEPTH};
use super::value::*;

pub const SET_ORDER: &str =
    "把两个以上元素的集合的顺序露出来的用法（打印、转成列表、遍历）：顺序由 CPython 的散列表决定，这里不模仿，先 sorted()";

/// str(x)：字符串原样，别的和 repr 一样
pub fn str_of(v: &V) -> R<String> {
    match v {
        V::Str(s) => Ok(s.to_string()),
        _ => repr(v),
    }
}

pub fn repr(v: &V) -> R<String> {
    let mut out = String::new();
    write_repr(&mut out, v, &mut Vec::new())?;
    Ok(out)
}

fn write_items(out: &mut String, items: &[V], seen: &mut Vec<usize>) -> R<()> {
    for (i, x) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_repr(out, x, seen)?;
    }
    Ok(())
}

/// seen：正在写的那几层容器，碰到自己套自己的写成 [...]
fn write_repr(out: &mut String, v: &V, seen: &mut Vec<usize>) -> R<()> {
    if seen.len() > MAX_DATA_DEPTH {
        return unsupported("数据嵌套太深");
    }
    match v {
        V::None => out.push_str("None"),
        V::Bool(true) => out.push_str("True"),
        V::Bool(false) => out.push_str("False"),
        V::Int(n) => {
            let _ = write!(out, "{n}");
        }
        V::Float(f) => out.push_str(&float_repr(*f)),
        V::Str(s) => repr_str(out, s),
        V::List(l) => {
            let id = Rc::as_ptr(l) as usize;
            if seen.contains(&id) {
                out.push_str("[...]");
                return Ok(());
            }
            seen.push(id);
            out.push('[');
            write_items(out, &l.borrow(), seen)?;
            out.push(']');
            seen.pop();
        }
        V::Tuple(t) => {
            let id = Rc::as_ptr(t) as usize;
            if seen.contains(&id) {
                out.push_str("(...)");
                return Ok(());
            }
            seen.push(id);
            out.push('(');
            write_items(out, t, seen)?;
            if t.len() == 1 {
                out.push(',');
            }
            out.push(')');
            seen.pop();
        }
        V::Dict(d) => {
            let id = Rc::as_ptr(d) as usize;
            if seen.contains(&id) {
                out.push_str("{...}");
                return Ok(());
            }
            seen.push(id);
            out.push('{');
            for (i, e) in d.borrow().entries().iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_repr(out, &e.k, seen)?;
                out.push_str(": ");
                write_repr(out, &e.v, seen)?;
            }
            out.push('}');
            seen.pop();
        }
        V::Set(s) => {
            let d = s.d.borrow();
            if d.len() > 1 {
                return unsupported(SET_ORDER);
            }
            match (s.frozen, d.entries().first()) {
                (false, None) => out.push_str("set()"),
                (true, None) => out.push_str("frozenset()"),
                (frozen, Some(e)) => {
                    out.push_str(if frozen { "frozenset({" } else { "{" });
                    write_repr(out, &e.k, seen)?;
                    out.push_str(if frozen { "})" } else { "}" });
                }
            }
        }
        V::Range(r) => {
            let _ = if r.step == 1 {
                write!(out, "range({}, {})", r.start, r.stop)
            } else {
                write!(out, "range({}, {}, {})", r.start, r.stop, r.step)
            };
        }
        V::Type(t) => {
            let _ = match t {
                Ty::File => write!(out, "<class '_io.TextIOWrapper'>"),
                _ => write!(out, "<class '{}'>", t.name()),
            };
        }
        V::View(w) => {
            let id = Rc::as_ptr(w) as usize;
            if seen.contains(&id) {
                out.push_str("...");
                return Ok(());
            }
            seen.push(id);
            let d = w.d.borrow();
            out.push_str(match w.kind {
                ViewKind::Keys => "dict_keys([",
                ViewKind::Values => "dict_values([",
                ViewKind::Items => "dict_items([",
            });
            for (i, e) in d.entries().iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                match w.kind {
                    ViewKind::Keys => write_repr(out, &e.k, seen)?,
                    ViewKind::Values => write_repr(out, &e.v, seen)?,
                    ViewKind::Items => {
                        out.push('(');
                        write_repr(out, &e.k, seen)?;
                        out.push_str(", ");
                        write_repr(out, &e.v, seen)?;
                        out.push(')');
                    }
                }
            }
            out.push_str("])");
            seen.pop();
        }
        V::Func(_) | V::Builtin(_) | V::Bound(_) | V::Unbound(..) | V::Iter(_) | V::Module(_) | V::File(_) | V::Stream(_) => {
            return unsupported(format!("{} 对象的 repr / str（Python 写出来的带内存地址）", type_name(v)));
        }
    }
    Ok(())
}

/// repr(字符串)：有单引号、没有双引号时用双引号括，别的都用单引号
pub fn repr_str(out: &mut String, s: &str) {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            ' '..='~' => out.push(c),
            c if printable(c) => out.push(c),
            c => {
                let n = c as u32;
                let _ = if n < 0x100 {
                    write!(out, "\\x{n:02x}")
                } else if n < 0x10000 {
                    write!(out, "\\u{n:04x}")
                } else {
                    write!(out, "\\U{n:08x}")
                };
            }
        }
    }
    out.push(quote);
}

/// 一个字符属于这类 Unicode 字符吗（表用到才编）
fn in_class(cell: &'static OnceLock<regex::Regex>, pattern: &'static str, c: char) -> bool {
    let re = cell.get_or_init(|| regex::Regex::new(pattern).expect("写死的正则"));
    re.is_match(c.encode_utf8(&mut [0; 4]))
}

/// str.isprintable 那个意思：Unicode 的 Other（控制符、格式符、没分配的……）和 Separator 两大类不可打印（空格除外）
fn printable(c: char) -> bool {
    let n = c as u32;
    if n < 0x7f {
        return n >= 0x20;
    }
    // 常见的几段直接认，不动用 Unicode 的表
    let known = matches!(n,
        0xa1..=0xac | 0xae..=0x24f | 0x391..=0x3a1 | 0x3a3..=0x3c9 | 0x410..=0x44f | 0x2010..=0x2027 | 0x2030..=0x205e
        | 0x2190..=0x22ff | 0x2500..=0x257f | 0x25a0..=0x26ff | 0x3001..=0x303f | 0x3041..=0x3096 | 0x30a0..=0x30ff
        | 0x4e00..=0x9fff | 0xac00..=0xd7a3 | 0xff01..=0xff5e);
    if known {
        return true;
    }
    static OTHER: OnceLock<regex::Regex> = OnceLock::new();
    !in_class(&OTHER, r"^[\p{C}\p{Z}]$", c)
}

/// str.isspace 认的空白：Unicode 的空白，加上 Python 另算的四个分隔控制符
pub fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// 字母（Unicode 的 L 大类：str.isalpha）
pub fn is_letter(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphabetic();
    }
    static LETTER: OnceLock<regex::Regex> = OnceLock::new();
    in_class(&LETTER, r"^\p{L}$", c)
}

/// 十进制数字（Nd）
pub fn is_decimal(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_digit();
    }
    static DECIMAL: OnceLock<regex::Regex> = OnceLock::new();
    in_class(&DECIMAL, r"^\p{Nd}$", c)
}

/// 别的数字类字符（Nl、No：上标、分数、罗马数字……）：算不算 isdigit 要看 Unicode 的另一张表
pub fn is_other_number(c: char) -> bool {
    if c.is_ascii() {
        return false;
    }
    static NUMBER: OnceLock<regex::Regex> = OnceLock::new();
    in_class(&NUMBER, r"^[\p{Nl}\p{No}]$", c)
}

// ---- % 格式化 ----

#[derive(Default)]
struct Spec {
    left: bool,
    plus: bool,
    space: bool,
    alt: bool,
    zero: bool,
    width: usize,
    prec: Option<usize>,
}

struct Args {
    items: Vec<V>,
    used: usize,
}

impl Args {
    fn take(&mut self) -> R<V> {
        match self.items.get(self.used) {
            Some(v) => {
                self.used += 1;
                Ok(v.clone())
            }
            None => type_err("not enough arguments for format string"),
        }
    }
    /// 宽度、精度写成 * 时从参数里拿
    fn star(&mut self) -> R<i128> {
        match as_int(&self.take()?) {
            Some(n) => Ok(n),
            None => type_err("* wants int"),
        }
    }
}

const MAX_FIELD: usize = 100_000;

fn field_size(n: u128) -> R<usize> {
    match usize::try_from(n) {
        Ok(n) if n <= MAX_FIELD => Ok(n),
        _ => unsupported("% 格式化的宽度或者精度太大"),
    }
}

fn digits(chars: &[char], i: &mut usize) -> R<Option<usize>> {
    let start = *i;
    let mut n: u128 = 0;
    while let Some(d) = chars.get(*i).and_then(|c| c.to_digit(10)) {
        n = n.saturating_mul(10).saturating_add(u128::from(d));
        *i += 1;
    }
    if *i == start {
        return Ok(None);
    }
    field_size(n).map(Some)
}

/// 格式串 % 参数：照 CPython 的 PyUnicode_Format
pub fn percent(fmt: &str, arg: &V) -> R<String> {
    let mut args = Args {
        items: match arg {
            V::Tuple(t) => t.to_vec(),
            other => vec![other.clone()],
        },
        used: 0,
    };
    // 右边是有下标的对象（列表、字典、range）时，没用上也不算错
    let lenient = matches!(arg, V::List(_) | V::Dict(_) | V::Range(_));
    let chars: Vec<char> = fmt.chars().collect();
    let mut out = String::with_capacity(fmt.len() + 16);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        if c != '%' {
            out.push(c);
            continue;
        }
        // 紧挨着的两个 % 才是一个百分号；中间夹了宽度之类的（%5%）在 Python 里是不认识的格式字符
        if chars.get(i) == Some(&'%') {
            out.push('%');
            i += 1;
            continue;
        }
        if chars.get(i) == Some(&'(') {
            return unsupported("% 格式化里的 %(名字)s");
        }
        let mut sp = Spec::default();
        while let Some(&f) = chars.get(i) {
            match f {
                '-' => sp.left = true,
                '+' => sp.plus = true,
                ' ' => sp.space = true,
                '#' => sp.alt = true,
                '0' => sp.zero = true,
                _ => break,
            }
            i += 1;
        }
        if chars.get(i) == Some(&'*') {
            i += 1;
            let w = args.star()?;
            sp.left |= w < 0;
            sp.width = field_size(w.unsigned_abs())?;
        } else if let Some(n) = digits(&chars, &mut i)? {
            sp.width = n;
        }
        if chars.get(i) == Some(&'.') {
            i += 1;
            if chars.get(i) == Some(&'*') {
                i += 1;
                sp.prec = Some(field_size(args.star()?.max(0).unsigned_abs())?);
            } else {
                sp.prec = Some(digits(&chars, &mut i)?.unwrap_or(0));
            }
        }
        if matches!(chars.get(i), Some('h' | 'l' | 'L')) {
            i += 1;
        }
        let Some(&conv) = chars.get(i) else { return value_err("incomplete format") };
        i += 1;
        let v = args.take()?;
        match conv {
            's' | 'r' => {
                let mut text = if conv == 's' { str_of(&v)? } else { repr(&v)? };
                if let Some(p) = sp.prec {
                    if let Some((at, _)) = text.char_indices().nth(p) {
                        text.truncate(at);
                    }
                }
                pad(&mut out, "", "", &text, &sp, false);
            }
            'a' | 'c' => return unsupported(format!("% 格式化里的 %{conv}")),
            'd' | 'i' | 'u' => {
                let n = match &v {
                    V::Float(f) => float_to_int(*f)?,
                    _ => match as_int(&v) {
                        Some(n) => n,
                        None => return type_err(format!("%{conv} format: a real number is required, not {}", type_name(&v))),
                    },
                };
                let body = min_digits(n.unsigned_abs().to_string(), sp.prec);
                pad(&mut out, sign(n < 0, &sp), "", &body, &sp, true);
            }
            'x' | 'X' | 'o' => {
                let Some(n) = as_int(&v) else {
                    return type_err(format!("%{conv} format: an integer is required, not {}", type_name(&v)));
                };
                let a = n.unsigned_abs();
                let (text, prefix) = match conv {
                    'x' => (format!("{a:x}"), "0x"),
                    'X' => (format!("{a:X}"), "0X"),
                    _ => (format!("{a:o}"), "0o"),
                };
                pad(&mut out, sign(n < 0, &sp), if sp.alt { prefix } else { "" }, &min_digits(text, sp.prec), &sp, true);
            }
            'e' | 'E' | 'f' | 'F' | 'g' | 'G' => {
                let Some(x) = num(&v) else { return type_err(format!("must be real number, not {}", type_name(&v))) };
                let f = x.f();
                let body = fmt_float(f.abs(), conv, sp.prec, sp.alt)?;
                pad(&mut out, sign(f.is_sign_negative() && !f.is_nan(), &sp), "", &body, &sp, true);
            }
            other => {
                return value_err(format!("unsupported format character '{other}' (0x{:x}) at index {}", other as u32, i - 1));
            }
        }
    }
    if args.used < args.items.len() && !lenient {
        return type_err("not all arguments converted during string formatting");
    }
    Ok(out)
}

fn sign(negative: bool, sp: &Spec) -> &'static str {
    if negative {
        "-"
    } else if sp.plus {
        "+"
    } else if sp.space {
        " "
    } else {
        ""
    }
}

/// 整数的精度：至少这么多位，不够的前面补 0
fn min_digits(text: String, prec: Option<usize>) -> String {
    match prec {
        Some(p) if text.len() < p => format!("{}{text}", "0".repeat(p - text.len())),
        _ => text,
    }
}

fn pad(out: &mut String, sign: &str, prefix: &str, body: &str, sp: &Spec, numeric: bool) {
    let len = sign.len() + prefix.len() + body.chars().count();
    let fill = sp.width.saturating_sub(len);
    let spaces = |out: &mut String, c: char| out.extend(std::iter::repeat_n(c, fill));
    if sp.left {
        out.push_str(sign);
        out.push_str(prefix);
        out.push_str(body);
        spaces(out, ' ');
    } else if sp.zero && numeric {
        out.push_str(sign);
        out.push_str(prefix);
        spaces(out, '0');
        out.push_str(body);
    } else {
        spaces(out, ' ');
        out.push_str(sign);
        out.push_str(prefix);
        out.push_str(body);
    }
}

fn strip_zeros(s: &mut String) {
    if s.contains('.') {
        let keep = s.trim_end_matches('0').trim_end_matches('.').len();
        s.truncate(keep);
    }
}

/// 指数写法：尾数、e、符号、至少两位指数
fn exp_form(mantissa: &str, exp: i32) -> String {
    format!("{mantissa}e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
}

/// 不带符号的小数写成 %e / %f / %g 的样子
fn fmt_float(a: f64, conv: char, prec: Option<usize>, alt: bool) -> R<String> {
    let upper = conv.is_ascii_uppercase();
    let done = |s: String| Ok(if upper { s.to_ascii_uppercase() } else { s });
    if !a.is_finite() {
        return done(if a.is_nan() { "nan" } else { "inf" }.to_string());
    }
    let p = prec.unwrap_or(6);
    if p > 1000 {
        return unsupported("% 格式化的精度太大");
    }
    // Rust 的 {:e} 给的是 "1.5e-7" 这种：拆成尾数和指数
    let sci = |digits: usize| -> (String, i32) {
        let s = format!("{a:.digits$e}");
        match s.split_once('e') {
            Some((m, e)) => (m.to_string(), e.parse().unwrap_or(0)),
            None => (s, 0),
        }
    };
    let point = |s: &mut String| {
        if alt && !s.contains('.') {
            s.push('.');
        }
    };
    match conv.to_ascii_lowercase() {
        'f' => {
            let mut s = format!("{a:.p$}");
            point(&mut s);
            done(s)
        }
        'e' => {
            let (mut m, x) = sci(p);
            point(&mut m);
            done(exp_form(&m, x))
        }
        _ => {
            // %g：照指数写法舍入后的指数 X 决定用哪一种；末尾的 0 去掉（带 # 的留着）
            let p = p.max(1);
            let (mut m, x) = sci(p - 1);
            if -4 <= x && i64::from(x) < p as i64 {
                let mut s = format!("{:.*}", (p as i64 - 1 - i64::from(x)) as usize, a);
                if alt {
                    point(&mut s);
                } else {
                    strip_zeros(&mut s);
                }
                return done(s);
            }
            if alt {
                point(&mut m);
            } else {
                strip_zeros(&mut m);
            }
            done(exp_form(&m, x))
        }
    }
}
