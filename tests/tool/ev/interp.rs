//! 求值器：表达式、语句、调用。全局变量在这里；lambda 的参数、推导式的循环变量各有一层作用域（value::Scope）。

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use super::ast::*;
use super::ops::{self, as_int, truthy};
use super::value::*;
use super::{builtins, jsonio, methods, names, pyfmt};

/// 表达式、迭代器一层套一层最多这么深（再深怕把栈用完；线程的栈是照这个数留的）
const MAX_DEPTH: usize = 20_000;
/// lambda 调用最多套这么多层（Python 自己在 1000 层左右报 RecursionError，具体哪一层说不准）
const MAX_CALLS: usize = 900;
/// 总共最多走这么多步：断言都是一眨眼的事，走到这个数是写错了
const MAX_STEPS: u64 = 50_000_000;

pub type Kw = Vec<(Rc<str>, V)>;

/// a op= b 出错时 Python 说的是 op=（“unsupported operand type(s) for +=: …”）
fn aug_error(e: Stop, op: BinOp) -> Stop {
    let Stop::Raise(Exc { kind: Kind::Type, msg }) = e else { return e };
    let plain = format!("unsupported operand type(s) for {}: ", if op == BinOp::Pow { "** or pow()" } else { op.symbol() });
    let msg = match msg.strip_prefix(&plain) {
        Some(rest) => format!("unsupported operand type(s) for {}=: {rest}", op.symbol()),
        None => msg,
    };
    Stop::Raise(Exc { kind: Kind::Type, msg })
}

/// Python 的 sys.stdout / sys.stderr 什么时候真的把字写出去，这里照着来：两路接到一起（2>&1）时谁先谁后、
/// 读的那一头关了时退出码是几，都由它定。
/// 行缓冲的（stderr 一直是；stdout 接着终端时是）：这一次写的内容里有换行，就把攒着的全写出去。
/// 块缓冲的（stdout 接着管道、文件时）：一直攒着，结束时才写——所以 Python 里异常的那一行在 print 的内容前面。
/// 攒得多了也会先写出去一部分，哪一部分照 Python 的两层来：上面一层（TextIOWrapper）够 CHUNK 了才交给下面一层，
/// 下面一层（BufferedWriter）放不下了才真的写。两层的规矩是拿 Python 3.14 一个个字节数量出来的（8192 上下、128 KiB 上下）。
struct Sink {
    w: Box<dyn Write>,
    /// 上面一层攒着的
    text: Vec<u8>,
    /// 下面一层攒着的
    buf: Vec<u8>,
    line: bool,
}

/// 上面一层攒到这么多字节就交给下面一层
const CHUNK: usize = 8192;
/// 下面一层的缓冲区（Python 3.14 起是 128 KiB；文件系统报的块更大时 Python 用更大的，这里不管）
const BLOCK: usize = 128 << 10;

impl Sink {
    fn new(w: Box<dyn Write>, line: bool) -> Sink {
        Sink { w, text: Vec::new(), buf: Vec::new(), line }
    }

    /// 一次 write：print 的每个参数、每个分隔、结尾各是一次（从哪里断开决定了攒到哪里算够）
    fn put(&mut self, piece: &str) -> std::io::Result<()> {
        // 这一块自己就够大：攒着的先交下去，不和它接在一起
        if piece.len() >= CHUNK && !self.text.is_empty() {
            self.hand_down()?;
        }
        self.text.extend_from_slice(piece.as_bytes());
        let newline = self.line && piece.contains(['\n', '\r']);
        if self.text.len() >= CHUNK || newline {
            self.hand_down()?;
        }
        if newline {
            self.write_out()?;
        }
        Ok(())
    }

    /// 上面一层攒着的交给下面一层：放得下就放着（正好有缓冲区那么大的不放）；放不下，先把下面攒着的写出去，
    /// 这一块有缓冲区那么大就直接写。交出去的不管写没写成上面都不留（Python 也是这样，所以直接写没写成的那一块就丢了）
    fn hand_down(&mut self) -> std::io::Result<()> {
        let data = std::mem::take(&mut self.text);
        if data.len() <= BLOCK - self.buf.len() && data.len() < BLOCK {
            self.buf.extend_from_slice(&data);
            return Ok(());
        }
        self.write_out()?;
        if data.len() >= BLOCK {
            return self.w.write_all(&data).and_then(|()| self.w.flush());
        }
        self.buf = data;
        Ok(())
    }

    /// 下面一层攒着的真的写出去；写不出去的留着，结束时再试一次（Python 也是）
    fn write_out(&mut self) -> std::io::Result<()> {
        if !self.buf.is_empty() {
            self.w.write_all(&self.buf)?;
            self.buf.clear();
        }
        self.w.flush()
    }

    /// flush()：两层攒着的都写出去
    fn flush(&mut self) -> std::io::Result<()> {
        self.hand_down()?;
        self.write_out()
    }
}

pub struct Interp {
    globals: Vec<(Rc<str>, V)>,
    out: Sink,
    err: Sink,
    depth: usize,
    calls: usize,
    steps: u64,
}

impl Interp {
    /// line_out：stdout 接着终端（行缓冲）；不然是块缓冲
    pub fn new(out: Box<dyn Write>, err: Box<dyn Write>, line_out: bool) -> Interp {
        Interp { globals: Vec::new(), out: Sink::new(out, line_out), err: Sink::new(err, true), depth: 0, calls: 0, steps: 0 }
    }

    pub fn set_global(&mut self, name: &str, v: V) {
        match self.globals.iter_mut().find(|(n, _)| &**n == name) {
            Some(slot) => slot.1 = v,
            None => self.globals.push((Rc::from(name), v)),
        }
    }

    pub fn global(&self, name: &str) -> Option<V> {
        self.globals.iter().find(|(n, _)| &**n == name).map(|(_, v)| v.clone())
    }

    /// 走了 n 步（一口气造出一大批东西的地方自己报数）
    pub fn charge(&mut self, n: u64) -> R<()> {
        self.steps = self.steps.saturating_add(n);
        if self.steps > MAX_STEPS {
            return unsupported("算得太久（步数超过上限）");
        }
        Ok(())
    }

    /// 往里套一层；出来时 leave
    pub fn enter(&mut self) -> R<()> {
        self.charge(1)?;
        if self.depth >= MAX_DEPTH {
            return unsupported("嵌套太深");
        }
        self.depth += 1;
        Ok(())
    }

    pub fn leave(&mut self) {
        self.depth -= 1;
    }

    fn sink(&mut self, to: Stream) -> &mut Sink {
        match to {
            Stream::Stdout => &mut self.out,
            Stream::Stderr => &mut self.err,
        }
    }

    /// print 的字、val 的结果：交给那一路输出，一次调用是 Python 的一次 write（什么时候真的写出去见 Sink）
    pub fn write(&mut self, to: Stream, piece: &str) -> R<()> {
        self.sink(to).put(piece).map_err(|e| Stop::Raise(jsonio::os_error(&e)))
    }

    /// print(…, flush=True)
    pub fn flush(&mut self, to: Stream) -> R<()> {
        self.sink(to).flush().map_err(|e| Stop::Raise(jsonio::os_error(&e)))
    }

    /// 往 stderr 说一句（异常的那一行、sys.exit 的话、“cca-test：…”）；写不出去就算了
    pub fn say(&mut self, piece: &str) {
        let _ = self.err.put(piece);
    }

    /// 结束时把攒着的写出去：先 stdout 后 stderr（Python 退出时也是这个先后）。有一路写不出去（读的那一头已经关了）时
    /// Python 的退出码变成 120，stdout 的还在 stderr 上说两行——返回的就是这时该用的退出码
    pub fn finish(&mut self) -> Option<i32> {
        let out = self.out.flush();
        if let Err(e) = &out {
            self.say(&format!("Exception ignored while flushing sys.stdout:\n{}\n", jsonio::os_error(e).line()));
        }
        let err = self.err.flush();
        (out.is_err() || err.is_err()).then_some(120)
    }

    // ---- 程序和语句 ----

    /// 跑一段程序：语句一条条来，最后那个表达式的值是结果（只有语句的没有）
    pub fn run(&mut self, p: &Program) -> R<Option<V>> {
        for s in &p.stmts {
            self.exec(s)?;
        }
        match &p.last {
            Some(e) => Ok(Some(self.eval(e, &None)?)),
            None => Ok(None),
        }
    }

    fn exec(&mut self, s: &Stmt) -> R<()> {
        match s {
            Stmt::Expr(e) => {
                self.eval(e, &None)?;
            }
            Stmt::Assign(targets, value) => {
                let v = self.eval(value, &None)?;
                for t in targets {
                    self.assign(t, v.clone())?;
                }
            }
            Stmt::Aug(target, op, value) => self.aug(target, *op, value)?,
            Stmt::Del(targets) => {
                for t in targets {
                    self.del(t)?;
                }
            }
            Stmt::Assert(cond, msg) => {
                if !truthy(&self.eval(cond, &None)?) {
                    let text = match msg {
                        Some(m) => pyfmt::str_of(&self.eval(m, &None)?)?,
                        None => String::new(),
                    };
                    return raise(Kind::Assertion, text);
                }
            }
            Stmt::Import(mods) => {
                for n in mods {
                    let m = match &**n {
                        "json" => Mod::Json,
                        "sys" => Mod::Sys,
                        _ => Mod::Time,
                    };
                    self.set_global(n, V::Module(m));
                }
            }
            Stmt::Pass => {}
        }
        Ok(())
    }

    fn assign(&mut self, t: &Target, v: V) -> R<()> {
        match t {
            Target::Name(n) => {
                self.set_global(n, v);
                Ok(())
            }
            Target::Tuple(items) => {
                let parts = self.unpack(&v, items.len())?;
                for (t, p) in items.iter().zip(parts) {
                    self.assign(t, p)?;
                }
                Ok(())
            }
            Target::Index(obj, idx) => {
                let o = self.eval(obj, &None)?;
                let i = self.eval(idx, &None)?;
                ops::setitem(&o, &i, v)
            }
        }
    }

    fn aug(&mut self, target: &Target, op: BinOp, value: &Expr) -> R<()> {
        match target {
            Target::Name(n) => {
                let cur = self.lookup(n, &None)?;
                let rhs = self.eval(value, &None)?;
                let new = self.inplace(op, &cur, &rhs)?;
                self.set_global(n, new);
                Ok(())
            }
            Target::Index(obj, idx) => {
                let o = self.eval(obj, &None)?;
                let i = self.eval(idx, &None)?;
                let cur = ops::getitem(&o, &i)?;
                let rhs = self.eval(value, &None)?;
                let new = self.inplace(op, &cur, &rhs)?;
                ops::setitem(&o, &i, new)
            }
            Target::Tuple(_) => unsupported("增量赋值的左边是元组"),
        }
    }

    /// a op= b：列表、字典、集合能就地改的就地改（别的名字指着同一个对象的也看得到），别的和 a op b 一样
    fn inplace(&mut self, op: BinOp, a: &V, b: &V) -> R<V> {
        match (op, a) {
            (BinOp::Add, V::List(l)) => {
                let items = self.collect(b)?;
                l.borrow_mut().extend(items);
                Ok(a.clone())
            }
            (BinOp::Mul, V::List(l)) => {
                let Some(n) = as_int(b) else {
                    return type_err(format!("can't multiply sequence by non-int of type '{}'", type_name(b)));
                };
                let repeated = ops::binop(BinOp::Mul, a, &V::Int(n))?;
                if let V::List(r) = &repeated {
                    let items = r.borrow().clone();
                    *l.borrow_mut() = items;
                }
                Ok(a.clone())
            }
            (BinOp::BitOr, V::Dict(d)) => {
                methods::dict_update(self, d, b)?;
                Ok(a.clone())
            }
            (BinOp::BitOr | BinOp::BitAnd | BinOp::BitXor | BinOp::Sub, V::Set(s)) if !s.frozen && matches!(b, V::Set(_)) => {
                if let V::Set(r) = ops::binop(op, a, b)? {
                    let new = r.d.borrow().copy();
                    s.d.borrow_mut().replace_with(new);
                }
                Ok(a.clone())
            }
            // 左边有序列的那一套方法、却不能重复（字典、集合、range、视图）：a *= 序列 时 CPython 不再去试右边
            (BinOp::Mul, V::Dict(_) | V::Set(_) | V::Range(_) | V::View(_))
                if matches!(b, V::Str(_) | V::List(_) | V::Tuple(_)) =>
            {
                type_err(format!("unsupported operand type(s) for *=: '{}' and '{}'", type_name(a), type_name(b)))
            }
            _ => self.binop(op, a, b).map_err(|e| aug_error(e, op)),
        }
    }

    fn del(&mut self, t: &Target) -> R<()> {
        match t {
            Target::Name(n) => match self.globals.iter().position(|(g, _)| **g == **n) {
                Some(i) => {
                    self.globals.remove(i);
                    Ok(())
                }
                None => unsupported(format!("del {n}：名字 {n} 没有定义")),
            },
            Target::Tuple(items) => {
                for x in items {
                    self.del(x)?;
                }
                Ok(())
            }
            Target::Index(obj, idx) => {
                let o = self.eval(obj, &None)?;
                let i = self.eval(idx, &None)?;
                ops::delitem(&o, &i)
            }
        }
    }

    // ---- 表达式 ----

    fn lookup(&self, name: &Rc<str>, env: &Env) -> R<V> {
        let mut cur = env;
        while let Some(scope) = cur {
            for (n, v) in scope.vars.borrow().iter() {
                if **n == **name {
                    return match v {
                        Some(v) => Ok(v.clone()),
                        None => raise(
                            Kind::UnboundLocal,
                            format!("cannot access local variable '{name}' where it is not associated with a value"),
                        ),
                    };
                }
            }
            cur = &scope.parent;
        }
        if let Some((_, v)) = self.globals.iter().find(|(n, _)| **n == **name) {
            return Ok(v.clone());
        }
        match builtins::builtin(name) {
            Some(v) => Ok(v),
            None if names::BUILTINS.contains(&&**name) => unsupported(format!("Python 内建的 {name} 这里没实现")),
            None => unsupported(format!("名字 {name} 没有定义")),
        }
    }

    pub fn eval(&mut self, e: &Expr, env: &Env) -> R<V> {
        // 字面量和名字不往下套，不算一层
        match e {
            Expr::None => return Ok(V::None),
            Expr::True => return Ok(V::Bool(true)),
            Expr::False => return Ok(V::Bool(false)),
            Expr::Int(n) => return Ok(V::Int(*n)),
            Expr::Float(f) => return Ok(V::Float(*f)),
            Expr::Str(s) => return Ok(V::Str(s.clone())),
            Expr::Name(n) => return self.lookup(n, env),
            _ => {}
        }
        self.enter()?;
        let r = self.eval_node(e, env);
        self.leave();
        r
    }

    fn eval_list(&mut self, items: &[Expr], env: &Env) -> R<Vec<V>> {
        let mut out = Vec::with_capacity(items.len());
        for x in items {
            out.push(self.eval(x, env)?);
        }
        Ok(out)
    }

    fn eval_opt(&mut self, e: &Option<Box<Expr>>, env: &Env) -> R<Option<V>> {
        match e {
            Some(e) => Ok(Some(self.eval(e, env)?)),
            None => Ok(None),
        }
    }

    fn eval_node(&mut self, e: &Expr, env: &Env) -> R<V> {
        match e {
            Expr::Tuple(items) => Ok(V::tuple(self.eval_list(items, env)?)),
            Expr::List(items) => Ok(V::list(self.eval_list(items, env)?)),
            Expr::Set(items) => {
                // 先都算出来再放（Python 也是）；相等的元素留先来的
                let values = self.eval_list(items, env)?;
                let mut d = Dict::new();
                for v in values {
                    d.add(ops::set_elem(&v)?, v, V::None);
                }
                Ok(V::set(d, false))
            }
            Expr::Dict(pairs) => {
                let mut values = Vec::with_capacity(pairs.len());
                for (k, v) in pairs {
                    let key = self.eval(k, env)?;
                    values.push((key, self.eval(v, env)?));
                }
                let mut d = Dict::new();
                for (k, v) in values {
                    d.set(ops::dict_key(&k)?, k, v);
                }
                Ok(V::dict(d))
            }
            Expr::Attr(obj, name) => {
                let o = self.eval(obj, env)?;
                methods::get_attr(&o, name)
            }
            Expr::Index(obj, idx) => {
                let o = self.eval(obj, env)?;
                if let Expr::Slice(a, b, c) = &**idx {
                    let (a, b, c) = (self.eval_opt(a, env)?, self.eval_opt(b, env)?, self.eval_opt(c, env)?);
                    return ops::get_slice(&o, &a, &b, &c);
                }
                let i = self.eval(idx, env)?;
                ops::getitem(&o, &i)
            }
            Expr::Slice(..) => unsupported("切片只能写在下标里"),
            Expr::Call(f, args, kwargs) => self.eval_call(f, args, kwargs, env),
            Expr::Not(x) => Ok(V::Bool(!truthy(&self.eval(x, env)?))),
            Expr::Neg(x) => ops::neg(&self.eval(x, env)?),
            Expr::Pos(x) => ops::pos(&self.eval(x, env)?),
            Expr::Bin(op, l, r) => {
                let a = self.eval(l, env)?;
                let b = self.eval(r, env)?;
                self.binop(*op, &a, &b)
            }
            Expr::And(items) => {
                let mut last = V::Bool(true);
                for x in items {
                    last = self.eval(x, env)?;
                    if !truthy(&last) {
                        break;
                    }
                }
                Ok(last)
            }
            Expr::Or(items) => {
                let mut last = V::Bool(false);
                for x in items {
                    last = self.eval(x, env)?;
                    if truthy(&last) {
                        break;
                    }
                }
                Ok(last)
            }
            Expr::Compare(first, rest) => {
                let mut left = self.eval(first, env)?;
                for (op, rhs) in rest {
                    let right = self.eval(rhs, env)?;
                    if !self.compare(*op, &left, &right)? {
                        return Ok(V::Bool(false));
                    }
                    left = right;
                }
                Ok(V::Bool(true))
            }
            Expr::IfElse(cond, then, other) => {
                if truthy(&self.eval(cond, env)?) {
                    self.eval(then, env)
                } else {
                    self.eval(other, env)
                }
            }
            Expr::Lambda(def) => {
                let defaults = self.eval_list(&def.defaults, env)?;
                Ok(V::Func(Rc::new(Lambda { def: def.clone(), defaults, env: env.clone() })))
            }
            Expr::Comp(def) => self.eval_comp(def, env),
            Expr::None | Expr::True | Expr::False | Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Name(_) => {
                self.eval(e, env)
            }
        }
    }

    pub fn compare(&mut self, op: CmpOp, a: &V, b: &V) -> R<bool> {
        match op {
            CmpOp::Eq => ops::eq(a, b),
            CmpOp::Ne => Ok(!ops::eq(a, b)?),
            CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge => ops::order(op, a, b, false),
            CmpOp::In => self.contains(b, a),
            CmpOp::NotIn => Ok(!self.contains(b, a)?),
            CmpOp::Is => ops::identical(a, b),
            CmpOp::IsNot => Ok(!ops::identical(a, b)?),
        }
    }

    /// x in c
    pub fn contains(&mut self, c: &V, x: &V) -> R<bool> {
        if let Some(found) = ops::contains(c, x)? {
            return Ok(found);
        }
        // 迭代器：一个个取出来比，找到就停（取走的就取走了）
        while let Some(item) = self.next(c)? {
            if ops::eq_item(&item, x)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn binop(&mut self, op: BinOp, a: &V, b: &V) -> R<V> {
        // keys() / items() 的 | & - ^：另一边可以是任何可迭代的，结果是 set
        if matches!(op, BinOp::BitOr | BinOp::BitAnd | BinOp::BitXor | BinOp::Sub) {
            let view = |v: &V| matches!(v, V::View(w) if w.kind != ViewKind::Values);
            let setlike = |v: &V| matches!(v, V::Set(_)) || view(v);
            if (view(a) && !setlike(b)) || (view(b) && !setlike(a)) {
                let (x, y) = (self.entries_of(a)?, self.entries_of(b)?);
                return ops::set_op(op, a, b, x, y);
            }
        }
        ops::binop(op, a, b)
    }

    /// 当集合用时的元素（键和值本身）：集合、keys()、items() 直接拿，别的可迭代对象一个个取
    pub fn entries_of(&mut self, v: &V) -> R<Vec<(Key, V)>> {
        if let Some(entries) = ops::set_entries(v)? {
            return Ok(entries);
        }
        let items = self.collect(v)?;
        items.into_iter().map(|x| Ok((ops::set_elem(&x)?, x))).collect()
    }

    // ---- 调用 ----

    fn eval_args(&mut self, args: &[Expr], kwargs: &[(Rc<str>, Expr)], env: &Env) -> R<(Vec<V>, Kw)> {
        let a = self.eval_list(args, env)?;
        let mut k = Vec::with_capacity(kwargs.len());
        for (name, e) in kwargs {
            k.push((name.clone(), self.eval(e, env)?));
        }
        Ok((a, k))
    }

    fn eval_call(&mut self, f: &Expr, args: &[Expr], kwargs: &[(Rc<str>, Expr)], env: &Env) -> R<V> {
        if let Expr::Attr(obj, name) = f {
            // 方法调用：先求对象、认方法（没有这个属性的话参数还没求值就报错了），再求参数
            let recv = self.eval(obj, env)?;
            let attr = methods::resolve(&recv, name)?;
            let (a, k) = self.eval_args(args, kwargs, env)?;
            return match attr {
                methods::Attr::Method(m) => methods::call(self, &recv, m, a, k),
                methods::Attr::Value(v) => self.call(&v, a, k),
            };
        }
        let fv = self.eval(f, env)?;
        let (a, k) = self.eval_args(args, kwargs, env)?;
        self.call(&fv, a, k)
    }

    pub fn call(&mut self, f: &V, args: Vec<V>, kwargs: Kw) -> R<V> {
        match f {
            V::Func(lam) => self.call_lambda(lam, args, kwargs),
            V::Builtin(b) => builtins::call(self, *b, args, kwargs),
            V::Type(t) => builtins::construct(self, *t, args, kwargs),
            V::Bound(b) => methods::call(self, &b.recv, b.name, args, kwargs),
            V::Unbound(ty, name) => {
                let mut args = args;
                if args.is_empty() {
                    return type_err(format!("unbound method {}.{name}() needs an argument", ty.name()));
                }
                let recv = args.remove(0);
                if !methods::is_instance(&recv, *ty) {
                    return type_err(format!(
                        "descriptor '{name}' for '{}' objects doesn't apply to a '{}' object",
                        ty.name(),
                        type_name(&recv)
                    ));
                }
                methods::call(self, &recv, name, args, kwargs)
            }
            _ => type_err(format!("'{}' object is not callable", type_name(f))),
        }
    }

    /// 调用只带一个参数的函数（key=、map、filter 用）
    pub fn call1(&mut self, f: &V, arg: V) -> R<V> {
        self.call(f, vec![arg], Vec::new())
    }

    fn call_lambda(&mut self, lam: &Rc<Lambda>, args: Vec<V>, kwargs: Kw) -> R<V> {
        let def = &lam.def;
        let n = def.params.len();
        let plural = |k: usize| if k == 1 { "" } else { "s" };
        if args.len() > n {
            let given = args.len();
            let takes = if lam.defaults.is_empty() {
                format!("{n} positional argument{}", plural(n))
            } else {
                format!("from {} to {n} positional arguments", n - lam.defaults.len())
            };
            return type_err(format!("<lambda>() takes {takes} but {given} {} given", if given == 1 { "was" } else { "were" }));
        }
        let mut slots: Vec<Option<V>> = args.into_iter().map(Some).collect();
        slots.resize(n, None);
        for (k, v) in kwargs {
            match def.params.iter().position(|p| **p == *k) {
                None => return type_err(format!("<lambda>() got an unexpected keyword argument '{k}'")),
                Some(i) if slots[i].is_some() => return type_err(format!("<lambda>() got multiple values for argument '{k}'")),
                Some(i) => slots[i] = Some(v),
            }
        }
        let first_default = n - lam.defaults.len();
        let mut missing: Vec<String> = Vec::new();
        for (i, slot) in slots.iter_mut().enumerate() {
            if slot.is_none() {
                if i >= first_default {
                    *slot = Some(lam.defaults[i - first_default].clone());
                } else {
                    missing.push(format!("'{}'", def.params[i]));
                }
            }
        }
        if !missing.is_empty() {
            let k = missing.len();
            let list = match k {
                1 => missing[0].clone(),
                2 => format!("{} and {}", missing[0], missing[1]),
                _ => format!("{}, and {}", missing[..k - 1].join(", "), missing[k - 1]),
            };
            return type_err(format!("<lambda>() missing {k} required positional argument{}: {list}", plural(k)));
        }
        let scope = Rc::new(Scope { vars: RefCell::new(def.params.iter().cloned().zip(slots).collect()), parent: lam.env.clone() });
        if self.calls >= MAX_CALLS {
            return unsupported("lambda 一层套一层调用得太深（Python 在 1000 层左右报 RecursionError，具体哪一层这里说不准）");
        }
        self.calls += 1;
        let r = self.eval(&def.body, &Some(scope));
        self.calls -= 1;
        r
    }

    // ---- 推导式 ----

    fn eval_comp(&mut self, def: &Rc<Comp>, env: &Env) -> R<V> {
        let mut st = self.comp_start(def, env)?;
        match def.kind {
            CompKind::Gen => Ok(V::iter(Ty::Generator, IterState::Gen(st))),
            CompKind::List => {
                let mut out = Vec::new();
                while let Some((v, _)) = self.comp_next(&mut st)? {
                    out.push(v);
                }
                Ok(V::list(out))
            }
            CompKind::Set => {
                let mut d = Dict::new();
                while let Some((v, _)) = self.comp_next(&mut st)? {
                    d.add(ops::set_elem(&v)?, v, V::None);
                }
                Ok(V::set(d, false))
            }
            CompKind::Dict => {
                let mut d = Dict::new();
                while let Some((k, v)) = self.comp_next(&mut st)? {
                    d.set(ops::dict_key(&k)?, k, v.unwrap_or(V::None));
                }
                Ok(V::dict(d))
            }
        }
    }
}
