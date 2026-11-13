# cca：一台 Mac 同时用多个 Claude、Codex 账号

有好几个 Claude 账号（比如个人号和工作号）？cca 让它们在同一台 Mac 上各用各的（OpenAI 的 Codex 也一样，见「Codex 账号」一节）：

- **各自登录**：每个账号单独登录、互不影响，切换账号不用退出重登。
- **各自的设备标识**：登录前为每个账号准备独立、持久的客户端设备标识，重新登录、改别名或换节点时保持不变。
- **各走各的网络**：每个账号的流量从你指定的代理节点出去，出口 IP 固定，不会混用。在家走路由器；出门在外连不上路由器时，自动改走 Mac 上的 Clash 客户端（例如 Clash Verge）。
- **工作资料共用**：对话记录、记忆、设置、插件、技能、MCP 等，所有账号共用一份。在一个账号里配好的东西，换个账号照样能用。
- **一眼看清用量**：`cca usage` 列出每个账号的 5 小时、本周额度用了多少、什么时候重置，挑个还有余量的账号接着干。
- **第三方 API 也能当账号用**：兼容 Anthropic 接口的服务（填地址和 key），和官方账号一起出现在菜单里，可以直连，也可以走节点。
- **Codex 也能多账号**：`cca codex` 管 ChatGPT 账号的 Codex（命令行和 ChatGPT 桌面 App），同样各自登录、各走各的出口，工作资料共用 `~/.codex`。
- **Antigravity 也能多账号**：`cca agy` 管 Google 账号的 Antigravity 命令行（`agy`），同样各自登录、各走各的出口，工作资料共用 `~/.gemini`（见「Antigravity 账号」一节）。

终端里的 Claude Code（`claude` 命令）、网页版 claude.ai 和 Claude 的 Mac 桌面 App 都支持（第三方 API 账号只用于 Claude Code）。Codex 那边支持 `codex` 命令、网页版 chatgpt.com 和 ChatGPT 桌面 App（Codex 的第三方 API 账号只用于 `codex` 命令）。

## 需要准备什么

- 一台 Mac（Linux 服务器上也能用命令行，见「Linux 服务器」）
- Claude Code（`claude` 命令）：没装也行，安装 cca 时会问要不要用官方安装脚本装上（见下面的「辅助安装」）
- 用 Codex 的话：装好 `codex` 命令（npm、Homebrew 或 OpenAI 的安装脚本都行，cca 会在 PATH、`~/.local/bin`、`/opt/homebrew/bin`、`/usr/local/bin` 里找；没装时 `cca codex install-cli` 用官方安装脚本装）；要用桌面版的话装好 ChatGPT App
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

### 辅助安装：claude、codex、agy 命令

`cca install-cli`（Codex：`cca codex install-cli`，Antigravity：`cca agy install-cli`）用它们的官方安装脚本装命令，和官网上的 `curl -fsSL … | bash` 一样：脚本自己下载、校验，装进 `~/.local/bin`。安装 cca 时没找到 Claude Code、终端里用到没装的命令时，也会先问要不要装；cca App 里账号页“没装 X 命令”旁边有「安装 X 命令…」，在终端里运行同一条命令。

- **先直连**。拿不到安装脚本、或者直连没装成（例如安装脚本自己下载不了）时，问过你再经 cca 内置代理的一个节点装：在本机临时起一个只走这个节点的代理端口（只在 127.0.0.1 上，不碰正在用的内置代理、Clash Verge 和路由器），装完就停；它的配置有节点密钥，放在只有你能读的临时目录里，用完删掉。
- **用哪个节点**：维护者在映射里指定（`install_node`，见「维护者的账号 → 节点映射」）。没配置映射、映射没指定、或者这台机器没有这个节点的定义时，在终端里现选一个（只用这一次，不记下）。节点定义和内置代理的一样：在家时从路由器拷的可选节点，加上维护者参考节点。
- **不带账号的东西**：安装脚本在空环境里运行，不带账号、代理设置和 API key；Claude Code 的安装（`claude install`）用一次性的配置目录，不往账号和 `~/.claude` 的配置里写（官方安装脚本只把下载的文件临时放在 `~/.claude/downloads`，装完删掉）。

## Linux 服务器

Linux 上也能装，用上面「一键安装」的同一行命令（Ubuntu 20.04 起、Debian 12 起、Rocky / AlmaLinux 9 起、Fedora、Amazon Linux 2023、Alpine、Arch、openSUSE 都测过）。要 python3（3.8 或更新）、OpenSSH 8.1 或更新（校验发布签名用）、curl、nc、perl、gzip、find：缺什么安装时会说，并给出这台机器上装它的那一行命令（例如 `apt install netcat-openbsd`）。RHEL / Rocky 8 的 OpenSSH 是 8.0，装不了。Claude Code 没装也行：安装时会问要不要用官方安装脚本装上（服务器直连不通时经内置代理装，节点由维护者提供，见下面的「直连不通的服务器」）。

Linux 上能用的是命令行：

- **账号的网络**：连得上路由器时照常走它的端口池；连不上的（云服务器）用 cca 内置的代理，mihomo 内核由 cca 在后台运行（不用 root，不靠 systemd）。节点在家时从路由器拷一份，连不到路由器的服务器上只能用维护者参考节点里有的节点。配置时问过的话，在你自己的 crontab 里加一行 `@reboot`，机器重启后内核自己起来（`cca local off` 时删掉）。Linux 桌面上装了 Clash Verge 的，和 Mac 上一样改走它。
- **直连不通的服务器（不用路由器）**：安装时 cca 先测这台机器能不能直连 claude.ai。连不上、或者出口在受限地区（比如国内的云服务器），又从没配置过路由器时，不再问路由器，直接用内置代理：
  1. cca 给这台机器生成一把只读 deploy key，显示一行命令 `./policy.sh key add '公钥' 机器名`。把它发给维护者，在 cca 的仓库里原样运行（见「维护者的账号 → 节点映射」）。
  2. cca 每 5 秒自动拉一次维护者的映射，维护者加好后几秒内就接着往下走（回车马上再拉；输入 s 先跳过，之后再运行 `cca install` 接着来）。
  3. 映射验过签名后，导入维护者指定的安装节点和映射里这台机器的账号要用的节点（只导入维护者参考节点里有的），配置好内置代理，再经它装上 Claude Code（不再先直连试），映射里的账号自动加进来，接着 `cca login 账号` 登录就能用。
  - 还没有可选节点时 `cca add` 也走这一套；能直连的服务器照旧问要不要配置路由器。要用路由器的，之后运行 `cca router setup`。
- **登录**：服务器上没有账号专属的浏览器。`cca login 工作` 给出一行在 Mac 上运行的命令，登录页在 Mac 上用这个账号专属的 Chrome 打开，再把拿到的验证码贴回服务器（见「常见问题」）。Mac 上还没有这个账号的话，会问要不要用同一个节点加上它。
- **泄露检测**：只测 DNS。WebRTC 只和浏览器、桌面 App 有关，服务器上没有它们。
- **API key**：存成 `~/.cca/secrets` 下只有你能读的文件。
- 网页版、桌面 App、图形界面（`cca web`、`billing`、`limit-resets`、`app`、`gui`）只在 macOS 上。

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
| 看某个账号的每月 API 额度和余额 | `cca billing 工作`（ChatGPT 账号：`cca codex billing 工作`。在这个账号的浏览器里打开控制台的账单页，第一次要登录控制台，邮箱已经复制好、粘贴即可；cca App：账号页“用量”里「订阅附带的每月 API 额度」那一行的「在控制台查看」） |
| 看 Claude 账号的重置次数和到期日 | `cca limit-resets 工作`（在这个账号的浏览器里打开 claude.ai 的用量页，邮箱同样复制好；cca App：账号页“用量”里「重置次数和到期日」那一行的「在 claude.ai 查看」） |
| 打开某个账号的 Claude App（桌面版） | `cca app 工作`，或者在启动台、聚焦搜索里点「Claude 工作」 |
| 添加账号 | `cca add`，或者在菜单里选"＋ 添加新账号" |
| 添加第三方 API 账号 | `cca add api`，或者在菜单里选"＋ 添加第三方 API 账号" |
| 查看各账号的用量 | `cca usage`（只看一个：`cca usage 工作`；给脚本用：`cca usage --json`） |
| 换第三方 API 账号的 key | `cca key kimi` |
| 给官方账号记一把 API key 备查 | `cca key 工作`（存进钥匙串，cca 不拿它启动；`--show` 打出来，`--forget` 删掉；维护者映射里的账号会加密同步给登录了它的其他设备） |
| 查看所有账号 | `cca ls` |
| 改别名 | `cca rename 账号 新别名`（写 `-` 去掉别名；登录、资料、端口都不变；cca App 里点名字旁边的笔） |
| 换节点（也能改别名） | `cca edit` |
| 删除账号 | `cca rm 工作` |
| 检查每个账号的出口 IP | `cca check` |
| 打开图形界面（用量、节点分流、一键打开 App） | `cca gui` |
| 装 claude 命令（Codex、Antigravity：`cca codex install-cli`、`cca agy install-cli`） | `cca install-cli`（官方安装脚本，直连不通时问过再经内置代理装） |
| 出门在外：配置本机的 Clash 客户端（第一次在外面启动时也会问） | `cca local setup` |
| 看现在走路由器还是本机 Clash | `cca local` |
| 退出某个账号的登录、重新登录 | `cca login 工作` |
| Codex：选账号启动 / 直接启动 / 添加账号 / 查用量 | `cca codex` / `cca codex @工作` / `cca codex add` / `cca codex usage`（`cca codex` 可以简写成 `cxa`） |
| Antigravity：选账号启动 / 直接启动 / 添加账号 | `cca agy` / `cca agy @工作` / `cca agy add`（`cca agy` 可以简写成 `aga`） |
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
- **交给维护者的一行命令**：cca 会给这台机器生成一把只读的 deploy key，显示维护者要运行的一行命令 `./policy.sh key add '公钥' 别名`（公钥和别名都写在里面，别名默认是这台机器的名字；Mac 上顺便复制到剪贴板）。把这一行发给维护者（顺便说一声是谁的哪台机器），维护者在 cca 的仓库里原样运行就加好了。引导里 cca 每 5 秒自动拉一次，加好后几秒内就拉到（回车马上再拉）；也可以输入 s 先跳过，cca 每天自动拉一次（cca App 开着时一小时一次，有变化就照着改账号表、同步路由器；想马上拉：App 的节点分流页点「拉取映射」，或者运行 `cca policy pull`）。`cca policy` 查看状态。
- **自动加账号**：映射和你的账号表合并。映射里有、这台机器上还没有的账号，拉到映射后自动加进账号表（别名用维护者起的，和你已有的别名重名时只写邮箱；已经加进来、还没有别名的，维护者之后在映射里起了别名也会补上），第一次启动它时登录；你自己加的、映射里没有的账号不受影响，你改过的别名也不会被改回去。
  - 不想要映射里的某个账号：`cca rm 别名` 删掉（在 `cca edit` 里删也一样），以后不会再自动加回来；想要了用 `cca add` 加回来。
  - 映射的节点不在这台机器的可选节点里时先不加：运行 `cca router setup` 选上那个节点，下次运行 cca 时就加进来（连不上路由器的服务器上，节点来自维护者，见「Linux 服务器」）。
  - Codex 账号要到第一次用 `cca codex` 时才加进来，不用 Codex 的机器不会白占路由器上的端口。
  - 维护者从映射里删掉了某个账号：你这台机器上的账号留着（登录还在），只是不再固定出口。
  - `cca policy` 里列出映射里还没加到这台机器上的账号和原因。
  - 想把某个账号现在的节点固定下来：`cca policy request 账号`（Codex 账号是 `cca codex policy request 账号`；App 里在账号的「…」或右键菜单选「请求维护者锁定节点」）会生成一条命令并复制到剪贴板，发给维护者；维护者运行它写进映射后，每天自动拉取时这个账号就固定在这个节点上了。
- **固定出口**：映射里的账号，添加时直接用映射的节点，不让选；账号表里改成别的节点，下次运行 cca 时会改回来，并提示一句。映射的节点不在这台机器的可选节点里时，运行 `cca router setup` 重新选可选节点。
- **节点改名**：路由器上的节点改了名（服务器地址没变）时，映射里还写着旧名字也照样能用：cca 跟着改名时记下了新旧名字的对应。维护者之后把映射更新成新名字就好。
- **验签**：映射要有发布 cca 用的那把密钥的签名才会被采用，签名不对、格式不对都不用，接着用上次拉到的。只管官方账号（按邮箱），第三方 API 账号不受影响。
- 这些检查都在你自己的 Mac 上做，用来防止配错，不防有意改动。`cca restore` 会删掉本机的 deploy key，记得让维护者把它从仓库上撤掉（`./policy.sh key rm 别名`）。
- 每台机器拉到的是整份映射（`~/.cca/policy/policy.json`，只有自己能读），里面有映射里所有人的邮箱和节点，映射里的账号也会加到每一台拉到它的机器上；只想给某些人用的账号、不想让大家互相看到的，维护者要分开建仓库。

维护者这边：

1. 建一个私有仓库（例如 `cca-policy`），放一个 `policy.json`：`{"version": 1, "claude": {"name@example.com": "US-A"}, "codex": {"name@example.com": "US-B"}, "aliases": {"name@example.com": "工作"}}`。`aliases` 可以不写：它是各台机器自动加账号时用的别名（Claude、Codex 通用），没写的账号只写邮箱。旧版本的 cca 会忽略这一项，照样拉得到映射。还可以加一项 `"install_node": "节点名"`：辅助安装直连不通时经内置代理走的节点（写它在路由器上、参考配置里的名字），各台机器问过再用它；同样可以不写，旧版本的 cca 也会忽略它。
2. 改好后在 cca 的仓库里运行 `./policy.sh 映射仓库的目录`：它会检查格式、确认仓库是私有的，然后用发布密钥签名，提交并推送。
3. 有人发来 cca 给的那一行（`./policy.sh key add 'ssh-ed25519 …' 别名`）时，看一眼，在 cca 的仓库里原样运行：加成只读的 deploy key，别名就是后面写的那个（和已有的重复时会提示另起一个）。那台机器上的 cca 正在等的话，几秒内就自己拉到映射。只发来公钥的，复制它运行 `./policy.sh key add`：读剪贴板里的公钥，提示给它起个别名（谁的哪台机器；直接回车用公钥备注里的机器名，不能和已有的重复）；公钥在文件里时写成 `./policy.sh key add 文件 [别名]`。之后按别名查、删、改：
   - `./policy.sh key`：列出各台机器的 key（添加时间、最近一次拉取、别名，标出你这台机器的）。
   - `./policy.sh key rm 别名`：删掉。那台机器以后拉不到新映射，已经拉到的照用。
   - `./policy.sh key mv 别名 新别名`：改别名。
   - 别名就是 GitHub 上 deploy key 的标题，仓库的 Settings → Deploy keys 里看到的也是它。GitHub 的 deploy key 不能修改，改别名是删掉后用同一把公钥重新加上，那台机器什么都不用做。
   - 经 gh 加的 key 跟着 gh 的授权走：在 GitHub 的 Settings → Applications 里撤销 GitHub CLI 的授权，这些 key 会一起被删掉；`gh auth logout` 只删本机的登录，不影响它们。
   - 默认管第 4 步说的默认仓库，别的仓库加 `-R 你/仓库`。
4. 默认仓库写在 `cca` 里（`POLICY_REPO_DEFAULT`）：用户运行 `cca policy setup` 不写仓库、启动时的引导，都用它。
5. 有人发来锁定命令（`cca policy request` 生成的 `./policy.sh lock claude 邮箱 节点`）时，看一眼内容，在 cca 的仓库里运行它：列出会改什么，确认后写进 `policy.json`，照常检查、签名、提交并推送（`-y` 不问）。映射仓库默认是 cca 仓库旁边的 `cca-policy`，别的位置加 `-C 目录` 或者设环境变量 `CCA_POLICY_DIR`。节点不在参考节点里时会提醒一句：路由器上没有这个节点的机器，这个账号会用不了。

### 设备统计：映射里的账号在哪些设备上用

维护者在映射里配置了设备统计的服务器之后，cca App 的账号页最下面多出「设备」一节：映射里的这个账号现在连着哪些设备，以及每台设备的状态。

- 状态分四种：
  - **在用**：开着会话，10 分钟内有动静。
  - **开着但闲置**：开着会话，很久没动静。
  - **离线**：开着会话，却很久没回报，可能休眠或断网了。
  - **没在用**：会话都关了，写着上次什么时候用。
- 命令行里看：`cca devices [账号]`，Codex 账号用 `cca codex devices`。

**哪些算在用**

- 经 cca 打开的命令行会话（`cca @账号`、`cxa @账号`），以及 `cca app` 打开的桌面 App。网页版不算，不经 cca 直接运行的 `claude`、`codex` 也不算。
- “动静”怎么看：
  - 命令行会话看它所在终端最后一次输入、输出的时间，和系统的 `w` 命令算闲置是同一个办法。
  - 桌面 App 看它用的 CPU。

**这台 Mac 报告了什么**

- 映射里的账号经 cca 用的时候，cca 在后台起一个上报进程，把这台 Mac 现在开着的会话报告给维护者的服务器。
  - 上报进程用后台优先级运行，每分钟一次，平时睡着，会话开始、结束时马上报。
  - 会话都关了，它报一声就退出。没有会话的时候，这台 Mac 上不运行任何东西。
- 只报映射里的官方账号。你自己加的账号、第三方 API 账号都不报。
- 折合金额（按官方 API 标价折算的用量，见下面「cca App」一节）也经这台服务器在设备间同步：映射里的 Claude、ChatGPT 官方账号，这台 Mac 登录着的，把这台 Mac 上按小时算好的金额加密后传上去，只有登录了这个账号的设备解得开。
- 服务器看到的东西：
  - 账号只看到一串摘要，看不到邮箱。
  - 设备名是维护者给你这台机器的 deploy key 起的别名。
  - 还有这台 Mac 的公网 IP，以及开着几个会话、闲置了多久。会话内容不会经过它。
  - 折合金额、记下的 API key 只看到密文（多大、什么时候传的），解不开。
- 请求用这台机器拉映射的 deploy key 签名，服务器只认维护者签发的设备清单。证书指纹写在签过名的映射里，连到的不是那台服务器时不发。
- 不想报告：在 `~/.cca/config.json` 里加上 `"presence": false`。这台 Mac 就不报告，也看不到这一节；折合金额、记下的 API key 也不再和其他设备同步。`cca policy` 里会说明这台 Mac 报不报。

**维护者这边**

- 部署：`./policy.sh presence deploy root@服务器`。
  - 服务器要装好 docker compose，默认放在 `/root/projects/cca-presence`，端口 18443，云服务商的安全组要放行这个端口。
  - 它会把地址、证书指纹写进映射目录的 `policy.json`，再把各台机器的 deploy key 推成服务器的设备清单。之后照常 `./policy.sh 映射仓库的目录` 发布。
- 设备清单：
  - 之后 `./policy.sh key add / rm / mv` 会自动同步，也可以手动运行 `./policy.sh presence sync`。
  - 撤销了 deploy key 的机器，服务器同时不再认它。
- 服务器的状态：`./policy.sh presence status`。
- 迁移：把服务器上的整个目录拷走，在新服务器上 `docker compose up -d`，再对新服务器运行一次 deploy。详见 `server/README.md`。

### 参考节点：自动补全 OpenAi 分流组

维护者还可以顺带发布自己 Clash 配置里的一个策略组（默认 `OpenAi`）和它引用的节点。各台机器拉映射时一起拉下来（同样要有发布密钥的签名，存在 `~/.cca/policy/nodes.json`，只有自己能读），你的配置里缺的直接写进你正在用的配置文件：

- **写到哪**：路由器上是 OpenClash 当前选用的配置文件（例如 `/etc/openclash/config/custom.yaml`），ShellCrash 是它的订阅配置 `yamls/config.yaml`；出门在外用的 Clash Verge 是它当前选用的配置（订阅或本地配置）。写进去的就是配置的一部分，打开配置文件就看得到。
- **怎么写**：缺的节点和你配置里别的节点一样一键一行（块写法），插在 `proxies` 开头，`name` 那一行行尾带 `# cca`；名字接在你的同名策略组后面（引号的写法照组里原有的成员），行尾也带 `# cca`；别的内容一个字节不动。改之前的配置留一份：路由器上在 `/etc/cca/source.bak.yaml`，Clash Verge 的在 `~/.cca/backup/`。
- **补什么**：这个组里的节点，你的配置里没有的。你已经有的不动：同名的节点照你自己的定义（只补进组）；同一个节点（类型、服务器、端口和 TLS 域名一样）在你那里叫别的名字，补进组的是你的名字；名字被你的策略组占了的不补；和 ShellCrash 自定义节点（`yamls/proxies.yaml`）同名的不补。你的配置里没有这个组，或者组写成了一行（`{name: OpenAi, …, proxies: […]}`），就只补节点。补上的节点也能选作账号的出口（`cca router setup`）；维护者的映射要哪个账号走参考节点时，这个节点自动加进可选节点，不用自己去选。
- **先校验**：每一版参考节点都先用你路由器上的内核做一次 `-t` 校验，通过了才写（别处的节点你的内核不一定认，认不出的话整个 OpenClash / ShellCrash 都起不来）。不通过就不写，端口池照常，用这些节点的账号暂时断开；内核升级以后会再试。Clash Verge 那边是热加载时它的内核收下了才写。`cca router status` 里能看到还缺哪些、为什么没写。
- **什么时候生效**：路由器上的配置文件写好后，在跑的 OpenClash / ShellCrash 要重启一次才用得上（约 20 秒，期间局域网的连接都会中断），cca 会先问你；不同意的话，就等它下次自己重启。Clash Verge 不用重启：cca 把这几个节点热加载进它当前的运行配置。
- **维护者改了节点**：带 `# cca` 的那个节点整个跟着改（同样先校验，路由器上同样要重启一次才用上）。你自己的同名节点、去掉了 `# cca` 的行都不动。维护者不再发布、或者删掉了某个参考节点，写进去的留着（卸载 cca 之后也一样），不要了就自己删。
- **订阅更新**：订阅更新会把配置文件整个换掉，写进去的节点也跟着没了。OpenClash / ShellCrash 更新完重启时，cca 再写回去，这次启动就能用上。注意 OpenClash 更新订阅时会拿新下载的和现有的配置文件比对，现有的多了这几行，所以订阅内容没变，也会被当成有更新、重启一次。
- 参考节点里是节点的完整定义（服务器、UUID、密钥），每台拉得到映射的机器都拿得到。

维护者这边：把你自己的 Clash 配置复制成映射仓库目录里的 `custom.yaml`，照常运行 `./policy.sh 映射仓库的目录`。它从里面抽出策略组 `OpenAi`（别的组名用 `CCA_FILL_GROUP=组名 ./policy.sh …`）引用的节点（组里的策略组、内置出站不算；规则、别的节点都不带），写成 `nodes.json`，签名后一起推上去。`custom.yaml` 本身不提交（里面有你全部的规则和局域网设备；已经提交过的会拒绝发布），也会加进这个仓库的 `.git/info/exclude`。想先看看会发布哪些：`./policy.sh nodes custom.yaml`。不再发布：删掉目录里的 `custom.yaml` 和 `nodes.json`，再运行一次。

## 查看用量

```sh
cca usage
```

```text
  账号                     套餐  5 小时/重置       本周/重置        备注
  工作  bob@example.com    Max    13% 2小时13分后   41% 周四 09:00
  个人  alice@example.com  Pro    98% 25分钟后      72% 明天 02:30  Sonnet 本周 60%
```

- 每个官方账号经它**自己的出口**查询，和在 Claude Code 里打开 `/usage` 看到的一样：5 小时额度、本周额度，以及按模型单独计的每周额度。几个账号一起查，一般几秒钟。
- 一个账号一行，整行不超过终端的宽度：终端窄了先收起有别名的账号的邮箱，再收起套餐，最后收起重置时间；备注放不下的项写成 `…`（`--json` 里什么都有）。
- 「账号」一列和菜单一样写别名和邮箱，没起别名的只写邮箱。
- 套餐的叫法和官网一样：Pro、Max（原来的 Max 5x）、Ultra（原来的 Max 20x）、Team、Team Premium（Team 的 Premium 席位）、Enterprise。新版 Claude Code 在账号里记了官方给的套餐名时，直接照它写。
- 升级、降级套餐以后，Claude Code 里显示的套餐（启动时的横幅、`/status`）可能还是旧的：它只在登录时记下套餐，之后不会自己更新（额度照实际的套餐算，不受影响）。`cca usage` 查到这种情况会在表格下面说一句，启动这个账号时的横幅、App 的账号页也会提示；重新登录一次（`cca login 工作`）就好。
- 查询由 Claude Code 自己完成（cca 让它以账号的配置无界面地查一下，不加载插件、hooks、MCP），**不发消息、不耗额度**，cca 也不碰登录凭证。
- 查之前同样会确认出口：端口不通、出口跑到了受限地区的账号不去查。查不到时显示上次查到的数据，备注里先写原因，再注明是多久前的。
- 菜单和 `cca ls` 里每个账号后面也会附上最近一次的用量（只读缓存，不联网，不拖慢菜单）。
- 每次查到用量，cca 顺带记下这一周用到多少（账号目录里的 `usage-weeks.json`，最多一年多）：cca App 的「每周用量」「用量统计」看的就是它，终端里不显示。中间重置过额度的那一周，重置前用掉的也算进去，所以可能超过 100%。
- `cca usage --json` 输出 JSON，方便接到脚本或状态栏里：`accounts` 里每个账号有 `plan`（套餐）、`source`（`live` 刚查到 / `cache` 查不到时用的缓存 / `none` 没有数据）、`error`（查不到的原因）和 `error_code`（同一个原因的代码：`login`、`port`、`timeout`……）、`fetched_at` / `fetched_epoch`、`stale`（超过 10 分钟），以及 `five_hour`、`seven_day`、`seven_day_opus`、`seven_day_sonnet` 等窗口（`{"utilization": 百分比, "resets_at": 重置时间, "resets_epoch": 重置时间（Unix 秒）, "reset": 重置时间是否已过}`）、`model_scoped`、`extra_usage`，`outdated_login_plan`（Claude Code 记的套餐过时了时是它记的那个，比如 `"Pro"`，否则为 null），和 `use_first`（本周额度快重置了、还剩至少 20% 时，建议先用这个账号的时间段：`left_percent`、`from_epoch`、`until_epoch`；cca App 在这段时间里提示，终端里不提示）。所有账号都没有数据时退出码是 1。完整格式见 [macos/PROTOCOL.md](macos/PROTOCOL.md)。
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
- **ChatGPT App（Codex 桌面版）**：`cca codex app 工作`，或者在启动台、聚焦搜索里点「ChatGPT 工作」。每个账号一份资料目录，用这个账号的 `CODEX_HOME`（和 `cca codex @工作` 同一份登录），App 的浏览器请求和遵守代理环境的子进程使用这个账号的出口。App 自己启动 codex 时 cca 加不了参数，所以第一次打开时会征求你同意，用 Codex 自带的 `codex features disable system_proxy_fallback` 在共享的 `~/.codex/config.toml` 里关掉回退到系统代理（之后直接敲 `codex` 也不再回退）。未同意关闭、关闭失败或当前 CLI 无法关闭时，不会启动 App。没登录时先在账号专属的 Chrome 里登录，App 打开时就是登录状态。如果 App 里还要你登录（比如登录过期了），别在 App 里登：它的登录页会在系统默认浏览器里打开，那一次不经账号的出口；关掉 App，运行 `cca codex login 工作`，再打开 App。
- **ChatGPT App 更新**：经 cca 打开的实例设置 `CODEX_SPARKLE_ENABLED=false`，关闭 App 更新请求，避免当前客户端从共享 `appData` 读取安装 ID 后通过更新检查发送。cca GUI 的「客户端更新」页可统一下载并验证官方更新；退出所有 ChatGPT 实例后，点安装并确认即可。它不会改写 App 的原生更新配置。这条共享 ID 调用链已核验 App 26.1021.31345；客户端升级后需重新验证，不代表所有 `appData` 或系统特征均已隔离。
- **其它命令**：`cca codex ls`、`edit`、`check`、`web`（用账号专属的浏览器打开 chatgpt.com）、`key`（换第三方 API 账号的 key），和 Claude 账号的同名命令一样。
- **简写 `cxa`**：`cxa` 就是 `cca codex`，后面接什么都一样（`cxa`、`cxa @工作`、`cxa usage`、`cxa add`……）。`cca install` 时和 `cca` 一起放进 `~/.local/bin`；用着 Codex 的机器上 `cxa` 不见了（比如以前装的 cca 升级上来），下一次运行 cca 时自动补上。已经有别的 `cxa` 命令时不会覆盖它。
- **删除**：`cca codex rm 工作` 退出登录、删掉账号目录（包括它的 git 工作树，删之前会提醒）；它开过的会话留在 `~/.codex` 里，cca 平时就把会话库里的路径写成 `~/.codex` 下的，删了账号也找得到。

启动前同样会检查：出口地区和纯净度（看 chatgpt.com 看到的 IP）；shell 里继承的 `OPENAI_*`、`CODEX_*`（例如会直接顶替登录的 `CODEX_ACCESS_TOKEN`）一律不带进去，只留公司网络根证书 `CODEX_CA_CERTIFICATE`；共享的 `~/.codex/config.toml` 或任一个 `*.config.toml` profile 里如果写了 `chatgpt_base_url`、`openai_base_url`、`experimental_realtime_ws_base_url`、`experimental_realtime_webrtc_call_base_url`、不是 `openai` 的 `model_provider`、不是 `file` 的 `cli_auth_credentials_store`、`forced_chatgpt_workspace_id`、`forced_login_method = "api"`、打开了 `respect_system_proxy`（也兼容旧版内嵌的 `profile` 配置），或者 `~/.codex/.env` 里写了代理、`OPENAI_*`，cca 会拒绝启动并指出来。账号内的配置合并回共享目录后还会再检查一次。

## Antigravity 账号

Google 的 Antigravity 命令行（`agy`）也能多账号，用法和 Codex 一样，前面加个 `agy`：

```sh
cca agy add            # 添加 Antigravity 账号：填 Google 账号的邮箱、别名（可选），选出口节点
cca agy                # 选账号启动 agy（cca agy @工作 直接启动，后面的参数原样交给 agy）
cca agy login 工作     # 重新登录
aga @工作              # 简写：aga 就是 cca agy（aga、aga add、aga ls 也都行）
```

- **账号表分开记**：记在 `~/.cca/agy.json`，格式和 `accounts.json` 一样（只有 Google 账号，值是节点名）。和 Claude、Codex 账号共用路由器（或本机 Clash）上的同一组端口，每个账号一个端口、一个出口。
- **各自登录**：每个账号有自己的数据目录（`~/.cca/agy/accounts/…/gemini`，cca 启动 agy 时用 `--gemini_dir` 指过去）。第一次启动时，cca 让 agy 生成 Google 的登录网址，补上账号的邮箱（`login_hint`），在**这个账号专属的 Chrome 窗口**里、经它的出口打开；登录后页面上显示一段授权码，把它（或者整个网址）粘贴回终端，cca 核对过再交给 agy，换 token 也经这个账号的出口。登录后 cca 还会核对邮箱，登成别的号就不启动。你原来 `~/.gemini` 里的登录不受影响，直接敲 `agy` 用的还是它。
- **登录存在账号自己的目录里**：agy 平时把登录存进 macOS 钥匙串，不管数据目录在哪都是同一份，没法分账号。cca 启动 agy 时设上 `SSH_CONNECTION`（值是本机地址），agy 看到 SSH 会话就把登录存成数据目录里的文件（`antigravity-cli/antigravity-oauth-token`，只有你能读）。这不是 agy 公开的做法：每换一个版本，cca 先在临时目录里试一次（不联网），确认它还这样才用，不是就停下、不启动。副作用是 agy 会当自己在 SSH 会话里运行。cca 只读这个文件里的邮箱和套餐档位，不碰里面的 token。
- **工作资料共用**：`~/.gemini` 里的会话（`conversations`）、记忆（`brain`）、历史、`settings.json`、`keybindings.json`、`GEMINI.md`、`AGENTS.md`，以及 `config/` 下的 MCP、技能、规则、工作流、插件等，所有账号共用一份；`--continue` 认的“这个目录上次的会话”也共用，换个账号照样接着聊。
- **不共用的**：登录、两个装机标识（`installation_id` 和 `jetski_state.pbtxt` 里的 `installation_uuid`：添加账号时 cca 给每个账号各生成一份，之后固定不变）、日志、会话摘要的索引（`conversation_summaries.db` 等，agy 会按共享的会话重建）。
- **没有的**：查用量的接口（在 agy 里用 `/usage` 看）、第三方 API 账号、桌面 App 和控制台的账单页；`cca agy usage`、`add api`、`app`、`billing` 会说清楚，什么都不做。
- **其它命令**：`cca agy ls`、`edit`、`check`（出口检测：Google 的网站没有 `/cdn-cgi/trace`，经同一个端口看 Cloudflare 看到的 IP）、`web`（用账号专属的浏览器打开 Gemini 网页版 gemini.google.com）、`rm`，和 Claude 账号的同名命令一样。服务器上登录和 Claude、Codex 一样：给出在 Mac 上运行的 `cca agy login-page …` 那一行，Mac 上打开登录页，授权码抄回服务器的终端。
- **简写 `aga`**：`aga` 就是 `cca agy`，后面接什么都一样（`aga`、`aga @工作`、`aga add`……），和 Codex 的 `cxa` 一样：`cca install` 时和 `cca` 一起放进 `~/.local/bin`；用着 Antigravity 的机器上 `aga` 不见了（比如以前装的 cca 升级上来），下一次运行 cca 时自动补上。已经有别的 `aga` 命令时不会覆盖它。
- **图形界面**：用过 `cca agy` 之后，总览和侧栏里多一组 Antigravity 账号（装了 `agy` 命令时，侧栏底部也能「启用 Antigravity 账号」），能打开终端、网页版、检测出口；没有用量和桌面 App。

启动前同样会检查：出口地区和纯净度；shell 里继承的 `GEMINI_*`、`ANTIGRAVITY_*`、`AGY_*`（例如会换掉账号的 `AGY_ACCOUNT`）、`GOOGLE_API_KEY`、`GOOGLE_APPLICATION_CREDENTIALS`、`GOOGLE_CLOUD_PROJECT` 这类会顶替登录或换项目的变量一律不带进去，只留界面开关 `AGY_CLI_*` 和公司网络根证书 `AGY_CA_CERT`；共享的 `~/.gemini/antigravity-cli/settings.json`（或这次的 `--settings`）里写了 Google Cloud 项目（`gcp`：各账号会记到同一个项目上），或者启动参数里另给了 `--gemini_dir`，cca 会拒绝启动并指出来。

## Claude App（桌面版）

Claude 的 Mac 桌面 App 也能每个账号开一份：

```sh
cca app 工作
```

也可以在启动台、聚焦搜索里点「Claude 工作」。cca 会给每个账号生成一个这样的启动器（放在 `~/Applications`），名字是"Claude"加别名，没起别名的用邮箱 @ 前面的部分（比如「Claude me」）；图标右下角有账号标识（别名的头两个字或首字母，每个账号一种颜色），在启动台里一眼就能分清。账号改名、删除时，启动器自动跟着改。

- 每个账号的 App 有自己独立的资料（登录状态、设置）；浏览器请求和遵守代理环境的子进程使用账号出口，打开前检查出口。启动时同样清理继承的凭证、接口和代理变量。外部浏览器登录与不遵守代理的网络流量不在此保证内。
- App 里的 Code 标签页和 `cca @工作` 用同一个账号配置，记忆、技能、设置照样共享。
- 这个账号的 App 已经开着时，先重新检查出口、共享配置和启动记录，一致才切到前面。路线、节点、程序版本或启动参数变化，以及升级前没有启动记录的旧实例，需要先保存工作、退出该实例再打开；cca 不会强行关闭已有实例。
- 你原来的 Claude App 不受影响，照常使用。

需要知道的几点：

- 正在运行的 Claude 在 Dock 里的图标改不了，开了几个账号就有几个一模一样的 Claude 图标；分不清时用 `cca app 账号` 或启动器切换。
- 第一次打开要登录。用邮箱登录全程走这个账号的出口；用 Google 登录时，那一次登录会走系统网络。不要同时在两个 App 里登录。
- 在别的 App 里点开的 claude.ai 链接，macOS 只会交给其中一个 Claude App。
- 这个功能依赖 Claude App 支持独立的资料目录。启动或激活已有实例时，cca 用 `lsof` 核验该实例或其子进程正在打开账号资料目录内的文件，并核对进程启动时间、实际程序路径、版本和代理参数；旧的 `Local State` 文件不算运行证据。新启动无法核验或启动中被取消时，会关闭能识别的本次实例；已有实例无法核验时，提示你自行退出后重开。这个核验不等于对 App 全部网络和硬件指纹的验证。

## 图形界面（cca App）

```sh
cca gui
```

第一次运行会下载和 cca 同一个版本的 cca App，装进 `~/Applications/cca.app` 后打开（要 macOS 26 或更新，Intel 和 Apple 芯片都能用）。它是 cca 的一个窗口加一个菜单栏图标：

- 左边是 Claude、ChatGPT（Codex）两组账号，每个账号显示套餐、登录状态、5 小时和本周用量（Claude 账号还有 Fable 用了多少）；
- 选中一个账号：用量的明细和重置时间、走哪个节点和端口、路由器上这个槽位现在选中的节点、上次检测到的出口（IP、地区、纯净度、DNS / WebRTC 泄露）；
- 一键打开这个账号的 Claude App / ChatGPT App、网页版，检测出口，刷新用量；
- 本周额度快重置了、还剩不少的账号，名字旁边有个沙漏，提醒你先用它，别浪费；
- 每周用量：账号页的「每周用量」是最近 12 周每周用到额度的百分之几（一周一根竖条，填满就是用完，鼠标指着一周看细节），左边的「用量统计」把所有账号排在一起比，哪个总不够用、哪个常常剩一大半，一眼看出来。Claude、OpenAI 不给以前的用量，所以是 cca 每次查用量时记一笔，从装上这个版本的那一周开始一周一周攒起来；
- 折合金额：把每个账号用掉的 token 按 Anthropic、OpenAI 官方 API 的标价折成美元，看哪个账号用得多（不是实际花费：订阅不按这个收费）。「用量统计」右上角在「额度用量」和「折合金额」之间切换：每个账号一行，最近 12 周一周一根竖条，右边是平均每周、本周和 12 周合计，用过它的每台设备在下面各占一行；账号页的「折合金额」按周或按日（最近 30 天）画柱状图，可以看整个账号，也可以只看一台设备。数字是 cca 在后台读本机的会话记录（`~/.claude/projects`、`~/.codex/sessions`，只读用量的数字，不读对话内容）按小时算的；维护者映射里的账号经设备统计的服务器加密同步，整个账号的合计包括每台设备上用的。不想统计：在 `~/.cca/config.json` 里加上 `"cost": false`；
- 账号的 API key（第三方 API 账号的，或者给官方账号记下备查的）：默认只显示掩码，点眼睛看全文，一键复制（剪贴板标成机密，60 秒后自动清掉）；维护者映射里的官方账号，在一台设备上记下、换掉、删掉，登录了这个账号的其他设备上也跟着变（经设备统计的服务器加密同步，服务器解不开），账号页写着是从哪台设备同步来的；
- 「节点分流」一页列出端口池每个槽位：应选的节点、路由器（在外面是本机 Clash Verge）上实际选中的、端口通不通，不一致时一键同步路由器；
- 菜单栏图标里是精简的账号列表和用量（5 小时、本周，Claude 账号还有 Fable），点一下就能打开账号的 App。

App 里做的每件事都是交给 cca 做的，检查和命令行里完全一样：要重载或重启 OpenClash / ShellCrash、出口纯净度偏低还要不要用、第一次打开 ChatGPT App 要关掉 Codex 的系统代理回退、在外面要打开 Clash Verge……都会弹窗问你，同意了才做。添加账号、换 key、删除账号、编辑账号表（节点、接口地址、模型、环境变量）、配置路由器、配置本机 Clash 这些要回答问题的事都在 App 里填表，检查和命令行里一样（配置路由器要用一次 SSH 密码时也在 App 里输入，只用这一次、不保存）；登录这些要用终端的事，App 会在「终端」里打开对应的 cca 命令。

- 平时在后台运行，只有屏幕顶上的菜单栏图标：点图标里的「打开主窗口」时才有窗口和 Dock 图标，关掉窗口又回到后台。在 Dock 里点“退出”、按 ⌘Q 也只是回到后台（用量继续更新，内置代理不停）；要彻底退出，点菜单栏面板里的「退出」，或者应用菜单的「退出 cca」。
- 账号页的「终端」按钮用你选的终端，其它要在终端里做的事（登录……）都用系统自带的“终端”。
- 装好以后不用管它：cca 自动更新到新版本后，下一次运行 cca 时会在后台下载对应版本的 App（App 开着就等它退出再换，也可以在 App 里点「更新并重新打开」）。`cca gui update` 立即更新，`cca gui remove` 删掉它。
- App 用 curl 下载、用发布密钥验过签名才装（和 cca 本身一样）。它没有苹果的公证，所以**不要用浏览器从 Releases 页面下载 cca-app.zip**：那样会被 Gatekeeper 拦下。
- 它通过 cca 的只读接口（`cca _gui`）读数据，格式见 [macos/PROTOCOL.md](macos/PROTOCOL.md)。

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

- **指定节点是否就绪**：路由器或本机内核切换节点后再读取实际结果；控制接口失败、结果不一致或需要重载但未完成时停止，端口开放不算验证通过。目前采用整个端口池核验：其他账号缺少节点也会阻止启动，需要先修复该节点或调整账号映射。出口探针忽略用户 `.curlrc`，显式固定代理及绕过规则，避免 shell 的 `NO_PROXY` 改变检测路线。
- **出口所在地区**：如果出口在 Claude 不提供服务的地区（例如中国大陆、香港、俄罗斯），直接停止。
- **IP 纯净度**（数据来自 ip.net.coffee）：分数太低会先问你还要不要继续。
- **DNS / WebRTC 泄露**（照 ip.net.coffee 的 [DNS](https://ip.net.coffee/dns/)、[WebRTC](https://ip.net.coffee/webrtc/) 检测页的做法）：和出口检测同时在后台测，结果写在启动时的「泄露」那一行；只提示，不拦着启动。
  - **DNS**：经这个账号的端口访问一次性的子域名，看是哪些 DNS 服务器来解析的。出口不在中国大陆、解析器却在中国大陆，就是泄露（网站的 DNS 能看出查询来自中国大陆），一般是节点在本地把域名解析好再发出去（比如直连、WireGuard 节点）。要几秒，所以结果按节点记 6 小时，平时启动不用等它。
  - **WebRTC**：不经代理向 STUN 服务器（UDP）探测这台 Mac 的出口。与账号出口不同时提示“直连风险”；一致时只说明 STUN 出口一致，均不代表已在浏览器内验证。超时、失败或没有取得 IP 时显示“没测成”，不会显示“无泄露”。账号 Chrome 另设 `--webrtc-ip-handling-policy=disable_non_proxied_udp`，关闭时合并独立 profile 的偏好并清除 WebRTC URL 例外，运行时发现偏好改变会要求重开。此限制可能影响语音或视频连接；企业策略、扩展优先级及原生 App 仍需单独验证，不能把宿主探针结果当成浏览器验收。
  - `cca check` 每次都重新测，并写出解析器在哪；不想测的话在 `~/.cca/config.json` 里加上 `"leak_check": false`。`cca web` 也执行启动前出口与泄露检查。DNS 没有返回解析器或无法确认归属地时按未验证处理；正常结果仅说明未发现地区异常，不代表已验证浏览器 DNS。
- **登录的是不是这个账号**：如果这个账号的窗口里登录成了别的邮箱，会拒绝启动并提示你重新登录，免得一个账号用了另一个账号的出口。
- **共享设置有没有"串号"的配置**：`~/.claude/settings.json`（或 `settings.local.json`）的 env 里如果写了代理（包括 `NO_PROXY`）、`ANTHROPIC_BASE_URL`、`ANTHROPIC_AUTH_TOKEN`、`ANTHROPIC_API_KEY`、`CLAUDE_CODE_OAUTH_TOKEN`、云厂商开关（`CLAUDE_CODE_USE_BEDROCK` / `VERTEX` / `FOUNDRY` / `MANTLE` 等），或者写了 `apiKeyHelper`，会盖掉每个账号自己的设置（官方账号的登录 token 甚至会被发到别的地址），cca 会拒绝启动并指出来。
- **shell 环境**：shell 里 export 的 `ANTHROPIC_*`（包括 `ANTHROPIC_MODEL`）、`CLAUDE_CODE_OAUTH_TOKEN`、`CLAUDE_CODE_*_FILE_DESCRIPTOR` 和云厂商开关，cca 启动时一律不带进去，并提示屏蔽了哪些。Claude 的 `CLAUDE_CONFIG_DIR` 和 `CLAUDE_SECURESTORAGE_CONFIG_DIR` 均固定为账号目录，保持原有钥匙串命名；共享及项目 settings 也不能覆盖这些凭证来源。官方账号要换模型，写进 `~/.claude/settings.json` 或在会话里用 `/model`。
- **第三方 API 账号**：只确认接口地址经它的出口连得上。

## 自动更新

### 桌面客户端和终端工具

cca GUI 的「客户端更新」页管理这台 Mac 上的 Claude、ChatGPT 桌面客户端（`com.anthropic.claudefordesktop`、`com.openai.codex`）和终端里的 Claude Code、Codex。GUI 运行期间默认每 6 小时检查一次，即使主窗口隐藏也会检查；桌面客户端发现新版本后下载到独立缓存，验证官方包签名（Claude 是官方给的 SHA-256 和大小）、App 签名、开发者身份和版本，再显示可安装状态。终端工具只查版本：官方安装器装的用它们自己的 `update` 命令更新，npm、Homebrew 装的提示在终端里运行什么。开发模式和演示模式不会自动检查。

默认安装前先问你：请先退出这个客户端的所有实例，再点「安装更新」；下载不会打断正在使用的 App，确认只适用于当时检查过的安装包和目标版本。打开「允许自动更新」后，查到新版本就直接安装、不再询问：桌面客户端开着时不会被关掉，等你退出它以后再装（GUI 看到它退出就来装，最晚 15 分钟后）；终端里开着的会话继续用旧版本，新开的才是新版本。不管哪种方式，原生更新器或其他工具改动了 App 时都会停止本次安装、要求重新检查；不会自动降级，也不会自动提权。

命令行使用同一流程（桌面客户端需要安装或编译好 cca GUI，供安装包验签使用）：

```bash
cca clients status                 # 查看状态
cca clients check --force          # 立即检查、下载并验证（允许自动更新时顺便装好）
cca clients install TOKEN          # 将 TOKEN 换成检查给出的凭据；确认后安装
cca clients auto-install on        # 允许自动更新：查到新版本就装，不再询问
cca clients auto-install off       # 恢复安装前先确认
cca clients mode off               # 关闭自动检查（允许自动更新也一起关掉）；手动检查仍可用
cca clients mode auto              # 恢复自动检查和准备
```

不改变这些客户端自己的更新设置。更新包请求不带账号登录凭证或共享安装 ID，但更新服务器仍可看到下载连接的 IP。新版本通过发行方签名验证，不等于已经重新完成设备指纹隔离审计。

### cca 本身

不用管它。cca 每天会在后台检查一次新版本，下载好之后，在下一次启动时自动换上。

- 只会安装带有效签名的正式版本，被篡改的文件不会装，测试版也不会自动装。
- 新版本有问题：`cca update --rollback` 退回上一版。
- 立即更新：`cca update`；只提示、不自动装：`cca update notify`；完全关闭：`cca update off`。

### 测试版

新功能有时先发测试版（版本号像 `2.8.0-beta.1`，在 [GitHub Releases](https://github.com/rivia7/claude-accounts/releases) 里标为 Pre-release）。自动更新和一键安装都不会装测试版，想试的话手动装（下面的 `X.Y.Z-beta.N` 换成那个测试版的版本号）：

- 已经装着 2.6.0 或更新的版本（2.6.0 的测试版也行）：`cca update X.Y.Z-beta.N`（照 GitHub 上 tag 的样子写成 `vX.Y.Z-beta.N` 也行；装着的是 2.8.0-beta.5 或更早的版本时，只认不带 v 的写法）
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

撤销 cca 做过的全部改动：每个账号（Claude、Codex 和 Antigravity）退出登录，并删除它的登录凭证（钥匙串里第三方 API 账号的 key、给官方账号记下备查的 key 也一起删掉）；路由器上的端口和配置全部移除；Clash Verge 全局扩展脚本里 cca 加的那段删掉（维护者的参考节点写进了你的配置文件的，留着：行尾带 `# cca`，不要的话自己删）；最后删除本机的 cca（包括 `cxa`、`aga`）、各账号的 App 启动器和图形界面 cca App（删除前会把配置打包备份到你的主目录，不含登录凭证）。Codex 账号建的 git 工作树（`worktrees/`）在账号目录里，会一起删掉，也不在备份里，还原前会提醒。你原来的 `~/.claude`、`~/.codex`、`~/.gemini`（对话记录、设置等）不受影响。卸载前先退出各账号的桌面 App。

## 常见问题

**会保存我的路由器密码吗？**
不会。密码只用一次，用来装免密登录的钥匙，之后 cca 都用这把钥匙登录路由器。

**cca 会碰我的 Claude 登录凭证吗？**
不会。登录完全走官方的 `claude auth login`，凭证由 Claude Code 自己存进 macOS 钥匙串；cca 不读取、不保存，也不转发任何登录凭证。`cca usage` 查用量也是让 Claude Code 自己去查。

在 Linux 服务器上没有账号专属的 Chrome：`cca login 工作` 照常在服务器上跑官方的登录，给出一行 `cca login-page …`，在 Mac 上运行它，Mac 上的 cca 用这个账号专属的 Chrome、经同一个节点打开登录页，登录完把验证码复制到剪贴板，贴回服务器即可（节点按地址认，两边叫法不同也行；Mac 上还没有这个账号时，会问要不要用同一个节点加上它）。经手的只是这个一次性的验证码：cca 核对过是这一次登录的，才交给服务器上正在等着的 Claude Code（或 codex），由它自己经账号的出口换成凭证、自己保存（Claude Code 在 Linux 上存成账号目录里只有你能读的 `.credentials.json`）。

**Codex 的登录凭证呢？**
登录、刷新都由 codex 自己完成，凭证由它写进账号目录里的 `auth.json`。cca 只从里面的 id_token 读出邮箱、套餐和工作空间 ID（核对登录的是不是这个账号），不碰真正用来访问的 access / refresh token，也不保存、不转发。

**不同账号的设备指纹独立吗？**
客户端设备标识独立：Claude Code 使用各账号自己的 `userID`、`machineID`，Codex 使用自己的 `installation_id`。cca 在登录前准备好这些随机标识，保存在账号目录的 `device.json`，再写入客户端配置；重新登录、改别名、换节点都沿用，客户端清掉标识时也会从这份记录恢复。旧账号已有合法且独立的标识会保留；与主配置或其它本机账号重复、通过链接共享的标识会自动分开。删除账号时一起删除，重新添加会生成新标识。

这里隔离的是客户端设备标识。Chrome 和桌面 App 的 Cookie、站点存储仍由各账号的独立资料目录隔离；Canvas、WebGL、字体、GPU、屏幕、时区和操作系统等浏览器或硬件特征不会因此改变。签名原生 App 没有统一的指纹替换接口；需要独立操作系统环境或设备才能扩大隔离边界。独立虚拟机也不会自动产生不同的 Canvas/GPU 结果，须逐项实测，仍不能承诺服务端无法关联。未运行网页或 WebView 的 CLI 不适用 Canvas/WebGL 指标。cca 不通过注入不完整的 JavaScript 补丁声称已隔离硬件。

Chrome 的启动记录同样绑定进程、程序路径、版本、账号节点和代理。没有匹配记录的旧窗口需要退出重开；新进程先打开空白页并核验，再主动交付目标网址（Chrome 自身可能恢复旧标签或发起后台请求）。资料目录和运行记录不能通过链接与其它账号共用。这些检查不验证网页 Cookie 所属的实际登录账号，也不改变直接打开原版 App 或原始 `claude` / `codex` 命令的行为。

**历史、记忆和配置还会跨账号共享吗？**
会。Claude 的 `projects`（会话和项目记忆）、`history.jsonl`、设置、指令、技能、规则、插件、hooks 脚本等保持共享；Codex 的会话、历史、记忆、配置和相关数据库也保持共享，切换账号后仍可续接会话。登录凭证、设备字段和已识别的账号运行状态独立保存。共享清单以 `cca` 中的 `CLAUDE_SHARED` / `CODEX_SHARED` 为准，未知新增条目默认不共享；插件目录内部的数据仍随插件目录共享。

CLI 启动和正常退出时会合并配置并修复被客户端原子保存替换掉的共享链接；桌面 App 在下次经 cca 启动时处理，不承诺运行中的配置立即刷新。顶层 JSON 配置按字段三方合并；同一字段冲突时保留共享值，完整账号副本存进 `~/.cca/conflicts/`（Codex 是 `~/.cca/codex/conflicts/`）并显示路径。TOML、指令文件和 `.env` 在双方同时修改时保留共享文件和完整冲突副本，不自动拼接。无效 JSON 也会保留副本，不覆盖有效共享配置。

升级时，旧版指向共享主目录的未知条目链接会拆成账号自己的副本，源文件保留；用户自建的外部链接不动。旧共享目录若含内部符号链接，自动拆分会停止并指出路径，避免追随外部目录或循环链接。已有设备 ID 不因这项迁移重置。共享与续接会保留共同上下文，因此客户端标识独立并不意味着服务端无法关联账号。

**第三方 API 的 key 存在哪？**
默认存在 macOS 钥匙串里（条目名 `cca-api-…`），启动时经 Claude Code 的 `apiKeyHelper` 交给它，不出现在命令行参数和环境变量里。删除账号（`cca rm`）或一键还原（`cca restore`）时一起删掉。给官方账号记下备查的 key（`cca key 工作`）也存在钥匙串里（条目名 `cca-ref-…`，Codex 的是 `cca-codex-ref-…`），cca 绝不拿它登录或启动账号，同样随 `cca rm`、`cca restore` 删掉（只删这台设备上的）。维护者映射里的 Claude、ChatGPT 官方账号、开着设备统计时，这把 key 经维护者的设备统计服务器同步给登录了这个账号的其他设备：只传密文，密钥由账号的登录身份派生，服务器和没登录过这个账号的设备都解不开；`cca key 工作 --forget` 删掉时其他设备上同步来的也一起删。只有你自己在账号表里写了 `key` 字段时，key 才会以明文存在账号表里（文件只有你能读；`cca restore` 打的备份包也只有你能读）。Linux 服务器上没有钥匙串，这些 key 存成 `~/.cca/secrets` 下只有你能读的文件（目录 700、文件 600），同样随 `cca rm`、`cca restore` 删掉，不进备份包。

**cca 启动的 Claude Code 能直接看、改 `~/.cca` 吗？**
只能直接看、改这个账号自己的目录。cca 启动 Claude Code 时把 `~/.cca/accounts/` 下这个账号的目录加成工作目录（`--add-dir`），里面有它的用量、出口检测结果和 Claude Code 在这个账号下的配置（`claude/`），Claude 帮你看这些时不用一次次确认；改文件照你选的权限模式来。账号表、`config.json`、映射、日志和别的账号的目录不在工作目录里，Claude 要读时照权限模式来（默认会先问你）。登录凭证和浏览器资料不管在不在工作目录里都不让碰：映射的 deploy key 私钥、维护者参考节点（`policy/nodes.json`，里面有节点的密钥）、写参考节点前留的 Clash Verge 配置备份（`backup/clash-verge-*.yaml`）、Codex 账号的 `auth.json`、各账号（包括这个账号自己）的 Chrome 和桌面 App 资料目录，cca 用 `--disallowedTools` 挡住了 Claude 的读写，直接拒绝、不会来问你（账号表里如果写了明文的 `key` 字段，你让 Claude 读账号表时它是看得到的，key 最好存钥匙串）。不想放开的话，在 `~/.cca/config.json` 里写上 `"claude_add_dir": false`。只管 cca 启动的 Claude Code，Codex 和桌面 App 不受影响。

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
- **网页版**：每个账号使用独立的 Chrome 资料目录和账号代理。未安装 Chrome、打开失败或 Codex 不支持独立登录流程时会停止，不会退回默认浏览器。浏览器中手工切换的登录账号不由 cca 自动核验；显示的账号名称表示资料目录归属，不是网页当前登录身份的证明。
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
- 在 Linux 上测：`bash tests/linux.sh`（临时 HOME、假的 claude / codex，不碰你的 crontab；要联网下载内核、测真实的出口）。覆盖一键安装和自动更新、内置代理（含重启后自己起来）、服务器上的登录、密钥文件、维护者的映射、设备统计、Codex、出口和泄露检测、Linux 桌面上的 Clash Verge。装了 Docker 的话 `bash tests/linux.sh --docker` 在 Ubuntu、Debian、Rocky、AlmaLinux、Fedora、Amazon Linux、Alpine、Arch、openSUSE 的容器里各跑一遍，容器里还测在家走路由器（假路由器上跑真的路由器端脚本和 mihomo 内核）。
- 只测设备标识的隔离、迁移和并发：`python3 tests/device_identity.py`（完整回归也会跑）。
- 隔离防护专项：`python3 -m unittest discover -s tests -p 'isolation_*.py'`，覆盖配置解析、危险覆盖、App 环境与资料目录验证、浏览器失败路径及泄露检测状态；使用临时目录和测试替身，不登录真实账号。
- 原生客户端兼容性测试（可选）：`CCA_TEST_CLAUDE_BIN=/绝对路径/claude python3 tests/isolation_credentials.py`，使用临时 HOME、假钥匙串和假凭证，验证实际客户端的凭证来源；`CCA_TEST_CHROME_BIN='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' python3 tests/isolation_chrome_native.py` 使用空 profile 和本机测试页/STUN 验证 WebRTC。需要 macOS 允许启动测试进程和本机监听；默认不执行这些原生测试。
- 只测历史/记忆/配置共享、旧链接迁移、配置合并和冲突保留：`python3 tests/shared_state.py`（完整回归也会跑）。
- 图形界面 App 的源码在 `macos/`（SwiftUI，只用苹果的命令行工具就能编，不用 Xcode）：`bash macos/build.sh` 编出 `macos/build/cca.app`，`bash macos/test.sh` 跑它的契约测试。开发模式（`cca` 旁边有 `.git`）下 `cca gui` 直接从源码编译并打开它。App 和 cca 之间的接口见 [macos/PROTOCOL.md](macos/PROTOCOL.md)。
- 发布新版本：改 `cca` 里的 `VERSION` 并提交，然后运行 `./release.sh "更新说明"`。它会跑测试、签名、编译界面 App、打 tag、推送，并创建 GitHub Release。

## 许可证

[MIT](LICENSE)
