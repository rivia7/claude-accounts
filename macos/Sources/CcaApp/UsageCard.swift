import SwiftUI

/// 用量：大块的额度（5 小时、本周，还有分模型的每周额度，例如 Claude 的 Fable），下面是旧的分模型额度、Codex 的 credits；
/// 再下面是一行一行的信息（InfoRow，右边是这一行的操作）：ChatGPT 账号还能重置几次（「重置额度…」）、查用量出的错、
/// Claude Code 记的套餐过时了（「重新登录…」）、只能在网页上看的重置次数和每月 API 额度、API key。
/// 标题右边是数据从哪来（刚查询 / 缓存、多久以前）和「刷新用量」。和时间有关的文字放在 TimelineView 里，每 30 秒重算一次。
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
                let now = context.date.timeIntervalSince1970
                VStack(alignment: .leading, spacing: 12) {
                    content(now: now)
                    infoRows(now: now)
                }
            }
        }
    }

    private var refreshButton: some View {
        let a = account
        let refreshing = model.isUsageRefreshing(a.ref)
        return SectionAction(refreshing ? "刷新中…" : "刷新用量", systemImage: "arrow.clockwise") {
            model.refreshUsage(tool: a.tool, key: a.key, manual: true)
        }
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
        if let u = model.usage(for: account), u.hasQuota {
            QuotaGroupsView(usage: u, now: now, dim: u.stale || (u.source == .cache && u.errorCode != nil))
        } else if let u = model.usage(for: account), u.source != .none {
            // 数据旧了（过期，或者这次没查到、用的是缓存）就用灰色显示
            let dim = u.stale || (u.source == .cache && u.errorCode != nil)
            let models = u.modelQuotas
            VStack(alignment: .leading, spacing: 16) {
                QuotaGrid {
                    if !u.noFiveHourLimit {   // 没有 5 小时限制的套餐（Ultra、Pro 500）不画这一块
                        QuotaTile(title: "5 小时", window: u.fiveHour, now: now, dim: dim)
                    }
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
                }
            }
        } else {
            Text(emptyReason)
                .foregroundStyle(.secondary)
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
        }
    }

    /// 额度下面一行一行的信息（InfoRow）：右边的操作一样大、一样宽，右边和「刷新用量」对齐。没登录也显示只能在网页上看的、API key。
    @ViewBuilder
    private func infoRows(now: Double) -> some View {
        let a = account
        let u = model.usage(for: a)
        let busy = model.activity(a.ref) != nil
        VStack(alignment: .leading, spacing: 8) {
            if let u, u.source != .none, let rc = u.resetCredits, rc.availableCount != nil {
                ResetCreditsRow(account: a, credits: rc, now: now)
            }
            if let u, let error = nonEmpty(u.error) ?? u.errorCode.map(Texts.usageError) {
                // 查到的结果里没有用量：Claude 登录失效时只会这样（分不清是不是别的原因），放个重新登录的入口；
                // 确定失效了的（auth）名字旁边的主按钮已经是重新登录了
                InfoRow(Text(error), systemImage: "exclamationmark.triangle.fill", tint: .orange) {
                    if u.errorCode == .nolimits, !a.isAPI {
                        reloginButton(busy: busy)
                    }
                }
            }
            if let u, u.source != .none, let old = u.outdatedLoginPlan {
                InfoRow(Text(Texts.outdatedLoginPlan(old, current: a.plan)), systemImage: "info.circle") {
                    reloginButton(busy: busy)
                        .help("Claude Code 只在登录时记录套餐，升降级后不会自动更新（额度按实际套餐计算）。"
                              + "点击后会在终端运行 \(model.loginHandoff(a).display)：先退出当前登录，再重新登录")
                }
            }
            if a.can.limitResets {
                InfoRow(Text("重置次数和到期日"), systemImage: "ticket") {
                    RowAction.web("在 claude.ai 查看") { model.openLimitResets(a.ref) }
                        .disabled(busy)
                        .help("用这个账号自己的浏览器和出口打开 claude.ai 的用量页；要登录时邮箱已复制好，直接粘贴")
                }
            }
            if a.can.billing {
                InfoRow(Text("订阅附带的每月 API 额度"), systemImage: "creditcard") {
                    RowAction.web("在控制台查看") { model.openBilling(a.ref) }
                        .disabled(busy)
                        .help("用这个账号自己的浏览器和出口打开控制台的账单页；要登录时邮箱已复制好，直接粘贴")
                }
            }
            if a.can.apiKey {
                APIKeyRow(account: a).id(a.ref)
            }
        }
    }

    private func reloginButton(busy: Bool) -> some View {
        RowAction("重新登录…", systemImage: "person.badge.key") { model.relogin(account) }
            .disabled(busy)
    }

    private var emptyReason: String {
        if let why = account.canWhy["usage"], !account.can.usage {
            return Texts.canWhy(why, appLabel: model.appLabel(account.tool), tool: account.tool)
        }
        if model.loginExpired(account) { return "登录已失效，无法查询用量。" }
        switch account.login {
        case .loggedOut: return "未登录，无法查询用量。"
        case .emailMismatch: return "登录的不是这个账号，无法查询用量。"
        default: return "还没有用量数据：点「刷新用量」查询。"
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
                out.append(MinorQuota(title: "Credits", value: "无", systemImage: "creditcard"))
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
            // 过了重置时间，那句话（已占多少、用完了）是旧的：等下次查到再说
            if !reset, let note = nonEmpty(note) {
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
        .accessibilityValue((pct == nil ? "暂无数据" : reset ? "已重置" : "\(Texts.percent(pct!))\(resetText.map { "，" + $0 } ?? "")")
                            + (reset ? "" : nonEmpty(note).map { "。" + $0 } ?? ""))
    }
}

/// ChatGPT 账号还能重置几次额度（codex 里 /usage 的 Redeem reset），最早的那次什么时候过期（cca 算好的）；还有次数时右边是
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
        InfoRow(title(count), systemImage: "ticket") {
            if count > 0 {
                RowAction(resetting ? "重置中…" : "重置额度…", systemImage: "arrow.counterclockwise") { model.resetQuota(a.ref) }
                    .disabled(busy || !a.can.usage)
                    .help("用掉 1 次重置，把 5 小时和本周的用量清零。会先问你；用量本来就是 0 时不扣次数。")
            }
        }
    }

    /// “还能重置额度 2 次 · 最早一次 11 月 24 日过期”（次数加重）
    private func title(_ count: Int) -> Text {
        var t = Text("还能重置额度 ") + Text("\(count) 次").monospacedDigit().fontWeight(.medium).foregroundStyle(.primary)
        if count > 0, let expiry = Texts.resetExpiry(credits, now: now) {
            t = t + Text(" · " + expiry)
        }
        return t
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

extension UsageEntry {
    /// 分模型的每周额度（model_scoped，例如 Claude 的 Fable；按名字去重，没有百分比的不要）：账号页各是一大块，侧栏各一行小条。
    /// note 说它和本周总额度的关系（`Texts.modelShare`，句子里带着名字）。
    var modelQuotas: [(name: String, window: UsageWindow, note: String)] {
        var seen = Set<String>()
        return modelScoped.compactMap { m in
            guard let name = nonEmpty(m.name), m.window.utilization != nil, seen.insert(name.lowercased()).inserted else { return nil }
            return (name, m.window, Texts.modelShare(m, weekly: sevenDay))
        }
    }
}

/// 侧栏、菜单栏面板、总览卡片右边的几行小用量：5 小时、本周，分模型的每周额度（Claude 的 Fable）也各一行，和账号页的大块一一对应。
/// 没有 5 小时限制的套餐（`noFiveHourLimit`：Claude 的 Ultra、ChatGPT 的 Pro 500）不画 5 小时那一行。
/// Antigravity 的几组额度：5 小时、本周各一行，写各组里用得最多的那个（鼠标指着说是哪一组）。
struct UsageStack: View {
    let usage: UsageEntry
    var width: CGFloat = 30

    var body: some View {
        if usage.hasQuota {
            QuotaStack(usage: usage, width: width)
        } else {
            windows
        }
    }

    private var windows: some View {
        VStack(alignment: .trailing, spacing: 3) {
            if !usage.noFiveHourLimit {
                MiniUsage(label: "5h", spoken: "5 小时用量", window: usage.fiveHour, stale: usage.stale, width: width)
            }
            MiniUsage(label: "周", spoken: "本周用量", window: usage.sevenDay, stale: usage.stale, width: width)
            // 标签栏 26：“Fable” 要 26.5，缩到 98% 看不出来；再宽一点，总览卡片最窄时「在用，已 12 分钟」就要折行
            ForEach(usage.modelQuotas, id: \.name) { m in
                MiniUsage(label: m.name, spoken: "\(m.name) 用量", window: m.window, stale: usage.stale, width: width,
                          labelWidth: 26, note: m.note)
            }
        }
    }
}

/// 一行小用量：“5h ▬▬ 13%”。每 30 秒重算一次（重置时间过了就显示“已重置”）。几行的标签靠右、条和百分比上下对齐。
struct MiniUsage: View {
    let label: String
    /// 读屏念的名字（“5 小时用量”）
    let spoken: String
    let window: UsageWindow?
    let stale: Bool
    var width: CGFloat = 30
    /// 标签那一栏多宽：“5h”“周”是 14；模型名（Fable）宽一些，放不下时先缩小（最多到 70%）、再截断（鼠标指着有全名）
    var labelWidth: CGFloat = 14
    /// 分模型的额度和本周总额度的关系（句子里带着名字）：鼠标指着时显示，读屏接在百分比后面念；过了重置时间就不说（那是旧的）
    var note: String?

    var body: some View {
        TimelineView(.periodic(from: .now, by: 30)) { context in
            let now = context.date.timeIntervalSince1970
            if let note, !(window?.isReset(now: now) ?? false) {
                cell(now: now, note: note).help(note)
            } else {
                cell(now: now, note: nil)
            }
        }
    }

    private func cell(now: Double, note: String?) -> some View {
        let reset = window?.isReset(now: now) ?? false
        let pct = window?.utilization
        let value: String = pct == nil ? "—" : reset ? "已重置" : Texts.percent(pct!)
        let style: AnyShapeStyle = {
            guard let p = pct, !reset, !stale else { return AnyShapeStyle(.plainSecondary) }
            return UsageLevel(percent: p) == .normal ? AnyShapeStyle(.primary) : AnyShapeStyle(Palette.usage(percent: p))
        }()
        return HStack(spacing: 5) {
            Text(label).font(.system(size: 10, weight: .medium)).foregroundStyle(.plainSecondary)
                .lineLimit(1).minimumScaleFactor(0.7)
                .frame(width: labelWidth, alignment: .trailing)
            Meter(fraction: reset ? 0 : (pct ?? 0) / 100, color: Palette.usage(percent: pct ?? 0), height: 4, dim: stale)
                .frame(width: width)
            Text(value)
                .font(.system(size: 11, weight: .semibold).monospacedDigit())
                .foregroundStyle(style)
                .frame(width: 34, alignment: .trailing)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(spoken)
        .accessibilityValue((value == "—" ? "暂无数据" : value) + (note.map { "，" + $0 } ?? ""))
    }
}

/// Antigravity 的额度（账号页）：一组一块（组名，agy 对这一组的说明），下面照 Claude 的样子画 5 小时、本周的大块；认不出窗口的用 agy 给的
/// 名字，停用的、只有剩几次的写成小行。最下面是 agy 对这些额度的说明。
struct QuotaGroupsView: View {
    let usage: UsageEntry
    let now: Double
    var dim = false

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            ForEach(Array(usage.quotaGroups.enumerated()), id: \.offset) { _, g in
                group(g)
            }
        }
    }

    private func group(_ g: QuotaGroup) -> some View {
        let tiles = Self.tiles(g)
        let solo = Self.solo(g)
        let minor = solo == nil ? Self.minor(g) : []
        return VStack(alignment: .leading, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 10) {
                    Text(g.displayName).font(.callout.weight(.semibold))
                    if let solo {
                        Text(solo).font(.callout).foregroundStyle(.secondary)
                    }
                }
            }
            if !tiles.isEmpty {
                QuotaGrid {
                    ForEach(Array(tiles.enumerated()), id: \.offset) { _, t in
                        QuotaTile(title: t.title, window: t.bucket.used, now: now, dim: dim)
                    }
                }
            }
            if !minor.isEmpty {
                LazyVGrid(columns: [GridItem(.flexible(), spacing: 28), GridItem(.flexible())], alignment: .leading, spacing: 10) {
                    ForEach(minor) { $0 }
                }
            }
        }
        .accessibilityElement(children: .contain)
    }

    /// 有百分比的额度：5 小时在前、本周其次（和 Claude 的一样叫），认不出窗口的照 agy 的顺序排在后面
    static func tiles(_ g: QuotaGroup) -> [(title: String, bucket: QuotaBucket)] {
        let shown = g.buckets.filter { !$0.disabled && ($0.used.utilization != nil || $0.used.reset) }
        func rank(_ b: QuotaBucket) -> Int { b.slot.map { $0 == .fiveHour ? 0 : 1 } ?? 2 }
        return shown.enumerated().sorted { (rank($0.element), $0.offset) < (rank($1.element), $1.offset) }.map { (title($0.element, in: g), $0.element) }
    }

    /// 认出窗口的叫 5 小时、本周（和 Claude 的一样）；认不出的不用 agy 给的英文名字，叫“其他额度”，一组里有几个时编上号
    static func title(_ b: QuotaBucket, in g: QuotaGroup) -> String {
        switch b.slot {
        case .fiveHour?: return "5 小时"
        case .sevenDay?: return "本周"
        case nil:
            let others = g.buckets.filter { $0.slot == nil }
            guard others.count > 1, let i = others.firstIndex(of: b) else { return "其他额度" }
            return "其他额度 \(i + 1)"
        }
    }

    /// 没有百分比的：停用了的写“不可用”，agy 给了剩几次的写次数
    static func minorValue(_ b: QuotaBucket) -> String? {
        if b.disabled { return "不可用" }
        guard b.used.utilization == nil, !b.used.reset, let n = b.remainingAmount else { return nil }
        return "还剩 \(n) 次"
    }

    static func minor(_ g: QuotaGroup) -> [MinorQuota] {
        g.buckets.compactMap { b in minorValue(b).map { MinorQuota(title: title(b, in: g), value: $0) } }
    }

    /// 一组只有一个额度、认不出窗口、没有百分比（Gemini Flash 这一组只有一个停用的）：不另起一行，
    /// “不可用”“还剩 N 次”直接写在组名后面
    static func solo(_ g: QuotaGroup) -> String? {
        guard g.buckets.count == 1, let b = g.buckets.first, b.slot == nil else { return nil }
        return minorValue(b)
    }
}

/// Antigravity 的小用量（侧栏、菜单栏面板、总览卡片）：5 小时、本周各一行，写各组里用得最多的那个；窗口都认不出时写用得最多的那个额度。
struct QuotaStack: View {
    let usage: UsageEntry
    var width: CGFloat = 30

    // 不另包 TimelineView（它会占满给的宽度，侧栏里百分比被挤出去）：挑哪个额度只在过了重置时间时会变，每行自己每 30 秒重算“已重置”
    var body: some View {
        VStack(alignment: .trailing, spacing: 3) {
            ForEach(Self.rows(usage, now: Date().timeIntervalSince1970), id: \.label) { r in
                MiniUsage(label: r.label, spoken: r.spoken, window: r.window, stale: usage.stale, width: width, note: r.note)
            }
        }
    }

    struct Row {
        var label: String
        var spoken: String
        var window: UsageWindow
        var note: String?
    }

    static func rows(_ u: UsageEntry, now: Double) -> [Row] {
        let named: [(QuotaSlot, String, String)] = [(.fiveHour, "5h", "5 小时"), (.sevenDay, "周", "本周")]
        let rows = named.compactMap { slot, label, spoken -> Row? in
            guard let m = u.quotaMost(slot, now: now) else { return nil }
            let several = Set(u.quotaBuckets(slot).map { $0.group ?? "" }).count > 1
            let note = several ? m.group.map { "用得最多的是 \($0)" } : nil
            return Row(label: label, spoken: "\(spoken)用量", window: m.window, note: note)
        }
        if !rows.isEmpty { return rows }
        // 窗口都认不出：写用得最多的那个额度（停用的、没有百分比的不算）
        let all = u.quotaGroups.flatMap { g in g.buckets.filter { !$0.disabled && $0.used.utilization != nil }.map { (g, $0) } }
        guard let (g, b) = all.max(by: { ($0.1.used.utilization ?? 0) < ($1.1.used.utilization ?? 0) }) else { return [] }
        return [Row(label: "额度", spoken: "额度用量", window: b.used, note: g.displayName)]
    }
}
