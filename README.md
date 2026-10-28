# cca：Claude Code 多账号启动器

在一台 Mac 上同时使用多个 Claude 账号。每个账号独立登录（官方登录流程），记忆、会话历史、设置、技能、插件、MCP 等其余状态全部共享；每个账号的 Claude Code 和网页版都走旁路由 OpenClash 上属于它自己的端口和出口节点。

## 工作方式

- **账号表** `~/.cca/accounts.json`：`{"别名(邮箱)": "出口节点"}`，别名可以省略。
- **账号隔离**：每个账号一个 `CLAUDE_CONFIG_DIR`（按邮箱命名，改别名不影响登录），登录凭证由 Claude Code 自己存进钥匙串。账号目录里其余条目都链接到 `~/.claude`；`~/.claude.json` 里与账号无关的键（项目信任、MCP、偏好）在每次启动和退出时三方合并。
- **按端口分流**：路由器上建一个端口池（7901 起，8 个一档），每个端口是一个 mixed 入站加一个 select 策略组。换节点、增删账号走 mihomo API，不重载 OpenClash；只有端口池要扩容或可选节点变了才需要重载（约 20 秒，期间局域网新连接会中断），会先征求同意。
- **不怕 OpenClash 升级**：路由器端文件放在 `/etc/cca`，通过 OpenClash 覆写脚本里的一行钩子注入；cron 每 2 分钟自检补回钩子，刷固件时保留 `/etc/cca`。
- **出口检测**：启动前经账号的端口取 claude.ai 看到的 IP，用 ip.net.coffee 的数据检查地区和纯净度；受限地区直接停止，纯净度低于阈值要确认。
- **网页版和登录页**：用该账号专属的 Chrome 实例（独立资料目录，走同一个端口），登录页预填账号邮箱。

cca 不读取、不保存、不转发任何 OAuth token，也不在账号之间共享登录；登录完全走官方的 `claude auth login`。

## 要求

- macOS，系统自带的 bash、python3、ssh、curl、perl
- Claude Code（`claude` 在 PATH 里）和 Google Chrome
- 局域网里一台装了 OpenClash（Mihomo 内核）的 OpenWrt 路由器，能 SSH 登录（第一次输一次密码，之后用密钥）

## 安装

```sh
curl -fsSL -o /tmp/cca https://github.com/rivia7/claude-accounts/releases/latest/download/cca
bash /tmp/cca install
```

安装会把脚本复制到 `~/.cca/bin/cca` 并链接到 `~/.local/bin/cca`，然后引导你配置路由器、从路由器的节点里选出可用的出口、添加第一个账号。

安装前想先校验签名：

```sh
curl -fsSL -o /tmp/cca.sig https://github.com/rivia7/claude-accounts/releases/latest/download/cca.sig
echo 'cca-release ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICxClUySZa2aVx7s6HRzufgJ49ad44smTnmkzDI/Osex' > /tmp/cca.allowed
ssh-keygen -Y verify -f /tmp/cca.allowed -I cca-release -n cca-release -s /tmp/cca.sig < /tmp/cca
```

## 使用

```text
cca                       方向键选账号（光标默认在上次用的账号上），回车启动 Claude Code
cca @<账号> [claude 参数]  直接用某个账号启动；账号可写别名、邮箱、编号或唯一前缀
cca add                   添加账号：填登录邮箱和别名（可选），选出口节点
cca edit | ls | rm | web | check
cca router setup | sync | status | nodes | log | ssh | uninstall
cca update [--check | --rollback | auto | notify | off]
cca restore               一键还原
```

完整说明见 `cca help`。

## 自动更新

- 每天在后台检查一次 GitHub Releases（脱离终端运行，不拖慢启动）。有新版本就下载并校验，下次启动时自动换上，再用新版本执行这次命令。
- 只安装用发布密钥签过名（`ssh-keygen -Y verify`）、语法检查通过、版本号比当前新的脚本。被篡改的、别的密钥签的、旧的版本一律不装。
- 替换用 `mv` 原子完成，正在运行的会话不受影响。`cca update --rollback` 退回上一个版本，并且不再自动安装刚退掉的那个版本。
- 新版本如果改了路由器端脚本，下次启动时自动重新部署（不重载 OpenClash；需要重载时照旧先问）。
- `cca update notify` 只提示不安装，`cca update off` 关闭。从 git 仓库里直接运行的 cca（开发模式）不自动更新。

发布公钥指纹：`SHA256:/JDnxJ7oMpg5H5K8rBbhsRo6sKoL65IaVLtQGeGrIZQ`

## 还原

`cca restore`：每个账号退出登录并删除钥匙串凭证；路由器上移除钩子、cron、端口池、`/etc/cca` 和 cca 的 SSH 公钥；本机先打包备份配置，再删除 `~/.cca` 和 `~/.local/bin/cca`。`~/.claude` 和 `~/.claude.json` 保持原样。

## 开发

- `bash tests/regress.sh`：全流程回归。用假的路由器、claude、curl、ssh 和 GitHub，不联网，也不碰真实的 `~/.cca`。
- 发布：改 `cca` 里的 `VERSION` 并提交，然后运行 `./release.sh "更新说明"`。它会跑回归、签名、打 tag、推送并创建 GitHub Release。签名私钥默认在 `~/.ssh/cca_release_ed25519`。
