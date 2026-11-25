# iOS 上的 cca App（ios/）

先读 `docs/dev/ios.md`：阶段表和定下来的、二维码里的链接（`cca-remote://pair?…`）、协议核心 `ios/CcaRemoteKit`、`ios/test.sh` 和 `tests/ios_kit.rs`、TestFlight。命令在根目录 CLAUDE.md 的「命令」。

配对、握手、记录、帧的格式是四份实现（辅助程序、`tests/kit/spec.rs`、网页端的 `core.js`、这里的 CcaRemoteKit），SwiftTerm 和 `macos/Vendor/SwiftTerm/` 是同一个版本：改的时候一起改，清单在 `.claude/rules/remote.md`（读这里的文件时自动加载）。
