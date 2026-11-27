---
paths:
  - "cli/src/cmd/remote/hook.rs"
  - "cli/src/cmd/remote/popup.rs"
  - "cli/src/cmd/remote/host.rs"
  - "cli/src/cmd/remote/vt.rs"
  - "cli/src/cmd/codex.rs"
  - "cli/src/cmd/agy.rs"
  - "tests/tool/fake/codex.rs"
  - "tests/tool/fake/agy.rs"
  - "tests/tool/fake_client.rs"
  - "ios/CcaRemoteKit/Sources/CcaRemoteKit/Push.swift"
---

# Codex、Antigravity 通知的成对修改

从根目录 CLAUDE.md「要一起改的」分出来的，只跟 Codex、Antigravity 会话的「做完了 / 在等你」有关（Claude Code 的钩子在 remote.md）。读到上面 `paths` 里的文件时自动加载；只改 `cca` 里 bash 那一侧（`_remote-hook`、开头交给辅助程序的 `_agy-statusline`、`run_account` 的 `LAUNCH_ARGS`）时，自己先来读这一份。怎么实现、怎么测见 docs/dev/remote.md「Codex、Antigravity 的通知」。

- Codex 的 notify：`codex.rs` 的 `codex_launch_args` 写的 `-c notify=[<cca>, "_remote-hook", "--codex"(, "--then", 用户的)]` ↔ bash 的 `_remote-hook`（`shift` 以后原样交给 `py remote-hook`）↔ `hook.rs` 的 `cmd_remote_hook` / `codex_hook`（`--codex [--then JSON] 载荷`）↔ 假 codex（`tests/setup.sh` 的 `codex_notify`、`cca-test codex-notify`）：开着的会话的命令行里写着这条命令，改名、改参数的话旧的写法也要一直认
- Codex 在等你：`CODEX_NOTIFY_TUI`（tui.* 那三项）↔ `vt.rs` 的 `osc_note_text`（认哪种序列）↔ `hook.rs` 的 `codex_osc_note`（认哪几句英文，Codex 换了写法跟着换）↔ `codex_probe` 探测时带的（`CODEX_NOTIFY_PROBE` + `CODEX_NOTIFY_TUI`：和启动时加的要是同一套，不然探测说认、启动时报错退出）↔ 假客户端的 `osc`、假 codex 的 `codex_osc` / `codex_nocfg`；`probe.json` 的 `notify`：探测的做法变了要让旧的缓存失效（读缓存时要求有这一项）
- Codex 交互不交互（`codex_interactive`）：`CODEX_BATCH` ↔ `settings.rs` 的 `CODEX_VALUES`（带值的选项）↔ 新版 codex 的子命令（`codex --help`）
- agy 的状态栏命令：`agy_launch_args` 写的 `<cca> _agy-statusline 账号目录名 会话标记 [--then 命令]` ↔ bash 开头那一行（找到辅助程序就交给 `agy-statusline`）↔ `cmd_agy_statusline`（也认没有会话标记的旧写法）↔ 假 agy 的 `statusline`（`agy_states`、`agy_title`）；`AGY_BUSY`、`turn_step` 用到的状态栏 JSON 的字段（`agent_state` 的取值、`tool_confirmation_pending`、`conversation_title`、`cwd`）↔ agy 的文档
- 通知的字：`codex_done`、`codex_osc_note`、`agy_note` 的标题照 Claude Code 的那几种写（`done_text`、`ask_text`）；按标题结尾的「做完了」挑声音：`popup.rs` 的 `sound`、iOS 的 `NoteSound`（CcaRemoteKit 的 `Push.swift`，额度的通知照 `event` 挑）↔ 这几处的标题
- 这台 Mac 上弹：config.json 的 `popup`（`popup::on`）↔ `cca help` 的配置说明（只在 macOS 上的那一段）↔ README（两份）↔ docs/dev/remote.md。测试里弹的都要到假的 osascript：回归沙箱的（`tests/setup.sh`）、suite 设备的（`kit::fleet` 的 `OSASCRIPT`）；新的测试夹具里起 cca 的 Codex / agy 会话、`remote-hook --codex`、宿主的 codex 会话时，PATH 上也放一个
