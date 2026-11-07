import AppKit
import SwiftUI

/// 主窗口：按连接状态显示引导卡片，或者侧栏 + 详情。终端交接和失败说明是这个窗口上的弹层。
struct MainWindow: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        @Bindable var model = model
        Group {
            switch model.phase {
            case .ready:
                SplitView()
            case .starting:
                VStack(spacing: 12) {
                    ProgressView()
                    Text("正在连接 cca…").foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            default:
                OnboardingView()
            }
        }
        .frame(minWidth: 860, minHeight: 560)
        .overlay(alignment: .bottom) {
            if let t = model.toast {
                ToastView(toast: t)
                    .padding(.bottom, 84)
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
            // 主窗口出来了就是普通 App：--core 启动时是 .accessory（没有 Dock 图标和菜单栏），不管是从哪里打开的都换回来
            if NSApp.activationPolicy() != .regular { NSApp.setActivationPolicy(.regular) }
        }
    }
}

struct SplitView: View {
    @Environment(AppModel.self) private var model
    @Environment(Guide.self) private var guide

    var body: some View {
        NavigationSplitView {
            Sidebar()
                .navigationSplitViewColumnWidth(min: 250, ideal: 290, max: 380)
        } detail: {
            DetailColumn()
        }
        .toolbar { MainToolbar() }
        // 第一次连上 cca 时打开卡片教程；按 Esc 关掉也算看过（以后从“帮助”菜单、工具栏的问号再看）
        .sheet(isPresented: Binding(get: { guide.tutorialShown }, set: { if !$0 { guide.finishTutorial() } })) {
            TutorialSheet().environment(guide)
        }
        .onAppear { guide.startIfNeeded() }
    }
}

struct DetailColumn: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            switch model.selection {
            case .account(let ref)?:
                AccountDetail(ref: ref)
            case .routing?:
                RoutingPage()
            case nil:
                if model.accounts(model.tool).isEmpty {
                    ContentUnavailableView("还没有\(Texts.product(model.tool))账号", systemImage: "person.crop.circle.badge.plus",
                                           description: Text("还没有账号 — 点工具栏的「＋」添加。"))
                } else {
                    ContentUnavailableView("选一个账号", systemImage: "person.2", description: Text("左边列出了这台 Mac 上的账号。"))
                }
            }
        }
        .safeAreaInset(edge: .top, spacing: 0) {
            TopBanners()
        }
    }
}

/// 详情上方的提示条：版本不同、账号表校验不过、共享设置有问题、文件读不了、后台刷新用量要同意或失败了、读 state 出错。
/// 提示条是内容层（实的，见 Glass.swift 的 Banner），里面的按钮用 .bordered / .borderedProminent。
struct TopBanners: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        BannerStack {
            if model.versionMismatch, let v = model.hello?.ccaVersion {
                Banner(systemImage: "arrow.down.app", tint: .accentColor,
                            title: model.hello?.gui?.stagedVersion == v ? "新版本的 cca App 已经下载好" : "cca 已经更新到 \(v)",
                            message: model.updating ? "正在下载和 cca 同版本的 App…" : "这个 App 是 \(model.appVersion)。更新后会自动重新打开。") {
                    if model.updating { ProgressView().controlSize(.small) }
                    Button("更新并重新打开") { model.updateAndRelaunch() }
                        .buttonStyle(.borderedProminent)
                        .disabled(model.updating)
                }
            }
            ForEach(model.invalidTables, id: \.tool) { item in
                Banner(systemImage: "tablecells.badge.ellipsis", tint: .red,
                            title: "\(Texts.product(item.tool)) 的账号表校验不过：启动账号会失败",
                            message: item.issues.map(\.message).filter { !$0.isEmpty }.joined(separator: "\n")) {
                    Button("在终端里编辑…") { model.runInTerminal(.edit(prefix: model.prefix(item.tool))) }
                        .buttonStyle(.bordered)
                }
            }
            ForEach(model.visibleTools, id: \.self) { t in
                if let problems = model.toolState(t)?.settingsProblems, !problems.isEmpty {
                    Banner(systemImage: "gearshape.2", tint: .orange,
                                title: "\(Texts.product(t)) 的共享设置里有会盖掉账号代理、接口或凭证的项",
                                message: problems.joined(separator: "、") + "。启动账号前 cca 会检查，可能要先在终端里处理。")
                }
            }
            ForEach(Array((model.state?.errors ?? []).enumerated()), id: \.offset) { _, e in
                Banner(systemImage: "exclamationmark.octagon", tint: .red,
                            title: (e.tool.map { Texts.product($0) + "：" } ?? "") + Texts.stateError(e),
                            message: nonEmpty(e.message) == nil || e.message == Texts.stateError(e) ? "" : e.message)
            }
            ForEach(model.visibleTools, id: \.self) { t in
                if let cs = model.usageNeedsConsent[t], let first = cs.first {
                    Banner(systemImage: "hand.raised", tint: .orange, title: "刷新 \(Texts.product(t)) 的用量要你同意",
                                message: first.title) {
                        Button("现在刷新") { model.refreshUsage(tool: t, manual: true) }
                            .buttonStyle(.bordered)
                    }
                } else if let f = model.usageFailure[t] {
                    Banner(systemImage: "exclamationmark.triangle", tint: .orange, title: "后台刷新 \(Texts.product(t)) 的用量没有成功",
                                message: f.message) {
                        Button("重试") { model.refreshUsage(tool: t, manual: true) }
                            .buttonStyle(.bordered)
                    }
                }
            }
            if let e = model.stateError {
                Banner(systemImage: "exclamationmark.triangle", tint: .red, title: "没能读取账号状态", message: e) {
                    Button("重试") { Task { await model.connect() } }.buttonStyle(.bordered)
                }
            }
            if let t = model.coreTrouble {
                Banner(systemImage: "exclamationmark.arrow.triangle.2.circlepath", tint: .orange,
                       title: "内置代理的内核停了，稍后自动重启",
                       message: [t.code >= 0 ? "退出码 \(t.code)" : nil, nonEmpty(t.detail)].compactMap { $0 }.joined(separator: "：")) {
                    Button("现在重启") { model.retryCore() }
                        .buttonStyle(.bordered)
                }
            }
        }
        .padding(.top, hasBanners ? 14 : 0)
        .padding(.bottom, hasBanners ? 2 : 0)
        .pageColumn()
    }

    private var hasBanners: Bool {
        model.versionMismatch || !model.invalidTables.isEmpty || !(model.state?.errors ?? []).isEmpty
            || !model.usageNeedsConsent.isEmpty || !model.usageFailure.isEmpty || model.stateError != nil || model.coreTrouble != nil
            || model.visibleTools.contains { !(model.toolState($0)?.settingsProblems ?? []).isEmpty }
    }
}

/// 工具栏：刷新、同步路由器（配置了路由器才有）、“＋”菜单（都在终端里完成）、新手教程。
struct MainToolbar: ToolbarContent {
    @Environment(AppModel.self) private var model
    @Environment(Guide.self) private var guide

    var body: some ToolbarContent {
        ToolbarItemGroup(placement: .primaryAction) {
            Button {
                model.refreshAll()
            } label: {
                Label("刷新", systemImage: "arrow.clockwise")
            }
            .help("重新读取账号状态，并经各账号的出口查用量")
            .disabled(model.phase != .ready)
            if model.routerConfigured {
                Button {
                    model.syncRouter()
                } label: {
                    Label("同步路由器", systemImage: "wifi.router")
                }
                .help("把这台 Mac 的账号表同步到路由器（可能要重载 OpenClash / ShellCrash，会先问你）")
                .disabled(model.syncActivity != nil)
            }
        }
        ToolbarItem(placement: .primaryAction) {
            Menu {
                Button("添加 Claude 账号…") { model.runInTerminal(.addAccount(prefix: [], product: "Claude")) }
                Button("添加 ChatGPT 账号…") { model.runInTerminal(.addAccount(prefix: model.prefix(.codex), product: "ChatGPT")) }
                Button("添加第三方 API 账号…") { model.runInTerminal(.addAPI) }
                Divider()
                Button("编辑 \(Texts.product(model.tool)) 账号表…") { model.runInTerminal(.edit(prefix: model.prefix(model.tool))) }
                Divider()
                Button("配置路由器…") { model.runInTerminal(.routerSetup) }
                Button("配置本机 Clash…") { model.runInTerminal(.localSetup) }
            } label: {
                Label("添加", systemImage: "plus")
            }
            .help("添加账号、编辑账号表、配置路由器（在终端里完成）")
        }
        ToolbarItem(placement: .primaryAction) {
            Button {
                guide.openTutorial()
            } label: {
                Label("新手教程", systemImage: "questionmark.circle")
            }
            .help("新手教程：几张卡片讲清楚怎么用（⇧⌘/）")
        }
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
                .frame(width: 96, height: 96)
            card
                .frame(maxWidth: 560)
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    @ViewBuilder
    private var card: some View {
        switch model.phase {
        case .missingCca:
            Card("先安装 cca", systemImage: "shippingbox") {
                VStack(alignment: .leading, spacing: 12) {
                    Text("这个 App 是 cca 的图形界面，需要先装好 cca。在“终端”里运行：")
                    Text(Self.installCommand)
                        .font(.system(.callout, design: .monospaced))
                        .textSelection(.enabled)
                        .padding(10)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(RoundedRectangle(cornerRadius: 8).fill(.quaternary.opacity(0.6)))
                    GlassRow(spacing: 10) {
                        Button("复制命令") { model.copy(Self.installCommand) }
                            .buttonStyle(.glassProminent)
                        Button("装好了，重新查找") { Task { await model.connect() } }
                            .buttonStyle(.glass)
                    }
                    Text("会装到 ~/.local/bin/cca（或 ~/.cca/bin/cca）。").font(.caption).foregroundStyle(.secondary)
                }
            }
        case .notInstalled:
            Card("先在终端里运行 cca install", systemImage: "wrench.and.screwdriver") {
                VStack(alignment: .leading, spacing: 12) {
                    Text("cca 还没完成安装（设置、账号表都还没有）。安装要在终端里回答几个问题。")
                    GlassRow(spacing: 10) {
                        Button("在终端里运行 cca install") { model.runInTerminal(.install) }
                            .buttonStyle(.glassProminent)
                        Button("重新检查") { Task { await model.connect() } }
                            .buttonStyle(.glass)
                    }
                }
            }
        case .unsupported(let api):
            Card("请更新 cca", systemImage: "arrow.triangle.2.circlepath.circle") {
                VStack(alignment: .leading, spacing: 12) {
                    if let api, api > GUIAPI.supported.upperBound {
                        Text("cca 比这个 App 新（接口版本 \(api)，App 支持 \(GUIAPI.supported.lowerBound)–\(GUIAPI.supported.upperBound)）。更新 App 后会自动重新打开。")
                        HStack(spacing: 10) {
                            Button("更新并重新打开") { model.updateAndRelaunch() }
                                .buttonStyle(.glassProminent)
                                .disabled(model.updating)
                            if model.updating {
                                ProgressView().controlSize(.small)
                                Text("正在下载…").font(.callout).foregroundStyle(.secondary)
                            }
                        }
                    } else {
                        Text(api == nil ? "这个版本的 cca 还不支持 cca App。在终端里运行 cca update 更新到最新版本。"
                             : "cca 太旧了（接口版本 \(api!)）。在终端里运行 cca update 更新。")
                        GlassRow(spacing: 10) {
                            Button("在终端里运行 cca update") { model.runInTerminal(.update) }
                                .buttonStyle(.glassProminent)
                            Button("重新检查") { Task { await model.connect() } }
                                .buttonStyle(.glass)
                        }
                    }
                }
            }
        case .failed(let msg):
            Card("没能连上 cca", systemImage: "exclamationmark.triangle") {
                VStack(alignment: .leading, spacing: 12) {
                    Text(msg).textSelection(.enabled)
                    Button("重试") { Task { await model.connect() } }
                        .buttonStyle(.glassProminent)
                }
            }
        default:
            EmptyView()
        }
    }
}
