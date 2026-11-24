//! 内建函数和类型（len、sorted、int()、dict()……）。行为、出错的说法都照 CPython。

use std::rc::Rc;

use kit::py;

use super::ast::{BinOp, CmpOp};
use super::interp::{Interp, Kw};
use super::iter::{reversed, set_items};
use super::ops::{self, as_int, num, truthy, Num};
use super::pyfmt::{self, SET_ORDER};
use super::value::*;
use super::{jsonio, methods};

/// 名字 → 内建的东西（全局里没有这个名字时才看这里）
pub fn builtin(name: &str) -> Option<V> {
    Some(match name {
        "len" => V::Builtin(B::Len),
        "any" => V::Builtin(B::Any),
        "all" => V::Builtin(B::All),
        "sum" => V::Builtin(B::Sum),
        "min" => V::Builtin(B::Min),
        "max" => V::Builtin(B::Max),
        "sorted" => V::Builtin(B::Sorted),
        "next" => V::Builtin(B::Next),
        "iter" => V::Builtin(B::Iter),
        "repr" => V::Builtin(B::Repr),
        "abs" => V::Builtin(B::Abs),
        "round" => V::Builtin(B::Round),
        "isinstance" => V::Builtin(B::Isinstance),
        "divmod" => V::Builtin(B::Divmod),
        "chr" => V::Builtin(B::Chr),
        "ord" => V::Builtin(B::Ord),
        "print" => V::Builtin(B::Print),
        "open" => V::Builtin(B::Open),
        "exit" => V::Builtin(B::Exit),
        "str" => V::Type(Ty::Str),
        "int" => V::Type(Ty::Int),
        "float" => V::Type(Ty::Float),
        "bool" => V::Type(Ty::Bool),
        "list" => V::Type(Ty::List),
        "tuple" => V::Type(Ty::Tuple),
        "dict" => V::Type(Ty::Dict),
        "set" => V::Type(Ty::Set),
        "frozenset" => V::Type(Ty::FrozenSet),
        "type" => V::Type(Ty::Type),
        "range" => V::Type(Ty::Range),
        "map" => V::Type(Ty::Map),
        "filter" => V::Type(Ty::Filter),
        "zip" => V::Type(Ty::Zip),
        "enumerate" => V::Type(Ty::Enumerate),
        "reversed" => V::Type(Ty::Reversed),
        "json" => V::Module(Mod::Json),
        "sys" => V::Module(Mod::Sys),
        "time" => V::Module(Mod::Time),
        _ => return None,
    })
}

// ---- 参数 ----

pub fn take_kw(kw: &mut Kw, name: &str) -> Option<V> {
    let i = kw.iter().position(|(k, _)| &**k == name)?;
    Some(kw.remove(i).1)
}

/// 还剩着没人认的关键字参数：是 Python 有、这里没做的，还是写错了，分不清，不猜
pub fn no_kw(name: &str, kw: &Kw) -> R<()> {
    match kw.first() {
        None => Ok(()),
        Some((k, _)) => unsupported(format!("{name}() 的关键字参数 {k}")),
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// 正好一个参数（len(x) 这一类）
fn one(name: &str, args: Vec<V>) -> R<V> {
    let n = args.len();
    match (args.into_iter().next(), n) {
        (Some(v), 1) => Ok(v),
        _ => type_err(format!("{name}() takes exactly one argument ({n} given)")),
    }
}

/// 参数个数在 min 到 max 之间
pub fn arity(name: &str, args: &[V], min: usize, max: usize) -> R<()> {
    let n = args.len();
    if n < min {
        if min == max {
            return type_err(format!("{name} expected {min} argument{}, got {n}", plural(min)));
        }
        return type_err(format!("{name} expected at least {min} argument{}, got {n}", plural(min)));
    }
    if n > max {
        if min == max {
            return type_err(format!("{name} expected {max} argument{}, got {n}", plural(max)));
        }
        return type_err(format!("{name} expected at most {max} argument{}, got {n}", plural(max)));
    }
    Ok(())
}

/// 要一个整数（bool 也算）
/// Python 写的、只要一个位置参数的函数（json.load、json.loads）：个数不对时的说法
fn py_one(name: &str, param: &str, args: &[V]) -> R<()> {
    match args.len() {
        1 => Ok(()),
        0 => type_err(format!("{name}() missing 1 required positional argument: '{param}'")),
        n => type_err(format!("{name}() takes 1 positional argument but {n} were given")),
    }
}

pub fn index_arg(v: &V) -> R<i128> {
    match as_int(v) {
        Some(n) => Ok(n),
        None => type_err(format!("'{}' object cannot be interpreted as an integer", type_name(v))),
    }
}

// ---- 调用 ----

pub fn call(it: &mut Interp, b: B, args: Vec<V>, mut kw: Kw) -> R<V> {
    match b {
        B::Len => {
            no_kw("len", &kw)?;
            let v = one("len", args)?;
            let n = match &v {
                V::Str(s) => ops::char_len(s),
                V::List(l) => l.borrow().len(),
                V::Tuple(t) => t.len(),
                V::Dict(d) => d.borrow().len(),
                V::Set(s) => s.d.borrow().len(),
                V::View(w) => w.d.borrow().len(),
                V::Range(r) => match r.len() {
                    // len() 的结果要放得进 C 的 ssize_t
                    Some(n) if n > i128::from(i64::MAX) => {
                        return raise(Kind::Overflow, "Python int too large to convert to C ssize_t");
                    }
                    Some(n) => return Ok(V::Int(n)),
                    None => return unsupported("range 太大（超过 128 位）"),
                },
                _ => return type_err(format!("object of type '{}' has no len()", type_name(&v))),
            };
            Ok(V::Int(n as i128))
        }
        B::Any | B::All => {
            let name = b.name();
            no_kw(name, &kw)?;
            let v = one(name, args)?;
            // any：碰到真的就是 True；all：碰到假的就是 False
            let stop_on = b == B::Any;
            if let V::Set(s) = &v {
                // 集合：结果和顺序无关，直接看
                let hit = s.d.borrow().entries().iter().any(|e| truthy(&e.k) == stop_on);
                return Ok(V::Bool(hit == stop_on));
            }
            let iter = it.iter_of(&v)?;
            while let Some(x) = it.next(&iter)? {
                if truthy(&x) == stop_on {
                    return Ok(V::Bool(stop_on));
                }
            }
            Ok(V::Bool(!stop_on))
        }
        B::Sum => sum(it, args, kw),
        B::Min | B::Max => min_max(it, b == B::Max, args, kw),
        B::Sorted => {
            let key = take_kw(&mut kw, "key").filter(|k| !matches!(k, V::None));
            // reverse 看真假（3.12 起什么对象都行）
            let reverse = take_kw(&mut kw, "reverse").is_some_and(|v| truthy(&v));
            no_kw("sorted", &kw)?;
            arity("sorted", &args, 1, 1)?;
            let items = match &args[0] {
                V::Set(s) if s.d.borrow().len() > 1 => {
                    // 集合里没有相等的两个元素：元素都是同一种能比大小的类型时，排出来的结果和原来的顺序无关
                    let items = set_items(s);
                    if key.is_some() || !uniform(&items) {
                        return unsupported(SET_ORDER);
                    }
                    items
                }
                other => it.collect(other)?,
            };
            Ok(V::list(sort_values(it, items, key.as_ref(), reverse)?))
        }
        B::Next => {
            no_kw("next", &kw)?;
            arity("next", &args, 1, 2)?;
            match it.next(&args[0])? {
                Some(v) => Ok(v),
                None => match args.get(1) {
                    Some(default) => Ok(default.clone()),
                    None => raise(Kind::StopIteration, ""),
                },
            }
        }
        B::Iter => {
            no_kw("iter", &kw)?;
            arity("iter", &args, 1, 2)?;
            if args.len() == 2 {
                return unsupported("iter(函数, 哨兵) 这种两个参数的写法");
            }
            it.iter_of(&args[0])
        }
        B::Repr => {
            no_kw("repr", &kw)?;
            Ok(V::str(pyfmt::repr(&one("repr", args)?)?))
        }
        B::Abs => {
            no_kw("abs", &kw)?;
            let v = one("abs", args)?;
            match num(&v) {
                Some(Num::I(n)) => match n.checked_abs() {
                    Some(a) => Ok(V::Int(a)),
                    None => unsupported("整数运算的结果超过 128 位"),
                },
                Some(Num::F(f)) => Ok(V::Float(f.abs())),
                None => type_err(format!("bad operand type for abs(): '{}'", type_name(&v))),
            }
        }
        B::Round => round(args, kw),
        B::Isinstance => {
            no_kw("isinstance", &kw)?;
            arity("isinstance", &args, 2, 2)?;
            Ok(V::Bool(isinstance(&args[0], &args[1])?))
        }
        B::Divmod => {
            no_kw("divmod", &kw)?;
            arity("divmod", &args, 2, 2)?;
            match (num(&args[0]), num(&args[1])) {
                (Some(Num::I(a)), Some(Num::I(b))) => {
                    Ok(V::tuple(vec![V::Int(ops::int_floor_div(a, b)?), V::Int(ops::int_mod(a, b)?)]))
                }
                (Some(a), Some(b)) => {
                    if b.f() == 0.0 {
                        return raise(Kind::ZeroDivision, "division by zero");
                    }
                    let (q, r) = ops::float_divmod(a.f(), b.f());
                    Ok(V::tuple(vec![V::Float(q), V::Float(r)]))
                }
                _ => type_err(format!(
                    "unsupported operand type(s) for divmod(): '{}' and '{}'",
                    type_name(&args[0]),
                    type_name(&args[1])
                )),
            }
        }
        B::Chr => {
            no_kw("chr", &kw)?;
            let v = one("chr", args)?;
            let n = index_arg(&v)?;
            match u32::try_from(n).ok().filter(|n| *n < 0x11_0000) {
                None => value_err("chr() arg not in range(0x110000)"),
                Some(n) => match char::from_u32(n) {
                    Some(c) => Ok(V::str(c.encode_utf8(&mut [0; 4]))),
                    None => unsupported("单独的代理项（U+D800 到 U+DFFF）这里不支持"),
                },
            }
        }
        B::Ord => {
            no_kw("ord", &kw)?;
            let v = one("ord", args)?;
            let V::Str(s) = &v else {
                return type_err(format!("ord() expected string of length 1, but {} found", type_name(&v)));
            };
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Ok(V::Int(i128::from(c as u32))),
                _ => type_err(format!("ord() expected a character, but string of length {} found", ops::char_len(s))),
            }
        }
        B::Print => {
            let text_kw = |v: Option<V>, what: &str, default: &str| -> R<String> {
                match v {
                    None | Some(V::None) => Ok(default.to_string()),
                    Some(V::Str(s)) => Ok(s.to_string()),
                    Some(o) => type_err(format!("{what} must be None or a string, not {}", type_name(&o))),
                }
            };
            let sep = text_kw(take_kw(&mut kw, "sep"), "sep", " ")?;
            let end = text_kw(take_kw(&mut kw, "end"), "end", "\n")?;
            let to = match take_kw(&mut kw, "file") {
                None | Some(V::None) | Some(V::Stream(Stream::Stdout)) => Stream::Stdout,
                Some(V::Stream(Stream::Stderr)) => Stream::Stderr,
                Some(_) => return unsupported("print 的 file= 只能是 sys.stdout 或者 sys.stderr"),
            };
            let flush = take_kw(&mut kw, "flush").is_some_and(|v| truthy(&v));
            no_kw("print", &kw)?;
            // 每个参数、每个分隔、结尾各写一次（Python 也是：前面的参数已经写了，后面的才转成字）
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    it.write(to, &sep)?;
                }
                it.write(to, &pyfmt::str_of(a)?)?;
            }
            it.write(to, &end)?;
            if flush {
                it.flush(to)?;
            }
            Ok(V::None)
        }
        B::Open => {
            let mode_kw = take_kw(&mut kw, "mode");
            let encoding = take_kw(&mut kw, "encoding");
            no_kw("open", &kw)?;
            if args.len() > 2 {
                return unsupported("open() 的第三个以后的参数");
            }
            arity("open", &args, 1, 2)?;
            let V::Str(path) = &args[0] else { return unsupported("open() 的路径只支持字符串") };
            match args.get(1).cloned().or(mode_kw) {
                None => {}
                Some(V::Str(m)) if matches!(&*m, "r" | "rt") => {}
                Some(_) => return unsupported("open() 只能读文本（mode 只支持 r）"),
            }
            match encoding {
                None | Some(V::None) => {}
                Some(V::Str(e)) if matches!(e.to_ascii_lowercase().replace('_', "-").as_str(), "utf-8" | "utf8") => {}
                Some(_) => return unsupported("open() 的 encoding 只支持 utf-8"),
            }
            jsonio::open_file(path)
        }
        B::Exit | B::SysExit => {
            no_kw("exit", &kw)?;
            if b == B::Exit && args.len() > 1 {
                // exit 是 site 模块里一个对象的 __call__
                return type_err(format!(
                    "Quitter.__call__() takes from 1 to 2 positional arguments but {} were given",
                    args.len() + 1
                ));
            }
            arity("exit", &args, 0, 1)?;
            // sys.exit(元组)：CPython 拿元组当异常的参数表——空的等于没给，一项的就是那一项（exit() 不这样）
            let code = match args.into_iter().next() {
                Some(V::Tuple(t)) if b == B::SysExit && t.len() < 2 => t.first().cloned(),
                other => other,
            };
            match code {
                None | Some(V::None) => Err(Stop::Exit(0)),
                Some(V::Bool(b)) => Err(Stop::Exit(i32::from(b))),
                Some(V::Int(n)) => Err(Stop::Exit((n & 0xff) as i32)),
                Some(other) => {
                    // 话写到 stderr，后面另写一个换行；写不出去 Python 也不管
                    it.say(&pyfmt::str_of(&other)?);
                    it.say("\n");
                    Err(Stop::Exit(1))
                }
            }
        }
        B::JsonLoad => {
            no_kw("json.load", &kw)?;
            py_one("load", "fp", &args)?;
            match &args[0] {
                V::File(f) => jsonio::parse_json(&jsonio::file_read(f)?),
                other => raise(Kind::Attribute, format!("'{}' object has no attribute 'read'", type_name(other))),
            }
        }
        B::JsonLoads => {
            no_kw("json.loads", &kw)?;
            py_one("loads", "s", &args)?;
            match &args[0] {
                V::Str(s) => jsonio::parse_json(s),
                other => type_err(format!("the JSON object must be str, bytes or bytearray, not {}", type_name(other))),
            }
        }
        B::JsonDumps => jsonio::dumps(args, kw),
        B::TimeTime => {
            no_kw("time.time", &kw)?;
            if !args.is_empty() {
                return type_err(format!("time.time() takes no arguments ({} given)", args.len()));
            }
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
            Ok(V::Float(now))
        }
    }
}

// ---- sum / min / max / sorted ----

/// sum()：照 CPython 的做法——整数先用机器整数累加；碰到小数以后带补偿地加（Neumaier），所以 sum([0.1] * 10) 是 1.0
fn sum(it: &mut Interp, args: Vec<V>, mut kw: Kw) -> R<V> {
    let start_kw = take_kw(&mut kw, "start");
    no_kw("sum", &kw)?;
    if args.is_empty() {
        return type_err("sum() takes at least 1 positional argument (0 given)");
    }
    if args.len() > 2 || (args.len() == 2 && start_kw.is_some()) {
        return type_err(format!("sum() takes at most 2 arguments ({} given)", args.len() + usize::from(start_kw.is_some())));
    }
    let start = args.get(1).cloned().or(start_kw).unwrap_or(V::Int(0));
    if matches!(start, V::Str(_)) {
        return type_err("sum() can't sum strings [use ''.join(seq) instead]");
    }
    if let V::Set(s) = &args[0] {
        if s.d.borrow().len() > 1 {
            // 集合：整数加起来和顺序无关；小数的和看先后，别的类型干脆不做
            let mut total = match start {
                V::Int(n) => n,
                _ => return unsupported(SET_ORDER),
            };
            for x in set_items(s) {
                total = match as_int(&x).and_then(|n| total.checked_add(n)) {
                    Some(t) => t,
                    None => return unsupported(SET_ORDER),
                };
            }
            return Ok(V::Int(total));
        }
    }
    let iter = it.iter_of(&args[0])?;
    let mut result = start;
    if let V::Int(n) = result {
        if let Ok(mut acc) = i64::try_from(n) {
            loop {
                let Some(item) = it.next(&iter)? else { return Ok(V::Int(i128::from(acc))) };
                if let Some(next) = as_int(&item).and_then(|b| i64::try_from(b).ok()).and_then(|b| acc.checked_add(b)) {
                    acc = next;
                    continue;
                }
                result = it.binop(BinOp::Add, &V::Int(i128::from(acc)), &item)?;
                break;
            }
        }
    }
    if let V::Float(mut f) = result {
        let mut c = 0.0f64;
        let settle = |f: f64, c: f64| if c != 0.0 && c.is_finite() { f + c } else { f };
        loop {
            let Some(item) = it.next(&iter)? else { return Ok(V::Float(settle(f, c))) };
            // 补偿求和（Neumaier）；3.14 起夹在里面的整数也这样加
            let x = match &item {
                V::Float(x) => *x,
                V::Int(n) => *n as f64,
                V::Bool(b) => f64::from(u8::from(*b)),
                _ => {
                    result = it.binop(BinOp::Add, &V::Float(settle(f, c)), &item)?;
                    break;
                }
            };
            let t = f + x;
            c += if f.abs() >= x.abs() { (f - t) + x } else { (x - t) + f };
            f = t;
        }
    }
    while let Some(item) = it.next(&iter)? {
        result = it.binop(BinOp::Add, &result, &item)?;
    }
    Ok(result)
}

/// 元素的“样子”：都是同一种样子的元素之间比大小不会出错
#[derive(PartialEq)]
enum Shape {
    Num,
    Str,
    Tuple(Vec<Shape>),
}

fn shape(v: &V) -> Option<Shape> {
    match v {
        V::Bool(_) | V::Int(_) => Some(Shape::Num),
        V::Float(f) if !f.is_nan() => Some(Shape::Num),
        V::Str(_) => Some(Shape::Str),
        V::Tuple(t) => t.iter().map(shape).collect::<Option<Vec<_>>>().map(Shape::Tuple),
        _ => None,
    }
}

fn uniform(items: &[V]) -> bool {
    let Some(first) = items.first().and_then(shape) else { return false };
    items.iter().all(|x| shape(x).as_ref() == Some(&first))
}

fn min_max(it: &mut Interp, is_max: bool, args: Vec<V>, mut kw: Kw) -> R<V> {
    let name = if is_max { "max" } else { "min" };
    let key = take_kw(&mut kw, "key").filter(|k| !matches!(k, V::None));
    let default = take_kw(&mut kw, "default");
    no_kw(name, &kw)?;
    let source = match args.len() {
        0 => return type_err(format!("{name} expected at least 1 argument, got 0")),
        1 => match &args[0] {
            V::Set(s) if s.d.borrow().len() > 1 => {
                let items = set_items(s);
                if key.is_some() || !uniform(&items) {
                    return unsupported(SET_ORDER);
                }
                V::tuple(items)
            }
            other => other.clone(),
        },
        _ => {
            if default.is_some() {
                return type_err(format!("Cannot specify a default for {name}() with multiple positional arguments"));
            }
            V::tuple(args)
        }
    };
    let iter = it.iter_of(&source)?;
    let op = if is_max { CmpOp::Gt } else { CmpOp::Lt };
    // 留着先碰到的那个：后来的要严格更大（更小）才换
    let mut best: Option<(V, V)> = None;
    while let Some(item) = it.next(&iter)? {
        let k = match &key {
            Some(f) => it.call1(f, item.clone())?,
            None => item.clone(),
        };
        let better = match &best {
            None => true,
            Some((_, best_key)) => ops::order(op, &k, best_key, false)?,
        };
        if better {
            best = Some((item, k));
        }
    }
    match (best, default) {
        (Some((item, _)), _) => Ok(item),
        (None, Some(d)) => Ok(d),
        (None, None) => value_err(format!("{name}() iterable argument is empty")),
    }
}

/// 排序：稳定，只用 <（和 Python 一样）；reverse 时相等的元素仍然保持原来的先后
pub fn sort_values(it: &mut Interp, items: Vec<V>, key: Option<&V>, reverse: bool) -> R<Vec<V>> {
    it.charge(items.len() as u64)?;
    // key 先都算出来（Python 也是）
    let mut pairs: Vec<(V, V)> = Vec::with_capacity(items.len());
    for x in items {
        let k = match key {
            Some(f) => it.call1(f, x.clone())?,
            None => x.clone(),
        };
        pairs.push((k, x));
    }
    if reverse {
        pairs.reverse();
    }
    let mut sorted = merge_sort(pairs)?;
    if reverse {
        sorted.reverse();
    }
    Ok(sorted.into_iter().map(|(_, x)| x).collect())
}

fn merge_sort(mut v: Vec<(V, V)>) -> R<Vec<(V, V)>> {
    if v.len() <= 1 {
        return Ok(v);
    }
    let right = v.split_off(v.len() / 2);
    let (left, right) = (merge_sort(v)?, merge_sort(right)?);
    let mut out = Vec::with_capacity(left.len() + right.len());
    let (mut l, mut r) = (left.into_iter().peekable(), right.into_iter().peekable());
    loop {
        // 右边的严格小于左边的才先出：相等时左边的在前（稳定）
        let take_right = match (l.peek(), r.peek()) {
            (Some(a), Some(b)) => ops::order(CmpOp::Lt, &b.0, &a.0, true)?,
            (Some(_), None) => false,
            (None, Some(_)) => true,
            (None, None) => return Ok(out),
        };
        out.extend(if take_right { r.next() } else { l.next() });
    }
}

// ---- round / isinstance ----

fn round(args: Vec<V>, mut kw: Kw) -> R<V> {
    let nd_kw = take_kw(&mut kw, "ndigits");
    no_kw("round", &kw)?;
    if args.is_empty() {
        return type_err("round() missing required argument 'number' (pos 1)");
    }
    if args.len() + usize::from(nd_kw.is_some()) > 2 {
        return type_err(format!("round() takes at most 2 arguments ({} given)", args.len() + usize::from(nd_kw.is_some())));
    }
    let digits = match args.get(1).cloned().or(nd_kw) {
        None | Some(V::None) => None,
        Some(v) => Some(index_arg(&v)?),
    };
    match (&args[0], digits) {
        (V::Int(_) | V::Bool(_), d) => {
            let n = as_int(&args[0]).unwrap_or(0);
            match d {
                Some(d) if d < 0 => round_int(n, d.unsigned_abs()),
                _ => Ok(V::Int(n)),
            }
        }
        (V::Float(f), None) => Ok(V::Int(ops::float_to_int(f.round_ties_even())?)),
        (V::Float(f), Some(d)) => round_float(*f, d).map(V::Float),
        (other, _) => type_err(format!("type {} doesn't define __round__ method", type_name(other))),
    }
}

/// 整数舍入到 10 的 k 次方的倍数；恰好一半时取偶数
fn round_int(n: i128, k: u128) -> R<V> {
    let Some(m) = u32::try_from(k).ok().and_then(|k| 10i128.checked_pow(k)) else { return Ok(V::Int(0)) };
    let (q, r) = (ops::int_floor_div(n, m)?, ops::int_mod(n, m)?);
    let up = r * 2 > m || (r * 2 == m && q % 2 != 0);
    match (q + i128::from(up)).checked_mul(m) {
        Some(v) => Ok(V::Int(v)),
        None => unsupported("整数运算的结果超过 128 位"),
    }
}

/// round(小数, 位数)：照 CPython——先按十进制正确舍入成那么多位（恰好一半时取偶数），再变回小数
fn round_float(x: f64, d: i128) -> R<f64> {
    if !x.is_finite() || x == 0.0 || d > 323 {
        return Ok(x);
    }
    if d >= 0 {
        return Ok(format!("{:.*}", d as usize, x).parse().unwrap_or(x));
    }
    if d < -308 {
        return Ok(0.0 * x);
    }
    let a = x.abs();
    if a >= 1.7e38 {
        return round_big(a, d.unsigned_abs() as usize).map(|r| if x < 0.0 { -r } else { r });
    }
    let (whole, frac) = (a.trunc() as u128, a.fract());
    let rounded = match 10u128.checked_pow(d.unsigned_abs() as u32) {
        // 10 的倍数比它还大得多：只能是 0
        None => 0,
        Some(m) => {
            let (q, r) = (whole / m, whole % m);
            let half = m / 2;
            let up = r > half || (r == half && (frac > 0.0 || q % 2 == 1));
            (q + u128::from(up)) * m
        }
    };
    let r = rounded as f64;
    if r.is_infinite() {
        return raise(Kind::Overflow, "rounded value too large to represent");
    }
    Ok(if x < 0.0 { -r } else { r })
}

/// 很大的小数（都是整数）舍入到 10 的 k 次方的倍数：在十进制的写法上做，恰好一半时取偶数
fn round_big(a: f64, k: usize) -> R<f64> {
    // {:.0} 写出来的是这个小数准确的值
    let digits = format!("{a:.0}");
    if k > digits.len() {
        return Ok(0.0);
    }
    let (head, tail) = digits.split_at(digits.len() - k);
    let half = format!("5{}", "0".repeat(k.saturating_sub(1)));
    // 一样长的两串数字，按字典序比就是按大小比
    let up = match tail.cmp(half.as_str()) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Equal => head.bytes().last().is_some_and(|b| (b - b'0') % 2 == 1),
        std::cmp::Ordering::Less => false,
    };
    let mut head: Vec<u8> = head.bytes().collect();
    if up {
        let mut i = head.len();
        loop {
            if i == 0 {
                head.insert(0, b'1');
                break;
            }
            i -= 1;
            if head[i] == b'9' {
                head[i] = b'0';
            } else {
                head[i] += 1;
                break;
            }
        }
    }
    let text = format!("{}{}", String::from_utf8_lossy(&head), "0".repeat(k));
    match text.parse::<f64>() {
        Ok(r) if r.is_finite() => Ok(r),
        _ => raise(Kind::Overflow, "rounded value too large to represent"),
    }
}

fn isinstance(v: &V, t: &V) -> R<bool> {
    match t {
        V::Type(ty) => Ok(methods::is_instance(v, *ty)),
        V::Tuple(items) => {
            for x in items.iter() {
                if isinstance(v, x)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        _ => type_err("isinstance() arg 2 must be a type, a tuple of types, or a union"),
    }
}

// ---- 类型当函数用 ----

pub fn construct(it: &mut Interp, ty: Ty, args: Vec<V>, mut kw: Kw) -> R<V> {
    let name = ty.name();
    match ty {
        Ty::Str => {
            no_kw(name, &kw)?;
            match args.len() {
                0 => Ok(V::str("")),
                1 => match &args[0] {
                    V::Str(_) => Ok(args[0].clone()),
                    other => Ok(V::str(pyfmt::str_of(other)?)),
                },
                _ => unsupported("str(字节串, 编码) 这种写法"),
            }
        }
        Ty::Int => {
            let base = take_kw(&mut kw, "base");
            no_kw(name, &kw)?;
            if args.len() > 2 || (args.len() == 2 && base.is_some()) {
                return type_err(format!("int expected at most 2 arguments, got {}", args.len() + usize::from(base.is_some())));
            }
            let base = args.get(1).cloned().or(base);
            match (args.first(), base) {
                (None, None) => Ok(V::Int(0)),
                (None, Some(_)) => type_err("int() missing string argument"),
                (Some(v), None) => match v {
                    V::Int(_) => Ok(v.clone()),
                    V::Bool(b) => Ok(V::Int(i128::from(*b))),
                    V::Float(f) => Ok(V::Int(ops::float_to_int(*f)?)),
                    V::Str(s) => parse_int(s, 10).map(V::Int),
                    other => type_err(format!(
                        "int() argument must be a string, a bytes-like object or a real number, not '{}'",
                        type_name(other)
                    )),
                },
                (Some(v), Some(b)) => {
                    let b = index_arg(&b)?;
                    let V::Str(s) = v else { return type_err("int() can't convert non-string with explicit base") };
                    if b != 0 && !(2..=36).contains(&b) {
                        return value_err("int() base must be >= 2 and <= 36, or 0");
                    }
                    parse_int(s, b as u32).map(V::Int)
                }
            }
        }
        Ty::Float => {
            no_kw(name, &kw)?;
            arity(name, &args, 0, 1)?;
            match args.first() {
                None => Ok(V::Float(0.0)),
                Some(V::Str(s)) => parse_float(s).map(V::Float),
                Some(v) => match num(v) {
                    Some(x) => Ok(V::Float(x.f())),
                    None => type_err(format!("float() argument must be a string or a real number, not '{}'", type_name(v))),
                },
            }
        }
        Ty::Bool => {
            no_kw(name, &kw)?;
            arity(name, &args, 0, 1)?;
            Ok(V::Bool(args.first().is_some_and(truthy)))
        }
        Ty::List => {
            no_kw(name, &kw)?;
            arity(name, &args, 0, 1)?;
            match args.first() {
                None => Ok(V::list(Vec::new())),
                Some(v) => Ok(V::list(it.collect(v)?)),
            }
        }
        Ty::Tuple => {
            no_kw(name, &kw)?;
            arity(name, &args, 0, 1)?;
            match args.first() {
                None => Ok(V::tuple(Vec::new())),
                Some(v @ V::Tuple(_)) => Ok(v.clone()),
                Some(v) => Ok(V::tuple(it.collect(v)?)),
            }
        }
        Ty::Dict => {
            arity(name, &args, 0, 1)?;
            let d = Rc::new(std::cell::RefCell::new(Dict::new()));
            if let Some(v) = args.first() {
                methods::dict_update(it, &d, v)?;
            }
            let mut dd = d.borrow_mut();
            for (k, v) in kw {
                dd.set(Key::Str(k.clone()), V::Str(k), v);
            }
            drop(dd);
            Ok(V::Dict(d))
        }
        Ty::Set | Ty::FrozenSet => {
            no_kw(name, &kw)?;
            arity(name, &args, 0, 1)?;
            let frozen = ty == Ty::FrozenSet;
            let mut d = Dict::new();
            match args.first() {
                None => {}
                Some(v @ V::Set(s)) => {
                    if frozen && s.frozen {
                        return Ok(v.clone());
                    }
                    d = s.d.borrow().copy();
                }
                Some(v) => {
                    for x in it.collect(v)? {
                        d.add(ops::set_elem(&x)?, x, V::None);
                    }
                }
            }
            Ok(V::set(d, frozen))
        }
        Ty::Type => {
            no_kw(name, &kw)?;
            match args.len() {
                // exit 在 Python 里是 site 模块的一个对象，不是内建函数
                1 if matches!(args[0], V::Builtin(B::Exit)) => unsupported("type(exit)"),
                1 => Ok(V::Type(ty_of(&args[0]))),
                3 => unsupported("type(名字, 基类, 属性) 这种造类的写法"),
                _ => type_err("type() takes 1 or 3 arguments"),
            }
        }
        Ty::Range => {
            no_kw(name, &kw)?;
            arity(name, &args, 1, 3)?;
            let nums = args.iter().map(index_arg).collect::<R<Vec<i128>>>()?;
            let (start, stop, step) = match nums[..] {
                [stop] => (0, stop, 1),
                [start, stop] => (start, stop, 1),
                [start, stop, step] => (start, stop, step),
                _ => (0, 0, 1),
            };
            if step == 0 {
                return value_err("range() arg 3 must not be zero");
            }
            Ok(V::Range(Rc::new(RangeV { start, stop, step })))
        }
        Ty::Map => {
            no_kw(name, &kw)?;
            if args.len() < 2 {
                return type_err("map() must have at least two arguments.");
            }
            let its = args[1..].iter().map(|v| it.iter_of(v)).collect::<R<Vec<V>>>()?;
            Ok(V::iter(Ty::Map, IterState::Map(args[0].clone(), its)))
        }
        Ty::Filter => {
            no_kw(name, &kw)?;
            arity(name, &args, 2, 2)?;
            let source = it.iter_of(&args[1])?;
            Ok(V::iter(Ty::Filter, IterState::Filter(args[0].clone(), source)))
        }
        Ty::Zip => {
            no_kw(name, &kw)?;
            let mut its = Vec::with_capacity(args.len());
            for v in &args {
                its.push(it.iter_of(v)?);
            }
            Ok(V::iter(Ty::Zip, IterState::Zip(its)))
        }
        Ty::Enumerate => {
            let start_kw = take_kw(&mut kw, "start");
            no_kw(name, &kw)?;
            if args.is_empty() {
                return type_err("enumerate() missing required argument 'iterable'");
            }
            arity(name, &args, 1, 2)?;
            let start = match args.get(1).cloned().or(start_kw) {
                Some(v) => index_arg(&v)?,
                None => 0,
            };
            let source = it.iter_of(&args[0])?;
            Ok(V::iter(Ty::Enumerate, IterState::Enumerate(source, start)))
        }
        Ty::Reversed => {
            no_kw(name, &kw)?;
            arity(name, &args, 1, 1)?;
            reversed(&args[0])
        }
        _ => unsupported(format!("把 {name} 这个类型当函数调用")),
    }
}

/// int(字符串, 进制)：照 Python 的写法（两头的空白、正负号、数字中间的下划线、进制前缀）
fn parse_int(text: &str, base: u32) -> R<i128> {
    let invalid = || -> R<i128> { value_err(format!("invalid literal for int() with base {base}: {}", py::repr_str(text))) };
    let s = py::strip(text);
    if !s.is_ascii() {
        // 别的文字里的数字（全角、阿拉伯文数字……）Python 也认，这里没做
        return if s.chars().any(|c| !c.is_ascii() && c.is_numeric()) {
            unsupported("int() 读非 ASCII 的数字")
        } else {
            invalid()
        };
    }
    let (negative, body) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let lower = body.to_ascii_lowercase();
    let prefixed = |p: &str| lower.strip_prefix(p).map(str::to_string);
    let (radix, digits) = match base {
        0 => match (prefixed("0x"), prefixed("0o"), prefixed("0b")) {
            (Some(d), _, _) => (16, d),
            (_, Some(d), _) => (8, d),
            (_, _, Some(d)) => (2, d),
            // 没有前缀就是十进制，而且不能以 0 开头（除非全是 0）
            _ if lower.len() > 1 && lower.starts_with('0') && lower.bytes().any(|b| b != b'0' && b != b'_') => return invalid(),
            _ => (10, lower.clone()),
        },
        16 => (16, prefixed("0x").unwrap_or_else(|| lower.clone())),
        8 => (8, prefixed("0o").unwrap_or_else(|| lower.clone())),
        2 => (2, prefixed("0b").unwrap_or_else(|| lower.clone())),
        b => (b, lower.clone()),
    };
    // 前缀后面可以紧跟一个下划线；数字之间只能夹单个下划线
    let had_prefix = digits.len() != lower.len();
    let digits = if had_prefix { digits.strip_prefix('_').unwrap_or(&digits).to_string() } else { digits };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return invalid();
    }
    let plain: String = digits.chars().filter(|c| *c != '_').collect();
    if !plain.chars().all(|c| c.is_digit(radix)) {
        return invalid();
    }
    match i128::from_str_radix(&plain, radix) {
        Ok(n) => Ok(if negative { -n } else { n }),
        Err(_) => unsupported("int() 读出来的整数超过 128 位"),
    }
}

/// float(字符串)
fn parse_float(text: &str) -> R<f64> {
    let invalid = || -> R<f64> { value_err(format!("could not convert string to float: {}", py::repr_str(text))) };
    let s = py::strip(text);
    if !s.is_ascii() {
        return if s.chars().any(|c| !c.is_ascii() && c.is_numeric()) {
            unsupported("float() 读非 ASCII 的数字")
        } else {
            invalid()
        };
    }
    let (negative, body) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let lower = body.to_ascii_lowercase();
    let value = match lower.as_str() {
        "inf" | "infinity" => f64::INFINITY,
        "nan" => f64::NAN,
        _ => {
            // 数字 [. 数字] [e [+-] 数字]，小数点两边至少一边有数字；数字之间可以夹单个下划线
            let (mantissa, exponent) = match lower.split_once('e') {
                Some((m, e)) => (m, Some(e.strip_prefix(['+', '-']).unwrap_or(e))),
                None => (lower.as_str(), None),
            };
            let (whole, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
            let digits = |part: &str| {
                !part.starts_with('_')
                    && !part.ends_with('_')
                    && !part.contains("__")
                    && part.bytes().all(|b| b.is_ascii_digit() || b == b'_')
            };
            let ok = digits(whole)
                && digits(frac)
                && !(whole.is_empty() && frac.is_empty())
                && exponent.is_none_or(|e| !e.is_empty() && digits(e));
            if !ok {
                return invalid();
            }
            match lower.replace('_', "").parse::<f64>() {
                Ok(f) => f,
                Err(_) => return invalid(),
            }
        }
    };
    Ok(if negative { -value } else { value })
}
