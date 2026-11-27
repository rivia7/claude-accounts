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

/// 底下的两页：账号（各台电脑上的账号的状态、用量）、会话（可以接入的远程会话）
enum AppTab: String {
    case accounts, sessions
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
    var terminal: TerminalSession? {
        didSet {
            guard oldValue !== terminal else { return }
            // 开着的会话变了：「已关闭的会话也推送」关着时，服务器上登记的只推哪个会话跟着变；开着终端时不查列表
            Pusher.shared.openChanged()
            updatePolling()
        }
    }
    /// 终端是从哪一页推进去的（账号页里点了这个账号的会话的，返回时回到账号页）
    private(set) var terminalTab: AppTab = .sessions
    var showSettings = false
    /// 「新建会话」那一页开着的：在哪台电脑上起（nil 是没开）
    var launchFor: Peer?
    /// 底下选着的那一页（记着，下次打开还是它）
    var tab: AppTab {
        didSet { UserDefaults.standard.set(tab.rawValue, forKey: "tab") }
    }
    /// 「账号」页推进去的账号
    var accountPath: [AccountRef] = []
    /// 「账号」页看哪个工具的账号（claude、codex、agy）、只看哪种套餐（cca 给的原名，如 Pro、Max、Plus，空的是全部）：记着，
    /// 下次打开还是它们。记着的已经没有账号了时照默认的看（AccountShelf）
    var accountTool: String {
        didSet { UserDefaults.standard.set(accountTool, forKey: "accountTool") }
    }
    var accountPlan: String {
        didSet { UserDefaults.standard.set(accountPlan, forKey: "accountPlan") }
    }

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
    /// App 在前台（回到后台时不查）
    @ObservationIgnored private var foreground = true

    init(demo: Bool = false) {
        let d = UserDefaults.standard
        appearance = AppearanceMode(rawValue: d.string(forKey: "appearance") ?? "") ?? .system
        tab = AppTab(rawValue: d.string(forKey: "tab") ?? "") ?? .sessions
        accountTool = d.string(forKey: "accountTool") ?? "claude"
        accountPlan = d.string(forKey: "accountPlan") ?? ""
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
        launchFor = nil
        backend = b
        peers = b.peers()
        listings = [:]
        accountPath = []
        fetchedAt = nil
        loading = false   // 前一个那头还没查完的，回来时看到换了就不管了（fetch 里的 b === backend）
        refresh()
        Pusher.shared.start(self)
    }

    // MARK: 会话

    /// App 在前台、没开着终端时查列表（会话和账号的状态是同一次查到的）：马上查一次，之后每 15 秒一次
    func updatePolling() {
        if foreground && terminal == nil {
            startPolling()
        } else {
            stopPolling()
        }
    }

    private func startPolling() {
        guard poll == nil else { return }
        poll = Task { [weak self] in
            while !Task.isCancelled {
                await self?.fetch()
                try? await Task.sleep(for: .seconds(Self.refreshEvery))
            }
        }
    }

    private func stopPolling() {
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

    // MARK: 账号

    /// 一台电脑放上来的账号的状态（那台电脑上的 cca、它所在的服务器都够新、放过一次以后才有）
    func status(_ p: Peer) -> HostStatus? { listings[p.pid]?.status }

    /// 「账号」页推进去的那个账号：哪台电脑、账号现在的状态（那台电脑后来没放它了是 nil）
    func account(_ ref: AccountRef) -> (peer: Peer, account: AccountStatus)? {
        guard let p = peers.first(where: { $0.pid == ref.pid }), let a = status(p)?.accounts.first(where: { $0.id == ref.id }) else { return nil }
        return (p, a)
    }

    /// 打开一个账号的那一页（点了额度的通知）：关掉开着的终端，换到「账号」页
    func openAccount(peer pid: String, account id: String) {
        closeTerminal()
        tab = .accounts
        let ref = AccountRef(pid: pid, id: id)
        accountPath = [ref]
        // 返回时列表里要有它：换到它的工具，筛着的套餐不是它的就不筛了（还没查到它的照上次的选择）
        if let a = account(ref)?.account {
            accountTool = a.tool
            if !accountPlan.isEmpty, a.plan != accountPlan { accountPlan = "" }
        }
        refresh()
    }

    // MARK: 配对

    func startPairing() {
        pairing = .choosing
    }

    /// 经链接打开 App（系统相机扫了二维码、点了链接）
    func open(_ url: URL) {
        guard let inv = PairInvite(url.absoluteString) else {
            pairing = .failed(nil, "这不是 cca 的配对链接。请在电脑上点「和手机配对…」，然后用本 App 扫描二维码。")
            return
        }
        // 正在配对时不打断
        if case .working = pairing { return }
        pairing = .confirm(inv)
    }

    /// 扫到的、粘贴的文字
    func scanned(_ text: String) {
        guard let inv = PairInvite(text) else {
            pairing = .failed(nil, "这不是 cca 的配对二维码。请在电脑上点「和手机配对…」，扫描显示的二维码。")
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
        if launchFor?.pid == p.pid { launchFor = nil }
        let b = backend
        Task {
            await b.unpair(p)
            guard b === backend else { return }
            peers = b.peers()
            listings[p.pid] = nil
            Pusher.shared.peersChanged(on: p.server)
        }
    }

    // MARK: 新建会话（远程启动）

    /// 一台电脑的启动器：那台电脑让这台手机远程启动会话、它的启动器在服务器上登记着时才有
    func launcher(_ p: Peer) -> HostLauncher? { listings[p.pid]?.launcher }

    /// 请那台电脑照模板起一个会话（多大见 TerminalGrid）。起来了：关掉「新建会话」那一页，打开新会话的终端（fresh：服务器上可能
    /// 还没有它），回 nil；没起成：回原因码和要说的那句话，列表过时了的（模板变了、没了，不让启动了，启动器不在了）顺手刷新
    func launch(_ t: LaunchTemplate, on peer: Peer, line: String?) async -> (code: String, text: String)? {
        guard let l = launcher(peer) else { return ("no_session", Texts.launchUnreached("no_session")) }
        let size = TerminalGrid.forLaunch
        let b = backend
        let r = await b.launch(peer: peer, launcher: l, template: t, cols: size.cols, rows: size.rows, line: line)
        guard b === backend else { return nil }   // 中途切了演示
        switch r {
        case .launched(let sid):
            let info = SessionInfo(machine: l.info.machine, tool: t.tool, name: t.account, cwd: t.dir, since: Date().timeIntervalSince1970,
                                   viewOnly: false, aid: t.aid)
            launchFor = nil
            closeTerminal()
            terminalTab = tab
            terminal = TerminalSession(backend: b, peer: peer, session: RemoteSession(sid: sid, info: info, online: true, viewers: 0),
                                       readOnly: false, fresh: true)
            return nil
        case .refused(let code, let text):
            if ["changed", "no_template", "denied", "other_hub"].contains(code) { refresh() }
            return (code, Texts.launchFailure(code: code, text: text))
        case .failed(let f):
            if f.code == "no_session" || f.code == "nohost" { refresh() }
            return (f.code, Texts.launchUnreached(f.code))
        }
    }

    // MARK: 终端

    /// 接入一个会话：终端推到现在这一页的导航里（账号页里点的，返回时回到账号页）
    func attach(_ s: RemoteSession, on peer: Peer, readOnly: Bool) {
        closeTerminal()
        terminalTab = tab
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
        foreground = true
        terminal?.nudge()
        updatePolling()   // 没开着终端：马上查一次
    }

    func wentBackground() {
        foreground = false
        updatePolling()
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
    /// 刚远程启动的会话：头一次接入时服务器上可能还没有它（Attachment 的 fresh）；重新接入时不算
    @ObservationIgnored private var fresh: Bool
    @ObservationIgnored var surface: AnyObject?
    /// 会话现在的大小（新的宿主说的）：nil 是没说，会话照这台手机的窗口排
    @ObservationIgnored private(set) var hostSize: (cols: Int, rows: Int)?
    /// 宿主说了会话的大小交给谁（TerminalSurface 接上以后设）
    @ObservationIgnored var sizeSink: ((Int, Int) -> Void)?

    init(backend: Backend, peer: Peer, session: RemoteSession, readOnly: Bool, fresh: Bool = false) {
        self.backend = backend
        self.peer = peer
        self.session = session
        self.readOnly = readOnly
        self.fresh = fresh
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
        let l = backend.link(peer: peer, session: session, cols: cols, rows: rows, readOnly: readOnly, fresh: fresh)
        // 换掉了的那一条（重新接入、关掉以后）再来的事件不算数
        l.onOutput = { [weak self, weak l] b in if let self, self.link === l { self.output(b) } }
        l.onSize = { [weak self, weak l] c, r in if let self, self.link === l { self.hostSized(c, r) } }
        l.onState = { [weak self, weak l] s in if let self, self.link === l { self.state = s } }
        l.onExit = { [weak self, weak l] c in if let self, self.link === l { self.exitCode = c } }
        l.onNote = { [weak self, weak l] n in if let self, self.link === l { self.noted(n) } }
        link = l
        l.start()
    }

    private func hostSized(_ c: Int, _ r: Int) {
        hostSize = (c, r)
        sizeSink?(c, r)
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
        // 打了字：新的宿主照这台手机的窗口排会话，下次远程启动就照这个大小起
        let s = terminalSize
        TerminalGrid.remember(cols: s.cols, rows: s.rows)
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
        fresh = false
        state = .connecting
        exitCode = nil
        sized(cols: cols, rows: rows)
    }

    func close() {
        link?.detach()
        link = nil
    }
}

/// 新会话（远程启动）照多大起：这台手机上次在会话里打字时的窗口大小（新的宿主那时照它排会话；记在 UserDefaults）。还没打过字的照屏幕
/// 算：宽是去掉安全区放得下的列数，高只算一半（终端打开时键盘弹出来，差不多占掉下面一半：宁可矮一点，高了的话新会话开头的那几行在
/// 屏幕上面、看到的是下面空着的那一截）。夹到 LaunchAsk 的范围里。只管起的时候：起来以后手机上一打字，会话就照这块屏幕排
enum TerminalGrid {
    private static let key = "terminalGrid"

    static func remember(cols: Int, rows: Int) {
        let d = UserDefaults.standard
        guard (d.array(forKey: key) as? [Int]) != [cols, rows] else { return }
        d.set([cols, rows], forKey: key)
    }

    @MainActor
    static var forLaunch: (cols: Int, rows: Int) {
        let last = (UserDefaults.standard.array(forKey: key) as? [Int]).flatMap { $0.count == 2 ? ($0[0], $0[1]) : nil }
        let (c, r) = last ?? guess
        return LaunchAsk.clamp(cols: c, rows: r)
    }

    /// 照屏幕算：现在这个窗口去掉安全区、导航栏（44 点），高再减半；一格多大照 SwiftTerm 的算法（字的上下高度加行距取整，“W” 的宽）
    @MainActor
    private static var guess: (Int, Int) {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        guard let w = scenes.first(where: { $0.activationState == .foregroundActive })?.keyWindow ?? scenes.first?.windows.first else {
            return (80, 24)
        }
        let area = w.bounds.inset(by: w.safeAreaInsets)
        let f = TerminalSurface.font
        let cell = CGSize(width: ("W" as NSString).size(withAttributes: [.font: f]).width, height: ceil(f.ascender - f.descender + f.leading))
        guard cell.width > 0, cell.height > 0 else { return (80, 24) }
        return (Int(area.width / cell.width), Int((area.height - 44) / cell.height) / 2)
    }
}
