import SwiftUI

/// 用量：大块的额度（5 小时、本周，还有分模型的每周额度，例如 Claude 的 Fable），下面是旧的分模型额度、Codex 的 credits，
/// ChatGPT 账号还能重置几次（旁边是「重置额度…」）。标题右边是数据从哪来（刚查询 / 缓存、多久以前）和“刷新用量”。
/// 和时间有关的文字放在 TimelineView 里，每 30 秒重算一次。
struct UsageSection: View {
    @Environment(AppModel.self) private var model
    let account: Account

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            SectionHeader("用量") {
                TimelineView(.periodic(from: .now, by: 30)) { context in
                    freshness(now: context.date.timeIntervalSince1970)
                }
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
        .buttonStyle(.glass)
        .disabled(!a.can.usage || refreshing)
        .help(reason ?? "经这个账号的出口查一次用量")
    }

    private var reason: String? {
        guard !account.can.usage, let w = account.canWhy["usage"] else { return nil }
        return Texts.canWhy(w, appLabel: model.appLabel(account.tool), tool: account.tool)
    }

    /// 数据从哪来：10 分钟内查到的写“刚查询”，否则是“缓存”；再写多久以前更新的、是否过时。
    @ViewBuilder
    private func freshness(now: Double) -> some View {
        if model.isUsageRefreshing(account.ref) {
            ProgressView().controlSize(.small)
        } else if let u = model.usage(for: account), u.source != .none {
            let age = u.fetchedEpoch.map { now - $0 }
            let fresh = u.source == .live && (age ?? .infinity) < 600
            HStack(spacing: 5) {
                if u.stale {
                    Image(systemName: "clock.badge.exclamationmark").foregroundStyle(.orange)
                } else if fresh {
                    Circle().fill(.green).frame(width: 6, height: 6)
                }
                Text(freshText(fresh: fresh, age: age, stale: u.stale))
            }
            .font(.caption)
            .foregroundStyle(.secondary)
            .accessibilityElement(children: .combine)
        }
    }

    private func freshText(fresh: Bool, age: Double?, stale: Bool) -> String {
        if fresh { return (age.map { Texts.agoPhrase($0) } ?? "刚刚") + "查询" }
        let when = age.map { "，" + Texts.agoPhrase($0) + "更新" } ?? ""
        return (stale ? "数据已经过时" : "缓存") + when
    }

    @ViewBuilder
    private func content(now: Double) -> some View {
        if let u = model.usage(for: account), u.source != .none {
            // 数据旧了（过期，或者这次没查到、用的是缓存）就用灰色显示
            let dim = u.stale || (u.source == .cache && u.errorCode != nil)
            let models = modelTiles(u)
            VStack(alignment: .leading, spacing: 16) {
                QuotaGrid {
                    QuotaTile(title: "5 小时", window: u.fiveHour, now: now, dim: dim)
                    QuotaTile(title: "本周", window: u.sevenDay, now: now, dim: dim)
                    ForEach(models, id: \.name) { m in
                        QuotaTile(title: m.name, window: m.window, now: now, dim: dim, note: m.note)
                    }
                }
                let minor = minorItems(u, shown: Set(models.map { $0.name.lowercased() }), now: now, dim: dim)
                if !minor.isEmpty {
                    LazyVGrid(columns: [GridItem(.flexible(), spacing: 28), GridItem(.flexible())], alignment: .leading, spacing: 10) {
                        ForEach(minor) { $0 }
                    }
                    .padding(.horizontal, 4)
                }
                if let rc = u.resetCredits, rc.availableCount != nil {
                    ResetCreditsRow(account: account, credits: rc, now: now)
                        .padding(.horizontal, 4)
                }
                if let code = u.errorCode {
                    HStack(spacing: 8) {
                        Label(nonEmpty(u.error) ?? Texts.usageError(code), systemImage: "exclamationmark.triangle.fill")
                            .font(.caption)
                            .symbolRenderingMode(.multicolor)
                            .foregroundStyle(.secondary)
                            .textSelection(.enabled)
                        reloginLink(u)
                    }
                }
            }
        } else {
            VStack(alignment: .leading, spacing: 6) {
                Text(emptyReason).foregroundStyle(.secondary)
                if let u = model.usage(for: account), let e = nonEmpty(u.error) {
                    HStack(spacing: 8) {
                        Label(e, systemImage: "exclamationmark.circle").font(.caption).foregroundStyle(.secondary)
                            .symbolRenderingMode(.multicolor)
                            .textSelection(.enabled)
                        reloginLink(u)
                    }
                }
            }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
        }
    }

    /// 查到的结果里没有用量：Claude 登录失效时只会这样（分不清是不是别的原因），放个重新登录的入口；
    /// 确定失效了的（`auth`）名字旁边的主按钮已经是重新登录了。
    @ViewBuilder
    private func reloginLink(_ u: UsageEntry) -> some View {
        if u.errorCode == .nolimits, !account.isAPI {
            Button("重新登录…") { model.relogin(account) }
                .buttonStyle(.borderless)
                .font(.caption)
        }
    }

    private var emptyReason: String {
        if let why = account.canWhy["usage"], !account.can.usage {
            return Texts.canWhy(why, appLabel: model.appLabel(account.tool), tool: account.tool)
        }
        if model.loginExpired(account) { return "登录已失效，查不了用量。" }
        switch account.login {
        case .loggedOut: return "还没登录，查不了用量。"
        case .emailMismatch: return "登录的不是这个账号，查不了用量。"
        default: return "还没有用量数据：点“刷新用量”查一次。"
        }
    }

    /// 分模型的每周额度（model_scoped，按名字去重）：各是一大块，下面一行说它和本周总额度的关系（cca 认得出时）。
    private func modelTiles(_ u: UsageEntry) -> [(name: String, window: UsageWindow, note: String)] {
        var seen = Set<String>()
        return u.modelScoped.compactMap { m in
            guard let name = nonEmpty(m.name), m.window.utilization != nil, seen.insert(name.lowercased()).inserted else { return nil }
            return (name, m.window, Texts.weeklyShare(allowance: m.allowancePercentOfWeekly, used: m.usedPercentOfWeekly))
        }
    }

    /// 大块下面的小行：旧的分模型每周额度（Opus、Sonnet、应用；大块里已经有的不再写），再是 credits、额外用量。
    private func minorItems(_ u: UsageEntry, shown: Set<String>, now: Double, dim: Bool) -> [MinorQuota] {
        var out: [MinorQuota] = []
        var seen = shown
        let fixed: [(String, UsageWindow?)] = [("Opus", u.sevenDayOpus), ("Sonnet", u.sevenDaySonnet), ("应用", u.sevenDayOauthApps)]
        for (name, w) in fixed {
            if let w, w.utilization != nil, seen.insert(name.lowercased()).inserted {
                out.append(MinorQuota(title: name + " 本周", window: w, now: now, dim: dim))
            }
        }
        if let c = u.credits {
            if c.unlimited == true {
                out.append(MinorQuota(title: "Credits", value: "不限", systemImage: "creditcard"))
            } else if c.hasCredits == true, let b = nonEmpty(c.balance) {
                out.append(MinorQuota(title: "Credits 余额", value: b, systemImage: "creditcard"))
            } else if c.hasCredits == false {
                out.append(MinorQuota(title: "Credits", value: "没有", systemImage: "creditcard"))
            }
        }
        if let e = u.extraUsage, e.isEnabled == true {
            out.append(MinorQuota(title: "额外用量", value: e.utilization.map { Texts.percent($0) } ?? "已开启", systemImage: "plus.circle"))
        }
        return out
    }
}

/// 额度的大块：放得下就排成一行（每块至少 minWidth 宽），放不下两块一行，单出来的最后一块占满一行。同一行的一样高。
/// 用 Layout：换排法时还是同一份视图，不用先量宽度再排一遍。
struct QuotaGrid: Layout {
    var minWidth: CGFloat = 196
    var spacing: CGFloat = 12

    /// 每行放哪几块
    private func rows(_ count: Int, width: CGFloat) -> [Range<Int>] {
        guard count > 0 else { return [] }
        let fits = { (n: Int) in CGFloat(n) * minWidth + CGFloat(n - 1) * spacing <= width }
        let perRow = fits(count) ? count : (fits(2) ? 2 : 1)
        return stride(from: 0, to: count, by: perRow).map { $0..<min($0 + perRow, count) }
    }

    private func tileWidth(_ row: Range<Int>, _ width: CGFloat) -> CGFloat {
        (width - spacing * CGFloat(row.count - 1)) / CGFloat(row.count)
    }

    private func layout(_ subviews: Subviews, width: CGFloat) -> [(row: Range<Int>, width: CGFloat, height: CGFloat)] {
        rows(subviews.count, width: width).map { r in
            let w = tileWidth(r, width)
            let h = r.map { subviews[$0].sizeThatFits(ProposedViewSize(width: w, height: nil)).height }.max() ?? 0
            return (r, w, h)
        }
    }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width.flatMap { $0.isFinite ? $0 : nil } ?? CGFloat(subviews.count) * (minWidth + spacing) - spacing
        let rows = layout(subviews, width: width)
        let height = rows.map(\.height).reduce(0, +) + spacing * CGFloat(max(rows.count - 1, 0))
        return CGSize(width: width, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var y = bounds.minY
        for (row, w, h) in layout(subviews, width: bounds.width) {
            for (k, i) in row.enumerated() {
                subviews[i].place(at: CGPoint(x: bounds.minX + CGFloat(k) * (w + spacing), y: y), anchor: .topLeading,
                                  proposal: ProposedViewSize(width: w, height: h))
            }
            y += h + spacing
        }
    }
}

/// 一块额度：标题和重置时间，大号的百分比（只有接近、超过上限时才着色），一根条；分模型的额度下面再写一句它和本周总额度的关系。
/// 已重置的不显示旧的百分比。
struct QuotaTile: View {
    let title: String
    let window: UsageWindow?
    let now: Double
    var dim = false
    var note: String?

    var body: some View {
        let reset = window?.isReset(now: now) ?? false
        let pct = window?.utilization
        let level = UsageLevel(percent: pct ?? 0)
        let color = Palette.usage(level)
        let resetText = reset ? nil : window?.resetsEpoch.map { Texts.resetText(epoch: $0, now: now) }
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline) {
                Text(title).font(.callout.weight(.semibold)).foregroundStyle(.secondary)
                Spacer(minLength: 8)
                if let resetText {
                    Text(resetText).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Group {
                if let pct, !reset {
                    HStack(alignment: .firstTextBaseline, spacing: 1) {
                        Text("\(Int(pct.rounded()))")
                            .font(.system(size: 32, weight: .semibold).monospacedDigit())
                        Text("%")
                            .font(.system(size: 17, weight: .semibold))
                    }
                    .foregroundStyle(dim ? AnyShapeStyle(.secondary) : level == .normal ? AnyShapeStyle(.primary) : AnyShapeStyle(color))
                } else {
                    Text(reset ? "已重置" : "—")
                        .font(.system(size: 22, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .frame(height: 38, alignment: .bottom)
                }
            }
            Meter(fraction: reset ? 0 : (pct ?? 0) / 100, color: color, height: 6, dim: dim)
            if let note = nonEmpty(note) {
                Text(note)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(title)用量")
        .accessibilityValue((pct == nil ? "没有数据" : reset ? "已重置" : "\(Texts.percent(pct!))\(resetText.map { "，" + $0 } ?? "")")
                            + (nonEmpty(note).map { "。" + $0 } ?? ""))
    }
}

/// ChatGPT 账号还能重置几次额度（codex 里 /usage 的 Redeem reset），最早的那次什么时候过期（cca 算好的）；有机会时旁边是
/// 「重置额度…」（cca 会先问：会用掉一次，默认不重置；用量还不需要重置时不扣次数）。
struct ResetCreditsRow: View {
    @Environment(AppModel.self) private var model
    let account: Account
    let credits: ResetCredits
    let now: Double

    var body: some View {
        let a = account
        let count = credits.availableCount ?? 0
        let busy = model.activity(a.ref) != nil
        let resetting = model.activity(a.ref)?.kind == .reset
        HStack(spacing: 10) {
            Label {
                HStack(spacing: 6) {
                    Text("还能重置额度")
                    Text("\(count) 次").monospacedDigit().fontWeight(.medium).foregroundStyle(.primary)
                    if count > 0, let e = credits.nextExpiresEpoch {
                        Text("·")
                        Text((count > 1 ? "最早一次 " : "") + Texts.expiryText(epoch: e, now: now)).lineLimit(1)
                    }
                }
            } icon: {
                Image(systemName: "arrow.counterclockwise")
            }
            .foregroundStyle(.secondary)
            Spacer(minLength: 8)
            if count > 0 {
                Button {
                    model.resetQuota(a.ref)
                } label: {
                    Label(resetting ? "重置中…" : "重置额度…", systemImage: "arrow.counterclockwise.circle")
                }
                .buttonStyle(.glass)
                .controlSize(.small)
                .disabled(busy || !a.can.usage)
                .help("用掉一次重置机会，把 5 小时和每周的用量清零（会先问你；用量还不需要重置时不扣次数）")
            }
        }
        .font(.callout)
        .accessibilityElement(children: .contain)
    }
}

/// 额度砖下面的一小行：分模型的额度（名字、短条、百分比），或者一个数值（credits、重置次数）。
struct MinorQuota: View, Identifiable {
    let title: String
    var window: UsageWindow?
    var now: Double = 0
    var dim = false
    var value: String?
    var systemImage: String?

    var id: String { title }

    var body: some View {
        HStack(spacing: 10) {
            if let systemImage {
                Label(title, systemImage: systemImage).foregroundStyle(.secondary)
            } else {
                Text(title).foregroundStyle(.secondary)
            }
            Spacer(minLength: 8)
            if let window {
                let reset = window.isReset(now: now)
                let pct = window.utilization ?? 0
                Meter(fraction: reset ? 0 : pct / 100, color: Palette.usage(percent: pct), height: 5, dim: dim)
                    .frame(width: 72)
                Text(reset ? "已重置" : Texts.percent(pct))
                    .monospacedDigit()
                    .fontWeight(.medium)
                    .foregroundStyle(reset || dim ? .secondary : .primary)
                    .frame(minWidth: 40, alignment: .trailing)
            } else if let value {
                Text(value).monospacedDigit().fontWeight(.medium)
            }
        }
        .font(.callout)
        .lineLimit(1)
        .accessibilityElement(children: .combine)
    }
}

/// 侧栏、菜单栏右边的两行小用量（5 小时、本周）。
struct UsageStack: View {
    let usage: UsageEntry
    var width: CGFloat = 30

    var body: some View {
        VStack(alignment: .trailing, spacing: 3) {
            MiniUsage(label: "5h", window: usage.fiveHour, stale: usage.stale, width: width)
            MiniUsage(label: "周", window: usage.sevenDay, stale: usage.stale, width: width)
        }
    }
}

/// 一行小用量：“5h ▬▬ 13%”。每 30 秒重算一次（重置时间过了就显示“已重置”）。
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
            guard let p = pct, !reset, !stale else { return AnyShapeStyle(.plainSecondary) }
            return UsageLevel(percent: p) == .normal ? AnyShapeStyle(.primary) : AnyShapeStyle(Palette.usage(percent: p))
        }()
        return HStack(spacing: 5) {
            Text(label).font(.system(size: 10, weight: .medium)).foregroundStyle(.plainSecondary).frame(width: 14, alignment: .trailing)
            Meter(fraction: reset ? 0 : (pct ?? 0) / 100, color: Palette.usage(percent: pct ?? 0), height: 4, dim: stale)
                .frame(width: width)
            Text(value)
                .font(.system(size: 11, weight: .semibold).monospacedDigit())
                .foregroundStyle(style)
                .frame(width: 34, alignment: .trailing)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label == "5h" ? "5 小时用量" : "本周用量")
        .accessibilityValue(value == "—" ? "没有数据" : value)
    }
}
