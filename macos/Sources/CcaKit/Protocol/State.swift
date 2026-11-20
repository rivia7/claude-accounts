import Foundation

/// 一个账号：工具 + 账号表里的键（PROTOCOL 1：两者一起才唯一）。
struct AccountRef: Hashable, Sendable, Decodable, CustomStringConvertible {
    var tool: ToolID
    var key: String

    init(tool: ToolID, key: String) {
        self.tool = tool
        self.key = key
    }

    enum CodingKeys: String, CodingKey { case tool, key }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let tool = c.lossy(ToolID.self, .tool), let key = c.string(.key), !key.isEmpty else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "account ref without tool/key"))
        }
        self.tool = tool
        self.key = key
    }

    var description: String { "\(tool.rawValue):\(key)" }
}

/// `cca _gui state`（PROTOCOL 2.2）。
struct StateDoc: CcaDocument, Equatable {
    static let expectedKind: String? = "state"

    var version: Int?
    var kind: String?
    var generatedAt: Double?
    var ccaVersion: String?
    var errors: [StateIssue]
    var notices: [StateNotice]
    var config: StateConfig?
    var routing: Routing?
    var tools: [ToolState]
    /// 所有账号的折合金额加起来（折合金额页的页头，PROTOCOL 2.2）：关掉了、没有 Claude / Codex 的官方账号时为 nil，旧版 cca 没有
    var costTotal: CostTotal?

    enum CodingKeys: String, CodingKey {
        case version, kind
        case generatedAt = "generated_at"
        case ccaVersion = "cca_version"
        case errors, notices, config, routing, tools
        case costTotal = "cost_total"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = c.int(.version)
        kind = c.string(.kind)
        generatedAt = c.double(.generatedAt)
        ccaVersion = c.string(.ccaVersion)
        errors = c.lossyArray(StateIssue.self, .errors) ?? []
        notices = c.lossyArray(StateNotice.self, .notices) ?? []
        config = c.lossy(StateConfig.self, .config)
        routing = c.lossy(Routing.self, .routing)
        tools = c.lossyArray(ToolState.self, .tools) ?? []
        costTotal = c.lossy(CostTotal.self, .costTotal)
    }

    func tool(_ id: ToolID) -> ToolState? { tools.first { $0.id == id } }

    func account(_ ref: AccountRef) -> Account? { tool(ref.tool)?.accounts.first { $0.key == ref.key } }

    var allAccounts: [Account] { tools.flatMap(\.accounts) }
}

/// `errors[]`（文件读不了）和 `tools[].errors[]`（账号表校验不过）。
struct StateIssue: Decodable, Sendable, Equatable {
    var code: StateErrorCode
    var tool: ToolID?
    var message: String

    enum CodingKeys: String, CodingKey { case code, tool, message }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        code = c.lossy(StateErrorCode.self, .code) ?? StateErrorCode(rawValue: "")
        tool = c.lossy(ToolID.self, .tool)
        message = c.string(.message) ?? ""
    }
}

/// `notices[]`：只读对账时“本来会做”的改动。
struct StateNotice: Decodable, Sendable, Equatable {
    var code: NoticeCode
    var tool: ToolID?
    var key: String?
    var oldKey: String?
    var node: String?
    var oldNode: String?

    enum CodingKeys: String, CodingKey {
        case code, tool, key
        case oldKey = "old_key"
        case node
        case oldNode = "old_node"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        code = c.lossy(NoticeCode.self, .code) ?? NoticeCode(rawValue: "")
        tool = c.lossy(ToolID.self, .tool)
        key = c.string(.key)
        oldKey = c.string(.oldKey)
        node = c.string(.node)
        oldNode = c.string(.oldNode)
    }
}

struct StateConfig: Decodable, Sendable, Equatable {
    var routePref: RoutePref?
    var router: RouterConfig?
    var local: LocalConfig?
    var nodes: [String]
    var minTrust: Double?
    var leakCheck: Bool?
    var update: String?
    var policy: PolicyConfig?
    /// 这台机器走过配置向导、或者已经有账号/路由器/本机客户端（老版 cca 没有这个字段 → 当作已配置，不弹向导）。
    var onboarded: Bool?

    enum CodingKeys: String, CodingKey {
        case routePref = "route_pref"
        case router, local, nodes
        case minTrust = "min_trust"
        case leakCheck = "leak_check"
        case update, policy, onboarded
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        routePref = c.lossy(RoutePref.self, .routePref)
        router = c.lossy(RouterConfig.self, .router)
        local = c.lossy(LocalConfig.self, .local)
        nodes = c.lossyArray(String.self, .nodes) ?? []
        minTrust = c.double(.minTrust)
        leakCheck = c.bool(.leakCheck)
        update = c.stringish(.update)
        policy = c.lossy(PolicyConfig.self, .policy)
        onboarded = c.bool(.onboarded)
    }

    var routerConfigured: Bool { router?.configured ?? false }
    var localConfigured: Bool { local?.configured ?? false }
    /// 配置向导还要不要出现：只有 cca 明确说 false 时才要（字段缺省、或 true 都不弹）。
    var needsOnboarding: Bool { onboarded == false }
}

struct RouterConfig: Decodable, Sendable, Equatable {
    var configured: Bool
    var host: String?
    var sshPort: Int?
    var proxyHost: String?

    enum CodingKeys: String, CodingKey {
        case configured, host
        case sshPort = "ssh_port"
        case proxyHost = "proxy_host"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        configured = c.bool(.configured) ?? false
        host = c.string(.host)
        sshPort = c.int(.sshPort)
        proxyHost = c.string(.proxyHost)
    }
}

struct LocalConfig: Decodable, Sendable, Equatable {
    var configured: Bool
    var client: LocalClient?
    var name: String?
    /// 本机 UDP 走节点（`cca local udp`，PROTOCOL 7.1）。只在 macOS 上有；Linux 上、旧版 cca 是 nil。
    var udp: LocalUDP?

    enum CodingKeys: String, CodingKey { case configured, client, name, udp }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        configured = c.bool(.configured) ?? false
        client = c.lossy(LocalClient.self, .client)
        name = c.string(.name)
        udp = c.lossy(LocalUDP.self, .udp)
    }
}

/// `config.local.udp`：这台 Mac 自己的 UDP（浏览器的 WebRTC、QUIC）经不经节点出去。内置代理只管账号的端口，这些平时直连；
/// 打开后一个 root 的系统服务（TUN）只把 UDP 交给设的节点。状态由 cca 判断，App 只照着显示。
struct LocalUDP: Decodable, Sendable, Equatable {
    /// 设的节点；没开是 nil
    var node: String?
    var state: LocalUDPState
    /// root 的系统服务装着（不管开没开）
    var installed: Bool
    /// root 的内核在跑
    var running: Bool
    /// 别的 VPN、代理客户端的 TUN 接管了这台 Mac 的流量，root 那边先让给了它：那块虚拟网卡的名字（如 `utun5`）。没让开、旧版 cca 是 nil
    var otherVPN: String?
    /// 在家暂停着（路由器已经让 UDP 出了大陆，出门在外时 cca 自动再打开）：`ok` 却没在跑也是正常的。旧版 cca 没有，当作 false
    var paused: Bool

    enum CodingKeys: String, CodingKey { case node, state, installed, running, otherVPN = "other_vpn", paused }

    init(node: String?, state: LocalUDPState, installed: Bool, running: Bool, otherVPN: String? = nil, paused: Bool = false) {
        self.node = node
        self.state = state
        self.installed = installed
        self.running = running
        self.otherVPN = otherVPN
        self.paused = paused
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        node = c.string(.node)
        state = c.lossy(LocalUDPState.self, .state) ?? LocalUDPState(rawValue: "")
        installed = c.bool(.installed) ?? false
        running = c.bool(.running) ?? false
        otherVPN = c.string(.otherVPN)
        paused = c.bool(.paused) ?? false
    }

    /// 开着、root 的内核在跑：UDP 正经节点出去
    var active: Bool { state == .ok && running }
    /// 开着、给别的 VPN 让开了：UDP 照它的规则出去，它关掉后 cca 自动接回节点（不用做什么）
    var yielded: Bool { state == .ok && otherVPN != nil }
}

/// 维护者共享的账号（维护者的映射，默认关着；PROTOCOL 2.2 的 `config.policy`、4.12）
struct PolicyConfig: Decodable, Sendable, Equatable {
    /// 共享开着（记着映射仓库）
    var configured: Bool
    var repo: String?
    /// App 下次该拉映射的时间（cca 算的，App 不重算）；没拉过是 nil。
    var nextCheckEpoch: Double?
    /// cca 报的状态（`state`）；旧版 cca 没有这一项
    var reported: PolicyState?
    /// 账号表里属于映射的账号（各工具加起来）；旧版 cca 是 0
    var accounts: Int
    /// 关掉共享时移出账号表的账号（登录都留着，打开时原样加回来）；旧版 cca 是 0
    var parked: Int

    enum CodingKeys: String, CodingKey {
        case configured, repo, accounts, parked
        case reported = "state"
        case nextCheckEpoch = "next_check_epoch"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        configured = c.bool(.configured) ?? false
        repo = c.string(.repo)
        nextCheckEpoch = c.double(.nextCheckEpoch)
        reported = c.lossy(PolicyState.self, .reported)
        accounts = c.int(.accounts) ?? 0
        parked = c.int(.parked) ?? 0
    }

    /// 关着 / 打开了还没拉到映射 / 拉到了。旧版 cca 没有 `state`：照 `configured` 当作开着（`on`）或关着（`off`）
    var state: PolicyState { reported ?? (configured ? .on : .off) }
    /// cca 有「共享与同步」的开关（`cca policy on` / `off`）：新版 cca 的这一项带 `state`，旧版没有
    var switchable: Bool { reported != nil }
    /// 共享开着（拉到了映射，或者在等维护者）：开关画成打开的
    var isOn: Bool { state != .off }
}

struct Routing: Decodable, Sendable, Equatable {
    var firstPort: Int?
    var poolSize: Int?
    var sync: SyncState
    var localSync: SyncState
    var slots: [Slot]
    var nodes: [NodeUse]

    enum CodingKeys: String, CodingKey {
        case firstPort = "first_port"
        case poolSize = "pool_size"
        case sync
        case localSync = "local_sync"
        case slots, nodes
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        firstPort = c.int(.firstPort)
        poolSize = c.int(.poolSize)
        sync = c.lossy(SyncState.self, .sync) ?? .notApplicable
        localSync = c.lossy(SyncState.self, .localSync) ?? .notApplicable
        slots = (c.lossyArray(Slot.self, .slots) ?? []).sorted { $0.slot < $1.slot }
        nodes = c.lossyArray(NodeUse.self, .nodes) ?? []
    }

    func slot(_ n: Int) -> Slot? { slots.first { $0.slot == n } }
}

/// 端口池的一个槽位。`tool` / `key` 为 null 的是空槽位。
struct Slot: Decodable, Sendable, Equatable, Identifiable {
    var slot: Int
    var group: String?
    var port: Int?
    var tool: ToolID?
    var key: String?
    var node: String?
    var localNode: String?

    var id: Int { slot }

    enum CodingKeys: String, CodingKey {
        case slot, group, port, tool, key, node
        case localNode = "local_node"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let n = c.int(.slot) else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "slot without number"))
        }
        slot = n
        group = c.string(.group)
        port = c.int(.port)
        tool = c.lossy(ToolID.self, .tool)
        key = c.string(.key)
        node = c.string(.node)
        localNode = c.string(.localNode)
    }

    var isEmpty: Bool { tool == nil || key == nil }

    var ref: AccountRef? {
        guard let tool, let key else { return nil }
        return AccountRef(tool: tool, key: key)
    }
}

/// 可选节点和用它的账号。
struct NodeUse: Decodable, Sendable, Equatable, Identifiable {
    var name: String
    var localName: String?
    var users: [AccountRef]

    var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name
        case localName = "local_name"
        case users
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let name = c.string(.name) else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "node without name"))
        }
        self.name = name
        localName = c.string(.localName)
        users = c.lossyArray(AccountRef.self, .users) ?? []
    }
}

/// 一个工具（Claude / Codex）。
/// `tools[].features`：这个工具有没有桌面 App（`app`）、第三方 API 账号（`api`）、查用量的接口（`usage`）、折合金额（`cost`）。
/// 旧版 cca 没有这个字段：那时只有 Claude 和 Codex，都有（折合金额除外：没有这个键当作没有）。
struct ToolFeatures: Decodable, Sendable, Equatable {
    var app: Bool
    var api: Bool
    var usage: Bool
    /// 折合金额（PROTOCOL 2.2）：旧版 cca 没有这个键，当作没有
    var cost: Bool

    enum CodingKeys: String, CodingKey { case app, api, usage, cost }

    init(app: Bool, api: Bool, usage: Bool, cost: Bool = false) {
        self.app = app
        self.api = api
        self.usage = usage
        self.cost = cost
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        app = c.bool(.app) ?? true
        api = c.bool(.api) ?? true
        usage = c.bool(.usage) ?? true
        cost = c.bool(.cost) ?? false
    }

    /// cca 没给 features 时：Antigravity 什么都没有，别的都有（折合金额都没有）
    static func defaults(_ id: ToolID) -> ToolFeatures {
        id == .agy ? ToolFeatures(app: false, api: false, usage: false) : ToolFeatures(app: true, api: true, usage: true)
    }
}

struct ToolState: Decodable, Sendable, Equatable, Identifiable {
    var id: ToolID
    var label: String?
    var appLabel: String?
    var cliPrefix: [String]
    var enabled: Bool
    var bin: String?
    var app: AppInstall?
    var chrome: Bool
    var settingsProblems: [String]
    var errors: [StateIssue]
    var last: String?
    var accounts: [Account]
    /// 这个工具有没有桌面 App、第三方 API 账号、查用量的接口（Antigravity 都没有）
    var features: ToolFeatures

    enum CodingKeys: String, CodingKey {
        case id, label
        case appLabel = "app_label"
        case cliPrefix = "cli_prefix"
        case enabled, bin, app, chrome
        case settingsProblems = "settings_problems"
        case errors, last, accounts, features
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let id = c.lossy(ToolID.self, .id), !id.rawValue.isEmpty else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "tool without id"))
        }
        self.id = id
        label = c.string(.label)
        appLabel = c.string(.appLabel)
        cliPrefix = c.lossyArray(String.self, .cliPrefix) ?? id.defaultPrefix
        enabled = c.bool(.enabled) ?? (id == .claude)
        bin = c.string(.bin)
        app = c.lossy(AppInstall.self, .app)
        chrome = c.bool(.chrome) ?? true
        settingsProblems = c.lossyArray(String.self, .settingsProblems) ?? []
        features = c.lossy(ToolFeatures.self, .features) ?? .defaults(id)
        errors = c.lossyArray(StateIssue.self, .errors) ?? []
        last = c.string(.last)
        accounts = (c.lossyArray(Account.self, .accounts) ?? []).map { a in
            var a = a
            a.tool = id
            return a
        }
    }

    /// 账号表校验不过（有它时启动会失败，App 提示在终端里 `cca [codex] edit`）。
    var tableInvalid: [StateIssue] { errors.filter { $0.code == .tableInvalid } }
}

struct AppInstall: Decodable, Sendable, Equatable {
    var installed: Bool
    var path: String?

    enum CodingKeys: String, CodingKey { case installed, path }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        installed = c.bool(.installed) ?? false
        path = c.string(.path)
    }
}

/// 一个账号（`tools[].accounts[]`）。
struct Account: Decodable, Sendable, Equatable, Identifiable {
    /// 所属工具：解码后由 ToolState 填上（账号对象里没有这个字段）。
    var tool: ToolID = .claude
    var key: String
    var accountID: String?
    var alias: String?
    var email: String?
    var name: String?
    var label: String?
    var kind: AccountKind
    var login: LoginState
    var loginEmail: String?
    var plan: String?
    /// 套餐档位（cca 算的；旧版 cca 没有这个字段时是 .unknown）
    var planTier: PlanTier
    var node: String?
    var port: Int?
    var slot: Int?
    var group: String?
    var pinned: Bool
    var pinnedNode: String?
    var isLast: Bool
    var api: APIInfo?
    var badge: Badge?
    var app: AppRun?
    /// 经 cca 启动、还在运行的命令行会话（旧版 cca 没有这个字段时是 nil：当作没有会话）
    var cli: CLIRun?
    var can: Can
    var canWhy: [String: CanWhy]
    /// 邮箱助手状态（账号页的「邮箱」一节，PROTOCOL 2.2、4.11）：只给 `can.mail` 为真的账号，其余（含旧版 cca）是 nil
    var mail: MailConfig?
    /// 存没存 API key、掩码（PROTOCOL 2.2；旧版 cca 没有，是 nil）
    var apiKey: APIKeyInfo?
    var usage: UsageEntry?
    /// 最近几周每周用了多少（PROTOCOL 2.2；cca 记下的，官方账号才有；旧版 cca 没有，是 nil）
    var usageWeeks: UsageWeeks?
    /// 折合金额（PROTOCOL 2.2；只给 `can.cost` 为真的账号，还没有任何记录、旧版 cca 是 nil）
    var cost: AccountCost?
    var health: Health?

    var id: AccountRef { ref }
    var ref: AccountRef { AccountRef(tool: tool, key: key) }

    enum CodingKeys: String, CodingKey {
        case key
        case accountID = "id"
        case alias, email, name, label, kind, login
        case loginEmail = "login_email"
        case plan
        case planTier = "plan_tier"
        case node, port, slot, group, pinned
        case pinnedNode = "pinned_node"
        case isLast = "is_last"
        case api, badge, app, cli, can, mail
        case canWhy = "can_why"
        case apiKey = "api_key"
        case usage
        case usageWeeks = "usage_weeks"
        case cost
        case health
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let key = c.string(.key), !key.isEmpty else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "account without key"))
        }
        self.key = key
        accountID = c.string(.accountID)
        alias = c.string(.alias)
        email = c.string(.email)
        name = c.string(.name)
        label = c.string(.label)
        kind = c.lossy(AccountKind.self, .kind) ?? .oauth
        login = c.lossy(LoginState.self, .login) ?? LoginState(rawValue: "")
        loginEmail = c.string(.loginEmail)
        plan = c.string(.plan)
        planTier = c.lossy(PlanTier.self, .planTier) ?? .unknown
        node = c.string(.node)
        port = c.int(.port)
        slot = c.int(.slot)
        group = c.string(.group)
        pinned = c.bool(.pinned) ?? false
        pinnedNode = c.string(.pinnedNode)
        isLast = c.bool(.isLast) ?? false
        api = c.lossy(APIInfo.self, .api)
        badge = c.lossy(Badge.self, .badge)
        app = c.lossy(AppRun.self, .app)
        cli = c.lossy(CLIRun.self, .cli)
        can = c.lossy(Can.self, .can) ?? Can()
        canWhy = c.lossyDictionary(CanWhy.self, .canWhy) ?? [:]
        mail = c.lossy(MailConfig.self, .mail)
        apiKey = c.lossy(APIKeyInfo.self, .apiKey)
        usage = c.lossy(UsageEntry.self, .usage)
        usageWeeks = c.lossy(UsageWeeks.self, .usageWeeks)
        cost = c.lossy(AccountCost.self, .cost)
        health = c.lossy(Health.self, .health)
    }

    var isAPI: Bool { kind == .api }
    /// App 记排序、置顶时认账号用：工具 + 账号目录名（改别名不变）。
    var stableID: String { tool.rawValue + ":" + (accountID ?? key) }
    /// 显示用的名字：别名，没有别名就是邮箱（cca 算好的 `name`）。
    var displayName: String { nonEmpty(name) ?? nonEmpty(alias) ?? nonEmpty(email) ?? key }
    var displayLabel: String { nonEmpty(label) ?? displayName }
    var appRunning: Bool { app?.running ?? false }
    /// 经 cca 启动的命令行会话有几个还在运行
    var cliSessions: Int { cli?.running == true ? cli?.sessions ?? 0 : 0 }
    /// 最早那个命令行会话开始的时间（没有会话时 nil）
    var cliSince: Double? { cliSessions > 0 ? cli?.sinceEpoch : nil }
    /// 在用：桌面 App 开着，或者有经 cca 启动的命令行会话（总览里的“在用”、头像右下角的绿点）
    var isWorking: Bool { appRunning || cliSessions > 0 }
    /// 最高档的套餐（Ultra、Pro 500）。第三方 API 账号的档位总是空的
    var isPremium: Bool { planTier == .premium }
    /// 走节点（有端口）。直连的第三方 API 账号没有。
    var isRouted: Bool { port != nil }
}

struct APIInfo: Decodable, Sendable, Equatable {
    var host: String?
    var model: String?

    enum CodingKeys: String, CodingKey { case host, model }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        host = c.string(.host)
        model = c.string(.model)
    }
}

/// `api_key`（PROTOCOL 2.2）：这个账号存没存 API key。官方账号的是记下备查的（cca 不拿它启动），第三方 API 账号的是启动要用的。
/// `hint` 是掩码（`sk-ant-…x7Qa`；短的 key 没有，是空的），存的时候 cca 算好的，**不是** key；明文只在点「显示」「复制」时
/// 经管道向 cca 要（`cca [codex] key 账号 --show`）。
/// 官方账号记下的那一把可能经设备统计的服务器加密同步给登录了这个账号的其他设备（`shared`）：`fromDevice` 是从哪台设备同步来的，
/// `savedEpoch` 是什么时候记下的，`syncError` 是最近一次没同步成的原因（都是 cca 给的；旧版 cca 没有这几个字段：当作不同步）。
struct APIKeyInfo: Decodable, Sendable, Equatable {
    var saved: Bool
    var hint: String
    var source: APIKeySource
    var shared = false
    var savedEpoch: Double?
    var fromDevice: String?
    var syncError: String?

    enum CodingKeys: String, CodingKey {
        case saved, hint, source, shared
        case savedEpoch = "saved_epoch", fromDevice = "from_device", syncError = "sync_error"
    }

    init(saved: Bool, hint: String, source: APIKeySource, shared: Bool = false, savedEpoch: Double? = nil,
         fromDevice: String? = nil, syncError: String? = nil) {
        self.saved = saved
        self.hint = hint
        self.source = source
        self.shared = shared
        self.savedEpoch = savedEpoch
        self.fromDevice = fromDevice
        self.syncError = syncError
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        saved = c.bool(.saved) ?? false
        hint = c.string(.hint) ?? ""
        source = c.lossy(APIKeySource.self, .source) ?? APIKeySource(rawValue: "")
        shared = c.bool(.shared) ?? false
        savedEpoch = c.double(.savedEpoch)
        fromDevice = c.string(.fromDevice).flatMap { $0.isEmpty ? nil : $0 }
        syncError = c.string(.syncError).flatMap { $0.isEmpty ? nil : $0 }
    }
}

struct Badge: Decodable, Sendable, Equatable {
    var label: String
    var color: String

    enum CodingKeys: String, CodingKey { case label, color }

    init(label: String, color: String) {
        self.label = label
        self.color = color
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        label = c.string(.label) ?? ""
        color = c.string(.color) ?? ""
    }

    /// `#RRGGBB` → (r, g, b) 0…1；格式不对返回 nil。
    var rgb: (Double, Double, Double)? {
        var s = color.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("#") { s.removeFirst() }
        guard s.count == 6, let v = UInt32(s, radix: 16) else { return nil }
        return (Double((v >> 16) & 0xFF) / 255, Double((v >> 8) & 0xFF) / 255, Double(v & 0xFF) / 255)
    }
}

struct AppRun: Decodable, Sendable, Equatable {
    var running: Bool
    var pid: Int?

    enum CodingKeys: String, CodingKey { case running, pid }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        running = c.bool(.running) ?? false
        pid = c.int(.pid)
    }
}

/// `cli`：经 cca 启动、还在运行的命令行会话（cca 核对过进程，App 不判活）。
struct CLIRun: Decodable, Sendable, Equatable {
    var running: Bool
    var sessions: Int
    /// 最早那个会话开始的时间（Unix 秒）
    var sinceEpoch: Double?

    enum CodingKeys: String, CodingKey {
        case running, sessions
        case sinceEpoch = "since_epoch"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let n = max(c.int(.sessions) ?? 0, 0)
        running = c.bool(.running) ?? (n > 0)
        sessions = running ? max(n, 1) : 0   // 在运行就至少一个
        sinceEpoch = c.double(.sinceEpoch)
    }
}

/// `can`：按钮可不可用。`check` 总是可用；缺的项当作不可用（由 cca 决定，App 不猜）。
struct Can: Decodable, Sendable, Equatable {
    var app: Bool = false
    var web: Bool = false
    /// 能不能打开控制台的账单页（每月 API 额度和余额在那里看；旧版 cca 没有这一项，当作不能）
    var billing: Bool = false
    /// 能不能打开 claude.ai 的用量页（Claude 账号的重置次数和到期日在那里看；旧版 cca 没有这一项，当作不能）
    var limitResets: Bool = false
    var usage: Bool = false
    var check: Bool = true
    /// 能不能在终端里打开（要有 claude / codex 命令）
    var cli: Bool = false
    /// 能不能请求维护者把节点锁进映射（官方账号、这台机器配置了映射、还没被映射固定；PROTOCOL 4.8）
    var policyLock: Bool = false
    /// 账号页有没有「设备」一节（映射里的官方账号、映射里配置了设备统计的服务器、这台 Mac 没关掉；PROTOCOL 3.4）。
    /// 旧版 cca 没有这一项，当作没有
    var devices: Bool = false
    /// 能不能记下 / 显示 / 复制 API key（`cca [codex] key`，PROTOCOL 4.10；旧版 cca 没有这一项，当作不能）
    var apiKey: Bool = false
    /// 账号页有没有「折合金额」一节、折合金额页里有没有这一行（Claude、Codex 的官方账号，没登录的也有；
    /// PROTOCOL 2.2）。旧版 cca 没有这一项，当作没有
    var cost: Bool = false
    /// 账号页有没有「邮箱」一节（邮箱助手，PROTOCOL 4.11；只有 Claude 官方账号、账号表里写了邮箱）。
    /// 旧版 cca 没有这一项，当作没有
    var mail: Bool = false
    /// 能不能给这个账号换上自定义代理（`cca [codex|agy] node use`，PROTOCOL 4.6 的 `node.custom`；维护者的映射固定了节点的官方账号不能）。
    /// 旧版 cca 没有这一项，当作不能
    var customProxy: Bool = false

    enum CodingKeys: String, CodingKey {
        case app, web, billing, usage, check, cli, devices, cost, mail
        case limitResets = "limit_resets"
        case policyLock = "policy_lock"
        case apiKey = "api_key"
        case customProxy = "custom_proxy"
    }

    init() {}

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        app = c.bool(.app) ?? false
        web = c.bool(.web) ?? false
        billing = c.bool(.billing) ?? false
        limitResets = c.bool(.limitResets) ?? false
        usage = c.bool(.usage) ?? false
        check = c.bool(.check) ?? true
        cli = c.bool(.cli) ?? false
        policyLock = c.bool(.policyLock) ?? false
        devices = c.bool(.devices) ?? false
        apiKey = c.bool(.apiKey) ?? false
        cost = c.bool(.cost) ?? false
        mail = c.bool(.mail) ?? false
        customProxy = c.bool(.customProxy) ?? false
    }
}

/// 账号的邮箱助手状态（`account.mail`，PROTOCOL 2.2、4.11）：只读 config.json、不碰钥匙串。
/// `source`：`account`（这个账号单独配了 Gmail）| `default`（用的是默认那个，所有账号共用）| `""`（没配置）。
/// 应用专用密码存不存在、能不能连上，要调 `_gui mail-list` 才知道。
struct MailConfig: Decodable, Sendable, Equatable {
    /// 解析得到收登录邮件的 Gmail 了没有
    var configured: Bool
    /// 那个 Gmail 地址（没配置为 nil）
    var user: String?
    /// 这个账号用的是单独配的还是默认的
    var source: String?
    /// 登录时自不自动打开登录邮件里的链接
    var auto: Bool
    /// 这个账号登记的「信任转发中转」域名（不安全的开关，默认 nil = 关；PROTOCOL 4.11）
    var forwarder: String?

    enum CodingKeys: String, CodingKey { case configured, user, source, auto, forwarder }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        configured = c.bool(.configured) ?? false
        user = c.string(.user)
        source = c.string(.source)
        auto = c.bool(.auto) ?? true
        forwarder = nonEmpty(c.string(.forwarder))
    }

    /// 这个账号单独配了 Gmail（不是用的默认那个）
    var isAccountSpecific: Bool { source == "account" }
    /// 开了「信任转发中转」（不安全）
    var trustsForwarder: Bool { forwarder != nil }
}

func nonEmpty(_ s: String?) -> String? {
    guard let s, !s.isEmpty else { return nil }
    return s
}
