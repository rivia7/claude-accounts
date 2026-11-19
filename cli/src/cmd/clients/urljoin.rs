//! `urllib.parse.urljoin`（CPython 3.14）：下载时手动跟着跳转，`Location` 可能是相对地址。兼容层的 `py::url` 没有它
//! （那里的 `urlsplit` 不分“没有这一段”和“这一段是空的”，urljoin 要分），先放在这里。对照数据在 tests/vectors/clients.json。

use crate::py::url::urlsplit;
use crate::py::{Fail, R};

const USES_RELATIVE: [&str; 20] =
    ["", "ftp", "http", "gopher", "nntp", "imap", "wais", "file", "https", "shttp", "mms", "prospero", "rtsp", "rtsps", "rtspu", "sftp", "svn", "svn+ssh", "ws", "wss"];
const USES_NETLOC: [&str; 27] = [
    "", "ftp", "http", "gopher", "nntp", "telnet", "imap", "wais", "file", "mms", "https", "shttp", "snews", "prospero", "rtsp", "rtsps", "rtspu", "rsync", "svn",
    "svn+ssh", "sftp", "nfs", "git", "git+ssh", "ws", "wss", "itms-services",
];

/// `_urlsplit(url, None)` 的结果：没有的那一段是 None
struct Parts {
    scheme: Option<String>,
    netloc: Option<String>,
    path: String,
    query: Option<String>,
    fragment: Option<String>,
}

/// `_urlsplit(url, None, True)`：方括号不成对、方括号里不是 IPv6、主机名规范化后多出分隔符是 ValueError
fn split(url: &str) -> R<Parts> {
    // 会报错的那几种和 urlsplit 是同一套检查，交给兼容层那一份
    urlsplit(url).map_err(Fail::Value)?;
    // 开头的空格和控制字符去掉（结尾的留着），哪里的制表符、回车、换行都去掉
    let url: String = url.trim_start_matches(|c: char| c <= ' ').chars().filter(|c| !matches!(c, '\t' | '\r' | '\n')).collect();
    let mut rest = url.as_str();
    let mut scheme = None;
    if let Some(i) = rest.find(':') {
        let head = &rest[..i];
        let first_ok = head.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
        if i > 0 && first_ok && head.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
            scheme = Some(head.to_lowercase());
            rest = &rest[i + 1..];
        }
    }
    let mut netloc = None;
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        netloc = Some(after[..end].to_string());
        rest = &after[end..];
    }
    let (rest, fragment) = match rest.split_once('#') {
        Some((a, b)) => (a, Some(b.to_string())),
        None => (rest, None),
    };
    let (path, query) = match rest.split_once('?') {
        Some((a, b)) => (a, Some(b.to_string())),
        None => (rest, None),
    };
    Ok(Parts { scheme, netloc, path: path.to_string(), query, fragment })
}

/// `_urlunsplit(scheme, netloc, url, query, fragment)`
fn unsplit(scheme: Option<&str>, netloc: Option<&str>, path: &str, query: Option<&str>, fragment: Option<&str>) -> String {
    let mut url = path.to_string();
    if let Some(netloc) = netloc {
        if !url.is_empty() && !url.starts_with('/') {
            url.insert(0, '/');
        }
        url = format!("//{netloc}{url}");
    } else if url.starts_with("//") {
        url = format!("//{url}");
    }
    if let Some(scheme) = scheme.filter(|s| !s.is_empty()) {
        url = format!("{scheme}:{url}");
    }
    if let Some(q) = query {
        url = format!("{url}?{q}");
    }
    if let Some(f) = fragment {
        url = format!("{url}#{f}");
    }
    url
}

/// `urljoin(base, url)`
pub fn urljoin(base: &str, url: &str) -> R<String> {
    if base.is_empty() {
        return Ok(url.to_string());
    }
    if url.is_empty() {
        return Ok(base.to_string());
    }
    let b = split(base)?;
    let u = split(url)?;
    let scheme = u.scheme.clone().or_else(|| b.scheme.clone());
    if scheme != b.scheme || scheme.as_deref().is_some_and(|s| !s.is_empty() && !USES_RELATIVE.contains(&s)) {
        return Ok(url.to_string());
    }
    let s = scheme.as_deref();
    let mut netloc = u.netloc.clone();
    if s.is_none_or(|s| s.is_empty() || USES_NETLOC.contains(&s)) {
        if u.netloc.as_deref().is_some_and(|n| !n.is_empty()) {
            return Ok(unsplit(s, u.netloc.as_deref(), &u.path, u.query.as_deref(), u.fragment.as_deref()));
        }
        netloc = b.netloc.clone();
    }
    if u.path.is_empty() {
        let (mut query, mut fragment) = (u.query.clone(), u.fragment.clone());
        if query.is_none() {
            query = b.query.clone();
            if fragment.is_none() {
                fragment = b.fragment.clone();
            }
        }
        return Ok(unsplit(s, netloc.as_deref(), &b.path, query.as_deref(), fragment.as_deref()));
    }
    let mut base_parts: Vec<&str> = b.path.split('/').collect();
    if base_parts.last().is_some_and(|last| !last.is_empty()) {
        // 最后一段不是目录：解析相对路径时不算它
        base_parts.pop();
    }
    // RFC 3986：以 / 开头的不看 base 的路径
    let segments: Vec<&str> = if u.path.starts_with('/') {
        u.path.split('/').collect()
    } else {
        let mut all = base_parts;
        all.extend(u.path.split('/'));
        // segments[1:-1] = filter(None, segments[1:-1])：去掉中间会拼出连着的斜杠的空段
        let n = all.len();
        if n > 2 {
            let middle: Vec<&str> = all[1..n - 1].iter().copied().filter(|seg| !seg.is_empty()).collect();
            let (first, last) = (all[0], all[n - 1]);
            all = vec![first];
            all.extend(middle);
            all.push(last);
        }
        all
    };
    let mut resolved: Vec<&str> = Vec::new();
    for seg in &segments {
        match *seg {
            ".." => {
                resolved.pop();
            }
            "." => {}
            other => resolved.push(other),
        }
    }
    if matches!(segments.last(), Some(&".") | Some(&"..")) {
        // 最后一段是相对目录：补上结尾的 /
        resolved.push("");
    }
    let joined = resolved.join("/");
    Ok(unsplit(s, netloc.as_deref(), if joined.is_empty() { "/" } else { &joined }, u.query.as_deref(), u.fragment.as_deref()))
}
