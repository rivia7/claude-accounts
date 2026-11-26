# iOS App（`ios/`）

手机上装不了 cca。iOS 上的 cca App 只做一件事：接入 Mac（或者服务器）上开着的远程会话。它是服务器上的**访客**（server/README.md「手机（访客）」），
没有 deploy key，是和它配对的那台设备领进来的。服务器、配对、握手、记录、帧都和 cca 之间的一样（remote.md），App 总是输入配对码的那一头（`b`）、接入方。
要替掉网页端（remote.md「网页端」，M5）：App 做好、发布以后，一个版本里把网页端、它的端口、证书助手、`compose.web.yaml` / `compose.acme.yaml` 一起去掉。

## 阶段

| 阶段 | 内容 | 状态 |
|---|---|---|
| 1 | 服务器认手机不再要网页端（`guest`、`--no-guests`）；二维码改成给 App 的链接（带配对码）；协议核心 `ios/CcaRemoteKit`；测试用的手机 `ios/tools/ccaguest`；`tests/ios_kit.rs` | 做完了，还没提交、没发布 |
| 2 | Xcode 工程、SwiftUI 界面（沿用 Mac App 的样子）、终端（SwiftTerm 的 iOS 视图）、扫码、钥匙串、演示模式 | 做完了，在模拟器里端到端测过（2026-11-25）；真机第一次试（第一个 TestFlight 构建）扫码必定「连不上服务器」：ATS，见下面「App」的 `Info.plist`；修好的第二个构建（20261125.183806）真机上配对、接入都成了（2026-11-26）。用户在真机上试出来的两处改了：接着的会话做完了手机不响（`Notifier`）、Claude Code 的屏幕拖不动（`SizedTerminalView`）；又加了推送（下面「推送」，模拟器里端到端测过，真的推送服务、通知扩展的进程还没验证）。这些都还没传构建 |
| 3 | TestFlight（`ios/testflight.sh`，见下面「TestFlight」）、App Store 审核（隐私清单、出口合规） | 上传脚本做好了；第一次上传 2026-11-25（1.0，构建号 20261125.110017，在一台登录着付费开发者账号的测试 Mac 上）；App Store 审核没开始 |
| 4 | 去掉网页端，服务器重新部署（每台都先问） | 等 App 上架 |

定下来的（别重新讨论）：
- 走 TCP 数据端口（`CCA-RELAY/1`），不走 WebSocket：托管平台（Railway、Sealos）本来就开不了中转
- 签名的请求发到主端口：服务器是自签证书，按二维码里的 `sha256//` 公钥指纹钉住（URLSession 的 delegate 自己解析证书的 DER 拿公钥），不认系统的证书机构；也不走系统代理
- 二维码里带配对码：扫了就配好，不用再输入
- 以后上 App Store；审核用的演示模式：本地模拟一台 Mac 和几个会话，终端里放预录的输出，不联网

## 二维码里的链接

`cca-remote://pair?v=1&u=<服务器地址>&p=<指纹>[&p=…]&c=<配对码>&n=<机器名>[&t=<Team 名>]`，值都是百分号编码（`-._~` 以外的都编）：
- `u`：`https://` 地址（不带用户名、路径只能是 `/` 或者空）；`p`：0～4 个 `sha256//base64` 指纹（服务器配置里有几个写几个；没有就是系统认的证书）
- `c`：配对码去掉横线的 16 个字符（同一个配对码只能用一次）；`n`：显示配对码的那台机器的名字（`clean` 过）；`t`：Team 的名字（服务器都是 Team 的，所以总有；本机没记名字时是 `Team 编号的前 8 位`）
- 不认的（App 说“不是 cca 的配对链接”）：`v` 不是 1、不是 `pair`、不是 https、配对码不对、重复的 `c`、指纹不对、超过 4 个
- 出处：辅助程序 `remote-pair-show --web` 的 `link` 那一行（`cli/src/cmd/remote/pair.rs` 的 `app_link`）；`ios/CcaRemoteKit` 的 `PairInvite` 解它

## 协议核心（`ios/CcaRemoteKit`）

没有界面的 Swift 包（iOS 17、macOS 14；只用 Foundation、CryptoKit、CommonCrypto 的 PBKDF2、Network.framework），第四份实现：
`cli/src/cmd/remote/crypto.rs` ↔ `tests/kit/spec.rs` ↔ `server/web/core.js` ↔ 这一份。
- `Bytes.swift`、`Crypto.swift`：配对码、配对消息、P、会话的说明（`dirOpen`）、`clean`
- `Channel.swift`：握手、记录、帧、开场白
- `Identity.swift`：Ed25519 的身份、SSHSIG 签名（命名空间 `cca-presence`）。每台服务器一把（键 `id <地址>`）
- `PairInvite.swift`、`Net.swift`：链接、钉住证书的请求（`Requester`：时钟偏了服务器回 `clock` 时按它的时间重发一次）、TCP 连接
- `Client.swift`：`RemoteClient`（配对、取消配对、列会话、接入）、`RemoteStore`（App 里是钥匙串；`ccaguest` 是目录里的文件）
- `Attachment.swift`：`Link`（一条连接：开场白、握手、20 秒一次心跳、60 秒没听到对面算断、输入按 8192 字节切）、`Attachment`（断了重新接：
  间隔 1、2、4、8、15 秒，共 300 秒；服务器说没有这个会话再等 45 秒；接上不到 10 秒又断连着 5 次就不再试——和 `attach.rs`、`core.js` 的同名常量一样）；
  `nudge()`：App 回到前台时调，断着的马上再试，开着却 25 秒没听到对面的当成断了；会话推来的通知（`NOTIFY` 帧）交给 `onNote`
- `Note.swift`：`NOTIFY` 帧里的 JSON（`Note.parse`：和 `local.rs` 的 `Note::parse` 一样去掉控制字符，标题、副标题 64 个字、正文 240 个字，
  没有标题也没有正文的不算）、限流（`NoteBudget`：一下子 5 条、之后每 3 秒一条，`host.rs` 的 `NOTE_BURST`、`NOTE_GAP`；几次重新接上算一处）
- 断开的原因是一个词：`detached`、`ended`、`lost`、`unreachable`、`refused`、`version`、`auth`、`nohost`、`busy`、`ticket`、`error`，
  `Attachment` 另有 `gone`、`unpaired`、`gave_up`

## App（`ios/cca.xcodeproj`）

两个目标：`cca`（App，iOS 26 起，iPhone 和 iPad）和 `SwiftTerm`（静态的 framework，见 `ios/Vendor/SwiftTerm-iOS/README.md`）；
协议核心 `ios/CcaRemoteKit` 是本地的 Swift 包。工程用 Xcode 16 起的“跟着文件夹走”的组：`ios/App/` 里新加的文件不用改工程。
`macos/Sources/CcaApp/LogoGlyph.swift` 直接引用（Logo 的形状和颜色只写在那一处），图标是 `macos/tools/make-icon --flat --opaque` 画的
（`ios/App/Assets.xcassets`：铺满、不带透明通道，圆角系统自己裁）。

- `CcaApp.swift`：入口。启动参数 `-demo`（演示模式）、`-screen terminal|pair|settings`（演示模式里直接开那一页，截图用）
- `Model/AppModel.swift`：配对过的电脑、它们上面的会话（列表看得见时 15 秒查一次，和 Mac App 一样）、配对的进度、开着的终端；
  `TerminalSession`：一个终端的连接状态、标题，字节进出
- `Model/Backend.swift`：`Backend`（`LiveBackend` 是 CcaRemoteKit 的 `RemoteClient`）、`KeychainStore`：密钥存钥匙串
  （`AfterFirstUnlockThisDeviceOnly`、不同步到 iCloud），配对表存 App 自己的目录；删了 App 再装时第一次打开清掉钥匙串里留下的
- `Model/Demo.swift`：演示模式（审核用、没有电脑时自己看看）：两台模拟的电脑、三个会话（一个只看、一个不在线），终端里放一段写好的输出，
  敲的字只在本机回显；不联网、不碰钥匙串
- `Views/`：欢迎页（没配对过）、会话列表（一台电脑一组，点一下接入，长按选只看）、终端页、配对（相机扫码用 VisionKit，扫不了时粘贴；
  经链接打开 App 时先问一句再配）、设置（外观、这台手机的名字、配对过的电脑和取消配对、演示模式、开源许可）
- `Terminal/TerminalSurface.swift`：唯一 `import SwiftTerm` 的文件，规矩和 Mac App 的同名文件一样：图形协议编的时候就去掉了，
  不认 OSC 1337、不报 sixel，链接只开 http / https，剪贴板只在 App 在前台、键盘焦点在终端上时写、不给读，不响铃；终端固定黑底，
  这一页的导航栏也是深色；只看的不弹键盘；会话里的程序自己写的通知（OSC 777 的 notify、OSC 9，数字开头的是进度条不算）交给 `Notifier`。
  上下拖（`SizedTerminalView`，照 Mac 上 SwiftTerm 的 `scrollWheel` 分）：会话开着鼠标上报时发滚轮（SGR 64 / 65，手指往下是往上滚，
  每移一格的高度发一格，按手指所在的格子报位置），备用屏幕没开鼠标上报时（less、man）发方向键，别的是 SwiftTerm 自己的回滚；松手时还在动的
  照惯性再滑一段（每毫秒剩 0.996），手指一按下就停；只看的照旧（发不了键）。Claude Code 2.1.332 一启动就进备用屏幕、开鼠标上报（`?1049h`、
  `?1000h`、`?1002h`、`?1003h`、`?1006h`），往回翻由它自己做，SwiftTerm 在这时装的手势把拖动当成按住鼠标拖着选字，手机上就滚不动了：
  `mouseModeChanged` 不调 super，不要那个手势（2026-11-26 真机上发现）。实测（假的 Messages API、伪终端里真的 Claude Code，50×30）：隔开的滚轮
  一格滚一行，连着来的（16 毫秒一格）加速到两行左右；滚轮在输入框、状态栏那几行上也滚对话记录
- `Model/Notifier.swift`：会话推来的通知（`TerminalSession` 收到 CcaRemoteKit 解好的 `NOTIFY`，或者 OSC 777 / 9）弹成本地通知，点它回到
  那个会话的终端（关了的重新接上：`AppModel.reopen`）。和 Mac App 的 `RemoteNotifier`（PROTOCOL 9.3）同一套规矩：标题、正文过 `Texts.plainLine`、
  限长度；来自哪台机器 App 自己写在副标题开头（「来自「机器名」 · 会话说的副标题或者会话的名字」）；一个终端 3 秒最多一条；头一次要弹时才问
  通知的权限。和 Mac 不一样：App 在前台、正开着这个终端时也弹（横幅、声音）——手机上只有开着终端时才连着会话，照「正看着就不弹」就从来不响。
  锁屏、切到别的 App 以后 App 被挂起、连接断了：那时经推送到（下面「推送」）；两条路都来了的同一条照宿主给的编号只弹先到的那一份（`firstTime`）。
  谁来推：cca 自己带的钩子（远程会话开着时启动 Claude Code 就带上：做完了、要你确认、在等你；remote.md「cca 自己带的钩子」），用户自己的钩子也可以调
  `cca remote notify`（remote.md「通知推到接着的别的机器」）
- `TestDriver.swift`：只在调试版里（`#if DEBUG`；`ios/test.sh` 查正式版里没有它）：端到端测试用启动参数让 App 自己配对、接入、敲键；
  `-test-note`：通知不真的弹（不要权限），说出来；`-test-wheel`：会话开了鼠标上报以后当成手指往下拖了三行；`-test-push-token 十六进制`：不向苹果要令牌，用这个登记、说出登记的结果和共用分组里的
  通知密钥，最后像通知扩展那样解开测试放进临时目录的 `pushed.json`
- `Design.swift`、`Texts.swift`：从 Mac App 搬过来的几样（`Brand`、`Surface`、`BrandMark`、`RemoteMark`、`Tag`、`Banner`、`Card`，玻璃只给按钮）和文字
- `PrivacyInfo.xcprivacy`：不追踪、不收集；用到的要说明原因的接口只有 UserDefaults（CA92.1）
- `Info.plist`（和 Xcode 生成的那一份合在一起，只放生成不了的）：链接的协议 `cca-remote`；`NSAppTransportSecurity` 的 `NSAllowsArbitraryLoads`。
  钉住自签证书要它：ATS 开着时 delegate 认了证书也不算，CFNetwork 照系统的规则再验一遍（不是证书机构签的、有效期太长、没有 SAN、
  没有 serverAuth），断开（`-1200`，底下是 `-9802`，系统日志里是 `ATS failed system trust`），App 只能说成「连不上服务器」：
  公网上的 Team 服务器一台都连不上（2026-11-26 真机上发现，第一个 TestFlight 构建扫码必定配不成）。本机地址 ATS 不管，所以连
  127.0.0.1 的 `in_the_simulator` 测不出来：`ios/test.sh` 查编好的 Info.plist 里有这一项（`--check` 也查，testflight.sh 拿它查归档）。
  只关 ATS，不关验证：只发 https（`ServerRef`），TLS 至少 1.2（`Requester`），有指纹时只认指纹对得上的证书，没有时照系统的证书机构验。
  上 App Store 时审核可能要说明为什么要这一项：连用户自己部署的服务器，证书是自签的，按配对时二维码里的指纹钉住

bundle id 是 `com.cca.remote`（2026-11-23 用户定的；SwiftTerm 那个目标是 `com.cca.remote.swiftterm`）。
TestFlight 先用用户自己的付费个人团队（2026-11-25），版本号先和 cca 分开走（见「TestFlight」）。
还没定的（要用户定）：出口合规怎么报（HTTPS 之外还有自己的端到端加密：HMAC-SHA256 做的流密码）；上 App Store 时用哪个团队。

## 推送

整个的做法（经谁送、谁知道什么、部署）在 docs/dev/push.md；App 这一头：

- `Model/Pusher.swift`：向系统要推送令牌（`registerForRemoteNotifications`，`AppDelegate` 收），在配了对的每台服务器上登记（`push_set`，CcaRemoteKit
  的 `Client.pushSet`）：令牌、环境（照 App 里的描述文件 `embedded.mobileprovision` 的 `aps-environment`：development 是 sandbox，别的是 production；
  模拟器是 sandbox）。每台服务器登记过什么记在 UserDefaults（`push <服务器>`）：一样的 7 天里不再登记；回 `no_push`（那台服务器没开推送）的一天以后再试；
  不让发通知了就把登记换成空的。启动、配好、取消配对、回到前台时对一遍；配了对、还没问过时问通知的权限
- 通知密钥：`Backend.syncNoteKeys` 照配对表把每个配对的通知密钥（CcaRemoteKit 的 `NoteKey`：`HMAC(P, "cca-remote/v1/note")`，不是 P）写进和通知扩展
  共用的钥匙串分组（`NoteKeychain`：service `cca.remote.note`，`AfterFirstUnlockThisDeviceOnly`，不同步；第一次装时连同别的一起清掉）。身份、配对的
  密钥照旧在 App 自己的分组里：权限里 `keychain-access-groups` 的第一个是它，以前存的不用搬。分组名写在两边 Info.plist 的 `CcaKeychainGroup`
  （`$(AppIdentifierPrefix)com.cca.remote.shared`；模拟器里没有团队时是 `FAKETEAMID.` 开头）
- `ios/Notify/`（通知扩展 `notify.appex`，bundle id `com.cca.remote.notify`，链 CcaRemoteKit）：`NotificationService` 拿共用分组里的通知密钥一把把试着解
  推送里的 `n`（`openPushed`），解开了换上 `NoteCard` 做的字、按会话叠在一起（`threadIdentifier` 是会话编号），userInfo 里带配对、会话、编号（点了回到
  那个会话）；解不开、时间到了还没做完照原样给（固定的那一句）
- 去重：`Notifier.firstTime` 照编号记 10 分钟：接着的会话里收到的 `NOTIFY` 和推来的那份只弹先到的；App 在前台时推来的经 delegate 的 `willPresent`，
  弹过了就不弹。本地通知的标识也用这个编号（推送服务照它合并）
- 权限（`ios/Entitlements/`）：App 有 `aps-environment`（工程里是 development，导出给 App Store 时换成 production）和两个钥匙串分组，扩展只有共用的那一个。
  `ios/test.sh` 查两个版本里都有通知扩展、版本号和 `CcaKeychainGroup` 和 App 的一样；`testflight.sh` 查导出的 App 的 `aps-environment` 是 production、
  两边的分组
- 设置页的「通知」：推送登记了没有、为什么没有（不让发通知、要不到令牌、哪台服务器没开推送），不让发通知时有去系统设置的按钮
- 测试、没验证的：docs/dev/push.md「测试」「没验证的」（模拟器里的 `simctl push` 不经通知扩展：扩展的进程只有真机上看得到）

## TestFlight

`bash ios/testflight.sh [--team 团队ID] [--upload]`：先跑 `test.sh --build` 的检查，再归档（Release、真机、自动签名、`-allowProvisioningUpdates`），
归档是另编的，用 `test.sh --check` 把查正式版的那三项再查一遍；导出成 .ipa 查签名（Apple Distribution 签的、App Store 的描述文件不带设备列表、
`get-task-allow` 是 false）；`--upload` 时查过以后再导出一次、直接传到 App Store Connect（`ExportOptions.plist` 的 `destination` 是 `upload`）。
东西都在 `ios/build/testflight/`（日志：`archive.log`、`export.log`、`upload.log`；查过的 .ipa 在 `export/`，解开的在 `ipa/`）。
- 要 Xcode 里登录着付费的开发者账号：免费的个人团队传不了 TestFlight。团队没写时用 Xcode 里的那个付费团队（只有一个时；照
  `defaults read com.apple.dt.Xcode IDEProvisioningTeamByIdentifier` 取，每个团队的键按字母排，`isFreeProvisioningTeam` 在 `teamID` 前面）。
  团队 ID 不写进工程：签名是谁的团队，由在哪台 Mac 上运行定
- 版本号是工程里的 `MARKETING_VERSION`（现在 1.0）；构建号是运行时的 UTC 时间 `年月日.时分秒`（时分秒去掉开头的 0，每一段都是整数），
  每次上传都比上次大（同一个版本号下 App Store Connect 不收重复的构建号）
- 签名都由 Xcode 管：归档用开发证书和通配的描述文件（`iOS Team Provisioning Profile: *`），导出时 Xcode 从云端拿分发证书、建
  `iOS Team Store Provisioning Profile: com.cca.remote`（第一次导出时顺带把套装 ID 注册到这个团队）
- 经 SSH 在别的 Mac 上运行：先 `security unlock-keychain` 解开登录钥匙串，不然 Xcode 读不了账号、签名报 `errSecInternalComponent`
- 上传前 App Store Connect 里要先有这个 App 的记录（只能在网页上建，API 建不了）：平台 iOS、套装 ID `com.cca.remote`、名称要在整个 App Store 里唯一。
  传上去处理完（十几分钟到半小时），每个构建要先回答出口合规才能给测试员装；内部测试（App Store Connect 的成员，最多 100 人）不用审核，
  外部测试的第一个构建要过 Beta 审核（审核员用演示模式）。构建 90 天后过期
- 实测（2026-11-25，测试 Mac 上的 Xcode 27.2）：检查、归档、导出、查签名一共两分半，.ipa 约 1.7 MB；`--upload` 再多十几秒，
  第一次上传（1.0，20261125.110017）上传时没有警告，日志最后是 `Upload succeeded`、`** EXPORT SUCCEEDED **`

## 测试

`bash ios/test.sh [--build]`：编（模拟器的调试版、真机的正式版）、查 SwiftTerm 编的是换过的那一份、正式版里没有 TestDriver、两个版本的 Info.plist 都关了 ATS（见「App」）；
不带 `--build` 时还开一台模拟器（`cca-test iPhone`，没有就建），在里面跑 SwiftTerm 的自检（`macos/tools/swiftterm-check` 链 App 用的那一份
SwiftTerm，`simctl spawn`），演示模式各页浅色 / 深色截图到 `ios/build/shots/`。要 Xcode 和模拟器的运行时（`xcodebuild -downloadPlatform iOS`）。
`bash ios/test.sh --check 派生数据目录 cca.app` 不编，只查编好的正式版那三项（SwiftTerm、TestDriver、ATS）：testflight.sh 拿它查归档。

`cd cli && cargo test --profile kit -p cca-tests --test suite -- ios_kit`（`tests/ios_kit.rs`，回归的 unit 一节里也跑；要先 `cargo build --release`；没有 swiftc 时跳过）：命令行工具的 swiftc 把 CcaRemoteKit 的源码加上一个 main 编成 macOS 上的程序（编出来的在 `ios/build/test/`，源码没变不重编）。
- `against_the_spec`：`tests/kit/spec.rs`（照文档独立写的那一份）出数据，`ios/Tests/KitTests` 逐项核对；Swift 封的配对消息、发的记录、签名再拿回 `kit::spec` 核对（签名用 `ssh-keygen -Y verify`）。证书指纹对 EC、RSA、Ed25519 三种证书和 openssl 算的比；通知的 JSON 怎么清理（和 `attach.rs` 的测试同一条）、限流
- `pair_list_attach_type`、`wrong_pin_and_used_code`、`session_ends`：真的服务器、真的辅助程序（`remote-host` 里的会话、`remote-pair-show --web`），手机那一头是 `ios/tools/ccaguest`：扫链接配对、列会话、接入、敲键、会话里的钩子推通知（`remote-notify`）、只看、会话结束、取消配对；指纹不对一个请求都不发成，配对码只能用一次
- `in_the_simulator`（`CCA_IOS_SIM=1`，先跑过 `bash ios/test.sh`：模拟器开着、App 编好了）：模拟器里真的 App（调试版），
  照 `cca remote pair --web` 给的链接配对、接入 box 上真的会话、敲键，box 上收到；会话里推一条通知，App 要弹的标题、副标题、正文对；
  会话开了鼠标上报（假客户端的 `mouse`）以后往下拖三行，box 的终端回显出三格往上的滚轮；截一张 `ios/build/shots/e2e-terminal.png`。
  拖动的手势本身（`WheelGate`、惯性）没有自动的测试：模拟器里注入不了触摸，要真机上手指试
  box 在 127.0.0.1 上，ATS 不管本机地址：ATS 拦不拦这里看不出来（见「App」的 `Info.plist`）
- 改了 CcaRemoteKit 故意弄错一处（比如帧的类型）跑一遍，看测试挂不挂
- `swift build`（`ios/CcaRemoteKit` 里）查它作为单独的模块编不编得过（`public` 的够不够用）
