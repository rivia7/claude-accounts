import AppKit
import SwiftUI

/// 入口：`--self-check` 在 NSApplication 启动之前处理（build.sh 用它确认编出来的程序能用）。
@main
enum Main {
    static func main() {
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

    var body: some Scene {
        Window("cca", id: "main") {
            MainWindow()
                .environment(delegate.model)
        }
        .defaultSize(width: 1040, height: 740)
        .commands {
            CommandGroup(replacing: .newItem) {}
            CommandGroup(after: .toolbar) {
                Button("刷新") { delegate.model.refreshAll() }
                    .keyboardShortcut("r", modifiers: .command)
            }
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
/// 退出时取消 usage / check / live 的子进程。
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let model: AppModel
    private var observers: [(NotificationCenter, NSObjectProtocol)] = []
    nonisolated private static let desktopApps: Set<String> = ["com.anthropic.claudefordesktop", "com.openai.codex"]

    override init() {
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0"
        // 演示模式（CCA_GUI_FIXTURES）：选择只记在内存里，不写进真正的 App 设置
        let demo = !(ProcessInfo.processInfo.environment["CCA_GUI_FIXTURES"] ?? "").isEmpty
        model = AppModel(appVersion: version, defaults: demo ? MemoryPrefs() : UserDefaults.standard,
                         hooks: AppHooks(prompter: AlertPrompter()))
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

    func applicationDidFinishLaunching(_ notification: Notification) {
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
            Task { await Snapshot.run(model: model, into: dir) }
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
