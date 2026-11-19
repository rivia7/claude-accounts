//! 用量（`claude_usage_query` 到 `cmd_reset_done`）：usage-query、login-plan-forget、usage-render、reset-plan、reset-done，
//! 以及时间的写法、套餐的叫法和档位。
//!
//! 子命令还没移植；`parse_ts`（检查更新也要用）已经移植。

use super::Cmd;
use crate::py::obj::V;
use crate::py::text;
use crate::py::time::timegm;
use crate::py::{Fail, R};
use crate::re;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[];

/// ISO 时间（小数秒、Z、时区都行）或秒/毫秒数 → 秒；看不懂返回 None。月份不在 1–12、年份是 0 时 Python 的 calendar.timegm
/// 报 ValueError（没人接），这里也往上传
pub fn parse_ts(s: &V) -> R<Option<f64>> {
    let s = match s {
        V::Bool(_) => return Ok(None),
        V::Int(_) | V::Big(_) | V::Float(_) => {
            let n = s.as_num().unwrap_or(f64::INFINITY);
            return Ok(Some(if n > 1e11 { n / 1000.0 } else { n }));
        }
        V::Str(s) => s,
        _ => return Ok(None),
    };
    let Some(m) = re!(r"^(\d{4})-(\d\d)-(\d\d)[T ](\d\d):(\d\d)(?::(\d\d)(\.\d+)?)?\s*(Z|[+-]\d\d:?\d\d)?$").match_(text::strip(s)) else {
        return Ok(None);
    };
    let int = |i: usize| text::to_int(m.g(i)).unwrap_or(0);
    let (y, mo) = (int(1), int(2));
    if !(1..=9999).contains(&y) {
        return Err(Fail::Value(format!("year must be in 1..9999, not {y}")));
    }
    if !(1..=12).contains(&mo) {
        return Err(Fail::Value(format!("month must be in 1..12, not {mo}")));
    }
    let frac = m.group(7).and_then(text::to_float).unwrap_or(0.0);
    let mut ts = timegm(y, mo, int(3), int(4), int(5), m.group(6).map_or(0, |_| int(6))) as f64 + frac;
    if let Some(tz) = m.group(8).filter(|tz| *tz != "Z") {
        let z = tz.replace(':', "");
        let sign = if z.starts_with('+') { 1.0 } else { -1.0 };
        let digits: Vec<i64> = z.chars().skip(1).map(|c| crate::py::uni::decimal_value(c).unwrap_or(0) as i64).collect();
        ts -= sign * ((digits[0] * 10 + digits[1]) * 3600 + (digits[2] * 10 + digits[3]) * 60) as f64;
    }
    Ok(Some(ts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ts_matches_python() {
        // 期望值是 CPython 3.14 算的
        let p = |s: &str| parse_ts(&V::from(s)).unwrap();
        assert_eq!(p("2026-11-19T08:00:00Z"), Some(1795075200.0));
        assert_eq!(p("2026-11-19 08:00"), Some(1795075200.0));
        assert_eq!(p(" 2026-11-19T08:00:00.25+08:00 "), Some(1795046400.25));
        assert_eq!(p("2026-11-19T08:00:00-0530"), Some(1795095000.0));
        assert_eq!(p("2026-02-31T25:61:00Z"), Some(1772589660.0)); // 日、时、分不核对，照样往后算
        assert_eq!(p("2026-11-19"), None);
        assert_eq!(p("2026-11-19T08:00:00 UTC"), None);
        assert!(parse_ts(&V::from("2026-13-01T00:00:00Z")).is_err() && parse_ts(&V::from("0000-01-01T00:00:00Z")).is_err());
        assert_eq!(parse_ts(&V::Int(1795075200)).unwrap(), Some(1795075200.0));
        assert_eq!(parse_ts(&V::Int(1795075200123)).unwrap(), Some(1795075200.123));
        assert_eq!(parse_ts(&V::Float(1.5)).unwrap(), Some(1.5));
        assert_eq!(parse_ts(&V::Bool(true)).unwrap(), None);
        assert_eq!(parse_ts(&V::None).unwrap(), None);
    }
}
