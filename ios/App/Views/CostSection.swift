import CcaRemoteKit
import SwiftUI

/// 账号页的「折合金额」（Mac App 的 CostSection）：按官方 API 的价格把这个账号用掉的 token 折成美元（不是实际花费，几台设备加起来的）。
/// 按周还是按天（和折合金额页同一个设置），一张柱状图（右边是纵轴的刻度；点一根柱子，下面那一行换成那一项的金额），图下面一行是
/// 本周 / 平均 / 合计 / 缓存命中；开始时还没有记录的那一项颜色浅（图例）；按模型的（cca 排在前面的几个）、各台设备的（两台以上时）；
/// 同步的状态。按谁的价格折算、价格表的日期、多久以前统计的一直写在最下面
struct CostSection: View {
    @Environment(AppModel.self) private var model
    let account: AccountStatus
    let host: HostStatus?
    let now: Double
    @State private var picked: Double?

    /// 这个账号的那一页有没有这一节：有数据；或者那台电脑开着折合金额（官方账号），只是没放上来、还没有记录
    static func shows(_ a: AccountStatus, host: HostStatus?) -> Bool {
        a.cost != nil || (!a.isAPI && (host?.cost != nil || host?.costLeftOut == true))
    }

    var body: some View {
        let days = model.costDays
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .center) {
                Text("折合金额").font(.headline).accessibilityAddTraits(.isHeader)
                Spacer(minLength: 8)
                if account.cost != nil { CostGranularityPicker() }
            }
            if let c = account.cost {
                chartTile(c, days: days)
            } else {
                Text(host?.costLeftOut == true ? "电脑上的账号较多，折合金额没有上传（请在电脑上的 cca App 中查看）。" : Texts.costEmpty(account.tool))
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(14)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
            }
            Text(([Texts.costFootnote([account.tool], pricesDate: host?.cost?.pricesDate)] + [Texts.costFreshness(host?.cost, now: now)].compactMap { $0 })
                    .joined(separator: ""))
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .onChange(of: days) { picked = nil }
    }

    private func chartTile(_ c: AccountCost, days: Bool) -> some View {
        let bars = c.periods(days: days)
        let hit = bars.first { $0.id == picked }
        let notes = Texts.costNotes(c, now: now)
        return VStack(alignment: .leading, spacing: 12) {
            if bars.contains(where: { $0.cents > 0 }) {
                CostBarChart(bars: bars, days: days, domain: CostChart.domain([bars], days: days, now: now),
                             top: CostChart.niceTop(CostChart.maxUSD([bars])), now: now, picked: $picked)
                    .frame(height: 150)
            } else {
                Text(days ? "最近 30 天没有用量。" : "最近 12 周没有用量。")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
            Text(hit.map { Texts.costBarDetail($0, days: days, now: now) } ?? Texts.costCaption(c, days: days))
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            if bars.contains(where: { $0.partial && $0.cents > 0 }) { CostPartialLegend() }
            if let models = c.models, models.contains(where: { $0.cents(days: days) > 0 }) {
                Divider()
                CostModelList(models: models, days: days)
            }
            if c.devices.count >= 2 {
                Divider()
                CostDeviceList(devices: c.devices, days: days, now: now)
            }
            if !notes.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(Array(notes.enumerated()), id: \.offset) { _, n in CostNoteLine(note: n) }
                }
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
    }
}
