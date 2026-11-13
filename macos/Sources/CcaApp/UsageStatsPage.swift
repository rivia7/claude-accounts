import SwiftUI

/// 用量统计（侧栏的「用量统计」）：所有官方账号每周用了多少放在一起比，Claude、ChatGPT 各一组（和总览一样的顺序；第三方 API
/// 账号没有订阅的额度，不在这里）。每个账号一行：头像和名字、最近 12 周的小柱状图（所有行同一个时间范围，上下对得齐）、
/// 平均每周用到多少和几周用完 / 没用到一半、本周到现在。点一行进这个账号的页面。数字都是 cca 算好的（usage_weeks），App 只排版。
/// cca 有折合金额时标题右边可以切到「折合金额」（CostStatsContent）：同样的几行换成按官方 API 的价格折成多少美元。
struct UsageStatsPage: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ScrollView {
            TimelineView(.periodic(from: .now, by: 60)) { ctx in
                let now = ctx.date.timeIntervalSince1970
                let groups = model.officialGroups().filter { !$0.accounts.isEmpty }
                let accounts = groups.flatMap(\.accounts)
                let lists = accounts.map { $0.usageWeeks?.weeks ?? [] }
                let scale = StatsScale(domain: WeeklyChart.domain(lists, now: now), top: WeeklyChart.scaleTop(lists))
                let mode = model.costAvailable ? model.statsMode : .quota
                VStack(alignment: .leading, spacing: 26) {
                    header(accounts, mode: mode, now: now)
                    if mode == .cost {
                        CostStatsContent(now: now)
                    } else {
                        TipCard(id: "usage-stats-page", title: "这一页怎么看",
                                message: "每个账号一行，一根竖条是一周：填得越满用得越多，填满就是这一周的额度用完了；填成灰色的那一周结束前一天没查到，实际可能更多。"
                                    + "右边是平均每周用到多少、几周用完、几周没用到一半，和本周到现在。点一行看这个账号的详情。")
                        if model.state == nil {
                            HStack(spacing: 8) {
                                ProgressView().controlSize(.small)
                                Text("正在读取账号…").foregroundStyle(.secondary)
                            }
                        } else if accounts.isEmpty {
                            Text("还没有 Claude、ChatGPT 账号。第三方 API 账号没有订阅的额度，不在这里统计。")
                                .foregroundStyle(.secondary)
                        } else {
                            VStack(alignment: .leading, spacing: 8) {
                                StatsHeader(domain: scale.domain, now: now)
                                ForEach(groups) { g in
                                    StatsGroup(group: g, scale: scale, now: now)
                                }
                            }
                        }
                    }
                }
                .padding(.top, 24)
                .padding(.bottom, 36)
                .pageColumn()
            }
        }
        .navigationTitle("用量统计")
    }

    /// 标题和一句说明：数字从哪来、从什么时候开始记。cca 有折合金额时右边是看哪种（额度用量 / 折合金额），金额模式多一个「刷新」
    private func header(_ accounts: [Account], mode: StatsMode, now: Double) -> some View {
        let since = accounts.compactMap { $0.usageWeeks?.sinceEpoch }.min()
        return VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 10) {
                Text("用量统计").font(.system(size: 24, weight: .semibold))
                if model.stateLoading { ProgressView().controlSize(.small) }
                if model.costAvailable {
                    Spacer(minLength: 8)
                    if mode == .cost, model.costEnabled {
                        SectionAction(model.costRefreshing ? "刷新中…" : "刷新", systemImage: "arrow.clockwise") { model.refreshCost() }
                            .disabled(model.costRefreshing)
                            .help("现在统计一次，再和各个账号的其他设备同步")
                    }
                    modePicker
                }
            }
            Text(mode == .cost ? CostStatsContent.subtitle(model.state?.costTotal, scanning: model.costScanning, now: now)
                 : since.map { "最近 12 周，每周用到额度的百分之几。cca 每次查用量时记一笔，" + Texts.weeksSince($0, now: now) + "。" }
                    ?? "还没有记录：cca 每次查用量时会记下这一周用到多少，过完一周这里就有统计。")
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    /// 看哪种：额度用量（每周用到额度的百分之几）/ 折合金额（按官方 API 的价格折成多少美元）；记在 App 的设置里
    private var modePicker: some View {
        Picker("看哪种", selection: Binding(get: { model.statsMode }, set: { model.statsMode = $0 })) {
            Text("额度用量").tag(StatsMode.quota)
            Text("折合金额").tag(StatsMode.cost)
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .fixedSize()
        .help("额度用量：每周用到额度的百分之几；折合金额：按官方 API 的价格折成多少美元（不是实际花费）")
    }
}

/// 所有行的小图共用的坐标：同一段时间（上下对得齐）、同一个纵轴高度（各行的高低才能比）
private struct StatsScale {
    let domain: ClosedRange<Double>
    let top: Double
}

/// 每一行的几栏有多宽（表头和各组的行共用，上下对齐；小图占剩下的宽度）
private enum StatsColumns {
    static let name: CGFloat = 210
    static let average: CGFloat = 150
    static let current: CGFloat = 64
    static let spacing: CGFloat = 18
    /// 行的左右内边距
    static let inset: CGFloat = 14
}

/// 表头：账号、小图的时间刻度（每两周一个日期，最右边是今天）、平均每周、本周。
private struct StatsHeader: View {
    let domain: ClosedRange<Double>
    let now: Double

    var body: some View {
        HStack(spacing: StatsColumns.spacing) {
            Text("账号").frame(width: StatsColumns.name, alignment: .leading)
            StatsTimeScale(domain: domain, now: now)
                .frame(maxWidth: .infinity)
            Text("平均每周").frame(width: StatsColumns.average, alignment: .trailing)
            Text("本周").frame(width: StatsColumns.current, alignment: .trailing)
        }
        .font(.caption)
        .foregroundStyle(.secondary)
        .frame(height: 16)
        .padding(.horizontal, StatsColumns.inset)
        .accessibilityHidden(true)
    }
}

/// 表头里小图上面的时间刻度：每两周一个日期，最右边是今天（额度用量、折合金额两种表头共用，和各行的小图同一个时间范围）
struct StatsTimeScale: View {
    let domain: ClosedRange<Double>
    let now: Double

    var body: some View {
        GeometryReader { geo in
            let span = domain.upperBound - domain.lowerBound
            ForEach(0..<7) { k in
                let t = now - Double(k) * 2 * WeeklyChart.week
                let x = (t - domain.lowerBound) / span * geo.size.width
                if x >= 0 {
                    Text(k == 0 ? "今天" : Self.day(t))
                        .fixedSize()
                        .position(x: min(max(x, 18), geo.size.width - 18), y: geo.size.height / 2)
                }
            }
        }
    }

    private static func day(_ t: Double) -> String {
        let c = Calendar.current.dateComponents([.month, .day], from: Date(timeIntervalSince1970: t))
        return "\(c.month ?? 0)/\(c.day ?? 0)"
    }
}

/// 一组（Claude / ChatGPT）：组名和一块底色上的几行
private struct StatsGroup: View {
    let group: AccountGroup
    let scale: StatsScale
    let now: Double

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(Texts.department(group.department))
                .font(.title3.weight(.semibold))
                .accessibilityAddTraits(.isHeader)
                .padding(.top, 10)
            VStack(spacing: 0) {
                ForEach(Array(group.accounts.enumerated()), id: \.element.id) { i, a in
                    if i > 0 { Divider().padding(.horizontal, StatsColumns.inset) }
                    StatsRow(account: a, scale: scale, now: now)
                }
            }
            .padding(.vertical, 4)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
        }
    }
}

/// 一个账号一行：点了进账号页
private struct StatsRow: View {
    @Environment(AppModel.self) private var model
    let account: Account
    let scale: StatsScale
    let now: Double
    /// 鼠标在不在这一行上（命令行工具编不了 @State 宏：State 类型直接用）
    private let hovering = State(initialValue: false)

    var body: some View {
        let a = account
        let uw = a.usageWeeks
        let summary = uw.flatMap { Texts.weeksSummary($0.summary) }
        Button { model.show(a.ref) } label: {
            HStack(spacing: StatsColumns.spacing) {
                HStack(spacing: 10) {
                    AccountAvatar(account: a, size: 28, dot: 9)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(a.displayName).font(.body.weight(.medium)).lineLimit(1)
                        if let plan = nonEmpty(a.plan) {
                            PlanBadge(plan: plan, tier: a.planTier, size: .compact)
                        }
                    }
                }
                .frame(width: StatsColumns.name, alignment: .leading)
                chart(uw)
                    .frame(maxWidth: .infinity)
                    .frame(height: 34)
                average(uw?.summary)
                    .frame(width: StatsColumns.average, alignment: .trailing)
                Text(uw?.current.map { Texts.percent($0.usedPercent) } ?? "—")
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(uw?.current == nil ? .secondary : .primary)
                    .frame(width: StatsColumns.current, alignment: .trailing)
            }
            .padding(.horizontal, StatsColumns.inset)
            .padding(.vertical, 10)
            .contentShape(Rectangle())
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(hovering.wrappedValue ? Color.primary.opacity(0.05) : .clear)
                .padding(.horizontal, 4))
        }
        .buttonStyle(.plain)
        .onHover { hovering.wrappedValue = $0 }
        .help(summary ?? "点一下看这个账号的详情")
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(a.displayName)
        .accessibilityValue(summary ?? (uw == nil ? "还没有记录" : "最近 12 周没有记录"))
        .accessibilityHint("打开这个账号的页面")
    }

    @ViewBuilder
    private func chart(_ uw: UsageWeeks?) -> some View {
        if let uw, !uw.weeks.isEmpty {
            WeeklyChart(weeks: uw.weeks, domain: scale.domain, now: now, compact: true, top: scale.top)
        } else {
            Text(uw == nil ? "还没有记录" : "最近 12 周没有记录")
                .font(.caption)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    /// 「平均 63%」，下面一行小字：几周用完、几周没用到一半
    @ViewBuilder
    private func average(_ s: UsageWeeksSummary?) -> some View {
        if let s, let avg = Texts.weeksAverage(s) {
            VStack(alignment: .trailing, spacing: 2) {
                (Text("平均 ").foregroundStyle(.secondary) + Text(avg).monospacedDigit().fontWeight(.semibold))
                    .font(.callout)
                let notes = Texts.weeksNotes(s)
                if !notes.isEmpty {
                    Text(notes.joined(separator: " · "))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .minimumScaleFactor(0.85)
                }
            }
        } else {
            Text("—").font(.callout).foregroundStyle(.secondary)
        }
    }
}
