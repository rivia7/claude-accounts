import Foundation
import Observation

/// 侧栏选中的东西。
enum SidebarItem: Hashable, Sendable {
    case account(AccountRef)
    case routing

    var account: AccountRef? { if case .account(let r) = self { return r }; return nil }
}

enum ActivityKind: Sendable, Equatable {
    case app, web, check, sync, rename
}

/// 一个账号正在做的事（按钮旁边显示进度）。
struct Activity: Sendable, Equatable {
    var kind: ActivityKind
    var step: StepCode?
    var started = Date()
}

struct Toast: Identifiable, Equatable {
    enum Style: Equatable { case info, success, warning }
    let id = UUID()
    var text: String
    var style: Style
}

struct FailureInfo: Equatable, Identifiable {
    let id = UUID()
    var title: String
    var failure: Failure
}

/// 主窗口上的弹层：终端交接、失败说明。
enum Sheet: Identifiable, Equatable {
    case handoff(Handoff)
    case failure(FailureInfo)

    var id: String {
        switch self {
        case .handoff(let h): return "handoff:" + h.id
        case .failure(let f): return "failure:" + f.id.uuidString
        }
    }
}

/// 记住选择用的键值存储：App 里是 UserDefaults，测试里是内存（不碰 ~/Library/Preferences）。
protocol PrefsStore: AnyObject {
    func string(forKey key: String) -> String?
    func bool(forKey key: String) -> Bool
    func set(_ value: Any?, forKey key: String)
}

extension UserDefaults: PrefsStore {}

final class MemoryPrefs: PrefsStore {
    private(set) var values: [String: Any] = [:]
    init() {}
    func string(forKey key: String) -> String? { values[key] as? String }
    func bool(forKey key: String) -> Bool { values[key] as? Bool ?? false }
    func set(_ value: Any?, forKey key: String) { values[key] = value }
}

/// AppKit 那边提供的动作（测试里换成记录用的假实现）。
struct AppHooks {
    var prompter: Prompter
    var openFile: @MainActor (URL) -> Void = { _ in }
    var openURL: @MainActor (URL) -> Void = { _ in }
    var copy: @MainActor (String) -> Void = { _ in }
    var showMainWindow: @MainActor () -> Void = {}
    var terminate: @MainActor () -> Void = {}
    /// 读屏软件播报（提示条出现时）。
    var announce: @MainActor (String) -> Void = { _ in }
}

/// 整个 App 的状态（主窗口和菜单栏面板共用一份）。所有界面状态都在这里（这套工具链里 @State 用不了）。
@MainActor @Observable
final class AppModel {
    enum Phase: Equatable {
        case starting
        /// 找不到 cca：显示安装命令。
        case missingCca
        /// cca 还没装好（hello.installed == false）。
        case notInstalled
        /// cca 的接口版本不在 App 支持的范围（或者 cca 太旧，没有 `_gui`）。
        case unsupported(api: Int?)
        case ready
        case failed(String)
    }

    // MARK: 连接

    private(set) var phase: Phase = .starting
    private(set) var hello: HelloDoc?
    private(set) var ccaPath: String?
    let appVersion: String
    let isFixtureMode: Bool

    // MARK: 数据

    private(set) var state: StateDoc?
    private(set) var stateError: String?
    private(set) var stateLoading = false
    private(set) var live: LiveDoc?
    private(set) var liveError: String?
    private(set) var liveLoading = false
    /// 这次 App 里 `usage --json` 查到的（工具 → 键 → 条目）。
    private(set) var usageRuns: [ToolID: [String: UsageEntry]] = [:]
    private(set) var usageRefreshing: Set<ToolID> = []
    private(set) var usageRefreshingKeys: Set<AccountRef> = []
    /// 后台自动刷新用量时遇到的、需要用户同意的事（不弹框，显示一条提示，点了再手动刷新）。
    private(set) var usageNeedsConsent: [ToolID: [ConsentEvent]] = [:]
    /// 后台自动刷新用量失败（不弹框，显示一条可以重试的提示）。
    private(set) var usageFailure: [ToolID: Failure] = [:]
    private(set) var activities: [AccountRef: Activity] = [:]
    private(set) var syncActivity: Activity?
    /// 「更新并重新打开」的第一步（`cca gui update`）正在跑。
    private(set) var updating = false
    var sheet: Sheet?
    private(set) var toast: Toast?

    // MARK: 选择（记在 UserDefaults 里）

    var tool: ToolID = .claude {
        didSet {
            guard tool != oldValue else { return }
            defaults.set(tool.rawValue, forKey: Prefs.tool)
            if selection != .routing { selection = defaultSelection(for: tool) }
        }
    }

    var selection: SidebarItem? {
        didSet {
            guard selection != oldValue else { return }
            switch selection {
            case .account(let r)?:
                defaults.set(r.key, forKey: Prefs.selected(r.tool))
                defaults.set(false, forKey: Prefs.routing)
            case .routing?:
                defaults.set(true, forKey: Prefs.routing)
            case nil:
                break
            }
            if let e = aliasEditing, selection != .account(e) { cancelRename() }
            visibilityChanged(wasVisible: isVisible, wasRouting: oldValue == .routing && mainVisible)
        }
    }

    /// 分流页看哪一边：nil 按 route_pref，否则用 CCA_ROUTE 指定。
    var liveRoute: Route? {
        didSet {
            guard liveRoute != oldValue else { return }
            live = nil
            refreshLive(restart: true)
        }
    }

    // MARK: 可见性

    private(set) var mainVisible = false
    private(set) var panelVisible = false
    var isVisible: Bool { mainVisible || panelVisible }
    var routingVisible: Bool { mainVisible && selection == .routing }

    // MARK: 内部

    @ObservationIgnored var hooks: AppHooks
    @ObservationIgnored let defaults: PrefsStore
    @ObservationIgnored private let environment: [String: String]
    @ObservationIgnored private let homeDir: String
    @ObservationIgnored private var transport: Transport?
    /// 测试注入的 transport：绝不自己去找、换成真的 cca。
    @ObservationIgnored private let injected: Bool
    @ObservationIgnored private(set) var client: CcaClient?
    @ObservationIgnored let scheduler: Scheduler
    @ObservationIgnored private var watcher: CcaWatcher?
    @ObservationIgnored private var dirty = false
    /// 看不见时 cca 本身换了（bin/cca）：看得见了再跑 hello。
    @ObservationIgnored private var helloDirty = false
    @ObservationIgnored private var stateTask: Task<Void, Never>?
    @ObservationIgnored private var stateAgain = false
    @ObservationIgnored private var connectTask: Task<Void, Never>?
    @ObservationIgnored private var liveTask: Task<Void, Never>?
    /// 每次查 live 加一：被取消、被取代的查询不改状态。
    @ObservationIgnored private var liveGeneration = 0
    @ObservationIgnored private var appsDebounce: Task<Void, Never>?
    @ObservationIgnored private var toastTask: Task<Void, Never>?
    /// 退出时要取消的任务（usage / check / live / gui update 的子进程）。app、web、router sync 不在这里：
    /// 它们的输出接文件，App 退出后照样跑完（PROTOCOL 1）。
    @ObservationIgnored private var cancelOnQuit: [UUID: Task<Void, Never>] = [:]
    @ObservationIgnored private var startedOnce = false

    enum Prefs {
        static let tool = "tool"
        static let routing = "selection.routing"
        static func selected(_ t: ToolID) -> String { "selection.\(t.rawValue)" }
    }

    /// transport 给了就用它（测试）；否则 CCA_GUI_FIXTURES 有值时用演示数据，不运行 cca；再否则找 cca。
    init(environment: [String: String] = ProcessInfo.processInfo.environment, home: String = NSHomeDirectory(),
         appVersion: String, defaults: PrefsStore = UserDefaults.standard, hooks: AppHooks, transport: Transport? = nil,
         scheduler: Scheduler? = nil) {
        self.environment = environment
        self.homeDir = home
        self.appVersion = appVersion
        self.defaults = defaults
        self.hooks = hooks
        self.scheduler = scheduler ?? Scheduler()
        if let transport {
            self.transport = transport
            isFixtureMode = transport.isFixture
            injected = true
        } else if let dir = environment["CCA_GUI_FIXTURES"], !dir.isEmpty {
            let url = URL(fileURLWithPath: CcaLocator.expandTilde(dir, home: home))
            self.transport = FixtureTransport(directory: url, environment: environment)
            isFixtureMode = true
            injected = false
        } else {
            isFixtureMode = false
            injected = false
        }
        if let transport = self.transport { client = CcaClient(transport: transport) }
        if let t = defaults.string(forKey: Prefs.tool) { tool = ToolID(rawValue: t) }
        self.scheduler.onUsageTick = { [weak self] in self?.refreshUsage(manual: false) }
        self.scheduler.onLiveTick = { [weak self] in self?.refreshLive() }
    }

    // MARK: 启动和连接

    /// 启动：找 cca → `_gui hello` → `_gui state` → 看得见的话 `_gui live` 和用量。
    func start() async {
        guard !startedOnce else { return }
        startedOnce = true
        await connect()
        if phase == .ready, isVisible {
            refreshLive()
            refreshUsageIfNeeded()
        }
    }

    /// 重新跑 `_gui hello` 和 `_gui state`（App 被激活、cca 换了版本时）。并发调用合并成一次。
    func connect() async {
        if let t = connectTask {
            await t.value
            return
        }
        let t = Task { [weak self] in
            guard let self else { return }
            await self.connectOnce()
            self.connectTask = nil
        }
        connectTask = t
        await t.value
    }

    /// 演示模式和测试注入的 transport 都不找 cca。
    private var canLocate: Bool { !isFixtureMode && !injected }

    private func connectOnce() async {
        if canLocate && (transport == nil || ccaPath.map { !CcaLocator.isExecutableFile($0) } ?? true) {
            locate()
        }
        guard let client else {
            phase = .missingCca
            return
        }
        do {
            let h = try await client.hello()
            await apply(hello: h)
        } catch {
            // 已经在用了（读到过 state）又偶尔出错：保留界面，显示一条可以重试的提示
            let wasReady = phase == .ready && state != nil
            let message: String
            switch error {
            case CcaError.launch:
                if canLocate { locate() }
                if transport == nil {
                    phase = .missingCca
                    return
                }
                message = "没能运行 cca"
            case CcaError.timedOut:
                message = "cca 没有响应（_gui hello 超时）"
            default:
                // 旧版 cca 没有 `_gui`：退出码非 0 或者输出不是 JSON
                if !wasReady {
                    phase = .unsupported(api: nil)
                    return
                }
                message = "cca 出错了：\(error)"
            }
            if wasReady {
                stateError = message
            } else {
                phase = .failed(message)
            }
            return
        }
        if phase == .ready { await loadState() }
    }

    /// 找 cca（演示模式、测试注入时不找）。
    private func locate() {
        guard canLocate else { return }
        if let p = CcaLocator.locate(environment: environment, home: homeDir) {
            if p != ccaPath || transport == nil {
                ccaPath = p
                let t = ProcessTransport(ccaPath: p, baseEnvironment: environment, home: homeDir)
                transport = t
                client = CcaClient(transport: t)
            }
        } else {
            ccaPath = nil
            transport = nil
            client = nil
        }
    }

    private func apply(hello h: HelloDoc) async {
        let homeChanged = h.home != hello?.home
        hello = h
        if !GUIAPI.isSupported(h.api) {
            phase = .unsupported(api: h.api)
        } else if !h.installed {
            phase = .notInstalled
        } else {
            phase = .ready
        }
        await client?.setCodexEnabled(h.codexEnabled)
        if !h.codexEnabled && tool != .claude { tool = .claude }
        if homeChanged { restartWatcher() }
    }

    private func restartWatcher() {
        watcher?.stop()
        watcher = nil
        guard canLocate, let home = hello?.home, !home.isEmpty else { return }
        let w = CcaWatcher(home: home) { [weak self] trigger in
            Task { @MainActor in self?.watcherFired(trigger) }
        }
        w.start()
        watcher = w
    }

    // MARK: 读取

    func refreshState() {
        Task { await loadState() }
    }

    /// 跑一次 `_gui state`；正在跑时等它跑完再跑一次（保证拿到的是这之后的状态）。
    func loadState() async {
        if let t = stateTask {
            stateAgain = true
            await t.value
            return
        }
        let t = Task { [weak self] in
            guard let self else { return }
            repeat {
                self.stateAgain = false
                await self.fetchStateOnce()
            } while self.stateAgain
            self.stateTask = nil
        }
        stateTask = t
        await t.value
    }

    private func fetchStateOnce() async {
        guard let client, phase == .ready else { return }
        stateLoading = true
        defer { stateLoading = false }
        do {
            let s = try await client.state()
            state = s
            stateError = nil
            fixSelection()
        } catch {
            stateError = String(describing: error)
        }
    }

    /// `_gui live`（同一时间只跑一个；restart 时取消正在跑的、重新查）。
    func refreshLive(restart: Bool = false) {
        guard phase == .ready, let client else { return }
        if liveTask != nil {
            guard restart else { return }
            liveTask?.cancel()
            liveTask = nil
        }
        guard routerConfigured || localConfigured || state == nil else {
            live = nil
            return
        }
        liveLoading = true
        liveGeneration += 1
        let gen = liveGeneration
        let route = liveRoute
        liveTask = track { [weak self] in
            var result: LiveDoc?
            var failure: String?
            do {
                result = try await client.live(route: route)
            } catch CcaError.cancelled {
            } catch {
                failure = String(describing: error)
            }
            guard let self, self.liveGeneration == gen else { return }
            if let result {
                self.live = result
                self.liveError = nil
            } else if let failure {
                self.liveError = failure
            }
            self.liveLoading = false
            self.liveTask = nil
        }
    }

    /// 登记一个退出时要取消的任务（usage / check / live / gui update），跑完自己注销。
    @discardableResult
    private func track(_ body: @escaping @MainActor () async -> Void) -> Task<Void, Never> {
        let id = UUID()
        let t = Task { @MainActor [weak self] in
            await body()
            self?.cancelOnQuit[id] = nil
        }
        cancelOnQuit[id] = t
        return t
    }

    /// 工具栏的“刷新”：重新读 state，查用量（要同意的事弹框问），分流页开着时也查 live。
    func refreshAll() {
        Task {
            await loadState()
            refreshUsage(manual: true)
            if routingVisible || selection == .routing { refreshLive() }
        }
    }

    // MARK: 用量

    /// 账号该显示的用量：`_gui state` 的缓存和这次 App 里查到的取新的那份（一样新时用查到的，它带着这次的出错原因）。
    /// 只给登录着的官方账号（和 `_gui state` 的规则一样）。
    func usage(for a: Account) -> UsageEntry? {
        guard a.kind != .api, a.login == .loggedIn else { return nil }
        let cached = a.usage
        guard let run = usageRuns[a.tool]?[a.key] else { return cached }
        guard let c = cached else { return run }
        if (c.fetchedEpoch ?? 0) > (run.fetchedEpoch ?? 0) { return c }
        return run
    }

    /// 看得见时：有账号没有缓存、数据过期或者 5 分钟没更新，就刷新。
    func refreshUsageIfNeeded() {
        let now = Date().timeIntervalSince1970
        for t in visibleTools {
            let items = accounts(t).map { (account: $0, usage: usage(for: $0)) }
            if Attention.usageNeedsRefresh(items, now: now) { refreshUsage(tool: t, manual: false) }
        }
    }

    /// 刷新用量。manual 为真时（用户点的）要同意的事弹框问；自动刷新不弹框，只记下来显示一条提示。
    /// key 为 nil 时查这个工具的全部账号；tool 为 nil 时 Claude 和 ChatGPT 一起（各走各的闸门，并行）。
    func refreshUsage(tool only: ToolID? = nil, key: String? = nil, manual: Bool) {
        guard phase == .ready, let client else { return }
        let tools = only.map { [$0] } ?? visibleTools
        for t in tools {
            guard toolState(t)?.enabled ?? (t == .claude), accounts(t).contains(where: { $0.can.usage }) else { continue }
            if let key {
                usageRefreshingKeys.insert(AccountRef(tool: t, key: key))
            } else {
                if usageRefreshing.contains(t) { continue }
                usageRefreshing.insert(t)
            }
            let prompter: Prompter? = manual ? hooks.prompter : nil
            track { [weak self] in
                let (doc, r) = await client.document(.usage(t, key: key), as: UsageDoc.self, prompter: prompter)
                guard let self else { return }
                if let key {
                    self.usageRefreshingKeys.remove(AccountRef(tool: t, key: key))
                } else {
                    self.usageRefreshing.remove(t)
                }
                if let doc {
                    var entries = self.usageRuns[t] ?? [:]
                    for e in doc.accounts { if let k = e.key ?? e.account { entries[k] = e } }
                    self.usageRuns[t] = entries
                }
                switch r.status {
                case .success:
                    self.usageNeedsConsent[t] = nil
                    self.usageFailure[t] = nil
                case .needsConsent(let cs):
                    self.usageNeedsConsent[t] = cs
                case .refused:
                    self.showToast("没有同意，没有做任何改动", .info)
                case .terminal(let e):
                    if manual { self.present(.handoff(Handoff(event: e))) }
                case .failed(let f):
                    if f.reason == .cancelled { break }
                    if manual {
                        self.usageFailure[t] = nil   // 已经弹框说过了，不再留着后台那一条
                        self.present(.failure(FailureInfo(title: "没能刷新用量", failure: f)))
                    } else {
                        self.usageFailure[t] = f
                    }
                }
                await self.loadState()
            }
        }
    }

    // MARK: 动作

    func openApp(_ ref: AccountRef) {
        launch(ref, kind: .app, command: .app(ref.tool, key: ref.key), title: "没能打开 \(appLabel(ref.tool))")
    }

    func openWeb(_ ref: AccountRef) {
        launch(ref, kind: .web, command: .web(ref.tool, key: ref.key), title: "没能打开网页版")
    }

    private func launch(_ ref: AccountRef, kind: ActivityKind, command: CcaCommand, title: String) {
        guard phase == .ready, let client, activities[ref] == nil else { return }
        activities[ref] = Activity(kind: kind)
        let prompter = hooks.prompter
        Task { [weak self] in
            let r = await client.execute(command, prompter: prompter, onEvent: { ev in
                Task { @MainActor in self?.progress(ref, ev) }
            })
            guard let self else { return }
            self.activities[ref] = nil
            self.finish(r, failureTitle: title,
                        success: r.resultEvent?.message ?? (kind == .web ? "已打开网页版" : "已打开 \(self.appLabel(ref.tool))"))
            await self.afterAction()
        }
    }

    /// 出口检测（`cca [codex] check KEY --json`）。
    func checkExit(_ ref: AccountRef) {
        guard phase == .ready, let client, activities[ref] == nil else { return }
        activities[ref] = Activity(kind: .check)
        let prompter = hooks.prompter
        track { [weak self] in
            let (doc, r) = await client.document(.check(ref.tool, key: ref.key), as: CheckDoc.self, prompter: prompter, onEvent: { ev in
                Task { @MainActor in self?.progress(ref, ev) }
            })
            guard let self else { return }
            self.activities[ref] = nil
            let health = doc?.accounts.first { $0.key == ref.key }?.health
            var text = nonEmpty(health?.message) ?? "检测完成"
            if let ips = doc?.sharedExitIPs, !ips.isEmpty { text += "（几个账号共用出口 IP：\(ips.joined(separator: "、"))）" }
            self.finish(r, failureTitle: "没能检测出口", success: text)
            await self.afterAction()
        }
    }

    /// `cca router sync`（同一时间一个）。
    func syncRouter() {
        guard phase == .ready, let client, syncActivity == nil else { return }
        syncActivity = Activity(kind: .sync)
        let prompter = hooks.prompter
        Task { [weak self] in
            let r = await client.execute(.routerSync, prompter: prompter, onEvent: { ev in
                Task { @MainActor in
                    if case .step(let s) = ev { self?.syncActivity?.step = s.code }
                }
            })
            guard let self else { return }
            self.syncActivity = nil
            self.finish(r, failureTitle: "没能同步路由器", success: "路由器已同步")
            await self.afterAction()
        }
    }

    private func progress(_ ref: AccountRef, _ ev: CcaEvent) {
        if case .step(let s) = ev, activities[ref] != nil { activities[ref]?.step = s.code }
    }

    /// 按重跑流程的结果提示（PROTOCOL 4.3）。
    private func finish(_ r: ExecResult, failureTitle: String, success: String) {
        switch r.status {
        case .success:
            showToast(success, .success)
        case .refused:
            showToast("没有同意，没有做任何改动", .info)
        case .needsConsent:
            showToast("需要你同意才能继续", .warning)
        case .terminal(let e):
            present(.handoff(Handoff(event: e)))
        case .failed(let f):
            if f.reason == .cancelled { return }
            present(.failure(FailureInfo(title: failureTitle, failure: f)))
        }
    }

    // MARK: 改别名

    /// 正在直接改哪个账号的别名（账号页的名字变成输入框）；换到别的账号时自动放弃。
    var aliasEditing: AccountRef?
    /// 改别名时 cca 说的原因（写在输入框下面）。
    private(set) var renameError: String?

    /// 开始改别名：选中这个账号，它的名字变成输入框。
    func beginRename(_ ref: AccountRef) {
        renameError = nil
        if tool != ref.tool { tool = ref.tool }
        selection = .account(ref)
        aliasEditing = ref
        hooks.showMainWindow()
    }

    func cancelRename() {
        aliasEditing = nil
        renameError = nil
    }

    /// 改别名（`cca [codex] rename KEY 新别名`；新别名空着就去掉别名、只留邮箱）。成功后输入框收起、选中改名后的账号
    /// （新的键是 cca 打在 stdout 上的，App 不自己拼）；cca 说不行时把原因写在输入框下面，接着改。
    func rename(_ ref: AccountRef, to alias: String) {
        guard phase == .ready, let client, activities[ref] == nil else { return }
        let wanted = alias.trimmingCharacters(in: .whitespacesAndNewlines)
        activities[ref] = Activity(kind: .rename)
        renameError = nil
        let prompter = hooks.prompter
        Task { [weak self] in
            let r = await client.execute(.rename(ref.tool, key: ref.key, alias: wanted.isEmpty ? "-" : wanted), prompter: prompter)
            guard let self else { return }
            self.activities[ref] = nil
            switch r.status {
            case .success:
                let out = String(decoding: r.last?.stdout ?? Data(), as: UTF8.self)
                let newKey = nonEmpty(out.split(whereSeparator: \.isNewline).last.map(String.init)) ?? ref.key
                await self.loadState()
                self.aliasEditing = nil
                if self.selection == .account(ref) { self.selection = .account(AccountRef(tool: ref.tool, key: newKey)) }
                self.showToast(newKey == ref.key ? "别名没有变" : "已改名：登录、资料和端口都不变", .success)
            case .failed(let f) where f.reason == .command:
                self.renameError = nonEmpty(f.message) ?? "没能改名"
            default:
                self.aliasEditing = nil
                self.finish(r, failureTitle: "没能改名", success: "已改名")
            }
        }
    }

    /// 每次操作之后重新读 state（分流页开着时也读 live）。
    private func afterAction() async {
        await loadState()
        if routingVisible { refreshLive() }
    }

    func present(_ s: Sheet) {
        sheet = s
        hooks.showMainWindow()
    }

    func dismissSheet() {
        sheet = nil
    }

    func showToast(_ text: String, _ style: Toast.Style) {
        let t = Toast(text: text, style: style)
        toast = t
        hooks.announce(text)
        toastTask?.cancel()
        toastTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 5_000_000_000)
            if !Task.isCancelled, self?.toast?.id == t.id { self?.toast = nil }
        }
    }

    // MARK: 终端交接、补救

    /// 在“终端”里运行一条 cca 命令（PROTOCOL 6）。
    func runInTerminal(_ h: Handoff) {
        if isFixtureMode {
            showToast("演示模式：不会打开终端（\(h.display)）", .info)
            return
        }
        guard let path = ccaPath else {
            showToast("找不到 cca", .warning)
            return
        }
        do {
            let url = try TerminalHandoff.write(h, ccaPath: path)
            hooks.openFile(url)
        } catch {
            showToast("没能写交接文件：\(error.localizedDescription)", .warning)
        }
    }

    func copy(_ text: String) {
        hooks.copy(text)
        showToast("已复制：\(text)", .info)
    }

    func apply(_ remedy: Remedy) {
        switch remedy {
        case .none: break
        case .routerSync: syncRouter()
        case .handoff(let h): runInTerminal(h)
        case .openURL(let s): if let u = URL(string: s) { hooks.openURL(u) }
        }
    }

    /// 失败的补救按钮（交接命令用 state 里这个工具真正的前缀和账号名）。
    func remedy(for f: Failure) -> Remedy {
        guard f.reason == .command, let code = f.code else { return .none }
        let tool = f.tool ?? .claude
        let r = Remedy.forCode(code, tool: tool, key: f.key, prefix: prefix(tool))
        if case .handoff(var h) = r, code == .notLoggedIn || code == .loginMismatch, let key = f.key {
            h.title = "登录 \(account(AccountRef(tool: tool, key: key))?.displayName ?? key)"
            return .handoff(h)
        }
        if r == .routerSync && !routerConfigured { return .none }
        return r
    }

    func loginHandoff(_ a: Account) -> Handoff {
        .login(prefix: prefix(a.tool), key: a.key, name: a.displayName)
    }

    /// 「更新并重新打开」（PROTOCOL 5）：先照常运行 `cca gui update`，等它下载校验好新版本（失败就显示原因，App 照常开着）；
    /// 成功后脱离 App 运行 `cca gui update --relaunch <pid>`（它等 App 退出后换上新版本再打开），然后退出。
    func updateAndRelaunch() {
        if isFixtureMode {
            showToast("演示模式：不会更新", .info)
            return
        }
        guard let client, !updating else { return }
        updating = true
        let prompter = hooks.prompter
        track { [weak self] in
            let r = await client.execute(.guiUpdate, prompter: prompter)
            guard let self else { return }
            self.updating = false
            guard r.isSuccess else {
                self.finish(r, failureTitle: "没能下载新版本的 cca App", success: "")
                return
            }
            do {
                try await client.relaunchAfterUpdate(pid: ProcessInfo.processInfo.processIdentifier)
            } catch {
                let f = Failure(reason: .launch, code: nil, message: "没能运行 cca gui update --relaunch：\(error)", stderrTail: "",
                                exitCode: nil, tool: nil, key: nil)
                self.present(.failure(FailureInfo(title: "没能重新打开 cca App", failure: f)))
                return
            }
            self.shutdown()
            self.hooks.terminate()
        }
    }

    // MARK: 可见性、外部事件

    func setMainVisible(_ v: Bool) {
        guard v != mainVisible else { return }
        let was = isVisible, wasRouting = routingVisible
        mainVisible = v
        visibilityChanged(wasVisible: was, wasRouting: wasRouting)
    }

    func setPanelVisible(_ v: Bool) {
        guard v != panelVisible else { return }
        let was = isVisible, wasRouting = routingVisible
        panelVisible = v
        visibilityChanged(wasVisible: was, wasRouting: wasRouting)
    }

    private func visibilityChanged(wasVisible: Bool, wasRouting: Bool) {
        scheduler.update(visible: isVisible, routingVisible: routingVisible)
        guard phase == .ready else { return }
        if isVisible && !wasVisible {
            if helloDirty {
                helloDirty = false
                dirty = false
                Task { await connect() }
            } else if dirty {
                dirty = false
                refreshState()
            }
            refreshUsageIfNeeded()
        }
    }

    /// App 被激活（包括从“终端”回来）：重新跑 hello 和 state。
    func becameActive() {
        guard startedOnce else { return }
        Task { await connect() }
    }

    /// Claude App / ChatGPT App 启动或退出：稍等一下再读 state（多开时会连着来好几个通知）。
    func desktopAppsChanged() {
        guard phase == .ready else { return }
        appsDebounce?.cancel()
        appsDebounce = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 1_000_000_000)
            guard !Task.isCancelled, let self else { return }
            if self.isVisible { await self.loadState() } else { self.dirty = true }
        }
    }

    /// ~/.cca 变了（CcaWatcher）：看得见就重新读；看不见只记下，等看得见了再读。
    func watcherFired(_ t: CcaWatcher.Trigger) {
        guard isVisible else {
            dirty = true
            if t == .hello { helloDirty = true }
            return
        }
        switch t {
        case .hello: Task { await connect() }
        case .state: refreshState()
        }
    }

    /// 退出前：取消 usage / check / live / gui update 的子进程（SIGTERM 进程组）。app、web、router sync 不杀：
    /// 它们的输出接文件，App 退出后照样跑完。
    func shutdown() {
        scheduler.stop()
        watcher?.stop()
        for (_, t) in cancelOnQuit { t.cancel() }
        cancelOnQuit.removeAll()
    }

    // MARK: 给界面用的派生数据

    var codexEnabled: Bool { hello?.codexEnabled ?? false }

    var visibleTools: [ToolID] { codexEnabled ? [.claude, .codex] : [.claude] }

    func toolState(_ t: ToolID) -> ToolState? { state?.tool(t) }

    func accounts(_ t: ToolID) -> [Account] { toolState(t)?.accounts ?? [] }

    func account(_ ref: AccountRef) -> Account? { state?.account(ref) }

    var selectedAccount: Account? { selection?.account.flatMap { account($0) } }

    /// 交接命令的前缀：`_gui state` 给的 cli_prefix，没有就用默认的。
    func prefix(_ t: ToolID) -> [String] { toolState(t)?.cliPrefix ?? t.defaultPrefix }

    func appLabel(_ t: ToolID) -> String { nonEmpty(toolState(t)?.appLabel) ?? (t == .codex ? "ChatGPT App" : "Claude App") }

    var routerConfigured: Bool { state?.config?.routerConfigured ?? false }
    var localConfigured: Bool { state?.config?.localConfigured ?? false }

    func liveSlot(for ref: AccountRef) -> LiveSlot? { live?.slot(for: ref) }

    func activity(_ ref: AccountRef) -> Activity? { activities[ref] }

    func isUsageRefreshing(_ ref: AccountRef) -> Bool {
        usageRefreshing.contains(ref.tool) || usageRefreshingKeys.contains(ref)
    }

    /// cca 和 App 的版本号不同（提示“更新并重新打开”）。
    var versionMismatch: Bool {
        guard let v = hello?.ccaVersion, !v.isEmpty else { return false }
        return v != appVersion
    }

    /// 菜单栏图标要不要带标记。
    var needsAttention: Bool {
        let now = Date().timeIntervalSince1970
        let usages = visibleTools.flatMap { t in accounts(t).compactMap { usage(for: $0) } }
        return Attention.needed(sync: state?.routing?.sync, live: live, usages: usages, now: now)
    }

    /// 账号表校验不过的工具（启动会失败：提示在终端里 `cca [codex] edit`）。
    var invalidTables: [(tool: ToolID, issues: [StateIssue])] {
        visibleTools.compactMap { t in
            guard let issues = toolState(t)?.tableInvalid, !issues.isEmpty else { return nil }
            return (t, issues)
        }
    }

    // MARK: 选择

    private func defaultSelection(for t: ToolID) -> SidebarItem? {
        let list = accounts(t)
        if let k = defaults.string(forKey: Prefs.selected(t)), list.contains(where: { $0.key == k }) {
            return .account(AccountRef(tool: t, key: k))
        }
        if let last = toolState(t)?.last, list.contains(where: { $0.key == last }) {
            return .account(AccountRef(tool: t, key: last))
        }
        return list.first.map { .account($0.ref) }
    }

    /// state 变了之后：选中的账号不在了就换一个；第一次加载时恢复上次的选择。
    private func fixSelection() {
        if !visibleTools.contains(tool) { tool = .claude }
        switch selection {
        case .routing?:
            return
        case .account(let r)?:
            if r.tool == tool, account(r) != nil { return }
            selection = defaultSelection(for: tool)
        case nil:
            if defaults.bool(forKey: Prefs.routing) {
                selection = .routing
            } else {
                selection = defaultSelection(for: tool)
            }
        }
    }
}
