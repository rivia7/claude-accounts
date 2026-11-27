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
    /// 远程启动（PROTOCOL 9.4）：等配对的设备来起会话的后台进程在不在跑、有几个模板、几台能用。旧版 cca 没有这一项（nil）
    var launcher: RemoteLauncher?
    /// 配对过的机器上允许这台 Mac 远程启动的（PROTOCOL 9.1、9.5，要 `hello.remote_launch_machines`）：那台机器上的启动器和它的模板。
    /// 旧版 cca 没有这一项（空的）
    var peerLaunchers: [RemotePeerLauncher]

    enum CodingKeys: String, CodingKey {
        case version, kind, enabled, relay, sessions, peers, errors, launcher
        case generatedEpoch = "generated_epoch"
        case peerLaunchers = "peer_launchers"
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
        launcher = c.lossy(RemoteLauncher.self, .launcher)
        peerLaunchers = c.lossyArray(RemotePeerLauncher.self, .peerLaunchers) ?? []
    }

    func session(_ id: String) -> RemoteSession? { sessions.first { $0.id == id } }
    /// 这个配对（`peers[].id`）那一头的启动器
    func peerLauncher(_ peer: String) -> RemotePeerLauncher? { peerLaunchers.first { $0.peer == peer } }
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
    /// 它是手机（`cca remote pair --web` 配的：iPhone 上的 cca App，或者以前的网页端）：只是给人看的，它能做什么照旧看 `incoming`。旧版 cca 没有这个字段
    var web: Bool
    /// 这个配对记在哪个 Team 的服务器上（经它看得到的是这个 Team 的账号的会话）。旧版 cca 没有这个字段、3.6.0 以前经维护者的
    /// 服务器配的（3.6.0 的 cca 不再列出）是 nil：照样列出来，不标是哪个 Team 的
    var team: String?
    /// 它能用这台 Mac 上的模板新建会话（远程启动，`cca remote allow <配对的 id> launch on`，PROTOCOL 9.4）。能看和操作的才会是 true
    /// （改成只看、不让时 cca 一起关掉）；3.7.6 只给手机，有 `hello.remote_launch_machines` 的 cca 配对过的电脑也行。旧版 cca 没有这个字段，当成 false
    var launch: Bool

    enum CodingKeys: String, CodingKey {
        case id, short, name, incoming, shown, state, web, team, launch
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
        launch = c.bool(.launch) ?? false
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

// MARK: 远程启动（PROTOCOL 9.4）

/// 远程启动的后台进程（启动器：等允许远程启动的手机、电脑来起会话）的样子：`remote list --json`、`remote template list --json` 的 `launcher`。
struct RemoteLauncher: Decodable, Sendable, Equatable {
    /// 在跑：允许远程启动的配对的设备现在就能起会话
    var running: Bool
    /// 有几个模板（列表里的这个数和手里的模板对不上时 App 再查一次模板）；没给是 nil
    var templates: Int?
    /// 有几台配对的设备（手机、电脑）能远程启动
    var peers: Int

    enum CodingKeys: String, CodingKey { case running, templates, peers }

    init(running: Bool, templates: Int?, peers: Int) {
        self.running = running
        self.templates = templates
        self.peers = peers
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        running = c.bool(.running) ?? false
        templates = c.int(.templates).map { max($0, 0) }
        peers = max(c.int(.peers) ?? 0, 0)
    }
}

/// `cca remote template list --json`（PROTOCOL 9.4）：远程启动用的模板和启动器的样子。只读、不联网。
/// 手机只能挑一个模板在这台 Mac 上起会话（可以带一句开场白）：账号、文件夹、参数都改不了。
struct RemoteTemplatesDoc: CcaDocument, Equatable {
    static let expectedKind: String? = "remote_templates"

    var version: Int?
    var kind: String?
    var templates: [RemoteTemplate]
    var launcher: RemoteLauncher?

    enum CodingKeys: String, CodingKey { case version, kind, templates, launcher }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = c.int(.version)
        kind = c.string(.kind)
        templates = c.lossyArray(RemoteTemplate.self, .templates) ?? []
        launcher = c.lossy(RemoteLauncher.self, .launcher)
    }

    func template(_ id: String) -> RemoteTemplate? { templates.first { $0.id == id } }
}

/// 一个模板：用哪个工具、在哪个文件夹、带哪些参数起会话。账号不在模板里（3.7.10 起）：起会话的那一头挑（手机、另一台机器）；
/// cca 照旧给的 `account`（空的）、`account_key`、`account_name`（null）是给 3.7.9 及以前的 App 看的，这里不读。
struct RemoteTemplate: Decodable, Sendable, Equatable, Identifiable {
    /// 模板的编号（8 个十六进制字符）：`cca remote template add --replace`、`rm` 的参数
    var id: String
    /// 名字（手机上看到的）
    var name: String
    /// 用哪个工具起；不认识的照样列出来
    var tool: ToolID?
    /// 在哪个文件夹里起（绝对路径）
    var dir: String
    /// 起会话时带的参数（一项一个）
    var args: [String]
    /// 参数拼成的一行（照 shell 的写法，要引的才用单引号引起来）：给人看、给人改，存的时候原样交给 cca 拆（`--args-line`），拆回去是
    /// 原样。旧版 cca 没给时照同样的写法拼
    var argsLine: String
    /// 手机起会话时能带一句开场白
    var line: Bool
    /// 现在用不了的原因；用得了是 nil
    var problem: RemoteTemplateProblem?

    enum CodingKeys: String, CodingKey {
        case id, name, tool, dir, args, line, problem
        case argsLine = "args_line"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let id = nonEmpty(c.string(.id)) else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "remote template without id"))
        }
        self.id = id
        name = c.string(.name) ?? ""
        tool = c.lossy(ToolID.self, .tool)
        dir = c.string(.dir) ?? ""
        args = c.lossyArray(String.self, .args) ?? []
        argsLine = c.string(.argsLine) ?? args.map(shellQuoteIfNeeded).joined(separator: " ")
        line = c.bool(.line) ?? false
        problem = c.lossy(RemoteTemplateProblem.self, .problem)
    }
}

/// 模板现在用不了的原因（`templates[].problem`）：`no_account` 模板的工具还没有账号（3.7.9 及以前是模板存着的账号在账号表里没有了）；
/// `no_dir` 文件夹不在了；`bad_args` 参数用不了（不是交互的会话，或者启动前的检查过不了）；`no_relay` 这个工具的账号的会话都不经中转
/// （这台 Mac 不在哪个 Team 里）。
struct RemoteTemplateProblem: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let noAccount = RemoteTemplateProblem(rawValue: "no_account")
    static let noDir = RemoteTemplateProblem(rawValue: "no_dir")
    static let badArgs = RemoteTemplateProblem(rawValue: "bad_args")
    static let noRelay = RemoteTemplateProblem(rawValue: "no_relay")
    static let allKnown: [RemoteTemplateProblem] = [.noAccount, .noDir, .badArgs, .noRelay]
}

/// App 里记着的模板（只在内存里）：最近一次查到的，和最近一次没查成的原因。
struct RemoteTemplatesSnapshot: Equatable, Sendable {
    var doc: RemoteTemplatesDoc?
    /// 最近一次没查成的原因；查成了是 nil（上次查到的照样显示）
    var error: String?
}

// MARK: 用别的机器上的模板新建会话（PROTOCOL 9.5）

/// 配对过的另一台机器上的启动器（`remote list --json` 的 `peer_launchers[]`）：那台机器允许这台 Mac 用它的模板在它上面新建会话
/// （那台机器上 `cca remote allow 这台 Mac launch on`）。启动器不是会话（不在 `sessions` 里）。
struct RemotePeerLauncher: Decodable, Sendable, Equatable, Identifiable {
    /// 启动器在服务器上的编号（32 个十六进制字符）
    var id: String
    /// 是哪个配对的（`peers[].id`）：`cca remote launch` 的第一个参数
    var peer: String
    /// 那台机器的名字
    var machine: String
    /// `ok` 那台机器连着服务器（起得了）；`offline` 暂时连不上
    var state: RemoteSessionState
    /// 经哪个 Team 的服务器上的配对看到的；没有是 nil
    var team: String?
    /// 说明里放不下、没列出来的模板有几个
    var more: Int
    var templates: [RemotePeerTemplate]
    /// 起会话时能挑的账号（3.7.10 起：模板不带账号，起的时候挑一个）；那台机器上的 cca 是旧版时是 nil（模板用它自己的账号：
    /// `templates[].account`）
    var accounts: [RemotePeerAccount]?

    enum CodingKeys: String, CodingKey { case id, peer, machine, state, team, more, templates, accounts }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let id = nonEmpty(c.string(.id)), let peer = nonEmpty(c.string(.peer)) else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "peer launcher without id or peer"))
        }
        self.id = id
        self.peer = peer
        machine = c.string(.machine) ?? ""
        state = c.lossy(RemoteSessionState.self, .state) ?? RemoteSessionState(rawValue: "")
        team = c.string(.team).flatMap { $0.isEmpty ? nil : $0 }
        more = max(c.int(.more) ?? 0, 0)
        templates = c.lossyArray(RemotePeerTemplate.self, .templates) ?? []
        accounts = c.lossyArray(RemotePeerAccount.self, .accounts)
    }

    /// 现在起得了：那台机器连着服务器
    var online: Bool { state == .ok }
    func template(_ id: String) -> RemotePeerTemplate? { templates.first { $0.id == id } }
    /// 用这个模板起会话时能挑的账号（模板的那个工具的）；那台机器上的 cca 是旧版时是 nil（模板用它自己的账号，不用挑）
    func accounts(for t: RemotePeerTemplate) -> [RemotePeerAccount]? {
        accounts.map { list in list.filter { $0.tool != nil && $0.tool == t.tool } }
    }
}

/// 另一台机器上起会话时能挑的一个账号（`peer_launchers[].accounts[]`，3.7.10 起）。
struct RemotePeerAccount: Decodable, Sendable, Equatable, Identifiable {
    /// 编号（16 个十六进制字符，按配对算的）：`cca remote launch … --account` 的值
    var id: String
    var tool: ToolID?
    /// 名字：这台 Mac 上也有这个账号时是这台 Mac 上的名字，不然是那台机器说的
    var name: String
    /// 那台机器说的名字（它那里的别名）
    var hostName: String

    enum CodingKeys: String, CodingKey {
        case id, tool, name
        case hostName = "host_name"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let id = nonEmpty(c.string(.id)) else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "peer account without id"))
        }
        self.id = id
        tool = c.lossy(ToolID.self, .tool)
        name = c.string(.name) ?? ""
        hostName = nonEmpty(c.string(.hostName)) ?? name
    }
}

/// 另一台机器上的一个模板（那台机器写好的：用哪个工具、在哪个文件夹、带哪些参数；那台机器上的 cca 是旧版时还有哪个账号）。这台 Mac
/// 挑一个起（3.7.10 起另挑一个账号：`RemotePeerLauncher.accounts`），能带开场白的可以带一句。
struct RemotePeerTemplate: Decodable, Sendable, Equatable, Identifiable {
    /// 模板的编号（8 个十六进制字符）：`cca remote launch` 的第二个参数
    var id: String
    /// 模板的摘要（16 个十六进制字符）：起的时候带上（`--digest`），那台机器上改过了就不起
    var digest: String
    var name: String
    var tool: ToolID?
    /// 账号：这台 Mac 上也有这个账号时是这台 Mac 上的名字，不然是那台机器说的。3.7.10 起的那一头写的是给旧版看的（能挑的只有一个时是它，
    /// 几个时是「启动时选账号」）：有 `RemotePeerLauncher.accounts` 时不用它
    var account: String
    /// 那台机器说的名字（它那里的别名）
    var hostAccount: String?
    /// 文件夹（那台机器上的路径，主目录写成 ~）
    var dir: String
    /// 参数拼成的一行（照 shell 的写法；只是给人看）
    var args: String
    /// 能带一句开场白
    var line: Bool

    enum CodingKeys: String, CodingKey {
        case id, digest, name, tool, account, dir, args, line
        case hostAccount = "host_account"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let id = nonEmpty(c.string(.id)), let digest = nonEmpty(c.string(.digest)) else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "peer template without id or digest"))
        }
        self.id = id
        self.digest = digest
        name = c.string(.name) ?? ""
        tool = c.lossy(ToolID.self, .tool)
        account = c.string(.account) ?? ""
        hostAccount = nonEmpty(c.string(.hostAccount))
        dir = c.string(.dir) ?? ""
        args = c.string(.args) ?? ""
        line = c.bool(.line) ?? false
    }
}

/// `cca remote launch <配对的 id> <模板的编号> --digest <摘要> --size <列>x<行> [--line <开场白>] --json`（PROTOCOL 9.5）：
/// 那台机器起好了会话。cca 等它在服务器上登记好（最多 45 秒）才返回；没等到时 `session` 是 nil（会话起来了，过一会儿列表里有）。
struct RemoteLaunchStartDoc: CcaDocument, Equatable {
    static let expectedKind: String? = "remote_launch"

    var version: Int?
    var kind: String?
    /// 新会话的编号
    var id: String
    /// 新会话：`remote list --json` 的 `sessions[]` 里它那一项
    var session: RemoteSession?

    enum CodingKeys: String, CodingKey { case version, kind, id, session }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = c.int(.version)
        kind = c.string(.kind)
        guard let id = nonEmpty(c.string(.id)) else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "remote launch without id"))
        }
        self.id = id
        session = c.lossy(RemoteSession.self, .session)
    }
}
