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

/// 底下的几页（和 Mac App 侧栏的先后、叫法一样）：总览（各台电脑上的账号，一个账号一张卡片）、用量统计、折合金额、
/// 远程会话（可以接入的会话）、共享与同步（这些电脑所在的 Team）。总览以前叫「账号」，记着的还是原来的那个值
enum AppTab: String {
    case overview = "accounts"
    case stats, cost, sessions, sharing
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

/// 「新建会话」那一页在哪台电脑上起；account：先选上的账号（账号页里点的「新建会话」：那个账号的编号，只列用得了它的模板）
struct LaunchTarget: Identifiable, Equatable {
    let peer: Peer
    var account: String?

    var id: String { peer.pid + "|" + (account ?? "") }
}

/// 浮在底下的一句提示（Mac App 的 Toast）：几秒后自己消失，读屏念出来（AppModel.showToast）
struct Toast: Identifiable, Equatable {
    enum Style: Equatable { case info, success, warning }
    let id = UUID()
    var text: String
    var style: Style
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
    /// 开着的终端（照接入的先后）：显示着的那个之外的照样接着（和 Mac App 一样），从终端返回不断开，在「远程会话」页的
    /// 「已打开的终端」里、终端页的菜单里断开。最多 terminalLimit 个
    private(set) var terminals: [TerminalSession] = [] {
        didSet {
            // 「已关闭的会话也推送」关着时，服务器上登记的只推开着的这些会话：开了、关了一个跟着改
            if oldValue.map(\.id) != terminals.map(\.id) { Pusher.shared.openChanged() }
        }
    }
    /// 显示着的那个终端（推到导航里的那一页）；nil 是没在看终端。看着终端时不查列表
    private(set) var terminal: TerminalSession? {
        didSet {
            guard oldValue !== terminal else { return }
            updatePolling()
        }
    }
    /// 最多同时开几个终端：每个都在会话那边占一处接入，断线时各自重试、都要问服务器（服务器给每台设备限了流），手机上来回切着看的
    /// 也就几个。Mac App 是 100 个（remoteTerminalLimit：一台机器最多开的会话数）
    nonisolated static let terminalLimit = 8
    /// 终端是从哪一页推进去的（账号页里点了这个账号的会话的，返回时回到账号页）
    private(set) var terminalTab: AppTab = .sessions
    var showSettings = false
    /// 「新建会话」那一页开着的：在哪台电脑上起、先选上哪个账号（nil 是没开）
    var launchFor: LaunchTarget?
    /// 正在等哪几台电脑回话（远程启动，配对的 pid）：「新建会话」那一页关了也照样等，起来了打开它的终端（和 Mac App 一样）
    private(set) var launching: Set<String> = []
    /// 没起成的原因（配对的 pid）：「新建会话」那一页上说；那一页已经关了的在底下的提示里说（toast）
    private(set) var launchFailure: [String: String] = [:]
    /// 正在后台结束的会话（列表里点了「结束会话」、没开着它能操作的终端的，见 end）
    private(set) var endingSessions: Set<String> = []
    /// 浮在底下的一句提示（Mac App 窗口底下的那一条）：开满了终端接不了、关了「新建会话」那一页以后没起成、结束会话成没成、取消了配对、
    /// 重新显示了提示卡片。5 秒后自己消失（点一下也消失），出来时读屏念一遍
    private(set) var toast: Toast?
    @ObservationIgnored private var toastTask: Task<Void, Never>?
    /// 新手教程、更新了什么、提示卡片（演示模式只记在内存里）
    let guide: Guide
    /// 会话列表里会话的先后（拖动排的，CcaRemoteKit 的 SessionOrder）：记在 UserDefaults 里；演示模式的只在内存里，退出演示时丢掉
    private(set) var sessionOrder = SessionOrder()
    /// 会话列表在「排序」（List 的编辑状态：拖动右边的把手调整先后）
    var sortingSessions = false
    /// 底下选着的那一页（记着，下次打开还是它）
    var tab: AppTab {
        didSet { UserDefaults.standard.set(tab.rawValue, forKey: "tab") }
    }
    /// 「总览」「用量统计」「折合金额」各自推进去的账号
    var accountPath: [AccountRef] = []
    var statsPath: [AccountRef] = []
    var costPath: [AccountRef] = []
    /// 折合金额按天看（不然按周）：账号页的「折合金额」、折合金额页共用（Mac App 的 costGranularity），记着
    var costDays: Bool {
        didSet { UserDefaults.standard.set(costDays, forKey: "costDays") }
    }
    /// 「账号」页看哪个工具的账号（claude、codex、agy）、只看哪种套餐（cca 给的原名，如 Pro、Max、Plus，空的是全部）：记着，
    /// 下次打开还是它们。记着的已经没有账号了时照默认的看（AccountShelf）
    var accountTool: String {
        didSet { UserDefaults.standard.set(accountTool, forKey: "accountTool") }
    }
    var accountPlan: String {
        didSet { UserDefaults.standard.set(accountPlan, forKey: "accountPlan") }
    }
    /// 按 Team 筛（Mac App 侧栏的「按 Team 筛选」：只管显示），记着；记着的那个 Team 已经没有账号了时照全部的看
    var accountTeam: AccountShelf.TeamPick {
        didSet { UserDefaults.standard.set(accountTeam.stored, forKey: "accountTeam") }
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
        // 没选过时打开总览（和 Mac App 一样是首页）；以后照上次看的那一页
        tab = AppTab(rawValue: d.string(forKey: "tab") ?? "") ?? .overview
        accountTool = d.string(forKey: "accountTool") ?? "claude"
        accountPlan = d.string(forKey: "accountPlan") ?? ""
        accountTeam = AccountShelf.TeamPick(stored: d.string(forKey: "accountTeam") ?? "")
        costDays = d.bool(forKey: "costDays")
        let name = Texts.nonEmpty(d.string(forKey: "deviceName")) ?? Self.defaultName
        deviceName = name
        let b: Backend = demo ? DemoBackend() : LiveBackend(deviceName: name)
        let known = b.peers()
        // 打开时已经配对过电脑的（从有引导以前的版本更新上来的）不再给新手教程（Guide）
        guide = Guide(demo: demo, paired: !known.isEmpty)
        backend = b
        sessionOrder = demo ? SessionOrder() : Self.storedOrder()
        peers = known
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
        closeAllTerminals()
        pairTask?.cancel()
        pairing = nil
        launchFor = nil
        launching = []
        launchFailure = [:]
        endingSessions = []
        dismissToast()
        guide.reset(demo: b.isDemo)
        backend = b
        peers = b.peers()
        listings = [:]
        sessionOrder = b.isDemo ? SessionOrder() : Self.storedOrder()
        sortingSessions = false
        accountPath = []
        statsPath = []
        costPath = []
        fetchedAt = nil
        loading = false   // 前一个那头还没查完的，回来时看到换了就不管了（fetch 里的 b === backend）
        refresh()
        Pusher.shared.start(self)
    }

    // MARK: 提示

    /// 底下浮出一句提示（Mac App 的 showToast）：5 秒后自己消失；同时让读屏念出来（优先级高：打断正在念的，和 Mac App 一样）
    func showToast(_ text: String, _ style: Toast.Style) {
        let t = Toast(text: text, style: style)
        toast = t
        var spoken = AttributedString(text)
        spoken.accessibilitySpeechAnnouncementPriority = .high
        AccessibilityNotification.Announcement(spoken).post()
        toastTask?.cancel()
        toastTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(5))
            if !Task.isCancelled, self?.toast?.id == t.id { self?.toast = nil }
        }
    }

    func dismissToast() {
        toastTask?.cancel()
        toast = nil
    }

    // MARK: 会话

    /// App 在前台、没在看终端时查列表（会话和账号的状态是同一次查到的；开着的终端在后面接着也查）：马上查一次，之后每 15 秒一次
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
        // 开着的终端照新查到的说明更新（像能不能结束：刚远程启动的会话起的时候还不知道）
        for t in terminals {
            if let s = listings[t.peer.pid]?.sessions.first(where: { $0.sid == t.session.sid }) { t.adopt(s) }
        }
    }

    func listing(_ p: Peer) -> PeerListing? { listings[p.pid] }

    // MARK: 会话的先后

    private static let orderKey = "sessionOrder"

    private static func storedOrder() -> SessionOrder {
        SessionOrder(ids: UserDefaults.standard.stringArray(forKey: orderKey) ?? [])
    }

    /// 一台电脑的会话，照拖动排过的先后（排过的在前，新起的照列表原来的先后接在后面）
    func sessions(_ l: PeerListing) -> [RemoteSession] { sessionOrder.sorted(l.sessions, id: \.sid) }

    /// 拖动排一台电脑的会话（List 的 onMove 给的位置，照 sessions(_:) 的先后）：整组记下来；演示模式的不写进 UserDefaults
    func moveSessions(of l: PeerListing, from: IndexSet, to: Int) {
        var ids = sessions(l).map(\.sid)
        let before = ids
        ids.move(fromOffsets: from, toOffset: to)
        guard ids != before else { return }
        sessionOrder.record(ids)
        if !isDemo { UserDefaults.standard.set(sessionOrder.ids, forKey: Self.orderKey) }
    }

    // MARK: 账号

    /// 一台电脑放上来的账号的状态（那台电脑上的 cca、它所在的服务器都够新、放过一次以后才有）
    func status(_ p: Peer) -> HostStatus? { listings[p.pid]?.status }

    /// 底下「总览」的角标（Mac App 侧栏总览那一行的「N 个要处理」）：各台电脑上要用户动手的账号（没登录、登错号、登录失效，在用的也算；
    /// 额度用完只是等，不算）。全部工具、全部套餐都算，不跟着「总览」上面的切换、筛选走（Mac 的那一行也不跟）
    var accountsNeedingAttention: Int {
        let now = Date().timeIntervalSince1970
        return peers.reduce(0) { n, p in
            guard let s = status(p), !s.off else { return n }
            return n + AccountCensus(s.accounts, now: now).needsAttention
        }
    }

    /// 底下「远程会话」的角标（Mac App 侧栏那一行的「已接入 N 个」）：开着、还没断开的终端有几个。返回时不断开，这个数提醒还接着几个
    var attachedCount: Int { terminals.filter { !$0.isClosed }.count }

    /// 底下「共享与同步」的角标（Mac App 侧栏的「Team 要处理」）：在 Team 里被移除、记录没了、没同步成的电脑有几台
    var teamsNeedingAttention: Int {
        peers.filter { p in status(p).map { !$0.off && $0.team?.failing == true } ?? false }.count
    }

    /// 「账号」页推进去的那个账号：哪台电脑、账号现在的状态（那台电脑后来没放它了是 nil）
    func account(_ ref: AccountRef) -> (peer: Peer, account: AccountStatus)? {
        guard let p = peers.first(where: { $0.pid == ref.pid }), let a = status(p)?.accounts.first(where: { $0.id == ref.id }) else { return nil }
        return (p, a)
    }

    /// 打开一个账号的那一页（点了额度的通知）：从显示着的终端退回来（开着的照样接着），换到「总览」
    func openAccount(peer pid: String, account id: String) {
        hideTerminal()
        tab = .overview
        let ref = AccountRef(pid: pid, id: id)
        accountPath = [ref]
        // 返回时列表里要有它：换到它的工具，筛着的套餐不是它的就不筛了（还没查到它的照上次的选择）
        if let a = account(ref)?.account {
            accountTool = a.tool
            if !accountPlan.isEmpty, a.plan != accountPlan { accountPlan = "" }
            if !accountTeam.includes(a) { accountTeam = .all }
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
        for t in terminals where t.peer.pid == p.pid { closeTerminal(t) }
        if launchFor?.peer.pid == p.pid { launchFor = nil }
        let b = backend
        Task {
            await b.unpair(p)
            guard b === backend else { return }
            peers = b.peers()
            listings[p.pid] = nil
            Pusher.shared.peersChanged(on: p.server)
            showToast("已取消和「\(p.name)」的配对", .success)
        }
    }

    // MARK: 新建会话（远程启动）

    /// 一台电脑的启动器：那台电脑让这台手机远程启动会话、它的启动器在服务器上登记着时才有
    func launcher(_ p: Peer) -> HostLauncher? { listings[p.pid]?.launcher }

    /// 用得了这个账号的模板（账号页的「新建会话」）：3.7.10 起的 cca 看模板能挑的账号，旧版的看模板自己定的账号
    func launchTemplates(on p: Peer, for account: AccountStatus) -> [LaunchTemplate] {
        guard let l = launcher(p) else { return [] }
        return l.info.templates.filter { t in
            if let list = l.info.accounts(for: t) { return list.contains { LaunchSheet.same($0, account) } }
            return !t.aid.isEmpty && t.aid == account.aid
        }
    }

    /// 「新建会话」那一页点了「启动」：在 App 这里等回话（那一页关了也照样等，见 launching），没起成的原因交给那一页；那一页已经关了的
    /// 弹出来说（和 Mac App 的 startRemoteLaunch 一样）
    func startLaunch(_ t: LaunchTemplate, on peer: Peer, as account: LaunchAccount?, line: String?) {
        guard !launching.contains(peer.pid) else { return }
        launching.insert(peer.pid)
        launchFailure[peer.pid] = nil
        let b = backend
        Task {
            let why = await launch(t, on: peer, as: account, line: line)
            guard b === backend else { return }
            launching.remove(peer.pid)
            guard let why else { return }
            if launchFor?.peer.pid == peer.pid {
                launchFailure[peer.pid] = why.text
            } else {
                showToast("没能在「\(peer.name)」上新建会话：\(why.text)", .warning)
            }
        }
    }

    /// 「新建会话」那一页换了选择：上次没起成的原因不说了
    func clearLaunchFailure(_ peer: Peer) {
        launchFailure[peer.pid] = nil
    }

    /// 请那台电脑照模板起一个会话（多大见 TerminalGrid）；account：挑的账号（那台电脑的 cca 让挑时，见 LaunchSheet；旧版的 cca 是 nil：
    /// 用模板自己定的）。起来了：关掉「新建会话」那一页，打开新会话的终端（fresh：服务器上可能还没有它），回 nil；没起成：回原因码和
    /// 要说的那句话，列表过时了的（模板变了、没了，不让启动了，账号没了、改走别的 Team 了，要挑账号了，启动器不在了）顺手刷新。
    /// 终端已经开满了的不起（起了也打不开它的终端）
    func launch(_ t: LaunchTemplate, on peer: Peer, as account: LaunchAccount?, line: String?) async -> (code: String, text: String)? {
        guard let l = launcher(peer) else { return ("no_session", Texts.launchUnreached("no_session")) }
        guard terminals.count < Self.terminalLimit else { return ("terminals", Texts.terminalsFull(launch: true)) }
        let size = TerminalGrid.forLaunch
        let b = backend
        let r = await b.launch(peer: peer, launcher: l, template: t, account: account, cols: size.cols, rows: size.rows, line: line)
        guard b === backend else { return nil }   // 中途切了演示
        switch r {
        case .launched(let sid):
            let info = SessionInfo(machine: l.info.machine, tool: t.tool, name: account?.name ?? t.account, cwd: t.dir,
                                   since: Date().timeIntervalSince1970, viewOnly: false, aid: account?.aid ?? t.aid)
            if launchFor?.peer.pid == peer.pid { launchFor = nil }
            let s = TerminalSession(backend: b, peer: peer, session: RemoteSession(sid: sid, info: info, online: true, viewers: 0),
                                    readOnly: false, fresh: true)
            terminals.append(s)
            show(s)
            // 起的时候还不知道宿主给这个会话的说明（能不能在手机上结束：说明里的 end）：接上以后查一次列表（看着终端时不定时查）
            Task {
                for _ in 0..<90 where !s.isOpen && !s.isClosed {
                    try? await Task.sleep(for: .milliseconds(500))
                }
                guard s.isOpen, b === backend else { return }
                await fetch()
            }
            return nil
        case .refused(let code, let text):
            if ["changed", "no_template", "denied", "other_hub", "no_account", "need_account"].contains(code) { refresh() }
            return (code, Texts.launchFailure(code: code, text: text, picked: account != nil))
        case .failed(let f):
            if f.code == "no_session" || f.code == "nohost" { refresh() }
            return (f.code, Texts.launchUnreached(f.code))
        }
    }

    // MARK: 终端

    /// 接入一个会话，显示它的终端（推到现在这一页的导航里；账号页里点的，返回时回到账号页），别的终端照样接着。和 Mac App 的
    /// attachRemote 一样：这个会话已经有一个还接着的终端、要的又是同一种接法（操作 / 只看）的，换过去就行，不再接一次；它的终端已经
    /// 断开、结束了，或者要换一种接法：原地换成新的（原来的断开）。开满了（terminalLimit）说一声、不接
    func attach(_ s: RemoteSession, on peer: Peer, readOnly: Bool) {
        let viewing = readOnly || s.info.viewOnly
        let old = openTerminal(for: s.sid, on: peer.pid)
        if let old, !old.isClosed, old.readOnly == viewing {
            old.adopt(s)
            show(old)
            return
        }
        if old == nil, terminals.count >= Self.terminalLimit {
            showToast(Texts.terminalsFull(launch: false), .warning)
            return
        }
        let t = TerminalSession(backend: backend, peer: peer, session: s, readOnly: viewing)
        if let old, let i = terminals.firstIndex(where: { $0 === old }) {
            old.close()
            terminals[i] = t
        } else {
            terminals.append(t)
        }
        show(t)
    }

    /// 这个会话在 App 里开着的终端（断开了、结束了但还没关掉的也算）
    func openTerminal(for sid: String, on pid: String) -> TerminalSession? {
        terminals.first { $0.peer.pid == pid && $0.session.sid == sid }
    }

    /// 显示一个开着的终端：推到现在这一页的导航里（显示着别的终端时换成它）
    func show(_ t: TerminalSession) {
        guard terminals.contains(where: { $0 === t }) else { return }
        terminalTab = tab
        terminal = t
        t.nudge()
    }

    /// 从终端返回：回到列表，终端照样接着
    func hideTerminal() {
        terminal = nil
    }

    /// 断开并关掉一个终端（不说是哪个就是显示着的那个；会话在电脑上照常运行），显示着的退回列表。会话已经结束了的，关掉时查一次列表
    func closeTerminal(_ which: TerminalSession? = nil) {
        guard let t = which ?? terminal, let i = terminals.firstIndex(where: { $0 === t }) else { return }
        let ended = t.isClosed
        t.close()
        terminals.remove(at: i)
        if terminal === t { terminal = nil }
        if ended { refresh() }
    }

    private func closeAllTerminals() {
        for t in terminals { t.close() }
        terminals = []
        terminal = nil
    }

    /// 结束一个远程启动的会话（列表里的「结束会话」，先问过一句）：开着它能操作、接着的终端时用那个终端结束；不然在后台接上它（不显示，
    /// 照远程启动的大小），接上了就结束、断开。20 秒还没结束成（电脑没回应、接不上）说一声
    func end(_ s: RemoteSession, on peer: Peer) {
        guard s.info.canEnd, !endingSessions.contains(s.sid) else { return }
        if let t = openTerminal(for: s.sid, on: peer.pid), t.canEnd, t.isOpen {
            t.end()
            return
        }
        endingSessions.insert(s.sid)
        let b = backend
        let t = TerminalSession(backend: b, peer: peer, session: s, readOnly: false)
        let size = TerminalGrid.forLaunch
        t.sized(cols: size.cols, rows: size.rows)
        Task {
            for _ in 0..<100 {
                if !t.ending, t.isOpen { t.end() }
                if t.isClosed || b !== backend { break }
                try? await Task.sleep(for: .milliseconds(200))
            }
            var why: String?
            if case .closed(let w) = t.state { why = w }
            let done = t.ending || why == "ended" || why == "gone"
            t.close()
            guard b === backend else { return }
            endingSessions.remove(s.sid)
            refresh()
            if done {
                showToast("已结束「\(Texts.sessionTitle(s))」", .success)
            } else {
                showToast("没能结束「\(Texts.sessionTitle(s))」：" + (why.map(Texts.closed) ?? "电脑没有响应，请稍后再试"), .warning)
            }
        }
    }

    /// 点了通知：回到那个会话的终端。开着它的终端（还接着的）就换过去；没开着、断了的接上（会话还在、那台电脑在线时）
    func reopen(peer pid: String, sid: String, readOnly: Bool) {
        if let t = openTerminal(for: sid, on: pid), !t.isClosed {
            show(t)
            return
        }
        let b = backend
        Task {
            // 刚启动（点通知把 App 打开的）时列表还没查过、正在查：等一等
            for i in 0..<5 {
                guard b === backend else { return }
                if let p = peers.first(where: { $0.pid == pid }), let s = listings[pid]?.sessions.first(where: { $0.sid == sid }) {
                    if s.attachable { attach(s, on: p, readOnly: readOnly) }
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
        for t in terminals { t.nudge() }
        updatePolling()   // 没在看终端：马上查一次
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
    /// 接着的会话（列表里新查到的说明照 adopt 换上：名字、能不能结束……）
    private(set) var session: RemoteSession
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

    /// 列表里新查到的这个会话的说明（同一个会话）：换上
    func adopt(_ s: RemoteSession) {
        guard s.sid == session.sid, s != session else { return }
        session = s
    }

    /// 这台手机结束得了这个会话（远程启动的、能操作的：宿主在会话的说明里说的；旧版的宿主不说）
    var canEnd: Bool { !readOnly && session.info.canEnd }
    /// 点了「结束会话」、送出去了：结束时不说退出码（是这台手机让它结束的）
    private(set) var ending = false

    /// 结束会话：宿主照关掉它的终端那样结束里面的程序，之后和会话自己退出一样（退出码、连接关上）
    func end() {
        guard canEnd, isOpen, let link, link.end() else { return }
        ending = true
    }

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
        ending = false
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
