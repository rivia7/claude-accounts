//! 设备统计（`presence_problems` 到 `cmd_presence_stop`、`past_time` 到 `cmd_presence_note`）：presence-agent、presence-wake、
//! presence-stop、presence-view、presence-note。
//!
//! 还没移植，先放着别处要用的入口。

use super::Cmd;
use crate::py::obj::V;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[];

/// 映射里 presence 一节的问题（cca:10081）
pub fn presence_problems(_p: &V) -> Vec<String> {
    unimplemented!("presence_problems 还没移植")
}
