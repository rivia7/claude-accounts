//! 语法树：表达式、简单语句、赋值的目标。只有求值器认的那一部分 Python。

use std::rc::Rc;

#[derive(Debug)]
pub enum Expr {
    None,
    True,
    False,
    Int(i128),
    Float(f64),
    Str(Rc<str>),
    Name(Rc<str>),
    Tuple(Vec<Expr>),
    List(Vec<Expr>),
    Set(Vec<Expr>),
    Dict(Vec<(Expr, Expr)>),
    /// 对象.名字
    Attr(Box<Expr>, Rc<str>),
    /// 对象[下标]；下标是 Slice 时是切片
    Index(Box<Expr>, Box<Expr>),
    /// a:b:c，只出现在 Index 的下标里
    Slice(Option<Box<Expr>>, Option<Box<Expr>>, Option<Box<Expr>>),
    /// 函数、位置参数、关键字参数
    Call(Box<Expr>, Vec<Expr>, Vec<(Rc<str>, Expr)>),
    Not(Box<Expr>),
    Neg(Box<Expr>),
    Pos(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    And(Vec<Expr>),
    Or(Vec<Expr>),
    /// a < b <= c：第一个操作数，后面每一段是（比较符，操作数）
    Compare(Box<Expr>, Vec<(CmpOp, Expr)>),
    /// 条件、条件成立的值、不成立的值（写法是 值 if 条件 else 值）
    IfElse(Box<Expr>, Box<Expr>, Box<Expr>),
    Lambda(Rc<LambdaDef>),
    Comp(Rc<Comp>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    BitOr,
    BitXor,
    BitAnd,
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::FloorDiv => "//",
            BinOp::Mod => "%",
            BinOp::Pow => "**",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::BitAnd => "&",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    NotIn,
    Is,
    IsNot,
}

impl CmpOp {
    pub fn symbol(self) -> &'static str {
        match self {
            CmpOp::Eq => "==",
            CmpOp::Ne => "!=",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
            CmpOp::In => "in",
            CmpOp::NotIn => "not in",
            CmpOp::Is => "is",
            CmpOp::IsNot => "is not",
        }
    }
}

/// lambda：参数名、默认值（对着最后几个参数）、函数体
#[derive(Debug)]
pub struct LambdaDef {
    pub params: Vec<Rc<str>>,
    pub defaults: Vec<Expr>,
    pub body: Expr,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CompKind {
    List,
    Set,
    Dict,
    Gen,
}

/// 推导式和生成器表达式：元素（字典推导式是键，值在 value 里）、一串 for … in … if …
#[derive(Debug)]
pub struct Comp {
    pub kind: CompKind,
    pub elt: Expr,
    pub value: Option<Expr>,
    pub clauses: Vec<Clause>,
    /// 所有的循环变量：推导式有自己的作用域，变量先占着位置（没赋值就读是 UnboundLocalError，不会读到外面同名的）
    pub names: Vec<Rc<str>>,
}

#[derive(Debug)]
pub struct Clause {
    pub target: Target,
    pub iter: Expr,
    pub ifs: Vec<Expr>,
}

/// 赋值的左边、for 的循环变量、del 的对象
#[derive(Debug)]
pub enum Target {
    Name(Rc<str>),
    Tuple(Vec<Target>),
    /// 对象[下标]
    Index(Expr, Expr),
}

#[derive(Debug)]
pub enum Stmt {
    Expr(Expr),
    /// a = b = 值：几个目标从左到右各赋一次
    Assign(Vec<Target>, Expr),
    /// a += 值
    Aug(Target, BinOp, Expr),
    Del(Vec<Target>),
    Assert(Expr, Option<Expr>),
    /// import json, sys, time：这几个名字本来就在，重新绑一次
    Import(Vec<Rc<str>>),
    Pass,
}

/// 一段程序：几条语句，最后可以有一个表达式（它的值是结果）
#[derive(Debug)]
pub struct Program {
    pub stmts: Vec<Stmt>,
    pub last: Option<Expr>,
}
