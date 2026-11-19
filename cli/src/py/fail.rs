//! 出错和提前退出。
//!
//! 原来的 Python 靠异常：`die()` 和 `sys.exit(n)` 是 SystemExit，从多深的地方都能直接退出（沿途的 `with`、`finally`
//! 照常收尾）；读文件、解析失败是 OSError / ValueError，调用方用 `except (OSError, ValueError)` 当“没有”处理。
//! 这里都是 `Result<T, Fail>`：`?` 一路传上去，`Drop` 收尾，main 最后照 Python 的样子打印、定退出码。
//! 调用方要“接住某一类”时看 `Fail::is_os()` / `is_value()` 这些，和 Python 的异常类一一对应（包括谁是谁的子类）。

use std::fmt;
use std::io;

pub type R<T> = Result<T, Fail>;

#[derive(Debug)]
pub enum Fail {
    /// `die(说明)`：说明写到 stderr，退出码 1
    Die(String),
    /// `sys.exit(n)`：不说话，退出码 n
    Exit(i32),
    /// OSError 一类（FileNotFoundError、PermissionError、超时、管道断了……）
    Os(io::Error),
    /// ValueError 一类（JSON 写错了、不是 UTF-8、`int()` 认不出……），带着 `str(e)`
    Value(String),
    /// 各模块自己的异常（MailRetry、ClientUpdateError、CostBusy、Stop……）：类名、`e.args` 里的几样
    Other(&'static str, Vec<String>),
}

impl Fail {
    /// `except OSError`
    pub fn is_os(&self) -> bool {
        matches!(self, Fail::Os(_))
    }

    /// `except FileNotFoundError`
    pub fn is_not_found(&self) -> bool {
        matches!(self, Fail::Os(e) if e.kind() == io::ErrorKind::NotFound)
    }

    /// `except ValueError`（UnicodeDecodeError、JSONDecodeError 都是它的子类）
    pub fn is_value(&self) -> bool {
        matches!(self, Fail::Value(_))
    }

    /// `except (OSError, ValueError)`：读不了、格式不对都当作没有
    pub fn is_os_or_value(&self) -> bool {
        self.is_os() || self.is_value()
    }

    /// 模块自己的那一类异常（`except MailRetry`）
    pub fn is(&self, class: &str) -> bool {
        matches!(self, Fail::Other(c, _) if *c == class)
    }

    /// `except SystemExit`（die 和 sys.exit）
    pub fn is_exit(&self) -> bool {
        matches!(self, Fail::Die(_) | Fail::Exit(_))
    }

    /// 接住了、不再往上传的地方调一下（`except SystemExit: pass` 这一类）：原来 `die()` 在抛出之前就把说明写到 stderr 了，
    /// 这里的说明是带在 `Fail::Die` 里、到 main 才写的，接住的地方要替它写出来
    pub fn swallowed(&self) {
        if let Fail::Die(msg) = self {
            crate::err!("{msg}");
        }
    }

    /// 没人接住时进程的退出码
    pub fn code(&self) -> i32 {
        match self {
            Fail::Exit(n) => *n,
            _ => 1,
        }
    }
}

impl fmt::Display for Fail {
    /// `str(e)`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fail::Die(m) | Fail::Value(m) => f.write_str(m),
            Fail::Exit(n) => write!(f, "{n}"),
            Fail::Os(e) => f.write_str(&os_error_text(e)),
            Fail::Other(_, args) => f.write_str(args.first().map_or("", |s| s.as_str())),
        }
    }
}

impl From<io::Error> for Fail {
    fn from(e: io::Error) -> Fail {
        Fail::Os(e)
    }
}

impl From<super::json::JsonError> for Fail {
    fn from(e: super::json::JsonError) -> Fail {
        Fail::Value(e.0)
    }
}

/// `die(说明)`
pub fn die<T>(msg: impl Into<String>) -> R<T> {
    Err(Fail::Die(msg.into()))
}

/// `sys.exit(n)`
pub fn exit<T>(code: i32) -> R<T> {
    Err(Fail::Exit(code))
}

/// `raise ValueError(说明)`
pub fn value_error<T>(msg: impl Into<String>) -> R<T> {
    Err(Fail::Value(msg.into()))
}

/// `str(OSError)` 的样子：`[Errno 2] No such file or directory`（系统给的说明，不带 Rust 加的「(os error 2)」）
pub fn os_error_text(e: &io::Error) -> String {
    match e.raw_os_error() {
        Some(n) => format!("[Errno {}] {}", n, errno_text(n)),
        None => e.to_string(),
    }
}

/// `os.strerror(n)`
pub fn errno_text(n: i32) -> String {
    // strerror 的说法各个系统自己定，和 Python 用的是同一个 C 库函数
    let mut buf = [0 as libc::c_char; 256];
    // SAFETY: buf 够长，strerror_r 写进去的是以 0 结尾的字符串
    unsafe {
        if libc::strerror_r(n, buf.as_mut_ptr(), buf.len()) != 0 {
            return format!("Unknown error {n}");
        }
        std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes() {
        let nf = Fail::from(io::Error::from_raw_os_error(libc::ENOENT));
        assert!(nf.is_os() && nf.is_not_found() && nf.is_os_or_value() && !nf.is_value());
        assert_eq!(nf.to_string(), "[Errno 2] No such file or directory");
        let v = Fail::Value("bad".into());
        assert!(v.is_value() && !v.is_os() && v.code() == 1);
        assert!(Fail::Other("MailRetry", vec!["net".into(), "x".into()]).is("MailRetry"));
        assert_eq!(Fail::Exit(3).code(), 3);
        assert!(Fail::Die("x".into()).is_exit());
    }
}
