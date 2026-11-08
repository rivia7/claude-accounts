import Foundation
import Observation

/// 侧栏选中的东西：办公室（首页，所有账号一眼看全）、某个账号、节点分流。
enum SidebarItem: Hashable, Sendable {
    case office
    case account(AccountRef)
    case routing
    case clientUpdates

    var account: AccountRef? { if case .account(let r) = self { return r }; return nil }
}

enum ActivityKind: Sendable, Equatable {
    case app, web, check, sync, rename, reset
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
    func double(forKey key: String) -> Double
    func set(_ value: Any?, forKey key: String)
}

extension UserDefaults: PrefsStore {}

final class MemoryPrefs: PrefsStore {
    private(set) var values: [String: Any] = [:]
    init() {}
    func string(forKey key: String) -> String? { values[key] as? String }
    func bool(forKey key: String) -> Bool { values[key] as? Bool ?? false }
    func double(forKey key: String) -> Double { values[key] as? Double ?? 0 }
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
    /// 「更新并重新打开」做到哪一步了：先把 cca 换成检查更新找到的新版本（`cca update 版本号`），再下载和它同版本的 App
    /// （`cca gui update`）；cca 已经比 App 新时只有后一步。
    enum UpdateStep: Equatable { case cca, app }
    private(set) var updateStep: UpdateStep?
    var updating: Bool { updateStep != nil }
    /// 最近一次检查更新的结果（`cca update --check --json`，PROTOCOL 5.1）
    private(set) var updateInfo: UpdateDoc?
    private(set) var updateChecking = false
    private(set) var clientUpdates: ClientUpdatesDoc?
    private(set) var clientUpdatesError: String?
    private(set) var clientUpdatesLoading = false
    enum ClientUpdateActivity: Equatable { case check, mode, install }
    private(set) var clientUpdateActivity: ClientUpdateActivity?
    var clientUpdatesBusy: Bool { clientUpdateActivity != nil || clientUpdatesLoading }
    /// 点过“以后再说”的版本（记在 App 的设置里）：这个版本不再提示，出了更新的照常提示
    private(set) var updateDismissed: String?
    /// 手动检查时正好有一次自动检查在跑：它跑完照样说一声
    @ObservationIgnored private var manualCheckPending = false
    /// 自动检查更新的间隔（查不了时 3 小时后再查）
    static let updateCheckInterval: Double = 86400
    /// 内置代理的内核（CoreHost，PROTOCOL 7）：退出了的时候界面上显示一条提示
    private(set) var coreStatus: CoreHost.Status = .off
    var sheet: Sheet?
    private(set) var toast: Toast?

    // MARK: 选择（记在 UserDefaults 里）

    var tool: ToolID = .claude {
        didSet {
            guard tool != oldValue else { return }
            defaults.set(tool.rawValue, forKey: Prefs.tool)
            // 办公室、分流页看的是所有工具：切工具只换侧栏的名册
            if selection?.account != nil { selection = defaultSelection(for: tool) }
        }
    }

    var selection: SidebarItem? {
        didSet {
            guard selection != oldValue else { return }
            if case .account(let r)? = selection { defaults.set(r.key, forKey: Prefs.selected(r.tool)) }
            if let e = aliasEditing, selection != .account(e) { cancelRename() }
            visibilityChanged(wasVisible: isVisible, wasRouting: oldValue == .routing && mainVisible)
            if selection == .office { refreshLiveIfStale() }
            if selection == .clientUpdates { refreshClientUpdates() }
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
    /// 看得见、并且用得上路由器实况的页面开着：分流页，或者办公室（端口连不上、选错节点的员工看它）
    var liveVisible: Bool { mainVisible && (selection == .routing || selection == .office) }

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
    /// 退出时要取消的任务（usage / check / live / 检查更新 / 更新 cca / gui update 的子进程）。app、web、router sync、重置额度
    /// 不在这里：它们的输出接文件，App 退出后照样跑完（PROTOCOL 1）。
    @ObservationIgnored private var cancelOnQuit: [UUID: Task<Void, Never>] = [:]
    @ObservationIgnored private var startedOnce = false
    @ObservationIgnored private var coreHost: CoreHost?

    enum Prefs {
        static let tool = "tool"
        /// 上次检查更新的时间（Unix 秒）：自动检查一天最多一次；查不了时记成 3 小时后再查
        static let updateCheckedAt = "update.checkedAt"
        /// 点过“以后再说”的版本
        static let updateDismissed = "update.dismissed"
        /// 各工具上次看的账号（侧栏换工具时选中它）
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
            // 演示数据的时间平移到现在（几点重置、几点回来才有意义）
            self.transport = FixtureTransport(directory: url, environment: environment, rebaseTo: Date().timeIntervalSince1970)
            isFixtureMode = true
            injected = false
        } else {
            isFixtureMode = false
            injected = false
        }
        if let transport = self.transport { client = CcaClient(transport: transport) }
        if let t = defaults.string(forKey: Prefs.tool) { tool = ToolID(rawValue: t) }
        updateDismissed = defaults.string(forKey: Prefs.updateDismissed)
        self.scheduler.onUsageTick = { [weak self] in self?.refreshUsage(manual: false) }
        self.scheduler.onLiveTick = { [weak self] in self?.refreshLive() }
        self.scheduler.onClientCheckTick = { [weak self] in self?.checkClientUpdates(manual: false) }
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
            checkForUpdatesIfDue()
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
        defer { reconcileClientChecks() }
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
        if phase == .ready {
            await loadState()
            await loadClientUpdates()
        }
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
        reconcileCore()
    }

    // MARK: 内置代理

    /// 内置代理（PROTOCOL 7）：state 里本机客户端是内置代理时让内核一直跑着（`cca _core run`），不是了就停掉。演示数据不跑。
    private func reconcileCore() {
        let want = !isFixtureMode && phase == .ready && state?.config?.local?.client == .builtin
        if want && coreHost == nil {
            let host = CoreHost(logPath: ProcessTransport.coreLogPath()) { [weak self] log, onExit in
                guard let t = self?.transport else { throw CcaError.launch("找不到 cca") }
                return try t.startCore(logPath: log, onExit: onExit)
            }
            host.onChange = { [weak self] s in self?.coreStatus = s }
            coreHost = host
        }
        coreHost?.setWanted(want)
    }

    /// 提示条上的“现在重启”：不等退避。
    func retryCore() {
        coreHost?.retryNow()
    }

    /// 内置代理的内核停了、等着重启：(退出码，没启动起来时是 -1；日志的最后几行)
    var coreTrouble: (code: Int32, detail: String)? {
        if case .waiting(_, let code, let detail) = coreStatus { return (code, detail) }
        return nil
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
            reconcileCore()
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

    /// 登记一个退出时要取消的任务（usage / check / live / 检查更新 / 更新），跑完自己注销。
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

    /// 工具栏的“刷新”：重新读 state，查用量（要同意的事弹框问），分流页、办公室开着时也查 live（端口连不上的员工要看它）。
    func refreshAll() {
        Task {
            await loadState()
            refreshUsage(manual: true)
            if selection == .routing || selection == .office { refreshLive() }
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
        show(ref)
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
            switch r.status {
            case .success:
                let out = String(decoding: r.last?.stdout ?? Data(), as: UTF8.self)
                let newKey = nonEmpty(out.split(whereSeparator: \.isNewline).last.map(String.init)) ?? ref.key
                // 读新的 state 时改名还算在进行：fixSelection 不会因为旧键没了就回办公室，读完再换成新键
                await self.loadState()
                self.activities[ref] = nil
                self.aliasEditing = nil
                if self.selection == .account(ref) { self.selection = .account(AccountRef(tool: ref.tool, key: newKey)) }
                self.showToast(newKey == ref.key ? "别名没有变" : "已改名：登录、资料和端口都不变", .success)
            case .failed(let f) where f.reason == .command:
                self.activities[ref] = nil
                self.renameError = nonEmpty(f.message) ?? "没能改名"
            default:
                self.activities[ref] = nil
                self.aliasEditing = nil
                self.finish(r, failureTitle: "没能改名", success: "已改名")
            }
        }
    }

    /// 每次操作之后重新读 state（分流页、办公室开着时也读 live：同步路由器之后工位要跟着好起来）。
    private func afterAction() async {
        await loadState()
        if liveVisible { refreshLive() }
    }

    /// 办公室打开、变得看得见时：上次的实况超过 2 分钟（或者还没有）就查一次。办公室不定时轮询（分流页才每 60 秒查）。
    func refreshLiveIfStale(maxAge: Double = 120) {
        guard phase == .ready, liveVisible else { return }
        if let at = live?.generatedAt, Date().timeIntervalSince1970 - at < maxAge { return }
        refreshLive()
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

    // MARK: 重置额度

    /// 重置额度（ChatGPT 账号，`cca codex reset KEY --json`，PROTOCOL 4.5）：cca 先要同意（reset_credit，默认不同意：
    /// 会用掉一次重置机会），同意后重置，带回重置后的用量。用量还不需要重置时 OpenAI 不扣次数，只提示一句。
    /// 同意之后就会用掉一次：跑完为止，App 退出也不杀（PROTOCOL 1），所以不登记到 cancelOnQuit。
    func resetQuota(_ ref: AccountRef) {
        guard phase == .ready, let client, activities[ref] == nil else { return }
        activities[ref] = Activity(kind: .reset)
        let prompter = hooks.prompter
        Task { [weak self] in
            let (doc, r) = await client.document(.reset(ref.tool, key: ref.key), as: ResetDoc.self, prompter: prompter)
            guard let self else { return }
            self.activities[ref] = nil
            if let u = doc?.usage, let k = u.key ?? u.account {
                self.usageRuns[ref.tool, default: [:]][k] = u
            }
            let text = nonEmpty(doc?.message) ?? "已重置额度"
            if r.isSuccess, doc?.outcome == .nothingToReset {
                self.showToast(text, .info)
            } else {
                self.finish(r, failureTitle: "没能重置额度", success: text)
            }
            await self.afterAction()
            // 重置成了却没读到重置后的用量：再查一次，不然页面上还是重置前的数
            if r.isSuccess, doc?.didReset == true, doc?.usage == nil {
                self.refreshUsage(tool: ref.tool, key: ref.key, manual: false)
            }
        }
    }

    // MARK: ChatGPT 客户端更新

    /// App 隐藏时也检查；是否到 24 小时由后端决定。检查绝不安装，开发和演示模式不自动运行。
    private func reconcileClientChecks() {
        scheduler.updateClientChecks(enabled: phase == .ready && !isFixtureMode && hello?.devMode != true)
    }

    func refreshClientUpdates() {
        track { [weak self] in await self?.loadClientUpdates() }
    }

    private func loadClientUpdates() async {
        guard phase == .ready, let client, !clientUpdatesBusy else { return }
        clientUpdatesLoading = true
        defer { clientUpdatesLoading = false }
        do {
            clientUpdates = try await client.clientUpdates()
            clientUpdatesError = nil
        } catch CcaError.cancelled {
        } catch {
            clientUpdatesError = "没能读取客户端更新状态：\(error)"
        }
    }

    func checkClientUpdates(manual: Bool) {
        if !manual {
            guard !isFixtureMode, hello?.devMode != true, clientUpdates?.mode == "auto" else { return }
        }
        runClientUpdate(.clientCheck(force: manual), activity: .check, manual: manual)
    }

    func setClientUpdateAutomatic(_ enabled: Bool) {
        guard !isFixtureMode else { showToast("演示模式：不会更改自动检查设置", .info); return }
        runClientUpdate(.clientMode(automatic: enabled), activity: .mode, manual: true)
    }

    func installClientUpdate(_ entry: ClientUpdateEntry) {
        guard !isFixtureMode else { showToast("演示模式：不会安装客户端更新", .info); return }
        // 从当前快照重取，不能使用页面尚未刷新的旧 token。
        guard let current = clientUpdates?.clients.first(where: { $0.id == entry.id }),
              current.readyToInstall, current.installToken == entry.installToken, let token = current.installToken else {
            showToast("目标版本或软件包校验值已变化，请重新检查后再安装", .warning)
            return
        }
        runClientUpdate(.clientInstall(token: token), activity: .install, manual: true)
    }

    private func runClientUpdate(_ command: CcaCommand, activity: ClientUpdateActivity, manual: Bool) {
        guard phase == .ready, let client, !clientUpdatesBusy else { return }
        clientUpdateActivity = activity
        clientUpdatesError = nil
        let prompter = manual ? hooks.prompter : nil
        let operation: @MainActor () async -> Void = { [weak self] in
            let (doc, result) = await client.document(command, as: ClientUpdatesDoc.self, prompter: prompter)
            guard let self else { return }
            self.clientUpdateActivity = nil
            if let doc {
                self.clientUpdates = doc
            } else if result.isSuccess {
                self.clientUpdatesError = "cca 没有返回有效的客户端更新状态，请重新检查。"
            } else if let failure = result.failure, failure.reason != .cancelled {
                self.clientUpdatesError = failure.message
            }
            if manual {
                let message: String
                if let entry = doc?.clients.first(where: { $0.status == "needs_recheck" }) {
                    message = entry.detail
                } else if activity == .mode {
                    message = doc?.mode == "auto" ? "已开启自动检查；安装前仍会询问" : "已关闭自动检查"
                } else if activity == .install {
                    message = doc?.clients.first?.detail ?? "客户端更新操作已完成"
                } else {
                    message = "客户端更新状态已刷新"
                }
                if result.isSuccess, doc?.clients.contains(where: { $0.status == "needs_recheck" }) == true {
                    self.showToast(message, .warning)
                } else if result.isSuccess, let error = self.clientUpdatesError {
                    self.showToast(error, .warning)
                } else {
                    self.finish(result, failureTitle: "客户端更新未完成", success: message)
                }
            }
            if activity == .install { await self.loadState() }
        }
        if command.runsToCompletion {
            Task { await operation() }   // 安装事务必须完成或回滚，退出 App 也不杀进程。
        } else {
            track(operation)
        }
    }

    // MARK: cca 自身的检查更新、更新

    /// 检查更新找到的新版本：比现在的 cca 新（按版本号的先后）、没点过“以后再说”、不是开发模式（看 hello）；cca 已经换成它了
    /// （顶上已经有“更新并重新打开”）就不再说。
    var availableUpdate: UpdateDoc? {
        guard hello?.devMode != true, let u = updateInfo, u.available, let v = nonEmpty(u.latest), v != updateDismissed,
              let latest = AppVersion(v) else { return nil }
        if let cur = hello?.ccaVersion.flatMap(AppVersion.init), !(latest > cur) { return nil }
        return u
    }

    /// 有新版本时它的版本号
    var availableVersion: String? { availableUpdate?.latest }

    /// 检查更新（`cca update --check --json`）：按现在的版本挑（测试版也看更新的测试版），cca 只检查不安装。
    /// manual 是用户点的「检查更新…」：结果都说一声（已是最新、有新版本、查不了，查不了的原因是 cca 的 fail 事件），
    /// 正好有一次自动检查在跑就等它的结果；自动检查只在有新版本时出现在窗口顶上，查不了不说、3 小时后再查。
    func checkForUpdates(manual: Bool) {
        guard phase == .ready, let client else { return }
        if manual { manualCheckPending = true }
        guard !updateChecking else { return }
        updateChecking = true
        track { [weak self] in
            let (doc, r) = await client.document(.updateCheck, as: UpdateDoc.self, prompter: nil)
            guard let self else { return }
            self.updateChecking = false
            let announce = self.manualCheckPending
            self.manualCheckPending = false
            let now = Date().timeIntervalSince1970
            if let u = doc, r.isSuccess {
                self.updateInfo = u
                self.defaults.set(now, forKey: Prefs.updateCheckedAt)
                if announce { self.announceUpdate(u) }
            } else {
                self.defaults.set(now - Self.updateCheckInterval + 3 * 3600, forKey: Prefs.updateCheckedAt)
                if announce, let f = r.failure, f.reason != .cancelled {
                    self.showToast("没能检查更新：\(f.message)", .warning)
                }
            }
        }
    }

    /// 手动检查的结果说一声；点过“以后再说”的版本这时也重新提示。
    private func announceUpdate(_ u: UpdateDoc) {
        if let d = updateDismissed, d == u.latest {
            updateDismissed = nil
            defaults.set(nil, forKey: Prefs.updateDismissed)
        }
        if hello?.devMode == true {
            showToast(u.available ? "有新版本 \(u.latest ?? "")：开发模式下在仓库里 git pull 更新" : "已是最新版本（开发模式）", .info)
        } else if let v = availableVersion {
            showToast("有新版本 \(v)：点窗口顶上的「更新并重新打开」", .info)
        } else {
            showToast("已是最新版本 \(hello?.ccaVersion ?? u.latest ?? "")", .success)
        }
    }

    /// 自动检查更新：窗口或菜单栏面板看得见、离上次检查超过一天（查不了时 3 小时）；关掉了自动更新（`cca update off`）、
    /// 开发模式、演示数据都不查。
    func checkForUpdatesIfDue(now: Date = Date()) {
        guard phase == .ready, isVisible, !isFixtureMode, hello?.devMode != true, let state, state.config?.update != "off" else { return }
        guard now.timeIntervalSince1970 - defaults.double(forKey: Prefs.updateCheckedAt) >= Self.updateCheckInterval else { return }
        checkForUpdates(manual: false)
    }

    /// 这个版本先不更新：不再提示它，出了更新的照常提示（手动「检查更新…」也会重新提示它）。
    func dismissUpdate() {
        guard let v = availableVersion else { return }
        updateDismissed = v
        defaults.set(v, forKey: Prefs.updateDismissed)
    }

    /// 开始「更新并重新打开」：演示模式只说一声；已经在更新时不再开始。
    private func beginUpdate(_ step: UpdateStep) -> CcaClient? {
        if isFixtureMode {
            showToast("演示模式：不会更新", .info)
            return nil
        }
        guard let client, updateStep == nil else { return nil }
        updateStep = step
        return client
    }

    /// 检查更新找到新版本时的「更新并重新打开」（PROTOCOL 5.1）：先 `cca update 版本号` 把 cca 换成新版本（失败就显示原因，
    /// 什么都没换），再照常下载同版本的 App、重新打开。
    func installUpdate() {
        guard let v = availableVersion, let client = beginUpdate(.cca) else { return }
        let prompter = hooks.prompter
        track { [weak self] in
            let r = await client.execute(.update(v), prompter: prompter)
            guard let self else { return }
            guard r.isSuccess else {
                self.updateStep = nil
                self.finish(r, failureTitle: "没能把 cca 更新到 \(v)", success: "")
                return
            }
            await self.connect()   // hello 换成新版本的 cca（App 的版本对不上了，顶上换成“更新并重新打开”的进度）
            await self.relaunchIntoNewApp()
        }
    }

    /// 「更新并重新打开」（cca 已经比 App 新，PROTOCOL 5）：先照常运行 `cca gui update`，等它下载校验好新版本（失败就显示原因，
    /// App 照常开着）；成功后脱离 App 运行 `cca gui update --relaunch <pid>`（它等 App 退出后换上新版本再打开），然后退出。
    func updateAndRelaunch() {
        guard beginUpdate(.app) != nil else { return }
        track { [weak self] in await self?.relaunchIntoNewApp() }
    }

    private func relaunchIntoNewApp() async {
        guard let client else {
            updateStep = nil
            return
        }
        updateStep = .app
        let r = await client.execute(.guiUpdate, prompter: hooks.prompter)
        updateStep = nil
        guard r.isSuccess else {
            finish(r, failureTitle: "没能下载新版本的 cca App", success: "")
            return
        }
        do {
            try await client.relaunchAfterUpdate(pid: ProcessInfo.processInfo.processIdentifier)
        } catch {
            let f = Failure(reason: .launch, code: nil, message: "没能运行 cca gui update --relaunch：\(error)", stderrTail: "",
                            exitCode: nil, tool: nil, key: nil)
            present(.failure(FailureInfo(title: "没能重新打开 cca App", failure: f)))
            return
        }
        shutdown()
        hooks.terminate()
    }

    // MARK: 可见性、外部事件

    func setMainVisible(_ v: Bool) {
        guard v != mainVisible else { return }
        let was = isVisible, wasRouting = routingVisible
        mainVisible = v
        visibilityChanged(wasVisible: was, wasRouting: wasRouting)
        if v, selection == .office { refreshLiveIfStale() }
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
            checkForUpdatesIfDue()
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
        // 内置代理的配置变了（或者 cca 碰了它一下）：cca 刚切到内置代理、在等内核（App 可能只在菜单栏里），看不见时也马上读 state；
        // 内核正在退避重启的话不再等
        if t == .core {
            Task {
                await loadState()
                coreHost?.retryNow()
            }
            return
        }
        guard isVisible else {
            dirty = true
            if t == .hello { helloDirty = true }
            return
        }
        switch t {
        case .hello: Task { await connect() }
        case .state, .core: refreshState()
        }
    }

    /// 退出前：取消 usage / check / live / 检查更新 / 更新的子进程（SIGTERM 进程组）。app、web、router sync、重置额度不杀：
    /// 它们的输出接文件，App 退出后照样跑完。内置代理的内核停掉并等它退出（App 托管它，App 退出它就停）。
    func shutdown() {
        coreHost?.shutdown()
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

    /// 在终端里添加这个组的账号（办公室各组的标题、工具栏的“＋”菜单）。
    func addHandoff(_ d: Employee.Department) -> Handoff { d.addHandoff(codexPrefix: prefix(.codex)) }

    /// 打开一个账号的页面（办公室的工位、分流图里的账号）：侧栏的工具跟着换过去。
    func show(_ ref: AccountRef) {
        if tool != ref.tool { tool = ref.tool }
        selection = .account(ref)
    }

    /// 办公室：按组排好的工位，每个账号此刻的样子（和账号页那句话用同一份用量、实况）。
    func office(now: Double) -> Office {
        Office.make(tools: state?.tools ?? [], codexEnabled: codexEnabled, usage: { self.usage(for: $0) },
                    live: { self.liveSlot(for: $0) }, now: now)
    }

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

    /// state 变了之后：选中的账号不在了就回办公室；第一次加载时打开办公室（首页）。
    private func fixSelection() {
        if !visibleTools.contains(tool) { tool = .claude }
        switch selection {
        case .office?, .routing?, .clientUpdates?:
            return
        case .account(let r)?:
            if r.tool == tool, account(r) != nil { return }
            // 正在改名：旧键没了是改名的结果，改名完成后会换成新键（rename）
            if activities[r]?.kind == .rename { return }
            selection = .office
        case nil:
            selection = .office
        }
    }
}
