import SwiftUI

/// 折合金额（侧栏的「折合金额」）：按官方 API 的价格，把每个账号用掉的 token 折成美元放在一起比。不是实际花费：订阅不按这个收费。
/// 和用量统计分开两页：钱没有额度、没有“填满”，图不一样（不画底槽，按纵轴的刻度比高低）。
/// 标题右边是看哪台设备、按周 / 按日（和账号页的「折合金额」共用一个设置）和「刷新」：一行放不下时（窗口窄）这三个挪到标题下面一行，右对齐。
struct CostStatsPage: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ScrollView {
            TimelineView(.periodic(from: .now, by: 60)) { ctx in
                let now = ctx.date.timeIntervalSince1970
                VStack(alignment: .leading, spacing: 26) {
                    header(now: now)
                    CostStatsContent(now: now)
                }
                .padding(.top, 24)
                .padding(.bottom, 36)
                .pageColumn()
            }
        }
        .navigationTitle("折合金额")
    }

    /// 标题和一句说明（数字是什么、多久以前统计的）；有能统计的账号时右边是三个控件
    private func header(now: Double) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            if model.costEnabled {
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 10) {
                        StatsPageTitle(text: "折合金额")
                        Spacer(minLength: 8)
                        controls
                    }
                    VStack(alignment: .trailing, spacing: 10) {
                        StatsPageTitle(text: "折合金额")
                            .frame(maxWidth: .infinity, alignment: .leading)
                        HStack(spacing: 10) { controls }
                    }
                }
            } else {
                StatsPageTitle(text: "折合金额")
            }
            Text(CostStatsContent.subtitle(model.state?.costTotal, scanning: model.costScanning, now: now))
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    /// 和账号页「折合金额」的标题上一样的两个切换（两页共用一个设置）、一样的顺序，和「刷新」
    @ViewBuilder
    private var controls: some View {
        CostScopePicker(page: .stats)
        CostGranularityPicker()
        SectionAction(model.costRefreshing ? "刷新中…" : "刷新", systemImage: "arrow.clockwise") { model.refreshCost() }
            .disabled(model.costRefreshing)
            .help("立即统计，并与各账号的其他设备同步")
    }
}

/// 折合金额页的内容：顶上是所有账号加起来的（cca 给的 `cost_total`，本机时是它的 `me`，另一台设备时是 `devices` 里那一台的：
/// 按周是本周、平均每周、近 12 周合计；按日是今天、平均每天、近 30 天合计和最近 30 天每天一根柱子；启用了两种以上的工具时分开列），
/// 下面每个账号一行，按工具分组（Claude、ChatGPT、Antigravity，和总览一样）、按用户排的顺序：名字、小柱子（最近 12 周每周一根或者最近 30 天每天一根，
/// 所有行同一个时间范围、同一个纵轴，高低才能比）、平均、本周（今天）、合计；看所有设备时，有两台以上设备的账号下面缩进列出各台设备，
/// 看另一台设备时只列它用过的账号（cca 给的 `used`）。点一行打开账号页的「折合金额」。数字都是 cca 给的，App 只排版。
struct CostStatsContent: View {
    @Environment(AppModel.self) private var model
    let now: Double
    /// 表有多宽（命令行工具编不了 @State 宏：State 类型直接用）：窗口窄、小图放不下时不画（CostStatsColumns.minChart）
    private let tableWidth = State<CGFloat?>(initialValue: nil)

    /// 看哪台设备（选的那台 cca 这次没列出来时是所有设备）
    private var scope: CostScope { model.costScopeShown }

    /// 标题下面那一句：数字是什么、多久以前统计的（scanning：第一次统计还没做完，AppModel.costScanning）
    static func subtitle(_ total: CostTotal?, scanning: Bool, now: Double) -> String {
        let what = "按官方 API 价格把每个账号用掉的 token 折算成美元，不是实际花费。"
        guard let scan = total?.scan else { return what }
        if scanning { return what + "正在统计…" }
        return what + (Texts.costFreshness(scan, now: now).map { $0 + "。" } ?? "")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 26) {
            TipCard(id: .costStats, title: "这一页怎么看",
                    message: "按 Anthropic、OpenAI、Google 官方 API 的价格，把每个账号用掉的 token 折算成美元，方便比较哪个账号用得多；订阅并不按此收费。"
                        + "右上角可以选择设备（所有设备、本机或其他设备），以及按周或按天查看。"
                        + "每个账号一行，一根竖条代表一周（或一天），颜色较浅的表示那一项开始时还没有记录；查看所有设备时，用过多台设备的账号会分设备列出。"
                        + "无法识别账号的用量不计入合计，单独列在最下面。"
                        + "点击一行查看这个账号的柱状图。")
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
                            Text("正在统计：首次需要读取全部会话记录，可能需要一些时间。")
                        }
                        .font(.callout)
                        .foregroundStyle(.secondary)
                    }
                    CostTotalTile(total: total, tools: tools.filter { total.all.byTool[$0] != nil }, scope: scope,
                                  granularity: model.costGranularity, now: now)
                    if let f = model.costFailure {
                        Label(f, systemImage: "exclamationmark.triangle")
                            .font(.caption)
                            .foregroundStyle(.orange)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    // 认不出是哪个账号的（看哪一方说哪一方的；旧版 cca 只有这台设备的，看另一台设备时不说）、还有哪几台没传它
                    if let u = Texts.costUnattributed(total.unattributed, scope: scope, total: total, currency: total.currency) {
                        Label(u, systemImage: "info.circle")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    if let u = Texts.costUnsynced(total.unattributed, scope: scope) {
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
                Text("没有可折算的账号：还没有 Claude、ChatGPT、Antigravity 官方账号，或已在 config.json 中关闭了折合金额（\"cost\": false）。")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    /// 各组的账号，最下面是「认不出账号」（cca 给了、看的那一方有这种用量时）；看的那台设备一个账号都没用过时先说一句
    @ViewBuilder
    private var table: some View {
        let total = model.state?.costTotal
        let groups = model.costGroups()
        let orphan = CostChart.orphan(total, scope: scope)
        if groups.isEmpty && orphan == nil {
            Text(Texts.costNoAccounts(scope, total: total))
                .foregroundStyle(.secondary)
        } else {
            let scale = CostStatsScale.make(groups.flatMap(\.accounts), extra: orphan.map { [$0] } ?? [], scope: scope,
                                            granularity: model.costGranularity, now: now)
            let charts = tableWidth.wrappedValue.map { CostStatsColumns.fitsChart(tableWidth: $0) } ?? true
            VStack(alignment: .leading, spacing: 8) {
                if groups.isEmpty {
                    Text(Texts.costNoAccounts(scope, total: total))
                        .foregroundStyle(.secondary)
                }
                CostStatsHeader(scale: scale, now: now, charts: charts)
                ForEach(groups) { g in
                    CostStatsGroup(group: g, scale: scale, now: now, charts: charts)
                }
                if let orphan, let total {
                    CostOrphanGroup(cost: orphan, total: total, scale: scale, now: now, charts: charts)
                }
            }
            .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { tableWidth.wrappedValue = $0 }
        }
    }
}

/// 所有行共用的：看谁的、按周还是按天，小图的坐标（同一段时间，上下对得齐；同一个纵轴，各行的高低才能比）
private struct CostStatsScale {
    let scope: CostScope
    let granularity: CostGranularity
    let domain: ClosedRange<Double>
    let top: Double

    /// extra：不是哪个账号的那几份（「认不出账号」），和账号那几行一起定纵轴、横轴
    static func make(_ accounts: [Account], extra: [AccountCost] = [], scope: CostScope, granularity g: CostGranularity,
                     now: Double) -> CostStatsScale {
        let costs = accounts.compactMap(\.cost) + extra
        // 各台设备那几行的柱子不比整个账号的高，纵轴只看账号那几行
        let bars = costs.map { CostChart.bars($0, granularity: g, scope: scope) }
        return CostStatsScale(scope: scope, granularity: g, domain: CostChart.domain(costs.map { CostChart.periods($0, g) }, granularity: g, now: now),
                              top: CostChart.niceTop(CostChart.maxUSD(bars)))
    }

    /// 右边三栏的表头：平均、当前那一项、合计
    var titles: [String] {
        granularity == .week ? ["平均每周", "本周", "12 周合计"] : ["平均每天", "今天", "30 天合计"]
    }

    /// 这三栏的数（cca 给的 summary）
    func figures(_ s: CostSummary) -> [Double?] {
        granularity == .week ? [s.averageUSD, s.currentUSD, s.weeksUSD] : [s.averageDayUSD, s.todayUSD, s.daysUSD]
    }

    /// 小图里一根也没有时说的
    var empty: String { granularity == .week ? "最近 12 周没有用量" : "最近 30 天没有用量" }
}

/// 每一行的几栏有多宽（表头和各组的行共用，上下对齐；小图占剩下的宽度）。间距、左右内边距和用量统计的表一样
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

    /// 右边三栏的宽：平均、当前那一项、合计
    static let figures = [average, current, total]

    /// 这么宽的表放不放得下小图（除了小图，一行里还有名字、三栏数字、四个间距和左右内边距）
    static func fitsChart(tableWidth: CGFloat) -> Bool {
        tableWidth - (name + average + current + total + spacing * 4 + inset * 2) >= minChart
    }
}

/// 顶上的合计：三个数（按周：本周、平均每周、近 12 周合计；按日：今天、平均每天、近 30 天合计；启用了两种工具时下面按工具分开列），
/// cca 给了13.7.6 起，合计的不是 null）时多一栏（按周是近 12 周的、按日是近 30 天的；按工具的那几行没有的写「—」），
/// 按日时再下面是最近 30 天每天的柱子（cca 给的 `days`，各账号加起来；各账号的周期不一样，所以按周时没有加起来的柱子）。
/// 鼠标指着一天时下面那一行换成那一天的。看本机时都是 cca 给的 `me` 里的（旧版 cca 没有，写「—」），看另一台设备时是 `devices` 里那一台的
private struct CostTotalTile: View {
    let total: CostTotal
    /// 分开列的工具（`by_tool` 里有的、启用了的）；只有一种时不分开列
    let tools: [ToolID]
    let scope: CostScope
    let granularity: CostGranularity
    let now: Double
    /// 鼠标指着的那一天（命令行工具编不了 @State 宏：State 类型直接用）
    private let hover = State<Double?>(initialValue: nil)

    private var titles: [String] {
        granularity == .week ? ["本周", "平均每周", "近 12 周合计"] : ["今天", "平均每天", "近 30 天合计"]
    }

    /// 第四栏（缓存命中率）的说明
    private var hitTitle: String {
        granularity == .week ? "近 12 周缓存命中" : "近 30 天缓存命中"
    }

    private func hit(_ s: CostSummary?) -> String? {
        s.flatMap { Texts.costCacheHit($0.cacheHit(granularity)) }
    }

    private func values(_ s: CostSummary?) -> [Double?] {
        guard let s else { return [nil, nil, nil] }
        return granularity == .week ? [s.currentUSD, s.averageUSD, s.weeksUSD] : [s.todayUSD, s.averageDayUSD, s.daysUSD]
    }

    var body: some View {
        let part = total.part(scope)
        let bars = granularity == .day ? (part?.days ?? []).map { CostBar(period: $0, usd: $0.usd) } : []
        let hovered = bars.first { $0.id == hover.wrappedValue }
        VStack(alignment: .leading, spacing: 16) {
            figures(part)
            if let part, bars.contains(where: { $0.usd > 0 }) {
                VStack(alignment: .leading, spacing: 10) {
                    CostBarChart(bars: bars, granularity: .day,
                                 domain: CostChart.domain([part.days], granularity: .day, now: now),
                                 top: CostChart.niceTop(CostChart.maxUSD([bars])), currency: total.currency,
                                 requests: false, now: now, hover: hover.projectedValue)
                        .frame(height: 104)
                    Text(hovered.map { Texts.costBarDetail($0, granularity: .day, scope: scope, currency: total.currency, now: now) }
                         ?? Texts.costCaption(part.summary, granularity: .day, currency: total.currency))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                }
            }
            if let rows = part?.models {
                CostModelList(rows: CostChart.modelRows(rows, granularity: granularity), granularity: granularity, currency: total.currency)
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Texts.costTotalLabel(scope, total: total))
    }

    private func money(_ v: Double?) -> String {
        v.map { Texts.money($0, currency: total.currency) } ?? "—"
    }

    /// 三个数（和缓存命中率）：上面一行是说明，下面是金额；分开列时左边多一栏写合计和各个工具（Claude、ChatGPT、Antigravity）
    private func figures(_ part: CostTotalPart?) -> some View {
        let split = tools.count >= 2
        let titles = titles
        let hits = hit(part?.summary) != nil
        return Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 28, verticalSpacing: 6) {
            // 读屏：每个金额自己说是哪一项（标签）和多少（值），表头、行头就不再单独读一遍
            GridRow {
                if split { Color.clear.gridCellUnsizedAxes([.horizontal, .vertical]) }
                ForEach(titles.indices, id: \.self) { i in
                    Text(titles[i]).font(.caption).foregroundStyle(.secondary)
                        .accessibilityHidden(true)
                }
                if hits {
                    Text(hitTitle).font(.caption).foregroundStyle(.secondary)
                        .accessibilityHidden(true)
                }
            }
            GridRow {
                if split { Text("合计").font(.callout).foregroundStyle(.secondary).accessibilityHidden(true) }
                let vs = values(part?.summary)
                ForEach(titles.indices, id: \.self) { i in
                    let v = money(vs[i])
                    Text(v)
                        .font(.title2.weight(.semibold).monospacedDigit())
                        .accessibilityLabel(split ? "合计 " + titles[i] : titles[i])
                        .accessibilityValue(v)
                }
                if hits {
                    let v = hit(part?.summary) ?? "—"
                    Text(v)
                        .font(.title2.weight(.semibold).monospacedDigit())
                        .accessibilityLabel(split ? "合计 " + hitTitle : hitTitle)
                        .accessibilityValue(v)
                }
            }
            if split {
                ForEach(tools, id: \.self) { t in
                    GridRow {
                        Text(Texts.product(t)).font(.callout).foregroundStyle(.secondary).accessibilityHidden(true)
                        let vs = values(part?.byTool[t])
                        ForEach(titles.indices, id: \.self) { i in
                            let v = money(vs[i])
                            Text(v)
                                .font(.callout.monospacedDigit())
                                .foregroundStyle(.secondary)
                                .accessibilityLabel(Texts.product(t) + " " + titles[i])
                                .accessibilityValue(v)
                        }
                        if hits {
                            let v = hit(part?.byTool[t]) ?? "—"
                            Text(v)
                                .font(.callout.monospacedDigit())
                                .foregroundStyle(.secondary)
                                .accessibilityLabel(Texts.product(t) + " " + hitTitle)
                                .accessibilityValue(v)
                        }
                    }
                }
            }
        }
    }
}

/// 表头：账号、小图的时间刻度（和用量统计的表头一样；按日时每隔几天一个日期；不画小图时空着）、平均、本周（今天）、合计。
private struct CostStatsHeader: View {
    let scale: CostStatsScale
    let now: Double
    var charts = true

    var body: some View {
        HStack(spacing: CostStatsColumns.spacing) {
            Text("账号").frame(width: CostStatsColumns.name, alignment: .leading)
            if charts {
                StatsTimeScale(domain: scale.domain, now: now, granularity: scale.granularity)
                    .frame(maxWidth: .infinity)
            } else {
                Spacer(minLength: 0)
            }
            ForEach(Array(zip(scale.titles, CostStatsColumns.figures)), id: \.0) { title, width in
                Text(title).frame(width: width, alignment: .trailing)
            }
        }
        .font(.caption)
        .foregroundStyle(.secondary)
        .frame(height: 16)
        .padding(.horizontal, CostStatsColumns.inset)
        .accessibilityHidden(true)
    }
}

/// 一组（Claude / ChatGPT）：组名和一块底色上的几行（看另一台设备时只有它用过的账号，AppModel.costGroups）；
/// 看所有设备时，有两台以上设备的账号下面紧跟着各台设备
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
                    CostAccountRow(account: a, scale: scale, now: now, charts: charts)
                    if scale.scope == .all, let c = a.cost, CostChart.listsDevices(c) {
                        VStack(spacing: 0) {
                            ForEach(CostChart.listedDevices(c)) { d in
                                CostDeviceRow(account: a, cost: c, device: d, scale: scale, now: now, charts: charts)
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

/// 一行里的小图：最近 12 周每周一根或者最近 30 天每天一根（所有行同一个时间范围、同一个纵轴）；没有记录、都是 0 时写一句
private struct CostStatsChart: View {
    /// 这一行的柱子；nil：这个账号还没有记录
    let bars: [CostBar]?
    let currency: String
    /// 读屏里说不说请求数（只有看所有设备时账号那一行有）
    var requests = false
    let scale: CostStatsScale
    let now: Double

    var body: some View {
        Group {
            if let bars {
                if bars.contains(where: { $0.usd > 0 }) {
                    CostBarChart(bars: bars, granularity: scale.granularity, domain: scale.domain, top: scale.top, currency: currency,
                                 requests: requests, compact: true, now: now)
                } else {
                    note(scale.empty)
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

/// 右边三栏：平均（和用量统计的表一样加粗）、本周（今天）、合计。设备那几行用灰字
private struct CostStatsFigures: View {
    let summary: CostSummary?
    let currency: String
    let scale: CostStatsScale
    var device = false

    var body: some View {
        let values = summary.map { scale.figures($0) } ?? [nil, nil, nil]
        HStack(spacing: CostStatsColumns.spacing) {
            ForEach(values.indices, id: \.self) { i in
                Text(values[i].map { Texts.money($0, currency: currency) } ?? "—")
                    .fontWeight(i == 0 && !device && values[i] != nil ? .semibold : .regular)
                    .frame(width: CostStatsColumns.figures[i], alignment: .trailing)
            }
        }
        .font(.callout.monospacedDigit())
        .foregroundStyle(device || summary == nil ? .secondary : .primary)
        .lineLimit(1)
    }
}

/// 一个账号一行：点了进账号页的「折合金额」。看所有设备、数字没算全（同步出错、有设备没算进来、很久没同步上来）时名字后面一个橙色的三角，
/// 原因在悬停提示和读屏里。看本机时是这台设备的数字（它没有这个账号的记录时是 0），看另一台设备时是那一台的
/// （很久没同步上来时也是一个三角）
private struct CostAccountRow: View {
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
        let summary = c.map { CostChart.summary($0, scope: scale.scope) }
        let caption = c.map { Texts.costCaption(CostChart.summary($0, scope: scale.scope), granularity: scale.granularity, currency: $0.currency) }
            ?? "还没有记录"
        let notes = c.map { Texts.costNotes($0, scope: scale.scope, now: now) } ?? []
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
                if charts {
                    CostStatsChart(bars: c.map { CostChart.bars($0, granularity: scale.granularity, scope: scale.scope) },
                                   currency: c?.currency ?? "USD", requests: scale.scope == .all, scale: scale, now: now)
                } else {
                    Spacer(minLength: 0)
                }
                CostStatsFigures(summary: summary, currency: c?.currency ?? "USD", scale: scale)
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

/// 账号下面的一台设备（看所有设备时；cca 排好的：这台设备在前，别的按 30 天的金额从多到少）：缩进、灰字，小图和数字是这台设备的。
/// 很久没同步上来的名字后面一个橙色的钟（cca 给的诊断 diag 不显示）。点了和点账号那一行一样，进账号页。
/// 「认不出账号」那一组的（account 是 nil）：不是哪个账号的，点了不跳转
private struct CostDeviceRow: View {
    @Environment(AppModel.self) private var model
    let account: Account?
    let cost: AccountCost
    let device: CostDevice
    let scale: CostStatsScale
    let now: Double
    var charts = true
    private let hovering = State(initialValue: false)

    var body: some View {
        let d = device
        let stale = Texts.costStale(d, now: now)
        let lines = [Texts.costCaption(d.summary, granularity: scale.granularity, currency: cost.currency)] + [stale].compactMap { $0 }
        let row = HStack(spacing: CostStatsColumns.spacing) {
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
            if charts {
                CostStatsChart(bars: CostChart.bars(cost, granularity: scale.granularity, device: d), currency: cost.currency,
                               scale: scale, now: now)
            } else {
                Spacer(minLength: 0)
            }
            CostStatsFigures(summary: d.summary, currency: cost.currency, scale: scale, device: true)
        }
        .padding(.horizontal, CostStatsColumns.inset)
        .padding(.vertical, 5)
        Group {
            if let account {
                Button { model.showCost(account.ref) } label: {
                    row
                        .contentShape(Rectangle())
                        .background(RoundedRectangle(cornerRadius: 10, style: .continuous)
                            .fill(hovering.wrappedValue ? Color.primary.opacity(0.05) : .clear)
                            .padding(.horizontal, 4))
                }
                .buttonStyle(.plain)
                .onHover { hovering.wrappedValue = $0 }
                .accessibilityHint("打开这个账号的折合金额")
            } else {
                row
            }
        }
        .help(lines.joined(separator: "\n"))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Texts.costDeviceName(d))
        .accessibilityValue(lines.joined(separator: "；"))
    }
}

/// 「认不出账号」（cca 给的 `unattributed.cost`，3.6.8 起）：会话记录里认不出是哪个账号的用量，
/// 没算进上面的合计，单独一组放在最下面：一行是看的那一方的（所有设备 / 这台设备 / 那台设备），看所有设备、有两台以上设备有这种用量时
/// 下面分台列出。小图、数字和账号那几行同一个纵轴、横轴。不是哪个账号的，点了不跳转
private struct CostOrphanGroup: View {
    let cost: AccountCost
    let total: CostTotal
    let scale: CostStatsScale
    let now: Double
    var charts = true

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("无法识别账号")
                .font(.title3.weight(.semibold))
                .accessibilityAddTraits(.isHeader)
                .padding(.top, 10)
            VStack(spacing: 0) {
                CostOrphanRow(cost: cost, total: total, scale: scale, now: now, charts: charts)
                if CostChart.listsOrphanDevices(cost, scope: scale.scope) {
                    VStack(spacing: 0) {
                        ForEach(cost.devices) { d in
                            CostDeviceRow(account: nil, cost: cost, device: d, scale: scale, now: now, charts: charts)
                        }
                    }
                    .padding(.bottom, 6)
                }
            }
            .padding(.vertical, 4)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
        }
    }
}

/// 「认不出账号」那一组的第一行：看的那一方（所有设备 / 这台设备 / 那台设备）认不出是哪个账号的用量。和账号那一行一样的几栏，
/// 左边是一个问号（没有账号的头像）和「没算进合计」
private struct CostOrphanRow: View {
    let cost: AccountCost
    let total: CostTotal
    let scale: CostStatsScale
    let now: Double
    var charts = true

    var body: some View {
        let c = cost
        let summary = CostChart.summary(c, scope: scale.scope)
        let name = Texts.costOrphanRowName(scale.scope, total: total)
        let lines = [Texts.costCaption(summary, granularity: scale.granularity, currency: c.currency),
                     Texts.costUnattributed(total.unattributed, scope: scale.scope, total: total, currency: c.currency),
                     Texts.costUnsynced(total.unattributed, scope: scale.scope)].compactMap { $0 }
        HStack(spacing: CostStatsColumns.spacing) {
            HStack(spacing: 10) {
                Image(systemName: "questionmark")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(.secondary)
                    .frame(width: 28, height: 28)
                    .background(Circle().fill(Color.primary.opacity(0.08)))
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 3) {
                    Text(name).font(.body.weight(.medium)).lineLimit(1)
                    Text("未计入合计").font(.caption).foregroundStyle(.secondary)
                }
            }
            .frame(width: CostStatsColumns.name, alignment: .leading)
            if charts {
                CostStatsChart(bars: CostChart.bars(c, granularity: scale.granularity, scope: scale.scope), currency: c.currency,
                               requests: scale.scope == .all, scale: scale, now: now)
            } else {
                Spacer(minLength: 0)
            }
            CostStatsFigures(summary: summary, currency: c.currency, scale: scale)
        }
        .padding(.horizontal, CostStatsColumns.inset)
        .padding(.vertical, 10)
        .help(lines.joined(separator: "\n"))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("无法识别账号：" + name)
        .accessibilityValue(lines.joined(separator: "；"))
    }
}
