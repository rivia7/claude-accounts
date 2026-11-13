import SwiftUI

/// 用量统计页的「折合金额」（UsageStatsPage 标题右边切过来）：按官方 API 的价格，把每个账号用掉的 token 折成美元放在一起比。
/// 不是实际花费：订阅不按这个收费。顶上是所有账号加起来的（cca 给的 `cost_total`：本周、平均每周、近 12 周合计、近 30 天，
/// 启用了两种工具时分开列；最近 30 天每天一根柱子），下面每个账号一行，和额度用量一样按 Claude、ChatGPT 分组、按用户排的顺序：
/// 名字、最近 12 周每周的小柱子（所有行同一个时间范围、同一个纵轴，高低才能比）、平均每周、本周、12 周合计；有两台以上设备的账号
/// 下面缩进列出各台设备。点一行打开账号页的「折合金额」（点一台设备就看那台的）。数字都是 cca 给的，App 只排版。
struct CostStatsContent: View {
    @Environment(AppModel.self) private var model
    let now: Double
    /// 表有多宽（命令行工具编不了 @State 宏：State 类型直接用）：窗口窄、小图放不下时不画（CostStatsColumns.minChart）
    private let tableWidth = State<CGFloat?>(initialValue: nil)

    /// 标题下面那一句：数字是什么、多久以前统计的（scanning：第一次统计还没做完，AppModel.costScanning）
    static func subtitle(_ total: CostTotal?, scanning: Bool, now: Double) -> String {
        let what = "每个账号用掉的 token 按官方 API 的价格折成美元，不是实际花费。"
        guard let scan = total?.scan else { return what }
        if scanning { return what + "正在统计…" }
        return what + (Texts.costFreshness(scan, now: now).map { $0 + "。" } ?? "")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 26) {
            TipCard(id: "usage-stats-cost", title: "折合金额怎么看",
                    message: "按 Anthropic、OpenAI 官方 API 的价格，把每个账号用掉的 token 折成美元，看哪个账号用得多；订阅不按这个收费。"
                        + "每个账号一行，一根竖条是一周，颜色淡的那一周开始时还没有记录；有好几台设备的账号下面分台列出。"
                        + "右边是平均每周、本周到现在和近 12 周合计。点一行看这个账号每周、每天的柱状图。")
            if model.state == nil {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("正在读取账号…").foregroundStyle(.secondary)
                }
            } else if let total = model.state?.costTotal {
                let tools = model.visibleTools.filter { model.features($0).cost }
                VStack(alignment: .leading, spacing: 10) {
                    if model.costScanning {
                        HStack(spacing: 8) {
                            ProgressView().controlSize(.small)
                            Text("正在统计：第一次要把会话记录全读一遍，可能要一会儿。")
                        }
                        .font(.callout)
                        .foregroundStyle(.secondary)
                    }
                    CostTotalTile(total: total, tools: tools.filter { total.byTool[$0] != nil }, now: now)
                    if let f = model.costFailure {
                        Label(f, systemImage: "exclamationmark.triangle")
                            .font(.caption)
                            .foregroundStyle(.orange)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    if let u = Texts.costUnattributed(total.unattributed, currency: total.currency) {
                        Label(u, systemImage: "info.circle")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Text(Texts.costFootnote(tools, pricesDate: total.pricesDate))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                table
            } else {
                Text("没有可以折算的账号：还没有 Claude、ChatGPT 官方账号，或者在 config.json 里关掉了折合金额（\"cost\": false）。")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    @ViewBuilder
    private var table: some View {
        let groups = model.costGroups()
        if groups.isEmpty {
            Text("还没有 Claude、ChatGPT 账号。第三方 API 账号按实际用量付费，不在这里折算。")
                .foregroundStyle(.secondary)
        } else {
            let scale = CostStatsScale.make(groups.flatMap(\.accounts), now: now)
            let charts = tableWidth.wrappedValue.map { CostStatsColumns.fitsChart(tableWidth: $0) } ?? true
            VStack(alignment: .leading, spacing: 8) {
                CostStatsHeader(domain: scale.domain, now: now, charts: charts)
                ForEach(groups) { g in
                    CostStatsGroup(group: g, scale: scale, now: now, charts: charts)
                }
            }
            .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { tableWidth.wrappedValue = $0 }
        }
    }
}

/// 所有行的小图共用的坐标：同一段时间（上下对得齐）、同一个纵轴（各行的高低才能比）
private struct CostStatsScale {
    let domain: ClosedRange<Double>
    let top: Double

    static func make(_ accounts: [Account], now: Double) -> CostStatsScale {
        let costs = accounts.compactMap(\.cost)
        let bars = costs.map { CostChart.bars($0, granularity: .week, scope: .account) }
        return CostStatsScale(domain: CostChart.domain(costs.map(\.weeks), granularity: .week, now: now),
                              top: CostChart.niceTop(CostChart.maxUSD(bars)))
    }
}

/// 每一行的几栏有多宽（表头和各组的行共用，上下对齐；小图占剩下的宽度）。间距、左右内边距和额度用量的表一样
private enum CostStatsColumns {
    static let name: CGFloat = 176
    static let average: CGFloat = 76
    static let current: CGFloat = 68
    static let total: CGFloat = 84
    static let spacing: CGFloat = 18
    static let inset: CGFloat = 14
    /// 设备那几行的名字缩进到账号名字下面（头像 28 + 间距 10）
    static let deviceIndent: CGFloat = 38
    /// 小图至少要这么宽才画：窗口窄时（不到 1000 来宽）不画每一行的小图、表头不写日期，名字和数字照样对齐
    static let minChart: CGFloat = 120

    /// 这么宽的表放不放得下小图（除了小图，一行里还有名字、三栏数字、四个间距和左右内边距）
    static func fitsChart(tableWidth: CGFloat) -> Bool {
        tableWidth - (name + average + current + total + spacing * 4 + inset * 2) >= minChart
    }
}

/// 顶上的合计：四个数（启用了两种工具时下面按工具分开列），再下面是最近 30 天每天的柱子（cca 给的 `cost_total.days`，
/// 各账号加起来；各账号的周期不一样，所以没有按周加起来的柱子）。鼠标指着一天时下面那一行换成那一天的。
private struct CostTotalTile: View {
    let total: CostTotal
    /// 分开列的工具（`by_tool` 里有的、启用了的）；只有一种时不分开列
    let tools: [ToolID]
    let now: Double
    /// 鼠标指着的那一天（命令行工具编不了 @State 宏：State 类型直接用）
    private let hover = State<Double?>(initialValue: nil)

    private static let titles = ["本周", "平均每周", "近 12 周合计", "近 30 天"]

    private static func value(_ s: CostSummary, _ i: Int) -> Double? {
        switch i {
        case 0: return s.currentUSD
        case 1: return s.averageUSD
        case 2: return s.weeksUSD
        default: return s.daysUSD
        }
    }

    var body: some View {
        let bars = total.days.map { CostBar(period: $0, usd: $0.usd) }
        let hovered = bars.first { $0.id == hover.wrappedValue }
        VStack(alignment: .leading, spacing: 16) {
            figures
            if bars.contains(where: { $0.usd > 0 }) {
                VStack(alignment: .leading, spacing: 10) {
                    CostBarChart(bars: bars, granularity: .day,
                                 domain: CostChart.domain([total.days], granularity: .day, now: now),
                                 top: CostChart.niceTop(CostChart.maxUSD([bars])), currency: total.currency,
                                 now: now, hover: hover.projectedValue)
                        .frame(height: 104)
                    Text(hovered.map { Texts.costBarDetail($0, granularity: .day, scope: .account, currency: total.currency, now: now) }
                         ?? Texts.costCaption(total.summary, granularity: .day, currency: total.currency))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                }
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("所有账号的折合金额")
    }

    private func money(_ v: Double?) -> String {
        v.map { Texts.money($0, currency: total.currency) } ?? "—"
    }

    /// 四个数：上面一行是说明，下面是金额；分开列时左边多一栏写合计、Claude、ChatGPT
    private var figures: some View {
        let split = tools.count >= 2
        return Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 28, verticalSpacing: 6) {
            // 读屏：每个金额自己说是哪一项（标签）和多少（值），表头、行头就不再单独读一遍
            GridRow {
                if split { Color.clear.gridCellUnsizedAxes([.horizontal, .vertical]) }
                ForEach(Self.titles.indices, id: \.self) { i in
                    Text(Self.titles[i]).font(.caption).foregroundStyle(.secondary)
                        .accessibilityHidden(true)
                }
            }
            GridRow {
                if split { Text("合计").font(.callout).foregroundStyle(.secondary).accessibilityHidden(true) }
                ForEach(Self.titles.indices, id: \.self) { i in
                    let v = money(Self.value(total.summary, i))
                    Text(v)
                        .font(.title2.weight(.semibold).monospacedDigit())
                        .accessibilityLabel(split ? "合计 " + Self.titles[i] : Self.titles[i])
                        .accessibilityValue(v)
                }
            }
            if split {
                ForEach(tools, id: \.self) { t in
                    GridRow {
                        Text(Texts.product(t)).font(.callout).foregroundStyle(.secondary).accessibilityHidden(true)
                        ForEach(Self.titles.indices, id: \.self) { i in
                            let v = money(total.byTool[t].flatMap { Self.value($0, i) })
                            Text(v)
                                .font(.callout.monospacedDigit())
                                .foregroundStyle(.secondary)
                                .accessibilityLabel(Texts.product(t) + " " + Self.titles[i])
                                .accessibilityValue(v)
                        }
                    }
                }
            }
        }
    }
}

/// 表头：账号、小图的时间刻度（和额度用量的表头一样；不画小图时空着）、平均每周、本周、12 周合计。
private struct CostStatsHeader: View {
    let domain: ClosedRange<Double>
    let now: Double
    var charts = true

    var body: some View {
        HStack(spacing: CostStatsColumns.spacing) {
            Text("账号").frame(width: CostStatsColumns.name, alignment: .leading)
            if charts {
                StatsTimeScale(domain: domain, now: now)
                    .frame(maxWidth: .infinity)
            } else {
                Spacer(minLength: 0)
            }
            Text("平均每周").frame(width: CostStatsColumns.average, alignment: .trailing)
            Text("本周").frame(width: CostStatsColumns.current, alignment: .trailing)
            Text("12 周合计").frame(width: CostStatsColumns.total, alignment: .trailing)
        }
        .font(.caption)
        .foregroundStyle(.secondary)
        .frame(height: 16)
        .padding(.horizontal, CostStatsColumns.inset)
        .accessibilityHidden(true)
    }
}

/// 一组（Claude / ChatGPT）：组名和一块底色上的几行；有两台以上设备的账号，下面紧跟着各台设备
private struct CostStatsGroup: View {
    let group: AccountGroup
    let scale: CostStatsScale
    let now: Double
    var charts = true

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(Texts.department(group.department))
                .font(.title3.weight(.semibold))
                .accessibilityAddTraits(.isHeader)
                .padding(.top, 10)
            VStack(spacing: 0) {
                ForEach(Array(group.accounts.enumerated()), id: \.element.id) { i, a in
                    if i > 0 { Divider().padding(.horizontal, CostStatsColumns.inset) }
                    CostStatsRow(account: a, scale: scale, now: now, charts: charts)
                    if let c = a.cost, CostChart.listsDevices(c) {
                        VStack(spacing: 0) {
                            ForEach(c.devices) { d in
                                CostDeviceRow(account: a, device: d, scale: scale, now: now, charts: charts)
                            }
                        }
                        .padding(.bottom, 6)
                    }
                }
            }
            .padding(.vertical, 4)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
        }
    }
}

/// 一行里的小图：最近 12 周每周一根（所有行同一个时间范围、同一个纵轴）；没有记录、都是 0 时写一句
private struct CostStatsChart: View {
    let cost: AccountCost?
    let scope: CostScope
    let scale: CostStatsScale
    let now: Double

    var body: some View {
        Group {
            if let c = cost {
                let bars = CostChart.bars(c, granularity: .week, scope: scope)
                if bars.contains(where: { $0.usd > 0 }) {
                    CostBarChart(bars: bars, granularity: .week, domain: scale.domain, top: scale.top, currency: c.currency,
                                 scope: scope, compact: true, now: now)
                } else {
                    note("最近 12 周没有用量")
                }
            } else {
                note("还没有记录")
            }
        }
        .frame(maxWidth: .infinity)
        .frame(height: 34)
    }

    private func note(_ text: String) -> some View {
        Text(text)
            .font(.caption)
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// 右边三栏：平均每周（和额度用量的表一样加粗）、本周、12 周合计。设备那几行用灰字
private struct CostStatsFigures: View {
    let summary: CostSummary?
    let currency: String
    var device = false

    var body: some View {
        let s = summary
        HStack(spacing: CostStatsColumns.spacing) {
            Text(s?.averageUSD.map { Texts.money($0, currency: currency) } ?? "—")
                .fontWeight(device || s?.averageUSD == nil ? .regular : .semibold)
                .frame(width: CostStatsColumns.average, alignment: .trailing)
            Text(s.map { Texts.money($0.currentUSD, currency: currency) } ?? "—")
                .frame(width: CostStatsColumns.current, alignment: .trailing)
            Text(s.map { Texts.money($0.weeksUSD, currency: currency) } ?? "—")
                .frame(width: CostStatsColumns.total, alignment: .trailing)
        }
        .font(.callout.monospacedDigit())
        .foregroundStyle(device || s == nil ? .secondary : .primary)
        .lineLimit(1)
    }
}

/// 一个账号一行：点了进账号页的「折合金额」。数字没算全（同步出错、有设备没算进来、很久没同步上来）时名字后面一个橙色的三角，
/// 原因在悬停提示和读屏里
private struct CostStatsRow: View {
    @Environment(AppModel.self) private var model
    let account: Account
    let scale: CostStatsScale
    let now: Double
    var charts = true
    /// 鼠标在不在这一行上（命令行工具编不了 @State 宏：State 类型直接用）
    private let hovering = State(initialValue: false)

    var body: some View {
        let a = account
        let c = a.cost
        let caption = c.map { Texts.costCaption($0.summary, granularity: .week, currency: $0.currency) } ?? "还没有记录"
        let notes = c.map { Texts.costNotes($0, now: now) } ?? []
        let warn = notes.contains(where: \.warning)
        let lines = [caption] + notes.map(\.text)
        Button { model.showCost(a.ref) } label: {
            HStack(spacing: CostStatsColumns.spacing) {
                HStack(spacing: 10) {
                    AccountAvatar(account: a, size: 28, dot: 9)
                    VStack(alignment: .leading, spacing: 3) {
                        HStack(spacing: 5) {
                            Text(a.displayName).font(.body.weight(.medium)).lineLimit(1)
                            if warn {
                                Image(systemName: "exclamationmark.triangle.fill")
                                    .font(.caption)
                                    .foregroundStyle(.orange)
                            }
                        }
                        if let plan = nonEmpty(a.plan) {
                            PlanBadge(plan: plan, tier: a.planTier, size: .compact)
                        }
                    }
                }
                .frame(width: CostStatsColumns.name, alignment: .leading)
                if charts { CostStatsChart(cost: c, scope: .account, scale: scale, now: now) } else { Spacer(minLength: 0) }
                CostStatsFigures(summary: c?.summary, currency: c?.currency ?? "USD")
            }
            .padding(.horizontal, CostStatsColumns.inset)
            .padding(.vertical, 10)
            .contentShape(Rectangle())
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(hovering.wrappedValue ? Color.primary.opacity(0.05) : .clear)
                .padding(.horizontal, 4))
        }
        .buttonStyle(.plain)
        .onHover { hovering.wrappedValue = $0 }
        .help(lines.joined(separator: "\n"))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(a.displayName)
        .accessibilityValue(lines.joined(separator: "；"))
        .accessibilityHint("打开这个账号的折合金额")
    }
}

/// 账号下面的一台设备（cca 排好的：这台设备在前，别的按 30 天的金额从多到少）：缩进、灰字，小图和数字是这台设备的。
/// 很久没同步上来的名字后面一个橙色的钟。点了进账号页，看这台设备的
private struct CostDeviceRow: View {
    @Environment(AppModel.self) private var model
    let account: Account
    let device: CostDevice
    let scale: CostStatsScale
    let now: Double
    var charts = true
    private let hovering = State(initialValue: false)

    var body: some View {
        let d = device
        let c = account.cost
        let currency = c?.currency ?? "USD"
        let stale = Texts.costStale(d, now: now)
        let lines = [Texts.costCaption(d.summary, granularity: .week, currency: currency)] + (stale.map { [$0] } ?? [])
        Button { model.showCost(account.ref, device: d.id) } label: {
            HStack(spacing: CostStatsColumns.spacing) {
                HStack(spacing: 5) {
                    Text(Texts.costDeviceName(d)).lineLimit(1).truncationMode(.middle)
                    if stale != nil {
                        Image(systemName: "clock.badge.exclamationmark").foregroundStyle(.orange)
                    }
                }
                .font(.callout)
                .foregroundStyle(.secondary)
                .padding(.leading, CostStatsColumns.deviceIndent)
                .frame(width: CostStatsColumns.name, alignment: .leading)
                if charts { CostStatsChart(cost: c, scope: .device(d.id), scale: scale, now: now) } else { Spacer(minLength: 0) }
                CostStatsFigures(summary: d.summary, currency: currency, device: true)
            }
            .padding(.horizontal, CostStatsColumns.inset)
            .padding(.vertical, 5)
            .contentShape(Rectangle())
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(hovering.wrappedValue ? Color.primary.opacity(0.05) : .clear)
                .padding(.horizontal, 4))
        }
        .buttonStyle(.plain)
        .onHover { hovering.wrappedValue = $0 }
        .help(lines.joined(separator: "\n"))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Texts.costDeviceName(d))
        .accessibilityValue(lines.joined(separator: "；"))
        .accessibilityHint("打开这个账号的折合金额，看这台设备的")
    }
}
