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
