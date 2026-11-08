import AppKit
import SwiftUI

/// 报告所在窗口看不看得见（遮挡状态、最小化、关闭、成为 / 不再是主窗口时重新算）。
/// 主窗口和菜单栏面板各放一个：AppModel 只在看得见时跑定时器、读 live。
struct WindowObserver: NSViewRepresentable {
    let onChange: (Bool) -> Void

    func makeNSView(context: Context) -> ObserverView {
        let v = ObserverView()
        v.onChange = onChange
        return v
    }

    func updateNSView(_ v: ObserverView, context: Context) {
        v.onChange = onChange
    }

    static func dismantleNSView(_ v: ObserverView, coordinator: ()) {
        v.stop()
    }
}

final class ObserverView: NSView {
    var onChange: ((Bool) -> Void)?
    private var tokens: [NSObjectProtocol] = []
    private var last: Bool?

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        stopObserving()
        guard let w = window else {
            report(false)
            return
        }
        let nc = NotificationCenter.default
        let names: [Notification.Name] = [NSWindow.didChangeOcclusionStateNotification, NSWindow.willCloseNotification,
                                          NSWindow.didMiniaturizeNotification, NSWindow.didDeminiaturizeNotification,
                                          NSWindow.didBecomeKeyNotification, NSWindow.didResignKeyNotification,
                                          NSWindow.didChangeScreenNotification]
        for name in names {
            tokens.append(nc.addObserver(forName: name, object: w, queue: .main) { [weak self] note in
                let closing = note.name == NSWindow.willCloseNotification
                MainActor.assumeIsolated { self?.evaluate(closing: closing) }
            })
        }
        evaluate(closing: false)
    }

    func stop() {
        stopObserving()
        report(false)
    }

    private func stopObserving() {
        for t in tokens { NotificationCenter.default.removeObserver(t) }
        tokens.removeAll()
    }

    private func evaluate(closing: Bool) {
        guard let w = window else {
            report(false)
            return
        }
        report(!closing && w.isVisible && !w.isMiniaturized && w.occlusionState.contains(.visible))
    }

    private func report(_ v: Bool) {
        guard v != last else { return }
        last = v
        onChange?(v)
    }

    deinit {
        for t in tokens { NotificationCenter.default.removeObserver(t) }
    }
}

/// 持续的动画（侧栏里最高档头像的光晕流动、总览的插画）能不能走：所在的窗口 / 面板看得见，而且没开“减少动态效果”。
/// 主窗口和菜单栏面板的根上各设一次（motionScope），下面的视图只读它；没设的地方（截图、教程的示意图）是 false，不动。
private struct MotionAllowedKey: EnvironmentKey {
    static let defaultValue = false
}

extension EnvironmentValues {
    var motionAllowed: Bool {
        get { self[MotionAllowedKey.self] }
        set { self[MotionAllowedKey.self] = newValue }
    }
}

extension View {
    /// 按这个窗口看不看得见（AppModel 的 mainVisible / panelVisible）设 motionAllowed
    func motionScope(_ visible: KeyPath<AppModel, Bool>) -> some View {
        modifier(MotionScope(visible: visible))
    }
}

/// 单独一层：窗口变得看得见 / 看不见时只有它和读 motionAllowed 的视图重算，不连累整个窗口。
private struct MotionScope: ViewModifier {
    @Environment(AppModel.self) private var model
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let visible: KeyPath<AppModel, Bool>

    func body(content: Content) -> some View {
        content.environment(\.motionAllowed, model[keyPath: visible] && !reduceMotion)
    }
}

/// 主窗口的开和关，连带 App 是普通 App（有 Dock 图标）还是在后台（只有菜单栏图标）。
/// 打开主窗口的办法：SwiftUI 的 openWindow 只能从视图里拿到，主窗口和菜单栏面板出现时登记。
@MainActor
final class WindowOpener {
    static let shared = WindowOpener()
    var open: (() -> Void)?

    /// 占着 Dock 图标的窗口：主窗口、设置（菜单栏面板、弹层、对话框不算）。
    private var windows: [NSWindow] {
        NSApp.windows.filter { $0.styleMask.contains(.titled) && !($0 is NSPanel) && $0.sheetParent == nil && ($0.isVisible || $0.isMiniaturized) }
    }

    func showMain() {
        let fromBackground = NSApp.activationPolicy() != .regular
        if fromBackground { NSApp.setActivationPolicy(.regular) }
        NSApp.activate()
        if let open {
            open()
        } else if let w = NSApp.windows.first(where: { $0.canBecomeMain && !($0 is NSPanel) }) {
            w.makeKeyAndOrderFront(nil)
        }
        guard fromBackground else { return }
        // 刚换成普通 App 时激活不一定马上生效，窗口又是新建的：稍等再激活一次，把窗口放到最前面，免得压在别的 App 下面
        Task {
            try? await Task.sleep(nanoseconds: 150_000_000)
            NSApp.activate()
            windows.first?.makeKeyAndOrderFront(nil)
            windows.first?.orderFrontRegardless()
        }
    }

    /// 回到后台。窗口上还挂着弹层（问题没答完之类）时不关，和点窗口的关闭按钮一样要先处理它：切回来让用户看到。
    func toBackground() {
        let open = windows
        if let busy = open.first(where: { $0.attachedSheet != nil }) {
            NSApp.activate()
            busy.makeKeyAndOrderFront(nil)
            return
        }
        open.forEach { $0.close() }
    }

    /// 最后一个窗口关掉了：收起 Dock 图标。
    func windowWillClose(_ window: NSWindow) {
        guard windows.contains(window), windows.count == 1, NSApp.activationPolicy() == .regular else { return }
        NSApp.setActivationPolicy(.accessory)
    }
}

/// 退出分两种：真的退出（菜单栏面板和应用菜单的“退出”、更新后重新打开、注销关机）和“不看了”（Dock 里的“退出”、⌘Q），后一种只回到后台。
/// 没有 Dock 图标时收到的退出请求（脚本、活动监视器）无所谓回到后台，照办。
@MainActor
enum AppExit {
    private static var confirmed = false
    /// 系统要注销、重启、关机了。
    static var systemEnding = false

    static func quit() {
        confirmed = true
        NSApp.terminate(nil)
        confirmed = false   // 走到这里是没退成（窗口上挂着弹层时 AppKit 不让退出）：这次不算数，免得下次 Dock 里的“退出”真的退出
    }

    static func shouldExit(policy: NSApplication.ActivationPolicy, event: NSAppleEventDescriptor?) -> Bool {
        confirmed || systemEnding || policy != .regular || systemQuit(event)
    }

    /// 注销、重启、关机发来的退出事件里写着原因；拦住它们，系统会说 cca 中断了关机。
    private static func systemQuit(_ event: NSAppleEventDescriptor?) -> Bool {
        let reason = event?.attributeDescriptor(forKeyword: AEKeyword(kAEQuitReason)) ?? event?.paramDescriptor(forKeyword: AEKeyword(kAEQuitReason))
        guard let code = reason?.enumCodeValue else { return false }
        return [kAELogOut, kAEReallyLogOut, kAEShowRestartDialog, kAERestart, kAEShowShutdownDialog, kAEShutDown].contains(code)
    }
}
