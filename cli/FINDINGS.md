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
- **`.claude.json` 里落单的代理项**读进来时换成 U+FFFD（见下面 PY_SRC 的第四条）。
- **原来先后不定的地方定了先后**：删了账号以后清掉没人再用的应用专用密码（`mail_forget_orphans`），原来按集合的顺序，现在按地址排。
- **文件名不是 UTF-8 的条目**（link）：列目录的地方跳过，整棵并走 / 拷走 / 删掉的地方报错停下；原来能处理这种名字（APFS 上建不出来）。
- **命令行参数里不是 UTF-8 的字节**原来原样带着走（surrogateescape），现在换成 U+FFFD；`CCA_STUN` 里非 ASCII 的主机名不做 IDNA 转换。
- **线程里出了没人接的错**（泄露检测的并行探测）原来只丢那一条线程，现在保证线程里不出这种错、改成往 stderr 说一句。
- **折合金额传给别的设备的密文，压缩出来的字节和原来不一样**（zlib 换成了 miniz_oxide：两个实现找重复的办法不同，长短也差
  一点）。对方照样解得开（解压的行为照 CPython 核对过，`tests/vectors/zlib.json`）；摘要算的是压缩前的内容，不受影响。小时很多、
  密文超过服务器上限时“去掉最早的四分之一”可能多去或少去一轮。
- **JSON 套得太深（超过 512 层）当作写错了的 JSON。** 原来 Python 的解析器到一千层上下抛 RecursionError（不是 ValueError，
  多数地方没人接，命令直接出错退出；折合金额读会话记录时碰上这样一行，每次统计都会停在它那里）。现在读配置的地方当作读不了，
  读会话记录的地方跳过这一行。
- **信号只在会阻塞的地方起作用**（上报进程被 SIGTERM 停掉）：原来处理函数里抛的异常能从任何两步之间冒出来，现在等到下一次
  睡眠、等子进程、等 kqueue 时才看（正在跑的子进程照样马上杀掉）。上报进程几乎一直睡着，实际没有差别。

## PY_SRC

- **两个死函数调用了不存在的函数。** `flow_get`、`fill_members`（`write_text` 后面）调用 `yaml_unquote`，全文件没有它的定义，
  调到就是 NameError；这两个函数自己也没有调用方。没有移植。（读代码看出来的）
- **两个子命令没有调用方。** `accounts-json`（`cmd_accounts_json`）、`file-replace`（`cmd_file_replace`）在 `CMDS` 里，
  但 cca、tests、macos、docs 里都没有调用它们的地方。照样移植了（免得“有这个名字却不能用”），确认不要了可以从 `CMDS` 里删掉。
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
