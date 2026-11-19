//! 标准输出、标准错误：缓冲的时机照 Python 的来。
//!
//! bash 把子命令的 stdout 接在管道上取返回值，这时 Python 的 stdout 是整块缓冲的：攒够 8192 字节或退出时才写出去，
//! stderr 则是一行一写。回归里常把两边合在一起（`2>&1`）看，子命令自己起的进程也往同一个地方写，先后顺序由这个
//! 时机决定。所以不用 `println!`（它总是一行一写）：输出一律经这里的 `out!` / `err!`。

use std::io::Write;
use std::sync::Mutex;

struct Stream {
    fd: i32,
    buf: Vec<u8>,
    /// 一行一写（终端上的 stdout、任何时候的 stderr）；第一次写的时候才看是不是终端
    line: Option<bool>,
    /// 写不出去了（读的那头关了）：后面的都丢掉，退出时照 Python 用退出码 120
    broken: bool,
}

const BLOCK: usize = 8192;

impl Stream {
    const fn new(fd: i32) -> Stream {
        Stream { fd, buf: Vec::new(), line: None, broken: false }
    }

    fn write(&mut self, s: &str) {
        let fd = self.fd;
        // SAFETY: isatty 只是问一下这个描述符
        let line = *self.line.get_or_insert_with(|| fd == 2 || unsafe { libc::isatty(fd) } == 1);
        self.buf.extend_from_slice(s.as_bytes());
        if (line && s.contains(['\n', '\r'])) || self.buf.len() >= BLOCK {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let data = std::mem::take(&mut self.buf);
        if self.broken {
            return;
        }
        let mut rest = &data[..];
        while !rest.is_empty() {
            // SAFETY: rest 是有效的内存，fd 是进程自己的 1 或 2
            let n = unsafe { libc::write(self.fd, rest.as_ptr().cast(), rest.len()) };
            if n > 0 {
                rest = &rest[n as usize..];
            } else if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            } else {
                self.broken = true;
                return;
            }
        }
    }
}

// 和 Python 的 sys.stdout / sys.stderr 一样全进程一份，线程里写的也进同一个缓冲
static OUT: Mutex<Stream> = Mutex::new(Stream::new(1));
static ERR: Mutex<Stream> = Mutex::new(Stream::new(2));

fn out() -> std::sync::MutexGuard<'static, Stream> {
    OUT.lock().unwrap_or_else(|e| e.into_inner())
}

fn err() -> std::sync::MutexGuard<'static, Stream> {
    ERR.lock().unwrap_or_else(|e| e.into_inner())
}

/// `sys.stdout.write(s)`
pub fn write_out(s: &str) {
    out().write(s);
}

/// `sys.stderr.write(s)`
pub fn write_err(s: &str) {
    err().write(s);
}

/// `sys.stdout.flush()`
pub fn flush_out() {
    out().flush();
}

/// `sys.stderr.flush()`
pub fn flush_err() {
    err().flush();
}

/// 进程正常结束前：两边都写出去。stdout 写不出去（管道那头关了）时返回 false，main 照 Python 用退出码 120
pub fn finish() -> bool {
    flush_out();
    flush_err();
    !out().broken
}

/// 原始字节直接写到 stdout（先把攒着的文字写出去，保持先后）
pub fn write_out_bytes(data: &[u8]) {
    flush_out();
    let _ = std::io::stdout().write_all(data);
    let _ = std::io::stdout().flush();
}

/// `sys.stdin.read()`：读到头，不是 UTF-8 的字节换成 U+FFFD（Python 在 `-X utf8` 下是 surrogateescape，
/// 要原样的字节时用 `read_stdin_bytes`）
pub fn read_stdin() -> String {
    String::from_utf8_lossy(&read_stdin_bytes()).into_owned()
}

/// `sys.stdin.read()`。和 `open()` 读文件不一样：POSIX 上的 sys.stdin 不做通用换行，`\r\n` 原样留着（对拍核实过）
pub fn read_stdin_text() -> String {
    read_stdin()
}

/// `sys.stdin.readline()`：读一行（带着结尾的换行；读到头是空串）
pub fn read_stdin_line() -> String {
    let mut line = String::new();
    let _ = std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut line);
    line
}

/// `sys.stdin.buffer.read()`
pub fn read_stdin_bytes() -> Vec<u8> {
    let mut data = Vec::new();
    let _ = std::io::Read::read_to_end(&mut std::io::stdin().lock(), &mut data);
    data
}

/// `print(…)`：文字加换行，写到 stdout
#[macro_export]
macro_rules! out {
    () => { $crate::py::io::write_out("\n") };
    ($($arg:tt)*) => {{
        let mut s = format!($($arg)*);
        s.push('\n');
        $crate::py::io::write_out(&s)
    }};
}

/// `print(…, end='')`
#[macro_export]
macro_rules! out_raw {
    ($($arg:tt)*) => { $crate::py::io::write_out(&format!($($arg)*)) };
}

/// `sys.stderr.write(… + '\n')`
#[macro_export]
macro_rules! err {
    ($($arg:tt)*) => {{
        let mut s = format!($($arg)*);
        s.push('\n');
        $crate::py::io::write_err(&s)
    }};
}

/// `sys.stderr.write(…)`（不加换行；要马上看得到就接着 `flush_err()`）
#[macro_export]
macro_rules! err_raw {
    ($($arg:tt)*) => { $crate::py::io::write_err(&format!($($arg)*)) };
}
