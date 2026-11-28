import CcaRemoteKit
import SwiftUI

// 按时间排的柱状图（Mac App 的 SlotChart.swift、CostSection.swift 的 CostBarChart、CostView.swift 的 CostChart）：每周用量的图、
// 折合金额的图共用一个骨架，所以两种图的排法、点选一个样子。手机上没有悬停：点一根柱子选中它（下面那一行说它），再点一下取消

/// 横轴上占一段时间的一项：每周用量的一周、折合金额的一项。用结束时间认（点的是哪一项）
protocol TimeSlot {
    var startEpoch: Double { get }
    var endEpoch: Double { get }
}

extension UsageWeek: TimeSlot {}
extension CostPeriod: TimeSlot {}

/// 柱子下面写不写日期时用得到的：第几项（从 0 数）、一共几项、这一项占多宽、整周（整天）的一项占多宽
struct SlotPlace {
    let index: Int
    let count: Int
    let width: CGFloat
    let fullWidth: CGFloat
}

/// 骨架：每一项在横轴上占它那一段（横轴的范围由用的地方给：统计页所有行同一个，上下对得齐），柱子在这一段的正中间，
/// 一项太窄（中间重新计时的那一周）时柱子跟着变细；选中的那一列垫一层浅底；柱子下面可以写日期（dateRow 是 0 时不写）
struct SlotChart<Item: TimeSlot, Bar: View>: View {
    let items: [Item]
    let domain: ClosedRange<Double>
    /// 柱子最粗多少（一项太窄时是它宽度的六成，至少 2 点）
    let barWidth: CGFloat
    var topInset: CGFloat = 0
    var dateRow: CGFloat = 0
    /// 选中的那一项（它的 endEpoch）；给了才能点
    var picked: Binding<Double?>?
    var label: (Item, SlotPlace) -> String? = { _, _ in nil }
    let accessibility: (Item) -> (label: String, value: String)
    let bar: (Item) -> Bar

    struct Slot {
        let index: Int
        let item: Item
        let x0: CGFloat
        let x1: CGFloat
        var key: Double { item.endEpoch }
        var mid: CGFloat { (x0 + x1) / 2 }
        var width: CGFloat { x1 - x0 }
    }

    var body: some View {
        let on = picked?.wrappedValue
        GeometryReader { geo in
            let size = geo.size
            let plot = max(size.height - topInset - dateRow, 1)
            let slots = Self.slots(items, domain: domain, width: size.width)
            let full = slots.map(\.width).max() ?? 0
            ZStack(alignment: .topLeading) {
                ForEach(slots, id: \.key) { s in
                    let chosen = s.key == on
                    if chosen {
                        RoundedRectangle(cornerRadius: 8, style: .continuous)
                            .fill(Color.primary.opacity(0.06))
                            .frame(width: max(s.width - 4, 0), height: size.height)
                            .position(x: s.mid, y: size.height / 2)
                    }
                    let a11y = accessibility(s.item)
                    bar(s.item)
                        .frame(width: min(barWidth, max(s.width * 0.6, 2)), height: plot)
                        .position(x: s.mid, y: topInset + plot / 2)
                        .accessibilityElement()
                        .accessibilityLabel(a11y.label)
                        .accessibilityValue(a11y.value)
                    if dateRow > 0, let text = label(s.item, SlotPlace(index: s.index, count: items.count, width: s.width, fullWidth: full)) {
                        Text(text)
                            .font(.caption2.monospacedDigit())
                            .foregroundStyle(chosen ? .primary : .secondary)
                            .fixedSize()
                            .position(x: s.mid, y: size.height - dateRow / 2)
                            .accessibilityHidden(true)
                    }
                }
            }
            .frame(width: size.width, height: size.height)
            .contentShape(Rectangle())
            .onTapGesture(coordinateSpace: .local) { p in
                guard let picked else { return }
                let hit = slots.first { $0.x0 <= p.x && p.x < $0.x1 }?.key
                picked.wrappedValue = hit == picked.wrappedValue ? nil : hit
            }
            .allowsHitTesting(picked != nil)
        }
    }

    static func slots(_ items: [Item], domain: ClosedRange<Double>, width: CGFloat) -> [Slot] {
        items.enumerated().compactMap { i, item in
            CostChart.slot(start: item.startEpoch, end: item.endEpoch, domain: domain, width: Double(width))
                .map { Slot(index: i, item: item, x0: CGFloat($0.x0), x1: CGFloat($0.x1)) }
        }
    }
}

/// 图怎么排（纯函数）：纵轴的刻度、横轴的范围、表头的日期（Mac App 的 CostChart）
enum CostChart {
    static let week: Double = 7 * 86400
    static let day: Double = 86400
    /// 按周看的范围（最近 12 周和这一周）、按天看的范围（最近 30 天）
    static let weekSpan = 13 * week
    static let daySpan = 30 * day
    /// 柱子顶上至少留这么多空：最高的那根最多到纵轴的九成
    static let headroom = 0.9
    /// 日期之间至少隔这么宽（「11/30」放得下）
    static let dayLabelMinWidth = 34.0
    /// 按周看时一项宽于这个才每一项都写日期（和 Mac App 的 weekLabelMinWidth 一样）；手机窄，不到它时隔一项写（Mac 那么窄时不写）
    static let weekLabelMinWidth = 30.0
    /// 纵轴最高到多少的候选（乘 10 的几次方）：一半也是整齐的数（刻度是 0、一半、最高）
    static let topSteps: [Double] = [1, 1.5, 2, 2.5, 3, 4, 5, 6, 8]

    /// 纵轴最高到多少（美元）：最高的那根不超过它的九成，取 topSteps 乘 10 的几次方里够用的最小的那个，最少 1；都是 0 时是 1
    static func niceTop(_ maxValue: Double) -> Double {
        guard maxValue.isFinite, maxValue > 0 else { return 1 }
        let target = maxValue / headroom
        var p = pow(10, floor(log10(target)))
        while p.isFinite {
            for m in topSteps where m * p >= target * (1 - 1e-12) { return max(m * p, 1) }
            p *= 10
        }
        return max(target, 1)
    }

    /// 纵轴的刻度：0、一半、最高
    static func ticks(top: Double) -> [Double] { [0, top / 2, top] }

    /// 几行里最高的一根（美元）
    static func maxUSD(_ lists: [[CostPeriod]]) -> Double {
        lists.joined().map(\.usd).max() ?? 0
    }

    /// 横轴的范围：最右边是最晚结束的那一项（这一周、今天的结束时间）和现在里晚的那个，往左 13 周 / 30 天。各行用同一个（上下对得齐）
    static func domain(_ lists: [[CostPeriod]], days: Bool, now: Double) -> ClosedRange<Double> {
        let right = max(now, lists.joined().map(\.endEpoch).max() ?? now)
        return (right - (days ? daySpan : weekSpan))...right
    }

    /// 一项在横轴上占的那一段（只算看得见的部分；不到 1 点宽的不画）
    static func slot(start: Double, end: Double, domain: ClosedRange<Double>, width: Double) -> (x0: Double, x1: Double)? {
        let span = domain.upperBound - domain.lowerBound
        guard span > 0, width > 0 else { return nil }
        func x(_ t: Double) -> Double { (t - domain.lowerBound) / span * width }
        let x0 = max(x(start), 0), x1 = min(x(end), width)
        return x1 - x0 >= 1 ? (x0, x1) : nil
    }

    /// 按周的日期：一周宽到 weekLabelMinWidth（30 点）时每一项都写（和 Mac App 一样）；手机上一周多半更窄，这时隔一根写一个，
    /// 从这一周往前数；中间重新计时、窄得放不下字的那一项不写
    static func weekLabel(_ text: String, _ p: SlotPlace) -> String? {
        let every = p.fullWidth < weekLabelMinWidth ? 2 : 1
        return (p.count - 1 - p.index) % every == 0 && p.width >= 16 ? text : nil
    }

    /// 按天看时隔几天写一个日期：一天的宽度乘它至少要放得下一个日期（1、2、7、14 天里最小的那个）
    static func dayLabelStride(slotWidth: Double) -> Int {
        for k in [1, 2, 7, 14] where slotWidth * Double(k) >= dayLabelMinWidth { return k }
        return 14
    }

    /// 表头的日期（用量统计页、折合金额页，和各行的小图同一个时间范围）：从今天往前每隔几周（按天看时每隔几天）一个。
    /// 挨着两头的往里挪到 edge；隔 2、3、4、6 周（按天 7、14 天）里取最密、又一个都不用跳过的。从右往左排，第一个是今天
    static func statsTicks(domain: ClosedRange<Double>, now: Double, width: Double, days: Bool = false,
                           edge: Double = 18) -> [(epoch: Double, x: Double)] {
        let span = domain.upperBound - domain.lowerBound
        guard span > 0, span.isFinite, now.isFinite, width > 2 * edge else { return [] }
        let start = days ? domain.upperBound - day / 2 : now
        let strides = days ? [7, 14].map { Double($0) * day } : [2, 3, 4, 6].map { Double($0) * week }
        var out: [(epoch: Double, x: Double)] = []
        for step in strides {
            out = []
            var t = start, skipped = false
            while t >= domain.lowerBound {
                let x = min(max((t - domain.lowerBound) / span * width, edge), width - edge)
                if out.last.map({ $0.x - x >= dayLabelMinWidth }) ?? true {
                    out.append((t, x))
                } else {
                    skipped = true
                }
                t -= step
            }
            if !skipped { break }
        }
        return out
    }

    /// 几个账号按天的金额加在一起（折合金额页合计那一块的 30 天）：以最晚结束的那一份为准，别的每一天落在它哪一天里就加到哪一天
    /// （几台电脑的时区不一样、有的电脑好几天没放上来时，日子也对得上；落在外面的不算）
    static func sumDays(_ lists: [[CostPeriod]]) -> [CostPeriod] {
        guard let base = lists.max(by: { ($0.last?.endEpoch ?? 0) < ($1.last?.endEpoch ?? 0) }), !base.isEmpty else { return [] }
        var cents = Array(repeating: 0, count: base.count)
        for l in lists {
            for p in l {
                let mid = (p.startEpoch + p.endEpoch) / 2
                if let i = base.firstIndex(where: { $0.startEpoch <= mid && mid < $0.endEpoch }) { cents[i] += p.cents }
            }
        }
        return base.enumerated().map { i, p in
            CostPeriod(startEpoch: p.startEpoch, endEpoch: p.endEpoch, cents: cents[i], current: p.current, partial: false)
        }
    }
}

/// 表头里小图上面的日期：最右边是今天，往前每隔几周（按天看时每隔几天）一个（CostChart.statsTicks）
struct StatsTimeScale: View {
    let domain: ClosedRange<Double>
    let now: Double
    var days = false
    /// 写不写「今天」（第一个刻度）：旁边那一栏已经写着「今天」时不写
    var today = true

    var body: some View {
        GeometryReader { geo in
            let ticks = CostChart.statsTicks(domain: domain, now: now, width: Double(geo.size.width), days: days)
            ForEach(Array(ticks.enumerated()), id: \.offset) { i, tick in
                if i > 0 || today {
                    Text(i == 0 ? "今天" : Self.day(back: ticks[0].epoch - tick.epoch, now: now))
                        .fixedSize()
                        .position(x: CGFloat(tick.x), y: geo.size.height / 2)
                }
            }
        }
        .font(.caption2.monospacedDigit())
        .foregroundStyle(.secondary)
        .accessibilityHidden(true)
    }

    /// 往前第几天的日期：按日历从今天往回数整天
    private static func day(back seconds: Double, now: Double) -> String {
        let cal = Calendar.current
        let today = Date(timeIntervalSince1970: now)
        let d = cal.date(byAdding: .day, value: -Int((seconds / 86400).rounded()), to: today) ?? today.addingTimeInterval(-seconds)
        let c = cal.dateComponents([.month, .day], from: d)
        return "\(c.month ?? 0)/\(c.day ?? 0)"
    }
}

/// 折合金额的柱状图（Mac App 的 CostBarChart）：账号页的大图、折合金额页合计的 30 天（右边写纵轴的刻度、柱子下面写日期、能点），
/// 折合金额页每一行的小图（compact：不写日期、不画刻度，所有行同一个横轴、同一个纵轴）。柱子是中性色（折了多少钱没有好坏），
/// 开始时还没有记录的那一项填灰色；美元没有上限，所以画了纵轴：右边三个刻度（0、一半、最高）和很淡的横线
struct CostBarChart: View {
    let bars: [CostPeriod]
    let days: Bool
    let domain: ClosedRange<Double>
    /// 纵轴最高到多少（美元，CostChart.niceTop）
    let top: Double
    var compact = false
    let now: Double
    var picked: Binding<Double?>?

    static let barColor = Color.primary.opacity(0.62)
    static let gridColor = Color.primary.opacity(0.08)

    private var topInset: CGFloat { compact ? 0 : 6 }
    private var dateRow: CGFloat { compact ? 0 : 18 }

    var body: some View {
        let ticks = CostChart.ticks(top: top)
        HStack(alignment: .top, spacing: 8) {
            ZStack {
                grid(compact ? [0] : ticks)
                SlotChart(items: bars, domain: domain, barWidth: compact ? 5 : 10, topInset: topInset, dateRow: dateRow, picked: picked,
                          label: label, accessibility: accessibility) { b in
                    CostColumn(usd: b.usd, top: top, color: b.partial ? Meter.dimColor : Self.barColor)
                }
            }
            if !compact { axis(ticks) }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(days ? "每天折合金额" : "每周折合金额")
    }

    private func label(_ bar: CostPeriod, _ p: SlotPlace) -> String? {
        let text = Texts.costLabel(bar, days: days)
        guard days else { return CostChart.weekLabel(text, p) }
        let stride = CostChart.dayLabelStride(slotWidth: Double(p.fullWidth))
        return (p.count - 1 - p.index) % stride == 0 ? text : nil
    }

    private func accessibility(_ bar: CostPeriod) -> (label: String, value: String) {
        let label = bar.current ? (days ? "今天" : "本周至今") : Texts.costRange(bar, days: days, now: now)
        return (label, Texts.money(cents: bar.cents) + (bar.partial ? "，这一项开始时还没有记录" : ""))
    }

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

    private func axis(_ ticks: [Double]) -> some View {
        let labels = ticks.map(Texts.moneyAxis)
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

/// 一根柱子：和每周用量的条一个形状（两头圆），只是没有底槽（钱没有上限）。很少时圆头落在实际的高度，多出来的按形状裁掉
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

/// 纵轴那一栏：宽是最宽的那个刻度（看不见的文字撑开）
struct AxisColumn: View {
    let labels: [String]

    var body: some View {
        ZStack(alignment: .leading) {
            ForEach(labels, id: \.self) { Text($0).hidden() }
        }
        .font(.caption2.monospacedDigit())
        .accessibilityHidden(true)
    }
}

/// 图例：前面是一根小的灰柱子（开始时还没有记录的那一项）
struct CostPartialLegend: View {
    var body: some View {
        HStack(spacing: 6) {
            Capsule().fill(Meter.dimColor).frame(width: 5, height: 12).accessibilityHidden(true)
            Text("颜色较浅的：那一项开始时还没有记录，实际可能更多。").fixedSize(horizontal: false, vertical: true)
        }
        .font(.caption)
        .foregroundStyle(.secondary)
    }
}

/// 按模型分开的金额（cca 排在前面的几个）：名字、一根细条（这一行的金额占这几行加起来的多少）、金额；按周看是近 12 周的，按天看是
/// 30 天的，这一段里没有用量的不列；按天看照 30 天的金额重新排（和 Mac App 的 modelRows 一样）。电脑只放前 3 个（status.rs 的
/// COST_MODELS）：其余的照这个账号这一段的合计减出来，算「其他」（Mac 那边 cca 给全了，没有这一行）；没有按模型记下的那一部分在最后、灰字
struct CostModelList: View {
    let models: [CostModel]
    let days: Bool
    /// 这个账号这一段的合计（AccountCost.total）
    let total: Int

    /// 一行：名字、金额、多少美分（细条的长短）、灰字（其他、没按模型记下的）、读屏念的金额
    private struct Entry {
        let name: String
        let money: String
        let cents: Int
        let dim: Bool
        let spoken: String
    }

    var body: some View {
        let shown = models.filter { $0.cents(days: days) > 0 }
        if !shown.isEmpty {
            let rows = entries(shown)
            let sum = rows.reduce(0) { $0 + $1.cents }
            VStack(alignment: .leading, spacing: 6) {
                Text(Texts.costModelsTitle(days: days))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .accessibilityAddTraits(.isHeader)
                VStack(alignment: .leading, spacing: 5) {
                    ForEach(Array(rows.enumerated()), id: \.offset) { _, r in
                        HStack(spacing: 10) {
                            Text(r.name)
                                .lineLimit(1)
                                .truncationMode(.middle)
                                .foregroundStyle(r.dim ? .secondary : .primary)
                                .frame(maxWidth: .infinity, alignment: .leading)
                            CostShareBar(fraction: sum > 0 ? Double(r.cents) / Double(sum) : 0)
                                .frame(width: 56)
                            Text(r.money)
                                .foregroundStyle(r.dim ? .secondary : .primary)
                                .frame(minWidth: 72, alignment: .trailing)
                        }
                        .accessibilityElement(children: .ignore)
                        .accessibilityLabel(r.name)
                        .accessibilityValue(r.spoken)
                    }
                }
                .font(.subheadline.monospacedDigit())
                if let note = Texts.costEstimated(shown, days: days) {
                    Text(note).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    /// 有名字的（按天看照金额重排）、其他、没按模型记下的
    private func entries(_ shown: [CostModel]) -> [Entry] {
        let named = shown.enumerated().filter { $0.element.name != nil }
            .sorted { a, b in
                guard days, a.element.days != b.element.days else { return a.offset < b.offset }
                return a.element.days > b.element.days
            }
            .map(\.element)
        func entry(_ m: CostModel) -> Entry {
            let money = Texts.costModelMoney(m, days: days)
            return Entry(name: Texts.costModelName(m), money: money, cents: m.cents(days: days), dim: m.name == nil,
                         spoken: money + (m.pricedAs.map { "，按 \($0) 的价格估算" } ?? ""))
        }
        let listed = shown.reduce(0) { $0 + $1.cents(days: days) }
        // 差几分钱的是四舍五入（合计是 cca 算好的，各项到分以后再加会差一两分），不算「其他」
        let other = total - listed > shown.count ? total - listed : 0
        let rest = other > 0 ? [Entry(name: "其他", money: Texts.money(cents: other), cents: other, dim: true, spoken: Texts.money(cents: other))] : []
        return named.map(entry) + rest + shown.filter { $0.name == nil }.map(entry)
    }
}

/// 各台设备的（账号页「折合金额」里，两台以上时）：和按模型的一个样子（名字、占多少的细条、金额）；很久没同步上来的名字后面一个橙色的钟
struct CostDeviceList: View {
    let devices: [CostDevice]
    let days: Bool
    let now: Double

    var body: some View {
        let total = devices.reduce(0) { $0 + (days ? $1.days : $1.weeks) }
        VStack(alignment: .leading, spacing: 6) {
            Text("按设备 · " + (days ? "近 30 天" : "近 12 周"))
                .font(.caption)
                .foregroundStyle(.secondary)
                .accessibilityAddTraits(.isHeader)
            VStack(alignment: .leading, spacing: 5) {
                ForEach(Array(devices.enumerated()), id: \.offset) { _, d in
                    let cents = days ? d.days : d.weeks
                    let stale = Texts.costStale(d, now: now)
                    HStack(spacing: 10) {
                        HStack(spacing: 5) {
                            CostDeviceName(device: d)
                            if stale != nil { Image(systemName: "clock.badge.exclamationmark").foregroundStyle(.orange) }
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        CostShareBar(fraction: total > 0 ? Double(cents) / Double(total) : 0)
                            .frame(width: 56)
                        Text(Texts.money(cents: cents))
                            .frame(minWidth: 72, alignment: .trailing)
                    }
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(Texts.costDeviceName(d))
                    .accessibilityValue(Texts.money(cents: cents) + (stale.map { "；" + $0 } ?? ""))
                }
            }
            .font(.subheadline.monospacedDigit())
        }
    }
}

/// 设备的名字（窄的时候从中间截）和后面的「（这台电脑）」（不截：截掉了就看不出是哪台）
struct CostDeviceName: View {
    let device: CostDevice

    var body: some View {
        HStack(spacing: 0) {
            Text(device.name).lineLimit(1).truncationMode(.middle)
            if let mark = Texts.costDeviceMark(device) {
                Text(mark).lineLimit(1).fixedSize()
            }
        }
    }
}

/// 按模型、按设备那一块的细条：这一行的金额占多少（没有底槽；很少时至少一个圆点那么长）
private struct CostShareBar: View {
    let fraction: Double

    var body: some View {
        GeometryReader { g in
            Capsule().fill(CostBarChart.barColor)
                .frame(width: fraction > 0 ? max(g.size.width * min(fraction, 1), 4) : 0, height: 5)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        }
        .frame(height: 10)
        .accessibilityHidden(true)
    }
}

/// 同步的状态、还没算进来的设备（Texts.costNotes）：要处理的橙色，别的灰字
struct CostNoteLine: View {
    let note: CostNote

    var body: some View {
        Label(note.text, systemImage: note.warning ? "exclamationmark.triangle" : "info.circle")
            .font(.caption)
            .foregroundStyle(note.warning ? Color.orange : Color.secondary)
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// 按周还是按天看（账号页「折合金额」、折合金额页共用一个设置：AppModel.costDays）
struct CostGranularityPicker: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Picker("按周还是按天", selection: Binding(get: { model.costDays }, set: { model.costDays = $0 })) {
            Text("周").tag(false)
            Text("日").tag(true)
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .fixedSize()
    }
}
