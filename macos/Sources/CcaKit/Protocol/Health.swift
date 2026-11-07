import Foundation

/// 出口检测结果（PROTOCOL 3.3）：`_gui state` 里账号的 `health`，也是 `check --json` 里每个账号的 `health`。
struct Health: Decodable, Sendable, Equatable {
    var version: Int?
    var checkedAt: Double?
    var ageSeconds: Double?
    var stale: Bool
    /// 缺字段时当作仍然适用（只在明确为 false 时显示“换了节点”）。
    var applies: Bool
    var source: HealthSource?
    var kind: HealthKind?
    var route: Route?
    var node: String?
    var port: Int?
    var result: HealthResult?
    var ip: String?
    var country: String?
    var org: String?
    var trust: Double?
    var trustLabel: String?
    var tags: [String]
    var restrictedRegion: String?
    var message: String?
    var note: String?
    var leak: Leak?

    enum CodingKeys: String, CodingKey {
        case version
        case checkedAt = "checked_at"
        case ageSeconds = "age_seconds"
        case stale, applies, source, kind, route, node, port, result, ip, country, org, trust
        case trustLabel = "trust_label"
        case tags
        case restrictedRegion = "restricted_region"
        case message, note, leak
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = c.int(.version)
        checkedAt = c.double(.checkedAt)
        ageSeconds = c.double(.ageSeconds)
        stale = c.bool(.stale) ?? false
        applies = c.bool(.applies) ?? true
        source = c.lossy(HealthSource.self, .source)
        kind = c.lossy(HealthKind.self, .kind)
        route = c.lossy(Route.self, .route)
        node = c.string(.node)
        port = c.int(.port)
        result = c.lossy(HealthResult.self, .result)
        ip = c.string(.ip)
        country = c.string(.country)
        org = c.string(.org)
        trust = c.double(.trust)
        trustLabel = c.string(.trustLabel)
        tags = c.lossyArray(String.self, .tags) ?? []
        restrictedRegion = c.string(.restrictedRegion)
        message = c.string(.message)
        note = c.string(.note)
        leak = c.lossy(Leak.self, .leak)
    }
}

struct Leak: Decodable, Sendable, Equatable {
    var dns: LeakState?
    var webrtc: LeakState?
    var line: String?
    var dnsMessage: String?
    var webrtcMessage: String?

    enum CodingKeys: String, CodingKey {
        case dns, webrtc, line
        case dnsMessage = "dns_message"
        case webrtcMessage = "webrtc_message"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        dns = c.lossy(LeakState.self, .dns)
        webrtc = c.lossy(LeakState.self, .webrtc)
        line = c.string(.line)
        dnsMessage = c.string(.dnsMessage)
        webrtcMessage = c.string(.webrtcMessage)
    }

    var hasLeak: Bool { dns == .leak || webrtc == .leak }
}
