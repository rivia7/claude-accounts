# cca.app（macOS 原生界面）

cca 的图形界面：总览（所有账号一眼看全：哪个在用、哪个额度用完了、哪个没登录）、Claude / ChatGPT（Codex）/ Antigravity 账号、用量、节点分流，
打开每个账号自己的桌面 App、网页版或终端，检测出口，同步路由器。
它只是 cca 的前端：**只经 [PROTOCOL.md](PROTOCOL.md) 和 cca 打交道**（运行 `cca _gui hello|state|live`、`cca [codex] usage|check … --json`、
`cca [codex] app|web KEY`、`cca codex reset KEY --json`、`cca router sync`、`cca update --check --json`、`cca update 版本号`、
`cca gui update --relaunch PID`），不读 `~/.cca` 里的文件、不解析中文输出、不在 Swift 里重算 cca 的规则。
添加账号、换 API key、删除账号、编辑账号表、配置路由器、配置本机 Clash 在 App 里问完（PROTOCOL 4.6 的提问会话）；登录还交给“终端”里的 cca（PROTOCOL 6 的终端交接）。

## 编译

只要 Command Line Tools（`xcode-select --install`），不要 Xcode：

```bash
bash macos/build.sh                      # universal（arm64 + x86_64），输出 macos/build/cca.app
bash macos/build.sh --arch native        # 只编本机架构（快）
bash macos/build.sh --debug              # -Onone -g，本机架构，带截图模式（见“看界面”）
bash macos/build.sh --version 2.9.0 --out /tmp/x/cca.app
```

- 版本号默认取 `../cca` 里的 `VERSION="…"`：App 的 `CFBundleShortVersionString` 必须和 cca 的版本一样，App 发现 `hello.cca_version` 不同就提示“更新并重新打开”。
- 步骤：`swiftc` 编 CcaKit + CcaApp（universal 时两个架构同时编，再 `lipo`）→ 画图标（`tools/make-icon/main.swift` + `Sources/CcaApp/LogoGlyph.swift`，
  `build/icon/AppIcon.icns` 有缓存，两个文件改了才重画）→ 写 Info.plist（bundle id `local.cca.app`，只有 zh-Hans 本地化，系统菜单也是中文）→ ad-hoc 签名 → `codesign --verify --strict`
  → 运行 `cca.app/Contents/MacOS/cca --self-check`（解码一份编进去的样例，输出 `ok <版本>`；装了 Rosetta 时 x86_64 也自检）。
- stdout 只输出 .app 的路径，进度写 stderr。冷编译 universal 约 30 秒。

## 测试

```bash
bash macos/test.sh               # 手写的 Tests/Fixtures：通用不变量 + 针对这些数据的断言 + 各部件的单元测试（约 20 秒）
bash macos/test.sh 目录           # 真实的 cca 输出（tests/regress.sh 10g 生成）：只解码 + 通用不变量，缺的文件跳过
```

- `Tests/ContractTests/` 是不用测试框架的小程序（`main.swift` 是入口，其它文件按节分：Harness、Generic、FixtureAssertions、Doubles、
  ProtocolTests、UnitTests、ModelTests、OfficeTests、FeatureTests、TerminalTests、PollingTests、ClientUpdateTests、FlowTests、AccountListTests、DevicesTests、APIKeyTests、WeeklyUsageTests、CostTests）：`check(名字, 条件)` 打印 PASS / FAIL，有失败就非 0 退出。只和 CcaKit 一起编（本机架构）。
- `test.sh` 还检查 CcaKit 没有 `import SwiftUI / AppKit`。临时文件都在 `macos/build/tmp`。
- 通用不变量（两种模式都跑）：每个文件都能解码、`version`/`kind` 对、槽位的端口 = 7900 + 槽位、空槽位是 REJECT、每个工具的账号键不重复、
  第三方 API 账号的 `api` 对象不带 key、`usage` 只给登录着的官方账号、`usage_weeks` 自洽（按时间排好、还没过完的最多一周且是最后一项、统计的是过完的那几周）、折合金额 `cost` 自洽（周期按时间排好、首尾相接、最多 13 项、`current` 只有最后一项，`days` 正好 30 天，每台设备的 `weeks_usd` / `days_usd` 和周期一一对应，数字不是负的，同步状态和扫描状态的值认识或者能降级显示）、`summary` 和槽位一致、事件逐行解析、consent 的 grant 是 `item:scope`……；
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

提问会话（添加账号、换 key、删除账号）回放 `flow_<参数用 _ 连起来>.flow`（`FixtureTransport.startSession`）：每行一个 `ask`、`step`、`result`、`fail` 事件。

开发模式下 `cca gui` 用 `open --env CCA_GUI_CCA=<仓库里的 cca>` 打开 `macos/build/cca.app`，App 优先用这个 cca。

## 看界面（截图）

```bash
bash macos/build.sh --debug --out "$PWD/macos/build/debug/cca.app"
CCA_GUI_FIXTURES="$PWD/macos/Tests/Fixtures" CCA_GUI_SNAPSHOT=/tmp/shots macos/build/debug/cca.app/Contents/MacOS/cca
```

只有调试版（`-D DEBUG`，`Snapshot.swift`）有：演示模式下依次切到几个场景（claude、claude-loggedout、claude-api、routing、codex、agy（要带 `CCA_GUI_FIXTURE_HELLO=hello_agy.json CCA_GUI_FIXTURE_STATE=state_agy.json`）、empty、office、client-updates、stats（用量统计页）、cost / cost-wide / cost-day / cost-me（折合金额页：所有设备按周、宽窗口 1680×1000、按日、本机）、account-cost / account-cost-day / account-cost-me / account-cost-empty / account-cost-codex（账号页的「折合金额」：按周、按日、本机、还没有记录、ChatGPT 账号）、pinned（有置顶账号的总览和侧栏），
再加上提问会话每一页的 flow-*、菜单栏面板 menubar、显示细节时的 details（账号页、分流页、折合金额）、直接改别名的 rename、卡片教程每一步的 tutorial、每个版本“更新了什么”的 whatsnew、
检查更新找到新版本时顶上的提示条 update、插画画廊 gallery：每种状态 × 几种档位，不依赖演示数据，调插画用；office 是总览页），每个截浅色、深色两张 PNG 后退出。截图时窗口不在前台，插画是静止的那一帧。`CCA_GUI_SNAPSHOT_SCENES=claude,routing` 只截这几个，
`CCA_GUI_SNAPSHOT_SIZE=1080x1240` 换窗口大小（看整页、看最小的 860 宽、`1900x1080` 看全屏时的两栏）。只截自己的窗口（`SCShareableContent.currentProcess`），
不要“屏幕录制”权限；窗口放在别的窗口后面、不抢焦点，所以截出来是窗口不在前台时的样子（主按钮是灰的）。菜单栏面板是近似的
（真正的面板要点图标才弹出来）。换 `CCA_GUI_FIXTURE_*` 可以截别的情况（比如安装引导）。

## 界面的规矩

- 母题是 App 图标里的“分流线”：一个出口分出几条彩色曲线。每个账号有自己的颜色（启动器角标的颜色），这条线贯穿侧栏的头像、
  账号页的线路（这台 Mac → 端口 → 节点 → 出口）和分流图（账号 → 路由器 / 本机 Clash 的端口 → 节点）。
- Logo 和品牌色参考 Google 2025 年后的图标语言（一个主色、颜色互相渐变过去、粗圆扁平，不加金属高光和阴影），用 cca 自己的形和颜色：
  一根粗圆的线分成三支，主干蓝色，三支分开以后才分别流向玫红、青绿、琥珀。字形和颜色只写在 `Sources/CcaApp/LogoGlyph.swift` 一处：
  App 图标（白底上的这个字形，`tools/make-icon/main.swift`，build.sh 把 LogoGlyph.swift 一起编进去）、App 里不带底的 `BrandMark`、
  `Brand` 的颜色都取它；“总览”的图标是同一套颜色的 2×2 圆角方块 `OverviewMark`；“客户端更新”的图标 `UpdatesMark` 是一支箭头落进敞口的盒子，
  同样粗圆的线（边长的 0.15），蓝渐变到青绿（整条 Logo 渐变只给最高档的套餐）；“用量统计”的 `StatsMark` 是三根高低不同的柱子，蓝渐变到紫；
  “折合金额”的 `CostMark` 是美元符号（一笔 S，上下露出一小截竖线），琥珀渐变到玫红（Logo 末端的暖色）。
- 以信息简洁易懂为主，不为装饰上色、画框。颜色只表达三件事：哪个账号（账号色），什么状态（系统的绿 / 橙 / 红，正常时尽量不出现，
  比如“已登录”不显示；提示、建议用系统的强调色），套餐的档位。档位要低调（`PlanBadge.swift`）：徽章和 `Tag` 一样是中性的胶囊、secondary 的字，颜色只在前面的小徽记上，
  一档比一档足——入门灰色圆圈、标准蓝色圆点、进阶紫色菱形、尊贵（Ultra、Pro 500）Logo 渐变（`Brand.spectrum`：蓝 → 紫 → 玫红 → 琥珀）的四角星。
  这条渐变只给最高档的两样东西：这颗星，和**侧栏**里它头像的光晕（一道细的渐变环，环外很淡的一层柔光，环和头像之间留一道透明的缝；
  这个账号在用时慢慢流动，6 秒一圈，转的是画好的一层、不每帧重画模糊）。光晕只在侧栏：账号页、总览、菜单栏、分流图的头像都不带；
  侧栏的一格只到头像的边，画到格子外面的会被裁掉，所以每一行左边都留出光晕的宽度（`sidebarRowPadding`，`BadgeAvatar.haloRoom`）。
  卡片不按档位描边上色，插画也不画档位。画法都在 `TierStyle`。套餐分档由 cca 给（`plan_tier`），App 不按套餐名猜。
  总览插画里的家具一律中性灰，不带含义。不设全局强调色（按钮、选中跟着用户的系统设置）。
- 持续的动画（侧栏的光晕流动、总览的插画）只在 `motionAllowed` 时走：主窗口、菜单栏面板的根上 `.motionScope(\.mainVisible / \.panelVisible)`
  按窗口看不看得见、开没开“减少动态效果”设一次（`Visibility.swift`），下面的视图只读它；没设的地方（截图、教程的示意图）不动。
- 用量一律画成用量条（`Meter`）：灰色的底槽是额度，两头圆，数据旧了、不全时填灰色（不靠透明度）；按用量的档上色（绿 / 橙 / 红），
  过去的（每周用量）也一样：同一个百分比在哪儿都是同一个颜色。用量条不填账号色：账号色里也有红、绿、橙，51% 的一周填成红色就像快用完了
  （账号靠头像和名字认）。图表也照这一套，不另起一套图表的样子：每周用量是一排竖着的用量条，不画坐标轴、网格线和每根上的数字
  （数字在旁边的一行字和读屏里），悬停和卡片、列表行一样垫一层浅底（`primary` 6%）。
- Liquid Glass 只给控件层：侧栏、工具栏（系统画）、按钮（`.glass` / `.glassProminent`，相邻的放进 `GlassRow`）、浮着的提示。
  内容层（额度砖、线路、分流图、提示条 `Banner`）是实的，色块用 `Surface`，玻璃上不叠玻璃。
- 玻璃上的文字：深色外观下系统给文字用的混色（vibrancy）让灰字的对比度只有约 3:1，小字发虚。菜单栏面板的根上用 `plainTextOnDarkGlass()` 清掉 `backgroundMaterial`
  （约 6:1，和内容区一样；浅色外观下这层混色反而让字更深，不动）。侧栏的列表不认它，侧栏和账号行里的灰字用 `.plainSecondary`（深色外观、窗口在前台时是不混色的 55% 白，约 5.6:1，别的情况照系统的），
  新加灰字时也用它；里面不要再套一层 `.secondary`（基色是具体颜色时会叠加变暗）。
- 操作放在它作用的内容旁边：打开 App / 网页版 / 终端在账号页头，刷新用量在“用量”标题上，「重置额度…」在“还能重置额度 N 次”那一行（ChatGPT 账号），
  检测出口在“网络出口”标题上，「刷新」设备在“设备”标题上，添加账号在侧栏账号列表和总览各组的标题上；不用浮在内容上的操作条。
- 按钮只有几种样子（`Glass.swift`）：一节标题上的操作是 `SectionAction`，一行信息（`InfoRow`）右边的操作是 `RowAction`——两种一个样子：玻璃、
  图标加文字、常规大小，右边和标题上的按钮对齐（信息行不缩进）；行里的按钮标签有最小宽度，同一节里上下一样宽、左边也对齐，按钮里的图标和文字靠左（图标占固定的宽度：上下几个按钮的图标排成一列，文字从同一处开始）。打开网页的
  （在账号自己的浏览器里）用 `RowAction.web`，图标是 ↗；「显示细节」是 `DetailsToggle`，和标题上别的按钮一样。提示条里的按钮小一号：
  解决问题的那个 `.borderedProminent`，其余 `.bordered`。账号页头的主操作大一号（`.controlSize(.large)`）。菜单栏面板账号行里的「打开」是重复出现的
  列表按钮，保持小的文字按钮。
- 账号页的设备（`DevicesSection.swift`，只有 cca 说 `can.devices` 的账号有，PROTOCOL 3.4）：在两栏下面占一整行，这台 Mac 排第一；状态点只表达状态
  （在用实心绿点、闲置绿圈、离线灰圈、没在用浅灰点），文字照实说（在用、开着，闲置 N 分钟、离线、没在用 · 上次使用）；离线的原因（可能休眠、断网）
  放在悬停说明里，怎么判断在用、闲置放在「显示细节」里。账号页看得见时一分钟查一次，看不见、换到别的页就停。
- 侧栏（`Sidebar.swift`）：最上面是几种工具共用的几页——总览、用量统计、折合金额（cca 有它时）、节点分流、客户端更新，切工具时不变；
  下面是工具切换（Claude / ChatGPT / Antigravity，启用了两种以上才有，`ToolPickerRow`，选不中的一行）和这个工具的账号。切换只管它下面列哪个工具的账号，
  所以放在账号列表正上方，不放在侧栏顶上（放在顶上像是连上面那几页也跟着换）。一页只放一种图：额度用量（有底槽、填满就是用完）和折合金额
  （美元没有上限、画纵轴）分成「用量统计」「折合金额」两页，不在一页里切换。
- 账号的排序和置顶（`AccountArrangement`）：侧栏上下拖、总览里拖到别的卡片上，右键或账号页的「…」置顶。顺序以账号表为准：调整之后 App 写回账号表（`cca reorder`，PROTOCOL 4.7），终端里的菜单和编号也是这个顺序，在终端里改了顺序 App 也跟着变；
  写回之前界面先按拖出来的顺序显示，不等 cca。置顶记在 App 的设置里，账号按 `Account.stableID`（工具 + 账号目录名）认，改别名不丢。分组和总览一样：Claude、ChatGPT、第三方 API（两个工具共用一组，侧栏只显示当前工具的，拖动时只换这几个之间的先后）。置顶的只在置顶的里面挪，拖过界的停在边上。
- 重新登录（`AppModel.relogin`，`cca [codex] login 账号`，在终端里）：右键或账号页的「…」随时可以；登录还好好的账号先弹窗（它会先退出现在的登录），登录失效的直接去终端。登录失效是查用量时才知道的（`error_code` 是 `auth`，`Diagnosis.loginExpired`）：Claude Code 不说原因（`nolimits`），用量那一行旁边放个「重新登录…」。
- 账号页的用量（`UsageCard.swift`）：每个额度一大块（5 小时、本周，还有按模型的每周额度，例如 Claude 的 Fable。Fable 用的是本周总额度的一部分，
  不是另外一份：下面一句（`Texts.modelShare`）写“Fable 最多占本周额度的 50%，已占 6%”，它那份用完了时写“Fable 的份额用完了，别的模型还能用本周
  剩下的 49%”；名字写在句子里，只写“最多用本周额度的 50%”会被看成整个账号最多只能用一半。份额由 cca 认出来给，过了重置时间不说），`QuotaGrid` 排：放得下就一行，放不下两块一行，单出来的最后一块占满一行，同一行的一样高。旧的分模型额度、
  credits、额外用量是下面的小行；再下面是一行一行的信息（`InfoRow`）：ChatGPT 账号还能重置几次、最早的一次什么时候过期（「重置额度…」，cca 先问，
  PROTOCOL 4.5），查用量出的错，Claude Code 记的套餐过时了（「重新登录…」），只能在网页上看的重置次数（「在 claude.ai 查看」）和订阅附带的
  每月 API 额度（「在控制台查看」），API key（掩码、一只眼睛、「复制」，`APIKeySection.swift`）。
  侧栏、菜单栏面板、总览卡片里账号名字右边是同样几块的小条（`UsageStack`）：5 小时、本周，分模型的每周额度（Fable）各一行，
  标签靠右、条和百分比上下对齐，鼠标指着 Fable 那行说的是同一句；模型名的标签宽 26（刚好放下 Fable，再宽总览卡片最窄时状态那句要折行），
  放不下先缩小再截断。有 Fable 的卡片多一行，比没登录的高一截：同一排的卡片撑成一样高（`frame(maxHeight: .infinity)`），多出来的空在下面。
- 周结算提醒（cca 给的 `use_first`，PROTOCOL 3.1）：本周额度快重置了、还剩不少，没有挡路的问题时（`Diagnosis.useFirst` 排在最前），账号页那句话说
  「本周额度还剩 59%，明天 19:55 重置，建议先用这个账号。」，总览卡片右下角、侧栏和菜单栏面板的名字后面一个沙漏（`UseFirstMark`）。沙漏用强调色：
  是提示不是警告（绿是在用，橙、红是问题）；不算进“要处理”。
- 每周用量（cca 给的 `usage_weeks`，PROTOCOL 2.2）：账号页「用量」下面的「每周用量」（`WeeklyUsage.swift`）是最近 12 周的一排竖条：和额度砖里的用量条（`Meter`）同一种条，只是竖着——灰色的底槽是这一周的额度，从下往上填到用了多少，填满就是用完，和额度砖一样按用了多少上色（绿 / 橙 / 红）；结束前一天没查到的填灰色（实际可能更多；和数据旧了的用量条一样，不靠透明度），中间重置过、超过 100% 的整根高出底槽（哪一段是重置前的 cca 不给，所以不在 100% 处断开）。用得很少时不像横着的条那样至少画一个圆点（竖着的条短，会显得比实际多），填的那段按底槽的形状裁。每根条在它那一周的正中间，日期写在正下方；不画坐标轴、网格线和每根上的数字：鼠标指着一周时那一列垫一层浅底（和卡片、列表行的悬停一样）、下面那一行换成这一周的细节，平时是统计（记下的几周平均用到多少、几周用完、几周没用到一半），读屏每根条都念。侧栏「总览」下面的「用量统计」（`UsageStatsPage.swift`）把官方账号排在一起比：Claude、ChatGPT 各一组，和总览一样的顺序，每个账号一行（头像和名字、小图（同样的竖条，和额度砖里的条一样细）、平均每周、本周），所有行同一个时间范围、同一个纵轴高度（有超过 100% 的就一起抬高），上下对得齐、高低能比；点一行进账号页。第三方 API 账号没有订阅的额度，不在里面。数字和哪一周填灰色都是 cca 给的；以前的用量官方不给，从 cca 开始记的那一周起一周一周长出来。
- 折合金额（cca 给的 `cost`、`cost_total`，PROTOCOL 2.2、3.5）：按官方 API 的价格把用掉的 token 折成美元，**不是实际花费**：每个图下面一直有一行小字说按谁的价格（Claude 账号 Anthropic、ChatGPT 账号 OpenAI）、价格表是哪天的、订阅不按这个收费。账号页「每周用量」下面的「折合金额」（`CostSection.swift`）和侧栏的「折合金额」页（`CostStats.swift`）是同一种柱子（`CostBarChart`）：和每周用量的图共用排法（`SlotChart.swift`：一项占它那段时间、柱子在正中间、日期在正下方、悬停垫浅底），柱子的形状也一样（两头圆，很少时按形状裁，不至少画一个圆点），但不填颜色：钱多少没有好坏，柱子一律中性的 `primary` 62%，不用账号色、也不用用量的绿 / 橙 / 红；开始时还没有记录的那一项（`partial`）填灰色（`Meter.dimColor`，和没记全的那一周一样）。美元没有上限、也没有“填满”，所以没有底槽，大图画纵轴：右边三个刻度（0、一半、最高，最高取整齐的数、最高的柱子最多到九成）和很淡的横线（`primary` 8%），账号页上面的「每周用量」在右边留出同样宽的一栏（`AxisColumn`，按同样的刻度撑开），两张图的绘图区一样宽、同一周的条上下对齐；折合金额页每一行的小图只画底下一条线，所有行同一个时间范围、同一个纵轴（才能比高低），小图放不下 120 点时（窗口窄）不画、表头不写日期。两页的标题右边都有同样的两个切换、同样的顺序：「本机 | 所有设备」（`CostScopePicker`：只算这台设备，或者各台设备加起来的整个账号）和「周 | 日」（`CostGranularityPicker`），两页共用一个设置（`AppModel.costScope`、`costGranularity`，记在 App 的设置里），从折合金额页点进账号页看到的和那一行一样；折合金额页的标题一行放不下时（窗口窄）这两个和「刷新」挪到标题下面一行、右对齐。折合金额页按日时每一行的小图是最近 30 天、右边三栏换成平均每天、今天、30 天合计，顶上的合计按日时多一张 30 天的柱状图（各账号的周期不一样，按周没有加起来的柱子，只有三个数）；看所有设备时有两台以上设备的账号下面缩进、灰字列出各台设备（点一台进账号页），看本机时不列。按周的周期和每周用量的一样（额度窗口，没有的时段按周一零点切），按日是最近 30 天。数字（每一项、平均、合计、本机的那一份、哪一项是灰色、同步的状态）都照 cca 给的，App 只排版、不重算；看所有设备时同步出错、有设备没算进来、很久没同步上来的设备用橙色一行说（折合金额页在名字后面一个橙色三角），只算了这台设备（不在映射里、关掉了设备统计、服务器还不支持）、还没同步过、没登录这个账号用灰字说；看本机时这些都不说（这台设备自己的数字不经同步）。token、没有公开价格的模型、从什么时候开始有记录放在「显示细节」里。
- API key（PROTOCOL 4.10）：平时只显示 cca 给的掩码；点眼睛才向 cca 要一次明文（`KeyReveal`，不被观察、不进读屏，30 秒后、再点一下、换账号、
  离开页面时收起来）；「复制」写进剪贴板时标成机密，60 秒后还是它就清掉（`SecretClipboard`），提示里不写 key。官方账号的 key 只是备查。
- 总览（`OfficePage.swift`，首页，侧栏最上面）：每个账号一张卡片，按 Claude 账号、ChatGPT 账号、Antigravity 账号（启用了才有）、第三方 API 账号分开排，按用户排的顺序（没排过的照账号表）、
  不随状态挪。画面设计上参考办公室（每张卡片画一个工位：桌子、显示器、椅子、坐着的人），**界面上的文字照实说**（账号、在用、空闲、额度用完、
  几点恢复、没登录、登错号、有问题），不写“办公室、员工、工位、在忙、休息、回来”这类比喻的词；代码里的类型名（`Office`、`Employee`）照设计参考起的，
  不出现在界面上。状态（`Employee` 的 mood：没登录 = 椅子空着、登错号 = 坐着灰色的人、有问题 = 端口连不上 / 节点选错 / 出口有问题、额度用完 = 趴在桌上、在用、空闲）
  从 `Diagnosis` 派生，和账号页那句话共用一份优先级（顺序只写在 `Diagnosis.all`）；“在忙”= 桌面 App 开着或者有经 cca 启动的命令行会话（`cli`）。
  插画是 Canvas 画的（`DeskArt.swift`，没有图片资源）：显示器上演在干什么，会动的部分只在窗口看得见、没开“减少动态效果”时走，帧率看状态
  （打字 10 帧、其余 4–6 帧，没人的不走）。ChatGPT 账号有重置机会时，插画左下角一张小票一次（最多画 5 张，再多写 +N），旁边是最早什么时候过期，只用中性灰。点卡片进账号页，右键和侧栏一样。账号页的页头（头像、名字、标签、按钮、那句话）直接放在页面上，不套卡片、不上底色。
- 版心（`Glass.swift` 的 `pageColumn()`）：详情页的内容最宽 1400pt、两边各留 28pt，再宽就居中；顶上的提示条、总览、账号页、分流页都用它，左右边对齐。
  窗口宽的时候（全屏）账号页的用量和网络出口左右并排（`SectionColumns`：每栏能有 500pt 才并排，窄了照旧上下排）；线路每站最宽 300pt，再宽就整条居中。
- 一套系统字体（SF Pro / 苹方），数字一律等宽数字；额度的百分比大号，“%”缩小。
- 照顾新手（`Guide.swift`）：第一次连上 cca 时弹出卡片教程（九张，每张一幅示意图加几条带图标的要点：一台 Mac 好几个账号、总览、左边的账号列表、
  打开账号、看额度、看网络出口、节点分流、改别名和外观、菜单栏更新和帮助；下面的小圆点能直接点到任意一张，最后一张能“从头再看”；
  侧栏左下角的“新手教程”、工具栏的问号、“帮助”菜单随时再看）；升级后第一次打开时一张张给你看这几个版本“更新了什么”（和教程一样的卡片，
  一个版本一张，跳过了几个版本就都给；第一次装只看教程；“帮助”菜单的「更新说明」能再看）。内容写在 CcaKit 的 `Model/WhatsNew.swift`
  （`ReleaseNote`，最新的在前），**每个版本都要写**：release.sh 发布前查这里有没有这个版本的一条，没有就不发布（顺便看看教程要不要跟着改）；页面顶上的提示卡片讲一句这一页怎么看，点“知道了”就不再出现（“帮助”菜单可以重新显示）；
  账号页名字下面一句话说现在能不能用、下一步点哪个按钮（只按 cca 给的状态说，最要紧的排前面，顺序在 `Diagnosis`）；端口、纯净度明细这些技术细节默认收起，
  “显示细节”打开（记住选择）。看过教程、关掉的提示、显示细节都记在 App 的设置里（`guide.*`），演示模式只记在内存里。
- 改别名：直接点账号页上的名字（或者侧栏账号的右键菜单），名字就地变成输入框（`AccountDetail.swift` 的 `AliasTitle` / `InlineAliasEditor`），
  回车保存、Esc 取消 → `cca [codex] rename KEY 新别名`（PROTOCOL 4.4）。别名的规则只由 cca 判断，它说不行时原因写在输入框下面；
  成功后选中 cca 打在 stdout 上的新键；换到别的账号时自动放弃。
- 检查更新：窗口或菜单栏面板看得见时一天最多自动查一次（`cca update --check --json`，PROTOCOL 5.1；关掉了自动更新、开发模式不自动查），
  “cca”菜单的「检查更新…」、侧栏左下角的版本号随时查。跟着现在装的版本：测试版上也提示更新的测试版。有新版本时顶上一条提示（版本号、
  更新说明的前两行，「更新说明」打开发布页）：「更新并重新打开」先把 cca 换成它、再换上同版本的 App；「以后再说」这个版本不再提示。
- 外观（`Appearance.swift`）：跟随系统 / 浅色 / 深色，侧栏右下角的按钮、“显示”菜单、设置窗口（⌘,）里都能换，用 `NSApp.appearance` 套到所有窗口，
  记在 App 的设置里（`appearance`）。设置窗口里还有新手引导的几个开关。

## 结构

```
Sources/CcaKit/        只用 Foundation / Observation / CoreServices（可测试）
  Protocol/            PROTOCOL 的模型：hello / state / live / usage / check / health / 重置额度（Reset）/ 检查更新（Update）/ 设备（Devices）/ 折合金额（Cost）/ 事件；
                       容错解码（Decoding.swift）；样例（Sample.swift）
  Process/             CcaProcess（posix_spawn）、CcaLocator、CcaEnvironment、Transport（进程 / 演示数据）、CcaClient（重跑流程、闸门）、
                       TerminalHandoff（.command 文件、shellQuote）
  Model/               AppModel（整个 App 的状态）、Scheduler（看得见才跑的定时器）、CcaWatcher（FSEvents）、CoreHost（内置代理的内核）、
                       Texts（文字、颜色档、补救、标记）、Diagnosis（一个账号的问题，按要紧程度排好）、Employee（总览里一个账号的状态、点名）、
                       WhatsNew（每个版本更新了什么、版本号的先后）、APIKey（API key 的明文只在这里：KeyReveal、机密剪贴板）、
                       CostView / CostTexts（折合金额的图怎么排：纵轴的刻度、横轴的范围、写哪些日期；金额、token 数和提示的文字）
Sources/CcaApp/        SwiftUI + AppKit：入口和生命周期（CcaApp）、主窗口和提示条（MainWindow）、侧栏（Sidebar）、总览（OfficePage；插画 DeskArt）、
                       套餐徽章（PlanBadge）、账号页（AccountDetail：页头；
                       UsageCard：用量；WeeklyUsage：每周用量的图（一排竖着的用量条）；CostSection：折合金额的一节和柱状图（CostBarChart）；SlotChart：按时间排的柱状图的骨架（每周用量、折合金额共用）和纵轴那一栏（AxisColumn）；APIKeySection：API key 那一行和第三方 API 账号的一节；RouteSection：线路与出口检测；DevicesSection：连着哪些设备）、用量统计（UsageStatsPage）、折合金额（CostStats）、分流页和分流图（RoutingPage）、菜单栏（MenuBarContent）、对话框和弹层（Prompter）、
                       可见性（Visibility）、颜色和小部件（Format）、玻璃和内容层的组件、版心和两栏（Glass）、新手引导（Guide）、外观和设置窗口（Appearance）、调试版的截图模式（Snapshot）
Tests/ContractTests/   契约测试
Tests/Fixtures/        手写的样例（假数据：US-A…US-D、HK-A、192.0.2.10、203.0.113.x、example.com）
tools/make-icon/       画图标（字形取 Sources/CcaApp/LogoGlyph.swift）
```

要点：

- **进程**：每次调用一个新进程，stdin 接 /dev/null，`POSIX_SPAWN_SETSID` 自己一个会话（cca 打不开 /dev/tty，一律非交互），信号恢复默认，
  只继承 0/1/2；stdout / stderr 分开读，各 2 MB 上限；超时或取消时 SIGTERM 整个进程组，3 秒后 SIGKILL；主进程退出后最多再等 1 秒读管道。
  环境：去掉 `CCA_TTY`、`CCA_YES`、`CCA_GRANT`、`CCA_EVENTS`、`CCA_RO`、`CCA_PROMPT`（还有 `CCA_SKIP_CHECK`），PATH 后面补 `~/.local/bin:/opt/homebrew/bin:/usr/local/bin`，
  设 `CCA_GUI=1`、`NO_COLOR=1`。
- **重跑流程**（PROTOCOL 4.3）在 `CcaClient.execute`：事件文件边运行边读（显示“同步路由器…”之类的进度），要同意时用 NSAlert 问（只用事件里的
  title / question / detail / severity / default），同意了带上 `CCA_GRANT` 重跑。后台自动刷新用量不弹框：记下来显示一条横幅，点了再手动刷新。
- **闸门**：同一个账号（工具 + 键）的 app / web / 改别名排队，key --show / --forget 也是（查设备由 AppModel 保证同一个账号一次一个）；每个工具的 usage 一次一个；同一个账号的重置额度一次一个；router sync 全局一个；
  更新 cca（`update 版本号`）和 gui update 全局一个（一个接着一个）。`cca codex …` 只在 `hello.tools.codex.enabled` 为真时运行。
- **刷新**：用量自动刷新不管看不看得见（`Scheduler` 每分钟看一眼，`UsagePolling` 只挑到期的账号，到期了才起进程）：在用的账号看得见时 5 分钟一次、看不见时 10 分钟，没在用的看得见时 8 分钟、看不见时 1 小时，低电量模式慢一倍；用户 15 分钟没碰电脑、系统过热时不查，查失败也按间隔重试。登录失效的账号慢下来（看得见时 15 分钟、看不见时 1 小时），点了“重新登录”之后的 15 分钟里每分钟查一次，登录一完成就看得到。网络出口也定时检测（同一个定时器，`ExitPolling`，`cca [codex] check 这几个账号`，比查用量慢得多：在用的账号看得见时半小时、看不见时 1 小时，没在用的 2 小时、4 小时，低电量模式慢一倍；一次最多 6 个，最久没测的先测；没登录的账号不测；正在查用量时等下一分钟；不弹框、不占用账号的按钮）。自动刷新和后台检查客户端更新都以 utility 优先级运行（子进程继承），定时器带容差。折合金额（`cca cost sync --json`，PROTOCOL 3.5，`CostPolling`）跟着同一个定时器：`hello.cost` 有、`cost_total` 不是 null 时，看得见金额（折合金额页、有「折合金额」的账号页）5 分钟一次、看不见 30 分钟一次，还没统计过（`scan.state` 是 `never`）马上一次；用户走开了、过热时不跑，没统计成按间隔重试、原因写在那一节下面；「刷新」带 `--now`、不降优先级。分流页开着时 live 每 60 秒。总览不定时查 live
  （端口连不上、选错节点的账号看它）：切到总览、主窗口变得看得见时，上次的超过 2 分钟才查一次；分流页、总览开着时操作完（同步路由器等）也查。变得看得见时，有账号的用量过期或超过
  5 分钟就刷新。FSEvents 只当信号（accounts.json、codex.json、config.json、*/usage.json、*/health.json、命令行会话的登记 accounts/*/cli-<数字>.json、
  policy/policy.json、折合金额的 cost/local.json、cost/remote.json、cost/sync.json → state；bin/cca → hello；
  chrome/、desktop/ 下面的不算；500 毫秒合并；看不见时只记下）。Claude App / ChatGPT App 启动、退出时读一次 state；App 被激活时跑 hello 和 state。
- **app、web、router sync、重置额度一旦开始就跑完**（PROTOCOL 1）：它们的 stdout / stderr 接到 `~/Library/Caches/local.cca.app/runs/` 里的文件，
  不接管道（App 退出或崩溃时管道没人读，cca 往 stderr 写字会被 SIGPIPE 杀掉：App 可能没打开、路由器同步了却没记下、重置额度用掉了一次却没记下）；
  不限时、不取消，App 退出时也不杀，跑完再读文件（“详情”用 stderr 的最后几行），进度和结果照样看事件文件。退出时只取消 usage / check / live /
  检查更新 / 更新 cca / gui update。
  App 中途退出留下的输出文件、事件文件，一天后清掉。
- **更新并重新打开**（PROTOCOL 5、5.1）：检查更新找到新版本时先 `cca update 版本号` 把 cca 换成它（失败就显示原因，什么都没换）；
  然后（cca 已经比 App 新时从这一步开始）运行 `cca gui update` 等它下载好（失败就显示原因，App 照常开着）；成功后脱离 App 运行
  `cca gui update --relaunch <pid>`（输出接到 runs 目录里的文件），然后退出。
- **内置代理**（PROTOCOL 7）：state 里本机客户端是 `builtin` 时，`CoreHost` 运行 `cca _core run`（cca 生成配置后 exec mihomo，这个进程就是内核），
  输出追加进 `~/Library/Caches/local.cca.app/core.log`；退出了按 1、2、4…最多 60 秒退避重启（顶上一条提示，可以“现在重启”），不再需要、App 退出时停掉
  （退出前等它退出）。cca 要内核又没开 App 时用 `open -g … --args --core` 在后台打开它（`CcaApp.launchedForCore`：什么情况下都不自己弹主窗口）。`core/config.yaml` 变了是
  CcaWatcher 的 `.core` 信号：看不见时也马上读 state。演示模式不跑内核。
- **后台运行**：平时只有菜单栏图标（`LSUIElement`、`.accessory`，`.defaultLaunchBehavior(.suppressed)`，不恢复上次的窗口）。`cca gui`、更新后重新打开带 `--window`（`CcaApp.opensWindowAtLaunch`，演示模式也是）启动时开主窗口；第一次打开、cca 没准备好时连上之后也开。`WindowOpener.showMain` 换成 `.regular`（有 Dock 图标），刚换时激活不一定马上生效，稍等再激活一次、把窗口放到最前面；最后一个窗口（主窗口、设置）关掉时换回 `.accessory`，焦点还给之前的 App。Dock 里的“退出”、⌘Q 只回到后台（`applicationShouldTerminate` 返回 `.terminateCancel`、`WindowOpener.toBackground`），真的退出走 `AppExit.quit`（菜单栏面板和应用菜单的「退出」、更新后重新打开）；注销、重启、关机（`NSWorkspace.willPowerOffNotification`、退出事件里的 `kAEQuitReason`）和没有 Dock 图标时收到的退出请求也真的退出。
- **安装命令**（PROTOCOL 2.1 的 `hello.install_cli`）：账号的终端打不开是因为没装 claude / codex / agy 命令（`can_why.cli` 是 `tool_missing`）时，账号页头“没装 X 命令”那句旁边有「安装 X 命令…」
  （`AppModel.installCLIHandoff`；提示那一行的按钮，小一号的 `.bordered`），`fail` 的 `tool_missing` 也补救成它：在系统终端里运行 `cca [codex|agy] install-cli`
  （官方安装脚本，直连不通时 cca 在终端里问要不要经内置代理装）。回到 App 时照常重新读 hello 和 state。旧版 cca 没有这个字段时不给。
- **请求锁定到映射**（PROTOCOL 4.8）：账号的「…」和右键菜单里的「请求维护者锁定节点」（`can.policy_lock`）运行 `cca [codex] policy request`，把 stdout 的那条命令复制给用户发给维护者。
- **查看 API 额度**：账号页“用量”一节底部“订阅附带的每月 API 额度”那一行的「在控制台查看」（右键菜单里的「在控制台查看 API 额度」；`can.billing`，第三方 API 账号、没装 Chrome 时没有）运行 `cca [codex] billing`，用账号自己的浏览器实例和出口打开控制台的账单页。订阅附带的每月 API 额度和余额只在那里显示，官方没有查询接口，所以 App 不显示数字。
- **查看重置次数**：Claude 账号页“用量”一节底部“重置次数和到期日”那一行的「在 claude.ai 查看」（右键菜单里的「在 claude.ai 查看重置次数」；`can.limit_resets`）运行 `cca limit-resets`，用账号自己的浏览器实例和出口打开 claude.ai 的用量页。这两个按钮打开前 cca 把账号邮箱放进剪贴板，要登录时粘贴（这些登录页不认网址里的邮箱参数）。Claude 的重置次数和到期日 cca 查不到（CLAUDE.md 里记着原因），所以总览上没有小票；ChatGPT 账号的次数和到期日直接显示。
- **提问会话**（PROTOCOL 4.6）：添加账号、换 key、删除账号、编辑账号表、配置路由器、配置本机 Clash 这些流程在 App 里问完：`ProcessSession` 开一个 cca 进程，fd 3 是答案管道（`CCA_PROMPT=1`），`FlowModel` 把 `ask` 事件变成页面，`FlowSheet` 显示。cca 不支持（`hello.prompt`）时走下面的终端交接。
- **客户端更新**（`ClientUpdatesPage`，PROTOCOL 8）：桌面 App 和终端里的 Claude Code、Codex 一页管，每行一张卡片，图标中性灰、固定宽度（桌面 App 是窗口，
  `kind=cli` 是终端），能装的才有「安装更新…」、要重新检查的才有「重新检查」。版本、能不能更新、为什么都是 cca 给的。侧栏这一行有新版本时右边写「N 个可更新」。
- **API key**（PROTOCOL 4.10）：官方账号在“用量”一节、第三方 API 账号在自己的「API key」一节；「…」和右键菜单里有记录 / 更换、复制、删除记下的 API key。
- **终端交接**：在 `~/Library/Caches/local.cca.app/handoff/` 写 `.command`（bash，补 PATH，逐个加单引号运行 cca，结束等按任意键；系统自带的“终端”进程结束后默认留着窗口，按键后脚本用 AppleScript 关掉这个窗口；
  文件一开始就挪走再删掉，中途关窗口也不留下），App 发起的命令（登录、添加账号、配置路由器……）用系统自带的“终端”打开，账号的会话用用户选的终端打开（`TerminalApp.known`：系统自带的、iTerm2、Ghostty、Warp、WezTerm、kitty、Alacritty、Tabby、Rio、Hyper、cmux，
  在设置和账号页「终端」的下拉里选，没装的灰着不能选）。Termius、Wave 没有从外面运行命令的办法，是半自动：打开它、把命令放进剪贴板，用户粘贴运行。第三方终端 30 秒还没运行就收回脚本、问要不要改用系统自带的。放了一小时还没运行的会清掉。
- **账号的终端会话**：账号页的「终端」（右键菜单也有）在选的文件夹里运行 `cca [codex] @账号`，结束后留在用户的登录 shell 里。
  最近的文件夹记在 App 的设置里（最多 8 个，所有账号共用，第一次先选一个）。

## 这套工具链的坑（Command Line Tools，Swift 6.4，SDK 27）

- **`@State` 编不过**：SDK 27 里它是宏，宏插件 SwiftUIMacros 只随 Xcode 提供。界面状态一律放在 `@Observable` 的 AppModel 里，视图里用
  `@Environment(AppModel.self)`，要绑定时在 `body` 里 `@Bindable var model = model`。只属于一个视图、随它一起没的状态（卡片的悬停、API key 那一行的
  `KeyReveal`）可以直接用 `State` 类型：`private let x = State(initialValue: …)`，读写 `x.wrappedValue`。同样别用 `@Entry`、`@Previewable`、`#Preview`、`@Animatable`。
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
