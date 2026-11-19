//! 内置代理（`core_cache` 到 `cmd_core_keep`）：本机端口池（local-env / local-set / local-node）、节点定义和分享链接的解析
//! （node-check / testconfig / save / ls / rm）、内核的配置（core-config / core-nodes-save / core-stale / core-missing / core-summary）、
//! 辅助安装的临时代理（install-env / install-node）、本机 UDP 走节点（udp-env / node / build / probe）、core-keep。
//!
//! 还没移植。

use super::Cmd;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[];
