# 设备标识、共享资料和隔离防护

## 设备标识
- 客户端设备标识由账号目录的 `device.json` 固定保存：Claude 的 `userID` / `machineID` 是 64 位小写 hex，Codex 的 `installation_id` 是 UUID 文本。`prepare_device` 必须在 `Lock` 内调用（`py device`、`py link`、Claude 的 `py sync`）；先于登录和查用量初始化，重新登录/改别名不重置。首次接管保留合法独立旧值，重复值和共享链接迁移；写设备文件用原子替换且 `save_json(..., follow_symlinks=False)`，不能写穿共享源。浏览器 profile 隔离不等于 Canvas/WebGL/硬件指纹隔离。
- 已只读核验 Claude Code 2.1.318：`userID` / `machineID` 均为 `crypto.randomBytes(32).toString("hex")`，合法旧值复用；Codex 0.166.0 的 `core/src/installation_id.rs` 使用 UUID v4，合法文件复用。当前生成方式与这些版本一致；尚未验证全部登出/升级调用链，不因共享机制改动重置 ID 或擅自更换初始化流程。

## 共享的历史、记忆和配置
- 历史、记忆、配置和跨账号续接是硬性要求。Claude 的 `CLAUDE_SHARED` 与 Codex 的 `CODEX_SHARED` 明确列出共享工作资料，未知新增状态默认独立；Claude 的 `projects` 包含会话及记忆（工作流续跑用的进度记录也在会话目录里，续跑只认同一个会话 ID），`tasks` 是会话的任务列表（`tasks/<列表 ID>`，列表 ID 默认是会话 ID；改成白名单时漏过它，各账号各有过一份：`py link` 并回时同一个列表整份认，不一样的留较新的、较旧的整份移进 conflicts，见 `adopt_task_lists`，逐个文件拼会混进过时的任务），Codex 会话数据库仍共用。不要用隔离这些数据来实现设备隔离。旧 Claude 未知共享链接只拆指向主目录同名项的，先复制再替换，不改共享源；含嵌套链接时停止自动拆分，用户外部链接不动。
- 共享顶层配置基线保存在账号目录 `.cca-shared-base.json`（600 权限）。`py link` 在锁内修复原子保存替换掉的链接：JSON 按字段合并，冲突保留共享值并备份完整账号副本；TOML/普通文本整文件三方比较，不逐行拼接；无基线分歧和坏 JSON 也保留副本。`.claude.json` 仅合并 `SHARED_KEYS`，冲突同样备份。CLI 启动/退出已有 `py link`，桌面 App 仅启动前同步，不承诺即时刷新；启动、登录、桌面 App 经 `prepare_account` 合并后必须再次 `settings-check`。

## 子进程的环境和权限
- `child_env` 一律不带继承来的 `ANTHROPIC_*`、`CLAUDE_CODE_OAUTH_TOKEN`、云厂商/网关开关（`CLAUDE_CODE_USE_BEDROCK` 等，见 `INHERITED_RE`）：shell 里的 `ANTHROPIC_BASE_URL` 会把官方账号的登录 token 发到别的地址。`INHERITED_RE`（bash）和 `CRED_ENV`（辅助程序，`cli/src/core/base.rs`；共享设置检查和 env 校验用）是同一份名单的两种写法，要一起改；Claude Code 新增 provider 开关时补上（`strings` 二进制里的 `CLAUDE_CODE_USE_` 能查到）
- cca 启动的 Claude Code 默认只对这个账号自己的目录放开权限（`cca_dir_args(key)`，在 `Tool.run_args` 里，官方和第三方 API 账号都加）：`--add-dir=` 只加 `adir(key)`（`~/.cca/accounts/<账号目录名>`，CLAUDE_CONFIG_DIR 的上一层；读不用确认，改照权限模式），不加整个 ~/.cca（账号表、config.json、映射、日志、别的账号的目录读之前照权限模式确认）；`--disallowedTools=` 照旧挡住整个 ~/.cca 的 `CCA_DIR_PRIVATE`（deploy key 私钥、参考节点 policy/nodes.json、内置代理的节点和配置（core/nodes.json、custom-nodes.json、config.yaml）、Codex 的 auth.json、agy 的登录 token、Linux 上 Claude Code 的 .credentials.json、密钥文件 secrets/**、Chrome / 桌面 App 资料目录，包括这个账号自己的；不在工作目录里也挡，deny 规则先于确认和 bypassPermissions；规则里 `//` 开头是绝对路径，Read 规则也管 cat、head 这类命令）。两个参数必须写成 `--x=值`：都能跟好几个值，写成 `--add-dir 目录` 会把后面用户写的提示词也当成目录吞掉（真 Claude Code 2.1.318 验证过：报 Input must be provided）。config.json 的 `claude_add_dir: false` 两样都不加；~/.cca 里新加了凭证类的文件，要补进 `CCA_DIR_PRIVATE`。Codex 不加（`CodexTool.run_args`）。
  这两个后面再跟一个 `--settings`：折合金额记下会话的 SessionStart hook（`session_hook`，回调 `cca _session-start 账号目录名`；官方账号只有
  `{"hooks": …}`，第三方 API 账号和 apiKeyHelper、env 在同一份里；config.json 的 `cost: false` 时不带，见 cost.md）。实测 Claude Code 2.1.328：
  它和配置目录 settings.json 里的 hook 一起跑；用户自己再给一个 `--settings` 时只有后面那个生效（cca 的在用户参数前面，于是这次没有 hook，
  会话照旧认不出账号；第三方 API 账号不许用户给 `--settings`）

## 共享设置的说明和修复
- 共享设置的说明和修复（bash 的 `settings_warn`、`cmd_settings`，辅助程序的「共享设置的说明和修复」一节）：Claude 的问题由 `py settings-explain shared | launch [启动参数…]` 照 settings-check / launch-check 重新查（`settings_warn "$bad" -- "$@"` 带上同样的启动参数），按文件说是哪份、哪几项（`SETTINGS_KINDS` 分类：第三方 API、云厂商或网关、代理、账号目录或登录凭证，认不出的算 other）、带着它启动会怎样、怎么办，只说路径和名字、不说值；Codex、agy 照旧列出问题。`cca settings [check]` 照 launch-check 查（这个目录的项目设置也查）；`cca settings fix` 只改共享的 `~/.claude/settings.json`、`settings.local.json`（项目设置、`--settings` 不改，照实说要手动删）：`py settings-fix plan` 给出摘要和说明，`consent settings_fix <摘要>`（文件、原文的摘要、要去掉的几项：文件变了就对不上），`apply` 在 `Lock` 里重算、摘要对得上才改：原文整份备份在旁边（`<文件>.cca-backup-<日期时间>`，600、`O_EXCL`；是链接（dotfiles）时备份在链接旁边、改它指向的文件，不写进链接指向的目录），备份后再读一遍没变才原子替换（`save_json` 保留权限）。去掉的是 `claude_config_problems` 那一套（`SETTINGS_BAD`、`apiKeyHelper`），有第三方 API、云厂商或网关时连同官方账号用不了的模型名（`SETTINGS_MODEL_ENV` 的、顶层 `model`，值不是 `CLAUDE_MODEL_NAME` 的）和 `ANTHROPIC_CUSTOM_HEADERS`；`API_TIMEOUT_MS` 这类不碍事的留着。CRED_ENV 加了名字时看看 `SETTINGS_KINDS` 要不要补。`cca codex|agy settings` 不建账号表（`main` 里跳过 `py init`），fix 只给 Claude（`unsupported`）。测试在 regress 10c

## 启动前后的检查
- 隔离防护：`py launch-check` 在 CLI 启动准备前后检查参数及当前项目祖先配置；配置诊断只输出路径/键名，禁止打印值。Chrome 缺失和旧 Codex 不支持独立登录时停止，不能降级默认浏览器。`cca web` 也须预检。泄露检测是宿主探针，失败/空结果不可当作“无泄露”；不宣称实测过浏览器。专项测试在 `tests/isolation_*.py`，完整回归也执行。
- 凭证和网络防护：Claude 固定 `CLAUDE_SECURESTORAGE_CONFIG_DIR=CLAUDE_CONFIG_DIR`，保留旧钥匙串命名；清理继承的 `CLAUDE_CODE_*_FILE_DESCRIPTOR`，共享/项目配置同样禁止。路由器、本机内核切节点必须 GET 回读；失败不能以端口开放放行。探针/控制 curl 的首参数为 `-q`，走代理时 `--noproxy ''`，直连时 `--noproxy '*'`。
- 客户端运行记录：`py runtime-record write|check` 绑定 PID/启动时间、实际程序路径/版本、账号目录/节点、代理；旧实例先预检，记录缺失或漂移要求退出重开，不杀已有工作。Chrome 新开先空白页核验再主动交付URL，不承诺浏览器自身不会恢复标签或后台联网。浏览器使用新版 `--webrtc-ip-handling-policy`（Chrome 157 的旧 `force-` 开关无效），关闭时 `py browser-profile prepare` 合并Preferences并清除URL例外；运行中只读检查。企业策略/扩展优先级仍是边界。
