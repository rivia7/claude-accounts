//! 值：照 Python 的那几种。容器是引用（别名看得到改动），字典记着键的先后，True、1、1.0 是同一个键。

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::BuildHasherDefault;
use std::rc::Rc;

use super::ast::{Comp, LambdaDef};

pub type R<T> = Result<T, Stop>;

/// 求值停下来的三种原因
pub enum Stop {
    /// Python 层面的异常：退出码 1
    Raise(Exc),
    /// 这里不认识、不敢担保的写法：退出码 2
    Unsupported(String),
    /// sys.exit / exit
    Exit(i32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Key,
    Index,
    Type,
    Value,
    Attribute,
    StopIteration,
    ZeroDivision,
    Overflow,
    Assertion,
    Runtime,
    Name,
    UnboundLocal,
    FileNotFound,
    Permission,
    IsADirectory,
    NotADirectory,
    BrokenPipe,
    Os,
    JsonDecode,
    UnicodeDecode,
}

impl Kind {
    /// 异常的类名，照 Python 的回溯最后一行那样写
    pub fn name(self) -> &'static str {
        match self {
            Kind::Key => "KeyError",
            Kind::Index => "IndexError",
            Kind::Type => "TypeError",
            Kind::Value => "ValueError",
            Kind::Attribute => "AttributeError",
            Kind::StopIteration => "StopIteration",
            Kind::ZeroDivision => "ZeroDivisionError",
            Kind::Overflow => "OverflowError",
            Kind::Assertion => "AssertionError",
            Kind::Runtime => "RuntimeError",
            Kind::Name => "NameError",
            Kind::UnboundLocal => "UnboundLocalError",
            Kind::FileNotFound => "FileNotFoundError",
            Kind::Permission => "PermissionError",
            Kind::IsADirectory => "IsADirectoryError",
            Kind::NotADirectory => "NotADirectoryError",
            Kind::BrokenPipe => "BrokenPipeError",
            Kind::Os => "OSError",
            Kind::JsonDecode => "json.decoder.JSONDecodeError",
            Kind::UnicodeDecode => "UnicodeDecodeError",
        }
    }
}

pub struct Exc {
    pub kind: Kind,
    pub msg: String,
}

impl Exc {
    /// 说在 stderr 的那一行：“KeyError: 'x'”，没有说明的只有类名
    pub fn line(&self) -> String {
        if self.msg.is_empty() {
            self.kind.name().to_string()
        } else {
            format!("{}: {}", self.kind.name(), self.msg)
        }
    }
}

pub fn raise<T>(kind: Kind, msg: impl Into<String>) -> R<T> {
    Err(Stop::Raise(Exc { kind, msg: msg.into() }))
}
pub fn type_err<T>(msg: impl Into<String>) -> R<T> {
    raise(Kind::Type, msg)
}
pub fn value_err<T>(msg: impl Into<String>) -> R<T> {
    raise(Kind::Value, msg)
}
pub fn unsupported<T>(msg: impl Into<String>) -> R<T> {
    Err(Stop::Unsupported(msg.into()))
}

#[derive(Clone)]
pub enum V {
    None,
    Bool(bool),
    Int(i128),
    Float(f64),
    Str(Rc<str>),
    List(Rc<RefCell<Vec<V>>>),
    Tuple(Rc<Vec<V>>),
    Dict(Rc<RefCell<Dict>>),
    Set(Rc<SetObj>),
    Range(Rc<RangeV>),
    /// lambda
    Func(Rc<Lambda>),
    /// 内建函数
    Builtin(B),
    /// 对象.方法（已经绑着对象）
    Bound(Rc<Bound>),
    /// 类型.方法：str.lower 这种，调用时第一个参数是对象
    Unbound(Ty, &'static str),
    /// 类型本身：str、int、type(x) 的结果
    Type(Ty),
    /// 迭代器：生成器、map、filter、zip、enumerate、reversed、iter(…)
    Iter(Rc<IterObj>),
    /// dict.keys() / values() / items()：盯着字典的视图
    View(Rc<View>),
    Module(Mod),
    File(Rc<FileObj>),
    /// sys.stdout / sys.stderr（只给 print 的 file= 用）
    Stream(Stream),
}

impl V {
    pub fn str(s: impl AsRef<str>) -> V {
        V::Str(Rc::from(s.as_ref()))
    }
    pub fn list(items: Vec<V>) -> V {
        V::List(Rc::new(RefCell::new(items)))
    }
    pub fn tuple(items: Vec<V>) -> V {
        V::Tuple(Rc::new(items))
    }
    pub fn dict(d: Dict) -> V {
        V::Dict(Rc::new(RefCell::new(d)))
    }
    pub fn set(d: Dict, frozen: bool) -> V {
        V::Set(Rc::new(SetObj { frozen, d: RefCell::new(d) }))
    }
    pub fn iter(ty: Ty, st: IterState) -> V {
        V::Iter(Rc::new(IterObj { ty, st: RefCell::new(st) }))
    }
}

/// 内建函数（类型另算：见 Ty）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum B {
    Len,
    Any,
    All,
    Sum,
    Min,
    Max,
    Sorted,
    Next,
    Iter,
    Repr,
    Abs,
    Round,
    Isinstance,
    Divmod,
    Chr,
    Ord,
    Print,
    Open,
    /// exit()：site 模块给的那个
    Exit,
    /// sys.exit()
    SysExit,
    JsonLoad,
    JsonLoads,
    JsonDumps,
    TimeTime,
}

impl B {
    pub fn name(self) -> &'static str {
        match self {
            B::Len => "len",
            B::Any => "any",
            B::All => "all",
            B::Sum => "sum",
            B::Min => "min",
            B::Max => "max",
            B::Sorted => "sorted",
            B::Next => "next",
            B::Iter => "iter",
            B::Repr => "repr",
            B::Abs => "abs",
            B::Round => "round",
            B::Isinstance => "isinstance",
            B::Divmod => "divmod",
            B::Chr => "chr",
            B::Ord => "ord",
            B::Print => "print",
            B::Open => "open",
            B::Exit | B::SysExit => "exit",
            B::JsonLoad => "load",
            B::JsonLoads => "loads",
            B::JsonDumps => "dumps",
            B::TimeTime => "time",
        }
    }
}

/// 类型。前面那些能用名字写出来（也能调用）；后面的只会从 type(x) 得到
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ty {
    Str,
    Int,
    Float,
    Bool,
    List,
    Tuple,
    Dict,
    Set,
    FrozenSet,
    Type,
    Range,
    Map,
    Filter,
    Zip,
    Enumerate,
    Reversed,
    NoneType,
    Function,
    BuiltinFn,
    Method,
    Generator,
    Iterator(&'static str),
    DictKeys,
    DictValues,
    DictItems,
    Module,
    File,
}

impl Ty {
    /// type(x).__name__
    pub fn name(self) -> &'static str {
        match self {
            Ty::Str => "str",
            Ty::Int => "int",
            Ty::Float => "float",
            Ty::Bool => "bool",
            Ty::List => "list",
            Ty::Tuple => "tuple",
            Ty::Dict => "dict",
            Ty::Set => "set",
            Ty::FrozenSet => "frozenset",
            Ty::Type => "type",
            Ty::Range => "range",
            Ty::Map => "map",
            Ty::Filter => "filter",
            Ty::Zip => "zip",
            Ty::Enumerate => "enumerate",
            Ty::Reversed => "reversed",
            Ty::NoneType => "NoneType",
            Ty::Function => "function",
            Ty::BuiltinFn => "builtin_function_or_method",
            Ty::Method => "builtin_function_or_method",
            Ty::Generator => "generator",
            Ty::Iterator(name) => name,
            Ty::DictKeys => "dict_keys",
            Ty::DictValues => "dict_values",
            Ty::DictItems => "dict_items",
            Ty::Module => "module",
            Ty::File => "TextIOWrapper",
        }
    }

    /// 出错说明里的写法（CPython 的 tp_name）：只有文件带着模块名
    pub fn label(self) -> &'static str {
        match self {
            Ty::File => "_io.TextIOWrapper",
            other => other.name(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mod {
    Json,
    Sys,
    Time,
}

impl Mod {
    pub fn name(self) -> &'static str {
        match self {
            Mod::Json => "json",
            Mod::Sys => "sys",
            Mod::Time => "time",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stream {
    Stdout,
    Stderr,
}

pub struct Bound {
    pub recv: V,
    pub name: &'static str,
}

/// 字典的键（也是集合的元素）散列和比较用的样子：True、1、1.0 归成同一个
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum Key {
    None,
    Int(i128),
    /// 不是整数值的小数：按位存
    Float(u64),
    Str(Rc<str>),
    Tuple(Vec<Key>),
    /// frozenset：元素排过序
    Frozen(Vec<Key>),
}

pub struct Entry {
    pub key: Key,
    /// 放进来时的那个键（1.0 和 1 是同一个键，留着先来的）
    pub k: V,
    pub v: V,
}

type Hasher = BuildHasherDefault<std::collections::hash_map::DefaultHasher>;

/// 字典：按放进去的先后排；给已有的键赋值不挪位置
#[derive(Default)]
pub struct Dict {
    entries: Vec<Entry>,
    index: HashMap<Key, usize, Hasher>,
    /// 增删过键就加一：遍历途中键变了要报错
    pub version: u64,
}

impl Dict {
    pub fn new() -> Dict {
        Dict::default()
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
    pub fn get(&self, key: &Key) -> Option<&V> {
        self.index.get(key).map(|&i| &self.entries[i].v)
    }
    pub fn contains(&self, key: &Key) -> bool {
        self.index.contains_key(key)
    }
    /// d[k] = v：键已经在的话只换值（位置、原来的键都不动）
    pub fn set(&mut self, key: Key, k: V, v: V) {
        match self.index.get(&key) {
            Some(&i) => self.entries[i].v = v,
            None => {
                self.index.insert(key.clone(), self.entries.len());
                self.entries.push(Entry { key, k, v });
                self.version += 1;
            }
        }
    }
    /// 没有才放（集合的 add、setdefault）；返回放进去了没有
    pub fn add(&mut self, key: Key, k: V, v: V) -> bool {
        if self.index.contains_key(&key) {
            return false;
        }
        self.set(key, k, v);
        true
    }
    pub fn remove(&mut self, key: &Key) -> Option<Entry> {
        let i = self.index.remove(key)?;
        let e = self.entries.remove(i);
        for (j, later) in self.entries.iter().enumerate().skip(i) {
            if let Some(slot) = self.index.get_mut(&later.key) {
                *slot = j;
            }
        }
        self.version += 1;
        Some(e)
    }
    pub fn pop_last(&mut self) -> Option<Entry> {
        let e = self.entries.pop()?;
        self.index.remove(&e.key);
        self.version += 1;
        Some(e)
    }
    pub fn clear(&mut self) {
        if !self.entries.is_empty() {
            self.version += 1;
        }
        self.entries.clear();
        self.index.clear();
    }
    /// 内容整个换成另一份（集合的 |= 这类就地运算）；版本接着往上加，正在遍历它的迭代器看得出来
    pub fn replace_with(&mut self, other: Dict) {
        let version = self.version + 1;
        *self = other;
        self.version = version;
    }
    /// 浅拷贝（里面的值还是同一个对象）
    pub fn copy(&self) -> Dict {
        let mut d = Dict::new();
        for e in &self.entries {
            d.set(e.key.clone(), e.k.clone(), e.v.clone());
        }
        d
    }
}

/// set / frozenset：就是只有键的字典。遍历顺序 Python 不保证，会把顺序露出来的用法在求值时拦下
pub struct SetObj {
    pub frozen: bool,
    pub d: RefCell<Dict>,
}

pub struct RangeV {
    pub start: i128,
    pub stop: i128,
    pub step: i128,
}

impl RangeV {
    /// 有多少项；算不下（超过 128 位）是 None
    pub fn len(&self) -> Option<i128> {
        let (lo, hi, step) =
            if self.step > 0 { (self.start, self.stop, self.step) } else { (self.stop, self.start, self.step.checked_neg()?) };
        if lo >= hi {
            return Some(0);
        }
        Some((hi.checked_sub(lo)? - 1) / step + 1)
    }
    pub fn at(&self, i: i128) -> Option<i128> {
        self.start.checked_add(i.checked_mul(self.step)?)
    }
}

pub type Env = Option<Rc<Scope>>;

/// lambda 的参数、推导式的循环变量各有一层；全局的不在这里（在求值器手上）
pub struct Scope {
    /// None：占着位置、还没赋值
    pub vars: RefCell<Vec<(Rc<str>, Option<V>)>>,
    pub parent: Env,
}

pub struct Lambda {
    pub def: Rc<LambdaDef>,
    /// 默认值：定义的时候就算好了
    pub defaults: Vec<V>,
    pub env: Env,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewKind {
    Keys,
    Values,
    Items,
}

pub struct View {
    pub d: Rc<RefCell<Dict>>,
    pub kind: ViewKind,
}

/// 推导式跑到哪了：每个 for 一个迭代器
pub struct CompState {
    pub def: Rc<Comp>,
    pub env: Env,
    pub iters: Vec<V>,
}

/// 迭代器：是哪一种（type(x).__name__ 要用，走完了也还知道）、走到哪了
pub struct IterObj {
    pub ty: Ty,
    pub st: RefCell<IterState>,
}

pub enum IterState {
    Done,
    List(Rc<RefCell<Vec<V>>>, usize),
    Tuple(Rc<Vec<V>>, usize),
    /// 字符串和读到第几个字节
    Str(Rc<str>, usize),
    /// 下一个值、还剩几个、步长
    Range(i128, i128, i128),
    /// 字典（或它的视图）：读到第几项、开始时的大小和版本
    Dict(Rc<RefCell<Dict>>, ViewKind, usize, usize, u64),
    Set(Rc<SetObj>, usize, usize, u64),
    /// 倒着走的（元组、字符串、range、字典）：剩下的项，从最后一个往前取
    Rev(Vec<V>),
    /// reversed(列表)：下标从后往前，列表中途变短了就结束
    RevList(Rc<RefCell<Vec<V>>>, usize),
    Gen(CompState),
    Map(V, Vec<V>),
    Filter(V, V),
    Zip(Vec<V>),
    Enumerate(V, i128),
}

pub enum FileState {
    /// 还没读
    Open(std::fs::File),
    /// 整个读进来了：文字、读到第几个字节
    Text(Rc<str>, usize),
    Closed,
}

pub struct FileObj {
    pub path: String,
    pub state: RefCell<FileState>,
}

/// 出错说明里的类型名
pub fn type_name(v: &V) -> &'static str {
    ty_of(v).label()
}

pub fn ty_of(v: &V) -> Ty {
    match v {
        V::None => Ty::NoneType,
        V::Bool(_) => Ty::Bool,
        V::Int(_) => Ty::Int,
        V::Float(_) => Ty::Float,
        V::Str(_) => Ty::Str,
        V::List(_) => Ty::List,
        V::Tuple(_) => Ty::Tuple,
        V::Dict(_) => Ty::Dict,
        V::Set(s) => {
            if s.frozen {
                Ty::FrozenSet
            } else {
                Ty::Set
            }
        }
        V::Range(_) => Ty::Range,
        V::Func(_) => Ty::Function,
        V::Builtin(_) => Ty::BuiltinFn,
        V::Bound(_) => Ty::Method,
        V::Unbound(..) => Ty::Iterator("method_descriptor"),
        V::Type(_) => Ty::Type,
        V::Iter(it) => it.ty,
        V::View(v) => match v.kind {
            ViewKind::Keys => Ty::DictKeys,
            ViewKind::Values => Ty::DictValues,
            ViewKind::Items => Ty::DictItems,
        },
        V::Module(_) => Ty::Module,
        V::File(_) | V::Stream(_) => Ty::File,
    }
}
