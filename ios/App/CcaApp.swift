import SwiftUI

/// iOS 上的 cca：接入电脑上开着的远程会话（docs/dev/ios.md）。
/// 启动参数（截图、界面测试用，ios/test.sh）：`-demo` 进演示模式；`-screen list|accounts|account|terminal|pair|settings|launch` 直接打开那一页
/// （演示模式里）
@main
struct CcaApp: App {
    /// 推送令牌经它来（Pusher）
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @State private var model: AppModel
    @Environment(\.scenePhase) private var phase

    init() {
        let args = ProcessInfo.processInfo.arguments
        let model = AppModel(demo: args.contains("-demo"))
        _model = State(initialValue: model)
        Notifier.start(model)
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .preferredColorScheme(model.appearance.scheme)
                .onOpenURL { model.open($0) }
                .task {
                    #if DEBUG
                    TestDriver.prepare()
                    #endif
                    Pusher.shared.start(model)
                    await Self.openScreen(model)
                    #if DEBUG
                    await TestDriver.run(model)
                    #endif
                }
        }
        .onChange(of: phase) { _, p in
            switch p {
            case .active: model.becameActive()
            case .background: model.wentBackground()
            default: break
            }
        }
    }

    /// `-screen 名字`：截图用，直接打开某一页（只在演示模式里）
    @MainActor
    private static func openScreen(_ model: AppModel) async {
        let args = ProcessInfo.processInfo.arguments
        guard model.isDemo, let i = args.firstIndex(of: "-screen"), i + 1 < args.count else { return }
        await model.fetch()
        let screen = args[i + 1]
        model.tab = screen.hasPrefix("account") ? .accounts : .sessions
        switch screen {
        case "account":
            // -account 第几个（从 0 数，各台电脑的账号接着数）：默认第一个
            let all = model.peers.flatMap { p in (model.status(p)?.accounts ?? []).map { AccountRef(pid: p.pid, id: $0.id) } }
            let n = args.firstIndex(of: "-account").flatMap { $0 + 1 < args.count ? Int(args[$0 + 1]) : nil } ?? 0
            if all.indices.contains(n) { model.accountPath = [all[n]] }
        case "terminal":
            if let p = model.peers.first, let s = model.listing(p)?.sessions.first { model.attach(s, on: p, readOnly: false) }
        case "launch":
            // 新建会话：头一台有启动器的电脑
            model.launchFor = model.peers.first { model.launcher($0) != nil }
        case "pair": model.startPairing()
        case "settings": model.showSettings = true
        default: break
        }
    }
}
