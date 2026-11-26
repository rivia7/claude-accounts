---
paths:
  - "cli/src/cmd/remote/launch.rs"
  - "cli/src/cmd/remote/template.rs"
  - "cli/src/cmd/remote/peers.rs"
  - "cli/src/cmd/remote/local.rs"
  - "cli/src/cmd/remote/crypto.rs"
  - "cli/src/cmd/remote/host.rs"
  - "cli/src/cmd/remote/link.rs"
  - "tests/remote_launch.rs"
  - "tests/remote_host.rs"
  - "tests/ios_kit.rs"
  - "tests/kit/spec.rs"
  - "tests/kit/relay.rs"
  - "ios/CcaRemoteKit/Sources/CcaRemoteKit/Launch.swift"
  - "ios/CcaRemoteKit/Sources/CcaRemoteKit/Attachment.swift"
  - "ios/App/Views/LaunchSheet.swift"
  - "macos/Sources/CcaKit/Model/RemoteLaunch.swift"
  - "macos/Sources/CcaKit/Protocol/Remote.swift"
  - "macos/Sources/CcaApp/RemoteSessionsPage.swift"
  - "macos/Tests/ContractTests/RemoteLaunchTests.swift"
  - "macos/Tests/Fixtures/remote_templates.json"
  - "server/README.md"
  - "docs/dev/launch.md"
---

# 远程启动的成对修改

从根目录 CLAUDE.md「要一起改的」分出来的，只跟远程启动有关（手机照这台机器上的模板新建会话）。读到上面 `paths` 里的文件时自动加载；只改 `cca` 里 bash 那一侧
（`remote_template`、`remote_allow_launch`、`launcher_*`、`remote_prepare` 的 `CCA_REMOTE_HOSTED`）时自己先来读。怎么实现、怎么测见 `docs/dev/launch.md`；远程会话别的成对修改在 `remote.md`。

- 线上的格式：启动器的说明（`Kl`、标签 `cca-remote-launch`、明文的字段和长度、补齐、`DOC_MAX`）、模板的摘要 `h`、LAUNCH `0x09` / LAUNCHED `0x18`（字段、范围、`code`）：
  `server/README.md`「启动器」↔ 辅助程序（`crypto.rs` 的 `launch_seal`、`template.rs` 的 `launch_doc` / `digest`、`launch.rs` 的 `Ask::parse`、`local.rs` 的 `LAUNCH` / `LAUNCHED`）↔
  `tests/kit/spec.rs`（照文档独立写的）↔ `tests/kit/relay.rs` 的 `LAUNCH` / `LAUNCHED` ↔ CcaRemoteKit 的 `Launch.swift`（`tests/ios_kit.rs` 的 `against_the_spec`、`ios/Tests/KitTests`）
- LAUNCH 只在手机和启动器之间：会话的通道（`link.rs` 的 `bridge`）不转它 ↔ `tests/remote_launch.rs` 里经会话的通道发它的那一句
- LAUNCHED 的 `code`：`launch.rs` 的 `launch` / `fail_text` ↔ `server/README.md` ↔ CcaRemoteKit 的 `LaunchResult`、iOS App 的 `Texts.launchFailure`（哪几个要刷新列表）
- 刚起的会话接入时的宽限：CcaRemoteKit `Attachment` 的 `freshFor`、`freshGap`（45 秒、1.5 秒）↔ docs/dev/launch.md；`launch.rs` 的 `SPAWN_WAIT` 比它短得多
- 起的会话：`spawn_session` 的参数 ↔ `host.rs` 的 `--headless` 那几个参数、`CCA_HEADLESS_LINGER` / `CCA_HEADLESS_IDLE` ↔ `tests/remote_host.rs` 的 `headless_sessions`；
  环境 `KEEP_ENV` ↔ docs/dev/launch.md ↔ `tests/remote_launch.rs` 核对的那几个；`CCA_REMOTE_HOSTED`：`spawn_session` ↔ bash 的 `remote_prepare` ↔ 回归 3e
- 模板：bash `remote_template` 的参数（`--args-line`）↔ 辅助程序 `remote-template add` ↔ Mac App 的 `CcaCommand`（模板命令）；拆那一行的规矩（`sh_split`）↔ PROTOCOL 9.4；
  `args_line` 的拼法（`sh_quote`）↔ Swift 缺这一项时的 `shellQuoteIfNeeded`；`list --json` ↔ PROTOCOL 9.4 ↔ Swift 的 `RemoteTemplatesDoc` ↔
  `macos/Tests/Fixtures/remote_templates.json`、回归 11b3c 写的 `guifx3/remote_templates.json`；`problem` 的取值 ↔ Swift 的 `RemoteTemplateProblem`、`Texts`
- 开关：peers.json 的 `launch`（`peers.rs`：只有手机、control 的读得出 true）、`remote list --json` 的 `peers[].launch`、`launcher`（`launch::summary`）↔ PROTOCOL 9.1 ↔
  Swift 的 `RemotePeer.launch`、`RemoteLauncher` ↔ `macos/Tests/Fixtures/remote_list.json`；同意项 `remote_launch`：`cca` ↔ PROTOCOL 4.2 ↔ Swift 的 `ConsentItem`；
  hello 的 `remote_launch`：`cca` ↔ PROTOCOL 2.1 ↔ `HelloDoc.remoteLaunch` ↔ `macos/Tests/Fixtures/hello.json`
- 后台进程：bash 的 `REMOTE_LAUNCHER_LABEL` ↔ `launch.rs` 的 `LABEL`；`plist_text` ↔ 回归 11b3c 核对 LaunchAgent 的那一条 ↔ `tests/setup.sh` 假的 launchctl 的用户域（`gui/<uid>`）；
  Linux 的记号 `REMOTE_LAUNCHER_TAG`（crontab 那两行，改了的话旧的那两行也要认得、删得掉）；`cca restore` 先 `remote_launcher_remove`（docs/dev/uninstall.md）
