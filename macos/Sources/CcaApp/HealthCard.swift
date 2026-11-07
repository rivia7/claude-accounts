import SwiftUI

/// 出口检测卡片（`health`：启动账号、`cca check` 时记下的）：IP、地区、运营商、纯净度、标签、DNS / WebRTC 泄露、多久以前。
/// 结果好坏只看 cca 给的 `result`（不在 App 里拿纯净度和设定比）。旧结果（过时、换了节点）用灰色显示。
struct HealthCard: View {
    @Environment(AppModel.self) private var model
    let account: Account

    var body: some View {
        Card("出口检测", systemImage: "checkmark.shield") {
            if let h = account.health, let r = h.result {
                GlassPill(text: Texts.healthResult(r, region: h.restrictedRegion), color: Palette.health(r))
            }
        } content: {
            if let h = account.health {
                details(h)
            } else {
                Text("还没测过。点下面的“检测出口”，看看这个账号的出口 IP、地区、纯净度和 DNS / WebRTC 有没有泄露。")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    @ViewBuilder
    private func details(_ h: Health) -> some View {
        let dim = h.stale || !h.applies
        VStack(alignment: .leading, spacing: 12) {
            if !h.applies {
                Label("换了节点：这是旧节点 \(h.node ?? "—") 的结果，现在用的是 \(account.node ?? "直连")。再检测一次看看新的出口。",
                      systemImage: "arrow.triangle.swap")
                    .font(.callout)
                    .symbolRenderingMode(.multicolor)
                    .foregroundStyle(.primary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Group {
                if h.kind == .api {
                    Text(nonEmpty(h.message) ?? Texts.healthResult(h.result ?? HealthResult(rawValue: ""), region: nil))
                        .textSelection(.enabled)
                } else {
                    grid(h, dim: dim)
                }
                if let note = nonEmpty(h.note) {
                    Text(note).font(.callout).foregroundStyle(.secondary).textSelection(.enabled)
                }
            }
            .foregroundStyle(dim ? .secondary : .primary)
            TimelineView(.periodic(from: .now, by: 30)) { context in
                footer(h, now: context.date.timeIntervalSince1970)
            }
        }
    }

    private func grid(_ h: Health, dim: Bool) -> some View {
        Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 16, verticalSpacing: 8) {
            if let ip = h.ip {
                GridRow {
                    label("出口 IP")
                    Text(ip).font(.system(.body, design: .monospaced)).textSelection(.enabled)
                }
            }
            if h.country != nil || h.org != nil {
                GridRow {
                    label("地区 · 运营商")
                    Text([h.country, h.org].compactMap { nonEmpty($0) }.joined(separator: " · ")).textSelection(.enabled)
                }
            }
            GridRow {
                label("纯净度")
                trust(h, dim: dim)
            }
            if !h.tags.isEmpty {
                GridRow {
                    label("标签")
                    HStack(spacing: 6) {
                        ForEach(h.tags, id: \.self) { Tag(text: $0) }
                    }
                }
            }
            if let leak = h.leak {
                GridRow {
                    label("DNS")
                    leakView(leak.dns, what: "DNS", message: leak.dnsMessage)
                }
                GridRow {
                    label("WebRTC")
                    leakView(leak.webrtc, what: "WebRTC", message: leak.webrtcMessage)
                }
            }
        }
    }

    private func label(_ s: String) -> some View {
        Text(s).foregroundStyle(.secondary).gridColumnAlignment(.trailing)
    }

    /// 纯净度条的颜色跟着 cca 的 result 走（纯净度低于设定时 result 是 low_purity）。
    @ViewBuilder
    private func trust(_ h: Health, dim: Bool) -> some View {
        if let t = h.trust {
            HStack(spacing: 8) {
                Meter(fraction: t / 100, color: Palette.health(h.result), height: 6, dim: dim).frame(width: 120)
                Text("\(Int(t.rounded()))").monospacedDigit().fontWeight(.semibold)
                if let l = nonEmpty(h.trustLabel) { Text(l).foregroundStyle(.secondary) }
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("纯净度")
            .accessibilityValue("\(Int(t.rounded()))\(h.trustLabel.map { "，" + $0 } ?? "")")
        } else {
            Text("没有评分").foregroundStyle(.secondary)
        }
    }

    private func leakView(_ s: LeakState?, what: String, message: String?) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 5) {
                Image(systemName: s == .leak ? "exclamationmark.triangle.fill" : s == .ok ? "checkmark.circle.fill" : "questionmark.circle")
                    .foregroundStyle(Palette.leak(s))
                Text(Texts.leak(s, what: what)).fontWeight(s == .leak ? .semibold : .regular)
            }
            if s == .leak || s == .failed, let m = nonEmpty(message) {
                Text(m).font(.caption).foregroundStyle(.secondary).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
            }
        }
        .accessibilityElement(children: .combine)
    }

    private func footer(_ h: Health, now: Double) -> some View {
        var parts: [String] = []
        if let at = h.checkedAt { parts.append(Texts.agoPhrase(now - at) + (h.source == .check ? "手动检测" : "启动时检测")) }
        if let via = Texts.via(h.route) { parts.append(via) }
        if let n = h.node { parts.append(n + (h.port.map { "（端口 \($0)）" } ?? "")) }
        return HStack(spacing: 6) {
            Text(parts.joined(separator: " · ")).font(.caption).foregroundStyle(.secondary)
            if h.stale {
                Label("结果已经过时", systemImage: "clock.badge.exclamationmark").font(.caption).foregroundStyle(.secondary)
            }
        }
    }
}
