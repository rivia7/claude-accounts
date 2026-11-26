//! 这台 Mac 上弹的通知：cca 启动的 Codex、Antigravity 会话做完了、在等你的时候（hook.rs 的 `deliver`，宿主认出的 Codex 的
//! 通知序列）。Claude Code 的不归这里：用户自己的钩子会弹（docs/dev/remote.md「cca 自己带的钩子」）。
//!
//! 用 `osascript` 的 display notification（通知中心里归在「脚本编辑器」名下，第一次可能要在系统设置里允许它发通知）。脚本从
//! stdin 进、字经参数（不拼进脚本：launchers.rs 的那一套），字是会话里的程序写的也注入不了。config.json 的 `popup` 设成 false
//! 时不弹；不是 macOS 时什么都不做。

use super::local::Note;
use std::io::Write;
use std::process::{Child, Command, Stdio};

const SCRIPT: &str = "on run argv\n\tdisplay notification (item 3 of argv) with title (item 1 of argv) subtitle (item 2 of argv) sound name (item 4 of argv)\nend run\n";

/// 弹不弹：macOS 上，config.json 的 `popup` 不是 false（没有这一项是弹）。每次现读：改了马上生效
pub fn on() -> bool {
    cfg!(target_os = "macos")
        && !crate::py::fs::load_quiet(&crate::core::base::CFG)
            .and_then(|c| c.as_dict())
            .is_some_and(|c| c.get("popup").is_some_and(|v| v.as_bool() == Some(false)))
}

/// 声音：做完了是 Glass，在等你的是 Ping（照维护者的个人钩子）
fn sound(title: &str) -> &'static str {
    if title.ends_with("做完了") {
        "Glass"
    } else {
        "Ping"
    }
}

/// 起 osascript 弹这一条，不等它：调用的要么马上就退出，要么像宿主那样留着它、过后收尸。起不来是 None
pub fn show(n: &Note) -> Option<Child> {
    let mut child = Command::new("osascript")
        .args(["-", &n.title, &n.subtitle, &n.body, sound(&n.title)])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(SCRIPT.as_bytes());
    }
    Some(child)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds() {
        assert_eq!(sound("Codex 做完了"), "Glass");
        assert_eq!(sound("Antigravity 做完了"), "Glass");
        assert_eq!(sound("Codex 要你确认"), "Ping");
        assert_eq!(sound("Antigravity 在等你"), "Ping");
        assert!(SCRIPT.starts_with("on run argv\n") && SCRIPT.ends_with("end run\n"));
    }
}
