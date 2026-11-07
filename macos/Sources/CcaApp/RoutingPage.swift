import SwiftUI

/// 一个槽位的一行：账号表算出来的（state）加上路由器 / 本机上现在的样子（live）。
struct SlotRow: Identifiable {
    let id: Int
    let group: String?
    let port: Int?
    let ref: AccountRef?
    let account: Account?
    let expected: String
    let selected: String?
    let state: SlotState?
}

/// 节点分流页：走哪边、后端、钩子 / 定时任务、同步状态，槽位表，可选节点和用它的账号。
/// 这一页可见时每 60 秒读一次 `_gui live`（Scheduler）。
struct RoutingPage: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                RoutingHeader()
                RoutingBanners()
                let rows = slotRows()
                if rows.isEmpty {
                    ContentUnavailableView("还没有端口池", systemImage: "point.3.connected.trianglepath.dotted",
                                           description: Text("配置路由器或本机 Clash 之后，每个走节点的账号会分到一个端口。"))
                } else {
                    SlotTable(rows: rows)
                        .frame(height: CGFloat(rows.count) * 30 + 34)
                }
                NodesSection()
            }
            .padding(24)
            .frame(maxWidth: 980, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
        .navigationTitle("节点分流")
    }

    private func slotRows() -> [SlotRow] {
        let slots = model.state?.routing?.slots ?? []
        let live = model.live
        let local = live?.route == .local
        return slots.map { s in
            let l = live?.slot(s.slot)
            let ref = s.ref
            return SlotRow(id: s.slot, group: s.group ?? l?.group, port: s.port ?? l?.port, ref: ref,
                           account: ref.flatMap { model.account($0) },
                           expected: l?.expected ?? (local ? s.localNode : s.node) ?? "—",
                           selected: l?.selected, state: l?.state ?? (s.isEmpty ? .empty : nil))
        }
    }
}

struct RoutingHeader: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        // 宽的时候信息和按钮左右排；窄的时候按钮（和看哪边的选择）放到下面
        ViewThatFits(in: .horizontal) {
            HStack(alignment: .top, spacing: 16) {
                info
                Spacer(minLength: 8)
                VStack(alignment: .trailing, spacing: 8) {
                    actions
                    routePicker
                }
            }
            VStack(alignment: .leading, spacing: 12) {
                info
                actions
                routePicker
            }
        }
    }

    private var info: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Text("现在走").foregroundStyle(.secondary)
                Text(routeTitle).font(.title3.weight(.semibold))
                if model.liveLoading { ProgressView().controlSize(.small) }
            }
            ViewThatFits(in: .horizontal) {
                GlassRow(spacing: 8) { pills }
                VStack(alignment: .leading, spacing: 6) { pills }
            }
            Text(detailLine).font(.caption).foregroundStyle(.secondary)
        }
    }

    /// 后端、本机 Clash、钩子、定时任务：状态写在文字里（不只靠颜色）。
    @ViewBuilder
    private var pills: some View {
        let cfg = model.state?.config
        let live = model.live
        if let r = live?.router, r.configured {
            let name = r.backend?.rawValue ?? "路由器"
            let state = r.reachable == false ? "连不上" : r.running == false ? "没在运行" : r.running == true ? "运行中" : "已配置"
            GlassPill(text: "\(name) \(state)", color: r.reachable == false || r.running == false ? .red : .green,
                      systemImage: "wifi.router")
            statusPill("钩子", r.hook)
            statusPill("定时任务", r.cron)
        } else if cfg?.routerConfigured == true, let host = cfg?.router?.host {
            GlassPill(text: "路由器 \(host)", color: .secondary, systemImage: "wifi.router")
        }
        if let l = live?.local, l.configured {
            let name = nonEmpty(l.name) ?? "本机 Clash"
            let state = l.running == true ? "运行中" : l.running == false ? "没开" : "没看这一边"
            GlassPill(text: "\(name) \(state)", color: l.running == false ? .red : (l.running == true ? .green : .secondary),
                      systemImage: "laptopcomputer")
        } else if cfg?.localConfigured == true {
            GlassPill(text: nonEmpty(cfg?.local?.name) ?? "本机 Clash", color: .secondary, systemImage: "laptopcomputer")
        }
    }

    private var actions: some View {
        GlassRow(spacing: 8) {
            Button {
                model.refreshLive()
            } label: {
                Label("刷新", systemImage: "arrow.clockwise")
            }
            .buttonStyle(.glass)
            .disabled(model.liveLoading)
            if model.routerConfigured {
                Button {
                    model.syncRouter()
                } label: {
                    Label(model.syncActivity == nil ? "同步路由器" : "同步中…", systemImage: "arrow.triangle.2.circlepath")
                }
                .buttonStyle(.glassProminent)
                .disabled(model.syncActivity != nil)
            }
        }
    }

    @ViewBuilder
    private var routePicker: some View {
        if model.routerConfigured && model.localConfigured {
            @Bindable var model = model
            Picker("看哪边", selection: $model.liveRoute) {
                Text("自动").tag(Route?.none)
                Text("路由器").tag(Route?.some(.router))
                Text("本机 Clash").tag(Route?.some(.local))
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .frame(width: 230)
        }
    }

    private var routeTitle: String {
        if let l = model.live {
            switch l.route {
            case .router: return "路由器" + (l.router?.host.map { " \($0)" } ?? "")
            case .local: return nonEmpty(l.local?.name) ?? "本机 Clash"
            default: return "无（没配置路由器和本机 Clash）"
            }
        }
        if model.routerConfigured { return "路由器" }
        if model.localConfigured { return "本机 Clash" }
        return "无"
    }

    private var detailLine: String {
        var parts: [String] = []
        if let pref = model.state?.config?.routePref { parts.append(Texts.routePref(pref)) }
        let ports = (model.state?.routing?.slots ?? []).compactMap(\.port)
        if let lo = ports.min(), let hi = ports.max() {
            parts.append(lo == hi ? "端口 \(lo)" : "端口 \(lo)–\(hi)")
        }
        if let at = model.live?.generatedAt {
            parts.append(Texts.agoPhrase(Date().timeIntervalSince1970 - at) + "查的")
        }
        return parts.joined(separator: " · ")
    }

    private func statusPill(_ name: String, _ v: Bool?) -> some View {
        GlassPill(text: name + (v == true ? "正常" : v == false ? "没装" : "未知"),
                  color: v == true ? .green : v == false ? .orange : .secondary,
                  systemImage: v == true ? "checkmark" : v == false ? "xmark" : "questionmark")
    }
}

struct RoutingBanners: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let routing = model.state?.routing
        let drift = model.live?.summary?.drift ?? 0
        GlassBannerStack {
            if routing?.sync == .pending {
                GlassBanner(systemImage: "arrow.triangle.2.circlepath", tint: .orange, title: "路由器待同步",
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
                GlassBanner(systemImage: "info.circle", tint: .accentColor, title: "下次运行 cca 时会有这些改动",
                            message: notices.map(Texts.notice).joined(separator: "\n"))
            }
            if routing?.localSync == .pending {
                GlassBanner(systemImage: "laptopcomputer", tint: .orange, title: "本机 Clash 的端口池不是最新的",
                            message: "下次在外面启动账号时会自动更新。")
            }
            if let e = model.live?.router?.error {
                routerErrorBanner(e)
            }
            if let e = model.live?.local?.error {
                GlassBanner(systemImage: "exclamationmark.triangle", tint: .red, title: "本机 Clash：\(Texts.liveError(e))") {
                    if e.code == .notConfigured {
                        Button("配置本机 Clash…") { model.runInTerminal(.localSetup) }.buttonStyle(.bordered)
                    }
                }
            }
            if let e = model.liveError {
                GlassBanner(systemImage: "exclamationmark.triangle", tint: .red, title: "没能读取路由器 / 本机 Clash 的状态", message: e) {
                    Button("重试") { model.refreshLive() }.buttonStyle(.bordered)
                }
            }
            if drift > 0, routing?.sync != .pending {
                GlassBanner(systemImage: "arrow.left.arrow.right", tint: .orange, title: "有 \(drift) 个槽位选错了节点",
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
        return GlassBanner(systemImage: "wifi.exclamationmark", tint: .red, title: "路由器：\(Texts.liveError(e))", message: message) {
            if e.code == .unreachable, !model.localConfigured {
                Button("配置本机 Clash…") { model.runInTerminal(.localSetup) }.buttonStyle(.bordered)
            }
        }
    }
}

struct SlotTable: View {
    @Environment(AppModel.self) private var model
    let rows: [SlotRow]

    var body: some View {
        Table(rows) {
            TableColumn("槽位") { r in
                Text(r.group ?? "—").monospacedDigit().foregroundStyle(r.ref == nil ? .secondary : .primary)
            }
            .width(min: 60, ideal: 70)
            TableColumn("端口") { r in
                Text(r.port.map(String.init) ?? "—").monospacedDigit().foregroundStyle(r.ref == nil ? .secondary : .primary)
            }
            .width(min: 50, ideal: 60)
            TableColumn("账号") { r in
                if let a = r.account {
                    HStack(spacing: 6) {
                        BadgeAvatar(badge: a.badge, isAPI: a.isAPI, size: 18)
                        Text(a.displayName).lineLimit(1)
                        if a.tool != .claude { Tag(text: Texts.product(a.tool)) }
                        if a.pinned {
                            Image(systemName: "lock.fill").font(.system(size: 9)).foregroundStyle(.secondary)
                                .accessibilityLabel("维护者锁定")
                        }
                    }
                } else if let ref = r.ref {
                    Text(ref.key).foregroundStyle(.secondary)
                } else {
                    Text("空闲").foregroundStyle(.tertiary)
                }
            }
            .width(min: 160, ideal: 240)
            TableColumn("应选") { r in
                Text(r.ref == nil ? "—" : r.expected).foregroundStyle(r.ref == nil ? .tertiary : .primary)
            }
            .width(min: 70, ideal: 100)
            TableColumn("实际") { r in
                HStack(spacing: 4) {
                    if r.state == .drift {
                        Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange).imageScale(.small)
                    }
                    Text(r.ref == nil ? "—" : (r.selected ?? "—")).foregroundStyle(r.ref == nil ? .secondary : .primary)
                }
            }
            .width(min: 70, ideal: 100)
            TableColumn("状态") { r in
                if let s = r.state {
                    Tag(text: Texts.slotState(s), color: Palette.slot(s))
                } else {
                    Text("未查").foregroundStyle(.tertiary)
                }
            }
            .width(min: 70, ideal: 90)
        }
        .scrollDisabled(true)
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(.separator.opacity(0.6), lineWidth: 0.5))
    }
}

struct NodesSection: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let nodes = model.state?.routing?.nodes ?? []
        if !nodes.isEmpty {
            VStack(alignment: .leading, spacing: 10) {
                Text("可选节点").font(.headline)
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 210), spacing: 12, alignment: .top)], alignment: .leading, spacing: 12) {
                    ForEach(nodes) { n in
                        NodeCard(node: n)
                    }
                }
            }
        }
    }
}

struct NodeCard: View {
    @Environment(AppModel.self) private var model
    let node: NodeUse

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Image(systemName: "globe").foregroundStyle(.secondary)
                Text(node.name).font(.callout.weight(.semibold))
                Spacer()
                Text("\(node.users.count) 个账号").font(.caption).foregroundStyle(.secondary)
            }
            if let local = nonEmpty(node.localName), local != node.name {
                Text("Clash Verge 里叫 \(local)").font(.caption).foregroundStyle(.secondary)
            }
            if node.users.isEmpty {
                Text("没有账号在用").font(.caption).foregroundStyle(.tertiary)
            } else {
                ForEach(node.users, id: \.self) { ref in
                    let a = model.account(ref)
                    HStack(spacing: 6) {
                        BadgeAvatar(badge: a?.badge, isAPI: a?.isAPI ?? false, size: 16)
                        Text(a?.displayName ?? ref.key).font(.caption).lineLimit(1)
                        if ref.tool != .claude { Text(Texts.product(ref.tool)).font(.caption2).foregroundStyle(.secondary) }
                    }
                }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(.background.secondary))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(.separator.opacity(0.6), lineWidth: 0.5))
        .accessibilityElement(children: .combine)
    }
}
