import Foundation

/// `cca _gui live`（PROTOCOL 2.3）。
struct LiveDoc: CcaDocument, Equatable {
    static let expectedKind: String? = "live"

    var version: Int?
    var kind: String?
    var generatedAt: Double?
    var route: Route
    var routePref: RoutePref?
    var atHome: Bool?
    /// 账号端口现在在哪台机器上（看路由器时是路由器的地址，看本机时是 127.0.0.1，route none 时为 null）；旧版没有这个字段。
    var proxyHost: String?
    var router: LiveRouter?
    var local: LiveLocal?
    var slots: [LiveSlot]
    var summary: LiveSummary?

    enum CodingKeys: String, CodingKey {
        case version, kind
        case generatedAt = "generated_at"
        case route
        case routePref = "route_pref"
        case atHome = "at_home"
        case proxyHost = "proxy_host"
        case router, local, slots, summary
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = c.int(.version)
        kind = c.string(.kind)
        generatedAt = c.double(.generatedAt)
        route = c.lossy(Route.self, .route) ?? .none
        routePref = c.lossy(RoutePref.self, .routePref)
        atHome = c.bool(.atHome)
        proxyHost = c.string(.proxyHost)
        router = c.lossy(LiveRouter.self, .router)
        local = c.lossy(LiveLocal.self, .local)
        slots = (c.lossyArray(LiveSlot.self, .slots) ?? []).sorted { $0.slot < $1.slot }
        summary = c.lossy(LiveSummary.self, .summary)
    }

    func slot(_ n: Int) -> LiveSlot? { slots.first { $0.slot == n } }

    func slot(for ref: AccountRef) -> LiveSlot? { slots.first { $0.ref == ref } }

    var hasDrift: Bool { slots.contains { $0.state == .drift } }
}

struct LiveIssue: Decodable, Sendable, Equatable {
    var code: LiveErrorCode
    var message: String

    enum CodingKeys: String, CodingKey { case code, message }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        code = c.lossy(LiveErrorCode.self, .code) ?? LiveErrorCode(rawValue: "")
        message = c.string(.message) ?? ""
    }
}

struct LiveRouter: Decodable, Sendable, Equatable {
    var configured: Bool
    var host: String?
    var reachable: Bool?
    var backend: Backend?
    var running: Bool?
    var hook: Bool?
    var cron: Bool?
    var error: LiveIssue?

    enum CodingKeys: String, CodingKey { case configured, host, reachable, backend, running, hook, cron, error }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        configured = c.bool(.configured) ?? false
        host = c.string(.host)
        reachable = c.bool(.reachable)
        backend = c.lossy(Backend.self, .backend)
        running = c.bool(.running)
        hook = c.bool(.hook)
        cron = c.bool(.cron)
        error = c.lossy(LiveIssue.self, .error)
    }
}

struct LiveLocal: Decodable, Sendable, Equatable {
    var configured: Bool
    var client: LocalClient?
    var name: String?
    var running: Bool?
    var error: LiveIssue?

    enum CodingKeys: String, CodingKey { case configured, client, name, running, error }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        configured = c.bool(.configured) ?? false
        client = c.lossy(LocalClient.self, .client)
        name = c.string(.name)
        running = c.bool(.running)
        error = c.lossy(LiveIssue.self, .error)
    }
}

struct LiveSlot: Decodable, Sendable, Equatable, Identifiable {
    var slot: Int
    var group: String?
    var port: Int?
    var tool: ToolID?
    var key: String?
    var expected: String?
    var selected: String?
    var inKernel: Bool?
    var listening: Bool?
    var reachable: Bool?
    var state: SlotState

    var id: Int { slot }

    enum CodingKeys: String, CodingKey {
        case slot, group, port, tool, key, expected, selected
        case inKernel = "in_kernel"
        case listening, reachable, state
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
        expected = c.string(.expected)
        selected = c.string(.selected)
        inKernel = c.bool(.inKernel)
        listening = c.bool(.listening)
        reachable = c.bool(.reachable)
        state = c.lossy(SlotState.self, .state) ?? .unknown
    }

    var ref: AccountRef? {
        guard let tool, let key else { return nil }
        return AccountRef(tool: tool, key: key)
    }
}

struct LiveSummary: Decodable, Sendable, Equatable {
    var ok: Int
    var drift: Int
    var closed: Int
    var unknown: Int
    var empty: Int

    enum CodingKeys: String, CodingKey { case ok, drift, closed, unknown, empty }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        ok = c.int(.ok) ?? 0
        drift = c.int(.drift) ?? 0
        closed = c.int(.closed) ?? 0
        unknown = c.int(.unknown) ?? 0
        empty = c.int(.empty) ?? 0
    }
}
