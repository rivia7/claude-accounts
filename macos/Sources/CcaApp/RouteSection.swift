import SwiftUI

/// 网络出口：这个账号的流量怎么走，画成一条账号颜色的线（和分流图、App 图标同一个样子），下面是出口检测。
/// 默认简单：这台 Mac → 节点 → 出口三站，检测结果一行小标签；点“显示细节”多出路由器的端口那一站（槽位、经哪边）和三块明细。
/// 端口的状态来自 `_gui live`（打开过分流页才有），出口来自 health（启动账号、检测出口时 cca 记下的）。
struct RouteSection: View {
    @Environment(AppModel.self) private var model
    @Environment(Guide.self) private var guide
    let account: Account

    var body: some View {
        let a = account
        VStack(alignment: .leading, spacing: 18) {
            SectionHeader("网络出口") {
                if let h = a.health, let r = h.result {
                    // 旧结果（过时、换了节点）的颜色换成灰的：它说的不是现在的出口
                    Tag(text: Texts.healthResult(r, region: h.restrictedRegion),
                        color: h.stale || !h.applies ? Color.gray : Palette.health(r))
                }
                Button(guide.showDetails ? "收起细节" : "显示细节") {
                    withAnimation(.snappy) { guide.toggleDetails() }
                }
                .buttonStyle(.borderless)
                .help("端口、槽位、纯净度和泄露检测的明细")
                Button {
                    model.checkExit(a.ref)
                } label: {
                    Label("检测出口", systemImage: "checkmark.shield")
                }
                .buttonStyle(.glass)
                .disabled(!a.can.check || model.activity(a.ref) != nil)
                .help(a.isAPI ? "检测接口连不连得上" : "检测出口 IP、地区、纯净度和 DNS / WebRTC 泄露（会访问检测网站）")
            }
            RouteStrip(stops: guide.showDetails ? stops(a) : simpleStops(a), color: a.lineColor)
                .padding(.top, 2)
            if a.isRouted, model.state?.routing?.sync == .pending, model.routerConfigured {
                Banner(systemImage: "arrow.triangle.2.circlepath", tint: .orange, title: "路由器还没同步",
                       message: "下次启动账号时会同步，也可以现在同步。") {
                    Button("同步路由器") { model.syncRouter() }
                        .buttonStyle(.bordered)
                        .disabled(model.syncActivity != nil)
                }
            }
            if guide.showDetails {
                HealthDetails(account: a)
            } else {
                HealthSummary(account: a)
            }
        }
    }

    // MARK: 线路上的几站

    /// 默认的简单线路：不画端口那一站；端口有问题（选错节点、连不上）时，问题挪到节点那一站说。
    private func simpleStops(_ a: Account) -> [RouteStop] {
        var s = stops(a)
        guard let pi = s.firstIndex(where: { $0.id == 1 }) else { return s }
        let port = s.remove(at: pi)
        if let ni = s.firstIndex(where: { $0.id == 2 }) {
            if port.status == .warn || port.status == .bad {
                s[ni].status = port.status
                s[ni].detail = port.detail
                s[ni].detailTint = port.detailTint
            } else {
                s[ni].detail = routeName.map { "经" + $0 }
                s[ni].detailTint = nil
            }
        }
        return s
    }

    private func stops(_ a: Account) -> [RouteStop] {
        var out = [RouteStop(id: 0, mark: .origin, title: "这台 Mac")]
        let live = model.liveSlot(for: a.ref)
        // 端口连不上时，后面的线画成断的；选错节点时，通向节点的那段画成橙色虚线
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
                    s.status = .warn
                    s.detail = "选中的是 \(live.selected ?? "—")"
                    s.detailTint = .orange
                case .closed:
                    broken = true
                    s.status = .bad
                    s.detail = live.listening == false ? "端口没在监听" : "这台 Mac 连不上这个端口"
                    s.detailTint = .red
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
            if drift { s.line = .warn }
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
            // 接口的地址；连不连得上写在标题旁边的结果里，这里只说直连还是经节点
            s.title = a.api?.host ?? "第三方接口"
            s.detail = a.node == nil ? "直连" : "接口"
            if let h, let r = h.result, !old, !broken {
                s.status = status(r)
                s.line = .solid
                if r != .apiOk {
                    s.detail = Texts.healthResult(r, region: nil)
                    s.detailTint = Palette.health(r)
                }
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
            s.status = status(r)
            s.line = .solid
            if r != .ok, r != .apiOk {
                s.detail = Texts.healthResult(r, region: h.restrictedRegion)
                s.detailTint = Palette.health(r)
            }
        }
        return s
    }

    private func status(_ r: HealthResult) -> RouteStop.Status {
        switch r {
        case .ok, .apiOk: return .normal
        case .lowPurity, .noScore: return .warn
        case .restricted, .noExit, .portClosed, .apiUnreachable: return .bad
        default: return .unknown
        }
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
    enum Status { case normal, unknown, warn, bad }
    /// 通向这一站的那段线
    enum Line { case solid, faint, warn, broken }

    let id: Int
    var mark: Mark = .stop
    var status: Status = .normal
    var line: Line = .solid
    var title: String
    var symbol: String?
    var detail: String?
    var detailTint: Color?
    var help: String?

    init(id: Int, mark: Mark = .stop, title: String) {
        self.id = id
        self.mark = mark
        self.title = title
    }
}

/// 一条横着的线路：几站等宽排开，站名在圆点下面居中；线用账号的颜色，有问题的那段换成橙色或灰色的虚线。
/// 每站最宽 300pt，再宽就整条居中（宽窗口里只有这一节的第三方 API 账号，两站不会被拉到页面两头）。
struct RouteStrip: View {
    let stops: [RouteStop]
    let color: Color
    private let dot: CGFloat = 14
    private let maxStopWidth: CGFloat = 300

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            ForEach(stops) { s in
                VStack(spacing: 10) {
                    marker(s)
                    VStack(spacing: 3) {
                        title(s)
                            .font(.callout.weight(.semibold))
                            .monospacedDigit()
                            .lineLimit(1)
                            .truncationMode(.middle)
                        if let d = s.detail, !d.isEmpty {
                            // 最多三行：说明里可能自带换行（“维护者锁定”下面一行“Clash Verge 里叫 …”），站窄时第二段还能再折一行
                            Text(d)
                                .font(.caption)
                                .foregroundStyle(s.detailTint.map { AnyShapeStyle($0) } ?? AnyShapeStyle(.secondary))
                                .lineLimit(3)
                                .multilineTextAlignment(.center)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    .padding(.horizontal, 6)
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
        .frame(maxWidth: maxStopWidth * CGFloat(max(stops.count, 1)))
        .frame(maxWidth: .infinity)
    }

    private func title(_ s: RouteStop) -> Text {
        if let symbol = s.symbol {
            return Text("\(s.title) \(Image(systemName: symbol))")
        }
        return Text(s.title)
    }

    private func tint(_ s: RouteStop) -> Color {
        switch s.status {
        case .normal: return color
        case .unknown: return .gray
        case .warn: return .orange
        case .bad: return .red
        }
    }

    @ViewBuilder
    private func marker(_ s: RouteStop) -> some View {
        let t = tint(s)
        ZStack {
            switch s.mark {
            case .origin:
                Circle().fill(t)
            case .stop:
                Circle().fill(s.status == .warn || s.status == .bad ? AnyShapeStyle(t) : AnyShapeStyle(.background))
                Circle().strokeBorder(t, lineWidth: 3)
            case .end:
                Circle().fill(.background)
                Circle().strokeBorder(t, lineWidth: 3)
                Circle().fill(t).padding(4.5)
            }
        }
        .frame(width: dot, height: dot)
    }

    private func lineColor(_ l: RouteStop.Line) -> Color {
        switch l {
        case .solid: return color
        case .faint: return color.opacity(0.35)
        case .warn: return .orange
        case .broken: return Color.secondary.opacity(0.5)
        }
    }

    private func lineStyle(_ l: RouteStop.Line) -> StrokeStyle {
        switch l {
        case .solid, .faint: return StrokeStyle(lineWidth: 3, lineCap: .round)
        case .warn, .broken: return StrokeStyle(lineWidth: 3, lineCap: .round, dash: [5, 5])
        }
    }
}

/// 出口检测的明细：纯净度、DNS、WebRTC 三块，下面是标签、说明和什么时候测的。
/// 结果好坏只看 cca 给的 `result`（不在 App 里拿纯净度和设定比）。旧结果（过时、换了节点）用灰色显示。
struct HealthDetails: View {
    let account: Account

    var body: some View {
        if let h = account.health {
            let dim = h.stale || !h.applies
            VStack(alignment: .leading, spacing: 12) {
                if !h.applies {
                    Label("换了节点：这是旧节点 \(h.node ?? "—") 的结果，现在用的是 \(account.node ?? "直连")。再检测一次看看新的出口。",
                          systemImage: "arrow.triangle.swap")
                        .font(.callout)
                        .foregroundStyle(.primary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if h.kind != .api {
                    // 三块一样宽、一样高（按最高的那块）；没有泄露检测时纯净度那块照样占三分之一
                    HStack(alignment: .top, spacing: 12) {
                        purity(h, dim: dim)
                        if let leak = h.leak {
                            leakTile("DNS", leak.dns, message: leak.dnsMessage, dim: dim)
                            leakTile("WebRTC", leak.webrtc, message: leak.webrtcMessage, dim: dim)
                        } else {
                            Color.clear.frame(maxWidth: .infinity).frame(height: 1)
                            Color.clear.frame(maxWidth: .infinity).frame(height: 1)
                        }
                    }
                    .fixedSize(horizontal: false, vertical: true)
                }
                if !h.tags.isEmpty {
                    HStack(spacing: 6) {
                        ForEach(h.tags, id: \.self) { Tag(text: $0) }
                    }
                }
                if let note = nonEmpty(h.note) {
                    Text(note).font(.callout).foregroundStyle(.secondary).textSelection(.enabled)
                }
                TimelineView(.periodic(from: .now, by: 30)) { context in
                    footer(h, now: context.date.timeIntervalSince1970)
                }
            }
        } else {
            Text(account.isAPI ? "还没测过。点“检测出口”看看接口连不连得上。"
                 : "还没测过。点“检测出口”，看看这个账号的出口 IP、地区、纯净度和 DNS / WebRTC 有没有泄露。")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    /// 纯净度：分数和条的颜色跟着 cca 的 result 走（纯净度低于设定时 result 是 low_purity）。
    private func purity(_ h: Health, dim: Bool) -> some View {
        CheckTile(title: "纯净度") {
            if let t = h.trust {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text("\(Int(t.rounded()))")
                        .font(.title2.weight(.semibold).monospacedDigit())
                        .foregroundStyle(dim ? .secondary : .primary)
                    if let l = nonEmpty(h.trustLabel) {
                        Text(l).font(.callout).foregroundStyle(.secondary)
                    }
                }
                Meter(fraction: t / 100, color: Palette.health(h.result), height: 5, dim: dim)
            } else {
                Text("没有评分").font(.title3.weight(.semibold)).foregroundStyle(.secondary)
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("纯净度")
        .accessibilityValue(h.trust.map { "\(Int($0.rounded()))\(h.trustLabel.map { "，" + $0 } ?? "")" } ?? "没有评分")
    }

    private func leakTile(_ what: String, _ s: LeakState?, message: String?, dim: Bool) -> some View {
        let word = Texts.leak(s, what: "").trimmingCharacters(in: .whitespaces)
        return CheckTile(title: what) {
            HStack(spacing: 6) {
                Image(systemName: s == .leak ? "exclamationmark.triangle.fill" : s == .ok ? "checkmark.circle.fill" : "questionmark.circle")
                    .foregroundStyle(dim ? Color.secondary : Palette.leak(s))
                Text(word)
                    .font(.title3.weight(.semibold))
                    .foregroundStyle(dim ? .secondary : .primary)
            }
            if s == .leak || s == .failed, let m = nonEmpty(message) {
                Text(m).font(.caption).foregroundStyle(.secondary).lineLimit(3)
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
        return HStack(spacing: 8) {
            Text(parts.joined(separator: "，")).font(.caption).foregroundStyle(.secondary)
            if h.stale {
                Label("结果已经过时", systemImage: "clock.badge.exclamationmark").font(.caption).foregroundStyle(.orange)
            }
        }
    }
}

/// 出口检测的一块：小标题，下面是结果。
struct CheckTile<Content: View>: View {
    let title: String
    let content: Content

    init(title: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.callout.weight(.semibold)).foregroundStyle(.secondary)
            content
        }
        .padding(14)
        .frame(maxWidth: .infinity, minHeight: 92, maxHeight: .infinity, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
    }
}

/// 出口检测的一行摘要（默认）：纯净度、DNS、WebRTC 各一个小标签，再是什么时候测的；点“显示细节”看三块明细。
struct HealthSummary: View {
    let account: Account

    var body: some View {
        if let h = account.health {
            let dim = h.stale || !h.applies
            VStack(alignment: .leading, spacing: 10) {
                if !h.applies {
                    Label("换了节点：这是旧节点 \(h.node ?? "—") 的结果，再检测一次看看新的出口。", systemImage: "arrow.triangle.swap")
                        .font(.callout)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if h.kind != .api {
                    HStack(spacing: 6) {
                        if let t = h.trust {
                            Tag(text: "纯净度 \(Int(t.rounded()))" + (nonEmpty(h.trustLabel).map { " · \($0)" } ?? ""),
                                color: dim ? Color.gray : Palette.health(h.result))
                        }
                        if let leak = h.leak {
                            Tag(text: Texts.leak(leak.dns, what: "DNS"), color: dim ? Color.gray : Palette.leak(leak.dns))
                            Tag(text: Texts.leak(leak.webrtc, what: "WebRTC"), color: dim ? Color.gray : Palette.leak(leak.webrtc))
                        }
                    }
                }
                TimelineView(.periodic(from: .now, by: 30)) { context in
                    HStack(spacing: 6) {
                        if let at = h.checkedAt {
                            Text(Texts.agoPhrase(context.date.timeIntervalSince1970 - at) + (h.source == .check ? "手动检测" : "启动时检测"))
                        }
                        if h.stale {
                            Label("结果已经过时", systemImage: "clock.badge.exclamationmark").foregroundStyle(.orange)
                        }
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                }
            }
        } else {
            Text(account.isAPI ? "还没测过。点“检测出口”看看接口连不连得上。"
                 : "还没测过。点“检测出口”，看看这个账号的出口 IP、地区、纯净度和 DNS / WebRTC 有没有泄露。")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
