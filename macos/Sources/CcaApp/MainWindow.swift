import AppKit
import SwiftUI

/// 主窗口的两页。
enum Page: Hashable {
    case accounts, routing
}

extension AppModel {
    /// 现在是哪一页：selection 是 `.routing` 时是分流页，否则是账号页（selection 是哪个账号就展开哪个）。
    var page: Page {
        get { selection == .routing ? .routing : .accounts }
        set {
            guard newValue != page else { return }
            selection = newValue == .routing ? .routing : nil
        }
    }

    /// 走哪边（菜单栏面板）：读到 live 时按它，否则按设置。
    var routeSummary: String {
        if let l = live {
            switch l.route {
            case .router: return "路由器" + (l.router?.backend.map { " · \($0.rawValue)" } ?? "")
            case .local: return nonEmpty(l.local?.name) ?? "本机 Clash"
            default: break
            }
        }
        if routerConfigured { return "路由器" }
        if localConfigured { return nonEmpty(state?.config?.local?.name) ?? "本机 Clash" }
        return "没配置路由器和本机 Clash"
    }

    /// 分流有问题时的一句话和语气：待同步、有槽位连不上、选错节点（分流页签上的点、菜单栏面板）。
    var routeIssue: (String, Tone)? {
        if state?.routing?.sync == .pending { return ("待同步", .warn) }
        if let s = live?.summary {
            if s.closed > 0 { return ("\(s.closed) 个槽位连不上", .bad) }
            if s.drift > 0 { return ("\(s.drift) 个槽位选错了节点", .warn) }
        }
        return nil
    }
}

/// 主窗口：按连接状态显示引导，或者工具栏的页签 + 这一页。终端交接和失败说明是这个窗口上的弹层。
struct MainWindow: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        @Bindable var model = model
        Group {
            switch model.phase {
            case .ready:
                VStack(spacing: 0) {
                    TopBanners()
                    switch model.page {
                    case .accounts: AccountsPage()
                    case .routing: RoutingPage()
                    }
                }
            case .starting:
                VStack(spacing: 12) {
                    ProgressView()
                    Text("正在连接 cca…").font(.system(size: 13)).foregroundStyle(Theme.muted)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            default:
                OnboardingView()
            }
        }
        .frame(minWidth: 780, minHeight: 540)
        .containerBackground(Theme.bg, for: .window)
        .toolbar { MainToolbar() }
        .toolbar(removing: .title)
        .overlay(alignment: .bottom) {
            if let t = model.toast {
                ToastView(toast: t)
                    .padding(.bottom, 28)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
                    .id(t.id)
            }
        }
        .animation(.snappy, value: model.toast)
        .sheet(item: $model.sheet) { s in
            SheetView(sheet: s).environment(model)
        }
        .background(WindowObserver { visible in model.setMainVisible(visible) })
        .onAppear {
            WindowOpener.shared.open = { openWindow(id: "main") }
        }
    }
}

/// 工具栏（参考 magpie 的页头）：左边品牌，中间“账号 / 分流”两个页签（分流有问题时“分流”带个点），右边刷新、添加。
struct MainToolbar: ToolbarContent {
    @Environment(AppModel.self) private var model

    var body: some ToolbarContent {
        ToolbarItem(placement: .navigation) {
            BrandLabel()
                .padding(.horizontal, 6)
        }
        .sharedBackgroundVisibility(.hidden)
        ToolbarItem(placement: .principal) {
            if model.phase == .ready {
                @Bindable var model = model
                Picker("页面", selection: $model.page) {
                    Text("账号").tag(Page.accounts)
                    Text(model.routeIssue == nil ? "分流" : "分流 •").tag(Page.routing)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
                .help(model.routeIssue.map { "分流：\($0.0)" } ?? "")
            }
        }
        ToolbarItemGroup(placement: .primaryAction) {
            Button {
                model.refreshAll()
            } label: {
                Label("刷新", systemImage: "arrow.clockwise")
            }
            .help("重新读取账号状态，并经各账号的出口查用量（⌘R）")
            .disabled(model.phase != .ready)
            Menu {
                Button("添加 Claude 账号…") { model.runInTerminal(.addAccount(prefix: [], product: "Claude")) }
                Button("添加 ChatGPT 账号…") { model.runInTerminal(.addAccount(prefix: model.prefix(.codex), product: "ChatGPT")) }
                Button("添加第三方 API 账号…") { model.runInTerminal(.addAPI) }
                Divider()
                Button("编辑 Claude 账号表…") { model.runInTerminal(.edit(prefix: model.prefix(.claude))) }
                if model.codexEnabled {
                    Button("编辑 ChatGPT 账号表…") { model.runInTerminal(.edit(prefix: model.prefix(.codex))) }
                }
                Divider()
                Button("配置路由器…") { model.runInTerminal(.routerSetup) }
                Button("配置本机 Clash…") { model.runInTerminal(.localSetup) }
            } label: {
                Label("添加", systemImage: "plus")
            }
            .help("添加账号、编辑账号表、配置路由器（在终端里完成）")
        }
    }
}

/// 页面上方的提示：版本不同、账号表校验不过、共享设置有问题、文件读不了、后台刷新用量要同意或失败了、读 state 出错。
struct TopBanners: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if hasBanners {
            VStack(spacing: 8) {
                if model.versionMismatch, let v = model.hello?.ccaVersion {
                    Notice(.info, systemImage: "arrow.down.circle",
                           title: model.hello?.gui?.stagedVersion == v ? "新版本的 cca App 已经下载好" : "cca 已经更新到 \(v)",
                           message: model.updating ? "正在下载和 cca 同版本的 App…" : "这个 App 是 \(model.appVersion)。更新后会自动重新打开。") {
                        if model.updating { ProgressView().controlSize(.small) }
                        Button("更新并重新打开") { model.updateAndRelaunch() }
                            .buttonStyle(.primary)
                            .disabled(model.updating)
                    }
                }
                ForEach(model.invalidTables, id: \.tool) { item in
                    Notice(.bad, systemImage: "tablecells.badge.ellipsis",
                           title: "\(Texts.product(item.tool)) 的账号表校验不过：启动账号会失败",
                           message: item.issues.map(\.message).filter { !$0.isEmpty }.joined(separator: "\n")) {
                        Button("在终端里编辑…") { model.runInTerminal(.edit(prefix: model.prefix(item.tool))) }
                            .buttonStyle(.action)
                    }
                }
                ForEach(model.visibleTools, id: \.self) { t in
                    if let problems = model.toolState(t)?.settingsProblems, !problems.isEmpty {
                        Notice(.warn, systemImage: "gearshape.2",
                               title: "\(Texts.product(t)) 的共享设置里有会盖掉账号代理、接口或凭证的项",
                               message: problems.joined(separator: "、") + "。启动账号前 cca 会检查，可能要先在终端里处理。")
                    }
                }
                ForEach(Array((model.state?.errors ?? []).enumerated()), id: \.offset) { _, e in
                    Notice(.bad, systemImage: "exclamationmark.octagon",
                           title: (e.tool.map { Texts.product($0) + "：" } ?? "") + Texts.stateError(e),
                           message: nonEmpty(e.message) == nil || e.message == Texts.stateError(e) ? "" : e.message)
                }
                ForEach(model.visibleTools, id: \.self) { t in
                    if let cs = model.usageNeedsConsent[t], let first = cs.first {
                        Notice(.warn, systemImage: "hand.raised", title: "刷新 \(Texts.product(t)) 的用量要你同意", message: first.title) {
                            Button("现在刷新") { model.refreshUsage(tool: t, manual: true) }
                                .buttonStyle(.action)
                        }
                    } else if let f = model.usageFailure[t] {
                        Notice(.warn, systemImage: "exclamationmark.triangle", title: "后台刷新 \(Texts.product(t)) 的用量没有成功",
                               message: f.message) {
                            Button("重试") { model.refreshUsage(tool: t, manual: true) }
                                .buttonStyle(.action)
                        }
                    }
                }
                if let e = model.stateError {
                    Notice(.bad, systemImage: "exclamationmark.triangle", title: "没能读取账号状态", message: e) {
                        Button("重试") { Task { await model.connect() } }.buttonStyle(.action)
                    }
                }
            }
            .padding(.horizontal, 24)
            .padding(.top, 12)
            .frame(maxWidth: 1000)
            .frame(maxWidth: .infinity)
        }
    }

    private var hasBanners: Bool {
        model.versionMismatch || !model.invalidTables.isEmpty || !(model.state?.errors ?? []).isEmpty
            || !model.usageNeedsConsent.isEmpty || !model.usageFailure.isEmpty || model.stateError != nil
            || model.visibleTools.contains { !(model.toolState($0)?.settingsProblems ?? []).isEmpty }
    }
}

/// 窗口底部浮着的提示（玻璃）：几秒后自己消失。
struct ToastView: View {
    let toast: Toast

    var body: some View {
        let tone: Tone = {
            switch toast.style {
            case .success: return .ok
            case .warning: return .warn
            case .info: return .info
            }
        }()
        HStack(spacing: 8) {
            StatusDot(tone: tone, size: 8)
            Text(toast.text).font(.system(size: 12.5)).foregroundStyle(Theme.fg).lineLimit(3).textSelection(.enabled)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 9)
        .glassEffect(.regular, in: .capsule)
        .accessibilityElement(children: .combine)
    }
}

/// 还不能用时的引导：找不到 cca、cca 没装好、接口版本不对、出错了。
struct OnboardingView: View {
    @Environment(AppModel.self) private var model

    static let installCommand = "curl -fsSL https://github.com/rivia7/claude-accounts/releases/latest/download/install.sh | bash"

    var body: some View {
        VStack(spacing: 18) {
            Image(nsImage: NSApp.applicationIconImage)
                .resizable()
                .frame(width: 88, height: 88)
            card
                .frame(maxWidth: 540)
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func panel<Content: View>(_ title: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(title).font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.fg)
            content()
        }
        .font(.system(size: 13))
        .foregroundStyle(Theme.fg2)
        .padding(20)
        .frame(maxWidth: .infinity, alignment: .leading)
        .card(radius: 14)
    }

    @ViewBuilder
    private var card: some View {
        switch model.phase {
        case .missingCca:
            panel("先安装 cca") {
                Text("这个 App 是 cca 的图形界面，需要先装好 cca。在“终端”里运行：")
                Text(Self.installCommand)
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundStyle(Theme.fg)
                    .textSelection(.enabled)
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Theme.pill, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                HStack(spacing: 8) {
                    Button("复制命令") { model.copy(Self.installCommand) }.buttonStyle(.primary)
                    Button("装好了，重新查找") { Task { await model.connect() } }.buttonStyle(.action)
                }
                Text("会装到 ~/.local/bin/cca（或 ~/.cca/bin/cca）。").font(.system(size: 11.5)).foregroundStyle(Theme.muted)
            }
        case .notInstalled:
            panel("先在终端里运行 cca install") {
                Text("cca 还没完成安装（设置、账号表都还没有）。安装要在终端里回答几个问题。")
                HStack(spacing: 8) {
                    Button("在终端里运行 cca install") { model.runInTerminal(.install) }.buttonStyle(.primary)
                    Button("重新检查") { Task { await model.connect() } }.buttonStyle(.action)
                }
            }
        case .unsupported(let api):
            panel("请更新 cca") {
                if let api, api > GUIAPI.supported.upperBound {
                    Text("cca 比这个 App 新（接口版本 \(api)，App 支持 \(GUIAPI.supported.lowerBound)–\(GUIAPI.supported.upperBound)）。更新 App 后会自动重新打开。")
                    HStack(spacing: 8) {
                        Button("更新并重新打开") { model.updateAndRelaunch() }
                            .buttonStyle(.primary)
                            .disabled(model.updating)
                        if model.updating {
                            ProgressView().controlSize(.small)
                            Text("正在下载…").font(.system(size: 12)).foregroundStyle(Theme.muted)
                        }
                    }
                } else {
                    Text(api == nil ? "这个版本的 cca 还不支持 cca App。在终端里运行 cca update 更新到最新版本。"
                         : "cca 太旧了（接口版本 \(api!)）。在终端里运行 cca update 更新。")
                    HStack(spacing: 8) {
                        Button("在终端里运行 cca update") { model.runInTerminal(.update) }.buttonStyle(.primary)
                        Button("重新检查") { Task { await model.connect() } }.buttonStyle(.action)
                    }
                }
            }
        case .failed(let msg):
            panel("没能连上 cca") {
                Text(msg).textSelection(.enabled)
                Button("重试") { Task { await model.connect() } }.buttonStyle(.primary)
            }
        default:
            EmptyView()
        }
    }
}
