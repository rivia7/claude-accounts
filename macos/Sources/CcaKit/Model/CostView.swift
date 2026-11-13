import Foundation

// 折合金额的图怎么排（纯函数，测试用）：柱的高低、哪一根是浅色、平均和合计都是 cca 给的（PROTOCOL 2.2 的 cost），
// 这里只挑看谁的数字、算纵轴的刻度和横轴的范围、哪根柱子下面写日期。

/// 用量统计页看哪种：额度用量（每周用到额度的百分之几）/ 折合金额（按官方 API 的标价折成美元）。记在 App 的设置里。
enum StatsMode: String, Sendable, CaseIterable {
    case quota, cost
}

/// 折合金额按周还是按天看（账号页、用量统计页共用）。记在 App 的设置里。
enum CostGranularity: String, Sendable, CaseIterable {
    case week, day
}

/// 折合金额看谁的（账号页、用量统计页共用）：这台设备（本机），或者所有设备加起来（整个账号）。记在 App 的设置里。
enum CostScope: String, Sendable, CaseIterable {
    case me, all
}

/// 一根柱子：一个周期（或者一天），和看的那一方在这一项里折了多少钱。
struct CostBar: Identifiable, Sendable, Equatable {
    let period: CostPeriod
    let usd: Double

    var id: Double { period.endEpoch }
}

/// 用量统计页表头的一个日期（CostChart.statsTicks）：哪个时刻、在小图那一栏里的横坐标（字的中间）。
struct StatsTick: Equatable, Sendable {
    let epoch: Double
    let x: Double
}

/// 页面上一行提示（同步的状态、哪台设备还没算进来）：warning 的用橙色，别的用灰字。
struct CostNote: Sendable, Equatable {
    let text: String
    var warning = false
}

enum CostChart {
    static let week: Double = 7 * 86400
    static let day: Double = 86400
    /// 按周看的范围（最近 12 周和这一周）、按天看的范围（最近 30 天）
    static let weekSpan = 13 * week
    static let daySpan = 30 * day
    /// 柱子顶上至少留这么多空：最高的那根最多到纵轴的九成
    static let headroom = 0.9
    /// 按周看时一项窄于这个宽度（不到一天的零头并进去以后，中间换了切法的那一项）不写日期，和每周用量的图一样
    static let weekLabelMinWidth = 30.0
    /// 按天看时日期之间至少隔这么宽（「11/30」放得下）；用量统计页表头的日期也是（statsTicks）
    static let dayLabelMinWidth = 34.0

    /// 纵轴最高到多少的候选（乘 10 的几次方）：一半也是整齐的数（刻度是 0、一半、最高）
    static let topSteps: [Double] = [1, 1.5, 2, 2.5, 3, 4, 5, 6, 8]

    /// 纵轴最高到多少：最高的那根不超过它的九成，取 topSteps 乘 10 的几次方里够用的最小的那个，最少 1（美元）；都是 0 时是 1。
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

    /// 大图右边写的刻度
    static func axisLabels(top: Double, currency: String) -> [String] {
        ticks(top: top).map { Texts.moneyAxis($0, currency: currency) }
    }

    /// 账号页「折合金额」画不画大图：有一根柱子有金额才画（都是 0 时只说没有用量）
    static func charted(_ bars: [CostBar]) -> Bool { bars.contains { $0.usd > 0 } }

    /// 账号页「每周用量」的图右边留多宽：和「折合金额」的大图右边的刻度一样宽（两张图的绘图区一样宽，同一周的条上下对齐），
    /// 那里没画图时不留（看本机、这台设备没有用量时也不留）
    static func gutterLabels(_ c: AccountCost?, granularity g: CostGranularity, scope: CostScope) -> [String] {
        guard let c else { return [] }
        let bars = bars(c, granularity: g, scope: scope)
        return charted(bars) ? axisLabels(top: niceTop(maxUSD([bars])), currency: c.currency) : []
    }

    /// 这几根柱子里最高的（几行共用一个纵轴时取所有行的）
    static func maxUSD(_ lists: [[CostBar]]) -> Double {
        lists.joined().map(\.usd).max() ?? 0
    }

    static func periods(_ c: AccountCost, _ g: CostGranularity) -> [CostPeriod] {
        g == .week ? c.weeks : c.days
    }

    /// 柱子：所有设备用 weeks / days 的 usd，本机用这台设备的（这台设备没有这个账号的记录时都是 0）
    static func bars(_ c: AccountCost, granularity g: CostGranularity, scope: CostScope) -> [CostBar] {
        scope == .all ? periods(c, g).map { CostBar(period: $0, usd: $0.usd) } : bars(c, granularity: g, device: c.me)
    }

    /// 一台设备的柱子（本机，用量统计页各台设备那一行）：它的 weeks_usd / days_usd（和 weeks / days 一一对应，cca 给的），
    /// 没有这台设备时都是 0。周期（日期、partial）都是账号的
    static func bars(_ c: AccountCost, granularity g: CostGranularity, device d: CostDevice?) -> [CostBar] {
        let values = d.map { g == .week ? $0.weeksUSD : $0.daysUSD } ?? []
        return periods(c, g).enumerated().map { i, p in CostBar(period: p, usd: i < values.count ? values[i] : 0) }
    }

    /// 统计：整个账号的 summary，或者这台设备的（没有记录时都是 0、没有平均）
    static func summary(_ c: AccountCost, scope: CostScope) -> CostSummary {
        scope == .all ? c.summary : c.me?.summary ?? CostSummary()
    }

    /// 横轴的范围：最右边是最晚结束的那一项（这一周、今天的结束时间）和现在里晚的那个，往左 13 周 / 30 天。
    /// 用量统计页所有行用同一个（上下对得齐），和每周用量的图一样按时间排，不按第几项排。
    static func domain(_ lists: [[CostPeriod]], granularity g: CostGranularity, now: Double) -> ClosedRange<Double> {
        let right = max(now, lists.joined().map(\.endEpoch).max() ?? now)
        return (right - (g == .week ? weekSpan : daySpan))...right
    }

    /// 一项在横轴上占的那一段（只算看得见的部分；不到 1 点宽的不画）
    static func slot(start: Double, end: Double, domain: ClosedRange<Double>, width: Double) -> (x0: Double, x1: Double)? {
        let span = domain.upperBound - domain.lowerBound
        guard span > 0, width > 0 else { return nil }
        func x(_ t: Double) -> Double { (t - domain.lowerBound) / span * width }
        let x0 = max(x(start), 0), x1 = min(x(end), width)
        return x1 - x0 >= 1 ? (x0, x1) : nil
    }

    /// 按天看时隔几天写一个日期：一天的宽度乘它至少要放得下一个日期（1、2、7、14 天里最小的那个）
    static func dayLabelStride(slotWidth: Double) -> Int {
        for k in [1, 2, 7, 14] where slotWidth * Double(k) >= dayLabelMinWidth { return k }
        return 14
    }

    /// 第 index 根（从 0 数，一共 count 根）下面写不写日期：从最后一根（今天）往前数，每 stride 根写一个
    static func showsDayLabel(index: Int, count: Int, stride: Int) -> Bool {
        guard index >= 0, index < count, stride > 0 else { return false }
        return (count - 1 - index) % stride == 0
    }

    /// 用量统计页表头的日期（额度用量、折合金额两种表头共用，和各行的小图同一个时间范围）：从今天往前每隔几周（按天看时每隔几天）一个。
    /// 位置按时间算，挨着两头的往里挪到 edge（字不出这一栏）；离右边那个不到 dayLabelMinWidth 的不写。按周隔 2、3、4、6 周，
    /// 按天隔 7、14 天，取最密、又一个都不用跳过的（今天常常挨着右边、往里挪了，只看一周多宽的话它左边那个会被跳过，隔得不匀）；
    /// 都要跳过时用最稀的。按周从现在往前数；按天从今天那一天的正中间往前数（和下面每天的柱子对齐：横轴的最右边是今天结束的时候）。
    /// 从右往左排，第一个是今天
    static func statsTicks(domain: ClosedRange<Double>, now: Double, width: Double, granularity: CostGranularity = .week,
                           edge: Double = 18) -> [StatsTick] {
        let span = domain.upperBound - domain.lowerBound
        guard span > 0, span.isFinite, now.isFinite, width > 2 * edge else { return [] }
        let start = granularity == .week ? now : domain.upperBound - day / 2
        let strides = granularity == .week ? [2, 3, 4, 6].map { Double($0) * week } : [7, 14].map { Double($0) * day }
        var out: [StatsTick] = []
        for step in strides {
            out = []
            var t = start, skipped = false
            while t >= domain.lowerBound {
                let x = min(max((t - domain.lowerBound) / span * width, edge), width - edge)
                if out.last.map({ $0.x - x >= dayLabelMinWidth }) ?? true {
                    out.append(StatsTick(epoch: t, x: x))
                } else {
                    skipped = true
                }
                t -= step
            }
            if !skipped { break }
        }
        return out
    }

    /// 用量统计页一行下面要不要列各台设备（看所有设备时）：有记录的设备至少两台（只有一台时和整个账号一样）
    static func listsDevices(_ c: AccountCost?) -> Bool {
        (c?.devices.count ?? 0) >= 2
    }
}
