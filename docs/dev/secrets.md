# 密钥：钥匙串、密钥文件和 API key

共享记下的 API key（keyshare）经设备统计的服务器同步，见 presence.md。

- 远程会话配对的密钥（remote.md）：两台机器配对时各存一把（32 个字节，存成 64 个十六进制字符），服务名 `cca-remote-<配对编号>`、账户名 `pair`，
  同样经 `ks_read` / `ks_write` / `ks_del`（没有钥匙串的系统是 `~/.cca/secrets/cca-remote-<配对编号>`）。只有辅助程序用（握手、给会话的说明加密），
  不交给任何客户端、不进命令行和环境变量；配对码也一样（`cca remote join` 从 stdin 交给辅助程序）。取消配对、`cca restore`（`py remote-forget`）时删掉。
  配对表 `~/.cca/remote/peers.json` 里没有密钥

- 密钥绝不进任何进程的命令行和环境变量：存钥匙串用 `security -i` 从 stdin 读命令、`-X` 十六进制（`-i` 里的命令失败也返回 0，必须读回来比对；只收可见 ASCII，否则 `-w` 输出十六进制）；交给 Claude Code 用 `--settings` 里的 `apiKeyHelper`（回调 `cca _api-key <账号目录名>`，只给账号表里现有的账号），地址和模型也放在这份 `--settings` 的 env 里（优先级高于共享 settings.json 和进程环境；第二个 `--settings` 会把它整个顶掉）。env 会出现在命令行上，所以像密钥的变量名（`ENV_SECRETISH`）不收
- 钥匙串里第三方 API 的 key：服务名 `cca-api-<账号目录名>`，账户名是接口的域名（换服务商就取不到旧 key）。代码里叫 secret（`secret-*` 子命令），免得和账号表的"键"混了。cca 存过的记在 config.json 的 `api_keys`，`secrets-forget` 据此清掉已经不在账号表里的（`cca edit` 之后、每次启动、`cca restore` 时全部清）
- Linux 上没有钥匙串（`keyfile_store`：不是 macOS，或者 `CCA_KEYSTORE=file`，只给测试）：key 存成 `~/.cca/secrets/<服务名>`（目录 700、文件 600，内容 `{"v":1,"host":域名,"key":…}`，域名对不上算没有，和钥匙串的账户名一个意思；`O_EXCL|O_NOFOLLOW` 建临时文件、fsync 后原子替换）。服务名、`api_keys` / `ref_keys` / `key_hints` 的记账和钥匙串一样，只在 `kc_read` / `kc_put` / `kc_del` 里分开，退出码照 `security` 的（没有 44、读不了 1）。文案用 bash 的 `${KEYSTORE}`（钥匙串 / 密钥文件）、`${KEYSTORE_AT}`、`${KEYSTORE_CRED}` 和 辅助程序的 `keystore_name()`，失败的说明按 `keyfile_store` 分开（文件的说权限和磁盘空间）。Claude Code 在 Linux 上把自己的登录凭证写在 `CLAUDE_CONFIG_DIR/.credentials.json`：`claude_logout` 删它（macOS 上删钥匙串条目）。它和 `secrets/**` 都在 `CCA_DIR_PRIVATE` 里
- 官方账号记下备查的 API key（`cca [codex] key 账号`，App 的「记录 API key…」；控制台里建的 key 之类）：服务名 `cca-ref-<账号目录名>`（Codex `cca-codex-ref-`），账户名 `reference`，config.json 记在 `ref_keys`。cca **绝不**拿它登录或启动账号（会变成按量计费）：`_api-key`、`--settings` 只给第三方 API 账号，前缀分开也免得 `cca edit` 把 API 账号改成官方账号时把 key 留下。`key_hints` 记每个 key 的掩码（`key_hint`：开头 8 个字符里最后一个 `-` 以前的前缀 + … + 最后 4 个；短于 24 个字符的不给）和存的时候的域名，`_gui state` 的 `api_key` 照它说、不读钥匙串（旧版本存的没有掩码，读到 key 时 `kc_hint` 补上）。`--show` 把 key 原样打到 stdout（App 经内存里的管道读，不能是 runsToCompletion；在 Claude Code 的会话里（有 `CLAUDECODE`、没有 `CCA_EVENTS`）不给，免得进对话记录），`--forget` 要同意（`key_forget`）。`cca rm`（`logout_account`）、`restore`（`secrets-forget all`，确认清单里说了）一起删；`cca edit` 删掉账号后 `secrets-forget` 也删，删了找不回来所以往 stderr 说一声（带掩码）；重新登录、改别名不动。`key_hints` 总记着存的时候的域名（短的 key 没有掩码，hint 是空字符串），换了服务商才认得出。App 的明文只在 `KeyReveal`（不被观察）里，复制走 `copySecret`（标成机密、60 秒后清），绝不走 `AppModel.copy`（会把文字放进提示和读屏）

## 测试
- 钥匙串：假 `security` 把条目存在 `$T/keychain/<服务名>/<账户名>`，命令行记在 `security.log`、`-i` 读到的命令记在 `security.stdin`；`touch "$T/keychain_locked"` 模拟存不进去，`touch "$T/keychain_unreadable"` 模拟读不了（`find-generic-password` 退出码 51：`key --show` 的 `keychain_unavailable`）。官方账号记下备查的 key 在 `cca-ref-<账号目录名>/reference`（Codex `cca-codex-ref-`）。假 `env` 把每次的命令行记在 `$T/env.log`（检查 key 没进命令行）。`cca edit` 用 `EDITOR="$T/edit.py" CCA_TEST_EDIT='j.pop("x")'` 这样改表
- 密钥文件（regress.sh 10c 末尾）：`run CCA_KEYSTORE=file …` 让 macOS 上也存成 `~/.cca/secrets` 下的文件（不碰假 `security`，security.log 里不该有新行）
- 第三方 API：假 `curl` 对 `https://api*.example.com/*` 回 401（直连记 `CURL api direct`），`touch "$T/api_down"` 连不上；假 `claude` 遇到 `--settings` 记下 `SETTINGS: env=…`，并像 Claude Code 一样执行 apiKeyHelper，取到的 key 写进 `$T/claude.key`（不进日志）
