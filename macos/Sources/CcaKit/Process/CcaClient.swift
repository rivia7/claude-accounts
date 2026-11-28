import Foundation

/// 问用户同不同意（PROTOCOL 4.2）。App 里是 NSAlert；测试里按剧本回答。
protocol Prompter: Sendable {
    func confirm(_ consent: ConsentEvent) async -> Bool
}

/// 移除邮箱助手设置的范围（`cca mail forget`，PROTOCOL 4.11）：只移这个账号单独配的（改回默认）/ 只移默认（各账号单独配的留着）。
enum MailForgetScope: String, Sendable, Hashable {
    case account
    case `default`
}

/// 会改东西的 cca 命令（带事件文件、走重跑流程）。
enum CcaCommand: Sendable, Hashable {
    case usage(ToolID, key: String?)
    /// 自动刷新用量：只查到期的这几个账号，低优先级，最多同时查两个。
    case usagePoll(ToolID, keys: [String])
    case check(ToolID, key: String?)
    /// 自动检测出口：只测到期的这几个账号，低优先级。
    case checkPoll(ToolID, keys: [String])
    case app(ToolID, key: String)
    case web(ToolID, key: String)
    /// 控制台的账单页（`cca [codex] billing KEY`）：每月 API 额度和余额在那里看，检查和网页版一样。
    case billing(ToolID, key: String)
    /// claude.ai 的用量页（`cca limit-resets KEY`）：Claude 账号的重置次数和到期日在那里看，检查和网页版一样。
    case limitResets(ToolID, key: String)
    case routerSync
    /// 「更新并重新打开」的第一步：下载校验好和 cca 同版本的 App，先放着（PROTOCOL 5）。
    case guiUpdate
    /// 账号表按这个顺序排（`cca [codex] reorder 账号…`）：只改先后，端口、路由器都不动。
    case reorder(ToolID, keys: [String])
    /// 改别名（`cca [codex] rename KEY 新别名`，新别名是 - 时去掉别名）：stdout 是新的键。
    case rename(ToolID, key: String, alias: String)
    /// 重置额度（`cca codex reset KEY --json`，PROTOCOL 4.5）：要同意（reset_credit），stdout 是 kind=reset 的文档。
    case reset(ToolID, key: String)
    /// 「更新并重新打开」之前先把 cca 更新到这个版本（`cca update 版本号`，PROTOCOL 5.1）。
    case update(String)
    /// 检查更新（`cca update --check --json`，PROTOCOL 5.1）：只读、要联网，stdout 是 kind=update 的文档；查不了时看 fail 事件。
    case updateCheck
    /// 客户端（Claude、ChatGPT 桌面版，终端里的 Claude Code、Codex、Antigravity）更新：检查不安装，安装需经过绑定 token 的 consent。
    /// install：用户允许了自动更新（ClientUpdatesDoc.autoInstall），这次检查可能顺带装上：要跑完为止（runsToCompletion）。
    case clientCheck(force: Bool, install: Bool = false)
    case clientMode(automatic: Bool)
    /// 允许自动更新（`cca clients auto-install on|off --json`）：打开时连自动检查一起打开。
    case clientAutoInstall(enabled: Bool)
    case clientInstall(token: String)
    /// 账号连着的设备（`cca [codex] devices KEY --json`，PROTOCOL 3.4）：只读、要联网。auto 是账号页开着时的定时刷新（低优先级）。
    case devices(ToolID, key: String, auto: Bool)
    /// 这个工具里经服务器同步的全部官方账号连着的设备（`cca [codex|agy] devices --json`，不写账号）：总览用它标出别的设备上在用的账号。
    /// 定时、低优先级
    case devicesAll(ToolID)
    /// 读这个账号存的 API key（`cca [codex] key KEY --show`，PROTOCOL 4.10）：stdout 原样是 key。**输出只经内存里的管道**，
    /// 不能是 runsToCompletion（那种的 stdout 写进文件）；用完马上丢，不进被观察的状态、不进提示、不进读屏。
    case keyShow(ToolID, key: String)
    /// 删掉官方账号记下的 API key（`cca [codex] key KEY --forget`）：要同意（key_forget，默认不同意）。
    case keyForget(ToolID, key: String)
    /// 统计折合金额（`cca cost sync [--now] --json`，PROTOCOL 3.5）：不带工具前缀（一次统计 Claude 和 Codex），stdout 是 kind=cost_sync
    /// 的文档，金额本身在 `_gui state` 里。manual 是用户点的「刷新」：带 `--now`（上次同步失败了也不等重试的时间）、不降优先级；
    /// 定时的低优先级。全局一次一个。
    case costSync(manual: Bool)
    /// Claude App 里的 Code 会话开始时记不记下 App 登录的账号（`cca cost hook on|off`）：改共享的 ~/.claude/settings.json，
    /// 打开时 cca 先问同意（`consent.cost_hook`）。
    case costHook(on: Bool)
    /// 在账号的浏览器里打开某封登录邮件（`cca mail open KEY --msg 文件夹:uid:uidvalidity`，PROTOCOL 4.11）：
    /// cca 重新取这封、再核对、打开里面的链接；要跑完为止，进 `launch:` 闸门。mail 只给 Claude。
    case mailOpen(ToolID, key: String, msg: String)
    /// 移除邮箱助手设置（`cca mail forget`，PROTOCOL 4.11）：account 时 key 是账号的键，default 时 key 是 nil。
    case mailForget(ToolID, key: String?, scope: MailForgetScope)
    /// 关掉一个账号的「信任转发中转」（`cca mail trust-forwarder KEY off`，PROTOCOL 4.11）：快速改 config.json，emit `mail_ready`。
    /// 开启是不安全操作，走 `Handoff.mailTrustForwarder`（要问域名、要同意）；关闭没有风险，直接跑。
    case mailTrustForwarderOff(ToolID, key: String)
    /// 可以接入的会话和配对过的机器（`cca remote list [--local] --json`，PROTOCOL 9.1）：只读。local：只要这台 Mac 上的（不联网）；
    /// auto 是这一页开着时的定时刷新（低优先级）。和工具无关，没有前缀。
    case remoteList(local: Bool, auto: Bool)
    /// 远程会话的开关（`cca remote on|off`）。
    case remoteSwitch(on: Bool)
    /// 配对的那台机器能对这台 Mac 的会话做什么（`cca remote allow <配对的 id> control|view|none`）。
    case remoteAllow(peer: String, access: RemoteAccess)
    /// 取消配对（`cca remote unpair <配对的 id>`）。
    case remoteUnpair(peer: String)
    /// 结束这台 Mac 上的一个会话（`cca remote end <会话的 id>`，PROTOCOL 9.2）：等于关掉它的终端。cca 先发 `consent.remote_end`
    /// （范围是这个会话的 id），同意了重跑才结束；等会话真的结束了才返回。要 `hello.remote_end`。
    case remoteEnd(session: String)
    /// 远程启动用的模板（`cca remote template list --json`，PROTOCOL 9.4）：只读、不联网。要 `hello.remote_launch`。
    case remoteTemplates
    /// 存一个模板（`cca remote template add --name … --tool … --dir … [--team <Team 的编号>] [--replace <编号>] --args-line …`）：dir 是绝对路径，
    /// replace 是改的那一个的编号（新建是 nil），team 是归属的 Team（模板的文档里有 `teams` 时才给：旧版 cca 不认）。账号不在模板里
    /// （3.7.10 起：起会话时挑）。cca 校验（文件夹在不在，参数能不能起交互的会话，这台 Mac 在不在这个 Team 里），不行时 `fail` 说原因
    case remoteTemplateSave(name: String, tool: ToolID, dir: String, argsLine: String, replace: String?, team: String? = nil)
    /// 删掉一个模板（`cca remote template rm <编号>`）。
    case remoteTemplateRemove(id: String)
    /// 这台手机能不能用这台 Mac 上的模板新建会话（`cca remote allow <配对的 id> launch on|off`）：打开时 cca 先发 `consent.remote_launch`
    /// （范围是配对的 id），同意了重跑。
    case remoteLaunch(peer: String, on: Bool)
    /// 用配对过的另一台机器上的模板在那台机器上新建会话（`cca remote launch <配对的 id> <模板的编号> --digest <摘要> --size <列>x<行>
    /// [--account <账号的编号>] [--line <开场白>] --json`，PROTOCOL 9.5）：cca 进那台机器的启动器、等它起好、等新会话在服务器上登记好，
    /// stdout 是 `RemoteLaunchStartDoc`；那台机器不起时 `fail` 说原因。account：挑的账号（`peer_launchers[].accounts[].id`；那台机器上的
    /// cca 是旧版时是 nil）。要 `hello.remote_launch_machines`。
    case remoteStart(peer: String, template: String, digest: String, cols: Int, rows: Int, line: String?, account: String? = nil)
    /// 一个 Team 的设备和配置（`cca team devices <Team> --json`，PROTOCOL 4.14；只有管理员的设备问得到）：和服务器对一遍，可能发布。
    /// auto：这一页看得见时自己查的（低优先级）
    case teamDevices(team: String, auto: Bool)
    /// 各个 Team 同步一次配置（`cca team sync`）：同步到新的，账号表马上照它改、同步路由器
    case teamSync
    /// 退出一个 Team（`cca team leave <编号>`）：要同意（成员是 team_leave，管理员是 team_delete，范围是 Team 的编号，默认不同意）
    case teamLeave(team: String)
    /// 给一台设备发邀请（`cca team invite <Team> <名字> --accounts …`）：stdout 一行是邀请链接，它是那台设备的密钥。
    /// **输出只经内存里的管道**（不能是 runsToCompletion）
    case teamInvite(team: String, label: String, accounts: String)
    /// 移除一台设备、撤回邀请（`cca team revoke <Team> <设备编号>`）：要同意（team_revoke，范围是设备编号，默认不同意）
    case teamRevoke(team: String, device: String)
    /// 授予 / 收回一台设备的管理员身份（`cca team admin <Team> add|remove <设备编号>`，cca 3.6.15 起；只有建 Team 的设备能）：要同意
    /// （team_admin_add / team_admin_remove，范围是设备编号，默认不同意）
    case teamAdmin(team: String, device: String, add: Bool)
    /// 改一台设备能用哪些账号（`cca team grant <Team> <设备编号> all | none | +工具:邮箱 | -工具:邮箱 …`）
    case teamGrant(team: String, device: String, changes: [String])
    /// 给一台设备改名字（`cca team rename <Team> <设备编号> <新名字>`）：加入了的，它在服务器上的名字跟着改
    case teamRename(team: String, device: String, name: String)
    /// 定一个账号走哪个节点（`cca team map <Team> <工具> <邮箱> <节点>`；节点写 `-` 是从这个 Team 里去掉这个账号）
    case teamMap(team: String, tool: ToolID, email: String, node: String)
    /// 改一个账号在这个 Team 里的别名（`cca team alias <Team> <工具:邮箱> <新别名>`：别名按工具记，只改这个工具里的）
    case teamAlias(team: String, account: String, alias: String)
    /// 锁定（解开）这个 Team 里账号的别名，再发布（`cca team alias Team --lock|--unlock`）
    case teamAliasLock(team: String, on: Bool)
    /// Team 给的 Claude 账号信任哪些转发中转（`cca team forwarder <Team> <域名…>`，一个域名都没有时是 `off`：不再信任；cca 3.7.11 起，
    /// `team devices --json` 有 `mail_forwarders` 时才有）：比现在宽了要同意（team_forwarders，范围是 Team 的编号和新名单的摘要，默认不同意），
    /// 改完发布、这台设备马上同步一次、照它对一遍账号表
    case teamForwarders(team: String, domains: [String])
    /// 把这个 Team 的配置导出成 JSON 文件（`cca team export <Team> <文件> --force`，要 `hello.team_config`）：保存面板问过要不要换掉已有的，
    /// 所以总是带 --force；文件里有节点的密钥（cca 写成只有自己能读）
    case teamExport(team: String, path: String)
    /// 照文件整份换掉这个 Team 的配置（`cca team import <Team> <文件>`）：要同意（team_config，范围是 Team 的编号和文件的摘要；要去掉账号时默认不同意），
    /// 换好以后发布、账号表照它对一遍
    case teamImport(team: String, path: String)

    var tool: ToolID? {
        switch self {
        case .usage(let t, _), .usagePoll(let t, _), .check(let t, _), .checkPoll(let t, _), .app(let t, _), .web(let t, _), .billing(let t, _), .limitResets(let t, _), .rename(let t, _, _), .reorder(let t, _), .reset(let t, _), .devices(let t, _, _), .devicesAll(let t), .keyShow(let t, _), .keyForget(let t, _), .mailOpen(let t, _, _), .mailForget(let t, _, _), .mailTrustForwarderOff(let t, _): return t
        case .routerSync, .guiUpdate, .update, .updateCheck, .clientCheck, .clientMode, .clientAutoInstall, .clientInstall,
             .costSync, .costHook, .remoteList, .remoteSwitch, .remoteAllow, .remoteUnpair, .remoteEnd,
             .remoteTemplates, .remoteTemplateSave, .remoteTemplateRemove, .remoteLaunch, .remoteStart,
             .teamDevices, .teamSync, .teamLeave, .teamInvite, .teamRevoke, .teamAdmin, .teamGrant, .teamRename, .teamMap, .teamAlias, .teamAliasLock,
             .teamForwarders, .teamExport, .teamImport: return nil
        }
    }

    var key: String? {
        switch self {
        case .usage(_, let k), .check(_, let k): return k
        case .app(_, let k), .web(_, let k), .billing(_, let k), .limitResets(_, let k), .rename(_, let k, _), .reset(_, let k), .devices(_, let k, _), .keyShow(_, let k), .keyForget(_, let k), .mailOpen(_, let k, _), .mailTrustForwarderOff(_, let k): return k
        case .mailForget(_, let k, _): return k
        case .usagePoll, .checkPoll, .reorder, .devicesAll, .routerSync, .guiUpdate, .update, .updateCheck, .clientCheck, .clientMode,
             .clientAutoInstall, .clientInstall, .costSync, .costHook, .remoteList, .remoteSwitch, .remoteAllow, .remoteUnpair, .remoteEnd,
             .remoteTemplates, .remoteTemplateSave, .remoteTemplateRemove, .remoteLaunch, .remoteStart,
             .teamDevices, .teamSync, .teamLeave, .teamInvite, .teamRevoke, .teamAdmin, .teamGrant, .teamRename, .teamMap, .teamAlias, .teamAliasLock,
             .teamForwarders, .teamExport, .teamImport: return nil
        }
    }

    /// 后台的活：低优先级运行。
    var isBackground: Bool {
        switch self {
        case .usagePoll, .checkPoll, .clientCheck(force: false, install: _), .devices(_, _, auto: true), .devicesAll,
             .costSync(manual: false), .remoteList(local: _, auto: true), .teamDevices(team: _, auto: true): return true
        default: return false
        }
    }

    var extraEnvironment: [String: String] {
        // CCA_USAGE_POLL：这是后台定时刷新，值是 App 的进程号（cca 记下要同意、没查成，给手机上这台 Mac 的一栏；App 退出了就不算，PROTOCOL 3.1）
        if case .usagePoll = self { return ["CCA_USAGE_JOBS": "2", "CCA_USAGE_POLL": String(ProcessInfo.processInfo.processIdentifier)] }
        return [:]
    }

    /// `cca` 后面的参数（工具的前缀见 ToolID.defaultPrefix）。
    var arguments: [String] {
        let prefix = tool?.defaultPrefix ?? []
        switch self {
        case .usage(_, let k): return prefix + ["usage"] + (k.map { [$0] } ?? []) + ["--json"]
        case .usagePoll(_, let keys): return prefix + ["usage"] + keys + ["--json"]
        case .check(_, let k): return prefix + ["check"] + (k.map { [$0] } ?? []) + ["--json"]
        case .checkPoll(_, let keys): return prefix + ["check"] + keys + ["--json"]
        case .app(_, let k): return prefix + ["app", k]
        case .web(_, let k): return prefix + ["web", k]
        case .billing(_, let k): return prefix + ["billing", k]
        case .limitResets(_, let k): return prefix + ["limit-resets", k]
        case .routerSync: return ["router", "sync"]
        case .guiUpdate: return ["gui", "update"]
        case .rename(_, let k, let a): return prefix + ["rename", k, a]
        case .reorder(_, let keys): return prefix + ["reorder"] + keys
        case .reset(_, let k): return prefix + ["reset", k, "--json"]
        case .update(let v): return ["update", v]
        case .updateCheck: return ["update", "--check", "--json"]
        case .clientCheck(let force, _): return ["clients", "check"] + (force ? ["--force"] : []) + ["--json"]
        case .clientMode(let automatic): return ["clients", "mode", automatic ? "auto" : "off", "--json"]
        case .clientAutoInstall(let enabled): return ["clients", "auto-install", enabled ? "on" : "off", "--json"]
        case .clientInstall(let token): return ["clients", "install", token, "--json"]
        case .devices(_, let k, _): return prefix + ["devices", k, "--json"]
        case .devicesAll: return prefix + ["devices", "--json"]
        case .keyShow(_, let k): return prefix + ["key", k, "--show"]
        case .keyForget(_, let k): return prefix + ["key", k, "--forget"]
        case .costSync(let manual): return ["cost", "sync"] + (manual ? ["--now"] : []) + ["--json"]
        case .costHook(let on): return ["cost", "hook", on ? "on" : "off"]
        case .mailOpen(_, let k, let msg): return prefix + ["mail", "open", k, "--msg", msg]
        case .mailForget(_, let k, let scope):
            switch scope {
            case .account: return prefix + ["mail", "forget", "--account", k ?? ""]
            case .default: return prefix + ["mail", "forget", "--default"]
            }
        case .mailTrustForwarderOff(_, let k): return prefix + ["mail", "trust-forwarder", k, "off"]
        case .remoteList(let local, _): return ["remote", "list"] + (local ? ["--local"] : []) + ["--json"]
        case .remoteSwitch(let on): return ["remote", on ? "on" : "off"]
        case .remoteAllow(let peer, let access): return ["remote", "allow", peer, access.rawValue]
        case .remoteUnpair(let peer): return ["remote", "unpair", peer]
        case .remoteEnd(let session): return ["remote", "end", session]
        case .remoteTemplates: return ["remote", "template", "list", "--json"]
        case .remoteTemplateSave(let name, let tool, let dir, let argsLine, let replace, let team):
            // 参数是弹层里的那一行字，原样交给 cca 拆（照 shell 的写法；拆不开的 cca 说原因）：放在最后，没有参数也写。归属的 Team 给编号
            // （旧版 cca 不认 --team：那时 team 是 nil）
            return ["remote", "template", "add", "--name", name, "--tool", tool.rawValue, "--dir", dir]
                + (team.map { ["--team", $0] } ?? []) + (replace.map { ["--replace", $0] } ?? []) + ["--args-line", argsLine]
        case .remoteTemplateRemove(let id): return ["remote", "template", "rm", id]
        case .remoteLaunch(let peer, let on): return ["remote", "allow", peer, "launch", on ? "on" : "off"]
        case .remoteStart(let peer, let template, let digest, let cols, let rows, let line, let account):
            return ["remote", "launch", peer, template, "--digest", digest, "--size", "\(cols)x\(rows)"] + (account.map { ["--account", $0] } ?? [])
                + (line.map { ["--line", $0] } ?? []) + ["--json"]
        case .teamDevices(let team, _): return ["team", "devices", team, "--json"]
        case .teamSync: return ["team", "sync"]
        case .teamLeave(let team): return ["team", "leave", team]
        case .teamInvite(let team, let label, let accounts): return ["team", "invite", team, label, "--accounts", accounts]
        case .teamRevoke(let team, let device): return ["team", "revoke", team, device]
        case .teamAdmin(let team, let device, let add): return ["team", "admin", team, add ? "add" : "remove", device]
        case .teamGrant(let team, let device, let changes): return ["team", "grant", team, device] + changes
        case .teamRename(let team, let device, let name): return ["team", "rename", team, device, name]
        case .teamMap(let team, let tool, let email, let node): return ["team", "map", team, tool.rawValue, email, node]
        case .teamAlias(let team, let account, let alias): return ["team", "alias", team, account, alias]
        case .teamAliasLock(let team, let on): return ["team", "alias", team, on ? "--lock" : "--unlock"]
        case .teamForwarders(let team, let domains): return ["team", "forwarder", team] + (domains.isEmpty ? ["off"] : domains)
        case .teamExport(let team, let path): return ["team", "export", team, path, "--force"]
        case .teamImport(let team, let path): return ["team", "import", team, path]
        }
    }

    /// PROTOCOL 1 建议的超时（runsToCompletion 的命令不限时）。
    var timeout: TimeInterval {
        switch self {
        case .usage, .usagePoll: return 150
        case .check: return 180
        case .checkPoll: return 300
        case .app, .web, .billing, .limitResets, .mailOpen: return 120
        case .mailForget, .mailTrustForwarderOff: return 60
        case .routerSync: return 180
        case .guiUpdate, .update: return 300
        case .rename, .reorder: return 60
        case .reset, .updateCheck: return 150
        case .clientCheck, .clientInstall: return 900
        case .clientMode, .clientAutoInstall, .devices: return 30
        case .devicesAll: return 60   // 账号可能分在几个 Team 的服务器上：各问一次
        case .keyShow, .keyForget: return 40   // cca 读钥匙串最多等 30 秒
        case .costSync: return 300   // 第一次要把会话记录全读一遍
        case .costHook: return 60   // 改共享的设置前等 ~/.cca/sync.lock（别的命令同步账号时拿着）
        case .remoteList, .remoteAllow, .remoteUnpair: return 40   // 问一次服务器（签名、直连），取消配对被限流时还要再试两次
        case .remoteSwitch: return 30
        case .remoteEnd: return 40   // 里面的程序不理会时 cca 等宿主强行结束它（5 秒），再等它收场
        case .remoteTemplates, .remoteTemplateSave, .remoteTemplateRemove: return 30   // 只读写这台 Mac 上的文件
        case .remoteLaunch: return 40   // 改配对表，再照它装上 / 卸掉启动器（launchctl）
        case .remoteStart: return 100   // 问服务器、进启动器、等它回话（最多 25 秒），再等新会话登记好（最多 45 秒）
        case .teamDevices, .teamInvite, .teamRevoke, .teamAdmin, .teamRename: return 60   // 问 Team 的服务器，发布配置（设备多时分几次存）
        case .teamSync, .teamLeave, .teamGrant, .teamMap, .teamAlias, .teamAliasLock, .teamForwarders: return 180   // 改完账号表马上照它对一遍，可能同步路由器
        case .teamExport: return 30   // 只读管理员这边的配置、写一个文件
        case .teamImport: return 240   // 新的节点定义先拿内核校验（可能要先下载内核），再发布、对账号表
        }
    }

    /// 一旦开始就要跑完（PROTOCOL 1）：输出接文件，不限时，App 退出时也不杀。重置额度也是：同意之后就会用掉一次，
    /// 半路杀掉可能用掉了却没记下重置后的用量。
    var runsToCompletion: Bool {
        switch self {
        case .app, .web, .billing, .limitResets, .mailOpen, .routerSync, .reset, .clientInstall: return true
        case .clientCheck(_, let install): return install   // 允许自动更新时检查完可能马上替换客户端：半路杀掉要等下次检查才回滚
        case .usage, .usagePoll, .check, .checkPoll, .guiUpdate, .rename, .reorder, .update, .updateCheck, .clientMode,
             .clientAutoInstall, .devices, .devicesAll, .keyShow, .keyForget, .costSync, .costHook, .mailForget, .mailTrustForwarderOff,
             .remoteList, .remoteSwitch, .remoteAllow, .remoteUnpair, .remoteEnd, .remoteTemplates, .remoteTemplateSave, .remoteTemplateRemove, .remoteLaunch,
             .remoteStart, .teamDevices, .teamInvite, .teamRevoke, .teamAdmin, .teamRename, .teamExport: return false
        case .teamSync, .teamLeave, .teamGrant, .teamMap, .teamAlias, .teamAliasLock, .teamForwarders, .teamImport: return true   // 改账号表、同步路由器：半路杀掉会留下一半
        }
    }

    /// 同一个闸门里的命令一次只跑一个：同一个账号的 app / web；同一个工具的 usage；同一个账号的 reset；
    /// router sync 全局一个；更新 cca、gui update 全局一个（一个接着一个）。
    var gate: String? {
        switch self {
        case .app(let t, let k), .web(let t, let k), .billing(let t, let k), .limitResets(let t, let k), .rename(let t, let k, _), .mailOpen(let t, let k, _): return "launch:\(t.rawValue):\(k)"
        case .mailForget, .mailTrustForwarderOff: return nil   // 快速改 config.json：不占闸门
        case .usage(let t, _), .usagePoll(let t, _): return "usage:\(t.rawValue)"
        case .reorder(let t, _): return "reorder:\(t.rawValue)"   // 连着拖几次：一个接一个，最后拖的那次写在最后
        case .reset(let t, let k): return "reset:\(t.rawValue):\(k)"
        case .routerSync: return "router-sync"
        case .guiUpdate, .update: return "gui-update"
        case .clientCheck, .clientMode, .clientAutoInstall, .clientInstall: return "client-update"
        case .check, .checkPoll, .updateCheck, .devices: return nil   // devices：AppModel 已保证同一个账号一次一个
        case .devicesAll(let t): return "devices-all:\(t.rawValue)"
        case .keyShow(let t, let k), .keyForget(let t, let k): return "key:\(t.rawValue):\(k)"
        case .costSync: return "cost-sync"   // 定时的和手动的一个接一个
        case .costHook: return "cost-hook"
        case .remoteList, .remoteTemplates: return nil   // AppModel 保证一次一个
        case .remoteStart: return "remote-start"   // 在别的机器上起会话：一个接一个（那台机器的启动器 3 秒内只认同一台的一次），不和改配对表的排队
        case .remoteSwitch, .remoteAllow, .remoteUnpair, .remoteEnd, .remoteLaunch, .remoteTemplateSave, .remoteTemplateRemove:
            return "remote"   // 改配对表、开关、模板、结束会话：一个接一个
        case .teamDevices: return nil   // AppModel 保证一次一个
        case .teamInvite, .teamRevoke, .teamAdmin, .teamRename, .teamExport: return "team"   // 改管理员这边记着的设备、读它的配置：一个接一个
        case .teamSync, .teamLeave, .teamGrant, .teamMap, .teamAlias, .teamAliasLock, .teamForwarders, .teamImport: return "router-sync"   // 账号表跟着变，可能同步路由器：和同步路由器排队
        }
    }
}

/// 只读查询出错。
enum CcaError: Error, CustomStringConvertible, Equatable {
    case launch(String)
    case timedOut(TimeInterval)
    case cancelled
    case exit(Int32, stderrTail: String)
    case badOutput(String, stderrTail: String)
    case codexDisabled

    var description: String {
        switch self {
        case .launch(let s): return s
        case .timedOut(let t): return "cca 在 \(Int(t)) 秒内没有完成"
        case .cancelled: return "已取消"
        case .exit(let c, let tail): return tail.isEmpty ? "cca 出错了（退出码 \(c)）" : "cca 出错了（退出码 \(c)）：\(tail)"
        case .badOutput(let s, _): return s
        case .codexDisabled: return "未启用这种账号"
        }
    }
}

/// 失败的原因（给界面挑文案和补救按钮）。
enum FailureReason: Sendable, Equatable {
    /// cca 报的失败（最后一条 fail 事件，或者没有事件时的退出码）。
    case command
    /// 同一个授权被再次要求（PROTOCOL 4.3 第 6 条）。
    case loopGuard
    /// 重跑了 3 轮还要授权。
    case tooManyRounds
    case launch
    case timedOut
    case cancelled
    case codexDisabled
}

struct Failure: Sendable, Equatable {
    var reason: FailureReason
    /// 最后一条 fail 事件的 code。
    var code: FailCode?
    var message: String
    /// stderr 的最后几行（“详情”）。
    var stderrTail: String
    var exitCode: Int32?
    var tool: ToolID?
    var key: String?
}

enum ExecStatus: Sendable, Equatable {
    case success
    /// 用户没同意：停下，什么都没改（不是错误）。
    case refused(ConsentEvent)
    /// 要同意，但这次调用不弹框（例如后台自动刷新用量）。
    case needsConsent([ConsentEvent])
    /// 要在终端里完成。
    case terminal(TerminalEvent)
    case failed(Failure)
}

struct ExecResult: Sendable {
    var status: ExecStatus
    /// 最后一次运行的输出。
    var last: CcaResponse?
    var runs: Int
    var grants: [String]

    var resultEvent: ResultEvent? { last?.events.compactMap(\.result).last }
    var isSuccess: Bool { status == .success }
    var failure: Failure? { if case .failed(let f) = status { return f }; return nil }
}

/// 先来先得的异步锁。
actor AsyncGate {
    private var busy = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func acquire() async {
        if !busy {
            busy = true
            return
        }
        await withCheckedContinuation { waiters.append($0) }
    }

    func release() {
        if waiters.isEmpty {
            busy = false
        } else {
            waiters.removeFirst().resume()
        }
    }
}

/// 和 cca 打交道的唯一入口（PROTOCOL 全文）。
actor CcaClient {
    let transport: Transport
    /// 最多重跑几轮（PROTOCOL 4.3 第 6 条）。
    let maxReruns = 3
    private var gates: [String: AsyncGate] = [:]
    /// 能调用的工具（PROTOCOL 1）：Claude 一直能；`cca codex …`、`cca agy …` 只在 hello 说启用了时才能（不然会建出账号表）。
    private(set) var enabledTools: Set<ToolID> = [.claude]
    var codexEnabled: Bool { enabledTools.contains(.codex) }

    init(transport: Transport) {
        self.transport = transport
    }

    /// 按最新的 hello 更新：哪些工具的命令能调用。
    func setEnabledTools(_ tools: [ToolID]) {
        enabledTools = Set(tools).union([.claude])
    }

    /// 只改 Codex 的（测试用）。
    func setCodexEnabled(_ enabled: Bool) {
        if enabled { enabledTools.insert(.codex) } else { enabledTools.remove(.codex) }
    }

    private func gate(_ name: String) -> AsyncGate {
        if let g = gates[name] { return g }
        let g = AsyncGate()
        gates[name] = g
        return g
    }

    // MARK: 只读查询（PROTOCOL 2）

    func hello() async throws -> HelloDoc {
        try await query(HelloDoc.self, ["_gui", "hello"], timeout: 5)
    }

    func state() async throws -> StateDoc {
        try await query(StateDoc.self, ["_gui", "state"], timeout: 15)
    }

    func clientUpdates() async throws -> ClientUpdatesDoc {
        try await query(ClientUpdatesDoc.self, ["clients", "status", "--json"], timeout: 30)
    }

    /// route 为 nil 时按 route_pref；给了就用 CCA_ROUTE 看指定的一边。
    func live(route: Route? = nil) async throws -> LiveDoc {
        var extra: [String: String] = [:]
        if let route, route == .router || route == .local { extra["CCA_ROUTE"] = route.rawValue }
        return try await query(LiveDoc.self, ["_gui", "live"], timeout: 30, extra: extra)
    }

    /// 账号页「邮箱」一节的登录邮件列表（`cca _gui mail-list --account KEY [小时]`，PROTOCOL 2.4）：只读、要联网（连 Gmail，慢几秒）。
    /// 文档自带 `error`（没配置、连不上）；硬错误（崩溃、超时）才 throw。mail 只给 Claude，前缀固定是空的。
    func mailList(key: String, hours: Int = 24) async throws -> MailListDoc {
        try await query(MailListDoc.self, ["_gui", "mail-list", "--account", key, String(hours)], timeout: 45)
    }

    private func query<T: CcaDocument>(_ type: T.Type, _ args: [String], timeout: TimeInterval,
                                       extra: [String: String] = [:]) async throws -> T {
        let resp: CcaResponse
        do {
            resp = try await transport.send(CcaRequest(arguments: args, timeout: timeout, extraEnvironment: extra), onEvent: nil)
        } catch {
            throw CcaError.launch(String(describing: error))
        }
        let tail = resp.stderrText.lastLines(8)
        if resp.cancelled { throw CcaError.cancelled }
        if resp.timedOut { throw CcaError.timedOut(timeout) }
        guard resp.exitCode == 0 else { throw CcaError.exit(resp.exitCode, stderrTail: tail) }
        do {
            return try DocumentDecoder.decode(T.self, from: resp.stdout)
        } catch {
            throw CcaError.badOutput(String(describing: error), stderrTail: tail)
        }
    }

    // MARK: 提问会话（PROTOCOL 4.6）

    /// App 同一时间只开一个流程，不用闸门；不限时（用户答题要多久就多久）。
    nonisolated func startSession(_ arguments: [String]) throws -> CcaSession {
        try transport.startSession(arguments)
    }

    /// 界面里的终端（PROTOCOL 9.3）：`cca remote attach [-r] <会话的 id>` 跑在 App 开的伪终端上，不限时。
    nonisolated func startTerminal(_ arguments: [String], cols: Int, rows: Int,
                                   onEvent: @escaping @Sendable (PtyEvent) -> Void) throws -> TerminalHandle {
        try transport.startTerminal(arguments, cols: cols, rows: rows, onEvent: onEvent)
    }

    // MARK: 会改东西的命令（PROTOCOL 4）

    /// 没有 JSON 输出的命令（app、web、router sync、gui update）。
    func execute(_ command: CcaCommand, prompter: Prompter?,
                 onEvent: (@Sendable (CcaEvent) -> Void)? = nil) async -> ExecResult {
        await gated(command, prompter: prompter, decode: { _ in Bool?.none }, onEvent: onEvent).1
    }

    /// 输出 JSON 的命令（usage、check）：stdout 的 JSON 能解码就以它为准（例如 usage 退出码 1：没有一个账号有数据）。
    /// 每次运行只解码一次；返回最后一次运行解码出来的文档。
    func document<T: CcaDocument>(_ command: CcaCommand, as type: T.Type, prompter: Prompter?,
                                  onEvent: (@Sendable (CcaEvent) -> Void)? = nil) async -> (T?, ExecResult) {
        await gated(command, prompter: prompter, decode: { try? DocumentDecoder.decode(T.self, from: $0.stdout) }, onEvent: onEvent)
    }

    /// 同一个闸门里的命令排队（整个流程，包括弹框和重跑，都在闸门里）。
    private func gated<T: Sendable>(_ command: CcaCommand, prompter: Prompter?, decode: @escaping @Sendable (CcaResponse) -> T?,
                                    onEvent: (@Sendable (CcaEvent) -> Void)?) async -> (T?, ExecResult) {
        if let tool = command.tool, !enabledTools.contains(tool) {
            let f = Failure(reason: .codexDisabled, code: nil, message: "未启用 \(Texts.product(tool)) 账号", stderrTail: "",
                            exitCode: nil, tool: tool, key: command.key)
            return (nil, ExecResult(status: .failed(f), last: nil, runs: 0, grants: []))
        }
        guard let name = command.gate else {
            return await rounds(command, prompter: prompter, decode: decode, onEvent: onEvent)
        }
        let g = gate(name)
        await g.acquire()
        let r = await rounds(command, prompter: prompter, decode: decode, onEvent: onEvent)
        await g.release()
        return r
    }

    /// PROTOCOL 4.3 的重跑流程。prompter 为 nil 表示不弹框：要同意时返回 `.needsConsent`。
    private func rounds<T: Sendable>(_ command: CcaCommand, prompter: Prompter?, decode: @Sendable (CcaResponse) -> T?,
                                     onEvent: (@Sendable (CcaEvent) -> Void)?) async -> (T?, ExecResult) {
        var grants: [String] = []
        var runs = 0
        func result(_ status: ExecStatus, _ resp: CcaResponse?, _ doc: T?) -> (T?, ExecResult) {
            (doc, ExecResult(status: status, last: resp, runs: runs, grants: grants))
        }
        func fail(_ reason: FailureReason, _ message: String, _ resp: CcaResponse?, _ doc: T? = nil) -> (T?, ExecResult) {
            let f = Failure(reason: reason, code: resp?.events.compactMap(\.fail).last?.code, message: message,
                            stderrTail: resp?.stderrText.lastLines(12) ?? "", exitCode: resp?.exitCode,
                            tool: command.tool, key: command.key)
            return result(.failed(f), resp, doc)
        }
        while true {
            runs += 1
            let req = CcaRequest(arguments: command.arguments, timeout: command.timeout, wantsEvents: true, grants: grants,
                                 extraEnvironment: command.extraEnvironment, runsToCompletion: command.runsToCompletion, background: command.isBackground)
            let resp: CcaResponse
            do {
                resp = try await transport.send(req, onEvent: onEvent)
            } catch {
                return fail(.launch, "无法运行 cca：\(error)", nil)
            }
            let doc = decode(resp)
            if resp.cancelled { return fail(.cancelled, "已取消", resp, doc) }
            if resp.timedOut { return fail(.timedOut, "cca 在 \(Int(command.timeout)) 秒内没有完成，已停止", resp, doc) }
            if resp.exitCode == 0 { return result(.success, resp, doc) }
            // 同一个授权被再次要求：停下（防止死循环）
            var seen = Set<String>()
            let consents = resp.events.compactMap(\.consent).filter { seen.insert($0.grant).inserted }
            if let again = consents.first(where: { grants.contains($0.grant) }) {
                let what = again.title.isEmpty ? again.grant : again.title
                return fail(.loopGuard, "已经同意过「\(what)」，cca 仍然要求同意，已停止", resp, doc)
            }
            if !consents.isEmpty {
                guard let prompter else { return result(.needsConsent(consents), resp, doc) }
                if runs > maxReruns {
                    return fail(.tooManyRounds, "重跑了 \(maxReruns) 次仍然需要同意，已停止", resp, doc)
                }
                for c in consents {
                    if !(await prompter.confirm(c)) { return result(.refused(c), resp, doc) }
                }
                grants.append(contentsOf: consents.map(\.grant))
                continue
            }
            if let t = resp.events.compactMap(\.terminal).last { return result(.terminal(t), resp, doc) }
            if doc != nil { return result(.success, resp, doc) }
            let lastFail = resp.events.compactMap(\.fail).last
            return fail(.command, nonEmpty(lastFail?.message) ?? "cca 没有完成（退出码 \(resp.exitCode)）", resp)
        }
    }

    // MARK: 更新（PROTOCOL 5 的第二步）

    /// 新版本已经下载好之后：脱离 App 运行 `cca gui update --relaunch <pid>`（不设 CCA_EVENTS、不限时，输出接到文件）。
    /// 它等 App 退出后换上新版本再打开；App 随后自己退出。
    func relaunchAfterUpdate(pid: Int32) throws {
        try transport.spawnDetached(CcaRequest(arguments: ["gui", "update", "--relaunch", String(pid)], timeout: 300))
    }
}
