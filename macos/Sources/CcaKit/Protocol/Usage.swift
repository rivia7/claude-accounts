import Foundation

/// 一个用量窗口（`five_hour`、`seven_day`……）。时间只用 `resets_epoch`（null 就不显示重置时间）。
struct UsageWindow: Decodable, Sendable, Equatable {
    var utilization: Double?
    var resetsEpoch: Double?
    var reset: Bool

    enum CodingKeys: String, CodingKey {
        case utilization
        case resetsEpoch = "resets_epoch"
        case reset
    }

    init(utilization: Double?, resetsEpoch: Double?, reset: Bool) {
        self.utilization = utilization
        self.resetsEpoch = resetsEpoch
        self.reset = reset
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        utilization = c.double(.utilization)
        resetsEpoch = c.double(.resetsEpoch)
        reset = c.bool(.reset) ?? false
    }

    /// 显示时是否算“已重置”：cca 算好的 `reset`，或者输出之后重置时间已经过了（界面上的快照可能放了一阵子）。
    func isReset(now: Double) -> Bool {
        if reset { return true }
        if let at = resetsEpoch { return at <= now }
        return false
    }
}

/// `model_scoped[]`：按模型的每周额度。
struct ModelWindow: Decodable, Sendable, Equatable {
    var name: String?
    var window: UsageWindow

    enum CodingKeys: String, CodingKey { case name }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = c.string(.name)
        window = try UsageWindow(from: decoder)
    }
}

/// Claude 的额外用量（字段原样来自 Claude Code，宽松解析）。
struct ExtraUsage: Decodable, Sendable, Equatable {
    var isEnabled: Bool?
    var utilization: Double?
    var monthlyLimit: Double?
    var usedCredits: Double?

    enum CodingKeys: String, CodingKey {
        case isEnabled = "is_enabled"
        case utilization
        case monthlyLimit = "monthly_limit"
        case usedCredits = "used_credits"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        isEnabled = c.bool(.isEnabled)
        utilization = c.double(.utilization)
        monthlyLimit = c.double(.monthlyLimit)
        usedCredits = c.double(.usedCredits)
    }
}

/// Codex 的 credits（codex 原样给的，键是驼峰）。
struct Credits: Decodable, Sendable, Equatable {
    var hasCredits: Bool?
    var unlimited: Bool?
    var balance: String?

    enum CodingKeys: String, CodingKey { case hasCredits, unlimited, balance }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        hasCredits = c.bool(.hasCredits)
        unlimited = c.bool(.unlimited)
        balance = c.stringish(.balance)
    }
}

/// Codex 还能重置几次额度（codex 原样给的，键是驼峰）。
struct ResetCredits: Decodable, Sendable, Equatable {
    var availableCount: Int?

    enum CodingKeys: String, CodingKey { case availableCount }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        availableCount = c.int(.availableCount)
    }
}

/// 一个账号的用量（`usage --json` 的 `accounts[]`，也是 `_gui state` 里账号的 `usage`）。
struct UsageEntry: Decodable, Sendable, Equatable {
    var account: String?
    var key: String?
    var email: String?
    var plan: String?
    var source: UsageSource
    var fetchedEpoch: Double?
    var ageSeconds: Double?
    var stale: Bool
    var error: String?
    var errorCode: UsageErrorCode?
    var errorDetail: String?
    var fiveHour: UsageWindow?
    var sevenDay: UsageWindow?
    var sevenDayOpus: UsageWindow?
    var sevenDaySonnet: UsageWindow?
    var sevenDayOauthApps: UsageWindow?
    var modelScoped: [ModelWindow]
    var extraUsage: ExtraUsage?
    var credits: Credits?
    var resetCredits: ResetCredits?

    enum CodingKeys: String, CodingKey {
        case account, key, email, plan, source
        case fetchedEpoch = "fetched_epoch"
        case ageSeconds = "age_seconds"
        case stale, error
        case errorCode = "error_code"
        case errorDetail = "error_detail"
        case fiveHour = "five_hour"
        case sevenDay = "seven_day"
        case sevenDayOpus = "seven_day_opus"
        case sevenDaySonnet = "seven_day_sonnet"
        case sevenDayOauthApps = "seven_day_oauth_apps"
        case modelScoped = "model_scoped"
        case extraUsage = "extra_usage"
        case credits
        case resetCredits = "reset_credits"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        account = c.string(.account)
        key = c.string(.key)
        email = c.string(.email)
        plan = c.string(.plan)
        source = c.lossy(UsageSource.self, .source) ?? .none
        fetchedEpoch = c.double(.fetchedEpoch)
        ageSeconds = c.double(.ageSeconds)
        stale = c.bool(.stale) ?? false
        error = c.string(.error)
        errorCode = c.lossy(UsageErrorCode.self, .errorCode)
        errorDetail = c.stringish(.errorDetail)
        fiveHour = c.lossy(UsageWindow.self, .fiveHour)
        sevenDay = c.lossy(UsageWindow.self, .sevenDay)
        sevenDayOpus = c.lossy(UsageWindow.self, .sevenDayOpus)
        sevenDaySonnet = c.lossy(UsageWindow.self, .sevenDaySonnet)
        sevenDayOauthApps = c.lossy(UsageWindow.self, .sevenDayOauthApps)
        modelScoped = c.lossyArray(ModelWindow.self, .modelScoped) ?? []
        extraUsage = c.lossy(ExtraUsage.self, .extraUsage)
        credits = c.lossy(Credits.self, .credits)
        resetCredits = c.lossy(ResetCredits.self, .resetCredits)
    }

    /// 有没有可以显示的数据（`source` 为 none 时没有）。
    var hasData: Bool { source != .none && (fiveHour != nil || sevenDay != nil) }
}

/// `cca [codex] usage [账号] --json`（PROTOCOL 3.1）。
struct UsageDoc: CcaDocument, Equatable {
    static let expectedKind: String? = "usage"

    var version: Int?
    var kind: String?
    var tool: ToolID?
    var generatedEpoch: Double?
    var accounts: [UsageEntry]
    var skippedAPIAccounts: [String]

    enum CodingKeys: String, CodingKey {
        case version, kind, tool
        case generatedEpoch = "generated_epoch"
        case accounts
        case skippedAPIAccounts = "skipped_api_accounts"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = c.int(.version)
        kind = c.string(.kind)
        tool = c.lossy(ToolID.self, .tool)
        generatedEpoch = c.double(.generatedEpoch)
        accounts = c.lossyArray(UsageEntry.self, .accounts) ?? []
        skippedAPIAccounts = c.lossyArray(String.self, .skippedAPIAccounts) ?? []
    }
}
