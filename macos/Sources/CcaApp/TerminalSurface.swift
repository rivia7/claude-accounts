import AppKit
import SwiftTerm

// 这个文件是 App 里唯一 import SwiftTerm 的地方（它有自己的 Color、Terminal 等名字，别的文件不用知道）。
// SwiftTerm 的来历、怎么编、怎么更新见 macos/Vendor/SwiftTerm/README.md。

/// 界面里的终端画在哪：SwiftTerm 的视图，加上它和 RemoteTerminal 之间的线（字节进出、大小、标题、链接、剪贴板）。
/// 跟着 RemoteTerminal 活着（放在它的 `surface` 里）：换到别的页再回来，画面还在。
/// 终端里的内容只在 SwiftTerm 的缓冲区里（内存），这里不写文件、不进日志。
///
/// 内容来自别的机器，不让它动这台 Mac：图形协议（Kitty、sixel）在编 SwiftTerm 的时候就去掉了（macos/build.sh），这里再关掉 iTerm2 的
/// 内联图片和文件（OSC 1337）、不说自己支持 sixel；链接只开 http / https；剪贴板只在用户正看着这块终端时才写、不给读；不响铃。
/// 用户自己的操作照终端的习惯来（PROTOCOL 9.3）：用鼠标选中的字马上放进剪贴板（选中即复制），右键粘贴（只看的、没在运行的不粘）。
@MainActor
final class TerminalSurface {
    let view: SizedTerminalView
    /// 给不认识 SwiftTerm 的文件用
    var nsView: NSView { view }
    private weak var terminal: RemoteTerminal?
    private let bridge = TerminalBridge()
    private let openURL: (URL) -> Void
    private let copy: (String) -> Void
    private var reported: (cols: Int, rows: Int)?
    private var sizing: DispatchWorkItem?

    /// 这个终端的画面：有就用原来的，没有就建一个
    static func of(_ terminal: RemoteTerminal, openURL: @escaping (URL) -> Void, copy: @escaping (String) -> Void) -> TerminalSurface {
        if let s = terminal.surface as? TerminalSurface { return s }
        let s = TerminalSurface(terminal, openURL: openURL, copy: copy)
        terminal.surface = s
        return s
    }

    private init(_ terminal: RemoteTerminal, openURL: @escaping (URL) -> Void, copy: @escaping (String) -> Void) {
        self.terminal = terminal
        self.openURL = openURL
        self.copy = copy
        view = SizedTerminalView(frame: .zero, font: NSFont.monospacedSystemFont(ofSize: 12, weight: .regular))
        view.optionAsMetaKey = true
        view.bellStyle = .none
        let engine = view.getTerminal()
        engine.options.enableSixelReported = false
        engine.registerOscHandler(code: 1337) { _ in }   // iTerm2 的内联图片、传文件：不要
        view.terminalDelegate = bridge
        bridge.surface = self
        view.onLayout = { [weak self] in self?.measure() }
        // 这块视图固定成深色外观：滚动条、光标这些跟着外观走的也照黑底来，App 是浅色时也一样
        view.appearance = NSAppearance(named: .darkAqua)
        view.nativeBackgroundColor = TerminalSurface.background
        view.nativeForegroundColor = TerminalSurface.foreground
        view.installColors(Color.terminalAppColors)   // 16 色用它默认的那一套，和系统终端一样
        view.onSelect = { [weak self] text in self?.selected(text) }
        view.canPaste = { [weak terminal] in terminal?.canPaste ?? false }
        view.followMouseMode()
        terminal.sink = { [weak view] data in
            view?.feed(byteArray: [UInt8](data)[...])
            view?.followMouseMode()
        }
    }

    /// 终端底色：始终是黑的，不跟 App 的浅色、深色（视图外面那一圈留白用同一个颜色）。会话多半是在深色的终端里启动的，
    /// 里面的程序照那个底色选的颜色，接到白底上有的字认不出来
    static let background = NSColor(srgbRed: 0, green: 0, blue: 0, alpha: 1)

    private static let foreground = NSColor(srgbRed: 0.88, green: 0.88, blue: 0.9, alpha: 1)

    /// 视图的大小定了（在窗口里、不是零）：告诉 RemoteTerminal 现在是几列几行。头一次就是起进程；拖窗口时一路在变，等停一下再说
    private func measure() {
        guard view.window != nil, view.frame.width > 1, view.frame.height > 1 else { return }
        sizing?.cancel()
        let work = DispatchWorkItem { [weak self] in
            guard let self, let t = self.terminal else { return }
            // 头一次（进程还没起）视图已经不在窗口里了也照量好的大小起：刚接入就换到了别的终端时，不然它一直等着、没接上
            guard self.view.window != nil || self.reported == nil else { return }
            let term = self.view.getTerminal()
            let cols = term.cols, rows = term.rows
            guard cols >= 2, rows >= 1 else { return }
            if let r = self.reported, r.cols == cols, r.rows == rows { return }
            self.reported = (cols, rows)
            t.resize(cols: cols, rows: rows)
        }
        sizing = work
        DispatchQueue.main.asyncAfter(deadline: .now() + (reported == nil ? 0.05 : 0.12), execute: work)
    }

    func focus() {
        guard let w = view.window, w.firstResponder !== view else { return }
        w.makeFirstResponder(view)
    }

    // MARK: SwiftTerm 的回调（经 TerminalBridge）

    fileprivate func typed(_ data: Data) {
        terminal?.write(data)
    }

    fileprivate func titled(_ title: String) {
        terminal?.title = Texts.plainLine(title, max: 120)   // 对方写的：控制字符、改文字方向的先去掉
    }

    /// 终端里的链接（会话里的程序写的，或者认出来的网址）：只开 http / https。别的协议、看着像本机路径的都不开——
    /// 内容来自另一台机器，不能让它在这台 Mac 上打开文件或者别的 App。
    fileprivate func open(_ link: String) {
        guard let url = URL(string: link), let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https",
              url.host?.isEmpty == false else { return }
        openURL(url)
    }

    /// 会话里的程序要往剪贴板放东西（OSC 52，Claude Code 的「复制」用它）：只在用户正看着这块终端时放——窗口在最前面、键盘焦点在
    /// 终端上。换到别的页、别的 App 时不放：不然那台机器上的任何输出都能在背后把这台 Mac 的剪贴板换掉（换成一条等着被粘贴的命令）。
    /// 几处接着时宿主只给主的那一处；读剪贴板不给（默认）。
    fileprivate func copied(_ content: Data) {
        guard let w = view.window, w.isKeyWindow, w.firstResponder === view, NSApp.isActive else { return }
        guard !content.isEmpty, content.count <= 1 << 20, let s = String(data: content, encoding: .utf8) else { return }
        copy(s)
    }

    /// 用户用鼠标选中了字：马上放进剪贴板（选中即复制）。是这边的人自己选的，不像 OSC 52 那样要看焦点
    fileprivate func selected(_ text: String) {
        copy(text)
    }
}

/// SwiftTerm 的终端视图，多做几件事：大小变了报一声；选中即复制、右键粘贴；会话里的程序不要鼠标时，选区不被它的输出冲掉。
final class SizedTerminalView: TerminalView {
    var onLayout: (() -> Void)?
    /// 用鼠标选好了字（拖、双击选词、三击选行、Shift 点击扩展）：松开时交出去放进剪贴板
    var onSelect: ((String) -> Void)?
    /// 右键粘贴现在行不行（只看的、没在运行的不行）
    var canPaste: () -> Bool = { false }
    /// 会话里的程序开着鼠标模式（要点击、拖动、滚轮）
    private var programWantsMouse = false
    /// 正在这台 Mac 上选字（按下到松开）
    private var selecting = false

    /// 会话里的程序开关了鼠标模式（每次喂完输出看一眼）：要鼠标时照常把点击、拖动、滚轮报给它；不要时不让 SwiftTerm 报——它报鼠标时
    /// 每来一次输出都清掉选区（Claude Code 一直在重画），字选不住、⌘C 也一直灰着
    func followMouseMode() {
        programWantsMouse = getTerminal().mouseMode != .off
        if !selecting { allowMouseReporting = programWantsMouse }
    }

    /// 程序不要鼠标时拖着就是选字；要鼠标时按住 Option（iTerm2 的习惯）或 Shift 拖才是在这台 Mac 上选字，选的这一下不报给它
    override func mouseDown(with event: NSEvent) {
        selecting = !programWantsMouse || !event.modifierFlags.isDisjoint(with: [.option, .shift])
        allowMouseReporting = !selecting
        super.mouseDown(with: event)
    }

    override func mouseUp(with event: NSEvent) {
        super.mouseUp(with: event)
        if selecting, let text = getSelection(), !text.isEmpty { onSelect?(text) }
        selecting = false
        allowMouseReporting = programWantsMouse
    }

    /// 右键直接粘贴（PuTTY、Xshell 的习惯）：和 ⌘V 一样经 SwiftTerm 的 paste，程序开着括号粘贴模式时带上标记（Claude Code 不会把
    /// 多行当成几次回车）。只看的、没在运行的终端什么都不做，也不弹菜单
    override func rightMouseDown(with event: NSEvent) {
        guard canPaste() else { return }
        window?.makeFirstResponder(self)
        paste(self)
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        onLayout?()
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window != nil { onLayout?() }
    }
}

/// SwiftTerm 的回调。它的协议不带隔离、回调都在主线程上（喂数据、按键都在主线程），所以单独一个不带隔离的类，进来就回到主线程的身份。
private final class TerminalBridge: TerminalViewDelegate {
    weak var surface: TerminalSurface?

    func send(source: TerminalView, data: ArraySlice<UInt8>) {
        let d = Data(data)
        MainActor.assumeIsolated { surface?.typed(d) }
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

    // 大小由 SizedTerminalView 报（头一次排版出来正好是默认的 80×25 时 SwiftTerm 不会调这个）
    func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {}
    func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}
    func scrolled(source: TerminalView, position: Double) {}
    func rangeChanged(source: TerminalView, startY: Int, endY: Int) {}
    func bell(source: TerminalView) {}
    func iTermContent(source: TerminalView, content: ArraySlice<UInt8>) {}
    func clipboardRead(source: TerminalView) -> Data? { nil }
}
