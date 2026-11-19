# 移植时在原实现里发现的问题

移植照原样做（见 PORTING.md），这里只记下来，要不要修、怎么修由维护者定，在主干上修了再跟着移植。
没有实跑验证的标着「读代码看出来的」。

## 有意和原实现不一样的地方

照原样移植是规矩；下面这几处做不到或者不该照做，行为和 Python 版有出入。都是正常使用碰不到的边角，列出来由维护者过目。

- **Codex 会话库的路径修复（`codex_fix_rollouts`）改成起系统的 `sqlite3` 命令。** 原来用 Python 自带的 sqlite3 库；白名单里没有
  SQLite 的库，Linux 的静态构建也链不了系统的。macOS 自带 `sqlite3`，行为一样（回归里那两条照过）；**Linux 上没装 `sqlite3`
  命令时这一步跳过**（账号删掉后，它开过的会话在 `codex resume` 的全部会话里看不到，文件还在；装上命令后下一次启动补上）。
  另外 `sqlite_home` 里的 `*`、`?`、`[` 原来会被当成通配符，现在按字面。
- **属性列表（plist）的值是一棵树。** 二进制文件里几处引用同一个数组 / 字典时读成各自一份，再写出去也各写一份；自己套自己的
  读不了；字典的键只认字符串；XML 只认 UTF-8、带 BOM 的 UTF-16、ISO-8859-1（见 `cli/src/py/plist.rs` 开头）。
- **TLS 出错原因的文字**（邮箱助手）是 rustls 的，原来是 OpenSSL 的；分类（证书校验没通过 / 别的握手失败）一样。
- **邮箱助手的邮件解析、IMAP 照 CPython 3.14.0 的行为手写**（email、email.header、email.utils、html.parser、imaplib、fnmatch；
  对照数据 `cli/tests/vectors/mail.json`）。这几个库各版本在边角上本来就不一样（macOS 自带的 3.9 和 3.14 就不同），Rust 这边固定成
  3.14.0 的。在这之上有几处做不到照样：
  - **字符集只认 UTF-8、US-ASCII、ISO-8859-1 / Latin-1、Windows-1252**（连同 Python 给它们的各种别名）。Python 认识的别的字符集
    （GBK、Big5、Shift_JIS、UTF-16……）在这里走原来「不认识这个字符集」的那条路：标题里的 `=?gbk?…?=` 不解码、照原样；正文照
    UTF-8 解（解不了的字节成 U+FFFD）；RFC 2231 的参数值用原样的。会不一样的输入：标题用这些字符集编码的邮件（核对标题时看到的
    是没解码的文字——Anthropic 的登录邮件标题是 ASCII，转发时加上的中文前后缀 Gmail 用 UTF-8）；正文是 UTF-16 这类和 ASCII
    不兼容的字符集的邮件（里面的链接取不出来，报「邮件里没找到登录链接」；GBK 这类兼容 ASCII 的，链接照样取得出，只有纯文本
    正文里链接后面紧跟着全角空格这类非 ASCII 的空白时不一样：Python 在那里断开，这里它成了 U+FFFD、连进链接里，这样的链接
    通不过 `link_ok`）。对照数据的 judge 一节里 `links_unknown` 记着这种时候 Rust 该给什么。
  - **一层套一层的上限照 3.14.0 直接跑 PY_SRC 时的算。** 地址里的注释、组，multipart / message 套到 Python 调用栈一千层的上限
    就是 RecursionError，落在第几层看解析的那个函数自己在第几层、是不是这个进程里头一次解析地址（见下面 mail 一节）。Rust 这边
    照这个规律算，每条路的上限两边都对拍过（`mail/*/nested-…`）。和别处一样（settings 一节「套得太深、数字太长……」那一条）：
    经 PY_BOOT 起的真的 cca 早一层到头，3.9 的层数又不同，这些不跟。
  - **放不进 64 位的数当作没有**：UID、UIDVALIDITY、UIDNEXT（IMAP 里它们是 32 位的；Python 是多大都行的整数）。`cca mail open --msg`
    里手写的这种 UID 照样发给 Gmail（原来就这样），但回应里这么大的 UID 认不出，说「邮件不在了」。
  - **Ctrl-C。** 原来 Python 在哪一句都可能抛 KeyboardInterrupt；这里是信号处理函数记一笔，等邮件歇着的时候、和代理握手的读被打断的
    时候、每条 IMAP 命令之前、等 `cca _mail-open` 的时候看到了才往上传（「已取消」、退出码 130 一样）。**正卡在 TLS 的读上时
    （等 Gmail 回一条命令）要等这次读回来才看得到**：rustls 把被信号打断的读自己接着读了（0.23 的 `complete_io`），Gmail 一直不回的话
    最多等到 20 秒的超时才说「已取消」，原来是马上说（说的话、退出码一样，只是晚；这一条没有对拍用例）。处理函数只在 `cca mail open`
    原来 `try` 的那一段装；那之前（读钥匙串、问系统代理）按 Ctrl-C 是系统默认的直接结束，没有 Python 的调用栈。
  - **关连接时多发一个 TLS 的 close_notify**（Python 的 `shutdown` 直接关套接字）；信任的根证书只读 `/etc/ssl/cert.pem`
    （原来是那个 Python 链的 OpenSSL 的默认位置：macOS 自带的 Python 就是这个文件，Homebrew 的是它自己那份）。
  - **系统代理的主机名里有非 ASCII 字符时不做 IDNA 转换**（原样交给系统解析；Python 先转成 `xn--…`）。`mail.proxy` 里手写的
    代理地址本来就只收 ASCII 的主机名。
- **原来先后不定的地方定了先后**（邮箱助手）：`cca mail setup`、`cca mail forget default|account` 删掉没人再用的旧地址的应用专用密码，
  原来按集合的顺序，现在按地址排（实际一次最多删一个）。
- **`.claude.json` 里落单的代理项**读进来时换成 U+FFFD（见下面 PY_SRC 的第四条）。
- **原来先后不定的地方定了先后**：删了账号以后清掉没人再用的应用专用密码（`mail_forget_orphans`），原来按集合的顺序，现在按地址排。
- **文件名不是 UTF-8 的条目**（link）：列目录的地方跳过，整棵并走 / 拷走 / 删掉的地方报错停下；原来能处理这种名字（APFS 上建不出来）。
- **命令行参数里不是 UTF-8 的字节**原来原样带着走（surrogateescape），现在换成 U+FFFD；`CCA_STUN` 里非 ASCII 的主机名不做 IDNA 转换。
- **线程里出了没人接的错**（泄露检测的并行探测）原来只丢那一条线程，现在保证线程里不出这种错、改成往 stderr 说一句。
- **折合金额传给别的设备的密文，压缩出来的字节和原来不一样**（zlib 换成了 miniz_oxide：两个实现找重复的办法不同，长短也差
  一点）。对方照样解得开（解压的行为照 CPython 核对过，`tests/vectors/zlib.json`）；摘要算的是压缩前的内容，不受影响。小时很多、
  密文超过服务器上限时“去掉最早的四分之一”可能多去或少去一轮。
- **JSON 套得太深（超过 1000 层）当作写错了的 JSON。** 原来 Python 的解析器套得太深抛 RecursionError（不是 ValueError，
  多数地方没人接，命令直接出错退出；折合金额读会话记录时碰上这样一行，每次统计都会停在它那里），多深算太深随版本变
  （3.9 是 992 层起，3.12 以后一万多层）。现在读配置的地方当作读不了，读会话记录的地方跳过这一行。
- **信号只在会阻塞的地方起作用**（上报进程被 SIGTERM 停掉）：原来处理函数里抛的异常能从任何两步之间冒出来，现在等到下一次
  睡眠、等子进程、等 kqueue 时才看（正在跑的子进程照样马上杀掉）。上报进程几乎一直睡着，实际没有差别。
- **上报进程不再漏掉“看完账号目录、还没开始盯”那一小段里的变化。** 原来（`cmd_presence_agent` 的循环）先看各账号目录里的登记、
  报给服务器，然后才建 kqueue 盯着这些目录：会话的登记正好在这中间出现或者消失（多半是在报前一个变化的那几十毫秒里）就没人
  知道，要等下一次心跳（60 秒）才报——服务器上「在用」的会话数、「没在用」要晚一分钟。Python 版也有这个空当，只是 cca 自己慢，
  两个会话的登记很少挨得这么近；换成辅助程序以后 cca 快了，两个会话同时结束时后一个的登记常落在这一段里，回归 11b3c 的
  「会话都关了：马上报“没在用”」「再启动一次…报 2 个」因此时过时不过。现在去看之前先记下各目录的修改时间，开始盯着以后比一次，
  变了就马上回去再看（`dir_stamps`、`presence_wait` 的 `seen`；有单元测试）。只多看一眼，报的内容和原来一样。
- **Codex 登录（`login-flow`）时 Ctrl-C 正好落在探测 codex 参数的那一两秒里**（`codex_probe`，只有换了 codex 版本后的第一次才探测）：
  原来当场取消，app-server 还没起；现在等探测完、起了 app-server 才看到这次取消，随即杀掉它。说的话（「已取消登录」）和退出码一样。
- **查用量、登录的超时写成 `nan`**：原来是空转着一直等（见下面 usage / codex 一节），现在也一直等，但不空转。
- **没有 `#!` 开头的脚本当命令起**（macOS）：原来 Python 的 subprocess 起不来（`[Errno 8] Exec format error`）；Rust 的标准库起命令走
  execvp，macOS 的 execvp 遇到内核不认的文件会改交给 `/bin/sh` 当脚本跑。凡是起外部命令的地方都一样（`py::proc::Cmd`，和 usage / codex
  里自己起子进程的 `Piped`）：`claude`、`codex` 是一个没写 `#!` 的包装脚本时，原来 `cca usage` 说「查询失败：[Errno 8] Exec format
  error…」，现在这个脚本跑得起来（bash 启动账号本来就跑得了这种脚本）。不是有意改的，是 C 库的做法；要照原样得在 `py::proc` 里起命令
  之前先看文件头。macOS 上两边实跑核对过，没放进对拍（结果不一样）。Linux 的发布版用 musl，它的 execvp 没有改交给 sh 这一步
  （读它的源码），应该和原来一样报错，没在 Linux 上实跑。
- **agy 登录时授权码写不进 agy 的 stdin（它已经不读了）以后，新的 Python 退出时会在 stderr 上多说两行**
  （3.14 上是 `Exception ignored while finalizing file <_io.BufferedWriter name=5>:`、`BrokenPipeError: [Errno 32] Broken pipe`；
  照 Python 的更新说明是 3.13 起，实测 3.12 和 macOS 自带的 3.9.6 不说）。那是解释器收尾时的话，cca-core 不说。
  （`agy-login/agy-closes-stdin`，比之前把这两行去掉）
- **agy 探测读日志那一步没有照搬 `glob` 的全部规则。** 临时目录的路径里有成对的方括号时，原来读不到日志（见 login 一节），
  cca-core 也当作读不到；方括号那一截碰巧通配得上它自己（目录就叫 `[[]*` 这种）、原来其实读得到的情形不认。
- **原实现的结果随 Python 版本变的地方，cca-core 照 3.14 的**（对拍拿来对的是 3.14；macOS 上 cca 实际跑在系统自带的 3.9.6 上）。
  login 这一块拿 3.9.6 当基准另跑过一遍，566 个用例里 3 个不一样，都是平时碰不到的输入：登录网址开头有空格时，3.9.6 的
  `urlsplit` 不去掉它、认不出协议，`login-url` 原样打回、不补 `login_hint`（3.12、3.14 去掉）；`login-forward` 遇到分块的回应里
  块的大小是负数时，3.9.6 都是「连不上…IncompleteRead(0 bytes read)」，3.12、3.14 里 `-1` 是 `IncompleteRead(12 bytes read)`、
  别的负数是没人接的 ValueError。拿 3.12 当基准时 566 个都一样。（`login-url/leading-space-tabs-newline`、
  `login-forward/reply/chunked-negative-size`、`chunked-size-minus-one`）

## PY_SRC

- **两个死函数调用了不存在的函数。** `flow_get`、`fill_members`（`write_text` 后面）调用 `yaml_unquote`，全文件没有它的定义，
  调到就是 NameError；这两个函数自己也没有调用方。没有移植。（读代码看出来的）
- **两个子命令没有调用方。** `accounts-json`（`cmd_accounts_json`）、`file-replace`（`cmd_file_replace`）在 `CMDS` 里，
  但 cca、tests、macos、docs 里都没有调用它们的地方。照样移植了（免得“有这个名字却不能用”），确认不要了可以从 `CMDS` 里删掉。
- **config.json 里的 `last` 被改成数字、列表时，所有命令都是调用栈。** `reconcile` 里 `st.get('last') not in t`（列表、字典
  不能当键查）、`acct_id(st['last'], tool)`（拿数字当文字用）都是没人接的 TypeError；每条经 `ctx()` 的命令都过这里。只有
  手改坏了才会这样。照样移植（Rust 里是 panic）。（对拍 `rows/last-not-str` 看得到）
- **`Lock` 没有管住账号表和 config.json 的写入。** `sync.lock` 只在 device / link / sync / reorder / node-save / node-rm 等处拿，
  `ctx()` 里的 `cfg_save`、`cmd_add`、`cmd_rm`、`cmd_rename`、`cmd_edit_apply` 都是“读、改、整份写回”而不拿锁：
  两条命令同时跑（比如 App 的后台命令和终端里的命令）时，后写的会盖掉先写的改动。（读代码看出来的）
- **`.claude.json` 里有落单的代理项时会出错退出。** Claude Code（Node）截断带表情的文字时可能写出 `"\ud83d"` 这样不成对的
  半个字符；Python 读得进来，但 `save_json`（`ensure_ascii=False`）往 UTF-8 文件里写它时抛 UnicodeEncodeError。
  Rust 这边读进来时把它换成 U+FFFD，不会出错（有意和原实现不一样，见最前面一节）。（读代码看出来的）

## presence / keyshare

- **Linux 上的上报进程每 5 秒醒一次。** `presence_wait` 靠 kqueue 睡到“会话进程退出、账号目录里有变化”；Linux 没有 kqueue，
  退回 `time.sleep(min(timeout, 5))`：有会话开着的时候，每 5 秒读一遍 config.json、映射、各张账号表，再把 /proc 里所有进程
  扫一遍。照样移植了（Rust 版醒一次便宜得多）；要省可以在 Linux 上用 inotify 加 pidfd 做同样的等待。（读代码看出来的）
- **“还不到时候”也会把 keys.json 重写一遍。** `keyshare_sync` 里 `with KeyshareFile() as j:` 中间 `return None`（离上次同步
  不到 10 分钟）也算正常退出，`__exit__` 照样 `save_json`：内容没变也重写一次文件（文件坏了的时候是换成一份空的）。照样移植。
  （对拍 `presence/key-sync/when-not-yet` 看得到）
- **status.json 里不是对象时上报进程会崩。** `presence_status` 的 `(load_quiet(p) or {}).get('error', '')`：文件里是列表、
  字符串、数字时是没人接的 AttributeError，上报进程退出（下次启动 cca 时再起来，又崩）。只有手改坏了才会这样。照样移植
  （Rust 里是 panic）。（读代码看出来的）
- **keys.json 里某个账号那一项不是对象时，这个账号永远取不回服务器上的新 key。** `keyshare_sync` 开头只把是对象的项拷进
  `acc`，`keyshare_round` 里拿 `j['acc'].get(slot) != x['raw']`（raw 是 None）当成“这期间在这台设备上改过”，每一轮都跳过。
  只有手改坏了才会这样；在这台设备上重新记一次 key 就好了。照样移植。（读代码看出来的）

## cost

- **`scan.json` 每次统计都整份重写，哪怕什么都没变。** `cost_run` 里 `cost_save('scan.json', j)` 不看内容变没变（`local.json`
  是变了才写）。用得多的人这个文件有几 MB（35 天里每条回复一项去重记录），会话结束、`cca usage`、上报进程里最多 5 分钟
  统计一次，每次都读进来、再写出去。照样移植了；要省可以在没有新增记录、文件位置也没动时不写。（读代码看出来的；实测 10 万条
  回复时 scan.json 3.2 MB，空跑一次 Python 0.16 秒 / 58 MB，照样移植的 Rust 0.04 秒 / 39 MB，内存主要就是这张去重表）
- **没认出账号的 `session-owner` 行也会在 `owners` 里留一项空的。** `claude_line` 里 `self.j['owners'].setdefault(sid, [])` 在
  判断 `u` 之前：账号身份不合规矩（太短、不是字符串）的主会话，scan.json 的 owners 里有一个空列表。没有害处。照样移植。
  （对拍 `cost/scan/owners-and-subagents` 看得到）
- **名字以 `.jsonl` 结尾的目录、指向不存在的文件的链接也在“要读的会话记录”里**（`glob` 不看类型），打开时出错才跳过；
  主会话的那种还会先记进 `mains`。没有害处。照样移植（Rust 里打开目录不会出错，另外判断了一下）。

## bash

- **`eval "$(py env "$key")" || die …` 这类检查不生效。** `py` 失败时 `$(…)` 是空的，`eval ""` 返回 0，`|| die` 不会触发，
  bash 带着空的（或上一个账号的）`ACC_*` 接着往下跑；`py` 的报错还是会出现在 stderr。`local-env`、`router-env`、`tool-env`
  的 `eval` 同理。（读代码看出来的）

## 测试

- **`tests/usage_view.py` 里有一个偶发失败的测试。** `test_usage_cached_keeps_model_scoped` 用 `assertEqual(x['at'], self.now - 2)`
  比两个浮点数：一边是把时间换成毫秒整数再除回来的，末位会差一点（例：`1795065882.2942758 != 1795065882.294276`）。
  实测连跑 41 次碰到一次；整套回归里它算一个用例，所以回归大约每 40 轮会莫名挂一次（这次移植的第 4 轮混合版回归就碰上了）。
  改成 `assertAlmostEqual(…, places=3)` 就行。
- **`tests/router_merge.py` 没接进回归。** `regress.sh` 的 unit 一节跑 15 个 Python 单测文件，没有它（只在 docs/dev/router.md 里提到）。

## secrets

钥匙串和密钥文件（`keyfile_store` 到 `cmd_key_show`）。三条都照原样移植了，对拍用例在 `cli/pydiff/cases/secrets.py`。

- **接口地址的域名里有非 ASCII 字符时，macOS 上存 key 是没人接的 UnicodeEncodeError。** `ks_write` 把交给 `security -i` 的整句命令
  （里面有域名）`.encode('ascii')`；`url_problem` 不拦这种地址（`https://api.例子.com/v1` 校验通过），`kc_put` 只挡空的和带空白的。
  `py secret-set` 打印调用栈、退出码 1，而且这个账号原来的 key 已经在这之前删掉了（`kc_put` 先 `kc_del`）。`cca add api` 时看得到
  调用栈；`cca key 账号`（`api_key_prompt` 把 stderr 丢了）只说「钥匙串被锁住了？」。密钥文件那一支存得进去。
  用例：`secrets/secret-set/host-not-ascii`、`…/host-not-ascii-file`。
- **结尾带一个换行的 key 也算合格。** `SECRET_RE`（`^[\x21-\x7e]{8,4096}$`）用 `match`，`$` 在结尾那个 `\n` 之前也匹配。账号表的 `key`
  字段写成 `"sk-…\n"` 通得过校验，`api-key-out`（交给 Claude Code 的）、`key-show` 连换行原样打出来；密钥文件里的 key 同样
  （`keyfile_read` 不去空白，钥匙串那一支读出来会 `strip()`）。经 `secret-check`、`secret-set` 输入的先 `strip()`，不受影响。
  `KEY_RE`、`MODEL_RE`、`EMAIL_RE` 是同一种写法。用例：`secrets/api-key-out/table-key-newline`、`secrets/key-show/file-key-newline`。
- **`kc_put` 先删后存：存不进去时旧 key 已经没了。** 先 `kc_del`（钥匙串里的、密钥文件、config.json 的记录都删），再 `ks_write`。
  密钥文件那一支 `keyfile_write` 本来是「临时文件 + 原子替换」，前面这一删让它白做了：磁盘满了写不进去时，旧文件已经删掉。
  官方账号记下备查的那一把删了找不回来（控制台只在建的时候给一次全文）。钥匙串那一支 `add-generic-password -U` 本来也是原地换；
  先删只是为了清掉别的服务商（别的账户名）的条目。用例：`secrets/secret-set/locked`、`…/locked-official`（假钥匙串：删得掉、存不进）。
  （磁盘满的情形是读代码看出来的）

## link

设备标识、`.claude.json` 三方合并、共享目录链接这一节（`device_fields` 到 `cmd_link`）。没标「读代码看出来的」的都拿原版 Python 实跑过，
对拍里有对应的用例（`cli/pydiff/cases/link.py`，括号里是用例名里的一段）；Rust 这边照原样移植，行为一样。

- **同一秒里两个同名的条目移进 conflicts，后一个盖掉前一个。** `aside` 起的名字是「原名.秒数」，不查目的地在不在。agy 的顶层和
  `config/` 里各有一个 `GEMINI.md`、`AGENTS.md`：两处都是“账号目录里是文件、`~/.gemini` 里是目录”时，一次 `py link` 里先后 aside，
  第二次 `shutil.move` 直接改名盖掉第一次存的那份（顶层那份就没了），两句提示说的还是同一个路径。目的地是目录时（同名的两个目录）
  第二个会被搬进第一个里面。连着跑两次 link 落在同一秒里也一样。（实跑验证过；时间赶巧才对得上，没做成对拍用例）
- **账号目录里的共享配置成了空白文件、共享的那份没变时，共享的那份被换成空白，不留副本。** `config_json` 把空白当 `{}`
  （为了 agy 第一次运行建的 0 字节 `mcp_config.json`），于是“只有账号这份变了”这条路照常 `copy_over`：`~/.claude/settings.json`
  对所有账号变成空文件，conflicts 里什么都没有。客户端写到一半留下空文件、又把链接换掉了的时候会走到。共享的那份也变过的话是报冲突、
  留共享的，没事。（`config/json-blank-here`）
- **`~/.claude`（`~/.codex`、`~/.gemini`）里的共享配置不是 UTF-8 时，每次 `py link` 都出错退出。** 结尾记基线时 `shared_text` 读
  `CLAUDE.md`、`AGENTS.md`、`settings.json`……，UnicodeDecodeError 没人接（调用栈，退出码 1）。链接其实都做完了，只是基线没写；
  bash 那边是「初始化账号目录失败」「链接共享目录失败」，账号起不来。账号目录里那份不是 UTF-8 也一样（在 `merge_shared_config` 里）。
  （`shared-config-not-utf8-in-master`、`…-in-account`）
- **共享主目录里的条目是断掉的链接、账号目录里有同名的真实文件时出错退出。** `adopt` 看 `os.path.lexists(dst)` 为真、不是目录，
  就当两边都有文件去 `merge_file`：`filecmp.cmp` / `merge_lines` 里的 `open` 抛 FileNotFoundError，没人接。账号这边的原样留着，
  每次 link 都错在这里，直到有人把那个断链删掉。账号这边是目录时走 `aside`，没事。（`adopt/file-here-dangling-link-there`）
- **agy：账号目录里的 `antigravity-cli` 是链接时，设备标识经它写到链接指向的地方，下一层 `antigravity-cli/cache` 也照样经它建目录、放链接。**
  `prepare_device` 只查账号目录和配置目录本身是不是链接，`private_file` 只看 `installation_id` 这个文件本身；`link_levels` 的注释说
  “那一层是链接时这一层不动，免得经它写到别处”，但只跳过了这一层自己，`antigravity-cli/cache` 这一层的 `os.makedirs` 和
  `os.symlink` 还是穿过去了。两个账号的 `antigravity-cli` 指向同一处的话，会轮流把对方的 `installation_id` 改成自己的
  （这一句是读代码看出来的；写穿是实跑验证过的：`levels/nest-is-a-symlink-here`、`device/agy/nest-is-symlink`）
- **旧版留下的链接指向只读的非空目录时拆不开（macOS），每次 `py link` 都出错退出。** `detach_legacy_shared` 先在临时目录里拷一份
  （`copytree` 连权限一起拷），再 `os.replace` 搬出来：macOS 上没有写权限的目录搬不到别的目录下（EACCES）。链接恢复原样、临时目录收掉，
  然后是没人接的 PermissionError。只是里面的子目录只读不碍事。（`legacy/readonly-directory-cannot-be-detached`）
- **账号目录里要并回去的目录没有写权限时也一样（macOS）。** `adopt` → `shutil.move`：改名不成，空目录改成拷过去再删（成了），
  非空的 shutil 自己抛 PermissionError（Cannot move the non-empty directory…），没人接。（`adopt/readonly-…`）
- **`~/.claude.json`、基线文件不是 JSON 对象时是调用栈，不是一句说明。** `claude_sync` 里 `load_json(…) or {}` 之后直接 `.get`：
  内容是非空的数组、字符串时 AttributeError 没人接（`prepare_device` 对账号自己的 `.claude.json` 是有说明的：「账号配置必须是 JSON 对象」）。
  （`sync/claude/master-list`、`base-list`、`…-text`）
- **Codex 的 `installation_id` 是个目录时，每跑一次留下一个临时文件。** `prepare_device` 写好 `installation_id.cca-<进程号>.tmp` 再
  `os.replace`，换不上去（调用栈），临时文件留在账号目录里，名字带进程号所以越积越多。（`device/codex/installation-id-is-a-directory`）
- **bash 对 Codex、agy 账号也调 `py sync`，那边什么都不做。** `T.sync` 只有 Claude 有内容；启动前后各起一次 Python 白跑约 45 毫秒
  （bash 的 `prepare_account`、会话结束那几处）。（读代码看出来的）
- **比修改时间新旧用的是小数的秒。** `merge_file`、`adopt_task_lists` 比的是 `st_mtime`（float）：现在的时间戳上两个文件差不到
  约 240 纳秒就算一样新；最后一位还随平台变（arm64 上 CPython 把 `秒 + 1e-9 × 纳秒` 合成一次乘加，x86-64 上是先乘后加）。
  Rust 这边照 arm64 上实测的算法做了（`pylib.rs` 的 `st_mtime`）。不算错，记一笔：想要确定的结果该比 `st_mtime_ns`。

## leak

出口检测的解析和 DNS / WebRTC 泄露检测（`coffee_parse` 到 `cmd_leak_verdict`）。带着对拍用例名字的是实跑核实过的（cli/pydiff/cases/leak.py）。

- **`coffee` 的退出码 1 有两个意思。** 退出码 1 是“纯净度低于 min_trust”，可 Python 里没人接的异常也是退出码 1（这时 stdout 是空的）。
  bash 的 `coffee_check` 分不出来：照“出口纯净度偏低”问要不要继续，那一句里出口的信息是空的。触发：config.json 的 `min_trust`
  不是整数的写法（`50.5`、`"abc"`、`null`：`cfg_load` 只在没有这一项时补 50，不看类型），而且要拿到了评分、出口不在受限地区才会走到
  `int(min_trust)`（平时看不出来）；或者 iprisk 的 `countryCode`、`asOrganization` / `company_name` 是数字、列表这类不是文字的值。
  `health-save` 用的是同一个 `coffee_parse`，这时 health.json 也没写。（退出码和空的 stdout：coffee/min/abc、coffee/org/bad-number、
  coffee/cc/bad-number；bash 和 health-save 那一半是读代码看出来的）
- **评分是 `NaN` 时放行，布尔值也当评分。** `trust_score` 用 `isinstance(…, (int, float))` 认：`true` 当 1 分，那一行写的是
  「纯净度 True（可疑）」；`NaN`（Python 的 json 认这个写法）档位算“可疑”，但 `score < int(min_trust)` 是假，退出码 0，不提示偏低。
  （coffee/score/NaN、coffee/score/true）
- **受限地区只看 iprisk 的 `countryCode`，有它就不看 trace 的 `loc`。** `cc = d.get('countryCode') or loc`：claude.ai 的 trace 说出口在
  HK、ip.net.coffee 的库里是 US 时不算受限地区，退出码 0。两边的归属地对不上时该信谁，由维护者定（查用量前的 `coffee_check region`
  不带 iprisk，看的是 `loc`）。（coffee/restricted/loc-ignored-when-risk-has-cc）
- **归属地接口回答了一个没有地区代码的 JSON 对象时，按“查到了”记 7 天。** `leak_geo` 里回答只要是个字典就不带 `miss`，`cc` 是空的：
  ip.net.coffee 回 `{}`、`{"error": …}` 时（curl 没带 `-f`，HTTP 出错退出码也是 0），这个网段 7 天里一直是「解析器归属地不完整」/
  「归属地没查到」，不像连不上、不是 JSON 那样一小时后重查。（leak-probe/geo-empty-object）
- **「WebRTC 没测成」那一句点名的可能不是没查到归属地的那个地址。** `shown = (cn or leaked)[0]`：STUN 看到两个和出口不同的地址、
  第一个查到了（不是中国大陆）、第二个没查到时，说的是「STUN 看到 <第一个>，归属地没查到」。（leak-verdict/webrtc-unknown-one-of-two）
- **agy 的 WebRTC 说明里缺了 App 的名字。** `T.app_label` 对 agy 是空串：「尚未验证浏览器或  是否会暴露该 IP」，中间两个空格。
  （leak-verdict/webrtc-leak-agy、leak-verdict/webrtc-note-agy）
- **`IP_TEXT` 只看字符，结尾多一个换行也算。** `^[0-9A-Fa-f:.]{2,45}$` 配 `re.match`：`ab`、`203.0.113.53\n` 都算 IP，进了 servers，
  查归属地的网址里也带着那个换行（查不到，结论是没测成，不会当成没问题）。ip.net.coffee 的回答不对时才遇得到。
  （leak-probe/servers-hex-word-passes-for-an-ip、leak-probe/servers-trailing-newline）
- **`stun_probe` 说“错误都返回空”，有两种其实是线程里没人接的异常。** `socket.socket(af, …)` 在 `try` 外面（解析出了 IPv6 地址、
  内核却建不了 IPv6 套接字时）；`getaddrinfo` 把主机名按 idna 编码失败是 UnicodeError，不是 OSError（`CCA_STUN=a..b:9`）。结果一样是
  这一路没有地址，只是 stderr 上多一段调用栈（bash 把 leak-probe 的 stderr 丢掉了）。cca-core 这两种情况往 stderr 说一句，同样没有结果。
  （leak-probe/stun-host-python-cannot-encode；建不了套接字的那种是读代码看出来的）
- **macOS 上几个线程同时解析主机名，偶尔有一个卡住一秒以上。** `stun_ips` 一口气起 4 个线程（两个服务器 × IPv4 / IPv6）各自
  `getaddrinfo`。开发机（macOS 27.2.1）上实测：新进程里 4 个线程同时解析 127.0.0.1、localhost（各按 IPv4、IPv6），C 写的小程序
  6000 次里 8 次有一个 localhost 的解析超过 1 秒没回来（线程错开 300 微秒起还有 2 次；只有一个线程解析时 8000 次 0 次）；原来的
  Python（`CCA_STUN=127.0.0.1:端口,localhost:端口`）3000 次里 2 次，cca-core 4000 次里 2 次。`stun_ips` 的等待上限（超时 + 0.5 秒）
  兜得住，那一路按没测到算：偶尔少一个服务器或者少 IPv6 的结果，leak-probe 多花 1.5 秒。不是移植带来的；对拍里的假 STUN 因此只写数字地址。
- **`leak.json` 的读改写没有锁。** `leak-verdict` 读缓存、改、整份写回，不拿 `Lock`：两个账号同时启动（App 的后台检查和终端里的启动）时
  后写的盖掉先写的。只是缓存，丢了的下次重测。（读代码看出来的）

## settings

共享设置的检查、说明和修复（`cmd_settings_check` 到 `cmd_codex_fallback_off`）。下面除了注明的都是对拍时实跑出来的（用例在
`pydiff/cases/settings.py`）。

- **内嵌的 tomli（`TOMLI_EMBEDDED`、`tomli_fallback`）移植后用不着了。** Rust 这边是照 tomllib / tomli 2.3.0 逐个函数搬过来的解析器
  （`src/py/toml.rs`），不分 Python 版本；那段压缩的源码、加载它的函数、`tests/isolation_settings.py` 里核对它摘要和版本的测试都没有
  移植，切换以后可以从 `cca` 里删掉。解析器是照着它写的，它的 MIT 许可说明已经抄在 `src/py/toml.rs` 开头。拿内嵌的那份在 macOS
  自带的 Python 3.9.6 上跑了一遍 `tests/vectors/toml.json`：除了下面说的整数位数，结果（值、出错的说法和位置）和 3.14 的 tomllib 一样。
- **`--settings` 后面少了值时，说明说错了。** `launch-check` 给的是 `--settings:missing_value`，`settings_explain` 先按“启动参数给的设置”
  那一支处理，把 `missing_value` 当成了变量名：「启动参数 --settings 里设置了会换掉账号设置的变量（env.missing_value）。带着它启动的话
  账号会被换到别处……去掉它再启动。」别的参数少了值说的是「启动参数 --model 后面少了值。」触发：`cca @账号 --settings`。
- **共享的配置是指向别处的链接（dotfiles）时，又被当成项目配置查一遍。** `project_settings_problems` 靠
  `路径 != realpath(~/.claude/settings.json)` 跳过已经当共享设置查过的那份；它是链接时两边不相等，于是在主目录下面任何目录启动，
  除了 `settings.json:NO_PROXY` 还会多出一条 `/Users/…/.claude/settings.json:NO_PROXY`，说明里既说“这份设置所有账号共用……运行
  cca settings fix”，又说“项目设置 ~/.claude/settings.json……删掉这几项再启动”。Codex 更糟：共享的 config.toml 里本来允许开着
  `system_proxy_fallback`（启动时用 `--disable` 关掉），当成项目配置再查时不允许，于是 `~/.codex/config.toml` 是链接、里面写着
  `system_proxy_fallback = true` 的人，在主目录下面启动会被 `…/.codex/config.toml:features.system_proxy_fallback` 拦住。
- **`--settings` 给的文件碰巧叫 `…/.claude/settings.json` 时被说成项目设置。** `settings_explain` 按路径的样子（`PROJECT_SETTINGS`）认项目
  设置，于是说「在这个目录下启动时 Claude Code 也会读它……删掉这几项再启动」，其实它是启动参数给的、那个目录也不一定是当前目录。
- **agy 的共享设置是坏链接时当作没有。** `agy_settings_problems` 只接 `FileNotFoundError` 就返回空，没有像 `config_text` 那样再看
  `lexists`：`~/.gemini/antigravity-cli/settings.json` 是指向不存在的文件的链接时放行，Claude、Codex 的同样情况是拦住的
  （“坏链接……不能当成空配置”）。
- **`settings-fix` 的动作只认 `plan`，别的一律当 `apply`。** `py settings-fix check 摘要`、打错的动作名都会真的去改（摘要对得上的话）。
  bash 只传 plan / apply，现在碰不到。
- **`env` 不是对象时，同一份文件里的 `apiKeyHelper` 不报。** `claude_config_problems` 发现 `env` 类型不对就返回
  `…:env:invalid_config`，后面的 `apiKeyHelper` 不看了。照样拦得住启动，只是说明里少一项。
- **键名以换行结尾也算匹配，输出里会多出换行。** `SETTINGS_BAD`、`SETTINGS_KINDS`、`SETTINGS_MODEL_ENV` 这些正则以 `$` 结尾，Python 的 `$`
  在结尾的换行之前也匹配：`"HTTPS_PROXY\n"` 这样的键会报出来，`settings-check` 那一行、`settings-fix plan` 的说明中间就断成两行。
- **`re.I` 把 İ（U+0130）、ı（U+0131）当成 i。** `ANTHROPıC_BASE_URL` 这样的键也被当成 `ANTHROPIC_BASE_URL` 拦住、被 `settings fix`
  去掉（它并不是那个变量）。照样移植了；`regex` 库不这么认，这个模块里自己补了一下，别处用到 `re.I` 的正则（`ENV_RESERVED` 等）
  Rust 这边现在不认这两个字符。
- **套得太深、数字太长在哪里算读不了，原来取决于 Python 的版本和调用的路径。** 各处把 RecursionError 和格式不对一样当作读不了，
  但它在哪一层出现不固定：JSON 在 3.11 及以前大约套 990 层（和调用栈共用一千层的上限），3.12 起按 C 的栈算，这台机器上的 3.14 是
  27741 层；TOML 的行内数组、行内表、一层套一层的 profiles 看调用栈已经有几层（`settings-check` 里数组 494 层，`launch-check` 里共享的
  config.toml 493 层、`-c` 的值 495 层；3.9.6 上实测调用栈还要早一层到头——那时比较运算也占一层递归——`settings-check` 里就成了
  493 层；经 PY_BOOT 起的再早一层）；十进制整数超过 4300 位报错是 3.9.14 / 3.10.7 以后才有的（macOS 自带的 3.9.6 没有）。Rust 这边
  固定成：TOML 和 profiles 照 3.14 直接跑 PY_SRC 时的层数（每条路都对拍过上限两边），JSON 1000 层，整数 4300 位。
- **启动参数里有不是 UTF-8 的字节时（比如 GBK 终端里打的中文），`-c 键=值` 被说成 invalid_config。** Python 把这些字节读成落单的
  代理项（surrogateescape）；值没加引号时退回去拼 `键 = json.dumps(值)`，`json.dumps` 把它们写成 `\udcXX`，TOML 不收这种转义，于是
  `codex -c model_instructions=中文`（GBK）被 `-c:invalid_config` 拦住。Rust 这边参数在 main.rs 里已经换成 U+FFFD，这种值会放行（键照样
  查，`-c chatgpt_base_url=…` 还是拦得住）——和原实现不一样的一处，起因在参数的读法，不在这个模块。同样的原因，Linux 上名字不是
  UTF-8 的 `--settings` 文件、`-C` 目录 Rust 找不到（Python 找得到）。
- **`settings fix` 遇到落单的代理项。** settings.json 的字符串里有 `"\ud83d"` 这样不成对的半个字符时，Python 在备份之后、写回时抛
  UnicodeEncodeError（没人接：退出码 1，留下备份和写了一半的 `settings.json.cca-进程号.tmp`，原文件没动）；Rust 读进来时已经把它换成
  U+FFFD（见上面 PY_SRC 一节），会照常改完，写回去的那个字符也就成了 U+FFFD（原文在备份里）——说好“别的设置不动”，这里动了一个
  字符。两边手动跑过，没放进对拍（结果不一样）。
- **`_gui state` 里接 `SystemExit` 的那一句是死代码。** `cmd_gui_state` 调 `T.settings_problems()` 时写着
  `except SystemExit:   # 共享的设置文件格式不对（load_json 报错退出）`，现在三种工具的 `settings_problems` 都不经 `load_json`、不会退出，
  `'%s:unreadable' % T.settings_disp` 这个结果出不来了。Rust 这边 `Tool::settings_problems` 直接返回列表。（读代码看出来的）

## usage / codex

查用量、用量表格和 JSON、每周用量、重置额度、Codex 的 app-server 和登录（`cmd_usage_query` 到 `cmd_login_flow`）。都照原样移植了，
这一段里没有死代码。带着用例名字的是对拍时实跑出来的（`pydiff/cases/usage.py`、`cases/codex.py`，括号里是用例名里的一段）。

- **一个账号的用量里有一个离谱的数，整张表都出不来。** `cca usage` 的表格是全算完了才打印的，中途一个没人接的异常就是调用栈、
  退出码 1，别的账号也跟着看不到（`--json` 多半没事）。会这样的数：
  - 重置时间是个数字，而且按秒算过了公元 9999 年：本周那一列算「今天 / 明天 / 周几」时 `date.fromtimestamp` 装不下（ValueError）。
    `parse_ts` 把大于 1e11 的数当毫秒，所以要到微秒那么大（`1795075200000000`）才会这样——接口哪天把单位从秒换成微秒、纳秒就是它
    （window-resets-microseconds、window-resets-year-10000-as-ms）；
  - ISO 写法的月份是 00、13：`calendar.timegm` 报 ValueError，这个连 `--json`、`reset-plan`、菜单里的那一格（`usage_cell`）也过不去
    （window-resets-month-13、reset-plan/bad-date）；
  - 百分比、重置时间、还能重置几次是 `NaN` / `Infinity`（Python 的 json 认这种写法）：`round()`、`int()`、`time.localtime()` 报错
    （window-…-nan、notes-…-percent-nan、codex-reset-count-nan）。
  这些数进了缓存（usage.json、Claude Code 自己的 cachedUsageUtilization）以后，每次 `cca usage` 都出不来，直到查到新的把它盖掉。
- **重置时间写成秒、又大于 1e11（公元 5138 年以后）时被当成毫秒**，落在 1970 年代，显示「已重置」
  （window-resets-seconds-past-year-5138-read-as-ms）。只是 `parse_ts` 按大小猜单位的边界，现在碰不到。
- **账号目录里的 `.claude.json` 读不了时，`cca usage` 整个退出。** `T.plan` → `acct_json` → `load_json` 报「JSON 格式错误」退出：
  一个账号的文件写坏了（或者正好读在 Claude Code 写到一半的时候），所有账号的用量都不显示（cache-claude-json-broken）。
- **claude 关了 stdout、进程却没退出时，说的是「没有回应就退出了（退出码 None）」。** 等它 3 秒没等到，`p.returncode` 还是 None，
  原样写进了那句话（usage-query/claude-closes-stdout-but-lingers）。
- **stderr 的最后 800 个字节是按字节截的**（`err.read()[-800:]`），截在一个汉字中间时说明的开头是 `��`
  （usage-query/claude-stderr-tail-cuts-a-character；Codex 那边 `Rpc.tail` 一样）。
- **超时写成 `nan` 时空转着一直等。** `float('nan')` 是合法的；`Queue.get(timeout=nan)` 里「还剩多久」和 0 比不出大小，
  `Condition.wait(nan)` 又当成不等，于是占满一个核、没有回答就永远不超时（`CCA_USAGE_TIMEOUT=nan cca usage`）。写成 `inf`、大于
  9223372036 秒是没人接的 OverflowError，这时已经起来的 claude / app-server 没人收尾，等它自己发现 stdin 断了才退出
  （usage-query/claude-nan-timeout、…-inf-timeout、usage-query/codex-inf-timeout）。
- **app-server 的出错说明正好是 `timeout` / `exit` 时，被当成 cca 自己等超时了 / 它退出了。** `Rpc.failure` 拿 `err in ('timeout', 'exit')`
  认是不是自己这边的两种情况：服务器回 `{"error": {"message": "timeout"}}` 时原因成了 `timeout`，说明换成了 stderr 的最后一段
  （可能是空的）（usage-query/codex-error-error-says-timeout、…-says-exit）。
- **`reset-consume`：codex 回的 `outcome` 是列表、对象时是调用栈，app-server 没人收尾。** `RESET_OUTCOMES.get(列表)` 是 TypeError，
  `codex_rpc_once` 后面的 `r.close()` 走不到（reset-consume/outcome-list）。`usage-render`、`reset-plan`、`reset-done` 拿结果文件里的
  `why`、`outcome` 查表时同理（result-json-why-list、reset-done/outcome-list）；这些文件是 cca 自己写的，正常不会是列表。
- **`login-flow`：登录完成的通知里 `params` 不是对象时是调用栈**（`x.get` / `p.get` 的 AttributeError；login-flow/params-not-an-object）。
  登录网址不是文字时同理（login-flow/url-not-text）。这两种 app-server 都会先被杀掉（在 `finally` 里）。
- **`planDisplayName` 结尾带一个换行也算合格的名字。** `PLAN_NAME_RE` 用 `match`，`$` 在结尾的换行之前也匹配（和 secrets 一节的
  `SECRET_RE` 是同一种写法）：表格里那个账号的一行从套餐那一列断成两行，JSON 里的 `plan` 也带着换行
  （plan-display-name-trailing-newline）。
- **结果文件 `N.skip` 不是 UTF-8 时是调用栈。** `usage_result` 读它时只接 OSError；`N.json` 的同样情况是当作没拿到结果
  （result-skip-not-utf8）。skip 文件是 bash 写的，里面可能带着登录的邮箱、出口检测的说明。
- **没给账号时表格那一支是调用栈**（`max()` 的参数是空的；no-keys-table），`--json` 没事。bash 不会不带账号调 `usage-render`。
- **同一个账号在一次 `usage-render` 里出现两次、两次都查到了**（`cca usage 账号 账号`）：第二次记不记进每周用量，看两次存盘是不是
  落在同一毫秒里（`weeks_add` 拿「不比上次新」认同一次查询）。不碍事，只是这一种情况结果不固定，对拍里避开了。
- **缓存是很久很久以前的、重置时间在很久很久以后时，表格里是几十上百位的数。** `span`、`ago` 对多大的秒数都照算
  （「缓存（11574074074074074625547 天前）」、`fetchedAtMs` 写成 `-1e30`；cache-ours-fetched-at-hugely-negative）。只是说明没有上限的检查。

## corenet

内置代理这一块（`slot_orders` 到 `write_text`，加上 `cmd_select_lines`、`cmd_slot_rows`）。带着对拍用例名字的是拿原版 Python 实跑核实过的
（`cli/pydiff/cases/corenet.py`；解析的那几条另有 `cli/tests/vectors/corenet.json` 里的对照），Rust 这边照原样移植，行为一样。

对拍和对照数据用的是这台机器的 `python3`（3.14）。用户机器上跑 PY_SRC 的是 PATH 上的 `python3`，没另装 Python 的 Mac 上就是自带的
3.9.6：拿它当原版把这一块的 807 个用例又跑了一遍，799 个一样，不一样的 8 个都是标准库随版本变了的地方（整数的位数上限 6 个、
方括号里的 IPv4 1 个、映射的 IPv4 地址 1 个），下面各条里分别说了；对照数据 1126 条里两个版本有 12 条不一样（多出来的也是网址的
宽严）。Rust 这边都照 3.14。

自定义节点（`cca node add`）：

- **有几种写错的链接给的是调用栈，不是一句「这段配置认不出来」。** `cmd_node_check` 只接 `NodeParseError`，下面这些是别的异常
  （调用栈、退出码 1；终端里直接看到，App 的表单 `node_custom_form` 把 `2>&1` 的全文当成出错的原因显示）：
  `ss://` 后面整段是 base64、又解不开（`_link_ss` 里没有 `@` 的那一支，`_b64` 在 `try` 外面）；解得开、但冒号只在主机那一半
  （`ss://` + base64 的 `abc@host:8388`，`method, password = …` 拆不出两项）；vmess 的 `aid` 不是数字的写法（`int(j.get('aid') or 0)`
  没经 `_node_int`：`"abc"`、`[1]`、`Infinity`）；vless / trojan / hysteria2 / tuic 网址的端口写错、方括号不成对
  （`urlsplit` 和 `u.port` 自己抛 ValueError，轮不到 `_node_int` 说「端口 不是数字」）。
  （node-check/parse/ss-bad-base64、ss-whole-colon-only-in-host、vmess-aid-text、vmess-aid-list、vmess-aid-infinity、vless-port-word、
  vless-port-too-big、vless-bad-bracket）
- **网址写得不规矩的链接读不读得了，取决于 Python 的版本。** `urlsplit` 和 `.port` 这些年收紧过：自带的 3.9.6 上
  `vless://uuid@[1.2.3.4]:443`（方括号里是 IPv4）读成服务器 `1.2.3.4`，`vless://uuid@[200vl1:db8::1]:443`（方括号里不是地址）照收，
  `vless://uuid@v.example.com:٤٤٣`（端口是别的文字的数字）读成 443，`vless://u@h: 1`（端口前有空格）读成 1；3.14 上这四种都是没人接的
  ValueError（调用栈）。Rust 这边照 3.14（`py::url`）。
  （node-check/parse/vless-ipv4-in-brackets；对照数据里的 vless/025、mutated/113、vless/020、vless/056。两个版本各生成了一遍对照数据比的）
- **`proxies:` 下面的列表不缩进就读不了，报的是「缩进对不齐（第 3 行）」。** 手写的 YAML（`_yaml_mini`）只在下一行缩得更深时才当它是
  这个键的值：`proxies:` 换行顶格写 `- name: …`（Go 的 yaml 库写出来就是这样，合法的 YAML）时，`proxies` 成了空的，`- name` 被当成
  一个键，再往下的 `  type: …` 对不上缩进。只贴节点那一段（不带 `proxies:`）、或者列表缩进两格都读得了。
  （node-check/parse/yaml-list-not-indented-under-key）
- **不分大小写认链接开头时 ſ、İ、ı 也算，认出来以后报错说的是端口。** `(?i)^(vmess|…|tuic)://` 配 Python 的 `re`：`ſs://`、`tuıc://`、
  `hysterİa2://` 都匹配；`.lower()` 以后不是那几个名字里的任何一个，走到最后通用的那一支，`urlsplit` 切不出主机和端口，于是
  「端口 不是数字：None」。`link_parse` 最后那句「不支持的链接」因此走不到（1126 条对照里没有一条出这句）。
  （node-check/parse/scheme-long-s、scheme-dotless-i、scheme-dotted-capital-i）
- **YAML 里的键不是文字时，写出来的 JSON 有重复的键。** `_y_scalar` 把键也当标量认：`1: a` 和 `"1": b` 是两个键（整数和文字），
  `json.dumps` 都写成 `"1"`；`true: x` 和 `1: y` 则是同一个键。node-check 的输出里就有两个一样的键（node-save 读回来只留后一个）。
  mihomo 的字段没有这样的键，实际碰不到。（node-check/parse/flow-odd-keys、yaml-odd-keys）
- **整数超过 4300 位在哪里算读不了，取决于 Python 的版本。** `int()` 的位数上限是 3.9.14 / 3.10.7 以后才有的（macOS 自带的 3.9.6 没有，
  和 settings 一节说的是同一件事）：有上限时 `{…, n: 999…}`（4301 位）在 `_y_scalar` 里是没人接的 ValueError，JSON 里的在 `json.loads`
  里算格式不对。Rust 这边固定成有上限（节点解析里自己数位数；`py::json::loads` 现在没有这条上限，粘贴进来的、stdin 读的 JSON 在
  corenet 里另外补了，见下面）。（node-check/parse/flow-huge-int、yaml-huge-int、vmess-huge-int-is-not-json、node-testconfig/huge-int、
  node-save/stdin-huge-int、core-nodes-save/huge-int：拿 3.9.6 当原版跑时不一样的就是这 6 个，那边都读得进）
- **套得很深的也是调用栈。** 块写法的 YAML 是一层层递归着读的（`parse_block`），套到一千层上下是没人接的 RecursionError：实测 3.14 下
  最里面一行是 `k: 1` 时套 990 层还行，是 `k:` 时 991 层，是 `- a` 时 989 层；系统自带的 3.9 是 988、988、987。JSON 写的（整段、
  一行写完的值、vmess 链接里的）套得深时，3.9 在 `json.loads` 里 992 层起出错，3.14 读得进一万多层、到写出来（`json.dumps`）时
  才出错（列表 24981 层、对象 14695 层起）。正常的节点定义只有两三层。Rust 这边的层数见下面。
  （node-check/parse/yaml-nested-deep、yaml-nested-too-deep、yaml-nested-too-deep-list-inside；JSON 的层数是手动跑的，没放进对拍）
- **起名字时，节点缓存里有一项不像节点定义就是调用栈。** `node_name_free` 里 `{p['name'] for p in core_cache().get('proxies', [])}` 不像
  `core_defs` 那样先筛：`core/nodes.json` 的 proxies 里有不是对象的、对象里没有 name 的，TypeError / KeyError。同一份缓存别的子命令都读得了。
  `core-summary` 的 `names` 那一句同理（没有 name 的对象）。`core-nodes-save` 写之前筛过，所以要手改的文件才会这样。
  （node-check/name/cache-entry-is-text、cache-entry-has-no-name、cache-name-is-a-list，core-summary/object-without-name）
- **node-save 不看 name 是不是文字。** 名字是数字、null、true 也照存：可选节点里多出一个 `5`，`node_ids` 多一个键；而存进
  custom-nodes.json 的那一项，`core_custom` 读的时候又筛掉（名字不是文字），于是 `node ls` 看不到、`node rm` 说没有这个节点，可选节点里
  那个 `5` 没人清。它每次还把 custom-nodes.json 整份重写成 `{version, proxies}`：里面不像样的项、顶层别的字段都丢掉。bash 只把
  node-check 的输出交给它（名字一定是文字），现在碰不到。（node-save/name-number、name-null、name-true、drops-junk-entries、
  custom-extra-fields-dropped）

内置代理的状态和配置：

- **`cca local status` 把自定义节点也算成「补上维护者参考节点」。** `cmd_core_summary` 的 `extra` 是“用到的节点里不在路由器拷来的那些”，
  `cca node add` 加的也在里面：没有参考节点、只有自定义节点的机器上也会说「补上维护者参考节点 N 个」。
  （core-summary/custom-nodes-are-counted-as-fill、no-cache-but-fill-and-custom）
- **手改坏的 config.json、节点缓存：这一块的子命令多半是调用栈，其中 `local-env` 每次启动都跑。** `cfg_load` 对 JSON 写错有一句说明，对类型
  不对没有：`local`、`router` 写成文字（`.get` 不存在）；`nodes` 里混进列表（当不了键）、数字（`local-env` 的 `sorted` 比不了、
  `core-missing` / `core-summary` 的 `'、'.join` 拼不了）；`core/nodes.json` 的 `saved_at` 是文字、是三百多位的整数（减的时候
  OverflowError）（`core-stale` 出错：bash 的 `core_nodes_hook` 每次在家启动都调它，拿到的是空的，于是再也不会去重新拷节点定义）。
  （local-env/local-is-text、nodes-mixed-types、nodes-with-a-list，slot-rows/nodes-with-a-list，core-stale/time-is-text、
  time-is-too-big-for-a-float、router-is-text，core-missing/nodes-with-a-number，core-summary/time-is-text、time-is-minus-infinity；
  bash 那一半是读代码看出来的）
- **dialer-proxy 连成一千层上下的一串，用到它的子命令都是调用栈。** `core_pick` 里的 `need` 顺着 dialer-proxy 一层层递归，到 Python 的
  递归上限是没人接的 RecursionError。第几层随子命令（调用栈有多深）和 Python 的版本差一两层：实测 3.14 下 core-missing、
  core-summary、install-node proxy、udp-build 到 996 层还行，local-env、core-config、udp-env 到 995 层；系统自带的 3.9 各少一层。
  `core-nodes-save` 也经它，这样的节点定义存不下来（只建了 core 目录），所以正常拷来的缓存里不会有。Rust 这边固定成 996 层，见下面。
  （core-missing/dialer-chain-long、dialer-chain-too-long，local-env、core-config、core-summary、udp-env、install-node、
  core-nodes-save 各有同名的两条）
- **哪些地址算公网、算本机，取决于 Python 的版本，出站绑定跟着变。** `core_bind` 拿节点服务器里第一个 `is_global` 的 IPv4 去问路由，
  `core_dns` 用 `is_loopback` / `is_unspecified` / `is_global` 看系统的 DNS 到不到得了。macOS 自带的 3.9.6 和 3.14 的 `ipaddress`
  对这几种地址答得不一样（两个版本各问了一遍）：`192.0.0.8`、`2002::1`（3.9.6 算公网，3.14 不算），`::ffff:8.8.8.8`（3.9.6 不算公网，
  3.14 算），`::ffff:127.0.0.1`（3.14 才算本机），`::ffff:0.0.0.0`（3.14 才算没指定）。Rust 这边固定成 3.14 的（`py::ipaddr`；
  `is_unspecified` 兼容层没有，`corenet/bind.rs` 里照 3.14 写了一份）。实际的节点服务器、DNS 不会是这些地址。
  （local-env/tun-resolv-ipv6-on-tun 里有 `::ffff:127.0.0.1`、`::ffff:0.0.0.0`：3.14 下直接算到不了，拿 3.9.6 当原版跑时它多问了
  两次路由，这一条就不一样）
- **写到一半留下的临时文件带着节点密钥，备份时会被打包。** `cmd_core_config` 先写 `core/config.yaml.<进程号>.tmp`（600）再
  `os.replace`；`save_json` 写的是 `nodes.json.cca-<进程号>.tmp`、`custom-nodes.json.cca-<进程号>.tmp`。换不上去（`config.yaml` 成了
  目录：调用栈、退出码 1）或者正好在两步之间被杀掉时，临时文件留在 core 目录里，以后没有谁清（进程号每次不同，一次留一个）。
  `cca restore` 备份时排除的是 `core/nodes.json`、`core/custom-nodes.json`、`core/config.yaml` 这几个具体的名字，带后缀的临时文件
  不在其中，会进备份包——那里的注释说节点密钥不打包（policy 那边写的是 `policy/nodes.json*`，带着通配）。
  `save_json` 的临时文件还是先照默认权限建出来（644）、写完才改成 600：写到一半出错时留下的那半份是 644 的（core 目录是 700，
  别的用户进不来）。
  （`config.yaml` 是目录时两边都留下临时文件、退出码 1：手动两边各跑一遍比过，文件名带进程号，没放进对拍；排除规则拿这台机器的
  bsdtar 3.5.3 试过，`core/config.yaml.12345.tmp`、`core/nodes.json.tmp` 都进了包；`save_json` 写到一半出错的那种拿 node-save 实跑过
  ——名字里有落单的代理项，留下 644 的 `custom-nodes.json.cca-<进程号>.tmp`；“两步之间被杀掉”是读代码看出来的）
- **install-env 先打印一行再建目录，中途出错时 bash 拿到的是半份。** `cmd_install_env` 先 `sh(INSTALL_SHELL=…)`，再 `T.install_env(folder)`
  （建一次性的配置目录、写 settings.json），最后才打印 `INSTALL_ENV`。后一步出错（磁盘满、目录建不了）时 stdout 里只有
  `INSTALL_SHELL=bash`；bash 的 `eval "$(py install-env "$tmp")" || exit 5` 不会触发（见上面 bash 一节），接着就用没有
  `CLAUDE_CONFIG_DIR` 的环境（或者这个 shell 里上一次留下的 `INSTALL_ENV`）去跑官方安装脚本。
  （install-env/claude-folder-is-a-file 核实了“先打印一行再出错”；bash 那一半是读代码看出来的）

守着内核的进程（`cca _core-keep`）：

- **停内核的那几秒里再收到一次信号，内核就没人停了。** SIGTERM / SIGINT 的处理函数抛 `Stop`，主循环外面接住、在 `finally` 里
  `core_halt`（SIGTERM，等 3 秒，SIGKILL）。`finally` 里没有再挡信号：这时来的第二个信号又抛一次 `Stop`，没人接——调用栈、退出码 1，
  不等内核退出、也不补 SIGKILL。内核退得慢（或者不理 SIGTERM）时它就留在后台（在自己的会话里），占着端口池的端口。
  配置撤了以后正在停内核的那一次（`try` 里面的 `core_halt`）被打断倒是接得住，会从头再停一遍。
  （core-keep/second-sigterm-while-stopping-the-kernel、sigterm-while-stopping-an-unwanted-kernel；什么时候会连着来两个信号——
  连着 `kill` 两次、关机时和 `core_retire` 赶在一起——是读代码推想的）
- **信号正好落在起内核的那一瞬间，起来的内核也没人停。** `subprocess.Popen(…)` 里面已经 fork、还没返回时来的信号，`Stop` 从 `Popen`
  里抛出来，`child` 还没赋值，`finally` 看到的是 `None`。窗口很小。Rust 这边只在等待的地方看信号，起来的内核一定记下了——这一种
  没有照原样（照不出来）。（读代码看出来的）
- **起不了内核时直接出错退出，不重试。** `Popen` 的 OSError（`CCA_SELF` 指的 cca 不在了、没有执行权限）没人接：调用栈、退出码 1，
  留下一个空的 core.log。说明里写的是“退出了就隔一会儿重启”，起不来不在其中；crontab 里 `@reboot` 起的那一个就此不再守着，
  要等下次在外面启动账号时 bash 再起。（core-keep/kernel-missing、kernel-not-executable、kernel-is-a-directory）

这一块里和原实现不一样的地方（都是正常使用碰不到的）：

- **stdin 里不是 UTF-8 的字节**（node-check、node-save、node-testconfig、core-nodes-save）原来原样带着走（surrogateescape；node-check
  原样打印出来，node-save 写文件时是没人接的 UnicodeEncodeError），Rust 这边读进来就换成 U+FFFD。没放进对拍。
- **粘贴的、stdin 读的 JSON 里落单的代理项**（`"\ud83d"`，不成对的半个表情；vmess 链接里的、块写法里一行写完的值也经 `json.loads`）：
  原来读得进，往外写的时候是没人接的 UnicodeEncodeError——node-check、node-testconfig 在打印时，node-save 在写 custom-nodes.json 时
  （什么都没存下，留下半份临时文件）。Rust 这边 `py::json` 把它读成 U+FFFD（最上面说的兼容层那一条），照常往下走：node-check 打印出
  带 U+FFFD 的节点，node-save 存得下来。手动两边各跑过，没放进对拍；对照数据里结果带落单代理项的也没收（写不成 UTF-8 的文件）。
- **node-save 的名字是数字、而 `node_ids` 里已经有同样写法的键**（名字 `5`、原来有 `"5"`）：原来一份文件里写出两个 `"5"`，Rust 的字典
  放不下，只有一个。没放进对拍。
- **`core/nodes.json`、`custom-nodes.json`、root 那边的 `info.json` 里超过 4300 位的整数**：原来整份算读不了（`load_quiet` 当作没有），
  Rust 这边读得进来（`py::json::loads` 没有这条上限）。粘贴的、stdin 读的 JSON 已经在 corenet 里补上了上限；文件这一半等兼容层。
- **dialer-proxy 连成的串正好在递归上限边上**（994 到 997 层）：原来第几层出错随子命令和 Python 的版本不一样（见上面），Rust 这边
  一律 996 层还行、997 层出错（`PICK_DEPTH`）。对拍只比离界线远的（980 层、1010 层）。
- **块写法的 YAML 正好套在递归上限边上**（987 到 992 层）：同上，Rust 这边一律套 990 层还行、991 层出错（`BLOCK_DEPTH`，和 3.14 下
  最里面是 `k: 1` 的一样）。对拍只比离界线远的（960 层、1020 层）。
- **套了上千层的 JSON**（粘贴的、stdin 读的、文件里的）：原来 3.9 是 992 层起 RecursionError，3.14 一万多层才出错（见上面）；
  `py::json` 没有层数的上限，开发版实测读得进 5000 层（对象）、8000 层（列表），再深（8000 / 12000 层）是栈溢出（进程被
  SIGABRT 结束，不是退出码 1）。这是兼容层的事，corenet 里没另外补。
- **core-keep 起内核那一瞬间的信号**：见上面。

## login

登录的辅助（`login_hint_url` 到 `cmd_login_catch`）。带着用例名的都是对拍时拿原版 Python 实跑出来的（`cli/pydiff/cases/login.py`），
Rust 这边照原样移植，行为一样。

- **login-forward 转不出去时把验证码打到了 stderr。** 回调的路径（登录网址里 `redirect_uri` 的路径，解码以后的）里有空格或控制字符时，
  `http.client` 不肯发（InvalidURL），它的说法里带着整个请求网址，`die('连不上…：%s' % e)` 原样打出来：
  `URL can't contain control characters. '/a b?code=…&state=…' (found at least ' ')`。验证码和 state 是一次性的凭证，别处都不打印。
  路径里有非 ASCII 字符时是没人接的 UnicodeEncodeError。claude / codex 给的回调路径是 `/callback`、`/auth/callback`，平时碰不到。
  （`login-forward/path-with-space`、`path-with-control-character`、`path-not-ascii`）
- **login-catch 拿到的 code 里有非 ASCII 字符时是没人接的异常，浏览器那边连回应都没有。** 交给 pbcopy 之前的 `text.encode('ascii')`
  抛 UnicodeEncodeError，外面只接 OSError / SubprocessError：终端上是调用栈、退出码 1，浏览器的连接直接被关掉（不是「没有登录成功」
  那一页）。（`login-catch/code-not-ascii`、`code-raw-bytes-not-ascii`）
- **login-catch 一次只管一个连接，一个不发请求的连接能让它停 10 秒。** 接下来的连接 `settimeout(10)` 以后才读，循环是单线的：
  浏览器先开了一个不发东西的连接（预连接）的话，带着验证码的那一个要在后面排 10 秒。（`login-catch/slow-request-is-dropped` 走的就是
  这条路；浏览器实际会不会这样是读代码看出来的）
- **login-catch 到点时说的分钟数是 `秒数 // 60`，超时的参数是开了浏览器以后才看的。** 不到一分钟的说「等了 0 分钟还没登录完」
  （回归里的 `CCA_LOGIN_WAIT=20` 就是），负数是向下取整（`-90` 是 -2 分钟）。参数写错了（`abc`、`nan`、`-inf`）时浏览器已经打开，
  然后才是调用栈；端口被占着（退出码 4）时根本不看它。（`login-catch/timeout/…`、`port-busy-not-a-number-timeout-never-read`）
- **login-forward：回调的回答既没说正文多长、也不关连接时，要等满 120 秒再报「连不上」——其实验证码已经送到了。** `r.read(65536)`
  在没有 `Content-Length`、也不是分块时一直读到对方关连接。Claude Code、Codex 现在的回答都带长度。
  （`login-forward/slow/headers-then-silence`：要等两分钟，`CCA_PYDIFF_SLOW=1` 时才跑）
- **agy 第二次打出登录网址时，网址原样到了 stderr。** `show` 只接第一个网址（`st['opened']`），之后带着网址的行落到最后一个分支，
  照“别的输出”转出去；docs/dev/accounts.md 说的是 agy 的输出由 cca 转述、不原样打出来。agy 换不到 token、重新给一个网址时会走到。
  （`agy-login/url-twice`）
- **agy 探测：临时目录的路径里有成对的方括号时认成“不支持”。** `glob.glob(os.path.join(g, 'antigravity-cli', 'log', '*.log'))`
  把整个路径当通配符看，`TMPDIR=/x/t[x]mp` 这样的目录对不上它自己，日志读不到，`agy_probe` 记下 `files: false`，bash 的 `agy_guard`
  就用「这个版本的 agy 不再把登录存进账号自己的目录」把登录和每次启动都挡住（缓存着，换了 TMPDIR 也不重测，要等 agy 的程序文件变了）。
  该先 `glob.escape`。另外各份日志是接成一串再找那一句的：前一份的结尾和后一份的开头凑成那一句也算数。
  （`agy-probe/tmpdir-with-brackets`、`said-across-two-files`）
- **以 `$` 结尾的几个正则，结尾多一个换行也算**（同 secrets 一节的第二条）。`NODE_ID_RE`：`--node-id` 后面带着换行时格式算对，
  比的时候又不等，于是说「不是同一个节点」。`LOGIN_CODE_RE`：以换行结尾的 state、验证码也收，`login-forward` 把那个换行编成 `%0A`
  转过去。（`login-page-acct/id-with-trailing-newline`、`login-forward/ok-code-ends-with-newline`、`login-check/…/state-trailing-newline`）
- **`re.I` 把 İ、ı 当成 i**（同 settings 一节的那一条）：`AGY_CODE_PROMPT`、`agy_probe` 里筛环境变量的 `drop` 也是。照样移植了
  （这两个正则里写明了）。（`agy-login/prompt-other-case-and-own-line`，筛环境变量的在 `login/agy.rs` 的单元测试里）
- **`node_ids` 里记的不是字符串时原样往外给。** `node_fp_here` 拿到什么返回什么：config.json 里被改成数字时 `node-fp` 打出 `123`，
  服务器上给出的那一行就是 `--node-id 123`（到了 Mac 上 `login-page-acct` 说它格式不对）。
  （`node-fp/recorded-number`、`agy-login/remote-node-id-not-a-string`）
- **`login-page-acct` 把 config.json 的 `nodes` 直接当列表用。** 被手改成字符串时按字符遍历、按“是不是它的一段”判断
  （`"nodes": "US-A,HK-A"`、服务器上的节点叫 `A`，结果是 `LP_NODE=A`）；是 `null`、数字时是调用栈。
  （`login-page-acct/nodes-string/A`、`nodes-null/…`、`nodes-number/…`）

## mail

邮箱助手（`mail_knob` 到 `cmd_mail_link_check`）。下面各条都照原样移植了；对拍用例在 `cli/pydiff/cases/mail.py`（假的 Gmail、代理、
钥匙串、浏览器），解析和盯邮箱的对照数据在 `cli/tests/vectors/mail.json`。

- **一封别人发来的邮件就能让登录时的自动打开停掉，让 `cca mail test` 和 App 的邮件列表出不来。** `mail_addrs` 解析 From / To 时，
  地址里的注释、组一层套一层（`From: ((((…` 几百层）到了 Python 调用栈的上限就是 RecursionError，哪儿都没接：
  - `mail_hunt`（登录时盯邮箱、`cca mail open`）：`mail_judge` 先看时间和标题、再解析地址，这两项都由发件人定——登录的那十几分钟里
    来一封标题写成 `Your secure link to Claude is here` 的这种邮件，`cmd_mail_watch` 接住后说一句「邮箱助手出错了（RecursionError），
    这次不看邮箱」就退出，这次登录不再自动打开；`cca mail open` 是调用栈、退出码 1。
  - `cca mail test`、`_gui mail-list`：每一封搜出来的（FROM anthropic.com 或 SUBJECT "secure link"）都先解析 To / From 再核对，
    最近这些小时里有一封这种邮件，整个命令就是调用栈（`_gui mail-list` 连 JSON 都不出），直到它过了时间。
  用例：`mail/watch/hostile-mail-is-an-error-said-once`、`mail/test/deep-comment-is-an-uncaught-error`，对照数据 hunt 一节的 hostile 两条。
  同一类的还有全文里 multipart / message 套上千层（`mail_parse`；头通过了核对才取全文，别人发不出来）。
- **「套几层出错」每条路不一样，同一条路上头一次和后面的也不一样。** 在 CPython 3.14.0 上直接跑 PY_SRC 实测（注释 / 组最多几层）：
  `cmd_mail_test`、`cmd_gui_mail_list` 里直接解析收件人 496 / 992（这个进程里头一次解析地址），紧接着解析发件人 495 / 991；
  经 `mail_judge_any → mail_judge` 再解析一遍少一层注释、两层组；`cca mail open`（经 `mail_hunt`）头一封的发件人 494 / 989，登录时
  （多隔一层 `mail_watch`）494 / 988；头通过核对以后取全文再核对一遍又深一层，好的登录邮件发件人实际只能套 493 层。「头一次多一层」
  是解释器的事：3.11 起执行过的调用会换成专门的指令，`getaddresses`、`parseaddr` 里 `_AddressList(…)` 这种「建对象、进 `__init__`」的
  换了以后多占一层调用栈。邮件结构是每多隔一层函数少一层：直接调 `mail_parse` 984 层，`cca mail test` 取全文 983，`cca mail open`
  981，登录时 980。数字本身无所谓（都远超正常邮件），记下来是因为它说明这个上限不是谁定的，是碰巧。Rust 这边照这个规律算
  （`cli/src/cmd/mail.rs` 的 `py_frame`），用例 `mail/test/nested-…`、`mail/gui-list/nested-…`、`mail/open/nested-…`、`mail/open/msg-nested-…`、
  `mail/watch/nested-…` 是各条路上限的两边。
- **另外几处没人接的异常**（都照样移植成同一类错）：
  - 全文里 `Content-Type` 的参数续行号（`x*999…9*=`）超过 4300 位、`charset` 里有 NUL：ValueError（`mail_links`；只有头已经通过核对的
    邮件才取全文，要 Anthropic 签过名的邮件才走得到）。
  - Gmail 的回应里 INTERNALDATE 的年份是 0000（`calendar.timegm`）、UID / UIDVALIDITY / literal 的大小超过 4300 位（`int()`）：ValueError；
    CAPABILITY 里有非 ASCII 字节：UnicodeDecodeError。只有 Gmail 自己（TLS 里面）发得出来。4300 位的上限是 3.9.14 / 3.10.7 以后的
    Python 才有的，macOS 自带的 3.9.6 上这几处不出错。
  - `cca mail open --msg` 的邮件夹名里有非 ASCII 字符：UnicodeEncodeError（imaplib 把参数按 ASCII 编码）。用例 `mail/open/msg-folder-not-ascii`。
  - 连接方式写成 `http://a..b:8080` 这种主机名里有空的一段的：`socket.create_connection` 里 idna 编码出的 UnicodeError，不是 OSError，
    `mail_dial` 不接。用例 `mail/setup/route-empty-label`。
  - config.json 的 `ports`（或某个工具的 `ports`）、`router` 不是对象：`mail_route_exit` 里 AttributeError（`cca mail setup`、`mail ready`
    是调用栈；`mail-watch` 说「出错了（AttributeError）」）。用例 `mail/ready/ports-not-a-dict`、`mail/watch/ports-not-a-dict-is-an-error-said-once`。
  - `~/.cca/mail` 是个文件：`mail_used` 里 `os.makedirs` 的 FileExistsError（`cca mail open` 是调用栈，而且在登录好、取到邮件之后才出）。
    用例 `mail/open/mail-dir-is-a-file`、`mail/watch/mail-dir-is-a-file`。
- **`MailBox.cmd` 里「和 Gmail 的连接断了」那一句走不到。** `folders`、`examine`、`search`、`headers`、`message` 都写成
  `self.cmd(self.im.list, …)`：会话已经关掉（`self.im` 是 None）时，先求值 `self.im.list` 就是 AttributeError，进不到 `cmd` 里的
  `if self.im is None: raise MailRetry`。现在各处出了错都不再接着用同一个会话（`MailSource` 每次先看 `im is None` 再重连），实际碰不到；
  哪天有人在出错以后接着调，得到的是没人接的 AttributeError 而不是「过会儿再试」。对照数据 imap 一节里 `"py": "AttributeError"` 的那些。
- **`mail_judge_any` 把「太旧」报成「标题不像」。** `best` 从 `(rules[0], 'subject')` 起，只有排得更靠后的原因才换掉它，而 `old` 在
  `MAIL_ORDER` 里排第一：一条规则都没过、原因是 `old` 时返回的是 `subject`。现在看不出差别（`mail_hunt` 里这两个原因都不说，
  `cca mail open` 数「另有几封没通过核对」时两个都不算，`cca mail test`、`_gui mail-list` 事先按时间筛过）；哪天要按原因分开说就错了。
  对照数据 hunt 一节的「old mail is ignored quietly」。
- **`cca mail setup --account` 重新配置时，这个账号登记的信任中转（forwarder）没了。** 新的覆盖只从旧的带回 `auto: false`，`proxy` 按这次的
  算，`forwarder` 不带。是有意（不安全的开关不悄悄留着）还是漏了，看不出来；App 里换一个 Gmail 以后「信任转发中转」会变回关。
  用例 `mail/setup/account-keeps-auto-off-drops-forwarder`。
- **`_gui mail-list 账号` 对没写邮箱的账号不按账号筛。** `want = parse_key(acct)[1].lower()` 是空串，`if want and tgt != want` 不成立：
  列出来的是默认 Gmail 里所有账号的登录邮件（`account`、`key` 还是这个账号）。用例 `mail/gui-list/alias-only-is-not-scoped`。
- **`cca mail status` 里名字长的那一行，名字和后面的内容粘在一起。** `'  %s%s%s' % (k, ' ' * (10 - dw(k)), v)`，`账号 ` 加名字超过 10 格时
  一个空格都不补：`账号 bob@example.cominbox@example.com（…）`。只写邮箱、没起别名的账号都会这样。用例 `mail/status/long-names`。
- **IMAP 命令的参数原样拼进去。** imaplib（3.9、3.14 都是）只给 LOGIN 的密码加引号，别的参数不加引号、不查控制字符：
  `cca mail open --msg` 的邮件夹名直接进了 `EXAMINE 名字`——名字里有空格而没带引号的会被 Gmail 当成两个参数，带 CR LF 的能在后面
  夹带别的命令。这个值是命令行上给的（App 传回的是 LIST 给出的带引号的名字），不是邮件里来的，所以只算「只读」的保证有个缺口：
  原来「没有通用的命令入口」这句话对 `--msg` 不完全成立。Rust 这边照样原样发；用例 `mail/open/msg-folder-with-crlf`（假 Gmail 的
  日志里多出一条 `ZZZZ9 NOOP`）、`mail/open/msg-folder-with-space`。LOGIN 的用户名同样不加引号（`MAIL_ADDR_RE` 允许的 `%` 按 IMAP
  的写法不能出现在不带引号的参数里；Gmail 的地址里没有它——这一句是读代码看出来的）。
- **取不到全文的那一封会一直占着。** `mail_take` 里 `src.full` 出错（Gmail 说没有这封、连接断了）时 `pend` 不清，之后每一轮都只重试它，
  直到盯完；这期间来的更旧的邮件不看（更新的会换掉它）。注释里写的是「取不到全文的那封照连不上退避」，是有意的；记一笔是因为
  一封被删掉的候选邮件会让这次 `cca mail open` 以「Gmail 没给出这封邮件」收场，而不是去开第二新的那封。对照数据 hunt 一节的
  「mail that cannot be fetched backs off」。

