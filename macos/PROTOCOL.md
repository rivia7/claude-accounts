# cca App ⇄ cca 的接口（GUI_API 1）

cca App（`macos/`）只经这里写的接口和 cca 打交道：**不读 `~/.cca` 里的文件、不解析中文输出、不在 Swift 里重算 cca 的规则**
（账号目录名、端口、维护者映射、用量是否过期……都由 cca 算好给出）。改这里的格式要同时改 cca、Swift 模型
（`macos/Sources/CcaKit/Protocol/`）和回归测试（`tests/regress.sh` 的 10g 节，提问会话是 10h 节）。

兼容规则：

- 只能**加**字段、加枚举值、加事件类型；App 遇到不认识的字段、枚举值、事件类型一律忽略（枚举按字符串收，不认识的当“其它”）。
- 删字段、改含义才升 `GUI_API`（`cca _gui hello` 的 `api`）。App 声明自己支持的范围，不在范围内就提示更新。
- 每份 JSON 文档都带 `"version": 1` 和 `"kind"`。时间一律是 Unix 秒（浮点数），字段名以 `_epoch` 或 `_at` 结尾
  （`usage --json` 里沿用的 `fetched_at`、`generated_at`、`resets_at` 是旧字段，保留原样，App 用对应的 `*_epoch`）。
- JSON 只写到 stdout；给人看的文字照旧写 stderr，App 只在“详情”里原样显示（提问会话的等待页还显示最新的一行，见 4.6），不解析。

## 1. 怎么运行 cca

- 可执行文件：`~/.local/bin/cca`，没有就 `~/.cca/bin/cca`；开发模式下 `cca gui` 会用 `open --env CCA_GUI_CCA=<路径>` 告诉 App。
- 每次调用都是一个新进程：stdin 接 `/dev/null`，自己一个会话（`POSIX_SPAWN_SETSID`），取消时给整个进程组发 SIGTERM，3 秒后 SIGKILL。
  这样 cca 永远打不开 `/dev/tty`，所有需要回答的地方都按“非交互”处理（例外是提问会话，见 4.6）。
- 环境：继承 App 的环境，去掉 `CCA_TTY`、`CCA_YES`、`CCA_SKIP_CHECK`、`CCA_GRANT`、`CCA_EVENTS`、`CCA_RO`、`CCA_PROMPT`；`PATH` 后面补上
  `$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin`；设 `CCA_GUI=1`、`NO_COLOR=1`；按需设 `CCA_EVENTS`、`CCA_GRANT`（见 4）、`CCA_PROMPT`（见 4.6）。
- **绝不**设 `CCA_TTY`、`CCA_YES`、`CCA_SKIP_CHECK`，绝不调用 `cca _api-key`。读 API key 只用 `cca [codex] key 账号 --show`（4.10），
  它的 stdout 只能经内存里的管道读，不能按 `runsToCompletion` 接到文件。
- `cca codex …` 只在 `hello.tools.codex.enabled` 为真时才能调用：没有 `~/.cca/codex.json` 时运行任何 `cca codex …` 都会把它建出来，
  维护者映射里的 Codex 账号随即被自动加进来、分走端口。
- 同一个账号（工具 + 键）的 `app` / `web` / `billing` / `limit-resets` / `rename` 一次只跑一个（同一个资料目录绝不能开两份），`devices`、`key --show` / `key --forget` 也是同一个账号一次一个；`usage` 每个工具一次一个；同一个账号的 `reset`
  一次一个；`router sync` 全局一次一个；`update 版本号`、`gui update` 全局一次一个（一个接着一个）。`_gui hello|state|live`、`update --check --json`
  随时可以跑。`clients check|mode|install` 共用一个客户端更新闸门，不能重叠执行（见 8）。
- 建议的超时：`hello` 5 秒、`state` 15 秒、`live` 30 秒、`usage` 150 秒、`check` 180 秒（App 定时检测几个账号时 300 秒）、`app` / `web` / `billing` / `limit-resets` 120 秒、`router sync` 180 秒、`gui update` 300 秒、
  `policy pull --json` 180 秒、`rename`、`reorder`、`policy request` 60 秒、`update --check --json` 150 秒、`update 版本号` 300 秒、`devices --json` 30 秒、
  `key --show` / `key --forget` 40 秒（cca 读钥匙串最多等 30 秒）。
- `app`、`web`、`billing`、`limit-resets`、`router sync`、`reset` 一旦开始就要让它们跑完：App 退出（或崩溃）时它们不能跟着被杀（cca 往 stderr 写字时管道没人读会被 SIGPIPE 杀掉，
  可能 App 没打开、路由器同步了却没记下；重置额度用掉了一次却没记下重置后的用量）。所以这几条命令的 stdout / stderr 接到文件，跑完再读，不限时；
  进度和结果看事件文件。
- `clients install TOKEN` 同样运行至完成，退出 App 不终止安装事务；`clients check` 和 `clients mode` 可取消。检查建议超时 900 秒，
  安装的下载/事务超时由后端控制，`clients status` 30 秒（要现跑 `claude --version`、`codex --version`，刚更新完第一次启动较慢），`clients mode` 30 秒。
- App 侧的刷新提示（不读内容，只当作“该重新跑一次 `_gui state`”的信号）：cca 保证这些文件的位置不变——`~/.cca` 下的 `accounts.json`、
  `codex.json`、`config.json`、`policy/policy.json`、各账号目录里的 `usage.json`、`health.json`、`cli-<数字>.json`（命令行会话的登记，见 2.2 的
  `cli`：出现、消失都要重新读 `state`；只认这个文件名，`*.tmp` 不算），`core/config.yaml`（内置代理的配置，见 7：
  App 看不见时也要马上重新读 `state`），以及 `bin/cca`（换了就重新 `hello`）；
  桌面 App 的 bundle id：Claude App `com.anthropic.claudefordesktop`、ChatGPT App `com.openai.codex`（它们启动、退出时刷新 `app.running`）。

## 2. 只读的查询：`cca _gui hello | state | live`

都不经过自动更新和映射拉取（不会中途换成新版本的 cca），不改 `~/.cca` 里的任何文件（`state`、`live` 在 `CCA_RO=1` 下运行：
账号表按维护者映射对账、分配端口都只在内存里算，结果照样反映在输出里；`live` 经 SSH 连路由器时，ssh 自己会建复用连接的
`~/.cca/cm-*`，第一次连时往 `~/.cca/known_hosts` 记主机公钥）。打印了 JSON 就退出 0（内容里可以带 `errors`）；
非 0 表示内部错误，stdout 不保证有 JSON。

### 2.1 `cca _gui hello`（只用 bash，约 20 毫秒）

```json
{
  "version": 1, "kind": "hello", "api": 1, "prompt": 1,
  "cca_version": "2.9.0", "dev_mode": false, "installed": true,
  "self": "/Users/me/.cca/bin/cca", "home": "/Users/me/.cca",
  "tools": {
    "claude": {"enabled": true, "bin": "/Users/me/.local/bin/claude"},
    "codex": {"enabled": false, "bin": null}
  },
  "gui": {"app": "/Users/me/Applications/cca.app", "app_version": "2.9.0", "staged_version": null}
}
```

- `reorder`：有 `cca [codex] reorder`（4.7），App 调整账号顺序后写回账号表；没有这个字段（旧版 cca）时顺序只记在 App 里。
- `prompt`：提问会话（4.6）的版本。没有这个字段（旧版 cca）或者版本不认识时，App 不开提问会话，流程走终端交接（6）。
- `forms`：配置路由器、配置本机 Clash、账号表能在 App 里填表（4.6）。没有这个字段时这三条走终端交接。
- `dock`：有 `cca _dock-tidy`（6）：终端交接退出系统终端以后，交给 cca 去掉程序坞「最近使用的 App」里它留下的图标。没有这个字段时交接脚本不调它（旧版 cca 会把不认识的 `_dock-tidy` 当成启动账号）。
- `installed`：`~/.cca/config.json` 在不在。为 false 时 App 显示“先在终端里运行 cca install”（可以用终端交接，见 6）。
- `tools.claude.enabled` 恒为 true；`tools.codex.enabled` = `~/.cca/codex.json` 存在。`bin` 是找到的 `claude` / `codex`，没装为 null。
- `gui.app_version`：装着的 cca.app 的版本（没装为 null）；`gui.staged_version`：已经下载校验好、等 App 退出后再换上的版本（没有为 null）。
- App 的版本号和 `cca_version` 不同时提示“更新并重新打开”（运行 `cca gui update --relaunch <App 的 pid>`，然后自己退出）。

### 2.2 `cca _gui state`（只读快照，两个工具一起，不联网）

```json
{
  "version": 1, "kind": "state", "generated_at": 1794042000.0, "cca_version": "2.9.0",
  "errors": [],
  "notices": [{"code": "policy_added", "tool": "claude", "key": "工作(bob@example.com)", "node": "US-B"}],
  "config": {
    "route_pref": "auto",
    "router": {"configured": true, "host": "192.0.2.10", "ssh_port": 22, "proxy_host": "192.0.2.10"},
    "local": {"configured": true, "client": "clash-verge", "name": "Clash Verge"},
    "nodes": ["US-A", "US-B", "US-C", "US-D"],
    "min_trust": 50, "leak_check": true, "update": "auto",
    "policy": {"configured": true, "repo": "example/cca-policy", "next_check_epoch": 1794045600.0}
  },
  "routing": {
    "first_port": 7901, "pool_size": 8,
    "sync": "ok", "local_sync": "n/a",
    "slots": [
      {"slot": 1, "group": "CCA-1", "port": 7901, "tool": "claude", "key": "alice@example.com", "node": "US-A", "local_node": "US-A"},
      {"slot": 2, "group": "CCA-2", "port": 7902, "tool": "claude", "key": "工作(bob@example.com)", "node": "US-B", "local_node": "Verge-B"},
      {"slot": 3, "group": "CCA-3", "port": 7903, "tool": null, "key": null, "node": "REJECT", "local_node": "REJECT"}
    ],
    "nodes": [{"name": "US-A", "local_name": "US-A", "users": [{"tool": "claude", "key": "alice@example.com"}]}]
  },
  "tools": [
    {
      "id": "claude", "label": "Claude", "app_label": "Claude App", "cli_prefix": [],
      "enabled": true, "bin": "/Users/me/.local/bin/claude",
      "app": {"installed": true, "path": "/Applications/Claude.app"},
      "chrome": true,
      "settings_problems": [], "errors": [], "last": "alice@example.com",
      "accounts": [ "…账号，见下…" ]
    },
    {"id": "codex", "label": "Codex", "app_label": "ChatGPT App", "cli_prefix": ["codex"], "enabled": false, "bin": null,
     "app": {"installed": false, "path": null}, "chrome": true, "settings_problems": [], "errors": [], "last": null, "accounts": []}
  ]
}
```

账号（`tools[].accounts[]`，顺序就是账号表的顺序）：

```json
{
  "key": "工作(bob@example.com)", "id": "bob-example-com-1a2b3c",
  "alias": "工作", "email": "bob@example.com", "name": "工作", "label": "工作（bob@example.com）",
  "kind": "oauth",
  "login": "logged_in", "login_email": "bob@example.com", "plan": "Max", "plan_tier": "advanced",
  "node": "US-B", "port": 7902, "slot": 2, "group": "CCA-2",
  "pinned": true, "pinned_node": "US-B", "is_last": false,
  "api": null,
  "badge": {"label": "工", "color": "#30A46C"},
  "app": {"running": false, "pid": null},
  "cli": {"running": true, "sessions": 1, "since_epoch": 1794041000.5},
  "can": {"app": true, "web": true, "billing": true, "limit_resets": true, "usage": true, "check": true, "cli": true, "api_key": true},
  "can_why": {},
  "api_key": {"saved": true, "hint": "sk-ant-…x7Qa", "source": "keychain"},
  "usage": { "…用量条目，见 3.1…" },
  "usage_weeks": { "…每周用量，见下面…" },
  "health": { "…出口检测结果，见 3.3…" }
}
```

第三方 API 账号：`kind: "api"`、`login: "api"`、`email: ""`、`plan: ""`、`plan_tier: ""`、`badge: null`、`api: {"host": "api.example.com", "model": "model-x"}`
（**绝不**带 key）；直连的没有端口：`node`、`port`、`slot`、`group` 都是 null；走节点的照常有。`usage`、`usage_weeks` 恒为 null。

- `login`：`logged_in` | `logged_out` | `email_mismatch`（登录的不是这个账号，`login_email` 是实际登录的）| `api`。
- `plan`：套餐的显示名（`Max`、`Pro 200`……），不知道为 `""`。
- `plan_tier`：套餐的档位，cca 按结构化的账号信息算好（Claude：组织类型 + 额度倍数，倍数缺失才看官方的套餐名；ChatGPT：原始的套餐值），
  **App 不从 `plan` 的文字猜**（官方随时改叫法）：`entry`（Free、Go）| `standard`（Pro、Plus、Team、Business、Enterprise、Edu 等）|
  `advanced`（Max、Pro 100 / 200、Team Premium、Business Premium）| `premium`（Ultra、Pro 500）。不知道为 `""`：没登录、登录的不是这个账号、
  第三方 API 账号、认不出的新套餐（这时 `plan` 可能仍有值）。没有这个字段（旧版 cca）当作 `""`。
- `pinned`：维护者的映射锁定了这个账号的节点（`pinned_node` 是映射要的节点，已按改名换算）；App 里不提供改节点。
- `is_last`：上次在命令行里用的账号。
- `badge`：和 `~/Applications` 里启动器图标右下角一样的标识（文字 + 颜色），App 用它当头像；API 账号为 null。
- `app.running` / `app.pid`：这个账号的桌面 App（Claude 账号是 Claude App，Codex 账号是 ChatGPT App）在不在运行。
- `cli`：经 cca 启动（`cca @账号`、`cxa @账号`、菜单里选的）、还在运行的命令行会话：`sessions` 几个，`running` 等于 `sessions > 0`，
  `since_epoch` 是最早那个会话开始的时间（Unix 秒），没有会话时为 null。官方账号、第三方 API 账号都有；`cca usage`、登录、`app-server`
  起的 claude / codex 不算，没经 cca 启动的也不算。cca 启动工具前在账号目录登记 `cli-<cca 的进程号>.json`，工具一退出就删；cca 被 `kill -9`、
  断电留下的登记，`state` 按进程号 + 进程的启动时间核对（对不上不算，也不删文件）。没有这个字段（旧版 cca）当作没有会话。
  “在用”（总览里画成在用、头像右下角的绿点）由 App 自己判断：`app.running || cli.running`。
- `can` / `can_why`：按钮可不可用、为什么不可用。原因码：`api_account`（第三方 API 账号没有 App / 网页版 / 订阅用量）、
  `app_missing`（没装对应的桌面 App）、`no_chrome`（没装 Google Chrome：网页版不能经账号的出口打开）、
  `not_logged_in`（没登录，查不了用量）、`tool_missing`（没装 claude / codex）。`cli` 是能不能在终端里打开（`cca [codex] @账号`，
  第三方 API 账号也可以）；旧版 cca 没有这一项，当作不能。`policy_lock` 是能不能请求维护者把节点锁进映射（4.8），没有对应的 `can_why`：不能就不显示这个入口。
  `billing` 是能不能打开控制台的账单页（`cca [codex] billing 账号`，用账号自己的浏览器实例和出口；订阅附带的每月 API 额度和余额只在那里显示，官方没有查询接口，
  所以 App 只给入口、不显示数字）：条件和 `web` 一样，也走同样的步骤、事件和同意。打开后 cca 把账号的邮箱放进剪贴板（这些登录页不认网址里的邮箱参数），
  `result` 的说明里会讲；`limit-resets` 也一样，网页版（`web`）不动剪贴板。同样没有对应的 `can_why`：不能就不显示这个入口；旧版 cca 没有这一项，当作不能。
  `limit_resets` 是能不能打开 claude.ai 的用量页（`cca limit-resets 账号`，Claude 账号的重置次数和到期日只在那里显示）：只有 Claude 的账号有，其余条件和 `web` 一样，也没有对应的 `can_why`；旧版 cca 没有这一项，当作不能。
  `devices` 是账号页有没有「设备」一节（3.4）：映射里的官方账号、维护者在映射里配置了设备统计的服务器、这台 Mac 有映射的 deploy key、
  config.json 里没写 `"presence": false`。同样没有对应的 `can_why`：没有就不显示这一节、不调 `devices`；旧版 cca 没有这一项，当作没有。
  `api_key` 是能不能记下 / 显示 / 复制 API key（4.10）：新版 cca 的所有账号都是 true；旧版 cca 没有这一项，当作不能，不显示入口。
- `api_key`：存没存 API key（4.10）。官方账号的是**记下备查**的（比如控制台里建的 key：只存进这台 Mac 的钥匙串，cca 绝不拿它登录或启动账号，
  那样会变成按量计费）；第三方 API 账号的是启动要用的那把。`saved`：存了没有（第三方 API 账号换了服务商、和存的时候域名不一样的算没存）；
  `hint`：掩码，存的时候 cca 算好的（开头 8 个字符里最后一个 `-` 以前的前缀 + `…` + 最后 4 个字符，例如 `sk-ant-…x7Qa`；短于 24 个字符的 key
  没有掩码，是 `""`，App 画圆点）；`source`：`keychain`（钥匙串里）| `table`（账号表的 `key` 字段里，明文；只有第三方 API 账号有）| `""`（没存）。
  `state` 不读钥匙串，掩码照 config.json 里记的说；**明文只在用户点「显示」「复制」时经 `key --show` 要**。旧版 cca 没有这个字段，当作 null。
- `usage`：最近一次的用量（缓存，不联网）。只给 `login == logged_in` 的官方账号；没有缓存为 null。
- `usage_weeks`：每周用量（账号页的「每周用量」、侧栏的「用量统计」）：最近 12 周每周用到这一周额度的百分之几，和这几周的统计。
  Claude、OpenAI 都不给以前的用量，这是 cca 每次查到用量时（`cca usage`、重置额度）记在账号目录里的，从开始记的那一周起一周一周长出来；
  没有记录为 null（还没查到过、第三方 API 账号；旧版 cca 没有这个字段，同样当作 null）。没登录的官方账号也照样给以前记下的。
  `state` 只读：比记下的新的用量（Claude Code 自己的缓存）只在内存里并进来。

  ```json
  "usage_weeks": {
    "since_epoch": 1787047200.0,
    "weeks": [
      {"start_epoch": 1793088000.0, "end_epoch": 1793692800.0, "used_percent": 72, "resets": 0, "last_epoch": 1793691300.0, "current": false, "partial": false},
      {"start_epoch": 1793692800.0, "end_epoch": 1794297600.0, "used_percent": 41, "resets": 0, "last_epoch": 1794041100.0, "current": true, "partial": false}
    ],
    "summary": {"weeks": 11, "average_percent": 84, "maxed": 3, "under_half": 0}
  }
  ```

  - `since_epoch`：从什么时候开始记的（最早记下的一次，可能早于列出的这几周）。
  - `weeks[]`：按时间排好，只列结束在最近 12 周里的。一项是本周额度（`seven_day`）的一个窗口：`end_epoch` 是它的重置时间，`start_epoch`
    往前 7 天；中间重新计时的（下一周在这一周结束前就开始了，例如官方重置了额度），`end_epoch` 是下一周开始的时候。
  - `used_percent`：这一周用到额度的百分之几：这一周查到的最高的；比最高的低了超过 5 个百分点算中间重置过（`resets` 加一），重置前的加上，
    所以可能超过 100。`last_epoch`：最后一次查到的时间。
  - `current`：还没过完的这一周（最多一项，是最后一项）。`partial`：过完的一周里，结束前一天一次也没查到（之后用的没记下，实际可能更多）；
    用完了的（100% 及以上）不算。
  - `summary`：`weeks` 里过完的那几周（`current` 为 false 的）的统计，百分比按四舍五入后的整数算：`weeks` 几周，`average_percent` 平均每周
    （一周都没过完时为 null），`maxed` 用完（100% 及以上）的几周，`under_half` 没用到一半的几周。

  App 只排版、不重算：条的高低、哪一周填灰色、统计的数字都照这里的。
- `health`：最近一次的出口检测结果（启动、`cca check` 时记下的），从没测过为 null。

`config`：

- `route_pref`：`auto`（连得上路由器走路由器，连不上走本机 Clash）| `router` | `local`。
- `router.configured`：配置过路由器；`proxy_host` 是账号端口所在的主机（可能和 `host` 不同）。
- `local.configured` / `client`：本机 Clash 客户端（`clash-verge` | `manual` | `builtin` | `""`）。`builtin` 是内置代理：App 要托管
  它的内核（见 7）。`name` 是给人看的名字（`Clash Verge`、`内置代理`……），App 照着显示。

`routing`：

- `slots`：端口池的每个槽位（数量是 `pool_size`，8 的倍数）。`tool` / `key` 为 null 的是空槽位（`node` 是 `REJECT`）。
  `local_node` 是本机 Clash Verge 里的叫法（不用 Clash Verge、换算不了时和 `node` 一样）。
- `sync`：路由器和这台 Mac 上的账号表一致吗。`ok` | `pending`（下次启动账号时会同步，也可以点“同步路由器”）| `n/a`（没配置路由器）。
- `local_sync`：本机 Clash 的端口池是否最新，`ok` | `pending` | `n/a`（没配置本机 Clash）。
- `nodes`：可选节点和用它的账号。

`errors[]`：`{"code": "config_unreadable" | "table_unreadable" | "account_unreadable", "tool": "codex" | null, "message": "…"}`（文件读不了；
`account_unreadable` 是某个账号目录里的登录状态文件读不了，这个账号当作没登录）。
`tools[].errors[]`：账号表校验不过的地方，`{"code": "table_invalid", "message": "…"}`（有它时启动会失败，App 提示在终端里 `cca [codex] edit`）。
`tools[].settings_problems[]`：共享设置里会盖掉账号代理 / 接口 / 凭证的项，形如 `"settings.json:ANTHROPIC_BASE_URL"`（只用来显示）。
`tools[].chrome`：装了 Google Chrome（网页版要它才能经账号的出口打开）。
`notices[]`：只读对账时“本来会做”的改动，下一次真正运行 cca 时会写进去：
`{"code": "policy_added", "tool", "key", "node"}`、`{"code": "policy_alias", "tool", "key"（新键）, "old_key"}`、
`{"code": "policy_pinned", "tool", "key", "node", "old_node"}`、`{"code": "nodes_filled", "node"}`。

`can_why` 各项可能的值：`app`：`api_account` | `app_missing` | `tool_missing`（ChatGPT App 打开前要用 codex 命令检查设置）；
`web`：`api_account` | `no_chrome`；`usage`：`api_account` | `tool_missing` | `not_logged_in`；`cli`：`tool_missing`；`check` 总是可用。

### 2.3 `cca _gui live`（只读，要联网：连路由器或本机 Clash Verge）

按 `route_pref` 选看哪一边（环境变量 `CCA_ROUTE=router|local` 可以指定看另一边）：

```json
{
  "version": 1, "kind": "live", "generated_at": 1794042010.0,
  "route": "router", "route_pref": "auto", "at_home": true, "proxy_host": "192.0.2.10",
  "router": {"configured": true, "host": "192.0.2.10", "reachable": true, "backend": "OpenClash",
             "running": true, "hook": true, "cron": true, "error": null},
  "local": {"configured": true, "client": "clash-verge", "name": "Clash Verge", "running": null, "error": null},
  "slots": [
    {"slot": 1, "group": "CCA-1", "port": 7901, "tool": "claude", "key": "alice@example.com",
     "expected": "US-A", "selected": "US-A", "in_kernel": true, "listening": true, "reachable": true, "state": "ok"}
  ],
  "summary": {"ok": 1, "drift": 0, "closed": 0, "unknown": 0, "empty": 7}
}
```

- `route`：`router` | `local` | `none`（哪边都没配置）。`at_home`：连得上路由器（1 秒探测）。`proxy_host`：账号端口现在在哪台机器上
  （看路由器时是路由器的地址，看本机时是 `127.0.0.1`，`none` 时为 null），App 显示「地址:端口」用它。
- `router.error` / `local.error`：`null` 或 `{"code": "unreachable" | "status_failed" | "not_configured" | "not_running", "message": "…"}`。
  `router.backend`：`OpenClash` | `ShellCrash` | null。`local.running`：本机客户端（Clash Verge、内置代理的内核……）的控制接口能不能连上
  （没看这一边时为 null）。
- 槽位：`expected` 是应该选中的节点（这台 Mac 的账号表算出来的；看本机时是本机的叫法），`selected` 是内核里现在选中的（拿不到为 null），
  `in_kernel`：内核里有没有这个组，`listening`：路由器 / 本机上这个端口在不在监听（拿不到为 null），`reachable`：这台 Mac 连不连得上这个端口。
- `state`：`ok` | `drift`（选中的不是应选的，同步路由器能修好）| `closed`（这台 Mac 连不上这个端口）| `unknown`（拿不到选中的节点）| `empty`（空槽位）。
- 慢：路由器那边要几秒。只在窗口可见时查：分流页打开时每 60 秒一次；总览（首页）不定时查，切到它、窗口变得可见时上次的超过 2 分钟才查一次，操作（同步路由器等）之后也查。

## 3. 用量、出口检测

### 3.1 `cca [codex] usage [账号…] --json`

不写账号查全部；写了就只查这几个（App 自动刷新时只查到期的账号，带 `CCA_USAGE_JOBS=2`：最多同时查两个，默认 4）。

格式和以前一样，文档另加 `"kind": "usage"` 和 `generated_epoch`，每个账号的条目**新增**：

```json
{
  "account": "alice@example.com", "key": "alice@example.com", "email": "alice@example.com", "plan": "Max",
  "source": "cache", "fetched_at": "2026-11-07T08:40:00Z", "fetched_epoch": 1794039600.0, "age_seconds": 2400, "stale": true,
  "error": "出口端口 7901 不通", "error_code": "port", "error_detail": "7901",
  "five_hour": {"utilization": 13.0, "resets_at": "2026-11-07T10:13:30.123456+00:00", "resets_epoch": 1794045210.12, "reset": false},
  "seven_day": {"utilization": 41.0, "resets_at": "2026-11-10T00:00:00+00:00", "resets_epoch": 1794297600.0, "reset": false},
  "seven_day_opus": null,
  "seven_day_sonnet": {"utilization": 60.0, "resets_at": "…", "resets_epoch": 1794297600.0, "reset": false},
  "seven_day_oauth_apps": null,
  "model_scoped": [{"name": "Sonnet", "utilization": 60.0, "resets_at": "…", "resets_epoch": 1794297600.0, "reset": false}],
  "extra_usage": null,
  "use_first": {"left_percent": 59, "from_epoch": 1794211200.0, "until_epoch": 1794297600.0},
  "outdated_login_plan": "Pro"
}
```

- `source`：`live`（这次查到的）| `cache`（没查到，用上次的）| `none`（没有数据）。
- `error_code`（`error` 是给人看的文字）：`login` | `mismatch` | `route` | `port` | `region` | `trace` | `unsupported` | `timeout` | `nolimits` | `auth` | `error`，没出错为 null。
  `auth`：登录凭证失效了（Codex 说 token 刷新不了、服务器回 401；改过密码、在别处退出过都会这样），本地的登录信息还在，所以 `_gui state` 的 `login` 还是 `logged_in`，要重新登录（`cca [codex] login 账号`）。
  Claude 登录失效时 Claude Code 只回“没有用量”，分不清原因，是 `nolimits`。
- `stale`：数据超过 10 分钟。`reset`：这个窗口的重置时间已经过了（显示“已重置”，不要显示旧的百分比）。
- 窗口：Claude 有 `five_hour`、`seven_day`、`seven_day_opus`、`seven_day_sonnet`、`seven_day_oauth_apps`；Codex 只有 `five_hour`、`seven_day`，
  另有 `credits`（`{"hasCredits": true, "unlimited": false, "balance": "12.5"}`）和 `reset_credits`（`{"availableCount": 2, "next_expires_epoch": …, …}`）。
- `model_scoped[]`：按模型的每周额度（Claude 的 Fable；Codex 另外计的额度）。Claude 的条目在 cca 认得出时**另有**它和本周总额度的关系：
  `allowance_percent_of_weekly`（最多能用本周总额度的百分之几，例如 50）、`used_percent_of_weekly`（已经用掉本周总额度的百分之几）；认不出时
  没有这两个字段。`utilization` 是这个模型自己那份额度用了多少。
- `reset_credits`（Codex，还能重置几次额度，codex 原样给的）：`availableCount` 次数；`credits[]` 每一次的明细（可能比次数少，codex 只给了次数时
  没有），`id`、`title`（没有为 null，界面上叫“完整重置”）、`status`（`available` 才能用）、`expiresAt` / `grantedAt`（codex 原样给的）。cca 另加：
  每一次的 `expires_epoch`（Unix 秒，不过期为 null），和还能用的里面最早过期的那次的 `next_expires_epoch`（都不过期、没有明细时为 null；
  App 显示“最早一次几号过期”用它，不自己挑）。重置见 4.5。
- `outdated_login_plan`（只有 Claude 的条目有，Codex 的没有这个字段）：Claude Code 登录时记下的套餐（查用量时它回报的 `subscription_type`）
  和账号现在的套餐（官方资料里的种类：Pro / Max / Team / Enterprise）对不上时，是记下的那个的叫法（`"Pro"`）；对得上、不知道时为 null。
  Claude Code 只在登录时记套餐，之后升级、降级都不改（2.1.322 核实），所以它还按旧套餐显示（启动横幅、`/status` 里写 Claude Pro）、按旧套餐处理；
  额度由服务器按实际的套餐算，不受影响。重新登录（`cca login 账号`）才会更新：cca 经它登录之前把记下的作废，下一次查用量对上了就是 null。
  App 照它说一句、给「重新登录…」，不自己比（`plan` 是账号现在的套餐）。没有这个字段（旧版 cca）当作 null。
- `use_first`（周结算提醒，不提示时为 null）：本周额度（`seven_day`）快重置了、还剩不少，建议先用这个账号，别浪费。`left_percent` 是本周还剩
  百分之几（整数：`100 - 四舍五入后的 utilization`，和显示的百分比一致）；`from_epoch` 是本周重置前 24 小时，`until_epoch` 是本周的重置时间。
  **App 拿当前时间和 `[from_epoch, until_epoch)` 比**（快照可能放了好几分钟），区间外当作没有；规则（24 小时、至少 20%）由 cca 定，App 不重算。
  cca 给 null 的情况：没有重置时间、本周窗口已经 `reset`、剩得不到 20%、这次查询说明账号现在用不了（`error_code` 是 `login` / `mismatch` /
  `route` / `port` / `trace` / `region` / `nolimits` / `auth`；`timeout`、`error`、`unsupported` 不算，照缓存里的数）。登录、线路、出口、纯净度、
  泄露、5 小时的额度用完或快用完时不提示，由 App 按各自的状态挡（`Diagnosis`）。只给图形界面：终端里 cca 不提示。旧版 cca 没有这个字段，当作 null。
- 退出码 1 表示没有一个账号有数据（JSON 照样输出）。查询会经各账号的出口，最慢 2 分钟左右。
- `_gui state` 里的 `usage` 是同样的条目（`source` 是 `cache` 或 `none`）。

### 3.2 `cca [codex] check [账号…] --json`

不写账号测全部；写了就只测这几个（App 定时检测只测到期的账号）。人看的文字照旧写 stderr，stdout 输出：

```json
{
  "version": 1, "kind": "check", "tool": "claude", "generated_at": 1794042100.0, "route": "router",
  "accounts": [{"key": "alice@example.com", "name": "alice@example.com", "node": "US-A", "port": 7901, "health": { "…3.3…" }}],
  "shared_exit_ips": ["203.0.113.1"]
}
```

`shared_exit_ips`：几个账号共用的出口 IP（不该出现）。`route`：`router` | `local` | null（没有走端口的账号）。

### 3.3 出口检测结果（`health`）

每次启动账号（命令行、桌面 App）和 `cca check` 都会记进账号目录的 `health.json`，`_gui state` 里的 `health` 就是它（加上 `age_seconds`、`stale`、`applies`）：

```json
{
  "version": 1, "checked_at": 1794041000.0, "age_seconds": 1000, "stale": false, "applies": true,
  "source": "launch", "kind": "exit", "route": "router", "node": "US-A", "port": 7901,
  "result": "ok",
  "ip": "203.0.113.1", "country": "US", "org": "Example Networks", "trust": 97, "trust_label": "极度纯净",
  "tags": ["住宅IP"], "restricted_region": null,
  "message": "203.0.113.1 · US · Example Networks · 纯净度 97（极度纯净） · 住宅IP", "note": "",
  "leak": {"dns": "ok", "webrtc": "ok", "line": "DNS 未发现地区异常 · WebRTC：STUN 出口一致（浏览器状态未验证）", "dns_message": "", "webrtc_message": ""}
}
```

- `source`：`launch` | `check`。`kind`：`exit`（官方账号：claude.ai / chatgpt.com 看到的出口）| `api`（第三方 API 账号：接口连不连得上）。
- `route`：`router` | `local` | `direct`（直连的第三方 API 账号，不经端口）| null。`port`、`node` 直连时为 null。
- `result`：`ok` | `low_purity`（纯净度低于设定）| `restricted`（出口在不可用地区，`restricted_region` 是地区名）| `no_exit`（拿不到出口 IP）|
  `no_score`（检测服务没给评分）| `port_closed` | `api_ok` | `api_unreachable`。
- `trust`：0–100，没有评分为 null。`stale`：超过 6 小时。`applies`：测的时候用的节点和账号现在的节点相同（换了节点就是旧结果）。
- `leak`：没测（关掉了泄露检测、出口本身就不对）为 null。`dns`：`ok` | `leak` | `failed` | null；`webrtc`：`ok` | `leak`（STUN 看到中国大陆的地址）| `note`（看到的地址和出口不同，但不是中国大陆的：只作提醒，不算问题）| `failed`（包括看到了别的地址、归属地查不到）| null；
  `*_message` 是发现风险或备注时给人看的说明。`ok` 仅表示宿主探针未发现该项异常，不是浏览器实测结论；空结果、超时及信息不足使用 `failed`。

### 3.4 `cca [codex] devices KEY --json`：账号连着的设备

`can.devices` 为真的账号，账号页看得见时 App 马上运行一次，之后每分钟一次（低优先级；用户 15 分钟没碰电脑时不查），点「刷新」时不降优先级。
cca 问维护者的设备统计服务器（直连，钉住映射里的证书公钥，用这台 Mac 的 deploy key 签名），把服务器给的相对秒数换成这台 Mac 的时钟：

```json
{
  "version": 1, "kind": "devices", "generated_epoch": 1794042000.0, "tool": "claude", "me": "studio-mac",
  "idle_after": 600, "offline_after": 150, "reporting_error": null,
  "accounts": [{"key": "工作(bob@example.com)", "id": "bob-example-com-1a2b3c", "devices": [
    {"id": "SHA256:…", "name": "studio-mac", "me": true, "state": "in_use", "cli": 2, "app": false,
     "since_epoch": 1794034800.0, "last_active_epoch": 1794041940.0, "last_used_epoch": 1794041940.0, "seen_epoch": 1794041985.0}
  ]}]
}
```

- `me`：这台 Mac 在服务器上的名字（维护者给它的 deploy key 起的别名），服务器还不认它时为 null。`idle_after`、`offline_after`：服务器的设置（秒），
  旧的服务器没给时为 null。
  `reporting_error`：这台 Mac 最近一次没报成的原因（给人看的话），报成了是 null。
- `accounts`：问的账号（不写账号时是这个工具里映射里的全部官方账号），`devices` 是 30 天内经 cca 用过它的设备，这台 Mac 排第一，
  其余按 `in_use`、`idle`、`offline`、`none`，同一种里最近用过的在前（cca 排好的，App 不重排）。
- `state`：`in_use`（开着会话或桌面 App，`idle_after` 秒内有动静）| `idle`（开着，很久没动静）| `none`（都关了）|
  `offline`（开着会话却超过 `offline_after` 秒没回报：可能休眠、断网、被强行结束，不知道现在还用不用）。不认识的值当 `none` 显示。
  动静：命令行会话看它终端的最后输入、输出时间，桌面 App 看它进程树用的 CPU（cca 和服务器算，App 不重算）。
- `cli`：开着几个命令行会话，`app`：桌面 App（`cca app` 打开的）开没开；离线时是最后一次回报时的。
- 时间（都是这台 Mac 的时钟，可能是 null）：`since_epoch` 开着的会话里最早那个从什么时候开始；`last_active_epoch` 开着的会话最后一次有动静
  （闲置从这时算起）；`last_used_epoch` 上次使用（会话都关了也留着）；`seen_epoch` 最后一次收到它的回报。
- 失败：退出码 1，没有 stdout，`fail` 事件的 `code` 是 `devices_unavailable`，`message` 是原因（连不上服务器、服务器还不认这台 Mac、
  证书指纹对不上、时钟差太多……）：App 留着上次查到的，旁边显示原因。用不了（这台 Mac 没配置映射、关掉了、账号不在映射里）时是 `devices_off`，
  `can.devices` 为假时 App 本来就不会调用。
- 这台 Mac 自己的上报不经 App：经 cca 启动映射里的账号（命令行会话、`cca app`）时 cca 在后台起一个上报进程，会话都关了它自己退出。

## 4. 事件和同意（`CCA_EVENTS`、`CCA_GRANT`）

### 4.1 事件文件

运行会改东西的命令（`app`、`web`、`billing`、`limit-resets`、`check`、`usage`、`router sync`、`gui update`、`rename`、`reorder`、`reset`、`update 版本号`、
`key --forget`）、检查更新（`update --check --json`）、查设备（`devices --json`）和读 API key（`key --show`）时（查不了的原因看 `fail` 事件），App 给一个新的空文件路径 `CCA_EVENTS=<文件>`，
cca 往里追加 JSON 行（一行一个事件）。不设就什么都不写；写不了也不影响命令本身。cca 的退出码、stderr 和不设时完全一样。
提问会话另有 `ask` 事件（见 4.6）。

```jsonl
{"v":1,"ev":"step","code":"preflight"}
{"v":1,"ev":"consent","item":"router_reload","scope":"OpenClash","grant":"router_reload:OpenClash","title":"需要重载 OpenClash","question":"现在重载吗？","detail":"…","severity":"warn","default":"yes"}
{"v":1,"ev":"terminal","code":"login","argv":["login","alice@example.com"],"display":"cca login alice@example.com","message":"alice@example.com 还没登录…"}
{"v":1,"ev":"fail","code":"port_closed","message":"端口 192.0.2.10:7901 不通。为避免走错出口，已停止。…"}
{"v":1,"ev":"result","code":"opened","route":"router","node":"US-A","port":7901,"message":"已打开 alice@example.com 的 Claude App，出口 US-A（端口 7901）"}
```

- `step.code`（打开 App 的进度）：`sync`（同步路由器）| `preflight`（检测出口）| `prepare`（准备账号目录）| `open`（打开 App）。
- `result.code`：`opened` | `activated`（已经开着，切到了前面）| `key_forgotten`（`key --forget`：删掉了官方账号记下的 API key，本来就没有时也是它，带 `tool`、`key`）；提问会话（4.6）做完时是 `added` | `key_saved` | `removed` | `saved`（账号表保存了）| `router_ready` | `local_ready`，带 `tool`、`key`（删除时 `key` 是 null），`added` 时 App 选中这个账号。`result.route`：`router` | `local` | `direct` | null。
- `fail.code`：`error`（其它）| `app_missing` | `api_account` | `tool_missing` | `table_invalid` | `sync_failed` | `no_router` | `port_closed` |
  `local_unavailable` | `node_missing`（在外面时本机 Clash 里没有这个节点）| `settings_conflict` | `region_restricted` | `no_exit_ip` |
  `low_purity` | `not_logged_in` | `login_mismatch` | `api_unreachable` | `open_failed` | `app_profile_unsupported` |
  `no_chrome`（无法使用独立浏览器）| `invalid_url`（不是 HTTP/HTTPS 地址）| `login_unsupported`（客户端不支持独立登录流程）|
  `alias_invalid`（改别名：新别名不合规、和别的账号重了、第三方 API 账号不能去掉别名、键里没有邮箱的官方账号不能改）|
  `account_missing`（改别名：没有这个账号）| `no_reset_credit`（重置额度：没有能用的重置机会了）|
  `reset_unsupported`（重置额度：这个版本的 codex 不支持，升级后再试）| `cancelled`（提问会话被取消，或用户在确认页答了“不做”）| `app_running`（删除账号：它的桌面 App 还开着）| `policy_unavailable`（`policy pull --json` 拉不到映射，上次拉到的照用）|
  `devices_unavailable`（`devices --json`：设备统计的服务器那边没成，见 3.4）| `devices_off`（`devices --json`：这台 Mac 用不了设备统计）|
  `key_missing`（`key --show`：这个账号还没存 API key）| `keychain_unavailable`（`key --show` 读不了钥匙串、`key` 存不进去，比如锁着、超时）。
  可能有好几条，最后一条是最终原因。
- `terminal`：这一步要在终端里完成（登录之类）。`argv` 是 `cca` 后面的参数（Codex 账号带 `codex`），`display` 是给人看、能直接复制进终端的
  完整命令（只有字母数字和 `@%+=:,./_-` 的参数照写，别的加单引号，例如 `cca login '工作(bob@example.com)'`）。
  `code`：`login` | `router_sync` | `router_setup` | `local_setup` | `api_key` | `edit`（账号表有表单改不了的问题：不是 JSON、键写错了）。App 能在表单里做的（`router_setup`、`local_setup`，要 `hello.forms`）直接开表单，别的走终端交接。

### 4.2 同意

需要用户同意的地方，cca 在非交互时不会自作主张：记一条 `consent` 事件，然后照旧拒绝（退出码和以前一样）。
App 弹原生对话框，用户同意后，带上 `CCA_GRANT=<grant>[,<grant>…]` **重新运行同一条命令**。

| item | scope | 什么情况 | 默认 |
|---|---|---|---|
| `router_reload` | 后端名（`OpenClash` / `ShellCrash`） | 端口池要调整，路由器要重载一次配置（约 20 秒，局域网的新连接会中断） | yes |
| `router_restart` | 后端名 | 运行配置不见了、或刚写进了参考节点，只能重启一次（所有连接都会断一下） | yes |
| `low_purity` | 出口 IP | 出口纯净度低于设定，问是否仍然用这个出口启动 | no |
| `codex_fallback_off` | 空 | 第一次打开 ChatGPT App：用 `codex features disable system_proxy_fallback` 在共享的 config.toml 里关掉系统代理回退 | yes |
| `open_clash_verge` | 空 | 在外面，Clash Verge 没开：打开它 | yes |
| `builtin_core` | 空 | 在外面，这台 Mac 上没有 Clash 客户端：改用内置代理（第一次要下载约 21MB 的内核，见 7） | yes |
| `reset_credit` | 账号目录名（`state` 里账号的 `id`） | 重置额度会用掉一次重置机会（见 4.5） | no |
| `key_forget` | 账号目录名 | 删掉官方账号记下的 API key（只删 cca 在钥匙串里记的这一份，控制台里的 key 不受影响；见 4.10） | no |

- `grant` 是 `item:scope`（scope 为空时就是 `item`），App 原样回传，不要自己拼。范围对不上的授权不算（同意了重载 OpenClash，不等于同意重载 ShellCrash；
  同意了用某个 IP 启动，换了 IP 要重新问）。
- 对话框只用事件里的 `title`、`question`、`detail`、`severity`、`default`。
- 授权只在这一次操作（连同它的重跑）里有效，不保存、不用于别的操作。

### 4.3 App 的重跑流程

1. 用新的 `CCA_EVENTS` 文件和到目前为止同意过的授权运行命令（第一次没有授权）。
2. 退出码 0：成功（没用上的 `consent` 事件忽略）。
3. 退出码非 0 且有还没授权的 `consent`：逐个弹框；全部同意就把它们的 `grant` 加进去回到 1；有一个不同意就停下，显示“没有同意，没有做任何改动”（不是错误）。
4. 退出码非 0 且有 `terminal`：提示“需要在终端里完成”，按钮【在终端里运行】【复制命令】【取消】。
5. 否则失败：显示最后一条 `fail` 的 `message`（按 `code` 给补救按钮，例如 `port_closed` → 同步路由器，`table_invalid` → 在终端里 `cca edit`），
   stderr 的最后几行放在“详情”里。
6. 最多重跑 3 轮；同一个授权被再次要求就停下（防止死循环）。

### 4.4 改别名：`cca [codex] rename KEY 新别名`

- 只换账号表里的键（值和顺序都不变）：官方账号按邮箱、第三方 API 账号按 `id` 认，账号目录、登录、浏览器资料、端口、“上次使用”都不变，
  路由器的同步摘要也不变（不用同步）；`~/Applications` 里的启动器跟着改名。不交互、不联网。
- 新别名写 `-` 去掉别名，只留邮箱（第三方 API 账号必须有别名）。别名的规则由 cca 判断（最多 12 个汉字或 24 个字母，不能有空格、引号、
  括号、反斜杠或 @，同一个工具里不能重名），App 不自己判断。
- 成功：退出码 0，stdout 只有一行新的键（App 选中它）。不行：退出码非 0，`fail` 事件的 `code` 是 `alias_invalid` 或 `account_missing`，
  `message` 是给人看的原因（App 显示在改名的弹层里，弹层不关）。

### 4.7 账号表的顺序：`cca [codex] reorder 账号…`

按这个顺序排账号表（没写到的账号接在后面，先后不变）。不交互、不输出，退出码 0 是成功。菜单、编号跟着变；端口池、路由器的摘要、启动器和头像的颜色
按调整之前的顺序（config.json 的 `order_base`，记账号目录名），不用同步路由器、不用重新加载本机 Clash。App 拖动账号、置顶之后写回：官方账号在前，第三方 API 账号在后，各组里和界面上一样（置顶的在前）。

### 4.8 请求维护者锁定节点：`cca [codex] policy request 账号`

别的机器只有只读 deploy key，改不了映射，只能请求：cca 写一条维护者直接运行的命令 `./policy.sh lock claude|codex 邮箱 节点`（账号和它现在的节点，不带别名，值按 shell 规则加好引号），用户发给维护者，维护者在 cca 的仓库里运行，确认后写进映射并发布。
不交互、只读（不改账号表和映射），stdout 是那条命令，退出码 0；App 把它复制到剪贴板（有事件文件时 cca 不碰剪贴板，命令行里 cca 自己复制）。
不能请求时（第三方 API 账号、这台机器没配置映射、已经被映射固定）退出码非 0，原因在 `fail` 事件里；`state` 账号的 `can.policy_lock` 是 false 时 App 不显示这个入口。
App 只复制、不解析。

### 4.9 定时拉维护者的映射：`cca policy pull --json`

配置了映射（`state.config.policy.configured`）、cca 够新（`hello.forms`）时 App 定时运行（低优先级，带事件文件，不弹框）：下次的时间是 `state.config.policy.next_check_epoch`（上次拉的时间 + 一小时，cca 算的，App 不重算；没拉过是 null），
App 开着就拉，不管窗口看不看得见（过热时跳过，下一次再试）。刚启动、到期了时至少隔 5 分钟，不空转。

- 成功：退出码 0，stdout 一行 `{"version":1,"kind":"policy_pull","generated_epoch":…,"changed":true,"summary":"Claude 1 个账号、Codex 0 个账号"}`。
  `changed`：这次拉到的映射（或参考节点）和上次不一样。拉到了就马上照映射改账号表，账号表和路由器的摘要对不上时同步路由器：要重载 / 重启的不做（`consent` 事件 App 忽略），账号表已经上传，
  `state.routing.sync` 留着 `pending`，用户点“同步”时再问；在外面连不上路由器时等回家。
- 拉不到：退出码 1，没有 stdout，`fail` 事件的 `code` 是 `policy_unavailable`，`message` 是原因（App 不打扰用户，到点再试；原因记在 cca 的日志里，`cca policy` 看得到）。
- 手动拉取（节点分流页的「拉取映射」、账号右键菜单和「…」里的「现在拉取映射」）是同一条命令，不降优先级，不看下次的时间，结果照实提示（`summary`；拉不到时弹框说原因）。要重载时（`consent` 事件）接着走 `router sync` 的重跑流程（4.3）：这次是用户点的，弹框问。
- 命令行的 `cca policy pull` 不带 `--json` 时和以前一样（只是多了 `checked` 的时间：启动时一天一次的后台拉取不用再拉）。

### 4.10 API key：`cca [codex] key KEY [--show | --forget]`

- 记下 / 更换（`cca [codex] key KEY`，不带参数）：提问会话（4.6，`key.secret`）。第三方 API 账号是换它启动要用的 key；官方账号是**记一把备查**的
  （比如控制台里建的 key：存进这台 Mac 的钥匙串，cca 绝不拿它登录或启动这个账号）。存好后 `result` 是 `key_saved`。
- 显示 / 复制（`--show`）：stdout **原样**是 key（不换行、没有别的字）；第三方 API 账号和启动时取的一样（账号表的 `key` 字段优先，再是钥匙串里
  这个服务商的）。**只经内存里的管道读**（不能接到文件，见 1），读完马上丢：不放进被观察的状态、不进提示、不进读屏、不进日志。
  没有：退出码 1，`fail` 的 `code` 是 `key_missing`；钥匙串读不了：`keychain_unavailable`。App 复制时自己写剪贴板：标成机密
  （`org.nspasteboard.ConcealedType`，剪贴板管理器不记），60 秒后剪贴板还是它就清掉，退出 App 前也清。
- 删除（`--forget`，只给官方账号）：要同意（4.2 的 `key_forget`，默认不删），删好是 `result` 的 `key_forgotten`；第三方 API 账号的 key 启动要用，
  不在这里删（`fail` 的 `api_account`）。删除账号（`rm`）、`restore` 时 cca 一起删。

### 4.5 重置额度：`cca codex reset KEY --json`

ChatGPT 账号（Codex 的官方账号）有重置机会时（`usage` 里 `reset_credits.availableCount > 0`），用掉一次就把 5 小时和每周的用量清零
（codex 里 `/usage` 的 Redeem reset）。和 `usage` 一样经账号自己的出口：

1. 先查一次还能重置几次（顺便存下用量）。没有机会了：`fail` 的 `no_reset_credit`；codex 太旧（不给次数、没有这个接口）：`reset_unsupported`；
   查不了时按原因：`not_logged_in` | `login_mismatch` | `local_unavailable` | `port_closed` | `region_restricted` | `no_exit_ip` | `error`。
2. 有机会：要同意（`consent`，`reset_credit:<账号目录名>`，默认不同意；`detail` 写着用掉哪一次、还剩几次），App 弹框同意后带上授权重跑。
   这条命令一旦开始就要跑完（见 1）：同意之后就会用掉一次。
3. 用掉最早过期的那一次（没有明细时由 OpenAI 挑），重置了就再读一次用量。stdout 输出：

```json
{"version": 1, "kind": "reset", "tool": "codex", "generated_epoch": 1794042060.0, "key": "frank@example.com",
 "outcome": "reset", "message": "已重置 frank@example.com 的额度（现在 5h 0% · 周 0%）", "usage": { "…3.1 的用量条目，重置后的…" }}
```

- `outcome`：`reset`（重置了）| `already_redeemed`（这一次早就重置过了，也算成了）| `nothing_to_reset`（用量还不需要重置，**没有扣次数**）；
  这三种退出码 0。`no_credit`（这一次已经不能用了）不输出文档：`fail` 的 `no_reset_credit`，退出码 1。已经请求重置、结果却不知道
  （超时、codex 出错）：`fail` 的 `error`，`message` 说可能已经用掉了一次、刷新用量看看。
- `usage` 是重置后的用量（读不到为 null：App 自己再刷新一次用量）；`message` 是给人看的一句话，App 照着提示。
  重置前后读到的用量都记进每周用量（2.2 的 `usage_weeks`）：这一周算中间重置过一次，重置前用掉的照样算进这一周。

### 4.6 提问会话：在 App 里问完的流程

添加账号这类要问好几个问题的命令，App 和命令行用**同一份流程、同一套校验**：一个 cca 进程跑完整条流程，每问一页就发一条 `ask` 事件，App 把这一页
显示成控件，答案写回管道。分支、默认值、校验都在 cca 里，App 里没有问题的文案和规则；cca 新增提问时 App 自动显示。

- 版本：`hello.prompt`（现在是 1）。没有这个字段、或者版本不认识：不开提问会话，走终端交接（6）。配置路由器、配置本机 Clash、账号表的表单另外要 `hello.forms`（现在是 1）：旧版 cca 这几条问的是终端里的那一套（比如 SSH 密码要在终端里输），没有这个字段时走终端。
- 怎么运行：和平时一样（stdin 接 `/dev/null`、自己一个会话、`CCA_EVENTS`），多两样：设 `CCA_PROMPT=1`；**fd 3 接答案管道的读端**。stdout / stderr 接文件
  （App 退出时 cca 不会被 SIGPIPE 杀掉），不限时。只给下面表里的流程用，不要给 `app`、`web`、`billing`、`limit-resets`、`usage`、`check`。
- `CCA_PROMPT=1`、`CCA_EVENTS`、fd 3 三样都有，cca 才进提问会话；缺一样就和平时的非交互一样（比如要终端的命令直接报错）。
- 同意（4.2）在提问会话里当场问，不用带 `CCA_GRANT` 重跑。

```jsonl
{"v":1,"ev":"ask","seq":1,"q":"add.basic","title":"添加 Claude 账号","message":"","fields":[{"id":"email","kind":"text","label":"登录邮箱","default":"","value":"","error":"","hint":"…","optional":false},{"id":"alias","kind":"text","label":"别名（可选）","default":"","value":"","error":"","hint":"…","optional":true}]}
```

- `seq`：这个事件文件里的第几页，答案要带上（重复点击发出来的旧答案序号对不上，cca 丢掉）。
- `q`：题号，全局唯一（下面的映射表）。App 不靠它决定显示什么：不认识的题号照样按字段显示。
- `title`、`message`：页标题和说明，可以为空。
- `fields[]`：这一页的字段。`id`（答案要带上）、`kind`、`label`、`default`、`value`（重问时上次填的，**密文永远是空的**）、`error`（上次的答案为什么不行，显示在输入框下面）、
  `hint`、`optional`，以及下表里各类型自己的。

| `kind` | 控件 | 答案 | 另外的字段 |
|---|---|---|---|
| `text` | 输入框 | 文字（一行） | |
| `secret` | 密文框 | 文字（一行），不回显、不重填 | |
| `choice` | 单选 | 选中项的 `value`；`optional` 时可以空着（“不选”的说法在 `skip`） | `options: [{value, label, note}]`，`default` 是预选项的 `value` |
| `multi` | 多选 | 选中项的 `value`，空格隔开 | `options`、`min`（至少选几个） |
| `confirm` | 是 / 否两个按钮（一页只有它） | `y` / `n` | `detail`、`severity`；`default` 是回车时的答案 |
| `code` | 只显示的一段多行文字（`value`），带“复制” | 不用回答（不写答案） | |

字段还可以带：`section`（分组的小标题，相邻的同一组放在一起）、`style`（`choice` 写 `menu` 时是一行里的弹出菜单，字段多的表单用；选中只有一个选项时只显示它）。页面还可以带 `submit`（提交按钮的说法，空的是“继续”）。这几个都是新加的，旧版 App 忽略它们照样能显示。

答案：每个字段一行 `<seq> <字段 id> <值>\n`（只有一个字段的页，字段 id 就是 `v`），按 `fields` 的顺序写进 fd 3；值里不能有换行。一页的字段对不上（id 不对）时 cca 按取消处理，免得把值放错地方。
App 只拦空着的必填项，别的校验都是 cca 做的：不合格时同一个 `q` 带着 `error` 和已填的值再发一条 `ask`。

- 取消：App 关掉答案管道。cca 读到 EOF 就发 `fail cancelled` 然后退出（SIGTERM），**绝不当成回车取默认值**（默认“是”的确认不能因为 App 退出就被同意了）。App 崩溃、退出时同样。
- 显示不了的一页（字段类型不认识、选择题没有选项）：App 关掉管道，失败页上有“改在终端里完成”，不能漏掉问题。
- 等待（cca 在干活，没在问）：App 显示 stderr 最新的一行，原样显示，不解析。超过 30 秒没结束时给“停止”按钮（给进程组发 SIGTERM）。
- 结束：退出码 0：做完了（有 `result` 事件时显示它的 `message`，带 `key` 时选中这个账号；没有 `result` 表示用户选了不做）；退出码 0 但有 `fail` 事件：做完了，要提醒（比如路由器没同步成）。
  退出码非 0：有 `terminal` 事件就走终端交接，否则显示最后一条 `fail`。
- 密钥（`secret` 字段）只经答案管道：不进命令行、环境变量、事件文件、输出文件。cca 读进没有 export 的变量，校验和存储都经管道给 python；错误文案是固定的，不回显输入。
  App 里密文不放进被观察的状态，交出去马上清掉。**第三方 API 的 key 会短暂经过 App 的内存**（这是用户选的：比终端里输入方便，换来这一点暴露面）。

题号（提问）和控件的对应（`cca` 里新加一个提问时要写进这张表，回归的 lint 守着）：

| 题号 | 命令 | 控件 | 问什么 |
|---|---|---|---|
| `add.basic` | `[codex] add` | `text` ×2 | 登录邮箱（校验 email）、别名（可选，校验 alias） |
| `add.node` | `[codex] add` | `choice` | 出口节点。映射固定了这个邮箱的节点时不问 |
| `api.basic` | `[codex] add api` | `text`、`text`、`secret`、`text` | 别名、接口地址（校验 url）、API key、模型（可选，校验 model） |
| `api.node` | `[codex] add api` | `choice` | 出口节点（第一项是直连）。没有可选节点时不问 |
| `key.secret` | `[codex] key 账号` | `secret` | API key（第三方 API 账号：换 key；官方账号：记一把备查的，页上的 `message` 说明 cca 不拿它启动） |
| `rm.confirm` | `[codex] rm 账号` | `confirm`（默认否） | 确认删除 |
| `node.replace` | 同步路由器的流程里 | `choice`（可以不选） | 节点在路由器上不见了，换成哪个 |
| `router.basic` | `router setup` | `text` ×3 | 路由器地址（校验 host）、SSH 用户（校验 sshuser）、SSH 端口（校验 port），预填现在的；连不上时带着原因重问 |
| `router.password` | `router setup` | `secret` | SSH 密码（只在密钥还不能登录时问；错了带着原因重问）。只经答案管道：cca 用 `SSH_ASKPASS` 交给 ssh，经私有目录里的 FIFO，读一次 |
| `router.nodes` | `router setup` | `multi`（至少一个） | 账号可用的出口节点（现在的预先选好；还有账号在用的不能去掉） |
| `local.verge` | `local setup` | `confirm` | 有 Clash Verge 时：配置它吗 |
| `local.client` | `local setup` | `choice` | 没有 Clash Verge 时：内置代理（默认）或手动配置别的客户端 |
| `local.manual` | `local setup` | `code`、`text`、`secret`（可选） | 要加的配置；外部控制地址（校验 controller）、密钥。连不上时带着原因重问 |
| `edit.form` | `[codex] edit` | `choice`（`menu`）、`text` | 账号表的常用项，一页：每个官方账号的节点（维护者固定的只有一个选项，答案不算数）；第三方 API 账号的接口地址、模型、出口、环境变量（只有 Claude 的有）。不显示 key。见下 |
| `consent.<item>` | 流程里需要同意时 | `confirm` | 4.2 的同意，默认值和 `severity` 不变 |

账号表的表单（`edit.form`）：字段 id 是 `n<编号>`（节点）、`u<编号>`（接口地址）、`m<编号>`（模型）、`e<编号>`（环境变量），编号是账号在表里的位置，App 不解释。不合格时同一个 `q` 带着 `error` 和已填的值再发一条，都合格才写账号表；
`message` 里可能说账号表刚刚变了（映射更新、别处改过）：这时显示的是最新的内容，再确认一次。接口地址换了域名时，账号表里写的 `key` 字段删掉（是旧服务商的），保存后当场问新的 key（`key.secret`）。
App 不碰账号表文件，也看不到 key；表单改不了的问题（不是 JSON、键写错了、有不认识的字段）发 `terminal` 事件（`code` 是 `edit`）走终端。

命令行功能和 App 的对应：

| 命令 | App 里 | 方式 |
|---|---|---|
| `[codex] add`、`add api` | 工具栏「＋」、总览各组的「添加」、侧栏「启用 ChatGPT 账号」 | 提问会话 |
| `[codex] key` | 账号页「…」、右键菜单、API key 那一行的「记录…」「输入…」 | 提问会话 |
| `[codex] key KEY --show`、`--forget` | API key 那一行的眼睛、「复制」；「…」、右键菜单里的「删除记下的 API key…」 | 无交互命令（4.10；`--show` 只经内存里的管道读，`--forget` 要同意） |
| `[codex] rm` | 账号页「…」、右键菜单 | 提问会话 |
| `[codex] rename` | 账号页的名字 | 无交互命令（4.4） |
| `[codex] reorder` | 拖动账号、置顶 | 无交互命令（4.7） |
| `[codex] policy request` | 账号页「…」、右键菜单 | 无交互命令（4.8） |
| `policy pull --json` | 定时（App 开着时一小时一次）、节点分流页「拉取映射」、账号的「现在拉取映射」 | 无交互命令（4.9） |
| `[codex] devices KEY --json` | 账号页的「设备」（看得见时每分钟一次、「刷新」） | 无交互命令（3.4） |
| `[codex] edit` | 工具栏「＋」、账号表校验不过的提示 | 提问会话（表单）；表单改不了时走终端 |
| `router setup`、`local setup` | 工具栏「＋」、分流页、失败页的补救按钮、`terminal` 事件 | 提问会话（要 `hello.forms`；旧版 cca 走终端） |
| `[codex] usage`、`check`、`app`、`web`、`billing`、`limit-resets`、`codex reset`、`router sync`、`update`、`clients`、`gui update` | 各自的按钮 | 无交互命令 + 同意重跑 |
| `[codex] login` | 账号页「登录…」 | 终端 |
| `install` | 引导页 | 终端 |
| `router uninstall`、`local off`、`policy`（查看、配置） | 没有入口 | 终端里自己运行 |
| `@账号`、`router ssh` | 账号页「终端」 / 没有入口 | 终端（本身就是终端程序） |
| `restore` | 没有入口 | 终端（要求 cca App 先退出） |

## 5. `cca gui`

- `cca gui`：打开 App；没装、或者版本和 cca 不同，先下载装好（只装和当前 cca 版本号相同的 App）。
- `cca gui update [--relaunch PID]`：下载校验好和 cca 同版本的 App；App 开着时先放着（`hello.gui.staged_version`），等它退出再换；
  给了 `--relaunch PID` 就等这个进程退出（最多 60 秒）后换上并重新打开。
- `cca gui` 和重新打开时带 `--args --window`：App 平时在后台启动，只有这样启动才开主窗口。App 已经开着时 `open` 不传参数，App 收到“重新打开”，同样显示主窗口。
- App 的「更新并重新打开」分两步：先照常运行 `cca gui update`（不带 `--relaunch`，等它结束，失败就显示原因、App 照常开着）；成功后
  （新版本已经下载好放着）再脱离 App 运行 `cca gui update --relaunch <自己的 pid>`（不设 CCA_EVENTS、不限时，输出接到文件），然后退出。
- `cca gui remove`：删掉 App。
- 发布：每个版本的 GitHub Release 附带 `cca-app.zip` 和 `cca-app.zip.sig`（`ssh-keygen -Y sign`，命名空间 `cca-app`，和 cca 本身的签名
  `cca-release` 分开）；装在 `~/Applications/cca.app`（bundle id `local.cca.app`）。用 curl 下载、校验后解开，不经浏览器，所以没有隔离标记，
  Gatekeeper 不拦；自己用浏览器下载 zip 会被拦。
- 自动更新：cca 自动更新后，下一次运行 cca 时在后台下载对应版本的 App（App 开着就先放着）。
- 开发模式（cca 旁边有 `.git`）：`cca gui` 用 `macos/build.sh` 从源码编译，打开 `macos/build/cca.app`，不装进 `~/Applications`。

### 5.1 检查更新：`cca update --check --json`

只读，要联网（GitHub）。按现在装的版本挑：**测试版上看更新的测试版和正式版，正式版上只看正式版**（自动更新、不带版本号的 `cca update`
不变，只装正式版）。stdout 只有 JSON；一个版本都拿不到（网络不通）时退出码 1，原因是 `fail` 事件的 `message`（App 带着事件文件运行它）。

```json
{"version": 1, "kind": "update", "generated_epoch": 1794042000.0, "cca_version": "2.9.0-beta.10", "channel": "beta",
 "source": "releases", "latest": "2.9.0-beta.11", "available": true, "notes": "cca 2.9.0-beta.11：…",
 "url": "https://github.com/rivia7/claude-accounts/releases/tag/v2.9.0-beta.11", "published_epoch": 1794038400.0}
```

- `channel`：`stable` | `beta`。`latest`：这个版本跟着的最新版本（不一定比现在新）；`available`：比现在新。版本号的先后和 cca 一样：
  同号的测试版比正式版旧（`2.9.0-beta.12` 比 `2.9.0` 旧），测试版之间比 beta 后面的数。
- `source`：`releases`（GitHub 的发布列表，有更新说明）| `latest`（列表拿不到、被限流时退回 `releases/latest`：只看得到正式版，`notes` 为 null）。
- `notes`：GitHub Release 的更新说明（原样）；`url`：发布页。开发模式看 `hello.dev_mode`：App 不自动查、不提供安装（用 git pull 更新）。
- App：窗口或菜单栏面板看得见时一天最多自动查一次（`state.config.update` 是 `off`、开发模式时不自动查；查不了的 3 小时后再查），
  也可以手动「检查更新…」。只有 `latest` 比 `hello.cca_version` 新（按上面的先后）才提示。
  有新版本时提示「更新并重新打开」：先运行 `cca update <latest>`（照常带事件文件；失败就显示原因，什么都没换），成功后照 5 的两步换上
  同版本的 App、重新打开。「以后再说」只记在 App 自己的设置里（这个版本不再提示，手动检查时照样提示）。

## 6. 终端交接

`terminal` 事件、`hello.installed == false`、账号的「终端」按钮（`cca [codex] @账号`）、以及还没搬进提问会话（4.6）的入口（cca 不支持提问会话时也是），都在终端里运行对应的 cca 命令：App 在
`~/Library/Caches/local.cca.app/handoff/` 写一个临时的 `.command` 文件（`#!/bin/bash`，先把自己挪走再删掉，补好 `PATH`，去掉 `CCA_GUI` 等 App 给 cca 的变量，要换文件夹时先 `cd`，再运行逐个加了引号的
命令（cca 用找到的绝对路径），结束后在终端里等按任意键，退出码就是命令的；账号的会话结束后留在用户的登录 shell 里），`chmod 755` 后打开：账号的「终端」按钮用用户选的终端，别的命令都用系统自带的“终端”。都不需要“自动化”权限。
一小时前留下的 `.command` 文件 App 会清掉。用户回到 App（App 被激活）时重新读一次 `state`。

用的是系统自带的“终端”时，按了任意键（或者点了关闭、^C）以后脚本关掉这个窗口（窗口里只有这一个标签页时）；“终端”没有别的窗口了就退出它，免得它一直开在程序坞里。
`hello.dock` 有时再运行 `cca _dock-tidy <终端.app 的位置> <终端的进程号>`：cca 把活交给它自己的后台小 App（`~/.cca/dock/cca-dock.app`，经 LaunchServices 启动、`LSBackgroundOnly`，
不在程序坞里出现）就返回；小 App 等“终端”退出、LaunchServices 里没了它的记录，再去掉「最近使用的 App」里它留下的图标、重启一次程序坞（图标栏消失再出来，不到一秒）。
从这个窗口里起的进程都算“终端”的下属，还活着时 LaunchServices 留着“终端”的记录（`lsappinfo` 里是 `exited-with-subordinates`），重启的程序坞照着它又把“终端”加回去，
所以关窗口的这一段除了交出去什么都不留下。cca 打开的账号 Chrome、Claude / ChatGPT App 退出以后 cca 自己同样整理，不经 App。

账号的会话用哪个终端、从哪个文件夹启动（最近用过的，所有账号共用），都记在 App 自己的设置里，不进 `~/.cca`，cca 不知道也不用知道。
App 按 bundle id 看这台 Mac 上装了哪些，没装的在选择列表里灰着、不能选。全自动：系统自带的“终端”、iTerm2、Warp、WezTerm、Hyper、cmux（把 `.command` 文件交给它们）、Ghostty、kitty、
Alacritty、Tabby、Rio（开新进程、参数里带上脚本：Ghostty、kitty 打开文件每次都要确认，Alacritty 不能打开文件，Tabby 每次运行前自己弹窗确认）。半自动：Termius、Wave 没有从外面运行命令的办法，
App 打开它，把 `cd 文件夹 && cca …`（cca 用绝对路径）放进剪贴板，用户在它的终端里粘贴运行，不写交接文件。没选过时用第一个装了的全自动第三方终端，都没有就是系统自带的；半自动的要用户自己选。
第三方终端 30 秒还没运行脚本（`.command` 还在）时，App 把它收回
（rename，和脚本开头的 `mv` 只有一边成功，不会跑两遍），弹窗问要不要改用系统自带的“终端”。

## 7. 内置代理（`cca _core run`）

在外面、这台 Mac 上又没有 Clash 客户端时，cca 用自己带的 mihomo 内核（`state.config.local.client == "builtin"`）。端口池和本机
Clash 的一样（`127.0.0.1:7901…`），配置、节点、切换都由 cca 管；**App 只负责让内核进程一直跑着**：

- `state` 里本机客户端是 `builtin` 时，App 运行 `cca _core run`：自己一个会话，stdin 接 `/dev/null`，不设 `CCA_EVENTS`，不限时，
  stdout / stderr 追加进 `~/Library/Caches/local.cca.app/core.log`（启动前超过 1 MB 就清空）。cca 照账号表生成配置后 `exec` 内核，
  所以这个进程就是内核本身。
- 进程退出了就隔一会儿重启：1、2、4…秒，最多 60 秒；跑够 60 秒才退出的从 1 秒重新算。重启前显示一条提示（退出码、日志最后几行），
  可以“现在重启”。
- 不再是 `builtin`（`cca local off`、换了客户端）时停掉：cca 改好 `config.json` 后删掉 `core/config.yaml`，App 看到后重新读 `state`、停掉内核
  （cca 会等它停下，免得旧内核还占着端口）。App 退出前停掉并等它退出（SIGTERM 进程组，3 秒后 SIGKILL）。App 崩溃时留下的内核，
  下一次 `cca _core run` 会按它的 pid 文件认出来停掉。
- cca 需要内核、App 又没开着时，在后台打开 App：`open -g <App> --args --core`。App 平时就在后台（只有菜单栏图标），带 `--core` 时什么情况下都不自己弹主窗口；
  用户从菜单栏打开主窗口时照常显示。App 开着时 cca 不再 `open`（会弹出主窗口），而是写 `core/config.yaml`：
  App 看到它变了就重新读 `state`、接着起内核（看不见时也读）。
- 演示数据（`CCA_GUI_FIXTURES`）不运行内核。

## 8. 客户端更新（桌面客户端和终端工具）

管理 `com.anthropic.claudefordesktop`（Claude）和 `com.openai.codex`（ChatGPT）两个桌面客户端，以及终端里的 Claude Code、Codex 命令；和 cca 自身的 `update` 命令分开。

- `cca clients status --json`：只读本机状态，启动和进入客户端更新页时调用。
- `cca clients check [--force] --json`：检查并准备更新，不安装。后台不带 `--force`，每个客户端 6 小时最多查一次，节流由 cca 负责；用户点“立即检查”时加 `--force`。
- `cca clients mode auto|off --json`：设置自动检查。默认 `auto`，含义仅是自动检查和准备软件包。
- `cca clients install TOKEN --json`：用户明确点击安装后调用；cca 返回 `client_update` consent，grant 为 `client_update:TOKEN`，默认不同意。
  TOKEN 绑定检查时的目标和软件包哈希，App 原样传递，不解析或生成。目标变化后原授权失效，必须重新检查。

以上命令均返回以下文档；不支持或内部错误时可能只有 fail 事件。字段缺失、null 和未知状态宽松处理。

```json
{
  "version": 1, "kind": "client_updates", "generated_epoch": 1794042000,
  "mode": "auto", "next_check_epoch": 1794063600,
  "clients": [{
    "id": "codex", "label": "ChatGPT", "path": "/Applications/ChatGPT.app",
    "installed_version": "1.0", "available_version": "1.1", "staged_version": "1.1",
    "status": "staged", "message": "软件包已准备好，等待确认安装。", "checked_epoch": 1794041940,
    "install_token": "opaque-checked-target-and-package-hash", "can_install": true,
    "package_version": "1.1", "package_build": "1100"
  }]
}
```

- `clients` 里 `id=claude`、`id=codex`（`kind=desktop`）、`id=claude-cli`、`id=codex-cli`（`kind=cli`）各一行（`status=missing` 是没装，UI 不显示）；UI 按数组显示，不因查询客户端状态而启用 Codex 账号。
- 终端工具（`kind=cli`）只查版本、不下载软件包：`status` 里的现在版本是现跑 `命令 --version`（空环境），最新版本从官方的公开地址查（官方安装器：Claude Code 是 `downloads.claude.ai` 的发布通道，跟用户设的 `autoUpdatesChannel`；Codex 是 GitHub 的 latest release；npm、Homebrew 装的查 npm、Homebrew）。有新版本时 `status=available`，`available_version` 是新版本。官方安装器装的 `can_install=true`，`install` 运行它们自己的 `update` 命令（空环境、真 HOME；claude 用一次性的配置目录，不碰账号）；npm、Homebrew 装的 `can_install=false`，`message` 写着在终端里运行什么。
  一个客户端出问题写在它自己那一行，不影响另一个；`check`、`install` 有问题时退出码 1。`install` 的 TOKEN 只对它那一个客户端有效。
- `status`：`missing/current/checking/busy/unsupported/staged/available/running/blocked/error/updated/unverified/needs_recheck`，以后可增加；原因直接显示 `message`。
  `busy` 显示「正在更新」，`unsupported` 显示「这个系统暂时没有可用的安装包」。
- `can_install` 与非空 `install_token` 决定安装按钮是否可用。`needs_recheck` 清空旧 token，UI 显示「要安装的版本变了，请重新检查后再安装」。
- `next_check_epoch`：下次该后台检查的时间（模式为 off、一个客户端都没装时为 null）。GUI 运行期间睡到这个时间再运行 `clients check`，窗口隐藏时也继续，不隔一会儿起一个进程；已经到了、这次没查成时 15 分钟后再来；
  没有这个字段时每小时看一次。模式为 off、开发模式或演示数据时不自动检查，低电量模式、系统过热时往后推。定时器不发安装命令，重入时跳过；后台检查以低优先级（utility）运行。
- 安装仅在用户点击并同意后进行，所有对应客户端进程退出后才允许替换；不强杀进程，不注册 LaunchAgent，不修改原生 updater 设置。

### 8.1 在解压前验证官方归档签名

已安装、签名校验通过的 `local.cca.app` 在 Info.plist 声明整数 `CCAClientUpdateVerifier=1`。cca 先验证该能力字段，
不能通过启动旧版 App 探测未知参数。安装版调用 `~/Applications/cca.app/Contents/MacOS/cca`；开发测试调用新编译的
`macos/build/cca.app/Contents/MacOS/cca`：

```
cca.app/Contents/MacOS/cca --verify-client-update /absolute/package.zip BASE64_SIGNATURE
```

该入口在 NSApplication 初始化前退出，不打开窗口，不执行归档内代码。使用 CryptoKit Ed25519、映射只读文件，固定公钥
`mNfr1v9t63BfgDtlw4C8lRvSY6uMggIXABDOCi3tS6k=`，不接受参数或环境替换。退出 0 表示签名有效，1 表示签名或读取失败，2 表示参数错误。
官方签名只证明归档来源和完整性，不代表所有隔离行为已经兼容审计；解压和后续安装检查由后端完成。

Claude 的更新源（`https://api.anthropic.com/api/desktop/darwin/<架构>/squirrel/update`，Squirrel 的 json 格式，204 是没有更新）没有 Ed25519 签名，给的是
SHA-256 和大小：cca 下载后比对，解压后同样检查代码签名、Team ID、产品标识、版本和 Gatekeeper，不需要 cca.app 验签。这个接口要求带 `device_id`：每次现生成一个，
不用也不存账号实例里的标识。下载地址只认 `downloads.claude.ai`，增量包不用。
