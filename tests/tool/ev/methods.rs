//! 属性和方法：对象.名字 是什么、各种类型的方法。没实现的 Python 方法是退出码 2；Python 也没有的名字是 AttributeError。

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use super::ast::BinOp;
use super::builtins::{arity, index_arg, no_kw, sort_values, take_kw};
use super::interp::{Interp, Kw};
use super::ops::{self, char_len, dict_key, eq_item, set_elem};
use super::pyfmt::{self, is_decimal, is_letter, is_other_number, py_space};
use super::value::*;
use super::{jsonio, names};

const STR: &[&str] = &[
    "capitalize",
    "count",
    "endswith",
    "find",
    "format",
    "index",
    "isalnum",
    "isalpha",
    "isdigit",
    "isspace",
    "join",
    "lower",
    "lstrip",
    "replace",
    "rfind",
    "rsplit",
    "rstrip",
    "split",
    "splitlines",
    "startswith",
    "strip",
    "title",
    "upper",
    "zfill",
];
const LIST: &[&str] = &["append", "clear", "copy", "count", "extend", "index", "insert", "pop", "remove", "reverse", "sort"];
const DICT: &[&str] = &["clear", "copy", "get", "items", "keys", "pop", "popitem", "setdefault", "update", "values"];
const SET: &[&str] = &[
    "add",
    "clear",
    "copy",
    "difference",
    "discard",
    "intersection",
    "isdisjoint",
    "issubset",
    "issuperset",
    "pop",
    "remove",
    "symmetric_difference",
    "union",
    "update",
];
const FROZENSET: &[&str] =
    &["copy", "difference", "intersection", "isdisjoint", "issubset", "issuperset", "symmetric_difference", "union"];
const TUPLE: &[&str] = &["count", "index"];
const FILE: &[&str] = &["close", "read", "readline", "readlines"];
const VIEW: &[&str] = &["isdisjoint"];
/// 不是方法的属性
const VALUES: &[&str] = &["__name__", "start", "stop", "step", "stdout", "stderr"];
/// 模块里有的
const MODULE: &[&str] = &["loads", "load", "dumps", "exit", "time"];

/// 这个类型上实现了的方法、Python 里它有的全部属性名
fn methods_of(ty: Ty) -> (&'static [&'static str], &'static [&'static str]) {
    match ty {
        Ty::Str => (STR, names::STR),
        Ty::Int | Ty::Bool => (&[], names::INT),
        Ty::Float => (&[], names::FLOAT),
        Ty::List => (LIST, names::LIST),
        Ty::Tuple => (TUPLE, names::TUPLE),
        Ty::Dict => (DICT, names::DICT),
        Ty::Set => (SET, names::SET),
        Ty::FrozenSet => (FROZENSET, names::FROZENSET),
        Ty::Range => (&[], names::RANGE),
        Ty::Type => (&[], names::TYPE),
        Ty::DictKeys | Ty::DictItems => (VIEW, names::DICT_KEYS),
        Ty::DictValues => (&[], names::DICT_VALUES),
        Ty::Generator => (&[], names::GENERATOR),
        Ty::File => (FILE, names::FILE),
        _ => (&[], &[]),
    }
}

/// syntax 子命令用：这个名字是不是哪种类型上支持的方法或者属性
pub fn known_attr(name: &str) -> bool {
    [STR, LIST, DICT, SET, FROZENSET, TUPLE, FILE, VIEW, VALUES, MODULE].iter().any(|t| t.contains(&name))
}

/// 方法认的关键字参数（syntax 子命令用）；None：这个方法什么关键字参数都认（dict.update）
pub fn method_keywords(name: &str) -> Option<&'static [&'static str]> {
    Some(match name {
        "sort" => &["key", "reverse"],
        "split" | "rsplit" => &["sep", "maxsplit"],
        "splitlines" => &["keepends"],
        "replace" => &["count"],
        "dumps" => &["indent", "ensure_ascii", "sort_keys", "separators"],
        "update" => return None,
        _ => &[],
    })
}

pub fn is_instance(v: &V, ty: Ty) -> bool {
    ty_of(v) == ty || (ty == Ty::Int && matches!(v, V::Bool(_)))
}

pub enum Attr {
    /// 方法：名字换成了表里的那一个（调用时照它分派）
    Method(&'static str),
    Value(V),
}

/// 对象.名字 是什么
pub fn resolve(recv: &V, name: &str) -> R<Attr> {
    match recv {
        V::Module(m) => return module_attr(*m, name).map(Attr::Value),
        V::Type(t) => return type_attr(*t, name).map(Attr::Value),
        V::Range(r) => match name {
            "start" => return Ok(Attr::Value(V::Int(r.start))),
            "stop" => return Ok(Attr::Value(V::Int(r.stop))),
            "step" => return Ok(Attr::Value(V::Int(r.step))),
            _ => {}
        },
        _ => {}
    }
    let ty = ty_of(recv);
    let (have, python) = methods_of(ty);
    if let Some(m) = have.iter().find(|m| **m == name) {
        return Ok(Attr::Method(m));
    }
    if name.starts_with('_') {
        return unsupported(format!("属性 {name}（下划线开头的这里都不支持）"));
    }
    if python.contains(&name) {
        return unsupported(format!("{} 的 {name}（Python 有，这里没实现）", ty.name()));
    }
    raise(Kind::Attribute, format!("'{}' object has no attribute '{name}'", ty.label()))
}

/// 对象.名字 当值用（不是马上调用）：方法连着对象一起带走
pub fn get_attr(recv: &V, name: &str) -> R<V> {
    match resolve(recv, name)? {
        Attr::Value(v) => Ok(v),
        Attr::Method(m) => Ok(V::Bound(Rc::new(Bound { recv: recv.clone(), name: m }))),
    }
}

fn module_attr(m: Mod, name: &str) -> R<V> {
    Ok(match (m, name) {
        (Mod::Json, "loads") => V::Builtin(B::JsonLoads),
        (Mod::Json, "load") => V::Builtin(B::JsonLoad),
        (Mod::Json, "dumps") => V::Builtin(B::JsonDumps),
        (Mod::Sys, "exit") => V::Builtin(B::SysExit),
        (Mod::Sys, "stdout") => V::Stream(Stream::Stdout),
        (Mod::Sys, "stderr") => V::Stream(Stream::Stderr),
        (Mod::Time, "time") => V::Builtin(B::TimeTime),
        _ => {
            if name.starts_with('_') || module_names(m).contains(&name) {
                return unsupported(format!("{}.{name}（这里没有）", m.name()));
            }
            return raise(Kind::Attribute, format!("module '{}' has no attribute '{name}'", m.name()));
        }
    })
}

fn module_names(m: Mod) -> &'static [&'static str] {
    match m {
        Mod::Json => names::MOD_JSON,
        Mod::Sys => names::MOD_SYS,
        Mod::Time => names::MOD_TIME,
    }
}

/// syntax 子命令用：模块.名字 这里有没有
pub fn module_has(module: &str, name: &str) -> bool {
    matches!((module, name), ("json", "loads" | "load" | "dumps") | ("sys", "exit" | "stdout" | "stderr") | ("time", "time"))
}

fn type_attr(ty: Ty, name: &str) -> R<V> {
    if name == "__name__" {
        return Ok(V::str(ty.name()));
    }
    let (have, python) = methods_of(ty);
    if let Some(m) = have.iter().find(|m| **m == name) {
        return Ok(V::Unbound(ty, m));
    }
    // 类型自己的方法，还有所有类型都有的那几个（mro）
    if name.starts_with('_') || python.contains(&name) || names::TYPE.contains(&name) {
        return unsupported(format!("{}.{name}（这里没实现）", ty.name()));
    }
    raise(Kind::Attribute, format!("type object '{}' has no attribute '{name}'", ty.name()))
}

pub fn call(it: &mut Interp, recv: &V, name: &'static str, args: Vec<V>, kw: Kw) -> R<V> {
    match recv {
        V::Str(s) => str_method(it, s, name, args, kw),
        V::List(l) => list_method(it, l, name, args, kw),
        V::Dict(d) => dict_method(it, d, name, args, kw),
        V::Set(s) => set_method(it, recv, s, name, args, kw),
        V::Tuple(t) => {
            no_kw(name, &kw)?;
            match name {
                "count" => {
                    exact("tuple", "count", &args, 1)?;
                    count_of(t, &args[0])
                }
                _ => {
                    arity("index", &args, 1, 3)?;
                    if args.len() > 1 {
                        return unsupported("index() 的起止位置");
                    }
                    match position(t, &args[0])? {
                        Some(i) => Ok(V::Int(i as i128)),
                        None => value_err("tuple.index(x): x not in tuple"),
                    }
                }
            }
        }
        V::File(f) => {
            no_kw(name, &kw)?;
            if !args.is_empty() {
                return unsupported(format!("文件的 {name}() 带参数"));
            }
            match name {
                "read" => Ok(V::Str(jsonio::file_read(f)?)),
                "readline" => Ok(jsonio::file_line(f)?.unwrap_or_else(|| V::str(""))),
                "readlines" => {
                    let mut lines = Vec::new();
                    while let Some(line) = jsonio::file_line(f)? {
                        lines.push(line);
                    }
                    Ok(V::list(lines))
                }
                _ => {
                    *f.state.borrow_mut() = FileState::Closed;
                    Ok(V::None)
                }
            }
        }
        V::View(_) => {
            no_kw(name, &kw)?;
            exact(type_name(recv), name, &args, 1)?;
            let (mine, other) = (it.entries_of(recv)?, it.entries_of(&args[0])?);
            Ok(V::Bool(!mine.iter().any(|(k, _)| other.iter().any(|(o, _)| o == k))))
        }
        _ => unsupported(format!("{} 的方法 {name}", type_name(recv))),
    }
}

fn position(items: &[V], x: &V) -> R<Option<usize>> {
    for (i, item) in items.iter().enumerate() {
        if eq_item(item, x)? {
            return Ok(Some(i));
        }
    }
    Ok(None)
}

fn count_of(items: &[V], x: &V) -> R<V> {
    let mut n = 0i128;
    for item in items {
        if eq_item(item, x)? {
            n += 1;
        }
    }
    Ok(V::Int(n))
}

// ---- str ----

/// 要一个字符串的参数（find、count 这些的第一个参数）
fn str_arg<'a>(v: &'a V, name: &str) -> R<&'a str> {
    match v {
        V::Str(s) => Ok(s),
        V::None => type_err(format!("{name}() argument 1 must be str, not None")),
        other => type_err(format!("{name}() argument 1 must be str, not {}", type_name(other))),
    }
}

/// 字节位置换成第几个字符
fn char_index(s: &str, byte: usize) -> i128 {
    char_len(&s[..byte]) as i128
}

fn str_method(it: &mut Interp, s: &Rc<str>, name: &'static str, args: Vec<V>, mut kw: Kw) -> R<V> {
    let text: &str = s;
    match name {
        "startswith" | "endswith" => {
            no_kw(name, &kw)?;
            arity(name, &args, 1, 3)?;
            if args.len() > 1 {
                return unsupported(format!("{name}() 的起止位置"));
            }
            let test = |p: &str| if name == "startswith" { text.starts_with(p) } else { text.ends_with(p) };
            let bad = |v: &V| type_err(format!("{name} first arg must be str or a tuple of str, not {}", type_name(v)));
            match &args[0] {
                V::Str(p) => Ok(V::Bool(test(p))),
                V::Tuple(options) => {
                    for o in options.iter() {
                        match o {
                            V::Str(p) if test(p) => return Ok(V::Bool(true)),
                            V::Str(_) => {}
                            other => {
                                return type_err(format!("tuple for {name} must only contain str, not {}", type_name(other)));
                            }
                        }
                    }
                    Ok(V::Bool(false))
                }
                other => bad(other),
            }
        }
        "split" | "rsplit" => {
            let sep_kw = take_kw(&mut kw, "sep");
            let max_kw = take_kw(&mut kw, "maxsplit");
            no_kw(name, &kw)?;
            kw_arity(name, &args, 0, 2)?;
            let max = match args.get(1).cloned().or(max_kw) {
                Some(v) => index_arg(&v)?,
                None => -1,
            };
            let right = name == "rsplit";
            let parts: Vec<&str> = match args.first().cloned().or(sep_kw) {
                None | Some(V::None) => split_space(text, max, right),
                Some(V::Str(sep)) => {
                    if sep.is_empty() {
                        return value_err("empty separator");
                    }
                    let pieces = usize::try_from(max).ok().map(|m| m.saturating_add(1));
                    let found: Vec<&str> = match (pieces, right) {
                        (None, false) => text.split(&*sep).collect(),
                        // 从右边找起："aaa".rsplit("aa") 是 ['a', '']
                        (None, true) => {
                            let mut v: Vec<&str> = text.rsplit(&*sep).collect();
                            v.reverse();
                            v
                        }
                        (Some(n), false) => text.splitn(n, &*sep).collect(),
                        (Some(n), true) => {
                            let mut v: Vec<&str> = text.rsplitn(n, &*sep).collect();
                            v.reverse();
                            v
                        }
                    };
                    return Ok(V::list(found.into_iter().map(V::str).collect()));
                }
                Some(other) => return type_err(format!("must be str or None, not {}", type_name(&other))),
            };
            Ok(V::list(parts.into_iter().map(V::str).collect()))
        }
        "splitlines" => {
            let keep_kw = take_kw(&mut kw, "keepends");
            no_kw(name, &kw)?;
            kw_arity(name, &args, 0, 1)?;
            let keep = args.first().cloned().or(keep_kw).is_some_and(|v| ops::truthy(&v));
            Ok(V::list(splitlines(text, keep).into_iter().map(V::str).collect()))
        }
        "strip" | "lstrip" | "rstrip" => {
            no_kw(name, &kw)?;
            arity(name, &args, 0, 1)?;
            let (left, right) = (name != "rstrip", name != "lstrip");
            let chars: Option<Vec<char>> = match args.first() {
                None | Some(V::None) => None,
                Some(V::Str(c)) => Some(c.chars().collect()),
                Some(_) => return type_err(format!("{name} arg must be None or str")),
            };
            let strip = |c: char| match &chars {
                Some(set) => set.contains(&c),
                None => py_space(c),
            };
            let mut out = text;
            if left {
                out = out.trim_start_matches(strip);
            }
            if right {
                out = out.trim_end_matches(strip);
            }
            Ok(V::str(out))
        }
        "join" => {
            no_kw(name, &kw)?;
            exact("str", name, &args, 1)?;
            let items = match it.collect(&args[0]) {
                Ok(items) => items,
                Err(Stop::Raise(e)) if e.kind == Kind::Type && e.msg.ends_with("is not iterable") => {
                    return type_err("can only join an iterable");
                }
                Err(e) => return Err(e),
            };
            let mut out = String::new();
            for (i, item) in items.iter().enumerate() {
                let V::Str(piece) = item else {
                    return type_err(format!("sequence item {i}: expected str instance, {} found", type_name(item)));
                };
                if i > 0 {
                    out.push_str(text);
                }
                out.push_str(piece);
            }
            Ok(V::str(out))
        }
        "replace" => {
            let count_kw = take_kw(&mut kw, "count");
            no_kw(name, &kw)?;
            kw_arity(name, &args, 2, 3)?;
            let old = match &args[0] {
                V::Str(o) => o,
                other => return type_err(format!("replace() argument 1 must be str, not {}", type_name(other))),
            };
            let new = match &args[1] {
                V::Str(n) => n,
                other => return type_err(format!("replace() argument 2 must be str, not {}", type_name(other))),
            };
            let count = match args.get(2).cloned().or(count_kw) {
                Some(v) => index_arg(&v)?,
                None => -1,
            };
            Ok(V::str(match usize::try_from(count) {
                Ok(n) => text.replacen(&**old, new, n),
                Err(_) => text.replace(&**old, new),
            }))
        }
        "lower" => {
            none(name, &args, &kw)?;
            Ok(V::str(text.to_lowercase()))
        }
        "upper" => {
            none(name, &args, &kw)?;
            Ok(V::str(text.to_uppercase()))
        }
        "title" | "capitalize" => {
            none(name, &args, &kw)?;
            // title：前一个字符是字母（有大小写的）就写成小写，不然写成大写；capitalize：只有第一个字符大写
            let title = name == "title";
            let mut out = String::with_capacity(text.len());
            let mut prev_cased = false;
            for (i, c) in text.chars().enumerate() {
                let upper = if title { !prev_cased } else { i == 0 };
                if c.is_ascii_alphabetic() {
                    out.push(if upper { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() });
                    prev_cased = true;
                } else if c.is_ascii() || uncased(c) {
                    out.push(c);
                    prev_cased = false;
                } else {
                    return unsupported(format!("{name}() 碰到非 ASCII 的字母（Unicode 的大小写规则这里没全做）"));
                }
            }
            Ok(V::str(out))
        }
        "count" => {
            no_kw(name, &kw)?;
            arity(name, &args, 1, 3)?;
            if args.len() > 1 {
                return unsupported("count() 的起止位置");
            }
            let sub = str_arg(&args[0], name)?;
            let n = if sub.is_empty() { char_len(text) + 1 } else { text.matches(sub).count() };
            Ok(V::Int(n as i128))
        }
        "find" | "rfind" | "index" => {
            no_kw(name, &kw)?;
            arity(name, &args, 1, 3)?;
            if args.len() > 1 {
                return unsupported(format!("{name}() 的起止位置"));
            }
            let sub = str_arg(&args[0], name)?;
            let at = if name == "rfind" { text.rfind(sub) } else { text.find(sub) };
            match at {
                Some(byte) => Ok(V::Int(char_index(text, byte))),
                None if name == "index" => value_err("substring not found"),
                None => Ok(V::Int(-1)),
            }
        }
        "isdigit" | "isalpha" | "isalnum" | "isspace" => {
            none(name, &args, &kw)?;
            // 有一个字符肯定不是 → False；没有肯定不是的、但有说不准的（上标数字、罗马数字这类）→ 不猜
            let mut unsure = false;
            for c in text.chars() {
                let (yes, maybe) = match name {
                    "isdigit" => (is_decimal(c), is_other_number(c)),
                    "isalpha" => (is_letter(c), false),
                    "isalnum" => (is_letter(c) || is_decimal(c), is_other_number(c)),
                    _ => (py_space(c), false),
                };
                if maybe {
                    unsure = true;
                } else if !yes {
                    return Ok(V::Bool(false));
                }
            }
            if unsure {
                return unsupported(format!("{name}() 碰到上标、分数、罗马数字这类字符"));
            }
            Ok(V::Bool(!text.is_empty()))
        }
        "zfill" => {
            no_kw(name, &kw)?;
            exact("str", name, &args, 1)?;
            let width = index_arg(&args[0])?;
            let len = char_len(text) as i128;
            if width <= len {
                return Ok(V::Str(s.clone()));
            }
            if width > 10_000_000 {
                return unsupported("zfill() 的宽度太大");
            }
            let zeros = "0".repeat((width - len) as usize);
            Ok(V::str(match text.chars().next() {
                Some(sign @ ('+' | '-')) => format!("{sign}{zeros}{}", &text[1..]),
                _ => format!("{zeros}{text}"),
            }))
        }
        _ => {
            if let Some((k, _)) = kw.first() {
                return unsupported(format!("format() 的关键字参数 {k}（只支持 {{}} 和 {{0}} 这样按位置的）"));
            }
            format(text, &args).map(V::str)
        }
    }
}

/// 不带参数的方法
/// 正好要一个参数、不要参数的方法：个数不对时 CPython 的说法带着类型名
fn exact(ty: &str, name: &str, args: &[V], want: usize) -> R<()> {
    let n = args.len();
    match want {
        _ if n == want => Ok(()),
        0 => type_err(format!("{ty}.{name}() takes no arguments ({n} given)")),
        _ => type_err(format!("{ty}.{name}() takes exactly one argument ({n} given)")),
    }
}

/// 认关键字参数的方法（split、splitlines、replace）：个数不对时的说法
fn kw_arity(name: &str, args: &[V], min: usize, max: usize) -> R<()> {
    let n = args.len();
    let s = |k: usize| if k == 1 { "" } else { "s" };
    if n > max {
        return type_err(format!("{name}() takes at most {max} argument{} ({n} given)", s(max)));
    }
    if n < min {
        return type_err(format!("{name}() takes at least {min} positional argument{} ({n} given)", s(min)));
    }
    Ok(())
}

fn none(name: &str, args: &[V], kw: &Kw) -> R<()> {
    no_kw(name, kw)?;
    if !args.is_empty() {
        return type_err(format!("str.{name}() takes no arguments ({} given)", args.len()));
    }
    Ok(())
}

/// 没有大小写之分的字符（汉字、标点……）
fn uncased(c: char) -> bool {
    !c.is_lowercase() && !c.is_uppercase() && c.to_lowercase().eq(std::iter::once(c)) && c.to_uppercase().eq(std::iter::once(c))
}

/// split() 不带分隔符：按空白切，没有空串；切够了次数剩下的原样留着（只去掉前面的空白；rsplit 是后面的）
fn split_space(text: &str, max: i128, right: bool) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    let mut left = if max < 0 { usize::MAX } else { max as usize };
    loop {
        rest = if right { rest.trim_end_matches(py_space) } else { rest.trim_start_matches(py_space) };
        if rest.is_empty() {
            break;
        }
        if left == 0 {
            out.push(rest);
            break;
        }
        left -= 1;
        if right {
            let cut = rest.rfind(py_space).map(|i| i + rest[i..].chars().next().map_or(1, char::len_utf8)).unwrap_or(0);
            out.push(&rest[cut..]);
            rest = &rest[..cut];
        } else {
            let cut = rest.find(py_space).unwrap_or(rest.len());
            out.push(&rest[..cut]);
            rest = &rest[cut..];
        }
    }
    if right {
        out.reverse();
    }
    out
}

/// str.splitlines 认的换行比文件多：\v、\f、\x1c–\x1e、\x85、U+2028、U+2029 也算
fn splitlines(text: &str, keep: bool) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if !matches!(c, '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}') {
            continue;
        }
        let mut end = i + c.len_utf8();
        if c == '\r' {
            if let Some((j, '\n')) = chars.peek().copied() {
                chars.next();
                end = j + 1;
            }
        }
        out.push(&text[start..if keep { end } else { i }]);
        start = end;
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// str.format：只认 {} 和 {0} 这样按位置的
fn format(text: &str, args: &[V]) -> R<String> {
    let mut out = String::new();
    let mut auto = 0usize;
    // 自动编号（{}）和手写编号（{0}）不能混用
    let mut manual: Option<bool> = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '}' => return value_err("Single '}' encountered in format string"),
            '{' => {
                let mut field = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some('{') => return unsupported("format() 里套着的 { }"),
                        Some(ch) => field.push(ch),
                        None if field.is_empty() => return value_err("Single '{' encountered in format string"),
                        None => return value_err("expected '}' before end of string"),
                    }
                }
                let index = if field.is_empty() {
                    if manual == Some(true) {
                        return value_err("cannot switch from manual field specification to automatic field numbering");
                    }
                    manual = Some(false);
                    auto += 1;
                    auto - 1
                } else if field.bytes().all(|b| b.is_ascii_digit()) {
                    if manual == Some(false) {
                        return value_err("cannot switch from automatic field numbering to manual field specification");
                    }
                    manual = Some(true);
                    field.parse().unwrap_or(usize::MAX)
                } else {
                    return unsupported(format!("format() 的 {{{field}}}（只支持 {{}} 和 {{0}} 这样按位置的）"));
                };
                match args.get(index) {
                    Some(v) => out.push_str(&pyfmt::str_of(v)?),
                    None => return raise(Kind::Index, format!("Replacement index {index} out of range for positional args tuple")),
                }
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

// ---- list ----

/// 下标换成位置（insert 用：出界的收到两头）
fn clamp_index(i: i128, len: usize) -> usize {
    let len = len as i128;
    (if i < 0 { (i + len).max(0) } else { i.min(len) }) as usize
}

fn list_method(it: &mut Interp, l: &Rc<RefCell<Vec<V>>>, name: &'static str, args: Vec<V>, mut kw: Kw) -> R<V> {
    if name == "sort" {
        let key = take_kw(&mut kw, "key").filter(|k| !matches!(k, V::None));
        let reverse = take_kw(&mut kw, "reverse").is_some_and(|v| ops::truthy(&v));
        no_kw(name, &kw)?;
        if !args.is_empty() {
            return type_err("sort() takes no positional arguments");
        }
        // 排的时候列表是空的（CPython 也是）：key 函数里改它，排完会发现
        let items = std::mem::take(&mut *l.borrow_mut());
        let backup = items.clone();
        let sorted = match sort_values(it, items, key.as_ref(), reverse) {
            Ok(sorted) => sorted,
            Err(e) => {
                *l.borrow_mut() = backup;
                return Err(e);
            }
        };
        let mut guard = l.borrow_mut();
        let touched = !guard.is_empty();
        *guard = sorted;
        return if touched { value_err("list modified during sort") } else { Ok(V::None) };
    }
    no_kw(name, &kw)?;
    match name {
        "append" => {
            exact("list", name, &args, 1)?;
            l.borrow_mut().push(args[0].clone());
            Ok(V::None)
        }
        "extend" => {
            exact("list", name, &args, 1)?;
            let items = it.collect(&args[0])?;
            l.borrow_mut().extend(items);
            Ok(V::None)
        }
        "insert" => {
            arity(name, &args, 2, 2)?;
            let i = index_arg(&args[0])?;
            let mut guard = l.borrow_mut();
            let at = clamp_index(i, guard.len());
            guard.insert(at, args[1].clone());
            Ok(V::None)
        }
        "pop" => {
            arity(name, &args, 0, 1)?;
            let mut guard = l.borrow_mut();
            if guard.is_empty() {
                return raise(Kind::Index, "pop from empty list");
            }
            let len = guard.len() as i128;
            let i = match args.first() {
                Some(v) => index_arg(v)?,
                None => -1,
            };
            let at = if i < 0 { i + len } else { i };
            if at < 0 || at >= len {
                return raise(Kind::Index, "pop index out of range");
            }
            Ok(guard.remove(at as usize))
        }
        "remove" => {
            exact("list", name, &args, 1)?;
            let at = position(&l.borrow(), &args[0])?;
            match at {
                Some(i) => {
                    l.borrow_mut().remove(i);
                    Ok(V::None)
                }
                None => value_err("list.remove(x): x not in list"),
            }
        }
        "index" => {
            arity(name, &args, 1, 3)?;
            if args.len() > 1 {
                return unsupported("index() 的起止位置");
            }
            match position(&l.borrow(), &args[0])? {
                Some(i) => Ok(V::Int(i as i128)),
                None => value_err("list.index(x): x not in list"),
            }
        }
        "count" => {
            exact("list", name, &args, 1)?;
            count_of(&l.borrow(), &args[0])
        }
        "reverse" => {
            exact("list", name, &args, 0)?;
            l.borrow_mut().reverse();
            Ok(V::None)
        }
        "copy" => {
            exact("list", name, &args, 0)?;
            Ok(V::list(l.borrow().clone()))
        }
        _ => {
            exact("list", name, &args, 0)?;
            l.borrow_mut().clear();
            Ok(V::None)
        }
    }
}

// ---- dict ----

/// d.update(other)、dict(other)、d |= other：other 是字典，或者一串键值对
pub fn dict_update(it: &mut Interp, d: &Rc<RefCell<Dict>>, other: &V) -> R<()> {
    if let V::Dict(o) = other {
        if Rc::ptr_eq(d, o) {
            return Ok(());
        }
        let mut target = d.borrow_mut();
        for e in o.borrow().entries() {
            target.set(e.key.clone(), e.k.clone(), e.v.clone());
        }
        return Ok(());
    }
    let items = it.collect(other)?;
    for (i, item) in items.iter().enumerate() {
        let pair = match item {
            V::List(_) | V::Tuple(_) | V::Str(_) | V::Dict(_) | V::Set(_) | V::Range(_) | V::View(_) | V::Iter(_) => {
                it.collect(item)?
            }
            _ => return type_err(format!("cannot convert dictionary update sequence element #{i} to a sequence")),
        };
        let [k, v] = &pair[..] else {
            return value_err(format!("dictionary update sequence element #{i} has length {}; 2 is required", pair.len()));
        };
        d.borrow_mut().set(dict_key(k)?, k.clone(), v.clone());
    }
    Ok(())
}

fn view(d: &Rc<RefCell<Dict>>, kind: ViewKind) -> V {
    V::View(Rc::new(View { d: d.clone(), kind }))
}

fn dict_method(it: &mut Interp, d: &Rc<RefCell<Dict>>, name: &'static str, args: Vec<V>, kw: Kw) -> R<V> {
    if name == "update" {
        arity(name, &args, 0, 1)?;
        if let Some(other) = args.first() {
            dict_update(it, d, other)?;
        }
        let mut guard = d.borrow_mut();
        for (k, v) in kw {
            guard.set(Key::Str(k.clone()), V::Str(k), v);
        }
        return Ok(V::None);
    }
    no_kw(name, &kw)?;
    match name {
        "get" => {
            arity(name, &args, 1, 2)?;
            let key = dict_key(&args[0])?;
            Ok(d.borrow().get(&key).cloned().unwrap_or_else(|| args.get(1).cloned().unwrap_or(V::None)))
        }
        "keys" | "values" | "items" => {
            exact("dict", name, &args, 0)?;
            Ok(view(
                d,
                match name {
                    "keys" => ViewKind::Keys,
                    "values" => ViewKind::Values,
                    _ => ViewKind::Items,
                },
            ))
        }
        "pop" => {
            arity(name, &args, 1, 2)?;
            // 空字典：CPython 不算键的哈希就回去了（所以 {}.pop([1], 2) 是 2，不是 TypeError）
            let removed = if d.borrow().is_empty() {
                None
            } else {
                let key = dict_key(&args[0])?;
                d.borrow_mut().remove(&key)
            };
            match (removed, args.get(1)) {
                (Some(e), _) => Ok(e.v),
                (None, Some(default)) => Ok(default.clone()),
                (None, None) => raise(Kind::Key, pyfmt::repr(&args[0])?),
            }
        }
        "popitem" => {
            exact("dict", name, &args, 0)?;
            match d.borrow_mut().pop_last() {
                Some(e) => Ok(V::tuple(vec![e.k, e.v])),
                None => raise(Kind::Key, "'popitem(): dictionary is empty'"),
            }
        }
        "setdefault" => {
            arity(name, &args, 1, 2)?;
            let key = dict_key(&args[0])?;
            let mut guard = d.borrow_mut();
            if let Some(v) = guard.get(&key) {
                return Ok(v.clone());
            }
            let v = args.get(1).cloned().unwrap_or(V::None);
            guard.set(key, args[0].clone(), v.clone());
            Ok(v)
        }
        "copy" => {
            exact("dict", name, &args, 0)?;
            Ok(V::dict(d.borrow().copy()))
        }
        _ => {
            exact("dict", name, &args, 0)?;
            d.borrow_mut().clear();
            Ok(V::None)
        }
    }
}

// ---- set ----

fn set_method(it: &mut Interp, recv: &V, s: &Rc<SetObj>, name: &'static str, args: Vec<V>, kw: Kw) -> R<V> {
    no_kw(name, &kw)?;
    let kind = if s.frozen { "frozenset" } else { "set" };
    let same_kind = |d: Dict| V::set(d, s.frozen);
    match name {
        "add" => {
            exact(kind, name, &args, 1)?;
            s.d.borrow_mut().add(set_elem(&args[0])?, args[0].clone(), V::None);
            Ok(V::None)
        }
        "discard" | "remove" => {
            exact(kind, name, &args, 1)?;
            let key = ops::set_lookup_key(&args[0])?;
            match s.d.borrow_mut().remove(&key) {
                None if name == "remove" => raise(Kind::Key, pyfmt::repr(&args[0])?),
                _ => Ok(V::None),
            }
        }
        "pop" => {
            exact(kind, name, &args, 0)?;
            let mut guard = s.d.borrow_mut();
            match guard.len() {
                0 => raise(Kind::Key, "'pop from an empty set'"),
                1 => Ok(guard.pop_last().map(|e| e.k).unwrap_or(V::None)),
                _ => unsupported("从两个以上元素的集合里 pop（拿出来哪一个由 CPython 的散列表决定）"),
            }
        }
        "clear" => {
            exact(kind, name, &args, 0)?;
            s.d.borrow_mut().clear();
            Ok(V::None)
        }
        "copy" => {
            exact(kind, name, &args, 0)?;
            if s.frozen {
                return Ok(recv.clone());
            }
            Ok(same_kind(s.d.borrow().copy()))
        }
        "union" | "update" => {
            // 已经有的元素留着原来的那个
            let mut d = s.d.borrow().copy();
            for other in &args {
                for (key, v) in it.entries_of(other)? {
                    d.add(key, v, V::None);
                }
            }
            if name == "update" {
                s.d.borrow_mut().replace_with(d);
                return Ok(V::None);
            }
            Ok(same_kind(d))
        }
        "intersection" => {
            if args.is_empty() {
                return Ok(if s.frozen { recv.clone() } else { same_kind(s.d.borrow().copy()) });
            }
            let mut current = recv.clone();
            for other in &args {
                current = match other {
                    // 两个集合：和 & 一样（留哪一边的元素有讲究，见 ops::set_op）
                    V::Set(_) => {
                        let (a, b) = (it.entries_of(&current)?, it.entries_of(other)?);
                        ops::set_op(BinOp::BitAnd, &current, other, a, b)?
                    }
                    // 别的可迭代对象：留的是它里面的那个元素
                    _ => {
                        let V::Set(cur) = &current else { return unsupported("内部错误：交集的中间结果不是集合") };
                        let mut d = Dict::new();
                        for (key, v) in it.entries_of(other)? {
                            if cur.d.borrow().contains(&key) {
                                d.add(key, v, V::None);
                            }
                        }
                        same_kind(d)
                    }
                };
            }
            Ok(current)
        }
        "difference" => {
            let mut d = s.d.borrow().copy();
            for other in &args {
                for (key, _) in it.entries_of(other)? {
                    d.remove(&key);
                }
            }
            Ok(same_kind(d))
        }
        "symmetric_difference" => {
            exact(kind, name, &args, 1)?;
            let (a, b) = (it.entries_of(recv)?, it.entries_of(&args[0])?);
            // 另一边可能有重复的元素（列表）：先去重
            let mut seen = Dict::new();
            let b: Vec<(Key, V)> = b.into_iter().filter(|(k, v)| seen.add(k.clone(), v.clone(), V::None)).collect();
            ops::set_op(BinOp::BitXor, recv, recv, a, b)
        }
        "issubset" | "issuperset" | "isdisjoint" => {
            exact(kind, name, &args, 1)?;
            let other: HashSet<Key> = it.entries_of(&args[0])?.into_iter().map(|(k, _)| k).collect();
            let mine = s.d.borrow();
            Ok(V::Bool(match name {
                "issubset" => mine.entries().iter().all(|e| other.contains(&e.key)),
                "issuperset" => other.iter().all(|k| mine.contains(k)),
                _ => !other.iter().any(|k| mine.contains(k)),
            }))
        }
        _ => unsupported(format!("集合的方法 {name}")),
    }
}
