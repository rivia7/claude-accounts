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
