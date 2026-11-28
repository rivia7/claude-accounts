# iOS App（`ios/`）

手机上装不了 cca。iOS 上的 cca App 只做一件事：接入 Mac（或者服务器）上开着的远程会话。它是服务器上的**访客**（server/README.md「手机（访客）」），
没有 deploy key，是和它配对的那台设备领进来的。服务器、配对、握手、记录、帧都和 cca 之间的一样（remote.md），App 总是输入配对码的那一头（`b`）、接入方。
要替掉网页端（remote.md「网页端」，M5）：App 做好、发布以后，一个版本里把网页端、它的端口、证书助手、`compose.web.yaml` / `compose.acme.yaml` 一起去掉。

## 阶段

| 阶段 | 内容 | 状态 |
|---|---|---|
| 1 | 服务器认手机不再要网页端（`guest`、`--no-guests`）；二维码改成给 App 的链接（带配对码）；协议核心 `ios/CcaRemoteKit`；测试用的手机 `ios/tools/ccaguest`；`tests/ios_kit.rs` | 做完了，随 3.7.0 发布（2026-11-25） |
| 2 | Xcode 工程、SwiftUI 界面（沿用 Mac App 的样子）、终端（SwiftTerm 的 iOS 视图）、扫码、钥匙串、演示模式 | 做完了，在模拟器里端到端测过（2026-11-25）；真机第一次试（第一个 TestFlight 构建）扫码必定「连不上服务器」：ATS，见下面「App」的 `Info.plist`；修好的第二个构建（20261125.183806）真机上配对、接入都成了（2026-11-26）。用户在真机上试出来的两处改了：接着的会话做完了手机不响（`Notifier`）、Claude Code 的屏幕拖不动（`SizedTerminalView`）；又加了推送（下面「推送」，模拟器里端到端测过）。这些在第三个构建里（20261126.54245，2026-11-26 传的）：真机上锁屏推送收到了、显示的是解开的字（2026-11-26，通知扩展、真的推送服务、Team 的服务器转都对）。之后用户提的两处改了：会话照最后打字的那一处排（手机接上不再让 Mac 上的变窄；下面「App」的 `TerminalBox`）、「已关闭的会话也推送」开关（下面「推送」）。这两处随 cca 3.7.4 发布、在第四个构建里（20261126.72224，2026-11-26 传的），两个 Team 的服务器也部署成了 3.7.4（认 `sids`）；真机上还没试。3.7.5：「账号」页（下面「App」，像 Mac App 的总览和账号页）、额度用完 / 恢复的通知（下面「推送」）：协议核心和真的服务器、真的 cca 端到端测过，演示模式里截图看过；随 cca 3.7.5 发布、在第五个构建里（20261126.101645，2026-11-26 传的），两个 Team 的服务器也部署成了 3.7.5（level 2）；真机上还没试。远程启动（会话列表里的「新建会话」，下面「新建会话（远程启动）」）：协议核心、App、演示模式做好了：协议核心和真的 cca 的启动器、真的服务器端到端测过（`tests/ios_kit.rs` 的 `a_phone_launches_a_session`），演示模式里截图看过、模拟器里起过；随 cca 3.7.6 发布、在第六个构建里（20261126.183151，2026-11-27 传的），真机上还没试。「账号」页照 Mac App 侧栏按工具分开（Claude / ChatGPT / Antigravity）、按套餐档位筛（2026-11-27，下面「App」的 `AccountsView`）：演示模式里截图看过，在第七个构建里（20261127.21638，2026-11-27 传的），真机上还没看。套餐改照 cca 给的原名筛（Pro / Max / Ultra……，不起中文的档位名）、两个 App 的文案改得简短统一（2026-11-27，d8b1c7b）：演示模式里截图看过，在第八个构建里（20261127.40727，2026-11-27 传的），真机上还没看。照 Mac App 的侧栏补上「总览」「用量统计」「折合金额」「共享与同步」（2026-11-27，用户要的：这几页都放到 iOS 上、设计语言和 Mac 一致；下面「App」）：总览一个账号一张带插画的卡片（和 Mac 的总览一样），用量统计、折合金额两页的图和 Mac 的同样的规矩，账号页加上折合金额，共享与同步说每台电脑在 Team 里怎样；明文里加了折合金额、Team、在用什么（remote.md「手机上的账号」，线上的格式、上限没变）：协议核心和真的 cca 端到端测过（`accounts_reach_the_phone`），演示模式里截图看过；随 cca 3.7.9 发布、在第九个构建里（20261127.81145，2026-11-27 传的），真机上还没看。远程启动时挑账号（模板不带账号：「新建会话」里选了模板再选账号）、远程会话的「排序」（拖着排先后，记在这台手机上）（2026-11-27）：协议核心和真的 cca 的启动器、真的服务器端到端测过，演示模式里截图看过；随 cca 3.7.10 发布。通知的提示音换成自己合成的两声（「做完了」「在等你」各一声，2026-11-27，下面「App」的 `ios/Sounds/`）：随 cca 3.7.11 发布。这两处在第十个构建里（20261127.132527，2026-11-27 传的），真机上还没试。远程启动的会话在手机上结束（终端页右上角的「结束会话」，2026-11-27，下面「App」）：协议核心和真的 cca 的启动器、真的服务器端到端测过，演示模式里截图看过、模拟器里端到端测过；随 cca 3.7.12 发布、在第十一个构建里（20261127.162027，2026-11-28 传的），真机上还没试。和 Mac App 对齐（下面「和 Mac App 的对照」：两份对不上的十处、总览和账号页的标记和说法、底下标签的角标、用量统计和折合金额的表头和说明、同时接着几个终端、列表上结束会话、账号页新建会话，2026-11-28）：演示模式里截图看过、模拟器里端到端测过（`tests/ios_kit.rs` 的 `in_the_simulator`）；随 cca 3.8.0-beta.1（测试版）发布、在第十二个构建里（20261128.24349，2026-11-28 传的），真机上还没看。新手教程、「更新了什么」、提示卡片、底下浮出的提示和读屏的播报、每一页写着演示模式、账号页两张图对齐（下面「新手引导」，2026-11-28）：演示模式里截图看过（iPhone 浅色 / 深色，两张图对齐在 iPad 上看的）。总览、账号页照 Mac 的口径说出口的问题（「有问题」、满头大汗的插画、右上角的小标记、「下一步」那句话、账号页的「网络出口」一节）、按 Team 筛、第三方 API 的模型、套餐过时，那台电脑报的问题、cca 的版本、「远程会话」的开关、这台手机能不能操作（明文里加的，remote.md「手机上的账号」；原来「和 Mac App 的对照」里「还没补的」第一项，2026-11-28）：协议核心和真的 cca 端到端测过（`accounts_reach_the_phone`），演示模式里截图看过。端口无法连接 / 节点选错了、电脑上的 cca 有新版本、Mac 页顶查用量和内置代理的那几条、会话和启动器的「无响应」（原来「还没补的」第一项剩下的，2026-11-28；都是明文里加的，「无响应」另要 Team 的服务器更新到 3.8.0-beta.2）：协议核心和真的 cca 端到端测过（`accounts_reach_the_phone`：新版本、后台刷新没查成、那台电脑的 App 关了就不算），演示模式里截图看过（`accounts-agy`、`account-node`、会话列表里的「无响应」）；随 cca 3.8.0-beta.2（测试版）发布。这三处在第十三个构建里（20261128.65254，2026-11-28 传的，不含 3.8.0-beta.3 手机上的 Antigravity 每组的每周用量）；加上它的是同一天传的第十四个构建（20261128.71735）。真机上都还没看 |
| 3 | TestFlight（`ios/testflight.sh`，见下面「TestFlight」）、App Store 审核（隐私清单、出口合规） | 上传脚本做好了；第一次上传 2026-11-25（1.0，构建号 20261125.110017，在一台登录着付费开发者账号的测试 Mac 上），到 2026-11-28 一共传了十二个构建；App Store 审核没开始 |
| 4 | 去掉网页端，服务器重新部署（每台都先问） | 等 App 上架 |

定下来的（别重新讨论）：
- 走 TCP 数据端口（`CCA-RELAY/1`），不走 WebSocket：托管平台（Railway、Sealos）本来就开不了中转
- 签名的请求发到主端口：服务器是自签证书，按二维码里的 `sha256//` 公钥指纹钉住（URLSession 的 delegate 自己解析证书的 DER 拿公钥），不认系统的证书机构；也不走系统代理
- 二维码里带配对码：扫了就配好，不用再输入
- 以后上 App Store；审核用的演示模式：本地模拟一台 Mac 和几个会话，终端里放预录的输出，不联网

## 和 Mac App 的对照

原则在 `ios/CLAUDE.md`「和 Mac App 对齐」：Mac App 有的，手机上能做的都要做，样子、口径、说法照 Mac 的。2026-11-28 照两边的代码核对（HEAD da4bdeb，cca 3.7.12，TestFlight 20261127.162027），同一天补了「两份对不上的」十处、〔界面〕的大半和同时接着几个终端，随后又补了教程、提示卡片、「更新了什么」、浮出的提示和读屏的播报、每一页写着演示模式、账号页两张图对齐，又在明文里加了出口检测的结果、那台电脑报的问题、Team 这些，再补上端口池的实况、Mac 页顶剩下的那几条、cca 有新版本、会话的「无响应」（原来「还没补的」第一项；下面写的是补完以后的）；三种工具之间的对照在 parity.md。补上一项、发现新的差别就改这一节。

记号：〔界面〕数据已经到了手机上，只差 iOS 的界面；〔数据〕电脑放上来的账号状态（remote.md「手机上的账号」）、会话的说明里没有，要先在明文里加；〔操作〕要一个经配对的电脑、在电脑那头执行的新操作（先问维护者）。

| | Mac App | iOS App |
|---|---|---|
| 数据 | 本机的 cca（PROTOCOL：`_gui state`、`usage`、`live`、`remote list`……），App 自己定时查 | 配对的电脑放上来的账号状态（变了才放，没变的隔半小时）和会话的说明；跑不了 cca，在前台、没在看终端时 15 秒取一次（Mac 只在「远程会话」页看得见时 15 秒查一次） |
| 页 | 总览、每个账号一页、用量统计、折合金额、节点分流、远程会话、共享与同步、客户端更新；菜单栏、设置、教程和提示卡片 | 总览（点进去是账号页）、用量统计、折合金额、远程会话、共享与同步；设置、配对、教程和提示卡片 |
| 操作 | 加、删、改账号，登录，打开 App 和网页版，终端，重置额度，API key，线路，Team，模板…… | 接入（操作 / 只看）、远程启动（会话列表、账号页）、结束远程启动的会话、扫码配对、取消配对 |

### 两边都有的页，iOS 差的
- 总览
  - 〔数据〕置顶（只记在 Mac App 的设置里）；别的设备上在用（Mac 的「在用（Mac-mini）」）。端口连不上、节点选错了（电脑那头另查 `_gui live`，remote.md「电脑那头另查的」：和 Mac 一样只在那台电脑的 App 开着时有）、出口不对、纯净度、泄露、从没检测过、没装桌面 App 都有了
  - 不算差的：Mac 按 Claude、ChatGPT、Antigravity、第三方 API 分组；手机一台电脑一组（一个配对只看得到走它那台服务器的账号）、按工具切换、按套餐筛。按 Team 筛两边是同一个菜单（全部账号、各个 Team、不在 Team 里的），手机照账号带的 Team 名字认（几台电脑上同名的算一个），账号分属两种以上时才放。「N 个要处理」在底下「总览」的角标上（全部工具，和 Mac 侧栏那一行一样不跟着筛选），页头那一句照列着的数；小条上分模型的额度和本周的关系、Antigravity 用得最多的是哪一组，Mac 是鼠标指着时说，手机是读屏时说
- 账号页
  - 〔数据〕「上次在终端里用的」、用量里的小项（`credits`、`extra_usage`、Opus / Sonnet 的每周额度）、线路（节点、端口）、连着哪些设备、API key 那一行；折合金额按设备看（只看这台 / 别的设备）、每一项的请求数和没有公开价格的、按模型的请求数和缓存命中率。套餐过时、出口检测（「网络出口」一节）、桌面 App 和终端里的命令没装（`can_why` 的 `app`、`cli`）有了；别的按钮手机上没有，用不着它们的 `can_why`
  - 「有问题」「建议先用」照 Mac `Diagnosis.all` 的先后和口径（端口的问题排在出口前面；端口、出口、纯净度、泄露有问题时不建议先用）
  - 〔操作〕登录、打开 App / 网页版、改别名、排序和置顶、删除、刷新用量、重置额度、在 claude.ai / 控制台查看、检测出口、同步路由器、折合金额的刷新、邮箱助手：现在都只在电脑上做
  - 定下来不给的：邮箱、用量出错时的说明（只给原因码：说明里有端口号）、出口检测的出口 IP、运营商和说明（只给结果、受限的地区、纯净度、DNS / WebRTC 的结论）
  - 不算差的：「下一步」那句话（`Advice`）里登录的问题写在电脑上运行哪条命令（`Texts.loginCommand`；Mac 是点「登录…」「重新登录…」，跑的是同一条），一切正常时不说「可以打开使用」；折合金额「从几月几日开始有记录」Mac 在「显示细节」里，手机总是写着
- 用量统计
  - 〔数据〕列哪些账号，Mac 另看 `features.usage`
  - 不算差的：手机一行分两层（名字和平均在上、小图和本周在下），表头也是两层；最下面写各台电脑多久以前上传的、少传了几个账号（`UploadedLine`；Mac 是本机现算的）
- 折合金额
  - 〔数据〕按设备看（所有设备 / 本机 / 其他设备）、合计里的缓存命中率和按模型、每一项的请求数和没公开价格的、「无法识别」单独一组（图、各台设备）、各台设备自己的图、已撤销的设备、币种、按天的日期（cca 给的 `date`）、同步没成的设备名、上次统计失败的原因、列哪些账号（`can.cost`）
  - 〔操作〕「刷新」（`cost sync --now`；手机的下拉只是重新取一次放上来的）
  - 不算差的：Mac 的「（这台）」手机上写「（这台电脑）」（放上来的那台电脑；只写「这台」读着像这台手机），地方窄时只截名字、不截它；名字后面橙色三角的原因 Mac 是鼠标指着时说，手机写在那一行下面
- 远程会话
  - 〔界面〕复制只有 SwiftTerm 长按的菜单（Mac 选中即复制、右键粘贴），只看的终端成不了第一响应者，多半复制不了（真机上没试）
  - 连不上时 Mac 分「无响应」「那台机器离线」「暂时无法连接」（`remote list --json` 的 `state`），手机也分「无响应」（服务器列会话时给的 `busy`：3.8.0 起的 Team 服务器才给，旧的照旧只说「离线」）和「离线」，连不上服务器的在那台电脑一组的顶上说；启动器（「新建会话」）一样。电脑上「远程会话」的开关（关着、又没有会话时照 Mac 说先打开它）、这台手机在那台电脑上是只看还是能操作（只看的组头写「只看」）有了
  - 不算差的：手机当不了宿主，也就没有开关、没有本机那一组；配对只扫码、粘贴链接（总是输入配对码的那一头）；没有设备连进手机，所以没有「已配对的机器」里的权限、「允许远程启动」；模板只在电脑上管。同时开着的终端最多 8 个（Mac 100 个：手机的内存、电量，服务器给每台设备限了流），Mac 页顶那一排在手机上是列表顶上「已打开的终端」那一组和终端页菜单里的「切换到」；关掉显示着的终端回到列表（Mac 显示旁边那个）
- 共享与同步：〔操作〕Team 的操作（粘贴邀请链接、同步配置、新建、部署、邀请设备、给设备分账号和节点、改名、设管理员、移除、信任的转发中转、更新和搬服务器、导出导入配置、退出、删除）都只在电脑上
- 设置和外壳
  - 〔界面〕iPad 只是版心宽、字大，没有分栏和键盘快捷键（Mac 有 ⌘R、⇧⌘[ / ⇧⌘]）
  - Mac 页顶的提示条手机上都有了：cca 有新版本（明文的 `update`：总览上那台电脑一组的最上面，设置里已配对的电脑那一行也写着）、账号表不对、文件读不了、共享设置有问题、后台刷新用量要同意或者失败了、内置代理停了（明文的 `problems`：总览上那台电脑一组的顶上，只列这个工具的和不分工具的，筛得一个账号都不剩时照样列着），记号、颜色、先后照 Mac 的 `TopBanners`；按钮（「现在刷新」「重试」「更新并重新打开」）只在电脑上，手机上说在电脑上点哪个。对不上的：Mac 上点了「以后再说」的新版本提示手机上照样有（记在 App 的设置里，cca 不知道）。App 和 cca 的版本对不上只有 Mac 有（手机的 App 不跟着 cca 发，新旧照「只能加字段」认）
  - 〔操作〕手机的下拉刷新只是重新取一次放上来的，Mac 的「刷新」真的去查用量
  - 额度用完 / 恢复的通知：Mac 是 App 自己查到了在本机弹（App 开着才有），手机经推送（电脑上的 cca 算好、Team 的服务器转）；开关的说法、标题、正文两边一样
  - 不算差的：底下标签的角标（总览「N 个要处理」、远程会话「已接入 N 个」、共享与同步「Team 要处理」，手机上是几台电脑）只有数字、只有一种颜色（Mac 侧栏那几行右边是橙的、绿的标签）；cca 没有折合金额、Team 时 Mac 不放那一页，手机五页总在（电脑没放上来时那一页照实说）；教程照手机写了五张（Mac 的十二张讲配置、加账号、出口、分流这些手机上没有的），「更新了什么」照编号认（手机的版本号一直是 1.0），提示卡片少分流页那一张，演示模式写在每一页的标题下面（Mac 用演示数据时写在侧栏最下面）

### Mac 有、手机上整个没有的
- 节点分流（分流页、同步路由器）〔数据〕〔操作〕：会让 OpenClash 重载的要先同意，不该从手机上点
- 客户端更新（桌面 App、终端里的命令）〔数据〕〔操作〕
- 菜单栏那样一眼看到的：手机上对应的是小组件、实时活动、快捷指令，一样都没有〔界面〕；小组件解不开账号状态（共用的钥匙串分组里只有通知密钥），要在 App Group 里放一份解开的快照
- 加账号、编辑账号表、删账号、登录、配置向导、配置路由器、内置代理、邮箱助手、API key、管理模板、给配对的设备开「允许远程启动」：见下面「还没补的」最后两条和「做不了、不做的」

### 只有手机有的
不算差距；写了「Mac 也该有」的反过来记着：
- 推送（锁屏、没开 App 也收到）、照编号去重、按会话叠在一起、点了接回那个会话（Mac 只能回到还开着的终端）、「做完了」「在等你」两声提示音（Mac App 弹的是默认的声音：Mac 也该有）
- 结束远程启动的会话：Mac App 结束不了它在别的机器上起的会话，`remote list --json` 的 `sessions[]` 没带宿主封好的 `end`（Mac 也该有）
- 捏合缩放、手指拖着滚动、键盘上那一排键（esc、ctrl、tab、方向键、F1～F10）；终端页上看得到正在接入、正在重连
- 几台电脑上的同一个账号在用量统计、折合金额里只列一次（`aid`）；新建会话里的账号带着头像、套餐、现在怎样；开场白发出去之前就查
- 演示模式（给审核看）、通知权限的状态和去系统设置的按钮

### 两份对不上的
2026-11-28 核对出的十处都改了：受托的管理员（`HostTeam.isAdmin` 认 `coadmin`，`tests/ios_kit.rs` 用 cca 真给的 member 加 coadmin）；折合金额按模型列「其他」（合计减去列出来的，多出列出的行数那么几分钱才列）；账号页按设备照 cca 放上来的列（一台、但很久没同步上来的也列；折合金额页照 Mac 两台以上才列）；`statsTicks` 的 `edge` 18；会话照 `since` 先起的在前；只有一个模板时才先选上；通知的副标题没有机器名时「来自「另一台机器」」、机器名不另截（和配对表里的名字一样长）、会话没有名字时用编号的前 8 位；账号的颜色（状态那句话灰字，颜色只在小记号上）、名字下面的标签（「Claude」「ChatGPT」、登录状态）、沙漏（「建议先用」）照 Mac；文案照 Mac；Mac 的配对那一行「手机在浏览器里接入」改成「用手机扫码」（3.7.0 起是 iPhone 上的 cca App），Mac 账号页那句话额度用完了几个窗口时照最晚恢复的那个说（`blocking`：和工位上的几点恢复一样，原来说的是先用完的那个）。

有意不一样的：按周的日期 Mac 一格不到 30 点就不写，手机窄、一格多半不到 30 点，不到时隔一个写（`CostChart.weekLabel`）。新发现的写在这里。

### 还没补的（能做，先后照用处排）
1. 〔数据〕折合金额补齐（按设备、缓存命中率、合计按模型、请求数）：放不下时和维护者商量放宽上限还是分开放
2. 〔操作〕要维护者定：马上查一次用量（电脑上查一遍、放上来）、折合金额马上统计、开关远程会话；再往后是重置额度、改别名、排序这些在电脑上执行、在电脑上同意的
3. iPad 的分栏（同时看几个终端）和键盘快捷键；小组件、实时活动

### 做不了、不做的
- 当宿主、在手机上运行 Claude Code / Codex / agy：iOS 上跑不了 cca
- 要电脑上的浏览器、终端、钥匙串的：登录（在账号自己的 Chrome 里、经账号的出口）、加账号、配置向导、配置路由器和内置代理、邮箱助手；API key 的明文不离开电脑
- 明文定下来不给的：邮箱、账号表的键、服务器地址、带端口号的说明（remote.md「手机上的账号」）
- 显示配对码、输入配对码：手机总是扫码的那一头（上面「定下来的」）
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
- `Client.swift`：`RemoteClient`（配对、取消配对、列会话、接入）、`RemoteStore`（App 里是钥匙串；`ccaguest` 是目录里的文件）。列会话时带着 `have`
  一起取回各台电脑放上来的账号的状态（`PeerListing.status`、`statusServed`；remote.md「手机上的账号」）
- `Status.swift`：账号的状态（`statusKeys`、`statusOpen`：解开、核对样子，编号不对、样子不对的账号丢掉）：`HostStatus`、`AccountStatus`（`exhausted`：
  额度用完的规则，和辅助程序 `status.rs` 的 `exhausted`、Mac App 的 `Diagnosis` 一样；`nearLimit`：快用完了）、`AccountUsage`、`UsageWeeks`；额度的通知
  （`UsageNote`）和字（`UsageText`：几点恢复、窗口和工具的叫法，和 Mac App 的 `Texts` 一样的说法）；`AccountStatus` 另有出口检测的结果（`net`：`AccountNet`，
  从没测过的 `checked` 是 false，旧版的宿主不给是 nil；`current` 是不过期、适用的那一份，Mac 的 `Diagnosis` 也只认这样的；`exitBad`、`hasLeak`）、为什么用不了
  （`whyApp`、`whyCLI`）、第三方 API 的模型（`model`）、是哪个 Team 给的（`team`）、`AccountUsage.outdatedLoginPlan`，`HostStatus` 另有那台电脑的 `ccaVersion`、
  `remoteOn`、`allow`、`problems`（`HostProblem`）
- `Cost.swift`：账号的折合金额（`AccountCost`：各项的起止照 `week_end`、`week_hours`、`day_end` 往前推，开始时还没有记录的（`since` 晚于开始）算 `partial`，
  最后一项是 `current`；合计、平均、缓存命中率照 cca 给的；按模型的 `CostModel`、按设备的 `CostDevice`）、这台电脑的折合金额那几样（`HostCost`）、
  这个配对所在的 Team（`HostTeam`）；`Status.swift` 里的 `AccountStatus` 另有在用什么（`appOpen`、`cliSessions`、`cliSince`）、`cost`，`HostStatus` 另有 `cost`、
  `costLeftOut`（`"cost": false`：账号太多没带）、`team`；旧版的宿主不给这几样，照没有
- `Attachment.swift`：`Link`（一条连接：开场白、握手、20 秒一次心跳、60 秒没听到对面算断、输入按 8192 字节切）、`Attachment`（断了重新接：
  间隔 1、2、4、8、15 秒，共 300 秒；服务器说没有这个会话再等 45 秒；接上不到 10 秒又断连着 5 次就不再试——和 `attach.rs`、`core.js` 的同名常量一样）；
  `nudge()`：App 回到前台时调，断着的马上再试，开着却 25 秒没听到对面的当成断了；会话推来的通知（`NOTIFY` 帧）交给 `onNote`；
  `fresh`：刚远程启动的会话（下面「新建会话（远程启动）」）；`end()`：请宿主结束会话（`END` 帧 0x07，没有内容；只给说明里 `end` 是 true 的会话用——
  `SessionInfo.canEnd`：远程启动的会话、能操作的配对，`jsonBool` 认的 true 才算，只看的不算；结束了和会话自己退出一样来 `onExit`、`closed("ended")`）
- `Launch.swift`：远程启动（下面「新建会话（远程启动）」）：启动器的说明（`launchOpen`）、`LAUNCH` 帧（`LaunchAsk`）、`LAUNCHED` 的回话（`LaunchResult`）
- `Note.swift`：`NOTIFY` 帧里的 JSON（`Note.parse`：和 `local.rs` 的 `Note::parse` 一样去掉控制字符，标题、副标题 64 个字、正文 240 个字，
  没有标题也没有正文的不算）、限流（`NoteBudget`：一下子 5 条、之后每 3 秒一条，`host.rs` 的 `NOTE_BURST`、`NOTE_GAP`；几次重新接上算一处）
- 断开的原因是一个词：`detached`、`ended`、`lost`、`unreachable`、`refused`、`version`、`auth`、`nohost`、`busy`、`ticket`、`error`，
  `Attachment` 另有 `gone`、`unpaired`、`gave_up`

## App（`ios/cca.xcodeproj`）

两个目标：`cca`（App，iOS 26 起，iPhone 和 iPad）和 `SwiftTerm`（静态的 framework，见 `ios/Vendor/SwiftTerm-iOS/README.md`）；
协议核心 `ios/CcaRemoteKit` 是本地的 Swift 包。工程用 Xcode 16 起的“跟着文件夹走”的组：`ios/App/` 里新加的文件不用改工程。
`macos/Sources/CcaApp/LogoGlyph.swift` 直接引用（Logo 的形状和颜色只写在那一处），图标是 `macos/tools/make-icon --flat --opaque` 画的
（`ios/App/Assets.xcassets`：铺满、不带透明通道，圆角系统自己裁）。

- `CcaApp.swift`：入口。启动参数 `-demo`（演示模式）、`-screen list|accounts|account|stats|cost|account-cost|sharing|terminal|pair|settings|launch|sort|tutorial|whatsnew|toast`（演示模式里直接开那一页，截图用：
  `list` 是远程会话，`sort` 是远程会话的「排序」，`accounts` 是总览，`account-cost` 是从折合金额页点进去的账号页；`account`、`account-cost` 另带 `-account N`：第几个账号，各台电脑的接着数；
  `accounts` 另带 `-tool 工具`、`-plan 套餐`、`-team outside|team:名字`：看哪个工具的、只看哪个套餐（cca 给的原名，比如 `Pro`）、按 Team 筛，没写的是 Claude、全部套餐、全部账号；`cost` 带 `-days` 是按天看；
  `tutorial` 是新手教程（`-step N`：第几张，从 0 数），`whatsnew` 是「更新说明」，`toast` 是远程会话页底下浮着的一句提示；都可以带 `-tips off`：提示卡片都关掉）。
  带 `-screen` 或 `-test-…`（TestDriver）时不自己弹新手教程、「更新了什么」
- `Model/AppModel.swift`：配对过的电脑、它们上面的会话和账号的状态（同一次查到的：App 在前台、没在看终端时 15 秒查一次）、
  配对的进度、开着的终端（`terminals`，照 Mac App 的规矩：一个会话一个终端，同一种接法再点是换过去，换一种接法、旧的已经断了时就地换掉，最多 8 个
  （`terminalLimit`），开满了说一句不再开；显示着的是 `terminal`）；底下五页（`AppTab`：总览、用量统计、折合金额、远程会话、共享与同步，和 Mac App 侧栏的先后一样，记着上次选的：总览的值还是原来「账号」页的 `accounts`；
  每一页右上角是配对和设置，`PageToolbar`；远程会话页前面多一个「排序」），每一页各有自己的导航（`accountPath`、`statsPath`、`costPath`）。终端推到点它的那一页的导航里（`terminalTab`：
  哪一页里点的返回哪一页），看着终端时底下的标签栏藏起来；返回不断开（`hideTerminal`）：终端照样接着，在「远程会话」页顶上「已打开的终端」、终端页的菜单里断开
  （`closeTerminal`：断开的是显示着的那个时回到列表）；底下三页的角标照 Mac App 侧栏那几行右边的标签（`accountsNeedingAttention`、`attachedCount`、
  `teamsNeedingAttention`）；没选过时打开总览；`openAccount`：点了额度的通知打开那个账号（换到总览、它的工具，筛着的套餐不是它的就不筛了）；`accountTool`、`accountPlan`：总览看哪个工具、
  只看哪个套餐，`costDays`：折合金额按周还是按天看（都记在 UserDefaults）；
  `TerminalSession`：一个终端的连接状态、标题，字节进出；会话的说明换了（重新列出来的、再点一次时的）照新的（`adopt`：刚远程启动的会话接上以后再取一次列表，
  拿到 `end`）；`launchFor`（`LaunchTarget`：哪台电脑，从账号页来的带着那个账号）、`startLaunch`、`launch`、`TerminalGrid`：新建会话（下面「新建会话（远程启动）」）；
  `end(_:on:)`：列表上结束远程启动的会话（开着它能操作的终端时用它；没开着时另开一个不显示的连接，接上就请宿主结束，20 秒没接上算没成）；
  `showToast`、`toast`：底下浮出的一句提示，同时让读屏播报（开满了终端、关了「新建会话」那一页以后没起成、结束会话、取消配对：和 Mac App 窗口底下浮出的一样，下面「新手引导」）；`guide`（`Guide`）：新手教程、「更新了什么」、提示卡片；
  `sessionOrder`（CcaRemoteKit 的 `SessionOrder`）、`sortingSessions`、`moveSessions`：远程会话的先后（拖过的照拖的，没拖过的——新起的——照电脑给的先后
  接在后面；拖完一台电脑的一组写回，记在 UserDefaults 的 `sessionOrder`，演示模式不记；和 Mac App 的 `RemoteOrder` 同一个排法，remote.md「一台设备 100 个会话」）
- `Model/Backend.swift`：`Backend`（`LiveBackend` 是 CcaRemoteKit 的 `RemoteClient`）、`KeychainStore`：密钥存钥匙串
  （`AfterFirstUnlockThisDeviceOnly`、不同步到 iCloud），配对表存 App 自己的目录；删了 App 再装时第一次打开清掉钥匙串里留下的
- `Model/Accounts.swift`：账号页的模型：`AccountRef`（配对编号 + 账号编号）、`AccountTrouble`、`AccountMood`（这个账号现在怎样：在用、额度已用完 · 几点恢复、
  未登录、账号不符、登录失效、出口有问题、建议先用，和右上角的小标记 `AccountFlag`：即将用完、没装桌面 App、纯净度低、有泄露、从没检测过；
  照 Mac App `Diagnosis.all` 的先后和说法，端口的问题手机上不知道）、`AccountCensus`（页头那一句：几个账号、几个在用、
  空闲、额度用完、未登录、有问题……）、`AccountShelf`（页头的工具切换、按 Team 筛（`TeamPick`：全部账号、各个 Team、不在 Team 里的，记在 UserDefaults 的 `accountTeam`）和按套餐筛：
  有账号的工具照 claude、codex、agy 的先后，记着的工具没有账号了照头一个看；筛的胶囊是这个工具的账号里有的套餐（cca 给的 `plan` 原名，不起中文的叫法），
  按档位（`plan_tier`）从低到高、同一档按名字排，只有一个套餐时不放；记着的套餐这个工具里没有就看全部（记着的不变，换回有它的工具又按它筛）；
  一台电脑上列哪些：这个工具、这个套餐的，官方账号在前、第三方 API 在后）、`PeerListing.sessions(of:)`（照 `aid` 找这个账号的会话）
- `Model/Pages.swift`：用量统计、折合金额两页列哪些账号：各台电脑放上来的（`hostedAccounts`；取消了配对、不让看的电脑没有），几台电脑上的同一个账号（`aid` 一样：
  同一个 Team 里的电脑算的）只列一次（`Pages.distinct`：留有数据的那一份，一样时留放得晚的，标上 `shared`），按工具分组（`groups`：claude、codex、agy 的先后）；
  用量统计只列官方账号（Antigravity 的每组额度一行：`Pages.statsLines`，和 Mac App 一样），折合金额只列官方账号、只列开着折合金额的电脑上的；列出来的账号在两台以上电脑上时一行写上是哪台的（`machines`）
- `Model/Demo.swift`：演示模式（审核用、没有电脑时自己看看）：两台模拟的电脑、三个会话（一个只看、一个不在线）、几个账号（`DemoAccounts`：四档套餐（Claude 的 Pro、Max、Ultra，ChatGPT 的 Go、Plus）、第三方 API 都有，ChatGPT 的两台电脑上都有；
  各有折合金额：按模型的、两台设备用过的、一台很久没同步上来的都有；一个账号开着桌面 App 和两个命令行会话）、两台电脑在同一个 Team 里（一台是管理员、一台在等同步）、
  新建会话的模板和能挑的账号（`DemoLaunch`：工作室的 Mac mini 上 Claude 的模板能挑三个账号，ChatGPT 的一个），终端里放一段写好的输出，敲的字只在本机回显；不联网、不碰钥匙串。
  Mac mini 上「工作」那个会话和「新建会话」起的算远程启动的（`canEnd`）：终端页有「结束会话」，结束了不再列出来；账号的出口检测各样都有（正常、受限的地区和 WebRTC 泄露、
  纯净度低、过期、从没测过、第三方 API 的接口正常），一个套餐过时、几个是 Team 给的，MacBook Air 报共享设置有问题、「远程会话」的开关关着
- `Views/`：欢迎页（没配对过）、远程会话（`SessionsView`：顶上「已打开的终端」（返回以后还接着的，点一下回到它，左滑、长按断开）；一台电脑一组，点一下接入（已经开着它的终端时
  换过去：「已接入」），长按选只看、换一种接法、断开、结束远程启动的会话（左滑「结束」也行，先问一句）；电脑不让这台手机接入（`off`）时照实说，只让看时组头写「只看」，电脑上「远程会话」的开关关着、又没有会话时照 Mac 说先打开它；右上角「排序」是列表的编辑状态：拖右边的把手排先后，只能挪、不能删，读屏的用「上移」「下移」）、终端页（`TerminalScreen`：标题下面写哪台电脑、哪个 Team、文件夹；右上角的菜单：切换到别的终端、改成只看 / 可以操作（重新接入）、断开（会话仍在电脑上运行）；远程启动的会话、能操作时右上角另有「结束会话」，
  先问一句「结束这个会话？」——电脑上的程序会被关掉，和关掉它的终端一样，没保存的会丢；结束了照会话自己退出那样说「会话已结束」，不说被 SIGHUP 结束的退出码；`TerminalSession.canEnd`、`end()`）、配对（相机扫码用 VisionKit，扫不了时粘贴；
  经链接打开 App 时先问一句再配）、新建会话（`LaunchSheet`）、设置（外观、这台手机的名称、已配对的电脑（那台电脑上 cca 的版本、这台手机能不能操作）和取消配对、通知、演示模式、帮助、开源许可）。
  每一页左上角是 Mac App 侧栏里这一页的图标，内容放在和 Mac 一样的实心圆角块里（`Surface.tile`）；总览、账号页、用量统计、折合金额顶上是提示卡片，演示模式时标题下面写着「演示模式」（下面「新手引导」）。
  总览（`AccountsView`，原来的「账号」页，照 Mac App 的总览）：上面是工具切换（Claude / ChatGPT / Antigravity 的分段，只有一个工具有账号时不放）和
  按套餐筛（「全部」和这个工具的账号里有的那几个套餐，一个套餐一个胶囊：档位的小徽记、套餐的原名（Pro、Max、Plus……）、几个账号；放不下先不写个数，再放不下左右划；
  这个工具只有一个套餐时不放；换工具时记着的套餐不变，换到的工具里没有它就看全部），
  下面一句点名（列着的这些），右边是按 Team 筛（和 Mac 同一个菜单）；再下面一台电脑一组（组的顶上是那台电脑报的问题，照 Mac 页顶的提示条：账号表校验不过、文件读不了是红的，
  共享设置有问题是橙的；只列这个工具的和不分工具的），只列这个工具（这个套餐、这个 Team）的账号，官方账号在前、第三方 API 在后，一个也没有、也没报问题的电脑不列；
  一个账号一张卡片（`DeskCard`，一行放得下几张放几张，最窄 150 点）：上面是和 Mac 总览一样的插画（`DeskArt.swift`：`DeskLook` 照账号现在怎样定人的样子和屏幕上的内容——
  在用时在打字，屏幕上是桌面 App、终端或者两样都有；额度用完时趴着、屏幕上写几点恢复；未登录时椅子空着；账号不符时是灰色的人；出口有问题时满头大汗、屏幕上没有信号；画法 `DeskPainter`、`OfficePalette`
  从 `macos/Sources/CcaApp/DeskArt.swift` 原样搬过来），左上角在用什么、右上角几点恢复、建议先用的标记，卡片右上角的小标记照 Mac 的 `flagBadge`（即将用完、纯净度低、有泄露是橙色的叹号，只有没装桌面 App、从没检测过时是灰色的 i，读屏时一样样念出来）；下面是名字、套餐（放不下套餐的徽章时只放档位的小徽记；第三方 API 写模型）、现在怎样
  （灰字，颜色只在前面的小记号上：在用绿点、账号不符和登录失效橙点、出口有问题红点、未登录灰点、额度用完灰色的月亮）、5 小时 / 本周 / 分模型的小条（`MiniUsage`：还没有数据写「—」，
  过了重置时间写「已重置」，百分比照用量上色；Antigravity 写各组里用得最多的那个）、Codex 的小票（还能重置几次、最早一次几时过期）；照 Mac 的帧率动，减弱动态效果、低电量模式时不动。那台电脑还在查、还没放、服务器太旧、不让这台手机看、账号太多没放下的各说一句，底下是多久以前放的。
  账号页（`AccountDetailView`）：名字下面的标签（工具、套餐或「第三方 API」和它的模型、登录有问题时的登录状态）、电脑那一行后面是哪个 Team 给的、在用时开着什么（桌面 App、命令行里几个会话）、
  「下一步」那句话（`Advice` / `AdviceLine`，照 Mac 的，取 `Diagnosis.all` 的头一条：登录的问题写在电脑上运行哪条命令；出口有问题时说在电脑上点「检测出口」或者运行哪条命令重新检测；哪个窗口的额度用完了几点恢复、快用完了用了多少几点重置；套餐过时、没装桌面 App、纯净度低、有泄露、从没检测过；建议先用（没有挡着它的问题时排最前）；一切正常）、额度（`UsageViews`：5 小时、本周、按模型、Codex 还能重置几次、
  Antigravity 的几组）、每周用量的图（13 周，点一根看那一周；右边留出折合金额那张图的刻度宽，同一周的柱子上下对齐；Antigravity 每组额度一张，第三方 API 没有）、折合金额（`CostSection`：按周 / 按天的柱子，点一根看那一项，开始时还没有记录的
  那几项淡一些；按模型、按设备（用过它的设备有两台以上、或者有很久没同步上来的时）、同步没成的说明；第三方 API 没有）、网络出口（`NetSection`，照 Mac 账号页「网络出口」里出口检测的那一部分：结果、纯净度、DNS / WebRTC，多久以前测的、是不是启动时测的；过期、不适用时灰着，从没测过时照实说；线路、出口 IP 不放上来）、这个账号的会话（点一下接入）和「新建会话」（那台电脑让这台手机远程启动、有用得了这个账号的模板时：先选好这个账号；没有会话时照实说为什么接不上：终端里的命令没装、电脑上「远程会话」的开关关着）；从折合金额页点进来时滚到折合金额。
  用量统计（`UsageStatsView`，照 Mac App 的「用量统计」）：`Pages.statsRows` 的账号按工具分组，一个账号一行（Antigravity 的每组额度一行：`Pages.statsLines`）：头像、名字、平均每周和几周用完 / 不到一半，下面 13 周的小柱子、这一周用了多少；上面两层的表头
  （账号、平均每周；共用的时间刻度 `StatsTimeScale`、本周）；最下面各台电脑多久以前上传的（`UploadedLine`）；点一行进账号页。折合金额（`CostStatsView`，照 Mac App 的「折合金额」）：上面是多久以前统计的、按周 / 按天的切换（`CostGranularityPicker`），
  合计（本周、平均每周、近 12 周合计，按天看时是今天、平均每天、近 30 天合计；有两个以上工具时分工具列，按天看时另有 30 天的合计柱子），无法识别属于哪个账号的、还在统计的各说一句；
  下面按工具分组，一个账号一行：头像、名字、柱子、这一周（今天）的金额，用过它的设备有两台以上时底下列着各台，名字后面有橙色三角时原因写在下面；
  两层的表头、最下面各台电脑多久以前上传的，和用量统计一样；点一行进账号页。共享与同步（`SharingView`，照 Mac App 的
  「共享与同步」，只看）：配对过的电脑照所在的 Team（名字和服务器地址一样的算一个）分组，每台电脑在里面是管理员还是成员、同步得怎样（没成时 cca 给的原因）、在里面的设备名、
  分给它几个账号、服务器要不要更新（和 Mac 一样不说配置第几版）；还在查、旧版 cca 没给 Team 的、不让这台手机看的各说一句；加入、新建 Team 要在电脑上做。
  两页共用的图在 `Charts.swift`：`SlotChart`（按时间排的柱子：一项一格，点一下选中、再点一下取消）、`CostChart`（纵轴的最高值取 1、1.5、2、2.5、3、4、5、6、8 乘 10 的几次方里
  够用的最小的那个，最高的那根不超过它的九成；横轴到现在和最后一项结束里晚的那个，13 周或者 30 天；日期隔几个写一个、表头的刻度：和 Mac App `CostView.swift` 的 `CostChart`
  同样的规矩）、`CostBarChart`、`CostModelList`、`CostDeviceList`、`CostDeviceName`（设备名和后面的「（这台电脑）」分开画：窄的时候只截名字）；`CostChart.gutterLabels`：账号页每周用量的图右边留多宽（折合金额那张图的刻度，经环境值 `weeklyGutter` 交给 `WeeklyChart`）
- `Terminal/TerminalSurface.swift`：唯一 `import SwiftTerm` 的文件，规矩和 Mac App 的同名文件一样：图形协议编的时候就去掉了，
  不认 OSC 1337、不报 sixel，链接只开 http / https，剪贴板只在 App 在前台、键盘焦点在终端上时写、不给读，不响铃；终端固定黑底，
  这一页的导航栏也是深色；只看的不弹键盘；会话里的程序自己写的通知（OSC 777 的 notify、OSC 9，数字开头的是进度条不算）交给 `Notifier`。
  上下拖（`SizedTerminalView`，照 Mac 上 SwiftTerm 的 `scrollWheel` 分）：会话开着鼠标上报时发滚轮（SGR 64 / 65，手指往下是往上滚，
  每移一格的高度发一格，按手指所在的格子报位置），备用屏幕没开鼠标上报时（less、man）发方向键，别的是 SwiftTerm 自己的回滚；松手时还在动的
  照惯性再滑一段（每毫秒剩 0.996），手指一按下就停；只看的照旧（发不了键）。Claude Code 2.1.332 一启动就进备用屏幕、开鼠标上报（`?1049h`、
  `?1000h`、`?1002h`、`?1003h`、`?1006h`），往回翻由它自己做，SwiftTerm 在这时装的手势把拖动当成按住鼠标拖着选字，手机上就滚不动了：
  `mouseModeChanged` 不调 super，不要那个手势（2026-11-26 真机上发现）。实测（假的 Messages API、伪终端里真的 Claude Code，50×30）：隔开的滚轮
  一格滚一行，连着来的（16 毫秒一格）加速到两行左右；滚轮在输入框、状态栏那几行上也滚对话记录。
  终端放在 `TerminalBox`（UIScrollView）里，照会话的大小排：HELLO 说自己能缩放（标志 4），宿主说会话多大（`SIZE` → CcaRemoteKit 的 `onSize` →
  `TerminalSession.hostSize`），终端定成正好那么多列、行（一格多大照 `getOptimalFrameSize` 算，多给半个点免得 SwiftTerm 取整差一；排完马上
  `layoutIfNeeded`，之后到的输出照新的大小画）。比这块地方宽时缩小到正好放得下宽度（放不下时看最下面那一截），双指捏着放大（最大到平常的字号）、
  双指拖着看；一个手指的照旧是终端的（上下拖滚动、长按选字、点一下弹键盘）。缩小了画时终端的 `contentScaleFactor` 跟着降（一层最多 800 万个像素）。
  报给会话的窗口大小是这块地方照平常的字号放得下几列几行（`natural`）：一接上、手机上打了字时会话就照它排（remote.md「几个终端接着同一个会话」），
  正好放得下、不缩放。旧的宿主不说 `SIZE`：照这块地方排，和以前一样。不用 SwiftTerm 的 `resize(cols:rows:)`：它会软重置终端
- `Model/Notifier.swift`：会话推来的通知（`TerminalSession` 收到 CcaRemoteKit 解好的 `NOTIFY`，或者 OSC 777 / 9）弹成本地通知，点它回到
  那个会话的终端（关了的重新接上：`AppModel.reopen`）。和 Mac App 的 `RemoteNotifier`（PROTOCOL 9.3）同一套规矩：标题、正文过 `Texts.plainLine`、
  限长度；来自哪台机器 App 自己写在副标题开头（「来自「机器名」 · 会话说的副标题或者会话的名字」）；一个终端 3 秒最多一条；头一次要弹时才问
  通知的权限。和 Mac 不一样：App 在前台、正看着这个终端时也弹（横幅、声音）——手机上只有开着的终端才连着会话，照「正看着就不弹」常常就不响了；
  返回以后、换到别的终端时开着的那几个照样连着、照样弹。
  锁屏、切到别的 App 以后 App 被挂起、连接断了：那时经推送到（下面「推送」）；两条路都来了的同一条照宿主给的编号只弹先到的那一份（`firstTime`）。
  谁来推：cca 自己带的钩子（远程会话开着时启动 Claude Code 就带上：做完了、要你确认、在等你；remote.md「cca 自己带的钩子」），用户自己的钩子也可以调
  `cca remote notify`（remote.md「通知推到接着的别的机器」）
- `ios/Sounds/`（`done.caf`、`ask.caf`；工程里是 App 和通知扩展共用的一个文件夹，两边的套装里各带一份）：通知的两声提示音。系统默认的那一声
  又短又轻，手机放在一边时常常听不见（2026-11-27 用户说的：没开静音、没开专注模式）。由 `ios/tools/make-sounds` 合成（加法合成的钟声：「做完了」
  是往上走的叮—咚，约 1.4 秒；「在等你」是高音的两下、隔一会儿再两下，约 1.2 秒；CAF，16 位整数、44.1 kHz、单声道），改了声音用它重新生成、
  换掉这两个文件（`ios/test.sh` 查两边一样，也查两个版本的 App、通知扩展里都带着）。响哪一声是 CcaRemoteKit 的 `NoteSound`（`NoteCard.sound`）：
  标题以「做完了」结尾的（cca 自己带的钩子、Codex、Antigravity 一轮做完了）、额度恢复了的响 `done`，别的（在等你、要你确认、在问你、额度用完了、
  会话里的程序自己写的）响 `ask`，和 Mac 上弹的（`popup.rs` 的 `sound`：Glass、Ping）同一个分法。App 自己弹的（`Notifier`）、通知扩展解开的都换上
  这一声；通知扩展解不开的照推送里写的 `default`（系统默认的那一声）。两边各带一份：App 弹的从 App 的套装里找；推来的经通知扩展改过以后系统从哪里找，
  文档只说「App 的主套装」，没说扩展改的算不算，放两份不用猜。真机上还没听过（要传 TestFlight）
- `TestDriver.swift`：只在调试版里（`#if DEBUG`；`ios/test.sh` 查正式版里没有它）：端到端测试用启动参数让 App 自己配对、接入、敲键；
  `-test-note`：通知不真的弹（不要权限），说出来；`-test-wheel`：会话开了鼠标上报以后当成手指往下拖了三行；`-test-sizes`：说出终端照多大排了
  （`shown`）、报给会话的窗口大小（`natural`）；`-test-launch 模板编号`（`-test-line 开场白`、`-test-account 账号的名字`：能挑的只有一个时可以不写，不止一个又没写时说 `launch fail no_pick`）：照「新建会话」那样启动，接着接入新会话；
  `-test-push-only`：「已关闭的会话也推送」关掉；`-test-push-token 十六进制`：不向苹果要令牌，用这个登记、说出登记的结果和共用分组里的
  通知密钥，最后像通知扩展那样解开测试放进临时目录的 `pushed.json`
- `Design.swift`、`Texts.swift`、`CostTexts.swift`：从 Mac App 搬过来的几样（`Brand`、`Surface`、`BrandMark`、各页的图标 `OverviewMark`、`StatsMark`、`CostMark`、`RemoteMark`、
  `SharingMark`、`Tag`、`Banner`、`Card`，玻璃只给按钮和浮出的提示）和文字（`CostTexts.swift`：金额、多久以前统计的、Team 那一句）；`Design.swift` 另有浮出的提示（`ToastView`、`toastOverlay`）、演示模式的副标题（`demoSubtitle`）
- `Views/Guide.swift`、`Model/WhatsNew.swift`：新手教程（`TutorialCard`、`TutorialSheet`）、「更新了什么」（`ReleaseNote`、`WhatsNew`、`WhatsNewSheet`）、提示卡片（`TipCard`）和它们记着的状态（`Guide`），见下面「新手引导」
- `PrivacyInfo.xcprivacy`：不追踪、不收集；用到的要说明原因的接口只有 UserDefaults（CA92.1）
- `Info.plist`（和 Xcode 生成的那一份合在一起，只放生成不了的）：链接的协议 `cca-remote`；`NSAppTransportSecurity` 的 `NSAllowsArbitraryLoads`。
  钉住自签证书要它：ATS 开着时 delegate 认了证书也不算，CFNetwork 照系统的规则再验一遍（不是证书机构签的、有效期太长、没有 SAN、
  没有 serverAuth），断开（`-1200`，底下是 `-9802`，系统日志里是 `ATS failed system trust`），App 只能说成「无法连接服务器」：
  公网上的 Team 服务器一台都连不上（2026-11-26 真机上发现，第一个 TestFlight 构建扫码必定配不成）。本机地址 ATS 不管，所以连
  127.0.0.1 的 `in_the_simulator` 测不出来：`ios/test.sh` 查编好的 Info.plist 里有这一项（`--check` 也查，testflight.sh 拿它查归档）。
  只关 ATS，不关验证：只发 https（`ServerRef`），TLS 至少 1.2（`Requester`），有指纹时只认指纹对得上的证书，没有时照系统的证书机构验。
  上 App Store 时审核可能要说明为什么要这一项：连用户自己部署的服务器，证书是自签的，按配对时二维码里的指纹钉住

bundle id 是 `com.cca.remote`（2026-11-23 用户定的；SwiftTerm 那个目标是 `com.cca.remote.swiftterm`）。
TestFlight 先用用户自己的付费个人团队（2026-11-25），版本号先和 cca 分开走（见「TestFlight」）。
还没定的（要用户定）：出口合规怎么报（HTTPS 之外还有自己的端到端加密：HMAC-SHA256 做的流密码）；上 App Store 时用哪个团队。

## 新手引导

照 Mac App 的那几样（`macos/Sources/CcaApp/Guide.swift`、CcaKit 的 `WhatsNew`）：新手教程、「更新了什么」、各页的提示卡片，另有底下浮出的提示（toast）和读屏的播报、每一页写着演示模式、账号页两张图对齐（2026-11-28，原来「和 Mac App 的对照」里「还没补的」第一项）。

- 新手教程（`Views/Guide.swift` 的 `TutorialCard`、`TutorialSheet`）：照手机写的五张（配对、总览、用量统计和折合金额、远程会话、通知和设置），上面的小图用 App 里真的部件画；
  左右划或者点按钮翻，「跳过」「开始使用」、往下滑掉都算看过（`finishTutorial`）
- 「更新了什么」（`Model/WhatsNew.swift`）：一次更新一条，新的写在最前面，`n` 比前一条大 1，`date` 写传到 TestFlight 的那一天（写好那天没传就改）。
  App 的版本号一直是 1.0（见「TestFlight」），所以照 `n` 认先后，不照版本号。打开时给哪几条（`WhatsNew.onLaunch`，和 Mac 的一个规矩）：总是把最新的记成看过；
  教程还没看过的不给（教程里都讲了）；一条都没看过的只给最新的那一条；看过的给比它新的几条
- 有它以前就配对过的（打开时已经有配对、教程和「更新了什么」都没记下）：算看过教程，给这一次的「更新了什么」，不从头讲
- 什么时候自己弹（`Guide.startIfNeeded`，RootView）：有配对、没在配对、没开设置、没开「新建会话」、没在看终端，再等 0.7 秒（让刚关的那一页收起来）；
  打开 App 以后（进出演示也算一次）只看一次要不要弹。截图、界面测试（`-screen`、`-test-…`）不自己弹。从设置里打开的弹在设置上面
- 提示卡片（`TipCard`）：总览、账号页、用量统计、折合金额顶上的「这一页怎么看」，点「知道了」不再出现；编号和 Mac 的一样（`office-page`、`account-page`、
  `usage-stats-page`、`cost-stats-page`；Mac 另有分流页的 `routing-map`）。用量统计、折合金额最下面原来那句说明挪进了卡片（和 Mac 一样）
- 记在 UserDefaults：`guide.tutorialDone`、`guide.dismissedTips`、`guide.whatsNewSeen`（和 Mac 同名）；演示模式只记在内存里，每次进演示都像刚装好（会弹教程），
  退出演示照真的那一份
- 设置里的「帮助」：新手教程、更新说明（全部的，新的在前）、重新显示提示卡片
- 浮出的提示（`AppModel.showToast`；`Design.swift` 的 `ToastView`、`toastOverlay`）：底下浮出一句，5 秒后自己收起，点一下也收起，新的一句顶掉旧的；
  同时让读屏播报出来（`AccessibilityNotification.Announcement`，优先级高）。原来弹框的几样都换成了它：开满了终端、关了「新建会话」那一页以后没起成、
  结束会话（成了、没成），另加取消配对、重新显示提示卡片
- 演示模式（`demoSubtitle`）：底下五页、账号页、设置、配对、新建会话的标题下面写着「演示模式」；终端页的标题是自己画的，写在第二行最前面
- 两张图对齐：账号页「每周用量」的图右边留出「折合金额」那张图的刻度宽（`CostChart.gutterLabels`，跟着折合金额按周 / 按天；没有金额时不留），
  经环境值 `weeklyGutter` 交给 `WeeklyChart`，同一周的柱子上下对齐，和 Mac 的 `WeeklyChart` 的 `gutter` 一样
- 测：`ios/test.sh` 截 `tutorial`、`tutorial-last`、`whatsnew`、`toast`、`account-notips`（浅色、深色）；iPhone 上两张图不在同一屏，对齐在 iPad（13 英寸）的模拟器里看过

## 新建会话（远程启动）

那台电脑让这台手机远程启动以后（Mac App「远程会话」的「已配对的机器」里的「允许远程启动」，终端里是 `cca remote allow 手机的名字 launch on`：只给能操作的配对；
2026-11-27 起配对过的电脑也能打开，见 launch.md「在别的机器上起」），它上面常驻的启动器在服务器上登记成一个“会话”，说明（机器名、模板：Mac App 的「远程启动」里写的，终端里是
`cca remote template add`；起会话时能挑的账号）用另一把密钥封给这台手机：旧的 App 拿会话说明的密钥解不开，当它不存在。格式见 server/README.md「启动器」。
- 协议核心（`Launch.swift`）：列会话时 `dirOpen` 解不开的再试 `launchOpen`，解得开的是启动器（`PeerListing.launcher`，一台电脑认头一个；
  `online`：启动器这会儿连着服务器）。模板只留编号（8 个小写十六进制）、摘要（16 个）、工具（claude、codex、agy）对得上的，编号重复的留头一个；
  名字、账号、文件夹、参数照会话的说明那样清理（40、64、200、300 个字），账号摘要不是 32 个小写十六进制的当没有。
  能挑的账号（`LauncherInfo.accounts`，`LaunchAccount`，3.7.10 起）同样：编号（16 个小写十六进制）、工具不对的、编号重复的不要，名字 64 个字；说明里没有这一项
  （那台电脑的 cca 是 3.7.9 及以前的）是 nil：模板用它自己的账号，不用挑。`accounts(for:)` 是这个模板的工具的那几个。
  `RemoteClient.launch`：要启动器的票据、握手（和接入会话一样），头一帧是 `LAUNCH`（0x09，不发 HELLO、不发心跳），等 `LAUNCHED`（0x18，别的帧不理），
  从要票据算起最多 20 秒，到了就断开。`LaunchAsk` 出的 LAUNCH 和辅助程序 `launch.rs` 的 `Ask::parse` 认的一样：键的先后固定、紧凑、非 ASCII 原样，挑了账号时带 `a`（它的编号，不是那个样子的不发）；
  20～500 列、5～300 行，开场白最多 500 个字、没有控制字符、不是空白，不合的不发（`bad_ask`）。结果 `LaunchResult`：起来了（新会话的编号）、
  启动器说没起（原因码、它写的原因）、没等到（服务器、连接的那几个词，另有 `timeout`、`cancelled`）
- `Attachment` 的 `fresh`：刚起的会话那头的转接进程可能还没在服务器上登记：头 45 秒里服务器说没有这个会话时每 1.5 秒再问一次（状态还是「正在接入」），
  接上过一次以后和别的会话一样。App 只给头一次接入用，点「重新接入」时不用
- App：会话列表里有启动器的电脑下面多一行「新建会话」（启动器不在线、没有模板时点不了）；账号页也有一个，先选好那个账号，只列用得了它的模板
  （`AppModel.launchTemplates`）。点了弹出 `LaunchSheet`：上面说一句文件夹、参数照模板（在电脑上改），一个模板一行（名字、工具、
  文件夹、参数；那台电脑的 cca 是 3.7.9 及以前的、模板自己定了账号时是账号的头像和「工具 · 账号」），说明里放不下的说「另有 N 个模板未显示」；只有一个模板时先选上（和 Mac 一样，PROTOCOL 9.5），不能带开场白的模板（参数里已经有一句话了）下面说一句。
  选了模板、要挑账号时下面一节「用哪个账号」：这个模板的工具能挑的，一个一行（头像、名字、套餐、现在怎样：照「账号」页里同一个账号——编号一样，
  对不上再照账号摘要——找；找不到的是灰圆、只有名字），只有一个时先选上，换了模板、刷新以后选着的不在能挑的里面了就不选；一个都没有时说一句
  （`Texts.launchNoAccount`），「启动」点不了。选着的模板能带开场白时下面有「开场白」（可以不写；换行换成空格），点「启动」等那台电脑回话。
  起来了：这一页关掉，打开新会话的终端（fresh；开满了 8 个终端时不发）。等着回话的时候关掉这一页不取消（「取消」换成「关闭」，和 Mac 一样）：起来了照样打开它的终端，
  没起成时底下浮出一句（`AppModel.showToast`）。没起成：说原因（`Texts.launchFailure`：认识的原因码 App 自己说，不认识的照那台电脑写的原因，
  再没有说一句通用的；`changed`、`no_template`、`denied`、`other_hub`、`no_account`、`need_account` 顺手刷新列表）；没等到回话的是 `Texts.launchUnreached`（发出去以后断了、超时的提醒会话可能已经起来了）
- 新会话多大（`TerminalGrid`）：这台手机上次在会话里打字时的窗口大小（新的宿主那时照它排会话；记在 UserDefaults）；还没打过字的照屏幕算，
  宽是去掉安全区放得下的列数，高只算一半：终端打开时键盘弹出来占掉差不多一半，高了的话新会话开头那几行在屏幕上面，看到的是下面空着的那一截。
  起来以后手机上一打字，会话就照这块屏幕排。模拟器（iPhone 11 Pro）里照屏幕算的是 50×23，键盘弹出来以后手机的窗口是 51×23
- 演示模式：工作室的 Mac mini 上三个模板（两个能带开场白；Claude 的能挑三个账号），MacBook Air 的启动器不在线；起的会话一秒左右就“起来”，终端里是刚起来的样子
  （有开场白的先放它），之后列在会话里
- 测试：`ios/Tests/KitTests` 读输入里的 `launch`（`kit::spec` 封的启动器的说明：解开、清理，能挑的账号也是，旧版的说明没有 `accounts`；别的密钥封的、会话说明的密钥、编号不对的不认，
  会话说明的密钥也解不开它；不合的开场白不发；LAUNCHED 的几种回话），出 `launch_frame`、`launch_frame_plain` 给 Rust 那头核对；
  `ccaguest` 的 `list` 带 `launcher`（模板、能挑的账号）、会话的 `end`，`launch 配对编号 模板编号 [--size 列x行] [--line 开场白] [--account 账号的编号]`、`attach --fresh`、`attach --end`（接上就请宿主结束）；App 的 `-test-launch`（`-test-account`）
- 测过的（2026-11-26，一次性的脚本，没进仓库）：KitTests 的 `launch` 对着照文档另写的一份封法（不用 CcaRemoteKit 的代码）出的数据；
  进启动器的那条连接对着假的中转和宿主（头一帧是 LAUNCH、不发 HELLO，别的帧、心跳、拆成两条记录的帧不碍事，样子不对的回话、断开、启动器没来、
  到时间了、调用方不等了）；模拟器里演示模式起会话（`-test-launch`）、截图（`-screen launch`）。和真的 cca 的启动器、真的服务器端到端：`tests/ios_kit.rs` 的
  `a_phone_launches_a_session`（ccaguest 列出模板和能挑的账号、挑一个照模板起会话、`attach --fresh` 接进去；刚起的会话服务器还不知道时的那几次重试不一定碰得上）。2026-11-27 起会话时挑账号同样这几样测过（KitTests、`a_phone_launches_a_session`、演示模式的截图）。没测的：真机

## 推送

整个的做法（经谁送、谁知道什么、部署）在 docs/dev/push.md；App 这一头：

- `Model/Pusher.swift`：向系统要推送令牌（`registerForRemoteNotifications`，`AppDelegate` 收），在配了对的每台服务器上登记（`push_set`，CcaRemoteKit
  的 `Client.pushSet`）：令牌、环境（照 App 里的描述文件 `embedded.mobileprovision` 的 `aps-environment`：development 是 sandbox，别的是 production；
  模拟器是 sandbox）。每台服务器登记过什么记在 UserDefaults（`push <服务器>`）：一样的 7 天里不再登记；回 `no_push`（那台服务器没开推送）的一天以后再试；
  不让发通知了就把登记换成空的。启动、配好、取消配对、回到前台时对一遍；配了对、还没问过时问通知的权限。
  设置里的「已关闭的会话也推送」（`Pusher.closedToo`，默认开着，存在 UserDefaults）：关着时登记里带 `sids`（App 里开着的那些会话；这台服务器上
  没开着的是空的：一个都不推），开、关终端时（`AppModel.terminal` 变了）跟着重新登记；记着的登记里带上它（`" only:会话编号"`）。服务器回的没有
  `"sids": true`（旧版，照旧都推）：记成 `unfiltered`，开着的会话变了也不再登记，一天以后再试；设置里说哪台服务器还不认。
  设置里的「额度用完、恢复时通知」（`Pusher.usageNotes`，默认开着，UserDefaults 的 `pushUsage`；docs/dev/push.md「额度的通知」）：登记时总说 `usage`
  （关着是 false），记着的登记后面多一段 `" usage"`；服务器回的没有 `"usage": true`（3.7.5 以前的）记成 `usageless`，一天以后再登记，设置里说哪台服务器推不了
- 通知密钥：`Backend.syncNoteKeys` 照配对表把每个配对的通知密钥（CcaRemoteKit 的 `NoteKey`：`HMAC(P, "cca-remote/v1/note")`，不是 P）写进和通知扩展
  共用的钥匙串分组（`NoteKeychain`：service `cca.remote.note`，`AfterFirstUnlockThisDeviceOnly`，不同步；第一次装时连同别的一起清掉）。身份、配对的
  密钥照旧在 App 自己的分组里：权限里 `keychain-access-groups` 的第一个是它，以前存的不用搬。分组名写在两边 Info.plist 的 `CcaKeychainGroup`
  （`$(AppIdentifierPrefix)com.cca.remote.shared`；模拟器里没有团队时是 `FAKETEAMID.` 开头）
- `ios/Notify/`（通知扩展 `notify.appex`，bundle id `com.cca.remote.notify`，链 CcaRemoteKit）：`NotificationService` 拿共用分组里的通知密钥一把把试着解
  推送里的 `n`（`openPushed`），解开了换上 `NoteCard` 做的字和提示音（上面「App」的 `ios/Sounds/`）、按会话叠在一起（`threadIdentifier` 是会话编号），userInfo 里带配对、会话、编号（点了回到
  那个会话）；额度的通知（`kind` 是 usage）字照这台手机的时区写（`UsageText`），同一个账号的叠在一起（`threadIdentifier` 是 `usage:<配对>:<账号编号>`），
  userInfo 里带配对、账号编号（点了打开那个账号：`Notifier` → `AppModel.openAccount`）；解不开、时间到了还没做完照原样给（固定的那一句）
- 去重：`Notifier.firstTime` 照编号记 10 分钟：接着的会话里收到的 `NOTIFY` 和推来的那份只弹先到的；App 在前台时推来的经 delegate 的 `willPresent`，
  弹过了就不弹。本地通知的标识也用这个编号（推送服务照它合并）
- 权限（`ios/Entitlements/`）：App 有 `aps-environment`（工程里是 development，导出给 App Store 时换成 production）和两个钥匙串分组，扩展只有共用的那一个。
  `ios/test.sh` 查两个版本里都有通知扩展、版本号和 `CcaKeychainGroup` 和 App 的一样；`testflight.sh` 查导出的 App 的 `aps-environment` 是 production、
  两边的分组
- 设置页的「通知」：推送登记了没有、为什么没有（不让发通知、要不到令牌、哪台服务器没开推送），不让发通知时有去系统设置的按钮；
  「已关闭的会话也推送」「额度用完、恢复时通知」两个开关（演示模式里没有），关着 / 开着时哪台服务器还不认它
- 测试、没验证的：docs/dev/push.md「测试」「没验证的」（模拟器里的 `simctl push` 不经通知扩展：扩展的进程只有真机上看得到）

## TestFlight

`bash ios/testflight.sh [--team 团队ID] [--upload]`：先跑 `test.sh --build` 的检查，再归档（Release、真机、自动签名、`-allowProvisioningUpdates`），
归档是另编的，用 `test.sh --check` 把查正式版的那五项（SwiftTerm、TestDriver、ATS、通知扩展、提示音）再查一遍；导出成 .ipa 查签名（Apple Distribution 签的、App Store 的描述文件不带设备列表、
`get-task-allow` 是 false）；`--upload` 时查过以后再导出一次、直接传到 App Store Connect（`ExportOptions.plist` 的 `destination` 是 `upload`）。
东西都在 `ios/build/testflight/`（日志：`archive.log`、`export.log`、`upload.log`；查过的 .ipa 在 `export/`，解开的在 `ipa/`）。
- 要 Xcode 里登录着付费的开发者账号：免费的个人团队传不了 TestFlight。团队没写时用 Xcode 里的那个付费团队（只有一个时；照
  `defaults read com.apple.dt.Xcode IDEProvisioningTeamByIdentifier` 取，每个团队的键按字母排，`isFreeProvisioningTeam` 在 `teamID` 前面）。
  团队 ID 不写进工程：签名是谁的团队，由在哪台 Mac 上运行定
- 版本号是工程里的 `MARKETING_VERSION`（现在 1.0）；构建号是运行时的 UTC 时间 `年月日.时分秒`（时分秒去掉开头的 0，每一段都是整数），
  每次上传都比上次大（同一个版本号下 App Store Connect 不收重复的构建号）
- 传之前：这一次有用户看得到的改动时，在 `ios/App/Model/WhatsNew.swift` 最前面加一条（`n` 比上一条大 1，`date` 写上传的那一天；见「新手引导」）
- 签名都由 Xcode 管：归档用开发证书和通配的描述文件（`iOS Team Provisioning Profile: *`），导出时 Xcode 从云端拿分发证书、建
  `iOS Team Store Provisioning Profile: com.cca.remote`（第一次导出时顺带把套装 ID 注册到这个团队）
- 经 SSH 在别的 Mac 上运行：先 `security unlock-keychain` 解开登录钥匙串，不然 Xcode 读不了账号、签名报 `errSecInternalComponent`
- 上传前 App Store Connect 里要先有这个 App 的记录（只能在网页上建，API 建不了）：平台 iOS、套装 ID `com.cca.remote`、名称要在整个 App Store 里唯一。
  传上去处理完（十几分钟到半小时），每个构建要先回答出口合规才能给测试员装；内部测试（App Store Connect 的成员，最多 100 人）不用审核，
  外部测试的第一个构建要过 Beta 审核（审核员用演示模式）。构建 90 天后过期
- 实测（2026-11-25，测试 Mac 上的 Xcode 27.2）：检查、归档、导出、查签名一共两分半，.ipa 约 1.7 MB；`--upload` 再多十几秒，
  第一次上传（1.0，20261125.110017）上传时没有警告，日志最后是 `Upload succeeded`、`** EXPORT SUCCEEDED **`

## 测试

`bash ios/test.sh [--build]`：编（模拟器的调试版、真机的正式版）、查 SwiftTerm 编的是换过的那一份、正式版里没有 TestDriver、两个版本的 Info.plist 都关了 ATS（见「App」）、两个版本里都有通知扩展、提示音是 `make-sounds` 合成的那两声（两个版本的 App、通知扩展里都带着）；
不带 `--build` 时还开一台模拟器（`cca-test iPhone`，没有就建），在里面跑 SwiftTerm 的自检（`macos/tools/swiftterm-check` 链 App 用的那一份
SwiftTerm，`simctl spawn`），演示模式各页浅色 / 深色截图到 `ios/build/shots/`（`-screen` 的每一种，远程会话的「排序」是 `sort`；总览另有换到 ChatGPT 的 `accounts-codex`、只看 Pro 套餐的 `accounts-pro`，折合金额另有按天看的 `cost-days`，新手教程另有最后一张 `tutorial-last`，账号页另有关掉提示卡片的 `account-notips`、出口有问题的那个账号的 `account-exit`，总览另有关掉提示卡片的 `accounts-notips`（卡片上的标记、那台电脑报的问题）、换到 Antigravity 的 `accounts-agy`（那台电脑有新版本、路由器选错了节点的账号），那个账号的那一页 `account-node`；
装好后头一次启动慢，先空跑一次再截）。要 Xcode 和模拟器的运行时（`xcodebuild -downloadPlatform iOS`）。
`bash ios/test.sh --check 派生数据目录 cca.app` 不编，只查编好的正式版那五项（SwiftTerm、TestDriver、ATS、通知扩展、提示音）：testflight.sh 拿它查归档。

`cd cli && cargo test --profile kit -p cca-tests --test suite -- ios_kit`（`tests/ios_kit.rs`，回归的 unit 一节里也跑；要先 `cargo build --release`；没有 swiftc 时跳过）：命令行工具的 swiftc 把 CcaRemoteKit 的源码加上一个 main 编成 macOS 上的程序（编出来的在 `ios/build/test/`，源码没变不重编）。
- `against_the_spec`：`tests/kit/spec.rs`（照文档独立写的那一份）出数据，`ios/Tests/KitTests` 逐项核对（会话的说明的 `end`：只看的、不是 true 的不算）；Swift 封的配对消息、发的记录、签名再拿回 `kit::spec` 核对（签名用 `ssh-keygen -Y verify`）。证书指纹对 EC、RSA、Ed25519 三种证书和 openssl 算的比；通知的 JSON 怎么清理（和 `attach.rs` 的测试同一条）、限流、响哪一声（`NoteSound`）；
  账号的状态（`kit::spec` 封的：解开、读成模型、哪些账号算用完了、快用完了，配对编号不对的、样子不对的不认；折合金额的各项推出来的起止、`partial`、合计、按模型、
  按设备（样子不对的那一台丢掉）、Team、在用什么，出口检测的结果（从没测过的 `{}`、旧版没有这一项的、过期的，纯净度超过 100 的截到 100）、为什么用不了、模型、
  是哪个 Team 给的、套餐过时，那台电脑的版本、开关、能不能操作、报的问题，不带折合金额、每周用量的那一份，少放了账号的那一份照样带着那台电脑的几样）、额度的通知（字、恢复的那一条、不认的样子）；另有会话的先后（`SessionOrder`：排法、拖完写回、最多记 400 个）
- `pair_list_attach_type`、`wrong_pin_and_used_code`、`session_ends`：真的服务器、真的辅助程序（`remote-host` 里的会话、`remote-pair-show --web`），手机那一头是 `ios/tools/ccaguest`：扫链接配对、列会话、接入、敲键、会话里的钩子推通知（`remote-notify`）、只看、会话结束、取消配对；指纹不对一个请求都不发成，配对码只能用一次。
  终端里起的会话说明里 `end` 是 false，硬发 `END`（`attach --end`）宿主那头不转、会话照常；`a_phone_launches_a_session` 的最后再起一个、`attach --end` 结束它（`exit 129`、`closed ended`，再接是 `gone`）
  接入时说自己能缩放（ccaguest 的 `--size 40x20`）：一接上会话就照它排（`size 40 20`），box 上打了字照 box 排（`size 80 24`），手机再打字又照它排
- `accounts_reach_the_phone`：总览和额度的通知那一条路（remote.md「手机上的账号」）：box 上 alice 的 5 小时额度用完了，ccaguest 列会话时带回
  box 上的账号（名字、用量、`exhausted`；没在用时没有 `app`、`cli`、`since`；从没检测过出口：`checked`、`current` 都是 false；是哪个 Team 给的：Team 的名字）、
  box 在这个 Team 里的状态（角色、分给它几个账号）和 box 自己的几样（这台手机能操作、「远程会话」的开关关着、box 上没有节点时 Claude 的账号表校验不过的那一条：只有原因码，不带说明）；登记推送时带 `--usage`，用完的那一条 box 马上推、恢复的那一条服务器到点推，像通知扩展那样解开、
  字照手机的时区写（测试里是 UTC），用完了响 `ask`、恢复了响 `done`；登记不带 `--usage` 就不推，再带上又推
- `in_the_simulator`（`CCA_IOS_SIM=1`，先跑过 `bash ios/test.sh`：模拟器开着、App 编好了）：模拟器里真的 App（调试版），
  照 `cca remote pair --web` 给的链接配对、接入 box 上真的会话、敲键，box 上收到；会话里推一条通知，App 要弹的标题、副标题、正文对；
  会话开了鼠标上报（假客户端的 `mouse`）以后往下拖三行，box 的终端回显出三格往上的滚轮；截一张 `ios/build/shots/e2e-terminal.png`。
  大小（`-test-sizes`）：一接上会话就照手机排、App 跟着排，box 上打了字又变回 80×24（App 缩小了画），拖着滚动不改大小。推送带 `-test-push-only`：登记回的是 `ok only`，开了会话以后服务器上的登记只推这个会话，通知照样推到。
  拖动的手势本身（`WheelGate`、惯性）没有自动的测试：模拟器里注入不了触摸，要真机上手指试
  box 在 127.0.0.1 上，ATS 不管本机地址：ATS 拦不拦这里看不出来（见「App」的 `Info.plist`）
- 改了 CcaRemoteKit 故意弄错一处（比如帧的类型）跑一遍，看测试挂不挂
- `swift build`（`ios/CcaRemoteKit` 里）查它作为单独的模块编不编得过（`public` 的够不够用）
