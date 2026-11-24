//! 从仓库里的 cca 脚本切出一段 bash 来单独测：一个函数，或者两个记号之间的一段。
//!
//! 和原来 Python 测试里的写法一个意思（`SOURCE.index(…)`）：记号找不到就 panic、说出是哪个——cca 里改了名字、挪了地方，
//! 测试要跟着改，不能悄悄切出一段空的来测。带 `_in` 的几个对着给的文字切（install.sh、安装脚本这些别的脚本）。

use std::sync::OnceLock;

/// cca 脚本的全文（仓库里的那一份，见 [`crate::harness::repo`]）：读一次、记下来
pub fn source() -> &'static str {
    static TEXT: OnceLock<String> = OnceLock::new();
    TEXT.get_or_init(|| {
        let path = crate::harness::repo().join("cca");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不了 {}：{e}", path.display()))
    })
}

/// cca 里一个函数的全文：从行首的 `名字() {` 起，到它后面第一个行首的 `}` 为止（不带后面的换行）
pub fn function(name: &str) -> String {
    function_in(source(), name).to_string()
}

/// cca 里几个函数接在一起（照给的先后，一个一行开头）
pub fn functions(names: &[&str]) -> String {
    let src = source();
    names.iter().map(|name| function_in(src, name)).collect::<Vec<_>>().join("\n")
}

/// cca 里从 start 第一次出现的地方起，到 end 第一次出现的地方为止（不含 end）。end 是从头找的
/// （`SOURCE[SOURCE.index(a):SOURCE.index(b)]`）：它在 start 前面就出现过的话 panic——原来那样写切出来是空的
pub fn between(start: &str, end: &str) -> String {
    between_in(source(), start, end).to_string()
}

/// 同 [`between`]，但 end 从 start 后面找起（`SOURCE.index(b, SOURCE.index(a))`）
pub fn between_after(start: &str, end: &str) -> String {
    between_after_in(source(), start, end).to_string()
}

/// [`function`]，对着给的文字
pub fn function_in<'a>(text: &'a str, name: &str) -> &'a str {
    let head = format!("\n{name}() {{");
    let start = find(text, &head, 0) + 1;
    let end = find(text, "\n}", start) + 2;
    &text[start..end]
}

/// [`between`]，对着给的文字
pub fn between_in<'a>(text: &'a str, start: &str, end: &str) -> &'a str {
    let from = find(text, start, 0);
    let to = find(text, end, 0);
    assert!(to >= from, "切不出来：{end:?} 在 {start:?} 前面就出现了");
    &text[from..to]
}

/// [`between_after`]，对着给的文字
pub fn between_after_in<'a>(text: &'a str, start: &str, end: &str) -> &'a str {
    let from = find(text, start, 0);
    let to = find(text, end, from);
    &text[from..to]
}

/// needle 在 text 里从 from 起第一次出现的位置；没有就 panic（`str.index`）
fn find(text: &str, needle: &str, from: usize) -> usize {
    match text[from..].find(needle) {
        Some(at) => from + at,
        None => panic!("脚本里找不到 {needle:?}（改了名字或者挪了地方？测试里切的记号要跟着改）"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str =
        "#!/bin/bash\nA=1\none() {\n\tif true; then\n\t\techo 1\n\tfi\n}\n# 说明\ntwo() { echo 2; }\nthree() {\n\techo 3\n}\nB=2\n";

    #[test]
    fn function_ends_at_first_closing_brace_line() {
        assert_eq!(function_in(TEXT, "one"), "one() {\n\tif true; then\n\t\techo 1\n\tfi\n}");
        assert_eq!(function_in(TEXT, "three"), "three() {\n\techo 3\n}");
        // 写成一行的函数：和原来一样，一直切到后面第一个行首的 }（连着下一个函数）——要单独切它得用 between
        assert_eq!(function_in(TEXT, "two"), "two() { echo 2; }\nthree() {\n\techo 3\n}");
    }

    #[test]
    fn between_like_index_slices() {
        assert_eq!(between_in(TEXT, "A=1", "\n# 说明"), "A=1\none() {\n\tif true; then\n\t\techo 1\n\tfi\n}");
        assert_eq!(between_after_in(TEXT, "two() {", "\n}"), "two() { echo 2; }\nthree() {\n\techo 3");
        assert_eq!(between_in(TEXT, "three() {", "\nB=2"), "three() {\n\techo 3\n}");
    }

    #[test]
    #[should_panic(expected = "找不到")]
    fn missing_name_panics() {
        function_in(TEXT, "four");
    }

    #[test]
    #[should_panic(expected = "前面就出现了")]
    fn end_before_start_panics() {
        between_in(TEXT, "three() {", "A=1");
    }

    #[test]
    fn reads_the_repo_script() {
        assert!(source().starts_with("#!/"));
        let f = function("cmd_clients");
        assert!(f.starts_with("cmd_clients() {") && f.ends_with("\n}"), "{}", &f[..f.len().min(80)]);
    }
}
