//! Python 兼容层：原来的实现靠的那些 Python 行为（JSON 的写法、正则的 `$`、取整、字符宽度、输出的缓冲……）在 Rust 里的对应。
//! 移植出来的代码只经这一层做这些事，不直接用 Rust 标准库或别的库里“差不多”的东西：差一点，文件和摘要就对不上。

pub mod base64;
pub mod fail;
pub mod fs;
pub mod hash;
pub mod io;
pub mod ipaddr;
pub mod json;
pub mod num;
pub mod obj;
pub mod path;
pub mod plist;
pub mod proc;
pub mod rand;
pub mod re;
pub mod shutil;
pub mod signal;
pub mod text;
pub mod time;
pub mod tls;
pub mod toml;
pub mod uni;
pub mod url;
pub mod xml;
pub mod zip;
pub mod zlib;

pub use fail::{die, exit, Fail, R};
#[allow(unused_imports)]
pub use obj::{Dict, List, V};
