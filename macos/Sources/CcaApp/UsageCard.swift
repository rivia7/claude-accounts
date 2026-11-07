import SwiftUI

/// 详情里的用量（magpie 的额度卡片）：每个窗口一格——名字和百分比在上，4 pt 的条，下面是什么时候重置；
/// 5 小时、本周在前，分模型的每周额度跟在后面，再是 credits、重置次数、额外用量。
/// 标题右边是数据从哪来（刚查询 / 缓存、多久以前）和“刷新用量”。和时间有关的文字每 30 秒重算一次。
struct UsageBlock: View {
    @Environment(AppModel.self) private var model
    let account: Account

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Text("用量").font(.system(size: 12.5, weight: .semibold)).foregroundStyle(Theme.fg)
                TimelineView(.periodic(from: .now, by: 30)) { context in
                    freshness(now: context.date.timeIntervalSince1970)
                }
                Spacer(minLength: 8)
                refreshButton
            }
            TimelineView(.periodic(from: .now, by: 30)) { context in
                content(now: context.date.timeIntervalSince1970)
            }
        }
    }

    private var refreshButton: some View {
        let a = account
        let refreshing = model.isUsageRefreshing(a.ref)
        return Button {
            model.refreshUsage(tool: a.tool, key: a.key, manual: true)
        } label: {
            Label(refreshing ? "刷新中…" : "刷新用量", systemImage: "arrow.clockwise")
        }
        .buttonStyle(.quiet)
        .disabled(!a.can.usage || refreshing)
        .help(reason ?? "经这个账号的出口查一次用量")
    }

    private var reason: String? {
        guard !account.can.usage, let w = account.canWhy["usage"] else { return nil }
        return Texts.canWhy(w, appLabel: model.appLabel(account.tool), tool: account.tool)
    }

    /// 数据从哪来：10 分钟内查到的写“刚刚查询”，否则是“缓存”；再写多久以前更新的、是否过时。
    @ViewBuilder
    private func freshness(now: Double) -> some View {
        if model.isUsageRefreshing(account.ref) {
            ProgressView().controlSize(.mini)
        } else if let u = model.usage(for: account), u.source != .none {
            let age = u.fetchedEpoch.map { now - $0 }
            let fresh = u.source == .live && (age ?? .infinity) < 600
            HStack(spacing: 5) {
                StatusDot(tone: u.stale ? .warn : fresh ? .ok : .idle, size: 6)
                Text(freshText(fresh: fresh, age: age, stale: u.stale))
            }
            .font(.system(size: 11.5))
            .foregroundStyle(Theme.muted)
            .accessibilityElement(children: .combine)
        }
    }

    private func freshText(fresh: Bool, age: Double?, stale: Bool) -> String {
        if fresh { return (age.map { Texts.agoPhrase($0) } ?? "刚刚") + "查询" }
        let when = age.map { " · " + Texts.agoPhrase($0) + "更新" } ?? ""
        return (stale ? "数据已经过时" : "缓存") + when
    }

    @ViewBuilder
    private func content(now: Double) -> some View {
        if let u = model.usage(for: account), u.source != .none {
            // 数据旧了（过期，或者这次没查到、用的是缓存）就用灰色显示
            let dim = u.stale || (u.source == .cache && u.errorCode != nil)
            VStack(alignment: .leading, spacing: 12) {
                LazyVGrid(columns: [GridItem(.flexible(), spacing: 22, alignment: .top), GridItem(.flexible(), alignment: .top)],
                          alignment: .leading, spacing: 14) {
                    QuotaWindowView(title: "5 小时", window: u.fiveHour, now: now, dim: dim, prominent: true)
                    QuotaWindowView(title: "本周", window: u.sevenDay, now: now, dim: dim, prominent: true)
                    ForEach(modelWindows(u), id: \.0) { item in
                        QuotaWindowView(title: item.0, window: item.1, now: now, dim: dim)
                    }
                }
                let extras = extraRows(u)
                if !extras.isEmpty {
                    HStack(spacing: 18) {
                        ForEach(extras, id: \.0) { row in
                            HStack(spacing: 5) {
                                Text(row.0).foregroundStyle(Theme.muted)
                                Text(row.1).fontWeight(.semibold).monospacedDigit().foregroundStyle(Theme.fg2)
                            }
                            .accessibilityElement(children: .combine)
                        }
                    }
                    .font(.system(size: 12))
                }
                if let code = u.errorCode {
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        StatusDot(tone: .warn, size: 6)
                        Text(nonEmpty(u.error) ?? Texts.usageError(code))
                            .foregroundStyle(Theme.fg2)
                            .textSelection(.enabled)
                    }
                    .font(.system(size: 12))
                }
            }
        } else {
            VStack(alignment: .leading, spacing: 4) {
                Text(emptyReason).font(.system(size: 12.5)).foregroundStyle(Theme.muted)
                if let u = model.usage(for: account), let e = nonEmpty(u.error) {
                    Text(e).font(.system(size: 12)).foregroundStyle(Theme.fg2).textSelection(.enabled)
                }
            }
        }
    }

    private var emptyReason: String {
        if let why = account.canWhy["usage"], !account.can.usage {
            return Texts.canWhy(why, appLabel: model.appLabel(account.tool), tool: account.tool)
        }
        switch account.login {
        case .loggedOut: return "还没登录，查不了用量。"
        case .emailMismatch: return "登录的不是这个账号，查不了用量。"
        default: return "还没有用量数据：点“刷新用量”查一次。"
        }
    }

    /// 分模型的每周额度：Opus、Sonnet、应用，加上 model_scoped（按名字去重）。
    private func modelWindows(_ u: UsageEntry) -> [(String, UsageWindow)] {
        var out: [(String, UsageWindow)] = []
        var seen = Set<String>()
        let fixed: [(String, UsageWindow?)] = [("Opus", u.sevenDayOpus), ("Sonnet", u.sevenDaySonnet), ("应用", u.sevenDayOauthApps)]
        for (name, w) in fixed {
            if let w, w.utilization != nil, seen.insert(name.lowercased()).inserted { out.append((name + " 本周", w)) }
        }
        for m in u.modelScoped {
            guard let name = nonEmpty(m.name), m.window.utilization != nil, seen.insert(name.lowercased()).inserted else { continue }
            out.append((name, m.window))
        }
        return out
    }

    /// credits、还能重置几次、额外用量。
    private func extraRows(_ u: UsageEntry) -> [(String, String)] {
        var rows: [(String, String)] = []
        if let c = u.credits {
            if c.unlimited == true {
                rows.append(("Credits", "不限"))
            } else if c.hasCredits == true, let b = nonEmpty(c.balance) {
                rows.append(("Credits 余额", b))
            } else if c.hasCredits == false {
                rows.append(("Credits", "没有"))
            }
        }
        if let n = u.resetCredits?.availableCount {
            rows.append(("还能重置额度", "\(n) 次"))
        }
        if let e = u.extraUsage, e.isEnabled == true {
            rows.append(("额外用量", e.utilization.map { Texts.percent($0) } ?? "已开启"))
        }
        return rows
    }
}

/// 一个额度窗口：名字（灰）和百分比（半粗）在一行，下面一根 4 pt 的条，再下面是重置时间。
/// 已重置的不显示旧的百分比；接近、到了上限时百分比和条换成琥珀色、红色。
struct QuotaWindowView: View {
    let title: String
    let window: UsageWindow?
    let now: Double
    var dim = false
    var prominent = false

    var body: some View {
        let reset = window?.isReset(now: now) ?? false
        let pct = window?.utilization
        let level = UsageLevel(percent: pct ?? 0)
        let resetText = reset ? nil : window?.resetsEpoch.map { Texts.resetText(epoch: $0, now: now) }
        VStack(alignment: .leading, spacing: 5) {
            HStack(alignment: .firstTextBaseline) {
                Text(title).font(.system(size: 12)).foregroundStyle(Theme.muted).lineLimit(1)
                Spacer(minLength: 6)
                Text(pct == nil ? "—" : reset ? "已重置" : Texts.percent(pct!))
                    .font(.system(size: prominent ? 15 : 12.5, weight: .semibold).monospacedDigit())
                    .foregroundStyle(valueColor(pct: pct, reset: reset, level: level))
            }
            QuotaBar(fraction: reset ? 0 : (pct ?? 0) / 100, color: dim ? Theme.faint : Palette.usage(level), height: 4)
            if prominent {
                Text(resetText ?? (reset ? "已重置" : " "))
                    .font(.system(size: 11))
                    .foregroundStyle(Theme.muted)
                    .monospacedDigit()
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(title)用量")
        .accessibilityValue(pct == nil ? "没有数据" : reset ? "已重置" : "\(Texts.percent(pct!))\(resetText.map { "，" + $0 } ?? "")")
    }

    private func valueColor(pct: Double?, reset: Bool, level: UsageLevel) -> Color {
        guard pct != nil, !reset, !dim else { return Theme.muted }
        return level == .normal ? Theme.fg : Palette.usage(level)
    }
}

/// 列表一行右边的小用量：5h、周两格，每格一个数和一根细条。每 30 秒重算一次（重置时间过了就显示“已重置”）。
struct RowQuota: View {
    let usage: UsageEntry
    var width: CGFloat = 58

    var body: some View {
        TimelineView(.periodic(from: .now, by: 30)) { context in
            HStack(spacing: 12) {
                cell("5h", usage.fiveHour, now: context.date.timeIntervalSince1970)
                cell("周", usage.sevenDay, now: context.date.timeIntervalSince1970)
            }
        }
    }

    private func cell(_ label: String, _ window: UsageWindow?, now: Double) -> some View {
        let reset = window?.isReset(now: now) ?? false
        let pct = window?.utilization
        let level = UsageLevel(percent: pct ?? 0)
        let value: String = pct == nil ? "—" : reset ? "已重置" : Texts.percent(pct!)
        let color: Color = {
            guard pct != nil, !reset, !usage.stale else { return Theme.muted }
            return level == .normal ? Theme.fg2 : Palette.usage(level)
        }()
        return VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 0) {
                Text(label).font(.system(size: 10.5)).foregroundStyle(Theme.muted)
                Spacer(minLength: 2)
                Text(value).font(.system(size: 11.5, weight: .semibold).monospacedDigit()).foregroundStyle(color)
            }
            QuotaBar(fraction: reset ? 0 : (pct ?? 0) / 100, color: usage.stale ? Theme.faint : Palette.usage(level), height: 3)
        }
        .frame(width: width)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label == "5h" ? "5 小时用量" : "本周用量")
        .accessibilityValue(value == "—" ? "没有数据" : value)
    }
}
