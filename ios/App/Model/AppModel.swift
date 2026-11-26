import CcaRemoteKit
import Observation
import SwiftUI

/// 外观：跟随系统、浅色、深色（和 Mac App 一样三种）
enum AppearanceMode: String, CaseIterable, Identifiable {
    case system, light, dark

    var id: String { rawValue }

    var title: String {
        switch self {
        case .system: return "跟随系统"
        case .light: return "浅色"
        case .dark: return "深色"
        }
    }

    var scheme: ColorScheme? {
        switch self {
        case .system: return nil
        case .light: return .light
        case .dark: return .dark
        }
    }
}

/// 配对的那一页现在到哪了
enum PairPhase: Equatable {
    /// 扫码或者粘贴
    case choosing
    /// 经链接打开的（系统相机扫的、点的链接）：先问一句
    case confirm(PairInvite)
    case working(PairInvite, PairStep)
    case done(Peer)
    case failed(PairInvite?, String)

    static func == (a: PairPhase, b: PairPhase) -> Bool {
        switch (a, b) {
        case (.choosing, .choosing): return true
        case let (.confirm(x), .confirm(y)): return x == y
        case let (.working(x, s), .working(y, t)): return x == y && s == t
        case let (.done(x), .done(y)): return x == y
        case let (.failed(x, m), .failed(y, n)): return x == y && m == n
        default: return false
        }
    }
}

/// App 的状态：配对过的电脑、它们上面的会话、配对、开着的终端。界面只读它、调它的方法
@MainActor
@Observable
final class AppModel {
    static let refreshEvery: TimeInterval = 15   // 和 Mac App 的「远程会话」一页一样（服务器给每台设备限了流）

    private(set) var backend: Backend
    private(set) var peers: [Peer] = []
    private(set) var listings: [String: PeerListing] = [:]
    private(set) var loading = false
    private(set) var fetchedAt: Date?
    /// 配对的那一页（nil 是没开）
    var pairing: PairPhase?
    /// 开着的终端（推到导航里的那一页）
    var terminal: TerminalSession?
    var showSettings = false

    var appearance: AppearanceMode {
        didSet { UserDefaults.standard.set(appearance.rawValue, forKey: "appearance") }
    }

    /// 这台手机在电脑那头叫什么（配对时告诉对方；改了只影响以后的配对）
    var deviceName: String {
        didSet {
            UserDefaults.standard.set(deviceName, forKey: "deviceName")
            (backend as? LiveBackend)?.client.deviceName = deviceName
        }
    }

    @ObservationIgnored private var poll: Task<Void, Never>?
    @ObservationIgnored private var pairTask: Task<Void, Never>?

    init(demo: Bool = false) {
        let d = UserDefaults.standard
        appearance = AppearanceMode(rawValue: d.string(forKey: "appearance") ?? "") ?? .system
        let name = Texts.nonEmpty(d.string(forKey: "deviceName")) ?? Self.defaultName
        deviceName = name
        backend = demo ? DemoBackend() : LiveBackend(deviceName: name)
        peers = backend.peers()
    }

    static var defaultName: String {
        UIDevice.current.userInterfaceIdiom == .pad ? "我的 iPad" : "我的 iPhone"
    }

    var isDemo: Bool { backend.isDemo }

    // MARK: 演示

    func enterDemo() {
        switchBackend(DemoBackend())
    }

    func leaveDemo() {
        switchBackend(LiveBackend(deviceName: deviceName))
    }

    private func switchBackend(_ b: Backend) {
        closeTerminal()
        pairTask?.cancel()
        pairing = nil
        backend = b
        peers = b.peers()
        listings = [:]
        fetchedAt = nil
        loading = false   // 前一个那头还没查完的，回来时看到换了就不管了（fetch 里的 b === backend）
        refresh()
        Pusher.shared.start(self)
    }

    // MARK: 会话

    /// 列表看得见、App 在前台时：马上查一次，之后每 15 秒一次
    func startPolling() {
        guard poll == nil else { return }
        poll = Task { [weak self] in
            while !Task.isCancelled {
                await self?.fetch()
                try? await Task.sleep(for: .seconds(Self.refreshEvery))
            }
        }
    }

    func stopPolling() {
        poll?.cancel()
        poll = nil
    }

    func refresh() {
        Task { await fetch() }
    }

    func fetch() async {
        peers = backend.peers()
        guard !peers.isEmpty, !loading else { return }
        loading = true
        let b = backend
        let rows = await b.list()
        guard b === backend else { return }   // 中途切了演示
        loading = false
        listings = Dictionary(rows.map { ($0.peer.pid, $0) }, uniquingKeysWith: { a, _ in a })
        fetchedAt = Date()
    }

    func listing(_ p: Peer) -> PeerListing? { listings[p.pid] }

    // MARK: 配对

    func startPairing() {
        pairing = .choosing
    }

    /// 经链接打开 App（系统相机扫了二维码、点了链接）
    func open(_ url: URL) {
        guard let inv = PairInvite(url.absoluteString) else {
            pairing = .failed(nil, "这不是 cca 的配对链接。在电脑上点「和手机配对…」，用这个 App 扫那里的二维码。")
            return
        }
        // 正在配对时不打断
        if case .working = pairing { return }
        pairing = .confirm(inv)
    }

    /// 扫到的、粘贴的文字
    func scanned(_ text: String) {
        guard let inv = PairInvite(text) else {
            pairing = .failed(nil, "这不是 cca 的配对二维码。在电脑上点「和手机配对…」，扫那里的二维码。")
            return
        }
        pair(inv)
    }

    func pair(_ inv: PairInvite) {
        pairing = .working(inv, .deriving)
        let b = backend
        pairTask = Task { [weak self] in
            do {
                let peer = try await b.pair(inv) { step in
                    Task { @MainActor in
                        guard let self, case .working(let i, _) = self.pairing, i == inv else { return }
                        self.pairing = .working(inv, step)
                    }
                }
                guard let self, b === self.backend else { return }
                self.pairing = .done(peer)
                self.peers = b.peers()
                Pusher.shared.peersChanged(on: peer.server)   // 通知密钥、在这台服务器上登记推送；头一次配好时问能不能发通知
                await self.fetch()
            } catch is CancellationError {
            } catch {
                guard let self, b === self.backend else { return }
                self.pairing = .failed(inv, Texts.pairFailure((error as? RemoteFailure)?.code ?? "error"))
            }
        }
    }

    func cancelPairing() {
        pairTask?.cancel()
        pairing = nil
    }

    func unpair(_ p: Peer) {
        if terminal?.peer.pid == p.pid { closeTerminal() }
        let b = backend
        Task {
            await b.unpair(p)
            guard b === backend else { return }
            peers = b.peers()
            listings[p.pid] = nil
            Pusher.shared.peersChanged(on: p.server)
        }
    }

    // MARK: 终端

    func attach(_ s: RemoteSession, on peer: Peer, readOnly: Bool) {
        closeTerminal()
        terminal = TerminalSession(backend: backend, peer: peer, session: s, readOnly: readOnly || s.info.viewOnly)
    }

    func closeTerminal() {
        terminal?.close()
        terminal = nil
    }

    /// 点了通知：回到那个会话的终端。正开着就不动；关了的重新接上（会话还在、那台电脑在线时）
    func reopen(peer pid: String, sid: String, readOnly: Bool) {
        if let t = terminal, t.peer.pid == pid, t.session.sid == sid {
            t.nudge()
            return
        }
        let b = backend
        Task {
            // 刚启动（点通知把 App 打开的）时列表还没查过、正在查：等一等
            for i in 0..<5 {
                guard b === backend else { return }
                if let p = peers.first(where: { $0.pid == pid }), let s = listings[pid]?.sessions.first(where: { $0.sid == sid }) {
                    if s.online, terminal?.session.sid != sid { attach(s, on: p, readOnly: readOnly) }
                    return
                }
                if i > 0 { try? await Task.sleep(for: .seconds(1)) }
                await fetch()
            }
        }
    }

    /// App 回到前台（系统设置里的通知开关可能改过：Pusher 对一对登记）
    func becameActive() {
        Pusher.shared.sync()
        if let t = terminal {
            t.nudge()
        } else {
            startPolling()   // 列表那一页：马上查一次（开着终端时不查，回到列表时 SessionsView 自己开始）
        }
    }

    func wentBackground() {
        stopPolling()
    }
}

/// 一个开着的终端：哪个会话、连接的状态、终端的标题。画面（SwiftTerm 的视图）在 TerminalSurface 里，这里只管字节进出
@MainActor
@Observable
final class TerminalSession: Identifiable, Hashable {
    nonisolated static func == (a: TerminalSession, b: TerminalSession) -> Bool { a === b }
    nonisolated func hash(into h: inout Hasher) { h.combine(ObjectIdentifier(self)) }

    let id = UUID()
    let peer: Peer
    let session: RemoteSession
    let readOnly: Bool
    private(set) var state: Attachment.State = .connecting
    private(set) var exitCode: Int32?
    var title = ""
    @ObservationIgnored private let backend: Backend
    @ObservationIgnored private var link: TerminalLink?
    /// 终端的字节交给谁（TerminalSurface 接上以后设）；接上之前来的先攒着
    @ObservationIgnored var sink: ((Bytes) -> Void)? {
        didSet {
            guard let sink, !pending.isEmpty else { return }
            sink(pending)
            pending = []
        }
    }
    @ObservationIgnored private var pending = Bytes()
    @ObservationIgnored var surface: AnyObject?

    init(backend: Backend, peer: Peer, session: RemoteSession, readOnly: Bool) {
        self.backend = backend
        self.peer = peer
        self.session = session
        self.readOnly = readOnly
    }

    var isOpen: Bool { state == .open }

    var isClosed: Bool {
        if case .closed = state { return true }
        return false
    }

    /// 终端排好版、知道了几列几行：头一次是接入，之后是改大小
    func sized(cols: Int, rows: Int) {
        if let link {
            link.resize(cols: cols, rows: rows)
            return
        }
        guard !isClosed else { return }
        let l = backend.link(peer: peer, session: session, cols: cols, rows: rows, readOnly: readOnly)
        // 换掉了的那一条（重新接入、关掉以后）再来的事件不算数
        l.onOutput = { [weak self, weak l] b in if let self, self.link === l { self.output(b) } }
        l.onState = { [weak self, weak l] s in if let self, self.link === l { self.state = s } }
        l.onExit = { [weak self, weak l] c in if let self, self.link === l { self.exitCode = c } }
        l.onNote = { [weak self, weak l] n in if let self, self.link === l { self.noted(n) } }
        link = l
        l.start()
    }

    private func output(_ b: Bytes) {
        if let sink {
            sink(b)
        } else if pending.count < 1 << 20 {
            pending += b
        }
    }

    func send(_ b: Bytes) {
        guard !readOnly else { return }
        link?.input(b)
    }

    /// 会话推来一条通知（宿主转来的 NOTIFY，或者会话里的程序写的 OSC 777 / 9）：弹成这台手机的通知（Notifier）
    func noted(_ n: Note) {
        Notifier.post(n, from: self)
    }

    /// 会话里的程序自己写的通知（OSC 777 / 9，TerminalSurface 认出来的）
    func noted(title: String, body: String) {
        noted(Note(title: title, body: body))
    }

    func nudge() { link?.nudge() }

    /// 重新接入（断了、没接上以后，用户点的）
    func reconnect(cols: Int, rows: Int) {
        link?.detach()
        link = nil
        state = .connecting
        exitCode = nil
        sized(cols: cols, rows: rows)
    }

    func close() {
        link?.detach()
        link = nil
    }
}
