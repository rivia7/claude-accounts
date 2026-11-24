//! Team 的同步服务器、证书助手在测试里用哪一份跑。
//!
//! 现在有两份：server/ 下 Python 的（平常用的）和辅助程序里 Rust 的（`cca-core team-server`、`cca-core team-acme`）。
//! `CCA_TEST_SERVER=rust` 时用 Rust 的，没设或者是别的值用 Python 的——和 tests/helper_bin.py、tests/hub_box.py、
//! regress.sh 的 tm_srv 认的是同一个开关，所以同一套测试两份都能对着跑。Python 的那一份删掉以后，这个开关和 Python 那一支一起去掉。

use crate::harness;
use crate::proc;

/// 服务器、证书助手用哪一份
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    /// server/cca-presence.py、server/cca-acme.py
    Py,
    /// 辅助程序的 team-server、team-acme
    Rust,
}

impl Flavor {
    /// 照 `CCA_TEST_SERVER` 定
    pub fn current() -> Flavor {
        Flavor::named(std::env::var("CCA_TEST_SERVER").ok().as_deref())
    }

    /// `CCA_TEST_SERVER` 是这个值时用哪一份：只有 rust 是 Rust 的
    pub fn named(value: Option<&str>) -> Flavor {
        match value {
            Some("rust") => Flavor::Rust,
            _ => Flavor::Py,
        }
    }

    /// 是不是 Rust 的那一份（直接调 Python 那一份里的函数的用例，这时候跳过）
    pub fn is_rust(self) -> bool {
        self == Flavor::Rust
    }
}

/// 起服务器的命令的开头（后面接它的参数）：Python 的是 `python3 -I server/cca-presence.py`，Rust 的是 `辅助程序 team-server`
pub fn server_argv() -> Vec<String> {
    argv(Flavor::current(), "cca-presence.py", "team-server")
}

/// 起证书助手的命令的开头：Python 的是 `python3 -I server/cca-acme.py`，Rust 的是 `辅助程序 team-acme`
pub fn acme_argv() -> Vec<String> {
    argv(Flavor::current(), "cca-acme.py", "team-acme")
}

fn argv(flavor: Flavor, script: &str, subcommand: &str) -> Vec<String> {
    match flavor {
        Flavor::Rust => vec![harness::helper().to_string_lossy().into_owned(), subcommand.to_string()],
        Flavor::Py => {
            // 子进程的环境是空的，命令要写全路径：照这个进程的 PATH 找
            let python = proc::which("python3")
                .unwrap_or_else(|| panic!("PATH 上没有 python3：{script} 跑不了（CCA_TEST_SERVER=rust 用辅助程序里的那一份）"));
            let path = harness::repo().join("server").join(script);
            vec![python.to_string_lossy().into_owned(), "-I".to_string(), path.to_string_lossy().into_owned()]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_rust_selects_rust() {
        assert_eq!(Flavor::named(Some("rust")), Flavor::Rust);
        for other in [None, Some(""), Some("py"), Some("Rust"), Some("1")] {
            assert_eq!(Flavor::named(other), Flavor::Py, "{other:?}");
        }
    }

    #[test]
    fn argv_heads() {
        let rust = argv(Flavor::Rust, "cca-presence.py", "team-server");
        assert_eq!(rust.len(), 2);
        assert!(rust[0].ends_with("cca-core") || std::env::var_os("CCA_HELPER").is_some(), "{rust:?}");
        assert_eq!(rust[1], "team-server");
        if proc::which("python3").is_some() {
            let py = argv(Flavor::Py, "cca-acme.py", "team-acme");
            assert_eq!(py.len(), 3);
            assert_eq!(py[1], "-I");
            assert!(py[2].ends_with("/server/cca-acme.py"), "{py:?}");
        }
    }
}
