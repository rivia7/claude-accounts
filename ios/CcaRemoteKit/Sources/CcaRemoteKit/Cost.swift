// 电脑上账号的折合金额、这个配对所在的 Team（手机上的「折合金额」「共享与同步」页）：宿主的 status.rs（cost_of、cost_meta、team_of）
// 放在账号的状态里一起封好的（Status.swift 解开）。格式见 docs/dev/remote.md「手机上的账号」：金额都是美分的整数；按周、按天的只给金额，
// 起止照首尾相接推出来（最后一项结束的时间、各项有几个小时）。数字都是那台电脑上的 cca 算好的（Mac App 的折合金额也照它们），这里只读
import Foundation

/// 一项（一周、一天）的折合金额
public struct CostPeriod: Sendable, Equatable, Identifiable {
    public let startEpoch: Double
    public let endEpoch: Double
    /// 美分
    public let cents: Int
    /// 最后一项：这一周（本周至今）、今天
    public let current: Bool
    /// 这一项开始时还没有记录（cca 的 partial：开始有记录的时间晚于这一项开始的时间），实际可能更多
    public let partial: Bool

    public var usd: Double { Double(cents) / 100 }
    public var id: Double { endEpoch }

    public init(startEpoch: Double, endEpoch: Double, cents: Int, current: Bool, partial: Bool) {
        self.startEpoch = startEpoch
        self.endEpoch = endEpoch
        self.cents = cents
        self.current = current
        self.partial = partial
    }
}

/// 按模型分开的一项（cca 排在前面的几个）
public struct CostModel: Sendable, Equatable {
    /// 模型名；没按模型记下的那一部分（旧的记录）是 nil
    public let name: String?
    /// 近 12 周、近 30 天（美分）
    public let weeks: Int
    public let days: Int
    /// cca 的价格表里还没有这个新模型，暂按这个同系列的模型估算（≈）；有公开价格的是 nil
    public let pricedAs: String?

    public func cents(days: Bool) -> Int { days ? self.days : weeks }
}

/// 用过这个账号的一台设备（同一个 Team 里的设备经它的服务器同步过来的）
public struct CostDevice: Sendable, Equatable {
    public let name: String
    /// 放上来的那台电脑自己
    public let me: Bool
    /// 近 12 周、近 30 天（美分）
    public let weeks: Int
    public let days: Int
    /// 很久没同步上来了
    public let stale: Bool
    /// 上次同步上来的时间
    public let syncedEpoch: Double?
}

/// 一个账号的折合金额：按官方 API 的价格把用掉的 token 折成美元（不是实际花费），几台设备加起来的（同一个 Team 里）
public struct AccountCost: Sendable, Equatable {
    /// 开始有记录的时间
    public let sinceEpoch: Double?
    /// 按周：最近 12 周和这一周，按时间排（最后一项是这一周）；中间换过切法的那一项不是 7 天
    public let weeks: [CostPeriod]
    /// 按天：最近 30 天（最后一项是今天）
    public let days: [CostPeriod]
    /// 合计（美分，cca 算好的）：列出的各周、30 天
    public let weeksTotal: Int
    public let daysTotal: Int
    /// 平均每周、每天（美分；一项都没过完、算不出来时是 nil）和平均了几周、几天
    public let averageWeek: Int?
    public let averageDay: Int?
    public let averageWeeks: Int
    public let averageDays: Int
    /// 缓存命中率（0～1，cca 给的）：列出的各周、30 天；没有输入时是 nil
    public let cacheHitWeeks: Double?
    public let cacheHitDays: Double?
    /// 同步没成时 cca 给的状态（not_logged_in、pending、local_only、off、error……）；同步着、cca 没说是空的
    public let sync: String
    /// 还有几台设备没算进来（可能 cca 太旧）
    public let missing: Int
    /// 按模型的（cca 排在前面的几个）；那台电脑的账号太多、没带时是 nil
    public let models: [CostModel]?
    /// 用过它的各台设备：两台以上、或者有很久没同步上来的时才有
    public let devices: [CostDevice]

    /// 一周最多有几个小时（中间换过切法、夏令时的那一项不是 168）：认不得的长度照一周算
    static let maxHours = 24 * 31

    init?(json x: Any?) {
        guard let j = x as? [String: Any], let weekEnd = finiteNumber(j["week_end"]), let dayEnd = finiteNumber(j["day_end"]) else { return nil }
        let since = finiteNumber(j["since"])
        sinceEpoch = since
        func cents(_ v: Any?) -> Int { max(roundedInt(v) ?? 0, 0) }
        func list(_ k: String) -> [Any] { j[k] as? [Any] ?? [] }
        /// 从最后一项结束的时间往前排：每一项接着上一项开始
        func periods(_ amounts: [Any], end: Double, hours: (Int) -> Int) -> [CostPeriod] {
            var out: [CostPeriod] = []
            var t = end
            for i in amounts.indices.reversed() {
                let start = t - Double(hours(i)) * 3600
                out.append(CostPeriod(startEpoch: start, endEpoch: t, cents: cents(amounts[i]), current: i == amounts.count - 1,
                                      partial: since.map { $0 > start } ?? false))
                t = start
            }
            return out.reversed()
        }
        let hours = list("week_hours").map { roundedInt($0) ?? 0 }
        weeks = periods(Array(list("weeks").suffix(20)), end: weekEnd) { i in
            let h = i < hours.count ? hours[i] : 0
            return (1...Self.maxHours).contains(h) ? h : 168
        }
        days = periods(Array(list("days").suffix(40)), end: dayEnd) { _ in 24 }
        guard !weeks.isEmpty || !days.isEmpty else { return nil }
        func pair(_ k: String) -> (Any?, Any?) {
            let l = list(k)
            return (l.first, l.count > 1 ? l[1] : nil)
        }
        let sum = pair("sum"), avg = pair("average"), n = pair("average_n"), hit = pair("cache_hit")
        weeksTotal = cents(sum.0)
        daysTotal = cents(sum.1)
        averageWeek = roundedInt(avg.0).map { max($0, 0) }
        averageDay = roundedInt(avg.1).map { max($0, 0) }
        averageWeeks = max(roundedInt(n.0) ?? 0, 0)
        averageDays = max(roundedInt(n.1) ?? 0, 0)
        func ratio(_ v: Any?) -> Double? { finiteNumber(v).map { min(max($0, 0), 1) } }
        cacheHitWeeks = ratio(hit.0)
        cacheHitDays = ratio(hit.1)
        sync = clean(j["sync"], 24)
        missing = max(roundedInt(j["missing"]) ?? 0, 0)
        models = (j["models"] as? [Any]).map { rows in
            rows.prefix(8).compactMap { r -> CostModel? in
                guard let r = r as? [Any], r.count >= 3 else { return nil }
                let name = r[0] is String ? clean(r[0], 48) : nil
                let base = r.count > 3 ? Texts0.nonEmpty(clean(r[3], 48)) : nil
                return CostModel(name: name, weeks: cents(r[1]), days: cents(r[2]), pricedAs: base)
            }
        }
        devices = list("devices").prefix(8).compactMap { r -> CostDevice? in
            guard let r = r as? [Any], r.count >= 6 else { return nil }
            return CostDevice(name: clean(r[0], 48), me: roundedInt(r[1]) == 1, weeks: cents(r[2]), days: cents(r[3]),
                              stale: roundedInt(r[4]) == 1, syncedEpoch: finiteNumber(r[5]))
        }
    }

    /// 按周还是按天的那几项
    public func periods(days: Bool) -> [CostPeriod] { days ? self.days : weeks }

    /// 这一周、今天（最后一项）的金额（美分）
    public func current(days: Bool) -> Int { periods(days: days).last.map { $0.current ? $0.cents : 0 } ?? 0 }

    public func total(days: Bool) -> Int { days ? daysTotal : weeksTotal }
    public func average(days: Bool) -> Int? { days ? averageDay : averageWeek }
    public func averageCount(days: Bool) -> Int { days ? averageDays : averageWeeks }
    public func cacheHit(days: Bool) -> Double? { days ? cacheHitDays : cacheHitWeeks }
}

/// 那台电脑的折合金额里不是哪个账号的那几样（cca 的 cost_total）
public struct HostCost: Sendable, Equatable {
    /// 价格表的日期（cca 给的 YYYY-MM-DD）
    public let pricesDate: String
    /// 上次统计的时间；还没统计过是 nil
    public let updatedEpoch: Double?
    /// 正在做第一次统计（要把会话记录全读一遍）
    public let scanning: Bool
    /// 那台电脑近 30 天认不出是哪个账号的（美分，只算它自己的）
    public let unattributed: Int
    /// 账号太多、放不下：各个账号的折合金额没带按模型、按设备的
    public let brief: Bool

    init?(json x: Any?) {
        guard let j = x as? [String: Any] else { return nil }
        pricesDate = clean(j["prices_date"], 16)
        updatedEpoch = finiteNumber(j["updated"])
        scanning = jsonBool(j["scanning"]) ?? false
        unattributed = max(roundedInt(j["unattributed"]) ?? 0, 0)
        brief = jsonBool(j["brief"]) ?? false
    }
}

/// 这个配对所在的 Team（cca 的 config.teams 里那一项）：手机上的「共享与同步」页
public struct HostTeam: Sendable, Equatable {
    public let name: String
    /// 那台电脑在这个 Team 里是 admin（管理员）还是 member
    public let role: String
    /// 别的管理员授予的管理员（不是建 Team 的那台）
    public let coadmin: Bool
    /// local、pending、ok、removed、lost、error……（PROTOCOL 4.14）
    public let state: String
    /// 没同步成、被移除时 cca 给的原因
    public let error: String
    /// 配置同步到第几版（0 是还没同步过）
    public let seq: Int
    /// 分给那台电脑几个账号
    public let accounts: Int
    /// 那台电脑在 Team 里叫什么
    public let device: String
    /// 服务器的地址（主机名和端口）
    public let host: String
    public let joinedEpoch: Double?
    /// 服务器比那台电脑上的 cca 要的旧、要更新：为什么（cca 给的）；不用更新是 nil
    public let update: String?

    init?(json x: Any?) {
        guard let j = x as? [String: Any] else { return nil }
        name = clean(j["name"], 64)
        role = clean(j["role"], 16)
        coadmin = jsonBool(j["coadmin"]) ?? false
        state = clean(j["state"], 16)
        error = clean(j["error"], 160)
        seq = max(roundedInt(j["seq"]) ?? 0, 0)
        accounts = max(roundedInt(j["accounts"]) ?? 0, 0)
        device = clean(j["device"], 64)
        host = clean(j["host"], 128)
        joinedEpoch = finiteNumber(j["joined"])
        update = j["update"] is String ? clean(j["update"], 160) : nil
    }

    /// 能管理这个 Team：建 Team 的那台（role 是 admin）和别的管理员（role 是 member、coadmin 是 true：建 Team 的那台给的），
    /// 和 Mac App 的 TeamEntry 一样
    public var isAdmin: Bool { role == "admin" || coadmin }
    /// 建 Team 的那台（拿着根密钥）
    public var isOwner: Bool { role == "admin" }
    /// 要用户知道的（Mac App 的 TeamState.failing：侧栏「共享与同步」那一行的「Team 要处理」）：被移除了、记录没了、没同步成，
    /// 和认不出的状态；ok、pending、local 不算
    public var failing: Bool { !["ok", "pending", "local"].contains(state) }
}
