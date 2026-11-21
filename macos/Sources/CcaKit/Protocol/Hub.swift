import Foundation

/// 自己的同步服务器（PROTOCOL 4.13）的版本：`hello.hub` 是它时 App 才显示「自己的同步服务器」、才调 `cca hub`。
enum HubAPI {
    static let version = 1
}

/// 账号的设备统计、记下的 API key、折合金额经哪一类服务器同步（PROTOCOL 2.2 的 `config.hubs[].kind`、账号的 `hub`、`cost.sync.hub`）
struct HubKind: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    /// 维护者的服务器：映射里的官方账号
    static let maintainer = HubKind(rawValue: "maintainer")
    /// 自己的同步服务器：别的官方账号（这台设备加入了时）
    static let own = HubKind(rawValue: "own")
    static let allKnown: [HubKind] = [.maintainer, .own]
}

/// 一台服务器同步得怎么样（`config.hubs[].state`）。不认识的值当 `error`
struct HubState: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    /// 最近一次成了
    static let ok = HubState(rawValue: "ok")
    /// 还没同步过
    static let pending = HubState(rawValue: "pending")
    /// 最近一次没成（`error` 是给人看的原因）
    static let error = HubState(rawValue: "error")
    /// 这台设备关掉了设备统计（config.json 里写了 `"presence": false`；只有自己的同步服务器会是它）
    static let off = HubState(rawValue: "off")
    static let allKnown: [HubState] = [.ok, .pending, .error, .off]

    /// 当作出了问题：`error` 和不认识的值
    var failing: Bool { self == .error || !isKnown }
}

/// `config.hubs[]`（PROTOCOL 2.2），`cca hub status --json` 的 `hub` 也是它
struct HubEntry: Decodable, Sendable, Equatable {
    var kind: HubKind
    var url: String
    /// 显示用的主机和端口
    var host: String
    var state: HubState
    var error: String?
    /// 最近一次成功的时间
    var checkedEpoch: Double?
    /// 经它同步的账号数（各工具加起来；用不了时 0）
    var accounts: Int
    /// 这台设备在它上面的名字（维护者的：上次同步折合金额时服务器说的，没同步过是 nil）
    var name: String?
    /// 自己的同步服务器上有几台设备（上次问到的）；维护者的是 nil
    var members: Int?
    /// 什么时候加入的（只有自己的同步服务器）
    var joinedEpoch: Double?
    /// 能给出加入链接（`cca hub invite`，只有自己的同步服务器）
    var canInvite: Bool
    /// 自己的同步服务器：加入链接里钉着证书的指纹（自己部署的，更新经 SSH：`cca hub deploy`）；false 是照证书机构验（托管平台上的，
    /// 更新照 `cca hub hosted` 的步骤）。维护者的是 nil
    var pinned: Bool?
    /// 托管平台上的那一台（`pinned` 是 false）：更新照 `cca hub hosted`，没有经 SSH 的从头部署
    var onPlatform: Bool { pinned == false }

    enum CodingKeys: String, CodingKey {
        case kind, url, host, state, error, accounts, name, members
        case checkedEpoch = "checked_epoch"
        case joinedEpoch = "joined_epoch"
        case canInvite = "can_invite"
        case pinned
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        kind = c.lossy(HubKind.self, .kind) ?? HubKind(rawValue: "")
        url = c.string(.url) ?? ""
        host = c.string(.host) ?? ""
        state = c.lossy(HubState.self, .state) ?? .error
        error = c.string(.error)
        checkedEpoch = c.double(.checkedEpoch)
        accounts = c.int(.accounts) ?? 0
        name = c.string(.name)
        members = c.int(.members)
        joinedEpoch = c.double(.joinedEpoch)
        canInvite = c.bool(.canInvite) ?? false
        pinned = c.bool(.pinned)
    }
}

/// `cca hub status --json`（PROTOCOL 4.13）：只读本机记着的，不联网
struct HubStatusDoc: CcaDocument, Equatable {
    static let expectedKind: String? = "hub_status"

    var version: Int?
    var kind: String?
    /// 加入了没有
    var joined: Bool
    /// 这台设备现在用不了它的原因（没加入、关掉了设备统计、密钥不见了）；用得了是 nil
    var reason: String?
    /// 和 `config.hubs` 里自己的那一项一样；没加入是 nil
    var hub: HubEntry?

    enum CodingKeys: String, CodingKey { case version, kind, joined, reason, hub }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = c.int(.version)
        kind = c.string(.kind)
        joined = c.bool(.joined) ?? false
        reason = c.string(.reason)
        hub = c.lossy(HubEntry.self, .hub)
    }
}

/// `cca hub devices --json`（PROTOCOL 4.13）：自己的同步服务器上加入了的设备，这台在前（cca 排好的）
struct HubDevicesDoc: CcaDocument, Equatable {
    static let expectedKind: String? = "hub_devices"

    var version: Int?
    var kind: String?
    var generatedEpoch: Double?
    var devices: [HubDevice]

    enum CodingKeys: String, CodingKey {
        case version, kind, devices
        case generatedEpoch = "generated_epoch"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = c.int(.version)
        kind = c.string(.kind)
        generatedEpoch = c.double(.generatedEpoch)
        devices = c.lossyArray(HubDevice.self, .devices) ?? []
    }
}

struct HubDevice: Decodable, Sendable, Equatable, Identifiable {
    /// 设备在服务器上的指纹（`SHA256:…`）：`cca hub remove` 的参数、`hub_remove` 同意的范围
    var id: String
    var name: String
    /// 这台设备
    var me: Bool
    var joinedEpoch: Double?
    /// 服务器最后一次收到它的请求（服务器重启以后还没来过的是 nil）
    var seenEpoch: Double?

    enum CodingKeys: String, CodingKey {
        case id, name, me
        case joinedEpoch = "joined_epoch"
        case seenEpoch = "seen_epoch"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = c.string(.id) ?? ""
        name = c.string(.name) ?? ""
        me = c.bool(.me) ?? false
        joinedEpoch = c.double(.joinedEpoch)
        seenEpoch = c.double(.seenEpoch)
    }
}

/// 最近一次问到的设备（`hub devices --json`，只在内存里）：问的是哪一台（`url`：退出、换了一台以后不再显示）、没问到的原因
/// （问到了是 nil，上次问到的照样显示）、什么时候问到的
struct HubSnapshot: Equatable, Sendable {
    var url: String?
    var doc: HubDevicesDoc?
    var error: String?
    var fetchedEpoch: Double?
}
