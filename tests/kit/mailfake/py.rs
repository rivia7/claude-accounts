//! 照 Python 的语义写的几个小函数：假 Gmail 是照原来的 tests/mail_fake.py 一行行搬过来的，空白、拆行、整数、repr、真假、时间都要和 Python 一样
//! （辅助程序的 IMAP 客户端是照着原来那一份写的）。空白、拆行、repr、真假用 kit 里共用的那一份（kit::py），这里只有假 Gmail 另外要的：
//! 整数和小数怎么读、时间。

use std::time::{SystemTime, UNIX_EPOCH};

pub use crate::py::{isspace, py_str, repr_str, split, splitlines, strip, truthy};

/// s.isspace()：不是空串、每个字符都是空白
pub fn all_space(s: &str) -> bool {
    !s.is_empty() && s.chars().all(isspace)
}

/// str.lstrip()
pub fn lstrip(s: &str) -> &str {
    s.trim_start_matches(isspace)
}

/// str.upper()（整个 Unicode 的大小写对应，ß 变成 SS）
pub fn upper(s: &str) -> String {
    s.to_uppercase()
}

/// int(s)：前后可以有空白、可以带正负号、数字之间可以有单个下划线。只认 ASCII 的数字（Python 还认别的文字的数字，测试里用不到）；
/// 超过 4300 位是 Python 的 ValueError（错误信息照它）。放不进 i128 的按符号取最大、最小（只拿来和 UID 比大小）
pub fn int(s: &str) -> Result<i128, String> {
    let bad = || format!("invalid literal for int() with base 10: {}", repr_str(s));
    let t = strip(s);
    let (neg, body) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let bytes = body.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_digit() || !bytes[bytes.len() - 1].is_ascii_digit() {
        return Err(bad());
    }
    let mut digits = 0usize;
    let mut v: i128 = 0;
    let mut prev_us = false;
    for &b in bytes {
        if b == b'_' {
            if prev_us {
                return Err(bad());
            }
            prev_us = true;
            continue;
        }
        if !b.is_ascii_digit() {
            return Err(bad());
        }
        prev_us = false;
        digits += 1;
        v = v.saturating_mul(10).saturating_add(i128::from(b - b'0'));
    }
    if digits > 4300 {
        return Err(format!(
            "Exceeds the limit (4300 digits) for integer string conversion: value has {digits} digits; use sys.set_int_max_str_digits() to increase the limit"
        ));
    }
    Ok(if neg { -v } else { v })
}

/// float(s)：前后可以有空白；inf、nan、指数、数字之间的单个下划线都认
pub fn float(s: &str) -> Option<f64> {
    let t = strip(s);
    if !t.is_ascii() {
        return None;
    }
    let bytes = t.as_bytes();
    let mut clean = String::with_capacity(t.len());
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'_' {
            let ok = i > 0 && bytes[i - 1].is_ascii_digit() && bytes.get(i + 1).is_some_and(u8::is_ascii_digit);
            if !ok {
                return None;
            }
            continue;
        }
        clean.push(b as char);
    }
    // 标准库认 inf、infinity、nan（不分大小写）和指数，和 Python 一样
    clean.parse::<f64>().ok()
}

/// time.time()
pub fn now() -> f64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as f64 + f64::from(d.subsec_nanos()) * 1e-9,
        Err(e) => -(e.duration().as_secs_f64()),
    }
}

/// time.gmtime 的结果（月份 1–12，星期一是 0）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tm {
    pub year: i64,
    pub mon: u32,
    pub mday: u32,
    pub hour: u32,
    pub min: u32,
    pub sec: u32,
    pub wday: u32,
}

/// time.gmtime(t)：小数往下取整到秒
pub fn gmtime(t: f64) -> Tm {
    gmtime_secs(t.floor() as i64)
}

/// 从 1970-01-01 起的秒数 → 年月日时分秒（公历往前推，和 C 的 gmtime 一样）
pub fn gmtime_secs(secs: i64) -> Tm {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // Howard Hinnant 的 civil_from_days
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    Tm {
        year: y,
        mon: m as u32,
        mday: d as u32,
        hour: (rem / 3600) as u32,
        min: (rem % 3600 / 60) as u32,
        sec: (rem % 60) as u32,
        // 1970-01-01 是星期四
        wday: (days + 3).rem_euclid(7) as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_like_python() {
        assert!(all_space(" \t\x1c\u{3000}") && !all_space("") && !all_space(" x"));
        assert_eq!(lstrip("\x1c \tx "), "x ");
    }

    #[test]
    fn int_like_python() {
        assert_eq!(int(" 12 "), Ok(12));
        assert_eq!(int("+1_000"), Ok(1000));
        assert_eq!(int("-3"), Ok(-3));
        for bad in ["", "1__0", "_1", "1_", "x", "1.0", "- 1"] {
            assert!(int(bad).is_err(), "{bad}");
        }
        assert!(int(&"9".repeat(4301)).unwrap_err().starts_with("Exceeds the limit (4300 digits)"));
        assert_eq!(float(" 1e3 "), Some(1000.0));
        assert_eq!(float("1_5"), Some(15.0));
        assert!(float("-inf").is_some_and(|f| f.is_infinite()));
        assert_eq!(float("x"), None);
    }

    #[test]
    fn gmtime_like_c() {
        let g = gmtime(1763200800.9);
        assert_eq!((g.year, g.mon, g.mday, g.hour, g.min, g.sec, g.wday), (2025, 11, 15, 10, 0, 0, 5));
        let g = gmtime(-1.5);
        assert_eq!((g.year, g.mon, g.mday, g.hour, g.min, g.sec), (1969, 12, 31, 23, 59, 58));
        assert_eq!(gmtime_secs(951782400).mday, 29); // 2000-02-29
    }
}
