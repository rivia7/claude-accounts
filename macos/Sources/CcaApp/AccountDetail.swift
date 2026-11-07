import SwiftUI

/// 一个账号的详情：头部、用量、出口与分流、出口检测，底部浮着操作条。
struct AccountDetail: View {
    @Environment(AppModel.self) private var model
    let ref: AccountRef

    var body: some View {
        if let a = model.account(ref) {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    DetailHeader(account: a)
                    if !a.isAPI { UsageCard(account: a) }
                    RoutingCard(account: a)
                    HealthCard(account: a)
                }
                .padding(.horizontal, 24)
                .padding(.top, 20)
                .padding(.bottom, 24)
                .frame(maxWidth: 820, alignment: .leading)
                .frame(maxWidth: .infinity)
            }
            .safeAreaInset(edge: .bottom) {
                ActionBar(account: a)
            }
            .navigationTitle(a.displayName)
        } else {
            ContentUnavailableView("没有这个账号", systemImage: "person.crop.circle.badge.questionmark",
                                   description: Text("账号表里已经没有它了。"))
        }
    }
}

struct DetailHeader: View {
    @Environment(AppModel.self) private var model
    let account: Account

    var body: some View {
        let a = account
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .center, spacing: 14) {
                BadgeAvatar(badge: a.badge, isAPI: a.isAPI, size: 54)
                VStack(alignment: .leading, spacing: 5) {
                    Text(a.displayLabel)
                        .font(.title2.weight(.semibold))
                        .textSelection(.enabled)
                        .lineLimit(2)
                    HStack(spacing: 6) {
                        if a.isAPI {
                            Tag(text: "第三方 API", color: .blue, systemImage: "key.horizontal")
                            if let m = nonEmpty(a.api?.model) { Tag(text: m) }
                        } else {
                            if let p = nonEmpty(a.plan) { Tag(text: p) }
                            Tag(text: Texts.login(a.login, loginEmail: a.loginEmail), color: Palette.login(a.login))
                        }
                        if a.isLast { Tag(text: "上次在命令行里用的", systemImage: "terminal") }
                        if a.appRunning { Tag(text: "\(model.appLabel(a.tool)) 开着", color: .green, systemImage: "macwindow") }
                    }
                }
                Spacer(minLength: 8)
                if a.login == .loggedOut || a.login == .emailMismatch {
                    Button {
                        model.runInTerminal(model.loginHandoff(a))
                    } label: {
                        Label(a.login == .emailMismatch ? "重新登录…" : "登录…", systemImage: "person.badge.key")
                    }
                    .buttonStyle(.glass)
                    .help("在终端里运行 \(model.loginHandoff(a).display)")
                }
            }
            let reasons = disabledReasons(a)
            if !reasons.isEmpty {
                Text(reasons.joined(separator: "；"))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    /// 按钮为什么不能用（cca 给的原因码）。API 账号没有这两个按钮，不说。
    private func disabledReasons(_ a: Account) -> [String] {
        if a.isAPI { return [] }
        var out: [String] = []
        let label = model.appLabel(a.tool)
        if !a.can.app, let w = a.canWhy["app"] { out.append("打开 App：" + Texts.canWhy(w, appLabel: label, tool: a.tool)) }
        if !a.can.web, let w = a.canWhy["web"] { out.append("网页版：" + Texts.canWhy(w, appLabel: label, tool: a.tool)) }
        return out
    }
}

/// 出口与分流：节点、端口、槽位、锁定、路由器 / 本机上这个槽位现在的状态。
struct RoutingCard: View {
    @Environment(AppModel.self) private var model
    let account: Account

    var body: some View {
        let a = account
        let live = model.liveSlot(for: a.ref)
        Card("出口与分流", systemImage: "point.3.filled.connected.trianglepath.dotted") {
            if let live {
                GlassPill(text: Texts.slotState(live.state), color: Palette.slot(live.state))
            }
        } content: {
            VStack(alignment: .leading, spacing: 10) {
                Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 16, verticalSpacing: 8) {
                    if a.isAPI {
                        GridRow {
                            label("接口")
                            Text(a.api?.host ?? "—").font(.system(.body, design: .monospaced)).textSelection(.enabled)
                        }
                        if let m = nonEmpty(a.api?.model) {
                            GridRow {
                                label("模型")
                                Text(m).textSelection(.enabled)
                            }
                        }
                    }
                    GridRow {
                        label("节点")
                        HStack(spacing: 6) {
                            Text(a.node ?? (a.isAPI ? "直连（不经节点）" : "—")).fontWeight(.medium)
                            if a.pinned {
                                Label("维护者锁定", systemImage: "lock.fill")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                    .help("维护者的映射锁定了这个账号的节点（\(a.pinnedNode ?? "—")），这里不能改")
                            }
                            if let local = localName(a), local != a.node {
                                Text("Clash Verge 里叫 \(local)").font(.caption).foregroundStyle(.secondary)
                            }
                        }
                    }
                    if let port = a.port {
                        GridRow {
                            label("端口")
                            Text(portText(port)).monospacedDigit().textSelection(.enabled)
                        }
                    }
                    if let group = a.group {
                        GridRow {
                            label("槽位")
                            Text(a.slot.map { "\(group)（第 \($0) 个）" } ?? group).monospacedDigit()
                        }
                    }
                    if let live {
                        GridRow {
                            label(Texts.route(model.live?.route))
                            liveLine(live)
                        }
                    }
                }
                if a.isRouted, model.state?.routing?.sync == .pending, model.routerConfigured {
                    HStack(spacing: 8) {
                        Image(systemName: "arrow.triangle.2.circlepath").foregroundStyle(.orange)
                        Text("路由器还没同步：下次启动账号时会同步，也可以现在同步。").font(.callout)
                        Spacer()
                        Button("同步路由器") { model.syncRouter() }
                            .buttonStyle(.bordered)
                            .disabled(model.syncActivity != nil)
                    }
                }
            }
        }
    }

    private func label(_ s: String) -> some View {
        Text(s).foregroundStyle(.secondary).gridColumnAlignment(.trailing)
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
        return "\(port)"
    }

    private func liveLine(_ s: LiveSlot) -> some View {
        var parts: [String] = []
        if let sel = s.selected {
            parts.append(s.state == .drift ? "选中的是 \(sel)，应为 \(s.expected ?? "—")" : "选中 \(sel)")
        } else {
            parts.append("拿不到选中的节点")
        }
        if s.listening == false { parts.append("端口没在监听") }
        if s.reachable == false { parts.append("这台 Mac 连不上这个端口") }
        if s.inKernel == false { parts.append("内核里没有这个组") }
        return HStack(spacing: 6) {
            if s.state != .ok {
                Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(Palette.slot(s.state))
            }
            Text(parts.joined(separator: " · "))
        }
    }
}

/// 底部浮着的操作条（玻璃）。官方账号：打开 App（主按钮）、网页版、检测出口、刷新用量；第三方 API 账号只有检测出口（主按钮）。
/// 进度（来自 step 事件）单独一行放在按钮上面；放不下时按钮只显示图标（名字在提示和读屏里）。
struct ActionBar: View {
    @Environment(AppModel.self) private var model
    let account: Account

    var body: some View {
        let busy = model.activity(account.ref)
        VStack(spacing: 8) {
            if let busy {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text(progressText(busy)).font(.callout)
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 7)
                .glassEffect(.regular, in: .capsule)
                .accessibilityElement(children: .combine)
            }
            ViewThatFits(in: .horizontal) {
                buttons(iconOnly: false, busy: busy != nil)
                buttons(iconOnly: true, busy: busy != nil)
            }
        }
        .controlSize(.large)
        .padding(.horizontal, 20)
        .padding(.bottom, 16)
        .padding(.top, 6)
    }

    private func buttons(iconOnly: Bool, busy: Bool) -> some View {
        let a = account
        let appLabel = model.appLabel(a.tool)
        return GlassRow(spacing: 10) {
            if !a.isAPI {
                Button {
                    model.openApp(a.ref)
                } label: {
                    Label("打开 \(appLabel)", systemImage: "macwindow")
                }
                .buttonStyle(.glassProminent)
                .disabled(!a.can.app || busy)
                .help(reason("app", a) ?? "用这个账号的出口和独立的资料目录打开 \(appLabel)")
                Button {
                    model.openWeb(a.ref)
                } label: {
                    Label("网页版", systemImage: "safari")
                }
                .buttonStyle(.glass)
                .disabled(!a.can.web || busy)
                .help(reason("web", a) ?? "在这个账号自己的 Chrome 资料里打开网页版，走账号的出口")
            }
            Button {
                model.checkExit(a.ref)
            } label: {
                Label("检测出口", systemImage: "checkmark.shield")
            }
            .glassButton(prominent: a.isAPI)
            .disabled(!a.can.check || busy)
            .help(a.isAPI ? "检测接口连不连得上" : "检测出口 IP、地区、纯净度和 DNS / WebRTC 泄露（会访问检测网站）")
            if !a.isAPI {
                Button {
                    model.refreshUsage(tool: a.tool, key: a.key, manual: true)
                } label: {
                    Label("刷新用量", systemImage: "arrow.clockwise")
                }
                .buttonStyle(.glass)
                .disabled(!a.can.usage || model.isUsageRefreshing(a.ref))
                .help(reason("usage", a) ?? "经这个账号的出口查一次用量")
            }
        }
        .labelStyle(iconOnly: iconOnly)
    }

    private func reason(_ action: String, _ a: Account) -> String? {
        guard let w = a.canWhy[action] else { return nil }
        let allowed: Bool
        switch action {
        case "app": allowed = a.can.app
        case "web": allowed = a.can.web
        default: allowed = a.can.usage
        }
        return allowed ? nil : Texts.canWhy(w, appLabel: model.appLabel(a.tool), tool: a.tool)
    }

    private func progressText(_ act: Activity) -> String {
        let appLabel = model.appLabel(account.tool)
        if let s = act.step { return Texts.step(s, appLabel: appLabel) }
        switch act.kind {
        case .app: return "打开 \(appLabel)…"
        case .web: return "打开网页版…"
        case .check: return "检测出口…"
        case .sync: return "同步路由器…"
        }
    }
}

extension View {
    /// 窄的时候按钮只显示图标（Label 的文字照样给读屏软件用，名字也在提示里）。
    @ViewBuilder
    func labelStyle(iconOnly: Bool) -> some View {
        if iconOnly {
            self.labelStyle(.iconOnly)
        } else {
            self.labelStyle(.titleAndIcon)
        }
    }

    /// 主按钮用 .glassProminent，其它 .glass。
    @ViewBuilder
    func glassButton(prominent: Bool) -> some View {
        if prominent {
            self.buttonStyle(.glassProminent)
        } else {
            self.buttonStyle(.glass)
        }
    }
}
