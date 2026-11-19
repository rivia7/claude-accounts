# 移植时在原实现里发现的问题

移植照原样做（见 PORTING.md），这里只记下来，要不要修、怎么修由维护者定，在主干上修了再跟着移植。
没有实跑验证的标着「读代码看出来的」。

## PY_SRC

- **两个死函数调用了不存在的函数。** `flow_get`、`fill_members`（`write_text` 后面）调用 `yaml_unquote`，全文件没有它的定义，
  调到就是 NameError；这两个函数自己也没有调用方。没有移植。（读代码看出来的）
- **两个子命令没有调用方。** `accounts-json`（`cmd_accounts_json`）、`file-replace`（`cmd_file_replace`）在 `CMDS` 里，
  但 cca、tests、macos、docs 里都没有调用它们的地方。先不移植，确认不要了可以从 `CMDS` 里删掉。
- **`Lock` 没有管住账号表和 config.json 的写入。** `sync.lock` 只在 device / link / sync / reorder / node-save / node-rm 等处拿，
  `ctx()` 里的 `cfg_save`、`cmd_add`、`cmd_rm`、`cmd_rename`、`cmd_edit_apply` 都是“读、改、整份写回”而不拿锁：
  两条命令同时跑（比如 App 的后台命令和终端里的命令）时，后写的会盖掉先写的改动。（读代码看出来的）
- **`.claude.json` 里有落单的代理项时会出错退出。** Claude Code（Node）截断带表情的文字时可能写出 `"\ud83d"` 这样不成对的
  半个字符；Python 读得进来，但 `save_json`（`ensure_ascii=False`）往 UTF-8 文件里写它时抛 UnicodeEncodeError。
  Rust 这边读进来时把它换成 U+FFFD，不会出错——这是唯一一处有意和原实现不一样的地方。（读代码看出来的）

## bash

- **`eval "$(py env "$key")" || die …` 这类检查不生效。** `py` 失败时 `$(…)` 是空的，`eval ""` 返回 0，`|| die` 不会触发，
  bash 带着空的（或上一个账号的）`ACC_*` 接着往下跑；`py` 的报错还是会出现在 stderr。`local-env`、`router-env`、`tool-env`
  的 `eval` 同理。（读代码看出来的）

## 测试

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
