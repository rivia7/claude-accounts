import Foundation

/// 远程会话（PROTOCOL 9）的版本：`hello.remote` 是它时 App 才显示「远程会话」一页、才调这些命令。
enum RemoteAPI {
    static let version = 1
}

/// `cca remote list [--local] --json`（PROTOCOL 9.1）：远程会话的开关、中转用不用得了、这台 Mac 上和配对过的机器上可以接入的
/// 会话、配对过的机器。没问到服务器不算失败：原因在 `errors` 里，这台 Mac 上的照给。
struct RemoteDoc: CcaDocument, Equatable {
    static let expectedKind: String? = "remote"

    var version: Int?
    var kind: String?
    var generatedEpoch: Double?
    /// 开关（`cca remote on`）：开着时，之后在终端里启动的会话放进会话宿主里运行，才接得上
    var enabled: Bool
    /// 这台 Mac 用不用得了中转（在哪个 Team 里、用得了它的服务器）；用不了时 `relayMessage` 说原因
    var relayAvailable: Bool
    var relayMessage: String?
    /// 用得了哪几台中转服务器（各个 Team 的，照加入的先后）：每个 Team 的账号的会话走这个 Team 自己的服务器，配对记在哪台上，
    /// 对方经它看得到的就是走那台的会话。有好几台时，显示配对码要先选在哪台上配。旧版 cca 没有这一项（空的：不用选）；
    /// 3.6.0 以前的 cca 还会给维护者的那一台（没有 `team`）：去掉了，App 不在它上面配对
    var relayHubs: [RemoteHub]
    var sessions: [RemoteSession]
    var peers: [RemotePeer]
    var errors: [RemoteProblem]

    enum CodingKeys: String, CodingKey {
        case version, kind, enabled, relay, sessions, peers, errors
        case generatedEpoch = "generated_epoch"
    }

    private struct Relay: Decodable {
        var available: Bool?
        var message: String?
        var hubs: [RemoteHub]
        enum CodingKeys: String, CodingKey { case available, message, hubs }
        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            available = c.bool(.available)
            message = c.string(.message)
            hubs = c.lossyArray(RemoteHub.self, .hubs) ?? []
        }
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = c.int(.version)
        kind = c.string(.kind)
        generatedEpoch = c.double(.generatedEpoch)
        enabled = c.bool(.enabled) ?? false
        let relay = c.lossy(Relay.self, .relay)
        relayAvailable = relay?.available ?? false
        relayMessage = relay?.message
        relayHubs = relay?.hubs ?? []
        sessions = c.lossyArray(RemoteSession.self, .sessions) ?? []
        peers = c.lossyArray(RemotePeer.self, .peers) ?? []
        errors = c.lossyArray(RemoteProblem.self, .errors) ?? []
    }

    func session(_ id: String) -> RemoteSession? { sessions.first { $0.id == id } }
}

/// 一个可以接入的会话。
struct RemoteSession: Decodable, Sendable, Equatable, Identifiable {
    /// 会话的编号（32 个十六进制字符）：`cca remote attach` 的参数
    var id: String
    /// 给人看的那一截（前 8 位）
    var short: String
    /// 在这台 Mac 上，还是在配对过的另一台机器上
    var place: RemotePlace
    /// 另一台机器上的：是哪个配对的（`peers[].id`）、那台机器的名字
    var peer: String?
    var machine: String?
    var tool: ToolID?
    /// 账号的名字（别名；没起别名的是邮箱）。别的机器上的会话：这台 Mac 上也有这个账号时是这台 Mac 上的别名（3.6.16 起）
    var name: String
    /// 别的机器上的会话：那台机器说的名字（它那里的别名）。这台 Mac 上的、旧版 cca 给的是 nil
    var hostName: String?
    var cwd: String
    var sinceEpoch: Double?
    /// 现在有几处接着（不算启动它的那个终端）
    var viewers: Int
    /// 启动它的那个终端还开着（只有这台 Mac 上的会话知道；另一台机器上的是 nil）
    var localTerminal: Bool?
    var state: RemoteSessionState
    /// 这台 Mac 接进去能做什么：`control` 看和操作，`view` 只能看（那台机器设的）
    var access: RemoteAccess
    /// 这个会话属于哪个 Team（Team 的编号）：这台 Mac 上的是它走哪个 Team 的服务器中转（账号是哪个 Team 给的；别的走「家」），
    /// 别的机器上的是经哪个 Team 的服务器上的配对看到的。没有中转、旧版 cca 没有这个字段时是 nil：不标
    var team: String?

    enum CodingKeys: String, CodingKey {
        case id, short, peer, machine, tool, name, cwd, viewers, state, access, team
        case place = "where"
        case sinceEpoch = "since_epoch"
        case localTerminal = "local_terminal"
        case hostName = "host_name"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let id = c.string(.id), !id.isEmpty else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "remote session without id"))
        }
        self.id = id
        short = c.string(.short) ?? String(id.prefix(8))
        place = c.lossy(RemotePlace.self, .place) ?? .local
        peer = c.string(.peer)
        machine = c.string(.machine)
        tool = c.lossy(ToolID.self, .tool)
        name = c.string(.name) ?? ""
        hostName = c.string(.hostName).flatMap { $0.isEmpty ? nil : $0 }
        cwd = c.string(.cwd) ?? ""
        sinceEpoch = c.double(.sinceEpoch)
        viewers = max(c.int(.viewers) ?? 0, 0)
        localTerminal = c.bool(.localTerminal)
        // 缺了、类型不对的往保守的方向取：不算接得上、只看（cca 一直都给这两项）
        state = c.lossy(RemoteSessionState.self, .state) ?? RemoteSessionState(rawValue: "")
        access = c.lossy(RemoteAccess.self, .access) ?? .view
        team = c.string(.team).flatMap { $0.isEmpty ? nil : $0 }
    }

    var isLocal: Bool { place == .local }
    /// 现在接得上：宿主有回应、那台机器连着服务器
    var attachable: Bool { state == .ok }
}

/// 一台用得了的中转服务器（`relay.hubs[]`）：某个 Team 的同步服务器。没有 `team` 的（3.6.0 以前的 cca 给的维护者的那一台）解不出来，
/// 列表里丢掉
struct RemoteHub: Decodable, Sendable, Equatable, Identifiable {
    /// Team 的编号
    var team: String
    /// 给人看的：「研发组」的服务器
    var name: String

    var id: String { team }
    /// `cca remote pair` 后面说在这台上配对的参数
    var arguments: [String] { ["--team", team] }

    enum CodingKeys: String, CodingKey { case team, name }

    init(team: String, name: String) {
        self.team = team
        self.name = name
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let team = nonEmpty(c.string(.team)) else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "relay hub without team"))
        }
        self.team = team
        name = c.string(.name) ?? ""
    }
}

/// 配对过的一台机器。
struct RemotePeer: Decodable, Sendable, Equatable, Identifiable {
    /// 配对的编号（32 个十六进制字符）：`cca remote allow|unpair` 的参数
    var id: String
    var short: String
    var name: String
    /// 它能对这台 Mac 的会话做什么
    var incoming: RemoteAccess
    /// 配对码是这台 Mac 显示的（是的话，配对时默认让它看和操作）
    var shown: Bool
    var pairedEpoch: Double?
    var state: RemotePeerState
    /// 它是手机上的网页端（`cca remote pair --web` 配的）：只是给人看的，它能做什么照旧看 `incoming`。旧版 cca 没有这个字段
    var web: Bool
    /// 这个配对记在哪个 Team 的服务器上（经它看得到的是这个 Team 的账号的会话）。旧版 cca 没有这个字段、3.6.0 以前经维护者的
    /// 服务器配的（3.6.0 的 cca 不再列出）是 nil：照样列出来，不标是哪个 Team 的
    var team: String?

    enum CodingKeys: String, CodingKey {
        case id, short, name, incoming, shown, state, web, team
        case pairedEpoch = "paired_epoch"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let id = c.string(.id), !id.isEmpty else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "remote peer without id"))
        }
        self.id = id
        short = c.string(.short) ?? String(id.prefix(8))
        name = c.string(.name) ?? ""
        incoming = c.lossy(RemoteAccess.self, .incoming) ?? .none
        shown = c.bool(.shown) ?? false
        pairedEpoch = c.double(.pairedEpoch)
        state = c.lossy(RemotePeerState.self, .state) ?? .unknown
        web = c.bool(.web) ?? false
        team = c.string(.team).flatMap { $0.isEmpty ? nil : $0 }
    }
}

/// 没问到别的机器上的会话的原因。
struct RemoteProblem: Decodable, Sendable, Equatable {
    var code: RemoteProblemCode
    var message: String

    enum CodingKeys: String, CodingKey { case code, message }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        code = c.lossy(RemoteProblemCode.self, .code) ?? .relayError
        message = c.string(.message) ?? ""
    }
}

/// `where`：会话在这台 Mac 上（`local`）还是配对过的机器上（`peer`）。
struct RemotePlace: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let local = RemotePlace(rawValue: "local")
    static let peer = RemotePlace(rawValue: "peer")
    static let allKnown: [RemotePlace] = [.local, .peer]
}

/// 会话的 `state`：`ok` 接得上；`busy` 宿主没有回应（被挂起了、机器正忙）；`offline` 那台机器暂时连不上服务器。
struct RemoteSessionState: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let ok = RemoteSessionState(rawValue: "ok")
    static let busy = RemoteSessionState(rawValue: "busy")
    static let offline = RemoteSessionState(rawValue: "offline")
    static let allKnown: [RemoteSessionState] = [.ok, .busy, .offline]
}

/// 能做什么：`control` 看和操作，`view` 只看，`none` 不让接入（只用在 `peers[].incoming`）。
struct RemoteAccess: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let control = RemoteAccess(rawValue: "control")
    static let view = RemoteAccess(rawValue: "view")
    static let none = RemoteAccess(rawValue: "none")
    static let allKnown: [RemoteAccess] = [.control, .view, .none]
}

/// 配对的 `state`：`ok` 服务器上还在；`gone` 对方取消了配对（或者太久没用被清掉了）；`unknown` 这次没问服务器、没问到。
struct RemotePeerState: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let ok = RemotePeerState(rawValue: "ok")
    static let gone = RemotePeerState(rawValue: "gone")
    static let unknown = RemotePeerState(rawValue: "unknown")
    static let allKnown: [RemotePeerState] = [.ok, .gone, .unknown]
}

/// `errors[].code`：`relay_off` 这台 Mac 用不了中转；`relay_old` 服务器还不支持；`relay_unreachable` 连不上；
/// `relay_denied` 服务器不认这台 Mac；`relay_error` 别的。
struct RemoteProblemCode: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let relayOff = RemoteProblemCode(rawValue: "relay_off")
    static let relayOld = RemoteProblemCode(rawValue: "relay_old")
    static let relayUnreachable = RemoteProblemCode(rawValue: "relay_unreachable")
    static let relayDenied = RemoteProblemCode(rawValue: "relay_denied")
    static let relayError = RemoteProblemCode(rawValue: "relay_error")
    static let allKnown: [RemoteProblemCode] = [.relayOff, .relayOld, .relayUnreachable, .relayDenied, .relayError]
}

/// App 里记着的「远程会话」（只在内存里）：最近一次查到的，和最近一次没查成的原因。
struct RemoteSnapshot: Equatable, Sendable {
    var doc: RemoteDoc?
    /// 最近一次没查成的原因；查成了是 nil（上次查到的照样显示）
    var error: String?
    var fetchedEpoch: Double?
}
