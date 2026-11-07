import Foundation

/// `cca [codex] check [账号] --json`（PROTOCOL 3.2）。
struct CheckDoc: CcaDocument, Equatable {
    static let expectedKind: String? = "check"

    var version: Int?
    var kind: String?
    var tool: ToolID?
    var generatedAt: Double?
    /// `router` | `local` | null。
    var route: Route?
    var accounts: [CheckAccount]
    var sharedExitIPs: [String]

    enum CodingKeys: String, CodingKey {
        case version, kind, tool
        case generatedAt = "generated_at"
        case route, accounts
        case sharedExitIPs = "shared_exit_ips"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = c.int(.version)
        kind = c.string(.kind)
        tool = c.lossy(ToolID.self, .tool)
        generatedAt = c.double(.generatedAt)
        route = c.lossy(Route.self, .route)
        accounts = c.lossyArray(CheckAccount.self, .accounts) ?? []
        sharedExitIPs = c.lossyArray(String.self, .sharedExitIPs) ?? []
    }
}

struct CheckAccount: Decodable, Sendable, Equatable {
    var key: String
    var name: String?
    /// 直连的第三方 API 账号没有节点和端口（null）。
    var node: String?
    var port: Int?
    var health: Health?

    enum CodingKeys: String, CodingKey { case key, name, node, port, health }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let key = c.string(.key), !key.isEmpty else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "check account without key"))
        }
        self.key = key
        name = c.string(.name)
        node = c.string(.node)
        port = c.int(.port)
        health = c.lossy(Health.self, .health)
    }
}
