import CcaRemoteKit
import SwiftUI

/// 「折合金额」（Mac App 的 CostStatsPage）：按官方 API 的价格，把各台电脑上每个账号用掉的 token 折成美元放在一起比。不是实际花费：
/// 订阅不按这个收费。和用量统计分开两页：钱没有额度、没有“填满”，图不一样（不画底槽，按纵轴的刻度比高低）。顶上是所有账号加起来的
/// （按周：本周、平均每周、近 12 周合计；按日：今天、平均每天、近 30 天合计和最近 30 天每天一根柱子；有两种以上工具时分开列），
/// 下面每个账号一行（几台电脑上都有的只列一次）：名字、平均、合计，最近 12 周每周一根（或者最近 30 天每天一根）的小柱子（所有行同一个
/// 时间范围、同一个纵轴）和本周（今天）；用过它的设备有两台以上时下面缩进列出各台设备。点一行打开账号页的「折合金额」。
/// 数字都是电脑上的 cca 给的（各台设备之间经 Team 的服务器同步过），这里只把几个账号的加起来、排版
struct CostStatsView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        TimelineView(.periodic(from: .now, by: 60)) { ctx in
            let now = ctx.date.timeIntervalSince1970
            let days = model.costDays
            let all = model.hostedAccounts()
            let rows = Pages.costRows(all)
            let hosts = Self.hosts(all)
            ScrollView {
                VStack(alignment: .leading, spacing: 22) {
                    VStack(alignment: .leading, spacing: 12) {
                        Text(Self.subtitle(hosts, now: now))
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                        if !rows.isEmpty {
                            HStack { Spacer(); CostGranularityPicker() }
                        }
                    }
                    if model.fetchedAt == nil, all.isEmpty {
                        HStack(spacing: 8) { ProgressView(); Text("正在查询…").foregroundStyle(.secondary) }
                    } else if rows.isEmpty {
                        Text(hosts.isEmpty
                             ? "电脑上的 cca 还没有上传折合金额：更新到最新版、统计一次后就会显示（电脑上关闭了折合金额时不显示）。"
                             : "还没有 Claude、ChatGPT 账号。第三方 API 账号按实际用量付费，不在这里折算。")
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    } else {
                        let costs = rows.compactMap(\.account.cost)
                        VStack(alignment: .leading, spacing: 10) {
                            CostTotalTile(rows: rows, days: days, now: now)
                            if hosts.contains(where: \.scanning) {
                                HStack(spacing: 8) {
                                    ProgressView()
                                    Text("电脑正在统计：首次需要读取全部会话记录，可能需要一些时间。")
                                }
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            }
                            let orphan = hosts.reduce(0) { $0 + $1.unattributed }
                            if orphan > 0 {
                                Label("近 30 天另有 \(Texts.money(cents: orphan)) 无法识别属于哪个账号，未计入。", systemImage: "info.circle")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                            if rows.contains(where: { $0.host.costLeftOut }) {
                                Label("有电脑上的账号较多，折合金额没有上传（请在电脑上的 cca App 中查看）。", systemImage: "info.circle")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                    .fixedSize(horizontal: false, vertical: true)
                            } else if hosts.contains(where: \.brief) {
                                Label("有电脑上的账号较多，按设备的金额没有上传（请在电脑上的 cca App 中查看）。", systemImage: "info.circle")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                            Text(Texts.costFootnote(Pages.groups(rows).map(\.tool), pricesDate: hosts.map(\.pricesDate).max()))
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        let lists = costs.map { $0.periods(days: days) }
                        let scale = CostScale(days: days, domain: CostChart.domain(lists, days: days, now: now),
                                              top: CostChart.niceTop(CostChart.maxUSD(lists)), machines: Pages.machines(rows) > 1)
                        VStack(alignment: .leading, spacing: 8) {
                            CostHeader(scale: scale, now: now)
                            ForEach(Pages.groups(rows)) { g in
                                CostGroup(group: g, scale: scale, now: now)
                            }
                        }
                        UploadedLine(now: now)
                    }
                    Text("每个账号一行，一根竖条代表一周（或一天），颜色较浅的表示那一项开始时还没有记录；用过多台设备的账号会分设备列出。点击一行查看这个账号的柱状图。")
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
        .navigationTitle("折合金额")
        .refreshable { await model.fetch() }
        .toolbar { PageToolbar(mark: { CostMark(size: 22) }) }
    }

    /// 开着折合金额的电脑放上来的那几样（每台一份）
    static func hosts(_ all: [HostedAccount]) -> [HostCost] {
        var seen = Set<String>()
        return all.compactMap { r in seen.insert(r.peer.pid).inserted ? r.host.cost : nil }
    }

    /// 标题下面那一句：数字是什么、多久以前统计的（几台电脑时说最近的那一次；Mac App 的 costFreshness：都还没统计过时照实说）
    static func subtitle(_ hosts: [HostCost], now: Double) -> String {
        let what = "按官方 API 价格把每个账号用掉的 token 折算成美元，不是实际花费。"
        if hosts.contains(where: \.scanning) { return what + "电脑正在统计…" }
        guard let at = hosts.compactMap(\.updatedEpoch).max() else { return what + (hosts.isEmpty ? "" : "电脑上还没有统计过。") }
        return what + "电脑上" + Texts.agoPhrase(now - at) + "统计。"
    }
}

/// 所有行共用的：按周还是按天，小图的坐标（同一段时间，上下对得齐；同一个纵轴，各行的高低才能比）
struct CostScale {
    let days: Bool
    let domain: ClosedRange<Double>
    let top: Double
    var machines = false
}

/// 顶上的合计：三个数（按周：本周、平均每周、近 12 周合计；按日：今天、平均每天、近 30 天合计；有两种以上工具时下面按工具分开列），
/// 按日时再下面是最近 30 天每天的柱子（各账号加起来；各账号的周期不一样，所以按周时没有加起来的柱子）。点一天，下面那一行换成那一天的
private struct CostTotalTile: View {
    let rows: [HostedAccount]
    let days: Bool
    let now: Double
    @State private var picked: Double?

    private var titles: [String] { days ? ["今天", "平均每天", "近 30 天合计"] : ["本周", "平均每周", "近 12 周合计"] }

    /// 几个账号加起来的三个数（美分；平均都算不出来时是 nil）
    private func figures(_ costs: [AccountCost]) -> [Int?] {
        let avg = costs.compactMap { $0.average(days: days) }
        return [costs.reduce(0) { $0 + $1.current(days: days) }, avg.isEmpty ? nil : avg.reduce(0, +),
                costs.reduce(0) { $0 + $1.total(days: days) }]
    }

    var body: some View {
        let costs = rows.compactMap(\.account.cost)
        let groups = Pages.groups(rows).filter { g in g.rows.contains { $0.account.cost != nil } }
        let bars = days ? CostChart.sumDays(costs.map(\.days)) : []
        let hit = bars.first { $0.id == picked }
        VStack(alignment: .leading, spacing: 14) {
            Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 14, verticalSpacing: 6) {
                GridRow {
                    if groups.count > 1 { Color.clear.gridCellUnsizedAxes([.horizontal, .vertical]) }
                    ForEach(titles, id: \.self) { Text($0).font(.caption).foregroundStyle(.secondary).accessibilityHidden(true) }
                }
                GridRow {
                    if groups.count > 1 { Text("合计").font(.callout).foregroundStyle(.secondary).accessibilityHidden(true) }
                    let vs = figures(costs)
                    ForEach(titles.indices, id: \.self) { i in
                        let v = vs[i].map { Texts.money(cents: $0) } ?? "—"
                        Text(v)
                            .font(.title3.weight(.semibold).monospacedDigit())
                            .lineLimit(1)
                            .minimumScaleFactor(0.7)
                            .accessibilityLabel(titles[i])
                            .accessibilityValue(v)
                    }
                }
                if groups.count > 1 {
                    ForEach(groups) { g in
                        GridRow {
                            Text(Texts.toolName(g.tool)).font(.callout).foregroundStyle(.secondary).lineLimit(1).minimumScaleFactor(0.7)
                                .accessibilityHidden(true)
                            let vs = figures(g.rows.compactMap(\.account.cost))
                            ForEach(titles.indices, id: \.self) { i in
                                let v = vs[i].map { Texts.money(cents: $0) } ?? "—"
                                Text(v)
                                    .font(.callout.monospacedDigit())
                                    .foregroundStyle(.secondary)
                                    .lineLimit(1)
                                    .minimumScaleFactor(0.7)
                                    .accessibilityLabel(Texts.toolName(g.tool) + " " + titles[i])
                                    .accessibilityValue(v)
                            }
                        }
                    }
                }
            }
            if bars.contains(where: { $0.cents > 0 }) {
                VStack(alignment: .leading, spacing: 8) {
                    CostBarChart(bars: bars, days: true, domain: CostChart.domain([bars], days: true, now: now),
                                 top: CostChart.niceTop(CostChart.maxUSD([bars])), now: now, picked: $picked)
                        .frame(height: 104)
                    Text(hit.map { Texts.costBarDetail($0, days: true, now: now) } ?? "点一根柱子查看那一天的金额。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("所有账号的折合金额")
    }
}

/// 表头（Mac App 的 CostStatsHeader：账号、时间刻度、平均、本周、合计）：两层，上面「账号」「平均每周 · 合计」对着名字和右上那两个数，
/// 下面是小图的时间刻度（按日时每隔几天一个日期）和「本周」（「今天」）
private struct CostHeader: View {
    let scale: CostScale
    let now: Double

    var body: some View {
        StatsHeaderLayers(right: scale.days ? "平均每天 · 合计" : "平均每周 · 合计") {
            // 按天看时最右边一根就是今天，右边那一栏写着「今天」：刻度上不再写一遍
            StatsTimeScale(domain: scale.domain, now: now, days: scale.days, today: !scale.days).frame(maxWidth: .infinity)
            Text(scale.days ? "今天" : "本周").frame(width: CostColumns.current, alignment: .trailing)
        }
    }
}

/// 金额那一栏比百分比宽
enum CostColumns {
    static let current: CGFloat = 70
}

/// 一组（Claude / ChatGPT / Antigravity）：组名和一块底色上的几行；用过它的设备有两台以上的账号下面紧跟着各台设备
private struct CostGroup: View {
    let group: HostedGroup
    let scale: CostScale
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
                    CostRow(row: r, scale: scale, now: now)
                }
            }
            .padding(.vertical, 4)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
        }
    }
}

/// 一个账号一行：点了进账号页的「折合金额」。数字没算全（同步出错、有设备没算进来、很久没同步上来）时名字后面一个橙色的三角
private struct CostRow: View {
    let row: HostedAccount
    let scale: CostScale
    let now: Double

    var body: some View {
        let a = row.account
        let c = a.cost
        let notes = c.map { Texts.costNotes($0, now: now) } ?? []
        let caption = c.map { Texts.costCaption($0, days: scale.days) } ?? (row.host.costLeftOut ? "没有上传" : "还没有记录")
        NavigationLink(value: row.ref) {
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .center, spacing: 10) {
                    AccountAvatar(account: a, size: StatsColumns.avatar)
                    AccountName(row: row, machines: scale.machines, warn: notes.contains(where: \.warning))
                    Spacer(minLength: 6)
                    figures(c)
                }
                HStack(spacing: StatsColumns.spacing) {
                    chart(c)
                        .frame(maxWidth: .infinity)
                        .frame(height: 34)
                    Text(c.map { Texts.money(cents: $0.current(days: scale.days)) } ?? "—")
                        .font(.callout.monospacedDigit())
                        .foregroundStyle(c == nil ? .secondary : .primary)
                        .lineLimit(1)
                        .minimumScaleFactor(0.8)
                        .frame(width: CostColumns.current, alignment: .trailing)
                }
                .padding(.leading, StatsColumns.lead)
                if let c, c.devices.count >= 2 {
                    VStack(alignment: .leading, spacing: 4) {
                        ForEach(Array(c.devices.enumerated()), id: \.offset) { _, d in
                            CostDeviceLine(device: d, days: scale.days, now: now)
                        }
                    }
                    .padding(.leading, StatsColumns.lead)
                }
                // 名字后面的橙色三角是为什么（Mac App 鼠标指着时的提示里一条条列着；手机上没有指着，直接写在下面）
                let warnings = notes.filter(\.warning)
                if !warnings.isEmpty {
                    VStack(alignment: .leading, spacing: 3) {
                        ForEach(Array(warnings.enumerated()), id: \.offset) { _, n in CostNoteLine(note: n) }
                    }
                    .padding(.leading, StatsColumns.lead)
                }
            }
            .padding(.horizontal, StatsColumns.inset)
            .padding(.vertical, 10)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Texts.nonEmpty(a.name) ?? "账号")
        .accessibilityValue(([caption] + notes.map(\.text)).joined(separator: "；"))
        .accessibilityHint("打开这个账号的折合金额")
    }

    /// 右上：平均（加粗），下面一行小字是合计
    @ViewBuilder
    private func figures(_ c: AccountCost?) -> some View {
        if let c {
            VStack(alignment: .trailing, spacing: 2) {
                Text("\(Text("平均 ").foregroundStyle(.secondary))\(Text(c.average(days: scale.days).map { Texts.money(cents: $0) } ?? "—").monospacedDigit().fontWeight(.semibold))")
                    .font(.callout)
                Text((scale.days ? "30 天合计 " : "12 周合计 ") + Texts.money(cents: c.total(days: scale.days)))
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        } else {
            Text("—").font(.callout).foregroundStyle(.secondary)
        }
    }

    @ViewBuilder
    private func chart(_ c: AccountCost?) -> some View {
        if let c {
            let bars = c.periods(days: scale.days)
            if bars.contains(where: { $0.cents > 0 }) {
                CostBarChart(bars: bars, days: scale.days, domain: scale.domain, top: scale.top, compact: true, now: now)
            } else {
                note(scale.days ? "最近 30 天没有用量" : "最近 12 周没有用量")
            }
        } else {
            note(row.host.costLeftOut ? "电脑上的账号较多，没有上传" : "还没有记录")
        }
    }

    private func note(_ text: String) -> some View {
        Text(text).font(.caption).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// 用过这个账号的一台设备（cca 排好的：放上来的那台在前）：缩进、灰字，金额是它近 12 周（近 30 天）的；很久没同步上来的名字后面一个橙色的钟
struct CostDeviceLine: View {
    let device: CostDevice
    let days: Bool
    let now: Double

    var body: some View {
        let d = device
        let stale = Texts.costStale(d, now: now)
        HStack(spacing: 5) {
            CostDeviceName(device: d)
            if stale != nil {
                Image(systemName: "clock.badge.exclamationmark").foregroundStyle(.orange)
            }
            Spacer(minLength: 6)
            Text(Texts.money(cents: days ? d.days : d.weeks)).monospacedDigit()
        }
        .font(.caption)
        .foregroundStyle(.secondary)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Texts.costDeviceName(d))
        .accessibilityValue(((days ? "近 30 天 " : "近 12 周 ") + Texts.money(cents: days ? d.days : d.weeks)) + (stale.map { "；" + $0 } ?? ""))
    }
}
