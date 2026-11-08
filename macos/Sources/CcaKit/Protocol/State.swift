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

    enum CodingKeys: String, CodingKey {
        case version, kind
        case generatedAt = "generated_at"
        case ccaVersion = "cca_version"
        case errors, notices, config, routing, tools
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

    enum CodingKeys: String, CodingKey {
        case routePref = "route_pref"
        case router, local, nodes
        case minTrust = "min_trust"
        case leakCheck = "leak_check"
        case update, policy
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
    }

    var routerConfigured: Bool { router?.configured ?? false }
    var localConfigured: Bool { local?.configured ?? false }
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

    enum CodingKeys: String, CodingKey { case configured, client, name }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        configured = c.bool(.configured) ?? false
        client = c.lossy(LocalClient.self, .client)
        name = c.string(.name)
    }
}

struct PolicyConfig: Decodable, Sendable, Equatable {
    var configured: Bool
    var repo: String?

    enum CodingKeys: String, CodingKey { case configured, repo }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        configured = c.bool(.configured) ?? false
        repo = c.string(.repo)
    }
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

    enum CodingKeys: String, CodingKey {
        case id, label
        case appLabel = "app_label"
        case cliPrefix = "cli_prefix"
        case enabled, bin, app, chrome
        case settingsProblems = "settings_problems"
        case errors, last, accounts
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
    var usage: UsageEntry?
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
        case api, badge, app, cli, can
        case canWhy = "can_why"
        case usage, health
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
        usage = c.lossy(UsageEntry.self, .usage)
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
    var usage: Bool = false
    var check: Bool = true
    /// 能不能在终端里打开（要有 claude / codex 命令）
    var cli: Bool = false
    /// 能不能请求维护者把节点锁进映射（官方账号、这台机器配置了映射、还没被映射固定；PROTOCOL 4.8）
    var policyLock: Bool = false

    enum CodingKeys: String, CodingKey {
        case app, web, usage, check, cli
        case policyLock = "policy_lock"
    }

    init() {}

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        app = c.bool(.app) ?? false
        web = c.bool(.web) ?? false
        usage = c.bool(.usage) ?? false
        check = c.bool(.check) ?? true
        cli = c.bool(.cli) ?? false
        policyLock = c.bool(.policyLock) ?? false
    }
}

func nonEmpty(_ s: String?) -> String? {
    guard let s, !s.isEmpty else { return nil }
    return s
}
