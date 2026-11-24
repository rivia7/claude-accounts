//! 值之间的运算：真假、相等、大小、散列、算术、下标和切片。都照 Python 的规矩；这里不跑用户的代码（不会回到求值器）。

use std::cmp::Ordering;
use std::collections::HashSet;
use std::rc::Rc;

use super::ast::{BinOp, CmpOp};
use super::pyfmt;
use super::value::*;

/// 数据能嵌套多少层（比较、打印是递归的）
pub const MAX_DATA_DEPTH: usize = 950;
const TWO127: f64 = 170141183460469231731687303715884105728.0;
const TWO53: i128 = 1 << 53;
/// 重复出来的字符串 / 列表最多这么长
const MAX_REPEAT: usize = 50_000_000;

pub fn truthy(v: &V) -> bool {
    match v {
        V::None => false,
        V::Bool(b) => *b,
        V::Int(n) => *n != 0,
        V::Float(f) => *f != 0.0,
        V::Str(s) => !s.is_empty(),
        V::List(l) => !l.borrow().is_empty(),
        V::Tuple(t) => !t.is_empty(),
        V::Dict(d) => !d.borrow().is_empty(),
        V::Set(s) => !s.d.borrow().is_empty(),
        V::Range(r) => r.len() != Some(0),
        V::View(v) => !v.d.borrow().is_empty(),
        _ => true,
    }
}

/// 数字：整数（bool 也是）或者小数
#[derive(Clone, Copy)]
pub enum Num {
    I(i128),
    F(f64),
}

impl Num {
    pub fn f(self) -> f64 {
        match self {
            Num::I(n) => n as f64,
            Num::F(f) => f,
        }
    }
}

pub fn num(v: &V) -> Option<Num> {
    match v {
        V::Bool(b) => Some(Num::I(i128::from(*b))),
        V::Int(n) => Some(Num::I(*n)),
        V::Float(f) => Some(Num::F(*f)),
        _ => None,
    }
}

/// 整数（bool 也算）
pub fn as_int(v: &V) -> Option<i128> {
    match v {
        V::Bool(b) => Some(i128::from(*b)),
        V::Int(n) => Some(*n),
        _ => None,
    }
}

/// 整数和小数比大小，不经过转换丢精度（2**53 + 1 和 float(2**53) 不相等）
pub fn cmp_int_float(a: i128, b: f64) -> Option<Ordering> {
    if b.is_nan() {
        return None;
    }
    if b >= TWO127 {
        return Some(Ordering::Less);
    }
    if b < -TWO127 {
        return Some(Ordering::Greater);
    }
    let t = b.trunc();
    let frac = if b > t {
        Ordering::Less
    } else if b < t {
        Ordering::Greater
    } else {
        Ordering::Equal
    };
    Some(a.cmp(&(t as i128)).then(frac))
}

pub fn num_cmp(a: Num, b: Num) -> Option<Ordering> {
    match (a, b) {
        (Num::I(x), Num::I(y)) => Some(x.cmp(&y)),
        (Num::F(x), Num::F(y)) => x.partial_cmp(&y),
        (Num::I(x), Num::F(y)) => cmp_int_float(x, y),
        (Num::F(x), Num::I(y)) => cmp_int_float(y, x).map(Ordering::reverse),
    }
}

/// int(小数)：往 0 取整
pub fn float_to_int(f: f64) -> R<i128> {
    if f.is_nan() {
        return value_err("cannot convert float NaN to integer");
    }
    if f.is_infinite() {
        return raise(Kind::Overflow, "cannot convert float infinity to integer");
    }
    if f.abs() >= TWO127 {
        return unsupported("小数转成的整数超过 128 位");
    }
    Ok(f.trunc() as i128)
}

// ---- 散列 ----

/// 当字典的键、集合的元素时的样子；列表、字典、集合不行（TypeError）
pub fn to_key(v: &V) -> R<Key> {
    to_key_at(v, 0)
}

fn to_key_at(v: &V, depth: usize) -> R<Key> {
    if depth > MAX_DATA_DEPTH {
        return unsupported("数据嵌套太深");
    }
    Ok(match v {
        V::None => Key::None,
        V::Bool(b) => Key::Int(i128::from(*b)),
        V::Int(n) => Key::Int(*n),
        V::Float(f) => {
            if f.is_nan() {
                return unsupported("NaN 当字典的键或者集合的元素（Python 看的是不是同一个对象，这里分不出来）");
            }
            if f.fract() == 0.0 && *f >= -TWO127 && *f < TWO127 {
                Key::Int(*f as i128)
            } else {
                Key::Float(f.to_bits())
            }
        }
        V::Str(s) => Key::Str(s.clone()),
        V::Tuple(t) => Key::Tuple(t.iter().map(|x| to_key_at(x, depth + 1)).collect::<R<Vec<_>>>()?),
        V::Set(s) if s.frozen => frozen_key(s),
        V::List(_) | V::Dict(_) | V::Set(_) => return type_err(format!("unhashable type: '{}'", type_name(v))),
        _ => return unsupported(format!("{} 当字典的键或者集合的元素", type_name(v))),
    })
}

fn frozen_key(s: &SetObj) -> Key {
    let mut keys: Vec<Key> = s.d.borrow().entries().iter().map(|e| e.key.clone()).collect();
    keys.sort();
    Key::Frozen(keys)
}

/// x in 集合：x 是可变的 set 时 Python 临时把它当 frozenset 找
pub fn set_lookup_key(v: &V) -> R<Key> {
    match v {
        V::Set(s) => Ok(frozen_key(s)),
        _ => set_elem(v),
    }
}

/// 不能哈希的东西拿去当字典的键、集合的元素：3.14 起的说法带着是哪一种用法、最外面那个值的类型
fn in_context(e: Stop, outer: &str, what: &str) -> Stop {
    match e {
        Stop::Raise(Exc { kind: Kind::Type, msg }) => {
            Stop::Raise(Exc { kind: Kind::Type, msg: format!("cannot use '{outer}' as a {what} ({msg})") })
        }
        other => other,
    }
}

/// 当字典的键
pub fn dict_key(v: &V) -> R<Key> {
    to_key(v).map_err(|e| in_context(e, type_name(v), "dict key"))
}

/// 当集合的元素
pub fn set_elem(v: &V) -> R<Key> {
    to_key(v).map_err(|e| in_context(e, type_name(v), "set element"))
}

// ---- 相等 ----

pub fn eq(a: &V, b: &V) -> R<bool> {
    eq_at(a, b, 0)
}

/// 容器里一项一项比（还有 in、index、count、remove）：Python 先看是不是同一个对象。
/// 两个 NaN 是不是同一个对象这里分不出来，不猜。
pub fn eq_item(a: &V, b: &V) -> R<bool> {
    eq_item_at(a, b, 0)
}

fn eq_item_at(a: &V, b: &V, depth: usize) -> R<bool> {
    if let (V::Float(x), V::Float(y)) = (a, b) {
        if x.is_nan() && y.is_nan() {
            return unsupported("容器里的 NaN 互相比较（Python 看的是不是同一个对象，这里分不出来）");
        }
    }
    eq_at(a, b, depth)
}

fn eq_at(a: &V, b: &V, depth: usize) -> R<bool> {
    if depth > MAX_DATA_DEPTH {
        return unsupported("数据嵌套太深");
    }
    Ok(match (a, b) {
        (V::None, V::None) => true,
        (V::Str(x), V::Str(y)) => x == y,
        (V::List(x), V::List(y)) => Rc::ptr_eq(x, y) || seq_eq(&x.borrow(), &y.borrow(), depth)?,
        (V::Tuple(x), V::Tuple(y)) => Rc::ptr_eq(x, y) || seq_eq(x, y, depth)?,
        (V::Dict(x), V::Dict(y)) => Rc::ptr_eq(x, y) || dict_eq(&x.borrow(), &y.borrow(), depth)?,
        (V::Set(x), V::Set(y)) => {
            Rc::ptr_eq(x, y) || {
                let (p, q) = (x.d.borrow(), y.d.borrow());
                p.len() == q.len() && p.entries().iter().all(|e| q.contains(&e.key))
            }
        }
        (V::View(x), V::View(y)) if Rc::ptr_eq(x, y) => true,
        // 至少一边是 keys() / items()：一样多、而且视图那边的每一项都在另一边里（两边都是视图时看左边的）。values() 只和自己相等
        (V::Set(_) | V::View(_), V::Set(_) | V::View(_)) => match (set_len(a), set_len(b)) {
            (Some(n), Some(m)) if n == m => {
                if matches!(a, V::View(_)) {
                    all_contained_in(a, b)?
                } else {
                    all_contained_in(b, a)?
                }
            }
            _ => false,
        },
        (V::Range(x), V::Range(y)) => {
            let (n, m) = (x.len(), y.len());
            n == m && (n == Some(0) || x.start == y.start && (n == Some(1) || x.step == y.step))
        }
        (V::Type(x), V::Type(y)) => x == y,
        (V::Builtin(x), V::Builtin(y)) => x == y,
        (V::Module(x), V::Module(y)) => x == y,
        (V::Stream(x), V::Stream(y)) => x == y,
        (V::Unbound(t, n), V::Unbound(u, m)) => t == u && n == m,
        (V::Func(x), V::Func(y)) => Rc::ptr_eq(x, y),
        (V::Iter(x), V::Iter(y)) => Rc::ptr_eq(x, y),
        (V::File(x), V::File(y)) => Rc::ptr_eq(x, y),
        (V::Bound(x), V::Bound(y)) => {
            if !Rc::ptr_eq(x, y) {
                return unsupported("两个方法对象之间的 ==");
            }
            true
        }
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => num_cmp(x, y) == Some(Ordering::Equal),
            _ => false,
        },
    })
}

fn seq_eq(a: &[V], b: &[V], depth: usize) -> R<bool> {
    if a.len() != b.len() {
        return Ok(false);
    }
    for (x, y) in a.iter().zip(b) {
        if !eq_item_at(x, y, depth + 1)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn dict_eq(a: &Dict, b: &Dict, depth: usize) -> R<bool> {
    if a.len() != b.len() {
        return Ok(false);
    }
    for e in a.entries() {
        match b.get(&e.key) {
            Some(v) if eq_item_at(&e.v, v, depth + 1)? => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

/// 集合那一类（set、frozenset、keys()、items()）有几个元素；别的类型（values() 也是）是 None
fn set_len(v: &V) -> Option<usize> {
    match v {
        V::Set(s) => Some(s.d.borrow().len()),
        V::View(w) if w.kind != ViewKind::Values => Some(w.d.borrow().len()),
        _ => None,
    }
}

/// a 的每个元素都在 b 里吗（照 CPython 的 all_contained_in：一个个拿出来问 in）
fn all_contained_in(a: &V, b: &V) -> R<bool> {
    if let (V::Set(x), V::Set(y)) = (a, b) {
        let (p, q) = (x.d.borrow(), y.d.borrow());
        return Ok(p.entries().iter().all(|e| q.contains(&e.key)));
    }
    let items: Vec<V> = match a {
        V::Set(s) => s.d.borrow().entries().iter().map(|e| e.k.clone()).collect(),
        V::View(w) => {
            let d = w.d.borrow();
            d.entries()
                .iter()
                .map(|e| match w.kind {
                    ViewKind::Keys => e.k.clone(),
                    _ => V::tuple(vec![e.k.clone(), e.v.clone()]),
                })
                .collect()
        }
        _ => return Ok(false),
    };
    for x in &items {
        if contains(b, x)? != Some(true) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// 集合那一类（set、frozenset、keys()、items()）的元素；别的类型是 None
pub fn set_entries(v: &V) -> R<Option<Vec<(Key, V)>>> {
    Ok(Some(match v {
        V::Set(s) => s.d.borrow().entries().iter().map(|e| (e.key.clone(), e.k.clone())).collect(),
        V::View(w) if w.kind == ViewKind::Keys => w.d.borrow().entries().iter().map(|e| (e.key.clone(), e.k.clone())).collect(),
        V::View(w) if w.kind == ViewKind::Items => {
            let d = w.d.borrow();
            let mut out = Vec::with_capacity(d.len());
            for e in d.entries() {
                // items() 的元素是（键, 值）这个元组：值不能哈希时说的是元组
                let value = to_key(&e.v).map_err(|err| in_context(err, "tuple", "set element"))?;
                out.push((Key::Tuple(vec![e.key.clone(), value]), V::tuple(vec![e.k.clone(), e.v.clone()])));
            }
            out
        }
        _ => return Ok(None),
    }))
}

// ---- 大小 ----

fn ord_is(op: CmpOp, o: Ordering) -> bool {
    match op {
        CmpOp::Lt => o == Ordering::Less,
        CmpOp::Le => o != Ordering::Greater,
        CmpOp::Gt => o == Ordering::Greater,
        CmpOp::Ge => o != Ordering::Less,
        _ => false,
    }
}

/// < <= > >=。sorting：给排序用时，结果会取决于 CPython 排序算法细节的比较（NaN、集合的子集关系）不做
pub fn order(op: CmpOp, a: &V, b: &V, sorting: bool) -> R<bool> {
    order_at(op, a, b, sorting, 0)
}

fn order_at(op: CmpOp, a: &V, b: &V, sorting: bool, depth: usize) -> R<bool> {
    if depth > MAX_DATA_DEPTH {
        return unsupported("数据嵌套太深");
    }
    if let (Some(x), Some(y)) = (num(a), num(b)) {
        return match num_cmp(x, y) {
            Some(o) => Ok(ord_is(op, o)),
            None if sorting => unsupported("NaN 参与排序（结果取决于 CPython 比较的先后）"),
            None => Ok(false),
        };
    }
    match (a, b) {
        (V::Str(x), V::Str(y)) => Ok(ord_is(op, x.as_bytes().cmp(y.as_bytes()))),
        (V::List(x), V::List(y)) => seq_order(op, &x.borrow(), &y.borrow(), sorting, depth),
        (V::Tuple(x), V::Tuple(y)) => seq_order(op, x, y, sorting, depth),
        (V::Set(_) | V::View(_), V::Set(_) | V::View(_)) => {
            let (Some(n), Some(m)) = (set_len(a), set_len(b)) else { return Err(not_ordered(op, a, b)) };
            if sorting {
                return unsupported("集合之间的 < 是子集关系，拿来排序的结果取决于 CPython 比较的先后");
            }
            Ok(match op {
                CmpOp::Le => n <= m && all_contained_in(a, b)?,
                CmpOp::Lt => n < m && all_contained_in(a, b)?,
                CmpOp::Ge => n >= m && all_contained_in(b, a)?,
                CmpOp::Gt => n > m && all_contained_in(b, a)?,
                _ => false,
            })
        }
        _ => Err(not_ordered(op, a, b)),
    }
}

fn not_ordered(op: CmpOp, a: &V, b: &V) -> Stop {
    Stop::Raise(Exc {
        kind: Kind::Type,
        msg: format!("'{}' not supported between instances of '{}' and '{}'", op.symbol(), type_name(a), type_name(b)),
    })
}

fn seq_order(op: CmpOp, a: &[V], b: &[V], sorting: bool, depth: usize) -> R<bool> {
    for (x, y) in a.iter().zip(b) {
        if !eq_item_at(x, y, depth + 1)? {
            return order_at(op, x, y, sorting, depth + 1);
        }
    }
    Ok(ord_is(op, a.len().cmp(&b.len())))
}

// ---- in ----

/// x in c，c 是字符串、列表、元组、字典、集合、视图、range；迭代器要遍历，由求值器自己做（这里回 None）
pub fn contains(c: &V, x: &V) -> R<Option<bool>> {
    Ok(Some(match c {
        V::Str(s) => match x {
            V::Str(sub) => s.contains(&**sub),
            _ => return type_err(format!("'in <string>' requires string as left operand, not {}", type_name(x))),
        },
        V::List(l) => seq_contains(&l.borrow(), x)?,
        V::Tuple(t) => seq_contains(t, x)?,
        V::Dict(d) => d.borrow().contains(&dict_key(x)?),
        V::Set(s) => s.d.borrow().contains(&set_lookup_key(x)?),
        V::View(w) => view_contains(w, x)?,
        V::Range(r) => range_contains(r, x),
        V::Iter(_) | V::File(_) => return Ok(None),
        _ => return type_err(format!("argument of type '{}' is not a container or iterable", type_name(c))),
    }))
}

fn seq_contains(items: &[V], x: &V) -> R<bool> {
    for it in items {
        if eq_item(it, x)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn view_contains(w: &View, x: &V) -> R<bool> {
    let d = w.d.borrow();
    match w.kind {
        ViewKind::Keys => Ok(d.contains(&dict_key(x)?)),
        ViewKind::Values => {
            for e in d.entries() {
                if eq_item(&e.v, x)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        ViewKind::Items => match x {
            V::Tuple(t) if t.len() == 2 => match d.get(&dict_key(&t[0])?) {
                Some(v) => eq_item(v, &t[1]),
                None => Ok(false),
            },
            _ => Ok(false),
        },
    }
}

fn range_contains(r: &RangeV, x: &V) -> bool {
    let n = match x {
        V::Bool(b) => i128::from(*b),
        V::Int(n) => *n,
        V::Float(f) if f.fract() == 0.0 && f.abs() < 1e30 => *f as i128,
        _ => return false,
    };
    if r.step > 0 {
        r.start <= n && n < r.stop && (n - r.start) % r.step == 0
    } else {
        r.stop < n && n <= r.start && (r.start - n) % r.step == 0
    }
}

// ---- is ----

/// a is b。两个整数、小数、字符串是不是同一个对象取决于 CPython 的实现（小整数缓存、字符串驻留），不猜
pub fn identical(a: &V, b: &V) -> R<bool> {
    Ok(match (a, b) {
        (V::None, V::None) => true,
        (V::Bool(x), V::Bool(y)) => x == y,
        (V::Int(_), V::Int(_)) | (V::Float(_), V::Float(_)) | (V::Str(_), V::Str(_)) => {
            return unsupported(format!("两个 {} 之间的 is（是不是同一个对象取决于 CPython 的实现，用 ==）", type_name(a)));
        }
        (V::List(x), V::List(y)) => Rc::ptr_eq(x, y),
        (V::Dict(x), V::Dict(y)) => Rc::ptr_eq(x, y),
        (V::Set(x), V::Set(y)) if !x.frozen && !y.frozen => Rc::ptr_eq(x, y),
        (V::Tuple(x), V::Tuple(y)) => {
            if !Rc::ptr_eq(x, y) {
                return unsupported("两个元组之间的 is（相等的元组是不是同一个对象取决于 CPython 的实现，用 ==）");
            }
            true
        }
        (V::Set(x), V::Set(y)) if x.frozen && y.frozen => {
            if !Rc::ptr_eq(x, y) {
                return unsupported("两个 frozenset 之间的 is（用 ==）");
            }
            true
        }
        (V::Range(x), V::Range(y)) => Rc::ptr_eq(x, y),
        (V::Func(x), V::Func(y)) => Rc::ptr_eq(x, y),
        (V::Iter(x), V::Iter(y)) => Rc::ptr_eq(x, y),
        (V::View(x), V::View(y)) => Rc::ptr_eq(x, y),
        (V::File(x), V::File(y)) => Rc::ptr_eq(x, y),
        (V::Bound(x), V::Bound(y)) => Rc::ptr_eq(x, y),
        (V::Type(x), V::Type(y)) => x == y,
        (V::Builtin(x), V::Builtin(y)) => x == y,
        (V::Module(x), V::Module(y)) => x == y,
        (V::Stream(x), V::Stream(y)) => x == y,
        (V::Unbound(..), V::Unbound(..)) => return unsupported("两个方法之间的 is"),
        _ => false,
    })
}

// ---- 算术 ----

fn too_big<T>() -> R<T> {
    unsupported("整数运算的结果超过 128 位（这里的整数最多 128 位）")
}

fn bad_operands(op: BinOp, a: &V, b: &V) -> Stop {
    let seq = |v: &V| matches!(v, V::Str(_) | V::List(_) | V::Tuple(_));
    let msg = match (op, a) {
        (BinOp::Add, V::Str(_)) => format!("can only concatenate str (not \"{}\") to str", type_name(b)),
        (BinOp::Add, V::List(_)) => format!("can only concatenate list (not \"{}\") to list", type_name(b)),
        (BinOp::Add, V::Tuple(_)) => format!("can only concatenate tuple (not \"{}\") to tuple", type_name(b)),
        // 序列 * 不是整数的：先看左边是不是序列（说右边的类型），再看右边
        (BinOp::Mul, _) if seq(a) => format!("can't multiply sequence by non-int of type '{}'", type_name(b)),
        (BinOp::Mul, _) if seq(b) => format!("can't multiply sequence by non-int of type '{}'", type_name(a)),
        (BinOp::Pow, _) => format!("unsupported operand type(s) for ** or pow(): '{}' and '{}'", type_name(a), type_name(b)),
        _ => format!("unsupported operand type(s) for {}: '{}' and '{}'", op.symbol(), type_name(a), type_name(b)),
    };
    Stop::Raise(Exc { kind: Kind::Type, msg })
}

pub fn binop(op: BinOp, a: &V, b: &V) -> R<V> {
    if let (Some(x), Some(y)) = (num(a), num(b)) {
        if let (V::Bool(p), V::Bool(q)) = (a, b) {
            // bool 之间的 & | ^ 还是 bool
            match op {
                BinOp::BitAnd => return Ok(V::Bool(p & q)),
                BinOp::BitOr => return Ok(V::Bool(p | q)),
                BinOp::BitXor => return Ok(V::Bool(p ^ q)),
                _ => {}
            }
        }
        return match (x, y) {
            (Num::I(p), Num::I(q)) => int_op(op, p, q),
            _ if matches!(op, BinOp::BitOr | BinOp::BitXor | BinOp::BitAnd) => Err(bad_operands(op, a, b)),
            _ => float_op(op, x.f(), y.f()),
        };
    }
    match (op, a, b) {
        (BinOp::Add, V::Str(x), V::Str(y)) => {
            let mut s = String::with_capacity(x.len() + y.len());
            s.push_str(x);
            s.push_str(y);
            Ok(V::str(s))
        }
        (BinOp::Add, V::List(x), V::List(y)) => {
            let mut items = x.borrow().clone();
            items.extend(y.borrow().iter().cloned());
            Ok(V::list(items))
        }
        (BinOp::Add, V::Tuple(x), V::Tuple(y)) => Ok(V::tuple(x.iter().chain(y.iter()).cloned().collect())),
        (BinOp::Mul, V::Str(_) | V::List(_) | V::Tuple(_), V::Int(_) | V::Bool(_)) => repeat(a, as_int(b).unwrap_or(0)),
        (BinOp::Mul, V::Int(_) | V::Bool(_), V::Str(_) | V::List(_) | V::Tuple(_)) => repeat(b, as_int(a).unwrap_or(0)),
        (BinOp::Mod, V::Str(f), _) => Ok(V::str(pyfmt::percent(f, b)?)),
        (BinOp::BitOr, V::Dict(x), V::Dict(y)) => {
            let mut d = x.borrow().copy();
            for e in y.borrow().entries() {
                d.set(e.key.clone(), e.k.clone(), e.v.clone());
            }
            Ok(V::dict(d))
        }
        (BinOp::BitOr, V::Type(_), V::Type(_) | V::None) | (BinOp::BitOr, V::None, V::Type(_)) => {
            unsupported("类型的联合（int | str 这种）")
        }
        (BinOp::BitOr | BinOp::BitAnd | BinOp::BitXor | BinOp::Sub, V::Set(_) | V::View(_), V::Set(_) | V::View(_)) => {
            match (set_entries(a)?, set_entries(b)?) {
                (Some(p), Some(q)) => set_op(op, a, b, p, q),
                _ => Err(bad_operands(op, a, b)),
            }
        }
        _ => Err(bad_operands(op, a, b)),
    }
}

/// 两个值“写出来一样”：1 和 1.0 和 True 相等但不一样。集合运算里要挑留哪一个的时候用
fn same_form(a: &V, b: &V) -> bool {
    match (a, b) {
        (V::None, V::None)
        | (V::Bool(_), V::Bool(_))
        | (V::Int(_), V::Int(_))
        | (V::Float(_), V::Float(_))
        | (V::Str(_), V::Str(_)) => true,
        (V::Tuple(x), V::Tuple(y)) => x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| same_form(p, q)),
        _ => false,
    }
}

/// 集合的 | & - ^。a、b 是两边的元素；left / right 是原来的值（看是不是 frozenset、是不是视图；视图的另一边可以是任何可迭代的）
pub fn set_op(op: BinOp, left: &V, right: &V, a: Vec<(Key, V)>, b: Vec<(Key, V)>) -> R<V> {
    let mut out = Dict::new();
    let has = |side: &[(Key, V)]| -> HashSet<Key> { side.iter().map(|(k, _)| k.clone()).collect() };
    match op {
        BinOp::BitOr => {
            for (k, v) in a.into_iter().chain(b) {
                out.add(k, v, V::None);
            }
        }
        BinOp::BitAnd => {
            // 两边都有的元素留哪一边的那个（1 和 1.0 是同一个元素）：两个 set 时 CPython 遍历小的那一边、一样大遍历右边；
            // 有视图掺进来时规则更绕，写出来不一样就不猜
            let plain = matches!((left, right), (V::Set(_), V::Set(_)));
            let from_left = b.len() > a.len();
            let in_a: std::collections::HashMap<&Key, &V> = a.iter().map(|(k, v)| (k, v)).collect();
            let mut picked = Vec::new();
            for (k, bv) in &b {
                if let Some(av) = in_a.get(k) {
                    if !plain && !same_form(av, bv) {
                        return unsupported("字典视图的 & 里两边相等、写出来却不一样的元素（1 和 1.0）");
                    }
                    picked.push((k.clone(), if from_left { (*av).clone() } else { bv.clone() }));
                }
            }
            for (k, v) in picked {
                out.add(k, v, V::None);
            }
        }
        BinOp::Sub => {
            let other = has(&b);
            for (k, v) in a {
                if !other.contains(&k) {
                    out.add(k, v, V::None);
                }
            }
        }
        _ => {
            let (in_a, in_b) = (has(&a), has(&b));
            for (k, v) in a {
                if !in_b.contains(&k) {
                    out.add(k, v, V::None);
                }
            }
            for (k, v) in b {
                if !in_a.contains(&k) {
                    out.add(k, v, V::None);
                }
            }
        }
    }
    // 有字典视图掺进来的结果总是 set；两个集合时跟左边的类型
    let frozen = matches!((left, right), (V::Set(s), V::Set(_)) if s.frozen);
    Ok(V::set(out, frozen))
}

fn repeat(seq: &V, n: i128) -> R<V> {
    let n = usize::try_from(n.max(0)).unwrap_or(usize::MAX);
    let check = |len: usize| if len.saturating_mul(n) > MAX_REPEAT { unsupported("重复出来的结果太大") } else { Ok(()) };
    match seq {
        V::Str(s) => {
            check(s.len())?;
            Ok(V::str(s.repeat(n)))
        }
        V::List(l) => {
            let l = l.borrow();
            check(l.len())?;
            Ok(V::list(l.iter().cloned().cycle().take(l.len() * n).collect()))
        }
        V::Tuple(t) => {
            check(t.len())?;
            Ok(V::tuple(t.iter().cloned().cycle().take(t.len() * n).collect()))
        }
        _ => unsupported("重复"),
    }
}

fn zero_div<T>() -> R<T> {
    raise(Kind::ZeroDivision, "division by zero")
}

pub fn int_floor_div(a: i128, b: i128) -> R<i128> {
    if b == 0 {
        return zero_div();
    }
    let Some(q) = a.checked_div(b) else { return too_big() };
    let r = a.wrapping_rem(b);
    Ok(if r != 0 && ((r < 0) != (b < 0)) { q - 1 } else { q })
}

pub fn int_mod(a: i128, b: i128) -> R<i128> {
    if b == 0 {
        return zero_div();
    }
    let r = a.wrapping_rem(b);
    Ok(if r != 0 && ((r < 0) != (b < 0)) { r + b } else { r })
}

fn int_op(op: BinOp, a: i128, b: i128) -> R<V> {
    let checked = |r: Option<i128>| match r {
        Some(n) => Ok(V::Int(n)),
        None => too_big(),
    };
    match op {
        BinOp::Add => checked(a.checked_add(b)),
        BinOp::Sub => checked(a.checked_sub(b)),
        BinOp::Mul => checked(a.checked_mul(b)),
        BinOp::Div => {
            if b == 0 {
                return zero_div();
            }
            if a.abs() <= TWO53 && b.abs() <= TWO53 {
                // 两边都能精确地变成小数：IEEE 的除法就是正确舍入的结果，和 Python 一样
                return Ok(V::Float(a as f64 / b as f64));
            }
            match (a.checked_rem(b), a.checked_div(b)) {
                (Some(0), Some(q)) if q.abs() <= TWO53 => Ok(V::Float(q as f64)),
                _ => unsupported("超过 2**53 的整数相除（这里做不到 Python 那样的精确舍入）"),
            }
        }
        BinOp::FloorDiv => Ok(V::Int(int_floor_div(a, b)?)),
        BinOp::Mod => Ok(V::Int(int_mod(a, b)?)),
        BinOp::Pow => {
            if b < 0 {
                if a == 0 {
                    return raise(Kind::ZeroDivision, "zero to a negative power");
                }
                return float_op(op, a as f64, b as f64);
            }
            match u32::try_from(b) {
                Ok(e) => checked(a.checked_pow(e)),
                Err(_) if a == 0 || a == 1 => Ok(V::Int(a)),
                Err(_) if a == -1 => Ok(V::Int(if b % 2 == 0 { 1 } else { -1 })),
                Err(_) => too_big(),
            }
        }
        BinOp::BitOr => Ok(V::Int(a | b)),
        BinOp::BitXor => Ok(V::Int(a ^ b)),
        BinOp::BitAnd => Ok(V::Int(a & b)),
    }
}

/// 小数的 // 和 %：照 CPython 的 _float_div_mod
pub fn float_divmod(vx: f64, wx: f64) -> (f64, f64) {
    let mut m = vx % wx;
    let mut div = (vx - m) / wx;
    if m != 0.0 {
        if (wx < 0.0) != (m < 0.0) {
            m += wx;
            div -= 1.0;
        }
    } else {
        m = 0.0f64.copysign(wx);
    }
    let floordiv = if div != 0.0 {
        let f = div.floor();
        if div - f > 0.5 {
            f + 1.0
        } else {
            f
        }
    } else {
        0.0f64.copysign(vx / wx)
    };
    (floordiv, m)
}

fn odd_integer(x: f64) -> bool {
    (x.abs() % 2.0) == 1.0
}

fn float_op(op: BinOp, x: f64, y: f64) -> R<V> {
    Ok(V::Float(match op {
        BinOp::Add => x + y,
        BinOp::Sub => x - y,
        BinOp::Mul => x * y,
        BinOp::Div => {
            if y == 0.0 {
                return zero_div();
            }
            x / y
        }
        BinOp::FloorDiv | BinOp::Mod => {
            if y == 0.0 {
                return zero_div();
            }
            let (div, m) = float_divmod(x, y);
            if op == BinOp::Mod {
                m
            } else {
                div
            }
        }
        BinOp::Pow => float_pow(x, y)?,
        BinOp::BitOr | BinOp::BitXor | BinOp::BitAnd => return unsupported("小数的位运算"),
    }))
}

/// 照 CPython 的 float_pow：特殊情况自己定，剩下的交给 pow()
fn float_pow(mut v: f64, w: f64) -> R<f64> {
    if w == 0.0 {
        return Ok(1.0);
    }
    if v.is_nan() {
        return Ok(v);
    }
    if w.is_nan() {
        return Ok(if v == 1.0 { 1.0 } else { w });
    }
    if w.is_infinite() {
        let a = v.abs();
        return Ok(if a == 1.0 {
            1.0
        } else if (w > 0.0) == (a > 1.0) {
            f64::INFINITY
        } else {
            0.0
        });
    }
    if v.is_infinite() {
        let odd = odd_integer(w);
        return Ok(if w > 0.0 {
            if odd {
                v
            } else {
                v.abs()
            }
        } else if odd {
            0.0f64.copysign(v)
        } else {
            0.0
        });
    }
    if v == 0.0 {
        if w < 0.0 {
            return raise(Kind::ZeroDivision, "zero to a negative power");
        }
        return Ok(if odd_integer(w) { v } else { 0.0 });
    }
    let mut negate = false;
    if v < 0.0 {
        if w != w.floor() {
            return unsupported("负数的小数次方（结果是复数）");
        }
        v = -v;
        negate = odd_integer(w);
    }
    if v == 1.0 {
        return Ok(if negate { -1.0 } else { 1.0 });
    }
    let r = v.powf(w);
    if r.is_infinite() {
        return raise(Kind::Overflow, "(34, 'Numerical result out of range')");
    }
    Ok(if negate { -r } else { r })
}

pub fn neg(v: &V) -> R<V> {
    match v {
        V::Bool(_) | V::Int(_) => match as_int(v).and_then(i128::checked_neg) {
            Some(n) => Ok(V::Int(n)),
            None => too_big(),
        },
        V::Float(f) => Ok(V::Float(-f)),
        _ => type_err(format!("bad operand type for unary -: '{}'", type_name(v))),
    }
}

pub fn pos(v: &V) -> R<V> {
    match v {
        V::Bool(b) => Ok(V::Int(i128::from(*b))),
        V::Int(_) | V::Float(_) => Ok(v.clone()),
        _ => type_err(format!("bad operand type for unary +: '{}'", type_name(v))),
    }
}

// ---- 下标和切片 ----

pub fn char_len(s: &str) -> usize {
    if s.is_ascii() {
        s.len()
    } else {
        s.chars().count()
    }
}

/// 下标 → 第几项；负数从后面数。what 是出错说明里的类型名
fn seq_index(i: &V, len: usize, what: &str) -> R<usize> {
    let Some(n) = as_int(i) else {
        return match what {
            "string" => type_err(format!("string indices must be integers, not '{}'", type_name(i))),
            "range object" => type_err(format!("range indices must be integers or slices, not {}", type_name(i))),
            _ => type_err(format!("{what} indices must be integers or slices, not {}", type_name(i))),
        };
    };
    let m = if n < 0 { n + len as i128 } else { n };
    if m < 0 || m >= len as i128 {
        return raise(Kind::Index, format!("{what} index out of range"));
    }
    Ok(m as usize)
}

pub fn getitem(o: &V, i: &V) -> R<V> {
    match o {
        V::List(l) => {
            let l = l.borrow();
            Ok(l[seq_index(i, l.len(), "list")?].clone())
        }
        V::Tuple(t) => Ok(t[seq_index(i, t.len(), "tuple")?].clone()),
        V::Str(s) => {
            let n = seq_index(i, char_len(s), "string")?;
            if s.is_ascii() {
                Ok(V::str(&s[n..n + 1]))
            } else {
                Ok(V::str(s.chars().nth(n).map(String::from).unwrap_or_default()))
            }
        }
        V::Dict(d) => {
            let key = dict_key(i)?;
            match d.borrow().get(&key) {
                Some(v) => Ok(v.clone()),
                None => raise(Kind::Key, pyfmt::repr(i)?),
            }
        }
        V::Range(r) => {
            let Some(len) = r.len() else { return too_big() };
            let n = seq_index(i, usize::try_from(len).unwrap_or(usize::MAX), "range object")?;
            match r.at(n as i128) {
                Some(v) => Ok(V::Int(v)),
                None => too_big(),
            }
        }
        _ => not_subscriptable(o),
    }
}

/// 不能取下标、不能切片的对象
fn not_subscriptable<T>(o: &V) -> R<T> {
    match o {
        // 能写成 list[int] 的那几种类型是合法的 Python（得到一个泛型别名），这里没有
        V::Type(Ty::List | Ty::Tuple | Ty::Dict | Ty::Set | Ty::FrozenSet | Ty::Type | Ty::Enumerate | Ty::Generator) => {
            unsupported("类型的下标（list[int] 这种）")
        }
        V::Type(t) => type_err(format!("type '{}' is not subscriptable", t.label())),
        _ => type_err(format!("'{}' object is not subscriptable", type_name(o))),
    }
}

fn slice_arg(v: &Option<V>) -> R<Option<i128>> {
    match v {
        None | Some(V::None) => Ok(None),
        Some(x) => match as_int(x) {
            Some(n) => Ok(Some(n)),
            None => type_err("slice indices must be integers or None or have an __index__ method"),
        },
    }
}

/// 照 PySlice_AdjustIndices：返回（起点、步长、取几项）
pub fn slice_indices(len: usize, start: Option<i128>, stop: Option<i128>, step: Option<i128>) -> R<(i128, i128, usize)> {
    let len = len as i128;
    let step = step.unwrap_or(1);
    if step == 0 {
        return value_err("slice step cannot be zero");
    }
    let clamp = |v: Option<i128>, default: i128| -> i128 {
        match v {
            None => default,
            Some(v) if v < 0 => {
                let w = v.saturating_add(len);
                if w < 0 {
                    if step < 0 {
                        -1
                    } else {
                        0
                    }
                } else {
                    w
                }
            }
            Some(v) if v >= len => {
                if step < 0 {
                    len - 1
                } else {
                    len
                }
            }
            Some(v) => v,
        }
    };
    let (start, stop) = if step < 0 { (clamp(start, len - 1), clamp(stop, -1)) } else { (clamp(start, 0), clamp(stop, len)) };
    let count = if step < 0 {
        if stop < start {
            (start - stop - 1) / (-step) + 1
        } else {
            0
        }
    } else if start < stop {
        (stop - start - 1) / step + 1
    } else {
        0
    };
    Ok((start, step, count as usize))
}

fn slice_of<T: Clone>(items: &[T], start: Option<i128>, stop: Option<i128>, step: Option<i128>) -> R<Vec<T>> {
    let (mut i, step, count) = slice_indices(items.len(), start, stop, step)?;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(items[i as usize].clone());
        i += step;
    }
    Ok(out)
}

pub fn get_slice(o: &V, a: &Option<V>, b: &Option<V>, c: &Option<V>) -> R<V> {
    match o {
        V::List(_) | V::Tuple(_) | V::Str(_) => {}
        V::Range(_) => return unsupported("range 的切片"),
        V::Dict(_) => return unsupported("拿切片当字典的键"),
        _ => return not_subscriptable(o),
    }
    let (start, stop, step) = (slice_arg(a)?, slice_arg(b)?, slice_arg(c)?);
    match o {
        V::List(l) => Ok(V::list(slice_of(&l.borrow(), start, stop, step)?)),
        V::Tuple(t) => Ok(V::tuple(slice_of(t, start, stop, step)?)),
        V::Str(s) if s.is_ascii() && step.is_none() => {
            let (i, _, count) = slice_indices(s.len(), start, stop, None)?;
            Ok(V::str(&s[i as usize..i as usize + count]))
        }
        V::Str(s) => {
            let chars: Vec<char> = s.chars().collect();
            Ok(V::str(slice_of(&chars, start, stop, step)?.into_iter().collect::<String>()))
        }
        _ => unsupported("切片"),
    }
}

pub fn setitem(o: &V, i: &V, v: V) -> R<()> {
    match o {
        V::List(l) => {
            let mut l = l.borrow_mut();
            let Some(n) = as_int(i) else {
                return type_err(format!("list indices must be integers or slices, not {}", type_name(i)));
            };
            let m = if n < 0 { n + l.len() as i128 } else { n };
            if m < 0 || m >= l.len() as i128 {
                return raise(Kind::Index, "list assignment index out of range");
            }
            l[m as usize] = v;
            Ok(())
        }
        V::Dict(d) => {
            let key = dict_key(i)?;
            d.borrow_mut().set(key, i.clone(), v);
            Ok(())
        }
        _ => type_err(format!("'{}' object does not support item assignment", type_name(o))),
    }
}

pub fn delitem(o: &V, i: &V) -> R<()> {
    match o {
        V::List(l) => {
            let mut l = l.borrow_mut();
            let Some(n) = as_int(i) else {
                return type_err(format!("list indices must be integers or slices, not {}", type_name(i)));
            };
            let m = if n < 0 { n + l.len() as i128 } else { n };
            if m < 0 || m >= l.len() as i128 {
                return raise(Kind::Index, "list assignment index out of range");
            }
            l.remove(m as usize);
            Ok(())
        }
        V::Dict(d) => {
            let key = dict_key(i)?;
            match d.borrow_mut().remove(&key) {
                Some(_) => Ok(()),
                None => raise(Kind::Key, pyfmt::repr(i)?),
            }
        }
        // CPython 里这是两条路：有序列那一套方法的类型碰到整数下标说 doesn't，别的说 does not
        V::Str(_) | V::Tuple(_) | V::Set(_) | V::Range(_) | V::View(_) if as_int(i).is_some() => {
            type_err(format!("'{}' object doesn't support item deletion", type_name(o)))
        }
        _ => type_err(format!("'{}' object does not support item deletion", type_name(o))),
    }
}
