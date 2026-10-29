# CLAUDE.md

`cca` 按 GitHub Releases 分发，是单个签名文件：bash 3.2 主体 + 内嵌 Python 辅助（`PY_SRC`，由 `py` 以 `-I -X utf8` 运行）+ 路由器端 busybox ash（`router_sh`）和 ruby（`router_rb`）。新功能也写进这个文件。

## 命令
- `bash tests/regress.sh` — 全流程回归，约 75 秒；全用假的路由器/claude/curl/ssh/GitHub，不联网、不碰真实 `~/.cca`；`KEEP=1` 保留沙箱；只能在 macOS 上跑（BSD sed、/bin/dash、ruby）
- `bash -n cca && bash -n install.sh` — 快速语法检查
- `./release.sh "更新说明"` — 先把 `cca` 里的 `VERSION` 改大并提交（工作区要干净）；跑回归、签名、打 tag、推送，发布 cca、cca.sig、install.sh

## 约定和坑
- 非 ASCII 字符前的变量一律写 `${VAR}`：macOS bash 3.2 在 UTF-8 下会把多字节字符的首字节吞进变量名（回归里有 lint）
- 交互输入从 fd 3 读（终端或 `CCA_TTY` 答案文件），提示写 stderr；stdout 只输出数据，很多函数靠 `$(...)` 取返回值
- 会让 OpenClash 重载的操作必须先征得同意（路由器端返回 3 → `allow_reload`），cron 永远不重载；重载会让局域网新连接中断约 20 秒
- 替换已安装的脚本只能用 `mv`：bash 边读边执行，原地覆盖会让正在运行的 cca 读到半截代码
- 改了 `router_sh`/`router_rb` 会改变同步摘要，已安装的机器下次启动会自动重新部署路由器端
- 账号身份按邮箱（`acct_id`）；账号表的键是 `别名(邮箱)`，改别名不能影响登录目录和端口
- 联网下载一律用 `retry3`（有些网络会丢新连接）
- 仓库是公开的：代码和测试里不放真实节点名、路由器 IP、邮箱；测试用 `US-A`…`US-D`、`HK-A`、`192.0.2.10`、`example.com`
- `cca` 和 `install.sh` 里各有一份 `RELEASE_KEY`，必须和 `~/.ssh/cca_release_ed25519.pub` 一致（release.sh 会检查）
- 改动要发版才会到达已安装的机器（包括维护者自己的 Mac）；`cca` 旁边有 `.git` 时是开发模式，不自动更新
- Claude App 多开只认 `--user-data-dir`（签名版忽略 `CLAUDE_USER_DATA_DIR`）；App 不加单实例锁，同一个资料目录绝不能开两份，`cmd_app` 先用 `app_pid` 查
- 启动器图标的轮廓必须和原图标一样：标识伸出圆角方形，macOS 26 起会把整个图标缩小、关进灰框。看效果要导出 `NSWorkspace.iconForFile`（系统实际显示的样子），只看 icns 本身发现不了

## 测试写法
- 交互流程：`CCA_TTY="$(ans 答案1 答案2 …)"`，一行一个答案
- 方向键菜单和 readline 输入要在真 pty 里测：`script -q 日志 bash 驱动脚本`，配合定时输入
- 自动更新：用 `tests/regress.sh` 里的 `mkver`/`publish`（临时发布密钥 + `$CCA_T/gh` 下的假 GitHub）
- Claude App：`run` 里的 `CCA_CLAUDE_APP` 指向假的 Claude.app；假 `open` 遇到 `--user-data-dir` 会建 `Local State`（有 `open_noprofile` 时不建）
