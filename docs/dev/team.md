# Team

目标：把「维护者共享的账号」（维护者的映射：3.6.0 起去掉了，见「3.6.0 去掉的旧通道」）和「自己的同步服务器」（`cca hub`：3.5.3 起去掉了，见「自己的同步服务器 → Team」）合成一个概念；3.6.0 起 Team 是唯一的通道。一个 Team 是一组设备加一份配置（哪些账号、各走哪个节点、节点的定义），
配置由管理员的设备维护、存在这个 Team 的同步服务器上；管理员给每台设备发邀请、定它能用哪些账号。一台设备可以在几个 Team 里。分阶段做，这一份跟着写：

| 阶段 | 内容 | 状态 |
|---|---|---|
| M1 | 线上格式和服务端：服务器的 `--mode team`、管理员和设备的 op、邀请链接、每台设备的配置（加密、签名），照文档独立实现一遍来测 | 做完了 |
| M2 | 辅助程序和命令：Team 在本机的状态（`~/.cca/teams/<编号>/`）、`cca team` 的建立、邀请、加入、同步配置、看设备、移除、退出；管理员这边的账号、节点、每台设备的授权 | 做完了；服务器要自己起（安装脚本是 M6） |
| M3 | Team 成为唯一的概念：一个地方回答「这个账号属于哪个 Team」，代替 `HubKind`、`route` 和映射的读取；对账、收起账号、三条同步管线都按 Team；当时的维护者映射当作旧式的 Team 接进来 | 做完了：账号表照 Team 的配置改，设备统计、记下的 API key、折合金额经各个 Team 的服务器同步（见「账号表照配置改」「同步经 Team 的服务器」）。维护者的映射当时照旧并着用，3.6.0 删掉（M7） |
| M4 | 迁移：导入现有的映射和参考节点；用着维护者映射的机器自己迁过来；自己的同步服务器升级成 Team | 做完了（见「迁移」）。迁映射用的那几样（导入映射文件、`./policy.sh team` 给各台机器发邀请、机器拉到映射后自己加入）3.6.0 跟着映射删掉了；留着的是 `cca team import Team --mine`（自己的账号建成一个 Team：自己的同步服务器去掉以后就是这么做） |
| M5 | 界面：`_gui state` 的 `teams`、账号的 `team`；侧栏、总览、菜单栏面板按 Team 筛选；管理员的管理页 | 做完了（见「界面」），App 里也能新建 Team、部署它的服务器（M6） |
| M6 | 部署：安装脚本在一台服务器上装几个 Team（各是一个服务）、托管平台 | 做完了（见「部署」）：`cca team deploy`、安装脚本（`--team-root`；3.5.3 起只装 Team 的，同时给这个 Team 的远程会话做中转；手机的网页端和证书助手是 team-only 这个分支里加的：3.5.3-beta.5 起一台服务器上几个 Team 共用一个证书助手 `cca-team-acme`）、`cca team hosted`（终端里）；App 里「新建 Team…」「部署到我的服务器…」 |
| M7 | 删掉旧通道：自己的同步服务器；维护者的映射（GitHub 上的映射仓库、手工登记 deploy key、`policy.sh`）和维护者的服务器 | 自己的同步服务器（`cca hub`、`HubKind::Own`、`--mode own`、`own.` 开头的状态文件、安装脚本里口令那一半、App 的卡片、配置向导问它的那一步）3.5.3 起删掉了（升级时 `hub_leftovers` 清掉留下的文件，以前装的 `/opt/cca-hub` 安装脚本只让卸载）。维护者的映射和维护者的服务器 3.6.0 删掉了（做完了、还没发布；删了什么、留了什么见「3.6.0 去掉的旧通道」）：3.5.3-beta.8 起维护者的日常（节点用 `cca team node` 改，加外部账号的 skill）已经在 Team 里，映射仓库不再加东西；没迁过去的机器升级以后说一声，向 Team 的管理员要邀请。发布以后还要做的，每一样都先问维护者：停掉维护者那台服务器上设备统计和中转的两个端口、把映射仓库归档、删掉仓库上各台机器的 deploy key |

定下来的（用户 2026-11-21、22 定的，照它做，别重新讨论）：

- 照「干净架构」做：Team 是一等概念，旧的两套（`HubKind` 的 maintainer / own、`policy_*`、`own.` 开头的文件）只留作迁移用的垫片，写明什么时候删（own 那一套 3.5.3 起、maintainer 和映射 3.6.0 起删掉了；`policy_*` 这些名字留着，管的是 Team 给的账号）。
- 授权是**每台设备一个邀请链接**：管理员生成时勾好这台设备能用哪些账号（以后能改），对方粘贴就行，没有「申请—批准」的来回。
- 一台服务器可以放几个 Team，**每个 Team 一个服务**（容器）：服务端进程还是只管一个 Team。
- 弃用公钥授权：不再手工加 deploy key、不再从 GitHub 拉映射；维护者的 `policy.json` + `custom.yaml` + `./policy.sh` 换成管理员设备上的 `cca team` 和 App 的管理页，
  当时的映射和参考节点导入一次（3.6.0 起映射整个去掉了）。锁定请求不要了。
- 界面上按 Team 筛选的是侧栏、总览、菜单栏图标点开的面板；筛选只管显示，`~/Applications` 里有哪些启动器不跟着变。
- 不经 Team 共享第三方 API 账号（等于把 key 发给别人）。

## 3.6.0 去掉的旧通道（M7）

3.6.0 删掉了维护者的映射和维护者的服务器（做完了、还没发布），Team 是唯一的通道。自己的同步服务器 3.5.3 起就没有了（还留着的只有升级时
清掉旧文件的 `hub_leftovers`，和安装脚本认得 `/opt/cca-hub`、只让卸载它）。3.6.0 删掉的：

- 映射：`~/.cca/policy/`（policy.json、nodes.json、deploy key、`known_hosts`、`checked`、`log`、`lock/`）和它的拉取、验签（bash 的 `policy_*`：
  `policy_setup`、`policy_prepare`、`policy_fetch`、`policy_pull`、`policy_hook`、`policy_wait`……）；共享的开关（`cca policy on|off|setup`，
  config.json 的 `policy`、`policy_was`、`policy_declined`）、`cca policy pull`（App 开着时一小时拉一次的那条）、锁定请求（`cca policy request`）；
  配置向导、服务器上安装时等维护者的那一支（`onboard_policy_quick`、`builtin_shared`）。辅助程序里读 policy.json 的那一半（`policy_doc`、
  `legacy_raw`、`legacy_fill`、`policy_problems`；`mapping` 只剩各个 Team 给的）和只给它用的子命令（`policy-repo`、`policy-set`、`policy-off`、
  `policy-rows`、`policy-check`、`fill-check`、`policy-request`、`policy-lock`……：记在 `cmd/mod.rs` 的 `REMOVED` 里）。
- 迁移用的：照映射里的邀请自己加入（`cmd/team.rs` 的 `adopt`、`teams/adopt.json`）、管理员给仓库上的 deploy key 发邀请（`team-handoff`、
  `./policy.sh team`）、导入映射文件（`cca team import Team policy.json [nodes.json]`：只剩 `--mine`）。
- 维护者那一头：`policy.sh`（发布、`key`、`lock`、`team`、`presence deploy / sync`）和它部署的 `server/compose.yaml`、`compose.web.yaml`、
  `compose.acme.yaml`。
- 维护者的服务器：`HubKind::Maintainer`（`HubKind` 只剩 Team 的）、`presence_server`、`is_maintainer`、服务端的 `--mode maintainer`
  （`/v1/machines`、`--release-key`）；远程会话的 `--maintainer`、`--hub maintainer`；只有它说得出的几句（`RELAY_OLD`、`NO_WEB`、
  `KEYSHARE_OLD`、`COST_OLD`）和折合金额的 `old_server`。
- 界面：「共享与同步」页的「维护者共享的账号」开关、侧栏的「等维护者」、拉取映射、请求锁定节点的菜单项。

留着的、改了的：

- **不升 `GUI_API`**（还是 1）：升了的话已经发布的 App 更新以前用不了；去掉 `config.policy` 的话它们连「共享与同步」页（连同 Team 的卡片）都不显示
  （它们照 `config.policy.state` 放这一页）。所以 `_gui state` 照旧给 `config.policy`，3.6.0 起是常量
  `{"configured": false, "repo": null, "next_check_epoch": null, "state": "off", "accounts": 0, "parked": 0}`，`can.policy_lock` 总是 false，
  `config.hubs[].kind`、账号的 `hub` 只会是 `team`（或者没有），折合金额不会是 `old_server`；PROTOCOL 里写成「3.6.0 起」的常量。新的 App 不读
  这几项，照 `hello.team` 放「共享与同步」页。
- `cca policy …` 只说一句（3.6.0 起去掉了：向 Team 的管理员要邀请链接、`cca team join`；自己的几台设备之间同步：`cca team new`），退出码 1；
  后台的 `cca _policy-fetch` 什么都不做（换上新版以前起的后台进程可能还会起它）；以前 `policy.sh` 调的 `_policy-check`、`_fill-check`、
  `_policy-lock`、`_team-handoff` 说去掉了。一键安装时带着 `CCA_POLICY_REPO` 只提醒一句（不再起作用，装好以后 `cca team join`）。
- **升级以后清掉留下的**（`policy_leftovers`，和 `hub_leftovers` 一个样子）：第一条走 `main` 默认那一支的命令（`_` 开头的、`install`、`gui`、`clients`、`cost`、
  `remote`、`team`、`update`、`restore` 这些不经过它；在 `update_after` 之后、`team_hook` 之前）看到 `~/.cca/policy/` 还在时，先 `py policy-retire` 去掉 config.json 的
  `policy`、`policy_was`、`policy_declined`（config.json 只读、读不了、写不进去时什么都不删，下一条命令再来；别的命令拿着 sync.lock 时等它放开），再删 `~/.cca/policy/`、经维护者的服务器
  同步的状态（没有 `team.<编号>.` 前缀的 `presence/status.json`、`presence/keys.json`、`presence/keys.lock`、`presence/keys-sync.lock`、
  `cost/sync.json`、`cost/remote.json`）和 `teams/adopt.json`、`teams/adopt.lock`；Team 的状态文件不碰。共享开着、又不在任何 Team 里的设备
  在终端里说一次怎么加入（`policy-retire` 的 stdout 不是空的）。映射加进来的账号留在账号表里，出口不再被固定；迁到了 Team 的设备照 Team 的配置用。
  `cca restore` 的备份照旧不带 `policy/nodes.json*`、`policy/tmp.*`：还原可能就是升级以后的第一条命令，那时它们还在。
- 以前关掉共享时收起来的账号（`parked` 里没有 `team` 的项）没有谁再放回来，留着不管；`cca restore` 照旧 `py unpark` 放回账号表、退出登录。
- 远程会话：`home_hub` 是第一个 Team 的服务器，不在 Team 里的设备上没有——配对（`cca remote pair`、`join`）说先加入或者新建一个 Team，这台设备
  自己的账号、第三方 API 账号的会话没有中转；配对表里没有 `hub` 的行（经维护者的服务器配的）读的时候丢掉（remote.md「每个 Team 各用各的中转服务器」）。
- 这台设备自己的账号（不是 Team 给的）记下的 API key、折合金额哪台服务器都不同步（以前退到维护者的服务器）。
- 服务端只剩 `--mode team`（托管平台的预设照旧是它）。Team 的服务器下次 `cca team deploy` 时换上新的程序；维护者那台照旧跑着原来的程序，另外停
  （见上面的阶段表）。
- 名字留着的：`core/policy.rs`、`policy_merge`、`policy_sources` 这些，事件的代码（`policy_added`、`policy_alias`、`policy_pinned`、
  `nodes_filled`），子命令 `policy-node`、`policy-have`、`policy-forget`、`unpark`、`nodes-import`：管的是 Team 给的账号（见「账号表照配置改」），
  话里不再说维护者。`cca team invite --key`（只认这把公钥）和 `expect` / `mismatch` 也留着：当初是给 deploy key 发邀请用的，本身是 Team 的功能。
- 路由器端的话（`router_sh` / `router_rb` 里的「维护者参考配置里的节点」「cca 写进去的维护者参考节点」这几句）故意没改：改了它们的字，同步摘要就变了，
  已安装的机器升级以后都要重新部署路由器端。

## 信任

- **Team 的根密钥**（ed25519）在管理员的设备上。服务器只配置根公钥（公开的），所以服务器上没有要保密的东西：被攻破了、放在托管平台上，
  都读不了、改不了配置。Team 的编号是根公钥的 SHA-256 的前 32 个十六进制字符；它只是个名字（目录名、认证里的一项），认一个 Team 比的是整把根公钥。
- **管理员 = 拿着根密钥的设备**：管理员的请求拿根密钥签名，服务器不另记谁是管理员。几台设备都当管理员就是都有这把密钥（M2 以后做）；
  所以没法单独撤掉一个管理员，只能换一个 Team。
- **每台设备一把配置的密钥**：邀请链接里那串随机数 `k` 派生出加入用的令牌、设备的编号和这台设备那份配置的密钥。只有管理员和这台设备知道 `k`，
  所以别的设备、服务器都读不了这一份。辅助程序没有非对称加密（依赖是白名单），这样不用加依赖。
- **配置防伪造、防调包**：密文带认证（Team、设备、序号都在里面），外面再有根密钥的签名；设备照邀请链接里钉住的根公钥验。两层各管一头：
  签名挡的是拿到了链接（有 `k`）的人伪造配置，认证挡的是挪到别的设备、别的序号上。
- **设备的公钥要过认证码才记**：设备加入时拿 `k` 给自己的公钥算一个认证码（`proof`），服务器原样存着、回给管理员；管理员照自己算的 `k` 核对，
  对得上才把这把公钥记成这台设备的，记下以后服务器再报别的公钥不照它的。服务器不知道 `k`，换不了公钥——不然它把公钥换成自己的，等管理员哪天
  搬到一台干净的服务器、照记下的公钥登记，那台干净的服务器上就多了一台它的设备。
- **管理员不把服务器的话当真**：名单、「存上了」「序号不够大」都没有签名，只当线索用。它说的序号要像话（不比记着的小、没到上限）才接着往上加；
  它那边某台设备的配置比发布过的旧就重新发布；回话里没提到的设备不当成不在了。
- **盐在配置里，不在邀请链接里**：账号摘要、API key 和折合金额的密钥都由它派生，整个 Team 一份。链接漏了、没用上的，带不出它。

做不到的，文档和界面里要照实说：

- 邀请链接就是那台设备的解密密钥，要当密码传；只能用一次，默认几天过期。**用之前**被别人拿到：别人冒充这台设备加入、取走给它存的配置
  （节点的凭证在里面），真的那台再用时服务器说用过了——管理员据此移除那个编号、换节点的凭证。这是「粘贴就加入、没有批准的来回」的代价
  （公钥的认证码挡不住它：拿到链接的人也有 `k`）。**用之后**被别人拿到：加入不了，但弄得到服务器上这台设备的密文的话解得开。
- 序号只增不减只挡得住「服务器拿旧的一份来换一台一直在用的设备手里的」。刚加入的、重装过的设备手里没有序号，服务器给哪一份它都只能认；
  服务器扣着新的不给，设备也看不出来，只能提示「多久没更新了」（明文里有管理员发布的时间）。管理员看名单时看得到各台设备手里是第几版（`have`）。
- 成员之间：盐是整个 Team 一份，所以知道邮箱的成员算得出别的账号的摘要（看得到它在哪些设备上在用、能往它的 API key 那一格里传东西——别的设备
  不认伪造的，只是白占一次同步）；API key、折合金额的内容要登录过那个账号才解得开，没给它这个账号的设备解不开。
- 服务器能让设备停在旧的配置上：扣着不给、对管理员谎称存上了、把设备从名单里藏起来（管理员会当它退出了，等它再出现才接着发布）。
  它也能让设备以为自己被移除了、或者以为服务器的记录没了（那个编号 `gen` 没有签名）：后果是说法不对，被说成移除了的设备拿新的邀请能直接换上。
- 移除一台设备收不回它已经看到的节点凭证，只能换节点的凭证。
- 根密钥丢了这个 Team 就得重建：建的时候提示导出备份。
- 管理员的设备不在线时，新的授权都等着；已经加入的设备照常用。
- 请求的签名里没有「发给哪台服务器」：同一个 Team 搬到新服务器时，旧服务器收到过的请求 5 分钟内能拿去新服务器重放一次。搬的时候等 5 分钟再用新的。

## 服务端（M1）

`server/cca-presence.py --mode team --team-root 根公钥`（`CCA_TEAM_ROOT`）。op、邀请链接、设备的配置的格式在 server/README.md「Team 的同步服务器」，
这里只写为什么这样：

- 管理员的 op（`team_enroll`、`team_invite`、`team_put`、`team_members`、`team_revoke`）拿根密钥签，`key` 是根公钥的指纹；设备的 op
  （`team_join`、`team_get`、`team_leave`，还有原来的上报、查看、API key、折合金额、中转）拿设备自己的密钥签。根密钥不是设备，别的 op 不认它。
- `team_enroll` 直接登记公钥：管理员自己的那台设备（不用给自己发邀请）；服务器的记录没了以后把各台设备登记回去（`team_members`
  回过公钥，管理员留着）；改设备的名字（同一把公钥再登记一次，`cca team rename`）。
- `team_join` 先看有没有这个邀请（便宜），有才验签；没有这个邀请和签名不对回一样的 403。签名对了才说过期了还是用过了：拿着真的邀请的人
  才问得出来，而「这个链接已经在另一台设备上用过了」是最常见的要说清楚的情况。
- 配置按设备的编号存、不按密钥：管理员在设备加入以前就能存好，设备一加入马上就能同步下来。存在状态文件旁边的 `team-docs.json`（心跳不用跟着重写它）。
- `team_put` 的序号一样、认证码也一样的算存过了（管理员没收到回复再发一次）；一样的序号、不一样的内容不存。
- 没有入伙口令、盐的校验值、换口令那一套（以前的自己的同步服务器有）：邀请是一台设备一个，盐在设备的配置里。服务器的记录没了（`gen` 变了）以后，
  设备自己加不回来（邀请只能用一次），由管理员照留着的公钥登记回去、再存一次配置（M2；同一个序号再存一次服务器照收，设备手里就是这一份）。
- 根密钥不是设备：拿它登记、加入都不收（不然「别的 op 不认它」就不成立了）。
- 配置跟着编号走：移除设备、撤回邀请时删掉；邀请过期 30 天被清掉时，预先给它存的也删掉（`docs_prune`，启动时也清一遍：移除时先写状态文件、
  再写 `team-docs.json`，中间停了会留下没人认的）。过期、用掉的邀请在发邀请、存配置、看名单时顺带清（`invite_prune`）。
- `team_get` 记下设备说的 `have`（只在内存里），`team_members` 回给管理员：管理页据此说哪台设备还没用上新的配置。
- 状态文件手改坏了的：样子不对的邀请、设备在启动时去掉（`team_setup`），不让以后每个请求都出错。
- 管理员的请求（根密钥签的）另有一个限额：每秒 2 个、攒得下 120 个（设备的是每 2 秒一个、攒得下 30 个：管理员的一条命令要问两三次，
  连着发几个邀请、改几次授权就不够用了）。`team_enroll`、`team_put` 照样一次带一批（最多 64 台），不要一台一个请求。
- 3.6.0 起服务端只有这一种模式（`--mode team`；维护者的 `--mode maintainer` 去掉了，`--mode own` 3.5.3 起就没有了）。状态文件是别的模式留下的
  （3.6.0 以前维护者的服务器的、3.5.3 以前自己的同步服务器的）或者根公钥换了，就从头开始（`Store.team_setup`：设备、记录、密文都不要了）。
- `team_put` 的请求上限放宽到 1 MB，别的 op 照旧 64 KB。

安装脚本、托管平台的预设（`CCA_HUB_HOSTED=1`）见「部署」。

## 设备拿到的配置

明文就是给这台设备的账号 → 节点加上节点的定义，样子照以前维护者映射的 `policy.json`、`nodes.json`（对账、补节点的规则沿用了下来），再加上盐和服务器在哪：

```
{"v": 1, "at": 管理员发布的时间, "name": Team 的名字, "salt": 盐,
 "server": {"u": 地址, "p": [证书公钥的指纹…]},
 "accounts": {"claude": {邮箱: 节点}, "codex": {…}, "agy": {…}},
 "aliases": {邮箱: 别名（3.5.3-beta.5 起每个账号都有：没起过的是 team-N）},
 "tool_aliases": {"claude": {邮箱: 别名}, "codex": {…}, "agy": {…}}（3.5.3-beta.6 起）,
 "alias_lock": true（3.5.3-beta.7 起，管理员锁定了别名；没锁定时没有这一项）, "install_node": 节点,
 "nodes": {"group": 组名, "proxies": [节点定义…]} | null}
```

只有给了这台设备的账号、这些账号要的节点（连同它们经 `dialer-proxy` 连着的、安装客户端用的那个）。Team 的编号、设备的编号、序号不在明文里：
它们在认证和签名里。管理员发布前、设备用之前都照 `doc_problems`（`cli/src/cmd/team/doc.rs`）查一遍：邮箱小写、节点的定义照 `fill_problems`，
不认得的项不管（以后加的旧版本照样用）。`name`、`server` 变了设备跟着改 team.json：管理员改名字（`cca team name`）、搬服务器、换证书都不用
重新发邀请。别名按工具记在 `tool_aliases` 里（3.5.3-beta.6 起：各工具各编各的 `team-N`，同一个邮箱在 Claude、Codex 里是两个账号、可以叫得
不一样），设备有它时照它；`aliases` 是照它算的按邮箱的那一份（每个邮箱用它最前面那个工具里的），给 3.5.3-beta.5 及以前的 cca。
`alias_lock` 是 true 或者 false（`doc_problems` 查；管理员只在锁定时写 true），旧版本的 cca 不认得它：照没锁定的用。

## 账号表照配置改（M3 的第一步）

对账、补节点、固定节点都经 `core/policy.rs` 的那几个读取函数（名字是维护者的映射那时起的：映射原来是它们唯一的来源，3.6.0 起去掉了）。它们给的是
各个 Team 给这台设备的配置**合起来的**（照加入的先后，同一个账号先出现的算数）；对账（`reconcile`）、`policy_merge`、`fill_plan`、路由器端的
`--fill-source` 沿用映射那时的规则：

- `core/team.rs` 的 `grants()`：各个 Team 手里验过的那一份配置（一个进程只读一次）。还没同步下来的不算；被管理员移除了的（`sync.json` 是 `removed`）
  也不算——手里那一份不再算数。`lost`、`error` 的照旧用手里的。
- `core/policy.rs`：`mapping(tool)` 是合起来的 [(邮箱, 节点, 哪个 Team 定的)]，`policy_raw` / `policy_map`（节点；配置里写的是路由器上改名以前的旧名字时
  照记下的改名换算：`node_alias`，见 router.md）、`policy_sources`（谁定的：Team 的编号）、`policy_fill`（节点的定义：同名的先出现的算数，策略组照
  第一份有节点的）、`policy_install_node`（安装客户端用的节点：第一个写了的）都从它来。
- 对账时说的话照是谁定的说（`by_whom`：「按「Team 的名字」的配置…」）。事件的代码是映射那时的（`policy_added`、`policy_alias`、`policy_pinned`、
  `nodes_filled`），没改。
- **合并进账号表**（每条命令读账号表都经过 `reconcile`：添加、`cca edit`、启动、同步前都管得到）：配置里有、这台设备上还没有的官方账号加进来
  （`policy_merge`，事件 `policy_added`；别名见下面）。不加的，`policy_missing` 给原因：这台设备上删掉过的（工具状态里的 `policy_skip`：`cmd_rm`、
  `cca edit` 记下——`cca edit` 经 `py policy-have` / `policy-forget`，要在 `py validate` 之前记，不然 ctx 又加回来了——`cmd_add` 清掉）、节点不在可选
  节点里的（加进来整张表就校验不过）、这台设备还没用过的工具（`policy_in_use`：当前的工具或者已经有账号表；Codex 账号第一次 `cca codex` 时才加，
  免得白占端口）。已经在表里的，节点和配置的不一样就改回配置的、写回账号表（事件 `policy_pinned`：「! … 的出口按「Team 的名字」的配置固定为 …
  （账号表里写的是 …，已改回）」）；`cca add` 加一个配置里有的邮箱时不问节点（`py policy-node`）。这台设备自己的账号（配置里没有的）不动。
  配置要的节点还不在可选节点里、定义在 Team 的配置里的，先加进可选节点（`policy_nodes_fill`，事件 `nodes_filled`，见 fill.md）。
- **别名**（3.5.3-beta.5 起）：`alias_sources(工具)` 是合起来的这个工具的 [(邮箱, 别名, 哪个 Team 给的、这台设备是不是它的管理员)]（照加入的先后，
  同一个邮箱先出现的算数；Team 的配置里有 `tool_aliases` 时照它里面这个工具的，没有时照 `aliases`）。管理员那边每个账号都有别名（见下面「管理员的设备」的
  `default_aliases`：没起过的各工具各自编成 `team-1`、`team-2`……）。
  成员设备上：加进来的带着它（`policy_merge`），只有邮箱的补上（`policy_alias_fill`）；和这台设备上别的账号撞了时，Team 给的换成「Team 的名字-N」
  （`alias_fallback`：`team-N` 取 N，别的别名接在 Team 的名字后面；Team 的名字里别名不能有的字符去掉，太长了截短到 24 格宽），还撞就只写邮箱 / 先不补。
  管理员后来改了：`team_alias_follow` 让这台设备上照 Team 给的用着的跟着改，自己改过的不动。哪个别名是 Team 给的记在工具状态的 `team_alias`
  （{账号目录名: {"a": 账号表里用着的别名, "t": 那时 Team 给的}}）：账号表里的就是 Team 给的（或者它撞了以后换成的那个）时记下；以后账号表里的还是
  `a`、Team 给的不再是 `t` 才换（撞了照样换成「Team 的名字-N」，还撞就下次再看）；账号表里的不是 `a` 了是这台设备上改过（`cca rename`），记着的
  去掉、不再跟。没有记录、账号表里是 `team-N`、Team 给的是另一个 `team-N` 的也跟着换：3.5.3-beta.5 各工具一起编号（Codex、Antigravity 的号
  接在 Claude 的后面），3.5.3-beta.6 照各工具重新编了，旧版本补上的别名没有记录。几个账号一起换时先当它们都换、再一轮轮去掉换不了的
  （账号表最后一起改，要换走的账号原来的别名不算占着）：挨个挪一位、两个对调都一次换好。账号不在账号表里、也没收起来的，记着的那一项去掉。
  事件还是 `policy_alias`（说「按「Team 的名字」的配置把 … 的别名改成 …」）。**这台设备是管理员的那个 Team 给的别名不补、不跟**（`Grant.admin`）：
  管理员这台设备上的别名不动。
- **Team 加进来的账号记着是它加的**（工具状态里的 `team_added`：{账号目录名: Team 的编号}，`policy_merge` 照 Team 的配置加的才记）。配置里没有它了
  ——管理员不再给这台设备、退出了那个 Team、被移除了——就收起来（`team_park`：记进 `parked`、从账号表去掉，登录目录和钥匙串不动；事件
  `team_parked`），再给回来时照原来的键加回去，不用重新登录。正开着命令行会话的先不收（`team_busy`），下一条命令再看。那个 Team 的目录还在、
  配置却读不出来时说不准还给不给（`grant_settled`）：不收。这台机器**本来就有**的账号 Team 只管节点、不记成它加的：不再给了账号还在，只是节点
  不再被固定。
- **收起来的账号**（工具状态里的 `parked`：{账号目录名: {"key": 原来账号表里的键, "node": 节点, "team": 哪个 Team 加的}}）：不在账号表里，登录目录、
  钥匙串原样留着；再给回来时 `policy_merge` 照原来的键加回来（`parked_key`；别名和这台设备上别的账号撞了就只写邮箱），同一个账号目录，不用重新登录；
  `reconcile` 看到它又在账号表里（给回来了，或者用户自己加回来了）就从 `parked` 里去掉。记下备查的 API key（`cca secrets forget` 清多余的 key 时）、
  邮箱助手的设置（`mail_forget_orphans`）都按收起来的账号留着；`cca restore` 退出登录以前先 `py unpark` 放回账号表，好让它们也退出登录。
  3.6.0 以前关掉维护者的共享时收起来的（没有 `team`）没有谁再放回来，`unpark` 照样放。
- 账号走哪一类同步服务器（`cmd/hub.rs` 的 `route`）看是谁定的：Team 给的是 `HubKind::Team(编号)`，别的（这台设备自己的官方账号、第三方 API 账号）
  哪台都不去（见下一节）。
- 配置什么时候同步：`cca team join`、`cca team sync`；启动时 `team_hook`（在 Team 里、距上次超过一个小时就在后台 `cca _team-sync`，只换本机存的那一份）。
  换了版本以后的第一条命令不等这一个小时（`update_after` 删掉 `teams/checked`，见 release.md）：一更新就和 Team 对一次，拿到最新的那一版，管理员看到的「手里那一份」也跟着新。
  账号表在下一条读它的命令里照新的改；`cca team join / sync / leave`、管理员的 `import / map / alias / publish / attach` 之后马上做完（`team_settle`：
  没配置过路由器的把配置带的节点导入可选节点，`py team-apply` 对一遍，同步路由器，改启动器）。
- 管理员自己这台设备默认拿全部账号（`grants` 是 `all`）：Team 里的账号都会加进它的账号表。不要的话 `cca team grant Team 自己这台 none`
  （或者只留几个）。

## 同步经 Team 的服务器（M3 的第二步）

设备统计、记下备查的 API key、折合金额三条管线是「这台设备用得了的每台服务器一条线」（`cmd/hub.rs` 的 `hubs()`）：各个 Team 的服务器
（`HubKind::Team(编号)`，照加入的先后；config.json 写着 `"presence": false` 时一台都没有）。3.6.0 以前排在最前面的维护者的那一台去掉了，三条管线没改：

- 一个 Team 的服务器在这台设备上用得了（`cmd/team.rs` 的 `hub_cfgs`）：接上了服务器、这台设备在 Team 里的密钥在、手里有验过的配置（盐在配置里）、
  没被管理员移除，而且没关掉设备统计（`"presence": false`）。请求拿这台设备在这个 Team 里的密钥签名，盐是这个 Team 的那一份：别的 Team、
  别的服务器上的账号摘要和密文都对不上。
- 账号走哪台：`route` 照 `policy_sources`——某个 Team 给的走那个 Team 的；别的（这台设备自己的官方账号 `own_account`、第三方 API 账号、
  没有邮箱的）哪台都不去：设备统计、记下的 API key、折合金额只在这台设备上（3.6.0 以前退到维护者的服务器，现在不退），`cca devices` 说要和
  自己的别的设备一起统计就建一个 Team、把它加进去（`unrouted_reason`）。
- 状态文件各是各的：`presence/` 和 `cost/` 里 `team.<编号>.` 开头（`HubKind::file`）。退出、被管理员移除（同步配置时知道的）以后把它们删掉
  （`drop_synced`）：不再读它们，而 `cost/` 里那一份是解开了的别的设备的折合金额，不该留在这台设备上。
  删的时候别的进程正好在同步（拿着 `team.<编号>.keys.lock` 之类的锁）时，它收尾写回的几个文件会留下来：服务器已经用不了了，没有害处，
  也不值得为它多做一遍清理。被说谎的服务器骗成「移除了」时丢的只有这些同步的状态和缓存，重新邀请、再同步一次就有了（key 本身在钥匙串里）。
  `cca restore` 的备份也不带 `cost/team.*.remote.json`（卸载时各个 Team 先退出、已经删了；排除清单是第二道：那个 Team 的目录不在了、
  退出时删不到的也不进备份。回归里放一份假的核对）。
- 上报进程每个 Team 一路（配置里的 `rev` 是 team.json 的修改时间：重新加入、配置里换了服务器以后算新的一路）。它一直开着，而各个 Team 的配置
  在一个进程里只读一次（`grants()` 记着）：所以每一圈先 `forget_grants()` 重读，并且盯着 `teams/` 和里面每个 Team 的目录——加入了、退出了、
  同步到了新的配置、被移除了，马上醒来换各路（不只盯已经有一路的：刚加入、配置还没同步下来的也盯着）。`teams/` 是上报进程开着的时候才第一次
  建出来的（这台设备第一次加入）时要到下一圈才开始盯，最多晚一次心跳。
- **没有账号经它时一次都不报**（`Lane::quiet`）：加入了一个 Team、它给的账号却没在用（没登录，或者这个账号是另一个 Team 先给的）时，不必每分钟
  告诉它的服务器这台设备开着。这一路从「报过什么都没开」起步；有会话了照常报、心跳，都关了报一次 `end: none`，然后又不出声。算下一次什么时候
  醒时不算这几路（它们的 `due` 没人动，算进去就成了每秒醒一次）。
- 服务器不认这台设备时（被移除了、服务器的记录没了）不自己重新加入——等管理员；那一路隔 10 分钟再试，
  下一次 `cca team sync`（启动时每小时一次）以后知道是被移除了，这一路就停了。
- 没成的原因前面带着是哪个 Team（`hub_why`：「「名字」的同步服务器：…」）；`cca devices` 里账号不经服务器时的原因照 `hub_missing_why`。
  「服务器太旧」那两句（`COST_OLD`、`KEYSHARE_OLD`，让维护者更新服务器）只对维护者的服务器说过，3.6.0 跟着它去掉了。
- 一个 Team 的密钥文件读不了只是它用不了（`hub_cfgs` 跳过它），别的服务器照常。没有 Team（或者都还没同步到配置）的机器上 `hubs()` 只多两次
  落空的系统调用（`teams/index.json`、`teams/`），不多读 config.json。
- 给界面和脚本的输出里是哪个 Team 写在旁边（可能同时在几个 Team 里，`kind` / `hub` 都是 `team`）：`_gui state` 的 `config.hubs` 里 Team 的项带
  `team`（编号）、`team_name`；账号多一项 `team`——是哪个 Team 给的，不看它的服务器用不用得了，不是 Team 给的（第三方 API 账号也是）没有这一项（没有 Team 的机器
  输出和原来逐字节一样）；`devices --json` 的账号和 `hubs[]`、`cost sync --json` 的 `sync.hubs[]` 里 Team 的也带 `team`。3.6.0 起 `kind` / `hub`
  只会是 `team`（App 那一头见「界面」）。

## 迁移（M4）

### 维护者的映射 → Team（3.6.0 删掉了）

3.5.3 到 3.5.5 这几版里，维护者把映射里的账号 → 节点、别名、安装客户端用的节点、参考节点导进一个 Team（导入映射文件），再给映射仓库上每台机器的
deploy key 各发一个邀请、写进映射（`./policy.sh team`）；机器拉到映射以后拿给自己的那个邀请自己加入，设备密钥就是原来的 deploy key
（`teams/<编号>/id_ed25519`）。这一套连同映射 3.6.0 都删掉了（见「3.6.0 去掉的旧通道」）：已经加入的设备照常用，那把密钥照旧是它在这个 Team 里的
设备密钥（仓库上的 deploy key 删掉也不碍事）；没迁过去的机器升级以后（共享开着、不在任何 Team 里）在终端里说一次怎么加入——向 Team 的管理员
要一个邀请链接，`cca team join`（新生成设备密钥）。留下来的只有邀请指定公钥（`cca team invite --key`，见「管理员这一头」）。

### 自己的同步服务器 → Team

3.5.3 去掉了自己的同步服务器（`cca hub`、服务端的 `--mode own`）：自己的几台设备之间同步改成建一个 Team。设备少、都是自己的，照步骤手动换，
不自动：

1. 一台设备上 `cca team new 名字`，部署服务器（`cca team deploy`，见「部署」）、接上。新建时就选上自己的账号（3.5.5 起，见「管理员这一头」），
   就不用下一步。
2. `cca team import Team --mine`：把这台设备上自己的官方账号（不是别的 Team 给的、不是第三方 API 的）连同现在走的节点、
   别名加进 Team 的配置。节点的定义不带（各台设备自己有：同一个路由器，或者各自的内置代理）。
3. 给别的设备各发一个邀请（`cca team invite`），在那台上 `cca team join`。

换过去以后这些账号的节点由 Team 的配置固定（管理员的设备上 `cca team map` 改），不再是各台设备自己选。折合金额、记下的 API key 各台设备
自己有一份，经 Team 的服务器重新传上去，不丢。不建 Team 的话，自己的官方账号只在这台设备上统计（`cca devices` 说怎么办，见「同步经 Team 的服务器」）。

以前留下的：

- 设备上的 `~/.cca/hub/`（加入信息、这台设备在它上面的密钥）、`hub-hosted.json`（部署到托管平台时还没用上的口令和盐）、`hub-removed.json`、
  `presence/own.*`、`cost/own.*`：升级以后第一条走 `main` 默认那一支的命令（启动账号、`cca add` 这些；`_gui`、`team`、`remote`、`cost`、`update`、
  `restore` 不经过它）看到 `~/.cca/hub/` 或者 `hub-hosted.json` 还在就删掉（bash 的 `hub_leftovers`）；服务器上这台设备的记录不管。没清掉以前它们
  还在，所以 `CCA_DIR_PRIVATE` 照旧挡着 `hub/hub.json`、`hub/id_ed25519`、`hub-hosted.json`，`cca restore` 的备份照旧不带 `hub/hub.json`、
  `hub-hosted.json`、`cost/own.remote.json`（`id_ed25519` 本来就不带）。
- 服务器上以前装的 `/opt/cca-hub`（服务名 `cca-hub`）：安装脚本只让卸载（`cca team deploy --script | ssh root@服务器 sh -s -- --uninstall --dir /opt/cca-hub`）。
  不带 `--uninstall` 指着它、带着 `--team-root` 往那个目录里装都停下（`team_here`、`other_kind`；更早的安装脚本没在 `.cca-hub` 里记服务名，
  `.env` 里有 `CCA_HUB_ADMIT_HASH` 的也认成它）。托管平台上的那一台在平台上删掉。
- 服务端不再认 `--mode own`；拿它的状态文件起 Team 的服务器是从头开始（见「服务端」）。

## 部署（M6）

服务器上只有 Team 的根公钥（公开的），所以装哪都行；麻烦只在「装」本身。三条路：

- **`cca team deploy Team [用户@]主机 […]`**（`cmd/team/admin.rs` 的 `cmd_team_deploy`）：经 SSH 装到自己的 Linux 服务器上，装好了接上
  （`attach`，和 `cca team attach` 是同一段）。装的那一半在 `cmd/hub/deploy.rs`（`opts`、`run_installer`、`ssh_check`、`team_install`，见下面
  「经 SSH 装」）。各台设备该连的地址：`--host`；没写时，装好的那台用的证书就是现在接着的那张（指纹对得上：还是那台服务器，只是更新）就照
  原来的地址，不然照安装脚本定下的（结果行的 `host`：服务器上记着的、`ssh -G` 的 hostname、它看得到的公网 IP），它也没有时说认不出地址
  （`host`：加 `--host`）。地址只由安装脚本定一次，网页端的证书和各台设备连的才是同一个。再运行是更新；写别的服务器就是搬过去（`attach` 记 `moved_from`，原来那台上收尾）。没有 `--reset`：
  设备、邀请都在管理员手里，服务器没有要换的口令。**装的那一段不拿着这个 Team 的锁**（经 SSH 要几分钟，拿着的话这台设备上同步配置、别的管理
  命令都卡着等）：先读出根公钥、现在接着哪台就放锁，装好了再取锁、重新读一遍，接上。装好了接不上（`unreachable`：多半是云服务商的安全组没放行）
  时说放行哪个端口、放行以后再运行一次（服务器上是更新）。
- **在服务器上自己运行安装脚本**（`cca team deploy --script`、发布页上的 `cca-hub-install.sh`）：
  `sudo sh 脚本 --team-root 'ssh-ed25519 AAAA…' [--host …] [--port …] [--relay-port … | --no-relay] [--web-port … | --no-web] [--docker | --systemd] [--dir …]`，
  最后打出管理员接上它的那一行（`cca team attach Team https://地址:端口 sha256//指纹`，地址和指纹都是公开的）。`--relay-port`、`--no-relay`
  只有这样运行时写得了：`cca team deploy` 不传它们。
- **托管平台**（`cca team hosted Team [railway | sealos] [--dir 目录]`，只在终端里）：见下面「托管平台」。

**一台服务器上几个 Team**（用户定的：每个 Team 一个服务，服务端进程还是只管一个）：安装脚本（`server/hub-install.sh` 的 `team_setup`）里
服务名、系统用户、容器名都是 `cca-team-<Team 编号的前 8 位>`（编号是根公钥的 SHA-256，脚本里拿 openssl 算，和辅助程序的 `team_id` 一样），
目录默认 `/opt/<服务名>`，端口没写时照上次的、第一次从 18445 起找一个空着的。所以各个 Team 在同一台机器上互不相干：各有各的 unit / compose
项目、证书、数据、`.env`。

- `.env` 里是 `CCA_TEAM_ROOT=`根公钥 `ssh-ed25519` 后面那一段（没有空格：systemd 的 EnvironmentFile、compose 的 `.env`、脚本自己的
  `env_get` 都好读）；服务端两种写法都认。
- 再运行：带 `--team-root`，或者只写 `--dir 那个目录`（`team_here` 照目录里的 `.env` 认出是哪个 Team：更新、卸载）。目录里装着另一个
  Team 的、以前的自己的同步服务器的不碰（`other_kind` → `taken`）——**卸载也查**：写错了 `--team-root`、`--dir` 的话，停的是这个 Team 的
  服务，删的却是那个目录里别人的数据。`.cca-hub` 里记着服务名（`# svc=…`）：`.env` 不见了、坏了时卸载照它停服务、删系统用户；
  不带根公钥再装不行（说带上 `--team-root` 再运行）。
- 一个 Team 在一台服务器上只装一个：systemd 的 unit 里记着别的目录时停下（`other_dir`：照旧用那个目录，或者先卸载它）。
- Docker 方式下 compose 的项目名照服务名（`-p`，`DCP`）：不写的话是目录名，两个 Team 装在同名的目录里（`/a/hub`、`/b/hub`）时
  `up --remove-orphans` 会把对方的容器当成孤儿删掉。
- 自动挑端口（`port_taken`）除了看有没有程序在听，还跳过这台服务器上别的同步服务器（默认目录 `/opt/cca-team-*`、`/opt/cca-hub` 的 `.env`）
  记着用的：没有 `ss`、`netstat` 的服务器上只看得到这个。
- 换端口可以：管理员重新接上（`cca team attach 新地址`），各台设备照配置里的新地址换过来。

### 安装脚本

- 模板 `server/hub-install.sh`（POSIX sh：dash、busybox ash、bash 都要能跑；`$变量` 后面紧跟中文要写 `${变量}`，服务器上的 sh 可能是 bash，
  回归的 lint 也查它）里标着 `# @CCA-HUB-EMBED@` 的那一行，辅助程序的 `installer()`（`cmd/hub/deploy.rs`）换成版本（`CCA_HUB_VERSION`）、
  `team_install` 给的那几项（直接在服务器上运行的那一份是空的）、服务端程序（函数 `hub_server_py` 里的 here-document，结束行是
  `CCA_HUB_SERVER_PY_EOF`）、证书助手（`hub_acme_py`，`CCA_HUB_ACME_PY_EOF`）和网页端的文件（`hub_web_files`：`server/web/` 里的每个文件
  （`WEB`）一段 base64，一行 64 个字符、`CCA_HUB_WEB_EOF` 结束，安装脚本的 `web_file` 拿 `openssl base64 -d` 解开、核对 SHA-256）。程序里
  不能有这几行结束行（单元测试查）。模板、服务端程序、证书助手、网页端的文件都是 `include_str!` / `include_bytes!` 编进辅助程序的：改了要
  重新编；`server/web/` 里新加、删掉文件时 `WEB` 跟着改（单元测试 `web_files_are_all_there` 查）。`cca team deploy --script`（`py hub-installer`）
  打出的、release.sh 放到发布页上的 `cca-hub-install.sh` 是同一份。模板本身不能运行（没有 `hub_server_py`：停下，说用生成的那一份）。
- `--dir`：`/` 下面的一个目录，只认字母、数字和 `._/-`，没有空的、`.`、`..` 这样的一节，末尾的 `/` 去掉，不能在 `/usr`、`/etc`、`/home`、
  `/root`、`/run`、`/tmp` 这些下面（`DIR_HIDDEN`：systemd 方式的 unit 带着 `ProtectSystem=full`、`ProtectHome`、`PrivateTmp`、`PrivateDevices`，
  服务看不到、写不了那里，`/etc` 下面的还能起来、要写状态时才出错；`/run` 重启就清掉）；安装脚本的 `dir_check` 和辅助程序的 `dir_of` 一样查。
  要是还没有、空的，或者以前装过的（有 `.cca-hub` 或者 `.env`，`dir_ours`）：别人的目录不碰，里面的 `data`、`certs` 会被 chown，卸载时会删。
- 装好的样子：`.cca-hub`（记着这个目录是安装脚本装的、服务名）；`app/`（755：`cca-presence.py` 644，`name` 644——Team 的名字，下面
  「Team 的名字」，网页端的文件在 `app/web/`，先都写进 `app/web.new`、核对过再换上）；`certs/`（700：`key.pem` 600、`cert.pem` 644，自签
  P-256、10 年，没有才生成：macOS 的 curl 只给 RSA 和 P-256/P-384 做公钥钉住；私钥不出这台服务器）；`data/`（700，服务端的状态文件）；`.env`
  （600：`CCA_HUB_MODE`、`CCA_HUB_PORT`、`CCA_TEAM_ROOT`、`CCA_HUB_RELAY`、`CCA_HUB_WEB`，有的话 `CCA_HUB_HOST`、`CCA_HUB_WEB_URL`、Docker 的
  `CCA_HUB_IMAGE`、`CCA_HUB_VERSION`）。选的都记在 `.env` 里，再运行照旧。文件都先写到旁边再 `mv`（`put`：正在运行的服务读不到半截）。
  网页端的证书不在这里：这台服务器上几个 Team 共用的证书助手放给它（下面「共用的证书助手」）。
- Docker 的另有 `compose.yaml`（`docker compose -p <服务名> -f compose.yaml`：目录里别的 compose 文件、override 不认；容器 `<服务名>`，用户
  10001，只读的根、tmpfs 的 /tmp，去掉所有 capability、`no-new-privileges`，内存 256M、进程数 64（网页端开着时 512：每条 WebSocket 一个线程），
  日志 3×1M；`certs/`、`data/` chown 给 10001）。容器里 HTTPS 听 18445、中转的数据端口 18446、网页端 18447，对外是 `.env` 里的
  端口。它是一份模板：`@SVC@` 换成服务名、`@AD@` 换成共用的证书助手的目录、`@PIDS@` 换成进程数，`@RELAY@…@@`、`@WEB@…@@`（里面套着
  `@NAME@…@@`：记着 Team 的名字时给 `--web-name-file /app/name`）那几处开着时留下、没开时整个去掉（不占端口）。网页端开着时只读挂着共用的
  证书助手放给它的那一份（`/opt/cca-team-acme/webcert/<服务名>:/webcert:ro,z`）。
- systemd 的是 `/etc/systemd/system/<服务名>.service`（系统用户 `<服务名>`、`EnvironmentFile` 是 `.env`、`NoNewPrivileges`、`PrivateTmp`、
  `PrivateDevices`、`ProtectSystem=full`、`ProtectHome`、没有 capability、`Restart=on-failure`、内存 256M、`TasksMax` 照上面的进程数），网页端开着时
  `--web-cert`、`--web-key` 指着共用的证书助手放给它的那一份。服务端用 `python3 -I` 运行（不看 `PYTHON*` 环境变量），两种方式下都
  监听 0.0.0.0；起来以后等它在 127.0.0.1 上用这张证书回话（`wait_served`：`openssl s_client`，最多 30 秒），起不来时打出最近的日志。
- 运行方式：`--docker`、`--systemd`；不写时上次是 systemd、还能用就照旧，再是 Docker（有 compose 插件、`docker info` 通；只有老的
  `docker-compose` 1.x 的不算：它认不得 `compose.yaml`，重建容器时还会出错），再是 systemd（`/run/systemd/system` 在、有 systemctl、python3 3.6 起、
  带 ssl）。镜像依次试上次用的、ECR 上的官方镜像、腾讯云和 DaoCloud 的同步源、Docker Hub（`IMAGES`；`docker pull` 每个最多 5 分钟；国内常常
  连不上 Docker Hub），都拉不到时、`docker compose up` 没成时（打出它说的最后 20 行）没指定 `--docker` 的改用 systemd。服务端 3.6 起能跑（3.7 才有的
  `ThreadingHTTPServer` 换成 `ThreadingMixIn`，没有 `ssl.TLSVersion` 时用 `OP_NO_TLSv1*`）：CentOS 7、一些云厂商镜像里的 python3 是 3.6。两种方式之间
  换的时候停掉原来的那个，数据、证书照旧（共用的证书助手照它自己的运行方式：还能用就不换）。
- 中转（远程会话的，见 remote.md「每个 Team 各用各的中转服务器」）：数据端口没写时照上次的、第一次从 HTTPS 那个端口的下一个起找空着的
  （`pick_relay`），被别的程序占着时停下。
- 网页端（手机的浏览器里接入，见 remote.md「网页端」）：端口照样从数据端口的下一个起找（`pick_web`）。这一次开不开再看几样：中转开着、
  认得出地址（`pick_host`：`--host`，没写时照上次记下的 `.env` 的 `CCA_HUB_HOST`，再是 `cca team deploy` 放进的 ssh 的主机名 `CCA_HUB_HOST_HINT`、机器上看得到的公网 IP）、不是内网地址、80 端口给得了证书助手（被别的程序占着、又不是共用的证书助手——或者这个
  Team 以前它自己的——时不行）。开不了时照样装好、说为什么（结果行 `web_off`：`web_port` 端口被占、`host` 认不出地址、`lan` 内网地址、`port80`、
  `acme_old`：占着 80 的是另一个 Team 用以前的安装脚本装的、它自己的证书助手，说是哪个、先用这一版更新那个 Team），选的端口记着、
  再运行时再看；写了 `--web-port` 的，它的端口或者 80 被别的程序占着时停下。
- 网页端的证书：共用的证书助手（`server/cca-acme.py`）向 Let's Encrypt 要（IP 地址的是短期证书），放进 `/opt/cca-team-acme/webcert/<服务名>/`，
  到期前自己续。服务端不等证书就
  起来（`CertCtx`：头一张来之前网页端那个端口一握手就断，隔一秒看一次文件；有了以后换了自己重新读）。安装脚本起好服务以后等证书（`wait_cert`：
  最多 `CCA_HUB_CERT_WAIT` 秒，默认 120；第一次要时证书机构来验证，多半十几秒），拿到了再等服务端在网页端的端口上用上（最多 10 秒）：结果行
  `web_cert ok`，没等到是 `wait`（多半是 80 端口没放行：证书助手过一会儿自己再试）。证书助手起不来时说一声、不等。
- 共用的证书助手（80 端口只有一个：一台服务器上几个 Team 都开网页端时只能有一个程序听它）：服务名 `cca-team-acme`（systemd 的服务名和
  系统用户、Docker 的容器名和 compose 的项目名；不叫 `cca-acme`：3.6.0 以前维护者的设备统计服务器的证书助手容器叫这个，那台服务器上可能还在），装在固定的
  `/opt/cca-team-acme`（每个 Team 的安装脚本都要找得到它，不跟 `--dir`）。里面：`.env`（600：`CCA_HUB_MODE`、Docker 的 `CCA_HUB_IMAGE`、
  `CCA_HUB_VERSION`）、`app/cca-acme.py`、`teams/<服务名>`（root 的、644：这个 Team 登记的地址，就是它的 `CCA_HUB_HOST`）、`store/<种类>-<值>/`
  （700：一个地址一张，`privkey.pem` 600、`fullchain.pem` 644；几个 Team 同一个地址时只要一张——同样的证书 7 天最多发 5 张）、`webcert/`（711）
  下面每个 Team 一份 `webcert/<服务名>/`（证书助手的用户、这个 Team 的组，2750：放进去的文件跟着它的组，私钥 640——只有这个 Team 读得了自己的
  私钥，不用另建一个共用的组；Docker 的 Team 的组是 10001）、`acme/`（700，证书机构那里的账户的私钥）。证书助手照 `teams/` 里登记的去要
  （`cca-acme.py --teams --store --out`，见 remote.md「证书从哪来」），过几秒看一次登记的变了没有。
- 第一个开网页端的 Team 装上它（运行方式照这个 Team 的；装着的照旧，还能用的话：`acme_mode`），后面的 Team 只是登记；网页端开着的每次装都换上
  这一版的程序、重新起它（`acme_start`，在这个 Team 的服务起来以后）。这个 Team 不再用网页端（`--no-web`、这一次开不了）时去掉它的登记和那一份
  （`acme_leave`）；没有 Team 用了就停掉它、删掉 unit / 容器（证书、账户留着，再开时接着用，不再要一张）；卸载最后一个用它的 Team 时连同目录、
  系统用户一起删掉（说一声）。Docker 的：compose 项目 `cca-team-acme`，用户 10001，`80:18080`，挂 `./teams`（只读）、`./store`、`./webcert`、
  `./acme`，内存 128M、进程数 32。systemd 的：`cca-team-acme.service`（系统用户 `cca-team-acme`，直接听 80：只给 `CAP_NET_BIND_SERVICE`）。
  证书助手和 Team 的运行方式可以不一样（目录的组照 Team 的）。
- 从 3.5.3-beta.4 以前的装法迁过来（每个 Team 自己一个证书助手 `<服务名>-acme`，占着 80 端口，证书在 Team 目录的 `webcert/`、账户在 `acme/`）：
  更新这个 Team 时停掉、删掉它自己的（Docker 的是同一个 compose 项目里的孤儿：`--remove-orphans`；systemd 的 `drop_unit`），服务起来以后
  `webcert/` 里的证书搬进 `webcert/<服务名>/`、`acme/account.key`（共用的还没有时）搬进 `acme/`，Team 目录里的 `webcert/`、`acme/`、
  `app/cca-acme.py` 删掉；证书助手看到 `store/` 里还没有、Team 的那一份里有还用得了的就接着用（不再要一张）。
- Team 的名字：`--name`（`cca team deploy` 放进 Team 的名字，单引号写成 `'\''`：`shq`）写进 `app/name`（644），没写时留着上次的；网页端开着时
  服务端带 `--web-name-file`（服务端的 `web_name`：第一行，1～64 个字符、没有控制字符，不对时照旧叫 cca），网页的标题（`cca 远程会话（名字）`）、
  手机主屏幕上的名字（`apple-mobile-web-app-title`、manifest 的 `name` / `short_name`：`cca 名字`）带上它（`web_named`，转义过）。安装脚本查：
  没有换行，`${#NAME}` 最多 256（dash 数的是字节）。
- `.env` 在起服务之前写（`write_env`），没装完（`die`，或者被打断：`trap` 了 HUP、INT、TERM、PIPE，SSH 断了以后往 stdout 写是 SIGPIPE）时换回
  原来的（`rollback`；第一次装的删掉）：下一次照原来的判断是新装还是更新。服务用新的 `.env` 起来了（`wait_served` 过了）才删掉 `.env.prev`、
  不再换回：起来以后再换回的话，跑着的服务和 `.env` 对不上。所以打结果时 SSH 断了（管理员那头是 `ssh`）服务器上已经是这一次的，管理员这台照旧
  接着原来的，再部署是更新、接上新的。
- 直接在服务器上运行：打出接上它的那一行要有地址（第一次装、换了端口），没有时有终端就问、没有就停下要 `--host`（`die host`）；最后说网页端
  怎么样了（`web_say`）、要放行哪些端口。只是更新（端口、证书都没换）时不再打那一行；证书是新生成的（原来的不见了）时说管理员要照新的
  那一行重新接上。
- 卸载：`--uninstall --dir 目录`（`cca team deploy --script | ssh root@服务器 sh -s -- --uninstall --dir /opt/<服务名>`）：只认装过的目录（有
  `.cca-hub` 或者 `.env`），停掉、删掉程序、证书、数据、`.env`、unit（连同以前它自己的证书助手的）和系统用户，去掉它在共用的证书助手那里的
  登记和那一份（最后一个用它的 Team：连同证书助手一起删掉）；说管理员要换一台接上（`cca team attach`），或者各台设备退出（`cca team leave`）。
- 防火墙：脚本不动。systemd 的端口经 ufw、firewalld：开着又没放行时说放行的那一行（`fw_hint`；Docker 发布的端口不经它们）；云服务商的安全组
  最后总要说一句（HTTPS 的端口、中转的数据端口，网页端开着时还有它的端口和 80）。

### 经 SSH 装（`cca team deploy`）

- 安装脚本从 ssh 的 stdin 给过去，前面放进 `CCA_HUB_MANAGED=1`、`CCA_HUB_TEAM_ROOT='根公钥后面那一段'`，命令里没写 `--host` 时
  `CCA_HUB_HOST_HINT='ssh -G 的 hostname'`（服务器上没记着地址——第一次装、搬过去——才用它：更新时不拿 ssh 的主机名盖掉记着的，ssh 走的
  可能是内网地址、别名），和 `set -- --dir … [--name '名字'] [--port …] [--no-web | --web-port …] [--host '…'] [--docker|--systemd]`（`--host` 只是命令里写的：
  网页端的证书要在装之前定下地址，地址由安装脚本定）。ssh 带 `-T`（不要伪终端：脚本从 stdin 过去，ssh 配置里写了 `RequestTTY` 也不要）和 `ServerAliveInterval=30`
  （拉镜像时好几分钟没有输出）。在服务器上运行的是 `sh -c '…'`（登录 shell 不一定是 sh）：root 直接 `sh -s`，别的用户能免密码 sudo 就
  `sudo -n sh -s`，要密码时什么都不运行、说用 root 登录（`root`）。
- 脚本的进度写 stderr（ssh 转过来，直接给用户看），结果一行一项写 stdout（`report_managed`：
  `CCA-HUB mode|port|host|relay|web|web_cert|web_off|pin|state|ok`，`host` 是这次定下的地址，认不出时不说；没成是 `CCA-HUB fail 原因码 说明`），
  `report` 读，`fail_code` 换成辅助程序的原因码（`root`、`deps`、`image`、`port`、`taken`（那个目录里装着别的）、`usage`、`start`、`host`，
  认不出的当作 `install`），bash 里是 `team_` 加上它（`team_fail`）。
- ssh 15 秒连不上算没连上（`ConnectTimeout=15`）；没有终端（App 里、`PROMPT_GUI`，或者没有 /dev/tty）时带 `--batch`（`BatchMode=yes`），ssh 退出码
  255、又没有结果是 `ssh`。整个最多等半个小时。参数不对辅助程序先查（`opts`：`usage`），不连服务器；bash 自己查的（不认得的选项、多写了服务器）
  也是 `team_usage`。
- 成了：stdout 是 `sh` 的 键=值（`mode`、`state`、`port`、`relay`、`dir`、`lan` 地址只在内网、`host`、`device`、`members`、`stored`、`note`、`svc`、
  `web`、`web_cert`、`web_off`），bash `eval` 它，说部署了还是更新了、要放行哪些端口、网页端怎么样了，地址是内网的 `fail_warn team_lan`，接着
  `team_settle`，`result` 的 `team_deployed`。

### App 里

问服务器、查登录、问密码、给出加公钥那一行的几段（`hub_deploy_ask`、`hub_deploy_where`、`hub_pubkey_ask`、`hub_deploy_run`）的名字、题号
（`hub.deploy`、`hub.password`、`hub.pubkey`）、原因码 `hub_ssh`、交给终端的 `hub_deploy` 是原来部署自己的同步服务器时起的，照旧（PROTOCOL 4.1、
4.6）；是哪个 Team 由 `DEPLOY_PY` / `DEPLOY_WORDS` / `DEPLOY_WHAT` / `DEPLOY_FAIL` / `DEPLOY_AT` / `DEPLOY_MSG` 说（`team_deploy` 设好），所以表单的
标题和说明说的是这个 Team。

- `hub_deploy_ask`（`PROMPT_GUI`，不写服务器）：先问一页（`hub.deploy`：地址、SSH 用户、端口；接着服务器的预填它的地址和 root、22，`message`
  说是更新、填别的服务器就是搬过去），再 `py team-deploy 编号 目标 --ssh-port 端口 --batch --check` 查登录得上不（`ssh_check`：只运行 `true`，
  什么都不装）。不成时 stdout 是 `ssh_trouble` 按 ssh 最后说的话分的原因码：
  - `ssh`（连不上：地址、端口不对，主机名解析不了……，`SSH 连不上 目标：ssh 说的最后一行`）带着原因重问这一页；
  - `ssh_auth`（密钥登录不了、服务器认密码或者 keyboard-interactive；还有 `Too many authentication failures`：ssh-agent 里的密钥太多，服务器只让
    试几次（`MaxAuthTries`），没轮到密码就断了，`LogLevel=ERROR` 时这是 ssh 说的最后一句）接着问密码（`hub.password`）；
  - `ssh_keyonly`（服务器不让用密码、只认密钥）接着 `hub_pubkey_ask`：一页 `hub.pubkey` 给出在服务器上运行的那一行（`hub_key_file`：`~/.ssh` 里
    ssh 默认会试的 id_ed25519、id_ecdsa、id_rsa 第一把有公钥文件的；`echo 公钥 >> ~/.ssh/authorized_keys`，用户在云服务商网页上的远程登录里
    运行），选「加好了，再试」再查（还不认带着原因重问，这回认密码了回去问密码）；还没有密钥的先问要不要生成一把（`ssh-keygen -t ed25519 -N ''`，
    `~/.ssh/id_ed25519` 已经有了、只缺公钥文件的不覆盖，说怎么补）；私钥有密码（App 里没法输）、要用别的私钥的选「在终端里部署」：
    `gui_terminal hub_deploy`，`argv` 是 `team deploy <编号的前 8 位> 用户@地址 -p 端口`，App 照 `terminal` 事件交给终端；
  - `ssh_hostkey`（主机密钥和 known_hosts 里的不一样：说 `ssh-keygen -R`）是 `hub_ssh`，停下。
- 问到的密码经 `askpass_run`（和配置路由器的 `router_ssh_password` 共用：700 的临时目录里的 FIFO，写的一方是 bash 内建的 printf，askpass 读一次
  就删掉，ssh 再问读不到东西也不会卡住）交给 `--askpass --check`：用了密码还 `Permission denied`（服务器认密码）是 `ssh_denied`（密码不对，重问，
  最多三次：错得太多，服务器上的 fail2ban 这类会封这个地址；服务器两种都认时 ssh 一次连接错两回：keyboard-interactive 用了密码，`password`
  拿到的是空的。很多系统默认不让 root 用密码登录（`PermitRootLogin prohibit-password`），却照样问密码、照样说 `Permission denied`：这边分不出来，
  用户是 root 时原因里提醒换成能免密码 sudo 的用户）；用密码那一次才看出来的 `ssh_keyonly`（多半是密钥太多，说在 `~/.ssh/config` 里写
  `IdentitiesOnly`）、别的 `ssh…` 是 `hub_ssh`，停下。对了以后部署那一次也经它（`hub_deploy_run`：部署只有一次要登录的连接，`ssh -G` 不登录）。
- App 里（`--check`、`--askpass`）没人能答要不要认这台主机：第一次连的认下（`StrictHostKeyChecking=accept-new`，和终端里答 yes 一样，变了照样拦；
  要 OpenSSH 7.6，Mac 上的都够新），命令行里没有终端（只有 `--batch`）的照旧不认没见过的主机（ssh 自己说 Host key verification failed），
  `LogLevel=ERROR` 不让「Warning: Permanently added」混进 ssh 最后说的那一行；`--askpass` 带 `PubkeyAuthentication=no`、
  `PreferredAuthentications=keyboard-interactive,password`、`NumberOfPasswordPrompts=1`，不带 `BatchMode`（它连 SSH_ASKPASS 也不问）。
- 终端里不问这些：服务器写在命令行上，ssh 自己问密码、问要不要认这台主机（有终端时不带 `--batch`）；终端里没写服务器（App 失败页的「改在
  终端里完成」交过来的就是不带服务器的 `cca team deploy 编号`）时 `hub_deploy_where` 先问地址（接着的预填）、SSH 用户（root）、端口（22），
  没有终端的照旧说用法。SSH 的目标不记：App 里再部署预填的是接着的那台的地址和 root、22，经别的用户、端口、ssh 配置里的别名部署的要自己改。
- `cca team new` 在 App 里不写名字时问（`team.new`，校验 `teamname`），接着一页多选问放进哪些账号（`team.accounts`，见「管理员这一头」），
  成了是 `team_created`；部署成了 `team_deployed`，地址是内网的另有 `team_lan`。
- 页面：Team 卡片下面「新建 Team…」（`Handoff.teamNew`），还没接上服务器的 Team 那一行有「部署到我的服务器…」，接着的在「更多」里
  「更新服务器、搬到别的服务器…」（`Handoff.teamDeploy`）。托管平台在 App 里还没有页面（卡片下面那句话说在终端里运行 `cca team hosted`）。
  截图场景 `flow-team-deploy`、`flow-team-password`（夹具 `flow_team_deploy.flow`）。

### 托管平台

- `cca team hosted Team [railway | sealos] [--dir 目录]`（`py team-hosted`，`cmd/hub/hosted.rs` 的 `team_files`）：没有服务器时部署到 Railway 或者
  Sealos（2026-11 调研过的两家：国外一家、国内一家；Cloudflare 那种要另写一份服务端，不做）。只在终端里：平台不写时问（`hub_platform_ask`，
  终端里 1/2，提问会话里是 `hub.platform`——App 不调这条命令；没有终端时说用法）。
- 服务端是同一个程序，平台做 HTTPS：`--behind-proxy`（只说 HTTP；限流按 `X-Forwarded-For` 的最后一项，同一个 IP 的连接数不限）；平台的表单
  只填得了环境变量和一条命令，所以 `CCA_HUB_HOSTED=1` 等于 `--mode team --behind-proxy --listen 0.0.0.0:8080 --state /data/state.json`（端口固定
  8080：Railway 的 `PORT` 不用，`railway domain --port 8080`），`/data` 不在时不启动；根公钥是 `CCA_TEAM_ROOT`（`ssh-ed25519` 后面那一段）。
  生成的设置里没有中转、网页端：托管平台上的 Team 服务器不给远程会话做中转。
- 生成部署文件（目录默认 `~/<服务名>-<平台>`；目录里只能有这几个文件和点开头的，不然 `dir`；这几个文件再运行时照样覆盖——更新要的——
  `fs::put` 跟着符号链接写，目录里有人放了同名的链接时写到它指的地方）：服务端程序 `cca-presence.py`、`README.md`（第一次部署、更新的步骤），
  Railway 另有 `Dockerfile`（`python:3.13-slim`、`ENV CCA_HUB_HOSTED=1 CCA_TEAM_ROOT=…`；不用 railway.json：它的 config-as-code 已经不推荐了，
  挂载存储也没写进文档）；Sealos 表单里的配置文件 `/app/cca-presence.py` 就是程序的内容。文件里只有根公钥（公开的）。stdout 是 `sh` 的 键=值
  （`dir`、`steps`、`tid`、`name`、`svc`）。`steps` 是要做的几步：Railway 是一行一行的命令（`cd 目录`、`railway login`、`init --name <服务名>`、
  `add --service <服务名>`、`volume add --mount-path /data`、`up --detach`、`domain --port 8080`；没有 `#` 注释：交互的 zsh 默认不认它），Sealos
  是表单里各项填什么（镜像、固定 1 个实例、容器端口 8080 打开公网访问、启动命令、两个环境变量、`/data` 的存储卷、配置文件）。
- 部署好了：`cca team attach Team https://平台给的地址`（平台给的地址只到主机名：没写端口时 `cmd_team_attach` 补上 443（`with_https_port`），
  再照 `url_ok` 查；不写指纹：照系统的证书机构验平台的证书，curl 不带 `-k`、`--pinnedpubkey`；验不过接不上）。
  这个 Team 已经接着一台没有指纹的服务器时再运行是更新（步骤只剩换上新版的程序：Railway 是 `railway up --detach`，Sealos 是「变更」里换配置
  文件的内容），数据在 `/data` 里照旧，设备不用重新加入。

还没做的：托管平台的那一页（App 里）；Railway、Sealos 上没真的部署过 Team 的服务器（步骤照它们现在的文档写，生成的 README 里说以平台现在
的界面为准）；真的 Linux 服务器上没跑过安装脚本（假服务器 `tests/hub_box.py` 上跑的，证书助手换成假的；`tests/linux.sh` 会在各个发行版的容器里
跑同一份测试）。`cca team deploy` 传不了 `--relay-port`、`--no-relay`（要换中转的端口、不开中转的，在服务器上直接运行安装脚本）。

## 界面（M5）

接口（PROTOCOL 2.1 的 `hello.team`，2.2 的 `config.teams`、账号的 `team`，4.14）：

- `hello.team` 是 1：App 才显示 Team 的那一块、才筛选、才调 `cca team`（旧版 cca 的 state 里就算有也不认）。
- `_gui state` 的 `config.teams`：这台设备在的 Team（照加入的先后，每项和 `cca team ls --json` 的一样：`team_rows`）。**一个都不在时没有
  这个字段**，账号的 `team` 也只有 Team 给的官方账号才有：没有 Team 的机器输出和原来逐字节一样。
- `cca team devices <Team> --json` 多了 `name`、`accounts`（工具 → 邮箱 → 节点）、`aliases`、`nodes`：管理页一次要到的就是这一份。3.5.3-beta.6 起
  还有 `tool_aliases`（工具 → 邮箱 → 别名，照 `alias_plan` 算：还没发布过的旧 Team 也是按工具编好的样子），App 有它时照它（`TeamDevicesDoc.accountList`）；
  3.5.3-beta.7 起还有 `alias_lock`（App 有它才放「锁定别名」的开关）。
- `_gui state` 的账号的 `alias_locked`（3.5.3-beta.7 起）：别名被 Team 锁定了，App 里账号页的名字不能点、侧栏菜单的「改别名…」灰着。
- 邀请链接是那台设备的密钥：`cca team invite` 的 stdout 只经内存里的管道读（`CcaCommand.teamInvite` 不是跑完为止的那种），放进剪贴板
  （`copySecret`：标成机密、60 秒后清掉），不进被观察的状态、提示和读屏。

App 里（`macos/Sources/CcaKit/Protocol/Team.swift`、`AppModel` 的「Team」一节、`CcaApp/TeamsCard.swift`）：

- **按 Team 筛选**（`TeamFilter`：全部 / 某个 Team / 不在 Team 里的）：侧栏（工具切换下面一行）、总览（标题右边）、菜单栏面板（工具切换
  下面）各放一个 `TeamFilterMenu`，改的是同一个选择（`AppModel.teamFilter`，记在 App 的设置里）。只在加入了 Team 时放出来。**只管显示**：
  `shownGroups` / `shownAccounts` 和 `office(now:)` 筛，`arrangedAccounts`（写回账号表的顺序、默认选中哪个）照全部的；启动器、账号表都不变。
  选的那个 Team 已经不在了（退出了）当作全部（`activeTeamFilter`）。筛选着的时候没有账号的组不摆「还没有账号、去添加」的空位。
- **「共享与同步」页的 Team 卡片**（`TeamsCard`）：各个 Team 的状态（照 `config.teams[].state`，cca 给了原因的照说）、给了这台设备几个账号；
  「粘贴邀请链接…」是提问会话（`Handoff.teamJoin` → `cca team join`，`team.join`），「同步配置」是 `cca team sync`，「更多」里退出 / 删掉
  （cca 先问同意：`team_leave` / `team_delete`）。有 Team 要处理（被移除、记录没了、没同步成）时侧栏这一行挂「Team 要处理」。
- **管理员的 Team 展开「设备和账号」**（展开时问一次 `team devices --json`，低优先级；改过东西以后再问一次）：设备（这台在前；状态、给了它
  几个账号、它手里的配置是不是最新的——不说第几版，序号只是用来比新旧的；名字只显示管理员起的 `label`；有 `problem` 的挂「要处理」、下面一行说原因）、
  每台设备的「账号」菜单（全部 / 逐个勾：`cca team grant`；全部都给
  的设备去掉一个 = `none` 加上剩下的）、「改名…」（这一行下面开一个输入框，`AppModel.teamRename`；名字没变不发命令，没改成时输入框留着：
  `cca team rename`）、移除 / 撤回邀请（`cca team revoke`，先问同意）、发邀请（起名字、勾账号，链接进剪贴板）、各个账号走
  哪个节点（`cca team map`；能选的是这个 Team 带着定义的节点加上这台设备上有的）、各个账号的「改别名…」（账号那一行下面开输入框，`AppModel.teamAlias`，
  按 `工具:邮箱` 改：`cca team alias`，只改这个工具里的；别名没变不发命令）、账号那一块标题右边的「锁定别名」开关（`cca team alias Team --lock`
  / `--unlock`）。加账号、导入还在终端里（`cca team map`、`import --mine`）。
- 改账号表的几条命令（`sync`、`leave`、`grant`、`map`、`alias`，连同锁定别名）跑完为止、和同步路由器排一个闸门（`router-sync`）；`invite`、`revoke`、`rename` 排 `team`。
- `HubKind` 认得 `team` 了；`config.hubs`、`devices --json` 的 `hubs[]` 里 Team 的服务器照 `team`（编号）分，账号问的是哪一台照
  `(hub, team)` 认（`DevicesDoc.hub(for:)`）。
- 截图场景：`teams`（卡片，展开着管理员的 Team）、`team-filter`（总览筛出一个 Team）；屏幕睡着时系统不给截屏，`Snapshot.capture` 退一步
  让窗口自己画进位图（没有玻璃效果，侧栏画不出来；版面和文字看得到）。

## 本机的状态和命令（M2）

`~/.cca/teams/`（目录 700，密钥、配置、状态文件 600，`ssh-keygen` 生成的 `.pub` 是 644；整个在 `CCA_DIR_PRIVATE` 里；布局在 `cli/src/core/team.rs` 开头）：`index.json` 是加入的先后，每个 Team 一个目录
（名字是编号）：`team.json`（名字、服务器、根公钥、角色、这台设备的编号和它在服务器上的名字、`gen`）、`id_ed25519`（这台设备在这个 Team 里的密钥）、
`secret`（邀请里那串随机数）、`doc.json`（验过、解开的配置和它的序号）、`sync.json`（最近一次同步配置的结果，变了才写）、`lock`；管理员的另有
`root_ed25519`（根密钥）和 `admin.json`。index.json 里没有、目录在的也算（排在后面：建到一半、加入到一半被打断留下的，`teams()`）。
**一个 Team 一把锁**（`team_lock`：`flock` 那个 `lock` 文件）：同步配置、管理员的每条命令整条拿着——读 admin.json、联网、写回这一段别的命令插不进来，
不然两个邀请会用同一个序数、算出同一把密钥。锁着的时候会联网，所以不用全局的 `Lock`（index.json 才用它）；同一个进程里不能拿两次（已经拿着的调 `sync_locked`）。分层：`core/team.rs` 只读文件（以后对账用它，不碰密钥、不联网），格式和密码学在 `cmd/team/doc.rs`，
设备这一头的子命令在 `cmd/team.rs`，管理员的在 `cmd/team/admin.rs`；bash 的 `cmd_team` 只管问和说。

设备这一头：

- `cca team join [--stdin] [--name 名字]`（`py team-join`）：邀请链接经 `ask_secret`（`team.join`）或者 stdin 给，不进命令行。没写 `--name` 时这台设备
  报给服务器的名字用管理员发邀请时给它起的（邀请里的 `d`，3.5.3-beta.7 起；旧版本发的邀请里没有、或者不合规时用主机名）。生成这台设备的密钥，
  拿邀请里的令牌加入，带上公钥的认证码；服务器回的设备编号要和自己照 `k` 算的一样。成了写 team.json、`secret`、记进 index.json，马上同步一次配置。
  没成什么都不留——连不上时请求可能已经到了服务器（那样邀请就用掉了）：原因里说再试一次、说用过了就让管理员移除重发。已经在这个 Team 里的不换
  （免得一条旧链接把正用着的换掉），只有被移除了的（`sync.json` 是 `removed`）拿新的邀请直接换上；team.json 坏了、管理员的密钥还在的也不当成没有。
- `cca team sync [Team]`（`py team-sync`，`sync_one`）：`team_get` 带着手里的序号。回来的那一份先核对设备编号，再验根密钥的签名（`ssh-keygen` 用不了
  和验不过分开说，都不用它）、认证解密、查内容；序号比手里的小的不要，一样的什么都不动，大的才换上：**先**照它改 team.json（名字、服务器在哪）、
  **再**写 doc.json（中间断了下次接着做完）。服务器回的这台设备的名字（`me_name`：管理员改过名字）和 team.json 的 `device` 不一样时也跟着改
  （没有签名，只是给人看的）。结果记进 sync.json：`ok`；`removed`（403 `unknown_device`，`gen` 和记着的一样）；`lost`（`gen` 不一样：
  服务器的记录没了，等管理员登记回去，这台设备什么都不用做）；`error`（连不上、签名不对、内容不对……原因在 `error`，手里的那一份照旧用着）。
- `cca team leave [Team]`（`py team-leave plan | apply`）：先问（成员 `consent team_leave`，管理员 `team_delete`，范围是 Team 的编号）。请服务器移除
  这台设备（连不上也接着退），本机的目录删掉。管理员退出就是根密钥没了。
- `cca team [ls] [--json]`（`py team-list`）：只看本机记着的。状态另有 `local`（管理员还没接上服务器）、`pending`（还没同步过）。

管理员这一头（`admin.json` 的样子在 `cmd/team/admin.rs` 开头）：

- `cca team new 名字 [--device 名字] [--accounts all|none|工具:邮箱,…]`：生成根密钥、主密钥（32 个随机字节）和盐，这台设备是第 0 台设备。先在
  `teams/.new.<进程号>` 里建好再改名（编号要有了根公钥才知道；以前建到一半留下的过一个小时清掉）。说下一步：服务器用这把根公钥运行，再 attach。
- **新建时放进这台设备上的账号**（3.5.5 起）：能放的是 `team_picks` 列的（`py team-candidates`：一行一个，工具、工具的名字、邮箱、别名、已经在哪个
  Team 里、节点，没有的栏是 `-`）——账号表里有邮箱、有节点的官方账号，自己加的（`own_account`），或者这台设备当管理员的别的 Team 给的（账号和
  节点的定义本来就归这台设备的人管）；这台设备只是成员的 Team 给的不能（是别人的，不能转手给别的设备），第三方 API 账号也不能
  （Team 的配置里只有官方账号）。没写 `--accounts` 时 bash 问（`team_new_pick`）：App 里一页多选（`team.accounts`：选项的值是编号，自己加的
  预先选好——写在 `value` 里、`default` 是空的，因为 `gui_ask` 拿默认值顶替空的答案，都不勾就成了全选），终端里列出来按编号选（`a` 全部、`0` 一个
  都不放，直接回车是自己加的那几个），选的拼成 `--accounts 工具:邮箱,…` 交给 `py team-new`；没有终端的不问、一个都不放（说怎么放）。
  `put_accounts`（`import --mine` 也用它）：节点照这台设备的账号表；别名照这台设备上起过的带进去，默认样子的 `team-N` 不带（是别的 Team 编的号，
  这里照 `default_aliases` 另编）；别的 Team 给的账号，那个 Team 带着它的节点的定义时连同经 `dialer-proxy` 连着的一起带过来（`needed_nodes`：
  别的设备照这一份连，不一定有这个节点；同名的已经有了的不换，策略组照第一个带过来的），自己加的账号的节点不带（各台设备自己有）。都在生成密钥
  之前算好：写的账号不能放、带过来的节点的定义不合格（`fill_problems`）时是 `accounts`，什么都没建。stdout 第三行是放进了几个。一个账号同时在
  两个 Team 里时，这台设备上照先加入（建）的那个用（`mapping` 照 index.json 的先后），别的设备照它们自己在的 Team；以后各改各的。
- **每台设备那串随机数不存**：`k = HMAC(主密钥, "cca-team-k-v1\0" 序数)` 的前 16 个字节，要用时重算（`device_secret`）。所以管理员这边要备份的只有
  根密钥和主密钥；序数发一个邀请用掉一个（没发成也不回收）。导出、交给第二个管理员的命令还没做。
- `cca team attach Team 地址 [指纹…]`：先 `team_enroll` 这台设备（服务器认不认这把根密钥、是不是 Team 的服务器，这一下就知道；认了才把地址写进
  team.json），再对一遍、发布。**换一台服务器**也是它：先在 admin.json 记下 `moved_from`（原来那台的地址、还没用掉的邀请），新的那台上把加入过的设备
  都登记回去、配置都发布一版（里面是新的地址），最后到原来那台上收尾（`finish_move`）——没用掉的邀请撤回（它们写的是原来那台的地址：不撤的话拿着的人
  会加入一台没人管的服务器；这边记成 `expired`，要重新发），同一个序号的配置也存一份上去：还连着原来那台的设备下次同步配置时拿到它、照 `server`
  换过去，到新的那台上发现手里的就是它存的那一版。中途没做完的（`moved_from` 还在）下次 `attach`、`publish` 接着收尾；原来那台连不上时说一声
  （`fail_warn team_forward`：那些设备找不到新的服务器，要重新邀请）。
- `cca team invite Team 名字 [--accounts all|工具:邮箱,…] [--ttl 秒] [--key '公钥']`：取下一个序数算出 `k`，`team_invite` 登记令牌的哈希，记进 admin.json，
  马上把它的配置存上去（设备一加入就能同步下来）。邀请链接只打到 stdout 一行；在 Claude Code 的会话里不给。默认 7 天过期。链接里带着这个名字（`d`，
  3.5.3-beta.7 起）：设备加入时用它。还在这个 Team 里的别的设备已经叫这个名字（`label` 或者服务器上的 `name`）的不行（`name`，和 `rename` 一样：
  `name_taken`）。`--key` 指定要拿哪把公钥加入（admin.json 里那台设备的 `expect`）：对账时服务器报的公钥不是它，就记成 `mismatch`——这段时间一个
  账号都不给它（`build_doc` 看这个记号，授权本身不动），告诉管理员移除它（提示里带设备编号：同名的不止一项），它已经取走的那一份收不回来；服务器
  没报公钥不算，后来报的对上了记号就去掉（服务器一时说错，不该把一台好好的设备的账号永久拿掉）。
- **和服务器对一遍**（`reconcile`，`devices`、`grant`、`import`、`map`、`publish`、`attach` 都先做）：`team_members` 的名单和 admin.json 比——
  - 加入了的设备 `invited` → `joined`，`name` 照服务器的（样子不对的不要）。公钥第一次见到时过了认证码才记下（没过的照样给它发布配置，名单里的
    `problem` 说一声：服务器的记录没了以后它登记不回去）；记下以后服务器报的不一样就不照它的（`problem`）。管理员自己这台的公钥是本机的。
  - 邀请不在了、设备也没加入的记成 `expired`（管理员分不出是过期了，还是加入以后没等到对这一遍又退出了）。
  - `gen` 没变、服务器上却没有了的是自己退出了（`left`，不登记回去；它又出现了恢复成 `joined`）。
  - `gen` 变了（服务器的记录没了、换了一台）才照**记下的**公钥把 `joined` 的都登记回去、配置都重新发布。
  - 服务器上某台设备存着的那一版比发布过的旧（它的记录回滚了、丢了）：重新发布。
  - **设备只有一个名字**（`align_names`，3.5.3-beta.7 起）：管理员起的 `label`。加入了的设备在服务器上的名字（`name`：旧版本发的邀请里没有名字，
    它报的是主机名；或者加入时写了 `--name`）不一样时，照 `label` 再 `team_enroll` 一次（和 `cca team rename` 一样：同一把公钥），它下次同步配置时
    跟着改（`me_name`）。`label` 不合规、这个 Team 里还在的别的设备用着（它们的 `label`，或者不跟着改的那些的 `name`）时反过来，`label` 记成
    服务器上的；服务器给改成了别的（服务器上有这边没记着的设备叫这个名字时加 `-2`）也照服务器的记。要让出名字的先登记（服务器照先后查重名）。
    公钥对不上的、管理员自己这台不碰；没登记成的原因进 `problem`，下次对账再试。名字一样了就不再发请求。
- **发布**（`publish`）：给每台 `invited` / `joined` 的设备照授权拼出明文，内容的摘要（发布时间不算）和上次发布的一样就不动；变了的序号加一、
  封好签好，一个请求最多 32 台、密文加起来不超过 600 KB（`put_docs`，换服务器的收尾也用它）。服务器说序号不比它存着的大（管理员的状态是从备份
  恢复的）：它说的数不比记着的小、没到上限才信，接着它的往上加再存一次（只追一次）；它明说不认得的编号记成 `expired`；回话里没提到的不动、说一声。
- `cca team grant Team 设备 all | none | +工具:邮箱 | -工具:邮箱 …`、`cca team map Team 工具 邮箱 节点|-`、`cca team import Team --mine`
  （见「自己的同步服务器 → Team」；3.6.0 以前还能导入维护者映射仓库里的 policy.json、nodes.json，跟着映射删掉了）、`cca team publish Team`：改完都发布。
  节点的定义一个个加、复制、去掉、实测见「Team 的节点」（`cca team node`）。
- **默认的别名**（`default_aliases` 照 `alias_plan` 改，3.5.3-beta.5 起；3.5.3-beta.6 起按工具记）：配置的 `tool_aliases`（{工具: {邮箱: 别名}}）里
  没有别名的账号（不合规的、在这个工具里和前面的账号撞了的也算）编成 `team-1`、`team-2`……——各工具各编各的，各自的第一个账号都是 `team-1`
  （照各工具里加进来的先后）；同一个工具里不重复。admin.json 的 `alias_n`（{工具: 编到第几}）记着各工具编到第几，用过的号不再用（去掉的账号的号
  不给新加的）；不在 Team 里了的账号的别名去掉。按邮箱的 `aliases` 照它算（每个邮箱用它最前面那个工具里的：旧版本的设备、管理员这台换回旧版本
  时用）。没有 `tool_aliases` 的 Team 照 `aliases` 给它在的每个工具；3.5.3-beta.5 的 `alias_n` 是一个数（那时各工具一起编：Claude 有 2 个账号时
  Codex 的从 team-3 编起），那时编的 `team-N` 都去掉、照各工具重新编。`import`、`map`、
  `alias` 时和每次发布前都过一遍（以前建的 Team 下次发布时换过来，各台设备的配置因此都发布新的一版）。`import --mine` 带进这台
  设备上起过的别名（各工具的账号表各是各的）：Team 里原来是默认的 `team-N`、这个工具里也没有别的账号叫它时换成它；没起过的（还有默认样子的
  `team-N`）编成 `team-N`。
- `cca team alias Team 账号 新别名`（`py team-alias`）：账号写 `工具:邮箱` 只改这个工具里的，只写邮箱改它在的每个工具里的；别名不合规、
  同一个工具里别的账号已经叫它是 `alias`（别的工具里的可以），Team 的配置里没有这个账号是 `no_account`。改完发布（stdout：原来的别名——几个工具里
  不一样时用「、」连起来——、存上了几台），各台设备下次同步配置时照上面的规矩跟着改；管理员这台设备的账号表不动。
- `cca team alias Team --lock | --unlock`（`py team-alias-lock Team on|off`，3.5.3-beta.7 起）：admin.json 的 `config.alias_lock`，再对一遍、发布
  （配置里带 `alias_lock: true`；解开时去掉这一项，没锁定过的 Team 的配置和以前一样）。stdout：Team 的名字、存上了几台。锁定了，成员设备上
  （`AliasFrom.locked`：配置里有它、这台设备不是这个 Team 的管理员）：`team_alias_follow` 里账号表的别名不是 Team 给的（或者撞了以后的
  「Team 的名字-N」）就换，有没有记录、自己改没改过都换，去掉了别名的也补上；`cca rename` 不让改（`alias_locks`：退出码 4，bash 报成
  `alias_locked`），`cca edit` 手改的下一条命令换回来；`_gui state` 的账号带 `alias_locked`。管理员自己这台的账号表照旧不管；管理员照样能改别名，
  各台设备跟着换。解开以后又能自己改，现在用着的不变。
- `cca team name Team 新名字`（`py team-name`，3.5.3-beta.6 起）：改 team.json 的 `name`，再发布（配置里的 `name` 变了，各台设备都发布新的
  一版，取到时跟着改 team.json：`follow`，3.5.3-beta.1 起的设备都会）。名字照 `team new` 的规矩（1～64 个字符、不能有控制字符），这台设备上别的
  Team 已经叫这个的不行（`name`：按名字选 Team 就分不清了）。已经发出去的邀请里带着的还是原来的名字，拿它加入的设备第一次同步到配置时换成新的；
  成员设备上撞了以后用的「Team 的名字-N」别名不跟着换。手机网页端的标题是部署时写进服务器的，下次 `cca team deploy` 才换。先和服务器对一遍
  （`reconcile`）：连不上、对不上（`server`）时名字还没改，照原来的命令再来一次就行；发布没成（`publish`）时名字已经改了，照原来的名字再来一次
  找不到这个 Team，所以原因里说照新名字 `cca team publish`。stdout：原来的名字、新的名字、存上了几台。
- `cca team revoke Team 设备`（`py team-revoke plan | apply`）：先问（`consent team_revoke`，范围是设备的编号）。管理员移除不了自己这台。
- `cca team rename Team 设备 新名字`（`py team-rename`）：admin.json 里它的 `label` 换成新名字；`joined` 的先对一遍（自己退出了的不登记回去），
  再照它的编号把**同一把公钥**再 `team_enroll` 一次、带着新名字——服务器上加入的时间、认证码（`proof`）照旧（3.5.3-beta.5 起的服务器；以前的
  服务器丢掉认证码，管理员这边已经记下了公钥，不碍事），远程会话的配对按公钥的指纹认，也照旧；名字被别的设备用了时服务器在后面加 `-2`
  （`label`、`name` 都记服务器给的：一台设备只有一个名字）。公钥用记下的（管理员自己这台是本机的）；没记下的用服务器报的（它本来就存着这一把，再登记一次不多给谁什么）；记下的
  和服务器报的不一样时不登记。那台设备下次同步配置时跟着改 team.json 的 `device`（`follow`）。还没加入的只改 `label`（发出去的邀请里还是原来的名字：
  它照那个名字加入，对一遍时 `align_names` 改成新的），已经不在的只改 `label`；管理员自己这台还没
  接上服务器时改的是接上时用的名字（team.json 的 `device`）。还在这个 Team 里的别的设备已经叫这个名字（`label` 或者 `name`）的不行（`name`），
  不然以后按名字就分不清。不发布：配置里没有设备的名字。**远程会话里已经配好的对不跟着改名**：配对表（各自的 `remote/peers.json`）记的是配对那一刻服务器的设备清单里
  对方的名字（没有时是它自己报的主机名，docs/dev/remote.md），只是给人看的、认设备靠指纹；改名以后再配对一次才换成新名字。
- `cca team devices Team [--json]`：对一遍以后列出来（`seq` 发布到第几版、`have` 它手里第几版、`problem` 对这一遍时发现的要告诉管理员的；
  `--json` 另有 `alias_lock`，给 bash 的那一份第一行是 1 / 0）。序号只用来比新旧：给人看的地方（`cca team`、`team sync`、`team devices`、App）
  只说「配置已同步」「配置是最新的」「还没同步到最新的配置」，不说第几版。
- 给 bash 的一行一项、制表符分开的输出里，没有的栏写 `-`（`team-list`、`team-devices`）：bash 按制表符读，空的栏会被并掉、后面的往前挪。

卸载（`cca restore`）：确认清单里一个 Team 一行（成员：退出；管理员：根密钥跟着删掉），真做时各个 `team-leave apply`（`restore_teams`）。备份不带
`teams/*/` 下的 `secret`、`root_ed25519`、`admin.json`、`doc.json`、写到一半的 `*.tmp` 和建到一半的 `teams/.new.*`（各台的 `id_ed25519` 本来就不带）。

还没做的、要知道的：

- `_gui state` 里没有 Team 的列表、账号是哪个 Team 给的（M5；只有 `hub` 是 `team` 和 `config.hubs` 里的项）。
- 根密钥和主密钥的导出、第二个管理员（现在换一台管理员的设备只能手工拷 `~/.cca/teams/<编号>/`）。
- `cca help` 里还没列 `cca team`（还用不上：账号表不照配置改）；失败的原因码在 Swift 的 `FailCode` 里还没登记（App 的页面是 M5）。
- 设备加入以后、管理员对一遍之前服务器的记录就没了：那台设备的公钥没记下，登记不回去，要重新邀请。
- Linux 上没跑过（代码里没有只在 macOS 上有的东西；`ssh-keygen -Y` 要 OpenSSH 8.0 起）。

## Team 的节点（`cca team node`，3.5.3-beta.8 起）

以前维护者改映射仓库的 `nodes.json`（或 `custom.yaml`）、签名推送（映射 3.6.0 起去掉了）；节点的定义在管理员这台的 admin.json 里（`config.nodes`，
`{"group", "proxies"}`，和以前的 `nodes.json` 一个样子），管理员用这几条命令改。辅助程序在 `cmd/team/admin/nodes.rs`，bash 是 `team_node`：

- `cca team node Team [ls [--json]]`（`py team-node-ls`）：不联网、不显示密钥（只有名字、类型、服务器、端口、`dialer-proxy`、走它的账号、
  是不是安装客户端用的）。`--json`（`kind: team_nodes`）另带这个 Team 全部的账号 → 节点（`accounts`：走的节点不在定义里的也算）。
- `add [链接]`：写法和 `cca node add` 一样（链接或者粘贴一段 Clash 节点配置，`node_read_block`）：`py node-check` 解析、起不冲突的名字
  （避开这个 Team 已有的和这台设备知道的），`node_validate` 拿内核 `-t` 校验，`py team-node-add`（stdin 是节点的 JSON）。
- `copy 模板 新名字 [--port 端口]`（`py team-node-copy`）：整份复制一个节点（`deepcopy`，`reality-opts` 这些也是新的一份），只换名字、
  端口。一台服务器上按端口分开的几个入站（各有各的出口）就这样一个个加，节点的密钥不出这台设备（skill 也不用经手）。
- `rm 节点`（`py team-node-rm`）：还有账号走它、别的节点经它连出去、它是 `install_node` 时不去掉（`node_used`，原因里说是谁）；去掉最后
  一个时 `config.nodes` 回到 null。
- 改了都先过一遍成员设备查配置的那一套（`fill_problems`），不合格的不存（`node`），再和 `map` 一样对一遍、发布。给各台设备的配置只带它的
  账号用得着的节点（`needed_nodes`）：加、复制、去掉没人用的节点，哪台设备的配置都不变（`各台设备的配置没有变`），有账号走它了（`cca team map`）
  才发出去。还没有节点的 Team 第一次加时策略组是 `OpenAi`。
- `test [节点…] [--tool 工具] [--json]`（`team_node_test`）：实测出口。另起一个临时内核（`core_bin`；不碰在跑的内置代理和路由器），
  `py team-node-testconfig` 把配置写进 `mktemp -d` 的目录（700，文件 600、`O_EXCL`，EXIT trap 停内核、删目录；corenet 的
  `test_proxy_config`：127.0.0.1 上每个节点一个入站 `cca-test-N` 直通它，带上 `dialer-proxy` 连着的，`MATCH,REJECT`，不开控制接口、
  TUN、DNS；别的程序开着 TUN 时和内置代理一样绑定物理网卡，`core_bind_apply`）。经每个入站同时取账号要用的服务（`--tool` 的，没写时看这个 Team 里走这些节点的
  账号是哪些工具的，都没有按 Claude：claude.ai / chatgpt.com）和 www.cloudflare.com（服务器自己的出口）的 `/cdn-cgi/trace`，各试两次；
  哪个都没取到的直连它的服务器端口（`port_open`），分清没开入站和入站参数不对。结果交给 `py team-node-verdict`（`judge`）：不通、经它出得去
  而要用的服务不通、出口在受限地区（`RESTRICTED`）、和别的测着的节点在服务那边是同一个 IP、别的节点在那边有自己的出口而它看到的是服务器
  自己的 IP。退出码：0 都好；1 有节点不通；2 全都不通（多半是这台机器的网络）；3 都通了、有要提醒的。不写节点是全部。`--json`
  （`kind: team_node_test`：`hosts`、每个节点的 `state`、`exits`、`tcp`、`notes`）给 skill 用。
- 只在命令行里（App 不调）；失败的原因码 `team_no_node`、`team_node`、`team_node_used`、`team_name`、`team_usage`（PROTOCOL 4.14）。
- 加外部账号的 skill（`cca-external-account`，只在维护者的仓库里、不进 git）3.5.3-beta.8 起照它做：`copy` 出新节点 → `test`
  （同一组里有账号在用的节点当对照）→ 好了 `cca team map`，不好 `rm` 撤回；不再碰映射仓库、不签名推送。要这台 Mac 装着有 `cca team node`
  的版本。

## 测试

- `tests/presence.py` 的 `TeamServerTest`（回归的 unit 一节跑整份）：真的服务器进程、真的 `ssh-keygen` 签名，客户端是照文档独立实现的
  `tests/spec_wire.py`；签请求的辅助函数在 `SignedCalls` 里。
  - `test_flow`：`/v1/health`；管理员的 op 只认根密钥（设备的密钥、冒用根公钥指纹的别的密钥、改过的请求）；根密钥登记、加入不了；登记、
    邀请（令牌和那串随机数不落在服务器上）、加入（没有这个邀请 / 签名不对一样的 403，用过了、同一个编号再发前一个作废、直接登记了邀请作废）；
    配置（只取得到自己的、签名照根公钥验、认证带着设备和序号、补齐到 1024、序号往回走的不存、设备手里的比存着的新也照给、还没加入的先存着）；
    名单（`seq`、`have`）；API key、折合金额照旧；请求的大小（`team_put` 1 MB，别的 64 KB）；
    移除、退出（配置和上报的记录跟着删）；换了密钥的设备重新登记；重启接着用、换了根公钥从头开始。
  - `test_records_lost`：状态文件丢了（根公钥照旧）以后 `gen` 变了，用过的邀请加不回来，管理员照留着的公钥登记回去、同一个序号再存一次配置。
  - `test_store`：直接调 `Store`（不用真的等）测过期、清理（邀请清掉时给它存的配置跟着删、只剩配置的编号也移除得掉）、邀请 / 配置 / 设备数的上限、
    手改坏了的状态文件、别的模式留下的状态文件。
  - `test_cli`：启动参数（根公钥从 `--team-root` 或者 `CCA_TEAM_ROOT` 来、只写后面那一段也认；托管的预设 `CCA_HUB_HOSTED=1` 下给了根公钥就是 Team 的服务器，
    没有 `/data` 不启动）。`test_behind_proxy`：只说 HTTP、不用证书，按 `X-Forwarded-For` 的最后一项限流，同一个 IP 的连接数不限。
  - 形状不对的请求直接调解析函数（`parse_enroll`、`parse_team_docs`）；`admin()` 遇到 429 等一会儿再试。
- `cargo test team`（`cmd/team/doc.rs`、`core/team.rs`）：编号、邀请链接、从 `k` 派生的令牌 / 设备编号 / 密钥、签的那一段字节、解 `tests/spec_wire.py`
  封的密文（`cli/tests/vectors/team_doc_ct.json`：它固定 nonce 封的），都和独立实现的那一份逐字节对得上；配置明文的检查。
- `tests/team.py`（回归的 unit 一节）：辅助程序对着真的 `--mode team` 服务器，各台设备各一个临时 HOME（`presence.Device`）。
  - `test_admin_local`：建 Team 只在本机、文件的权限、`k` 不落盘（照主密钥算得出来）、没接上服务器时导入和 `map` 只改本机、接不上的各种原因。
  - `test_lifecycle`：接上、导入、邀请（那串随机数和令牌不在管理员这台的任何文件、请求、curl 的命令行里，也不在服务器上）、加入（没成什么都不留、
    用过的邀请、已经在里面）、成员用不了管理员的命令、**独立实现的那一份拿邀请加入并解开辅助程序封的配置**、名单、改授权 / `map` 以后设备同步到新的一版、
    没变的不重新发布、移除（设备知道被移除了、拿新的邀请再加入）、退出（管理员那边记成 `left`）。
  - `test_spec_admin`：**管理员由独立实现的那一份扮演**：它封的、签的辅助程序解得开；不是根密钥签的、别的设备的那一份、内容不对的、比手里旧的都不要；
    配置里说服务器搬了跟着换，新的那台上没有这台设备是 `lost`。
  - `test_records_lost_and_move`：服务器的状态文件丢了以后管理员登记回去、重新发布（没用掉的邀请跟着没了）；换一台服务器，设备从原来那台上知道新的地址，
    没用掉的邀请在原来那台上撤回了；原来那台连不上时说一声。
  - `test_server_lies`：服务器把一台设备的公钥换了（停下来改它的状态文件）——记下的不照它改、名单里说一声；它那边的配置没了——重新发布；它乱说序号
    （假 curl 照 `~/reply/team_put` 回话：比记着的小、大得离谱、不是数）——不信、记着的序号不动；说得过去的只追一次；回话里没提到的设备不当成不在了。
  - 别的：没带认证码加入的设备（独立实现的那一头不带 `proof`）公钥不记、`problem` 说一声；index.json 没了也列得出来；team.json 坏了、管理员的密钥还在时
    拿这个 Team 的邀请来加入不删东西。
- regress 11b3d：`cca team` 的命令行一层。这台 Mac 先当管理员（成员由 `tm_spec`——照文档独立实现的那一头——扮演）：`new` 说下一步、还没接上时发不了邀请、
  `attach` 指纹对不上、`import`、`invite`（Claude Code 里不给；链接只一行打到 stdout，那串随机数不在 curl / env 的命令行里、不落盘）、`devices`、`grant`、
  `map`、`publish`、`node`（列出、粘贴链接加、复制、有账号在用的去不掉、`test` 撞了 IP 是 3 / 不通是 1 / 全都不通是 2、去掉；那一头的序号不动）、
  `revoke` 的同意事件和同意之后、管理员 `leave` 的同意事件（`team_delete`）；再当成员（管理员由 `tm_spec` 扮演）：链接少一截、加入
  （`team_joined`）、两个 Team 照先后列出、成员用不了 `team node`、`sync` 同步到新的一版并跟着改名、
  被移除以后 `sync` 说出来、`leave` 的同意事件和同意之后；换了版本以后的第一条命令（`update_after`：版本没换、全新安装的不提前；
  在 Team 里的马上同步到管理员刚发布的一版）。这一节结束时停掉两台服务器、删掉 `~/.cca/teams`。
  `team node test` 的假内核：假 mihomo 看到入站 `cca-test-1` 就把状态放在 `$T/core_test`（`core_load` 另记 `listeners/<端口>`：入站名和节点），
  假 nc 认它的端口；假 curl 经它时照 `$T/trace/<节点>/<网站>` 回话（默认服务那边 `203.0.113.<100+序号>`、www.cloudflare.com
  `198.51.100.250`），`$T/trace_down/<节点>` 时连不上；直连节点服务器的端口看 `$T/tcp_open`（「主机 端口」一行一个）。
- 节点（`tests/team.py` 的 `test_nodes`）：列出来不带密码、`accounts`；加（重名、不是对象、过不了 `fill_problems` 的不要）、复制（整份、
  模板不变）、去掉（账号、`dialer-proxy`、`install_node` 用着的不行）；没人用的节点不改成员设备的配置，`map` 到它以后才发出去（连同安装用的节点）；
  `testconfig` 的入站、带上的节点、权限、要测的网站、不写节点是全部；`verdict` 的几种结果和 `--json`；没接上服务器的 Team 只改本机。
  `cargo test nodes`：`judge` 的各种情况。
- 账号表照配置改（`tests/team.py` 的 `test_accounts_follow`）：加入以后 alice 的节点照 Team 的（说「按「测试组」的配置固定」）、补上别名，本来就有的
  账号只改节点、不记成 Team 加的；管理员多给一个账号加进来（记进 `team_added`），不再给了收起来（`parked`），再给回来照原来的键；不再给本来就有的
  那个，账号留着；被移除以后 Team 加的收起来。
  regress 11b3d：管理员自己这台 `grant … none`（导入的账号不进这台 Mac 的账号表），成员加入以后 alice 的节点由 Team 定、说一声；这一节结束时
  账号表和配置照开头的还原。`cmd/hub.rs` 的单元测试 `route_table`：账号走哪一类服务器看是谁定的。
- 别名（`tests/team.py` 的 `test_aliases`、`test_alias_per_tool`）：各工具各自从 `team-1` 编、同一个邮箱在两个工具里各有各的号（成员设备上各工具的
  账号表各用各的）、3.5.3-beta.5 的 Team 发布时照各工具重新编、成员设备上没有记录的旧编号跟着换、几个账号对调别名一次换好、`team-name` 改名字；
  导入时编号、再导入（先后变了、多了账号）编号不变、去掉的号不再用；两个 Team 都有 `team-1` 时后加入的
  那个在成员设备上叫「Team 的名字-1」；`team-alias` 的原因码（不合规、重名、没有这个账号、不是管理员）；改了以后成员设备上照 Team 给的用着的
  （连同撞了以后换成的）跟着改、自己改过的不跟；管理员这台设备上只有邮箱的不补、以前加进来的不跟；以前的 Team 发布时补上编号。
  `core/policy.rs` 的单元测试 `alias_fallback_names`：「Team 的名字-N」怎么拼、截短、拼不出来的时候。
- 同步经 Team 的服务器（`tests/team.py` 的 `test_pipelines`）：Team 给的账号记下 API key、同步折合金额、看设备，请求都是这台设备在 Team 里的
  密钥签的、账号摘要是照 Team 的盐算的；服务器上存的密文照文档（`spec_wire`）拿 Team 的盐解得开；别的服务器的状态文件没有；被移除以后
  不再发请求，留下的状态文件删掉了。
- 上报进程（`test_agent`，起真的 `presence-agent`）：没有会话开着时一次都不报、自己退出；开着时拿 Team 的密钥报；进程开着时加入第二个 Team
  （另一台服务器）接着就有一路，各报各的账号；被第一个 Team 的管理员移除以后那一路停了（再开会话也不报），另一路照旧。假 ps 的进程表里
  一行都没有时上报进程当成没问到（真的 ps 不会这样）：这条测试先放一个不相干的进程。
- 回归 11b3d：`_gui state` 里 Team 给的账号 `hub` 是 `team`、`team` 是编号，`config.hubs` 里有那个 Team，别的账号没有 `team`。
- 导入、新建时放进账号（`tests/team.py`）：`test_import_mine`：只加自己的官方账号。`test_new_with_accounts`：新建时能放的（自己加的、这台设备当管理员的
  Team 里的；成员的 Team、第三方 API 账号的不能）、写错了什么都不建、照账号表的先后放、别名带进去（默认样子的另编）、节点的定义连同 dialer 带过来、
  发布出去、`all`。
- 3.6.0 去掉映射以后（回归）：`cca policy …` 只说一句、退出码不是 0，`cca _policy-fetch` 什么都不做；升级上来的沙箱里留着 `~/.cca/policy/`、
  没有前缀的同步状态、`teams/adopt.*`、config.json 的三个开关时，第一条命令清掉它们（Team 的状态文件不动），只有共享开着、又不在 Team 里时
  说那一句；`_gui state` 的 `config.policy` 是那个常量、`can.policy_lock` 是 false；一键安装带着 `CCA_POLICY_REPO` 只提醒。
- 界面（`macos/Tests/ContractTests/TeamTests.swift`，手写的夹具 `hello.json`、`state.json`、`team_devices.json`）：解码（旧版 cca 没有
  这些字段、不认识的角色 / 状态）、筛选（三处用的列表、记进设置、Team 不在了当全部、旧版 cca 不筛）、展开时才问设备、邀请只经
  `copySecret`、改授权和节点发的命令。通用检查（`Generic.swift`）：`config.teams` 和账号的 `team` 对得上、Team 的服务器照编号算、
  `team_devices` / `team_list` 两种文档。回归 11b3d 把真实的 hello、带 Team 的 state、`team ls --json`、`team devices --json` 存进
  `$T/guifx4` 交给 `macos/test.sh` 解码。
- 部署（`tests/hub_deploy.py`：生成一次安装脚本，在 `tests/hub_box.py` 的假服务器上运行；回归的 unit 一节跑它，`tests/linux.sh` 在各个发行版的
  容器里也跑——那个发行版的 sh、openssl、python3）。假服务器 `Box`：临时目录当根（安装脚本的 `CCA_HUB_ROOT`，只给测试用），`bin/` 里 `id`、
  `useradd`、`userdel`、`chown`、`ip`、`journalctl`、`sudo` 是假的（`ufw`、`ss` 要的用例另放：`Box.fake`），`systemctl`、`docker` 是 Python
  写的（照 unit 的 `ExecStart`、compose.yaml 里各个服务的 `command`、端口和挂载起真的服务端，监听 127.0.0.1；证书助手换成 `acme_stub.py`：共用的
  那种照 `--teams` 里登记的地址一个地址一张自签的证书，存在 `--store`、放进各个 Team 的那一份，`store` 里没有、Team 的那一份里有就接着用；以前
  每个 Team 自己的那种照 `--id` 往 `--out` 里写一张；写过的记在 `acme.issued`；假的 `id -g` 说 `useradd` 时记下的组），`sh` 是 `/bin/dash`（没有时 `/bin/sh`）；每个服务各有各的进程号文件（`server.<服务名>.pid`，以前的 `cca-hub`
  是 `server.pid`）；目录下的几个文件让它出各种状况（`uid`、`sudo_ok`、`ip`、`listening`、`no_pull`、`docker_down`、`no_compose`、`compose_fail`、
  `start_fail`、`start_hang`、`acme_fail`、`ssh_denied`……，见它开头），等证书最多 4 秒（`CCA_HUB_CERT_WAIT`），`any_ip` 时内网地址也开网页端
  （`CCA_HUB_ANY_IP`）；`enter` 是假 ssh 的服务器那一头。
  - `InstallerTest`（在服务器上直接运行）：`test_script_itself`（`sh -n`、带着版本、服务端程序和证书助手；模板不能运行；参数、root 的检查）、
    `test_systemd`（装好、打出接上它的那一行，内网地址不开网页端、说一声；目录、权限、unit；再运行是更新；换端口再打一次那一行；`--uninstall`）、
    `test_web`（共用的证书助手 `cca-team-acme`：自己的系统用户、只给绑 80 的权限、目录和权限，照登记的地址要来证书、放进这个 Team 的那一份，
    服务端用它、标题带上 `--name`；再运行照旧；`--no-web` 去掉登记、没别的 Team 用了停掉它（证书留着）；80 被占着不开，写了 `--web-port`
    就停下；没要到证书时说放行 80；卸载最后一个 Team 连同证书助手删掉）、`test_web_two_teams`（两个 Team 共用一个证书助手、同一个地址只要一张、
    各是各的组和标题；卸载一个另一个照旧）、`test_web_old_helper`（另一个 Team 以前它自己的证书助手占着 80：`acme_old`、写了 `--web-port` 停下；
    更新那个 Team：停掉它自己的，证书、账户搬给共用的、接着用那张，之后另一个也开得了）、`test_web_skipped`（几种开不了网页端的情况照样装好；不开中转就没有网页端）、`test_docker`（第一个拉得到的镜像、
    用户 10001、项目名是服务名、共用的证书助手是另一个 compose 项目，这个 Team 只读挂它的那一份；改用 systemd 时这个 Team 的容器停掉、证书助手照旧是
    Docker 的；拉不到镜像改用 systemd，`--docker` 时停下）、
    `test_compose`（只有老的 docker-compose、`compose up` 起不来时改用 systemd，`--docker` 时停下、第一次装的 `.env` 不留；Docker 的卸载）、
    `test_dir`（别人的目录装、卸载都不碰，末尾的 `/` 去掉，同一个 Team 一台只装一个）、`test_failed_update_rolls_back`（起不来、被打断时 `.env`
    换回原来的）、`test_needs`（缺东西时停下什么都不动；看得到的公网 IP 照用）、`test_old_own_server`（`/opt/cca-hub` 只能卸载、不往里装）、
    `test_teams`（两个 Team 各一个服务、从 18445 起找端口、只写目录再运行是更新、中转的端口能换能关、另一个 Team 的目录不碰、卸载一个另一个照旧）。
  - `DeployTest`（`team-deploy`：管理员是 `presence.Device`，假 ssh 把命令交给假服务器上的 sh，`-G` 说在 127.0.0.1）：`test_deploy`（经 SSH 装好
    （Docker）、接上，发邀请、别的设备加入、同步到配置；再部署是更新；内网地址不开网页端；托管平台的部署文件）、`test_web`（`web_cert ok`、标题和
    manifest 带上 Team 的名字、`--no-web`、`wait`）、`test_args`（参数不对是 `usage`、不连服务器）、`test_unreachable_then_again`（接不上时说放行哪个端口，放行后再部署是更新；服务器自己的
    防火墙也说）、`test_not_root`（免密码的 sudo 能装，要密码时什么都不运行）、`test_dropped_after_start`（服务起来以后 SSH 断了：服务器上不换回，
    管理员照旧接着原来的，再部署是更新）、`test_login_check_and_password`（假 ssh `SSH_PW` 只认密码、经 `SSH_ASKPASS` 取，`SSH_SAYS` 照
    `$HUB_SSH_SAYS` 说一句就退出：`ssh_auth`、`ssh_denied`（不带 `BatchMode`）、`--check` 什么都不装、`ssh_keyonly`、密钥太多是 `ssh_auth`，`--batch`
    不带 `accept-new`，`--askpass` 只问一次密码、密码不在命令行和服务器上；只认公钥、主机密钥变了、解析不了、什么都没说各是哪个原因码）、
    `test_ssh_and_cert`（ssh 连不上、登录不了；服务器上的证书没了重新生成时照新的重新接上）。
- 服务端的托管预设、`--behind-proxy` 在 `tests/presence.py` 的 `TeamServerTest.test_cli`、`test_behind_proxy`；不写指纹的服务器（托管平台上的）
  照证书机构验在 `tests/team.py` 的 `test_ca_mode`。
- 回归 11b3d 的部署（`hubbox`：`tests/hub_box.py` 建的假服务器，假 ssh 认得 `hubbox`、`*@hubbox`，交给它的 `enter`；`$T/hubbox/ssh_denied` 时
  登录不了，`$T/hubbox/password` 在时不认密钥、只认这个密码；`nohub` 是解析不了的主机名）：`--script`（带着版本、两段程序和网页端的文件原样、
  `dash -n` 过得去、没有 `CCA_HUB_MANAGED`）；App 里新建（`team.new` → `team.accounts` → `team_created`：选项和 `team-candidates` 的一样多、自己加的预先选好；都不勾是空的 Team，勾了的连同节点
  放进去），终端里按编号选（没有的编号重问），没有终端时不问，`--accounts` 写了这台设备上没有的是 `team_accounts`、什么都不建；没写服务器、`--reset` 是 `team_usage`，端口写错了也是、
  不连服务器；登录不了 `team_ssh`；`cca team deploy` 装好（systemd，`team_lan` 在 `team_deployed` 前面，内网地址不开网页端，`.env` 里只有公开的
  根公钥）；App 里再部署（预填 127.0.0.1、root、22，说数据不变，先查登录 `SSH root@hubbox :: true`；`nohub` 带着 ssh 的原因重问，密钥不行问密码、
  经 askpass 部署，`prompts_hub_deploy.jsonl` 交给契约测试；错了三次 `hub_ssh`；只认密钥时生成密钥、给出加公钥那一行、「在终端里部署」的
  `terminal` 事件）；终端里不写服务器时问；`cca team hosted sealos`；照文档的那一行卸载、`team leave`。
