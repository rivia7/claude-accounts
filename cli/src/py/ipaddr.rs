//! `ipaddress`：认 IP 地址、是不是私有 / 本机 / 公网地址、所在的网段。规则是 CPython 3.14 的
//! （私有网段的名单各版本改过几次），对照数据在 tests/vectors/ipaddress.json。

use std::net::{Ipv4Addr, Ipv6Addr};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ip {
    V4(Ipv4Addr),
    V6(Ipv6Addr),
}

/// 带着区域标识的 IPv6 地址解析出来的结果
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub ip: Ip,
    /// `fe80::1%eth0` 里的 `eth0`
    pub scope: Option<String>,
}

/// `ipaddress.ip_address(s)`：认不出是 ValueError，这里给出它的说法
pub fn ip_address(s: &str) -> Result<Parsed, String> {
    let bad = || format!("{} does not appear to be an IPv4 or IPv6 address", super::obj::repr_str(s));
    if let Some(v4) = parse_v4(s) {
        return Ok(Parsed { ip: Ip::V4(v4), scope: None });
    }
    let (addr, scope) = match s.split_once('%') {
        Some((a, z)) if !z.is_empty() && !z.contains('%') => (a, Some(z.to_string())),
        Some(_) => return Err(bad()),
        None => (s, None),
    };
    // 标准库的解析和 Python 认的写法一样（只有区域标识要自己切）；ASCII 之外的、带空白的都不认
    if !addr.is_ascii() || addr.is_empty() {
        return Err(bad());
    }
    match addr.parse::<Ipv6Addr>() {
        Ok(v6) => Ok(Parsed { ip: Ip::V6(v6), scope }),
        Err(_) => Err(bad()),
    }
}

/// 四段十进制，每段 1–3 位、不许有多余的前导 0、不超过 255
fn parse_v4(s: &str) -> Option<Ipv4Addr> {
    let mut parts = [0u8; 4];
    let mut n = 0;
    for p in s.split('.') {
        if n == 4 || p.is_empty() || p.len() > 3 || !p.bytes().all(|b| b.is_ascii_digit()) || (p.len() > 1 && p.starts_with('0')) {
            return None;
        }
        parts[n] = p.parse::<u16>().ok().filter(|v| *v <= 255)? as u8;
        n += 1;
    }
    (n == 4).then(|| Ipv4Addr::from(parts))
}

fn in4(ip: Ipv4Addr, net: [u8; 4], bits: u32) -> bool {
    let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
    (u32::from(ip) & mask) == (u32::from(Ipv4Addr::from(net)) & mask)
}

fn in6(ip: Ipv6Addr, net: &str, bits: u32) -> bool {
    let net: Ipv6Addr = net.parse().expect("写在代码里的网段");
    let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
    (u128::from(ip) & mask) == (u128::from(net) & mask)
}

impl Ip {
    pub fn version(&self) -> u8 {
        match self {
            Ip::V4(_) => 4,
            Ip::V6(_) => 6,
        }
    }

    /// `ip.is_private`：IANA 登记的非全球可达的网段，去掉其中几个例外
    pub fn is_private(&self) -> bool {
        match *self {
            Ip::V4(ip) => {
                let nets: [([u8; 4], u32); 14] = [
                    ([0, 0, 0, 0], 8),
                    ([10, 0, 0, 0], 8),
                    ([127, 0, 0, 0], 8),
                    ([169, 254, 0, 0], 16),
                    ([172, 16, 0, 0], 12),
                    ([192, 0, 0, 0], 24),
                    ([192, 0, 0, 170], 31),
                    ([192, 0, 2, 0], 24),
                    ([192, 168, 0, 0], 16),
                    ([198, 18, 0, 0], 15),
                    ([198, 51, 100, 0], 24),
                    ([203, 0, 113, 0], 24),
                    ([240, 0, 0, 0], 4),
                    ([255, 255, 255, 255], 32),
                ];
                nets.iter().any(|&(n, b)| in4(ip, n, b)) && !in4(ip, [192, 0, 0, 9], 32) && !in4(ip, [192, 0, 0, 10], 32)
            }
            Ip::V6(ip) => {
                if let Some(v4) = ip.to_ipv4_mapped() {
                    return Ip::V4(v4).is_private();
                }
                let nets = [
                    ("::1", 128),
                    ("::", 128),
                    ("::ffff:0:0", 96),
                    ("64:ff9b:1::", 48),
                    ("100::", 64),
                    ("2001::", 23),
                    ("2001:db8::", 32),
                    ("2002::", 16),
                    ("3fff::", 20),
                    ("fc00::", 7),
                    ("fe80::", 10),
                ];
                let except = [("2001:1::1", 128), ("2001:1::2", 128), ("2001:3::", 32), ("2001:4:112::", 48), ("2001:20::", 28), ("2001:30::", 28)];
                nets.iter().any(|&(n, b)| in6(ip, n, b)) && !except.iter().any(|&(n, b)| in6(ip, n, b))
            }
        }
    }

    /// `ip.is_loopback`
    pub fn is_loopback(&self) -> bool {
        match *self {
            Ip::V4(ip) => in4(ip, [127, 0, 0, 0], 8),
            Ip::V6(ip) => match ip.to_ipv4_mapped() {
                Some(v4) => Ip::V4(v4).is_loopback(),
                None => ip == Ipv6Addr::LOCALHOST,
            },
        }
    }

    /// `ip.is_global`
    pub fn is_global(&self) -> bool {
        match *self {
            Ip::V4(ip) => !in4(ip, [100, 64, 0, 0], 10) && !self.is_private(),
            Ip::V6(ip) => match ip.to_ipv4_mapped() {
                Some(v4) => Ip::V4(v4).is_global(),
                None => !self.is_private(),
            },
        }
    }

    /// `str(ipaddress.ip_network(str(ip) + '/前缀长度', strict=False))`
    pub fn network(&self, bits: u32) -> String {
        match *self {
            Ip::V4(ip) => {
                let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
                format!("{}/{}", Ipv4Addr::from(u32::from(ip) & mask), bits)
            }
            Ip::V6(ip) => {
                let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
                format!("{}/{}", v6_text(Ipv6Addr::from(u128::from(ip) & mask)), bits)
            }
        }
    }
}

/// `str(IPv6Address)`：最长的一串 0（至少两段）缩成 `::`，映射的 IPv4 写成 `::ffff:a.b.c.d`
fn v6_text(ip: Ipv6Addr) -> String {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return format!("::ffff:{v4}");
    }
    let seg = ip.segments();
    // 找最长的一串 0（一样长取靠前的），只有一段的不缩
    let (mut best, mut best_len, mut i) = (0, 0, 0);
    while i < 8 {
        if seg[i] != 0 {
            i += 1;
            continue;
        }
        let start = i;
        while i < 8 && seg[i] == 0 {
            i += 1;
        }
        if i - start > best_len {
            (best, best_len) = (start, i - start);
        }
    }
    let hex = |s: &[u16]| s.iter().map(|x| format!("{x:x}")).collect::<Vec<_>>().join(":");
    if best_len < 2 {
        return hex(&seg);
    }
    format!("{}::{}", hex(&seg[..best]), hex(&seg[best + best_len..]))
}

impl Parsed {
    /// `str(ip)`
    pub fn text(&self) -> String {
        match (&self.ip, &self.scope) {
            (Ip::V4(ip), _) => ip.to_string(),
            (Ip::V6(ip), Some(z)) => format!("{}%{}", v6_text(*ip), z),
            (Ip::V6(ip), None) => v6_text(*ip),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::py::json;

    #[test]
    fn matches_cpython_vectors() {
        let doc = json::loads(include_str!("../../tests/vectors/ipaddress.json")).unwrap();
        let cases = doc.as_dict().unwrap().list("cases").unwrap().items();
        assert!(cases.len() > 90);
        for c in cases {
            let c = c.as_dict().unwrap();
            let input = c.str("in").unwrap();
            let out = c.dict("out").unwrap();
            match (ip_address(&input), out.dict("ok"), out.str("err")) {
                (Ok(p), Some(want), _) => {
                    assert_eq!(p.ip.version() as i64, want.get("version").unwrap().as_int().unwrap(), "{input}");
                    assert_eq!(p.text(), &*want.str("str").unwrap(), "{input}");
                    assert_eq!(Some(p.ip.is_private()), want.get("private").unwrap().as_bool(), "{input} private");
                    assert_eq!(Some(p.ip.is_loopback()), want.get("loopback").unwrap().as_bool(), "{input} loopback");
                    assert_eq!(Some(p.ip.is_global()), want.get("global").unwrap().as_bool(), "{input} global");
                    match p.ip {
                        Ip::V4(_) => assert_eq!(p.ip.network(24), &*want.str("net24").unwrap(), "{input}"),
                        Ip::V6(_) => assert_eq!(p.ip.network(48), &*want.str("net48").unwrap(), "{input}"),
                    }
                }
                (Err(e), None, Some(want)) => assert_eq!(e, &*want, "{input}"),
                (got, _, _) => panic!("{input:?}：Rust 给的是 {got:?}，Python 给的是 {out:?}"),
            }
        }
    }
}
