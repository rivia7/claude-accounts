# SwiftTerm（放进仓库的第三方源码）

cca App「远程会话」里的终端用它显示（`macos/Sources/CcaApp/TerminalSurface.swift`，App 里只有这一个文件 `import SwiftTerm`）。
它是一个终端模拟器的视图：App 自己开伪终端、运行 `cca remote attach`，把读到的字节喂给它，把它给出的字节（按键、鼠标、
对终端查询的回答）写回去。做法和为什么见 docs/dev/remote.md「界面 App」。

- 上游：<https://github.com/migueldeicaza/SwiftTerm>，标签 `v1.20.0`（提交 `5d14406844143538cd8f8851d2d8a67c1fe443e5`）。
- 许可证：MIT，见 [LICENSE](LICENSE)（上游的原文，连同各条版权声明）。随 App 放在 `Contents/Resources/Licenses/SwiftTerm.txt`。
- `Sources/SwiftTerm/` 里的文件和上游那个标签里的**逐字节一样**，没有改动；每个文件的 SHA-256 在 `SHA256SUMS`
  （`shasum -a 256 -c SHA256SUMS` 核对）。要改它的行为，在 `TerminalSurface.swift` 里改（子类、回调），不改这里的文件。
- 没带的：`iOS/`（iOS、visionOS 的视图）、`Documentation.docc/`、自己起子进程的那几份（`LocalProcess.swift`、`Pty.swift`、
  `HeadlessTerminal.swift`、`Mac/MacLocalTerminalView.swift`：App 自己开伪终端，不用它们）、`Apple/Metal/Shaders.metal`
  （Metal 渲染默认不开，App 也不开；`Apple/Metal/` 里的 Swift 文件要留着，`MacTerminalView.swift` 里引用它们的地方没有编译开关）。
- `BuildInfo.swift` 是 cca 写的：上游用 SwiftPM 的构建插件生成这个类型，这里给一份固定的。
- 回归里查 cca 自己的 Swift 的那几条 lint（`macos/Sources`、`macos/tools`）不查这里：是上游的源码，照它自己的写法。

## 不要的功能

终端里的内容来自别的机器，SwiftTerm 默认开着的图形协议在这里是口子（Kitty 图形协议能让终端里的内容叫它打开这台 Mac 上的文件、
把成不成回给对方；sixel、iTerm2 的内联图片是解码别人给的图）。上游没有开关，这里的源码又不改，所以：

- `macos/build.sh` 编的时候用换过两行的一份 `EscapeSequenceParser.swift`（`dispatchApc` 里分派 Kitty 图形的那一行换成什么都不做，
  `dispatchDcs` 里建 `SixelDcsHandler` 的那一行换成 `return nil`）。这两行在上游的文件里必须正好各有一处，对不上编译就停。
- 编好以后跑 `macos/tools/swiftterm-check/main.swift`：喂几条 Kitty 图形的序列（各种入口），有回话就算没关掉，不让编过。
- iTerm2 的 OSC 1337、sixel 的上报在 `TerminalSurface.swift` 里用它公开的接口关。

换版本以后编译停在这两行上：去看上游把分派改成了什么样，照新的写法换（`build.sh` 里的 `ST_KITTY`、`ST_SIXEL`），**不要**为了编过把检查去掉。

## 怎么编

`macos/build.sh` 把它单独编成一个模块 `SwiftTerm`（每个架构一份目标文件和 `.swiftmodule`，放在 `macos/build/swiftterm/`，
源码、编译器、SDK、编译选项都没变时用上次编好的），再和 App 链在一起。不和 App 编进同一个模块：它导出 `Color`、`Terminal`、
`Line` 这些常见的名字。编的时候是 Swift 5 模式（上游按 Swift 6 维护，但有不带锁的静态变量，App 只在主线程用它）、不带
`DEBUG`。只要 Command Line Tools，不要 Xcode、SwiftPM。

## 换版本

1. 把上游那个标签里 `Sources/SwiftTerm/` 下要的文件原样取出来换掉这里的（上面“没带的”照旧不带），`LICENSE` 一起换。
2. 重新生成 `SHA256SUMS`，改 `BuildInfo.swift` 和这一份里的版本、提交。
3. `bash macos/build.sh` 编一遍；API 变了的地方只会影响 `TerminalSurface.swift`。
4. 它新加的、会让终端里的内容碰到这台 Mac 的功能（打开链接和文件、读文件、剪贴板、往磁盘写东西、通知、新的图形协议、新的 APC / DCS / OSC）
   要对着源码看一遍（`EscapeSequenceParser.swift` 的 `dispatchApc` / `dispatchDcs` / `dispatchOsc`），该关的在 `build.sh`、
   `TerminalSurface.swift`、`tools/swiftterm-check` 里补上：终端里的内容来自别的机器（docs/dev/remote.md「界面 App」）。
