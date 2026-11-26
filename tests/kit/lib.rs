//! 测试的支持库：回归用的小工具（tests/tool）和各份测试（tests/*.rs）共用。
//! 不用辅助程序（cli/src）的代码——把它当黑盒；协议那一头照 server/README.md 独立实现。

pub mod acmefake;
pub mod apnsfake;
pub mod bash;
pub mod cca_src;
pub mod crypto;
pub mod fleet;
pub mod fsx;
pub mod gui_session;
pub mod harness;
pub mod http;
pub mod httpd;
pub mod hubbox;
pub mod json;
pub mod mailfake;
pub mod pki;
pub mod plist;
pub mod proc;
pub mod pty;
pub mod py;
pub mod relay;
pub mod sandbox;
pub mod server;
pub mod spec;
pub mod term;
pub mod tls;
pub mod tlsd;
pub mod width;

// 这两个依赖要等用到它们的模块写好（TLS、压缩）：先占着，免得说没用到
use {miniz_oxide as _, rustls as _};
