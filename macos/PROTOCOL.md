# cca App ⇄ cca 的接口（GUI_API 1）

cca App（`macos/`）只经这里写的接口和 cca 打交道：**不读 `~/.cca` 里的文件、不解析中文输出、不在 Swift 里重算 cca 的规则**
（账号目录名、端口、维护者映射、用量是否过期……都由 cca 算好给出）。改这里的格式要同时改 cca、Swift 模型
（`macos/Sources/CcaKit/Protocol/`）和回归测试（`tests/regress.sh` 的 10g 节）。

兼容规则：

- 只能**加**字段、加枚举值、加事件类型；App 遇到不认识的字段、枚举值、事件类型一律忽略（枚举按字符串收，不认识的当“其它”）。
- 删字段、改含义才升 `GUI_API`（`cca _gui hello` 的 `api`）。App 声明自己支持的范围，不在范围内就提示更新。
- 每份 JSON 文档都带 `"version": 1` 和 `"kind"`。时间一律是 Unix 秒（浮点数），字段名以 `_epoch` 或 `_at` 结尾
  （`usage --json` 里沿用的 `fetched_at`、`generated_at`、`resets_at` 是旧字段，保留原样，App 用对应的 `*_epoch`）。
- JSON 只写到 stdout；给人看的文字照旧写 stderr，App 只在“详情”里原样显示，不解析。

## 1. 怎么运行 cca

- 可执行文件：`~/.local/bin/cca`，没有就 `~/.cca/bin/cca`；开发模式下 `cca gui` 会用 `open --env CCA_GUI_CCA=<路径>` 告诉 App。
- 每次调用都是一个新进程：stdin 接 `/dev/null`，自己一个会话（`POSIX_SPAWN_SETSID`），取消时给整个进程组发 SIGTERM，3 秒后 SIGKILL。
  这样 cca 永远打不开 `/dev/tty`，所有需要回答的地方都按“非交互”处理。
- 环境：继承 App 的环境，去掉 `CCA_TTY`、`CCA_YES`、`CCA_SKIP_CHECK`、`CCA_GRANT`、`CCA_EVENTS`、`CCA_RO`；`PATH` 后面补上
  `$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin`；设 `CCA_GUI=1`、`NO_COLOR=1`；按需设 `CCA_EVENTS`、`CCA_GRANT`（见 4）。
- **绝不**设 `CCA_TTY`、`CCA_YES`、`CCA_SKIP_CHECK`，绝不调用 `cca _api-key`。
- `cca codex …` 只在 `hello.tools.codex.enabled` 为真时才能调用：没有 `~/.cca/codex.json` 时运行任何 `cca codex …` 都会把它建出来，
  维护者映射里的 Codex 账号随即被自动加进来、分走端口。
- 同一个账号（工具 + 键）的 `app` / `web` / `rename` 一次只跑一个（同一个资料目录绝不能开两份）；`usage` 每个工具一次一个；同一个账号的 `reset`
  一次一个；`router sync` 全局一次一个；`update 版本号`、`gui update` 全局一次一个（一个接着一个）。`_gui hello|state|live`、`update --check --json`
  随时可以跑。
- 建议的超时：`hello` 5 秒、`state` 15 秒、`live` 30 秒、`usage` 150 秒、`check` 180 秒、`app` / `web` 120 秒、`router sync` 180 秒、`gui update` 300 秒、
  `rename` 60 秒、`update --check --json` 150 秒、`update 版本号` 300 秒。
- `app`、`web`、`router sync`、`reset` 一旦开始就要让它们跑完：App 退出（或崩溃）时它们不能跟着被杀（cca 往 stderr 写字时管道没人读会被 SIGPIPE 杀掉，
  可能 App 没打开、路由器同步了却没记下；重置额度用掉了一次却没记下重置后的用量）。所以这几条命令的 stdout / stderr 接到文件，跑完再读，不限时；
  进度和结果看事件文件。
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
  "version": 1, "kind": "hello", "api": 1,
  "cca_version": "2.9.0", "dev_mode": false, "installed": true,
  "self": "/Users/me/.cca/bin/cca", "home": "/Users/me/.cca",
  "tools": {
    "claude": {"enabled": true, "bin": "/Users/me/.local/bin/claude"},
    "codex": {"enabled": false, "bin": null}
  },
  "gui": {"app": "/Users/me/Applications/cca.app", "app_version": "2.9.0", "staged_version": null}
}
```

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
    "policy": {"configured": true, "repo": "example/cca-policy"}
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
  "can": {"app": true, "web": true, "usage": true, "check": true},
  "can_why": {},
  "usage": { "…用量条目，见 3.1…" },
  "health": { "…出口检测结果，见 3.3…" }
}
```

第三方 API 账号：`kind: "api"`、`login: "api"`、`email: ""`、`plan: ""`、`plan_tier: ""`、`badge: null`、`api: {"host": "api.example.com", "model": "model-x"}`
（**绝不**带 key）；直连的没有端口：`node`、`port`、`slot`、`group` 都是 null；走节点的照常有。`usage` 恒为 null。

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
  `not_logged_in`（没登录，查不了用量）、`tool_missing`（没装 claude / codex）。
- `usage`：最近一次的用量（缓存，不联网）。只给 `login == logged_in` 的官方账号；没有缓存为 null。
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
`web`：`api_account` | `no_chrome`；`usage`：`api_account` | `tool_missing` | `not_logged_in`；`check` 总是可用。

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

### 3.1 `cca [codex] usage [账号] --json`

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
  "extra_usage": null
}
```

- `source`：`live`（这次查到的）| `cache`（没查到，用上次的）| `none`（没有数据）。
- `error_code`（`error` 是给人看的文字）：`login` | `mismatch` | `route` | `port` | `region` | `trace` | `unsupported` | `timeout` | `nolimits` | `error`，没出错为 null。
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
- 退出码 1 表示没有一个账号有数据（JSON 照样输出）。查询会经各账号的出口，最慢 2 分钟左右。
- `_gui state` 里的 `usage` 是同样的条目（`source` 是 `cache` 或 `none`）。

### 3.2 `cca [codex] check [账号] --json`

人看的文字照旧写 stderr，stdout 输出：

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
  "leak": {"dns": "ok", "webrtc": "ok", "line": "DNS 无泄露 · WebRTC 无泄露", "dns_message": "", "webrtc_message": ""}
}
```

- `source`：`launch` | `check`。`kind`：`exit`（官方账号：claude.ai / chatgpt.com 看到的出口）| `api`（第三方 API 账号：接口连不连得上）。
- `route`：`router` | `local` | `direct`（直连的第三方 API 账号，不经端口）| null。`port`、`node` 直连时为 null。
- `result`：`ok` | `low_purity`（纯净度低于设定）| `restricted`（出口在不可用地区，`restricted_region` 是地区名）| `no_exit`（拿不到出口 IP）|
  `no_score`（检测服务没给评分）| `port_closed` | `api_ok` | `api_unreachable`。
- `trust`：0–100，没有评分为 null。`stale`：超过 6 小时。`applies`：测的时候用的节点和账号现在的节点相同（换了节点就是旧结果）。
- `leak`：没测（关掉了泄露检测、出口本身就不对）为 null。`dns`：`ok` | `leak` | `failed` | null；`webrtc`：`ok` | `leak` | null；
  `*_message` 是泄露时给人看的说明。

## 4. 事件和同意（`CCA_EVENTS`、`CCA_GRANT`）

### 4.1 事件文件

运行会改东西的命令（`app`、`web`、`check`、`usage`、`router sync`、`gui update`、`rename`、`reset`、`update 版本号`）和检查更新（`update --check --json`，
查不了的原因看 `fail` 事件）时，App 给一个新的空文件路径 `CCA_EVENTS=<文件>`，
cca 往里追加 JSON 行（一行一个事件）。不设就什么都不写；写不了也不影响命令本身。cca 的退出码、stderr 和不设时完全一样。

```jsonl
{"v":1,"ev":"step","code":"preflight"}
{"v":1,"ev":"consent","item":"router_reload","scope":"OpenClash","grant":"router_reload:OpenClash","title":"需要重载 OpenClash","question":"现在重载吗？","detail":"…","severity":"warn","default":"yes"}
{"v":1,"ev":"terminal","code":"login","argv":["login","alice@example.com"],"display":"cca login alice@example.com","message":"alice@example.com 还没登录…"}
{"v":1,"ev":"fail","code":"port_closed","message":"端口 192.0.2.10:7901 不通。为避免走错出口，已停止。…"}
{"v":1,"ev":"result","code":"opened","route":"router","node":"US-A","port":7901,"message":"已打开 alice@example.com 的 Claude App，出口 US-A（端口 7901）"}
```

- `step.code`（打开 App 的进度）：`sync`（同步路由器）| `preflight`（检测出口）| `prepare`（准备账号目录）| `open`（打开 App）。
- `result.code`：`opened` | `activated`（已经开着，切到了前面）。`result.route`：`router` | `local` | `direct` | null。
- `fail.code`：`error`（其它）| `app_missing` | `api_account` | `tool_missing` | `table_invalid` | `sync_failed` | `no_router` | `port_closed` |
  `local_unavailable` | `node_missing`（在外面时本机 Clash 里没有这个节点）| `settings_conflict` | `region_restricted` | `no_exit_ip` |
  `low_purity` | `not_logged_in` | `login_mismatch` | `api_unreachable` | `open_failed` | `app_profile_unsupported` |
  `alias_invalid`（改别名：新别名不合规、和别的账号重了、第三方 API 账号不能去掉别名、键里没有邮箱的官方账号不能改）|
  `account_missing`（改别名：没有这个账号）| `no_reset_credit`（重置额度：没有能用的重置机会了）|
  `reset_unsupported`（重置额度：这个版本的 codex 不支持，升级后再试）。可能有好几条，最后一条是最终原因。
- `terminal`：这一步要在终端里完成（登录之类）。`argv` 是 `cca` 后面的参数（Codex 账号带 `codex`），`display` 是给人看、能直接复制进终端的
  完整命令（只有字母数字和 `@%+=:,./_-` 的参数照写，别的加单引号，例如 `cca login '工作(bob@example.com)'`）。
  `code`：`login` | `router_sync` | `router_setup` | `local_setup` | `api_key`。

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

## 5. `cca gui`

- `cca gui`：打开 App；没装、或者版本和 cca 不同，先下载装好（只装和当前 cca 版本号相同的 App）。
- `cca gui update [--relaunch PID]`：下载校验好和 cca 同版本的 App；App 开着时先放着（`hello.gui.staged_version`），等它退出再换；
  给了 `--relaunch PID` 就等这个进程退出（最多 60 秒）后换上并重新打开。
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

`terminal` 事件、`hello.installed == false`、以及 App 里“添加账号”之类的入口，都在“终端”里运行对应的 cca 命令：App 在
`~/Library/Caches/local.cca.app/handoff/` 写一个临时的 `.command` 文件（`#!/bin/bash`，补好 `PATH`，去掉 `CCA_GUI` 等 App 给 cca 的变量，先删掉自己，再运行逐个加了引号的
命令（cca 用找到的绝对路径），结束后在终端里等按任意键，退出码就是命令的），`chmod 755` 后用“终端”打开。不需要“自动化”权限。
一小时前留下的 `.command` 文件 App 会清掉。用户回到 App（App 被激活）时重新读一次 `state`。

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
- cca 需要内核、App 又没开着时，在后台打开 App：`open -g <App> --args --core`。带 `--core` 启动的 App 不弹主窗口、不占 Dock，
  只留菜单栏图标；用户从菜单栏、Dock 打开主窗口时照常显示。App 开着时 cca 不再 `open`（会弹出主窗口），而是写 `core/config.yaml`：
  App 看到它变了就重新读 `state`、接着起内核（看不见时也读）。
- 演示数据（`CCA_GUI_FIXTURES`）不运行内核。
