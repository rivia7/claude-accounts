# 卸载（`cca uninstall` / `cca restore`）

两个名字是同一条命令（`cmd_restore`；确认时输入的就是用的那个名字）。`restore` 是原来的名字，文档、提示里说“还原”的地方照旧。
它只在终端里跑（`need_tty`），界面 App 没有入口（PROTOCOL 4.6 的对应表）：App 开着时它先停下。

## 做什么、先后

1. **先查能不能做，不能就什么都不动**：账号的桌面 App、账号专属的 Chrome、cca App 还开着（资料目录在 `~/.cca` 里，删了它们还接着往里写，
   `~/.cca` 又长出来）；还有开着的远程会话（删了 `~/.cca` 就再也接不上）；要打包备份却没有 `tar`。
2. 列出要做的事，输入命令的名字确认。
3. 每个账号：把会话里新产生的共享内容并回 `~/.claude`、`~/.codex`、`~/.gemini`（`py link`、`py sync`），退出登录、删凭证；钥匙串里 cca 存的
   key、邮箱助手的密码、远程会话配对的密钥；退出各个 Team（`restore_teams`：服务器上移除这台设备；是管理员的，根密钥跟着 `~/.cca` 删掉、
   备份里也不带，第 2 步列出来时说了）；路由器（`router_uninstall`，失败时问要不要接着做本机的）；本机端口池、本机 UDP 的系统服务、内核、
   crontab 里开机起内核的那一行、上报和统计折合金额的后台进程；`cca cost hook on` 加在共享的 `~/.claude/settings.json` 里的 hook（`py cost-hook off`：
   要拿 `~/.cca` 里的锁，所以在删 `~/.cca` 以前；去不掉（不是合法的 JSON 之类）时说一声、接着做，cca 删掉以后它什么也不做）。
4. 打包备份 `~/cca-backup-日期.tar.gz`（600；哪些不进包见 `cmd_restore` 里 `tar` 那一句的注释）。没打成就留着 `~/.cca`、停下。
   `--no-backup` 跳过这一步（也就不要 `tar`）。
5. 删 `~/.local/bin` 里 cca 建的链接、cca App、`~/.cca`，然后是放在 `~/.cca` 以外的（下一节）。**删了 `~/.cca` 以后不能再调 `py`**：
   解出来的辅助程序这就删掉，`~/.cca/bin/cca` 也没有了，再解不出来。

## `~/.cca` 以外的

| 东西 | 在哪 | 谁删 |
|---|---|---|
| 解出来的辅助程序（各个版本的） | 缓存目录的 `cca-helper/`（`CACHE_DIR`：macOS 是这个用户的临时目录，Linux 是 `${XDG_CACHE_HOME:-~/.cache}`） | `cache_leftovers`：整个目录。仓库里的 cca（辅助程序在 `cli/target` 下）不动它 |
| 程序坞整理的排队记录 | `DOCK_DIR`（缓存目录的 `cca-dock/`） | `cache_leftovers` |
| 3.5.0 以前的版本留下的字节码缓存 | 缓存目录（Linux 上还可能在 `XDG_RUNTIME_DIR`）的 `cca-py/` | `cache_leftovers` |
| 内置代理控制接口的 socket | `core_sock` | `cache_leftovers` |
| 界面 App 的缓存、内核日志、窗口状态、设置 | `~/Library/{Caches,HTTPStorages,Saved Application State,WebKit}/local.cca.app…`、`~/Library/Preferences/local.cca.app.plist` | `gui_leftovers`（`cca gui remove` 也调）：按 bundle id 认。设置先 `defaults delete`，它留下的空文件再删 |
| shell 启动文件里把 `~/.local/bin` 加进 PATH 的那两行 | `path_hint` 写的：`~/.zshrc`、`~/.bash_profile`、`~/.bashrc`、`~/.profile` 之一 | `path_unhint`，见下 |
| 空了的 `~/.local/bin` | | `path_unhint`（`rmdir`：里面还有东西就删不掉） |
| Claude App 里的会话记下账号的 hook（3.6.9 起） | 共享的 `~/.claude/settings.json` 的 `hooks.SessionStart` 里命令带 ` _session-start --desktop` 的那一条 | 第 3 步的 `py cost-hook off`（只去掉这一条；第 2 步的清单里开着时才列）。文件是 cca 为它新建的，去掉以后留下 `{}` |

`path_unhint` 的规矩：
- 只认 `PATH_MARK`、`PATH_LINE` 这两行一字不差、挨在一起的（`path_hint` 写的就是这两个变量：改了它们，以前写下的就认不出来了）；
  前面那个空行也是 `path_hint` 写的，一起去掉。用户改过的不动。
- `~/.local/bin` 里还有别的命令时留着、说一声：`claude`、`codex`、`agy` 的官方安装器也装在那，cca 帮着装的时候 PATH 里已经有这个目录，
  安装器自己就没再写启动文件——去掉这两行它们就不在 PATH 里了。
- 启动文件是链接（dotfiles 管着的）时不改，说一声。
- 先拷一份（权限跟着）、写好了再 `mv` 换上去：写到一半出错不会把用户的启动文件弄坏。

## 不删、要用户自己处理的

- `~/.claude`、`~/.claude.json`、`~/.codex`、`~/.gemini` 和 `claude` / `codex` / `agy` 命令：不是 cca 的。cca 写进这几份里的东西也留着
  （`~/.claude.json` 里给附加目录记的 `hasTrustDialogAccepted`、征得同意后在 `~/.codex/config.toml` 里关掉的 `system_proxy_fallback`、
  `cca settings fix` 留在旁边的 `settings.json.cca-backup-*`）：没记过哪些是 cca 写的，分不出来。
- Google 账号里的应用专用密码、Team 的同步服务器（只退出，不卸载服务器：管理员照 team.md「部署」卸载）。
- 钉在程序坞里的启动器图标：启动器删了以后是个问号，没去改程序坞的设置。
- 临时目录里 App 放事件文件的 `cca-gui-events`：空目录，系统自己会清；开发时编的另一份 App 可能正用着。
- 本机 UDP 的系统服务在授权框里点了取消、路由器连不上却选了接着做：那一头留着，卸载时说了怎么办。

## 测试

- `tests/path_unhint.sh`（回归的 unit 一节）：把 `path_unhint` 从 `cca` 里切出来，临时目录当 HOME 跑各种情况（逐字节还原、权限、
  目录里还有别的命令、改过的、链接、写不了、几个文件都有）。
- `tests/isolation_apps.py`（unit 一节）：把 `cmd_restore` 切出来配上桩，查不了桌面 App、账号专属的 Chrome 的进程时停下，不问、不动。
- regress 第 12 节：`cca restore` 在 App 开着、有远程会话时停下；`cca uninstall` 输错确认词、`--no-backup` 的清单、不认识的参数、
  账号专属的 Chrome 开着时停下；整条 `cca uninstall`（原来 `restore` 的那些断言，加上缓存目录、App 留在 `~/Library` 里的、
  启动文件里那两行留着的情况）；最后用仓库里的 cca 对一份空的 `~/.cca` 跑 `cca uninstall --no-backup`（不留备份、
  `~/.local/bin` 空了时目录和那两行都去掉）。
- 两个坑：
  - **`defaults` 不看 `HOME`**（经系统的设置进程读写这个用户真的 `~/Library/Preferences`，实测）：沙箱里用假的（`tests/setup.sh`，
    记在 `$T/defaults.log`）。没有它时，回归里的 `cca gui remove`、`cca restore` 每跑一次就把这台 Mac 上真的 cca App 的设置删一次。
  - **缓存目录在 macOS 上是这个用户真的临时目录**（`run` 不带 `TMPDIR`）：整条卸载的那两次带 `TMPDIR=<临时目录下单独的一个>`
    （`TMPDIR` 在 `/var/folders` 下时 cca 就拿它当缓存目录；`$T` 是 `/private/var/…` 开头的，不认），不然删掉的是真的 cca、
    同时在跑的别的回归解出来的辅助程序。
- Linux：`tests/linux.sh` 里整条 `cca uninstall`（原来 `cca restore` 那一条：备份、凭证不进包、crontab 那一行；加上缓存目录里的 `cca-helper`、`cca-py`
  删掉，`~/.bashrc` 里那两行去掉或者留着）；没有 `tar` 的发行版上先停下，再用 `--no-backup` 卸载。`tests/path_unhint.sh` 在 Linux 上也能直接跑
  （bash 5、GNU coreutils；取权限按 `OSTYPE` 分 `stat -f` / `stat -c`）。
