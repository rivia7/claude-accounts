//! cca-test 的规矩：用法不对、遇到这里不敢担保的写法，退出码是 2，并在环境变量 CCA_TEST_TRAP 指的文件里记一行。回归里有取反的
//! 断言（! cfgok '…'），退出码 2 会让它悄悄通过，所以回归和 tests/linux.sh 结束时都看这个文件：不是空的就算失败。各个角色、
//! 子命令都经这里记，不各写一份。
//!
//! 一行三栏，用制表符隔开：谁（角色、子命令的名字）、为什么、带的参数（求值器记的是那段程序）。栏里的换行、制表符写成转义的样子，
//! 太长的截掉——一条记录占一行。一行是一次 write 写完的（几个进程同时写也不会串行）；没设这个变量、写不进去都不管。

use std::ffi::OsString;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// “为什么”那一栏最多留多少个字符
const WHY_MAX: usize = 500;
/// “带的参数”那一栏最多留多少个字符
pub const WHAT_MAX: usize = 300;

/// CCA_TEST_TRAP 的值 → 记到哪个文件；没设、设成空的都是不记
pub fn path_of(var: Option<OsString>) -> Option<PathBuf> {
    var.filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// 换行、制表符写成转义的样子，最多留 max 个字符（一条记录占一行、用制表符分栏）
pub fn one_line(text: &str, max: usize) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    match out.char_indices().nth(max) {
        Some((at, _)) => out[..at].to_string(),
        None => out,
    }
}

/// 往 path 追加一行：谁、为什么、带的参数；path 是 None 就不记（求值器的测试自己给文件，别处用 [`record`]）
pub fn record_to(path: Option<&Path>, who: &str, why: &str, what: &str) {
    let Some(path) = path else { return };
    let line = format!("{who}\t{}\t{}\n", one_line(why, WHY_MAX), one_line(what, WHAT_MAX));
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).mode(0o644).open(path) {
        let _ = f.write(line.as_bytes());
    }
}

/// 往 $CCA_TEST_TRAP 追加一行
pub fn record(who: &str, why: &str, what: &str) {
    record_to(path_of(std::env::var_os("CCA_TEST_TRAP")).as_deref(), who, why, what);
}

/// 用法不对：stderr 上说一句（“cca-test 角色：原因”），记一行，退出码 2
pub fn usage(role: &str, args: &[String], why: &str) -> i32 {
    usage_with(role, args, why, "")
}

/// 同上，stderr 上那一句后面接着 tail（这个角色的用法说明；不进记录）
pub fn usage_with(role: &str, args: &[String], why: &str, tail: &str) -> i32 {
    eprintln!("cca-test {role}：{why}{tail}");
    record(role, why, &args.join(" "));
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_comes_from_the_environment() {
        // CCA_TEST_TRAP 没设、设成空的：不记
        assert_eq!(path_of(None), None);
        assert_eq!(path_of(Some("".into())), None);
        assert_eq!(path_of(Some("/x/trap.log".into())), Some(PathBuf::from("/x/trap.log")));
    }

    #[test]
    fn one_record_is_one_line() {
        let dir = kit::fsx::TempDir::new("cca-trap");
        let trap = dir.path().join("trap.log");
        record_to(None, "port", "不要参数", "x");
        assert!(!trap.exists());
        record_to(Some(&trap), "port", "不要参数", "x y");
        record_to(Some(&trap), "re-search", "正则认不了：\n第二行", "a\tb\r\nc");
        record_to(Some(&trap), "val", &"为".repeat(600), &"x".repeat(400));
        let text = std::fs::read_to_string(&trap).unwrap();
        let rows: Vec<Vec<&str>> = text.lines().map(|line| line.split('\t').collect()).collect();
        assert!(text.ends_with('\n') && rows.len() == 3, "{text:?}");
        assert_eq!(rows[0], ["port", "不要参数", "x y"]);
        assert_eq!(rows[1], ["re-search", "正则认不了：\\n第二行", "a\\tb\\r\\nc"]);
        assert_eq!((rows[2][1].chars().count(), rows[2][2].len()), (500, 300));
        // 写不进去（目录不存在）：不管它
        record_to(Some(&dir.path().join("no-such-dir").join("trap.log")), "port", "不要参数", "");
        assert_eq!(one_line("a\nb", 2), "a\\");
    }
}
