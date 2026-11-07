# cca.app（macOS 原生界面）

cca 的图形界面：办公室（所有账号一眼看全：每个账号是一位员工，谁在忙、谁在休息、谁没来）、Claude / ChatGPT（Codex）账号、用量、节点分流，
打开每个账号自己的桌面 App、网页版，检测出口，同步路由器。
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
  ProtocolTests、UnitTests、ModelTests、OfficeTests）：`check(名字, 条件)` 打印 PASS / FAIL，有失败就非 0 退出。只和 CcaKit 一起编（本机架构）。
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

App 从这个目录取固定的输出（`FixtureTransport`），绝不运行 cca、不开终端、不更新；选中的账号也只记在内存里。
夹具里的时间（`generated_at`、`checked_at`、`*_epoch`）平移到打开的这一刻（以 state.json 的 `generated_at` 为准），几点重置、几点回来永远像刚生成的一样；
契约测试读原样的文件。可以换文件看别的情况：

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

只有调试版（`-D DEBUG`，`Snapshot.swift`）有：演示模式下依次切到几个场景（claude、claude-loggedout、claude-api、routing、codex、empty、office，
再加上菜单栏面板 menubar、显示细节时的 details、直接改别名的 rename、卡片教程每一步的 tutorial、工位画廊 gallery：每种状态 × 几种档位，
不依赖演示数据，调插画用），每个截浅色、深色两张 PNG 后退出。截图时窗口不在前台，工位画的是静止的那一帧。`CCA_GUI_SNAPSHOT_SCENES=claude,routing` 只截这几个，
`CCA_GUI_SNAPSHOT_SIZE=1080x1240` 换窗口大小（看整页、看最小的 860 宽、`1900x1080` 看全屏时的两栏）。只截自己的窗口（`SCShareableContent.currentProcess`），
不要“屏幕录制”权限；窗口放在别的窗口后面、不抢焦点，所以截出来是窗口不在前台时的样子（主按钮是灰的）。菜单栏面板是近似的
（真正的面板要点图标才弹出来）。换 `CCA_GUI_FIXTURE_*` 可以截别的情况（比如安装引导）。

## 界面的规矩

- 母题是 App 图标里的“分流线”：一个出口分出几条彩色曲线。每个账号有自己的颜色（启动器角标的颜色），这条线贯穿侧栏的头像、
  账号页的线路（这台 Mac → 端口 → 节点 → 出口）和分流图（账号 → 路由器 / 本机 Clash 的端口 → 节点）。
- 颜色只表达三件事：哪个账号（账号色），什么状态（系统的绿 / 橙 / 红，正常时尽量不出现，比如“已登录”不显示），最高档的套餐（金色，只给
  Ultra、Pro 500 这一档：金徽章、头像金圈、金边的工位和工牌；进阶档的银色是金属的灰，不算颜色）。套餐分档由 cca 给（`plan_tier`），App 不按套餐名猜。
  办公室插画里的家具一律中性灰，不带含义。图标的深靛蓝（`Brand`）只用在品牌标记和分流图的面板上，不设全局强调色（按钮、选中跟着用户的系统设置）。
- Liquid Glass 只给控件层：侧栏、工具栏（系统画）、按钮（`.glass` / `.glassProminent`，相邻的放进 `GlassRow`）、浮着的提示。
  内容层（额度砖、线路、分流图、提示条 `Banner`）是实的，色块用 `Surface`，玻璃上不叠玻璃。
- 操作放在它作用的内容旁边：打开 App / 网页版在账号页头，刷新用量在“用量”标题上，检测出口在“网络出口”标题上，添加账号在办公室各组的标题上；
  不用浮在内容上的操作条。
- 办公室（`OfficePage.swift`，首页，侧栏最上面）：每个账号是一位员工，按组坐好（Claude 组、ChatGPT 组、外包 = 第三方 API 账号），座位照账号表的顺序、
  不随状态挪。员工的状态（`Employee`：没来 = 没登录、坐着陌生人 = 登错号、不舒服 = 端口连不上 / 节点选错 / 出口有问题、休息 = 额度用完、在忙、空闲）
  从 `Diagnosis` 派生，和账号页那句话共用一份优先级（顺序只写在 `Diagnosis.all`）；“在忙”= 桌面 App 开着或者有经 cca 启动的命令行会话（`cli`）。
  工位是 Canvas 画的（`DeskArt.swift`，没有图片资源）：显示器上演在干什么，会动的部分只在窗口看得见、没开“减少动态效果”时走，帧率看状态
  （打字 10 帧、其余 4–6 帧，没人的工位不走）。点工位进账号页，右键和侧栏一样；账号页的页头是工牌（`EmployeeBadgeCard`：账号色的带子写着哪个组）。
- 版心（`Glass.swift` 的 `pageColumn()`）：详情页的内容最宽 1400pt、两边各留 28pt，再宽就居中；顶上的提示条、办公室、账号页、分流页都用它，左右边对齐。
  窗口宽的时候（全屏）账号页的用量和网络出口左右并排（`SectionColumns`：每栏能有 500pt 才并排，窄了照旧上下排）；线路每站最宽 300pt，再宽就整条居中。
- 一套系统字体（SF Pro / 苹方），数字一律等宽数字；额度的百分比大号，“%”缩小。
- 照顾新手（`Guide.swift`）：第一次连上 cca 时弹出卡片教程（九张，每张一幅示意图加几条带图标的要点：一台 Mac 好几个账号、办公室、左边的账号列表、
  打开账号、看额度、看网络出口、节点分流、改别名和外观、菜单栏和帮助；下面的小圆点能直接点到任意一张，最后一张能“从头再看”；
  侧栏左下角的“新手教程”、工具栏的问号、“帮助”菜单随时再看）；页面顶上的提示卡片讲一句这一页怎么看，点“知道了”就不再出现（“帮助”菜单可以重新显示）；
  账号页名字下面一句话说现在能不能用、下一步点哪个按钮（只按 cca 给的状态说，最要紧的排前面，顺序在 `Diagnosis`）；端口、槽位、纯净度明细这些技术细节默认收起，
  “显示细节”打开（记住选择）。看过教程、关掉的提示、显示细节都记在 App 的设置里（`guide.*`），演示模式只记在内存里。
- 改别名：直接点账号页上的名字（或者侧栏账号的右键菜单），名字就地变成输入框（`AccountDetail.swift` 的 `AliasTitle` / `InlineAliasEditor`），
  回车保存、Esc 取消 → `cca [codex] rename KEY 新别名`（PROTOCOL 4.4）。别名的规则只由 cca 判断，它说不行时原因写在输入框下面；
  成功后选中 cca 打在 stdout 上的新键；换到别的账号时自动放弃。
- 外观（`Appearance.swift`）：跟随系统 / 浅色 / 深色，侧栏右下角的按钮、“显示”菜单、设置窗口（⌘,）里都能换，用 `NSApp.appearance` 套到所有窗口，
  记在 App 的设置里（`appearance`）。设置窗口里还有新手引导的几个开关。

## 结构

```
Sources/CcaKit/        只用 Foundation / Observation / CoreServices（可测试）
  Protocol/            PROTOCOL 的模型：hello / state / live / usage / check / health / 事件；容错解码（Decoding.swift）；样例（Sample.swift）
  Process/             CcaProcess（posix_spawn）、CcaLocator、CcaEnvironment、Transport（进程 / 演示数据）、CcaClient（重跑流程、闸门）、
                       TerminalHandoff（.command 文件、shellQuote）
  Model/               AppModel（整个 App 的状态）、Scheduler（看得见才跑的定时器）、CcaWatcher（FSEvents）、CoreHost（内置代理的内核）、
                       Texts（文字、颜色档、补救、标记）、Diagnosis（一个账号的问题，按要紧程度排好）、Employee（办公室里的员工、点名）
Sources/CcaApp/        SwiftUI + AppKit：入口和生命周期（CcaApp）、主窗口和提示条（MainWindow）、侧栏（Sidebar）、办公室（OfficePage；工位插画 DeskArt）、
                       套餐徽章（PlanBadge）、账号页（AccountDetail：工牌页头；
                       UsageCard：用量；RouteSection：线路与出口检测）、分流页和分流图（RoutingPage）、菜单栏（MenuBarContent）、对话框和弹层（Prompter）、
                       可见性（Visibility）、颜色和小部件（Format）、玻璃和内容层的组件、版心和两栏（Glass）、新手引导（Guide）、外观和设置窗口（Appearance）、调试版的截图模式（Snapshot）
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
- **刷新**：看得见时（主窗口没被挡住、或菜单栏面板开着）才跑定时器：用量每 15 分钟，分流页开着时 live 每 60 秒。办公室不定时查 live
  （端口连不上、选错节点的员工看它）：切到办公室、主窗口变得看得见时，上次的超过 2 分钟才查一次；分流页、办公室开着时操作完（同步路由器等）也查。变得看得见时，有账号的用量过期或超过
  5 分钟就刷新。FSEvents 只当信号（accounts.json、codex.json、config.json、*/usage.json、*/health.json、命令行会话的登记 accounts/*/cli-<数字>.json、
  policy/policy.json → state；bin/cca → hello；
  chrome/、desktop/ 下面的不算；500 毫秒合并；看不见时只记下）。Claude App / ChatGPT App 启动、退出时读一次 state；App 被激活时跑 hello 和 state。
- **app、web、router sync 一旦开始就跑完**（PROTOCOL 1）：它们的 stdout / stderr 接到 `~/Library/Caches/local.cca.app/runs/` 里的文件，
  不接管道（App 退出或崩溃时管道没人读，cca 往 stderr 写字会被 SIGPIPE 杀掉：App 可能没打开、路由器同步了却没记下）；不限时、不取消，
  App 退出时也不杀，跑完再读文件（“详情”用 stderr 的最后几行），进度和结果照样看事件文件。退出时只取消 usage / check / live / gui update。
  App 中途退出留下的输出文件、事件文件，一天后清掉。
- **更新并重新打开**（PROTOCOL 5）分两步：先运行 `cca gui update` 等它下载好（失败就显示原因，App 照常开着）；成功后脱离 App 运行
  `cca gui update --relaunch <pid>`（输出接到 runs 目录里的文件），然后退出。
- **内置代理**（PROTOCOL 7）：state 里本机客户端是 `builtin` 时，`CoreHost` 运行 `cca _core run`（cca 生成配置后 exec mihomo，这个进程就是内核），
  输出追加进 `~/Library/Caches/local.cca.app/core.log`；退出了按 1、2、4…最多 60 秒退避重启（顶上一条提示，可以“现在重启”），不再需要、App 退出时停掉
  （退出前等它退出）。cca 要内核又没开 App 时用 `open -g … --args --core` 在后台打开它：不弹主窗口、不占 Dock（`CcaApp.launchedForCore`，
  `.defaultLaunchBehavior(.suppressed)`、不恢复上次的窗口、`.accessory`）；“打开主窗口”的回调在菜单栏图标里就登记（这时主窗口、面板都还没建），
  主窗口一出现就换回普通 App（`MainWindow.onAppear`）。`core/config.yaml` 变了是
  CcaWatcher 的 `.core` 信号：看不见时也马上读 state。演示模式不跑内核。
- **终端交接**：在 `~/Library/Caches/local.cca.app/handoff/` 写 `.command`（bash，补 PATH，逐个加单引号运行 cca，结束等按任意键；
  文件一开始就删掉自己，中途关窗口也不留下），用“终端”打开。放了一小时还没运行的会清掉。

## 这套工具链的坑（Command Line Tools，Swift 6.4，SDK 27）

- **`@State` 编不过**：SDK 27 里它是宏，宏插件 SwiftUIMacros 只随 Xcode 提供。界面状态一律放在 `@Observable` 的 AppModel 里，视图里用
  `@Environment(AppModel.self)`，要绑定时在 `body` 里 `@Bindable var model = model`。同样别用 `@Entry`、`@Previewable`、`#Preview`、`@Animatable`。
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
