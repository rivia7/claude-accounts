# CLAUDE.md

`cca` 按 GitHub Releases 分发，是单个签名文件：bash 3.2 主体 + 路由器端 busybox ash（`router_sh`）和 ruby（`router_rb`），文件最后接着四种机器的辅助程序（`cca-core`，Rust，源码在 `cli/`；bash 里的 `py 子命令 …` 就是运行它。它原来是内嵌的 Python，函数名都没变：文档里说“辅助程序的 `xxx`”，在 `cli/src` 里按名字找）。新功能的 bash 部分写进 `cca`，辅助程序的部分写进 `cli/src`。管三种工具的账号：Claude Code（`cca …`）、Codex（`cca codex …`）和 Antigravity（`cca agy …`）。

这一份只放每次都要守的规矩、要一起改的地方和索引。各块怎么实现、实测过什么、怎么测，按主题写在 `docs/dev/` 下：动到哪一块，先读末尾「细节在哪」里对应的那一份。改了行为就顺手改那一份；新功能写进最贴近的那一份，没有就新建一份、加进索引。`tests/`、`macos/` 下各有一份 CLAUDE.md，读那里的文件时自动加载。

## 命令
- `bash -n cca && bash -n install.sh` — 快速语法检查
- `cd cli && cargo build --release --locked` — 编辅助程序：仓库里的 `cca` 用的是 `cli/target/release/cca-core`，改了 `cli/src` 要重新编才生效（回归开头会编）。`cargo test --locked`、`cargo fmt`（回归里查 `cargo fmt --check`）、`cargo clippy --all-targets -- -D warnings`（Linux 那一半的代码再加 `--target x86_64-unknown-linux-musl` 查一遍）。见 docs/dev/rust.md
- `bash tests/regress.sh` — 全流程回归，约 12 分钟，超过 Bash 工具前台 10 分钟的上限，放到后台跑。只能在 macOS 上跑；全用假的路由器 / claude / curl / ssh / GitHub，不联网、不碰真实 `~/.cca`。`KEEP=1`、`DEBUG_OUT=1`、`TIMING=文件` 和分节续跑（`CKPT` / `FROM` / `ONLY`）见 tests/CLAUDE.md
- `bash tests/linux.sh [--docker [镜像…]]` — Linux 上的测试，要联网；见 tests/CLAUDE.md
- `bash macos/build.sh`、`bash macos/test.sh` — 编界面 App、跑 App 的契约测试，只要苹果的命令行工具、不用 Xcode；见 macos/CLAUDE.md
- `./release.sh "更新说明"` — 发布，约 25 分钟（大半是回归，另有四种机器的辅助程序各编一遍），同样放到后台跑（要装 Rust 和四个目标：先编四种机器的辅助程序，签名前接到 `cca` 后面）；见下面「发布」和 docs/dev/release.md

## 发布
- 正式版（stable）必须 release.sh 全流程、回归过了才发。发布前把 `cca` 里的 `VERSION` 改大，在 `macos/Sources/CcaKit/Model/WhatsNew.swift` 最前面写好这个版本的更新说明（没有就不发布），再提交（工作区要干净）
- `VERSION` 写成 `X.Y.Z-beta.N` 是测试版（GitHub pre-release，自动更新和一键安装都不装）。测试版允许「先发后验」：已经单独跑过一轮完整回归、PASS 之后，用复制到仓库外、去掉回归那一步的 release.sh 先发布，发布后马上在后台把完整回归再跑一遍，没过就撤回。步骤见 docs/dev/release.md
- 改动要发版才会到达已安装的机器（包括维护者自己的 Mac）；`cca` 旁边有 `.git` 时是开发模式，不自动更新

## 每次都要守的

### bash 和辅助程序
- 非 ASCII 字符前的变量一律写 `${VAR}`：macOS bash 3.2 在 UTF-8 下会把多字节字符的首字节吞进变量名（回归里有 lint，也查 App 在 Swift 里拼的 bash，比如终端交接脚本；C locale 下看不出来，测这类脚本要带 `LANG=en_US.UTF-8`）
- 交互输入从 fd 3 读（终端或 `CCA_TTY` 答案文件），提示写 stderr；stdout 只输出数据，很多函数靠 `$(...)` 取返回值
- macOS 的 bash 3.2 里 `read -t` 超时的退出码是 1，和读到头一样（bash 4 起大于 128）：要分开超时和读到头时看等了多久（`gui_answer_wait`：读到头是马上返回的）。回归跑在 bash 3.2 上，答案文件只测得到读到头，超时要用一直开着、没有内容的管道当 `CCA_TTY`
- bash 3.2 另外两处：`$(…)` 里解析不了 `case` 的右括号，要放进函数；函数调用前的赋值只在这次调用里有效（`FAIL_CODE=代码 die …` 就靠它）
- 每次 `py` 都是一个新进程（辅助程序，一次约 4 毫秒、4 MB；一条命令起十几二十次，回归里几千次），别让它变慢：启动的路上不加东西——系统框架用到时才 `dlopen`（写成 `#[link]` 的话每条子命令都要先载入它），正则用 `re!`（用到才编译），只有一条子命令用的东西别在 `main` 里做。bash 里能少起就少起：同一条命令里反复要的记下来（`load_router` 按 config.json 的 inode、大小、修改时间认没变；`stat` 在 macOS 上是 `-f`，GNU / busybox 上是 `-c`，GNU 的 `-f` 是文件系统的信息、缓存永远不失效），一起要的几样合成一个子命令（`sync-state`：校验账号表、清掉多余的 key、摘要）
- 辅助程序在哪（`helper_find`）：发布的 `cca` 第一次运行时把接在文件最后的那一份解到缓存目录的 `cca-helper/<SHA-256>`（校验过才换上），以后每次只看文件在不在，不为它起进程（中途被清掉了 `py` 自己再解）；仓库里的 `cca`（`HELPER_SUMS` 空着）用 `cli/target/release/cca-core`，或者 `CCA_HELPER` 指的（发布的不认这个变量）。`cca` 的最后一行是 `exit`：它后面的不是脚本，别往后面加东西。见 docs/dev/rust.md
- bash 和辅助程序之间的约定（子命令的参数、stdout 的格式、退出码）两边一起改；辅助程序里起外部命令照旧按名字起（回归把它们换成 PATH 上的假命令、核对参数）；辅助程序的依赖是白名单，加新的之前先问维护者
- 替换已安装的脚本只能用 `mv`：bash 边读边执行，原地覆盖会让正在运行的 cca 读到半截代码
- 联网下载一律用 `retry3`（有些网络会丢新连接；两次之间等 `CCA_RETRY_WAIT` 秒，默认 2，回归里是 0）
- Linux 上：别用 `cmp`、`diff`（最小安装的发行版没有 diffutils，`! cmp` 恒为真），比文件用 `sha256_of`；要账号专属的 Chrome、桌面 App、界面 App 的命令开头 `mac_only`。见 docs/dev/linux.md
- 仓库里没有 Python 的程序，cca、辅助程序、服务端、测试也都不靠机器上的 python3：要算的写进辅助程序（`cli/src`），测试用的写进 `tests/`（Rust 的 `kit`、`cca-test`、`suite`）或者 sh。回归守着两道：`cca-test lint nopython` 查文件（`.py` 这类、第一行是 python 的脚本），沙箱 PATH 最前面那个记账的假 python3 记下谁调了它（见 tests/CLAUDE.md）

### 安全和隔离
- 密钥绝不进任何进程的命令行和环境变量：存钥匙串、交给 Claude Code / Codex 的做法见 docs/dev/secrets.md
- `child_env` 一律不带继承来的 `ANTHROPIC_*`、`CLAUDE_CODE_OAUTH_TOKEN`、云厂商 / 网关的开关（`INHERITED_RE`）：shell 里的 `ANTHROPIC_BASE_URL` 会把官方账号的登录 token 发到别的地址。Codex、agy 各有自己的名单。见 docs/dev/isolation.md
- 历史、记忆、配置和跨账号续接是硬性要求：共享的由 `CLAUDE_SHARED` / `CODEX_SHARED` / `AGY_SHARED` 明确列出，未知的新状态默认独立；不要用隔离这些数据来实现设备隔离。设备标识（账号目录的 `device.json`）的 `prepare_device` 必须在 `Lock` 内调用，重新登录、改别名不重置。见 docs/dev/isolation.md
- 切节点（路由器、本机内核）必须 GET 回读核实，失败不能以端口开放放行；探针、控制接口用的 curl 第一个参数是 `-q`，走代理时 `--noproxy ''`，直连时 `--noproxy '*'`
- 泄露检测、隔离检查失败或没有结果，不能当作没问题；Chrome 没装、旧版 Codex 不支持独立登录时停下，不能退回默认浏览器
- Claude App 多开只认 `--user-data-dir`，同一个资料目录绝不能开两份。见 docs/dev/desktop.md
- `~/.cca` 里新加了凭证类的文件：补进 `CCA_DIR_PRIVATE`（cca 启动的 Claude Code 照它挡住读写），`cca restore` 的备份也别打包它（bsdtar 的 `--exclude` 不锚定开头，写具体文件名）
- 仓库是公开的：代码和测试里不放真实节点名、路由器 IP、邮箱；测试用 `US-A`…`US-D`、`HK-A`、`192.0.2.10`、`example.com`

### 路由器
- 会让 OpenClash 重载的操作必须先征得同意（路由器端返回 3 → `allow_reload`），cron 永远不重载；重载会让局域网新连接中断约 20 秒
- 改了 `router_sh`/`router_rb` 会改变同步摘要，已安装的机器下次启动会自动重新部署路由器端
- 只有官方账号的账号表，`router_accounts` 的输出必须和以前逐字节一样：否则所有已安装的机器升级后都会重新部署路由器端
- 路由器端只用 busybox 一定有的命令（OpenWrt 默认的 busybox 没有 `cksum`）。见 docs/dev/router.md

### 账号表和工具
- 账号身份按邮箱（`acct_id`）；账号表的键是 `别名(邮箱)`，改别名不能影响登录目录和端口
- 账号表的值是字符串（官方账号，值是节点）或对象（第三方 API 账号）：读值一律用 `is_api` / `node_of`；要端口用 `routed(c, t, tool)`，跨工具用 `pool(c)`；各工具的端口、上次使用这些经 `T.state(c)`，不要直接碰 `c['ports']`。见 docs/dev/accounts.md
- 依赖账号先后的逻辑（端口池、路由器的摘要、本机的端口池、启动器和头像的颜色）用 `base_order`，不要直接遍历账号表：不然调整一次顺序就要同步路由器、账号换颜色。见 docs/dev/accounts.md
- 工具差异集中在辅助程序的 `Tool`（`cli/src/core/tool.rs`：claude、codex、agy 三份，按 `CCA_TOOL` 选出当前的 `t()`，原来的 `T`）和 bash 的 `TOOL`（`tool xxx` 调成对的 `claude_xxx` / `codex_xxx`）；`TOOLS` 的顺序 claude、codex、agy 不能变（端口池按它分）。见 docs/dev/accounts.md

### 界面 App 的接口
- App 只经 `macos/PROTOCOL.md` 写的接口和 cca 打交道。改了 `_gui hello|state|live`、各个 `--json` 输出、`health.json`、事件，要同时改 PROTOCOL.md、Swift 模型（`macos/Sources/CcaKit/Protocol`）和 regress 的 10g；只能加字段，删改要升 `GUI_API`
- `cca _gui state|live` 在 `CCA_RO=1` 下跑：只在内存里对账，不写文件、不往 stderr 说。辅助程序里新加会写文件、会说话的对账逻辑也要照这个规矩
- 要用户同意的地方一律用 `consent 项 范围 …`（授权带范围、原样匹配、不保存）；问题只能经提问的基元（`ask`、`confirm`、`ask_secret`、`ask_fields`、`pick_node`……），别处不许读 fd 3；有 `q=` 的题号要写进 PROTOCOL 4.6 的映射表（回归 lint）
- 事件、失败原因（`fail_warn`、`FAIL_CODE`）、提问会话、`_gui live` 的细节见 docs/dev/gui-contract.md

## 要一起改的
- `INHERITED_RE`（bash）↔ `CRED_ENV`（辅助程序，`cli/src/core/base.rs`）：同一份名单的两种写法。Claude Code 新增 provider 开关时补上（`strings` 二进制里的 `CLAUDE_CODE_USE_` 能查到）；`CRED_ENV` 加了名字时看看 `SETTINGS_KINDS` 要不要补
- `CODEX_INHERITED_RE` ↔ `codex_settings_problems`：Codex 新增会改地址、换凭证的设置或环境变量时两边一起补
- 让 agy 把登录存成文件的环境变量：bash 的 `agy_env` ↔ 辅助程序的 `AGY_FILE_ENV`（`cli/src/core/tool.rs`，探测和查额度用它）↔ `login/agy.rs` 的 `AGY_FILE_TOKENS`（探测时认的日志那几句）；探测的做法变了要换缓存的文件名 `PROBE_FILE`：旧版的 cca 还读原来那一份、只核对程序文件，新的结果写进去它会以为可以启动
- `install.sh` 下载前查的 ↔ `cca install` 的 `need_deps`；两份 `pkg_hint`
- 辅助程序的 `fill_plan` ↔ `router_rb` 的 `--fill-source`（Team 的配置带的节点（参考节点）的同一套规则）
- `VIRTUAL_IFACE_RE`：bash ↔ 辅助程序（`cli/src/cmd/corenet/bind.rs`；回归 lint 核对）
- 版本的先后：bash 的 `ver_gt` ↔ 辅助程序的 `ver_key` ↔ Swift 的 `AppVersion`
- `CORE_VERSION` 和四个 `CORE_SHA256_*` 一起换（macOS 的两行后面别加注释：mkver 的 sed 按整行换）
- `RELEASE_KEY`：`cca` ↔ `install.sh` ↔ `~/.ssh/cca_release_ed25519.pub`（release.sh 会检查）
- 辅助程序的四个目标：`cca` 的 `helper_find` ↔ `cca-core pack`（`cli/src/cmd/pack.rs`）的 `TARGETS` ↔ `release.sh` 的 `TARGETS` ↔ `install.sh` 查的机器种类；`cca` 里以 `HELPER_SUMS=""` 开头的那一行由 `cca-core pack` 填上；它接在后面的那几段的格式 ↔ `cca` 的 `helper_unpack`（`sed` 切出来、`base64` 解码、`gunzip`）↔ `cli/src/cmd/hub/payload.rs`（同样读那几段，取出 Linux 的两份给 Team 的服务器；它的 `LINUX` 要在 `TARGETS` 里）
- bash 里 `py 子命令` 的参数和输出 ↔ `cli/src/cmd/` 里那个子命令（`CMDS` 表）
- 辅助程序里的常量 ↔ tests/ 下把它当黑盒测的那几份里写死的同一个值：`UDP_HELPER`（`cli/src/cmd/corenet/udp.rs`）↔ `tests/local_udp.rs`，`LEAK_FAIL_TTL`（`cli/src/cmd/leak.rs`）↔ `tests/isolation_leaks.rs`
- App 的接口：PROTOCOL.md ↔ Swift 模型 ↔ regress 的 10g；`router_sh` 的 `status` 输出 ↔ `cmd_gui_live` 的 `GUI_SLOT_RE`（regress 守着）
- Claude App 的会话记下账号（`cca cost hook`）：写进用户共享的 `~/.claude/settings.json` 的命令（`cost.rs` 的 `desktop_hook_command`、`DESKTOP_HOOK_MARK`）↔ bash 的 `_session-start` 分支（`--desktop` 原样交给 `py session-start`）↔ `cmd_session_start` 的 `--desktop`：已经加上的 hook 留在各台设备的设置里，改了这几处也要一直认原来的命令（`off`、卸载照 `DESKTOP_HOOK_MARK` 认出它）；同意项 `cost_hook`、结果 `cost_hook`、`unattributed.app_hook`：`cca` ↔ PROTOCOL（2.2、3.5、4.1、4.2）↔ Swift 的 `ConsentItem`、`ResultCode`、`CostAppHook` ↔ `macos/Tests/Fixtures/state.json`
- `VERSION` ↔ `WhatsNew.swift` 最前面的 `ReleaseNote`（`macos/test.sh` 查用词和版本先后）
- 部署到托管平台（`cca team hosted`）：服务端 `CCA_HUB_HOSTED=1` 的预设（`cli/src/server/cli.rs`：端口 8080、`/data/state.json`）↔ `cli/src/cmd/hub/hosted.rs` 的 `team_files` 生成的 Dockerfile（`FROM scratch`，里面只有一个 `cca-core`：`payload.rs` 给的 x86_64 那一份）、步骤、README ↔ server/README.md「托管平台」
- Team 的同步服务器的一键部署（`cca team deploy`）：`server/hub-install.sh` ↔ `cli/src/cmd/hub/deploy.rs`：前面放进的几项（`CCA_HUB_MANAGED`、`CCA_HUB_HOST_HINT`、`set -- …`，里面的 `--name` 照 `shq` 转义）、结果行（`report_managed` 的 `CCA-HUB 键 值` ↔ `report`）、原因码（`die` 的 ↔ `fail_code` ↔ bash `team_fail` 给的 `team_…` ↔ PROTOCOL 4.14）、`--dir` 的规矩（`dir_check` ↔ `dir_of`，共用的证书助手的目录 `AD` ↔ `ACME_DIR`）、网页端没开的原因（`WEB_WHY` ↔ `report` 的 `web_off` ↔ bash `team_deploy` 的 `case`：`port80`、`acme_old`、`lan`、`host`、`web_port`）；安装脚本带着的服务端程序（两种机器的 `cca-core`，gzip + base64，各在一个 here-document 里）：`deploy.rs` 的 `installer` 写的 `CCA_HUB_VERSION='…'`、`CCA_HUB_CORES='…'`、`CCA_HUB_CORE_SHA256_<机器名>='…'` 这几行和函数 `hub_core_<机器名>`（结束行 `CCA_HUB_CORE_<机器名>_EOF`）↔ `hub-install.sh` 的 `core_pick` / `core_put` ↔ release.sh 的 `core_ok` ↔ regress 11b3d 的 `hbcore`（照它们取出来核对）；机器名：`cli/src/cmd/hub/payload.rs` 的 `LINUX` ↔ `cca-core pack` 的 `TARGETS` ↔ `hub-install.sh` 的 `core_arch`（照 `uname -m` 认）；Docker 方式的容器用的空镜像：`hub-install.sh` 的 `RIMG`（`cca-team-runtime:1`，`rimg` 用 `docker import` 在本机建，不拉镜像）↔ `tests/kit/hubbox.rs` 的 `RUNTIME`（假的 docker 照它查 compose.yaml）；网页端的文件编在服务端程序里（`cli/src/server/web.rs` 的 `FILES` ↔ `server/web/` 里服务端给出去的文件），安装脚本不带；以前的版本（3.6.x，服务端是 Python 的）装的要能就地换过来、它的安装脚本也要能在这一版装的上面再运行：`hub-install.sh` 里清掉以前的文件的那一行、`.env` 里不再写的 `CCA_HUB_IMAGE` ↔ `tests/hub_deploy.rs` 的 `make_python_era`；证书助手（`cca-core team-acme`）的参数（`--teams`、`--store`、`--out`、`--account`、`--listen`，Docker 方式另有 `--ca /app/ca.pem`：容器里没有 CA 证书，`acme_start` 把服务器上的那一份拷进去；CA 证书在哪：`hub-install.sh` 的 `ca_bundle` ↔ `cli/src/server/acme/roots.rs` 的 `SYSTEM`，它找的那几个都要在 `ca_bundle` 里、先后一样；它放进 `out/<服务名>/` 的两个文件名）↔ `hub-install.sh` 起它的那几处（几个 Team 共用的 `cca-team-acme`：`acme_compose`、`acme_unit_file`，目录 `/opt/cca-team-acme`、`teams/<服务名>` 的登记）↔ `tests/kit/hubbox.rs` 的 `acme_stub`、`stage`（假服务器只起 `app/cca-core team-server`，`team-acme` 换成假的证书助手）；Team 的名字：`hub-install.sh` 写的 `app/name` ↔ 服务端的 `--web-name-file`（`web_name`、`web_named`：标题、`apple-mobile-web-app-title`、manifest 的 `name` / `short_name`）↔ `server/web/index.html`、`manifest.webmanifest` 里被换掉的那几处；App 里部署前查登录的原因码（`ssh_trouble`：`ssh`、`ssh_auth`、`ssh_denied`、`ssh_keyonly`、`ssh_hostkey`）↔ bash `hub_deploy_ask` 的 `case`；安装脚本的模板是 `include_str!` 编进辅助程序的，改了要重新编
- Team 的线上格式（`team_*` 的请求、邀请链接、从邀请里那串随机数派生的令牌 / 设备编号 / 密钥 / 公钥的认证码、每台设备的配置的密文和签名）：`server/README.md`「Team 的同步服务器」↔ 服务端（`cca-core team-server`，`cli/src/server/`：请求的检查在 `wire.rs`、存的在 `store/team.rs`）↔ `cli/src/cmd/team/doc.rs` ↔ `tests/kit/spec.rs`（照文档独立实现的，`cca-test spec` 在回归里用它；`doc.rs` 测试里的对照数据、`cli/tests/vectors/team_doc_ct.json` 和它自己的 `tests/vectors/spec_wire.json` 都是固定的数据，没有重新生成它们的命令：改了格式两边的代码和这几处数据一起换，`tests/kit/spec/vectors.rs` 的 `the_helpers_team_doc_vector` 看着两边封出来的密文一样）；服务器的上限（`cli/src/server/mod.rs` 开头的常量）↔ 辅助程序里写死的同一个值（`MAX_TEAM_DOC` ↔ `doc.rs` 的 `DOC_MAX`，`SEQ_MAX`，`MAX_TEAM_BATCH` ↔ `admin.rs` 的 `BATCH`，`MAX_ACCOUNTS` ↔ `cost.rs` 的 `COST_BATCH`）；配置明文的样子（`doc_problems`）↔ docs/dev/team.md「设备拿到的配置」
- Team 的同步服务器的状态文件名（`HubKind::file` 加 `team.<编号>.`）↔ `cli/src/cmd/team.rs` 的 `drop_synced`（退出、被移除时删掉的前缀）↔ App 的 `CcaWatcher` 盯的文件（`teamFile`：`presence/team.<编号>.status.json`、`cost/team.<编号>.remote.json`、`sync.json`）↔ PROTOCOL 1 的那份清单 ↔ `cca restore` 的 `--exclude`（`cost/team.*.remote.json`）
- Team 的一键部署：`server/hub-install.sh` 的 Team 模式（`CCA_HUB_TEAM_ROOT`、`--team-root`、服务名 `cca-team-<编号的前 8 位>`、`.env` 的 `CCA_TEAM_ROOT`）↔ `cli/src/cmd/hub/deploy.rs` 的 `team_install`（前面放进的几项、目录默认 `/opt/<服务名>`）↔ `cmd/team/admin.rs` 的 `cmd_team_deploy`（服务名）↔ 服务端（`cli/src/server/cli.rs`：`CCA_TEAM_ROOT` 两种写法、托管的预设）↔ `cmd/hub/hosted.rs` 的 `team_files` ↔ `tests/kit/hubbox.rs`（按服务名分的 unit、进程号文件：`systemctl_run`、`pid_file`）；bash 里部署用的那几段（`DEPLOY_PY` 等几个变量，`hub_deploy_ask`、`hub_deploy_run` 这些函数：名字里的 hub 是 3.5.2 部署自己的同步服务器时起的）↔ `team_deploy`
- Team 在界面里：PROTOCOL 2.1 `hello.team`、2.2 `config.teams` / 账号的 `team`、4.14 ↔ `cli/src/cmd/team.rs` 的 `team_rows`、`admin.rs` 的 `cmd_team_devices`（`--json`）↔ Swift 的 `Protocol/Team.swift`（`TeamAPI.version` ↔ bash `gui_hello` 的 `"team":1`）↔ `macos/Tests/Fixtures/team_devices.json`、`state.json` ↔ regress 11b3d 的 `$T/guifx4`
- Team 的命令：bash 的 `cmd_team`（`py team-*` 的参数、一行一项的 stdout、原因码）↔ `cli/src/cmd/team.rs`、`team/admin.rs`；同意项 `team_leave`、`team_delete`、`team_revoke`、题号 `team.join`、结果和失败的代码：`cca` ↔ PROTOCOL 4.14（4.2、4.6）↔ Swift 的 `ConsentItem`、`ResultCode`；`~/.cca/teams/` 是凭证：`CCA_DIR_PRIVATE` 的 `teams/**` ↔ `cca restore` 的 `--exclude`（`teams/*/secret`、`root_ed25519`、`admin.json`、`doc.json`）和 `restore_teams`
- Team 的节点（`cca team node`）：`ls --json`（`kind: team_nodes`）、`test --json`（`kind: team_node_test`）和退出码 0 / 1 / 2 / 3（`cmd/team/admin/nodes.rs`）↔ 维护者本机的 `cca-external-account` skill（不在仓库里：改了告诉维护者）；实测的临时内核的入站名 `cca-test-N` ↔ `tests/setup.sh` 的假 mihomo（认 `"cca-test-1"`）
- 远程会话的中转（线上的格式）：`server/README.md`「远程会话的中转」↔ 服务端（`cli/src/server/remote.rs`、`relay.rs`）↔ `cli/src/cmd/remote/crypto.rs`、`relay.rs` ↔ `tests/kit/spec.rs`（照文档独立实现的；`crypto.rs` 测试里的向量和它自己的对照数据 `tests/vectors/spec_wire.json` 都是固定的，没有重新生成它们的命令：改了格式两边的代码和数据一起换）；同意项 `remote_pair`：`cca` ↔ PROTOCOL 4.2 ↔ Swift 的 `ConsentItem`
- 远程会话：`run_account` 的 `HOSTCMD`（`remote_prepare`）↔ 辅助程序 `remote-host` 的参数；`local.rs` 新加的接入方 → 宿主的帧 ↔ `link.rs` 的 `bridge` 放不放行（只该这台机器上做的事不放行，像 `END`）↔ `tests/remote_relay.rs` 里发这一种帧的那一句；同意项 `remote_end`：`cca` ↔ PROTOCOL 4.2 ↔ Swift 的 `ConsentItem`；客户端新加的终端查询序列 ↔ `cli/src/cmd/remote/vt.rs` 的 `csi_class` / `osc_class` / `str_class`（漏了的话几个终端同时接着时回答会变成按键）；`cca remote attach` 的退出码、断开的按键 ↔ docs/dev/remote.md ↔ PROTOCOL 9.3 ↔ App 的 `Texts.remoteEnded`；`remote-list --json`（`manage.rs` 的 `list_doc`）↔ PROTOCOL 9.1 ↔ Swift 的 `Protocol/Remote.swift` ↔ `macos/Tests/Fixtures/remote_list.json`
- SwiftTerm（`macos/Vendor/SwiftTerm/`）：换版本时源码、`SHA256SUMS`、README 里的版本和提交号一起换；App 里只有 `TerminalSurface.swift` 用它；不要的功能（图形协议）：`macos/build.sh` 换掉的那两行（`ST_KITTY`、`ST_SIXEL`）↔ `macos/tools/swiftterm-check` ↔ `TerminalSurface.swift` ↔ PROTOCOL 9.3
- 「远程会话」页多久查一次（`Scheduler.remoteInterval`）↔ PROTOCOL 9.1 ↔ docs/dev/remote.md ↔ macos/README.md（服务器给每台设备限了流：`cli/src/server/limits.rs`）
- 远程会话的网页端（手机在浏览器里接入，线上的格式）：`server/README.md`「网页端」↔ 服务端（`cli/src/server/` 的 `GUEST_OPS`、`remote.rs` 的 `guest_key`、`ws.rs` 的 `pump`、`web.rs` 的 `Web`）↔ `server/web/core.js` ↔ `tests/web_relay.rs` 的 `Guest`、`WsSock`（照文档独立写的）；配对、握手、记录、帧是三份实现：`cli/src/cmd/remote/crypto.rs` ↔ `tests/kit/spec.rs` ↔ `server/web/core.js`（改了格式三处一起改，`tests/web_relay.rs` 的 `the_same_numbers`、`handshake_and_records`、`signature` 拿 `core.js` 和 `kit::spec` 对）；断了重新接上的间隔：`core.js` 的 `RETRY_GAP`、`RETRY_FOR`、`MISSING_FOR`、`FLAPS` ↔ `attach.rs` 的同名常量；辅助程序 `remote-pair-show --web` 的输出（`code` 后面紧跟一行 `web 地址`）↔ bash 的 `remote_pair_run`
- 和手机配对在界面里：同意项 `remote_pair` 的范围 `web`、题号 `remote.pair_web`、字段的种类 `qr`、hello 的 `remote_web`、`peers[].web`：`cca` ↔ PROTOCOL（2.1、4.2、4.6、9.1、9.2）↔ Swift（`PromptKind.qr`、`Handoff.remotePairWeb`、`HelloDoc.remoteWeb`、`RemotePeer.web`）↔ `macos/Tests/Fixtures`（`flow_remote_pair_--web.flow`、`remote_list.json`、`hello.json`）
- 网页端的页面（`server/web/`）：xterm.js 换版本时那四个文件和 `server/web/README.md` 里的版本、校验和一起换；目录里新加文件 ↔ `tests/web_relay.rs` 的 `NEED`（`the_real_page` 照它一个个去要）↔ 服务器的 `WEB_TYPES`；`macos/Sources/CcaApp/LogoGlyph.swift` ↔ `server/web/logo.svg`（照它手写的同一个字形）、`icon-*.png`（`macos/tools/make-icon --flat` 画的）；页面里的类名别叫 `terminal`（xterm.js 给自己的根元素用了）
- 每个 Team 各用各的中转服务器：`cli/src/cmd/remote/relay.rs` 的 `session_hub`（会话走哪台）↔ `cmd/hub.rs` 的 `route` / `core/policy.rs` 的 `policy_sources`（账号是谁给的）；配对表的 `hub`（`peers.rs`：Team 的编号；没有这一项的是 3.6.0 以前经维护者的服务器配的，读的时候丢掉）↔ `remote list --json` 的 `peers[].team`、`relay.hubs` ↔ PROTOCOL 9.1 ↔ Swift 的 `RemoteHub`、`RemotePeer.team` ↔ `macos/Tests/Fixtures/remote_list.json`；bash 的 `--team 名字`（`remote_hub_args`）↔ 辅助程序的 `--hub`（`pair.rs` 的 `hub_named`）↔ App 的 `RemoteHub.arguments`；Team 服务器的数据端口：`server/hub-install.sh` 的 `pick_relay`、`.env` 的 `CCA_HUB_RELAY`、`compose_yaml` 里带 `@RELAY@` 的那几处 ↔ `tests/kit/hubbox.rs` 假的 docker 解释它的那几行（`compose_up`）↔ `deploy.rs` 的 `report`（`relay`）↔ bash `team_deploy` 说要放行的端口
- 证书助手（`cca-core team-acme`，`cli/src/server/acme/`）↔ `tests/kit/acmefake.rs`（照 RFC 8555 / 8738 和证书种类的草案核对它的请求；改了流程两边一起看）；它写的文件名（`fullchain.pem`、`privkey.pem`）↔ 服务端的 `--web-cert` / `--web-key`；Let's Encrypt 的规矩（短期证书那一种的名字 `shortlived`、续的时机、限额）变了要改 `acme/mod.rs` 开头的说明和 `RETRY`、`args.rs` 的 `fraction`

## 细节在哪
动到哪一块，先读 `docs/dev/` 下对应的那一份（这一块怎么测也写在里面）：
- `rust.md` — 辅助程序（`cli/`）：cca 怎么找到它、怎么编和测、要守的规矩、依赖
- `gui-contract.md` — 给界面 App 的接口：`_gui hello|state|live`、事件、`consent`、提问会话和题号、`CCA_RO`
- `onboarding.md` — 配置向导（`cca _onboard`、`cca install --gui`）、配置路由器和账号表的表单、不先配置路由器就加账号
- `accounts.md` — 账号表的值、`base_order`、三种工具的差异：`Tool` 的子类、`cxa` / `aga`、CODEX_HOME、Codex 的 app-server、Antigravity
- `isolation.md` — 设备标识、共享的历史 / 记忆 / 配置、`child_env`、`--add-dir` 和 `--disallowedTools`、共享设置的说明和修复（`cca settings`）、启动前后的隔离检查
- `secrets.md` — 钥匙串和密钥文件、第三方 API 的 key、官方账号记下备查的 key
- `usage.md` — 查用量和用量表格、5 小时 / 按模型 / 每周的额度、周结算提醒、Codex 重置额度、登录失效、套餐档位、命令行会话登记、API 额度的入口
- `cost.md` — 折合金额的统计和同步
- `presence.md` — 设备统计（经各个 Team 的服务器）、共享记下的 API key
- `team.md` — Team：3.6.0 起唯一的通道（维护者的映射、维护者的服务器、自己的同步服务器先后并进 Team、去掉了；管理员的设备维护账号 → 节点和节点的定义、给每台设备发邀请，自己的几台设备之间同步也用它）；Team 给的账号怎么并进账号表（固定节点、别名、收起来）；分阶段的计划和已经定下来的、信任怎么来、服务器的 `--mode team`、账号走哪台服务器（`route`）、本机的状态（`~/.cca/teams/`）和 `cca team` 的命令（成员：加入、同步配置、退出；管理员：建、接上服务器、邀请、授权、导入这台设备的账号、节点、发布、移除）；部署（一键部署 `cca team deploy`、安装脚本 `server/hub-install.sh`、假服务器 `tests/kit/hubbox.rs`、App 里部署（问地址和密码、`askpass_run`）、托管平台 `cca team hosted`）；3.6.0 去掉了什么、留着什么（`policy_leftovers`、`cca policy` 只说一声、`GUI_API` 不升）、从维护者的映射和自己的同步服务器迁过来（`hub_leftovers`）；做到哪一步了看它的阶段表
- `fill.md` — 参考节点：Team 的配置带的节点，写进路由器的配置文件
- `router.md` — 路由器端：OpenClash / ShellCrash、几台设备共用一个路由器、节点改名
- `local-proxy.md` — 内置代理：mihomo 内核、拷节点、自定义节点（`cca node`）、本机 UDP 走节点
- `network.md` — 在家还是在外、出口检测的结果、DNS / WebRTC 泄露检测
- `linux.md` — Linux 上安装、服务器上登录、服务器上不用路由器
- `clients.md` — 辅助安装 claude / codex / agy、客户端统一更新
- `release.md` — release.sh 的步骤、测试版先发后验、自动更新、换了版本以后马上同步 Team 的配置（`update_after`）、检查更新、`cca gui`
- `mail.md` — 邮箱助手（`cca mail`）
- `remote.md` — 远程会话：会话宿主（`cca remote on|off|list|attach`、`CCA_REMOTE`）、终端关了会话照样运行、接回去；两台机器配对（`cca remote pair|join|peers|allow|unpair`）、经 Team 的服务器中转、端到端加密、宿主那头的转接进程；界面 App 的「远程会话」页和内嵌的终端（伪终端、SwiftTerm）；分阶段的计划和已经定下来的也在里面；网页端（手机不装 cca，在浏览器里接入：访客设备、WebSocket 进中转、`server/web/` 的页面、`cca remote pair --web`、带证书的端口和部署；`cca-test web-demo` 在本机跑起来看，命令见 tests/CLAUDE.md）；每个 Team 的会话走这个 Team 自己的服务器中转（配对记在哪台服务器上、`cca remote pair --team`）；给网页端要 IP 证书的证书助手（`cca-core team-acme`）
- `desktop.md` — 程序坞整理、Claude App 多开、启动器图标
- `uninstall.md` — 卸载（`cca uninstall` / `cca restore`）：先后、`~/.cca` 以外要清掉的、不删的、测试里 `defaults` 和缓存目录的坑
