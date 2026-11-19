//! 多设备共用一个路由器的合并（`router_merge`、`cmd_router_merge`、`cmd_slot_name`、`cmd_router_evict`）、节点改名跟着走
//! （`node_match` 到 `cmd_nodes_rename`）、fill-check / fill-summary / nodes-fill、policy-check / policy-request / policy-lock。
//!
//! 还没移植。

use super::Cmd;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[];
