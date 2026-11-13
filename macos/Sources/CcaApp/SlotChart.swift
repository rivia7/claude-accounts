import SwiftUI

/// 横轴上占一段时间的一项：每周用量的一周（UsageWeek）、折合金额的一项（CostBar）。用结束时间认（鼠标指着的是哪一项）。
protocol TimeSlot {
    var startEpoch: Double { get }
    var endEpoch: Double { get }
}

extension UsageWeek: TimeSlot {}

extension CostBar: TimeSlot {
    var startEpoch: Double { period.startEpoch }
    var endEpoch: Double { period.endEpoch }
}

/// 按时间排的柱状图的骨架：每周用量的图（WeeklyChart）和折合金额的图（CostBarChart）共用，所以两种图的排法、悬停一个样子。
/// 每一项在横轴上占它那一段（start…end；横轴的范围由用的地方给，用量统计页所有行同一个，上下对得齐），柱子在这一段的正中间，
/// 一项太窄（中间重新计时的那一周）时柱子跟着变细；鼠标指着的那一列垫一层浅底（和卡片、列表行的悬停一样），整列都算
/// （柱子旁边的空当、下面的日期也是这一项）；柱子下面可以写日期（dateRow 是 0 时不写）。柱子本身画成什么样由用的地方给。
struct SlotChart<Item: TimeSlot, Bar: View>: View {
    let items: [Item]
    let domain: ClosedRange<Double>
    /// 柱子最粗多少（一项太窄时是它宽度的六成，至少 2 点）
    let barWidth: CGFloat
    /// 柱子上面留的空（鼠标指着时那一列的浅底不贴着柱子）、下面写日期那一行的高
    var topInset: CGFloat = 0
    var dateRow: CGFloat = 0
    /// 鼠标指着的那一项（它的 endEpoch）；给了才跟着鼠标
    var hover: Binding<Double?>?
    /// 柱子下面写什么（nil 不写）：第几项、这一项、它在横轴上占多宽
    var label: (Int, Item, CGFloat) -> String? = { _, _, _ in nil }
    /// 读屏：这一项叫什么、是多少
    let accessibility: (Item) -> (label: String, value: String)
    /// 一根柱子：它拿到的大小是柱子的粗细 × 画柱子那一段的高（从下往上画）
    let bar: (Item) -> Bar

    var body: some View {
        let hovered = hover?.wrappedValue
        GeometryReader { geo in
            let size = geo.size
            let plot = max(size.height - topInset - dateRow, 1)
            let slots = Self.slots(items, domain: domain, width: size.width)
            let chart = ZStack(alignment: .topLeading) {
                ForEach(slots, id: \.key) { s in
                    let on = s.key == hovered
                    if on {
                        // 鼠标指着的那一列垫一层浅底，和卡片、列表行的悬停一样
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
                    // 日期写在柱子的正下方
                    if dateRow > 0, let text = label(s.index, s.item, s.width) {
                        Text(text)
                            .font(.caption.monospacedDigit())
                            .foregroundStyle(on ? .primary : .secondary)
                            .fixedSize()
                            .position(x: s.mid, y: size.height - dateRow / 2)
                            .accessibilityHidden(true)
                    }
                }
            }
            .frame(width: size.width, height: size.height)
            .contentShape(Rectangle())
            if let hover {
                // 整列都算：指着柱子旁边的空当、下面的日期也是这一项
                chart.onContinuousHover { phase in
                    guard case .active(let p) = phase else {
                        hover.wrappedValue = nil
                        return
                    }
                    hover.wrappedValue = slots.first { $0.x0 <= p.x && p.x < $0.x1 }?.key
                }
            } else {
                chart
            }
        }
    }

    /// 一项在横轴上占的那一段（只算看得见的部分，不到 1 点宽的不画：CostChart.slot）
    struct Slot {
        let index: Int
        let item: Item
        let x0: CGFloat
        let x1: CGFloat
        var key: Double { item.endEpoch }
        var mid: CGFloat { (x0 + x1) / 2 }
        var width: CGFloat { x1 - x0 }
    }

    static func slots(_ items: [Item], domain: ClosedRange<Double>, width: CGFloat) -> [Slot] {
        items.enumerated().compactMap { i, item in
            CostChart.slot(start: item.startEpoch, end: item.endEpoch, domain: domain, width: Double(width))
                .map { Slot(index: i, item: item, x0: CGFloat($0.x0), x1: CGFloat($0.x1)) }
        }
    }
}

/// 纵轴那一栏：宽是最宽的那个刻度（看不见的文字撑开）。折合金额的大图在上面写刻度（CostBarChart）；账号页「每周用量」的图拿同样的
/// 刻度留出同样宽的空（WeeklyChart 的 gutter），两张图的绘图区一样宽，同一周的条上下对齐
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
