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
        .accessibilityValue(pct == nil ? "没有数据" : reset ? "已重置" : Texts.percent(pct ?? 0) + (resetText.map { "，" + $0 } ?? ""))
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
        if a.isAPI { return "第三方 API 账号没有订阅的用量。" }
        switch a.login {
        case "logged_out": return "还没登录，查不了用量。"
        case "email_mismatch": return "登录的不是这个账号，查不了用量。"
        default:
            if a.usage?.errorCode == "auth" { return "登录已失效，查不了用量。" }
            return "还没有用量数据：那台电脑上的 cca 查过一次用量以后就有了。"
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

/// 列表里一个账号右边的几行小用量（Mac App 的 UsageStack）：5 小时、本周，各一个短条和百分比。没有 5 小时限制的套餐不画 5 小时那一行；
/// Antigravity 写各组里用得最多的那个
struct MiniUsage: View {
    let usage: AccountUsage
    let now: Double

    var body: some View {
        let rows = Self.rows(usage, now: now)
        VStack(alignment: .trailing, spacing: 4) {
            ForEach(rows, id: \.label) { r in
                let reset = r.window.isReset(now: now), p = reset ? 0 : r.window.utilization ?? 0
                HStack(spacing: 5) {
                    Text(r.label).font(.caption2).foregroundStyle(.secondary)
                    Meter(fraction: p / 100, color: Palette.usage(percent: p), height: 4, dim: usage.stale).frame(width: 30)
                    Text(Texts.percent(p)).font(.caption2.monospacedDigit()).foregroundStyle(.secondary).fixedSize()
                        .frame(minWidth: 32, alignment: .trailing)
                }
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(r.spoken)
                .accessibilityValue(Texts.percent(p))
            }
        }
    }

    struct Row {
        let label: String
        let spoken: String
        let window: UsageWindow
    }

    static func rows(_ u: AccountUsage, now: Double) -> [Row] {
        guard u.source != "none" else { return [] }
        if u.hasQuota {
            return [(QuotaSlot.fiveHour, "5h", "5 小时用量"), (.sevenDay, "周", "本周用量")].compactMap { slot, label, spoken in
                u.quotaMost(slot, now: now).map { Row(label: label, spoken: spoken, window: $0.window) }
            }
        }
        var out: [Row] = []
        if !u.noFiveHourLimit, let w = u.fiveHour, w.utilization != nil { out.append(Row(label: "5h", spoken: "5 小时用量", window: w)) }
        if let w = u.sevenDay, w.utilization != nil { out.append(Row(label: "周", spoken: "本周用量", window: w)) }
        return out
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
                            Text("填成灰色的：那一周结束前一天没查到，实际可能更多。").fixedSize(horizontal: false, vertical: true)
                        }
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    }
                }
                .padding(14)
                .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
            } else {
                Text(weeks != nil ? "最近 12 周没有记录。"
                    : leftOut ? "那台电脑上的账号太多，每周用量没放上来（在那台电脑的 cca App 里看）。"
                    : "还没有记录：那台电脑每次查用量时会记下这一周用到多少。")
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
/// 结束前一天没查到的填灰色；超过 100% 的（中间重置过）整根高出底槽。横轴是时间：最右边是这一周的重置时间和现在里晚的那个，往左 13 周
struct WeeklyChart: View {
    let weeks: [UsageWeek]
    let now: Double
    @Binding var picked: Double?

    static let week: Double = 7 * 86400

    private struct Slot {
        let week: UsageWeek
        let index: Int
        let mid: CGFloat
        let width: CGFloat
        let x0: CGFloat
        let x1: CGFloat
    }

    var body: some View {
        let right = max(now, weeks.map(\.endEpoch).max() ?? now)
        let left = right - 13 * Self.week
        let top = max(100, weeks.map(\.usedPercent).max() ?? 0)
        GeometryReader { geo in
            let size = geo.size
            let dateRow: CGFloat = 18, inset: CGFloat = 6
            let plot = max(size.height - dateRow - inset, 1)
            let slots = Self.slots(weeks, left: left, right: right, width: size.width)
            // 窄的时候（一周不到 34 点）日期隔一根写一个，从这一周往前数
            let every = (slots.map(\.width).min() ?? 40) < 34 ? 2 : 1
            ZStack(alignment: .topLeading) {
                ForEach(slots, id: \.week.endEpoch) { s in
                    let on = s.week.endEpoch == picked
                    if on {
                        RoundedRectangle(cornerRadius: 8, style: .continuous)
                            .fill(Color.primary.opacity(0.06))
                            .frame(width: max(s.width - 4, 0), height: size.height)
                            .position(x: s.mid, y: size.height / 2)
                    }
                    WeekBar(percent: s.week.usedPercent, top: top,
                            color: s.week.partial ? Meter.dimColor : Palette.usage(percent: s.week.usedPercent))
                        .frame(width: min(10, max(s.width * 0.6, 2)), height: plot)
                        .position(x: s.mid, y: inset + plot / 2)
                        .accessibilityElement()
                        .accessibilityLabel(Texts.weekLabel(s.week))
                        .accessibilityValue(Texts.weekDetail(s.week, now: now))
                    if (slots.count - 1 - s.index) % every == 0, s.width >= 16 {
                        Text(Texts.weekLabel(s.week))
                            .font(.caption2.monospacedDigit())
                            .foregroundStyle(on ? .primary : .secondary)
                            .fixedSize()
                            .position(x: s.mid, y: size.height - dateRow / 2)
                            .accessibilityHidden(true)
                    }
                }
            }
            .frame(width: size.width, height: size.height)
            .contentShape(Rectangle())
            .onTapGesture(coordinateSpace: .local) { p in
                let hit = slots.first { $0.x0 <= p.x && p.x < $0.x1 }?.week.endEpoch
                picked = hit == picked ? nil : hit
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("每周用量")
    }

    /// 每一周在横轴上占哪一段（截到图的范围里）
    private static func slots(_ weeks: [UsageWeek], left: Double, right: Double, width: CGFloat) -> [Slot] {
        let span = max(right - left, 1)
        var out: [Slot] = []
        for w in weeks {
            let a = max(w.startEpoch, left), b = min(w.endEpoch, right)
            guard b > a else { continue }
            let x0 = CGFloat((a - left) / span) * width, x1 = CGFloat((b - left) / span) * width
            out.append(Slot(week: w, index: out.count, mid: (x0 + x1) / 2, width: x1 - x0, x0: x0, x1: x1))
        }
        return out
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
