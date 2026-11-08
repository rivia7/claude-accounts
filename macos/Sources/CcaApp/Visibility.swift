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

/// 打开主窗口的办法（SwiftUI 的 openWindow 只能从视图里拿到：主窗口和菜单栏面板出现时登记）。
@MainActor
final class WindowOpener {
    static let shared = WindowOpener()
    var open: (() -> Void)?

    func showMain() {
        // 只在菜单栏里托管内置代理时（--core 启动）没有 Dock 图标：要显示主窗口了，换回普通 App
        if NSApp.activationPolicy() != .regular { NSApp.setActivationPolicy(.regular) }
        NSApp.activate()
        if let open {
            open()
        } else if let w = NSApp.windows.first(where: { $0.canBecomeMain && !($0 is NSPanel) }) {
            w.makeKeyAndOrderFront(nil)
        }
    }
}
