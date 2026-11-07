import Foundation

/// `cca _gui hello`（PROTOCOL 2.1）。
struct HelloDoc: CcaDocument, Equatable {
    static let expectedKind: String? = "hello"

    var version: Int?
    var kind: String?
    var api: Int?
    var ccaVersion: String?
    var devMode: Bool
    /// 缺这个字段时当作已安装（不无故显示安装引导）。
    var installed: Bool
    var selfPath: String?
    var home: String?
    var tools: [ToolID: HelloTool]
    var gui: HelloGUI?

    enum CodingKeys: String, CodingKey {
        case version, kind, api
        case ccaVersion = "cca_version"
        case devMode = "dev_mode"
        case installed
        case selfPath = "self"
        case home, tools, gui
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = c.int(.version)
        kind = c.string(.kind)
        api = c.int(.api)
        ccaVersion = c.string(.ccaVersion)
        devMode = c.bool(.devMode) ?? false
        installed = c.bool(.installed) ?? true
        selfPath = c.string(.selfPath)
        home = c.string(.home)
        var t: [ToolID: HelloTool] = [:]
        for (k, v) in c.lossyDictionary(HelloTool.self, .tools) ?? [:] {
            t[ToolID(rawValue: k)] = v
        }
        tools = t
        gui = c.lossy(HelloGUI.self, .gui)
    }

    /// `cca codex …` 只有这里为真时才能调用（PROTOCOL 1）：缺字段一律当作没启用。
    var codexEnabled: Bool { tools[.codex]?.enabled ?? false }
}

struct HelloTool: Decodable, Sendable, Equatable {
    var enabled: Bool
    var bin: String?

    enum CodingKeys: String, CodingKey { case enabled, bin }

    init(enabled: Bool, bin: String?) {
        self.enabled = enabled
        self.bin = bin
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        enabled = c.bool(.enabled) ?? false
        bin = c.string(.bin)
    }
}

struct HelloGUI: Decodable, Sendable, Equatable {
    var app: String?
    var appVersion: String?
    var stagedVersion: String?

    enum CodingKeys: String, CodingKey {
        case app
        case appVersion = "app_version"
        case stagedVersion = "staged_version"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        app = c.string(.app)
        appVersion = c.string(.appVersion)
        stagedVersion = c.string(.stagedVersion)
    }
}
