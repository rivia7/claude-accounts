//! 文件和 JSON：读文件（照 Python 的文本模式）、JSON 和这里的值互相转。JSON 的读写都交给 kit::json。

use std::cell::RefCell;
use std::io::Read;
use std::rc::Rc;

use kit::json::{Obj, Style, Value};

use super::ast::CmpOp;
use super::builtins::{no_kw, take_kw};
use super::interp::Kw;
use super::ops::{self, as_int, truthy, MAX_DATA_DEPTH};
use super::value::*;

// ---- 文件 ----

/// 读文件时系统的出错 → Python 的异常（FileNotFoundError: [Errno 2] No such file or directory: '路径'）
pub fn io_error(e: &std::io::Error, path: &str) -> Stop {
    let Exc { kind, msg } = os_error(e);
    Stop::Raise(Exc { kind, msg: format!("{msg}: {}", kit::py::repr_str(path)) })
}

/// 系统的出错 → Python 的异常是哪一类、“[Errno 2] No such file or directory” 这一段
pub fn os_error(e: &std::io::Error) -> Exc {
    use std::io::ErrorKind;
    let kind = match e.kind() {
        ErrorKind::NotFound => Kind::FileNotFound,
        ErrorKind::PermissionDenied => Kind::Permission,
        ErrorKind::IsADirectory => Kind::IsADirectory,
        ErrorKind::NotADirectory => Kind::NotADirectory,
        ErrorKind::BrokenPipe => Kind::BrokenPipe,
        _ => Kind::Os,
    };
    let text = e.to_string();
    let msg = match e.raw_os_error() {
        // Rust 写的是 “No such file or directory (os error 2)”，Python 写的是 “[Errno 2] No such file or directory”
        Some(code) => format!("[Errno {code}] {}", text.strip_suffix(&format!(" (os error {code})")).unwrap_or(&text)),
        None => text,
    };
    Exc { kind, msg }
}

/// 字节 → 字符串：不是 UTF-8 的是 UnicodeDecodeError（说法和 Python 把整个文件一次解完时一样）
pub fn decode(bytes: Vec<u8>) -> R<String> {
    String::from_utf8(bytes).map_err(|e| utf8_error(e.as_bytes(), &e.utf8_error()))
}

/// 哪几个字节、错在哪：CPython 分三种——不能当开头的字节、后面跟的不是接续的字节、到头了还没完
fn utf8_error(bytes: &[u8], e: &std::str::Utf8Error) -> Stop {
    let start = e.valid_up_to();
    let lead = bytes.get(start).copied().unwrap_or(0);
    let (end, why) = match e.error_len() {
        None => (bytes.len(), "unexpected end of data"),
        Some(_) if (0x80..0xc2).contains(&lead) || lead >= 0xf5 => (start + 1, "invalid start byte"),
        Some(n) => (start + n, "invalid continuation byte"),
    };
    let msg = if end == start + 1 {
        format!("'utf-8' codec can't decode byte 0x{lead:02x} in position {start}: {why}")
    } else {
        format!("'utf-8' codec can't decode bytes in position {start}-{}: {why}", end - 1)
    };
    Stop::Raise(Exc { kind: Kind::UnicodeDecode, msg })
}

/// Python 一行一行读文件时是这么大一块一块地解码的
const CHUNK: usize = 8192;

/// Python 的文本模式读进来时把 \r\n、\r 都换成 \n
pub fn universal_newlines(text: String) -> String {
    if !text.contains('\r') {
        return text;
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

pub fn read_bytes(path: &str) -> R<Vec<u8>> {
    std::fs::read(path).map_err(|e| io_error(&e, path))
}

/// open(path)：打不开的这时就报错
pub fn open_file(path: &str) -> R<V> {
    let file = std::fs::File::open(path).map_err(|e| io_error(&e, path))?;
    if file.metadata().map(|m| m.is_dir()).unwrap_or(false) {
        return Err(io_error(&std::io::Error::from_raw_os_error(libc::EISDIR), path));
    }
    Ok(V::File(Rc::new(FileObj { path: path.to_string(), state: RefCell::new(FileState::Open(file)) })))
}

/// 文件里还没读的部分：第一次用到时整个读进来。whole：这一次是 read()（一次解完整个文件）还是一行一行读。
/// 不是 UTF-8 的文件一行一行读时，Python 一块一块地解：错在第一块里（而且不是到头了还没完）时和一次解完一样，
/// 在第一次读的时候报、位置也一样；别的情况它会先交出前面的几行、报的位置是相对那一块的——这里不照着做，退出码 2。
fn loaded(f: &FileObj, whole: bool) -> R<(Rc<str>, usize)> {
    let mut state = f.state.borrow_mut();
    if let FileState::Open(file) = &mut *state {
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|e| io_error(&e, &f.path))?;
        if !whole && std::str::from_utf8(&bytes).is_err() {
            let first = &bytes[..bytes.len().min(CHUNK)];
            return match std::str::from_utf8(first) {
                Err(e) if e.error_len().is_some() => Err(utf8_error(first, &e)),
                _ => unsupported("一行一行读一个不是 UTF-8 的文件（Python 一块一块地解码，先交出几行才报错）"),
            };
        }
        *state = FileState::Text(Rc::from(universal_newlines(decode(bytes)?)), 0);
    }
    match &*state {
        FileState::Text(text, pos) => Ok((text.clone(), *pos)),
        _ => value_err("I/O operation on closed file."),
    }
}

fn seek(f: &FileObj, pos: usize) {
    if let FileState::Text(_, at) = &mut *f.state.borrow_mut() {
        *at = pos;
    }
}

/// f.read()：剩下的全部
pub fn file_read(f: &FileObj) -> R<Rc<str>> {
    let (text, pos) = loaded(f, true)?;
    seek(f, text.len());
    Ok(if pos == 0 { text } else { Rc::from(&text[pos..]) })
}

/// 下一行（带着结尾的换行）；读完了是 None
pub fn file_line(f: &FileObj) -> R<Option<V>> {
    let (text, pos) = loaded(f, false)?;
    if pos >= text.len() {
        return Ok(None);
    }
    let end = text[pos..].find('\n').map_or(text.len(), |i| pos + i + 1);
    seek(f, end);
    Ok(Some(V::str(&text[pos..end])))
}

// ---- JSON → 值 ----

pub fn from_json(v: Value) -> V {
    match v {
        Value::Null => V::None,
        Value::Bool(b) => V::Bool(b),
        Value::Int(n) => V::Int(n),
        Value::Float(f) => V::Float(f),
        Value::Str(s) => V::str(s),
        Value::List(items) => V::list(items.into_iter().map(from_json).collect()),
        Value::Obj(o) => {
            let mut d = Dict::new();
            for (k, v) in o.into_pairs() {
                let key: Rc<str> = Rc::from(k);
                d.set(Key::Str(key.clone()), V::Str(key), from_json(v));
            }
            V::dict(d)
        }
    }
}

/// json.loads：写错了的是 JSONDecodeError；Python 读得了、这里读不了的（特别大的整数……）是退出码 2
pub fn parse_json(text: &str) -> R<V> {
    // 开头带着 BOM 的：Python 的 json 单独有一句
    if text.starts_with('\u{feff}') {
        return raise(Kind::JsonDecode, "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)");
    }
    match kit::json::parse(text) {
        Ok(v) => Ok(from_json(v)),
        Err(e) if e.unsupported() => unsupported(format!("JSON 里有这里读不了的内容：{e}")),
        Err(e) => {
            // 位置照 Python 的写法：第几行第几列（从 1 数）、第几个字符（从 0 数）
            let chars: Vec<char> = text.chars().collect();
            let (msg, pos) = python_error(&chars, &e.msg, e.pos.min(chars.len()));
            let before = &chars[..pos];
            let line = before.iter().filter(|c| **c == '\n').count() + 1;
            let column = before.iter().rev().take_while(|c| **c != '\n').count() + 1;
            raise(Kind::JsonDecode, format!("{msg}: line {line} column {column} (char {pos})"))
        }
    }
}

/// kit::json 报的错和位置 → Python 的 json 模块说的那一句、它指的位置
fn python_error<'a>(chars: &[char], msg: &'a str, pos: usize) -> (&'a str, usize) {
    // 前面（隔着空白）是不是逗号：是的话 Python 单独说“多了个逗号”，位置指着逗号
    let comma_before = || {
        let i = chars[..pos].iter().rposition(|c| !matches!(c, ' ' | '\t' | '\n' | '\r'))?;
        (chars[i] == ',').then_some(i)
    };
    match (msg, chars.get(pos)) {
        ("缺一个值", Some(']')) => {
            if let Some(i) = comma_before() {
                return ("Illegal trailing comma before end of array", i);
            }
        }
        ("键要用双引号括起来", Some('}')) => {
            if let Some(i) = comma_before() {
                return ("Illegal trailing comma before end of object", i);
            }
        }
        ("字符串没有结束", _) => return unterminated(chars),
        // Python 指的是那个反斜杠
        ("不认识的转义", _) if pos > 0 && chars[pos - 1] == '\\' => return ("Invalid \\escape", pos - 1),
        // Python 指的是反斜杠后面的那个 u
        ("反斜杠 u 后面要 4 位十六进制", _) => {
            let last = pos.min(chars.len().saturating_sub(1));
            if let Some(i) = (1..=last).rev().find(|&i| chars[i] == 'u' && chars[i - 1] == '\\') {
                return ("Invalid \\uXXXX escape", i);
            }
        }
        _ => {}
    }
    (english(msg), pos)
}

/// 字符串没有结束：Python 指的是它开头的那个引号（前面的内容都是对的，从头数引号就行）。
/// 一种情况除外：最后是反斜杠 u 加四个字符、后面什么都没有了，Python 说的是这个转义不对
fn unterminated(chars: &[char]) -> (&'static str, usize) {
    let (mut i, mut start, mut inside) = (0, 0, false);
    while i < chars.len() {
        match chars[i] {
            '\\' if inside => {
                if chars.get(i + 1) == Some(&'u') && i + 6 >= chars.len() {
                    return ("Invalid \\uXXXX escape", i + 1);
                }
                i += 1;
            }
            '"' => {
                inside = !inside;
                if inside {
                    start = i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    ("Unterminated string starting at", start)
}

/// kit::json 的出错说明 → Python 的 json 模块对应的那一句
fn english(msg: &str) -> &str {
    match msg {
        "缺一个值" => "Expecting value",
        "值后面还有内容" => "Extra data",
        "键要用双引号括起来" => "Expecting property name enclosed in double quotes",
        "缺 ':'" => "Expecting ':' delimiter",
        "缺 ',' 或 '}'" | "缺 ',' 或 ']'" => "Expecting ',' delimiter",
        "字符串没有结束" => "Unterminated string starting at",
        "不认识的转义" => "Invalid \\escape",
        "字符串里有控制字符" => "Invalid control character at",
        "反斜杠 u 后面要 4 位十六进制" => "Invalid \\uXXXX escape",
        other => other,
    }
}

/// 一行一个 JSON 的文件 → 列表（空行跳过）
pub fn json_lines(text: &str) -> R<V> {
    let mut items = Vec::new();
    for line in text.split('\n') {
        if kit::py::strip(line).is_empty() {
            continue;
        }
        items.push(parse_json(line)?);
    }
    Ok(V::list(items))
}

// ---- 值 → JSON ----

/// 值 → kit::json 的值（照 json.dumps：元组写成数组，键只能是 str、int、float、bool、None）
pub fn to_json(v: &V, sort_keys: bool) -> R<Value> {
    convert(v, sort_keys, &mut Vec::new())
}

fn nested(stack: &mut Vec<usize>, id: usize) -> R<()> {
    if stack.contains(&id) {
        return value_err("Circular reference detected");
    }
    if stack.len() > MAX_DATA_DEPTH {
        return unsupported("数据嵌套太深");
    }
    stack.push(id);
    Ok(())
}

fn convert(v: &V, sort_keys: bool, stack: &mut Vec<usize>) -> R<Value> {
    Ok(match v {
        V::None => Value::Null,
        V::Bool(b) => Value::Bool(*b),
        V::Int(n) => Value::Int(*n),
        V::Float(f) => Value::Float(*f),
        V::Str(s) => Value::Str(s.to_string()),
        V::List(l) => {
            nested(stack, Rc::as_ptr(l) as usize)?;
            let items = l.borrow().iter().map(|x| convert(x, sort_keys, stack)).collect::<R<Vec<_>>>()?;
            stack.pop();
            Value::List(items)
        }
        V::Tuple(t) => {
            nested(stack, Rc::as_ptr(t) as usize)?;
            let items = t.iter().map(|x| convert(x, sort_keys, stack)).collect::<R<Vec<_>>>()?;
            stack.pop();
            Value::List(items)
        }
        V::Dict(d) => {
            nested(stack, Rc::as_ptr(d) as usize)?;
            let d = d.borrow();
            let mut entries: Vec<&Entry> = d.entries().iter().collect();
            if sort_keys {
                // Python 排的是原来的键（1、2、10 按数值），排完才变成字符串；键之间比不了的是 TypeError
                entries = sort_entries(entries)?;
            }
            let mut obj = Obj::new();
            for e in entries {
                let name = key_name(&e.k)?;
                if obj.contains_key(&name) {
                    return unsupported(format!("json.dumps：两个键写成 JSON 都是 {name:?}（Python 会写出重复的键）"));
                }
                obj.insert(name, convert(&e.v, sort_keys, stack)?);
            }
            stack.pop();
            Value::Obj(obj)
        }
        other => return type_err(format!("Object of type {} is not JSON serializable", type_name(other))),
    })
}

fn sort_entries(v: Vec<&Entry>) -> R<Vec<&Entry>> {
    if v.len() <= 1 {
        return Ok(v);
    }
    let mut left = v;
    let right = left.split_off(left.len() / 2);
    let (left, right) = (sort_entries(left)?, sort_entries(right)?);
    let mut out = Vec::with_capacity(left.len() + right.len());
    let (mut l, mut r) = (left.into_iter().peekable(), right.into_iter().peekable());
    loop {
        let take_right = match (l.peek(), r.peek()) {
            (Some(a), Some(b)) => ops::order(CmpOp::Lt, &b.k, &a.k, true)?,
            (Some(_), None) => false,
            (None, Some(_)) => true,
            (None, None) => return Ok(out),
        };
        out.extend(if take_right { r.next() } else { l.next() });
    }
}

fn key_name(k: &V) -> R<String> {
    Ok(match k {
        V::Str(s) => s.to_string(),
        V::Int(n) => n.to_string(),
        V::Bool(true) => "true".to_string(),
        V::Bool(false) => "false".to_string(),
        V::None => "null".to_string(),
        V::Float(f) if f.is_nan() => "NaN".to_string(),
        V::Float(f) if f.is_infinite() => if *f > 0.0 { "Infinity" } else { "-Infinity" }.to_string(),
        V::Float(f) => kit::json::float_repr(*f),
        other => return type_err(format!("keys must be str, int, float, bool or None, not {}", type_name(other))),
    })
}

/// json.dumps(obj, indent=, ensure_ascii=, sort_keys=, separators=)
pub fn dumps(args: Vec<V>, mut kw: Kw) -> R<V> {
    let indent = take_kw(&mut kw, "indent");
    let ensure_ascii = take_kw(&mut kw, "ensure_ascii");
    let sort_keys = take_kw(&mut kw, "sort_keys").is_some_and(|v| truthy(&v));
    let separators = take_kw(&mut kw, "separators");
    no_kw("json.dumps", &kw)?;
    if args.len() > 1 {
        return type_err(format!("dumps() takes 1 positional argument but {} were given", args.len()));
    }
    if args.is_empty() {
        return type_err("dumps() missing 1 required positional argument: 'obj'");
    }
    let mut style = Style { unicode: ensure_ascii.is_some_and(|v| !truthy(&v)), ..Style::default() };
    style.indent = match indent {
        None | Some(V::None) => None,
        Some(v) => match as_int(&v) {
            Some(n) => Some(n.clamp(0, 1000) as usize),
            None => return unsupported("json.dumps 的 indent 只支持整数"),
        },
    };
    let pair = match &separators {
        None | Some(V::None) => None,
        Some(V::Tuple(t)) => Some(t.to_vec()),
        Some(V::List(l)) => Some(l.borrow().clone()),
        Some(_) => return unsupported("json.dumps 的 separators 要是两个字符串"),
    };
    if let Some(pair) = pair {
        match &pair[..] {
            [V::Str(item), V::Str(key)] => match (&**item, &**key, style.indent.is_some()) {
                (",", ":", _) => style.compact = true,
                // 就是默认的那一组
                (", ", ": ", false) | (",", ": ", true) => {}
                _ => return unsupported("json.dumps 的 separators 只支持 (\",\", \":\") 和默认的那一组"),
            },
            _ => return unsupported("json.dumps 的 separators 要是两个字符串"),
        }
    }
    Ok(V::str(dump_text(&args[0], &style, sort_keys)?))
}

/// 值写成 JSON 的字（排序在这里做，不交给 kit::json：它按字符串排，Python 按原来的键排）
pub fn dump_text(v: &V, style: &Style, sort_keys: bool) -> R<String> {
    let value = to_json(v, sort_keys || style.sort_keys)?;
    Ok(kit::json::dumps(&value, &Style { indent: style.indent, compact: style.compact, unicode: style.unicode, sort_keys: false }))
}
