// 电脑上账号的状态（手机上的「账号」页）：宿主（cli/src/cmd/remote/status.rs）把那台电脑上各个账号的状态（登录、套餐、用量、每周用量、
// 在不在用）封给这个配对，放到配对所在的服务器上（status_put）；这里查会话列表时一起拿（relay_list 的 have / status，Client.swift）。
// 格式见 docs/dev/remote.md「手机上的账号」。密文和会话的说明一样（标签 cca-remote-status，密钥 HMAC(P, "cca-remote/v1/status")），
// 和 crypto.rs 的 status_seal、tests/kit/spec.rs 的 status_seal 是同一套。用量的几样和 Mac App 的 Protocol/Usage.swift 同名、同样的规矩
import Foundation

/// JSON 里的 true / false（数不算：JSONSerialization 把它们也给成 NSNumber）
func jsonBool(_ x: Any?) -> Bool? {
    guard let n = x as? NSNumber, CFGetTypeID(n) == CFBooleanGetTypeID() else { return nil }
    return n.boolValue
}

/// 有限的数（null、不是数的是 nil）
func finiteNumber(_ x: Any?) -> Double? {
    jsonNumber(x).flatMap { $0.isFinite ? $0 : nil }
}

/// 四舍五入成整数（cca 给的百分比、次数；不是数是 nil）
func roundedInt(_ x: Any?) -> Int? {
    finiteNumber(x).flatMap { abs($0) < 1e15 ? Int($0.rounded()) : nil }
}

/// n 个小写的十六进制字符
func lowerHex(_ s: String, _ n: Int) -> Bool {
    s.utf8.count == n && s.utf8.allSatisfy { (0x30...0x39).contains($0) || (0x61...0x66).contains($0) }
}

/// 一个用量窗口（5 小时、本周、分模型的、Antigravity 的一个额度）：用了百分之几、什么时候重置、过没过重置时间（cca 算好的 reset）
public struct UsageWindow: Sendable, Equatable {
    public let utilization: Double?
    public let resetsEpoch: Double?
    public let reset: Bool

    public init(utilization: Double?, resetsEpoch: Double? = nil, reset: Bool = false) {
        self.utilization = utilization
        self.resetsEpoch = resetsEpoch
        self.reset = reset
    }

    init?(json x: Any?) {
        guard let j = x as? [String: Any] else { return nil }
        self.init(utilization: finiteNumber(j["utilization"]), resetsEpoch: finiteNumber(j["resets_epoch"]), reset: jsonBool(j["reset"]) ?? false)
    }

    /// 显示时算不算已经重置：cca 算好的 reset，或者重置时间已经过了（放上来的那一份可能是半小时以前的）
    public func isReset(now: Double) -> Bool {
        if reset { return true }
        if let at = resetsEpoch { return at <= now }
        return false
    }
}

/// 判断额度用完的两个窗口；Antigravity 的额度认出是哪一个的（cca 认的 slot）
public enum QuotaSlot: String, Sendable, CaseIterable {
    case fiveHour = "five_hour"
    case sevenDay = "seven_day"
}

/// 按模型的每周额度（Claude 的 Fable、Codex 另外计的）：占本周总额度多少是 cca 认出来的
public struct ModelWindow: Sendable, Equatable {
    public let name: String
    public let window: UsageWindow
    /// 最多能用本周总额度的百分之几（Fable 是 50）；cca 认不出时是 nil
    public let allowancePercentOfWeekly: Double?
    /// 已经用掉本周总额度的百分之几
    public let usedPercentOfWeekly: Double?

    init?(json x: Any?) {
        guard let j = x as? [String: Any], let w = UsageWindow(json: j) else { return nil }
        name = clean(j["name"], 32)
        window = w
        allowancePercentOfWeekly = finiteNumber(j["allowance_percent_of_weekly"])
        usedPercentOfWeekly = finiteNumber(j["used_percent_of_weekly"])
    }
}

/// 本周额度快重置了、还剩不少，建议先用这个账号（cca 给的 use_first；规则由 cca 定，这里只拿现在的时间和区间比）
public struct UseFirst: Sendable, Equatable {
    /// 本周还剩百分之几
    public let leftPercent: Int
    public let fromEpoch: Double
    /// 本周的重置时间
    public let untilEpoch: Double

    init?(json x: Any?) {
        guard let j = x as? [String: Any], let left = roundedInt(j["left_percent"]), let from = finiteNumber(j["from_epoch"]),
              let until = finiteNumber(j["until_epoch"]), from < until
        else { return nil }
        leftPercent = left
        fromEpoch = from
        untilEpoch = until
    }

    public func isActive(now: Double) -> Bool { fromEpoch <= now && now < untilEpoch }
}

/// Codex 还能重置几次额度，最早的那次什么时候过期（cca 算好的）
public struct ResetCredits: Sendable, Equatable {
    public let availableCount: Int?
    public let nextExpiresEpoch: Double?

    init?(json x: Any?) {
        guard let j = x as? [String: Any] else { return nil }
        availableCount = roundedInt(j["availableCount"]).map { max($0, 0) }
        nextExpiresEpoch = finiteNumber(j["next_expires_epoch"])
    }
}

/// Antigravity 的一个额度
public struct QuotaBucket: Sendable, Equatable {
    public let used: UsageWindow
    /// 认出的窗口（5 小时、本周）；认不出是 nil
    public let slot: QuotaSlot?
    /// agy 停用了这个额度：不显示百分比
    public let disabled: Bool
    /// 没有百分比时 agy 可能给还剩几次
    public let remainingAmount: Int?

    init?(json x: Any?) {
        guard let j = x as? [String: Any], let w = UsageWindow(json: j) else { return nil }
        used = w
        slot = (j["slot"] as? String).flatMap(QuotaSlot.init(rawValue:))
        disabled = jsonBool(j["disabled"]) ?? false
        remainingAmount = jsonInt(j["remaining_amount"])
    }
}

/// Antigravity 的一组额度（一般是一类模型），各有 5 小时、本周的额度
public struct QuotaGroup: Sendable, Equatable {
    public let name: String
    public let buckets: [QuotaBucket]

    init?(json x: Any?) {
        guard let j = x as? [String: Any] else { return nil }
        name = clean(j["name"], 64)
        buckets = (j["buckets"] as? [Any] ?? []).prefix(16).compactMap { QuotaBucket(json: $0) }
    }

    /// 界面上的组名（和 Mac App 的 QuotaGroup.displayName 一样）：agy 给的是英文，“… models”写成“… 模型”、“ and ”写成顿号；
    /// 没有名字是“额度”
    public var displayName: String {
        var n = name.trimmingCharacters(in: .whitespaces)
        guard !n.isEmpty else { return "额度" }
        for suffix in [" models", " model"] where n.lowercased().hasSuffix(suffix) {
            n = String(n.dropLast(suffix.count)) + " 模型"
            break
        }
        return n.replacingOccurrences(of: " and ", with: "、", options: .caseInsensitive)
    }
}

/// 一个账号的用量（PROTOCOL 3.1 的用量条目里手机要的那几项）
public struct AccountUsage: Sendable, Equatable {
    /// live（刚查的）、cache（缓存的）、none（没有）
    public let source: String
    public let fetchedEpoch: Double?
    /// 数据过时了
    public let stale: Bool
    /// 这次没查成的原因码（auth 是登录失效）
    public let errorCode: String?
    public let fiveHour: UsageWindow?
    public let sevenDay: UsageWindow?
    public let modelScoped: [ModelWindow]
    public let useFirst: UseFirst?
    public let resetCredits: ResetCredits?
    /// Antigravity 的几组额度（别的工具是空的）
    public let quotaGroups: [QuotaGroup]

    init?(json x: Any?) {
        guard let j = x as? [String: Any] else { return nil }
        source = Texts0.nonEmpty(clean(j["source"], 16)) ?? "none"
        fetchedEpoch = finiteNumber(j["fetched_epoch"])
        stale = jsonBool(j["stale"]) ?? false
        errorCode = Texts0.nonEmpty(clean(j["error_code"], 32))
        fiveHour = UsageWindow(json: j["five_hour"])
        sevenDay = UsageWindow(json: j["seven_day"])
        modelScoped = (j["model_scoped"] as? [Any] ?? []).prefix(8).compactMap { ModelWindow(json: $0) }
        useFirst = UseFirst(json: j["use_first"])
        resetCredits = ResetCredits(json: j["reset_credits"])
        quotaGroups = (j["quota_groups"] as? [Any] ?? []).prefix(8).compactMap { QuotaGroup(json: $0) }
    }

    /// 有 5 小时、本周的用量可以显示
    public var hasData: Bool { source != "none" && (fiveHour != nil || sevenDay != nil) }
    /// 这个账号没有 5 小时的限制（Claude 的 Ultra、ChatGPT 的 Pro 500 这类：有本周的、没有 5 小时的）
    public var noFiveHourLimit: Bool { source != "none" && fiveHour == nil && sevenDay != nil }
    /// 有 Antigravity 的额度可以显示
    public var hasQuota: Bool { source != "none" && !quotaGroups.isEmpty }

    /// 各组里这个窗口的额度（停用的、没有百分比的不算），带着组名
    public func quotaBuckets(_ slot: QuotaSlot) -> [(group: String, bucket: QuotaBucket)] {
        quotaGroups.flatMap { g in
            g.buckets.filter { $0.slot == slot && !$0.disabled && $0.used.utilization != nil }.map { (g.displayName, $0) }
        }
    }

    /// 这个窗口用得最多的那个（过了重置时间的算最少）：小条显示它
    public func quotaMost(_ slot: QuotaSlot, now: Double) -> (window: UsageWindow, group: String)? {
        let all = quotaBuckets(slot)
        func used(_ b: QuotaBucket) -> Double { b.used.isReset(now: now) ? -1 : b.used.utilization ?? 0 }
        guard let top = all.max(by: { used($0.bucket) < used($1.bucket) }) else { return nil }
        return (top.bucket.used, top.group)
    }

    /// 这个窗口用得最少的那个（过了重置时间的算 0；一样多时先重置的那个）：它也用完了，这个窗口才算用完
    public func quotaLeast(_ slot: QuotaSlot, now: Double) -> UsageWindow? {
        let all = quotaBuckets(slot).map(\.bucket.used)
        func used(_ w: UsageWindow) -> Double { w.isReset(now: now) ? 0 : w.utilization ?? 0 }
        return all.min { a, b in
            used(a) != used(b) ? used(a) < used(b) : (a.resetsEpoch ?? .infinity) < (b.resetsEpoch ?? .infinity)
        }
    }

    /// 判断额度紧不紧的两个窗口（5 小时、本周）：Claude、Codex 是 fiveHour、sevenDay；Antigravity 是各组里用得最少的那个
    public func pressureWindows(now: Double) -> [(slot: QuotaSlot, window: UsageWindow?)] {
        hasQuota ? QuotaSlot.allCases.map { ($0, quotaLeast($0, now: now)) } : [(.fiveHour, fiveHour), (.sevenDay, sevenDay)]
    }
}

/// 一周（cca 记下的每周用量）
public struct UsageWeek: Sendable, Equatable, Identifiable {
    public let startEpoch: Double
    public let endEpoch: Double
    /// 用到这一周额度的百分之几：中间重置过的把重置前的加上，可能超过 100
    public let usedPercent: Double
    /// 中间重置过几次
    public let resets: Int
    /// 还没过完的这一周
    public let current: Bool
    /// 结束前一天里没查到过：实际可能更多
    public let partial: Bool

    public var id: Double { endEpoch }

    init?(json x: Any?) {
        guard let j = x as? [String: Any], let start = finiteNumber(j["start_epoch"]), let end = finiteNumber(j["end_epoch"]),
              let used = finiteNumber(j["used_percent"]), start < end
        else { return nil }
        startEpoch = start
        endEpoch = end
        usedPercent = max(used, 0)
        resets = max(roundedInt(j["resets"]) ?? 0, 0)
        current = jsonBool(j["current"]) ?? false
        partial = jsonBool(j["partial"]) ?? false
    }
}

/// 过完的几周的统计（cca 算好的）
public struct UsageWeeksSummary: Sendable, Equatable {
    public var weeks = 0
    public var averagePercent: Int?
    public var maxed = 0
    public var underHalf = 0

    init() {}

    init?(json x: Any?) {
        guard let j = x as? [String: Any] else { return nil }
        weeks = max(roundedInt(j["weeks"]) ?? 0, 0)
        averagePercent = roundedInt(j["average_percent"])
        maxed = max(roundedInt(j["maxed"]) ?? 0, 0)
        underHalf = max(roundedInt(j["under_half"]) ?? 0, 0)
    }
}

/// 最近几周每周用到这一周额度的百分之几（cca 每次查用量时记下的）
public struct UsageWeeks: Sendable, Equatable {
    /// 从什么时候开始记的（最早一次记下的时间）
    public let sinceEpoch: Double?
    /// 按时间排好，最后一项可能是还没过完的这一周
    public let weeks: [UsageWeek]
    public let summary: UsageWeeksSummary

    init?(json x: Any?) {
        guard let j = x as? [String: Any] else { return nil }
        sinceEpoch = finiteNumber(j["since_epoch"])
        weeks = (j["weeks"] as? [Any] ?? []).prefix(60).compactMap { UsageWeek(json: $0) }.sorted { $0.endEpoch < $1.endEpoch }
        summary = UsageWeeksSummary(json: j["summary"]) ?? UsageWeeksSummary()
    }
}

/// 那台电脑上的一个账号
public struct AccountStatus: Sendable, Equatable, Identifiable {
    /// 这个配对里认这个账号的编号（16 个十六进制字符，宿主按配对算的：换了别名也不变，别的配对、服务器看不出是哪个账号）
    public let id: String
    /// 会话的说明里的同一个账号摘要（SessionInfo.aid）：照它把会话放到账号下面；那台电脑不在 Team 里时是空的
    public let aid: String
    /// claude、codex、agy
    public let tool: String
    /// 别名（没起别名的是邮箱 @ 前面那一截：不给邮箱）
    public let name: String
    /// oauth（官方账号）、api（第三方 API）
    public let kind: String
    /// logged_in、logged_out、email_mismatch、api……（PROTOCOL 2.2 的 login）
    public let login: String
    public let plan: String
    /// 套餐档位：entry、standard、advanced、premium（不知道是空的）
    public let tier: String
    /// 在用：那台电脑上桌面 App 开着、命令行里有会话
    public let busy: Bool
    /// 头像的颜色（#RRGGBB）和上面的字（启动器的那一个）
    public let color: String?
    public let badge: String
    public let usage: AccountUsage?
    public let weeks: UsageWeeks?

    init?(json x: Any?) {
        guard let j = x as? [String: Any], let id = j["id"] as? String, lowerHex(id, 16) else { return nil }
        self.id = id
        aid = (j["aid"] as? String).flatMap { lowerHex($0, 32) ? $0 : nil } ?? ""
        tool = clean(j["tool"], 16)
        name = clean(j["name"], 64)
        kind = clean(j["kind"], 16)
        login = clean(j["login"], 32)
        plan = clean(j["plan"], 32)
        tier = clean(j["tier"], 32)
        busy = jsonBool(j["busy"]) ?? false
        color = (j["color"] as? String).flatMap { c in
            c.utf8.count == 7 && c.hasPrefix("#") && c.utf8.dropFirst().allSatisfy { isxdigit(Int32($0)) != 0 } ? c : nil
        }
        badge = clean(j["badge"], 8)
        usage = AccountUsage(json: j["usage"])
        weeks = UsageWeeks(json: j["weeks"])
    }

    /// 第三方 API 账号（没有订阅的用量）
    public var isAPI: Bool { kind == "api" || login == "api" }

    /// 登录着、有用量、不是登录失效：才看额度用没用完
    private var counts: Bool {
        guard login == "logged_in", let u = usage else { return false }
        return u.source != "none" && u.errorCode != "auth"
    }

    /// 挡住这个账号的额度窗口（5 小时在前，带着重置时间）。和 cca 的 status.rs 的 `exhausted`（推「额度用完了」照它）、Mac App 的
    /// Diagnosis.quotaPressure 里用完的那一半是同一个规则：登录着、有用量、不是登录失效；5 小时、本周两个窗口（Antigravity 是各组里这个
    /// 窗口用得最少的那个：一组用完了别的组的模型照样能用），四舍五入以后到了 100%、还没到重置时间（数据旧不旧不看）。模型级的窗口不算
    public func exhausted(now: Double) -> [(slot: QuotaSlot, resetsEpoch: Double?)] {
        guard counts, let u = usage else { return [] }
        return u.pressureWindows(now: now).compactMap { slot, w in
            guard let w, let p = w.utilization, !w.isReset(now: now), p.rounded() >= 100 else { return nil }
            return (slot, w.resetsEpoch)
        }
    }

    /// 用完了的里面最晚重置的那个（不知道什么时候重置的算最晚）：「额度用完了，16:00 恢复」照它（Mac App 的 `blocking`）
    public func blocking(now: Double) -> (slot: QuotaSlot, resetsEpoch: Double?)? {
        exhausted(now: now).max { ($0.resetsEpoch ?? .infinity) < ($1.resetsEpoch ?? .infinity) }
    }

    /// 快用完了（90% 以上、还没用完，数据是新的）：Mac App 的小标记 nearLimit
    public func nearLimit(now: Double) -> Bool {
        guard counts, let u = usage, !u.stale, exhausted(now: now).isEmpty else { return false }
        return u.pressureWindows(now: now).contains { _, w in
            guard let w, let p = w.utilization, !w.isReset(now: now) else { return false }
            return p >= 90
        }
    }

    /// 5 小时的额度快用完了（90% 以上，不看数据新旧）：这时候不建议先用它（Mac App 的 Diagnosis.fiveHourPressed）
    public func fiveHourPressed(now: Double) -> Bool {
        guard let u = usage, let w = u.pressureWindows(now: now).first(where: { $0.slot == .fiveHour })?.window,
              let p = w.utilization, !w.isReset(now: now)
        else { return false }
        return p >= 90
    }
}

/// 一台电脑放上来的那一份
public struct HostStatus: Sendable, Equatable {
    /// 账号最多认这么多个（宿主那边放不下的也不会更多：服务器只收 40 KB）
    public static let maxAccounts = 64

    /// 那台电脑叫什么（它自己的主机名）
    public let machine: String
    /// 照账号表的先后（三个工具的）
    public let accounts: [AccountStatus]
    /// 太多了、没放下的还有几个
    public let more: Int
    /// 那台电脑现在不让这台手机接入（改过）：没有账号
    public let off: Bool
    /// 账号太多、放不下，没带每周用量（各个账号的 weeks 都是 nil）
    public let weeksLeftOut: Bool
    /// 那台电脑封它的时间（那台电脑的钟）
    public let at: Double
    /// 查到时它在服务器上放了多久（服务器的钟，秒）
    public internal(set) var age: Double
    /// 这台手机什么时候拿到的（秒）
    public internal(set) var received: Double

    /// 演示数据、测试用：照放上来的那份 JSON 读；不是 v1 的是 nil
    public init?(json j: [String: Any], age: Double, received: Double) {
        guard jsonInt(j["v"]) == 1 else { return nil }
        machine = clean(j["machine"], 48)
        off = jsonBool(j["off"]) ?? false
        weeksLeftOut = jsonBool(j["weeks"]) == false
        var seen = Set<String>()
        accounts = (j["accounts"] as? [Any] ?? []).prefix(Self.maxAccounts).compactMap { AccountStatus(json: $0) }.filter { seen.insert($0.id).inserted }
        more = max(jsonInt(j["more"]) ?? 0, 0)
        at = finiteNumber(j["at"]) ?? 0
        self.age = max(age, 0)
        self.received = received
    }

    /// 放到服务器上的时间（照这台手机的钟：拿到的时间减去在服务器上放了多久）
    public var putAt: Double { received - age }
}

/// P 派生的“账号的状态”的密钥
func statusKeys(_ p: Bytes) -> SealKeys {
    SealKeys(from: Mac(key: hmac(p, Bytes(utf8: "cca-remote/v1/status"))))
}

/// 解开一份：认证带着配对编号（别的配对的、别的密钥封的、会话的说明、通知都解不开）；不是 v1 的、样子不对的是 nil
func statusOpen(_ keys: SealKeys, pid: String, _ blob: Any?, age: Double, received: Double) -> HostStatus? {
    guard let pt = unseal("cca-remote-status", keys, [pid], blob), let j = parsed(pt) else { return nil }
    return HostStatus(json: j, age: age, received: received)
}

/// 额度的通知（宿主的 status.rs 推的，Push.swift 解开）：哪个账号的哪个窗口用完了、恢复了
public struct UsageNote: Sendable, Equatable {
    public enum Event: String, Sendable {
        case usedUp = "used_up"
        case back
    }

    public let event: Event
    /// 哪个账号（AccountStatus.id）
    public let account: String
    /// 会话的说明里的账号摘要（没有是空的）
    public let aid: String
    public let tool: String
    public let name: String
    public let slot: QuotaSlot
    /// 什么时候重置（恢复的那一条是恢复的时间）；不知道是 nil
    public let resetsEpoch: Double?

    init?(json j: [String: Any]) {
        guard let e = (j["event"] as? String).flatMap(Event.init(rawValue:)), let a = j["account"] as? String, lowerHex(a, 16),
              let s = (j["window"] as? String).flatMap(QuotaSlot.init(rawValue:))
        else { return nil }
        event = e
        account = a
        aid = (j["aid"] as? String).flatMap { lowerHex($0, 32) ? $0 : nil } ?? ""
        tool = clean(j["tool"], 16)
        name = clean(j["name"], 64)
        slot = s
        resetsEpoch = finiteNumber(j["resets_epoch"])
    }
}

/// 额度的几句话（App 和通知扩展用同一份；说法照 Mac App 的 Texts：backTime、backAt、resetText）
public enum UsageText {
    static let weekdays = ["日", "一", "二", "三", "四", "五", "六"]

    /// “09:00”
    public static func clock(_ epoch: Double, calendar: Calendar = .current) -> String {
        let c = calendar.dateComponents([.hour, .minute], from: Date(timeIntervalSince1970: epoch))
        return String(format: "%02d:%02d", c.hour ?? 0, c.minute ?? 0)
    }

    /// “周二 09:00”
    public static func weekdayTime(_ epoch: Double, calendar: Calendar = .current) -> String {
        let c = calendar.dateComponents([.weekday], from: Date(timeIntervalSince1970: epoch))
        return "周" + weekdays[((c.weekday ?? 1) - 1 + 7) % 7] + " " + clock(epoch, calendar: calendar)
    }

    /// 额度用完的账号几点恢复：今天的“16:00”，明天的“明天 09:00”，再远“周二 16:00”；today 写在今天的时间前面
    public static func backTime(epoch: Double, now: Double, calendar: Calendar = .current, today: String = "") -> String {
        let d = Date(timeIntervalSince1970: epoch), n = Date(timeIntervalSince1970: now)
        if calendar.isDate(d, inSameDayAs: n) { return today + clock(epoch, calendar: calendar) }
        if let t = calendar.date(byAdding: .day, value: 1, to: n), calendar.isDate(d, inSameDayAs: t) {
            return "明天 " + clock(epoch, calendar: calendar)
        }
        return weekdayTime(epoch, calendar: calendar)
    }

    /// “16:00 恢复”；时间已经过了是“马上恢复”（等那台电脑下一次查到新的用量）
    public static func backAt(epoch: Double, now: Double, calendar: Calendar = .current) -> String {
        epoch <= now ? "马上恢复" : backTime(epoch: epoch, now: now, calendar: calendar) + " 恢复"
    }

    /// 窗口的叫法
    public static func window(_ s: QuotaSlot) -> String {
        s == .fiveHour ? "5 小时" : "本周"
    }

    /// 账号是哪个工具的
    public static func product(_ tool: String) -> String {
        switch tool {
        case "claude": return "Claude Code"
        case "codex": return "Codex"
        case "agy": return "Antigravity"
        default: return tool
        }
    }

    /// 额度的通知弹出来的标题、正文（时间照这台手机的时区写）：「额度用完了」「5 小时的额度用完了，16:00 恢复」；
    /// 「额度恢复了」「本周的额度恢复了，可以接着用了」
    public static func note(_ u: UsageNote, now: Double, calendar: Calendar = .current) -> (title: String, body: String) {
        let w = window(u.slot)
        switch u.event {
        case .usedUp:
            let at = u.resetsEpoch.flatMap { $0 > now ? "，" + backTime(epoch: $0, now: now, calendar: calendar) + " 恢复" : nil } ?? ""
            return ("额度用完了", "\(w)的额度用完了" + at)
        case .back:
            return ("额度恢复了", "\(w)的额度恢复了，可以接着用了")
        }
    }

    /// 副标题里机器名后面的那一段：「工作 · Claude Code」
    public static func who(_ u: UsageNote) -> String {
        [Texts0.nonEmpty(u.name), Texts0.nonEmpty(product(u.tool))].compactMap { $0 }.joined(separator: " · ")
    }
}

/// 这里自己用的小工具（App 里的 Texts 另有一份）
enum Texts0 {
    static func nonEmpty(_ s: String?) -> String? {
        guard let s, !s.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }
        return s
    }
}
