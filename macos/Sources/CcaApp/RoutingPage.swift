import SwiftUI

/// 一个槽位（端口）：账号表算出来的（state）加上路由器 / 本机上现在的样子（live）。
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

    var name: String { account?.displayName ?? ref?.key ?? "—" }
}

/// 分流图右边的一个节点。
struct MapNode: Identifiable {
    let name: String
    var note: String?
    /// 账号表里用它的账号（按配置，不按现在实际选中的）
    var users: [AccountRef] = []
    /// 在可选节点里（不在的是线连到了别处）
    var listed = true
    var id: String { name }
}

/// 节点分流页（参考 magpie 的路由页）：标题行（实时的点、看哪边、刷新、同步路由器），提示，
/// 一张卡片里是分流图（左边账号 → 中间路由器 / 本机 Clash → 右边节点，每个账号一条自己颜色的线）、统计和有问题的槽位，
/// 下面是端口的列表。这一页可见时每 60 秒读一次 `_gui live`（Scheduler）。
struct RoutingPage: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                RoutingHead()
                RoutingBanners()
                let rows = slotRows()
                if rows.isEmpty {
                    VStack(spacing: 8) {
                        Image(systemName: "point.3.connected.trianglepath.dotted")
                            .font(.system(size: 26))
                            .foregroundStyle(Theme.faint)
                        Text("还没有端口池").font(.system(size: 14, weight: .semibold)).foregroundStyle(Theme.fg)
                        Text("配置路由器或本机 Clash 之后，每个走节点的账号会分到一个端口。")
                            .font(.system(size: 12.5))
                            .foregroundStyle(Theme.muted)
                        HStack(spacing: 8) {
                            Button("配置路由器…") { model.runInTerminal(.routerSetup) }.buttonStyle(.action)
                            Button("配置本机 Clash…") { model.runInTerminal(.localSetup) }.buttonStyle(.action)
                        }
                        .padding(.top, 4)
                    }
                    .padding(28)
                    .frame(maxWidth: .infinity)
                    .card(radius: 14)
                } else {
                    RouteCard(rows: rows, nodes: mapNodes(rows))
                    PortList(rows: rows)
                    DirectAccounts()
                }
            }
            .padding(.horizontal, 24)
            .padding(.top, 14)
            .padding(.bottom, 28)
            .frame(maxWidth: 1000)
            .frame(maxWidth: .infinity)
        }
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
            return MapNode(name: name, note: note, users: n.users)
        }
        for r in rows where !r.isFree {
            if let t = r.target, !out.contains(where: { $0.name == t }) {
                out.append(MapNode(name: t, note: "不在可选节点里", listed: false))
            }
        }
        return out
    }
}

/// 标题行：“分流”、实时的点和一句状态；右边是看哪边（都配置了才有）、刷新、同步路由器（待同步时是主按钮）。
struct RoutingHead: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 10) {
                title
                Spacer(minLength: 12)
                controls
            }
            VStack(alignment: .leading, spacing: 10) {
                title
                controls
            }
        }
        .padding(.horizontal, 4)
    }

    private var title: some View {
        HStack(spacing: 8) {
            Text("节点分流").font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.fg)
            if model.liveLoading {
                ProgressView().controlSize(.mini)
            } else {
                StatusDot(tone: liveTone, size: 7)
            }
            TimelineView(.periodic(from: .now, by: 30)) { context in
                Text(statusText(now: context.date.timeIntervalSince1970))
                    .font(.system(size: 12))
                    .foregroundStyle(Theme.muted)
                    .lineLimit(1)
            }
        }
    }

    private var liveTone: Tone {
        guard let l = model.live else { return .unknown }
        if l.router?.error != nil || l.local?.error != nil { return .bad }
        if l.hasDrift || (l.summary?.closed ?? 0) > 0 { return .warn }
        return .ok
    }

    private func statusText(now: Double) -> String {
        guard let l = model.live else { return model.liveError == nil ? "还没读到路由器上现在的样子" : "没读到" }
        var parts = ["实时"]
        if let at = l.generatedAt { parts.append(Texts.agoPhrase(now - at) + "查的") }
        parts.append(l.route == .local ? "本机 Clash 上现在的样子" : "路由器上现在的样子")
        return parts.joined(separator: " · ")
    }

    private var controls: some View {
        HStack(spacing: 8) {
            if model.routerConfigured && model.localConfigured {
                @Bindable var model = model
                SegControl(options: [(Route?.none, "自动"), (Route?.some(.router), "路由器"), (Route?.some(.local), "本机")],
                           selection: $model.liveRoute)
                    .help("看哪边。自动：按 cca 的设置（连得上路由器看路由器，连不上看本机 Clash）")
            }
            Button {
                model.refreshLive()
            } label: {
                Label("刷新", systemImage: "arrow.clockwise")
            }
            .buttonStyle(.quiet)
            .disabled(model.liveLoading)
            .help("重新读取路由器 / 本机 Clash 上各个端口现在的样子")
            if model.routerConfigured {
                Button {
                    model.syncRouter()
                } label: {
                    Label(model.syncActivity == nil ? "同步路由器" : "同步中…", systemImage: "wifi.router")
                }
                .buttonStyle(MagpieButtonStyle(kind: model.state?.routing?.sync == .pending ? .primary : .action))
                .disabled(model.syncActivity != nil)
                .help("把这台 Mac 的账号表同步到路由器（可能要重载 OpenClash / ShellCrash，会先问你）")
            }
        }
    }
}

/// 分流页的提示：待同步、下次运行 cca 时的改动、本机 Clash 不是最新的、连不上、读不到、选错节点。
struct RoutingBanners: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let routing = model.state?.routing
        let drift = model.live?.summary?.drift ?? 0
        VStack(spacing: 8) {
            if routing?.sync == .pending {
                Notice(.warn, systemImage: "arrow.triangle.2.circlepath", title: "路由器待同步",
                       message: "路由器上的端口池和这台 Mac 的账号表不一致" + (drift > 0 ? "（有 \(drift) 个槽位选错了节点）" : "")
                           + "：下次启动账号时会自动同步，也可以现在同步。")
            }
            if let notices = model.state?.notices, !notices.isEmpty {
                Notice(.info, systemImage: "info.circle", title: "下次运行 cca 时会有这些改动",
                       message: notices.map(Texts.notice).joined(separator: "\n"))
            }
            if routing?.localSync == .pending {
                Notice(.warn, systemImage: "laptopcomputer", title: "本机 Clash 的端口池不是最新的",
                       message: "下次在外面启动账号时会自动更新。")
            }
            if let e = model.live?.router?.error {
                routerError(e)
            }
            if let e = model.live?.local?.error {
                Notice(.bad, systemImage: "exclamationmark.triangle", title: "本机 Clash：\(Texts.liveError(e))") {
                    if e.code == .notConfigured {
                        Button("配置本机 Clash…") { model.runInTerminal(.localSetup) }.buttonStyle(.action)
                    }
                }
            }
            if let e = model.liveError {
                Notice(.bad, systemImage: "exclamationmark.triangle", title: "没能读取路由器 / 本机 Clash 的状态", message: e) {
                    Button("重试") { model.refreshLive() }.buttonStyle(.action)
                }
            }
            if drift > 0, routing?.sync != .pending {
                Notice(.warn, systemImage: "arrow.left.arrow.right", title: "有 \(drift) 个槽位选错了节点",
                       message: "同步路由器能修好。") {
                    if model.routerConfigured {
                        Button("同步路由器") { model.syncRouter() }.buttonStyle(.action).disabled(model.syncActivity != nil)
                    }
                }
            }
        }
    }

    /// 连不上路由器：只有配置了本机 Clash、又不是“只走路由器”时，账号才会改走本机 Clash；否则启动会失败。
    private func routerError(_ e: LiveIssue) -> some View {
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
        return Notice(.bad, systemImage: "wifi.exclamationmark", title: "路由器：\(Texts.liveError(e))", message: message) {
            if e.code == .unreachable, !model.localConfigured {
                Button("配置本机 Clash…") { model.runInTerminal(.localSetup) }.buttonStyle(.action)
            }
        }
    }
}

/// 分流卡片：上面一行是后端和地址、怎么选；中间是分流图；下面一句话和几个数，再是有问题的槽位。
struct RouteCard: View {
    @Environment(AppModel.self) private var model
    let rows: [SlotRow]
    let nodes: [MapNode]

    var body: some View {
        let used = rows.filter { !$0.isFree }
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 0) {
                    Text(hub.title).font(.system(size: 13.5, weight: .semibold)).foregroundStyle(Theme.fg)
                    Text(headTail).font(.system(size: 13)).foregroundStyle(Theme.muted)
                }
                .lineLimit(1)
                if let mode = modeLine {
                    Text(mode).font(.system(size: 12.5)).foregroundStyle(Theme.fg2).fixedSize(horizontal: false, vertical: true)
                }
            }
            if used.isEmpty {
                Text("还没有账号用到端口池。").font(.system(size: 12.5)).foregroundStyle(Theme.muted).padding(.vertical, 22)
            } else {
                HubMap(rows: used, nodes: nodes, hub: hub) { ref in
                    if model.tool != ref.tool { model.tool = ref.tool }
                    model.selection = .account(ref)
                }
                .padding(.vertical, 18)
            }
            Hairline()
            HStack(alignment: .center, spacing: 20) {
                Text(summary(used))
                    .font(.system(size: 13))
                    .foregroundStyle(Theme.fg)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 12)
                stats(used)
            }
            .padding(.vertical, 12)
            let bad = used.filter { $0.state == .drift || $0.state == .closed }
            if !bad.isEmpty {
                Hairline()
                VStack(alignment: .leading, spacing: 7) {
                    Text("有问题的槽位").font(.system(size: 12.5, weight: .semibold)).foregroundStyle(Theme.fg)
                    ForEach(bad) { r in
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            StatusDot(tone: Palette.slot(r.state), size: 7)
                            Text(issueLine(r)).font(.system(size: 12.5)).foregroundStyle(Theme.fg2)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
                .padding(.top, 12)
            }
        }
        .padding(.horizontal, 18)
        .padding(.top, 14)
        .padding(.bottom, 12)
        .card(radius: 14)
    }

    /// 中间那块：后端（OpenClash / ShellCrash / Clash Verge）、地址、状态。
    private var hub: HubInfo {
        let live = model.live
        if live?.route == .local {
            let l = live?.local
            let state: (String, Bool) = l?.error != nil ? ("出错了", false) : l?.running == false ? ("没开", false) : ("本机", true)
            return HubInfo(title: nonEmpty(l?.name) ?? "本机 Clash", subtitle: "这台 Mac", systemImage: "laptopcomputer",
                           badge: state.0, healthy: state.1)
        }
        if let r = live?.router, r.configured {
            let state: (String, Bool) = r.reachable == false ? ("连不上", false) : r.running == false ? ("没在运行", false) : ("运行中", true)
            return HubInfo(title: r.backend?.rawValue ?? "路由器", subtitle: r.host ?? model.state?.config?.router?.host,
                           systemImage: "wifi.router", badge: state.0, healthy: state.1)
        }
        if model.routerConfigured {
            return HubInfo(title: "路由器", subtitle: model.state?.config?.router?.host, systemImage: "wifi.router", badge: "还没查", healthy: true)
        }
        return HubInfo(title: nonEmpty(model.state?.config?.local?.name) ?? "本机 Clash", subtitle: "这台 Mac",
                       systemImage: "laptopcomputer", badge: "还没查", healthy: true)
    }

    private var headTail: String {
        var parts: [String] = []
        if model.live?.route != .local, let host = hub.subtitle { parts.append("路由器 \(host)") }
        let ports = rows.compactMap(\.port)
        if let lo = ports.min(), let hi = ports.max() { parts.append(lo == hi ? "端口 \(lo)" : "端口 \(lo)–\(hi)") }
        return parts.isEmpty ? "" : " · " + parts.joined(separator: " · ")
    }

    /// 怎么选（都配置了才说）、钩子和定时任务。
    private var modeLine: String? {
        var parts: [String] = []
        if model.routerConfigured && model.localConfigured, let pref = model.state?.config?.routePref {
            parts.append(Texts.routePref(pref))
        }
        if let r = model.live?.router, r.configured, model.live?.route != .local {
            parts.append("钩子" + (r.hook == true ? "正常" : r.hook == false ? "没装" : "未知")
                         + "，定时任务" + (r.cron == true ? "正常" : r.cron == false ? "没装" : "未知"))
        }
        return parts.isEmpty ? nil : parts.joined(separator: "。") + "。"
    }

    private func summary(_ used: [SlotRow]) -> String {
        guard model.live != nil else { return "还没读到路由器上现在的样子：点“刷新”读一次。" }
        let nodeCount = Set(used.compactMap(\.target)).count
        let drift = used.filter { $0.state == .drift }.count
        let closed = used.filter { $0.state == .closed }.count
        var s = "\(used.count) 个账号走 \(nodeCount) 个节点"
        if drift == 0 && closed == 0 { return s + "，都正常。" }
        var bad: [String] = []
        if drift > 0 { bad.append("\(drift) 个选错了节点") }
        if closed > 0 { bad.append("\(closed) 个连不上") }
        s += "；" + bad.joined(separator: "，") + "。"
        return s
    }

    private func stats(_ used: [SlotRow]) -> some View {
        let drift = used.filter { $0.state == .drift }.count
        let closed = used.filter { $0.state == .closed }.count
        let free = rows.filter(\.isFree).count
        return HStack(alignment: .top, spacing: 20) {
            stat("\(used.count)", "账号", Theme.fg)
            stat("\(drift)", "选错", drift > 0 ? Theme.drift : Theme.fg)
            stat("\(closed)", "连不上", closed > 0 ? Theme.red : Theme.fg)
            stat("\(free)", "空闲", Theme.fg)
        }
    }

    private func stat(_ n: String, _ label: String, _ color: Color) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(n).font(.system(size: 20, weight: .semibold).monospacedDigit()).foregroundStyle(color)
            Text(label).font(.system(size: 11)).foregroundStyle(Theme.muted)
        }
        .accessibilityElement(children: .combine)
    }

    private func issueLine(_ r: SlotRow) -> String {
        let slot = [r.group, r.port.map { "端口 \($0)" }].compactMap { $0 }.joined(separator: "，")
        if r.state == .drift {
            return "\(r.name)（\(slot)）选中的是 \(r.selected ?? "—")，应为 \(r.expected ?? "—")"
        }
        return "\(r.name)（\(slot)）连不上"
    }
}

/// 中间那块的内容。
struct HubInfo {
    let title: String
    let subtitle: String?
    let systemImage: String
    let badge: String
    let healthy: Bool
}

/// 分流图：左边一列账号卡片，中间是路由器 / 本机 Clash，右边一列节点。每个账号一条自己颜色的线：
/// 从卡片进到中间（各占一条道），再从中间连到它的节点（几个账号同一个节点时在节点那头错开）。
/// 选错节点：线是红色虚线、连到实际选中的节点，应选的那个留一道点线；连不上：后半段是灰色虚线。没人用的节点连一根细灰线。
struct HubMap: View {
    let rows: [SlotRow]
    let nodes: [MapNode]
    let hub: HubInfo
    let select: (AccountRef) -> Void

    var body: some View {
        GeometryReader { g in
            let geo = HubGeometry(width: g.size.width, rows: rows, nodes: nodes)
            ZStack(alignment: .topLeading) {
                wires(geo)
                ForEach(Array(rows.enumerated()), id: \.element.id) { i, r in
                    accountCard(r, compact: geo.compact)
                        .frame(width: geo.cardW, height: geo.cardH)
                        .position(x: geo.cardW / 2, y: geo.cardY[i])
                }
                hubCard
                    .frame(width: geo.hubW, height: geo.hubH)
                    .position(x: geo.hubX + geo.hubW / 2, y: geo.hubY)
                ForEach(Array(nodes.enumerated()), id: \.element.id) { j, n in
                    nodeRow(n)
                        .frame(width: geo.nodeW, height: HubGeometry.nodeH)
                        .position(x: geo.nodeX + geo.nodeW / 2, y: geo.nodeY[j])
                }
            }
        }
        .frame(height: HubGeometry.height(rows: rows.count, nodes: nodes.count))
        .frame(minWidth: 560)
    }

    // MARK: 线

    private func wires(_ geo: HubGeometry) -> some View {
        Canvas { ctx, _ in
            let solid = StrokeStyle(lineWidth: 1.5, lineCap: .round)
            let dashed = StrokeStyle(lineWidth: 1.5, lineCap: .round, dash: [3, 5])
            let dotted = StrokeStyle(lineWidth: 1.2, lineCap: .round, dash: [1, 4])
            // 没人用的节点：从中间连一根细灰线
            for (j, _) in nodes.enumerated() where geo.links[j].isEmpty {
                let p = curve(CGPoint(x: geo.hubX + geo.hubW, y: geo.hubY), CGPoint(x: geo.nodeX, y: geo.nodeY[j]))
                ctx.stroke(p, with: .color(Theme.line), style: solid)
            }
            for (i, r) in rows.enumerated() {
                let color = r.account?.lineColor ?? Color.gray
                let lane = geo.hubY + geo.lane(i)
                // 账号 → 中间
                let a = curve(CGPoint(x: geo.cardW, y: geo.cardY[i]), CGPoint(x: geo.hubX, y: lane))
                ctx.stroke(a, with: .color(color), style: solid)
                let from = CGPoint(x: geo.hubX + geo.hubW, y: lane)
                // 选错节点：应选的那个留一道点线
                if r.state == .drift, let e = nonEmpty(r.expected), let j = nodes.firstIndex(where: { $0.name == e }) {
                    ctx.stroke(curve(from, CGPoint(x: geo.nodeX, y: geo.nodeY[j])), with: .color(color.opacity(0.6)), style: dotted)
                }
                // 中间 → 节点
                guard let t = r.target, let j = nodes.firstIndex(where: { $0.name == t }) else { continue }
                let p = curve(from, CGPoint(x: geo.nodeX, y: geo.nodeY[j] + geo.fan(row: i, node: j)))
                switch r.state {
                case .drift?: ctx.stroke(p, with: .color(Theme.drift), style: dashed)
                case .closed?: ctx.stroke(p, with: .color(Theme.faint), style: dashed)
                default: ctx.stroke(p, with: .color(color), style: solid)
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

    // MARK: 左边的账号

    private func accountCard(_ r: SlotRow, compact: Bool) -> some View {
        let a = r.account
        let bad = r.state == .drift || r.state == .closed
        return Button {
            if let ref = r.ref { select(ref) }
        } label: {
            VStack(spacing: 3) {
                if !compact {
                    BadgeAvatar(badge: a?.badge, isAPI: a?.isAPI ?? false, size: 22)
                        .padding(.bottom, 1)
                }
                HStack(spacing: 5) {
                    Circle().fill(a?.lineColor ?? Color.gray).frame(width: 7, height: 7)
                    Text(a?.shortName ?? r.name)
                        .font(.system(size: 12.5, weight: .semibold))
                        .foregroundStyle(Theme.fg)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Text(cardNote(r))
                    .font(.system(size: 11).monospacedDigit())
                    .foregroundStyle(r.state == .drift ? Theme.drift : r.state == .closed ? Theme.red : Theme.muted)
                    .lineLimit(1)
            }
            .padding(.horizontal, 8)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .card(radius: 12, border: bad ? (r.state == .drift ? Theme.drift : Theme.red).opacity(0.5) : Theme.line)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help("看 \(r.name) 的详情")
        .accessibilityLabel("\(r.name)，\(cardNote(r))")
        .accessibilityHint("看这个账号的详情")
    }

    private func cardNote(_ r: SlotRow) -> String {
        switch r.state {
        case .drift?: return "选错 · 实际 \(r.selected ?? "—")"
        case .closed?: return "端口 \(r.port.map(String.init) ?? "—") 连不上"
        default:
            let tool = r.ref?.tool == .codex ? "ChatGPT · " : ""
            return tool + "端口 \(r.port.map(String.init) ?? "—")"
        }
    }

    // MARK: 中间

    private var hubCard: some View {
        VStack(spacing: 2) {
            Image(systemName: hub.systemImage)
                .font(.system(size: 19))
                .foregroundStyle(Theme.fg)
                .padding(.bottom, 3)
            Text(hub.title).font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.fg).lineLimit(1)
            if let s = hub.subtitle {
                Text(s).font(.system(size: 11).monospacedDigit()).foregroundStyle(Theme.muted).lineLimit(1)
            }
            Text(hub.badge)
                .font(.system(size: 10.5, weight: .semibold))
                .foregroundStyle(Theme.card)
                .padding(.horizontal, 8)
                .padding(.vertical, 1.5)
                .background(Capsule().fill(hub.healthy ? Theme.fg : Theme.red))
                .padding(.top, 5)
        }
        .padding(.horizontal, 8)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .card(radius: 12, border: Theme.fg)
        .padding(4)
        .background(Theme.card2, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .accessibilityElement(children: .combine)
    }

    // MARK: 右边的节点

    private func nodeRow(_ n: MapNode) -> some View {
        let linked = rows.filter { $0.target == n.name }
        let closed = rows.filter { $0.state == .closed && $0.target == n.name }
        let driftIn = rows.filter { $0.state == .drift && $0.target == n.name }
        let status: (String, Color) = {
            if !closed.isEmpty { return (closed.map { $0.account?.shortName ?? $0.name }.joined(separator: "、") + " 连不上", Theme.red) }
            if !driftIn.isEmpty { return (driftIn.map { $0.account?.shortName ?? $0.name }.joined(separator: "、") + " 选错到这里", Theme.drift) }
            if !n.listed { return ("不在可选节点里", Theme.amber) }
            if n.users.isEmpty { return ("没有账号在用", Theme.faint) }
            return ("\(n.users.count) 个账号", Theme.muted)
        }()
        let on = !linked.isEmpty
        return HStack(spacing: 8) {
            ZStack {
                if on {
                    Circle().fill(Theme.fg)
                } else {
                    Circle().strokeBorder(Theme.faint, lineWidth: 1.2)
                }
            }
            .frame(width: 8, height: 8)
            Text(n.name).font(.system(size: 13, weight: .semibold)).foregroundStyle(on ? Theme.fg : Theme.muted).lineLimit(1)
            if let note = n.note, n.listed {
                Text(note).font(.system(size: 11)).foregroundStyle(Theme.muted).lineLimit(1).truncationMode(.middle)
            }
            Spacer(minLength: 6)
            Text(status.0).font(.system(size: 11.5)).foregroundStyle(status.1).lineLimit(1).truncationMode(.middle)
        }
        .padding(.horizontal, 12)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .card(radius: 10, border: !driftIn.isEmpty ? Theme.drift.opacity(0.5) : Theme.line)
        .opacity(on || !n.users.isEmpty ? 1 : 0.75)
        .help([n.name, n.note, status.0].compactMap { $0 }.joined(separator: "，"))
        .accessibilityElement(children: .combine)
    }
}

/// 分流图的几何：三列的位置；左右两列各自在高度上居中；中间每个账号占一条道，节点那头几条线错开。
struct HubGeometry {
    static let nodeH: CGFloat = 38
    static let nodeGap: CGFloat = 8
    static let cardGap: CGFloat = 10

    let width: CGFloat
    let height: CGFloat
    let compact: Bool
    let cardW: CGFloat
    let cardH: CGFloat
    let hubX: CGFloat
    let hubW: CGFloat = 148
    let hubH: CGFloat = 118
    let hubY: CGFloat
    let nodeX: CGFloat
    let nodeW: CGFloat
    let cardY: [CGFloat]
    let nodeY: [CGFloat]
    /// 每个节点连着哪几行（线的目标）
    let links: [[Int]]
    private let laneStep: CGFloat

    static func cardHeight(rows: Int) -> CGFloat { rows > 6 ? 46 : 70 }

    static func height(rows: Int, nodes: Int) -> CGFloat {
        let left = CGFloat(rows) * cardHeight(rows: rows) + CGFloat(max(rows - 1, 0)) * cardGap
        let right = CGFloat(nodes) * nodeH + CGFloat(max(nodes - 1, 0)) * nodeGap
        return max(left, right, 130)
    }

    init(width: CGFloat, rows: [SlotRow], nodes: [MapNode]) {
        // 先算成局部变量：闭包里不能在所有存储属性赋值之前用 self
        let ch = HubGeometry.cardHeight(rows: rows.count)
        let h = HubGeometry.height(rows: rows.count, nodes: nodes.count)
        let cw = min(max(width * 0.17, 116), 150)
        let nw = min(max(width * 0.36, 210), 340)
        let hw: CGFloat = 148
        self.width = width
        compact = rows.count > 6
        cardH = ch
        height = h
        cardW = cw
        nodeW = nw
        nodeX = width - nw
        // 中间那块放在账号和节点之间，左边的线短一些
        hubX = cw + max(width - nw - cw - hw, 0) * 0.42
        hubY = h / 2

        let leftH = CGFloat(rows.count) * ch + CGFloat(max(rows.count - 1, 0)) * HubGeometry.cardGap
        let leftTop = (h - leftH) / 2
        cardY = rows.indices.map { leftTop + CGFloat($0) * (ch + HubGeometry.cardGap) + ch / 2 }

        let rightH = CGFloat(nodes.count) * HubGeometry.nodeH + CGFloat(max(nodes.count - 1, 0)) * HubGeometry.nodeGap
        let rightTop = (h - rightH) / 2
        nodeY = nodes.indices.map { rightTop + CGFloat($0) * (HubGeometry.nodeH + HubGeometry.nodeGap) + HubGeometry.nodeH / 2 }

        var l = Array(repeating: [Int](), count: nodes.count)
        for (i, r) in rows.enumerated() {
            if let t = r.target, let j = nodes.firstIndex(where: { $0.name == t }) { l[j].append(i) }
        }
        links = l
        // 道的间距：几条道都放得进中间那块（上下各留 16）
        laneStep = rows.count > 1 ? min(7, (118 - 32) / CGFloat(rows.count - 1)) : 0
    }

    /// 第 i 行在中间那块上的道（相对中线的偏移）。
    func lane(_ i: Int) -> CGFloat {
        guard cardY.count > 1 else { return 0 }
        return (CGFloat(i) - CGFloat(cardY.count - 1) / 2) * laneStep
    }

    /// 几条线连同一个节点时，在节点那头错开。
    func fan(row i: Int, node j: Int) -> CGFloat {
        let list = links[j]
        guard list.count > 1, let m = list.firstIndex(of: i) else { return 0 }
        let step = min(5, (HubGeometry.nodeH - 14) / CGFloat(list.count - 1))
        return (CGFloat(m) - CGFloat(list.count - 1) / 2) * step
    }
}

/// 端口的列表：每个槽位一行——端口、槽位名、账号、应选（选错时“应选 → 实际”）、状态。
struct PortList: View {
    @Environment(AppModel.self) private var model
    let rows: [SlotRow]

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Caption("端口")
            VStack(spacing: 0) {
                ForEach(Array(rows.enumerated()), id: \.element.id) { i, r in
                    if i > 0 { Hairline() }
                    row(r)
                }
            }
            .card()
        }
    }

    private func row(_ r: SlotRow) -> some View {
        HStack(spacing: 12) {
            Text(r.port.map(String.init) ?? "—")
                .font(.system(size: 13, weight: .semibold).monospacedDigit())
                .foregroundStyle(r.isFree ? Theme.faint : Theme.fg)
                .frame(width: 40, alignment: .leading)
            if let g = r.group { CodePill(text: g) }
            if r.isFree {
                Text("空闲").font(.system(size: 12.5)).foregroundStyle(Theme.faint)
            } else {
                HStack(spacing: 7) {
                    BadgeAvatar(badge: r.account?.badge, isAPI: r.account?.isAPI ?? false, size: 18)
                    Text(r.name).font(.system(size: 12.5)).foregroundStyle(Theme.fg).lineLimit(1).truncationMode(.middle)
                    if r.ref?.tool == .codex { Text("ChatGPT").font(.system(size: 11)).foregroundStyle(Theme.muted) }
                }
            }
            Spacer(minLength: 8)
            if !r.isFree {
                node(r)
                StatusLabel(tone: r.state.map { Palette.slot($0) } ?? .unknown,
                            text: r.state.map { Texts.slotState($0) } ?? "未查")
                    .frame(width: 76, alignment: .leading)
            }
        }
        .padding(.horizontal, 14)
        .frame(height: 38)
    }

    @ViewBuilder
    private func node(_ r: SlotRow) -> some View {
        if r.state == .drift, let sel = r.selected {
            HStack(spacing: 4) {
                Text(r.expected ?? "—").foregroundStyle(Theme.muted).strikethrough()
                Image(systemName: "arrow.right").font(.system(size: 9, weight: .semibold)).foregroundStyle(Theme.drift)
                Text(sel).foregroundStyle(Theme.drift)
            }
            .font(.system(size: 12.5, weight: .medium))
        } else {
            Text(r.expected ?? "—").font(.system(size: 12.5, weight: .medium)).foregroundStyle(Theme.fg2)
        }
    }
}

/// 不经节点的账号（直连的第三方 API 账号）：图里没有它们，在这里提一句。
struct DirectAccounts: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let direct = model.visibleTools.flatMap { model.accounts($0) }.filter { $0.isAPI && !$0.isRouted }
        if !direct.isEmpty {
            HStack(spacing: 8) {
                Text("不经节点、直连的：").foregroundStyle(Theme.muted)
                ForEach(direct) { a in
                    Button {
                        if model.tool != a.tool { model.tool = a.tool }
                        model.selection = .account(a.ref)
                    } label: {
                        HStack(spacing: 5) {
                            BadgeAvatar(badge: a.badge, isAPI: true, size: 16)
                            Text(a.displayName).foregroundStyle(Theme.fg2)
                        }
                    }
                    .buttonStyle(.plain)
                    .help("看 \(a.displayName) 的详情")
                }
            }
            .font(.system(size: 12))
            .padding(.horizontal, 4)
        }
    }
}
