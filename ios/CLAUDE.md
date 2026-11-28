# iOS 上的 cca App（ios/）

先读 `docs/dev/ios.md`：阶段表和定下来的、和 Mac App 的对照、二维码里的链接（`cca-remote://pair?…`）、协议核心 `ios/CcaRemoteKit`、`ios/test.sh` 和 `tests/ios_kit.rs`、TestFlight。命令在根目录 CLAUDE.md 的「命令」。

配对、握手、记录、帧的格式是四份实现（辅助程序、`tests/kit/spec.rs`、网页端的 `core.js`、这里的 CcaRemoteKit），SwiftTerm 和 `macos/Vendor/SwiftTerm/` 是同一个版本：改的时候一起改，清单在 `.claude/rules/remote.md`（读这里的文件时自动加载）。

传 TestFlight 之前：有用户看得到的改动就在 `ios/App/Model/WhatsNew.swift` 最前面加一条「更新了什么」（`n` 比上一条大 1，`date` 写上传那天；docs/dev/ios.md「新手引导」）。

## 和 Mac App 对齐
- iOS App 是 Mac App 在手机上的那一份：Mac App 有的页面、信息、提示、通知，手机上能做的都要做，样子、口径、说法照 Mac 的，Mac 改了 iOS 跟着改。差在哪、还没补的（排好了先后）、做不了的见 docs/dev/ios.md「和 Mac App 的对照」：补上一项、发现新的差别就改它
- 新功能两个 App 一起做：Mac App 加了页面、字段、提示、通知，同一次把 iOS 也做了；这次不做的写进对照的「还没补的」，不能默默只做 Mac 的。手机上先有、Mac 也该有的（像结束在别的机器上远程启动的会话）一样记进去
- 先后：先修「两份对不上的」，再补〔界面〕（数据已经在手机上），再补〔数据〕（要电脑多放），最后是〔操作〕（要在手机上做的）
- 手机上没有 cca，数据只有两条路：配对的电脑封好放上来的账号状态（`status.rs` 的明文，remote.md「手机上的账号」）和会话的说明、帧。缺的在明文里加字段：只能加，旧手机不认的照没有；排进 `STATUS_BLOB` 的上限和少放的先后；不放邮箱、账号表的键、API key、服务器地址、带端口号的说明；成对修改见 `.claude/rules/phone-accounts.md`
- 手机上的操作现在只有接入（操作 / 只看）、远程启动、结束远程启动的会话。新的操作都在电脑那头执行：新的帧或请求只给能操作的配对（`bridge` 放行，只看的不放），电脑上照 cca 的同意（`consent`）问，不在手机上绕开 cca 的规矩。「只看」是 3.7.5 做「账号」页时划的范围，没有定死：加新的手机操作之前先问维护者
- 口径只有一份：额度用完、在用、问题的先后照 cca 和 Mac 的 `Diagnosis`，电脑能算好放上来的（像 `exhausted`）不在手机上重算；拷贝的两份（`DeskArt`、`CostChart`、`Texts` 里的说法）对不上时以 Mac 为准，Mac 那份错了两边一起改
- 只有手机才有的（推送、两声提示音、捏合缩放、键盘上那一排键、演示模式）不算差距。改了界面对着 Mac 的同一页看：`ios/test.sh` 的演示模式截图 ↔ `macos/build.sh --debug` 的截图模式
