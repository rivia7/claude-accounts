import Foundation

/// Team（PROTOCOL 4.14）的版本：`hello.team` 是它时 App 才显示 Team 的那一块、按 Team 筛选账号，才调 `cca team`。
enum TeamAPI {
    static let version = 1
}

/// 这台设备在一个 Team 里是什么（`config.teams[].role`）。不认识的值当成员
struct TeamRole: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    /// 拿着根密钥：管设备、定账号走哪个节点
    static let admin = TeamRole(rawValue: "admin")
    static let member = TeamRole(rawValue: "member")
    static let allKnown: [TeamRole] = [.admin, .member]
}

/// 一个 Team 在这台设备上的状态（`config.teams[].state`）。不认识的值当 `error`
struct TeamState: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    /// 管理员刚建的，还没接上同步服务器
    static let local = TeamState(rawValue: "local")
    /// 还没同步过配置
    static let pending = TeamState(rawValue: "pending")
    /// 最近一次同步配置成了
    static let ok = TeamState(rawValue: "ok")
    /// 被管理员移除了
    static let removed = TeamState(rawValue: "removed")
    /// 服务器的记录没了，等管理员把这台设备登记回去
    static let lost = TeamState(rawValue: "lost")
    /// 最近一次没同步成（`error` 是给人看的原因）
    static let error = TeamState(rawValue: "error")
    static let allKnown: [TeamState] = [.local, .pending, .ok, .removed, .lost, .error]

    /// 要用户知道的：被移除了、记录没了、没同步成，和不认识的值
    var failing: Bool { self == .removed || self == .lost || self == .error || !isKnown }
}

/// `config.teams[]`（PROTOCOL 2.2），`cca team ls --json` 的 `teams[]` 也是它
struct TeamEntry: Decodable, Sendable, Equatable, Identifiable {
    /// Team 的编号（32 个十六进制字符）：账号的 `team`、`cca team` 命令里认它
    var id: String
    var name: String
    var role: TeamRole
    /// 同步服务器的主机和端口（显示用）；管理员还没接上服务器时是 nil
    var host: String?
    /// 这台设备在这个 Team 里的名字
    var device: String?
    var state: TeamState
    var error: String?
    /// 手里那一份配置是第几版（还没同步下来是 0）
    var seq: Int
    /// 这一份配置给了这台设备几个账号
    var accounts: Int
    var joinedEpoch: Double?

    var isAdmin: Bool { role == .admin }

    enum CodingKeys: String, CodingKey {
        case id, name, role, host, device, state, error, seq, accounts
        case joinedEpoch = "joined_epoch"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = c.string(.id) ?? ""
        name = c.string(.name) ?? ""
        role = c.lossy(TeamRole.self, .role) ?? .member
        host = c.string(.host)
        device = c.string(.device)
        state = c.lossy(TeamState.self, .state) ?? .error
        error = c.string(.error)
        seq = c.int(.seq) ?? 0
        accounts = c.int(.accounts) ?? 0
        joinedEpoch = c.double(.joinedEpoch)
    }
}

/// `cca team devices <Team> --json`（PROTOCOL 4.14；只有管理员的设备问得到）：这个 Team 的设备、配置里的账号 → 节点
struct TeamDevicesDoc: CcaDocument, Equatable {
    static let expectedKind: String? = "team_devices"

    var version: Int?
    var kind: String?
    /// Team 的编号
    var team: String
    var name: String?
    /// 配置里的账号：工具 → 邮箱 → 节点（管理员定的）
    var accounts: [String: [String: String]]
    /// 邮箱 → 别名（各工具通用的那一份：每个邮箱用它最前面那个工具里的；cca 3.5.3-beta.5 及以前只有它）
    var aliases: [String: String]
    /// 工具 → 邮箱 → 别名（按工具记的，各工具各编各的 team-N；cca 3.5.3-beta.6 起，有它时照它）
    var toolAliases: [String: [String: String]]?
    /// 管理员锁定了别名（各台设备上不能自己改；cca 3.5.3-beta.7 起，以前的 cca 没有这一项：nil，App 不给锁定的开关）
    var aliasLock: Bool?
    /// 配置里带着定义的节点的名字
    var nodes: [String]
    var devices: [TeamDevice]

    enum CodingKeys: String, CodingKey {
        case version, kind, team, name, accounts, aliases, nodes, devices
        case toolAliases = "tool_aliases"
        case aliasLock = "alias_lock"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = c.int(.version)
        kind = c.string(.kind)
        team = c.string(.team) ?? ""
        name = c.string(.name)
        accounts = c.lossyDictionary([String: String].self, .accounts) ?? [:]
        aliases = c.lossyDictionary(String.self, .aliases) ?? [:]
        toolAliases = c.lossyDictionary([String: String].self, .toolAliases)
        aliasLock = c.bool(.aliasLock)
        nodes = c.lossyArray(String.self, .nodes) ?? []
        devices = c.lossyArray(TeamDevice.self, .devices) ?? []
    }

    /// 配置里的账号，排好的：先照工具（Claude、Codex、Antigravity，别的在后），再照别名（数字按大小：team-2 在 team-10 前面；
    /// 不分大小写、不随系统的语言变），没有别名的在后，最后照邮箱
    var accountList: [TeamAccount] {
        accounts.flatMap { tool, m in
            m.map { TeamAccount(tool: ToolID(rawValue: tool), email: $0.key, node: $0.value, alias: toolAliases?[tool]?[$0.key] ?? aliases[$0.key]) }
        }
            .sorted(by: TeamAccount.listOrder)
    }
}

/// Team 的配置里的一个账号
struct TeamAccount: Equatable, Sendable, Identifiable {
    var tool: ToolID
    var email: String
    var node: String
    var alias: String?

    /// `cca team grant`、`cca team invite --accounts` 里的写法：工具:邮箱
    var spec: String { "\(tool.rawValue):\(email)" }
    var id: String { spec }

    /// `TeamDevicesDoc.accountList` 的先后：工具、别名（`.numeric`，不带 locale：结果不随系统的语言变）、邮箱
    static func listOrder(_ a: TeamAccount, _ b: TeamAccount) -> Bool {
        if a.tool != b.tool { return a.tool < b.tool }
        let named = { (s: String?) in s.flatMap { $0.isEmpty ? nil : $0 } }
        switch (named(a.alias), named(b.alias)) {
        case let (x?, y?):
            let r = x.compare(y, options: [.numeric, .caseInsensitive])
            if r != .orderedSame { return r == .orderedAscending }
        case (.some, nil): return true
        case (nil, .some): return false
        case (nil, nil): break
        }
        return a.email < b.email
    }
}

/// 一台设备在管理员这边的状态（`devices[].state`）。不认识的值照原样显示不了，当 `invited` 以外的“别的”
struct TeamDeviceState: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    /// 管理员自己这台，还没接上服务器时
    static let local = TeamDeviceState(rawValue: "local")
    /// 发了邀请，还没加入
    static let invited = TeamDeviceState(rawValue: "invited")
    static let joined = TeamDeviceState(rawValue: "joined")
    /// 邀请过期了，没用上
    static let expired = TeamDeviceState(rawValue: "expired")
    /// 它自己退出了
    static let left = TeamDeviceState(rawValue: "left")
    /// 管理员移除了
    static let revoked = TeamDeviceState(rawValue: "revoked")
    static let allKnown: [TeamDeviceState] = [.local, .invited, .joined, .expired, .left, .revoked]

    /// 还在 Team 里、或者还能加入的（列在前面；别的是留着备查的）
    var active: Bool { self == .local || self == .invited || self == .joined }
    /// 不再列出来的：自己退出了的、被移除了的（3.6.6 起 cca 不再记、不再给它们，以前的 cca 还给：界面上不列，解码照样留着）
    var removed: Bool { self == .left || self == .revoked }
}

struct TeamDevice: Decodable, Sendable, Equatable, Identifiable {
    /// 设备编号（十六进制）：`cca team revoke` / `grant` 的参数、`team_revoke` 同意的范围
    var id: String
    /// 设备的名字（管理员起的；界面上只用它）
    var label: String
    /// 它在服务器上的名字：cca 3.5.3-beta.7 起对一遍时照 label 改成一样的（以前的设备加入时报的是主机名）；界面上不显示
    var name: String?
    var state: TeamDeviceState
    /// 管理员自己这台
    var me: Bool
    /// 全部账号都给它
    var grantsAll: Bool
    /// 只给这些（`grantsAll` 时是空的）：工具 → 邮箱
    var grants: [String: [String]]
    /// 发布到第几版
    var seq: Int
    /// 它手里是第几版（服务器说的；没说是 nil）
    var have: Int?
    /// 多少秒以前来过服务器
    var seenAge: Double?
    /// 邀请什么时候过期（只有还没加入的）
    var expiresEpoch: Double?
    /// 要告诉管理员的（公钥对不上、配置没存上……）
    var problem: String?

    enum CodingKeys: String, CodingKey {
        case id = "dev"
        case label, name, state, me, grants, seq, have, problem
        case seenAge = "seen_age"
        case expiresEpoch = "expires_epoch"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = c.string(.id) ?? ""
        label = c.string(.label) ?? ""
        name = c.string(.name)
        state = c.lossy(TeamDeviceState.self, .state) ?? TeamDeviceState(rawValue: "")
        me = c.bool(.me) ?? false
        grantsAll = c.string(.grants) == "all"
        grants = c.lossyDictionary([String].self, .grants) ?? [:]
        seq = c.int(.seq) ?? 0
        have = c.int(.have)
        seenAge = c.double(.seenAge)
        expiresEpoch = c.double(.expiresEpoch)
        problem = c.string(.problem)
    }

    /// 这个账号给不给它
    func has(_ a: TeamAccount) -> Bool { grantsAll || grants[a.tool.rawValue]?.contains(a.email) == true }

    /// 给了它几个账号（all：这个 Team 里的全部）
    func grantCount(of total: Int) -> Int { grantsAll ? total : grants.values.reduce(0) { $0 + $1.count } }
}

/// 最近一次问到的一个 Team 的设备（`team devices --json`，只在内存里）：问的是哪个 Team、没问到的原因（问到了是 nil，上次问到的
/// 照样显示）、什么时候问到的
struct TeamSnapshot: Equatable, Sendable {
    var team: String?
    var doc: TeamDevicesDoc?
    var error: String?
    var fetchedEpoch: Double?
}

/// 正在填的邀请：给哪个 Team 的哪台设备（起的名字）、给哪些账号（all：全部，以后加进 Team 的也给；不然是 picked 里的「工具:邮箱」）
struct TeamInviteDraft: Equatable, Sendable {
    var team: String?
    var label = ""
    var all = true
    var picked: Set<String> = []
}

/// 正在改名字的设备：哪个 Team 的哪台（设备编号）、输入框里的名字（一次只改一台）
struct TeamRenameDraft: Equatable, Sendable {
    var team: String
    var device: String
    var name: String
}

/// 正在改别名的账号：哪个 Team 的哪个账号（`spec`：输入框开在这一行下面；别名按工具记，只改这个工具里的）、输入框里的别名（一次只改一个）
struct TeamAliasDraft: Equatable, Sendable {
    var team: String
    var spec: String
    var email: String
    var alias: String
}

/// 按 Team 筛选账号（侧栏、总览、菜单栏面板共用一个选择；只管显示）
enum TeamFilter: Equatable, Hashable, Sendable {
    /// 全部账号
    case all
    /// 某个 Team 给的
    case team(String)
    /// 不是哪个 Team 给的（自己的账号、第三方 API 的）
    case outside

    /// 存进设置里的写法
    var stored: String {
        switch self {
        case .all: return ""
        case .team(let id): return "team:" + id
        case .outside: return "outside"
        }
    }

    init(stored: String?) {
        switch stored {
        case "outside"?: self = .outside
        case let s? where s.hasPrefix("team:") && s.count > 5: self = .team(String(s.dropFirst(5)))
        default: self = .all
        }
    }

    /// 这个账号在不在筛出来的里面
    func includes(_ a: Account) -> Bool {
        switch self {
        case .all: return true
        case .team(let id): return a.team == id
        case .outside: return a.team == nil
        }
    }
}
