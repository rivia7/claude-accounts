//! 客户端统一更新：bash 那一半（cmd_clients）——安装要一字不差的令牌、有效的计划、用户的同意。
//! （原来是 tests/client_updates.py）
//!
//! 辅助程序那一半（client-updates：status、check、mode、auto-install、plan、install，签名、目标漂移、中断恢复……）在辅助程序
//! 自己的测试里：移植时拿原来的 Python 把那 70 个用例一步一步走了一遍，每一步的输出、退出码、起的命令、写的文件记在
//! `cli/tests/vectors/src_clients.json`，`cmd::clients::tests::like_python`（cargo test）照着重放、逐步核对。

use kit::harness::{Ctx, Module};
use kit::proc::Cmd;
use kit::py::splitlines;
use kit::sandbox::ran;
use kit::{case, cca_src, fsx};

pub const MODULE: Module = Module {
    name: "client_updates",
    src: include_str!("client_updates.rs"),
    default: true,
    tests: &[case!(shell_requires_exact_token_authorization_and_valid_plan), case!(consent_says_how_a_terminal_tool_is_updated)],
};

/// cca 里的 cmd_clients 后面接上 `cmd_clients install "$1" --json`，前面是 prologue 里的替身：用 /bin/bash 跑，$1 是 supplied
fn run_install(prologue: &str, supplied: &str, env: &[(&str, &str)]) -> kit::sandbox::Ran {
    let script = format!("{prologue}{}\ncmd_clients install \"$1\" --json", cca_src::function("cmd_clients"));
    let mut cmd = Cmd::new("/bin/bash");
    cmd.args(["-c", &script, "test", supplied]);
    for (key, value) in env {
        cmd.env(key, value);
    }
    ran(&mut cmd)
}

fn shell_requires_exact_token_authorization_and_valid_plan(t: &mut Ctx) {
    let root = t.tmp("cca-client-update-");
    let token = "a".repeat(64);
    let prologue = r#"
die() { echo "$*" >&2; exit 1; }
fail_warn() { :; }
client_update_py() {
  echo "$1 $2" >> "$TEST_LOG"
  if [ "$1" = plan ]; then
    [ "$PLAN_OK" = 1 ] || return 1
    echo "CU_VERSION=2.0; CU_INSTALLED=1.0; CU_TARGET=/tmp/ChatGPT.app; CU_LABEL=ChatGPT"
  else echo '{}'; fi
}
consent() { echo "consent $1 $2" >> "$TEST_LOG"; return "$CONSENT_RC"; }
"#;
    for (supplied, plan, consent, installs) in
        [(token.as_str(), "1", "1", false), (&token, "0", "0", false), ("not-a-token", "1", "0", false), (&token, "1", "0", true)]
    {
        let log = root.join("shell.log");
        fsx::write(&log, "");
        let log_path = log.display().to_string();
        let result = run_install(
            prologue,
            supplied,
            &[("PATH", "/usr/bin:/bin"), ("TEST_LOG", &log_path), ("PLAN_OK", plan), ("CONSENT_RC", consent)],
        );
        let text = fsx::read_text(&log);
        let lines = splitlines(&text);
        assert_eq!(lines.contains(&format!("install {token}").as_str()), installs, "{}{lines:?}", result.err);
        if plan == "0" || supplied != token {
            assert!(!lines.iter().any(|x| x.starts_with("consent ")), "{lines:?}");
        }
        if installs {
            assert!(lines.contains(&format!("consent client_update {token}").as_str()), "{lines:?}");
        }
    }
}

fn consent_says_how_a_terminal_tool_is_updated(t: &mut Ctx) {
    let root = t.tmp("cca-client-update-");
    let token = "a".repeat(64);
    let prologue = r#"
die() { echo "$*" >&2; exit 1; }
fail_warn() { :; }
client_update_py() {
  if [ "$1" = plan ]; then printf '%s\n' "CU_VERSION=2.1; CU_INSTALLED=2.0; CU_TARGET=/x/claude; CU_LABEL=Claude; CU_NOTE=${NOTE@Q}"
  else echo '{}'; fi
}
consent() { printf '%s\n' "$5" >> "$TEST_LOG"; return 0; }
"#;
    // 和原来一样换掉 ${NOTE@Q}（macOS 的 /bin/bash 是 3.2，不认这个写法）
    let prologue = prologue.replace("${NOTE@Q}", "\"'$NOTE'\"");
    for (note, expected) in [("用 claude update 更新", "用 claude update 更新"), ("", "目标：/x/claude")] {
        let log = root.join("shell.log");
        fsx::write(&log, "");
        let log_path = log.display().to_string();
        let result = run_install(&prologue, &token, &[("PATH", "/usr/bin:/bin"), ("TEST_LOG", &log_path), ("NOTE", note)]);
        let said = fsx::read_text(&log);
        assert!(said.contains(expected), "{:?} 不在 {said:?} 里\n{}", expected, result.err);
    }
}
