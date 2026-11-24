//! 迭代：iter()、next()、生成器和推导式一步步往前走。都是“要一个给一个”（生成器、map、filter、zip 用多少算多少）。

use std::cell::RefCell;
use std::rc::Rc;

use super::ast::{Comp, Target};
use super::interp::Interp;
use super::ops::truthy;
use super::pyfmt::SET_ORDER;
use super::value::*;

fn not_iterable<T>(v: &V) -> R<T> {
    type_err(format!("'{}' object is not iterable", type_name(v)))
}

impl Interp {
    /// iter(v)：迭代器原样返回
    pub fn iter_of(&mut self, v: &V) -> R<V> {
        Ok(match v {
            V::Iter(_) | V::File(_) => v.clone(),
            V::List(l) => V::iter(Ty::Iterator("list_iterator"), IterState::List(l.clone(), 0)),
            V::Tuple(t) => V::iter(Ty::Iterator("tuple_iterator"), IterState::Tuple(t.clone(), 0)),
            V::Str(s) => {
                let name = if s.is_ascii() { "str_ascii_iterator" } else { "str_iterator" };
                V::iter(Ty::Iterator(name), IterState::Str(s.clone(), 0))
            }
            V::Dict(d) => {
                let (len, version) = (d.borrow().len(), d.borrow().version);
                V::iter(Ty::Iterator("dict_keyiterator"), IterState::Dict(d.clone(), ViewKind::Keys, 0, len, version))
            }
            V::View(w) => {
                let (len, version) = (w.d.borrow().len(), w.d.borrow().version);
                let name = match w.kind {
                    ViewKind::Keys => "dict_keyiterator",
                    ViewKind::Values => "dict_valueiterator",
                    ViewKind::Items => "dict_itemiterator",
                };
                V::iter(Ty::Iterator(name), IterState::Dict(w.d.clone(), w.kind, 0, len, version))
            }
            V::Set(s) => {
                let (len, version) = (s.d.borrow().len(), s.d.borrow().version);
                V::iter(Ty::Iterator("set_iterator"), IterState::Set(s.clone(), 0, len, version))
            }
            V::Range(r) => match r.len() {
                Some(n) => V::iter(Ty::Iterator("range_iterator"), IterState::Range(r.start, n, r.step)),
                None => return unsupported("range 太大（超过 128 位）"),
            },
            _ => return not_iterable(v),
        })
    }

    /// next(it)：走完了是 None（不是异常：调用的人自己决定要不要报 StopIteration）
    pub fn next(&mut self, it: &V) -> R<Option<V>> {
        match it {
            V::Iter(obj) => {
                let obj = obj.clone();
                self.enter()?;
                let r = match obj.st.try_borrow_mut() {
                    Ok(mut st) => self.advance(&mut st),
                    Err(_) if obj.ty == Ty::Generator => value_err("generator already executing"),
                    Err(_) => unsupported("迭代器在给自己算下一个值的途中又被取了一次"),
                };
                self.leave();
                r
            }
            V::File(f) => {
                self.charge(1)?;
                super::jsonio::file_line(f)
            }
            _ => type_err(format!("'{}' object is not an iterator", type_name(it))),
        }
    }

    fn advance(&mut self, st: &mut IterState) -> R<Option<V>> {
        // done：走完了，把状态换成 Done（放掉手里的对象；以后再取也还是走完了）
        let mut done = false;
        let item = match st {
            IterState::Done => None,
            IterState::List(l, i) => {
                let item = l.borrow().get(*i).cloned();
                *i += 1;
                done = item.is_none();
                item
            }
            IterState::Tuple(t, i) => {
                let item = t.get(*i).cloned();
                *i += 1;
                done = item.is_none();
                item
            }
            IterState::Str(s, pos) => match s[*pos..].chars().next() {
                Some(c) => {
                    *pos += c.len_utf8();
                    Some(V::str(c.encode_utf8(&mut [0; 4])))
                }
                None => {
                    done = true;
                    None
                }
            },
            IterState::Range(next, remaining, step) => {
                if *remaining > 0 {
                    let v = *next;
                    *remaining -= 1;
                    *next = next.wrapping_add(*step);
                    Some(V::Int(v))
                } else {
                    done = true;
                    None
                }
            }
            IterState::Dict(d, kind, i, len0, ver0) => {
                let d = d.borrow();
                if d.len() != *len0 {
                    return raise(Kind::Runtime, "dictionary changed size during iteration");
                }
                if d.version != *ver0 {
                    return unsupported("遍历字典的途中换了键（Python 这时的行为取决于散列表的细节）");
                }
                match d.entries().get(*i) {
                    Some(e) => {
                        *i += 1;
                        Some(match kind {
                            ViewKind::Keys => e.k.clone(),
                            ViewKind::Values => e.v.clone(),
                            ViewKind::Items => V::tuple(vec![e.k.clone(), e.v.clone()]),
                        })
                    }
                    None => {
                        done = true;
                        None
                    }
                }
            }
            IterState::Set(s, i, len0, ver0) => {
                let d = s.d.borrow();
                if d.len() != *len0 {
                    return raise(Kind::Runtime, "Set changed size during iteration");
                }
                if d.version != *ver0 {
                    return unsupported("遍历集合的途中换了元素");
                }
                if *len0 > 1 {
                    return unsupported(SET_ORDER);
                }
                match d.entries().get(*i) {
                    Some(e) => {
                        *i += 1;
                        Some(e.k.clone())
                    }
                    None => {
                        done = true;
                        None
                    }
                }
            }
            IterState::Rev(items) => {
                let item = items.pop();
                done = item.is_none();
                item
            }
            IterState::RevList(l, left) => {
                // 列表中途变短了就到此为止（和 CPython 的 list_reverseiterator 一样）
                let l = l.borrow();
                if *left >= 1 && *left <= l.len() {
                    *left -= 1;
                    Some(l[*left].clone())
                } else {
                    done = true;
                    None
                }
            }
            IterState::Gen(cs) => match self.comp_next(cs) {
                Ok(Some((v, _))) => Some(v),
                Ok(None) => {
                    done = true;
                    None
                }
                // PEP 479：生成器里冒出来的 StopIteration 不能当成“走完了”
                Err(Stop::Raise(e)) if e.kind == Kind::StopIteration => {
                    return raise(Kind::Runtime, "generator raised StopIteration")
                }
                Err(e) => return Err(e),
            },
            IterState::Map(f, its) => {
                let mut args = Vec::with_capacity(its.len());
                for it in its.iter() {
                    match self.next(it)? {
                        Some(v) => args.push(v),
                        None => {
                            done = true;
                            break;
                        }
                    }
                }
                if done {
                    None
                } else {
                    match self.call(f, args, Vec::new()) {
                        Ok(v) => Some(v),
                        // 函数里冒出来的 StopIteration：取的人看到的就是“这一次没有了”（map 自己并没有走完）
                        Err(Stop::Raise(e)) if e.kind == Kind::StopIteration => None,
                        Err(e) => return Err(e),
                    }
                }
            }
            IterState::Filter(f, it) => loop {
                let Some(v) = self.next(it)? else {
                    done = true;
                    break None;
                };
                let keep = match f {
                    V::None => truthy(&v),
                    _ => match self.call1(f, v.clone()) {
                        Ok(r) => truthy(&r),
                        Err(Stop::Raise(e)) if e.kind == Kind::StopIteration => break None,
                        Err(e) => return Err(e),
                    },
                };
                if keep {
                    break Some(v);
                }
            },
            IterState::Zip(its) => {
                let mut items = Vec::with_capacity(its.len());
                for it in its.iter() {
                    match self.next(it)? {
                        Some(v) => items.push(v),
                        None => {
                            done = true;
                            break;
                        }
                    }
                }
                if its.is_empty() {
                    done = true;
                }
                if done {
                    None
                } else {
                    Some(V::tuple(items))
                }
            }
            IterState::Enumerate(it, n) => match self.next(it)? {
                Some(v) => {
                    let i = *n;
                    *n = match n.checked_add(1) {
                        Some(m) => m,
                        None => return unsupported("enumerate 的序号超过 128 位"),
                    };
                    Some(V::tuple(vec![V::Int(i), v]))
                }
                None => {
                    done = true;
                    None
                }
            },
        };
        if done {
            *st = IterState::Done;
        }
        Ok(item)
    }

    /// list(v)：全部取出来。两个以上元素的集合不给（顺序说不准）
    pub fn collect(&mut self, v: &V) -> R<Vec<V>> {
        match v {
            V::List(l) => Ok(l.borrow().clone()),
            V::Tuple(t) => Ok(t.to_vec()),
            V::Dict(d) => Ok(d.borrow().entries().iter().map(|e| e.k.clone()).collect()),
            V::View(w) => {
                let d = w.d.borrow();
                Ok(d.entries()
                    .iter()
                    .map(|e| match w.kind {
                        ViewKind::Keys => e.k.clone(),
                        ViewKind::Values => e.v.clone(),
                        ViewKind::Items => V::tuple(vec![e.k.clone(), e.v.clone()]),
                    })
                    .collect())
            }
            V::Str(s) => Ok(s.chars().map(|c| V::str(c.encode_utf8(&mut [0; 4]))).collect()),
            V::Set(s) => {
                let d = s.d.borrow();
                if d.len() > 1 {
                    return unsupported(SET_ORDER);
                }
                Ok(d.entries().iter().map(|e| e.k.clone()).collect())
            }
            V::Range(r) => {
                let Some(n) = r.len() else { return unsupported("range 太大（超过 128 位）") };
                self.charge(u64::try_from(n).unwrap_or(u64::MAX))?;
                Ok((0..n).map(|i| V::Int(r.start + i * r.step)).collect())
            }
            _ => {
                let it = self.iter_of(v)?;
                let mut out = Vec::new();
                while let Some(x) = self.next(&it)? {
                    out.push(x);
                }
                Ok(out)
            }
        }
    }

    /// a, b = v：拆成正好 n 个
    pub fn unpack(&mut self, v: &V, n: usize) -> R<Vec<V>> {
        let (items, sized) = match v {
            V::List(l) => (l.borrow().clone(), true),
            V::Tuple(t) => (t.to_vec(), true),
            _ => {
                let it = match self.iter_of(v) {
                    Ok(it) => it,
                    Err(Stop::Raise(e)) if e.kind == Kind::Type => {
                        return type_err(format!("cannot unpack non-iterable {} object", type_name(v)));
                    }
                    Err(e) => return Err(e),
                };
                // 多取一个就知道是不是太多了
                let mut out = Vec::new();
                while out.len() <= n {
                    match self.next(&it)? {
                        Some(x) => out.push(x),
                        None => break,
                    }
                }
                (out, false)
            }
        };
        if items.len() < n {
            return value_err(format!("not enough values to unpack (expected {n}, got {})", items.len()));
        }
        if items.len() > n {
            return value_err(if sized {
                format!("too many values to unpack (expected {n}, got {})", items.len())
            } else {
                format!("too many values to unpack (expected {n})")
            });
        }
        Ok(items)
    }

    // ---- 推导式和生成器 ----

    /// 开始一个推导式：第一个 for 的对象在外面的作用域里求值（这时就报错，不等到取第一个元素）；循环变量先占好位置
    pub fn comp_start(&mut self, def: &Rc<Comp>, env: &Env) -> R<CompState> {
        let first = self.eval(&def.clauses[0].iter, env)?;
        let it = self.iter_of(&first)?;
        let vars = def.names.iter().map(|n| (n.clone(), None)).collect();
        let scope = Rc::new(Scope { vars: RefCell::new(vars), parent: env.clone() });
        Ok(CompState { def: def.clone(), env: Some(scope), iters: vec![it] })
    }

    /// 推导式往前走到下一个元素：（元素，字典推导式的值）；走完了是 None
    pub fn comp_next(&mut self, st: &mut CompState) -> R<Option<(V, Option<V>)>> {
        let def = st.def.clone();
        let env = st.env.clone();
        loop {
            // 最里面那一层 for 正在走的迭代器
            let Some(it) = st.iters.last().cloned() else { return Ok(None) };
            let level = st.iters.len() - 1;
            let Some(item) = self.next(&it)? else {
                st.iters.pop();
                continue;
            };
            let clause = &def.clauses[level];
            self.bind(&clause.target, item, &env)?;
            let mut pass = true;
            for cond in &clause.ifs {
                if !truthy(&self.eval(cond, &env)?) {
                    pass = false;
                    break;
                }
            }
            if !pass {
                continue;
            }
            if let Some(inner) = def.clauses.get(level + 1) {
                let source = self.eval(&inner.iter, &env)?;
                st.iters.push(self.iter_of(&source)?);
                continue;
            }
            let elt = self.eval(&def.elt, &env)?;
            let value = match &def.value {
                Some(v) => Some(self.eval(v, &env)?),
                None => None,
            };
            return Ok(Some((elt, value)));
        }
    }

    /// 给循环变量赋值（变量在推导式自己那一层作用域里，位置早就占好了）
    fn bind(&mut self, t: &Target, v: V, env: &Env) -> R<()> {
        match t {
            Target::Name(n) => {
                if let Some(scope) = env {
                    if let Some(slot) = scope.vars.borrow_mut().iter_mut().find(|(name, _)| **name == **n) {
                        slot.1 = Some(v);
                        return Ok(());
                    }
                }
                unsupported(format!("内部错误：循环变量 {n} 没有占位"))
            }
            Target::Tuple(items) => {
                let parts = self.unpack(&v, items.len())?;
                for (t, p) in items.iter().zip(parts) {
                    self.bind(t, p, env)?;
                }
                Ok(())
            }
            Target::Index(..) => unsupported("循环变量只能是名字或者元组"),
        }
    }
}

/// 集合的元素（不管顺序：只给和顺序无关的用法）
pub fn set_items(s: &SetObj) -> Vec<V> {
    s.d.borrow().entries().iter().map(|e| e.k.clone()).collect()
}

/// reversed(v)
pub fn reversed(v: &V) -> R<V> {
    Ok(match v {
        V::List(l) => {
            let n = l.borrow().len();
            V::iter(Ty::Iterator("list_reverseiterator"), IterState::RevList(l.clone(), n))
        }
        V::Tuple(t) => V::iter(Ty::Reversed, IterState::Rev(t.to_vec())),
        V::Str(s) => V::iter(Ty::Reversed, IterState::Rev(s.chars().map(|c| V::str(c.encode_utf8(&mut [0; 4]))).collect())),
        V::Range(r) => {
            let Some(n) = r.len() else { return unsupported("range 太大（超过 128 位）") };
            // 从最后一项往回走
            let last = if n > 0 { r.at(n - 1).unwrap_or(r.start) } else { r.start };
            let Some(step) = r.step.checked_neg() else { return unsupported("range 太大（超过 128 位）") };
            V::iter(Ty::Iterator("range_iterator"), IterState::Range(last, n, step))
        }
        V::Dict(d) => {
            let keys = d.borrow().entries().iter().map(|e| e.k.clone()).collect();
            V::iter(Ty::Iterator("dict_reversekeyiterator"), IterState::Rev(keys))
        }
        V::View(w) => {
            let d = w.d.borrow();
            let (name, items) = match w.kind {
                ViewKind::Keys => ("dict_reversekeyiterator", d.entries().iter().map(|e| e.k.clone()).collect()),
                ViewKind::Values => ("dict_reversevalueiterator", d.entries().iter().map(|e| e.v.clone()).collect()),
                ViewKind::Items => {
                    ("dict_reverseitemiterator", d.entries().iter().map(|e| V::tuple(vec![e.k.clone(), e.v.clone()])).collect())
                }
            };
            V::iter(Ty::Iterator(name), IterState::Rev(items))
        }
        _ => return type_err(format!("'{}' object is not reversible", type_name(v))),
    })
}
