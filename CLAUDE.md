# CLAUDE.md

`cca` 按 GitHub Releases 分发，是单个签名文件：bash 3.2 主体 + 路由器端 busybox ash（`router_sh`）和 ruby（`router_rb`），文件最后接着四种机器的辅助程序（`cca-core`，Rust，源码在 `cli/`；bash 里的 `py 子命令 …` 就是运行它。它原来是内嵌的 Python，函数名都没变：文档里说“辅助程序的 `xxx`”，在 `cli/src` 里按名字找）。新功能的 bash 部分写进 `cca`，辅助程序的部分写进 `cli/src`。管三种工具的账号：Claude Code（`cca …`）、Codex（`cca codex …`）和 Antigravity（`cca agy …`）。

这一份只放每次都要守的规矩、要一起改的地方和索引。各块怎么实现、实测过什么、怎么测，按主题写在 `docs/dev/` 下：动到哪一块，先读末尾「细节在哪」里对应的那一份。改了行为就顺手改那一份；新功能写进最贴近的那一份，没有就新建一份、加进索引。`tests/`、`macos/` 下各有一份 CLAUDE.md，读那里的文件时自动加载。

## 命令
- `bash -n cca && bash -n install.sh` — 快速语法检查
- `cd cli && cargo build --release --locked` — 编辅助程序：仓库里的 `cca` 用的是 `cli/target/release/cca-core`，改了 `cli/src` 要重新编才生效（回归开头会编）。`cargo test --locked`、`cargo clippy --all-targets -- -D warnings`（Linux 那一半的代码再加 `--target x86_64-unknown-linux-musl` 查一遍）；对拍 `python3 -I cli/pydiff/run.py`。见 docs/dev/rust.md
- `bash tests/regress.sh` — 全流程回归，约 12 分钟，超过 Bash 工具前台 10 分钟的上限，放到后台跑。只能在 macOS 上跑；全用假的路由器 / claude / curl / ssh / GitHub，不联网、不碰真实 `~/.cca`。`KEEP=1`、`DEBUG_OUT=1`、`TIMING=文件` 和分节续跑（`CKPT` / `FROM` / `ONLY`）见 tests/CLAUDE.md
- `bash tests/linux.sh [--docker [镜像…]]` — Linux 上的测试，要联网；见 tests/CLAUDE.md
- `bash macos/build.sh`、`bash macos/test.sh` — 编界面 App、跑 App 的契约测试，只要苹果的命令行工具、不用 Xcode；见 macos/CLAUDE.md
- `./release.sh "更新说明"` — 发布，约 25 分钟（大半是回归，另有四种机器的辅助程序各编一遍），同样放到后台跑（要装 Rust 和四个目标：先编四种机器的辅助程序，签名前接到 `cca` 后面）；见下面「发布」和 docs/dev/release.md
- `./policy.sh …` — 维护者发布账号 → 节点映射和参考节点、管理各台机器的 deploy key、部署设备统计的服务器；见 docs/dev/policy.md、docs/dev/presence.md

## 发布
- 正式版（stable）必须 release.sh 全流程、回归过了才发。发布前把 `cca` 里的 `VERSION` 改大，在 `macos/Sources/CcaKit/Model/WhatsNew.swift` 最前面写好这个版本的更新说明（没有就不发布），再提交（工作区要干净）
- `VERSION` 写成 `X.Y.Z-beta.N` 是测试版（GitHub pre-release，自动更新和一键安装都不装）。测试版允许「先发后验」：已经单独跑过一轮完整回归、PASS 之后，用复制到仓库外、去掉回归那一步的 release.sh 先发布，发布后马上在后台把完整回归再跑一遍，没过就撤回。步骤见 docs/dev/release.md
- 改动要发版才会到达已安装的机器（包括维护者自己的 Mac）；`cca` 旁边有 `.git` 时是开发模式，不自动更新

## 每次都要守的

### bash 和辅助程序
- 非 ASCII 字符前的变量一律写 `${VAR}`：macOS bash 3.2 在 UTF-8 下会把多字节字符的首字节吞进变量名（回归里有 lint，也查 App 在 Swift 里拼的 bash，比如终端交接脚本；C locale 下看不出来，测这类脚本要带 `LANG=en_US.UTF-8`）
- 交互输入从 fd 3 读（终端或 `CCA_TTY` 答案文件），提示写 stderr；stdout 只输出数据，很多函数靠 `$(...)` 取返回值
- macOS 的 bash 3.2 里 `read -t` 超时的退出码是 1，和读到头一样（bash 4 起大于 128）：要分开超时和读到头时看等了多久（`policy_wait`：读到头是马上返回的）。回归跑在 bash 3.2 上，答案文件只测得到读到头，超时要用一直开着、没有内容的管道当 `CCA_TTY`（11b6）
- bash 3.2 另外两处：`$(…)` 里解析不了 `case` 的右括号，要放进函数；函数调用前的赋值只在这次调用里有效（`FAIL_CODE=代码 die …` 就靠它）
- 每次 `py` 都是一个新进程（辅助程序，一次约 4 毫秒、不到 4 MB；一条命令起十几二十次，回归里几千次），别让它变慢：启动的路上不加东西——系统框架用到时才 `dlopen`（写成 `#[link]` 的话每条子命令都要先载入它），正则用 `re!`（用到才编译），只有一条子命令用的东西别在 `main` 里做。bash 里能少起就少起：同一条命令里反复要的记下来（`load_router` 按 config.json 的 inode、大小、修改时间认没变；`stat` 在 macOS 上是 `-f`，GNU / busybox 上是 `-c`，GNU 的 `-f` 是文件系统的信息、缓存永远不失效），一起要的几样合成一个子命令（`sync-state`：校验账号表、清掉多余的 key、摘要）
- 辅助程序在哪（`helper_find`）：发布的 `cca` 第一次运行时把接在文件最后的那一份解到缓存目录的 `cca-helper/<SHA-256>`（校验过才换上），以后每次只看文件在不在，不为它起进程（中途被清掉了 `py` 自己再解）；仓库里的 `cca`（`HELPER_SUMS` 空着）用 `cli/target/release/cca-core`，或者 `CCA_HELPER` 指的（发布的不认这个变量）。`cca` 的最后一行是 `exit`：它后面的不是脚本，别往后面加东西。见 docs/dev/rust.md
- bash 和辅助程序之间的约定（子命令的参数、stdout 的格式、退出码）两边一起改；辅助程序里起外部命令照旧按名字起（回归把它们换成 PATH 上的假命令、核对参数）；辅助程序的依赖是白名单，加新的之前先问维护者
- 替换已安装的脚本只能用 `mv`：bash 边读边执行，原地覆盖会让正在运行的 cca 读到半截代码
- 联网下载一律用 `retry3`（有些网络会丢新连接；两次之间等 `CCA_RETRY_WAIT` 秒，默认 2，回归里是 0）
- Linux 上：别用 `cmp`、`diff`（最小安装的发行版没有 diffutils，`! cmp` 恒为真），比文件用 `sha256_of`；要账号专属的 Chrome、桌面 App、界面 App 的命令开头 `mac_only`。见 docs/dev/linux.md

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
- `install.sh` 下载前查的 ↔ `cca install` 的 `need_deps`；两份 `pkg_hint`
- 辅助程序的 `fill_plan` ↔ `router_rb` 的 `--fill-source`（参考节点的同一套规则）
- `VIRTUAL_IFACE_RE`：bash ↔ 辅助程序（`cli/src/cmd/corenet/bind.rs`；回归 lint 核对）
- 版本的先后：bash 的 `ver_gt` ↔ 辅助程序的 `ver_key` ↔ Swift 的 `AppVersion`
- `CORE_VERSION` 和四个 `CORE_SHA256_*` 一起换（macOS 的两行后面别加注释：mkver 的 sed 按整行换）
- `RELEASE_KEY`：`cca` ↔ `install.sh` ↔ `~/.ssh/cca_release_ed25519.pub`（release.sh 会检查）
- 辅助程序的四个目标：`cca` 的 `helper_find` ↔ `cli/pack.py` 的 `TARGETS` ↔ `release.sh` 的 `TARGETS` ↔ `install.sh` 查的机器种类；`cca` 里 `HELPER_SUMS=""` 那一行 ↔ `cli/pack.py` 认的那一行
- bash 里 `py 子命令` 的参数和输出 ↔ `cli/src/cmd/` 里那个子命令（`CMDS` 表）
- 辅助程序里的常量 ↔ tests/ 下把它当黑盒测的那几份里写死的同一个值：`UDP_HELPER`（`cli/src/cmd/corenet/udp.rs`）↔ `tests/local_udp.py`，`LEAK_FAIL_TTL`（`cli/src/cmd/leak.rs`）↔ `tests/isolation_leaks.py`
- App 的接口：PROTOCOL.md ↔ Swift 模型 ↔ regress 的 10g；`router_sh` 的 `status` 输出 ↔ `cmd_gui_live` 的 `GUI_SLOT_RE`（regress 守着）
- `VERSION` ↔ `WhatsNew.swift` 最前面的 `ReleaseNote`（`macos/test.sh` 查用词和版本先后）

## 细节在哪
动到哪一块，先读 `docs/dev/` 下对应的那一份（这一块怎么测也写在里面）：
- `rust.md` — 辅助程序（`cli/`）：cca 怎么找到它、怎么编和测、要守的规矩、依赖、对拍、主干还带着 Python 时怎么跟
- `gui-contract.md` — 给界面 App 的接口：`_gui hello|state|live`、事件、`consent`、提问会话和题号、`CCA_RO`
- `onboarding.md` — 配置向导（`cca _onboard`、`cca install --gui`）、配置路由器和账号表的表单、不先配置路由器就加账号
- `accounts.md` — 账号表的值、`base_order`、三种工具的差异：`Tool` 的子类、`cxa` / `aga`、CODEX_HOME、Codex 的 app-server、Antigravity
- `isolation.md` — 设备标识、共享的历史 / 记忆 / 配置、`child_env`、`--add-dir` 和 `--disallowedTools`、共享设置的说明和修复（`cca settings`）、启动前后的隔离检查
- `secrets.md` — 钥匙串和密钥文件、第三方 API 的 key、官方账号记下备查的 key
- `usage.md` — 查用量和用量表格、5 小时 / 按模型 / 每周的额度、周结算提醒、Codex 重置额度、登录失效、套餐档位、命令行会话登记、API 额度的入口
- `cost.md` — 折合金额的统计和同步
- `presence.md` — 设备统计的服务器（`./policy.sh presence`）、共享记下的 API key
- `policy.md` — 维护者的映射：拉取、合并进账号表、启动时的引导、锁定请求、deploy key（`./policy.sh`）
- `fill.md` — 维护者的参考节点，写进路由器的配置文件
- `router.md` — 路由器端：OpenClash / ShellCrash、几台设备共用一个路由器、节点改名
- `local-proxy.md` — 内置代理：mihomo 内核、拷节点、自定义节点（`cca node`）、本机 UDP 走节点
- `network.md` — 在家还是在外、出口检测的结果、DNS / WebRTC 泄露检测
- `linux.md` — Linux 上安装、服务器上登录、服务器上不用路由器
- `clients.md` — 辅助安装 claude / codex / agy、客户端统一更新
- `release.md` — release.sh 的步骤、测试版先发后验、自动更新、检查更新、`cca gui`
- `mail.md` — 邮箱助手（`cca mail`）
- `desktop.md` — 程序坞整理、Claude App 多开、启动器图标
