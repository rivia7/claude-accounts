import Foundation

/// 一个账号现在的问题，按要紧程度排好。账号页那句“下一步”（Advice）和办公室里员工的状态（Employee）都从这里取，
/// 顺序只写在这一处。只按 cca 给的状态说话（登录、can_why、路由器实况、出口检测、用量），不重算 cca 的规则。
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
    /// 这台 Mac 上没装桌面 App（只是提示：还能用网页版；排在真正的问题后面）
    case appMissing
    case lowPurity
    case leak
    /// 从没检测过出口
    case neverChecked

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
        if !a.isAPI, !a.can.app, a.canWhy["app"] == .appMissing { out.append(.appMissing) }
        if let h {
            if h.result == .lowPurity { out.append(.lowPurity) }
            if h.leak?.hasLeak == true { out.append(.leak) }
        }
        if a.health == nil { out.append(.neverChecked) }
        return out
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
