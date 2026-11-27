import CcaRemoteKit
import SwiftUI

/// 「用量统计」（Mac App 的 UsageStatsPage）：各台电脑上的官方账号每周用了多少放在一起比，Claude、ChatGPT 各一组（第三方 API 账号
/// 没有订阅的额度，Antigravity 没有每周用量的统计，都不在这里）。几台电脑上都有的账号（同一个 Team）只列一次。每个账号一行：头像和名字、
/// 平均每周用到多少和几周用完 / 没用到一半，下面是最近 12 周的小柱状图（所有行同一个时间范围、同一个纵轴，上下对得齐、高低能比）和本周
/// 到现在。点一行进这个账号的页面。数字都是电脑上的 cca 算好的（usage_weeks），这里只排版
struct UsageStatsView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        TimelineView(.periodic(from: .now, by: 60)) { ctx in
            let now = ctx.date.timeIntervalSince1970
            let all = model.hostedAccounts()
            let rows = Pages.statsRows(all)
            let lists = rows.map { $0.account.weeks?.weeks ?? [] }
            let scale = StatsScale(domain: WeeklyChart.domain(lists, now: now), top: WeeklyChart.scaleTop(lists),
                                   machines: Pages.machines(rows) > 1)
            ScrollView {
                VStack(alignment: .leading, spacing: 22) {
                    Text(Self.subtitle(rows, now: now))
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    if model.fetchedAt == nil, all.isEmpty {
                        HStack(spacing: 8) { ProgressView(); Text("正在查询…").foregroundStyle(.secondary) }
                    } else if rows.isEmpty {
                        Text("电脑上还没有 Claude、ChatGPT 账号（或电脑还没上传账号状态）。第三方 API 账号没有订阅的额度，不在这里统计。")
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    } else {
                        VStack(alignment: .leading, spacing: 8) {
                            StatsHeader(scale: scale, now: now)
                            ForEach(Pages.groups(rows)) { g in
                                StatsGroup(group: g, scale: scale, now: now)
                            }
                        }
                        if rows.contains(where: { $0.host.weeksLeftOut }) {
                            Label("有电脑上的账号较多，每周用量没有上传（请在电脑上的 cca App 中查看）。", systemImage: "info.circle")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    Text("每个账号一行，一根竖条代表一周：填得越满用得越多，填满表示这一周的额度已用完；灰色的那一周最后一天没有数据，实际可能更多。点击一行查看这个账号的详情。")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(.horizontal, 20)
                .padding(.top, 8)
                .padding(.bottom, 30)
                .pageColumn()
            }
        }
        .navigationTitle("用量统计")
        .refreshable { await model.fetch() }
        .toolbar { PageToolbar(mark: { StatsMark(size: 22) }) }
    }

    /// 标题下面那一句：数字从哪来、从什么时候开始记
    static func subtitle(_ rows: [HostedAccount], now: Double) -> String {
        guard let since = rows.compactMap({ $0.account.weeks?.sinceEpoch }).min() else {
            return "还没有记录：电脑上的 cca 每次查询用量时会记录本周的用量，满一周后这里就会显示统计。"
        }
        return "最近 12 周每周用了多少额度。" + Texts.weeksSince(since, now: now) + "，电脑上的 cca 每次查询用量时更新。"
    }
}

/// 所有行的小图共用的：同一段时间（上下对得齐）、同一个纵轴高度（各行的高低才能比）；有几台电脑（两台以上时一行写上是哪台的）
struct StatsScale {
    let domain: ClosedRange<Double>
    let top: Double
    var machines = false
}

/// 一行里几栏有多宽（表头和各组的行共用，上下对齐）
enum StatsColumns {
    /// 右边那一栏：本周（今天）
    static let current: CGFloat = 58
    static let spacing: CGFloat = 10
    static let inset: CGFloat = 14
    /// 头像的宽和它后面的间距：表头的日期、各行的小图从这里开始，和名字对齐
    static let avatar: CGFloat = 28
    static let lead: CGFloat = avatar + 10
}

/// 表头：小图的时间刻度（从今天往前每隔几周一个日期）、本周
private struct StatsHeader: View {
    let scale: StatsScale
    let now: Double

    var body: some View {
        HStack(spacing: StatsColumns.spacing) {
            StatsTimeScale(domain: scale.domain, now: now).frame(maxWidth: .infinity)
            Text("本周").frame(width: StatsColumns.current, alignment: .trailing)
        }
        .font(.caption2)
        .foregroundStyle(.secondary)
        .frame(height: 16)
        .padding(.leading, StatsColumns.inset + StatsColumns.lead)
        .padding(.trailing, StatsColumns.inset)
        .accessibilityHidden(true)
    }
}

/// 一组（Claude / ChatGPT）：组名和一块底色上的几行
private struct StatsGroup: View {
    let group: HostedGroup
    let scale: StatsScale
    let now: Double

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(Texts.department(group.tool))
                .font(.title3.weight(.semibold))
                .accessibilityAddTraits(.isHeader)
                .padding(.top, 10)
            VStack(spacing: 0) {
                ForEach(Array(group.rows.enumerated()), id: \.element.id) { i, r in
                    if i > 0 { Divider().padding(.leading, StatsColumns.inset + StatsColumns.lead) }
                    StatsRow(row: r, scale: scale, now: now)
                }
            }
            .padding(.vertical, 4)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
        }
    }
}

/// 一个账号一行：点了进账号页
private struct StatsRow: View {
    let row: HostedAccount
    let scale: StatsScale
    let now: Double

    var body: some View {
        let a = row.account
        let uw = a.weeks
        let summary = uw.flatMap { Texts.weeksSummary($0.summary) }
        NavigationLink(value: row.ref) {
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .center, spacing: 10) {
                    AccountAvatar(account: a, size: StatsColumns.avatar)
                    AccountName(row: row, machines: scale.machines)
                    Spacer(minLength: 6)
                    average(uw?.summary)
                }
                HStack(spacing: StatsColumns.spacing) {
                    chart(uw)
                        .frame(maxWidth: .infinity)
                        .frame(height: 34)
                    Text(uw?.weeks.last(where: \.current).map { Texts.percent($0.usedPercent) } ?? "—")
                        .font(.callout.monospacedDigit())
                        .foregroundStyle(uw?.weeks.last(where: \.current) == nil ? .secondary : .primary)
                        .frame(width: StatsColumns.current, alignment: .trailing)
                }
                .padding(.leading, StatsColumns.lead)
            }
            .padding(.horizontal, StatsColumns.inset)
            .padding(.vertical, 10)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Texts.nonEmpty(a.name) ?? "账号")
        .accessibilityValue(summary ?? (uw == nil ? "还没有记录" : "最近 12 周没有记录"))
        .accessibilityHint("打开这个账号的页面")
    }

    @ViewBuilder
    private func chart(_ uw: UsageWeeks?) -> some View {
        if let uw, !uw.weeks.isEmpty {
            WeeklyChart(weeks: uw.weeks, domain: scale.domain, now: now, compact: true, top: scale.top)
        } else {
            Text(uw != nil ? "最近 12 周没有记录" : row.host.weeksLeftOut ? "没有上传" : "还没有记录")
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
                Text("\(Text("平均 ").foregroundStyle(.secondary))\(Text(avg).monospacedDigit().fontWeight(.semibold))")
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

/// 一行左边的名字：名字，下面是套餐（和几台电脑时是哪台电脑的；几台电脑上都有的不写）
struct AccountName: View {
    let row: HostedAccount
    let machines: Bool
    var warn = false

    var body: some View {
        let a = row.account
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 5) {
                Text(Texts.nonEmpty(a.name) ?? "账号").font(.body.weight(.medium)).lineLimit(1)
                if warn {
                    Image(systemName: "exclamationmark.triangle.fill").font(.caption).foregroundStyle(.orange)
                }
            }
            HStack(spacing: 6) {
                if !a.plan.isEmpty { PlanBadge(plan: a.plan, tier: a.tier) }
                if machines, !row.shared {
                    Text(row.peer.name).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
            }
        }
    }
}
