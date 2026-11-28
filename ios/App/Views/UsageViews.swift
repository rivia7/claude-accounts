import CcaRemoteKit
import SwiftUI

// 用量的几样（照 Mac App 的 UsageCard.swift、WeeklyUsage.swift 搬过来，手机上一行两块）：额度的大块、Antigravity 的几组、列表里的小条、
// 每周用量的图。数据都是那台电脑放上来的（Status.swift），这里只照着画

/// 额度的一大块（Mac App 的 QuotaTile）：名字、百分比、用量条、什么时候重置，下面可能还有一句（分模型的额度占本周的多少）
struct QuotaTile: View {
    let title: String
    let window: UsageWindow?
    let now: Double
    var dim = false
    var note: String?

    var body: some View {
        let reset = window?.isReset(now: now) ?? false
        let pct = window?.utilization
        let color = Palette.usage(percent: pct ?? 0)
        let resetText = reset ? nil : window?.resetsEpoch.map { Texts.resetText(epoch: $0, now: now) }
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.subheadline.weight(.semibold)).foregroundStyle(.secondary).lineLimit(1)
            Group {
                if let pct, !reset {
                    HStack(alignment: .firstTextBaseline, spacing: 1) {
                        Text("\(Int(pct.rounded()))").font(.system(size: 28, weight: .semibold).monospacedDigit())
                        Text("%").font(.system(size: 15, weight: .semibold))
                    }
                    .foregroundStyle(dim ? AnyShapeStyle(.secondary) : pct < 70 ? AnyShapeStyle(.primary) : AnyShapeStyle(color))
                } else {
                    Text(reset ? "已重置" : "—")
                        .font(.system(size: 20, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .frame(height: 34, alignment: .bottom)
                }
            }
            Meter(fraction: reset ? 0 : (pct ?? 0) / 100, color: color, height: 6, dim: dim)
            if let resetText {
                Text(resetText).font(.caption).foregroundStyle(.secondary).lineLimit(1).monospacedDigit()
            }
            // 过了重置时间，那句话（已占多少、用完了）是旧的：等下次查到再说
            if !reset, let note = Texts.nonEmpty(note) {
                Text(note).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(title)用量")
        .accessibilityValue(pct == nil ? "暂无数据" : reset ? "已重置" : Texts.percent(pct ?? 0) + (resetText.map { "，" + $0 } ?? ""))
    }
}

/// 一行两块，单出来的最后一块占满一行；同一行的一样高
struct TileRows<Item, Tile: View>: View {
    let items: [Item]
    @ViewBuilder let tile: (Item) -> Tile

    var body: some View {
        VStack(spacing: 10) {
            ForEach(Array(stride(from: 0, to: items.count, by: 2)), id: \.self) { i in
                HStack(alignment: .top, spacing: 10) {
                    tile(items[i])
                    if i + 1 < items.count { tile(items[i + 1]) }
                }
                .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

/// 账号页的「用量」：5 小时、本周、分模型的每周额度（Antigravity 是几组额度），下面是还能重置几次、这次没查成的原因
struct UsageSection: View {
    let account: AccountStatus
    let now: Double

    private struct Item {
        let title: String
        let window: UsageWindow?
        var note: String?
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline) {
                Text("用量").font(.headline)
                Spacer(minLength: 8)
                if let u = account.usage, u.source != "none" {
                    let f = Texts.freshness(u, now: now)
                    HStack(spacing: 5) {
                        if u.stale {
                            Image(systemName: "clock.badge.exclamationmark").foregroundStyle(.orange)
                        } else if f.fresh {
                            Circle().fill(.green).frame(width: 6, height: 6)
                        }
                        Text(f.text)
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .accessibilityElement(children: .combine)
                }
            }
            content
            rows
        }
    }

    @ViewBuilder
    private var content: some View {
        let a = account
        if let u = a.usage, u.hasQuota {
            QuotaGroupsView(usage: u, now: now, dim: dim(u))
        } else if let u = a.usage, u.source != "none" {
            TileRows(items: items(u)) { QuotaTile(title: $0.title, window: $0.window, now: now, dim: dim(u), note: $0.note) }
        } else {
            Text(emptyReason)
                .font(.callout)
                .foregroundStyle(.secondary)
                .padding(14)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
        }
    }

    @ViewBuilder
    private var rows: some View {
        if let u = account.usage, u.source != "none", let rc = u.resetCredits, rc.availableCount != nil {
            Label(Texts.resetCredits(rc, now: now), systemImage: "ticket").font(.footnote).foregroundStyle(.secondary)
        }
        if let code = account.usage?.errorCode {
            Label(Texts.usageError(code), systemImage: "exclamationmark.triangle.fill")
                .font(.footnote)
                .foregroundStyle(.orange)
        }
    }

    /// 数据旧了（过时了，或者这次没查到、用的是缓存）用灰色画
    private func dim(_ u: AccountUsage) -> Bool {
        u.stale || (u.source == "cache" && u.errorCode != nil)
    }

    private func items(_ u: AccountUsage) -> [Item] {
        var out: [Item] = []
        if !u.noFiveHourLimit { out.append(Item(title: "5 小时", window: u.fiveHour)) }   // 没有 5 小时限制的套餐不画这一块
        out.append(Item(title: "本周", window: u.sevenDay))
        var seen = Set<String>()
        for m in u.modelScoped where !m.name.isEmpty && m.window.utilization != nil && seen.insert(m.name.lowercased()).inserted {
            out.append(Item(title: m.name, window: m.window, note: Texts.modelShare(m, weekly: u.sevenDay)))
        }
        return out
    }

    private var emptyReason: String {
        let a = account
        if a.isAPI { return "第三方 API 账号没有订阅额度。" }
        switch a.login {
        case "logged_out": return "未登录，无法查询用量。"
        case "email_mismatch": return "登录的不是这个账号，无法查询用量。"
        default:
            if a.usage?.errorCode == "auth" { return "登录已失效，无法查询用量。" }
            return "暂无用量数据：电脑上的 cca 查询一次用量后就会显示。"
        }
    }
}

/// Antigravity 的几组额度（Mac App 的 QuotaGroupsView）：一组一段，组名下面是它的几个额度
struct QuotaGroupsView: View {
    let usage: AccountUsage
    let now: Double
    var dim = false

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            ForEach(Array(usage.quotaGroups.enumerated()), id: \.offset) { _, g in
                let tiles = Self.tiles(g)
                let solo = Self.solo(g)
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 8) {
                        Text(g.displayName).font(.callout.weight(.semibold))
                        if let solo { Text(solo).font(.callout).foregroundStyle(.secondary) }
                    }
                    if !tiles.isEmpty {
                        TileRows(items: tiles) { QuotaTile(title: $0.title, window: $0.bucket.used, now: now, dim: dim) }
                    }
                    if solo == nil {
                        ForEach(Array(Self.minor(g).enumerated()), id: \.offset) { _, m in
                            HStack {
                                Text(m.title).foregroundStyle(.secondary)
                                Spacer()
                                Text(m.value)
                            }
                            .font(.footnote)
                        }
                    }
                }
                .accessibilityElement(children: .contain)
            }
        }
    }

    /// 有百分比的额度：5 小时在前、本周其次，认不出窗口的照 agy 的顺序排在后面
    static func tiles(_ g: QuotaGroup) -> [(title: String, bucket: QuotaBucket)] {
        let shown = g.buckets.filter { !$0.disabled && ($0.used.utilization != nil || $0.used.reset) }
        func rank(_ b: QuotaBucket) -> Int { b.slot.map { $0 == .fiveHour ? 0 : 1 } ?? 2 }
        return shown.enumerated().sorted { (rank($0.element), $0.offset) < (rank($1.element), $1.offset) }
            .map { (title($0.element, in: g), $0.element) }
    }

    /// 认出窗口的叫 5 小时、本周；认不出的叫“其他额度”，一组里有几个时编上号
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

    static func minor(_ g: QuotaGroup) -> [(title: String, value: String)] {
        g.buckets.compactMap { b in minorValue(b).map { (title(b, in: g), $0) } }
    }

    /// 一组只有一个额度、认不出窗口、没有百分比：直接写在组名后面
    static func solo(_ g: QuotaGroup) -> String? {
        guard g.buckets.count == 1, let b = g.buckets.first, b.slot == nil else { return nil }
        return minorValue(b)
    }
}

/// 列表里一个账号右边的几行小用量（Mac App 的 UsageStack、QuotaStack，一行是它的 MiniUsage）：5 小时、本周，分模型的每周额度
/// （Claude 的 Fable）也各一行，和账号页的大块一一对应；没有 5 小时限制的套餐不画 5 小时那一行。还没有数据写「—」，过了重置时间写
/// 「已重置」；百分比用量正常时是正文的颜色、到了 70% 是用量条的颜色，数据旧了是灰的。Antigravity 写各组里用得最多的那个（读屏说是
/// 哪一组），窗口都认不出时写用得最多的那个额度
struct MiniUsage: View {
    let usage: AccountUsage
    let now: Double

    var body: some View {
        VStack(alignment: .trailing, spacing: 4) {
            ForEach(Array(Self.rows(usage, now: now).enumerated()), id: \.offset) { _, r in
                line(r)
            }
        }
    }

    private func line(_ r: Row) -> some View {
        let reset = r.window?.isReset(now: now) ?? false
        let pct = r.window?.utilization
        let value = pct.map { reset ? "已重置" : Texts.percent($0) } ?? "—"
        let style: Color = {
            guard let p = pct, !reset, !usage.stale else { return .secondary }
            return p < 70 ? .primary : Palette.usage(percent: p)
        }()
        // 分模型的额度和本周总额度的关系、是哪一组：过了重置时间就不说（那是旧的）
        let note = reset ? nil : r.note
        return HStack(spacing: 5) {
            Text(r.label).font(.caption2).foregroundStyle(.secondary)
                .lineLimit(1).minimumScaleFactor(0.7)
                .frame(maxWidth: r.named ? 44 : nil, alignment: .trailing)
            Meter(fraction: reset ? 0 : (pct ?? 0) / 100, color: Palette.usage(percent: pct ?? 0), height: 4, dim: usage.stale)
                .frame(width: 30)
            Text(value).font(.caption2.weight(.semibold).monospacedDigit()).foregroundStyle(style).fixedSize()
                .frame(minWidth: 34, alignment: .trailing)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(r.spoken)
        .accessibilityValue((pct == nil ? "暂无数据" : value) + (note.map { "，" + $0 } ?? ""))
    }

    struct Row {
        let label: String
        /// 读屏念的名字（「5 小时用量」）
        let spoken: String
        let window: UsageWindow?
        /// 读屏接在百分比后面念的：分模型的额度和本周总额度的关系（Texts.modelShare）、Antigravity 用得最多的是哪一组
        var note: String?
        /// 标签是模型名（比「5h」「周」宽：放不下时先缩小、再截断）
        var named = false
    }

    static func rows(_ u: AccountUsage, now: Double) -> [Row] {
        guard u.source != "none" else { return [] }
        if u.hasQuota { return quotaRows(u, now: now) }
        var out: [Row] = []
        if !u.noFiveHourLimit { out.append(Row(label: "5h", spoken: "5 小时用量", window: u.fiveHour)) }
        out.append(Row(label: "周", spoken: "本周用量", window: u.sevenDay))
        // 按名字去重，没有百分比的不要（和账号页的大块、Mac App 的 modelQuotas 一样）
        var seen = Set<String>()
        for m in u.modelScoped where !m.name.isEmpty && m.window.utilization != nil && seen.insert(m.name.lowercased()).inserted {
            out.append(Row(label: m.name, spoken: "\(m.name) 用量", window: m.window, note: Texts.modelShare(m, weekly: u.sevenDay),
                           named: true))
        }
        return out
    }

    /// Antigravity（Mac App 的 QuotaStack）：5 小时、本周各一行，写各组里用得最多的那个；窗口都认不出时写用得最多的那个额度（停用的、
    /// 没有百分比的不算）
    private static func quotaRows(_ u: AccountUsage, now: Double) -> [Row] {
        let named: [(QuotaSlot, String, String)] = [(.fiveHour, "5h", "5 小时"), (.sevenDay, "周", "本周")]
        let rows = named.compactMap { slot, label, spoken -> Row? in
            guard let m = u.quotaMost(slot, now: now) else { return nil }
            let several = Set(u.quotaBuckets(slot).map(\.group)).count > 1
            return Row(label: label, spoken: spoken + "用量", window: m.window, note: several ? "用得最多的是 \(m.group)" : nil)
        }
        if !rows.isEmpty { return rows }
        let all = u.quotaGroups.flatMap { g in g.buckets.filter { !$0.disabled && $0.used.utilization != nil }.map { (g, $0) } }
        guard let (g, b) = all.max(by: { ($0.1.used.utilization ?? 0) < ($1.1.used.utilization ?? 0) }) else { return [] }
        return [Row(label: "额度", spoken: "额度用量", window: b.used, note: g.displayName)]
    }
}

/// 账号页的「每周用量」（Mac App 的 WeeklyUsageSection）：最近 12 周每周用到这一周额度的百分之几（cca 每次查用量时记下的）。
/// 点一根柱子，下面那一行说那一周的细节
struct WeeklySection: View {
    let weeks: UsageWeeks?
    /// 那台电脑的账号太多，没把每周用量放上来
    var leftOut = false
    let now: Double
    @State private var picked: Double?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline) {
                Text("每周用量").font(.headline)
                Spacer(minLength: 8)
                if let since = weeks?.sinceEpoch {
                    Text(Texts.weeksSince(since, now: now)).font(.caption).foregroundStyle(.secondary)
                }
            }
            if let uw = weeks, !uw.weeks.isEmpty {
                let hit = uw.weeks.first { $0.endEpoch == picked }
                VStack(alignment: .leading, spacing: 10) {
                    WeeklyChart(weeks: uw.weeks, now: now, picked: $picked)
                        .frame(height: 128)
                    Text(hit.map { Texts.weekDetail($0, now: now) } ?? Texts.weeksSummary(uw.summary) ?? "这一周过完就有第一周的统计。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    if uw.weeks.contains(where: \.partial) {
                        HStack(spacing: 6) {
                            WeekBar(percent: 70, top: 100, color: Meter.dimColor).frame(width: 5, height: 12).accessibilityHidden(true)
                            Text("灰色：那一周最后一天没有数据，实际可能更多。").fixedSize(horizontal: false, vertical: true)
                        }
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    }
                }
                .padding(14)
                .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
            } else {
                Text(weeks != nil ? "最近 12 周没有记录。"
                    : leftOut ? "电脑上的账号较多，每周用量没有上传（请在电脑上的 cca App 中查看）。"
                    : "还没有记录：电脑每次查询用量时会记录本周的用量。")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .padding(14)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
            }
        }
    }
}

/// 每周用量的图（Mac App 的 WeeklyChart）：一周一根竖着的用量条，底槽是这一周的额度（100%），按用了多少上色（和额度的条同一套），
/// 结束前一天没查到的填灰色；超过 100% 的（中间重置过）整根高出底槽。横轴是时间：最右边是这一周的重置时间和现在里晚的那个，往左 13 周。
/// compact 是用量统计页的小图：条细一点，不写日期，不能点；横轴的范围、纵轴的高由用的地方给（统计页所有行同一个，上下对得齐、高低能比）
struct WeeklyChart: View {
    let weeks: [UsageWeek]
    var domain: ClosedRange<Double>?
    let now: Double
    var compact = false
    /// 纵轴最高到多少（百分比，至少 100）；不给就按这几周自己的
    var top: Double?
    /// 点选的那一周（它的 endEpoch）；给了才能点
    var picked: Binding<Double?>?

    static let week: Double = 7 * 86400

    /// 横轴的范围：最右边是最晚结束的那一周（还没过完的这一周的重置时间）和现在里晚的那个，往左 13 周
    static func domain(_ lists: [[UsageWeek]], now: Double) -> ClosedRange<Double> {
        let right = max(now, lists.joined().map(\.endEpoch).max() ?? now)
        return (right - 13 * week)...right
    }

    /// 纵轴最高到多少：100%，有超过 100% 的（中间重置过）就到最高的那一周
    static func scaleTop(_ lists: [[UsageWeek]]) -> Double {
        max(100, lists.joined().map(\.usedPercent).max() ?? 0)
    }

    var body: some View {
        let top = self.top ?? Self.scaleTop([weeks])
        SlotChart(items: weeks, domain: domain ?? Self.domain([weeks], now: now), barWidth: compact ? 5 : 10, topInset: compact ? 0 : 6,
                  dateRow: compact ? 0 : 18, picked: picked, label: { w, p in CostChart.weekLabel(Texts.weekLabel(w), p) },
                  accessibility: { w in (Texts.weekLabel(w), Texts.weekDetail(w, now: now)) }) { w in
            WeekBar(percent: w.usedPercent, top: top, color: w.partial ? Meter.dimColor : Palette.usage(percent: w.usedPercent))
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("每周用量")
    }
}

/// 一周的条：竖着的用量条（Mac App 的 WeekBar）。底槽是这一周的额度（100%），高是 100 / top；从下往上填
struct WeekBar: View {
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
