//! `urllib.parse`：`urlsplit`、`parse_qsl`、`quote`、`unquote`，规则是 CPython 3.14 的（对照数据在
//! tests/vectors/urlsplit.json、urlquery.json）。
//!
//! 登录网址、接口地址、邮件里的链接都靠它核对，所以不用现成的 URL 库：那些库照浏览器的规则归一化
//! （改大小写、补斜杠、转码），和 Python 宽松的切法不是一回事，核对的松紧会跟着变。

use super::ipaddr;
use super::obj::repr_str;
use unicode_normalization::UnicodeNormalization;

/// `urlsplit()` 的结果（`SplitResult`）
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Url {
    pub scheme: String,
    pub netloc: String,
    pub path: String,
    pub query: String,
    pub fragment: String,
}

/// `urlsplit(url)`：方括号不成对、方括号里不是 IPv6、主机名规范化后多出分隔符是 ValueError（这里给出它的说法）
pub fn urlsplit(url: &str) -> Result<Url, String> {
    // 开头的空格和控制字符去掉（结尾的留着），哪里的制表符、回车、换行都去掉
    let mut url: String = url.trim_start_matches(|c: char| c <= ' ').chars().filter(|c| !matches!(c, '\t' | '\r' | '\n')).collect();
    let mut out = Url::default();
    if let Some(i) = url.find(':') {
        let head = &url[..i];
        let scheme_ok = i > 0
            && head.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && head.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        if scheme_ok {
            out.scheme = head.to_ascii_lowercase();
            url = url[i + 1..].to_string();
        }
    }
    if url.starts_with("//") {
        let end = url[2..].find(['/', '?', '#']).map_or(url.len(), |i| i + 2);
        out.netloc = url[2..end].to_string();
        url = url[end..].to_string();
        let (open, close) = (out.netloc.contains('['), out.netloc.contains(']'));
        if open != close {
            return Err("Invalid IPv6 URL".into());
        }
        if open {
            check_bracketed_netloc(&out.netloc)?;
        }
    }
    if let Some((u, f)) = url.split_once('#') {
        out.fragment = f.to_string();
        url = u.to_string();
    }
    if let Some((u, q)) = url.split_once('?') {
        out.query = q.to_string();
        url = u.to_string();
    }
    check_netloc(&out.netloc)?;
    out.path = url;
    Ok(out)
}

/// 方括号前面、方括号和端口之间不许有东西；方括号里是 IPv6 或 `v数字.…`（IPvFuture）
fn check_bracketed_netloc(netloc: &str) -> Result<(), String> {
    let hostport = netloc.rsplit_once('@').map_or(netloc, |x| x.1);
    let host = match hostport.split_once('[') {
        Some((before, bracketed)) => {
            if !before.is_empty() {
                return Err("Invalid IPv6 URL".into());
            }
            let (host, after) = bracketed.split_once(']').unwrap_or((bracketed, ""));
            if !after.is_empty() && !after.starts_with(':') {
                return Err("Invalid IPv6 URL".into());
            }
            host
        }
        None => hostport.split_once(':').map_or(hostport, |x| x.0),
    };
    if let Some(rest) = host.strip_prefix('v') {
        // \Av[a-fA-F0-9]+\..+\z
        let ok = rest.split_once('.').is_some_and(|(hex, tail)| !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()) && !tail.is_empty() && !tail.contains('\n'));
        return if ok { Ok(()) } else { Err("IPvFuture address is invalid".into()) };
    }
    match ipaddr::ip_address(host)?.ip {
        ipaddr::Ip::V4(_) => Err("An IPv4 address cannot be in brackets".into()),
        ipaddr::Ip::V6(_) => Ok(()),
    }
}

/// 主机名里有 ℀ 这类字符时，照 NFKC 规范化会变出 `/?#@:`，等于偷偷改了网址的结构：不收
fn check_netloc(netloc: &str) -> Result<(), String> {
    if netloc.is_ascii() {
        return Ok(());
    }
    let n: String = netloc.chars().filter(|c| !matches!(c, '@' | ':' | '#' | '?')).collect();
    let n2: String = n.nfkc().collect();
    if n != n2 && n2.contains(['/', '?', '#', '@', ':']) {
        return Err(format!("netloc {} contains invalid characters under NFKC normalization", repr_str(netloc)));
    }
    Ok(())
}

impl Url {
    fn userinfo(&self) -> (Option<&str>, Option<&str>) {
        match self.netloc.rsplit_once('@') {
            None => (None, None),
            Some((info, _)) => match info.split_once(':') {
                Some((u, p)) => (Some(u), Some(p)),
                None => (Some(info), None),
            },
        }
    }

    /// (主机, 端口的文字)
    fn hostinfo(&self) -> (&str, Option<&str>) {
        let hostinfo = self.netloc.rsplit_once('@').map_or(self.netloc.as_str(), |x| x.1);
        let (host, port) = match hostinfo.split_once('[') {
            Some((_, bracketed)) => {
                let (host, after) = bracketed.split_once(']').unwrap_or((bracketed, ""));
                (host, after.split_once(':').map_or("", |x| x.1))
            }
            None => hostinfo.split_once(':').unwrap_or((hostinfo, "")),
        };
        (host, if port.is_empty() { None } else { Some(port) })
    }

    /// `p.username`
    pub fn username(&self) -> Option<&str> {
        self.userinfo().0
    }

    /// `p.password`
    pub fn password(&self) -> Option<&str> {
        self.userinfo().1
    }

    /// `p.hostname`：小写；IPv6 的区域标识（`%` 后面）原样
    pub fn hostname(&self) -> Option<String> {
        let (host, _) = self.hostinfo();
        if host.is_empty() {
            return None;
        }
        Some(match host.split_once('%') {
            Some((h, zone)) => format!("{}%{}", h.to_lowercase(), zone),
            None => host.to_lowercase(),
        })
    }

    /// `p.port`：不是 0–65535 的十进制数是 ValueError（这里给出它的说法）
    pub fn port(&self) -> Result<Option<u16>, String> {
        let Some(port) = self.hostinfo().1 else {
            return Ok(None);
        };
        if !port.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("Port could not be cast to integer value as {}", repr_str(port)));
        }
        match port.parse::<u32>() {
            Ok(n) if n <= 65535 => Ok(Some(n as u16)),
            _ => Err("Port out of range 0-65535".into()),
        }
    }
}

/// 把 `%XX` 解成字节（不成对的十六进制原样留着）
fn unquote_bytes(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            if let (Some(h), Some(l)) = (b.get(i + 1).and_then(|c| (*c as char).to_digit(16)), b.get(i + 2).and_then(|c| (*c as char).to_digit(16))) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// `unquote(s)`：解出来不是 UTF-8 的字节换成 U+FFFD
pub fn unquote(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    String::from_utf8_lossy(&unquote_bytes(s)).into_owned()
}

/// `quote(s, safe=…)`：字母、数字、`_.-~` 和 safe 里的字符之外都写成 `%XX`（大写）
pub fn quote(s: &str, safe: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) || (b.is_ascii() && safe.as_bytes().contains(&b)) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `parse_qsl(qs, keep_blank_values=…)`：只按 `&` 切（分号不算），`+` 是空格
pub fn parse_qsl(qs: &str, keep_blank_values: bool) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for pair in qs.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (name, value) = match pair.split_once('=') {
            Some(nv) => nv,
            None if keep_blank_values => (pair, ""),
            None => continue,
        };
        if !value.is_empty() || keep_blank_values {
            out.push((unquote(&name.replace('+', " ")), unquote(&value.replace('+', " "))));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::py::json;
    use crate::py::obj::{Dict, V};

    fn opt(d: &Dict, k: &str) -> Option<String> {
        d.str(k).map(|s| s.to_string())
    }

    #[test]
    fn urlsplit_matches_cpython_vectors() {
        let doc = json::loads(include_str!("../../tests/vectors/urlsplit.json")).unwrap();
        let cases = doc.as_dict().unwrap().list("cases").unwrap().items();
        assert!(cases.len() > 60);
        for c in cases {
            let c = c.as_dict().unwrap();
            let input = c.str("in").unwrap();
            let out = c.dict("out").unwrap();
            match (urlsplit(&input), out.dict("ok"), out.str("err")) {
                (Ok(u), Some(want), _) => {
                    for (got, k) in [(&u.scheme, "scheme"), (&u.netloc, "netloc"), (&u.path, "path"), (&u.query, "query"), (&u.fragment, "fragment")] {
                        assert_eq!(got.as_str(), &*want.str(k).unwrap(), "{input:?} {k}");
                    }
                    assert_eq!(u.username().map(String::from), opt(&want, "username"), "{input:?} username");
                    assert_eq!(u.password().map(String::from), opt(&want, "password"), "{input:?} password");
                    assert_eq!(u.hostname(), opt(&want, "hostname"), "{input:?} hostname");
                    let port = want.dict("port").unwrap();
                    match (u.port(), port.get("ok"), port.str("err")) {
                        (Ok(None), Some(V::None), _) => {}
                        (Ok(Some(p)), Some(V::Int(w)), _) => assert_eq!(p as i64, w, "{input:?} port"),
                        (Err(e), None, Some(w)) => assert_eq!(e, &*w, "{input:?} port"),
                        (got, _, _) => panic!("{input:?} 的端口：Rust {got:?}，Python {port:?}"),
                    }
                }
                (Err(e), None, Some(want)) => assert_eq!(e, &*want, "{input:?}"),
                (got, _, _) => panic!("{input:?}：Rust 给的是 {got:?}，Python 给的是 {out:?}"),
            }
        }
    }

    #[test]
    fn query_helpers_match_cpython_vectors() {
        let doc = json::loads(include_str!("../../tests/vectors/urlquery.json")).unwrap();
        let cases = doc.as_dict().unwrap().dict("cases").unwrap();
        let pairs = |v: V| -> Vec<(String, String)> {
            v.as_list().unwrap().items().iter().map(|p| {
                let p = p.as_list().unwrap();
                (p.at(0).py_str(), p.at(1).py_str())
            }).collect()
        };
        for c in cases.list("parse_qsl").unwrap().items() {
            let c = c.as_dict().unwrap();
            let q = c.str("in").unwrap();
            assert_eq!(parse_qsl(&q, false), pairs(c.at("out")), "{q:?}");
            assert_eq!(parse_qsl(&q, true), pairs(c.at("keep")), "{q:?} keep");
        }
        for c in cases.list("quote").unwrap().items() {
            let c = c.as_dict().unwrap();
            let s = c.str("in").unwrap();
            assert_eq!(quote(&s, "/"), &*c.str("quote").unwrap(), "{s:?}");
            assert_eq!(quote(&s, ""), &*c.str("quote_safe_empty").unwrap(), "{s:?}");
            assert_eq!(unquote(&s), &*c.str("unquote").unwrap(), "{s:?}");
        }
    }
}
