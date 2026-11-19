//! Python 的值在 Rust 里的样子。
//!
//! config.json、账号表、别人的 JSON（.claude.json、settings.json、Chrome 的 Preferences）在原来的 Python 里都是
//! 动态的字典：到处是 `isinstance`、`x or 默认值`，拿到里面一层的字典直接改，改完整份存回去，键的顺序不能变。
//! 这里照 Python 的语义做一套：字典、列表是共享的引用（`Rc<RefCell<…>>`，`clone` 只是多一个引用，和 Python 里
//! 把同一个对象赋给另一个名字一样），字典记着键的先后。移植时可以逐行对着写。
//!
//! 只在单线程里用（`Rc` 不能跨线程）：要交给线程的数据先取成 `String`、`Vec` 这类普通值。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Clone)]
pub enum V {
    None,
    Bool(bool),
    Int(i64),
    /// 放不进 i64 的整数：原样的十进制数字，只经手、不运算
    Big(Rc<str>),
    Float(f64),
    Str(Rc<str>),
    List(List),
    Dict(Dict),
}

#[derive(Clone, Default)]
pub struct List(Rc<RefCell<Vec<V>>>);

#[derive(Clone, Default)]
pub struct Dict(Rc<RefCell<DictInner>>);

#[derive(Default)]
struct DictInner {
    items: Vec<(Rc<str>, V)>,
    /// 键 → 位置。项多了才建（小字典顺着找更快），删过项就作废、下次用到再建
    index: Option<HashMap<Rc<str>, usize>>,
}

/// 多于这么多项的字典按键查找时建索引
const INDEX_FROM: usize = 24;

impl DictInner {
    fn find(&mut self, k: &str) -> Option<usize> {
        if self.items.len() < INDEX_FROM {
            return self.items.iter().position(|(x, _)| &**x == k);
        }
        if self.index.is_none() {
            self.index = Some(self.items.iter().enumerate().map(|(i, (x, _))| (x.clone(), i)).collect());
        }
        self.index.as_ref().and_then(|m| m.get(k).copied())
    }
}

impl Dict {
    pub fn new() -> Dict {
        Dict::default()
    }

    pub fn from_pairs<K: Into<Rc<str>>, T: Into<V>>(pairs: impl IntoIterator<Item = (K, T)>) -> Dict {
        let d = Dict::new();
        for (k, v) in pairs {
            d.set(k, v);
        }
        d
    }

    pub fn len(&self) -> usize {
        self.0.borrow().items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `d.get(k)`
    pub fn get(&self, k: &str) -> Option<V> {
        let mut d = self.0.borrow_mut();
        d.find(k).map(|i| d.items[i].1.clone())
    }

    /// `k in d`
    pub fn has(&self, k: &str) -> bool {
        self.0.borrow_mut().find(k).is_some()
    }

    /// `d[k]`：没有这个键在 Python 里是没人接的 KeyError（打印调用栈、退出码 1），这里是 panic，由 main 的钩子照样处理
    pub fn at(&self, k: &str) -> V {
        self.get(k).unwrap_or_else(|| panic!("KeyError: {}", repr_str(k)))
    }

    /// `d[k] = v`：已有的键留在原来的位置
    pub fn set(&self, k: impl Into<Rc<str>>, v: impl Into<V>) {
        let (k, v) = (k.into(), v.into());
        let mut d = self.0.borrow_mut();
        match d.find(&k) {
            Some(i) => d.items[i].1 = v,
            None => {
                let i = d.items.len();
                if let Some(m) = d.index.as_mut() {
                    m.insert(k.clone(), i);
                }
                d.items.push((k, v));
            }
        }
    }

    /// `d.setdefault(k, v)`
    pub fn setdefault(&self, k: &str, v: impl Into<V>) -> V {
        if let Some(x) = self.get(k) {
            return x;
        }
        let v = v.into();
        self.set(k, v.clone());
        v
    }

    /// `d.pop(k, None)`
    pub fn pop(&self, k: &str) -> Option<V> {
        let mut d = self.0.borrow_mut();
        let i = d.find(k)?;
        d.index = None;
        Some(d.items.remove(i).1)
    }

    /// `list(d)`：键的快照（边遍历边改字典也安全）
    pub fn keys(&self) -> Vec<Rc<str>> {
        self.0.borrow().items.iter().map(|(k, _)| k.clone()).collect()
    }

    pub fn values(&self) -> Vec<V> {
        self.0.borrow().items.iter().map(|(_, v)| v.clone()).collect()
    }

    /// `list(d.items())`
    pub fn items(&self) -> Vec<(Rc<str>, V)> {
        self.0.borrow().items.clone()
    }

    pub fn clear(&self) {
        let mut d = self.0.borrow_mut();
        d.items.clear();
        d.index = None;
    }

    /// `d.update(other)`
    pub fn update(&self, other: &Dict) {
        for (k, v) in other.items() {
            self.set(k, v);
        }
    }

    /// `dict(d)`：浅拷贝（里面的字典、列表还是同一个）
    pub fn copy(&self) -> Dict {
        Dict::from_pairs(self.items())
    }

    /// `a is b`
    pub fn same(&self, other: &Dict) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    // 常见的「取出来、看类型对不对」

    /// 值是字符串才给
    pub fn str(&self, k: &str) -> Option<Rc<str>> {
        match self.get(k) {
            Some(V::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// 值是字典才给
    pub fn dict(&self, k: &str) -> Option<Dict> {
        match self.get(k) {
            Some(V::Dict(d)) => Some(d),
            _ => None,
        }
    }

    /// 值是列表才给
    pub fn list(&self, k: &str) -> Option<List> {
        match self.get(k) {
            Some(V::List(l)) => Some(l),
            _ => None,
        }
    }

    /// `d.get(k) or 别的`里的那个判断：有这个键、值算真
    pub fn truthy(&self, k: &str) -> bool {
        self.get(k).is_some_and(|v| v.truthy())
    }
}

impl List {
    pub fn new() -> List {
        List::default()
    }

    pub fn from_vec(v: Vec<V>) -> List {
        List(Rc::new(RefCell::new(v)))
    }

    pub fn len(&self) -> usize {
        self.0.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, i: usize) -> Option<V> {
        self.0.borrow().get(i).cloned()
    }

    /// `l[i]`（i 可以是负数）：越界是没人接的 IndexError
    pub fn at(&self, i: isize) -> V {
        let n = self.len() as isize;
        let j = if i < 0 { i + n } else { i };
        if j < 0 || j >= n {
            panic!("IndexError: list index out of range");
        }
        self.0.borrow()[j as usize].clone()
    }

    pub fn set(&self, i: usize, v: impl Into<V>) {
        self.0.borrow_mut()[i] = v.into();
    }

    pub fn push(&self, v: impl Into<V>) {
        self.0.borrow_mut().push(v.into());
    }

    #[cfg(test)]
    pub fn remove(&self, i: usize) -> V {
        self.0.borrow_mut().remove(i)
    }

    pub fn clear(&self) {
        self.0.borrow_mut().clear();
    }

    /// `list(l)`：快照
    pub fn items(&self) -> Vec<V> {
        self.0.borrow().clone()
    }

    /// `x in l`（照 Python 的 ==）
    pub fn contains(&self, x: &V) -> bool {
        self.0.borrow().iter().any(|y| y.py_eq(x))
    }

    /// `s in l`，s 是字符串
    pub fn has_str(&self, s: &str) -> bool {
        self.0.borrow().iter().any(|y| matches!(y, V::Str(t) if &**t == s))
    }

    /// 里面的字符串（不是字符串的项跳过）
    pub fn strs(&self) -> Vec<Rc<str>> {
        self.0.borrow().iter().filter_map(|v| v.rc_str()).collect()
    }

    pub fn same(&self, other: &List) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl V {
    pub fn str(s: impl AsRef<str>) -> V {
        V::Str(Rc::from(s.as_ref()))
    }

    pub fn is_none(&self) -> bool {
        matches!(self, V::None)
    }

    pub fn is_str(&self) -> bool {
        matches!(self, V::Str(_))
    }

    pub fn is_dict(&self) -> bool {
        matches!(self, V::Dict(_))
    }

    pub fn is_list(&self) -> bool {
        matches!(self, V::List(_))
    }

    pub fn is_bool(&self) -> bool {
        matches!(self, V::Bool(_))
    }

    /// `type(x) is int`：整数，布尔值不算
    pub fn is_int(&self) -> bool {
        matches!(self, V::Int(_) | V::Big(_))
    }

    /// `isinstance(x, int)`：Python 里布尔值也是 int
    pub fn isinstance_int(&self) -> bool {
        matches!(self, V::Bool(_) | V::Int(_) | V::Big(_))
    }

    /// `isinstance(x, (int, float))`：布尔值也算
    pub fn isinstance_num(&self) -> bool {
        matches!(self, V::Bool(_) | V::Int(_) | V::Big(_) | V::Float(_))
    }

    /// 真值：None、False、0、0.0、空串、空列表、空字典是假
    pub fn truthy(&self) -> bool {
        match self {
            V::None => false,
            V::Bool(b) => *b,
            V::Int(i) => *i != 0,
            V::Big(_) => true,
            V::Float(f) => *f != 0.0,
            V::Str(s) => !s.is_empty(),
            V::List(l) => !l.is_empty(),
            V::Dict(d) => !d.is_empty(),
        }
    }

    /// `x or other`
    pub fn or(self, other: impl Into<V>) -> V {
        if self.truthy() {
            self
        } else {
            other.into()
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            V::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn rc_str(&self) -> Option<Rc<str>> {
        match self {
            V::Str(s) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<Dict> {
        match self {
            V::Dict(d) => Some(d.clone()),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<List> {
        match self {
            V::List(l) => Some(l.clone()),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            V::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// 整数的值（布尔值不算）
    pub fn as_int(&self) -> Option<i64> {
        match self {
            V::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// 数字的值（整数、小数；布尔值不算）
    pub fn as_num(&self) -> Option<f64> {
        match self {
            V::Int(i) => Some(*i as f64),
            V::Big(s) => s.parse().ok(),
            V::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// `float(x)` 对 `isinstance(x, (int, float))` 的值（布尔值是 1.0 / 0.0）
    pub fn as_pynum(&self) -> Option<f64> {
        match self {
            V::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => self.as_num(),
        }
    }

    /// Python 的 `==`：`True == 1 == 1.0`，字典不看顺序，列表看顺序
    pub fn py_eq(&self, o: &V) -> bool {
        match (self, o) {
            (V::None, V::None) => true,
            (V::Str(a), V::Str(b)) => a == b,
            (V::Big(a), V::Big(b)) => a == b,
            (V::List(a), V::List(b)) => {
                if a.same(b) {
                    return true;
                }
                let (x, y) = (a.0.borrow(), b.0.borrow());
                x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| p.py_eq(q))
            }
            (V::Dict(a), V::Dict(b)) => {
                if a.same(b) {
                    return true;
                }
                a.len() == b.len() && a.items().iter().all(|(k, v)| b.get(k).is_some_and(|w| v.py_eq(&w)))
            }
            // 放不进 i64 的整数和小数：小数正好是这个整数才相等（和别的整数、布尔值一定不等：值不在它们的范围里）
            (V::Big(a), V::Float(f)) | (V::Float(f), V::Big(a)) => f.is_finite() && f.fract() == 0.0 && format!("{f:.0}") == **a,
            _ => match (self.num_key(), o.num_key()) {
                (Some(Num::I(a)), Some(Num::I(b))) => a == b,
                (Some(Num::I(a)), Some(Num::F(b))) | (Some(Num::F(b)), Some(Num::I(a))) => int_eq_float(a, b),
                (Some(Num::F(a)), Some(Num::F(b))) => a == b,
                _ => false,
            },
        }
    }

    fn num_key(&self) -> Option<Num> {
        match self {
            V::Bool(b) => Some(Num::I(*b as i64)),
            V::Int(i) => Some(Num::I(*i)),
            V::Float(f) => Some(Num::F(*f)),
            _ => None,
        }
    }

    /// `str(x)`
    pub fn py_str(&self) -> String {
        match self {
            V::Str(s) => s.to_string(),
            _ => self.py_repr(),
        }
    }

    /// `repr(x)`
    pub fn py_repr(&self) -> String {
        match self {
            V::None => "None".into(),
            V::Bool(true) => "True".into(),
            V::Bool(false) => "False".into(),
            V::Int(i) => i.to_string(),
            V::Big(s) => s.to_string(),
            V::Float(f) => super::num::float_repr(*f),
            V::Str(s) => repr_str(s),
            V::List(l) => format!("[{}]", l.items().iter().map(|v| v.py_repr()).collect::<Vec<_>>().join(", ")),
            V::Dict(d) => format!(
                "{{{}}}",
                d.items().iter().map(|(k, v)| format!("{}: {}", repr_str(k), v.py_repr())).collect::<Vec<_>>().join(", ")
            ),
        }
    }

    /// `copy.deepcopy(x)`（也是 `json.loads(json.dumps(x))` 那种“拷一份”的写法）
    pub fn deepcopy(&self) -> V {
        match self {
            V::List(l) => V::List(List::from_vec(l.items().iter().map(|v| v.deepcopy()).collect())),
            V::Dict(d) => V::Dict(Dict::from_pairs(d.items().into_iter().map(|(k, v)| (k, v.deepcopy())))),
            x => x.clone(),
        }
    }
}

// 调试、断言失败时显示的样子就是 Python 的 repr
impl std::fmt::Debug for V {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.py_repr())
    }
}

impl std::fmt::Debug for Dict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&V::Dict(self.clone()).py_repr())
    }
}

impl std::fmt::Debug for List {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&V::List(self.clone()).py_repr())
    }
}

enum Num {
    I(i64),
    F(f64),
}

/// 整数 == 小数：Python 比的是精确的值，不是先把整数变成小数（那样 2**53 以上相邻的几个整数都“等于”同一个小数：
/// `9007199254740993 == 9007199254740992.0` 是 False）。小数是整数值、落在 i64 的范围里时 `as i64` 是精确的
fn int_eq_float(i: i64, f: f64) -> bool {
    f.fract() == 0.0 && (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&f) && f as i64 == i
}

/// `repr(字符串)`：默认单引号，里面有单引号、没有双引号时换成双引号；不可打印的字符写成转义
pub fn repr_str(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(q);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            c if super::uni::is_printable(c) => out.push(c),
            c if (c as u32) < 0x100 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push(q);
    out
}

impl From<&str> for V {
    fn from(s: &str) -> V {
        V::Str(Rc::from(s))
    }
}
impl From<String> for V {
    fn from(s: String) -> V {
        V::Str(Rc::from(s))
    }
}
impl From<&String> for V {
    fn from(s: &String) -> V {
        V::Str(Rc::from(s.as_str()))
    }
}
impl From<Rc<str>> for V {
    fn from(s: Rc<str>) -> V {
        V::Str(s)
    }
}
impl From<&Rc<str>> for V {
    fn from(s: &Rc<str>) -> V {
        V::Str(s.clone())
    }
}
impl From<bool> for V {
    fn from(b: bool) -> V {
        V::Bool(b)
    }
}
impl From<i64> for V {
    fn from(i: i64) -> V {
        V::Int(i)
    }
}
impl From<i32> for V {
    fn from(i: i32) -> V {
        V::Int(i as i64)
    }
}
impl From<u16> for V {
    fn from(i: u16) -> V {
        V::Int(i as i64)
    }
}
impl From<u32> for V {
    fn from(i: u32) -> V {
        V::Int(i as i64)
    }
}
impl From<usize> for V {
    fn from(i: usize) -> V {
        V::Int(i as i64)
    }
}
impl From<f64> for V {
    fn from(f: f64) -> V {
        V::Float(f)
    }
}
impl From<Dict> for V {
    fn from(d: Dict) -> V {
        V::Dict(d)
    }
}
impl From<&Dict> for V {
    fn from(d: &Dict) -> V {
        V::Dict(d.clone())
    }
}
impl From<List> for V {
    fn from(l: List) -> V {
        V::List(l)
    }
}
impl From<&List> for V {
    fn from(l: &List) -> V {
        V::List(l.clone())
    }
}
impl From<Vec<V>> for V {
    fn from(v: Vec<V>) -> V {
        V::List(List::from_vec(v))
    }
}
impl From<&V> for V {
    fn from(v: &V) -> V {
        v.clone()
    }
}
impl<T: Into<V>> From<Option<T>> for V {
    fn from(o: Option<T>) -> V {
        o.map_or(V::None, Into::into)
    }
}

/// 字符串的列表
pub fn str_list<S: AsRef<str>>(items: impl IntoIterator<Item = S>) -> List {
    List::from_vec(items.into_iter().map(|s| V::str(s)).collect())
}

/// 写字典的字面量：`dict! { "键" => 值, … }`
#[macro_export]
macro_rules! dict {
    ($($k:expr => $v:expr),* $(,)?) => {{
        let d = $crate::py::obj::Dict::new();
        $( d.set($k, $v); )*
        d
    }};
}

/// 写列表的字面量：`list![值, …]`
#[macro_export]
macro_rules! list {
    ($($v:expr),* $(,)?) => {
        $crate::py::obj::List::from_vec(vec![$( $crate::py::obj::V::from($v) ),*])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dict_keeps_order_and_position() {
        let d = dict! { "b" => 1, "a" => 2 };
        d.set("b", 3);
        d.set("c", 4);
        assert_eq!(d.keys().iter().map(|k| k.to_string()).collect::<Vec<_>>(), ["b", "a", "c"]);
        assert_eq!(d.pop("a").unwrap().as_int(), Some(2));
        d.set("a", 5);
        assert_eq!(d.keys().iter().map(|k| k.to_string()).collect::<Vec<_>>(), ["b", "c", "a"]);
    }

    #[test]
    fn big_dict_index_survives_removal() {
        let d = Dict::new();
        for i in 0..100 {
            d.set(format!("k{i}"), i);
        }
        assert_eq!(d.get("k57").unwrap().as_int(), Some(57));
        d.pop("k3");
        assert_eq!(d.get("k57").unwrap().as_int(), Some(57));
        assert!(d.get("k3").is_none());
        d.set("k3", 1000);
        assert_eq!(d.keys().last().unwrap().as_ref(), "k3");
        assert_eq!(d.len(), 100);
    }

    #[test]
    fn shared_reference() {
        let c = dict! { "ports" => Dict::new() };
        let ports = c.dict("ports").unwrap();
        ports.set("a", 7901);
        assert_eq!(c.dict("ports").unwrap().get("a").unwrap().as_int(), Some(7901));
    }

    #[test]
    fn equality_like_python() {
        assert!(V::Bool(true).py_eq(&V::Int(1)));
        assert!(V::Int(1).py_eq(&V::Float(1.0)));
        assert!(!V::Str("1".into()).py_eq(&V::Int(1)));
        let a = dict! { "x" => 1, "y" => list![1, 2] };
        let b = dict! { "y" => list![1, 2], "x" => 1.0 };
        assert!(V::Dict(a).py_eq(&V::Dict(b)));
        assert!(!V::from(list![1, 2]).py_eq(&V::from(list![2, 1])));
        // 整数和小数比精确的值（期望值是 Python 算的）
        let eq = |a: V, b: V| (a.py_eq(&b), b.py_eq(&a));
        assert_eq!(eq(V::Int(9007199254740992), V::Float(9007199254740992.0)), (true, true));
        assert_eq!(eq(V::Int(9007199254740993), V::Float(9007199254740992.0)), (false, false));
        assert_eq!(eq(V::Int(1795078097521852400), V::Float(1.7950780975218524e18)), (false, false));
        assert_eq!(eq(V::Int(1795078097521852416), V::Float(1.7950780975218524e18)), (true, true));
        assert_eq!(eq(V::Int(i64::MAX), V::Float(i64::MAX as f64)), (false, false)); // 这个小数是 2**63
        assert_eq!(eq(V::Int(i64::MIN), V::Float(i64::MIN as f64)), (true, true));
        assert_eq!(eq(V::Int(1), V::Float(1.5)), (false, false));
        assert_eq!(eq(V::Int(0), V::Float(-0.0)), (true, true));
        assert_eq!(eq(V::Int(0), V::Float(f64::NAN)), (false, false));
        assert_eq!(eq(V::Int(0), V::Float(f64::INFINITY)), (false, false));
        assert_eq!(eq(V::Bool(true), V::Float(1.0)), (true, true));
        assert_eq!(eq(V::Float(f64::NAN), V::Float(f64::NAN)), (false, false));
        // 放不进 i64 的整数
        let big = |s: &str| V::Big(s.into());
        assert_eq!(eq(big("9223372036854775808"), V::Float(2f64.powi(63))), (true, true));
        assert_eq!(eq(big("1000000000000000019884624838656"), V::Float(1e30)), (true, true));
        assert_eq!(eq(big("1000000000000000000000000000000"), V::Float(1e30)), (false, false)); // 10**30 != 1e30
        assert_eq!(eq(big("-9223372036854775809"), V::Float(-(2f64.powi(63)))), (false, false));
        assert_eq!(eq(big("9223372036854775808"), V::Int(i64::MAX)), (false, false));
    }

    #[test]
    fn repr_like_python() {
        assert_eq!(V::from(list!["a", "it's", 1, true, V::None]).py_repr(), "['a', \"it's\", 1, True, None]");
        assert_eq!(repr_str("a\n\x01\u{7f}"), "'a\\n\\x01\\x7f'");
        assert_eq!(repr_str("中文"), "'中文'");
        assert_eq!(V::Dict(dict! { "k" => 1.5 }).py_repr(), "{'k': 1.5}");
    }

    #[test]
    fn truthiness() {
        assert!(!V::str("").truthy());
        assert!(!V::Dict(Dict::new()).truthy());
        assert!(V::Float(0.1).truthy());
        assert_eq!(V::str("").or("x").as_str(), Some("x"));
    }
}
