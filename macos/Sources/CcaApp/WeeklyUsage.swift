import SwiftUI

/// 每周用量的图：账号页的「每周用量」（WeeklyUsageSection）和「用量统计」页的每一行（UsageStatsPage）共用。
/// 和额度砖里的用量条（Meter）是同一种条，只是竖着排：一周一根，灰色的底槽是这一周的额度（100%），从下往上填到用了多少，填满就是用完；
/// 按这一周用了多少上色，和额度砖里的条同一套（70% 以下绿、70–89% 橙、90% 以上红：同一个百分比在哪儿都是同一个颜色；
/// 不填账号色，账号色里也有红、绿、橙，会被看成状态），结束前一天没查到的（实际可能更多）填灰色（和数据旧了的用量条一样，不靠透明度）；中间重置过、超过 100% 的，
/// 整根高出底槽。横轴是时间，每根条在它那一周的正中间（中间重置后重新计时的，那一周截到下一周开始）。
/// 不画坐标轴、网格线和每根条上的数字：数字在下面那一行（鼠标指着哪一周就说哪一周）和读屏里。数字、哪一周没记全都是 cca 给的。
/// compact 是用量统计页的小图：条细一点，不画日期，不跟着鼠标。横轴的范围由用的地方给（统计页所有行同一个范围，上下对得齐）。
struct WeeklyChart: View {
    let weeks: [UsageWeek]
    let domain: ClosedRange<Double>
    let now: Double
    var compact = false
    /// 纵轴最高到多少（百分比，至少 100）；不给就按这几周自己的。用量统计页所有行用同一个，各行的高低才能比
    var top: Double?
    /// 鼠标指着的那一周（它的 endEpoch）；给了才跟着鼠标
    var hover: Binding<Double?>?
    /// 右边留出和这几个刻度一样宽的一栏（账号页：下面「折合金额」的大图的纵轴，CostChart.gutterLabels），两张图的绘图区一样宽
    var gutter: [String] = []

    static let week: Double = 7 * 86400

    /// 横轴的范围：最右边是最晚结束的那一周（还没过完的这一周的重置时间）和现在里晚的那个，往左 13 周（最近 12 周和这一周）。
    static func domain(_ lists: [[UsageWeek]], now: Double) -> ClosedRange<Double> {
        let right = max(now, lists.joined().map(\.endEpoch).max() ?? now)
        return (right - 13 * week)...right
    }

    /// 纵轴最高到多少：100%，有超过 100% 的（中间重置过）就到最高的那一周
    static func scaleTop(_ lists: [[UsageWeek]]) -> Double {
        max(100, lists.joined().map(\.usedPercent).max() ?? 0)
    }

    /// 条有多粗：统计页的小图和额度砖里的条（6）一样细，账号页的粗一些。一周太窄（中间重新计时的）时跟着变细
    private var barWidth: CGFloat { compact ? 6 : 10 }
    /// 账号页的条上面留的空（鼠标指着时那一列的浅底不贴着条）和下面日期那一行的高
    private var topInset: CGFloat { compact ? 0 : 8 }
    private var dateRow: CGFloat { compact ? 0 : 18 }

    var body: some View {
        let top = self.top ?? Self.scaleTop([weeks])
        // 排法、悬停、日期都在 SlotChart（和折合金额的图共用）；日期写在条的正下方，一周太窄（中间重新计时的）时不写，
        // 免得和旁边的挤在一起（鼠标指着时下面那一行会说）
        HStack(alignment: .top, spacing: 8) {
            SlotChart(items: weeks, domain: domain, barWidth: barWidth, topInset: topInset, dateRow: dateRow, hover: hover,
                      label: { _, w, width in width >= CGFloat(CostChart.weekLabelMinWidth) ? Texts.weekLabel(w) : nil },
                      accessibility: { w in (Texts.weekLabel(w), Texts.weekDetail(w, now: now)) }) { w in
                WeekBar(percent: w.usedPercent, top: top,
                        color: w.partial ? Meter.dimColor : Palette.usage(percent: w.usedPercent))
            }
            if !gutter.isEmpty { AxisColumn(labels: gutter) }   // 和折合金额的大图的 HStack 一样：间距 8、右边一栏
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("每周用量")
    }
}

/// 一周的条：竖着的 Meter（同样的底槽、两头圆）。底槽是这一周的额度（100%），高是 100 / top；从下往上填。
/// 超过 100% 的（中间重置过）整根高出底槽：哪一段是重置前的 cca 不给，所以不在 100% 处断开。
/// 用得很少时不像横着的 Meter 那样至少画一个圆点（竖着的条短，会显得比实际多）：填的那段的圆头落在实际的高度，按底槽的形状裁掉多出来的。
private struct WeekBar: View {
    let percent: Double
    let top: Double
    let color: Color

    var body: some View {
        GeometryReader { g in
            let w = g.size.width, h = g.size.height
            let scale = max(top, 100)
            let track = h * 100 / scale
            let fill = percent > 0 ? max(h * min(percent, scale) / scale, 1.5) : 0
            Group {
                if fill > track {
                    Capsule().fill(color).frame(width: w, height: fill)
                } else {
                    ZStack(alignment: .bottom) {
                        Capsule().fill(Surface.track)
                        if fill > 0 {
                            Capsule().fill(color)
                                .frame(height: max(fill, w))
                                .offset(y: max(w - fill, 0))
                        }
                    }
                    .frame(width: w, height: track)
                    .clipShape(Capsule())
                }
            }
            .frame(width: w, height: h, alignment: .bottom)
        }
    }
}

/// 账号页的「每周用量」（官方账号，在「用量」下面）：最近 12 周每周用到这一周额度的百分之几。Claude、OpenAI 都不给以前的用量，
/// 这是 cca 每次查用量时记下的，所以图从开始记的那一周起一周一周长出来。下面一行是统计，鼠标指着一周时换成那一周的细节。
struct WeeklyUsageSection: View {
    let account: Account
    /// 图的右边留多宽（WeeklyChart 的 gutter）：账号页给下面「折合金额」的大图的刻度
    var gutter: [String] = []
    /// 鼠标指着的那一周（命令行工具编不了 @State 宏：State 类型直接用）
    private let hover = State<Double?>(initialValue: nil)

    var body: some View {
        TimelineView(.periodic(from: .now, by: 60)) { ctx in
            let now = ctx.date.timeIntervalSince1970
            VStack(alignment: .leading, spacing: 12) {
                SectionHeader("每周用量") {
                    if let since = account.usageWeeks?.sinceEpoch {
                        Text(Texts.weeksSince(since, now: now)).font(.caption).foregroundStyle(.secondary)
                    }
                }
                content(now: now)
            }
        }
    }

    @ViewBuilder
    private func content(now: Double) -> some View {
        if let uw = account.usageWeeks, !uw.weeks.isEmpty {
            let hovered = uw.weeks.first { $0.endEpoch == hover.wrappedValue }
            VStack(alignment: .leading, spacing: 10) {
                WeeklyChart(weeks: uw.weeks, domain: WeeklyChart.domain([uw.weeks], now: now), now: now, hover: hover.projectedValue,
                            gutter: gutter)
                    .frame(height: 128)
                Text(hovered.map { Texts.weekDetail($0, now: now) } ?? Texts.weeksSummary(uw.summary) ?? "这一周过完就有第一周的统计。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                // 图例：前面是一根小的灰条。鼠标指着一周时也留着：这一块不变高矮，下面的内容不跟着跳
                if uw.weeks.contains(where: \.partial) {
                    HStack(spacing: 6) {
                        WeekBar(percent: 70, top: 100, color: Meter.dimColor)
                            .frame(width: 5, height: 12)
                            .accessibilityHidden(true)
                        Text("填成灰色的：那一周结束前一天没查到，实际可能更多。")
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                }
            }
            .padding(16)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
        } else {
            Text(account.usageWeeks == nil ? "还没有记录：每次查用量时会记下这一周用到多少。" : "最近 12 周没有记录。")
                .foregroundStyle(.secondary)
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
        }
    }
}
