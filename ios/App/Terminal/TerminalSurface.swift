import SwiftTerm
import SwiftUI
import UIKit

// 这个文件是 App 里唯一 import SwiftTerm 的地方（它有自己的 Color、Terminal 等名字，别的文件不用知道）。
// SwiftTerm 的来历和怎么编见 ios/Vendor/SwiftTerm-iOS/README.md（大部分源码和 Mac App 共用 macos/Vendor/SwiftTerm）。

/// 终端画在哪：SwiftTerm 的 iOS 视图（自带键盘上面那一排 Esc、Ctrl、Tab、方向键），加上它和 TerminalSession 之间的线。
/// 跟着 TerminalSession 活着（放在它的 `surface` 里）。终端里的内容只在 SwiftTerm 的缓冲区里（内存），不写文件、不进日志。
///
/// 内容来自别的机器，不让它动这台手机：图形协议（Kitty、sixel）编 SwiftTerm 的时候就去掉了（和 Mac App 同一个脚本：
/// macos/tools/swiftterm-parser.sh），这里再关掉 iTerm2 的内联图片和文件（OSC 1337）、不说自己支持 sixel；链接只开 http / https；
/// 剪贴板只在 App 在前台、用户正看着这块终端时才写，不给读；不响铃。和 Mac App 的 TerminalSurface 是同一套规矩
@MainActor
final class TerminalSurface {
    let view: SizedTerminalView
    private weak var session: TerminalSession?
    private let bridge = TerminalBridge()
    private var reported: (cols: Int, rows: Int)?
    private var sizing: DispatchWorkItem?

    static func of(_ s: TerminalSession) -> TerminalSurface {
        if let t = s.surface as? TerminalSurface { return t }
        let t = TerminalSurface(s)
        s.surface = t
        return t
    }

    /// 终端底色：始终是黑的（会话多半是在深色的终端里启动的，里面的程序照那个底色选的颜色）
    static let background = UIColor(red: 0, green: 0, blue: 0, alpha: 1)
    private static let foreground = UIColor(red: 0.88, green: 0.88, blue: 0.9, alpha: 1)

    private init(_ s: TerminalSession) {
        session = s
        view = SizedTerminalView(frame: CGRect(x: 0, y: 0, width: 320, height: 480),
                                 font: UIFont.monospacedSystemFont(ofSize: UIDevice.current.userInterfaceIdiom == .pad ? 14 : 12, weight: .regular))
        view.readOnly = s.readOnly
        view.keyboardAppearance = .dark
        view.overrideUserInterfaceStyle = .dark
        view.nativeBackgroundColor = Self.background
        view.nativeForegroundColor = Self.foreground
        view.installColors(SwiftTerm.Color.terminalAppColors)   // 16 色用它默认的那一套（和 Mac App 一样）
        let engine = view.getTerminal()
        engine.options.enableSixelReported = false
        engine.registerOscHandler(code: 1337) { _ in }
        view.terminalDelegate = bridge
        bridge.surface = self
        view.onLayout = { [weak self] in self?.measure() }
        s.sink = { [weak view] b in view?.feed(byteArray: b[...]) }
    }

    /// 视图的大小定了：告诉 TerminalSession 现在几列几行（头一次就是接入）。键盘弹出、收起、转屏时一路在变，等停一下再说
    private func measure() {
        guard view.window != nil, view.bounds.width > 1, view.bounds.height > 1 else { return }
        sizing?.cancel()
        let work = DispatchWorkItem { [weak self] in
            guard let self, let s = self.session else { return }
            let t = self.view.getTerminal()
            let cols = t.cols, rows = t.rows
            guard cols >= 2, rows >= 2 else { return }
            if let r = self.reported, r.cols == cols, r.rows == rows { return }
            self.reported = (cols, rows)
            s.sized(cols: cols, rows: rows)
        }
        sizing = work
        DispatchQueue.main.asyncAfter(deadline: .now() + (reported == nil ? 0.05 : 0.15), execute: work)
    }

    /// 现在的大小（重新接入时用）
    var size: (cols: Int, rows: Int) {
        let t = view.getTerminal()
        return (max(t.cols, 2), max(t.rows, 2))
    }

    func focus() {
        guard !view.readOnly, view.window != nil, !view.isFirstResponder else { return }
        _ = view.becomeFirstResponder()
    }

    func hideKeyboard() {
        _ = view.resignFirstResponder()
    }

    // MARK: SwiftTerm 的回调（经 TerminalBridge）

    fileprivate func typed(_ data: ArraySlice<UInt8>) {
        session?.send(Array(data))
    }

    fileprivate func titled(_ title: String) {
        session?.title = Texts.plainLine(title, max: 120)
    }

    /// 只开 http / https 的链接（内容来自另一台机器，不能让它在这台手机上打开别的 App）
    fileprivate func open(_ link: String) {
        guard let url = URL(string: link), let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https",
              url.host?.isEmpty == false else { return }
        UIApplication.shared.open(url)
    }

    /// 会话里的程序要往剪贴板放东西（OSC 52）：只在 App 在前台、键盘焦点在这块终端上时放（不然那台机器上的任何输出都能在背后
    /// 把这台手机的剪贴板换掉）。读剪贴板不给
    fileprivate func copied(_ content: Data) {
        guard UIApplication.shared.applicationState == .active, view.isFirstResponder else { return }
        guard !content.isEmpty, content.count <= 1 << 20, let s = String(data: content, encoding: .utf8) else { return }
        UIPasteboard.general.string = s
    }
}

/// SwiftTerm 的终端视图，多报一件事：大小变了；只看的不弹键盘
final class SizedTerminalView: TerminalView {
    var onLayout: (() -> Void)?
    var readOnly = false

    override func layoutSubviews() {
        super.layoutSubviews()
        onLayout?()
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        if window != nil { onLayout?() }
    }

    override var canBecomeFirstResponder: Bool {
        !readOnly
    }
}

/// SwiftTerm 的回调（都在主线程上）
private final class TerminalBridge: TerminalViewDelegate {
    weak var surface: TerminalSurface?

    func send(source: TerminalView, data: ArraySlice<UInt8>) {
        MainActor.assumeIsolated { surface?.typed(data) }
    }

    func setTerminalTitle(source: TerminalView, title: String) {
        MainActor.assumeIsolated { surface?.titled(title) }
    }

    func requestOpenLink(source: TerminalView, link: String, params: [String: String]) {
        MainActor.assumeIsolated { surface?.open(link) }
    }

    func clipboardCopy(source: TerminalView, content: Data) {
        MainActor.assumeIsolated { surface?.copied(content) }
    }

    func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {}
    func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}
    func scrolled(source: TerminalView, position: Double) {}
    func rangeChanged(source: TerminalView, startY: Int, endY: Int) {}
    func bell(source: TerminalView) {}
    func iTermContent(source: TerminalView, content: ArraySlice<UInt8>) {}
    func clipboardRead(source: TerminalView) -> Data? { nil }
}

/// 放进 SwiftUI：视图是 TerminalSession 自己留着的那一个
struct TerminalHost: UIViewRepresentable {
    let session: TerminalSession

    func makeUIView(context: Context) -> UIView {
        let s = TerminalSurface.of(session)
        DispatchQueue.main.async { s.focus() }
        return s.view
    }

    func updateUIView(_ view: UIView, context: Context) {}
}

extension TerminalSession {
    var terminalSize: (cols: Int, rows: Int) {
        (surface as? TerminalSurface)?.size ?? (80, 24)
    }

    func showKeyboard() {
        (surface as? TerminalSurface)?.focus()
    }

    func hideKeyboard() {
        (surface as? TerminalSurface)?.hideKeyboard()
    }
}
