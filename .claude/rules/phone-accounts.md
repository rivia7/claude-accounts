---
paths:
  - "cli/src/cmd/remote/status.rs"
  - "cli/src/cmd/remote/crypto.rs"
  - "cli/src/server/remote.rs"
  - "cli/src/server/remote/**"
  - "cli/src/server/push.rs"
  - "ios/CcaRemoteKit/Sources/CcaRemoteKit/Status.swift"
  - "ios/CcaRemoteKit/Sources/CcaRemoteKit/Push.swift"
  - "ios/CcaRemoteKit/Sources/CcaRemoteKit/Client.swift"
  - "ios/App/**"
  - "ios/Notify/**"
  - "ios/tools/ccaguest/**"
  - "macos/Sources/CcaKit/Model/UsageAlerts.swift"
  - "macos/Sources/CcaKit/Model/Diagnosis.swift"
  - "macos/Sources/CcaKit/Model/Texts.swift"
  - "macos/Sources/CcaApp/UsageNotifier.swift"
  - "tests/kit/spec.rs"
  - "tests/web_relay.rs"
  - "tests/ios_kit.rs"
---

# 手机上的账号、额度的通知的成对修改

从根目录 CLAUDE.md「要一起改的」分出来的：iPhone 上 cca App 的「账号」页（各台电脑放上来的账号的状态）和额度用完 / 恢复的通知（手机经推送、Mac App 在本机弹）。读到上面 `paths` 里的文件时自动加载。怎么实现、怎么测见 docs/dev/remote.md「手机上的账号」、docs/dev/push.md「额度的通知」、docs/dev/usage.md「额度」；配对、推送本身的成对修改在 `.claude/rules/remote.md`。

- 账号的状态（线上的格式）：密文（`Ks = HMAC(P, "cca-remote/v1/status")`、标签 `cca-remote-status`、补齐）、`status_put`（`later`）、`relay_list` 的 `have` / `status`、`push_note`、`push_set` 的 `usage`，上限 `STATUS_BLOB`、`LATER_MAX`、`LATER_SPAN`（`cli/src/server/mod.rs`；辅助程序的 `status.rs` 照它们少放、挑 `later`）：server/README.md「账号的状态」「推送」↔ 服务端（`remote/ops.rs`、`remote.rs` 的 `usage_job` / `due`、`push.rs`）↔ `cmd/remote/crypto.rs` 的 `status_seal`、`usage_tag` ↔ `tests/kit/spec.rs` ↔ CcaRemoteKit 的 `Status.swift`、`Client.swift`（`tests/ios_kit.rs` 的 `against_the_spec`）
- 账号的状态的明文（docs/dev/remote.md「手机上的账号」）：`status.rs` 的 `row_of`、`usage_of`、`weeks_of`、`doc_for`（`more`、`"weeks": false`、`off`）↔ CcaRemoteKit 的 `HostStatus`、`AccountStatus`、`AccountUsage`、`UsageWeeks` ↔ iOS App 的演示数据（`Demo.swift` 的 `DemoAccounts`）↔ `ccaguest` 的 `statusJSON`（`tests/ios_kit.rs` 读它）；内容取自 `_gui state`（`gui_snapshot`）：PROTOCOL 2.2、3.1 里账号、用量的字段改了，这里跟着看
- 额度用完的规则（登录着、不是登录失效、四舍五入到 100%、没到重置时间，数据旧不旧不看；Antigravity 看各组里用得最少的那个；重置时间差不到一小时算同一轮，过了重置时间一天忘掉）：`status.rs` 的 `exhausted`、`SAME_ROUND`、`FORGET` ↔ Mac App 的 `Diagnosis.quotaPressure`、`UsageAlerts`（`sameRound`、`forget`）↔ CcaRemoteKit 的 `AccountStatus.exhausted` ↔ iOS App 的 `AccountMood`
- 额度的通知：明文（`status.rs` 的 `usage_note`：`kind` 是 usage、`event`、`window`、`resets_epoch`……）↔ CcaRemoteKit `Push.swift` 的 `openPushed`、`UsageNote`、`NoteCard` ↔ `ios/Notify/NotificationService.swift`（`threadIdentifier`、userInfo 的 `peer`、`account`）↔ iOS 的 `Notifier`（点了 `openAccount`）↔ `ccaguest` 的 `push-open`；字（窗口、工具的叫法，几点恢复：今天的只写钟点、明天、周几）：CcaRemoteKit 的 `UsageText` ↔ Mac 的 `Texts.usageNote` ↔ `status.rs` 的 `usage_note` 写给旧手机看的那几句；开关「额度用完、恢复时通知」：iOS 的 `Pusher.usageNotes` ↔ Mac 的 `UsageNotifier.enabled`（同一个说法）
