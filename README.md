# cca：一台 Mac 同时用多个 Claude、Codex 账号

有好几个 Claude 账号（比如个人号和工作号）？cca 让它们在同一台 Mac 上各用各的（OpenAI 的 Codex 也一样，见「Codex 账号」一节）：

- **各自登录**：每个账号单独登录、互不影响，切换账号不用退出重登。
- **各自的设备标识**：登录前为每个账号准备独立、持久的客户端设备标识，重新登录、改别名或换节点时保持不变。
- **各走各的网络**：每个账号的流量从你指定的代理节点出去，出口 IP 固定，不会混用。在家走路由器；出门在外连不上路由器时，自动改走 Mac 上的 Clash 客户端（例如 Clash Verge）。
- **工作资料共用**：对话记录、记忆、设置、插件、技能、MCP 等，所有账号共用一份。在一个账号里配好的东西，换个账号照样能用。
- **一眼看清用量**：`cca usage` 列出每个账号的 5 小时、本周额度用了多少、什么时候重置，挑个还有余量的账号接着干。
- **第三方 API 也能当账号用**：兼容 Anthropic 接口的服务（填地址和 key），和官方账号一起出现在菜单里，可以直连，也可以走节点。
- **Codex 也能多账号**：`cca codex` 管 ChatGPT 账号的 Codex（命令行和 ChatGPT 桌面 App），同样各自登录、各走各的出口，工作资料共用 `~/.codex`。

终端里的 Claude Code（`claude` 命令）、网页版 claude.ai 和 Claude 的 Mac 桌面 App 都支持（第三方 API 账号只用于 Claude Code）。Codex 那边支持 `codex` 命令、网页版 chatgpt.com 和 ChatGPT 桌面 App（Codex 的第三方 API 账号只用于 `codex` 命令）。

## 需要准备什么

- 一台 Mac
- 装好 Claude Code（没装的话先运行 `curl -fsSL https://claude.ai/install.sh | bash`）
- 用 Codex 的话：装好 `codex` 命令（npm、Homebrew 或 OpenAI 的安装脚本都行，cca 会在 PATH、`~/.local/bin`、`/opt/homebrew/bin`、`/usr/local/bin` 里找）；要用桌面版的话装好 ChatGPT App
- Google Chrome（登录和网页版都用它）
- 局域网里有一台装了 **OpenClash** 或 **ShellCrash** 的 OpenWrt 路由器（常说的"旁路由"），开着 SSH，你知道它的 IP 和 root 密码（只用直连的第三方 API 账号的话不需要，见「第三方 API 账号」一节）。用的是哪个 cca 会自动认出来；ShellCrash 要 1.9 或更新的版本、用 Meta（mihomo）内核，路由器上还要装好 ruby（`opkg update && opkg install ruby ruby-yaml`，OpenClash 自带）
- 出门在外也要用的话：Mac 上装 **Clash Verge**（或其它基于 mihomo 的 Clash 客户端），里面有和路由器上同名的节点

## 一键安装

打开终端（Terminal、iTerm、Termius 都行），粘贴下面这一行，回车：

```sh
curl -fsSL https://github.com/rivia7/claude-accounts/releases/latest/download/install.sh | bash
```

安装过程中会问几个问题，照着回答就行：

1. **路由器地址**：装了 OpenClash（或 ShellCrash）的那台路由器的 IP，例如 `192.168.1.2`。用户名（一般是 `root`）和端口（一般是 `22`）直接回车。
2. **路由器密码**：只输入这一次，用来装一把免密登录的钥匙。密码本身不会被保存。
3. **出口节点**：屏幕上会列出路由器上的全部节点，输入编号选出允许账号使用的，例如 `1 3 4`。
4. **第一个账号**：填登录邮箱，再起个别名（比如"工作"，不想起可以直接回车），然后选一个出口节点。
5. **重载 OpenClash / ShellCrash**：第一次需要让它重新加载一次配置，大约 20 秒，这期间局域网里的设备新开的网络连接会断一下。以后加账号、换节点都不用再重载。

装好后打开一个新的终端标签，输入 `cca` 就能用了。

已经装过的，再运行一次这条命令就是修复或重装，账号和配置都会保留。

## 日常使用

输入 `cca`，会出现账号菜单：

```text
选择 Claude 账号    ↑↓ 移动 · 回车确认 · 数字键直接选 · q 退出

  ❯ 工作   work@example.com   已登录     US-A   5h 13% · 周 41%   · 上次使用
    个人   me@example.com     未登录     US-B
    kimi   api.example.com    API        直连
    ＋ 添加新账号
    ＋ 添加第三方 API 账号
    退出
```

用方向键选一个账号，回车，就会用这个账号启动 Claude Code。光标默认停在上次用的账号上，平时直接回车就行。官方账号后面是最近一次查到的用量（5 小时 / 本周用了多少），数据旧了会注明是多久前的。

第一次用某个账号时，会自动打开**这个账号专属的 Chrome 窗口**让你登录，邮箱已经帮你填好。登录一次之后，Claude Code 和这个窗口里的网页版就都是登录状态了。

| 想做什么 | 命令 |
| --- | --- |
| 选账号，启动 Claude Code | `cca` |
| 直接用某个账号启动 | `cca @工作`（写别名、邮箱或开头几个字都行） |
| 打开某个账号的网页版 | `cca web 工作` |
| 打开某个账号的 Claude App（桌面版） | `cca app 工作`，或者在启动台、聚焦搜索里点「Claude 工作」 |
| 添加账号 | `cca add`，或者在菜单里选"＋ 添加新账号" |
| 添加第三方 API 账号 | `cca add api`，或者在菜单里选"＋ 添加第三方 API 账号" |
| 查看各账号的用量 | `cca usage`（只看一个：`cca usage 工作`；给脚本用：`cca usage --json`） |
| 换第三方 API 账号的 key | `cca key kimi` |
| 查看所有账号 | `cca ls` |
| 改别名、换节点 | `cca edit` |
| 删除账号 | `cca rm 工作` |
| 检查每个账号的出口 IP | `cca check` |
| 出门在外：配置本机的 Clash 客户端（第一次在外面启动时也会问） | `cca local setup` |
| 看现在走路由器还是本机 Clash | `cca local` |
| 退出某个账号的登录、重新登录 | `cca login 工作` |
| Codex：选账号启动 / 直接启动 / 添加账号 / 查用量 | `cca codex` / `cca codex @工作` / `cca codex add` / `cca codex usage`（`cca codex` 可以简写成 `cxa`） |
| 查看全部命令 | `cca help` |

## 账号表

所有账号都记在 `~/.cca/accounts.json` 里，一个账号一行：

```json
{
  "工作(work@example.com)": "US-A",
  "me@example.com": "US-B"
}
```

左边是"别名(邮箱)"，没起别名的就只写邮箱；右边是这个账号走的出口节点。用 `cca edit` 修改，保存后自动生效。只改别名不影响登录。

第三方 API 账号写成一个对象（见下面的「第三方 API 账号」一节）：

```json
{
  "工作(work@example.com)": "US-A",
  "kimi": {"base_url": "https://api.example.com/anthropic", "model": "kimi-k2", "node": "", "id": "kimi"}
}
```

## 维护者的账号 → 节点映射（可选）

如果有人替你维护"哪个账号必须走哪个节点"，映射放在他的**私有** GitHub 仓库里，cca 自己去拉，映射里的账号出口就固定下来，映射里的账号也会自动加到你的账号表里（你自己的账号不受影响）：

- **启动时引导**：还没配置映射时，在终端里启动账号，cca 会先引导你配置；可以这次先跳过，也可以选「不用映射」，以后不再问。随时也可以运行 `cca policy setup`（不写仓库就是维护者的默认仓库 `rivia7/cca-policy`，别的仓库写成 `cca policy setup 维护者/仓库`）。一键安装时带上 `CCA_POLICY_REPO=维护者/仓库` 也会配置。
- **把公钥交给维护者**：cca 会给这台机器生成一把只读的 deploy key，显示它的公钥并复制到剪贴板。把它发给维护者（顺便说一声是谁的哪台机器，维护者加的时候要起个别名），维护者加到仓库上之后，在引导里按回车就能马上拉到；也可以先跳过，cca 每天自动拉一次（`cca policy pull` 马上拉）。`cca policy` 查看状态。
- **自动加账号**：映射和你的账号表合并。映射里有、这台机器上还没有的账号，拉到映射后自动加进账号表（别名用维护者起的，和你已有的别名重名时只写邮箱；已经加进来、还没有别名的，维护者之后在映射里起了别名也会补上），第一次启动它时登录；你自己加的、映射里没有的账号不受影响，你改过的别名也不会被改回去。
  - 不想要映射里的某个账号：`cca rm 别名` 删掉（在 `cca edit` 里删也一样），以后不会再自动加回来；想要了用 `cca add` 加回来。
  - 映射的节点不在这台机器的可选节点里时先不加：运行 `cca router setup` 选上那个节点，下次运行 cca 时就加进来。
  - Codex 账号要到第一次用 `cca codex` 时才加进来，不用 Codex 的机器不会白占路由器上的端口。
  - 维护者从映射里删掉了某个账号：你这台机器上的账号留着（登录还在），只是不再固定出口。
  - `cca policy` 里列出映射里还没加到这台机器上的账号和原因。
- **固定出口**：映射里的账号，添加时直接用映射的节点，不让选；账号表里改成别的节点，下次运行 cca 时会改回来，并提示一句。映射的节点不在这台机器的可选节点里时，运行 `cca router setup` 重新选可选节点。
- **节点改名**：路由器上的节点改了名（服务器地址没变）时，映射里还写着旧名字也照样能用：cca 跟着改名时记下了新旧名字的对应。维护者之后把映射更新成新名字就好。
- **验签**：映射要有发布 cca 用的那把密钥的签名才会被采用，签名不对、格式不对都不用，接着用上次拉到的。只管官方账号（按邮箱），第三方 API 账号不受影响。
- 这些检查都在你自己的 Mac 上做，用来防止配错，不防有意改动。`cca restore` 会删掉本机的 deploy key，记得让维护者把它从仓库上撤掉（`./policy.sh key rm 别名`）。
- 每台机器拉到的是整份映射（`~/.cca/policy/policy.json`，只有自己能读），里面有映射里所有人的邮箱和节点，映射里的账号也会加到每一台拉到它的机器上；只想给某些人用的账号、不想让大家互相看到的，维护者要分开建仓库。

维护者这边：

1. 建一个私有仓库（例如 `cca-policy`），放一个 `policy.json`：`{"version": 1, "claude": {"name@example.com": "US-A"}, "codex": {"name@example.com": "US-B"}, "aliases": {"name@example.com": "工作"}}`。`aliases` 可以不写：它是各台机器自动加账号时用的别名（Claude、Codex 通用），没写的账号只写邮箱。旧版本的 cca 会忽略这一项，照样拉得到映射。
2. 改好后在 cca 的仓库里运行 `./policy.sh 映射仓库的目录`：它会检查格式、确认仓库是私有的，然后用发布密钥签名，提交并推送。
3. 有人发来公钥时，复制它，在 cca 的仓库里运行 `./policy.sh key add`：读剪贴板里的公钥，提示给它起个别名（谁的哪台机器；直接回车用公钥备注里的机器名，不能和已有的重复），加成只读的 deploy key。公钥在文件里时写成 `./policy.sh key add 文件 [别名]`。你自己的机器上，启动时的引导已经把公钥复制到剪贴板了，直接运行就行。之后按别名查、删、改：
   - `./policy.sh key`：列出各台机器的 key（添加时间、最近一次拉取、别名，标出你这台机器的）。
   - `./policy.sh key rm 别名`：删掉。那台机器以后拉不到新映射，已经拉到的照用。
   - `./policy.sh key mv 别名 新别名`：改别名。
   - 别名就是 GitHub 上 deploy key 的标题，仓库的 Settings → Deploy keys 里看到的也是它。GitHub 的 deploy key 不能修改，改别名是删掉后用同一把公钥重新加上，那台机器什么都不用做。
   - 经 gh 加的 key 跟着 gh 的授权走：在 GitHub 的 Settings → Applications 里撤销 GitHub CLI 的授权，这些 key 会一起被删掉；`gh auth logout` 只删本机的登录，不影响它们。
   - 默认管第 4 步说的默认仓库，别的仓库加 `-R 你/仓库`。
4. 默认仓库写在 `cca` 里（`POLICY_REPO_DEFAULT`）：用户运行 `cca policy setup` 不写仓库、启动时的引导，都用它。

### 参考节点：自动补全 OpenAi 分流组

维护者还可以顺带发布自己 Clash 配置里的一个策略组（默认 `OpenAi`）和它引用的节点。各台机器拉映射时一起拉下来（同样要有发布密钥的签名，存在 `~/.cca/policy/nodes.json`，只有自己能读），你的配置里缺的直接写进你正在用的配置文件：

- **写到哪**：路由器上是 OpenClash 当前选用的配置文件（例如 `/etc/openclash/config/custom.yaml`），ShellCrash 是它的订阅配置 `yamls/config.yaml`；出门在外用的 Clash Verge 是它当前选用的配置（订阅或本地配置）。写进去的就是配置的一部分，打开配置文件就看得到。
- **怎么写**：缺的节点一个一行插在 `proxies` 开头，名字接在你的同名策略组后面，行尾都带 `# cca`；别的内容一个字节不动。改之前的配置留一份：路由器上在 `/etc/cca/source.bak.yaml`，Clash Verge 的在 `~/.cca/backup/`。
- **补什么**：这个组里的节点，你的配置里没有的。你已经有的不动：同名的节点照你自己的定义（只补进组）；同一个节点（类型、服务器、端口和 TLS 域名一样）在你那里叫别的名字，补进组的是你的名字；名字被你的策略组占了的不补；和 ShellCrash 自定义节点（`yamls/proxies.yaml`）同名的不补。你的配置里没有这个组，或者组写成了一行（`{name: OpenAi, …, proxies: […]}`），就只补节点。补上的节点也能选作账号的出口（`cca router setup`）。
- **先校验**：每一版参考节点都先用你路由器上的内核做一次 `-t` 校验，通过了才写（别处的节点你的内核不一定认，认不出的话整个 OpenClash / ShellCrash 都起不来）。不通过就不写，端口池照常，用这些节点的账号暂时断开；内核升级以后会再试。Clash Verge 那边是热加载时它的内核收下了才写。`cca router status` 里能看到还缺哪些、为什么没写。
- **什么时候生效**：路由器上的配置文件写好后，在跑的 OpenClash / ShellCrash 要重启一次才用得上（约 20 秒，期间局域网的连接都会中断），cca 会先问你；不同意的话，就等它下次自己重启。Clash Verge 不用重启：cca 把这几个节点热加载进它当前的运行配置。
- **维护者改了节点**：行尾带 `# cca` 的那一行跟着改（同样先校验，路由器上同样要重启一次才用上）。你自己的同名节点、去掉了 `# cca` 的行都不动。维护者不再发布、或者删掉了某个参考节点，写进去的留着（卸载 cca 之后也一样），不要了就自己删。
- **订阅更新**：订阅更新会把配置文件整个换掉，写进去的节点也跟着没了。OpenClash / ShellCrash 更新完重启时，cca 再写回去，这次启动就能用上。注意 OpenClash 更新订阅时会拿新下载的和现有的配置文件比对，现有的多了这几行，所以订阅内容没变，也会被当成有更新、重启一次。
- 参考节点里是节点的完整定义（服务器、UUID、密钥），每台拉得到映射的机器都拿得到。

维护者这边：把你自己的 Clash 配置复制成映射仓库目录里的 `custom.yaml`，照常运行 `./policy.sh 映射仓库的目录`。它从里面抽出策略组 `OpenAi`（别的组名用 `CCA_FILL_GROUP=组名 ./policy.sh …`）引用的节点（组里的策略组、内置出站不算；规则、别的节点都不带），写成 `nodes.json`，签名后一起推上去。`custom.yaml` 本身不提交（里面有你全部的规则和局域网设备；已经提交过的会拒绝发布），也会加进这个仓库的 `.git/info/exclude`。想先看看会发布哪些：`./policy.sh nodes custom.yaml`。不再发布：删掉目录里的 `custom.yaml` 和 `nodes.json`，再运行一次。

## 查看用量

```sh
cca usage
```

```text
  账号                     套餐  5 小时                    本周                   备注
  工作  bob@example.com    Max    13%  2 小时 13 分后重置   41%  周四 09:00 重置
  个人  alice@example.com  Pro    98%  25 分钟后重置        72%  周六 02:30 重置  Sonnet 本周 60%
```

- 每个官方账号经它**自己的出口**查询，和在 Claude Code 里打开 `/usage` 看到的一样：5 小时额度、本周额度，以及按模型单独计的每周额度。几个账号一起查，一般几秒钟。
- 「账号」一列和菜单一样写别名和邮箱，没起别名的只写邮箱。
- 套餐的叫法和官网一样：Pro、Max（原来的 Max 5x）、Ultra（原来的 Max 20x）、Team、Team Premium（Team 的 Premium 席位）、Enterprise。新版 Claude Code 在账号里记了官方给的套餐名时，直接照它写。
- 查询由 Claude Code 自己完成（cca 让它以账号的配置无界面地查一下，不加载插件、hooks、MCP），**不发消息、不耗额度**，cca 也不碰登录凭证。
- 查之前同样会确认出口：端口不通、出口跑到了受限地区的账号不去查。查不到时显示上次查到的数据，并注明是多久前的。
- 菜单和 `cca ls` 里每个账号后面也会附上最近一次的用量（只读缓存，不联网，不拖慢菜单）。
- `cca usage --json` 输出 JSON，方便接到脚本或状态栏里：`accounts` 里每个账号有 `plan`（套餐）、`source`（`live` 刚查到 / `cache` 查不到时用的缓存 / `none` 没有数据）、`error`（查不到的原因）、`fetched_at`，以及 `five_hour`、`seven_day`、`seven_day_opus`、`seven_day_sonnet` 等窗口（`{"utilization": 百分比, "resets_at": 重置时间, "reset": 重置时间是否已过}`）、`model_scoped`、`extra_usage`。所有账号都没有数据时退出码是 1。
- 这个查询用的是 Claude Code 标为实验性的接口；以后的 Claude Code 版本如果改了，cca 会提示这个版本的 Claude Code 查不了用量，不会影响启动账号。

## 第三方 API 账号

兼容 Anthropic 接口的服务（各家大模型的 Claude Code 接入地址、API 中转站等）可以加成一个账号，和官方账号一样在菜单里选、用 `cca @kimi` 启动：

```sh
cca add api
```

按提示填：别名、接口地址（就是服务商文档里的 `ANTHROPIC_BASE_URL`）、API key、模型（可选），再选出口：

- **直连**（默认）：不经代理，适合国内的服务。直连的账号不占路由器的端口，不配置路由器也能用。
- **走某个节点**：和官方账号一样经路由器（在外面经本机 Clash）的端口出去，适合需要代理才能访问的服务。

几点说明：

- **key 存在钥匙串里**，不写进账号表，`cca edit` 时也看不到；启动时经 Claude Code 的 `apiKeyHelper` 交给它，不出现在命令行参数和环境变量里（Claude 运行的命令用 `env`、`ps` 之类看不到它；不过同一个用户下的程序仍然能从钥匙串读到，这一点和其它存在钥匙串里的密码一样）。换 key 用 `cca key kimi`。想手动管理的，也可以在账号表里写 `"key": "…"`，写了就优先用表里的。
- **key 跟着服务商走**：钥匙串里的 key 是按接口地址的域名存的。把账号的 `base_url` 改成别的服务商时，旧 key 不会被发过去，启动时会要你重新输入。在 `cca edit` 里删掉账号、或者直接改账号表删掉账号，钥匙串里它的 key 也会一起删掉。
- **模型**：填了的话，会同时用作 opus / sonnet / haiku 三档（`/model` 切换和后台的小任务都用它）。不填就用 Claude Code 的设置，服务商不认那些模型名时请填上。
- **更多环境变量**：在账号表里加 `"env": {"API_TIMEOUT_MS": "600000"}` 这样的写法。接口地址、key、代理和云厂商开关由 cca 管，不能写在 env 里。env 的内容会出现在 claude 的命令行参数里（`ps` 能看到），所以别放密钥：名字像 `…_KEY`、`…_TOKEN`、`…_SECRET`、`…PASSWORD` 的变量 cca 不收。
- 第三方 API 账号默认关掉发往 Anthropic 的遥测和更新检查（`CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`）。
- 启动前不做 claude.ai 的出口检测（和第三方服务无关），只确认接口地址连得上。
- 第三方 API 账号没有网页版和 Claude App，也不查订阅用量。
- 改别名不影响它的 key 和配置（cca 按 `id` 认账号，改别名时别动 `id`）。
- 记忆、对话记录、设置、技能等照样和官方账号共用：在一个账号里聊到一半，可以换到另一个账号 `claude --resume` 接着聊。

## Codex 账号

OpenAI 的 Codex（`codex` 命令和 ChatGPT 桌面 App）也能多账号，用法和 Claude 账号一样，只是前面加个 `codex`：

```sh
cca codex add          # 添加 Codex 账号：填登录邮箱、别名（可选），选出口节点
cca codex              # 选账号启动 codex（cca codex @工作 直接启动，后面的参数原样交给 codex）
cca codex usage        # 查各账号的 5 小时、每周额度和剩余的重置次数
cxa @工作              # 简写：cxa 就是 cca codex（cxa、cxa usage、cxa add 也都行）
```

- **账号表分开记**：Codex 账号记在 `~/.cca/codex.json`，格式和 `accounts.json` 一样。同一个人的 Claude 号和 ChatGPT 号可以用同一个别名、同一个邮箱，`cca @工作` 启动 Claude，`cca codex @工作` 启动 Codex。两种账号共用路由器（或本机 Clash）上的同一组端口，每个账号还是一个端口、一个出口。选节点时只看同一种账号：Codex 账号之间最好各用各的节点，和 Claude 账号用同一个节点没关系（两家服务商互不相干）。
- **各自登录**：每个账号有自己的 `CODEX_HOME`（`~/.cca/codex/accounts/…/codex`），登录凭证（`auth.json`）只在这里。第一次启动时，cca 让 codex 生成登录网址，在**这个账号专属的 Chrome 窗口**里打开，整个登录都走这个账号的出口；登录网址里会带上账号的邮箱（`login_hint`，OpenAI 的登录页认的话会预填），登录后 cca 还会核对邮箱，登成别的号就不启动。要换登录用 `cca codex login 工作`。你原来 `~/.codex` 里的登录不受影响，直接敲 `codex` 用的还是它。
- **工作资料共用**：`~/.codex` 里的 `config.toml`、`*.config.toml` 配置 profile、`AGENTS.md`、会话记录、历史、技能、规则、提示词、插件、记忆等，所有账号共用一份；会话和记忆的数据库也共用（`CODEX_SQLITE_HOME` 指向 `~/.codex`），在一个账号里开的会话，换个账号 `codex resume` 照样能接着聊。
- **不共用的**：登录凭证、`installation_id`（codex 每个请求都带着它，共用会把几个账号关联起来）、按账号缓存的模型列表和应用列表、日志、`codex` 自己装在 `packages/` 里的程序，以及 git 工作树（`worktrees/`，macOS 的沙箱不让它的路径里有链接）。ChatGPT 桌面 App 放在 `~/.codex` 里的运行状态（自动化任务、内置浏览器等）也各账号一份。
- **出口不会悄悄换**：cca 启动 codex 时关掉它的两个开关：`system_proxy_fallback`（连不上代理时改走 macOS 的系统代理）和 `respect_system_proxy`（系统代理优先）；也不让它挂到常驻的后台进程上（`--no-daemon`，那个进程的代理在它启动时就定死了）。代价是在 cca 启动的 Codex 里用不了靠后台进程的功能（例如 `codex agents`）。
- **查用量**：`cca codex usage [账号] [--json]` 经每个账号自己的出口，让 codex 自己去查（和它的 `/status` 一样，不发消息、不耗额度），显示套餐（叫法和 codex 里一样：Plus、Pro 100、Pro 200、Pro 500…）、5 小时、本周额度、单独计的额度、剩余的重置次数（在 codex 里用 `/usage` → Redeem reset 兑换）和 credits 余额；`--json` 里另有 codex 原样给的 `credits` 和 `reset_credits`（`availableCount` 是剩余次数）。菜单里也附上最近一次的用量。`cca usage` 只查 Claude 账号。
- **第三方 API**：`cca codex add api` 添加兼容 OpenAI **Responses 接口**（`/v1/responses`）的服务，填地址（例如 `https://api.example.com/v1`）、key、模型。key 存在钥匙串里（条目名 `cca-codex-api-…`），启动时由 codex 的 `auth.command` 回调 cca 现取，不出现在命令行参数和环境变量里；地址和模型经 `-c` 交给 codex。只提供 Chat Completions 接口的服务用不了。
- **ChatGPT App（Codex 桌面版）**：`cca codex app 工作`，或者在启动台、聚焦搜索里点「ChatGPT 工作」。每个账号一份资料目录，用这个账号的 `CODEX_HOME`（和 `cca codex @工作` 同一份登录），整个 App 走这个账号的出口。App 自己启动 codex 时 cca 加不了参数，所以第一次打开时会征求你同意，用 Codex 自带的 `codex features disable system_proxy_fallback` 在共享的 `~/.codex/config.toml` 里关掉回退到系统代理（之后直接敲 `codex` 也不再回退）。没登录时先在账号专属的 Chrome 里登录，App 打开时就是登录状态。如果 App 里还要你登录（比如登录过期了），别在 App 里登：它的登录页会在系统默认浏览器里打开，那一次不经账号的出口；关掉 App，运行 `cca codex login 工作`，再打开 App。
- **其它命令**：`cca codex ls`、`edit`、`check`、`web`（用账号专属的浏览器打开 chatgpt.com）、`key`（换第三方 API 账号的 key），和 Claude 账号的同名命令一样。
- **简写 `cxa`**：`cxa` 就是 `cca codex`，后面接什么都一样（`cxa`、`cxa @工作`、`cxa usage`、`cxa add`……）。`cca install` 时和 `cca` 一起放进 `~/.local/bin`；用着 Codex 的机器上 `cxa` 不见了（比如以前装的 cca 升级上来），下一次运行 cca 时自动补上。已经有别的 `cxa` 命令时不会覆盖它。
- **删除**：`cca codex rm 工作` 退出登录、删掉账号目录（包括它的 git 工作树，删之前会提醒）；它开过的会话留在 `~/.codex` 里，cca 平时就把会话库里的路径写成 `~/.codex` 下的，删了账号也找得到。

启动前同样会检查：出口地区和纯净度（看 chatgpt.com 看到的 IP）；shell 里继承的 `OPENAI_*`、`CODEX_*`（例如会直接顶替登录的 `CODEX_ACCESS_TOKEN`）一律不带进去，只留公司网络根证书 `CODEX_CA_CERTIFICATE`；共享的 `~/.codex/config.toml` 或任一个 `*.config.toml` profile 里如果写了 `chatgpt_base_url`、`openai_base_url`、`experimental_realtime_ws_base_url`、`experimental_realtime_webrtc_call_base_url`、不是 `openai` 的 `model_provider`、不是 `file` 的 `cli_auth_credentials_store`、`forced_chatgpt_workspace_id`、`forced_login_method = "api"`、打开了 `respect_system_proxy`（也兼容旧版内嵌的 `profile` 配置），或者 `~/.codex/.env` 里写了代理、`OPENAI_*`，cca 会拒绝启动并指出来。账号内的配置合并回共享目录后还会再检查一次。

## Claude App（桌面版）

Claude 的 Mac 桌面 App 也能每个账号开一份：

```sh
cca app 工作
```

也可以在启动台、聚焦搜索里点「Claude 工作」。cca 会给每个账号生成一个这样的启动器（放在 `~/Applications`），名字是"Claude"加别名，没起别名的用邮箱 @ 前面的部分（比如「Claude me」）；图标右下角有账号标识（别名的头两个字或首字母，每个账号一种颜色），在启动台里一眼就能分清。账号改名、删除时，启动器自动跟着改。

- 每个账号的 App 有自己独立的资料（登录状态、设置），整个 App 都走这个账号的出口，打开前同样会检查出口。
- App 里的 Code 标签页和 `cca @工作` 用同一个账号配置，记忆、技能、设置照样共享。
- 这个账号的 App 已经开着时，再打开只会把它切到前面。
- 你原来的 Claude App 不受影响，照常使用。

需要知道的几点：

- 正在运行的 Claude 在 Dock 里的图标改不了，开了几个账号就有几个一模一样的 Claude 图标；分不清时用 `cca app 账号` 或启动器切换。
- 第一次打开要登录。用邮箱登录全程走这个账号的出口；用 Google 登录时，那一次登录会走系统网络。不要同时在两个 App 里登录。
- 在别的 App 里点开的 claude.ai 链接，macOS 只会交给其中一个 Claude App。
- 这个功能依赖 Claude App 支持独立的资料目录（4.29 版已验证）。以后的版本如果不支持了，cca 会停下并提示，不会用错账号。

## 出门在外

离开家、连不上路由器时，cca 自动改走 Mac 上的 Clash 客户端，每个账号照样走它自己的节点。

第一次在外面启动时，cca 会说明情况，问你要不要配置。也可以出门前在家里先配置好：

```sh
cca local setup
```

- **Clash Verge**：全自动。cca 在它的「全局扩展脚本」末尾加一段代码，在本机开一组端口（127.0.0.1:7901 起，每个账号一个），然后让它重新加载一次配置，不用重启 Clash Verge。你原来的订阅、规则和系统代理都不变。
- **其它基于 mihomo（Clash Meta 内核）的客户端**：cca 给出一段配置让你加进客户端，再填上它的外部控制地址。

之后就不用管了：

- 每次启动先试一下能不能连上路由器（最多等 1 秒）：连得上走路由器，连不上走本机的 Clash 客户端。出口检查照常做。
- 在外面换节点、加账号都即时生效；路由器那边等你回家后下一次启动时自动补上。
- Clash Verge 没开着时，cca 会问你要不要帮你打开。
- `cca local` 查看现在走哪条路、每个端口选中了哪个节点；`cca local off` 删掉 cca 加的那段代码。

需要知道的几点：

- Clash Verge 里的节点可以和路由器上叫不同的名字：cca 按服务器地址（类型、服务器、端口和 TLS 域名）认出是同一个节点，切换时用 Clash Verge 里的名字，账号表不动。名字和地址都对不上的，用这个节点的账号在外面会停下并提示，不会换成别的出口。
- 在家里打开的 Claude Code、网页版和 Claude App 走的是路由器的端口，出门后要关掉，用 cca 重新打开。在外面打开的，只要 Clash Verge 开着，回家后也能接着用。
- 判断"在不在家"看的是能不能连上路由器，所以在外面如果经 ZeroTier 之类能连到路由器，会照常走路由器。想固定走一种，把 `~/.cca/config.json` 里的 `route` 改成 `router` 或 `local`（默认 `auto`）。

## 每次启动前会检查什么

cca 会先通过这个账号的出口检查一遍，有问题就不启动：

- **出口所在地区**：如果出口在 Claude 不提供服务的地区（例如中国大陆、香港、俄罗斯），直接停止。
- **IP 纯净度**（数据来自 ip.net.coffee）：分数太低会先问你还要不要继续。
- **DNS / WebRTC 泄露**（照 ip.net.coffee 的 [DNS](https://ip.net.coffee/dns/)、[WebRTC](https://ip.net.coffee/webrtc/) 检测页的做法）：和出口检测同时在后台测，结果写在启动时的「泄露」那一行；只提示，不拦着启动。
  - **DNS**：经这个账号的端口访问一次性的子域名，看是哪些 DNS 服务器来解析的。出口不在中国大陆、解析器却在中国大陆，就是泄露（网站的 DNS 能看出查询来自中国大陆），一般是节点在本地把域名解析好再发出去（比如直连、WireGuard 节点）。要几秒，所以结果按节点记 6 小时，平时启动不用等它。
  - **WebRTC**：不经代理直接问 STUN 服务器（UDP）这台 Mac 的公网 IP，就是浏览器的 WebRTC 直连时网页拿得到的 IP；和账号的出口不一样就是泄露：账号的网页版、Claude App 里的网页（还有实时语音这类功能）都拿得到它，几个账号会露出同一个 IP。它和出口检测同时完成，不额外花时间。打开 App、要在浏览器里登录时会另外说明；可以在路由器上拦掉 STUN（UDP 3478、19302 端口）。
  - `cca check` 每次都重新测，并写出解析器在哪；不想测的话在 `~/.cca/config.json` 里加上 `"leak_check": false`。`cca web` 照旧不做启动前检测（打开得快）。
- **登录的是不是这个账号**：如果这个账号的窗口里登录成了别的邮箱，会拒绝启动并提示你重新登录，免得一个账号用了另一个账号的出口。
- **共享设置有没有"串号"的配置**：`~/.claude/settings.json`（或 `settings.local.json`）的 env 里如果写了代理（包括 `NO_PROXY`）、`ANTHROPIC_BASE_URL`、`ANTHROPIC_AUTH_TOKEN`、`ANTHROPIC_API_KEY`、`CLAUDE_CODE_OAUTH_TOKEN`、云厂商开关（`CLAUDE_CODE_USE_BEDROCK` / `VERTEX` / `FOUNDRY` / `MANTLE` 等），或者写了 `apiKeyHelper`，会盖掉每个账号自己的设置（官方账号的登录 token 甚至会被发到别的地址），cca 会拒绝启动并指出来。
- **shell 环境**：shell 里 export 的 `ANTHROPIC_*`（包括 `ANTHROPIC_MODEL`）、`CLAUDE_CODE_OAUTH_TOKEN` 和云厂商开关，cca 启动时一律不带进去，并提示屏蔽了哪些。官方账号要换模型，写进 `~/.claude/settings.json` 或在会话里用 `/model`。
- **第三方 API 账号**：只确认接口地址经它的出口连得上。

## 自动更新

不用管它。cca 每天会在后台检查一次新版本，下载好之后，在下一次启动时自动换上。

- 只会安装带有效签名的正式版本，被篡改的文件不会装，测试版也不会自动装。
- 新版本有问题：`cca update --rollback` 退回上一版。
- 立即更新：`cca update`；只提示、不自动装：`cca update notify`；完全关闭：`cca update off`。

### 测试版

新功能有时先发测试版（版本号像 `2.8.0-beta.1`，在 [GitHub Releases](https://github.com/rivia7/claude-accounts/releases) 里标为 Pre-release）。自动更新和一键安装都不会装测试版，想试的话手动装（下面的 `X.Y.Z-beta.N` 换成那个测试版的版本号）：

- 已经装着 2.6.0 或更新的版本（2.6.0 的测试版也行）：`cca update X.Y.Z-beta.N`
- 2.5.0 及更早的版本不认这个写法，还没装 cca 的也一样：用一键安装，在后面写上版本号（账号和配置都保留）：

  ```sh
  curl -fsSL https://github.com/rivia7/claude-accounts/releases/download/vX.Y.Z-beta.N/install.sh | bash -s X.Y.Z-beta.N
  ```

- 退回原来的版本：`cca update --rollback`（退回后这个测试版不会再被装上）。
- 装了测试版之后，不会自动更新到别的测试版；出了更新的正式版（比如装着 2.8.0-beta.2 时出了 2.8.0）时照常自动更新。

## 卸载

```sh
cca restore
```

撤销 cca 做过的全部改动：每个账号（Claude 和 Codex）退出登录，并删除它的登录凭证（第三方 API 账号删除钥匙串里的 key）；路由器上的端口和配置全部移除；Clash Verge 全局扩展脚本里 cca 加的那段删掉（维护者的参考节点写进了你的配置文件的，留着：行尾带 `# cca`，不要的话自己删）；最后删除本机的 cca（包括 `cxa`）和各账号的 App 启动器（删除前会把配置打包备份到你的主目录，不含登录凭证）。Codex 账号建的 git 工作树（`worktrees/`）在账号目录里，会一起删掉，也不在备份里，还原前会提醒。你原来的 `~/.claude`、`~/.codex`（对话记录、设置等）不受影响。卸载前先退出各账号的桌面 App。

## 常见问题

**会保存我的路由器密码吗？**
不会。密码只用一次，用来装免密登录的钥匙，之后 cca 都用这把钥匙登录路由器。

**cca 会碰我的 Claude 登录凭证吗？**
不会。登录完全走官方的 `claude auth login`，凭证由 Claude Code 自己存进 macOS 钥匙串；cca 不读取、不保存，也不转发任何登录凭证。`cca usage` 查用量也是让 Claude Code 自己去查。

**Codex 的登录凭证呢？**
登录、刷新都由 codex 自己完成，凭证由它写进账号目录里的 `auth.json`。cca 只从里面的 id_token 读出邮箱、套餐和工作空间 ID（核对登录的是不是这个账号），不碰真正用来访问的 access / refresh token，也不保存、不转发。

**不同账号的设备指纹独立吗？**
客户端设备标识独立：Claude Code 使用各账号自己的 `userID`、`machineID`，Codex 使用自己的 `installation_id`。cca 在登录前准备好这些随机标识，保存在账号目录的 `device.json`，再写入客户端配置；重新登录、改别名、换节点都沿用，客户端清掉标识时也会从这份记录恢复。旧账号已有合法且独立的标识会保留；与主配置或其它本机账号重复、通过链接共享的标识会自动分开。删除账号时一起删除，重新添加会生成新标识。

这里隔离的是客户端设备标识。Chrome 和桌面 App 的 Cookie、站点存储仍由各账号的独立资料目录隔离；Canvas、WebGL、字体、屏幕和操作系统等浏览器或硬件特征不会因此改变。

**历史、记忆和配置还会跨账号共享吗？**
会。Claude 的 `projects`（会话和项目记忆）、`history.jsonl`、设置、指令、技能、规则、插件、hooks 脚本等保持共享；Codex 的会话、历史、记忆、配置和相关数据库也保持共享，切换账号后仍可续接会话。登录凭证、设备字段和已识别的账号运行状态独立保存。共享清单以 `cca` 中的 `CLAUDE_SHARED` / `CODEX_SHARED` 为准，未知新增条目默认不共享；插件目录内部的数据仍随插件目录共享。

CLI 启动和正常退出时会合并配置并修复被客户端原子保存替换掉的共享链接；桌面 App 在下次经 cca 启动时处理，不承诺运行中的配置立即刷新。顶层 JSON 配置按字段三方合并；同一字段冲突时保留共享值，完整账号副本存进 `~/.cca/conflicts/`（Codex 是 `~/.cca/codex/conflicts/`）并显示路径。TOML、指令文件和 `.env` 在双方同时修改时保留共享文件和完整冲突副本，不自动拼接。无效 JSON 也会保留副本，不覆盖有效共享配置。

升级时，旧版指向共享主目录的未知条目链接会拆成账号自己的副本，源文件保留；用户自建的外部链接不动。旧共享目录若含内部符号链接，自动拆分会停止并指出路径，避免追随外部目录或循环链接。已有设备 ID 不因这项迁移重置。共享与续接会保留共同上下文，因此客户端标识独立并不意味着服务端无法关联账号。

**第三方 API 的 key 存在哪？**
默认存在 macOS 钥匙串里（条目名 `cca-api-…`），启动时经 Claude Code 的 `apiKeyHelper` 交给它，不出现在命令行参数和环境变量里。删除账号（`cca rm`）或一键还原（`cca restore`）时一起删掉。只有你自己在账号表里写了 `key` 字段时，key 才会以明文存在账号表里（文件只有你能读；`cca restore` 打的备份包也只有你能读）。

**cca 启动的 Claude Code 能直接看、改 `~/.cca` 吗？**
能。cca 启动 Claude Code 时把 `~/.cca` 加成工作目录（`--add-dir`），让 Claude 帮你看账号表、配置、映射和日志时不用一次次确认；改文件照你选的权限模式来。登录凭证和浏览器资料除外：映射的 deploy key 私钥、维护者参考节点（`policy/nodes.json`，里面有节点的密钥）、写参考节点前留的 Clash Verge 配置备份（`backup/clash-verge-*.yaml`）、Codex 账号的 `auth.json`、各账号的 Chrome 和桌面 App 资料目录，cca 用 `--disallowedTools` 挡住了 Claude 的读写（账号表里如果写了明文的 `key` 字段，Claude 是看得到的，key 最好存钥匙串）。不想放开的话，在 `~/.cca/config.json` 里写上 `"claude_add_dir": false`。只管 cca 启动的 Claude Code，Codex 和桌面 App 不受影响。

**cca 会改我的 Clash Verge 配置吗？**
端口池只在它的全局扩展脚本末尾加一段带标记的代码，不碰你的订阅、规则和其它设置；`cca local off` 或 `cca restore` 会把这段原样删掉，脚本恢复成原来的样子。维护者发布了参考节点时，你当前选用的配置里缺的那几个节点会直接写进去（行尾带 `# cca`，改之前的留一份在 `~/.cca/backup/`），见上面的「参考节点」。

**为什么第一次要重载 OpenClash / ShellCrash？**
第一次要在路由器上建好一组端口。之后只要账号不超过 8 个，加账号、换节点都不用重载。账号超过 8 个，或者改了可选节点，才需要再重载一次，到时候会先问你。

**提示"端口不通"怎么办？**
运行 `cca router status` 查看路由器上的端口状态，运行 `cca router sync` 重新同步。

**提示出口纯净度低怎么办？**
用 `cca edit` 给这个账号换一个节点。

**OpenClash / ShellCrash 升级了会不会失效？**
不会。cca 在路由器上的文件放在 `/etc/cca`，不属于它们。升级、重装或者在 ShellCrash 的任务菜单里清空了任务之后，cca 会自动把钩子补回去；如果端口需要重建，下次启动 cca 时会先问你同不同意重载。

**路由器上 OpenClash 和 ShellCrash 都装了怎么办？**
不用管。cca 看哪个在运行（都没在运行时看哪个开机自启），切换节点、重载都对它做；两边的钩子都装上，换着用也不会丢端口池。

**ShellCrash 里有什么限制？**
要 ShellCrash 1.9 或更新的版本（下一版挪了任务文件和运行目录的位置，也认得），1.8 及更早的不行；要用 Meta（mihomo）内核，sing-box 内核不行。能选作出口的是订阅配置里的节点和策略组；在 ShellCrash 里另外添加的自定义节点不行（它们启动时才拼进配置，配置出错时又会被去掉），选了的话这个账号的端口会断开，不会经别的节点出去。在 ShellCrash 的 `yamls/others.yaml` 里自己写了 `listeners` 的话，启动时没法注入端口池（会重复），要等 cca 问过后热加载。

**在路由器上给节点改了名怎么办？**
不用管。cca 记着每个可选节点的服务器地址（类型、服务器、端口和 TLS 域名，只存摘要，不存地址本身）。下次启动或同步时，cca 按地址认出改名后的节点，把可选节点和账号表一起改过来，并提示一句；端口池要跟着重建时，会先问你同不同意重载。名字和地址都变了的认不出来，在终端里会让你从路由器上的节点里选一个代替；两个可选节点地址一样、其中一个没了时，也不会自动合到另一个上，同样让你选。选之前，用这个节点的账号暂时用不了（宁可断开也不换出口）。

**运行配置文件被删了会怎样？**
切换节点照常（控制接口的端口和密钥改从 OpenClash / ShellCrash 的设置里读）。要建立或调整端口池时，没有运行配置就没法热加载，cca 会问你要不要重启一次 OpenClash / ShellCrash（约 20 秒），重启时由钩子注入端口池。`cca router status` 会注明运行配置不见了。

## 它是怎么做到的

- **分开登录、共享工作资料**：每个账号有自己的 Claude Code 配置目录（`CLAUDE_CONFIG_DIR`，按邮箱命名）。登录信息、设备标识和账号/进程状态各自保存；历史、记忆、配置等明确列入共享清单，链接到你原来的 `~/.claude`。`~/.claude.json` 里和账号无关的设置，每次启动和退出时按字段合并；新增未知字段留在账号目录。
- **分开设备标识**：各账号的 `device.json` 固定保存客户端标识，初始化和旧配置迁移使用文件锁；Claude 的标识不参加共享设置合并，Codex 的 `installation_id` 不在共享白名单里。旧的设备文件若是软链接或硬链接，会原子替换为该账号自己的文件，避免写到共享源。
- **分开出口**：cca 在路由器上开一组端口（从 7901 开始，8 个一组），每个端口绑定一个出口节点，每个账号用自己的端口上网。换节点通过 OpenClash / ShellCrash 内核的接口直接切换，不用重启。
- **网页版**：每个账号有一个专属的 Chrome 窗口（独立的浏览器资料，走同一个端口），所以网页版和 Claude Code 的出口一致。
- **泄露检测**：DNS 那项经账号的端口访问 `<随机串>-1.d.ip.net.coffee`，ip.net.coffee 的权威 DNS 记下来查的解析器，再经同一个端口问它要结果、查归属地（按网段记 7 天）；WebRTC 那项是 Python 直接发 STUN Binding 请求（IPv4、IPv6 都试），对方看到的 IP 不经账号的出口查归属地。结果在 `~/.cca/leak.json`。
- **用量**：cca 以账号的配置目录、经账号的端口无界面地启动 Claude Code（`--safe-mode`，不加载插件、hooks、MCP），发一个 `get_usage` 控制请求，拿到和 `/usage` 一样的数据；登录 token 的读取和刷新都由 Claude Code 自己完成。
- **第三方 API 账号**：接口地址、模型经 `claude --settings` 传入（优先级高于共享的 settings.json），key 由 `apiKeyHelper` 回调 cca 从钥匙串取，不进环境变量。
- **Codex**：每个账号一个 `CODEX_HOME`，`~/.codex` 里白名单上的条目链接进去（白名单以外的默认各账号一份，新版本 Codex 加了文件也不会误共享）；数据库经 `CODEX_SQLITE_HOME` 共用。登录和查用量都经 codex 自己的 app-server（`account/login/start`、`account/rateLimits/read`），报的客户端名和 `codex login` 一样。
- **路由器端**：改动放在 `/etc/cca`。OpenClash 通过它的"覆写脚本"注入；ShellCrash 通过它的"启动前任务"（`task/bfstart` 里的一行，下一版起是 `configs/task/bfstart`）把订阅配置换成插好端口池的副本，端口池不写进订阅配置。维护者的参考节点直接写进 OpenClash 选用的配置文件、ShellCrash 的订阅配置（每一版先用内核校验过，记在 `/etc/cca/fill.ok`）；同步、每次在家启动、OpenClash / ShellCrash 启动时都核对一遍（配置文件和参考节点都没变时只算一下校验和）。每 2 分钟自检一次，刷固件时保留。
- **出门在外**：本机的端口池和路由器上的一样（127.0.0.1:7901 起，每个端口一个策略组），写在 Clash Verge 的全局扩展脚本里，它每次重新生成配置都会带上；cca 经它内核的控制接口切换节点，第一次配置时把端口池直接热加载进正在运行的内核。

<details>
<summary>不想用一键安装？可以手动下载并校验签名</summary>

```sh
curl -fsSL -o /tmp/cca     https://github.com/rivia7/claude-accounts/releases/latest/download/cca
curl -fsSL -o /tmp/cca.sig https://github.com/rivia7/claude-accounts/releases/latest/download/cca.sig
echo 'cca-release ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICxClUySZa2aVx7s6HRzufgJ49ad44smTnmkzDI/Osex' > /tmp/cca.allowed
ssh-keygen -Y verify -f /tmp/cca.allowed -I cca-release -n cca-release -s /tmp/cca.sig < /tmp/cca
bash /tmp/cca install
```

发布公钥指纹：`SHA256:/JDnxJ7oMpg5H5K8rBbhsRo6sKoL65IaVLtQGeGrIZQ`。一键安装脚本也会做同样的校验。

</details>

## 开发

- 跑测试：`bash tests/regress.sh`。全部使用假的路由器、claude 和 GitHub，不联网，也不碰你真实的配置。
- 只测设备标识的隔离、迁移和并发：`python3 tests/device_identity.py`（完整回归也会跑）。
- 只测历史/记忆/配置共享、旧链接迁移、配置合并和冲突保留：`python3 tests/shared_state.py`（完整回归也会跑）。
- 发布新版本：改 `cca` 里的 `VERSION` 并提交，然后运行 `./release.sh "更新说明"`。它会跑测试、签名、打 tag、推送，并创建 GitHub Release。

## 许可证

[MIT](LICENSE)
