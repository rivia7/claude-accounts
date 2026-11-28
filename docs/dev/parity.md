# 三种工具的功能对照（Claude Code / Codex / Antigravity）

原则（根目录 CLAUDE.md「账号表和工具」那一条）：三种工具一样的地位。Claude 有的功能，Codex（ChatGPT 账号）、Antigravity（`agy`）能做的都要做；新功能三种一起做、一起测，列工具的文案三种都列（`cca help`、README 两份、App、手机、PROTOCOL、docs/dev）。上游做不到、cca 的规矩不许的，给 `Tool` 加开关、用 `FAIL_CODE=unsupported` 说清楚，并在下面「做不了的」写明卡在哪、等什么，不能默默只做 Claude 的。补上一项、发现新的差异，就改这一份。

2026-11-28 照代码和文档核对（HEAD 2e4c2b9，VERSION 3.7.13：4e9da05 的「三种工具一样的地位」在这一版发布了）；之后补上的（3.8.0-beta.3 发布）：Antigravity 的远程启动带开场白、`cca agy settings fix`、`cca agy key` 和还原时删它的测试、文案里漏了的 Antigravity、Antigravity 的每周用量（每组额度各记一份）、Linux 上测 Antigravity（`tests/linux.sh`，2026-11-28 在测试机上 53 条全过）、Codex 和 Antigravity 的凭证防护（isolation.md「Codex、agy 的凭证防护」；Linux 上的 Codex 也加）、出口地区的名单按工具分（network.md）、Antigravity 的权益页（`cca agy billing`）。差异是怎么实现的（`Tool`、bash 的 `TOOL` 和 `tool xxx`）见 accounts.md「三种工具的差异」。

## 写新功能时
- 先按三种工具想：要么三种都做，要么给 `Tool` 加能力开关（照 `has_app`、`has_api`、`has_usage`、`has_cost`，`_gui state` 的 `features`），做不了的那种 `FAIL_CODE=unsupported`，再在「做不了的」写一条。App、手机只看 `features`、`can` / `can_why`，不按工具名猜（gui-contract.md）
- 测试三种都要有：Codex 的在回归 10f、Antigravity 的在 10f2（假 `agy` 是 `cca-test fake-agy`），Linux 上的在 `tests/linux.sh`
- 文案：列工具的地方三种都列；只有一两种有的，写明另一种为什么没有（像「Antigravity 没有桌面 App」）
- 再核对一遍的做法：`Tool`（`cli/src/core/tool.rs`）的开关；`cca` 里 `[ "$TOOL" = … ]`、`case "$TOOL"`、`FAIL_CODE=unsupported` 的分支；各份 docs/dev 和 PROTOCOL 的 `features` / `can_why`；漏写的文案 grep 只列了两种的写法（「Claude Code、Codex」「claude / codex」「Claude、ChatGPT」「Claude / Codex」「Claude、Codex」），去掉带 agy、Antigravity 的行；PROTOCOL 里的 `cca [codex] …` 照它开头的约定也指 `cca agy …`

## 对照
✓ 做了，✗ 没有（下面「还没补的」或「做不了的」里有它），— 这个工具没有这回事；括号里是做法不同的地方。

| 功能 | Claude Code | Codex | Antigravity |
|---|---|---|---|
| 添加官方账号、在账号的 Chrome 里登录（补 `login_hint`） | ✓ | ✓（经 app-server） | ✓（授权码贴回终端） |
| 服务器上登录（`login-page`） | ✓ | ✓ | ✓（授权码） |
| 登错号、登录失效 | ✓（失效分不清，`nolimits`） | ✓（`auth`） | ✓（`auth`：agy 打出登录网址） |
| 第三方 API 账号 | ✓（带 `env`） | ✓（Responses 接口，没有 `env`） | ✗ |
| 官方账号记下备查的 API key | ✓ | ✓ | ✓ |
| 记下的 key 经 Team 同步给别的设备（keyshare） | ✓ | ✓ | ✗ |
| 设备标识、共享白名单、配置三方合并、跨账号续接 | ✓ | ✓ | ✓ |
| 继承的环境变量过滤、启动前查设置、`settings` 的说明 | ✓ | ✓ | ✓（只有 `gcp` 一类） |
| `settings fix` | ✓ | ✗（TOML，见「做不了的」） | ✓（只去掉 `gcp` 一类） |
| 挡住 `~/.cca` 里的凭证（`CCA_DIR_PRIVATE`，isolation.md） | ✓（`--add-dir`、`--disallowedTools`） | ✓（`-c` 定义、选中的权限配置，macOS、Linux 都加；用户自己定了沙箱时不加、说一句；往 deny 的目录里新建文件挡不全，见「还没补的」） | ✓（`--settings` 的 `permissions`；登录着的会话里没核实） |
| 出口检测、纯净度、DNS / WebRTC 泄露、不可用地区（各家自己的名单，network.md） | ✓（claude.ai 的 trace） | ✓（chatgpt.com） | ✓（www.cloudflare.com） |
| 查用量 | ✓（5 小时、本周、分模型） | ✓（5 小时、本周、重置次数） | ✓（几组额度） |
| 每周用量统计（`usage_weeks`、用量统计页） | ✓ | ✓ | ✓（每组额度各记一份：`usage_weeks_groups`，usage.md） |
| 周结算提醒、额度用完 / 恢复的通知 | ✓ | ✓ | ✓（看用得最少的那组） |
| 套餐档位 | ✓ | ✓ | ✓（要经 cca 开过交互会话，或 Team 里别的设备记下） |
| 重置额度 / 看重置次数 | ✓（`limit-resets` 开网页） | ✓（`reset`） | — |
| 账单页（订阅附带的 API 额度） | ✓ | ✓ | ✓（Google 开发者计划的权益页） |
| 折合金额（含经 Team 同步） | ✓ | ✓ | ✓（只认得出经 cca 启动的；Linux 上要 `sqlite3`） |
| 设备统计（`devices`） | ✓ | ✓ | ✓（只有命令行会话） |
| 网页版（`web`） | ✓ claude.ai | ✓ chatgpt.com | ✓ gemini.google.com |
| 桌面 App 多开、启动器、程序坞整理 | ✓ | ✓ | ✗ |
| 邮箱助手（登录邮件里的链接） | ✓ | ✗ | — |
| 远程会话（会话宿主、接入、手机） | ✓ | ✓ | ✓ |
| 做完了 / 在等你推给接着它的地方 | ✓（cca 带的钩子） | ✓（notify；在等你只在宿主里） | ✓（状态栏的命令） |
| 这台 Mac 上弹通知（`popup`） | ✓（cca 带的钩子；用户自己的钩子已经会弹的那一种不弹） | ✓ | ✓ |
| 远程启动（模板） | ✓ | ✓ | ✓ |
| 远程启动带开场白（`line`） | ✓（`-- 开场白`） | ✓（同左） | ✓（`--prompt-interactive=开场白`，launch.md） |
| Team：账号表、别名、固定节点、给了马上加进来 | ✓ | ✓（马上加进来：3.7.13 起） | ✓（同左） |
| 配置向导、`cca install` 先问加哪种 | ✓ | ✓ | ✓ |
| 辅助安装（`install-cli`） | ✓ | ✓ | ✓（官方安装脚本没实测过） |
| 客户端更新：终端里的命令 | ✓ | ✓ | ✓（3.6.15 起） |
| 客户端更新：桌面 App | ✓ | ✓ | — |
| App：总览、账号页、用量 | ✓ | ✓ | ✓（几组额度） |
| App：用量统计页 | ✓ | ✓ | ✓（每组额度一行） |
| App：折合金额页 | ✓ | ✓ | ✓ |
| 手机：总览、账号、折合金额 | ✓ | ✓ | ✓ |
| 手机：用量统计 | ✓ | ✓ | ✓（每组额度一行） |
| Linux 上的测试（`tests/linux.sh`） | ✓ | ✓ | ✓（假的 agy 是回归里那个 `fake-agy`） |
| 卸载、还原 | ✓ | ✓ | ✓ |

## 还没补的（能做，先后照用处排）
1. **Codex 的凭证防护挡不住往 deny 的目录里新建文件**：2026-11-28 实测（codex 0.172.0，离线：macOS 上的 Seatbelt，测试机上 Ubuntu 26.04 的 bubblewrap；isolation.md「Codex、agy 的凭证防护」）：`:workspace` 又让 ~/.cca 可写的时候（工作目录是记过信任的 HOME、`--add-dir` 盖住了它、HOME 在 /tmp 下），Linux 上 deny 只把已有的文件一个个盖住，往 `teams/`、`secrets/` 里新建文件、建目录、把 `teams/` 改名都能成；macOS 上已有的目录连新建也挡住；两边都挡不住还没有的 `目录/**` 被建出来（还没加入过 Team 时的 `teams/`、没加过 Antigravity 账号时的 `agy/`）。读都挡住了，`:read-only` 下写也都挡住。做法：~/.cca 在可写的目录里时再加一条 `"<~/.cca 的真实路径>" = "read"`（先在两边核对它和工作目录的 write、里面的 deny 谁先，用户自己 `--add-dir ~/.cca` 时怎么办）
2. **真机核对 Antigravity**：`cca agy install-cli`（官方安装脚本没实测，clients.md）、`agy update` 从旧版升上来、开关下重新登录的整个流程和 token 文件的格式（accounts.md「还没核实的」）；远程启动带的开场白在登录着的账号上是不是真的当第一句发出去（只离线核对过：1.5.18 认 `--prompt-interactive=`、不认放在最后的一句话，launch.md「开场白放进参数」）。2026-11-28 想用维护者的一个 Antigravity 账号带开场白启动一次核对，自动模式的检查不让跑：要维护者自己在终端里跑（`cca agy @别名 --prompt-interactive="请只回复 CCA"`，看 agy 是不是把它当第一句发出去、回的是不是 CCA）；凭证防护（isolation.md「Codex、agy 的凭证防护」）在登录着的会话里真的挡不挡：让它读 `~/.cca/teams` 下的一个文件、再在它的终端里 cat 一次，看是不是都被拒；Google 开发者计划的权益页（`cca agy billing`，usage.md「API 额度」）在登录着的账号上是什么样、没开 Cloud 结算时是什么样
3. **ChatGPT App 里的会话认不认得出账号**：折合金额认 Codex 的会话靠会话记录里第一个 `session_meta` 的 `creator_user_id`（cost.md）；ChatGPT App 起的会话是不是都带着它没核实过。认不出就照 Claude App 的 `cca cost hook` 想办法补。2026-11-28 在维护者的 Mac 上只数了、没读内容：`~/.codex` 里的会话都是命令行起的（`originator` 是 `codex-tui`）、都带着它，ChatGPT App 起的一个也没有，所以还是没核实：在 ChatGPT App 里用一次 Codex 再数一遍

## 做不了的（卡在上游或 cca 的规矩，条件变了再做）
- **Antigravity 的桌面 App 多开、启动器**：Antigravity 编辑器不认资料目录参数（`cca` 里 `agy_app_path` 那一段的注释），多开的实例会共用登录。要等它能按资料目录多开、登录也存在资料目录里（Claude App 多开只认 `--user-data-dir` 的规矩一样适用）
- **Antigravity 的第三方 API 账号**：`api_problems` 拒收对象，`AGY_INHERITED_RE` 挡着 `GEMINI_*`、`GOOGLE_API_KEY`。agy 要是只从环境变量读 key，就违反「密钥绝不进命令行和环境变量」：先核实它有没有像 Claude 的 `apiKeyHelper`、Codex 的 `auth.command` 那样从命令里拿 key 的办法，有才做
- **Antigravity 记下的 key 经 Team 同步**：派生密钥要一个只有登录了这个账号的设备才知道的身份（`Tool.share_ident`：Claude 的 `accountUuid`、Codex 的 `chatgpt_user_id`）；agy 的登录文件里 cca 只读邮箱、不碰 token，没有这样的身份（presence.md）
- **Antigravity 的重置额度**：Google 没有重置次数（`limit-resets` 回 `unsupported`）
- **Antigravity 的折合金额认账号、套餐**：会话库里没有账号的身份，只认得出经 cca 启动的那几次运行（不经 cca 的算认不出账号）；套餐 1.5 起只有交互会话的状态栏给。都是上游给的信息不够，cca 已经尽量补了（cost.md、accounts.md「agy 的套餐」）
- **Codex 的 `settings fix`**：TOML 改写会丢注释，只说是哪几项、让用户手动删（isolation.md）
- **Codex 在等你（不在会话宿主里时）**：Codex 的 `notify` 只给一轮做完了；在等你只能靠宿主读终端里的 OSC 9（remote.md「Codex、Antigravity 的通知」），不放进宿主的会话 cca 看不到，只有终端自己的提醒。等 Codex 的 notify 给出在等你的事件再补
- **邮箱助手**：认的是 Anthropic 带登录链接的邮件（`MAIL_RULES`，mail.md）。Antigravity 是 Google 登录，用不上；OpenAI 的登录邮件（验证码还是链接）没核实过，cca 不往网页里填东西，只有链接才帮得上

## 只有一种工具才有、不用补的（厂商自己的功能）
- Claude：`limit-resets`（claude.ai 的用量页）、分模型的每周额度（`seven_day_opus`、`seven_day_sonnet`……）、`outdated_login_plan`、`cca cost hook`（Claude App 里的 Code 会话不写 `session-owner`，要它记下账号；Codex 的会话按 `session_meta` 的 `creator_user_id` 认，见上面「还没补的」第 4 条）
- Codex：`cca codex reset` 和剩下的重置次数、profile、ChatGPT App 打开前要同意关掉 `system_proxy_fallback`
- Antigravity：几组额度（`quota_groups`）、状态栏的命令记套餐、登录存成文件的探测（`agy_probe`）

## 反过来：Claude 比另两种少的
- 登录失效：Claude Code 的 `get_usage` 分不清是失效还是网络（`nolimits`，usage.md），Codex、agy 都说得清。上游

## 文案
- 2026-11-28 补齐了列工具时漏了 Antigravity 的地方（还没发布）：`cca help`（第一行、`devices`、`clients status`，Codex、Antigravity 两节加了 `settings`）、`cca` 开头的注释、`cca restore` 说的 `key --show`；客户端更新（App 的页面和教程、README 两份、PROTOCOL 8）；折合金额（App、手机里「还没有……账号」，`cost_total` 是 null 的条件：PROTOCOL 2.2 / 3.5 和 Swift、辅助程序里的注释）；手机网页端的说明；macos/CLAUDE.md、macos/README.md、docs/dev 的几份；项目的 `.claude/hooks/tree-status.sh` 也认得出同一份检出里的 agy；补上 Antigravity 的每周用量以后，去掉了 App、手机的用量统计页和 README 两份里「暂时没有按周的统计」那一句
- 剩下只写了 Claude、Codex（或 Claude、ChatGPT）的，都是确实只有这两种的：第三方 API 账号、记下的 key 经 Team 同步（keyshare）、服务器上登录时的本机回调（agy 是授权码贴回）、测试里只造了这两种账号的用例
