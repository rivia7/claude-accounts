//! 测试的支持库：回归用的小工具（tests/tool）和各份测试（tests/*.rs）共用。
//! 不用辅助程序（cli/src）的代码——把它当黑盒；协议那一头照 server/README.md 独立实现。

pub mod crypto;
pub mod fsx;
pub mod harness;
pub mod json;
pub mod proc;

// 这几个依赖要等用到它们的模块写好（起进程、加解密、TLS、正则、压缩）：先占着，免得说没用到
use {libc as _, miniz_oxide as _, regex as _, ring as _, rustls as _};
