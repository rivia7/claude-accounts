import Foundation

// 额度用完、恢复的通知（UsageAlerts，CcaApp 的 UsageNotifier 照它弹）：什么时候弹“用完了”、同一轮只弹一次、什么时候忘掉、通知上的字。
// 用完的规则和 Diagnosis（OfficeTests 测着）、cca 推给手机的（status.rs 的 exhausted）一样

private let t0 = 1794042000.0

private func acct(_ key: String, tool: ToolID = .claude, login: String = "logged_in", kind: String = "oauth", id: String? = nil) -> Account {
    var f: [String: Any] = ["key": key, "kind": kind, "login": login, "plan": "Max", "plan_tier": "advanced"]
    if let id { f["id"] = id }
    return Account.sample(f, tool: tool)
}

private func usage(five: Double?, week: Double? = 10, stale: Bool = false, fiveResets: Double = t0 + 3600,
                   weekResets: Double = t0 + 86400 * 3, error: String? = nil) -> UsageEntry {
    var d: [String: Any] = ["key": "a@example.com", "source": "cache", "stale": stale]
    if let five { d["five_hour"] = ["utilization": five, "resets_epoch": fiveResets, "reset": false] }
    if let week { d["seven_day"] = ["utilization": week, "resets_epoch": weekResets, "reset": false] }
    if let error { d["error_code"] = error }
    return try! JSONDecoder().decode(UsageEntry.self, from: JSONSerialization.data(withJSONObject: d))
}

func usageAlertsTests() {
    section("额度的通知")
    let a = acct("a@example.com")
    var s = UsageAlerts()
    let first = s.update([(a, usage(five: 100))], now: t0)
    check("额度：5 小时用完了弹一条，带着重置时间", first.count == 1 && first[0].event == .usedUp && first[0].window == .fiveHour
          && first[0].resetsEpoch == t0 + 3600 && first[0].account == a.ref, "\(first)")
    check("额度：同一轮不再弹", s.update([(a, usage(five: 100))], now: t0 + 60).isEmpty)
    check("额度：重置时间差一两秒还是同一轮（Codex）", s.update([(a, usage(five: 100, fiveResets: t0 + 3602))], now: t0 + 120).isEmpty)
    check("额度：没用完时什么都不弹，记着的不丢", s.update([(a, usage(five: 40))], now: t0 + 180).isEmpty && s.memo.count == 1)
    let next = s.update([(a, usage(five: 100, fiveResets: t0 + 3600 + 5 * 3600))], now: t0 + 4 * 3600)
    check("额度：下一轮又用完了再弹", next.count == 1 && next[0].resetsEpoch == t0 + 6 * 3600)
    check("额度：过了重置时间一天忘掉", { var x = s; _ = x.update([], now: t0 + 6 * 3600 + 86400 + 1); return x.memo.isEmpty }())

    var b = UsageAlerts()
    check("额度：本周也用完了各弹一条（5 小时在前）", b.update([(a, usage(five: 100, week: 100))], now: t0).map(\.window) == [.fiveHour, .week])
    check("额度：用完了不看数据旧不旧、显示成 100% 才算",
          UsageAlerts().updated([(a, usage(five: 100, stale: true))], now: t0).count == 1
          && UsageAlerts().updated([(a, usage(five: 99.4))], now: t0).isEmpty
          && UsageAlerts().updated([(a, usage(five: 99.5))], now: t0).count == 1)
    check("额度：快用完了（90%）、过了重置时间的不弹", UsageAlerts().updated([(a, usage(five: 95))], now: t0).isEmpty
          && UsageAlerts().updated([(a, usage(five: 100))], now: t0 + 7200).isEmpty)
    check("额度：没登录、登录失效、第三方 API、没有用量的不弹",
          UsageAlerts().updated([(acct("b@example.com", login: "logged_out"), usage(five: 100))], now: t0).isEmpty
          && UsageAlerts().updated([(a, usage(five: 100, error: "auth"))], now: t0).isEmpty
          && UsageAlerts().updated([(acct("k", kind: "api"), usage(five: 100))], now: t0).isEmpty
          && UsageAlerts().updated([(a, nil)], now: t0).isEmpty)
    let two = UsageAlerts().updated([(a, usage(five: 100)), (acct("c@example.com", tool: .codex), usage(five: 100))], now: t0)
    check("额度：账号按工具和键分开记", two.count == 2 && Set(two.map { UsageAlerts.key($0.stable, $0.window) }).count == 2)
    // 改了别名（账号表的键变了，cca 给的账号目录名没变）：同一轮的不再弹
    var renamed = UsageAlerts()
    _ = renamed.update([(acct("a@example.com", id: "a-dir"), usage(five: 100))], now: t0)
    check("额度：改了别名同一轮不再弹", renamed.update([(acct("工作(a@example.com)", id: "a-dir"), usage(five: 100))], now: t0 + 60).isEmpty)

    // 恢复的那一条：到了重置时间弹；重置时间不知道、已经过了的没有
    let n = first[0]
    check("恢复：到点弹，标识和用完的那条分开", n.back(now: t0)?.event == .back && n.back(now: t0)?.resetsEpoch == t0 + 3600
          && n.back(now: t0)?.identifier != n.identifier && n.back(now: t0 + 3600) == nil)
    let decoded = try? JSONDecoder().decode(UsageAlerts.self, from: JSONEncoder().encode(s))
    check("记着的存得下、读得回", decoded == s)

    // 字（和手机上的一样：CcaRemoteKit 的 UsageText.note）
    var utc = Calendar(identifier: .gregorian)
    utc.timeZone = TimeZone(identifier: "UTC")!
    let used = Texts.usageNote(n, now: t0, calendar: utc)
    check("字：用完了", used.title == "额度用完了" && used.subtitle == "a@example.com · Claude Code"
          && used.body == "5 小时的额度用完了，\(Texts.clock(t0 + 3600, calendar: utc)) 恢复", "\(used)")
    let week = UsageAlerts.Note(event: .usedUp, account: a.ref, stable: a.stableID, name: "工作", window: .week, resetsEpoch: t0 + 86400)
    check("字：本周的、明天恢复", Texts.usageNote(week, now: t0, calendar: utc).body == "本周的额度用完了，明天 \(Texts.clock(t0 + 86400, calendar: utc)) 恢复")
    let unknown = UsageAlerts.Note(event: .usedUp, account: a.ref, stable: a.stableID, name: "工作", window: .fiveHour, resetsEpoch: nil)
    check("字：不知道什么时候重置的不说几点", Texts.usageNote(unknown, now: t0, calendar: utc).body == "5 小时的额度用完了")
    let back = Texts.usageNote(n.back(now: t0)!, now: t0 + 3600, calendar: utc)
    check("字：恢复了", back.title == "额度恢复了" && back.body == "5 小时的额度恢复了，可以接着用了")
}

private extension UsageAlerts {
    /// 一份新的照这些用量弹哪些
    func updated(_ items: [(account: Account, usage: UsageEntry?)], now: Double) -> [Note] {
        var x = self
        return x.update(items, now: now)
    }
}
