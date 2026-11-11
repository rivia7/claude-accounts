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

/// `model_scoped[]`：按模型的每周额度（Claude 的 Fable；Codex 另外计的额度）。Claude 的还带着它占本周总额度多少（cca 认出来的）。
struct ModelWindow: Decodable, Sendable, Equatable {
    var name: String?
    var window: UsageWindow
    /// 最多能用本周总额度的百分之几（Fable 是 50）；cca 认不出时为 nil
    var allowancePercentOfWeekly: Double?
    /// 已经用掉本周总额度的百分之几
    var usedPercentOfWeekly: Double?

    enum CodingKeys: String, CodingKey {
        case name
        case allowancePercentOfWeekly = "allowance_percent_of_weekly"
        case usedPercentOfWeekly = "used_percent_of_weekly"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = c.string(.name)
        window = try UsageWindow(from: decoder)
        allowancePercentOfWeekly = c.double(.allowancePercentOfWeekly)
        usedPercentOfWeekly = c.double(.usedPercentOfWeekly)
    }
}

/// `use_first`：本周额度快重置了、还剩不少，建议先用这个账号（PROTOCOL 3.1）。规则（重置前 24 小时、还剩至少 20%）由 cca 定；
/// App 只拿当前时间和 `[from_epoch, until_epoch)` 比——快照可能放了好几分钟，到了重置时间提示就该消失。
struct UseFirst: Decodable, Sendable, Equatable {
    /// 本周还剩百分之几（整数，和显示的百分比一致）
    var leftPercent: Int
    var fromEpoch: Double
    /// 本周的重置时间
    var untilEpoch: Double

    enum CodingKeys: String, CodingKey {
        case leftPercent = "left_percent"
        case fromEpoch = "from_epoch"
        case untilEpoch = "until_epoch"
    }

    init(leftPercent: Int, fromEpoch: Double, untilEpoch: Double) {
        self.leftPercent = leftPercent
        self.fromEpoch = fromEpoch
        self.untilEpoch = untilEpoch
    }

    /// 缺字段、类型不对、区间是空的都算没有（lossy 得到 nil）。
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let left = c.int(.leftPercent), let from = c.double(.fromEpoch), let until = c.double(.untilEpoch), from < until else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "use_first"))
        }
        leftPercent = left
        fromEpoch = from
        untilEpoch = until
    }

    /// 现在该不该提示。
    func isActive(now: Double) -> Bool { fromEpoch <= now && now < untilEpoch }
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

/// Codex 还能重置几次额度（codex 原样给的 `availableCount`），和 cca 算好的还能用的里面最早什么时候过期（PROTOCOL 3.1）。
/// 每一次的明细（`credits[]`）App 不用。
struct ResetCredits: Decodable, Sendable, Equatable {
    var availableCount: Int?
    /// 最早过期的那一次什么时候过期（Unix 秒）；都不过期、没有明细时为 nil
    var nextExpiresEpoch: Double?

    enum CodingKeys: String, CodingKey {
        case availableCount
        case nextExpiresEpoch = "next_expires_epoch"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        availableCount = c.int(.availableCount)
        nextExpiresEpoch = c.double(.nextExpiresEpoch)
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
    /// Claude Code 登录时记下的套餐和账号现在的对不上（升降级以后要重新登录才会更新）：记下的那个的叫法（“Pro”）。
    /// cca 比好的，App 不自己比；对得上、不知道、Codex 账号、旧版 cca 都是 nil。
    var outdatedLoginPlan: String?
    /// 本周额度快重置了、还剩不少：建议先用（旧版 cca 没有，是 nil）
    var useFirst: UseFirst?

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
        case outdatedLoginPlan = "outdated_login_plan"
        case useFirst = "use_first"
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
        outdatedLoginPlan = nonEmpty(c.string(.outdatedLoginPlan))
        useFirst = c.lossy(UseFirst.self, .useFirst)
    }

    /// 有没有可以显示的数据（`source` 为 none 时没有）。
    var hasData: Bool { source != .none && (fiveHour != nil || sevenDay != nil) }

    /// 这个账号没有 5 小时的限制（PROTOCOL 3.1：查到了用量、`seven_day` 有而 `five_hour` 是 null。Claude 的 Ultra 这类套餐、
    /// ChatGPT 的 Pro 500；Claude 说限制没生效的 cca 也给 null）：侧栏、菜单栏面板、总览卡片、账号页都不画 5 小时那一行。
    var noFiveHourLimit: Bool { source != .none && fiveHour == nil && sevenDay != nil }
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

/// `usage_weeks`（PROTOCOL 2.2）：最近几周每周用到这一周额度的百分之几。Claude、OpenAI 都不给过去几周的额度，是 cca 每次查用量时
/// 记下的；哪一周算用完、算没记全、统计怎么算都由 cca 定，App 照着画。旧版 cca 没有，是 nil。
struct UsageWeeks: Decodable, Sendable, Equatable {
    /// 从什么时候开始记的（最早一次记下的时间）
    var sinceEpoch: Double?
    /// 按时间排好，最后一项可能是还没过完的这一周
    var weeks: [UsageWeek]
    var summary: UsageWeeksSummary

    enum CodingKeys: String, CodingKey {
        case sinceEpoch = "since_epoch"
        case weeks, summary
    }

    init(sinceEpoch: Double?, weeks: [UsageWeek], summary: UsageWeeksSummary) {
        self.sinceEpoch = sinceEpoch
        self.weeks = weeks
        self.summary = summary
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        sinceEpoch = c.double(.sinceEpoch)
        weeks = (c.lossyArray(UsageWeek.self, .weeks) ?? []).sorted { $0.endEpoch < $1.endEpoch }
        summary = c.lossy(UsageWeeksSummary.self, .summary) ?? UsageWeeksSummary()
    }

    /// 还没过完的这一周（cca 标的 `current`）
    var current: UsageWeek? { weeks.last(where: \.current) }
}

/// 一周（`usage_weeks.weeks[]`）：从 `start_epoch` 到 `end_epoch`（中间重置后重新计时的，到下一周开始时为止）。
struct UsageWeek: Decodable, Sendable, Equatable, Identifiable {
    var startEpoch: Double
    var endEpoch: Double
    /// 用到这一周额度的百分之几：中间重置过的把重置前的加上，可能超过 100
    var usedPercent: Double
    /// 中间重置过几次（重置额度、官方重置）
    var resets: Int
    /// 最后一次查到的时间
    var lastEpoch: Double?
    /// 这一周还没过完
    var current: Bool
    /// 结束前一天里没查到过：后来的用量没记下，实际可能更多
    var partial: Bool

    var id: Double { endEpoch }

    enum CodingKeys: String, CodingKey {
        case startEpoch = "start_epoch"
        case endEpoch = "end_epoch"
        case usedPercent = "used_percent"
        case resets
        case lastEpoch = "last_epoch"
        case current, partial
    }

    init(startEpoch: Double, endEpoch: Double, usedPercent: Double, resets: Int = 0, lastEpoch: Double? = nil,
         current: Bool = false, partial: Bool = false) {
        self.startEpoch = startEpoch
        self.endEpoch = endEpoch
        self.usedPercent = usedPercent
        self.resets = resets
        self.lastEpoch = lastEpoch
        self.current = current
        self.partial = partial
    }

    /// 缺时间、百分比，或者区间是空的：这一项不要（lossyArray 跳过）
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let start = c.double(.startEpoch), let end = c.double(.endEpoch), let used = c.double(.usedPercent), start < end else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "usage week"))
        }
        startEpoch = start
        endEpoch = end
        usedPercent = max(used, 0)
        resets = max(c.int(.resets) ?? 0, 0)
        lastEpoch = c.double(.lastEpoch)
        current = c.bool(.current) ?? false
        partial = c.bool(.partial) ?? false
    }
}

/// `usage_weeks.summary`：过完的几周（不算还没过完的这一周）的统计，cca 算好的（百分比都是四舍五入后的整数）。
struct UsageWeeksSummary: Decodable, Sendable, Equatable {
    /// 算进来的有几周
    var weeks: Int = 0
    /// 平均每周用到百分之几；一周都没有时 nil
    var averagePercent: Int?
    /// 用完（100% 及以上）的有几周
    var maxed: Int = 0
    /// 没用到一半的有几周
    var underHalf: Int = 0

    enum CodingKeys: String, CodingKey {
        case weeks, maxed
        case averagePercent = "average_percent"
        case underHalf = "under_half"
    }

    init(weeks: Int = 0, averagePercent: Int? = nil, maxed: Int = 0, underHalf: Int = 0) {
        self.weeks = weeks
        self.averagePercent = averagePercent
        self.maxed = maxed
        self.underHalf = underHalf
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        weeks = max(c.int(.weeks) ?? 0, 0)
        averagePercent = c.int(.averagePercent)
        maxed = max(c.int(.maxed) ?? 0, 0)
        underHalf = max(c.int(.underHalf) ?? 0, 0)
    }
}
