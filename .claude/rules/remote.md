---
paths:
  - "cli/src/cmd/remote.rs"
  - "cli/src/cmd/remote/**"
  - "cli/src/cmd/hub.rs"
  - "cli/src/cmd/hub/deploy.rs"
  - "cli/src/server/**"
  - "server/**"
  - "ios/**"
  - "tests/remote_host.rs"
  - "tests/remote_relay.rs"
  - "tests/team_relay.rs"
  - "tests/web_relay.rs"
  - "tests/web_client.mjs"
  - "tests/web_page.mjs"
  - "tests/ios_kit.rs"
  - "tests/kit/relay.rs"
  - "tests/kit/spec.rs"
  - "tests/kit/spec/**"
  - "tests/kit/hubbox.rs"
  - "tests/tool/web_demo.rs"
  - "tests/vectors/spec_wire.json"
  - "macos/Vendor/SwiftTerm/**"
  - "macos/tools/swiftterm-parser.sh"
  - "macos/tools/swiftterm-check/**"
  - "macos/Sources/CcaApp/TerminalSurface.swift"
  - "macos/Sources/CcaApp/LogoGlyph.swift"
  - "macos/Sources/CcaKit/Model/Scheduler.swift"
  - "macos/Sources/CcaKit/Protocol/Remote.swift"
  - "macos/Tests/Fixtures/remote_list.json"
  - "macos/Tests/Fixtures/flow_remote_pair_--web.flow"
  - "docs/dev/remote.md"
  - "docs/dev/ios.md"
---

# 远程会话的成对修改

从根目录 CLAUDE.md「要一起改的」分出来的，只跟远程会话有关：经服务器中转、端到端加密、宿主和接入、网页端、iOS 的 App、SwiftTerm、和手机配对、每个 Team 各用各的中转服务器。读到上面 `paths` 里的文件时自动加载；只改 `cca` 里 bash 那一侧（`remote_*` 这些函数、`run_account` 的 `HOSTCMD`）时，自己先来读这一份。远程会话怎么实现、怎么测见 `docs/dev/remote.md`，iOS 的见 `docs/dev/ios.md`；远程启动（手机照模板新建会话）的成对修改在 `launch.md`。

- 远程会话的中转（线上的格式）：`server/README.md`「远程会话的中转」↔ 服务端（`cli/src/server/remote.rs`、`relay.rs`）↔ `cli/src/cmd/remote/crypto.rs`、`relay.rs` ↔ `tests/kit/spec.rs`（照文档独立实现的；`crypto.rs` 测试里的向量和它自己的对照数据 `tests/vectors/spec_wire.json` 都是固定的，没有重新生成它们的命令：改了格式两边的代码和数据一起换）；同意项 `remote_pair`：`cca` ↔ PROTOCOL 4.2 ↔ Swift 的 `ConsentItem`
- 远程会话：`run_account` 的 `HOSTCMD`（`remote_prepare`）↔ 辅助程序 `remote-host` 的参数；`local.rs` 新加的接入方 → 宿主的帧 ↔ `link.rs` 的 `bridge` 放不放行（只该这台机器上做的事不放行，像 `END`）↔ `tests/remote_relay.rs` 里发这一种帧的那一句；宿主 → 接入方新加的帧（`SIZE`）、HELLO 的标志 ↔ `bridge` ↔ CcaRemoteKit 的 `Frame` ↔ `kit/relay.rs` 的 `Reader`；同意项 `remote_end`：`cca` ↔ PROTOCOL 4.2 ↔ Swift 的 `ConsentItem`；客户端新加的终端查询序列 ↔ `cli/src/cmd/remote/vt.rs` 的 `csi_class` / `osc_class` / `str_class`（漏了的话几个终端同时接着时回答会变成按键）；`cca remote attach` 的退出码、断开的按键 ↔ docs/dev/remote.md ↔ PROTOCOL 9.3 ↔ App 的 `Texts.remoteEnded`；`remote-list --json`（`manage.rs` 的 `list_doc`）↔ PROTOCOL 9.1 ↔ Swift 的 `Protocol/Remote.swift` ↔ `macos/Tests/Fixtures/remote_list.json`
- SwiftTerm（`macos/Vendor/SwiftTerm/`，iOS 的视图在 `ios/Vendor/SwiftTerm-iOS/`：同一个版本，一起换）：换版本时源码、两份 `SHA256SUMS`、两份 README 里的版本和提交号、许可证（`LICENSE` ↔ `ios/App/Resources/SwiftTerm.txt`）一起换；两个 App 里都只有 `TerminalSurface.swift` 用它；不要的功能（图形协议）：`macos/tools/swiftterm-parser.sh` 换掉的那两行（`macos/build.sh` 和 `ios/cca.xcodeproj` 的「去掉图形协议」那一步都用它）↔ `macos/tools/swiftterm-check`（`ios/test.sh` 也在模拟器里跑它）↔ 两份 `TerminalSurface.swift` ↔ PROTOCOL 9.3
- 通知（`NOTIFY` 帧）：`local.rs` 的 `Note::parse`（`TITLE_MAX`、`BODY_MAX`）、`host.rs` 的 `NOTE_BURST`、`NOTE_GAP` ↔ CcaRemoteKit 的 `Note`、`NoteBudget`（`ios/Tests/KitTests` 用 `attach.rs` 测试里的同一条）；iOS 的 `Notifier` ↔ Mac App 的 `RemoteNotifier`（副标题、长度、间隔）；cca 自己带的钩子：`hook.rs` 的 `hooks`（事件、`MATCHER`、命令、超时）↔ `tests/tool/fx.rs` 的 `notifyless`（回归 3e 照它认出、去掉）；bash 的 `_remote-hook` ↔ 辅助程序的 `remote-hook`（开着的会话的 `--settings` 里写着这条命令：改名的话旧名字也要一直认）；通知的字（`ask_text`、`done_text`、`note_for`）照维护者的个人钩子写，宿主的 `NOTE_SAME` 靠两边一模一样才去得了重
- 「远程会话」页多久查一次（`Scheduler.remoteInterval`）↔ PROTOCOL 9.1 ↔ docs/dev/remote.md ↔ macos/README.md（服务器给每台设备限了流：`cli/src/server/limits.rs`）
- 远程会话的网页端（手机在浏览器里接入，线上的格式）：`server/README.md`「网页端」↔ 服务端（`cli/src/server/` 的 `GUEST_OPS`、`remote.rs` 的 `guest_key`、`ws.rs` 的 `pump`、`web.rs` 的 `Web`）↔ `server/web/core.js` ↔ `tests/web_relay.rs` 的 `Guest`、`WsSock`（照文档独立写的）；配对、握手、记录、帧是四份实现：`cli/src/cmd/remote/crypto.rs` ↔ `tests/kit/spec.rs` ↔ `server/web/core.js` ↔ `ios/CcaRemoteKit`（改了格式四处一起改，`tests/web_relay.rs` 的 `the_same_numbers`、`handshake_and_records`、`signature` 拿 `core.js`、`tests/ios_kit.rs` 拿 CcaRemoteKit 和 `kit::spec` 对）；断了重新接上的间隔：`core.js` 的 `RETRY_GAP`、`RETRY_FOR`、`MISSING_FOR`、`FLAPS` ↔ `attach.rs` 的同名常量 ↔ CcaRemoteKit `Attachment` 的 `retryGap`、`retryFor`、`missingFor`、`flapsMax`；辅助程序 `remote-pair-show --web` 的输出（`code` 后面紧跟一行 `link cca-remote://pair?…`）↔ bash 的 `remote_pair_run` ↔ CcaRemoteKit 的 `PairInvite`（链接的格式见 docs/dev/ios.md「二维码里的链接」）；服务器认不认手机（`pair_open` 的 `guest`、`--no-guests`、health 的 `relay.guests`）↔ `server/README.md`「手机（访客）」↔ CcaRemoteKit
- 和手机配对在界面里：同意项 `remote_pair` 的范围 `web`、题号 `remote.pair_web`、字段的种类 `qr`、hello 的 `remote_web`、`peers[].web`：`cca` ↔ PROTOCOL（2.1、4.2、4.6、9.1、9.2）↔ Swift（`PromptKind.qr`、`Handoff.remotePairWeb`、`HelloDoc.remoteWeb`、`RemotePeer.web`）↔ `macos/Tests/Fixtures`（`flow_remote_pair_--web.flow`、`remote_list.json`、`hello.json`）
- 网页端的页面（`server/web/`）：xterm.js 换版本时那四个文件和 `server/web/README.md` 里的版本、校验和一起换；目录里新加文件 ↔ `tests/web_relay.rs` 的 `NEED`（`the_real_page` 照它一个个去要）↔ 服务器的 `WEB_TYPES`；`macos/Sources/CcaApp/LogoGlyph.swift` ↔ `server/web/logo.svg`（照它手写的同一个字形）、`icon-*.png`（`macos/tools/make-icon --flat` 画的）；页面里的类名别叫 `terminal`（xterm.js 给自己的根元素用了）
- 每个 Team 各用各的中转服务器：`cli/src/cmd/remote/relay.rs` 的 `session_hub`（会话走哪台）↔ `cmd/hub.rs` 的 `route` / `core/policy.rs` 的 `policy_sources`（账号是谁给的）；配对表的 `hub`（`peers.rs`：Team 的编号；没有这一项的是 3.6.0 以前经维护者的服务器配的，读的时候丢掉）↔ `remote list --json` 的 `peers[].team`、`relay.hubs` ↔ PROTOCOL 9.1 ↔ Swift 的 `RemoteHub`、`RemotePeer.team` ↔ `macos/Tests/Fixtures/remote_list.json`；bash 的 `--team 名字`（`remote_hub_args`）↔ 辅助程序的 `--hub`（`pair.rs` 的 `hub_named`）↔ App 的 `RemoteHub.arguments`；Team 服务器的数据端口：`server/hub-install.sh` 的 `pick_relay`、`.env` 的 `CCA_HUB_RELAY`、`compose_yaml` 里带 `@RELAY@` 的那几处 ↔ `tests/kit/hubbox.rs` 假的 docker 解释它的那几行（`compose_up`）↔ `deploy.rs` 的 `report`（`relay`）↔ bash `team_deploy` 说要放行的端口
- 推送（docs/dev/push.md）：`PUSH` 帧（`F_PUSH` 0x0a）、`push_set`（含 `sids`）、网关的 `POST /v1/push`、通知的密文（`Kn`、标签 `cca-remote-note`、补齐、`PUSH_BLOB`）：`server/README.md`「推送」↔ 服务端（`server/push.rs`、`remote.rs` 的 `push_job`、`remote/ops.rs` 的 `push_set`、`gateway.rs`）↔ `cmd/remote/crypto.rs` 的 `note_seal`、`link.rs` 的 `push_note` ↔ `tests/kit/spec.rs` 的 `note_seal` ↔ CcaRemoteKit 的 `Push.swift`（`tests/ios_kit.rs` 的 `against_the_spec`）；通知的编号：`host.rs` 的 `Note::id` ↔ 服务端的 `note_id_re` ↔ CcaRemoteKit 的 `Note.idOK`；网关交给推送服务的（头、内容、JWT）↔ `tests/kit/apnsfake.rs`；通知扩展的字 `NoteCard` ↔ App 的 `Notifier` ↔ Mac App 的 `RemoteNotifier`；钥匙串分组：`ios/Entitlements/` ↔ 两份 Info.plist 的 `CcaKeychainGroup` ↔ `ios/test.sh` 的 `notify_embedded`、`testflight.sh`
