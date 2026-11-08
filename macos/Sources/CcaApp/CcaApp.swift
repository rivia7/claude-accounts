import AppKit
import SwiftUI

/// 入口：`--self-check` 在 NSApplication 启动之前处理（build.sh 用它确认编出来的程序能用）。
@main
enum Main {
    static func main() {
        let arguments = Array(CommandLine.arguments.dropFirst())
        if arguments.contains(ClientUpdateSignature.option) {
            exit(ClientUpdateSignature.run(arguments: arguments))
        }
        if CommandLine.arguments.contains("--self-check") {
            SelfCheck.run()
        }
        CcaApp.main()
    }
}

enum SelfCheck {
    static func run() -> Never {
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "unknown"
        do {
            try SampleDocs.verify()
            print("ok \(version)")
            exit(0)
        } catch {
            FileHandle.standardError.write(Data("self-check failed: \(error)\n".utf8))
            exit(1)
        }
    }
}

struct CcaApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    /// cca 在后台打开的（`open -g … --args --core`，内置代理要内核，PROTOCOL 7）：不弹主窗口、不占 Dock，只留菜单栏图标；
    /// 之后从菜单栏、“帮助”菜单打开主窗口时照常显示（WindowOpener 把 Dock 图标换回来）
    static let launchedForCore = CommandLine.arguments.contains("--core")

    var body: some Scene {
        Window("cca", id: "main") {
            MainWindow()
                .environment(delegate.model)
                .environment(delegate.guide)
                .environment(delegate.appearance)
        }
        .defaultSize(width: 1080, height: 720)
        .defaultLaunchBehavior(Self.launchedForCore ? .suppressed : .automatic)
        // 后台打开时也不恢复上次退出时开着的主窗口（恢复出来的窗口不经过 showMain，没有 Dock 图标和菜单栏）
        .restorationBehavior(Self.launchedForCore ? .disabled : .automatic)
        .commands {
            CommandGroup(after: .appInfo) {
                Button("检查 cca 更新…") {
                    WindowOpener.shared.showMain()
                    delegate.model.checkForUpdates(manual: true)
                }
                .disabled(delegate.model.phase != .ready)
                Button("客户端更新…") {
                    WindowOpener.shared.showMain()
                    delegate.model.selection = .clientUpdates
                }
                .disabled(delegate.model.phase != .ready)
            }
            CommandGroup(replacing: .newItem) {}
            CommandGroup(after: .toolbar) {
                Button("刷新") { delegate.model.refreshAll() }
                    .keyboardShortcut("r", modifiers: .command)
                Picker("外观", selection: delegate.appearance.binding) {
                    ForEach(AppearanceMode.allCases) { m in
                        Text(m.title).tag(m)
                    }
                }
            }
            CommandGroup(replacing: .help) {
                Button("新手教程") {
                    WindowOpener.shared.showMain()
                    delegate.guide.openTutorial()
                }
                .keyboardShortcut("/", modifiers: [.command, .shift])
                Button("更新说明") {
                    WindowOpener.shared.showMain()
                    delegate.guide.openWhatsNew()
                }
                Button("重新显示提示卡片") { delegate.guide.resetTips() }
            }
        }

        Settings {
            SettingsView()
                .environment(delegate.guide)
                .environment(delegate.appearance)
        }

        MenuBarExtra {
            MenuBarContent()
                .environment(delegate.model)
        } label: {
            MenuBarLabel(model: delegate.model)
        }
        .menuBarExtraStyle(.window)
    }
}

/// App 的生命周期：关掉主窗口不退出；Claude App / ChatGPT App 启动、退出时重新读 state；App 被激活时重新跑 hello 和 state；
/// 退出时取消 usage / check / live / 检查更新 / 更新的子进程（app、web、router sync、重置额度跑完为止）。
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let model: AppModel
    /// 新手教程、提示卡片、显示技术细节（记在 App 的设置里；演示模式只记在内存里）
    let guide: Guide
    /// 外观：跟随系统 / 浅色 / 深色
    let appearance: AppearanceSetting
    private var observers: [(NotificationCenter, NSObjectProtocol)] = []
    nonisolated private static let desktopApps: Set<String> = ["com.anthropic.claudefordesktop", "com.openai.codex"]

    override init() {
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0"
        // 演示模式（CCA_GUI_FIXTURES）：选择只记在内存里，不写进真正的 App 设置
        let demo = !(ProcessInfo.processInfo.environment["CCA_GUI_FIXTURES"] ?? "").isEmpty
        model = AppModel(appVersion: version, defaults: demo ? MemoryPrefs() : UserDefaults.standard,
                         hooks: AppHooks(prompter: AlertPrompter()))
        guide = Guide(prefs: demo ? MemoryPrefs() : UserDefaults.standard, appVersion: version)
        appearance = AppearanceSetting(prefs: demo ? MemoryPrefs() : UserDefaults.standard)
        super.init()
        model.hooks.openFile = { Platform.openInTerminal($0) }
        model.hooks.openURL = { NSWorkspace.shared.open($0) }
        model.hooks.copy = { Platform.copy($0) }
        model.hooks.showMainWindow = { WindowOpener.shared.showMain() }
        model.hooks.terminate = { NSApp.terminate(nil) }
        // 提示条出现时让读屏软件播报
        model.hooks.announce = { text in
            NSAccessibility.post(element: NSApp as Any, notification: .announcementRequested,
                                 userInfo: [.announcement: text, .priority: NSAccessibilityPriorityLevel.high.rawValue])
        }
    }

    func applicationWillFinishLaunching(_ notification: Notification) {
        if CcaApp.launchedForCore { NSApp.setActivationPolicy(.accessory) }
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        appearance.apply()
        let ws = NSWorkspace.shared.notificationCenter
        for name in [NSWorkspace.didLaunchApplicationNotification, NSWorkspace.didTerminateApplicationNotification] {
            let token = ws.addObserver(forName: name, object: nil, queue: .main) { [weak self] note in
                let app = note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication
                guard let id = app?.bundleIdentifier, AppDelegate.desktopApps.contains(id) else { return }
                MainActor.assumeIsolated { self?.model.desktopAppsChanged() }
            }
            observers.append((ws, token))
        }
        let nc = NotificationCenter.default
        let token = nc.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.model.becameActive() }
        }
        observers.append((nc, token))
        Task { await model.start() }
        #if DEBUG
        if let dir = Snapshot.directory {
            Task { await Snapshot.run(model: model, guide: guide, into: dir) }
        }
        #endif
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { WindowOpener.shared.showMain() }
        return true
    }

    func applicationWillTerminate(_ notification: Notification) {
        model.shutdown()
        for (center, token) in observers { center.removeObserver(token) }
    }
}
