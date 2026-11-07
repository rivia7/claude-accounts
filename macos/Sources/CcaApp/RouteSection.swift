import SwiftUI

/// 详情里的“线路与出口”：这个账号的流量怎么走——这台 Mac → 路由器（或本机 Clash）的端口 → 节点 → 出口，
/// 画成一条账号颜色的细线；下面三格是出口检测（纯净度、DNS、WebRTC），再是标签和什么时候测的。
/// 端口的状态来自 `_gui live`（看过分流页才有），出口来自 health（启动账号、检测出口时 cca 记下的）。
struct RouteBlock: View {
    @Environment(AppModel.self) private var model
    let account: Account

    var body: some View {
        let a = account
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                Text("线路与出口").font(.system(size: 12.5, weight: .semibold)).foregroundStyle(Theme.fg)
                if let h = a.health, let r = h.result {
                    // 旧结果（过时、换了节点）的点是灰的：它说的不是现在的出口
                    StatusLabel(tone: h.stale || !h.applies ? .unknown : Palette.health(r),
                                text: Texts.healthResult(r, region: h.restrictedRegion))
                }
                Spacer(minLength: 8)
                Button {
                    model.checkExit(a.ref)
                } label: {
                    Label("检测出口", systemImage: "checkmark.shield")
                }
                .buttonStyle(.quiet)
                .disabled(!a.can.check || model.activity(a.ref) != nil)
                .help(a.isAPI ? "检测接口连不连得上" : "检测出口 IP、地区、纯净度和 DNS / WebRTC 泄露（会访问检测网站）")
            }
            RouteStrip(stops: stops(a), color: a.lineColor)
            if a.isRouted, model.state?.routing?.sync == .pending, model.routerConfigured {
                Notice(.warn, systemImage: "arrow.triangle.2.circlepath", title: "路由器还没同步",
                       message: "下次启动账号时会同步，也可以现在同步。") {
                    Button("同步路由器") { model.syncRouter() }
                        .buttonStyle(.action)
                        .disabled(model.syncActivity != nil)
                }
            }
            HealthDetails(account: a)
        }
    }

    // MARK: 线路上的几站

    private func stops(_ a: Account) -> [RouteStop] {
        var out = [RouteStop(id: 0, mark: .origin, title: "这台 Mac")]
        let live = model.liveSlot(for: a.ref)
        // 端口连不上时，后面的线画成断的；选错节点时，通向节点的那段画成虚线
        var broken = false
        var drift = false
        if let port = a.port {
            var s = RouteStop(id: 1, title: portText(port))
            s.detail = [a.group, routeName].compactMap { $0 }.joined(separator: " · ")
            if let live {
                s.help = liveLine(live)
                switch live.state {
                case .ok:
                    break
                case .drift:
                    drift = true
                    s.status = .drift
                    s.detail = "选中的是 \(live.selected ?? "—")"
                case .closed:
                    broken = true
                    s.status = .bad
                    s.detail = live.listening == false ? "端口没在监听" : "这台 Mac 连不上这个端口"
                default:
                    s.status = .unknown
                }
            }
            out.append(s)
        }
        if let node = a.node {
            var s = RouteStop(id: 2, title: node)
            s.symbol = a.pinned ? "lock.fill" : nil
            var notes: [String] = []
            if a.pinned { notes.append("维护者锁定") }
            if let local = localName(a), local != node { notes.append("Clash Verge 里叫 \(local)") }
            s.detail = notes.isEmpty ? nil : notes.joined(separator: "\n")
            if a.pinned { s.help = "维护者的映射锁定了这个账号的节点（\(a.pinnedNode ?? node)），这里不能改" }
            if broken { s.status = .unknown; s.line = .broken }
            if drift { s.line = .drift }
            out.append(s)
        }
        out.append(exitStop(a, broken: broken))
        return out
    }

    /// 最后一站：出口 IP 和地区（第三方 API 账号是接口的地址）。
    private func exitStop(_ a: Account, broken: Bool) -> RouteStop {
        var s = RouteStop(id: 3, mark: .end, title: "还没检测")
        s.status = .unknown
        s.line = broken ? .broken : .faint
        let h = a.health
        let old = h.map { $0.stale || !$0.applies } ?? false
        if a.isAPI {
            s.title = a.api?.host ?? "第三方接口"
            s.detail = a.node == nil ? "直连" : "接口"
            if let h, let r = h.result, !old, !broken {
                s.status = Palette.health(r)
                s.line = .solid
                if r != .apiOk { s.detail = Texts.healthResult(r, region: nil) }
            }
            if a.node == nil, !broken { s.line = .solid }
            return s
        }
        guard let h else { return s }
        if let ip = h.ip { s.title = ip }
        let place = [h.country, h.org].compactMap { nonEmpty($0) }.joined(separator: " · ")
        s.detail = place.isEmpty ? nil : place
        if old {
            s.detail = [s.detail, "旧结果"].compactMap { $0 }.joined(separator: " · ")
        } else if let r = h.result, !broken {
            s.status = Palette.health(r)
            s.line = .solid
            if r != .ok, r != .apiOk { s.detail = Texts.healthResult(r, region: h.restrictedRegion) }
        }
        return s
    }

    /// 经哪边：读到 live 时按它，否则按设置（配置了路由器就是路由器）。
    private var routeName: String? {
        switch model.live?.route {
        case .router?: return "路由器"
        case .local?: return nonEmpty(model.live?.local?.name) ?? "本机 Clash"
        default: break
        }
        if model.routerConfigured { return "路由器" }
        if model.localConfigured { return "本机 Clash" }
        return nil
    }

    private func localName(_ a: Account) -> String? {
        guard let n = a.slot else { return nil }
        return model.state?.routing?.slot(n)?.localNode
    }

    /// 「地址:端口」：读到 live 时用它的 proxy_host（看本机时是 127.0.0.1），否则用设置里路由器的 proxy_host。
    private func portText(_ port: Int) -> String {
        let host = model.live != nil ? nonEmpty(model.live?.proxyHost) : nil
        if let host = host ?? nonEmpty(model.state?.config?.router?.proxyHost) {
            return "\(host):\(port)"
        }
        return "端口 \(port)"
    }

    private func liveLine(_ s: LiveSlot) -> String {
        var parts: [String] = []
        if let sel = s.selected {
            parts.append(s.state == .drift ? "选中的是 \(sel)，应为 \(s.expected ?? "—")" : "选中 \(sel)")
        } else {
            parts.append("拿不到选中的节点")
        }
        if s.listening == false { parts.append("端口没在监听") }
        if s.reachable == false { parts.append("这台 Mac 连不上这个端口") }
        if s.inKernel == false { parts.append("内核里没有这个组") }
        return Texts.route(model.live?.route) + "：" + parts.joined(separator: "，")
    }
}

/// 线路上的一站。
struct RouteStop: Identifiable {
    enum Mark { case origin, stop, end }
    /// 通向这一站的那段线
    enum Line { case solid, faint, drift, broken }

    let id: Int
    var mark: Mark = .stop
    var status: Tone = .ok
    var line: Line = .solid
    var title: String
    var symbol: String?
    var detail: String?
    var help: String?

    init(id: Int, mark: Mark = .stop, title: String) {
        self.id = id
        self.mark = mark
        self.title = title
    }
}

/// 一条横着的线路：几站等宽排开，站名在点下面居中；线是账号的颜色，有问题的那段换成虚线。
struct RouteStrip: View {
    let stops: [RouteStop]
    let color: Color
    private let dot: CGFloat = 11

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            ForEach(stops) { s in
                VStack(spacing: 8) {
                    marker(s)
                    VStack(spacing: 2) {
                        title(s)
                            .font(.system(size: 12.5, weight: .semibold).monospacedDigit())
                            .foregroundStyle(Theme.fg)
                            .lineLimit(1)
                            .truncationMode(.middle)
                        if let d = s.detail, !d.isEmpty {
                            Text(d)
                                .font(.system(size: 11))
                                .foregroundStyle(detailColor(s))
                                .lineLimit(2)
                                .multilineTextAlignment(.center)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    .padding(.horizontal, 4)
                }
                .frame(maxWidth: .infinity)
                .help(s.help ?? "")
                .accessibilityElement(children: .combine)
            }
        }
        .background(alignment: .top) {
            GeometryReader { g in
                let w = g.size.width / CGFloat(max(stops.count, 1))
                ForEach(1..<max(stops.count, 1), id: \.self) { i in
                    let s = stops[i]
                    Path { p in
                        p.move(to: CGPoint(x: w * (CGFloat(i) - 0.5), y: dot / 2))
                        p.addLine(to: CGPoint(x: w * (CGFloat(i) + 0.5), y: dot / 2))
                    }
                    .stroke(lineColor(s.line), style: lineStyle(s.line))
                }
            }
            .frame(height: dot)
            .accessibilityHidden(true)
        }
    }

    private func title(_ s: RouteStop) -> Text {
        if let symbol = s.symbol {
            return Text("\(s.title) \(Image(systemName: symbol))")
        }
        return Text(s.title)
    }

    private func detailColor(_ s: RouteStop) -> Color {
        switch s.status {
        case .drift, .bad, .warn: return s.status.color
        default: return Theme.muted
        }
    }

    @ViewBuilder
    private func marker(_ s: RouteStop) -> some View {
        let t: Color = s.status == .ok ? color : (s.status == .unknown ? Theme.faint : s.status.color)
        ZStack {
            switch s.mark {
            case .origin:
                Circle().fill(t)
            case .stop:
                Circle().fill(s.status == .drift || s.status == .bad || s.status == .warn ? t : Theme.card2)
                Circle().strokeBorder(t, lineWidth: 2)
            case .end:
                Circle().fill(Theme.card2)
                Circle().strokeBorder(t, lineWidth: 2)
                Circle().fill(t).padding(3.5)
            }
        }
        .frame(width: dot, height: dot)
    }

    private func lineColor(_ l: RouteStop.Line) -> Color {
        switch l {
        case .solid: return color
        case .faint: return color.opacity(0.3)
        case .drift: return Theme.drift
        case .broken: return Theme.faint
        }
    }

    private func lineStyle(_ l: RouteStop.Line) -> StrokeStyle {
        switch l {
        case .solid, .faint: return StrokeStyle(lineWidth: 2, lineCap: .round)
        case .drift, .broken: return StrokeStyle(lineWidth: 2, lineCap: .round, dash: [3, 5])
        }
    }
}

/// 出口检测的明细：纯净度、DNS、WebRTC 三格，下面是标签、说明和什么时候测的。
/// 结果好坏只看 cca 给的 `result`（不在 App 里拿纯净度和设定比）。旧结果（过时、换了节点）用灰色显示。
struct HealthDetails: View {
    let account: Account

    var body: some View {
        if let h = account.health {
            let dim = h.stale || !h.applies
            VStack(alignment: .leading, spacing: 10) {
                if !h.applies {
                    Notice(.warn, systemImage: "arrow.triangle.swap",
                           title: "换了节点：这是旧节点 \(h.node ?? "—") 的结果",
                           message: "现在用的是 \(account.node ?? "直连")。再检测一次看看新的出口。")
                }
                if h.kind != .api {
                    // 三格一样宽、一样高（按最高的那格）；没有泄露检测时纯净度那格照样占三分之一
                    HStack(alignment: .top, spacing: 10) {
                        purity(h, dim: dim)
                        if let leak = h.leak {
                            leakCell("DNS", leak.dns, message: leak.dnsMessage, dim: dim)
                            leakCell("WebRTC", leak.webrtc, message: leak.webrtcMessage, dim: dim)
                        } else {
                            Color.clear.frame(maxWidth: .infinity).frame(height: 1)
                            Color.clear.frame(maxWidth: .infinity).frame(height: 1)
                        }
                    }
                    .fixedSize(horizontal: false, vertical: true)
                }
                HStack(spacing: 6) {
                    ForEach(h.tags, id: \.self) { CodePill(text: $0) }
                    TimelineView(.periodic(from: .now, by: 30)) { context in
                        footer(h, now: context.date.timeIntervalSince1970)
                    }
                }
                if let note = nonEmpty(h.note) {
                    Text(note).font(.system(size: 12)).foregroundStyle(Theme.fg2).textSelection(.enabled)
                }
            }
        } else {
            Text(account.isAPI ? "还没测过。点“检测出口”看看接口连不连得上。"
                 : "还没测过。点“检测出口”，看看这个账号的出口 IP、地区、纯净度和 DNS / WebRTC 有没有泄露。")
                .font(.system(size: 12.5))
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    /// 纯净度：分数和条的颜色跟着 cca 的 result 走（纯净度低于设定时 result 是 low_purity）。
    private func purity(_ h: Health, dim: Bool) -> some View {
        StatCell(title: "纯净度") {
            if let t = h.trust {
                HStack(alignment: .firstTextBaseline, spacing: 5) {
                    Text("\(Int(t.rounded()))")
                        .font(.system(size: 17, weight: .semibold).monospacedDigit())
                        .foregroundStyle(dim ? Theme.muted : Theme.fg)
                    if let l = nonEmpty(h.trustLabel) {
                        Text(l).font(.system(size: 11.5)).foregroundStyle(Theme.muted)
                    }
                }
                QuotaBar(fraction: t / 100, color: dim ? Theme.faint : Palette.health(h.result).color, height: 4)
            } else {
                Text("没有评分").font(.system(size: 14, weight: .semibold)).foregroundStyle(Theme.muted)
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("纯净度")
        .accessibilityValue(h.trust.map { "\(Int($0.rounded()))\(h.trustLabel.map { "，" + $0 } ?? "")" } ?? "没有评分")
    }

    private func leakCell(_ what: String, _ s: LeakState?, message: String?, dim: Bool) -> some View {
        let word = Texts.leak(s, what: "").trimmingCharacters(in: .whitespaces)
        return StatCell(title: what) {
            HStack(spacing: 6) {
                StatusDot(tone: dim ? .unknown : Palette.leak(s), size: 8)
                Text(word)
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(dim ? Theme.muted : Theme.fg)
            }
            if s == .leak || s == .failed, let m = nonEmpty(message) {
                Text(m).font(.system(size: 11)).foregroundStyle(Theme.muted).lineLimit(3)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
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
            Text(parts.joined(separator: "，")).foregroundStyle(Theme.muted)
            if h.stale {
                StatusLabel(tone: .warn, text: "结果已经过时")
            }
        }
        .font(.system(size: 11.5))
    }
}

/// 一格数据（magpie 的统计格）：白底细边，上面是小标题，下面是结果。
struct StatCell<Content: View>: View {
    let title: String
    let content: Content

    init(title: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.system(size: 11.5)).foregroundStyle(Theme.muted)
            content
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .card(radius: 10)
    }
}
