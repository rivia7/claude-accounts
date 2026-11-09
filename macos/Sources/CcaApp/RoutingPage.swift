import SwiftUI

/// 分流图的一行：一个槽位（端口）。账号表算出来的（state）加上路由器 / 本机上现在的样子（live）。
struct SlotRow: Identifiable {
    let id: Int
    let group: String?
    let port: Int?
    let ref: AccountRef?
    let account: Account?
    /// 应选的节点（看本机时是本机 Clash 里的叫法）
    let expected: String?
    /// 现在选中的节点（live）
    let selected: String?
    /// nil：还没查
    let state: SlotState?

    var isFree: Bool { ref == nil }

    /// 线连到哪个节点：选错了节点时连到实际选中的那个，否则连到应选的。
    var target: String? {
        if state == .drift, let s = nonEmpty(selected) { return s }
        return nonEmpty(expected)
    }
}

/// 节点分流页：标题和概况、提示条、分流图（账号 → 路由器 / 本机 Clash 的端口 → 节点），图下面逐条列出有问题的槽位。
/// 这一页可见时每 60 秒读一次 `_gui live`（Scheduler）。
struct RoutingPage: View {
    @Environment(AppModel.self) private var model
    @Environment(Guide.self) private var guide

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 22) {
                RoutingHeader()
                TipCard(id: "routing-map", title: "这张图怎么看",
                        message: "左边是账号，中间是路由器上给每个账号留的端口，右边是节点（出口）。每条彩色的线就是一个账号的流量走向：实线正常；虚线说明有问题（选错节点、连不上），点「同步路由器」能修好。点左边的账号可以看它的详情。")
                RoutingBanners()
                let rows = slotRows()
                if rows.isEmpty {
                    ContentUnavailableView("还没有端口池", systemImage: "point.3.connected.trianglepath.dotted",
                                           description: Text("配置路由器或本机 Clash 之后，每个走节点的账号会分到一个端口。"))
                } else {
                    RouteMap(rows: rows, nodes: mapNodes(rows), panel: panelTitle, showGroups: guide.showDetails) { ref in
                        model.show(ref)
                    }
                    SlotIssues(rows: rows)
                    DirectAccounts()
                }
            }
            .padding(.top, 24)
            .padding(.bottom, 36)
            .pageColumn()
        }
        .navigationTitle("节点分流")
    }

    private var lookingLocal: Bool { model.live?.route == .local }

    private func slotRows() -> [SlotRow] {
        let slots = model.state?.routing?.slots ?? []
        let live = model.live
        let local = lookingLocal
        return slots.map { s in
            let l = live?.slot(s.slot)
            let ref = s.ref
            return SlotRow(id: s.slot, group: s.group ?? l?.group, port: s.port ?? l?.port, ref: ref,
                           account: ref.flatMap { model.account($0) },
                           expected: l?.expected ?? (local ? s.localNode ?? s.node : s.node),
                           selected: l?.selected, state: l?.state ?? (s.isEmpty ? .empty : nil))
        }
    }

    /// 右边的节点：可选节点（看本机时用本机的叫法），再补上线连到、但不在可选节点里的（比如选错到别处）。
    private func mapNodes(_ rows: [SlotRow]) -> [MapNode] {
        let local = lookingLocal
        var out: [MapNode] = (model.state?.routing?.nodes ?? []).map { n in
            let name = local ? (nonEmpty(n.localName) ?? n.name) : n.name
            var note: String?
            if let ln = nonEmpty(n.localName), ln != n.name {
                note = local ? "路由器上叫 \(n.name)" : "Clash Verge 里叫 \(ln)"
            }
            return MapNode(name: name, note: note, users: n.users.count)
        }
        for r in rows where !r.isFree {
            if let t = r.target, !out.contains(where: { $0.name == t }) {
                out.append(MapNode(name: t, note: "不在可选节点里", users: 0, listed: false))
            }
        }
        return out
    }

    /// 中间面板的名字：OpenClash / ShellCrash / Clash Verge，下面一行是路由器的地址。
    private var panelTitle: (String, String?) {
        let live = model.live
        if lookingLocal {
            return (nonEmpty(live?.local?.name) ?? "本机 Clash", "这台 Mac")
        }
        if let r = live?.router, r.configured {
            return (r.backend?.rawValue ?? "路由器", r.host ?? model.state?.config?.router?.host)
        }
        if model.routerConfigured { return ("路由器", model.state?.config?.router?.host) }
        if model.localConfigured { return (nonEmpty(model.state?.config?.local?.name) ?? "本机 Clash", "这台 Mac") }
        return ("端口池", nil)
    }
}

/// 页头：标题、一行概况（走哪边、怎么选、端口范围、多久以前查的），后端 / 钩子 / 定时任务的状态；右边是看哪边、刷新、同步路由器。
struct RoutingHeader: View {
    @Environment(AppModel.self) private var model
    @Environment(Guide.self) private var guide

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            ViewThatFits(in: .horizontal) {
                HStack(alignment: .center, spacing: 16) {
                    titleBlock
                    Spacer(minLength: 12)
                    controls
                }
                VStack(alignment: .leading, spacing: 14) {
                    titleBlock
                    controls
                }
            }
            // 后端、钩子、定时任务的状态：平时收在“显示细节”里，有问题时才露出来
            if guide.showDetails || chipsNeedAttention {
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 8) { chips }
                    VStack(alignment: .leading, spacing: 6) { chips }
                }
            }
        }
    }

    private var chipsNeedAttention: Bool {
        if let r = model.live?.router, r.configured {
            if r.reachable == false || r.running == false || r.hook == false || r.cron == false { return true }
        }
        if let l = model.live?.local, l.configured, l.running == false, model.live?.route == .local { return true }
        return false
    }

    private var titleBlock: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 10) {
                Text("节点分流").font(.system(size: 24, weight: .semibold))
                if model.liveLoading { ProgressView().controlSize(.small) }
            }
            TimelineView(.periodic(from: .now, by: 30)) { context in
                Text(summary(now: context.date.timeIntervalSince1970))
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var controls: some View {
        HStack(spacing: 10) {
            routePicker
            GlassRow(spacing: 8) {
                Button {
                    model.refreshLive()
                } label: {
                    Label("刷新", systemImage: "arrow.clockwise")
                }
                .buttonStyle(.glass)
                .disabled(model.liveLoading)
                .help("重新读取路由器 / 本机 Clash 上各个端口现在的样子")
                if model.routerConfigured {
                    Button {
                        model.syncRouter()
                    } label: {
                        Label(model.syncActivity == nil ? "同步路由器" : "同步中…", systemImage: "wifi.router")
                    }
                    .buttonStyle(.glassProminent)
                    .disabled(model.syncActivity != nil)
                    .help("把这台 Mac 的账号表同步到路由器（可能要重载 OpenClash / ShellCrash，会先问你）")
                }
            }
            Button(guide.showDetails ? "收起细节" : "显示细节") {
                withAnimation(.snappy) { guide.toggleDetails() }
            }
            .buttonStyle(.borderless)
            .help("后端、钩子、定时任务的状态和槽位名")
        }
    }

    @ViewBuilder
    private var routePicker: some View {
        if model.routerConfigured && model.localConfigured {
            @Bindable var model = model
            Picker("看哪边", selection: $model.liveRoute) {
                Text("自动").tag(Route?.none)
                Text("路由器").tag(Route?.some(.router))
                Text("本机").tag(Route?.some(.local))
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .fixedSize()
            .help("自动：按 cca 的设置（连得上路由器看路由器，连不上看本机 Clash）")
        }
    }

    /// 一行概况：现在走哪边、怎么选、端口范围、多久以前查的。
    private func summary(now: Double) -> String {
        var parts: [String] = []
        if let l = model.live {
            switch l.route {
            case .router: parts.append("现在走路由器" + (l.router?.host.map { " \($0)" } ?? ""))
            case .local: parts.append("现在走" + (nonEmpty(l.local?.name) ?? "本机 Clash"))
            default: parts.append("没配置路由器和本机 Clash")
            }
        } else if model.routerConfigured {
            parts.append("走路由器")
        } else if model.localConfigured {
            parts.append("走本机 Clash")
        }
        let ports = (model.state?.routing?.slots ?? []).compactMap(\.port)
        if let lo = ports.min(), let hi = ports.max() {
            parts.append(lo == hi ? "端口 \(lo)" : "端口 \(lo)–\(hi)")
        }
        if let at = model.live?.generatedAt {
            parts.append(Texts.agoPhrase(now - at) + "查的")
        }
        return parts.joined(separator: "，")
    }

    /// 后端、本机 Clash、钩子、定时任务：状态写在文字里（不只靠颜色）。
    @ViewBuilder
    private var chips: some View {
        let cfg = model.state?.config
        let live = model.live
        if let r = live?.router, r.configured {
            let name = r.backend?.rawValue ?? "路由器"
            let state = r.reachable == false ? "连不上" : r.running == false ? "没在运行" : r.running == true ? "运行中" : "已配置"
            Tag(text: "\(name) \(state)", color: r.reachable == false || r.running == false ? .red : .green, systemImage: "wifi.router")
            statusTag("钩子", r.hook)
            statusTag("定时任务", r.cron)
        } else if cfg?.routerConfigured == true, let host = cfg?.router?.host {
            Tag(text: "路由器 \(host)", systemImage: "wifi.router")
        }
        if let l = live?.local, l.configured {
            let name = nonEmpty(l.name) ?? "本机 Clash"
            let state = l.running == true ? "运行中" : l.running == false ? "没开" : "这次没看"
            Tag(text: "\(name) \(state)", color: l.running == false ? Color.red : l.running == true ? Color.green : nil, systemImage: "laptopcomputer")
        } else if cfg?.localConfigured == true {
            Tag(text: nonEmpty(cfg?.local?.name) ?? "本机 Clash", systemImage: "laptopcomputer")
        }
    }

    private func statusTag(_ name: String, _ v: Bool?) -> some View {
        Tag(text: name + (v == true ? "正常" : v == false ? "没装" : "未知"),
            color: v == true ? Color.green : v == false ? Color.orange : nil,
            systemImage: v == true ? "checkmark" : v == false ? "xmark" : "questionmark")
    }
}

struct RoutingBanners: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let routing = model.state?.routing
        let drift = model.live?.summary?.drift ?? 0
        BannerStack {
            if routing?.sync == .pending {
                Banner(systemImage: "arrow.triangle.2.circlepath", tint: .orange, title: "路由器待同步",
                       message: "路由器上的端口池和这台 Mac 的账号表不一致" + (drift > 0 ? "（有 \(drift) 个槽位选错了节点）" : "")
                           + "：下次启动账号时会自动同步，也可以现在同步。") {
                    if model.routerConfigured {
                        Button("同步路由器") { model.syncRouter() }
                            .buttonStyle(.borderedProminent)
                            .disabled(model.syncActivity != nil)
                    }
                }
            }
            if let notices = model.state?.notices, !notices.isEmpty {
                Banner(systemImage: "info.circle", tint: .accentColor, title: "下次运行 cca 时会有这些改动",
                       message: notices.map(Texts.notice).joined(separator: "\n"))
            }
            if routing?.localSync == .pending {
                Banner(systemImage: "laptopcomputer", tint: .orange, title: "本机 Clash 的端口池不是最新的",
                       message: "下次在外面启动账号时会自动更新。")
            }
            if let e = model.live?.router?.error {
                routerErrorBanner(e)
            }
            if let e = model.live?.local?.error {
                Banner(systemImage: "exclamationmark.triangle", tint: .red, title: "本机 Clash：\(Texts.liveError(e))") {
                    if e.code == .notConfigured {
                        Button("配置本机 Clash…") { model.run(.localSetup) }.buttonStyle(.bordered)
                    }
                }
            }
            if let e = model.liveError {
                Banner(systemImage: "exclamationmark.triangle", tint: .red, title: "没能读取路由器 / 本机 Clash 的状态", message: e) {
                    Button("重试") { model.refreshLive() }.buttonStyle(.bordered)
                }
            }
            if drift > 0, routing?.sync != .pending {
                Banner(systemImage: "arrow.left.arrow.right", tint: .orange, title: "有 \(drift) 个槽位选错了节点",
                       message: "同步路由器能修好。") {
                    if model.routerConfigured {
                        Button("同步路由器") { model.syncRouter() }.buttonStyle(.bordered).disabled(model.syncActivity != nil)
                    }
                }
            }
        }
    }

    /// 连不上路由器：只有配置了本机 Clash、又不是“只走路由器”时，账号才会改走本机 Clash；否则启动会失败。
    private func routerErrorBanner(_ e: LiveIssue) -> some View {
        let localFallback = model.localConfigured && model.state?.config?.routePref != .router
        let message: String
        if e.code != .unreachable {
            message = ""
        } else if localFallback {
            message = "不在家、或者路由器没开？在外面时账号会走本机 Clash。"
        } else if model.localConfigured {
            message = "设成了只走路由器：连得上路由器之前，启动账号会失败。"
        } else {
            message = "连得上路由器之前启动账号会失败；在外面也想用，就配置本机 Clash。"
        }
        return Banner(systemImage: "wifi.exclamationmark", tint: .red, title: "路由器：\(Texts.liveError(e))", message: message) {
            if e.code == .unreachable, !model.localConfigured {
                Button("配置本机 Clash…") { model.run(.localSetup) }.buttonStyle(.bordered)
            }
        }
    }
}

/// 分流图右边的一个节点。
struct MapNode: Identifiable {
    let name: String
    var note: String?
    /// 账号表里用它的账号数（按配置，不按现在实际选中的）
    var users = 0
    /// 在可选节点里（不在的是线连到了别处）
    var listed = true
    var id: String { name }
}

/// 分流图：左边是账号（头像在靠线的一侧），中间是路由器 / 本机 Clash 的端口面板，右边是节点；
/// 每个账号一条自己颜色的线，经它的端口连到节点（和 App 图标一样，从一处分出去的彩色曲线）。
/// 选错节点：端口的灯是橙色，线是橙色虚线、连到实际选中的节点，应选的那条留一道淡淡的点线；连不上：红灯，后半段是灰色虚线。
/// 点账号跳到它的详情。
struct RouteMap: View {
    let rows: [SlotRow]
    let nodes: [MapNode]
    let panel: (String, String?)
    /// 端口旁边写槽位名（CCA-1 这些）：显示细节时才有
    var showGroups = false
    let select: (AccountRef) -> Void

    private let rowHeight: CGFloat = 44
    private let panelHeader: CGFloat = 52
    private let nodeHeight: CGFloat = 46
    private let nodeGap: CGFloat = 10

    var body: some View {
        GeometryReader { g in
            let geo = MapGeometry(width: g.size.width, rows: rows, nodes: nodes, rowHeight: rowHeight,
                                  top: panelHeader, nodeHeight: nodeHeight, nodeGap: nodeGap)
            ZStack(alignment: .topLeading) {
                panelBackground(geo)
                wires(geo)
                ForEach(Array(rows.enumerated()), id: \.element.id) { i, r in
                    if let ref = r.ref {
                        accountChip(r, ref: ref)
                            .frame(width: geo.accountWidth, height: rowHeight, alignment: .trailing)
                            .position(x: geo.accountWidth / 2, y: geo.rowY(i))
                    }
                    jack(r)
                        .frame(width: geo.jackWidth, height: 28)
                        .position(x: geo.panelX + geo.panelWidth / 2, y: geo.rowY(i))
                }
                ForEach(Array(nodes.enumerated()), id: \.element.id) { j, n in
                    nodeCard(n, linked: geo.nodeLinks[j] > 0)
                        .frame(width: geo.nodeWidth, height: nodeHeight)
                        .position(x: geo.nodeX + geo.nodeWidth / 2, y: geo.nodeY[j])
                }
            }
        }
        .frame(height: MapGeometry.height(rows: rows.count, nodes: nodes.count, rowHeight: rowHeight, top: panelHeader,
                                          nodeHeight: nodeHeight, nodeGap: nodeGap))
        .frame(minWidth: 520)
    }

    // MARK: 部件

    private func panelBackground(_ geo: MapGeometry) -> some View {
        let shape = RoundedRectangle(cornerRadius: 18, style: .continuous)
        return ZStack(alignment: .top) {
            shape.fill(Surface.tile)
            shape.strokeBorder(Surface.hairline, lineWidth: 0.5)
            VStack(spacing: 2) {
                Text(panel.0).font(.callout.weight(.semibold)).lineLimit(1)
                if let sub = panel.1 {
                    Text(sub).font(.caption).foregroundStyle(.secondary).monospacedDigit().lineLimit(1)
                }
            }
            .padding(.top, 10)
            .padding(.horizontal, 8)
        }
        .frame(width: geo.panelWidth, height: geo.height)
        .offset(x: geo.panelX)
        .accessibilityElement(children: .combine)
    }

    private func wires(_ geo: MapGeometry) -> some View {
        Canvas { ctx, _ in
            let solid = StrokeStyle(lineWidth: 2.5, lineCap: .round, lineJoin: .round)
            let dashed = StrokeStyle(lineWidth: 2.5, lineCap: .round, dash: [6, 5])
            let dotted = StrokeStyle(lineWidth: 1.5, lineCap: .round, dash: [1, 4])
            for (i, r) in rows.enumerated() where !r.isFree {
                let y = geo.rowY(i)
                let color = r.account?.lineColor ?? Color.gray
                var lead = Path()
                lead.move(to: CGPoint(x: geo.accountWidth + 3, y: y))
                lead.addLine(to: CGPoint(x: geo.jackLeft, y: y))
                ctx.stroke(lead, with: .color(color), style: solid)
                let from = CGPoint(x: geo.jackRight, y: y)
                // 选错节点：应选的那条留一道淡淡的点线
                if r.state == .drift, let e = nonEmpty(r.expected), let j = nodes.firstIndex(where: { $0.name == e }) {
                    ctx.stroke(curve(from, CGPoint(x: geo.nodeX, y: geo.nodeY[j])), with: .color(color.opacity(0.55)), style: dotted)
                }
                guard let t = r.target, let j = nodes.firstIndex(where: { $0.name == t }) else { continue }
                let path = curve(from, CGPoint(x: geo.nodeX, y: geo.nodeY[j]))
                switch r.state {
                case .drift?: ctx.stroke(path, with: .color(.orange), style: dashed)
                case .closed?: ctx.stroke(path, with: .color(Color.secondary.opacity(0.5)), style: dashed)
                default: ctx.stroke(path, with: .color(color), style: solid)
                }
            }
        }
        .frame(width: geo.width, height: geo.height)
        .accessibilityHidden(true)
    }

    private func curve(_ a: CGPoint, _ b: CGPoint) -> Path {
        var p = Path()
        p.move(to: a)
        let dx = (b.x - a.x) * 0.5
        p.addCurve(to: b, control1: CGPoint(x: a.x + dx, y: a.y), control2: CGPoint(x: b.x - dx, y: b.y))
        return p
    }

    private func accountChip(_ r: SlotRow, ref: AccountRef) -> some View {
        let a = r.account
        return Button {
            select(ref)
        } label: {
            HStack(spacing: 10) {
                VStack(alignment: .trailing, spacing: 1) {
                    Text(a?.displayName ?? ref.key)
                        .font(.callout.weight(.medium))
                        .foregroundStyle(a == nil ? .secondary : .primary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    HStack(spacing: 4) {
                        Text(a?.isAPI == true ? "第三方 API" : Texts.product(ref.tool))
                        if a?.isAPI != true, let a, let p = nonEmpty(a.plan) {
                            PlanBadge(plan: p, tier: a.planTier, size: .compact)
                        }
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                }
                BadgeAvatar(badge: a?.badge, isAPI: a?.isAPI ?? false, size: 24)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(a.map { "看 \($0.displayName) 的详情" } ?? ref.key)
        .accessibilityLabel("\(a?.displayName ?? ref.key)，\(chipSubtitle(a, ref: ref))")
        .accessibilityHint("看这个账号的详情")
    }

    private func chipSubtitle(_ a: Account?, ref: AccountRef) -> String {
        if a?.isAPI == true { return "第三方 API" }
        let tool = Texts.product(ref.tool)
        if let p = nonEmpty(a?.plan) { return "\(tool) \(p)" }
        return tool
    }

    private func jack(_ r: SlotRow) -> some View {
        let color: Color = r.isFree ? .clear : (r.state.map { Palette.slot($0) } ?? .gray)
        return HStack(spacing: 6) {
            ZStack {
                if r.isFree {
                    Circle().strokeBorder(.tertiary, lineWidth: 1)
                } else {
                    Circle().fill(color)
                }
            }
            .frame(width: 7, height: 7)
            Text(r.port.map(String.init) ?? "—")
                .font(.callout.weight(.semibold).monospacedDigit())
                .foregroundStyle(r.isFree ? .tertiary : .primary)
            Text(r.isFree ? "空闲" : (showGroups ? (r.group ?? "") : ""))
                .font(.caption2)
                .foregroundStyle(.tertiary)
                .lineLimit(1)
        }
        .padding(.horizontal, 10)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Capsule().fill(.background))
        .overlay(Capsule().strokeBorder(Surface.hairline, lineWidth: 0.5))
        .opacity(r.isFree ? 0.75 : 1)
        .help(jackHelp(r))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(jackHelp(r))
    }

    private func jackHelp(_ r: SlotRow) -> String {
        let port = r.port.map { "端口 \($0)" } ?? "端口"
        let group = r.group.map { "（\($0)）" } ?? ""
        guard let ref = r.ref else { return "\(port)\(group)：空闲" }
        let who = r.account?.displayName ?? ref.key
        var text = "\(port)\(group)：\(who) → \(nonEmpty(r.expected) ?? "—")"
        switch r.state {
        case .drift?: text += "；选中的是 \(r.selected ?? "—")"
        case .closed?: text += "；连不上"
        case .ok?: text += "；正常"
        case nil: text += "；还没查"
        default: text += "；" + Texts.slotState(r.state!)
        }
        return text
    }

    private func nodeCard(_ n: MapNode, linked: Bool) -> some View {
        let shape = RoundedRectangle(cornerRadius: 12, style: .continuous)
        let count = n.users == 0 ? "没有账号在用" : "\(n.users) 个账号"
        return VStack(alignment: .leading, spacing: 2) {
            Text(n.name)
                .font(.callout.weight(.semibold))
                .foregroundStyle(linked ? .primary : .secondary)
                .lineLimit(1)
            Text(n.note ?? count)
                .font(.caption)
                .foregroundStyle(n.listed ? AnyShapeStyle(.secondary) : AnyShapeStyle(Color.orange))
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .padding(.horizontal, 12)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        .background(shape.fill(.background))
        .overlay(shape.strokeBorder(Surface.hairline, lineWidth: 0.5))
        .opacity(linked ? 1 : 0.7)
        .help([n.name, n.note, count].compactMap { $0 }.joined(separator: "，"))
        .accessibilityElement(children: .combine)
    }
}

/// 分流图的几何：三列的位置，每行的高度，节点放在连到它的那几行的中间（互相挤开、不越界）。
struct MapGeometry {
    let width: CGFloat
    let height: CGFloat
    let accountWidth: CGFloat
    let panelX: CGFloat
    let panelWidth: CGFloat
    let jackWidth: CGFloat
    let nodeX: CGFloat
    let nodeWidth: CGFloat
    let rowHeight: CGFloat
    let top: CGFloat
    let nodeY: [CGFloat]
    /// 连到每个节点的线（实际的线，加上选错节点时应选的那道点线）
    let nodeLinks: [Int]

    var jackLeft: CGFloat { panelX + (panelWidth - jackWidth) / 2 }
    var jackRight: CGFloat { jackLeft + jackWidth }

    func rowY(_ i: Int) -> CGFloat { top + CGFloat(i) * rowHeight + rowHeight / 2 }

    static func height(rows: Int, nodes: Int, rowHeight: CGFloat, top: CGFloat, nodeHeight: CGFloat, nodeGap: CGFloat) -> CGFloat {
        let byRows = top + CGFloat(rows) * rowHeight + 12
        let byNodes = top + CGFloat(nodes) * (nodeHeight + nodeGap)
        return max(byRows, byNodes)
    }

    init(width: CGFloat, rows: [SlotRow], nodes: [MapNode], rowHeight: CGFloat, top: CGFloat, nodeHeight: CGFloat, nodeGap: CGFloat) {
        self.width = width
        self.rowHeight = rowHeight
        self.top = top
        height = MapGeometry.height(rows: rows.count, nodes: nodes.count, rowHeight: rowHeight, top: top,
                                    nodeHeight: nodeHeight, nodeGap: nodeGap)
        accountWidth = min(max(width * 0.28, 150), 230)
        panelWidth = 156
        jackWidth = 132
        nodeWidth = min(max(width * 0.23, 140), 210)
        nodeX = width - nodeWidth
        // 面板放在账号和节点之间偏左的地方：右边的曲线要更多地方
        let free = max(nodeX - accountWidth - panelWidth, 0)
        panelX = accountWidth + free * 0.32

        // 每个节点理想的位置：连到它的那几行的平均高度（选错节点时应选的那个也算）；没有线的排在后面
        var users = Array(repeating: 0, count: nodes.count)
        var sum = Array(repeating: CGFloat(0), count: nodes.count)
        for (i, r) in rows.enumerated() where !r.isFree {
            var names = [r.target]
            if r.state == .drift { names.append(nonEmpty(r.expected)) }
            for t in Set(names.compactMap { $0 }) {
                guard let j = nodes.firstIndex(where: { $0.name == t }) else { continue }
                users[j] += 1
                sum[j] += top + CGFloat(i) * rowHeight + rowHeight / 2
            }
        }
        nodeLinks = users
        let lastRow = top + CGFloat(max(rows.count - 1, 0)) * rowHeight + rowHeight / 2
        let desired: [CGFloat] = nodes.indices.map { j in users[j] > 0 ? sum[j] / CGFloat(users[j]) : lastRow + CGFloat(j) }
        let order = nodes.indices.sorted { a, b in
            (users[a] > 0) != (users[b] > 0) ? users[a] > 0 : desired[a] < desired[b]
        }
        let step = nodeHeight + nodeGap
        var placed = Array(repeating: CGFloat(0), count: nodes.count)
        var prev = -CGFloat.infinity
        let minY = top + nodeHeight / 2 - 6
        for j in order {
            let y = max(desired[j], prev + step, minY)
            placed[j] = y
            prev = y
        }
        // 越过底边就整体往上挪（不越过顶边）
        let maxY = height - nodeHeight / 2 - 4
        if let last = order.last, placed[last] > maxY {
            let shift = placed[last] - maxY
            for j in order { placed[j] = max(placed[j] - shift, minY) }
            // 往上挪时挤在顶上的，再按顺序推开
            var p = -CGFloat.infinity
            for j in order {
                placed[j] = max(placed[j], p + step)
                p = placed[j]
            }
        }
        nodeY = placed
    }
}

/// 图下面逐条列出有问题的槽位（选错节点、连不上），说清楚应选什么、现在是什么。
struct SlotIssues: View {
    @Environment(AppModel.self) private var model
    let rows: [SlotRow]

    var body: some View {
        let bad = rows.filter { !$0.isFree && ($0.state == .drift || $0.state == .closed) }
        if !bad.isEmpty {
            VStack(alignment: .leading, spacing: 8) {
                ForEach(bad) { r in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Image(systemName: r.state == .drift ? "arrow.left.arrow.right" : "xmark.octagon.fill")
                            .foregroundStyle(Palette.slot(r.state))
                        Text(line(r)).font(.callout).fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            .accessibilityElement(children: .contain)
        }
    }

    private func line(_ r: SlotRow) -> String {
        let who = r.account?.displayName ?? r.ref?.key ?? "—"
        let slot = [r.group, r.port.map { "端口 \($0)" }].compactMap { $0 }.joined(separator: "，")
        if r.state == .drift {
            return "\(who)（\(slot)）选中的是 \(r.selected ?? "—")，应为 \(r.expected ?? "—")"
        }
        return "\(who)（\(slot)）连不上"
    }
}

/// 不经节点的账号（直连的第三方 API 账号）：图里没有它们，在这里提一句。
struct DirectAccounts: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let direct = model.visibleTools.flatMap { model.accounts($0) }.filter { $0.isAPI && !$0.isRouted }
        if !direct.isEmpty {
            HStack(spacing: 8) {
                Text("不经节点、直连的：").foregroundStyle(.secondary)
                ForEach(direct) { a in
                    Button {
                        model.show(a.ref)
                    } label: {
                        HStack(spacing: 5) {
                            BadgeAvatar(badge: a.badge, isAPI: true, size: 16)
                            Text(a.displayName)
                        }
                    }
                    .buttonStyle(.plain)
                    .help("看 \(a.displayName) 的详情")
                }
            }
            .font(.callout)
        }
    }
}
