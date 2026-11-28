import SwiftUI

/// iOS 上的 cca：接入电脑上开着的远程会话（docs/dev/ios.md）。
/// 启动参数（截图、界面测试用，ios/test.sh）：`-demo` 进演示模式；
/// `-screen list|accounts|account|stats|cost|account-cost|sharing|terminal|pair|settings|launch|sort|tutorial|whatsnew|toast` 直接打开那一页
/// （演示模式里；list 是远程会话，accounts 是总览，account 是总览里第一个账号的那一页，account-cost 是从折合金额页点进去的那一页；
/// 总览另带 `-tool 工具`、`-plan 套餐`、`-team outside|team:名字`：看哪个工具的、只看哪种套餐、按 Team 筛；`-account 第几个` 是哪个账号；折合金额带 `-days` 是按天看；
/// tutorial 是新手教程（`-step 第几步`，从 0 数），whatsnew 是更新说明，toast 是会话列表底下浮着一句提示；`-tips off` 把提示卡片都关掉）。
/// 截图、界面测试（`-test-…`，TestDriver）时不自己弹新手教程、更新了什么
@main
struct CcaApp: App {
    /// 推送令牌经它来（Pusher）
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @State private var model: AppModel
    @Environment(\.scenePhase) private var phase

    init() {
        let args = ProcessInfo.processInfo.arguments
        let model = AppModel(demo: args.contains("-demo"))
        if args.contains("-screen") || args.contains(where: { $0.hasPrefix("-test-") }) { model.guide.automatic = false }
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
        model.tab = Self.tab(screen)
        // 总览看哪个工具、只看哪种套餐（-tool codex、-plan Pro）：没写的照默认（Claude、全部套餐）；折合金额按周还是按天（-days）：
        // 上次截图记下的不算
        func value(_ flag: String) -> String? {
            args.firstIndex(of: flag).flatMap { $0 + 1 < args.count ? args[$0 + 1] : nil }
        }
        model.accountTool = value("-tool") ?? "claude"
        model.accountPlan = value("-plan") ?? ""
        // 按 Team 筛（-team outside、-team team:示例 Team）：没写的是全部账号
        model.accountTeam = AccountShelf.TeamPick(stored: value("-team") ?? "")
        model.costDays = args.contains("-days")
        // 提示卡片都关掉（像点过「知道了」；演示模式只记在内存里）
        if value("-tips") == "off" {
            for t in TipCard.ID.allCases { model.guide.dismiss(t) }
        }
        switch screen {
        case "account", "account-cost":
            // -account 第几个（从 0 数，各台电脑的账号接着数）：默认第一个
            let all = model.peers.flatMap { p in (model.status(p)?.accounts ?? []).map { AccountRef(pid: p.pid, id: $0.id) } }
            let n = args.firstIndex(of: "-account").flatMap { $0 + 1 < args.count ? Int(args[$0 + 1]) : nil } ?? 0
            if all.indices.contains(n) {
                if screen == "account" { model.accountPath = [all[n]] } else { model.costPath = [all[n]] }
            }
        case "terminal":
            if let p = model.peers.first, let s = model.listing(p)?.sessions.first { model.attach(s, on: p, readOnly: false) }
        case "launch":
            // 新建会话：头一台有启动器的电脑
            model.launchFor = model.peers.first { model.launcher($0) != nil }.map { LaunchTarget(peer: $0) }
        case "sort":
            // 会话列表在「排序」
            model.sortingSessions = true
        case "pair": model.startPairing()
        case "settings": model.showSettings = true
        case "tutorial": model.guide.openTutorial(at: value("-step").flatMap(Int.init) ?? 0)
        case "whatsnew": model.guide.openWhatsNew()
        case "toast":
            // 结束会话没成的那一句（Mac App 的提示条）
            if let p = model.peers.first, let s = model.listing(p)?.sessions.first {
                model.showToast("没能结束「\(Texts.sessionTitle(s))」：电脑没有响应，请稍后再试", .warning)
            }
        default: break
        }
    }

    /// 截图的那一页在底下哪一页里
    private static func tab(_ screen: String) -> AppTab {
        switch screen {
        case "accounts", "account": return .overview
        case "stats": return .stats
        case "cost", "account-cost": return .cost
        case "sharing": return .sharing
        default: return .sessions
        }
    }
}
