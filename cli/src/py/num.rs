//! 数字：小数怎么写成文字、取整。和 Python 逐字一致的地方都在这里，别处不要自己 `format!` 小数。

/// `repr(小数)`：最短的、读回来还是同一个数的写法；小于 1e-4 或不小于 1e16 时写成 `1e-05`、`1e+16`
pub fn float_repr(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    // Rust 的 {:e} 给的就是最短的那串数字：「[-]d[.ddd]e[-]N」
    let s = format!("{:e}", f);
    let (mant, exp) = s.split_once('e').expect("{:e} 总带着 e");
    let exp: i32 = exp.parse().expect("指数是整数");
    let (neg, mant) = match mant.strip_prefix('-') {
        Some(m) => (true, m),
        None => (false, mant),
    };
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let decpt = exp + 1; // 值 = 0.digits × 10^decpt
    let n = digits.len() as i32;
    let body = if decpt <= -4 || decpt > 16 {
        let e = decpt - 1;
        let frac = if n > 1 { format!(".{}", &digits[1..]) } else { String::new() };
        format!("{}{}e{}{:02}", &digits[..1], frac, if e < 0 { '-' } else { '+' }, e.abs())
    } else if decpt <= 0 {
        format!("0.{}{}", "0".repeat((-decpt) as usize), digits)
    } else if decpt >= n {
        format!("{}{}.0", digits, "0".repeat((decpt - n) as usize))
    } else {
        format!("{}.{}", &digits[..decpt as usize], &digits[decpt as usize..])
    };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

/// `round(x)`：四舍六入五成双（Python 3 的 round；0.5 → 0，1.5 → 2，2.5 → 2）
pub fn round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// `round(x, n)`：按这个小数真正的二进制值取到 n 位小数（`round(2.675, 2)` 是 2.67）
pub fn round_to(x: f64, n: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{:.*}", n, x).parse().expect("刚写出来的小数")
}

/// `int(x)`：朝 0 截断
pub fn trunc(x: f64) -> i64 {
    x.trunc() as i64
}

/// `int(x + 0.5)`：代码里故意避开 round 的那种四舍五入（只用在不小于 0 的数上）
pub fn half_up(x: f64) -> i64 {
    (x + 0.5).trunc() as i64
}

/// `a // b`（向下取整）
pub fn floordiv(a: i64, b: i64) -> i64 {
    let q = a / b;
    if (a % b != 0) && ((a < 0) != (b < 0)) {
        q - 1
    } else {
        q
    }
}

/// `a % b`（结果和 b 同号）
pub fn pymod(a: i64, b: i64) -> i64 {
    let r = a % b;
    if r != 0 && ((r < 0) != (b < 0)) {
        r + b
    } else {
        r
    }
}

/// `-(-a // b) * b`：向上取到 b 的整数倍
pub fn ceil_to(a: i64, b: i64) -> i64 {
    -floordiv(-a, b) * b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_repr_matches_python() {
        // 右边是 CPython 的 repr()
        for (f, want) in [
            (1.0, "1.0"),
            (1.5, "1.5"),
            (0.1, "0.1"),
            (100.0, "100.0"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e+16"),
            (1.5e16, "1.5e+16"),
            (123456789012345680.0, "1.2345678901234568e+17"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (0.000015, "1.5e-05"),
            (1e-7, "1e-07"),
            (1e100, "1e+100"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            (5e-324, "5e-324"),
            (-2.5, "-2.5"),
            (-1e-5, "-1e-05"),
            (1731999999.123456, "1731999999.123456"),
            (0.30000000000000004, "0.30000000000000004"),
            (9999999999999998.0, "9999999999999998.0"),
            (0.0, "0.0"),
            (-0.0, "-0.0"),
        ] {
            assert_eq!(float_repr(f), want, "{f:e}");
        }
        assert_eq!(float_repr(f64::NAN), "nan");
        assert_eq!(float_repr(f64::NEG_INFINITY), "-inf");
    }

    #[test]
    fn rounding_matches_python() {
        assert_eq!([round(0.5), round(1.5), round(2.5), round(-0.5), round(-1.5), round(2.4999)], [0, 2, 2, 0, -2, 2]);
        assert_eq!(round_to(2.675, 2), 2.67);
        assert_eq!(round_to(1.25, 1), 1.2);
        assert_eq!(round_to(0.125, 2), 0.12);
        assert_eq!(round_to(123.456, 0), 123.0);
        assert_eq!([half_up(0.5), half_up(1.5), half_up(2.5), half_up(2.49)], [1, 2, 3, 2]);
        assert_eq!([trunc(2.9), trunc(-2.9)], [2, -2]);
    }

    #[test]
    fn floor_division_like_python() {
        assert_eq!([floordiv(7, 2), floordiv(-7, 2), floordiv(7, -2), floordiv(-8, 2)], [3, -4, -4, -4]);
        assert_eq!([pymod(7, 3), pymod(-7, 3), pymod(7, -3)], [1, 2, -2]);
        assert_eq!([ceil_to(0, 8), ceil_to(1, 8), ceil_to(8, 8), ceil_to(9, 8)], [0, 8, 8, 16]);
    }
}
