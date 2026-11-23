import Foundation

/// 账号的设备统计、记下的 API key、折合金额经哪一类服务器同步（PROTOCOL 2.2 的 `config.hubs[].kind`、账号的 `hub`、`cost.sync.hub`）。
/// 3.5.3 以前还有 `own`（自己的同步服务器），3.6.0 以前还有 `maintainer`（维护者的服务器），后来都去掉了：旧版 cca 给的这两个值不认识，
/// 当作没有（不当成哪个 Team 的）
struct HubKind: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    /// 某个 Team 的同步服务器：这个 Team 给这台设备的账号（PROTOCOL 4.14；是哪个 Team 看旁边的 `team`）
    static let team = HubKind(rawValue: "team")
    static let allKnown: [HubKind] = [.team]
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
    static let allKnown: [HubState] = [.ok, .pending, .error]

    /// 当作出了问题：`error` 和不认识的值
    var failing: Bool { self == .error || !isKnown }
}

/// `config.hubs[]`（PROTOCOL 2.2）。cca 照旧给的 `members`、`joined_epoch`、`can_invite`、`pinned`（以前只有自己的同步服务器才有值）不读
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
    /// 这台设备在它上面的名字（上次同步折合金额时服务器说的，没同步过是 nil）
    var name: String?
    /// 是哪个 Team 的服务器（编号）、这个 Team 的名字
    var team: String?
    var teamName: String?

    enum CodingKeys: String, CodingKey {
        case kind, url, host, state, error, accounts, name, team
        case teamName = "team_name"
        case checkedEpoch = "checked_epoch"
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
        team = c.string(.team)
        teamName = c.string(.teamName)
    }
}
