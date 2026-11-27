# 远程启动

配对的设备照这台机器上的模板新建一个会话：这台机器前面不用有人。起来的会话和在终端里启动的一样
（同一个账号的环境、出口、隔离），那一头马上接进去；这台机器上照样 `cca remote attach` 得到、`cca remote end` 得了。2026-11-26 维护者定的（「支持远程启动 cca，
并支持添加若干启动参数的模板」）。随 3.7.6 发布（2026-11-27），那时只给手机（iOS 上的 cca App，配对过的「访客」），iPhone 上的 App 在第六个 TestFlight 构建里（20261126.183151）；协议核心和真的 cca、真的服务器端到端测过（下面「测试」），真机、真的 LaunchAgent 还没试。
2026-11-27 维护者又定了 Mac 上的 App 也要能远程启动（「macOS 客户端也需要支持远程启动会话」）：配对过的电脑也能打开远程启动，装着 cca 的机器用
`cca remote launch`、Mac 上的 cca App 用「新建会话…」照那台机器上的模板起（下面「在别的机器上起」）；随 3.7.7 发布（2026-11-27）：真的辅助程序、真的服务器端到端测过（`a_machine_launches_a_session`），App 有契约测试和截图，两台真的电脑之间、App 里点「新建会话…」还没试。
同一天维护者又定了模板里不放账号（「远程会话模板中不需要指定账号，应该是在使用模版要启用远程会话时选择账号」）：起会话时那一头挑（下面「起的时候挑账号」）；
一起定的「同时至多 100 个会话」见 remote.md「一台设备 100 个会话」。随 3.7.10 发布（2026-11-27）：真的辅助程序、真的服务器端到端测过，两个 App 有测试和截图，真机、两台真的电脑之间还没试。
同一天维护者又定了新建模板要选归属的 Team（「新建模版需要选择归属的Team组」）：只有在这个 Team 的服务器上配对的设备看得到、起得了它（下面「归属的 Team」）。
随 3.7.12 发布（2026-11-28）：真的辅助程序、真的服务器端到端测过（`what_changed_on_the_host`），Mac 上的 App 有契约测试，没人在 App 里点过。
同一天维护者又说手机上结束不了照模板起的会话（「iOS端按照模版新建的远程会话，无法在iOS上结束」）：能操作的配对结束得了远程启动的会话（下面「起的会话」），
iPhone 上的 App 在终端页给「结束会话」；随 3.7.12 发布（2026-11-28）：真的辅助程序、真的服务器端到端测过，App 在演示模式里截图看过、模拟器里端到端测过，真机上还没试。

定下来的（别重新讨论）：
- 只能照模板起：那一头（手机、配对过的电脑）只挑模板、挑账号、给窗口大小和一句开场白，文件夹、参数都在这台机器上定好（`cca remote template`）；账号只能挑这台机器的
  账号表里模板那个工具的、会话走同一台服务器的（下面「起的时候挑账号」）。那一头给不了任意的命令、参数、文件夹
- 只给能操作这台机器的配对：能看和操作（`incoming` 是 `control`）、单独打开过远程启动（`launch`，打开要同意：同意项 `remote_launch`，
  范围是配对编号，默认不同意）。改成只让看、不让接入时 `launch` 跟着清掉，改回来不跟着回来。3.7.6 还要是手机（`web`）；2026-11-27 起手机、电脑都行
  （`peers.rs` 的 `can_launch` 只看 `incoming`）
- 开场白放进参数：模板能带开场白（`line`：claude、codex，参数里还没有一句话——除了选项和选项的值没有别的参数，也没有 `--`）时，那一头给的那句作为客户端的
  最后一个参数（`-- 开场白`），不是起来以后往终端里敲。参数里已经有一句话的再接一句：claude 只拿头一句、那一头给的那句不理（2.1.332：多出来的参数
  它不拦也不用），codex 起不来（0.172.0：`unexpected argument`），所以不给带（`template.rs` 的 `has_words`）
- 服务器不为它改：启动器在每台走得到的 Team 服务器上登记成一个“会话”（`relay_host`，占这台设备的一个会话名额：3.7.10 起的服务器 101 个，以前的 8 个）；
  那一头和它握手、说要起哪个，跟接入会话一样
- 模板的摘要（`h`）：那一头列出来以后这台机器上改了模板，拿旧的摘要来起是 `changed`，不照改过的起
- 模板归属这台机器在的一个 Team：只有在这个 Team 的服务器上配对的设备看得到、起得了它；3.7.11 及以前存的没有归属，哪个 Team 配对的设备都看得到（和以前一样），
  改的时候再选（下面「归属的 Team」）
- 新会话的大小是那一头说的（列 20～500、行 5～300）：iPhone 上的 App 记着在会话里打字那一刻的窗口（新的宿主就是那时照手机排会话）；从没打过字照屏幕算、高只算一半（键盘会弹出来）。
  `cca remote launch` 用这个终端的大小；Mac 上的 App 用上一次在「远程会话」页开的终端的大小，还没开过是 120x36
- 刚起的会话服务器可能还不知道：手机接入时服务器说 `no_session` 不算会话结束，45 秒内每 1.5 秒再问（CcaRemoteKit `Attachment` 的 `fresh`）；
  `cca remote launch` 是先等它在 `relay_list` 里出现再接（`start.rs` 的 `FRESH_FOR`、`FRESH_GAP`，同样的 45 秒、1.5 秒）
- 起的会话的环境只带白名单（`KEEP_ENV`，下面「起的会话」）；模板、日志里没有凭证，不进 `CCA_DIR_PRIVATE`
- macOS 上后台进程是用户的 LaunchAgent，Linux 上是用户 crontab 里的两行（`@reboot` 和每 10 分钟一次 `serve --if-down`）：都不用 root

## 模板（`cca remote template`）

- 存在 `~/.cca/remote/templates.json`（600，改之前拿 `Lock`）：`{"v":1,"templates":[{"id","name","tool","dir","args","at","team"}]}`。
  `id` 是 8 个小写十六进制字符；`dir` 是绝对路径，要已经在；`team` 是归属的 Team 的编号（下面「归属的 Team」）。账号不在模板里（起的时候挑）：3.7.9 及以前的模板带着的 `account` 读的时候不理、存的时候不再写
- 辅助程序 `cli/src/cmd/remote/template.rs`（`remote-template list [--json | --team 编号] | teams | add 名字 工具 文件夹 [--replace 编号] [--team Team] (-- 参数… | --args-line 一行) | rm 名字或编号`，
  照位置认：名字、参数那一行本身是 `--` 也认得对；`--replace`、`--team` 在参数前面，先后都行）；bash 的 `remote_template`：`cca remote template [list [--json]]`、
  `add [--name][--tool][--dir][--team Team 的名字][--replace 编号] [-- 参数… | --args-line 一行参数]`（没写的在终端里 `ask`，界面 App 都给全；3.7.9 及以前的 App 还会给 `--account`：认得、不理）、`rm 名字或编号`。
  `add` 的 stdout 是 `编号<TAB>名字<TAB>归属的 Team 的名字`（存好以后才说；没有归属的最后一项是空的）；终端里的 `list` 一行是 `编号  名字  工具  归属的 Team  文件夹  参数`
- 参数两种给法，只能用一种：一项一项放在 `--` 后面（终端里），或者照 shell 的写法写成的一行（`--args-line`：界面 App 一律用它，弹层里的那一行原样给），
  辅助程序 `sh_split` 拆（`shlex.split` 的 POSIX 规则：不展开 `~`、`$`、通配符；拆不开说「参数里的引号没有配对」「参数最后多了一个 \」）。
  反过来 `args_line`（`list --json`）是 `sh_quote` 一项项拼的（要引的才用单引号），拆回去是原样：App 显示、修改都用它，不自己拆、拼
- 存之前查：名字 1～40 个字、不重复；最多 16 个（`MAX`）；参数最多 64 个、合起来 4096 字节；只能起交互的会话（`-p`、子命令这类跑完就退出的不行：
  `claude_interactive` / `codex_interactive` / `agy_interactive`）；参数过得了启动前的检查（`args_problems`：带着参数的 launch-check 比不带的多出来的问题）
- 用不了的（`problem`，先后照这个）：`no_team`（这台机器退出了归属的 Team）、`no_account`（这个工具一个账号都没有）、`no_dir`、`bad_args`（共享的设置变了也会这样）、
  `no_relay`（这个工具没有会话走归属的 Team 的服务器的账号；没有归属的：这个工具的账号的会话都不经哪个 Team 的服务器）。
  `list --json` 给界面 App（PROTOCOL 9.4）：`templates[]` 多 `args_line`、`line`、`digest`、`team`、`team_name`、`problem`，`teams` 是能归属的 Team（`{id, name, default}`），`launcher` 是 `{running, templates, peers}`；
  给 3.7.9 及以前的 App 的 `account` 是空字符串、`account_key`、`account_name` 是 null（它当这个模板的账号没了：改模板时要先选一个账号才存得了，选的 cca 不理）
- 摘要 `digest`：`hex(SHA256(紧凑 JSON [工具, 文件夹, 参数数组]))` 的前 16 个字符（名字不算：改名不算改过；3.7.9 及以前账号也算在里面）

## 归属的 Team

2026-11-27 维护者定的（「新建模版需要选择归属的Team组」）：每个模板归属这台机器在的一个 Team（`team`：Team 的编号）。

- 看得到、起得了的：只有在这个 Team 的服务器上配对的设备（`Template::for_hub`）。给那一头的说明本来就按服务器各算各的（`LaunchDoc::build`）：别的 Team 的服务器上配对的设备
  收到的说明里没有它；起会话时 `launch` 再查一遍，不是这个 Team 的当它没有（`no_template`：编号、摘要都对也一样，不说它在别的 Team 里）。能挑的账号照旧是会话走这台服务器的
  （下面「起的时候挑账号」）
- 新建时选：`--team 编号或名字`（这台机器在的；名字重了要写编号，`team_named`）；没说的用 `default_team`：这台机器的家（`relay.rs` 的 `home_hub`：加入的第一个、用得了它的服务器的），
  一台都用不了时是加入的第一个；不在哪个 Team 里建不了（`NO_TEAM_WHY`）。终端里这台机器在好几个 Team 里、没写 `--team` 时 bash 问一句（`remote-template teams`：一行 `编号<TAB>名字`，
  默认的那一个后面多一列 `default`；直接回车是它）；界面 App 一律给编号（弹层里选，`list --json` 的 `teams`）。改模板（`--replace`）没说就不变
- 3.7.11 及以前存的没有 `team`：哪个 Team 配对的设备都看得到（和以前一样）；终端里写「所有 Team」，App 改它时先选上默认的那一个，存了就有归属。
  `team` 写坏了的（不是 32 个小写十六进制字符）整条不认：不能当它没有归属，那样哪个 Team 都看得到
- 这台机器退出了归属的 Team：`problem` 是 `no_team`（先于别的原因说），改的时候换一个
- 打开远程启动之前说那台设备能用几个模板：`remote-allow --show` 最后多一列这个配对在哪个 Team 的服务器上，bash 照它 `remote-template list --team 编号` 数（只列它看得到的）

## 起的时候挑账号

2026-11-27 起（3.7.10）：模板不带账号，那一头起会话时挑。

- 能挑的（`template.rs` 的 `picks`）：模板那个工具的账号表里的账号，照账号表的先后，会话走这台服务器的（`relay.rs` 的 `session_hub`：Team 给的走那个 Team 的，
  这台机器自己的、第三方 API 的走「家」）。别的 Team 给的（会话走那个 Team 的服务器）不在里面：在那个 Team 的配对上挑
- 说明里多一项 `accounts`（server/README.md「启动器」）：说明里的模板用到的工具的能挑的账号，一个工具最多 16 个（`PICKS_MAX`），各带按配对算的编号
  （`status.rs` 的 `account_tag`：和手机「账号」页里同一个账号的编号一样，手机照它找头像、额度；换别名不变，别的配对、服务器看不出是哪个账号）、工具、
  短名字、账号摘要（Team 给的才有）。一个能挑的都没有的模板不放进说明。模板那一行的 `a`、`aid` 留给旧版的 App、cca（它们不认 `accounts`）：
  能挑的只有一个时是它的短名字和账号摘要，几个时 `a` 是「启动时选账号」（`ANY_ACCOUNT`）、没有 `aid`
- LAUNCH 多一项 `a`（挑的账号的编号）。启动器（`launch.rs` 的 `pick_account`）照这个配对的密钥算各个能挑的账号的编号来找：找不到是 `no_account`（删了、换了邮箱，
  或者是别的配对的编号），找到了但会话不走这台服务器是 `other_hub`；没说的（旧版的 App、cca）能挑的只有一个就用它，一个都没有是 `no_account`，几个是
  `need_account`（叫它更新）。起会话前重读各个 Team 的配置（`forget_grants`：启动器常驻，`grants()` 一个进程只读一次，账号可能刚改归别的 Team）
- 那一头：iPhone 的「新建会话」选了模板再选账号（ios.md「新建会话（远程启动）」）；`cca remote launch … --account 账号`（下面「在别的机器上起」）；
  Mac 上的 App 的「新建会话…」（PROTOCOL 9.5）。那台机器的 cca 是 3.7.9 及以前的（说明里没有 `accounts`）时照旧：模板用它自己的账号，不用挑

## 打开（`cca remote allow 配对的名字 launch on|off`）

- bash 的 `remote_allow_launch`：`py remote-allow --show 名字`（`配对编号<TAB>名字<TAB>web<TAB>incoming<TAB>launch`）认出是谁；打开时不是 control 的直接停（3.7.6 不是手机的也停），
  先把要做的说清楚（能用几个模板、模板里有 `--dangerously-skip-permissions` 这类参数的话那一头起的也不问就执行、要装一个后台进程），再 `consent remote_launch 配对编号 … n`
- 改好（`py remote-allow 名字 launch on|off`，peers.json 那一行只在 true 时写 `"launch": true`）以后 `remote_launcher_sync`：`py remote-launcher wanted`（有允许远程启动的配对：
  退出码 0）就 `remote_launcher_install`，没有了就 `remote_launcher_remove`。改 `incoming`、取消配对以后也照配对表装卸，但用 `remote_launcher_sync keep`：
  装着的 LaunchAgent 不重写（plist 里是装的时候的 PATH、LANG：在 App 里、ssh 进来时跑的命令环境不一样，重写要先 `bootout`，正在起的会话会被打断），
  只有开关远程启动时才照这一次的环境换。Linux 上没装（crontab 里没有那两行）时卸载不碰 crontab（`remote_launcher_installed`）
- `cca remote` 的状态里多一句「远程启动开着」；`cca remote launcher [status]` 说哪几台配对的设备、后台进程在不在跑、几个模板、最近一次；`cca remote launcher log` 列日志

## 后台进程（`cca remote launcher serve`）

- macOS：`~/Library/LaunchAgents/local.cca.remote-launcher.plist`（辅助程序 `remote-launcher plist cca的路径` 写：`ProgramArguments` 是 `[cca, remote, launcher, serve]`、
  `RunAtLoad`、`KeepAlive {SuccessfulExit: false}`（出错退出了才再起，正常退出——没有允许的配对了——不再起）、`ThrottleInterval 30`、`AbandonProcessGroup`、
  `EnvironmentVariables {PATH, LANG, CCA_HOME}`（装的时候的）。`remote_launcher_install`：内容变了（cca 挪了地方、PATH 变了）才先 `bootout`、换文件；
  再 `launchctl bootstrap gui/<uid>`（已经装着时它说 `Bootstrap failed: 5`，不算）、`kickstart gui/<uid>/local.cca.remote-launcher`（没在跑的起来）。卸载 `bootout`、删掉文件、`py remote-launcher stop`
- Linux：`cron_tagged`（和内置代理开机起内核的那一行共用）管两行，记号 `# cca-remote-launcher`：`@reboot cca remote launcher serve --if-down`、
  `*/10 * * * *` 同样的一行（`--if-down`：已经在跑就不说话地退出）；装上时另外马上起一个（perl 的 setsid）。没有 crontab 命令时装不上，只说一句
- 只有一个：`launcher.lock`（flock；别的进程问它在不在跑时只拿共享锁、一下就放开）。状态在 `launcher.json`（600：进程号、开始的时间、每个 Team 的会话编号 `sids`——
  重启以后照旧用它，那一头不会多看到一个启动器）。`cca remote launcher serve` 在终端里也能直接跑（已经有一个时说一句就退出）
- 循环（`serve`）：每 3 秒（`CHECK_EVERY`）看配对表变了没有，照允许远程启动的配对算出要连哪几台服务器，一台一个转接进程（`remote-link --launcher`）；
  一台都不用了就退出（退出码 0）。每 3 秒看辅助程序的文件还在不在、每 30 秒（`SELF_EVERY`）看 cca 换没换版本：没了、换了就收场、经 cca 重新执行自己（进程号不变，launchd 照旧管着）。
  转接进程退出码 3（这台服务器用不了：不在 Team 里了、太旧）的 600 秒后再试（`OFF_RETRY`；配对表变了马上试）
- 转接进程（和会话的转接进程合用 `link.rs` 的 `Link`：看配对表、连服务器和退避、分子进程、收场）：照会话那样拿票据、连数据端口，ANNOUNCE 里给每个允许的配对一份启动器的说明（最多 6 台：`PEERS_MAX`）：看配对表的时候顺便看模板文件、各个工具的账号表、各个 Team 的配置变了没有
  （`template::stamp`），变了马上重算，不然每 60 秒（`DOC_EVERY`）重算一遍（文件夹没了、账号的别名变了也跟着变）；重算之前重读各个 Team 的配置（`forget_grants`）。那一头接进来（JOIN）分出一个子进程（最多 4 个、10 秒内最多分 8 次：`link.rs` 的 `MAX_KIDS`、`FORKS_PER_10S`，超了不理）：握手（只认这一个配对）、
  等它的头一个 LAUNCH（最多 20 秒，别的帧不理；缺了哪一项都是 `bad`）、现读配对表和模板再查一遍、起会话、回 LAUNCHED、断开，
  在 `launch.log`（600，64 KB 换成 `launch.log.1`）记一行 `时间  配对的名字  模板的名字  起了会话 编号前 8 位` 或 `没起（code）`（不记开场白、参数）。
  同一个配对两次接进来不到 3 秒（`JOIN_GAP`）：照样接、回 `rate`（不理的话那一头要等到服务器说启动器不在，说的原因就错了；这一次不算进间隔）
- 配对的密钥读不出来（钥匙串一时读不了：登录以后还没解锁之类）的配对先不接，30 秒（`link.rs` 的 `SECRET_RETRY`）后再读，不用等配对表变；
  启动器在 `launch.log` 里记一句（每个配对一次，读出来以前不再记）
- 查的先后（`launch`）：这个配对还让不让（`denied`）、一分钟里起成了 4 个没有（`rate`）、有没有这个模板（`no_template`：归属别的 Team 的也算没有）、摘要对不对（`changed`）、用哪个账号
  （`pick_account`：`no_account`、`other_hub`、`need_account`，上面「起的时候挑账号」）、文件夹（`no_dir`）、参数（`bad_args`）、带了开场白模板却不让带（`bad`）、
  这台机器走这台服务器的会话到上限没有（`busy`：`sessions_max`，同步配置时记下的服务器级别——sync.json 的 `level`——到 3 是 100 个，不到、没问到过是 7 个：
  3.7.10 起的服务器给一台设备 101 个、以前的 8 个，启动器自己占一个）、起没起来（`spawn`：4 秒内等不到新会话的 socket）。给那一头看的话在 `fail_text`（`cca remote launch` 不用它：照 `code` 自己说，`start.rs` 的 `refused_text`）
- 会话的通道上（`link.rs` 的 `bridge`）不转 LAUNCH：接着一个会话的那一头起不了别的

## 起的会话

- `remote-host --headless --sid 编号 --size 列x行 工具 账号键 名字 -- cca [codex|agy] @账号键 -- 模板的参数… [-- 开场白]`（`spawn_session`），当前目录是模板的文件夹，
  自己一个会话（setsid），标准输入输出是 /dev/null
- 环境只带 `KEEP_ENV`：`HOME USER LOGNAME PATH LANG LC_ALL LC_CTYPE TMPDIR SHELL SSH_AUTH_SOCK __CF_USER_TEXT_ENCODING CCA_HOME CCA_HELPER CCA_KEYSTORE TZ CCA_HEADLESS_LINGER CCA_HEADLESS_IDLE`，
  再加 `TERM=xterm-256color`、`CCA_REMOTE=1`、`CCA_REMOTE_HOSTED=会话编号`。后台进程的环境是 launchd / cron 给的，别的不该带进会话；`CCA_KEYSTORE`（密钥存成文件，只给测试）
  要和启动器一样：会话的转接进程照它找配对的密钥。之后 cca 照常 `child_env`（isolation.md），客户端看到的和在终端里启动的一样
- bash 的 `remote_prepare`：`CCA_REMOTE_HOSTED` 有值时不再套一层宿主（已经在无头的宿主里了），但算在宿主里（Codex 的 `--hosted`）；这个变量只认这一次，unset 掉、不传给客户端
- 无头的宿主（`host.rs` 的 `--headless`）：没有启动它的终端，大小先照 `--size`，之后照接进来的（remote.md 的「几个终端接着同一个会话」）。客户端退出时还没人接入过：
  留着等人接上来看它最后说了什么（`CCA_HEADLESS_LINGER`，默认 600 秒），接上的那一处收到退出码；一直没人接入：`CCA_HEADLESS_IDLE`（默认 900 秒）后照 `cca remote end` 那样结束
- 那一头结束得了它（2026-11-27 维护者说的「iOS端按照模版新建的远程会话，无法在iOS上结束」）：说明文件里记着 `headless`（`Meta.headless`），给能操作的配对的会话的说明里
  多一项 `"end": true`，转接进程只替这样的配对转 `END`（remote.md「结束一个会话」）：宿主照 `cca remote end` 那样结束它，那一头收到退出码（被 SIGHUP 结束的是 129）。
  iPhone 上的 App 在终端页右上角给「结束会话」（先问一句，ios.md「App」）；终端里起的会话照旧只在这台机器上结束得了

## 在别的机器上起（`cca remote launch`）

2026-11-27 起：装着 cca 的机器也能当远程启动的那一头，用配对过的另一台机器上的模板在那台机器上新建会话（那台机器上
`cca remote allow 这台的名字 launch on`；那台的 cca 也要是这以后的：3.7.6 的不让电脑打开）。

- 列出来：`far.rs` 的 `fetch` 问 `relay_list` 时，会话的说明解不开的再用 `crypto.rs` 的 `launch_open` 试，解得开的是启动器（`FarLauncher`，一个配对认头一个）；
  `launcher_of` 照 server/README.md「启动器」读说明：编号、摘要、工具不对的、编号重复的丢掉，名字、账号、文件夹、参数照会话的说明那样去掉控制字符、截短，`more` 夹在 0～99；
  能挑的账号（`accounts`，`FarAccount`）同样丢掉编号、工具不对的和重复的，旧版的 cca 没有这一项（`None`：模板用它自己的账号）。
  账号的名字照会话那样换成这台机器上的别名（`rename_here`），那台机器说的留在 `host_account` / `host_name`。`remote-list --json` 多一项 `peer_launchers`
  （`manage.rs` 的 `launcher_entry`，PROTOCOL 9.1）；终端里的 `cca remote list` 有启动器时在 stderr 说一句去 `cca remote launch` 看模板
- 辅助程序 `cli/src/cmd/remote/start.rs`（`remote-launch [机器] [模板] [--account 账号] [--line 开场白] [--size 列x行] [--digest 摘要] [--json]`；bash 的 `cca remote launch` 原样交给它，
  关掉 fd 3）：不写模板时列出各台机器上能用的（`机器  编号  名字  工具  账号  文件夹  参数`，账号是起的时候能挑的那几个），写了照名字或者编号认（只有一台机器上有这个模板时不用写机器；
  机器照 `cca remote allow` 那样认：名字、配对编号的前几位）。`--account`（`pick_for`）认编号（`peer_launchers[].accounts[].id`）和名字（这台机器上的别名、那台机器说的名字；
  重名的要用编号）：能挑的只有一个时不用写，几个时不写是 64，没有这个账号 65；那台机器的 cca 是旧版（说明里没有 `accounts`）时不能写（64）。先查完（在不在终端里、开场白、模板的 `line`、启动器连没连着服务器）再连：不然起了接不进去。
  拿票据、握手（`attach.rs` 的 `dial`，和接入会话一样）、发 LAUNCH、等 LAUNCHED（`ANSWER_WAIT` 25 秒：启动器等 LAUNCH 20 秒、起会话最多 4 秒），
  起成了再问 `relay_list` 等它登记好（`FRESH_FOR` 45 秒，每 `FRESH_GAP` 1.5 秒问一次：服务器给每台设备限了流，问得太勤别的命令会被拒），
  然后在这个终端里接进去（`attach_launched`：之后和 `cca remote attach` 一样）
- 退出码：64 用法不对、对上了好几个模板，65 没有这台机器、这个模板、那台机器没让这台远程启动（或者它的启动器不在），66 经服务器没成（连不上、那台机器不在线、
  没等到回话、45 秒还没登记好），67 那台机器不起（LAUNCHED 的 `code` 照这一头说：`refused_text`，比如 `changed` 是「模板在那台机器上改过了：刷新一下再起」；
  不认识的 `code` 用那台机器写的 `text`），69 不在终端里（不带 `--json` 时）。原因写 stderr，设了 `CCA_EVENTS` 时另记一条 `fail` 事件（`code` 是 `error`）
- `--json`（界面 App，PROTOCOL 9.5）：不接进去，等登记好以后打出 `{"version":1,"kind":"remote_launch","id":新会话的编号,"session":remote list 里它那一项或者 null}`；
  `--digest` 是 App 列出来时看到的摘要（不写时用这次现查到的）
- 同一个配对两次接进启动器不到 3 秒是 `rate`（启动器的 `JOIN_GAP`）：一次 `cca remote launch` 只接一回，App 一次只跑一个
- 界面 App：`peer_launchers` 在「可以接入的会话」里自成一组（`RemoteSessionsPage` 的 `SessionGroup.launcher`，没有会话也有），标题旁边是「新建会话…」：
  弹层 `RemoteLaunchSheet`（`RemoteLaunchDraft`：挑模板，只有一个时先选上；挑账号：这个模板的工具能挑的，只有一个时先选上，换了别的工具的模板、选着的不在能挑的里面了就不选，
  那台机器的 cca 是旧版时不挑、不给 `--account`；能带开场白的写一句），`AppModel.startRemoteLaunch` 跑 `cca remote launch … --json`
  （`CcaCommand.remoteStart`），起好了照 PROTOCOL 9.3 接入（能操作）；没成把 cca 说的原因写在弹层里、再查一次列表；`session` 是 null 时说一句、不接。
  要 `hello.remote_launch_machines`：3.7.6 的 cca 没有，App 照旧只给手机那一行「允许远程启动」，没有「新建会话…」

## 线上的格式

见 server/README.md「启动器」：说明的密钥 `Kl = HMAC(P, "cca-remote/v1/launch")`、标签 `cca-remote-launch`（会话的说明的密钥解不开它：旧的 App 当它不存在），
帧 LAUNCH `0x09`（那一头 → 启动器）、LAUNCHED `0x18`（启动器 → 那一头）。四份实现里有三份要认它：辅助程序（`crypto.rs` 的 `launch_seal` / `launch_open`、`template.rs` 的 `launch_doc`、
`launch.rs` 的 `Ask`；那一头是 `far.rs` 的 `launcher_of`、`start.rs`）、`tests/kit/spec.rs`、CcaRemoteKit 的 `Launch.swift`；网页端（`server/web/core.js`）不做远程启动。

## 手机、界面 App

- iPhone：会话列表里有启动器的电脑下面多一行「新建会话」，选模板、选账号、开场白、起来了马上接进去；见 ios.md「新建会话（远程启动）」
- Mac 的界面 App：「远程会话」页最下面一节管模板（新建、改、删；参数是一行字：cca 给 `args_line`、存的时候 `--args-line` 原样交给 cca 拆，App 不自己拆），手机那一行有「允许远程启动」开关（先弹同意；
  cca 有 `hello.remote_launch_machines` 时电脑那一行也有）；要 `hello.remote_launch`。用别的机器上的模板起：上面「在别的机器上起」。见 PROTOCOL 9.4、9.5、macos/README.md

## 测试

- `tests/remote_launch.rs`（suite 的 `remote_launch`）：真的服务器、真的启动器（`remote-launcher serve`，`CCA_SELF` 指着假的客户端），手机是照 server/README.md
  独立实现的那一头（`kit::relay::SpecPeer`）：没有允许的手机时启动器马上退出；只让看的打不开；起一个（说明里能挑的账号、挑了的账号，参数、开场白、大小、环境、
  当前目录照模板和手机说的）、马上接进去、退出码；经会话的通道发的 LAUNCH 不理；日志；SIGTERM 重启以后同一个编号；关掉以后启动器自己退出、说明没了。`what_the_launcher_refuses`：
  模板改过了、没有这个模板、接得太勤、请求不对（账号的编号不是那个样子也是）、没有这个账号、文件夹没了；`what_changed_on_the_host`：参数里已经有一句话的不能带开场白（说明里的 `l`、带了是 `bad`）、
  能挑的账号（另一个 Team 给的不在里面；编号按配对算的，两台手机看到的不一样）、模板能归属的 Team（`remote-template teams`、`list --json` 的 `teams`、`team`、`team_name`）、
  归属另一个 Team 的模板（说明里没有它，照它起是 `no_template`）、不说挑哪个而能挑的不止一个（`need_account`）、拿另一台手机的编号（`no_account`）、
  挑的账号后来归另一个 Team 了（`other_hub`：常驻的启动器重读 Team 的配置）、参数后来过不了检查（`bad_args`）、会话已经有 7 个（服务器的级别不到 3 时 `busy`，到 3 照起）、
  关掉一台手机的远程启动以后它收不到说明、要不来票据（另一台照旧）。`denied` 只在配对表改了、转接进程还没看到的那 3 秒里出得来，没有单独测。
  `a_phone_launches_a_session` 的最后：手机结束远程启动的会话（说明里的 `end`；改成只看以后说明里没有它，硬发 `END` 也不转；改回能操作以后发 `END`：`<EXIT 129>`、会话没了）
- `tests/remote_launch.rs` 的 `a_machine_launches_a_session`：两台真的设备配对，盒子上的启动器允许 mac（不是手机）：`remote-list --json` 的 `peer_launchers`（模板的各项、
  账号的名字）、不写模板时列出来、起之前就停的（不在终端里 69、开场白里有控制字符 64、没有这台机器 / 这个模板 65，启动器的日志是空的）、
  能挑的账号（`peer_launchers[].accounts`）、`--account` 写了没有的账号是 65、`--json --size --line --digest --account` 起一个（文档、会话那一项、没有事件；日志里不记开场白）再接进去（大小、参数）、终端里起一个马上接进去（能挑的只有一个：不写 `--account`；退出码）、
  摘要不对 67 和 `fail` 事件、关掉以后启动器退出、`peer_launchers` 空了、再起是 65。单元：`start.rs` 的 `answers_from_the_launcher`、`sizes_and_lines`，
  `far.rs` 的 `launcher_docs_from_the_other_side`
- `tests/remote_host.rs` 的 `headless_sessions`：无头的宿主（参数不对、接入、没人接入时留多久、多久结束）
- `tests/ios_kit.rs`：`against_the_spec` 的 `launch`（启动器的说明解开、清理——能挑的账号也是，旧版的说明没有 `accounts`——，拼 LAUNCH（带 `a`）、读 LAUNCHED，和 `kit::spec` 对）；`a_phone_launches_a_session`：ccaguest
  对着真的启动器列模板和能挑的账号、挑 alice 起会话、`--fresh` 接进去
- 回归：3e（`CCA_REMOTE_HOSTED` 不再套一层）；11b3c 在和手机配好以后（阁楼组的服务器）：模板的增删改（只在阁楼组里：不问、归属它；`--team` 写了这台机器不在的 Team）、`--args-line`、`--json`（给 App 的测试数据 `remote_templates.json`、
  `remote_list_launch.json`）、打开要同意（终端、App；配对过的电脑也问，同意的范围是它的配对编号）、LaunchAgent 的内容和装卸（假的 launchctl 的用户域：照 `ProgramArguments` 真的起后台进程，`bootout` 停掉）、
  改别的配对时不重写 LaunchAgent（keep）、打开时内容变了才换、改成只看 / 取消配对时卸掉；12：`cca restore` 卸掉 LaunchAgent（确认清单里说一句）。
  `-- 开场白` 里提到 `--settings` 不算（3 节第三方 API 账号那几条）。Linux 的 crontab 那一支只在代码里（`tests/linux.sh` 没测）
- App：`macos/Tests/ContractTests/RemoteLaunchTests.swift`（模板的弹层：归属的 Team 先选上默认的、只能换成能选的、存的时候带 `--team`、改旧版本建的没有归属的模板；`remoteLaunchGuestTests`：「新建会话…」的弹层、挑账号、起不成、起好了接入、还没登记好、旧版的那台机器不挑账号；3.7.6 的 cca 没有 `remote_launch_machines`）；iOS：`ios/Tests/KitTests` 的 `launch`、模拟器里的演示模式（`-screen launch`：挑账号的那一节）

## 没验证的

- 真的 LaunchAgent：登录以后起不起来、launchd 给的环境里找不找得到 claude / codex（PATH 是装的时候的）、从 LaunchAgent 起的进程读不读得了钥匙串里配对的密钥
- 真机上起会话；TestFlight 还没传带这一页的构建
- 两台真的电脑之间（真的 Team 服务器、真的 LaunchAgent）用 `cca remote launch`、Mac 上的 App 起会话；App 里「新建会话…」的弹层只在契约测试里走过，没人点过
- 挑账号：真机上的「用哪个账号」、两台真的电脑之间 `--account`；新的 App 对着 3.7.9 及以前的那台机器（不挑账号）只在契约测试、ccaguest 里走过
- 手机上结束会话：真机上点「结束会话」（模拟器里演示模式走过、ccaguest 对着真的 cca 走过）；网页端不给结束（`server/web/core.js` 不认 `end`）
- 归属的 Team：终端里在好几个 Team 里时问归属哪个（回归里这台 Mac 同时只在一个 Team 里）只手动在临时的 HOME 里走过；App 弹层里的「Team」只在契约测试里走过，没人点过
- Linux 上的 crontab 两行、开机以后起来
