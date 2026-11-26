//! 远程会话（docs/dev/remote.md）。这一块不是从原来的 Python 移植的，是在辅助程序里新写的，子命令都叫 `remote-…`。
//!
//! - `remote/host.rs`：会话宿主（remote-host）：客户端跑在它开的 pty 里，启动它的终端没了会话照样运行，别的终端能接进来
//! - `remote/attach.rs`：接入端（remote-attach）：把一个终端接到宿主上；`remote/keys.rs`：从按键里认出“断开”
//! - `remote/manage.rs`：开关、列表、结束一个会话（remote-set、remote-list、remote-end）、推通知（remote-notify）
//! - `remote/hook.rs`：cca 自己带给 Claude Code 的钩子（remote-hook：做完了、在等你），Codex、Antigravity 的这两种也从这里推
//! - `remote/popup.rs`：这台 Mac 上弹的通知（Codex、Antigravity 的会话）
//! - `remote/status.rs`：给配对的手机放账号的状态、推额度用完 / 恢复的通知（remote-status）
//! - `remote/local.rs`：这台机器上的会话目录、socket、帧的格式
//! - `remote/vt.rs`：终端输出的识别（查询、模式、回放）
//! - `remote/pty.rs`：伪终端和终端模式；`remote/sig.rs`：等信号
//!
//! 终端的内容可能有密钥和源码：只在内存里转，不写文件、不进日志、不进任何进程的命令行和环境变量。

mod attach;
mod crypto;
mod far;
mod hook;
mod host;
mod keys;
mod link;
mod local;
mod manage;
mod pair;
mod peers;
mod popup;
mod pty;
mod relay;
mod sig;
mod status;
mod vt;

pub use hook::{agy_note, deliver as notify_deliver, hooks as notify_hooks, AgyEvent};
pub use local::activity as hosted_activity;
pub use manage::{hooks_on as notify_hooks_on, remote_on};
pub use popup::on as popup_on;
pub use pty::restore_on_panic;
pub use status::status_kick;

use super::Cmd;

pub const CMDS: &[Cmd] = &[
    Cmd::all("remote-host", host::cmd_remote_host),
    Cmd::all("remote-attach", attach::cmd_remote_attach),
    Cmd::all("remote-list", manage::cmd_remote_list),
    Cmd::all("remote-set", manage::cmd_remote_set),
    Cmd::all("remote-end", manage::cmd_remote_end),
    Cmd::all("remote-notify", manage::cmd_remote_notify),
    Cmd::all("remote-hook", hook::cmd_remote_hook),
    Cmd::all("remote-link", link::cmd_remote_link),
    Cmd::all("remote-pair-show", pair::cmd_remote_pair_show),
    Cmd::all("remote-pair-join", pair::cmd_remote_pair_join),
    Cmd::all("remote-peers", pair::cmd_remote_peers),
    Cmd::all("remote-allow", pair::cmd_remote_allow),
    Cmd::all("remote-unpair", pair::cmd_remote_unpair),
    Cmd::all("remote-forget", pair::cmd_remote_forget),
    Cmd::all("remote-hubs", pair::cmd_remote_hubs),
    Cmd::all("remote-status", status::cmd_remote_status),
];
