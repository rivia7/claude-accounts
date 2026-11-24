//! 子命令：参数怎么读、数据怎么绑到名字上、结果怎么变成退出码。
//!
//! PROGRAM 是一行里用分号隔开的几条简单语句，最后是一个表达式（它的值是结果）；edit、run 的只有语句。
//!
//! - `jok PROGRAM HOME`：stdin 的 JSON 是 d，H 是 HOME，tab(名字) 读 HOME/名字，cfg 是 tab("config.json")，另有 acct、uacct
//! - `evq FILE PROGRAM`：FILE 一行一个 JSON；有一条（绑在 e 上）让 PROGRAM 为真就是 0
//! - `check [绑定…] PROGRAM [ARG…]`：PROGRAM 的值为真是 0，为假是 1
//! - `val [绑定…] [--json [--indent N] [--compact] [--unicode] [--sort-keys]] PROGRAM [ARG…]`：打印 PROGRAM 的值
//!   （和 print 一样，带 --json 是 json.dumps）
//! - `edit [绑定…] [--indent N] [--compact] [--unicode] [--sort-keys] [--or-empty] [--atomic] 名字=文件 PROGRAM [ARG…]`：
//!   文件读进 名字，跑语句，照 json.dump 写回去；语句出了错文件不动
//! - `run [绑定…] PROGRAM [ARG…]`：只跑语句
//! - `syntax [--stmts] PROGRAM…`：只看不跑——读得懂、用到的内建函数和方法这里都有才是 0
//!
//! 绑定：`-f 名字=JSON 文件`、`-l 名字=一行一个 JSON 的文件`、`-t 名字=文本文件`、`-s 名字=字符串`；文件写 - 是 stdin。
//! 选项只能在 PROGRAM 前面，“--” 以后的都不当选项；PROGRAM 后面的参数是列表 argv。
//!
//! 退出码：0 / 1 是 Python 那边的结果（真 / 假，或者一个 Python 层面的异常：stderr 上一行 “KeyError: 'x'” 这样的）；
//! 2 是这里不敢担保的（用法不对、语法不对、不认识的名字、没实现的写法），stderr 上以 “cca-test：” 开头，另记进
//! CCA_TEST_TRAP 指的文件（见 write_trap）。程序自己 sys.exit(n) 的照 Python；输出写不出去（读的那一头关了）时和
//! Python 一样是 120（见 interp 的 Sink）。

use std::io::{IsTerminal, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use kit::json::Style;

use super::ast::*;
use super::interp::Interp;
use super::jsonio::{decode, dump_text, io_error, json_lines, parse_json, read_bytes, universal_newlines};
use super::ops::truthy;
use super::parse::{parse, Mode, KEYWORDS, MODULES};
use super::value::*;
use super::{builtins, methods, names, pyfmt};

/// 求值的线程留多大的栈：表达式、数据都是递归着处理的（没用到的部分不占内存）
const STACK: usize = 256 << 20;

/// 输入输出接在哪：真的运行时是进程自己的，测试里换成内存里的
pub struct Io {
    pub stdin: Box<dyn Read + Send>,
    pub out: Box<dyn Write + Send>,
    pub err: Box<dyn Write + Send>,
    /// stdout 接着终端：Python 这时是写一行出一行，不然攒到结束（见 interp 的 Sink）
    pub line_out: bool,
    /// 环境变量 CCA_TEST_TRAP 指的文件：每次以退出码 2 结束，先往里追加一行
    pub trap: Option<PathBuf>,
}

impl Io {
    pub fn real() -> Io {
        let trap = trap_of(std::env::var_os("CCA_TEST_TRAP"));
        let out = std::io::stdout();
        let line_out = out.is_terminal();
        Io { stdin: Box::new(std::io::stdin()), out: Box::new(out), err: Box::new(std::io::stderr()), line_out, trap }
    }
}

/// CCA_TEST_TRAP 的值 → 记到哪个文件；没设、设成空的都是不记
pub fn trap_of(var: Option<std::ffi::OsString>) -> Option<PathBuf> {
    var.filter(|v| !v.is_empty()).map(PathBuf::from)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sub {
    Jok,
    Evq,
    Check,
    Val,
    Edit,
    Run,
    Syntax,
}

impl Sub {
    pub fn name(self) -> &'static str {
        match self {
            Sub::Jok => "jok",
            Sub::Evq => "evq",
            Sub::Check => "check",
            Sub::Val => "val",
            Sub::Edit => "edit",
            Sub::Run => "run",
            Sub::Syntax => "syntax",
        }
    }
}

/// 一个子命令从头到尾：在栈够大的线程里跑；求值器自己出了岔子（panic）也算退出码 2
pub fn run(sub: Sub, args: Vec<String>, io: Io) -> i32 {
    let trap = io.trap.clone();
    let shown = args.join(" ");
    let fail = |msg: String| {
        eprintln!("cca-test：{msg}");
        write_trap(&trap, sub, &msg, &shown);
        2
    };
    match std::thread::Builder::new().stack_size(STACK).spawn(move || session(sub, &args, io)) {
        Ok(worker) => match worker.join() {
            Ok(code) => code,
            Err(_) => fail("内部错误（求值器自己 panic 了）".to_string()),
        },
        Err(e) => fail(format!("起不了线程：{e}")),
    }
}

/// 换行、制表符写成转义的样子，最多留 max 个字符（一条记录占一行、用制表符分栏）
fn one_line(text: &str, max: usize) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    match out.char_indices().nth(max) {
        Some((at, _)) => out[..at].to_string(),
        None => out,
    }
}

/// 往 CCA_TEST_TRAP 追加一行：子命令、原因、程序。回归里有取反的断言（! cfgok '…'），退出码 2 会让它悄悄通过，
/// 所以回归结束时看这个文件：不是空的就算失败。一次 write 写完一整行（几个进程同时写也不会串行）；写不进去不管。
fn write_trap(path: &Option<PathBuf>, sub: Sub, msg: &str, program: &str) {
    let Some(path) = path else { return };
    let line = format!("{}\t{}\t{}\n", sub.name(), one_line(msg, 500), one_line(program, 300));
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).mode(0o644).open(path) {
        let _ = f.write(line.as_bytes());
    }
}

fn usage<T>(msg: impl Into<String>) -> R<T> {
    unsupported(format!("用法不对：{}", msg.into()))
}

fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c == '_' || c.is_alphabetic())
        && chars.all(|c| c == '_' || c.is_alphanumeric())
        && !KEYWORDS.contains(&s)
}

/// 一个绑定：-f 名字=JSON 文件、-l 名字=一行一个 JSON 的文件、-t 名字=文本文件、-s 名字=字符串
struct Bind {
    kind: char,
    name: String,
    value: String,
}

#[derive(Default)]
struct Opts {
    binds: Vec<Bind>,
    json: bool,
    style: Style,
    styled: bool,
    or_empty: bool,
    atomic: bool,
    stmts: bool,
}

/// 读 PROGRAM 前面的选项；返回选项和剩下的参数。选项只能在 PROGRAM 前面，“--” 以后的都不当选项
fn options(sub: Sub, args: &[String]) -> R<(Opts, &[String])> {
    let mut o = Opts::default();
    let mut i = 0;
    let binds = !matches!(sub, Sub::Syntax);
    let style = matches!(sub, Sub::Val | Sub::Edit);
    while let Some(a) = args.get(i) {
        i += 1;
        match a.as_str() {
            "--" => break,
            "-f" | "-l" | "-t" | "-s" if binds => {
                let spec = args.get(i).map(String::as_str).unwrap_or("");
                i += 1;
                match spec.split_once('=') {
                    Some((name, value)) if is_name(name) => {
                        o.binds.push(Bind {
                            kind: a.chars().nth(1).unwrap_or('s'),
                            name: name.to_string(),
                            value: value.to_string(),
                        });
                    }
                    _ => return usage(format!("{a} 后面要 名字=值，却是 {spec:?}")),
                }
            }
            "--json" if sub == Sub::Val => o.json = true,
            "--indent" if style => {
                let Some(n) = args.get(i).and_then(|n| n.parse::<usize>().ok()) else {
                    return usage("--indent 后面要一个数");
                };
                i += 1;
                o.style.indent = Some(n);
                o.styled = true;
            }
            "--compact" if style => (o.style.compact, o.styled) = (true, true),
            "--unicode" if style => (o.style.unicode, o.styled) = (true, true),
            "--sort-keys" if style => (o.style.sort_keys, o.styled) = (true, true),
            "--or-empty" if sub == Sub::Edit => o.or_empty = true,
            "--atomic" if sub == Sub::Edit => o.atomic = true,
            "--stmts" if sub == Sub::Syntax => o.stmts = true,
            other if other.len() > 1 && other.starts_with('-') => {
                return usage(format!("{} 不认识选项 {other}（程序本身以 - 开头的话，前面加 --）", sub.name()));
            }
            _ => {
                i -= 1;
                break;
            }
        }
    }
    if sub == Sub::Val && o.styled && !o.json {
        return usage("--indent、--compact、--unicode、--sort-keys 要和 --json 一起用");
    }
    if o.binds.iter().filter(|b| b.kind != 's' && b.value == "-").count() > 1 {
        return usage("最多只能有一个绑定从 stdin（-）读");
    }
    Ok((o, &args[i..]))
}

struct Session {
    sub: Sub,
    it: Interp,
    stdin: Option<Box<dyn Read + Send>>,
    trap: Option<PathBuf>,
    /// 正在跑的程序的原文：出错时说出来
    program: String,
}

fn session(sub: Sub, args: &[String], io: Io) -> i32 {
    let Io { stdin, out, err, line_out, trap } = io;
    let mut s = Session { sub, it: Interp::new(out, err, line_out), stdin: Some(stdin), trap, program: String::new() };
    let outcome = match sub {
        Sub::Jok => s.jok(args),
        Sub::Evq => s.evq(args),
        Sub::Check => s.check(args),
        Sub::Val => s.val(args),
        Sub::Edit => s.edit(args),
        Sub::Run => s.run(args),
        Sub::Syntax => s.syntax(args),
    };
    // vouched：这个退出码是 Python 也会给的那一个（不是 2 = 这里不敢担保）
    let (code, vouched) = match outcome {
        Ok(code) | Err(Stop::Exit(code)) => (code, true),
        Err(Stop::Raise(e)) => {
            s.it.say(&format!("{}\n", e.line()));
            (1, true)
        }
        Err(Stop::Unsupported(msg)) => {
            s.it.say(&format!("cca-test：{msg}\n"));
            if s.program.is_empty() {
                // 还没读到程序就停下了（用法不对）：记下整条命令的参数
                s.program = args.join(" ");
            } else {
                s.it.say(&format!("  {} 的程序：{}\n", sub.name(), one_line(&s.program, 300)));
            }
            write_trap(&s.trap, sub, &msg, &s.program);
            (2, false)
        }
    };
    // 攒着的输出这时才写出去；写不出去（读的那一头关了）的话退出码照 Python 的来（120）。不敢担保的那个 2 不让它盖掉
    let code = match s.it.finish() {
        Some(broken) if vouched => broken,
        _ => code,
    };
    // 马上就退出了：数据不一层层释放（嵌套很深的数据释放起来又慢又费栈）
    std::mem::forget(s);
    code
}

impl Session {
    fn parse(&mut self, program: &str, mode: Mode) -> R<Program> {
        self.program = program.to_string();
        parse(program, mode).map_err(|bad| Stop::Unsupported(format!("{}（第 {} 个字符）", bad.msg, bad.pos + 1)))
    }

    /// 读一个文件的全部（“-” 是 stdin），按 UTF-8 解
    fn read(&mut self, path: &str) -> R<String> {
        if path != "-" {
            return decode(read_bytes(path)?);
        }
        let mut bytes = Vec::new();
        if let Some(mut stdin) = self.stdin.take() {
            stdin.read_to_end(&mut bytes).map_err(|e| io_error(&e, "<stdin>"))?;
        }
        decode(bytes)
    }

    /// PROGRAM 后面的参数是 argv；绑定照写的先后来（后面的盖住前面的）
    fn bind_all(&mut self, binds: &[Bind], argv: &[String]) -> R<()> {
        self.it.set_global("argv", V::list(argv.iter().map(V::str).collect()));
        for b in binds {
            let v = match b.kind {
                'f' => parse_json(&self.read(&b.value)?)?,
                'l' => json_lines(&universal_newlines(self.read(&b.value)?))?,
                't' => V::str(universal_newlines(self.read(&b.value)?)),
                _ => V::str(&b.value),
            };
            self.it.set_global(&b.name, v);
        }
        Ok(())
    }

    /// 跑一遍，结果是真是假 → 退出码 0 / 1
    fn truth(&mut self, prog: &Program) -> R<i32> {
        let v = self.it.run(prog)?.unwrap_or(V::None);
        Ok(if truthy(&v) { 0 } else { 1 })
    }

    /// 几个名字是用这门语言自己定义的（行为就和原来 Python 里那几行一样）
    fn define(&mut self, source: &str) -> R<()> {
        let prog = parse(source, Mode::Stmts).map_err(|bad| Stop::Unsupported(format!("内部错误：{}", bad.msg)))?;
        self.it.run(&prog)?;
        Ok(())
    }

    fn jok(&mut self, args: &[String]) -> R<i32> {
        let [program, home] = args else { return usage("cca-test jok PROGRAM HOME（文档从 stdin 读）") };
        let prog = self.parse(program, Mode::Expr)?;
        self.it.set_global("H", V::str(home));
        self.define(r#"tab = lambda n: json.load(open(H + "/" + n)); cfg = tab("config.json")"#)?;
        let d = parse_json(&self.read("-")?)?;
        self.it.set_global("d", d);
        self.define(r#"acct = lambda t, k: next(a for a in d["tools"][t]["accounts"] if a["key"] == k)"#)?;
        self.define(r#"uacct = lambda k: next(a for a in d["accounts"] if a["key"] == k)"#)?;
        self.truth(&prog)
    }

    fn evq(&mut self, args: &[String]) -> R<i32> {
        let [file, program] = args else { return usage("cca-test evq FILE PROGRAM") };
        let prog = self.parse(program, Mode::Expr)?;
        let ev = json_lines(&universal_newlines(self.read(file)?))?;
        self.it.set_global("ev", ev.clone());
        let events = self.it.collect(&ev)?;
        // 有一条让 PROGRAM 为真就行；前面的某一条出了异常就是异常（和 any(… for e in ev) 一样）
        for e in events {
            self.it.set_global("e", e);
            if self.truth(&prog)? == 0 {
                return Ok(0);
            }
        }
        Ok(1)
    }

    fn check(&mut self, args: &[String]) -> R<i32> {
        let (o, rest) = options(Sub::Check, args)?;
        let Some((program, argv)) = rest.split_first() else { return usage("cca-test check [绑定…] PROGRAM [ARG…]") };
        let prog = self.parse(program, Mode::Expr)?;
        self.bind_all(&o.binds, argv)?;
        self.truth(&prog)
    }

    fn val(&mut self, args: &[String]) -> R<i32> {
        let (o, rest) = options(Sub::Val, args)?;
        let Some((program, argv)) = rest.split_first() else {
            return usage("cca-test val [绑定…] [--json …] PROGRAM [ARG…]");
        };
        let prog = self.parse(program, Mode::Expr)?;
        self.bind_all(&o.binds, argv)?;
        let v = self.it.run(&prog)?.unwrap_or(V::None);
        let text = if o.json { dump_text(&v, &o.style, false)? } else { pyfmt::str_of(&v)? };
        // print(结果)：字和换行各写一次
        self.it.write(Stream::Stdout, &text)?;
        self.it.write(Stream::Stdout, "\n")?;
        Ok(0)
    }

    fn run(&mut self, args: &[String]) -> R<i32> {
        let (o, rest) = options(Sub::Run, args)?;
        let Some((program, argv)) = rest.split_first() else { return usage("cca-test run [绑定…] PROGRAM [ARG…]") };
        let prog = self.parse(program, Mode::Stmts)?;
        self.bind_all(&o.binds, argv)?;
        self.it.run(&prog)?;
        Ok(0)
    }

    fn edit(&mut self, args: &[String]) -> R<i32> {
        let (o, rest) = options(Sub::Edit, args)?;
        let [target, program, argv @ ..] = rest else {
            return usage("cca-test edit [绑定…] [选项…] 名字=文件 PROGRAM [ARG…]");
        };
        let Some((name, path)) = target.split_once('=').filter(|(n, _)| is_name(n)) else {
            return usage(format!("edit 要改的那一项写成 名字=文件，却是 {target:?}"));
        };
        let prog = self.parse(program, Mode::Stmts)?;
        self.bind_all(&o.binds, argv)?;
        let doc = match read_bytes(path) {
            Ok(bytes) => parse_json(&decode(bytes)?)?,
            Err(Stop::Raise(e)) if o.or_empty && e.kind == Kind::FileNotFound => V::dict(Dict::new()),
            Err(e) => return Err(e),
        };
        self.it.set_global(name, doc);
        self.it.run(&prog)?;
        // 到这里语句都跑完了；写不成 JSON 的（里面放了集合……）也在动文件之前就停下
        let Some(v) = self.it.global(name) else { return raise(Kind::Name, format!("name '{name}' is not defined")) };
        let text = dump_text(&v, &o.style, false)?;
        write_file(path, &text, o.atomic)?;
        Ok(0)
    }

    fn syntax(&mut self, args: &[String]) -> R<i32> {
        let (o, programs) = options(Sub::Syntax, args)?;
        if programs.is_empty() {
            return usage("cca-test syntax [--stmts] PROGRAM…");
        }
        let mode = if o.stmts { Mode::Stmts } else { Mode::Expr };
        let mut first: Option<(String, String)> = None;
        for p in programs {
            let problems = match parse(p, mode) {
                Ok(prog) => problems(&prog),
                Err(bad) => vec![format!("{}（第 {} 个字符）", bad.msg, bad.pos + 1)],
            };
            for msg in problems {
                self.it.say(&format!("cca-test：{msg}：{}\n", one_line(p, 300)));
                first.get_or_insert((msg, p.clone()));
            }
        }
        match first {
            None => Ok(0),
            Some((msg, program)) => {
                write_trap(&self.trap, self.sub, &msg, &program);
                Ok(2)
            }
        }
    }
}

/// edit 的结果写回去：不带结尾的换行（和 json.dump 一样）。不带 atomic：就地覆盖（FILE 是符号链接的话，改的是它指着的
/// 那个文件，权限不变）。atomic：先写到旁边的临时文件再改名，和回归里原来那几行 Python 一样（open(临时文件, "w") 再
/// os.replace）——FILE 是符号链接的话，链接换成普通文件；新文件的权限是新建文件默认的（照 umask），不是原来那个文件的
fn write_file(path: &str, text: &str, atomic: bool) -> R<()> {
    if !atomic {
        return std::fs::write(path, text).map_err(|e| io_error(&e, path));
    }
    let target = Path::new(path);
    let dir = target.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let base = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{base}.{}.tmp", std::process::id()));
    let write = || -> std::io::Result<()> {
        // 上一次没删掉的临时文件（权限可能不一样）先拿掉
        let _ = std::fs::remove_file(&tmp);
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, target)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io_error(&e, path)
    })
}

// ---- syntax：不跑，只看认不认识 ----

/// 一段程序里这里不认识的东西：没实现的内建函数、方法名、关键字参数。
/// 普通的名字查不了（要跑起来才知道绑没绑上）。
fn problems(prog: &Program) -> Vec<String> {
    let mut c = Checker { bound: Vec::new(), problems: Vec::new() };
    for s in &prog.stmts {
        c.bound_in_stmt(s);
    }
    if let Some(e) = &prog.last {
        c.bound_in_expr(e);
    }
    for s in &prog.stmts {
        c.stmt(s);
    }
    if let Some(e) = &prog.last {
        c.expr(e);
    }
    c.problems
}

struct Checker {
    /// 程序自己绑过的名字（赋值、lambda 的参数、循环变量）：这些名字不再当内建的看
    bound: Vec<Rc<str>>,
    problems: Vec<String>,
}

impl Checker {
    fn bind_target(&mut self, t: &Target) {
        match t {
            Target::Name(n) => self.bound.push(n.clone()),
            Target::Tuple(items) => items.iter().for_each(|x| self.bind_target(x)),
            Target::Index(obj, idx) => {
                self.bound_in_expr(obj);
                self.bound_in_expr(idx);
            }
        }
    }

    fn bound_in_stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Expr(e) => self.bound_in_expr(e),
            Stmt::Assign(targets, value) => {
                targets.iter().for_each(|t| self.bind_target(t));
                self.bound_in_expr(value);
            }
            Stmt::Aug(target, _, value) => {
                self.bind_target(target);
                self.bound_in_expr(value);
            }
            Stmt::Del(_) | Stmt::Pass | Stmt::Import(_) => {}
            Stmt::Assert(cond, msg) => {
                self.bound_in_expr(cond);
                if let Some(m) = msg {
                    self.bound_in_expr(m);
                }
            }
        }
    }

    fn bound_in_expr(&mut self, e: &Expr) {
        match e {
            Expr::Lambda(def) => self.bound.extend(def.params.iter().cloned()),
            Expr::Comp(def) => self.bound.extend(def.names.iter().cloned()),
            _ => {}
        }
        each_child(e, &mut |child| self.bound_in_expr(child));
    }

    fn is_builtin(&self, name: &str) -> bool {
        !self.bound.iter().any(|b| &**b == name) && builtins::builtin(name).is_some()
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Expr(e) => self.expr(e),
            Stmt::Assign(targets, value) => {
                targets.iter().for_each(|t| self.target(t));
                self.expr(value);
            }
            Stmt::Aug(target, _, value) => {
                self.target(target);
                self.expr(value);
            }
            Stmt::Del(targets) => targets.iter().for_each(|t| self.target(t)),
            Stmt::Assert(cond, msg) => {
                self.expr(cond);
                if let Some(m) = msg {
                    self.expr(m);
                }
            }
            Stmt::Import(_) | Stmt::Pass => {}
        }
    }

    fn target(&mut self, t: &Target) {
        match t {
            Target::Name(_) => {}
            Target::Tuple(items) => items.iter().for_each(|x| self.target(x)),
            Target::Index(obj, idx) => {
                self.expr(obj);
                self.expr(idx);
            }
        }
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Name(n) => {
                let known = self.bound.iter().any(|b| **b == **n) || builtins::builtin(n).is_some();
                if !known && names::BUILTINS.contains(&&**n) {
                    self.problems.push(format!("Python 内建的 {n} 这里没实现"));
                }
            }
            Expr::Attr(obj, name) => match &**obj {
                Expr::Name(m) if MODULES.contains(&&**m) && self.is_builtin(m) => {
                    if !methods::module_has(m, name) {
                        self.problems.push(format!("{m}.{name} 这里没有"));
                    }
                }
                _ => {
                    if !methods::known_attr(name) {
                        self.problems.push(format!("方法或者属性 .{name} 这里没实现"));
                    }
                }
            },
            Expr::Call(f, _, kwargs) if !kwargs.is_empty() => {
                // 认哪些关键字参数；None 是查不了（lambda、别的值）或者什么都认（dict()、update()）
                let (what, allowed): (String, Option<&[&str]>) = match &**f {
                    Expr::Name(n) if self.is_builtin(n) => (format!("{n}()"), builtins::keywords(n)),
                    Expr::Attr(_, name) => (format!(".{name}()"), methods::method_keywords(name)),
                    _ => (String::new(), None),
                };
                if let Some(allowed) = allowed {
                    for (k, _) in kwargs {
                        if !allowed.contains(&&**k) {
                            self.problems.push(format!("{what} 的关键字参数 {k} 这里没实现"));
                        }
                    }
                }
            }
            _ => {}
        }
        each_child(e, &mut |child| self.expr(child));
    }
}

/// 一个表达式直接套着的那些子表达式
fn each_child(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    match e {
        Expr::None | Expr::True | Expr::False | Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Name(_) => {}
        Expr::Tuple(items) | Expr::List(items) | Expr::Set(items) | Expr::And(items) | Expr::Or(items) => items.iter().for_each(f),
        Expr::Dict(pairs) => {
            for (k, v) in pairs {
                f(k);
                f(v);
            }
        }
        Expr::Attr(obj, _) => f(obj),
        Expr::Index(obj, idx) => {
            f(obj);
            f(idx);
        }
        Expr::Slice(a, b, c) => [a, b, c].into_iter().flatten().for_each(|x| f(x)),
        Expr::Call(func, args, kwargs) => {
            f(func);
            args.iter().for_each(&mut *f);
            kwargs.iter().for_each(|(_, v)| f(v));
        }
        Expr::Not(x) | Expr::Neg(x) | Expr::Pos(x) => f(x),
        Expr::Bin(_, a, b) => {
            f(a);
            f(b);
        }
        Expr::Compare(first, rest) => {
            f(first);
            rest.iter().for_each(|(_, x)| f(x));
        }
        Expr::IfElse(a, b, c) => {
            f(a);
            f(b);
            f(c);
        }
        Expr::Lambda(def) => {
            def.defaults.iter().for_each(&mut *f);
            f(&def.body);
        }
        Expr::Comp(def) => {
            f(&def.elt);
            if let Some(v) = &def.value {
                f(v);
            }
            for clause in &def.clauses {
                f(&clause.iter);
                clause.ifs.iter().for_each(&mut *f);
            }
        }
    }
}
