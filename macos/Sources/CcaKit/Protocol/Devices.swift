import Foundation

/// 账号连着的设备（`cca [codex] devices KEY --json`，PROTOCOL 3.4）：账号走的那台服务器（维护者的、自己的同步服务器）上的记录，
/// 时间 cca 已经换成这台 Mac 的时钟。只有经服务器同步的官方账号才有（`can.devices`）。
struct DevicesDoc: CcaDocument, Equatable {
    static let expectedKind: String? = "devices"

    var version: Int?
    var kind: String?
    var generatedEpoch: Double?
    var tool: ToolID?
    /// 这台 Mac 在服务器上的名字（维护者给它的 deploy key 起的别名）；服务器还不认它时是 nil
    var me: String?
    /// 多久没动静算闲置、多久没回报算离线（秒，服务器的设置）
    var idleAfter: Double?
    var offlineAfter: Double?
    /// 这台 Mac 最近一次没报成的原因（报成了是 nil）
    var reportingError: String?
    /// 这台设备用得了的每台服务器（这一次没问的那台 me、idleAfter、offlineAfter 是 nil；顶层的是问了的第一台的）；旧版 cca 没有，是空的
    var hubs: [DevicesHub]
    var accounts: [DevicesAccount]

    enum CodingKeys: String, CodingKey {
        case version, kind, tool, me, accounts, hubs
        case generatedEpoch = "generated_epoch"
        case idleAfter = "idle_after"
        case offlineAfter = "offline_after"
        case reportingError = "reporting_error"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = c.int(.version)
        kind = c.string(.kind)
        generatedEpoch = c.double(.generatedEpoch)
        tool = c.lossy(ToolID.self, .tool)
        me = c.string(.me)
        idleAfter = c.double(.idleAfter)
        offlineAfter = c.double(.offlineAfter)
        reportingError = c.string(.reportingError)
        hubs = c.lossyArray(DevicesHub.self, .hubs) ?? []
        accounts = c.lossyArray(DevicesAccount.self, .accounts) ?? []
    }

    func account(_ key: String) -> DevicesAccount? { accounts.first { $0.key == key } }

    /// 这个账号问的那台服务器的设置和名字（旧版 cca 没有 `hubs`：用顶层的几项）
    func hub(for account: DevicesAccount) -> DevicesHub {
        // 可能同时在几个 Team 里（kind 都是 team）：照编号认
        if let h = hubs.first(where: { $0.kind == account.hub && $0.team == account.team }) { return h }
        return DevicesHub(kind: account.hub ?? .maintainer, me: me, idleAfter: idleAfter, offlineAfter: offlineAfter, reportingError: reportingError)
    }
}

/// `devices --json` 的 `hubs[]`：这台设备用得了的一台服务器
struct DevicesHub: Decodable, Sendable, Equatable {
    var kind: HubKind
    /// 这台 Mac 在它上面的名字；它还不认这台 Mac 时是 nil
    var me: String?
    var idleAfter: Double?
    var offlineAfter: Double?
    /// 这台 Mac 最近一次没报到它的原因（报成了是 nil）
    var reportingError: String?
    /// Team 的服务器（`kind` 是 `team`）：是哪个 Team 的；别的是 nil
    var team: String?

    enum CodingKeys: String, CodingKey {
        case kind, me, team
        case idleAfter = "idle_after"
        case offlineAfter = "offline_after"
        case reportingError = "reporting_error"
    }

    init(kind: HubKind, me: String?, idleAfter: Double?, offlineAfter: Double?, reportingError: String?) {
        self.kind = kind
        self.me = me
        self.idleAfter = idleAfter
        self.offlineAfter = offlineAfter
        self.reportingError = reportingError
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        kind = c.lossy(HubKind.self, .kind) ?? HubKind(rawValue: "")
        me = c.string(.me)
        idleAfter = c.double(.idleAfter)
        offlineAfter = c.double(.offlineAfter)
        reportingError = c.string(.reportingError)
        team = c.string(.team)
    }
}

struct DevicesAccount: Decodable, Sendable, Equatable {
    var key: String
    /// 问的哪台服务器（旧版 cca 没有：nil，当作维护者的）
    var hub: HubKind?
    /// 经 Team 的服务器问的（`hub` 是 `team`）：是哪个 Team 的
    var team: String?
    /// 这台 Mac 排第一，其余按 在用、闲置、离线、没在用，同一种里最近用过的在前（cca 排好的）
    var devices: [DeviceRow]

    enum CodingKeys: String, CodingKey { case key, hub, team, devices }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let key = c.string(.key), !key.isEmpty else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "devices account without key"))
        }
        self.key = key
        hub = c.lossy(HubKind.self, .hub)
        team = c.string(.team)
        devices = c.lossyArray(DeviceRow.self, .devices) ?? []
    }
}

/// 一台设备在这个账号上的状态。
struct DeviceRow: Decodable, Sendable, Equatable, Identifiable {
    /// 设备在那台服务器上的指纹（SHA256:…）
    var deviceID: String?
    var name: String
    /// 是这台 Mac
    var isMe: Bool
    var state: DeviceState
    /// 开着几个命令行会话、桌面 App 开没开（离线时是最后一次回报时的）
    var cli: Int
    var app: Bool
    /// 开着的会话里最早的那个从什么时候开始
    var sinceEpoch: Double?
    /// 开着的会话最后一次有动静（闲置从这时算起）
    var lastActiveEpoch: Double?
    /// 上次使用（会话都关了也留着，30 天）
    var lastUsedEpoch: Double?
    /// 最后一次收到它的回报
    var seenEpoch: Double?

    var id: String { deviceID ?? name }

    enum CodingKeys: String, CodingKey {
        case name, me, state, cli, app
        case deviceID = "id"
        case sinceEpoch = "since_epoch"
        case lastActiveEpoch = "last_active_epoch"
        case lastUsedEpoch = "last_used_epoch"
        case seenEpoch = "seen_epoch"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let name = c.string(.name), !name.isEmpty else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "device without name"))
        }
        self.name = name
        deviceID = c.string(.deviceID)
        isMe = c.bool(.me) ?? false
        state = c.lossy(DeviceState.self, .state) ?? .none
        cli = max(c.int(.cli) ?? 0, 0)
        app = c.bool(.app) ?? false
        sinceEpoch = c.double(.sinceEpoch)
        lastActiveEpoch = c.double(.lastActiveEpoch)
        lastUsedEpoch = c.double(.lastUsedEpoch)
        seenEpoch = c.double(.seenEpoch)
    }
}

/// `state`：在用（开着，10 分钟内有动静）/ 闲置（开着，很久没动静）/ 没在用（都关了）/ 离线（开着会话却很久没回报：
/// 可能休眠、断网、被强行结束）。不认识的值当“没在用”显示。
struct DeviceState: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let inUse = DeviceState(rawValue: "in_use")
    static let idle = DeviceState(rawValue: "idle")
    static let none = DeviceState(rawValue: "none")
    static let offline = DeviceState(rawValue: "offline")
    static let allKnown: [DeviceState] = [.inUse, .idle, .none, .offline]
}

/// App 里记着的一个账号的「设备」（只在内存里）：最近一次查到的，和最近一次没查成的原因。
struct DevicesSnapshot: Equatable, Sendable {
    var doc: DevicesDoc?
    var account: DevicesAccount?
    /// 最近一次没查成的原因；查成了是 nil（上次查到的照样显示）
    var error: String?
    /// 最近一次查成的时间
    var fetchedEpoch: Double?
}
