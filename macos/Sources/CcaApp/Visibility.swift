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

/// 打开主窗口的办法（SwiftUI 的 openWindow 只能从视图里拿到：主窗口和菜单栏面板出现时登记）。
@MainActor
final class WindowOpener {
    static let shared = WindowOpener()
    var open: (() -> Void)?

    func showMain() {
        NSApp.activate()
        if let open {
            open()
        } else if let w = NSApp.windows.first(where: { $0.canBecomeMain && !($0 is NSPanel) }) {
            w.makeKeyAndOrderFront(nil)
        }
    }
}
