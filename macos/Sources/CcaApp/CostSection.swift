import SwiftUI

/// 折合金额的柱状图：账号页「折合金额」的大图、折合金额页合计的 30 天（右边写纵轴的刻度、柱子下面写日期、跟着鼠标），
/// 折合金额页每一行的小图（compact：不写日期、不画刻度，所有行同一个横轴、同一个纵轴）。
/// 排法和每周用量的图一样（SlotChart）：一项占它那一段时间，柱子在正中间，鼠标指着的那一列垫一层浅底。
/// 柱子是中性色：不用账号色，也不用用量的绿、橙、红（折了多少钱没有好坏）；开始时还没有记录的那一项（cca 标的 `partial`）
/// 填灰色（和每周用量里没记全的那一周一样）。美元没有上限，所以和每周用量的图不同，画了纵轴：右边三个刻度（0、一半、最高，
/// 最高是 CostChart.niceTop 取的整齐的数）和很淡的横线。数字、哪一项是灰色都是 cca 给的。
struct CostBarChart: View {
    let bars: [CostBar]
    let granularity: CostGranularity
    let domain: ClosedRange<Double>
    /// 纵轴最高到多少（CostChart.niceTop）：折合金额页所有行用同一个，各行的高低才能比
    let top: Double
    var currency = "USD"
    /// 读屏里说不说请求数：cca 只给整个账号的（看所有设备时），本机、一台设备的没有
    var requests = true
    var compact = false
    let now: Double
    /// 鼠标指着的那一项（它的 endEpoch）；给了才跟着鼠标
    var hover: Binding<Double?>?

    /// 柱子的颜色：和每周用量的条差不多重，深浅色下都看得清
    static let barColor = Color.primary.opacity(0.62)
    /// 纵轴刻度的横线：比分隔线还淡
    static let gridColor = Color.primary.opacity(0.08)

    /// 条有多粗：小图和每周用量的小图（6）一样细，大图和每周用量的大图（10）一样；一项太窄时跟着变细（SlotChart）
    private var barWidth: CGFloat { compact ? 6 : 10 }
    private var topInset: CGFloat { compact ? 0 : 8 }
    private var dateRow: CGFloat { compact ? 0 : 18 }

    var body: some View {
        let ticks = CostChart.ticks(top: top)
        HStack(alignment: .top, spacing: 8) {
            ZStack {
                grid(compact ? [0] : ticks)   // 小图只画底下那一条（各行的柱子落在同一条线上）
                SlotChart(items: bars, domain: domain, barWidth: barWidth, topInset: topInset, dateRow: dateRow, hover: hover,
                          label: { label($0, $1, $2) }, accessibility: accessibility) { b in
                    CostColumn(usd: b.usd, top: top, color: b.period.partial ? Meter.dimColor : Self.barColor)
                }
            }
            if !compact { axis(ticks) }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(granularity == .week ? "每周折合金额" : "每天折合金额")
    }

    /// 柱子下面的日期：按周的和每周用量的图一样，一项太窄（换了切法的零头）时不写；按天的从今天往前每隔几天写一个
    /// （一天的宽度乘它放得下一个日期：1、2、7、14 天）
    private func label(_ index: Int, _ bar: CostBar, _ width: CGFloat) -> String? {
        switch granularity {
        case .week:
            return Double(width) >= CostChart.weekLabelMinWidth ? Texts.costLabel(bar.period, granularity: .week) : nil
        case .day:
            // 夏令时的那一天长一点、短一点：按 24 小时的宽度算隔几天，各天才一样
            let span = bar.period.endEpoch - bar.period.startEpoch
            let day = span > 0 ? Double(width) * CostChart.day / span : Double(width)
            let stride = CostChart.dayLabelStride(slotWidth: day)
            return CostChart.showsDayLabel(index: index, count: bars.count, stride: stride)
                ? Texts.costLabel(bar.period, granularity: .day) : nil
        }
    }

    private func accessibility(_ bar: CostBar) -> (label: String, value: String) {
        var value = Texts.money(bar.usd, currency: currency)
        if requests, bar.period.requests > 0 { value += "，\(Texts.grouped(bar.period.requests)) 次请求" }
        if bar.period.partial { value += "，这一项开始时还没有记录" }
        let label = bar.period.current ? (granularity == .week ? "本周到现在" : "今天到现在")
            : Texts.costRange(bar.period, granularity: granularity, now: now)
        return (label, value)
    }

    /// 0、一半、最高三条很淡的横线（和柱子同一个纵轴）；小图只有 0 那一条
    private func grid(_ ticks: [Double]) -> some View {
        GeometryReader { g in
            let plot = max(g.size.height - topInset - dateRow, 1)
            Path { p in
                for v in ticks {
                    let y = topInset + plot * (1 - CGFloat(v / top))
                    p.move(to: CGPoint(x: 0, y: y))
                    p.addLine(to: CGPoint(x: g.size.width, y: y))
                }
            }
            .stroke(Self.gridColor, lineWidth: 1)
        }
        .accessibilityHidden(true)
    }

    /// 右边的刻度：和横线对齐，这一栏的宽是最宽的那个刻度（AxisColumn：账号页「每周用量」的图照它留出同样宽的一栏）
    private func axis(_ ticks: [Double]) -> some View {
        let labels = CostChart.axisLabels(top: top, currency: currency)
        return AxisColumn(labels: labels)
            .frame(maxHeight: .infinity)
        .overlay {
            GeometryReader { g in
                let plot = max(g.size.height - topInset - dateRow, 1)
                ForEach(Array(zip(ticks, labels)), id: \.0) { v, text in
                    Text(text)
                        .font(.caption2.monospacedDigit())
                        .foregroundStyle(.secondary)
                        .fixedSize()
                        .frame(width: g.size.width, alignment: .leading)
                        .position(x: g.size.width / 2, y: topInset + plot * (1 - CGFloat(v / top)))
                }
            }
        }
        .accessibilityHidden(true)
    }
}

/// 一根柱子：和每周用量的条一个形状（两头圆，竖着的 Meter 填的那一段），只是没有底槽（钱没有上限，没有“填满”）。
/// 很少时不至少画一个圆点（竖着的条短，会显得比实际多）：圆头落在实际的高度，按两头圆的形状裁掉多出来的（和每周用量的条一样）。
struct CostColumn: View {
    let usd: Double
    let top: Double
    let color: Color

    var body: some View {
        GeometryReader { g in
            let w = g.size.width, h = g.size.height
            let fill = usd > 0 && top > 0 ? max(h * min(usd / top, 1), 1.5) : 0
            ZStack(alignment: .bottom) {
                if fill > 0 {
                    Capsule().fill(color)
                        .frame(height: max(fill, w))
                        .offset(y: max(w - fill, 0))
                }
            }
            .frame(width: w, height: h, alignment: .bottom)
            .clipShape(Capsule())
        }
    }
}

/// 图例：前面是一根小的灰柱子（开始时还没有记录的那一项）
struct CostPartialLegend: View {
    var body: some View {
        HStack(spacing: 6) {
            Capsule().fill(Meter.dimColor)
                .frame(width: 5, height: 12)
                .accessibilityHidden(true)
            Text("颜色淡的：那一项开始时还没有记录，实际可能更多。")
                .fixedSize(horizontal: false, vertical: true)
        }
        .font(.caption)
        .foregroundStyle(.secondary)
    }
}

/// 看本机还是所有设备：账号页「折合金额」的标题、折合金额页的标题上各一个，两页共用一个设置（AppModel.costScope）
struct CostScopePicker: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Picker("看哪些设备", selection: Binding(get: { model.costScope }, set: { model.costScope = $0 })) {
            Text("本机").tag(CostScope.me)
            Text("所有设备").tag(CostScope.all)
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .fixedSize()
        .help("本机：只算这台设备上用的；所有设备：整个账号，各台设备加起来")
    }
}

/// 按周还是按天看：同上，两页共用一个设置（AppModel.costGranularity）
struct CostGranularityPicker: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Picker("按周还是按天", selection: Binding(get: { model.costGranularity }, set: { model.costGranularity = $0 })) {
            Text("周").tag(CostGranularity.week)
            Text("日").tag(CostGranularity.day)
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .fixedSize()
        .help("周：最近 12 周和本周；日：最近 30 天")
    }
}

/// 同步的状态、还没算进来的设备（cca 给的，Texts.costNotes 写成一句一句的）：要处理的橙色，别的灰字
struct CostNoteLine: View {
    let note: CostNote

    var body: some View {
        Label(note.text, systemImage: note.warning ? "exclamationmark.triangle" : "info.circle")
            .font(.caption)
            .foregroundStyle(note.warning ? Color.orange : Color.secondary)
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// 账号页的「折合金额」（在「每周用量」下面；cca 说这个账号有（`can.cost`）、cca 也有折合金额时才显示）：
/// 这个账号（所有设备加起来，或者只算这台设备）最近 12 周每周、最近 30 天每天按官方 API 的价格折成多少美元。不是实际花费：订阅不按这个收费。
/// 标题右边是多久以前统计的、本机 / 所有设备、按周 / 按日（这两个和折合金额页共用）和「刷新」。图下面一行是统计（鼠标指着一根柱子时换成那一项的细节），
/// 再下面是同步的提示，显示细节时多几行（token、没有公开价格的模型、从什么时候开始有记录、上次同步）；最下面一直有一行小字：
/// 按谁的价格折算、价格表是哪天的。数字都是 cca 给的（PROTOCOL 2.2 的 `cost`），App 只排版。
struct CostSection: View {
    @Environment(AppModel.self) private var model
    @Environment(Guide.self) private var guide
    let account: Account
    /// 鼠标指着的那一项（命令行工具编不了 @State 宏：State 类型直接用）
    private let hover = State<Double?>(initialValue: nil)

    /// 折合金额页点了一行时滚到这里（ScrollViewReader 的 id）
    static let anchor = "cost"

    var body: some View {
        TimelineView(.periodic(from: .now, by: 60)) { ctx in
            let now = ctx.date.timeIntervalSince1970
            VStack(alignment: .leading, spacing: 12) {
                SectionHeader("折合金额") { header(now: now) }
                content(now: now)
            }
        }
    }

    // MARK: 标题右边

    @ViewBuilder
    private func header(now: Double) -> some View {
        let c = account.cost
        let scan = model.state?.costTotal?.scan
        Group {
            if model.costRefreshing || model.costScanning {
                ProgressView().controlSize(.small)
            } else if let text = Texts.costFreshness(scan, now: now) {
                Text(text)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
                    .lineLimit(1)
            }
        }
        .layoutPriority(-1)   // 放不下时先让它省略，按钮不挤
        if c != nil {
            CostScopePicker()
            CostGranularityPicker()
        }
        SectionAction(model.costRefreshing ? "刷新中…" : "刷新", systemImage: "arrow.clockwise") { model.refreshCost() }
            .disabled(model.costRefreshing)
            .help("现在统计一次，再和这个账号的其他设备同步")
    }

    // MARK: 内容

    @ViewBuilder
    private func content(now: Double) -> some View {
        let total = model.state?.costTotal
        VStack(alignment: .leading, spacing: 10) {
            if let c = account.cost {
                tile(c, now: now)
            } else {
                emptyTile(scanning: model.costScanning, never: total?.scan.state == .never)
            }
            if let f = model.costFailure {
                Label(f, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Text(Texts.costFootnote([account.tool], pricesDate: account.cost?.pricesDate ?? total?.pricesDate))
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func tile(_ c: AccountCost, now: Double) -> some View {
        let g = model.costGranularity
        let scope = model.costScope
        let bars = CostChart.bars(c, granularity: g, scope: scope)
        let hovered = bars.first { $0.id == hover.wrappedValue }
        return VStack(alignment: .leading, spacing: 10) {
            if CostChart.charted(bars) {
                CostBarChart(bars: bars, granularity: g,
                             domain: CostChart.domain([CostChart.periods(c, g)], granularity: g, now: now),
                             top: CostChart.niceTop(CostChart.maxUSD([bars])), currency: c.currency, requests: scope == .all,
                             now: now, hover: hover.projectedValue)
                    .frame(height: 128)
                Text(hovered.map { Texts.costBarDetail($0, granularity: g, scope: scope, currency: c.currency, now: now) }
                     ?? Texts.costCaption(CostChart.summary(c, scope: scope), granularity: g, currency: c.currency))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
                    .fixedSize(horizontal: false, vertical: true)
                // 图例：鼠标指着一项时也留着，这一块不变高矮
                if bars.contains(where: { $0.period.partial && $0.usd > 0 }) { CostPartialLegend() }
            } else {
                Text((scope == .me ? "这台设备" : "") + (g == .week ? "最近 12 周没有用量。" : "最近 30 天没有用量。"))
                    .foregroundStyle(.secondary)
            }
            ForEach(Array(Texts.costNotes(c, scope: scope, now: now).enumerated()), id: \.offset) { _, n in
                CostNoteLine(note: n)
            }
            if guide.showDetails { details(c, bars: bars, hovered: hovered, scope: scope, granularity: g, now: now) }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
    }

    /// 显示细节时：指着的那一项（没指着时是本周 / 今天）的 token（cca 只给整个账号的：看所有设备时才有）、没有公开价格的模型、
    /// 从什么时候开始有记录、上次同步
    @ViewBuilder
    private func details(_ c: AccountCost, bars: [CostBar], hovered: CostBar?, scope: CostScope,
                         granularity g: CostGranularity, now: Double) -> some View {
        let bar = hovered ?? bars.last(where: \.period.current)
        VStack(alignment: .leading, spacing: 4) {
            if scope == .all, let bar, let t = bar.period.tokens, t.total > 0 {
                let when = hovered == nil || bar.period.current ? (g == .week ? "本周到现在" : "今天到现在")
                    : Texts.costRange(bar.period, granularity: g, now: now)
                Text(when + "的 token：" + Texts.costTokens(t))
            }
            if let u = Texts.costUnpriced(c.unpricedModels, requests: c.summary.unpricedRequests) { Text(u) }
            if let since = c.sinceEpoch { Text(Texts.costSince(since, tool: account.tool, now: now)) }
            if let at = c.sync?.checkedEpoch { Text(Texts.costSynced(at, now: now)) }
        }
        .font(.caption)
        .foregroundStyle(.secondary)
        .monospacedDigit()
        .fixedSize(horizontal: false, vertical: true)
    }

    /// 还没有记录：正在统计（第一次要把会话记录全读一遍）时说一声，否则说用过以后会有
    /// scanning：第一次统计还没做完；never：还没统计过、也没在统计（上次没统计成，或者用户走开了）
    private func emptyTile(scanning: Bool, never: Bool) -> some View {
        Group {
            if scanning {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("正在统计：第一次要把会话记录全读一遍，可能要一会儿。")
                }
            } else {
                Text(never ? Texts.costNotScanned : Texts.costEmpty(account.tool))
            }
        }
        .foregroundStyle(.secondary)
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
    }
}
