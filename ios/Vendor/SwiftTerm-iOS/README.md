# SwiftTerm 的 iOS 视图（放进仓库的第三方源码）

iOS App 的终端（`ios/App/Terminal/TerminalSurface.swift`，App 里只有这一个文件 `import SwiftTerm`）用 SwiftTerm 的 iOS 视图显示。
SwiftTerm 的其余部分（终端的引擎、Apple 平台共用的那些）和 Mac App 用同一份：`macos/Vendor/SwiftTerm/`，许可证、版本、
“不要的功能”都照那里的 README。这里只放 Mac App 用不着的 iOS 视图。

- 上游：<https://github.com/migueldeicaza/SwiftTerm>，标签 `v1.20.0`（提交 `5d14406844143538cd8f8851d2d8a67c1fe443e5`），
  和 `macos/Vendor/SwiftTerm` 是同一个版本——换版本时两处一起换。
- `iOS/` 里的文件和上游 `Sources/SwiftTerm/iOS/` 里的**逐字节一样**，没有改动；SHA-256 在 `SHA256SUMS`
  （在这个目录里 `shasum -a 256 -c SHA256SUMS` 核对）。要改它的行为，在 `TerminalSurface.swift` 里改（子类、回调）。
- 没带的：`SwiftUITerminalView.swift`（上游自己测试用的，只在 `DEBUG` 下编；App 自己把视图放进 SwiftUI）。
- 许可证：MIT，原文随 App 带着（`ios/App/Resources/SwiftTerm.txt`，和 `macos/Vendor/SwiftTerm/LICENSE` 一样），「设置」→「开源许可」里看得到。

## 怎么编

`ios/cca.xcodeproj` 里的 `SwiftTerm` 目标（静态的 framework，App 链它）：`macos/Vendor/SwiftTerm/Sources/SwiftTerm`（不要 `Mac/` 下
那几个只给 macOS 的视图，`MacAccessibilityService.swift` 要：iOS 的视图也用它）、这里的 `iOS/`、`macos/Vendor/SwiftTerm/BuildInfo.swift`。
`EscapeSequenceParser.swift` 不编上游原样的那一份：编之前「去掉图形协议」那一步跑 `macos/tools/swiftterm-parser.sh`（和 Mac App 的
`macos/build.sh` 同一个脚本）写出换过两行的一份到 `DERIVED_FILE_DIR`，编的是它。总是优化、不带 `DEBUG`、Swift 5 模式、不报上游的警告。

`ios/test.sh` 查：编进去的是换过的那一份（看编译的文件清单）；Mac App 的自检 `macos/tools/swiftterm-check` 链上 App 用的这一份
SwiftTerm、在模拟器里跑，图形协议确实不处理。

## 换版本

1. 先照 `macos/Vendor/SwiftTerm/README.md`「换版本」换那一边。
2. 把上游同一个标签里 `Sources/SwiftTerm/iOS/` 下的文件（上面“没带的”照旧不带）原样取出来换掉 `iOS/` 里的，重新生成 `SHA256SUMS`，改这一份里的版本、提交。
3. 上游加了、删了 `Mac/` 下的文件：改 `ios/cca.xcodeproj` 里 `SwiftTerm` 目标的排除清单（Xcode 里那个文件夹的「Target Membership」）。
4. `bash ios/test.sh`。
