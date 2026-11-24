//! 求值器的测试，三部分：
//! ① 对照数据 tests/vectors/ev_py.json：几千段小程序（带着绑定、stdin、文件）和真的 Python 3.14 算出来的退出码、stdout、stderr，
//!    在这里一条条重放。生成它的脚本不在仓库里（仓库里不放 Python）：改了求值器，这份数据照样得过；
//! ② 词法、语法、容易错的语义：期望的值也都是真的 Python 算的；
//! ③ 对照数据表达不了的：CCA_TEST_TRAP、edit 怎么写文件（符号链接、--atomic）、不是 UTF-8 的文件。
//!
//! 这个包的 [[bin]] 写着 test = false，要点名才跑：cargo test --offline --locked --profile kit -p cca-tests --bin cca-test

use std::io::{Cursor, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use kit::json::Value;

use super::cli::{self, Io, Sub};
use super::lex::{lex, Tok};
use super::parse::{parse, Mode};

// ---- 底座：在内存里跑一条命令 ----

/// 内存里的输出（求值在另一个线程里，所以要能跨线程）
#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Buf {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

struct Got {
    code: i32,
    out: String,
    err: String,
}

fn sub_named(name: &str) -> Sub {
    match name {
        "jok" => Sub::Jok,
        "evq" => Sub::Evq,
        "check" => Sub::Check,
        "val" => Sub::Val,
        "edit" => Sub::Edit,
        "run" => Sub::Run,
        "syntax" => Sub::Syntax,
        other => panic!("没有 {other} 这个子命令"),
    }
}

/// 跑一条命令（第一项是子命令）：stdin 的内容、CCA_TEST_TRAP 指的文件由调用的给
fn run_full<S: AsRef<str>>(cmd: &[S], stdin: &str, trap: Option<&Path>) -> Got {
    let (out, err) = (Buf::default(), Buf::default());
    let io = Io {
        stdin: Box::new(Cursor::new(stdin.as_bytes().to_vec())),
        out: Box::new(out.clone()),
        err: Box::new(err.clone()),
        line_out: false,
        trap: trap.map(Path::to_path_buf),
    };
    let args = cmd[1..].iter().map(|a| a.as_ref().to_string()).collect();
    let code = cli::run(sub_named(cmd[0].as_ref()), args, io);
    Got { code, out: out.text(), err: err.text() }
}

fn run<S: AsRef<str>>(cmd: &[S]) -> Got {
    run_full(cmd, "", None)
}

/// 用完就删的临时目录
struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Tmp {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let name = format!("cca-test-ev-{}-{}-{tag}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed));
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Tmp(dir)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_string()
    }
    fn write(&self, name: &str, content: impl AsRef<[u8]>) -> String {
        let path = self.path(name);
        std::fs::write(&path, content).unwrap();
        path
    }
    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.path(name)).unwrap()
    }
    /// 目录里现在有哪些名字（排好序）
    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> =
            std::fs::read_dir(&self.0).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn mode_of(path: &str) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

// ---- ① 对照数据 ----

/// 文件的样子：{"docs": {文件名: 内容}, "cases": [一条, …]}。docs 是所有用例共用的文档（放在 {DOCS} 目录）。一条用例：
/// "cmd"：命令行（第一项是子命令）；最常见的两种写短了——{"val": 程序} 是 val -f c={DOCS}/c.json -- 程序，"run" 同理；
/// "files"：这一条自己的文件（放在 {DIR} 目录）；"stdin"；"exit"、"out"、"err"：Python 那边的退出码、stdout、stderr
/// （没写的就是空的）；"loose"：stderr 只比异常是哪一类（冒号前面）；"unsupported"：这里不支持，要的是退出码 2、
/// stderr 以 “cca-test：” 开头；"after"：跑完以后 {DIR} 里各个文件的内容（edit 改了什么、出错时是不是原样）。
#[test]
fn vectors_from_python() {
    let text = include_str!("../../vectors/ev_py.json");
    // 仓库是公开的：对照数据里只能有造出来的内容，不能带着生成它的那台机器上的路径
    for private in ["/Users/", "/home/", "/tmp/", "/var/folders/"] {
        assert!(!text.contains(private), "对照数据里有 {private}");
    }
    let doc = kit::json::parse(text).unwrap();
    let tmp = Tmp::new("vectors");
    for (name, content) in doc["docs"].obj().iter() {
        tmp.write(name, content.str());
    }
    let base = tmp.0.to_str().unwrap();
    let cases = doc["cases"].list();
    assert!(cases.len() >= 400, "对照数据只有 {} 条", cases.len());
    let mut wrong = Vec::new();
    for (idx, case) in cases.iter().enumerate() {
        if let Err(why) = replay(base, idx, case) {
            wrong.push(format!("#{idx} {}\n    {why}", case.compact()));
        }
    }
    let shown: Vec<&str> = wrong.iter().take(25).map(String::as_str).collect();
    assert!(wrong.is_empty(), "{} 条里有 {} 条对不上，前几条：\n{}", cases.len(), wrong.len(), shown.join("\n"));
}

/// 重放一条；对不上时说哪里对不上
fn replay(base: &str, idx: usize, case: &Value) -> Result<(), String> {
    let short = |sub: &str, program: &Value| {
        vec![sub.to_string(), "-f".into(), "c={DOCS}/c.json".into(), "--".into(), program.str().into()]
    };
    let cmd: Vec<String> = match (case.get("val"), case.get("run")) {
        (Some(program), _) => short("val", program),
        (_, Some(program)) => short("run", program),
        _ => case["cmd"].list().iter().map(|a| a.str().to_string()).collect(),
    };
    let files: Vec<(&str, &Value)> = case.get("files").map(|f| f.obj().iter().collect()).unwrap_or_default();
    let dir = (case.get("files").is_some() || cmd.iter().any(|a| a.contains("{DIR}"))).then(|| format!("{base}/{idx}"));
    if let Some(dir) = &dir {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        for (name, content) in &files {
            std::fs::write(format!("{dir}/{name}"), content.str()).map_err(|e| e.to_string())?;
        }
    }
    // 命令行里的 {DOCS}、{DIR} 换成真的目录；输出里的目录换回去再比
    let place = |s: &str| {
        let s = s.replace("{DOCS}", base);
        match &dir {
            Some(dir) => s.replace("{DIR}", dir),
            None => s,
        }
    };
    let unplace = |s: String| {
        let s = match &dir {
            Some(dir) => s.replace(dir.as_str(), "{DIR}"),
            None => s,
        };
        s.replace(base, "{DOCS}")
    };
    let args: Vec<String> = cmd.iter().map(|a| place(a)).collect();
    let got = run_full(&args, case.get("stdin").map_or("", |s| s.str()), None);
    let (out, err) = (unplace(got.out), unplace(got.err));
    let mut wrong = Vec::new();
    if case.get("unsupported").is_some() {
        if got.code != 2 || !err.starts_with("cca-test：") {
            wrong.push(format!(
                "要的是退出码 2、stderr 以 “cca-test：” 开头，却是退出码 {}、stdout {out:?}、stderr {err:?}",
                got.code
            ));
        }
    } else {
        let (want_code, want_out) = (case["exit"].i64(), case.get("out").map_or("", |s| s.str()));
        let want_err = case.get("err").map_or("", |s| s.str());
        // 进程的退出码只有低 8 位（sys.exit(-1) 是 255）
        if i64::from(got.code & 0xff) != want_code {
            wrong.push(format!("退出码 {}，要的是 {want_code}（stderr {err:?}）", got.code));
        }
        if out != want_out {
            wrong.push(format!("stdout {out:?}，要的是 {want_out:?}"));
        }
        let class = |s: &str| s.split(':').next().unwrap_or("").trim().to_string();
        let same = if case.get("loose").is_some() { class(&err) == class(want_err) } else { err == want_err };
        if !same {
            wrong.push(format!("stderr {err:?}，要的是 {want_err:?}"));
        }
    }
    if let Some(after) = case.get("after") {
        let mut want: Vec<(String, String)> =
            after.obj().iter().map(|(name, content)| (name.to_string(), content.str().to_string())).collect();
        want.sort();
        let have = dir.as_deref().map(snapshot).unwrap_or_default();
        if have != want {
            wrong.push(format!("跑完以后的文件 {have:?}，要的是 {want:?}"));
        }
    }
    if wrong.is_empty() {
        Ok(())
    } else {
        Err(wrong.join("\n    "))
    }
}

/// 目录里各个普通文件的内容，按名字排好
fn snapshot(dir: &str) -> Vec<(String, String)> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if entry.path().is_file() {
            let content = String::from_utf8_lossy(&std::fs::read(entry.path()).unwrap_or_default()).into_owned();
            files.push((entry.file_name().to_string_lossy().into_owned(), content));
        }
    }
    files.sort();
    files
}

// ---- ② 词法 ----

fn toks(src: &str) -> Vec<Tok> {
    lex(src, true).unwrap_or_else(|bad| panic!("{src:?}: {bad:?}")).into_iter().map(|t| t.tok).collect()
}

fn name(s: &str) -> Tok {
    Tok::Name(s.to_string())
}

fn text(s: &str) -> Tok {
    Tok::Str(s.to_string())
}

#[test]
fn lexer_cuts_tokens() {
    use Tok::{End, Float, Int, Newline, Op};
    assert_eq!(
        toks(r#"x1 = d["k"] + 0x1f // 2  # 注释"#),
        vec![name("x1"), Op("="), name("d"), Op("["), text("k"), Op("]"), Op("+"), Int(31), Op("//"), Int(2), End]
    );
    // 运算符先试长的
    assert_eq!(
        toks("a**=b//c<=d!=e"),
        vec![name("a"), Op("**="), name("b"), Op("//"), name("c"), Op("<="), name("d"), Op("!="), name("e"), End]
    );
    // 关键字、True / None 在这一层都只是名字；名字可以是中文
    assert_eq!(toks("not None 账号_1"), vec![name("not"), name("None"), name("账号_1"), End]);
    // 整数的各种写法
    assert_eq!(
        toks("0 00 0_0 7 1_000 0x1F 0XfF 0o17 0b101"),
        vec![Int(0), Int(0), Int(0), Int(7), Int(1000), Int(31), Int(255), Int(15), Int(5), End]
    );
    assert_eq!(
        toks("1.5 .5 5. 1e3 1E-2 1_0.2_5 1e1_0"),
        vec![Float(1.5), Float(0.5), Float(5.0), Float(1000.0), Float(0.01), Float(10.25), Float(1e10), End]
    );
    // 整数用 i128：装得下的最大的数认得，再大就不认（不悄悄算错）
    assert_eq!(toks("170141183460469231731687303715884105727"), vec![Int(i128::MAX), End]);
    assert!(lex("170141183460469231731687303715884105728", true).is_err());
    // 括号外面的换行把语句分开（连着几个空行只算一个）；括号里面的、反斜杠续行的不算
    assert_eq!(toks("x\n\n\ny"), vec![name("x"), Newline, name("y"), End]);
    assert_eq!(toks("(1,\n 2)"), vec![Op("("), Int(1), Op(","), Int(2), Op(")"), End]);
    assert_eq!(toks("1 + \\\n  2"), vec![Int(1), Op("+"), Int(2), End]);
    // Python 不认的数字写法
    for bad in ["012", "1__0", "1_", "0x", "0xg", "1e", "1.e", "0b12", "0o8", "1abc", "1.real", "1if 1 else 2", "0_", "1j", "1.5J"]
    {
        assert!(lex(bad, true).is_err(), "{bad}");
    }
    // 位置按字符数，不是字节数
    assert_eq!(lex("\"中文\" $", true).unwrap_err().pos, 5);
}

#[test]
fn lexer_reads_strings() {
    use Tok::End;
    assert_eq!(toks(r#"'a' "b" r"\d" R'\n' u"x""#), vec![text("a"), text("b"), text("\\d"), text("\\n"), text("x"), End]);
    assert_eq!(toks(r#""it's" 'say "hi"' '\'' "\"""#), vec![text("it's"), text("say \"hi\""), text("'"), text("\""), End]);
    // 转义：\n \t \\ \' \" \xhh、八进制、\uhhhh、\Uhhhhhhhh
    assert_eq!(toks(r#""\n\t\\\'\"\x41\101\0\u00e9\U0001F600""#), vec![text("\n\t\\'\"AA\0é😀"), End]);
    // 不认识的转义 Python 原样留着（只给一个警告）
    assert_eq!(toks(r#""\d\ \%""#), vec![text("\\d\\ \\%"), End]);
    // 行尾的反斜杠：字符串接到下一行
    assert_eq!(toks("'a\\\nb'"), vec![text("ab"), End]);
    // 这里不支持的、Python 也不认的
    for bad in [
        r#"b"x""#,
        r#"f"x""#,
        r#"rb"x""#,
        r#"'''x'''"#,
        r#""""x""""#,
        r#""abc"#,
        "'a\nb'",
        r#""\N{BULLET}""#,
        r#""\x4""#,
        r#""\u12""#,
        r#""\ud800""#,
        r#""\U00110000""#,
    ] {
        assert!(lex(bad, true).is_err(), "{bad}");
    }
}

#[test]
fn lexer_refuses_indented_lines() {
    // 第一行前面的空格、制表符：eval() 会先去掉（表达式那一种不算缩进），exec() 不会
    assert!(lex("  1", true).is_ok());
    assert!(lex("  1", false).is_err());
    assert!(lex("\t1", false).is_err());
    // 后面的行哪一种都不能缩进（Python 报 unexpected indent）
    assert!(lex("1\n 2", true).is_err());
    assert!(lex("x = 1\n\ty = 2", false).is_err());
    // 空行、只有注释的行、括号里面的行、反斜杠续的行不算
    assert!(lex("x = 1\n\n   \n  # 注释\ny = 2\n   ", false).is_ok());
    assert!(lex("x = [1,\n     2]\ny = (\n  3\n)", false).is_ok());
    assert!(lex("x = 1 + \\\n    2", false).is_ok());
}

// ---- ② 语法 ----

/// 语法树写成字符串：两种写法得到同一棵树，就是优先级、结合方向一样
fn tree(src: &str) -> String {
    format!("{:?}", parse(src, Mode::Expr).unwrap_or_else(|bad| panic!("{src:?}: {bad:?}")))
}

#[test]
fn parser_precedence() {
    // 左边不带括号，右边是照 Python 的优先级和结合方向加上括号的
    for (plain, explicit) in [
        ("a + b * c", "a + (b * c)"),
        ("a - b - c", "(a - b) - c"),
        ("a / b // c % d", "((a / b) // c) % d"),
        ("-a ** b", "-(a ** b)"),
        ("a ** -b", "a ** (-b)"),
        ("a ** b ** c", "a ** (b ** c)"),
        ("- - a", "-(-a)"),
        ("a * -b", "a * (-b)"),
        ("-a.b", "-(a.b)"),
        ("a.b(c)[d].e", "(((a.b)(c))[d]).e"),
        ("a | b ^ c & d", "a | (b ^ (c & d))"),
        ("a + b | c", "(a + b) | c"),
        ("a | b == c", "(a | b) == c"),
        ("a < b + c", "a < (b + c)"),
        ("not a == b", "not (a == b)"),
        ("not a in b", "not (a in b)"),
        ("x in a or y not in b", "(x in a) or (y not in b)"),
        ("a or b and c", "a or (b and c)"),
        ("a and not b or c", "(a and (not b)) or c"),
        ("a if b else c if d else e", "a if b else (c if d else e)"),
        ("a or b if c else d", "(a or b) if c else d"),
        ("lambda: a if b else c", "lambda: (a if b else c)"),
        ("lambda x: x, y", "(lambda x: x), y"),
        ("a, b if c else d", "a, (b if c else d)"),
        ("1 + 2,", "(1 + 2,)"),
        ("(a)", "a"),
        ("[x for x in a if b or c]", "[x for x in a if (b or c)]"),
        ("{k: v for k, v in a}", "{k: v for (k, v) in a}"),
    ] {
        assert_eq!(tree(plain), tree(explicit), "{plain}");
    }
    // 连着比较是一个节点（中间那个只算一次），不是两次比较套起来
    assert_ne!(tree("a < b < c"), tree("(a < b) < c"));
    assert_ne!(tree("a < b < c"), tree("a < (b < c)"));
    // 带逗号才是元组
    assert_ne!(tree("(a,)"), tree("a"));
    assert_ne!(tree("a, b"), tree("[a, b]"));
}

#[test]
fn parser_modes() {
    // 表达式那一种（jok、evq、check、val）：前面是语句，最后一个表达式是结果
    let program = parse("x = 1; y = x + 1; x + y", Mode::Expr).unwrap();
    assert_eq!(program.stmts.len(), 2);
    assert!(program.last.is_some());
    // 语句那一种（edit、run）：最后的表达式也只是一条语句；可以是空的
    let program = parse("x = 1; x + 1", Mode::Stmts).unwrap();
    assert_eq!(program.stmts.len(), 2);
    assert!(program.last.is_none());
    assert!(parse("", Mode::Stmts).unwrap().stmts.is_empty());
    assert!(parse("# 只有注释\n\n", Mode::Stmts).unwrap().stmts.is_empty());
    for bad in ["", "   ", "# 只有注释", "x = 1", "x = 1;", "pass", "1; pass", "assert 1"] {
        assert!(parse(bad, Mode::Expr).is_err(), "{bad:?}");
    }
    // 分号两边都要有语句，只有行尾能多一个
    for good in ["1;", "1; 2;", "x = 1; x;\n", "x = 1\ny = 2;\nx + y"] {
        assert!(parse(good, Mode::Expr).is_ok(), "{good:?}");
    }
    for bad in [";", "; 1", "1;; 2", "1; ; 2", "1\n; 2"] {
        assert!(parse(bad, Mode::Expr).is_err(), "{bad:?}");
        assert!(parse(bad, Mode::Stmts).is_err(), "{bad:?}");
    }
    // 赋值的左边：名字、下标、元组（可以套着）；别的不行
    for good in [
        "a = b = 1",
        "a, (b, c) = x",
        "[a, b] = x",
        "d['k'] = d[0][1] = 1",
        "a, b = b, a",
        "x += 1",
        "d['k'] -= 1",
        "del d['k'], x",
    ] {
        assert!(parse(good, Mode::Stmts).is_ok(), "{good:?}");
    }
    for bad in [
        "1 = 2",
        "f() = 1",
        "a + b = 1",
        "True = 1",
        "None = 1",
        "a, b += 1",
        "x = ",
        "= 1",
        "a.b = 1",
        "x: int = 1",
        "del f()",
        "a, *b = x",
    ] {
        assert!(parse(bad, Mode::Stmts).is_err(), "{bad:?}");
    }
}

#[test]
fn parser_says_where_and_what() {
    // 写错了的：指着第几个字符（从 0 数）
    for (src, pos) in [
        ("1 +", 3),
        ("(1", 2),
        ("1 2", 2),
        ("$", 0),
        ("1 ? 2", 2),
        ("\"中文\" $", 5),
        ("f(a=1, 2)", 7),
        ("x = 1", 5),
        ("a if b", 6),
    ] {
        let bad = parse(src, Mode::Expr).unwrap_err();
        assert_eq!(bad.pos, pos, "{src}: {}", bad.msg);
    }
    // Python 有、这里不支持的：说出是哪一种写法
    for (src, what) in [
        ("f\"x\"", "f-string"),
        ("b'x'", "字节串"),
        ("'''x'''", "三引号"),
        ("1j", "复数"),
        ("[*a]", "展开"),
        ("f(**a)", "展开"),
        ("(x := 1)", ":="),
        ("x: int = 1", "类型标注"),
        ("a.b = 1", "属性赋值"),
        ("1 << 2", "<<"),
        ("~1", "~"),
        ("1 @ 2", "@"),
        ("...", "Ellipsis"),
        ("x[1:2, 3]", "多维切片"),
        ("lambda *a: a", "lambda"),
        ("import os", "import"),
        ("import json as j", "import"),
        ("from json import loads", "from"),
        ("def f(): pass", "def"),
        ("for x in y: pass", "for"),
        ("if x: pass", "if"),
        ("while x: pass", "while"),
        ("raise ValueError", "raise"),
        ("await x", "await"),
        ("x = yield", "yield"),
        ("l[0:1] = []", "切片"),
        ("\"\\N{BULLET}\"", "\\N"),
    ] {
        let bad = parse(src, Mode::Stmts).unwrap_err();
        assert!(bad.msg.contains(what), "{src}: {}", bad.msg);
    }
    // import 只认这三个模块（本来就在），别的写法都不认
    assert!(parse("import json, sys, time", Mode::Stmts).is_ok());
}

// ---- ② 容易错的语义 ----

/// 程序（val）和 Python 3.14 打印出来的那一行
const SEMANTICS: &[(&str, &str)] = &[
    (r"7 // -2, -7 // 2, 7 % -2, -7 % 2, divmod(-7, 2)", r"(-4, -4, -1, 1, (-4, 1))"),
    (
        r"7.5 // 2, -7.5 // 2, 7.5 % -2, 2 ** -1, (-2) ** 3, 2 ** 0.5, -2 ** 2, 2 ** 3 ** 2",
        r"(3.0, -4.0, -0.5, 0.5, -8, 1.4142135623730951, -4, 512)",
    ),
    (
        r"round(0.5), round(1.5), round(2.5), round(-0.5), round(2.675, 2), round(1234, -2), round(1.25, 1), round(7)",
        r"(0, 2, 2, 0, 2.67, 1200, 1.2, 7)",
    ),
    (
        r#"1 / 3, 1e16, 1e-5, 123456789012345678.0, 0.1 + 0.2, -0.0, 1e300 * 1e10, float("nan"), float("-inf")"#,
        r"(0.3333333333333333, 1e+16, 1e-05, 1.2345678901234568e+17, 0.30000000000000004, -0.0, inf, nan, -inf)",
    ),
    (
        r#"int("  -12  "), int("1f", 16), int("z", 36), int(3.99), int(-3.99), float("1e3"), int(True), int("1_000")"#,
        r"(-12, 31, 35, 3, -3, 1000.0, 1, 1000)",
    ),
    (r#"True + True, True == 1, 1 == 1.0, {1: "a", 1.0: "b", True: "c"}"#, r"(2, True, True, {1: 'c'})"),
    (
        r"10 ** 20, 2 ** 100, -(2 ** 63), 0xFF ^ 0x0F, 5 & -2, 7 | 8",
        r"(100000000000000000000, 1267650600228229401496703205376, -9223372036854775808, 240, 4, 15)",
    ),
    (r"1 < 2 < 3, 1 < 3 > 2, 1 == 1 != 2, 3 > 2 > 2, 1 < 2 == 2.0", r"(True, True, True, False, True)"),
    (
        r"abs(-3), abs(-2.5), min(3, 1, 2), max([1, 5, 2], key=lambda v: -v), sum([1, 2.5]), sum([[1], [2]], [])",
        r"(3, 2.5, 1, 1, 3.5, [1, 2])",
    ),
    (
        r"3 * 0.1, 0.1 * 3 == 0.3, 1e22, 1e21 + 1, 5e-324, 2.0 ** 1023 * 2 > 1e308, 9007199254740993 == 9007199254740992.0",
        r"(0.30000000000000004, False, 1e+22, 1e+21, 5e-324, True, False)",
    ),
    (r"1 if 0 else 2 if 0 else 3, not 1 == 2, 1 or 2 and 3, 1 | 2 ^ 3 & 4, - - 1, +-+1, 1 - - 1", r"(3, True, 1, 3, 1, -1, 2)"),
    (
        r#""abc"[::-1], "abcdef"[1:-1:2], "abc"[5:], "abc"[-10:2], "héllo"[1], len("héllo中"), "abc"[-1]"#,
        r"('cba', 'bd', '', 'ab', 'é', 6, 'c')",
    ),
    (
        r#""a,b,,c".split(","), "  a  b ".split(), "a,b,c".split(",", 1), "a,b,c".rsplit(",", 1), "a\nb\r\nc".splitlines()"#,
        r"(['a', 'b', '', 'c'], ['a', 'b'], ['a', 'b,c'], ['a,b', 'c'], ['a', 'b', 'c'])",
    ),
    (
        r#""%5.2f|%-5d|%05d|%s|%r|%x|%%|%+d|%e|%g" % (3.14159, 42, 42, "s", "r", 255, 5, 12345.678, 0.00001234)"#,
        r" 3.14|42   |00042|s|'r'|ff|%|+5|1.234568e+04|1.234e-05",
    ),
    (
        r#""Hello".lower(), "hello world".title(), "ß".upper(), "x".join(["a", "b"]), "abc".replace("b", "bb"), " x ".strip(), "xxaxx".strip("x")"#,
        r"('hello', 'Hello World', 'SS', 'axb', 'abbc', 'x', 'a')",
    ),
    (
        r#""abc" < "abd", "a" < "B", "" < "a", "abc".startswith(("x", "a")), "abc".find("z"), "b" in "abc", "" in "abc", "abc".count("")"#,
        r"(True, False, True, True, -1, True, True, 4)",
    ),
    (
        r#"repr("it's"), repr('say "hi"'), repr("both ' and \""), repr("tab\there"), repr("é中\x00\x7f\u2028")"#,
        r#"('"it\'s"', '\'say "hi"\'', '\'both \\\' and "\'', "'tab\\there'", "'é中\\x00\\x7f\\u2028'")"#,
    ),
    (
        r#"str(1.0), str(None), str(True), str([1, "a", None]), str({"a": (1,)}), str((1,)), str(()), str({1}), repr(set()), str(frozenset())"#,
        r#"('1.0', 'None', 'True', "[1, 'a', None]", "{'a': (1,)}", '(1,)', '()', '{1}', 'set()', 'frozenset()')"#,
    ),
    (
        r#""a" * 3, 3 * "ab", "a" + "b", "abc" * 0, "abc" * -1, "a" "b" 'c', "ab" in "cabd", "中文".encode if False else "ok""#,
        r"('aaa', 'ababab', 'ab', '', '', 'abc', True, 'ok')",
    ),
    (
        r#""%s and %s" % ("a", "b"), "%s" % [1, 2], "%s" % ((1, 2),), "%d%%" % 50, "%(a)s" if False else "%3s|%-3s|" % ("x", "y")"#,
        r"('a and b', '[1, 2]', '(1, 2)', '50%', '  x|y  |')",
    ),
    (
        r#""Straße".casefold() if False else "ǅ".lower(), "i̇".upper(), "ﬁ".upper(), "ΑΣ".lower(), "a1".isalnum(), "١".isdigit()"#,
        r"('ǆ', 'İ', 'FI', 'ας', True, True)",
    ),
    (
        r"[1, 2, 3][-1], [1, 2, 3][1:], [1, 2, 3][::-1], [1, 2] + [3], [0] * 3, [1, [2, 3]] == [1, [2, 3]], [1, 2] < [1, 3], (1, 2) < (1, 2, 0)",
        r"(3, [2, 3], [3, 2, 1], [1, 2, 3], [0, 0, 0], True, True, True)",
    ),
    (
        r#"d = {"b": 1, "a": 2}; d["c"] = 0; d["b"] = 9; list(d), list(d.values()), list(d.items())"#,
        r"(['b', 'a', 'c'], [9, 2, 0], [('b', 9), ('a', 2), ('c', 0)])",
    ),
    (
        r#"{"a": 1} == {"a": 1.0}, {"a": 1, "b": 2} == {"b": 2, "a": 1}, {"a": 1} | {"b": 2}, {"a": 1} | {"a": 2}, list({"b": 1, "a": 2} | {"b": 3})"#,
        r"(True, True, {'a': 1, 'b': 2}, {'a': 2}, ['b', 'a'])",
    ),
    (r#"d = {"a": 1}; d.setdefault("b", []).append(1); d.get("z", 0), d.pop("a"), d, d.get("zz")"#, r"(0, 1, {'b': [1]}, None)"),
    (r#"d = {"a": 1, "b": 2}; del d["a"]; d["a"] = 3; list(d)"#, r"['b', 'a']"),
    (
        r#"sorted([3, 1, 2]), sorted("bca"), sorted([("b", 1), ("a", 2)]), sorted([1, 2, 3], key=lambda v: -v), sorted({"b": 1, "a": 2}), sorted([3, 1, 2], reverse=True)"#,
        r"([1, 2, 3], ['a', 'b', 'c'], [('a', 2), ('b', 1)], [3, 2, 1], ['a', 'b'], [3, 2, 1])",
    ),
    (
        r#"sorted([1, True, 0, False, 1.0]), sorted(["b", "A", "a", "B"]), sorted(["b", "A", "a", "B"], key=str.lower), sorted([(1, "b"), (1, "a")], key=lambda p: p[0])"#,
        r"([0, False, 1, True, 1.0], ['A', 'B', 'a', 'b'], ['A', 'a', 'b', 'B'], [(1, 'b'), (1, 'a')])",
    ),
    (
        r#"list(zip("ab", [1, 2, 3])), list(enumerate("ab", 1)), list(map(str, [1, 2])), list(filter(None, [0, 1, "", "a"])), list(reversed([1, 2])), list(range(5, 0, -2))"#,
        r"([('a', 1), ('b', 2)], [(1, 'a'), (2, 'b')], ['1', '2'], [1, 'a'], [2, 1], [5, 3, 1])",
    ),
    (
        r"any([]), all([]), any(x > 1 for x in [1, 2]), all(x > 1 for x in [1, 2]), len({1, 1.0, True}), {1, 2} == {2, 1}, {1} < {1, 2}, {1, 2} & {2, 3} == {2}",
        r"(False, True, True, False, 1, True, True, True)",
    ),
    (
        r"a = [1, 2]; b = a; b.append(3); a, a is b, a == [1, 2, 3], a is [1, 2, 3], a is not None",
        r"([1, 2, 3], True, True, False, True)",
    ),
    (r"a = [1, 2]; b = a[:]; b.append(3); c = a + []; a, b, c is a", r"([1, 2], [1, 2, 3], False)"),
    (
        r#"[x * y for x in range(3) for y in range(3) if x != y], {k: v for k, v in zip("ab", [1, 2])}, [[j for j in range(i)] for i in range(3)]"#,
        r"([0, 0, 0, 2, 0, 2], {'a': 1, 'b': 2}, [[], [0], [0, 1]])",
    ),
    (r"x = 5; y = [x for x in range(3)]; x, y", r"(5, [0, 1, 2])"),
    (r"fs = [lambda: i for i in range(3)]; [f() for f in fs]", r"[2, 2, 2]"),
    (r"fs = [lambda i=i: i for i in range(3)]; [f() for f in fs]", r"[0, 1, 2]"),
    (r"f = lambda a, b=2: a + b; f(1), f(1, 3), f(b=5, a=1)", r"(3, 4, 6)"),
    (r#"next(x for x in [1, 2, 3] if x > 1), next(iter([]), "none"), next((x for x in [] if x), None)"#, r"(2, 'none', None)"),
    (
        r#"1 if [] else 2, [] or "d", 0 and 1 / 0, None or 0 or "", not [], [] and 1, "a" and "b", 0 or None"#,
        r"(2, 'd', 0, '', True, [], 'b', None)",
    ),
    (
        r#"None is None, None == 0, [] is not None, type(1) is int, type(True) is bool, isinstance(True, int), isinstance(1, (str, int)), type("") == str"#,
        r"(True, False, True, True, True, True, True, True)",
    ),
    (r#"g = (x for x in [1, 2, 3]); list(g), list(g), sum(x for x in [1, 2]), max(x for x in "abc")"#, r"([1, 2, 3], [], 3, 'c')"),
    (r"it = iter([1, 2, 3]); next(it), list(it), list(it)", r"(1, [2, 3], [])"),
    (
        r"t = (1, [2]); t[1].append(3); t, len(t), t + (4,), t * 2 == (1, [2, 3], 1, [2, 3])",
        r"((1, [2, 3]), 2, (1, [2, 3], 4), True)",
    ),
    (r"l = [3, 1, 2]; r = l.sort(); l, r, l.pop(), l.pop(0), l, l.index(2) if 2 in l else None", r"([2], None, 3, 1, [2], 0)"),
    (
        r#"len({(1, 2): "a", (1, 2.0): "b"}), {"a": 1}.items() == {"a": 1}.items(), list({"a": 1}.keys() & {"a", "b"}), ("a", 1) in {"a": 1}.items()"#,
        r"(1, True, ['a'], True)",
    ),
    (
        r#"[1, 2] * 2, [[0] * 2] * 2, [[]] * 2 == [[], []], bool([[]]), bool(()), bool({}), bool(" "), bool(0.0), bool(range(0))"#,
        r"([1, 2, 1, 2], [[0, 0], [0, 0]], True, True, False, False, True, False, False)",
    ),
    (
        r"range(3) == range(0, 3, 1), range(0) == range(5, 1), range(10)[-1], 5 in range(0, 10, 2), len(range(0, 10, 3)), str(range(3))",
        r"(True, True, 9, False, 4, 'range(0, 3)')",
    ),
    (r"x = y = []; x.append(1); y, x is y", r"([1], True)"),
    (r"a, b = 1, 2; a, b = b, a; a, b", r"(2, 1)"),
    (r"(a, b), c = (1, 2), 3; a + b + c", r"6"),
    (r"x = [1, 2]; x += [3]; x *= 2; x", r"[1, 2, 3, 1, 2, 3]"),
    (r#"s = "ab"; s += "c"; s *= 2; s"#, r"abcabc"),
    (r#"d = {"n": 1}; d["n"] += 1; d["l"] = []; d["l"] += [1]; d"#, r"{'n': 2, 'l': [1]}"),
    (
        r"str(1e100), str(1.5e-10), str(float(10 ** 16)), str(123456789.123456789), repr(0.1), str(1.0e15), str(-1e-7), str(2 ** 0.5)",
        r"('1e+100', '1.5e-10', '1e+16', '123456789.12345679', '0.1', '1000000000000000.0', '-1e-07', '1.4142135623730951')",
    ),
    (r#"json.dumps({"a": [1, 2.0, None, True], "é": "中"})"#, r#"{"a": [1, 2.0, null, true], "\u00e9": "\u4e2d"}"#),
    (
        r#"json.dumps([1, 2], separators=(",", ":")), json.dumps({"b": 1, "a": 2}, sort_keys=True), json.dumps("é", ensure_ascii=False), json.dumps("é")"#,
        r#"('[1,2]', '{"a": 2, "b": 1}', '"é"', '"\\u00e9"')"#,
    ),
    (
        r#"json.dumps({"b": [1, {"c": []}], "a": {}}, indent=2)"#,
        r#"{
  "b": [
    1,
    {
      "c": []
    }
  ],
  "a": {}
}"#,
    ),
    (
        r#"json.loads('{"a": [1, 2.5, "x", null, true]}'), json.loads("1e2"), json.loads("-0"), json.loads("-0.0"), json.loads('"\u00e9"')"#,
        r"({'a': [1, 2.5, 'x', None, True]}, 100.0, 0, -0.0, 'é')",
    ),
    (
        r#"json.dumps({1: "a", None: "b", True: "c", 1.5: "d"}), json.dumps(float("inf")), json.dumps((1, 2)), json.dumps("\x7f\x00/"), json.dumps(10 ** 20)"#,
        r#"('{"1": "c", "null": "b", "1.5": "d"}', 'Infinity', '[1, 2]', '"\\u007f\\u0000/"', '100000000000000000000')"#,
    ),
    (
        r#"json.loads(json.dumps({"k": [1, {"n": None}]})) == {"k": [1, {"n": None}]}, json.loads("[]") == [], json.loads(" 1 ")"#,
        r"(True, True, 1)",
    ),
    (r#""{}-{}".format(1, "a"), "{0}{1}{0}".format("x", "y")"#, r"('1-a', 'xyx')"),
];

/// 子命令、程序、Python 报的那一行（退出码 1）
const ERRORS: &[(&str, &str, &str)] = &[
    ("val", r#"{}["k"]"#, r"KeyError: 'k'"),
    ("val", r#"{"a": 1}[1]"#, r"KeyError: 1"),
    ("val", r"[1][5]", r"IndexError: list index out of range"),
    ("val", r#""abc"[3]"#, r"IndexError: string index out of range"),
    ("val", r"(1,)[-2]", r"IndexError: tuple index out of range"),
    ("val", r"1 / 0", r"ZeroDivisionError: division by zero"),
    ("val", r"1 % 0", r"ZeroDivisionError: division by zero"),
    ("val", r"1 // 0.0", r"ZeroDivisionError: division by zero"),
    ("val", r"0 ** -1", r"ZeroDivisionError: zero to a negative power"),
    ("val", r#""a" + 1"#, r#"TypeError: can only concatenate str (not "int") to str"#),
    ("val", r#"1 + "a""#, r"TypeError: unsupported operand type(s) for +: 'int' and 'str'"),
    ("val", r"[1] + (2,)", r#"TypeError: can only concatenate list (not "tuple") to list"#),
    ("val", r#"int("x")"#, r"ValueError: invalid literal for int() with base 10: 'x'"),
    ("val", r#"int("1.5")"#, r"ValueError: invalid literal for int() with base 10: '1.5'"),
    ("val", r#"float("abc")"#, r"ValueError: could not convert string to float: 'abc'"),
    ("val", r"int([])", r"TypeError: int() argument must be a string, a bytes-like object or a real number, not 'list'"),
    ("val", r"None.x", r"AttributeError: 'NoneType' object has no attribute 'x'"),
    ("val", r#""s".nope()"#, r"AttributeError: 'str' object has no attribute 'nope'"),
    ("val", r"[].nope", r"AttributeError: 'list' object has no attribute 'nope'"),
    ("val", r"{}.nope", r"AttributeError: 'dict' object has no attribute 'nope'"),
    ("val", r"(1).nope", r"AttributeError: 'int' object has no attribute 'nope'"),
    ("val", r"None()", r"TypeError: 'NoneType' object is not callable"),
    ("val", r#""s"()"#, r"TypeError: 'str' object is not callable"),
    ("val", r"1[0]", r"TypeError: 'int' object is not subscriptable"),
    ("val", r"None[0]", r"TypeError: 'NoneType' object is not subscriptable"),
    ("val", r"len(5)", r"TypeError: object of type 'int' has no len()"),
    ("val", r"len()", r"TypeError: len() takes exactly one argument (0 given)"),
    ("val", r"next(iter([]))", r"StopIteration"),
    ("val", r#"sorted([1, "a"])"#, r"TypeError: '<' not supported between instances of 'str' and 'int'"),
    ("val", r"{[]: 1}", r"TypeError: cannot use 'list' as a dict key (unhashable type: 'list')"),
    ("val", r"{{}}", r"TypeError: cannot use 'dict' as a set element (unhashable type: 'dict')"),
    ("val", r"max([])", r"ValueError: max() iterable argument is empty"),
    ("val", r"min(())", r"ValueError: min() iterable argument is empty"),
    ("val", r"[1, 2].index(3)", r"ValueError: list.index(x): x not in list"),
    ("val", r#"{}.pop("k")"#, r"KeyError: 'k'"),
    ("val", r#""abc".index("z")"#, r"ValueError: substring not found"),
    ("val", r"[].pop()", r"IndexError: pop from empty list"),
    ("val", r"{}.popitem()", r"KeyError: 'popitem(): dictionary is empty'"),
    ("val", r"[1].remove(2)", r"ValueError: list.remove(x): x not in list"),
    ("val", r"x, y = 1, 2, 3; x", r"ValueError: too many values to unpack (expected 2, got 3)"),
    ("val", r"x, y = [1]; x", r"ValueError: not enough values to unpack (expected 2, got 1)"),
    ("val", r"x, y = 5; x", r"TypeError: cannot unpack non-iterable int object"),
    (
        "val",
        r#"json.loads("{")"#,
        r"json.decoder.JSONDecodeError: Expecting property name enclosed in double quotes: line 1 column 2 (char 1)",
    ),
    ("val", r#"json.loads("")"#, r"json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)"),
    (
        "val",
        r#"json.loads("[1,]")"#,
        r"json.decoder.JSONDecodeError: Illegal trailing comma before end of array: line 1 column 3 (char 2)",
    ),
    ("val", r#"json.loads("nul")"#, r"json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)"),
    ("val", r"json.dumps({1, 2})", r"TypeError: Object of type set is not JSON serializable"),
    ("val", r"json.dumps({(1, 2): 3})", r"TypeError: keys must be str, int, float, bool or None, not tuple"),
    ("val", r"json.dumps(lambda: 1)", r"TypeError: Object of type function is not JSON serializable"),
    ("val", r#"open("/nonexistent/x")"#, r"FileNotFoundError: [Errno 2] No such file or directory: '/nonexistent/x'"),
    ("val", r#"open("/nonexistent/x").read()"#, r"FileNotFoundError: [Errno 2] No such file or directory: '/nonexistent/x'"),
    ("val", r#"json.load(open("/"))"#, r"IsADirectoryError: [Errno 21] Is a directory: '/'"),
    ("val", r#""%d" % "x""#, r"TypeError: %d format: a real number is required, not str"),
    ("val", r#""%s %s" % (1,)"#, r"TypeError: not enough arguments for format string"),
    ("val", r#""%s" % (1, 2)"#, r"TypeError: not all arguments converted during string formatting"),
    ("val", r#"1 < "a""#, r"TypeError: '<' not supported between instances of 'int' and 'str'"),
    ("val", r"[1] < (1,)", r"TypeError: '<' not supported between instances of 'list' and 'tuple'"),
    ("val", r"None < None", r"TypeError: '<' not supported between instances of 'NoneType' and 'NoneType'"),
    ("val", r"{} < {}", r"TypeError: '<' not supported between instances of 'dict' and 'dict'"),
    ("val", r#"-"a""#, r"TypeError: bad operand type for unary -: 'str'"),
    ("val", r#"not_defined if False else {}["x"]"#, r"KeyError: 'x'"),
    ("val", r"(lambda a: a)()", r"TypeError: <lambda>() missing 1 required positional argument: 'a'"),
    ("val", r"(lambda a: a)(1, 2)", r"TypeError: <lambda>() takes 1 positional argument but 2 were given"),
    ("val", r"(lambda a: a)(b=1)", r"TypeError: <lambda>() got an unexpected keyword argument 'b'"),
    ("val", r"range(1, 2, 0)", r"ValueError: range() arg 3 must not be zero"),
    ("val", r"list(range(1.5))", r"TypeError: 'float' object cannot be interpreted as an integer"),
    ("val", r#""a".join([1])"#, r"TypeError: sequence item 0: expected str instance, int found"),
    ("val", r#""abc".split("")"#, r"ValueError: empty separator"),
    ("val", r"[1, 2, 3][1.0]", r"TypeError: list indices must be integers or slices, not float"),
    ("val", r#"[1, 2, 3]["a"]"#, r"TypeError: list indices must be integers or slices, not str"),
    ("val", r#"{"a": 1}.get()"#, r"TypeError: get expected at least 1 argument, got 0"),
    ("val", r"isinstance(1, 2)", r"TypeError: isinstance() arg 2 must be a type, a tuple of types, or a union"),
    ("val", r#"sum(["a", "b"])"#, r"TypeError: unsupported operand type(s) for +: 'int' and 'str'"),
    ("val", r"sum([[1]], 0)", r"TypeError: unsupported operand type(s) for +: 'int' and 'list'"),
    ("val", r#"abs("x")"#, r"TypeError: bad operand type for abs(): 'str'"),
    ("val", r#"round("x")"#, r"TypeError: type str doesn't define __round__ method"),
    ("val", r"enumerate(5)", r"TypeError: 'int' object is not iterable"),
    ("val", r"zip(5)", r"TypeError: 'int' object is not iterable"),
    ("val", r"dict(5)", r"TypeError: 'int' object is not iterable"),
    ("val", r"set(5)", r"TypeError: 'int' object is not iterable"),
    ("val", r"tuple(5)", r"TypeError: 'int' object is not iterable"),
    ("run", r"assert False", r"AssertionError"),
    ("run", r#"assert 1 == 2, "values differ""#, r"AssertionError: values differ"),
    ("run", r#"assert [], {"k": 1}"#, r"AssertionError: {'k': 1}"),
    ("run", r"x = []; assert x, x", r"AssertionError: []"),
];

/// 这里不认的：val、run 都是退出码 2（不认识的名字、Python 有而这里没实现的、不支持的语法、写错了的）
const REFUSED: &[&str] = &[
    r"nope",
    r"nope()",
    r#"d["x"]"#,
    r"1 is 1",
    r#""a" is "a""#,
    r"1.0 is 1.0",
    r"list({1, 2})",
    r"[x for x in {1, 2}]",
    r"str({1, 2})",
    r"{1, 2}.pop()",
    r#"f"{1}""#,
    r#"b"x""#,
    r#""""x""""#,
    r"1j",
    r"[*[1]]",
    r"{**{}}",
    r"(x := 1)",
    r"lambda *a: a",
    r"lambda **k: k",
    r"print(*[1])",
    r"x: int = 1",
    r"x.y = 1",
    r"import os",
    r"from json import loads",
    r"def f(): pass",
    r"for x in []: pass",
    r"while 1: pass",
    r"if 1: pass",
    r"try: pass",
    r#"with open("x") as f: pass"#,
    r"class A: pass",
    r"return 1",
    r"yield 1",
    r"await x",
    r"global x",
    r"raise ValueError",
    r"x = 1; del x; x",
    r#"open("/dev/null", "w")"#,
    r#"open("/dev/null", mode="w")"#,
    r#"__import__("os")"#,
    r#"eval("1")"#,
    r#"exec("1")"#,
    r#"compile("1", "", "eval")"#,
    r"globals()",
    r"locals()",
    r"vars()",
    r"dir()",
    r"input()",
    r"id(1)",
    r#"hash("a")"#,
    r"object()",
    r#""a".encode()"#,
    r"bytes(1)",
    r"bytearray()",
    r"memoryview",
    r"complex(1)",
    r"1 @ 2",
    r"a[1, 2]",
    r"[1][...]",
    r"...",
    r"1 if 1",
    r"1 +",
    r"(1",
    r"1 2",
    r"1 = 2",
    r#""abc"#,
    r"$",
    r"x ==",
    r"lambda",
    r"not",
    r"1;; 2",
    r"; 1",
    " x = 1\n x",
    "x = 1\n  x",
    r"1 <> 2",
    r"print 1",
    r"sys.stdin.read()",
    r"sys.argv",
    r"sys.path",
    r"time.sleep(0)",
    r"json.dump",
    r"json.JSONDecodeError",
    r"os.path",
    r"len.__name__",
    r"(1).__class__",
    r"[].__len__()",
    r"1 .real",
    r#""%c" % 65"#,
    r#""%(a)s" % {"a": 1}"#,
    r"2 ** 200",
    r"10 ** 39 // 1",
    r#"int("9" * 40)"#,
    r"1e400 if False else 170141183460469231731687303715884105728",
    r"1 << 2",
    r"8 >> 1",
    r"~1",
    r"hash(1)",
    r#""a".partition("a")"#,
    r"l = [1]; l[0:1] = []",
    r"l = [1, 2]; del l[0:1]",
    r"range(3)[1:]",
    r#""²".isdigit()"#,
    r#""{k}".format(k=3)"#,
];

#[test]
fn semantics_match_python() {
    for (program, want) in SEMANTICS {
        let got = run(&["val", "--", program]);
        assert_eq!((got.code, got.out.as_str(), got.err.as_str()), (0, format!("{want}\n").as_str(), ""), "{program}");
    }
}

#[test]
fn errors_are_worded_like_python() {
    for (sub, program, want) in ERRORS {
        let got = run(&[sub, "--", program]);
        assert_eq!((got.code, got.out.as_str(), got.err.as_str()), (1, "", format!("{want}\n").as_str()), "{program}");
    }
}

#[test]
fn what_is_not_supported_exits_2() {
    for program in REFUSED {
        for sub in ["val", "run"] {
            let got = run(&[sub, "--", program]);
            assert_eq!(got.code, 2, "{sub} {program:?}: stdout {:?}，stderr {:?}", got.out, got.err);
            assert!(got.err.starts_with("cca-test："), "{sub} {program:?}: {:?}", got.err);
            assert!(got.out.is_empty(), "{sub} {program:?}: {:?}", got.out);
        }
    }
}

// ---- ③ CCA_TEST_TRAP ----

/// 记下的那几行：每行拆成 子命令、原因、程序
fn trap_lines(trap: &Path) -> Vec<Vec<String>> {
    let text = std::fs::read_to_string(trap).unwrap_or_default();
    assert!(text.is_empty() || text.ends_with('\n'), "最后一行没写完：{text:?}");
    text.lines().map(|line| line.split('\t').map(str::to_string).collect()).collect()
}

#[test]
fn trap_only_when_exit_is_2() {
    let tmp = Tmp::new("trap");
    let trap = tmp.0.join("trap.log");
    // 退出码 0、1 是 Python 那边的结果（Python 层面的异常也是）：什么都不记，文件都不建
    let quiet: [&[&str]; 8] = [
        &["check", "1 == 1"],
        &["check", "1 == 2"],
        &["val", "1 / 0"],
        &["val", "{}['k']"],
        &["run", "sys.exit(1)"],
        &["run", "assert False"],
        &["syntax", "1 + 1"],
        &["evq", "/nonexistent/ev.jsonl", "True"],
    ];
    for cmd in quiet {
        let got = run_full(cmd, "", Some(&trap));
        assert!(got.code == 0 || got.code == 1, "{cmd:?}: {}", got.code);
        assert!(!trap.exists(), "{cmd:?} 不该往 CCA_TEST_TRAP 里记");
    }
    // 程序自己 sys.exit(2)：也是 Python 那边的结果，不是这里不敢担保，不记
    let got = run_full(&["run", "sys.exit(2)"], "", Some(&trap));
    assert_eq!((got.code, got.err.as_str()), (2, ""));
    assert!(!trap.exists());
}

#[test]
fn trap_gets_one_line_per_exit_2() {
    let tmp = Tmp::new("trap");
    let trap = tmp.0.join("trap.log");
    // 命令、记下的子命令、记下的程序（用法不对、还没读到程序的，记的是整条命令的参数）
    let cases: [(&[&str], &str, &str); 11] = [
        (&["val", "nope + 1"], "val", "nope + 1"),
        (&["check", "1 +"], "check", "1 +"),
        (&["run", "import os"], "run", "import os"),
        (&["val", "'a'.partition('a')"], "val", "'a'.partition('a')"),
        (&["val", "sorted([1], cmp=None)"], "val", "sorted([1], cmp=None)"),
        (&["jok", "f\"x\"", "/nonexistent"], "jok", "f\"x\""),
        (&["evq", "/nonexistent/ev.jsonl", "1 +"], "evq", "1 +"),
        (&["edit", "j=/nonexistent/x.json", "def f(): pass"], "edit", "def f(): pass"),
        (&["syntax", "1 + 1", "nope(", "also bad ("], "syntax", "nope("),
        (&["val"], "val", ""),
        (&["check", "--bogus", "1"], "check", "--bogus 1"),
    ];
    for (cmd, sub, program) in cases {
        let _ = std::fs::remove_file(&trap);
        let got = run_full(cmd, "", Some(&trap));
        assert_eq!(got.code, 2, "{cmd:?}: {:?}", got.err);
        let lines = trap_lines(&trap);
        assert_eq!(lines.len(), 1, "{cmd:?}: {lines:?}");
        let [got_sub, why, got_program] = &lines[0][..] else { panic!("{cmd:?}: 不是三栏：{lines:?}") };
        assert_eq!((got_sub.as_str(), got_program.as_str()), (sub, program), "{cmd:?}");
        // 原因就是 stderr 上 “cca-test：” 后面的那一句
        assert!(!why.is_empty() && got.err.starts_with(&format!("cca-test：{why}")), "{cmd:?}: {why:?} / {:?}", got.err);
    }
    // 是追加：几次就是几行
    let _ = std::fs::remove_file(&trap);
    for _ in 0..3 {
        run_full(&["val", "nope"], "", Some(&trap));
    }
    assert_eq!(trap_lines(&trap).len(), 3);
    // 程序里的换行、制表符写成转义的样子（一条记录占一行），最多留 300 个字符
    let _ = std::fs::remove_file(&trap);
    let long = format!("nope(\n\t{}1)", "1 + ".repeat(200));
    assert_eq!(run_full(&["val", &long], "", Some(&trap)).code, 2);
    let lines = trap_lines(&trap);
    assert_eq!(lines.len(), 1);
    let want: String = long.replace('\n', "\\n").replace('\t', "\\t").chars().take(300).collect();
    assert_eq!(lines[0][2], want);
    assert_eq!(lines[0][2].chars().count(), 300);
    // 写不进去（目录不存在）：不管它，照样是退出码 2
    let nowhere = tmp.0.join("no-such-dir").join("trap.log");
    assert_eq!(run_full(&["val", "nope"], "", Some(&nowhere)).code, 2);
    assert!(!nowhere.exists());
}

#[test]
fn trap_lines_do_not_interleave() {
    // 几个进程同时往里写（这里用线程代替）：一行是一次 write 写完的，不会串
    let tmp = Tmp::new("trap");
    let trap = tmp.0.join("trap.log");
    let workers: Vec<_> = (0..8)
        .map(|n| {
            let trap = trap.clone();
            std::thread::spawn(move || {
                for i in 0..40 {
                    let program = format!("nope_{n}_{i}({})", "x, ".repeat(60));
                    assert_eq!(run_full(&["val", &program], "", Some(&trap)).code, 2);
                }
            })
        })
        .collect();
    for w in workers {
        w.join().unwrap();
    }
    let lines = trap_lines(&trap);
    assert_eq!(lines.len(), 8 * 40);
    let mut seen: Vec<String> = Vec::new();
    for line in &lines {
        let [sub, why, program] = &line[..] else { panic!("不是三栏：{line:?}") };
        assert_eq!(sub, "val");
        assert!(!why.is_empty());
        assert!(program.starts_with("nope_") && program.ends_with("x, )"), "{program}");
        seen.push(program.clone());
    }
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 8 * 40);
}

#[test]
fn trap_comes_from_the_environment() {
    // CCA_TEST_TRAP 没设、设成空的：不记
    assert_eq!(cli::trap_of(None), None);
    assert_eq!(cli::trap_of(Some("".into())), None);
    assert_eq!(cli::trap_of(Some("/x/trap.log".into())), Some(PathBuf::from("/x/trap.log")));
}

// ---- ③ stdout 和 stderr 的先后 ----

/// 两路输出接到同一个地方（2>&1）时看到的样子
fn run_merged(cmd: &[&str], line_out: bool) -> (i32, String) {
    let both = Buf::default();
    let io = Io {
        stdin: Box::new(Cursor::new(Vec::new())),
        out: Box::new(both.clone()),
        err: Box::new(both.clone()),
        line_out,
        trap: None,
    };
    let code = cli::run(sub_named(cmd[0]), cmd[1..].iter().map(|a| a.to_string()).collect(), io);
    (code, both.text())
}

#[test]
fn output_order_is_pythons() {
    // stdout 接着管道、文件时（回归里都是）：Python 把 stdout 攒到结束才写，stderr 是写的内容里有换行就写出去——
    // 接到一起时 stderr 的在前面。期望的都是真的 python3 -c '…' 2>&1 的输出（去掉了回溯的那几行）
    for (program, want) in [
        (r#"print("a"); print("b", file=sys.stderr); print("c")"#, "b\na\nc\n"),
        (r#"print("before"); 1/0"#, "ZeroDivisionError: division by zero\nbefore\n"),
        (r#"print("a", flush=True); print("b", file=sys.stderr); print("c")"#, "a\nb\nc\n"),
        (r#"print("x", end="", file=sys.stderr); print("y"); print("z", file=sys.stderr)"#, "xz\ny\n"),
        (r#"print("x", end="", file=sys.stderr); print("y")"#, "y\nx"),
        (r#"print("a\nb", end="", file=sys.stderr); print("y"); print("!", file=sys.stderr, end="")"#, "a\nby\n!"),
        (r#"print("a"); sys.exit("bye")"#, "bye\na\n"),
        (r#"print("x", end="", file=sys.stderr, flush=True); print("y", flush=0); print("z", end="", file=sys.stderr)"#, "xy\nz"),
        (r#"print("a", end=""); print("b", file=sys.stderr, end=""); assert False, "m""#, "bAssertionError: m\na"),
    ] {
        assert_eq!(run_merged(&["run", program], false).1, want, "{program}");
    }
    // 接着终端时 stdout 也是一行一行写
    assert_eq!(run_merged(&["run", r#"print("a"); print("b", file=sys.stderr); print("c")"#], true).1, "a\nb\nc\n");
    assert_eq!(run_merged(&["run", r#"print("before"); 1/0"#], true).1, "before\nZeroDivisionError: division by zero\n");
}

#[test]
fn big_output_is_cut_where_python_cuts_it() {
    // stdout 攒得多了 Python 会先写出去一部分。往 stdout 写一大堆、再往 stderr 写一个 Z：Z 前面已经有多少字节，
    // 就是那时 stdout 真的写出去了多少。数都是 Python 3.14 量出来的（python3 -c '…' > 文件 2>&1）
    for (program, before_z) in [
        (r#"print("c" * 131071, end="")"#, 0),
        (r#"print("c" * 131072, end="")"#, 131072),
        (r#"print("c" * 131073, end="")"#, 131073),
        (r#"print("a" * 8192, end=""); print("c" * 122880, end="")"#, 0),
        (r#"print("a" * 8192, end=""); print("c" * 122881, end="")"#, 8192),
        (r#"print("f" * 100, end=""); print("g" * 131000, end="")"#, 100),
        (r#"print("f" * 1, end=""); print("g" * 131071, end="")"#, 0),
        (r#"print("f" * 1, end=""); print("g" * 131072, end="")"#, 131073),
        (r#"print("f" * 8191, end=""); print("g" * 1, end=""); print("h" * 122880, end="")"#, 0),
        (r#"print("f" * 8191, end=""); print("g" * 2, end=""); print("h" * 122880, end="")"#, 8193),
        (r#"print("f" * 8000, end=""); print("g" * 8193, end=""); print("h" * 114880, end="")"#, 16193),
        (r#"[print("h" * 1000, end="") for _ in range(200)]"#, 126000),
        (r#"[print("h" * 1024, end="") for _ in range(200)]"#, 131072),
        (r#"[print("h" * 1000) for _ in range(200)]"#, 126125),
        (r#"[print("c" * 4096) for _ in range(40)]"#, 122909),
        (r#"[print("c" * 8193, end="") for _ in range(40)]"#, 245790),
        (r#"[print("c" * 100, end="") for _ in range(2000)]"#, 123000),
        (r#"print("f" * 9000, end=""); [print("h" * 1000, end="") for _ in range(200)]"#, 126000),
        (
            r#"print("f" * 140000, end="--"); [print("g" * 1000, end="--") for _ in range(17)]; [print("h" * 1000, end="") for _ in range(200)]"#,
            266036,
        ),
    ] {
        let (code, merged) = run_merged(&["run", &format!(r#"{program}; print("Z", file=sys.stderr)"#)], false);
        assert_eq!((code, merged.find('Z')), (0, Some(before_z)), "{program}");
    }
}

/// 写不进去的那一路：读的那一头已经关了的管道
struct Closed;

impl Write for Closed {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::from_raw_os_error(libc::EPIPE))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// stdout 写不进去时的退出码和 stderr 上的字
fn closed_out(cmd: &[&str]) -> (i32, String) {
    let err = Buf::default();
    let io = Io {
        stdin: Box::new(Cursor::new(Vec::new())),
        out: Box::new(Closed),
        err: Box::new(err.clone()),
        line_out: false,
        trap: None,
    };
    let code = cli::run(sub_named(cmd[0]), cmd[1..].iter().map(|a| a.to_string()).collect(), io);
    (code, err.text())
}

/// stderr 写不进去时的退出码和 stdout 上的字
fn closed_err(cmd: &[&str]) -> (i32, String) {
    let out = Buf::default();
    let io = Io {
        stdin: Box::new(Cursor::new(Vec::new())),
        out: Box::new(out.clone()),
        err: Box::new(Closed),
        line_out: false,
        trap: None,
    };
    let code = cli::run(sub_named(cmd[0]), cmd[1..].iter().map(|a| a.to_string()).collect(), io);
    (code, out.text())
}

#[test]
fn closed_pipe_is_exit_120() {
    // Python：结束时 stdout 写不出去，就在 stderr 上说这两行，退出码是 120（不管本来该是几）。都和真的 python3 对过
    let broken = "BrokenPipeError: [Errno 32] Broken pipe\n";
    let ignored = format!("Exception ignored while flushing sys.stdout:\n{broken}");
    let cases: [(&[&str], &str); 6] = [
        (&["run", r#"print("x")"#], ""),
        (&["val", "1 + 1"], ""),
        (&["run", r#"print("x"); sys.exit(3)"#], ""),
        (&["run", r#"print("x"); 1/0"#], "ZeroDivisionError: division by zero\n"),
        // 写的时候就发现了（flush=True；攒满了一缓冲区）：print 自己抛 BrokenPipeError，没写成的结束时再试一次
        (&["run", r#"print("x", flush=True); print("after", file=sys.stderr)"#], broken),
        (&["run", r#"[print("x" * 1000) for _ in range(200)]"#], broken),
    ];
    for (cmd, before) in cases {
        assert_eq!(closed_out(cmd), (120, format!("{before}{ignored}")), "{cmd:?}");
    }
    // 一缓冲区那么大的一块是直接写的：没写成就丢了，结束时没有攒着的，退出码是异常的那个 1
    assert_eq!(closed_out(&["run", r#"print("x" * 200000)"#]), (1, broken.to_string()));
    // 没往 stdout 写过东西的不受影响
    assert_eq!(closed_out(&["check", "1 == 1"]), (0, String::new()));
    assert_eq!(closed_out(&["run", r#"print("e", file=sys.stderr)"#]), (0, "e\n".to_string()));

    // stderr 写不出去：没处说，退出码一样是 120
    assert_eq!(closed_err(&["run", r#"print("e", file=sys.stderr)"#]), (120, String::new()));
    assert_eq!(closed_err(&["run", r#"print("x"); 1/0"#]), (120, "x\n".to_string()));
    assert_eq!(closed_err(&["run", r#"sys.exit("bye")"#]), (120, String::new()));
    assert_eq!(closed_err(&["run", r#"print("x"); sys.exit(3)"#]), (3, "x\n".to_string()));
    assert_eq!(closed_err(&["check", "1 == 2"]), (1, String::new()));
}

#[test]
fn exit_2_is_not_hidden_by_a_closed_pipe() {
    // 不敢担保的那个 2 是给回归看的，不能让 120 盖掉；记录照样写
    let tmp = Tmp::new("closed");
    let trap = tmp.path("trap");
    let io = Io {
        stdin: Box::new(Cursor::new(Vec::new())),
        out: Box::new(Closed),
        err: Box::new(Buf::default()),
        line_out: false,
        trap: Some(PathBuf::from(&trap)),
    };
    assert_eq!(cli::run(Sub::Run, vec![r#"print("x"); hash(1)"#.to_string()], io), 2);
    assert_eq!(std::fs::read_to_string(&trap).unwrap().lines().count(), 1);
}

// ---- ③ edit 怎么写文件 ----

#[test]
fn edit_in_place_goes_through_a_symlink() {
    let tmp = Tmp::new("edit");
    let real = tmp.write("real.json", "{\"a\": 1}");
    let link = tmp.path("link.json");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
    let got = run(&["edit", &format!("j={link}"), "j['b'] = 'é'"]);
    assert_eq!((got.code, got.out.as_str(), got.err.as_str()), (0, "", ""));
    // 链接还是链接，改的是它指着的那个文件；权限不变；写出来和 json.dump 默认的一样（非 ASCII 转义、不带结尾的换行）
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    assert_eq!(tmp.read("real.json"), r#"{"a": 1, "b": "\u00e9"}"#);
    assert_eq!(mode_of(&real), 0o600);
    assert_eq!(tmp.names(), ["link.json", "real.json"]);
}

#[test]
fn edit_atomic_replaces_the_file() {
    let tmp = Tmp::new("edit");
    let real = tmp.write("real.json", "{\"a\": 1}");
    let link = tmp.path("link.json");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
    let got = run(&["edit", "--atomic", "--indent", "2", "--unicode", &format!("j={link}"), "j['b'] = 'é'"]);
    assert_eq!((got.code, got.out.as_str(), got.err.as_str()), (0, "", ""));
    // 和 Python 里 open(临时文件, "w") 再 os.replace 一样：链接换成了普通文件，原来指着的那个没动
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_file());
    assert_eq!(tmp.read("link.json"), "{\n  \"a\": 1,\n  \"b\": \"é\"\n}");
    assert_eq!(tmp.read("real.json"), "{\"a\": 1}");
    // 新文件的权限是新建文件默认的（照 umask），不是原来那个的；旁边不留临时文件
    let fresh = tmp.write("fresh", "");
    assert_eq!(mode_of(&link), mode_of(&fresh));
    assert_eq!(mode_of(&real), 0o600);
    assert_eq!(tmp.names(), ["fresh", "link.json", "real.json"]);
}

#[test]
fn edit_leaves_the_file_alone_when_it_fails() {
    let tmp = Tmp::new("edit");
    let before = "{\n  \"a\": 1\n}\n";
    let file = tmp.write("t.json", before);
    let target = format!("j={file}");
    // 语句出了异常、写不成 JSON 的（集合）、程序自己退出：文件原样，旁边不留临时文件
    for (program, code, err) in [
        ("j['b'] = 1; j['x']['y'] = 2", 1, "KeyError: 'x'\n"),
        ("j['s'] = {1}", 1, "TypeError: Object of type set is not JSON serializable\n"),
        ("j['b'] = 1; sys.exit(3)", 3, ""),
    ] {
        for atomic in [false, true] {
            let mut cmd = vec!["edit"];
            if atomic {
                cmd.push("--atomic");
            }
            cmd.extend([target.as_str(), program]);
            let got = run(&cmd);
            assert_eq!((got.code, got.err.as_str()), (code, err), "{program}");
            assert_eq!(tmp.read("t.json"), before, "{program}");
            assert_eq!(tmp.names(), ["t.json"], "{program}");
        }
    }
    // 这里不支持的写法（退出码 2）也一样
    let got = run(&["edit", &target, "j['b'] = 1; import os"]);
    assert_eq!(got.code, 2);
    assert_eq!(tmp.read("t.json"), before);
    // 文件不存在：不带 --or-empty 是 FileNotFoundError，也不建；带着就从 {} 开始、建出来
    let missing = tmp.path("new.json");
    let got = run(&["edit", &format!("j={missing}"), "j['a'] = 1"]);
    assert_eq!(got.code, 1);
    assert!(got.err.starts_with("FileNotFoundError: [Errno 2] No such file or directory: "), "{}", got.err);
    assert_eq!(tmp.names(), ["t.json"]);
    for (flags, name) in [(&["--or-empty"][..], "new.json"), (&["--or-empty", "--atomic"][..], "new2.json")] {
        let target = format!("j={}", tmp.path(name));
        let mut cmd = vec!["edit"];
        cmd.extend(flags);
        cmd.extend([target.as_str(), "j['a'] = [1, None]"]);
        let got = run(&cmd);
        assert_eq!((got.code, got.err.as_str()), (0, ""));
        assert_eq!(tmp.read(name), "{\"a\": [1, null]}");
    }
    assert_eq!(tmp.names(), ["new.json", "new2.json", "t.json"]);
}

// ---- ③ 不是 UTF-8 的文件 ----

#[test]
fn files_that_are_not_utf8() {
    let tmp = Tmp::new("utf8");
    // 文件里的字节 → Python 3.14 的 open(p).read() 报的那一句
    let cases: [(&[u8], &str); 15] = [
        (&b"\xff"[..], "'utf-8' codec can't decode byte 0xff in position 0: invalid start byte"),
        (&b"ab\xc0\x80"[..], "'utf-8' codec can't decode byte 0xc0 in position 2: invalid start byte"),
        (&b"abc\x80def"[..], "'utf-8' codec can't decode byte 0x80 in position 3: invalid start byte"),
        (&b"abc\xe4"[..], "'utf-8' codec can't decode byte 0xe4 in position 3: unexpected end of data"),
        (&b"abc\xe4\xb8"[..], "'utf-8' codec can't decode bytes in position 3-4: unexpected end of data"),
        (&b"\xf0\x90\x80"[..], "'utf-8' codec can't decode bytes in position 0-2: unexpected end of data"),
        (&b"\xe4A"[..], "'utf-8' codec can't decode byte 0xe4 in position 0: invalid continuation byte"),
        (&b"a\xe4\xb8A"[..], "'utf-8' codec can't decode bytes in position 1-2: invalid continuation byte"),
        (&b"\xed\xa0\x80"[..], "'utf-8' codec can't decode byte 0xed in position 0: invalid continuation byte"),
        (&b"\xf4\x90\x80\x80"[..], "'utf-8' codec can't decode byte 0xf4 in position 0: invalid continuation byte"),
        (&b"ok\n\xf0\x90\x80A"[..], "'utf-8' codec can't decode bytes in position 3-5: invalid continuation byte"),
        (&b"caf\xe9\n"[..], "'utf-8' codec can't decode byte 0xe9 in position 3: invalid continuation byte"),
        (&b"\xc2"[..], "'utf-8' codec can't decode byte 0xc2 in position 0: unexpected end of data"),
        (&b"x\xf5\x80\x80\x80"[..], "'utf-8' codec can't decode byte 0xf5 in position 1: invalid start byte"),
        (&b"\xe0\x80\x80"[..], "'utf-8' codec can't decode byte 0xe0 in position 0: invalid continuation byte"),
    ];
    for (bytes, want) in cases {
        let file = tmp.write("f", bytes);
        let want = format!("UnicodeDecodeError: {want}\n");
        // 绑定（-f、-l、-t）和 open().read()、json.load(open()) 都是一次解完整个文件
        let whole: [&[&str]; 5] = [
            &["val", "-f", "x=@F@", "x"],
            &["val", "-l", "x=@F@", "x"],
            &["val", "-t", "x=@F@", "x"],
            &["val", "-s", "x=@F@", "open(x).read()"],
            &["check", "-s", "x=@F@", "json.load(open(x))"],
        ];
        for cmd in whole {
            let cmd: Vec<String> = cmd.iter().map(|a| a.replace("@F@", &file)).collect();
            let got = run(&cmd);
            assert_eq!((got.code, got.out.as_str(), got.err.as_str()), (1, "", want.as_str()), "{bytes:?} {cmd:?}");
        }
        // 一行一行读：Python 一块一块地解码。错在第一块里、不是“到头了还没完”的，和上面一样；
        // 别的情况它先交出前面的几行才报错、位置也是相对那一块的——这里不照着做，退出码 2
        let got = run(&["val", "-s", &format!("x={file}"), "[line for line in open(x)]"]);
        if want.contains("unexpected end of data") {
            assert_eq!(got.code, 2, "{bytes:?}: {}", got.err);
        } else {
            assert_eq!((got.code, got.err.as_str()), (1, want.as_str()), "{bytes:?}");
        }
    }
    // 开头的 BOM：读成文本时留着（Python 的 utf-8 不去掉它），当 JSON 读时 Python 单独有一句
    let bom = tmp.write("bom.json", b"\xef\xbb\xbf{\"a\": 1}");
    let got = run(&["val", "-t", &format!("x={bom}"), r#"len(x), x[0] == "\ufeff""#]);
    assert_eq!((got.code, got.out.as_str()), (0, "(9, True)\n"));
    let got = run(&["val", "-f", &format!("x={bom}"), "x"]);
    assert_eq!(got.err, "json.decoder.JSONDecodeError: Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)\n");
    // 文本模式：\r\n、单独的 \r 都读成 \n
    let crlf = tmp.write("crlf.txt", b"a\r\nb\rc\n");
    let got = run(&["val", "-t", &format!("x={crlf}"), "-s", &format!("p={crlf}"), "x == 'a\\nb\\nc\\n', open(p).readlines()"]);
    assert_eq!(got.out, "(True, ['a\\n', 'b\\n', 'c\\n'])\n");
}
