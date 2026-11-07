# cca.app（macOS 原生界面）

cca 的图形界面：列出 Claude / ChatGPT（Codex）账号、用量、节点分流，打开每个账号自己的桌面 App、网页版，检测出口，同步路由器。
它只是 cca 的前端：**只经 [PROTOCOL.md](PROTOCOL.md) 和 cca 打交道**（运行 `cca _gui hello|state|live`、`cca [codex] usage|check … --json`、
`cca [codex] app|web KEY`、`cca router sync`、`cca gui update --relaunch PID`），不读 `~/.cca` 里的文件、不解析中文输出、不在 Swift 里重算 cca 的规则。
账号管理（添加、登录、编辑账号表、配置路由器）都交给“终端”里的 cca（PROTOCOL 6 的终端交接）。

## 编译

只要 Command Line Tools（`xcode-select --install`），不要 Xcode：

```bash
bash macos/build.sh                      # universal（arm64 + x86_64），输出 macos/build/cca.app
bash macos/build.sh --arch native        # 只编本机架构（快）
bash macos/build.sh --debug              # -Onone -g，本机架构，带截图模式（见“看界面”）
bash macos/build.sh --version 2.9.0 --out /tmp/x/cca.app
```

- 版本号默认取 `../cca` 里的 `VERSION="…"`：App 的 `CFBundleShortVersionString` 必须和 cca 的版本一样，App 发现 `hello.cca_version` 不同就提示“更新并重新打开”。
- 步骤：`swiftc` 编 CcaKit + CcaApp（universal 时两个架构同时编，再 `lipo`）→ 画图标（`tools/make-icon.swift`，`build/icon/AppIcon.icns` 有缓存，
  脚本改了才重画）→ 写 Info.plist（bundle id `local.cca.app`，只有 zh-Hans 本地化，系统菜单也是中文）→ ad-hoc 签名 → `codesign --verify --strict`
  → 运行 `cca.app/Contents/MacOS/cca --self-check`（解码一份编进去的样例，输出 `ok <版本>`；装了 Rosetta 时 x86_64 也自检）。
- stdout 只输出 .app 的路径，进度写 stderr。冷编译 universal 约 30 秒。

## 测试

```bash
bash macos/test.sh               # 手写的 Tests/Fixtures：通用不变量 + 针对这些数据的断言 + 各部件的单元测试（约 20 秒）
bash macos/test.sh 目录           # 真实的 cca 输出（tests/regress.sh 10g 生成）：只解码 + 通用不变量，缺的文件跳过
```

- `Tests/ContractTests/` 是不用测试框架的小程序（`main.swift` 是入口，其它文件按节分：Harness、Generic、FixtureAssertions、Doubles、
  ProtocolTests、UnitTests、ModelTests）：`check(名字, 条件)` 打印 PASS / FAIL，有失败就非 0 退出。只和 CcaKit 一起编（本机架构）。
- `test.sh` 还检查 CcaKit 没有 `import SwiftUI / AppKit`。临时文件都在 `macos/build/tmp`。
- 通用不变量（两种模式都跑）：每个文件都能解码、`version`/`kind` 对、槽位的端口 = 7900 + 槽位、空槽位是 REJECT、每个工具的账号键不重复、
  第三方 API 账号的 `api` 对象不带 key、`usage` 只给登录着的官方账号、`summary` 和槽位一致、事件逐行解析、consent 的 grant 是 `item:scope`……；
  再把每个文件改一改（加不认识的字段、整数和浮点数互换、不认识的枚举值、类型不对的字段）确认照样解码、不丢元素。
  另有防漂移检查：真实输出里每个枚举字段的值都必须在 App 已知的列表里（`Codes.swift` 的 `allKnown`）；cca 加了新值就先在这里报出来。
- 完整模式另外测：重跑流程（同意 → 带授权重跑 → 成功、不同意、同一个授权再次被要求、最多重跑 3 轮、终端交接、失败取最后一条 fail、
  不弹框时返回 needsConsent、usage 退出码 1 也收 JSON、Codex 没启用时不运行）、闸门、shellQuote（经 `/bin/sh` 还原）、子进程（自己的会话、
  打不开 /dev/tty、超时杀进程组、SIGTERM 后 SIGKILL、取消、2 MB 上限、宽松 UTF-8、后台进程不拖住；输出接文件的取消、超时都不杀）、
  假 cca 脚本走一遍 ProcessTransport（含 web 中途取消照样跑完）、「更新并重新打开」的两步、
  事件文件边写边读、环境变量、找 cca、FSEvents 过滤和合并、终端交接文件、AppModel 的各个流程。

## 演示模式（不运行 cca）

```bash
CCA_GUI_FIXTURES="$PWD/macos/Tests/Fixtures" macos/build/cca.app/Contents/MacOS/cca
```

App 从这个目录取固定的输出（`FixtureTransport`），绝不运行 cca、不开终端、不更新；选中的账号也只记在内存里。可以换文件看别的情况：

| 变量 | 默认 | 例子 |
|---|---|---|
| `CCA_GUI_FIXTURE_HELLO` | hello.json | hello_nocodex_notinstalled.json（安装引导） |
| `CCA_GUI_FIXTURE_STATE` | state.json | state_nocodex.json（账号表校验不过的横幅） |
| `CCA_GUI_FIXTURE_LIVE` | live_router.json（看本机时 live_local.json） | live_unreachable.json |
| `CCA_GUI_FIXTURE_APP` / `_WEB` | events_open.jsonl | events_fail.jsonl（失败说明）、events_terminal.jsonl（终端交接）、events_consent.jsonl（低纯度要同意） |
| `CCA_GUI_FIXTURE_SYNC` | events_sync.jsonl（先要同意重载，同意后成功） | |

开发模式下 `cca gui` 用 `open --env CCA_GUI_CCA=<仓库里的 cca>` 打开 `macos/build/cca.app`，App 优先用这个 cca。

## 看界面（截图）

```bash
bash macos/build.sh --debug --out "$PWD/macos/build/debug/cca.app"
CCA_GUI_FIXTURES="$PWD/macos/Tests/Fixtures" CCA_GUI_SNAPSHOT=/tmp/shots macos/build/debug/cca.app/Contents/MacOS/cca
```

只有调试版（`-D DEBUG`，`Snapshot.swift`）有：演示模式下依次切到几个场景（claude、claude-loggedout、claude-api、codex 是展开了那个账号的账号页，empty 是都收起来，
routing 是分流页，再加上菜单栏面板 menubar），每个截浅色、深色两张 PNG 后退出。`CCA_GUI_SNAPSHOT_SCENES=claude,routing` 只截这几个，
`CCA_GUI_SNAPSHOT_SIZE=1080x1240` 换窗口大小（看整页、看最小的 860 宽）。只截自己的窗口（`SCShareableContent.currentProcess`），
不要“屏幕录制”权限；窗口放在别的窗口后面、不抢焦点，所以截出来是窗口不在前台时的样子（主按钮是灰的）。菜单栏面板是近似的
（真正的面板要点图标才弹出来）。换 `CCA_GUI_FIXTURE_*` 可以截别的情况（比如安装引导）。

## 界面的规矩

- 设计语言参考 [magpie](https://github.com/yetone/magpie)：一个安静的底色（浅色 `#F4F4F6`、深色 `#1A1A1E`），白色的列表卡片
  （圆角 12、1 pt 的边、行与行之间一条细线），黑白为主；强调色是靛蓝（`#4F46E5` / `#A5B4FC`，额度条、主按钮），接近上限换琥珀色、到了上限红色。
  色值和按钮、分段控件、提示条、状态点这些部件都在 `Theme.swift`（色值取自 magpie 的 app.css，浅色、深色两套）。
- 颜色只表达两件事：哪个账号（账号色，就是启动器角标的颜色：头像、分流图里它的线、名字前的点）和什么状态（绿 / 琥珀 / 红；
  失败的点画成叉，只看形状也分得出）。正常时尽量安静。
- 结构：工具栏左边品牌、中间“账号 / 分流”两个页签（分流有问题时带个点）、右边刷新和添加。账号页每个工具一张列表，
  一行一个账号（右边 5h / 周的用量、节点、【打开】），点一行在下面展开详情（用量、线路与出口、网页版 / 打开 App）；
  分流页是一张卡片里的分流图（左边账号 → 中间路由器 / 本机 Clash → 右边节点，每个账号一条自己颜色的线）、统计和有问题的槽位，下面是端口的列表。
  菜单栏面板同一套（白色列表、右上角图标按钮、底部文字按钮）。
- Liquid Glass 只剩系统画的部分（工具栏、页签、弹出的提示）；内容层全是实的。
- 一套系统字体（SF Pro / 苹方），数字一律等宽数字；13 pt 正文，名字 13.5 pt 中粗，第二行 11.5 pt 灰色。

## 结构

```
Sources/CcaKit/        只用 Foundation / Observation / CoreServices（可测试）
  Protocol/            PROTOCOL 的模型：hello / state / live / usage / check / health / 事件；容错解码（Decoding.swift）；样例（Sample.swift）
  Process/             CcaProcess（posix_spawn）、CcaLocator、CcaEnvironment、Transport（进程 / 演示数据）、CcaClient（重跑流程、闸门）、
                       TerminalHandoff（.command 文件、shellQuote）
  Model/               AppModel（整个 App 的状态）、Scheduler（看得见才跑的定时器）、CcaWatcher（FSEvents）、Texts（文字、颜色档、补救、标记）
Sources/CcaApp/        SwiftUI + AppKit：入口和生命周期（CcaApp）、主窗口（MainWindow：工具栏、页签、提示条、引导）、账号页（AccountsPage；
                       UsageCard：用量；RouteSection：线路与出口检测）、分流页（RoutingPage：分流图、端口列表）、菜单栏（MenuBarContent）、
                       对话框和弹层（Prompter）、可见性（Visibility）、色值和部件（Theme）、状态的颜色和头像（Format）、调试版的截图模式（Snapshot）
Tests/ContractTests/   契约测试
Tests/Fixtures/        手写的样例（假数据：US-A…US-D、HK-A、192.0.2.10、203.0.113.x、example.com）
tools/make-icon.swift  画图标
```

要点：

- **进程**：每次调用一个新进程，stdin 接 /dev/null，`POSIX_SPAWN_SETSID` 自己一个会话（cca 打不开 /dev/tty，一律非交互），信号恢复默认，
  只继承 0/1/2；stdout / stderr 分开读，各 2 MB 上限；超时或取消时 SIGTERM 整个进程组，3 秒后 SIGKILL；主进程退出后最多再等 1 秒读管道。
  环境：去掉 `CCA_TTY`、`CCA_YES`、`CCA_GRANT`、`CCA_EVENTS`、`CCA_RO`（还有 `CCA_SKIP_CHECK`），PATH 后面补 `~/.local/bin:/opt/homebrew/bin:/usr/local/bin`，
  设 `CCA_GUI=1`、`NO_COLOR=1`。
- **重跑流程**（PROTOCOL 4.3）在 `CcaClient.execute`：事件文件边运行边读（显示“同步路由器…”之类的进度），要同意时用 NSAlert 问（只用事件里的
  title / question / detail / severity / default），同意了带上 `CCA_GRANT` 重跑。后台自动刷新用量不弹框：记下来显示一条横幅，点了再手动刷新。
- **闸门**：同一个账号（工具 + 键）的 app / web 排队；每个工具的 usage 一次一个；router sync 全局一个。`cca codex …` 只在 `hello.tools.codex.enabled` 为真时运行。
- **刷新**：看得见时（主窗口没被挡住、或菜单栏面板开着）才跑定时器：用量每 15 分钟，分流页开着时 live 每 60 秒。变得看得见时，有账号的用量过期或超过
  5 分钟就刷新。FSEvents 只当信号（accounts.json、codex.json、config.json、*/usage.json、*/health.json、policy/policy.json → state；bin/cca → hello；
  chrome/、desktop/ 下面的不算；500 毫秒合并；看不见时只记下）。Claude App / ChatGPT App 启动、退出时读一次 state；App 被激活时跑 hello 和 state。
- **app、web、router sync 一旦开始就跑完**（PROTOCOL 1）：它们的 stdout / stderr 接到 `~/Library/Caches/local.cca.app/runs/` 里的文件，
  不接管道（App 退出或崩溃时管道没人读，cca 往 stderr 写字会被 SIGPIPE 杀掉：App 可能没打开、路由器同步了却没记下）；不限时、不取消，
  App 退出时也不杀，跑完再读文件（“详情”用 stderr 的最后几行），进度和结果照样看事件文件。退出时只取消 usage / check / live / gui update。
  App 中途退出留下的输出文件、事件文件，一天后清掉。
- **更新并重新打开**（PROTOCOL 5）分两步：先运行 `cca gui update` 等它下载好（失败就显示原因，App 照常开着）；成功后脱离 App 运行
  `cca gui update --relaunch <pid>`（输出接到 runs 目录里的文件），然后退出。
- **终端交接**：在 `~/Library/Caches/local.cca.app/handoff/` 写 `.command`（bash，补 PATH，逐个加单引号运行 cca，结束等按任意键；
  文件一开始就删掉自己，中途关窗口也不留下），用“终端”打开。放了一小时还没运行的会清掉。

## 这套工具链的坑（Command Line Tools，Swift 6.4，SDK 27）

- **`@State` 编不过**：SDK 27 里它是宏，宏插件 SwiftUIMacros 只随 Xcode 提供。界面状态一律放在 `@Observable` 的 AppModel 里，视图里用
  `@Environment(AppModel.self)`，要绑定时在 `body` 里 `@Bindable var model = model`。视图自己的局部状态（悬停）写成
  `private let hover = State(initialValue: false)`（不加 @）：State 类型本身照样是 SwiftUI 的动态属性，读写 `hover.wrappedValue`。同样别用 `@Entry`、`@Previewable`、`#Preview`、`@Animatable`。
  `@Observable`、`@ObservationIgnored`、`@Namespace`、`@Environment`、`@NSApplicationDelegateAdaptor` 能用。
- 没有 xcodebuild / actool / xcstrings：不用 asset catalog，图标用 `sips` + `iconutil` 生成 icns（`iconutil -o` 的文件名必须以 .icns 结尾）。
- 部署目标 macOS 26.0；只有 macOS 27 才有的放在 `if #available(macOS 27, *)` 里（`.pickerStyle(.tabs)`）。
- 编译用 `swiftc -parse-as-library -swift-version 5 -module-name cca`；默认参数里不能调用 `@MainActor` 的初始化方法（在非隔离的上下文里求值）。
- JSON：每个模型都写 `CodingKeys`（下划线的原名），不用 `.convertFromSnakeCase`（它连字典的键都改）；字段一律可缺可 null，类型不对当作没有；
  枚举是 `struct X: RawRepresentable`（不认识的值照样收）；数字整数、浮点都收。
- 图标的轮廓必须是系统的圆角方形（824 × 824，四周留 100，连续圆角 185.4），外面一个像素都不能画，否则 macOS 26 会把整个图标缩小、关进灰框。
  看效果要导出 `NSWorkspace.shared.icon(forFile:)`（系统实际显示的样子）。
- bash 3.2：非 ASCII 字符前的变量写 `${VAR}`（不然变量名会吞掉多字节字符的首字节）。
- 看界面用调试版的截图模式（上面“看界面”）：`ImageRenderer`、`cacheDisplay` 画不出玻璃和 AppKit 控件；`screencapture` 要“屏幕录制”权限，
  App 截自己的窗口（ScreenCaptureKit 的 `SCShareableContent.currentProcess`）不用。
