import Foundation

// 折合金额（PROTOCOL 2.2 的 `cost`、`cost_total`，3.5 的 `cca cost sync --json`）：账号用 Claude Code / Codex / agy 干的活，
// 按官方 API 的标价折成美元，**不是实际花费**。柱的高低、哪一根是浅色（partial）、平均和合计都是 cca 算好的，App 只排版、不重算。

/// `hello.cost`：App 认识的版本（不认识的版本当作没有，不显示折合金额、不调 `cost sync`）。
enum CostAPI {
    static let version = 1
}

/// `weeks[].basis`：这一项怎么切的：本周额度的窗口（和每周用量的柱子对得上）/ 本地时间周一零点。
struct CostBasis: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let quota = CostBasis(rawValue: "quota")
    static let calendar = CostBasis(rawValue: "calendar")
    static let allKnown: [CostBasis] = [.quota, .calendar]
}

/// `cost.sync.state`：设备间同步的状态。不认识的值当 `error` 显示（`shown`；3.6.0 以前的 `old_server`：维护者的服务器还不支持，
/// 也照这样说 cca 给的原因）。
struct CostSyncState: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    /// 同步过（`checked_epoch` 是最后一次成功的时间）
    static let ok = CostSyncState(rawValue: "ok")
    /// 会同步，还没同步过
    static let pending = CostSyncState(rawValue: "pending")
    /// 只有这台设备：账号不经哪台服务器同步（不是哪个 Team 给的），或者该走的那一台这台设备用不了
    static let localOnly = CostSyncState(rawValue: "local_only")
    /// config.json 里写了 `"presence": false`
    static let off = CostSyncState(rawValue: "off")
    /// 这台设备上没登录这个账号：解不开别的设备的数据，显示上次同步来的（3.6.5 及以前的 cca；3.6.6 起 Team 给的账号不用登录也同步）
    static let notLoggedIn = CostSyncState(rawValue: "not_logged_in")
    /// 最近一次没同步成（`message` 是原因），显示上次同步来的
    static let error = CostSyncState(rawValue: "error")
    static let allKnown: [CostSyncState] = [.ok, .pending, .localOnly, .off, .notLoggedIn, .error]

    /// 界面按哪种说：不认识的值当 error
    var shown: CostSyncState { isKnown ? self : .error }
}

/// `cost_total.scan.state` / `cost sync` 的 `scan.state`：统计过 / 正在统计（第一次要把会话记录全读一遍）/ 还没统计过。
/// 不认识的值当作统计过（照常显示）。
struct CostScanState: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let ready = CostScanState(rawValue: "ready")
    static let scanning = CostScanState(rawValue: "scanning")
    static let never = CostScanState(rawValue: "never")
    static let allKnown: [CostScanState] = [.ready, .scanning, .never]
}

/// `devices[].diag.why[].code`、`cost_total.unattributed.why[].code`：为什么有 Claude 的回复认不出是哪个账号的。
/// 不认识的值当 `unknown` 说（`shown`）。
struct CostWhyCode: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    /// Claude Code 2.1.317 以前的版本：会话不带账号信息，又不是经 cca 启动的（经 cca 启动的，cca 记下了是哪个账号）
    static let oldClaudeCode = CostWhyCode(rawValue: "old_claude_code")
    /// Claude App 里的 Code 会话：从来不带账号信息
    static let claudeApp = CostWhyCode(rawValue: "claude_app")
    /// 别的：新版 Claude Code 也没带账号信息（登录的不是 cca 里的账号、不是经 cca 启动的……）
    static let unknown = CostWhyCode(rawValue: "unknown")
    static let allKnown: [CostWhyCode] = [.oldClaudeCode, .claudeApp, .unknown]

    /// 界面按哪种说：不认识的值当 unknown
    var shown: CostWhyCode { isKnown ? self : .unknown }
}

/// 有回复认不出是哪个账号的一个原因（`devices[].diag.why[]`、`cost_total.unattributed.why[]`）：最近一次是什么时候、那时的
/// Claude Code 版本。没有条数（别的设备传来的时间只到天）
struct CostWhy: Decodable, Sendable, Equatable {
    var code: CostWhyCode
    var lastEpoch: Double
    var claudeCode: String?

    enum CodingKeys: String, CodingKey {
        case code
        case lastEpoch = "last_epoch"
        case claudeCode = "claude_code"
    }

    init(code: CostWhyCode, lastEpoch: Double, claudeCode: String? = nil) {
        self.code = code
        self.lastEpoch = lastEpoch
        self.claudeCode = claudeCode
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let code = nonEmpty(c.string(.code)), let at = c.double(.lastEpoch) else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "cost why without code/last_epoch"))
        }
        self.code = CostWhyCode(rawValue: code)
        lastEpoch = at
        claudeCode = nonEmpty(c.string(.claudeCode))
    }
}

/// `devices[].diag`：一台设备的诊断（别的设备传来的；这台设备的是本机统计的）。旧版 cca 的设备没有（null）
struct CostDiag: Decodable, Sendable, Equatable {
    /// 那台设备的 cca 版本
    var cca: String?
    /// 那台设备命令行里最近用的 Claude Code 版本
    var claudeCode: String?
    /// claudeCode 还是不带账号信息的旧版本：要更新（cca 比的版本，App 不比）
    var claudeCodeOld: Bool
    /// 最近 30 天里有回复认不出账号的原因（空的：都认得出）
    var why: [CostWhy]

    enum CodingKeys: String, CodingKey {
        case cca, why
        case claudeCode = "claude_code"
        case claudeCodeOld = "claude_code_old"
    }

    init(cca: String? = nil, claudeCode: String? = nil, claudeCodeOld: Bool = false, why: [CostWhy] = []) {
        self.cca = cca
        self.claudeCode = claudeCode
        self.claudeCodeOld = claudeCodeOld
        self.why = why
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        cca = nonEmpty(c.string(.cca))
        claudeCode = nonEmpty(c.string(.claudeCode))
        claudeCodeOld = c.bool(.claudeCodeOld) ?? false
        why = c.lossyArray(CostWhy.self, .why) ?? []
    }
}

/// `cost sync --json` 的 `sync.state`：这一次和别的设备同步得怎么样。
struct CostSyncRunState: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let ok = CostSyncRunState(rawValue: "ok")
    /// 不用同步：没有经服务器同步的账号、这台设备用不了哪台服务器
    static let skipped = CostSyncRunState(rawValue: "skipped")
    static let error = CostSyncRunState(rawValue: "error")
    /// 上次失败了，还没到重试的时候（`--now` 不等）
    static let waiting = CostSyncRunState(rawValue: "waiting")
    static let allKnown: [CostSyncRunState] = [.ok, .skipped, .error, .waiting]
}

/// 一项的 token 数：输入（不含缓存）、输出、缓存读、缓存写。只给账号合计，不给每台设备。
struct CostTokens: Decodable, Sendable, Equatable {
    var input = 0
    var output = 0
    var cacheRead = 0
    var cacheWrite = 0

    enum CodingKeys: String, CodingKey {
        case input, output
        case cacheRead = "cache_read"
        case cacheWrite = "cache_write"
    }

    init(input: Int = 0, output: Int = 0, cacheRead: Int = 0, cacheWrite: Int = 0) {
        self.input = input
        self.output = output
        self.cacheRead = cacheRead
        self.cacheWrite = cacheWrite
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        input = max(c.int(.input) ?? 0, 0)
        output = max(c.int(.output) ?? 0, 0)
        cacheRead = max(c.int(.cacheRead) ?? 0, 0)
        cacheWrite = max(c.int(.cacheWrite) ?? 0, 0)
    }

    var total: Int { input + output + cacheRead + cacheWrite }
}

/// 按模型分开的一段（`models[].weeks`：列出的周期加起来；`models[].days`：30 天加起来）。
struct CostModelRange: Decodable, Sendable, Equatable {
    var usd = 0.0
    var requests = 0
    /// 其中没有公开价格的请求数（没有标价的模型：金额是 0，请求都在这里）
    var unpricedRequests = 0
    /// 只给账号的（设备、cost_total 的没有）
    var tokens: CostTokens?
    /// 这一段的13.7.6 起，0～1，cca 算的）；没有输入、旧版 cca 没有时为 nil
    var cacheHit: Double?

    enum CodingKeys: String, CodingKey {
        case usd, requests, tokens
        case unpricedRequests = "unpriced_requests"
        case cacheHit = "cache_hit"
    }

    init(usd: Double = 0, requests: Int = 0, unpricedRequests: Int = 0, tokens: CostTokens? = nil, cacheHit: Double? = nil) {
        self.usd = usd
        self.requests = requests
        self.unpricedRequests = unpricedRequests
        self.tokens = tokens
        self.cacheHit = cacheHit
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        usd = max(c.double(.usd) ?? 0, 0)
        requests = max(c.int(.requests) ?? 0, 0)
        unpricedRequests = max(c.int(.unpricedRequests) ?? 0, 0)
        tokens = c.lossy(CostTokens.self, .tokens)
        cacheHit = c.double(.cacheHit).map { min(max($0, 0), 1) }
    }

    /// 这一段里没有用量（没有请求、没有金额）
    var isEmpty: Bool { requests == 0 && usd == 0 }
    /// 都是没有公开价格的请求（没算钱）
    var allUnpriced: Bool { usd == 0 && requests > 0 && unpricedRequests >= requests }
}

/// `models[]` 的一项（PROTOCOL 2.2，3.6.8 起）：一个模型在列出的周期、30 天里折了多少。cca 排好的：按列出的周期的金额从多到少，
/// 没有按模型记下的那一项（`model` 是 null）在最后
struct CostModelRow: Decodable, Sendable, Equatable, Identifiable {
    /// 模型名（会话记录里的写法；空的：会话记录里没写）。nil：没有按模型记下的那一部分（以前的版本统计的、旧版 cca 的设备传来的）
    var model: String?
    var weeks: CostModelRange
    var days: CostModelRange
    /// cca 的价格表里还没有、按同系列别的模型的标价估算的新模型：按的是哪一个（3.6.14 起）。nil：照它自己的标价，或者没有公开价格
    var pricedAs: String?

    var id: String { model.map { "m:" + $0 } ?? "rest" }

    enum CodingKeys: String, CodingKey {
        case model, weeks, days
        case pricedAs = "priced_as"
    }

    init(model: String?, weeks: CostModelRange = CostModelRange(), days: CostModelRange = CostModelRange(), pricedAs: String? = nil) {
        self.model = model
        self.weeks = weeks
        self.days = days
        self.pricedAs = pricedAs
    }

    /// `model` 要么是字符串（模型名），要么是 null（没有按模型记下的）；缺了、别的类型：这一项不要
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        if (try? c.decodeNil(forKey: .model)) == true {
            model = nil
        } else if let m = c.string(.model) {
            model = m
        } else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "cost model row without model"))
        }
        weeks = c.lossy(CostModelRange.self, .weeks) ?? CostModelRange()
        days = c.lossy(CostModelRange.self, .days) ?? CostModelRange()
        pricedAs = model == nil ? nil : c.string(.pricedAs).flatMap { $0.isEmpty ? nil : $0 }
    }

    /// 按周看是列出的周期的那一段，按天看是 30 天的
    func range(_ g: CostGranularity) -> CostModelRange { g == .week ? weeks : days }
}

/// 一个周期（`weeks[]`）或者一天（`days[]`、`cost_total.days[]`）：从 `start_epoch` 到 `end_epoch`。
struct CostPeriod: Decodable, Sendable, Equatable, Identifiable {
    var startEpoch: Double
    var endEpoch: Double
    /// 只有 weeks 有：本周额度的窗口 / 本地时间周一零点切的
    var basis: CostBasis?
    /// 只有 days 有：本地日期（YYYY-MM-DD），只拿来显示
    var date: String?
    /// 还没过完的那一项（最后一项）
    var current: Bool
    /// 这一项开始时还没有记录（`since_epoch` 落在它里面或者后面）：数字可能偏小，画成浅色
    var partial: Bool
    var usd: Double
    var requests: Int
    /// 其中没有公开价格的模型的请求数（只计数、不算钱）
    var unpricedRequests: Int
    var tokens: CostTokens?

    var id: Double { endEpoch }

    enum CodingKeys: String, CodingKey {
        case startEpoch = "start_epoch"
        case endEpoch = "end_epoch"
        case basis, date, current, partial, usd, requests
        case unpricedRequests = "unpriced_requests"
        case tokens
    }

    init(startEpoch: Double, endEpoch: Double, basis: CostBasis? = nil, date: String? = nil, current: Bool = false,
         partial: Bool = false, usd: Double, requests: Int = 0, unpricedRequests: Int = 0, tokens: CostTokens? = nil) {
        self.startEpoch = startEpoch
        self.endEpoch = endEpoch
        self.basis = basis
        self.date = date
        self.current = current
        self.partial = partial
        self.usd = usd
        self.requests = requests
        self.unpricedRequests = unpricedRequests
        self.tokens = tokens
    }

    /// 缺时间或者区间是空的：这一项不要（金额缺了当作 0：和设备的数组一一对应，不能丢）
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let start = c.double(.startEpoch), let end = c.double(.endEpoch), start < end else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "cost period"))
        }
        startEpoch = start
        endEpoch = end
        basis = c.lossy(CostBasis.self, .basis)
        date = nonEmpty(c.string(.date))
        current = c.bool(.current) ?? false
        partial = c.bool(.partial) ?? false
        usd = max(c.double(.usd) ?? 0, 0)
        requests = max(c.int(.requests) ?? 0, 0)
        unpricedRequests = max(c.int(.unpricedRequests) ?? 0, 0)
        tokens = c.lossy(CostTokens.self, .tokens)
    }
}

/// 统计：账号的 `summary`、设备的 `summary`（没有 `requests`）、`cost_total` 顶层、`me` 和 `by_tool` 的几项（没有 `average_weeks`、`average_days`、`requests`）。
struct CostSummary: Decodable, Sendable, Equatable {
    /// 列出的周期加起来
    var weeksUSD = 0.0
    /// 当前那一项
    var currentUSD = 0.0
    /// 过完的、不是 partial、至少 6 天的周期平均；一项都没有为 nil
    var averageUSD: Double?
    /// 平均算了几项
    var averageWeeks = 0
    /// 30 天加起来
    var daysUSD = 0.0
    /// 今天
    var todayUSD = 0.0
    /// 过完的、不是 partial 的天平均（今天不算）；一天都没有为 nil（旧版 cca 没有，也是 nil）
    var averageDayUSD: Double?
    /// 按天的平均算了几天
    var averageDays = 0
    var requests = 0
    var unpricedRequests = 0
    /// 13.7.6 起，0～1，cca 算的）：列出的周期、30 天里输入的 token 有多少是读的缓存。没有输入、旧版 cca 没有时为 nil
    var weeksCacheHit: Double?
    var daysCacheHit: Double?

    enum CodingKeys: String, CodingKey {
        case weeksUSD = "weeks_usd"
        case currentUSD = "current_usd"
        case averageUSD = "average_usd"
        case averageWeeks = "average_weeks"
        case daysUSD = "days_usd"
        case todayUSD = "today_usd"
        case averageDayUSD = "average_day_usd"
        case averageDays = "average_days"
        case requests
        case unpricedRequests = "unpriced_requests"
        case weeksCacheHit = "weeks_cache_hit"
        case daysCacheHit = "days_cache_hit"
    }

    init(weeksUSD: Double = 0, currentUSD: Double = 0, averageUSD: Double? = nil, averageWeeks: Int = 0, daysUSD: Double = 0,
         todayUSD: Double = 0, averageDayUSD: Double? = nil, averageDays: Int = 0, requests: Int = 0, unpricedRequests: Int = 0,
         weeksCacheHit: Double? = nil, daysCacheHit: Double? = nil) {
        self.weeksUSD = weeksUSD
        self.currentUSD = currentUSD
        self.averageUSD = averageUSD
        self.averageWeeks = averageWeeks
        self.daysUSD = daysUSD
        self.todayUSD = todayUSD
        self.averageDayUSD = averageDayUSD
        self.averageDays = averageDays
        self.requests = requests
        self.unpricedRequests = unpricedRequests
        self.weeksCacheHit = weeksCacheHit
        self.daysCacheHit = daysCacheHit
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        weeksUSD = max(c.double(.weeksUSD) ?? 0, 0)
        currentUSD = max(c.double(.currentUSD) ?? 0, 0)
        averageUSD = c.double(.averageUSD).map { max($0, 0) }
        averageWeeks = max(c.int(.averageWeeks) ?? 0, 0)
        daysUSD = max(c.double(.daysUSD) ?? 0, 0)
        todayUSD = max(c.double(.todayUSD) ?? 0, 0)
        averageDayUSD = c.double(.averageDayUSD).map { max($0, 0) }
        averageDays = max(c.int(.averageDays) ?? 0, 0)
        requests = max(c.int(.requests) ?? 0, 0)
        unpricedRequests = max(c.int(.unpricedRequests) ?? 0, 0)
        weeksCacheHit = c.double(.weeksCacheHit).map { min(max($0, 0), 1) }
        daysCacheHit = c.double(.daysCacheHit).map { min(max($0, 0), 1) }
    }
}

/// 一台有记录的设备（这台设备在最前，别的按 30 天的金额从多到少，cca 排好的）。
/// `weeksUSD`、`daysUSD` 和账号的 `weeks`、`days` 一一对应（解码时跟着那两个数组一起排好、去掉坏掉的项；缺的当作 0）。
struct CostDevice: Sendable, Equatable, Identifiable {
    /// 设备统计里的设备指纹（`SHA256:…`）；这台设备没开设备统计时是 `local`
    var id: String
    /// 设备统计里的名字（Team 的管理员给这台设备起的名字）；这台设备没开设备统计时是主机名
    var name: String
    var isMe: Bool
    /// 它的 key 作废了（被 Team 的管理员移除了：数字是那以前同步来的，照样算进合计）
    var revoked: Bool
    /// 这些数字是什么时候的（别的设备最后一次同步上来的时间；这台设备是最后一次统计的时间）
    var syncedEpoch: Double?
    /// 超过 36 小时没同步上来：之后用的可能还没算进来
    var stale: Bool
    var weeksUSD: [Double]
    var daysUSD: [Double]
    var summary: CostSummary
    /// 那台设备的诊断：有没有回复认不出账号、为什么（旧版 cca 的设备没有）。最近有认不出的设备没有这个账号的记录也列出来（数字都是 0）
    var diag: CostDiag? = nil
    /// 那台设备用过这个账号（cca 给的 `used`）：只为诊断列出来的设备是 false；旧版 cca 没有这一项，当作用过
    var used = true
    /// 那台设备按模型分开的（没有 tokens）；旧版 cca 没有
    var models: [CostModelRow]? = nil
}

/// `devices[]` 原样解码（数组按位置留着，交给 AccountCost 和 weeks、days 对齐）。
private struct RawCostDevice: Decodable {
    var id: String
    var name: String
    var isMe: Bool
    var revoked: Bool
    var syncedEpoch: Double?
    var stale: Bool
    var weeks: [Double?]
    var days: [Double?]
    var summary: CostSummary
    var diag: CostDiag?
    var used: Bool
    var models: [CostModelRow]?

    enum CodingKeys: String, CodingKey {
        case id, name, me, revoked, stale, summary, diag, used, models
        case syncedEpoch = "synced_epoch"
        case weeksUSD = "weeks_usd"
        case daysUSD = "days_usd"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let id = nonEmpty(c.string(.id)), name = nonEmpty(c.string(.name))
        guard let key = id ?? name else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "cost device without id/name"))
        }
        self.id = key
        self.name = name ?? key
        isMe = c.bool(.me) ?? false
        revoked = c.bool(.revoked) ?? false
        syncedEpoch = c.double(.syncedEpoch)
        stale = c.bool(.stale) ?? false
        weeks = (c.positional(CostNumber.self, .weeksUSD) ?? []).map { $0?.value }
        days = (c.positional(CostNumber.self, .daysUSD) ?? []).map { $0?.value }
        summary = c.lossy(CostSummary.self, .summary) ?? CostSummary()
        diag = c.lossy(CostDiag.self, .diag)
        used = c.bool(.used) ?? true
        models = c.lossyArray(CostModelRow.self, .models)
    }

    /// 按账号的 weeks、days 解码后留下的下标取值（缺的、坏的是 0）
    func aligned(weeks w: [Int], days d: [Int]) -> CostDevice {
        func pick(_ raw: [Double?], _ order: [Int]) -> [Double] {
            order.map { i in i < raw.count ? max(raw[i] ?? 0, 0) : 0 }
        }
        return CostDevice(id: id, name: name, isMe: isMe, revoked: revoked, syncedEpoch: syncedEpoch, stale: stale,
                          weeksUSD: pick(weeks, w), daysUSD: pick(days, d), summary: summary, diag: diag, used: used, models: models)
    }
}

/// 金额数组的一项：数字（或者写成字符串的数字），别的算坏掉。
private struct CostNumber: Decodable {
    let value: Double

    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if let d = try? c.decode(Double.self), d.isFinite {
            value = d
        } else if let s = try? c.decode(String.self), let d = Double(s), d.isFinite {
            value = d
        } else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "not a number"))
        }
    }
}

private extension KeyedDecodingContainer {
    /// 数组：每一项都留着位置（坏掉的、null 是 nil），和别的数组一一对应时用；缺或不是数组返回 nil。
    func positional<T: Decodable>(_ type: T.Type, _ key: Key) -> [T?]? {
        guard contains(key), var c = try? nestedUnkeyedContainer(forKey: key) else { return nil }
        var out: [T?] = []
        while !c.isAtEnd {
            let before = c.currentIndex
            if (try? c.decodeNil()) == true {
                out.append(nil)
                continue
            }
            if let v = try? c.decode(T.self) {
                out.append(v)
            } else {
                _ = try? c.decode(SkipValue.self)
                out.append(nil)
            }
            if c.currentIndex == before { break }   // 防止解码器卡住
        }
        return out
    }
}

/// 一台 30 天内用过这个账号、却没同步过金额的设备（多半是 cca 太旧）：合计里不含它的用量。
struct CostMissingDevice: Decodable, Sendable, Equatable {
    var name: String
    /// 它最近一次用这个账号的时间
    var usedEpoch: Double?

    enum CodingKeys: String, CodingKey {
        case name
        case usedEpoch = "used_epoch"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let name = nonEmpty(c.string(.name)) else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "missing device without name"))
        }
        self.name = name
        usedEpoch = c.double(.usedEpoch)
    }
}

/// `cost.sync`：设备间同步的状态。
struct CostSync: Decodable, Sendable, Equatable {
    var state: CostSyncState
    /// 给人看的原因（error 时）
    var message: String?
    /// 最后一次同步成功的时间
    var checkedEpoch: Double?
    /// 经哪台服务器同步（和账号的 `hub` 一样；`local_only`、`off`、不认识的值（3.6.0 以前的 `maintainer`）、旧版 cca 没有这个字段时是 nil）
    var hub: HubKind?
    var missing: [CostMissingDevice]

    enum CodingKeys: String, CodingKey {
        case state, message, missing, hub
        case checkedEpoch = "checked_epoch"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        state = c.lossy(CostSyncState.self, .state) ?? CostSyncState(rawValue: "")
        message = nonEmpty(c.string(.message))
        checkedEpoch = c.double(.checkedEpoch)
        hub = c.lossy(HubKind.self, .hub).flatMap { $0.isKnown ? $0 : nil }
        missing = c.lossyArray(CostMissingDevice.self, .missing) ?? []
    }
}

/// 一个账号的折合金额（`tools[].accounts[].cost`）：只给 `can.cost` 为真的账号，还没有任何记录时为 null。
struct AccountCost: Decodable, Sendable, Equatable {
    var currency: String
    /// 核实价格的日期（YYYY-MM-DD）
    var pricesDate: String?
    /// 这个账号最早的一条记录（所有设备里最早的）
    var sinceEpoch: Double?
    /// 最近 12 周的周期，按时间排好、首尾相接，最多 13 项；最后一项可能是还没过完的
    var weeks: [CostPeriod]
    /// 最近 30 个本地日历日，最后一项是今天
    var days: [CostPeriod]
    var summary: CostSummary
    var devices: [CostDevice]
    /// 没有公开价格的模型（只计了请求数）
    var unpricedModels: [String]
    var sync: CostSync?
    /// 按模型分开的（带 tokens）；旧版 cca 没有（不显示按模型的那一块）
    var models: [CostModelRow]?

    enum CodingKeys: String, CodingKey {
        case currency, weeks, days, summary, devices, sync, models
        case pricesDate = "prices_date"
        case sinceEpoch = "since_epoch"
        case unpricedModels = "unpriced_models"
    }

    init(currency: String = "USD", pricesDate: String? = nil, sinceEpoch: Double? = nil, weeks: [CostPeriod] = [],
         days: [CostPeriod] = [], summary: CostSummary = CostSummary(), devices: [CostDevice] = [],
         unpricedModels: [String] = [], sync: CostSync? = nil, models: [CostModelRow]? = nil) {
        self.currency = currency
        self.pricesDate = pricesDate
        self.sinceEpoch = sinceEpoch
        self.weeks = weeks
        self.days = days
        self.summary = summary
        self.devices = devices
        self.unpricedModels = unpricedModels
        self.sync = sync
        self.models = models
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        currency = nonEmpty(c.string(.currency)) ?? "USD"
        pricesDate = nonEmpty(c.string(.pricesDate))
        sinceEpoch = c.double(.sinceEpoch)
        let rawWeeks = c.positional(CostPeriod.self, .weeks) ?? []
        let rawDays = c.positional(CostPeriod.self, .days) ?? []
        let weekOrder = Self.order(rawWeeks), dayOrder = Self.order(rawDays)
        weeks = weekOrder.compactMap { rawWeeks[$0] }
        days = dayOrder.compactMap { rawDays[$0] }
        summary = c.lossy(CostSummary.self, .summary) ?? CostSummary()
        devices = (c.lossyArray(RawCostDevice.self, .devices) ?? []).map { $0.aligned(weeks: weekOrder, days: dayOrder) }
        unpricedModels = (c.lossyArray(String.self, .unpricedModels) ?? []).filter { !$0.isEmpty }
        sync = c.lossy(CostSync.self, .sync)
        models = c.lossyArray(CostModelRow.self, .models)
    }

    /// 解码出来的项的下标，按结束时间排好（cca 排好了，这里只是保险；设备的数组跟着同样排）
    private static func order(_ raw: [CostPeriod?]) -> [Int] {
        raw.indices.filter { raw[$0] != nil }.sorted {
            let a = raw[$0]!.endEpoch, b = raw[$1]!.endEpoch
            return a != b ? a < b : $0 < $1
        }
    }

    /// 还没过完的那一项（cca 标的 `current`）
    var currentWeek: CostPeriod? { weeks.last(where: \.current) }
    /// 今天
    var today: CostPeriod? { days.last(where: \.current) }
    /// 一台设备（按 id）
    func device(_ id: String?) -> CostDevice? {
        guard let id else { return nil }
        return devices.first { $0.id == id }
    }
    /// 这台设备（它没有这个账号的记录时没有）
    var me: CostDevice? { devices.first(where: \.isMe) }
    /// 设备间同步的状态（cca 没给时当作只有这台设备）
    var syncState: CostSyncState { sync?.state.shown ?? .localOnly }
}

/// `cost_total.scan`：会话记录统计得怎么样。
struct CostScan: Decodable, Sendable, Equatable {
    var state: CostScanState
    /// 最后一次统计的时间
    var updatedEpoch: Double?

    enum CodingKeys: String, CodingKey {
        case state
        case updatedEpoch = "updated_epoch"
    }

    init(state: CostScanState = .ready, updatedEpoch: Double? = nil) {
        self.state = state
        self.updatedEpoch = updatedEpoch
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        state = c.lossy(CostScanState.self, .state) ?? .ready
        updatedEpoch = c.double(.updatedEpoch)
    }
}

/// `cost_total.unattributed.unsynced[]`：设备菜单里还没传过认不出的用量的别的设备（cca 太旧，或者还没同步过）
struct CostUnsyncedDevice: Decodable, Sendable, Equatable, Identifiable {
    /// 和 `cost_total.devices[].id` 一样
    var id: String
    var name: String

    enum CodingKeys: String, CodingKey {
        case id, name
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let id = nonEmpty(c.string(.id)) else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "unsynced device without id"))
        }
        self.id = id
        name = nonEmpty(c.string(.name)) ?? id
    }
}

/// `cost_total.unattributed.app_hook`（3.6.9 起）：这台设备记不记 Claude App 里 Code 会话的账号（`cca cost hook`：共享的
/// ~/.claude/settings.json 里 cca 加的一条 SessionStart hook）。不是 macOS、旧版 cca 没有：nil
struct CostAppHook: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let on = CostAppHook(rawValue: "on")
    static let off = CostAppHook(rawValue: "off")
    static let allKnown: [CostAppHook] = [.on, .off]
}

/// `cost_total.unattributed`：认不出是哪个账号的用量，没算进合计。
/// `daysUSD`、`requests` 是这台设备最近 30 天的；为什么认不出、命令行里最近用的 Claude Code 是不是太旧（旧版 cca 没有：why 空的、
/// claudeCodeOld 是 false）。3.6.8 起还有所有设备的（`cost`：和账号的折合金额一样的样子）和还没传过它的设备（`unsynced`）
struct CostUnattributed: Decodable, Sendable, Equatable {
    var daysUSD: Double
    var requests: Int
    var why: [CostWhy]
    var claudeCode: String?
    var claudeCodeOld: Bool
    /// 所有设备的（这台设备的和别的设备经 Team 的服务器传来的，按日历周）；哪台设备都没有、旧版 cca 时是 nil
    var cost: AccountCost?
    /// 设备菜单里还没传过它的别的设备；旧版 cca 没有，是空的
    var unsynced: [CostUnsyncedDevice]
    /// 这台设备记不记 Claude App 里 Code 会话的账号（`cca cost hook`）；旧版 cca、不是 macOS 时是 nil
    var appHook: CostAppHook?

    enum CodingKeys: String, CodingKey {
        case daysUSD = "days_usd"
        case requests, why, cost, unsynced
        case claudeCode = "claude_code"
        case claudeCodeOld = "claude_code_old"
        case appHook = "app_hook"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        daysUSD = max(c.double(.daysUSD) ?? 0, 0)
        requests = max(c.int(.requests) ?? 0, 0)
        why = c.lossyArray(CostWhy.self, .why) ?? []
        claudeCode = nonEmpty(c.string(.claudeCode))
        claudeCodeOld = c.bool(.claudeCodeOld) ?? false
        cost = c.lossy(AccountCost.self, .cost)
        unsynced = c.lossyArray(CostUnsyncedDevice.self, .unsynced) ?? []
        appHook = c.lossy(CostAppHook.self, .appHook)
    }
}

/// `cost_total` 的一份：所有设备的（顶层的几项、`days`、`by_tool`），或者只算这台设备的（`me` 里同样几项）。
struct CostTotalPart: Decodable, Sendable, Equatable {
    /// current_usd、weeks_usd、average_usd、average_day_usd、days_usd、today_usd
    var summary: CostSummary
    /// 最近 30 个本地日历日，各账号加起来
    var days: [CostPeriod]
    /// 按工具分开的同样几项（只认识的工具：`by_tool` 的键就是数据）
    var byTool: [ToolID: CostSummary]
    /// 按模型分开的（没有 tokens；不含认不出是哪个账号的）；旧版 cca 没有
    var models: [CostModelRow]?

    enum CodingKeys: String, CodingKey {
        case days, models
        case byTool = "by_tool"
    }

    init(summary: CostSummary = CostSummary(), days: [CostPeriod] = [], byTool: [ToolID: CostSummary] = [:], models: [CostModelRow]? = nil) {
        self.summary = summary
        self.days = days
        self.byTool = byTool
        self.models = models
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        summary = (try? CostSummary(from: decoder)) ?? CostSummary()
        days = (c.lossyArray(CostPeriod.self, .days) ?? []).sorted { $0.endEpoch < $1.endEpoch }
        var tools: [ToolID: CostSummary] = [:]
        for (k, v) in c.lossyDictionary(CostSummary.self, .byTool) ?? [:] {
            let t = ToolID(rawValue: k)
            if t.isKnown { tools[t] = v }
        }
        byTool = tools
        models = c.lossyArray(CostModelRow.self, .models)
    }

    var today: CostPeriod? { days.last(where: \.current) }
}

/// `cost_total.devices[]` 的一项：一台设备在各个账号里的那一份加起来（App 的设备菜单；这台设备在最前，别的按 30 天的金额从多到少，cca 排好的）。
struct CostTotalDevice: Decodable, Sendable, Equatable, Identifiable {
    /// 和账号里 `devices[].id` 一样（设备统计里的设备指纹；这台设备没开设备统计时是 `local`）
    var id: String
    var name: String
    var isMe: Bool
    /// 它的 key 作废了（被 Team 的管理员移除了：数字是那以前同步来的）
    var revoked: Bool
    /// 每个账号里都超过 36 小时没同步上来：之后用的可能还没算进来（设备菜单里标出来）
    var stale: Bool
    /// 几项统计、最近 30 天每天、按工具分开（和 `me` 一样）
    var part: CostTotalPart

    enum CodingKeys: String, CodingKey {
        case id, name, me, revoked, stale
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let id = nonEmpty(c.string(.id)) else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "cost_total device without id"))
        }
        self.id = id
        name = nonEmpty(c.string(.name)) ?? id
        isMe = c.bool(.me) ?? false
        revoked = c.bool(.revoked) ?? false
        stale = c.bool(.stale) ?? false
        part = (try? CostTotalPart(from: decoder)) ?? CostTotalPart()
    }
}

/// `cost_total`（顶层，折合金额页的页头）：所有账号加起来。各账号的周期不一样，没有按周加起来的柱子。
/// config.json 里关掉了折合金额、没有 Claude、Codex、Antigravity 的官方账号时为 null（旧版 cca 没有这个字段，同样当作 null）。
struct CostTotal: Decodable, Sendable, Equatable {
    var currency: String
    var pricesDate: String?
    /// 所有设备的（顶层的几项、`days`、`by_tool`）
    var all: CostTotalPart
    /// 只算这台设备的（`me`：各账号里这台设备的那一份加起来）；旧版 cca 没有
    var me: CostTotalPart?
    /// 按设备分开的（设备菜单）；旧版 cca 没有，是空的（菜单里只有所有设备、本机）
    var devices: [CostTotalDevice]
    var scan: CostScan
    var unattributed: CostUnattributed?

    enum CodingKeys: String, CodingKey {
        case currency, me, devices, scan, unattributed
        case pricesDate = "prices_date"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        currency = nonEmpty(c.string(.currency)) ?? "USD"
        pricesDate = nonEmpty(c.string(.pricesDate))
        all = (try? CostTotalPart(from: decoder)) ?? CostTotalPart()
        me = c.lossy(CostTotalPart.self, .me)
        devices = c.lossyArray(CostTotalDevice.self, .devices) ?? []
        scan = c.lossy(CostScan.self, .scan) ?? CostScan()
        unattributed = c.lossy(CostUnattributed.self, .unattributed)
    }

    /// 看的那一方：所有设备的、这台设备的（旧版 cca 没给时没有），或者另一台设备的（cca 没列这台设备时没有）
    func part(_ scope: CostScope) -> CostTotalPart? {
        switch scope {
        case .all: return all
        case .me: return me
        case .device(let id): return device(id)?.part
        }
    }

    /// 一台设备（按 id）
    func device(_ id: String) -> CostTotalDevice? {
        devices.first { $0.id == id }
    }

    /// 设备菜单里分隔线下面的那几台：别的设备（这台设备在上面单独一项「本机」）
    var otherDevices: [CostTotalDevice] { devices.filter { !$0.isMe } }
}

/// `cca cost sync [--now] --json`（PROTOCOL 3.5）：统计和同步得怎么样（金额本身在 `_gui state` 里）。
struct CostSyncDoc: CcaDocument, Equatable {
    static let expectedKind: String? = "cost_sync"

    var version: Int?
    var kind: String?
    var generatedEpoch: Double?
    var scan: CostSyncScan?
    var sync: CostSyncRun?

    enum CodingKeys: String, CodingKey {
        case version, kind, scan, sync
        case generatedEpoch = "generated_epoch"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = c.int(.version)
        kind = c.string(.kind)
        generatedEpoch = c.double(.generatedEpoch)
        scan = c.lossy(CostSyncScan.self, .scan)
        sync = c.lossy(CostSyncRun.self, .sync)
    }
}

/// `cost sync` 的 `scan`：看了几个会话记录文件、这次读了多少字节、新算进来几条用量、是不是第一次全读。
struct CostSyncScan: Decodable, Sendable, Equatable {
    var state: CostScanState
    var updatedEpoch: Double?
    var files: Int
    var readBytes: Int
    var records: Int
    var first: Bool

    enum CodingKeys: String, CodingKey {
        case state, files, records, first
        case updatedEpoch = "updated_epoch"
        case readBytes = "read_bytes"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        state = c.lossy(CostScanState.self, .state) ?? .ready
        updatedEpoch = c.double(.updatedEpoch)
        files = max(c.int(.files) ?? 0, 0)
        readBytes = max(c.int(.readBytes) ?? 0, 0)
        records = max(c.int(.records) ?? 0, 0)
        first = c.bool(.first) ?? false
    }
}

/// `cost sync` 的 `sync`：这一次和别的设备同步得怎么样（每个账号的在 `_gui state` 的 `cost.sync`）。顶层是各台服务器里最要紧的那一台的，
/// `hubs` 是各台的（每项多一个 `kind`；旧版 cca 没有，是空的）
struct CostSyncRun: Decodable, Sendable, Equatable {
    var state: CostSyncRunState
    var message: String?
    var checkedEpoch: Double?
    /// `hubs` 里的那一项是哪台服务器的；顶层是 nil
    var kind: HubKind?
    var hubs: [CostSyncRun]

    enum CodingKeys: String, CodingKey {
        case state, message, kind, hubs
        case checkedEpoch = "checked_epoch"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        state = c.lossy(CostSyncRunState.self, .state) ?? CostSyncRunState(rawValue: "")
        message = nonEmpty(c.string(.message))
        checkedEpoch = c.double(.checkedEpoch)
        kind = c.lossy(HubKind.self, .kind)
        hubs = c.lossyArray(CostSyncRun.self, .hubs) ?? []
    }
}
