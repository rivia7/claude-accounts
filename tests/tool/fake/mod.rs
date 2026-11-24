//! 沙箱里的假命令要的那些角色（tests/setup.sh 写出来的 shell 脚本调它们）：假的 claude、codex、agy 的会话和登录，
//! 假的用量接口、OAuth 回调、浏览器、STUN、GitHub 的发布列表、程序坞的配置……一个角色一个文件。

pub const CMDS: &[(&str, crate::Run)] = &[];
