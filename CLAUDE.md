# CLAUDE.md

`cca` 按 GitHub Releases 分发，是单个签名文件：bash 3.2 主体 + 内嵌 Python 辅助（`PY_SRC`，由 `py` 以 `-I -X utf8` 运行）+ 路由器端 busybox ash（`router_sh`）和 ruby（`router_rb`）。新功能也写进这个文件。

## 命令
- `bash tests/regress.sh` — 全流程回归，约 2 分钟；全用假的路由器/claude/curl/ssh/GitHub，不联网、不碰真实 `~/.cca`；`KEEP=1` 保留沙箱；只能在 macOS 上跑（BSD sed、/bin/dash、ruby）
- `bash -n cca && bash -n install.sh` — 快速语法检查
- `./release.sh "更新说明"` — 先把 `cca` 里的 `VERSION` 改大并提交（工作区要干净）；跑回归、签名、打 tag、推送，发布 cca、cca.sig、install.sh

## 约定和坑
- 非 ASCII 字符前的变量一律写 `${VAR}`：macOS bash 3.2 在 UTF-8 下会把多字节字符的首字节吞进变量名（回归里有 lint）
- 交互输入从 fd 3 读（终端或 `CCA_TTY` 答案文件），提示写 stderr；stdout 只输出数据，很多函数靠 `$(...)` 取返回值
- 会让 OpenClash 重载的操作必须先征得同意（路由器端返回 3 → `allow_reload`），cron 永远不重载；重载会让局域网新连接中断约 20 秒
- 替换已安装的脚本只能用 `mv`：bash 边读边执行，原地覆盖会让正在运行的 cca 读到半截代码
- 改了 `router_sh`/`router_rb` 会改变同步摘要，已安装的机器下次启动会自动重新部署路由器端
- 账号身份按邮箱（`acct_id`）；账号表的键是 `别名(邮箱)`，改别名不能影响登录目录和端口
- 账号表的值是字符串（官方账号，值是节点）或对象（第三方 API 账号：`base_url`/`model`/`node`/`env`/`key`/`id`）。读值一律用 `is_api`/`node_of`，不要假设是节点字符串；`c['ports']` 里只有走节点的账号（直连的 API 账号没端口），要端口时用 `routed(c, t)`。第三方 API 账号按 `id` 认（`API_IDS`，派发子命令前 `load_ids()` 读好；config.json 的 `api_ids` 记着上次的 键 → 账号目录名，改几次别名端口和"上次使用"都认得出），改别名时目录、key、端口不变
- 只有官方账号的账号表，`router_accounts` 的输出必须和以前逐字节一样：否则所有已安装的机器升级后都会重新部署路由器端
- 密钥绝不进任何进程的命令行和环境变量：存钥匙串用 `security -i` 从 stdin 读命令、`-X` 十六进制（`-i` 里的命令失败也返回 0，必须读回来比对；只收可见 ASCII，否则 `-w` 输出十六进制）；交给 Claude Code 用 `--settings` 里的 `apiKeyHelper`（回调 `cca _api-key <账号目录名>`，只给账号表里现有的账号），地址和模型也放在这份 `--settings` 的 env 里（优先级高于共享 settings.json 和进程环境；第二个 `--settings` 会把它整个顶掉）。env 会出现在命令行上，所以像密钥的变量名（`ENV_SECRETISH`）不收
- 钥匙串里第三方 API 的 key：服务名 `cca-api-<账号目录名>`，账户名是接口的域名（换服务商就取不到旧 key）。代码里叫 secret（`secret-*` 子命令），免得和账号表的"键"混了。cca 存过的记在 config.json 的 `api_keys`，`secrets-forget` 据此清掉已经不在账号表里的（`cca edit` 之后、每次启动、`cca restore` 时全部清）
- `child_env` 一律不带继承来的 `ANTHROPIC_*`、`CLAUDE_CODE_OAUTH_TOKEN`、云厂商/网关开关（`CLAUDE_CODE_USE_BEDROCK` 等，见 `INHERITED_RE`）：shell 里的 `ANTHROPIC_BASE_URL` 会把官方账号的登录 token 发到别的地址。`INHERITED_RE`（bash）和 `CRED_ENV`（PY_SRC，共享设置检查和 env 校验用）是同一份名单的两种写法，要一起改；Claude Code 新增 provider 开关时补上（`strings` 二进制里的 `CLAUDE_CODE_USE_` 能查到）
- `cca usage` 用的 `get_usage` 是 Claude Code 标为实验性的 SDK 控制请求：`claude -p --safe-mode --no-session-persistence --strict-mcp-config --input-format stream-json --output-format stream-json --verbose`，stdin 写一行请求，拿到 `control_response` 之前别关 stdin；字段要宽松解析，不支持时返回 `error: Unsupported control request subtype`。查询必须经账号自己的端口（和启动一样），登错号、出口在受限地区时不查
- 联网下载一律用 `retry3`（有些网络会丢新连接）
- 仓库是公开的：代码和测试里不放真实节点名、路由器 IP、邮箱；测试用 `US-A`…`US-D`、`HK-A`、`192.0.2.10`、`example.com`
- `cca` 和 `install.sh` 里各有一份 `RELEASE_KEY`，必须和 `~/.ssh/cca_release_ed25519.pub` 一致（release.sh 会检查）
- 改动要发版才会到达已安装的机器（包括维护者自己的 Mac）；`cca` 旁边有 `.git` 时是开发模式，不自动更新
- Claude App 多开只认 `--user-data-dir`（签名版忽略 `CLAUDE_USER_DATA_DIR`）；App 不加单实例锁，同一个资料目录绝不能开两份，`cmd_app` 先用 `app_pid` 查
- 启动器图标的轮廓必须和原图标一样：标识伸出圆角方形，macOS 26 起会把整个图标缩小、关进灰框。看效果要导出 `NSWorkspace.iconForFile`（系统实际显示的样子），只看 icns 本身发现不了
- `PY_SRC=$(cat <<'PY' … PY)` 里不能有落单的 `'` 或 `"`：bash 3.2 扫描 `$(…)` 时会数引号，认错结尾后报错的行号在后面毫不相干的地方。正则和字符串里写 `\x22` / `\x27`
- 出门在外（连不上路由器）走本机 Clash：Clash Verge 的 Merge 会整个替换列表，所以端口池写进全局扩展脚本（包住用户的 main）；生效靠 `PUT /configs` 热加载它当前的运行配置（文本插入，不重写），不重启 Clash Verge。它的内核控制接口默认只有 Unix socket（运行配置里的 `external-controller-unix`）
- 在外面不连路由器：路由器同步留到回家后的下次启动（摘要对不上会自动补）；`at_home` 只探测 1 秒

## 测试写法
- 交互流程：`CCA_TTY="$(ans 答案1 答案2 …)"`，一行一个答案
- 方向键菜单和 readline 输入要在真 pty 里测：`script -q 日志 bash 驱动脚本`，配合定时输入
- 自动更新：用 `tests/regress.sh` 里的 `mkver`/`publish`（临时发布密钥 + `$CCA_T/gh` 下的假 GitHub）
- Claude App：`run` 里的 `CCA_CLAUDE_APP` 指向假的 Claude.app；假 `open` 遇到 `--user-data-dir` 会建 `Local State`（有 `open_noprofile` 时不建）
- 出门在外：`touch "$T/away"` 让假 `nc`/`ssh` 连不上路由器；假 Clash Verge 的内核由假 `curl` 的 `--unix-socket` 分支模拟（状态在 `$T/verge`）；`$T/verge_regen` 用真的 JavaScript 引擎（osascript）跑全局扩展脚本，模拟它重新生成配置
- 测试辅助脚本只用沙箱里的路径，不能靠 `$HOME`：在 `run` 外面直接调用时 `$HOME` 是真实的主目录（会碰到真的 Clash Verge 等）
- 钥匙串：假 `security` 把条目存在 `$T/keychain/<服务名>/<账户名>`，命令行记在 `security.log`、`-i` 读到的命令记在 `security.stdin`；`touch "$T/keychain_locked"` 模拟存不进去。假 `env` 把每次的命令行记在 `$T/env.log`（检查 key 没进命令行）。`cca edit` 用 `EDITOR="$T/edit.py" CCA_TEST_EDIT='j.pop("x")'` 这样改表
- 第三方 API：假 `curl` 对 `https://api*.example.com/*` 回 401（直连记 `CURL api direct`），`touch "$T/api_down"` 连不上；假 `claude` 遇到 `--settings` 记下 `SETTINGS: env=…`，并像 Claude Code 一样执行 apiKeyHelper，取到的 key 写进 `$T/claude.key`（不进日志）
- 用量：假 `claude` 的 stream-json 分支是 `$T/fake_usage.py`，按登录的邮箱回 `$T/usage/<邮箱>.json`（`canned` 生成），`$T/usage/unsupported`、`$T/usage/hang` 模拟不支持和卡住；`$T/loc/<端口>` 改 claude.ai trace 里的出口地区
- 菜单行很宽：要看行尾的内容时用 `run COLUMNS=160 …`（`tput cols` 认 `COLUMNS`）
