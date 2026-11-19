//! 字符的分类：占几格、能不能打印、算不算空白和数字。表在 uni_tables.rs（由 tools/gen_unicode.py 从 Python 的
//! unicodedata 生成），所以和原来 Python 实现的判断一样；不用 Rust 标准库和别的库里的同名判断（范围不一样）。

#[path = "uni_tables.rs"]
mod tables;

fn in_table(t: &[(u32, u32)], c: char) -> bool {
    let c = c as u32;
    t.binary_search_by(|&(a, b)| {
        if c < a {
            std::cmp::Ordering::Greater
        } else if c > b {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Equal
        }
    })
    .is_ok()
}

/// 终端里占两格（East Asian Width 是 W 或 F）。别的一律算一格，组合字符、零宽字符也是：原来就这么算
pub fn is_wide(c: char) -> bool {
    (c as u32) >= 0x1100 && in_table(tables::WIDE, c)
}

/// `c.isprintable()`
pub fn is_printable(c: char) -> bool {
    match c as u32 {
        0x20..=0x7e => true,
        0..=0x1f | 0x7f => false,
        _ => !in_table(tables::NONPRINT, c),
    }
}

/// `c.isspace()`
pub fn is_space(c: char) -> bool {
    match c as u32 {
        0x20 | 0x09..=0x0d | 0x1c..=0x1f => true,
        0..=0x7f => false,
        _ => in_table(tables::SPACE, c),
    }
}

/// `c.isdigit()`
pub fn is_digit(c: char) -> bool {
    c.is_ascii_digit() || (!c.is_ascii() && in_table(tables::DIGIT, c))
}

/// `c.isdecimal()`
pub fn is_decimal(c: char) -> bool {
    c.is_ascii_digit() || (!c.is_ascii() && in_table(tables::DECIMAL, c))
}

/// 十进制数字的值（`int()` 认的那些）：各种文字的数字都是从 0 起连着的十个
pub fn decimal_value(c: char) -> Option<u32> {
    if c.is_ascii_digit() {
        return Some(c as u32 - '0' as u32);
    }
    let x = c as u32;
    tables::DECIMAL.iter().find(|&&(a, b)| a <= x && x <= b).map(|&(a, _)| (x - a) % 10)
}

/// `c.isalnum()`
pub fn is_alnum(c: char) -> bool {
    c.is_ascii_alphanumeric() || (!c.is_ascii() && in_table(tables::ALNUM, c))
}

/// 一段文字在终端里占几格（原来的 `dw`）
pub fn width(s: &str) -> usize {
    s.chars().map(|c| if is_wide(c) { 2 } else { 1 }).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths() {
        assert_eq!(width("abc"), 3);
        assert_eq!(width("中文"), 4);
        assert_eq!(width("ａ"), 2); // 全角字母
        assert_eq!(width("e\u{301}"), 2); // 组合字符照样算一格
        assert_eq!(width("…"), 1);
        assert_eq!(width("😀"), 2);
    }

    #[test]
    fn classes() {
        assert!(is_printable('中') && is_printable(' ') && !is_printable('\u{a0}') && !is_printable('\u{200b}'));
        assert!(!is_printable('\n') && !is_printable('\u{7f}') && !is_printable('\u{ad}'));
        assert!(is_space('\u{1c}') && is_space('\u{3000}') && is_space('\u{85}') && !is_space('\u{200b}'));
        assert!(is_digit('²') && !is_decimal('²') && is_decimal('٣'));
        assert_eq!(decimal_value('٣'), Some(3));
        assert_eq!(decimal_value('９'), Some(9));
        assert!(is_alnum('中') && is_alnum('½') && !is_alnum('_') && !is_alnum('-'));
    }
}
