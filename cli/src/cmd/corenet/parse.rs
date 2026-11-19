//! 自定义节点的解析（`NodeParseError` 到 `node_parse`）：用户粘贴的分享链接（vmess / ss / vless / trojan / hysteria2 / tuic）
//! 或一段 Clash 节点配置（JSON、一行写完的 `{…}`、手写的那一小块 YAML）→ 一个节点定义。
//!
//! 手写的 YAML 解析（`_y_scalar`、`_y_split`、`_y_flow`、`_yaml_mini`）只认原来认的那个子集，认不出的地方报的错
//! （`NodeParseError` 的原话）也照原样；原来没人接的异常（`ss://` 后面不是 base64、`aid` 不是数字、网址的端口写错……）
//! 这里是往上传的 `Fail::Value` / `Fail::Other`，main 照样说一句、退出码 1。
//!
//! 节点定义里有密钥（密码、uuid）：这里只在内存里经手，出错的说明里原来不回显的也不回显。

use crate::py::base64::b64decode;
use crate::py::json;
use crate::py::obj::{repr_str, Dict, V};
use crate::py::text;
use crate::py::uni;
use crate::py::url::{parse_qsl, unquote, urlsplit};
use crate::py::{Fail, R};
use crate::{dict, re};
use std::rc::Rc;

/// `raise NodeParseError(说明)`
fn npe<T>(msg: impl Into<String>) -> R<T> {
    Err(Fail::Other("NodeParseError", vec![msg.into()]))
}

/// 解析出来的一个值。JSON 读出来的（字典的键都是字符串）原样是 `V`；`_y_flow` / `_yaml_mini` 自己拼的字典，键是 `_y_scalar`
/// 给的，可能是数字、布尔值、None（`1: x`、`true: x`、`~: x`），`V` 的字典放不下，另用一种：键照 Python 的 `==` 认
/// （`1` 和 `true` 是同一个键，`1` 和 `"1"` 不是），写成 JSON 时才换成文字（所以可能写出两个一样的键，原来就这样）
#[derive(Clone)]
pub(super) enum Y {
    V(V),
    Map(Vec<(V, Y)>),
    Seq(Vec<Y>),
}

impl Y {
    /// `bool(x)`
    fn truthy(&self) -> bool {
        match self {
            Y::V(v) => v.truthy(),
            Y::Map(m) => !m.is_empty(),
            Y::Seq(s) => !s.is_empty(),
        }
    }

    pub(super) fn as_str(&self) -> Option<&str> {
        match self {
            Y::V(v) => v.as_str(),
            _ => None,
        }
    }

    /// `isinstance(x, list)`
    fn is_list(&self) -> bool {
        matches!(self, Y::Seq(_) | Y::V(V::List(_)))
    }

    /// `x[0]`，x 是列表；空的是 None
    fn first(&self) -> Option<Y> {
        match self {
            Y::Seq(items) => items.first().cloned(),
            Y::V(V::List(l)) => l.get(0).map(Y::V),
            _ => None,
        }
    }

    /// `isinstance(x, dict)` 时它的各项（键、值，照先后）；不是字典是 None
    fn into_items(self) -> Option<Vec<(V, Y)>> {
        match self {
            Y::Map(items) => Some(items),
            Y::V(V::Dict(d)) => Some(d.items().into_iter().map(|(k, v)| (V::Str(k), Y::V(v))).collect()),
            _ => None,
        }
    }

    /// `x.get(k)`，x 是字典（k 是字符串）
    fn get(&self, k: &str) -> Option<Y> {
        match self {
            Y::Map(items) => items.iter().find(|(key, _)| key.as_str() == Some(k)).map(|(_, v)| v.clone()),
            Y::V(V::Dict(d)) => d.get(k).map(Y::V),
            _ => None,
        }
    }

    /// `repr(x)`
    fn py_repr(&self) -> String {
        match self {
            Y::V(v) => v.py_repr(),
            Y::Map(items) => format!(
                "{{{}}}",
                items.iter().map(|(k, v)| format!("{}: {}", k.py_repr(), v.py_repr())).collect::<Vec<_>>().join(", ")
            ),
            Y::Seq(items) => format!("[{}]", items.iter().map(|v| v.py_repr()).collect::<Vec<_>>().join(", ")),
        }
    }

    /// `str(x)`
    fn py_str(&self) -> String {
        match self {
            Y::V(v) => v.py_str(),
            _ => self.py_repr(),
        }
    }

    /// `json.dumps(x, ensure_ascii=False)` 接在 out 后面
    fn dump(&self, out: &mut String) {
        match self {
            Y::V(v) => out.push_str(&json::dumps(v, json::LINE)),
            Y::Seq(items) => {
                out.push('[');
                for (n, v) in items.iter().enumerate() {
                    if n > 0 {
                        out.push_str(", ");
                    }
                    v.dump(out);
                }
                out.push(']');
            }
            Y::Map(items) => dump_items(items, out),
        }
    }
}

impl std::fmt::Debug for Y {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.py_repr())
    }
}

impl<T: Into<V>> From<T> for Y {
    fn from(v: T) -> Y {
        Y::V(v.into())
    }
}

/// 字典写成 JSON：键不是字符串的照 `json.dumps` 换成文字（`true`、`null`、十进制的整数）
fn dump_items(items: &[(V, Y)], out: &mut String) {
    out.push('{');
    for (n, (k, v)) in items.iter().enumerate() {
        if n > 0 {
            out.push_str(", ");
        }
        json::write_str(out, &json_key(k), false);
        out.push_str(": ");
        v.dump(out);
    }
    out.push('}');
}

/// `json.dumps` 把字典的键写成什么文字。列表、字典当不了键（Python 里是没人接的 TypeError）
pub(super) fn json_key(k: &V) -> String {
    match k {
        V::Str(s) => s.to_string(),
        V::None => "null".into(),
        V::Bool(true) => "true".into(),
        V::Bool(false) => "false".into(),
        V::Int(_) | V::Big(_) | V::Float(_) => k.py_repr(),
        V::List(_) | V::Dict(_) => panic!("TypeError: unhashable type: '{}'", if k.is_list() { "list" } else { "dict" }),
    }
}

/// `d[key] = value`：已有的键（照 Python 的 `==`）留在原来的位置、留着原来那个键，只换值
fn map_set(items: &mut Vec<(V, Y)>, key: V, value: Y) {
    match items.iter_mut().find(|(k, _)| k.py_eq(&key)) {
        Some(slot) => slot.1 = value,
        None => items.push((key, value)),
    }
}

/// 一个节点定义：各项照放进去的先后，也就是写出来的先后
#[derive(Clone, Debug)]
pub(super) struct Node(Vec<(V, Y)>);

impl Node {
    fn new() -> Node {
        Node(Vec::new())
    }

    /// `p.get(k)`
    pub(super) fn get(&self, k: &str) -> Option<&Y> {
        self.0.iter().find(|(key, _)| key.as_str() == Some(k)).map(|(_, v)| v)
    }

    /// `p[k] = v`
    pub(super) fn set(&mut self, k: &str, v: impl Into<Y>) {
        map_set(&mut self.0, V::from(k), v.into());
    }

    /// `json.dumps(p, ensure_ascii=False)`
    pub(super) fn dumps(&self) -> String {
        let mut out = String::new();
        dump_items(&self.0, &mut out);
        out
    }
}

/// `int(文字)` 的结果：放得进 64 位的，和放不进的（十进制的写法，不带前导 0）
enum PyInt {
    Small(i64),
    Big(String),
}

impl PyInt {
    fn into_v(self) -> V {
        match self {
            PyInt::Small(n) => V::Int(n),
            PyInt::Big(s) => V::Big(Rc::from(s)),
        }
    }
}

/// Python 3.11 起 `int(文字)` 最多认这么多位数字（多了是 ValueError）
const INT_MAX_DIGITS: usize = 4300;

/// `int(文字)` 超过上限时 ValueError 的说法
fn too_many_digits(n: usize) -> String {
    format!("Exceeds the limit ({INT_MAX_DIGITS} digits) for integer string conversion: value has {n} digits; use sys.set_int_max_str_digits() to increase the limit")
}

/// `json.loads(s)`。兼容层的 `json::loads` 还没有上面那条上限（Python 读到超过 4300 位的整数是 ValueError，它照收）：
/// 粘贴进来的、从 stdin 读的 JSON 先在这里补上，兼容层有了以后换回去
pub(super) fn json_loads(s: &str) -> Result<V, json::JsonError> {
    fn huge(v: &V) -> Option<usize> {
        match v {
            V::Big(digits) => Some(digits.trim_start_matches('-').len()).filter(|n| *n > INT_MAX_DIGITS),
            V::List(l) => l.items().iter().find_map(huge),
            V::Dict(d) => d.values().iter().find_map(huge),
            _ => None,
        }
    }
    let v = json::loads(s)?;
    match huge(&v) {
        Some(n) => Err(json::JsonError(too_many_digits(n))),
        None => Ok(v),
    }
}

/// `int(s)`：前后的空白、正负号、数字之间的下划线、各种文字的十进制数字都认。认不出、数字超过 4300 位是 ValueError（None）
fn py_int(s: &str) -> Option<PyInt> {
    let s = text::strip(s);
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let mut digits = String::new();
    let mut prev_digit = false;
    for c in body.chars() {
        if c == '_' {
            if !prev_digit {
                return None;
            }
            prev_digit = false;
            continue;
        }
        digits.push(char::from_digit(uni::decimal_value(c)?, 10)?);
        prev_digit = true;
    }
    if !prev_digit || digits.len() > INT_MAX_DIGITS {
        return None;
    }
    let canon = match digits.trim_start_matches('0') {
        "" => "0",
        d => d,
    };
    let signed = if neg && canon != "0" { format!("-{canon}") } else { canon.to_string() };
    Some(match signed.parse::<i64>() {
        Ok(n) => PyInt::Small(n),
        Err(_) => PyInt::Big(signed),
    })
}

/// `int(x)`，x 是 JSON 里读出来的值（vmess 链接的 aid）。认不出在 Python 里是没人接的 ValueError / TypeError / OverflowError
fn int_of(v: &V) -> R<V> {
    match v {
        V::Int(_) | V::Big(_) => Ok(v.clone()),
        V::Bool(b) => Ok(V::Int(*b as i64)),
        V::Float(f) if f.is_nan() => Err(Fail::Value("cannot convert float NaN to integer".into())),
        V::Float(f) if f.is_infinite() => {
            Err(Fail::Other("OverflowError", vec!["cannot convert float infinity to integer".into()]))
        }
        // 朝 0 截断；放不进 64 位的小数照它真正的值写成整数
        V::Float(f) if f.abs() < 9.2e18 => Ok(V::Int(f.trunc() as i64)),
        V::Float(f) => Ok(V::Big(Rc::from(format!("{:.0}", f.trunc())))),
        V::Str(s) => match py_int(s) {
            Some(n) => Ok(n.into_v()),
            None => Err(Fail::Value(format!("invalid literal for int() with base 10: {}", repr_str(s)))),
        },
        V::None | V::List(_) | V::Dict(_) => {
            Err(Fail::Other("TypeError", vec!["int() argument must be a string, a bytes-like object or a real number".into()]))
        }
    }
}

/// `_b64`：分享链接里的 base64。URL 安全的写法（`-`、`_`）也认，缺的 `=` 补上；别的宽严照 `base64.b64decode`
fn b64(s: &str) -> R<Vec<u8>> {
    let mut s = text::strip(s).replace('-', "+").replace('_', "/");
    let pad = (4 - text::len(&s) % 4) % 4; // '=' * (-len(s) % 4)
    s.push_str(&"=".repeat(pad));
    b64decode(&s, false)
}

/// `_node_int`：端口这类数字，`int(str(v).strip())`，要在 1–65535 里
fn node_int(v: &Y, what: &str) -> R<i64> {
    match py_int(&v.py_str()) {
        None => npe(format!("{} 不是数字：{}", what, v.py_repr())),
        Some(PyInt::Small(n)) if (1..=65535).contains(&n) => Ok(n),
        Some(_) => npe(format!("端口超出范围：{}", v.py_repr())),
    }
}

/// `_y_scalar`：YAML 里的一个标量。空的、`~`、`null` 是 None；成对的引号去掉（里面不转义）；`true` / `false`；整数；别的是文字
fn y_scalar(tok: &str) -> R<V> {
    let t = text::strip(tok);
    // t.lower() == 'null'：小写以后是这几个词的只有它们的 ASCII 大小写写法
    if t.is_empty() || t == "~" || t.eq_ignore_ascii_case("null") {
        return Ok(V::None);
    }
    let (first, last) = (t.chars().next(), t.chars().next_back());
    if text::len(t) >= 2 && matches!(first, Some('"' | '\'')) && last == first {
        return Ok(V::str(&t[1..t.len() - 1]));
    }
    if t.eq_ignore_ascii_case("true") {
        return Ok(V::Bool(true));
    }
    if t.eq_ignore_ascii_case("false") {
        return Ok(V::Bool(false));
    }
    // re.match(r'^-?\d+$', t)：\d 认各种文字的十进制数字，int() 也认
    let digits = t.strip_prefix('-').unwrap_or(t);
    if !digits.is_empty() && digits.chars().all(uni::is_decimal) {
        return match py_int(t) {
            Some(n) => Ok(n.into_v()),
            None => Err(Fail::Value(too_many_digits(text::len(digits)))),
        };
    }
    Ok(V::str(t))
}

/// `_y_split`：按最外层的逗号切开（引号里、括号里的逗号不算）
fn y_split(s: &str) -> Vec<String> {
    let (mut out, mut depth, mut cur, mut quote) = (Vec::new(), 0i64, String::new(), None::<char>);
    for ch in s.chars() {
        if let Some(q) = quote {
            cur.push(ch);
            if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => {
                quote = Some(ch);
                cur.push(ch);
            }
            '{' | '[' => {
                depth += 1;
                cur.push(ch);
            }
            '}' | ']' => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if depth == 0 => out.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    out.push(cur);
    out
}

/// `_y_flow`：一行写完的值。是 JSON 就照 JSON；`{k: v, …}`、`[a, b]` 里面只认标量（不再往里套）；别的是一个标量
fn y_flow(tok: &str) -> R<Y> {
    let t = text::strip(tok);
    if let Ok(v) = json_loads(t) {
        return Ok(Y::V(v));
    }
    if t.starts_with('{') && t.ends_with('}') {
        let mut d = Vec::new();
        for part in y_split(&t[1..t.len() - 1]) {
            if text::strip(&part).is_empty() {
                continue;
            }
            let (k, sep, v) = text::partition(&part, ":");
            if sep.is_empty() {
                return npe(format!("读不懂：{part}"));
            }
            let value = y_scalar(v)?;
            map_set(&mut d, y_scalar(k)?, Y::V(value));
        }
        return Ok(Y::Map(d));
    }
    if t.starts_with('[') && t.ends_with(']') {
        let mut items = Vec::new();
        for p in y_split(&t[1..t.len() - 1]) {
            if !text::strip(&p).is_empty() {
                items.push(Y::V(y_scalar(&p)?));
            }
        }
        return Ok(Y::Seq(items));
    }
    Ok(Y::V(y_scalar(t)?))
}

/// 一行开头有几个空格（只数空格，制表符不算）
fn indent(ln: &str) -> usize {
    ln.len() - ln.trim_start_matches(' ').len()
}

/// `_yaml_mini`：块写法的那一小块 YAML——`键: 值`、往里缩进的下一层、`- ` 开头的列表。空行、注释、`---`、`...` 先去掉；
/// 多行文字、锚点、合并这些都不认
fn yaml_mini(src: &str) -> R<Y> {
    let mut lines: Vec<String> = src
        .split('\n')
        .filter(|s| {
            let t = text::strip(s);
            !t.is_empty() && !text::lstrip(s).starts_with('#') && t != "---" && t != "..."
        })
        .map(|s| text::rstrip(s).to_string())
        .collect();
    if lines.is_empty() {
        return npe("没有内容");
    }
    let mut pos = 0;
    let base = indent(&lines[0]);
    let val = parse_block(&mut lines, &mut pos, base, 1)?;
    if pos != lines.len() {
        return npe(format!("缩进对不齐（第 {} 行）", pos + 1));
    }
    Ok(val)
}

/// 一块套一块最多几层，最外面那一块算第 1 层（Python 3.14 下实测的：最里面一行是 `键: 值` 时 991 层还行，再深是 RecursionError）
const BLOCK_DEPTH: usize = 991;

/// `_yaml_mini` 里的 `parse_block`：从第 pos 行起、缩进是 base 的这一块（一个列表或一个字典），读完 pos 停在它后面那一行。
/// depth：这是第几层。原来是一层层递归的，到 Python 的递归上限是没人接的 RecursionError；具体第几层随最里面那一行写的是什么、
/// Python 的版本差两三层，这里固定成 BLOCK_DEPTH 层（见 FINDINGS.md）
fn parse_block(lines: &mut [String], pos: &mut usize, base: usize, depth: usize) -> R<Y> {
    if depth > BLOCK_DEPTH {
        panic!("RecursionError: maximum recursion depth exceeded");
    }
    let Some(head) = lines.get(*pos) else {
        panic!("IndexError: list index out of range");
    };
    if text::lstrip(head).starts_with("- ") {
        let mut out = Vec::new();
        while *pos < lines.len() {
            let ln = lines[*pos].clone();
            let ind = indent(&ln);
            let body = text::lstrip(&ln);
            if ind != base || !body.starts_with("- ") {
                break;
            }
            let content = &body[2..];
            *pos += 1;
            if text::strip(content).is_empty() {
                out.push(parse_block(lines, pos, base + 2, depth + 1)?);
            } else if content.contains(':') && !matches!(text::strip(content).chars().next(), Some('{' | '[')) {
                // `- 键: 值`：这一项是个字典，第一行就在 `- ` 后面。把这一行改成往里缩两格的样子，当成一块字典接着读
                lines[*pos - 1] = format!("{}{}", " ".repeat(ind + 2), content);
                *pos -= 1;
                out.push(parse_block(lines, pos, ind + 2, depth + 1)?);
            } else {
                out.push(y_flow(content)?);
            }
        }
        return Ok(Y::Seq(out));
    }
    let mut out = Vec::new();
    while *pos < lines.len() {
        let ln = lines[*pos].clone();
        if indent(&ln) != base {
            break;
        }
        let line = text::strip(&ln);
        let Some(m) = re!(r"^(\x22(?:[^\x22\\]|\\.)*\x22|\x27[^\x27]*\x27|[^:]+?)\s*:(?:\s+(.*))?$").match_(line) else {
            return npe(format!("读不懂这一行：{line}"));
        };
        let key = y_scalar(m.g(1))?;
        *pos += 1;
        let value = match m.group(2) {
            None | Some("") => {
                if *pos < lines.len() && indent(&lines[*pos]) > base {
                    let deeper = indent(&lines[*pos]);
                    parse_block(lines, pos, deeper, depth + 1)?
                } else {
                    Y::V(V::None)
                }
            }
            Some(val) => y_flow(val)?,
        };
        map_set(&mut out, key, value);
    }
    Ok(Y::Map(out))
}

/// `clash_block_parse`：一段 Clash 配置里的第一个节点（整份配置的 `proxies` 里的、一个列表里的、或者它自己就是一个节点）
fn clash_block_parse(src: &str) -> R<Node> {
    let t = text::strip(src);
    let mut j = match json_loads(t) {
        Ok(v) => Y::V(v),
        // text[:1] in '{['：空的也算（'' in 任何文字 都是真）
        Err(_) if matches!(t.chars().next(), None | Some('{' | '[')) => y_flow(t)?,
        Err(_) => yaml_mini(t)?,
    };
    if let Some(px) = j.get("proxies").filter(|px| px.is_list()) {
        j = px;
    }
    if j.is_list() {
        let Some(first) = j.first() else {
            return npe("没有节点");
        };
        j = first;
    }
    match j.into_items() {
        Some(items) => Ok(Node(items)),
        None => npe("不是一个 Clash 节点定义"),
    }
}

/// 分享链接 `?` 后面的参数：同一个名字出现几次的只要第一个（`{k: v[0] for k, v in parse_qs(…).items()}`）。
/// 空的值 parse_qs 本来就不留，所以有就是真
struct Query(Vec<(String, String)>);

impl Query {
    fn parse(qs: &str) -> Query {
        let mut q: Vec<(String, String)> = Vec::new();
        for (k, v) in parse_qsl(qs, false) {
            if !q.iter().any(|(have, _)| *have == k) {
                q.push((k, v));
            }
        }
        Query(q)
    }

    fn get(&self, k: &str) -> Option<&str> {
        self.0.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str()).filter(|v| !v.is_empty())
    }
}

/// `_link_transport`：链接里的传输方式（ws、grpc）
fn link_transport(p: &mut Node, q: &Query) {
    let net = q.get("type").or_else(|| q.get("net"));
    if let Some(n) = net {
        if !matches!(n, "tcp" | "none" | "original") {
            p.set("network", n);
        }
    }
    if net == Some("ws") {
        let opts = Dict::new();
        if let Some(path) = q.get("path") {
            opts.set("path", unquote(path));
        }
        if let Some(host) = q.get("host") {
            opts.set("headers", dict! { "Host" => host });
        }
        if !opts.is_empty() {
            p.set("ws-opts", opts);
        }
    } else if net == Some("grpc") {
        if let Some(service) = q.get("serviceName") {
            p.set("grpc-opts", dict! { "grpc-service-name" => service });
        }
    }
}

/// `_link_vmess`：`vmess://` 后面是 base64 的 JSON
fn link_vmess(url: &str) -> R<Node> {
    let decoded = match b64(text::skip(url, 8)) {
        Ok(raw) => json_loads(&String::from_utf8_lossy(&raw)).ok(),
        Err(e) if e.is_value() => None,
        Err(e) => return Err(e),
    };
    let Some(j) = decoded else {
        return npe("vmess 链接解不开（应为 base64 的 JSON）");
    };
    let V::Dict(j) = j else {
        return npe("vmess 链接里不是 JSON 对象");
    };
    let truthy = |k: &str| j.get(k).filter(|v| v.truthy());
    let mut p = Node::new();
    p.set("name", truthy("ps").unwrap_or_else(|| "".into()));
    p.set("type", "vmess");
    p.set("server", truthy("add").unwrap_or_else(|| "".into()));
    p.set("port", node_int(&Y::V(j.get("port").unwrap_or(V::None)), "端口")?);
    p.set("uuid", truthy("id").unwrap_or_else(|| "".into()));
    p.set("alterId", int_of(&truthy("aid").unwrap_or(V::Int(0)))?);
    p.set("cipher", truthy("scy").unwrap_or_else(|| "auto".into()));
    let net = j.get("net");
    let net_is = |s: &str| matches!(&net, Some(V::Str(n)) if &**n == s);
    if let Some(n) = net.as_ref().filter(|n| n.truthy()) {
        if !net_is("tcp") && !net_is("none") {
            p.set("network", n.clone());
        }
    }
    if j.get("tls").unwrap_or(V::None).py_str() == "tls" {
        p.set("tls", true);
    }
    if let Some(sni) = truthy("sni") {
        p.set("servername", sni);
    }
    if net_is("ws") {
        let opts = Dict::new();
        if let Some(path) = truthy("path") {
            opts.set("path", path);
        }
        if let Some(host) = truthy("host") {
            opts.set("headers", dict! { "Host" => host });
        }
        if !opts.is_empty() {
            p.set("ws-opts", opts);
        }
    } else if net_is("grpc") {
        if let Some(path) = truthy("path") {
            p.set("grpc-opts", dict! { "grpc-service-name" => path });
        }
    }
    Ok(p)
}

/// `_link_ss`：`ss://base64(method:password)@主机:端口#名字`，或者整个 `method:password@主机:端口` 一起 base64
fn link_ss(url: &str) -> R<Node> {
    let mut body = text::skip(url, 5);
    let mut name = String::new();
    if let Some((before, frag)) = body.split_once('#') {
        body = before;
        name = unquote(frag);
    }
    let body = body.split_once('?').map_or(body, |(before, _)| before);
    let (method, password, hostport);
    if let Some((userinfo, hp)) = body.rsplit_once('@') {
        // base64 解不开、解出来没有冒号都退一步：当它是没编码的 method:password
        let decoded = match b64(userinfo) {
            Ok(raw) => Some(String::from_utf8_lossy(&raw).into_owned()),
            Err(e) if e.is_value() => None,
            Err(e) => return Err(e),
        };
        let plain = unquote(userinfo);
        let Some((m, pw)) = decoded.as_deref().and_then(|d| d.split_once(':')).or_else(|| plain.split_once(':')) else {
            return npe("ss 链接的 method:password 解不开");
        };
        (method, password, hostport) = (m.to_string(), pw.to_string(), hp.to_string());
    } else {
        // 这里的 base64 解不开在 Python 里是没人接的 binascii.Error
        let dec = String::from_utf8_lossy(&b64(body)?).into_owned();
        let Some((userinfo, hp)) = dec.rsplit_once('@').filter(|_| dec.contains(':')) else {
            return npe("ss 链接解不开");
        };
        // method, password = userinfo.split(':', 1)：冒号只在主机那一半里时是没人接的 ValueError
        let Some((m, pw)) = userinfo.split_once(':') else {
            return Err(Fail::Value("not enough values to unpack (expected 2, got 1)".into()));
        };
        (method, password, hostport) = (m.to_string(), pw.to_string(), hp.to_string());
    }
    let (host, _, port) = text::rpartition(&hostport, ":");
    let mut p = Node::new();
    p.set("name", name);
    p.set("type", "ss");
    p.set("server", host);
    p.set("port", node_int(&Y::from(port), "端口")?);
    p.set("cipher", method);
    p.set("password", password);
    Ok(p)
}

/// `link_parse`：一条分享链接 → 节点定义
fn link_parse(url: &str, scheme: &str) -> R<Node> {
    if scheme == "vmess" {
        return link_vmess(url);
    }
    if scheme == "ss" {
        return link_ss(url);
    }
    let (url, name) = match url.split_once('#') {
        Some((before, frag)) => (before, unquote(frag)),
        None => (url, String::new()),
    };
    // 网址切不开、端口不是数字在 Python 里是没人接的 ValueError
    let u = urlsplit(url).map_err(Fail::Value)?;
    let host = u.hostname().unwrap_or_default();
    let port = node_int(&Y::V(V::from(u.port().map_err(Fail::Value)?)), "端口")?;
    let q = Query::parse(&u.query);
    let user = unquote(u.username().unwrap_or(""));
    let pw = u.password().map(unquote);
    let secret = || if user.is_empty() { pw.clone().unwrap_or_default() } else { user.clone() }; // user or pw
    let mut p = Node::new();
    p.set("name", name);
    match scheme {
        "vless" => {
            p.set("type", "vless");
            p.set("server", host);
            p.set("port", port);
            p.set("uuid", user);
            link_transport(&mut p, &q);
            if let Some(flow) = q.get("flow") {
                p.set("flow", flow);
            }
            match q.get("security") {
                Some("tls") => p.set("tls", true),
                Some("reality") => {
                    p.set("tls", true);
                    let ro = Dict::new();
                    if let Some(pbk) = q.get("pbk") {
                        ro.set("public-key", pbk);
                    }
                    if let Some(sid) = q.get("sid") {
                        ro.set("short-id", sid);
                    }
                    if !ro.is_empty() {
                        p.set("reality-opts", ro);
                    }
                }
                _ => {}
            }
            if let Some(sni) = q.get("sni") {
                p.set("servername", sni);
            }
            if let Some(fp) = q.get("fp") {
                p.set("client-fingerprint", fp);
            }
        }
        "trojan" => {
            p.set("type", "trojan");
            p.set("server", host);
            p.set("port", port);
            p.set("password", secret());
            if let Some(sni) = q.get("sni") {
                p.set("sni", sni);
            }
            if matches!(q.get("allowInsecure"), Some("1" | "true")) {
                p.set("skip-cert-verify", true);
            }
            link_transport(&mut p, &q);
        }
        "hysteria2" | "hy2" => {
            p.set("type", "hysteria2");
            p.set("server", host);
            p.set("port", port);
            p.set("password", secret());
            if let Some(sni) = q.get("sni") {
                p.set("sni", sni);
            }
            if let Some(obfs) = q.get("obfs") {
                p.set("obfs", obfs);
            }
            if let Some(pass) = q.get("obfs-password") {
                p.set("obfs-password", pass);
            }
            if matches!(q.get("insecure"), Some("1" | "true")) {
                p.set("skip-cert-verify", true);
            }
        }
        "tuic" => {
            p.set("type", "tuic");
            p.set("server", host);
            p.set("port", port);
            match pw {
                Some(pw) => {
                    p.set("uuid", user);
                    p.set("password", pw);
                }
                None => p.set("token", user),
            }
            if let Some(sni) = q.get("sni") {
                p.set("sni", sni);
            }
            if let Some(cc) = q.get("congestion_control") {
                p.set("congestion-controller", cc);
            }
            if let Some(alpn) = q.get("alpn") {
                p.set("alpn", crate::py::obj::str_list(alpn.split(',')));
            }
        }
        _ => return npe(format!("不支持的链接：{scheme}")),
    }
    Ok(p)
}

/// `re.match(r'(?i)^(vmess|vless|ss|ssr|trojan|hysteria2|hy2|tuic)://', text)` 的第 1 组（原样的大小写）。
/// 手写，不用兼容层的正则：Python 不分大小写时还把 ſ（U+017F）当 s、İ（U+0130）和 ı（U+0131）当 i，`regex` 库后两个不认
fn link_scheme(t: &str) -> Option<&str> {
    const SCHEMES: [&str; 8] = ["vmess", "vless", "ss", "ssr", "trojan", "hysteria2", "hy2", "tuic"];
    let same = |c: char, want: char| {
        c.to_ascii_lowercase() == want || (want == 's' && c == 'ſ') || (want == 'i' && matches!(c, 'İ' | 'ı'))
    };
    'next: for sch in SCHEMES {
        let (mut chars, mut end) = (t.char_indices(), 0);
        for want in sch.chars() {
            match chars.next() {
                Some((i, c)) if same(c, want) => end = i + c.len_utf8(),
                _ => continue 'next,
            }
        }
        if t[end..].starts_with("://") {
            return Some(&t[..end]);
        }
    }
    None
}

/// `node_parse`：用户粘贴的一段文字 → 一个节点定义（type、server 是文字，port 是 1–65535 的整数）
pub(super) fn node_parse(src: &str) -> R<Node> {
    let t = text::strip(src);
    if t.is_empty() {
        return npe("没有内容");
    }
    let mut p = match link_scheme(t) {
        Some(found) => {
            let sch = found.to_lowercase();
            if sch == "ssr" {
                return npe("暂不支持 ssr:// 链接");
            }
            link_parse(t, &sch)?
        }
        None => clash_block_parse(t)?,
    };
    for k in ["type", "server", "port"] {
        if !p.get(k).is_some_and(|v| v.truthy()) {
            return npe(format!("节点缺少 {k}（必须有 type、server、port）"));
        }
    }
    let port = node_int(p.get("port").expect("上面查过有 port"), "端口")?;
    p.set("port", port);
    if p.get("type").and_then(|v| v.as_str()).is_none() || p.get("server").and_then(|v| v.as_str()).is_none() {
        return npe("type 和 server 要是文本");
    }
    Ok(p)
}

/// `node_clean_name`：节点名规整到 fill_name_ok 的样子——去掉控制字符、首尾空格、开头的 CCA-，截到 128
pub(super) fn node_clean_name(n: Option<&Y>) -> String {
    let Some(n) = n.and_then(|n| n.as_str()) else {
        return String::new();
    };
    let spaced: String = n.chars().map(|c| if (c as u32) < 0x20 || c == '\u{7f}' { ' ' } else { c }).collect();
    let mut n = text::head(text::strip(&spaced), 128).to_string();
    while let Some(rest) = n.strip_prefix("CCA-") {
        n = text::strip(rest).to_string();
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对照数据：原版 PY_SRC 的 node_parse、node_clean_name 对一批输入给出的结果（cli/tools/gen_corenet_vectors.py 生成）
    fn vectors() -> Dict {
        json::loads(include_str!("../../../tests/vectors/corenet.json")).unwrap().as_dict().unwrap()
    }

    #[test]
    fn node_parse_matches_python_vectors() {
        let cases = vectors().list("node_parse").unwrap().items();
        assert!(cases.len() > 1000);
        let (mut ok, mut refused, mut raised) = (0, 0, 0);
        for c in cases {
            let c = c.as_dict().unwrap();
            let (name, input) = (c.str("name").unwrap(), c.str("in").unwrap());
            match (node_parse(&input), c.str("ok"), c.str("err"), c.str("raise")) {
                // 解析成了：写成 JSON 逐字节一样（键的先后、不是字符串的键、数字的写法都在里面）
                (Ok(p), Some(want), _, _) => {
                    assert_eq!(p.dumps(), &*want, "{name}");
                    ok += 1;
                }
                // 认不出：NodeParseError 的原话
                (Err(Fail::Other("NodeParseError", args)), _, Some(want), _) => {
                    assert_eq!(args[0], &*want, "{name}");
                    refused += 1;
                }
                // 原来没人接的异常：是同一类
                (Err(Fail::Value(_)), _, _, Some(class)) if &*class == "ValueError" => raised += 1,
                (Err(Fail::Other(class, _)), _, _, Some(want)) if class == &*want => raised += 1,
                (got, ok, err, raise) => {
                    panic!("{name}（{input:?}）：Rust 给的是 {got:?}，Python 给的是 ok={ok:?} err={err:?} raise={raise:?}")
                }
            }
        }
        assert!(ok > 300 && refused > 600 && raised > 30, "{ok} {refused} {raised}");
    }

    #[test]
    fn node_clean_name_matches_python_vectors() {
        let cases = vectors().list("node_clean_name").unwrap().items();
        assert!(cases.len() > 20);
        for c in cases {
            let c = c.as_dict().unwrap();
            let input = c.at("in");
            assert_eq!(node_clean_name(Some(&Y::V(input.clone()))), &*c.str("out").unwrap(), "{input:?}");
        }
        assert_eq!(node_clean_name(None), "");
    }

    #[test]
    fn keys_that_are_not_text() {
        // _y_scalar 给的键照 Python 的 == 认：1 和 true 是同一个（留先来的那个键），1 和 "1" 不是——写成 JSON 就有两个 "1"
        let p = node_parse("{1: a, \"1\": b, true: c, type: ss, server: s, port: 5}").unwrap();
        assert_eq!(p.dumps(), r#"{"1": "c", "1": "b", "type": "ss", "server": "s", "port": 5}"#);
        let p = node_parse("type: ss\nserver: s\nport: 1\n~: a\nnull: b\nfalse: c\n0: d").unwrap();
        assert_eq!(p.dumps(), r#"{"type": "ss", "server": "s", "port": 1, "null": "b", "false": "d"}"#);
    }
}
