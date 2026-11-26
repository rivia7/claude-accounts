import Foundation

/// 额度用完、恢复的通知（App 自己弹的，设置里可以关，CcaApp 的 UsageNotifier 弹）：一个账号的 5 小时或本周的额度用完了马上弹一条，
/// 到了重置的时间再弹一条（定好时间交给系统：那时 App 没在跑也到）。算不算用完是 Diagnosis 里用完的那一半（quotaPressure 的
/// exhausted：登录着、不是登录失效，四舍五入到 100%、没到重置时间，数据旧不旧不看；Antigravity 看各组里用得最少的那个），和 cca 推给
/// 配对的手机的（cli/src/cmd/remote/status.rs 的 exhausted）是同一个规则。同一轮（重置时间差不到一小时：Codex 的重置时间每次差一两秒，
/// 下一轮至少晚 5 小时）只弹一次；弹过的记着（App 的设置里），过了重置时间一天忘掉。只看数，不读 cca 的文件
struct UsageAlerts: Equatable, Codable {
    /// 两次查到的重置时间差不到这么多算同一轮（和 status.rs 的 SAME_ROUND 一样）
    static let sameRound: Double = 3600
    /// 过了重置时间多久忘掉（status.rs 的 FORGET）
    static let forget: Double = 86400

    /// 弹过“用完了”的那一轮：重置时间（不知道是 nil）、什么时候弹的
    struct Round: Codable, Equatable {
        var resets: Double?
        var at: Double
    }

    /// 要弹的一条
    struct Note: Equatable {
        enum Event: String {
            case usedUp = "used_up"
            case back
        }

        let event: Event
        let account: AccountRef
        /// 认账号（Account.stableID：工具 + cca 给的账号目录名，改别名不变）：记着弹过的、通知的标识都照它
        let stable: String
        let name: String
        let window: Diagnosis.QuotaPressure.Window
        let resetsEpoch: Double?

        /// 恢复的那一条：到了重置的时间弹（重置时间不知道、已经过了的没有）
        func back(now: Double) -> Note? {
            guard event == .usedUp, let r = resetsEpoch, r > now else { return nil }
            return Note(event: .back, account: account, stable: stable, name: name, window: window, resetsEpoch: r)
        }

        /// 通知的标识：同一轮的同一条只有一个（定好的“恢复了”照它找、照它换掉）
        var identifier: String {
            "cca.usage.\(event.rawValue).\(UsageAlerts.key(stable, window)).\(resetsEpoch.map { String(Int($0)) } ?? "-")"
        }
    }

    /// 账号 + 窗口 → 弹过的那一轮
    var memo: [String: Round] = [:]

    /// stable 是 Account.stableID（改别名不变：改了别名，同一轮的不再弹一遍）
    static func key(_ stable: String, _ w: Diagnosis.QuotaPressure.Window) -> String {
        "\(stable):\(w == .fiveHour ? "five_hour" : "seven_day")"
    }

    /// 两个重置时间算不算同一轮（有一个不知道的算）
    static func sameRound(_ a: Double?, _ b: Double?) -> Bool {
        guard let a, let b else { return true }
        return abs(a - b) < sameRound
    }

    /// 照现在的用量看要弹的“用完了”（新的一轮才弹）；记下弹过的、忘掉过了重置时间一天的
    mutating func update(_ items: [(account: Account, usage: UsageEntry?)], now: Double) -> [Note] {
        memo = memo.filter { now - ($0.value.resets ?? $0.value.at) < Self.forget }
        var out: [Note] = []
        for (a, u) in items {
            guard a.kind != .api, a.login == .loggedIn, let u, u.errorCode != .auth else { continue }
            for p in Diagnosis.quotaPressure(u, now: now) where p.exhausted {
                let k = Self.key(a.stableID, p.window)
                if let m = memo[k], Self.sameRound(m.resets, p.resetsEpoch) {
                    if m.resets == nil { memo[k]?.resets = p.resetsEpoch }
                    continue
                }
                memo[k] = Round(resets: p.resetsEpoch, at: now)
                out.append(Note(event: .usedUp, account: a.ref, stable: a.stableID, name: a.displayName, window: p.window,
                                resetsEpoch: p.resetsEpoch))
            }
        }
        return out
    }
}
