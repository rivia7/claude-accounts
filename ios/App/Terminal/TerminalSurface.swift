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
    /// 放进界面的那一层：能缩放、拖动，终端在它里面
    let box: TerminalBox
    let view: SizedTerminalView
    private weak var session: TerminalSession?
    private let bridge = TerminalBridge()
    private var reported: (cols: Int, rows: Int)?
    private var sizing: DispatchWorkItem?
    #if DEBUG
    /// TestDriver 用：终端照多大排了（`shown 列 行`）、报给会话的这台手机的窗口大小（`natural 列 行`）
    static var sizeSpy: ((String) -> Void)?
    #endif

    static func of(_ s: TerminalSession) -> TerminalSurface {
        if let t = s.surface as? TerminalSurface { return t }
        let t = TerminalSurface(s)
        s.surface = t
        return t
    }

    /// 终端底色：始终是黑的（会话多半是在深色的终端里启动的，里面的程序照那个底色选的颜色）
    static let background = UIColor(red: 0, green: 0, blue: 0, alpha: 1)
    private static let foreground = UIColor(red: 0.88, green: 0.88, blue: 0.9, alpha: 1)
    /// 终端的字：等宽的系统字体，iPad 上大一点（新会话照它算多大：TerminalGrid）
    static let font = UIFont.monospacedSystemFont(ofSize: UIDevice.current.userInterfaceIdiom == .pad ? 14 : 12, weight: .regular)

    private init(_ s: TerminalSession) {
        session = s
        view = SizedTerminalView(frame: CGRect(x: 0, y: 0, width: 320, height: 480), font: Self.font)
        box = TerminalBox(terminal: view)
        view.readOnly = s.readOnly
        view.keyboardAppearance = .dark
        view.overrideUserInterfaceStyle = .dark
        view.nativeBackgroundColor = Self.background
        view.nativeForegroundColor = Self.foreground
        view.installColors(SwiftTerm.Color.terminalAppColors)   // 16 色用它默认的那一套（和 Mac App 一样）
        let engine = view.getTerminal()
        engine.options.enableSixelReported = false
        engine.registerOscHandler(code: 1337) { _ in }
        // 会话里的程序自己写的通知：OSC 777 的 notify（标题;正文）、OSC 9（只有一段字：iTerm2 的写法；数字开头的是进度条，不算）。
        // 宿主转来的 NOTIFY 帧 CcaRemoteKit 解好了交给 TerminalSession；两样都经 Notifier 弹（和 Mac App 的 TerminalSurface 一样认这两种）
        engine.registerOscHandler(code: 777) { [weak self] data in
            guard let text = String(bytes: data, encoding: .utf8) else { return }
            let parts = text.components(separatedBy: ";")
            guard parts.count >= 3, parts[0] == "notify" else { return }
            MainActor.assumeIsolated { self?.notified(title: parts[1], body: parts[2...].joined(separator: ";")) }
        }
        engine.registerOscHandler(code: 9) { [weak self] data in
            guard let text = String(bytes: data, encoding: .utf8), let first = text.first, !first.isNumber else { return }
            MainActor.assumeIsolated { self?.notified(title: "", body: text) }
        }
        view.terminalDelegate = bridge
        bridge.surface = self
        box.onLayout = { [weak self] in self?.measure() }
        s.sink = { [weak view] b in view?.feed(byteArray: b[...]) }
        s.sizeSink = { [weak box] c, r in box?.hostSized(cols: c, rows: r) }
        if let h = s.hostSize { box.hostSized(cols: h.cols, rows: h.rows) }
    }

    /// 这块地方的大小定了：告诉 TerminalSession 照平常的字号放得下几列几行（头一次就是接入）。键盘弹出、收起、转屏时一路在变，
    /// 等停一下再说。新的宿主接上时、这台手机上打了字时照它排（电脑上打了字就照电脑排）；会话多大它另外说（TerminalBox.hostSized）
    private func measure() {
        guard box.window != nil, box.natural() != nil else { return }
        sizing?.cancel()
        let work = DispatchWorkItem { [weak self] in
            guard let self, let s = self.session, let n = self.box.natural() else { return }
            if let r = self.reported, r.cols == n.cols, r.rows == n.rows { return }
            self.reported = n
            #if DEBUG
            Self.sizeSpy?("natural \(n.cols) \(n.rows)")
            #endif
            s.sized(cols: n.cols, rows: n.rows)
        }
        sizing = work
        DispatchQueue.main.asyncAfter(deadline: .now() + (reported == nil ? 0.05 : 0.15), execute: work)
    }

    /// 这台手机的窗口大小（重新接入时用）
    var size: (cols: Int, rows: Int) {
        box.natural() ?? (80, 24)
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

    fileprivate func notified(title: String, body: String) {
        session?.noted(title: title, body: body)
    }

    #if DEBUG
    /// TestDriver 用（见 SizedTerminalView.testDrag）
    func testDrag(rows: Int) -> Bool {
        view.testDrag(rows: rows)
    }
    #endif

    /// 会话里的程序要往剪贴板放东西（OSC 52）：只在 App 在前台、键盘焦点在这块终端上时放（不然那台机器上的任何输出都能在背后
    /// 把这台手机的剪贴板换掉）。读剪贴板不给
    fileprivate func copied(_ content: Data) {
        guard UIApplication.shared.applicationState == .active, view.isFirstResponder else { return }
        guard !content.isEmpty, content.count <= 1 << 20, let s = String(data: content, encoding: .utf8) else { return }
        UIPasteboard.general.string = s
    }
}

/// 终端外面的一层（UIScrollView）：终端照会话的大小排——宿主说的（`hostSized`；旧的宿主不说，照这块地方排）。会话比这块地方宽时
/// （电脑上最后打的字：会话照电脑的窗口排）缩小到正好放得下宽度，双指捏着放大（最大到平常的字号）、双指拖着看；手机上打了字，
/// 会话照这台手机排，正好放得下，不缩放。一个手指的还是终端自己的（上下拖是滚动、长按选字、点一下弹键盘）
final class TerminalBox: UIScrollView, UIScrollViewDelegate {
    let terminal: SizedTerminalView
    /// 这块地方的大小变了（报给会话的窗口大小跟着变）
    var onLayout: (() -> Void)?
    /// 宿主说的会话的大小；nil：没说（旧的宿主），照这块地方排
    private var host: (cols: Int, rows: Int)?
    /// 终端现在排成的
    private var shown: (cols: Int, rows: Int)?
    private var laidOut = CGSize.zero
    /// 终端画得最细的那一层不超过这么多像素：特别大的会话放大到平常的字号时，一层也不会占掉几十兆内存
    static let pixelsMax: CGFloat = 8_000_000

    init(terminal: SizedTerminalView) {
        self.terminal = terminal
        super.init(frame: CGRect(x: 0, y: 0, width: 320, height: 480))
        delegate = self
        backgroundColor = TerminalSurface.background
        contentInsetAdjustmentBehavior = .never
        indicatorStyle = .white
        bounces = false
        bouncesZoom = true
        delaysContentTouches = false
        panGestureRecognizer.minimumNumberOfTouches = 2   // 一个手指的是终端自己的
        terminal.panGestureRecognizer.maximumNumberOfTouches = 1   // 两个手指的是这一层的
        addSubview(terminal)
    }

    required init?(coder: NSCoder) {
        fatalError("不从 storyboard 建")
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        guard bounds.size != laidOut else { return }   // 拖着、缩放着的时候也一直来：只管大小变了的
        laidOut = bounds.size
        arrange()
        onLayout?()
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        if window != nil {
            arrange()
            onLayout?()
        }
    }

    /// 一格多大（点）：SwiftTerm 照字体算的
    private var cell: CGSize? {
        let t = terminal.getTerminal()
        let f = terminal.getOptimalFrameSize()
        guard t.cols > 0, t.rows > 0, f.width > 0, f.height > 0 else { return nil }
        return CGSize(width: f.width / CGFloat(t.cols), height: f.height / CGFloat(t.rows))
    }

    /// 这块地方照平常的字号放得下几列几行：报给宿主的这台手机的窗口大小
    func natural() -> (cols: Int, rows: Int)? {
        guard let cell, bounds.width >= 1, bounds.height >= 1 else { return nil }
        let c = Int(bounds.width / cell.width), r = Int(bounds.height / cell.height)
        return c >= 2 && r >= 2 ? (c, r) : nil
    }

    /// 宿主说了会话现在的大小
    func hostSized(cols: Int, rows: Int) {
        host = (cols, rows)
        arrange()
    }

    /// 终端照会话的大小排，马上排好（之后到的输出是照这个大小画的）；比这块地方宽就缩小到正好放得下宽度。会话的大小变了才重新定
    /// 缩放、看最下面那一截（Claude Code 这些等你输入的地方在最下面）；只是这块地方变了（键盘、转屏）的留着现在的缩放
    private func arrange() {
        guard let cell, bounds.width >= 1, bounds.height >= 1, let want = host ?? natural() else { return }
        // 多给半个点：SwiftTerm 拿这块的大小除以一格的大小再取整，算出来正好是 want，不会差一
        let size = CGSize(width: CGFloat(want.cols) * cell.width + 0.5, height: CGFloat(want.rows) * cell.height + 0.5)
        let changed = shown.map { $0.cols != want.cols || $0.rows != want.rows } ?? true
        if changed {
            zoomScale = 1
            terminal.frame = CGRect(origin: .zero, size: size)
            contentSize = size
            terminal.layoutIfNeeded()
            shown = want
            #if DEBUG
            TerminalSurface.sizeSpy?("shown \(want.cols) \(want.rows)")
            #endif
        }
        let fit = min(1, bounds.width / size.width)
        minimumZoomScale = min(fit, bounds.height / size.height)
        maximumZoomScale = 1
        if changed {
            zoomScale = fit
            contentOffset = CGPoint(x: 0, y: max(0, contentSize.height - bounds.height))
        } else if zoomScale < minimumZoomScale {
            zoomScale = minimumZoomScale
        }
        sharpen()
    }

    func viewForZooming(in scrollView: UIScrollView) -> UIView? {
        terminal
    }

    func scrollViewDidEndZooming(_ scrollView: UIScrollView, with view: UIView?, atScale scale: CGFloat) {
        sharpen()
    }

    /// 终端画得多细：正好是屏幕上的像素（缩小了的不用画那么细：省内存、画得快），一层最多 `pixelsMax` 个像素
    private func sharpen() {
        let screen = traitCollection.displayScale > 0 ? traitCollection.displayScale : 2
        let s = terminal.bounds.size
        let cap = (Self.pixelsMax / max(1, s.width * s.height)).squareRoot()
        let want = max(1, min(screen, screen * zoomScale, cap))
        guard abs(terminal.contentScaleFactor - want) > 0.01 else { return }
        terminal.contentScaleFactor = want
        terminal.setNeedsDisplay()
    }
}

/// SwiftTerm 的终端视图，多管几件事：只看的不弹键盘；手指上下拖动是滚动。
///
/// 滚动照 Mac 上 SwiftTerm 的 scrollWheel 分三种（`scrollTarget`）：会话开着鼠标上报时（Claude Code 的全屏界面：备用屏幕，
/// 往回翻由它自己做）发滚轮，手指每移一行的高度发一格（Claude Code 2.1.332 实测：隔开的一格滚一行，连着来的加速到两行左右）；
/// 备用屏幕、没开鼠标上报（less、man 这类）发方向键；别的时候是 SwiftTerm 自己的回滚（UIScrollView）。松手时手指还在动的照惯性
/// 再滑一段，手指一按下就停。SwiftTerm 自己在会话开鼠标上报时装一个把拖动当成按住鼠标拖的手势（电脑上拖着选字的那种），手机上
/// 就滚不动了：不要它（`mouseModeChanged` 不调 super）
final class SizedTerminalView: TerminalView {
    var readOnly = false

    enum ScrollTarget { case native, wheel, arrows }

    /// 松手后的惯性每毫秒剩下多少（UIScrollView 的 .normal 是 0.998、.fast 是 0.99）：Claude Code 连着来的滚轮一格滚两行左右，
    /// 照 .normal 滑得太远
    static let deceleration: CGFloat = 0.996
    /// 一次最多发几格：手指再快、卡了一下，也不一下子灌进去一大串
    static let stepMax = 8

    private var wheelPan: UIPanGestureRecognizer?
    private let gate = WheelGate()
    /// 还没凑够一行的那一截（点）
    private var rest: CGFloat = 0
    private var glide: CADisplayLink?
    private var glideSpeed: CGFloat = 0
    private var glideAt = CGPoint.zero
    private var glideLast: CFTimeInterval = 0

    override func didMoveToWindow() {
        super.didMoveToWindow()
        if window != nil {
            installWheel()
        } else {
            stopGlide()
        }
    }

    override var canBecomeFirstResponder: Bool {
        !readOnly
    }

    override func mouseModeChanged(source: Terminal) {
        // 不调 super（见上）。会话关了鼠标上报：还在滑的停下
        if source.mouseMode == .off { stopGlide() }
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        stopGlide()   // 手指按下：惯性停住（和列表一样）
        super.touchesBegan(touches, with: event)
    }

    /// 现在上下拖是什么
    var scrollTarget: ScrollTarget {
        let t = getTerminal()
        if readOnly { return .native }   // 只看：发不了键，滚轮、方向键都送不过去
        if allowMouseReporting && t.mouseMode != .off { return .wheel }
        if t.isCurrentBufferAlternate && t.alternateScrollMode && !selectionActive { return .arrows }
        return .native
    }

    private func installWheel() {
        guard wheelPan == nil else { return }
        let g = UIPanGestureRecognizer(target: self, action: #selector(panned(_:)))
        g.maximumNumberOfTouches = 1
        gate.view = self
        g.delegate = gate
        addGestureRecognizer(g)
        panGestureRecognizer.require(toFail: g)   // SwiftTerm 自己的回滚：等它不接手了再动
        wheelPan = g
    }

    @objc private func panned(_ g: UIPanGestureRecognizer) {
        switch g.state {
        case .began, .changed:
            if g.state == .began {
                stopGlide()
                rest = 0
            }
            let dy = g.translation(in: self).y
            g.setTranslation(.zero, in: self)
            scroll(by: dy, at: g.location(in: self))
        case .ended:
            glideFrom(speed: g.velocity(in: self).y, at: g.location(in: self))
        default:
            rest = 0
        }
    }

    /// 手指（或者惯性）往下移了 dy 点：凑够一行发一格。往下拖是往回看（滚轮往上），和列表一样
    private func scroll(by dy: CGFloat, at point: CGPoint) {
        let target = scrollTarget
        guard target != .native, let cell = cellSize() else {
            rest = 0
            stopGlide()
            return
        }
        rest += dy
        var n = Int(rest / cell.height)
        rest -= CGFloat(n) * cell.height
        if abs(n) > Self.stepMax {
            n = n > 0 ? Self.stepMax : -Self.stepMax
            rest = 0
        }
        guard n != 0 else { return }
        let t = getTerminal()
        if target == .wheel {
            // 手指在哪一格（程序可能按位置认滚的是哪一块）。UIScrollView 的坐标带着滚过的距离：减掉，是屏幕上的第几行
            let x = max(0, min(point.x - contentOffset.x, bounds.width - 1))
            let y = max(0, min(point.y - contentOffset.y, bounds.height - 1))
            let col = min(t.cols - 1, Int(x / cell.width)), row = min(t.rows - 1, Int(y / cell.height))
            let scale = window?.contentScaleFactor ?? 1
            let flags = t.encodeButton(button: n > 0 ? 4 : 5, release: false, shift: false, meta: false, control: false)
            for _ in 0..<abs(n) {
                t.sendEvent(buttonFlags: flags, x: col, y: row, pixelX: Int(x * scale), pixelY: Int(y * scale))
            }
        } else {
            let key = n > 0 ? (t.applicationCursor ? EscapeSequences.moveUpApp : EscapeSequences.moveUpNormal)
                : (t.applicationCursor ? EscapeSequences.moveDownApp : EscapeSequences.moveDownNormal)
            let keys = Array(Array(repeating: key, count: abs(n)).joined())
            send(data: keys[...])
        }
    }

    /// 一格多大（点）：SwiftTerm 只给出按窗口的缩放取整过的像素
    private func cellSize() -> CGSize? {
        guard let px = cellSizeInPixels(source: getTerminal()), px.width > 0, px.height > 0 else { return nil }
        let scale = window?.contentScaleFactor ?? 1
        return CGSize(width: CGFloat(px.width) / scale, height: CGFloat(px.height) / scale)
    }

    /// 松手时手指还在动：照惯性接着滑，越来越慢；慢下来、手指按下、会话关了鼠标上报、视图不在了就停
    private func glideFrom(speed: CGFloat, at point: CGPoint) {
        stopGlide()
        guard abs(speed) > 150, scrollTarget != .native else { return }
        glideSpeed = speed
        glideAt = point
        glideLast = CACurrentMediaTime()
        let link = CADisplayLink(target: self, selector: #selector(glideStep(_:)))
        link.add(to: .main, forMode: .common)
        glide = link
    }

    @objc private func glideStep(_ link: CADisplayLink) {
        let now = link.timestamp
        let dt = CGFloat(min(max(now - glideLast, 0), 0.1))
        glideLast = now
        scroll(by: glideSpeed * dt, at: glideAt)
        glideSpeed *= pow(Self.deceleration, dt * 1000)
        if abs(glideSpeed) < 40 { stopGlide() }
    }

    private func stopGlide() {
        glide?.invalidate()   // CADisplayLink 留着 self：停的时候一定放掉
        glide = nil
        glideSpeed = 0
    }
}

#if DEBUG
extension SizedTerminalView {
    /// TestDriver 用：会话开着鼠标上报时，当成手指在当中往下拖了 rows 行（发 rows 格往上的滚轮）；没开是 false
    func testDrag(rows: Int) -> Bool {
        guard scrollTarget == .wheel, let cell = cellSize() else { return false }
        rest = 0
        scroll(by: (CGFloat(rows) + 0.5) * cell.height, at: CGPoint(x: bounds.midX, y: bounds.midY))
        rest = 0
        return true
    }
}
#endif

/// 自己的拖动手势什么时候接手：会话要滚轮、方向键的时候，并且是上下拖（左右拖的不管）
private final class WheelGate: NSObject, UIGestureRecognizerDelegate {
    weak var view: SizedTerminalView?

    func gestureRecognizerShouldBegin(_ g: UIGestureRecognizer) -> Bool {
        guard let view, let pan = g as? UIPanGestureRecognizer, view.scrollTarget != .native else { return false }
        let v = pan.velocity(in: view)
        return abs(v.y) > abs(v.x)
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
        return s.box
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
