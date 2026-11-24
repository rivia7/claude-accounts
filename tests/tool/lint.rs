//! 回归里的几个检查（`cca-test lint 名字 参数…`）：原来是 regress.sh 里的三段 Python，这里照原样重写——查出来的、打印的那一行、
//! 退出码都一样（regress.sh 拿打印的那一行当结果：是 ok 才算过，别的接在用例的说明后面）。
//!
//! ```text
//! prompts cca脚本 PROTOCOL.md    提问只能经提问的基元：cca 里读 fd 3（写着 <&3）的行，除了注释、“: <&3” 这种只是接上的，都得在
//!                                ask、ask_secret、arrow_menu、gui_ask、gui_answer、gui_answer_wait、cmd_edit、node_read_block 这几个函数里
//!                                （一行开头的 “名字() {” 起算到下一个这样的行）；cca 里的题号（q=a.b 和 ask_fields a.b 后面跟空格的）
//!                                都要在 PROTOCOL.md 里写成 `a.b`。都对（而且找得到题号）：打出 ok；不然打出
//!                                “：读 fd 3 的 [函数名…]；PROTOCOL 里没写的题号 [题号…]”（两个列表是 Python 打印列表的样子，题号排过序）
//! ifaces cca脚本 bind.rs         虚拟网卡的名单两边是同一份：cca 里一整行的 VIRTUAL_IFACE_RE='…' 和 bind.rs 里
//!                                pub const VIRTUAL_IFACE_RE: &str = r"…";（等号后面可以折行）引号里一样就打出 ok；不然打出
//!                                “：bash 的 …，辅助程序的 …”（哪边没找到，哪边写 None）
//! dollar 仓库 文件或目录…        $变量 后面紧跟非 ASCII 字符的行（macOS 的 bash 3.2 在 UTF-8 下会把多字节字符的首字节吞进变量名）。后面的
//!                                每一个参数接在 仓库 后面（本身是绝对路径的就是它自己），最后一段可以带 * 和 ?（不配 . 开头的名字）；
//!                                是目录的话查它下面所有 .sh、.swift（不跟着链接进别的目录）。没有：打出 ok；有：打出
//!                                “：文件:行号、文件:行号…”（文件名是相对 仓库 的，先后照参数、同一个参数里照路径排）。一个不带 * 的
//!                                参数什么都没找到：打出 “：没有 那个参数”，不再往下查
//! nopython 仓库                  仓库里没有 Python 的程序了（辅助程序、服务端、测试都是 Rust）：仓库 下面名字以 .py、.pyc、.pyi、.pyw 结尾的
//!                                文件、第一行是 #!…python… 的文件都算。不进 .git 这类名字以 . 开头的目录、target、build、
//!                                node_modules、__pycache__（编出来的、别人的、以前留下的缓存），不跟着链接进别的目录。没有：打出 ok；有：打出
//!                                “：文件、文件…”（相对 仓库 的路径，排过序）
//! ```
//!
//! 退出码（和 fx.rs 一样，收尾用的就是它的 dispatch）：0 是查完了（查出毛病也是 0，看打印的那一行）；1 是原来的 Python 也会出的
//! 异常（少了参数、文件读不了、不是 UTF-8）；2 是这里不敢担保的（见下），stderr 上以 “cca-test：” 开头，另记进 $CCA_TEST_TRAP。
//!
//! 不敢担保的：prompts 里 q= 前面紧挨着上标、分数、罗马数字这类字符（Python 的 \b 把它们大多算作词的一部分，要查 Unicode 的
//! 另一张表）；dollar 的通配符出现在最后一段以外的地方（仓库的路径里有 * ? [ 也算：Python 的 glob 会把它们当通配符）、用了
//! [ ]、目录里有不是 UTF-8 的文件名。
//!
//! Rust 的 regex 没有“往后看一眼”：dollar 原来的 \$[A-Za-z_][A-Za-z0-9_]*(?=[^\x00-\x7f]) 在这里是手写的一小段（dollar_hit）；
//! prompts 的 \b 也是手写的（Python 和 regex 对“哪些字符算词的一部分”说法不一样）。

use crate::fake::{self, is_py_space, repr_str};
use crate::fx::{arg, dispatch, raise, read_text, unsupported, Sub, R};

pub const CMDS: &[(&str, crate::Run)] = &[("lint", run)];

const SUBS: &[(&str, Sub)] = &[("prompts", prompts), ("ifaces", ifaces), ("dollar", dollar), ("nopython", nopython)];

fn run(args: &[String]) -> i32 {
    dispatch("lint", SUBS, args)
}

fn rx(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).expect("写死的正则")
}

/// '%s' % 一个字符串的列表：['a', 'b']
fn py_list(items: &[&str]) -> String {
    format!("[{}]", items.iter().map(|s| repr_str(s)).collect::<Vec<_>>().join(", "))
}

// ---- prompts ----

/// 能直接读 fd 3 的函数：提问的基元
const ASKERS: [&str; 8] =
    ["ask", "ask_secret", "arrow_menu", "gui_ask", "gui_answer", "gui_answer_wait", "cmd_edit", "node_read_block"];

/// Python 的 \b 眼里 c 算不算词的一部分（字母、数字、下划线：str.isalnum() 或者 _）
fn py_word(c: char) -> R<bool> {
    if c.is_ascii() {
        return Ok(c.is_ascii_alphanumeric() || c == '_');
    }
    let s = c.to_string();
    if rx(r"^[\p{L}\p{Nd}]$").is_match(&s) {
        return Ok(true);
    }
    if rx(r"^[\p{Nl}\p{No}]$").is_match(&s) {
        return unsupported(format!("q= 前面紧挨着 {c}（U+{:04X}）：这类字符算不算词的一部分，这里说不准", c as u32));
    }
    Ok(false)
}

/// re.findall(r'\bq=([a-z_]+\.[a-z_.]+)', src)：一个个 “q=” 看过去——前面不是词的一部分，后面是 小写字母和下划线、一个点、
/// 小写字母 / 下划线 / 点（都尽量长）。配上的那一段里不再找（findall 找的是不重叠的）
fn q_ids(src: &str) -> R<Vec<&str>> {
    let b = src.as_bytes();
    let head = |c: u8| c.is_ascii_lowercase() || c == b'_';
    let mut found = Vec::new();
    let mut i = 0;
    while let Some(off) = src[i..].find("q=") {
        let at = i + off;
        i = at + 1;
        if let Some(prev) = src[..at].chars().next_back() {
            if py_word(prev)? {
                continue;
            }
        }
        let start = at + 2;
        let mut j = start;
        while j < b.len() && head(b[j]) {
            j += 1;
        }
        // 开头那一段不能是空的，紧接着得是点，点后面至少还有一个
        if j == start || b.get(j) != Some(&b'.') || !b.get(j + 1).is_some_and(|c| head(*c) || *c == b'.') {
            continue;
        }
        j += 1;
        while j < b.len() && (head(b[j]) || b[j] == b'.') {
            j += 1;
        }
        found.push(&src[start..j]);
        i = j;
    }
    Ok(found)
}

fn prompts(a: &[String], out: &mut String) -> R<i32> {
    let src = read_text(arg(a, 0)?)?;
    let doc = read_text(arg(a, 1)?)?;
    let def = rx(r"^([A-Za-z_][A-Za-z0-9_]*)\(\) \{");
    let (mut bad, mut func) = (Vec::new(), "");
    for line in src.split('\n') {
        if let Some(m) = def.captures(line) {
            func = m.get(1).map_or("", |g| g.as_str());
        }
        if line.contains("<&3")
            && !line.contains(": <&3")
            && !line.trim_start_matches(is_py_space).starts_with('#')
            && !ASKERS.contains(&func)
        {
            bad.push(func);
        }
    }
    let mut qs = q_ids(&src)?;
    qs.extend(rx(r"ask_fields ([a-z]+\.[a-z_.]+) ").captures_iter(&src).filter_map(|m| m.get(1)).map(|g| g.as_str()));
    qs.sort_unstable();
    qs.dedup();
    let missing: Vec<&str> = qs.iter().copied().filter(|q| !doc.contains(&format!("`{q}`"))).collect();
    if bad.is_empty() && missing.is_empty() && !qs.is_empty() {
        out.push_str("ok\n");
    } else {
        out.push_str(&format!("：读 fd 3 的 {}；PROTOCOL 里没写的题号 {}\n", py_list(&bad), py_list(&missing)));
    }
    Ok(0)
}

// ---- ifaces ----

fn ifaces(a: &[String], out: &mut String) -> R<i32> {
    let sh = read_text(arg(a, 0)?)?;
    let rs = read_text(arg(a, 1)?)?;
    let first = |pattern: &str, text: &str| rx(pattern).captures(text).and_then(|m| m.get(1)).map(|g| g.as_str().to_string());
    let b = first(r"(?m)^VIRTUAL_IFACE_RE='([^']*)'$", &sh);
    // 等号后面可以折行（rustfmt 排的：这一行超过行宽）。Python 的 \s 比 regex 的多认四个分隔控制符
    let p = first(r#"(?m)^pub const VIRTUAL_IFACE_RE: &str =[\s\x1c-\x1f]*r"([^"]*)";$"#, &rs);
    if b.is_some() && b == p {
        out.push_str("ok\n");
    } else {
        let shown = |v: &Option<String>| v.clone().unwrap_or_else(|| "None".to_string());
        out.push_str(&format!("：bash 的 {}，辅助程序的 {}\n", shown(&b), shown(&p)));
    }
    Ok(0)
}

// ---- dollar ----

/// 这一行里有没有 $变量 后面紧跟非 ASCII 字符。文件是 UTF-8 的：变量名（ASCII）后面那个字节在 0x80 以上，就是一个非 ASCII 字符的开头
fn dollar_hit(line: &[u8]) -> bool {
    let mut i = 0;
    while let Some(off) = line[i..].iter().position(|c| *c == b'$') {
        let mut j = i + off + 1;
        if line.get(j).is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_') {
            j += 1;
            while line.get(j).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') {
                j += 1;
            }
            if line.get(j).is_some_and(|c| *c >= 0x80) {
                return true;
            }
        }
        i += off + 1;
    }
    false
}

fn has_magic(s: &str) -> bool {
    s.contains(['*', '?', '['])
}

/// fnmatch：* 是随便多少个字符，? 是一个字符（[ ] 这里不认，调用的地方先挡掉了）
fn wild(pattern: &[char], name: &[char]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some(('*', rest)) => (0..=name.len()).any(|k| wild(rest, &name[k..])),
        Some(('?', rest)) => !name.is_empty() && wild(rest, &name[1..]),
        Some((c, rest)) => name.first() == Some(c) && wild(rest, &name[1..]),
    }
}

/// 一个目录里的名字：(名字, 顺着链接看是不是目录, 自己是不是链接)。读不了是 None（Python 的 glob、os.walk 都当它是空的）
fn names_in(dir: &str) -> R<Option<Vec<(String, bool, bool)>>> {
    let Ok(entries) = std::fs::read_dir(if dir.is_empty() { "." } else { dir }) else { return Ok(None) };
    let mut names = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { return Ok(None) };
        let Ok(name) = entry.file_name().into_string() else {
            return unsupported(format!("{dir} 里有不是 UTF-8 的文件名"));
        };
        let is_dir = std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir());
        let is_link = entry.file_type().is_ok_and(|t| t.is_symlink());
        names.push((name, is_dir, is_link));
    }
    Ok(Some(names))
}

/// sorted(glob.glob(pathname))
fn glob(pathname: &str) -> R<Vec<String>> {
    let (dir, base) = match pathname.rfind('/') {
        Some(i) => (&pathname[..=i], &pathname[i + 1..]),
        None => ("", pathname),
    };
    if !has_magic(pathname) {
        // 不带通配符：在就是它（链接本身在就算；以 / 结尾的得是目录）
        let there = if base.is_empty() { std::fs::metadata(pathname).is_ok_and(|m| m.is_dir()) } else { fake::lexists(pathname) };
        return Ok(if there { vec![pathname.to_string()] } else { Vec::new() });
    }
    if has_magic(dir) || base.contains('[') {
        return unsupported(format!("通配符只认最后一段里的 * 和 ?：{pathname}"));
    }
    let pattern: Vec<char> = base.chars().collect();
    // os.path.split 给的目录名不带结尾的 /（全是 / 的除外），拼回去时再补上
    let head = fake::dirname(pathname);
    let mut found = Vec::new();
    for (name, _, _) in names_in(&head)?.unwrap_or_default() {
        // * 和 ? 不配 . 开头的名字，除非模式自己以 . 开头
        if (name.starts_with('.') && !base.starts_with('.')) || !wild(&pattern, &name.chars().collect::<Vec<_>>()) {
            continue;
        }
        found.push(fake::join(&[&head, &name]));
    }
    found.sort_unstable();
    Ok(found)
}

/// sorted(os.path.join(r, f) for r, _, fs in os.walk(top) for f in fs if f.endswith(('.sh', '.swift')))
fn scripts_under(top: &str) -> R<Vec<String>> {
    let mut found = Vec::new();
    let mut stack = vec![top.to_string()];
    while let Some(dir) = stack.pop() {
        let Some(names) = names_in(&dir)? else { continue };
        for (name, is_dir, is_link) in names {
            let path = fake::join(&[&dir, &name]);
            if !is_dir {
                if name.ends_with(".sh") || name.ends_with(".swift") {
                    found.push(path);
                }
            } else if !is_link {
                stack.push(path);
            }
        }
    }
    found.sort_unstable();
    Ok(found)
}

/// os.path.normpath（POSIX）
fn normpath(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    // 开头正好两个 / 的留着两个，别的都算一个
    let root = match path.bytes().take_while(|c| *c == b'/').count() {
        0 => "",
        2 => "//",
        _ => "/",
    };
    let mut comps: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp != ".." || (root.is_empty() && comps.is_empty()) || comps.last() == Some(&"..") {
            comps.push(comp);
        } else {
            comps.pop();
        }
    }
    let out = format!("{root}{}", comps.join("/"));
    if out.is_empty() {
        ".".to_string()
    } else {
        out
    }
}

/// os.path.abspath
fn abspath(path: &str) -> R<String> {
    if path.starts_with('/') {
        return Ok(normpath(path));
    }
    Ok(normpath(&fake::join(&[&fake::getcwd()?, path])))
}

/// os.path.relpath(path, start)
fn relpath(path: &str, start: &str) -> R<String> {
    if path.is_empty() {
        return raise("ValueError: no path specified");
    }
    let (start, path) = (abspath(if start.is_empty() { "." } else { start })?, abspath(path)?);
    let parts = |p: &str| p.trim_start_matches('/').split('/').filter(|c| !c.is_empty()).map(str::to_string).collect::<Vec<_>>();
    let (from, to) = (parts(&start), parts(&path));
    let same = from.iter().zip(&to).take_while(|(x, y)| x == y).count();
    let mut rel: Vec<&str> = vec![".."; from.len() - same];
    rel.extend(to[same..].iter().map(String::as_str));
    Ok(if rel.is_empty() { ".".to_string() } else { rel.join("/") })
}

fn dollar(a: &[String], out: &mut String) -> R<i32> {
    let root = arg(a, 0)?;
    let mut files = Vec::new();
    for pattern in &a[1..] {
        let found = glob(&fake::join(&[root, pattern]))?;
        if found.is_empty() && !pattern.contains('*') {
            out.push_str(&format!("：没有 {pattern}\n"));
            return Ok(0);
        }
        for path in found {
            if std::fs::metadata(&path).is_ok_and(|m| m.is_dir()) {
                files.extend(scripts_under(&path)?);
            } else {
                files.push(path);
            }
        }
    }
    let mut bad = Vec::new();
    for file in &files {
        // 一行一行数：\n、\r\n、单独的 \r 都算换行（read_text 已经统一成 \n）
        for (i, line) in read_text(file)?.split('\n').enumerate() {
            if dollar_hit(line.as_bytes()) {
                bad.push(format!("{}:{}", relpath(file, root)?, i + 1));
            }
        }
    }
    if bad.is_empty() {
        out.push_str("ok\n");
    } else {
        out.push_str(&format!("：{}\n", bad.join("、")));
    }
    Ok(0)
}

// ---- nopython ----

/// 不进去的目录：编出来的（cli/target、macos/build）、别人的（node_modules）、以前跑 Python 的测试留在检出里的缓存
/// （__pycache__：不归 git 管，留着也不算仓库里有 Python），名字以 . 开头的另算（.git、.claude、.build）
const NOT_OURS: [&str; 4] = ["target", "build", "node_modules", "__pycache__"];

/// 名字说它是 Python 的
fn python_name(name: &str) -> bool {
    [".py", ".pyc", ".pyi", ".pyw"].iter().any(|ext| name.ends_with(ext))
}

/// 第一行是 #!…python…（只读开头的一小段：不把大文件整个读进来；只看普通文件：管道、套接字一打开就卡住）
fn python_shebang(path: &str) -> bool {
    use std::io::Read;
    let mut head = [0u8; 256];
    if !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file()) {
        return false;
    }
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let n = f.read(&mut head).unwrap_or(0);
    let line = head[..n].split(|b| *b == b'\n').next().unwrap_or_default();
    line.starts_with(b"#!") && line.windows(6).any(|w| w == b"python")
}

fn nopython(a: &[String], out: &mut String) -> R<i32> {
    let root = arg(a, 0)?;
    if !std::fs::metadata(root).is_ok_and(|m| m.is_dir()) {
        return raise(format!("NotADirectoryError: [Errno 20] Not a directory: {}", repr_str(root)));
    }
    let mut bad = Vec::new();
    let mut stack = vec![root.to_string()];
    while let Some(dir) = stack.pop() {
        let Some(names) = names_in(&dir)? else { continue };
        for (name, is_dir, is_link) in names {
            let path = fake::join(&[&dir, &name]);
            if is_dir {
                if !is_link && !name.starts_with('.') && !NOT_OURS.contains(&name.as_str()) {
                    stack.push(path);
                }
            } else if python_name(&name) || python_shebang(&path) {
                bad.push(relpath(&path, root)?);
            }
        }
    }
    bad.sort_unstable();
    if bad.is_empty() {
        out.push_str("ok\n");
    } else {
        out.push_str(&format!("：{}\n", bad.join("、")));
    }
    Ok(0)
}

// ---- 测试：prompts、ifaces、dollar 期望的值都是原来那三段 Python 在同样的输入上打印的（生成它们的脚本不在仓库里） ----

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::Stop;
    use kit::fsx::{self, TempDir};

    fn call(sub: Sub, args: &[&str]) -> (R<i32>, String) {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let mut out = String::new();
        let r = sub(&args, &mut out);
        (r, out)
    }

    /// 查完了（退出码 0）：打印的那一行
    fn said(sub: Sub, args: &[&str]) -> String {
        let (r, out) = call(sub, args);
        assert!(matches!(r, Ok(0)), "{args:?}：{r:?}");
        out
    }

    fn raised(sub: Sub, args: &[&str]) -> String {
        match call(sub, args) {
            (Err(Stop::Raise(msg)), out) if out.is_empty() => msg,
            other => panic!("{args:?}：{other:?}"),
        }
    }

    fn refused(sub: Sub, args: &[&str]) {
        let (r, out) = call(sub, args);
        assert!(matches!(r, Err(Stop::Unsupported(_))) && out.is_empty(), "{args:?}：{r:?}");
    }

    fn at(dir: &TempDir, name: &str) -> String {
        dir.join(name).to_str().expect("临时目录的路径是 UTF-8").to_string()
    }

    #[test]
    fn prompts_finds_stray_readers_and_undocumented_ids() {
        let dir = TempDir::new("lint-prompts-");
        let (src, doc) = (at(&dir, "cca"), at(&dir, "PROTOCOL.md"));
        let cases: &[(&str, &str, &str)] = &[
            (
                "ask() {\n\tread -r x <&3\n}\nreader() {\n\t: <&3\n\t# read <&3\n\tread -r y <&3\n}\nmenu() { ask q=team.join \"?\"; ask_fields node.add x; xq=no.match; 中q=cjk.no; .q=dot.yes; }\n",
                "see `team.join` and `dot.yes`\n",
                "：读 fd 3 的 ['reader']；PROTOCOL 里没写的题号 ['node.add']\n",
            ),
            (
                "ask() {\n\tread -r x <&3\n}\ngui_ask() {\n\t: <&3\n\t# read <&3\n\tread -r y <&3\n}\nmenu() { ask q=team.join \"?\"; ask_fields node.add x; xq=no.match; 中q=cjk.no; .q=dot.yes; }\n",
                r#"`team.join` `node.add` `dot.yes`"#,
                "ok\n",
            ),
            (
                "ask() {\n\tread x <&3\n}\n",
                r#""#,
                "：读 fd 3 的 []；PROTOCOL 里没写的题号 []\n",
            ),
            (
                "top <&3\n",
                r#""#,
                "：读 fd 3 的 ['']；PROTOCOL 里没写的题号 []\n",
            ),
        ];
        for (text, documented, want) in cases {
            fsx::write(&src, text);
            fsx::write(&doc, documented);
            assert_eq!(said(prompts, &[&src, &doc]), *want, "{text}");
        }
        // q= 前面紧挨着上标、罗马数字：Python 把它们算作词的一部分（所以不是题号），这里说不准
        for before in ['²', 'Ⅷ'] {
            fsx::write(&src, format!("ask {before}q=team.join x\n"));
            refused(prompts, &[&src, &doc]);
        }
        assert!(raised(prompts, &[&src, &at(&dir, "nope.md")]).starts_with("FileNotFoundError: "));
        assert_eq!(raised(prompts, &[&src]), "IndexError: list index out of range");
    }

    #[test]
    fn q_ids_follow_pythons_word_boundary() {
        // 前面是字母、数字、下划线（含汉字）的不算；点、引号、空白、组合记号后面的算；配上的那一段里不再找
        let text = "xq=w.one _q=w.two 9q=w.three .q=d.four \"q=d.five\" 中q=cjk.six e\u{301}q=mark.seven -q=a.b.q=c.d q=q=g.h q=nodot q=.lead q=a. q=a.B q=a..b q=_._";
        assert_eq!(q_ids(text).unwrap(), ["d.four", "d.five", "mark.seven", "a.b.q", "g.h", "a..b", "_._"]);
    }

    #[test]
    fn ifaces_compares_the_two_lists() {
        let dir = TempDir::new("lint-ifaces-");
        let (sh, rs) = (at(&dir, "cca"), at(&dir, "bind.rs"));
        let cases: &[(&str, &str, &str)] = &[
            ("x=1\nVIRTUAL_IFACE_RE='utun|tap'\n", "pub const VIRTUAL_IFACE_RE: &str =\n    r\"utun|tap\";\n", "ok\n"),
            (
                "VIRTUAL_IFACE_RE='utun|tap'\n",
                "pub const VIRTUAL_IFACE_RE: &str = r\"utun\";\n",
                "：bash 的 utun|tap，辅助程序的 utun\n",
            ),
            ("IFACE='utun'\n", "pub const VIRTUAL_IFACE_RE: &str = r\"utun\";\n", "：bash 的 None，辅助程序的 utun\n"),
            ("VIRTUAL_IFACE_RE='utun' \n", "// none\n", "：bash 的 None，辅助程序的 None\n"),
            ("VIRTUAL_IFACE_RE=''\n", "pub const VIRTUAL_IFACE_RE: &str = \u{1c}r\"\";\n", "ok\n"),
        ];
        for (bash, rust, want) in cases {
            fsx::write(&sh, bash);
            fsx::write(&rs, rust);
            assert_eq!(said(ifaces, &[&sh, &rs]), *want, "{bash:?} {rust:?}");
        }
        fsx::write(&sh, b"VIRTUAL_IFACE_RE='a'\n\xff\n");
        assert!(raised(ifaces, &[&sh, &rs]).starts_with("UnicodeDecodeError: "));
    }

    #[test]
    fn dollar_lists_the_lines_where_a_variable_meets_non_ascii() {
        let dir = TempDir::new("lint-dollar-");
        for (name, text) in [
            (r#"cca"#, "a\necho \"$x中\"\n"),
            (r#"install.sh"#, "echo \"${x}中 $1中 $ 中\"\n"),
            (r#"macos/build.sh"#, "echo \"$x中\"\n"),
            (r#"macos/.hidden.sh"#, "echo \"$x中\"\n"),
            (r#"macos/Sources/Kit/A.swift"#, "let a = 1\r\nlet b = \"$name的\"\recho \"$x中\"\n"),
            (r#"macos/Sources/notes.txt"#, "echo \"$x中\"\n"),
            (r#"macos/tools/sub/u.sh"#, "\n\necho \"$x中\"\n"),
            (r#"macos/tools/t.sh"#, "ok\n"),
        ] {
            fsx::write(dir.join("repo").join(name), text);
        }
        std::fs::create_dir(dir.join("repo/tests")).unwrap();
        let root = at(&dir, "repo");
        let cases: &[(&[&str], &str)] = &[
            (
                &[r#"@/tests/.."#, r#"@/cca"#, r#"install.sh"#, r#"macos/*.sh"#, r#"macos/Sources"#, r#"macos/tools"#],
                "：cca:2、macos/build.sh:1、macos/Sources/Kit/A.swift:2、macos/Sources/Kit/A.swift:3、macos/tools/sub/u.sh:3\n",
            ),
            (&[r#"@"#, r#"install.sh"#, r#"macos/tools/t.sh"#, r#"macos/*.zip"#], "ok\n"),
            (&[r#"@"#, r#"install.sh"#, r#"nope.sh"#, r#"cca"#], "：没有 nope.sh\n"),
            (&[r#"@/macos"#, r#"@/cca"#, r#"tools/"#], "：../cca:2、tools/sub/u.sh:3\n"),
            (&[r#"@"#], "ok\n"),
        ];
        for (args, want) in cases {
            let args: Vec<String> = args.iter().map(|a| a.replace('@', &root)).collect();
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            assert_eq!(said(dollar, &args), *want, "{args:?}");
        }
        assert_eq!(raised(dollar, &[]), "IndexError: list index out of range");
        // 通配符在别的地方、[ ]：这里不担保
        refused(dollar, &[&root, "mac*/build.sh"]);
        refused(dollar, &[&root, "macos/[bt]*.sh"]);
        // 不是 UTF-8 的文件：和 Python 一样是异常（什么都不打）；断了的链接也是
        fsx::write(dir.join("repo/macos/tools/bad.sh"), b"echo \xff\n");
        assert!(raised(dollar, &[&root, "macos/tools"]).starts_with("UnicodeDecodeError: "));
        std::fs::remove_file(dir.join("repo/macos/tools/bad.sh")).unwrap();
        std::os::unix::fs::symlink("nowhere", dir.join("repo/macos/tools/gone.sh")).unwrap();
        assert!(raised(dollar, &[&root, "macos/tools"]).starts_with("FileNotFoundError: "));
    }

    #[test]
    fn nopython_lists_python_files_and_shebangs() {
        let dir = TempDir::new("lint-nopython-");
        for (name, text) in [
            ("repo/cca", "#!/bin/bash\n# python3 以前在这里\npython3 -c pass\n"),
            ("repo/cli/src/py/json.rs", "// 像 Python 的 json\n"),
            ("repo/cli/target/debug/build/x.py", "print(1)\n"),
            ("repo/macos/build/gen.py", "print(1)\n"),
            ("repo/server/web/node_modules/a/setup.py", "print(1)\n"),
            ("repo/.git/hooks/pre-commit", "#!/usr/bin/env python3\n"),
            ("repo/.claude/worktrees/w/old.py", "print(1)\n"),
            ("repo/tests/__pycache__/spec_wire.cpython-314.pyc", "\0\0"),
            ("repo/docs/notes.pyx", "不算\n"),
            ("repo/empty", ""),
        ] {
            fsx::write(dir.join(name), text);
        }
        let root = at(&dir, "repo");
        assert_eq!(said(nopython, &[&root]), "ok\n");
        // 名字是 Python 的、第一行的 shebang 是 python 的（第二行起提到它不算）；排过序，路径相对仓库
        for (name, text) in [
            ("repo/server/cca-presence.py", "#!/usr/bin/env python3\n"),
            ("repo/tests/a.pyc", "\0\0"),
            ("repo/tests/stubs.pyi", ""),
            ("repo/tools/gen", "#!/usr/bin/env python3\nprint(1)\n"),
            ("repo/tools/gen2", "#! /usr/bin/python\n"),
            ("repo/tools/sh", "#!/bin/sh\nexec python3 x\n"),
            ("outside/far.py", "print(1)\n"),
            ("outside/tool", "#!/usr/bin/python3\n"),
        ] {
            fsx::write(dir.join(name), text);
        }
        // 链接：名字是 .py 的算（不管指着什么），指着别处的 Python 程序的不去读，指着目录的不进去
        std::os::unix::fs::symlink("../cca", dir.join("repo/tools/link.py")).unwrap();
        std::os::unix::fs::symlink(dir.join("outside/tool"), dir.join("repo/tools/tool")).unwrap();
        std::os::unix::fs::symlink(dir.join("outside"), dir.join("repo/vendor")).unwrap();
        assert_eq!(
            said(nopython, &[&root]),
            "：server/cca-presence.py、tests/a.pyc、tests/stubs.pyi、tools/gen、tools/gen2、tools/link.py\n"
        );
        assert_eq!(raised(nopython, &[]), "IndexError: list index out of range");
        assert!(raised(nopython, &[&at(&dir, "repo/cca")]).starts_with("NotADirectoryError: "));
    }

    #[test]
    fn dollar_hit_wants_a_name_then_a_non_ascii_byte() {
        for (line, want) in [
            ("echo \"$x中\"", true),
            ("echo \"$_中\"", true),
            ("echo \"$x_1é\"", true),
            ("echo \"$x$y中\"", true),
            ("echo \"$$x中\"", true),
            ("echo \"${x}中\"", false),
            ("echo \"$x 中\"", false),
            ("echo \"$1中\"", false),
            ("echo \"$中\"", false),
            ("echo \"$x\"", false),
            ("中文 $", false),
        ] {
            assert_eq!(dollar_hit(line.as_bytes()), want, "{line}");
        }
    }

    #[test]
    fn wildcards_match_like_fnmatch() {
        let m = |p: &str, n: &str| wild(&p.chars().collect::<Vec<_>>(), &n.chars().collect::<Vec<_>>());
        assert!(
            m("*.sh", "build.sh")
                && m("*.sh", ".sh")
                && m("*", "")
                && m("b*d.s*", "build.sh")
                && m("?uild.sh", "build.sh")
                && m("**.sh", "a.sh")
                && m("中?", "中文")
        );
        assert!(!m("*.sh", "build.swift") && !m("?", "") && !m("?", "ab") && !m("a*", "ba") && !m("*.sh", "a.SH"));
    }

    #[test]
    fn paths_normalize_like_os_path() {
        for (path, want) in [
            (r#""#, r#"."#),
            (r#"."#, r#"."#),
            (r#"/"#, r#"/"#),
            (r#"//"#, r#"//"#),
            (r#"///"#, r#"/"#),
            (r#"a/b/../c"#, r#"a/c"#),
            (r#"../a"#, r#"../a"#),
            (r#"/.."#, r#"/"#),
            (r#"a/./b//c/"#, r#"a/b/c"#),
            (r#"/a/../../b"#, r#"/b"#),
            (r#"a/.."#, r#"."#),
            (r#"//a/.."#, r#"//"#),
            (r#"../../a/../b"#, r#"../../b"#),
            (r#"/a/b/"#, r#"/a/b"#),
        ] {
            assert_eq!(normpath(path), want, "{path:?}");
        }
        for (path, start, want) in [
            (r#"/a/b/c"#, r#"/a"#, r#"b/c"#),
            (r#"/a"#, r#"/a/b/c"#, r#"../.."#),
            (r#"/a/b"#, r#"/a/b"#, r#"."#),
            (r#"/x/y"#, r#"/a/b"#, r#"../../x/y"#),
            (r#"/a/b/../c"#, r#"/a/tests/.."#, r#"c"#),
            (r#"/a/b"#, r#"/"#, r#"a/b"#),
            (r#"//a/b"#, r#"/a"#, r#"b"#),
        ] {
            assert_eq!(relpath(path, start).unwrap(), want, "{path:?} {start:?}");
        }
    }
}
