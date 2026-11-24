//! 语法：记号 → 语法树。优先级和结合性照 Python；不支持的写法在这里就停下，说清楚是什么（退出码 2）。

use std::rc::Rc;

use super::ast::*;
use super::lex::{bad, lex, Bad, Tok, Token};

/// 表达式能嵌套多少层（Python 自己在 200 层括号上报 too many nested parentheses）
const MAX_NEST: usize = 200;
/// a + b + c + … 一口气能连多少个（求值时每一个是一层递归）
const MAX_CHAIN: usize = 2000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// 几条语句，最后是一个表达式（jok、evq、check、val）
    Expr,
    /// 只有语句（edit、run）
    Stmts,
}

pub const KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else",
    "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise",
    "return", "try", "while", "with", "yield",
];

/// 能 import 的（本来就在的那几个名字）
pub const MODULES: &[&str] = &["json", "sys", "time"];

pub fn parse(src: &str, mode: Mode) -> Result<Program, Bad> {
    let toks = lex(src, mode == Mode::Expr)?;
    let mut p = Parser { toks, i: 0, depth: 0 };
    p.program(mode)
}

struct Parser {
    toks: Vec<Token>,
    i: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.i].tok
    }
    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.i + n).min(self.toks.len() - 1)].tok
    }
    fn pos(&self) -> usize {
        self.toks[self.i].pos
    }
    fn bump(&mut self) -> Tok {
        let t = self.toks[self.i].tok.clone();
        if self.i + 1 < self.toks.len() {
            self.i += 1;
        }
        t
    }
    fn is_op(&self, op: &str) -> bool {
        matches!(self.peek(), Tok::Op(o) if *o == op)
    }
    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Name(n) if n == kw)
    }
    fn eat_op(&mut self, op: &str) -> bool {
        let hit = self.is_op(op);
        if hit {
            self.bump();
        }
        hit
    }
    fn eat_kw(&mut self, kw: &str) -> bool {
        let hit = self.is_kw(kw);
        if hit {
            self.bump();
        }
        hit
    }
    fn expect_op(&mut self, op: &str) -> Result<(), Bad> {
        if self.eat_op(op) {
            Ok(())
        } else {
            bad(self.pos(), format!("这里要 {op}，却是{}", self.describe()))
        }
    }
    fn expect_kw(&mut self, kw: &str) -> Result<(), Bad> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            bad(self.pos(), format!("这里要 {kw}，却是{}", self.describe()))
        }
    }
    fn describe(&self) -> String {
        match self.peek() {
            Tok::Name(n) => format!(" {n}"),
            Tok::Int(n) => format!("数字 {n}"),
            Tok::Float(f) => format!("数字 {f}"),
            Tok::Str(s) => format!("字符串 {s:?}"),
            Tok::Op(o) => format!(" {o}"),
            Tok::Newline => "换行".to_string(),
            Tok::End => "结尾".to_string(),
        }
    }
    fn unexpected<T>(&self) -> Result<T, Bad> {
        bad(self.pos(), format!("这里不该是{}", self.describe()))
    }
    fn at_stmt_end(&self) -> bool {
        matches!(self.peek(), Tok::End | Tok::Newline | Tok::Op(";"))
    }
    /// 嵌套加一层；太深就停（递归下降，一层要用不少栈）
    fn enter(&mut self) -> Result<(), Bad> {
        self.depth += 1;
        if self.depth > MAX_NEST {
            return bad(self.pos(), "表达式嵌套太深");
        }
        Ok(())
    }

    fn program(&mut self, mode: Mode) -> Result<Program, Bad> {
        let mut stmts = Vec::new();
        loop {
            while matches!(self.peek(), Tok::Newline) {
                self.bump();
            }
            if matches!(self.peek(), Tok::End) {
                break;
            }
            // 一行：语句 (; 语句)* [;]——分号前后都得有语句，只有行尾能多一个分号
            loop {
                stmts.push(self.stmt()?);
                if !self.eat_op(";") || matches!(self.peek(), Tok::Newline | Tok::End) {
                    break;
                }
            }
            if !matches!(self.peek(), Tok::Newline | Tok::End) {
                return self.unexpected();
            }
        }
        match mode {
            Mode::Stmts => Ok(Program { stmts, last: None }),
            Mode::Expr => match stmts.pop() {
                Some(Stmt::Expr(e)) => Ok(Program { stmts, last: Some(e) }),
                Some(_) => bad(self.pos(), "程序的最后要是一个表达式（它的值就是结果）"),
                None => bad(0, "程序是空的"),
            },
        }
    }

    fn stmt(&mut self) -> Result<Stmt, Bad> {
        let start = self.pos();
        if let Tok::Name(n) = self.peek() {
            match n.as_str() {
                "pass" => {
                    self.bump();
                    return Ok(Stmt::Pass);
                }
                "import" => return self.import(),
                "del" => {
                    self.bump();
                    let e = self.exprlist()?;
                    return Ok(Stmt::Del(match to_target(e, start)? {
                        Target::Tuple(items) => items,
                        one => vec![one],
                    }));
                }
                "assert" => {
                    self.bump();
                    let cond = self.expr()?;
                    let msg = if self.eat_op(",") { Some(self.expr()?) } else { None };
                    return Ok(Stmt::Assert(cond, msg));
                }
                "if" | "for" | "while" | "def" | "class" | "with" | "try" | "async" | "return" | "raise" | "global"
                | "nonlocal" | "break" | "continue" | "from" | "yield" | "elif" | "else" | "except" | "finally" => {
                    return bad(start, format!("不支持 {n} 语句（只能写简单语句：表达式、赋值、del、assert）"));
                }
                _ => {}
            }
        }
        let first = self.exprlist()?;
        if self.is_op("=") {
            let mut targets = vec![to_target(first, start)?];
            loop {
                self.bump();
                let at = self.pos();
                let e = self.exprlist()?;
                if self.is_op("=") {
                    targets.push(to_target(e, at)?);
                } else {
                    return Ok(Stmt::Assign(targets, e));
                }
            }
        }
        let aug = match self.peek() {
            Tok::Op("+=") => Some(BinOp::Add),
            Tok::Op("-=") => Some(BinOp::Sub),
            Tok::Op("*=") => Some(BinOp::Mul),
            Tok::Op("/=") => Some(BinOp::Div),
            Tok::Op("//=") => Some(BinOp::FloorDiv),
            Tok::Op("%=") => Some(BinOp::Mod),
            Tok::Op("**=") => Some(BinOp::Pow),
            Tok::Op("|=") => Some(BinOp::BitOr),
            Tok::Op("^=") => Some(BinOp::BitXor),
            Tok::Op("&=") => Some(BinOp::BitAnd),
            Tok::Op(op @ (">>=" | "<<=" | "@=")) => return bad(self.pos(), format!("不支持 {op}")),
            _ => None,
        };
        if let Some(op) = aug {
            self.bump();
            let target = to_target(first, start)?;
            if matches!(target, Target::Tuple(_)) {
                return bad(start, "增量赋值的左边不能是元组");
            }
            return Ok(Stmt::Aug(target, op, self.exprlist()?));
        }
        if self.is_op(":") {
            return bad(self.pos(), "不支持带类型标注的赋值");
        }
        Ok(Stmt::Expr(first))
    }

    fn import(&mut self) -> Result<Stmt, Bad> {
        self.bump();
        let mut names = Vec::new();
        loop {
            let at = self.pos();
            let Tok::Name(n) = self.bump() else { return bad(at, "import 后面要是模块名") };
            if self.is_op(".") || self.is_kw("as") {
                return bad(at, "import 只支持 import json, sys, time 这一种写法");
            }
            if !MODULES.contains(&n.as_str()) {
                return bad(at, format!("不能 import {n}：这里只有 json、sys、time"));
            }
            names.push(Rc::from(n));
            if !self.eat_op(",") {
                return Ok(Stmt::Import(names));
            }
        }
    }

    /// 语句一级的表达式：不带括号的 a, b 是元组
    fn exprlist(&mut self) -> Result<Expr, Bad> {
        let first = self.expr()?;
        if !self.is_op(",") {
            return Ok(first);
        }
        let mut items = vec![first];
        while self.eat_op(",") {
            if self.at_stmt_end() || matches!(self.peek(), Tok::Op(op) if op.ends_with('=')) {
                break;
            }
            items.push(self.expr()?);
        }
        Ok(Expr::Tuple(items))
    }

    fn expr(&mut self) -> Result<Expr, Bad> {
        self.enter()?;
        let e = self.expr_inner();
        self.depth -= 1;
        e
    }

    fn expr_inner(&mut self) -> Result<Expr, Bad> {
        if self.is_kw("lambda") {
            return self.lambda();
        }
        if self.is_op("*") || self.is_op("**") {
            return bad(self.pos(), "不支持 * / ** 展开");
        }
        let e = self.or_test()?;
        if self.eat_kw("if") {
            let cond = self.or_test()?;
            self.expect_kw("else")?;
            let other = self.expr()?;
            return Ok(Expr::IfElse(Box::new(cond), Box::new(e), Box::new(other)));
        }
        if self.is_op(":=") {
            return bad(self.pos(), "不支持 :=（赋值表达式）");
        }
        Ok(e)
    }

    fn lambda(&mut self) -> Result<Expr, Bad> {
        let start = self.pos();
        self.bump();
        let mut params: Vec<Rc<str>> = Vec::new();
        let mut defaults = Vec::new();
        while !self.is_op(":") {
            let at = self.pos();
            match self.bump() {
                Tok::Name(n) if !KEYWORDS.contains(&n.as_str()) => {
                    if params.iter().any(|p| **p == *n) {
                        return bad(at, format!("lambda 的参数 {n} 重复了"));
                    }
                    params.push(Rc::from(n));
                }
                Tok::Op("*" | "**" | "/") => return bad(at, "lambda 只支持位置参数和默认值（不支持 *、**、/）"),
                _ => return bad(at, "lambda 的参数写得不对"),
            }
            if self.eat_op("=") {
                defaults.push(self.expr()?);
            } else if !defaults.is_empty() {
                return bad(at, "没有默认值的参数不能排在有默认值的后面");
            }
            if !self.eat_op(",") && !self.is_op(":") {
                return bad(self.pos(), "lambda 的参数写得不对");
            }
        }
        self.expect_op(":")?;
        if self.is_kw("yield") {
            return bad(start, "不支持 yield");
        }
        let body = self.expr()?;
        Ok(Expr::Lambda(Rc::new(LambdaDef { params, defaults, body })))
    }

    fn or_test(&mut self) -> Result<Expr, Bad> {
        let first = self.and_test()?;
        if !self.is_kw("or") {
            return Ok(first);
        }
        let mut items = vec![first];
        while self.eat_kw("or") {
            items.push(self.and_test()?);
        }
        Ok(Expr::Or(items))
    }

    fn and_test(&mut self) -> Result<Expr, Bad> {
        let first = self.not_test()?;
        if !self.is_kw("and") {
            return Ok(first);
        }
        let mut items = vec![first];
        while self.eat_kw("and") {
            items.push(self.not_test()?);
        }
        Ok(Expr::And(items))
    }

    fn not_test(&mut self) -> Result<Expr, Bad> {
        if self.is_kw("not") {
            self.enter()?;
            self.bump();
            let e = self.not_test();
            self.depth -= 1;
            return Ok(Expr::Not(Box::new(e?)));
        }
        self.comparison()
    }

    fn comparison(&mut self) -> Result<Expr, Bad> {
        let first = self.binary(1)?;
        let mut rest = Vec::new();
        loop {
            let op = match (self.peek(), self.peek_at(1)) {
                (Tok::Op("=="), _) => CmpOp::Eq,
                (Tok::Op("!="), _) => CmpOp::Ne,
                (Tok::Op("<"), _) => CmpOp::Lt,
                (Tok::Op("<="), _) => CmpOp::Le,
                (Tok::Op(">"), _) => CmpOp::Gt,
                (Tok::Op(">="), _) => CmpOp::Ge,
                (Tok::Name(a), Tok::Name(b)) if a == "not" && b == "in" => {
                    self.bump();
                    CmpOp::NotIn
                }
                (Tok::Name(a), Tok::Name(b)) if a == "is" && b == "not" => {
                    self.bump();
                    CmpOp::IsNot
                }
                (Tok::Name(a), _) if a == "in" => CmpOp::In,
                (Tok::Name(a), _) if a == "is" => CmpOp::Is,
                _ => break,
            };
            self.bump();
            rest.push((op, self.binary(1)?));
        }
        Ok(if rest.is_empty() { first } else { Expr::Compare(Box::new(first), rest) })
    }

    /// 二元运算（优先级爬升）：| 最低，然后 ^、&、移位、+ -、* / // %
    fn binary(&mut self, min: u8) -> Result<Expr, Bad> {
        let mut left = self.factor()?;
        let mut chain = 0;
        loop {
            let (prec, op) = match self.peek() {
                Tok::Op("|") => (1, BinOp::BitOr),
                Tok::Op("^") => (2, BinOp::BitXor),
                Tok::Op("&") => (3, BinOp::BitAnd),
                Tok::Op(op @ ("<<" | ">>" | "@")) => return bad(self.pos(), format!("不支持 {op} 运算")),
                Tok::Op("+") => (5, BinOp::Add),
                Tok::Op("-") => (5, BinOp::Sub),
                Tok::Op("*") => (6, BinOp::Mul),
                Tok::Op("/") => (6, BinOp::Div),
                Tok::Op("//") => (6, BinOp::FloorDiv),
                Tok::Op("%") => (6, BinOp::Mod),
                _ => break,
            };
            if prec < min {
                break;
            }
            chain += 1;
            if chain > MAX_CHAIN {
                return bad(self.pos(), "表达式太长");
            }
            self.bump();
            let right = self.binary(prec + 1)?;
            left = Expr::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn factor(&mut self) -> Result<Expr, Bad> {
        let neg = match self.peek() {
            Tok::Op("-") => true,
            Tok::Op("+") => false,
            Tok::Op("~") => return bad(self.pos(), "不支持 ~ 运算"),
            _ => return self.power(),
        };
        self.enter()?;
        self.bump();
        let e = self.factor();
        self.depth -= 1;
        Ok(if neg { Expr::Neg(Box::new(e?)) } else { Expr::Pos(Box::new(e?)) })
    }

    fn power(&mut self) -> Result<Expr, Bad> {
        if self.is_kw("await") {
            return bad(self.pos(), "不支持 await");
        }
        let base = self.primary()?;
        if self.eat_op("**") {
            self.enter()?;
            let exp = self.factor();
            self.depth -= 1;
            return Ok(Expr::Bin(BinOp::Pow, Box::new(base), Box::new(exp?)));
        }
        Ok(base)
    }

    fn primary(&mut self) -> Result<Expr, Bad> {
        let mut e = self.atom()?;
        loop {
            if self.eat_op("(") {
                e = self.call(e)?;
            } else if self.eat_op("[") {
                let mut items = Vec::new();
                let mut tuple = false;
                loop {
                    items.push(self.slice_item()?);
                    if !self.eat_op(",") {
                        break;
                    }
                    tuple = true;
                    if self.is_op("]") {
                        break;
                    }
                }
                self.expect_op("]")?;
                let idx = match items.pop() {
                    Some(one) if !tuple => one,
                    Some(last) => {
                        items.push(last);
                        if items.iter().any(|x| matches!(x, Expr::Slice(..))) {
                            return bad(self.pos(), "不支持多维切片");
                        }
                        Expr::Tuple(items)
                    }
                    None => return self.unexpected(),
                };
                e = Expr::Index(Box::new(e), Box::new(idx));
            } else if self.eat_op(".") {
                let at = self.pos();
                match self.bump() {
                    Tok::Name(n) if !KEYWORDS.contains(&n.as_str()) => e = Expr::Attr(Box::new(e), Rc::from(n)),
                    _ => return bad(at, "点后面要是名字"),
                }
            } else {
                return Ok(e);
            }
        }
    }

    fn slice_item(&mut self) -> Result<Expr, Bad> {
        let lower = if self.is_op(":") { None } else { Some(self.expr()?) };
        if !self.eat_op(":") {
            return match lower {
                Some(e) => Ok(e),
                None => self.unexpected(),
            };
        }
        let end = |p: &Parser| p.is_op(":") || p.is_op("]") || p.is_op(",");
        let upper = if end(self) { None } else { Some(self.expr()?) };
        let step = if self.eat_op(":") && !end(self) { Some(self.expr()?) } else { None };
        Ok(Expr::Slice(lower.map(Box::new), upper.map(Box::new), step.map(Box::new)))
    }

    fn call(&mut self, func: Expr) -> Result<Expr, Bad> {
        let mut args = Vec::new();
        let mut kwargs: Vec<(Rc<str>, Expr)> = Vec::new();
        loop {
            if self.eat_op(")") {
                break;
            }
            let at = self.pos();
            if self.is_op("*") || self.is_op("**") {
                return bad(at, "调用里不支持 * / ** 展开");
            }
            if let (Tok::Name(n), Tok::Op("=")) = (self.peek(), self.peek_at(1)) {
                let name = n.clone();
                if KEYWORDS.contains(&name.as_str()) {
                    return self.unexpected();
                }
                if kwargs.iter().any(|(k, _)| **k == *name) {
                    return bad(at, format!("关键字参数 {name} 重复了"));
                }
                self.bump();
                self.bump();
                kwargs.push((Rc::from(name), self.expr()?));
            } else {
                let v = self.expr()?;
                if self.is_kw("for") || self.is_kw("async") {
                    // f(x for x in y)：生成器表达式是唯一的参数时可以不另加括号
                    if !args.is_empty() || !kwargs.is_empty() {
                        return bad(at, "生成器表达式不是唯一的参数时要另加括号");
                    }
                    args.push(self.comp_tail(CompKind::Gen, v, None)?);
                    if !self.is_op(")") {
                        return bad(self.pos(), "生成器表达式不是唯一的参数时要另加括号");
                    }
                    self.bump();
                    break;
                }
                if !kwargs.is_empty() {
                    return bad(at, "位置参数不能跟在关键字参数后面");
                }
                args.push(v);
            }
            if !self.eat_op(",") {
                self.expect_op(")")?;
                break;
            }
        }
        Ok(Expr::Call(Box::new(func), args, kwargs))
    }

    fn atom(&mut self) -> Result<Expr, Bad> {
        let at = self.pos();
        match self.bump() {
            Tok::Int(n) => Ok(Expr::Int(n)),
            Tok::Float(f) => Ok(Expr::Float(f)),
            Tok::Str(mut text) => {
                // 挨着的几个字符串字面量拼成一个
                while let Tok::Str(more) = self.peek() {
                    text.push_str(more);
                    self.bump();
                }
                Ok(Expr::Str(Rc::from(text)))
            }
            Tok::Name(n) => match n.as_str() {
                "None" => Ok(Expr::None),
                "True" => Ok(Expr::True),
                "False" => Ok(Expr::False),
                "yield" | "await" => bad(at, format!("不支持 {n}")),
                kw if KEYWORDS.contains(&kw) => bad(at, format!("这里不该是 {kw}")),
                _ => Ok(Expr::Name(Rc::from(n))),
            },
            Tok::Op("(") => {
                if self.eat_op(")") {
                    return Ok(Expr::Tuple(Vec::new()));
                }
                let first = self.expr()?;
                if self.is_kw("for") || self.is_kw("async") {
                    let c = self.comp_tail(CompKind::Gen, first, None)?;
                    self.expect_op(")")?;
                    return Ok(c);
                }
                if self.eat_op(")") {
                    return Ok(first);
                }
                Ok(Expr::Tuple(self.items(first, ")")?))
            }
            Tok::Op("[") => {
                if self.eat_op("]") {
                    return Ok(Expr::List(Vec::new()));
                }
                let first = self.expr()?;
                if self.is_kw("for") || self.is_kw("async") {
                    let c = self.comp_tail(CompKind::List, first, None)?;
                    self.expect_op("]")?;
                    return Ok(c);
                }
                Ok(Expr::List(self.items(first, "]")?))
            }
            Tok::Op("{") => {
                if self.eat_op("}") {
                    return Ok(Expr::Dict(Vec::new()));
                }
                let first = self.expr()?;
                if !self.eat_op(":") {
                    if self.is_kw("for") || self.is_kw("async") {
                        let c = self.comp_tail(CompKind::Set, first, None)?;
                        self.expect_op("}")?;
                        return Ok(c);
                    }
                    return Ok(Expr::Set(self.items(first, "}")?));
                }
                let value = self.expr()?;
                if self.is_kw("for") || self.is_kw("async") {
                    let c = self.comp_tail(CompKind::Dict, first, Some(value))?;
                    self.expect_op("}")?;
                    return Ok(c);
                }
                let mut pairs = vec![(first, value)];
                while self.eat_op(",") {
                    if self.is_op("}") {
                        break;
                    }
                    let k = self.expr()?;
                    self.expect_op(":")?;
                    pairs.push((k, self.expr()?));
                }
                self.expect_op("}")?;
                Ok(Expr::Dict(pairs))
            }
            Tok::Op("...") => bad(at, "不支持 ...（Ellipsis）"),
            Tok::Op("*" | "**") => bad(at, "不支持 * / ** 展开"),
            _ => self.unexpected_at(at),
        }
    }

    fn unexpected_at<T>(&mut self, at: usize) -> Result<T, Bad> {
        // 回到出问题的那个记号上，说它是什么
        if let Some(i) = self.toks.iter().position(|t| t.pos == at) {
            self.i = i;
        }
        self.unexpected()
    }

    /// 括号里第一项后面的那些项（逗号分开，最后可以多一个逗号），读到右括号为止
    fn items(&mut self, first: Expr, close: &str) -> Result<Vec<Expr>, Bad> {
        let mut items = vec![first];
        while self.eat_op(",") {
            if self.is_op(close) {
                break;
            }
            items.push(self.expr()?);
        }
        self.expect_op(close)?;
        Ok(items)
    }

    /// 元素后面的 for … in … if … 一串
    fn comp_tail(&mut self, kind: CompKind, elt: Expr, value: Option<Expr>) -> Result<Expr, Bad> {
        let mut clauses = Vec::new();
        let mut names: Vec<Rc<str>> = Vec::new();
        loop {
            if self.is_kw("async") {
                return bad(self.pos(), "不支持 async for");
            }
            if !self.eat_kw("for") {
                break;
            }
            let target = self.target_list()?;
            target_names(&target, &mut names);
            self.expect_kw("in")?;
            let iter = self.or_test()?;
            let mut ifs = Vec::new();
            while self.eat_kw("if") {
                ifs.push(self.or_test()?);
            }
            clauses.push(Clause { target, iter, ifs });
        }
        Ok(Expr::Comp(Rc::new(Comp { kind, elt, value, clauses, names })))
    }

    fn target_list(&mut self) -> Result<Target, Bad> {
        let first = self.target()?;
        if !self.is_op(",") {
            return Ok(first);
        }
        let mut items = vec![first];
        while self.eat_op(",") {
            if self.is_kw("in") {
                break;
            }
            items.push(self.target()?);
        }
        Ok(Target::Tuple(items))
    }

    fn target(&mut self) -> Result<Target, Bad> {
        let at = self.pos();
        self.enter()?;
        let t = match self.bump() {
            Tok::Name(n) if !KEYWORDS.contains(&n.as_str()) => {
                if self.is_op(".") || self.is_op("[") || self.is_op("(") {
                    return bad(at, "for 的循环变量只支持名字和元组");
                }
                Target::Name(Rc::from(n))
            }
            Tok::Op(open @ ("(" | "[")) => {
                let close = if open == "(" { ")" } else { "]" };
                let mut items = Vec::new();
                let mut tuple = open == "[";
                while !self.is_op(close) {
                    items.push(self.target()?);
                    if !self.eat_op(",") {
                        break;
                    }
                    tuple = true;
                }
                self.expect_op(close)?;
                match items.pop() {
                    Some(one) if !tuple => one,
                    Some(last) => {
                        items.push(last);
                        Target::Tuple(items)
                    }
                    None => Target::Tuple(items),
                }
            }
            Tok::Op("*") => return bad(at, "不支持带 * 的解包"),
            _ => return bad(at, "for 后面要是名字或者元组"),
        };
        self.depth -= 1;
        Ok(t)
    }
}

fn target_names(t: &Target, out: &mut Vec<Rc<str>>) {
    match t {
        Target::Name(n) => {
            if !out.contains(n) {
                out.push(n.clone());
            }
        }
        Target::Tuple(items) => items.iter().for_each(|x| target_names(x, out)),
        Target::Index(..) => {}
    }
}

/// 表达式改成赋值的目标：名字、元组、下标
fn to_target(e: Expr, at: usize) -> Result<Target, Bad> {
    match e {
        Expr::Name(n) => Ok(Target::Name(n)),
        Expr::Tuple(items) | Expr::List(items) => {
            Ok(Target::Tuple(items.into_iter().map(|x| to_target(x, at)).collect::<Result<_, _>>()?))
        }
        Expr::Index(_, idx) if matches!(*idx, Expr::Slice(..)) => bad(at, "不支持给切片赋值、删切片"),
        Expr::Index(obj, idx) => Ok(Target::Index(*obj, *idx)),
        Expr::Attr(..) => bad(at, "不支持给属性赋值"),
        _ => bad(at, "这个表达式不能放在赋值的左边"),
    }
}
