import SwiftUI

/// 用量卡片：5 小时、本周、分模型的每周额度、Codex 的 credits / 重置次数，来源和出错原因。
/// 和时间有关的文字放在 TimelineView 里，每 30 秒重算一次（“2 小时 13 分后重置”“3 分钟前更新”会往前走）。
struct UsageCard: View {
    @Environment(AppModel.self) private var model
    let account: Account

    var body: some View {
        Card("用量", systemImage: "gauge.with.dots.needle.33percent") {
            if model.isUsageRefreshing(account.ref) {
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text("刷新中…").font(.caption).foregroundStyle(.secondary)
                }
            }
        } content: {
            TimelineView(.periodic(from: .now, by: 30)) { context in
                content(now: context.date.timeIntervalSince1970)
            }
        }
    }

    @ViewBuilder
    private func content(now: Double) -> some View {
        if let u = model.usage(for: account), u.source != .none {
            // 数据旧了（过期，或者这次没查到、用的是缓存）就用灰色显示
            let dim = u.stale || (u.source == .cache && u.errorCode != nil)
            VStack(alignment: .leading, spacing: 14) {
                UsageBar(title: "5 小时", window: u.fiveHour, now: now, dim: dim)
                UsageBar(title: "本周", window: u.sevenDay, now: now, dim: dim)
                let models = modelWindows(u)
                if !models.isEmpty {
                    Divider()
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 180), spacing: 16)], alignment: .leading, spacing: 12) {
                        ForEach(models, id: \.0) { item in
                            UsageBar(title: item.0 + " 本周", window: item.1, now: now, dim: dim, compact: true)
                        }
                    }
                }
                extras(u)
                footer(u, now: now)
            }
        } else {
            VStack(alignment: .leading, spacing: 6) {
                Text(emptyReason).foregroundStyle(.secondary)
                if let u = model.usage(for: account), let e = nonEmpty(u.error) {
                    Label(e, systemImage: "exclamationmark.circle").font(.caption).foregroundStyle(.secondary)
                        .symbolRenderingMode(.multicolor)
                        .textSelection(.enabled)
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
            if let w, w.utilization != nil, seen.insert(name.lowercased()).inserted { out.append((name, w)) }
        }
        for m in u.modelScoped {
            guard let name = nonEmpty(m.name), m.window.utilization != nil, seen.insert(name.lowercased()).inserted else { continue }
            out.append((name, m.window))
        }
        return out
    }

    @ViewBuilder
    private func extras(_ u: UsageEntry) -> some View {
        let rows = extraRows(u)
        if !rows.isEmpty {
            Divider()
            VStack(alignment: .leading, spacing: 6) {
                ForEach(rows, id: \.0) { row in
                    HStack {
                        Label(row.0, systemImage: row.2).foregroundStyle(.secondary)
                        Spacer()
                        Text(row.1).monospacedDigit().fontWeight(.medium)
                    }
                    .font(.callout)
                    .accessibilityElement(children: .combine)
                }
            }
        }
    }

    private func extraRows(_ u: UsageEntry) -> [(String, String, String)] {
        var rows: [(String, String, String)] = []
        if let c = u.credits {
            if c.unlimited == true {
                rows.append(("Credits", "不限", "creditcard"))
            } else if c.hasCredits == true, let b = nonEmpty(c.balance) {
                rows.append(("Credits 余额", b, "creditcard"))
            } else if c.hasCredits == false {
                rows.append(("Credits", "没有", "creditcard"))
            }
        }
        if let n = u.resetCredits?.availableCount {
            rows.append(("还能重置额度", "\(n) 次", "arrow.counterclockwise"))
        }
        if let e = u.extraUsage, e.isEnabled == true {
            rows.append(("额外用量", e.utilization.map { Texts.percent($0) } ?? "已开启", "plus.circle"))
        }
        return rows
    }

    /// 来源：10 分钟内查到的写“刚查询”，否则是“缓存”；再写多久以前更新的、数据是否过时、这次的出错原因。
    private func footer(_ u: UsageEntry, now: Double) -> some View {
        let age = u.fetchedEpoch.map { now - $0 }
        let fresh = u.source == .live && (age ?? .infinity) < 600
        return VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                if fresh {
                    Tag(text: "刚查询", color: .green)
                } else {
                    Tag(text: "缓存")
                }
                if let age {
                    Text("\(Texts.agoPhrase(age))更新").font(.caption).foregroundStyle(.secondary)
                }
                if u.stale {
                    Label("数据已经过时", systemImage: "clock.badge.exclamationmark")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            if let code = u.errorCode {
                Label(nonEmpty(u.error) ?? Texts.usageError(code), systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .symbolRenderingMode(.multicolor)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
        }
    }
}

/// 一个窗口的用量条：标题、百分比（颜色按档）、重置时间。已重置的不显示旧的百分比（也不再重复写“已重置”）。
struct UsageBar: View {
    let title: String
    let window: UsageWindow?
    let now: Double
    var dim = false
    var compact = false

    var body: some View {
        let reset = window?.isReset(now: now) ?? false
        let pct = window?.utilization ?? 0
        let color = Palette.usage(percent: pct)
        let resetText = reset ? nil : window?.resetsEpoch.map { Texts.resetText(epoch: $0, now: now) }
        VStack(alignment: .leading, spacing: compact ? 3 : 5) {
            HStack(alignment: .firstTextBaseline) {
                Text(title).font(compact ? .caption.weight(.medium) : .callout.weight(.medium))
                    .foregroundStyle(dim ? .secondary : .primary)
                Spacer()
                if window?.utilization == nil {
                    Text("—").foregroundStyle(.secondary)
                } else if reset {
                    Text("已重置").font(compact ? .caption : .callout).foregroundStyle(.secondary)
                } else {
                    Text(Texts.percent(pct))
                        .font(compact ? .caption.monospacedDigit().weight(.semibold) : .title3.monospacedDigit().weight(.semibold))
                        .foregroundStyle(dim ? AnyShapeStyle(.secondary)
                                         : UsageLevel(percent: pct) == .normal ? AnyShapeStyle(.primary) : AnyShapeStyle(color))
                }
            }
            Meter(fraction: reset ? 0 : pct / 100, color: color, height: compact ? 5 : 8, dim: dim)
            if let resetText, !compact {
                Text(resetText).font(.caption).foregroundStyle(.secondary)
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(title)用量")
        .accessibilityValue(window?.utilization == nil ? "没有数据" : reset ? "已重置" : "\(Texts.percent(pct))\(resetText.map { "，" + $0 } ?? "")")
    }
}

/// 侧栏、菜单栏里的小用量条：“5h 13%”。每 30 秒重算一次（重置时间过了就显示“已重置”）。
struct MiniUsage: View {
    let label: String
    let window: UsageWindow?
    let stale: Bool
    var width: CGFloat = 30

    var body: some View {
        TimelineView(.periodic(from: .now, by: 30)) { context in
            cell(now: context.date.timeIntervalSince1970)
        }
    }

    private func cell(now: Double) -> some View {
        let reset = window?.isReset(now: now) ?? false
        let pct = window?.utilization
        let value: String = pct == nil ? "—" : reset ? "已重置" : Texts.percent(pct!)
        let style: AnyShapeStyle = {
            guard let p = pct, !reset, !stale else { return AnyShapeStyle(.secondary) }
            return UsageLevel(percent: p) == .normal ? AnyShapeStyle(.primary) : AnyShapeStyle(Palette.usage(percent: p))
        }()
        return HStack(spacing: 4) {
            Text(label).font(.system(size: 10, weight: .semibold)).foregroundStyle(.secondary).frame(width: 16, alignment: .trailing)
            Meter(fraction: reset ? 0 : (pct ?? 0) / 100, color: Palette.usage(percent: pct ?? 0), height: 4, dim: stale)
                .frame(width: width)
            Text(value)
                .font(.system(size: 11, weight: .medium).monospacedDigit())
                .foregroundStyle(style)
                .frame(width: 40, alignment: .leading)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label == "5h" ? "5 小时用量" : "本周用量")
        .accessibilityValue(value == "—" ? "没有数据" : value)
    }
}
