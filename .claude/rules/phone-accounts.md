---
paths:
  - "cli/src/cmd/remote/status.rs"
  - "cli/src/cmd/remote/crypto.rs"
  - "cli/src/server/remote.rs"
  - "cli/src/server/remote/**"
  - "cli/src/server/push.rs"
  - "ios/CcaRemoteKit/Sources/CcaRemoteKit/Status.swift"
  - "ios/CcaRemoteKit/Sources/CcaRemoteKit/Cost.swift"
  - "ios/CcaRemoteKit/Sources/CcaRemoteKit/Push.swift"
  - "ios/CcaRemoteKit/Sources/CcaRemoteKit/Client.swift"
  - "ios/App/**"
  - "ios/Notify/**"
  - "ios/tools/ccaguest/**"
  - "macos/Sources/CcaKit/Model/UsageAlerts.swift"
  - "macos/Sources/CcaKit/Model/Diagnosis.swift"
  - "macos/Sources/CcaKit/Model/Texts.swift"
  - "macos/Sources/CcaKit/Model/CostTexts.swift"
  - "macos/Sources/CcaKit/Model/CostView.swift"
  - "macos/Sources/CcaApp/DeskArt.swift"
  - "macos/Sources/CcaApp/UsageNotifier.swift"
  - "tests/kit/spec.rs"
  - "tests/web_relay.rs"
  - "tests/ios_kit.rs"
---

# 手机上的账号、额度的通知的成对修改

从根目录 CLAUDE.md「要一起改的」分出来的：iPhone 上 cca App 的「总览」「用量统计」「折合金额」「共享与同步」几页（各台电脑放上来的账号的状态、折合金额、Team）和额度用完 / 恢复的通知（手机经推送、Mac App 在本机弹）。读到上面 `paths` 里的文件时自动加载。怎么实现、怎么测见 docs/dev/remote.md「手机上的账号」、docs/dev/push.md「额度的通知」、docs/dev/usage.md「额度」；配对、推送本身的成对修改在 `.claude/rules/remote.md`。

- 账号的状态（线上的格式）：密文（`Ks = HMAC(P, "cca-remote/v1/status")`、标签 `cca-remote-status`、补齐）、`status_put`（`later`）、`relay_list` 的 `have` / `status`、`push_note`、`push_set` 的 `usage`，上限 `STATUS_BLOB`、`LATER_MAX`、`LATER_SPAN`（`cli/src/server/mod.rs`；辅助程序的 `status.rs` 照它们少放、挑 `later`）：server/README.md「账号的状态」「推送」↔ 服务端（`remote/ops.rs`、`remote.rs` 的 `usage_job` / `due`、`push.rs`）↔ `cmd/remote/crypto.rs` 的 `status_seal`、`usage_tag` ↔ `tests/kit/spec.rs` ↔ CcaRemoteKit 的 `Status.swift`、`Client.swift`（`tests/ios_kit.rs` 的 `against_the_spec`）
- 账号的状态的明文（docs/dev/remote.md「手机上的账号」）：`status.rs` 的 `row_of`（`app`、`cli`、`since`、`model`、`team`）、`usage_of`、`weeks_of`、`cost_of`、`cost_meta`、`team_of`、`net_of`、`why_of`、`host_of`、`doc_for` / `seal_doc`（`more`、`"weeks": false`、`"cost": false`、`brief`、`off`，太长时少放的先后）↔ CcaRemoteKit 的 `HostStatus`、`HostProblem`、`AccountStatus`、`AccountNet`、`AccountUsage`、`UsageWeeks`、`Cost.swift`（`AccountCost`、`HostCost`、`HostTeam`）↔ iOS App 的演示数据（`Demo.swift` 的 `DemoAccounts`、`cost(…)`、两台电脑的 `team`、`cca`、`remote`、`problems`）↔ `ccaguest` 的 `statusJSON`、`costJSON`（`tests/ios_kit.rs` 读它）↔ `ios/Tests/KitTests`；内容取自 `_gui state`（`gui_snapshot`）：PROTOCOL 2.2（账号的 `cost`、`cost_total`、`health`、`can_why`、`api`）、3.1、4.14（`config.teams`）和 `errors`、各工具的 `errors`、`settings_problems` 里的字段改了，这里跟着看
- 电脑那头另查的（docs/dev/remote.md「电脑那头另查的」）：端口池的实况 `status.rs` 的 `live_of`（`_gui live` 的 `slots[].state`、`route`、`local`：PROTOCOL 2.3）↔ Mac 的 `Diagnosis`（portClosed、portDrift）和页顶内置代理那一条；App 后台刷新用量的结果：bash `cmd_usage` 的 EXIT trap ↔ `remote-usage-poll` 的 `poll_end`（退出码、事件）↔ Mac App 的 `CCA_USAGE_POLL`、`finishUsage`（PROTOCOL 3.1）；新版本：`gui.rs` 的 `update_check_save` ↔ `status.rs` 的 `update_of`（PROTOCOL 5.1）；`watched` 认 App 的进程 ↔ bash 的 `gui_running`；手机上的字、记号：iOS 的 `AccountTrouble`、`Texts.hostProblem` / `hostUpdate`、`AccountsView.problemLook` ↔ Mac 的 `TopBanners`、`Texts.ailment`、Guide 的 `Advice`
- 额度用完的规则（登录着、不是登录失效、四舍五入到 100%、没到重置时间，数据旧不旧不看；Antigravity 看各组里用得最少的那个；重置时间差不到一小时算同一轮，过了重置时间一天忘掉）：`status.rs` 的 `exhausted`、`SAME_ROUND`、`FORGET` ↔ Mac App 的 `Diagnosis.quotaPressure`、`UsageAlerts`（`sameRound`、`forget`）↔ CcaRemoteKit 的 `AccountStatus.exhausted` ↔ iOS App 的 `AccountMood`
- 「有问题」「建议先用」的口径和说法（ios/CLAUDE.md「口径只有一份」）：Mac 的 `Diagnosis.all`（先后、`blocksUseFirst`）、`Guide.swift` 的 `Advice` ↔ iOS `Model/Accounts.swift` 的 `AccountMood`、`AccountFlag` 和账号页的 `Advice`；出口检测的结果、泄露、`can_why`、页顶提示的字：Mac `Texts` 的 `healthResult`、`leak`、`flags`、`canWhy` 和 `MainWindow.swift` 的 `TopBanners` ↔ iOS `Texts` 的同名几个和 `hostProblem`；颜色：Mac `Format.swift` 的 `Palette.health`、`leak` ↔ iOS `Design.swift` 的同名两个。结果码（`health.result`）、提示的原因码（`errors[].code`）是 cca 的，cca 加了新的两边一起认
- 额度的通知：明文（`status.rs` 的 `usage_note`：`kind` 是 usage、`event`、`window`、`resets_epoch`……）↔ CcaRemoteKit `Push.swift` 的 `openPushed`、`UsageNote`、`NoteCard` ↔ `ios/Notify/NotificationService.swift`（`threadIdentifier`、userInfo 的 `peer`、`account`）↔ iOS 的 `Notifier`（点了 `openAccount`）↔ `ccaguest` 的 `push-open`；字（窗口、工具的叫法，几点恢复：今天的只写钟点、明天、周几）：CcaRemoteKit 的 `UsageText` ↔ Mac 的 `Texts.usageNote` ↔ `status.rs` 的 `usage_note` 写给旧手机看的那几句；开关「额度用完、恢复时通知」：iOS 的 `Pusher.usageNotes` ↔ Mac 的 `UsageNotifier.enabled`（同一个说法）
- 和 Mac App 一样的画法、说法：iOS 的 `DeskArt.swift`（`DeskPainter`、`OfficePalette` 原样搬的；`DeskLook` 对着 Mac 的 `Employee.pose` / `screen`）↔ `macos/Sources/CcaApp/DeskArt.swift`；iOS `Charts.swift` 的 `CostChart`（`topSteps`、`headroom`、`niceTop`、`ticks`、`domain`、`slot`、`statsTicks`、`dayLabelStride`）↔ `macos/Sources/CcaKit/Model/CostView.swift` 的 `CostChart`；iOS `CostTexts.swift` 的 `teamState`、`teamServerUpdate`、`teamSummary`、`costFreshness` ↔ Mac 的 `Texts`、`CostTexts.swift` 的同名几个。Mac 改了画法、刻度的规矩、这几句话时 iOS 跟着改
- 总览按工具分开、按套餐筛：工具（cca 的 `TOOLS`：claude、codex、agy）、档位（`plan_tier`：entry、standard、advanced、premium，PROTOCOL 2.2）的取值 ↔ iOS 的 `AccountShelf.knownTools` / `knownTiers`、`Texts.toolName`、`Texts.planName`、`TierMark` ↔ Mac 的 `ToolID`、`PlanTier`、`Texts.product`、`Texts.planName`（叫法一样：套餐照 cca 给的原名，档位不起中文的叫法）；cca 加了工具、档位时一起补（认不出的工具、档位排在最后，认不出的档位不画徽记）
