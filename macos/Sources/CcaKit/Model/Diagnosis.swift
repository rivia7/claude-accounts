import Foundation

/// 一个账号现在的问题，按要紧程度排好。账号页那句“下一步”（Advice）和办公室里员工的状态（Employee）都从这里取，
/// 顺序只写在这一处。只按 cca 给的状态说话（登录、can_why、路由器实况、出口检测、用量），不重算 cca 的规则。
/// 第一条可能是建议而不是问题：`useFirst`（本周额度快重置了、还剩不少），只在没有挡路的问题时出现，排在最前。
enum Diagnosis: Equatable, Sendable {
    case loggedOut
    case emailMismatch(loginEmail: String?)
    /// 看着还登录着，但凭证已经失效了（改过密码、在别处退出过；查用量时才知道）
    case loginExpired
    /// 路由器 / 本机 Clash 上这个账号的端口连不上
    case portClosed
    /// 路由器给这个账号选错了节点
    case portDrift
    /// 出口检测：不可用地区、拿不到出口、端口不通、接口连不上
    case exitBad(HealthResult, region: String?)
    /// 5 小时 / 本周的额度用完了，或者快用完了（≥90%）；5 小时在前
    case quota([QuotaPressure])
    /// Claude Code 登录时记下的套餐过时了（升降级以后要重新登录才会更新）：值是记下的那个（“Pro”）。
    /// 额度照账号实际的套餐算，只是 Claude Code 里还按旧套餐显示、处理，所以排在真正的问题后面
    case outdatedLoginPlan(String)
    /// 这台 Mac 上没装桌面 App（只是提示：还能用网页版；排在真正的问题后面）
    case appMissing
    case lowPurity
    case leak
    /// 从没检测过出口
    case neverChecked
    /// 建议，不是问题：本周额度快重置了、还剩不少，先用这个账号（cca 给的 use_first，App 只比时间）
    case useFirst(UseFirst)

    /// 一个快用完（或者已经用完）的额度窗口。
    struct QuotaPressure: Equatable, Sendable {
        enum Window: Sendable { case fiveHour, week }
        var window: Window
        var percent: Double
        var resetsEpoch: Double?
        /// 用完了：显示成 100%（和 Texts.percent 的四舍五入一致）
        var exhausted: Bool { percent.rounded() >= 100 }
    }

    /// 全部问题，最要紧的在前；空的就是一切正常。
    /// 用完的额度窗口不看数据旧不旧（没到重置时间不会降下来）；快用完（90–99%）的要数据是新的才算。
    static func all(_ a: Account, usage u: UsageEntry?, live: LiveSlot?, now: Double) -> [Diagnosis] {
        switch a.login {
        case .loggedOut: return [.loggedOut]
        case .emailMismatch: return [.emailMismatch(loginEmail: nonEmpty(a.loginEmail))]
        default: break
        }
        if u?.errorCode == .auth { return [.loginExpired] }
        var out: [Diagnosis] = []
        switch live?.state {
        case .closed?: out.append(.portClosed)
        case .drift?: out.append(.portDrift)
        default: break
        }
        let h = a.health.flatMap { $0.applies && !$0.stale ? $0 : nil }
        if let h, let r = h.result, [.restricted, .noExit, .portClosed, .apiUnreachable].contains(r) {
            out.append(.exitBad(r, region: h.restrictedRegion))
        }
        let pressed = quotaPressure(u, now: now)
        if !pressed.isEmpty { out.append(.quota(pressed)) }
        if let p = nonEmpty(u?.outdatedLoginPlan) { out.append(.outdatedLoginPlan(p)) }
        if !a.isAPI, !a.can.app, a.canWhy["app"] == .appMissing { out.append(.appMissing) }
        if let h {
            if h.result == .lowPurity { out.append(.lowPurity) }
            if h.leak?.hasLeak == true { out.append(.leak) }
        }
        if a.health == nil { out.append(.neverChecked) }
        if let f = u?.useFirst, f.isActive(now: now), !out.contains(where: \.blocksUseFirst), !fiveHourPressed(u, now: now) {
            out.insert(.useFirst(f), at: 0)
        }
        return out
    }

    /// 有它就不建议先用这个账号：用不了、额度紧张、出口有风险的都算；只是提示的（套餐过时、没装 App、没检测过出口）不算。
    /// 穷举、不写 default：加了新的问题，编译器会逼着决定它挡不挡。
    var blocksUseFirst: Bool {
        switch self {
        case .loggedOut, .emailMismatch, .loginExpired, .portClosed, .portDrift, .exitBad, .quota, .lowPurity, .leak: return true
        case .outdatedLoginPlan, .appMissing, .neverChecked, .useFirst: return false
        }
    }

    /// 5 小时的额度快用完了（≥90%，不看数据新旧：没到重置时间不会降下来）：这时候不建议先用它。
    static func fiveHourPressed(_ u: UsageEntry?, now: Double) -> Bool {
        guard let w = u?.fiveHour, let p = w.utilization, !w.isReset(now: now) else { return false }
        return UsageLevel(percent: p) == .critical
    }

    /// 5 小时、本周两个窗口里用完的、快用完的（模型级的窗口不算：一个模型用完了，别的模型照样能用）。
    static func quotaPressure(_ u: UsageEntry?, now: Double) -> [QuotaPressure] {
        guard let u, u.source != .none else { return [] }
        var out: [QuotaPressure] = []
        for (window, w) in [(QuotaPressure.Window.fiveHour, u.fiveHour), (.week, u.sevenDay)] {
            guard let w, let p = w.utilization, !w.isReset(now: now) else { continue }
            let q = QuotaPressure(window: window, percent: p, resetsEpoch: w.resetsEpoch)
            if q.exhausted || (!u.stale && UsageLevel(percent: p) == .critical) { out.append(q) }
        }
        return out
    }
}

extension Array where Element == Diagnosis.QuotaPressure {
    /// 挡住这个账号的窗口：用完了的里面最晚重置的那个（不知道什么时候重置的算最晚）；都没用完是 nil。
    /// 账号页那句话和工位上的“几点回来”都按它。
    var blocking: Diagnosis.QuotaPressure? {
        filter(\.exhausted).max { ($0.resetsEpoch ?? .infinity) < ($1.resetsEpoch ?? .infinity) }
    }
}
