# 远程启动

手机（iOS 上的 cca App，配对过的「访客」）照这台机器上的模板新建一个会话：这台机器前面不用有人。起来的会话和在终端里启动的一样
（同一个账号的环境、出口、隔离），手机马上接进去；这台机器上照样 `cca remote attach` 得到、`cca remote end` 得了。2026-11-26 维护者定的（「支持远程启动 cca，
并支持添加若干启动参数的模板」）。还没发布；协议核心和真的 cca、真的服务器端到端测过（下面「测试」），真机、真的 LaunchAgent 还没试。

定下来的（别重新讨论）：
- 只能照模板起：手机只挑模板、给窗口大小和一句开场白，账号、文件夹、参数都在这台机器上定好（`cca remote template`）。手机给不了任意的命令、参数、文件夹
- 只给手机：配对表里是手机（`web`）、能看和操作（`incoming` 是 `control`）、单独打开过远程启动（`launch`，打开要同意：同意项 `remote_launch`，
  范围是配对编号，默认不同意）。改成只让看、不让接入时 `launch` 跟着清掉，改回来不跟着回来
- 开场白放进参数：模板能带开场白（`line`：claude、codex，参数里没有 `--`）时，手机给的那句作为客户端的最后一个参数（`-- 开场白`），不是起来以后往终端里敲
- 服务器不改：启动器在每台走得到的 Team 服务器上登记成一个“会话”（`relay_host`，占这台设备 8 个名额里的一个）；手机和它握手、说要起哪个，跟接入会话一样
- 模板的摘要（`h`）：手机列出来以后这台机器上改了模板，手机拿旧的摘要来起是 `changed`，不照改过的起
- 新会话的大小是手机说的（列 20～500、行 5～300）：App 记着在会话里打字那一刻的窗口（新的宿主就是那时照手机排会话）；从没打过字照屏幕算、高只算一半（键盘会弹出来）
- 刚起的会话服务器可能还不知道：手机接入时服务器说 `no_session` 不算会话结束，45 秒内每 1.5 秒再问（CcaRemoteKit `Attachment` 的 `fresh`）
- 起的会话的环境只带白名单（`KEEP_ENV`，下面「起的会话」）；模板、日志里没有凭证，不进 `CCA_DIR_PRIVATE`
- macOS 上后台进程是用户的 LaunchAgent，Linux 上是用户 crontab 里的两行（`@reboot` 和每 10 分钟一次 `serve --if-down`）：都不用 root

## 模板（`cca remote template`）

- 存在 `~/.cca/remote/templates.json`（600，改之前拿 `Lock`）：`{"v":1,"templates":[{"id","name","tool","account","dir","args","at"}]}`。
  `id` 是 8 个小写十六进制字符；`account` 存邮箱（照 `cca @账号` 那样认：别名、邮箱、编号、唯一的前缀，改别名不影响）；`dir` 是绝对路径，要已经在
- 辅助程序 `cli/src/cmd/remote/template.rs`（`remote-template list [--json] | add 名字 工具 账号 文件夹 [--replace 编号] -- 参数… | rm 名字或编号`）；
  bash 的 `remote_template`：`cca remote template [list [--json]]`、`add [--name][--tool][--account][--dir][--replace 编号] [-- 参数…]`（没写的在终端里 `ask`，
  界面 App 都给全）、`rm 名字或编号`。`add` 的 stdout 是 `编号<TAB>名字`（存好以后才说）
- 存之前查：名字 1～40 个字、不重复；最多 16 个（`MAX`）；参数最多 64 个、合起来 4096 字节；只能起交互的会话（`-p`、子命令这类跑完就退出的不行：
  `claude_interactive` / `codex_interactive` / `agy_interactive`）；参数过得了启动前的检查（`args_problems`：带着参数的 launch-check 比不带的多出来的问题）
- 用不了的（`problem`）：`no_account`（账号表里没有了）、`no_dir`、`bad_args`（共享的设置变了也会这样）、`no_relay`（这个账号的会话不经哪个 Team 的服务器）。
  `list --json` 给界面 App（PROTOCOL 9.4）：`templates[]` 多 `account_name`、`line`、`digest`、`problem`，`launcher` 是 `{running, templates, peers}`
- 摘要 `digest`：`hex(SHA256(紧凑 JSON [工具, 账号, 文件夹, 参数数组]))` 的前 16 个字符（名字不算：改名不算改过）

## 打开（`cca remote allow 手机 launch on|off`）

- bash 的 `remote_allow_launch`：`py remote-allow --show 名字`（`配对编号<TAB>名字<TAB>web<TAB>incoming<TAB>launch`）认出是谁；打开时不是手机、不是 control 的直接停，
  先把要做的说清楚（能用几个模板、模板里有 `--dangerously-skip-permissions` 这类参数的话手机上起的也不问就执行、要装一个后台进程），再 `consent remote_launch 配对编号 … n`
- 改好（`py remote-allow 名字 launch on|off`，peers.json 那一行只在 true 时写 `"launch": true`）以后 `launcher_sync`：`py remote-launcher wanted`（有允许远程启动的手机：
  退出码 0）就 `launcher_install`，没有了就 `launcher_remove`。改 `incoming`、取消配对以后也 `launcher_sync`
- `cca remote` 的状态里多一句「远程启动开着」；`cca remote launcher [status]` 说哪几台手机、后台进程在不在跑、几个模板、最近一次；`cca remote launcher log` 列日志

## 后台进程（`cca remote launcher serve`）

- macOS：`~/Library/LaunchAgents/local.cca.remote-launcher.plist`（辅助程序 `remote-launcher plist cca的路径` 写：`ProgramArguments` 是 `[cca, remote, launcher, serve]`、
  `RunAtLoad`、`KeepAlive {SuccessfulExit: false}`（出错退出了才再起，正常退出——没有手机了——不再起）、`ThrottleInterval 30`、`AbandonProcessGroup`、
  `EnvironmentVariables {PATH, LANG, CCA_HOME}`（装的时候的）。`launcher_install`：内容变了（cca 挪了地方、PATH 变了）才先 `bootout`、换文件；
  再 `launchctl bootstrap gui/<uid>`（已经装着时它说 `Bootstrap failed: 5`，不算）、`kickstart gui/<uid>/local.cca.remote-launcher`（没在跑的起来）。卸载 `bootout`、删掉文件、`py remote-launcher stop`
- Linux：`cron_tagged`（和内置代理开机起内核的那一行共用）管两行，记号 `# cca-remote-launcher`：`@reboot cca remote launcher serve --if-down`、
  `*/10 * * * *` 同样的一行（`--if-down`：已经在跑就不说话地退出）；装上时另外马上起一个（perl 的 setsid）。没有 crontab 命令时装不上，只说一句
- 只有一个：`launcher.lock`（flock；别的进程问它在不在跑时只拿共享锁、一下就放开）。状态在 `launcher.json`（600：进程号、开始的时间、每个 Team 的会话编号 `sids`——
  重启以后照旧用它，手机上不会多出一个启动器）。`cca remote launcher serve` 在终端里也能直接跑（已经有一个时说一句就退出）
- 循环（`serve`）：每 3 秒（`CHECK_EVERY`）看配对表变了没有，照允许远程启动的手机算出要连哪几台服务器，一台一个转接进程（`remote-link --launcher`）；
  一台都不用了就退出（退出码 0）。每 3 秒看辅助程序的文件还在不在、每 30 秒（`SELF_EVERY`）看 cca 换没换版本：没了、换了就收场、经 cca 重新执行自己（进程号不变，launchd 照旧管着）。
  转接进程退出码 3（这台服务器用不了：不在 Team 里了、太旧）的 600 秒后再试（`OFF_RETRY`；配对表变了马上试）
- 转接进程（和会话的转接进程合用 `link.rs` 的 `Link`：看配对表、连服务器和退避、分子进程、收场）：照会话那样拿票据、连数据端口，ANNOUNCE 里给每个允许的手机一份启动器的说明（最多 6 台：`PEERS_MAX`），每 60 秒（`DOC_EVERY`）重算一遍（账号的别名、
  文件夹变了也跟着变）。手机接进来（JOIN）分出一个子进程（最多 4 个、10 秒内最多分 8 次：`link.rs` 的 `MAX_KIDS`、`FORKS_PER_10S`；同一台手机两次接进来至少隔 3 秒：
  `JOIN_GAP`）：握手（只认这一台手机的配对）、等它的头一个 LAUNCH（最多 20 秒，别的帧不理）、现读配对表和模板再查一遍、起会话、回 LAUNCHED、断开，
  在 `launch.log`（600，64 KB 换成 `launch.log.1`）记一行 `时间  手机  模板的名字  起了会话 编号前 8 位` 或 `没起（code）`（不记开场白、参数）
- 查的先后（`launch`）：这台手机还让不让（`denied`）、一分钟里起成了 4 个没有（`rate`）、有没有这个模板（`no_template`）、摘要对不对（`changed`）、账号还在不在
  （`no_account`）、账号的会话走不走这台服务器（`other_hub`）、文件夹（`no_dir`）、参数（`bad_args`）、带了开场白模板却不让带（`bad`）、这台机器走这台服务器的会话
  到 7 个没有（`busy`：服务器给一台设备 8 个，启动器自己占一个）、起没起来（`spawn`：4 秒内等不到新会话的 socket）。给手机看的话在 `fail_text`
- 会话的通道上（`link.rs` 的 `bridge`）不转 LAUNCH：接着一个会话的手机起不了别的

## 起的会话

- `remote-host --headless --sid 编号 --size 列x行 工具 账号键 名字 -- cca [codex|agy] @账号键 -- 模板的参数… [-- 开场白]`（`spawn_session`），当前目录是模板的文件夹，
  自己一个会话（setsid），标准输入输出是 /dev/null
- 环境只带 `KEEP_ENV`：`HOME USER LOGNAME PATH LANG LC_ALL LC_CTYPE TMPDIR SHELL SSH_AUTH_SOCK __CF_USER_TEXT_ENCODING CCA_HOME CCA_HELPER CCA_KEYSTORE TZ CCA_HEADLESS_LINGER CCA_HEADLESS_IDLE`，
  再加 `TERM=xterm-256color`、`CCA_REMOTE=1`、`CCA_REMOTE_HOSTED=会话编号`。后台进程的环境是 launchd / cron 给的，别的不该带进会话；`CCA_KEYSTORE`（密钥存成文件，只给测试）
  要和启动器一样：会话的转接进程照它找配对的密钥。之后 cca 照常 `child_env`（isolation.md），客户端看到的和在终端里启动的一样
- bash 的 `remote_prepare`：`CCA_REMOTE_HOSTED` 有值时不再套一层宿主（已经在无头的宿主里了），但算在宿主里（Codex 的 `--hosted`）；这个变量只认这一次，unset 掉、不传给客户端
- 无头的宿主（`host.rs` 的 `--headless`）：没有启动它的终端，大小先照 `--size`，之后照接进来的（remote.md 的「几个终端接着同一个会话」）。客户端退出时还没人接入过：
  留着等人接上来看它最后说了什么（`CCA_HEADLESS_LINGER`，默认 600 秒），接上的那一处收到退出码；一直没人接入：`CCA_HEADLESS_IDLE`（默认 900 秒）后照 `cca remote end` 那样结束

## 线上的格式

见 server/README.md「启动器」：说明的密钥 `Kl = HMAC(P, "cca-remote/v1/launch")`、标签 `cca-remote-launch`（会话的说明的密钥解不开它：旧的 App 当它不存在），
帧 LAUNCH `0x09`（手机 → 启动器）、LAUNCHED `0x18`（启动器 → 手机）。四份实现里有三份要认它：辅助程序（`crypto.rs` 的 `launch_seal`、`template.rs` 的 `launch_doc`、
`launch.rs` 的 `Ask`）、`tests/kit/spec.rs`、CcaRemoteKit 的 `Launch.swift`；网页端（`server/web/core.js`）不做远程启动。

## 手机、界面 App

- iPhone：会话列表里有启动器的电脑下面多一行「新建会话」，选模板、开场白、起来了马上接进去；见 ios.md「新建会话（远程启动）」
- Mac 的界面 App：「远程会话」页最下面一节管模板（新建、改、删；参数照 shell 的写法拆开，`ShellWords`），手机那一行有「允许远程启动」开关（先弹同意）；
  要 `hello.remote_launch`。见 PROTOCOL 9.4、macos/README.md

## 测试

- `tests/remote_launch.rs`（suite 的 `remote_launch`）：真的服务器、真的启动器（`remote-launcher serve`，`CCA_SELF` 指着假的客户端），手机是照 server/README.md
  独立实现的那一头（`kit::relay::SpecPeer`）：没有允许的手机时启动器马上退出；只让看的打不开；起一个（参数、开场白、大小、环境、当前目录照模板和手机说的）、
  马上接进去、退出码；经会话的通道发的 LAUNCH 不理；日志；SIGTERM 重启以后同一个编号；关掉以后启动器自己退出、说明没了。另一个用例是各种拒绝
- `tests/remote_host.rs` 的 `headless_sessions`：无头的宿主（参数不对、接入、没人接入时留多久、多久结束）
- `tests/ios_kit.rs`：`against_the_spec` 的 `launch`（启动器的说明解开、清理，拼 LAUNCH、读 LAUNCHED，和 `kit::spec` 对）；`a_phone_launches_a_session`：ccaguest
  对着真的启动器列模板、起会话、`--fresh` 接进去
- 回归：3e（`CCA_REMOTE_HOSTED` 不再套一层）；11b3c 在和手机配好以后（阁楼组的服务器）：模板的增删改、`--json`（给 App 的测试数据 `remote_templates.json`、
  `remote_list_launch.json`）、打开要同意（终端、App）、LaunchAgent 的内容和装卸（假的 launchctl 的用户域：照 `ProgramArguments` 真的起后台进程，`bootout` 停掉）、
  改成只看 / 取消配对时卸掉；12：`cca restore` 卸掉 LaunchAgent。Linux 的 crontab 那一支只在代码里（`tests/linux.sh` 没测）
- App：`macos/Tests/ContractTests/RemoteLaunchTests.swift`；iOS：`ios/Tests/KitTests` 的 `launch`、模拟器里的演示模式（`-screen launch`）

## 没验证的

- 真的 LaunchAgent：登录以后起不起来、launchd 给的环境里找不找得到 claude / codex（PATH 是装的时候的）、从 LaunchAgent 起的进程读不读得了钥匙串里配对的密钥
- 真机上起会话；TestFlight 还没传带这一页的构建
- Linux 上的 crontab 两行、开机以后起来
